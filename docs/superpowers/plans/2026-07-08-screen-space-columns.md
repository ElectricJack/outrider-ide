# Screen-Space Column Widths Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the world-space x-axis with per-depth pixel column widths (peaked profile), so zooming never grows columns past a cap and passed ancestors compress into gutter strips.

**Architecture:** Column widths become a pure function of `(depth, zoom)` computed per frame in `world.rs`; the camera loses its x-axis entirely (`Camera { center_y, zoom }`, stack left-anchored); the y pipeline (f64 composition, 8× grid) is untouched. Spec: `docs/superpowers/specs/2026-07-08-screen-space-columns-design.md`.

**Tech Stack:** Rust workspace; GPUI only inside `crates/outrider/src/treemap.rs` + `main.rs`.

## Global Constraints

- GPUI is pinned to rev `029bf2f284b4e59f20175d78443e630468f3a3e5` — NEVER change the dependency to fix a compile error.
- Prefix every cargo command with `export PATH="$HOME/.cargo/bin:$PATH" && `.
- `world.rs` and `camera.rs` must import no gpui types (headless-testable).
- Y-axis floating-origin contract: composition and camera subtraction in f64; f32 only sees screen-relative values. `abs = parent_abs · 8 + start` with the existing `debug_assert!(abs < 2f64.powi(53))`.
- Constants, verbatim from the spec: `CELL_ASPECT = 3.0`, `MAX_COLUMN_PX = 400.0`, `GUTTER_PX = 24.0`, `LABEL_MIN_W = 60.0`, `MERGE_PX = 4.0`, `LABEL_PX = 20.0`, `CARD_PX = 80.0`, home margin factor `1.05`, grid `RATIO = 8` (from outrider-layout).
- Width profile, verbatim: `w(h) = CELL_ASPECT·h` if `h ≤ MAX_COLUMN_PX/CELL_ASPECT`, else `max(GUTTER_PX, MAX_COLUMN_PX²/(CELL_ASPECT·h))`, where `h = zoom · 8^-depth`.
- Rung rule: select by pixel height (unclipped) as today, then downgrade to Dot when column width `< LABEL_MIN_W`.
- Zoom clamps unchanged: min = `0.5 × home_zoom`, max = `vh · 8^15`.

---

### Task 1: Width profile, column table, and combined rung selection (world.rs, additive)

Everything in this task is pure math added to `crates/outrider/src/world.rs`. Do NOT remove or modify any existing function in this task — the old geometry (`COLUMN_SHRINK`, `column_width`, `column_x`, `world_width`, `rung_for_px_height`, `node_world_rect`, the walk) stays in place until Task 2 replaces it. The crate must keep compiling with both old and new functions present.

**Files:**
- Modify: `crates/outrider/src/world.rs` (add constants + functions + tests; touch nothing existing)

**Interfaces:**
- Consumes: existing `column_scale(depth: u8) -> f64` (8^-depth) and `Rung` enum in the same file.
- Produces (Task 2 relies on these exact signatures):
  - `pub const MAX_COLUMN_PX: f64 = 400.0;`
  - `pub const GUTTER_PX: f64 = 24.0;`
  - `pub const LABEL_MIN_W: f64 = 60.0;`
  - `pub const MAX_DEPTH: usize = 24;`
  - `pub fn cell_px_height(depth: u8, zoom: f64) -> f64`
  - `pub fn column_px_width(h: f64) -> f64`
  - `pub struct ColPx { pub x: f64, pub w: f64 }` (Debug, Clone, Copy, PartialEq)
  - `pub fn column_table(zoom: f64) -> Vec<ColPx>` (length `MAX_DEPTH + 1`)
  - `pub fn rung_for(px_h: f64, px_w: f64) -> Option<Rung>`

- [ ] **Step 1: Write the failing tests**

Append inside the existing `mod tests` in `crates/outrider/src/world.rs` (it already has a `close(a, b)` helper):

```rust
    #[test]
    fn width_profile_rising_side() {
        // w = 3h up to the peak
        close(column_px_width(10.0), 30.0);
        close(column_px_width(100.0), 300.0);
        let peak_h = MAX_COLUMN_PX / CELL_ASPECT; // ≈ 133.33 px cells
        close(column_px_width(peak_h), MAX_COLUMN_PX);
    }

    #[test]
    fn width_profile_decay_side() {
        // past the peak, w = MAX² / (3h): halves when h doubles
        let peak_h = MAX_COLUMN_PX / CELL_ASPECT;
        close(column_px_width(2.0 * peak_h), MAX_COLUMN_PX / 2.0);
        close(column_px_width(8.0 * peak_h), MAX_COLUMN_PX / 8.0);
        // gutter floor is reached exactly and held forever
        let floor_h = MAX_COLUMN_PX * peak_h / GUTTER_PX;
        close(column_px_width(floor_h), GUTTER_PX);
        close(column_px_width(floor_h * 100.0), GUTTER_PX);
    }

    #[test]
    fn width_profile_self_similar() {
        // spec §3: the table at zoom 8z equals the table at z shifted one depth right
        for &z in &[10.0, 127.0, 1000.0, 54321.0] {
            let t1 = column_table(z);
            let t8 = column_table(8.0 * z);
            for d in 0..MAX_DEPTH {
                close(t8[d + 1].w, t1[d].w);
            }
        }
    }

    #[test]
    fn column_table_prefix_sums_and_bound() {
        for &z in &[1.0, 571.4285714285714, 36571.42857142857, 1e12] {
            let t = column_table(z);
            assert_eq!(t.len(), MAX_DEPTH + 1);
            close(t[0].x, 0.0);
            for d in 1..t.len() {
                close(t[d].x, t[d - 1].x + t[d - 1].w);
                assert!(t[d].x > t[d - 1].x, "x must be strictly increasing");
            }
            // spec §3: total stack width is bounded at any zoom
            let total = t[MAX_DEPTH].x + t[MAX_DEPTH].w;
            assert!(total < 1600.0, "total {total} not bounded at zoom {z}");
        }
    }

    #[test]
    fn rung_for_thresholds_and_downgrade() {
        // height thresholds (wide column: no downgrade)
        assert_eq!(rung_for(3.9, 400.0), None);
        assert_eq!(rung_for(4.0, 400.0), Some(Rung::Dot));
        assert_eq!(rung_for(19.9, 400.0), Some(Rung::Dot));
        assert_eq!(rung_for(20.0, 400.0), Some(Rung::Label));
        assert_eq!(rung_for(79.9, 400.0), Some(Rung::Label));
        assert_eq!(rung_for(80.0, 400.0), Some(Rung::Card));
        // narrow columns are forced to Dot regardless of height (gutters)
        assert_eq!(rung_for(100_000.0, 59.9), Some(Rung::Dot));
        assert_eq!(rung_for(100_000.0, 60.0), Some(Rung::Card));
        // the merge rule wins over everything
        assert_eq!(rung_for(3.9, 24.0), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider 2>&1 | tail -20`
Expected: compile error — `column_px_width`, `column_table`, `rung_for`, `MAX_COLUMN_PX` etc. not found.

- [ ] **Step 3: Write the implementation**

Add to `crates/outrider/src/world.rs`, after the existing constants block (keep `COLUMN_SHRINK` and everything else in place):

```rust
pub const MAX_COLUMN_PX: f64 = 400.0;
pub const GUTTER_PX: f64 = 24.0;
/// Columns narrower than this render fill + border only (forced Dot).
pub const LABEL_MIN_W: f64 = 60.0;
/// Depths beyond this are sub-merge at any legal zoom (max zoom = vh·8^15).
pub const MAX_DEPTH: usize = 24;

/// Pixel height of one level-`depth` cell at `zoom` (px per world unit).
pub fn cell_px_height(depth: u8, zoom: f64) -> f64 {
    zoom * column_scale(depth)
}

/// Peaked width profile (screen-space-columns spec §3): rises as
/// CELL_ASPECT·h until the peak (h = MAX/CELL_ASPECT, cells comfortably in
/// Card rung), then decays as 1/h — 8× per zoom octave on both sides, so the
/// profile is self-similar — floored at the gutter for zoomed-past ancestors.
pub fn column_px_width(h: f64) -> f64 {
    let peak_h = MAX_COLUMN_PX / CELL_ASPECT;
    if h <= peak_h {
        CELL_ASPECT * h
    } else {
        (MAX_COLUMN_PX * peak_h / h).max(GUTTER_PX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColPx {
    pub x: f64,
    pub w: f64,
}

/// Per-frame column table: x is the prefix sum of shallower widths — the
/// stack is left-anchored at x = 0 and fully determined by zoom.
pub fn column_table(zoom: f64) -> Vec<ColPx> {
    let mut out = Vec::with_capacity(MAX_DEPTH + 1);
    let mut x = 0.0;
    for d in 0..=MAX_DEPTH {
        let w = column_px_width(cell_px_height(d as u8, zoom));
        out.push(ColPx { x, w });
        x += w;
    }
    out
}

/// Rung by pixel height, downgraded to Dot when the column is too narrow
/// for text (gutter strips). Heights below MERGE_PX merge into the parent.
pub fn rung_for(px_h: f64, px_w: f64) -> Option<Rung> {
    let by_height = if px_h < MERGE_PX {
        return None;
    } else if px_h < LABEL_PX {
        Rung::Dot
    } else if px_h < CARD_PX {
        Rung::Label
    } else {
        Rung::Card
    };
    Some(if px_w < LABEL_MIN_W { Rung::Dot } else { by_height })
}
```

Note: `Rung` is declared later in the file than the constants — Rust doesn't care about item order, so add the code where it reads best (right after the existing geometry helpers is fine).

- [ ] **Step 4: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider 2>&1 | tail -10`
Expected: all tests pass (5 new + all pre-existing; the old geometry tests still pass because nothing existing changed).

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy -p outrider -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/world.rs
git commit -m "feat: peaked screen-space column width profile"
```

---

### Task 2: Y-only camera, screen-space culling walk, treemap integration

Replaces the world-space x-axis everywhere. Three files change together because the `Camera` struct change ripples; the plan below gives the complete target code for `camera.rs`, the new walk, and the exact `treemap.rs` edits.

**Files:**
- Rewrite: `crates/outrider/src/camera.rs` (complete replacement below)
- Modify: `crates/outrider/src/world.rs` (replace walk + delete old x-axis geometry; replace tests)
- Modify: `crates/outrider/src/treemap.rs` (mechanical API updates)
- No changes: `crates/outrider/src/main.rs`, `theme.rs`, other crates

**Interfaces:**
- Consumes (from Task 1, exact signatures): `column_table(zoom: f64) -> Vec<ColPx>`, `ColPx { x, w }`, `rung_for(px_h: f64, px_w: f64) -> Option<Rung>`, `column_scale(depth: u8) -> f64`, `LABEL_MIN_W`, `MAX_DEPTH`.
- Produces:
  - `Camera { pub center_y: f64, pub zoom: f64 }` with `world_to_screen_y(&self, wy: f64, vh: f64) -> f64`, `screen_to_world_y(&self, sy: f64, vh: f64) -> f64`, `pan(&mut self, dy_px: f64)`, `zoom_about(&mut self, sy: f64, vh: f64, factor: f64, min_zoom: f64, max_zoom: f64)`, `frame(world_h: f64, vh: f64) -> Camera`
  - `world::visible_nodes(tree, layout, camera, vw, vh) -> Vec<DrawItem>` — signature unchanged, DrawItem/PxRect unchanged; px rects are now viewport-clipped in y.
- Deleted (nothing may reference them afterward): `Camera::world_to_screen`, `Camera::screen_to_world` (2-axis versions), `Camera.center_x`, `world::COLUMN_SHRINK`, `world::column_width`, `world::column_x`, `world::world_width`, `world::node_world_rect`, `world::WorldRect`, `world::rung_for_px_height`.

- [ ] **Step 1: Replace camera.rs entirely**

Write `crates/outrider/src/camera.rs` with exactly:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World y at the viewport's vertical center. X is not a camera concern:
    /// the column stack is left-anchored and fully determined by zoom.
    pub center_y: f64,
    /// Pixels per world unit.
    pub zoom: f64,
}

impl Camera {
    pub fn world_to_screen_y(&self, wy: f64, vh: f64) -> f64 {
        (wy - self.center_y) * self.zoom + vh / 2.0
    }

    pub fn screen_to_world_y(&self, sy: f64, vh: f64) -> f64 {
        (sy - vh / 2.0) / self.zoom + self.center_y
    }

    /// Drag by dy pixels: content follows the cursor. Horizontal drag is ignored.
    pub fn pan(&mut self, dy_px: f64) {
        self.center_y -= dy_px / self.zoom;
    }

    /// Multiply zoom by `factor`, keeping the world y under screen `sy` fixed.
    pub fn zoom_about(&mut self, sy: f64, vh: f64, factor: f64, min_zoom: f64, max_zoom: f64) {
        let wy = self.screen_to_world_y(sy, vh);
        self.zoom = (self.zoom * factor).clamp(min_zoom, max_zoom);
        self.center_y = wy - (sy - vh / 2.0) / self.zoom;
    }

    /// Frame a world height with a 5% margin (Home).
    pub fn frame(world_h: f64, vh: f64) -> Camera {
        Camera { center_y: world_h / 2.0, zoom: vh / (world_h * 1.05) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn screen_world_round_trip_y() {
        let c = Camera { center_y: 0.5, zoom: 200.0 };
        let sy = c.world_to_screen_y(0.75, 600.0);
        close(sy, 350.0); // (0.75-0.5)*200 + 300
        close(c.screen_to_world_y(sy, 600.0), 0.75);
    }

    #[test]
    fn pan_moves_center_against_drag() {
        let mut c = Camera { center_y: 1.0, zoom: 2.0 };
        c.pan(-4.0); // dragging content up moves center down
        close(c.center_y, 3.0); // 1.0 - (-4.0)/2.0
    }

    #[test]
    fn zoom_about_fixes_cursor_y() {
        for &sy in &[0.0, 300.0, 599.0, 456.0] {
            for &f in &[0.5, 0.9, 1.1, 2.0, 7.3] {
                let mut c = Camera { center_y: 0.4, zoom: 222.0 };
                let before = c.screen_to_world_y(sy, 600.0);
                c.zoom_about(sy, 600.0, f, 1e-9, 1e18);
                close(before, c.screen_to_world_y(sy, 600.0));
            }
        }
    }

    #[test]
    fn zoom_about_clamps() {
        let mut c = Camera { center_y: 0.0, zoom: 100.0 };
        c.zoom_about(300.0, 600.0, 1e9, 50.0, 400.0);
        close(c.zoom, 400.0);
        c.zoom_about(300.0, 600.0, 1e-9, 50.0, 400.0);
        close(c.zoom, 50.0);
    }

    #[test]
    fn frame_fits_height_with_margin() {
        let c = Camera::frame(1.0, 600.0);
        close(c.center_y, 0.5);
        close(c.zoom, 600.0 / 1.05);
        assert!(c.zoom * 1.0 <= 600.0 + 1e-9); // framed band fits the viewport
    }
}
```

- [ ] **Step 2: Rework the walk in world.rs**

In `crates/outrider/src/world.rs`:

Delete these items (and ONLY these): `COLUMN_SHRINK` const, `column_width`, `column_x`, `world_width`, `WorldRect` struct, `node_world_rect`, `rung_for_px_height`. Keep `CELL_ASPECT`, `column_scale`, everything from Task 1, `Rung`, `PxRect`, `DrawItem`.

Replace `visible_nodes` and `walk` with:

```rust
/// Cull the tree against the viewport and the 4px merge rule.
/// Returns visible nodes in pre-order (parents before children = painter's order).
pub fn visible_nodes<'a>(
    tree: &'a SymbolTree,
    layout: &WorldLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
) -> Vec<DrawItem<'a>> {
    let cols = column_table(camera.zoom);
    let mut out = Vec::new();
    walk(&tree.root, layout, camera, &cols, vw, vh, 0.0, &mut out);
    out
}

#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    node: &'a SymbolNode,
    layout: &WorldLayout,
    camera: &Camera,
    cols: &[ColPx],
    vw: f64,
    vh: f64,
    parent_abs: f64,
    out: &mut Vec<DrawItem<'a>>,
) {
    let Some(nl) = layout.nodes.get(&node.id) else { return };
    let depth = nl.cells.level;
    let abs = parent_abs * outrider_layout::RATIO as f64 + nl.cells.start as f64;
    debug_assert!(abs < 2f64.powi(53), "cell address exceeds exact f64 range");
    let s = column_scale(depth);
    let px_y = camera.world_to_screen_y(abs * s, vh);
    let px_h = nl.cells.len as f64 * s * camera.zoom;
    let Some(&ColPx { x: px_x, w: px_w }) = cols.get(depth as usize) else { return };

    // Below the merge threshold: this node merges into its parent's tile,
    // and children (8x smaller) are below it too. Stop.
    let Some(rung) = rung_for(px_h, px_w) else { return };
    // Children's y-ranges are contained in the parent's: off-screen y prunes the subtree.
    if px_y > vh || px_y + px_h < 0.0 {
        return;
    }
    // Deeper columns are further right: past the right edge prunes the subtree.
    if px_x > vw {
        return;
    }
    // Zoomed-past ancestors have enormous pixel heights; clip to the viewport
    // (2px slack keeps their borders off-screen) before f32 ever sees them.
    // The rung above is chosen from the UNclipped height.
    let y0 = px_y.max(-2.0);
    let y1 = (px_y + px_h).min(vh + 2.0);
    out.push(DrawItem { node, px: PxRect { x: px_x, y: y0, w: px_w, h: y1 - y0 }, rung });
    for child in &node.children {
        walk(child, layout, camera, cols, vw, vh, abs, out);
    }
}
```

(The old "off-screen-left skip-draw-but-recurse" case is gone: nothing is ever left of x = 0.)

- [ ] **Step 3: Replace the affected world.rs tests**

In `world.rs`'s `mod tests`, keep the Task 1 tests and the `n(...)`/`worked_example()` helpers unchanged. Delete `column_geometry`, `worked_example_rects`, `rung_thresholds`, `culling_home_view_prunes_submerge_nodes`, `culling_recurses_past_offscreen_left_parent`. Keep `culling_offscreen_y_is_empty` but fix its camera construction (shown below). Add:

```rust
    #[test]
    fn worked_example_bands() {
        // y-composition unchanged from the world-space model:
        // b.rs::g — depth 2, abs cell 44, len 1 → y = 44/64, h = 1/64
        let s = column_scale(2);
        close(44.0 * s, 0.6875);
        close(1.0 * s, 0.015625);
    }

    #[test]
    fn culling_home_view() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // Home: root band (world height 1.0) fits 600px with 5% margin → zoom = 4000/7
        let cam = Camera::frame(1.0, 600.0);
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        // home zoom is now height-only (571.4) — even g (8.9px) is above merge
        assert_eq!(names, vec!["", "a.rs", "b.rs", "f", "g"]);
        let rungs: Vec<Rung> = items.iter().map(|i| i.rung).collect();
        // heights: root 571.4, a.rs 285.7, b.rs 71.4, f 26.8, g 8.9
        // widths:  d0 93.33 (decay side), d1 214.29, d2 26.79 (< LABEL_MIN_W → Dot)
        assert_eq!(rungs, vec![Rung::Card, Rung::Card, Rung::Label, Rung::Dot, Rung::Dot]);
        // hand-computed px rect for f (zoom = 4000/7):
        // x = w0+w1 = 280/3 + 1500/7, y = 0.125·zoom + 300, w = 3·zoom/64, h = 3·zoom/64
        let f = &items[3].px;
        assert!((f.x - 307.6190476).abs() < 1e-6, "{}", f.x);
        assert!((f.y - 371.4285714).abs() < 1e-6, "{}", f.y);
        assert!((f.w - 26.7857143).abs() < 1e-6, "{}", f.w);
        assert!((f.h - 26.7857143).abs() < 1e-6, "{}", f.h);
    }

    #[test]
    fn culling_x_prune_stops_recursion() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        let cam = Camera::frame(1.0, 600.0);
        // viewport only 80px wide: x1 = 93.33 > 80 → depth ≥ 1 pruned
        let items = visible_nodes(&tree, &layout, &cam, 80.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec![""]);
    }

    #[test]
    fn gutters_are_clipped_narrow_dots() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // two octaves past home (zoom·64), centered on g: root and b.rs are
        // zoomed-past ancestors → 24px gutter strips, clipped to the viewport
        let cam = Camera { center_y: 0.6875, zoom: 256000.0 / 7.0 };
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        // a.rs and f are entirely above the viewport (y-pruned)
        assert_eq!(names, vec!["", "b.rs", "g"]);
        let rungs: Vec<Rung> = items.iter().map(|i| i.rung).collect();
        assert_eq!(rungs, vec![Rung::Dot, Rung::Dot, Rung::Card]);
        // root gutter: x=0, w=24, y clipped to [-2, 602]
        let root = &items[0].px;
        assert!((root.x - 0.0).abs() < 1e-6 && (root.w - 24.0).abs() < 1e-6);
        assert!((root.y - -2.0).abs() < 1e-6 && (root.h - 604.0).abs() < 1e-6);
        // g: x = 24+24 = 48, w = 93.33 (decay side), y = 300, h clipped to 302
        let g = &items[2].px;
        assert!((g.x - 48.0).abs() < 1e-6, "{}", g.x);
        assert!((g.w - 93.3333333).abs() < 1e-6, "{}", g.w);
        assert!((g.y - 300.0).abs() < 1e-6, "{}", g.y);
        assert!((g.h - 302.0).abs() < 1e-6, "{}", g.h);
        // nothing exceeds the clipped viewport band
        for i in &items {
            assert!(i.px.y >= -2.0 - 1e-9 && i.px.y + i.px.h <= 602.0 + 1e-9);
        }
    }
```

And change `culling_offscreen_y_is_empty` to construct the camera the new way:

```rust
    #[test]
    fn culling_offscreen_y_is_empty() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        let mut cam = Camera::frame(1.0, 600.0);
        cam.center_y = 100.0; // world is y ∈ [0,1]
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        assert!(items.is_empty());
    }
```

- [ ] **Step 4: Update treemap.rs to the new APIs**

Exact edits in `crates/outrider/src/treemap.rs` (nothing else changes — PaintItem, the canvas paint closure, and text rendering stay as they are):

1. `home_camera` loses its width argument:
```rust
    fn home_camera(&self, vh: f64) -> Camera {
        Camera::frame(self.root_world_height(), vh)
    }
```
(the `use crate::world::{self, Rung};` import stays exactly as it is — `paint_items` still calls `world::visible_nodes`)

2. In `paint_items`, the home-camera call site:
```rust
        if self.camera.is_none() {
            let c = self.home_camera(vh);
            self.home_zoom = c.zoom;
            self.camera = Some(c);
        }
```

3. The mouse-move handler pans y only:
```rust
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _w, cx| {
                if e.pressed_button != Some(gpui::MouseButton::Left) {
                    return;
                }
                let Some(last) = this.drag_last else { return };
                let dy = f64::from(e.position.y - last.y);
                if let Some(cam) = this.camera.as_mut() {
                    cam.pan(dy);
                }
                this.drag_last = Some(e.position);
                cx.notify();
            }))
```

4. The scroll handler drops the x/vw arguments:
```rust
            .on_scroll_wheel(cx.listener(move |this, e: &gpui::ScrollWheelEvent, w, cx| {
                let dy = match e.delta {
                    gpui::ScrollDelta::Pixels(p) => f64::from(p.y),
                    gpui::ScrollDelta::Lines(l) => l.y as f64 * 40.0,
                };
                let vh = f64::from(w.viewport_size().height);
                if let Some(cam) = this.camera.as_mut() {
                    // scroll up (positive dy) zooms in; flip the sign here if
                    // manual testing shows it inverted on this platform
                    let factor = (dy * 0.002).exp();
                    cam.zoom_about(f64::from(e.position.y), vh, factor, min_zoom, max_zoom);
                }
                cx.notify();
            }))
```

5. The Home-key handler:
```rust
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, w, cx| {
                if e.keystroke.key == "home" {
                    let c = this.home_camera(f64::from(w.viewport_size().height));
                    this.home_zoom = c.zoom;
                    this.camera = Some(c);
                    cx.notify();
                }
            }))
```

- [ ] **Step 5: Run the full test suite and lints**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test 2>&1 | tail -15`
Expected: all tests in all three crates pass (world: Task 1 tests + 5 culling/band tests; camera: 5 tests; treemap truncation; outrider-index and outrider-layout untouched).

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings 2>&1 | tail -5`
Expected: clean. (Note: `zoom_about` is down to 6 args, so the old `#[allow(clippy::too_many_arguments)]` must NOT be carried over to camera.rs; the new `walk` has 8 args and carries the allow, as shown in Step 2.)

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build 2>&1 | tail -3`
Expected: clean build of the `outrider` binary.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/camera.rs crates/outrider/src/world.rs crates/outrider/src/treemap.rs
git commit -m "feat: screen-space column widths with y-only camera"
```

---

## Manual exit gate (not a subagent task)

After both tasks land, the human runs `cargo run -p outrider -- .` and verifies (spec §7): zooming from home into a deep symbol never grows a column past 400px; passed ancestors compress smoothly into 24px gutters; no width popping; y pan/zoom behavior identical to before. Tuning candidates if the feel is off: `MAX_COLUMN_PX`, `GUTTER_PX`, `LABEL_MIN_W` in `world.rs`.
