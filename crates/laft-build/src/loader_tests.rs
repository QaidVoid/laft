use std::os::unix::fs::symlink;

use super::*;

fn rel(path: &str) -> RelPath {
    RelPath::new(path).expect("a relative path")
}

fn setup() -> (laft_test::TempDir, PathBuf) {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("core");
    for (path, text) in [
        ("recipes.stav", "# descriptor\n"),
        ("recipes/foo/package.stav", "# foo\n"),
        ("recipes/bar/package.stav", "# bar\n"),
        ("common/lib.stav", "# lib\n"),
        ("common/net/http.stav", "# http\n"),
    ] {
        let path = repo.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }
    fs::write(dir.path().join("outside.stav"), "# outside\n").expect("write");
    (dir, repo)
}

#[test]
fn a_package_reads_its_recipe_and_the_shared_modules_only() {
    let (_dir, repo) = setup();
    let loader = RepoLoader::new(vec![repo.clone()], Scope::Package("foo".into()));
    let root = ImportRoot::new(&repo);
    for (path, text) in [
        ("recipes/foo/package.stav", "# foo\n"),
        ("common/lib.stav", "# lib\n"),
        ("common/net/http.stav", "# http\n"),
    ] {
        assert_eq!(
            loader.read(&root, &rel(path)).as_deref(),
            Ok(text),
            "{path}"
        );
    }
    for path in ["recipes/bar/package.stav", "recipes.stav", "common"] {
        assert!(
            matches!(loader.read(&root, &rel(path)), Err(LoadError::Other(_))),
            "{path}"
        );
    }
}

#[test]
fn a_descriptor_reads_only_itself() {
    let (_dir, repo) = setup();
    let loader = RepoLoader::new(vec![repo.clone()], Scope::Descriptor);
    let root = ImportRoot::new(&repo);
    assert_eq!(
        loader.read(&root, &rel("recipes.stav")).as_deref(),
        Ok("# descriptor\n")
    );
    assert!(loader.read(&root, &rel("common/lib.stav")).is_err());
}

#[test]
fn symbolic_links_and_other_roots_cannot_leave_the_repository() {
    let (dir, repo) = setup();
    symlink(
        dir.path().join("outside.stav"),
        repo.join("common/out.stav"),
    )
    .expect("symlink");
    let loader = RepoLoader::new(vec![repo.clone()], Scope::Package("foo".into()));
    assert_eq!(
        loader.read(&ImportRoot::new(&repo), &rel("common/out.stav")),
        Err(LoadError::OutsideRoot)
    );
    assert_eq!(
        loader.read(&ImportRoot::new(dir.path()), &rel("outside.stav")),
        Err(LoadError::OutsideRoot)
    );
    assert!(
        loader
            .std_module("build")
            .is_some_and(|s| s.contains("fn gnu_mirrors("))
    );
    assert!(loader.std_module("services").is_none());
}
