//! Recipes and items built by hand for unit tests.

use std::collections::BTreeMap;

use laft_core::{PkgRef, Variant};

use crate::recipe::{
    BuildOptions, Ci, Metadata, Operations, Outputs, Phases, Recipe, Runner, Subpackage,
};
use crate::{Item, Shell, Split};

/// A recipe of `name` at version 1.0 for the variant of `item`, written
/// like a package reference, with nothing else set.
pub(crate) fn recipe(item: &str) -> Recipe {
    let item = self::item(item);
    Recipe {
        name: item.name,
        version: "1.0".parse().expect("a valid version"),
        rev: 1,
        variant: item.variant,
        repos: vec!["core".parse().expect("a valid name")],
        meta: Metadata {
            summary: "A package".into(),
            description: None,
            homepage: None,
            license: vec!["MIT".into()],
            license_files: None,
        },
        sources: Vec::new(),
        vendors: Vec::new(),
        patches: Vec::new(),
        patch_strip: 1,
        files: Vec::new(),
        build_deps: Vec::new(),
        link_deps: Vec::new(),
        check_deps: Vec::new(),
        shell: Shell::Sh,
        phases: Phases {
            prepare: None,
            configure: None,
            build: None,
            check: None,
            install: None,
            post_install: None,
        },
        env: BTreeMap::new(),
        options: BuildOptions {
            check: true,
            parallel: true,
            strip: true,
            debug_split: true,
            static_pie: true,
            keep_archives: false,
            lto: false,
            empty: false,
            keep_dirs: Vec::new(),
        },
        outputs: Outputs {
            split: match item.variant {
                Variant::Dynamic => vec![Split::Dbg, Split::Dev, Split::Doc],
                Variant::Static => vec![Split::Dbg],
            },
            subpackages: BTreeMap::new(),
            alternatives: BTreeMap::new(),
            deps: Vec::new(),
            provides: Vec::new(),
            conflicts: Vec::new(),
            replaces: Vec::new(),
            ignore_deps: Vec::new(),
            setuid: Vec::new(),
            caps: BTreeMap::new(),
            ownership: BTreeMap::new(),
        },
        smoke: None,
        abi_epoch: 0,
        toolchain_epoch: 0,
        operations: Operations {
            maintainers: Vec::new(),
            update: None,
            cpe: Vec::new(),
            ci: Ci {
                runner: Runner::Standard,
                timeout_minutes: 360,
            },
        },
    }
}

/// Parses an item written like a package reference, `zlib:static`.
pub(crate) fn item(text: &str) -> Item {
    let r: PkgRef = text.parse().expect("a valid reference");
    Item::new(r.name().clone(), r.variant())
}

/// Parses package references.
pub(crate) fn refs(texts: &[&str]) -> Vec<PkgRef> {
    texts
        .iter()
        .map(|t| t.parse().expect("a valid reference"))
        .collect()
}

/// An explicit subpackage holding `usr/lib/*.so.*`.
pub(crate) fn subpackage() -> Subpackage {
    Subpackage {
        summary: "Libraries".into(),
        files: vec!["usr/lib/*.so.*".into()],
        exclude: Vec::new(),
        deps: Vec::new(),
        provides: Vec::new(),
        conflicts: Vec::new(),
        replaces: Vec::new(),
        depends_on_main: false,
        license: vec!["MIT".into()],
        keep_empty: false,
    }
}

/// Writes `files` below `root`, creating parent directories, each with
/// mode `0o644`.
pub(crate) fn write_tree(root: &std::path::Path, files: &[(&str, &[u8])]) {
    use std::os::unix::fs::PermissionsExt;
    for (path, content) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("a file has a parent")).expect("mkdir");
        std::fs::write(&path, content).expect("writable");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    }
}

/// Creates the symlink `path` below `root` pointing at `target`.
pub(crate) fn link(root: &std::path::Path, path: &str, target: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().expect("a link has a parent")).expect("mkdir");
    std::os::unix::fs::symlink(target, path).expect("a symlink can be made");
}

/// The bytes of an ELF fixture of `laft-elf`: `libversioned.so` (soname
/// `libversioned.so.1`, `DT_RPATH` `/opt/legacy/lib`), `libunnamed.so`
/// (the same library without a soname), `program` (needs
/// `libversioned.so.1`, `RUNPATH` `$ORIGIN/../lib:/usr/lib`), `program32`
/// or `static`.
pub(crate) fn elf_fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../laft-elf/tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The mode bits of `path`.
pub(crate) fn mode(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .expect("the path exists")
        .permissions()
        .mode()
        & 0o7777
}

/// A package to add to a store in tests.
pub(crate) struct TestPkg<'a> {
    pub(crate) name: &'a str,
    pub(crate) variant: Variant,
    pub(crate) files: &'a [(&'a str, &'a [u8])],
    pub(crate) depends: &'a [&'a str],
    pub(crate) provides: &'a [&'a str],
    /// The package its recipe is named after, `name` when unset.
    pub(crate) origin: Option<&'a str>,
    /// An alternatives group with the paths it ships and its priority.
    pub(crate) alternative: Option<(&'a str, &'a [&'a str], i64)>,
}

impl<'a> TestPkg<'a> {
    /// A `dynamic` package `name` with one file `usr/share/<name>/file`.
    pub(crate) fn new(name: &'a str) -> Self {
        Self {
            name,
            variant: Variant::Dynamic,
            files: &[],
            depends: &[],
            provides: &[],
            origin: None,
            alternative: None,
        }
    }

    /// Writes the package file into `dir`, adds it to `store` and returns
    /// its store id.
    pub(crate) fn add(
        &self,
        store: &laft_store::Store,
        dir: &std::path::Path,
    ) -> laft_core::StoreId {
        let (file, id) = self.write(dir);
        store
            .insert(&file, &id, &dir.join("tmp"))
            .expect("the package enters the store");
        id
    }

    /// Writes the package file into `dir` and returns its path and store
    /// id.
    pub(crate) fn write(&self, dir: &std::path::Path) -> (std::path::PathBuf, laft_core::StoreId) {
        use std::os::unix::fs::PermissionsExt;
        let payload = dir.join(format!("payload-{}-{}", self.name, self.variant));
        let default = format!("usr/share/{}/file", self.name);
        let files: Vec<(&str, &[u8])> = if self.files.is_empty() {
            vec![(default.as_str(), self.name.as_bytes())]
        } else {
            self.files.to_vec()
        };
        write_tree(&payload, &files);
        for (path, _) in &files {
            std::fs::set_permissions(payload.join(path), std::fs::Permissions::from_mode(0o444))
                .expect("chmod");
        }
        let meta = laft_pkg::Meta {
            format: laft_pkg::FORMAT,
            name: self.name.parse().expect("a valid name"),
            version: "1.0".parse().expect("a valid version"),
            rev: 1,
            variant: self.variant,
            arch: laft_pkg::Arch::X86_64V3,
            origin: self
                .origin
                .unwrap_or(self.name)
                .parse()
                .expect("a valid name"),
            summary: "A test package".into(),
            homepage: None,
            license: vec!["MIT".into()],
            depends: self
                .depends
                .iter()
                .map(|d| d.parse().expect("a dep"))
                .collect(),
            provides: self
                .provides
                .iter()
                .map(|p| p.parse().expect("a provide"))
                .collect(),
            conflicts: vec![],
            replaces: vec![],
            alternatives: self
                .alternative
                .iter()
                .map(|(group, paths, priority)| laft_pkg::Alternative {
                    group: group.parse().expect("a valid group"),
                    paths: paths.iter().map(|p| (*p).to_owned()).collect(),
                    priority: *priority,
                })
                .collect(),
            triggers: vec![],
            static_deps: vec![],
            abi: Default::default(),
            abi_epoch: 2,
            requested_privileges: vec![],
            installed_size: 0,
        };
        let entries = laft_pkg::scan(&payload).expect("scannable");
        let mut meta = meta;
        meta.installed_size = entries
            .iter()
            .map(|e| match e {
                laft_pkg::Entry::File { size, .. } => *size,
                _ => 0,
            })
            .sum();
        let manifest = laft_pkg::Manifest::new(
            meta.identity(),
            laft_core::Blake3Hash::of(&meta.to_json().expect("valid meta")),
            entries,
        )
        .expect("a valid manifest");
        let file = dir.join(manifest.package().file_name());
        let mut out = std::fs::File::create(&file).expect("writable");
        laft_pkg::write_package(
            &mut out,
            &payload,
            &meta,
            &manifest,
            laft_pkg::Compression::DEVELOPMENT,
        )
        .expect("the package is written");
        (file, manifest.store_id())
    }
}
