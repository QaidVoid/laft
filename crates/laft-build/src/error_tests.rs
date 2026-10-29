use super::*;

#[test]
fn only_permission_and_missing_feature_errors_point_at_namespaces() {
    use rustix::io::Errno;
    for errno in [Errno::PERM, Errno::ACCESS, Errno::NOSPC, Errno::NOSYS] {
        assert!(refused(&errno.into()), "{errno}");
    }
    for errno in [Errno::NOENT, Errno::NOTDIR, Errno::INVAL] {
        assert!(!refused(&errno.into()), "{errno}");
    }
}
