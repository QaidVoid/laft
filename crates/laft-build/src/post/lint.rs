//! The step that reports suspicious payload contents.

use std::fs;
use std::io::Read;
use std::path::Path;

use crate::error::{BuildError, Warning, io};
use crate::fsx::{self, Kind};
use crate::post::strip::DEBUG_DIR;

/// Absolute symlink targets that are fine outside `/usr`.
const SYSTEM_DIRS: [&str; 7] = ["/usr", "/etc", "/run", "/var", "/proc", "/dev", "/sys"];

fn under(path: &str, dir: &str) -> bool {
    path == dir || path.starts_with(&format!("{dir}/"))
}

/// Reports what the payload `out` should probably not contain.
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
            Kind::File
                if !rel.starts_with(&format!("{DEBUG_DIR}/"))
                    && contains(&path, BUILD_PATH).map_err(io(&path))? =>
            {
                warnings.push(Warning::new(
                    "W121",
                    format!("{rel} contains /build/, which is probably a leaked build path"),
                ));
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

/// What a leaked build path starts with.
const BUILD_PATH: &[u8] = b"/build/";

/// How much of a file [`contains`] reads at once.
const CHUNK: usize = 64 * 1024;

/// Whether the file `path` contains `needle`, read in chunks so that a
/// large file never sits in memory whole.
fn contains(path: &Path, needle: &[u8]) -> std::io::Result<bool> {
    let mut file = fs::File::open(path)?;
    let keep = needle.len().saturating_sub(1);
    let mut buffer = vec![0; keep + CHUNK];
    let mut filled = 0;
    loop {
        let n = match file.read(&mut buffer[filled..]) {
            Ok(0) => return Ok(false),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        filled += n;
        if buffer[..filled].windows(needle.len()).any(|w| w == needle) {
            return Ok(true);
        }
        let tail = filled.saturating_sub(keep);
        buffer.copy_within(tail..filled, 0);
        filled -= tail;
    }
}

#[cfg(test)]
#[path = "lint_tests.rs"]
mod tests;
