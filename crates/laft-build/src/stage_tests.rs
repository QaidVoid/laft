use laft_fetch::{Config, Fetcher};
use laft_test::write_program;

use super::*;

const RECIPE: &str = r#"package greet {
  version = "1"
  summary = "Greets"
  license = [MIT]
  empty   = true
  build custom {}
  phases {
    install = """
      mkdir -p "$DESTDIR/usr/share"
      echo hi > "$DESTDIR/usr/share/greeting"
      """
  }
}
"#;

/// Writes a recipe repository with `greet` and a stage file with `steps`
/// below `dir`, and returns the stage file.
fn write_stage(dir: &Path, steps: &str) -> PathBuf {
    let repo = dir.join("repo");
    fs::create_dir_all(repo.join("recipes/greet")).unwrap();
    fs::write(repo.join("recipes.stav"), "recipe_repo t {}\n").unwrap();
    fs::write(repo.join("recipes/greet/package.stav"), RECIPE).unwrap();
    let file = dir.join("stage.stav");
    fs::write(
        &file,
        format!("stage demo {{\n  recipes = \"repo\"\n  path = [\"tools/bin\"]\n  steps = [\n{steps}\n  ]\n}}\n"),
    )
    .unwrap();
    file
}

const STEPS: &str = r#"    { name = "build", packages = [greet] },
    { name = "check", script = """
        test -f "$ROOT/usr/share/greeting"
        echo checked >> "$ROOT/count"
        """ },"#;

/// The options of a run into `dir/root`.
fn options(dir: &Path) -> StageOptions {
    StageOptions {
        root: dir.join("root"),
        jobs: 1,
        timeout: Duration::from_secs(600),
        min_available: 0,
        dry_run: false,
        rebuild: false,
        laft_build: PathBuf::from("/nonexistent"),
        mirrors: Vec::new(),
    }
}

/// Runs `stage` with `options` and the build root `dir/build`, and returns
/// the states it reported.
fn run_with(
    dir: &Path,
    stage: &Stage,
    options: &StageOptions,
) -> Result<Vec<(String, StageState)>, BuildError> {
    states(dir, stage, options, true)
}

/// Runs `stage` like [`run_with`], with the checks of [`run_stage`] only
/// when `checked`.
fn states(
    dir: &Path,
    stage: &Stage,
    options: &StageOptions,
    checked: bool,
) -> Result<Vec<(String, StageState)>, BuildError> {
    let repos = Repos::open(&[&stage.recipes]).unwrap();
    let build_root = BuildRoot::new(dir.join("build"));
    let config = Config {
        offline: true,
        ..Config::new(dir.join("cache"))
    };
    let cache = SourceCache::new(build_root.sources(), Fetcher::new(config));
    let mut states = Vec::new();
    let mut progress = |event: StageEvent<'_>| {
        if let StageEvent::Finished { step, state, .. } = event {
            states.push((step.to_owned(), state));
        }
    };
    if checked {
        run_stage(stage, &repos, &cache, &build_root, options, &mut progress)?;
    } else {
        run_steps(stage, &repos, &cache, &build_root, options, &mut progress)?;
    }
    Ok(states)
}

/// Runs `stage` into `dir/root` and returns the states it reported.
fn run(dir: &Path, stage: &Stage, dry_run: bool) -> Result<Vec<(String, StageState)>, BuildError> {
    run_with(
        dir,
        stage,
        &StageOptions {
            dry_run,
            ..options(dir)
        },
    )
}

fn no_shell() -> bool {
    if Path::new("/bin/sh").exists() {
        return false;
    }
    println!("skipping: the host has no /bin/sh");
    true
}

#[test]
fn a_stage_file_reads_into_ordered_steps() {
    let dir = laft_test::tempdir();
    let stage = Stage::load(&write_stage(dir.path(), STEPS)).unwrap();
    assert_eq!(stage.name, "demo");
    assert_eq!(
        stage.recipes,
        fs::canonicalize(dir.path().join("repo")).unwrap()
    );
    assert_eq!(stage.path, [PathBuf::from("tools/bin")]);
    let names: Vec<&str> = stage.steps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["build", "check"]);
    assert_eq!(
        stage.steps[0].work,
        Work::Packages(vec!["greet".parse().unwrap()])
    );
    assert!(matches!(stage.steps[1].work, Work::Script { .. }));
    assert_eq!(stage.steps[1].place, Place::Host);
}

#[test]
fn a_step_needs_exactly_one_kind_of_work_and_a_unique_name() {
    for steps in [
        r#"    { name = "a" },"#,
        r#"    { name = "a", packages = [greet], script = "true" },"#,
        r#"    { name = "a", script = "true" }, { name = "a", script = "true" },"#,
    ] {
        let dir = laft_test::tempdir();
        let error = Stage::load(&write_stage(dir.path(), steps)).unwrap_err();
        assert!(error.to_string().contains("R060"), "{steps}: {error}");
    }
}

#[test]
fn a_stage_installs_runs_scripts_once_and_resumes() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let file = write_stage(dir.path(), STEPS);
    let stage = Stage::load(&file).unwrap();
    let pending = run(dir.path(), &stage, true).unwrap();
    assert!(
        pending.iter().all(|(_, s)| *s == StageState::Pending),
        "{pending:?}"
    );
    assert!(
        !dir.path().join("root").exists(),
        "a dry run changes nothing"
    );

    let first = run(dir.path(), &stage, false).unwrap();
    assert!(
        first.iter().all(|(_, s)| *s == StageState::Done),
        "{first:?}"
    );
    let root = dir.path().join("root");
    assert_eq!(
        fs::read_link(root.join("lib64")).unwrap(),
        Path::new("usr/lib")
    );
    assert_eq!(
        fs::read_link(root.join("sbin")).unwrap(),
        Path::new("usr/bin")
    );
    assert_eq!(
        fs::read_to_string(root.join("usr/share/greeting")).unwrap(),
        "hi\n"
    );
    let logs = dir.path().join("build/logs/stage/demo");
    assert!(logs.join("02-check.log").is_file());

    let second = run(dir.path(), &stage, false).unwrap();
    assert!(
        second.iter().all(|(_, s)| *s == StageState::UpToDate),
        "{second:?}"
    );
    assert_eq!(fs::read_to_string(root.join("count")).unwrap(), "checked\n");

    let changed = STEPS.replace("checked", "rechecked");
    let stage = Stage::load(&write_stage(dir.path(), &changed)).unwrap();
    let third = run(dir.path(), &stage, false).unwrap();
    assert_eq!(
        third,
        [
            ("build".to_owned(), StageState::UpToDate),
            ("check".to_owned(), StageState::Done)
        ]
    );
}

/// Makes the stage of `file` a stage of packages whose `greet` installs
/// the greeting from a source that a `file://` mirror below `dir` serves.
fn packages_stage(dir: &Path, file: &Path) -> Stage {
    use sha2::Digest;
    let greeting = b"hi\n";
    let hex: String = sha2::Sha256::digest(greeting)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mirror = dir.join("mirror/sha256");
    fs::create_dir_all(&mirror).unwrap();
    fs::write(mirror.join(&hex), greeting).unwrap();
    let repo = dir.join("repo");
    fs::write(
        repo.join("recipes.stav"),
        format!(
            "recipe_repo t {{\n  source_mirrors = [\"file://{}\"]\n}}\n",
            dir.join("mirror").display()
        ),
    )
    .unwrap();
    let recipe = format!(
        r#"package greet {{
  version = "1"
  summary = "Greets"
  license = [MIT]
  source main {{
    url     = ["https://example.org/greeting"]
    sha256  = "sha256:{hex}"
    extract = false
  }}
  build custom {{}}
  phases {{
    install = """
      install -Dm644 "$SRC/greeting" "$DESTDIR/usr/share/greeting"
      """
  }}
}}
"#
    );
    fs::write(repo.join("recipes/greet/package.stav"), recipe).unwrap();
    let text = fs::read_to_string(file).unwrap();
    fs::write(
        file,
        text.replace(
            "recipes = \"repo\"",
            "recipes = \"repo\"\n  install = packages",
        ),
    )
    .unwrap();
    Stage::load(file).unwrap()
}

#[test]
fn a_stage_of_packages_unpacks_the_packages_it_makes_into_the_root() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let file = write_stage(dir.path(), STEPS);
    let stage = packages_stage(dir.path(), &file);
    assert_eq!(stage.install, Install::Packages);
    let first = run(dir.path(), &stage, false).unwrap();
    assert!(
        first.iter().all(|(_, s)| *s == StageState::Done),
        "{first:?}"
    );
    let root = dir.path().join("root");
    assert_eq!(
        fs::read_to_string(root.join("usr/share/greeting")).unwrap(),
        "hi\n"
    );
    let build_root = BuildRoot::new(dir.path().join("build"));
    let packages: Vec<String> = fs::read_dir(build_root.out())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".lpkg"))
        .collect();
    assert_eq!(packages.len(), 1, "{packages:?}");
    assert!(packages[0].starts_with("greet-1-r1."), "{packages:?}");
    let repos = Repos::open(&[&stage.recipes]).unwrap();
    let recipe = recipes(&repos, &"greet".parse().unwrap())
        .unwrap()
        .remove(0);
    let item = Item::of(&recipe);
    let built = build_root.db().latest(&item).unwrap().expect("a record");
    assert_eq!(installed(&root, &item).unwrap(), Some(built.record.key));

    let second = run(dir.path(), &stage, false).unwrap();
    assert!(
        second.iter().all(|(_, s)| *s == StageState::UpToDate),
        "{second:?}"
    );
    let direct = Stage {
        install: Install::Direct,
        ..stage
    };
    let third = run(dir.path(), &direct, false).unwrap();
    assert!(
        third.iter().all(|(_, s)| *s == StageState::Done),
        "a stage that stops making packages runs again: {third:?}"
    );
}

#[test]
fn a_failed_packages_step_resumes_with_the_package_that_failed() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let steps = r#"    { name = "build", packages = [greet, wave] },"#;
    let file = write_stage(dir.path(), steps);
    let repo = dir.path().join("repo");
    let builds = dir.path().join("greet-builds");
    fs::write(
        repo.join("recipes/greet/package.stav"),
        RECIPE.replace(
            "echo hi > \"$DESTDIR/usr/share/greeting\"",
            &format!(
                "echo hi > \"$DESTDIR/usr/share/greeting\"\n      echo built >> '{}'",
                builds.display()
            ),
        ),
    )
    .unwrap();
    let allow = dir.path().join("allow");
    fs::create_dir_all(repo.join("recipes/wave")).unwrap();
    fs::write(
        repo.join("recipes/wave/package.stav"),
        RECIPE.replace("package greet", "package wave").replace(
            "mkdir -p",
            &format!("test -f '{}'\n      mkdir -p", allow.display()),
        ),
    )
    .unwrap();
    let stage = Stage::load(&file).unwrap();
    let error = run(dir.path(), &stage, false).unwrap_err();
    assert_eq!(error.code(), Some("B040"), "{error}");

    fs::write(&allow, "").unwrap();
    let states = run(dir.path(), &stage, false).unwrap();
    assert_eq!(
        states,
        [
            ("build".to_owned(), StageState::UpToDate),
            ("build".to_owned(), StageState::Done)
        ]
    );
    assert_eq!(fs::read_to_string(&builds).unwrap(), "built\n");
    let states = run(dir.path(), &stage, false).unwrap();
    assert!(
        states.iter().all(|(_, s)| *s == StageState::UpToDate),
        "{states:?}"
    );

    let rebuild = StageOptions {
        rebuild: true,
        ..options(dir.path())
    };
    run_with(dir.path(), &stage, &rebuild).unwrap();
    assert_eq!(fs::read_to_string(&builds).unwrap(), "built\nbuilt\n");
}

#[test]
fn a_step_that_failed_after_a_change_runs_again_when_the_change_is_undone() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let file = write_stage(dir.path(), STEPS);
    let recipe = dir.path().join("repo/recipes/greet/package.stav");
    let stage = Stage::load(&file).unwrap();
    run(dir.path(), &stage, false).unwrap();

    let failing = RECIPE.replace("mkdir -p", "exit 1\n      mkdir -p");
    fs::write(&recipe, failing).unwrap();
    let error = run(dir.path(), &stage, false).unwrap_err();
    assert_eq!(error.code(), Some("B040"), "{error}");
    fs::write(&recipe, RECIPE).unwrap();
    let states = run(dir.path(), &stage, false).unwrap();
    assert_eq!(
        states,
        [
            ("build".to_owned(), StageState::Done),
            ("check".to_owned(), StageState::UpToDate)
        ],
        "the failed build may have left files, so the package builds again"
    );

    let text = fs::read_to_string(&file).unwrap();
    let checked = "echo checked >> \"$ROOT/count\"";
    fs::write(
        &file,
        text.replace(checked, &format!("{checked}\n        exit 1")),
    )
    .unwrap();
    let error = run(dir.path(), &Stage::load(&file).unwrap(), false).unwrap_err();
    assert_eq!(error.code(), Some("B040"), "{error}");
    fs::write(&file, text).unwrap();
    let states = run(dir.path(), &Stage::load(&file).unwrap(), false).unwrap();
    assert_eq!(
        states,
        [
            ("build".to_owned(), StageState::UpToDate),
            ("check".to_owned(), StageState::Done)
        ],
        "the failed script may have changed the root, so it runs again"
    );
}

#[test]
fn a_failing_script_stops_the_stage_and_runs_again_next_time() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let steps = r#"    { name = "fail", script = "echo broken; exit 3" },
    { name = "build", packages = [greet] },"#;
    let stage = Stage::load(&write_stage(dir.path(), steps)).unwrap();
    let error = run(dir.path(), &stage, false).unwrap_err();
    assert_eq!(error.code(), Some("B040"));
    assert!(error.to_string().contains("| broken"), "{error}");
    assert!(!dir.path().join("root/usr/share/greeting").exists());
    let error = run(dir.path(), &stage, false).unwrap_err();
    assert_eq!(error.code(), Some("B040"));
}

#[test]
fn a_root_with_another_layout_is_refused() {
    let dir = laft_test::tempdir();
    let stage = Stage::load(&write_stage(dir.path(), STEPS)).unwrap();
    fs::create_dir_all(dir.path().join("root/lib64")).unwrap();
    let error = run(dir.path(), &stage, false).unwrap_err();
    assert_eq!(error.code(), Some("B042"));
}

/// This test program when it is dynamically linked, which it is unless
/// the tests were built for a static target.
fn dynamic_program() -> Option<PathBuf> {
    let program = std::env::current_exe().unwrap();
    let elf = laft_elf::inspect(&fs::read(&program).unwrap()).unwrap()?;
    if elf.is_static() {
        println!("skipping: the test program is statically linked");
        return None;
    }
    Some(program)
}

#[test]
fn a_dynamic_laft_build_cannot_build_inside_the_root() {
    let Some(program) = dynamic_program() else {
        return;
    };
    let dir = laft_test::tempdir();
    let steps = r#"    { name = "build", place = root, packages = [greet] },"#;
    let stage = Stage::load(&write_stage(dir.path(), steps)).unwrap();
    let options = StageOptions {
        laft_build: program,
        ..options(dir.path())
    };
    let error = run_with(dir.path(), &stage, &options).unwrap_err();
    assert_eq!(error.code(), Some("B042"));
    assert!(
        error.to_string().contains("not statically linked"),
        "{error}"
    );
}

#[test]
fn a_step_that_runs_again_runs_every_later_step_again() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let stage = Stage::load(&write_stage(dir.path(), STEPS)).unwrap();
    run(dir.path(), &stage, false).unwrap();
    let recipe = dir.path().join("repo/recipes/greet/package.stav");
    fs::write(&recipe, RECIPE.replace("echo hi", "echo hello")).unwrap();
    let states = run(dir.path(), &stage, false).unwrap();
    assert_eq!(
        states,
        [
            ("build".to_owned(), StageState::Done),
            ("check".to_owned(), StageState::Done)
        ]
    );
    let root = dir.path().join("root");
    assert_eq!(
        fs::read_to_string(root.join("count")).unwrap(),
        "checked\nchecked\n"
    );
}

#[test]
fn a_rebuild_runs_every_step_again() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let stage = Stage::load(&write_stage(dir.path(), STEPS)).unwrap();
    run(dir.path(), &stage, false).unwrap();
    let rebuild = StageOptions {
        rebuild: true,
        ..options(dir.path())
    };
    let states = run_with(dir.path(), &stage, &rebuild).unwrap();
    assert!(
        states.iter().all(|(_, s)| *s == StageState::Done),
        "{states:?}"
    );
    let states = run(dir.path(), &stage, false).unwrap();
    assert!(
        states.iter().all(|(_, s)| *s == StageState::UpToDate),
        "{states:?}"
    );
}

#[test]
fn a_script_runs_again_when_its_environment_changes() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let steps = |value: &str| {
        format!(
            r#"    {{ name = "note", script = "echo \"$NOTE\" >> \"$ROOT/notes\"", env = {{ NOTE = "{value}" }} }},"#
        )
    };
    let stage = Stage::load(&write_stage(dir.path(), &steps("a"))).unwrap();
    run(dir.path(), &stage, false).unwrap();
    run(dir.path(), &stage, false).unwrap();
    let stage = Stage::load(&write_stage(dir.path(), &steps("b"))).unwrap();
    let states = run(dir.path(), &stage, false).unwrap();
    assert_eq!(states, [("note".to_owned(), StageState::Done)]);
    assert_eq!(
        fs::read_to_string(dir.path().join("root/notes")).unwrap(),
        "a\nb\n"
    );
}

#[test]
fn a_host_step_runs_again_when_the_search_path_changes() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let steps = r#"    { name = "note", script = "echo run >> \"$ROOT/notes\"" },
    { name = "inside", place = root, script = "true" },"#;
    let file = write_stage(dir.path(), steps);
    let stage = Stage::load(&file).unwrap();
    let host_only = Stage {
        steps: stage.steps[..1].to_vec(),
        ..stage.clone()
    };
    run(dir.path(), &host_only, false).unwrap();
    let moved = Stage {
        path: vec![PathBuf::from("cross/bin")],
        ..host_only
    };
    let states = run(dir.path(), &moved, false).unwrap();
    assert_eq!(states, [("note".to_owned(), StageState::Done)]);
    assert_eq!(
        fs::read_to_string(dir.path().join("root/notes")).unwrap(),
        "run\nrun\n"
    );
    let root_step = |path: &str| {
        let stage = Stage {
            path: vec![PathBuf::from(path)],
            ..stage.clone()
        };
        let step = &stage.steps[1];
        step_key(step, None, &stage.path, serde_json::json!({})).unwrap()
    };
    assert_eq!(
        root_step("tools/bin"),
        root_step("cross/bin"),
        "a step inside the root does not search the path"
    );
}

#[test]
fn a_stage_name_must_be_a_plain_name() {
    let dir = laft_test::tempdir();
    let file = write_stage(dir.path(), STEPS);
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, text.replace("stage demo", "stage \"../demo\"")).unwrap();
    let error = Stage::load(&file).unwrap_err();
    assert!(error.to_string().contains("../demo"), "{error}");
}

#[test]
fn a_laft_build_that_is_no_elf_file_is_named_as_such() {
    let dir = laft_test::tempdir();
    let program = dir.path().join("laft-build");
    fs::write(&program, "not a program").unwrap();
    let error = check_static(&program).unwrap_err();
    assert_eq!(error.code(), Some("B042"));
    assert!(error.to_string().contains("not an ELF file"), "{error}");
}

/// Where the host keeps shared libraries.
const LIBRARY_DIRS: [&str; 6] = [
    "/lib64",
    "/usr/lib64",
    "/lib",
    "/usr/lib",
    "/lib/x86_64-linux-gnu",
    "/usr/lib/x86_64-linux-gnu",
];

/// Copies the dynamic loader and every library that the host's `program`
/// needs into `root`, at their host paths, so that it runs there.
fn copy_libraries(root: &Path, program: &Path) {
    let mut queue = vec![program.to_path_buf()];
    let mut copied = BTreeSet::new();
    while let Some(file) = queue.pop() {
        let Some(elf) = laft_elf::inspect(&fs::read(&file).unwrap()).unwrap() else {
            continue;
        };
        let found = elf.needed.iter().filter_map(|lib| {
            LIBRARY_DIRS
                .iter()
                .map(|dir| Path::new(dir).join(lib.as_str()))
                .find(|path| path.exists())
        });
        for dep in elf.interpreter.iter().map(PathBuf::from).chain(found) {
            if copied.insert(dep.clone()) {
                let dest = root.join(dep.strip_prefix("/").unwrap());
                fs::create_dir_all(dest.parent().unwrap()).unwrap();
                write_program(&dest, &fs::read(&dep).unwrap());
                queue.push(dep);
            }
        }
    }
}

/// The `laft-build` program that cargo built next to this test program.
fn built_laft_build() -> Option<PathBuf> {
    let exe = std::env::current_exe().unwrap();
    let program = exe.parent()?.parent()?.join("laft-build");
    if program.is_file() {
        return Some(program);
    }
    println!("skipping: {} is not built", program.display());
    None
}

/// A stage whose `greet` writes its greeting with shell builtins only, and
/// the root `dir/root` with Laft's layout and a copy of the host's
/// `/bin/sh` and its libraries.
fn root_stage(dir: &Path, steps: &str) -> Stage {
    let stage = Stage::load(&write_stage(dir, steps)).unwrap();
    fs::write(
        stage.recipes.join("recipes/greet/package.stav"),
        RECIPE.replace(
            "mkdir -p \"$DESTDIR/usr/share\"\n      echo hi > \"$DESTDIR/usr/share/greeting\"",
            "echo hi > \"$DESTDIR/greeting\"",
        ),
    )
    .unwrap();
    let root = dir.join("root");
    layout(&root).unwrap();
    // Some hosts' dynamic loaders search only `/usr/lib64`.
    std::os::unix::fs::symlink("lib", root.join("usr/lib64")).unwrap();
    let sh = fs::canonicalize("/bin/sh").unwrap();
    write_program(&root.join("usr/bin/sh"), &fs::read(&sh).unwrap());
    copy_libraries(&root, &sh);
    stage
}

/// Whether `result` failed because the sandbox cannot run here, which it
/// prints.
fn refused<T>(result: &Result<T, BuildError>) -> bool {
    match result {
        Err(error) if error.refuses_namespaces() => {
            println!("skipping: {error}");
            true
        }
        _ => false,
    }
}

#[test]
fn a_root_step_builds_inside_the_root_with_laft_build() {
    if no_shell() {
        return;
    }
    let Some(laft_build) = built_laft_build() else {
        return;
    };
    let dir = laft_test::tempdir();
    let steps = r#"    { name = "build", place = root, packages = [greet] },
    { name = "check", place = root, script = """
        test -f /greeting
        : > /run/laft-build-root/written
        test ! -w /run/laft-recipes
        test ! -e /run/laft-build
        """ },"#;
    let stage = root_stage(dir.path(), steps);
    let root = dir.path().join("root");
    copy_libraries(&root, &laft_build);
    let options = StageOptions {
        laft_build: laft_build.clone(),
        ..options(dir.path())
    };
    let result = states(dir.path(), &stage, &options, false);
    if refused(&result) {
        return;
    }
    let logs = dir.path().join("build/logs/stage/demo");
    let log = fs::read_to_string(logs.join("01-build-greet.log")).unwrap_or_default();
    let states = result.unwrap_or_else(|e| panic!("{e}\n{log}"));
    assert!(
        states.iter().all(|(_, s)| *s == StageState::Done),
        "{states:?}"
    );
    assert_eq!(fs::read_to_string(root.join("greeting")).unwrap(), "hi\n");
    assert!(dir.path().join("build/written").is_file());
    let repos = Repos::open(&[&stage.recipes]).unwrap();
    let recipe = recipes(&repos, &"greet".parse().unwrap())
        .unwrap()
        .remove(0);
    let item = Item::of(&recipe);
    assert!(installed(&root, &item).unwrap().is_some());

    fs::remove_file(root.join("greeting")).unwrap();
    let rebuild = StageOptions {
        rebuild: true,
        ..options.clone()
    };
    self::states(dir.path(), &stage, &rebuild, false).unwrap();
    assert!(root.join("greeting").is_file(), "the build ran again");
}

#[test]
fn a_root_build_that_records_no_key_fails() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let steps = r#"    { name = "build", place = root, packages = [greet] },"#;
    let stage = root_stage(dir.path(), steps);
    let fake = dir.path().join("fake-laft-build");
    write_program(&fake, b"#!/bin/sh\nexit 0\n");
    let options = StageOptions {
        laft_build: fake,
        ..options(dir.path())
    };
    let result = states(dir.path(), &stage, &options, false);
    if refused(&result) {
        return;
    }
    let error = result.unwrap_err();
    assert_eq!(error.code(), Some("B040"), "{error}");
    assert!(
        error
            .to_string()
            .contains("without recording its build key"),
        "{error}"
    );
}

#[test]
fn a_root_step_of_a_stage_of_packages_unpacks_inside_the_root() {
    if no_shell() {
        return;
    }
    let dir = laft_test::tempdir();
    let steps = r#"    { name = "build", place = root, packages = [greet] },"#;
    let stage = Stage {
        install: Install::Packages,
        ..root_stage(dir.path(), steps)
    };
    let fake = dir.path().join("fake-laft-build");
    write_program(
        &fake,
        b"#!/bin/sh\necho \"$@\" > /run/laft-build-root/arguments\n",
    );
    let options = StageOptions {
        laft_build: fake,
        ..options(dir.path())
    };
    let result = states(dir.path(), &stage, &options, false);
    if refused(&result) {
        return;
    }
    let error = result.unwrap_err();
    assert_eq!(error.code(), Some("B040"), "{error}");
    let arguments = fs::read_to_string(dir.path().join("build/arguments")).unwrap();
    assert!(
        arguments.contains("bootstrap --root / --unpack / "),
        "{arguments}"
    );
}

#[test]
fn a_root_script_that_runs_too_long_is_killed() {
    if no_shell() {
        return;
    }
    let Ok(sleep) = fs::canonicalize("/usr/bin/sleep") else {
        println!("skipping: the host has no /usr/bin/sleep");
        return;
    };
    let dir = laft_test::tempdir();
    let steps = r#"    { name = "spin", place = root, script = "sleep 3; : > /survived" },"#;
    let stage = root_stage(dir.path(), steps);
    let root = dir.path().join("root");
    write_program(&root.join("usr/bin/sleep"), &fs::read(&sleep).unwrap());
    copy_libraries(&root, &sleep);
    let options = StageOptions {
        timeout: Duration::from_secs(1),
        ..options(dir.path())
    };
    let result = states(dir.path(), &stage, &options, false);
    if refused(&result) {
        return;
    }
    assert_eq!(result.unwrap_err().code(), Some("B030"));
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        !root.join("survived").exists(),
        "the script was killed before it wrote its marker"
    );
}
