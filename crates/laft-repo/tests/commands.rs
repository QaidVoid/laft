//! `index`, `sign`, `yank`, `promote`, `keys` and `publish`, run as the
//! binary against temporary repositories.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use laft_core::Timestamp;
use laft_index::{Index, IndexComment, Keys, KeysComment, KeysFetch, Signature};
use support::{DAY, HOUR, T0, at, fails, keygen, ok, pkg, public, read_index, run, s};

const INDEX: &str = "channels/current/x86_64-v3/index.json.zst";

fn signature(path: &Path) -> Signature {
    let bytes = fs::read(path).expect("the signature file exists");
    Signature::decode(&bytes).expect("laft-repo writes valid signatures")
}

fn index_args<'a>(repo: &'a Path, channel: &'a str, now: &'a str) -> Vec<&'a str> {
    vec![
        "index",
        "--repo",
        s(repo),
        "--name",
        "core",
        "--channel",
        channel,
        "--now",
        now,
    ]
}

#[test]
fn index_copies_packages_by_store_id_and_signs_a_new_generation() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").file("usr/bin/hello", "hi").write(&out);
    let repo = dir.path().join("repo");
    let key = keygen(dir.path(), "online");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.extend(["--key", s(&key), s(&out)]);
    let stdout = ok(&args);
    assert!(
        stdout.contains(&format!("generation {T0}: 1 packages, 1 new")),
        "{stdout}"
    );

    let index = read_index(&repo, "current");
    assert_eq!(index.generation, T0);
    assert_eq!(index.expires, Timestamp::from_secs(T0 + 14 * DAY));
    let entry = &index.packages[0];
    assert_eq!(entry.file, format!("pool/{}.lpkg", entry.id));
    assert!(repo.join(&entry.file).is_file());
    assert!(repo.join(format!("buildinfo/{}.json", entry.id)).is_file());

    let bytes = fs::read(repo.join(INDEX)).unwrap();
    let signature = signature(&repo.join(format!("{INDEX}.minisig")));
    assert!(public(&key).verifies(&bytes, &signature));
    let comment: IndexComment = signature.trusted_comment().parse().unwrap();
    assert_eq!(comment, index.comment());
    let snapshot = repo.join(format!(
        "channels/current/x86_64-v3/snapshots/{T0}.json.zst"
    ));
    assert_eq!(fs::read(&snapshot).unwrap(), bytes);
    assert_eq!(
        fs::read(snapshot.with_extension("zst.minisig")).unwrap(),
        fs::read(repo.join(format!("{INDEX}.minisig"))).unwrap()
    );
}

/// Writes a recipe repository declaring the profile `minimal`, which sets
/// `x11 = false` on mesa.
fn profile_repo(dir: &Path) -> PathBuf {
    let recipes = dir.join("recipes-src");
    fs::create_dir_all(&recipes).expect("writable");
    fs::write(
        recipes.join("recipes.stav"),
        "recipe_repo core {}\n\nprofile minimal {\n  package mesa {\n    options {\n      x11 = false\n    }\n  }\n}\n",
    )
    .expect("writable");
    recipes
}

#[test]
fn index_writes_a_profile_index_with_the_profile_member() {
    let dir = laft_test::tempdir();
    let recipes = profile_repo(dir.path());
    let out = dir.path().join("out");
    pkg("hello", "1.0").file("usr/bin/hello", "hi").write(&out);
    let repo = dir.path().join("repo");
    let key = keygen(dir.path(), "online");
    let now = at(0);
    let stdout = ok(&[
        "index",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--channel",
        "current",
        "--profile",
        "minimal",
        "--recipes",
        s(&recipes),
        "--now",
        &now,
        "--key",
        s(&key),
        s(&out),
    ]);
    assert!(
        stdout.contains("current/x86_64-v3 profiles/minimal"),
        "{stdout}"
    );

    let entry = repo.join("channels/current/x86_64-v3/profiles/minimal/index.json.zst");
    let index = Index::decode(&fs::read(&entry).unwrap()).unwrap();
    let profile = index.profile.as_ref().expect("the profile member");
    assert_eq!(profile.name, "minimal");
    let mesa: laft_core::Name = "mesa".parse().unwrap();
    let mesa = &profile.values[&mesa];
    assert_eq!(mesa["x11"], laft_core::OptionValue::Bool(false));

    let bytes = fs::read(&entry).unwrap();
    let signature =
        signature(&repo.join("channels/current/x86_64-v3/profiles/minimal/index.json.zst.minisig"));
    assert!(public(&key).verifies(&bytes, &signature));
    let comment: IndexComment = signature.trusted_comment().parse().unwrap();
    assert_eq!(comment, index.comment());
    let snapshot = repo.join(format!(
        "channels/current/x86_64-v3/profiles/minimal/snapshots/{T0}.json.zst"
    ));
    assert_eq!(fs::read(&snapshot).unwrap(), bytes);
}

#[test]
fn a_profile_index_needs_the_recipe_repository() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let (_, stderr) = fails(&[
        "index",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--channel",
        "current",
        "--profile",
        "minimal",
        "--now",
        &at(0),
        s(&out),
    ]);
    assert!(stderr.contains("--recipes"), "{stderr}");
}

#[test]
fn an_unknown_profile_is_refused() {
    let dir = laft_test::tempdir();
    let recipes = profile_repo(dir.path());
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let (_, stderr) = fails(&[
        "index",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--channel",
        "current",
        "--profile",
        "wayland_only",
        "--recipes",
        s(&recipes),
        "--now",
        &at(0),
        s(&out),
    ]);
    assert!(stderr.contains("no recipe repository declares"), "{stderr}");
}

#[test]
fn sign_signs_a_profile_index() {
    let dir = laft_test::tempdir();
    let recipes = profile_repo(dir.path());
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let key = keygen(dir.path(), "online");
    ok(&[
        "index",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--channel",
        "current",
        "--profile",
        "minimal",
        "--recipes",
        s(&recipes),
        "--now",
        &at(0),
        s(&out),
    ]);
    let stdout = ok(&[
        "sign",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "--profile",
        "minimal",
        "--key",
        s(&key),
    ]);
    assert!(stdout.contains(&format!("generation={T0}")), "{stdout}");
    let bytes =
        fs::read(repo.join("channels/current/x86_64-v3/profiles/minimal/index.json.zst")).unwrap();
    let signature =
        signature(&repo.join("channels/current/x86_64-v3/profiles/minimal/index.json.zst.minisig"));
    assert!(public(&key).verifies(&bytes, &signature));
}

/// Writes a recipe repository for the package `hello` at `dir`.
fn recipe_repo(dir: &Path) -> PathBuf {
    let recipes = dir.join("recipes-src");
    fs::create_dir_all(recipes.join("recipes/hello")).expect("writable");
    fs::write(recipes.join("recipes.stav"), "recipe_repo core {}\n").expect("writable");
    fs::write(
        recipes.join("recipes/hello/package.stav"),
        "package hello {\n  version = \"1.0\"\n  summary = \"Say hello\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");
    recipes
}

#[test]
fn index_records_the_recipe_snapshot_of_a_generation() {
    let dir = laft_test::tempdir();
    let recipes = recipe_repo(dir.path());
    let out = dir.path().join("out");
    pkg("hello", "1.0").file("usr/bin/hello", "hi").write(&out);
    let repo = dir.path().join("repo");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.extend(["--recipes", s(&recipes), s(&out)]);
    ok(&args);
    let index = read_index(&repo, "current");
    let first = index.recipes.as_ref().expect("the recipes member");
    assert!(first.file.starts_with("recipes/b3:"));
    assert_eq!(first.size, fsx_size(&repo.join(&first.file)));
    assert!(repo.join(&first.file).is_file());
    assert_eq!(first.file, format!("recipes/{}.tar.zst", first.blake3));

    let now = at(HOUR);
    let mut args = index_args(&repo, "current", &now);
    args.extend(["--recipes", s(&recipes), s(&out)]);
    ok(&args);
    let index = read_index(&repo, "current");
    assert_eq!(
        index.recipes.as_ref().unwrap().blake3,
        first.blake3,
        "the same tree packs to the same archive"
    );
    let entries: Vec<_> = fs::read_dir(repo.join("recipes")).unwrap().collect();
    assert_eq!(entries.len(), 1, "one archive for two generations");

    fs::write(
        recipes.join("recipes/hello/package.stav"),
        "package hello {\n  version = \"1.1\"\n  summary = \"Say hello\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    )
    .unwrap();
    let now = at(2 * HOUR);
    let mut args = index_args(&repo, "current", &now);
    args.extend(["--recipes", s(&recipes), s(&out)]);
    ok(&args);
    let index = read_index(&repo, "current");
    let third = index.recipes.as_ref().unwrap();
    assert_ne!(third.blake3, first.blake3);
    assert!(repo.join(&third.file).is_file());
}

/// The size of `path`, or 0 when it does not exist.
fn fsx_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[test]
fn index_records_the_option_declarations_of_the_recipes() {
    let dir = laft_test::tempdir();
    let recipes = dir.path().join("recipes-src");
    fs::create_dir_all(recipes.join("recipes/hello")).expect("writable");
    fs::create_dir_all(recipes.join("recipes/zlib")).expect("writable");
    fs::write(recipes.join("recipes.stav"), "recipe_repo core {}\n").expect("writable");
    fs::write(
        recipes.join("recipes/hello/package.stav"),
        "package hello {\n  version = \"1.0\"\n  summary = \"Say hello\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");
    fs::write(
        recipes.join("recipes/hello/options.stav"),
        "struct Options {\n  ## Ship the extra command line tool.\n  tool: Bool = true\n}\n",
    )
    .expect("writable");
    fs::write(
        recipes.join("recipes/zlib/package.stav"),
        "package zlib {\n  version = \"1.2\"\n  summary = \"Deflate\"\n  license = [Zlib]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");

    let out = dir.path().join("out");
    pkg("hello", "1.0").file("usr/bin/hello", "hi").write(&out);
    let repo = dir.path().join("repo");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.extend(["--recipes", s(&recipes), s(&out)]);
    ok(&args);
    let index = read_index(&repo, "current");
    let hello = &index.options[&"hello".parse().unwrap()];
    assert_eq!(hello.len(), 1);
    assert_eq!(hello[0].name, "tool");
    assert_eq!(hello[0].doc, "Ship the extra command line tool.");
    assert_eq!(hello[0].default, laft_core::OptionValue::Bool(true));
    assert_eq!(index.options.len(), 1, "zlib declares no options");
}

#[test]
fn index_refuses_a_recipe_with_broken_options() {
    let dir = laft_test::tempdir();
    let recipes = recipe_repo(dir.path());
    fs::write(
        recipes.join("recipes/hello/options.stav"),
        "struct Options {\n  tool: Bool = true\n}\n",
    )
    .expect("writable");
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.extend(["--recipes", s(&recipes), s(&out)]);
    let (_, stderr) = fails(&args);
    assert!(stderr.contains("R072"), "{stderr}");
    assert!(stderr.contains("of repository core"), "{stderr}");
}

#[test]
fn a_generation_must_be_later_than_the_last_one() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let now = at(HOUR);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(&out));
    ok(&args);
    let (_, stderr) = fails(&args);
    assert!(
        stderr.contains("not greater than the last published generation"),
        "{stderr}"
    );
    let earlier = at(0);
    let mut args = index_args(&repo, "current", &earlier);
    args.push(s(&out));
    fails(&args);
    assert_eq!(read_index(&repo, "current").generation, T0 + HOUR);
}

#[test]
fn index_refuses_a_candidate_with_check_errors_and_writes_nothing() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("vim", "9.1.0800").file("usr/bin/vi", "vim").write(&out);
    pkg("neovim", "0.11.4")
        .file("usr/bin/vi", "neovim")
        .write(&out);
    let repo = dir.path().join("repo");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(&out));
    let (stdout, stderr) = fails(&args);
    assert!(
        stdout.contains("KR001 error   file conflict: usr/bin/vi"),
        "{stdout}"
    );
    assert!(stderr.contains("nothing was written"), "{stderr}");
    assert!(!repo.exists());
}

#[test]
fn index_sets_defaults_and_keeps_them_in_later_generations() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    for name in ["vim", "neovim"] {
        pkg(name, "1.0")
            .file("usr/bin/vi", name)
            .alternative("vi", &["usr/bin/vi"])
            .write(&out);
    }
    let repo = dir.path().join("repo");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(&out));
    let (stdout, _) = fails(&args);
    assert!(
        stdout.contains("alternatives group vi has no default"),
        "{stdout}"
    );
    args.extend(["--alternative", "vi=vim"]);
    ok(&args);
    let later = dir.path().join("later");
    pkg("hello", "1.0").write(&later);
    let now = at(HOUR);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(&later));
    ok(&args);
    let index = read_index(&repo, "current");
    assert_eq!(index.packages.len(), 3);
    assert_eq!(
        index.defaults.alternatives[&"vi".parse().unwrap()].as_str(),
        "vim"
    );
}

#[test]
fn a_changed_build_needs_a_new_revision() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let first = pkg("hello", "1.0")
        .file("usr/bin/hello", "one")
        .write(&dir.path().join("a"));
    let second = pkg("hello", "1.0")
        .file("usr/bin/hello", "two")
        .write(&dir.path().join("b"));
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(first.parent().unwrap()));
    ok(&args);
    let now = at(HOUR);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(second.parent().unwrap()));
    let (_, stderr) = fails(&args);
    assert!(
        stderr.contains("revision not bumped: hello 1.0-r1"),
        "{stderr}"
    );
    let third = pkg("hello", "1.0")
        .rev(2)
        .file("usr/bin/hello", "two")
        .write(&dir.path().join("c"));
    let mut args = index_args(&repo, "current", &now);
    args.push(s(third.parent().unwrap()));
    ok(&args);
    let revs: Vec<u32> = read_index(&repo, "current")
        .packages
        .iter()
        .map(|e| e.meta.rev)
        .collect();
    assert_eq!(revs, [2, 1]);
}

#[test]
fn old_snapshots_beyond_the_kept_number_are_removed() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    for hour in 0..4 {
        let now = at(hour * HOUR);
        let mut args = index_args(&repo, "current", &now);
        args.extend(["--keep-snapshots", "2", s(&out)]);
        ok(&args);
    }
    let mut snapshots: Vec<String> =
        fs::read_dir(repo.join("channels/current/x86_64-v3/snapshots"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
    snapshots.sort();
    let expected: Vec<String> = [2, 3]
        .iter()
        .map(|h| format!("{}.json.zst", T0 + h * HOUR))
        .collect();
    assert_eq!(snapshots, expected);
}

#[test]
fn sign_signs_an_unsigned_generation_and_refuses_a_revoked_key() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let root = keygen(dir.path(), "root");
    let online = keygen(dir.path(), "online");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(&out));
    let stdout = ok(&args);
    assert!(stdout.contains("unsigned"), "{stdout}");
    assert!(!repo.join(format!("{INDEX}.minisig")).exists());

    ok(&[
        "sign",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "--key",
        s(&online),
    ]);
    let bytes = fs::read(repo.join(INDEX)).unwrap();
    assert!(public(&online).verifies(&bytes, &signature(&repo.join(format!("{INDEX}.minisig")))));

    let id = public(&online).id().to_string();
    let pub_file = online.with_extension("pub");
    ok(&[
        "keys",
        "add",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--public",
        s(&pub_file),
        "--not-before",
        &at(0),
        "--not-after",
        &at(DAY),
        "--key",
        s(&root),
    ]);
    ok(&["keys", "revoke", "--repo", s(&repo), &id, "--key", s(&root)]);
    let output = run(&["keys", "revoke", "--repo", s(&repo), &id]);
    assert!(!output.status.success(), "a key cannot be revoked twice");
    let (_, stderr) = fails(&[
        "sign",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "--key",
        s(&online),
    ]);
    assert!(stderr.contains("revoked"), "{stderr}");
}

#[test]
fn keys_json_is_versioned_and_signed_by_the_root_key() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let root = keygen(dir.path(), "root");
    let online = keygen(dir.path(), "online");
    let next = keygen(dir.path(), "next");
    let add = |key: &Path, not_after: &str| {
        let pub_file = key.with_extension("pub");
        ok(&[
            "keys",
            "add",
            "--repo",
            s(&repo),
            "--name",
            "core",
            "--public",
            s(&pub_file),
            "--not-before",
            &at(0),
            "--not-after",
            not_after,
            "--key",
            s(&root),
        ]);
    };
    add(&online, &at(365 * DAY));
    add(&next, &at(730 * DAY));
    let online_id = public(&online).id();
    ok(&[
        "keys",
        "revoke",
        "--repo",
        s(&repo),
        &online_id.to_string(),
        "--generation-floor",
        &T0.to_string(),
        "--key",
        s(&root),
    ]);

    let json = fs::read(repo.join("keys.json")).unwrap();
    let keys = Keys::from_json(&json).unwrap();
    assert_eq!(keys.version, 3);
    assert_eq!(keys.revoked, [online_id]);
    assert_eq!(keys.generation_floor, Some(T0));
    assert_eq!(keys.keys.len(), 1);
    assert_eq!(keys.keys[0].id, public(&next).id());

    let sig = signature(&repo.join("keys.json.minisig"));
    assert!(public(&root).verifies(&json, &sig));
    let comment: KeysComment = sig.trusted_comment().parse().unwrap();
    assert_eq!(comment, keys.comment());

    let state = laft_index::IndexState::new(dir.path().join("state"));
    let config = laft_index::Repo {
        name: "core".parse().unwrap(),
        channel: "current".parse().unwrap(),
        arch: laft_pkg::Arch::X86_64V3,
        key: public(&root),
        keys: vec![],
    };
    let trust = state
        .accept_keys(
            &config,
            KeysFetch::Found {
                json: &json,
                signature: &fs::read(repo.join("keys.json.minisig")).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(trust.keys().unwrap().version, 3);

    let (_, stderr) = fails(&["sign", "--repo", s(&repo), "--keys", "--key", s(&next)]);
    assert!(stderr.contains("only a root key signs it"), "{stderr}");
}

#[test]
fn a_removed_key_is_no_longer_listed_but_not_revoked() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let online = keygen(dir.path(), "online");
    ok(&[
        "keys",
        "add",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--public",
        s(&online.with_extension("pub")),
        "--not-before",
        &at(0),
        "--not-after",
        &at(365 * DAY),
    ]);
    let id = public(&online).id().to_string();
    ok(&["keys", "remove", "--repo", s(&repo), &id]);
    let keys = Keys::from_json(&fs::read(repo.join("keys.json")).unwrap()).unwrap();
    assert_eq!(keys.version, 2);
    assert!(keys.keys.is_empty());
    assert!(keys.revoked.is_empty());
    let (_, stderr) = fails(&["keys", "remove", "--repo", s(&repo), &id]);
    assert!(stderr.contains("is not listed"), "{stderr}");
}

#[test]
fn an_online_key_signs_only_within_its_window() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let root = keygen(dir.path(), "root");
    let online = keygen(dir.path(), "online");
    let pub_file = online.with_extension("pub");
    ok(&[
        "keys",
        "add",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--public",
        s(&pub_file),
        "--not-before",
        &at(DAY),
        "--not-after",
        &at(2 * DAY),
        "--key",
        s(&root),
    ]);
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.extend(["--key", s(&online), s(&out)]);
    let (_, stderr) = fails(&args);
    assert!(
        stderr.contains("does not contain the signing time"),
        "{stderr}"
    );
    assert!(!repo.join("pool").exists());
    let now = at(DAY + HOUR);
    let mut args = index_args(&repo, "current", &now);
    args.extend(["--key", s(&online), s(&out)]);
    ok(&args);
}

#[test]
fn yank_moves_the_package_to_yanked_in_a_new_generation() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let file = pkg("zlib", "1.3.1").write(&out);
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(&out));
    ok(&args);
    let reason = "breaks gzip";
    let stdout = ok(&[
        "yank",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "zlib",
        "1.3.1",
        "--reason",
        reason,
        "--now",
        &at(HOUR),
    ]);
    assert!(stdout.contains("yanked zlib 1.3.1-r1"), "{stdout}");
    let index = read_index(&repo, "current");
    assert_eq!(index.generation, T0 + HOUR);
    assert_eq!(index.packages.len(), 1);
    let yank = &index.yanked[0];
    assert_eq!((yank.name.as_str(), yank.reason.as_str()), ("zlib", reason));
    assert_eq!(yank.time, Timestamp::from_secs(T0 + HOUR));

    let (_, stderr) = fails(&[
        "yank",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "zlib",
        "1.3.1",
        "--reason",
        reason,
        "--now",
        &at(2 * HOUR),
    ]);
    assert!(stderr.contains("lists no package zlib 1.3.1"), "{stderr}");
    let now = at(2 * HOUR);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(&file));
    let (_, stderr) = fails(&args);
    assert!(stderr.contains("was yanked"), "{stderr}");
}

#[test]
fn a_yanked_revision_is_refused_until_its_yank_expires() {
    let dir = laft_test::tempdir();
    let (first, rebuild) = (dir.path().join("a"), dir.path().join("b"));
    pkg("zlib", "1.3.1")
        .file("usr/lib/libz.so.1", "one")
        .write(&first);
    pkg("zlib", "1.3.1")
        .file("usr/lib/libz.so.1", "two")
        .write(&rebuild);
    let repo = dir.path().join("repo");
    let add = |file: &Path, offset: u64| {
        let now = at(offset);
        let mut args = index_args(&repo, "current", &now);
        args.push(s(file));
        run(&args)
    };
    assert!(add(&first, 0).status.success());
    ok(&[
        "yank",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "zlib",
        "1.3.1",
        "--reason",
        "bad",
        "--now",
        &at(HOUR),
    ]);
    let output = add(&rebuild, 2 * HOUR);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("was yanked"), "{stderr}");
    assert!(add(&first, 91 * DAY).status.success());
    let index = read_index(&repo, "current");
    assert_eq!(index.packages.len(), 1);
    assert!(index.yanked.is_empty());
}

#[test]
fn yanked_packages_are_listed_for_ninety_days() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("zlib", "1.3.1").write(&out);
    let repo = dir.path().join("repo");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(&out));
    ok(&args);
    ok(&[
        "yank",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "zlib",
        "1.3.1",
        "--reason",
        "bad",
        "--now",
        &at(HOUR),
    ]);
    let resign = |offset: u64| {
        let now = at(offset);
        ok(&[
            "index",
            "--repo",
            s(&repo),
            "--channel",
            "current",
            "--now",
            &now,
        ]);
        read_index(&repo, "current").yanked.len()
    };
    assert_eq!(resign(90 * DAY), 1);
    assert_eq!(resign(91 * DAY), 0);
}

#[test]
fn promote_honors_its_gates_and_copies_the_packages() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let root = keygen(dir.path(), "root");
    let root_pub = root.with_extension("pub");
    let now = at(0);
    let mut args = index_args(&repo, "testing", &now);
    args.extend(["--key", s(&root), s(&out)]);
    ok(&args);
    let promote = |extra: &[&str]| {
        let mut args = vec!["promote", "--repo", s(&repo), "--min-soak", "24h"];
        args.extend(["--root-key", s(&root_pub)]);
        args.extend(extra);
        run(&args)
    };
    let output = promote(&["--now", &at(HOUR)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("promotion needs 86400"));

    let report = dir.path().join("report.json");
    let stdout = ok(&[
        "check",
        "--repo",
        s(&repo),
        "--channel",
        "testing",
        "--json",
    ]);
    fs::write(&report, &stdout).unwrap();
    let stale = dir.path().join("stale.json");
    fs::write(
        &stale,
        stdout.replace(&T0.to_string(), &(T0 - 1).to_string()),
    )
    .unwrap();
    let output = promote(&["--now", &at(DAY), "--check-report", s(&stale)]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("not on generation"));
    let failed = dir.path().join("failed.json");
    fs::write(&failed, stdout.replace("\"errors\":0", "\"errors\":2")).unwrap();
    let output = promote(&["--now", &at(DAY), "--check-report", s(&failed)]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("reports 2 errors"));
    assert!(!repo.join(INDEX).exists());

    let output = promote(&["--now", &at(DAY), "--check-report", s(&report)]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let testing = read_index(&repo, "testing");
    let current = read_index(&repo, "current");
    assert_eq!(current.generation, T0 + DAY);
    assert_eq!(current.packages, testing.packages);
    assert_eq!(current.channel.as_str(), "current");
}

#[test]
fn the_soak_gate_counts_only_a_verified_source() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let root = keygen(dir.path(), "root");
    let other = keygen(dir.path(), "other");
    let online = keygen(dir.path(), "online");
    let promote = |root_pub: &Path| {
        run(&[
            "promote",
            "--repo",
            s(&repo),
            "--min-soak",
            "1h",
            "--root-key",
            s(root_pub),
            "--now",
            &at(DAY),
        ])
    };
    let index = |key: Option<&Path>, offset: u64| {
        let now = at(offset);
        let mut args = index_args(&repo, "testing", &now);
        if let Some(key) = key {
            args.extend(["--key", s(key)]);
        }
        args.push(s(&out));
        ok(&args);
    };
    let stderr = |output: std::process::Output| {
        assert_eq!(output.status.code(), Some(1));
        String::from_utf8(output.stderr).unwrap()
    };

    let output = run(&["promote", "--repo", s(&repo), "--min-soak", "1h"]);
    assert_eq!(output.status.code(), Some(2), "--min-soak needs --root-key");

    index(None, 0);
    let error = stderr(promote(&root.with_extension("pub")));
    assert!(error.contains("is not signed"), "{error}");
    index(Some(&other), HOUR);
    let error = stderr(promote(&root.with_extension("pub")));
    assert!(error.contains("does not verify"), "{error}");

    ok(&[
        "keys",
        "add",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--public",
        s(&online.with_extension("pub")),
        "--not-before",
        &at(0),
        "--not-after",
        &at(365 * DAY),
    ]);
    index(Some(&online), 2 * HOUR);
    let error = stderr(promote(&root.with_extension("pub")));
    assert!(error.contains("keys.json is not signed"), "{error}");
    ok(&["sign", "--repo", s(&repo), "--keys", "--key", s(&other)]);
    let error = stderr(promote(&root.with_extension("pub")));
    assert!(error.contains("keys.json does not verify"), "{error}");
    ok(&["sign", "--repo", s(&repo), "--keys", "--key", s(&root)]);
    let output = promote(&root.with_extension("pub"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn signing_with_a_key_keys_json_does_not_list_needs_an_explicit_flag() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let online = keygen(dir.path(), "online");
    let unlisted = keygen(dir.path(), "unlisted");
    ok(&[
        "keys",
        "add",
        "--repo",
        s(&repo),
        "--name",
        "core",
        "--public",
        s(&online.with_extension("pub")),
        "--not-before",
        &at(0),
        "--not-after",
        &at(365 * DAY),
    ]);
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.extend(["--key", s(&unlisted), s(&out)]);
    let (_, stderr) = fails(&args);
    assert!(stderr.contains("is not listed in keys.json"), "{stderr}");
    assert!(!repo.join("pool").exists());

    args.push("--allow-unlisted-key");
    let output = run(&args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("warning") && stderr.contains("until keys.json lists"),
        "{stderr}"
    );

    let sign = |extra: &[&str]| {
        let mut args = vec!["sign", "--repo", s(&repo), "--channel", "current"];
        args.extend(["--key", s(&unlisted)]);
        args.extend(extra);
        run(&args)
    };
    assert_eq!(sign(&[]).status.code(), Some(1));
    let output = sign(&["--allow-unlisted-key"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("warning"));
    ok(&[
        "sign",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "--key",
        s(&online),
    ]);
}

#[test]
fn publish_refuses_an_unsigned_index_and_a_rollback() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let target = dir.path().join("www");
    let key = keygen(dir.path(), "online");
    let now = at(0);
    let mut args = index_args(&repo, "current", &now);
    args.push(s(&out));
    ok(&args);
    let (_, stderr) = fails(&["publish", s(&repo), s(&target)]);
    assert!(stderr.contains("is not signed"), "{stderr}");
    assert!(!target.exists());

    ok(&[
        "sign",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "--key",
        s(&key),
    ]);
    let stdout = ok(&["publish", s(&repo), s(&target)]);
    assert!(
        stdout.contains(&format!("published current/x86_64-v3 generation {T0}")),
        "{stdout}"
    );
    let stdout = ok(&["publish", s(&repo), s(&target)]);
    assert!(stdout.contains("0 files copied"), "{stdout}");

    let newer = dir.path().join("newer");
    fs::create_dir(&newer).unwrap();
    let now = at(HOUR);
    let mut args = index_args(&newer, "current", &now);
    args.extend(["--key", s(&key), s(&out)]);
    ok(&args);
    ok(&["publish", s(&newer), s(&target)]);
    let (_, stderr) = fails(&["publish", s(&repo), s(&target)]);
    assert!(stderr.contains("newer"), "{stderr}");
}
