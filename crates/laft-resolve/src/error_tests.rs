use laft_index::Index;

use crate::test_support::*;
use crate::{Error, Request, Upgrade};

const SYSTEM: &str = "/etc/laft/system.stav";

fn at(line: u32, col: u32) -> String {
    format!("{SYSTEM}:{line}:{col}")
}

fn message(repos: &[(&Index, i64)], view: &str, requests: Vec<Request>) -> String {
    error(repos, view, requests).to_string()
}

fn error(repos: &[(&Index, i64)], view: &str, requests: Vec<Request>) -> Error {
    universe(repos)
        .resolve(&input(view, requests, &[]))
        .expect_err("the resolution should fail")
}

#[test]
fn r001_lists_the_available_versions() {
    let core = index("core", vec![pkg("llvm 20.1.2"), pkg("llvm 19.1.7-r2")]);
    assert_eq!(
        message(
            &[(&core, 0)],
            "system",
            vec![req_at("llvm@18", &at(12, 15))]
        ),
        "error[R001]: view \"system\": no package matches llvm@18\n  \
         requested at /etc/laft/system.stav:12:15\n  \
         available: llvm 20.1.2-r1, llvm 19.1.7-r2"
    );
}

#[test]
fn r002_lists_the_conflicting_requests() {
    let core = index("core", vec![pkg("ripgrep 14 static")]);
    let requests = vec![
        req_at("ripgrep:static", "/etc/laft/cli.stav:3:16"),
        req_at("ripgrep", &at(8, 22)),
    ];
    assert_eq!(
        message(&[(&core, 0)], "system", requests),
        "error[R002]: view \"system\": conflicting requests for ripgrep\n  \
         ripgrep          at /etc/laft/system.stav:8:22\n  \
         ripgrep:static   at /etc/laft/cli.stav:3:16\n  \
         hint: request one variant"
    );
}

#[test]
fn r003_shows_the_chain_back_to_the_request() {
    let core = index(
        "core",
        vec![
            pkg("libxml2 2.14.2")
                .depends(&["so:libicuuc.so.76"])
                .provides(&["so:libxml2.so.2"]),
            pkg("gtk3 3.24.49")
                .depends(&["so:libxml2.so.2"])
                .provides(&["so:libgtk-3.so.0"]),
            pkg("firefox 144.0").depends(&["so:libgtk-3.so.0"]),
        ],
    );
    assert_eq!(
        message(&[(&core, 0)], "system", vec![req_at("firefox", &at(9, 5))]),
        "error[R003]: view \"system\": nothing provides so:libicuuc.so.76\n  \
         needed by libxml2 2.14.2-r1\n    \
         needed by gtk3 3.24.49-r1 (so:libxml2.so.2)\n      \
         needed by firefox 144.0-r1 (so:libgtk-3.so.0)\n        \
         requested at /etc/laft/system.stav:9:5\n  \
         hint: the repository is missing a package or is mid-update; run `laft repo sync` later"
    );
}

#[test]
fn r004_lists_the_providers() {
    let core = index(
        "core",
        vec![
            pkg("libglvnd 1.7").provides(&["so:libGL.so.1"]),
            pkg("mesa-amber 21.3").provides(&["so:libGL.so.1"]),
            pkg("mpv 0.40.0").depends(&["so:libGL.so.1"]),
        ],
    );
    assert_eq!(
        message(&[(&core, 0)], "system", vec![req_at("mpv", &at(4, 5))]),
        "error[R004]: view \"system\": several packages provide so:libGL.so.1: libglvnd, mesa-amber\n  \
         needed by mpv 0.40.0-r1\n    \
         requested at /etc/laft/system.stav:4:5\n  \
         hint: request one of them in packages"
    );
}

#[test]
fn r005_lists_every_constraint_and_its_source() {
    let core = index(
        "core",
        vec![
            pkg("foo 2.1.0"),
            pkg("foo 1.9.3-r2"),
            pkg("baz 1.4").depends(&["foo<2"]),
        ],
    );
    assert_eq!(
        message(
            &[(&core, 0)],
            "system",
            vec![req_at("foo@2.1", &at(20, 5)), req_at("baz", &at(21, 5))]
        ),
        "error[R005]: view \"system\": no version of foo satisfies every constraint\n  \
         foo<2         from baz 1.4-r1\n  \
         foo@2.1       requested at /etc/laft/system.stav:20:5\n  \
         available: foo 2.1.0-r1, foo 1.9.3-r2"
    );
}

#[test]
fn r005_lists_learned_constraints() {
    let core = index(
        "core",
        vec![
            pkg("foo 1.9"),
            pkg("foo 2.1"),
            pkg("app 1").depends(&["bar", "baz"]),
            pkg("bar 3.0").depends(&["foo>=2"]),
            pkg("baz 1.4").depends(&["foo<2"]),
        ],
    );
    assert_eq!(
        message(&[(&core, 0)], "system", vec![req_at("app", &at(20, 5))]),
        "error[R005]: view \"system\": no version of foo satisfies every constraint\n  \
         foo<2         from baz 1.4-r1\n  \
         foo>=2        from bar 3.0-r1\n  \
         available: foo 2.1-r1, foo 1.9-r1"
    );
}

#[test]
fn r006_shows_how_each_package_got_in() {
    let core = index(
        "core",
        vec![
            pkg("openssl 3.5.0")
                .provides(&["so:libssl.so.3"])
                .conflicts(&["libressl"]),
            pkg("libressl 4.1.0"),
            pkg("curl 8.16.0").depends(&["so:libssl.so.3"]),
        ],
    );
    assert_eq!(
        message(
            &[(&core, 0)],
            "system",
            vec![req_at("curl", &at(7, 5)), req_at("libressl", &at(11, 5))]
        ),
        "error[R006]: view \"system\": openssl 3.5.0-r1 conflicts with libressl 4.1.0-r1\n  \
         openssl  needed by curl 8.16.0-r1 (so:libssl.so.3), requested at /etc/laft/system.stav:7:5\n  \
         libressl requested at /etc/laft/system.stav:11:5"
    );
}

#[test]
fn r006_uses_replaces_wording() {
    let core = index("core", vec![pkg("bar 2").replaces(&["foo"]), pkg("foo 1")]);
    assert_eq!(
        message(
            &[(&core, 0)],
            "system",
            vec![req_at("bar", &at(1, 5)), req_at("foo", &at(2, 5))]
        ),
        "error[R006]: view \"system\": bar 2-r1 replaces foo 1-r1\n  \
         bar requested at /etc/laft/system.stav:1:5\n  \
         foo requested at /etc/laft/system.stav:2:5\n  \
         hint: bar takes the place of foo, so remove foo from this view"
    );
}

#[test]
fn r007_names_the_variant_that_would_provide() {
    let core = index(
        "core",
        vec![
            pkg("zlib 1.3.1").provides(&["so:libz.so.1"]),
            pkg("zlib 1.3.1 static"),
            pkg("python 3.12.11").depends(&["so:libz.so.1"]),
        ],
    );
    assert_eq!(
        message(
            &[(&core, 0)],
            "legacy",
            vec![
                req_at("zlib:static", &at(31, 20)),
                req_at("python", &at(32, 5))
            ]
        ),
        "error[R007]: view \"legacy\": so:libz.so.1 is needed by python 3.12.11-r1\n  \
         zlib provides it in the dynamic variant, but zlib:static is selected\n    \
         requested at /etc/laft/system.stav:31:20\n  \
         hint: static library variants do not provide shared libraries; remove zlib:static from this view"
    );
}

#[test]
fn r007_gives_the_static_hint_only_when_the_static_variant_is_selected() {
    let core = index(
        "core",
        vec![
            pkg("foo 1"),
            pkg("foo 1 static").provides(&["so:libx.so.1"]),
            pkg("app 1").depends(&["so:libx.so.1"]),
        ],
    );
    assert_eq!(
        message(
            &[(&core, 0)],
            "system",
            vec![req_at("foo", &at(3, 5)), req_at("app", &at(4, 5))]
        ),
        "error[R007]: view \"system\": so:libx.so.1 is needed by app 1-r1\n  \
         foo provides it in the static variant, but foo is selected\n    \
         requested at /etc/laft/system.stav:3:5\n  \
         hint: request foo in a version or variant that provides so:libx.so.1"
    );
}

#[test]
fn r008_names_the_repositories() {
    let core = index("core", vec![pkg("mesa 25.2.5")]);
    let mine = index("mine", vec![pkg("mesa 25.1.0")]);
    assert_eq!(
        message(&[(&mine, 0), (&core, 0)], "system", vec![req("mesa")]),
        "error[R008]: view \"system\": package mesa is in repositories core and mine, both with priority 0\n  \
         hint: give one repository a higher priority"
    );
}

#[test]
fn r010_gives_the_yank_reason() {
    let yanked = pkg("ripgrep 14.1.1");
    let mut core = index("core", vec![pkg("ripgrep 14.0.3")]);
    core.yanked
        .push(yank(&yanked, "crashes on startup with PCRE2 10.46"));
    let u = universe(&[(&core, 0)]);
    let mut i = input(
        "system",
        vec![req_at("ripgrep@14.1.1", &at(8, 22))],
        &[&yanked],
    );
    i.upgrade = Upgrade::None;
    assert_eq!(
        u.resolve(&i).unwrap_err().to_string(),
        "error[R010]: view \"system\": ripgrep 14.1.1-r1 was yanked and no other version matches ripgrep@14.1.1\n  \
         yank reason: crashes on startup with PCRE2 10.46 (2026-10-27)\n  \
         requested at /etc/laft/system.stav:8:22\n  \
         hint: relax the version prefix, or wait for a fixed revision"
    );
}

#[test]
fn initrd_errors_name_the_initrd() {
    let core = index(
        "core",
        vec![pkg("cryptsetup 2.8 static").depends(&["so:libc.so.6"])],
    );
    assert_eq!(
        message(
            &[(&core, 0)],
            "initrd",
            vec![req_at("cryptsetup:static", "boot.initrd.luks")]
        ),
        "error[R003]: view \"initrd\": nothing provides so:libc.so.6\n  \
         needed by cryptsetup:static 2.8-r1\n    \
         requested at boot.initrd.luks\n  \
         hint: the repository is missing a package or is mid-update; run `laft repo sync` later"
    );
}

#[test]
fn alternatives_errors_name_the_group_and_members() {
    let core = index(
        "core",
        vec![
            pkg("neovim 0.11").alternative("vi", &["usr/bin/vi"]),
            pkg("vim 9.1").alternative("vi", &["usr/bin/vi"]),
            pkg("gcc 15").alternative("cc", &["usr/bin/cc"]),
        ],
    );
    assert_eq!(
        message(&[(&core, 0)], "system", vec![req("neovim"), req("vim")]),
        "error: alternatives group \"vi\" has several members in view \"system\": neovim, vim\n  \
         hint: choose one with `alternatives.vi = neovim`"
    );
    let u = universe(&[(&core, 0)]);
    let mut i = input("system", vec![req("gcc")], &[]);
    i.alternatives.insert(name("cc"), name("clang"));
    assert_eq!(
        u.resolve(&i).unwrap_err().to_string(),
        "error: alternatives.cc = clang, but clang is not in view \"system\"\n  \
         members present: gcc"
    );
}
