use std::collections::BTreeSet;

use super::*;

const DEP: &str = "\
kernel/drivers/nvme/host/nvme.ko.zst: kernel/drivers/nvme/host/nvme-core.ko.zst kernel/drivers/nvme/common/nvme-auth.ko.zst
kernel/drivers/nvme/host/nvme-core.ko.zst: kernel/drivers/nvme/common/nvme-auth.ko.zst
kernel/drivers/nvme/common/nvme-auth.ko.zst:
kernel/drivers/md/dm-crypt.ko.zst: kernel/drivers/md/dm-mod.ko.zst
kernel/drivers/md/dm-mod.ko.zst:
kernel/fs/ext4/ext4.ko.zst: kernel/fs/jbd2/jbd2.ko.zst kernel/lib/crc16.ko.zst
kernel/fs/jbd2/jbd2.ko.zst:
kernel/lib/crc16.ko.zst:
kernel/crypto/crc32c_generic.ko.zst:
kernel/drivers/hid/usbhid/usbhid.ko.zst:
kernel/sound/snd.ko.zst:
";

const ORDER: &str = "\
kernel/lib/crc16.ko
kernel/fs/jbd2/jbd2.ko
kernel/fs/ext4/ext4.ko
kernel/drivers/md/dm-mod.ko
kernel/drivers/md/dm-crypt.ko
kernel/drivers/nvme/common/nvme-auth.ko
kernel/drivers/nvme/host/nvme-core.ko
kernel/drivers/nvme/host/nvme.ko
";

fn index(builtin: &str, softdep: &str) -> ModuleIndex {
    ModuleIndex::parse(DEP, builtin, softdep, ORDER).unwrap()
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|n| (*n).to_owned()).collect()
}

#[test]
fn dependencies_load_first() {
    let order = index("", "").select(&names(&["nvme", "ext4"]), &[]);
    assert_eq!(
        order,
        ["crc16", "jbd2", "ext4", "nvme_auth", "nvme_core", "nvme"]
    );
}

#[test]
fn builtin_and_missing_modules_are_left_out() {
    let index = index("kernel/fs/ext4/ext4.ko\n", "");
    assert!(index.is_builtin("ext4"));
    let order = index.select(&names(&["ext4", "vfat", "dm-crypt"]), &[]);
    assert_eq!(order, ["dm_mod", "dm_crypt"]);
}

#[test]
fn soft_dependencies_join_and_load_before() {
    let index = index("", "softdep ext4 pre: crc32c_generic post: snd\n");
    let order = index.select(&names(&["ext4"]), &[]);
    assert_eq!(order, ["crc16", "jbd2", "crc32c_generic", "ext4"]);
}

#[test]
fn directories_select_every_module_below_them() {
    let order = index("", "").select(
        &BTreeSet::new(),
        &["kernel/drivers/nvme/", "kernel/drivers/hid/"],
    );
    assert_eq!(order, ["nvme_auth", "nvme_core", "nvme", "usbhid"]);
}

#[test]
fn malformed_dependency_lines_are_rejected() {
    assert!(ModuleIndex::parse("kernel/a.ko\n", "", "", "").is_err());
}

#[test]
fn module_names_drop_suffixes_and_dashes() {
    assert_eq!(
        module_name("kernel/drivers/nvme/host/nvme-core.ko.zst"),
        "nvme_core"
    );
    assert_eq!(module_name("kernel/fs/ext4/ext4.ko"), "ext4");
    assert_eq!(module_name("a.ko.xz"), "a");
}

#[test]
fn modinfo_values_are_found_by_key() {
    let modinfo = b"license=GPL\0firmware=i915/a.bin\0firmware=i915/b.bin\0firmwarex=no\0";
    assert_eq!(
        modinfo_values(modinfo, "firmware"),
        ["i915/a.bin", "i915/b.bin"]
    );
}

#[test]
fn a_dependency_cycle_still_lists_every_module_in_build_order() {
    let index = ModuleIndex::parse(
        "kernel/a.ko:\nkernel/b.ko:\nkernel/c.ko: kernel/a.ko\n",
        "",
        "softdep a pre: b\nsoftdep b pre: a\n",
        "kernel/b.ko\nkernel/a.ko\nkernel/c.ko\n",
    )
    .unwrap();
    assert_eq!(index.select(&names(&["c"]), &[]), ["b", "a", "c"]);
}
