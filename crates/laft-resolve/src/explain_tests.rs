use crate::test_support::*;

fn system() -> laft_index::Index {
    index(
        "core",
        vec![
            pkg("libpcre2 10.45").provides(&["so:libpcre2-8.so.0"]),
            pkg("ripgrep 14.1.1").depends(&["so:libpcre2-8.so.0"]),
            pkg("git 2.51.0").depends(&["so:libpcre2-8.so.0"]),
            pkg("grep 3.12").depends(&["so:libpcre2-8.so.0"]),
            pkg("base 1").depends(&["git", "grep"]),
        ],
    )
}

#[test]
fn why_prints_the_shortest_chain_and_other_dependents() {
    let core = system();
    let u = universe(&[(&core, 0)]);
    let r = u
        .resolve(&input(
            "system",
            vec![
                req_at("ripgrep", "/etc/laft/system.stav:8:22"),
                req_at("base", "/etc/laft/system.stav:7:5"),
            ],
            &[],
        ))
        .unwrap();
    assert_eq!(
        r.why(&name("libpcre2")).unwrap().to_string(),
        "libpcre2 10.45-r1 is in view \"system\" because:\n  \
         ripgrep 14.1.1-r1 needs so:libpcre2-8.so.0\n    \
         requested at /etc/laft/system.stav:8:22 (ripgrep)\n\
         also needed by: git 2.51.0-r1, grep 3.12-r1\n"
    );
}

#[test]
fn why_follows_dependencies_through_several_packages() {
    let core = system();
    let u = universe(&[(&core, 0)]);
    let r = u
        .resolve(&input("system", vec![req_at("base", "cli")], &[]))
        .unwrap();
    assert_eq!(
        r.why(&name("libpcre2")).unwrap().to_string(),
        "libpcre2 10.45-r1 is in view \"system\" because:\n  \
         git 2.51.0-r1 needs so:libpcre2-8.so.0\n    \
         base 1-r1 needs git\n      \
         requested at cli (base)\n\
         also needed by: grep 3.12-r1\n"
    );
    assert_eq!(
        r.why(&name("base")).unwrap().to_string(),
        "base 1-r1 is in view \"system\" because:\n  requested at cli (base)\n"
    );
    assert!(r.why(&name("ripgrep")).is_none());
}

#[test]
fn learned_replacement_is_shown_as_replacing_the_name() {
    let core = index(
        "core",
        vec![
            pkg("bar 1").replaces(&["foo"]),
            pkg("bar 2"),
            pkg("x 1").depends(&["foo"]),
            pkg("app 1").depends(&["bar", "x"]),
        ],
    );
    let r = resolve_core(&core, &["app"], &[]).unwrap();
    assert_eq!(labels(&r), ["app 1-r1", "bar 1-r1", "x 1-r1"]);
    assert_eq!(
        r.learned
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["learned bar replaces foo from x 1-r1"]
    );
}
