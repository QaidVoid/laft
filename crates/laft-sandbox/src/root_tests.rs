use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Stdio;

use super::*;
use crate::test_support::sandbox_refused;

/// A tree whose `usr` is the host's `/usr`, bound read-only below `/run`,
/// so that real programs run inside it.
fn tree(dir: &Path) -> PathBuf {
    let root = dir.join("root");
    fs::create_dir(&root).unwrap();
    symlink("/run/usr", root.join("usr")).unwrap();
    for (link, target) in [("bin", "usr/bin"), ("lib", "usr/lib"), ("lib64", "usr/lib")] {
        symlink(target, root.join(link)).unwrap();
    }
    root
}

#[test]
fn a_command_runs_as_root_of_the_tree_and_sees_the_binds() {
    let dir = laft_test::tempdir();
    let root = tree(dir.path());
    let (data, out) = (dir.path().join("data"), dir.path().join("out"));
    fs::create_dir(&data).unwrap();
    fs::create_dir(&out).unwrap();
    fs::write(data.join("hello"), "hello\n").unwrap();
    let script = "echo made > /made && cp /run/data/hello /run/out/copy && id -u && \
                  ! touch /run/data/new 2>/dev/null && test ! -e /home && test -c /dev/null && \
                  test -d /proc/self && pwd";
    let output = RootSandbox::new(&root)
        .bind("/usr", "usr", false)
        .bind(&data, "data", false)
        .bind(&out, "out", true)
        .command("/usr/bin/sh")
        .args(["-c", script])
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => match sandbox_refused(&error) {
            Some(reason) => {
                println!("skipping: {reason}");
                return;
            }
            None => panic!("the sandbox fails: {error}"),
        },
    };
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "0\n/\n");
    assert_eq!(fs::read_to_string(root.join("made")).unwrap(), "made\n");
    assert_eq!(fs::read_to_string(out.join("copy")).unwrap(), "hello\n");
    assert!(!data.join("new").exists());
    for dir in MOUNT_POINTS {
        let path = root.join(dir);
        assert!(
            path.is_dir() && fs::read_dir(&path).unwrap().next().is_none(),
            "{dir}"
        );
    }
}

#[test]
fn a_bind_name_with_a_slash_is_refused() {
    let dir = laft_test::tempdir();
    let root = tree(dir.path());
    let error = RootSandbox::new(&root)
        .bind("/usr", "a/b", false)
        .command("/usr/bin/true")
        .output()
        .expect_err("the name is not a file name");
    assert!(error.to_string().contains("a/b"), "{error}");
}
