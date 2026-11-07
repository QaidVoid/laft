//! A read-only HTTP file server for the local repository, behind
//! `laft repo serve`.
//!
//! The server answers `GET` and `HEAD` for regular files under the
//! repository directory and nothing else: it accepts no uploads, lists no
//! directories, follows no paths outside the root and no symlink under
//! it. A client cannot make it buffer without bound: lines are read with
//! a fixed cap, the headers are counted, connections pause at most half
//! a minute between bytes in each direction, and at most sixty-four are
//! served at the same time, at most eight of them from one peer address.
//! One connection may take at most ten minutes in total, so a client that
//! trickles bytes cannot hold a slot for good. Files are streamed from an
//! open file, never read into memory whole. Partial downloads resume,
//! because every range request of the form `bytes=<n>-` or `bytes=-<n>`
//! is answered. Each connection serves one request, and every response is
//! sent with `Connection: close`, which every client of a repository
//! understands.

use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::failure::{Failure, exit};

/// The address `laft repo serve` listens on without `--listen`.
const DEFAULT_LISTEN: &str = "127.0.0.1:7979";

/// The largest request line or header the server reads.
const MAX_HEADER: usize = 8 * 1024;

/// The most header lines one request may hold.
const MAX_HEADERS: usize = 64;

/// The most connections the server serves at the same time.
const MAX_CONNECTIONS: usize = 64;

/// How long one connection may pause between bytes, in each direction.
const TIMEOUT: Duration = Duration::from_secs(30);

/// How long one connection may take in total, including the body.
const DEADLINE: Duration = Duration::from_secs(600);

/// The most connections one peer address may hold at the same time.
const MAX_PER_PEER: usize = 8;

/// The most bytes one copy between the file and the socket takes at once.
const COPY_CHUNK: usize = 64 * 1024;

/// The read-only file server of a local repository.
pub(crate) struct Serve {
    root: PathBuf,
    listener: TcpListener,
}

impl Serve {
    /// Binds `listen`, or the default address, to serve `root`. Port `0`
    /// picks a free port, which the tests use.
    ///
    /// # Errors
    ///
    /// Fails when the address cannot be parsed or bound.
    pub(crate) fn bind(root: &Path, listen: Option<&str>) -> Result<Self, Failure> {
        let address = listen.unwrap_or(DEFAULT_LISTEN);
        let addr: SocketAddr = address.parse().map_err(|e| {
            Failure::new(
                exit::USAGE,
                format!("--listen {address} is not an address like 127.0.0.1:7979: {e}"),
            )
        })?;
        let listener = TcpListener::bind(addr)
            .map_err(|e| Failure::new(exit::FAILURE, format!("{addr}: {e}")))?;
        Ok(Self {
            root: root.to_path_buf(),
            listener,
        })
    }

    /// The address the server listens on.
    pub(crate) fn addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("a bound listener has an address")
    }

    /// Answers requests until the process is stopped. Unless `quiet`, every
    /// request is reported on standard error. At most [`MAX_CONNECTIONS`]
    /// connections are served at the same time, at most
    /// [`MAX_PER_PEER`] from one peer address; the rest are closed
    /// without an answer.
    pub(crate) fn run(self, quiet: bool) {
        let Self { root, listener } = self;
        let root = Arc::from(root);
        let live = Arc::new(AtomicUsize::new(0));
        let peers: Arc<Mutex<HashMap<std::net::IpAddr, usize>>> =
            Arc::new(Mutex::new(HashMap::new()));
        for stream in listener.incoming().flatten() {
            let peer = stream.peer_addr().ok().map(|addr| addr.ip());
            let mut counts = peers.lock().expect("the peer counts cannot be poisoned");
            let busy = live.load(Ordering::Relaxed) >= MAX_CONNECTIONS
                || peer.is_some_and(|peer| {
                    counts.get(&peer).is_some_and(|held| *held >= MAX_PER_PEER)
                });
            if busy {
                continue;
            }
            live.fetch_add(1, Ordering::Relaxed);
            if let Some(peer) = peer {
                *counts.entry(peer).or_default() += 1;
            }
            drop(counts);
            let live = Arc::clone(&live);
            let peers = Arc::clone(&peers);
            let _ = stream.set_read_timeout(Some(TIMEOUT));
            let _ = stream.set_write_timeout(Some(TIMEOUT));
            let _ = stream.set_nodelay(true);
            let root = Arc::clone(&root);
            thread::spawn(move || {
                let line = handle(stream, &root, &Deadline::start());
                live.fetch_sub(1, Ordering::Relaxed);
                if let Some(peer) = peer {
                    let mut counts = peers.lock().expect("the peer counts cannot be poisoned");
                    if let Some(held) = counts.get_mut(&peer) {
                        *held = held.saturating_sub(1);
                        if *held == 0 {
                            counts.remove(&peer);
                        }
                    }
                }
                if !quiet {
                    let _ = writeln!(std::io::stderr(), "{line}");
                }
            });
        }
    }
}

/// The total time one connection may take.
struct Deadline {
    at: Instant,
}

impl Deadline {
    /// The deadline of a connection that starts now.
    fn start() -> Self {
        Self {
            at: Instant::now() + DEADLINE,
        }
    }

    /// A deadline that is already over.
    #[cfg(test)]
    fn over() -> Self {
        Self { at: Instant::now() }
    }

    /// The longest wait that fits the deadline and the between-bytes cap.
    fn timeout(&self) -> Duration {
        self.at
            .saturating_duration_since(Instant::now())
            .min(TIMEOUT)
    }

    /// Whether the connection took too long and must be dropped.
    fn expired(&self) -> bool {
        Instant::now() >= self.at
    }
}

/// Applies the shorter of the between-bytes timeout and what is left of
/// the deadline to `stream`, in both directions.
fn arm(stream: &TcpStream, deadline: &Deadline) {
    let _ = stream.set_read_timeout(Some(deadline.timeout()));
    let _ = stream.set_write_timeout(Some(deadline.timeout()));
}

/// Answers one request on `stream` and returns the log line for it.
fn handle(stream: TcpStream, root: &Path, deadline: &Deadline) -> String {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_owned());
    if deadline.expired() {
        return format!("{peer} 400 Bad Request");
    }
    let mut reader = BufReader::new(stream);
    let line = match request(&mut reader, root, deadline) {
        Ok(line) => line,
        Err(status) => {
            // A connection that took too long is only closed; a truncated
            // body is what a dropped connection looks like anyway.
            if !deadline.expired() {
                let mut stream = reader.into_inner();
                arm(&stream, deadline);
                let _ = respond(&mut stream, status, &[], false);
            }
            format!("{} {}", status.code(), status.reason())
        }
    };
    format!("{peer} {line}")
}

/// Reads one line of at most [`MAX_HEADER`] bytes, so that a client
/// cannot make the server buffer without bound.
fn read_line(reader: &mut BufReader<TcpStream>, deadline: &Deadline) -> Result<String, Status> {
    if deadline.expired() {
        return Err(Status::BadRequest);
    }
    arm(reader.get_ref(), deadline);
    let mut buf = Vec::new();
    (&mut *reader)
        .take(MAX_HEADER as u64 + 1)
        .read_until(b'\n', &mut buf)
        .map_err(|_| Status::BadRequest)?;
    if buf.len() > MAX_HEADER || buf.last() != Some(&b'\n') {
        return Err(Status::BadRequest);
    }
    String::from_utf8(buf).map_err(|_| Status::BadRequest)
}

/// The request target as it may be logged: every control character is
/// escaped, so that a client cannot forge log lines.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            out.push_str(&format!("\\x{:02x}", character as u32));
        } else {
            out.push(character);
        }
    }
    out
}

/// Reads one request, serves it and returns its log line.
fn request(
    reader: &mut BufReader<TcpStream>,
    root: &Path,
    deadline: &Deadline,
) -> Result<String, Status> {
    let line = read_line(reader, deadline)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    if method.is_empty() || target.is_empty() {
        return Err(Status::BadRequest);
    }
    let mut range = None;
    let mut ended = false;
    for _ in 0..MAX_HEADERS {
        let header = read_line(reader, deadline)?;
        let trimmed = header.trim();
        if trimmed.is_empty() {
            ended = true;
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_owned());
        }
    }
    if !ended {
        return Err(Status::BadRequest);
    }
    let head = method == "HEAD";
    if !head && method != "GET" {
        return Err(Status::MethodNotAllowed);
    }
    let path = resolve(root, &target)?;
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Status::NotFound),
        Err(_) => return Err(Status::Forbidden),
    };
    let len = file.metadata().map_err(|_| Status::Forbidden)?.len();
    let stream = reader.get_mut();
    match range {
        None => {
            send_header(stream, Status::Ok, "", len, head, deadline)?;
            if !head {
                copy_bounded(&mut file, stream, len, deadline)?;
            }
            Ok(format!("{} 200 {}", clean(&target), len))
        }
        Some(range) => match slice(len, &range) {
            Some((first, last)) => {
                let extra = format!("Content-Range: bytes {first}-{last}/{len}\r\n");
                send_header(
                    stream,
                    Status::PartialContent,
                    &extra,
                    last - first + 1,
                    head,
                    deadline,
                )?;
                if !head {
                    file.seek(SeekFrom::Start(first))
                        .map_err(|_| Status::BadRequest)?;
                    copy_bounded(&mut file, stream, last - first + 1, deadline)?;
                }
                Ok(format!("{} 206 {first}-{last}", clean(&target)))
            }
            None => {
                let extra = format!("Content-Range: bytes */{len}\r\n");
                respond_with(stream, Status::RangeNotSatisfiable, &extra, &[], head)?;
                Ok(format!("{} 416 {}", clean(&target), clean(&range)))
            }
        },
    }
}

/// The file under `root` that `target` names, or an error status. Percent
/// escapes are decoded, a target that leaves `root` is refused, and every
/// component must be a directory or a regular file of its own, so no
/// symlink under the tree is ever followed.
fn resolve(root: &Path, target: &str) -> Result<PathBuf, Status> {
    let path = target.split('?').next().unwrap_or(target);
    let path = percent_decode(path);
    let path = path.trim_start_matches('/');
    if path.is_empty() || path.ends_with('/') {
        return Err(Status::Forbidden);
    }
    let mut file = root.to_path_buf();
    for part in Path::new(path).components() {
        match part {
            Component::Normal(part) => {
                file.push(part);
                let meta = file.symlink_metadata().map_err(|_| Status::NotFound)?;
                if meta.file_type().is_symlink() {
                    return Err(Status::Forbidden);
                }
            }
            Component::CurDir => {}
            _ => return Err(Status::Forbidden),
        }
    }
    match fs::symlink_metadata(&file) {
        Ok(meta) if meta.is_file() => Ok(file),
        Ok(_) => Err(Status::Forbidden),
        Err(_) => Err(Status::NotFound),
    }
}

/// Decodes every `%XX` escape of `text`, on bytes, so that an escape that
/// runs into a multi-byte character is literal text instead of a slice
/// that does not exist.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let (high, low) = match (bytes.get(i + 1), bytes.get(i + 2)) {
            (Some(&high), Some(&low)) if bytes[i] == b'%' => (hex(high), hex(low)),
            _ => (None, None),
        };
        match (high, low) {
            (Some(high), Some(low)) => {
                out.push(high * 16 + low);
                i += 3;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The value of one hex digit.
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The first and last byte of the slice that `range` names within `len`
/// bytes, such as `bytes=2-`, `bytes=2-7` or `bytes=-5` for the last five
/// bytes.
fn slice(len: u64, range: &str) -> Option<(u64, u64)> {
    let spec = range.trim().strip_prefix("bytes=")?.trim();
    let (first, last) = spec.split_once('-')?;
    if first.trim().is_empty() {
        let count: u64 = last.trim().parse().ok()?;
        if count == 0 || len == 0 {
            return None;
        }
        let count = count.min(len);
        return Some((len - count, len - 1));
    }
    let first: u64 = first.trim().parse().ok()?;
    let last = if last.trim().is_empty() {
        len.checked_sub(1)?
    } else {
        last.trim().parse::<u64>().ok()?.min(len.checked_sub(1)?)
    };
    if first > last {
        return None;
    }
    Some((first, last))
}

/// Sends the header of `status` with a body of `length` bytes. The body
/// itself is sent by the caller, from the file it has open.
fn send_header(
    stream: &mut TcpStream,
    status: Status,
    extra: &str,
    length: u64,
    head: bool,
    deadline: &Deadline,
) -> Result<(), Status> {
    arm(stream, deadline);
    let header = format!(
        "HTTP/1.1 {} {}\r\n{}Content-Type: application/octet-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        status.code(),
        status.reason(),
        extra,
        length,
    );
    stream
        .write_all(header.as_bytes())
        .map_err(|_| Status::BadRequest)?;
    if head {
        stream.flush().map_err(|_| Status::BadRequest)?;
    }
    Ok(())
}

/// Copies exactly `count` bytes of `file` to `stream`, in bounded chunks
/// and only while the deadline holds, so a client that stops reading
/// cannot hold the connection for good.
fn copy_bounded(
    file: &mut File,
    stream: &mut TcpStream,
    count: u64,
    deadline: &Deadline,
) -> Result<(), Status> {
    let mut left = file.take(count);
    let mut buf = vec![0u8; COPY_CHUNK];
    loop {
        if deadline.expired() {
            return Err(Status::BadRequest);
        }
        arm(stream, deadline);
        let read = left.read(&mut buf).map_err(|_| Status::BadRequest)?;
        if read == 0 {
            break;
        }
        stream
            .write_all(&buf[..read])
            .map_err(|_| Status::BadRequest)?;
    }
    stream.flush().map_err(|_| Status::BadRequest)
}

/// Sends `bytes` as the body of `status`.
fn respond(stream: &mut TcpStream, status: Status, bytes: &[u8], head: bool) -> Result<(), Status> {
    respond_with(stream, status, "", bytes, head)
}
/// Sends `bytes` as the body of `status`, with `extra` between the status
/// line and the fixed headers.
fn respond_with(
    stream: &mut TcpStream,
    status: Status,
    extra: &str,
    bytes: &[u8],
    head: bool,
) -> Result<(), Status> {
    let header = format!(
        "HTTP/1.1 {} {}\r\n{}Content-Type: application/octet-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        status.code(),
        status.reason(),
        extra,
        bytes.len(),
    );
    stream
        .write_all(header.as_bytes())
        .map_err(|_| Status::BadRequest)?;
    if !head {
        stream.write_all(bytes).map_err(|_| Status::BadRequest)?;
    }
    stream.flush().map_err(|_| Status::BadRequest)?;
    Ok(())
}

/// The status codes the server answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    PartialContent,
    BadRequest,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    RangeNotSatisfiable,
}

impl Status {
    /// The number of the status.
    fn code(self) -> u16 {
        match self {
            Status::Ok => 200,
            Status::PartialContent => 206,
            Status::BadRequest => 400,
            Status::Forbidden => 403,
            Status::NotFound => 404,
            Status::MethodNotAllowed => 405,
            Status::RangeNotSatisfiable => 416,
        }
    }

    /// The reason phrase of the status.
    fn reason(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::PartialContent => "Partial Content",
            Status::BadRequest => "Bad Request",
            Status::Forbidden => "Forbidden",
            Status::NotFound => "Not Found",
            Status::MethodNotAllowed => "Method Not Allowed",
            Status::RangeNotSatisfiable => "Range Not Satisfiable",
        }
    }
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
