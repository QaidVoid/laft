use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rustix::mount::MountFlags;

use super::{Op, activate, execute, kernel_filesystems, move_kernel_filesystems};

fn position(ops: &[Op], wanted: impl Fn(&Op) -> bool) -> usize {
    ops.iter()
        .position(wanted)
        .expect("the operation is planned")
}

#[test]
fn kernel_filesystems_are_mounted_hardened_before_run_laft_is_created() {
    let ops = kernel_filesystems(Path::new("/"));
    let mounts: Vec<_> = ops
        .iter()
        .filter_map(|op| match op {
            Op::Mount {
                target,
                fstype,
                flags,
                data,
                ..
            } => Some((
                target.to_str().unwrap(),
                fstype.as_str(),
                *flags,
                data.as_str(),
            )),
            _ => None,
        })
        .collect();
    let hardened = MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC;
    assert_eq!(
        mounts,
        [
            ("/proc", "proc", hardened, ""),
            ("/sys", "sysfs", hardened, ""),
            ("/dev", "devtmpfs", MountFlags::NOSUID, "mode=0755"),
            (
                "/run",
                "tmpfs",
                MountFlags::NOSUID | MountFlags::NODEV,
                "mode=0755"
            ),
        ]
    );
    assert_eq!(ops.last(), Some(&Op::MkdirAll("/run/laft".into())));
}

#[test]
fn generation_mount_honors_setuid_and_laft_does_not() {
    let ops = activate(Path::new("/"), Path::new("/newroot"), 42, "runit");
    assert!(ops.contains(&Op::RemountBind {
        target: "/run/laft/mounts/42".into(),
        flags: MountFlags::RDONLY | MountFlags::NODEV,
    }));
    assert!(ops.contains(&Op::RemountBind {
        target: "/newroot/laft".into(),
        flags: MountFlags::RDONLY | MountFlags::NOSUID | MountFlags::NODEV,
    }));
}

#[test]
fn generation_is_mounted_from_laft_before_laft_becomes_read_only() {
    let ops = activate(Path::new("/"), Path::new("/newroot"), 42, "runit");
    let mkdir = position(&ops, |op| *op == Op::MkdirAll("/run/laft/mounts/42".into()));
    let bind_gen = position(&ops, |op| {
        *op == Op::Bind {
            source: "/newroot/laft/gens/42".into(),
            target: "/run/laft/mounts/42".into(),
        }
    });
    let bind_laft = position(
        &ops,
        |op| matches!(op, Op::Bind { target, .. } if target == Path::new("/newroot/laft")),
    );
    let ro_laft = position(
        &ops,
        |op| matches!(op, Op::RemountBind { target, .. } if target == Path::new("/newroot/laft")),
    );
    assert!(mkdir < bind_gen && bind_gen < bind_laft && bind_laft < ro_laft);
    assert_eq!(ro_laft, ops.len() - 1);
}

#[test]
fn runtime_links_are_relative_and_name_the_booted_generation() {
    let ops = activate(Path::new("/"), Path::new("/newroot"), 42, "runit");
    for name in ["system", "booted"] {
        assert!(ops.contains(&Op::Symlink {
            target: "mounts/42".into(),
            link: Path::new("/run/laft").join(name),
        }));
    }
    assert!(ops.contains(&Op::Write {
        path: "/run/laft/init-backend".into(),
        contents: "runit\n".into(),
    }));
}

#[test]
fn kernel_filesystems_move_into_the_new_root_with_run_last() {
    let ops = move_kernel_filesystems(Path::new("/"), Path::new("/newroot"));
    let mut expected = Vec::new();
    for name in ["dev", "proc", "sys", "run"] {
        expected.push(Op::MkdirAll(Path::new("/newroot").join(name)));
        expected.push(Op::Move {
            source: Path::new("/").join(name),
            target: Path::new("/newroot").join(name),
        });
    }
    assert_eq!(ops, expected);
}

const CHILD: &str = "LAFT_BOOT_NAMESPACE_DIR";
const SKIP: &str = "laft-boot-test-skip:";

/// Runs the mount plans for real, in a new user and mount namespace that a
/// child copy of this test runs in, against a temporary directory.
#[test]
fn mount_sequence_runs_in_a_namespace() {
    if let Some(dir) = std::env::var_os(CHILD) {
        mount_sequence(Path::new(&dir));
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new("unshare")
        .args([
            "--user",
            "--map-root-user",
            "--mount",
            "--propagation",
            "private",
        ])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "plan::tests::mount_sequence_runs_in_a_namespace",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, tmp.path())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(e) => return println!("skipping: cannot run unshare: {e}"),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    match output.status.code() {
        Some(0) => {
            if let Some(reason) = stdout.lines().find_map(|l| l.strip_prefix(SKIP)) {
                println!("skipping:{reason}");
            }
        }
        Some(101) => panic!("the namespace test failed:\n{stdout}\n{stderr}"),
        _ => println!(
            "skipping: user namespaces are unavailable: {}",
            stderr.trim()
        ),
    }
}

fn mount_sequence(dir: &Path) {
    let tmpfs = |target: PathBuf| Op::Mount {
        source: "tmpfs".into(),
        target,
        fstype: "tmpfs".into(),
        flags: MountFlags::empty(),
        data: String::new(),
    };
    if let Err(e) = execute(&[tmpfs(dir.to_owned())]) {
        return println!("{SKIP} mounting is refused in user namespaces: {e}");
    }
    let root = dir.join("initramfs");
    let newroot = dir.join("newroot");
    let gen_dir = newroot.join("laft/gens/7");
    fs::create_dir_all(gen_dir.join("usr/bin")).unwrap();
    fs::write(gen_dir.join("usr/bin/marker"), "7").unwrap();

    let mut setup = Vec::new();
    for name in super::KERNEL_FILESYSTEMS {
        setup.push(Op::MkdirAll(root.join(name)));
        setup.push(tmpfs(root.join(name)));
    }
    setup.push(Op::MkdirAll(root.join("run/laft")));
    execute(&setup).unwrap();
    execute(&activate(&root, &newroot, 7, "runit")).unwrap();
    execute(&move_kernel_filesystems(&root, &newroot)).unwrap();

    let run = newroot.join("run/laft");
    assert_eq!(
        fs::read_link(run.join("system")).unwrap(),
        Path::new("mounts/7")
    );
    assert_eq!(
        fs::read_link(run.join("booted")).unwrap(),
        Path::new("mounts/7")
    );
    assert_eq!(
        fs::read_to_string(run.join("init-backend")).unwrap(),
        "runit\n"
    );
    assert_eq!(
        fs::read_to_string(run.join("system/usr/bin/marker")).unwrap(),
        "7"
    );

    let mounts = mount_options();
    let options = |path: &Path| {
        mounts
            .iter()
            .rev()
            .find(|(point, _)| point == path)
            .map(|(_, options)| options.clone())
            .unwrap_or_else(|| panic!("{} is not a mount point", path.display()))
    };
    let generation = options(&run.join("mounts/7"));
    assert!(generation.contains(&"ro".into()) && generation.contains(&"nodev".into()));
    assert!(!generation.contains(&"nosuid".into()), "{generation:?}");
    let laft = options(&newroot.join("laft"));
    for flag in ["ro", "nosuid", "nodev"] {
        assert!(laft.contains(&flag.into()), "{laft:?}");
    }
    for name in super::KERNEL_FILESYSTEMS {
        options(&newroot.join(name));
        assert!(!mounts.iter().any(|(p, _)| *p == root.join(name)));
    }
    let err = fs::write(newroot.join("laft/new"), "x").unwrap_err();
    assert_eq!(
        err.raw_os_error(),
        Some(rustix::io::Errno::ROFS.raw_os_error())
    );
}

/// Every mount point of this mount namespace with its per-mount options.
fn mount_options() -> Vec<(PathBuf, Vec<String>)> {
    fs::read_to_string("/proc/self/mountinfo")
        .unwrap()
        .lines()
        .map(|line| {
            let fields: Vec<_> = line.split(' ').collect();
            let options = fields[5].split(',').map(str::to_owned).collect();
            (PathBuf::from(fields[4]), options)
        })
        .collect()
}
