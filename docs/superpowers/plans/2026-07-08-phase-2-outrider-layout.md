# Phase 2 — `outrider-layout` (SymbolTree → WorldLayout) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A pure, deterministic layout crate mapping any `SymbolTree` to a `WorldLayout` (icicle-on-a-grid, ratio 8), with all five spec §8.1 property tests passing headless — spec milestone 2.

**Architecture:** `outrider-layout` consumes `outrider-index` types only. Two passes: post-order **measure** (cell lengths, integer math, round-up-only) and pre-order **arrange** (name-order placement, per-child gaps, parent-relative offsets). One entry point: `layout(&SymbolTree) -> WorldLayout`.

**Tech Stack:** Rust 2021, `outrider-index` (path dep), `proptest` (dev-dep). No other dependencies. No GPUI, no serde, no I/O.

**Source spec:** `docs/superpowers/specs/2026-07-05-outrider-walking-skeleton-design.md` §4.1, §6, §8.1, §9 (milestone 2).
**Roadmap:** `docs/superpowers/plans/2026-07-08-walking-skeleton-roadmap.md` (Phase 2).

## Global Constraints

- No GPUI types anywhere in `outrider-layout` (spec §4).
- `layout()` is a **pure function**: no I/O, no clocks, no randomness, no env reads (spec §4).
- `BTreeMap` / sorted `Vec` only; **no `HashMap` iteration anywhere in this crate** (spec §6.4).
- **Integer-only cell math**; no floating point anywhere in this crate (spec §6.4 — floats appear only at render time, which is Phase 3).
- Subdivision ratio `r = 8` (spec §6.1). A parent occupying *n* level-*d* cells subdivides into `n·r` level-*(d+1)* cells.
- `lines_per_cell` initial values: **32 at file level, 4 at method level** (spec §6.2; tuning deferred to milestone 3).
- Leaf cells: `max(1, ceil(measure / lines_per_cell))` (spec §6.2).
- Rounding only ever rounds **up**; one post-order sweep, no iteration to convergence (spec §6.2).
- Children placed in **(name byte-wise, ordinal) order — never by size** (spec §6.3, parent invariant 3).
- `CellRange.start` is **relative to the parent's range** (hierarchical address / floating origin, spec §6.3).
- Slack distributed as gaps: one gap after each child, remainder appended at the end (spec §6.3).
- Exit gate: **all five §8.1 property tests pass.** Per spec §10: if a continuity bound proves wrong, tighten the algorithm, not the test.

## Interpretation Decisions

Spec ambiguities resolved by this plan (flag deviations to the controller if they prove wrong in practice):

1. **Per-child slack, not pooled slack.** Spec §6.2 writes `slack = ceil(0.15 × Σ child cells)` pooled per parent. Under pure recomputation, any pooled quantity makes *every* child's position depend on *every* sibling's size — growing one leaf would move its **predecessors**, violating §8.1 property 2 ("only that leaf and at most its within-parent successors change"). We therefore compute slack per child: `gap(len) = ceil(0.15 × len)`, parent total `Σ (child_len + gap(child_len))`. A child's start then depends only on the children before it. This is spec §10's "tighten the algorithm, not the test" applied at plan time. The §6.3 shape is preserved: one gap after each child, remainder (from the parent's round-up) at the end.
2. **`lines_per_cell` is keyed by `SymbolKind`, not depth**: `Folder`/`File` → 32, all item kinds → 4. Files appear at varying tree depths, so "file level" cannot be a fixed depth-d constant.
3. **Container cells derive only from children** (spec §6.2's parent formula, applied literally). A file's own line count influences its cells only when it has no item children (then it is a leaf).
4. **"Serialized" = `Debug` formatting.** Determinism compares `format!("{:?}", world)` bytes. `BTreeMap<SymbolId, _>` has struct keys, which JSON cannot represent; `Debug` over `BTreeMap` is deterministic and needs no new dependency.
5. **Arrange re-derives child order** by `(name, ordinal)` itself rather than trusting input `Vec` order — the ordering invariant is enforced where it is consumed.
6. **Tree-wide `SymbolId` uniqueness is a precondition** — layout keys demand it (spec §4.1: "the stable identity used by layout keys"). Phase 1 has a latent collision: `index.rs` builds child paths as `{parent_qual}::{name}`, and ordinals are assigned per sibling group, so same-named children of same-named containers (e.g. two cfg-gated `mod imp` blocks each containing `fn connect`) collide exactly. Task 1 fixes this in `outrider-index` with a deterministic post-pass; `layout()` guards it with a `debug_assert`.

## File Structure

- Modify: `crates/outrider-index/src/types.rs` (add `dedupe_ids`), `crates/outrider-index/src/index.rs` (call it), `crates/outrider-index/tests/index_test.rs` (uniqueness test) — Task 1 only.
- Create: `crates/outrider-layout/src/types.rs` — `CellRange`, `NodeLayout`, `WorldLayout`, `RATIO`, `absolute_start`.
- Create: `crates/outrider-layout/src/measure.rs` — `lines_per_cell`, `leaf_cells`, `gap_cells`, `node_cells` (post-order).
- Create: `crates/outrider-layout/src/arrange.rs` — `arrange` (pre-order) + public `layout()` entry.
- Modify: `crates/outrider-layout/src/lib.rs`, `crates/outrider-layout/Cargo.toml`.
- Create: `crates/outrider-layout/tests/common/mod.rs` — proptest `SymbolTree` generator (shared).
- Create: `crates/outrider-layout/tests/props_basic.rs` — properties 1, 4, 5.
- Create: `crates/outrider-layout/tests/props_continuity.rs` — properties 2, 3.
- Create: `crates/outrider-layout/tests/cross_process.rs` — cross-process determinism.

## Worked Example (used by unit tests)

Tree: root folder `""` containing file `a.rs` (100 lines, no items) and file `b.rs` (40 lines) containing `fn f` (10 lines) and `fn g` (1 line).

Measure (post-order): `f` = ceil(10/4) = **3**; `g` = max(1, ceil(1/4)) = **1**; `b.rs` = ceil(((3+gap 1)+(1+gap 1))/8) = ceil(6/8) = **1**; `a.rs` = ceil(100/32) = **4**; root = ceil(((4+gap 1)+(1+gap 1))/8) = ceil(7/8) = **1**. (gap(len) = ceil(0.15·len): gap(1)=1, gap(3)=1, gap(4)=1.)

Arrange (pre-order, parent-relative): root `{level 0, start 0, len 1}`; root's 8 subcells hold `a.rs` `{1, 0, 4}` then `b.rs` `{1, 5, 1}` (cursor 0 → 4+1=5 → 5+1+1=7 ≤ 8). `b.rs`'s 8 subcells: `f` `{2, 0, 3}`, `g` `{2, 4, 1}` (cursor 3+1=4 → 4+1+1=6 ≤ 8).

Absolute starts (abs(child) = abs(parent)·8 + start): `b.rs` = 5, `f` = 5·8+0 = **40**, `g` = **44**.

---

### Task 1: Tree-wide `SymbolId` uniqueness in `outrider-index`

**Files:**
- Modify: `crates/outrider-index/src/types.rs` (add `dedupe_ids` + unit test)
- Modify: `crates/outrider-index/src/index.rs` (call `dedupe_ids` after assembly)
- Modify: `crates/outrider-index/src/lib.rs` (re-export `dedupe_ids`)
- Modify: `crates/outrider-index/tests/index_test.rs` (tree-wide uniqueness integration test)

**Interfaces:**
- Consumes: existing `SymbolNode`, `finalize_children` (types.rs).
- Produces: `pub fn dedupe_ids(root: &mut SymbolNode)` — after it runs, every `SymbolId` in the tree is unique. `index_repo` output now carries this guarantee; Tasks 2–7 rely on it.

**Why:** two same-named containers (legal via `#[cfg]`, e.g. `mod imp` twice) produce children with identical `(kind, qualified_path, ordinal)` — `qualified_path` is `{parent_qual}::{name}` with no parent-ordinal component, and ordinals are per-sibling-group. Layout's `BTreeMap<SymbolId, NodeLayout>` would silently drop one node.

- [ ] **Step 1: Write the failing unit test** — append to the `tests` module in `crates/outrider-index/src/types.rs`:

```rust
    #[test]
    fn dedupe_ids_disambiguates_cross_scope_duplicates() {
        // Simulates two cfg-gated `mod imp` blocks, each containing `fn connect`.
        let mk_mod = || {
            SymbolNode {
                id: SymbolId {
                    kind: SymbolKind::Module,
                    qualified_path: "net.rs::imp".into(),
                    ordinal: 0,
                },
                name: "imp".into(),
                byte_range: None,
                measure: 2,
                churn: 0.0,
                churn_count: 0,
                children: vec![SymbolNode {
                    id: SymbolId {
                        kind: SymbolKind::Fn,
                        qualified_path: "net.rs::imp::connect".into(),
                        ordinal: 0,
                    },
                    name: "connect".into(),
                    byte_range: None,
                    measure: 1,
                    churn: 0.0,
                    churn_count: 0,
                    children: vec![],
                }],
            }
        };
        let mut file = mk("net.rs");
        file.children = vec![mk_mod(), mk_mod()];
        finalize_children(&mut file.children);
        // finalize gives the mods ordinals 0,1 — but both `connect` fns still collide
        assert_eq!(
            file.children[0].children[0].id,
            file.children[1].children[0].id
        );

        dedupe_ids(&mut file);
        let a = &file.children[0].children[0].id;
        let b = &file.children[1].children[0].id;
        assert_ne!(a, b);
        assert_eq!((a.ordinal, b.ordinal), (0, 1));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p outrider-index dedupe_ids -- --nocapture`
Expected: FAIL to compile with "cannot find function `dedupe_ids`".

- [ ] **Step 3: Implement `dedupe_ids`** — add to `crates/outrider-index/src/types.rs` (below `finalize_children`), plus the import `use std::collections::BTreeMap;` at the top:

```rust
/// Enforce tree-wide `SymbolId` uniqueness (spec §4.1: the ID is the stable
/// identity used by layout keys). `finalize_children` disambiguates only
/// within one sibling group; same-named children of same-named containers
/// (e.g. cfg-gated duplicate `mod` blocks) still collide across scopes.
/// Deterministic pre-order walk: on a repeated `(kind, qualified_path)`,
/// bump the ordinal to the next unseen value. Within-scope relative order
/// is preserved because visit order is deterministic and bumps are monotonic.
pub fn dedupe_ids(root: &mut SymbolNode) {
    fn walk(node: &mut SymbolNode, seen: &mut BTreeMap<(SymbolKind, String), u16>) {
        let next = seen
            .entry((node.id.kind, node.id.qualified_path.clone()))
            .or_insert(0);
        if node.id.ordinal < *next {
            node.id.ordinal = *next;
        }
        *next = node.id.ordinal + 1;
        for child in &mut node.children {
            walk(child, seen);
        }
    }
    walk(root, &mut BTreeMap::new());
}
```

- [ ] **Step 4: Run the unit test to verify it passes**

Run: `cargo test -p outrider-index dedupe_ids`
Expected: PASS.

- [ ] **Step 5: Wire into `index_repo` and re-export.** In `crates/outrider-index/src/index.rs`, inside `index_repo`, immediately after the tree assembly and **before** churn annotation, add (adapting to the local variable name for the root node — read the function first; as of Phase 1 it builds a tree via `build_tree` then annotates churn):

```rust
    crate::types::dedupe_ids(&mut tree.root);
```

In `crates/outrider-index/src/lib.rs`, extend the types re-export:

```rust
pub use types::{dedupe_ids, finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
```

- [ ] **Step 6: Add the integration regression test** — append to `crates/outrider-index/tests/index_test.rs`:

```rust
#[test]
fn symbol_ids_are_unique_tree_wide() {
    let dir = common::copy_fixture("mini_repo");
    let tree = outrider_index::index_repo(dir.path()).unwrap();
    fn walk(n: &outrider_index::SymbolNode, out: &mut Vec<outrider_index::SymbolId>) {
        out.push(n.id.clone());
        for c in &n.children {
            walk(c, out);
        }
    }
    let mut ids = Vec::new();
    walk(&tree.root, &mut ids);
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), total, "duplicate SymbolIds in indexed tree");
}
```

(`common::copy_fixture("mini_repo")` is the same helper the existing tests in that file use.)

- [ ] **Step 7: Run the full index test suite**

Run: `cargo test -p outrider-index`
Expected: all tests pass (previous 12 + 2 new).

- [ ] **Step 8: Commit**

```bash
git add crates/outrider-index/src/types.rs crates/outrider-index/src/index.rs crates/outrider-index/src/lib.rs crates/outrider-index/tests/index_test.rs
git commit -m "fix: enforce tree-wide SymbolId uniqueness (cross-scope duplicate containers)"
```

---

### Task 2: Layout crate types

**Files:**
- Modify: `crates/outrider-layout/Cargo.toml`
- Create: `crates/outrider-layout/src/types.rs`
- Modify: `crates/outrider-layout/src/lib.rs` (currently empty)

**Interfaces:**
- Consumes: `outrider_index::SymbolId`.
- Produces (exact, spec §4.1 — later tasks and Phase 3 rely on these): `pub const RATIO: u32 = 8;`, `pub struct CellRange { pub level: u8, pub start: u64, pub len: u64 }`, `pub struct NodeLayout { pub id: SymbolId, pub parent: Option<SymbolId>, pub cells: CellRange }`, `pub struct WorldLayout { pub nodes: BTreeMap<SymbolId, NodeLayout>, pub ratio: u32 }`, and `WorldLayout::absolute_start(&self, id: &SymbolId) -> Option<u64>`.

- [ ] **Step 1: Add the dependency.** Replace the `[dependencies]` section of `crates/outrider-layout/Cargo.toml` with:

```toml
[dependencies]
outrider-index = { path = "../outrider-index" }

[dev-dependencies]
proptest = "1"
```

- [ ] **Step 2: Write the failing test.** Create `crates/outrider-layout/src/types.rs`:

```rust
use std::collections::BTreeMap;

use outrider_index::SymbolId;

/// Subdivision ratio: a parent's n level-d cells subdivide into n·r
/// level-(d+1) cells (spec §6.1).
pub const RATIO: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellRange {
    /// Structural depth: root = 0.
    pub level: u8,
    /// Offset in level-`level` cells, relative to the parent's range
    /// (hierarchical address, spec §6.3).
    pub start: u64,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLayout {
    pub id: SymbolId,
    pub parent: Option<SymbolId>,
    pub cells: CellRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldLayout {
    pub nodes: BTreeMap<SymbolId, NodeLayout>,
    pub ratio: u32,
}

impl WorldLayout {
    /// Absolute level-d cell index of a node's first cell, composed from the
    /// ancestor chain: abs(child) = abs(parent) · r + child.start.
    /// (Render code will compose only near-camera ancestors instead — this
    /// full composition is for tests and tools.)
    pub fn absolute_start(&self, id: &SymbolId) -> Option<u64> {
        let node = self.nodes.get(id)?;
        match &node.parent {
            None => Some(node.cells.start),
            Some(p) => Some(self.absolute_start(p)? * self.ratio as u64 + node.cells.start),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind};

    fn id(kind: SymbolKind, qp: &str) -> SymbolId {
        SymbolId {
            kind,
            qualified_path: qp.into(),
            ordinal: 0,
        }
    }

    #[test]
    fn absolute_start_composes_ancestor_chain() {
        // Worked example from the plan header: root {0,0,1}, b.rs {1,5,1}, f {2,0,3}
        let root = id(SymbolKind::Folder, "");
        let b = id(SymbolKind::File, "b.rs");
        let f = id(SymbolKind::Fn, "b.rs::f");
        let mut nodes = BTreeMap::new();
        nodes.insert(
            root.clone(),
            NodeLayout {
                id: root.clone(),
                parent: None,
                cells: CellRange { level: 0, start: 0, len: 1 },
            },
        );
        nodes.insert(
            b.clone(),
            NodeLayout {
                id: b.clone(),
                parent: Some(root.clone()),
                cells: CellRange { level: 1, start: 5, len: 1 },
            },
        );
        nodes.insert(
            f.clone(),
            NodeLayout {
                id: f.clone(),
                parent: Some(b.clone()),
                cells: CellRange { level: 2, start: 0, len: 3 },
            },
        );
        let world = WorldLayout { nodes, ratio: RATIO };
        assert_eq!(world.absolute_start(&root), Some(0));
        assert_eq!(world.absolute_start(&b), Some(5));
        assert_eq!(world.absolute_start(&f), Some(40));
        assert_eq!(world.absolute_start(&id(SymbolKind::Fn, "missing")), None);
    }
}
```

Replace `crates/outrider-layout/src/lib.rs` with:

```rust
pub mod types;

pub use types::{CellRange, NodeLayout, WorldLayout, RATIO};
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p outrider-layout`
Expected: PASS (1 test). (The types and test land together; the failing-first step is trivial here and skipped deliberately — the test still asserts real composition arithmetic.)

- [ ] **Step 4: Commit**

```bash
git add crates/outrider-layout/Cargo.toml crates/outrider-layout/src/types.rs crates/outrider-layout/src/lib.rs Cargo.lock
git commit -m "feat: outrider-layout cell types and absolute_start composition"
```

---

### Task 3: Measure pass (post-order)

**Files:**
- Create: `crates/outrider-layout/src/measure.rs`
- Modify: `crates/outrider-layout/src/lib.rs`

**Interfaces:**
- Consumes: `outrider_index::{SymbolId, SymbolKind, SymbolNode}`, `types::RATIO`.
- Produces: `pub fn lines_per_cell(kind: SymbolKind) -> u64`; `pub(crate) fn gap_cells(len: u64) -> u64`; `pub(crate) fn node_cells(node: &SymbolNode, lens: &mut BTreeMap<SymbolId, u64>) -> u64` (fills every node's cell length, returns the node's own). Task 4 consumes `node_cells` + `gap_cells`; the continuity tests consume `lines_per_cell`.

- [ ] **Step 1: Write the failing tests.** Create `crates/outrider-layout/src/measure.rs` with the tests module only (implementation comes next step — leave the non-test part as just the imports so the module compiles standalone will fail; that is expected):

```rust
use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolKind, SymbolNode};

use crate::types::RATIO;

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    pub(crate) fn node(
        kind: SymbolKind,
        qp: &str,
        name: &str,
        measure: u64,
        children: Vec<SymbolNode>,
    ) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind,
                qualified_path: qp.into(),
                ordinal: 0,
            },
            name: name.into(),
            byte_range: None,
            measure,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    #[test]
    fn gap_is_fifteen_percent_rounded_up() {
        assert_eq!(gap_cells(1), 1);
        assert_eq!(gap_cells(4), 1);
        assert_eq!(gap_cells(7), 2);   // ceil(1.05)
        assert_eq!(gap_cells(20), 3);
        assert_eq!(gap_cells(100), 15);
    }

    #[test]
    fn leaf_cells_by_kind_with_floor_of_one() {
        assert_eq!(leaf_cells(100, SymbolKind::File), 4); // ceil(100/32)
        assert_eq!(leaf_cells(1, SymbolKind::File), 1);
        assert_eq!(leaf_cells(10, SymbolKind::Fn), 3); // ceil(10/4)
        assert_eq!(leaf_cells(1, SymbolKind::Fn), 1);
        assert_eq!(leaf_cells(0, SymbolKind::Fn), 1); // floor of one
    }

    #[test]
    fn worked_example_measures() {
        // Plan-header worked example.
        let b = node(
            SymbolKind::File,
            "b.rs",
            "b.rs",
            40,
            vec![
                node(SymbolKind::Fn, "b.rs::f", "f", 10, vec![]),
                node(SymbolKind::Fn, "b.rs::g", "g", 1, vec![]),
            ],
        );
        let root = node(
            SymbolKind::Folder,
            "",
            "",
            140,
            vec![node(SymbolKind::File, "a.rs", "a.rs", 100, vec![]), b],
        );
        let mut lens = BTreeMap::new();
        let root_len = node_cells(&root, &mut lens);
        assert_eq!(root_len, 1);
        let get = |qp: &str| {
            lens.iter()
                .find(|(id, _)| id.qualified_path == qp)
                .map(|(_, l)| *l)
                .unwrap()
        };
        assert_eq!(get("b.rs::f"), 3);
        assert_eq!(get("b.rs::g"), 1);
        assert_eq!(get("b.rs"), 1); // ceil(((3+1)+(1+1))/8)
        assert_eq!(get("a.rs"), 4);
        assert_eq!(get(""), 1); // ceil(((4+1)+(1+1))/8)
        assert_eq!(lens.len(), 5);
    }
}
```

Add `pub mod measure;` to `crates/outrider-layout/src/lib.rs` and `pub use measure::lines_per_cell;`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p outrider-layout measure`
Expected: FAIL to compile — `gap_cells`, `leaf_cells`, `node_cells` not found.

- [ ] **Step 3: Implement** — insert between the imports and the tests module in `measure.rs`:

```rust
/// Initial spec §6.2 constants: 32 lines/cell at file level, 4 at method
/// level; keyed by kind because files occur at varying depths. Tune during
/// milestone 3.
pub fn lines_per_cell(kind: SymbolKind) -> u64 {
    match kind {
        SymbolKind::Folder | SymbolKind::File => 32,
        _ => 4,
    }
}

pub(crate) fn leaf_cells(measure: u64, kind: SymbolKind) -> u64 {
    std::cmp::max(1, measure.div_ceil(lines_per_cell(kind)))
}

/// Per-child slack: ceil(0.15 · len), in integer math. Per-child (not pooled
/// per parent) so a child's position never depends on its successors — see
/// plan "Interpretation Decisions" #1.
pub(crate) fn gap_cells(len: u64) -> u64 {
    (len * 15).div_ceil(100)
}

/// Post-order measure pass (spec §6.2). Fills `lens` for every node in the
/// subtree; returns this node's length in cells at its own level. Round-up
/// only — one sweep, no convergence iteration.
pub(crate) fn node_cells(node: &SymbolNode, lens: &mut BTreeMap<SymbolId, u64>) -> u64 {
    let len = if node.children.is_empty() {
        leaf_cells(node.measure, node.id.kind)
    } else {
        let total: u64 = node
            .children
            .iter()
            .map(|c| {
                let l = node_cells(c, lens);
                l + gap_cells(l)
            })
            .sum();
        total.div_ceil(RATIO as u64)
    };
    lens.insert(node.id.clone(), len);
    len
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider-layout`
Expected: PASS (4 tests total).

- [ ] **Step 5: Commit**

```bash
git add crates/outrider-layout/src/measure.rs crates/outrider-layout/src/lib.rs
git commit -m "feat: post-order measure pass (leaf cells, per-child gaps, round-up parents)"
```

---

### Task 4: Arrange pass and the `layout()` entry point

**Files:**
- Create: `crates/outrider-layout/src/arrange.rs`
- Modify: `crates/outrider-layout/src/lib.rs`

**Interfaces:**
- Consumes: `measure::{node_cells, gap_cells}`, `types::*`, `outrider_index::{SymbolId, SymbolNode, SymbolTree}`.
- Produces: **`pub fn layout(tree: &SymbolTree) -> WorldLayout`** — the crate's single entry point; Phase 3 and all property tests consume exactly this.

- [ ] **Step 1: Write the failing test.** Create `crates/outrider-layout/src/arrange.rs`:

```rust
use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};

use crate::measure::{gap_cells, node_cells};
use crate::types::{CellRange, NodeLayout, WorldLayout, RATIO};

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::SymbolKind;

    fn node(
        kind: SymbolKind,
        qp: &str,
        name: &str,
        measure: u64,
        children: Vec<SymbolNode>,
    ) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind,
                qualified_path: qp.into(),
                ordinal: 0,
            },
            name: name.into(),
            byte_range: None,
            measure,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    #[test]
    fn worked_example_layout_exact() {
        let b = node(
            SymbolKind::File,
            "b.rs",
            "b.rs",
            40,
            vec![
                node(SymbolKind::Fn, "b.rs::f", "f", 10, vec![]),
                node(SymbolKind::Fn, "b.rs::g", "g", 1, vec![]),
            ],
        );
        let root = node(
            SymbolKind::Folder,
            "",
            "",
            140,
            vec![node(SymbolKind::File, "a.rs", "a.rs", 100, vec![]), b],
        );
        let tree = SymbolTree {
            root,
            repo_root: "/ex".into(),
        };
        let w = layout(&tree);
        assert_eq!(w.ratio, RATIO);
        assert_eq!(w.nodes.len(), 5);

        let get = |qp: &str| {
            w.nodes
                .iter()
                .find(|(id, _)| id.qualified_path == qp)
                .map(|(_, n)| n)
                .unwrap()
        };
        let root_l = get("");
        assert_eq!(root_l.parent, None);
        assert_eq!(root_l.cells, CellRange { level: 0, start: 0, len: 1 });

        assert_eq!(get("a.rs").cells, CellRange { level: 1, start: 0, len: 4 });
        assert_eq!(get("b.rs").cells, CellRange { level: 1, start: 5, len: 1 });
        assert_eq!(get("b.rs").parent.as_ref().unwrap().qualified_path, "");

        assert_eq!(get("b.rs::f").cells, CellRange { level: 2, start: 0, len: 3 });
        assert_eq!(get("b.rs::g").cells, CellRange { level: 2, start: 4, len: 1 });

        // absolute composition (worked example)
        let g_id = w.nodes.keys().find(|id| id.qualified_path == "b.rs::g").unwrap().clone();
        assert_eq!(w.absolute_start(&g_id), Some(44));
    }

    #[test]
    fn children_placed_by_name_then_ordinal_never_size() {
        // "zeta" is huge, "alpha" tiny — alpha still comes first.
        let root = node(
            SymbolKind::Folder,
            "",
            "",
            0,
            vec![
                node(SymbolKind::File, "zeta.rs", "zeta.rs", 5000, vec![]),
                node(SymbolKind::File, "alpha.rs", "alpha.rs", 1, vec![]),
            ],
        );
        let tree = SymbolTree {
            root,
            repo_root: "/ex".into(),
        };
        let w = layout(&tree);
        let start = |qp: &str| {
            w.nodes
                .iter()
                .find(|(id, _)| id.qualified_path == qp)
                .map(|(_, n)| n.cells.start)
                .unwrap()
        };
        assert!(start("alpha.rs") < start("zeta.rs"));
        assert_eq!(start("alpha.rs"), 0);
    }
}
```

Update `crates/outrider-layout/src/lib.rs` to its final form:

```rust
pub mod arrange;
pub mod measure;
pub mod types;

pub use arrange::layout;
pub use measure::lines_per_cell;
pub use types::{CellRange, NodeLayout, WorldLayout, RATIO};
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p outrider-layout arrange`
Expected: FAIL to compile — `layout` not found.

- [ ] **Step 3: Implement** — insert between the imports and tests module in `arrange.rs`:

```rust
/// Map a `SymbolTree` to a `WorldLayout` (spec §6). Pure function:
/// deterministic, no I/O, integer-only cell math.
pub fn layout(tree: &SymbolTree) -> WorldLayout {
    let mut lens = BTreeMap::new();
    node_cells(&tree.root, &mut lens);
    let mut nodes = BTreeMap::new();
    arrange(&tree.root, None, 0, 0, &lens, &mut nodes);
    debug_assert_eq!(
        nodes.len(),
        count(&tree.root),
        "duplicate SymbolId in input tree (index must run dedupe_ids)"
    );
    WorldLayout { nodes, ratio: RATIO }
}

/// Pre-order arrange pass (spec §6.3): children in (name, ordinal) order,
/// each followed by its own gap; round-up remainder accumulates at the end.
/// `start` is relative to the parent's range.
fn arrange(
    node: &SymbolNode,
    parent: Option<&SymbolId>,
    level: u8,
    start: u64,
    lens: &BTreeMap<SymbolId, u64>,
    out: &mut BTreeMap<SymbolId, NodeLayout>,
) {
    let len = lens[&node.id];
    out.insert(
        node.id.clone(),
        NodeLayout {
            id: node.id.clone(),
            parent: parent.cloned(),
            cells: CellRange { level, start, len },
        },
    );
    // Re-derive the ordering invariant locally; never trust input Vec order
    // for placement (plan decision #5).
    let mut order: Vec<&SymbolNode> = node.children.iter().collect();
    order.sort_by(|a, b| {
        a.name
            .as_bytes()
            .cmp(b.name.as_bytes())
            .then(a.id.ordinal.cmp(&b.id.ordinal))
    });
    let mut cursor = 0u64;
    for child in order {
        let child_len = lens[&child.id];
        arrange(child, Some(&node.id), level + 1, cursor, lens, out);
        cursor += child_len + gap_cells(child_len);
    }
    debug_assert!(
        cursor <= len * RATIO as u64,
        "children overflow parent allocation"
    );
}

fn count(node: &SymbolNode) -> usize {
    1 + node.children.iter().map(count).sum::<usize>()
}
```

- [ ] **Step 4: Run all layout tests**

Run: `cargo test -p outrider-layout`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/outrider-layout/src/arrange.rs crates/outrider-layout/src/lib.rs
git commit -m "feat: pre-order arrange pass and layout() entry point"
```

---

### Task 5: Proptest generator + properties 1, 4, 5 (determinism, ordering, containment)

**Files:**
- Create: `crates/outrider-layout/tests/common/mod.rs`
- Create: `crates/outrider-layout/tests/props_basic.rs`

**Interfaces:**
- Consumes: `outrider_layout::layout`, `outrider_index` types, `finalize_children`.
- Produces: `common::{g_folder, to_tree, NAMES, FRESH_NAMES}` — the generator every property test (including Task 6) uses. `to_tree` returns trees satisfying the index invariants: children sorted with ordinals assigned (via `finalize_children`) and tree-wide-unique `SymbolId`s.

**Generator design notes (for the implementer):**
- `qualified_path` is a **unique counter string** (`n0`, `n1`, …), *not* the real path scheme. Layout never inspects the path; uniqueness is the only requirement (the index guarantees it via `dedupe_ids`). Names come from a small pool (`NAMES`) so same-name collisions — and therefore ordinal disambiguation — are actually exercised.
- Kinds: item leaves are `Fn`, item containers `Impl` (an empty `impl` block is legal Rust, so `Impl` with zero children is a fine leaf too); files are `File`; folders are `Folder` and always have ≥1 child.

- [ ] **Step 1: Write the shared generator.** Create `crates/outrider-layout/tests/common/mod.rs`:

```rust
// Shared across multiple test binaries; not every binary uses every item.
#![allow(dead_code)]

use outrider_index::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
use proptest::prelude::*;

/// Small pool → frequent same-name siblings → ordinals exercised.
pub const NAMES: &[&str] = &["a", "aa", "b", "c", "x"];
/// Disjoint from NAMES: inserting one of these never re-shuffles existing
/// ordinals (used by the continuity-insert property).
pub const FRESH_NAMES: &[&str] = &["ab", "bz", "zz"];

#[derive(Debug, Clone)]
pub struct GNode {
    pub kind: SymbolKind,
    pub name_idx: usize,
    pub lines: u64,
    pub children: Vec<GNode>,
}

fn g_item() -> impl Strategy<Value = GNode> {
    let leaf = (0..NAMES.len(), 1u64..300).prop_map(|(name_idx, lines)| GNode {
        kind: SymbolKind::Fn,
        name_idx,
        lines,
        children: vec![],
    });
    leaf.prop_recursive(3, 20, 4, |inner| {
        (0..NAMES.len(), 1u64..300, prop::collection::vec(inner, 0..4)).prop_map(
            |(name_idx, lines, children)| GNode {
                kind: SymbolKind::Impl,
                name_idx,
                lines,
                children,
            },
        )
    })
}

fn g_file() -> impl Strategy<Value = GNode> {
    (0..NAMES.len(), 1u64..3000, prop::collection::vec(g_item(), 0..4)).prop_map(
        |(name_idx, lines, children)| GNode {
            kind: SymbolKind::File,
            name_idx,
            lines,
            children,
        },
    )
}

pub fn g_folder() -> impl Strategy<Value = GNode> {
    let base = prop::collection::vec(g_file(), 1..4).prop_map(|children| GNode {
        kind: SymbolKind::Folder,
        name_idx: 0,
        lines: 0,
        children,
    });
    base.prop_recursive(3, 40, 3, |inner| {
        (
            0..NAMES.len(),
            prop::collection::vec(inner, 1..3),
            prop::collection::vec(g_file(), 0..3),
        )
            .prop_map(|(name_idx, subs, files)| {
                let mut children = subs;
                children.extend(files);
                GNode {
                    kind: SymbolKind::Folder,
                    name_idx,
                    lines: 0,
                    children,
                }
            })
    })
}

pub fn to_tree(g: &GNode) -> SymbolTree {
    let mut counter = 0u64;
    let mut root = convert(g, &mut counter);
    root.name = String::new(); // root folder is named "" (spec §4.1)
    SymbolTree {
        root,
        repo_root: "/generated".into(),
    }
}

fn convert(g: &GNode, counter: &mut u64) -> SymbolNode {
    let qp = format!("n{}", *counter);
    *counter += 1;
    let mut children: Vec<SymbolNode> = g.children.iter().map(|c| convert(c, counter)).collect();
    finalize_children(&mut children);
    SymbolNode {
        id: SymbolId {
            kind: g.kind,
            qualified_path: qp,
            ordinal: 0,
        },
        name: NAMES[g.name_idx].to_string(),
        byte_range: None,
        measure: g.lines,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}
```

- [ ] **Step 2: Write the three properties.** Create `crates/outrider-layout/tests/props_basic.rs`:

```rust
mod common;

use common::{g_folder, to_tree};
use outrider_index::SymbolNode;
use outrider_layout::{layout, WorldLayout};
use proptest::prelude::*;

/// Property 4 helper (spec §8.1 #4, strengthened): in every layout, sibling
/// start-order equals (name, ordinal) order — order is a function of names
/// alone, so *no* size permutation can ever change it.
fn assert_sibling_order(node: &SymbolNode, w: &WorldLayout) {
    let mut by_name: Vec<&SymbolNode> = node.children.iter().collect();
    by_name.sort_by(|a, b| {
        a.name
            .as_bytes()
            .cmp(b.name.as_bytes())
            .then(a.id.ordinal.cmp(&b.id.ordinal))
    });
    let starts: Vec<u64> = by_name.iter().map(|c| w.nodes[&c.id].cells.start).collect();
    for pair in starts.windows(2) {
        assert!(pair[0] < pair[1], "sibling starts out of (name, ordinal) order");
    }
    for c in &node.children {
        assert_sibling_order(c, w);
    }
}

/// Property 5 helper (spec §8.1 #5): every child's absolute range lies
/// within its parent's; siblings never overlap.
fn assert_containment(node: &SymbolNode, w: &WorldLayout) {
    let p = &w.nodes[&node.id];
    let p_abs = w.absolute_start(&node.id).unwrap();
    let sub_lo = p_abs * w.ratio as u64;
    let sub_hi = (p_abs + p.cells.len) * w.ratio as u64;

    let mut ranges: Vec<(u64, u64)> = node
        .children
        .iter()
        .map(|c| {
            let abs = w.absolute_start(&c.id).unwrap();
            let len = w.nodes[&c.id].cells.len;
            assert!(abs >= sub_lo && abs + len <= sub_hi, "child escapes parent");
            assert!(len >= 1, "zero-cell node");
            (abs, abs + len)
        })
        .collect();
    ranges.sort();
    for pair in ranges.windows(2) {
        assert!(pair[0].1 <= pair[1].0, "sibling overlap");
    }
    for c in &node.children {
        assert_containment(c, w);
    }
}

proptest! {
    /// Spec §8.1 property 1 (in-process half; cross-process is Task 7).
    #[test]
    fn determinism_byte_identical(g in g_folder()) {
        let t = to_tree(&g);
        let a = layout(&t);
        let b = layout(&t);
        prop_assert_eq!(format!("{:?}", a), format!("{:?}", b));
    }

    /// Spec §8.1 property 4.
    #[test]
    fn stable_ordering_never_by_size(g in g_folder()) {
        let t = to_tree(&g);
        let w = layout(&t);
        assert_sibling_order(&t.root, &w);
    }

    /// Spec §8.1 property 5.
    #[test]
    fn containment_no_overlap(g in g_folder()) {
        let t = to_tree(&g);
        let w = layout(&t);
        assert_containment(&t.root, &w);
    }
}
```

- [ ] **Step 3: Run the properties**

Run: `cargo test -p outrider-layout --test props_basic`
Expected: PASS (3 property tests, 256 cases each by default). If a property fails, proptest prints a minimized counterexample — that is a real algorithm bug (or generator bug); fix it, do not weaken the assertion (spec §10).

- [ ] **Step 4: Run the whole crate's tests**

Run: `cargo test -p outrider-layout`
Expected: PASS (6 unit + 3 property tests).

- [ ] **Step 5: Commit**

```bash
git add crates/outrider-layout/tests/common/mod.rs crates/outrider-layout/tests/props_basic.rs Cargo.lock
git commit -m "test: proptest tree generator; determinism, ordering, containment properties"
```

---

### Task 6: Continuity properties 2 and 3 (grow, insert)

**Files:**
- Create: `crates/outrider-layout/tests/props_continuity.rs`

**Interfaces:**
- Consumes: `common::{g_folder, to_tree, FRESH_NAMES}`, `outrider_layout::{layout, lines_per_cell, WorldLayout}`, `outrider_index::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree}`.
- Produces: nothing consumed later — this is the continuity exit gate.

**The displacement bound (spec §8.1 #2–3, made precise for pure recomputation):** perturbing one node may change the `CellRange` of (a) the node itself, (b) at each ancestor level, the *later siblings* of the node/ancestor on the path (their `start` shifts), and (c) an ancestor itself only if its own `len` changed (slack overflow bubbling up). Nothing else may change — in particular **predecessor siblings never move**, and a shifted sibling's *descendants* keep their parent-relative ranges (hierarchical addressing). The `allowed_changed` helper computes exactly this set by walking the path leaf→root and stopping at the first ancestor whose `len` is unchanged (unchanged `len` ⇒ unchanged gap ⇒ nothing above is affected).

- [ ] **Step 1: Write the continuity tests.** Create `crates/outrider-layout/tests/props_continuity.rs`:

```rust
mod common;

use std::collections::BTreeSet;

use common::{g_folder, to_tree, FRESH_NAMES};
use outrider_index::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::{layout, lines_per_cell, WorldLayout};
use proptest::prelude::*;

/// Index paths (through `children`) of all leaves.
fn leaf_paths(node: &SymbolNode, prefix: Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if node.children.is_empty() {
        out.push(prefix);
        return;
    }
    for (i, c) in node.children.iter().enumerate() {
        let mut p = prefix.clone();
        p.push(i);
        leaf_paths(c, p, out);
    }
}

/// Index paths of all folders (insert targets).
fn folder_paths(node: &SymbolNode, prefix: Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if node.id.kind == SymbolKind::Folder {
        out.push(prefix.clone());
    }
    for (i, c) in node.children.iter().enumerate() {
        let mut p = prefix.clone();
        p.push(i);
        folder_paths(c, p, out);
    }
}

fn node_at<'a>(root: &'a SymbolNode, path: &[usize]) -> &'a SymbolNode {
    path.iter().fold(root, |n, &i| &n.children[i])
}

fn node_at_mut<'a>(root: &'a mut SymbolNode, path: &[usize]) -> &'a mut SymbolNode {
    path.iter().fold(root, |n, &i| &mut n.children[i])
}

/// The exact allowed-changed set (see task header). `path` addresses the
/// perturbed node in `t` (the *perturbed* tree); ancestor ids are identical
/// in both trees. A node absent from `w1` (freshly inserted) counts as
/// "len changed", so the walk continues upward past it.
fn allowed_changed(
    t: &SymbolTree,
    path: &[usize],
    w1: &WorldLayout,
    w2: &WorldLayout,
) -> BTreeSet<SymbolId> {
    let mut allowed = BTreeSet::new();
    for k in (0..=path.len()).rev() {
        let node = node_at(&t.root, &path[..k]);
        allowed.insert(node.id.clone());
        if k > 0 {
            // later siblings, by (name, ordinal) order
            let parent = node_at(&t.root, &path[..k - 1]);
            let mut order: Vec<&SymbolNode> = parent.children.iter().collect();
            order.sort_by(|a, b| {
                a.name
                    .as_bytes()
                    .cmp(b.name.as_bytes())
                    .then(a.id.ordinal.cmp(&b.id.ordinal))
            });
            let pos = order.iter().position(|c| c.id == node.id).unwrap();
            for later in &order[pos + 1..] {
                allowed.insert(later.id.clone());
            }
        }
        let len_changed = match (w1.nodes.get(&node.id), w2.nodes.get(&node.id)) {
            (Some(a), Some(b)) => a.cells.len != b.cells.len,
            _ => true,
        };
        if !len_changed {
            break;
        }
    }
    allowed
}

fn assert_only_allowed_changed(
    t2: &SymbolTree,
    path: &[usize],
    w1: &WorldLayout,
    w2: &WorldLayout,
) {
    let allowed = allowed_changed(t2, path, w1, w2);
    for (id, n1) in &w1.nodes {
        if let Some(n2) = w2.nodes.get(id) {
            if n1 != n2 {
                assert!(
                    allowed.contains(id),
                    "unexpectedly changed: {id:?}\n  before {n1:?}\n  after  {n2:?}"
                );
            }
        }
    }
}

proptest! {
    /// Spec §8.1 property 2: grow one leaf by one cell-worth of lines.
    #[test]
    fn continuity_grow(g in g_folder(), sel in any::<prop::sample::Index>()) {
        let t1 = to_tree(&g);
        let mut paths = Vec::new();
        leaf_paths(&t1.root, Vec::new(), &mut paths);
        let path = paths[sel.index(paths.len())].clone();

        let mut t2 = t1.clone();
        {
            let leaf = node_at_mut(&mut t2.root, &path);
            leaf.measure += lines_per_cell(leaf.id.kind); // exactly +1 cell
        }
        let w1 = layout(&t1);
        let w2 = layout(&t2);
        prop_assert_eq!(w1.nodes.len(), w2.nodes.len());
        // the leaf itself must have grown by exactly one cell
        let leaf_id = &node_at(&t2.root, &path).id;
        prop_assert_eq!(w2.nodes[leaf_id].cells.len, w1.nodes[leaf_id].cells.len + 1);
        assert_only_allowed_changed(&t2, &path, &w1, &w2);
    }

    /// Spec §8.1 property 3: insert one new file into a folder.
    #[test]
    fn continuity_insert(
        g in g_folder(),
        sel in any::<prop::sample::Index>(),
        name_sel in any::<prop::sample::Index>(),
        lines in 1u64..3000,
    ) {
        let t1 = to_tree(&g);
        let mut folders = Vec::new();
        folder_paths(&t1.root, Vec::new(), &mut folders);
        let fpath = folders[sel.index(folders.len())].clone();

        let mut t2 = t1.clone();
        let new_id = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "n-inserted".into(),
            ordinal: 0,
        };
        {
            let folder = node_at_mut(&mut t2.root, &fpath);
            folder.children.push(SymbolNode {
                id: new_id.clone(),
                // fresh name: never collides with NAMES, so existing
                // siblings keep their ordinals (and therefore their ids)
                name: FRESH_NAMES[name_sel.index(FRESH_NAMES.len())].to_string(),
                byte_range: None,
                measure: lines,
                churn: 0.0,
                churn_count: 0,
                children: vec![],
            });
            finalize_children(&mut folder.children);
        }
        let w1 = layout(&t1);
        let w2 = layout(&t2);
        prop_assert_eq!(w2.nodes.len(), w1.nodes.len() + 1);

        // path to the inserted node in t2
        let folder = node_at(&t2.root, &fpath);
        let idx = folder.children.iter().position(|c| c.id == new_id).unwrap();
        let mut path = fpath.clone();
        path.push(idx);
        assert_only_allowed_changed(&t2, &path, &w1, &w2);
    }
}
```

- [ ] **Step 2: Run the continuity properties**

Run: `cargo test -p outrider-layout --test props_continuity`
Expected: PASS. **If a case fails:** read the minimized counterexample carefully. If a node outside the allowed set moved, the algorithm violates the continuity contract — fix `measure.rs`/`arrange.rs` (candidates: a pooled quantity crept in, or gap depends on something besides the child's own len). Do not widen `allowed_changed` to make the test pass without controller sign-off (spec §10: tighten the algorithm, not the test).

- [ ] **Step 3: Run the whole workspace**

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/outrider-layout/tests/props_continuity.rs
git commit -m "test: continuity properties — bounded displacement on grow and insert"
```

---

### Task 7: Cross-process determinism

**Files:**
- Create: `crates/outrider-layout/tests/cross_process.rs`

**Interfaces:**
- Consumes: `outrider_layout::layout`, index types.
- Produces: nothing — closes spec §8.1 property 1's "verified across two separate processes" clause. (No CI exists yet; this test IS the check and will run in CI whenever CI lands.)

**Mechanism:** the test re-executes its own test binary (`std::env::current_exe`) twice with `OUTRIDER_DET_CHILD=1`; in child mode it prints a hash of the canonical layout and returns. `DefaultHasher::new()` uses fixed keys, so hashes are comparable across processes of the same binary.

- [ ] **Step 1: Write the test.** Create `crates/outrider-layout/tests/cross_process.rs`:

```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::process::Command;

use outrider_index::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::layout;

fn node(
    kind: SymbolKind,
    qp: &str,
    name: &str,
    measure: u64,
    mut children: Vec<SymbolNode>,
) -> SymbolNode {
    finalize_children(&mut children);
    SymbolNode {
        id: SymbolId {
            kind,
            qualified_path: qp.into(),
            ordinal: 0,
        },
        name: name.into(),
        byte_range: None,
        measure,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}

/// Worked example plus one extra folder level — fixed by hand, not generated.
fn canonical_tree() -> SymbolTree {
    let b = node(
        SymbolKind::File,
        "src/b.rs",
        "b.rs",
        40,
        vec![
            node(SymbolKind::Fn, "src/b.rs::f", "f", 10, vec![]),
            node(SymbolKind::Fn, "src/b.rs::g", "g", 1, vec![]),
        ],
    );
    let src = node(
        SymbolKind::Folder,
        "src",
        "src",
        140,
        vec![node(SymbolKind::File, "src/a.rs", "a.rs", 100, vec![]), b],
    );
    let root = node(
        SymbolKind::Folder,
        "",
        "",
        141,
        vec![src, node(SymbolKind::File, "README.md", "README.md", 30, vec![])],
    );
    SymbolTree {
        root,
        repo_root: "/canon".into(),
    }
}

fn layout_hash() -> u64 {
    let w = layout(&canonical_tree());
    let mut h = DefaultHasher::new(); // fixed keys — stable across processes
    format!("{w:?}").hash(&mut h);
    h.finish()
}

#[test]
fn layout_hash_identical_across_processes() {
    if std::env::var("OUTRIDER_DET_CHILD").is_ok() {
        println!("LAYOUT_HASH={}", layout_hash());
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let run_child = || {
        let out = Command::new(&exe)
            .args([
                "layout_hash_identical_across_processes",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("OUTRIDER_DET_CHILD", "1")
            .output()
            .expect("failed to spawn child test process");
        assert!(out.status.success(), "child process failed");
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("LAYOUT_HASH=").map(str::to_string))
            .expect("child printed no LAYOUT_HASH line")
    };
    let h1 = run_child();
    let h2 = run_child();
    assert_eq!(h1, h2, "two child processes disagree");
    assert_eq!(h1, layout_hash().to_string(), "parent disagrees with children");
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p outrider-layout --test cross_process -- --nocapture`
Expected: PASS (one test; it spawns two child processes internally).

- [ ] **Step 3: Milestone 2 exit check — full workspace**

Run: `cargo test --workspace && cargo clippy --workspace 2>&1 | grep -c "^warning"`
Expected: all tests green; `0` clippy warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/outrider-layout/tests/cross_process.rs
git commit -m "test: cross-process layout determinism via self-exec hash comparison"
```

---

## Exit Gate (spec §9 milestone 2)

All five §8.1 properties pass headless: determinism (in-process + cross-process), continuity-grow, continuity-insert, stable ordering, containment/no-overlap. `cargo test --workspace` green, clippy clean. The layout crate exposes exactly `layout()`, the three types, `RATIO`, `lines_per_cell`, and `absolute_start` — Phase 3 builds on these.
