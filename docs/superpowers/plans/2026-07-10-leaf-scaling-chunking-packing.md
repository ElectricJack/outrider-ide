# Leaf-Page Scaling, File Chunking & Square Packing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make leaf pages scale uniformly with their box, split large text files into ordered chunk sub-pages, render a far-zoom minimap that resolves into live text, and pack containers column-first toward square.

**Architecture:** Four coupled changes to leaf-page rendering/layout. `outrider-index` gains a pure `chunk.rs` strategy layer and a `SymbolKind::Chunk` node injected by `scan.rs`; `buffer.rs` precomputes per-line minimap rows. `outrider`'s `content.rs`/`world.rs`/`treemap.rs` replace the clamped `code_scale` with uniform per-page scaling and a `LeafDraw` LOD ladder (Dot/Label/Minimap/Text). `outrider-layout`'s `pack.rs` fills columns top-to-bottom.

**Tech Stack:** Rust workspace (`outrider-index`, `outrider-layout`, `outrider` bin), GPUI (pinned rev, confined to `main.rs`/`treemap.rs`/`chrome.rs`), tree-sitter, ropey, tempfile (dev).

## Global Constraints

- Every `cargo` command needs the PATH prefix: `export PATH="$HOME/.cargo/bin:$PATH" && <cargo ...>`.
- Standing gate after every task: `cargo test --workspace` green AND `cargo clippy --workspace --all-targets -- -D warnings` clean.
- GPUI stays confined to `main.rs`, `treemap.rs`, `chrome.rs`. `content.rs`, `world.rs`, `chunk.rs`, `buffer.rs`, `pack.rs` remain GPUI-free and headless-testable.
- Constants (verbatim): `FONT_PX = 12.0`, `LINE_STEP = 15.6`, `HEADER = 20.8`, `BOTTOM_PAD = 6.0`, `MERGE_PX = 4.0`, `LABEL_PX = 20.0`, `CARD_PX = 80.0`, `DETAIL_PX = 250.0`, `FULL_PX = 700.0`, `CODE_MIN_W = 300.0`, `LABEL_MIN_W = 60.0`, `PAGE_W = 480.0`, `PACK_GAP = 8.0`, `MAX_ZOOM = 8.0`. New: `MIN_TEXT_FONT_PX = 7.0`, `CHAR_ADV = 7.2` (= 0.6·FONT_PX), `CHUNK_MAX_LINES = 60`, `PACK_ASPECT = 1.0` (was 1.6).
- `SymbolKind::Chunk` is appended after `Fn` (last variant) so `Ord`/serialized values of existing variants are unchanged.
- Chunk node fields at build time: `kind: Chunk`, `qualified_path: "{file_qual}#{i}"`, `name: chunk.label`, `byte_range: Some(start_byte..end_byte)`, `signature: None`, `doc: None`, `measure: (end_line-start_line) as u64`, `churn: 0.0`, `churn_count: 0`, `children: vec![]`. (Final chunk churn is inherited from the file by `churn::annotate`; the `0.0` is only the initializer.)
- Row geometry is shared between text and minimap: row `r` (0-based, row 0 = header) sits at `y = top + (HEADER + r·LINE_STEP)·scale` where `scale = ph/natural_px`. Code line `j` is row `1 + rows_of_body + j`.

---

### Task 1: Chunk strategy layer (`outrider-index/src/chunk.rs`)

**Files:**
- Create: `crates/outrider-index/src/chunk.rs`
- Modify: `crates/outrider-index/src/lib.rs:1-7` (add `pub mod chunk;`)
- Test: inline `#[cfg(test)]` in `chunk.rs`

**Interfaces:**
- Consumes: nothing (pure `&str` functions).
- Produces:
  - `pub struct Chunk { pub start_line: usize, pub end_line: usize, pub start_byte: usize, pub end_byte: usize, pub label: String }` (start_line inclusive, end_line exclusive, both 0-based).
  - `pub trait ChunkStrategy { fn chunks(&self, text: &str) -> Vec<Chunk>; }`
  - `pub struct LineChunker;` and `pub struct MarkdownChunker;` implementing `ChunkStrategy`.
  - `pub fn strategy_for(ext: &str) -> Box<dyn ChunkStrategy>;`
  - `pub const CHUNK_MAX_LINES: usize = 60;`

- [ ] **Step 1: Write the failing tests**

Create `crates/outrider-index/src/chunk.rs` with only the test module first (it will not compile until the impl exists — that is the failing state):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Build "L1\nL2\n...Ln\n" with n lines.
    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("L{i}\n")).collect()
    }

    fn assert_contiguous(chunks: &[Chunk], text: &str) {
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].start_line, 0);
        assert_eq!(chunks[0].start_byte, 0);
        assert_eq!(chunks.last().unwrap().end_byte, text.len());
        for w in chunks.windows(2) {
            assert_eq!(w[0].end_line, w[1].start_line);
            assert_eq!(w[0].end_byte, w[1].start_byte);
        }
    }

    #[test]
    fn line_chunker_short_file_is_one_chunk() {
        let text = numbered(3);
        let cs = LineChunker.chunks(&text);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].label, "1–3");
        assert_eq!((cs[0].start_line, cs[0].end_line), (0, 3));
        assert_contiguous(&cs, &text);
    }

    #[test]
    fn line_chunker_splits_into_60_line_slices() {
        let text = numbered(150);
        let cs = LineChunker.chunks(&text);
        let labels: Vec<&str> = cs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["1–60", "61–120", "121–150"]);
        assert_eq!(
            cs.iter().map(|c| (c.start_line, c.end_line)).collect::<Vec<_>>(),
            vec![(0, 60), (60, 120), (120, 150)]
        );
        assert_contiguous(&cs, &text);
    }

    #[test]
    fn markdown_chunker_splits_at_headings_with_heading_labels() {
        let text = "# Alpha\none\ntwo\n## Beta\nthree\n";
        let cs = MarkdownChunker.chunks(text);
        let labels: Vec<&str> = cs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["Alpha", "Beta"]);
        assert_eq!(
            cs.iter().map(|c| (c.start_line, c.end_line)).collect::<Vec<_>>(),
            vec![(0, 3), (3, 5)]
        );
        assert_contiguous(&cs, text);
    }

    #[test]
    fn markdown_chunker_merges_preamble_into_first_chunk() {
        let text = "intro line\n# Alpha\nbody\n## Beta\nend\n";
        let cs = MarkdownChunker.chunks(text);
        // preamble (line 0) merges with the Alpha section: one chunk 0..3,
        // then Beta 3..5. The merged first chunk begins on a non-heading
        // line, so it uses the range label.
        assert_eq!(cs.len(), 2);
        assert_eq!((cs[0].start_line, cs[0].end_line), (0, 3));
        assert_eq!(cs[0].label, "1–3");
        assert_eq!(cs[1].label, "Beta");
        assert_contiguous(&cs, text);
    }

    #[test]
    fn markdown_chunker_splits_long_section_at_a_blank_line() {
        // Heading + 65 non-blank lines + blank + tail. The blank at index 66
        // is the first blank once the running chunk exceeds 60 lines.
        let mut text = String::from("# Head\n");
        for _ in 0..65 {
            text.push_str("x\n");
        }
        text.push('\n'); // blank line, index 66
        text.push_str("tail\n");
        let cs = MarkdownChunker.chunks(&text);
        assert!(cs.len() >= 2, "expected a blank-line split, got {}", cs.len());
        // no chunk boundary lands inside the paragraph of x's (lines 1..=65)
        for c in &cs {
            assert!(
                c.start_line == 0 || c.start_line >= 66,
                "boundary at {} broke the paragraph",
                c.start_line
            );
        }
        assert_contiguous(&cs, &text);
    }

    #[test]
    fn markdown_chunker_short_doc_is_one_chunk() {
        let text = "just a paragraph\nwith two lines\n";
        let cs = MarkdownChunker.chunks(text);
        assert_eq!(cs.len(), 1);
        assert_contiguous(&cs, text);
    }

    #[test]
    fn strategy_for_selects_by_extension() {
        // 150 non-markdown lines chunk by slices; a markdown file with no
        // headings/blank-splits stays one chunk.
        assert_eq!(strategy_for("txt").chunks(&numbered(150)).len(), 3);
        assert_eq!(strategy_for("rs").chunks(&numbered(150)).len(), 3);
        let md = "# A\ntext\n# B\nmore\n";
        assert_eq!(strategy_for("md").chunks(md).len(), 2);
        assert_eq!(strategy_for("markdown").chunks(md).len(), 2);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index chunk:: 2>&1 | head -30`
Expected: FAIL to compile — `cannot find type Chunk`, `LineChunker`, etc.

- [ ] **Step 3: Write the implementation**

Prepend the implementation above the test module in `crates/outrider-index/src/chunk.rs`:

```rust
//! Pluggable file chunking: split an over-threshold text file into ordered,
//! contiguous, covering sub-pages. Pure functions of `&str` — no GPUI, no
//! filesystem.

/// One contiguous slice of a file. `start_line`/`end_line` are 0-based
/// (start inclusive, end exclusive); `start_byte`/`end_byte` cover the same
/// span including each line's trailing newline, so adjacent chunks meet
/// exactly and the last chunk ends at `text.len()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub label: String,
}

pub trait ChunkStrategy {
    /// Ordered, contiguous, covering chunks. Returns a single whole-file
    /// chunk when the file is under threshold; the caller treats `len == 1`
    /// as "do not chunk".
    fn chunks(&self, text: &str) -> Vec<Chunk>;
}

/// Soft cap / slice size in lines.
pub const CHUNK_MAX_LINES: usize = 60;

/// `"md" | "markdown"` → semantic Markdown splits; everything else → line
/// slices.
pub fn strategy_for(ext: &str) -> Box<dyn ChunkStrategy> {
    match ext {
        "md" | "markdown" => Box::new(MarkdownChunker),
        _ => Box::new(LineChunker),
    }
}

/// (start_byte, end_byte) of each line, newline included in end_byte. Matches
/// `buffer::line_bounds` line counting: a trailing newline does not add an
/// empty final line, and `""` yields zero lines.
fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for seg in text.split_inclusive('\n') {
        out.push((start, start + seg.len()));
        start += seg.len();
    }
    out
}

/// Line content (newline/CR trimmed) for each line.
fn line_contents(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').map(|s| s.trim_end_matches(['\n', '\r'])).collect()
}

/// Build a Chunk over lines `[a, b)` with the given label.
fn chunk_of(spans: &[(usize, usize)], a: usize, b: usize, label: String) -> Chunk {
    Chunk {
        start_line: a,
        end_line: b,
        start_byte: spans[a].0,
        end_byte: spans[b - 1].1,
        label,
    }
}

fn range_label(a: usize, b: usize) -> String {
    format!("{}–{}", a + 1, b) // 1-based inclusive, en dash
}

pub struct LineChunker;

impl ChunkStrategy for LineChunker {
    fn chunks(&self, text: &str) -> Vec<Chunk> {
        let spans = line_spans(text);
        let n = spans.len();
        if n == 0 {
            return Vec::new();
        }
        if n <= CHUNK_MAX_LINES {
            return vec![chunk_of(&spans, 0, n, range_label(0, n))];
        }
        let mut out = Vec::new();
        let mut a = 0;
        while a < n {
            let b = (a + CHUNK_MAX_LINES).min(n);
            out.push(chunk_of(&spans, a, b, range_label(a, b)));
            a = b;
        }
        out
    }
}

pub struct MarkdownChunker;

/// `^\s{0,3}#{1,6}\s` — up to 3 leading spaces, 1–6 `#`, then a space/tab.
fn is_heading(line: &str) -> bool {
    let leading = line.len() - line.trim_start_matches(' ').len();
    if leading > 3 {
        return false;
    }
    let rest = &line[leading..];
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    rest[hashes..].starts_with([' ', '\t'])
}

/// Heading text with markers and surrounding whitespace stripped.
fn heading_label(line: &str) -> String {
    line.trim_start_matches(' ').trim_start_matches('#').trim().to_string()
}

impl ChunkStrategy for MarkdownChunker {
    fn chunks(&self, text: &str) -> Vec<Chunk> {
        let spans = line_spans(text);
        let content = line_contents(text);
        let n = spans.len();
        if n == 0 {
            return Vec::new();
        }
        // Chunk-start line indices.
        let mut bounds = vec![0usize];
        let mut cur_start = 0usize;
        // Whether the current chunk has already started on/absorbed a heading;
        // lets a leading preamble merge into the first heading's section.
        let mut seen_heading = is_heading(content[0]);
        for i in 1..n {
            let start_new = if is_heading(content[i]) {
                if seen_heading {
                    true
                } else {
                    seen_heading = true; // merge preamble into this section
                    false
                }
            } else {
                content[i].trim().is_empty() && (i - cur_start) >= CHUNK_MAX_LINES
            };
            if start_new {
                bounds.push(i);
                cur_start = i;
                seen_heading = is_heading(content[i]);
            }
        }
        bounds
            .iter()
            .enumerate()
            .map(|(k, &a)| {
                let b = bounds.get(k + 1).copied().unwrap_or(n);
                let label = if is_heading(content[a]) {
                    heading_label(content[a])
                } else {
                    range_label(a, b)
                };
                chunk_of(&spans, a, b, label)
            })
            .collect()
    }
}
```

- [ ] **Step 4: Wire the module**

In `crates/outrider-index/src/lib.rs`, add after `pub mod buffer;` (keep alphabetical-ish order used in the file):

```rust
pub mod chunk;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index chunk::`
Expected: PASS (7 tests).

- [ ] **Step 6: Clippy + commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy -p outrider-index --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/outrider-index/src/chunk.rs crates/outrider-index/src/lib.rs
git commit -m "feat(index): pluggable file chunking (line + markdown strategies)"
```

---

### Task 2: `SymbolKind::Chunk` variant and content arms (`types.rs`, `content.rs`)

**Files:**
- Modify: `crates/outrider-index/src/types.rs:7-17` (append `Chunk`)
- Modify: `crates/outrider/src/content.rs:89-105` (kind_counts) and `:133` (body_lines match)
- Test: inline in `content.rs`

**Interfaces:**
- Consumes: `SymbolKind` from Task nothing (existing enum).
- Produces: `SymbolKind::Chunk` (last variant); `kind_counts` counts `Chunk` children as `"{n} parts"`; `body_lines(chunk, Detail|Full)` = `[BodyLine::Dim(churn_readout(chunk))]`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/outrider/src/content.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn chunked_file_counts_parts_and_chunk_body_is_one_readout() {
        use BodyLine::Dim;
        // A File container whose children are Chunk nodes.
        let mut file = node(SymbolKind::File, "README.md", 120, 0.2, 5, None, None, vec![
            node(SymbolKind::Chunk, "README.md#0", 60, 0.2, 5, None, None, vec![]),
            node(SymbolKind::Chunk, "README.md#1", 60, 0.2, 5, None, None, vec![]),
        ]);
        file.byte_range = Some(0..1000);
        assert_eq!(kind_counts(&file), "2 parts");
        assert_eq!(inventory(&file), "2 parts · 120L · 5 commits · p20");
        // a single-part edge still pluralizes correctly
        let one = node(SymbolKind::File, "x.txt", 60, 0.0, 0, None, None, vec![
            node(SymbolKind::Chunk, "x.txt#0", 60, 0.0, 0, None, None, vec![]),
        ]);
        assert_eq!(kind_counts(&one), "1 part");
        // a Chunk leaf's Full body is exactly its churn readout row
        let chunk = &file.children[0];
        assert_eq!(body_lines(chunk, Rung::Full), vec![Dim("60L · 5 commits · p20".into())]);
        assert_eq!(body_lines(chunk, Rung::Detail), vec![Dim("60L · 5 commits · p20".into())]);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider content:: 2>&1 | head -30`
Expected: FAIL to compile — `no variant named Chunk`.

- [ ] **Step 3: Append the enum variant**

In `crates/outrider-index/src/types.rs`, the `SymbolKind` enum becomes (append `Chunk` last):

```rust
pub enum SymbolKind {
    Folder,
    File,
    Module,
    Struct,
    Enum,
    Trait,
    Impl,
    Fn,
    Chunk,
}
```

- [ ] **Step 4: Fix the exhaustive `kind_counts` match and add the Chunk body arm**

In `crates/outrider/src/content.rs`, widen the count array to 7 slots and add the `Chunk` arm. Replace the `count` fn and its caller (lines 89-111) with:

```rust
    fn count(node: &SymbolNode, c: &mut [usize; 7]) {
        for k in &node.children {
            match k.id.kind {
                SymbolKind::Fn => c[0] += 1,
                SymbolKind::Struct => c[1] += 1,
                SymbolKind::Enum => c[2] += 1,
                SymbolKind::Trait => c[3] += 1,
                SymbolKind::Impl => c[4] += 1,
                SymbolKind::Module => c[5] += 1,
                SymbolKind::Chunk => c[6] += 1,
                SymbolKind::File | SymbolKind::Folder => {}
            }
            count(k, c);
        }
    }
    let mut c = [0usize; 7];
    count(node, &mut c);
    let words = ["fn", "struct", "enum", "trait", "impl", "mod", "part"];
```

In the `body_lines` `Rung::Detail | Rung::Full => match node.id.kind {` block, add a `Chunk` arm before the `_ =>` arm (after the `SymbolKind::File => {...}` arm at line 170):

```rust
            SymbolKind::Chunk => vec![BodyLine::Dim(churn_readout(node))],
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider content:: && cargo test -p outrider-index`
Expected: PASS (new test green; existing content + index tests unaffected).

- [ ] **Step 6: Clippy + commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/outrider-index/src/types.rs crates/outrider/src/content.rs
git commit -m "feat: SymbolKind::Chunk variant with parts count and readout body"
```

---
### Task 3: Chunk injection in the tree (`scan.rs`)

**Files:**
- Modify: `crates/outrider-index/src/scan.rs:70-160` (thread `repo_root` into `build_folder`; inject chunks)
- Test: inline in `scan.rs`

**Interfaces:**
- Consumes: `chunk::{strategy_for, CHUNK_MAX_LINES, Chunk}` (Task 1); `SymbolKind::Chunk` (Task 2).
- Produces: a childless File node with `lines > CHUNK_MAX_LINES` whose on-disk text yields >1 chunk becomes a File **container** of `Chunk` children in source order; other files unchanged.

- [ ] **Step 1: Write the failing test**

Add to `crates/outrider-index/src/scan.rs` (create a `#[cfg(test)] mod tests` at the end of the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SymbolKind;

    fn scan_tree(dir: &std::path::Path) -> SymbolTree {
        let files = scan_files(dir).unwrap();
        build_tree(dir, &files, &BTreeMap::new())
    }

    fn child<'a>(root: &'a SymbolNode, name: &str) -> &'a SymbolNode {
        root.children.iter().find(|c| c.name == name).expect("child present")
    }

    #[test]
    fn large_markdown_file_becomes_a_chunk_container() {
        let dir = tempfile::tempdir().unwrap();
        // 3 headed sections, each long enough to be its own chunk.
        let mut text = String::new();
        for h in ["Alpha", "Beta", "Gamma"] {
            text.push_str(&format!("# {h}\n"));
            for i in 0..25 {
                text.push_str(&format!("line {i}\n"));
            }
        }
        std::fs::write(dir.path().join("BIG.md"), &text).unwrap();
        let tree = scan_tree(dir.path());
        let f = child(&tree.root, "BIG.md");
        assert_eq!(f.id.kind, SymbolKind::File);
        assert_eq!(f.children.len(), 3);
        assert!(f.children.iter().all(|c| c.id.kind == SymbolKind::Chunk));
        // byte ranges are contiguous and cover the whole file, in source order
        let mut sorted: Vec<&SymbolNode> = f.children.iter().collect();
        sorted.sort_by_key(|c| c.byte_range.as_ref().unwrap().start);
        assert_eq!(sorted[0].byte_range.as_ref().unwrap().start, 0);
        assert_eq!(sorted.last().unwrap().byte_range.as_ref().unwrap().end, text.len());
        for w in sorted.windows(2) {
            assert_eq!(w[0].byte_range.as_ref().unwrap().end, w[1].byte_range.as_ref().unwrap().start);
        }
        // chunk qualified_path is "{file}#{i}"
        assert!(f.children.iter().all(|c| c.id.qualified_path.starts_with("BIG.md#")));
    }

    #[test]
    fn small_file_stays_a_single_page() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("small.txt"), "one\ntwo\nthree\n").unwrap();
        let tree = scan_tree(dir.path());
        let f = child(&tree.root, "small.txt");
        assert!(f.children.is_empty(), "under threshold: not chunked");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index scan:: 2>&1 | head -30`
Expected: FAIL — `large_markdown_file_becomes_a_chunk_container` sees `f.children.is_empty()` (no injection yet).

- [ ] **Step 3: Thread `repo_root` and inject chunks**

In `crates/outrider-index/src/scan.rs`, add imports near the top (after the existing `use crate::types::...`):

```rust
use crate::chunk::{strategy_for, CHUNK_MAX_LINES};
```

Change `build_tree`'s call (line 91) to pass `repo_root`:

```rust
    let root = build_folder(repo_root, &root_name, "", &decomposed, rs_children);
```

Change `build_folder`'s signature (line 98) to take `repo_root: &Path` first, and recurse with it (line 140):

```rust
fn build_folder(
    repo_root: &Path,
    name: &str,
    qualified: &str,
    entries: &[(Vec<String>, &ScannedFile)],
    rs_children: &BTreeMap<PathBuf, ParsedFile>,
) -> SymbolNode {
```

```rust
    for (folder_name, sub_entries) in &by_subfolder {
        let qual = join_path(qualified, folder_name);
        children.push(build_folder(repo_root, folder_name, &qual, sub_entries, rs_children));
    }
```

Replace the `[file_name]` match arm (lines 109-127) so a childless over-threshold file is chunked:

```rust
            [file_name] => {
                let qual = join_path(qualified, file_name);
                let parsed = rs_children.get(&file.rel_path).cloned().unwrap_or_default();
                let mut node = SymbolNode {
                    id: SymbolId {
                        kind: SymbolKind::File,
                        qualified_path: qual.clone(),
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
                };
                if node.children.is_empty() && file.lines > CHUNK_MAX_LINES as u64 {
                    if let Ok(text) = std::fs::read_to_string(repo_root.join(&file.rel_path)) {
                        let ext = file
                            .rel_path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("");
                        let chunks = strategy_for(ext).chunks(&text);
                        if chunks.len() > 1 {
                            node.children = chunks
                                .iter()
                                .enumerate()
                                .map(|(i, ch)| SymbolNode {
                                    id: SymbolId {
                                        kind: SymbolKind::Chunk,
                                        qualified_path: format!("{qual}#{i}"),
                                        ordinal: 0,
                                    },
                                    name: ch.label.clone(),
                                    byte_range: Some(ch.start_byte..ch.end_byte),
                                    signature: None,
                                    doc: None,
                                    measure: (ch.end_line - ch.start_line) as u64,
                                    churn: 0.0,
                                    churn_count: 0,
                                    children: vec![],
                                })
                                .collect();
                            finalize_children(&mut node.children);
                        }
                    }
                }
                children.push(node);
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index`
Expected: PASS (new scan tests green; existing index tests unaffected).

- [ ] **Step 5: Clippy + commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/outrider-index/src/scan.rs
git commit -m "feat(index): inject Chunk children for over-threshold text files"
```

---

### Task 4: Verify chunk anchors (`buffers.rs`, test only)

**Files:**
- Test: `crates/outrider/src/buffers.rs` (add one test; no production change)

**Interfaces:**
- Consumes: `collect_file_symbols` (existing). A chunked File is non-empty, so the existing `items(node)` path already emits one `(chunk.id, chunk.byte_range.start)` per chunk. This task proves it.

- [ ] **Step 1: Write the test**

Add to `crates/outrider/src/buffers.rs` `#[cfg(test)] mod tests` (reuse the local `node` helper pattern already in that module — define a fresh one inside the test):

```rust
    #[test]
    fn collect_file_symbols_anchors_each_chunk_at_its_start() {
        fn node(kind: SymbolKind, qual: &str, byte_range: Option<std::ops::Range<usize>>, children: Vec<SymbolNode>) -> SymbolNode {
            SymbolNode {
                id: SymbolId { kind, qualified_path: qual.into(), ordinal: 0 },
                name: qual.rsplit(['#', ':']).next().unwrap_or(qual).to_string(),
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
                    "BIG.md",
                    Some(0..300),
                    vec![
                        node(SymbolKind::Chunk, "BIG.md#0", Some(0..100), vec![]),
                        node(SymbolKind::Chunk, "BIG.md#1", Some(100..300), vec![]),
                    ],
                )],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        };
        let map = collect_file_symbols(&tree);
        let got: Vec<(&str, usize)> = map
            .get("BIG.md")
            .unwrap()
            .iter()
            .map(|(id, s)| (id.qualified_path.as_str(), *s))
            .collect();
        assert_eq!(got, vec![("BIG.md#0", 0), ("BIG.md#1", 100)]);
    }
```

Ensure `use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};` is in scope for the test module (the module already imports `SymbolId, SymbolKind, SymbolNode, SymbolTree` at line 111 — confirm and extend if needed).

- [ ] **Step 2: Run to verify it passes immediately (no prod change)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider buffers::`
Expected: PASS — the generic `items()` path already anchors chunks. If it FAILS, `collect_file_symbols` needs the chunk case; do not assume — read the failure and fix minimally.

- [ ] **Step 3: Clippy + commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy -p outrider --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/outrider/src/buffers.rs
git commit -m "test(app): chunk children anchor at their first line"
```

---

### Task 5: Minimap rows (`outrider-index/src/buffer.rs`)

**Files:**
- Modify: `crates/outrider-index/src/buffer.rs` (add `MinimapRow`, cache field, `minimap_row`)
- Test: inline in `buffer.rs`

**Interfaces:**
- Consumes: existing `FileBuffer` internals (`lines: Vec<Vec<HighlightSpan>>`, `line_bounds`, `HighlightKind`).
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct MinimapRow { pub indent: u32, pub len: u32, pub kind: HighlightKind }`
  - `pub fn FileBuffer::minimap_row(&self, i: usize) -> MinimapRow` (returns a cached, precomputed row; panics on out-of-range `i`, mirroring slice indexing — callers iterate `0..len_lines()`).

- [ ] **Step 1: Write the failing tests**

Add to `crates/outrider-index/src/buffer.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn minimap_rows_report_indent_len_and_dominant_kind() {
        // line 0: comment; line 1: indented let with a string; line 2: blank
        let text = "// hello world\n    let s = \"xy\";\n\n";
        let buf = FileBuffer::new(text.to_string(), "rs").unwrap();
        assert_eq!(buf.len_lines(), 3);
        let r0 = buf.minimap_row(0);
        assert_eq!(r0.indent, 0);
        assert_eq!(r0.len, "// hello world".chars().count() as u32);
        assert_eq!(r0.kind, HighlightKind::Comment); // whole line is comment
        let r1 = buf.minimap_row(1);
        assert_eq!(r1.indent, 4); // four leading spaces
        assert_eq!(r1.len, "let s = \"xy\";".chars().count() as u32);
        // blank line: no bar
        let r2 = buf.minimap_row(2);
        assert_eq!(r2.len, 0);
        assert_eq!(r2.kind, HighlightKind::Default);
    }

    #[test]
    fn minimap_dominant_kind_breaks_ties_by_first_occurrence() {
        // plain-mode line has no spans → Default
        let buf = FileBuffer::new("abcdef\n".to_string(), "txt").unwrap();
        assert_eq!(buf.minimap_row(0).kind, HighlightKind::Default);
        assert_eq!(buf.minimap_row(0).len, 6);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index buffer::minimap 2>&1 | head -30`
Expected: FAIL to compile — no `minimap_row`.

- [ ] **Step 3: Implement**

In `crates/outrider-index/src/buffer.rs`:

Add the struct after `HighlightSpan` (around line 66):

```rust
/// Cheap per-line texture summary for the far-zoom minimap: leading
/// whitespace width, trimmed visible length, and the dominant highlight
/// kind. Precomputed once at materialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimapRow {
    pub indent: u32,
    pub len: u32,
    pub kind: HighlightKind,
}
```

Add a `minimap` field to `FileBuffer` (after `lines` at line 75):

```rust
    minimap: Vec<MinimapRow>,
```

In `FileBuffer::new`, compute the minimap before moving `text` into the rope. Replace the final `Ok(Self { ... })` (line 114) with:

```rust
        let minimap = compute_minimap(&text, &lines);
        Ok(Self { rope: Rope::from(text), tree, lines, minimap, anchors: AnchorList::default() })
```

Add the accessor in `impl FileBuffer` (near `len_lines`):

```rust
    /// The precomputed minimap summary for line `i`.
    pub fn minimap_row(&self, i: usize) -> MinimapRow {
        self.minimap[i]
    }
```

Add the free functions near `line_bounds` (after line 156):

```rust
/// Dominant highlight kind on a line: the kind covering the most bytes,
/// ties broken by first occurrence; `Default` when the line has no mapped
/// spans (blank or all-default).
fn dominant_kind(spans: &[HighlightSpan]) -> HighlightKind {
    let mut acc: Vec<(HighlightKind, usize)> = Vec::new();
    for s in spans {
        let w = s.range.end - s.range.start;
        if let Some(e) = acc.iter_mut().find(|(k, _)| *k == s.kind) {
            e.1 += w;
        } else {
            acc.push((s.kind, w));
        }
    }
    let mut best: Option<(HighlightKind, usize)> = None;
    for &(k, w) in &acc {
        if best.is_none_or(|(_, bw)| w > bw) {
            best = Some((k, w));
        }
    }
    best.map(|(k, _)| k).unwrap_or(HighlightKind::Default)
}

/// One MinimapRow per line, aligned with `lines` (the per-line spans).
fn compute_minimap(text: &str, lines: &[Vec<HighlightSpan>]) -> Vec<MinimapRow> {
    let bounds = line_bounds(text);
    bounds
        .iter()
        .zip(lines)
        .map(|(b, spans)| {
            let content = &text[b.clone()];
            let indent =
                content.chars().take_while(|&c| c == ' ' || c == '\t').count() as u32;
            let trimmed = content.trim();
            let len = trimmed.chars().count() as u32;
            let kind = if len == 0 { HighlightKind::Default } else { dominant_kind(spans) };
            MinimapRow { indent, len, kind }
        })
        .collect()
}
```

Note: `is_none_or` is stable in the pinned toolchain; if clippy/compile rejects it, use `best.map_or(true, |(_, bw)| w > bw)`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-index buffer::`
Expected: PASS (minimap tests + existing buffer tests green).

- [ ] **Step 5: Clippy + commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/outrider-index/src/buffer.rs
git commit -m "feat(index): precompute per-line minimap rows on FileBuffer"
```

---
### Task 6: Minimap color (`theme.rs`)

**Files:**
- Modify: `crates/outrider/src/theme.rs` (add `minimap_color`)
- Test: inline in `theme.rs`

**Interfaces:**
- Consumes: `syntax_color`, `lerp_rgb` (private, same module), `CODE_BG`.
- Produces: `pub fn minimap_color(kind: HighlightKind) -> u32` = `lerp_rgb(syntax_color(kind), CODE_BG, 0.15)` — the syntax color dimmed 15% toward the page background.

- [ ] **Step 1: Write the failing test**

Add to `crates/outrider/src/theme.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn minimap_color_dims_syntax_toward_code_bg() {
        use outrider_index::buffer::HighlightKind;
        let kw = syntax_color(HighlightKind::Keyword);
        assert_eq!(minimap_color(HighlightKind::Keyword), lerp_rgb(kw, CODE_BG, 0.15));
        assert_eq!(
            minimap_color(HighlightKind::Default),
            lerp_rgb(TEXT_PRIMARY, CODE_BG, 0.15)
        );
        // dimming moves the color: never equal to the full-brightness syntax color
        assert_ne!(minimap_color(HighlightKind::Keyword), kw);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider theme::minimap 2>&1 | head -20`
Expected: FAIL to compile — no `minimap_color`.

- [ ] **Step 3: Implement**

In `crates/outrider/src/theme.rs`, add after `syntax_color` (line 73):

```rust
/// Minimap bar color: the syntax color dimmed toward the page background so
/// the far-zoom minimap reads as texture rather than full-brightness code.
pub fn minimap_color(kind: HighlightKind) -> u32 {
    lerp_rgb(syntax_color(kind), CODE_BG, 0.15)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider theme::`
Expected: PASS.

- [ ] **Step 5: Clippy + commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy -p outrider --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/outrider/src/theme.rs
git commit -m "feat(app): minimap_color dims syntax toward the page background"
```

---

### Task 7: `LeafDraw` LOD function (`content.rs`, `world.rs`) — additive

**Files:**
- Modify: `crates/outrider/src/content.rs` (add `MIN_TEXT_FONT_PX`)
- Modify: `crates/outrider/src/world.rs` (add `LeafDraw`, `leaf_draw`)
- Test: inline in `world.rs`

This task is **purely additive** — it introduces `leaf_draw` alongside the
existing `rung_for` without changing `DrawItem` or the paint path, so the
workspace keeps compiling and the pure function is unit-tested in isolation.
Task 8 wires it into `walk`/`treemap.rs` and removes the superseded code.

**Interfaces:**
- Consumes: `content::FONT_PX`, new `content::MIN_TEXT_FONT_PX`, and the width/height constants in `world.rs`.
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum LeafDraw { Dot, Label, Minimap, Text }`
  - `pub fn leaf_draw(ph: f64, pw: f64, natural_px: f64) -> Option<LeafDraw>` (`None` = merged away).

- [ ] **Step 1: Write the failing tests**

Add to `crates/outrider/src/world.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn leaf_draw_tiers_at_their_boundaries() {
        use LeafDraw::*;
        // merge
        assert_eq!(leaf_draw(3.9, 400.0, 100.0), None);
        // Dot: below LABEL_PX height, or below LABEL_MIN_W width
        assert_eq!(leaf_draw(4.0, 400.0, 100.0), Some(Dot));
        assert_eq!(leaf_draw(19.9, 400.0, 100.0), Some(Dot));
        assert_eq!(leaf_draw(1000.0, 59.9, 100.0), Some(Dot));
        // Label: [LABEL_PX, CARD_PX) height, wide enough
        assert_eq!(leaf_draw(20.0, 400.0, 100.0), Some(Label));
        assert_eq!(leaf_draw(79.9, 400.0, 100.0), Some(Label));
        // Text: font ≥ 7 (ph/natural ≥ 7/12) AND pw ≥ CODE_MIN_W
        assert_eq!(leaf_draw(80.0, 400.0, 100.0), Some(Text)); // font 9.6
        // Minimap: tall page, font sub-7
        assert_eq!(leaf_draw(80.0, 400.0, 200.0), Some(Minimap)); // font 4.8
        // width gate forces Minimap even when font clears 7
        assert_eq!(leaf_draw(80.0, 299.9, 100.0), Some(Minimap));
    }

    #[test]
    fn tall_leaf_steps_minimap_then_text_as_it_grows() {
        use LeafDraw::*;
        let natural = 3000.0; // ~190-line page
        // low zoom: box 200px tall → font 0.8 → Minimap
        assert_eq!(leaf_draw(200.0, 400.0, natural), Some(Minimap));
        // zoom until font ≥ 7 → ph ≥ 7/12·natural = 1750
        assert_eq!(leaf_draw(1750.0, 400.0, natural), Some(Text));
        assert_eq!(leaf_draw(1749.0, 400.0, natural), Some(Minimap));
    }

    #[test]
    fn short_leaf_never_enters_minimap() {
        use LeafDraw::*;
        // natural ≤ ~137 → at CARD_PX height font already ≥ 7, so a short
        // leaf steps Label → Text with no Minimap tier.
        let natural = 100.0;
        assert_eq!(leaf_draw(79.9, 400.0, natural), Some(Label));
        assert_eq!(leaf_draw(80.0, 400.0, natural), Some(Text));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider world::leaf 2>&1 | head -20`
Expected: FAIL to compile — no `LeafDraw`/`leaf_draw`.

- [ ] **Step 3: Add `MIN_TEXT_FONT_PX`**

In `crates/outrider/src/content.rs`, add near `FONT_PX` (after line 11):

```rust
/// Below this on-screen font size a leaf paints its minimap instead of live
/// text (the text/minimap tier boundary).
pub const MIN_TEXT_FONT_PX: f64 = 7.0;
```

- [ ] **Step 4: Add `LeafDraw` and `leaf_draw`**

In `crates/outrider/src/world.rs`, add after the `Rung` enum / `rung_for` block (around line 71):

```rust
/// Draw mode for a leaf page, chosen by on-screen box size (spec §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafDraw {
    Dot,
    Label,
    Minimap,
    Text,
}

/// Leaf LOD ladder. `None` => merged away (below MERGE_PX). First match wins:
/// tiny → Dot, short → Label (pinned name), then Text once the font clears
/// MIN_TEXT_FONT_PX and the column clears CODE_MIN_W, else Minimap.
pub fn leaf_draw(ph: f64, pw: f64, natural_px: f64) -> Option<LeafDraw> {
    if ph < MERGE_PX {
        return None;
    }
    if pw < LABEL_MIN_W || ph < LABEL_PX {
        return Some(LeafDraw::Dot);
    }
    if ph < CARD_PX {
        return Some(LeafDraw::Label);
    }
    let font = content::FONT_PX * ph / natural_px;
    if font >= content::MIN_TEXT_FONT_PX && pw >= CODE_MIN_W {
        Some(LeafDraw::Text)
    } else {
        Some(LeafDraw::Minimap)
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider world::`
Expected: PASS (leaf_draw tests + existing world tests green).

- [ ] **Step 6: Clippy + commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/outrider/src/content.rs crates/outrider/src/world.rs
git commit -m "feat(app): leaf_draw LOD ladder (Dot/Label/Minimap/Text)"
```

---
### Task 8: Rendering integration — uniform scale, minimap paint, draw dispatch (`content.rs`, `world.rs`, `treemap.rs`)

**Files:**
- Modify: `crates/outrider/src/content.rs` (delete `code_scale` + clamp constants + their two tests)
- Modify: `crates/outrider/src/world.rs` (`Draw` enum, `DrawItem.draw`, trimmed `rung_for`, `walk` dispatch, test updates)
- Modify: `crates/outrider/src/treemap.rs` (uniform-scale leaf body, minimap bars, canvas dispatch, test updates)

This is one task because the three files must compile together: deleting
`code_scale` breaks `treemap.rs`, renaming `DrawItem.rung → draw` breaks
`treemap.rs`, and trimming `rung_for` breaks `world.rs`'s own tests. Task 7
already introduced `leaf_draw` additively; this task wires it into the walk
and the paint path and removes the superseded `Rung`-based leaf code path.

**Interfaces:**
- Consumes: `world::leaf_draw`, `world::LeafDraw` (Task 7); `content::natural_px`,
  `content::is_leaf_item`, `content::body_lines`; `buffer::MinimapRow` and
  `FileBuffer::minimap_row` (Task 5); `theme::minimap_color` (Task 6);
  `content::FONT_PX`, `HEADER`, `LINE_STEP`; `world::{PAGE_W, CODE_MIN_W}`.
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq)] pub enum Draw { Container(Rung), Leaf(LeafDraw) }`
  - `pub fn rung_for(px_h: f64, px_w: f64) -> Option<Rung>` (natural_px param removed)
  - `DrawItem { …, pub draw: Draw, … }` (field `rung: Rung` → `draw: Draw`)

- [ ] **Step 1: Delete `code_scale` and the clamp constants from `content.rs`**

In `crates/outrider/src/content.rs`, delete the two constant blocks (old
lines 13–20) — remove exactly:

```rust
/// Floor for scaled code text (spec 4d §4).
pub const MIN_CODE_FONT_PX: f64 = 7.0;
pub const MIN_CODE_SCALE: f64 = MIN_CODE_FONT_PX / FONT_PX;

/// Shortest leaf box that still shows code: header + three code rows at
/// the floor font + bottom pad (≈ 54.1px). Below this a leaf drops to the
/// container ladder (spec 4d §3).
pub const LEAF_CODE_MIN_PX: f64 = HEADER + 3.0 * LINE_STEP * MIN_CODE_SCALE + BOTTOM_PAD;
```

Then delete the `code_scale` function (old lines 38–44) — remove exactly:

```rust
/// Per-box text scale for a Full leaf: 1.0 when the box fits the whole
/// method, shrinking with the box down to the floor, after which the
/// window clips. `px_h` must be the UNCLIPPED box height — the clipped
/// height would wrongly shrink zoomed-in giants (spec 4d §4).
pub fn code_scale(node: &SymbolNode, px_h: f64) -> f64 {
    (px_h / natural_px(node)).clamp(MIN_CODE_SCALE, 1.0)
}
```

Then delete the two now-dead tests `leaf_code_min_px_value` and
`code_scale_clamps_between_floor_and_one` (old lines 396–418) — remove
exactly:

```rust
    #[test]
    fn leaf_code_min_px_value() {
        // HEADER 20.8 + 3·15.6·(7/12) + BOTTOM_PAD 6 = 54.1
        assert!((LEAF_CODE_MIN_PX - 54.1).abs() < 1e-9);
        assert!((MIN_CODE_SCALE - 7.0 / 12.0).abs() < 1e-12);
    }

    #[test]
    fn code_scale_clamps_between_floor_and_one() {
        // measure 3 → natural 89.2 (see natural_px_arithmetic). Compare
        // against natural_px itself, not a decimal literal: n/n is exactly
        // 1.0, while a re-typed 89.2 can land one ulp under and miss the top
        // of the clamp.
        let three = node(SymbolKind::Fn, "a.rs::f", 3, 0.0, 0, Some("fn f()"), None, vec![]);
        let n = natural_px(&three);
        // box fits the whole method (and anything taller): exact 1.0
        assert_eq!(code_scale(&three, n), 1.0);
        assert_eq!(code_scale(&three, 500.0), 1.0);
        // mid value: 80% of natural → 0.8
        assert!((code_scale(&three, 0.8 * n) - 0.8).abs() < 1e-9);
        // tiny box: exact 7/12 floor, after which the window clips
        assert_eq!(code_scale(&three, 10.0), 7.0 / 12.0);
    }
```

(`content.rs` will not compile standalone until Task 8 finishes because
`treemap.rs` still references `code_scale`; that is fixed in Step 4.)

- [ ] **Step 2: Rewrite `rung_for`, add `Draw`, change `DrawItem`, dispatch in `walk` (`world.rs`)**

In `crates/outrider/src/world.rs`, replace the whole `rung_for` function
(old lines 45–71) with the trimmed version — the leaf `natural_px` special
case is gone (leaves use `leaf_draw` now):

```rust
/// Container rung by pixel height, downgraded to Dot when the column is too
/// narrow for text and from Full to Detail when too narrow for code.
/// Heights below MERGE_PX merge into the parent. Leaf items do NOT use this
/// — they go through `leaf_draw` (spec §3).
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

Add the `Draw` enum immediately after the `LeafDraw` enum / `leaf_draw` fn
that Task 7 added:

```rust
/// The chosen draw mode for a visible node: containers keep the `Rung`
/// ladder, leaf pages get a `LeafDraw` tier (spec §3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Draw {
    Container(Rung),
    Leaf(LeafDraw),
}
```

Change the `DrawItem` struct: replace the field `pub rung: Rung,` with
`pub draw: Draw,`.

Rewrite the body of `walk` from the `natural`/`rung_for` block through the
`out.push` so it dispatches on `is_leaf_item`. Replace old lines 130–148:

```rust
    let draw = if content::is_leaf_item(node) {
        match leaf_draw(ph, pw, content::natural_px(node)) {
            Some(ld) => Draw::Leaf(ld),
            None => return, // merged away
        }
    } else {
        match rung_for(ph, pw) {
            Some(r) => Draw::Container(r),
            None => return, // merged away
        }
    };
    // Clip to the viewport (±2px slack keeps borders off-screen) before f32
    // ever sees the coordinates; the draw mode and scale use the UNclipped size.
    let x0 = sx.max(-2.0);
    let x1 = (sx + pw).min(vw + 2.0);
    let y0 = sy.max(-2.0);
    let y1 = (sy + ph).min(vh + 2.0);
    out.push(DrawItem {
        node,
        px: PxRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 },
        label_w: pw,
        level,
        draw,
        top: sy,
        left: sx,
        full_h: ph,
    });
```

- [ ] **Step 3: Update `world.rs` tests to the new API**

In `crates/outrider/src/world.rs` `mod tests`, replace the whole
`rung_for_thresholds_and_downgrade` test (old lines 214–251) with the
version that drops the `natural_px` argument and the leaf-specific cases
(leaf tiers are covered by Task 7's `leaf_draw` tests):

```rust
    #[test]
    fn rung_for_thresholds_and_downgrade() {
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
        // narrow boxes are forced to Dot regardless of height
        assert_eq!(rung_for(100_000.0, 59.9), Some(Rung::Dot));
        // Full downgrades to Detail when too narrow for code (spec §4.2)
        assert_eq!(rung_for(100_000.0, 60.0), Some(Rung::Detail));
        assert_eq!(rung_for(100_000.0, 299.9), Some(Rung::Detail));
        assert_eq!(rung_for(100_000.0, 300.0), Some(Rung::Full));
        // the CODE_MIN_W downgrade applies only to Full
        assert_eq!(rung_for(100.0, 60.0), Some(Rung::Card));
        // the merge rule wins over everything
        assert_eq!(rung_for(3.9, 24.0), None);
    }
```

In `packed_walk_zoom_one_clips_and_keeps_unclipped_fields`, replace the
`rungs` assertion block (old lines 286–289) with a `draws` check. In this
fixture `a.rs` has `measure` but **no `byte_range`** (lines 271–273 only set
`byte_range` on `f` and `g`), so `is_leaf_item(a.rs)` is `false` → it is a
`Container(Full)`. `f` (198.4px page) and `g` (58px page) are childless with
bytes → leaf pages: `leaf_draw(198.4, 480, 198.4)` → font 12 ≥ 7 and 480 ≥
300 → `Text`; `leaf_draw(58, 480, 58)` → 58 < CARD_PX(80) → `Label`:

```rust
        use LeafDraw::{Label, Text};
        let draws: Vec<Draw> = items.iter().map(|i| i.draw).collect();
        assert_eq!(
            draws,
            vec![
                Draw::Container(Rung::Full),   // root 1639px
                Draw::Container(Rung::Full),   // a.rs 1602px (no byte_range → container)
                Draw::Container(Rung::Detail), // b.rs 301px
                Draw::Leaf(Text),              // f: 198.4px page, font 12, wide
                Draw::Leaf(Label),             // g: 58px page (< CARD_PX)
            ]
        );
```

In `packed_walk_merges_tiny_nodes`, replace the final assertion (old line 329)
`assert!(items.iter().all(|i| i.rung == Rung::Dot));` with a check that every
surviving node is a Dot in either family:

```rust
        assert!(items.iter().all(|i| matches!(
            i.draw,
            Draw::Container(Rung::Dot) | Draw::Leaf(LeafDraw::Dot)
        )));
```

- [ ] **Step 4: Rewrite the `treemap.rs` paint path (production code)**

**4a — imports & constants.** In `crates/outrider/src/treemap.rs`, change the
world import (old line 17) to pull in the new types:

```rust
use crate::world::{self, Draw, LeafDraw, Rung};
```

Add two constants after `truncate_to_width` (after old line 36):

```rust
/// Monospace advance width used by the minimap bars (spec §3): 0.6·FONT_PX.
const CHAR_ADV: f64 = 0.6 * content::FONT_PX;
/// Left text inset shared by name rows, body rows, and minimap bars.
const BODY_PAD: f64 = 6.0;
```

**4b — structs.** Replace the `BodyText` struct and the `PaintItem` struct
(old lines 53–81) with these four:

```rust
/// One shaped body/code line: canvas position, text, and colored runs.
struct BodyText {
    x: f32,
    y: f32,
    text: String,
    runs: Vec<(usize, u32)>,
}

/// A name row — pinned at 12px (containers, Label leaves) or scaled with a
/// leaf page (Text leaves).
struct NameRow {
    x: f32,
    y: f32,
    font_px: f32,
    text: String,
}

/// One minimap bar: a source line drawn as a single colored quad.
struct MinimapBar {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: u32,
}

/// Owned, GPUI-free paint instruction — built in render (which may borrow
/// self), moved into the 'static canvas closure.
struct PaintItem {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fill: u32,
    border: u32,
    stripe: Option<u32>,
    focused: bool,
    /// Font size for body rows: FONT_PX·scale for a Text leaf, else 12.0.
    body_font_px: f32,
    name: Option<NameRow>,
    body: Vec<BodyText>,
    bars: Vec<MinimapBar>,
}
```

**4c — body builders.** Replace the whole `build_body` function (old lines
123–191) with three functions. `runs_from_spans` and `code_line` above it
are unchanged.

```rust
/// Content-table rows for a container, pinned at 12px to the CLIPPED top so
/// the header stays readable when the box is scrolled part-off (spec §2).
fn container_body(
    node: &SymbolNode,
    rung: Rung,
    px: &world::PxRect,
    left: f64,
    label_w: f64,
    vh: f64,
) -> Vec<BodyText> {
    if rung == Rung::Dot || rung == Rung::Label {
        return Vec::new();
    }
    let font = FONT_PX as f32;
    let mut out = Vec::new();
    for (k, line) in content::body_lines(node, rung).into_iter().enumerate() {
        let y = px.y + HEADER + k as f64 * LINE_STEP;
        if y + LINE_STEP > px.y + px.h || y > vh {
            break;
        }
        let (text, color) = match line {
            BodyLine::Plain(t) => (t, theme::TEXT_PRIMARY),
            BodyLine::Dim(t) => (t, theme::TEXT_SECONDARY),
        };
        if let Some(shown) = truncate_to_width(&text, label_w as f32, font) {
            let len = shown.len();
            out.push(BodyText {
                x: (left + BODY_PAD) as f32,
                y: y as f32,
                text: shown,
                runs: vec![(len, color)],
            });
        }
    }
    out
}

/// A leaf page's rows at uniform scale (spec §2): the signature/readout row
/// then every source line, anchored to the UNCLIPPED top/left so the whole
/// page moves and scales as one unit — no windowing, no clipping. Rows whose
/// scaled y-band leaves the viewport are skipped for cost only.
#[allow(clippy::too_many_arguments)]
fn leaf_text_body(
    node: &SymbolNode,
    left: f64,
    top: f64,
    full_h: f64,
    label_w: f64,
    vh: f64,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> Vec<BodyText> {
    let scale = full_h / content::natural_px(node);
    let font = (FONT_PX * scale) as f32;
    let step = LINE_STEP * scale;
    let x = (left + BODY_PAD * scale) as f32;
    let mut out = Vec::new();
    let lines = content::body_lines(node, Rung::Full);
    let rows = lines.len();
    for (k, line) in lines.into_iter().enumerate() {
        let y = top + (HEADER + k as f64 * LINE_STEP) * scale;
        if y > vh {
            break;
        }
        if y + step < 0.0 {
            continue;
        }
        let (text, color) = match line {
            BodyLine::Plain(t) => (t, theme::TEXT_PRIMARY),
            BodyLine::Dim(t) => (t, theme::TEXT_SECONDARY),
        };
        if let Some(shown) = truncate_to_width(&text, label_w as f32, font) {
            let len = shown.len();
            out.push(BodyText { x, y: y as f32, text: shown, runs: vec![(len, color)] });
        }
    }
    let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
    let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
    if let Some(m) = buffers.get(&rel, syms) {
        if let Some(start) = m.symbol_start_line(&node.id) {
            let count = (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
            for j in 0..count {
                let y = top + (HEADER + (rows + j) as f64 * LINE_STEP) * scale;
                if y > vh {
                    break;
                }
                if y + step < 0.0 {
                    continue;
                }
                if let Some((text, spans)) = m.buffer.line(start + j) {
                    if let Some((shown, runs)) = code_line(&text, spans, label_w as f32, font) {
                        out.push(BodyText { x, y: y as f32, text: shown, runs });
                    }
                }
            }
        }
    }
    out
}

/// Minimap bars for a far-zoom leaf (spec §3): one colored quad per source
/// line, pixel-aligned to the rows the glyphs occupy at the Text tier, so
/// the Minimap→Text switch is seamless.
fn leaf_minimap(
    node: &SymbolNode,
    left: f64,
    top: f64,
    full_h: f64,
    vh: f64,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> Vec<MinimapBar> {
    let scale = full_h / content::natural_px(node);
    let step = LINE_STEP * scale;
    let bar_h = (step * 0.7) as f32;
    let mut bars = Vec::new();
    let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
    let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
    if let Some(m) = buffers.get(&rel, syms) {
        if let Some(start) = m.symbol_start_line(&node.id) {
            let count = (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
            for r in 0..count {
                // Same y-band as code row r (past the header + 1 readout row).
                let row_y = top + (HEADER + (1 + r) as f64 * LINE_STEP) * scale;
                if row_y > vh {
                    break;
                }
                if row_y + step < 0.0 {
                    continue;
                }
                let mr = m.buffer.minimap_row(start + r);
                if mr.len == 0 {
                    continue;
                }
                let indent = mr.indent as f64;
                let x = left + (BODY_PAD + indent * CHAR_ADV) * scale;
                let avail = (world::PAGE_W - BODY_PAD - indent * CHAR_ADV).max(0.0);
                let w = (mr.len as f64 * CHAR_ADV).min(avail) * scale;
                bars.push(MinimapBar {
                    x: x as f32,
                    y: (row_y + step * 0.15) as f32,
                    w: w as f32,
                    h: bar_h,
                    color: theme::minimap_color(mr.kind),
                });
            }
        }
    }
    bars
}
```

**4d — name helpers.** Add these two associated functions inside
`impl TreemapView` (place them right before `paint_items`):

```rust
    /// A name pinned at 12px to the clipped box corner; `center` vertically
    /// centers it in the box (the Label tier). None when it doesn't fit.
    fn pinned_name(item: &world::DrawItem, center: bool) -> Option<NameRow> {
        let font = FONT_PX as f32;
        let text = truncate_to_width(&item.node.name, item.label_w as f32, font)?;
        let y = if center {
            item.px.y + (item.px.h - f64::from(font) * 1.3) / 2.0
        } else {
            item.px.y + 4.0
        };
        Some(NameRow { x: (item.px.x + BODY_PAD) as f32, y: y as f32, font_px: font, text })
    }

    /// A leaf page's name row at uniform scale, anchored to the UNCLIPPED
    /// top/left so it moves and scales with the page (spec §2).
    fn scaled_name(item: &world::DrawItem, scale: f64) -> Option<NameRow> {
        let font = (FONT_PX * scale) as f32;
        let text = truncate_to_width(&item.node.name, item.label_w as f32, font)?;
        Some(NameRow {
            x: (item.left + BODY_PAD * scale) as f32,
            y: (item.top + 4.0 * scale) as f32,
            font_px: font,
            text,
        })
    }
```

**4e — `paint_items` dispatch.** Replace the `for item in items { … }` loop
body in `paint_items` (old lines 299–332) with the draw-mode dispatch:

```rust
        for item in items {
            let is_leaf = matches!(item.draw, Draw::Leaf(_));
            let fill = theme::box_fill(is_leaf, item.level);
            let mut body_font_px = FONT_PX as f32;
            let mut name = None;
            let mut body = Vec::new();
            let mut bars = Vec::new();
            match item.draw {
                Draw::Container(rung) => {
                    if rung != Rung::Dot && item.px.h >= 14.0 {
                        name = Self::pinned_name(&item, rung == Rung::Label);
                    }
                    body = container_body(item.node, rung, &item.px, item.left, item.label_w, vh);
                }
                Draw::Leaf(LeafDraw::Dot) => {}
                Draw::Leaf(LeafDraw::Label) => {
                    if item.px.h >= 14.0 {
                        name = Self::pinned_name(&item, true);
                    }
                }
                Draw::Leaf(LeafDraw::Minimap) => {
                    bars = leaf_minimap(
                        item.node,
                        item.left,
                        item.top,
                        item.full_h,
                        vh,
                        &mut self.buffers,
                        &self.file_symbols,
                    );
                }
                Draw::Leaf(LeafDraw::Text) => {
                    let scale = item.full_h / content::natural_px(item.node);
                    body_font_px = (FONT_PX * scale) as f32;
                    name = Self::scaled_name(&item, scale);
                    body = leaf_text_body(
                        item.node,
                        item.left,
                        item.top,
                        item.full_h,
                        item.label_w,
                        vh,
                        &mut self.buffers,
                        &self.file_symbols,
                    );
                }
            }
            out.push(PaintItem {
                x: item.px.x as f32,
                y: item.px.y as f32,
                w: item.px.w as f32,
                h: item.px.h as f32,
                fill,
                border: theme::border_for(fill),
                stripe: (item.node.churn > 0.0).then(|| theme::churn_heat(item.node.churn)),
                focused: item.node.id == focus_id,
                body_font_px,
                name,
                body,
                bars,
            });
        }
```

**4f — canvas closure.** Replace the whole second `canvas` closure body (old
lines 490–582, from `let origin = bounds.origin;` through the end of the
per-item `for` loop) with the dispatcher that paints box, stripe, bars,
name, then body:

```rust
                    move |bounds, _prepaint, window, _cx: &mut App| {
                        let origin = bounds.origin;
                        let run = |len: usize, color: u32| TextRun {
                            len,
                            font: gpui::font(theme::FONT_FAMILY),
                            color: rgb(color).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        for item in &items {
                            let b = Bounds::new(
                                point(origin.x + px(item.x), origin.y + px(item.y)),
                                size(px(item.w), px(item.h)),
                            );
                            let (bw, bc) = if item.focused {
                                (2.0, theme::FOCUS_BORDER)
                            } else {
                                (1.0, item.border)
                            };
                            window.paint_quad(quad(
                                b,
                                px(theme::CORNER_RADIUS),
                                rgb(item.fill),
                                px(bw),
                                rgb(bc),
                                BorderStyle::default(),
                            ));
                            if let Some(heat) = item.stripe {
                                let sb = Bounds::new(
                                    point(origin.x + px(item.x + 1.0), origin.y + px(item.y + 1.0)),
                                    size(px(theme::STRIPE_W), px((item.h - 2.0).max(0.0))),
                                );
                                window.paint_quad(quad(
                                    sb,
                                    px(0.),
                                    rgb(heat),
                                    px(0.),
                                    rgb(heat),
                                    BorderStyle::default(),
                                ));
                            }
                            for bar in &item.bars {
                                let bb = Bounds::new(
                                    point(origin.x + px(bar.x), origin.y + px(bar.y)),
                                    size(px(bar.w), px(bar.h)),
                                );
                                window.paint_quad(quad(
                                    bb,
                                    px(0.),
                                    rgb(bar.color),
                                    px(0.),
                                    rgb(bar.color),
                                    BorderStyle::default(),
                                ));
                            }
                            if let Some(n) = &item.name {
                                let line = window.text_system().shape_line(
                                    n.text.clone().into(),
                                    px(n.font_px),
                                    &[run(n.text.len(), theme::TEXT_PRIMARY)],
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(n.x), origin.y + px(n.y)),
                                    px(n.font_px * 1.3),
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
                            }
                            let body_line_height = px(item.body_font_px * 1.3);
                            for bt in &item.body {
                                if bt.text.is_empty() {
                                    continue;
                                }
                                let runs: Vec<TextRun> =
                                    bt.runs.iter().map(|&(len, color)| run(len, color)).collect();
                                let line = window.text_system().shape_line(
                                    bt.text.clone().into(),
                                    px(item.body_font_px),
                                    &runs,
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(bt.x), origin.y + px(bt.y)),
                                    body_line_height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
                            }
                        }
                    },
```

- [ ] **Step 5: Update the `treemap.rs` tests**

The old `build_body` tests exercised the removed scale/clip window. In
`crates/outrider/src/treemap.rs` `mod tests`, change the `use super::{…}`
line (old line 605) to:

```rust
    use super::{
        code_line, container_body, leaf_minimap, leaf_text_body, runs_from_spans,
        truncate_to_width, HEADER, LINE_STEP,
    };
```

Replace the three tests `build_body_positions_detail_lines`,
`build_body_full_leaf_appends_windowed_code`, and
`build_body_full_leaf_scales_step_and_clips_at_box_edge` (old lines 656–723)
with these four:

```rust
    #[test]
    fn container_body_positions_detail_lines() {
        let f = node(SymbolKind::File, "a.rs", Some(0..24), 2, None, Some("Doc line."));
        let px = PxRect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let body = container_body(&f, Rung::Detail, &px, 0.0, 400.0, 600.0);
        // churn readout + doc first line (no items → no kind-counts line)
        assert_eq!(body.len(), 2);
        assert_eq!(body[1].text, "Doc line.");
        assert!((f64::from(body[0].y) - HEADER).abs() < 1e-3);
        assert!((f64::from(body[1].y) - (HEADER + LINE_STEP)).abs() < 1e-3);
    }

    #[test]
    fn leaf_text_body_paints_signature_and_code_at_scale_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let leaf = node(SymbolKind::Fn, "a.rs::two", Some(12..23), 1, Some("fn two()"), None);
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 12)]);
        let natural = crate::content::natural_px(&leaf);
        // scale 1.0: full_h == natural
        let body =
            leaf_text_body(&leaf, 0.0, 0.0, natural, 480.0, 600.0, &mut mgr, &file_symbols);
        // signature row + the symbol's one code line — no window, no clip
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].text, "fn two()");
        assert_eq!(body[1].text, "fn two() {}");
        assert!(body[1].runs.len() > 1, "code rows carry colored runs");
        assert_eq!(body[1].runs.iter().map(|r| r.0).sum::<usize>(), body[1].text.len());
        // row 0 (signature) at natural-y HEADER; code row at HEADER + LINE_STEP
        assert!((f64::from(body[0].y) - HEADER).abs() < 1e-3);
        assert!((f64::from(body[1].y) - (HEADER + LINE_STEP)).abs() < 1e-3);
    }

    #[test]
    fn leaf_text_body_scales_uniformly_past_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let leaf = node(SymbolKind::Fn, "a.rs::two", Some(12..23), 1, Some("fn two()"), None);
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 12)]);
        let natural = crate::content::natural_px(&leaf);
        // zoom 2× (full_h = 2·natural): every row's y doubles, still no clip
        let body = leaf_text_body(
            &leaf, 0.0, 0.0, 2.0 * natural, 960.0, 100_000.0, &mut mgr, &file_symbols,
        );
        assert_eq!(body.len(), 2);
        assert!((f64::from(body[0].y) - 2.0 * HEADER).abs() < 1e-3);
        assert!((f64::from(body[1].y) - 2.0 * (HEADER + LINE_STEP)).abs() < 1e-3);
        // buffer unavailable → signature only, no code
        let mut broken = BufferManager::new(std::path::PathBuf::from("/nonexistent"));
        let body =
            leaf_text_body(&leaf, 0.0, 0.0, natural, 480.0, 600.0, &mut broken, &BTreeMap::new());
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].text, "fn two()");
    }

    #[test]
    fn leaf_minimap_bars_align_to_code_rows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\n    let x = 1;\n").unwrap();
        let leaf = node(SymbolKind::File, "a.rs", Some(0..24), 2, None, None);
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 0)]);
        let natural = crate::content::natural_px(&leaf);
        let bars = leaf_minimap(&leaf, 0.0, 0.0, natural, 600.0, &mut mgr, &file_symbols);
        // two non-blank source lines → two bars
        assert_eq!(bars.len(), 2);
        // bar 0 sits centered in the first code row (HEADER + LINE_STEP)
        let row_y0 = HEADER + LINE_STEP;
        assert!((f64::from(bars[0].y) - (row_y0 + LINE_STEP * 0.15)).abs() < 1e-3);
        // second line is indented 4 spaces → its bar starts further right
        assert!(bars[1].x > bars[0].x);
    }
```

- [ ] **Step 6: Run the workspace tests**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace 2>&1 | tail -30`
Expected: PASS. If `packed_walk_zoom_one_clips_and_keeps_unclipped_fields`
disagrees on a `Draw` value, recompute `leaf_draw` for that box from the
fixture rect and correct the expected vector (do not change `leaf_draw`).

- [ ] **Step 7: Clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/outrider/src/content.rs crates/outrider/src/world.rs crates/outrider/src/treemap.rs
git commit -m "feat(app): uniform leaf-page scaling, minimap paint, leaf/container draw dispatch"
```

---
### Task 9: Column-first square packing (`outrider-layout/src/pack.rs`, `world.rs`)

**Files:**
- Modify: `crates/outrider-layout/src/pack.rs` (column-first `size`, chunk ordering, `SymbolKind` import, test updates)
- Modify: `crates/outrider/src/world.rs` (`PACK_ASPECT` 1.6 → 1.0)

**Interfaces:**
- Consumes: `outrider_index::SymbolKind` (Task 2's `Chunk` variant), `PackConfig`, `SymbolNode`.
- Produces: same `pack`/`PackLayout` API; only the internal `size` layout
  and the app's `PACK_ASPECT` change. `world.rs::PACK_ASPECT = 1.0`.

The worked-example and single-child fixtures are numerically identical under
column-first packing (a tall first child fills its column, the next wraps to
the same x/y a shelf would give). Only `sibling_subtree_stable_under_edit`
changes, and two new fixtures assert the column-fill and chunk-order
behavior.

- [ ] **Step 1: Update `pack.rs` tests to the column-first layout**

In `crates/outrider-layout/src/pack.rs` `mod tests`, replace the
`sibling_subtree_stable_under_edit` test (old lines 227–243) with the
column-first rects (verified in the plan):

```rust
    #[test]
    fn sibling_subtree_stable_under_edit() {
        // Grow f (10 → 50 lines): b.rs reflows internally and root resizes,
        // but a.rs — a sibling subtree — keeps its exact position. Under
        // column-first packing f fills the first column and g wraps to the
        // second column of b.rs.
        let before = pack(&worked_example(), &cfg());
        let mut edited = worked_example();
        edited.root.children[1].children[0].measure = 50;
        let after = pack(&edited, &cfg());
        assert_eq!(rect(&before, "a.rs"), rect(&after, "a.rs"));
        // f: 480 × 822.4; b.rs grows wide (two columns): 984 × 859.2
        assert_rect(rect(&after, "b.rs::f"), 504.0, 57.6, 480.0, 822.4);
        assert_rect(rect(&after, "b.rs"), 496.0, 28.8, 984.0, 859.2);
        // g wraps to b.rs's second column
        let g = rect(&after, "b.rs::g");
        close(g.x, 992.0);
        close(g.y, 57.6);
    }
```

Update the two stale shelf comments (assertions unchanged). In
`worked_example_exact_rects` (old lines 186–191) replace the comment lines:

```rust
        // leaf pages: w = page_w, h = header + (1+measure)·line_step + bottom_pad
        assert_rect(rect(&p, "a.rs"), 8.0, 28.8, 480.0, 1602.4);
        // b.rs: f fills the first column, g stacks under it (one column)
        assert_rect(rect(&p, "b.rs::f"), 504.0, 57.6, 480.0, 198.4);
        assert_rect(rect(&p, "b.rs::g"), 504.0, 264.0, 480.0, 58.0);
        assert_rect(rect(&p, "b.rs"), 496.0, 28.8, 496.0, 301.2);
        // root: a.rs fills column 1 (tall), b.rs wraps to column 2
        assert_rect(rect(&p, ""), 0.0, 0.0, 1000.0, 1639.2);
```

In `children_placed_by_name_then_ordinal_never_size` (old lines 219–224)
replace the trailing comment/asserts:

```rust
        // alpha is placed first: top-left of the content area
        close(a.x, 8.0);
        close(a.y, 28.8);
        // zeta is placed second: it wraps to the next column (alpha's column
        // is full), landing at the same top — name order still decides first
        close(z.x, 496.0);
        close(z.y, 28.8);
```

In `wide_child_sets_the_floor_for_target_width` (old lines 245–263) replace
the comment lines only (assertions unchanged); the `tallest` floor now
guarantees the single child never wraps alone:

```rust
    #[test]
    fn wide_child_sets_the_floor_for_target_width() {
        // A single child never wraps alone: target_h = max(tallest child,
        // √(area/aspect)) floors the column height to fit it.
        let tree = SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                0,
                vec![n(SymbolKind::File, "one.rs", "one.rs", 1, vec![])],
            ),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        // single 480×58 child: content 480×58 → root 496 × 94.8
        assert_rect(rect(&p, "one.rs"), 8.0, 28.8, 480.0, 58.0);
        assert_rect(rect(&p, ""), 0.0, 0.0, 496.0, 94.8);
    }
```

Add two new tests at the end of `mod tests` (before the closing `}`):

```rust
    #[test]
    fn columns_fill_down_then_wrap_right() {
        // Four equal 480×120.4 pages, aspect 1.6 (test cfg): target_h ≈ 380
        // holds three per column, the fourth wraps to a second column.
        let files: Vec<SymbolNode> = (1..=4)
            .map(|i| n(SymbolKind::File, &format!("c{i}.rs"), &format!("c{i}.rs"), 5, vec![]))
            .collect();
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, files),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        assert_rect(rect(&p, "c1.rs"), 8.0, 28.8, 480.0, 120.4);
        assert_rect(rect(&p, "c2.rs"), 8.0, 157.2, 480.0, 120.4);
        assert_rect(rect(&p, "c3.rs"), 8.0, 285.6, 480.0, 120.4);
        assert_rect(rect(&p, "c4.rs"), 496.0, 28.8, 480.0, 120.4);
        assert_rect(rect(&p, ""), 0.0, 0.0, 984.0, 414.0);
    }

    #[test]
    fn chunk_children_pack_in_source_order_not_label_order() {
        // Three chunks whose labels sort reverse to their byte order; the
        // packer must order them by byte_range.start, not by name.
        let chunk = |label: &str, start: usize, ord: u32| SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Chunk,
                qualified_path: format!("f.rs#{ord}"),
                ordinal: ord,
            },
            name: label.into(),
            byte_range: Some(start..start + 10),
            signature: None,
            doc: None,
            measure: 2,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        };
        let mut file = n(
            SymbolKind::File,
            "f.rs",
            "f.rs",
            12,
            vec![chunk("zzz", 0, 0), chunk("mmm", 60, 1), chunk("aaa", 120, 2)],
        );
        file.byte_range = Some(0..200);
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, vec![file]),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        let z = rect(&p, "f.rs#0"); // "zzz", byte 0
        let m = rect(&p, "f.rs#1"); // "mmm", byte 60
        let a = rect(&p, "f.rs#2"); // "aaa", byte 120
        // one column (same x); source order sets the vertical order
        close(z.x, m.x);
        close(m.x, a.x);
        assert!(z.y < m.y && m.y < a.y, "chunks stack zzz(0) < mmm(60) < aaa(120)");
    }
```

- [ ] **Step 2: Run to verify the updated/new tests fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-layout 2>&1 | tail -30`
Expected: FAIL — `sibling_subtree_stable_under_edit`,
`columns_fill_down_then_wrap_right`, and
`chunk_children_pack_in_source_order_not_label_order` fail under the current
shelf packer.

- [ ] **Step 3: Rewrite `size` for column-first packing**

In `crates/outrider-layout/src/pack.rs`, add `SymbolKind` to the import
(old line 3):

```rust
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
```

Replace the whole `size` function (old lines 49–88) with:

```rust
/// Bottom-up size pass: returns (w, h) and records each node's position
/// relative to its parent's origin in `rel` (x, y, w, h). Children fill
/// columns top-to-bottom, wrapping right toward a square aspect (spec §5).
/// The root's relative position stays (0, 0).
fn size(
    node: &SymbolNode,
    cfg: &PackConfig,
    rel: &mut BTreeMap<SymbolId, (f64, f64, f64, f64)>,
) -> (f64, f64) {
    if node.children.is_empty() {
        let h = cfg.header + (1.0 + node.measure as f64) * cfg.line_step + cfg.bottom_pad;
        rel.insert(node.id.clone(), (0.0, 0.0, cfg.page_w, h));
        return (cfg.page_w, h);
    }
    // Re-derive the ordering invariant locally; never trust input Vec order.
    let mut order: Vec<&SymbolNode> = node.children.iter().collect();
    if order.first().map(|c| c.id.kind) == Some(SymbolKind::Chunk) {
        // Chunk children pack in source order, ignoring their heading labels.
        order.sort_by(|a, b| {
            let ka = a.byte_range.as_ref().map(|r| r.start).unwrap_or(0);
            let kb = b.byte_range.as_ref().map(|r| r.start).unwrap_or(0);
            ka.cmp(&kb).then(a.id.ordinal.cmp(&b.id.ordinal))
        });
    } else {
        order.sort_by(|a, b| {
            a.name.as_bytes().cmp(b.name.as_bytes()).then(a.id.ordinal.cmp(&b.id.ordinal))
        });
    }
    let sizes: Vec<(f64, f64)> = order.iter().map(|c| size(c, cfg, rel)).collect();
    let area: f64 = sizes.iter().map(|(w, h)| w * h).sum();
    let tallest = sizes.iter().map(|&(_, h)| h).fold(0.0, f64::max);
    // tallest.max(...) guarantees no child is ever forced to wrap alone.
    let target_h = tallest.max((area / cfg.aspect).sqrt());
    let (mut x, mut y, mut col_w, mut content_h) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (child, &(w, h)) in order.iter().zip(&sizes) {
        if y > 0.0 && y + h > target_h {
            x += col_w + cfg.gap;
            y = 0.0;
            col_w = 0.0;
        }
        let e = rel.get_mut(&child.id).expect("child sized above");
        e.0 = cfg.gap + x;
        e.1 = cfg.header + cfg.gap + y;
        col_w = col_w.max(w);
        content_h = content_h.max(y + h);
        y += h + cfg.gap;
    }
    let wh = (x + col_w + 2.0 * cfg.gap, cfg.header + content_h + 2.0 * cfg.gap);
    rel.insert(node.id.clone(), (0.0, 0.0, wh.0, wh.1));
    wh
}
```

- [ ] **Step 4: Flip the app aspect to square (`world.rs`)**

In `crates/outrider/src/world.rs`, change `PACK_ASPECT` (old line 21):

```rust
/// Target container width/height ratio (≈ square).
pub const PACK_ASPECT: f64 = 1.0;
```

- [ ] **Step 5: Run the workspace tests**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace 2>&1 | tail -30`
Expected: PASS (all pack tests, including the two new ones, green; the app
tests unaffected — `world.rs` tests pass their own `pack_cfg`).

- [ ] **Step 6: Clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/outrider-layout/src/pack.rs crates/outrider/src/world.rs
git commit -m "feat(layout): column-first square packing with source-order chunk placement"
```

---
## Self-Review

Reviewed the plan against the spec (`docs/superpowers/specs/2026-07-10-leaf-scaling-chunking-packing-design.md`) with fresh eyes.

### 1. Spec coverage

| Spec section | Task(s) |
|---|---|
| §1 Uniform leaf-page scaling (delete `code_scale`/clamps, whole page at `scale = ph/natural_px`, uncapped) | Task 8 (Steps 1, 4) — constants + `code_scale` removed; `leaf_text_body` anchors page to unclipped top/left and paints every run at `scale = ph/natural_px`. |
| §2 Far-zoom minimap LOD (`LeafDraw` ladder Dot/Label/Minimap/Text) | Task 7 (`leaf_draw` + `LeafDraw` enum), Task 8 (dispatch through `Draw::Leaf`). |
| §3 Minimap bar geometry (one bar per source line, row_y/bar_h/x/w/color formulas, `len==0` draws nothing) | Task 5 (`MinimapRow`/`minimap_row`), Task 6 (`minimap_color`), Task 8 (`leaf_minimap` builder). |
| §4 Pluggable chunking (`ChunkStrategy`, `LineChunker`, `MarkdownChunker`) | Task 1. |
| §5 `SymbolKind::Chunk` + content arms | Task 2. |
| §6 Chunk injection in `build_folder`/`scan.rs` | Task 3, with anchor verification in Task 4. |
| §7 Column-first square packing (`PACK_ASPECT` 1.6→1.0, fill-down-then-wrap-right) | Task 9 (`size()` rewrite + `PACK_ASPECT` flip). |
| §8 Chunk children ordered by `byte_range.start` | Task 9 (Step 3 source-order branch + `chunk_children_pack_in_source_order_not_label_order`). |
| §9 `Draw` enum threading `rung`→`draw`, walk dispatch | Task 8 (Steps 2–3). |

No spec section is left without an implementing task.

### 2. Placeholder scan

No `TBD`/`TODO`/"handle edge cases"/"similar to Task N" placeholders. Every code step carries complete code; every run step carries an exact command and expected output. Deletions name exact line ranges and the surrounding retained context.

### 3. Type consistency

Cross-task identifiers verified end to end:
- `Draw::Container(Rung)` / `Draw::Leaf(LeafDraw)` — defined Task 8 Step 2, produced by `walk`, consumed by `paint_items` (Task 8 Step 4).
- `LeafDraw { Dot, Label, Minimap, Text }` and `leaf_draw(ph, pw, natural_px) -> LeafDraw` — introduced additively in Task 7, dispatched in Task 8, no signature drift.
- `MinimapRow { indent: u32, len: u32, kind: HighlightKind }` returned by value from `minimap_row` (Task 5) and destructured by `leaf_minimap` (Task 8); `minimap_color(kind: HighlightKind) -> u32` (Task 6) matches.
- `SymbolKind::Chunk` — added Task 2, matched in `content.rs` arms (Task 2), keyed in `scan.rs` injection (Task 3) and pack ordering (Task 9).
- Packing builders in `treemap.rs` (`container_body`, `leaf_text_body`, `leaf_minimap`, `runs_from_spans`, `truncate_to_width`) — names identical between the production rewrite (Task 8 Step 4) and the test imports (Task 8 Step 5).
- Constants added exactly once and referenced consistently: `CHAR_ADV`/`BODY_PAD` (Task 8), `MIN_TEXT_FONT_PX` (Task 7), `CHUNK_MAX_LINES` (Task 1), `PACK_ASPECT=1.0` (Task 9). Deleted constants (`MIN_CODE_FONT_PX`, `MIN_CODE_SCALE`, `LEAF_CODE_MIN_PX`, `code_scale`) are removed in Task 8 and referenced nowhere afterward.

No inconsistencies found. Plan is complete.

