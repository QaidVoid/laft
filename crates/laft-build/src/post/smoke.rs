//! The step that checks a package in a view with nothing but its runtime
//! dependencies.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::error::{BuildError, io};
use crate::exec::Executor;
use crate::fsx;

/// Runs the recipe's smoke test `script` in a sandbox whose `/usr` is
/// `view`, a view that holds only the main package, its runtime dependency
/// closure, `bash` and `coreutils`. The sandbox's `/build` and `/out` are
/// new empty directories below `work`, which is removed afterwards.
///
/// The test catches runtime dependencies that were neither declared nor
/// detected, because the view holds nothing else.
///
/// # Errors
///
/// Fails with `B110` and the script's output when it fails, and when the
/// sandbox cannot be set up.
pub fn smoke(
    view: &Path,
    script: &str,
    work: &Path,
    logs: &Path,
    timeout: Duration,
) -> Result<(), BuildError> {
    let (build, out) = (work.join("build"), work.join("out"));
    for dir in [build.join("src"), build.join("tmp"), out.clone()] {
        fs::create_dir_all(&dir).map_err(io(&dir))?;
    }
    let env = BTreeMap::from([
        ("PATH".to_owned(), "/usr/bin".to_owned()),
        ("HOME".to_owned(), "/build".to_owned()),
        ("TMPDIR".to_owned(), "/build/tmp".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("TZ".to_owned(), "UTC".to_owned()),
    ]);
    let result = Executor::sandboxed(view, &build, &out, logs)
        .timeout(timeout)
        .run("smoke", script, &env);
    fsx::remove_tree(work)?;
    match result {
        Err(BuildError::Problem(p)) if p.code == "B040" => Err(BuildError::problem(
            "B110",
            format!("smoke test failed: {}", p.message),
        )),
        other => other,
    }
}

#[cfg(test)]
#[path = "smoke_tests.rs"]
mod tests;
