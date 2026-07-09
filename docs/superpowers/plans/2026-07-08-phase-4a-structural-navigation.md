# Phase 4a: Structural Navigation + Camera-Follow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keyboard-primary navigation — arrow keys step focus through the symbol tree and the camera follows with a 250 ms eased tween (Bet #1's motion half).

**Architecture:** A GPUI-free `focus.rs` (focus state + tree stepping), pure framing/tween math added to `camera.rs`, `world_band`/`hit_test` added to `world.rs`, and thin wiring in `treemap.rs` (key handlers, click-to-focus, animation-frame drive, accent focus border). Spec: `docs/superpowers/specs/2026-07-08-phase-4a-structural-navigation-design.md`.

**Tech Stack:** Rust, GPUI (pinned), existing `outrider-index`/`outrider-layout` types.

## Global Constraints

- Every cargo command needs `export PATH="$HOME/.cargo/bin:$PATH" && ` first (rustup lives outside the default PATH).
- GPUI stays pinned at rev `029bf2f284b4e59f20175d78443e630468f3a3e5` — NEVER bump it to fix a compile error.
- `focus.rs`, `camera.rs`, `world.rs` stay GPUI-free (no `gpui::` types). GPUI appears only in `treemap.rs`/`main.rs`.
- The app mutates neither `SymbolTree` nor `WorldLayout` (one-way data flow).
- Exact constants (spec §3–§4): `FOCUS_FRACTION = 0.5`, `END_FRACTION = 0.95`, `TWEEN_SECS = 0.25`. Zoom clamps unchanged: `min_zoom = home_zoom * 0.5`, `max_zoom = vh * 8^15`.
- Tween: ease-in-out cubic; `center_y` linear; `zoom` geometric (log-space). `sample(t ≥ duration)` returns `to` exactly.
- Left pops history without pushing; every other focus change pushes the previous focus. Landing on node N records `last_child[parent(N)] = N`. Tab gets no handler.
- Click sets focus but never moves the camera.
- `cargo clippy --workspace -- -D warnings` must stay clean.

---

### Task 1: Focus model (`focus.rs`)

**Files:**
- Create: `crates/outrider/src/focus.rs`
- Modify: `crates/outrider/src/main.rs` (add `mod focus;` beside the existing `mod camera;` etc.)

**Interfaces:**
- Consumes: `outrider_index::{SymbolId, SymbolNode, SymbolTree}` (children are already name-sorted; `SymbolId` is `Ord + Clone`).
- Produces (Task 3 relies on these exact signatures):
  - `TreeIndex::new(&'a SymbolTree) -> TreeIndex<'a>`, `TreeIndex::node(&SymbolId) -> Option<&'a SymbolNode>`, `TreeIndex::parent(&SymbolId) -> Option<&'a SymbolId>`
  - `Focus::new(root: SymbolId) -> Focus`, field `pub current: SymbolId`
  - `Focus::step_in(&mut self, &TreeIndex) -> bool` (Right), `step_sibling(&mut self, delta: isize, &TreeIndex) -> bool` (Up = -1, Down = +1), `step_back(&mut self, &TreeIndex) -> bool` (Left), `set(&mut self, SymbolId, &TreeIndex) -> bool` (click). Return = focus changed.

- [ ] **Step 1: Write `crates/outrider/src/focus.rs` with failing tests**

```rust
use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};

/// Lookup maps over an immutable SymbolTree, built once per use.
pub struct TreeIndex<'a> {
    nodes: BTreeMap<&'a SymbolId, &'a SymbolNode>,
    parents: BTreeMap<&'a SymbolId, &'a SymbolId>,
}

impl<'a> TreeIndex<'a> {
    pub fn new(tree: &'a SymbolTree) -> Self {
        fn walk<'a>(node: &'a SymbolNode, idx: &mut TreeIndex<'a>) {
            idx.nodes.insert(&node.id, node);
            for c in &node.children {
                idx.parents.insert(&c.id, &node.id);
                walk(c, idx);
            }
        }
        let mut idx = TreeIndex { nodes: BTreeMap::new(), parents: BTreeMap::new() };
        walk(&tree.root, &mut idx);
        idx
    }

    pub fn node(&self, id: &SymbolId) -> Option<&'a SymbolNode> {
        self.nodes.get(id).copied()
    }

    pub fn parent(&self, id: &SymbolId) -> Option<&'a SymbolId> {
        self.parents.get(id).copied()
    }
}

/// Keyboard focus (phase-4a spec §2): linear history stack + last-visited
/// child memory. Focus never dangles in 4a — the tree is immutable at
/// runtime until live reload (Phase 6).
pub struct Focus {
    pub current: SymbolId,
    history: Vec<SymbolId>,
    last_child: BTreeMap<SymbolId, SymbolId>,
}

impl Focus {
    pub fn new(root: SymbolId) -> Self {
        Focus { current: root, history: Vec::new(), last_child: BTreeMap::new() }
    }

    /// Land on `next`: push the previous focus, record last-visited on the
    /// parent. Landing on the current focus is a no-op (returns false).
    fn land(&mut self, next: SymbolId, index: &TreeIndex) -> bool {
        if next == self.current {
            return false;
        }
        let prev = std::mem::replace(&mut self.current, next);
        self.history.push(prev);
        self.record_visit(index);
        true
    }

    fn record_visit(&mut self, index: &TreeIndex) {
        if let Some(p) = index.parent(&self.current) {
            self.last_child.insert(p.clone(), self.current.clone());
        }
    }

    /// Right: last-visited child if still valid, else first child.
    pub fn step_in(&mut self, index: &TreeIndex) -> bool {
        let Some(node) = index.node(&self.current) else { return false };
        if node.children.is_empty() {
            return false;
        }
        let next = self
            .last_child
            .get(&self.current)
            .filter(|lc| node.children.iter().any(|c| &c.id == *lc))
            .cloned()
            .unwrap_or_else(|| node.children[0].id.clone());
        self.land(next, index)
    }

    /// Up (-1) / Down (+1): cycle name-ordered siblings, wrapping.
    pub fn step_sibling(&mut self, delta: isize, index: &TreeIndex) -> bool {
        let Some(parent_id) = index.parent(&self.current) else { return false };
        let parent = index.node(parent_id).expect("parent id is in the index");
        let n = parent.children.len() as isize;
        let i = parent
            .children
            .iter()
            .position(|c| c.id == self.current)
            .expect("focus is a child of its parent") as isize;
        let next = parent.children[(i + delta).rem_euclid(n) as usize].id.clone();
        self.land(next, index)
    }

    /// Left: pop the history stack — no push (spec §2).
    pub fn step_back(&mut self, index: &TreeIndex) -> bool {
        let Some(prev) = self.history.pop() else { return false };
        self.current = prev;
        self.record_visit(index);
        true
    }

    /// Click: set focus directly. The caller must not move the camera.
    pub fn set(&mut self, id: SymbolId, index: &TreeIndex) -> bool {
        self.land(id, index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolKind, SymbolTree};

    fn n(
        kind: SymbolKind,
        qp: &str,
        name: &str,
        children: Vec<outrider_index::SymbolNode>,
    ) -> outrider_index::SymbolNode {
        outrider_index::SymbolNode {
            id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
            name: name.into(),
            byte_range: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    /// The Phase 2 worked example: root { a.rs, b.rs { f, g } }.
    fn tree() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                vec![
                    n(SymbolKind::File, "a.rs", "a.rs", vec![]),
                    n(
                        SymbolKind::File,
                        "b.rs",
                        "b.rs",
                        vec![
                            n(SymbolKind::Fn, "b.rs::f", "f", vec![]),
                            n(SymbolKind::Fn, "b.rs::g", "g", vec![]),
                        ],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    fn id(kind: SymbolKind, qp: &str) -> SymbolId {
        SymbolId { kind, qualified_path: qp.into(), ordinal: 0 }
    }

    #[test]
    fn right_steps_into_first_child_and_leaf_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "a.rs"));
        assert!(!f.step_in(&idx)); // a.rs is a leaf
        assert_eq!(f.current, id(SymbolKind::File, "a.rs"));
    }

    #[test]
    fn up_down_cycle_and_wrap() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.step_in(&idx); // a.rs
        assert!(f.step_sibling(1, &idx));
        assert_eq!(f.current, id(SymbolKind::File, "b.rs"));
        assert!(f.step_sibling(1, &idx)); // wraps
        assert_eq!(f.current, id(SymbolKind::File, "a.rs"));
        assert!(f.step_sibling(-1, &idx)); // wraps the other way
        assert_eq!(f.current, id(SymbolKind::File, "b.rs"));
    }

    #[test]
    fn sibling_at_root_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        assert!(!f.step_sibling(1, &idx));
        assert!(!f.step_sibling(-1, &idx));
        assert_eq!(f.current, t.root.id);
    }

    #[test]
    fn left_pops_history_and_empty_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.step_in(&idx); // a.rs (push root)
        f.step_sibling(1, &idx); // b.rs (push a.rs)
        assert!(f.step_back(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "a.rs"));
        assert!(f.step_back(&idx));
        assert_eq!(f.current, t.root.id);
        assert!(!f.step_back(&idx)); // stack empty
    }

    #[test]
    fn right_remembers_last_visited_child() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.step_in(&idx); // a.rs
        f.step_sibling(1, &idx); // b.rs → last_child[root] = b.rs
        assert!(f.set(t.root.id.clone(), &idx)); // click back to root
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "b.rs")); // not a.rs
    }

    #[test]
    fn set_to_current_is_noop_and_pushes_nothing() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        assert!(!f.set(t.root.id.clone(), &idx));
        assert!(!f.step_back(&idx)); // nothing was pushed
    }
}
```

- [ ] **Step 2: Add `mod focus;` to `crates/outrider/src/main.rs`** (next to the existing `mod camera;` / `mod theme;` / `mod treemap;` / `mod world;` declarations)

- [ ] **Step 3: Run the tests**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider focus:: 2>&1 | tail -5`
Expected: 6 passed. (The module is new, so tests and implementation land together; verify each test actually exercises the behavior it names before trusting green.)

Note: `focus.rs` is not yet referenced outside tests, so `dead_code` warnings will fail clippy. Add `#[allow(dead_code)]` above `pub struct TreeIndex` impl block items ONLY if clippy complains — and prefer one `#![allow(dead_code)]` at the top of `focus.rs` with a `// TODO(task 3): remove` comment. Task 3 removes it when wiring in.

- [ ] **Step 4: Clippy + full test suite**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings 2>&1 | tail -3 && cargo test 2>&1 | grep -c "test result: ok"`
Expected: `Finished`, count ≥ 13.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/focus.rs crates/outrider/src/main.rs
git commit -m "feat: focus model with history and last-visited child (phase 4a)"
```

---

### Task 2: Framing, tween, world band, hit test

**Files:**
- Modify: `crates/outrider/src/camera.rs` (append after `impl Camera`)
- Modify: `crates/outrider/src/world.rs` (append after `visible_nodes`/`walk`)

**Interfaces:**
- Consumes: `Camera` (Copy), `WorldLayout::absolute_start` (exists in `outrider-layout`), `world::column_scale`, `world::DrawItem`.
- Produces (Task 3 relies on these exact signatures):
  - `camera::FOCUS_FRACTION: f64 = 0.5`, `camera::END_FRACTION: f64 = 0.95`, `camera::TWEEN_SECS: f64 = 0.25`
  - `camera::frame_band(y: f64, h: f64, vh: f64, fraction: f64, min_zoom: f64, max_zoom: f64) -> Camera`
  - `camera::CameraTween` (Copy) with `new(from: Camera, to: Camera) -> Self`, `sample(&self, t: f64) -> Camera`, `done(&self, t: f64) -> bool`, `retarget(&self, t: f64, to: Camera) -> CameraTween`
  - `world::world_band(id: &SymbolId, layout: &WorldLayout) -> Option<(f64, f64)>` (y, h)
  - `world::hit_test<'a>(items: &'a [DrawItem<'a>], x: f64, y: f64) -> Option<&'a DrawItem<'a>>`

- [ ] **Step 1: Append to `crates/outrider/src/camera.rs`** (implementation and tests; the spec deviation note: `world_band` drops the spec's `TreeIndex` parameter because `WorldLayout` already stores parents)

```rust
/// Arrow-step framing: the focus band lands at half the viewport height.
pub const FOCUS_FRACTION: f64 = 0.5;
/// End-key framing: the focus band fills the viewport.
pub const END_FRACTION: f64 = 0.95;
/// Camera-follow tween duration, seconds (spec: ~250 ms, interruptible).
pub const TWEEN_SECS: f64 = 0.25;

/// Camera showing world band (y, h) at `fraction` of the viewport height,
/// centered. The zoom clamp may prevent exact framing (accepted).
pub fn frame_band(y: f64, h: f64, vh: f64, fraction: f64, min_zoom: f64, max_zoom: f64) -> Camera {
    Camera {
        center_y: y + h / 2.0,
        zoom: (fraction * vh / h).clamp(min_zoom, max_zoom),
    }
}

fn ease_in_out_cubic(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Eased camera animation, pure and clock-free: the caller supplies elapsed
/// seconds. center_y interpolates linearly; zoom geometrically (log-space)
/// so zoom speed feels uniform across octaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraTween {
    pub from: Camera,
    pub to: Camera,
    pub duration: f64,
}

impl CameraTween {
    pub fn new(from: Camera, to: Camera) -> Self {
        CameraTween { from, to, duration: TWEEN_SECS }
    }

    pub fn sample(&self, t: f64) -> Camera {
        if t >= self.duration {
            return self.to;
        }
        let e = ease_in_out_cubic((t / self.duration).max(0.0));
        Camera {
            center_y: self.from.center_y + (self.to.center_y - self.from.center_y) * e,
            zoom: self.from.zoom * (self.to.zoom / self.from.zoom).powf(e),
        }
    }

    pub fn done(&self, t: f64) -> bool {
        t >= self.duration
    }

    /// Retarget mid-flight: the new tween starts from the current sample,
    /// so motion is continuous — never restarted from the old origin.
    pub fn retarget(&self, t: f64, to: Camera) -> CameraTween {
        CameraTween::new(self.sample(t), to)
    }
}
```

- [ ] **Step 2: Append tests inside camera.rs's existing `mod tests`**

```rust
    #[test]
    fn frame_band_centers_at_fraction() {
        // b.rs::g worked example: band (0.6875, 0.015625), vh 600, fraction ½
        let c = frame_band(0.6875, 0.015625, 600.0, FOCUS_FRACTION, 1e-9, 1e18);
        close(c.zoom, 19200.0); // 0.5·600/0.015625
        close(c.center_y, 0.6953125);
        // clamp may prevent exact framing
        let c = frame_band(0.6875, 0.015625, 600.0, FOCUS_FRACTION, 1e-9, 100.0);
        close(c.zoom, 100.0);
    }

    #[test]
    fn tween_endpoints_exact_and_done() {
        let from = Camera { center_y: 0.0, zoom: 100.0 };
        let to = Camera { center_y: 1.0, zoom: 6400.0 };
        let tw = CameraTween::new(from, to);
        close(tw.duration, TWEEN_SECS);
        assert_eq!(tw.sample(0.0), from);
        assert_eq!(tw.sample(TWEEN_SECS), to); // exact, not approximate
        assert_eq!(tw.sample(TWEEN_SECS * 2.0), to);
        assert!(!tw.done(TWEEN_SECS - 1e-6));
        assert!(tw.done(TWEEN_SECS));
    }

    #[test]
    fn tween_midpoint_linear_y_geometric_zoom() {
        let from = Camera { center_y: 0.0, zoom: 100.0 };
        let to = Camera { center_y: 1.0, zoom: 6400.0 };
        let tw = CameraTween::new(from, to);
        let mid = tw.sample(TWEEN_SECS / 2.0); // ease(½) = ½
        close(mid.center_y, 0.5);
        close(mid.zoom, 800.0); // √(100·6400)
    }

    #[test]
    fn tween_monotonic() {
        let from = Camera { center_y: 0.0, zoom: 100.0 };
        let to = Camera { center_y: 1.0, zoom: 6400.0 };
        let tw = CameraTween::new(from, to);
        let mut last = tw.sample(0.0);
        for i in 1..=100 {
            let c = tw.sample(TWEEN_SECS * i as f64 / 100.0);
            assert!(c.center_y >= last.center_y - 1e-12);
            assert!(c.zoom >= last.zoom - 1e-9);
            last = c;
        }
    }

    #[test]
    fn retarget_is_continuous() {
        let tw = CameraTween::new(
            Camera { center_y: 0.0, zoom: 100.0 },
            Camera { center_y: 1.0, zoom: 6400.0 },
        );
        let other = Camera { center_y: -3.0, zoom: 50.0 };
        let t = 0.1;
        let re = tw.retarget(t, other);
        assert_eq!(re.sample(0.0), tw.sample(t)); // no jump at the splice
        assert_eq!(re.to, other);
    }
```

- [ ] **Step 3: Append to `crates/outrider/src/world.rs`** (after `walk`, before `#[cfg(test)]`; `SymbolId` needs importing — change the existing `use outrider_index::{SymbolNode, SymbolTree};` to `use outrider_index::{SymbolId, SymbolNode, SymbolTree};`)

```rust
/// Absolute world-y band (y, h) of a node: full ancestor composition via
/// WorldLayout::absolute_start, then y = abs·8^-level, h = len·8^-level.
/// (The render walk composes incrementally; this is for framing targets.)
pub fn world_band(id: &SymbolId, layout: &WorldLayout) -> Option<(f64, f64)> {
    let nl = layout.nodes.get(id)?;
    let abs = layout.absolute_start(id)? as f64;
    debug_assert!(abs < 2f64.powi(53), "cell address exceeds exact f64 range");
    let s = column_scale(nl.cells.level);
    Some((abs * s, nl.cells.len as f64 * s))
}

/// Visible node containing the point. Columns are horizontally disjoint,
/// so at most one item matches; take the last (deepest) for robustness.
pub fn hit_test<'a>(items: &'a [DrawItem<'a>], x: f64, y: f64) -> Option<&'a DrawItem<'a>> {
    items
        .iter()
        .rev()
        .find(|i| x >= i.px.x && x < i.px.x + i.px.w && y >= i.px.y && y < i.px.y + i.px.h)
}
```

- [ ] **Step 4: Append tests inside world.rs's existing `mod tests`**

```rust
    #[test]
    fn world_band_composes_ancestors() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // g: depth 2, abs cell 44, len 1 (the Phase 2 worked example)
        let g_id = tree.root.children[1].children[1].id.clone();
        let (y, h) = world_band(&g_id, &layout).unwrap();
        close(y, 0.6875);
        close(h, 0.015625);
        let (y, h) = world_band(&tree.root.id, &layout).unwrap();
        close(y, 0.0);
        close(h, 1.0);
        let unknown = outrider_index::SymbolId {
            kind: SymbolKind::Fn,
            qualified_path: "nope".into(),
            ordinal: 0,
        };
        assert!(world_band(&unknown, &layout).is_none());
    }

    #[test]
    fn hit_test_picks_the_column_under_the_point() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // the zoomed-past-ancestors scene: root [0,36.19), b.rs [36.19,180.95),
        // g [180.95,760) horizontally; g only spans y ∈ [300, 602]
        let cam = Camera { center_y: 0.6875, zoom: 256000.0 / 7.0 };
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        assert_eq!(hit_test(&items, 10.0, 10.0).unwrap().node.name, "");
        assert_eq!(hit_test(&items, 100.0, 500.0).unwrap().node.name, "b.rs");
        assert_eq!(hit_test(&items, 400.0, 450.0).unwrap().node.name, "g");
        assert!(hit_test(&items, 400.0, 100.0).is_none()); // above g's band
        assert!(hit_test(&items, 790.0, 300.0).is_none()); // right of the stack
    }
```

- [ ] **Step 5: Run the tests**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider 2>&1 | grep -E "test result|FAILED"`
Expected: all pass (19 existing + 7 new + Task 1's 6 = 32).

Note: `frame_band`, `CameraTween`, `world_band`, `hit_test` are unused outside tests until Task 3. These are `pub` in a bin crate, so clippy may flag dead_code — same policy as Task 1: add `#[allow(dead_code)]` only to items clippy actually flags, each with `// TODO(task 3): remove`.

- [ ] **Step 6: Clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings 2>&1 | tail -3`
Expected: `Finished`, no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/outrider/src/camera.rs crates/outrider/src/world.rs
git commit -m "feat: frame_band, CameraTween, world_band, hit_test (phase 4a)"
```

---

### Task 3: Wiring — keys, click focus, tween drive, focus border

**Files:**
- Modify: `crates/outrider/src/treemap.rs`
- Modify: `crates/outrider/src/theme.rs`

**Interfaces:**
- Consumes everything Tasks 1–2 produced (exact signatures listed there), plus GPUI's `window.request_animation_frame()` (exists on the pinned rev, `Window`, no args).
- Produces: user-visible behavior only. Remove any `#[allow(dead_code)]`/`TODO(task 3)` markers left by Tasks 1–2 — everything is wired now.

- [ ] **Step 1: Add the theme accent**

In `crates/outrider/src/theme.rs`, after `TEXT_SECONDARY`:

```rust
/// Focused-node border accent (clearly distinct from churn fills/borders).
pub const FOCUS_BORDER: u32 = 0x4da6ff;
```

- [ ] **Step 2: Rework `treemap.rs`**

Imports (top of file) — replace the two `use crate::…` lines:

```rust
use crate::camera::{self, Camera, CameraTween};
use crate::focus::{Focus, TreeIndex};
use crate::theme;
use crate::world::{self, Rung};
```

Struct — replace `TreemapView` fields and `new`:

```rust
pub struct TreemapView {
    tree: SymbolTree,
    layout: WorldLayout,
    /// None until the first render supplies a viewport; then Home-framed.
    camera: Option<Camera>,
    home_zoom: f64,
    drag_last: Option<gpui::Point<Pixels>>,
    press_origin: Option<gpui::Point<Pixels>>,
    focus: Focus,
    tween: Option<(CameraTween, std::time::Instant)>,
    focus_handle: FocusHandle,
}
```

```rust
    pub fn new(tree: SymbolTree, layout: WorldLayout, cx: &mut Context<Self>) -> Self {
        let root_id = tree.root.id.clone();
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
        }
    }
```

Helpers — add to `impl TreemapView` (after `home_camera`):

```rust
    /// Start (or retarget) the camera-follow tween from the current sample.
    /// Retargeting goes through CameraTween::retarget, whose continuity is
    /// unit-tested (spec §7 item 7): from == sampled camera by construction.
    fn start_tween(&mut self, to: Camera) {
        let tw = match self.tween.take() {
            Some((tw, started)) => tw.retarget(started.elapsed().as_secs_f64(), to),
            None => match self.camera {
                Some(c) => CameraTween::new(c, to),
                None => return, // no viewport yet; ignore keys until first render
            },
        };
        self.camera = Some(tw.from);
        self.tween = Some((tw, std::time::Instant::now()));
    }

    /// Mouse is free (spec §4): manual camera ops drop any live tween,
    /// continuing from the current sampled state.
    fn cancel_tween(&mut self) {
        if let Some((tw, started)) = self.tween.take() {
            self.camera = Some(tw.sample(started.elapsed().as_secs_f64()));
        }
    }
```

Tween drive — in `paint_items`, insert at the very top (before the `if self.camera.is_none()` block):

```rust
        if let Some((tw, started)) = self.tween {
            let t = started.elapsed().as_secs_f64();
            self.camera = Some(tw.sample(t));
            if tw.done(t) {
                self.tween = None;
            }
        }
```

Focus flag — `PaintItem` gains `focused: bool`; in the `paint_items` map, capture the id before the closure and set the flag:

```rust
        let focus_id = self.focus.current.clone();
        world::visible_nodes(&self.tree, &self.layout, &camera, vw, vh)
            .into_iter()
            .map(|item| {
                let f = theme::churn_fill(item.node.churn);
                PaintItem {
                    x: item.px.x as f32,
                    y: item.px.y as f32,
                    w: item.px.w as f32,
                    h: item.px.h as f32,
                    fill: f,
                    border: theme::border_for(f),
                    focused: item.node.id == focus_id,
                    rung: item.rung,
                    name: item.node.name.clone(),
                    meta: format!(
                        "{} · p{:.0} · {}L",
                        item.node.churn_count,
                        item.node.churn * 100.0,
                        item.node.measure
                    ),
                }
            })
            .collect()
```

Paint — in the canvas closure, replace the `window.paint_quad(...)` call so the focused node gets the accent border, 2 px:

```rust
                            let (bw, bc) = if item.focused {
                                (2.0, theme::FOCUS_BORDER)
                            } else {
                                (1.0, item.border)
                            };
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                rgb(item.fill),
                                px(bw),
                                rgb(bc),
                                BorderStyle::default(),
                            ));
```

Animation frames — in `render`, after `let items = self.paint_items(vw, vh);`:

```rust
        if self.tween.is_some() {
            window.request_animation_frame();
        }
```

Mouse — replace the three mouse listeners:

```rust
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, _w, _cx| {
                    this.drag_last = Some(e.position);
                    this.press_origin = Some(e.position);
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseUpEvent, w, cx| {
                    this.drag_last = None;
                    let Some(origin) = this.press_origin.take() else { return };
                    let slop = f64::from(e.position.x - origin.x)
                        .abs()
                        .max(f64::from(e.position.y - origin.y).abs());
                    if slop > 4.0 {
                        return; // drag, not click
                    }
                    let Some(cam) = this.camera else { return };
                    let vp = w.viewport_size();
                    let items = world::visible_nodes(
                        &this.tree,
                        &this.layout,
                        &cam,
                        f64::from(vp.width),
                        f64::from(vp.height),
                    );
                    // view fills the window, so window coords == canvas coords
                    let (mx, my) = (f64::from(e.position.x), f64::from(e.position.y));
                    let hit = items
                        .iter()
                        .rev()
                        .find(|i| {
                            mx >= i.px.x && mx < i.px.x + i.px.w && my >= i.px.y && my < i.px.y + i.px.h
                        })
                        .map(|i| i.node.id.clone());
                    drop(items);
                    if let Some(id) = hit {
                        let index = TreeIndex::new(&this.tree);
                        // click sets focus; camera does NOT move (spec §2)
                        this.focus.set(id, &index);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _w, cx| {
                if e.pressed_button != Some(gpui::MouseButton::Left) {
                    return;
                }
                let Some(last) = this.drag_last else { return };
                this.cancel_tween();
                let dy = f64::from(e.position.y - last.y);
                if let Some(cam) = this.camera.as_mut() {
                    cam.pan(dy);
                }
                this.drag_last = Some(e.position);
                cx.notify();
            }))
```

(Use `world::hit_test(&items, mx, my)` instead of the inline `.rev().find(...)` if the borrow checker is happy — it should be: `let hit = world::hit_test(&items, mx, my).map(|i| i.node.id.clone());`. Prefer the helper; the inline form above is the fallback.)

Scroll — add `this.cancel_tween();` as the first statement inside the scroll-wheel listener body (before computing `dy`).

Keys — replace the `on_key_down` listener entirely:

```rust
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, w, cx| {
                if this.camera.is_none() {
                    return;
                }
                let vh = f64::from(w.viewport_size().height);
                let max_zoom = vh * 8f64.powi(15);
                let min_zoom = this.home_zoom * 0.5;
                let index = TreeIndex::new(&this.tree);
                let key = e.keystroke.key.as_str();
                let moved = match key {
                    "right" => this.focus.step_in(&index),
                    "left" => this.focus.step_back(&index),
                    "up" => this.focus.step_sibling(-1, &index),
                    "down" => this.focus.step_sibling(1, &index),
                    _ => false,
                };
                let target = match key {
                    "right" | "left" | "up" | "down" => {
                        if !moved {
                            return;
                        }
                        world::world_band(&this.focus.current, &this.layout).map(|(y, h)| {
                            camera::frame_band(y, h, vh, camera::FOCUS_FRACTION, min_zoom, max_zoom)
                        })
                    }
                    "end" => world::world_band(&this.focus.current, &this.layout).map(|(y, h)| {
                        camera::frame_band(y, h, vh, camera::END_FRACTION, min_zoom, max_zoom)
                    }),
                    "home" => {
                        let c = this.home_camera(vh);
                        this.home_zoom = c.zoom;
                        Some(c)
                    }
                    _ => return, // Tab included: explicitly no handler
                };
                if let Some(to) = target {
                    this.start_tween(to);
                    cx.notify();
                }
            }))
```

- [ ] **Step 3: Remove dead-code allowances from Tasks 1–2**

Delete any `#[allow(dead_code)]` / `#![allow(dead_code)]` markers tagged `TODO(task 3)` in `focus.rs`, `camera.rs`, `world.rs`. If clippy then reports something genuinely unused, wire it or delete it — do not re-add the allow.

- [ ] **Step 4: Full test suite + clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test 2>&1 | grep -E "test result" && cargo clippy --workspace -- -D warnings 2>&1 | tail -3`
Expected: all suites ok; clippy `Finished`.

- [ ] **Step 5: Build the binary**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p outrider 2>&1 | tail -3`
Expected: `Finished`. (Manual feel-testing happens at the exit gate, not in this task — WSLg rendering can't be verified headless.)

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/treemap.rs crates/outrider/src/theme.rs crates/outrider/src/focus.rs crates/outrider/src/camera.rs crates/outrider/src/world.rs
git commit -m "feat: keyboard navigation with camera-follow tween (phase 4a)"
```

---

## Exit gate (manual, after all tasks)

`export PATH="$HOME/.cargo/bin:$PATH" && cargo run -p outrider -- .` — arrow-step through Outrider's own repo: Right/Left/Up/Down follow with ~250 ms ease; a key mid-flight retargets without a jump; mouse drag/wheel cancels cleanly; End fills the viewport with the focus; Home reframes root; click focuses without moving the camera; Tab does nothing. Then the informal Bet #1 read (place vs slideshow), recorded in the ledger.
