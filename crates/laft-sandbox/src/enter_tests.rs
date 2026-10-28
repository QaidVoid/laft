use std::fs::{self, File};
use std::os::fd::AsFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use rustix::fs::StatVfsMountFlags;
use rustix::io::Errno;
use rustix::mount::{MountFlags, MountPropagationFlags};
use rustix::thread::LinkNameSpaceType;

use crate::enter_view;
use crate::error::Step;
use crate::test_support::in_user_namespace;

/// Turns a failed expectation into the test's failure message.
macro_rules! ensure {
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err(format!($($message)+));
        }
    };
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Mounts a tmpfs over `base`, shared so that later mounts below it
/// propagate, and fills it with a system generation and a view:
///
/// - `gen/usr/bin/doas` and `gen/usr/bin/passwd`, setuid, and
///   `gen/usr/bin/sh`
/// - `gen/etc`
/// - `view/usr/bin/doas` and `view/usr/bin/tool`
///
/// The tmpfs is `nosuid`, so privileged files only honor setuid when the
/// clear works. It belongs to the test's user namespace, which may therefore
/// clear `nosuid` as real root could on the host.
fn generation_and_view(base: &Path) -> Result<(), String> {
    rustix::mount::mount(c"tmpfs", base, c"tmpfs", MountFlags::NOSUID, None).map_err(text)?;
    rustix::mount::mount_change(base, MountPropagationFlags::SHARED).map_err(text)?;
    for dir in ["gen/usr/bin", "gen/etc", "view/usr/bin"] {
        fs::create_dir_all(base.join(dir)).map_err(text)?;
    }
    for (file, contents) in [
        ("gen/usr/bin/doas", "system doas"),
        ("gen/usr/bin/passwd", "system passwd"),
        ("gen/usr/bin/sh", "system sh"),
        ("view/usr/bin/doas", "view doas"),
        ("view/usr/bin/tool", "view tool"),
    ] {
        fs::write(base.join(file), contents).map_err(text)?;
    }
    for file in ["gen/usr/bin/doas", "gen/usr/bin/passwd"] {
        fs::set_permissions(base.join(file), fs::Permissions::from_mode(0o4755)).map_err(text)?;
    }
    Ok(())
}

fn enter(base: &Path, privileged: &[&Path]) -> Result<(), crate::Error> {
    enter_view(&base.join("gen"), &base.join("view/usr"), privileged)
}

fn mount_flags(path: &Path) -> Result<StatVfsMountFlags, String> {
    Ok(rustix::fs::statvfs(path).map_err(text)?.f_flag)
}

#[test]
fn view_replaces_usr_with_the_privileged_files_on_top() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    in_user_namespace(|| {
        generation_and_view(base)?;
        let privileged = [Path::new("/usr/bin/doas"), Path::new("/usr/bin/passwd")];
        enter(base, &privileged).map_err(text)?;

        let usr = base.join("gen/usr");
        ensure!(
            fs::read_to_string(usr.join("bin/tool")).map_err(text)? == "view tool",
            "the view does not cover usr"
        );
        ensure!(
            !usr.join("bin/sh").exists(),
            "the system's usr shows through"
        );
        ensure!(
            !usr.join("bin/passwd").exists(),
            "a privileged path the view lacks was added"
        );
        let flags = mount_flags(&usr)?;
        ensure!(
            flags.contains(
                StatVfsMountFlags::RDONLY | StatVfsMountFlags::NOSUID | StatVfsMountFlags::NODEV
            ),
            "the view is mounted with {flags:?}"
        );
        let refused = fs::write(usr.join("bin/new"), "x").map_err(|e| e.raw_os_error());
        ensure!(
            refused == Err(Some(Errno::ROFS.raw_os_error())),
            "writing to the view gave {refused:?}"
        );

        let doas = usr.join("bin/doas");
        ensure!(
            fs::read_to_string(&doas).map_err(text)? == "system doas",
            "the privileged file is the view's"
        );
        ensure!(
            fs::metadata(&doas).map_err(text)?.mode() & 0o4000 != 0,
            "the privileged file lost its setuid bit"
        );
        let flags = mount_flags(&doas)?;
        ensure!(
            flags.contains(StatVfsMountFlags::RDONLY | StatVfsMountFlags::NODEV)
                && !flags.contains(StatVfsMountFlags::NOSUID),
            "the privileged file is mounted with {flags:?}"
        );
        Ok(())
    });
}

#[test]
fn caller_namespace_is_untouched_and_its_later_mounts_propagate_in() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    in_user_namespace(|| {
        generation_and_view(base)?;
        let outer = File::open("/proc/self/ns/mnt").map_err(text)?;
        enter(base, &[Path::new("/usr/bin/doas")]).map_err(text)?;
        let inner = File::open("/proc/self/ns/mnt").map_err(text)?;

        rustix::thread::move_into_link_name_space(outer.as_fd(), Some(LinkNameSpaceType::Mount))
            .map_err(text)?;
        let usr = base.join("gen/usr");
        ensure!(
            usr.join("bin/sh").exists() && !usr.join("bin/tool").exists(),
            "the view leaked into the caller's namespace"
        );
        let etc = base.join("gen/etc");
        rustix::mount::mount(c"tmpfs", &etc, c"tmpfs", MountFlags::empty(), None).map_err(text)?;

        rustix::thread::move_into_link_name_space(inner.as_fd(), Some(LinkNameSpaceType::Mount))
            .map_err(text)?;
        let generation = fs::metadata(base.join("gen")).map_err(text)?;
        ensure!(
            fs::metadata(&etc).map_err(text)?.dev() != generation.dev(),
            "a mount made outside did not propagate into the view"
        );
        Ok(())
    });
}

#[test]
fn malformed_privileged_paths_are_rejected_before_unsharing() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    in_user_namespace(|| {
        generation_and_view(base)?;
        let before = File::open("/proc/self/ns/mnt").map_err(text)?;
        for path in ["usr/bin/doas", "/etc/shadow", "/usr", "/usr/../etc/shadow"] {
            let result = enter(base, &[Path::new(path)]);
            ensure!(
                matches!(&result, Err(error) if error.step() == Step::InvalidPath),
                "{path} gave {result:?}"
            );
        }
        let after = File::open("/proc/self/ns/mnt").map_err(text)?;
        let same = rustix::fs::fstat(&before).map_err(text)?.st_ino
            == rustix::fs::fstat(&after).map_err(text)?.st_ino;
        ensure!(same, "a rejected call created a mount namespace");
        Ok(())
    });
}

#[test]
fn privileged_paths_must_be_regular_files() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    in_user_namespace(|| {
        generation_and_view(base)?;
        let result = enter(base, &[Path::new("/usr/bin")]);
        ensure!(
            matches!(&result, Err(error) if error.step() == Step::NotRegularFile),
            "a directory gave {result:?}"
        );
        Ok(())
    });
}
