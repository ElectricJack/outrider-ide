//! Integration-style tests for the diff module's parsing functions.

use std::collections::BTreeMap;

use outrider_index::diff::{
    merge_diff_statuses, parse_deleted_lines, parse_hunk_header, parse_hunks, parse_numstat,
    parse_range_spec,
};
use outrider_index::types::{DiffStatus, HunkKind};

#[test]
fn numstat_parses_basic_output() {
    let input = "10\t2\tsrc/main.rs\n3\t0\tREADME.md\n";
    let files = parse_numstat(input);
    assert!(files.contains_key("src/main.rs"));
    assert!(files.contains_key("README.md"));
    assert_eq!(files.len(), 2);
}

#[test]
fn numstat_handles_empty_input() {
    assert!(parse_numstat("").is_empty());
    assert!(parse_numstat("\n\n").is_empty());
}

#[test]
fn numstat_handles_binary_files() {
    let input = "-\t-\timage.png\n";
    let files = parse_numstat(input);
    assert!(files.contains_key("image.png"));
}

#[test]
fn merge_staged_files_get_staged_status() {
    let unstaged = BTreeMap::new();
    let mut staged = BTreeMap::new();
    staged.insert("a.rs".to_string(), ());
    let result = merge_diff_statuses(&unstaged, &staged);
    assert_eq!(result.get("a.rs"), Some(&DiffStatus::Staged));
    assert_eq!(result.len(), 1);
}

#[test]
fn merge_unstaged_files_get_unstaged_status() {
    let mut unstaged = BTreeMap::new();
    unstaged.insert("a.rs".to_string(), ());
    let staged = BTreeMap::new();
    let result = merge_diff_statuses(&unstaged, &staged);
    assert_eq!(result.get("a.rs"), Some(&DiffStatus::Unstaged));
}

#[test]
fn merge_files_in_both_get_both_status() {
    let mut unstaged = BTreeMap::new();
    unstaged.insert("a.rs".to_string(), ());
    let mut staged = BTreeMap::new();
    staged.insert("a.rs".to_string(), ());
    let result = merge_diff_statuses(&unstaged, &staged);
    assert_eq!(result.get("a.rs"), Some(&DiffStatus::Both));
}

#[test]
fn merge_disjoint_sets() {
    let mut unstaged = BTreeMap::new();
    unstaged.insert("a.rs".to_string(), ());
    let mut staged = BTreeMap::new();
    staged.insert("b.rs".to_string(), ());
    let result = merge_diff_statuses(&unstaged, &staged);
    assert_eq!(result.get("a.rs"), Some(&DiffStatus::Unstaged));
    assert_eq!(result.get("b.rs"), Some(&DiffStatus::Staged));
    assert_eq!(result.len(), 2);
}

#[test]
fn hunk_header_added_lines() {
    let hunks = parse_hunk_header("@@ -10,0 +11,3 @@ fn main()", false);
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].kind, HunkKind::Added);
    assert_eq!(hunks[0].start_line, 11);
    assert_eq!(hunks[0].end_line, 13);
    assert!(!hunks[0].staged);
}

#[test]
fn hunk_header_deleted_lines() {
    let hunks = parse_hunk_header("@@ -5,3 +4,0 @@ fn old()", true);
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].kind, HunkKind::Deleted);
    assert!(hunks[0].staged);
}

#[test]
fn hunk_header_modified_splits_into_deleted_and_added() {
    let hunks = parse_hunk_header("@@ -1,4 +1,6 @@", false);
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0].kind, HunkKind::Deleted);
    assert_eq!(hunks[0].start_line, 1);
    assert_eq!(hunks[1].kind, HunkKind::Added);
    assert_eq!(hunks[1].start_line, 1);
    assert_eq!(hunks[1].end_line, 6);
}

#[test]
fn hunks_parsed_across_multiple_files() {
    let diff = "\
diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,5 @@ fn main()
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,0 +11,2 @@ mod tests
@@ -20,2 +22,0 @@ fn helper()
";
    let hunks = parse_hunks(diff, false);
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks["src/main.rs"].len(), 2); // Modified splits into Deleted + Added
    assert_eq!(hunks["src/main.rs"][0].kind, HunkKind::Deleted);
    assert_eq!(hunks["src/main.rs"][1].kind, HunkKind::Added);
    assert_eq!(hunks["src/lib.rs"].len(), 2);
    assert_eq!(hunks["src/lib.rs"][0].kind, HunkKind::Added);
    assert_eq!(hunks["src/lib.rs"][1].kind, HunkKind::Deleted);
}

#[test]
fn hunks_staged_flag_propagated() {
    let diff = "\
diff --git a/foo.rs b/foo.rs
--- a/foo.rs
+++ b/foo.rs
@@ -1,2 +1,4 @@
";
    let hunks = parse_hunks(diff, true);
    assert!(hunks["foo.rs"][0].staged);
}

#[test]
fn range_spec_with_count() {
    assert_eq!(parse_range_spec("+10,3"), Some((10, 3)));
    assert_eq!(parse_range_spec("-5,0"), Some((5, 0)));
}

#[test]
fn range_spec_without_count() {
    assert_eq!(parse_range_spec("+7"), Some((7, 1)));
    assert_eq!(parse_range_spec("-1"), Some((1, 1)));
}

#[test]
fn range_spec_invalid() {
    assert_eq!(parse_range_spec("abc"), None);
    assert_eq!(parse_range_spec(""), None);
}

#[test]
fn deleted_lines_extracted_from_diff() {
    let diff = "\
diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -5,3 +5,2 @@
-old line five
-old line six
-old line seven
+new line five
+new line six
";
    let deleted = parse_deleted_lines(diff);
    assert_eq!(deleted.len(), 1);
    let lines = &deleted["src/main.rs"];
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].position, 5);
    assert_eq!(lines[0].text, "old line five");
    assert_eq!(lines[1].text, "old line six");
    assert_eq!(lines[2].text, "old line seven");
}

#[test]
fn deleted_lines_pure_deletion() {
    let diff = "\
diff --git a/lib.rs b/lib.rs
--- a/lib.rs
+++ b/lib.rs
@@ -10,2 +9,0 @@
-removed one
-removed two
";
    let deleted = parse_deleted_lines(diff);
    let lines = &deleted["lib.rs"];
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].position, 9);
    assert_eq!(lines[1].position, 9);
}

#[test]
fn deleted_lines_multiple_hunks() {
    let diff = "\
diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
@@ -1,1 +1,1 @@
-old first
+new first
@@ -10,1 +10,0 @@
-old tenth
";
    let deleted = parse_deleted_lines(diff);
    let lines = &deleted["f.rs"];
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].position, 1);
    assert_eq!(lines[0].text, "old first");
    assert_eq!(lines[1].position, 10);
    assert_eq!(lines[1].text, "old tenth");
}

#[test]
fn deleted_lines_skips_file_header() {
    let diff = "\
diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
@@ -1,1 +1,0 @@
-content
";
    let deleted = parse_deleted_lines(diff);
    let lines = &deleted["f.rs"];
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "content");
}
