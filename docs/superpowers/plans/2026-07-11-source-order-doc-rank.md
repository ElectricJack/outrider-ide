# Source-Ordered Files and Doc-Rank Folder Packing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Children of prose/declaration-order files (markdown, text, C/C++) pack in source order at every nesting level, and doc files/folders sink below source siblings when packing a folder.

**Architecture:** All changes live in `crates/outrider-layout/src/pack.rs` (plus a one-line invariant amendment in `docs/code-comprehension-viewer-design.md`). Task 1 adds pure helpers: extension extraction from qualified paths, the two extension sets, and a recursive doc classifier. Task 2 rewires the sort in `size()`: the existing Chunk source-order branch widens to also cover children of source-ordered files, and the kind-grouped branch gains `doc_rank` as its leading key (ranks precomputed per child, never inside the comparator). Spec: `docs/superpowers/specs/2026-07-11-source-order-doc-rank-design.md`.

**Tech Stack:** Rust, cargo test. No new dependencies.

## Global Constraints

- Determinism: identical input trees must produce identical layouts (existing `deterministic` test must keep passing).
- `Chunk` children keep their existing source-order behavior; the Chunk condition takes precedence and its comparator semantics are unchanged.
- `DOC_EXTS` = exactly `md`, `markdown`, `txt`, `rst`. `SOURCE_ORDERED_EXTS` = `DOC_EXTS` ∪ exactly `c`, `h`, `cpp`, `hpp`, `cc`, `hh`, `cxx`, `hxx`, `inl`.
- Folder doc classification: **more than 70%** of files under it recursively, in exact integer form `doc_files * 10 > total_files * 7`; empty folders are not doc.
- Extension comes from the file part of a qualified path: everything before the first `::` (and before any `#` chunk suffix), then after the last `.`.
- Doc ranks are computed once per child before sorting — no tree walks inside the comparator.
- No renderer changes; `treemap.rs` is order-agnostic.
- All existing pack.rs tests keep passing unchanged.

---

### Task 1: Extension and doc-rank helpers

**Files:**
- Modify: `crates/outrider-layout/src/pack.rs` (helpers after `kind_rank`, ~line 65; tests in existing `mod tests`)

**Interfaces:**
- Consumes: `outrider_index::{SymbolKind, SymbolNode}` (already imported in pack.rs).
- Produces (all private, used by Task 2's sort):
  - `fn file_ext(qualified_path: &str) -> Option<&str>`
  - `fn is_doc_ext(ext: &str) -> bool`
  - `fn is_source_ordered_ext(ext: &str) -> bool`
  - `fn name_is_doc(name: &str) -> bool`
  - `fn doc_stats(node: &SymbolNode) -> (u64, u64)` — (doc files, total files) recursively
  - `fn doc_rank(node: &SymbolNode) -> u8` — 0 or 1

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/outrider-layout/src/pack.rs` (helpers are in scope via the existing `use super::*;`):

```rust
#[test]
fn file_ext_takes_file_part_of_qualified_path() {
    assert_eq!(file_ext("src/m.c"), Some("c"));
    assert_eq!(file_ext("src/m.c::s::field"), Some("c"));
    assert_eq!(file_ext("BIG.md#3"), Some("md"));
    assert_eq!(file_ext("README.markdown"), Some("markdown"));
    assert_eq!(file_ext("Makefile"), None);
}

#[test]
fn source_ordered_exts_cover_docs_and_c_family() {
    for e in ["md", "markdown", "txt", "rst"] {
        assert!(is_doc_ext(e), "{e} is doc");
        assert!(is_source_ordered_ext(e), "{e} is source-ordered");
    }
    for e in ["c", "h", "cpp", "hpp", "cc", "hh", "cxx", "hxx", "inl"] {
        assert!(!is_doc_ext(e), "{e} is not doc");
        assert!(is_source_ordered_ext(e), "{e} is source-ordered");
    }
    for e in ["rs", "py", "ts"] {
        assert!(!is_doc_ext(e), "{e} is not doc");
        assert!(!is_source_ordered_ext(e), "{e} reorganizes");
    }
}

#[test]
fn doc_rank_files_by_name_folders_by_recursive_share() {
    let f = |name: &str| n(SymbolKind::File, name, name, 1, vec![]);
    assert_eq!(doc_rank(&f("README.md")), 1);
    assert_eq!(doc_rank(&f("main.rs")), 0);
    // 3 of 4 files doc (75% > 70%) — doc, counted through a subfolder
    let d75 = n(
        SymbolKind::Folder,
        "d",
        "d",
        0,
        vec![
            n(SymbolKind::Folder, "d/sub", "sub", 0, vec![f("a.md"), f("b.md")]),
            f("c.md"),
            f("x.rs"),
        ],
    );
    assert_eq!(doc_rank(&d75), 1);
    // 1 of 2 (50%, not > 70%) — not doc
    let mixed = n(SymbolKind::Folder, "m", "m", 0, vec![f("a.md"), f("x.rs")]);
    assert_eq!(doc_rank(&mixed), 0);
    // empty folder — not doc
    assert_eq!(doc_rank(&n(SymbolKind::Folder, "e", "e", 0, vec![])), 0);
    // non-file/folder kinds never rank
    let it = n(SymbolKind::Item { label: "fn".into() }, "a.md::x", "x", 1, vec![]);
    assert_eq!(doc_rank(&it), 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider-layout -- file_ext_takes source_ordered_exts doc_rank_files`
Expected: COMPILE ERROR — `cannot find function file_ext` (etc.).

- [ ] **Step 3: Write the implementation**

Add after `kind_rank` (before `fn size`) in `crates/outrider-layout/src/pack.rs`:

```rust
/// Documentation file extensions: these sink below source siblings when
/// packing a folder.
fn is_doc_ext(ext: &str) -> bool {
    matches!(ext, "md" | "markdown" | "txt" | "rst")
}

/// Extensions whose files read top-to-bottom: prose plus declare-before-
/// use languages (C/C++). Their children pack in source order at every
/// nesting level instead of kind/size order.
fn is_source_ordered_ext(ext: &str) -> bool {
    is_doc_ext(ext)
        || matches!(ext, "c" | "h" | "cpp" | "hpp" | "cc" | "hh" | "cxx" | "hxx" | "inl")
}

/// Extension of the file part of a qualified path: everything before the
/// first `::` (and before any `#` chunk suffix), then after the last `.`.
fn file_ext(qualified_path: &str) -> Option<&str> {
    let file = qualified_path.split("::").next().unwrap_or(qualified_path);
    let file = file.split('#').next().unwrap_or(file);
    file.rfind('.').map(|dot| &file[dot + 1..])
}

fn name_is_doc(name: &str) -> bool {
    name.rfind('.').is_some_and(|dot| is_doc_ext(&name[dot + 1..]))
}

/// (doc files, total files) under a folder, recursively. Symbol items
/// inside files are not files and don't count.
fn doc_stats(node: &SymbolNode) -> (u64, u64) {
    let (mut doc, mut total) = (0, 0);
    for c in &node.children {
        match c.id.kind {
            SymbolKind::File => {
                total += 1;
                doc += name_is_doc(&c.name) as u64;
            }
            SymbolKind::Folder => {
                let (d, t) = doc_stats(c);
                doc += d;
                total += t;
            }
            _ => {}
        }
    }
    (doc, total)
}

/// 1 if this folder child is documentation — a doc file, or a folder
/// whose files are more than 70% doc — else 0. Doc children pack after
/// source children so source never competes with docs purely by size.
fn doc_rank(node: &SymbolNode) -> u8 {
    match node.id.kind {
        SymbolKind::File => name_is_doc(&node.name) as u8,
        SymbolKind::Folder => {
            let (doc, total) = doc_stats(node);
            (doc * 10 > total * 7) as u8
        }
        _ => 0,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider-layout`
Expected: ALL PASS (3 new + all existing). Dead-code warnings for the new helpers in non-test builds are acceptable until Task 2 wires them in.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider-layout/src/pack.rs
git commit -m "feat: extension and doc-rank helpers for packing"
```

---

### Task 2: Sort integration in `size()` + invariant amendment

**Files:**
- Modify: `crates/outrider-layout/src/pack.rs:81-108` (the sort block and the two consumers of `order` below it), plus tests in the same file.
- Modify: `docs/code-comprehension-viewer-design.md:396` (invariant #3 amendment).

**Interfaces:**
- Consumes: Task 1's `file_ext`, `is_source_ordered_ext`, `doc_rank`; existing `kind_rank`.
- Produces: no signature changes — `size()` keeps `(node, cfg, rel) -> (f64, f64)`; only child ordering changes.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/outrider-layout/src/pack.rs`:

```rust
#[test]
fn c_file_children_pack_in_source_order_not_kind_or_size() {
    // Scrambled: the tall struct is declared LAST. Kind/size order would
    // place it first (rank 0, tallest); a .c file must keep byte order.
    let item = |label: &str, qp: &str, name: &str, measure: u64, start: usize| {
        let mut it =
            n(SymbolKind::Item { label: label.into() }, qp, name, measure, vec![]);
        it.byte_range = Some(start..start + 10);
        it
    };
    let mut file = n(
        SymbolKind::File,
        "src/m.c",
        "m.c",
        60,
        vec![
            item("struct", "src/m.c::S", "S", 50, 200),
            item("fn", "src/m.c::zebra", "zebra", 2, 0),
            item("fn", "src/m.c::mid", "mid", 5, 100),
        ],
    );
    file.byte_range = Some(0..300);
    let tree = SymbolTree {
        root: n(SymbolKind::Folder, "", "", 0, vec![file]),
        repo_root: "/x".into(),
    };
    let p = pack(&tree, &cfg());
    let z = rect(&p, "src/m.c::zebra"); // byte 0
    let m = rect(&p, "src/m.c::mid"); // byte 100
    let s = rect(&p, "src/m.c::S"); // byte 200
    // zebra and mid stack in the first column in byte order; the struct —
    // which kind/size order would have placed first — packs last (wraps)
    close(z.x, m.x);
    assert!(z.y < m.y, "zebra(0) above mid(100)");
    assert!(s.x > m.x, "S(200) last despite kind rank 0 and max height");
}

#[test]
fn nested_markdown_container_keeps_source_order() {
    // A section inside a .md file: its children pack by byte offset even
    // though tallest-first would reverse them.
    let item = |qp: &str, name: &str, measure: u64, start: usize| {
        let mut it =
            n(SymbolKind::Item { label: "h2".into() }, qp, name, measure, vec![]);
        it.byte_range = Some(start..start + 10);
        it
    };
    let mut sec = n(
        SymbolKind::Item { label: "h1".into() },
        "g.md::Sec",
        "Sec",
        0,
        vec![item("g.md::Sec::zz", "zz", 2, 0), item("g.md::Sec::aa", "aa", 30, 100)],
    );
    sec.byte_range = Some(0..200);
    let mut file = n(SymbolKind::File, "g.md", "g.md", 40, vec![sec]);
    file.byte_range = Some(0..200);
    let tree = SymbolTree {
        root: n(SymbolKind::Folder, "", "", 0, vec![file]),
        repo_root: "/x".into(),
    };
    let p = pack(&tree, &cfg());
    let zz = rect(&p, "g.md::Sec::zz"); // byte 0, short
    let aa = rect(&p, "g.md::Sec::aa"); // byte 100, tall
    // byte order beats tallest-first: zz is placed first (reading order)
    assert!(zz.x < aa.x || (zz.x == aa.x && zz.y < aa.y), "zz(0) before aa(100)");
}

#[test]
fn doc_file_sinks_below_source_in_folder() {
    // README.md is far taller; size order would place it first, but doc
    // rank sinks it below the source file.
    let tree = SymbolTree {
        root: n(
            SymbolKind::Folder,
            "",
            "",
            0,
            vec![
                n(SymbolKind::File, "README.md", "README.md", 500, vec![]),
                n(SymbolKind::File, "main.rs", "main.rs", 5, vec![]),
            ],
        ),
        repo_root: "/x".into(),
    };
    let p = pack(&tree, &cfg());
    let (r, m) = (rect(&p, "README.md"), rect(&p, "main.rs"));
    // main.rs first: top-left of the content area
    close(m.x, 8.0);
    close(m.y, 60.0);
    // README wraps to the second column
    close(r.x, 496.0);
    close(r.y, 60.0);
}

#[test]
fn folder_doc_share_over_70_percent_sinks() {
    let f = |qp: &str, name: &str| n(SymbolKind::File, qp, name, 1, vec![]);
    // "a_docs" (3/4 doc, recursive through sub) sinks after "mixed"
    // (1/2 doc, not doc) even though a_docs wins BOTH fallback keys:
    // it is taller (more children) and alphabetically first.
    let docs = n(
        SymbolKind::Folder,
        "a_docs",
        "a_docs",
        0,
        vec![
            n(
                SymbolKind::Folder,
                "a_docs/sub",
                "sub",
                0,
                vec![f("a_docs/sub/a.md", "a.md"), f("a_docs/sub/b.md", "b.md")],
            ),
            f("a_docs/c.md", "c.md"),
            f("a_docs/x.rs", "x.rs"),
        ],
    );
    let mixed = n(
        SymbolKind::Folder,
        "mixed",
        "mixed",
        0,
        vec![f("mixed/a.md", "a.md"), f("mixed/x.rs", "x.rs")],
    );
    let tree = SymbolTree {
        root: n(SymbolKind::Folder, "", "", 0, vec![docs, mixed]),
        repo_root: "/x".into(),
    };
    let p = pack(&tree, &cfg());
    let (d, m) = (rect(&p, "a_docs"), rect(&p, "mixed"));
    assert!(m.x < d.x || (m.x == d.x && m.y < d.y), "mixed before a_docs");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider-layout -- c_file_children nested_markdown doc_file_sinks folder_doc_share`
Expected: 4 FAILURES — kind/size/name order places the struct first, `aa` first, `README.md` first, and `a_docs` first respectively.

- [ ] **Step 3: Implement the sort change**

In `fn size` (`crates/outrider-layout/src/pack.rs:81-102`), replace this block:

```rust
    // Re-derive the ordering invariant locally; never trust input Vec order.
    let mut order: Vec<(&SymbolNode, (f64, f64))> =
        node.children.iter().map(|c| (c, size(c, cfg, rel))).collect();
    if order.first().map(|(c, _)| &c.id.kind) == Some(&SymbolKind::Chunk) {
        // Chunk children pack in source order, ignoring their heading labels.
        order.sort_by(|(a, _), (b, _)| {
            let ka = a.byte_range.as_ref().map(|r| r.start).unwrap_or(0);
            let kb = b.byte_range.as_ref().map(|r| r.start).unwrap_or(0);
            ka.cmp(&kb).then(a.id.ordinal.cmp(&b.id.ordinal))
        });
    } else {
        // Kind groups first (types → fns → classes → modules), tallest
        // first within a group so greedy column fill becomes FFD; name
        // then ordinal keep equal-height runs alphabetical/deterministic.
        order.sort_by(|(a, sa), (b, sb)| {
            kind_rank(&a.id.kind)
                .cmp(&kind_rank(&b.id.kind))
                .then(sb.1.total_cmp(&sa.1))
                .then(a.name.as_bytes().cmp(b.name.as_bytes()))
                .then(a.id.ordinal.cmp(&b.id.ordinal))
        });
    }
```

with:

```rust
    // Re-derive the ordering invariant locally; never trust input Vec order.
    let mut order: Vec<(&SymbolNode, (f64, f64), u8)> = node
        .children
        .iter()
        .map(|c| (c, size(c, cfg, rel), doc_rank(c)))
        .collect();
    // Chunk children, and all descendants of source-ordered files (prose,
    // declare-before-use C/C++), pack in source order: reorganizing them
    // would break top-to-bottom reading.
    let source_ordered = order.first().map(|(c, ..)| &c.id.kind) == Some(&SymbolKind::Chunk)
        || (!matches!(node.id.kind, SymbolKind::Folder)
            && file_ext(&node.id.qualified_path).is_some_and(is_source_ordered_ext));
    if source_ordered {
        order.sort_by(|(a, ..), (b, ..)| {
            let ka = a.byte_range.as_ref().map(|r| r.start).unwrap_or(0);
            let kb = b.byte_range.as_ref().map(|r| r.start).unwrap_or(0);
            ka.cmp(&kb).then(a.id.ordinal.cmp(&b.id.ordinal))
        });
    } else {
        // Docs sink last; then kind groups (types → fns → classes →
        // modules), tallest first within a group so greedy column fill
        // becomes FFD; name then ordinal keep equal-height runs
        // alphabetical/deterministic. Doc ranks were precomputed above —
        // no tree walks inside the comparator.
        order.sort_by(|(a, sa, da), (b, sb, db)| {
            da.cmp(db)
                .then(kind_rank(&a.id.kind).cmp(&kind_rank(&b.id.kind)))
                .then(sb.1.total_cmp(&sa.1))
                .then(a.name.as_bytes().cmp(b.name.as_bytes()))
                .then(a.id.ordinal.cmp(&b.id.ordinal))
        });
    }
```

Then update the three consumers of `order` below (tuple gains a third element). Replace:

```rust
    let area: f64 = order.iter().map(|(_, (w, h))| w * h).sum();
    let tallest = order.iter().map(|&(_, (_, h))| h).fold(0.0, f64::max);
```

with:

```rust
    let area: f64 = order.iter().map(|(_, (w, h), _)| w * h).sum();
    let tallest = order.iter().map(|&(_, (_, h), _)| h).fold(0.0, f64::max);
```

and the placement loop header:

```rust
    for &(child, (w, h)) in &order {
```

with:

```rust
    for &(child, (w, h), _) in &order {
```

(the loop body is unchanged).

Note this deliberately merges the Chunk branch and the new source-ordered condition into one comparator: the Chunk condition still takes precedence (it is the first alternative of the `||`) and its comparator semantics are byte-identical to before.

- [ ] **Step 4: Run the full workspace suite**

Run: `cargo test --workspace`
Expected: ALL PASS. Existing pack tests keep passing unchanged: every existing fixture uses `.rs` files (not source-ordered) whose children all have `doc_rank` 0, and the chunk test still hits the source-order comparator. If any existing test fails, the sort implementation is wrong — do not re-derive their expected rects.

- [ ] **Step 5: Amend invariant #3**

In `docs/code-comprehension-viewer-design.md`, line 396 currently ends with:

```
…sibling subtrees are still unaffected by each other's internal edits.
```

Append to the end of that same list item (one sentence group, same line or wrapped):

```
Amended 2026-07-11 (2): children of source-ordered files (markdown/text, C/C++) keep byte order at every nesting level instead of kind/size order, and within a folder doc children (doc files; folders whose files are >70% doc) pack after source children — so folder layout now also depends on descendant file *extensions* (a doc-only folder gaining a source file can float above doc siblings). Editing file *contents* still never changes classification.
```

- [ ] **Step 6: Run clippy and the suite once more**

Run: `cargo clippy --workspace --all-targets` and `cargo test --workspace`
Expected: clippy clean (Task 1's dead-code warnings are gone — all helpers are now live), all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/outrider-layout/src/pack.rs docs/code-comprehension-viewer-design.md
git commit -m "feat: source-ordered file packing and doc-rank folder sink"
```
