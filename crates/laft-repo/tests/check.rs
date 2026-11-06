//! Each repository check has a fixture repository that triggers exactly
//! its finding.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use laft_core::Blake3Hash;
use serde_json::Value;
use support::{DAY, HOUR, at, fixture, pkg, run, s};

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

/// An overlay can constrain the options of a base package: KR015 looks
/// the declarations up in the base indexes too.
#[test]
fn kr015_resolves_option_declarations_against_a_base_index() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let base = dir.path().join("base");
    let core = pkg("mesa", "25.1")
        .option("llvm", laft_core::OptionValue::TRUE)
        .write(&out);
    fixture(
        &base,
        "current",
        0,
        &[core],
        |index: &mut laft_index::Index| {
            index.options.insert(
                "mesa".parse().unwrap(),
                vec![bool_decl("llvm", true, "Build the LLVM backend.")],
            );
        },
    );
    let repo = dir.path().join("repo");
    fixture(
        &repo,
        "current",
        0,
        &[pkg("app", "1.0").depends("mesa{llvm}").write(&out)],
        |index: &mut laft_index::Index| {
            index.repo = "overlay".parse().unwrap();
        },
    );
    let base_index = base.join("channels/current/x86_64-v3/index.json.zst");
    let (codes, report, success) = check(&repo, &["--with-index", s(&base_index)]);
    assert!(codes.is_empty(), "{report}");
    assert!(success);

    // A constraint of the wrong type is still refused.
    let bad = dir.path().join("bad");
    fixture(
        &repo,
        "current",
        HOUR,
        &[pkg("app", "1.0")
            .rev(2)
            .depends("mesa{vulkan=amd}")
            .write(&bad)],
        |index: &mut laft_index::Index| {
            index.repo = "overlay".parse().unwrap();
        },
    );
    let (codes, report, _) = check(&repo, &["--with-index", s(&base_index)]);
    assert_eq!(codes, ["KR015"], "{report}");
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

/// Checks an index whose alternatives group `vi` has `vim` with priority
/// `vim`, `neovim` 0.10.0 with priority 9 and `neovim` 0.11.4 with
/// priority `neovim`, and `vim` as its default when `default` is set.
fn check_vi(vim: i64, neovim: i64, default: bool) -> (Vec<String>, serde_json::Value) {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let vi = &["usr/bin/vi"];
    let packages = [
        pkg("vim", "9.1.0800")
            .file("usr/bin/vi", "vim")
            .ranked_alternative("vi", vi, vim)
            .write(&out),
        pkg("neovim", "0.10.0")
            .file("usr/bin/vi", "nvim")
            .ranked_alternative("vi", vi, 9)
            .write(&out),
        pkg("neovim", "0.11.4")
            .file("usr/bin/vi", "nvim")
            .ranked_alternative("vi", vi, neovim)
            .write(&out),
    ];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |index| {
        if default {
            index.defaults.alternatives.insert(
                "vi".parse().expect("a valid group"),
                "vim".parse().expect("a valid name"),
            );
        }
    });
    let (codes, report, _) = check(&repo, &[]);
    (codes, report)
}

#[test]
fn kr004_is_silent_when_one_member_has_the_highest_priority() {
    let (codes, report) = check_vi(5, 0, false);
    assert!(codes.is_empty(), "{report}");
}

#[test]
fn kr004_is_silent_for_a_tied_priority_with_a_default() {
    let (codes, report) = check_vi(5, 5, true);
    assert!(codes.is_empty(), "{report}");
}

#[test]
fn kr004_a_tied_priority_without_a_default() {
    let (codes, report) = check_vi(5, 5, false);
    assert_eq!(codes, ["KR004"], "{report}");
    assert_eq!(
        report["findings"][0]["message"],
        "alternatives group vi has no default, and neovim, vim share the highest priority 5"
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
fn kr004_a_member_that_does_not_ship_a_listed_path() {
    let dir = laft_test::tempdir();
    let packages = [pkg("vim", "9.1.0800")
        .file("usr/bin/vim", "vim")
        .alternative("vi", &["usr/bin/vi"])
        .write(&dir.path().join("out"))];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |index| {
        index
            .defaults
            .alternatives
            .insert("vi".parse().unwrap(), "vim".parse().unwrap());
    });
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR004"], "{report}");
    let finding = &report["findings"][0];
    assert_eq!(
        finding["message"],
        "alternatives group vi of vim 9.1.0800-r1 lists usr/bin/vi, which the package does not ship"
    );
    assert_eq!(finding["paths"], serde_json::json!(["usr/bin/vi"]));
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
fn kr005_the_same_revision_with_other_option_values_is_another_build() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let first = pkg("hello", "1.0")
        .option("tool", laft_core::OptionValue::TRUE)
        .file("usr/bin/hello", "one")
        .write(&dir.path().join("a"));
    let second = pkg("hello", "1.0")
        .option("tool", laft_core::OptionValue::FALSE)
        .file("usr/bin/hello", "two")
        .write(&dir.path().join("b"));
    fixture(&repo, "testing", 0, &[first], |_| {});
    fixture(&repo, "current", 0, &[second], |_| {});
    let (codes, report, _) = check(&repo, &[]);
    assert!(
        !codes.contains(&"KR005".to_owned()),
        "different option values are different builds: {report}"
    );
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
fn kr007_a_default_that_names_no_provider_does_not_choose() {
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
    fixture(&repo, "current", 0, &packages, |index| {
        index
            .defaults
            .providers
            .insert("so:libjpeg.so.8".into(), "libjpegturbo".parse().unwrap());
    });
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR007"], "{report}");
    assert_eq!(
        report["findings"][0]["details"][0],
        "the default libjpegturbo does not provide so:libjpeg.so.8"
    );
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
    fs::write(repo.join(&file), garbage).unwrap();
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR008"], "{report}");
    assert_eq!(report["findings"][0]["paths"], serde_json::json!([file]));
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
fn kr009_a_package_file_that_differs_from_its_entry() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let index = fixture(
        &repo,
        "current",
        0,
        &[pkg("hello", "1.0").write(&dir.path().join("out"))],
        |index| index.packages[0].size += 1,
    );
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR009"], "{report}");
    let finding = &report["findings"][0];
    assert_eq!(
        finding["paths"],
        serde_json::json!([index.packages[0].file])
    );
    assert!(
        finding["message"]
            .as_str()
            .unwrap()
            .starts_with("package file differs from its entry"),
        "{report}"
    );
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
fn ci003_buildinfo_of_another_format_or_without_recipe_state() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let index = fixture(
        &repo,
        "current",
        0,
        &[pkg("hello", "1.0").write(&dir.path().join("out"))],
        |_| {},
    );
    let id = index.packages[0].id;
    let output = serde_json::json!([{"id": id}]);
    let buildinfo = repo.join(format!("buildinfo/{id}.json"));
    for written in [
        serde_json::json!({"format": 2, "recipe_repos": [{"dirty": false}], "outputs": output}),
        serde_json::json!({"format": 1, "outputs": output}),
        serde_json::json!({"format": 1, "recipe_repos": [{"name": "core"}], "outputs": output}),
    ] {
        fs::write(&buildinfo, written.to_string()).unwrap();
        let (codes, report, _) = check(&repo, &[]);
        assert_eq!(codes, ["CI003"], "{written}: {report}");
    }
}

/// The index declarations of the Bool option `name`, default `value`.
fn bool_decl(name: &str, default: bool, doc: &str) -> laft_index::OptionDecl {
    laft_index::OptionDecl {
        name: name.to_owned(),
        kind: laft_index::OptionType::Bool("bool".to_owned()),
        default: laft_core::OptionValue::Bool(default),
        doc: doc.to_owned(),
    }
}

#[test]
fn kr014_option_metadata_that_is_not_what_the_recipe_declares() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");

    let declarations = |index: &mut laft_index::Index| {
        index.options.insert(
            "hello".parse().unwrap(),
            vec![bool_decl("tool", true, "Ship the tool.")],
        );
    };
    let repo = dir.path().join("repo");
    let packages = [pkg("hello", "1.0")
        .option("tool", laft_core::OptionValue::TRUE)
        .write(&out)];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, report, success) = check(&repo, &[]);
    assert!(codes.is_empty(), "{report}");
    assert!(success);

    let extra = dir.path().join("extra");
    let packages = [pkg("hello", "1.0")
        .option("tool", laft_core::OptionValue::TRUE)
        .option("nope", laft_core::OptionValue::FALSE)
        .write(&extra)];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR014"], "{report}");
    assert!(!success);
    assert!(
        report["findings"][0]["details"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d
                .as_str()
                .unwrap()
                .contains("which the recipe does not declare")),
        "{report}"
    );

    let missing = dir.path().join("missing");
    let packages = [pkg("hello", "1.0").write(&missing)];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, _, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR014"], "{codes:?}");
    assert!(!success);

    let wrong = dir.path().join("wrong");
    let packages = [pkg("hello", "1.0")
        .option("tool", laft_core::OptionValue::tag("amd"))
        .write(&wrong)];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR014"], "{report}");
    assert!(!success);
}

/// A subpackage carries the options of the recipe that built it, so KR014
/// looks the declarations and the profile values up by origin, not by the
/// package's own name.
#[test]
fn kr014_a_subpackage_is_checked_against_its_recipe_declarations() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let declarations = |index: &mut laft_index::Index| {
        index.options.insert(
            "mesa".parse().unwrap(),
            vec![bool_decl("llvm", true, "Build the LLVM backend.")],
        );
    };
    let repo = dir.path().join("repo");
    let packages = [
        pkg("mesa", "25.1")
            .option("llvm", laft_core::OptionValue::TRUE)
            .write(&out),
        pkg("mesa-dev", "25.1")
            .origin("mesa")
            .option("llvm", laft_core::OptionValue::TRUE)
            .write(&out),
    ];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, report, success) = check(&repo, &[]);
    assert!(codes.is_empty(), "{report}");
    assert!(success);

    // A value the recipe does not declare is refused on the subpackage too.
    let extra = dir.path().join("extra");
    let packages = [
        pkg("mesa", "25.1")
            .option("llvm", laft_core::OptionValue::TRUE)
            .write(&extra),
        pkg("mesa-dev", "25.1")
            .origin("mesa")
            .option("llvm", laft_core::OptionValue::TRUE)
            .option("nope", laft_core::OptionValue::FALSE)
            .write(&extra),
    ];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR014"], "{report}");
    assert!(
        report["findings"][0]["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.as_str().unwrap().contains("mesa-dev")),
        "{report}"
    );

    // The profile's promise covers the subpackage through the origin.
    let profile = dir.path().join("profile");
    let declarations = |index: &mut laft_index::Index| {
        index.options.insert(
            "mesa".parse().unwrap(),
            vec![bool_decl("llvm", true, "Build the LLVM backend.")],
        );
        index.profile = Some(laft_index::ProfileMember {
            name: "minimal".to_owned(),
            values: [(
                "mesa".parse::<laft_core::Name>().unwrap(),
                [("llvm".to_owned(), laft_core::OptionValue::FALSE)]
                    .into_iter()
                    .collect(),
            )]
            .into_iter()
            .collect(),
        });
    };
    let packages = [
        pkg("mesa", "25.1")
            .option("llvm", laft_core::OptionValue::FALSE)
            .write(&profile),
        pkg("mesa-dev", "25.1")
            .origin("mesa")
            .option("llvm", laft_core::OptionValue::TRUE)
            .write(&profile),
    ];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, report, _) = check(&repo, &[]);
    assert_eq!(codes, ["KR014"], "{report}");
    assert!(
        report["findings"][0]["details"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d.as_str().unwrap().contains("the profile minimal sets")),
        "{report}"
    );
}

/// A profile index rebuilds dependents with new store ids, so KR005
/// compares only indexes that are not profiles.
#[test]
fn kr005_compares_only_indexes_that_are_not_profiles() {
    let dir = laft_test::tempdir();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    let a = pkg("hello", "1.0").file("usr/bin/hello", "a").write(&first);
    let b = pkg("hello", "1.0")
        .file("usr/bin/hello", "b")
        .write(&second);
    assert_ne!(support::entry(&a).id, support::entry(&b).id);

    // Two plain channels: the same identity with another store id is a
    // forgotten revision bump.
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, std::slice::from_ref(&a), |_| {});
    fixture(&repo, "stable", 0, std::slice::from_ref(&b), |_| {});
    let (codes, report, _) = check(&repo, &[]);
    assert!(codes.contains(&"KR005".to_owned()), "{report}");

    // A sibling profile lists a rebuilt build with a new id: building a
    // profile rebuilds the packages built against its changed ones.
    let repo = dir.path().join("profiles");
    fixture(&repo, "current", 0, &[a], |_| {});
    let b_entry = support::entry(&b);
    fs::create_dir_all(repo.join("pool")).unwrap();
    fs::copy(&b, repo.join(format!("pool/{}.lpkg", b_entry.id))).unwrap();
    fs::create_dir_all(repo.join("buildinfo")).unwrap();
    let buildinfo = b.with_extension("").with_extension("");
    fs::copy(
        PathBuf::from(format!("{}.buildinfo.json", buildinfo.to_string_lossy())),
        repo.join(format!("buildinfo/{}.json", b_entry.id)),
    )
    .unwrap();
    let mut profile = support::read_index(&repo, "current");
    profile.generation += HOUR;
    profile.timestamp = laft_core::Timestamp::from_secs(profile.generation);
    profile.expires = laft_core::Timestamp::from_secs(profile.generation + DAY);
    profile.packages = vec![support::entry(&b)];
    profile.profile = Some(laft_index::ProfileMember {
        name: "minimal".to_owned(),
        values: Default::default(),
    });
    let profile_dir = repo.join("channels/current/x86_64-v3/profiles/minimal");
    fs::create_dir_all(&profile_dir).unwrap();
    fs::write(
        profile_dir.join("index.json.zst"),
        profile.encode().unwrap(),
    )
    .unwrap();

    let (codes, report, _) = check(&repo, &[]);
    assert!(codes.is_empty(), "{report}");
    let output = support::run(&[
        "check",
        "--repo",
        s(&repo),
        "--channel",
        "current",
        "--profile",
        "minimal",
        "--json",
    ]);
    let report: Value =
        serde_json::from_slice(&output.stdout).expect("check --json prints a report");
    let codes: Vec<String> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap().to_owned())
        .collect();
    assert!(codes.is_empty(), "{report}");
}

#[test]
fn kr016_a_recipe_snapshot_the_repository_does_not_hold() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let content = b"archive";
    let hash = laft_core::Blake3Hash::of(content);
    let member = |snapshot: laft_index::RecipeSnapshot| {
        move |index: &mut laft_index::Index| {
            index.recipes = Some(snapshot);
        }
    };

    // The repository holds the archive the member names.
    let repo = dir.path().join("repo");
    let archive = laft_index::RecipeSnapshot {
        file: format!("recipes/{hash}.tar.zst"),
        size: content.len() as u64,
        blake3: hash,
    };
    let packages = [pkg("hello", "1.0").write(&out)];
    fs::create_dir_all(repo.join("recipes")).unwrap();
    fs::write(repo.join(&archive.file), content).unwrap();
    fixture(&repo, "current", 0, &packages, member(archive));
    let (codes, report, success) = check(&repo, &[]);
    assert!(codes.is_empty(), "{report}");
    assert!(success);

    // The archive is missing.
    let repo = dir.path().join("repo2");
    let packages = [pkg("hello", "1.0").write(&out)];
    let dangling = laft_index::RecipeSnapshot {
        file: format!("recipes/{hash}.tar.zst"),
        size: content.len() as u64,
        blake3: hash,
    };
    fixture(&repo, "current", 0, &packages, member(dangling));
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR016"], "{report}");
    assert!(!success);

    // The archive holds other bytes than its hash.
    let repo = dir.path().join("repo3");
    let packages = [pkg("hello", "1.0").write(&out)];
    fs::create_dir_all(repo.join("recipes")).unwrap();
    fs::write(repo.join(format!("recipes/{hash}.tar.zst")), b"other").unwrap();
    let swapped = laft_index::RecipeSnapshot {
        file: format!("recipes/{hash}.tar.zst"),
        size: 5,
        blake3: hash,
    };
    fixture(&repo, "current", 0, &packages, member(swapped));
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR016"], "{report}");
    assert!(!success);
    assert!(
        report["findings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("differs from"),
        "{report}"
    );
}

#[test]
fn kr014_a_package_that_does_not_match_the_profile_member() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let declarations = |index: &mut laft_index::Index| {
        index.options.insert(
            "hello".parse().unwrap(),
            vec![bool_decl("strip", true, "Strip binaries.")],
        );
        index.profile = Some(laft_index::ProfileMember {
            name: "minimal".to_owned(),
            values: [(
                "hello".parse::<laft_core::Name>().unwrap(),
                [("strip".to_owned(), laft_core::OptionValue::FALSE)]
                    .into_iter()
                    .collect(),
            )]
            .into_iter()
            .collect(),
        });
    };

    // The build carries exactly the values the profile promises.
    let repo = dir.path().join("repo");
    let packages = [pkg("hello", "1.0")
        .option("strip", laft_core::OptionValue::FALSE)
        .write(&out)];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, report, _) = check(&repo, &[]);
    assert!(codes.is_empty(), "{report}");

    // A build with another value breaks the promise of the profile.
    let broken = dir.path().join("broken");
    let packages = [pkg("hello", "1.1")
        .option("strip", laft_core::OptionValue::TRUE)
        .write(&broken)];
    fixture(&repo, "current", HOUR, &packages, declarations);
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR014"], "{report}");
    assert!(!success);
    assert!(
        report["findings"][0]["details"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d.as_str().unwrap().contains("the profile minimal sets")),
        "{report}"
    );

    // A build that does not set the option breaks it too.
    let unset = dir.path().join("unset");
    let packages = [pkg("hello", "1.2").write(&unset)];
    fixture(&repo, "current", HOUR, &packages, declarations);
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR014"], "{report}");
    assert!(!success);
    assert!(
        report["findings"][0]["details"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d.as_str().unwrap().contains("the profile minimal sets")),
        "{report}"
    );
}

#[test]
fn kr015_a_constraint_the_target_does_not_declare() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let declarations = |index: &mut laft_index::Index| {
        index.options.insert(
            "hello".parse().unwrap(),
            vec![bool_decl("tool", true, "Ship the tool.")],
        );
    };

    let repo = dir.path().join("repo");
    let packages = [
        pkg("hello", "1.0")
            .option("tool", laft_core::OptionValue::TRUE)
            .write(&out),
        pkg("app", "1.0")
            .depends("hello{tool}")
            .file("usr/bin/app", "app")
            .write(&out),
    ];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, report, success) = check(&repo, &[]);
    assert!(codes.is_empty(), "{report}");
    assert!(success);

    let packages = [
        pkg("hello", "1.0")
            .option("tool", laft_core::OptionValue::TRUE)
            .write(&out),
        pkg("app", "1.0")
            .depends("hello{vulkan=amd}")
            .file("usr/bin/app", "app")
            .write(&out),
    ];
    fixture(&repo, "current", 0, &packages, declarations);
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR015"], "{report}");
    assert!(!success);
    assert!(
        report["findings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("does not declare"),
        "{report}"
    );

    let packages = [
        pkg("zlib", "1.2").write(&out),
        pkg("app", "1.0")
            .depends("zlib{tool}")
            .file("usr/bin/app", "app")
            .write(&out),
    ];
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, success) = check(&repo, &[]);
    assert_eq!(codes, ["KR015"], "{report}");
    assert!(!success);
    assert!(
        report["findings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("cannot be verified"),
        "{report}"
    );
}

/// Writes a recipe repository whose recipe `hello` reads its option in
/// `use_option` style: either in the summary or not at all.
fn recipe_repo(dir: &Path, uses_option: bool) -> PathBuf {
    let recipes = dir.join("recipes-src");
    fs::create_dir_all(recipes.join("recipes/hello")).expect("writable");
    fs::write(recipes.join("recipes.stav"), "recipe_repo core {}\n").expect("writable");
    fs::write(
        recipes.join("recipes/hello/options.stav"),
        "struct Options {\n  ## Ship the extra command line tool.\n  tool: Bool = true\n}\n",
    )
    .expect("writable");
    let body = if uses_option {
        "  summary = if opt.tool { \"with tool\" } else { \"without tool\" }\n"
    } else {
        "  summary = \"Hello\"\n"
    };
    fs::write(
        recipes.join("recipes/hello/package.stav"),
        format!(
            "import \"laft/options\" as opt\n\npackage hello {{\n  version = \"1.0\"\n{body}  license = [MIT]\n  empty = true\n  build custom {{}}\n}}\n"
        ),
    )
    .expect("writable");
    recipes
}

#[test]
fn kr012_an_option_that_does_not_change_the_plan() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [pkg("app", "1.0").file("usr/bin/app", "app").write(&out)];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let recipes = recipe_repo(dir.path(), false);
    let (codes, report, success) = check(&repo, &["--recipes", s(&recipes)]);
    assert_eq!(codes, ["KR012"], "{report}");
    assert!(success, "a warning does not fail the check");
    assert_eq!(report["findings"][0]["level"], "warning", "{report}");

    let used = recipe_repo(dir.path(), true);
    let (codes, report, success) = check(&repo, &["--recipes", s(&used)]);
    assert!(codes.is_empty(), "{report}");
    assert!(success);
}

/// Writes a recipe repository for `hello` with the Bool options `names`
/// and a summary that uses all of them together.
fn pair_repo(dir: &Path, names: &[&str], body: &str) -> PathBuf {
    let recipes = dir.join("recipes-src");
    fs::create_dir_all(recipes.join("recipes/hello")).expect("writable");
    fs::write(recipes.join("recipes.stav"), "recipe_repo core {}\n").expect("writable");
    let mut options = String::from("struct Options {\n");
    for name in names {
        options.push_str(&format!(
            "  ## The {name} switch.\n  {name}: Bool = false\n"
        ));
    }
    options.push_str("}\n");
    fs::write(recipes.join("recipes/hello/options.stav"), options).expect("writable");
    fs::write(recipes.join("recipes/hello/package.stav"), body).expect("writable");
    recipes
}

#[test]
fn kr012_reports_an_effect_that_needs_two_options() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [pkg("app", "1.0").file("usr/bin/app", "app").write(&out)];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let recipes = pair_repo(
        dir.path(),
        &["alpha", "beta"],
        "import \"laft/options\" as opt\n\npackage hello {\n  version = \"1.0\"\n  summary = if opt.alpha && opt.beta { \"both on\" } else { \"plain\" }\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    );
    let (codes, report, success) = check(&repo, &["--recipes", s(&recipes)]);
    assert!(
        codes.is_empty(),
        "the effect needs both values together: {report}"
    );
    assert!(success);
}

#[test]
fn kr012_notes_the_combinations_it_did_not_evaluate() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [pkg("app", "1.0").file("usr/bin/app", "app").write(&out)];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let names = ["a", "b", "c", "d", "e", "f", "g"];
    let recipes = dir.path().join("recipes-src");
    fs::create_dir_all(recipes.join("recipes/hello")).expect("writable");
    fs::write(recipes.join("recipes.stav"), "recipe_repo core {}\n").expect("writable");
    let mut options = String::new();
    for name in names {
        let kind = name.to_uppercase();
        options.push_str(&format!("type {kind} = {name}0 | {name}1 | {name}2\n"));
    }
    options.push_str("struct Options {\n");
    for name in names {
        let kind = name.to_uppercase();
        options.push_str(&format!(
            "  ## The {name} flavor.\n  {name}: {kind} = {name}0\n"
        ));
    }
    options.push_str("}\n");
    fs::write(recipes.join("recipes/hello/options.stav"), options).expect("writable");
    fs::write(
        recipes.join("recipes/hello/package.stav"),
        "package hello {\n  version = \"1.0\"\n  summary = \"Hello\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");
    let (codes, report, success) = check(&repo, &["--recipes", s(&recipes)]);
    assert_eq!(codes, ["KR012"], "{report}");
    assert!(success, "a warning does not fail the check");
    let message = report["findings"][0]["message"].as_str().unwrap();
    assert!(message.contains("option without effect"), "{report}");
    let detail = report["findings"][0]["details"][0].as_str().unwrap();
    assert!(
        detail.contains("the first 64 of 128 combinations"),
        "{report}"
    );
}

#[test]
fn kr012_skips_a_recipe_that_builds_nothing_here() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [pkg("app", "1.0").file("usr/bin/app", "app").write(&out)];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let recipes = pair_repo(
        dir.path(),
        &["alpha"],
        "package hello {\n  version = \"1.0\"\n  summary = \"Hello\"\n  arch = [\"aarch64\"]\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    );
    let (codes, report, success) = check(&repo, &["--recipes", s(&recipes)]);
    assert_eq!(codes, ["KR012"], "{report}");
    assert!(success, "a warning does not fail the check");
    let message = report["findings"][0]["message"].as_str().unwrap();
    assert!(message.contains("builds nothing"), "{report}");
    assert!(!message.contains("option without effect"), "{report}");
}

#[test]
fn kr012_reports_declarations_it_cannot_read() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [pkg("app", "1.0").file("usr/bin/app", "app").write(&out)];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let recipes = pair_repo(
        dir.path(),
        &["alpha"],
        "package hello {\n  version = \"1.0\"\n  summary = \"Hello\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    );
    // The declaration lost the doc comment its grammar requires.
    fs::write(
        recipes.join("recipes/hello/options.stav"),
        "struct Options {\n  alpha: Bool = false\n}\n",
    )
    .expect("writable");
    let (codes, report, success) = check(&repo, &["--recipes", s(&recipes)]);
    assert_eq!(codes, ["KR012"], "{report}");
    assert!(!success, "a load error is an error, not a silent skip");
    assert!(
        report["findings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("cannot read the option declarations"),
        "{report}"
    );
}

#[test]
fn kr013_names_a_recipe_no_repository_defines() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [pkg("app", "1.0").file("usr/bin/app", "app").write(&out)];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});
    let recipes = dir.path().join("recipes-src");
    fs::create_dir_all(recipes.join("recipes/hello")).expect("writable");
    fs::write(
        recipes.join("recipes.stav"),
        "recipe_repo core {}\n\npublish ghost {\n  options {\n    tool = false\n  }\n}\n",
    )
    .expect("writable");
    fs::write(
        recipes.join("recipes/hello/package.stav"),
        "package hello {\n  version = \"1.0\"\n  summary = \"Hello\"\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");
    let (codes, report, _) = check(&repo, &["--recipes", s(&recipes)]);
    assert_eq!(codes, ["KR013"], "{report}");
    let message = report["findings"][0]["message"].as_str().unwrap();
    assert!(message.contains("no repository defines"), "{report}");
}

#[test]
fn kr015_names_the_unverifiable_target_precisely() {
    let dir = laft_test::tempdir();
    let repo = dir.path().join("repo");
    let recipes = dir.path().join("recipes-src");
    fs::create_dir_all(recipes.join("recipes/zlib")).expect("writable");
    fs::write(recipes.join("recipes.stav"), "recipe_repo core {}\n").expect("writable");
    fs::write(
        recipes.join("recipes/zlib/package.stav"),
        "package zlib {\n  version = \"1.2\"\n  summary = \"Deflate\"\n  license = [Zlib]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");

    // No repository defines the target at all.
    let out = dir.path().join("out-ghost");
    let packages = [
        pkg("zlib", "1.2").write(&out),
        pkg("app", "1.0")
            .depends("ghost{tool}")
            .file("usr/bin/app", "app")
            .write(&out),
    ];
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &["--recipes", s(&recipes)]);
    assert_eq!(codes, ["KR002", "KR015"], "{report}");
    let message = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "KR015")
        .expect("the KR015 finding")["message"]
        .as_str()
        .unwrap();
    assert!(
        message.contains("no repository defines the package ghost"),
        "{report}"
    );

    // The target exists but declares no options.
    let out = dir.path().join("out-plain");
    let packages = [
        pkg("zlib", "1.2").write(&out),
        pkg("app", "1.0")
            .depends("zlib{tool}")
            .file("usr/bin/app", "app")
            .write(&out),
    ];
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &["--recipes", s(&recipes)]);
    assert_eq!(codes, ["KR015"], "{report}");
    let message = report["findings"][0]["message"].as_str().unwrap();
    assert!(message.contains("declares no options"), "{report}");
}

#[test]
fn kr015_verifies_the_constraints_the_recipes_declare() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let repo = dir.path().join("repo");
    let recipes = dir.path().join("recipes-src");
    fs::create_dir_all(recipes.join("recipes/zlib")).expect("writable");
    fs::write(recipes.join("recipes.stav"), "recipe_repo core {}\n").expect("writable");
    fs::write(
        recipes.join("recipes/zlib/options.stav"),
        "struct Options {\n  ## Ship the extra tools.\n  tool: Bool = true\n}\n",
    )
    .expect("writable");
    fs::write(
        recipes.join("recipes/zlib/package.stav"),
        "import \"laft/options\" as opt\n\npackage zlib {\n  version = \"1.2\"\n  summary = if opt.tool { \"with tools\" } else { \"plain\" }\n  license = [Zlib]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");

    // The index records no declarations, but the recipe does, so the
    // constraint is verified against it and holds. The listed zlib sets no
    // option, so KR014 has nothing to compare.
    let packages = [
        pkg("zlib", "1.2").write(&out),
        pkg("app", "1.0")
            .depends("zlib{tool}")
            .file("usr/bin/app", "app")
            .write(&out),
    ];
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &["--recipes", s(&recipes)]);
    assert!(codes.is_empty(), "{report}");

    // A constraint the recipe does not declare is named precisely.
    let packages = [
        pkg("zlib", "1.2").write(&out),
        pkg("app", "1.0")
            .depends("zlib{nope}")
            .file("usr/bin/app", "app")
            .write(&out),
    ];
    fixture(&repo, "current", 0, &packages, |_| {});
    let (codes, report, _) = check(&repo, &["--recipes", s(&recipes)]);
    assert_eq!(codes, ["KR015"], "{report}");
    let message = report["findings"][0]["message"].as_str().unwrap();
    assert!(message.contains("does not declare"), "{report}");
    assert!(!message.contains("cannot be verified"), "{report}");
}

#[test]
fn kr013_a_publish_block_or_profile_the_recipes_do_not_declare() {
    let dir = laft_test::tempdir();
    let out = dir.path().join("out");
    let packages = [pkg("app", "1.0").file("usr/bin/app", "app").write(&out)];
    let repo = dir.path().join("repo");
    fixture(&repo, "current", 0, &packages, |_| {});

    let unknown = dir.path().join("unknown");
    fs::create_dir_all(unknown.join("recipes/hello")).expect("writable");
    fs::write(
        unknown.join("recipes.stav"),
        "recipe_repo core {}\n\npublish hello {\n  options {\n    nope = false\n  }\n}\n",
    )
    .expect("writable");
    fs::write(
        unknown.join("recipes/hello/options.stav"),
        "struct Options {\n  ## Ship the extra command line tool.\n  tool: Bool = true\n}\n",
    )
    .expect("writable");
    fs::write(
        unknown.join("recipes/hello/package.stav"),
        "import \"laft/options\" as opt\n\npackage hello {\n  version = \"1.0\"\n  summary = if opt.tool { \"with tool\" } else { \"without tool\" }\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");
    let (codes, report, success) = check(&repo, &["--recipes", s(&unknown)]);
    assert_eq!(codes, ["KR013"], "{report}");
    assert!(!success);

    let without = dir.path().join("without");
    fs::create_dir_all(without.join("recipes/zlib")).expect("writable");
    fs::write(
        without.join("recipes.stav"),
        "recipe_repo core {}\n\npublish zlib {\n  options {\n    tool = false\n  }\n}\n",
    )
    .expect("writable");
    fs::write(
        without.join("recipes/zlib/package.stav"),
        "package zlib {\n  version = \"1.2\"\n  summary = \"Deflate\"\n  license = [Zlib]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");
    let (codes, report, success) = check(&repo, &["--recipes", s(&without)]);
    assert_eq!(codes, ["KR013"], "{report}");
    assert!(!success);
    assert!(
        report["findings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("declares no options"),
        "{report}"
    );

    let valid = dir.path().join("valid");
    fs::create_dir_all(valid.join("recipes/hello")).expect("writable");
    fs::write(
        valid.join("recipes/hello/options.stav"),
        "struct Options {\n  ## Ship the extra command line tool.\n  tool: Bool = true\n}\n",
    )
    .expect("writable");
    fs::write(
        valid.join("recipes/hello/package.stav"),
        "import \"laft/options\" as opt\n\npackage hello {\n  version = \"1.0\"\n  summary = if opt.tool { \"with tool\" } else { \"without tool\" }\n  license = [MIT]\n  empty = true\n  build custom {}\n}\n",
    )
    .expect("writable");
    fs::write(
        valid.join("recipes.stav"),
        "recipe_repo core {}\n\npublish hello {\n  options {\n    tool = false\n  }\n}\n",
    )
    .expect("writable");
    let (codes, report, success) = check(&repo, &["--recipes", s(&valid)]);
    assert!(codes.is_empty(), "{report}");
    assert!(success);
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
