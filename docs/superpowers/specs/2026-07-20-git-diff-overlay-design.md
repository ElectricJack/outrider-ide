# Git Diff Overlay Design

Adds git diff awareness to Outrider's treemap so users can visually focus on what's changed in a repository.

## Overview

Two independent toggles and a diff source selector provide four useful view states:

| Changes | Focus | Result |
|---------|-------|--------|
| OFF | OFF | Normal treemap |
| ON | OFF | Full treemap with diff annotations overlaid |
| ON | ON | Only changed files at full brightness, everything else dimmed |
| OFF | ON | Changed files at full brightness, no diff coloring |

A diff source selector defaults to "Working Tree vs HEAD" and can be switched to view changes from a specific commit.

## Data Model

New types in `outrider-index/src/types.rs`:

```rust
pub enum DiffStatus {
    Staged,
    Unstaged,
    Both,
    Added,
    Deleted,
}

pub struct DiffHunk {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: HunkKind,
    pub staged: bool,       // true = staged hunk, false = unstaged hunk
}

pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}
```

New fields on `SymbolNode`:

```rust
pub diff_status: Option<DiffStatus>,  // None = unmodified
pub diff_hunks: Vec<DiffHunk>,        // empty for folders, populated for files
```

Folders inherit a `DiffStatus` if any descendant is modified, following the same post-order traversal pattern as churn annotation. The specific `DiffStatus` variant on a folder is not used — only `Some` vs `None` matters for the dimming logic, and folders do not get diff stripes. Any non-None value suffices. This ensures folders containing modified files remain visible/highlighted at all zoom levels.

## Diff Computation

New module `outrider-index/src/diff.rs`, following the `churn.rs` pattern.

### Git commands

For working tree diffs:
- `git diff --numstat HEAD` — unstaged file-level changes
- `git diff --numstat --cached` — staged file-level changes
- `git diff -U0 HEAD` — unstaged line-level hunks
- `git diff -U0 --cached` — staged line-level hunks
- `git ls-files --others --exclude-standard` — untracked (added) files

For commit view:
- `git diff --numstat <commit>~1 <commit>` — file-level changes in a commit
- `git diff -U0 <commit>~1 <commit>` — line-level hunks in a commit

### Pipeline

1. `diff_status(repo_root, source)` → runs numstat commands, produces `BTreeMap<String, DiffStatus>`. Files appearing in both staged and unstaged get `DiffStatus::Both`.
2. `diff_hunks(repo_root, source)` → runs `-U0` commands, parses `@@ -a,b +c,d @@` headers into `BTreeMap<String, Vec<DiffHunk>>`.
3. `annotate_diff(tree, statuses, hunks)` → post-order traversal: files get their status and hunks directly, folders get a status if any child has one.

### Diff source

```rust
pub enum DiffSource {
    WorkingTree,
    Commit(String),
}
```

Controls which git arguments are passed. The annotation pipeline is identical regardless of source.

## Paint Pipeline

### Settings

Two new flags on `Settings`:

```rust
pub show_changes: bool,   // "Changes" toggle
pub focus_changes: bool,   // "Focus" toggle
```

### Changes toggle — stripe rendering

When `show_changes` is on, modified files get a colored stripe following the existing churn stripe pattern. Stripe colors by status:

- **Staged** → green
- **Unstaged** → orange
- **Both** → yellow
- **Added** → blue
- **Deleted** → red (commit view only)

The diff stripe renders on the right edge of the node box to avoid collision with the churn stripe (left edge).

### Focus toggle — dimming

When `focus_changes` is on, nodes with `diff_status: None`:
- Fill color gets heavily desaturated and pushed toward the background
- `body_opacity` and `tex_opacity` drop to ~0.15
- Churn stripe suppressed
- Node remains present, clickable, and occupies its treemap space

Nodes with a `DiffStatus` render at full brightness, making them pop against the dimmed background.

Folders with any modified descendant render at full brightness. Only folders where nothing inside changed get dimmed. This preserves container structure around modified files.

## Line-Level Highlighting

### At text LOD (zoomed in)

When zoomed in to `LeafDraw::Text` level (`font >= MIN_TEXT_FONT_PX`), lines within `diff_hunks` ranges get a subtle background tint:
- Green background tint for added/modified lines in staged hunks
- Orange background tint for unstaged hunks
- Applied as a background rect behind syntax-highlighted text, similar to the existing `highlighted` field on `BodyText` for call-graph call sites

Deleted lines are not shown inline (they don't exist in the current file). Deletion gutter markers are deferred.

### At minimap LOD (zoomed out)

When viewing the pre-baked texture minimap (`LeafDraw::Minimap`), diff hunks render as thin colored horizontal bars overlaid on the texture at the corresponding vertical positions. This provides a "scrollbar minimap" view — where in the file changes are concentrated — even before zooming in.

These indicators are drawn on top during paint, not baked into the texture cache. This avoids texture invalidation when diff state changes.

## Refresh Mechanism

Diff state is ephemeral and must be refreshed periodically, unlike churn which is computed once on load.

### Triggers

- **On load** — diff computed alongside churn during initial indexing
- **Polling timer** — every 2 seconds, a background task runs the git diff commands and re-annotates the existing `SymbolNode` fields, then calls `cx.notify()` to repaint
- **On window focus** — immediate refresh when Outrider regains focus

### Constraints

- Refresh only updates diff annotations on the existing tree; it does not detect new files added since last index (full re-index handles that)
- Diff source changes (switching the commit picker) trigger an immediate refresh, not a re-index
- No caching — the git commands are fast (typically under 50ms for normal working trees)

## Toolbar UI

### Toggles

Two new toggle buttons in the toolbar row alongside the existing "Git Churn" toggle, using the same `toolbar_toggle()` pattern:

- **"Changes"** — toggles `show_changes`
- **"Focus"** — toggles `focus_changes`

### Diff source selector

A small dropdown to the right of the toggles, visible only when `show_changes` is on. Defaults to "Working Tree."

Options:
- **Working Tree** — diff vs HEAD with staged/unstaged distinction
- **Commit...** — opens a mini-palette (reusing existing palette/search infrastructure) for fuzzy-searching commits

The commit palette runs `git log --oneline -50` to populate recent commits. Once selected, the toolbar shows a truncated commit label (e.g. `a3f21b4 Fix layout bug`) with an "x" to clear back to Working Tree.

## Scope

### Included
- Changes toggle with per-status colored stripes
- Focus toggle with opacity dimming
- Line-level diff highlighting at text and minimap LOD
- Working tree diff source (staged + unstaged)
- Commit diff source with palette picker
- 2-second polling refresh + window-focus refresh

### Deferred
- Branch range comparison (`main..feature`)
- Filesystem watcher (could replace or supplement polling)
- Re-layout mode for Focus (collapsing unmodified nodes to reclaim space)
