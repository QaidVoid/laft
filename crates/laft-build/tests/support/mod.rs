//! Helpers shared by the integration tests.

#![allow(dead_code, reason = "each test file uses a different part")]

use std::fs;
use std::path::{Path, PathBuf};

use laft_build::{Recipe, Repos};
use laft_core::Name;

/// The repository with the example recipes.
pub(crate) fn core() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core")
}

/// Parses a package name.
pub(crate) fn name(name: &str) -> Name {
    name.parse().expect("a valid package name")
}

/// Evaluates `package` in `repos` and returns its recipes, failing the test
/// with the rendered report otherwise.
pub(crate) fn evaluate(repos: &Repos, package: &str) -> Vec<Recipe> {
    match repos.evaluate(&name(package)) {
        Ok(evaluation) => evaluation.recipes,
        Err(e) => panic!("{package} does not evaluate:\n{e}"),
    }
}

/// Writes a recipe repository named `repo` at `dir` with the given
/// descriptor body and files.
pub(crate) fn write_repo(
    dir: &Path,
    repo: &str,
    descriptor: &str,
    files: &[(&str, &str)],
) -> PathBuf {
    fs::create_dir_all(dir).expect("writable");
    fs::write(
        dir.join("recipes.stav"),
        format!("recipe_repo {repo} {{\n{descriptor}}}\n"),
    )
    .expect("writable");
    for (path, text) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().expect("a file has a parent")).expect("writable");
        fs::write(path, text).expect("writable");
    }
    dir.to_path_buf()
}

/// A recipe for `name` with the given extra body lines, a source and the
/// `custom` style unless the lines choose another.
pub(crate) fn recipe(name: &str, body: &str) -> String {
    format!(
        "package {name} {{\n  version = \"1.0\"\n  summary = \"The {name} package\"\n  license = [MIT]\n{body}\n}}\n"
    )
}

/// A `source main` block with a URL and a valid hash.
pub(crate) const SOURCE: &str = "  source main {\n    url = [\"https://example.org/x-#{self.version}.tar.gz\"]\n    sha256 = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"\n  }\n";
