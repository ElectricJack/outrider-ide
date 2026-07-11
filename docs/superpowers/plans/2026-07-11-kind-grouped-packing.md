# Kind-Grouped, Size-Aware Packing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pack same-level treemap children grouped by kind (types → loose fns → classes/impls → modules) and tallest-first within groups, so columns fill evenly instead of leaving large empty strips.

**Architecture:** All changes live in `crates/outrider-layout/src/pack.rs`. A `kind_rank` helper maps `SymbolKind` to a group rank; `size()` computes child sizes *before* sorting, then sorts by `(rank, height desc, name, ordinal)`. The greedy column-fill placement loop is unchanged — tallest-first input turns it into first-fit-decreasing. Chunk children keep their existing source-order branch. Spec: `docs/superpowers/specs/2026-07-11-packing-order-design.md`.

**Tech Stack:** Rust, cargo test. No new dependencies.

## Global Constraints

- Determinism: identical input trees must produce identical layouts (existing `deterministic` test must keep passing).
- Hierarchical stability: a container's internal layout depends only on its own children's kinds, names, and sizes.
- `Chunk` children keep source order (`byte_range.start`, then ordinal) — untouched.
- No renderer changes; `treemap.rs` is order-agnostic.

---

### Task 1: `kind_rank` helper

**Files:**
- Modify: `crates/outrider-layout/src/pack.rs` (helper after `pack()`, ~line 51; test in existing `mod tests`)

**Interfaces:**
- Consumes: `outrider_index::SymbolKind` (already imported in pack.rs).
- Produces: `fn kind_rank(kind: &SymbolKind) -> u8` — private, used by Task 2's sort.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/outrider-layout/src/pack.rs`:

```rust
#[test]
fn kind_rank_groups_types_fns_classes_modules() {
    let item = |l: &str| SymbolKind::Item { label: l.into() };
    for l in ["struct", "enum", "trait", "interface", "type"] {
        assert_eq!(kind_rank(&item(l)), 0, "{l} is a type");
    }
    assert_eq!(kind_rank(&item("fn")), 1);
    assert_eq!(kind_rank(&item("class")), 2);
    assert_eq!(kind_rank(&item("impl")), 2);
    assert_eq!(kind_rank(&item("module")), 3);
    assert_eq!(kind_rank(&item("namespace")), 3);
    // unknown labels pack last
    assert_eq!(kind_rank(&item("macro")), 4);
    // files/folders have no kind grouping: all rank 0
    assert_eq!(kind_rank(&SymbolKind::File), 0);
    assert_eq!(kind_rank(&SymbolKind::Folder), 0);
}
```

`kind_rank` is in scope via the existing `use super::*;`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p outrider-layout kind_rank_groups -- --nocapture`
Expected: COMPILE ERROR — `cannot find function kind_rank`.

- [ ] **Step 3: Write minimal implementation**

Add after `pack()` (before `fn size`) in `crates/outrider-layout/src/pack.rs`:

```rust
/// Packing group for a sibling (spec: types → loose fns → classes/impls
/// → modules → unknown). Files/folders don't group — all rank 0.
fn kind_rank(kind: &SymbolKind) -> u8 {
    match kind {
        SymbolKind::Item { label } => match label.as_str() {
            "struct" | "enum" | "trait" | "interface" | "type" => 0,
            "fn" => 1,
            "class" | "impl" => 2,
            "module" | "namespace" => 3,
            _ => 4,
        },
        _ => 0,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p outrider-layout kind_rank_groups`
Expected: PASS (1 passed). A dead-code warning for `kind_rank` is acceptable until Task 2 wires it in (test usage should suppress it anyway).

- [ ] **Step 5: Commit**

```bash
git add crates/outrider-layout/src/pack.rs
git commit -m "feat: kind_rank packing groups for treemap siblings"
```

---

### Task 2: Size-aware sort in `size()`

**Files:**
- Modify: `crates/outrider-layout/src/pack.rs:56-102` (`fn size`), plus tests in the same file.

**Interfaces:**
- Consumes: `kind_rank(&SymbolKind) -> u8` from Task 1.
- Produces: no signature changes — `size()` keeps `(node, cfg, rel) -> (f64, f64)`; only child ordering changes.

- [ ] **Step 1: Write the failing tests**

In `crates/outrider-layout/src/pack.rs` tests, REPLACE the entire test `children_placed_by_name_then_ordinal_never_size` (the old contract is deliberately reversed) with:

```rust
#[test]
fn children_placed_tallest_first_names_break_ties() {
    // "zeta" is huge, "alpha" tiny — zeta packs first now (size-aware).
    let tree = SymbolTree {
        root: n(
            SymbolKind::Folder,
            "",
            "",
            0,
            vec![
                n(SymbolKind::File, "zeta.rs", "zeta.rs", 5000, vec![]),
                n(SymbolKind::File, "alpha.rs", "alpha.rs", 1, vec![]),
            ],
        ),
        repo_root: "/x".into(),
    };
    let p = pack(&tree, &cfg());
    let (a, z) = (rect(&p, "alpha.rs"), rect(&p, "zeta.rs"));
    // zeta is placed first: top-left of the content area
    close(z.x, 8.0);
    close(z.y, 60.0);
    // alpha wraps to the second column (zeta alone fills target_h)
    close(a.x, 496.0);
    close(a.y, 60.0);
}

#[test]
fn kind_groups_beat_size_types_first_modules_last() {
    // Scrambled input: huge loose fn, small module, tiny struct, small
    // class. Group rank wins over height: the tiny struct still packs
    // first; the module packs last despite the fn being far taller.
    let item = |label: &str, qp: &str, name: &str, measure: u64| {
        n(SymbolKind::Item { label: label.into() }, qp, name, measure, vec![])
    };
    let file = n(
        SymbolKind::File,
        "m.rs",
        "m.rs",
        0,
        vec![
            item("fn", "m.rs::big", "big", 200),
            item("module", "m.rs::sub", "sub", 3),
            item("struct", "m.rs::S", "S", 2),
            item("class", "m.rs::C", "C", 3),
        ],
    );
    let tree = SymbolTree {
        root: n(SymbolKind::Folder, "", "", 0, vec![file]),
        repo_root: "/x".into(),
    };
    let p = pack(&tree, &cfg());
    let (s, big, c, sub) = (
        rect(&p, "m.rs::S"),
        rect(&p, "m.rs::big"),
        rect(&p, "m.rs::C"),
        rect(&p, "m.rs::sub"),
    );
    // struct is first: top-left of m.rs's content area
    assert!(s.x < big.x && s.y < big.y + big.h, "struct before fn");
    // big fn wraps to its own column right of the struct
    assert!(big.x > s.x, "fn in a later column than the struct");
    // class after fn, module after class (later column or lower in same)
    assert!(c.x > big.x || (c.x == big.x && c.y > big.y), "class after fn");
    assert!(sub.x > c.x || (sub.x == c.x && sub.y > c.y), "module after class");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider-layout -- children_placed_tallest_first kind_groups_beat_size`
Expected: FAIL — `children_placed_tallest_first_names_break_ties` asserts `z.x == 8.0` but alphabetical order puts alpha there; `kind_groups_beat_size_types_first_modules_last` fails on "struct before fn" (alphabetical puts "C" and "big" before "S").

- [ ] **Step 3: Implement the sort change**

In `fn size` (`crates/outrider-layout/src/pack.rs:66-80`), replace this block:

```rust
    // Re-derive the ordering invariant locally; never trust input Vec order.
    let mut order: Vec<&SymbolNode> = node.children.iter().collect();
    if order.first().map(|c| &c.id.kind) == Some(&SymbolKind::Chunk) {
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
```

with (sizes are computed BEFORE sorting so height can be a sort key):

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

Then update the two loops below that consumed the old `order`/`sizes` pair. Replace:

```rust
    let area: f64 = sizes.iter().map(|(w, h)| w * h).sum();
    let tallest = sizes.iter().map(|&(_, h)| h).fold(0.0, f64::max);
```

with:

```rust
    let area: f64 = order.iter().map(|(_, (w, h))| w * h).sum();
    let tallest = order.iter().map(|&(_, (_, h))| h).fold(0.0, f64::max);
```

and replace the placement loop header:

```rust
    for (child, &(w, h)) in order.iter().zip(&sizes) {
```

with:

```rust
    for &(child, (w, h)) in &order {
```

(the loop body is unchanged).

- [ ] **Step 4: Run the full workspace suite**

Run: `cargo test --workspace`
Expected: ALL PASS. Existing exact-rect tests (`worked_example_exact_rects`, `sibling_subtree_stable_under_edit`, `columns_fill_down_then_wrap_right`, `chunk_children_pack_in_source_order_not_label_order`) keep passing unchanged: in each, the taller sibling already came first alphabetically, equal heights fall back to name order, and chunks are untouched. If any of these fail, the sort implementation is wrong — do not re-derive their expected rects.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider-layout/src/pack.rs
git commit -m "feat: kind-grouped, tallest-first sibling packing"
```
