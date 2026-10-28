//! Runs the built binary without the setuid bit. Everything past the
//! installation check needs root and a mounted generation, and is covered
//! by the virtual machine tests.

use std::process::Command;

#[test]
fn without_the_setuid_bit_it_refuses_to_run() {
    if rustix::process::geteuid().is_root() {
        println!("skipping: running as root, so the binary cannot tell it is not setuid");
        return;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_laft-enter"))
        .args(["dev", "true"])
        .env("LAFT_VIEW", "x")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "laft-enter: not installed setuid root\n"
    );
    assert!(output.stdout.is_empty());
}
