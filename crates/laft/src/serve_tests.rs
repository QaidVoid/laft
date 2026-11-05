use super::*;

use std::io::{Read, Write};
use std::net::TcpStream;

/// A running server over a temporary repository, its address and the
/// directory that must outlive it.
struct Fixture {
    addr: std::net::SocketAddr,
    _dir: laft_test::TempDir,
}

impl Fixture {
    fn start() -> Self {
        let dir = laft_test::tempdir();
        let root = dir.path().join("laft/local");
        std::fs::create_dir_all(&root).expect("writable");
        std::fs::write(root.join("pool.txt"), b"hello world").expect("writable");
        let server = Serve::bind(&root, Some("127.0.0.1:0")).expect("the server binds");
        let addr = server.addr();
        thread::spawn(move || server.run(true));
        Self { addr, _dir: dir }
    }

    /// Sends a raw request and reads the whole answer.
    fn ask(&self, request: &str) -> Vec<u8> {
        let mut stream = TcpStream::connect(self.addr).expect("the server is listening");
        stream.write_all(request.as_bytes()).expect("sent");
        let mut answer = Vec::new();
        stream
            .read_to_end(&mut answer)
            .expect("the connection closes");
        answer
    }

    /// The status line of an answer.
    fn status(answer: &[u8]) -> String {
        let line = std::str::from_utf8(answer)
            .expect("the answer is text")
            .lines()
            .next()
            .expect("a status line");
        line.to_owned()
    }

    /// The value of a header of an answer.
    fn header<'a>(answer: &'a [u8], name: &str) -> Option<&'a str> {
        let text = std::str::from_utf8(answer).expect("the answer is text");
        text.lines()
            .take_while(|line| !line.is_empty())
            .find_map(|line| {
                let (field, value) = line.split_once(':')?;
                field
                    .trim()
                    .eq_ignore_ascii_case(name)
                    .then(|| value.trim())
            })
    }

    /// The body of an answer, after its empty line.
    fn body(answer: &[u8]) -> &[u8] {
        answer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|at| &answer[at + 4..])
            .unwrap_or(&[])
    }
}

#[test]
fn a_file_is_served_whole() {
    let fx = Fixture::start();
    let answer = fx.ask("GET /pool.txt HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 200 OK");
    assert_eq!(Fixture::header(&answer, "content-length"), Some("11"));
    assert_eq!(Fixture::body(&answer), b"hello world");
}

#[test]
fn a_head_request_serves_no_body() {
    let fx = Fixture::start();
    let answer = fx.ask("HEAD /pool.txt HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 200 OK");
    assert_eq!(Fixture::header(&answer, "content-length"), Some("11"));
    assert!(Fixture::body(&answer).is_empty());
}

#[test]
fn a_suffix_of_a_file_is_served_for_a_range() {
    let fx = Fixture::start();
    let answer = fx.ask("GET /pool.txt HTTP/1.1\r\nRange: bytes=6-\r\n\r\n");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 206 Partial Content");
    assert_eq!(
        Fixture::header(&answer, "content-range"),
        Some("bytes 6-10/11")
    );
    assert_eq!(Fixture::body(&answer), b"world");
    let answer = fx.ask("GET /pool.txt HTTP/1.1\r\nRange: bytes=0-4\r\n\r\n");
    assert_eq!(Fixture::body(&answer), b"hello");
}

#[test]
fn an_impossible_range_is_refused() {
    let fx = Fixture::start();
    let answer = fx.ask("GET /pool.txt HTTP/1.1\r\nRange: bytes=99-\r\n\r\n");
    assert_eq!(
        Fixture::status(&answer),
        "HTTP/1.1 416 Range Not Satisfiable"
    );
    assert_eq!(
        Fixture::header(&answer, "content-range"),
        Some("bytes */11")
    );
}

#[test]
fn a_missing_file_is_not_found() {
    let fx = Fixture::start();
    let answer = fx.ask("GET /pool/none.txt HTTP/1.1\r\n\r\n");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 404 Not Found");
}

#[test]
fn nothing_outside_the_root_is_served() {
    let fx = Fixture::start();
    for target in [
        "/../etc/passwd",
        "/%2e%2e/etc/passwd",
        "/..%2fetc%2fpasswd",
        "/pool.txt/../etc/passwd",
    ] {
        let answer = fx.ask(format!("GET {target} HTTP/1.1\r\n\r\n").as_str());
        assert_eq!(
            Fixture::status(&answer),
            "HTTP/1.1 403 Forbidden",
            "{target} escaped the root"
        );
    }
    let answer = fx.ask("GET /pool%2etxt HTTP/1.1\r\n\r\n");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 200 OK");
    assert_eq!(Fixture::body(&answer), b"hello world");
}

#[test]
fn methods_that_are_not_get_or_head_are_refused() {
    let fx = Fixture::start();
    let answer = fx.ask("PUT /pool.txt HTTP/1.1\r\n\r\nhello");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 405 Method Not Allowed");
    assert!(Fixture::body(&answer).is_empty());
}

#[test]
fn the_root_and_directories_are_not_served() {
    let fx = Fixture::start();
    for target in ["/", "/pool.txt/"] {
        let answer = fx.ask(format!("GET {target} HTTP/1.1\r\n\r\n").as_str());
        assert_ne!(
            Fixture::status(&answer),
            "HTTP/1.1 200 OK",
            "{target} was served"
        );
    }
}

#[test]
fn an_address_that_cannot_be_parsed_is_a_usage_error() {
    let dir = laft_test::tempdir();
    let error = match Serve::bind(dir.path(), Some("no-address")) {
        Ok(_) => panic!("a bad address is accepted"),
        Err(error) => error,
    };
    assert!(
        error.message.contains("not an address"),
        "{}",
        error.message
    );
}
