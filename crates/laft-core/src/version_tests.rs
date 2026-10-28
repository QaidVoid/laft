use std::cmp::Ordering::{self, Equal, Greater, Less};

use proptest::prelude::*;

use super::*;

fn v(s: &str) -> Version {
    s.parse().expect("test version is valid")
}

#[test]
fn ordering_matches_the_reference_table() {
    let table: &[(&str, &str, Ordering)] = &[
        ("1.0", "1.0", Equal),
        ("1.0", "1.0.0", Less),
        ("1.2", "1.10", Less),
        ("1.0~rc1", "1.0", Less),
        ("1.0~rc1", "1.0~rc2", Less),
        ("1.0~~", "1.0~", Less),
        ("1.0~", "1.0", Less),
        ("1.0a", "1.0", Greater),
        ("1.0a", "1.0b", Less),
        ("1.0a", "1.0.1", Less),
        ("1.0.0", "1.0a", Greater),
        ("r9", "r10", Less),
        ("010", "10", Less),
        ("1.2", "1_2", Less),
        ("2024.01", "2024.1", Less),
        ("6.17.3", "6.17.10", Less),
    ];
    for &(a, b, expected) in table {
        assert_eq!(v(a).cmp(&v(b)), expected, "cmp({a}, {b})");
        assert_eq!(v(b).cmp(&v(a)), expected.reverse(), "cmp({b}, {a})");
    }
}

#[test]
fn prefix_matching_is_token_wise() {
    let table = [
        ("19.1.7", "19", true),
        ("19.1.7", "19.1", true),
        ("19.10.0", "19.1", false),
        ("3.12.4", "3.12", true),
        ("2.0~rc1", "2", true),
        ("20.1.0", "2", false),
        ("1.2", "1.2.3", false),
        ("01.2", "1", true),
    ];
    for (version, prefix, expected) in table {
        let prefix: VersionPrefix = prefix.parse().expect("test prefix is valid");
        assert_eq!(
            prefix.matches(&v(version)),
            expected,
            "{prefix} on {version}"
        );
    }
}

#[test]
fn narrower_prefix_is_within_a_wider_one() {
    let p = |s: &str| s.parse::<VersionPrefix>().expect("test prefix is valid");
    assert!(p("19.1").is_within(&p("19")));
    assert!(!p("19").is_within(&p("19.1")));
    assert!(!p("19.10").is_within(&p("19.1")));
}

#[test]
fn rejects_versions_outside_the_alphabet() {
    for bad in ["", "1.0-1", "1.0+git", "1 0", "v1/2"] {
        assert!(
            bad.parse::<Version>().is_err(),
            "{bad:?} should be rejected"
        );
    }
    assert!("..".parse::<Version>().is_ok());
    assert!("..".parse::<VersionPrefix>().is_err());
}

#[test]
fn package_version_round_trips_and_orders_by_revision() {
    let pv: PackageVersion = "14.1.1-r12".parse().expect("valid package version");
    assert_eq!(pv.to_string(), "14.1.1-r12");
    assert!(pv > "14.1.1-r9".parse().expect("valid package version"));
    assert!(pv < "14.1.2-r1".parse().expect("valid package version"));
}

#[test]
fn package_version_rejects_bad_revisions() {
    for bad in [
        "14.1.1",
        "14.1.1-r0",
        "14.1.1-r",
        "14.1.1-r01",
        "14.1.1-rx",
        "-r1",
    ] {
        assert!(
            bad.parse::<PackageVersion>().is_err(),
            "{bad:?} should be rejected"
        );
    }
}

fn version_strategy() -> impl Strategy<Value = Version> {
    "[0-9a-z._~]{1,12}".prop_map(Version)
}

proptest! {
    #[test]
    fn ordering_is_a_total_order(a in version_strategy(), b in version_strategy(), c in version_strategy()) {
        prop_assert_eq!(a.cmp(&b), b.cmp(&a).reverse());
        prop_assert_eq!(a.cmp(&b) == Equal, a == b);
        if a <= b && b <= c {
            prop_assert!(a <= c);
        }
    }

    #[test]
    fn every_version_matches_its_own_prefixes(s in "[0-9a-z]{1,4}(\\.[0-9a-z~]{1,4}){0,4}") {
        let version = Version(s.clone());
        for (i, _) in s.match_indices('.') {
            let prefix: VersionPrefix = s[..i].parse().expect("a leading part of a valid version is a valid prefix");
            prop_assert!(prefix.matches(&version), "{} should match {}", prefix, version);
        }
    }
}
