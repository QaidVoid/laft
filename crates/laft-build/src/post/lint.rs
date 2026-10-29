//! The step that reports suspicious payload contents.

use std::fs;
use std::path::Path;

use crate::error::{BuildError, Warning, io};
use crate::fsx::{self, Kind};
use crate::post::strip::DEBUG_DIR;

/// Absolute symlink targets that are fine outside `/usr`.
const SYSTEM_DIRS: [&str; 7] = ["/usr", "/etc", "/run", "/var", "/proc", "/dev", "/sys"];

fn under(path: &str, dir: &str) -> bool {
    path == dir || path.starts_with(&format!("{dir}/"))
}

/// P15: reports what the payload `out` should probably not contain.
///
/// A symlink into `/build` or `/out` is the error `B109`. The warnings are
/// `W120` for an absolute symlink outside the system directories, `W121`
/// for a file other than debug info that contains `/build/`, a leaked build
/// path, `W122` for every file of `world_writable`, and `W101` when no
/// license file is in `usr/share/licenses/`.
///
/// # Errors
///
/// Fails with `B109`, and when the payload cannot be read.
pub fn lint(
    out: &Path,
    world_writable: &[String],
    warnings: &mut Vec<Warning>,
) -> Result<(), BuildError> {
    let mut licensed = false;
    for (rel, kind) in fsx::walk(out)? {
        let path = out.join(&rel);
        licensed |= kind != Kind::Dir && rel.starts_with("usr/share/licenses/");
        match kind {
            Kind::Symlink => {
                let target = fs::read_link(&path).map_err(io(&path))?;
                let target = target.to_string_lossy();
                if under(&target, "/build") || under(&target, "/out") {
                    return Err(BuildError::problem(
                        "B109",
                        format!("{rel} points into the build at {target}"),
                    ));
                }
                if target.starts_with('/') && !SYSTEM_DIRS.iter().any(|d| under(&target, d)) {
                    warnings.push(Warning::new(
                        "W120",
                        format!("{rel} points to {target}, outside the system directories"),
                    ));
                }
            }
            Kind::File if !rel.starts_with(&format!("{DEBUG_DIR}/")) => {
                let data = fs::read(&path).map_err(io(&path))?;
                if data.windows(7).any(|w| w == b"/build/") {
                    warnings.push(Warning::new(
                        "W121",
                        format!("{rel} contains /build/, which is probably a leaked build path"),
                    ));
                }
            }
            _ => {}
        }
    }
    for rel in world_writable {
        warnings.push(Warning::new(
            "W122",
            format!("{rel} was installed writable by everyone"),
        ));
    }
    if !licensed {
        warnings.push(Warning::new(
            "W101",
            "the payload has no license file in usr/share/licenses/",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "lint_tests.rs"]
mod tests;
