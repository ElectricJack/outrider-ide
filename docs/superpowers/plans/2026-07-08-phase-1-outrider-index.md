# Phase 1 — `outrider-index` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn a repository on disk into a `SymbolTree` — folders → files → Rust items → methods, with line-count measures and git-churn percentiles — plus a CLI dump binary (spec milestone 1).

**Architecture:** One lib crate, `outrider-index`, with four modules: `types` (the data model), `scan` (ignore-aware repo walk), `parse` (tree-sitter Rust item extraction), `churn` (git-log commit counts + percentiles). A public `index_repo()` composes them; a `outrider-dump` binary prints the result. No GPUI anywhere.

**Tech Stack:** Rust (edition 2021), `ignore`, `tree-sitter` + `tree-sitter-rust`, `rayon`, `serde`/`serde_json`, `anyhow`; `tempfile` for tests; `git` as a subprocess (not `git2`).

**Source spec:** `docs/superpowers/specs/2026-07-05-outrider-walking-skeleton-design.md` §4.1, §5, §8.2.
**Prerequisite:** Phase 0 **Task 1 only** (workspace scaffold). No GPUI needed; this plan runs in parallel with Phase 0 Tasks 2–4.

## Global Constraints

- **No GPUI types in this crate, ever** (spec §4).
- `children` of every node are **sorted by name (byte-wise), ties by ordinal, always** (spec §4.1, §6.3). Sorting happens at construction; consumers may rely on it.
- `SymbolId.qualified_path` format: repo-relative path with `/` separators for folders/files (`src/auth/session.rs`), `::`-joined for items (`src/auth/session.rs::validate`). Root folder has qualified_path `""`. Same-named siblings disambiguate by `ordinal` (spec §4.1).
- Churn is a **within-repo percentile 0.0–1.0**: files ranked among files, folders (sum of descendants) ranked among folders, methods inherit their file's value (spec §5.4).
- `.gitignore` and standard ignore files define what is scanned — never hardcode exclusions (spec §5.1, parent §4.5).
- Churn cache lives at `.outrider/churn-cache.json`, keyed by HEAD commit hash (spec §5.4). `.outrider/` is already gitignored by Phase 0.
- Determinism: `BTreeMap`/sorted `Vec` in all public data; no `HashMap` iteration order may leak into output.

**Interpretation decisions (recorded here because the spec is ambiguous):**
1. **Non-`.rs` files become `File` leaf nodes** (measure = line count, no children). Required for "Folder measure = sum of children" (§5.2) to hold alongside "every non-ignored file contributes to folder measure" (§5.1).
2. **Container items (`Impl`, `Module`, …) use their own line span as measure** — the span already contains the children, satisfying "aggregate (container)" (§4.1). Layout's measure pass only reads leaf measures anyway (§6.2).
3. **`SymbolNode` gains a `churn_count: u64` field** beyond the spec's §4.1 struct. The §5.4 inspectability readout (`churn: 47 commits · 96th percentile`) needs the raw count as well as the percentile.
4. **Ordinals** are assigned within same-name sibling groups in source-byte order (stable sort preserves it), then the final order is `(name, ordinal)`.

---

### Task 1: Core types

**Files:**
- Create: `crates/outrider-index/src/types.rs`
- Modify: `crates/outrider-index/src/lib.rs`
- Modify: `crates/outrider-index/Cargo.toml`

**Interfaces:**
- Consumes: nothing.
- Produces: `SymbolKind`, `SymbolId { kind, qualified_path: String, ordinal: u16 }`, `SymbolNode { id, name, byte_range: Option<Range<usize>>, measure: u64, churn: f32, churn_count: u64, children: Vec<SymbolNode> }`, `SymbolTree { root, repo_root }`, and `finalize_children(&mut Vec<SymbolNode>)`. Phase 2 (`outrider-layout`) consumes these types verbatim; later tasks in this plan call `finalize_children` after building any child list.

- [ ] **Step 1: Add dependencies**

```bash
cargo add -p outrider-index anyhow serde --features serde/derive
cargo add -p outrider-index serde_json
```

- [ ] **Step 2: Write the failing test**

Create `crates/outrider-index/src/types.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn mk(name: &str) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Impl,
                qualified_path: format!("f.rs::{name}"),
                ordinal: 0,
            },
            name: name.to_string(),
            byte_range: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        }
    }

    #[test]
    fn finalize_children_sorts_by_name_and_assigns_ordinals() {
        let mut kids = vec![mk("beta"), mk("alpha"), mk("alpha")];
        finalize_children(&mut kids);
        let got: Vec<(&str, u16)> = kids
            .iter()
            .map(|c| (c.name.as_str(), c.id.ordinal))
            .collect();
        assert_eq!(got, vec![("alpha", 0), ("alpha", 1), ("beta", 0)]);
    }

    #[test]
    fn symbol_tree_serde_roundtrip() {
        let tree = SymbolTree {
            root: mk("root"),
            repo_root: std::path::PathBuf::from("/tmp/x"),
        };
        let json = serde_json::to_string(&tree).unwrap();
        let back: SymbolTree = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
    }
}
```

In `crates/outrider-index/src/lib.rs`:

```rust
pub mod types;

pub use types::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p outrider-index`
Expected: FAIL — compile error, types not defined.

- [ ] **Step 4: Implement the types**

Prepend to `crates/outrider-index/src/types.rs` (above the test module):

```rust
use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Folder,
    File,
    Module,
    Struct,
    Enum,
    Trait,
    Impl,
    Fn,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolId {
    pub kind: SymbolKind,
    pub qualified_path: String,
    pub ordinal: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolNode {
    pub id: SymbolId,
    pub name: String,
    /// Byte range within the containing file. `None` for folders.
    pub byte_range: Option<Range<usize>>,
    pub measure: u64,
    /// Within-repo churn percentile, 0.0–1.0.
    pub churn: f32,
    /// Raw commit count behind `churn` (inspectability, spec §5.4).
    pub churn_count: u64,
    pub children: Vec<SymbolNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolTree {
    pub root: SymbolNode,
    pub repo_root: PathBuf,
}

/// Sort children byte-wise by name; assign ordinals within same-name runs in
/// prior (source) order. Final order is (name, ordinal) — spec §4.1, §6.3.
pub fn finalize_children(children: &mut [SymbolNode]) {
    children.sort_by(|a, b| a.name.cmp(&b.name)); // stable sort keeps source order on ties
    let mut i = 0;
    while i < children.len() {
        let mut j = i + 1;
        while j < children.len() && children[j].name == children[i].name {
            j += 1;
        }
        for (ord, child) in children[i..j].iter_mut().enumerate() {
            child.id.ordinal = ord as u16;
        }
        i = j;
    }
}
```

Also re-export in `lib.rs`:

```rust
pub use types::finalize_children;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p outrider-index`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider-index Cargo.lock
git commit -m "feat: outrider-index core types (SymbolTree, sorted children with ordinals)"
```

---

### Task 2: Scanner — ignore-aware walk to folders + file leaves

**Files:**
- Create: `crates/outrider-index/src/scan.rs`
- Create: `crates/outrider-index/tests/fixtures/mini_repo/_gitignore`
- Create: `crates/outrider-index/tests/fixtures/mini_repo/README.md`
- Create: `crates/outrider-index/tests/fixtures/mini_repo/src/lib.rs`
- Create: `crates/outrider-index/tests/fixtures/mini_repo/src/util.rs`
- Create: `crates/outrider-index/tests/fixtures/mini_repo/generated/junk.rs`
- Create: `crates/outrider-index/tests/fixtures/mini_repo/debug.log`
- Create: `crates/outrider-index/tests/common/mod.rs`
- Create: `crates/outrider-index/tests/scan_test.rs`
- Modify: `crates/outrider-index/src/lib.rs`

**Interfaces:**
- Consumes: `SymbolNode`/`SymbolKind`/`finalize_children` from Task 1.
- Produces: `scan::scan_files(repo_root: &Path) -> anyhow::Result<Vec<ScannedFile>>` where `ScannedFile { rel_path: PathBuf, lines: u64, bytes: u64 }` (sorted by `rel_path`), and `scan::build_tree(repo_root: &Path, files: &[ScannedFile], rs_children: &BTreeMap<PathBuf, Vec<SymbolNode>>) -> SymbolTree`. Task 4 supplies a non-empty `rs_children`; this task always passes an empty map. Also the shared test fixture + `copy_fixture` helper used by Tasks 4–6.

**Fixture note:** the fixture's ignore file is checked in as `_gitignore` (a real `.gitignore` inside `tests/fixtures/` would make the *outer* repo ignore fixture files). The test helper renames it to `.gitignore` after copying to a temp dir. The ignored directory is deliberately named `generated/` — never `target/` — so the outer repo's own ignore rules can't swallow it.

- [ ] **Step 1: Add dependencies**

```bash
cargo add -p outrider-index ignore
cargo add -p outrider-index --dev tempfile
```

- [ ] **Step 2: Create the fixture**

`tests/fixtures/mini_repo/_gitignore`:

```gitignore
generated/
*.log
```

`tests/fixtures/mini_repo/README.md`:

```markdown
# mini repo
fixture for outrider-index tests
```

`tests/fixtures/mini_repo/src/lib.rs` (content matters for Task 4; create it now exactly like this):

```rust
mod inner {
    pub fn helper() {
        println!("help");
    }
}

struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn new() -> Self {
        Point { x: 0, y: 0 }
    }

    fn norm(&self) -> f64 {
        ((self.x * self.x + self.y * self.y) as f64).sqrt()
    }
}

fn free() {
    let _ = Point::new();
}
```

`tests/fixtures/mini_repo/src/util.rs`:

```rust
pub fn clamp(v: i64, lo: i64, hi: i64) -> i64 {
    v.max(lo).min(hi)
}
```

`tests/fixtures/mini_repo/generated/junk.rs`:

```rust
fn should_never_be_indexed() {}
```

`tests/fixtures/mini_repo/debug.log`:

```
noise
```

- [ ] **Step 3: Write the shared test helper**

`crates/outrider-index/tests/common/mod.rs`:

```rust
use std::fs;
use std::path::Path;

/// Copy a fixture repo to a temp dir, renaming `_gitignore` -> `.gitignore`.
pub fn copy_fixture(name: &str) -> tempfile::TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&src, dir.path());
    let marker = dir.path().join("_gitignore");
    if marker.exists() {
        fs::rename(&marker, dir.path().join(".gitignore")).unwrap();
    }
    dir
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
        }
    }
}
```

- [ ] **Step 4: Write the failing test**

`crates/outrider-index/tests/scan_test.rs`:

```rust
mod common;

use std::collections::BTreeMap;

use outrider_index::scan::{build_tree, scan_files};
use outrider_index::SymbolKind;

#[test]
fn scan_respects_gitignore_and_builds_sorted_tree() {
    let dir = common::copy_fixture("mini_repo");
    let files = scan_files(dir.path()).unwrap();

    let paths: Vec<String> = files
        .iter()
        .map(|f| f.rel_path.to_string_lossy().into_owned())
        .collect();
    // generated/ and *.log excluded by .gitignore; .gitignore itself is a
    // dotfile, skipped by the walker's hidden-files default.
    assert_eq!(paths, vec!["README.md", "src/lib.rs", "src/util.rs"]);

    let tree = build_tree(dir.path(), &files, &BTreeMap::new());
    let root = &tree.root;
    assert_eq!(root.id.kind, SymbolKind::Folder);
    assert_eq!(root.id.qualified_path, "");

    let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["README.md", "src"]);

    let src = &root.children[1];
    assert_eq!(src.id.kind, SymbolKind::Folder);
    assert_eq!(src.id.qualified_path, "src");
    let src_names: Vec<&str> = src.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(src_names, vec!["lib.rs", "util.rs"]);
    assert_eq!(src.children[0].id.qualified_path, "src/lib.rs");
    assert_eq!(src.children[0].id.kind, SymbolKind::File);

    // folder measure = sum of children (spec §5.2)
    assert_eq!(src.measure, src.children.iter().map(|c| c.measure).sum::<u64>());
    assert_eq!(
        root.measure,
        root.children.iter().map(|c| c.measure).sum::<u64>()
    );

    // file measure = line count; util.rs has 3 lines
    assert_eq!(src.children[1].measure, 3);
}
```

- [ ] **Step 5: Run test to verify it fails**

Run: `cargo test -p outrider-index --test scan_test`
Expected: FAIL — `scan` module not defined.

- [ ] **Step 6: Implement the scanner**

`crates/outrider-index/src/scan.rs`:

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use ignore::WalkBuilder;

use crate::types::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub rel_path: PathBuf,
    pub lines: u64,
    pub bytes: u64,
}

/// Walk the repo honoring .gitignore / standard ignore files (spec §5.1).
/// `require_git(false)` so ignore rules also apply in non-git dirs (fixtures).
/// Hidden files (dotfiles, .git) are skipped by the walker's default.
pub fn scan_files(repo_root: &Path) -> anyhow::Result<Vec<ScannedFile>> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(repo_root).require_git(false).build();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel_path = entry
            .path()
            .strip_prefix(repo_root)
            .context("walker yielded path outside repo root")?
            .to_path_buf();
        let bytes = std::fs::read(entry.path())
            .with_context(|| format!("reading {}", entry.path().display()))?;
        files.push(ScannedFile {
            rel_path,
            lines: count_lines(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(files)
}

fn count_lines(bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    if bytes.ends_with(b"\n") {
        newlines
    } else {
        newlines + 1
    }
}

/// Build the folder/file skeleton. `rs_children` maps a file's rel_path to its
/// parsed item nodes (empty map until Task 4 wires in the parser).
pub fn build_tree(
    repo_root: &Path,
    files: &[ScannedFile],
    rs_children: &BTreeMap<PathBuf, Vec<SymbolNode>>,
) -> SymbolTree {
    let root_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string());
    // decompose rel paths into components once
    let decomposed: Vec<(Vec<String>, &ScannedFile)> = files
        .iter()
        .map(|f| {
            let comps = f
                .rel_path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            (comps, f)
        })
        .collect();
    let root = build_folder(&root_name, "", &decomposed, rs_children);
    SymbolTree {
        root,
        repo_root: repo_root.to_path_buf(),
    }
}

fn build_folder(
    name: &str,
    qualified: &str,
    entries: &[(Vec<String>, &ScannedFile)],
    rs_children: &BTreeMap<PathBuf, Vec<SymbolNode>>,
) -> SymbolNode {
    let mut children: Vec<SymbolNode> = Vec::new();
    let mut by_subfolder: BTreeMap<String, Vec<(Vec<String>, &ScannedFile)>> = BTreeMap::new();

    for (comps, file) in entries {
        match comps.as_slice() {
            [file_name] => {
                let qual = join_path(qualified, file_name);
                let file_children = rs_children
                    .get(&file.rel_path)
                    .cloned()
                    .unwrap_or_default();
                children.push(SymbolNode {
                    id: SymbolId {
                        kind: SymbolKind::File,
                        qualified_path: qual,
                        ordinal: 0,
                    },
                    name: file_name.clone(),
                    byte_range: Some(0..file.bytes as usize),
                    measure: file.lines,
                    churn: 0.0,
                    churn_count: 0,
                    children: file_children,
                });
            }
            [folder, ..] => {
                by_subfolder
                    .entry(folder.clone())
                    .or_default()
                    .push((comps[1..].to_vec(), *file));
            }
            [] => {}
        }
    }

    for (folder_name, sub_entries) in &by_subfolder {
        let qual = join_path(qualified, folder_name);
        children.push(build_folder(folder_name, &qual, sub_entries, rs_children));
    }

    finalize_children(&mut children);
    let measure = children.iter().map(|c| c.measure).sum();
    SymbolNode {
        id: SymbolId {
            kind: SymbolKind::Folder,
            qualified_path: qualified.to_string(),
            ordinal: 0,
        },
        name: name.to_string(),
        byte_range: None,
        measure,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}
```

Add to `lib.rs`:

```rust
pub mod scan;
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p outrider-index`
Expected: all pass (2 from Task 1 + 1 new).

- [ ] **Step 8: Commit**

```bash
git add crates/outrider-index Cargo.lock
git commit -m "feat: ignore-aware repo scan building folder/file SymbolTree skeleton"
```

---

### Task 3: Rust item extraction with tree-sitter

**Files:**
- Create: `crates/outrider-index/src/parse.rs`
- Modify: `crates/outrider-index/src/lib.rs`

**Interfaces:**
- Consumes: `SymbolKind` from Task 1.
- Produces: `parse::RawItem { kind: SymbolKind, name: String, byte_range: Range<usize>, line_count: u64, children: Vec<RawItem> }` and `parse::parse_rust_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>` (items in source order; Task 4 converts + sorts them).

- [ ] **Step 1: Add dependencies**

```bash
cargo add -p outrider-index tree-sitter tree-sitter-rust
```

(Let cargo resolve current compatible versions. If `parser.set_language` fails to compile, check the tree-sitter version's language-binding API — recent versions use `&tree_sitter_rust::LANGUAGE.into()`.)

- [ ] **Step 2: Write the failing test**

Create `crates/outrider-index/src/parse.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SymbolKind;

    const SRC: &str = r#"mod inner {
    pub fn helper() {
        println!("help");
    }
}

struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn new() -> Self {
        Point { x: 0, y: 0 }
    }

    fn norm(&self) -> f64 {
        ((self.x * self.x + self.y * self.y) as f64).sqrt()
    }
}

fn free() {
    let _ = Point::new();
}
"#;

    #[test]
    fn extracts_nested_items_with_names_kinds_measures() {
        let items = parse_rust_items(SRC.as_bytes()).unwrap();
        let summary: Vec<(SymbolKind, &str, usize)> = items
            .iter()
            .map(|i| (i.kind, i.name.as_str(), i.children.len()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (SymbolKind::Module, "inner", 1),
                (SymbolKind::Struct, "Point", 0),
                (SymbolKind::Impl, "Point", 2),
                (SymbolKind::Fn, "free", 0),
            ]
        );

        // nested fn inside mod
        assert_eq!(items[0].children[0].name, "helper");
        assert_eq!(items[0].children[0].kind, SymbolKind::Fn);

        // methods inside impl, in source order at this stage
        let methods: Vec<&str> = items[2].children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(methods, vec!["new", "norm"]);

        // line-count measures: `mod inner { ... }` spans lines 1-5
        assert_eq!(items[0].line_count, 5);
        // `fn free() { ... }` spans 3 lines
        assert_eq!(items[3].line_count, 3);
    }

    #[test]
    fn trait_impl_name_includes_trait() {
        let src = b"trait Show {}\nimpl Show for i32 {}\n";
        let items = parse_rust_items(src).unwrap();
        assert_eq!(items[0].kind, SymbolKind::Trait);
        assert_eq!(items[0].name, "Show");
        assert_eq!(items[1].kind, SymbolKind::Impl);
        assert_eq!(items[1].name, "Show for i32");
    }
}
```

Add to `lib.rs`:

```rust
pub mod parse;
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p outrider-index parse`
Expected: FAIL — `parse_rust_items` not defined.

- [ ] **Step 4: Implement extraction**

Prepend to `crates/outrider-index/src/parse.rs`:

```rust
use std::ops::Range;

use anyhow::Context;
use tree_sitter::Node;

use crate::types::SymbolKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawItem {
    pub kind: SymbolKind,
    pub name: String,
    pub byte_range: Range<usize>,
    pub line_count: u64,
    pub children: Vec<RawItem>,
}

/// Extract mod/struct/enum/trait/impl/fn items, nested per the syntax tree
/// (spec §5.2). Items are returned in source order.
pub fn parse_rust_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("loading tree-sitter-rust grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    Ok(collect_items(tree.root_node(), source))
}

fn collect_items(node: Node, src: &[u8]) -> Vec<RawItem> {
    let mut items = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(kind) = item_kind(child.kind()) {
            items.push(RawItem {
                kind,
                name: item_name(child, src),
                byte_range: child.byte_range(),
                line_count: (child.end_position().row - child.start_position().row + 1) as u64,
                children: collect_items(child, src),
            });
        } else {
            items.extend(collect_items(child, src));
        }
    }
    items
}

fn item_kind(node_kind: &str) -> Option<SymbolKind> {
    match node_kind {
        "mod_item" => Some(SymbolKind::Module),
        "struct_item" => Some(SymbolKind::Struct),
        "enum_item" => Some(SymbolKind::Enum),
        "trait_item" => Some(SymbolKind::Trait),
        "impl_item" => Some(SymbolKind::Impl),
        "function_item" => Some(SymbolKind::Fn),
        _ => None,
    }
}

fn node_text(node: Node, src: &[u8]) -> String {
    String::from_utf8_lossy(&src[node.byte_range()]).into_owned()
}

fn item_name(node: Node, src: &[u8]) -> String {
    if node.kind() == "impl_item" {
        let ty = node
            .child_by_field_name("type")
            .map(|n| node_text(n, src))
            .unwrap_or_default();
        return match node.child_by_field_name("trait") {
            Some(tr) => format!("{} for {}", node_text(tr, src), ty),
            None => ty,
        };
    }
    node.child_by_field_name("name")
        .map(|n| node_text(n, src))
        .unwrap_or_else(|| "<anon>".to_string())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p outrider-index`
Expected: all pass. If the impl-name assertion fails on grammar-node details, inspect the actual CST with `tree-sitter parse` or a debug print of `child.kind()` — field names are `type` and `trait` in tree-sitter-rust's `impl_item`.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider-index Cargo.lock
git commit -m "feat: tree-sitter extraction of nested rust items with line measures"
```

---

### Task 4: Wire parsing into the tree (parallel with rayon)

**Files:**
- Create: `crates/outrider-index/src/index.rs`
- Create: `crates/outrider-index/tests/index_test.rs`
- Modify: `crates/outrider-index/src/lib.rs`

**Interfaces:**
- Consumes: `scan_files`/`build_tree`/`ScannedFile` (Task 2), `parse_rust_items`/`RawItem` (Task 3), `finalize_children` (Task 1).
- Produces: `index::index_repo(repo_root: &Path) -> anyhow::Result<SymbolTree>` — the crate's main entry point (Task 5 adds churn inside it; Task 6 and Phase 3 call it). Item nodes carry qualified paths like `src/lib.rs::inner::helper`.

- [ ] **Step 1: Add dependency**

```bash
cargo add -p outrider-index rayon
```

- [ ] **Step 2: Write the failing test**

`crates/outrider-index/tests/index_test.rs`:

```rust
mod common;

use outrider_index::{index_repo, SymbolKind, SymbolNode};

fn find<'a>(node: &'a SymbolNode, qual: &str) -> Option<&'a SymbolNode> {
    if node.id.qualified_path == qual {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, qual))
}

#[test]
fn index_repo_parses_rust_files_into_items() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path()).unwrap();

    let lib = find(&tree.root, "src/lib.rs").expect("src/lib.rs node");
    assert_eq!(lib.id.kind, SymbolKind::File);

    // file children are name-sorted (spec §4.1), not source-ordered:
    // Point (impl), Point (struct), free, inner  -> sorted byte-wise:
    // "Point"(impl? struct?) ties resolved by source order via ordinal
    let kids: Vec<(&str, SymbolKind, u16)> = lib
        .children
        .iter()
        .map(|c| (c.name.as_str(), c.id.kind, c.id.ordinal))
        .collect();
    assert_eq!(
        kids,
        vec![
            ("Point", SymbolKind::Struct, 0), // struct appears before impl in source
            ("Point", SymbolKind::Impl, 1),
            ("free", SymbolKind::Fn, 0),
            ("inner", SymbolKind::Module, 0),
        ]
    );

    // nesting + qualified paths
    let helper = find(&tree.root, "src/lib.rs::inner::helper").expect("nested fn");
    assert_eq!(helper.id.kind, SymbolKind::Fn);
    assert!(helper.byte_range.is_some());

    let norm = find(&tree.root, "src/lib.rs::Point::norm").expect("method");
    assert_eq!(norm.id.kind, SymbolKind::Fn);
    assert_eq!(norm.measure, 3); // 3-line method body span

    // ignored file contributed nothing (spec §8.2)
    assert!(find(&tree.root, "generated/junk.rs").is_none());

    // util.rs has its free fn
    let clamp = find(&tree.root, "src/util.rs::clamp").expect("clamp fn");
    assert_eq!(clamp.measure, 3);
}
```

Note on the duplicate-name expectation: `struct Point` and `impl Point` share a scope and a name, so they disambiguate by ordinal in source order (interpretation decision 4): struct (earlier in source) gets ordinal 0, impl gets 1.

Note on `src/lib.rs::Point::norm`: **two** fixture nodes are named `Point` (`src/lib.rs::Point` the struct and `src/lib.rs::Point` the impl — same qualified path, different kind/ordinal). `norm`'s parent is the impl, so its qualified path is built from the impl's `Point`. Qualified paths of *items* are name-joins, so the struct/impl distinction lives in `(kind, ordinal)`, not the path string — exactly why `SymbolId` is the triple, not the bare path (spec §4.1).

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p outrider-index --test index_test`
Expected: FAIL — `index_repo` not defined.

- [ ] **Step 4: Implement `index_repo`**

`crates/outrider-index/src/index.rs`:

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rayon::prelude::*;

use crate::parse::{parse_rust_items, RawItem};
use crate::scan::{build_tree, scan_files, ScannedFile};
use crate::types::{finalize_children, SymbolId, SymbolNode, SymbolTree};

pub fn index_repo(repo_root: &Path) -> anyhow::Result<SymbolTree> {
    let files = scan_files(repo_root)?;
    let rs_children = parse_all_rust(repo_root, &files)?;
    Ok(build_tree(repo_root, &files, &rs_children))
}

/// Parse every .rs file in parallel (spec §5.2: rayon, whole repo at startup).
fn parse_all_rust(
    repo_root: &Path,
    files: &[ScannedFile],
) -> anyhow::Result<BTreeMap<PathBuf, Vec<SymbolNode>>> {
    files
        .par_iter()
        .filter(|f| f.rel_path.extension().is_some_and(|e| e == "rs"))
        .map(|f| {
            let source = std::fs::read(repo_root.join(&f.rel_path))
                .with_context(|| format!("reading {}", f.rel_path.display()))?;
            let items = parse_rust_items(&source)
                .with_context(|| format!("parsing {}", f.rel_path.display()))?;
            let file_qual = f.rel_path.to_string_lossy().replace('\\', "/");
            let mut children: Vec<SymbolNode> = items
                .into_iter()
                .map(|item| to_symbol_node(item, &file_qual))
                .collect();
            finalize_children(&mut children);
            Ok((f.rel_path.clone(), children))
        })
        .collect()
}

fn to_symbol_node(item: RawItem, parent_qual: &str) -> SymbolNode {
    let qual = format!("{parent_qual}::{}", item.name);
    let mut children: Vec<SymbolNode> = item
        .children
        .into_iter()
        .map(|c| to_symbol_node(c, &qual))
        .collect();
    finalize_children(&mut children);
    SymbolNode {
        id: SymbolId {
            kind: item.kind,
            qualified_path: qual,
            ordinal: 0,
        },
        name: item.name,
        byte_range: Some(item.byte_range),
        measure: item.line_count,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}
```

(`SymbolKind` is not needed in this module — import only `finalize_children`, `SymbolId`, `SymbolNode`, `SymbolTree` from `crate::types`.)

Update `lib.rs` to its final Task-4 form:

```rust
pub mod index;
pub mod parse;
pub mod scan;
pub mod types;

pub use index::index_repo;
pub use types::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p outrider-index`
Expected: all pass. If the ordinal assertion fails, check that `finalize_children`'s sort is the stable `sort_by` (source order must survive within same-name groups).

- [ ] **Step 6: Commit**

```bash
git add crates/outrider-index Cargo.lock
git commit -m "feat: index_repo assembles full SymbolTree with parallel rust parsing"
```

---

### Task 5: Churn — git log counts, percentiles, cache, annotation

**Files:**
- Create: `crates/outrider-index/src/churn.rs`
- Create: `crates/outrider-index/tests/churn_test.rs`
- Modify: `crates/outrider-index/src/index.rs`
- Modify: `crates/outrider-index/src/lib.rs`

**Interfaces:**
- Consumes: `SymbolTree`/`SymbolNode` (Task 1); `index_repo` (Task 4) gains churn annotation as its final step.
- Produces: `churn::commit_counts_from_log(&str) -> BTreeMap<String, u64>`, `churn::percentiles(&[u64]) -> Vec<f32>`, `churn::churn_counts(repo_root: &Path) -> anyhow::Result<BTreeMap<String, u64>>` (cached), `churn::annotate(tree: &mut SymbolTree, counts: &BTreeMap<String, u64>)`. Phase 3's Card rung reads `churn` + `churn_count` off the nodes.

**Behavior decisions:**
- Non-git directories (fixtures): `churn_counts` returns an empty map; all churn stays 0.0 — indexing must not require git history.
- Percentile definition: for value *v* among *n* values, `percentile = (count strictly below v) / (n − 1)`; single-element sets get 0.0. Ties share a percentile.
- Files are ranked among files, folders (sum of descendant counts) among folders, item nodes inherit their file's values (spec §5.4).

- [ ] **Step 1: Write the failing unit tests**

Create `crates/outrider-index/src/churn.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_commits_per_path_from_numstat_log() {
        // format=%H then numstat lines; binary files show "-\t-"
        let log = "aaaa1111\n\
                   10\t2\tsrc/main.rs\n\
                   3\t0\tREADME.md\n\
                   bbbb2222\n\
                   5\t5\tsrc/main.rs\n\
                   -\t-\tlogo.png\n\
                   cccc3333\n\
                   1\t1\tsrc/main.rs\n";
        let counts = commit_counts_from_log(log);
        assert_eq!(counts.get("src/main.rs"), Some(&3));
        assert_eq!(counts.get("README.md"), Some(&1));
        assert_eq!(counts.get("logo.png"), Some(&1));
        assert_eq!(counts.get("aaaa1111"), None);
    }

    #[test]
    fn percentiles_are_fraction_strictly_below_over_n_minus_1() {
        assert_eq!(percentiles(&[10, 20, 30, 20]), vec![0.0, 1.0 / 3.0, 1.0, 1.0 / 3.0]);
        assert_eq!(percentiles(&[7]), vec![0.0]);
        assert_eq!(percentiles(&[]), Vec::<f32>::new());
        // all equal -> everyone at 0.0
        assert_eq!(percentiles(&[4, 4, 4]), vec![0.0, 0.0, 0.0]);
    }
}
```

Add to `lib.rs`:

```rust
pub mod churn;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider-index churn`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement parsing and percentiles**

Prepend to `crates/outrider-index/src/churn.rs`:

```rust
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::types::{SymbolKind, SymbolNode, SymbolTree};

/// Parse `git log --numstat --no-renames --format=%H` output into
/// commit-count-per-path (spec §5.4).
pub fn commit_counts_from_log(log: &str) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for line in log.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue; // commit-hash line or blank
        };
        let is_stat = |s: &str| s == "-" || (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
        if is_stat(added) && is_stat(deleted) {
            *counts.entry(path.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// Percentile of each value among the slice: fraction of values strictly
/// below, over (n - 1). Ties share a value; single element -> 0.0.
pub fn percentiles(values: &[u64]) -> Vec<f32> {
    let n = values.len();
    if n <= 1 {
        return vec![0.0; n];
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    values
        .iter()
        .map(|v| sorted.partition_point(|x| x < v) as f32 / (n - 1) as f32)
        .collect()
}
```

- [ ] **Step 4: Run unit tests to verify they pass**

Run: `cargo test -p outrider-index churn`
Expected: 2 passed.

- [ ] **Step 5: Write the failing integration test (real git + cache)**

`crates/outrider-index/tests/churn_test.rs`:

```rust
mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use outrider_index::churn::churn_counts;
use outrider_index::index_repo;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn git_fixture() -> tempfile::TempDir {
    let dir = common::copy_fixture("mini_repo");
    let p = dir.path();
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "test@test"]);
    git(p, &["config", "user.name", "test"]);
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "one"]);
    // touch lib.rs twice more so it out-churns everything
    fs::write(p.join("src/lib.rs"), fs::read_to_string(p.join("src/lib.rs")).unwrap() + "\n// x\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "two"]);
    fs::write(p.join("src/lib.rs"), fs::read_to_string(p.join("src/lib.rs")).unwrap() + "// y\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "three"]);
    dir
}

#[test]
fn churn_counts_real_repo_writes_and_reuses_cache() {
    let dir = git_fixture();
    let p = dir.path();

    let counts = churn_counts(p).unwrap();
    assert_eq!(counts.get("src/lib.rs"), Some(&3));
    assert_eq!(counts.get("README.md"), Some(&1));

    let cache = p.join(".outrider/churn-cache.json");
    assert!(cache.exists(), "cache written");

    // poison the cache counts but keep the HEAD key valid? No — prove reuse
    // by asserting a second call returns identical data with the cache intact.
    let mtime = fs::metadata(&cache).unwrap().modified().unwrap();
    let again = churn_counts(p).unwrap();
    assert_eq!(again, counts);
    assert_eq!(fs::metadata(&cache).unwrap().modified().unwrap(), mtime, "cache not rewritten");
}

#[test]
fn index_repo_annotates_percentiles_and_inherits_to_methods() {
    let dir = git_fixture();
    let tree = index_repo(dir.path()).unwrap();

    fn find<'a>(
        n: &'a outrider_index::SymbolNode,
        qual: &str,
    ) -> Option<&'a outrider_index::SymbolNode> {
        if n.id.qualified_path == qual {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, qual))
    }

    let lib = find(&tree.root, "src/lib.rs").unwrap();
    let util = find(&tree.root, "src/util.rs").unwrap();
    assert_eq!(lib.churn_count, 3);
    assert_eq!(util.churn_count, 1);
    // lib.rs is the most-churned of 3 files -> percentile 1.0
    assert_eq!(lib.churn, 1.0);
    assert_eq!(util.churn, 0.0);

    // methods inherit the file's values (spec §5.4)
    let norm = find(&tree.root, "src/lib.rs::Point::norm").unwrap();
    assert_eq!(norm.churn, lib.churn);
    assert_eq!(norm.churn_count, lib.churn_count);

    // folder churn = sum of descendants, ranked among folders
    let src = find(&tree.root, "src").unwrap();
    assert_eq!(src.churn_count, 4); // 3 + 1
}

#[test]
fn non_git_dir_yields_zero_churn_not_error() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path()).unwrap();
    assert_eq!(tree.root.churn_count, 0);
}
```

- [ ] **Step 6: Run to verify failure**

Run: `cargo test -p outrider-index --test churn_test`
Expected: FAIL — `churn_counts` not defined.

- [ ] **Step 7: Implement `churn_counts`, cache, and `annotate`**

Append to `crates/outrider-index/src/churn.rs` (below `percentiles`, above the test module):

```rust
#[derive(Serialize, Deserialize)]
struct ChurnCache {
    head: String,
    counts: BTreeMap<String, u64>,
}

/// Commit counts per current path, cached in .outrider/churn-cache.json keyed
/// by HEAD (spec §5.4). Non-git dirs yield an empty map.
pub fn churn_counts(repo_root: &Path) -> anyhow::Result<BTreeMap<String, u64>> {
    let Ok(head) = git_stdout(repo_root, &["rev-parse", "HEAD"]) else {
        return Ok(BTreeMap::new()); // not a git repo, or no commits yet
    };
    let head = head.trim().to_string();

    let cache_path = repo_root.join(".outrider/churn-cache.json");
    if let Ok(bytes) = std::fs::read(&cache_path) {
        if let Ok(cache) = serde_json::from_slice::<ChurnCache>(&bytes) {
            if cache.head == head {
                return Ok(cache.counts);
            }
        }
    }

    let log = git_stdout(repo_root, &["log", "--numstat", "--no-renames", "--format=%H"])?;
    let counts = commit_counts_from_log(&log);

    std::fs::create_dir_all(cache_path.parent().expect("cache path has parent"))?;
    std::fs::write(
        &cache_path,
        serde_json::to_vec_pretty(&ChurnCache {
            head,
            counts: counts.clone(),
        })?,
    )?;
    Ok(counts)
}

fn git_stdout(repo_root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .context("spawning git")?;
    if !out.status.success() {
        bail!("git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8(out.stdout).context("git output not utf-8")?)
}

/// Annotate the tree: file counts -> percentile among files; folder counts
/// (sum of descendants) -> percentile among folders; items inherit their
/// file's values (spec §5.4).
pub fn annotate(tree: &mut SymbolTree, counts: &BTreeMap<String, u64>) {
    set_counts(&mut tree.root, counts);

    let mut file_counts = Vec::new();
    let mut folder_counts = Vec::new();
    collect_counts(&tree.root, &mut file_counts, &mut folder_counts);
    let file_pcts: BTreeMap<u64, f32> = zip_pct(&file_counts);
    let folder_pcts: BTreeMap<u64, f32> = zip_pct(&folder_counts);

    set_percentiles(&mut tree.root, &file_pcts, &folder_pcts);
}

/// Post-order: files read the counts map, folders sum descendants.
fn set_counts(node: &mut SymbolNode, counts: &BTreeMap<String, u64>) -> u64 {
    match node.id.kind {
        SymbolKind::File => {
            node.churn_count = counts.get(&node.id.qualified_path).copied().unwrap_or(0);
        }
        SymbolKind::Folder => {
            node.churn_count = node
                .children
                .iter_mut()
                .map(|c| set_counts(c, counts))
                .sum();
            return node.churn_count;
        }
        _ => {} // items are filled from their file in set_percentiles
    }
    // descend into file's items only to keep recursion uniform
    for child in &mut node.children {
        set_counts(child, counts);
    }
    node.churn_count
}

fn collect_counts(node: &SymbolNode, files: &mut Vec<u64>, folders: &mut Vec<u64>) {
    match node.id.kind {
        SymbolKind::File => files.push(node.churn_count),
        SymbolKind::Folder => folders.push(node.churn_count),
        _ => return, // items don't rank
    }
    for child in &node.children {
        collect_counts(child, files, folders);
    }
}

/// Map each distinct count to its percentile (ties share one entry).
fn zip_pct(counts: &[u64]) -> BTreeMap<u64, f32> {
    let pcts = percentiles(counts);
    counts.iter().copied().zip(pcts).collect()
}

fn set_percentiles(
    node: &mut SymbolNode,
    file_pcts: &BTreeMap<u64, f32>,
    folder_pcts: &BTreeMap<u64, f32>,
) {
    match node.id.kind {
        SymbolKind::File => {
            node.churn = file_pcts.get(&node.churn_count).copied().unwrap_or(0.0);
            let (pct, count) = (node.churn, node.churn_count);
            inherit(&mut node.children, pct, count);
            return;
        }
        SymbolKind::Folder => {
            node.churn = folder_pcts.get(&node.churn_count).copied().unwrap_or(0.0);
        }
        _ => {}
    }
    for child in &mut node.children {
        set_percentiles(child, file_pcts, folder_pcts);
    }
}

fn inherit(children: &mut [SymbolNode], pct: f32, count: u64) {
    for child in children {
        child.churn = pct;
        child.churn_count = count;
        inherit(&mut child.children, pct, count);
    }
}
```

Wire into `index_repo` in `crates/outrider-index/src/index.rs` — replace the function body:

```rust
pub fn index_repo(repo_root: &Path) -> anyhow::Result<SymbolTree> {
    let files = scan_files(repo_root)?;
    let rs_children = parse_all_rust(repo_root, &files)?;
    let mut tree = build_tree(repo_root, &files, &rs_children);
    let counts = crate::churn::churn_counts(repo_root)?;
    crate::churn::annotate(&mut tree, &counts);
    Ok(tree)
}
```

- [ ] **Step 8: Run the full suite**

Run: `cargo test -p outrider-index`
Expected: all pass, including the three churn integration tests. The `mtime` cache assertion can be flaky on coarse-grained filesystems only if the cache *were* rewritten — a rewrite is a real bug, so keep the assertion.

- [ ] **Step 9: Commit**

```bash
git add crates/outrider-index
git commit -m "feat: git churn counts with percentile annotation and HEAD-keyed cache"
```

---

### Task 6: CLI dump binary — the milestone-1 acceptance artifact

**Files:**
- Create: `crates/outrider-index/src/bin/outrider-dump.rs`
- Create: `crates/outrider-index/tests/dump_test.rs`

**Interfaces:**
- Consumes: `index_repo` and the node types.
- Produces: `outrider-dump` binary: `cargo run -p outrider-index --bin outrider-dump -- <path>` prints the indented tree with kind, name, measure, and churn readout per node.

- [ ] **Step 1: Write the failing test**

`crates/outrider-index/tests/dump_test.rs`:

```rust
mod common;

use outrider_index::index_repo;

#[test]
fn dump_format_shows_kind_name_measure_and_churn_readout() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path()).unwrap();
    let out = outrider_index::dump::render(&tree);

    // spec §5.4 inspectability: raw count and percentile, both visible
    assert!(out.contains("File util.rs"), "out was:\n{out}");
    assert!(out.contains("[3 lines"), "out was:\n{out}");
    assert!(out.contains("churn 0 · p0"), "out was:\n{out}");
    // nesting is indented: method deeper than file
    let file_line = out.lines().find(|l| l.contains("File lib.rs")).unwrap();
    let fn_line = out.lines().find(|l| l.contains("Fn norm")).unwrap();
    let indent = |s: &str| s.len() - s.trim_start().len();
    assert!(indent(fn_line) > indent(file_line));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p outrider-index --test dump_test`
Expected: FAIL — `dump` module not defined.

- [ ] **Step 3: Implement the dump renderer and binary**

Create `crates/outrider-index/src/dump.rs`:

```rust
use std::fmt::Write;

use crate::types::{SymbolNode, SymbolTree};

pub fn render(tree: &SymbolTree) -> String {
    let mut out = String::new();
    render_node(&tree.root, 0, &mut out);
    out
}

fn render_node(node: &SymbolNode, depth: usize, out: &mut String) {
    writeln!(
        out,
        "{:indent$}{:?} {} [{} lines, churn {} · p{:.0}]",
        "",
        node.id.kind,
        node.name,
        node.measure,
        node.churn_count,
        node.churn * 100.0,
        indent = depth * 2
    )
    .expect("string write");
    for child in &node.children {
        render_node(child, depth + 1, out);
    }
}
```

Add to `lib.rs`:

```rust
pub mod dump;
```

Create `crates/outrider-index/src/bin/outrider-dump.rs`:

```rust
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .map_or_else(std::env::current_dir, Ok)?;
    let tree = outrider_index::index_repo(&root)?;
    print!("{}", outrider_index::dump::render(&tree));
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider-index`
Expected: all pass.

- [ ] **Step 5: Milestone-1 acceptance — dump a real repo**

Run: `cargo run -p outrider-index --bin outrider-dump -- .`
Expected: the Outrider repo's own tree — folders, files (including this plan under `docs/`), `.rs` items nested with measures, churn counts + percentiles populated (this repo has git history). Eyeball: names sorted, no ignored paths (`target/`), measures plausible.

Run: `cargo run -p outrider-index --bin outrider-dump -- <path-to-a-larger-rust-repo>` (if one is available locally)
Expected: completes in seconds (spec §5.2 scale assumption); spot-check a known file's methods.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider-index
git commit -m "feat: outrider-dump CLI prints indexed SymbolTree (milestone 1 complete)"
```

---

## Exit gate (Phase 1 complete)

- `cargo test -p outrider-index` — all green (fixture tree, parse, churn math, churn integration, ignore handling, dump format: spec §8.2 fully covered).
- `outrider-dump` output on the real repo looks correct by inspection.
- Hand `SymbolTree`/`SymbolNode`/`SymbolId` to Phase 2 (`outrider-layout`) as-is; note for Phase 2: layout consumes only `id`, `name` (ordering already baked), `measure`, and `children`.
