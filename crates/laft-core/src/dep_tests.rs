use proptest::prelude::*;

use super::*;

fn constraint(op: Op, version: &str, rev: Option<u32>) -> Constraint {
    Constraint {
        op,
        version: version.parse().expect("test version is valid"),
        rev: rev.and_then(NonZeroU32::new),
    }
}

fn name(s: &str) -> Name {
    s.parse().expect("test name is valid")
}

#[test]
fn parses_every_dependency_form() {
    let cases = [
        ("so:libz.so.1", Dep::So("libz.so.1".parse().expect("valid"))),
        (
            "pc:zlib>=1.3",
            Dep::Pc(
                "zlib".parse().expect("valid"),
                vec![constraint(Op::Ge, "1.3", None)],
            ),
        ),
        (
            "pc:gtk+-3.0",
            Dep::Pc("gtk+-3.0".parse().expect("valid"), vec![]),
        ),
        ("virtual:java-runtime", Dep::Virtual(name("java-runtime"))),
        ("zlib", Dep::Pkg(name("zlib"), VersionReq::Any)),
        (
            "zlib>=1.3,<2",
            Dep::Pkg(
                name("zlib"),
                VersionReq::from_constraints(vec![
                    constraint(Op::Ge, "1.3", None),
                    constraint(Op::Lt, "2", None),
                ]),
            ),
        ),
        (
            "zlib=1.3.1-r2",
            Dep::Pkg(
                name("zlib"),
                VersionReq::from_constraints(vec![constraint(Op::Eq, "1.3.1", Some(2))]),
            ),
        ),
        (
            "llvm@19",
            Dep::Pkg(
                name("llvm"),
                VersionReq::Prefix("19".parse().expect("valid")),
            ),
        ),
        (
            "openssl1.1<=1.1.1w",
            Dep::Pkg(
                name("openssl1.1"),
                VersionReq::from_constraints(vec![constraint(Op::Le, "1.1.1w", None)]),
            ),
        ),
    ];
    for (input, expected) in cases {
        let dep: Dep = input.parse().expect("test dependency is valid");
        assert_eq!(dep, expected, "{input}");
        assert_eq!(dep.to_string(), input);
    }
}

#[test]
fn rejects_malformed_dependencies() {
    for bad in [
        "",
        "so:",
        "so:lib/z.so",
        "pc:",
        "pc:zlib@1",
        "virtual:",
        "virtual:Java",
        "zlib>=",
        "zlib>=1.3,",
        "zlib=>1.3",
        "zlib@19>=1",
        "zlib@",
        "zlib=1.3-r0",
        "zlib=1.3-r01",
        "zlib=1.3-r",
        "zlib:static",
        "Zlib",
    ] {
        assert!(bad.parse::<Dep>().is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn parses_every_provide_form() {
    for input in [
        "so:libz.so.1",
        "pc:zlib",
        "pc:zlib=1.3.1",
        "virtual:java-runtime",
    ] {
        let provide: Provide = input.parse().expect("test provide is valid");
        assert_eq!(provide.to_string(), input);
    }
}

#[test]
fn rejects_malformed_provides() {
    for bad in [
        "zlib",
        "so:",
        "pc:zlib>=1.3",
        "pc:zlib=",
        "virtual:a b",
        "pkg:zlib",
    ] {
        assert!(
            bad.parse::<Provide>().is_err(),
            "{bad:?} should be rejected"
        );
    }
}

fn arb_name() -> impl Strategy<Value = Name> {
    "[a-z][a-z0-9_-]{0,8}(\\.[0-9]{1,2})?".prop_map(|s| s.parse().expect("generated name is valid"))
}

fn arb_version() -> impl Strategy<Value = Version> {
    "[A-Za-z0-9._~]{1,10}".prop_map(|s| s.parse().expect("generated version is valid"))
}

fn arb_library_name() -> impl Strategy<Value = String> {
    "[A-Za-z0-9._+-]{1,16}"
}

fn arb_constraint() -> impl Strategy<Value = Constraint> {
    (
        prop_oneof![
            Just(Op::Ge),
            Just(Op::Le),
            Just(Op::Gt),
            Just(Op::Lt),
            Just(Op::Eq)
        ],
        arb_version(),
        proptest::option::of(1u32..),
    )
        .prop_map(|(op, version, rev)| Constraint {
            op,
            version,
            rev: rev.and_then(NonZeroU32::new),
        })
}

fn arb_dep() -> impl Strategy<Value = Dep> {
    prop_oneof![
        arb_library_name().prop_map(|s| Dep::So(s.parse().expect("generated soname is valid"))),
        (
            arb_library_name(),
            proptest::collection::vec(arb_constraint(), 0..3)
        )
            .prop_map(|(s, c)| Dep::Pc(s.parse().expect("generated module name is valid"), c)),
        arb_name().prop_map(Dep::Virtual),
        arb_name().prop_map(|n| Dep::Pkg(n, VersionReq::Any)),
        (arb_name(), "[0-9][0-9.]{0,6}").prop_map(|(n, p)| {
            Dep::Pkg(
                n,
                VersionReq::Prefix(p.parse().expect("generated prefix is valid")),
            )
        }),
        (
            arb_name(),
            proptest::collection::vec(arb_constraint(), 0..4)
        )
            .prop_map(|(n, c)| Dep::Pkg(n, VersionReq::from_constraints(c))),
    ]
}

fn arb_provide() -> impl Strategy<Value = Provide> {
    prop_oneof![
        arb_library_name().prop_map(|s| Provide::So(s.parse().expect("generated soname is valid"))),
        (arb_library_name(), proptest::option::of(arb_version())).prop_map(|(s, v)| {
            Provide::Pc(s.parse().expect("generated module name is valid"), v)
        }),
        arb_name().prop_map(Provide::Virtual),
    ]
}

fn version(s: &str) -> Version {
    s.parse().expect("test version is valid")
}

#[test]
fn constraints_without_revision_ignore_the_revision() {
    let lt2 = constraint(Op::Lt, "2", None);
    assert!(lt2.matches(&version("1.9"), Some(7)));
    assert!(!lt2.matches(&version("2"), Some(1)));
    assert!(!lt2.matches(&version("2.0"), Some(1)));
    let eq = constraint(Op::Eq, "1.3", None);
    assert!(eq.matches(&version("1.3"), Some(1)));
    assert!(eq.matches(&version("1.3"), Some(9)));
    assert!(!eq.matches(&version("1.3.0"), Some(1)));
    assert!(constraint(Op::Ge, "1.3", None).matches(&version("1.3"), Some(1)));
    assert!(constraint(Op::Le, "1.3", None).matches(&version("1.3~rc1"), None));
    assert!(!constraint(Op::Gt, "1.3", None).matches(&version("1.3"), Some(5)));
}

#[test]
fn constraints_with_revision_compare_it_on_equal_versions() {
    let exact = constraint(Op::Eq, "1.3.1", Some(2));
    assert!(exact.matches(&version("1.3.1"), Some(2)));
    assert!(!exact.matches(&version("1.3.1"), Some(1)));
    let ge = constraint(Op::Ge, "1.3.1", Some(2));
    assert!(ge.matches(&version("1.3.2"), Some(1)));
    assert!(!ge.matches(&version("1.3.1"), Some(1)));
    assert!(constraint(Op::Lt, "1.3.1", Some(2)).matches(&version("1.3.1"), Some(1)));
}

#[test]
fn version_requirements_match_by_kind() {
    assert!(VersionReq::Any.matches(&version("0.1"), 1));
    let prefix = VersionReq::Prefix("19".parse().expect("valid"));
    assert!(prefix.matches(&version("19.1.7"), 1));
    assert!(!prefix.matches(&version("190.1"), 1));
    let range = VersionReq::from_constraints(vec![
        constraint(Op::Ge, "1.3", None),
        constraint(Op::Lt, "2", None),
    ]);
    assert!(range.matches(&version("1.9.9"), 3));
    assert!(!range.matches(&version("2.0"), 1));
    assert!(!range.matches(&version("1.2"), 1));
}

proptest! {
    #[test]
    fn dependency_text_round_trips(dep in arb_dep()) {
        let text = dep.to_string();
        prop_assert_eq!(text.parse::<Dep>(), Ok(dep));
    }

    #[test]
    fn provide_text_round_trips(provide in arb_provide()) {
        let text = provide.to_string();
        prop_assert_eq!(text.parse::<Provide>(), Ok(provide));
    }

    #[test]
    fn dependency_parsing_never_panics(input in "\\PC{0,24}") {
        let _ = input.parse::<Dep>();
        let _ = input.parse::<Provide>();
    }
}
