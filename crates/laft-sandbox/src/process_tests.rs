use rustix::fs::{Mode, OFlags};
use rustix::io::FdFlags;

use crate::test_support::in_child;
use crate::{close_inherited_on_exec, group_ids, reset_signal_dispositions, user_ids};

#[test]
fn inherited_descriptors_are_closed_on_exec_but_stdio_is_not() {
    let (status, message) = in_child(|| {
        let file = match rustix::fs::open(c"/dev/null", OFlags::RDONLY, Mode::empty()) {
            Ok(file) => file,
            Err(e) => return (1, format!("opening /dev/null: {e}")),
        };
        let stderr = std::io::stderr();
        let before = rustix::io::fcntl_getfd(&stderr);
        if let Err(e) = close_inherited_on_exec() {
            return (1, e.to_string());
        }
        match rustix::io::fcntl_getfd(&file) {
            Ok(flags) if flags.contains(FdFlags::CLOEXEC) => {}
            other => return (1, format!("the inherited descriptor has {other:?}")),
        }
        if rustix::io::fcntl_getfd(&stderr) != before {
            return (1, "the flags of stderr changed".to_owned());
        }
        (0, String::new())
    });
    assert_eq!(status, Some(0), "{message}");
}

#[test]
fn an_ignored_signal_kills_again_after_the_reset() {
    let (status, message) = in_child(|| {
        // SAFETY: installing `SIG_IGN` registers no handler code.
        unsafe { libc::signal(libc::SIGUSR1, libc::SIG_IGN) };
        reset_signal_dispositions();
        if let Err(e) =
            rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::USR1)
        {
            return (1, format!("sending SIGUSR1: {e}"));
        }
        (0, "SIGUSR1 was still ignored".to_owned())
    });
    assert_eq!(status, None, "{message}");
}

#[test]
fn ids_report_the_process_credentials() {
    let uids = user_ids().unwrap();
    let gids = group_ids().unwrap();
    assert_eq!(uids[0], rustix::process::getuid().as_raw());
    assert_eq!(uids[1], rustix::process::geteuid().as_raw());
    assert_eq!(gids[0], rustix::process::getgid().as_raw());
    assert_eq!(gids[1], rustix::process::getegid().as_raw());
}
