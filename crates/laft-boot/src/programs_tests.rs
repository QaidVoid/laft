use std::path::Path;

use super::{FsckOutcome, cryptsetup_args, fsck_outcome};
use crate::config::{Luks, LuksOption};

#[test]
fn cryptsetup_opens_the_device_with_its_options() {
    let luks = Luks {
        device: "UUID=x".into(),
        name: "cryptroot".into(),
        options: vec![
            LuksOption::Discard,
            LuksOption::NoReadWorkqueue,
            LuksOption::NoWriteWorkqueue,
        ],
    };
    let args = cryptsetup_args(Path::new("/dev/nvme0n1p2"), &luks);
    assert_eq!(
        args,
        [
            "open",
            "--type",
            "luks",
            "--allow-discards",
            "--perf-no_read_workqueue",
            "--perf-no_write_workqueue",
            "/dev/nvme0n1p2",
            "cryptroot"
        ]
    );
}

#[test]
fn cryptsetup_without_options_only_opens() {
    let luks = Luks {
        device: "/dev/sda2".into(),
        name: "root".into(),
        options: vec![],
    };
    assert_eq!(
        cryptsetup_args(Path::new("/dev/sda2"), &luks),
        ["open", "--type", "luks", "/dev/sda2", "root"]
    );
}

#[test]
fn fsck_exit_codes_follow_the_fsck_convention() {
    assert_eq!(fsck_outcome(Some(0)), FsckOutcome::Continue);
    assert_eq!(fsck_outcome(Some(1)), FsckOutcome::Continue);
    assert_eq!(fsck_outcome(Some(2)), FsckOutcome::Reboot);
    assert_eq!(fsck_outcome(Some(3)), FsckOutcome::Reboot);
    for code in [4, 5, 8, 12, 16, 32, 128] {
        assert_eq!(fsck_outcome(Some(code)), FsckOutcome::Repair, "{code}");
    }
    assert_eq!(fsck_outcome(None), FsckOutcome::Repair);
}
