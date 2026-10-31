//! Tests of the jobserver pipe and its tokens.

use std::fs::File;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::fs::FileTypeExt;

use rustix::fs::{Mode, OFlags};

use crate::jobserver::Jobserver;

/// Reads every token the pipe at `path` holds, without waiting.
fn take(path: &std::path::Path) -> Vec<u8> {
    let mut file = File::from(
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .expect("the pipe can be opened for reading"),
    );
    let mut tokens = Vec::new();
    let mut buf = [0; 16];
    loop {
        match file.read(&mut buf) {
            Ok(n) if n > 0 => tokens.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    tokens
}

/// Writes `tokens` into the pipe at `path`, like a build that gives one
/// back.
fn give(path: &std::path::Path, tokens: &[u8]) {
    let file = File::from(
        rustix::fs::open(path, OFlags::WRONLY | OFlags::CLOEXEC, Mode::empty())
            .expect("the pipe can be opened for writing"),
    );
    rustix::io::write(file.as_fd(), tokens).expect("the tokens can be written");
}

#[test]
fn a_jobserver_holds_its_tokens() {
    let dir = laft_test::tempdir();
    let server = Jobserver::open(dir.path(), 3).expect("the jobserver can be created");
    assert!(
        dir.path()
            .join("jobserver")
            .symlink_metadata()
            .expect("the pipe exists")
            .file_type()
            .is_fifo()
    );
    assert_eq!(take(server.path()), b"\n\n\n");
}

#[test]
fn a_refill_restores_the_tokens_a_build_lost() {
    let dir = laft_test::tempdir();
    let server = Jobserver::open(dir.path(), 3).expect("the jobserver can be created");
    assert_eq!(take(server.path()).len(), 3);
    give(server.path(), b"\n");
    server.refill();
    assert_eq!(take(server.path()), b"\n\n\n");
}

#[test]
fn a_jobserver_of_no_tokens_holds_none() {
    let dir = laft_test::tempdir();
    let server = Jobserver::open(dir.path(), 0).expect("the jobserver can be created");
    assert!(take(server.path()).is_empty());
    server.refill();
    assert!(take(server.path()).is_empty());
}

#[test]
fn a_writer_of_the_pipe_never_waits_for_a_reader() {
    let dir = laft_test::tempdir();
    let server = Jobserver::open(dir.path(), 1).expect("the jobserver can be created");
    give(server.path(), b"\n");
}
