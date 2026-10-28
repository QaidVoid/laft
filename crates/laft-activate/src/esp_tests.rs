use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use laft_core::{Blake3Hash, StoreId, Timestamp};
use laft_store::{ConfigRecord, Generation, Init, PackageRecord};

use super::*;
use crate::test_support::Env;

fn id(text: &str) -> StoreId {
    StoreId::from_manifest_hash(&Blake3Hash::of(text.as_bytes()))
}

/// Writes a generation with the boot files `install_boot` reads: a kernel
/// of release `release` from package `linux` with id `id(release)`, and an
/// initramfs with content `initramfs`.
fn generation(env: &Env, number: u64, release: &str, initramfs: &[u8], config: &str) -> PathBuf {
    let dir = env.path(&format!("laft/gens/{number}"));
    let kernel = dir.join(format!("usr/lib/modules/{release}"));
    fs::create_dir_all(&kernel).unwrap();
    fs::write(kernel.join("vmlinuz"), format!("kernel {release}")).unwrap();
    fs::create_dir_all(dir.join("boot")).unwrap();
    fs::write(dir.join("boot/initramfs"), initramfs).unwrap();
    let hash = Blake3Hash::of(initramfs).to_string()[3..35].to_owned();
    let record = BootRecord {
        version: 1,
        cmdline: vec![format!("laft.gen={number}"), "quiet".into()],
        firmware: vec![],
        initramfs: hash,
        kernel_id: id(release),
        kernel_release: release.into(),
        modules: vec![],
    };
    fs::write(
        dir.join("boot/boot.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    fs::write(dir.join("config.json"), config).unwrap();
    let generation = Generation {
        format: 1,
        generation: number,
        parent: None,
        created: Timestamp::from_secs(1_761_660_120 + number * 86_400),
        message: None,
        laft_version: "0.1.0".into(),
        arch: "x86_64-v3".into(),
        init: Init {
            backend: "runit".into(),
            exec: vec!["/usr/bin/runit".into()],
        },
        config: ConfigRecord {
            hash: Blake3Hash::of(config.as_bytes()),
            files: vec![],
        },
        indexes: vec![],
        packages: vec![PackageRecord {
            name: "linux".parse().unwrap(),
            version: "6.17".parse().unwrap(),
            rev: 1,
            variant: Default::default(),
            id: id(release),
            dir: format!("{}-linux-6.17", id(release)),
            reasons: vec![],
        }],
        initrd_packages: vec![],
        alternatives: BTreeMap::new(),
        views: BTreeMap::new(),
        carried_modules: vec![],
        etc: Default::default(),
    };
    fs::write(
        dir.join("gen.json"),
        serde_json::to_vec(&generation).unwrap(),
    )
    .unwrap();
    dir
}

fn set_default(env: &Env, number: u64) {
    let link = env.path("laft/gens/default");
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(number.to_string(), link).unwrap();
}

fn esp(env: &Env) -> PathBuf {
    let esp = env.path("boot");
    fs::create_dir_all(&esp).unwrap();
    esp
}

/// Every file `limine.conf` names exists on the partition.
fn assert_menu_files_exist(esp: &Path) {
    let conf = fs::read_to_string(esp.join("limine.conf")).unwrap();
    for line in conf.lines() {
        if let Some(file) = line
            .trim()
            .strip_prefix("path: boot():/")
            .or_else(|| line.trim().strip_prefix("module_path: boot():/"))
        {
            assert!(esp.join(file).is_file(), "{file} is missing");
        }
    }
}

#[test]
fn menu_lists_generations_newest_first_with_the_default() {
    let env = Env::new();
    generation(&env, 1, "6.17.4-laft1", b"initramfs one", "{}");
    generation(
        &env,
        2,
        "6.17.5-laft1",
        b"initramfs two",
        r#"{"boot": {"timeout": null, "editor": true}}"#,
    );
    set_default(&env, 1);
    let esp = esp(&env);
    let installed = install_boot(&env.root, &esp).unwrap();
    assert_eq!(installed.entries, [2, 1]);
    assert_eq!(installed.default, Some(1));
    assert_eq!(installed.copied.len(), 4);

    let one = Blake3Hash::of(b"initramfs one").to_string()[3..35].to_owned();
    let two = Blake3Hash::of(b"initramfs two").to_string()[3..35].to_owned();
    let (k1, k2) = (id("6.17.4-laft1"), id("6.17.5-laft1"));
    assert_eq!(
        fs::read_to_string(esp.join("limine.conf")).unwrap(),
        format!(
            "# Rendered by laft. Changes are overwritten on the next apply.\n\
             timeout: 3\n\
             default_entry: 2\n\
             remember_last_entry: no\n\
             editor_enabled: no\n\
             interface_branding: Laft\n\
             \n\
             /Laft 2\n    comment: 2025-10-30 14:02 UTC, linux 6.17.5-laft1\n    protocol: linux\n    \
             path: boot():/laft/{k2}.vmlinuz\n    module_path: boot():/laft/{two}.initramfs\n    \
             cmdline: laft.gen=2 quiet\n\
             \n\
             /Laft 1\n    comment: 2025-10-29 14:02 UTC, linux 6.17.4-laft1\n    protocol: linux\n    \
             path: boot():/laft/{k1}.vmlinuz\n    module_path: boot():/laft/{one}.initramfs\n    \
             cmdline: laft.gen=1 quiet\n"
        )
    );
    assert_eq!(
        fs::read_to_string(esp.join("laft/entries.json")).unwrap(),
        r#"{"default_gen":1,"entries":[{"gen":2,"index":1},{"gen":1,"index":2}],"version":1}"#
    );
    assert_eq!(
        fs::read(esp.join(format!("laft/{k1}.vmlinuz"))).unwrap(),
        b"kernel 6.17.4-laft1"
    );
    assert_eq!(
        fs::read(esp.join(format!("laft/{two}.initramfs"))).unwrap(),
        b"initramfs two"
    );

    set_default(&env, 2);
    install_boot(&env.root, &esp).unwrap();
    let conf = fs::read_to_string(esp.join("limine.conf")).unwrap();
    assert!(conf.contains("timeout: no\ndefault_entry: 1\n"), "{conf}");
    assert!(conf.contains("editor_enabled: yes\n"), "{conf}");
}

#[test]
fn identical_files_are_shared_and_unused_ones_removed() {
    let env = Env::new();
    generation(&env, 1, "6.17.4-laft1", b"same", "{}");
    generation(&env, 2, "6.17.4-laft1", b"same", "{}");
    generation(&env, 3, "6.17.5-laft1", b"newer", "{}");
    let esp = esp(&env);
    install_boot(&env.root, &esp).unwrap();
    assert_eq!(fs::read_dir(esp.join("laft")).unwrap().count(), 5);
    fs::write(esp.join("laft/.tmp-123-0"), b"leftover").unwrap();

    fs::remove_dir_all(env.path("laft/gens/3")).unwrap();
    let installed = install_boot(&env.root, &esp).unwrap();
    assert_eq!(installed.entries, [2, 1]);
    assert_eq!(installed.removed.len(), 3, "{:?}", installed.removed);
    let mut names: Vec<String> = fs::read_dir(esp.join("laft"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_menu_files_exist(&esp);
}

#[test]
fn generations_without_boot_files_are_left_out() {
    let env = Env::new();
    generation(&env, 1, "6.17.4-laft1", b"one", "{}");
    let incomplete = generation(&env, 2, "6.17.5-laft1", b"two", "{}");
    fs::remove_file(incomplete.join("gen.json")).unwrap();
    let broken = generation(&env, 3, "6.17.5-laft1", b"three", "{}");
    fs::remove_file(broken.join("boot/boot.json")).unwrap();
    let installed = install_boot(&env.root, &esp(&env)).unwrap();
    assert_eq!(installed.entries, [1]);
    assert_eq!(installed.default, None);
}

#[test]
fn failed_generations_are_marked() {
    let env = Env::new();
    generation(&env, 1, "6.17.4-laft1", b"one", "{}");
    generation(&env, 2, "6.17.5-laft1", b"two", "{}");
    env.write(
        "laft/state/boot.json",
        r#"{"bad":[2],"good":[1],"version":1}"#,
    );
    let esp = esp(&env);
    install_boot(&env.root, &esp).unwrap();
    let conf = fs::read_to_string(esp.join("limine.conf")).unwrap();
    assert!(conf.contains("/Laft 2 (failed to boot)\n"), "{conf}");
    assert!(conf.contains("/Laft 1\n"), "{conf}");
}

#[test]
fn limine_is_updated_from_the_newest_generation() {
    let env = Env::new();
    let dir = generation(&env, 1, "6.17.4-laft1", b"one", "{}");
    fs::create_dir_all(dir.join("usr/share/limine")).unwrap();
    fs::write(dir.join(LIMINE), b"limine 9").unwrap();
    let esp = esp(&env);
    let installed = install_boot(&env.root, &esp).unwrap();
    assert!(installed.copied.contains(&LIMINE_ON_ESP.to_owned()));
    assert_eq!(fs::read(esp.join(LIMINE_ON_ESP)).unwrap(), b"limine 9");
    let again = install_boot(&env.root, &esp).unwrap();
    assert!(again.copied.is_empty(), "{:?}", again.copied);
}

#[test]
fn an_interrupted_installation_leaves_a_bootable_menu() {
    for &point in fault::POINTS.iter().filter(|p| p.starts_with("esp.")) {
        let env = Env::new();
        generation(&env, 1, "6.17.4-laft1", b"one", "{}");
        let esp = esp(&env);
        install_boot(&env.root, &esp).unwrap();
        fs::remove_dir_all(env.path("laft/gens/1")).unwrap();
        generation(&env, 2, "6.17.5-laft1", b"two", "{}");

        fault::fail_at(Some(point));
        let result = install_boot(&env.root, &esp);
        fault::fail_at(None);
        assert!(
            matches!(result, Err(Error::Injected(p)) if p == point),
            "{point}"
        );
        assert_menu_files_exist(&esp);

        let installed = install_boot(&env.root, &esp).unwrap();
        assert_eq!(installed.entries, [2]);
        assert_menu_files_exist(&esp);
        assert_eq!(fs::read_dir(esp.join("laft")).unwrap().count(), 3);
    }
}

#[test]
fn a_directory_is_not_an_esp() {
    let env = Env::new();
    assert!(check_esp(&esp(&env)).is_err());
}

#[test]
fn boot_state_is_read_like_laft_boot_reads_it() {
    let env = Env::new();
    generation(&env, 1, "6.17.4-laft1", b"one", "{}");
    let esp = esp(&env);
    env.write("laft/state/boot.json", r#"{"version":2,"attempts":[]}"#);
    let err = install_boot(&env.root, &esp).unwrap_err().to_string();
    assert!(err.contains("unsupported version 2"), "{err}");
    env.write(
        "laft/state/boot.json",
        r#"{"bad":[1],"unknown":true,"version":1}"#,
    );
    assert!(install_boot(&env.root, &esp).is_err());
}

#[test]
fn the_entry_index_is_written_before_the_menu() {
    let env = Env::new();
    generation(&env, 1, "6.17.4-laft1", b"one", "{}");
    let esp = esp(&env);
    install_boot(&env.root, &esp).unwrap();
    generation(&env, 2, "6.17.5-laft1", b"two", "{}");

    fault::fail_at(Some("esp.entries"));
    let result = install_boot(&env.root, &esp);
    fault::fail_at(None);
    assert!(matches!(result, Err(Error::Injected("esp.entries"))));
    let entries = fs::read_to_string(esp.join("laft/entries.json")).unwrap();
    assert!(entries.contains(r#""gen":2"#), "{entries}");
    let menu = fs::read_to_string(esp.join("limine.conf")).unwrap();
    assert!(!menu.contains("/Laft 2"), "{menu}");
}

#[test]
fn removed_files_are_reported_in_name_order() {
    let env = Env::new();
    generation(&env, 1, "6.17.4-laft1", b"one", "{}");
    let esp = esp(&env);
    fs::create_dir_all(esp.join("laft")).unwrap();
    for name in ["c.vmlinuz", "a.initramfs", "b.vmlinuz"] {
        fs::write(esp.join("laft").join(name), b"").unwrap();
    }
    let installed = install_boot(&env.root, &esp).unwrap();
    assert_eq!(
        installed.removed,
        ["laft/a.initramfs", "laft/b.vmlinuz", "laft/c.vmlinuz"]
    );
}
