use rustix::mount::MountFlags;

use super::parse;

#[test]
fn generic_options_become_flags_and_the_rest_stays_data_in_order() {
    let (flags, data) =
        parse("noatime,compress=zstd:3,nosuid,rw,subvol=@,nodev,lazytime,defaults,ssd");
    assert_eq!(
        flags,
        MountFlags::NOATIME | MountFlags::NOSUID | MountFlags::NODEV | MountFlags::LAZYTIME
    );
    assert_eq!(data, "compress=zstd:3,subvol=@,ssd");
}

#[test]
fn every_flag_option_is_recognized() {
    let (flags, data) = parse(
        "ro,nosuid,nodev,noexec,sync,dirsync,noatime,nodiratime,relatime,strictatime,lazytime",
    );
    assert_eq!(
        flags,
        MountFlags::RDONLY
            | MountFlags::NOSUID
            | MountFlags::NODEV
            | MountFlags::NOEXEC
            | MountFlags::SYNCHRONOUS
            | MountFlags::DIRSYNC
            | MountFlags::NOATIME
            | MountFlags::NODIRATIME
            | MountFlags::RELATIME
            | MountFlags::STRICTATIME
            | MountFlags::LAZYTIME
    );
    assert_eq!(data, "");
}

#[test]
fn empty_options_mean_defaults() {
    assert_eq!(parse(""), (MountFlags::empty(), String::new()));
    assert_eq!(
        parse(",,umask=0077,"),
        (MountFlags::empty(), "umask=0077".into())
    );
}
