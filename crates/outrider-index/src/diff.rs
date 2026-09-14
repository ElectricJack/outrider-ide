//! Git diff analysis: working-tree and commit-level diffs parsed into
//! per-file `DiffStatus` and `DiffHunk` annotations, then applied to the
//! `SymbolTree`. Follows the same pattern as `churn.rs`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::churn::{git_command, git_stdout};
use crate::types::{DeletedDiffLine, DiffHunk, DiffSource, DiffStatus, HunkKind, SymbolKind, SymbolNode, SymbolTree};

/// Aggregated diff result: per-file statuses and hunks.
#[derive(Debug, Clone, Default)]
pub struct DiffOutcome {
    pub statuses: BTreeMap<String, DiffStatus>,
    pub hunks: BTreeMap<String, Vec<DiffHunk>>,
    pub deleted_lines: BTreeMap<String, Vec<DeletedDiffLine>>,
}

/// Parse `git diff --numstat` output into a set of changed file paths.
pub fn parse_numstat(output: &str) -> BTreeMap<String, ()> {
    let mut files = BTreeMap::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(_added), Some(_deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        files.insert(path.to_string(), ());
    }
    files
}

/// Merge unstaged and staged file sets into combined `DiffStatus` per path.
pub fn merge_diff_statuses(
    unstaged: &BTreeMap<String, ()>,
    staged: &BTreeMap<String, ()>,
) -> BTreeMap<String, DiffStatus> {
    let mut result = BTreeMap::new();
    for path in unstaged.keys() {
        if staged.contains_key(path) {
            result.insert(path.clone(), DiffStatus::Both);
        } else {
            result.insert(path.clone(), DiffStatus::Unstaged);
        }
    }
    for path in staged.keys() {
        if !unstaged.contains_key(path) {
            result.insert(path.clone(), DiffStatus::Staged);
        }
    }
    result
}

/// Parse a `+start,count` or `-start,count` range spec from a hunk header.
pub fn parse_range_spec(s: &str) -> Option<(u32, u32)> {
    let s = s.strip_prefix('+').or_else(|| s.strip_prefix('-'))?;
    if let Some((start_s, count_s)) = s.split_once(',') {
        let start: u32 = start_s.parse().ok()?;
        let count: u32 = count_s.parse().ok()?;
        Some((start, count))
    } else {
        let start: u32 = s.parse().ok()?;
        Some((start, 1))
    }
}

/// Parse a single `@@ -old_start,old_count +new_start,new_count @@` header.
/// Modified hunks (lines both removed and added) are split into a Deleted
/// marker followed by an Added range so they render as red + green.
pub fn parse_hunk_header(line: &str, staged: bool) -> Vec<DiffHunk> {
    let Some(line) = line.strip_prefix("@@") else { return Vec::new() };
    let Some(end) = line.find("@@") else { return Vec::new() };
    let range_part = &line[..end].trim();

    let mut parts = range_part.split_whitespace();
    let (Some(old_spec), Some(new_spec)) = (parts.next(), parts.next()) else {
        return Vec::new();
    };
    let (Some((_, old_count)), Some((new_start, new_count))) =
        (parse_range_spec(old_spec), parse_range_spec(new_spec))
    else {
        return Vec::new();
    };

    if old_count == 0 {
        vec![DiffHunk {
            start_line: new_start,
            end_line: new_start + new_count.saturating_sub(1),
            kind: HunkKind::Added,
            staged,
        }]
    } else if new_count == 0 {
        vec![DiffHunk {
            start_line: new_start,
            end_line: new_start,
            kind: HunkKind::Deleted,
            staged,
        }]
    } else {
        vec![
            DiffHunk {
                start_line: new_start,
                end_line: new_start,
                kind: HunkKind::Deleted,
                staged,
            },
            DiffHunk {
                start_line: new_start,
                end_line: new_start + new_count.saturating_sub(1),
                kind: HunkKind::Added,
                staged,
            },
        ]
    }
}

/// Parse unified diff output (`git diff -U0`) into per-file hunk lists.
pub fn parse_hunks(output: &str, staged: bool) -> BTreeMap<String, Vec<DiffHunk>> {
    let mut result: BTreeMap<String, Vec<DiffHunk>> = BTreeMap::new();
    let mut current_file: Option<String> = None;

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            current_file = Some(rest.to_string());
        } else if line.starts_with("+++ /dev/null") {
            // File was deleted — hunks will reference the --- a/ path
        } else if let Some(rest) = line.strip_prefix("--- a/") {
            // For deleted files, this is the file path
            if current_file.is_none() {
                // Will be overwritten if +++ b/ follows
            }
            let _ = rest; // used only by the +++ b/ branch
        } else if line.starts_with("@@") {
            if let Some(ref file) = current_file {
                let hunks = parse_hunk_header(line, staged);
                result.entry(file.clone()).or_default().extend(hunks);
            }
        } else if line.starts_with("diff --git") {
            // Reset for next file; +++ line will set the actual path
            current_file = None;
        }
    }
    result
}

/// Parse deleted-line text content from `git diff -U0` output.
/// Each `-` content line is recorded with the new-file position where it was removed.
pub fn parse_deleted_lines(output: &str) -> BTreeMap<String, Vec<DeletedDiffLine>> {
    let mut result: BTreeMap<String, Vec<DeletedDiffLine>> = BTreeMap::new();
    let mut current_file: Option<String> = None;
    let mut hunk_new_start: u32 = 0;

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            current_file = Some(rest.to_string());
        } else if line.starts_with("+++ ") {
            // +++ /dev/null or similar — skip
        } else if line.starts_with("--- ") {
            // --- a/file or --- /dev/null — skip
        } else if line.starts_with("@@") {
            if let Some(header) = line.strip_prefix("@@") {
                if let Some(end) = header.find("@@") {
                    let range_part = header[..end].trim();
                    if let Some(new_spec) = range_part.split_whitespace().nth(1) {
                        if let Some((start, _)) = parse_range_spec(new_spec) {
                            hunk_new_start = start;
                        }
                    }
                }
            }
        } else if line.starts_with("diff --git") {
            current_file = None;
        } else if let Some(text) = line.strip_prefix('-') {
            if let Some(ref file) = current_file {
                result.entry(file.clone()).or_default().push(DeletedDiffLine {
                    position: hunk_new_start,
                    text: text.to_string(),
                });
            }
        }
    }
    result
}

/// List untracked files in the repository.
pub fn untracked_files(repo_root: &Path) -> Vec<String> {
    match git_stdout(repo_root, &["ls-files", "--others", "--exclude-standard"]) {
        Ok(output) => output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Compute diff outcome for working tree changes (staged + unstaged).
pub fn working_tree_diff(repo_root: &Path) -> DiffOutcome {
    // Unstaged changes
    let unstaged_numstat = git_stdout(repo_root, &["diff", "--numstat"])
        .unwrap_or_default();
    let unstaged_files = parse_numstat(&unstaged_numstat);

    // Staged changes
    let staged_numstat = git_stdout(repo_root, &["diff", "--cached", "--numstat"])
        .unwrap_or_default();
    let staged_files = parse_numstat(&staged_numstat);

    // Merge statuses
    let mut statuses = merge_diff_statuses(&unstaged_files, &staged_files);

    // Untracked files get Added status
    for path in untracked_files(repo_root) {
        statuses.insert(path, DiffStatus::Added);
    }

    // Parse hunks from unified diffs
    let unstaged_diff = git_stdout(repo_root, &["diff", "-U0"])
        .unwrap_or_default();
    let staged_diff = git_stdout(repo_root, &["diff", "--cached", "-U0"])
        .unwrap_or_default();

    let mut hunks = parse_hunks(&unstaged_diff, false);
    for (file, file_hunks) in parse_hunks(&staged_diff, true) {
        hunks.entry(file).or_default().extend(file_hunks);
    }

    let mut deleted_lines = parse_deleted_lines(&unstaged_diff);
    for (file, lines) in parse_deleted_lines(&staged_diff) {
        deleted_lines.entry(file).or_default().extend(lines);
    }

    DiffOutcome { statuses, hunks, deleted_lines }
}

/// Compute diff outcome for a specific commit compared to its parent.
pub fn commit_diff(repo_root: &Path, sha: &str) -> DiffOutcome {
    let numstat = git_stdout(repo_root, &["diff", "--numstat", &format!("{sha}~1"), sha])
        .unwrap_or_default();
    let files = parse_numstat(&numstat);
    let statuses: BTreeMap<String, DiffStatus> = files
        .keys()
        .map(|p| (p.clone(), DiffStatus::Unstaged))
        .collect();

    let diff_output = git_stdout(repo_root, &["diff", "-U0", &format!("{sha}~1"), sha])
        .unwrap_or_default();
    let hunks = parse_hunks(&diff_output, false);
    let deleted_lines = parse_deleted_lines(&diff_output);

    DiffOutcome { statuses, hunks, deleted_lines }
}

/// Main entry point: compute diff outcome for a given source.
pub fn diff_outcome(repo_root: &Path, source: &DiffSource) -> DiffOutcome {
    // Verify git is available
    if git_command(repo_root)
        .arg("rev-parse")
        .arg("--git-dir")
        .output()
        .is_err()
    {
        return DiffOutcome::default();
    }

    match source {
        DiffSource::WorkingTree => working_tree_diff(repo_root),
        DiffSource::Commit(sha) => commit_diff(repo_root, sha),
    }
}

/// Annotate the symbol tree with diff information. Files get their status and
/// hunks directly; folders get a status if any descendant file has one.
pub fn annotate_diff(tree: &mut SymbolTree, outcome: &DiffOutcome) {
    set_diff(&mut tree.root, outcome);
}

/// Post-order traversal: assign diff data to file nodes, propagate to folders.
/// Returns true if any descendant has a diff status.
fn set_diff(node: &mut SymbolNode, outcome: &DiffOutcome) -> bool {
    match &node.id.kind {
        SymbolKind::File => {
            let path = &node.id.qualified_path;
            if let Some(&status) = outcome.statuses.get(path) {
                node.diff_status = Some(status);
            }
            if let Some(hunks) = outcome.hunks.get(path) {
                node.diff_hunks = hunks.clone();
            }
            if let Some(status) = node.diff_status {
                let file_deleted = outcome.deleted_lines.get(path)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                if node.diff_hunks.is_empty() {
                    mark_all_leaves(&mut node.children, status);
                } else {
                    let file_lines = node.measure.max(1) as f64;
                    let file_bytes = node
                        .byte_range
                        .as_ref()
                        .map(|r| r.end.max(1) as f64)
                        .unwrap_or(1.0);
                    mark_overlapping(
                        &mut node.children,
                        &node.diff_hunks,
                        status,
                        file_lines,
                        file_bytes,
                        file_deleted,
                    );
                }
            }
            node.diff_status.is_some()
        }
        SymbolKind::Folder => {
            node.children
                .iter_mut()
                .fold(false, |acc, child| set_diff(child, outcome) || acc)
        }
        _ => {
            false
        }
    }
}

fn mark_all_leaves(children: &mut [SymbolNode], status: DiffStatus) {
    for child in children.iter_mut() {
        if child.children.is_empty() {
            child.diff_status = Some(status);
        } else {
            mark_all_leaves(&mut child.children, status);
        }
    }
}

/// Mark only leaf symbols whose approximate line range overlaps a diff hunk.
fn mark_overlapping(
    children: &mut [SymbolNode],
    hunks: &[DiffHunk],
    status: DiffStatus,
    file_lines: f64,
    file_bytes: f64,
    deleted_lines: &[DeletedDiffLine],
) {
    for child in children.iter_mut() {
        let Some(ref range) = child.byte_range else {
            continue;
        };
        let start = (range.start as f64 / file_bytes * file_lines).floor() as u32 + 1;
        let end = (range.end as f64 / file_bytes * file_lines).ceil() as u32 + 1;
        let overlapping: Vec<DiffHunk> = hunks
            .iter()
            .filter(|h| start <= h.end_line && h.start_line <= end)
            .map(|h| DiffHunk {
                start_line: h.start_line.saturating_sub(start.saturating_sub(1)),
                end_line: h.end_line.saturating_sub(start.saturating_sub(1)),
                kind: h.kind,
                staged: h.staged,
            })
            .collect();
        if !overlapping.is_empty() {
            if child.children.is_empty() {
                child.diff_status = Some(status);
                child.diff_hunks = overlapping;
                child.deleted_lines = deleted_lines
                    .iter()
                    .filter(|d| d.position >= start && d.position <= end)
                    .map(|d| DeletedDiffLine {
                        position: d.position.saturating_sub(start.saturating_sub(1)),
                        text: d.text.clone(),
                    })
                    .collect();
            } else {
                mark_overlapping(&mut child.children, hunks, status, file_lines, file_bytes, deleted_lines);
            }
        }
    }
}

/// Clear all diff annotations from the tree.
pub fn clear_diff(tree: &mut SymbolTree) {
    clear_node(&mut tree.root);
}

fn clear_node(node: &mut SymbolNode) {
    node.diff_status = None;
    node.diff_hunks.clear();
    node.deleted_lines.clear();
    for child in &mut node.children {
        clear_node(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numstat_basic() {
        let input = "10\t2\tsrc/main.rs\n3\t0\tREADME.md\n";
        let files = parse_numstat(input);
        assert!(files.contains_key("src/main.rs"));
        assert!(files.contains_key("README.md"));
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn parse_numstat_empty() {
        let files = parse_numstat("");
        assert!(files.is_empty());
    }

    #[test]
    fn parse_numstat_binary_files() {
        let input = "-\t-\tlogo.png\n5\t3\tsrc/lib.rs\n";
        let files = parse_numstat(input);
        assert!(files.contains_key("logo.png"));
        assert!(files.contains_key("src/lib.rs"));
    }

    #[test]
    fn merge_staged_only() {
        let unstaged = BTreeMap::new();
        let mut staged = BTreeMap::new();
        staged.insert("src/main.rs".into(), ());
        let result = merge_diff_statuses(&unstaged, &staged);
        assert_eq!(result.get("src/main.rs"), Some(&DiffStatus::Staged));
    }

    #[test]
    fn merge_unstaged_only() {
        let mut unstaged = BTreeMap::new();
        unstaged.insert("src/main.rs".into(), ());
        let staged = BTreeMap::new();
        let result = merge_diff_statuses(&unstaged, &staged);
        assert_eq!(result.get("src/main.rs"), Some(&DiffStatus::Unstaged));
    }

    #[test]
    fn merge_both() {
        let mut unstaged = BTreeMap::new();
        unstaged.insert("src/main.rs".into(), ());
        let mut staged = BTreeMap::new();
        staged.insert("src/main.rs".into(), ());
        let result = merge_diff_statuses(&unstaged, &staged);
        assert_eq!(result.get("src/main.rs"), Some(&DiffStatus::Both));
    }

    #[test]
    fn parse_hunk_header_added() {
        let header = "@@ -10,0 +11,3 @@ fn main()";
        let hunks = parse_hunk_header(header, false);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Added);
        assert_eq!(hunks[0].start_line, 11);
        assert_eq!(hunks[0].end_line, 13);
        assert!(!hunks[0].staged);
    }

    #[test]
    fn parse_hunk_header_deleted() {
        let header = "@@ -5,3 +4,0 @@ fn old()";
        let hunks = parse_hunk_header(header, true);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Deleted);
        assert_eq!(hunks[0].start_line, 4);
        assert_eq!(hunks[0].end_line, 4);
        assert!(hunks[0].staged);
    }

    #[test]
    fn parse_hunk_header_modified_splits_into_deleted_and_added() {
        let header = "@@ -1,4 +1,6 @@";
        let hunks = parse_hunk_header(header, false);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].kind, HunkKind::Deleted);
        assert_eq!(hunks[0].start_line, 1);
        assert_eq!(hunks[0].end_line, 1);
        assert_eq!(hunks[1].kind, HunkKind::Added);
        assert_eq!(hunks[1].start_line, 1);
        assert_eq!(hunks[1].end_line, 6);
    }

    #[test]
    fn parse_hunk_header_single_line_modified() {
        let header = "@@ -1 +1 @@";
        let hunks = parse_hunk_header(header, false);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].kind, HunkKind::Deleted);
        assert_eq!(hunks[1].kind, HunkKind::Added);
        assert_eq!(hunks[1].start_line, 1);
        assert_eq!(hunks[1].end_line, 1);
    }

    #[test]
    fn parse_hunks_multiple_files() {
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
    fn parse_range_spec_with_count() {
        assert_eq!(parse_range_spec("+10,3"), Some((10, 3)));
        assert_eq!(parse_range_spec("-5,0"), Some((5, 0)));
    }

    #[test]
    fn parse_range_spec_without_count() {
        assert_eq!(parse_range_spec("+7"), Some((7, 1)));
        assert_eq!(parse_range_spec("-1"), Some((1, 1)));
    }

    #[test]
    fn parse_range_spec_invalid() {
        assert_eq!(parse_range_spec("abc"), None);
        assert_eq!(parse_range_spec(""), None);
    }

    #[test]
    fn parse_deleted_lines_basic() {
        let diff = "\
diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
@@ -5,2 +5,1 @@
-old five
-old six
+new five
";
        let deleted = parse_deleted_lines(diff);
        assert_eq!(deleted["f.rs"].len(), 2);
        assert_eq!(deleted["f.rs"][0].position, 5);
        assert_eq!(deleted["f.rs"][0].text, "old five");
        assert_eq!(deleted["f.rs"][1].text, "old six");
    }

    #[test]
    fn parse_deleted_lines_empty_diff() {
        let deleted = parse_deleted_lines("");
        assert!(deleted.is_empty());
    }
}
