use std::fs;
use std::os::unix::fs::symlink;

use super::*;

#[test]
fn device_modules_come_from_every_driver_above_the_device() {
    let dir = laft_test::tempdir();
    let sys = dir.path();
    let disk = sys.join("devices/pci0000:00/0000:00:1d.0/nvme/nvme0/nvme0n1/nvme0n1p2");
    fs::create_dir_all(&disk).unwrap();
    fs::create_dir_all(sys.join("module/nvme")).unwrap();
    fs::create_dir_all(sys.join("module/nvme_core")).unwrap();
    fs::create_dir_all(sys.join("bus/pci/drivers/nvme")).unwrap();
    fs::create_dir_all(sys.join("dev/block")).unwrap();
    let pci = sys.join("devices/pci0000:00/0000:00:1d.0");
    symlink(sys.join("bus/pci/drivers/nvme"), pci.join("driver")).unwrap();
    symlink(
        sys.join("module/nvme"),
        sys.join("bus/pci/drivers/nvme/module"),
    )
    .unwrap();
    let ctrl = pci.join("nvme/nvme0");
    fs::create_dir_all(sys.join("bus/nvme/drivers/nvme-core")).unwrap();
    symlink(sys.join("bus/nvme/drivers/nvme-core"), ctrl.join("driver")).unwrap();
    symlink(
        sys.join("module/nvme_core"),
        sys.join("bus/nvme/drivers/nvme-core/module"),
    )
    .unwrap();
    symlink(&disk, sys.join("dev/block/259:2")).unwrap();

    let found = device_modules(sys, &[(259, 2), (8, 1)]);
    assert_eq!(
        found,
        BTreeSet::from(["nvme".to_owned(), "nvme_core".to_owned()])
    );
}

#[test]
fn file_system_and_luks_drivers_the_kernel_lacks_fail_the_build() {
    let index = ModuleIndex::parse(
        "kernel/fs/ext4/ext4.ko.zst:\nkernel/drivers/md/dm-mod.ko.zst:\n",
        "kernel/fs/vfat.ko\nkernel/fs/nls/nls_cp437.ko\nkernel/fs/nls/nls_iso8859-1.ko\n",
        "",
        "",
    )
    .unwrap();
    let with_root = |fstype: &str| {
        let mut config = BootConfig::default();
        config.fs.insert(
            "/".into(),
            Fs {
                fstype: fstype.into(),
                ..Fs::default()
            },
        );
        config.fs.insert(
            config.boot.esp.clone(),
            Fs {
                fstype: "vfat".into(),
                ..Fs::default()
            },
        );
        config
    };
    let host = Host::default();
    let (names, _) = requested(&with_root("ext4"), &host, &index, "6.1", &mut Vec::new()).unwrap();
    assert!(
        names.contains("ext4") && names.contains("vfat"),
        "{names:?}"
    );

    let err = requested(&with_root("btrfs"), &host, &index, "6.1", &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("no module btrfs"), "{err}");

    let mut luks = with_root("ext4");
    luks.luks.insert(
        "root".into(),
        crate::config::Luks {
            device: "UUID=x".into(),
            options: BTreeSet::new(),
        },
    );
    let err = requested(&luks, &host, &index, "6.1", &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("no module dm_crypt"), "{err}");
}

#[test]
fn a_host_only_initramfs_without_host_modules_takes_the_generic_drivers() {
    let index = ModuleIndex::parse(
        "kernel/drivers/nvme/host/nvme.ko.zst:\nkernel/sound/snd.ko.zst:\n",
        "",
        "",
        "",
    )
    .unwrap();
    let config = BootConfig::default();
    assert!(config.boot.initrd.hostonly);

    let mut warnings = Vec::new();
    let (_, dirs) = requested(&config, &Host::default(), &index, "6.1", &mut warnings).unwrap();
    assert_eq!(dirs, modules::GENERIC_DIRS);
    assert_eq!(warnings, [Warning::NoHostModules]);

    let host = Host {
        modules: BTreeSet::from(["nvme".to_owned()]),
        ..Host::default()
    };
    let mut warnings = Vec::new();
    let (names, dirs) = requested(&config, &host, &index, "6.1", &mut warnings).unwrap();
    assert!(
        dirs.is_empty() && names.contains("nvme"),
        "{dirs:?} {names:?}"
    );
    assert!(warnings.is_empty());
}
