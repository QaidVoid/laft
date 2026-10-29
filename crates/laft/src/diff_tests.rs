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
