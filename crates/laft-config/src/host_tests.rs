use super::*;

#[test]
fn package_references_are_written_canonically() {
    assert_eq!(parse_pkg("llvm@19:dynamic").as_deref(), Ok("llvm@19"));
    assert_eq!(parse_pkg("ripgrep:static").as_deref(), Ok("ripgrep:static"));
    assert!(parse_pkg("ripgrep:shared").is_err());
    assert!(parse_pkg("Ripgrep").is_err());
}
