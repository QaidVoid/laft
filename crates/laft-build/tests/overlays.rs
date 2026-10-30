//! Overlays merge into the recipes of the repositories they extend.

mod support;

use laft_build::{Error, Fetch, Repos};

fn overlay(dir: &std::path::Path, recipe: &str) -> std::path::PathBuf {
    support::write_repo(
        &dir.join("personal"),
        "personal",
        "  extends = [\"core\"]\n",
        &[
            ("recipes/ripgrep/package.stav", recipe),
            (
                "recipes/ripgrep/patches/0001-my-tweak.patch",
                "--- a\n+++ b\n",
            ),
        ],
    )
}

#[test]
fn an_overlay_changes_the_version_and_every_url_built_from_it() {
    let dir = laft_test::tempdir();
    let personal = overlay(
        dir.path(),
        "package ripgrep {\n  version = force(\"14.2.0\")\n  rev     = force(1)\n  \
         source main { sha256 = force(\"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\") }\n  \
         vendor cargo { sha256 = force(\"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\") }\n  \
         patches = [./patches/0001-my-tweak.patch]\n}\n",
    );
    let repos = Repos::open(&[support::core(), personal]).expect("the repositories open");
    let recipes = support::evaluate(&repos, "ripgrep");
    assert_eq!(recipes.len(), 2);
    for recipe in &recipes {
        assert_eq!(recipe.version.as_str(), "14.2.0");
        let Fetch::Url { urls, sha256, .. } = &recipe.sources[0].fetch;
        assert_eq!(
            urls,
            &["https://github.com/BurntSushi/ripgrep/archive/14.2.0/ripgrep-14.2.0.tar.gz"]
        );
        assert!(sha256.to_string().ends_with("aaaa"));
        assert!(recipe.vendors[0].sha256.to_string().ends_with("bbbb"));
        let repos: Vec<&str> = recipe.repos.iter().map(|r| r.as_str()).collect();
        assert_eq!(repos, ["core", "personal"]);
        let patches: Vec<&str> = recipe.patches.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(
            patches,
            ["/build/recipes/personal/ripgrep/patches/0001-my-tweak.patch"]
        );
        assert_eq!(recipe.env["RECIPE"], "/build/recipes/core/ripgrep");
        assert_eq!(recipe.env["LAFT_VERSION"], "14.2.0");
    }
}

#[test]
fn an_overlay_must_force_the_scalars_it_overrides() {
    let dir = laft_test::tempdir();
    let personal = overlay(dir.path(), "package ripgrep {\n  version = \"14.2.0\"\n}\n");
    let repos = Repos::open(&[support::core(), personal]).expect("the repositories open");
    let Err(Error::Report(report)) = repos.evaluate(&support::name("ripgrep")) else {
        panic!("conflicting versions do not evaluate");
    };
    let error = report.iter().next().expect("one error");
    assert_eq!(error.diagnostic.code, "E0501", "{report}");
    assert!(error.diagnostic.message.contains("version"), "{report}");
}

#[test]
fn overlay_patches_follow_the_patches_of_the_recipe() {
    let dir = laft_test::tempdir();
    let body = |patch: &str| {
        support::recipe(
            "foo",
            &format!(
                "{}  build custom {{}}\n  patches = [./patches/{patch}]\n",
                support::SOURCE
            ),
        )
    };
    let base = support::write_repo(
        &dir.path().join("base"),
        "base",
        "",
        &[
            ("recipes/foo/package.stav", &body("0001-base.patch")),
            ("recipes/foo/patches/0001-base.patch", "base\n"),
        ],
    );
    let top = support::write_repo(
        &dir.path().join("top"),
        "top",
        "  extends = [\"base\"]\n  toolchain_epoch = 2\n",
        &[
            (
                "recipes/foo/package.stav",
                "package foo {\n  patches = [./patches/0002-top.patch]\n}\n",
            ),
            ("recipes/foo/patches/0002-top.patch", "top\n"),
            (
                "recipes/bar/package.stav",
                &support::recipe("bar", "  empty = true\n  build custom {}\n"),
            ),
        ],
    );
    let repos = Repos::open(&[base, top]).expect("the repositories open");
    let names: Vec<String> = repos.packages().map(ToString::to_string).collect();
    assert_eq!(names, ["bar", "foo"], "an overlay may add recipes");
    let foo = &support::evaluate(&repos, "foo")[0];
    let patches: Vec<&str> = foo.patches.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(
        patches,
        [
            "/build/recipes/base/foo/patches/0001-base.patch",
            "/build/recipes/top/foo/patches/0002-top.patch",
        ]
    );
    assert_eq!(foo.patches[1].blake3, laft_core::Blake3Hash::of(b"top\n"));
    assert_eq!(foo.toolchain_epoch, 2);
    let host = repos.host_path(patches[1]).expect("a recipe path");
    assert_eq!(std::fs::read(host).expect("readable"), b"top\n");
    assert_eq!(repos.host_path("/build/recipes/top/foo/../../x"), None);
}

#[test]
fn an_overlay_extends_only_repositories_listed_before_it() {
    let dir = laft_test::tempdir();
    let personal = overlay(dir.path(), "package ripgrep {}\n");
    let Err(Error::Problems(problems)) = Repos::open(&[personal, support::core()]) else {
        panic!("the order is wrong");
    };
    assert_eq!(problems[0].code, "R004");
    assert_eq!(
        problems[0].message,
        "recipe repository personal extends core, which is not listed before it"
    );
}

#[test]
fn every_contributing_repository_and_the_recipe_restrict_the_architecture() {
    let dir = laft_test::tempdir();
    let body = |arch: &str| {
        support::recipe(
            "foo",
            &format!("{}  build custom {{}}\n{arch}", support::SOURCE),
        )
    };
    let cases = std::cell::Cell::new(0);
    let count = |core_arch: &str, overlay_arch: &str, recipe_arch: &str| {
        cases.set(cases.get() + 1);
        let core = support::write_repo(
            &dir.path().join(format!("case{}/core", cases.get())),
            "core",
            core_arch,
            &[("recipes/foo/package.stav", &body(recipe_arch))],
        );
        let overlay = support::write_repo(
            &dir.path().join(format!("case{}/personal", cases.get())),
            "personal",
            &format!("  extends = [\"core\"]\n{overlay_arch}"),
            &[("recipes/foo/package.stav", "package foo {}\n")],
        );
        let repos = Repos::open(&[core, overlay]).expect("the repositories open");
        support::evaluate(&repos, "foo").len()
    };
    let foreign = "  arch = [\"aarch64\"]\n";
    assert_eq!(count("", "", ""), 1);
    assert_eq!(count(foreign, "", ""), 0);
    assert_eq!(count("", foreign, ""), 0);
    assert_eq!(count("", "", foreign), 0);
    assert_eq!(count("", "", "  arch = [\"x86_64-v3\"]\n"), 1);
}
