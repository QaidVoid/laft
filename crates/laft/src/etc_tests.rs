use super::*;

#[test]
fn paths_name_files_below_etc_only() {
    assert_eq!(relative("/etc/ssh/sshd_config").unwrap(), "ssh/sshd_config");
    assert_eq!(relative("hosts").unwrap(), "hosts");
    for bad in [
        "../shadow",
        "/etc/../shadow",
        "//etc/x",
        "a//b",
        "/usr/x",
        "/etc/",
        "./x",
    ] {
        assert!(relative(bad).is_err(), "{bad}");
    }
}
