use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Output;

use laft_test::TempDir;
use rustix::io::Errno;

use crate::error::Step;
use crate::test_support::sandbox_refused;
use crate::{Error, FixedSandbox, NOBODY};

/// A tree whose `usr` holds the host's `dash` as `usr/bin/sh` with the C
/// library it needs, a writable `usr/out` and a read-only `usr/data/file`,
/// next to a configuration directory with `marker`.
struct Tree {
    dir: TempDir,
}

impl Tree {
    /// The tree, or `None` after printing why the host cannot provide a
    /// shell for it.
    fn new() -> Option<Self> {
        let dir = laft_test::tempdir();
        let usr = dir.path().join("tree/usr");
        for sub in ["bin", "lib", "out", "data"] {
            fs::create_dir_all(usr.join(sub)).unwrap();
        }
        let files = [
            ("/usr/bin/dash", "bin/sh"),
            ("/lib64/ld-linux-x86-64.so.2", "lib/ld-linux-x86-64.so.2"),
            ("/usr/lib/libc.so.6", "lib/libc.so.6"),
        ];
        for (host, inside) in files {
            let Ok(bytes) = fs::read(host) else {
                println!("skipping: the host has no {host} to build a minimal tree from");
                return None;
            };
            laft_test::write_program(&usr.join(inside), &bytes);
        }
        // The host's loader may look in `/usr/lib64` rather than `/usr/lib`.
        std::os::unix::fs::symlink("lib", usr.join("lib64")).unwrap();
        fs::write(usr.join("data/file"), b"store\n").unwrap();
        fs::set_permissions(usr.join("data/file"), fs::Permissions::from_mode(0o444)).unwrap();
        if rustix::process::geteuid().is_root() {
            let nobody = Some(rustix::process::Uid::from_raw(NOBODY));
            rustix::fs::chown(usr.join("out"), nobody, None).unwrap();
        }
        fs::create_dir(dir.path().join("etc")).unwrap();
        fs::write(dir.path().join("etc/marker"), b"etc\n").unwrap();
        Some(Self { dir })
    }

    fn tree(&self) -> PathBuf {
        self.dir.path().join("tree")
    }

    fn sandbox(&self) -> FixedSandbox {
        FixedSandbox::new(self.tree(), self.dir.path().join("etc")).writable("usr/out")
    }

    /// Runs `script` with the tree's shell, or returns `None` after printing
    /// why this environment cannot run it.
    fn run(&self, script: &str) -> Option<Output> {
        supported(
            self.sandbox()
                .command("/usr/bin/sh")
                .arg("-c")
                .arg(script)
                .output(),
        )
    }
}

fn supported<T>(result: Result<T, Error>) -> Option<T> {
    let error = match result {
        Ok(value) => return Some(value),
        Err(error) => error,
    };
    if let Some(reason) = sandbox_refused(&error) {
        println!("skipping: {reason}");
        return None;
    }
    if error.step() == Step::Exec
        && error.io_error().and_then(Errno::from_io_error) == Some(Errno::NOENT)
    {
        println!("skipping: the host's shell does not run from a minimal tree: {error}");
        return None;
    }
    panic!("{error}")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn program_runs_as_nobody_with_a_fixed_environment() {
    let Some(tree) = Tree::new() else { return };
    let Some(output) = tree.run(
        "while read key real rest; do [ \"$key\" = Uid: ] && echo \"uid $real\"; done < /proc/self/status; \
         echo \"$PATH $LC_ALL $HOME $SOURCE_DATE_EPOCH\"; echo $PWD",
    ) else {
        return;
    };
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        format!("uid {NOBODY}\n/usr/bin C.UTF-8 /nonexistent 1\n/\n")
    );
}

#[test]
fn only_writable_directories_and_scratch_mounts_accept_writes() {
    let Some(tree) = Tree::new() else { return };
    let Some(output) = tree.run(
        "for p in /usr/out/new /usr/data/new /usr/data/file /etc/new /new /tmp/new /var/new /home/new /root/new; do \
         if echo x > $p; then echo \"wrote $p\"; fi; done 2>/dev/null",
    ) else {
        return;
    };
    assert_eq!(
        stdout(&output),
        "wrote /usr/out/new\nwrote /tmp/new\nwrote /var/new\nwrote /home/new\nwrote /root/new\n"
    );
    assert_eq!(fs::read(tree.tree().join("usr/out/new")).unwrap(), b"x\n");
    assert_eq!(
        fs::read(tree.tree().join("usr/data/file")).unwrap(),
        b"store\n"
    );
    assert!(!tree.tree().join("usr/data/new").exists());
}

#[test]
fn etc_and_the_runtime_links_resolve_in_the_tree() {
    let Some(tree) = Tree::new() else { return };
    let Some(output) = tree.run(
        "read line < /etc/marker; echo $line; read line < /run/laft/system/usr/data/file; echo $line; \
         [ -x /run/laft/booted/usr/bin/sh ] && echo booted",
    ) else {
        return;
    };
    assert_eq!(stdout(&output), "etc\nstore\nbooted\n");
}

#[test]
fn a_writable_directory_must_not_be_reached_through_a_symlink() {
    let Some(tree) = Tree::new() else { return };
    let outside = tree.dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, tree.tree().join("usr/link")).unwrap();
    let sandbox = FixedSandbox::new(tree.tree(), tree.dir.path().join("etc")).writable("usr/link");
    let error = sandbox.command("/usr/bin/sh").output().unwrap_err();
    assert_eq!(error.step(), Step::InvalidWritable);
    for bad in ["etc", "usr", "usr/../out", "/usr/out"] {
        let sandbox = FixedSandbox::new(tree.tree(), tree.dir.path().join("etc")).writable(bad);
        let error = sandbox.command("/usr/bin/sh").output().unwrap_err();
        assert_eq!(error.step(), Step::InvalidWritable, "{bad}");
    }
}

#[test]
fn files_written_by_the_program_belong_to_the_caller() {
    let Some(tree) = Tree::new() else { return };
    let Some(output) = tree.run("echo x > /usr/out/owned") else {
        return;
    };
    assert!(output.status.success());
    let owner = fs::metadata(Path::new(&tree.tree()).join("usr/out/owned"))
        .unwrap()
        .uid();
    let expected = if rustix::process::geteuid().is_root() {
        NOBODY
    } else {
        rustix::process::getuid().as_raw()
    };
    assert_eq!(owner, expected);
}
