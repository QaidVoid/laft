use std::os::unix::fs::PermissionsExt;

use laft_test::TempDir;

use super::*;

/// A temporary root whose `usr/bin` programs log their arguments to
/// `calls`, one line per call, and exit with the status in
/// `status-<name>` when that file exists.
struct Root {
    dir: TempDir,
}

impl Root {
    fn new(programs: &[&str]) -> Self {
        let dir = laft_test::tempdir();
        let root = Self { dir };
        for name in programs {
            root.program(name);
        }
        root.write(
            "proc/self/mountinfo",
            "22 1 0:21 / / rw - ext4 /dev/sda1 rw\n",
        );
        root
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn program(&self, name: &str) {
        let script = format!(
            "#!/bin/sh\necho \"{name} $*\" >> '{log}'\nstatus='{dir}/status-{name}'\nif [ -f \"$status\" ]; then read code < \"$status\"; exit \"$code\"; fi\nexit 0\n",
            log = self.path().join("calls").display(),
            dir = self.path().display(),
        );
        let path = self.path().join("usr/bin").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        laft_test::write_program(&path, script.as_bytes());
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        let calls = fs::read_to_string(self.path().join("calls")).unwrap_or_default();
        let root = self.path().display().to_string();
        calls.lines().map(|l| l.replace(&root, "")).collect()
    }
}

#[test]
fn every_step_of_the_base_part_parses() {
    for (text, step) in [
        ("kernel-fs", Step::KernelFs),
        ("kmod", Step::Kmod),
        ("udev-trigger", Step::UdevTrigger),
        ("hostname", Step::Hostname),
        ("mount-fs", Step::MountFs),
        ("random-seed", Step::RandomSeed),
        ("console", Step::Console),
    ] {
        assert_eq!(text.parse::<Step>(), Ok(step));
    }
    assert!("swap".parse::<Step>().is_err());
    assert_eq!("down".parse::<Action>(), Ok(Action::Down));
    assert!("restart".parse::<Action>().is_err());
}

#[test]
fn kernel_filesystems_are_mounted_once_and_efivars_only_on_efi() {
    let root = Root::new(&["mount"]);
    let info = "1 0 0:1 / /dev/pts rw - devpts devpts rw\n";
    let targets = |r: &Root| -> Vec<&str> {
        kernel_mounts(r.path(), info)
            .iter()
            .map(|m| m.target)
            .collect()
    };
    assert_eq!(
        targets(&root),
        ["/dev/shm", "/sys/fs/cgroup", "/sys/kernel/security"]
    );
    fs::create_dir_all(root.path().join("sys/firmware/efi")).unwrap();
    assert!(targets(&root).contains(&"/sys/firmware/efi/efivars"));

    run(root.path(), Step::KernelFs, Action::Up).unwrap();
    assert!(root.path().join("dev/pts").is_dir());
    assert_eq!(
        root.calls()[0],
        "mount -t devpts -o nosuid,noexec,gid=5,mode=0620,ptmxmode=0666 devpts /dev/pts"
    );
    assert_eq!(root.calls().len(), 5);
}

#[test]
fn modules_come_from_etc_before_usr_lib_each_once() {
    let root = Root::new(&["modprobe"]);
    root.write(
        "usr/lib/modules-load.d/a.conf",
        "# comment\nfuse\n\n; old\nloop\n",
    );
    root.write("usr/lib/modules-load.d/b.conf", "hidden\n");
    root.write("etc/modules-load.d/b.conf", "loop\nkvm\n");
    root.write("etc/modules-load.d/c.txt", "ignored\n");
    assert_eq!(modules(root.path()).unwrap(), ["fuse", "loop", "kvm"]);
    run(root.path(), Step::Kmod, Action::Up).unwrap();
    assert_eq!(root.calls(), ["modprobe -a -b fuse loop kvm"]);
}

#[test]
fn no_modules_runs_nothing() {
    let root = Root::new(&["modprobe"]);
    run(root.path(), Step::Kmod, Action::Up).unwrap();
    assert!(root.calls().is_empty());
}

#[test]
fn udev_replays_subsystems_then_devices_and_settles() {
    let root = Root::new(&["udevadm"]);
    run(root.path(), Step::UdevTrigger, Action::Up).unwrap();
    assert_eq!(
        root.calls(),
        [
            "udevadm trigger --action=add --type=subsystems",
            "udevadm trigger --action=add --type=devices",
            "udevadm settle",
        ]
    );
    root.write("status-udevadm", "1");
    assert!(matches!(
        run(root.path(), Step::UdevTrigger, Action::Up),
        Err(Error::Program { .. })
    ));
}

#[test]
fn the_host_name_comes_from_the_first_line_of_etc_hostname() {
    let root = Root::new(&[]);
    root.write("etc/hostname", "box-1.lan\nignored\n");
    root.write("proc/sys/kernel/hostname", "old");
    run(root.path(), Step::Hostname, Action::Up).unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("proc/sys/kernel/hostname")).unwrap(),
        "box-1.lan"
    );
    for bad in ["", "a b", "x/y"] {
        root.write("etc/hostname", bad);
        assert!(matches!(
            read_hostname(root.path()),
            Err(Error::Malformed { .. })
        ));
    }
}

const FSTAB: &str = "# Generated by laft.\n\
    UUID=r / ext4 defaults 0 1\n\
    UUID=h /home ext4 defaults 0 2\n\
    LABEL=data /srv xfs defaults 0 0\n\
    /dev/sdc1 /backup ext4 noauto 0 2\n\
    UUID=b /boot vfat defaults 0 2\n\
    UUID=s none swap defaults 0 0\n";

#[test]
fn filesystems_are_checked_then_mounted_like_mount_a() {
    let root = Root::new(&["e2fsck", "mount"]);
    root.write("etc/fstab", FSTAB);
    let warnings = run(root.path(), Step::MountFs, Action::Up).unwrap();
    assert_eq!(
        root.calls(),
        [
            "e2fsck -p /dev/disk/by-uuid/h",
            "mount /home",
            "mount /srv",
            "mount /boot",
        ]
    );
    assert_eq!(
        warnings,
        ["/boot is mounted unchecked, because fsck.vfat is not installed"]
    );
}

#[test]
fn a_filesystem_that_fails_its_check_stays_unmounted() {
    let root = Root::new(&["e2fsck", "mount", "fsck.vfat"]);
    root.write("etc/fstab", FSTAB);
    root.write("status-e2fsck", "4");
    let error = run(root.path(), Step::MountFs, Action::Up).unwrap_err();
    assert!(
        matches!(&error, Error::Filesystems(f) if f.len() == 1 && f[0].0 == "/home"),
        "{error}"
    );
    assert!(root.calls().contains(&"mount /boot".to_owned()));
    assert!(!root.calls().contains(&"mount /home".to_owned()));

    root.write("status-e2fsck", "1");
    run(root.path(), Step::MountFs, Action::Up).expect("corrected errors still mount");
}

#[test]
fn mounted_filesystems_are_skipped_and_unmounted_in_reverse() {
    let root = Root::new(&["e2fsck", "mount", "umount"]);
    root.write("etc/fstab", FSTAB);
    root.write(
        "proc/self/mountinfo",
        "1 0 0:1 / / rw - ext4 x rw\n2 1 0:2 / /home rw - ext4 x rw\n3 1 0:3 / /boot rw - vfat x rw\n",
    );
    run(root.path(), Step::MountFs, Action::Up).unwrap();
    assert_eq!(root.calls(), ["mount /srv"]);
    fs::remove_file(root.path().join("calls")).unwrap();
    run(root.path(), Step::MountFs, Action::Down).unwrap();
    assert_eq!(root.calls(), ["umount /boot", "umount /home"]);
}

#[test]
fn tags_name_their_udev_links() {
    assert_eq!(device_path("UUID=1-2"), "/dev/disk/by-uuid/1-2");
    assert_eq!(device_path("LABEL=data"), "/dev/disk/by-label/data");
    assert_eq!(device_path("PARTUUID=a"), "/dev/disk/by-partuuid/a");
    assert_eq!(device_path("PARTLABEL=b"), "/dev/disk/by-partlabel/b");
    assert_eq!(device_path("/dev/sda1"), "/dev/sda1");
}

#[test]
fn the_random_seed_is_fed_once_and_replaced() {
    let root = Root::new(&[]);
    root.write("dev/urandom", "");
    run(root.path(), Step::RandomSeed, Action::Up).unwrap();
    assert_eq!(fs::read(root.path().join("dev/urandom")).unwrap(), b"");
    let seed_path = root.path().join(RANDOM_SEED);
    let first = fs::read(&seed_path).unwrap();
    assert_eq!(first.len(), SEED_BYTES);
    assert_eq!(
        fs::metadata(&seed_path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    run(root.path(), Step::RandomSeed, Action::Up).unwrap();
    assert_eq!(fs::read(root.path().join("dev/urandom")).unwrap(), first);
    let second = fs::read(&seed_path).unwrap();
    assert_ne!(first, second, "a seed is never used twice");

    run(root.path(), Step::RandomSeed, Action::Down).unwrap();
    assert_ne!(fs::read(&seed_path).unwrap(), second);
}

#[test]
fn the_console_gets_its_keymap_and_font() {
    let root = Root::new(&["loadkeys", "setfont"]);
    run(root.path(), Step::Console, Action::Up).unwrap();
    assert!(root.calls().is_empty(), "no vconsole.conf sets nothing");
    root.write("etc/vconsole.conf", "KEYMAP=de-latin1\nFONT=\"ter-v22n\"\n");
    run(root.path(), Step::Console, Action::Up).unwrap();
    assert_eq!(root.calls(), ["loadkeys -q de-latin1", "setfont ter-v22n"]);
}

#[test]
fn steps_without_down_do_nothing_on_down() {
    let root = Root::new(&["mount", "modprobe", "udevadm"]);
    for step in [
        Step::KernelFs,
        Step::Kmod,
        Step::UdevTrigger,
        Step::Hostname,
        Step::Console,
    ] {
        run(root.path(), step, Action::Down).unwrap();
    }
    assert!(root.calls().is_empty());
}
