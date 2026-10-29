use std::os::unix::fs::symlink;
use std::process::Command;
use std::thread;

use super::*;

#[test]
fn a_read_only_tree_is_removed_on_drop() {
    let dir = tempdir();
    let deep = dir.path().join("laft/store/abc/usr/bin");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("hello"), b"hi").unwrap();
    for path in [
        "laft/store/abc/usr/bin",
        "laft/store/abc/usr",
        "laft/store/abc",
        "laft/store",
    ] {
        fs::set_permissions(dir.path().join(path), fs::Permissions::from_mode(0o555)).unwrap();
    }
    let path = dir.path().to_path_buf();
    drop(dir);
    assert!(!path.exists());
}

#[test]
fn symlinks_are_not_followed() {
    let outside = tempdir();
    fs::set_permissions(outside.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let dir = tempdir();
    symlink(outside.path(), dir.path().join("link")).unwrap();
    make_writable(dir.path()).unwrap();
    let mode = fs::metadata(outside.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o555);
}

#[test]
fn a_written_program_runs_at_once_while_other_threads_spawn() {
    let dir = tempdir();
    thread::scope(|scope| {
        for worker in 0..4 {
            let dir = dir.path();
            scope.spawn(move || {
                for round in 0..50 {
                    let program = dir.join(format!("program-{worker}"));
                    let script = format!("#!/bin/sh\necho {round}\n");
                    write_program(&program, script.as_bytes());
                    let output = Command::new(&program).output().expect("the program runs");
                    assert_eq!(output.stdout, format!("{round}\n").as_bytes());
                }
            });
        }
    });
    let mode = fs::metadata(dir.path().join("program-0"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}
