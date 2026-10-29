//! Configuration directories, loaders and diagnostic helpers shared by the
//! integration tests.

// Each test binary uses a different part of this module.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

use laft_config::{Loaded, PkgError, Report};
use laft_core::PkgRef;
use laft_test::TempDir;

/// A root filesystem and an EFI system partition, which every system needs.
pub(crate) const DISKS: &str = r#"
  fs "/" { device = "UUID=3f1c2a8e", fstype = ext4 }
  fs "/boot" { device = "UUID=1A2B-3C4D", fstype = vfat }
"#;

/// Writes `files` into a new configuration directory.
pub(crate) fn config_dir(files: &[(&str, &str)]) -> TempDir {
    let dir = laft_test::tempdir();
    for (path, text) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a parent")).expect("mkdir");
        fs::write(path, text).expect("the file can be written");
    }
    dir
}

/// A resolver that knows every package except those whose name starts
/// with `missing`.
pub(crate) fn any_package(pkg: &PkgRef) -> Result<(), PkgError> {
    if pkg.name().as_str().starts_with("missing") {
        return Err(PkgError {
            message: format!("no package `{}` in the index", pkg.name()),
            help: None,
        });
    }
    Ok(())
}

/// Loads the configuration in `dir` with [`any_package`].
pub(crate) fn load(dir: &Path) -> Result<Loaded, Report> {
    laft_config::load(dir, &any_package)
}

/// Loads a configuration whose entry file is `system { <body> }` with
/// [`DISKS`] added to the body.
pub(crate) fn load_body(body: &str) -> Result<Loaded, Report> {
    let dir = config_dir(&[("system.stav", &format!("system {{\n{DISKS}\n{body}\n}}\n"))]);
    load(dir.path())
}

/// Returns the codes of the diagnostics of `report`, in order.
pub(crate) fn codes(report: &Report) -> Vec<&'static str> {
    report.iter().map(|d| d.diagnostic.code).collect()
}

/// Returns the source text a diagnostic points at.
pub(crate) fn pointed_at(report: &Report, index: usize) -> &str {
    let item = report.iter().nth(index).expect("the diagnostic exists");
    let file = item.file.expect("the diagnostic has a place");
    let text = report.source(file).expect("the file is known");
    &text[item.diagnostic.span.range()]
}
