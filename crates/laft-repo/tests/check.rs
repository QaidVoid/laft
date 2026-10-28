//! Each repository check has a fixture repository that triggers exactly
//! its finding.

mod support;

use std::fs;
use std::path::Path;

use laft_core::Blake3Hash;
use serde_json::Value;
use support::{DAY, at, fixture, pkg, run, s};

/// Runs `laft-repo check --json` on channel `current` and returns the
/// codes it reports, its report and whether it exited successfully.
fn check(repo: &Path, extra: &[&str]) -> (Vec<String>, Value, bool) {
    let mut args = vec!["check", "--repo", s(repo), "--channel", "current", "--json"];
    args.extend(extra);
    let output = run(&args);
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value =
        serde_json::from_slice(&output.stdout).expect("check --json prints a JSON report");
    let codes = report["findings"]
        .as_array()
        .expect("a report lists its findings")
        .iter()
        .map(|f| {
            f["code"]
                .as_str()
                .expect("every finding has a code")
                .to_owned()
        })
        .collect();
    (codes, report, output.status.success())
}

#[test]
fn a_clean_repository_has_no_findings() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [
        pkg("pcre2", "10.46")
            .provides("so:libpcre2-8.so.0")
            .file("usr/lib/libpcre2-8.so.0", "elf")
            .write(&out),
        pkg("ripgrep", "14.1.1")
            .depends("so:libpcre2-8.so.0")
            .file("usr/bin/rg", "rg")
            .write(&out),
    ];
    fixture(&dir.path().join("repo"), "current", 0, &packages, |_| {});
    let (codes, report, success) = check(&dir.path().join("repo"), &[]);
    assert!(codes.is_empty(), "{report}");
    assert!(success);
    assert_eq!(report["generation"], support::T0);
}

#[test]
fn kr001_two_packages_owning_one_path() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [
        pkg("vim", "9.1.0800").file("usr/bin/vi", "vim").write(&out),
        pkg("neovim", "0.11.4")
            .file("usr/bin/vi", "neovim")
            .write(&out),
    ];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR001"], "{report}");
    assert!(!success);
    let finding = &report["findings"][0];
    assert_eq!(finding["message"], "file conflict: usr/bin/vi");
    assert_eq!(finding["paths"], serde_json::json!(["usr/bin/vi"]));
    assert_eq!(
        finding["packages"],
        serde_json::json!(["neovim 0.11.4-r1", "vim 9.1.0800-r1"])
    );
}

#[test]
fn kr001_is_silent_for_conflicting_packages_and_shared_alternatives() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [
        pkg("vim", "9.1.0800")
            .file("usr/bin/vi", "vim")
            .file("usr/bin/vimdiff", "vim")
            .alternative("vi", &["usr/bin/vi"])
            .write(&out),
        pkg("neovim", "0.11.4")
            .file("usr/bin/vi", "neovim")
            .alternative("vi", &["usr/bin/vi"])
            .write(&out),
        pkg("vim-tiny", "9.1.0800")
            .file("usr/bin/vimdiff", "tiny")
            .conflicts("vim")
            .write(&out),
    ];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |index| {
        index
            .defaults
            .alternatives
            .insert("vi".parse().unwrap(), "vim".parse().unwrap());
    });
    let (codes, report, _) = check(&repo, &[]);
    assert!(codes.is_empty(), "{report}");
}

#[test]
fn kr002_a_dependency_nothing_provides() {
    let dir = laft_test::tempdir();
    let packages = [pkg("ripgrep", "14.1.1")
        .depends("so:libpcre2-8.so.0")
        .write(&dir.path().join("out"))];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR002"], "{report}");
    assert_eq!(
        report["findings"][0]["message"],
        "dangling dependency: ripgrep 14.1.1-r1 needs so:libpcre2-8.so.0"
    );
}

#[test]
fn kr002_resolves_against_a_base_index() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let base = dir.path().join("base");
    fixture(
        &base,
        "current",
        0,
        &[pkg("pcre2", "10.46")
            .provides("so:libpcre2-8.so.0")
            .write(&out)],
        |_| {},
    );
    let repo = dir.path().join("repo");
    fixture(
        &repo,
        "current",
        0,
        &[pkg("ripgrep", "14.1.1")
            .depends("so:libpcre2-8.so.0")
            .write(&out)],
        |index| {
            index.repo = "overlay".parse().unwrap();
        },
    );
    let base_index = base.join("channels/current/x86_64-v3/index.json.zst");
    let (codes, report, _) = check(&repo, &["--with-index", s(&base_index)]);
    assert!(codes.is_empty(), "{report}");
}

#[test]
fn kr003_a_package_whose_closure_conflicts() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [
        pkg("app", "1.0")
            .depends("left")
            .depends("right")
            .write(&out),
        pkg("left", "1.0").conflicts("right").write(&out),
        pkg("right", "1.0").write(&out),
    ];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR003"], "{report}");
    assert_eq!(
        report["findings"][0]["packages"],
        serde_json::json!(["app 1.0-r1"])
    );
}

#[test]
fn kr004_an_alternatives_group_without_default() {
    let dir = laft_test::tempdir();
    let packages = [pkg("vim", "9.1.0800")
        .file("usr/bin/vi", "vim")
        .alternative("vi", &["usr/bin/vi"])
        .write(&dir.path().join("out"))];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR004"], "{report}");
    assert_eq!(
        report["findings"][0]["message"],
        "alternatives group vi has no default"
    );
}

#[test]
fn kr004_members_with_different_paths() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [
        pkg("vim", "9.1.0800")
            .file("usr/bin/vi", "vim")
            .file("usr/bin/view", "vim")
            .alternative("vi", &["usr/bin/vi", "usr/bin/view"])
            .write(&out),
        pkg("neovim", "0.11.4")
            .file("usr/bin/vi", "nvim")
            .alternative("vi", &["usr/bin/vi"])
            .write(&out),
    ];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |index| {
        index
            .defaults
            .alternatives
            .insert("vi".parse().unwrap(), "vim".parse().unwrap());
    });
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR004"], "{report}");
    assert_eq!(
        report["findings"][0]["message"],
        "members of alternatives group vi ship different paths"
    );
}

#[test]
fn kr005_a_rebuild_without_a_revision_bump() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let first = pkg("hello", "1.0")
        .file("usr/bin/hello", "one")
        .write(&dir.path().join("a"));
    let second = pkg("hello", "1.0")
        .file("usr/bin/hello", "two")
        .write(&dir.path().join("b"));
    fixture(&repo, "testing", 0, &[first], |_| {});
    fixture(&repo, "current", 0, &[second], |_| {});
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR005"], "{report}");
}

#[test]
fn kr006_a_downgrade_is_only_a_warning() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let repo = dir.path().join("repo");
    fixture(
        &repo,
        "current",
        0,
        &[pkg("mesa", "25.2.5").write(&out)],
        |_| {},
    );
    fixture(
        &repo,
        "current",
        10,
        &[pkg("mesa", "25.2.4").write(&out)],
        |_| {},
    );
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR006"], "{report}");
    assert_eq!(report["findings"][0]["level"], "warning");
    assert_eq!(
        report["findings"][0]["message"],
        "downgrade: mesa 25.2.4-r1 (published 25.2.5-r1)"
    );
    assert!(success);
}

#[test]
fn kr007_two_providers_without_a_default() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [
        pkg("libjpeg-turbo", "3.1.2")
            .provides("so:libjpeg.so.8")
            .write(&out),
        pkg("mozjpeg", "4.1.5")
            .provides("so:libjpeg.so.8")
            .write(&out),
    ];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR007"], "{report}");
    fixture(&repo, "current", 10, &packages, |index| {
        index
            .defaults
            .providers
            .insert("so:libjpeg.so.8".into(), "libjpeg-turbo".parse().unwrap());
    });
    let (codes, report, _) = check(&repo, &[]);
    assert!(codes.is_empty(), "{report}");
}

#[test]
fn kr008_a_package_file_that_is_not_a_package() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let packages = [pkg("hello", "1.0").write(&dir.path().join("out"))];
    let garbage = b"not a package file";
    fixture(&repo, "current", 0, &packages, |index| {
        let entry = &mut index.packages[0];
        entry.size = garbage.len() as u64;
        entry.blake3 = Blake3Hash::of(garbage);
    });
    let file = support::read_index(&repo, "current").packages[0]
        .file
        .clone();
    fs::write(repo.join(file), garbage).unwrap();
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR008"], "{report}");
}

#[test]
fn kr009_a_missing_package_file() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let index = fixture(
        &repo,
        "current",
        0,
        &[pkg("hello", "1.0").write(&dir.path().join("out"))],
        |_| {},
    );
    fs::remove_file(repo.join(&index.packages[0].file)).unwrap();
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR009"], "{report}");
    assert_eq!(report["findings"][0]["paths"][0], index.packages[0].file);
}

#[test]
fn kr010_an_entry_that_differs_from_its_package() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let packages = [pkg("hello", "1.0").write(&dir.path().join("out"))];
    fixture(&repo, "current", 0, &packages, |index| {
        index.packages[0].meta.summary = "Something else".into();
    });
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR010"], "{report}");
    assert_eq!(
        report["findings"][0]["details"][0],
        "differing fields: summary"
    );
}

#[test]
fn kr011_a_payload_with_a_generated_file() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let packages = [pkg("glib", "2.86.0")
        .file(
            "usr/share/glib-2.0/schemas/org.gtk.Settings.xml",
            "<schemalist/>",
        )
        .file("usr/share/glib-2.0/schemas/gschemas.compiled", "cache")
        .write(&dir.path().join("out"))];
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR011"], "{report}");
    assert_eq!(
        report["findings"][0]["paths"],
        serde_json::json!(["usr/share/glib-2.0/schemas/gschemas.compiled"])
    );
}

#[test]
fn ci001_a_dirty_build_unless_allowed() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    fixture(
        &repo,
        "current",
        0,
        &[pkg("hello", "1.0").dirty().write(&dir.path().join("out"))],
        |_| {},
    );
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["CI001"], "{report}");
    let (codes, report, _) = check(&repo, &["--allow-bootstrap"]);
    assert!(codes.is_empty(), "{report}");
}

#[test]
fn ci003_a_package_without_buildinfo() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let packages = [pkg("hello", "1.0")
        .without_buildinfo()
        .write(&dir.path().join("out"))];
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["CI003"], "{report}");
}

#[test]
fn the_text_report_names_packages_and_ends_with_a_summary() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [
        pkg("vim", "9.1.0800").file("usr/bin/vi", "vim").write(&out),
        pkg("neovim", "0.11.4")
            .file("usr/bin/vi", "neovim")
            .write(&out),
    ];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let output = run(&[
        "check",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "--now",
        &at(DAY),
    ]);
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with(
        "KR001 error   file conflict: usr/bin/vi\n              owned by: neovim 0.11.4-r1, vim 9.1.0800-r1\n"
    ), "{text}");
    assert!(text.ends_with("1 finding: 1 error, 0 warnings\n"), "{text}");
}
