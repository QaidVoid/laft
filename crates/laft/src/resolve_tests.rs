use super::*;

use crate::env::ARCH;
use laft_core::Timestamp;
use laft_index::Index;

fn request(text: &str, origin: &str) -> Request {
    Request {
        pkg: text.parse().expect("valid reference"),
        origin: origin.into(),
    }
}

fn texts(requests: &[Request]) -> Vec<String> {
    requests.iter().map(|r| r.pkg.to_string()).collect()
}

fn config() -> SystemConfig {
    let dir = laft_test::tempdir();
    std::fs::write(
        dir.path().join("system.stav"),
        "system {\n  packages = [ripgrep, llvm]\n  alternatives.vi = neovim\n  \
         fs \"/\" { device = \"UUID=1\", fstype = ext4 }\n  \
         fs \"/boot\" { device = \"UUID=2\", fstype = vfat }\n  \
         view legacy { packages = [llvm@19, openssl] }\n  \
         view bare { inherit = none, packages = [cmake] }\n}\n",
    )
    .expect("writable");
    laft_config::load(
        dir.path(),
        &|_| Ok(()),
        &laft_config::IndexOptions::default(),
    )
    .expect("valid")
    .config
}

#[test]
fn a_view_that_inherits_replaces_system_requests_of_the_same_name() {
    let config = config();
    let system = vec![request("ripgrep", "a"), request("llvm", "b")];
    let (requests, alternatives) =
        view_requests(&config, &"legacy".parse().expect("valid"), &system);
    assert_eq!(texts(&requests), ["ripgrep", "llvm@19", "openssl"]);
    assert_eq!(
        alternatives
            .get(&"vi".parse().expect("valid"))
            .map(Name::as_str),
        Some("neovim")
    );
    let (requests, alternatives) = view_requests(&config, &"bare".parse().expect("valid"), &system);
    assert_eq!(texts(&requests), ["cmake"]);
    assert!(alternatives.is_empty());
}

#[test]
fn the_system_requests_add_the_kernel_and_service_packages_once() {
    let config = config();
    let services = services(&config).expect("valid services");
    let requests = system_requests(&config, &services);
    assert_eq!(texts(&requests)[..3], ["llvm", "ripgrep", "linux"]);
    let kernel = requests.iter().find(|r| r.pkg.name().as_str() == "linux");
    assert_eq!(kernel.map(|r| r.origin.as_str()), Some("boot.kernel"));
    let names: Vec<&str> = requests.iter().map(|r| r.pkg.name().as_str()).collect();
    let unique: std::collections::BTreeSet<&&str> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
}

/// The metadata of one published package of `name`, with the recorded
/// option values `options`.
fn meta_of(
    name: &str,
    version: &str,
    options: &[(&str, laft_core::OptionValue)],
) -> laft_pkg::Meta {
    laft_pkg::Meta {
        format: laft_pkg::FORMAT,
        name: name.parse().expect("a name"),
        version: version.parse().expect("a version"),
        rev: 1,
        variant: Variant::default(),
        arch: ARCH,
        origin: name.parse().expect("a name"),
        summary: format!("The {name} test package"),
        homepage: None,
        license: vec!["MIT".to_owned()],
        depends: Vec::new(),
        provides: Vec::new(),
        conflicts: Vec::new(),
        replaces: Vec::new(),
        alternatives: Vec::new(),
        triggers: Vec::new(),
        static_deps: Vec::new(),
        abi: BTreeMap::new(),
        abi_epoch: 0,
        options: options
            .iter()
            .map(|(o, v)| ((*o).to_owned(), v.clone()))
            .collect(),
        requested_privileges: Vec::new(),
        installed_size: 0,
    }
}

/// The declaration of one bool option.
fn bool_decl(name: &str, default: bool) -> laft_index::OptionDecl {
    laft_index::OptionDecl {
        name: name.to_owned(),
        kind: laft_index::OptionType::Bool("bool".to_owned()),
        default: laft_core::OptionValue::Bool(default),
        doc: "d".to_owned(),
    }
}

/// The snapshot member of an index that publishes recipe snapshots.
fn snapshot() -> laft_index::RecipeSnapshot {
    laft_index::RecipeSnapshot {
        file: "recipes/aa.tar.zst".to_owned(),
        size: 1,
        blake3: laft_core::Blake3Hash::of(b"aa"),
    }
}

/// The parts of an index every buildable test needs.
fn index_base(repo: &str) -> Index {
    Index {
        format: Index::FORMAT,
        repo: repo.parse().expect("a name"),
        channel: "current".parse().expect("a name"),
        arch: ARCH,
        generation: 1,
        timestamp: Timestamp::now(),
        expires: laft_core::Timestamp::from_secs(
            laft_core::Timestamp::now().as_secs() + laft_index::MAX_VALIDITY_SECS,
        ),
        defaults: Default::default(),
        packages: Vec::new(),
        yanked: Vec::new(),
        options: BTreeMap::new(),
        recipes: None,
        profile: None,
    }
}

/// An accepted index of `repo` that publishes `packages` and `options`
/// from a recipe snapshot, unless `recipes` says otherwise.
fn repo_index(
    repo: &str,
    priority: i64,
    packages: Vec<laft_index::IndexEntry>,
    options: BTreeMap<Name, Vec<laft_index::OptionDecl>>,
    recipes: bool,
) -> RepoIndex {
    let none: Vec<String> = Vec::new();
    let mut index = index_base(repo);
    index.packages = packages;
    index.options = options;
    index.recipes = recipes.then(snapshot);
    RepoIndex {
        name: repo.parse().expect("a name"),
        repo: laft_config::Repo {
            url: "file:///tmp/none".to_owned(),
            mirrors: Vec::new(),
            key: "RWSO9Zvh1ju0E6MO5PDnzQFYnptNxsLpLdwYSfm5Cs4Jc+14umRuka1M"
                .parse()
                .expect("a key"),
            keys: Vec::new(),
            channel: "current".parse().expect("a name"),
            profile: None,
            priority,
            enable: true,
            allow_http: false,
        },
        accepted: laft_index::AcceptedIndex {
            index,
            record: laft_index::IndexRecord {
                channel: "current".parse().expect("a name"),
                profile: None,
                arch: ARCH,
                generation: 1,
                blake3: Some(laft_core::Blake3Hash::of(b"x")),
                signer: "0123456789abcdef".parse().expect("a key id"),
            },
            expired: false,
        },
        mirrors: laft_fetch::Mirrors::new("file:///tmp/none", &none, false).expect("a mirror"),
        synced: None,
    }
}

/// One entry of an index.
fn entry(meta: laft_pkg::Meta, id: &str) -> laft_index::IndexEntry {
    laft_index::IndexEntry {
        meta,
        id: laft_core::StoreId::from_manifest_hash(&laft_core::Blake3Hash::of(id.as_bytes())),
        file: format!("pool/{}.lpkg", id),
        size: 1,
        blake3: laft_core::Blake3Hash::of(id.as_bytes()),
    }
}

#[test]
fn buildables_come_from_the_repository_that_owns_the_name() {
    // The overlay owns mesa: the highest priority that lists it, even
    // though core publishes a newer version with a snapshot.
    let overlay = repo_index(
        "overlay",
        10,
        vec![entry(meta_of("mesa", "25.1.0", &[]), "low")],
        BTreeMap::from([(
            "mesa".parse().expect("a name"),
            vec![bool_decl("llvm", false)],
        )]),
        true,
    );
    let core = repo_index(
        "core",
        0,
        vec![entry(meta_of("mesa", "25.3.0", &[]), "high")],
        BTreeMap::from([(
            "mesa".parse().expect("a name"),
            vec![bool_decl("llvm", true)],
        )]),
        true,
    );
    let found = buildables(&[overlay, core]).0;
    assert_eq!(found.len(), 1);
    let mesa = &found[0];
    assert_eq!(mesa.version.to_string(), "25.1.0");
    assert_eq!(mesa.rev, 1);
    assert_eq!(mesa.variant, Variant::default());
    assert_eq!(
        mesa.defaults.get("llvm"),
        Some(&laft_core::OptionValue::Bool(false))
    );
}

#[test]
fn a_name_owned_by_a_repository_without_a_snapshot_is_not_buildable() {
    // bare owns mesa and publishes no snapshot, so no repository builds
    // it, whatever the lower priority repositories carry.
    let bare = repo_index(
        "bare",
        7,
        vec![entry(meta_of("mesa", "25.3.0", &[]), "bare")],
        BTreeMap::new(),
        false,
    );
    let high = repo_index(
        "high",
        5,
        vec![entry(meta_of("mesa", "25.2.0", &[]), "high")],
        BTreeMap::from([(
            "mesa".parse().expect("a name"),
            vec![bool_decl("llvm", true)],
        )]),
        true,
    );
    assert!(buildables(&[high, bare]).0.is_empty());
}

#[test]
fn a_priority_tie_gives_the_name_to_the_greater_repository_name() {
    let high = repo_index(
        "high",
        5,
        vec![entry(meta_of("mesa", "25.2.0", &[]), "high")],
        BTreeMap::from([(
            "mesa".parse().expect("a name"),
            vec![bool_decl("llvm", false)],
        )]),
        true,
    );
    let twin = repo_index(
        "twin",
        5,
        vec![entry(meta_of("mesa", "25.1.0", &[]), "low")],
        BTreeMap::from([(
            "mesa".parse().expect("a name"),
            vec![bool_decl("llvm", true)],
        )]),
        true,
    );
    let found = buildables(&[high, twin]).0;
    assert_eq!(found.len(), 1);
    // twin owns mesa: the same priority, the greater name.
    assert_eq!(found[0].version.to_string(), "25.1.0");
    assert_eq!(
        found[0].defaults.get("llvm"),
        Some(&laft_core::OptionValue::Bool(true))
    );
}

/// The local repository is never an owner: it publishes no snapshot, so
/// a name it lists with its owner keeps the owner's buildable. Before
/// the ownership rule, the implicit local repository took the name away
/// and nothing could build it again.
#[test]
fn the_local_repository_never_takes_a_buildable_from_its_owner() {
    let local = repo_index(
        "local",
        i64::MAX,
        vec![entry(meta_of("mesa", "25.1.0", &[]), "local")],
        BTreeMap::from([(
            "mesa".parse().expect("a name"),
            vec![bool_decl("llvm", false)],
        )]),
        false,
    );
    let core = repo_index(
        "core",
        0,
        vec![entry(meta_of("mesa", "25.3.0", &[]), "core")],
        BTreeMap::from([(
            "mesa".parse().expect("a name"),
            vec![bool_decl("llvm", true)],
        )]),
        true,
    );
    let found = buildables(&[local, core]).0;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].version.to_string(), "25.3.0");
    assert_eq!(
        found[0].defaults.get("llvm"),
        Some(&laft_core::OptionValue::Bool(true))
    );
}

#[test]
fn the_owner_contributes_its_newest_entry() {
    let core = repo_index(
        "core",
        1,
        vec![
            entry(meta_of("mesa", "25.1.0", &[]), "low"),
            entry(
                laft_pkg::Meta {
                    rev: 2,
                    ..meta_of("mesa", "25.2.0", &[])
                },
                "high",
            ),
        ],
        BTreeMap::new(),
        true,
    );
    let found = buildables(&[core]).0;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].version.to_string(), "25.2.0");
    assert_eq!(found[0].rev, 2);
}

/// The initrd closure resolves with the system view's option values, the
/// same buildables and the same `allow_builds`, so that a package in both
/// closures, the kernel included, is built with the same values.
#[test]
fn the_initrd_resolves_with_the_systems_option_values_and_buildables() {
    let dir = laft_test::tempdir();
    std::fs::write(
        dir.path().join("system.stav"),
        "system {\n  packages = [busybox]\n  \
         fs \"/\" { device = \"UUID=1\", fstype = ext4 }\n  \
         fs \"/boot\" { device = \"UUID=2\", fstype = vfat }\n}\n",
    )
    .expect("writable");
    let config = laft_config::load(
        dir.path(),
        &|_| Ok(()),
        &laft_config::IndexOptions::default(),
    )
    .expect("valid")
    .config;
    let mut static_meta = meta_of(
        "busybox",
        "1.37.0",
        &[("x", laft_core::OptionValue::Bool(true))],
    );
    static_meta.variant = Variant::Static;
    let mut e2fsprogs = meta_of("e2fsprogs", "1.47.0", &[]);
    e2fsprogs.variant = Variant::Static;
    let core = repo_index(
        "core",
        1,
        vec![
            entry(
                meta_of(
                    "busybox",
                    "1.37.0",
                    &[("x", laft_core::OptionValue::Bool(true))],
                ),
                "d",
            ),
            entry(static_meta, "s"),
            entry(e2fsprogs, "e"),
            entry(meta_of("intel-ucode", "20261001", &[]), "u"),
            entry(meta_of("linux", "6.12.0", &[]), "k"),
        ],
        BTreeMap::new(),
        false,
    );
    let mut options = BTreeMap::new();
    options.insert(
        "system".parse().expect("a name"),
        laft_config::OptionValues {
            fixed: BTreeMap::from([(
                "busybox".parse().expect("a name"),
                BTreeMap::from([(
                    "x".to_owned(),
                    laft_config::FixedSetting {
                        value: laft_core::OptionValue::Bool(false),
                        origin: "/etc/laft/system.stav:2:21".to_owned(),
                    },
                )]),
            )]),
            preferred: BTreeMap::new(),
        },
    );
    let buildable = |variant| laft_resolve::Buildable {
        name: "busybox".parse().expect("a name"),
        version: "1.38.0".parse().expect("a version"),
        rev: 1,
        variant,
        defaults: BTreeMap::from([("x".to_owned(), laft_core::OptionValue::Bool(true))]),
    };
    let context = ViewContext {
        options: &options,
        buildable: &[buildable(Variant::Dynamic), buildable(Variant::Static)],
        allow_builds: true,
    };
    let installed = InstalledMetas {
        views: BTreeMap::new(),
    };
    let services = ServiceSet::new(&config.services).expect("valid services");
    let host = laft_activate::Host {
        cpu_vendor: Some("GenuineIntel".to_owned()),
        ..Default::default()
    };
    let indexes = [core];
    let resolved = resolve(
        &universe(&indexes),
        &config,
        &context,
        &services,
        &installed,
        &Upgrade::None,
        &host,
    )
    .expect("the views resolve");
    assert_eq!(
        resolved.system.builds[0].values.get("x"),
        Some(&laft_core::OptionValue::Bool(false))
    );
    assert_eq!(resolved.initrd.builds.len(), 1);
    assert_eq!(resolved.initrd.builds[0].buildable.variant, Variant::Static);
    assert_eq!(
        resolved.initrd.builds[0].values.get("x"),
        Some(&laft_core::OptionValue::Bool(false))
    );
}

#[test]
fn a_repository_without_a_snapshot_contributes_no_buildables() {
    let plain = repo_index(
        "core",
        1,
        vec![entry(meta_of("mesa", "25.2.0", &[]), "c")],
        BTreeMap::new(),
        false,
    );
    assert!(buildables(&[plain]).0.is_empty());
}

#[test]
fn preferred_values_of_a_package_nothing_fixes_still_reach_the_resolver() {
    let values = laft_config::OptionValues {
        fixed: BTreeMap::new(),
        preferred: BTreeMap::from([(
            "mesa".parse().expect("a name"),
            BTreeMap::from([(
                "vulkan".to_owned(),
                laft_core::OptionValue::Tag("amd".to_owned()),
            )]),
        )]),
    };
    let input = option_request(&values);
    assert_eq!(input.len(), 1, "{input:?}");
    let mesa = &input[&"mesa".parse::<Name>().expect("a name")];
    assert_eq!(
        mesa.preferred["vulkan"],
        laft_core::OptionValue::Tag("amd".to_owned())
    );
    assert!(mesa.fixed.is_empty());
}

#[test]
fn every_variant_of_a_buildable_name_is_offered() {
    let mut static_meta = meta_of("mesa", "25.2.0", &[]);
    static_meta.variant = Variant::Static;
    let index = repo_index(
        "core",
        1,
        vec![
            entry(meta_of("mesa", "25.2.0", &[]), "d"),
            entry(static_meta, "s"),
        ],
        BTreeMap::new(),
        true,
    );
    let (found, snapshots) = buildables(&[index]);
    assert_eq!(found.len(), 2, "{found:?}");
    let mesa = "mesa".parse::<Name>().expect("a name");
    assert!(snapshots.contains_key(&(mesa.clone(), Variant::default())));
    assert!(snapshots.contains_key(&(mesa.clone(), Variant::Static)));
    assert!(
        found
            .iter()
            .any(|buildable| buildable.variant == Variant::Static)
    );
}

#[test]
fn the_options_of_a_view_become_the_fixed_and_preferred_values_of_the_resolver() {
    let values = laft_config::OptionValues {
        fixed: BTreeMap::from([(
            "mesa".parse().expect("a name"),
            BTreeMap::from([(
                "llvm".to_owned(),
                laft_config::FixedSetting {
                    value: laft_core::OptionValue::Bool(false),
                    origin: "/etc/laft/system.stav:9:5".to_owned(),
                },
            )]),
        )]),
        preferred: BTreeMap::from([(
            "mesa".parse().expect("a name"),
            BTreeMap::from([(
                "vulkan".to_owned(),
                laft_core::OptionValue::Tag("amd".to_owned()),
            )]),
        )]),
    };
    let input = option_request(&values);
    assert_eq!(input.len(), 1);
    let mesa = &input[&"mesa".parse::<Name>().expect("a name")];
    assert_eq!(
        mesa.fixed["llvm"].value,
        laft_core::OptionValue::Bool(false)
    );
    assert_eq!(mesa.fixed["llvm"].origin, "/etc/laft/system.stav:9:5");
    assert_eq!(
        mesa.preferred["vulkan"],
        laft_core::OptionValue::Tag("amd".to_owned())
    );
    assert!(option_request(&laft_config::OptionValues::default()).is_empty());
}

#[test]
fn store_reasons_carry_the_option_deviation() {
    let reasons = crate::resolve::reasons(&[laft_resolve::Reason::OptionMoved {
        option: "vulkan".into(),
        chosen: laft_core::OptionValue::Tag("all".into()),
        preferred: laft_core::OptionValue::Tag("amd".into()),
        why: "needed by blender 4.5.0-r1 (mesa{vulkan=all})".into(),
    }]);
    assert_eq!(
        reasons,
        vec![laft_store::Reason::OptionMoved {
            option: "vulkan".into(),
            chosen: laft_core::OptionValue::Tag("all".into()),
            preferred: laft_core::OptionValue::Tag("amd".into()),
            why: "needed by blender 4.5.0-r1 (mesa{vulkan=all})".into(),
        }]
    );
}
