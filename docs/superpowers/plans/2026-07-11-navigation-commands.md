# Navigation Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add fuzzy file search (Ctrl+P), fuzzy symbol search (Ctrl+T), back/forward navigation history (Alt+Left/Right), and open-in-file-manager (Ctrl+Shift+E) to the treemap viewer.

**Architecture:** Navigation history lives on `TreemapView` as a `Vec<SymbolId>` + cursor, pushed on focus jumps. A new `palette.rs` module owns the search overlay state and fuzzy matching logic. The palette renders as a `div().absolute()` overlay in the `Render` impl. File-manager opening resolves the focused node's path and spawns a detached process.

**Tech Stack:** Rust, GPUI (div/canvas rendering, key events, focus handles), outrider-index types (`SymbolId`, `SymbolKind`, `SymbolNode`, `SymbolTree`)

## Global Constraints

- No external crate dependencies for fuzzy matching (subsequence only).
- Max 12 visible palette rows.
- Nav history capped at 64 entries; oldest dropped on overflow.
- Arrow-key sibling steps do NOT record history.
- Platform-specific file-manager: `explorer.exe /select,"<path>"` on Windows, `xdg-open "<dir>"` on Linux.

---

### Task 1: Navigation History

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (add fields, push/navigate logic, key handler arms)

**Interfaces:**
- Consumes: `Focus::step_in`, `Focus::step_out`, `Focus::set` (existing)
- Produces: `TreemapView::nav_push(&mut self)`, `TreemapView::nav_back(&mut self, ...) -> Option<Camera>`, `TreemapView::nav_forward(&mut self, ...) -> Option<Camera>` (internal to treemap, consumed by key handler)

- [ ] **Step 1: Write failing tests for nav history push/back/forward/truncation/cap**

Add to the `#[cfg(test)] mod tests` section of `treemap.rs`:

```rust
#[test]
fn nav_history_push_and_back() {
    let ids: Vec<SymbolId> = (0..4)
        .map(|i| SymbolId {
            kind: SymbolKind::File,
            qualified_path: format!("f{i}.rs"),
            ordinal: 0,
        })
        .collect();
    let mut hist = vec![ids[0].clone()];
    let mut cursor: usize = 0;

    // push 3 more
    for id in &ids[1..] {
        nav_push_to(&mut hist, &mut cursor, id.clone());
    }
    assert_eq!(hist.len(), 4);
    assert_eq!(cursor, 3);

    // back twice
    cursor = nav_back_cursor(&hist, cursor).unwrap();
    assert_eq!(cursor, 2);
    assert_eq!(hist[cursor], ids[2]);
    cursor = nav_back_cursor(&hist, cursor).unwrap();
    assert_eq!(cursor, 1);

    // back at beginning is None
    cursor = nav_back_cursor(&hist, cursor).unwrap();
    assert_eq!(cursor, 0);
    assert!(nav_back_cursor(&hist, cursor).is_none());
}

#[test]
fn nav_history_forward_after_back() {
    let ids: Vec<SymbolId> = (0..3)
        .map(|i| SymbolId {
            kind: SymbolKind::File,
            qualified_path: format!("f{i}.rs"),
            ordinal: 0,
        })
        .collect();
    let mut hist = vec![ids[0].clone()];
    let mut cursor: usize = 0;
    for id in &ids[1..] {
        nav_push_to(&mut hist, &mut cursor, id.clone());
    }
    // back to f1
    cursor = nav_back_cursor(&hist, cursor).unwrap();
    assert_eq!(hist[cursor], ids[1]);
    // forward to f2
    cursor = nav_forward_cursor(&hist, cursor).unwrap();
    assert_eq!(cursor, 2);
    assert_eq!(hist[cursor], ids[2]);
    // forward at end is None
    assert!(nav_forward_cursor(&hist, cursor).is_none());
}

#[test]
fn nav_history_push_truncates_forward() {
    let ids: Vec<SymbolId> = (0..4)
        .map(|i| SymbolId {
            kind: SymbolKind::File,
            qualified_path: format!("f{i}.rs"),
            ordinal: 0,
        })
        .collect();
    let mut hist = vec![ids[0].clone()];
    let mut cursor: usize = 0;
    for id in &ids[1..3] {
        nav_push_to(&mut hist, &mut cursor, id.clone());
    }
    // back to f0
    cursor = nav_back_cursor(&hist, cursor).unwrap();
    cursor = nav_back_cursor(&hist, cursor).unwrap();
    // push f3 — truncates f1, f2
    nav_push_to(&mut hist, &mut cursor, ids[3].clone());
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0], ids[0]);
    assert_eq!(hist[1], ids[3]);
    assert_eq!(cursor, 1);
}

#[test]
fn nav_history_caps_at_64() {
    let mut hist = vec![SymbolId {
        kind: SymbolKind::File,
        qualified_path: "f0.rs".into(),
        ordinal: 0,
    }];
    let mut cursor: usize = 0;
    for i in 1..=70 {
        nav_push_to(
            &mut hist,
            &mut cursor,
            SymbolId {
                kind: SymbolKind::File,
                qualified_path: format!("f{i}.rs"),
                ordinal: 0,
            },
        );
    }
    assert_eq!(hist.len(), 64);
    // cursor points to the most recent entry
    assert_eq!(cursor, 63);
    // oldest was dropped — first entry is no longer f0
    assert_ne!(hist[0].qualified_path, "f0.rs");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider nav_history 2>&1 | tail -20`
Expected: compilation error — `nav_push_to`, `nav_back_cursor`, `nav_forward_cursor` not defined.

- [ ] **Step 3: Implement nav history helpers and wire into TreemapView**

Add these free functions above the `impl TreemapView` block (around line 396):

```rust
const NAV_HISTORY_CAP: usize = 64;

fn nav_push_to(hist: &mut Vec<SymbolId>, cursor: &mut usize, id: SymbolId) {
    hist.truncate(*cursor + 1);
    hist.push(id);
    *cursor = hist.len() - 1;
    if hist.len() > NAV_HISTORY_CAP {
        let excess = hist.len() - NAV_HISTORY_CAP;
        hist.drain(..excess);
        *cursor -= excess;
    }
}

fn nav_back_cursor(hist: &[SymbolId], cursor: usize) -> Option<usize> {
    if cursor == 0 { None } else { Some(cursor - 1) }
}

fn nav_forward_cursor(hist: &[SymbolId], cursor: usize) -> Option<usize> {
    if cursor + 1 >= hist.len() { None } else { Some(cursor + 1) }
}
```

Add fields to `TreemapView` struct:

```rust
nav_history: Vec<SymbolId>,
nav_cursor: usize,
```

Initialize in `TreemapView::new` (after `focus: Focus::new(root_id)`):

```rust
nav_history: vec![root_id.clone()],
nav_cursor: 0,
```

(Note: `root_id` is already bound via `let root_id = tree.root.id.clone();` at line 422.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider nav_history 2>&1 | tail -10`
Expected: 4 tests pass.

- [ ] **Step 5: Wire nav_push into existing focus-change call sites in the key handler**

In the `on_key_down` handler, after each successful focus move (Enter, Escape), and in `on_mouse_up` after a click that changes focus, call:

```rust
nav_push_to(&mut this.nav_history, &mut this.nav_cursor, this.focus.current.clone());
```

Specifically:
- After `this.focus.step_in(&index)` succeeds (line ~887)
- After `this.focus.step_out(&index)` succeeds (line ~893)
- After `this.focus.set(id, &index)` succeeds in the click handler (line ~821)

Do NOT add it to arrow-key steps (they are local exploration).

- [ ] **Step 6: Add Alt+Left / Alt+Right key handler arms**

Add before the existing `"up" | "down" | "left" | "right"` arm in the key handler:

```rust
"left" if e.keystroke.modifiers.alt => {
    let Some(c) = nav_back_cursor(&this.nav_history, this.nav_cursor) else {
        return;
    };
    this.nav_cursor = c;
    let id = this.nav_history[c].clone();
    this.focus.current = id;
    this.focus.record_visit(&index);
    this.neighbors = None;
    this.frame_focus(&index, vw, vh, min_zoom, max_zoom)
}
"right" if e.keystroke.modifiers.alt => {
    let Some(c) = nav_forward_cursor(&this.nav_history, this.nav_cursor) else {
        return;
    };
    this.nav_cursor = c;
    let id = this.nav_history[c].clone();
    this.focus.current = id;
    this.focus.record_visit(&index);
    this.neighbors = None;
    this.frame_focus(&index, vw, vh, min_zoom, max_zoom)
}
```

Note: These arms must appear BEFORE the unmodified `"left" | "right"` arm so they match first. The `record_visit` method is currently private — make it `pub(crate)` so treemap can call it.

- [ ] **Step 7: Make `Focus::record_visit` pub(crate)**

In `crates/outrider/src/focus.rs`, change:

```rust
fn record_visit(&mut self, index: &TreeIndex) {
```

to:

```rust
pub(crate) fn record_visit(&mut self, index: &TreeIndex) {
```

- [ ] **Step 8: Run full test suite**

Run: `cargo test -p outrider 2>&1 | tail -5`
Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/outrider/src/treemap.rs crates/outrider/src/focus.rs
git commit -m "feat: add navigation history with Alt+Left/Right back/forward"
```

---

### Task 2: Fuzzy Matching and Palette State

**Files:**
- Create: `crates/outrider/src/palette.rs`
- Modify: `crates/outrider/src/main.rs` (add `mod palette;`)

**Interfaces:**
- Consumes: `SymbolId`, `SymbolKind`, `SymbolNode`, `SymbolTree` (from outrider-index)
- Produces:
  - `pub fn fuzzy_match(query: &str, name: &str) -> bool` — case-insensitive subsequence match
  - `pub enum PaletteMode { File, Symbol }`
  - `pub struct Palette { open, mode, query, results, selection }` with methods:
    - `pub fn open(&mut self, mode: PaletteMode, tree: &SymbolTree)`
    - `pub fn close(&mut self)`
    - `pub fn is_open(&self) -> bool`
    - `pub fn type_char(&mut self, ch: char, tree: &SymbolTree)`
    - `pub fn backspace(&mut self, tree: &SymbolTree)`
    - `pub fn move_selection(&mut self, delta: i32)`
    - `pub fn confirm(&self) -> Option<SymbolId>`

- [ ] **Step 1: Write failing tests for fuzzy matching**

Create `crates/outrider/src/palette.rs` with only:

```rust
//! Search palette: fuzzy file/symbol search with filtered results list.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_match_exact() {
        assert!(fuzzy_match("parse", "parse"));
    }

    #[test]
    fn fuzzy_match_subsequence() {
        assert!(fuzzy_match("prs", "parse"));
        assert!(fuzzy_match("fmn", "file_manager_new"));
    }

    #[test]
    fn fuzzy_match_case_insensitive() {
        assert!(fuzzy_match("PRS", "parse"));
        assert!(fuzzy_match("prs", "PARSE"));
    }

    #[test]
    fn fuzzy_match_no_match() {
        assert!(!fuzzy_match("xyz", "parse"));
        assert!(!fuzzy_match("srp", "parse")); // wrong order
    }

    #[test]
    fn fuzzy_match_empty_query_matches_all() {
        assert!(fuzzy_match("", "anything"));
    }
}
```

- [ ] **Step 2: Add `mod palette;` to main.rs and run tests to verify they fail**

In `crates/outrider/src/main.rs`, add `mod palette;` after `mod theme;`.

Run: `cargo test -p outrider fuzzy_match 2>&1 | tail -10`
Expected: compilation error — `fuzzy_match` not defined.

- [ ] **Step 3: Implement `fuzzy_match`**

Add above the test module in `palette.rs`:

```rust
pub fn fuzzy_match(query: &str, name: &str) -> bool {
    let mut name_chars = name.chars().flat_map(|c| c.to_lowercase());
    for qc in query.chars().flat_map(|c| c.to_lowercase()) {
        loop {
            match name_chars.next() {
                Some(nc) if nc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}
```

- [ ] **Step 4: Run fuzzy_match tests to verify they pass**

Run: `cargo test -p outrider fuzzy_match 2>&1 | tail -10`
Expected: 5 tests pass.

- [ ] **Step 5: Write failing tests for Palette state machine**

Add to the test module:

```rust
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

fn test_tree() -> SymbolTree {
    fn node(kind: SymbolKind, qp: &str, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
            name: name.into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }
    SymbolTree {
        root: node(
            SymbolKind::Folder,
            "",
            "",
            vec![
                node(SymbolKind::File, "parse.rs", "parse.rs", vec![
                    node(SymbolKind::Item { label: "fn".into() }, "parse.rs::parse_item", "parse_item", vec![]),
                    node(SymbolKind::Item { label: "fn".into() }, "parse.rs::tokenize", "tokenize", vec![]),
                ]),
                node(SymbolKind::File, "main.rs", "main.rs", vec![
                    node(SymbolKind::Item { label: "fn".into() }, "main.rs::main", "main", vec![]),
                ]),
                node(SymbolKind::Folder, "utils", "utils", vec![
                    node(SymbolKind::File, "utils/helpers.rs", "helpers.rs", vec![]),
                ]),
            ],
        ),
        repo_root: std::path::PathBuf::from("/tmp"),
    }
}

#[test]
fn palette_file_mode_lists_files() {
    let tree = test_tree();
    let mut p = Palette::new();
    p.open(PaletteMode::File, &tree);
    assert!(p.is_open());
    // 3 files: parse.rs, main.rs, helpers.rs
    assert_eq!(p.results.len(), 3);
}

#[test]
fn palette_symbol_mode_lists_non_folders() {
    let tree = test_tree();
    let mut p = Palette::new();
    p.open(PaletteMode::Symbol, &tree);
    // all non-Folder: 3 files + 3 items = 6
    assert_eq!(p.results.len(), 6);
}

#[test]
fn palette_filters_on_type() {
    let tree = test_tree();
    let mut p = Palette::new();
    p.open(PaletteMode::Symbol, &tree);
    p.type_char('t', &tree);
    p.type_char('k', &tree);
    // "tk" matches "tokenize" only
    assert_eq!(p.results.len(), 1);
    assert_eq!(p.results[0].qualified_path, "parse.rs::tokenize");
}

#[test]
fn palette_backspace_widens_results() {
    let tree = test_tree();
    let mut p = Palette::new();
    p.open(PaletteMode::Symbol, &tree);
    p.type_char('t', &tree);
    p.type_char('k', &tree);
    assert_eq!(p.results.len(), 1);
    p.backspace(&tree);
    // "t" matches tokenize, parse_item (has 't'), helpers.rs (has no 't'?)
    // Actually "t" matches: tokenize, parse_item (no 't'... wait 'i' 't')
    // parse_item has 't' at position 8 — yes matches
    // main.rs has no... wait 'main.rs' no 't'... hmm
    // Let's just check it's more than 1
    assert!(p.results.len() > 1);
}

#[test]
fn palette_selection_wraps() {
    let tree = test_tree();
    let mut p = Palette::new();
    p.open(PaletteMode::File, &tree);
    assert_eq!(p.selection, 0);
    p.move_selection(1);
    assert_eq!(p.selection, 1);
    p.move_selection(-1);
    assert_eq!(p.selection, 0);
    // wraps at bottom
    p.move_selection(-1);
    assert_eq!(p.selection, p.results.len() - 1);
}

#[test]
fn palette_confirm_returns_selected() {
    let tree = test_tree();
    let mut p = Palette::new();
    p.open(PaletteMode::File, &tree);
    p.move_selection(1);
    let id = p.confirm().unwrap();
    assert_eq!(id, p.results[1]);
}

#[test]
fn palette_close_clears_state() {
    let tree = test_tree();
    let mut p = Palette::new();
    p.open(PaletteMode::File, &tree);
    p.close();
    assert!(!p.is_open());
    assert!(p.confirm().is_none());
}

#[test]
fn palette_caps_results_at_12() {
    // Build a tree with 20 files
    fn node(kind: SymbolKind, qp: &str, name: &str) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
            name: name.into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        }
    }
    let files: Vec<SymbolNode> = (0..20)
        .map(|i| node(SymbolKind::File, &format!("f{i}.rs"), &format!("f{i}.rs")))
        .collect();
    let tree = SymbolTree {
        root: SymbolNode {
            id: SymbolId { kind: SymbolKind::Folder, qualified_path: "".into(), ordinal: 0 },
            name: "".into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children: files,
        },
        repo_root: std::path::PathBuf::from("/tmp"),
    };
    let mut p = Palette::new();
    p.open(PaletteMode::File, &tree);
    assert_eq!(p.results.len(), 12);
}
```

- [ ] **Step 6: Run tests to verify they fail**

Run: `cargo test -p outrider palette 2>&1 | tail -10`
Expected: compilation error — `Palette`, `PaletteMode` not defined.

- [ ] **Step 7: Implement Palette struct and methods**

Add to `palette.rs` above the test module:

```rust
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

const MAX_RESULTS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    File,
    Symbol,
}

pub struct Palette {
    open: bool,
    pub mode: PaletteMode,
    pub query: String,
    pub results: Vec<SymbolId>,
    pub selection: usize,
    candidates: Vec<(SymbolId, String)>,
}

impl Palette {
    pub fn new() -> Self {
        Self {
            open: false,
            mode: PaletteMode::File,
            query: String::new(),
            results: Vec::new(),
            selection: 0,
            candidates: Vec::new(),
        }
    }

    pub fn open(&mut self, mode: PaletteMode, tree: &SymbolTree) {
        self.open = true;
        self.mode = mode;
        self.query.clear();
        self.selection = 0;
        self.candidates.clear();
        Self::collect_candidates(&tree.root, mode, &mut self.candidates);
        self.candidates.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| a.1.cmp(&b.1)));
        self.refilter();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.results.clear();
        self.candidates.clear();
        self.selection = 0;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn type_char(&mut self, ch: char, _tree: &SymbolTree) {
        self.query.push(ch);
        self.refilter();
    }

    pub fn backspace(&mut self, _tree: &SymbolTree) {
        self.query.pop();
        self.refilter();
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.results.is_empty() {
            return;
        }
        let len = self.results.len() as i32;
        self.selection = ((self.selection as i32 + delta).rem_euclid(len)) as usize;
    }

    pub fn confirm(&self) -> Option<SymbolId> {
        if !self.open {
            return None;
        }
        self.results.get(self.selection).cloned()
    }

    fn refilter(&mut self) {
        self.results = self
            .candidates
            .iter()
            .filter(|(_, name)| fuzzy_match(&self.query, name))
            .take(MAX_RESULTS)
            .map(|(id, _)| id.clone())
            .collect();
        if self.selection >= self.results.len() {
            self.selection = self.results.len().saturating_sub(1);
        }
    }

    fn collect_candidates(node: &SymbolNode, mode: PaletteMode, out: &mut Vec<(SymbolId, String)>) {
        let dominated = match mode {
            PaletteMode::File => node.id.kind == SymbolKind::File,
            PaletteMode::Symbol => node.id.kind != SymbolKind::Folder,
        };
        if dominated {
            out.push((node.id.clone(), node.name.clone()));
        }
        for c in &node.children {
            Self::collect_candidates(c, mode, out);
        }
    }
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p outrider palette 2>&1 | tail -10`
Expected: all palette tests pass.

- [ ] **Step 9: Run full suite**

Run: `cargo test -p outrider 2>&1 | tail -5`
Expected: all tests pass.

- [ ] **Step 10: Commit**

```bash
git add crates/outrider/src/palette.rs crates/outrider/src/main.rs
git commit -m "feat: add palette module with fuzzy matching and search state"
```

---

### Task 3: Palette Rendering and Keyboard Integration

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (add palette field, render overlay, intercept keys)

**Interfaces:**
- Consumes: `Palette`, `PaletteMode`, `fuzzy_match` (from Task 2); `nav_push_to` (from Task 1); `TreeIndex`, `Focus`, `frame_focus` (existing)
- Produces: Palette overlay visible in the UI; Ctrl+P / Ctrl+T open it; typing filters; Enter confirms and navigates; Esc closes.

- [ ] **Step 1: Add `palette: Palette` field to TreemapView**

In `TreemapView` struct, add:

```rust
palette: palette::Palette,
```

Initialize in `TreemapView::new`:

```rust
palette: palette::Palette::new(),
```

- [ ] **Step 2: Add Ctrl+P / Ctrl+T key handling that opens the palette**

In the `on_key_down` listener, add at the TOP (before the camera-none early return):

```rust
if e.keystroke.modifiers.control && !e.keystroke.modifiers.shift {
    match e.keystroke.key.as_str() {
        "p" => {
            this.palette.open(palette::PaletteMode::File, &this.tree);
            cx.notify();
            return;
        }
        "t" => {
            this.palette.open(palette::PaletteMode::Symbol, &this.tree);
            cx.notify();
            return;
        }
        _ => {}
    }
}
```

- [ ] **Step 3: Suppress map key handling while palette is open; route keys to palette**

Right after the Ctrl+P/T block, add:

```rust
if this.palette.is_open() {
    match e.keystroke.key.as_str() {
        "escape" => {
            this.palette.close();
            cx.notify();
        }
        "enter" => {
            if let Some(id) = this.palette.confirm() {
                this.palette.close();
                let index = TreeIndex::new(&this.tree);
                if this.focus.set(id, &index) {
                    nav_push_to(
                        &mut this.nav_history,
                        &mut this.nav_cursor,
                        this.focus.current.clone(),
                    );
                }
                let (vw, vh) = Self::map_viewport(w);
                let max_zoom = camera::MAX_ZOOM;
                let min_zoom = (this.home_zoom * 0.5).min(camera::MAX_ZOOM);
                if let Some(to) =
                    this.frame_focus(&index, vw, vh, min_zoom, max_zoom)
                {
                    this.start_tween(to);
                }
            }
            cx.notify();
        }
        "up" => {
            this.palette.move_selection(-1);
            cx.notify();
        }
        "down" => {
            this.palette.move_selection(1);
            cx.notify();
        }
        "backspace" => {
            this.palette.backspace(&this.tree);
            cx.notify();
        }
        _ => {
            // single printable character → type into palette
            if let Some(ch) = e.keystroke.ime_key.as_ref().and_then(|s| {
                let mut chars = s.chars();
                let c = chars.next()?;
                if chars.next().is_none() { Some(c) } else { None }
            }) {
                this.palette.type_char(ch, &this.tree);
                cx.notify();
            }
        }
    }
    return;
}
```

- [ ] **Step 4: Render the palette overlay in the `Render` impl**

In the `render` method, after building the `map` div and before the final `v_flex()` return, conditionally add the palette overlay. The palette is rendered as a sibling of the canvas inside the map div (using `.absolute()` positioning). Add it as a `.child(...)` of the `map` div (after the `.child(canvas(...))` call):

```rust
.children(if this_palette_open {
    Some(self.render_palette())
} else {
    None
})
```

Where `this_palette_open` is `self.palette.is_open()` captured before the closure.

Add a helper method:

```rust
fn render_palette(&self) -> gpui::Div {
    use gpui::{div, rgb, px, IntoElement};
    let w = 500.0;
    let mode_label = match self.palette.mode {
        palette::PaletteMode::File => "File",
        palette::PaletteMode::Symbol => "Symbol",
    };
    let index = TreeIndex::new(&self.tree);

    div()
        .absolute()
        .top(px(60.0))
        .left_auto()
        .right_auto()
        .ml(px((self.map_viewport_cached_w - w) as f32 / 2.0))
        .w(px(w as f32))
        .bg(rgb(theme::CODE_BG))
        .border_1()
        .border_color(rgb(theme::FOCUS_BORDER))
        .rounded(px(4.0))
        .overflow_hidden()
        .child(
            // Query input row
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(px(14.0))
                .font_family(theme::FONT_FAMILY)
                .text_color(rgb(theme::TEXT_PRIMARY))
                .child(format!("[{mode_label}] {}{}", self.palette.query, "│"))
        )
        .children(
            self.palette.results.iter().enumerate().map(|(i, id)| {
                let node = index.node(id);
                let name = node.map(|n| n.name.as_str()).unwrap_or("?");
                let path = &id.qualified_path;
                let selected = i == self.palette.selection;
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .text_size(px(13.0))
                    .font_family(theme::FONT_FAMILY)
                    .text_color(if selected {
                        rgb(theme::TEXT_PRIMARY)
                    } else {
                        rgb(theme::TEXT_DIM)
                    })
                    .when(selected, |d| d.bg(rgb(0x2a2d32)))
                    .child(format!("{name}  {path}"))
            })
        )
}
```

Note: The exact GPUI API usage for `left_auto()` / centering may need adjustment — use `ml(px(...))` with half the remaining viewport width for centering. Store the map viewport width in a field or compute from `window` at render time. The implementer should check how `self` is accessible in `render` to make this work cleanly — the key point is the overlay is a `div().absolute()` inside the map container.

- [ ] **Step 5: Run `cargo test -p outrider` to ensure compilation and tests pass**

Run: `cargo test -p outrider 2>&1 | tail -5`
Expected: all tests pass (no new unit tests in this task — behavior is visual).

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: render search palette with Ctrl+P/T, keyboard nav, and fuzzy filter"
```

---

### Task 4: Open File Location (Ctrl+Shift+E)

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (add key handler arm, path resolution helper)

**Interfaces:**
- Consumes: `BufferManager::file_path_of` (existing), `SymbolTree::repo_root` (existing), `Focus::current` (existing), `SymbolKind` (existing), `TreeIndex` (existing)
- Produces: `fn resolve_fs_path(id: &SymbolId, repo_root: &Path) -> PathBuf` (internal helper)

- [ ] **Step 1: Write failing test for path resolution**

Add to the test module in `treemap.rs`:

```rust
#[test]
fn resolve_fs_path_file_node() {
    let root = std::path::Path::new("/home/user/project");
    let id = SymbolId {
        kind: SymbolKind::File,
        qualified_path: "src/main.rs".into(),
        ordinal: 0,
    };
    let path = resolve_fs_path(&id, root);
    assert_eq!(path, std::path::PathBuf::from("/home/user/project/src/main.rs"));
}

#[test]
fn resolve_fs_path_item_node() {
    let root = std::path::Path::new("/home/user/project");
    let id = SymbolId {
        kind: SymbolKind::Item { label: "fn".into() },
        qualified_path: "src/lib.rs::Point::norm".into(),
        ordinal: 0,
    };
    let path = resolve_fs_path(&id, root);
    assert_eq!(path, std::path::PathBuf::from("/home/user/project/src/lib.rs"));
}

#[test]
fn resolve_fs_path_chunk_node() {
    let root = std::path::Path::new("/repo");
    let id = SymbolId {
        kind: SymbolKind::Chunk,
        qualified_path: "BIG.md#2".into(),
        ordinal: 0,
    };
    let path = resolve_fs_path(&id, root);
    assert_eq!(path, std::path::PathBuf::from("/repo/BIG.md"));
}

#[test]
fn resolve_fs_path_folder_node() {
    let root = std::path::Path::new("/repo");
    let id = SymbolId {
        kind: SymbolKind::Folder,
        qualified_path: "src/utils".into(),
        ordinal: 0,
    };
    let path = resolve_fs_path(&id, root);
    assert_eq!(path, std::path::PathBuf::from("/repo/src/utils"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider resolve_fs_path 2>&1 | tail -10`
Expected: compilation error — `resolve_fs_path` not defined.

- [ ] **Step 3: Implement `resolve_fs_path`**

Add as a free function in `treemap.rs`:

```rust
fn resolve_fs_path(id: &SymbolId, repo_root: &std::path::Path) -> std::path::PathBuf {
    let rel = match id.kind {
        SymbolKind::Folder => id.qualified_path.as_str(),
        _ => crate::buffers::BufferManager::file_path_of(&id.qualified_path),
    };
    repo_root.join(rel)
}
```

Note: `BufferManager::file_path_of` is already `pub` and strips `::` and `#` suffixes. For Folder nodes the `qualified_path` is already the relative dir path.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider resolve_fs_path 2>&1 | tail -10`
Expected: 4 tests pass.

- [ ] **Step 5: Add Ctrl+Shift+E key handler arm**

In the Ctrl modifier block added in Task 3 (or add a new block for `ctrl + shift`), add:

```rust
if e.keystroke.modifiers.control && e.keystroke.modifiers.shift {
    if e.keystroke.key.as_str() == "e" {
        let path = resolve_fs_path(&this.focus.current, &this.tree.repo_root);
        open_in_file_manager(&path);
        return;
    }
}
```

Implement the platform helper:

```rust
fn open_in_file_manager(path: &std::path::Path) {
    use std::process::Command;

    if cfg!(target_os = "windows") {
        let arg = if path.is_dir() {
            format!("{}", path.display())
        } else {
            format!("/select,\"{}\"", path.display())
        };
        let _ = Command::new("explorer.exe").raw_arg(&arg).spawn();
    } else {
        let dir = if path.is_dir() { path.to_path_buf() } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        let _ = Command::new("xdg-open").arg(&dir).spawn();
    }
}
```

Note: On Windows, `raw_arg` passes the argument without additional quoting. If the GPUI/Rust version doesn't have `raw_arg`, use `.arg(arg)` instead. The implementer should check API availability.

- [ ] **Step 6: Run full test suite**

Run: `cargo test -p outrider 2>&1 | tail -5`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: add Ctrl+Shift+E to open focused file in system file manager"
```

---

## Summary

| Task | Feature | Tests |
|------|---------|-------|
| 1 | Navigation history (Alt+Left/Right) | 4 unit tests for push/back/forward/truncate/cap |
| 2 | Fuzzy matching + palette state | 5 fuzzy tests + 8 palette state tests |
| 3 | Palette rendering + keyboard integration | Visual (no new unit tests) |
| 4 | Open file location (Ctrl+Shift+E) | 4 path resolution tests |
