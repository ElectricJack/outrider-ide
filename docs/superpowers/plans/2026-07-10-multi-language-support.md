# Multi-Language Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add symbol extraction for C, Python, JavaScript, TypeScript, and C# so the treemap shows meaningful structure for mixed-language repos. Refactor `SymbolKind` to a flexible `Item { label }` system so adding future languages never touches downstream code.

**Architecture:** Replace the language-specific `SymbolKind` enum variants (`Fn`, `Struct`, `Enum`, `Trait`, `Impl`, `Module`) with a single `Item { label: String }` variant. Generalize `collect_items` to accept a language-specific kind-mapping closure. Add per-language parse functions and dispatch by file extension in `index.rs`. Update `content.rs` counts to use `BTreeMap` instead of a fixed array. Add tree-sitter grammar dependencies and highlighting support.

**Tech Stack:** Rust, tree-sitter (0.26), tree-sitter-c (0.24), tree-sitter-python (0.25), tree-sitter-javascript (0.25), tree-sitter-typescript (0.23), tree-sitter-c-sharp (0.23), serde, rayon

## Global Constraints

- `SymbolKind::Folder`, `File`, and `Chunk` remain enum variants — they have structural meaning in layout, rendering, and chunking.
- All language-level concepts become `Item { label: String }` — uses `String` not `&'static str` because `SymbolTree` derives `Deserialize`.
- Labels are lowercase: `"fn"`, `"struct"`, `"class"`, `"interface"`, `"enum"`, `"trait"`, `"impl"`, `"module"`, `"namespace"`, `"type"`, `"typedef"`.
- `Chunk` keeps its `"part"` display label in `kind_counts`.
- The `_` wildcard arm in `body_lines` already handles the generic item case — it becomes `Item { .. }`.
- No C++ support (deferred per spec §7).

---

### Task 1: SymbolKind Refactor + All Downstream Updates

**Files:**
- Modify: `crates/outrider-index/src/types.rs:7-18` (SymbolKind enum)
- Modify: `crates/outrider-index/src/types.rs:77` (dedupe_ids BTreeMap key)
- Modify: `crates/outrider-index/src/types.rs:92-185` (tests)
- Modify: `crates/outrider-index/src/parse.rs:49-59` (item_kind fn)
- Modify: `crates/outrider-index/src/parse.rs:112-210` (tests)
- Modify: `crates/outrider-index/src/index.rs:1-71` (imports, parse_all_rust)
- Modify: `crates/outrider-index/src/scan.rs:69-75` (rs_children rename)
- Modify: `crates/outrider-index/src/lib.rs:11` (re-exports)
- Modify: `crates/outrider/src/content.rs:63-100` (kind_counts)
- Modify: `crates/outrider/src/content.rs:117-172` (body_lines)
- Modify: `crates/outrider/src/content.rs:174-406` (tests)
- Modify: `crates/outrider/src/buffers.rs:117-306` (tests)
- Modify: `crates/outrider/src/focus.rs:164-449` (tests)
- Modify: `crates/outrider/src/treemap.rs:757-893` (tests)
- Modify: `crates/outrider/src/world.rs:204-422` (tests)
- Modify: `crates/outrider-layout/src/pack.rs:164-167` (tests)
- Modify: `crates/outrider-index/tests/index_test.rs:1-84` (integration test)

**Interfaces:**
- Produces: `SymbolKind::Item { label: String }` variant used by all later tasks
- Produces: `kind_counts` returning `BTreeMap`-based string instead of fixed-array string

- [ ] **Step 1: Write failing test for new SymbolKind serde roundtrip**

In `crates/outrider-index/src/types.rs`, add a test that constructs an `Item { label }` node and roundtrips it through serde:

```rust
#[test]
fn item_kind_serde_roundtrip() {
    let tree = SymbolTree {
        root: SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Item { label: "fn".into() },
                qualified_path: "f.rs::main".into(),
                ordinal: 0,
            },
            name: "main".to_string(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        },
        repo_root: std::path::PathBuf::from("/tmp/x"),
    };
    let json = serde_json::to_string(&tree).unwrap();
    let back: SymbolTree = serde_json::from_str(&json).unwrap();
    assert_eq!(tree, back);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p outrider-index item_kind_serde_roundtrip 2>&1 | tail -20`
Expected: FAIL — `Item` variant doesn't exist yet.

- [ ] **Step 3: Refactor SymbolKind enum**

In `crates/outrider-index/src/types.rs`, replace the enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Folder,
    File,
    Chunk,
    Item { label: String },
}
```

Note: `Copy` is removed (the old enum derived `Copy`, but `String` is not `Copy`).

- [ ] **Step 4: Fix all compilation errors from the SymbolKind change**

This is a mechanical find-and-replace across the codebase. The changes fall into these categories:

**A. `parse.rs` — `item_kind` function (line 49):**

```rust
fn item_kind(node_kind: &str) -> Option<SymbolKind> {
    match node_kind {
        "mod_item" => Some(SymbolKind::Item { label: "module".into() }),
        "struct_item" => Some(SymbolKind::Item { label: "struct".into() }),
        "enum_item" => Some(SymbolKind::Item { label: "enum".into() }),
        "trait_item" => Some(SymbolKind::Item { label: "trait".into() }),
        "impl_item" => Some(SymbolKind::Item { label: "impl".into() }),
        "function_item" => Some(SymbolKind::Item { label: "fn".into() }),
        _ => None,
    }
}
```

**B. `content.rs` — `kind_counts` function (line 63):**

Replace the fixed-array counting with `BTreeMap`:

```rust
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
    fn count(node: &SymbolNode, counts: &mut std::collections::BTreeMap<String, usize>) {
        for k in &node.children {
            match &k.id.kind {
                SymbolKind::Item { label } => *counts.entry(label.clone()).or_insert(0) += 1,
                SymbolKind::Chunk => *counts.entry("part".to_string()).or_insert(0) += 1,
                SymbolKind::File | SymbolKind::Folder => {}
            }
            count(k, counts);
        }
    }
    let mut counts = std::collections::BTreeMap::new();
    count(node, &mut counts);
    counts
        .iter()
        .filter(|(_, &n)| n > 0)
        .map(|(w, &n)| plural(n, w))
        .collect::<Vec<_>>()
        .join(" · ")
}
```

**C. `content.rs` — `body_lines` function (line 121):**

Replace the `Detail`/`Full` match arms. The `_` wildcard already handles the generic case. Replace it with `Item { .. }`:

```rust
SymbolKind::Chunk => vec![BodyLine::Dim(churn_readout(node))],
SymbolKind::Item { .. } => {
    let mut out = Vec::new();
    if let Some(sig) = &node.signature {
        out.push(BodyLine::Plain(sig.clone()));
    }
    if rung == Rung::Full && !node.children.is_empty() {
        out.push(BodyLine::Dim(inventory(node)));
    }
    out
}
```

**D. `types.rs` — `dedupe_ids` inner function (line 77):**

The `BTreeMap` key uses `SymbolKind` which no longer derives `Copy`. Change the key clone:

```rust
let next = seen
    .entry((node.id.kind.clone(), node.id.qualified_path.clone()))
    .or_insert(0);
```

**E. All test fixtures across every file** — mechanical replacement:

| Old | New |
|-----|-----|
| `SymbolKind::Fn` | `SymbolKind::Item { label: "fn".into() }` |
| `SymbolKind::Struct` | `SymbolKind::Item { label: "struct".into() }` |
| `SymbolKind::Enum` | `SymbolKind::Item { label: "enum".into() }` |
| `SymbolKind::Trait` | `SymbolKind::Item { label: "trait".into() }` |
| `SymbolKind::Impl` | `SymbolKind::Item { label: "impl".into() }` |
| `SymbolKind::Module` | `SymbolKind::Item { label: "module".into() }` |

Files with test fixtures to update:
- `crates/outrider-index/src/types.rs` — `mk()` uses `SymbolKind::Impl`, `mk_mod()` uses `Module` and `Fn`
- `crates/outrider-index/src/parse.rs` — test assertions use `SymbolKind::Module`, `Struct`, `Impl`, `Fn`, `Trait`
- `crates/outrider-index/tests/index_test.rs` — assertions use `Struct`, `Impl`, `Fn`, `Module`
- `crates/outrider/src/content.rs` — `file()` helper uses `Struct`, `Impl`, `Fn`; test assertions use string `"3 fns · 1 struct · 1 impl"` which becomes `"1 fn · 1 impl · 1 struct"` (BTreeMap sorts alphabetically, and singular "fn" not "fns" when count is 1... actually 3 fns stays plural). The new BTreeMap order is alphabetical: `"3 fns · 1 impl · 1 struct"`.
- `crates/outrider/src/buffers.rs` — `fn_id()` uses `Fn`, tests use `Impl`, `Fn`, `Chunk`
- `crates/outrider/src/focus.rs` — `n()` and `id()` use `Folder`, `File`, `Fn`
- `crates/outrider/src/world.rs` — `n()` uses `Folder`, `File`, `Fn`
- `crates/outrider/src/treemap.rs` — `node()` uses `File`, `Fn`
- `crates/outrider-layout/src/pack.rs` — tests use `Fn`

Also fix any `kind: SymbolKind::Fn` in function parameters — since `Copy` is gone, pattern matching now requires `ref` or cloning. Equality comparisons like `node.id.kind == SymbolKind::Fn` become `node.id.kind == SymbolKind::Item { label: "fn".into() }`. However, for `Folder`, `File`, `Chunk` comparisons stay the same.

Note on test assertion string changes: The BTreeMap-based `kind_counts` sorts labels alphabetically, so the output order changes from `"3 fns · 1 struct · 1 impl"` to `"3 fns · 1 impl · 1 struct"`. Update all test assertions accordingly.

**F. `scan.rs` — rename `rs_children` to `parsed_children`:**

In `build_tree` (line 71-97) and `build_folder` (line 99-195), rename the parameter `rs_children` to `parsed_children`. Also update the call site in `index.rs` (line 14).

- [ ] **Step 5: Run full test suite to verify everything passes**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: All tests pass. If any assertion strings changed due to alphabetical reordering of kind_counts, fix them.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider-index/src/types.rs crates/outrider-index/src/parse.rs crates/outrider-index/src/index.rs crates/outrider-index/src/scan.rs crates/outrider-index/src/lib.rs crates/outrider-index/tests/index_test.rs crates/outrider/src/content.rs crates/outrider/src/buffers.rs crates/outrider/src/focus.rs crates/outrider/src/treemap.rs crates/outrider/src/world.rs crates/outrider-layout/src/pack.rs
git commit -m "refactor: replace SymbolKind language variants with Item { label: String }"
```

---

### Task 2: Generalize Parser + Add C Support

**Files:**
- Modify: `crates/outrider-index/Cargo.toml` (add `tree-sitter-c`)
- Modify: `crates/outrider-index/src/parse.rs` (generalize `collect_items`, add `parse_c_items`)
- Modify: `crates/outrider-index/src/index.rs` (rename `parse_all_rust` → `parse_all`, add `.c`/`.h` dispatch)
- Modify: `crates/outrider-index/src/buffer.rs` (add C highlighting)

**Interfaces:**
- Consumes: `SymbolKind::Item { label: String }` from Task 1
- Produces: `parse_c_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>` — used by `parse_all`
- Produces: `collect_items(node: Node, src: &[u8], kind_fn: &dyn Fn(&str, Node, &[u8]) -> Option<&'static str>) -> Vec<RawItem>` — used by all language parsers
- Produces: `item_name_default(node: Node, src: &[u8]) -> String` — default name extraction, used by all language parsers

- [ ] **Step 1: Add tree-sitter-c dependency**

In `crates/outrider-index/Cargo.toml`, add to `[dependencies]`:

```toml
tree-sitter-c = "0.24.2"
```

- [ ] **Step 2: Write the failing C parser test**

In `crates/outrider-index/src/parse.rs`, add:

```rust
#[test]
fn extracts_c_items() {
    let src = br#"
struct Point {
    int x;
    int y;
};

enum Color { RED, GREEN, BLUE };

typedef unsigned long ulong;

void draw(struct Point p) {
    // body
}
"#;
    let items = parse_c_items(src).unwrap();
    let summary: Vec<(&str, &str)> = items
        .iter()
        .map(|i| (i.kind.label(), i.name.as_str()))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("struct", "Point"),
            ("enum", "Color"),
            ("typedef", "ulong"),
            ("fn", "draw"),
        ]
    );
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p outrider-index extracts_c_items 2>&1 | tail -20`
Expected: FAIL — `parse_c_items` and `label()` don't exist yet.

- [ ] **Step 4: Add `label()` method to SymbolKind**

In `crates/outrider-index/src/types.rs`, add:

```rust
impl SymbolKind {
    pub fn label(&self) -> &str {
        match self {
            SymbolKind::Folder => "folder",
            SymbolKind::File => "file",
            SymbolKind::Chunk => "chunk",
            SymbolKind::Item { label } => label,
        }
    }
}
```

- [ ] **Step 5: Generalize `collect_items` to accept a `kind_fn` closure**

In `crates/outrider-index/src/parse.rs`, refactor `collect_items` and rename the existing Rust-specific `item_kind` to `rust_kind_fn`. Also extract `item_name` into `item_name_default` (for most languages) and keep the Rust-specific `item_name` as `rust_item_name`:

```rust
fn collect_items(
    node: Node,
    src: &[u8],
    kind_fn: &dyn Fn(&str, Node, &[u8]) -> Option<&'static str>,
    name_fn: &dyn Fn(Node, &[u8]) -> String,
) -> Vec<RawItem> {
    let mut items = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(label) = kind_fn(child.kind(), child, src) {
            items.push(RawItem {
                kind: SymbolKind::Item { label: label.into() },
                name: name_fn(child, src),
                signature: item_signature(child, src),
                byte_range: child.byte_range(),
                line_count: (child.end_position().row - child.start_position().row + 1) as u64,
                children: collect_items(child, src, kind_fn, name_fn),
            });
        } else {
            items.extend(collect_items(child, src, kind_fn, name_fn));
        }
    }
    items
}

fn item_name_default(node: Node, src: &[u8]) -> String {
    node.child_by_field_name("name")
        .map(|n| node_text(n, src))
        .unwrap_or_else(|| "<anon>".to_string())
}

fn rust_item_name(node: Node, src: &[u8]) -> String {
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
    item_name_default(node, src)
}
```

Update `parse_rust_items` to use the generalized `collect_items`:

```rust
pub fn parse_rust_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("loading tree-sitter-rust grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, _node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "mod_item" => Some("module"),
            "struct_item" => Some("struct"),
            "enum_item" => Some("enum"),
            "trait_item" => Some("trait"),
            "impl_item" => Some("impl"),
            "function_item" => Some("fn"),
            _ => None,
        }
    };
    Ok(collect_items(tree.root_node(), source, &kind_fn, &rust_item_name))
}
```

- [ ] **Step 6: Implement `parse_c_items`**

In `crates/outrider-index/src/parse.rs`, add:

```rust
pub fn parse_c_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_c::LANGUAGE.into())
        .context("loading tree-sitter-c grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "function_definition" => Some("fn"),
            "struct_specifier" if node.child_by_field_name("body").is_some() => Some("struct"),
            "enum_specifier" if node.child_by_field_name("body").is_some() => Some("enum"),
            "type_definition" => Some("typedef"),
            _ => None,
        }
    };
    Ok(collect_items(tree.root_node(), source, &kind_fn, &item_name_default))
}
```

- [ ] **Step 7: Add C/H dispatch to index.rs**

In `crates/outrider-index/src/index.rs`:

Rename `parse_all_rust` to `parse_all`. Change the filter to accept multiple extensions, and dispatch to the right parser:

```rust
use crate::parse::{parse_rust_items, parse_c_items, RawItem};

fn parse_all(
    repo_root: &Path,
    files: &[ScannedFile],
) -> anyhow::Result<BTreeMap<PathBuf, ParsedFile>> {
    files
        .par_iter()
        .filter_map(|f| {
            let ext = f.rel_path.extension()?.to_str()?;
            let parser: fn(&[u8]) -> anyhow::Result<Vec<RawItem>> = match ext {
                "rs" => parse_rust_items,
                "c" | "h" => parse_c_items,
                _ => return None,
            };
            Some((f, parser))
        })
        .map(|(f, parser)| {
            let source = std::fs::read(repo_root.join(&f.rel_path))
                .with_context(|| format!("reading {}", f.rel_path.display()))?;
            let items = parser(&source)
                .with_context(|| format!("parsing {}", f.rel_path.display()))?;
            let file_qual = f.rel_path.to_string_lossy().replace('\\', "/");
            let mut children: Vec<SymbolNode> = items
                .into_iter()
                .map(|item| to_symbol_node(item, &file_qual))
                .collect();
            finalize_children(&mut children);
            let doc = if f.rel_path.extension().is_some_and(|e| e == "rs") {
                crate::parse::file_doc(&source)
            } else {
                None
            };
            Ok((f.rel_path.clone(), ParsedFile { items: children, doc }))
        })
        .collect()
}
```

Update `index_repo` to call `parse_all` instead of `parse_all_rust`, and update `scan.rs` to rename `rs_children` → `parsed_children` in `build_tree` and `build_folder`.

- [ ] **Step 8: Add C highlighting to buffer.rs**

In `crates/outrider-index/src/buffer.rs`, add to the `match ext` in `FileBuffer::new`:

```rust
"c" | "h" => Some((tree_sitter_c::LANGUAGE.into(), tree_sitter_c::HIGHLIGHTS_QUERY)),
```

- [ ] **Step 9: Run tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: All tests pass, including `extracts_c_items`.

- [ ] **Step 10: Commit**

```bash
git add crates/outrider-index/Cargo.toml crates/outrider-index/src/types.rs crates/outrider-index/src/parse.rs crates/outrider-index/src/index.rs crates/outrider-index/src/scan.rs crates/outrider-index/src/buffer.rs
git commit -m "feat: generalize parser architecture and add C language support"
```

---

### Task 3: Python Support

**Files:**
- Modify: `crates/outrider-index/Cargo.toml` (add `tree-sitter-python`)
- Modify: `crates/outrider-index/src/parse.rs` (add `parse_python_items`)
- Modify: `crates/outrider-index/src/index.rs` (add `.py` dispatch)
- Modify: `crates/outrider-index/src/buffer.rs` (add Python highlighting)

**Interfaces:**
- Consumes: generalized `collect_items` from Task 2
- Produces: `parse_python_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>` — used by `parse_all`

- [ ] **Step 1: Add tree-sitter-python dependency**

In `crates/outrider-index/Cargo.toml`, add:

```toml
tree-sitter-python = "0.25.0"
```

- [ ] **Step 2: Write failing test**

In `crates/outrider-index/src/parse.rs`, add:

```rust
#[test]
fn extracts_python_items() {
    let src = br#"
class Animal:
    def speak(self):
        pass

    def eat(self):
        pass

def standalone():
    pass

@staticmethod
def decorated():
    pass
"#;
    let items = parse_python_items(src).unwrap();
    let summary: Vec<(&str, &str, usize)> = items
        .iter()
        .map(|i| (i.kind.label(), i.name.as_str(), i.children.len()))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("class", "Animal", 2),
            ("fn", "standalone", 0),
            ("fn", "decorated", 0),
        ]
    );
    assert_eq!(items[0].children[0].name, "speak");
    assert_eq!(items[0].children[1].name, "eat");
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p outrider-index extracts_python_items 2>&1 | tail -20`
Expected: FAIL — `parse_python_items` doesn't exist.

- [ ] **Step 4: Implement `parse_python_items`**

In `crates/outrider-index/src/parse.rs`, add:

```rust
pub fn parse_python_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .context("loading tree-sitter-python grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, _node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "function_definition" => Some("fn"),
            "class_definition" => Some("class"),
            "decorated_definition" => None,
            _ => None,
        }
    };
    Ok(collect_items(tree.root_node(), source, &kind_fn, &item_name_default))
}
```

The `decorated_definition` maps to `None` so `collect_items` recurses into it and finds the inner `function_definition` or `class_definition`. The decorator's byte range is the inner item's range. If you need the decorator to wrap the inner item (capturing the full byte range including decorators), instead handle it in the `kind_fn`:

```rust
let kind_fn = |node_kind: &str, node: Node, src: &[u8]| -> Option<&'static str> {
    match node_kind {
        "function_definition" => Some("fn"),
        "class_definition" => Some("class"),
        "decorated_definition" => {
            // Unwrap: find the inner definition's kind
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "function_definition" => return Some("fn"),
                    "class_definition" => return Some("class"),
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
};
```

Use the name function that unwraps decorated definitions:

```rust
fn python_item_name(node: Node, src: &[u8]) -> String {
    if node.kind() == "decorated_definition" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "function_definition" || child.kind() == "class_definition" {
                return item_name_default(child, src);
            }
        }
    }
    item_name_default(node, src)
}
```

Then pass `&python_item_name` as the name function:

```rust
Ok(collect_items(tree.root_node(), source, &kind_fn, &python_item_name))
```

- [ ] **Step 5: Add `.py` dispatch to index.rs**

In the `parse_all` match, add:

```rust
"py" => parse_python_items,
```

And add the import: `use crate::parse::{..., parse_python_items};`

- [ ] **Step 6: Add Python highlighting to buffer.rs**

```rust
"py" => Some((tree_sitter_python::LANGUAGE.into(), tree_sitter_python::HIGHLIGHTS_QUERY)),
```

- [ ] **Step 7: Run tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: All pass.

- [ ] **Step 8: Commit**

```bash
git add crates/outrider-index/Cargo.toml crates/outrider-index/src/parse.rs crates/outrider-index/src/index.rs crates/outrider-index/src/buffer.rs
git commit -m "feat: add Python language support"
```

---

### Task 4: JavaScript Support

**Files:**
- Modify: `crates/outrider-index/Cargo.toml` (add `tree-sitter-javascript`)
- Modify: `crates/outrider-index/src/parse.rs` (add `parse_js_items`)
- Modify: `crates/outrider-index/src/index.rs` (add `.js`/`.jsx` dispatch)
- Modify: `crates/outrider-index/src/buffer.rs` (add JS highlighting)

**Interfaces:**
- Consumes: generalized `collect_items` from Task 2
- Produces: `parse_js_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>` — used by `parse_all` and by Task 5 (TS reuses JS kind_fn)

- [ ] **Step 1: Add tree-sitter-javascript dependency**

In `crates/outrider-index/Cargo.toml`, add:

```toml
tree-sitter-javascript = "0.25.0"
```

- [ ] **Step 2: Write failing test**

In `crates/outrider-index/src/parse.rs`, add:

```rust
#[test]
fn extracts_js_items() {
    let src = br#"
function greet(name) {
    return "hello " + name;
}

class Greeter {
    constructor(name) {
        this.name = name;
    }
    greet() {
        return "hello " + this.name;
    }
}

const add = (a, b) => a + b;

export function exported() {}
"#;
    let items = parse_js_items(src).unwrap();
    let summary: Vec<(&str, &str, usize)> = items
        .iter()
        .map(|i| (i.kind.label(), i.name.as_str(), i.children.len()))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("fn", "greet", 0),
            ("class", "Greeter", 2),
            ("fn", "add", 0),
            ("fn", "exported", 0),
        ]
    );
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p outrider-index extracts_js_items 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 4: Implement `parse_js_items`**

In `crates/outrider-index/src/parse.rs`, add:

```rust
fn js_kind_fn(node_kind: &str, node: Node, src: &[u8]) -> Option<&'static str> {
    match node_kind {
        "function_declaration" | "generator_function_declaration" => Some("fn"),
        "class_declaration" => Some("class"),
        "method_definition" => Some("fn"),
        "lexical_declaration" | "variable_declaration" => {
            // Named arrow functions: const add = (a, b) => ...
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "variable_declarator" {
                    if let Some(value) = child.child_by_field_name("value") {
                        if value.kind() == "arrow_function" || value.kind() == "function" {
                            return Some("fn");
                        }
                    }
                }
            }
            None
        }
        "export_statement" => None, // recurse into inner declaration
        _ => None,
    }
}

fn js_item_name(node: Node, src: &[u8]) -> String {
    if node.kind() == "lexical_declaration" || node.kind() == "variable_declaration" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "variable_declarator" {
                return child
                    .child_by_field_name("name")
                    .map(|n| node_text(n, src))
                    .unwrap_or_else(|| "<anon>".to_string());
            }
        }
    }
    item_name_default(node, src)
}

pub fn parse_js_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .context("loading tree-sitter-javascript grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    Ok(collect_items(tree.root_node(), source, &js_kind_fn, &js_item_name))
}
```

- [ ] **Step 5: Add `.js`/`.jsx` dispatch to index.rs**

In the `parse_all` match, add:

```rust
"js" | "jsx" => parse_js_items,
```

And add the import.

- [ ] **Step 6: Add JS highlighting to buffer.rs**

```rust
"js" | "jsx" => Some((tree_sitter_javascript::LANGUAGE.into(), tree_sitter_javascript::HIGHLIGHTS_QUERY)),
```

- [ ] **Step 7: Run tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: All pass.

- [ ] **Step 8: Commit**

```bash
git add crates/outrider-index/Cargo.toml crates/outrider-index/src/parse.rs crates/outrider-index/src/index.rs crates/outrider-index/src/buffer.rs
git commit -m "feat: add JavaScript language support"
```

---

### Task 5: TypeScript Support

**Files:**
- Modify: `crates/outrider-index/Cargo.toml` (add `tree-sitter-typescript`)
- Modify: `crates/outrider-index/src/parse.rs` (add `parse_ts_items`, `parse_tsx_items`)
- Modify: `crates/outrider-index/src/index.rs` (add `.ts`/`.tsx` dispatch)
- Modify: `crates/outrider-index/src/buffer.rs` (add TS/TSX highlighting)

**Interfaces:**
- Consumes: `js_kind_fn` and `js_item_name` from Task 4
- Produces: `parse_ts_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>` and `parse_tsx_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>`

- [ ] **Step 1: Add tree-sitter-typescript dependency**

In `crates/outrider-index/Cargo.toml`, add:

```toml
tree-sitter-typescript = "0.23.2"
```

- [ ] **Step 2: Write failing test**

In `crates/outrider-index/src/parse.rs`, add:

```rust
#[test]
fn extracts_ts_items() {
    let src = br#"
function greet(name: string): string {
    return "hello " + name;
}

class Greeter {
    greet(): string {
        return "hello";
    }
}

interface Printable {
    print(): void;
}

enum Direction {
    Up,
    Down,
}

type UserId = string;

const add = (a: number, b: number): number => a + b;
"#;
    let items = parse_ts_items(src).unwrap();
    let summary: Vec<(&str, &str)> = items
        .iter()
        .map(|i| (i.kind.label(), i.name.as_str()))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("fn", "greet"),
            ("class", "Greeter"),
            ("interface", "Printable"),
            ("enum", "Direction"),
            ("type", "UserId"),
            ("fn", "add"),
        ]
    );
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p outrider-index extracts_ts_items 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 4: Implement `parse_ts_items` and `parse_tsx_items`**

In `crates/outrider-index/src/parse.rs`, add:

```rust
fn ts_kind_fn(node_kind: &str, node: Node, src: &[u8]) -> Option<&'static str> {
    match node_kind {
        "interface_declaration" => Some("interface"),
        "enum_declaration" => Some("enum"),
        "type_alias_declaration" => Some("type"),
        _ => js_kind_fn(node_kind, node, src),
    }
}

pub fn parse_ts_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .context("loading tree-sitter-typescript grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    Ok(collect_items(tree.root_node(), source, &ts_kind_fn, &js_item_name))
}

pub fn parse_tsx_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
        .context("loading tree-sitter-tsx grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    Ok(collect_items(tree.root_node(), source, &ts_kind_fn, &js_item_name))
}
```

Note: `tree-sitter-typescript` exports `LANGUAGE_TYPESCRIPT` and `LANGUAGE_TSX` as separate grammars.

- [ ] **Step 5: Add `.ts`/`.tsx` dispatch to index.rs**

In the `parse_all` match, add:

```rust
"ts" => parse_ts_items,
"tsx" => parse_tsx_items,
```

And add imports.

- [ ] **Step 6: Add TS/TSX highlighting to buffer.rs**

```rust
"ts" => Some((tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), tree_sitter_typescript::HIGHLIGHTS_QUERY)),
"tsx" => Some((tree_sitter_typescript::LANGUAGE_TSX.into(), tree_sitter_typescript::HIGHLIGHTS_QUERY)),
```

Note: Check the crate's exports — it may be `HIGHLIGHT_QUERY` (singular) or `HIGHLIGHTS_QUERY`. The TypeScript crate typically exports a shared highlights query for both TS and TSX. If the crate doesn't export a highlights query constant, use `tree_sitter_typescript::HIGHLIGHTS_QUERY` or fall back to an empty string (plain mode).

- [ ] **Step 7: Run tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: All pass.

- [ ] **Step 8: Commit**

```bash
git add crates/outrider-index/Cargo.toml crates/outrider-index/src/parse.rs crates/outrider-index/src/index.rs crates/outrider-index/src/buffer.rs
git commit -m "feat: add TypeScript and TSX language support"
```

---

### Task 6: C# Support

**Files:**
- Modify: `crates/outrider-index/Cargo.toml` (add `tree-sitter-c-sharp`)
- Modify: `crates/outrider-index/src/parse.rs` (add `parse_csharp_items`)
- Modify: `crates/outrider-index/src/index.rs` (add `.cs` dispatch)
- Modify: `crates/outrider-index/src/buffer.rs` (add C# highlighting)

**Interfaces:**
- Consumes: generalized `collect_items` from Task 2
- Produces: `parse_csharp_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>`

- [ ] **Step 1: Add tree-sitter-c-sharp dependency**

In `crates/outrider-index/Cargo.toml`, add:

```toml
tree-sitter-c-sharp = "0.23.5"
```

- [ ] **Step 2: Write failing test**

In `crates/outrider-index/src/parse.rs`, add:

```rust
#[test]
fn extracts_csharp_items() {
    let src = br#"
namespace MyApp {
    class Greeter {
        public Greeter() {
        }
        public string Greet() {
            return "hello";
        }
    }

    interface IPrintable {
        void Print();
    }

    enum Color {
        Red,
        Green,
        Blue
    }

    struct Point {
        public int X;
        public int Y;
    }

    record Person(string Name, int Age);
}
"#;
    let items = parse_csharp_items(src).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind.label(), "namespace");
    assert_eq!(items[0].name, "MyApp");
    let inner: Vec<(&str, &str)> = items[0]
        .children
        .iter()
        .map(|i| (i.kind.label(), i.name.as_str()))
        .collect();
    assert_eq!(
        inner,
        vec![
            ("class", "Greeter"),
            ("interface", "IPrintable"),
            ("enum", "Color"),
            ("struct", "Point"),
            ("class", "Person"),
        ]
    );
    // Greeter has constructor + method
    assert_eq!(items[0].children[0].children.len(), 2);
    assert_eq!(items[0].children[0].children[0].kind.label(), "fn");
    assert_eq!(items[0].children[0].children[0].name, "Greeter");
    assert_eq!(items[0].children[0].children[1].name, "Greet");
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p outrider-index extracts_csharp_items 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 4: Implement `parse_csharp_items`**

In `crates/outrider-index/src/parse.rs`, add:

```rust
pub fn parse_csharp_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .context("loading tree-sitter-c-sharp grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, _node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "class_declaration" | "record_declaration" => Some("class"),
            "interface_declaration" => Some("interface"),
            "struct_declaration" => Some("struct"),
            "enum_declaration" => Some("enum"),
            "method_declaration" | "constructor_declaration" => Some("fn"),
            "namespace_declaration" => Some("namespace"),
            _ => None,
        }
    };
    Ok(collect_items(tree.root_node(), source, &kind_fn, &item_name_default))
}
```

- [ ] **Step 5: Add `.cs` dispatch to index.rs**

In the `parse_all` match, add:

```rust
"cs" => parse_csharp_items,
```

And add the import.

- [ ] **Step 6: Add C# highlighting to buffer.rs**

```rust
"cs" => Some((tree_sitter_c_sharp::LANGUAGE.into(), tree_sitter_c_sharp::HIGHLIGHTS_QUERY)),
```

Note: Check the crate's exports. If it doesn't export `HIGHLIGHTS_QUERY`, this language falls back to plain mode. In that case, omit this line.

- [ ] **Step 7: Run tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: All pass.

- [ ] **Step 8: Commit**

```bash
git add crates/outrider-index/Cargo.toml crates/outrider-index/src/parse.rs crates/outrider-index/src/index.rs crates/outrider-index/src/buffer.rs
git commit -m "feat: add C# language support"
```
