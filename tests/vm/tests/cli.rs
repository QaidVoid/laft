//! Diagnostics and byte forwarding from the internal fixture entry point.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn fixture_relay_failures_explain_why_forwarding_failed() {
    let output = Command::new(env!("CARGO_BIN_EXE_laft-vmtest"))
        .args(["--fixture-relay", "0"])
        .output()
        .expect("start fixture relay");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture relay failed:"));
}

#[test]
fn fixture_relay_forwards_binary_replies_before_the_connection_closes() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("local listener");
    let port = listener.local_addr().expect("listener address").port();
    let (mut stdout, sink) = UnixStream::pair().expect("stdout socket pair");
    stdout
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("stdout timeout");
    let mut relay = Command::new(env!("CARGO_BIN_EXE_laft-vmtest"))
        .args(["--fixture-relay", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(sink)))
        .spawn()
        .expect("start fixture relay");
    let (mut server, _) = listener.accept().expect("relay connects");
    let packet = [0x16, 0x03, 0x03, 0x00, 0x01];
    server.write_all(&packet).expect("send binary reply");
    let mut bytes = [0; 5];
    let result = stdout.read_exact(&mut bytes);
    let _ = relay.kill();
    let _ = relay.wait();
    drop(server);
    result.expect("binary reply arrives while the server connection is open");
    assert_eq!(bytes, packet);
}
