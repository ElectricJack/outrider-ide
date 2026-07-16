# Makefile Parsing and Syntax Highlighting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recognize conventional Makefile paths, expose complete ordered target/section structure, and syntax-highlight Make source.

**Architecture:** A shared `SourceLanguage::for_path` classifier becomes the single path-to-language decision used by indexing and buffer materialization. `tree-sitter-make` 1.1.1 supplies the grammar; a Make parser extracts rule ranges and fills every gap with labeled sections, while `FileBuffer` uses a project-owned query derived from the grammar's stable nodes.

**Tech Stack:** Rust 2021, tree-sitter 0.26.10, tree-sitter-make 1.1.1, anyhow, ropey, Cargo tests.

## Global Constraints

- Recognize exactly `Makefile`, `makefile`, `GNUmakefile`, and files with extension `mk`.
- Preserve every source byte in ordered, non-overlapping target or section ranges.
- Do not evaluate variables/includes, inject a shell grammar, add recipe call graphs, or create nodes for every directive.
- Use test-driven development: observe each regression test fail before production changes.
- Preserve unrelated working-tree changes in `crates/outrider/src/rasterize.rs` and `crates/outrider/src/theme.rs`.

## File Map

- Create `crates/outrider-index/src/language.rs`: shared path-aware language classification.
- Modify `crates/outrider-index/src/lib.rs`: export `SourceLanguage`.
- Modify `crates/outrider-index/src/index.rs`: dispatch parsers from `SourceLanguage`.
- Modify `crates/outrider-index/src/parse.rs`: Make target extraction and coverage-preserving sections.
- Modify `crates/outrider-index/src/buffer.rs`: select Make grammar/query from `SourceLanguage`.
- Modify `crates/outrider/src/buffers.rs`: pass the relative path, not only its extension.
- Modify `crates/outrider-index/Cargo.toml` and `Cargo.lock`: add `tree-sitter-make = "1.1.1"`.
- Modify `crates/outrider-index/tests/index_test.rs`: extensionless and `.mk` end-to-end indexing.

---

### Task 1: Shared Path-aware Language Classification

**Files:**
- Create: `crates/outrider-index/src/language.rs`
- Modify: `crates/outrider-index/src/lib.rs`

**Interfaces:**
- Produces: `pub enum SourceLanguage` and `pub fn SourceLanguage::for_path(path: &Path) -> Option<Self>`.
- Consumed by: Tasks 2–4.

- [ ] **Step 1: Write failing classifier tests**

Add table-driven tests in `language.rs`:

```rust
#[test]
fn recognizes_make_paths() {
    for path in ["Makefile", "makefile", "GNUmakefile", "build/rules.mk"] {
        assert_eq!(SourceLanguage::for_path(Path::new(path)), Some(SourceLanguage::Make));
    }
}

#[test]
fn rejects_make_lookalikes() {
    for path in ["Makefile.txt", "GNUMakefile", "MAKEFILE", "rules.mk.bak"] {
        assert_ne!(SourceLanguage::for_path(Path::new(path)), Some(SourceLanguage::Make));
    }
}
```

- [ ] **Step 2: Run the tests and verify RED**

Run: `cargo test -p outrider-index language::tests -- --nocapture`
Expected: compilation failure because `SourceLanguage` is not defined.

- [ ] **Step 3: Implement the classifier**

Define a `Copy + Eq` enum covering every currently supported parser/highlighter language and implement exact filename matching before lowercase extension matching:

```rust
pub fn for_path(path: &Path) -> Option<Self> {
    match path.file_name().and_then(|n| n.to_str()) {
        Some("Makefile" | "makefile" | "GNUmakefile") => return Some(Self::Make),
        _ => {}
    }
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "mk" => Some(Self::Make),
        "rs" => Some(Self::Rust),
        // Include C, Cpp, Python, JavaScript, TypeScript, Tsx, CSharp,
        // Markdown, Toml and all other languages already handled by FileBuffer.
        _ => None,
    }
}
```

Export it with `pub mod language; pub use language::SourceLanguage;`.

- [ ] **Step 4: Run tests and verify GREEN**

Run: `cargo test -p outrider-index language::tests`
Expected: both classifier tests pass.

- [ ] **Step 5: Commit**

```powershell
git add crates/outrider-index/src/language.rs crates/outrider-index/src/lib.rs
git commit -m "refactor(index): centralize source language detection"
```

### Task 2: Make Grammar and Coverage-preserving Structural Parser

**Files:**
- Modify: `crates/outrider-index/Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/outrider-index/src/parse.rs`

**Interfaces:**
- Consumes: `tree_sitter_make::LANGUAGE`.
- Produces: `pub fn parse_make_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>`.

- [ ] **Step 1: Add failing structural tests**

Add tests that parse this representative source and assert names, kinds, and coverage:

```rust
const MAKE: &[u8] = b"# build config\nCC := clang\ninclude local.mk\n\nall clean: deps\n\t$(CC) app.c -o app\n\n%.o: %.c\n\t$(CC) -c $< -o $@\n";

#[test]
fn make_targets_and_sections_cover_the_entire_file() {
    let items = parse_make_items(MAKE).unwrap();
    assert!(items.iter().any(|i| i.kind == SymbolKind::Item { label: "target".into() } && i.name == "all clean"));
    assert!(items.iter().any(|i| i.name == "%.o"));
    assert!(items.iter().any(|i| i.kind == SymbolKind::Item { label: "section".into() }));
    assert_eq!(items.first().unwrap().byte_range.start, 0);
    assert_eq!(items.last().unwrap().byte_range.end, MAKE.len());
    for pair in items.windows(2) {
        assert_eq!(pair[0].byte_range.end, pair[1].byte_range.start);
    }
}
```

Add focused tests for explicit/pattern/multi-target rules, variable/include/conditional/define section labels, a no-target file, and malformed input retaining `0..source.len()`.

- [ ] **Step 2: Run tests and verify RED**

Run: `cargo test -p outrider-index parse::tests::make_ -- --nocapture`
Expected: compilation failure because `parse_make_items` and the dependency are absent.

- [ ] **Step 3: Add the exact grammar dependency**

Add `tree-sitter-make = "1.1.1"` beside the existing grammar crates and run `cargo check -p outrider-index` to update `Cargo.lock`.

- [ ] **Step 4: Implement target extraction**

Parse with `tree_sitter_make::LANGUAGE`. Walk named nodes recursively so rules nested in conditionals are found. For each `rule`, obtain its `targets` child, normalize only surrounding whitespace for the display name, and record the complete rule byte range. Sort ranges and discard nested/overlapping duplicates.

Construct target items with:

```rust
RawItem {
    kind: SymbolKind::Item { label: "target".into() },
    name,
    signature: header_text.trim().to_string(),
    doc: None,
    byte_range: range.clone(),
    line_count: line_count(&source[range.clone()]),
    children: Vec::new(),
}
```

- [ ] **Step 5: Implement the coverage pass**

Walk sorted target ranges with a `cursor`. Emit `section` items for `cursor..target.start`, then the target, and finally `cursor..source.len()`. Do not omit zero-content files; a nonempty target-free file returns one section. Label sections by inspecting top-level named nodes in the range, using priority `Definitions`, `Conditionals`, `Includes`, `Variables`, `Preamble`, then `Section`.

- [ ] **Step 6: Run tests and verify GREEN**

Run: `cargo test -p outrider-index parse::tests::make_`
Expected: all Make parser and coverage tests pass.

- [ ] **Step 7: Commit**

```powershell
git add Cargo.lock crates/outrider-index/Cargo.toml crates/outrider-index/src/parse.rs
git commit -m "feat(index): parse Make targets without losing source"
```

### Task 3: Wire Path-aware Indexing End to End

**Files:**
- Modify: `crates/outrider-index/src/index.rs`
- Modify: `crates/outrider-index/tests/index_test.rs`

**Interfaces:**
- Consumes: `SourceLanguage::for_path`, `parse_make_items`.
- Produces: extensionless and `.mk` files retained and structurally indexed.

- [ ] **Step 1: Write failing integration tests**

Create temporary repositories containing `Makefile` and `config/rules.mk`; call `index_repo`, locate both file nodes, and assert each contains a `target` child and children whose byte ranges cover the file length.

- [ ] **Step 2: Run tests and verify RED**

Run: `cargo test -p outrider-index --test index_test makefile -- --nocapture`
Expected: extensionless `Makefile` has no parsed target children.

- [ ] **Step 3: Replace extension dispatch**

In `materialize_file`, compute `let language = SourceLanguage::for_path(path);`. Change `parser_for` to accept `SourceLanguage` and include `SourceLanguage::Make => Some(parse_make_items)`. Retention becomes `parser.is_some() || is_retained_text(ext)`. Keep Rust/Python doc extraction keyed by the enum.

- [ ] **Step 4: Run integration and crate tests**

Run: `cargo test -p outrider-index --test index_test makefile && cargo test -p outrider-index`
Expected: integration tests and the full crate suite pass.

- [ ] **Step 5: Commit**

```powershell
git add crates/outrider-index/src/index.rs crates/outrider-index/tests/index_test.rs
git commit -m "feat(index): recognize conventional Makefile paths"
```

### Task 4: Make Syntax Highlighting and Buffer Materialization

**Files:**
- Modify: `crates/outrider-index/src/buffer.rs`
- Modify: `crates/outrider/src/buffers.rs`

**Interfaces:**
- Consumes: `SourceLanguage::for_path`, `tree_sitter_make::{LANGUAGE, HIGHLIGHTS_QUERY}`.
- Produces: `FileBuffer::new(text: String, path: &Path)` with Make highlight spans.

- [ ] **Step 1: Write failing highlight tests**

Change buffer fixtures to pass representative paths (`sample.rs`, `notes.md`, etc.), then add a table test for `Makefile`, `makefile`, `GNUmakefile`, and `rules.mk`. Assert `# note` maps to `HighlightKind::Comment`, `CC` maps to `Number` or `Property` after capture mapping, and `include`/conditional tokens map to `Keyword`.

- [ ] **Step 2: Run tests and verify RED**

Run: `cargo test -p outrider-index buffer::tests::make_paths_enable_syntax_highlighting -- --exact`
Expected: failure because `FileBuffer::new` still accepts an extension and has no Make branch.

- [ ] **Step 3: Make buffer selection path-aware**

Change `FileBuffer::new` to classify the supplied path once. Match the enum to existing grammar/query pairs and add Make using `tree_sitter_make::LANGUAGE` plus a project constant `MAKE_HIGHLIGHTS` based on the crate query. Ensure `kind_for` maps `conditional`, `repeat`, `include`, and `exception` to existing palette kinds rather than silently dropping them.

- [ ] **Step 4: Pass paths from BufferManager**

Replace extension extraction in `BufferManager::get` with:

```rust
let mut buffer = FileBuffer::new(text, Path::new(rel_path)).ok()?;
```

Update its tests to verify an extensionless `Makefile` materializes with nonempty highlight spans.

- [ ] **Step 5: Run focused and full tests**

Run: `cargo test -p outrider-index buffer::tests && cargo test -p outrider buffers::tests`
Expected: all buffer/highlighting tests pass.

- [ ] **Step 6: Commit**

```powershell
git add crates/outrider-index/src/buffer.rs crates/outrider/src/buffers.rs
git commit -m "feat: syntax-highlight conventional Makefiles"
```

### Task 5: Final Verification

**Files:**
- Verify only; do not modify unrelated files.

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces: fresh evidence that the feature and repository checks pass.

- [ ] **Step 1: Verify formatting for changed Rust files**

Run `rustfmt --edition 2021 --check` separately on every changed `.rs` file. Do not run a workspace-wide rewrite in the dirty worktree.

- [ ] **Step 2: Verify tests**

Run: `cargo test -p outrider-index && cargo test -p outrider buffers::tests`
Expected: zero failures.

- [ ] **Step 3: Verify build and diff hygiene**

Run: `cargo check --workspace` and `git diff --check`.
Expected: both exit zero; only intended feature files plus the user's pre-existing changes appear in `git status --short`.

- [ ] **Step 4: Review requirements**

Confirm all four filename forms, complete source coverage, target navigation, non-target visibility, syntax highlighting, and malformed-input fallback against the approved design spec.
