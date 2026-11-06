//! A caught signal ends a stage whose big build waits for memory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use laft_build::{BuildRoot, Repos, SourceCache, Stage, StageEvent, StageOptions, run_stage};
use laft_fetch::{Config, Fetcher};

const RECIPE: &str = r#"package greet {
  version = "1"
  summary = "Greets"
  license = [MIT]
  empty   = true
  ci { runner = big }
  build custom {}
  phases {
    install = """
      mkdir -p "$DESTDIR/usr/share"
      echo hi > "$DESTDIR/usr/share/greeting"
      """
  }
}
"#;

/// The options of a run whose big build waits for memory that the host
/// never holds.
fn options(dir: &Path) -> StageOptions {
    StageOptions {
        root: Some(dir.join("root")),
        check: false,
        jobs: 1,
        workers: 1,
        timeout: Duration::from_secs(600),
        min_available: u64::MAX,
        dry_run: false,
        rebuild: false,
        laft_build: PathBuf::from("/nonexistent"),
        mirrors: Vec::new(),
        lock: None,
        options: Default::default(),
    }
}

/// Writes the repository with `greet` and the stage file below `dir`, and
/// returns the stage.
fn write_stage(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    fs::create_dir_all(repo.join("recipes/greet")).expect("the test setup works");
    fs::write(repo.join("recipes.stav"), "recipe_repo t {}\n").expect("the test setup works");
    fs::write(repo.join("recipes/greet/package.stav"), RECIPE).expect("the test setup works");
    let file = dir.join("stage.stav");
    fs::write(
        &file,
        "stage demo {\n  recipes = \"repo\"\n  path = [\"tools/bin\"]\n  steps = [\n    \
         { name = \"build\", packages = [greet] }\n  ]\n}\n",
    )
    .expect("the test setup works");
    file
}

#[test]
fn an_interrupt_ends_the_wait_for_memory() {
    if !Path::new("/bin/sh").exists() {
        println!("skipping: /bin/sh is missing");
        return;
    }
    laft_sandbox::catch_termination();
    let dir = laft_test::tempdir();
    let stage = Stage::load(&write_stage(dir.path())).expect("the stage is valid");
    let repos = Repos::open(&[&stage.recipes]).expect("the repository opens");
    let build_root = BuildRoot::new(dir.path().join("build"));
    let config = Config {
        offline: true,
        ..Config::new(dir.path().join("cache"))
    };
    let cache = SourceCache::new(build_root.sources(), Fetcher::new(config));
    // The signal goes out while the wait for the memory of the big build
    // is actually running, at the latest after a minute, so the test
    // stays bounded when the wait never comes.
    let (waiting, waited) = mpsc::channel();
    let sender = thread::spawn(move || {
        let _ = waited.recv_timeout(Duration::from_secs(60));
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -TERM {}", std::process::id()))
            .status()
            .expect("the signal is sent");
        assert!(status.success(), "the signal is sent");
    });
    let error = run_stage(
        &stage,
        &repos,
        &cache,
        &build_root,
        &options(dir.path()),
        &move |event| {
            if let StageEvent::Waiting { .. } = event {
                let _ = waiting.send(());
            }
        },
    )
    .expect_err("the interrupted run fails instead of waiting");
    sender.join().expect("the signal thread ends");
    assert_eq!(
        error.code(),
        Some("B045"),
        "the run fails as interrupted, not {error}"
    );
}
