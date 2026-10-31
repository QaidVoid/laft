//! The jobserver that shares the job budget of a run between its builds.

use rustix::fs::{CWD, Mode, OFlags};
use rustix::io::Errno;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A GNU make jobserver: a named pipe that holds one token for each job
/// the builds of a run may start beyond the one every running build holds.
///
/// A build that joins it reads one token before it starts such a job and
/// writes it back when the job ends, so the tokens of the pipe bound the
/// jobs of every build together. A build killed outright loses the tokens
/// it held, so [`Jobserver::refill`] restores the full count whenever no
/// build runs.
///
/// Both ends of the pipe stay open for the life of the server, so a build
/// never waits for a peer to open it and a token handed back is never lost
/// to a broken pipe.
#[derive(Debug)]
pub(crate) struct Jobserver {
    /// The path of the pipe.
    path: PathBuf,
    /// The end that reads the tokens back.
    read: File,
    /// The end that hands them out.
    write: File,
    /// The number of tokens the pipe holds when every job that took one
    /// gave it back.
    tokens: usize,
}

impl Jobserver {
    /// A jobserver of `tokens` tokens in `dir`, which every build of the
    /// run binds at its own fixed path.
    ///
    /// # Errors
    ///
    /// Fails when the pipe cannot be created or an end cannot be opened.
    pub(crate) fn open(dir: impl AsRef<Path>, tokens: usize) -> std::io::Result<Self> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let path = dir.join("jobserver");
        // A leftover pipe of a run that died carries no state, so it is
        // replaced.
        if let Err(error) = rustix::fs::mkfifoat(CWD, &path, Mode::from_raw_mode(0o666)) {
            if error != Errno::EXIST {
                return Err(error.into());
            }
            std::fs::remove_file(&path)?;
            rustix::fs::mkfifoat(CWD, &path, Mode::from_raw_mode(0o666))?;
        }
        let flags = OFlags::CLOEXEC | OFlags::NONBLOCK;
        let read = File::from(rustix::fs::open(
            &path,
            flags | OFlags::RDONLY,
            Mode::empty(),
        )?);
        let write = File::from(rustix::fs::open(
            &path,
            flags | OFlags::WRONLY,
            Mode::empty(),
        )?);
        let server = Self {
            path,
            read,
            write,
            tokens,
        };
        server.refill();
        Ok(server)
    }

    /// The path of the pipe.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Empties the pipe and fills it back up to its full token count,
    /// which restores the tokens a killed build lost. It runs while no
    /// build is running, so no token it takes away belongs to a job.
    pub(crate) fn refill(&self) {
        let mut sink = [0; 4096];
        while matches!((&self.read).read(&mut sink), Ok(n) if n > 0) {}
        let chunk = [b'\n'; 64];
        let mut left = self.tokens;
        while left > 0 {
            let n = left.min(chunk.len());
            if (&self.write).write_all(&chunk[..n]).is_err() {
                return;
            }
            left -= n;
        }
    }
}

#[cfg(test)]
#[path = "jobserver_tests.rs"]
mod tests;
