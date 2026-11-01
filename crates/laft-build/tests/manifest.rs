//! The repository descriptor beyond its `recipe_repo` block.

mod support;

use laft_build::{OptionText, Repos};
use support::write_repo;

/// The `Name` of `s`, for map lookups.
fn name(s: &str) -> laft_core::Name {
    s.parse().expect("the test name is valid")
}

/// Opens the repository at `dir` whose `recipes.stav` is `text`.
fn manifest(dir: &laft_test::TempDir, text: &str) -> Result<Repos, laft_build::Error> {
    let repo = write_repo(
        &dir.path().join("core"),
        "core",
        "",
        &[(
            "recipes/mesa/package.stav",
            &support::recipe("mesa", "  empty = true\n  build custom {}\n"),
        )],
    );
    std::fs::write(repo.join("recipes.stav"), text).expect("the descriptor is written");
    Repos::open(&[repo])
}

#[test]
fn a_descriptor_declares_publish_sets_and_profiles() {
    let dir = laft_test::tempdir();
    let repos = manifest(
        &dir,
        "recipe_repo core {}\n\npublish mesa {\n  options {\n    llvm = false\n    vulkan = \"amd\"\n  }\n}\n\nprofile minimal {\n  package mesa {\n    options {\n      x11 = false\n    }\n  }\n}\n",
    )
    .expect("the descriptor is valid");
    let core = &repos.repos()[0];
    let mesa = core.publish.get(&name("mesa")).expect("the mesa set");
    assert_eq!(mesa.get("llvm"), Some(&OptionText::Bool(false)));
    assert_eq!(mesa.get("vulkan"), Some(&OptionText::Tag("amd".to_owned())));
    let minimal = core.profiles.get("minimal").expect("the profile");
    let mesa = minimal.package.get(&name("mesa")).expect("the mesa values");
    assert_eq!(mesa.get("x11"), Some(&OptionText::Bool(false)));
}
#[test]
fn a_descriptor_declares_neither_publish_nor_profiles_by_default() {
    let dir = laft_test::tempdir();
    let repos = manifest(&dir, "recipe_repo core {}\n").expect("the descriptor is valid");
    let core = &repos.repos()[0];
    assert!(core.publish.is_empty());
    assert!(core.profiles.is_empty());
}

#[test]
fn a_publish_or_profile_name_that_is_no_recipe_name_is_invalid() {
    let dir = laft_test::tempdir();
    let error = manifest(
        &dir,
        "recipe_repo core {}\n\npublish Mesa {\n  options {\n    x11 = false\n  }\n}\n",
    )
    .expect_err("the recipe name is invalid");
    assert!(error.to_string().contains("core publishes Mesa"), "{error}");
    let dir = laft_test::tempdir();
    let error = manifest(
        &dir,
        "recipe_repo core {}\n\nprofile minimal {\n  package Mesa {\n    options {\n      x11 = false\n    }\n  }\n}\n",
    )
    .expect_err("the recipe name is invalid");
    assert!(
        error
            .to_string()
            .contains("the profile minimal for Mesa, which is invalid"),
        "{error}"
    );
}

#[test]
fn an_option_value_that_is_no_bool_or_tag_is_rejected() {
    let dir = laft_test::tempdir();
    let error = manifest(
        &dir,
        "recipe_repo core {}\n\npublish mesa {\n  options {\n    jobs = 3\n  }\n}\n",
    )
    .expect_err("an Int is no option value");
    assert!(
        error.to_string().to_lowercase().contains("bool")
            || error.to_string().to_lowercase().contains("tag")
            || error.to_string().to_lowercase().contains("string"),
        "{error}"
    );
}
