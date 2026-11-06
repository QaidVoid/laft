use std::collections::BTreeSet;

use laft_core::Dep;
use laft_index::Index;
use proptest::prelude::*;

use super::*;
use crate::test_support::*;
use crate::universe::provides;
use crate::{Repository, Universe};

fn code<T>(result: Result<T, Error>) -> &'static str {
    match result {
        Ok(_) => panic!("the resolution should fail"),
        Err(e) => e.code().unwrap_or("none"),
    }
}

#[test]
fn the_local_repository_wins_only_on_the_same_identity() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.3"),
            pkg("mesa 25.1").options(&[("llvm", "false")]),
        ],
    );
    let local = index(
        "local",
        vec![pkg("mesa 25.1").options(&[("llvm", "false")])],
    );
    let repos = [
        Repository {
            index: &core,
            priority: 0,
            local: false,
        },
        Repository {
            index: &local,
            priority: i64::MAX,
            local: true,
        },
    ];
    let u = Universe::new(&repos);
    let r = u.resolve(&input("system", vec![req("mesa")], &[])).unwrap();
    assert_eq!(
        labels(&r),
        ["mesa 25.3-r1"],
        "the owner's newest version wins over the local build"
    );
    let r = u
        .resolve(&input("system", vec![req("mesa{!llvm}")], &[]))
        .unwrap();
    assert_eq!(labels(&r), ["mesa 25.1-r1"]);
    match r.packages[&name("mesa")].candidate.source {
        Source::Index { repo, .. } => {
            assert_eq!(
                *repo,
                name("local"),
                "the local build wins on the same identity"
            );
        }
        Source::Installed => panic!("an index entry is selected"),
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
    assert_eq!(e.code(), Some("S005"));
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
    assert_eq!(code(resolve_core(&core, &["app"], &[])), "S004");
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
    assert_eq!(code(resolve_core(&core, &["a", "b"], &[])), "S006");
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
        "S008"
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
        "S007"
    );
}

/// An option that removes a shared library: the error names the option
/// values of the selected build that lost it.
#[test]
fn a_missing_library_names_the_option_of_the_selected_build() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0")
                .options(&[("x11", "true")])
                .provides(&["so:libEGL_mesa.so.0"]),
            pkg("mesa 25.2.0").options(&[("x11", "false")]),
            pkg("kwin 6.3.0").depends(&["so:libEGL_mesa.so.0"]),
        ],
    );
    let i = input_with(
        "system",
        vec![req("kwin")],
        &[],
        &[fixed(
            "mesa",
            &[("x11", "false", "/etc/laft/system.stav:12:3")],
        )],
    );
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S005"));
    assert!(
        e.to_string()
            .contains("which mesa with x11=false does not provide"),
        "{}",
        e
    );
}

/// The same loss on a build the view chose: the selected package is the
/// wrong provider, and the error names its option values.
#[test]
fn an_installed_build_without_a_library_names_its_options() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0")
                .options(&[("x11", "true")])
                .provides(&["so:libEGL_mesa.so.0"]),
            pkg("mesa 25.2.0").options(&[("x11", "false")]),
            pkg("kwin 6.3.0").depends(&["so:libEGL_mesa.so.0"]),
        ],
    );
    let i = input_with("system", vec![req("mesa{!x11}"), req("kwin")], &[], &[]);
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S007"));
    assert!(
        e.to_string()
            .contains("which mesa with x11=false does not provide"),
        "{}",
        e
    );
}

/// The same view with the requests in the other order: the dependency on
/// mesa is learned as a constraint and drives the same build.
#[test]
fn a_so_dependency_is_met_by_a_pending_build_from_either_order() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0")
                .options(&[("vulkan", "all")])
                .provides(&["so:libEGL.so.1"]),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=amd}"]),
            pkg("kwin 6.0").depends(&["so:libEGL.so.1"]),
        ],
    );
    let mut i = input("system", vec![req("kwin"), req("blender")], &[]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[("vulkan", "all")])];
    i.allow_builds = true;
    let r = universe(&[(&core, 0)])
        .resolve(&i)
        .expect("the view resolves");
    assert!(
        !r.packages.contains_key(&name("mesa")),
        "mesa is built, not also selected: {:?}",
        labels(&r)
    );
    let build = r
        .builds
        .iter()
        .find(|b| b.buildable.name == name("mesa"))
        .expect("mesa is built");
    assert_eq!(
        build.values.get("vulkan").map(ToString::to_string),
        Some("amd".to_owned())
    );
    assert!(build.reasons.iter().any(|reason| *reason
        == Reason::Dependency {
            by: name("kwin"),
            dep: "so:libEGL.so.1".parse().unwrap(),
        }));
}

/// A provider excluded by a version prefix rather than the option
/// values: the error carries no note blaming the options.
#[test]
fn a_provider_lost_to_a_prefix_is_not_blamed_on_the_options() {
    let core = index(
        "core",
        vec![
            pkg("mesa 1.0")
                .options(&[("x11", "true")])
                .provides(&["so:libEGL_mesa.so.0"]),
            pkg("mesa 2.0")
                .options(&[("x11", "false")])
                .provides(&["so:libEGL_mesa.so.0"]),
            pkg("kwin 6.3.0").depends(&["so:libEGL_mesa.so.0"]),
        ],
    );
    let i = input_with("system", vec![req("mesa@1{!x11}"), req("kwin")], &[], &[]);
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S005"));
    assert!(!e.to_string().contains("does not provide"), "{}", e);
}

/// A provider that lost two option values: the hint names both, sorted
/// and joined.
#[test]
fn a_hint_for_two_differing_options_names_both() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0")
                .options(&[("llvm", "true"), ("x11", "true")])
                .provides(&["so:libEGL_mesa.so.0"]),
            pkg("mesa 25.2.0").options(&[("llvm", "false"), ("x11", "false")]),
            pkg("kwin 6.3.0").depends(&["so:libEGL_mesa.so.0"]),
        ],
    );
    let i = input_with(
        "system",
        vec![req("mesa{!llvm,!x11}"), req("kwin")],
        &[],
        &[],
    );
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S007"));
    assert!(
        e.to_string()
            .contains("which mesa with llvm=false, x11=false does not provide"),
        "{}",
        e
    );
}

/// A fixed value that differs from the preferred one names the setting's
/// origin in the reason.
#[test]
fn a_fixed_value_off_the_preference_names_its_origin() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0")
                .options(&[("vulkan", "amd")])
                .provides(&["so:libEGL_mesa.so.0"]),
            pkg("kwin 6.3.0").depends(&["so:libEGL_mesa.so.0"]),
        ],
    );
    let mut request = fixed("mesa", &[("vulkan", "amd", "/etc/laft/system.stav:12:3")]).1;
    request
        .preferred
        .insert("vulkan".to_owned(), "all".parse().expect("valid value"));
    let i = input_with(
        "system",
        vec![req("kwin")],
        &[],
        &[("mesa".parse().expect("valid"), request)],
    );
    let r = universe(&[(&core, 0)]).resolve(&i).expect("resolves");
    assert!(
        r.packages[&name("mesa")]
            .reasons
            .contains(&Reason::OptionMoved {
                option: "vulkan".into(),
                chosen: "amd".parse().expect("valid value"),
                preferred: "all".parse().expect("valid value"),
                why: "set at /etc/laft/system.stav:12:3".into(),
            })
    );
}

/// A dependency that demands another value than the preferred one names
/// itself in the reason.
#[test]
fn a_dependency_moving_a_value_off_the_preference_names_itself() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0")
                .options(&[("vulkan", "amd")])
                .provides(&["so:libEGL_mesa.so.0"]),
            pkg("app 1").depends(&["mesa{vulkan=amd}"]),
            pkg("kwin 6.3.0").depends(&["so:libEGL_mesa.so.0"]),
        ],
    );
    let request = OptionRequest {
        fixed: BTreeMap::new(),
        preferred: BTreeMap::from([("vulkan".to_owned(), "all".parse().expect("valid value"))]),
    };
    let i = input_with(
        "system",
        vec![req("app"), req("kwin")],
        &[],
        &[("mesa".parse().expect("valid"), request)],
    );
    let r = universe(&[(&core, 0)]).resolve(&i).expect("resolves");
    assert!(
        r.packages[&name("mesa")]
            .reasons
            .contains(&Reason::OptionMoved {
                option: "vulkan".into(),
                chosen: "amd".parse().expect("valid value"),
                preferred: "all".parse().expect("valid value"),
                why: "needed by app 1-r1 (mesa{vulkan=amd})".into(),
            })
    );
}

/// A request constraint that moves a value off the preference names its
/// request in the reason.
#[test]
fn a_request_moving_a_value_off_the_preference_names_itself() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0")
                .options(&[("vulkan", "amd")])
                .provides(&["so:libEGL_mesa.so.0"]),
            pkg("kwin 6.3.0").depends(&["so:libEGL_mesa.so.0"]),
        ],
    );
    let request = OptionRequest {
        fixed: BTreeMap::new(),
        preferred: BTreeMap::from([("vulkan".to_owned(), "all".parse().expect("valid value"))]),
    };
    let i = input_with(
        "system",
        vec![req_at("mesa{vulkan=amd}", "laft.stav:4:9"), req("kwin")],
        &[],
        &[("mesa".parse().expect("valid"), request)],
    );
    let r = universe(&[(&core, 0)]).resolve(&i).expect("resolves");
    assert!(
        r.packages[&name("mesa")]
            .reasons
            .contains(&Reason::OptionMoved {
                option: "vulkan".into(),
                chosen: "amd".parse().expect("valid value"),
                preferred: "all".parse().expect("valid value"),
                why: "asked at laft.stav:4:9 (mesa{vulkan=amd})".into(),
            })
    );
}

/// A value at its preferred value has no deviation reason.
#[test]
fn a_value_at_its_preference_has_no_reason() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0")
                .options(&[("vulkan", "all")])
                .provides(&["so:libEGL_mesa.so.0"]),
            pkg("kwin 6.3.0").depends(&["so:libEGL_mesa.so.0"]),
        ],
    );
    let request = OptionRequest {
        fixed: BTreeMap::new(),
        preferred: BTreeMap::from([("vulkan".to_owned(), "all".parse().expect("valid value"))]),
    };
    let i = input_with(
        "system",
        vec![req("kwin")],
        &[],
        &[("mesa".parse().expect("valid"), request)],
    );
    let r = universe(&[(&core, 0)]).resolve(&i).expect("resolves");
    assert!(
        r.packages[&name("mesa")]
            .reasons
            .iter()
            .all(|reason| !matches!(reason, Reason::OptionMoved { .. }))
    );
}

#[test]
fn requests_for_two_variants_conflict() {
    let core = index("core", vec![pkg("rg 14 static")]);
    assert_eq!(code(resolve_core(&core, &["rg", "rg:static"], &[])), "S002");
}

#[test]
fn missing_provider_fails_with_the_chain() {
    let core = index("core", vec![pkg("x 1").depends(&["so:liby.so.1"])]);
    let e = resolve_core(&core, &["x"], &[]).unwrap_err();
    let ErrorKind::Unsatisfied { dep, by, chain, .. } = *e.kind else {
        panic!("expected S003, got {e}");
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
    assert_eq!(code(resolve_core(&core, &["tool"], &[&yanked])), "S010");
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
    assert_eq!(code(resolve_core(&core, &["app"], &[])), "S003");
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
        "S002"
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
        panic!("expected S001, got {e}");
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
    assert_eq!(code(resolve_core(&core, &["postfix", "exim"], &[])), "S006");
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
fn the_highest_priority_member_wins_when_nothing_chooses() {
    let mut core = index(
        "core",
        vec![
            pkg("neovim 0.11").ranked_alternative("vi", &["usr/bin/vi"], 20),
            pkg("nvi 1.8").ranked_alternative("vi", &["usr/bin/vi"], -5),
            pkg("vim 9.1").ranked_alternative("vi", &["usr/bin/vi"], 10),
            pkg("busybox 1.37").ranked_alternative("vi", &["usr/bin/vi"], 20),
        ],
    );
    let u = universe(&[(&core, 0)]);
    let r = u
        .resolve(&input(
            "system",
            vec![req("vim"), req("neovim"), req("nvi")],
            &[],
        ))
        .unwrap();
    assert_eq!(r.alternatives[&name("vi")], name("neovim"));

    let tied = vec![req("vim"), req("neovim"), req("busybox")];
    let e = u.resolve(&input("system", tied.clone(), &[])).unwrap_err();
    assert!(matches!(*e.kind, ErrorKind::AlternativeAmbiguous { .. }));

    let mut i = input("system", tied.clone(), &[]);
    i.alternatives.insert(name("vi"), name("vim"));
    assert_eq!(
        u.resolve(&i).unwrap().alternatives[&name("vi")],
        name("vim")
    );

    core.defaults.alternatives.insert(name("vi"), name("vim"));
    let u = universe(&[(&core, 0)]);
    let r = u.resolve(&input("system", tied, &[])).unwrap();
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
        panic!("expected S001, got {e}");
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
fn check_sound(r: &Resolution<'_>, requests: &[Request], options: &BTreeMap<Name, OptionRequest>) {
    for build in &r.builds {
        assert!(
            !r.packages.contains_key(&build.buildable.name),
            "{} is both selected and a build",
            build.buildable.name
        );
    }
    for (name, selected) in &r.packages {
        let meta = selected.candidate.meta;
        assert_eq!(&meta.name, name);
        if let Some(req) = options.get(name) {
            // A preferred value may lose to a newer version: the
            // preference order of spec 17 §6.4 ranks deviations below the
            // version, so only fixed values are hard for a package.
            for (opt, fixed) in &req.fixed {
                assert_eq!(
                    meta.options.get(opt),
                    Some(&fixed.value),
                    "{name} does not record the fixed value of {opt}"
                );
            }
        }
        for dep in &meta.depends {
            let ok = match dep {
                Dep::Pkg(n, req, opts) => match r.packages.get(n) {
                    Some(s) => {
                        req.matches(&s.candidate.meta.version, s.candidate.meta.rev)
                            && constraints_hold(&s.candidate.meta.options, opts)
                    }
                    None => match r.builds.iter().find(|b| &b.buildable.name == n) {
                        Some(b) => {
                            req.matches(&b.buildable.version, b.buildable.rev)
                                && constraints_hold(&b.values, opts)
                        }
                        None => {
                            *req == VersionReq::Any
                                && r.packages.values().any(|s| {
                                    s.candidate
                                        .meta
                                        .replaces
                                        .iter()
                                        .any(|d| matches!(d, Dep::Pkg(x, _, _) if x == n))
                                })
                        }
                    },
                },
                _ => {
                    r.packages.values().any(|s| provides(s.candidate.meta, dep))
                        || r.builds.iter().any(|b| {
                            b.reasons.iter().any(
                                |reason| matches!(reason, Reason::Dependency { dep: d, .. } if d == dep),
                            )
                        })
                }
            };
            assert!(ok, "{name} needs {dep}, which is not satisfied");
        }
        for dep in meta.conflicts.iter().chain(&meta.replaces) {
            let hit = match dep {
                Dep::Pkg(n, req, _) => {
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
            assert!(
                constraints_hold_strict(&s.candidate.meta.options, request.pkg.options()),
                "request {} is met by a candidate without its option values",
                request.pkg
            );
        } else if let Some(b) = r
            .builds
            .iter()
            .find(|b| b.buildable.name == *request.pkg.name())
        {
            assert_eq!(b.buildable.variant, request.pkg.variant());
            if let Some(prefix) = request.pkg.prefix() {
                assert!(prefix.matches(&b.buildable.version));
            }
            assert!(
                constraints_hold_strict(&b.values, request.pkg.options()),
                "request {} is met by a build without its option values",
                request.pkg
            );
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
    for build in &r.builds {
        if let Some(req) = options.get(&build.buildable.name) {
            for (opt, fixed) in &req.fixed {
                assert_eq!(
                    build.values.get(opt),
                    Some(&fixed.value),
                    "the build of {} would ignore the fixed value of {opt}",
                    build.buildable.name
                );
            }
            // The values of a build start from the declared defaults and
            // take the preferred values over them, so a preference that
            // neither a fixed value nor the option constraints of a
            // satisfied dependency overrides holds.
            for (opt, value) in &req.preferred {
                if req.fixed.contains_key(opt)
                    || build.reasons.iter().any(|reason| {
                        matches!(
                            reason,
                            Reason::Dependency { dep: Dep::Pkg(_, _, opts), .. }
                                if opts.as_slice().iter().any(|c| c.name() == opt)
                        )
                    })
                    || r.learned.iter().any(|learned| {
                        matches!(
                            learned,
                            Learned::Require { name, dep: Dep::Pkg(_, _, opts), .. }
                                if *name == build.buildable.name
                                    && opts.as_slice().iter().any(|c| c.name() == opt)
                        )
                    })
                {
                    continue;
                }
                assert_eq!(
                    build.values.get(opt),
                    Some(value),
                    "the build of {} would ignore the preferred value of {opt}",
                    build.buildable.name
                );
            }
        }
    }
}

/// A random universe: two repositories, packages `p0` to `p9` in a few
/// versions and variants, `so:` and name dependencies, provides, conflicts
/// and replaces, plus requests and an installed set.
#[derive(Debug, Clone, Default)]
struct Scenario {
    repos: Vec<(Index, i64)>,
    requests: Vec<Request>,
    installed: Vec<Meta>,
    upgrade: bool,
    /// The fixed and preferred option values of the view.
    input_options: BTreeMap<Name, OptionRequest>,
    /// The packages a recipe snapshot can build.
    buildable: Vec<Buildable>,
    /// Whether the view may select a package the snapshot builds.
    allow_builds: bool,
}

/// The option values a generated package may record.
fn recorded_option() -> impl Strategy<Value = (&'static str, &'static str)> {
    prop_oneof![
        Just(("x11", "true")),
        Just(("x11", "false")),
        Just(("llvm", "true")),
        Just(("llvm", "false")),
        Just(("vaapi", "amd")),
        Just(("vaapi", "intel")),
        Just(("vaapi", "all")),
    ]
}

fn arb_scenario(max_packages: usize) -> impl Strategy<Value = Scenario> {
    let names = 10usize;
    let dep = prop_oneof![
        (0..5usize).prop_map(|k| format!("so:lib{k}.so.1")),
        (0..names).prop_map(|n| format!("p{n}")),
        (0..names).prop_map(|n| format!("p{n}<2")),
        (0..names).prop_map(|n| format!("p{n}>=2")),
        (0..names).prop_map(|n| format!("p{n}@1")),
        (0..names).prop_map(|n| format!("p{n}{{x11}}")),
        (0..names).prop_map(|n| format!("p{n}{{!x11}}")),
        (0..names).prop_map(|n| format!("p{n}{{vaapi=amd}}")),
        (0..names).prop_map(|n| format!("p{n}{{vaapi=intel}}")),
    ];
    // One recorded option value of a generated package.
    let recorded = recorded_option();
    let package = (
        0..names,
        prop::sample::select(vec!["1", "1.5", "2", "2.1~rc1", "3"]),
        prop::bool::weighted(0.15),
        prop::collection::vec(dep, 0..3),
        prop::collection::vec(0..5usize, 0..3),
        prop::option::weighted(0.1, 0..names),
        prop::option::weighted(0.05, 0..names),
        prop::collection::vec(recorded, 0..2),
        0..2usize,
    );
    let request = (any::<prop::sample::Index>(), prop::bool::weighted(0.15));
    let view_value = (0..names, recorded_option(), any::<bool>());
    (
        prop::collection::vec(package, max_packages / 4..max_packages),
        prop::collection::vec(request, 1..5),
        0..3i64,
        prop_oneof![9 => Just(1i64), 9 => Just(-1), 1 => Just(0)],
        prop::collection::vec(any::<bool>(), max_packages),
        prop::collection::vec(view_value, 0..3),
        prop::bool::weighted(0.2),
        any::<bool>(),
    )
        .prop_map(
            |(
                packages,
                requests,
                prio0,
                delta,
                installed_mask,
                view_values,
                allow_builds,
                upgrade,
            )| {
                let mut repos = vec![(Vec::new(), prio0), (Vec::new(), prio0 + delta)];
                let mut all: Vec<Meta> = Vec::new();
                let mut seen = BTreeSet::new();
                let mut installed: Vec<Meta> = Vec::new();
                for (i, (n, version, is_static, deps, provides, conflict, replace, values, repo)) in
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
                    let mut recorded: BTreeMap<&str, &str> = BTreeMap::new();
                    for (opt, value) in values {
                        recorded.entry(opt).or_insert(value);
                    }
                    let recorded: Vec<(&str, &str)> = recorded.into_iter().collect();
                    let meta = pkg(&format!("p{n} {version}{variant}"))
                        .depends(&deps)
                        .provides(&provides)
                        .conflicts(&conflicts)
                        .replaces(&replaces)
                        .options(&recorded);
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
                let mut input_options: BTreeMap<Name, OptionRequest> = BTreeMap::new();
                for (n, (opt, value), fixed) in view_values {
                    let value: OptionValue = value.parse().expect("test option value is valid");
                    let req = input_options.entry(name(&format!("p{n}"))).or_default();
                    if fixed && !req.fixed.contains_key(opt) {
                        req.fixed.insert(
                            opt.to_owned(),
                            FixedValue {
                                value,
                                origin: "/etc/laft/gen.stav:1:1".to_owned(),
                            },
                        );
                    } else if !fixed && !req.preferred.contains_key(opt) {
                        req.preferred.insert(opt.to_owned(), value);
                    }
                }
                let mut seen_buildable = BTreeSet::new();
                let buildable: Vec<Buildable> = all
                    .iter()
                    .step_by(7)
                    .filter(|m| {
                        seen_buildable.insert((
                            m.name.clone(),
                            m.version.clone(),
                            m.rev,
                            m.variant,
                            m.options.clone(),
                        ))
                    })
                    .map(|m| Buildable {
                        name: m.name.clone(),
                        version: m.version.clone(),
                        rev: m.rev,
                        variant: m.variant,
                        defaults: m.options.clone(),
                    })
                    .collect();
                Scenario {
                    repos,
                    requests,
                    installed,
                    upgrade,
                    input_options,
                    buildable,
                    allow_builds,
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
        i.options = self.input_options.clone();
        i.buildable = self.buildable.clone();
        i.allow_builds = self.allow_builds;
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
        rng.shuffle(&mut copy.buildable);
        copy
    }
}

/// A buildable of each variant: the static one does not displace the
/// dynamic one, and a fixed value reaches the build of either variant.
#[test]
fn each_variant_keeps_its_own_buildable() {
    let core = index("core", vec![pkg("zlib 1.3.1")]);
    let mut i = input_with(
        "system",
        vec![req("zlib")],
        &[],
        &[fixed(
            "zlib",
            &[("x", "false", "/etc/laft/system.stav:3:1")],
        )],
    );
    i.buildable = vec![
        buildable("zlib", "1.3.2", &[("x", "true")]),
        Buildable {
            variant: Variant::Static,
            ..buildable("zlib", "1.3.2", &[("x", "true")])
        },
    ];
    i.allow_builds = true;
    let r = universe(&[(&core, 0)])
        .resolve(&i)
        .expect("the view resolves");
    assert_eq!(labels(&r), Vec::<String>::new());
    assert_eq!(r.builds.len(), 1);
    assert_eq!(r.builds[0].buildable.variant, Variant::Dynamic);
    assert_eq!(
        r.builds[0].values.get("x"),
        Some(&laft_core::OptionValue::Bool(false))
    );
    let mut i = input_with(
        "system",
        vec![req("zlib:static")],
        &[],
        &[fixed(
            "zlib",
            &[("x", "false", "/etc/laft/system.stav:3:1")],
        )],
    );
    i.buildable = vec![
        buildable("zlib", "1.3.2", &[("x", "true")]),
        Buildable {
            variant: Variant::Static,
            ..buildable("zlib", "1.3.2", &[("x", "true")])
        },
    ];
    i.allow_builds = true;
    let r = universe(&[(&core, 0)])
        .resolve(&i)
        .expect("the view resolves");
    assert_eq!(r.builds.len(), 1);
    assert_eq!(r.builds[0].buildable.variant, Variant::Static);
    assert_eq!(
        r.builds[0].values.get("x"),
        Some(&laft_core::OptionValue::Bool(false))
    );
}

/// Builds denied still resolve like builds allowed, and the S011 names
/// the values of the build the resolution would have run, not the values
/// of the pass that first reached for a build.
#[test]
fn s011_reports_the_values_of_the_resolution_when_builds_are_denied() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vulkan", "all")]),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=amd}"]),
            pkg("blender 4.4.0").depends(&["mesa"]),
        ],
    );
    let mut i = input("system", vec![req("blender"), req("mesa")], &[]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[("vulkan", "all")])];
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S011"));
    assert!(e.to_string().contains("mesa with vulkan=amd"), "{}", e);
    i.allow_builds = true;
    let r = universe(&[(&core, 0)])
        .resolve(&i)
        .expect("the view resolves");
    assert_eq!(labels(&r), ["blender 4.5.0-r1"]);
    assert_eq!(r.builds.len(), 1);
    assert_eq!(
        r.builds[0].values.get("vulkan"),
        Some(&OptionValue::Tag("amd".into()))
    );
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
        i.options = scenario.input_options.clone();
        i.buildable = scenario.buildable.clone();
        i.allow_builds = scenario.allow_builds;
        if let Ok(r) = universe(&repos).resolve(&i) {
            check_sound(&r, &scenario.requests, &i.options);
        }
    }

    #[test]
    fn large_universes_resolve_soundly(scenario in arb_scenario(200)) {
        let repos: Vec<(&Index, i64)> = scenario.repos.iter().map(|(i, p)| (i, *p)).collect();
        let installed: Vec<&Meta> = scenario.installed.iter().collect();
        let mut i = input("system", scenario.requests.clone(), &installed);
        if scenario.upgrade {
            i.upgrade = Upgrade::All;
        }
        i.options = scenario.input_options.clone();
        i.buildable = scenario.buildable.clone();
        i.allow_builds = scenario.allow_builds;
        if let Ok(r) = universe(&repos).resolve(&i) {
            check_sound(&r, &scenario.requests, &i.options);
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
        ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        },
        Scenario {
            repos: vec![
                (index("core", vec![pkg("foo 2.0")]), 0),
                (index("extra", vec![pkg("foo 2.1")]), 0),
            ],
            requests: vec![req("foo")],
            ..Default::default()
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
        // A dependency constraint restarts over the newest candidate.
        one(
            vec![
                pkg("mesa 25.2").options(&[("vaapi", "intel")]),
                pkg("mesa 25.1").options(&[("vaapi", "amd")]),
                pkg("gl 1").depends(&["mesa"]),
                pkg("hevc 1").depends(&["mesa{vaapi=amd}"]),
            ],
            &["gl", "hevc"],
        ),
        // Two requests demanding different values of one option.
        one(
            vec![
                pkg("mesa 25.2").options(&[("vaapi", "intel")]),
                pkg("mesa 25.1").options(&[("vaapi", "amd")]),
            ],
            &["mesa{vaapi=intel}", "mesa{vaapi=amd}"],
        ),
        // A fixed value contradicting a dependency.
        Scenario {
            repos: vec![(
                index(
                    "core",
                    vec![
                        pkg("mesa 25.2").options(&[("vulkan", "all")]),
                        pkg("bl 1").depends(&["mesa{vulkan=all}"]),
                    ],
                ),
                0,
            )],
            requests: vec![req("bl")],
            input_options: BTreeMap::from([fixed_option(
                "mesa",
                ("vulkan", "amd", "/etc/laft/gen.stav:1:1"),
            )]),
            ..Default::default()
        },
        // A build the view selects with the preferred value of the view.
        Scenario {
            repos: vec![(index("core", vec![pkg("bl 1").depends(&["mesa"])]), 0)],
            requests: vec![req("bl")],
            input_options: BTreeMap::from([preferred_option("mesa", ("vaapi", "amd"))]),
            buildable: vec![buildable("mesa", "25.3", &[("vaapi", "intel")])],
            allow_builds: true,
            ..Default::default()
        },
        // The same build without the permission fails with S011.
        Scenario {
            repos: vec![(index("core", vec![pkg("bl 1").depends(&["mesa"])]), 0)],
            requests: vec![req("bl")],
            buildable: vec![buildable("mesa", "25.3", &[])],
            ..Default::default()
        },
    ]
}

/// One fixed option value for the view, as a map entry for a scenario.
fn fixed_option(of: &str, (opt, value, origin): (&str, &str, &str)) -> (Name, OptionRequest) {
    (
        name(of),
        OptionRequest {
            fixed: BTreeMap::from([(
                opt.to_owned(),
                FixedValue {
                    value: value.parse().expect("test option value is valid"),
                    origin: origin.to_owned(),
                },
            )]),
            preferred: BTreeMap::new(),
        },
    )
}

/// One preferred option value for the view, as a map entry for a scenario.
fn preferred_option(of: &str, (opt, value): (&str, &str)) -> (Name, OptionRequest) {
    (
        name(of),
        OptionRequest {
            fixed: BTreeMap::new(),
            preferred: BTreeMap::from([(
                opt.to_owned(),
                value.parse().expect("test option value is valid"),
            )]),
        },
    )
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
        panic!("expected S005, got {e}");
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
        panic!("expected S001, got {e}");
    };
    assert_eq!(replaced_by, &[name("bar")]);
}

#[test]
fn a_renamed_dependency_checks_its_option_constraints() {
    let core = index(
        "core",
        vec![
            pkg("bar 2")
                .options(&[("vaapi", "intel")])
                .replaces(&["foo"]),
            pkg("app 1").depends(&["foo{vaapi=amd}"]),
        ],
    );
    assert_eq!(code(resolve_core(&core, &["app"], &[])), "S003");

    let core = index(
        "core",
        vec![
            pkg("bar 2")
                .options(&[("vaapi", "intel")])
                .replaces(&["foo"]),
            pkg("app 1").depends(&["foo{vaapi=intel}"]),
        ],
    );
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "bar 2-r1"]);
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
    assert_eq!(code(resolve_core(&core, &["app"], &[])), "S003");
}

#[test]
fn exhausted_pass_budget_reports_the_first_failure() {
    let mut packages: Vec<Meta> = (1..=1100)
        .map(|v| pkg(&format!("lib {v}")).depends(&["so:libmissing.so.1"]))
        .collect();
    packages.push(pkg("app 1").depends(&["lib"]));
    let core = index("core", packages);
    assert_eq!(code(resolve_core(&core, &["app"], &[])), "S003");
}

#[test]
fn search_gives_up_after_a_bounded_number_of_passes() {
    let core = one_bound_per_pass_universe();
    let e = resolve_core(&core, &["app"], &[]).unwrap_err();
    let ErrorKind::RestartLimit { passes, learned } = &*e.kind else {
        panic!("expected S009, got {e}");
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
    assert_eq!(code(resolve_core(&core, &["tool@2"], &[&yanked])), "S001");
    assert_eq!(
        code(resolve_core(&core, &["tool:static"], &[&yanked])),
        "S001"
    );
    assert_eq!(code(resolve_core(&core, &["app"], &[&yanked])), "S003");
    assert_eq!(code(resolve_core(&core, &["tool@1.1"], &[&yanked])), "S010");
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

#[test]
fn an_option_constraint_rejects_a_candidate_built_with_other_values() {
    let without_x11 = pkg("mesa 25.1.0").options(&[("x11", "false")]);
    let with_x11 = pkg("mesa 25.2.0").options(&[("x11", "true")]);
    let core = index(
        "core",
        vec![
            with_x11,
            without_x11.clone(),
            pkg("kwin 6").depends(&["mesa{!x11}"]),
        ],
    );
    let r = resolve_core(&core, &["kwin"], &[]).unwrap();
    assert_eq!(labels(&r), ["kwin 6-r1", "mesa 25.1.0-r1"]);

    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("x11", "true")]),
            pkg("kwin 6").depends(&["mesa{!x11}"]),
        ],
    );
    let e = resolve_core(&core, &["kwin"], &[]).unwrap_err();
    assert_eq!(e.code(), Some("S005"));
}

#[test]
fn an_option_constraint_restarts_over_the_newest_candidate() {
    let intel = pkg("mesa 25.2.0").options(&[("vulkan", "intel")]);
    let amd = pkg("mesa 25.1.0").options(&[("vulkan", "amd")]);
    let core = index(
        "core",
        vec![
            intel,
            amd.clone(),
            pkg("glbench 1").depends(&["mesa"]),
            pkg("hevc 1").depends(&["mesa{vulkan=amd}"]),
        ],
    );
    let r = resolve_core(&core, &["glbench", "hevc"], &[]).unwrap();
    assert_eq!(labels(&r), ["glbench 1-r1", "hevc 1-r1", "mesa 25.1.0-r1"]);
    assert_eq!(
        r.learned
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["learned mesa{vulkan=amd} from hevc 1-r1"]
    );
}

#[test]
fn an_option_constraint_rejects_an_already_selected_candidate() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("x11", "true")]),
            pkg("kwin 6").depends(&["mesa{!x11}"]),
        ],
    );
    let e = resolve_core(&core, &["mesa", "kwin"], &[]).unwrap_err();
    assert_eq!(e.code(), Some("S005"));
    let ErrorKind::Unsatisfiable { constraints, .. } = &*e.kind else {
        panic!("expected S005, got {e}");
    };
    assert_eq!(
        constraints
            .iter()
            .map(|b| b.dep.to_string())
            .collect::<Vec<_>>(),
        ["mesa{!x11}"]
    );
}

#[test]
fn an_option_constraint_holds_when_the_candidate_records_no_options() {
    let core = index(
        "core",
        vec![pkg("zlib 1.2"), pkg("app 1.0").depends(&["zlib{tool}"])],
    );
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1.0-r1", "zlib 1.2-r1"]);
}

#[test]
fn a_request_with_option_constraints_selects_a_matching_candidate() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vulkan", "all")]),
            pkg("mesa 25.2.0").options(&[("vulkan", "amd"), ("llvm", "true")]),
        ],
    );
    let u = universe(&[(&core, 0)]);
    let r = u
        .resolve(&input("system", vec![req("mesa{vulkan=amd}")], &[]))
        .unwrap();
    assert_eq!(labels(&r), ["mesa 25.2.0-r1"]);
    let r = u
        .resolve(&input("system", vec![req("mesa{llvm,!x11}")], &[]))
        .unwrap_err();
    assert_eq!(r.code(), Some("S005"));
    let ErrorKind::Unsatisfiable { constraints, .. } = &*r.kind else {
        panic!("expected S005, got {r}");
    };
    assert_eq!(
        constraints
            .iter()
            .map(|b| b.dep.to_string())
            .collect::<Vec<_>>(),
        ["mesa{llvm,!x11}"]
    );
}

#[test]
fn fixed_values_rule_out_candidates_that_record_other_values() {
    let without_x11 = pkg("mesa 25.1.0").options(&[("x11", "false")]);
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("x11", "true")]),
            without_x11.clone(),
        ],
    );
    let u = universe(&[(&core, 0)]);
    let i = input_with(
        "system",
        vec![req("mesa")],
        &[],
        &[fixed(
            "mesa",
            &[("x11", "false", "/etc/laft/system.stav:3:5")],
        )],
    );
    let r = u.resolve(&i).unwrap();
    assert_eq!(labels(&r), ["mesa 25.1.0-r1"]);

    let installed = pkg("mesa 25.2.0").options(&[("x11", "true")]);
    let i = input_with(
        "system",
        vec![req("mesa")],
        &[&installed],
        &[fixed(
            "mesa",
            &[("x11", "false", "/etc/laft/system.stav:3:5")],
        )],
    );
    let r = u.resolve(&i).unwrap();
    assert_eq!(labels(&r), ["mesa 25.1.0-r1"]);
    assert!(!r.packages[&name("mesa")].kept_installed);
}

#[test]
fn a_fixed_option_must_be_recorded_by_the_selected_package() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0"),
            pkg("mesa 25.1.0").options(&[("x11", "true")]),
        ],
    );
    let i = input_with(
        "system",
        vec![req("mesa")],
        &[],
        &[fixed(
            "mesa",
            &[("x11", "false", "/etc/laft/system.stav:3:5")],
        )],
    );
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S005"));
}

#[test]
fn a_dependency_contradicting_a_fixed_value_cannot_select() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vulkan", "all")]),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=all}"]),
        ],
    );
    let i = input_with(
        "system",
        vec![req("blender")],
        &[],
        &[fixed(
            "mesa",
            &[("vulkan", "amd", "/etc/laft/system.stav:14:5")],
        )],
    );
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S013"));
    assert_eq!(
        e.to_string(),
        "error[S013]: view \"system\": blender 4.5.0-r1 needs mesa{vulkan=all}\n  \
         demands vulkan=all, but vulkan=amd is set at /etc/laft/system.stav:14:5\n  \
         requested at test:blender\n  \
         hint: remove the setting, or set vulkan with default(...) to let dependencies choose"
    );
}

#[test]
fn s013_for_a_request_prints_its_origin_once() {
    let core = index(
        "core",
        vec![pkg("mesa 25.2.0").options(&[("vulkan", "all")])],
    );
    let i = input_with(
        "system",
        vec![req("mesa{vulkan=amd}")],
        &[],
        &[fixed(
            "mesa",
            &[("vulkan", "all", "/etc/laft/system.stav:5:1")],
        )],
    );
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S013"));
    assert_eq!(
        e.to_string(),
        "error[S013]: view \"system\": mesa{vulkan=amd} requested at test:mesa{vulkan=amd}\n  \
         demands vulkan=amd, but vulkan=all is set at /etc/laft/system.stav:5:1\n  \
         hint: remove the setting, or set vulkan with default(...) to let dependencies choose"
    );
}

#[test]
fn the_installed_extra_ranks_by_its_option_values() {
    let listed = index(
        "core",
        vec![pkg("mesa 25.2.0").options(&[("x11", "false")])],
    );
    let installed = pkg("mesa 25.2.0").options(&[("x11", "true")]);
    let i = input_with(
        "system",
        vec![req("mesa")],
        &[&installed],
        &[preferred("mesa", &[("x11", "false")])],
    );
    let mut i = i;
    i.upgrade = Upgrade::All;
    let r = universe(&[(&listed, 0)]).resolve(&i).unwrap();
    assert_eq!(options_of(&r, "mesa"), "x11=false");
}

#[test]
fn a_buildable_takes_the_preferred_values_of_the_view() {
    let core = index("core", vec![]);
    let mut i = input("system", vec![req("mesa")], &[]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[("vaapi", "intel")])];
    i.options.insert(
        name("mesa"),
        OptionRequest {
            fixed: BTreeMap::new(),
            preferred: BTreeMap::from([("vaapi".to_owned(), OptionValue::Tag("amd".into()))]),
        },
    );
    i.allow_builds = true;
    let r = universe(&[(&core, 0)]).resolve(&i).unwrap();
    assert_eq!(r.builds.len(), 1);
    assert_eq!(
        r.builds[0].values.get("vaapi"),
        Some(&OptionValue::Tag("amd".into()))
    );
}

/// A name the snapshot builds is never also selected: a provide
/// dependency follows the build instead of picking a published package
/// of the same name.
#[test]
fn a_built_name_is_not_also_selected_for_a_provide() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0")
                .options(&[("vulkan", "all")])
                .provides(&["so:libEGL.so.1"]),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=amd}"]),
            pkg("kwin 6.0").depends(&["so:libEGL.so.1"]),
        ],
    );
    let mut i = input("system", vec![req("blender"), req("kwin")], &[]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[("vulkan", "all")])];
    i.allow_builds = true;
    let r = universe(&[(&core, 0)]).resolve(&i).unwrap();
    assert!(
        !r.packages.contains_key(&name("mesa")),
        "mesa is built, so it is not selected: {:?}",
        r.packages
            .keys()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    assert_eq!(r.builds.len(), 1, "blender still needs its mesa");
    assert_eq!(
        r.builds[0].values.get("vulkan"),
        Some(&OptionValue::Tag("amd".into()))
    );
    assert!(r.builds[0].reasons.iter().any(|reason| *reason
        == Reason::Dependency {
            by: name("kwin"),
            dep: "so:libEGL.so.1".parse().unwrap(),
        }));
}

#[test]
fn s011_names_a_name_nobody_publishes_at_all() {
    let core = index("core", vec![pkg("blender 4.5.0").depends(&["mesa"])]);
    let mut i = input("system", vec![req("blender")], &[]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[])];
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S011"));
    assert_eq!(
        e.to_string(),
        "error[S011]: view \"system\": no repository publishes mesa with its default option values\n  \
         hint: set `build.missing = build` to build it on this machine,\n        \
         or run `laft apply --build-missing` once"
    );
}

#[test]
fn unsatisfiable_names_the_fixed_values_when_nothing_else_remains() {
    let core = index("core", vec![pkg("mesa 25.2.0").options(&[("x11", "true")])]);
    let i = input_with(
        "system",
        vec![req("mesa")],
        &[],
        &[fixed(
            "mesa",
            &[("x11", "false", "/etc/laft/system.stav:3:5")],
        )],
    );
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S005"));
    assert_eq!(
        e.to_string(),
        "error[S005]: view \"system\": no version of mesa satisfies every constraint\n  \
         mesa{!x11}    set at /etc/laft/system.stav:3:5\n  \
         available: mesa 25.2.0-r1"
    );
}

#[test]
fn two_requests_contradicting_each_other_report_s012() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vaapi", "intel")]),
            pkg("mesa 25.1.0").options(&[("vaapi", "amd")]),
        ],
    );
    let e = resolve_core(&core, &["mesa{vaapi=intel}", "mesa{vaapi=amd}"], &[]).unwrap_err();
    assert_eq!(e.code(), Some("S012"));
    assert_eq!(
        e.to_string(),
        "error[S012]: view \"system\": mesa cannot have vaapi=amd and vaapi=intel\n  \
         vaapi=amd requested at test:mesa{vaapi=amd}\n  \
         vaapi=intel requested at test:mesa{vaapi=intel}\n  \
         hint: put one of the dependents into another view, where it can have its own mesa"
    );
}

#[test]
fn a_learned_constraint_contradicting_the_request_reports_s012() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vaapi", "intel")]),
            pkg("mesa 25.1.0").options(&[("vaapi", "amd")]),
            pkg("game 1").depends(&["mesa{vaapi=amd}"]),
            pkg("app 1").depends(&["game"]),
        ],
    );
    let e = resolve_core(&core, &["mesa{vaapi=intel}", "app"], &[]).unwrap_err();
    assert_eq!(e.code(), Some("S012"));
    assert_eq!(
        e.to_string(),
        "error[S012]: view \"system\": mesa cannot have vaapi=intel and vaapi=amd\n  \
         vaapi=intel requested at test:mesa{vaapi=intel}\n  \
         vaapi=amd needed by game 1-r1 (mesa{vaapi=amd})\n  \
         hint: put one of the dependents into another view, where it can have its own mesa"
    );
}

/// A dependency moves a free option off the value the view prefers: a
/// preference only orders, so the constraint wins.
#[test]
fn a_dependency_moves_a_free_option_off_the_preferred_value() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vulkan", "amd")]),
            pkg("mesa 25.1.0").options(&[("vulkan", "all")]),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=all}"]),
        ],
    );
    let i = input_with(
        "system",
        vec![req("blender")],
        &[],
        &[preferred("mesa", &[("vulkan", "amd")])],
    );
    let r = universe(&[(&core, 0)]).resolve(&i).unwrap();
    assert_eq!(options_of(&r, "mesa"), "vulkan=all");
    assert_eq!(labels(&r), ["blender 4.5.0-r1", "mesa 25.1.0-r1"]);
    assert_eq!(
        r.packages[&name("mesa")].reasons,
        vec![
            Reason::Dependency {
                by: name("blender"),
                dep: "mesa{vulkan=all}".parse().expect("a dependency"),
            },
            Reason::OptionMoved {
                option: "vulkan".into(),
                chosen: "all".parse().expect("a value"),
                preferred: "amd".parse().expect("a value"),
                why: "needed by blender 4.5.0-r1 (mesa{vulkan=all})".into(),
            },
        ],
        "`laft why mesa` names blender and its constraint as the reason"
    );
}

/// Two dependents needing different values of one option: each view holds
/// one of them, and the two views keep different mesa builds.
#[test]
fn clashing_option_values_settle_across_two_views() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vaapi", "intel")]),
            pkg("mesa 25.1.0").options(&[("vaapi", "amd")]),
            pkg("blender 4.5.0").depends(&["mesa{vaapi=intel}"]),
            pkg("game 1.0").depends(&["mesa{vaapi=amd}"]),
        ],
    );
    let u = universe(&[(&core, 0)]);
    let e = u
        .resolve(&input("system", vec![req("blender"), req("game")], &[]))
        .unwrap_err();
    assert_eq!(e.code(), Some("S012"));
    let system = u
        .resolve(&input("system", vec![req("blender")], &[]))
        .unwrap();
    let game = u.resolve(&input("game", vec![req("game")], &[])).unwrap();
    assert_eq!(options_of(&system, "mesa"), "vaapi=intel");
    assert_eq!(options_of(&game, "mesa"), "vaapi=amd");
    assert_ne!(
        system.packages[&name("mesa")].candidate.id,
        game.packages[&name("mesa")].candidate.id
    );
}

#[test]
fn two_dependents_contradicting_each_other_report_s012() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vaapi", "intel")]),
            pkg("mesa 25.1.0").options(&[("vaapi", "amd")]),
            pkg("blender 4.5.0").depends(&["mesa{vaapi=intel}"]),
            pkg("game 1.0").depends(&["mesa{vaapi=amd}"]),
        ],
    );
    let i = input("system", vec![req("blender"), req("game")], &[]);
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(e.code(), Some("S012"));
    assert_eq!(
        e.to_string(),
        "error[S012]: view \"system\": mesa cannot have vaapi=intel and vaapi=amd\n  \
         vaapi=intel needed by blender 4.5.0-r1 (mesa{vaapi=intel})\n    \
         requested at test:blender\n  \
         vaapi=amd needed by game 1.0-r1 (mesa{vaapi=amd})\n    \
         requested at test:game\n  \
         hint: put one of the dependents into another view, where it can have its own mesa"
    );
}

#[test]
fn an_unpublished_fixed_value_reports_s011() {
    let core = index("core", vec![pkg("mesa 25.2.0").options(&[("x11", "true")])]);
    let mut i = input_with(
        "system",
        vec![req("mesa")],
        &[],
        &[fixed(
            "mesa",
            &[("x11", "false", "/etc/laft/system.stav:12:3")],
        )],
    );
    i.buildable = vec![buildable("mesa", "25.2.0", &[("x11", "true")])];
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(
        e.to_string(),
        "error[S011]: view \"system\": no repository publishes mesa with x11=false\n  \
         x11=false set at /etc/laft/system.stav:12:3\n  \
         published: mesa 25.2.0-r1 (defaults)\n  \
         hint: set `build.missing = build` to build it on this machine,\n        \
         or run `laft apply --build-missing` once"
    );
}

#[test]
fn s011_names_a_dependency_that_demands_the_values() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vulkan", "all")]),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=amd}"]),
        ],
    );
    let mut i = input("system", vec![req("blender")], &[]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[("vulkan", "all")])];
    let e = universe(&[(&core, 0)]).resolve(&i).unwrap_err();
    assert_eq!(
        e.to_string(),
        "error[S011]: view \"system\": no repository publishes mesa with vulkan=amd\n  \
         vulkan=amd needed by blender 4.5.0-r1 (mesa{vulkan=amd})\n  \
         published: mesa 25.2.0-r1 (defaults)\n  \
         hint: set `build.missing = build` to build it on this machine,\n        \
         or run `laft apply --build-missing` once"
    );
}

#[test]
fn allowing_builds_returns_a_build_request() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vulkan", "all")]),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=amd}"]),
        ],
    );
    let mut i = input("system", vec![req("blender")], &[]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[("vulkan", "all")])];
    i.allow_builds = true;
    let r = universe(&[(&core, 0)]).resolve(&i).unwrap();
    assert_eq!(labels(&r), ["blender 4.5.0-r1"]);
    assert_eq!(r.builds.len(), 1);
    let build = &r.builds[0];
    assert_eq!(build.buildable.name, name("mesa"));
    assert_eq!(build.buildable.version.to_string(), "25.3.0");
    assert_eq!(build.values.get("vulkan").unwrap().to_string(), "amd");
    assert_eq!(
        build.reasons,
        [Reason::Dependency {
            by: name("blender"),
            dep: "mesa{vulkan=amd}".parse().unwrap(),
        }]
    );
}

#[test]
fn a_newer_buildable_beats_an_older_published_option_set() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vulkan", "amd")]),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=amd}"]),
        ],
    );
    let mut i = input("system", vec![req("blender")], &[]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[("vulkan", "all")])];
    i.allow_builds = true;
    let r = universe(&[(&core, 0)]).resolve(&i).unwrap();
    assert!(
        r.builds[0]
            .buildable
            .version
            .to_string()
            .starts_with("25.3")
    );
}

#[test]
fn published_beats_a_buildable_at_the_same_version() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.3.0").options(&[("vulkan", "amd")]),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=amd}"]),
        ],
    );
    let mut i = input("system", vec![req("blender")], &[]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[("vulkan", "all")])];
    i.allow_builds = true;
    let r = universe(&[(&core, 0)]).resolve(&i).unwrap();
    assert!(r.builds.is_empty());
    assert_eq!(labels(&r), ["blender 4.5.0-r1", "mesa 25.3.0-r1"]);
}

#[test]
fn the_installed_package_beats_a_newer_buildable() {
    let installed = pkg("mesa 25.1.0").options(&[("vulkan", "amd")]);
    let core = index(
        "core",
        vec![
            installed.clone(),
            pkg("blender 4.5.0").depends(&["mesa{vulkan=amd}"]),
        ],
    );
    let mut i = input("system", vec![req("blender")], &[&installed]);
    i.buildable = vec![buildable("mesa", "25.3.0", &[("vulkan", "all")])];
    i.allow_builds = true;
    let r = universe(&[(&core, 0)]).resolve(&i).unwrap();
    assert!(r.builds.is_empty());
    assert_eq!(labels(&r), ["blender 4.5.0-r1", "mesa 25.1.0-r1"]);
    assert!(r.packages[&name("mesa")].kept_installed);
}

#[test]
fn a_name_nobody_publishes_is_built_with_its_defaults() {
    let core = index("core", vec![]);
    let mut i = input("system", vec![req("mesa")], &[]);
    i.buildable = vec![buildable(
        "mesa",
        "25.3.0",
        &[("llvm", "true"), ("x11", "true")],
    )];
    i.allow_builds = true;
    let r = universe(&[(&core, 0)]).resolve(&i).unwrap();
    assert!(r.packages.is_empty());
    assert_eq!(r.builds.len(), 1);
    assert_eq!(r.builds[0].values.get("llvm").unwrap().to_string(), "true");
    assert_eq!(
        r.builds[0].reasons,
        [Reason::Requested {
            origin: "test:mesa".into(),
            text: "mesa".into(),
        }]
    );
}

#[test]
fn the_fewest_deviations_from_the_preferred_values_win() {
    let core = index(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("x11", "true"), ("llvm", "true")]),
            pkg("mesa 25.2.0").options(&[("x11", "false"), ("llvm", "true")]),
        ],
    );
    let u = universe(&[(&core, 0)]);
    let i = input("system", vec![req("mesa")], &[]);
    let r = u.resolve(&i).unwrap();
    assert_eq!(options_of(&r, "mesa"), "llvm=true, x11=false");

    let i = input_with(
        "system",
        vec![req("mesa")],
        &[],
        &[preferred("mesa", &[("x11", "false")])],
    );
    let r = u.resolve(&i).unwrap();
    assert_eq!(options_of(&r, "mesa"), "llvm=true, x11=false");
}

#[test]
fn declared_defaults_decide_when_the_view_prefers_nothing() {
    let core = index_declaring(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("x11", "true")]),
            pkg("mesa 25.2.0").options(&[("x11", "false")]),
        ],
        vec![("mesa", vec![decl_bool("x11", "true")])],
    );
    let r = resolve_core(&core, &["mesa"], &[]).unwrap();
    assert_eq!(options_of(&r, "mesa"), "x11=true");
}

#[test]
fn false_comes_before_true_in_the_tie_break() {
    let core = index_declaring(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vulkan", "amd"), ("llvm", "false")]),
            pkg("mesa 25.2.0").options(&[("vulkan", "all"), ("llvm", "true")]),
        ],
        vec![(
            "mesa",
            vec![
                decl_bool("llvm", "true"),
                decl_tags("vulkan", &["amd", "all"], "all"),
            ],
        )],
    );
    let mut i = input("system", vec![req("mesa")], &[]);
    // Both candidates deviate once, so the tie break compares llvm first,
    // where false comes before true.
    i.options = [preferred("mesa", &[("vulkan", "amd"), ("llvm", "true")])]
        .into_iter()
        .collect();
    let r = universe(&[(&core, 0)]).resolve(&i).unwrap();
    assert_eq!(options_of(&r, "mesa"), "llvm=false, vulkan=amd");
}

#[test]
fn tags_come_in_declaration_order_in_the_tie_break() {
    let core = index_declaring(
        "core",
        vec![
            pkg("mesa 25.2.0").options(&[("vulkan", "intel")]),
            pkg("mesa 25.2.0").options(&[("vulkan", "amd")]),
        ],
        vec![(
            "mesa",
            vec![decl_tags("vulkan", &["amd", "intel", "all"], "all")],
        )],
    );
    // Both deviate once from the default, so the declaration order of the
    // tags decides.
    let r = resolve_core(&core, &["mesa"], &[]).unwrap();
    assert_eq!(options_of(&r, "mesa"), "vulkan=amd");
}

/// The option values the resolution records for `of`, as `key=value`
/// texts joined key sorted.
fn options_of(r: &Resolution<'_>, of: &str) -> String {
    r.packages[&name(of)]
        .candidate
        .meta
        .options
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
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
            Ok(r) => check_sound(&r, &planted.requests, &BTreeMap::new()),
            Err(e) => prop_assert!(false, "{e}"),
        }
    }
}
