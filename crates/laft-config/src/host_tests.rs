use super::*;

#[test]
fn package_references_are_written_canonically() {
    assert_eq!(parse_pkg("llvm@19:dynamic").as_deref(), Ok("llvm@19"));
    assert_eq!(parse_pkg("ripgrep:static").as_deref(), Ok("ripgrep:static"));
    assert!(parse_pkg("ripgrep:shared").is_err());
    assert!(parse_pkg("Ripgrep").is_err());
}

#[test]
fn license_identifiers_are_single_spdx_ids() {
    for id in ["MIT", "Apache-2.0", "GPL-2.0+", "LicenseRef-laft"] {
        assert_eq!(parse_spdx(id).as_deref(), Ok(id));
    }
    for id in ["", "+", "MIT_X", "GPL-2.0++"] {
        assert!(parse_spdx(id).is_err(), "{id}");
    }
}
