use std::collections::BTreeMap;
use std::os::unix::fs::symlink;

use laft_core::OptionValue;
use laft_fetch::Mirrors;
use laft_index::AcceptedIndex;

use super::*;

#[test]
fn names_one_edit_apart_are_similar() {
    assert!(similar("htop", "hto"));
    assert!(similar("htop", "htoq"));
    assert!(!similar("htop", "htpo"), "a transposition is two edits");
    assert!(similar("ripgrep", "ripgre"));
    assert!(similar("vim", "vin"));
    assert!(!similar("vim", "emacs"));
    assert!(!similar("abc", "abcde"));
}

#[test]
fn generation_links_are_read_by_their_last_component() {
    let dir = laft_test::tempdir();
    let link = dir.path().join("system");
    assert_eq!(link_number(&link).expect("absent is fine"), None);
    symlink("mounts/42", &link).expect("can link");
    assert_eq!(link_number(&link).expect("a number"), Some(42));
    let bad = dir.path().join("bad");
    symlink("mounts/x", &bad).expect("can link");
    assert!(link_number(&bad).is_err());
}

#[test]
fn the_esp_comes_from_the_default_generation_s_configuration() {
    let dir = laft_test::tempdir();
    let root = Root::new(dir.path());
    assert_eq!(recorded_esp(&root), None);
    for (number, esp) in [(1, "/efi"), (2, "/boot/efi")] {
        let gen_dir = dir.path().join(format!("laft/gens/{number}"));
        std::fs::create_dir_all(&gen_dir).expect("writable");
        let config = format!("{{\"boot\": {{\"esp\": \"{esp}\"}}}}");
        std::fs::write(gen_dir.join("config.json"), config).expect("writable");
    }
    assert_eq!(
        recorded_esp(&root).as_deref(),
        Some("/boot/efi"),
        "the newest"
    );
    symlink("1", dir.path().join("laft/gens/default")).expect("can link");
    assert_eq!(recorded_esp(&root).as_deref(), Some("/efi"), "the default");
}

#[test]
fn only_paths_below_the_root_are_renamed() {
    let root = Path::new("/mnt");
    let seen = |path: &str| seen_from(root, path);
    assert_eq!(
        seen("/mnt/etc/laft/system.stav:9:31").as_deref(),
        Some("/etc/laft/system.stav:9:31")
    );
    assert_eq!(seen("/mntx/etc/laft/system.stav"), None);
    assert_eq!(seen("/srv/etc/laft/system.stav"), None);
    assert_eq!(seen("boot.kernel"), None);
    assert_eq!(seen_from(Path::new("/"), "/etc/laft/system.stav"), None);
}

/// The environment of a root, quiet so that tests print nothing.
fn env_of(root: &Path) -> Env {
    let global = crate::cli::Global {
        config: None,
        root: Some(root.to_path_buf()),
        json: false,
        color: None,
        ascii: false,
        verbose: 0,
        quiet: true,
        yes: false,
        offline: false,
        no_wait: false,
        no_pager: false,
        message_format: crate::cli::MessageFormat::Human,
    };
    Env::new(&global)
}

/// An index of `repo` with the option declarations `options`.
fn index_of(
    repo: &str,
    options: BTreeMap<Name, Vec<laft_index::OptionDecl>>,
    profile: Option<laft_index::ProfileMember>,
) -> Index {
    Index {
        format: Index::FORMAT,
        repo: repo.parse().expect("a name"),
        channel: "current".parse().expect("a name"),
        arch: ARCH,
        generation: 1,
        timestamp: Timestamp::now(),
        expires: Timestamp::from_secs(Timestamp::now().as_secs() + laft_index::MAX_VALIDITY_SECS),
        defaults: Default::default(),
        packages: Vec::new(),
        yanked: Vec::new(),
        options,
        recipes: None,
        profile,
    }
}

/// The declaration of one bool option.
fn bool_option(name: &str, default: bool) -> laft_index::OptionDecl {
    laft_index::OptionDecl {
        name: name.to_owned(),
        kind: laft_index::OptionType::Bool("bool".to_owned()),
        default: OptionValue::Bool(default),
        doc: "d".to_owned(),
    }
}

/// Writes the index of `repo` into the cache and the state of `env`, as
/// a sync that accepted it would have.
fn put_index(env: &Env, repo: &str, index: &Index) {
    let bytes = index.encode().expect("the index encodes");
    let dir = env
        .path("/laft/cache/index")
        .join(repo)
        .join("channels/current")
        .join(ARCH.as_str());
    fs::create_dir_all(&dir).expect("writable");
    fs::write(dir.join("index.json.zst"), &bytes).expect("writable");
    let record = serde_json::json!({
        "format": 1,
        "records": [{
            "channel": "current",
            "arch": ARCH.as_str(),
            "generation": 1,
            "blake3": Blake3Hash::of(&bytes).to_string(),
            "signer": "0123456789abcdef",
        }],
    });
    let state = env.path("/laft/state/index");
    fs::create_dir_all(&state).expect("writable");
    fs::write(
        state.join(format!("{repo}.json")),
        serde_json::to_vec(&record).expect("the record is JSON"),
    )
    .expect("writable");
}

fn mesa_options(llvm: bool) -> BTreeMap<Name, Vec<laft_index::OptionDecl>> {
    let decls = vec![
        bool_option("llvm", llvm),
        laft_index::OptionDecl {
            name: "vulkan".to_owned(),
            kind: laft_index::OptionType::Tags(vec![
                "all".to_owned(),
                "amd".to_owned(),
                "intel".to_owned(),
            ]),
            default: OptionValue::Tag("all".to_owned()),
            doc: "d".to_owned(),
        },
    ];
    [("mesa".parse::<Name>().expect("a name"), decls)]
        .into_iter()
        .collect()
}

#[test]
fn cached_options_come_from_the_cache_and_the_later_repository_wins() {
    let dir = laft_test::tempdir();
    let env = env_of(dir.path());
    let first = index_of("aaa", mesa_options(true), None);
    let mut second = index_of("zzz", mesa_options(false), None);
    let values = [(
        "mesa".parse::<Name>().expect("a name"),
        [("vulkan".to_owned(), OptionValue::Tag("amd".to_owned()))]
            .into_iter()
            .collect::<BTreeMap<String, OptionValue>>(),
    )]
    .into_iter()
    .collect();
    second.profile = Some(laft_index::ProfileMember {
        name: "wayland_only".parse().expect("a name"),
        values,
    });
    put_index(&env, "aaa", &first);
    put_index(&env, "zzz", &second);
    let options = env.cached_options();
    let mesa = "mesa".parse::<Name>().expect("a name");
    assert_eq!(
        options.options[&mesa][0].default,
        OptionValue::Bool(false),
        "the later repository wins"
    );
    assert_eq!(options.options[&mesa].len(), 2);
    let values = &options.profile[&mesa];
    assert_eq!(values["vulkan"], OptionValue::Tag("amd".to_owned()));
}

#[test]
fn a_cached_index_that_no_longer_matches_its_pin_contributes_nothing() {
    let dir = laft_test::tempdir();
    let env = env_of(dir.path());
    let mut index = index_of("core", mesa_options(true), None);
    index.options.clear();
    put_index(&env, "core", &index);
    let bytes = index_of("core", mesa_options(true), None)
        .encode()
        .expect("the index encodes");
    let tampered = env
        .path("/laft/cache/index")
        .join("core/channels/current")
        .join(ARCH.as_str())
        .join("index.json.zst");
    fs::write(&tampered, &bytes).expect("writable");
    assert!(env.cached_options().options.is_empty());
}

#[test]
fn the_options_of_the_synced_indexes_follow_the_repository_priority() {
    let index = |llvm: bool| AcceptedIndex {
        index: index_of("repo", mesa_options(llvm), None),
        record: IndexRecord {
            channel: "current".parse().expect("a name"),
            profile: None,
            arch: ARCH,
            generation: 1,
            blake3: Some(Blake3Hash::of(b"x")),
            signer: "0123456789abcdef".parse().expect("a key id"),
        },
        expired: false,
    };
    let mirrors = || {
        let none: Vec<String> = Vec::new();
        Mirrors::new("file:///tmp/none", &none, false).expect("a mirror")
    };
    let repo = |name: &str, priority: i64, index| RepoIndex {
        name: name.parse().expect("a name"),
        repo: laft_config::Repo {
            url: "file:///tmp/none".to_owned(),
            mirrors: Vec::new(),
            key: "RWSO9Zvh1ju0E6MO5PDnzQFYnptNxsLpLdwYSfm5Cs4Jc+14umRuka1M"
                .parse()
                .expect("a key"),
            keys: Vec::new(),
            channel: "current".parse().expect("a name"),
            priority,
            enable: true,
            allow_http: false,
        },
        accepted: index,
        mirrors: mirrors(),
        synced: None,
    };
    let indexes = vec![repo("low", 0, index(true)), repo("high", 5, index(false))];
    let options = options_of(&indexes);
    let mesa = "mesa".parse::<Name>().expect("a name");
    assert_eq!(options.options[&mesa][0].default, OptionValue::Bool(false));
}
