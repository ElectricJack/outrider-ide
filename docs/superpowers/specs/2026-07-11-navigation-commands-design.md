# Navigation Commands — Design

**Date:** 2026-07-11
**Status:** Approved

## Goal

Add four navigation features to outrider: fuzzy file search, fuzzy symbol
search, back/forward navigation history, and open-in-file-manager — bringing
the app's keyboard navigation closer to VS Code/Sublime conventions.

## 1. Search Palette

A reusable `Palette` component (text input + filtered results list) rendered
as a centered overlay at the top of the map viewport (~500px wide, top 40%
of the screen, matching VS Code command-palette positioning).

### Activation

- `Ctrl+P` — file mode (candidates = all File-kind SymbolIds from the tree)
- `Ctrl+T` — symbol mode (candidates = all non-Folder SymbolIds: items,
  files, chunks)

### Behavior

- Typing filters candidates by fuzzy subsequence match on `node.name`
  (case-insensitive). A character in the query must appear in order in the
  candidate name, but not necessarily contiguously.
- Results: max 12 visible rows. Each row shows the name, then a dimmed
  qualified path for context (e.g. `parse_rust_items  crates/outrider-index/src/parse.rs`).
- Up/Down arrows navigate the list selection. Enter confirms. Esc dismisses
  (without navigating).
- On selection: set focus to the chosen node, tween the camera to frame it,
  push a history entry, close the palette.
- While the palette is open, all keystrokes go to it (map key handler is
  suppressed).

### Implementation

New module `crates/outrider/src/palette.rs` owning:
- `open: bool`
- `mode: PaletteMode` (File | Symbol)
- `query: String`
- `results: Vec<SymbolId>` (filtered, capped)
- `selection: usize` (index into results)

Rendered in the `Render` impl as a `div().absolute()` overlay above the
canvas. The palette reads the symbol tree to build candidates on open, then
filters on each keystroke.

Fuzzy matching: for each candidate, check whether the query chars appear as
a subsequence of the name (case-folded). No scoring or ranking beyond
"shorter names first, then alphabetical" for tied matches.

## 2. Navigation History

A linear history buffer with a movable cursor (browser-style back/forward).

### Data

On `TreemapView`:
```
nav_history: Vec<SymbolId>
nav_cursor: usize
```

Initialized with the starting focus (the root) at index 0.

### Recording

A new entry is pushed (and any forward entries are truncated) when focus
changes via:
- Enter (step_in)
- Escape (step_out)
- Click (set)
- Palette selection

Arrow-key steps between siblings are NOT recorded — they are local
exploration, not jumps.

### Navigating

- `Alt+Left` — move cursor back one entry, set focus + tween camera
- `Alt+Right` — move cursor forward one entry, set focus + tween camera

Back/forward moves do NOT themselves push new history entries (they only
slide the cursor). Max history depth: 64 entries; when exceeded, drop the
oldest entry and adjust the cursor.

## 3. Open File Location

### Hotkey

`Ctrl+Shift+E` (mnemonic: Explorer)

### Behavior

Resolve the focused node's file path:
- For File/Item/Chunk nodes: strip the `::symbol` suffixes from
  `qualified_path` to get the relative file path, join with `repo_root`.
- For Folder nodes: join the `qualified_path` (which is already a relative
  dir path) with `repo_root`.

Then open the system file manager:
- **Windows:** `explorer.exe /select,"<absolute_path>"`
- **Linux:** `xdg-open "<parent_directory>"` (xdg-open cannot select a
  specific file, so open the containing directory)

Spawn the process detached (fire-and-forget, no stdout capture needed).

## Testing

- **Palette:** unit tests for fuzzy subsequence matching (match/no-match,
  case insensitivity, ordering). Integration test: open palette, type query,
  verify filtered results contain expected symbol.
- **History:** unit tests for push/back/forward/truncation/cap-at-64
  behavior.
- **Open file location:** unit test for path resolution from qualified_path;
  the actual `Command::new` spawn is not tested (platform-dependent).
- Manual visual gate for all four features after implementation.
