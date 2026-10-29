//! Every example recipe evaluates for each variant it declares and renders
//! the documented phase scripts.

mod support;

use laft_build::{Recipe, Repos};
use laft_core::Variant;

fn scripts(recipe: &Recipe) -> String {
    let mut out = String::new();
    for (phase, script) in recipe.phases.iter() {
        if let Some(script) = script {
            out.push_str(&format!("### {phase}\n{script}"));
        }
    }
    out
}

fn repos() -> Repos {
    Repos::open(&[support::core()]).expect("the example repository opens")
}

#[test]
fn every_example_renders_its_phase_scripts() {
    let repos = repos();
    let names: Vec<String> = repos.packages().map(ToString::to_string).collect();
    assert_eq!(
        names,
        [
            "base",
            "fontconfig",
            "gcc",
            "linux",
            "openssl",
            "pixman",
            "ripgrep",
            "zlib"
        ]
    );
    for name in &names {
        for recipe in support::evaluate(&repos, name) {
            let variant = recipe.variant;
            insta::assert_snapshot!(format!("{name}-{variant}"), scripts(&recipe));
        }
    }
}

#[test]
fn zlib_plans_match_for_both_variants() {
    let recipes = support::evaluate(&repos(), "zlib");
    let variants: Vec<Variant> = recipes.iter().map(|r| r.variant).collect();
    assert_eq!(variants, [Variant::Dynamic, Variant::Static]);
    let static_plan = &recipes[1];
    assert_eq!(
        static_plan.env["DESTDIR"],
        "/out/usr/x86_64-laft-linux-musl"
    );
    assert!(static_plan.link_deps.is_empty());
    for recipe in &recipes {
        let json = serde_json::to_string_pretty(recipe).expect("serializable");
        insta::assert_snapshot!(format!("zlib-{}-plan", recipe.variant), json);
    }
}

#[test]
fn styles_adjust_the_plan_beyond_their_scripts() {
    let repos = repos();
    let linux = &support::evaluate(&repos, "linux")[0];
    assert!(!linux.options.check && !linux.options.debug_split);
    assert_eq!(linux.files.len(), 1);
    assert_eq!(
        linux.files[0].path,
        "/build/recipes/core/linux/files/config-x86_64"
    );
    assert_eq!(
        linux.files[0].blake3,
        laft_core::Blake3Hash::of(b"CONFIG_MODULES=y\n")
    );
    let gcc = &support::evaluate(&repos, "gcc")[0];
    let split: Vec<&str> = gcc.outputs.split.iter().map(|s| s.as_str()).collect();
    assert_eq!(split, ["dbg", "dev", "doc", "locale"]);
    assert_eq!(gcc.meta.license, ["GCC-exception-3.1", "GPL-3.0-or-later"]);
    let ripgrep = support::evaluate(&repos, "ripgrep");
    let links: Vec<String> = ripgrep.iter().map(|r| r.link_deps[0].to_string()).collect();
    assert_eq!(links, ["pcre2", "pcre2:static"]);
    assert_eq!(ripgrep[1].outputs.split, [laft_build::Split::Dbg]);
    let openssl = &support::evaluate(&repos, "openssl")[0];
    assert_eq!(
        openssl.outputs.subpackages["libs"].summary,
        "OpenSSL shared libraries and default configuration"
    );
    assert_eq!(openssl.outputs.subpackages["libs"].license, ["Apache-2.0"]);
}

#[test]
fn the_python_style_brings_its_tools() {
    let dir = laft_test::tempdir();
    let body = format!(
        "{}  build python-pep517 {{}}\n  deps = [foo]",
        support::SOURCE
    );
    let repo = support::write_repo(
        &dir.path().join("core"),
        "core",
        "",
        &[(
            "recipes/meson/package.stav",
            &support::recipe("meson", &body),
        )],
    );
    let repos = Repos::open(&[repo]).expect("the repository opens");
    let meson = &support::evaluate(&repos, "meson")[0];
    let names =
        |deps: &[laft_core::PkgRef]| deps.iter().map(ToString::to_string).collect::<Vec<_>>();
    assert_eq!(
        names(&meson.build_deps),
        ["python", "python-build", "python-installer", "python-wheel"]
    );
    assert_eq!(names(&meson.outputs.deps), ["foo", "python"]);
}
