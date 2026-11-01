//! Package options: declarations, the values of a build, and what they
//! change about metadata and build keys.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use laft_build::{Error, KeyInputs, LinkInput, Repos};
use laft_core::{Blake3Hash, OptionValue, to_canonical_json};

/// The option set of name value pairs.
fn set(entries: &[(&str, OptionValue)]) -> BTreeMap<String, OptionValue> {
    entries
        .iter()
        .map(|(n, v)| ((*n).to_owned(), v.clone()))
        .collect()
}

/// A repository with a package that declares a tag option and a Bool
/// option, and branches on the Bool.
fn options_repo(dir: &Path) -> PathBuf {
    support::write_repo(
        &dir.join("core"),
        "core",
        "",
        &[
            (
                "recipes/optpkg/options.stav",
                "## How the drivers are chosen.\ntype Drivers = all | amd | intel\n\nstruct Options {\n  ## Build for every driver the hardware supports.\n  drivers: Drivers = all\n  ## Ship the extra command line tool.\n  tool: Bool = true\n}\n",
            ),
            (
                "recipes/optpkg/package.stav",
                "import \"laft/options\" as opt\n\npackage optpkg {\n  version = \"1.0\"\n  summary = if opt.tool { \"tools in\" } else { \"tools out\" }\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
            ),
        ],
    )
}

/// Writes a repository named `repo` holding `files` below `dir`.
fn repo(dir: &Path, repo: &str, files: &[(&str, &str)]) -> PathBuf {
    support::write_repo(&dir.join(repo), repo, "", files);
    dir.join(repo)
}

/// Writes a repository whose descriptor and files are given verbatim.
fn repo_at(path: &Path, repo: &str, descriptor: &str, files: &[(&str, &str)]) -> PathBuf {
    support::write_repo(path, repo, descriptor, files);
    path.to_path_buf()
}

#[test]
fn options_default_and_override() {
    let dir = laft_test::tempdir();
    let repo = options_repo(dir.path());
    let repos = Repos::open(&[repo]).expect("the repository opens");
    let name = support::name("optpkg");

    let recipes = repos
        .evaluate(&name)
        .expect("the defaults evaluate")
        .recipes;
    assert_eq!(recipes[0].meta.summary, "tools in");
    assert_eq!(
        recipes[0].option_values,
        set(&[
            ("drivers", OptionValue::tag("all")),
            ("tool", OptionValue::TRUE)
        ])
    );

    let recipes = repos
        .evaluate_with(&name, &set(&[("tool", OptionValue::FALSE)]))
        .expect("the set evaluates")
        .recipes;
    assert_eq!(recipes[0].meta.summary, "tools out");
    assert_eq!(
        recipes[0].option_values,
        set(&[
            ("drivers", OptionValue::tag("all")),
            ("tool", OptionValue::FALSE)
        ])
    );

    let recipes = repos
        .evaluate_with(&name, &set(&[("drivers", OptionValue::tag("amd"))]))
        .expect("the tag fits the sum type")
        .recipes;
    assert_eq!(recipes[0].option_values["drivers"], OptionValue::tag("amd"));
    assert_eq!(recipes[0].meta.summary, "tools in");
}

#[test]
fn a_set_that_names_an_unknown_option_or_tag_is_r073() {
    let dir = laft_test::tempdir();
    let repos_dir = dir.path().join("repos");
    std::fs::create_dir_all(&repos_dir).expect("writable");
    let options = options_repo(&repos_dir);
    let repos = Repos::open(&[options]).expect("the repository opens");
    let name = support::name("optpkg");

    let err = repos
        .evaluate_with(&name, &set(&[("speed", OptionValue::TRUE)]))
        .expect_err("the option does not exist");
    let text = err.to_string();
    assert!(text.contains("R073"), "{text}");
    assert!(text.contains("speed"), "{text}");

    let err = repos
        .evaluate_with(&name, &set(&[("drivers", OptionValue::tag("nv"))]))
        .expect_err("the tag does not exist");
    assert!(err.to_string().contains("R073"), "{err}");

    let err = repos
        .evaluate_with(&name, &set(&[("tool", OptionValue::tag("amd"))]))
        .expect_err("the option is a Bool");
    assert!(err.to_string().contains("R073"), "{err}");

    let err = repos
        .evaluate_with(&name, &set(&[("drivers", OptionValue::TRUE)]))
        .expect_err("the option is a sum type");
    assert!(err.to_string().contains("R073"), "{err}");

    let plain = repo(
        dir.path(),
        "core",
        &[(
            "recipes/plain/package.stav",
            &support::recipe("plain", "  empty = true\n  build custom {}"),
        )],
    );
    let repos = Repos::open(&[plain]).expect("the repository opens");
    let err = repos
        .evaluate_with(
            &support::name("plain"),
            &set(&[("tool", OptionValue::TRUE)]),
        )
        .expect_err("the package declares no options");
    assert!(err.to_string().contains("R073"), "{err}");
}

#[test]
fn an_import_without_options_stav_is_r070() {
    let dir = laft_test::tempdir();
    let path = repo(
        dir.path(),
        "core",
        &[(
            "recipes/wish/package.stav",
            "import \"laft/options\" as opt\n\npackage wish {\n  version = \"1.0\"\n  summary = \"Wishful\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
        )],
    );
    let repos = Repos::open(&[path]).expect("the repository opens");
    let Err(Error::Report(report)) = repos.evaluate(&support::name("wish")) else {
        panic!("the import has no module");
    };
    let codes: Vec<_> = report.iter().map(|d| d.diagnostic.code).collect();
    assert_eq!(codes, ["R070"]);
}

#[test]
fn an_overlay_redeclaring_a_declaration_is_r071_and_adding_one_works() {
    let dir = laft_test::tempdir();
    let base = repo_at(
        &dir.path().join("base"),
        "base",
        "",
        &[
            (
                "recipes/optpkg/options.stav",
                "## How the drivers are chosen.\ntype Drivers = all | amd\n\nstruct Options {\n  ## Build for every driver the hardware supports.\n  drivers: Drivers = all\n}\n",
            ),
            (
                "recipes/optpkg/package.stav",
                "import \"laft/options\" as opt\n\npackage optpkg {\n  version = \"1.0\"\n  summary = \"drivers\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
            ),
        ],
    );
    let descriptor = "  extends = [\"base\"]\n";

    let adding = support::write_repo(
        &dir.path().join("extra"),
        "extra",
        descriptor,
        &[(
            "recipes/optpkg/options.stav",
            "## The extra video API.\ntype Vulkan = on | off\n\nstruct Options {\n  ## Build the Vulkan loader.\n  vulkan: Vulkan = on\n}\n",
        )],
    );
    let repos = Repos::open(&[base.clone(), adding]).expect("the repositories open");
    let recipes = repos
        .evaluate(&support::name("optpkg"))
        .expect("evaluates")
        .recipes;
    assert_eq!(
        recipes[0].option_values,
        set(&[
            ("drivers", OptionValue::tag("all")),
            ("vulkan", OptionValue::tag("on"))
        ])
    );

    let redeclared = support::write_repo(
        &dir.path().join("again"),
        "extra",
        descriptor,
        &[(
            "recipes/optpkg/options.stav",
            "struct Options {\n  ## A second try at the same option.\n  drivers: Drivers = amd\n}\n",
        )],
    );
    let repos = Repos::open(&[base, redeclared]).expect("the repositories open");
    let err = repos
        .evaluate(&support::name("optpkg"))
        .expect_err("the overlay redeclares the option");
    assert!(err.to_string().contains("R071"), "{err}");
}

#[test]
fn options_files_that_break_a_rule_are_r072() {
    let dir = laft_test::tempdir();
    let header = "## The modes.\ntype modes = fast | slow\n\n";
    let cases: Vec<(&str, String)> = vec![
        (
            "a field without doc comment",
            "struct Options {\n  fast: Bool = true\n}\n".to_owned(),
        ),
        (
            "a field without default",
            "struct Options {\n  ## The speed.\n  fast: Bool\n}\n".to_owned(),
        ),
        (
            "a field of a type that is neither Bool nor a sum type",
            "struct Options {\n  ## The speed.\n  fast: Str = \"fast\"\n}\n".to_owned(),
        ),
        (
            "a field of a sum type the file does not declare",
            "struct Options {\n  ## The speed.\n  fast: speeds = fast\n}\n".to_owned(),
        ),
        (
            "a default that is no tag of the field type",
            format!("{header}struct Options {{\n  ## The speed.\n  fast: modes = quick\n}}\n"),
        ),
        (
            "a tag with a payload",
            "## The modes.\ntype modes = fast(n: Int) | slow\n\nstruct Options {\n  ## The speed.\n  fast: modes = slow\n}\n".to_owned(),
        ),
        (
            "a struct other than Options",
            "struct Other {\n  ## The speed.\n  fast: Bool = true\n}\n".to_owned(),
        ),
        (
            "a declaration that is neither a sum type nor Options",
            "let fast = true\n".to_owned(),
        ),
        (
            "an import",
            "import \"std/build\"\n\nstruct Options {\n  ## The speed.\n  fast: Bool = true\n}\n".to_owned(),
        ),
        (
            "a tag outside the name rules",
            "## The modes.\ntype modes = Fast | slow\n\nstruct Options {\n  ## The speed.\n  fast: modes = slow\n}\n".to_owned(),
        ),
        (
            "an option name outside the name rules",
            "struct Options {\n  ## The speed.\n  Fast: Bool = true\n}\n".to_owned(),
        ),
        (
            "a doc comment line over 80 characters",
            "struct Options {\n  ## This line says what the option does but it keeps going and going well past the limit of eighty\n  fast: Bool = true\n}\n".to_owned(),
        ),
        (
            "a file that does not parse",
            "struct Options {\n".to_owned(),
        ),
        (
            "a sum type named like the struct",
            "## The modes.\ntype Options = fast | slow\n\nstruct Options {\n  ## The speed.\n  fast: Bool = true\n}\n".to_owned(),
        ),
        (
            "a sum type named like a built-in type",
            "## The modes.\ntype Bool = fast | slow\n\nstruct Options {\n  ## The speed.\n  fast: Bool = true\n}\n".to_owned(),
        ),
        (
            "a sum type named like a type of the recipe language",
            "## The shells.\ntype Shell = sh | bash\n\nstruct Options {\n  ## The shell.\n  fast: Bool = true\n}\n".to_owned(),
        ),
        (
            "a tag written like a bool",
            "## The modes.\ntype Modes = true | slow\n\nstruct Options {\n  ## The speed.\n  fast: Modes = slow\n}\n".to_owned(),
        ),
    ];
    let package = (
        "recipes/optpkg/package.stav",
        "package optpkg {\n  version = \"1.0\"\n  summary = \"The options package\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    );
    for (why, text) in cases {
        let repos_dir = dir.path().join("repos");
        std::fs::create_dir_all(&repos_dir).expect("writable");
        let path = repo(
            &repos_dir,
            "core",
            &[("recipes/optpkg/options.stav", text.as_str()), package],
        );
        let repos = Repos::open(&[path]).expect("the repository opens");
        let err = repos.evaluate(&support::name("optpkg")).expect_err(why);
        assert!(err.to_string().contains("R072"), "{why}: {err}");
    }
}

#[test]
fn a_broken_options_file_names_its_repository() {
    let dir = laft_test::tempdir();
    let broken = (
        "recipes/optpkg/options.stav",
        "struct Options {\n  fast: Bool = true\n}\n",
    );
    let package = (
        "recipes/optpkg/package.stav",
        "package optpkg {\n  version = \"1.0\"\n  summary = \"The options package\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    );
    let first = repo_at(&dir.path().join("first"), "first", "", &[broken, package]);
    let second = repo_at(
        &dir.path().join("second"),
        "second",
        "  extends = [\"first\"]\n",
        &[broken, package],
    );
    let repos = Repos::open(&[first, second]).expect("the repositories open");
    let err = repos
        .evaluate(&support::name("optpkg"))
        .expect_err("both options files break the same rule");
    let text = err.to_string();
    assert!(text.contains("repository first"), "{text}");
    assert!(text.contains("repository second"), "{text}");
}

#[test]
fn a_broken_options_file_is_reported_before_the_set_is() {
    let dir = laft_test::tempdir();
    let repo = repo(
        dir.path(),
        "core",
        &[
            ("recipes/optpkg/options.stav", "struct Options {\n"),
            (
                "recipes/optpkg/package.stav",
                "import \"laft/options\" as opt\n\npackage optpkg {\n  version = \"1.0\"\n  summary = \"The options package\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
            ),
        ],
    );
    let repos = Repos::open(&[repo]).expect("the repository opens");
    let err = repos
        .evaluate_with(
            &support::name("optpkg"),
            &set(&[("speed", OptionValue::TRUE)]),
        )
        .expect_err("the file does not parse");
    assert!(err.to_string().contains("R072"), "{err}");
    assert!(!err.to_string().contains("R073"), "{err}");
}

#[test]
fn a_rejected_declaration_names_the_file_and_the_place() {
    let dir = laft_test::tempdir();
    let repo = repo(
        dir.path(),
        "core",
        &[
            (
                "recipes/optpkg/options.stav",
                "## Gr\u{f6}\u{df}e\nstruct Options {\n  ## The speed.\n  fast: Bool\n}\n",
            ),
            (
                "recipes/optpkg/package.stav",
                "package optpkg {\n  version = \"1.0\"\n  summary = \"The options package\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
            ),
        ],
    );
    let repos = Repos::open(&[repo]).expect("the repository opens");
    let err = repos
        .evaluate(&support::name("optpkg"))
        .expect_err("the option has no default");
    let text = err.to_string();
    assert!(text.contains("recipes/optpkg/options.stav"), "{text}");
    assert!(text.contains("line 4, column 3"), "{text}");
}

#[test]
fn a_recipe_without_options_keeps_its_key_exactly() {
    let dir = laft_test::tempdir();
    let path = repo(
        dir.path(),
        "core",
        &[(
            "recipes/zlib/package.stav",
            &support::recipe("zlib", "  empty = true\n  build custom {}"),
        )],
    );
    let repos = Repos::open(&[path]).expect("the repository opens");
    let recipes = support::evaluate(&repos, "zlib");
    assert!(recipes[0].option_values.is_empty());

    let mut sonames = BTreeSet::new();
    sonames.insert("libz.so.1".to_owned());
    let inputs = KeyInputs::new(
        &recipes[0],
        vec![LinkInput {
            name: support::name("zlib"),
            abi_epoch: 1,
            sonames,
            options: BTreeMap::new(),
        }],
        Vec::new(),
    );
    let json = String::from_utf8(to_canonical_json(&inputs).expect("serializable")).expect("UTF-8");
    let plan = inputs.plan.to_string();
    assert_eq!(
        json,
        format!(
            "{{\"links\":[{{\"abi_epoch\":1,\"name\":\"zlib\",\"sonames\":[\"libz.so.1\"]}}],\
             \"patches\":[],\"plan\":\"{plan}\",\"sources\":[],\"static\":[],\"toolchain_epoch\":0}}"
        )
    );
}

#[test]
fn option_values_change_the_build_key_but_not_the_plan_hash() {
    let dir = laft_test::tempdir();
    let path = repo(
        dir.path(),
        "core",
        &[
            (
                "recipes/optpkg/options.stav",
                "struct Options {\n  ## Ship the extra command line tool.\n  tool: Bool = true\n}\n",
            ),
            (
                "recipes/optpkg/package.stav",
                "package optpkg {\n  version = \"1.0\"\n  summary = \"The options package\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
            ),
        ],
    );
    let repos = Repos::open(&[path]).expect("the repository opens");
    let name = support::name("optpkg");
    let mut recipes = repos
        .evaluate(&name)
        .expect("the defaults evaluate")
        .recipes;
    let defaults = recipes.remove(0);
    let mut others = repos
        .evaluate_with(&name, &set(&[("tool", OptionValue::FALSE)]))
        .expect("evaluates")
        .recipes;
    let other = others.remove(0);

    assert_eq!(defaults.plan_hash(), other.plan_hash());
    let plain = KeyInputs::new(&defaults, Vec::new(), Vec::new());
    let changed = KeyInputs::new(&other, Vec::new(), Vec::new());
    assert_ne!(plain.build_key(), changed.build_key());
    let json =
        String::from_utf8(to_canonical_json(&changed).expect("serializable")).expect("UTF-8");
    assert!(json.contains(r#""options":{"tool":false}"#), "{json}");
}

#[test]
fn a_link_dependency_with_options_changes_the_dependents_key() {
    let mut sonames = BTreeSet::new();
    sonames.insert("libz.so.1".to_owned());
    let without = LinkInput {
        name: support::name("zlib"),
        abi_epoch: 1,
        sonames: sonames.clone(),
        options: BTreeMap::new(),
    };
    let with = LinkInput {
        options: set(&[("ssl", OptionValue::TRUE)]),
        ..without.clone()
    };
    let a = String::from_utf8(to_canonical_json(&without).expect("serializable")).expect("UTF-8");
    let b = String::from_utf8(to_canonical_json(&with).expect("serializable")).expect("UTF-8");
    assert_eq!(
        a,
        r#"{"abi_epoch":1,"name":"zlib","sonames":["libz.so.1"]}"#
    );
    assert_eq!(
        b,
        r#"{"abi_epoch":1,"name":"zlib","options":{"ssl":true},"sonames":["libz.so.1"]}"#
    );
    assert_ne!(Blake3Hash::of(a.as_bytes()), Blake3Hash::of(b.as_bytes()));
}
