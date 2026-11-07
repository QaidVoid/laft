use std::io;

use rustix::io::Errno;

use super::{Error, Step};

fn refused(step: Step, errno: Errno) -> bool {
    Error::new(step, io::Error::from_raw_os_error(errno.raw_os_error())).refuses_namespaces()
}

#[test]
fn only_the_namespace_steps_count_as_a_refusal() {
    assert!(refused(Step::Unshare, Errno::PERM));
    assert!(refused(Step::Unshare, Errno::ACCESS));
    assert!(refused(Step::MapIds, Errno::NOSPC));
    assert!(refused(Step::MapIds, Errno::NOSYS));

    // A later failure says nothing about the namespaces even with one of
    // the same errnos: when a machine denies the build account /dev/null,
    // the spawn of a sandbox fails with EACCES at the command's own step.
    assert!(!refused(Step::Exec, Errno::ACCESS));
    assert!(!refused(Step::Exec, Errno::PERM));
    assert!(!refused(Step::CloneTree, Errno::ACCESS));
    assert!(!refused(Step::MountProc, Errno::NOSPC));
    assert!(!refused(Step::Unshare, Errno::NOENT));
}
