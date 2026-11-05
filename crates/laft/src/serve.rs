//! A read-only HTTP file server for the local repository, behind
//! `laft repo serve`.
//!
//! The server answers `GET` and `HEAD` for the files under the repository
//! directory and nothing else: it accepts no uploads, lists no directories
//! and follows no paths outside the root. Partial downloads resume, because
//! every range request of the form `bytes=<n>-` is answered with the rest
//! of the file. Each connection serves one request, and every response is
//! sent with `Connection: close`, which every client of a repository
//! understands.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::thread;

use crate::failure::{Failure, exit};

/// The address `laft repo serve` listens on without `--listen`.
const DEFAULT_LISTEN: &str = "127.0.0.1:7979";

/// The largest request line or header the server reads.
const MAX_HEADER: usize = 8 * 1024;

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
    /// request is reported on standard error.
    pub(crate) fn run(self, quiet: bool) {
        let Self { root, listener } = self;
        let root = std::sync::Arc::from(root);
        for stream in listener.incoming().flatten() {
            let root = std::sync::Arc::clone(&root);
            thread::spawn(move || {
                let line = handle(stream, &root);
                if !quiet {
                    eprintln!("{line}");
                }
            });
        }
    }
}

/// Answers one request on `stream` and returns the log line for it.
fn handle(stream: TcpStream, root: &Path) -> String {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_owned());
    let mut reader = BufReader::new(stream);
    let line = match request(&mut reader, root) {
        Ok(line) => line,
        Err(status) => {
            let mut stream = reader.into_inner();
            let _ = respond(&mut stream, status, &[], false);
            format!("{} {}", status.code(), status.reason())
        }
    };
    format!("{peer} {line}")
}

/// Reads one request, serves it and returns its log line.
fn request(reader: &mut BufReader<TcpStream>, root: &Path) -> Result<String, Status> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|_| Status::BadRequest)?;
    if line.len() > MAX_HEADER {
        return Err(Status::BadRequest);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    if method.is_empty() || target.is_empty() {
        return Err(Status::BadRequest);
    }
    let mut range = None;
    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .map_err(|_| Status::BadRequest)?;
        if header.len() > MAX_HEADER {
            return Err(Status::BadRequest);
        }
        let trimmed = header.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_owned());
        }
    }
    let head = method.eq_ignore_ascii_case("HEAD");
    if !head && !method.eq_ignore_ascii_case("GET") {
        return Err(Status::MethodNotAllowed);
    }
    let path = resolve(root, &target)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Status::NotFound),
        Err(_) => return Err(Status::Forbidden),
    };
    let stream = reader.get_mut();
    match range {
        None => {
            respond(stream, Status::Ok, &bytes, head)?;
            Ok(format!("{target} 200 {}", bytes.len()))
        }
        Some(range) => match slice(&bytes, &range) {
            Some((first, last)) => {
                let body = &bytes[first..=last];
                let extra = format!("Content-Range: bytes {first}-{last}/{}\r\n", bytes.len());
                respond_with(stream, Status::PartialContent, &extra, body, head)?;
                Ok(format!("{target} 206 {first}-{last}"))
            }
            None => {
                let extra = format!("Content-Range: bytes */{}\r\n", bytes.len());
                respond_with(stream, Status::RangeNotSatisfiable, &extra, &[], head)?;
                Ok(format!("{target} 416 {range}"))
            }
        },
    }
}

/// The file under `root` that `target` names, or an error status. Percent
/// escapes are decoded, and a target that leaves `root` is refused.
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
            Component::Normal(part) => file.push(part),
            Component::CurDir => {}
            _ => return Err(Status::Forbidden),
        }
    }
    Ok(file)
}

/// Decodes every `%XX` escape of `text`.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(value) = u8::from_str_radix(&text[i + 1..i + 3], 16).ok()
        {
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The first and last byte of the slice that `range` names, such as
/// `bytes=2-` or `bytes=2-7`.
fn slice(bytes: &[u8], range: &str) -> Option<(usize, usize)> {
    let spec = range.trim().strip_prefix("bytes=")?.trim();
    let (first, last) = spec.split_once('-')?;
    let first: usize = first.trim().parse().ok()?;
    let last = if last.trim().is_empty() {
        bytes.len().checked_sub(1)?
    } else {
        last.trim()
            .parse::<usize>()
            .ok()?
            .min(bytes.len().checked_sub(1)?)
    };
    if first > last {
        return None;
    }
    Some((first, last))
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
