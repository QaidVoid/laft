use super::*;

#[test]
fn build_and_out_paths_are_rebased() {
    let (build, out) = (Path::new("/w/k/build"), Path::new("/w/k/out"));
    assert_eq!(rebase("/build/src", build, out), "/w/k/build/src");
    assert_eq!(rebase("/out", build, out), "/w/k/out");
    assert_eq!(
        rebase("/out/usr/x86_64-laft-linux-musl", build, out),
        "/w/k/out/usr/x86_64-laft-linux-musl"
    );
    assert_eq!(
        rebase(
            "-g -ffile-prefix-map=/build/src=/usr/src/debug/z",
            build,
            out
        ),
        "-g -ffile-prefix-map=/w/k/build/src=/usr/src/debug/z"
    );
    assert_eq!(
        rebase(
            "cd \"/build/obj\" && make DESTDIR='/out' install",
            build,
            out
        ),
        "cd \"/w/k/build/obj\" && make DESTDIR='/w/k/out' install"
    );
}

#[test]
fn other_paths_are_left_alone() {
    let (build, out) = (Path::new("/b"), Path::new("/o"));
    for text in [
        "/usr/bin",
        "/outside",
        "/builder/x",
        "a/build/x",
        "$OUT/out",
    ] {
        assert_eq!(rebase(text, build, out), text);
    }
}

#[test]
fn a_direct_phase_sees_the_real_directories_and_logs_its_output() {
    let dir = laft_test::tempdir();
    let (build, out) = (dir.path().join("build"), dir.path().join("out"));
    fs::create_dir_all(&build).unwrap();
    fs::create_dir_all(&out).unwrap();
    let exec = Executor::direct("/", &build, &out, dir.path().join("logs"));
    let env = BTreeMap::from([("DESTDIR".to_owned(), "/out".to_owned())]);
    exec.run(
        "install",
        "echo hello > \"$DESTDIR/greeting\"; echo done",
        &env,
    )
    .expect("the script succeeds");
    assert_eq!(fs::read(out.join("greeting")).unwrap(), b"hello\n");
    let log = fs::read_to_string(dir.path().join("logs/install.log")).unwrap();
    assert_eq!(log, "done\n");
}

#[test]
fn a_failing_phase_names_its_log_and_shows_its_end() {
    let dir = laft_test::tempdir();
    let exec = Executor::direct("/", dir.path(), dir.path(), dir.path().join("logs"));
    let error = exec
        .run(
            "configure",
            "echo 'no compiler found' >&2; exit 3",
            &BTreeMap::new(),
        )
        .expect_err("the script fails");
    assert_eq!(error.code(), Some("B040"));
    let message = error.to_string();
    assert!(message.contains("configure.log"), "{message}");
    assert!(message.contains("| no compiler found"), "{message}");
}

#[test]
fn a_phase_that_runs_too_long_is_killed() {
    let dir = laft_test::tempdir();
    let exec = Executor::direct("/", dir.path(), dir.path(), dir.path().join("logs"))
        .timeout(Duration::from_millis(200));
    let started = Instant::now();
    let error = exec
        .run("build", "sleep 30", &BTreeMap::new())
        .expect_err("the phase times out");
    assert_eq!(error.code(), Some("B030"));
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_direct_phase_that_times_out_leaves_no_process_behind() {
    let dir = laft_test::tempdir();
    let marker = dir.path().join("late");
    let exec = Executor::direct("/", dir.path(), dir.path(), dir.path().join("logs"))
        .timeout(Duration::from_millis(300));
    let script = format!("(sleep 1; touch '{}') & sleep 30", marker.display());
    let error = exec
        .run("build", &script, &BTreeMap::new())
        .expect_err("the phase times out");
    assert_eq!(error.code(), Some("B030"));
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!marker.exists(), "a process of the phase outlived it");
}

#[test]
fn a_direct_phase_finds_programs_in_the_search_directories_first() {
    let dir = laft_test::tempdir();
    let tools = dir.path().join("tools/bin");
    fs::create_dir_all(&tools).unwrap();
    let program = tools.join("uname");
    fs::write(&program, "#!/usr/bin/bash\necho from-tools\n").unwrap();
    fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let exec = Executor::direct("/", dir.path(), dir.path(), dir.path().join("logs"))
        .search_first(vec![tools.clone()]);
    exec.run(
        "build",
        "uname > found; printf %s \"$PATH\" > path",
        &BTreeMap::new(),
    )
    .expect("the script succeeds");
    assert_eq!(
        fs::read_to_string(dir.path().join("found")).unwrap(),
        "from-tools\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("path")).unwrap(),
        format!("{}:/usr/bin", tools.display())
    );
}

#[test]
fn a_search_directory_with_a_colon_is_refused() {
    let dir = laft_test::tempdir();
    let exec = Executor::direct("/", dir.path(), dir.path(), dir.path().join("logs"))
        .search_first(vec![dir.path().join("a:b")]);
    let error = exec
        .run("build", "true", &BTreeMap::new())
        .expect_err("PATH cannot hold the directory");
    assert!(error.to_string().contains("a:b"), "{error}");
}
