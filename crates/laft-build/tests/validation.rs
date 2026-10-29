//! Recipes that break a rule are reported with the rule's code and the
//! source the problem is about.

mod support;

use laft_build::{Error, Repos};
use support::{SOURCE, recipe, write_repo};

/// Evaluates the package `name` of a repository holding `files`, and
/// returns the codes and messages of the errors.
fn errors(name: &str, files: &[(&str, &str)]) -> Vec<(String, String)> {
    let dir = laft_test::tempdir();
    let repo = write_repo(&dir.path().join("core"), "core", "", files);
    let repos = Repos::open(&[repo]).expect("the repository opens");
    match repos.evaluate(&support::name(name)) {
        Ok(_) => Vec::new(),
        Err(Error::Report(report)) => report
            .iter()
            .map(|d| (d.diagnostic.code.to_owned(), d.diagnostic.message.clone()))
            .collect(),
        Err(Error::Problems(problems)) => problems
            .into_iter()
            .map(|p| (p.code.to_owned(), p.message))
            .collect(),
    }
}

fn codes(name: &str, body: &str) -> Vec<String> {
    let path = format!("recipes/{name}/package.stav");
    errors(name, &[(&path, &recipe(name, body))])
        .into_iter()
        .map(|(code, _)| code)
        .collect()
}

#[test]
fn a_recipe_declares_the_package_its_directory_names() {
    let errors = errors(
        "zlib",
        &[(
            "recipes/zlib/package.stav",
            &recipe("libz", &format!("{SOURCE}  build custom {{}}")),
        )],
    );
    assert_eq!(
        errors,
        [
            (
                "R001".to_owned(),
                "recipes/zlib/package.stav declares package \"libz\", expected \"zlib\"".to_owned()
            ),
            (
                "R001".to_owned(),
                "recipes/zlib/package.stav declares no package \"zlib\"".to_owned()
            ),
        ]
    );
}

#[test]
fn path_literals_stay_in_the_recipe_directory_and_common() {
    let build = format!("{SOURCE}  build custom {{}}");
    let with_patch = |patch: &str| format!("{build}\n  patches = [{patch}]");
    let files = |patch: &str| {
        vec![
            (
                "recipes/foo/package.stav".to_owned(),
                recipe("foo", &with_patch(patch)),
            ),
            ("recipes/bar/patches/x.patch".to_owned(), "x\n".to_owned()),
            ("common/patches/y.patch".to_owned(), "y\n".to_owned()),
        ]
    };
    let run = |patch: &str| {
        let files = files(patch);
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        errors("foo", &files)
    };
    let escaping = run("../../../../etc/passwd");
    assert_eq!(
        escaping,
        [(
            "R002".to_owned(),
            "the path `../../../../etc/passwd` leaves the recipe repository".to_owned()
        )]
    );
    let other = run("../bar/patches/x.patch");
    assert_eq!(other[0].0, "R002");
    assert!(
        other[0]
            .1
            .ends_with("is outside recipes/foo/ and common/ of the recipe repository")
    );
    assert_eq!(run("../../common/patches/y.patch"), []);
    assert_eq!(run("./patches/missing.patch")[0].0, "R006");
}

#[test]
fn sources_need_https_a_hash_and_exactly_one_origin() {
    let hash =
        "sha256 = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"";
    let source = |fields: &str| format!("  source main {{\n{fields}\n  }}\n  build custom {{}}");
    let commit = "commit = \"3f1c2a9e0d6b4c8a7e5f9d1b2c3a4e5f6a7b8c9d\"";
    let cases = [
        (
            format!("url = [\"http://example.org/a.tar.gz\"]\n{hash}"),
            "R010",
        ),
        (
            format!(
                "url = [\"https://example.org/a.tar.gz\"]\ngit = \"https://example.org/a.git\"\n{hash}"
            ),
            "R011",
        ),
        (hash.to_owned(), "R011"),
        ("url = []".to_owned(), "R011"),
        (
            format!("git = \"https://example.org/a.git\"\n{commit}\nfile = \"a.tar\""),
            "R011",
        ),
        (
            "url = [\"https://example.org/a.tar.gz\"]".to_owned(),
            "R012",
        ),
        ("git = \"https://example.org/a.git\"".to_owned(), "R013"),
        (
            format!("url = [\"https://example.org/a.tar.gz\"]\n{hash}\ndest = \"../up\""),
            "R015",
        ),
        (
            format!("url = [\"https://example.org/download?id=3\"]\n{hash}"),
            "R015",
        ),
    ];
    for (fields, code) in cases {
        assert_eq!(codes("foo", &source(&fields)), [code], "{fields}");
    }
    let fine =
        format!("url = [\"https://example.org/download?id=3\"]\n{hash}\nfile = \"a.tar.gz\"");
    assert_eq!(codes("foo", &source(&fine)), Vec::<String>::new());
    let git = format!("git = \"https://example.org/a.git\"\n{commit}");
    assert_eq!(codes("foo", &source(&git)), Vec::<String>::new());
}

#[test]
fn source_labels_are_plain_names() {
    let labelled = |label: &str| {
        format!(
            "  source {label} {{\n    url = [\"https://example.org/a.tar.gz\"]\n    sha256 = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"\n    dest = \"safe\"\n  }}\n  build custom {{}}"
        )
    };
    for label in [
        "\"../../victim\"",
        "\"a/b\"",
        "\"..\"",
        "\".hidden\"",
        "\"a b\"",
        "\"\"",
    ] {
        assert_eq!(codes("foo", &labelled(label)), ["R018"], "{label}");
    }
    for label in ["main", "\"docs-1.2_x\""] {
        assert_eq!(
            codes("foo", &labelled(label)),
            Vec::<String>::new(),
            "{label}"
        );
    }
}

#[test]
fn styles_and_empty_packages_fit_their_sources() {
    let vendor = "  vendor cargo { sha256 = \"sha256:1111111111111111111111111111111111111111111111111111111111111111\", source = \"other\" }";
    let cases = [
        (
            format!("{SOURCE}{vendor}\n  build cargo {{}}"),
            vec!["R014"],
        ),
        (format!("{SOURCE}  build cargo {{}}"), vec!["R017"]),
        (format!("{SOURCE}  build go {{}}"), vec!["R017"]),
        (
            format!("{SOURCE}  empty = true\n  build make {{}}"),
            vec!["R016", "R016"],
        ),
        ("  build custom {}".to_owned(), vec!["R016"]),
        (
            format!("{SOURCE}  build custom {{}}\n  phases {{ subdir = \"a/../b\" }}"),
            vec!["R015"],
        ),
        (
            format!("{SOURCE}  build custom {{}}\n  subpackage \"Libs\" {{ files = [\"x\"] }}"),
            vec!["R003"],
        ),
        (
            format!("{SOURCE}  build custom {{}}\n  alternative \"C\" {{ paths = [\"x\"] }}"),
            vec!["R003"],
        ),
        (
            format!(
                "{SOURCE}  build custom {{}}\n  alternative cc {{ paths = [\"x\"], subpackage = \"tools\" }}"
            ),
            vec!["R019"],
        ),
        (
            format!(
                "{SOURCE}  build custom {{}}\n  subpackage tools {{ files = [\"x\"] }}\n  alternative cc {{ paths = [\"x\"], subpackage = \"tools\" }}"
            ),
            vec![],
        ),
        (
            format!(
                "{SOURCE}  build custom {{}}\n  alternative cc {{ paths = [\"x\"], subpackage = \"dev\" }}"
            ),
            vec![],
        ),
    ];
    for (body, expected) in cases {
        assert_eq!(codes("foo", &body), expected, "{body}");
    }
}

#[test]
fn environment_variables_are_named_and_replace_the_builder_s_only_when_forced() {
    let body = |env: &str| format!("{SOURCE}  build custom {{}}\n  env {{ {env} }}");
    assert_eq!(codes("foo", &body("FOO_1 = \"x\"")), Vec::<String>::new());
    assert_eq!(codes("foo", &body("\"foo\" = \"x\"")), ["R030"]);
    assert_eq!(codes("foo", &body("\"1FOO\" = \"x\"")), ["R030"]);
    assert_eq!(codes("foo", &body("PATH = \"/bin\"")), ["R031"]);
    assert_eq!(codes("foo", &body("MAKEFLAGS = \"-j1\"")), ["R031"]);
    assert_eq!(codes("foo", &body("MAKEFLAGS = force(\"-j1\")")), ["R031"]);
    assert_eq!(codes("foo", &body("JOBS = force(\"1\")")), ["R031"]);
    assert_eq!(codes("foo", &body("PATH = default(\"/bin\")")), ["R031"]);

    let dir = laft_test::tempdir();
    let text = recipe("foo", &body("PATH = force(\"/opt/bin:/usr/bin\")"));
    let repo = write_repo(
        &dir.path().join("core"),
        "core",
        "",
        &[("recipes/foo/package.stav", &text)],
    );
    let repos = Repos::open(&[repo]).expect("the repository opens");
    let recipes = support::evaluate(&repos, "foo");
    assert_eq!(recipes[0].env["PATH"], "/opt/bin:/usr/bin");
}

#[test]
fn update_blocks_choose_one_way_of_finding_versions() {
    let body = |update: &str| format!("{SOURCE}  build custom {{}}\n  update {{ {update} }}");
    assert_eq!(
        codes("foo", &body("github = \"a/b\"")),
        Vec::<String>::new()
    );
    assert_eq!(
        codes(
            "foo",
            &body("url = \"https://example.org/\", pattern = r\"foo-(.*)\"")
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        codes("foo", &body("github = \"a/b\", anitya = 3")),
        ["R050"]
    );
    assert_eq!(
        codes("foo", &body("url = \"https://example.org/\"")),
        ["R050"]
    );
    assert_eq!(codes("foo", &body("")), ["R050"]);
}

#[test]
fn recipe_directories_and_repositories_are_checked_when_opened() {
    let dir = laft_test::tempdir();
    let repo = write_repo(
        &dir.path().join("core"),
        "core",
        "",
        &[("recipes/Foo/package.stav", "")],
    );
    let Err(Error::Problems(problems)) = Repos::open(&[repo.clone(), repo]) else {
        panic!("the repository is invalid");
    };
    let codes: Vec<&str> = problems.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["R003", "R004", "R003"], "{problems:?}");
}

#[test]
fn recipes_branch_on_the_target_they_are_evaluated_for() {
    let body = format!(
        "{SOURCE}  variants = [dynamic, static]\n  build custom {{}}\n  \
         env {{ TRIPLE = target.triple, LIBC = \"#{{target.libc}}\" }}\n  \
         cflags = match target.variant {{\n    dynamic => [\"-DDYN\"]\n    static => [\"-DSTATIC\"]\n  }}"
    );
    let dir = laft_test::tempdir();
    let text = format!(
        "import \"laft/target\" as target\n\n{}",
        recipe("foo", &body)
    );
    let repo = write_repo(
        &dir.path().join("core"),
        "core",
        "",
        &[("recipes/foo/package.stav", &text)],
    );
    let repos = Repos::open(&[repo]).expect("the repository opens");
    let recipes = support::evaluate(&repos, "foo");
    let seen: Vec<(&str, &str, bool)> = recipes
        .iter()
        .map(|r| {
            (
                r.env["TRIPLE"].as_str(),
                r.env["LIBC"].as_str(),
                r.env["CFLAGS"].ends_with(if r.variant == laft_core::Variant::Static {
                    " -DSTATIC"
                } else {
                    " -DDYN"
                }),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("x86_64-laft-linux-gnu", "glibc", true),
            ("x86_64-laft-linux-musl", "musl", true)
        ]
    );
}

#[test]
fn warnings_are_reported_with_the_errors_of_a_failed_evaluation() {
    let text = format!(
        "let dynamic: Variant = Variant.static\n{}",
        recipe(
            "foo",
            &format!("{SOURCE}  build custom {{}}\n  variants = [dynamic]\n  rev = 1 / 0")
        )
    );
    let found = errors("foo", &[("recipes/foo/package.stav", &text)]);
    let codes: Vec<&str> = found.iter().map(|(code, _)| code.as_str()).collect();
    assert_eq!(codes, ["E0603", "W0302"], "{found:?}");
}

#[test]
fn a_missing_patch_is_reported_where_the_list_names_it() {
    let dir = laft_test::tempdir();
    let text = recipe(
        "foo",
        &format!("{SOURCE}  build custom {{}}\n  patches = [./a.patch, ./missing.patch]"),
    );
    let repo = write_repo(
        &dir.path().join("core"),
        "core",
        "",
        &[
            ("recipes/foo/package.stav", &text),
            ("recipes/foo/a.patch", "a\n"),
        ],
    );
    let repos = Repos::open(&[repo]).expect("the repository opens");
    let Err(Error::Report(report)) = repos.evaluate(&support::name("foo")) else {
        panic!("the patch is missing");
    };
    let item = report.iter().next().expect("one error");
    assert_eq!(item.diagnostic.code, "R006");
    let source = report.source(item.file.expect("a place")).expect("known");
    assert_eq!(&source[item.diagnostic.span.range()], "./missing.patch");
}

#[test]
fn recipe_paths_are_relative_and_stay_below_their_tree() {
    let body = |field: &str| format!("{SOURCE}  build custom {{}}\n  {field}");
    for field in [
        "license_files = [\"/etc/shadow\"]",
        "license_files = [\"../../../../etc/shadow\"]",
        "license_files = [\"doc/../../x\"]",
        "license_files = [\"\"]",
        "setuid = [\"/usr/bin/su\"]",
        "caps = { \"usr/../../bin/ping\" = \"cap_net_raw+ep\" }",
        "alternative cc { paths = [\"/usr/bin/cc\"] }",
    ] {
        assert_eq!(codes("foo", &body(field)), ["R015"], "{field}");
    }
    for field in [
        "license_files = [\"doc/LICENSE\", \"COPYING $1\"]",
        "setuid = [\"usr/bin/su\"]",
        "alternative cc { paths = [\"usr/bin/cc\"] }",
    ] {
        assert_eq!(codes("foo", &body(field)), Vec::<String>::new(), "{field}");
    }
}
