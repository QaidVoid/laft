use std::fs;

use super::*;

fn source_tree(dir: &std::path::Path) -> std::path::PathBuf {
    let recipes = dir.join("recipes-src");
    fs::create_dir_all(recipes.join("recipes/mesa")).unwrap();
    fs::write(recipes.join("recipes.stav"), "recipe_repo core {}\n").unwrap();
    fs::write(
        recipes.join("recipes/mesa/package.stav"),
        "package mesa {}\n",
    )
    .unwrap();
    fs::write(recipes.join("recipes/mesa/build.sh"), "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            recipes.join("recipes/mesa/build.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    recipes
}

#[test]
fn the_same_tree_packs_to_the_same_archive() {
    let dir = laft_test::tempdir();
    let tree = source_tree(dir.path());
    let repo = Repository::new(dir.path().join("repo"));
    let first = write(&repo, &tree).unwrap();
    let second = write(&repo, &tree).unwrap();
    assert_eq!(first, second);
    let bytes = fs::read(repo.path(&first.file)).unwrap();
    assert_eq!(bytes.len() as u64, first.size);
    assert!(first.file.starts_with("recipes/b3:"));

    let changed = source_tree(dir.path());
    fs::write(
        changed.join("recipes/mesa/package.stav"),
        "package mesa { }\n",
    )
    .unwrap();
    let third = write(&repo, &changed).unwrap();
    assert_ne!(third.blake3, first.blake3);
}

#[test]
fn the_archive_entries_are_sorted_and_normalized() {
    let dir = laft_test::tempdir();
    let tree = source_tree(dir.path());
    let repo = Repository::new(dir.path().join("repo"));
    let snapshot = write(&repo, &tree).unwrap();
    let bytes = fs::read(repo.path(&snapshot.file)).unwrap();
    let tar = zstd::stream::decode_all(&bytes[..]).unwrap();
    let mut archive = tar::Archive::new(&tar[..]);
    let mut entries = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        entries.push((
            entry.path().unwrap().to_string_lossy().into_owned(),
            entry.header().mode().unwrap(),
            entry.header().mtime().unwrap(),
        ));
    }
    let mut sorted = entries.clone();
    sorted
        .sort_by_key(|(path, mode, mtime)| (path.trim_end_matches('/').to_owned(), *mode, *mtime));
    assert_eq!(entries, sorted);
    assert!(entries.iter().all(|(_, _, mtime)| *mtime == 315_532_800));
    let build_sh = entries
        .iter()
        .find(|(path, _, _)| path == "recipes/mesa/build.sh")
        .unwrap();
    assert_eq!(build_sh.1, 0o755);
}
