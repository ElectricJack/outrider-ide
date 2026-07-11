# Beam-Cast Arrow Navigation + Neighbor Highlights Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace center-distance arrow-key navigation with a beam cast (dead key when nothing is hit) and paint a dimmed focus-color border on the four boxes the arrows would land on.

**Architecture:** `spatial_step` in `crates/outrider/src/focus.rs` keeps its signature and eligibility rules but requalifies candidates by orthogonal-span overlap ("beam") and scores by edge-to-edge distance. A new pure `focus::neighbors` runs it for all four directions. `TreemapView` caches the four results keyed by the focused `SymbolId` and paints qualifying items with a blended border via a new `theme::neighbor_border`.

**Tech Stack:** Rust, GPUI (pinned rev — do not touch), existing workspace crates only. No new dependencies.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-11-cardinal-nav-highlight-design.md`.
- Beam overlap is **strict**: for Left/Right a candidate qualifies iff `cand.y < cur.y + cur.h && cand.y + cand.h > cur.y` (same on x for Up/Down). Corner-touching (equality) does NOT qualify.
- Edge distance qualifies at **≥ 0** (shared edges reachable). Score order: lowest edge distance, then lowest orthogonal center misalignment, then lesser `SymbolId`.
- **No fallback**: no qualifier → `None` → arrow key is dead, no highlight in that direction.
- `neighbors` array order is exactly `[Left, Right, Up, Down]`.
- Neighbor border color: `lerp_rgb(FOCUS_BORDER, normal_border, 0.5)` at 1px; focused stays 2px `FOCUS_BORDER`.
- Gates for every task: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo build -p outrider`. One pre-existing clippy warning is expected and NOT yours to fix: `type_complexity` at `crates/outrider-index/src/parse.rs:255`.
- If test harness output is not visible through pipes, run the newest test binary directly: `ls -t target/debug/deps/outrider-* | grep -v '\.d' | head -1` and execute it.

---

### Task 1: Beam-cast `spatial_step` + `neighbors` helper

**Files:**
- Modify: `crates/outrider/src/focus.rs` (function at :113-162, doc comment included; tests at :380-405 replaced; comment inside test at :349 updated)

**Interfaces:**
- Consumes: existing `Dir`, `TreeIndex`, `PackLayout` (`pack.rects: BTreeMap<SymbolId, Rect>` where `Rect {x, y, w, h}: f64`), `crate::content::is_leaf_item`.
- Produces: `pub fn spatial_step(current: &SymbolId, dir: Dir, pack: &PackLayout, index: &TreeIndex) -> Option<SymbolId>` (signature unchanged, new semantics) and `pub fn neighbors(current: &SymbolId, pack: &PackLayout, index: &TreeIndex) -> [Option<SymbolId>; 4]` (order: Left, Right, Up, Down). Task 2 calls `neighbors`.

- [ ] **Step 1: Replace the old scoring test with four failing beam-cast tests**

In `crates/outrider/src/focus.rs`, **delete** the entire test `spatial_step_penalizes_orthogonal_offset` (lines ~380-405) and add in its place (same spot, after `scoring_tree`):

```rust
    #[test]
    fn spatial_step_requires_beam_overlap() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // q is nearest by center but sits entirely below the beam (y-span
        // 11..21 vs c's 0..10); p is farther but on-beam. Old center
        // scoring picked q for Left — the "Left moves down" bug.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: -60.0, y: 2.0, w: 10.0, h: 6.0 }),
            (q.clone(), Rect { x: -15.0, y: 11.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Left, &lay, &idx), Some(p.clone()));
        // Only the off-beam candidate remains → Left is a dead key.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: -15.0, y: 11.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Left, &lay, &idx), None);
        // A node missing from the layout steps nowhere.
        assert_eq!(spatial_step(&c, Dir::Left, &hand_layout(&[]), &idx), None);
    }

    #[test]
    fn spatial_step_nearest_edge_wins() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // Both overlap the beam. q's near edge (x=14, distance 4) beats
        // p's (x=20, distance 10) even though p is perfectly centered
        // and q's center is 10 off-axis.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 14.0, y: -30.0, w: 10.0, h: 50.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(q.clone()));
    }

    #[test]
    fn spatial_step_ties_break_on_misalignment_then_id() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // Equal edge distance (both at x=20). p center-y 11 → misalign 6;
        // q center-y -4 → misalign 9. Better-centered p wins.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 6.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 20.0, y: -9.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
        // Exact tie on distance AND misalignment (6 each): lesser SymbolId
        // wins → p.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 6.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 20.0, y: -6.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
    }

    #[test]
    fn spatial_step_shared_edge_qualifies_but_corner_touch_does_not() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // p shares c's right edge (distance 0) → reachable. q touches only
        // at the bottom-right corner → not reachable Down or Right.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 10.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 10.0, y: 10.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
        assert_eq!(spatial_step(&c, Dir::Down, &lay, &idx), None);
    }

    #[test]
    fn neighbors_returns_left_right_up_down() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // p directly right, q directly below; nothing left or up.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 0.0, y: 20.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(
            neighbors(&c, &lay, &idx),
            [None, Some(p.clone()), None, Some(q.clone())]
        );
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test -p outrider spatial_step -- --nocapture` and `cargo test -p outrider neighbors_returns`
Expected: `neighbors_returns_left_right_up_down` fails to COMPILE (`neighbors` not defined). Comment that one test out temporarily if needed to see the others run; the beam/edge/tie tests must FAIL against the old center-distance scoring (e.g. `spatial_step_nearest_edge_wins` picks `p` under old scoring: p score 20+0=20 beats q 19+... — verify at least one assertion fails).

- [ ] **Step 3: Replace `spatial_step` (doc comment included) and add `neighbors`**

Replace lines 113-162 of `crates/outrider/src/focus.rs` (the doc comment starting `/// Spatial arrow step.` through the end of `spatial_step`) with:

```rust
/// Spatial arrow step: a beam cast. Slide `current`'s rect in `dir`; a
/// candidate qualifies iff its span on the orthogonal axis strictly
/// overlaps the current rect's span (corner contact does not count) and
/// its near edge lies at or beyond the current rect's leading edge.
/// Nearest edge-to-edge distance wins; ties break by orthogonal center
/// misalignment, then SymbolId. When `current` is a leaf page
/// (`content::is_leaf_item`), candidates are all other leaf pages at any
/// tree depth; otherwise candidates are the nodes at `current`'s own
/// depth. No wrap and no fallback: no qualifier → None (dead key).
pub fn spatial_step(
    current: &SymbolId,
    dir: Dir,
    pack: &PackLayout,
    index: &TreeIndex,
) -> Option<SymbolId> {
    let cur = pack.rects.get(current)?;
    let depth = index.depth(current)?;
    let leaf_mode = index.node(current).is_some_and(crate::content::is_leaf_item);
    let mut best: Option<(f64, f64, &SymbolId)> = None;
    for (id, r) in &pack.rects {
        if id == current {
            continue;
        }
        let eligible = if leaf_mode {
            index.node(id).is_some_and(crate::content::is_leaf_item)
        } else {
            index.depth(id) == Some(depth)
        };
        if !eligible {
            continue;
        }
        let (overlap, primary, misalign) = match dir {
            Dir::Left | Dir::Right => (
                r.y < cur.y + cur.h && r.y + r.h > cur.y,
                if dir == Dir::Left { cur.x - (r.x + r.w) } else { r.x - (cur.x + cur.w) },
                ((r.y + r.h / 2.0) - (cur.y + cur.h / 2.0)).abs(),
            ),
            Dir::Up | Dir::Down => (
                r.x < cur.x + cur.w && r.x + r.w > cur.x,
                if dir == Dir::Up { cur.y - (r.y + r.h) } else { r.y - (cur.y + cur.h) },
                ((r.x + r.w / 2.0) - (cur.x + cur.w / 2.0)).abs(),
            ),
        };
        if !overlap || primary < 0.0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((bp, bm, bid)) => {
                primary < bp || (primary == bp && (misalign < bm || (misalign == bm && id < bid)))
            }
        };
        if better {
            best = Some((primary, misalign, id));
        }
    }
    best.map(|(_, _, id)| id.clone())
}

/// The four beam-cast arrow targets of `current`, indexed Left, Right,
/// Up, Down. `None` entries are dead directions (and get no highlight).
pub fn neighbors(
    current: &SymbolId,
    pack: &PackLayout,
    index: &TreeIndex,
) -> [Option<SymbolId>; 4] {
    [Dir::Left, Dir::Right, Dir::Up, Dir::Down].map(|d| spatial_step(current, d, pack, index))
}
```

Also update the stale comment inside `spatial_step_crosses_parent_boundaries_at_same_depth` at the `spatial_step(&f, Dir::Right, ...)` assertion: change `// same x-center → not "right of"` to `// nothing beyond the right edge → dead key`. Do not change that test's assertions — all of them hold under beam-cast semantics (the fn column shares one x-span, so Up/Down beams fully overlap).

- [ ] **Step 4: Run the focus tests**

Run: `cargo test -p outrider focus`
Expected: PASS, including `spatial_step_crosses_parent_boundaries_at_same_depth` and `spatial_step_leaf_mode_crosses_depth_and_skips_non_leaves` unchanged.

- [ ] **Step 5: Gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo build -p outrider`
Expected: all tests pass; clippy shows only the pre-existing `type_complexity` warning at `crates/outrider-index/src/parse.rs:255`.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/focus.rs
git commit -m "feat: beam-cast spatial navigation with dead keys and neighbors()"
```

---

### Task 2: Neighbor highlight — theme helper, cache, paint

**Files:**
- Modify: `crates/outrider/src/theme.rs` (add `neighbor_border` after `border_for` at :98-100; add test in the existing `#[cfg(test)]` module)
- Modify: `crates/outrider/src/treemap.rs` (struct field ~:57, `new()` ~:311, `PaintItem` ~:96, `paint_items` ~:412 and ~:518, canvas border ~:728)

**Interfaces:**
- Consumes: `focus::neighbors(&SymbolId, &PackLayout, &TreeIndex) -> [Option<SymbolId>; 4]` from Task 1 (module already imported — `focus::spatial_step` is called in the arrow handler); `theme::FOCUS_BORDER: u32`; private `theme::lerp_rgb(a: u32, b: u32, t: f32) -> u32`.
- Produces: `pub fn neighbor_border(border: u32) -> u32` in `theme.rs`; `PaintItem.neighbor: bool`; `TreemapView.neighbors: Option<(SymbolId, [Option<SymbolId>; 4])>`.

- [ ] **Step 1: Write the failing theme test**

In the existing `#[cfg(test)]` module of `crates/outrider/src/theme.rs`, add:

```rust
    #[test]
    fn neighbor_border_is_a_genuine_blend() {
        let base = border_for(box_fill(BoxKind::Leaf, 0, BoxTint::Normal));
        let nb = neighbor_border(base);
        assert_ne!(nb, FOCUS_BORDER, "must be dimmer than the focus border");
        assert_ne!(nb, base, "must be visibly different from the normal border");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p outrider neighbor_border`
Expected: FAIL to compile — `neighbor_border` not found.

- [ ] **Step 3: Implement `neighbor_border`**

In `crates/outrider/src/theme.rs`, directly after `border_for` (:98-100), add:

```rust
/// Border for the four arrow-key neighbor targets: the focus color
/// blended halfway toward the item's normal border.
pub fn neighbor_border(border: u32) -> u32 {
    lerp_rgb(FOCUS_BORDER, border, 0.5)
}
```

- [ ] **Step 4: Run the theme tests**

Run: `cargo test -p outrider theme`
Expected: PASS.

- [ ] **Step 5: Wire the neighbor cache and paint flag in `treemap.rs`**

Four edits:

**(a)** Add a field to `TreemapView` (after `bake_pending: bool,` at :57):

```rust
    /// The four beam-cast arrow targets of the focused node (Left, Right,
    /// Up, Down), cached because layout is immutable per session.
    neighbors: Option<(SymbolId, [Option<SymbolId>; 4])>,
```

and initialize it in `new()` (after `bake_pending: false,` at :311):

```rust
            neighbors: None,
```

**(b)** Add a field to `PaintItem` (after `focused: bool,` at :96):

```rust
    /// One of the four arrow-key targets — painted with a dimmed focus border.
    neighbor: bool,
```

**(c)** In `paint_items`, directly after `let focus_id = self.focus.current.clone();` (:412), insert:

```rust
        let stale = !matches!(&self.neighbors, Some((k, _)) if k == &focus_id);
        if stale {
            let index = TreeIndex::new(&self.tree);
            self.neighbors =
                Some((focus_id.clone(), focus::neighbors(&focus_id, &self.layout, &index)));
        }
        let (_, neighbor_ids) = self.neighbors.clone().unwrap();
```

and in the `PaintItem` construction (after `focused: item.node.id == focus_id,` at ~:518), add:

```rust
                neighbor: item.node.id != focus_id
                    && neighbor_ids.iter().flatten().any(|n| *n == item.node.id),
```

**(d)** In the canvas Pass 1 border resolution (:728-732), replace:

```rust
                            let (bw, bc) = if item.focused {
                                (2.0, theme::FOCUS_BORDER)
                            } else {
                                (1.0, item.border)
                            };
```

with:

```rust
                            let (bw, bc) = if item.focused {
                                (2.0, theme::FOCUS_BORDER)
                            } else if item.neighbor {
                                (1.0, theme::neighbor_border(item.border))
                            } else {
                                (1.0, item.border)
                            };
```

- [ ] **Step 6: Gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo build -p outrider`
Expected: all tests pass; clippy shows only the pre-existing `type_complexity` warning at `crates/outrider-index/src/parse.rs:255`.

- [ ] **Step 7: Commit**

```bash
git add crates/outrider/src/theme.rs crates/outrider/src/treemap.rs
git commit -m "feat: highlight the four arrow-key neighbor targets"
```

---

## Manual verification (after both tasks)

Run `cargo run -p outrider -- .`:
- The four highlighted borders track focus as you click and arrow around.
- Every arrow press lands exactly on a highlighted box.
- A direction with no highlight is a dead key (focus does not move).
- Left never moves down.
