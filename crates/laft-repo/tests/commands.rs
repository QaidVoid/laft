//! `index`, `sign`, `yank`, `promote`, `keys` and `publish`, run as the
//! binary against temporary repositories.

mod support;

use std::fs;
use std::path::Path;

use laft_core::Timestamp;
use laft_index::{IndexComment, Keys, KeysComment, KeysFetch, Signature};
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
    let dir = tempfile::tempdir().unwrap();
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

#[test]
fn a_generation_must_be_later_than_the_last_one() {
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    pkg("vim", "9.1.0800")
        .file("usr/bin/vi", "vim")
        .alternative("vi", &["usr/bin/vi"])
        .write(&out);
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
    assert_eq!(index.packages.len(), 2);
    assert_eq!(
        index.defaults.alternatives[&"vi".parse().unwrap()].as_str(),
        "vim"
    );
}

#[test]
fn a_changed_build_needs_a_new_revision() {
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
fn an_online_key_signs_only_within_its_window() {
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
fn yanked_packages_are_listed_for_ninety_days() {
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    pkg("hello", "1.0").write(&out);
    let repo = dir.path().join("repo");
    let now = at(0);
    let mut args = index_args(&repo, "testing", &now);
    args.push(s(&out));
    ok(&args);
    let promote = |extra: &[&str]| {
        let mut args = vec!["promote", "--repo", s(&repo), "--min-soak", "24h"];
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
fn publish_refuses_an_unsigned_index_and_a_rollback() {
    let dir = tempfile::tempdir().unwrap();
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
