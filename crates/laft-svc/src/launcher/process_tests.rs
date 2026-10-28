use std::collections::BTreeMap;
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

use super::{ensure_dir, environment, place_fd, write_atomic};
use crate::launcher::accounts::Credentials;
use crate::validate::resolve;
use crate::{Error, Service};

fn me() -> (u32, u32) {
    (
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw(),
    )
}

#[test]
fn environment_is_minimal_and_the_service_may_override_it() {
    let service = Service {
        env: BTreeMap::from([
            ("PATH".into(), "/opt/bin:/usr/bin".into()),
            ("RUST_LOG".into(), "info".into()),
        ]),
        ..Service::longrun(&["/usr/bin/x"])
    };
    let credentials = Credentials {
        user: "myapp".into(),
        uid: 101,
        gid: 101,
        groups: vec![101],
        home: "/var/lib/myapp".into(),
        shell: "/usr/bin/nologin".into(),
    };
    let env = environment(&resolve("x", &service), &credentials);
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        pairs,
        [
            ("HOME", "/var/lib/myapp"),
            ("LOGNAME", "myapp"),
            ("PATH", "/opt/bin:/usr/bin"),
            ("RUST_LOG", "info"),
            ("SHELL", "/usr/bin/nologin"),
            ("USER", "myapp"),
        ]
    );
}

#[test]
fn directories_are_created_and_their_mode_corrected() {
    let root = tempfile::tempdir().expect("can create a temporary directory");
    let (uid, gid) = me();
    ensure_dir(root.path(), "/var/lib", "app/data", uid, gid, 0o750).expect("creates");
    let parent = fs::metadata(root.path().join("var/lib/app")).expect("exists");
    let data = root.path().join("var/lib/app/data");
    assert_eq!(parent.mode() & 0o7777, 0o755);
    assert_eq!(fs::metadata(&data).expect("exists").mode() & 0o7777, 0o750);
    fs::set_permissions(&data, fs::Permissions::from_mode(0o777)).expect("chmod");
    ensure_dir(root.path(), "/var/lib", "app/data", uid, gid, 0o750).expect("corrects");
    let corrected = fs::metadata(&data).expect("exists");
    assert_eq!(corrected.mode() & 0o7777, 0o750);
    assert_eq!((corrected.uid(), corrected.gid()), (uid, gid));
}

#[test]
fn symlinked_directory_is_refused() {
    let root = tempfile::tempdir().expect("can create a temporary directory");
    let elsewhere = root.path().join("elsewhere");
    fs::create_dir(&elsewhere).expect("mkdir");
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o700)).expect("chmod");
    fs::create_dir_all(root.path().join("var/lib")).expect("mkdir");
    symlink(&elsewhere, root.path().join("var/lib/app")).expect("symlink");
    let (uid, gid) = me();
    let result = ensure_dir(root.path(), "/var/lib", "app", uid, gid, 0o777);
    assert!(matches!(result, Err(Error::Io { .. })), "{result:?}");
    let mode = fs::metadata(&elsewhere).expect("exists").mode() & 0o7777;
    assert_eq!(mode, 0o700);
}

#[test]
fn readiness_descriptor_lands_on_its_number_and_survives_exec() {
    let (_reader, writer) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).expect("pipe");
    let target = 900;
    let placed = place_fd("x", &writer, target).expect("places");
    assert_eq!(placed.as_raw_fd(), 900);
    let flags = rustix::io::fcntl_getfd(&placed).expect("flags");
    assert!(!flags.contains(rustix::io::FdFlags::CLOEXEC));
    let again = place_fd("x", &writer, target);
    assert!(matches!(again, Err(Error::Setup { .. })), "{again:?}");
}

#[test]
fn atomic_write_replaces_the_file_with_the_mode() {
    let dir = tempfile::tempdir().expect("can create a temporary directory");
    let path = dir.path().join("config");
    fs::write(&path, "old").expect("write");
    write_atomic(&path, b"s5000000\nn5\n", 0o644).expect("writes");
    assert_eq!(fs::read(&path).expect("read"), b"s5000000\nn5\n");
    assert_eq!(fs::metadata(&path).expect("exists").mode() & 0o7777, 0o644);
    assert_eq!(fs::read_dir(dir.path()).expect("read").count(), 1);
}
