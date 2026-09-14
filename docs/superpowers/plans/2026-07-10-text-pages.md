# Text Pages & Leaf Backgrounds Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Childless file leaves (markdown, TOML, plain text, unparsed `.rs`) render their full text at the Full rung — syntax highlighted where a grammar exists — and every leaf box keeps the editor-black background at every zoom level.

**Architecture:** One predicate change (`is_leaf_item` admits childless `File` nodes) flows through the existing rung/framing/scale machinery untouched. Supporting changes: `FileBuffer` grows md/toml/plain modes, `collect_file_symbols` anchors childless files at byte 0 so the text window starts at line 0, `body_lines` gives childless files a single readout row (keeping the `natural_px = header + (1+measure)·rows` math exact), and the paint fill keys on the predicate instead of the Full rung.

**Tech Stack:** Rust (edition 2021), tree-sitter 0.26.10, tree-sitter-md 0.5.3, tree-sitter-toml-ng 0.7.0, ropey, GPUI (only in `treemap.rs`/`main.rs`).

**Spec:** `docs/superpowers/specs/2026-07-10-text-pages-design.md`

## Global Constraints

- Every cargo command needs the PATH prefix: `export PATH="$HOME/.cargo/bin:$PATH" && `.
- Gate for every commit: `cargo test --workspace` green AND `cargo clippy --workspace --all-targets -- -D warnings` clean.
- New dependencies (exact, verified against tree-sitter 0.26.10 ABI): `tree-sitter-md = "0.5.3"`, `tree-sitter-toml-ng = "0.7.0"`. No other new dependencies.
- GPUI types must not leak outside `crates/outrider/src/treemap.rs` and `main.rs`.
- Markdown uses the **block** grammar only (`tree_sitter_md::LANGUAGE` + `HIGHLIGHT_QUERY_BLOCK`); the inline grammar and injections are out of scope.
- TOML must NOT use the crate's shipped `HIGHLIGHTS_QUERY` (its `(pair (bare_key)) @property` captures the whole pair node and the outermost-first overlap resolution would drop the inner key/value spans). Use the embedded `TOML_HIGHLIGHTS` query defined in Task 1.
- Working branch: `text-pages` (already created off main).

---

### Task 1: Multi-grammar FileBuffer (rs / md / toml / plain)

**Files:**
- Modify: `crates/outrider-index/Cargo.toml` (dependencies section)
- Modify: `crates/outrider-index/src/buffer.rs` (struct `FileBuffer`, `FileBuffer::new`, `kind_for`, `highlight_lines`, tests)
- Modify: `crates/outrider/src/buffers.rs:52-53` (the one caller of `FileBuffer::new`)

**Interfaces:**
- Consumes: existing `FileBuffer` internals (`line_bounds`, `highlight_lines`, `AnchorList`), `tree_sitter::{Parser, Query}`.
- Produces: `pub fn FileBuffer::new(text: String, ext: &str) -> anyhow::Result<Self>` — `ext` is the bare lowercase extension without dot (`"rs"`, `"md"`, `"toml"`, anything else = plain mode). Task 3's tests and the paint path rely on plain/md/toml buffers materializing successfully.

**Context for the implementer:** `FileBuffer` (in crate `outrider-index`) wraps a ropey rope plus per-line syntax-highlight spans computed once at construction by running a tree-sitter query over the parsed file. Today it hardcodes the Rust grammar. This task makes the grammar depend on the file extension and adds a "plain" mode (no parse, no spans) for everything else. The `tree` field is kept only for a future incremental-reparse phase; it becomes `Option<Tree>` so plain mode can store `None`.

- [ ] **Step 1: Add the grammar dependencies**

In `crates/outrider-index/Cargo.toml`, extend `[dependencies]`:

```toml
tree-sitter-md = "0.5.3"
tree-sitter-toml-ng = "0.7.0"
```

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p outrider-index`
Expected: builds clean (new deps compile; nothing uses them yet).

- [ ] **Step 2: Write the failing tests**

In `crates/outrider-index/src/buffer.rs`, inside `mod tests`, first update the two existing constructor calls to the new signature (lines 239 and 270):

```rust
let buf = FileBuffer::new(SNIPPET.to_string(), "rs").unwrap();
```
```rust
let mut buf = FileBuffer::new(SNIPPET.to_string(), "rs").unwrap();
```

Then add these tests at the end of `mod tests`:

```rust
    #[test]
    fn plain_mode_has_lines_but_no_spans() {
        let text = "alpha beta\n\ngamma\n";
        let buf = FileBuffer::new(text.to_string(), "txt").unwrap();
        assert_eq!(buf.len_lines(), 3);
        let (t0, s0) = buf.line(0).unwrap();
        assert_eq!(t0, "alpha beta");
        assert!(s0.is_empty());
        let (t1, s1) = buf.line(1).unwrap();
        assert_eq!(t1, "");
        assert!(s1.is_empty());
        let (t2, s2) = buf.line(2).unwrap();
        assert_eq!(t2, "gamma");
        assert!(s2.is_empty());
        // anchors still work in plain mode
        let mut buf = FileBuffer::new(text.to_string(), "").unwrap();
        let a = buf.create_anchor(12);
        assert_eq!(buf.byte_to_line(buf.resolve_anchor(a)), 2);
    }

    const MD_SNIPPET: &str = "# Title\n\nplain text\n\n```\nlet x = 1;\n```\n";

    #[test]
    fn markdown_headings_and_fences_highlight() {
        let buf = FileBuffer::new(MD_SNIPPET.to_string(), "md").unwrap();
        assert_eq!(buf.len_lines(), 7);
        // heading content is Type ("text.title")
        let (t0, s0) = buf.line(0).unwrap();
        assert!(
            s0.iter().any(|s| s.kind == HighlightKind::Type && &t0[s.range.clone()] == "Title"),
            "no Type span over 'Title' in {s0:?}"
        );
        // plain paragraph line: no mapped spans
        let (_t2, s2) = buf.line(2).unwrap();
        assert!(s2.is_empty(), "paragraph should be unhighlighted: {s2:?}");
        // fenced block ("text.literal" spans the whole block): every fence
        // line carries a String span
        for i in 4..=6 {
            let (_t, s) = buf.line(i).unwrap();
            assert!(
                s.iter().any(|sp| sp.kind == HighlightKind::String),
                "no String span on fence line {i}: {s:?}"
            );
        }
    }

    const TOML_SNIPPET: &str = "# note\n[package]\nname = \"x\"\ncount = 3\n";

    #[test]
    fn toml_keys_and_values_highlight() {
        let buf = FileBuffer::new(TOML_SNIPPET.to_string(), "toml").unwrap();
        assert_eq!(buf.len_lines(), 4);
        let (t0, s0) = buf.line(0).unwrap();
        assert!(s0
            .iter()
            .any(|s| s.kind == HighlightKind::Comment && &t0[s.range.clone()] == "# note"));
        // table header key
        let (t1, s1) = buf.line(1).unwrap();
        assert!(s1
            .iter()
            .any(|s| s.kind == HighlightKind::Property && &t1[s.range.clone()] == "package"));
        // pair: key is Property, value is String — both present (the
        // embedded query must not let a whole-pair capture swallow them)
        let (t2, s2) = buf.line(2).unwrap();
        assert!(s2
            .iter()
            .any(|s| s.kind == HighlightKind::Property && &t2[s.range.clone()] == "name"));
        assert!(s2.iter().any(|s| s.kind == HighlightKind::String));
        let (_t3, s3) = buf.line(3).unwrap();
        assert!(s3.iter().any(|s| s.kind == HighlightKind::Number));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index buffer 2>&1 | tail -20`
Expected: COMPILE ERROR — `FileBuffer::new` takes 1 argument but 2 were supplied.

- [ ] **Step 4: Implement the multi-grammar constructor**

In `crates/outrider-index/src/buffer.rs`:

Replace the `tree` field of `FileBuffer` (currently `tree: Tree`):

```rust
pub struct FileBuffer {
    rope: Rope,
    /// Held for Phase 6 incremental re-parse; unused until then. `None`
    /// in plain mode (no grammar for the extension).
    #[allow(dead_code)]
    tree: Option<Tree>,
    /// Per-line spans, computed once at materialization (spec §3.2).
    lines: Vec<Vec<HighlightSpan>>,
    anchors: AnchorList,
}
```

Add the embedded TOML query as a module-level const (see Global Constraints for why the shipped query is unusable):

```rust
/// Leaf-only TOML captures. The crate's shipped HIGHLIGHTS_QUERY has a
/// `(pair (bare_key)) @property` pattern that captures the whole pair
/// node; outermost-first overlap resolution would keep that whole-line
/// span and drop the inner key/string/number spans.
const TOML_HIGHLIGHTS: &str = r#"
(bare_key) @property
(quoted_key) @string
(boolean) @constant
(comment) @comment
(string) @string
[(integer) (float)] @number
[(offset_date_time) (local_date_time) (local_date) (local_time)] @string.special
"#;
```

Replace `FileBuffer::new`:

```rust
    /// `ext` is the bare lowercase file extension (no dot). Known
    /// extensions parse and highlight; anything else is plain mode —
    /// no parse, every line's span list empty.
    pub fn new(text: String, ext: &str) -> anyhow::Result<Self> {
        let lang: Option<(tree_sitter::Language, &str)> = match ext {
            "rs" => Some((tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY)),
            "md" => Some((tree_sitter_md::LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_BLOCK)),
            "toml" => Some((tree_sitter_toml_ng::LANGUAGE.into(), TOML_HIGHLIGHTS)),
            _ => None,
        };
        let (tree, lines) = match lang {
            Some((language, query_src)) => {
                let mut parser = tree_sitter::Parser::new();
                parser.set_language(&language).context("loading tree-sitter grammar")?;
                let tree = parser.parse(&text, None).context("tree-sitter parse failed")?;
                let lines = highlight_lines(&text, &tree, &language, query_src)?;
                (Some(tree), lines)
            }
            None => (None, vec![Vec::new(); line_bounds(&text).len()]),
        };
        Ok(Self { rope: Rope::from(text), tree, lines, anchors: AnchorList::default() })
    }
```

Extend `kind_for` with full-name matches before the prefix map (the markdown block query uses two-part names where the *second* part decides the kind):

```rust
/// Capture-name → HighlightKind (spec §3.2). Full-name matches first
/// (markdown block captures), then the prefix map. Unmapped captures
/// (punctuation, operators, `none`, …) are skipped and paint as Default.
fn kind_for(capture: &str) -> Option<HighlightKind> {
    match capture {
        "text.title" => return Some(HighlightKind::Type),
        "text.literal" => return Some(HighlightKind::String),
        "text.uri" | "text.reference" => return Some(HighlightKind::Property),
        _ => {}
    }
    match capture.split('.').next().unwrap_or(capture) {
        "keyword" => Some(HighlightKind::Keyword),
        "function" => Some(HighlightKind::Function),
        "type" | "constructor" => Some(HighlightKind::Type),
        "string" | "escape" => Some(HighlightKind::String),
        "comment" => Some(HighlightKind::Comment),
        "constant" | "number" => Some(HighlightKind::Number),
        "property" => Some(HighlightKind::Property),
        _ => None,
    }
}
```

Generalize `highlight_lines` (only the signature and the `Query::new` line change):

```rust
fn highlight_lines(
    text: &str,
    tree: &Tree,
    language: &tree_sitter::Language,
    query_src: &str,
) -> anyhow::Result<Vec<Vec<HighlightSpan>>> {
    let bounds = line_bounds(text);
    let mut lines: Vec<Vec<HighlightSpan>> = vec![Vec::new(); bounds.len()];
    let query = Query::new(language, query_src).context("compiling highlight query")?;
    // … rest of the function body is unchanged …
```

Update the one caller in `crates/outrider/src/buffers.rs` (inside `BufferManager::get`, currently lines 52-53):

```rust
            let text = std::fs::read_to_string(self.repo_root.join(rel_path)).ok()?;
            let ext = std::path::Path::new(rel_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let mut buffer = FileBuffer::new(text, ext).ok()?;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace 2>&1 | grep -E "test result|FAILED"`
Expected: all suites PASS (the pre-existing rust-highlight and buffers tests plus the three new ones).

- [ ] **Step 6: Clippy gate**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/outrider-index/Cargo.toml crates/outrider-index/src/buffer.rs crates/outrider/src/buffers.rs Cargo.lock
git commit -m "feat(index): multi-grammar FileBuffer — rust, markdown, toml, plain fallback"
```

---

### Task 2: Leaf-page predicate + childless-file Full body

**Files:**
- Modify: `crates/outrider/src/content.rs:24-28` (`is_leaf_item`), `content.rs:144-163` (`body_lines` File arm), tests in the same file

**Interfaces:**
- Consumes: nothing new.
- Produces: `is_leaf_item(node) == true` for childless `File` nodes with `byte_range` — Task 4's fill and the existing paint/framing/rung paths key on this. `body_lines(childless_file, Rung::Full) == vec![BodyLine::Dim(churn_readout(node))]` — exactly one row, so the paint path's appended file text (rows 1..=measure) fills the `natural_px` box exactly.

**Context for the implementer:** `is_leaf_item` decides which boxes render code at the Full rung (and, after this feature, which boxes keep the editor background). It currently excludes all `File` nodes. Childless files (every non-`.rs` file, plus `.rs` files that parse to zero items) should now qualify. `body_lines` returns the non-code body rows for a box; for a Full leaf the paint path appends the buffer text after those rows, so a childless file must contribute exactly ONE row (the churn readout — its "signature-equivalent") to keep the box-height arithmetic `HEADER + (1 + measure)·LINE_STEP + BOTTOM_PAD` exact. Scanner facts you can rely on: `File` nodes always have `byte_range: Some(0..bytes)`; `Folder` nodes have `byte_range: None`.

- [ ] **Step 1: Update the predicate test and add the body test**

In `crates/outrider/src/content.rs` tests, replace the `leaf_item_predicate` test's File case (currently asserts files are never leaf items):

```rust
    #[test]
    fn leaf_item_predicate() {
        let mut f = node(SymbolKind::Fn, "a.rs::f", 3, 0.0, 0, None, None, vec![]);
        assert!(!is_leaf_item(&f)); // no byte_range
        f.byte_range = Some(0..10);
        assert!(is_leaf_item(&f));
        // childless file WITH bytes is a leaf page now
        let mut file = node(SymbolKind::File, "a.md", 3, 0.0, 0, None, None, vec![]);
        assert!(!is_leaf_item(&file)); // no byte_range
        file.byte_range = Some(0..10);
        assert!(is_leaf_item(&file));
        // file with children is a container, not a page
        let mut parent_file = node(
            SymbolKind::File,
            "a.rs",
            3,
            0.0,
            0,
            None,
            None,
            vec![node(SymbolKind::Fn, "a.rs::f", 1, 0.0, 0, None, None, vec![])],
        );
        parent_file.byte_range = Some(0..10);
        assert!(!is_leaf_item(&parent_file));
        // folders never qualify
        let mut folder = node(SymbolKind::Folder, "src", 3, 0.0, 0, None, None, vec![]);
        folder.byte_range = Some(0..10);
        assert!(!is_leaf_item(&folder));
        let parent = node(SymbolKind::Impl, "a.rs::I", 3, 0.0, 0, None, None,
            vec![node(SymbolKind::Fn, "a.rs::I::m", 1, 0.0, 0, None, None, vec![])]);
        assert!(!is_leaf_item(&parent)); // has children
    }
```

And add this test after `body_lines_follow_the_content_table`:

```rust
    #[test]
    fn childless_file_full_body_is_one_readout_row() {
        use BodyLine::{Dim, Plain};
        // even with a doc comment, Full is exactly one row: the paint
        // path appends the file text (which contains the doc) from row 1,
        // keeping natural_px = HEADER + (1+measure)·LINE_STEP + BOTTOM_PAD
        let f = node(
            SymbolKind::File,
            "README.md",
            12,
            0.2,
            5,
            None,
            Some("# Readme\nIntro."),
            vec![],
        );
        assert_eq!(body_lines(&f, Rung::Full), vec![Dim("12L · 5 commits · p20".into())]);
        // Detail is unchanged: readout + doc first line (no kinds — childless)
        assert_eq!(
            body_lines(&f, Rung::Detail),
            vec![Dim("12L · 5 commits · p20".into()), Plain("# Readme".into())]
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider content 2>&1 | tail -20`
Expected: FAIL — `leaf_item_predicate` (childless file with bytes returns false) and `childless_file_full_body_is_one_readout_row` (Full returns `[Plain("# Readme"), Plain("Intro."), Dim(inventory)]`).

- [ ] **Step 3: Implement**

In `crates/outrider/src/content.rs`, replace `is_leaf_item` (and its doc comment — it no longer excludes files):

```rust
/// A leaf page: has source bytes, no children, and is not a folder.
/// Items are code pages; childless files (markdown, TOML, plain text,
/// unparsed .rs) are text pages. These boxes render their content at
/// Full and keep the editor background at every rung.
pub fn is_leaf_item(node: &SymbolNode) -> bool {
    node.byte_range.is_some()
        && node.children.is_empty()
        && node.id.kind != SymbolKind::Folder
}
```

In `body_lines`, replace the `SymbolKind::File` Full branch (the `else` arm of `if rung == Rung::Detail`, currently lines 155-163):

```rust
                } else if node.children.is_empty() {
                    // Text page: one signature-equivalent row; the paint
                    // path appends the file text from row 1 (spec §3).
                    vec![BodyLine::Dim(churn_readout(node))]
                } else {
                    let mut out: Vec<BodyLine> = node
                        .doc
                        .as_deref()
                        .map(|d| d.lines().map(|l| BodyLine::Plain(l.to_string())).collect())
                        .unwrap_or_default();
                    out.push(BodyLine::Dim(inventory(node)));
                    out
                }
```

- [ ] **Step 4: Run the workspace suite**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace 2>&1 | grep -E "test result|FAILED"`
Expected: all PASS. Note: the existing `body_lines_follow_the_content_table` test's "file without docs" case (`n.rs` childless, Full → `[Dim("9L · 0 commits · p0")]`) already matches the new arm — inventory of a childless file degrades to the readout — so it keeps passing without edits. If anything else fails, STOP and report rather than adjusting unrelated tests.

- [ ] **Step 5: Clippy gate**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/content.rs
git commit -m "feat(app): childless files are leaf pages — predicate + single-readout Full body"
```

---

### Task 3: File-node anchors in collect_file_symbols

**Files:**
- Modify: `crates/outrider/src/buffers.rs:79-89` (the `walk` closure inside `collect_file_symbols`), tests in the same file

**Interfaces:**
- Consumes: `SymbolNode.byte_range` (`Some(0..bytes)` on File nodes, per the scanner).
- Produces: for a childless file, `collect_file_symbols(tree)[path]` contains exactly `[(file_id, 0)]`, so `Materialized::symbol_start_line(&file_id)` resolves to `Some(0)` and the paint path's text window starts at rope line 0. Files with children are unchanged (items only — the file's own id is never in the list).

**Context for the implementer:** The paint path renders a Full leaf's text by looking up the leaf's `SymbolId` in a per-file symbol list (built once from the tree) to create rope anchors at materialization; `symbol_start_line` then maps the anchor to the first rope line of the text window. Today the builder only records a file's *children*, so a childless file resolves to `None` and its text would silently never render. This task records the childless file itself at its byte start (0).

- [ ] **Step 1: Write the failing test**

In `crates/outrider/src/buffers.rs` `mod tests`, add after `collect_file_symbols_maps_items_by_file` (reuse its local `node` helper by copying it — the existing test defines it inside the test fn, so define this test's fixture the same way):

```rust
    #[test]
    fn collect_file_symbols_anchors_childless_files_at_zero() {
        fn node(kind: SymbolKind, qual: &str, byte_range: Option<std::ops::Range<usize>>, children: Vec<SymbolNode>) -> SymbolNode {
            SymbolNode {
                id: SymbolId { kind, qualified_path: qual.into(), ordinal: 0 },
                name: qual.rsplit("::").next().unwrap_or(qual).to_string(),
                byte_range,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                children,
            }
        }
        let tree = SymbolTree {
            root: node(
                SymbolKind::Folder,
                "",
                None,
                vec![
                    node(SymbolKind::File, "README.md", Some(0..120), vec![]),
                    node(
                        SymbolKind::File,
                        "a.rs",
                        Some(0..40),
                        vec![node(SymbolKind::Fn, "a.rs::f", Some(5..30), vec![])],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        };
        let map = collect_file_symbols(&tree);
        // childless file: its own id at byte 0
        let readme = map.get("README.md").unwrap();
        assert_eq!(readme.len(), 1);
        assert_eq!(readme[0].0.kind, SymbolKind::File);
        assert_eq!(readme[0].0.qualified_path, "README.md");
        assert_eq!(readme[0].1, 0);
        // file with children: items only, own id absent
        let a = map.get("a.rs").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].0.qualified_path, "a.rs::f");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider buffers::tests::collect_file_symbols_anchors 2>&1 | tail -10`
Expected: FAIL — `readme.len()` is 0.

- [ ] **Step 3: Implement**

In `collect_file_symbols`'s inner `walk` fn, replace the File branch:

```rust
    fn walk(node: &SymbolNode, out: &mut BTreeMap<String, Vec<(SymbolId, usize)>>) {
        if node.id.kind == SymbolKind::File {
            let mut v = Vec::new();
            if node.children.is_empty() {
                // Text page: anchor the file itself so its window starts
                // at rope line 0 (spec §4).
                if let Some(r) = &node.byte_range {
                    v.push((node.id.clone(), r.start));
                }
            } else {
                items(node, &mut v);
            }
            out.insert(node.id.qualified_path.clone(), v);
        } else {
            for c in &node.children {
                walk(c, out);
            }
        }
    }
```

Also update the doc comment on `collect_file_symbols` (currently "…of every item inside that file"):

```rust
/// rel file path → (id, byte_range.start) of every item inside that file
/// — or, for a childless file, the file node itself at byte 0. Built once
/// at view construction; `get` uses it to create anchors at
/// materialization.
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace 2>&1 | grep -E "test result|FAILED"`
Expected: all PASS.

- [ ] **Step 5: Clippy gate**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/buffers.rs
git commit -m "feat(app): anchor childless file nodes at byte 0 so text pages find line 0"
```

---

### Task 4: Leaf background at every rung

**Files:**
- Modify: `crates/outrider/src/theme.rs` (new `box_fill` fn + test)
- Modify: `crates/outrider/src/treemap.rs:280-283` (fill decision in `paint_items`)

**Interfaces:**
- Consumes: `content::is_leaf_item` (Task 2's widened predicate), `theme::{CODE_BG, depth_fill}`.
- Produces: `pub fn theme::box_fill(is_leaf_page: bool, level: u8) -> u32`.

**Context for the implementer:** Today the paint loop picks `CODE_BG` only when a leaf is at the Full rung (`is_code`), so zooming into a leaf pops its background from the depth-shaded gray to editor black at the rung switch. The fix keys the fill on the leaf predicate alone: leaf pages are black at every rung (Dot through Full), containers keep the depth ramp. The decision moves into `theme.rs` as a pure function so it's headlessly testable (`treemap.rs` is the GPUI shell and has no test harness).

- [ ] **Step 1: Write the failing test**

In `crates/outrider/src/theme.rs` `mod tests`, add:

```rust
    #[test]
    fn box_fill_leaf_pages_are_editor_black_at_every_depth() {
        assert_eq!(box_fill(true, 0), CODE_BG);
        assert_eq!(box_fill(true, 5), CODE_BG);
        assert_eq!(box_fill(false, 0), depth_fill(0));
        assert_eq!(box_fill(false, 5), depth_fill(5));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider theme 2>&1 | tail -10`
Expected: COMPILE ERROR — `box_fill` not found.

- [ ] **Step 3: Implement**

In `crates/outrider/src/theme.rs`, after `depth_fill`:

```rust
/// Box background: leaf pages (code or text) keep the editor background
/// at every rung — zooming in never changes a leaf's background —
/// containers use the depth ramp.
pub fn box_fill(is_leaf_page: bool, level: u8) -> u32 {
    if is_leaf_page {
        CODE_BG
    } else {
        depth_fill(level)
    }
}
```

In `crates/outrider/src/treemap.rs` `paint_items` (currently lines 280-283), replace:

```rust
            let is_code = item.rung == Rung::Full && content::is_leaf_item(item.node);
            let scale =
                if is_code { content::code_scale(item.node, item.full_h) } else { 1.0 };
            let fill = if is_code { theme::CODE_BG } else { theme::depth_fill(item.level) };
```

with:

```rust
            let is_leaf = content::is_leaf_item(item.node);
            let is_code = item.rung == Rung::Full && is_leaf;
            let scale =
                if is_code { content::code_scale(item.node, item.full_h) } else { 1.0 };
            let fill = theme::box_fill(is_leaf, item.level);
```

- [ ] **Step 4: Run the workspace suite**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace 2>&1 | grep -E "test result|FAILED"`
Expected: all PASS.

- [ ] **Step 5: Clippy gate**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/theme.rs crates/outrider/src/treemap.rs
git commit -m "feat(app): leaf pages keep the editor background at every rung"
```

---

## Manual Exit Gate (after all tasks + final review)

Not a task — the human runs `export PATH="$HOME/.cargo/bin:$PATH" && cargo run -p outrider -- .` and verifies:
- README.md and Cargo.toml render their text (highlighted) when zoomed to Full.
- A plain-text file (e.g. `.gitignore`) renders unhighlighted text.
- Leaf boxes are editor-black at every zoom level; zooming into code no longer pops the background.
