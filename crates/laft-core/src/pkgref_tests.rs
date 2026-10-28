use super::*;

#[test]
fn parses_every_combination_of_parts() {
    let cases = [
        ("ripgrep", "ripgrep", None, Variant::Dynamic),
        ("ripgrep:static", "ripgrep", None, Variant::Static),
        ("llvm@19", "llvm", Some("19"), Variant::Dynamic),
        ("llvm@19.1:static", "llvm", Some("19.1"), Variant::Static),
        ("openssl1.1", "openssl1.1", None, Variant::Dynamic),
    ];
    for (input, name, prefix, variant) in cases {
        let r: PkgRef = input.parse().expect("test reference is valid");
        assert_eq!(r.name().as_str(), name, "{input}");
        assert_eq!(r.prefix().map(VersionPrefix::as_str), prefix, "{input}");
        assert_eq!(r.variant(), variant, "{input}");
    }
}

#[test]
fn display_round_trips_and_omits_the_default_variant() {
    for input in ["ripgrep", "ripgrep:static", "llvm@19", "llvm@19.1:static"] {
        let r: PkgRef = input.parse().expect("test reference is valid");
        assert_eq!(r.to_string(), input);
    }
    let r: PkgRef = "zlib:dynamic"
        .parse()
        .expect("explicit default variant is valid");
    assert_eq!(r.to_string(), "zlib");
}

#[test]
fn rejects_malformed_references() {
    for bad in [
        "",
        "@19",
        "llvm@",
        "llvm:",
        "llvm:shared",
        "llvm@19@20",
        "llvm:static:static",
        "llvm:static@19",
    ] {
        assert!(bad.parse::<PkgRef>().is_err(), "{bad:?} should be rejected");
    }
}
