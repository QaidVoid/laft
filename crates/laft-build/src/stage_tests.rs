use laft_fetch::{Config, Fetcher};

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

/// Runs `stage` into `dir/root` and returns the states it reported.
fn run(dir: &Path, stage: &Stage, dry_run: bool) -> Result<Vec<(String, StageState)>, BuildError> {
    let repos = Repos::open(&[&stage.recipes]).unwrap();
    let build_root = BuildRoot::new(dir.join("build"));
    let config = Config {
        offline: true,
        ..Config::new(dir.join("cache"))
    };
    let cache = SourceCache::new(build_root.sources(), Fetcher::new(config));
    let options = StageOptions {
        root: dir.join("root"),
        jobs: 1,
        timeout: Duration::from_secs(600),
        min_available: 0,
        dry_run,
        laft_build: PathBuf::from("/nonexistent"),
        mirrors: Vec::new(),
    };
    let mut states = Vec::new();
    run_stage(stage, &repos, &cache, &build_root, &options, &mut |event| {
        if let StageEvent::Finished { step, state, .. } = event {
            states.push((step.to_owned(), state));
        }
    })?;
    Ok(states)
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

#[test]
fn a_dynamic_laft_build_cannot_build_inside_the_root() {
    let error = check_static(Path::new("/bin/sh")).unwrap_err();
    assert_eq!(error.code(), Some("B042"));
}
