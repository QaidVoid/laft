//! The local repository key on a root that is mounted read-only.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use laft_test::tempdir;

mod support;

use support::{repo_tool, s};

/// Why this environment cannot run the test, or `None` when it can: the
/// test bind-mounts the root read-only in a user and mount namespace.
fn unavailable() -> Option<String> {
    let status = Command::new("unshare")
        .args(["--user", "--map-root-user", "--mount", "true"])
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => None,
        _ => Some("user and mount namespaces are unavailable".into()),
    }
}

/// The configuration the root needs to be usable.
fn configuration(published: &Path, key: &str) -> String {
    format!(
        "system {{\n\
         repo core {{ url = \"file://{}\", key = \"{key}\" }}\n\
         fs \"/\" {{ device = \"UUID=11111111-1111-1111-1111-111111111111\", fstype = ext4 }}\n\
         }}\n",
        published.display(),
    )
}

/// Runs `laft` inside a namespace whose view of `root` is a read-only
/// bind mount, and returns its output.
fn key_of_read_only(root: &Path) -> (i32, String, String) {
    let script = format!(
        "mount --bind '{r}' '{r}' && mount -o remount,bind,ro '{r}' && \
         exec \"$LAFT\" --root '{r}' repo key local",
        r = root.display(),
    );
    let output = Command::new("unshare")
        .args(["--user", "--map-root-user", "--mount", "sh", "-c", &script])
        .env("LAFT", env!("CARGO_BIN_EXE_laft"))
        .env("NO_COLOR", "1")
        .env_remove("LAFT_CONFIG")
        .stdin(Stdio::null())
        .output()
        .expect("unshare runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn the_local_key_is_readable_on_a_read_only_root() {
    if let Some(reason) = unavailable() {
        println!("skipping: {reason}");
        return;
    }
    let dir = tempdir();
    let key = dir.path().join("online");
    repo_tool(&["keygen", "--unencrypted", "--out", s(&key)]);
    let public = fs::read_to_string(key.with_extension("pub")).expect("public key");
    let public = public.lines().last().expect("key line");
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("etc/laft")).expect("configuration directory");
    fs::write(
        root.join("etc/laft/system.stav"),
        configuration(&dir.path().join("www"), public),
    )
    .expect("configuration");
    // The writable first run generates the local key and, with it, the
    // lock file of the root.
    let first = Command::new(env!("CARGO_BIN_EXE_laft"))
        .args([
            "--root",
            root.to_str().expect("UTF-8"),
            "repo",
            "key",
            "local",
        ])
        .env("NO_COLOR", "1")
        .env_remove("LAFT_CONFIG")
        .stdin(Stdio::null())
        .output()
        .expect("laft runs");
    assert!(
        first.status.success(),
        "the writable run failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let listed = String::from_utf8_lossy(&first.stdout).into_owned();

    let (code, stdout, stderr) = key_of_read_only(&root);
    assert_eq!(
        code, 0,
        "the read-only run failed: {stderr}\nexpected the key {listed}"
    );
    assert_eq!(stdout.trim(), listed.trim());
}
