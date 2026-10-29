use std::collections::BTreeSet;

use laft_core::Dep;
use laft_index::Index;
use proptest::prelude::*;

use super::*;
use crate::test_support::*;
use crate::universe::provides;

fn code<T>(result: Result<T, Error>) -> &'static str {
    match result {
        Ok(_) => panic!("the resolution should fail"),
        Err(e) => e.code().unwrap_or("none"),
    }
}

#[test]
fn soname_dependency_pulls_in_its_provider() {
    let core = index(
        "core",
        vec![
            pkg("app 1.0").depends(&["so:libx.so.1"]),
            pkg("libx 1.2").provides(&["so:libx.so.1"]),
        ],
    );
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1.0-r1", "libx 1.2-r1"]);
    assert_eq!(
        r.packages[&name("libx")].reasons,
        [Reason::Dependency {
            by: name("app"),
            dep: "so:libx.so.1".parse().unwrap(),
        }]
    );
    assert_eq!(
        r.packages[&name("app")].reasons,
        [Reason::Requested {
            origin: "test:app".into(),
            text: "app".into(),
        }]
    );
}

#[test]
fn newest_version_wins_and_tilde_sorts_first() {
    let core = index(
        "core",
        vec![pkg("tool 1.0"), pkg("tool 1.1~rc1"), pkg("tool 1.1")],
    );
    let r = resolve_core(&core, &["tool"], &[]).unwrap();
    assert_eq!(labels(&r), ["tool 1.1-r1"]);
}

#[test]
fn version_prefix_matches_token_wise() {
    let core = index("core", vec![pkg("llvm 19.1.7"), pkg("llvm 20.1.2")]);
    let r = resolve_core(&core, &["llvm@19"], &[]).unwrap();
    assert_eq!(labels(&r), ["llvm 19.1.7-r1"]);
    let core = index("core", vec![pkg("llvm 190.1"), pkg("llvm 19.1.7")]);
    let r = resolve_core(&core, &["llvm@19"], &[]).unwrap();
    assert_eq!(labels(&r), ["llvm 19.1.7-r1"]);
}

#[test]
fn static_request_selects_only_the_static_variant() {
    let core = index(
        "core",
        vec![
            pkg("rg 14.1.1").depends(&["so:libpcre2-8.so.0"]),
            pkg("rg 14.1.1 static"),
            pkg("pcre2 10.45").provides(&["so:libpcre2-8.so.0"]),
        ],
    );
    let r = resolve_core(&core, &["rg:static"], &[]).unwrap();
    assert_eq!(labels(&r), ["rg:static 14.1.1-r1"]);
}

#[test]
fn installed_version_is_kept() {
    let old = pkg("tool 1.0");
    let core = index("core", vec![old.clone(), pkg("tool 1.1")]);
    let r = resolve_core(&core, &["tool"], &[&old]).unwrap();
    assert_eq!(labels(&r), ["tool 1.0-r1"]);
    assert!(r.packages[&name("tool")].kept_installed);
}

#[test]
fn upgrade_moves_to_the_newest_version() {
    let old = pkg("tool 1.0");
    let core = index("core", vec![old.clone(), pkg("tool 1.1")]);
    let u = universe(&[(&core, 0)]);
    let mut i = input("system", vec![req("tool")], &[&old]);
    i.upgrade = Upgrade::All;
    let r = u.resolve(&i).unwrap();
    assert_eq!(labels(&r), ["tool 1.1-r1"]);
    assert!(!r.packages[&name("tool")].kept_installed);

    i.upgrade = Upgrade::Names(BTreeSet::from([name("tool")]));
    assert_eq!(labels(&u.resolve(&i).unwrap()), ["tool 1.1-r1"]);
    i.upgrade = Upgrade::Names(BTreeSet::from([name("other")]));
    assert_eq!(labels(&u.resolve(&i).unwrap()), ["tool 1.0-r1"]);
}

#[test]
fn pruned_installed_version_is_kept() {
    let pruned = pkg("tool 0.9");
    let core = index("core", vec![pkg("tool 1.1")]);
    let r = resolve_core(&core, &["tool"], &[&pruned]).unwrap();
    assert_eq!(labels(&r), ["tool 0.9-r1"]);
    let selected = &r.packages[&name("tool")];
    assert!(selected.kept_installed);
    assert_eq!(selected.candidate.source, Source::Installed);
    assert!(r.warnings.is_empty());
}

fn constrained_universe() -> Index {
    index(
        "core",
        vec![
            pkg("foo 1.0"),
            pkg("foo 2.0"),
            pkg("bar 1.0").depends(&["foo<2"]),
        ],
    )
}

#[test]
fn contradicting_dependency_is_learned_and_resolution_restarts() {
    let core = constrained_universe();
    let r = resolve_core(&core, &["foo", "bar"], &[]).unwrap();
    assert_eq!(labels(&r), ["bar 1.0-r1", "foo 1.0-r1"]);
    assert_eq!(r.learned.len(), 1);
    assert_eq!(r.learned[0].to_string(), "learned foo<2 from bar 1.0-r1");
}

#[test]
fn requested_version_cannot_be_overridden_by_a_dependency() {
    let core = constrained_universe();
    let e = resolve_core(&core, &["foo@2", "bar"], &[]).unwrap_err();
    assert_eq!(e.code(), Some("R005"));
}

fn gl_universe(default: Option<&str>) -> Index {
    let mut core = index(
        "core",
        vec![
            pkg("ga 1").provides(&["so:libGL.so.1"]),
            pkg("gb 1").provides(&["so:libGL.so.1"]),
            pkg("app 1").depends(&["so:libGL.so.1"]),
        ],
    );
    if let Some(default) = default {
        core.defaults
            .providers
            .insert("so:libGL.so.1".into(), name(default));
    }
    core
}

#[test]
fn several_providers_without_a_default_are_ambiguous() {
    let core = gl_universe(None);
    assert_eq!(code(resolve_core(&core, &["app"], &[])), "R004");
}

#[test]
fn repository_default_chooses_the_provider() {
    let core = gl_universe(Some("gb"));
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "gb 1-r1"]);
}

#[test]
fn requests_are_selected_before_dependencies() {
    let core = gl_universe(None);
    let r = resolve_core(&core, &["app", "ga"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "ga 1-r1"]);
}

#[test]
fn conflicting_packages_fail() {
    let core = index("core", vec![pkg("a 1").conflicts(&["b"]), pkg("b 1")]);
    assert_eq!(code(resolve_core(&core, &["a", "b"], &[])), "R006");
}

#[test]
fn request_for_a_replaced_name_follows_the_rename() {
    let core = index("core", vec![pkg("bar 2").replaces(&["foo"])]);
    let r = resolve_core(&core, &["foo"], &[]).unwrap();
    assert_eq!(labels(&r), ["bar 2-r1"]);
    assert_eq!(
        r.warnings,
        [Warning::Renamed {
            from: name("foo"),
            to: name("bar"),
            origin: "test:foo".into(),
        }]
    );
}

#[test]
fn higher_priority_repository_shadows_the_name() {
    let core = index("core", vec![pkg("foo 2.0")]);
    let mine = index("mine", vec![pkg("foo 1.5")]);
    let u = universe(&[(&core, 0), (&mine, 10)]);
    let r = u.resolve(&input("system", vec![req("foo")], &[])).unwrap();
    assert_eq!(labels(&r), ["foo 1.5-r1"]);
    assert!(matches!(
        r.packages[&name("foo")].candidate.source,
        Source::Index { repo, .. } if repo.as_str() == "mine"
    ));
}

#[test]
fn equal_priority_repositories_make_a_name_ambiguous() {
    let core = index("core", vec![pkg("foo 2.0")]);
    let extra = index("extra", vec![pkg("foo 2.1")]);
    let u = universe(&[(&core, 0), (&extra, 0)]);
    assert_eq!(
        code(u.resolve(&input("system", vec![req("foo")], &[]))),
        "R008"
    );
}

#[test]
fn equal_priority_matters_only_for_needed_names() {
    let core = index("core", vec![pkg("foo 2.0"), pkg("app 1")]);
    let extra = index("extra", vec![pkg("foo 2.1")]);
    let u = universe(&[(&core, 0), (&extra, 0)]);
    let r = u.resolve(&input("system", vec![req("app")], &[])).unwrap();
    assert_eq!(labels(&r), ["app 1-r1"]);
}

#[test]
fn static_variant_does_not_provide_a_shared_library() {
    let core = index(
        "core",
        vec![
            pkg("zlib 1.3.1").provides(&["so:libz.so.1"]),
            pkg("zlib 1.3.1 static"),
            pkg("py 3.12").depends(&["so:libz.so.1"]),
        ],
    );
    assert_eq!(
        code(resolve_core(&core, &["zlib:static", "py"], &[])),
        "R007"
    );
}

#[test]
fn requests_for_two_variants_conflict() {
    let core = index("core", vec![pkg("rg 14 static")]);
    assert_eq!(code(resolve_core(&core, &["rg", "rg:static"], &[])), "R002");
}

#[test]
fn missing_provider_fails_with_the_chain() {
    let core = index("core", vec![pkg("x 1").depends(&["so:liby.so.1"])]);
    let e = resolve_core(&core, &["x"], &[]).unwrap_err();
    let ErrorKind::Unsatisfied { dep, by, chain, .. } = *e.kind else {
        panic!("expected R003, got {e}");
    };
    assert_eq!(dep.to_string(), "so:liby.so.1");
    assert_eq!(by.to_string(), "x 1-r1");
    assert!(chain.steps.is_empty());
    assert_eq!(chain.origin, "test:x");
}

#[test]
fn dependency_cycles_resolve() {
    let core = index(
        "core",
        vec![pkg("a 1").depends(&["b"]), pkg("b 1").depends(&["a"])],
    );
    let r = resolve_core(&core, &["a"], &[]).unwrap();
    assert_eq!(labels(&r), ["a 1-r1", "b 1-r1"]);
}

#[test]
fn yanked_installed_version_moves_to_another() {
    let yanked = pkg("tool 1.1");
    let mut core = index("core", vec![pkg("tool 1.0")]);
    core.yanked.push(yank(&yanked, "corrupts its config"));
    let r = resolve_core(&core, &["tool"], &[&yanked]).unwrap();
    assert_eq!(labels(&r), ["tool 1.0-r1"]);
    assert_eq!(
        r.warnings
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["warning[W003]: tool 1.1-r1 was yanked (corrupts its config); moving to tool 1.0-r1"]
    );
}

#[test]
fn yanked_installed_version_without_replacement_fails() {
    let yanked = pkg("tool 1.1");
    let mut core = index("core", vec![]);
    core.yanked.push(yank(&yanked, "corrupts its config"));
    assert_eq!(code(resolve_core(&core, &["tool"], &[&yanked])), "R010");
}

#[test]
fn upgrade_leaves_a_pruned_version() {
    let pruned = pkg("tool 0.9");
    let core = index("core", vec![pkg("tool 1.1")]);
    let u = universe(&[(&core, 0)]);
    let mut i = input("system", vec![req("tool")], &[&pruned]);
    i.upgrade = Upgrade::All;
    assert_eq!(labels(&u.resolve(&i).unwrap()), ["tool 1.1-r1"]);
}

#[test]
fn initrd_closure_is_independent_of_the_system_view() {
    let core = index(
        "core",
        vec![
            pkg("busybox 1.37 static"),
            pkg("busybox 1.37").depends(&["so:libc.so.6"]),
            pkg("glibc 2.42").provides(&["so:libc.so.6"]),
        ],
    );
    let u = universe(&[(&core, 0)]);
    let initrd = u
        .resolve(&input("initrd", vec![req("busybox:static")], &[]))
        .unwrap();
    let system = u
        .resolve(&input("system", vec![req("busybox")], &[]))
        .unwrap();
    assert_eq!(labels(&initrd), ["busybox:static 1.37-r1"]);
    assert_eq!(labels(&system), ["busybox 1.37-r1", "glibc 2.42-r1"]);
}

#[test]
fn name_dependency_prefers_the_dynamic_variant_but_accepts_static() {
    let core = index(
        "core",
        vec![
            pkg("tool 2 static"),
            pkg("tool 1"),
            pkg("lib 1 static"),
            pkg("app 1").depends(&["lib", "tool"]),
        ],
    );
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "lib:static 1-r1", "tool 1-r1"]);
}

#[test]
fn name_dependency_is_satisfied_by_a_selected_static_variant() {
    let core = index(
        "core",
        vec![
            pkg("rg 14"),
            pkg("rg 14 static"),
            pkg("app 1").depends(&["rg>=14"]),
        ],
    );
    let r = resolve_core(&core, &["rg:static", "app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "rg:static 14-r1"]);
}

#[test]
fn name_dependency_constraints_and_prefixes_select_matching_versions() {
    let core = index(
        "core",
        vec![
            pkg("llvm 18.1"),
            pkg("llvm 19.1.7"),
            pkg("llvm 20.1"),
            pkg("zlib 1.2-r3"),
            pkg("zlib 1.3.1-r1"),
            pkg("zlib 1.3.1-r2"),
            pkg("zlib 2.0"),
            pkg("app 1").depends(&["llvm@19", "zlib=1.3.1-r1"]),
            pkg("other 1").depends(&["zlib>=1.3,<2"]),
        ],
    );
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "llvm 19.1.7-r1", "zlib 1.3.1-r1"]);
    let r = resolve_core(&core, &["other"], &[]).unwrap();
    assert_eq!(labels(&r), ["other 1-r1", "zlib 1.3.1-r2"]);
}

#[test]
fn virtual_and_pkg_config_dependencies_are_satisfied_by_provides() {
    let core = index(
        "core",
        vec![
            pkg("openjdk 21").provides(&["virtual:java-runtime"]),
            pkg("zlib-old 1").provides(&["pc:zlib=1.2"]),
            pkg("zlib 1.3").provides(&["pc:zlib=1.3.1"]),
            pkg("app 1").depends(&["pc:zlib>=1.3", "virtual:java-runtime"]),
        ],
    );
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "openjdk 21-r1", "zlib 1.3-r1"]);
}

#[test]
fn pkg_config_dependency_rejects_a_provide_outside_its_constraints() {
    let core = index(
        "core",
        vec![
            pkg("zlib 1.2").provides(&["pc:zlib=1.2"]),
            pkg("app 1").depends(&["pc:zlib>=1.3"]),
        ],
    );
    assert_eq!(code(resolve_core(&core, &["app"], &[])), "R003");
}

#[test]
fn installed_provider_is_preferred_over_an_ambiguous_choice() {
    let ga = pkg("ga 1").provides(&["so:libGL.so.1"]);
    let core = index(
        "core",
        vec![
            ga.clone(),
            pkg("gb 1").provides(&["so:libGL.so.1"]),
            pkg("app 1").depends(&["so:libGL.so.1"]),
        ],
    );
    let r = resolve_core(&core, &["app"], &[&ga]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "ga 1-r1"]);
    assert!(r.packages[&name("ga")].kept_installed);
}

#[test]
fn orphaned_installed_package_is_kept_with_a_warning() {
    let orphan = pkg("oldtool 1.2").provides(&["so:libold.so.1"]);
    let core = index("core", vec![pkg("app 1").depends(&["so:libold.so.1"])]);
    let r = resolve_core(&core, &["app", "oldtool"], &[&orphan]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "oldtool 1.2-r1"]);
    assert_eq!(
        r.warnings
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        [
            "warning[W002]: oldtool 1.2-r1 is no longer in any repository; keeping the installed version"
        ]
    );
}

#[test]
fn pruned_installed_version_gives_way_to_a_constraint() {
    let pruned = pkg("foo 0.9");
    let core = index(
        "core",
        vec![pkg("foo 1.1"), pkg("app 1").depends(&["foo>=1"])],
    );
    let r = resolve_core(&core, &["app"], &[&pruned]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "foo 1.1-r1"]);
}

#[test]
fn installed_version_outside_the_request_prefix_is_not_kept() {
    let old = pkg("llvm 19.1.7");
    let core = index("core", vec![old.clone(), pkg("llvm 20.1.2")]);
    let r = resolve_core(&core, &["llvm@20"], &[&old]).unwrap();
    assert_eq!(labels(&r), ["llvm 20.1.2-r1"]);
}

#[test]
fn compatible_prefixes_merge_to_the_longer_one() {
    let core = index("core", vec![pkg("llvm 19.1.7"), pkg("llvm 19.2.0")]);
    let r = resolve_core(&core, &["llvm@19", "llvm@19.1", "llvm"], &[]).unwrap();
    assert_eq!(labels(&r), ["llvm 19.1.7-r1"]);
    assert_eq!(r.packages[&name("llvm")].reasons.len(), 3);
    assert_eq!(
        code(resolve_core(&core, &["llvm@19", "llvm@20"], &[])),
        "R002"
    );
}

#[test]
fn rename_with_several_replacing_packages_fails() {
    let core = index(
        "core",
        vec![
            pkg("bar 1").replaces(&["foo"]),
            pkg("baz 1").replaces(&["foo"]),
        ],
    );
    let e = resolve_core(&core, &["foo"], &[]).unwrap_err();
    let ErrorKind::NoCandidate { replaced_by, .. } = &*e.kind else {
        panic!("expected R001, got {e}");
    };
    assert_eq!(replaced_by, &[name("bar"), name("baz")]);
}

#[test]
fn replacing_a_selected_package_fails() {
    let core = index(
        "core",
        vec![pkg("bar 2").replaces(&["foo<2"]), pkg("foo 1")],
    );
    let e = resolve_core(&core, &["bar", "foo"], &[]).unwrap_err();
    assert!(matches!(
        *e.kind,
        ErrorKind::Conflict { replaces: true, .. }
    ));
    let core = index(
        "core",
        vec![pkg("bar 2").replaces(&["foo<2"]), pkg("foo 2")],
    );
    assert!(resolve_core(&core, &["bar", "foo"], &[]).is_ok());
}

#[test]
fn conflict_on_a_virtual_matches_other_providers_only() {
    let core = index(
        "core",
        vec![
            pkg("postfix 3")
                .provides(&["virtual:mta"])
                .conflicts(&["virtual:mta"]),
            pkg("exim 4").provides(&["virtual:mta"]),
        ],
    );
    assert!(resolve_core(&core, &["postfix"], &[]).is_ok());
    assert_eq!(code(resolve_core(&core, &["postfix", "exim"], &[])), "R006");
}

#[test]
fn alternatives_use_the_configured_member_then_the_only_one_then_the_default() {
    let mut core = index(
        "core",
        vec![
            pkg("neovim 0.11").alternative("vi", &["usr/bin/vi"]),
            pkg("vim 9.1").alternative("vi", &["usr/bin/vi"]),
        ],
    );
    let u = universe(&[(&core, 0)]);
    let both = vec![req("neovim"), req("vim")];

    let r = u.resolve(&input("system", vec![req("vim")], &[])).unwrap();
    assert_eq!(r.alternatives[&name("vi")], name("vim"));

    let e = u.resolve(&input("system", both.clone(), &[])).unwrap_err();
    assert!(matches!(*e.kind, ErrorKind::AlternativeAmbiguous { .. }));

    let mut i = input("system", both.clone(), &[]);
    i.alternatives.insert(name("vi"), name("neovim"));
    assert_eq!(
        u.resolve(&i).unwrap().alternatives[&name("vi")],
        name("neovim")
    );

    let mut i = input("system", vec![req("vim")], &[]);
    i.alternatives.insert(name("vi"), name("neovim"));
    let e = u.resolve(&i).unwrap_err();
    assert!(matches!(*e.kind, ErrorKind::AlternativeAbsent { .. }));

    core.defaults.alternatives.insert(name("vi"), name("vim"));
    let u = universe(&[(&core, 0)]);
    let r = u.resolve(&input("system", both, &[])).unwrap();
    assert_eq!(r.alternatives[&name("vi")], name("vim"));
}

#[test]
fn constraint_learned_from_a_dependency_applies_to_a_later_provider_choice() {
    let core = index(
        "core",
        vec![
            pkg("ssl 1").provides(&["so:libssl.so.1"]),
            pkg("ssl 2").provides(&["so:libssl.so.1"]),
            pkg("curl 1").depends(&["so:libssl.so.1"]),
            pkg("app 1").depends(&["curl", "pin"]),
            pkg("pin 1").depends(&["ssl<2"]),
        ],
    );
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(
        labels(&r),
        ["app 1-r1", "curl 1-r1", "pin 1-r1", "ssl 1-r1"]
    );
    assert_eq!(r.learned.len(), 1);
}

#[test]
fn requests_of_a_missing_name_fail_with_the_available_versions() {
    let core = index("core", vec![pkg("llvm 20.1.2"), pkg("llvm 19.1.7-r2")]);
    let e = resolve_core(&core, &["llvm@18"], &[]).unwrap_err();
    let ErrorKind::NoCandidate { available, .. } = &*e.kind else {
        panic!("expected R001, got {e}");
    };
    assert_eq!(
        available
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["llvm 20.1.2-r1", "llvm 19.1.7-r2"]
    );
}

/// Checks that a successful resolution is sound: one package per name,
/// every request and dependency satisfied, and no conflicts. An unversioned
/// dependency on a name nothing selected may be met by a package that
/// replaces the name.
fn check_sound(r: &Resolution<'_>, requests: &[Request]) {
    for (name, selected) in &r.packages {
        let meta = selected.candidate.meta;
        assert_eq!(&meta.name, name);
        for dep in &meta.depends {
            let ok = match dep {
                Dep::Pkg(n, req) => match r.packages.get(n) {
                    Some(s) => req.matches(&s.candidate.meta.version, s.candidate.meta.rev),
                    None => {
                        *req == VersionReq::Any
                            && r.packages.values().any(|s| {
                                s.candidate
                                    .meta
                                    .replaces
                                    .iter()
                                    .any(|d| matches!(d, Dep::Pkg(x, _) if x == n))
                            })
                    }
                },
                _ => r.packages.values().any(|s| provides(s.candidate.meta, dep)),
            };
            assert!(ok, "{name} needs {dep}, which is not satisfied");
        }
        for dep in meta.conflicts.iter().chain(&meta.replaces) {
            let hit = match dep {
                Dep::Pkg(n, req) => {
                    n != name
                        && r.packages.get(n).is_some_and(|s| {
                            req.matches(&s.candidate.meta.version, s.candidate.meta.rev)
                        })
                }
                _ => r
                    .packages
                    .iter()
                    .any(|(n, s)| n != name && provides(s.candidate.meta, dep)),
            };
            assert!(!hit, "{name} conflicts with {dep}, which is selected");
        }
    }
    for request in requests {
        if let Some(s) = r.packages.get(request.pkg.name()) {
            assert_eq!(s.candidate.meta.variant, request.pkg.variant());
            if let Some(prefix) = request.pkg.prefix() {
                assert!(prefix.matches(&s.candidate.meta.version));
            }
        } else {
            assert!(
                r.warnings.iter().any(
                    |w| matches!(w, Warning::Renamed { from, .. } if from == request.pkg.name())
                ),
                "request {} is missing",
                request.pkg
            );
        }
    }
}

/// A random universe: two repositories, packages `p0` to `p9` in a few
/// versions and variants, `so:` and name dependencies, provides, conflicts
/// and replaces, plus requests and an installed set.
#[derive(Debug, Clone)]
struct Scenario {
    repos: Vec<(Index, i64)>,
    requests: Vec<Request>,
    installed: Vec<Meta>,
    upgrade: bool,
}

fn arb_scenario(max_packages: usize) -> impl Strategy<Value = Scenario> {
    let names = 10usize;
    let dep = prop_oneof![
        (0..5usize).prop_map(|k| format!("so:lib{k}.so.1")),
        (0..names).prop_map(|n| format!("p{n}")),
        (0..names).prop_map(|n| format!("p{n}<2")),
        (0..names).prop_map(|n| format!("p{n}>=2")),
        (0..names).prop_map(|n| format!("p{n}@1")),
    ];
    let package = (
        0..names,
        prop::sample::select(vec!["1", "1.5", "2", "2.1~rc1", "3"]),
        prop::bool::weighted(0.15),
        prop::collection::vec(dep, 0..3),
        prop::collection::vec(0..5usize, 0..3),
        prop::option::weighted(0.1, 0..names),
        prop::option::weighted(0.05, 0..names),
        0..2usize,
    );
    let request = (any::<prop::sample::Index>(), prop::bool::weighted(0.15));
    (
        prop::collection::vec(package, max_packages / 4..max_packages),
        prop::collection::vec(request, 1..5),
        0..3i64,
        prop_oneof![9 => Just(1i64), 9 => Just(-1), 1 => Just(0)],
        prop::collection::vec(any::<bool>(), max_packages),
        any::<bool>(),
    )
        .prop_map(
            |(packages, requests, prio0, delta, installed_mask, upgrade)| {
                let mut repos = vec![(Vec::new(), prio0), (Vec::new(), prio0 + delta)];
                let mut all: Vec<Meta> = Vec::new();
                let mut seen = BTreeSet::new();
                let mut installed: Vec<Meta> = Vec::new();
                for (i, (n, version, is_static, deps, provides, conflict, replace, repo)) in
                    packages.into_iter().enumerate()
                {
                    if !seen.insert((repo, n, version, is_static)) {
                        continue;
                    }
                    let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
                    let provides: Vec<String> = if is_static {
                        vec![]
                    } else {
                        provides.iter().map(|k| format!("so:lib{k}.so.1")).collect()
                    };
                    let provides: Vec<&str> = provides.iter().map(String::as_str).collect();
                    let conflicts: Vec<String> =
                        conflict.map(|c| format!("p{c}")).into_iter().collect();
                    let conflicts: Vec<&str> = conflicts.iter().map(String::as_str).collect();
                    let replaces: Vec<String> =
                        replace.map(|c| format!("p{c}<1")).into_iter().collect();
                    let replaces: Vec<&str> = replaces.iter().map(String::as_str).collect();
                    let variant = if is_static { " static" } else { "" };
                    let meta = pkg(&format!("p{n} {version}{variant}"))
                        .depends(&deps)
                        .provides(&provides)
                        .conflicts(&conflicts)
                        .replaces(&replaces);
                    if installed_mask[i] && !installed.iter().any(|m| m.name == meta.name) {
                        installed.push(meta.clone());
                    }
                    all.push(meta.clone());
                    repos[repo].0.push(meta);
                }
                let repos = repos
                    .into_iter()
                    .enumerate()
                    .map(|(i, (packages, priority))| (index(&format!("r{i}"), packages), priority))
                    .collect();
                let requests = requests
                    .into_iter()
                    .map(|(i, with_prefix)| {
                        let meta = i.get(&all);
                        let prefix = if with_prefix {
                            format!(
                                "@{}",
                                meta.version.as_str().split('.').next().unwrap_or("1")
                            )
                        } else {
                            String::new()
                        };
                        let variant = match meta.variant {
                            Variant::Static => ":static",
                            Variant::Dynamic => "",
                        };
                        req(&format!("{}{prefix}{variant}", meta.name))
                    })
                    .collect();
                Scenario {
                    repos,
                    requests,
                    installed,
                    upgrade,
                }
            },
        )
}

impl Scenario {
    fn outcome(&self) -> String {
        let repos: Vec<(&Index, i64)> = self.repos.iter().map(|(i, p)| (i, *p)).collect();
        let installed: Vec<&Meta> = self.installed.iter().collect();
        let mut i = input("system", self.requests.clone(), &installed);
        if self.upgrade {
            i.upgrade = Upgrade::All;
        }
        format!("{:?}", universe(&repos).resolve(&i))
    }

    fn shuffled(&self, seed: u64) -> Self {
        let mut rng = Seeded::new(seed);
        let mut copy = self.clone();
        rng.shuffle(&mut copy.repos);
        for (index, _) in &mut copy.repos {
            rng.shuffle(&mut index.packages);
            rng.shuffle(&mut index.yanked);
        }
        rng.shuffle(&mut copy.requests);
        copy
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn successful_resolutions_are_sound(scenario in arb_scenario(40)) {
        let repos: Vec<(&Index, i64)> = scenario.repos.iter().map(|(i, p)| (i, *p)).collect();
        let installed: Vec<&Meta> = scenario.installed.iter().collect();
        let mut i = input("system", scenario.requests.clone(), &installed);
        if scenario.upgrade {
            i.upgrade = Upgrade::All;
        }
        if let Ok(r) = universe(&repos).resolve(&i) {
            check_sound(&r, &scenario.requests);
        }
    }

    #[test]
    fn large_universes_resolve_soundly(scenario in arb_scenario(200)) {
        let repos: Vec<(&Index, i64)> = scenario.repos.iter().map(|(i, p)| (i, *p)).collect();
        let installed: Vec<&Meta> = scenario.installed.iter().collect();
        if let Ok(r) = universe(&repos).resolve(&input("system", scenario.requests.clone(), &installed)) {
            check_sound(&r, &scenario.requests);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn result_does_not_depend_on_input_order(scenario in arb_scenario(40), seed in any::<u64>()) {
        prop_assert_eq!(scenario.outcome(), scenario.shuffled(seed).outcome());
    }
}

/// The universes of the examples above, for the order independence check.
fn example_scenarios() -> Vec<Scenario> {
    let one = |packages: Vec<Meta>, requests: &[&str]| Scenario {
        repos: vec![(index("core", packages), 0)],
        requests: requests.iter().map(|r| req(r)).collect(),
        installed: vec![],
        upgrade: false,
    };
    let mut gl = gl_universe(Some("gb"));
    gl.defaults.alternatives.insert(name("vi"), name("vim"));
    vec![
        one(
            vec![
                pkg("app 1.0").depends(&["so:libx.so.1"]),
                pkg("libx 1.2").provides(&["so:libx.so.1"]),
            ],
            &["app"],
        ),
        one(
            vec![pkg("tool 1.0"), pkg("tool 1.1~rc1"), pkg("tool 1.1")],
            &["tool"],
        ),
        one(vec![pkg("llvm 19.1.7"), pkg("llvm 20.1.2")], &["llvm@19"]),
        one(
            vec![
                pkg("rg 14.1.1").depends(&["so:libpcre2-8.so.0"]),
                pkg("rg 14.1.1 static"),
                pkg("pcre2 10.45").provides(&["so:libpcre2-8.so.0"]),
            ],
            &["rg:static"],
        ),
        Scenario {
            installed: vec![pkg("tool 1.0")],
            ..one(vec![pkg("tool 1.0"), pkg("tool 1.1")], &["tool"])
        },
        Scenario {
            installed: vec![pkg("tool 0.9")],
            upgrade: true,
            ..one(vec![pkg("tool 1.1")], &["tool"])
        },
        one(
            constrained_universe()
                .packages
                .into_iter()
                .map(|e| e.meta)
                .collect(),
            &["foo", "bar"],
        ),
        one(
            constrained_universe()
                .packages
                .into_iter()
                .map(|e| e.meta)
                .collect(),
            &["foo@2", "bar"],
        ),
        one(
            gl_universe(None)
                .packages
                .into_iter()
                .map(|e| e.meta)
                .collect(),
            &["app"],
        ),
        Scenario {
            repos: vec![(gl, 0)],
            requests: vec![req("app")],
            installed: vec![],
            upgrade: false,
        },
        one(
            gl_universe(None)
                .packages
                .into_iter()
                .map(|e| e.meta)
                .collect(),
            &["app", "ga"],
        ),
        one(vec![pkg("a 1").conflicts(&["b"]), pkg("b 1")], &["a", "b"]),
        one(vec![pkg("bar 2").replaces(&["foo"])], &["foo"]),
        Scenario {
            repos: vec![
                (index("core", vec![pkg("foo 2.0")]), 0),
                (index("mine", vec![pkg("foo 1.5")]), 10),
            ],
            requests: vec![req("foo")],
            installed: vec![],
            upgrade: false,
        },
        Scenario {
            repos: vec![
                (index("core", vec![pkg("foo 2.0")]), 0),
                (index("extra", vec![pkg("foo 2.1")]), 0),
            ],
            requests: vec![req("foo")],
            installed: vec![],
            upgrade: false,
        },
        one(
            vec![
                pkg("zlib 1.3.1").provides(&["so:libz.so.1"]),
                pkg("zlib 1.3.1 static"),
                pkg("py 3.12").depends(&["so:libz.so.1"]),
            ],
            &["zlib:static", "py"],
        ),
        one(vec![pkg("rg 14 static")], &["rg", "rg:static"]),
        one(vec![pkg("x 1").depends(&["so:liby.so.1"])], &["x"]),
        one(
            vec![pkg("a 1").depends(&["b"]), pkg("b 1").depends(&["a"])],
            &["a"],
        ),
    ]
}

#[test]
fn example_results_do_not_depend_on_input_order() {
    for scenario in example_scenarios() {
        let expected = scenario.outcome();
        for seed in 0..64 {
            assert_eq!(scenario.shuffled(seed).outcome(), expected);
        }
    }
}

/// A synthetic repository shaped like a real one: libraries that provide
/// sonames and depend on lower libraries, a few with an older version or a
/// static variant, and applications that need several libraries and
/// sometimes a library by name with a version range.
fn synthetic(packages: usize) -> Index {
    let mut rng = Seeded::new(packages as u64);
    let libs = packages * 3 / 5;
    let mut metas = Vec::with_capacity(packages + packages / 10);
    for i in 0..libs {
        let deps: Vec<String> = (0..rng.below(5))
            .filter(|_| i > 0)
            .map(|_| format!("so:lib{}.so.1", rng.below(i)))
            .collect();
        let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
        let provide = format!("so:lib{i}.so.1");
        metas.push(
            pkg(&format!("lib{i} 2.{}", rng.below(9)))
                .depends(&deps)
                .provides(&[&provide]),
        );
        if rng.below(10) == 0 {
            metas.push(
                pkg(&format!("lib{i} 1.0"))
                    .depends(&deps)
                    .provides(&[&provide]),
            );
        }
        if rng.below(20) == 0 {
            metas.push(pkg(&format!("lib{i} 2.0 static")));
        }
    }
    for i in 0..packages - libs {
        let mut deps: Vec<String> = (0..2 + rng.below(7))
            .map(|_| format!("so:lib{}.so.1", rng.below(libs)))
            .collect();
        if rng.below(5) == 0 {
            deps.push(format!("lib{}>=1", rng.below(libs)));
        }
        let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
        metas.push(pkg(&format!("app{i} 1.{}", rng.below(5))).depends(&deps));
    }
    index("core", metas)
}

fn time_resolution(packages: usize, apps: usize) {
    use std::time::Instant;

    let core = synthetic(packages);
    let started = Instant::now();
    let u = universe(&[(&core, 0)]);
    let built = started.elapsed();
    let requests: Vec<Request> = (0..apps).map(|i| req(&format!("app{i}"))).collect();
    let i = input("system", requests, &[]);
    let mut runs = Vec::new();
    let mut closure = 0;
    for _ in 0..20 {
        let started = Instant::now();
        let r = u.resolve(&i).unwrap();
        runs.push(started.elapsed());
        closure = r.packages.len();
    }
    runs.sort();
    println!(
        "universe {} candidates: built in {built:?}, closure {closure}: median {:?}, best {:?}",
        core.packages.len(),
        runs[runs.len() / 2],
        runs[0],
    );
}

/// Measures resolution time on synthetic repositories. Run it with
/// `cargo test --release -p laft-resolve -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a test"]
fn synthetic_repositories_resolve_quickly() {
    time_resolution(1500, 600);
    time_resolution(20_000, 120);
}

fn imposer_universe(with_older_imposer: bool) -> Index {
    let mut packages = vec![
        pkg("app 1").depends(&["lib>=2", "mid"]),
        pkg("mid 2").depends(&["lib<2"]),
        pkg("lib 1"),
        pkg("lib 2"),
    ];
    if with_older_imposer {
        packages.push(pkg("mid 1"));
    }
    index("core", packages)
}

#[test]
fn learned_constraint_gives_way_to_an_older_imposer() {
    let core = imposer_universe(true);
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "lib 2-r1", "mid 1-r1"]);
    assert_eq!(
        r.learned
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["learned to avoid mid 2-r1"]
    );
}

#[test]
fn unresolvable_imposer_conflict_reports_both_constraints() {
    let core = imposer_universe(false);
    let e = resolve_core(&core, &["app"], &[]).unwrap_err();
    let ErrorKind::Unsatisfiable {
        name: n,
        constraints,
        ..
    } = &*e.kind
    else {
        panic!("expected R005, got {e}");
    };
    assert_eq!(n, &name("lib"));
    assert_eq!(
        constraints
            .iter()
            .map(|b| b.dep.to_string())
            .collect::<Vec<_>>(),
        ["lib<2", "lib>=2"]
    );
}

#[test]
fn requested_prefix_admits_an_older_version_within_it() {
    let core = index(
        "core",
        vec![
            pkg("llvm 19.1.7"),
            pkg("llvm 19.1.5"),
            pkg("app 1").depends(&["llvm<19.1.6"]),
        ],
    );
    let r = resolve_core(&core, &["llvm@19", "app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "llvm 19.1.5-r1"]);
}

#[test]
fn pkg_config_constraint_moves_a_kept_provider_to_a_newer_version() {
    let old = pkg("zlib 1.2").provides(&["pc:zlib=1.2", "so:libz.so.1"]);
    let core = index(
        "core",
        vec![
            old.clone(),
            pkg("zlib 1.3").provides(&["pc:zlib=1.3", "so:libz.so.1"]),
            pkg("app 1").depends(&["so:libz.so.1"]),
            pkg("zdev 1").depends(&["pc:zlib>=1.3"]),
        ],
    );
    let r = resolve_core(&core, &["app", "zdev"], &[&old]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "zdev 1-r1", "zlib 1.3-r1"]);
    assert!(!r.packages[&name("zlib")].kept_installed);
    assert_eq!(
        r.learned
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["learned zlib provides pc:zlib>=1.3 from zdev 1-r1"]
    );
}

#[test]
fn prefixed_request_for_a_replaced_name_names_the_replacer() {
    let core = index("core", vec![pkg("bar 2").replaces(&["foo"])]);
    let e = resolve_core(&core, &["foo@1"], &[]).unwrap_err();
    let ErrorKind::NoCandidate { replaced_by, .. } = &*e.kind else {
        panic!("expected R001, got {e}");
    };
    assert_eq!(replaced_by, &[name("bar")]);
}

#[test]
fn dependency_on_a_replaced_name_follows_the_rename() {
    let core = index(
        "core",
        vec![
            pkg("bar 2").replaces(&["foo"]),
            pkg("app 1").depends(&["foo"]),
        ],
    );
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "bar 2-r1"]);
    assert_eq!(
        r.why(&name("bar")).unwrap().chain.steps[0].dep.to_string(),
        "foo"
    );
    let core = index(
        "core",
        vec![
            pkg("bar 2").replaces(&["foo"]),
            pkg("app 1").depends(&["foo>=1"]),
        ],
    );
    assert_eq!(code(resolve_core(&core, &["app"], &[])), "R003");
}

#[test]
fn exhausted_pass_budget_reports_the_first_failure() {
    let mut packages: Vec<Meta> = (1..=1100)
        .map(|v| pkg(&format!("lib {v}")).depends(&["so:libmissing.so.1"]))
        .collect();
    packages.push(pkg("app 1").depends(&["lib"]));
    let core = index("core", packages);
    assert_eq!(code(resolve_core(&core, &["app"], &[])), "R003");
}

#[test]
fn search_gives_up_after_a_bounded_number_of_passes() {
    let core = one_bound_per_pass_universe();
    let e = resolve_core(&core, &["app"], &[]).unwrap_err();
    let ErrorKind::RestartLimit { passes, learned } = &*e.kind else {
        panic!("expected R009, got {e}");
    };
    assert_eq!(*passes, 1000);
    assert!(!learned.is_empty());
}

#[test]
fn yanked_installed_version_outside_the_request_is_not_blamed() {
    let yanked = pkg("tool 1.1");
    let mut core = index(
        "core",
        vec![pkg("tool 1.0"), pkg("app 1").depends(&["tool>=2"])],
    );
    core.yanked.push(yank(&yanked, "corrupts its config"));
    assert_eq!(code(resolve_core(&core, &["tool@2"], &[&yanked])), "R001");
    assert_eq!(
        code(resolve_core(&core, &["tool:static"], &[&yanked])),
        "R001"
    );
    assert_eq!(code(resolve_core(&core, &["app"], &[&yanked])), "R003");
    assert_eq!(code(resolve_core(&core, &["tool@1.1"], &[&yanked])), "R010");
}

#[test]
fn pkg_config_provider_default_is_keyed_by_the_bare_module() {
    let mut core = index(
        "core",
        vec![
            pkg("zlib 1.3").provides(&["pc:zlib=1.3"]),
            pkg("zlib-ng 2.2").provides(&["pc:zlib=1.3.1"]),
            pkg("app 1").depends(&["pc:zlib>=1.3"]),
        ],
    );
    core.defaults
        .providers
        .insert("pc:zlib".into(), name("zlib-ng"));
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "zlib-ng 2.2-r1"]);
}

/// A universe that is known to resolve, because it contains a planted
/// solution next to decoys.
#[derive(Debug, Clone)]
struct Planted {
    core: Index,
    requests: Vec<Request>,
    installed: Vec<Meta>,
    upgrade: bool,
}

const PLANTED_VERSIONS: [&str; 5] = ["1", "1.5", "2", "2.1~rc1", "3"];

/// Planted universes over the names `p0` to `p7`.
///
/// Each planted name has one planted candidate that provides
/// `so:lib<i>.so.1` and `pc:p<i>=<version>` and depends only on other
/// planted candidates, in forms they satisfy. Decoys are other versions and
/// variants of any name with arbitrary dependencies, provides of their own
/// name, conflicts and replaces. Requests name planted packages, sometimes
/// with a prefix of the planted version, and any candidate may be
/// installed.
fn arb_planted() -> impl Strategy<Value = Planted> {
    let names = 8usize;
    let planted = (
        0..PLANTED_VERSIONS.len(),
        prop::bool::weighted(0.7),
        prop::collection::vec((0..names, 0..6usize), 0..3),
    );
    let decoy_dep = prop_oneof![
        (0..names).prop_map(|k| format!("so:lib{k}.so.1")),
        (0..names).prop_map(|k| format!("so:libmissing{k}.so.1")),
        (0..names).prop_map(|k| format!("p{k}")),
        (0..names).prop_map(|k| format!("p{k}<2")),
        (0..names).prop_map(|k| format!("p{k}>=2")),
        (0..names).prop_map(|k| format!("p{k}@1")),
        (0..names).prop_map(|k| format!("pc:p{k}>=2")),
    ];
    let decoy = (
        0..names,
        0..PLANTED_VERSIONS.len(),
        prop::bool::weighted(0.2),
        prop::collection::vec(decoy_dep, 0..3),
        any::<bool>(),
        prop::option::weighted(0.15, 0..names),
        prop::option::weighted(0.05, 0..names),
    );
    (
        prop::collection::vec(planted, names),
        prop::collection::vec(decoy, 0..20),
        prop::collection::vec((0..names, prop::bool::weighted(0.3)), 1..4),
        prop::collection::vec(
            prop::option::weighted(0.3, 0..PLANTED_VERSIONS.len() + 1),
            names,
        ),
        any::<bool>(),
    )
        .prop_map(|(planted, decoys, requests, installed, upgrade)| {
            let in_set: Vec<usize> = (0..planted.len()).filter(|i| planted[*i].1).collect();
            let in_set = if in_set.is_empty() { vec![0] } else { in_set };
            let version = |i: usize| PLANTED_VERSIONS[planted[i].0];
            let major = |i: usize| version(i).split('.').next().unwrap_or("1").to_owned();
            let mut metas = Vec::new();
            for &i in &in_set {
                let deps: Vec<String> = planted[i]
                    .2
                    .iter()
                    .map(|(target, kind)| {
                        let j = in_set[target % in_set.len()];
                        match kind {
                            0 => format!("p{j}"),
                            1 => format!("p{j}>={}", version(j)),
                            2 => format!("p{j}<={}", version(j)),
                            3 => format!("p{j}@{}", major(j)),
                            4 => format!("so:lib{j}.so.1"),
                            _ => format!("pc:p{j}>={}", version(j)),
                        }
                    })
                    .filter(|d| !d.starts_with(&format!("p{i}")) || d.starts_with("pc:"))
                    .collect();
                let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
                let provides = [format!("so:lib{i}.so.1"), format!("pc:p{i}={}", version(i))];
                let provides: Vec<&str> = provides.iter().map(String::as_str).collect();
                metas.push(
                    pkg(&format!("p{i} {}", version(i)))
                        .depends(&deps)
                        .provides(&provides),
                );
            }
            for (n, v, is_static, deps, provides, conflict, replace) in decoys {
                let text = format!(
                    "p{n} {}{}",
                    PLANTED_VERSIONS[v],
                    if is_static { " static" } else { "" }
                );
                let label = |m: &Meta| format!("{} {} {:?}", m.name, m.version, m.variant);
                let decoy = pkg(&text);
                if metas.iter().any(|m| label(m) == label(&decoy)) {
                    continue;
                }
                let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
                let provides: Vec<String> = if provides && !is_static {
                    vec![
                        format!("so:lib{n}.so.1"),
                        format!("pc:p{n}={}", PLANTED_VERSIONS[v]),
                    ]
                } else {
                    vec![]
                };
                let provides: Vec<&str> = provides.iter().map(String::as_str).collect();
                let conflicts: Vec<String> =
                    conflict.map(|c| format!("p{c}")).into_iter().collect();
                let conflicts: Vec<&str> = conflicts.iter().map(String::as_str).collect();
                let replaces: Vec<String> = replace.map(|c| format!("p{c}")).into_iter().collect();
                let replaces: Vec<&str> = replaces.iter().map(String::as_str).collect();
                metas.push(
                    decoy
                        .depends(&deps)
                        .provides(&provides)
                        .conflicts(&conflicts)
                        .replaces(&replaces),
                );
            }
            let requests = requests
                .into_iter()
                .map(|(i, with_prefix)| {
                    let i = in_set[i % in_set.len()];
                    if with_prefix {
                        req(&format!("p{i}@{}", major(i)))
                    } else {
                        req(&format!("p{i}"))
                    }
                })
                .collect();
            let mut chosen = BTreeSet::new();
            let installed = installed
                .into_iter()
                .enumerate()
                .filter_map(|(n, pick)| {
                    let pick = pick?;
                    let of_name: Vec<&Meta> = metas
                        .iter()
                        .filter(|m| m.name.as_str() == format!("p{n}"))
                        .collect();
                    let meta = of_name.get(pick % of_name.len().max(1))?;
                    chosen.insert(meta.name.clone()).then(|| (*meta).clone())
                })
                .collect();
            Planted {
                core: index("core", metas),
                requests,
                installed,
                upgrade,
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn universes_with_a_solution_resolve(planted in arb_planted()) {
        let installed: Vec<&Meta> = planted.installed.iter().collect();
        let mut i = input("system", planted.requests.clone(), &installed);
        if planted.upgrade {
            i.upgrade = Upgrade::All;
        }
        let result = universe(&[(&planted.core, 0)]).resolve(&i);
        match result {
            Ok(r) => check_sound(&r, &planted.requests),
            Err(e) => prop_assert!(false, "{e}"),
        }
    }
}
