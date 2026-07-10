# Spatial Treemap Pivot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the icicle/column layout with a real 2D treemap — the whole repo packed bottom-up from code-page-shaped leaves — with a 2D camera and map-first navigation.

**Architecture:** World units are natural pixels (zoom 1.0 = code at natural size), so the entire 4b–4d content stack (rungs, code scaling, highlighting) transfers unchanged. A new shelf packer in `outrider-layout` produces absolute rects; `camera.rs` gains `center_x`; `world.rs` gets a rect-based cull walk; `treemap.rs`/`main.rs` switch over; the old cell system is then deleted; spatial arrow stepping lands last. Spec: `docs/superpowers/specs/2026-07-09-spatial-treemap-pivot-design.md`.

**Tech Stack:** Rust workspace (`outrider-index`, `outrider-layout`, `outrider` bin), GPUI (only `treemap.rs`/`main.rs` touch it).

## Global Constraints

- Every cargo command needs `export PATH="$HOME/.cargo/bin:$PATH" && ` prefixed.
- After each task: `cargo test --workspace` green AND `cargo clippy --workspace --all-targets -- -D warnings` clean.
- The bin crate denies unused-code warnings: new public items in `crates/outrider/src/*` that are not yet wired get a temporary `#[allow(dead_code)]` with comment `// TODO(pivot): consumed in the switchover task`; the switchover task (Task 4) removes every one of them. The lib crate does not need this.
- Pack constants (used verbatim everywhere): `page_w = 480.0`, `line_step = 15.6` (= content::LINE_STEP), `header = 20.8` (= content::HEADER), `bottom_pad = 6.0` (= content::BOTTOM_PAD), `gap = 8.0`, `aspect = 1.6`.
- Camera constants: `FOCUS_FRACTION = 0.5`, `END_FRACTION = 0.95`, `TWEEN_SECS = 0.25`, `MAX_ZOOM = 8.0`; `min_zoom = 0.5 × home (fit) zoom`.
- Leaf box height formula (any childless node): `header + (1 + measure) · line_step + bottom_pad`. Leaf width: `page_w`.
- Child ordering inside a container: `(name bytes, ordinal)` — identical to the old arrange pass.
- Do NOT touch: `content.rs`, `buffers.rs`, `theme.rs`, anything in `outrider-index`.

---

### Task 1: Shelf packer (`outrider-layout/src/pack.rs`)

**Files:**
- Create: `crates/outrider-layout/src/pack.rs`
- Modify: `crates/outrider-layout/src/lib.rs` (add module + re-exports; keep the old ones for now)

**Interfaces:**
- Consumes: `outrider_index::{SymbolId, SymbolNode, SymbolTree}` (existing).
- Produces (later tasks rely on these exact names):
  - `pub struct Rect { pub x: f64, pub y: f64, pub w: f64, pub h: f64 }` (Copy)
  - `pub struct PackConfig { pub page_w: f64, pub line_step: f64, pub header: f64, pub bottom_pad: f64, pub gap: f64, pub aspect: f64 }` (Copy)
  - `pub struct PackLayout { pub rects: BTreeMap<SymbolId, Rect> }`
  - `pub fn pack(tree: &SymbolTree, cfg: &PackConfig) -> PackLayout` — absolute rects, root at (0, 0)
  - Re-exported from the crate root: `outrider_layout::{pack, PackConfig, PackLayout, Rect}`

- [ ] **Step 1: Write the failing tests**

Create `crates/outrider-layout/src/pack.rs` with the types, an `unimplemented!()` body for `pack`, and the tests below. Worked-example numbers (derived by hand from the algorithm in Step 3 — they are the source of truth):

- f (measure 10): 480 × 198.4; g (measure 1): 480 × 58.0; a.rs leaf (measure 100): 480 × 1602.4.
- b.rs packs f, g: target_w = max(480, √(480·256.4·1.6)) ≈ 443.8 → 480; g wraps to shelf 2 at y = 206.4; content 480 × 264.4; b.rs = 496 × 301.2; f rel (8, 28.8), g rel (8, 264.0 − 28.8 + 28.8) = (8, 235.2)… use the absolute values below, which are what the tests assert.
- root packs a.rs, b.rs: target_w = max(496, √(918547.2·1.6)) ≈ 1212.3 → both on one shelf; content 984 × 1602.4; root = 1000 × 1639.2.
- Absolute rects: root (0, 0, 1000, 1639.2); a.rs (8, 28.8, 480, 1602.4); b.rs (496, 28.8, 496, 301.2); f (504, 57.6, 480, 198.4); g (504, 264.0, 480, 58.0).

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    fn cfg() -> PackConfig {
        PackConfig {
            page_w: 480.0,
            line_step: 15.6,
            header: 20.8,
            bottom_pad: 6.0,
            gap: 8.0,
            aspect: 1.6,
        }
    }

    fn n(kind: SymbolKind, qp: &str, name: &str, measure: u64, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
            name: name.into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    /// The worked example: root { a.rs(100), b.rs(40) { f(10), g(1) } }.
    fn worked_example() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                0,
                vec![
                    n(SymbolKind::File, "a.rs", "a.rs", 100, vec![]),
                    n(
                        SymbolKind::File,
                        "b.rs",
                        "b.rs",
                        40,
                        vec![
                            n(SymbolKind::Fn, "b.rs::f", "f", 10, vec![]),
                            n(SymbolKind::Fn, "b.rs::g", "g", 1, vec![]),
                        ],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    fn rect(p: &PackLayout, qp: &str) -> Rect {
        *p.rects
            .iter()
            .find(|(id, _)| id.qualified_path == qp)
            .map(|(_, r)| r)
            .unwrap()
    }

    fn assert_rect(r: Rect, x: f64, y: f64, w: f64, h: f64) {
        close(r.x, x);
        close(r.y, y);
        close(r.w, w);
        close(r.h, h);
    }

    #[test]
    fn worked_example_exact_rects() {
        let p = pack(&worked_example(), &cfg());
        assert_eq!(p.rects.len(), 5);
        // leaf pages: w = page_w, h = header + (1+measure)·line_step + bottom_pad
        assert_rect(rect(&p, "a.rs"), 8.0, 28.8, 480.0, 1602.4);
        // b.rs: f and g don't fit one 480-wide shelf → two shelves
        assert_rect(rect(&p, "b.rs::f"), 504.0, 57.6, 480.0, 198.4);
        assert_rect(rect(&p, "b.rs::g"), 504.0, 264.0, 480.0, 58.0);
        assert_rect(rect(&p, "b.rs"), 496.0, 28.8, 496.0, 301.2);
        // root: a.rs and b.rs share one shelf (984 ≤ target_w ≈ 1212.3)
        assert_rect(rect(&p, ""), 0.0, 0.0, 1000.0, 1639.2);
    }

    #[test]
    fn deterministic() {
        let a = pack(&worked_example(), &cfg());
        let b = pack(&worked_example(), &cfg());
        assert_eq!(a.rects, b.rects);
    }

    #[test]
    fn children_placed_by_name_then_ordinal_never_size() {
        // "zeta" is huge, "alpha" tiny — alpha still comes first.
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
        // alpha is placed first: top-left of the content area
        close(a.x, 8.0);
        close(a.y, 28.8);
        assert!(z.y > a.y, "zeta wraps below alpha (or right of it)");
    }

    #[test]
    fn sibling_subtree_stable_under_edit() {
        // Grow f (10 → 50 lines): b.rs reflows internally and root resizes,
        // but a.rs — a sibling subtree — keeps its exact position, and g
        // only shifts along y inside b.rs.
        let before = pack(&worked_example(), &cfg());
        let mut edited = worked_example();
        edited.root.children[1].children[0].measure = 50;
        let after = pack(&edited, &cfg());
        assert_eq!(rect(&before, "a.rs"), rect(&after, "a.rs"));
        // f: 480 × 822.4 now; b.rs: 496 × 925.2; g slides down, same x
        assert_rect(rect(&after, "b.rs::f"), 504.0, 57.6, 480.0, 822.4);
        assert_rect(rect(&after, "b.rs"), 496.0, 28.8, 496.0, 925.2);
        let g = rect(&after, "b.rs::g");
        close(g.x, 504.0);
        close(g.y, 888.0);
    }

    #[test]
    fn wide_child_sets_the_floor_for_target_width() {
        // A container whose packed width exceeds √(area·aspect) still fits:
        // target_w = max(widest child, …) — no child is ever split.
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
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-layout pack`
Expected: FAIL — `unimplemented!()` panics (or compile error if types are missing).

- [ ] **Step 3: Implement the packer**

Full implementation of `crates/outrider-layout/src/pack.rs` (above the tests):

```rust
use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};

/// An absolute world rectangle. World units are natural pixels: a leaf
/// page at zoom 1.0 renders at exactly this size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Sizing knobs, passed in by the app so this crate stays independent of
/// the render-side content constants.
#[derive(Debug, Clone, Copy)]
pub struct PackConfig {
    /// Leaf page width (world px).
    pub page_w: f64,
    /// Per-code-line height; leaf h = header + (1+measure)·line_step + bottom_pad.
    pub line_step: f64,
    /// Name-row strip height, reserved at the top of every container too.
    pub header: f64,
    pub bottom_pad: f64,
    /// Space between siblings, both axes; also the container's inner margin.
    pub gap: f64,
    /// Target container width/height ratio for shelf wrapping.
    pub aspect: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackLayout {
    /// Absolute rects for every node; the root sits at (0, 0).
    pub rects: BTreeMap<SymbolId, Rect>,
}

/// Shelf-pack the tree bottom-up (spec §3). Pure and deterministic; a
/// container's internal layout depends only on its own children's sizes,
/// so an edit repacks only its ancestor chain (hierarchical stability).
pub fn pack(tree: &SymbolTree, cfg: &PackConfig) -> PackLayout {
    let mut rel = BTreeMap::new();
    size(&tree.root, cfg, &mut rel);
    let mut rects = BTreeMap::new();
    absolute(&tree.root, 0.0, 0.0, &rel, &mut rects);
    PackLayout { rects }
}

/// Bottom-up size pass: returns (w, h) and records each node's position
/// relative to its parent's origin in `rel` (x, y, w, h). The root's
/// relative position stays (0, 0).
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
    order.sort_by(|a, b| {
        a.name.as_bytes().cmp(b.name.as_bytes()).then(a.id.ordinal.cmp(&b.id.ordinal))
    });
    let sizes: Vec<(f64, f64)> = order.iter().map(|c| size(c, cfg, rel)).collect();
    let area: f64 = sizes.iter().map(|(w, h)| w * h).sum();
    let widest = sizes.iter().map(|&(w, _)| w).fold(0.0, f64::max);
    let target_w = widest.max((area * cfg.aspect).sqrt());
    let (mut x, mut y, mut shelf_h, mut content_w) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (child, &(w, h)) in order.iter().zip(&sizes) {
        if x > 0.0 && x + w > target_w {
            x = 0.0;
            y += shelf_h + cfg.gap;
            shelf_h = 0.0;
        }
        let e = rel.get_mut(&child.id).expect("child sized above");
        e.0 = cfg.gap + x;
        e.1 = cfg.header + cfg.gap + y;
        shelf_h = shelf_h.max(h);
        content_w = content_w.max(x + w);
        x += w + cfg.gap;
    }
    let wh = (content_w + 2.0 * cfg.gap, cfg.header + y + shelf_h + 2.0 * cfg.gap);
    rel.insert(node.id.clone(), (0.0, 0.0, wh.0, wh.1));
    wh
}

fn absolute(
    node: &SymbolNode,
    ox: f64,
    oy: f64,
    rel: &BTreeMap<SymbolId, (f64, f64, f64, f64)>,
    out: &mut BTreeMap<SymbolId, Rect>,
) {
    let &(rx, ry, w, h) = &rel[&node.id];
    let (x, y) = (ox + rx, oy + ry);
    out.insert(node.id.clone(), Rect { x, y, w, h });
    for c in &node.children {
        absolute(c, x, y, rel, out);
    }
}
```

Then modify `crates/outrider-layout/src/lib.rs` to (old exports kept until Task 5):

```rust
pub mod arrange;
pub mod measure;
pub mod pack;
pub mod types;

pub use arrange::layout;
pub use measure::lines_per_cell;
pub use pack::{pack, PackConfig, PackLayout, Rect};
pub use types::{CellRange, NodeLayout, WorldLayout, RATIO};
```

- [ ] **Step 4: Run tests and clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-layout && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all tests PASS (new 5 + old suite), clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider-layout/src/pack.rs crates/outrider-layout/src/lib.rs
git commit -m "feat(layout): bottom-up shelf packer producing absolute 2D rects"
```

### Task 2: Camera goes 2D (`crates/outrider/src/camera.rs`)

**Files:**
- Modify: `crates/outrider/src/camera.rs` (full rewrite below)
- Modify: `crates/outrider/src/world.rs` (add `center_x: 0.0` to Camera literals — 6 spots)
- Modify: `crates/outrider/src/treemap.rs` (2 call sites: pan, zoom_about)

**Interfaces:**
- Consumes: `outrider_layout::Rect` (Task 1).
- Produces: `Camera { center_x, center_y, zoom }`; `world_to_screen(wx, wy, vw, vh) -> (f64, f64)`; `screen_to_world(sx, sy, vw, vh) -> (f64, f64)`; `pan(dx_px, dy_px)`; `zoom_about(sx, sy, vw, vh, factor, min_zoom, max_zoom)`; `Camera::fit(rect, vw, vh) -> Camera`; `frame_rect(rect, vw, vh, fraction, min_zoom, max_zoom) -> Camera`; `frame_page(rect, vw, vh, min_zoom, max_zoom) -> Camera`; `MAX_ZOOM: f64 = 8.0`; `CameraTween` (center_x interpolates linearly too). Legacy `world_to_screen_y`, `Camera::frame`, `frame_band` survive until Task 4.

- [ ] **Step 1: Rewrite camera.rs — implementation**

Replace the entire non-test portion of `crates/outrider/src/camera.rs` with:

```rust
use outrider_layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World point at the viewport center. World units are natural pixels
    /// (zoom 1.0 = code at natural size).
    pub center_x: f64,
    pub center_y: f64,
    /// Pixels per world unit.
    pub zoom: f64,
}

impl Camera {
    pub fn world_to_screen(&self, wx: f64, wy: f64, vw: f64, vh: f64) -> (f64, f64) {
        (
            (wx - self.center_x) * self.zoom + vw / 2.0,
            (wy - self.center_y) * self.zoom + vh / 2.0,
        )
    }

    pub fn screen_to_world(&self, sx: f64, sy: f64, vw: f64, vh: f64) -> (f64, f64) {
        (
            (sx - vw / 2.0) / self.zoom + self.center_x,
            (sy - vh / 2.0) / self.zoom + self.center_y,
        )
    }

    /// Legacy y-only transform for the icicle render walk.
    /// TODO(pivot): deleted in the switchover task.
    pub fn world_to_screen_y(&self, wy: f64, vh: f64) -> f64 {
        (wy - self.center_y) * self.zoom + vh / 2.0
    }

    /// Drag by (dx, dy) pixels: content follows the cursor.
    pub fn pan(&mut self, dx_px: f64, dy_px: f64) {
        self.center_x -= dx_px / self.zoom;
        self.center_y -= dy_px / self.zoom;
    }

    /// Multiply zoom by `factor`, keeping the world point under screen
    /// (sx, sy) fixed.
    #[allow(clippy::too_many_arguments)]
    pub fn zoom_about(
        &mut self,
        sx: f64,
        sy: f64,
        vw: f64,
        vh: f64,
        factor: f64,
        min_zoom: f64,
        max_zoom: f64,
    ) {
        let (wx, wy) = self.screen_to_world(sx, sy, vw, vh);
        self.zoom = (self.zoom * factor).clamp(min_zoom, max_zoom);
        self.center_x = wx - (sx - vw / 2.0) / self.zoom;
        self.center_y = wy - (sy - vh / 2.0) / self.zoom;
    }

    /// Home: `rect` fits the viewport with a 5% margin.
    pub fn fit(rect: Rect, vw: f64, vh: f64) -> Camera {
        Camera {
            center_x: rect.x + rect.w / 2.0,
            center_y: rect.y + rect.h / 2.0,
            zoom: (vw / rect.w).min(vh / rect.h) / 1.05,
        }
    }

    /// Legacy y-only Home framing (icicle).
    /// TODO(pivot): deleted in the switchover task.
    pub fn frame(world_h: f64, vh: f64) -> Camera {
        Camera { center_x: 0.0, center_y: world_h / 2.0, zoom: vh / (world_h * 1.05) }
    }
}

/// Enter/Esc framing for containers: the focus rect lands at half the
/// viewport's tighter dimension.
pub const FOCUS_FRACTION: f64 = 0.5;
/// End-key framing: the focus rect fills the viewport.
pub const END_FRACTION: f64 = 0.95;
/// Camera-follow tween duration, seconds (spec: ~250 ms, interruptible).
pub const TWEEN_SECS: f64 = 0.25;
/// World units are natural pixels; 8× natural size is as far as zoom goes.
pub const MAX_ZOOM: f64 = 8.0;

/// Camera showing `rect` at `fraction` of the viewport's tighter
/// dimension, centered. The zoom clamp may prevent exact framing (accepted).
pub fn frame_rect(
    rect: Rect,
    vw: f64,
    vh: f64,
    fraction: f64,
    min_zoom: f64,
    max_zoom: f64,
) -> Camera {
    Camera {
        center_x: rect.x + rect.w / 2.0,
        center_y: rect.y + rect.h / 2.0,
        zoom: (fraction * (vw / rect.w).min(vh / rect.h)).clamp(min_zoom, max_zoom),
    }
}

/// Leaf framing: END_FRACTION fit, capped at natural size (zoom 1.0) —
/// stepping onto a small method never blows its code up past 12px.
pub fn frame_page(rect: Rect, vw: f64, vh: f64, min_zoom: f64, max_zoom: f64) -> Camera {
    Camera {
        center_x: rect.x + rect.w / 2.0,
        center_y: rect.y + rect.h / 2.0,
        zoom: (END_FRACTION * (vw / rect.w).min(vh / rect.h))
            .min(1.0)
            .clamp(min_zoom, max_zoom),
    }
}

/// Legacy y-band framing (icicle). TODO(pivot): deleted in the switchover task.
pub fn frame_band(y: f64, h: f64, vh: f64, fraction: f64, min_zoom: f64, max_zoom: f64) -> Camera {
    Camera {
        center_x: 0.0,
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
/// seconds. Centers interpolate linearly; zoom geometrically (log-space)
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
            center_x: self.from.center_x + (self.to.center_x - self.from.center_x) * e,
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

- [ ] **Step 2: Rewrite the camera tests**

Replace the whole `#[cfg(test)] mod tests` in `camera.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn screen_world_round_trip_2d() {
        let c = Camera { center_x: 100.0, center_y: 50.0, zoom: 2.0 };
        let (sx, sy) = c.world_to_screen(150.0, 75.0, 800.0, 600.0);
        close(sx, 500.0); // (150-100)·2 + 400
        close(sy, 350.0); // (75-50)·2 + 300
        let (wx, wy) = c.screen_to_world(sx, sy, 800.0, 600.0);
        close(wx, 150.0);
        close(wy, 75.0);
    }

    #[test]
    fn pan_moves_center_against_drag() {
        let mut c = Camera { center_x: 100.0, center_y: 50.0, zoom: 2.0 };
        c.pan(-4.0, 6.0);
        close(c.center_x, 102.0); // 100 - (-4)/2
        close(c.center_y, 47.0); // 50 - 6/2
    }

    #[test]
    fn zoom_about_fixes_cursor_point() {
        for &(sx, sy) in &[(0.0, 0.0), (400.0, 300.0), (799.0, 1.0), (123.0, 456.0)] {
            for &f in &[0.5, 0.9, 1.1, 2.0, 7.3] {
                let mut c = Camera { center_x: 40.0, center_y: 700.0, zoom: 0.7 };
                let before = c.screen_to_world(sx, sy, 800.0, 600.0);
                c.zoom_about(sx, sy, 800.0, 600.0, f, 1e-9, 1e18);
                let after = c.screen_to_world(sx, sy, 800.0, 600.0);
                close(before.0, after.0);
                close(before.1, after.1);
            }
        }
    }

    #[test]
    fn zoom_about_clamps() {
        let mut c = Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 };
        c.zoom_about(400.0, 300.0, 800.0, 600.0, 1e9, 0.5, 4.0);
        close(c.zoom, 4.0);
        c.zoom_about(400.0, 300.0, 800.0, 600.0, 1e-9, 0.5, 4.0);
        close(c.zoom, 0.5);
    }

    #[test]
    fn fit_centers_with_margin() {
        // the Task 1 worked-example root: 1000 × 1639.2 in 800 × 600 —
        // height is the tight side
        let r = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 1639.2 };
        let c = Camera::fit(r, 800.0, 600.0);
        close(c.center_x, 500.0);
        close(c.center_y, 819.6);
        close(c.zoom, 600.0 / 1639.2 / 1.05);
        // framed rect fits the viewport in both axes
        assert!(c.zoom * r.w <= 800.0 + 1e-9);
        assert!(c.zoom * r.h <= 600.0 + 1e-9);
    }

    #[test]
    fn frame_rect_uses_tighter_dimension() {
        // g's page from the Task 1 worked example: width is the tight side
        let r = Rect { x: 504.0, y: 264.0, w: 480.0, h: 58.0 };
        let c = frame_rect(r, 800.0, 600.0, FOCUS_FRACTION, 1e-9, 1e18);
        close(c.center_x, 744.0);
        close(c.center_y, 293.0);
        close(c.zoom, 0.5 * 800.0 / 480.0);
        // clamp may prevent exact framing
        let c = frame_rect(r, 800.0, 600.0, FOCUS_FRACTION, 1e-9, 0.3);
        close(c.zoom, 0.3);
    }

    #[test]
    fn frame_page_caps_at_natural_size() {
        // small page: END framing would be 0.95·800/480 ≈ 1.58 → capped at 1.0
        let small = Rect { x: 504.0, y: 264.0, w: 480.0, h: 58.0 };
        let c = frame_page(small, 800.0, 600.0, 1e-9, 1e18);
        close(c.zoom, 1.0);
        close(c.center_x, 744.0);
        close(c.center_y, 293.0);
        // tall page: fit dominates — 0.95·600/1602.4, below the 1.0 cap
        let tall = Rect { x: 8.0, y: 28.8, w: 480.0, h: 1602.4 };
        let c = frame_page(tall, 800.0, 600.0, 1e-9, 1e18);
        close(c.zoom, 0.95 * 600.0 / 1602.4);
        // clamp still applies
        let c = frame_page(small, 800.0, 600.0, 2.0, 4.0);
        close(c.zoom, 2.0);
    }

    #[test]
    fn tween_endpoints_exact_and_done() {
        let from = Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 };
        let to = Camera { center_x: 300.0, center_y: 900.0, zoom: 64.0 };
        let tw = CameraTween::new(from, to);
        close(tw.duration, TWEEN_SECS);
        assert_eq!(tw.sample(0.0), from);
        assert_eq!(tw.sample(TWEEN_SECS), to); // exact, not approximate
        assert_eq!(tw.sample(TWEEN_SECS * 2.0), to);
        assert!(!tw.done(TWEEN_SECS - 1e-6));
        assert!(tw.done(TWEEN_SECS));
    }

    #[test]
    fn tween_midpoint_linear_centers_geometric_zoom() {
        let from = Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 };
        let to = Camera { center_x: 300.0, center_y: 900.0, zoom: 64.0 };
        let tw = CameraTween::new(from, to);
        let mid = tw.sample(TWEEN_SECS / 2.0); // ease(½) = ½
        close(mid.center_x, 150.0);
        close(mid.center_y, 450.0);
        close(mid.zoom, 8.0); // √(1·64)
    }

    #[test]
    fn retarget_is_continuous() {
        let tw = CameraTween::new(
            Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 },
            Camera { center_x: 300.0, center_y: 900.0, zoom: 64.0 },
        );
        let other = Camera { center_x: -3.0, center_y: 7.0, zoom: 0.5 };
        let t = 0.1;
        let re = tw.retarget(t, other);
        assert_eq!(re.sample(0.0), tw.sample(t)); // no jump at the splice
        assert_eq!(re.to, other);
    }

    #[test]
    fn legacy_y_frames_until_switchover() {
        // TODO(pivot): delete with Camera::frame / frame_band in the switchover
        let c = Camera::frame(1.0, 600.0);
        close(c.center_y, 0.5);
        close(c.zoom, 600.0 / 1.05);
        let c = frame_band(0.6875, 0.015625, 600.0, FOCUS_FRACTION, 1e-9, 1e18);
        close(c.zoom, 19200.0);
        close(c.center_y, 0.6953125);
    }
}
```

- [ ] **Step 3: Patch the compile breaks in world.rs and treemap.rs**

`world.rs` — the `Camera` struct gained a field; add `center_x: 0.0,` to every literal:

1. In `frame_leaf` (currently `Some(Camera { center_y: y + h / 2.0, zoom: z.clamp(min_zoom, max_zoom) })`) → `Some(Camera { center_x: 0.0, center_y: y + h / 2.0, zoom: z.clamp(min_zoom, max_zoom) })`.
2. In tests, every `Camera { center_y: …, zoom: … }` literal (five: `culling_x_prune_stops_recursion`, `zoomed_past_ancestors_clip_and_compress`, `hit_test_picks_the_column_under_the_point`, `nested_rects_extend_over_descendants` ×2) gains `center_x: 0.0,` as the first field.

`treemap.rs` — two handler bodies change:

The `on_mouse_move` listener body becomes (dx now feeds pan; center_x is ignored by the old render so behavior is unchanged):

```rust
.on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _w, cx| {
    if e.pressed_button != Some(gpui::MouseButton::Left) {
        return;
    }
    let Some(last) = this.drag_last else { return };
    this.cancel_tween();
    let dx = f64::from(e.position.x - last.x);
    let dy = f64::from(e.position.y - last.y);
    if let Some(cam) = this.camera.as_mut() {
        cam.pan(dx, dy);
    }
    this.drag_last = Some(e.position);
    cx.notify();
}))
```

The `on_scroll_wheel` listener body becomes:

```rust
.on_scroll_wheel(cx.listener(move |this, e: &gpui::ScrollWheelEvent, w, cx| {
    this.cancel_tween();
    let dy = match e.delta {
        gpui::ScrollDelta::Pixels(p) => f64::from(p.y),
        gpui::ScrollDelta::Lines(l) => l.y as f64 * 40.0,
    };
    let vp = w.viewport_size();
    let (vw, vh) = (f64::from(vp.width), f64::from(vp.height));
    if let Some(cam) = this.camera.as_mut() {
        // scroll up (positive dy) zooms in; flip the sign here if
        // manual testing shows it inverted on this platform
        let factor = (dy * 0.002).exp();
        cam.zoom_about(
            f64::from(e.position.x),
            f64::from(e.position.y),
            vw,
            vh,
            factor,
            min_zoom,
            max_zoom,
        );
    }
    cx.notify();
}))
```

- [ ] **Step 4: Run tests and clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all PASS (camera suite rewritten, world/treemap suites untouched in behavior), clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/camera.rs crates/outrider/src/world.rs crates/outrider/src/treemap.rs
git commit -m "feat(app): 2D camera — center_x, rect framing, fit; legacy y-API kept for switchover"
```

### Task 3: Packed-rect cull walk (`crates/outrider/src/world.rs`)

**Files:**
- Modify: `crates/outrider/src/world.rs` (add `visible_packed` + `left` field; old walk untouched)

**Interfaces:**
- Consumes: `outrider_layout::{PackLayout, Rect, pack, PackConfig}` (Task 1); `Camera::world_to_screen` (Task 2); existing `rung_for`, `content::{is_leaf_item, natural_px}`, `PxRect`, `DrawItem`, `hit_test`.
- Produces: `pub fn visible_packed<'a>(tree: &'a SymbolTree, pack: &PackLayout, camera: &Camera, vw: f64, vh: f64) -> Vec<DrawItem<'a>>` (pre-order / painter's order); `DrawItem` gains `pub left: f64` (unclipped screen x, sibling of the existing `top`).

Note: the bin crate denies dead code — `visible_packed`, its helper `walk_packed`, and the `left` field each get `#[allow(dead_code)] // TODO(pivot): consumed in the switchover task` until Task 4 wires them.

- [ ] **Step 1: Add the `left` field**

In `DrawItem`, directly above `/// UNclipped pixel height`, add:

```rust
    /// UNclipped screen-x of the box left (`px.x` is clipped to the viewport).
    #[allow(dead_code)] // TODO(pivot): consumed in the switchover task
    pub left: f64,
```

and in the old `walk` fn's `out.push(DrawItem { … })`, add `left: px_x,` next to `top: px_y,` (the old columns never clip x, so left == px.x there).

- [ ] **Step 2: Write the failing tests**

Append to `world.rs`'s test module. The scene numbers are derived by hand from the Task 1 worked-example rects (root (0,0,1000,1639.2); a.rs (8,28.8,480,1602.4); b.rs (496,28.8,496,301.2); f (504,57.6,480,198.4); g (504,264,480,58)) under `screen = (world − center)·zoom + viewport/2`:

```rust
    fn pack_cfg() -> outrider_layout::PackConfig {
        outrider_layout::PackConfig {
            page_w: 480.0,
            line_step: 15.6,
            header: 20.8,
            bottom_pad: 6.0,
            gap: 8.0,
            aspect: 1.6,
        }
    }

    /// Worked example with measures matching the Task 1 pack fixtures and
    /// byte ranges making f and g leaf items.
    fn packed_example() -> (SymbolTree, outrider_layout::PackLayout) {
        let mut t = worked_example();
        t.root.children[0].measure = 100; // a.rs
        t.root.children[1].measure = 40; // b.rs
        t.root.children[1].children[0].measure = 10; // f
        t.root.children[1].children[1].measure = 1; // g
        t.root.children[1].children[0].byte_range = Some(0..10);
        t.root.children[1].children[1].byte_range = Some(10..20);
        let p = outrider_layout::pack(&t, &pack_cfg());
        (t, p)
    }

    #[test]
    fn packed_walk_zoom_one_clips_and_keeps_unclipped_fields() {
        let (tree, p) = packed_example();
        // zoom 1.0 centered on g's page center (744, 293)
        let cam = Camera { center_x: 744.0, center_y: 293.0, zoom: 1.0 };
        let items = visible_packed(&tree, &p, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["", "a.rs", "b.rs", "f", "g"]);
        let rungs: Vec<Rung> = items.iter().map(|i| i.rung).collect();
        // root 1639px → Full; a.rs 1602px file → Full; b.rs 301px → Detail;
        // f (leaf, 198.4 ≥ 54.1) → Full; g (leaf, 58 ≥ min(58, 54.1)) → Full
        assert_eq!(rungs, vec![Rung::Full, Rung::Full, Rung::Detail, Rung::Full, Rung::Full]);
        assert_eq!(
            items.iter().map(|i| i.level).collect::<Vec<_>>(),
            vec![0, 1, 1, 2, 2]
        );
        // a.rs hangs off the left edge: clipped x/w, unclipped left/full_h
        let a = &items[1];
        close(a.px.x, -2.0);
        close(a.left, -336.0); // 8 − 744 + 400
        close(a.px.w, 146.0); // right edge 144, clipped left −2
        close(a.px.y, 35.8);
        close(a.top, 35.8); // on-screen top: clipped == unclipped
        close(a.px.h, 566.2); // bottom clipped to 602
        close(a.full_h, 1602.4);
        assert!((a.label_w - 480.0).abs() < 1e-9); // truncation uses the box width
        // root's top is above the viewport: top unclipped, px.y clipped
        close(items[0].top, 7.0); // 0 − 293 + 300
        close(items[0].left, -344.0);
        close(items[0].px.x, -2.0);
        // g fully on-screen: nothing clipped
        let g = &items[4];
        close(g.px.x, 160.0);
        close(g.px.y, 271.0);
        close(g.px.w, 480.0);
        close(g.px.h, 58.0);
        close(g.full_h, 58.0);
        // hit-test picks the deepest node under the point
        assert_eq!(hit_test(&items, 400.0, 290.0).unwrap().node.name, "g");
        assert_eq!(hit_test(&items, 400.0, 100.0).unwrap().node.name, "f");
    }

    #[test]
    fn packed_walk_merges_tiny_nodes() {
        let (tree, p) = packed_example();
        // zoomed far out: g is 58·0.03 = 1.74px < MERGE_PX and vanishes;
        // everything else survives as Dot (all widths < LABEL_MIN_W)
        let cam = Camera { center_x: 500.0, center_y: 819.6, zoom: 0.03 };
        let items = visible_packed(&tree, &p, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["", "a.rs", "b.rs", "f"]);
        assert!(items.iter().all(|i| i.rung == Rung::Dot));
    }

    #[test]
    fn packed_walk_prunes_offscreen_subtrees() {
        let (tree, p) = packed_example();
        let cam = Camera { center_x: 100_000.0, center_y: 100_000.0, zoom: 1.0 };
        assert!(visible_packed(&tree, &p, &cam, 800.0, 600.0).is_empty());
        // panned right so only b.rs's column of the map remains: a.rs's
        // right edge (488) is left of the viewport's world-left edge
        // (900 − 400 = 500) → a.rs pruned, b.rs subtree survives
        let cam = Camera { center_x: 900.0, center_y: 293.0, zoom: 1.0 };
        let items = visible_packed(&tree, &p, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["", "b.rs", "f", "g"]);
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider packed_walk`
Expected: FAIL to compile — `visible_packed` not found.

- [ ] **Step 4: Implement the packed walk**

Add to `world.rs` (below the existing `visible_nodes`/`walk`):

```rust
/// Cull the tree against the viewport using packed absolute rects.
/// Returns visible nodes in pre-order (parents before children =
/// painter's order). Children are strictly inside their parents, so an
/// off-screen or sub-merge node prunes its whole subtree.
#[allow(dead_code)] // TODO(pivot): consumed in the switchover task
pub fn visible_packed<'a>(
    tree: &'a SymbolTree,
    pack: &outrider_layout::PackLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
) -> Vec<DrawItem<'a>> {
    let mut out = Vec::new();
    walk_packed(&tree.root, pack, camera, vw, vh, 0, &mut out);
    out
}

#[allow(dead_code)] // TODO(pivot): consumed in the switchover task
fn walk_packed<'a>(
    node: &'a SymbolNode,
    pack: &outrider_layout::PackLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
    level: u8,
    out: &mut Vec<DrawItem<'a>>,
) {
    let Some(r) = pack.rects.get(&node.id) else { return };
    let (sx, sy) = camera.world_to_screen(r.x, r.y, vw, vh);
    let (pw, ph) = (r.w * camera.zoom, r.h * camera.zoom);
    // Children sit strictly inside the parent: off-screen prunes the subtree.
    if sx > vw || sx + pw < 0.0 || sy > vh || sy + ph < 0.0 {
        return;
    }
    let natural = content::is_leaf_item(node).then(|| content::natural_px(node));
    // Below MERGE_PX the node — and its strictly smaller children — merge away.
    let Some(rung) = rung_for(ph, pw, natural) else { return };
    // Clip to the viewport (±2px slack keeps borders off-screen) before f32
    // ever sees the coordinates; rung and code scale use the UNclipped size.
    let x0 = sx.max(-2.0);
    let x1 = (sx + pw).min(vw + 2.0);
    let y0 = sy.max(-2.0);
    let y1 = (sy + ph).min(vh + 2.0);
    out.push(DrawItem {
        node,
        px: PxRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 },
        label_w: pw,
        level,
        rung,
        top: sy,
        left: sx,
        full_h: ph,
    });
    for child in &node.children {
        walk_packed(child, pack, camera, vw, vh, level.saturating_add(1), out);
    }
}
```

- [ ] **Step 5: Run tests and clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all PASS (old world suite still green — old walk merely gained `left: px_x`), clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/world.rs
git commit -m "feat(app): packed-rect cull walk with 2D clipping alongside the icicle walk"
```

### Task 4: Switchover — app runs on the packed world; icicle code deleted from the bin crate

**Files:**
- Modify: `crates/outrider/src/main.rs` (pack instead of layout)
- Modify: `crates/outrider/src/treemap.rs` (PackLayout, 2D home/framing, Enter/Esc keys, body_x)
- Modify: `crates/outrider/src/world.rs` (delete column machinery + old walk; rename `visible_packed` → `visible_nodes`; add `pack_config`)
- Modify: `crates/outrider/src/camera.rs` (delete legacy y-API)
- Modify: `crates/outrider/src/focus.rs` (delete `step_sibling`)

**Interfaces:**
- Consumes: everything produced by Tasks 1–3.
- Produces: `world::pack_config() -> outrider_layout::PackConfig` and consts `world::{PAGE_W = 480.0, PACK_GAP = 8.0, PACK_ASPECT = 1.6}`; `world::visible_nodes(tree, &PackLayout, camera, vw, vh)` (the renamed packed walk — Task 6 relies on this name); `TreemapView::new(tree: SymbolTree, layout: PackLayout, cx)`. Arrow keys are intentionally unhandled until Task 6; Enter descends, Escape ascends.
- After this task ZERO `#[allow(dead_code)] // TODO(pivot)` markers remain.

- [ ] **Step 1: Rewire `treemap.rs` and `main.rs`**

`main.rs`: replace `let layout = outrider_layout::layout(&tree);` and the count line with:

```rust
    let layout = outrider_layout::pack(&tree, &world::pack_config());
    eprintln!("{} symbols packed", layout.rects.len());
```

(`world` is already a declared module; no new imports needed.)

`world.rs`: add near the top (below the rung constants):

```rust
/// Leaf page width in world units (= natural pixels).
pub const PAGE_W: f64 = 480.0;
/// World-px gap between siblings and container inner margin.
pub const PACK_GAP: f64 = 8.0;
/// Target container width/height ratio.
pub const PACK_ASPECT: f64 = 1.6;

/// The app's packing configuration: leaf pages sized by the content
/// module's row metrics, so a page at zoom 1.0 is exactly natural size.
pub fn pack_config() -> outrider_layout::PackConfig {
    outrider_layout::PackConfig {
        page_w: PAGE_W,
        line_step: content::LINE_STEP,
        header: content::HEADER,
        bottom_pad: content::BOTTOM_PAD,
        gap: PACK_GAP,
        aspect: PACK_ASPECT,
    }
}
```

(`content.rs` already exposes `LINE_STEP`, `HEADER`, `BOTTOM_PAD` as `pub` — do not modify it.)

`treemap.rs` imports: replace `use outrider_layout::WorldLayout;` with `use outrider_layout::{PackLayout, Rect};`. Struct field: `layout: WorldLayout` → `layout: PackLayout`; `TreemapView::new(tree: SymbolTree, layout: PackLayout, cx: &mut Context<Self>)` (body unchanged).

`PaintItem` gains, next to `body_font_px`:

```rust
    /// UNclipped screen-x of the box: body/code rows move with the box,
    /// while the name row pins to the clipped corner.
    body_x: f32,
```

Replace `root_world_height` and `home_camera` with:

```rust
    fn root_rect(&self) -> Rect {
        self.layout
            .rects
            .get(&self.tree.root.id)
            .copied()
            .unwrap_or(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 })
    }

    /// Framing target for the current focus: leaf pages at natural size
    /// (capped END fit), containers at FOCUS_FRACTION.
    fn frame_focus(
        &self,
        index: &TreeIndex,
        vw: f64,
        vh: f64,
        min_zoom: f64,
        max_zoom: f64,
    ) -> Option<Camera> {
        let r = *self.layout.rects.get(&self.focus.current)?;
        match index.node(&self.focus.current) {
            Some(n) if content::is_leaf_item(n) => {
                Some(camera::frame_page(r, vw, vh, min_zoom, max_zoom))
            }
            _ => Some(camera::frame_rect(r, vw, vh, camera::FOCUS_FRACTION, min_zoom, max_zoom)),
        }
    }
```

In `paint_items`, the camera-init block becomes (fit needs vw now):

```rust
        if self.camera.is_none() {
            let c = Camera::fit(self.root_rect(), vw, vh);
            self.home_zoom = c.zoom;
            self.camera = Some(c);
        }
```

and the walk call becomes `world::visible_nodes(&self.tree, &self.layout, &camera, vw, vh)` (same name after the Step 3 rename; until then use `world::visible_packed` — Step 3 flips both call sites). The `PaintItem` push gains `body_x: item.left as f32,`.

In `render`, the zoom clamps become:

```rust
        let max_zoom = camera::MAX_ZOOM;
        let min_zoom = self.home_zoom * 0.5;
```

The `on_key_down` listener is replaced wholesale:

```rust
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, w, cx| {
                if this.camera.is_none() {
                    return;
                }
                let vp = w.viewport_size();
                let (vw, vh) = (f64::from(vp.width), f64::from(vp.height));
                let max_zoom = camera::MAX_ZOOM;
                let min_zoom = this.home_zoom * 0.5;
                let index = TreeIndex::new(&this.tree);
                let target = match e.keystroke.key.as_str() {
                    "enter" => {
                        if !this.focus.step_in(&index) {
                            return;
                        }
                        this.frame_focus(&index, vw, vh, min_zoom, max_zoom)
                    }
                    "escape" => {
                        if !this.focus.step_out(&index) {
                            return;
                        }
                        this.frame_focus(&index, vw, vh, min_zoom, max_zoom)
                    }
                    "end" => this.layout.rects.get(&this.focus.current).map(|&r| {
                        camera::frame_rect(r, vw, vh, camera::END_FRACTION, min_zoom, max_zoom)
                    }),
                    "home" => {
                        let c = Camera::fit(this.root_rect(), vw, vh);
                        this.home_zoom = c.zoom;
                        Some(c)
                    }
                    // Arrows land in the spatial-step task; Tab stays disabled.
                    _ => return,
                };
                if let Some(to) = target {
                    this.start_tween(to);
                    cx.notify();
                }
            }))
```

In the canvas paint closure, body rows paint at the unclipped x — replace the body-paint `line.paint(point(origin.x + px(item.x + 6.0), …)` (the one inside `for bt in &item.body`) with:

```rust
                                let _ = line.paint(
                                    point(origin.x + px(item.body_x + 6.0), origin.y + px(bt.y)),
                                    body_line_height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
```

(The name row keeps `item.x + 6.0` — it pins to the clipped corner.)

The click handler (`on_mouse_up`) keeps its exact shape — `world::visible_nodes(&this.tree, &this.layout, &cam, …)` + `hit_test` work unchanged on the packed walk.

- [ ] **Step 2: Verify tests pass mid-refactor**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace`
Expected: PASS (clippy will fail on now-dead icicle code — that is Step 3's job, don't run it yet).

- [ ] **Step 3: Delete the icicle machinery from `world.rs`**

Delete (with their doc comments): `STACK_FRACTION`, `PEAK_CELL_PX`, `WIDTH_RATIO`, `GUTTER_PX`, `MAX_DEPTH`, `NEST_PAD`, `column_scale`, `cell_px_height`, `width_alpha`, `column_weight`, `ColPx`, `column_table`, `tree_max_level`, the old `visible_nodes` + `walk`, `world_band`, `frame_leaf`, and `use outrider_layout::RATIO;` / the `WorldLayout` import. Rename `visible_packed` → `visible_nodes` and `walk_packed` → `walk`; update the doc comment and the test callers; update the two `treemap.rs` call sites from `visible_packed` to `visible_nodes`. Remove all three `#[allow(dead_code)] // TODO(pivot)` markers in `world.rs` (fn ×2 + the `left` field).

Delete these tests (icicle-only): `culling_offscreen_y_is_empty`, `weight_profile_peak_and_falloff`, `column_table_sums_to_target`, `column_table_gutter_floor`, `width_ratios_self_similar`, `worked_example_bands`, `culling_home_view`, `culling_x_prune_stops_recursion`, `zoomed_past_ancestors_clip_and_compress`, `frame_leaf_natural_size_and_width_floor`, `world_band_composes_ancestors`, `hit_test_picks_the_column_under_the_point`, `nested_rects_extend_over_descendants`, and the now-unused `leafy_example` helper. Keep: `rung_for_thresholds_and_downgrade`, the three `packed_walk_*` tests, and the `n`/`worked_example`/`close`/`pack_cfg`/`packed_example` helpers.

- [ ] **Step 4: Delete legacy camera API and `step_sibling`**

`camera.rs`: delete `world_to_screen_y`, `Camera::frame`, `frame_band`, and the `legacy_y_frames_until_switchover` test (all marked `TODO(pivot)`).

`focus.rs`: delete `step_sibling` and the tests `up_down_cycle_and_wrap` and `sibling_at_root_is_noop`. Update `step_in`'s doc comment first line to `/// Enter: last-visited child if still valid, else first child.` and `step_out`'s to `/// Esc: move to the structural parent (no-op at the root).` Rewrite the three tests that used `step_sibling` as setup:

```rust
    #[test]
    fn esc_moves_to_parent_and_root_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.set(id(SymbolKind::Fn, "b.rs::f"), &idx);
        assert!(f.step_out(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "b.rs"));
        assert!(f.step_out(&idx));
        assert_eq!(f.current, t.root.id);
        assert!(!f.step_out(&idx)); // root has no parent
    }

    #[test]
    fn esc_then_enter_returns_to_the_same_child() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.set(id(SymbolKind::Fn, "b.rs::g"), &idx);
        f.step_out(&idx); // b.rs
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::Fn, "b.rs::g")); // last visited, not first
    }

    #[test]
    fn enter_remembers_last_visited_child() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.set(id(SymbolKind::File, "b.rs"), &idx); // last_child[root] = b.rs
        assert!(f.set(t.root.id.clone(), &idx)); // click back to root
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "b.rs")); // not a.rs
    }
```

(These replace `left_moves_to_parent_and_root_is_noop`, `left_then_right_returns_to_the_same_child`, `right_remembers_last_visited_child`. `right_steps_into_first_child_and_leaf_is_noop` survives renamed to `enter_steps_into_first_child_and_leaf_is_noop`, body unchanged.)

- [ ] **Step 5: Full verification**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo build -p outrider`
Expected: all PASS, clippy clean, binary builds.
Also verify no pivot allows remain: `grep -rn "TODO(pivot)" crates/` → no output.
(Manual smoke test for the human, not the subagent: `cargo run -p outrider .` — whole repo visible as nested boxes; drag pans 2D; wheel zooms about the cursor; zooming into a method reaches real code at 12px; Enter/Esc/Home/End tween.)

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src crates/outrider-layout/src
git commit -m "feat(app)!: switch to packed 2D treemap world; delete icicle columns from the bin crate"
```

### Task 5: Delete the cell system from `outrider-layout`

**Files:**
- Delete: `crates/outrider-layout/src/arrange.rs`, `crates/outrider-layout/src/measure.rs`, `crates/outrider-layout/src/types.rs`
- Modify: `crates/outrider-layout/src/lib.rs`

**Interfaces:**
- Consumes: nothing new. After Task 4 nothing in the workspace references `layout`, `lines_per_cell`, `CellRange`, `NodeLayout`, `WorldLayout`, or `RATIO`.
- Produces: the crate root exports exactly `pack, PackConfig, PackLayout, Rect`.

- [ ] **Step 1: Verify nothing references the cell system**

Run: `grep -rn "WorldLayout\|CellRange\|NodeLayout\|lines_per_cell\|outrider_layout::layout\|outrider_layout::RATIO\|absolute_start" crates/ --include="*.rs" | grep -v outrider-layout/src`
Expected: no output. (If anything shows up, Task 4 missed a call site — fix that first, don't delete around it.)

- [ ] **Step 2: Delete the modules**

```bash
git rm crates/outrider-layout/src/arrange.rs crates/outrider-layout/src/measure.rs crates/outrider-layout/src/types.rs
```

Replace the whole `crates/outrider-layout/src/lib.rs` with:

```rust
pub mod pack;

pub use pack::{pack, PackConfig, PackLayout, Rect};
```

- [ ] **Step 3: Run tests and clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all PASS (pack + camera + world + focus + content + treemap + buffers suites), clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/outrider-layout/src/lib.rs
git commit -m "refactor(layout)!: delete the 1D cell system — pack is the only layout"
```

---

### Task 6: Spatial arrow stepping

**Files:**
- Modify: `crates/outrider/src/focus.rs` (add `Dir`, `TreeIndex::depth`, `spatial_step` + tests)
- Modify: `crates/outrider/src/treemap.rs` (wire arrow keys)

**Interfaces:**
- Consumes: `outrider_layout::{PackLayout, Rect}`; `TreeIndex` internals (`nodes`, `parents` maps); `Focus::set` (records last-visited); `TreemapView::frame_focus` (Task 4); `world::visible_nodes` name from Task 4.
- Produces: `pub enum Dir { Left, Right, Up, Down }`; `TreeIndex::depth(&self, id: &SymbolId) -> Option<usize>`; `pub fn spatial_step(current: &SymbolId, dir: Dir, pack: &PackLayout, index: &TreeIndex) -> Option<SymbolId>`.

- [ ] **Step 1: Write the failing tests**

Append to `focus.rs`'s test module (geometry derived by hand from the packer with the standard config and all measures = 1; every leaf page is 480 × 58):

```rust
    use outrider_layout::{PackConfig, PackLayout, Rect};

    fn cfg() -> PackConfig {
        PackConfig {
            page_w: 480.0,
            line_step: 15.6,
            header: 20.8,
            bottom_pad: 6.0,
            gap: 8.0,
            aspect: 1.6,
        }
    }

    /// root { a.rs { x }, b.rs { f, g } } — two files whose fns stack in
    /// one vertical column, so Up/Down cross file boundaries.
    fn two_files() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                vec![
                    n(
                        SymbolKind::File,
                        "a.rs",
                        "a.rs",
                        vec![n(SymbolKind::Fn, "a.rs::x", "x", vec![])],
                    ),
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

    #[test]
    fn depth_counts_ancestors() {
        let t = two_files();
        let idx = TreeIndex::new(&t);
        assert_eq!(idx.depth(&t.root.id), Some(0));
        assert_eq!(idx.depth(&id(SymbolKind::File, "b.rs")), Some(1));
        assert_eq!(idx.depth(&id(SymbolKind::Fn, "b.rs::g")), Some(2));
        assert_eq!(idx.depth(&id(SymbolKind::Fn, "nope")), None);
    }

    #[test]
    fn spatial_step_crosses_parent_boundaries_at_same_depth() {
        let t = two_files();
        let idx = TreeIndex::new(&t);
        let p = outrider_layout::pack(&t, &cfg());
        // packed geometry: x (16, 57.6), f (16, 160.4), g (16, 226.4) —
        // one column of depth-2 pages spanning two files
        let x = id(SymbolKind::Fn, "a.rs::x");
        let f = id(SymbolKind::Fn, "b.rs::f");
        let g = id(SymbolKind::Fn, "b.rs::g");
        assert_eq!(spatial_step(&x, Dir::Down, &p, &idx), Some(f.clone())); // into b.rs
        assert_eq!(spatial_step(&f, Dir::Up, &p, &idx), Some(x.clone())); // back into a.rs
        assert_eq!(spatial_step(&g, Dir::Up, &p, &idx), Some(f.clone())); // nearest, not x
        assert_eq!(spatial_step(&g, Dir::Down, &p, &idx), None); // no wrap
        assert_eq!(spatial_step(&f, Dir::Right, &p, &idx), None); // same x-center → not "right of"
        // depth 1: the two files stack vertically
        let a = id(SymbolKind::File, "a.rs");
        let b = id(SymbolKind::File, "b.rs");
        assert_eq!(spatial_step(&a, Dir::Down, &p, &idx), Some(b.clone()));
        assert_eq!(spatial_step(&b, Dir::Up, &p, &idx), Some(a.clone()));
        // the root has no same-depth peers
        assert_eq!(spatial_step(&t.root.id, Dir::Down, &p, &idx), None);
    }

    fn hand_layout(entries: &[(SymbolId, Rect)]) -> PackLayout {
        PackLayout { rects: entries.iter().cloned().collect() }
    }

    /// root { c, p, q } with hand-placed rects to probe the scoring rule.
    fn scoring_tree() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                vec![
                    n(SymbolKind::Fn, "c", "c", vec![]),
                    n(SymbolKind::Fn, "p", "p", vec![]),
                    n(SymbolKind::Fn, "q", "q", vec![]),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    #[test]
    fn spatial_step_penalizes_orthogonal_offset() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let (c, p, q) =
            (id(SymbolKind::Fn, "c"), id(SymbolKind::Fn, "p"), id(SymbolKind::Fn, "q"));
        // p: straight right, farther (primary 20, ortho 0 → 20);
        // q: nearer in x but 20 off-axis (primary 12, ortho 20 → 52)
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -10.0, y: -30.0, w: 100.0, h: 100.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 12.0, y: 20.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
        // exact tie (both primary 20, ortho 20): lesser SymbolId wins → p
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -10.0, y: -30.0, w: 100.0, h: 100.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 20.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 20.0, y: -20.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
        // a node missing from the layout steps nowhere
        assert_eq!(spatial_step(&c, Dir::Right, &hand_layout(&[]), &idx), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider spatial`
Expected: FAIL to compile — `Dir`, `depth`, `spatial_step` not found.

- [ ] **Step 3: Implement `Dir`, `depth`, `spatial_step`**

In `focus.rs`, add `use outrider_layout::PackLayout;` to the imports. Inside `impl<'a> TreeIndex<'a>` add:

```rust
    /// Number of ancestors above `id`; None if the id is unknown.
    pub fn depth(&self, id: &SymbolId) -> Option<usize> {
        if !self.nodes.contains_key(id) {
            return None;
        }
        let mut d = 0;
        let mut cur = id;
        while let Some(p) = self.parents.get(cur) {
            d += 1;
            cur = p;
        }
        Some(d)
    }
```

Below the `Focus` impl add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Spatial arrow step (spec §6): among all nodes at the same tree depth
/// as `current`, pick the candidate whose center lies strictly in `dir`,
/// scored by primary distance + 2·|orthogonal offset|; SymbolId breaks
/// exact ties. No wrap: no candidate → None.
pub fn spatial_step(
    current: &SymbolId,
    dir: Dir,
    pack: &PackLayout,
    index: &TreeIndex,
) -> Option<SymbolId> {
    let cur = pack.rects.get(current)?;
    let (cx, cy) = (cur.x + cur.w / 2.0, cur.y + cur.h / 2.0);
    let depth = index.depth(current)?;
    let mut best: Option<(f64, &SymbolId)> = None;
    for (id, r) in &pack.rects {
        if id == current || index.depth(id) != Some(depth) {
            continue;
        }
        let (nx, ny) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (primary, ortho) = match dir {
            Dir::Right => (nx - cx, (ny - cy).abs()),
            Dir::Left => (cx - nx, (ny - cy).abs()),
            Dir::Down => (ny - cy, (nx - cx).abs()),
            Dir::Up => (cy - ny, (nx - cx).abs()),
        };
        if primary <= 0.0 {
            continue;
        }
        let score = primary + 2.0 * ortho;
        let better = match best {
            None => true,
            Some((s, b)) => score < s || (score == s && id < b),
        };
        if better {
            best = Some((score, id));
        }
    }
    best.map(|(_, id)| id.clone())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider`
Expected: PASS (clippy would flag `spatial_step`/`Dir`/`depth` as dead in the bin crate — Step 5 wires them; run clippy after).

- [ ] **Step 5: Wire the arrow keys**

`treemap.rs`: change the focus import to `use crate::focus::{self, Focus, TreeIndex};` and add an arm to the Task 4 key match, directly above the `_ => return` arm:

```rust
                    "up" | "down" | "left" | "right" => {
                        let dir = match e.keystroke.key.as_str() {
                            "up" => focus::Dir::Up,
                            "down" => focus::Dir::Down,
                            "left" => focus::Dir::Left,
                            _ => focus::Dir::Right,
                        };
                        let Some(next) =
                            focus::spatial_step(&this.focus.current, dir, &this.layout, &index)
                        else {
                            return;
                        };
                        if !this.focus.set(next, &index) {
                            return;
                        }
                        this.frame_focus(&index, vw, vh, min_zoom, max_zoom)
                    }
```

Update the `// Arrows land in the spatial-step task; Tab stays disabled.` comment on the final arm to `// Tab stays disabled — no handler.`

- [ ] **Step 6: Full verification**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo build -p outrider`
Expected: all PASS, clippy clean.
(Manual, for the human: arrows hop between spatially adjacent boxes at the same depth with camera-follow; Enter/Esc still descend/ascend.)

- [ ] **Step 7: Commit**

```bash
git add crates/outrider/src/focus.rs crates/outrider/src/treemap.rs
git commit -m "feat(app): spatial arrow stepping across same-depth neighbors with camera-follow"
```

---

## Execution notes

- Task order is load-bearing: the workspace stays green after every task because new modules land beside old ones (1–3), the app flips in one task (4), and deletions follow use (4–5).
- Tests + clippy after every task are the controller's verification gate; the counts should only ever grow until Task 4/5 remove icicle suites.
- The manual exit gate (Bet #1 re-run: "one place, whole project") happens after Task 6, run by the human.

