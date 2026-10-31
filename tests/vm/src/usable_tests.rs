use super::*;

#[test]
fn git_fixture_paths_cannot_escape_the_server_root_or_inject_shell() {
    assert!(safe_repo_path("objects/ab/0123456789abcdef"));
    assert!(safe_repo_path("refs/heads/main"));
    for name in [
        "",
        "/HEAD",
        "../HEAD",
        "objects/../../key",
        "HEAD;id",
        "HEAD\n",
        "a//b",
        "a/./b",
    ] {
        assert!(!safe_repo_path(name), "{name:?}");
    }
}

#[test]
fn shell_quoting_preserves_spaces_and_single_quotes() {
    let value = "fixture's executable path";
    let output = Command::new("/bin/sh")
        .args(["-c", &format!("printf %s {}", quote(value))])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, value.as_bytes());
}

fn http(request: &[u8]) -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        serve_http(&mut stream).unwrap();
    });
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(request).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    thread.join().unwrap();
    response
}

#[test]
fn http_fixture_requires_cookie_replay_and_returns_deterministic_content() {
    let response = http(b"GET /payload HTTP/1.1\r\nHost: fixture\r\n\r\n");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Set-Cookie: fixture=present; Path=/\r\n"));
    assert!(response.ends_with(PAYLOAD));
    assert!(http(b"GET /cookie HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 403"));
    assert!(http(b"GET /cookie HTTP/1.1\r\nCookie: fixture=present\r\n\r\n").ends_with(PAYLOAD));
    assert!(http(b"GET /other HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 403"));
}

#[test]
fn guest_commands_are_posix_shell() {
    for script in [NETWORK_CHECK, TOOLS, TRANSFER, INSTALL] {
        assert!(laft_test::bashisms(script).is_empty());
        assert!(
            Command::new("/bin/sh")
                .args(["-n", "-c", script])
                .status()
                .unwrap()
                .success()
        );
    }
}

#[test]
fn fixture_certificates_require_the_generated_ca_and_correct_address() {
    for program in ["openssl", "ssh-keygen"] {
        if Command::new(program)
            .arg("-help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping local TLS fixture test: {program} is unavailable");
            return;
        }
    }
    let temp = laft_test::tempdir();
    let paths = Paths {
        source: temp.path().into(),
        bin: temp.path().into(),
        cache: temp.path().into(),
        work: temp.path().into(),
    };
    let fixture = Fixture::new(&paths).unwrap();
    let netdev = fixture.netdev().unwrap();
    assert!(netdev.contains("restrict=on,ipv6=off"));
    assert!(netdev.contains("hostfwd=tcp:127.0.0.1:"));
    assert!(netdev.contains("guestfwd=tcp:10.0.2.100:443-cmd:"));
    for (ca, address, expected) in [
        ("ca.pem", "10.0.2.100", true),
        ("untrusted.pem", "10.0.2.100", false),
        ("ca.pem", "10.0.2.101", false),
    ] {
        let output = fixture
            .command("openssl")
            .args(["verify", "-CAfile", ca, "-verify_ip", address, "server.pem"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            expected,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut client = fixture.command("openssl");
    let mut child = client
        .args([
            "s_client",
            "-quiet",
            "-verify_return_error",
            "-verify_ip",
            "10.0.2.100",
            "-CAfile",
            "ca.pem",
            "-connect",
        ])
        .arg(format!("127.0.0.1:{}", fixture.tls_port))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"GET /payload HTTP/1.0\r\n\r\n")
        .unwrap();
    let response = child.wait_with_output().unwrap();
    assert!(
        response.status.success(),
        "{}",
        String::from_utf8_lossy(&response.stderr)
    );
    assert!(
        response.stdout.ends_with(PAYLOAD.as_bytes()),
        "{}",
        String::from_utf8_lossy(&response.stdout)
    );
    let config = include_str!("../system-usable.stav").replace("\"@REPO_KEY@\"", "\"test-key\"");
    assert_eq!(config.matches("@REPO_KEY@").count(), 1);
    let directory = fixture.dir.clone();
    drop(fixture);
    assert!(!directory.exists());
}

#[test]
fn root_rejection_uses_an_otherwise_authorized_key() {
    let config = include_str!("../system-usable.stav");
    let root = config
        .split("user root {")
        .nth(1)
        .expect("root account")
        .split('}')
        .next()
        .expect("root body");
    assert!(root.contains("ssh_keys = [\"@SSH_KEY@\"]"));
}

#[test]
fn fixture_copy_failures_set_the_reported_install_status() {
    let temp = laft_test::tempdir();
    fs::create_dir(temp.path().join("fixture")).unwrap();
    let install = INSTALL
        .replace("/fixture", "\"${TEST_ROOT}/fixture\"")
        .replace("/mnt", "\"${TEST_ROOT}/mnt\"");
    let script = format!(
        "mkdir() {{ return 1; }}\nstatus=0\n{install}\nprintf 'install-status=%s' \"$status\""
    );
    let output = Command::new("/bin/sh")
        .args(["-c", &script])
        .env("TEST_ROOT", temp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"install-status=1");
}

#[test]
fn hung_fixture_clients_are_killed_and_reaped() {
    let started = Instant::now();
    let error = output_timeout(
        Command::new("/bin/sleep").arg("10"),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert!(error.contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(2));
}
