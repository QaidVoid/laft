use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use laft_store::{GenerationInput, Init, PackageInput, Plan, Reason, Transaction, ViewInput};
use laft_svc::{ServiceConfig, ServiceSet, standard_services};

use super::*;
use crate::test_support::{
    Built, Env, Pkg, base_users, mode, module_elf, read_cpio, sandbox_unavailable, shell_package,
    zstd,
};
use crate::{BootConfig, Declared, EtcConfig, File, Fs, Privileged, Security, fault};

const RELEASE: &str = "6.1.0-laft1";

/// A fake `depmod` that lists every module of the release as a
/// `modules.dep` without dependencies, using only shell builtins.
const DEPMOD: &[u8] = b"#!/bin/sh\n\
dir=/usr/lib/modules/$2\n\
for m in $dir/kernel/*/*.ko* $dir/kernel/*/*/*.ko*; do [ -e \"$m\" ] && echo \"${m#$dir/}:\"; done > $dir/modules.dep\n\
exit 0\n";

/// The packages of the tests, built once per fixture.
struct Packages {
    shell: Built,
    kmod: Built,
    linux: Built,
    laft: Built,
    firmware: Built,
    fonts: Built,
    fontconfig: Built,
    app: Built,
    desktop: Built,
    schemas: Built,
    config: Built,
    busybox: Built,
}

/// A temporary root with built packages and a configuration.
struct Fixture {
    env: Env,
    pkgs: PathBuf,
    packages: Packages,
    etc: EtcConfig,
    services: ServiceSet,
    boot: BootConfig,
    security: Security,
}

fn linux(release: &str) -> Pkg {
    let ext4 = module_elf(b"license=GPL\0firmware=test/fw.bin\0firmware=test/missing.bin\0");
    Pkg::new("linux", &release.replace("-laft1", ""))
        .file(
            &format!("usr/lib/modules/{release}/vmlinuz"),
            release.as_bytes(),
        )
        .file(
            &format!("usr/lib/modules/{release}/kernel/fs/ext4.ko.zst"),
            &zstd(&ext4),
        )
        .file(
            &format!("usr/lib/modules/{release}/kernel/drivers/nvme/nvme.ko"),
            &module_elf(b"license=GPL\0"),
        )
        .file(
            &format!("usr/lib/modules/{release}/modules.builtin"),
            b"kernel/fs/vfat.ko\n",
        )
        .trigger("depmod")
}

impl Fixture {
    /// The fixture, or `None` after printing why this environment cannot
    /// run triggers.
    fn new() -> Option<Self> {
        if let Some(reason) = sandbox_unavailable() {
            println!("skipping: {reason}");
            return None;
        }
        let env = Env::new();
        let pkgs = env.path("pkgs");
        fs::create_dir_all(&pkgs).unwrap();
        let build = |pkg: Pkg| pkg.build(&pkgs);
        let packages = Packages {
            shell: build(shell_package(&[])?),
            kmod: build(Pkg::new("kmod", "33").file("usr/bin/depmod", DEPMOD)),
            linux: build(linux(RELEASE)),
            laft: build(
                Pkg::new("laft", "0.1")
                    .file("usr/lib/laft/laft-boot", b"laft-boot binary")
                    .file("usr/bin/doas", b"doas binary"),
            ),
            firmware: build(
                Pkg::new("linux-firmware", "1").file("usr/lib/firmware/test/fw.bin", b"fw"),
            ),
            fonts: build(
                Pkg::new("fonts", "1")
                    .file("usr/share/fonts/a.ttf", b"font")
                    .trigger("fontconfig"),
            ),
            fontconfig: build(Pkg::new("fontconfig", "2").file(
                "usr/bin/fc-cache",
                b"#!/bin/sh\necho \"$1 $2\" > /usr/lib/fontconfig/cache/fonts.cache\n",
            )),
            app: build(
                Pkg::new("app", "1")
                    .file("usr/share/applications/app.desktop", b"[Desktop Entry]\n")
                    .trigger("desktop"),
            ),
            desktop: build(Pkg::new("desktop-utils", "1").file(
                "usr/bin/update-desktop-database",
                b"#!/bin/sh\necho changed > /usr/share/applications/app.desktop\n",
            )),
            schemas: build(
                Pkg::new("schemas", "1")
                    .file("usr/share/glib-2.0/schemas/a.xml", b"<schemalist/>")
                    .trigger("glib-schemas"),
            ),
            config: build(Pkg::new("pkgconf", "1").file("etc/pkg.conf", b"default\n")),
            busybox: build(
                Pkg::new("busybox", "1")
                    .file("usr/bin/busybox", b"busybox binary")
                    .file("usr/bin/e2fsck", b"e2fsck binary"),
            ),
        };
        let mut etc = EtcConfig {
            hostname: "laft-test".into(),
            users: base_users(),
            ..EtcConfig::default()
        };
        etc.files.insert(
            "/etc/motd".into(),
            File {
                text: Some("hello\n".into()),
                ..File::default()
            },
        );
        let services = ServiceSet::new(&ServiceConfig {
            services: standard_services(),
            ..ServiceConfig::default()
        })
        .unwrap();
        let mut boot = BootConfig::default();
        boot.boot.initrd.hostonly = false;
        boot.fs.insert(
            "/".into(),
            Fs {
                device: Some("UUID=6b1d0b3e-6f5c-4a62-9a3f-1f4f0c8b9e21".into()),
                fstype: "ext4".into(),
                options: vec!["noatime".into()],
                ..Fs::default()
            },
        );
        boot.fs.insert(
            "/boot".into(),
            Fs {
                device: Some("UUID=4A2B-91C0".into()),
                fstype: "vfat".into(),
                ..Fs::default()
            },
        );
        let mut security = Security::default();
        security
            .privileged
            .insert("/usr/bin/doas".into(), Privileged::default());
        security
            .privileged
            .insert("/usr/bin/missing".into(), Privileged::default());
        Some(Self {
            env,
            pkgs,
            packages,
            etc,
            services,
            boot,
            security,
        })
    }

    fn system(&self) -> Vec<&Built> {
        let p = &self.packages;
        vec![
            &p.shell,
            &p.kmod,
            &p.linux,
            &p.laft,
            &p.firmware,
            &p.fonts,
            &p.fontconfig,
            &p.app,
            &p.desktop,
            &p.schemas,
            &p.config,
        ]
    }

    fn input(&self, packages: &[&Built], views: BTreeMap<laft_core::Name, u64>) -> GenerationInput {
        let inputs = |list: &[&Built]| {
            list.iter()
                .map(|b| PackageInput {
                    dir: b.dir.clone(),
                    reasons: if b.name == "shell" {
                        vec![Reason::Dependency {
                            by: "laft".parse().unwrap(),
                            dep: "so:libc.so.6".into(),
                        }]
                    } else {
                        vec![Reason::Requested {
                            origin: "test".into(),
                            text: b.name.clone(),
                        }]
                    },
                })
                .collect()
        };
        GenerationInput {
            packages: inputs(packages),
            initrd_packages: inputs(&[&self.packages.busybox]),
            carry_over: vec![],
            alternatives: BTreeMap::new(),
            views,
            message: None,
            laft_version: "0.1.0".into(),
            arch: "x86_64-v3".into(),
            init: Init {
                backend: "runit".into(),
                exec: vec!["/usr/bin/runit".into()],
            },
            config: b"{\"boot\": {\"timeout\": 5}}".to_vec(),
            config_files: vec![],
            indexes: vec![],
            etc: Default::default(),
        }
    }

    fn plan(&self, views: &[&str]) -> Plan {
        Plan {
            command: "apply".into(),
            previous: self.env.root.default_generation().unwrap(),
            views: views.iter().map(|v| v.parse().unwrap()).collect(),
        }
    }

    /// Adds `extra` and the busybox package to the store in `tx`.
    fn add(&self, tx: &mut Transaction<'_>, packages: &[&Built]) {
        for package in packages.iter().chain([&&self.packages.busybox]) {
            tx.add(&package.path, &package.id).unwrap();
        }
    }

    /// Assembles a generation of `packages` and makes it the default.
    fn apply(&self, packages: &[&Built], host: &Host) -> Result<Assembled> {
        let lock = self.env.lock();
        let mut tx = Transaction::begin(&lock, &self.plan(&[]))?;
        self.add(&mut tx, packages);
        let input = self.input(packages, BTreeMap::new());
        let assembled = assemble(&lock, &mut tx, &self.assembly(&input, host))?;
        tx.boot_installed()?;
        tx.make_default()?;
        Ok(assembled)
    }

    fn assembly<'a>(&'a self, input: &'a GenerationInput, host: &'a Host) -> Assembly<'a> {
        Assembly {
            generation: input,
            etc: &self.etc,
            services: &self.services,
            boot: &self.boot,
            security: &self.security,
            host,
        }
    }

    fn generation_dir(&self, number: u64) -> PathBuf {
        self.env.path(&format!("laft/gens/{number}"))
    }

    fn store_file(&self, package: &Built, path: &str) -> PathBuf {
        self.env.root.store().path(&package.dir).join(path)
    }
}

fn inode(path: &Path) -> (u64, u64) {
    let metadata = fs::symlink_metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Every directory below `dir`, which must all be sealed.
fn assert_sealed(dir: &Path) {
    for item in fs::read_dir(dir).unwrap() {
        let path = item.unwrap().path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            assert_eq!(
                metadata.permissions().mode() & 0o7777,
                0o555,
                "{}",
                path.display()
            );
            assert_sealed(&path);
        }
    }
}

#[test]
fn assembles_a_complete_generation() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let assembled = fixture.apply(&fixture.system(), &Host::default()).unwrap();
    assert_eq!(assembled.generation, 1);
    let dir = fixture.generation_dir(1);
    let p = &fixture.packages;

    let generation = fixture.env.root.generation(1).unwrap();
    assert!(generation.etc.managed.contains(&"hostname".to_owned()));
    assert!(generation.etc.managed.contains(&"runit/1".to_owned()));
    assert!(generation.etc.copy.contains(&"passwd".to_owned()));
    assert_eq!(generation.initrd_packages[0].id, p.busybox.id);
    assert!(generation.carried_modules.is_empty());

    assert_eq!(
        fs::read_to_string(dir.join("etc/hostname")).unwrap(),
        "laft-test\n"
    );
    assert_eq!(fs::read_to_string(dir.join("etc/motd")).unwrap(), "hello\n");
    assert_eq!(
        inode(&dir.join("etc/pkg.conf")),
        inode(&fixture.store_file(&p.config, "etc/pkg.conf"))
    );
    assert_eq!(
        fs::read_link(dir.join("etc/runit/1")).unwrap(),
        Path::new("/run/laft/booted/services/runit/stage/1")
    );
    assert!(dir.join("services/runit/stage/1").is_file());
    assert!(dir.join("services/spec/udevd.json").is_file());
    assert_eq!(mode(&dir.join("accounts.json")), 0o600);
    assert!(Declared::read(&dir).unwrap().users.contains_key("root"));

    let release = format!("usr/lib/modules/{RELEASE}");
    let records = trigger::read_records(&dir).unwrap();
    assert_eq!(records, assembled.triggers);
    let depmod = &records[&format!("depmod:{RELEASE}")];
    assert_eq!(depmod.status, laft_store::TriggerStatus::Ok);
    assert_eq!(depmod.outputs, [format!("{release}/modules.dep")]);
    assert_eq!(
        fs::read_to_string(dir.join(&release).join("modules.dep")).unwrap(),
        "kernel/fs/ext4.ko.zst:\nkernel/drivers/nvme/nvme.ko:\n"
    );
    let fonts = &records["fontconfig"];
    assert_eq!(
        fonts.outputs,
        [
            "usr/lib/fontconfig",
            "usr/lib/fontconfig/cache",
            "usr/lib/fontconfig/cache/fonts.cache"
        ]
    );
    assert_eq!(
        fs::read_to_string(dir.join("usr/lib/fontconfig/cache/fonts.cache")).unwrap(),
        "-s -f\n"
    );
    assert_eq!(
        mode(&dir.join("usr/lib/fontconfig/cache/fonts.cache")) & 0o222,
        0
    );
    assert_eq!(records["desktop"].status, laft_store::TriggerStatus::Failed);
    assert_eq!(
        records["glib-schemas"].status,
        laft_store::TriggerStatus::Skipped
    );
    assert_eq!(
        fs::read_to_string(fixture.store_file(&p.app, "usr/share/applications/app.desktop"))
            .unwrap(),
        "[Desktop Entry]\n",
        "a trigger cannot write to a store file"
    );
    assert!(assembled.warnings.iter().any(|w| matches!(
        w, Warning::TriggerFailed { instance, reason } if instance == "desktop" && reason.contains("Permission denied")
    )));

    let doas = dir.join("usr/bin/doas");
    assert_ne!(
        inode(&doas),
        inode(&fixture.store_file(&p.laft, "usr/bin/doas"))
    );
    assert_eq!(fs::read(&doas).unwrap(), b"doas binary");
    assert_eq!(mode(&doas), 0o4755);
    assert_eq!(mode(&fixture.store_file(&p.laft, "usr/bin/doas")), 0o555);
    let privileged = PrivilegedRecord::read(&dir).unwrap();
    assert_eq!(privileged, assembled.privileged);
    assert_eq!(privileged.files.len(), 1);
    assert_eq!(privileged.files[0].path, "/usr/bin/doas");
    assert_eq!(privileged.files[0].source, p.laft.id);
    assert!(
        assembled
            .warnings
            .contains(&Warning::InactivePrivileged("/usr/bin/missing".into()))
    );
    if !crate::fsx::is_root() {
        assert!(
            assembled
                .warnings
                .contains(&Warning::PrivilegesNotApplied(vec!["/usr/bin/doas".into()]))
        );
    }

    let boot = BootRecord::read(&dir).unwrap();
    assert_eq!(boot, assembled.boot);
    assert_eq!(boot.cmdline, ["laft.gen=1", "quiet", "loglevel=3"]);
    assert_eq!(boot.kernel_id, p.linux.id);
    assert_eq!(boot.kernel_release, RELEASE);
    assert_eq!(boot.modules, ["ext4", "nvme"]);
    assert_eq!(boot.firmware, ["/lib/firmware/test/fw.bin"]);
    let initramfs = fs::read(dir.join("boot/initramfs")).unwrap();
    assert_eq!(
        laft_core::Blake3Hash::of(&initramfs).to_string()[3..35],
        boot.initramfs
    );
    let archive = read_cpio(&zstd::stream::decode_all(initramfs.as_slice()).unwrap());
    assert_eq!(archive["init"], (0o100755, b"laft-boot binary".to_vec()));
    assert_eq!(archive["bin/busybox"].1, b"busybox binary");
    assert_eq!(archive["bin/sh"], (0o120777, b"busybox".to_vec()));
    assert_eq!(archive["dev/console"].0, 0o020600);
    assert_eq!(archive["lib/firmware/test/fw.bin"].1, b"fw");
    let ext4 = &archive[&format!("lib/modules/{RELEASE}/kernel/fs/ext4.ko")].1;
    assert!(ext4.starts_with(b"\x7fELF"), "modules are decompressed");
    assert_eq!(json(&dir.join("boot/boot.json"))["version"], 1);
    assert_eq!(archive["usr/bin/e2fsck"].1, b"e2fsck binary");
    let laft_boot: serde_json::Value =
        serde_json::from_slice(&archive["laft-boot.json"].1).unwrap();
    assert_eq!(laft_boot["release"], RELEASE);
    assert_eq!(
        laft_boot["modules"],
        serde_json::json!([
            format!("/lib/modules/{RELEASE}/kernel/fs/ext4.ko"),
            format!("/lib/modules/{RELEASE}/kernel/drivers/nvme/nvme.ko"),
        ])
    );
    assert_eq!(laft_boot["root"]["fsck"], "/usr/bin/e2fsck");
    assert_eq!(laft_boot["root"]["options"], "noatime");
    assert_eq!(laft_boot["esp"]["type"], "vfat");

    assert_sealed(&dir);
    let tmp: Vec<_> = fs::read_dir(fixture.env.path("laft/tmp"))
        .unwrap()
        .collect();
    assert!(tmp.is_empty());
}

#[test]
fn unchanged_triggers_reuse_their_outputs_and_the_initramfs() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let first = fixture.apply(&fixture.system(), &Host::default()).unwrap();
    let second = fixture.apply(&fixture.system(), &Host::default()).unwrap();
    assert_eq!(first.triggers, second.triggers);
    let dep = format!("usr/lib/modules/{RELEASE}/modules.dep");
    assert_eq!(
        inode(&fixture.generation_dir(1).join(&dep)),
        inode(&fixture.generation_dir(2).join(&dep))
    );
    let cache = "usr/lib/fontconfig/cache/fonts.cache";
    assert_eq!(
        inode(&fixture.generation_dir(1).join(cache)),
        inode(&fixture.generation_dir(2).join(cache))
    );
    assert_eq!(first.boot.initramfs, second.boot.initramfs);
    assert_eq!(
        inode(&fixture.generation_dir(1).join("boot/initramfs")),
        inode(&fixture.generation_dir(2).join("boot/initramfs"))
    );
}

#[test]
fn modules_of_the_running_kernel_are_carried_into_a_new_kernel() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.apply(&fixture.system(), &Host::default()).unwrap();
    fs::create_dir_all(fixture.env.path("run/laft")).unwrap();
    std::os::unix::fs::symlink("mounts/1", fixture.env.path("run/laft/booted")).unwrap();

    let newer = linux("6.2.0-laft1").build(&fixture.pkgs);
    let mut packages = fixture.system();
    packages.retain(|p| p.name != "linux");
    packages.push(&newer);
    let host = Host {
        kernel: Some(RELEASE.into()),
        ..Host::default()
    };
    let assembled = fixture.apply(&packages, &host).unwrap();

    let dir = fixture.generation_dir(2);
    let old = format!("usr/lib/modules/{RELEASE}/kernel/fs/ext4.ko.zst");
    assert_eq!(
        inode(&dir.join(&old)),
        inode(&fixture.store_file(&fixture.packages.linux, &old))
    );
    let generation = fixture.env.root.generation(2).unwrap();
    assert_eq!(generation.carried_modules.len(), 1);
    assert_eq!(generation.carried_modules[0].kernel, RELEASE);
    assert_eq!(generation.carried_modules[0].id, fixture.packages.linux.id);
    assert!(
        assembled
            .triggers
            .contains_key(&format!("depmod:{RELEASE}"))
    );
    assert!(assembled.triggers.contains_key("depmod:6.2.0-laft1"));
    assert!(
        dir.join(format!("usr/lib/modules/{RELEASE}/modules.dep"))
            .is_file()
    );
    assert_eq!(assembled.boot.kernel_release, "6.2.0-laft1");
    assert_eq!(assembled.boot.kernel_id, newer.id);

    let third = fixture.apply(
        &packages,
        &Host {
            kernel: Some("6.2.0-laft1".into()),
            ..Host::default()
        },
    );
    assert!(
        third
            .unwrap()
            .triggers
            .keys()
            .all(|k| k != &format!("depmod:{RELEASE}"))
    );
}

#[test]
fn a_failing_required_trigger_fails_the_assembly_and_leaves_nothing() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let broken = Pkg::new("kmod", "34")
        .file(
            "usr/bin/depmod",
            b"#!/bin/sh\necho 'cannot index' >&2\nexit 1\n",
        )
        .build(&fixture.pkgs);
    let mut packages = fixture.system();
    packages.retain(|p| p.name != "kmod");
    packages.push(&broken);
    let error = fixture.apply(&packages, &Host::default()).unwrap_err();
    match error {
        Error::Trigger { instance, reason } => {
            assert_eq!(instance, format!("depmod:{RELEASE}"));
            assert!(reason.contains("cannot index"), "{reason}");
        }
        other => panic!("{other}"),
    }
    let lock = fixture.env.lock();
    assert_eq!(lock.recover().unwrap().len(), 1);
    assert!(fixture.env.root.generations().unwrap().is_empty());
    assert_eq!(
        fs::read_dir(fixture.env.path("laft/tmp")).unwrap().count(),
        0
    );
}

#[test]
fn interrupted_assembly_recovers_to_a_consistent_tree() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    for &point in fault::POINTS.iter().filter(|p| p.starts_with("assemble.")) {
        fault::fail_at(Some(point));
        let result = fixture.apply(&fixture.system(), &Host::default());
        fault::fail_at(None);
        assert!(
            matches!(result, Err(Error::Injected(p)) if p == point),
            "{point}"
        );
        let lock = fixture.env.lock();
        assert_eq!(lock.recover().unwrap().len(), 1, "{point}");
        drop(lock);
        let numbers = fixture.env.root.generations().unwrap();
        assert!(numbers.is_empty(), "{point}: {numbers:?}");
        assert_eq!(
            fs::read_dir(fixture.env.path("laft/tmp")).unwrap().count(),
            0
        );
    }
    let assembled = fixture.apply(&fixture.system(), &Host::default()).unwrap();
    assert_eq!(
        fixture.env.root.default_generation().unwrap(),
        Some(assembled.generation)
    );
}

#[test]
fn privileged_files_reach_only_views_with_the_same_closure() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let other = fixture.pkgs.join("other");
    fs::create_dir_all(&other).unwrap();
    let other_shell = shell_package(&[("usr/share/doc/shell/NEWS", b"news")])
        .unwrap()
        .build(&other);
    let p = &fixture.packages;
    let lock = fixture.env.lock();
    let mut tx = Transaction::begin(&lock, &fixture.plan(&["same", "older"])).unwrap();
    fixture.add(&mut tx, &fixture.system());
    tx.add(&other_shell.path, &other_shell.id).unwrap();
    let view = |name: &str, shell: &Built| ViewInput {
        name: name.parse().unwrap(),
        packages: [&p.laft, shell]
            .iter()
            .map(|b| PackageInput {
                dir: b.dir.clone(),
                reasons: vec![],
            })
            .collect(),
        alternatives: BTreeMap::new(),
        users: BTreeMap::new(),
        groups: BTreeMap::new(),
    };
    let same = tx.assemble_view(&view("same", &p.shell)).unwrap();
    let older = tx.assemble_view(&view("older", &other_shell)).unwrap();
    let views = BTreeMap::from([
        ("same".parse().unwrap(), same),
        ("older".parse().unwrap(), older),
    ]);
    let input = fixture.input(&fixture.system(), views);
    let assembled = assemble(&lock, &mut tx, &fixture.assembly(&input, &Host::default())).unwrap();
    tx.make_default().unwrap();
    let views = &assembled.privileged.views;
    assert_eq!(views[&"same".parse().unwrap()].paths, ["/usr/bin/doas"]);
    assert_eq!(views[&"same".parse().unwrap()].version, same);
    assert!(views[&"older".parse().unwrap()].paths.is_empty());
    let names: BTreeSet<_> = views.keys().map(ToString::to_string).collect();
    assert_eq!(
        names,
        BTreeSet::from(["older".to_owned(), "same".to_owned()])
    );
}

#[test]
fn recovery_renders_the_boot_menu_again() {
    let env = Env::new();
    let lock = env.lock();
    let mut tx = Transaction::begin(
        &lock,
        &Plan {
            command: "apply".into(),
            previous: None,
            views: BTreeSet::new(),
        },
    )
    .unwrap();
    tx.assemble_generation(&GenerationInput {
        packages: vec![],
        initrd_packages: vec![],
        carry_over: vec![],
        alternatives: BTreeMap::new(),
        views: BTreeMap::new(),
        message: None,
        laft_version: "0.1.0".into(),
        arch: "x86_64-v3".into(),
        init: Init {
            backend: "runit".into(),
            exec: vec!["/usr/bin/runit".into()],
        },
        config: b"{}".to_vec(),
        config_files: vec![],
        indexes: vec![],
        etc: Default::default(),
    })
    .unwrap();
    tx.boot_installed().unwrap();
    drop(tx);
    let esp = env.path("boot");
    fs::create_dir_all(&esp).unwrap();
    let recovered = recover(&lock, &esp).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].outcome, laft_store::Outcome::Built);
    assert!(esp.join("limine.conf").is_file());
    assert_eq!(
        fs::read_to_string(esp.join("laft/entries.json")).unwrap(),
        r#"{"default_gen":null,"entries":[],"version":1}"#
    );
    assert!(recover(&lock, &esp).unwrap().is_empty());
}
