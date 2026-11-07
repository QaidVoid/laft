use super::*;

use std::io::{Read, Write};
use std::net::TcpStream;

/// A running server over a temporary repository, its address and the
/// directory that must outlive it.
struct Fixture {
    addr: std::net::SocketAddr,
    root: std::path::PathBuf,
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
        Self {
            addr,
            root,
            _dir: dir,
        }
    }

    /// Sends a raw request and reads the whole answer.
    fn ask(&self, request: &str) -> Vec<u8> {
        self.try_ask(request).expect("the connection closes")
    }

    /// Sends a raw request and reads the answer, or `None` when the
    /// connection is reset before an answer.
    fn try_ask(&self, request: &str) -> Option<Vec<u8>> {
        let mut stream = TcpStream::connect(self.addr).expect("the server is listening");
        stream.write_all(request.as_bytes()).expect("sent");
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).ok().map(|_| answer)
    }

    /// The status line of an answer, from its header alone.
    fn status(answer: &[u8]) -> String {
        let end = answer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap_or(answer.len());
        let text = std::str::from_utf8(&answer[..end]).expect("the header is text");
        text.lines().next().expect("a status line").to_owned()
    }

    /// The value of a header of an answer.
    fn header<'a>(answer: &'a [u8], name: &str) -> Option<&'a str> {
        let end = answer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap_or(answer.len());
        let text = std::str::from_utf8(&answer[..end]).expect("the header is text");
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
fn methods_are_case_sensitive() {
    let fx = Fixture::start();
    let answer = fx.ask("get /pool.txt HTTP/1.1\r\n\r\n");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 405 Method Not Allowed");
}

#[test]
fn the_last_bytes_of_a_file_are_served_for_a_suffix_range() {
    let fx = Fixture::start();
    let answer = fx.ask("GET /pool.txt HTTP/1.1\r\nRange: bytes=-5\r\n\r\n");
    assert_eq!(
        Fixture::status(&answer),
        "HTTP/1.1 206 Partial Content",
        "{}",
        String::from_utf8_lossy(&answer)
    );
    assert_eq!(
        Fixture::header(&answer, "content-range"),
        Some("bytes 6-10/11")
    );
    assert_eq!(Fixture::body(&answer), b"world");
    let answer = fx.ask("GET /pool.txt HTTP/1.1\r\nRange: bytes=-0\r\n\r\n");
    assert_eq!(
        Fixture::status(&answer),
        "HTTP/1.1 416 Range Not Satisfiable"
    );
}

#[test]
fn only_regular_files_are_served() {
    let fx = Fixture::start();
    std::os::unix::fs::symlink("/etc/hostname", fx.root.join("outside")).expect("linked");
    std::os::unix::fs::symlink("pool.txt", fx.root.join("inside")).expect("linked");
    for target in ["/outside", "/inside"] {
        let answer = fx.ask(format!("GET {target} HTTP/1.1\r\n\r\n").as_str());
        assert_ne!(
            Fixture::status(&answer),
            "HTTP/1.1 200 OK",
            "{target} is a symlink and is not served"
        );
    }
}

#[test]
fn too_many_header_lines_are_refused() {
    let fx = Fixture::start();
    let headers: String = (0..100).map(|i| format!("X-{i}: y\r\n")).collect();
    let answer = fx.ask(format!("GET /pool.txt HTTP/1.1\r\n{headers}\r\n").as_str());
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 400 Bad Request");
}

#[test]
fn a_line_that_is_too_long_is_refused() {
    let fx = Fixture::start();
    let long = format!("X: {}a\r\n", "a".repeat(MAX_HEADER));
    let answer = fx.ask(format!("GET /pool.txt HTTP/1.1\r\n{long}\r\n").as_str());
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 400 Bad Request");
    let mut stream = TcpStream::connect(fx.addr).expect("the server is listening");
    stream.write_all(b"GET /").expect("sent");
    let rest = vec![b'a'; MAX_HEADER + 1];
    stream.write_all(&rest).expect("sent");
    stream.shutdown(std::net::Shutdown::Write).expect("closed");
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .expect("the connection closes");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 400 Bad Request");
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
fn a_broken_percent_escape_does_not_crash_the_server() {
    let fx = Fixture::start();
    let answer = fx.ask("GET /%aé HTTP/1.1\r\n\r\n");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 404 Not Found");
}

#[test]
fn percent_escapes_decode_on_bytes() {
    assert_eq!(percent_decode("/pool%2etxt"), "/pool.txt");
    assert_eq!(percent_decode("/%C3%A9"), "/\u{e9}");
    assert_eq!(percent_decode("/%a"), "/%a");
    assert_eq!(percent_decode("/%aé"), "/%a\u{e9}");
    assert_eq!(percent_decode("/%zz"), "/%zz");
    assert_eq!(percent_decode("/%2"), "/%2");
}

#[test]
fn one_peer_cannot_hold_the_server() {
    let fx = Fixture::start();
    let mut idle = Vec::new();
    for _ in 0..MAX_PER_PEER {
        idle.push(TcpStream::connect(fx.addr).expect("connected"));
    }
    // The idle connections sent nothing, so they hold their slots. The
    // next connection of the same peer is not served.
    let answer = fx.try_ask("GET /pool.txt HTTP/1.1\r\n\r\n");
    assert!(
        answer.as_ref().is_none_or(|answer| answer.is_empty()),
        "the connection over the per-peer limit is not served: {}",
        answer
            .map(|answer| String::from_utf8_lossy(&answer).into_owned())
            .unwrap_or_default()
    );
}

/// A deadline that is over stops the header read: a client that trickles
/// bytes cannot hold its slot past the deadline.
#[test]
fn an_over_deadline_stops_the_header_read() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bound");
    let addr = listener.local_addr().expect("an address");
    let _client = TcpStream::connect(addr).expect("connected");
    let (stream, _) = listener.accept().expect("accepted");
    let mut reader = BufReader::new(stream);
    let started = Instant::now();
    let error = read_line(&mut reader, &Deadline::over()).expect_err("the deadline is over");
    assert_eq!(error, Status::BadRequest);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the read returns at once, not after the between-bytes timeout"
    );
}

#[test]
fn a_connection_past_its_deadline_is_dropped() {
    let dir = laft_test::tempdir();
    let root = dir.path().join("laft/local");
    std::fs::create_dir_all(&root).expect("writable");
    std::fs::write(root.join("pool.txt"), b"hello world").expect("writable");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bound");
    let addr = listener.local_addr().expect("an address");
    let mut client = TcpStream::connect(addr).expect("connected");
    let (server, _) = listener.accept().expect("accepted");
    let root_for_thread = root.clone();
    thread::spawn(move || {
        handle(server, &root_for_thread, &Deadline::over());
    });
    client
        .write_all(b"GET /pool.txt HTTP/1.1\r\n\r\n")
        .expect("sent");
    let mut answer = Vec::new();
    match client.read_to_end(&mut answer) {
        // The server closes without reading, so the request it never read
        // may also reset the connection.
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("the connection closes: {e}"),
    }
    assert!(answer.is_empty(), "nothing is served past the deadline");
}

#[test]
fn a_large_file_is_served_whole_and_in_ranges() {
    let fx = Fixture::start();
    let mut bytes = Vec::new();
    for i in 0..(192 * 1024) {
        bytes.push((i % 251) as u8);
    }
    std::fs::write(fx.root.join("big.bin"), &bytes).expect("writable");
    let answer = fx.ask("GET /big.bin HTTP/1.1\r\n\r\n");
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 200 OK");
    assert_eq!(
        Fixture::header(&answer, "content-length"),
        Some(bytes.len().to_string().as_str())
    );
    assert_eq!(Fixture::body(&answer), bytes.as_slice());
    let first = 64 * 1024 + 17;
    let last = 128 * 1024 - 5;
    let answer =
        fx.ask(format!("GET /big.bin HTTP/1.1\r\nRange: bytes={first}-{last}\r\n\r\n").as_str());
    assert_eq!(Fixture::status(&answer), "HTTP/1.1 206 Partial Content");
    assert_eq!(Fixture::body(&answer), &bytes[first..=last]);
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
