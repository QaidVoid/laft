use super::*;

#[test]
fn equal_texts_have_no_diff() {
    assert_eq!(unified("a", "b", "x\ny\n", "x\ny\n"), "");
}

#[test]
fn a_changed_line_shows_with_its_context() {
    let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
    let new = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n";
    assert_eq!(
        unified("/etc/x", "generation 3", old, new),
        "--- /etc/x\n+++ generation 3\n@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+five\n 6\n 7\n 8\n"
    );
}

#[test]
fn distant_changes_get_separate_hunks_and_additions_count_from_zero() {
    let old: String = (1..=20).map(|i| format!("{i}\n")).collect();
    let new = old
        .replace("2\n", "two\n")
        .replacen("19\n", "nineteen\n", 1);
    let diff = unified("a", "b", &old, &new);
    assert_eq!(diff.matches("@@ -").count(), 2, "{diff}");
    assert_eq!(
        unified("a", "b", "", "new\n"),
        "--- a\n+++ b\n@@ -0,0 +1,1 @@\n+new\n"
    );
}

#[test]
fn a_difference_in_the_final_newline_is_shown() {
    let diff = unified("a", "b", "x\n", "x");
    assert_eq!(diff, "--- a\n+++ b\n\\ No newline at end of file\n");
}

#[test]
fn etc_trees_show_added_removed_and_changed_files() {
    let dir = laft_test::tempdir();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    for (root, files) in [
        (
            &old,
            &[("motd", "hello\n"), ("gone", "x\n"), ("same", "s\n")][..],
        ),
        (
            &new,
            &[
                ("motd", "hi\n"),
                ("ssh/sshd_config", "Port 22\n"),
                ("same", "s\n"),
            ][..],
        ),
    ] {
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }
    std::os::unix::fs::symlink("/usr/share/zoneinfo/UTC", new.join("localtime")).unwrap();
    let diff = etc_trees(Some(&old), &new).unwrap();
    assert!(
        diff.contains("--- /etc/motd\n+++ /etc/motd\n@@ -1,1 +1,1 @@\n-hello\n+hi\n"),
        "{diff}"
    );
    assert!(diff.contains("--- /etc/gone\n+++ /dev/null\n"), "{diff}");
    assert!(
        diff.contains("+++ /etc/ssh/sshd_config\n@@ -0,0 +1,1 @@\n+Port 22\n"),
        "{diff}"
    );
    assert!(diff.contains("+-> /usr/share/zoneinfo/UTC"), "{diff}");
    assert!(!diff.contains("same"), "{diff}");
}
