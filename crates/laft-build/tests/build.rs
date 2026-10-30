//! Builds a small C package from a local tarball with the host's compiler
//! as the build view, in the sandbox, and checks the packages, their
//! dependencies, the debug split, the buildinfo, early cutoff and that two
//! builds give the same store ids.
//!
//! The test is skipped with a reason when the machine cannot run
//! sandboxes or has no C toolchain in `/usr/bin`.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use laft_build::{
    BuildRoot, Builder, Buildinfo, Cutoff, Event, Item, Mode, Recipe, Repos, SourceCache, Trigger,
    check_repro, installed,
};
use laft_core::Dep;
use laft_fetch::{Config, Fetcher};
use laft_pkg::{Entry, Reader};
use sha2::{Digest, Sha256};

const MAKEFILE: &str = "PREFIX ?= /usr
all: hello
libgreet.so.1.0.0: greet.c greet.h
\t$(CC) $(CFLAGS) -fPIC -shared -Wl,-soname,libgreet.so.1 -Wl,--build-id $(LDFLAGS) -o $@ greet.c
hello: hello.c libgreet.so.1.0.0
\tln -sf libgreet.so.1.0.0 libgreet.so
\t$(CC) $(CFLAGS) -Wl,--build-id $(LDFLAGS) -o $@ hello.c -L. -lgreet
install: all
\tinstall -Dm755 hello $(DESTDIR)$(PREFIX)/bin/hello
\tinstall -Dm755 libgreet.so.1.0.0 $(DESTDIR)$(PREFIX)/lib/libgreet.so.1.0.0
\tln -sf libgreet.so.1 $(DESTDIR)$(PREFIX)/lib/libgreet.so
\tinstall -Dm644 greet.h $(DESTDIR)$(PREFIX)/include/greet.h
";

const FILES: &[(&str, &str)] = &[
    ("Makefile", MAKEFILE),
    ("greet.h", "void greet(const char *name);\n"),
    (
        "greet.c",
        "#include <stdio.h>\n#include \"greet.h\"\nvoid greet(const char *name) { printf(\"hello, %s\\n\", name); }\n",
    ),
    (
        "hello.c",
        "#include \"greet.h\"\nint main(void) { greet(\"world\"); return 0; }\n",
    ),
    ("LICENSE", "Permission is granted to do anything.\n"),
];

/// Why this machine cannot run the test, or `None` when it can.
fn unsupported(dir: &Path) -> Option<String> {
    for tool in ["bash", "gcc", "make", "objcopy", "strip", "install", "ln"] {
        if !Path::new("/usr/bin").join(tool).exists() {
            return Some(format!("/usr/bin/{tool} is missing"));
        }
    }
    let (build, out) = (dir.join("probe/build"), dir.join("probe/out"));
    fs::create_dir_all(&build).expect("the test setup works");
    fs::create_dir_all(&out).expect("the test setup works");
    let status = laft_sandbox::Sandbox::new("/usr", &build, &out)
        .command("/usr/bin/true")
        .status();
    match status {
        Ok(s) if s.success() => None,
        Ok(s) => Some(format!("a sandboxed program exits with {s}")),
        Err(e) => Some(format!("sandboxes are unavailable: {e}")),
    }
}

/// A gzip compressed tarball of the sources below `hello-1.0/`.
fn tarball() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, text) in FILES {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("hello-1.0/{name}"), text.as_bytes())
            .expect("the test setup works");
    }
    let tar = builder.into_inner().expect("the test setup works");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar).expect("the test setup works");
    gz.finish().expect("the test setup works")
}

/// A recipe repository whose `hello` recipe takes its source from a
/// `file://` mirror below `dir`.
fn repos(dir: &Path) -> Repos {
    repos_with(dir, "  check = false\n  build make {}")
}

/// Like [`repos`], with `build` as the rest of the `hello` recipe.
fn repos_with(dir: &Path, build: &str) -> Repos {
    let bytes = tarball();
    let hex: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mirror = dir.join("mirror");
    fs::create_dir_all(mirror.join("sha256")).expect("the test setup works");
    fs::write(mirror.join("sha256").join(&hex), bytes).expect("the test setup works");
    let body = format!(
        "  source main {{\n    url = [\"https://example.org/hello-1.0.tar.gz\"]\n    \
         sha256 = \"sha256:{hex}\"\n  }}\n{build}"
    );
    let repo = support::write_repo(
        &dir.join("core"),
        "core",
        &format!("  source_mirrors = [\"file://{}\"]\n", mirror.display()),
        &[(
            "recipes/hello/package.stav",
            &support::recipe("hello", &body),
        )],
    );
    Repos::open(&[repo]).expect("the repository opens")
}

/// The entries of the package file `path`, by payload path.
fn entries(path: &Path) -> (laft_pkg::Meta, Vec<Entry>) {
    let reader = Reader::new(fs::File::open(path).expect("the test setup works"))
        .expect("the test setup works");
    let meta = laft_pkg::Meta::from_json(reader.meta_bytes()).expect("the test setup works");
    (meta, reader.manifest().entries().to_vec())
}

fn unpacked(path: &Path, into: &Path) -> PathBuf {
    let reader = Reader::new(fs::File::open(path).expect("the test setup works"))
        .expect("the test setup works");
    fs::create_dir_all(into).expect("the test setup works");
    reader.unpack(into).expect("the test setup works");
    into.to_path_buf()
}

fn contains(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn a_c_package_builds_in_the_sandbox_into_reproducible_packages() {
    let dir = laft_test::tempdir();
    if let Some(reason) = unsupported(dir.path()) {
        println!("skipping: {reason}");
        return;
    }
    let repos = repos(dir.path());
    let recipe: Recipe = support::evaluate(&repos, "hello").remove(0);
    let config = Config {
        offline: true,
        ..Config::new(dir.path().join("cache"))
    };
    let root = BuildRoot::new(dir.path().join("root"));
    let cache = SourceCache::new(root.sources(), Fetcher::new(config));
    let mut builder = Builder::new(&repos, &cache, &root, Mode::View("/usr".into()));
    builder.jobs = 2;
    let db = root.db();

    let none = BTreeSet::new();
    let (graph, mut schedule) = builder
        .plan(vec![recipe.clone()], &db, &none, &none)
        .expect("the run plans");
    let planned: Vec<String> = schedule
        .planned()
        .iter()
        .map(|(item, triggers)| format!("{item} {triggers:?}"))
        .collect();
    assert_eq!(
        planned,
        [format!("hello {:?}", BTreeSet::from([Trigger::New]))]
    );
    let outcomes = match builder.run(&graph, &mut schedule, &db, &mut |_| {}) {
        Ok(outcomes) => outcomes,
        Err(e) => panic!("the build fails: {e}"),
    };
    let outcome = &outcomes[0];
    let names: Vec<String> = outcome
        .buildinfo
        .outputs
        .iter()
        .map(|o| o.name.to_string())
        .collect();
    assert_eq!(names, ["hello", "hello-dev", "hello-dbg"]);

    let (main, main_entries) = entries(&outcome.packages[0]);
    let paths: BTreeSet<&str> = main_entries.iter().map(Entry::path).collect();
    for path in [
        "usr/bin/hello",
        "usr/lib/libgreet.so.1.0.0",
        "usr/lib/libgreet.so.1",
        "usr/share/licenses/hello/LICENSE",
    ] {
        assert!(
            paths.contains(path),
            "the main package lacks {path}: {paths:?}"
        );
    }
    assert!(!paths.iter().any(|p| p.starts_with("usr/include")));
    assert!(main_entries.iter().any(|e| matches!(
        e,
        Entry::Symlink { path, target } if path == "usr/lib/libgreet.so.1" && target == "libgreet.so.1.0.0"
    )));
    let depends: Vec<String> = main.depends.iter().map(Dep::to_string).collect();
    assert!(depends.contains(&"so:libc.so.6".to_owned()), "{depends:?}");
    assert!(
        !depends.contains(&"so:libgreet.so.1".to_owned()),
        "{depends:?}"
    );
    let provides: Vec<String> = main.provides.iter().map(|p| p.to_string()).collect();
    assert_eq!(provides, ["so:libgreet.so.1"]);
    assert!(main.abi.contains_key("usr/lib/libgreet.so.1.0.0"));

    let (dev, dev_entries) = entries(&outcome.packages[1]);
    let dev_paths: BTreeSet<&str> = dev_entries.iter().map(Entry::path).collect();
    assert!(dev_paths.contains("usr/include/greet.h"));
    assert!(dev_paths.contains("usr/lib/libgreet.so"));
    assert!(dev.depends.iter().any(|d| d.to_string() == "hello=1.0-r1"));

    let (dbg, dbg_entries) = entries(&outcome.packages[2]);
    let debug_files: Vec<&str> = dbg_entries
        .iter()
        .filter(|e| matches!(e, Entry::File { .. }))
        .map(Entry::path)
        .collect();
    assert_eq!(debug_files.len(), 2, "{debug_files:?}");
    assert!(
        debug_files
            .iter()
            .all(|p| p.starts_with("usr/lib/debug/.build-id/") && p.ends_with(".debug"))
    );
    assert!(dbg.depends.iter().any(|d| d.to_string() == "hello=1.0-r1"));
    let main_tree = unpacked(&outcome.packages[0], &dir.path().join("main"));
    let dbg_tree = unpacked(&outcome.packages[2], &dir.path().join("dbg"));
    let program = fs::read(main_tree.join("usr/bin/hello")).unwrap();
    assert!(
        !contains(&program, b".debug_info"),
        "the program is stripped"
    );
    assert!(contains(&program, b".gnu_debuglink"));
    let debug = fs::read(dbg_tree.join(debug_files[0])).unwrap();
    assert!(contains(&debug, b".debug_info"));
    assert!(!contains(&program, b"/build/"));

    assert!(outcome.stored.record.imports["libgreet.so.1"].contains("greet"));
    assert!(outcome.built.exports["libgreet.so.1"].contains("greet"));

    let text = fs::read(root.out().join(&outcome.stored.buildinfo)).unwrap();
    let buildinfo = Buildinfo::from_json(&text).expect("the buildinfo reads back");
    assert_eq!(buildinfo, outcome.buildinfo);
    assert_eq!(buildinfo.file_name(), "hello-1.0-r1.buildinfo.json");
    assert_eq!(buildinfo.build_key, outcome.stored.record.key);
    assert!(!buildinfo.bootstrap);
    assert_eq!(
        buildinfo.env.get("SOURCE_DATE_EPOCH").map(String::as_str),
        Some("315532800")
    );
    let ids: Vec<_> = buildinfo.outputs.iter().map(|o| o.id).collect();
    let recorded: Vec<_> = buildinfo
        .outputs
        .iter()
        .map(|o| outcome.built.outputs[&o.name])
        .collect();
    assert_eq!(ids, recorded);
    assert!(
        !root
            .work()
            .join(&outcome.stored.record.key.to_string()[3..])
            .exists()
    );
    assert!(
        fs::read_dir(root.logs().join("hello+dynamic"))
            .unwrap()
            .next()
            .is_some()
    );

    let (_, again) = builder
        .plan(vec![recipe.clone()], &db, &none, &none)
        .expect("the run plans");
    assert!(
        again.planned().is_empty(),
        "an unchanged recipe is up to date"
    );

    let forced = BTreeSet::from([laft_build::Item::of(&recipe)]);
    let (graph, mut schedule) = builder
        .plan(vec![recipe.clone()], &db, &forced, &none)
        .expect("the run plans");
    let mut cutoffs = Vec::new();
    builder
        .run(&graph, &mut schedule, &db, &mut |event| {
            if let Event::Done { cutoff, .. } = event {
                cutoffs.push(cutoff.clone());
            }
        })
        .expect("the forced build succeeds");
    assert_eq!(
        cutoffs,
        [Cutoff::Unchanged],
        "identical outputs stop the run"
    );

    let previous = db
        .latest(&laft_build::Item::of(&recipe))
        .unwrap()
        .expect("a record");
    let mut second = Builder::new(&repos, &cache, &root, Mode::View("/usr".into()));
    second.jobs = 1;
    second.work = dir.path().join("elsewhere/work");
    second.out = dir.path().join("elsewhere/out");
    second.logs = dir.path().join("elsewhere/logs");
    let report = check_repro(&second, &recipe, &previous, &root.out()).expect("the rebuild works");
    assert!(report.reproducible, "{:?}", report.differing_files);
    assert!(
        report.differing_packages.is_empty(),
        "the package files are byte for byte the same"
    );
    assert_eq!(report.build_key, previous.record.key);
}

#[test]
fn bootstrap_mode_builds_without_a_sandbox_and_says_so() {
    let dir = laft_test::tempdir();
    for tool in ["bash", "gcc", "make", "objcopy", "strip", "install", "ln"] {
        if !Path::new("/usr/bin").join(tool).exists() {
            println!("skipping: /usr/bin/{tool} is missing");
            return;
        }
    }
    let repos = repos(dir.path());
    let recipe: Recipe = support::evaluate(&repos, "hello").remove(0);
    let config = Config {
        offline: true,
        ..Config::new(dir.path().join("cache"))
    };
    let root = BuildRoot::new(dir.path().join("root"));
    let cache = SourceCache::new(root.sources(), Fetcher::new(config));
    let builder = Builder::new(&repos, &cache, &root, Mode::Bootstrap("/".into()));
    let inputs = builder.key_inputs(&recipe).unwrap();
    let outcome = match builder.build(&recipe, &inputs) {
        Ok(outcome) => outcome,
        Err(e) => panic!("the bootstrap build fails: {e}"),
    };
    assert!(outcome.buildinfo.bootstrap);
    assert!(outcome.buildinfo.build_view.is_empty());
    let text = fs::read_to_string(root.out().join(&outcome.stored.buildinfo)).unwrap();
    assert!(text.contains("\"bootstrap\":true"), "{text}");
    let (_, main) = entries(&outcome.packages[0]);
    assert!(main.iter().any(|e| e.path() == "usr/bin/hello"));
    assert!(
        root.store().list().unwrap().is_empty(),
        "bootstrap builds leave the store alone"
    );
}

/// A bootstrap builder for `repos` that keeps everything below `dir` and
/// fetches nothing.
fn offline_cache(dir: &Path) -> (BuildRoot, SourceCache) {
    let config = Config {
        offline: true,
        ..Config::new(dir.join("cache"))
    };
    let root = BuildRoot::new(dir.join("root"));
    let cache = SourceCache::new(root.sources(), Fetcher::new(config));
    (root, cache)
}

#[test]
fn an_installing_bootstrap_build_fills_the_destination_and_records_its_key() {
    let dir = laft_test::tempdir();
    for tool in ["bash", "cc", "make", "install", "ln"] {
        if !Path::new("/usr/bin").join(tool).exists() {
            println!("skipping: /usr/bin/{tool} is missing");
            return;
        }
    }
    let repos = repos(dir.path());
    let recipe: Recipe = support::evaluate(&repos, "hello").remove(0);
    let (root, cache) = offline_cache(dir.path());
    let builder = Builder::new(&repos, &cache, &root, Mode::Bootstrap("/".into()));
    let inputs = builder.key_inputs(&recipe).unwrap();
    let dest = dir.path().join("system");
    let item = Item::of(&recipe);
    assert_eq!(installed(&dest, &item).unwrap(), None);
    if let Err(e) = builder.install(&recipe, &inputs, &dest) {
        panic!("the installing build fails: {e}");
    }
    for file in ["usr/bin/hello", "usr/lib/libgreet.so.1.0.0"] {
        assert!(dest.join(file).is_file(), "{file} is not installed");
    }
    assert!(dest.join("usr/include/greet.h").is_file());
    assert_eq!(installed(&dest, &item).unwrap(), Some(inputs.build_key()));
    assert!(
        !root.out().exists() || fs::read_dir(root.out()).unwrap().next().is_none(),
        "installing writes no package"
    );
    assert!(!builder.work_dir(&inputs.build_key()).exists());
}

#[test]
fn an_unpacking_bootstrap_build_makes_packages_and_fills_the_destination() {
    let dir = laft_test::tempdir();
    for tool in ["bash", "gcc", "make", "objcopy", "strip", "install", "ln"] {
        if !Path::new("/usr/bin").join(tool).exists() {
            println!("skipping: /usr/bin/{tool} is missing");
            return;
        }
    }
    let repos = repos(dir.path());
    let recipe: Recipe = support::evaluate(&repos, "hello").remove(0);
    let (root, cache) = offline_cache(dir.path());
    let builder = Builder::new(&repos, &cache, &root, Mode::Bootstrap("/".into()));
    let inputs = builder.key_inputs(&recipe).unwrap();
    let dest = dir.path().join("system");
    let item = Item::of(&recipe);
    let outcome = match builder.build_into(&recipe, &inputs, &dest) {
        Ok(outcome) => outcome,
        Err(e) => panic!("the unpacking build fails: {e}"),
    };
    let names: Vec<&str> = outcome
        .buildinfo
        .outputs
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert!(names.contains(&"hello-dbg"), "{names:?}");
    assert!(names.contains(&"hello-dev"), "{names:?}");
    assert!(outcome.packages.iter().all(|p| p.is_file()));
    for file in [
        "usr/bin/hello",
        "usr/lib/libgreet.so.1.0.0",
        "usr/include/greet.h",
    ] {
        assert!(dest.join(file).is_file(), "{file} is not unpacked");
    }
    assert!(
        !dest.join("usr/lib/debug").exists(),
        "the debug info stays out"
    );
    assert_eq!(installed(&dest, &item).unwrap(), Some(inputs.build_key()));

    let other = Builder::new(&repos, &cache, &root, Mode::View("/usr".into()));
    let error = other
        .build_into(&recipe, &inputs, &dest)
        .expect_err("only bootstrap mode unpacks");
    assert_eq!(error.code(), Some("B041"));
}

#[test]
fn an_unpacking_build_unpacks_an_explicit_dbg_subpackage() {
    let dir = laft_test::tempdir();
    for tool in ["bash", "gcc", "make", "objcopy", "strip", "install", "ln"] {
        if !Path::new("/usr/bin").join(tool).exists() {
            println!("skipping: /usr/bin/{tool} is missing");
            return;
        }
    }
    let repos = repos_with(
        dir.path(),
        "  check = false\n  build make {}\n  subpackage dbg {\n    files = [\"usr/include/**\"]\n  }",
    );
    let recipe: Recipe = support::evaluate(&repos, "hello").remove(0);
    let (root, cache) = offline_cache(dir.path());
    let builder = Builder::new(&repos, &cache, &root, Mode::Bootstrap("/".into()));
    let inputs = builder.key_inputs(&recipe).unwrap();
    let dest = dir.path().join("system");
    let outcome = match builder.build_into(&recipe, &inputs, &dest) {
        Ok(outcome) => outcome,
        Err(e) => panic!("the unpacking build fails: {e}"),
    };
    assert!(
        outcome
            .buildinfo
            .outputs
            .iter()
            .any(|o| o.name.as_str() == "hello-dbg"),
        "{:?}",
        outcome.buildinfo.outputs
    );
    assert!(
        dest.join("usr/include/greet.h").is_file(),
        "a dbg subpackage the recipe declares is unpacked"
    );
}

#[test]
fn a_failed_install_forgets_the_earlier_one() {
    let dir = laft_test::tempdir();
    if !Path::new("/bin/sh").exists() {
        println!("skipping: the host has no /bin/sh");
        return;
    }
    let dest = dir.path().join("system");
    let (root, cache) = offline_cache(dir.path());
    let attempt = |name: &str, install: &str| {
        let repos = repos_with(
            &dir.path().join(name),
            &format!("  build custom {{}}\n  phases {{\n    install = \"{install}\"\n  }}"),
        );
        let recipe: Recipe = support::evaluate(&repos, "hello").remove(0);
        let builder = Builder::new(&repos, &cache, &root, Mode::Bootstrap("/".into()));
        let inputs = builder.key_inputs(&recipe).unwrap();
        let result = builder.install(&recipe, &inputs, &dest);
        (Item::of(&recipe), inputs.build_key(), result)
    };
    let (item, key, result) = attempt("good", "true");
    result.unwrap();
    assert_eq!(installed(&dest, &item).unwrap(), Some(key));
    let (item, _, result) = attempt("bad", "false");
    result.expect_err("the install phase fails");
    assert_eq!(
        installed(&dest, &item).unwrap(),
        None,
        "the destination may hold files of the failed install"
    );
}

#[test]
fn an_installing_build_sees_the_destination_and_no_toolchain() {
    let dir = laft_test::tempdir();
    let repos = repos_with(
        dir.path(),
        "  build custom {}\n  phases {\n    install = \"printenv > \\\"$DESTDIR/env\\\"\"\n  }",
    );
    let recipe: Recipe = support::evaluate(&repos, "hello").remove(0);
    let (root, cache) = offline_cache(dir.path());
    let mut builder = Builder::new(&repos, &cache, &root, Mode::Bootstrap("/".into()));
    let tools = dir.path().join("system/tools/bin");
    builder.search = vec![tools.clone()];
    let inputs = builder.key_inputs(&recipe).unwrap();
    let dest = dir.path().join("system");
    builder.install(&recipe, &inputs, &dest).unwrap();
    let env = fs::read_to_string(dest.join("env")).unwrap();
    let lines: Vec<&str> = env.lines().collect();
    assert!(
        lines.contains(&format!("DESTDIR={}", dest.display()).as_str()),
        "{env}"
    );
    assert!(
        lines.contains(&format!("PATH={}:/usr/bin", tools.display()).as_str()),
        "{env}"
    );
    for key in ["CC", "CXX", "CFLAGS", "LDFLAGS", "CHOST"] {
        assert!(
            !lines.iter().any(|l| l.starts_with(&format!("{key}="))),
            "{env}"
        );
    }

    let other = Builder::new(&repos, &cache, &root, Mode::View("/usr".into()));
    let error = other
        .install(&recipe, &inputs, &dest)
        .expect_err("only bootstrap mode installs");
    assert_eq!(error.code(), Some("B041"));
}

#[test]
fn the_check_phase_installs_nothing_into_the_packages() {
    let dir = laft_test::tempdir();
    if let Some(reason) = unsupported(dir.path()) {
        println!("skipping: {reason}");
        return;
    }
    let repos = repos_with(
        dir.path(),
        "  build make {}\n  phases {\n    check = \"test -z \\\"${DESTDIR:-}${OUT:-}\\\"\"\n  }",
    );
    let recipe: Recipe = support::evaluate(&repos, "hello").remove(0);
    let (root, cache) = offline_cache(dir.path());
    let builder = Builder::new(&repos, &cache, &root, Mode::View("/usr".into()));
    let inputs = builder.key_inputs(&recipe).unwrap();
    if let Err(e) = builder.build(&recipe, &inputs) {
        panic!("the build fails: {e}");
    }
}
