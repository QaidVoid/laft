use rustix::io::Errno;

/// Whether a sandbox error means the kernel or its policy forbids the
/// namespaces, rather than a bug.
pub(crate) fn namespaces_refused(error: &crate::Error) -> bool {
    use crate::error::Step;
    matches!(error.step(), Step::Unshare | Step::MapIds)
        && matches!(
            Errno::from_io_error(error.io_error()),
            Some(Errno::PERM | Errno::ACCESS | Errno::NOSPC | Errno::INVAL | Errno::NOSYS)
        )
}
