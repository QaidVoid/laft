//! Local network fixtures and interactive acceptance for the self-built tools.

use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::cpio::{Archive, Kind};
use crate::qemu::Machine;
use crate::{Paths, Result, check, fail, shell};

const PAYLOAD: &str = "laft-vmtest: local fixture\n";

/// Disposable credentials and loopback-only fixture processes for one run.
pub(crate) struct Fixture {
    dir: PathBuf,
    tls: Option<Child>,
    http: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    ssh_port: u16,
    http_port: u16,
    tls_port: u16,
}

impl Fixture {
    /// Creates fresh keys, certificates and local HTTP and HTTPS fixtures.
    ///
    /// # Errors
    ///
    /// Fails if OpenSSL, OpenSSH, a socket or a fixture file is unavailable.
    pub(crate) fn new(paths: &Paths) -> Result<Self> {
        let dir = paths.work.join("usable-fixture");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).map_err(|e| fail(format!("creating fixture: {e}")))?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        let mut fixture = Self {
            dir,
            tls: None,
            http: None,
            stop: Arc::new(AtomicBool::new(false)),
            ssh_port: unused_port()?,
            http_port: 0,
            tls_port: unused_port()?,
        };
        fixture.generate()?;
        let www = fixture.dir.join("www");
        fs::create_dir(&www).map_err(|e| e.to_string())?;
        fs::write(www.join("payload"), PAYLOAD).map_err(|e| e.to_string())?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        fixture.http_port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::clone(&fixture.stop);
        fixture.http = Some(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = serve_http(&mut stream);
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
        }));
        let mut server = fixture.command("openssl");
        server
            .args([
                "s_server",
                "-quiet",
                "-WWW",
                "-http_server_binmode",
                "-accept",
            ])
            .arg(format!("127.0.0.1:{}", fixture.tls_port))
            .arg("-cert")
            .arg(fixture.dir.join("server.pem"))
            .arg("-key")
            .arg(fixture.dir.join("server.key"))
            .current_dir(&www)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        fixture.tls = Some(
            server
                .spawn()
                .map_err(|e| fail(format!("starting TLS fixture: {e}")))?,
        );
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if TcpStream::connect(("127.0.0.1", fixture.tls_port)).is_ok() {
                break;
            }
            if Instant::now() >= until {
                return Err(fail("TLS fixture did not start"));
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(fixture)
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.dir)
            .env("LC_ALL", "C")
            .env("OPENSSL_CONF", "/dev/null")
            .current_dir(&self.dir);
        command
    }

    fn generate(&self) -> Result<()> {
        for name in ["client", "host"] {
            success(self.command("ssh-keygen").args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "vm-fixture",
                "-f",
                name,
            ]))?;
        }
        for name in ["ca", "untrusted"] {
            success(
                self.command("openssl")
                    .args([
                        "req",
                        "-x509",
                        "-newkey",
                        "rsa:2048",
                        "-noenc",
                        "-days",
                        "2",
                        "-subj",
                        "/CN=VM Fixture CA",
                        "-addext",
                        "basicConstraints=critical,CA:TRUE",
                        "-addext",
                        "keyUsage=critical,keyCertSign,cRLSign",
                        "-keyout",
                    ])
                    .arg(format!("{name}.key"))
                    .arg("-out")
                    .arg(format!("{name}.pem")),
            )?;
        }
        success(self.command("openssl").args([
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-noenc",
            "-subj",
            "/CN=VM Fixture",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
        ]))?;
        fs::write(self.dir.join("extensions"),
            "subjectAltName=IP:10.0.2.100\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n")
            .map_err(|e| e.to_string())?;
        success(self.command("openssl").args([
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-set_serial",
            "2",
            "-days",
            "2",
            "-extfile",
            "extensions",
            "-out",
            "server.pem",
        ]))?;
        let host = fs::read_to_string(self.dir.join("host.pub")).map_err(|e| e.to_string())?;
        fs::write(self.dir.join("known_hosts"), format!("vm-fixture {host}"))
            .map_err(|e| e.to_string())
    }

    /// The QEMU user network. Only SSH ingress and two fixture TCP endpoints exist.
    ///
    /// # Errors
    ///
    /// Fails if the executable path cannot be represented in QEMU's option syntax.
    pub(crate) fn netdev(&self) -> Result<String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let executable = executable
            .to_str()
            .ok_or_else(|| fail("non-UTF-8 fixture executable"))?;
        if executable.contains([',', '\n', '\r']) {
            return Err(fail(
                "fixture executable path contains QEMU option separators",
            ));
        }
        Ok(format!(
            "user,id=usable,restrict=on,ipv6=off,net=10.0.2.0/24,host=10.0.2.2,dhcpstart=10.0.2.15,dns=10.0.2.3,hostfwd=tcp:127.0.0.1:{}-10.0.2.15:22,guestfwd=tcp:10.0.2.100:80-cmd:{} --fixture-relay {},guestfwd=tcp:10.0.2.100:443-cmd:{} --fixture-relay {}",
            self.ssh_port,
            quote(executable),
            self.http_port,
            quote(executable),
            self.tls_port,
        ))
    }

    /// Copies disposable guest material into the installer archive.
    ///
    /// # Errors
    ///
    /// Fails if a generated credential cannot be read.
    pub(crate) fn add_archive(&self, archive: &mut Archive) -> Result<()> {
        archive.add("fixture", 0o700, Kind::Dir);
        for name in ["client", "host", "host.pub", "ca.pem", "untrusted.pem"] {
            let bytes = fs::read(self.dir.join(name)).map_err(|e| e.to_string())?;
            archive.add(&format!("fixture/{name}"), 0o600, Kind::File(bytes));
        }
        Ok(())
    }

    /// The disposable public key declared for the non-root account.
    ///
    /// # Errors
    ///
    /// Fails if the generated key cannot be read.
    pub(crate) fn public_key(&self) -> Result<String> {
        fs::read_to_string(self.dir.join("client.pub"))
            .map(|s| s.trim().to_owned())
            .map_err(|e| e.to_string())
    }

    fn ssh(&self, user: &str, script: &str) -> Result<Output> {
        let mut command = self.command("ssh");
        command
            .args([
                "-F",
                "/dev/null",
                "-o",
                "BatchMode=yes",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "IdentityAgent=none",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "GlobalKnownHostsFile=/dev/null",
                "-o",
                "UpdateHostKeys=no",
                "-o",
                "HostKeyAlias=vm-fixture",
                "-o",
                "LogLevel=ERROR",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "ConnectionAttempts=1",
                "-o",
                "ClearAllForwardings=yes",
            ])
            .arg("-o")
            .arg(format!(
                "UserKnownHostsFile={}",
                self.dir.join("known_hosts").display()
            ))
            .arg("-i")
            .arg(self.dir.join("client"))
            .arg("-p")
            .arg(self.ssh_port.to_string())
            .arg(format!("{user}@127.0.0.1"))
            .arg(script);
        output_timeout(&mut command, Duration::from_secs(90))
    }

    /// Checks DHCP and authenticated non-root SSH, also after a reboot.
    ///
    /// # Errors
    ///
    /// Fails if network state or SSH authentication is incorrect.
    pub(crate) fn recheck(&self, machine: &mut Machine) -> Result<()> {
        let network = shell(machine, NETWORK_CHECK)?;
        check(
            "isolated DHCP address, subnet route and resolver",
            &network,
            "network-status=0",
        )?;
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            let output = self.ssh(
                "vmtest",
                "test \"$(id -u)\" = 1000 && printf 'nonroot-ssh-ok\\n'",
            )?;
            if output.status.success() {
                check(
                    "non-root SSH login",
                    &String::from_utf8_lossy(&output.stdout),
                    "nonroot-ssh-ok",
                )?;
                break;
            }
            if Instant::now() >= until {
                return Err(fail(format!(
                    "non-root SSH failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            thread::sleep(Duration::from_secs(1));
        }
        let root = self.ssh("root", "printf 'unexpected-root-login\\n'")?;
        if root.status.code() != Some(255)
            || !String::from_utf8_lossy(&root.stderr).contains("Permission denied")
        {
            return Err(fail("SSH did not reject root authentication"));
        }
        Ok(())
    }

    /// Exercises the editor, Git over SSH and HTTPS, and curl trust and cookies.
    ///
    /// # Errors
    ///
    /// Fails if an interactive tool or its transport does not work.
    pub(crate) fn tools(&self) -> Result<()> {
        self.script(TOOLS)?;
        // Git's dumb HTTPS transport serves the actual commit created in the guest.
        let files = self.ssh(
            "vmtest",
            "cd /home/vmtest/project/.git && find HEAD objects refs info -type f",
        )?;
        if !files.status.success() {
            return Err(fail("listing Git fixture files failed"));
        }
        for name in String::from_utf8_lossy(&files.stdout).lines() {
            if !safe_repo_path(name) {
                return Err(fail("invalid Git fixture path"));
            }
            let bytes = self.ssh("vmtest", &format!("cat /home/vmtest/project/.git/{name}"))?;
            if !bytes.status.success() {
                return Err(fail("reading Git fixture file failed"));
            }
            let path = self.dir.join("www/repo").join(name);
            fs::create_dir_all(
                path.parent()
                    .ok_or_else(|| fail("fixture path has no parent"))?,
            )
            .and_then(|()| fs::write(path, bytes.stdout))
            .map_err(|e| e.to_string())?;
        }
        self.script(TRANSFER)
    }

    fn script(&self, script: &str) -> Result<()> {
        let command = format!(
            "env -i HOME=/home/vmtest USER=vmtest LOGNAME=vmtest PATH=/usr/bin:/bin LC_ALL=C /usr/bin/sh -eu -c {}",
            quote(script)
        );
        let output = self.ssh("vmtest", &command)?;
        if !output.status.success() {
            return Err(fail(format!(
                "guest tool acceptance failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        eprintln!(
            "laft-vmtest: ok: {}",
            String::from_utf8_lossy(&output.stdout).trim()
        );
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.tls.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(thread) = self.http.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn output_timeout(command: &mut Command, timeout: Duration) -> Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| fail(format!("starting fixture client: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("client stdout is not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| fail("client stderr is not piped"))?;
    let read = |mut stream: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).map(|_| bytes)
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let until = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(fail("fixture client timed out or could not be waited for"));
            }
        }
    };
    let stdout = out
        .join()
        .map_err(|_| fail("fixture stdout reader failed"))?
        .map_err(|e| e.to_string())?;
    let stderr = err
        .join()
        .map_err(|_| fail("fixture stderr reader failed"))?
        .map_err(|e| e.to_string())?;
    Ok(Output {
        status: status?,
        stdout,
        stderr,
    })
}

fn success(command: &mut Command) -> Result<()> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| fail(format!("starting fixture tool: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(fail(format!(
            "fixture tool failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn unused_port() -> Result<u16> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn safe_repo_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        })
}

fn serve_http(stream: &mut TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::new();
    let mut byte = [0];
    while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            return Ok(());
        }
        request.push(byte[0]);
    }
    let request = String::from_utf8_lossy(&request);
    let cookie = request.lines().any(|l| {
        l.trim_end_matches('\r')
            .eq_ignore_ascii_case("Cookie: fixture=present")
    });
    let (status, body) = if request.starts_with("GET /payload HTTP/")
        || (request.starts_with("GET /cookie HTTP/") && cookie)
    {
        ("200 OK", PAYLOAD)
    } else {
        ("403 Forbidden", "fixture request rejected\n")
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nSet-Cookie: fixture=present; Path=/\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Relays one QEMU guestfwd connection to a loopback fixture socket.
///
/// # Errors
///
/// Fails if the loopback fixture or either stream is unavailable.
pub(crate) fn relay(port: u16) -> Result<()> {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    let mut upstream = socket.try_clone().map_err(|e| e.to_string())?;
    thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut upstream);
        let _ = upstream.shutdown(Shutdown::Write);
    });
    // Binary handshakes must not wait for stdout's line buffer to fill.
    let fd = std::io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|e| e.to_string())?;
    std::io::copy(&mut socket, &mut fs::File::from(fd))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// Restricted user networking omits the DHCP gateway and DNS options.
const NETWORK_CHECK: &str = r#"n=0; until ip -4 -o addr show dev eth0 | grep -q ' inet 10.0.2.15/24 ' && ip -4 route | grep -q '^10.0.2.0/24 dev eth0 ' && test -z "$(ip -4 route show default)" && test -f /etc/resolv.conf && ! grep -q '^nameserver ' /etc/resolv.conf; do n=$((n+1)); [ "$n" -lt 60 ] || break; sleep 1; done; ip -4 -o addr show dev eth0; ip -4 route; cat /etc/resolv.conf; test "$n" -lt 60; echo "network-status=$?""#;

const TOOLS: &str = r#"
ip -o link show up dev lo | grep -q '<LOOPBACK,'
cd "$HOME"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
test "$(id -u)" = 1000
mkdir -p .ssh
chmod 700 .ssh
printf '%s\n' 'editor persistence' > expected
vim -Nu NONE -n -i NONE -es -c 'call setline(1, "editor persistence")' -c wq editor.txt
vim -Nu NONE -n -i NONE -es editor.txt -c 'if getline(1) !=# "editor persistence" | cquit | endif' -c q
cmp expected editor.txt
git -c init.defaultBranch=main init -q project
cp editor.txt project/note.txt
git -C project add note.txt
git -C project -c user.name='VM Fixture' -c user.email=fixture@example.invalid -c commit.gpgsign=false commit -qm 'Create fixture'
git -C project update-server-info
export GIT_SSH_COMMAND='ssh -F /dev/null -i /home/vmtest/.ssh/id_ed25519 -o IdentityAgent=none -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=yes -o GlobalKnownHostsFile=/dev/null -o UserKnownHostsFile=/home/vmtest/.ssh/known_hosts'
git clone -q ssh://vmtest@127.0.0.1/home/vmtest/project ssh-clone
cmp editor.txt ssh-clone/note.txt
test "$(git -C project rev-parse HEAD)" = "$(git -C ssh-clone rev-parse HEAD)"
printf 'Vim write/reopen and Git commit/SSH clone passed\n'
"#;

const TRANSFER: &str = r#"
cd "$HOME"
curl -q --noproxy '*' --connect-timeout 5 --max-time 20 --fail --silent --show-error -c cookies http://10.0.2.100/payload > http.txt
curl -q --noproxy '*' --connect-timeout 5 --max-time 20 --fail --silent --show-error -b cookies http://10.0.2.100/cookie > cookie.txt
cmp http.txt cookie.txt
curl -q --noproxy '*' --connect-timeout 5 --max-time 20 --fail --silent --show-error --cacert ca.pem https://10.0.2.100/payload > https.txt
cmp http.txt https.txt
status=0
curl -q --noproxy '*' --connect-timeout 5 --max-time 20 --silent --show-error --cacert untrusted.pem https://10.0.2.100/payload > rejected.txt 2> rejected.log || status=$?
test "$status" = 60
test ! -s rejected.txt
status=0
curl -q --noproxy '*' --connect-timeout 5 --max-time 20 --silent --show-error https://10.0.2.100/payload > default-rejected.txt 2> default-rejected.log || status=$?
test "$status" = 60
test ! -s default-rejected.txt
curl -q --version | grep -q OpenSSL
curl -q --version | grep '^Features:' | grep -qw PSL
test -s /etc/ssl/certs/ca-certificates.crt
GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 GIT_SMART_HTTP=0 GIT_SSL_CAINFO="$HOME/ca.pem" git -c http.lowSpeedLimit=1 -c http.lowSpeedTime=15 clone -q https://10.0.2.100/repo https-clone
cmp editor.txt https-clone/note.txt
test "$(git -C project rev-parse HEAD)" = "$(git -C https-clone rev-parse HEAD)"
printf 'curl HTTP/cookies/PSL, trusted HTTPS, untrusted CA rejection and Git HTTPS clone passed\n'
"#;

/// Extra installer actions run only when disposable usable material exists.
pub(crate) const INSTALL: &str = r#"
if [ -d /fixture ]; then
  mkdir -p /mnt/etc/ssh /mnt/home/vmtest/.ssh &&
  cp /fixture/host /mnt/etc/ssh/ssh_host_ed25519_key &&
  cp /fixture/host.pub /mnt/etc/ssh/ssh_host_ed25519_key.pub &&
  chmod 600 /mnt/etc/ssh/ssh_host_ed25519_key &&
  cp /fixture/client /mnt/home/vmtest/.ssh/id_ed25519 &&
  { printf '127.0.0.1 '; cat /fixture/host.pub; } > /mnt/home/vmtest/.ssh/known_hosts &&
  cp /fixture/ca.pem /fixture/untrusted.pem /mnt/home/vmtest/ &&
  chmod 700 /mnt/home/vmtest/.ssh &&
  chmod 600 /mnt/home/vmtest/.ssh/id_ed25519 &&
  chown -R 1000:1000 /mnt/home/vmtest || status=1
fi
"#;

#[cfg(test)]
#[path = "usable_tests.rs"]
mod tests;
