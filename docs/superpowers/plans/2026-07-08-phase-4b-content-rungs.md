# Phase 4b: Detail/Full Rungs + Rope Substrate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two new fidelity rungs — Detail (250–700 px: signatures/summaries) and Full (≥700 px: tree-sitter-highlighted code rendered from a rope buffer with anchors) — completing Bet #1's content half.

**Architecture:** The buffer substrate (`FileBuffer` = ropey Rope + tree-sitter Tree + per-line highlight spans + `AnchorList`) lives in `outrider-index`, which already owns tree-sitter. The app owns an ephemeral LRU `BufferManager`, a GPUI-free content-assembly module, and the paint code. Signatures and `//!` docs are index-derived `SymbolNode` metadata; code at Full always renders from the rope via anchors.

**Tech Stack:** Rust, ropey 1.6, tree-sitter 0.26.10 + tree-sitter-rust 0.24.2 (`HIGHLIGHTS_QUERY` run directly via `Query`/`QueryCursor` — no new highlighting dependency), GPUI (pinned).

**Spec:** `docs/superpowers/specs/2026-07-08-phase-4b-content-rungs-design.md`

## Global Constraints

- Every cargo command needs the PATH prefix: `export PATH="$HOME/.cargo/bin:$PATH" && cargo …`
- `cargo clippy --workspace -- -D warnings` must stay clean at every commit.
- GPUI stays pinned at rev `029bf2f284b4e59f20175d78443e630468f3a3e5` — never touch it.
- Only new runtime dependency: `ropey` (in `outrider-index`). `tempfile` may be added as a **dev**-dependency of `outrider`.
- New constants (exact values): `MAX_BUFFERS: usize = 64`, `DETAIL_PX: f64 = 250.0`, `FULL_PX: f64 = 700.0`, `CODE_MIN_W: f64 = 300.0`.
- `world.rs`, `camera.rs`, `focus.rs`, `buffers.rs`, `content.rs`, and everything in `outrider-index` stay GPUI-free. GPUI types appear only in `treemap.rs` and `main.rs`.
- The mini_repo fixture file `src/util.rs` must NOT change (its `measure = 3` is asserted absolutely). Only `src/lib.rs` gains `//!` lines.
- Single-buffer invariant scope: **code at Full renders from the rope, resolved through anchors — never raw `byte_range` offsets**. Signatures/docs/inventories are index-derived metadata.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/outrider-index/src/parse.rs` | modify | `RawItem.signature`, `item_signature`, `file_doc` |
| `crates/outrider-index/src/types.rs` | modify | `SymbolNode.signature` / `.doc` fields |
| `crates/outrider-index/src/index.rs` | modify | thread signature/doc into `SymbolNode`s; return `ParsedFile` per file |
| `crates/outrider-index/src/scan.rs` | modify | `ParsedFile` type; File nodes carry `doc` |
| `crates/outrider-index/src/buffer.rs` | create | `FileBuffer`, `HighlightSpan`/`HighlightKind`, `AnchorList` (GPUI-free) |
| `crates/outrider-index/src/lib.rs` | modify | `pub mod buffer;` |
| `crates/outrider-index/Cargo.toml` | modify | add `ropey = "1.6"` |
| `crates/outrider/src/world.rs` | modify | `Rung::{Detail, Full}`, new constants, `DrawItem.top` |
| `crates/outrider/src/buffers.rs` | create | `BufferManager` (LRU), `Materialized`, `collect_file_symbols` (GPUI-free) |
| `crates/outrider/src/content.rs` | create | `BodyLine`, `card_meta`, `churn_readout`, `kind_counts`, `inventory`, `body_lines` (GPUI-free) |
| `crates/outrider/src/theme.rs` | modify | `syntax_color` palette |
| `crates/outrider/src/treemap.rs` | modify | `BodyText`, `runs_from_spans`, `code_line`, `build_body`, paint wiring |
| `crates/outrider/src/main.rs` | modify | `mod buffers; mod content;` |
| `crates/outrider/Cargo.toml` | modify | `[dev-dependencies] tempfile` |
| `crates/outrider-index/tests/fixtures/mini_repo/src/lib.rs` | modify | add leading `//!` block |
| `crates/outrider-index/tests/index_test.rs` | modify | signature/doc assertions |
| ~9 test/helper files across the workspace | modify | mechanical `signature: None, doc: None,` in `SymbolNode` literals |

Task order: 1 (index metadata) → 2 (buffer substrate) → 3 (rungs) → 4 (BufferManager) → 5 (content) → 6 (paint). 2 and 3 are independent of each other; everything else follows this order.

---

### Task 1: Index metadata — `signature` and `doc`

**Files:**
- Modify: `crates/outrider-index/src/parse.rs`
- Modify: `crates/outrider-index/src/types.rs`
- Modify: `crates/outrider-index/src/index.rs`
- Modify: `crates/outrider-index/src/scan.rs`
- Modify: `crates/outrider-index/tests/fixtures/mini_repo/src/lib.rs`
- Modify: `crates/outrider-index/tests/index_test.rs`
- Mechanical fixes (add `signature: None, doc: None,` to `SymbolNode` struct literals): `crates/outrider-index/src/types.rs` (~lines 91, 132, 143), `crates/outrider-layout/src/arrange.rs` (~line 80), `crates/outrider-layout/src/measure.rs` (~line 61), `crates/outrider-layout/tests/common/mod.rs` (~line 92), `crates/outrider-layout/tests/cross_process.rs` (~line 16), `crates/outrider-layout/tests/props_continuity.rs` (~line 146), `crates/outrider/src/focus.rs` (~line 120), `crates/outrider/src/world.rs` (~line 244)

**Interfaces:**
- Consumes: existing `RawItem`, `SymbolNode`, `parse_rust_items`, `build_tree`.
- Produces: `SymbolNode { …, pub signature: Option<String>, pub doc: Option<String>, … }`; `parse::file_doc(source: &[u8]) -> Option<String>`; `RawItem.signature: String`; `scan::ParsedFile { pub items: Vec<SymbolNode>, pub doc: Option<String> }` (implements `Default`, `Clone`); `build_tree(repo_root, files, rs_children: &BTreeMap<PathBuf, ParsedFile>)`. Later tasks read `node.signature` and `node.doc` directly.

- [ ] **Step 1: Write the failing parse tests**

Append inside `mod tests` in `crates/outrider-index/src/parse.rs` (the `SRC` constant already exists there):

```rust
    #[test]
    fn signatures_cut_before_body_and_collapse_whitespace() {
        let items = parse_rust_items(SRC.as_bytes()).unwrap();
        assert_eq!(items[0].signature, "mod inner");
        assert_eq!(items[1].signature, "struct Point");
        assert_eq!(items[2].signature, "impl Point");
        assert_eq!(items[2].children[1].signature, "fn norm(&self) -> f64");
        assert_eq!(items[3].signature, "fn free()");
        // multi-line declarations collapse to one line; `;` terminators cut too
        let src = b"fn multi(\n    a: i32,\n    b: i32,\n) -> i32 { a + b }\nstruct Unit;\n";
        let items = parse_rust_items(src).unwrap();
        assert_eq!(items[0].signature, "fn multi( a: i32, b: i32, ) -> i32");
        assert_eq!(items[1].signature, "struct Unit");
    }

    #[test]
    fn file_doc_extracts_leading_bang_comments() {
        use super::file_doc;
        assert_eq!(
            file_doc(b"//! First line.\n//!\n//! Third.\nfn x() {}\n"),
            Some("First line.\n\nThird.".to_string())
        );
        assert_eq!(file_doc(b"\n\n//! After blanks.\nfn x() {}\n"), Some("After blanks.".to_string()));
        assert_eq!(file_doc(b"fn x() {}\n"), None);
        assert_eq!(file_doc(b"// plain comment\n//! not leading\n"), None);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index --lib parse`
Expected: FAIL to compile — no field `signature` on `RawItem`, no `file_doc`.

- [ ] **Step 3: Implement in parse.rs**

Add `pub signature: String,` to `RawItem` (after `name`). In `collect_items`, add `signature: item_signature(child, src),` to the `RawItem` literal. Add below `item_name`:

```rust
/// Declaration text up to (excluding) the body `{` or a terminating `;`,
/// whitespace collapsed to one line.
fn item_signature(node: Node, src: &[u8]) -> String {
    let text = node_text(node, src);
    let end = text.find(['{', ';']).unwrap_or(text.len());
    text[..end].split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Leading `//!` block: skip blank lines, collect consecutive `//!` lines,
/// strip the marker plus one following space. None when there is no block.
pub fn file_doc(source: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(source);
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if lines.is_empty() && t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("//!") {
            lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else {
            break;
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}
```

- [ ] **Step 4: Run to verify parse tests pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index --lib parse`
Expected: PASS (all parse tests, including the two new ones).

- [ ] **Step 5: Add the SymbolNode fields and fix every literal**

In `crates/outrider-index/src/types.rs`, add to `SymbolNode` after `byte_range`:

```rust
    /// Item declaration up to (excluding) the body `{`, whitespace
    /// collapsed to one line. None for folders and files.
    pub signature: Option<String>,
    /// Leading `//!` block, comment markers stripped. File nodes only.
    pub doc: Option<String>,
```

Then run `export PATH="$HOME/.cargo/bin:$PATH" && cargo build --workspace 2>&1 | grep "missing field"` and add `signature: None, doc: None,` (right after `byte_range: …,`) to every `SymbolNode { … }` literal the compiler flags. Expected sites: `types.rs` tests (3 literals), `index.rs::to_symbol_node`, `scan.rs::build_folder` (2 literals), `outrider-layout/src/arrange.rs`, `outrider-layout/src/measure.rs`, `outrider-layout/tests/common/mod.rs`, `outrider-layout/tests/cross_process.rs`, `outrider-layout/tests/props_continuity.rs`, `outrider/src/focus.rs` test helper, `outrider/src/world.rs` test helper. Two of these are NOT plain `None` — wire them in the next step.

- [ ] **Step 6: Thread signature and doc through index/scan**

In `crates/outrider-index/src/scan.rs`, add above `build_tree`:

```rust
/// Parsed per-file payload: item nodes plus the file's `//!` doc block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedFile {
    pub items: Vec<SymbolNode>,
    pub doc: Option<String>,
}
```

Change `build_tree` and `build_folder` parameters from `rs_children: &BTreeMap<PathBuf, Vec<SymbolNode>>` to `rs_children: &BTreeMap<PathBuf, ParsedFile>`. In `build_folder`'s file branch replace the `file_children` binding and the File literal's tail:

```rust
                let parsed = rs_children.get(&file.rel_path).cloned().unwrap_or_default();
                children.push(SymbolNode {
                    id: SymbolId {
                        kind: SymbolKind::File,
                        qualified_path: qual,
                        ordinal: 0,
                    },
                    name: file_name.clone(),
                    byte_range: Some(0..file.bytes as usize),
                    signature: None,
                    doc: parsed.doc,
                    measure: file.lines,
                    churn: 0.0,
                    churn_count: 0,
                    children: parsed.items,
                });
```

(The Folder literal at the bottom of `build_folder` gets plain `signature: None, doc: None,`.)

In `crates/outrider-index/src/index.rs`: change `parse_all_rust`'s return type to `anyhow::Result<BTreeMap<PathBuf, ParsedFile>>` (import `ParsedFile` from `crate::scan`), and change its `Ok((…))` to:

```rust
            Ok((
                f.rel_path.clone(),
                ParsedFile { items: children, doc: crate::parse::file_doc(&source) },
            ))
```

(rename the local `children` binding accordingly if needed). In `to_symbol_node`, the literal gets `signature: Some(item.signature), doc: None,`.

- [ ] **Step 7: Update the fixture and index_test**

Prepend to `crates/outrider-index/tests/fixtures/mini_repo/src/lib.rs` (do NOT touch `util.rs`):

```rust
//! Mini fixture library.
//! Exercises doc extraction.

```

(two `//!` lines plus one blank line before `mod inner {`). Append inside `index_repo_parses_rust_files_into_items` in `crates/outrider-index/tests/index_test.rs`:

```rust
    // Phase 4b metadata: signature + doc (spec §3.1)
    assert_eq!(
        lib.doc.as_deref(),
        Some("Mini fixture library.\nExercises doc extraction.")
    );
    assert_eq!(lib.signature, None);
    assert_eq!(norm.signature.as_deref(), Some("fn norm(&self) -> f64"));
    let util = find(&tree.root, "src/util.rs").expect("util.rs node");
    assert_eq!(util.doc, None);
    assert_eq!(tree.root.signature, None);
    assert_eq!(tree.root.doc, None);
```

Note: `lib`, `norm` are existing bindings in that test. `scan_test.rs` needs no edits — its `build_tree(dir.path(), &files, &BTreeMap::new())` infers the new map type.

- [ ] **Step 8: Full workspace verification**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: all suites PASS (previously 13 suites / 58 tests, plus the new ones), clippy clean.

- [ ] **Step 9: Commit**

```bash
git add -A crates docs
git commit -m "feat(index): extract item signatures and file //! docs into SymbolNode"
```

---

### Task 2: Buffer substrate — `FileBuffer`, highlighting, `AnchorList`

**Files:**
- Create: `crates/outrider-index/src/buffer.rs`
- Modify: `crates/outrider-index/src/lib.rs`
- Modify: `crates/outrider-index/Cargo.toml`

**Interfaces:**
- Consumes: `tree_sitter::{Query, QueryCursor, StreamingIterator, Tree}` (StreamingIterator is re-exported by tree-sitter 0.26), `tree_sitter_rust::{LANGUAGE, HIGHLIGHTS_QUERY}`, `ropey::Rope`.
- Produces (all `pub` in `outrider_index::buffer`):
  - `enum HighlightKind { Keyword, Function, Type, String, Comment, Number, Property, Default }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `struct HighlightSpan { pub range: Range<usize>, pub kind: HighlightKind }` (byte range within the line; derives `Debug, Clone, PartialEq, Eq`)
  - `struct FileBuffer` with `new(text: String) -> anyhow::Result<Self>`, `len_lines(&self) -> usize`, `line(&self, i: usize) -> Option<(String, &[HighlightSpan])>`, `byte_to_line(&self, byte: usize) -> usize`, `create_anchor(&mut self, offset: usize) -> AnchorId`, `resolve_anchor(&self, id: AnchorId) -> usize`
  - `struct AnchorId(usize)` (derives `Debug, Clone, Copy, PartialEq, Eq, Hash`), `struct Edit { pub range: Range<usize>, pub new_len: usize }`, `struct AnchorList` (derives `Debug, Default`) with `create`, `resolve`, `remap(&mut self, edit: &Edit)`

- [ ] **Step 1: Add ropey and the module**

In `crates/outrider-index/Cargo.toml` `[dependencies]`, add `ropey = "1.6"`. In `crates/outrider-index/src/lib.rs`, add `pub mod buffer;` (alphabetical, before `churn`). Create `crates/outrider-index/src/buffer.rs` containing only the anchor types for now:

```rust
use std::ops::Range;

/// Handle to a tracked byte position (spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnchorId(usize);

/// A buffer mutation: `range` replaced by `new_len` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub new_len: usize,
}

#[derive(Debug, Default)]
pub struct AnchorList {
    positions: Vec<usize>,
}

impl AnchorList {
    pub fn create(&mut self, offset: usize) -> AnchorId {
        self.positions.push(offset);
        AnchorId(self.positions.len() - 1)
    }

    pub fn resolve(&self, id: AnchorId) -> usize {
        self.positions[id.0]
    }

    /// Survive-edits rule: positions at/after the edit's end shift by the
    /// length delta; positions strictly inside a replaced/deleted range
    /// clamp to its start. A position at the edit's start stays put
    /// (unless the edit is an insertion exactly there, which shifts it).
    pub fn remap(&mut self, edit: &Edit) {
        let delta = edit.new_len as isize - edit.range.len() as isize;
        for p in &mut self.positions {
            if *p >= edit.range.end {
                *p = (*p as isize + delta) as usize;
            } else if *p > edit.range.start {
                *p = edit.range.start;
            }
        }
    }
}
```

- [ ] **Step 2: Write the failing anchor tests**

Append to `buffer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_remap_insert_before_inside_after() {
        let mut a = AnchorList::default();
        let id = a.create(100);
        a.remap(&Edit { range: 50..50, new_len: 10 }); // insert before → shifts
        assert_eq!(a.resolve(id), 110);
        a.remap(&Edit { range: 200..200, new_len: 7 }); // insert after → unchanged
        assert_eq!(a.resolve(id), 110);
        a.remap(&Edit { range: 105..120, new_len: 3 }); // replace spanning → clamp
        assert_eq!(a.resolve(id), 105);
    }

    #[test]
    fn anchor_delete_spanning_clamps_to_start() {
        let mut a = AnchorList::default();
        let id = a.create(30);
        a.remap(&Edit { range: 20..40, new_len: 0 });
        assert_eq!(a.resolve(id), 20);
    }

    #[test]
    fn anchor_at_edit_boundaries() {
        let mut a = AnchorList::default();
        let id = a.create(50);
        a.remap(&Edit { range: 50..60, new_len: 1 }); // edit starts at anchor → stays
        assert_eq!(a.resolve(id), 50);
        a.remap(&Edit { range: 50..50, new_len: 4 }); // insertion at anchor → shifts
        assert_eq!(a.resolve(id), 54);
    }

    #[test]
    fn multi_anchor_ordering_preserved() {
        let mut a = AnchorList::default();
        let x = a.create(10);
        let y = a.create(20);
        let z = a.create(30);
        a.remap(&Edit { range: 15..25, new_len: 2 }); // y clamps to 15; z shifts −8
        assert_eq!((a.resolve(x), a.resolve(y), a.resolve(z)), (10, 15, 22));
        assert!(a.resolve(x) <= a.resolve(y) && a.resolve(y) <= a.resolve(z));
    }
}
```

- [ ] **Step 3: Run the anchor tests**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index --lib buffer`
Expected: PASS (the Step 1 implementation already satisfies them; if any fail, fix `remap` — the rules are in its doc comment).

- [ ] **Step 4: Write the failing FileBuffer tests**

Append inside `mod tests`:

```rust
    const SNIPPET: &str =
        "// a comment line\nfn free() -> i32 {\n    let s = \"hi\";\n    42\n}\n";

    #[test]
    fn highlight_kinds_and_bounds() {
        let buf = FileBuffer::new(SNIPPET.to_string()).unwrap();
        assert_eq!(buf.len_lines(), 5);
        let (t0, s0) = buf.line(0).unwrap();
        assert_eq!(t0, "// a comment line");
        assert!(s0.iter().any(|s| s.kind == HighlightKind::Comment));
        let (t1, s1) = buf.line(1).unwrap();
        assert_eq!(t1, "fn free() -> i32 {");
        assert!(s1
            .iter()
            .any(|s| s.kind == HighlightKind::Keyword && &t1[s.range.clone()] == "fn"));
        let (_t2, s2) = buf.line(2).unwrap();
        assert!(s2.iter().any(|s| s.kind == HighlightKind::String));
        // every span lies within its line's bounds, sorted and non-overlapping
        for i in 0..buf.len_lines() {
            let (text, spans) = buf.line(i).unwrap();
            let mut end = 0;
            for s in spans {
                assert!(s.range.start < s.range.end);
                assert!(
                    s.range.start >= end && s.range.end <= text.len(),
                    "span {:?} out of bounds in line {i}: {text:?}",
                    s.range
                );
                end = s.range.end;
            }
        }
        assert!(buf.line(5).is_none());
    }

    #[test]
    fn byte_to_line_and_anchor_roundtrip() {
        let mut buf = FileBuffer::new(SNIPPET.to_string()).unwrap();
        assert_eq!(buf.byte_to_line(0), 0);
        assert_eq!(buf.byte_to_line(18), 1); // first byte of "fn free…"
        let a = buf.create_anchor(18);
        assert_eq!(buf.resolve_anchor(a), 18);
        assert_eq!(buf.byte_to_line(buf.resolve_anchor(a)), 1);
    }
```

- [ ] **Step 5: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index --lib buffer`
Expected: FAIL to compile — `FileBuffer` not defined.

- [ ] **Step 6: Implement FileBuffer**

Add to the top of `buffer.rs` (replacing the lone `use std::ops::Range;`):

```rust
use std::ops::Range;

use anyhow::Context;
use ropey::Rope;
use tree_sitter::{Query, QueryCursor, StreamingIterator, Tree};
```

Add after the `Edit`/`AnchorList` block:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    Function,
    Type,
    String,
    Comment,
    Number,
    Property,
    Default,
}

/// A colored span; `range` is a byte range within its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

pub struct FileBuffer {
    rope: Rope,
    /// Held for Phase 6 incremental re-parse; unused until then.
    #[allow(dead_code)]
    tree: Tree,
    /// Per-line spans, computed once at materialization (spec §3.2).
    lines: Vec<Vec<HighlightSpan>>,
    anchors: AnchorList,
}

impl FileBuffer {
    pub fn new(text: String) -> anyhow::Result<Self> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .context("loading tree-sitter-rust grammar")?;
        let tree = parser.parse(&text, None).context("tree-sitter parse failed")?;
        let lines = highlight_lines(&text, &tree)?;
        Ok(Self { rope: Rope::from(text), tree, lines, anchors: AnchorList::default() })
    }

    /// Content lines (the empty final line after a trailing newline is not counted).
    pub fn len_lines(&self) -> usize {
        self.lines.len()
    }

    /// Line text (newline stripped) plus its highlight spans. Text comes
    /// from the rope — never from a cached copy of the original string.
    pub fn line(&self, i: usize) -> Option<(String, &[HighlightSpan])> {
        let spans = self.lines.get(i)?;
        let mut text = self.rope.line(i).to_string();
        while text.ends_with(['\n', '\r']) {
            text.pop();
        }
        Some((text, spans.as_slice()))
    }

    pub fn byte_to_line(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte.min(self.rope.len_bytes()))
    }

    pub fn create_anchor(&mut self, offset: usize) -> AnchorId {
        self.anchors.create(offset)
    }

    pub fn resolve_anchor(&self, id: AnchorId) -> usize {
        self.anchors.resolve(id)
    }
}

/// Byte bounds of each line's content (trailing newline/CR excluded).
fn line_bounds(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for seg in text.split_inclusive('\n') {
        let content = seg.trim_end_matches(['\n', '\r']);
        out.push(start..start + content.len());
        start += seg.len();
    }
    out
}

/// Capture-name prefix → HighlightKind (spec §3.2). Unmapped captures
/// (punctuation, operators, …) are skipped and paint as Default.
fn kind_for(capture: &str) -> Option<HighlightKind> {
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

/// Run HIGHLIGHTS_QUERY over the whole file, splitting captures into
/// per-line spans. Overlaps resolve outermost-first (sort by start asc,
/// end desc; drop spans starting inside an earlier-kept span).
fn highlight_lines(text: &str, tree: &Tree) -> anyhow::Result<Vec<Vec<HighlightSpan>>> {
    let bounds = line_bounds(text);
    let mut lines: Vec<Vec<HighlightSpan>> = vec![Vec::new(); bounds.len()];
    let query = Query::new(&tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY)
        .context("compiling rust highlight query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let Some(kind) = kind_for(query.capture_names()[cap.index as usize]) else {
                continue;
            };
            let r = cap.node.byte_range();
            let first = bounds.partition_point(|b| b.end < r.start);
            for (l, b) in bounds.iter().enumerate().skip(first) {
                if b.start >= r.end {
                    break;
                }
                let s = r.start.max(b.start);
                let e = r.end.min(b.end);
                if s < e {
                    lines[l].push(HighlightSpan { range: s - b.start..e - b.start, kind });
                }
            }
        }
    }
    for spans in &mut lines {
        spans.sort_by(|a, b| a.range.start.cmp(&b.range.start).then(b.range.end.cmp(&a.range.end)));
        let mut end = 0;
        spans.retain(|s| {
            if s.range.start >= end {
                end = s.range.end;
                true
            } else {
                false
            }
        });
    }
    Ok(lines)
}
```

Note: `StreamingIterator` must be in scope for `matches.next()` — tree-sitter 0.26's `QueryCursor::matches` returns a streaming iterator, not a std `Iterator`.

- [ ] **Step 7: Run to verify pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index --lib buffer`
Expected: PASS (6 tests). If a kind assertion fails, inspect the actual capture names: add a temporary `dbg!(query.capture_names())` — the mapping in `kind_for` keys on the prefix before the first `.`.

- [ ] **Step 8: Workspace check and commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: PASS, clippy clean.

```bash
git add crates/outrider-index docs Cargo.lock
git commit -m "feat(index): rope-backed FileBuffer with tree-sitter highlighting and anchors"
```

---

### Task 3: `Rung::{Detail, Full}` and the unclipped top

**Files:**
- Modify: `crates/outrider/src/world.rs`

**Interfaces:**
- Consumes: existing `rung_for`, `Rung`, `walk`, `DrawItem`.
- Produces: `pub const DETAIL_PX: f64 = 250.0; pub const FULL_PX: f64 = 700.0; pub const CODE_MIN_W: f64 = 300.0;`; `Rung` gains `Detail` and `Full` variants; `DrawItem` gains `pub top: f64` — the UNclipped screen-y of the box top (`px.y` stays clipped to the viewport). Task 6's line-window cull needs `top`.

- [ ] **Step 1: Update the rung test to the new ladder**

In `crates/outrider/src/world.rs`, replace the body of `rung_for_thresholds_and_downgrade` with:

```rust
        // height thresholds (wide column: no downgrade)
        assert_eq!(rung_for(3.9, 400.0), None);
        assert_eq!(rung_for(4.0, 400.0), Some(Rung::Dot));
        assert_eq!(rung_for(19.9, 400.0), Some(Rung::Dot));
        assert_eq!(rung_for(20.0, 400.0), Some(Rung::Label));
        assert_eq!(rung_for(79.9, 400.0), Some(Rung::Label));
        assert_eq!(rung_for(80.0, 400.0), Some(Rung::Card));
        assert_eq!(rung_for(249.9, 400.0), Some(Rung::Card));
        assert_eq!(rung_for(250.0, 400.0), Some(Rung::Detail));
        assert_eq!(rung_for(699.9, 400.0), Some(Rung::Detail));
        assert_eq!(rung_for(700.0, 400.0), Some(Rung::Full));
        // narrow columns are forced to Dot regardless of height (gutters)
        assert_eq!(rung_for(100_000.0, 59.9), Some(Rung::Dot));
        // Full downgrades to Detail when too narrow for code (spec §4.2)
        assert_eq!(rung_for(100_000.0, 60.0), Some(Rung::Detail));
        assert_eq!(rung_for(100_000.0, 299.9), Some(Rung::Detail));
        assert_eq!(rung_for(100_000.0, 300.0), Some(Rung::Full));
        // the CODE_MIN_W downgrade applies only to Full
        assert_eq!(rung_for(100.0, 60.0), Some(Rung::Card));
        // the merge rule wins over everything
        assert_eq!(rung_for(3.9, 24.0), None);
```

- [ ] **Step 2: Update the two culling tests' rung expectations**

In `culling_home_view`: root (571.4 px) and a.rs (285.7 px) are now Detail. Replace the rungs assertion (and its height comment stays accurate) with:

```rust
        assert_eq!(rungs, vec![Rung::Detail, Rung::Detail, Rung::Label, Rung::Label, Rung::Dot]);
```

In `zoomed_past_ancestors_clip_and_compress`: root is Full-by-height (36571 px) but 36.19 px wide → Dot; b.rs is Full-by-height (4571 px) but 144.76 < CODE_MIN_W → Detail; g is 571.4 px → Detail. Replace the rungs assertion and its comment with:

```rust
        // root narrow (36.2 < LABEL_MIN_W) → Dot; b.rs Full-height but
        // 144.76 < CODE_MIN_W → Detail; g 571.4px → Detail
        assert_eq!(rungs, vec![Rung::Dot, Rung::Detail, Rung::Detail]);
```

Also append to `zoomed_past_ancestors_clip_and_compress` (tests the new `top` field — root's unclipped top is `300 − 0.6875·zoom`):

```rust
        // DrawItem.top is the UNclipped screen top (px.y is clipped to -2)
        assert!((items[0].top - (300.0 - 0.6875 * (256000.0 / 7.0))).abs() < 1e-6);
        assert!((items[2].top - 300.0).abs() < 1e-6); // on-screen: top == px.y
```

- [ ] **Step 3: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider world::`
Expected: FAIL to compile — no `Rung::Detail` / `Rung::Full` / `DrawItem.top`.

- [ ] **Step 4: Implement**

In `world.rs`:

1. After `pub const CARD_PX: f64 = 80.0;` add:

```rust
pub const DETAIL_PX: f64 = 250.0;
pub const FULL_PX: f64 = 700.0;
/// Full is useless in a sliver column; below this width it downgrades to Detail.
pub const CODE_MIN_W: f64 = 300.0;
```

2. `Rung` gains the variants (order matters for readability, not semantics):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    Dot,
    Label,
    Card,
    Detail,
    Full,
}
```

3. Replace `rung_for`:

```rust
/// Rung by pixel height, downgraded to Dot when the column is too narrow
/// for text (gutter strips) and from Full to Detail when too narrow for
/// code. Heights below MERGE_PX merge into the parent.
pub fn rung_for(px_h: f64, px_w: f64) -> Option<Rung> {
    let by_height = if px_h < MERGE_PX {
        return None;
    } else if px_h < LABEL_PX {
        Rung::Dot
    } else if px_h < CARD_PX {
        Rung::Label
    } else if px_h < DETAIL_PX {
        Rung::Card
    } else if px_h < FULL_PX {
        Rung::Detail
    } else {
        Rung::Full
    };
    let rung = if px_w < LABEL_MIN_W { Rung::Dot } else { by_height };
    Some(if rung == Rung::Full && px_w < CODE_MIN_W { Rung::Detail } else { rung })
}
```

4. `DrawItem` gains the unclipped top:

```rust
#[derive(Debug)]
pub struct DrawItem<'a> {
    pub node: &'a SymbolNode,
    pub px: PxRect,
    pub rung: Rung,
    /// UNclipped screen-y of the box top (`px.y` is clipped to the viewport).
    pub top: f64,
}
```

and in `walk`, the push becomes:

```rust
    out.push(DrawItem { node, px: PxRect { x: px_x, y: y0, w: px_w, h: y1 - y0 }, rung, top: px_y });
```

- [ ] **Step 5: Run to verify pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider`
Expected: PASS — all world/focus/camera/treemap unit tests (treemap.rs compiles unchanged; it only matches on `Rung::Dot`/`Label`/`Card`, which still exist).

- [ ] **Step 6: Clippy and commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings`
Expected: clean.

```bash
git add crates/outrider/src/world.rs docs
git commit -m "feat(world): Detail and Full rungs with CODE_MIN_W downgrade, unclipped DrawItem.top"
```

---

### Task 4: `BufferManager` — LRU materialization + per-symbol anchors

**Files:**
- Create: `crates/outrider/src/buffers.rs`
- Modify: `crates/outrider/src/main.rs` (add `mod buffers;`)
- Modify: `crates/outrider/Cargo.toml` (add `[dev-dependencies] tempfile = "3.27.0"`)

**Interfaces:**
- Consumes: `outrider_index::buffer::{AnchorId, FileBuffer}` (Task 2), `outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree}`.
- Produces (all in `crate::buffers`, GPUI-free):
  - `pub const MAX_BUFFERS: usize = 64;`
  - `pub struct Materialized { pub buffer: FileBuffer, /* private */ anchors: BTreeMap<SymbolId, AnchorId> }` with `pub fn symbol_start_line(&self, id: &SymbolId) -> Option<usize>`
  - `pub struct BufferManager` with `pub fn new(repo_root: PathBuf) -> Self`, `pub fn file_path_of(qualified_path: &str) -> &str`, `pub fn get(&mut self, rel_path: &str, symbols: &[(SymbolId, usize)]) -> Option<&Materialized>` (`symbols` is used only on first materialization; ignored on cache hits)
  - `pub fn collect_file_symbols(tree: &SymbolTree) -> BTreeMap<String, Vec<(SymbolId, usize)>>` — rel file path → `(id, byte_range.start)` of every item in that file's subtree

- [ ] **Step 1: Write the failing tests**

Create `crates/outrider/src/buffers.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    fn write_file(dir: &std::path::Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn fn_id(qual: &str) -> SymbolId {
        SymbolId { kind: SymbolKind::Fn, qualified_path: qual.into(), ordinal: 0 }
    }

    #[test]
    fn file_path_of_splits_at_first_colons() {
        assert_eq!(BufferManager::file_path_of("src/lib.rs::Point::norm"), "src/lib.rs");
        assert_eq!(BufferManager::file_path_of("src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn get_materializes_creates_anchors_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.rs", "fn one() {}\nfn two() {}\n");
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let syms = vec![(fn_id("a.rs::one"), 0), (fn_id("a.rs::two"), 12)];
        let m = mgr.get("a.rs", &syms).unwrap();
        assert_eq!(m.buffer.len_lines(), 2);
        assert_eq!(m.symbol_start_line(&fn_id("a.rs::one")), Some(0));
        assert_eq!(m.symbol_start_line(&fn_id("a.rs::two")), Some(1));
        assert_eq!(m.symbol_start_line(&fn_id("a.rs::absent")), None);
        // cache hit: delete from disk; a second get must NOT re-read
        std::fs::remove_file(dir.path().join("a.rs")).unwrap();
        assert!(mgr.get("a.rs", &[]).is_some());
    }

    #[test]
    fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        assert!(mgr.get("nope.rs", &[]).is_none());
    }

    #[test]
    fn lru_evicts_least_recent_beyond_cap() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..=MAX_BUFFERS {
            write_file(dir.path(), &format!("f{i}.rs"), "fn x() {}\n");
        }
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        for i in 0..MAX_BUFFERS {
            mgr.get(&format!("f{i}.rs"), &[]).unwrap();
        }
        // touch f0 (refresh recency), then insert one past the cap
        mgr.get("f0.rs", &[]).unwrap();
        mgr.get(&format!("f{MAX_BUFFERS}.rs"), &[]).unwrap();
        // f1 is now least-recent and was evicted: with the file gone, a
        // fresh get must fail (re-materialization from disk)
        std::fs::remove_file(dir.path().join("f1.rs")).unwrap();
        assert!(mgr.get("f1.rs", &[]).is_none());
        // f0 survived the eviction (recency was refreshed)
        std::fs::remove_file(dir.path().join("f0.rs")).unwrap();
        assert!(mgr.get("f0.rs", &[]).is_some());
    }

    #[test]
    fn collect_file_symbols_maps_items_by_file() {
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
                vec![node(
                    SymbolKind::File,
                    "a.rs",
                    Some(0..40),
                    vec![node(
                        SymbolKind::Impl,
                        "a.rs::T",
                        Some(0..30),
                        vec![node(SymbolKind::Fn, "a.rs::T::m", Some(10..25), vec![])],
                    )],
                )],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        };
        let map = collect_file_symbols(&tree);
        assert_eq!(map.len(), 1);
        let got: Vec<(&str, SymbolKind, usize)> = map
            .get("a.rs")
            .unwrap()
            .iter()
            .map(|(id, s)| (id.qualified_path.as_str(), id.kind, *s))
            .collect();
        assert_eq!(
            got,
            vec![("a.rs::T", SymbolKind::Impl, 0), ("a.rs::T::m", SymbolKind::Fn, 10)]
        );
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider buffers::`
Expected: FAIL to compile — module not declared / types missing. First add `mod buffers;` to `crates/outrider/src/main.rs` (after `mod camera;`) and `tempfile = "3.27.0"` under a new `[dev-dependencies]` section in `crates/outrider/Cargo.toml`, then re-run to see the type errors.

- [ ] **Step 3: Implement**

Prepend to `crates/outrider/src/buffers.rs`:

```rust
use std::collections::BTreeMap;
use std::path::PathBuf;

use outrider_index::buffer::{AnchorId, FileBuffer};
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

pub const MAX_BUFFERS: usize = 64;

/// A materialized file: rope-backed buffer plus one anchor per symbol,
/// created at materialization (spec §3.3).
pub struct Materialized {
    pub buffer: FileBuffer,
    anchors: BTreeMap<SymbolId, AnchorId>,
}

impl Materialized {
    /// Rope line index of the symbol's start, via its anchor — the Full
    /// render never reads raw `byte_range` offsets.
    pub fn symbol_start_line(&self, id: &SymbolId) -> Option<usize> {
        let a = self.anchors.get(id)?;
        Some(self.buffer.byte_to_line(self.buffer.resolve_anchor(*a)))
    }
}

/// LRU cache of materialized buffers, keyed by relative file path.
/// Most-recently-used entry is last (spec §4.1).
pub struct BufferManager {
    repo_root: PathBuf,
    entries: Vec<(String, Materialized)>,
}

impl BufferManager {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root, entries: Vec::new() }
    }

    /// The file-path portion of a qualified_path: everything before the
    /// first `::` (the whole path when there is none, as on File nodes).
    pub fn file_path_of(qualified_path: &str) -> &str {
        qualified_path.split("::").next().unwrap_or(qualified_path)
    }

    /// Materialize from disk on first access, creating one anchor per
    /// symbol; refresh recency on hits (no disk re-read); LRU-evict beyond
    /// MAX_BUFFERS. None if the file cannot be read or parsed — the box
    /// falls back to Detail content.
    pub fn get(&mut self, rel_path: &str, symbols: &[(SymbolId, usize)]) -> Option<&Materialized> {
        if let Some(i) = self.entries.iter().position(|(p, _)| p == rel_path) {
            let e = self.entries.remove(i);
            self.entries.push(e);
        } else {
            let text = std::fs::read_to_string(self.repo_root.join(rel_path)).ok()?;
            let mut buffer = FileBuffer::new(text).ok()?;
            let anchors = symbols
                .iter()
                .map(|(id, start)| (id.clone(), buffer.create_anchor(*start)))
                .collect();
            self.entries.push((rel_path.to_string(), Materialized { buffer, anchors }));
            if self.entries.len() > MAX_BUFFERS {
                self.entries.remove(0);
            }
        }
        self.entries.last().map(|(_, m)| m)
    }
}

/// rel file path → (id, byte_range.start) of every item inside that file,
/// from the tree. Built once at view construction; `get` uses it to create
/// anchors at materialization.
pub fn collect_file_symbols(tree: &SymbolTree) -> BTreeMap<String, Vec<(SymbolId, usize)>> {
    fn items(node: &SymbolNode, out: &mut Vec<(SymbolId, usize)>) {
        for c in &node.children {
            if let Some(r) = &c.byte_range {
                out.push((c.id.clone(), r.start));
            }
            items(c, out);
        }
    }
    fn walk(node: &SymbolNode, out: &mut BTreeMap<String, Vec<(SymbolId, usize)>>) {
        if node.id.kind == SymbolKind::File {
            let mut v = Vec::new();
            items(node, &mut v);
            out.insert(node.id.qualified_path.clone(), v);
        } else {
            for c in &node.children {
                walk(c, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&tree.root, &mut out);
    out
}
```

Note: until Task 6 wires the module into `treemap.rs`, these items are unused from the binary — if `cargo clippy` flags dead code, put `#![allow(dead_code)]`-style attributes NOWHERE; instead add `pub(crate)` is already implied by `pub` in a bin crate. If dead-code warnings appear, add a single `#[allow(dead_code)]` on the items the compiler names, and REMOVE those allows in Task 6 Step 5.

- [ ] **Step 4: Run to verify pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider buffers::`
Expected: PASS (5 tests). The LRU test parses 65 small files — expect a few seconds, not minutes.

- [ ] **Step 5: Workspace check and commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: PASS, clippy clean (with the temporary allows from Step 3 if needed).

```bash
git add crates/outrider docs Cargo.lock
git commit -m "feat(app): LRU BufferManager materializing FileBuffers with per-symbol anchors"
```

---

### Task 5: Content assembly — inventories, summaries, body lines

**Files:**
- Create: `crates/outrider/src/content.rs`
- Modify: `crates/outrider/src/main.rs` (add `mod content;`)

**Interfaces:**
- Consumes: `SymbolNode.{signature, doc}` (Task 1), `crate::world::Rung` with `Detail`/`Full` (Task 3).
- Produces (all in `crate::content`, GPUI-free):
  - `pub enum BodyLine { Plain(String), Dim(String) }` (derives `Debug, Clone, PartialEq, Eq`) — Plain paints TEXT_PRIMARY, Dim paints TEXT_SECONDARY
  - `pub fn card_meta(node: &SymbolNode) -> String` — the pre-4b Card meta, format unchanged: `"{churn_count} · p{churn*100:.0} · {measure}L"`
  - `pub fn churn_readout(node: &SymbolNode) -> String` — `"480L · 47 commits · p96"`
  - `pub fn kind_counts(node: &SymbolNode) -> String` — `"3 fns · 1 struct · 1 impl"` (descendant items, order fns/structs/enums/traits/impls/mods, zero counts omitted); for Folders: direct-child `"2 files · 1 folder"`. Empty string when nothing to count.
  - `pub fn inventory(node: &SymbolNode) -> String` — `kind_counts · churn_readout` joined (just the readout when counts are empty)
  - `pub fn body_lines(node: &SymbolNode, rung: Rung) -> Vec<BodyLine>` — the spec §4.3 table. Full leaf items return only their signature; the paint path appends code (or leaves this Detail-equivalent content when the buffer is unavailable).

- [ ] **Step 1: Write the failing tests**

Create `crates/outrider/src/content.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    #[allow(clippy::too_many_arguments)]
    fn node(
        kind: SymbolKind,
        qual: &str,
        measure: u64,
        churn: f32,
        churn_count: u64,
        signature: Option<&str>,
        doc: Option<&str>,
        children: Vec<SymbolNode>,
    ) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qual.into(), ordinal: 0 },
            name: qual.rsplit(['/', ':']).next().unwrap_or(qual).to_string(),
            byte_range: None,
            signature: signature.map(str::to_string),
            doc: doc.map(str::to_string),
            measure,
            churn,
            churn_count,
            children,
        }
    }

    /// File m.rs: struct Point, impl Point { fn new, fn norm }, fn free —
    /// 480L, 47 commits, p96, two-line doc.
    fn file() -> SymbolNode {
        node(
            SymbolKind::File,
            "m.rs",
            480,
            0.96,
            47,
            None,
            Some("Doc first.\nDoc second."),
            vec![
                node(SymbolKind::Struct, "m.rs::Point", 4, 0.5, 3, Some("struct Point"), None, vec![]),
                node(
                    SymbolKind::Impl,
                    "m.rs::Point",
                    9,
                    0.5,
                    3,
                    Some("impl Point"),
                    None,
                    vec![
                        node(SymbolKind::Fn, "m.rs::Point::new", 3, 0.5, 3, Some("fn new() -> Self"), None, vec![]),
                        node(SymbolKind::Fn, "m.rs::Point::norm", 3, 0.5, 3, Some("fn norm(&self) -> f64"), None, vec![]),
                    ],
                ),
                node(SymbolKind::Fn, "m.rs::free", 3, 0.5, 3, Some("fn free()"), None, vec![]),
            ],
        )
    }

    fn folder() -> SymbolNode {
        node(
            SymbolKind::Folder,
            "src",
            812,
            0.4,
            12,
            None,
            None,
            vec![
                node(SymbolKind::File, "src/a.rs", 400, 0.0, 0, None, None, vec![]),
                node(SymbolKind::File, "src/b.rs", 400, 0.0, 0, None, None, vec![]),
                node(SymbolKind::Folder, "src/sub", 12, 0.0, 0, None, None, vec![]),
            ],
        )
    }

    #[test]
    fn inventory_strings_are_exact() {
        let f = file();
        assert_eq!(churn_readout(&f), "480L · 47 commits · p96");
        assert_eq!(kind_counts(&f), "3 fns · 1 struct · 1 impl");
        assert_eq!(inventory(&f), "3 fns · 1 struct · 1 impl · 480L · 47 commits · p96");
        let d = folder();
        assert_eq!(kind_counts(&d), "2 files · 1 folder");
        assert_eq!(inventory(&d), "2 files · 1 folder · 812L · 12 commits · p40");
        // empty node: inventory degrades to the readout alone
        let empty = node(SymbolKind::File, "e.rs", 0, 0.0, 0, None, None, vec![]);
        assert_eq!(kind_counts(&empty), "");
        assert_eq!(inventory(&empty), "0L · 0 commits · p0");
        // card meta keeps the pre-4b format exactly
        assert_eq!(card_meta(&f), "47 · p96 · 480L");
    }

    #[test]
    fn body_lines_follow_the_content_table() {
        use BodyLine::{Dim, Plain};
        let f = file();
        let leaf = &f.children[2]; // fn free
        let container = &f.children[1]; // impl Point (2 children)
        let d = folder();

        // leaf item: signature at Detail AND Full (code appended by paint)
        assert_eq!(body_lines(leaf, Rung::Detail), vec![Plain("fn free()".into())]);
        assert_eq!(body_lines(leaf, Rung::Full), vec![Plain("fn free()".into())]);
        // container item: signature; Full adds the inventory
        assert_eq!(body_lines(container, Rung::Detail), vec![Plain("impl Point".into())]);
        assert_eq!(
            body_lines(container, Rung::Full),
            vec![Plain("impl Point".into()), Dim(inventory(container))]
        );
        // file Detail: churn readout + doc first line + kind counts
        assert_eq!(
            body_lines(&f, Rung::Detail),
            vec![
                Dim("480L · 47 commits · p96".into()),
                Plain("Doc first.".into()),
                Dim("3 fns · 1 struct · 1 impl".into()),
            ]
        );
        // file Full: whole doc block + inventory
        assert_eq!(
            body_lines(&f, Rung::Full),
            vec![
                Plain("Doc first.".into()),
                Plain("Doc second.".into()),
                Dim(inventory(&f)),
            ]
        );
        // folder Detail: readout + counts; Full: inventory only
        assert_eq!(
            body_lines(&d, Rung::Detail),
            vec![Dim("812L · 12 commits · p40".into()), Dim("2 files · 1 folder".into())]
        );
        assert_eq!(body_lines(&d, Rung::Full), vec![Dim(inventory(&d))]);
        // file without docs
        let nodoc = node(SymbolKind::File, "n.rs", 9, 0.0, 0, None, None, vec![]);
        assert_eq!(body_lines(&nodoc, Rung::Detail), vec![Dim("9L · 0 commits · p0".into())]);
        assert_eq!(body_lines(&nodoc, Rung::Full), vec![Dim("9L · 0 commits · p0".into())]);
        // Card keeps the legacy meta; Dot/Label have no body
        assert_eq!(body_lines(&f, Rung::Card), vec![Dim("47 · p96 · 480L".into())]);
        assert_eq!(body_lines(&f, Rung::Dot), vec![]);
        assert_eq!(body_lines(&f, Rung::Label), vec![]);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Add `mod content;` to `crates/outrider/src/main.rs` (after `mod camera;`, alphabetical). Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider content::`
Expected: FAIL to compile — the functions don't exist yet.

- [ ] **Step 3: Implement**

Prepend to `crates/outrider/src/content.rs`:

```rust
use outrider_index::{SymbolKind, SymbolNode};

use crate::world::Rung;

/// One rendered body line under a box's name row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyLine {
    /// TEXT_PRIMARY
    Plain(String),
    /// TEXT_SECONDARY
    Dim(String),
}

/// Card meta line — format unchanged from the pre-4b render (spec §4.4).
pub fn card_meta(node: &SymbolNode) -> String {
    format!("{} · p{:.0} · {}L", node.churn_count, node.churn * 100.0, node.measure)
}

/// e.g. "480L · 47 commits · p96"
pub fn churn_readout(node: &SymbolNode) -> String {
    format!("{}L · {} commits · p{:.0}", node.measure, node.churn_count, node.churn * 100.0)
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Item counts by kind: all descendants for files/items ("3 fns · 1 struct");
/// direct child files/folders for folders ("2 files · 1 folder"). Empty
/// string when there is nothing to count.
pub fn kind_counts(node: &SymbolNode) -> String {
    if node.id.kind == SymbolKind::Folder {
        let files = node.children.iter().filter(|c| c.id.kind == SymbolKind::File).count();
        let folders = node.children.iter().filter(|c| c.id.kind == SymbolKind::Folder).count();
        let mut parts = Vec::new();
        if files > 0 {
            parts.push(plural(files, "file"));
        }
        if folders > 0 {
            parts.push(plural(folders, "folder"));
        }
        return parts.join(" · ");
    }
    fn count(node: &SymbolNode, c: &mut [usize; 6]) {
        for k in &node.children {
            match k.id.kind {
                SymbolKind::Fn => c[0] += 1,
                SymbolKind::Struct => c[1] += 1,
                SymbolKind::Enum => c[2] += 1,
                SymbolKind::Trait => c[3] += 1,
                SymbolKind::Impl => c[4] += 1,
                SymbolKind::Module => c[5] += 1,
                SymbolKind::File | SymbolKind::Folder => {}
            }
            count(k, c);
        }
    }
    let mut c = [0usize; 6];
    count(node, &mut c);
    let words = ["fn", "struct", "enum", "trait", "impl", "mod"];
    c.iter()
        .zip(words)
        .filter(|(&n, _)| n > 0)
        .map(|(&n, w)| plural(n, w))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The full inventory line (spec §4.3): kind counts + churn readout,
/// e.g. "4 fns · 2 structs · 480L · 47 commits · p96".
pub fn inventory(node: &SymbolNode) -> String {
    let kinds = kind_counts(node);
    if kinds.is_empty() {
        churn_readout(node)
    } else {
        format!("{kinds} · {}", churn_readout(node))
    }
}

/// Non-code body lines by node type and rung — the spec §4.3 content table.
/// Full leaf items return only their signature; the paint path appends the
/// highlighted code (or leaves this Detail-equivalent content when the
/// buffer is unavailable).
pub fn body_lines(node: &SymbolNode, rung: Rung) -> Vec<BodyLine> {
    match rung {
        Rung::Dot | Rung::Label => vec![],
        Rung::Card => vec![BodyLine::Dim(card_meta(node))],
        Rung::Detail | Rung::Full => match node.id.kind {
            SymbolKind::Folder => {
                if rung == Rung::Detail {
                    let mut out = vec![BodyLine::Dim(churn_readout(node))];
                    let kinds = kind_counts(node);
                    if !kinds.is_empty() {
                        out.push(BodyLine::Dim(kinds));
                    }
                    out
                } else {
                    vec![BodyLine::Dim(inventory(node))]
                }
            }
            SymbolKind::File => {
                if rung == Rung::Detail {
                    let mut out = vec![BodyLine::Dim(churn_readout(node))];
                    if let Some(first) = node.doc.as_deref().and_then(|d| d.lines().next()) {
                        out.push(BodyLine::Plain(first.to_string()));
                    }
                    let kinds = kind_counts(node);
                    if !kinds.is_empty() {
                        out.push(BodyLine::Dim(kinds));
                    }
                    out
                } else {
                    let mut out: Vec<BodyLine> = node
                        .doc
                        .as_deref()
                        .map(|d| d.lines().map(|l| BodyLine::Plain(l.to_string())).collect())
                        .unwrap_or_default();
                    out.push(BodyLine::Dim(inventory(node)));
                    out
                }
            }
            _ => {
                let mut out = Vec::new();
                if let Some(sig) = &node.signature {
                    out.push(BodyLine::Plain(sig.clone()));
                }
                if rung == Rung::Full && !node.children.is_empty() {
                    out.push(BodyLine::Dim(inventory(node)));
                }
                out
            }
        },
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider content::`
Expected: PASS (2 tests). As in Task 4, if clippy flags dead code (module not yet wired into treemap), add targeted `#[allow(dead_code)]` and remove them in Task 6 Step 5.

- [ ] **Step 5: Workspace check and commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: PASS, clean.

```bash
git add crates/outrider docs
git commit -m "feat(app): GPUI-free content assembly for Detail/Full bodies and inventories"
```

---

### Task 6: Painting — syntax palette, body lines, windowed code

**Files:**
- Modify: `crates/outrider/src/theme.rs`
- Modify: `crates/outrider/src/treemap.rs`

**Interfaces:**
- Consumes: `content::{body_lines, BodyLine}` (Task 5), `buffers::{BufferManager, collect_file_symbols, MAX_BUFFERS is not needed here}` (Task 4), `Rung::{Detail, Full}` + `DrawItem.top` (Task 3), `outrider_index::buffer::{HighlightKind, HighlightSpan}` (Task 2), `SymbolNode.{signature, doc}` (Task 1).
- Produces: `theme::syntax_color(kind: HighlightKind) -> u32`; `treemap::{BodyText, runs_from_spans, code_line, build_body}` (module-private; unit-tested). This is the last task — nothing consumes it except the user's eyes.

- [ ] **Step 1: Syntax palette in theme.rs**

Add to `crates/outrider/src/theme.rs` (top: `use outrider_index::buffer::HighlightKind;`):

```rust
/// Syntax palette for Full-rung code: one color per HighlightKind,
/// legible on BG (0x1a1a1c). Default falls back to TEXT_PRIMARY.
pub fn syntax_color(kind: HighlightKind) -> u32 {
    match kind {
        HighlightKind::Keyword => 0xc586c0,
        HighlightKind::Function => 0xdcdcaa,
        HighlightKind::Type => 0x4ec9b0,
        HighlightKind::String => 0xce9178,
        HighlightKind::Comment => 0x6a9955,
        HighlightKind::Number => 0xb5cea8,
        HighlightKind::Property => 0x9cdcfe,
        HighlightKind::Default => TEXT_PRIMARY,
    }
}
```

and in theme's test module:

```rust
    #[test]
    fn syntax_default_is_text_primary() {
        use outrider_index::buffer::HighlightKind;
        assert_eq!(syntax_color(HighlightKind::Default), TEXT_PRIMARY);
    }
```

- [ ] **Step 2: Write the failing treemap unit tests**

Replace the `#[cfg(test)] mod tests` in `crates/outrider/src/treemap.rs` with (keeps the existing `truncation` test verbatim, adds three):

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    use super::{build_body, code_line, runs_from_spans, truncate_to_width, HEADER, LINE_STEP};
    use crate::buffers::BufferManager;
    use crate::world::{PxRect, Rung};

    #[test]
    fn truncation() {
        // 12 + 10*0.62*12 = wide enough for exactly 10 chars at 12px
        let w = 12.0 + 10.0 * 0.62 * 12.0;
        assert_eq!(truncate_to_width("short.rs", w, 12.0), Some("short.rs".into()));
        assert_eq!(
            truncate_to_width("a_very_long_file_name.rs", w, 12.0),
            Some("a_very_lo…".into())
        );
        assert_eq!(truncate_to_width("anything", 10.0, 12.0), None);
        // multi-byte chars must not panic
        assert_eq!(truncate_to_width("ééééééééééééé", w, 12.0), Some("ééééééééé…".into()));
    }

    fn node(kind: SymbolKind, qual: &str, byte_range: Option<std::ops::Range<usize>>, measure: u64, signature: Option<&str>, doc: Option<&str>) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qual.into(), ordinal: 0 },
            name: qual.to_string(),
            byte_range,
            signature: signature.map(str::to_string),
            doc: doc.map(str::to_string),
            measure,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        }
    }

    #[test]
    fn runs_cover_text_exactly_and_truncate() {
        use outrider_index::buffer::{HighlightKind, HighlightSpan};
        let spans = vec![
            HighlightSpan { range: 0..2, kind: HighlightKind::Keyword },
            HighlightSpan { range: 3..7, kind: HighlightKind::Function },
        ];
        let runs = runs_from_spans(10, &spans);
        assert_eq!(runs.iter().map(|r| r.0).sum::<usize>(), 10);
        assert_eq!(runs.len(), 4); // keyword, gap, function, tail
        // truncated code line: run lengths still cover the shown bytes exactly
        let w = 12.0 + 5.0 * 0.62 * 12.0; // 5-char budget at 12px
        let (shown, runs) = code_line("fn frobnicate()", &spans, w, 12.0).unwrap();
        assert_eq!(shown, "fn f…");
        assert_eq!(runs.iter().map(|r| r.0).sum::<usize>(), shown.len());
        // too narrow for any text → no line
        assert!(code_line("fn x()", &spans, 10.0, 12.0).is_none());
    }

    #[test]
    fn build_body_positions_detail_lines() {
        let f = node(SymbolKind::File, "a.rs", Some(0..24), 2, None, Some("Doc line."));
        let px = PxRect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let mut mgr = BufferManager::new(std::path::PathBuf::from("/nonexistent"));
        let body = build_body(&f, Rung::Detail, &px, 0.0, 600.0, &mut mgr, &BTreeMap::new());
        // churn readout + doc first line (no items → no kind-counts line)
        assert_eq!(body.len(), 2);
        assert_eq!(body[1].text, "Doc line.");
        assert!((f64::from(body[0].y) - HEADER).abs() < 1e-3);
        assert!((f64::from(body[1].y) - (HEADER + LINE_STEP)).abs() < 1e-3);
    }

    #[test]
    fn build_body_full_leaf_appends_windowed_code() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let leaf = node(SymbolKind::Fn, "a.rs::two", Some(12..23), 1, Some("fn two()"), None);
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 12)]);
        let px = PxRect { x: 0.0, y: 0.0, w: 400.0, h: 800.0 };
        let body = build_body(&leaf, Rung::Full, &px, 0.0, 600.0, &mut mgr, &file_symbols);
        // signature row + exactly the symbol's one code line (line-window)
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].text, "fn two()");
        assert_eq!(body[1].text, "fn two() {}");
        assert!(body[1].runs.len() > 1, "code rows carry colored runs");
        assert_eq!(body[1].runs.iter().map(|r| r.0).sum::<usize>(), body[1].text.len());
        assert!((f64::from(body[1].y) - (HEADER + LINE_STEP)).abs() < 1e-3);
        // buffer unavailable → Detail-equivalent content (signature, no code)
        let mut broken = BufferManager::new(std::path::PathBuf::from("/nonexistent"));
        let body = build_body(&leaf, Rung::Full, &px, 0.0, 600.0, &mut broken, &BTreeMap::new());
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].text, "fn two()");
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider treemap::`
Expected: FAIL to compile — `build_body`, `code_line`, `runs_from_spans`, `HEADER`, `LINE_STEP` missing.

- [ ] **Step 4: Implement the treemap changes**

All in `crates/outrider/src/treemap.rs`.

1. Imports — replace the two `use outrider_index…`/`use crate…` groups with:

```rust
use std::collections::BTreeMap;

use outrider_index::buffer::HighlightSpan;
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::WorldLayout;

use crate::buffers::{collect_file_symbols, BufferManager};
use crate::camera::{self, Camera, CameraTween};
use crate::content::{self, BodyLine};
use crate::focus::{Focus, TreeIndex};
use crate::theme;
use crate::world::{self, Rung};
```

2. Layout constants, below the imports (HEADER matches the old Card meta offset `y + 4 + font·1.4`; LINE_STEP matches the existing shaped line height `font·1.3`):

```rust
const FONT_PX: f64 = 12.0;
const LINE_STEP: f64 = FONT_PX * 1.3;
/// Name-row height: text top padding (4) plus one meta-line offset.
const HEADER: f64 = 4.0 + FONT_PX * 1.4;
```

3. `TreemapView` gains two fields (after `focus_handle`):

```rust
    buffers: BufferManager,
    file_symbols: BTreeMap<String, Vec<(SymbolId, usize)>>,
```

and `new` initializes them before constructing `Self` (order matters — `collect_file_symbols` borrows `tree` before it moves):

```rust
    pub fn new(tree: SymbolTree, layout: WorldLayout, cx: &mut Context<Self>) -> Self {
        let root_id = tree.root.id.clone();
        let file_symbols = collect_file_symbols(&tree);
        let buffers = BufferManager::new(tree.repo_root.clone());
        Self {
            tree,
            layout,
            camera: None,
            home_zoom: 1.0,
            drag_last: None,
            press_origin: None,
            focus: Focus::new(root_id),
            tween: None,
            focus_handle: cx.focus_handle(),
            buffers,
            file_symbols,
        }
    }
```

4. `PaintItem`: replace `meta: String` with `body: Vec<BodyText>`, and add:

```rust
/// One shaped body line: canvas y plus full-coverage (byte len, color) runs.
struct BodyText {
    y: f32,
    text: String,
    runs: Vec<(usize, u32)>,
}
```

5. Free helper functions (place before `impl TreemapView`):

```rust
/// Full-coverage colored runs for the first `len` bytes of a line from its
/// highlight spans; gaps paint TEXT_PRIMARY. Run lengths sum exactly to `len`.
fn runs_from_spans(len: usize, spans: &[HighlightSpan]) -> Vec<(usize, u32)> {
    let mut runs = Vec::new();
    let mut pos = 0;
    for s in spans {
        let start = s.range.start.min(len);
        let end = s.range.end.min(len);
        if start > pos {
            runs.push((start - pos, theme::TEXT_PRIMARY));
        }
        if end > start {
            runs.push((end - start, theme::syntax_color(s.kind)));
        }
        pos = pos.max(end);
    }
    if pos < len {
        runs.push((len - pos, theme::TEXT_PRIMARY));
    }
    runs
}

/// Truncate a code line to the box width, clipping its runs to the kept
/// bytes; a trailing ellipsis paints TEXT_PRIMARY.
fn code_line(
    text: &str,
    spans: &[HighlightSpan],
    w: f32,
    font_px: f32,
) -> Option<(String, Vec<(usize, u32)>)> {
    let shown = truncate_to_width(text, w, font_px)?;
    let truncated = shown != text;
    let kept = if truncated { shown.len() - '…'.len_utf8() } else { shown.len() };
    let mut runs = runs_from_spans(kept, spans);
    if truncated {
        runs.push(('…'.len_utf8(), theme::TEXT_PRIMARY));
    }
    Some((shown, runs))
}

/// Body content for one box: content-table lines anchored to the CLIPPED
/// top (they pin like the name row), then — for Full leaf items — the
/// symbol's highlighted code laid out from the UNCLIPPED top and
/// line-window culled to the viewport (spec §4.4). Rows that would sit
/// under the pinned name/signature block or off-screen are skipped.
fn build_body(
    node: &SymbolNode,
    rung: Rung,
    px: &world::PxRect,
    top: f64,
    vh: f64,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> Vec<BodyText> {
    if rung == Rung::Dot || rung == Rung::Label {
        return Vec::new();
    }
    let mut out = Vec::new();
    let lines = content::body_lines(node, rung);
    let rows = lines.len();
    for (k, line) in lines.into_iter().enumerate() {
        let y = px.y + HEADER + k as f64 * LINE_STEP;
        if y + LINE_STEP > px.y + px.h || y > vh {
            break;
        }
        let (text, color) = match line {
            BodyLine::Plain(t) => (t, theme::TEXT_PRIMARY),
            BodyLine::Dim(t) => (t, theme::TEXT_SECONDARY),
        };
        if let Some(shown) = truncate_to_width(&text, px.w as f32, FONT_PX as f32) {
            let len = shown.len();
            out.push(BodyText { y: y as f32, text: shown, runs: vec![(len, color)] });
        }
    }
    let is_leaf_item = node.byte_range.is_some()
        && node.children.is_empty()
        && !matches!(node.id.kind, SymbolKind::File | SymbolKind::Folder);
    if rung == Rung::Full && is_leaf_item {
        let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
        let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
        if let Some(m) = buffers.get(&rel, syms) {
            if let Some(start) = m.symbol_start_line(&node.id) {
                let count = (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
                let code_y0 = top + HEADER + rows as f64 * LINE_STEP;
                let min_y = px.y + HEADER + rows as f64 * LINE_STEP - 0.5;
                let max_y = (px.y + px.h).min(vh) - LINE_STEP;
                for j in 0..count {
                    let y = code_y0 + j as f64 * LINE_STEP;
                    if y < min_y {
                        continue;
                    }
                    if y > max_y {
                        break;
                    }
                    if let Some((text, spans)) = m.buffer.line(start + j) {
                        if let Some((shown, runs)) =
                            code_line(&text, spans, px.w as f32, FONT_PX as f32)
                        {
                            out.push(BodyText { y: y as f32, text: shown, runs });
                        }
                    }
                }
            }
        }
    }
    out
}
```

6. `paint_items`: replace the `.into_iter().map(…)` pipeline with a loop that calls `build_body` (disjoint field borrows: `items` borrows `self.tree`, `build_body` takes `&mut self.buffers`):

```rust
        let camera = *self.camera.as_ref().unwrap();
        let focus_id = self.focus.current.clone();
        let items = world::visible_nodes(&self.tree, &self.layout, &camera, vw, vh);
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let f = theme::churn_fill(item.node.churn);
            let body = build_body(
                item.node,
                item.rung,
                &item.px,
                item.top,
                vh,
                &mut self.buffers,
                &self.file_symbols,
            );
            out.push(PaintItem {
                x: item.px.x as f32,
                y: item.px.y as f32,
                w: item.px.w as f32,
                h: item.px.h as f32,
                fill: f,
                border: theme::border_for(f),
                focused: item.node.id == focus_id,
                rung: item.rung,
                name: item.node.name.clone(),
                body,
            });
        }
        out
```

7. Canvas closure: delete the whole `if item.rung == Rung::Card { … }` meta block and put in its place (the `run` closure and `line_height` are already in scope; Card meta now arrives via `body`):

```rust
                            for bt in &item.body {
                                if bt.text.is_empty() {
                                    continue;
                                }
                                let runs: Vec<TextRun> =
                                    bt.runs.iter().map(|&(len, color)| run(len, color)).collect();
                                let line = window.text_system().shape_line(
                                    bt.text.clone().into(),
                                    px(font_px),
                                    &runs,
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(item.x + 6.0), origin.y + px(bt.y)),
                                    line_height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
                            }
```

The name-line painting (including the Label centering branch and the `Dot || h < 14` skip) stays exactly as is.

- [ ] **Step 5: Run to verify pass; remove temporary allows**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider`
Expected: PASS (all treemap/world/camera/focus/buffers/content tests). Now delete any `#[allow(dead_code)]` added in Tasks 4–5 and re-run `cargo clippy --workspace -- -D warnings` — everything is wired, so it must be clean without them.

- [ ] **Step 6: Full workspace verification and build**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo build`
Expected: all PASS, clean, binary builds.

- [ ] **Step 7: Commit**

```bash
git add crates/outrider docs
git commit -m "feat(app): paint Detail/Full content — summaries, signatures, windowed highlighted code"
```

---

## Final verification (whole branch)

- `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo build`
- Manual exit gate (spec §6, run by the user, not the implementer): `cargo run -- .` on Outrider's own repo — `End` on a method fills the box with highlighted code; arrow-stepping between methods at Full reads as moving through the code; file/folder summaries appear on the way down. Bet #1's complete verdict goes in the ledger.

## Out of scope

Enter/Esc descend (Phase 5), live reload / real `remap` invocation (Phase 6), editing, LLM text, non-Rust highlighting (see spec §7).



