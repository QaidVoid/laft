//! Builds a small C package from a local tarball with the host's compiler
//! as the build view, in the sandbox, and checks the packages, their
//! dependencies, the debug split, the buildinfo, early cutoff and that two
//! builds give the same store ids. Parallel runs keep the builds a package
//! depends on ahead of it, share one job budget through a jobserver, and
//! follow the memory and worker limits of the host.
//!
//! The test is skipped with a reason when the machine cannot run
//! sandboxes or has no C toolchain in `/usr/bin`.

mod support;

use std::collections::{BTreeMap, BTreeSet};
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
    tarball_of("hello-1.0", FILES)
}

/// A gzip compressed tarball of `files` below `name/`.
fn tarball_of(name: &str, files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (file, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{name}/{file}"), text.as_bytes())
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
    let outcomes = match builder.run(&graph, &mut schedule, &db, &|_| {}) {
        Ok(outcomes) => outcomes,
        Err(e) => panic!("the build fails: {}", e.error),
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
    let cutoffs = std::sync::Mutex::new(Vec::new());
    builder
        .run(&graph, &mut schedule, &db, &|event| {
            if let Event::Done { cutoff, .. } = event {
                cutoffs.lock().unwrap().push(cutoff.clone());
            }
        })
        .expect("the forced build succeeds");
    let cutoffs = cutoffs.into_inner().unwrap();
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

/// A recipe repository whose `packages` are `(name, build_deps)` pairs
/// that share the tarball `files` from a `file://` mirror below `dir`,
/// with `body` rendered per package name after the source block.
fn graph_repos(
    dir: &Path,
    files: &[(&str, &str)],
    packages: &[(&str, &[&str])],
    body: &dyn Fn(&str) -> String,
) -> Repos {
    let bytes = tarball_of("hello-1.0", files);
    let hex: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mirror = dir.join("mirror");
    fs::create_dir_all(mirror.join("sha256")).expect("the test setup works");
    fs::write(mirror.join("sha256").join(&hex), bytes).expect("the test setup works");
    let mut recipes = Vec::new();
    for (name, deps) in packages {
        let mut text = format!(
            "  source main {{\n    url = [\"https://example.org/hello-1.0.tar.gz\"]\n    \
             sha256 = \"sha256:{hex}\"\n  }}\n"
        );
        if !deps.is_empty() {
            text.push_str(&format!("  build_deps = [{}]\n", deps.join(", ")));
        }
        text.push_str(&body(name));
        recipes.push((
            format!("recipes/{name}/package.stav"),
            support::recipe(name, &text),
        ));
    }
    let repo = support::write_repo(
        &dir.join("core"),
        "core",
        &format!("  source_mirrors = [\"file://{}\"]\n", mirror.display()),
        &recipes
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect::<Vec<_>>(),
    );
    Repos::open(&[repo]).expect("the repository opens")
}

/// The recipes of `packages`, one per package.
fn evaluated(repos: &Repos, packages: &[(&str, &[&str])]) -> Vec<Recipe> {
    packages
        .iter()
        .map(|(name, _)| support::evaluate(repos, name).remove(0))
        .collect()
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

/// The files of the host's `tools`, with the loader and the libraries
/// they need, laid out below `usr` so that they run inside a store view.
fn host_tool_files(tools: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut queue = Vec::new();
    for (name, host) in tools {
        let host = fs::canonicalize(host).expect("the host has the tool");
        files.insert(
            format!("usr/bin/{name}"),
            fs::read(&host).expect("the host tool reads"),
        );
        queue.push(host);
    }
    let mut seen = BTreeSet::new();
    while let Some(file) = queue.pop() {
        let Some(elf) =
            laft_elf::inspect(&fs::read(&file).expect("the file reads")).expect("the ELF inspects")
        else {
            continue;
        };
        let found = elf.needed.iter().filter_map(|lib| {
            LIBRARY_DIRS
                .iter()
                .map(|d| Path::new(d).join(lib.as_str()))
                .find(|path| path.exists())
        });
        for dep in elf.interpreter.iter().map(PathBuf::from).chain(found) {
            if !seen.insert(dep.clone()) {
                continue;
            }
            let host = dep.to_str().expect("a UTF-8 library path");
            let host = host.strip_prefix('/').expect("an absolute library path");
            let under = host.strip_prefix("usr/").unwrap_or(host);
            let rel = match under.split_once('/') {
                Some(("lib" | "lib64", tail)) => format!("usr/lib/{tail}"),
                _ => format!("usr/{under}"),
            };
            files.insert(rel, fs::read(&dep).expect("the library reads"));
            queue.push(dep);
        }
    }
    files.into_iter().collect()
}

/// Adds a package `name` holding `files` to the store of `root`, with one
/// file `usr/share/<name>/file` when `files` is empty.
fn add_package(root: &BuildRoot, dir: &Path, name: &str, files: &[(String, Vec<u8>)]) {
    use std::os::unix::fs::PermissionsExt;
    let payload = dir.join(format!("payload-{name}"));
    let default = format!("usr/share/{name}/file");
    let own: Vec<(String, Vec<u8>)> = if files.is_empty() {
        vec![(default, name.as_bytes().to_vec())]
    } else {
        files.to_vec()
    };
    for (rel, bytes) in &own {
        let path = payload.join(rel);
        fs::create_dir_all(path.parent().expect("a file has a parent")).expect("writable");
        fs::write(&path, bytes).expect("writable");
        let mode = if rel.starts_with("usr/bin/") || bytes.starts_with(b"\x7fELF") {
            0o555
        } else {
            0o444
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
    }
    let mut meta = laft_pkg::Meta {
        format: laft_pkg::FORMAT,
        name: name.parse().expect("a valid name"),
        version: "1.0".parse().expect("a valid version"),
        rev: 1,
        variant: laft_core::Variant::Dynamic,
        arch: laft_pkg::Arch::X86_64V3,
        origin: name.parse().expect("a valid name"),
        summary: "A test package".into(),
        homepage: None,
        license: vec!["MIT".into()],
        depends: vec![],
        provides: vec![],
        conflicts: vec![],
        replaces: vec![],
        alternatives: vec![],
        triggers: vec![],
        static_deps: vec![],
        abi: Default::default(),
        abi_epoch: 2,
        requested_privileges: vec![],
        installed_size: 0,
    };
    let entries = laft_pkg::scan(&payload).expect("scannable");
    meta.installed_size = entries
        .iter()
        .map(|e| match e {
            laft_pkg::Entry::File { size, .. } => *size,
            _ => 0,
        })
        .sum();
    let manifest = laft_pkg::Manifest::new(
        meta.identity(),
        laft_core::Blake3Hash::of(&meta.to_json().expect("valid meta")),
        entries,
    )
    .expect("a valid manifest");
    let file = dir.join(manifest.package().file_name());
    let mut out = fs::File::create(&file).expect("writable");
    laft_pkg::write_package(
        &mut out,
        &payload,
        &meta,
        &manifest,
        laft_pkg::Compression::DEVELOPMENT,
    )
    .expect("the package is written");
    root.store()
        .insert(&file, &manifest.store_id(), &dir.join("tmp"))
        .expect("the package enters the store");
}

/// Fills the store of `root` with the build base, `dash` carrying the
/// host's `sh`, `make` and `flock` with the libraries they need.
fn seed_store(root: &BuildRoot, dir: &Path) {
    let tools = host_tool_files(&[
        ("sh", "/bin/sh"),
        ("make", "/usr/bin/make"),
        ("flock", "/usr/bin/flock"),
        ("sleep", "/usr/bin/sleep"),
        ("mkdir", "/usr/bin/mkdir"),
        ("cat", "/usr/bin/cat"),
        ("dd", "/usr/bin/dd"),
        ("wc", "/usr/bin/wc"),
    ]);
    for name in laft_build::view::BUILD_BASE {
        let own: &[(String, Vec<u8>)] = if *name == "dash" { &tools } else { &[] };
        add_package(root, dir, name, own);
    }
}

/// The gigabytes of memory the host holds for new work, rounded down.
fn available_gib() -> u64 {
    fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("MemAvailable:"))
                .and_then(|line| line.split_whitespace().nth(1).map(String::from))
        })
        .and_then(|kib| kib.parse::<u64>().ok())
        .map_or(0, |kib| kib / (1 << 20))
}

/// The `(time, change)` events of the metered jobs of every kept working
/// tree, one timeline per build.
fn meter_timelines(root: &BuildRoot) -> Vec<Vec<(u64, i64)>> {
    let mut timelines = Vec::new();
    for entry in fs::read_dir(root.work()).expect("the working trees are kept") {
        let path = entry.expect("a work tree").path().join("build/tmp/events");
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let timeline = text
            .lines()
            .map(|line| {
                let (time, change) = line.split_once(' ').expect("an event names a time");
                (
                    time.parse().expect("the time is a number"),
                    change.parse().expect("the change is a number"),
                )
            })
            .collect();
        timelines.push(timeline);
    }
    timelines
}

/// The most jobs that ran at once over the `timelines` of one or more
/// builds.
fn peak_of(timelines: &[Vec<(u64, i64)>]) -> i64 {
    let mut events: Vec<(u64, i64)> = timelines.iter().flatten().copied().collect();
    // A job that ends when another starts counts once, not twice, so a
    // handing-over ends before the next job starts.
    events.sort_by_key(|(time, change)| (*time, *change));
    let mut running = 0;
    let mut peak = 0;
    for (_, change) in events {
        running += change;
        peak = peak.max(running);
    }
    peak
}

#[test]
fn a_run_builds_a_package_only_after_the_packages_it_depends_on() {
    let dir = laft_test::tempdir();
    if let Some(reason) = unsupported(dir.path()) {
        println!("skipping: {reason}");
        return;
    }
    let packages: &[(&str, &[&str])] = &[
        ("a", &[]),
        ("b", &["a"]),
        ("c", &["a"]),
        ("d", &["b", "c"]),
        ("e", &[]),
        ("f", &["e"]),
        ("g", &["f"]),
    ];
    let repos = graph_repos(dir.path(), FILES, packages, &marker_phase);
    let (root, cache) = offline_cache(dir.path());
    let mut builder = Builder::new(&repos, &cache, &root, Mode::View("/usr".into()));
    builder.jobs = 4;
    builder.workers = 4;
    let db = root.db();
    let none = BTreeSet::new();
    let (graph, mut schedule) = builder
        .plan(evaluated(&repos, packages), &db, &none, &none)
        .expect("the run plans");
    let events = std::sync::Mutex::new(Vec::new());
    if let Err(e) = builder.run(&graph, &mut schedule, &db, &|event| {
        let mut events = events.lock().unwrap();
        match event {
            Event::Start { item, .. } => events.push(("start", item.to_string())),
            Event::Done { item, .. } => events.push(("done", item.to_string())),
            Event::Waiting { .. } => {}
        }
    }) {
        panic!("the run fails: {}", e.error);
    }
    let events = events.into_inner().unwrap();
    assert_eq!(
        events.len(),
        2 * packages.len(),
        "every build starts and finishes: {events:?}"
    );
    for (kind, item) in &events {
        assert_eq!(
            events
                .iter()
                .filter(|(k, i)| k == kind && i == item)
                .count(),
            1,
            "{item} reports {kind} once: {events:?}"
        );
    }
    for (index, (kind, item)) in events.iter().enumerate() {
        if *kind != "start" {
            continue;
        }
        let deps = packages
            .iter()
            .find(|(name, _)| name == item)
            .expect("a fixture package")
            .1;
        for dep in deps {
            assert!(
                events[..index]
                    .iter()
                    .any(|(kind, done)| *kind == "done" && done == dep),
                "{item} starts before {dep} is done: {events:?}"
            );
        }
    }
}

/// Builds the graph `packages` with `workers`, and returns the build key
/// and the output ids of every package.
fn graph_records(
    dir: &Path,
    packages: &[(&str, &[&str])],
    workers: usize,
) -> BTreeMap<String, (String, Vec<String>)> {
    let repos = graph_repos(dir, FILES, packages, &marker_phase);
    let (root, cache) = offline_cache(dir);
    let mut builder = Builder::new(&repos, &cache, &root, Mode::View("/usr".into()));
    builder.jobs = 4;
    builder.workers = workers;
    let db = root.db();
    let none = BTreeSet::new();
    let (graph, mut schedule) = builder
        .plan(evaluated(&repos, packages), &db, &none, &none)
        .expect("the run plans");
    let records = std::sync::Mutex::new(BTreeMap::new());
    if let Err(e) = builder.run(&graph, &mut schedule, &db, &|event| {
        if let Event::Done { item, outcome, .. } = event {
            let mut outputs: Vec<String> = outcome
                .built
                .outputs
                .iter()
                .map(|(name, id)| format!("{name} {id}"))
                .collect();
            outputs.sort();
            records.lock().expect("the records unlock").insert(
                item.to_string(),
                (outcome.stored.record.key.to_string(), outputs),
            );
        }
    }) {
        panic!("the run fails: {}", e.error);
    }
    records.into_inner().expect("the records unlock")
}

#[test]
fn a_run_gives_the_same_records_with_one_and_four_workers() {
    let dir = laft_test::tempdir();
    if let Some(reason) = unsupported(dir.path()) {
        println!("skipping: {reason}");
        return;
    }
    let packages: &[(&str, &[&str])] = &[
        ("a", &[]),
        ("b", &["a"]),
        ("c", &["a"]),
        ("d", &["b", "c"]),
        ("e", &[]),
        ("f", &["e"]),
        ("g", &["f"]),
    ];
    let one = graph_records(&dir.path().join("one"), packages, 1);
    let four = graph_records(&dir.path().join("four"), packages, 4);
    assert_eq!(one.len(), packages.len(), "every package has a record");
    assert_eq!(one, four, "the same build keys and outputs");
}

/// The `make` recipe of the job meter: eight jobs that each count
/// themselves up under a `flock` guard, sleep, and count themselves down
/// again, writing each change with the time of `/proc/uptime`, so the
/// highest count is the most jobs `make` ran at once.
const METER_MAKEFILE: &str = "all: job1 job2 job3 job4 job5 job6 job7 job8
job1 job2 job3 job4 job5 job6 job7 job8:
\t@flock \"$$TMPDIR/lock\" sh -c 'u=$$(cat /proc/uptime); u=$${u%% *}; e=$${u%.*}$${u#*.}; \
echo \"$$e +1\" >> \"$$TMPDIR/events\"; \
n=$$(cat \"$$TMPDIR/n\" 2>/dev/null || echo 0); \
n=$$((n+1)); echo $$n > \"$$TMPDIR/n\"; \
m=$$(cat \"$$TMPDIR/m\" 2>/dev/null || echo 0); \
if [ $$n -gt $$m ]; then echo $$n > \"$$TMPDIR/m\"; fi'
\t@sleep 0.2
\t@flock \"$$TMPDIR/lock\" sh -c 'u=$$(cat /proc/uptime); u=$${u%% *}; e=$${u%.*}$${u#*.}; \
echo \"$$e -1\" >> \"$$TMPDIR/events\"; \
echo $$(( $$(cat \"$$TMPDIR/n\") - 1 )) > \"$$TMPDIR/n\"'
.PHONY: all job1 job2 job3 job4 job5 job6 job7 job8
";

#[test]
fn the_builds_of_a_store_run_share_one_job_budget() {
    let dir = laft_test::tempdir();
    if let Some(reason) = unsupported(dir.path()) {
        println!("skipping: {reason}");
        return;
    }
    for tool in ["make", "flock"] {
        if !Path::new("/usr/bin").join(tool).exists() {
            println!("skipping: /usr/bin/{tool} is missing");
            return;
        }
    }
    // GNU make 4.3 and earlier do not take the spare tokens of a
    // jobserver, so their meter stays flat.
    match make_version() {
        Some(version) if version >= (4, 4) => {}
        other => {
            println!("skipping: make {other:?} shares no jobserver tokens");
            return;
        }
    }
    let packages: &[(&str, &[&str])] = &[
        ("meter-a", &[]),
        ("meter-b", &[]),
        ("meter-c", &[]),
        ("meter-d", &[]),
    ];
    let meter = &[("Makefile", METER_MAKEFILE), ("LICENSE", FILES[4].1)];
    let repos = graph_repos(dir.path(), meter, packages, &|_| {
        "  build custom {}\n  env = { LD_LIBRARY_PATH = \"/usr/lib\" }\n  phases {\n    \
         build = \"test -p /build/jobserver\"\n    install = \"cd \\\"$SRC\\\" && make \
         && mkdir -p \\\"$OUT/usr/share\\\" && echo m > \\\"$OUT/usr/share/m\\\"\"\n  }\n"
            .to_owned()
    });
    let (root, cache) = offline_cache(dir.path());
    seed_store(&root, dir.path());
    let mut builder = Builder::new(&repos, &cache, &root, Mode::Store);
    builder.jobs = 6;
    builder.workers = 3;
    builder.keep_work = true;
    builder.share_jobs().expect("the jobserver opens");
    let db = root.db();
    let none = BTreeSet::new();
    let (graph, mut schedule) = builder
        .plan(evaluated(&repos, packages), &db, &none, &none)
        .expect("the run plans");
    if let Err(e) = builder.run(&graph, &mut schedule, &db, &|_| {}) {
        panic!("the run fails: {}", e.error);
    }
    let timelines = meter_timelines(&root);
    assert_eq!(timelines.len(), packages.len(), "every build keeps a meter");
    for timeline in &timelines {
        let peak = peak_of(std::slice::from_ref(timeline));
        assert!(peak >= 2, "make runs more than one job at once: {peak}");
        assert!(
            peak <= 4,
            "make sees more than its one job and the three spare tokens: {peak}"
        );
    }
    // Every build draws its jobs from the one jobserver of the run, so
    // the jobs of all builds together never exceed the budget, and the
    // builds do run side by side.
    let shared = peak_of(&timelines);
    assert!(shared >= 2, "two builds run side by side: {shared}");
    assert!(
        shared <= 6,
        "the builds together run more than the six jobs of the run: {shared}"
    );
}

/// The version of the host `make`, as `(major, minor)`.
fn make_version() -> Option<(u32, u32)> {
    let output = std::process::Command::new("/usr/bin/make")
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.strip_prefix("GNU Make ")?.split([' ', '-']).next()?;
    let mut numbers = version.split('.');
    let major = numbers.next()?.parse().ok()?;
    let minor = numbers.next()?.parse().ok()?;
    Some((major, minor))
}

/// The install phase of a package that writes its own marker file.
fn marker_phase(name: &str) -> String {
    format!(
        "  build custom {{}}\n  phases {{\n    install = \"mkdir -p \\\"$OUT/usr/share\\\" \
         && echo {name} > \\\"$OUT/usr/share/{name}\\\"\"\n  }}\n"
    )
}

#[test]
fn a_big_build_runs_alone_while_small_builds_share_the_workers() {
    let dir = laft_test::tempdir();
    if let Some(reason) = unsupported(dir.path()) {
        println!("skipping: {reason}");
        return;
    }
    if available_gib() < 4 {
        let free = available_gib();
        println!("skipping: the host holds {free} GiB, a second build needs three");
        return;
    }
    let packages: &[(&str, &[&str])] = &[
        ("big-a", &[]),
        ("big-b", &[]),
        ("small-a", &[]),
        ("small-b", &[]),
    ];
    let repos = graph_repos(dir.path(), FILES, packages, &|name| {
        let runner = if name.starts_with("big") {
            "  ci { runner = big }\n"
        } else {
            ""
        };
        format!("{}{}", runner, marker_phase(name))
    });
    let (root, cache) = offline_cache(dir.path());
    let mut builder = Builder::new(&repos, &cache, &root, Mode::View("/usr".into()));
    builder.jobs = 4;
    builder.workers = 4;
    builder.min_available = 0;
    let db = root.db();
    let none = BTreeSet::new();
    let (graph, mut schedule) = builder
        .plan(evaluated(&repos, packages), &db, &none, &none)
        .expect("the run plans");
    let events = std::sync::Mutex::new(Vec::new());
    if let Err(e) = builder.run(&graph, &mut schedule, &db, &|event| {
        let mut events = events.lock().unwrap();
        match event {
            Event::Start { item, .. } => events.push(("start", item.to_string())),
            Event::Done { item, .. } => events.push(("done", item.to_string())),
            Event::Waiting { .. } => {}
        }
    }) {
        panic!("the run fails: {}", e.error);
    }
    let events = events.into_inner().unwrap();
    let mut running: BTreeSet<String> = BTreeSet::new();
    let mut running_big = 0;
    for (kind, item) in &events {
        let big = item.starts_with("big");
        if *kind == "start" {
            if big {
                assert_eq!(running_big, 0, "a big build runs alone: {events:?}");
                running_big += 1;
            }
            running.insert(item.clone());
        } else {
            if big {
                running_big -= 1;
            }
            running.remove(item);
        }
    }
    assert!(running.is_empty(), "every build finishes: {events:?}");
    for name in ["small-a", "small-b"] {
        assert!(
            events
                .iter()
                .take(4)
                .any(|(kind, item)| *kind == "start" && item == name),
            "both small builds start while the first big build runs: {events:?}"
        );
    }
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
