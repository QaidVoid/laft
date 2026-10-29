use super::*;

/// Why sandboxes cannot run here, or `None` when they can.
fn unsupported(dir: &Path) -> Option<String> {
    let (build, out) = (dir.join("probe/build"), dir.join("probe/out"));
    fs::create_dir_all(&build).unwrap();
    fs::create_dir_all(&out).unwrap();
    match laft_sandbox::Sandbox::new("/usr", &build, &out)
        .command("/usr/bin/true")
        .status()
    {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!("a sandboxed program exits with {status}")),
        Err(e) => Some(format!("sandboxes are unavailable: {e}")),
    }
}

#[test]
fn a_failing_smoke_test_fails_the_build_and_its_tree_is_removed() {
    let dir = laft_test::tempdir();
    if let Some(reason) = unsupported(dir.path()) {
        println!("skipping: {reason}");
        return;
    }
    let work = dir.path().join("smoke");
    let logs = dir.path().join("logs");
    let timeout = Duration::from_secs(60);
    smoke(
        Path::new("/usr"),
        "test -d /build/src && ls /out",
        &work,
        &logs,
        timeout,
    )
    .expect("the script succeeds");
    assert!(!work.exists());
    let error = smoke(
        Path::new("/usr"),
        "echo missing tool >&2; exit 1",
        &work,
        &logs,
        timeout,
    )
    .expect_err("the script fails");
    assert_eq!(error.code(), Some("B110"));
    assert!(error.to_string().contains("missing tool"), "{error}");
    assert!(!work.exists());
}
