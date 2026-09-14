# Git Diff Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add git diff awareness to Outrider's treemap — two toggles ("Changes" and "Focus") plus a commit picker let users highlight modified files, dim unmodified files, and see line-level diffs when zoomed in.

**Architecture:** Extends the existing churn annotation pattern: a new `diff` module in `outrider-index` runs git commands and annotates `SymbolNode` with diff status and hunk data. The paint pipeline in `outrider` reads these annotations to render stripes, dimming, and line-level highlights. A 2-second polling timer keeps diff state current.

**Tech Stack:** Rust, GPUI framework, `std::process::Command` for git, existing `outrider-index` and `outrider` crates.

**Spec:** `docs/superpowers/specs/2026-07-20-git-diff-overlay-design.md`

---

## File Structure

| Action | Path | Purpose |
|--------|------|---------|
| Create | `crates/outrider-index/src/diff.rs` | Git diff computation + tree annotation |
| Modify | `crates/outrider-index/src/types.rs:48-65` | Add `DiffStatus`, `DiffHunk`, `HunkKind` types + fields on `SymbolNode` |
| Modify | `crates/outrider-index/src/lib.rs:1-16` | Add `pub mod diff;` |
| Modify | `crates/outrider-index/src/index.rs:189-206` | Call `diff::annotate_diff` during indexing |
| Modify | `crates/outrider/src/settings.rs:34-51` | Add `show_changes` and `focus_changes` fields |
| Modify | `crates/outrider/src/theme.rs` | Add `diff_stripe_color` function + diff color constants |
| Modify | `crates/outrider/src/paint_model.rs:41-62` | Add `diff_stripe` field to `PaintItem`, `diff_highlight` to `BodyText` |
| Modify | `crates/outrider/src/treemap.rs:1682-1704` | Wire diff stripe + dimming into `paint_items()` |
| Modify | `crates/outrider/src/treemap.rs:4338-4351` | Render diff stripe on right edge |
| Modify | `crates/outrider/src/treemap.rs:4131-4148` | Add toolbar toggles + diff source selector |
| Modify | `crates/outrider/src/treemap.rs:512-567` | Add diff polling state to `TreemapView` |
| Create | `crates/outrider-index/tests/diff_test.rs` | Unit tests for diff parsing + annotation |

---

### Task 1: Data Model — Add Diff Types to `outrider-index`

**Files:**
- Modify: `crates/outrider-index/src/types.rs:48-65`
- Modify: `crates/outrider-index/src/lib.rs:1-16`

- [ ] **Step 1: Add diff types to types.rs**

Add these types before the `SymbolNode` struct (after line 45):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffStatus {
    Staged,
    Unstaged,
    Both,
    Added,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: HunkKind,
    pub staged: bool,
}
```

- [ ] **Step 2: Add diff fields to SymbolNode**

Add two new fields to the `SymbolNode` struct (after `churn_count: u64,` at line 62):

```rust
pub diff_status: Option<DiffStatus>,
pub diff_hunks: Vec<DiffHunk>,
```

- [ ] **Step 3: Update all SymbolNode construction sites**

Every place that constructs a `SymbolNode` needs the new fields defaulted. Search for `churn: 0.0` to find them all — there are construction sites in:
- `crates/outrider-index/src/index.rs` (lines 525-526, 555-556)
- `crates/outrider/src/treemap.rs` (lines 1182-1183, 5085-5086, 5308-5309)

Add after each `churn_count: 0,`:
```rust
diff_status: None,
diff_hunks: Vec::new(),
```

- [ ] **Step 4: Add DiffSource enum to types.rs**

Add after the `DiffHunk` struct:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSource {
    WorkingTree,
    Commit(String),
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p outrider-index -p outrider`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```bash
git add crates/outrider-index/src/types.rs crates/outrider-index/src/index.rs crates/outrider/src/treemap.rs
git commit -m "feat: add DiffStatus, DiffHunk, HunkKind types and fields on SymbolNode"
```

---

### Task 2: Diff Computation Module — `diff.rs`

**Files:**
- Create: `crates/outrider-index/src/diff.rs`
- Modify: `crates/outrider-index/src/lib.rs`
- Create: `crates/outrider-index/tests/diff_test.rs`

This module follows the `churn.rs` pattern. Reuse the git helper functions from `churn.rs` — specifically `git_command` (line 238) and `git_stdout` (line 245). Since these are private to `churn.rs`, either make them `pub(crate)` or duplicate them in `diff.rs`. Prefer making them `pub(crate)` to avoid duplication — extract `git_command` and `git_stdout` from `churn.rs` so both modules can use them.

- [ ] **Step 1: Extract shared git helpers from churn.rs**

In `crates/outrider-index/src/churn.rs`, change visibility of `git_command` (line 238) and `git_stdout` (line 245) from `fn` to `pub fn` (they are needed cross-crate by Task 13's commit log fetcher).

- [ ] **Step 2: Write failing tests for diff status parsing**

Create `crates/outrider-index/tests/diff_test.rs`:

```rust
use outrider_index::diff::{parse_numstat, DiffFileStatus};
use outrider_index::types::DiffStatus;
use std::collections::BTreeMap;

#[test]
fn parse_numstat_basic() {
    let output = "3\t1\tsrc/main.rs\n10\t0\tsrc/new_file.rs\n";
    let result = parse_numstat(output);
    assert_eq!(result.len(), 2);
    assert!(result.contains_key("src/main.rs"));
    assert!(result.contains_key("src/new_file.rs"));
}

#[test]
fn parse_numstat_empty() {
    let result = parse_numstat("");
    assert!(result.is_empty());
}

#[test]
fn merge_staged_unstaged() {
    use outrider_index::diff::merge_diff_statuses;
    let mut unstaged = BTreeMap::new();
    unstaged.insert("src/main.rs".to_string(), ());
    let mut staged = BTreeMap::new();
    staged.insert("src/main.rs".to_string(), ());
    staged.insert("src/lib.rs".to_string(), ());

    let merged = merge_diff_statuses(&unstaged, &staged);
    assert_eq!(merged.get("src/main.rs"), Some(&DiffStatus::Both));
    assert_eq!(merged.get("src/lib.rs"), Some(&DiffStatus::Staged));
}
```

Run: `cargo test -p outrider-index --test diff_test`
Expected: compilation error (module doesn't exist yet)

- [ ] **Step 3: Add diff module declaration**

In `crates/outrider-index/src/lib.rs`, add after `pub mod churn;` (line 9):

```rust
pub mod diff;
```

- [ ] **Step 4: Implement diff status computation**

Create `crates/outrider-index/src/diff.rs`:

```rust
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::churn::{git_command, git_stdout};
use crate::types::{DiffHunk, DiffSource, DiffStatus, HunkKind, SymbolKind, SymbolNode, SymbolTree};

/// Parsed numstat entry (file path only — we just need to know which files changed).
pub fn parse_numstat(output: &str) -> BTreeMap<String, ()> {
    let mut result = BTreeMap::new();
    for line in output.lines() {
        // numstat format: "added\tremoved\tpath"
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() == 3 {
            let path = parts[2].to_string();
            result.insert(path, ());
        }
    }
    result
}

/// Merge unstaged and staged file sets into a single DiffStatus map.
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

/// Parse unified diff `-U0` output into per-file hunk lists.
pub fn parse_hunks(output: &str, staged: bool) -> BTreeMap<String, Vec<DiffHunk>> {
    let mut result: BTreeMap<String, Vec<DiffHunk>> = BTreeMap::new();
    let mut current_file: Option<String> = None;

    for line in output.lines() {
        if line.starts_with("+++ b/") {
            current_file = Some(line[6..].to_string());
        } else if line.starts_with("+++ /dev/null") {
            current_file = None; // deleted file
        } else if line.starts_with("@@ ") {
            if let Some(ref file) = current_file {
                if let Some(hunk) = parse_hunk_header(line, staged) {
                    result.entry(file.clone()).or_default().push(hunk);
                }
            }
        }
    }
    result
}

/// Parse a `@@ -a,b +c,d @@` header into a DiffHunk.
fn parse_hunk_header(line: &str, staged: bool) -> Option<DiffHunk> {
    // Format: @@ -old_start[,old_count] +new_start[,new_count] @@
    let at_end = line[3..].find("@@")?;
    let range_str = &line[3..3 + at_end].trim();
    let parts: Vec<&str> = range_str.split(' ').collect();
    if parts.len() != 2 {
        return None;
    }

    let old = parse_range_spec(parts[0].trim_start_matches('-'))?;
    let new = parse_range_spec(parts[1].trim_start_matches('+'))?;

    let kind = if old.1 == 0 {
        HunkKind::Added
    } else if new.1 == 0 {
        HunkKind::Deleted
    } else {
        HunkKind::Modified
    };

    if new.1 == 0 {
        // Deletion: report old-side range
        Some(DiffHunk {
            start_line: old.0,
            end_line: old.0.saturating_add(old.1).saturating_sub(1).max(old.0),
            kind,
            staged,
        })
    } else {
        Some(DiffHunk {
            start_line: new.0,
            end_line: new.0 + new.1 - 1,
            kind,
            staged,
        })
    }
}

/// Parse "start,count" or "start" (count defaults to 1) from a hunk range specifier.
fn parse_range_spec(s: &str) -> Option<(u32, u32)> {
    if let Some((start, count)) = s.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((s.parse().ok()?, 1))
    }
}

/// Untracked files (new files not yet staged).
fn untracked_files(repo_root: &Path) -> Vec<String> {
    let mut cmd = git_command(repo_root);
    cmd.args(["ls-files", "--others", "--exclude-standard"]);
    git_stdout(&mut cmd)
        .unwrap_or_default()
        .lines()
        .map(|s| s.to_string())
        .collect()
}

/// Full diff outcome: file statuses + per-file hunks.
pub struct DiffOutcome {
    pub statuses: BTreeMap<String, DiffStatus>,
    pub hunks: BTreeMap<String, Vec<DiffHunk>>,
}

/// Compute diff for the given source.
pub fn diff_outcome(repo_root: &Path, source: &DiffSource) -> DiffOutcome {
    match source {
        DiffSource::WorkingTree => working_tree_diff(repo_root),
        DiffSource::Commit(sha) => commit_diff(repo_root, sha),
    }
}

fn working_tree_diff(repo_root: &Path) -> DiffOutcome {
    // File-level: unstaged
    let mut cmd_unstaged = git_command(repo_root);
    cmd_unstaged.args(["diff", "--numstat", "HEAD"]);
    let unstaged_numstat = git_stdout(&mut cmd_unstaged).unwrap_or_default();
    let unstaged_files = parse_numstat(&unstaged_numstat);

    // File-level: staged
    let mut cmd_staged = git_command(repo_root);
    cmd_staged.args(["diff", "--numstat", "--cached"]);
    let staged_numstat = git_stdout(&mut cmd_staged).unwrap_or_default();
    let staged_files = parse_numstat(&staged_numstat);

    // Merge file statuses
    let mut statuses = merge_diff_statuses(&unstaged_files, &staged_files);

    // Untracked files
    for path in untracked_files(repo_root) {
        statuses.insert(path, DiffStatus::Added);
    }

    // Line-level hunks: unstaged
    let mut cmd_hunks_unstaged = git_command(repo_root);
    cmd_hunks_unstaged.args(["diff", "-U0", "HEAD"]);
    let unstaged_hunks_output = git_stdout(&mut cmd_hunks_unstaged).unwrap_or_default();
    let mut hunks = parse_hunks(&unstaged_hunks_output, false);

    // Line-level hunks: staged
    let mut cmd_hunks_staged = git_command(repo_root);
    cmd_hunks_staged.args(["diff", "-U0", "--cached"]);
    let staged_hunks_output = git_stdout(&mut cmd_hunks_staged).unwrap_or_default();
    for (path, file_hunks) in parse_hunks(&staged_hunks_output, true) {
        hunks.entry(path).or_default().extend(file_hunks);
    }

    DiffOutcome { statuses, hunks }
}

fn commit_diff(repo_root: &Path, sha: &str) -> DiffOutcome {
    let parent = format!("{}~1", sha);

    // File-level
    let mut cmd = git_command(repo_root);
    cmd.args(["diff", "--numstat", &parent, sha]);
    let numstat = git_stdout(&mut cmd).unwrap_or_default();
    let files = parse_numstat(&numstat);
    let statuses: BTreeMap<String, DiffStatus> = files
        .keys()
        .map(|p| (p.clone(), DiffStatus::Staged))
        .collect();

    // Line-level hunks
    let mut cmd_hunks = git_command(repo_root);
    cmd_hunks.args(["diff", "-U0", &parent, sha]);
    let hunks_output = git_stdout(&mut cmd_hunks).unwrap_or_default();
    let hunks = parse_hunks(&hunks_output, true);

    DiffOutcome { statuses, hunks }
}

/// Annotate a SymbolTree with diff data. Post-order traversal: files get their
/// status and hunks directly, folders get a status if any child has one.
pub fn annotate_diff(tree: &mut SymbolTree, outcome: &DiffOutcome) {
    set_diff_status(&mut tree.root, &outcome.statuses, &outcome.hunks, "");
}

fn set_diff_status(
    node: &mut SymbolNode,
    statuses: &BTreeMap<String, DiffStatus>,
    hunks: &BTreeMap<String, Vec<DiffHunk>>,
    prefix: &str,
) {
    let rel_path = if prefix.is_empty() {
        node.name.clone()
    } else {
        format!("{}/{}", prefix, node.name)
    };

    match &node.id.kind {
        SymbolKind::File => {
            node.diff_status = statuses.get(&rel_path).copied();
            node.diff_hunks = hunks.get(&rel_path).cloned().unwrap_or_default();
        }
        SymbolKind::Folder => {
            for child in &mut node.children {
                set_diff_status(child, statuses, hunks, &rel_path);
            }
            // Folder gets a status if any child has one
            let has_modified_child = node
                .children
                .iter()
                .any(|c| c.diff_status.is_some());
            if has_modified_child {
                node.diff_status = Some(DiffStatus::Unstaged); // variant unused for folders
            }
        }
        _ => {
            // Item/Chunk: inherit from parent file (set later in paint pipeline via node ref)
        }
    }
}

/// Clear all diff annotations from a tree (used before re-annotating).
pub fn clear_diff(tree: &mut SymbolTree) {
    clear_node(&mut tree.root);
}

fn clear_node(node: &mut SymbolNode) {
    node.diff_status = None;
    node.diff_hunks.clear();
    for child in &mut node.children {
        clear_node(child);
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p outrider-index --test diff_test`
Expected: all tests pass

- [ ] **Step 6: Add more tests for hunk parsing**

Add to `crates/outrider-index/tests/diff_test.rs`:

```rust
use outrider_index::diff::parse_hunks;
use outrider_index::types::{DiffHunk, HunkKind};

#[test]
fn parse_hunks_added_lines() {
    let output = "diff --git a/src/main.rs b/src/main.rs\n\
                  --- a/src/main.rs\n\
                  +++ b/src/main.rs\n\
                  @@ -0,0 +1,5 @@\n\
                  +line1\n+line2\n+line3\n+line4\n+line5\n";
    let result = parse_hunks(output, false);
    let hunks = result.get("src/main.rs").unwrap();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].start_line, 1);
    assert_eq!(hunks[0].end_line, 5);
    assert_eq!(hunks[0].kind, HunkKind::Added);
    assert!(!hunks[0].staged);
}

#[test]
fn parse_hunks_modified_lines() {
    let output = "diff --git a/src/lib.rs b/src/lib.rs\n\
                  --- a/src/lib.rs\n\
                  +++ b/src/lib.rs\n\
                  @@ -10,3 +10,4 @@\n";
    let result = parse_hunks(output, true);
    let hunks = result.get("src/lib.rs").unwrap();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].start_line, 10);
    assert_eq!(hunks[0].end_line, 13);
    assert_eq!(hunks[0].kind, HunkKind::Modified);
    assert!(hunks[0].staged);
}

#[test]
fn parse_hunks_deleted_lines() {
    let output = "diff --git a/src/old.rs b/src/old.rs\n\
                  --- a/src/old.rs\n\
                  +++ b/src/old.rs\n\
                  @@ -5,3 +5,0 @@\n";
    let result = parse_hunks(output, false);
    let hunks = result.get("src/old.rs").unwrap();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].kind, HunkKind::Deleted);
}

#[test]
fn parse_hunks_multiple_files() {
    let output = "diff --git a/a.rs b/a.rs\n\
                  --- a/a.rs\n\
                  +++ b/a.rs\n\
                  @@ -1,2 +1,3 @@\n\
                  diff --git a/b.rs b/b.rs\n\
                  --- a/b.rs\n\
                  +++ b/b.rs\n\
                  @@ -5,1 +5,2 @@\n";
    let result = parse_hunks(output, false);
    assert_eq!(result.len(), 2);
    assert!(result.contains_key("a.rs"));
    assert!(result.contains_key("b.rs"));
}
```

Run: `cargo test -p outrider-index --test diff_test`
Expected: all tests pass

- [ ] **Step 7: Verify full crate compiles**

Run: `cargo check -p outrider-index -p outrider`
Expected: compiles clean

- [ ] **Step 8: Commit**

```bash
git add crates/outrider-index/src/diff.rs crates/outrider-index/src/lib.rs crates/outrider-index/src/churn.rs crates/outrider-index/tests/diff_test.rs
git commit -m "feat: add diff.rs module for git diff computation and tree annotation"
```

---

### Task 3: Integrate Diff into Indexing Pipeline

**Files:**
- Modify: `crates/outrider-index/src/index.rs:189-206`

- [ ] **Step 1: Call diff annotation during indexing**

In `crates/outrider-index/src/index.rs`, after the churn annotation (line 196 `crate::churn::annotate(&mut tree, &churn.counts);`), add:

```rust
let diff = crate::diff::diff_outcome(repo_root, &crate::types::DiffSource::WorkingTree);
crate::diff::annotate_diff(&mut tree, &diff);
```

- [ ] **Step 2: Verify it compiles and tests pass**

Run: `cargo check -p outrider-index -p outrider && cargo test -p outrider-index`
Expected: compiles, all tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/outrider-index/src/index.rs
git commit -m "feat: annotate tree with diff status during indexing"
```

---

### Task 4: Settings — Add `show_changes` and `focus_changes`

**Files:**
- Modify: `crates/outrider/src/settings.rs:34-51`

- [ ] **Step 1: Add settings fields**

In `crates/outrider/src/settings.rs`, add two new fields to the `Settings` struct after `show_churn` (around line 48):

```rust
#[serde(default)]
pub show_changes: bool,
#[serde(default)]
pub focus_changes: bool,
```

Both default to `false` via `#[serde(default)]`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 3: Commit**

```bash
git add crates/outrider/src/settings.rs
git commit -m "feat: add show_changes and focus_changes settings"
```

---

### Task 5: Theme — Diff Stripe Colors

**Files:**
- Modify: `crates/outrider/src/theme.rs`

- [ ] **Step 1: Add diff color constants**

In `crates/outrider/src/theme.rs`, after the existing color constants (around line 38), add:

```rust
pub const DIFF_STAGED: u32 = 0x4ec94e;
pub const DIFF_UNSTAGED: u32 = 0xe08840;
pub const DIFF_BOTH: u32 = 0xd8c840;
pub const DIFF_ADDED: u32 = 0x5090d0;
pub const DIFF_DELETED: u32 = 0xd04040;
pub const DIFF_LINE_STAGED_BG: u32 = 0x30604030;
pub const DIFF_LINE_UNSTAGED_BG: u32 = 0x60402030;
```

- [ ] **Step 2: Add diff_stripe_color function**

After the `churn_heat` function (line 173), add:

```rust
pub fn diff_stripe_color(status: outrider_index::types::DiffStatus) -> u32 {
    use outrider_index::types::DiffStatus;
    match status {
        DiffStatus::Staged => DIFF_STAGED,
        DiffStatus::Unstaged => DIFF_UNSTAGED,
        DiffStatus::Both => DIFF_BOTH,
        DiffStatus::Added => DIFF_ADDED,
        DiffStatus::Deleted => DIFF_DELETED,
    }
}

pub fn diff_line_bg(staged: bool) -> u32 {
    if staged { DIFF_LINE_STAGED_BG } else { DIFF_LINE_UNSTAGED_BG }
}

/// Desaturate and darken a fill color for Focus mode dimming.
pub fn dimmed_fill(fill: u32) -> u32 {
    let r = ((fill >> 16) & 0xFF) as f32;
    let g = ((fill >> 8) & 0xFF) as f32;
    let b = (fill & 0xFF) as f32;
    let gray = (r * 0.299 + g * 0.587 + b * 0.114) * 0.3;
    let g = gray.clamp(0.0, 255.0) as u32;
    (g << 16) | (g << 8) | g
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 4: Commit**

```bash
git add crates/outrider/src/theme.rs
git commit -m "feat: add diff stripe colors and dimming helpers to theme"
```

---

### Task 6: Paint Model — Add Diff Fields to PaintItem and BodyText

**Files:**
- Modify: `crates/outrider/src/paint_model.rs:10-62`

- [ ] **Step 1: Add diff_stripe to PaintItem**

In `PaintItem` struct, after `stripe: Option<u32>,` (line 50), add:

```rust
pub(crate) diff_stripe: Option<u32>,
```

- [ ] **Step 2: Add diff_highlight to BodyText**

In `BodyText` struct, after `highlighted: bool,` (line 16), add:

```rust
pub(crate) diff_bg: Option<u32>,
```

- [ ] **Step 3: Update all PaintItem construction sites**

Search for `stripe:` in `treemap.rs` to find PaintItem constructions. Add `diff_stripe: None,` after each `stripe:` line. There should be one primary site around line 1691.

Similarly, search for `highlighted:` in `treemap.rs` and add `diff_bg: None,` after each occurrence (lines 880, 895, 1731).

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/paint_model.rs crates/outrider/src/treemap.rs
git commit -m "feat: add diff_stripe and diff_bg fields to paint model"
```

---

### Task 7: Paint Pipeline — Wire Diff Stripe + Dimming into `paint_items()`

**Files:**
- Modify: `crates/outrider/src/treemap.rs:1682-1704`

This is the core visual wiring. The PaintItem construction at line 1682 needs three changes.

- [ ] **Step 1: Compute diff stripe color**

In `paint_items()`, just before the `out.push(PaintItem { ... })` block (around line 1682), add:

```rust
let diff_stripe = (self.settings.show_changes)
    .then(|| item.node.diff_status)
    .flatten()
    .filter(|_| matches!(item.node.id.kind, SymbolKind::File))
    .map(theme::diff_stripe_color);
```

This only shows stripes on File nodes (not folders), gated by `show_changes`.

- [ ] **Step 2: Apply Focus dimming to fill**

Before the PaintItem construction, add logic to dim the fill color when Focus is on and the node has no diff status:

```rust
let fill = if self.settings.focus_changes && item.node.diff_status.is_none() {
    theme::dimmed_fill(fill)
} else {
    fill
};
```

This must be placed after the existing `fill` variable is computed (by `theme::box_fill(...)`) but before it's used in the PaintItem construction. Note: `fill` is first computed via `let fill = theme::box_fill(kind, level, tint);` — shadow it with the dimmed version.

- [ ] **Step 3: Apply Focus dimming to opacity**

Similarly, dim body and texture opacity. After the existing `body_opacity` and `tex_opacity` variables are computed, add:

```rust
let (body_opacity, tex_opacity) = if self.settings.focus_changes && item.node.diff_status.is_none() {
    (body_opacity * 0.15, tex_opacity * 0.15)
} else {
    (body_opacity, tex_opacity)
};
```

- [ ] **Step 4: Suppress churn stripe when dimmed**

Modify the existing stripe computation to suppress churn when focus-dimmed:

```rust
stripe: (self.settings.show_churn && item.node.churn > 0.0
    && !(self.settings.focus_changes && item.node.diff_status.is_none()))
    .then(|| theme::churn_heat(item.node.churn)),
```

- [ ] **Step 5: Wire diff_stripe into PaintItem construction**

In the `PaintItem { ... }` literal, set:

```rust
diff_stripe,
```

(using the variable computed in step 1).

- [ ] **Step 6: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 7: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: wire diff stripe and focus dimming into paint pipeline"
```

---

### Task 8: Render Diff Stripe on Right Edge

**Files:**
- Modify: `crates/outrider/src/treemap.rs:4338-4351`

- [ ] **Step 1: Add right-edge stripe rendering**

In the canvas paint callback, right after the existing churn stripe rendering block (lines 4338-4351, the `if let Some(heat) = item.stripe { ... }` block), add a similar block for the diff stripe:

```rust
if let Some(diff_color) = item.diff_stripe {
    let sb = Bounds::new(
        point(
            origin.x + px(item.x + item.w - 1.0 - theme::STRIPE_W),
            origin.y + px(item.y + 1.0),
        ),
        size(px(theme::STRIPE_W), px((item.h - 2.0).max(0.0))),
    );
    window.paint_quad(quad(
        sb,
        px(0.),
        rgb(diff_color),
        px(0.),
        rgb(diff_color),
        BorderStyle::default(),
    ));
}
```

This places the diff stripe at the right edge (`item.x + item.w - 1.0 - STRIPE_W`), mirroring the churn stripe on the left.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 3: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: render diff status stripe on right edge of nodes"
```

---

### Task 9: Line-Level Diff Highlighting

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (body text construction + canvas rendering)

- [ ] **Step 1: Wire diff_bg into BodyText for leaf text lines**

In `paint_items()`, where `BodyText` items are constructed for leaf code lines (around line 1731 where `highlighted: false` is set), add diff highlight logic. The line index (1-based) needs to be checked against the node's `diff_hunks`:

Find the code section in `paint_items()` where body text lines for leaf nodes are built. For each line, compute whether it falls in a diff hunk:

```rust
let diff_bg = if self.settings.show_changes {
    item.node.diff_hunks.iter().find(|h| {
        let line_num = (line_index + 1) as u32;  // 1-based
        line_num >= h.start_line && line_num <= h.end_line
    }).map(|h| theme::diff_line_bg(h.staged))
} else {
    None
};
```

Then set `diff_bg` on the `BodyText` being constructed.

Note: `line_index` will need to be tracked — look at how the existing body text loop iterates lines and adjust accordingly. The body text construction loops over syntax-highlighted lines from the buffer; `line_index` corresponds to the line number in the file.

- [ ] **Step 2: Render diff_bg in canvas paint callback**

In the canvas paint callback where `BodyText` items are rendered (around line 4406-4425, the existing `bt.highlighted` block), add a similar block for `diff_bg` right after:

```rust
if let Some(bg) = bt.diff_bg {
    let char_w = item.body_font_px * 0.62;
    let hw = char_w * bt.text.len() as f32 + 12.0;
    let hh = item.body_font_px * 1.3;
    window.paint_quad(quad(
        Bounds::new(
            point(origin.x + px(bt.x - 4.0), origin.y + px(bt.y)),
            size(px(hw), px(hh)),
        ),
        px(0.0),
        rgba(bg),
        px(0.),
        transparent_black(),
        BorderStyle::default(),
    ));
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 4: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: add line-level diff highlighting in zoomed-in text view"
```

---

### Task 10: Minimap Diff Indicators

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (canvas paint callback, texture rendering section)

- [ ] **Step 1: Add hunk bar overlays on texture/minimap quads**

In the canvas paint callback, after the texture quad is painted for a leaf node (search for `tex` rendering or `TexQuad`), add diff hunk indicators. These are thin horizontal colored bars overlaid at the vertical positions corresponding to the hunks.

Find the texture rendering section in the paint callback. After the texture quad is drawn, if `show_changes` is on, iterate the node's `diff_hunks` and paint thin bars:

```rust
// After texture quad painting, for the current item:
if self.settings.show_changes && !item.diff_hunks.is_empty() {
    let total_lines = item.node.measure.max(1) as f32;
    for hunk in &item.diff_hunks {
        let y_frac_start = (hunk.start_line as f32 - 1.0) / total_lines;
        let y_frac_end = hunk.end_line as f32 / total_lines;
        let bar_y = item.y + y_frac_start * item.h;
        let bar_h = ((y_frac_end - y_frac_start) * item.h).max(2.0);
        let color = theme::diff_line_bg(hunk.staged);
        window.paint_quad(quad(
            Bounds::new(
                point(origin.x + px(item.x), origin.y + px(bar_y)),
                size(px(item.w), px(bar_h)),
            ),
            px(0.0),
            rgba(color),
            px(0.),
            transparent_black(),
            BorderStyle::default(),
        ));
    }
}
```

Note: this requires `diff_hunks` to be accessible from the paint callback. Since `PaintItem` doesn't carry hunks, we have two options: (a) add a `diff_hunks` field to `PaintItem`, or (b) build a separate map during `paint_items()` keyed by item index. Option (a) is simpler — add `pub(crate) diff_hunks: Vec<DiffHunk>` to `PaintItem` and populate it during construction. This keeps the pattern consistent.

Add to `PaintItem`:
```rust
pub(crate) diff_hunks: Vec<outrider_index::types::DiffHunk>,
pub(crate) total_lines: u64,
```

Default to `diff_hunks: Vec::new(), total_lines: 0,` in all PaintItem constructions.

In `paint_items()`, populate from the node:
```rust
diff_hunks: if self.settings.show_changes { item.node.diff_hunks.clone() } else { Vec::new() },
total_lines: item.node.measure,
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 3: Commit**

```bash
git add crates/outrider/src/treemap.rs crates/outrider/src/paint_model.rs
git commit -m "feat: add minimap-level diff hunk indicators over texture quads"
```

---

### Task 11: Toolbar Toggles

**Files:**
- Modify: `crates/outrider/src/treemap.rs:4131-4148`

- [ ] **Step 1: Add Changes and Focus toggles**

Replace the existing toolbar overlay block (lines 4131-4148) with one that includes all three toggles in a row:

```rust
let toolbar_overlay = (!has_overlays && self.map_interaction_enabled()).then(|| {
    let show_churn = self.settings.show_churn;
    let show_changes = self.settings.show_changes;
    let focus_changes = self.settings.focus_changes;
    div()
        .absolute()
        .top(px(8.0))
        .right(px(8.0))
        .flex()
        .flex_row()
        .gap(px(4.0))
        .child(
            crate::overlays::toolbar_toggle("churn-toggle", "Git Churn", show_churn).on_click(
                cx.listener(|this, _event, _window, cx| {
                    this.settings.show_churn = !this.settings.show_churn;
                    this.global_settings.show_churn = this.settings.show_churn;
                    let _ = this.global_settings.save();
                    cx.notify();
                }),
            ),
        )
        .child(
            crate::overlays::toolbar_toggle("changes-toggle", "Changes", show_changes).on_click(
                cx.listener(|this, _event, _window, cx| {
                    this.settings.show_changes = !this.settings.show_changes;
                    this.global_settings.show_changes = this.settings.show_changes;
                    let _ = this.global_settings.save();
                    cx.notify();
                }),
            ),
        )
        .child(
            crate::overlays::toolbar_toggle("focus-toggle", "Focus", focus_changes).on_click(
                cx.listener(|this, _event, _window, cx| {
                    this.settings.focus_changes = !this.settings.focus_changes;
                    this.global_settings.focus_changes = this.settings.focus_changes;
                    let _ = this.global_settings.save();
                    cx.notify();
                }),
            ),
        )
});
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 3: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: add Changes and Focus toolbar toggles"
```

---

### Task 12: Diff Polling Refresh

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (TreemapView struct + poll cycle)

- [ ] **Step 1: Add diff polling state to TreemapView**

In the `TreemapView` struct (around line 566), add:

```rust
diff_source: outrider_index::types::DiffSource,
diff_last_poll: std::time::Instant,
```

Initialize in the constructor with:
```rust
diff_source: outrider_index::types::DiffSource::WorkingTree,
diff_last_poll: std::time::Instant::now(),
```

- [ ] **Step 2: Add poll_diff method**

Add a new method to `TreemapView`:

```rust
fn poll_diff(&mut self) -> bool {
    if !self.settings.show_changes && !self.settings.focus_changes {
        return false;
    }
    let now = std::time::Instant::now();
    if now.duration_since(self.diff_last_poll).as_secs() < 2 {
        return false;
    }
    self.diff_last_poll = now;
    self.refresh_diff();
    true
}

fn refresh_diff(&mut self) {
    outrider_index::diff::clear_diff(&mut self.tree);
    let outcome = outrider_index::diff::diff_outcome(&self.tree.repo_root, &self.diff_source);
    outrider_index::diff::annotate_diff(&mut self.tree, &outcome);
}
```

- [ ] **Step 3: Wire poll_diff into the render loop**

In the `render()` method (around line 4063), where `poll_loading`, `poll_call_graph`, and `poll_pre_scan` are called, add:

```rust
needs_notify |= self.poll_diff();
```

- [ ] **Step 4: Keep render loop alive when diff polling is active**

In the animation frame request block (around line 4109), add diff polling as a condition:

```rust
|| self.settings.show_changes || self.settings.focus_changes
```

This keeps `request_animation_frame()` firing so `poll_diff` gets called every frame (but only actually does work every 2 seconds).

- [ ] **Step 5: Add window focus refresh**

Add a handler so that when Outrider regains window focus, diff state refreshes immediately. In the existing `render()` method or an appropriate GPUI event handler, detect window activation and call:

```rust
self.diff_last_poll = std::time::Instant::now() - std::time::Duration::from_secs(3);
```

This forces the next `poll_diff()` call to fire immediately (the 2-second threshold will be exceeded). Look at how GPUI exposes window focus events — if there's a `WindowContext::on_focus` or similar, wire it to reset `diff_last_poll`. If GPUI doesn't expose a direct window focus event, the 2-second poll is sufficient as a fallback.

- [ ] **Step 6: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 7: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: add 2-second diff polling refresh cycle with window focus trigger"
```

---

### Task 13: Commit Picker UI

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (toolbar + palette integration)

- [ ] **Step 1: Add diff source state to TreemapView**

The `diff_source` field was already added in Task 12. Add a field to track the display label:

```rust
diff_source_label: Option<String>,
```

Initialize to `None` in the constructor.

- [ ] **Step 2: Add commit log fetcher**

Add a helper function:

```rust
fn recent_commits(repo_root: &Path) -> Vec<(String, String)> {
    let mut cmd = outrider_index::churn::git_command(repo_root);
    cmd.args(["log", "--oneline", "-50"]);
    outrider_index::churn::git_stdout(&mut cmd)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (hash, msg) = line.split_once(' ')?;
            Some((hash.to_string(), msg.to_string()))
        })
        .collect()
}
```

Note: `git_command` and `git_stdout` were already made `pub` in Task 2 Step 1 for cross-crate access.

- [ ] **Step 3: Add diff source indicator to toolbar**

In the toolbar overlay (from Task 11), after the Focus toggle, conditionally show a diff source indicator when `show_changes` is on:

```rust
.children(show_changes.then(|| {
    let label = self.diff_source_label.clone().unwrap_or_else(|| "Working Tree".into());
    div()
        .id("diff-source")
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .bg(rgba(0x00000080))
        .child(
            div()
                .text_size(px(12.0))
                .font_family(theme::FONT_FAMILY_SANS)
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(label),
        )
        .on_click(cx.listener(|this, _event, _window, cx| {
            // Open commit picker palette
            this.open_commit_palette(cx);
        }))
}))
```

- [ ] **Step 4: Implement commit palette**

Add a method that populates the existing palette with recent commits:

```rust
fn open_commit_palette(&mut self, cx: &mut Context<Self>) {
    let commits = recent_commits(&self.tree.repo_root);
    // Prepend "Working Tree" as first option to allow clearing
    let mut entries: Vec<(String, String)> = vec![("working-tree".into(), "Working Tree".into())];
    entries.extend(commits);
    // Use palette in a new "commit" mode — reuse the palette infrastructure
    // to show entries and handle selection via select_commit()
    self.palette.open_commit_picker(entries);
    cx.notify();
}

fn select_commit(&mut self, hash: &str) {
    if hash == "working-tree" {
        self.diff_source = DiffSource::WorkingTree;
        self.diff_source_label = None;
    } else {
        self.diff_source = DiffSource::Commit(hash.to_string());
        self.diff_source_label = Some(format!("{}", &hash[..7.min(hash.len())]));
    }
    self.refresh_diff();
}
```

The exact palette integration depends on how `palette::Palette` exposes mode switching. Look at how the existing file search (Ctrl+P) and symbol search (Ctrl+T) modes are distinguished, and follow the same pattern to add a commit-picker mode. The palette already handles fuzzy matching and selection — feed it the commit list as entries.

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p outrider`
Expected: compiles clean

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/treemap.rs crates/outrider-index/src/churn.rs
git commit -m "feat: add commit picker UI for diff source selection"
```

---

### Task 14: Integration Test — Full Build + Manual Verification

**Files:**
- No new files

- [ ] **Step 1: Full build**

Run: `cargo build -p outrider --release 2>&1 | tail -5`
Expected: builds successfully

- [ ] **Step 2: Run all tests**

Run: `cargo test -p outrider-index`
Expected: all tests pass including new diff tests

- [ ] **Step 3: Verify no clippy warnings**

Run: `cargo clippy -p outrider-index -p outrider -- -D warnings 2>&1 | tail -20`
Expected: no warnings

- [ ] **Step 4: Commit any fixups**

If any clippy or compile issues were found and fixed:

```bash
git add -A
git commit -m "fix: address clippy warnings and compile issues"
```
