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
