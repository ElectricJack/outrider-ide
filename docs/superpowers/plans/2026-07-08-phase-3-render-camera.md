# Phase 3 — Render + Camera Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The treemap of a real repo on screen in GPUI: mouse pan, continuous wheel zoom, Home frames root; Dot/Label/Card fidelity rungs by pixel height; gray→red churn fill.

**Architecture:** Pure math (geometry, camera, culling, rung selection, colors) lives in `world.rs`/`camera.rs`/`theme.rs` with zero GPUI imports and full headless unit tests. `treemap.rs` is a canvas-style GPUI element that converts the culled draw list into `paint_quad`/`shape_line` calls; input handlers on the wrapping `div` mutate a `Camera` struct and `cx.notify()`. One-way data flow: `index_repo` → `layout` → render; the app never mutates `SymbolTree` or `WorldLayout`.

**Tech Stack:** Rust, GPUI pinned at rev `029bf2f284b4e59f20175d78443e630468f3a3e5` + `gpui_platform` (wayland) at the same rev (do NOT touch these pins), `outrider-index`, `outrider-layout`.

**Spec:** `docs/superpowers/specs/2026-07-08-phase-3-render-camera-design.md` (governing), parent spec §6.1/§6.3/§7.1/§7.2.

## Global Constraints

- **No GPUI types** in `world.rs`, `camera.rs`, `theme.rs`. GPUI appears only in `treemap.rs` and `main.rs`.
- **f64 composition:** world coordinates and camera math in f64; f32 (`Pixels`) appears only at the final paint call. `debug_assert!(abs < 2f64.powi(53))` guards composition depth.
- **Constants (exact values):** `CELL_ASPECT = 3.0`, `MERGE_PX = 4.0`, `LABEL_PX = 20.0`, `CARD_PX = 80.0`; colors `BG = 0x1a1a1c`, `FILL_COLD = 0x2a2a2e`, `FILL_HOT = 0xb03030`, `TEXT_PRIMARY = 0xd8d8d8`, `TEXT_SECONDARY = 0x9a9a9a`; home margin factor `1.05`; zoom clamp min = `0.5 × home_zoom`, max = `viewport_height_px × 8^15`.
- **Rung thresholds:** height < 4 px → merged (not drawn, subtree pruned); 4–20 Dot; 20–80 Label; ≥ 80 Card.
- App owns only ephemeral state (camera, drag state, focus handle). Tree and layout are immutable after startup.
- Interim `dead_code` warnings are expected until Task 5 wires the modules together; the branch must end (Task 7) with `cargo clippy --workspace --all-targets` clean.
- Environment: prefix cargo commands with `export PATH="$HOME/.cargo/bin:$PATH" && `.
- GPUI API signatures referenced below were verified against the pinned rev (canvas: `gpui/src/elements/canvas.rs`; `paint_quad`/`quad`/`fill`: `gpui/src/window.rs`; `shape_line`: `gpui/src/text_system.rs`; events: `gpui/src/interactive.rs`). If a call doesn't compile verbatim, adjust to the rev's actual signature — do not change the pinned rev.

---

### Task 1: World geometry + rung selection (`world.rs`)

**Files:**
- Modify: `crates/outrider/Cargo.toml` (add path deps)
- Modify: `crates/outrider/src/main.rs` (add `mod world;`)
- Create: `crates/outrider/src/world.rs`

**Interfaces:**
- Consumes: `outrider_layout::RATIO`
- Produces (later tasks rely on these exact names): `CELL_ASPECT: f64`, `MERGE_PX: f64`, `LABEL_PX: f64`, `CARD_PX: f64`, `column_scale(depth: u8) -> f64`, `column_x(depth: u8) -> f64`, `world_width() -> f64`, `WorldRect { x, y, w, h: f64 }`, `node_world_rect(depth: u8, abs_start: f64, len: u64) -> WorldRect`, `Rung { Dot, Label, Card }`, `rung_for_px_height(h: f64) -> Option<Rung>`

- [ ] **Step 1: Add dependencies**

In `crates/outrider/Cargo.toml` under `[dependencies]` (keep the existing gpui/gpui_platform lines untouched):

```toml
outrider-index = { path = "../outrider-index" }
outrider-layout = { path = "../outrider-layout" }
```

- [ ] **Step 2: Write the failing tests**

Create `crates/outrider/src/world.rs` containing only the test module for now, and add `mod world;` as the first line of `crates/outrider/src/main.rs` (before the `use` block):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn column_geometry() {
        close(column_scale(0), 1.0);
        close(column_scale(1), 0.125);
        close(column_scale(2), 0.015625);
        close(column_x(0), 0.0);
        close(column_x(1), 3.0);
        close(column_x(2), 3.375);
        close(world_width(), 24.0 / 7.0);
    }

    #[test]
    fn worked_example_rects() {
        // root {0,0,1}
        let r = node_world_rect(0, 0.0, 1);
        close(r.x, 0.0);
        close(r.y, 0.0);
        close(r.w, 3.0);
        close(r.h, 1.0);
        // b.rs::g — depth 2, abs cell 44, len 1 (Phase 2 worked example)
        let g = node_world_rect(2, 44.0, 1);
        close(g.x, 3.375);
        close(g.y, 0.6875);
        close(g.w, 0.046875);
        close(g.h, 0.015625);
    }

    #[test]
    fn rung_thresholds() {
        assert_eq!(rung_for_px_height(3.9), None);
        assert_eq!(rung_for_px_height(4.0), Some(Rung::Dot));
        assert_eq!(rung_for_px_height(19.9), Some(Rung::Dot));
        assert_eq!(rung_for_px_height(20.0), Some(Rung::Label));
        assert_eq!(rung_for_px_height(79.9), Some(Rung::Label));
        assert_eq!(rung_for_px_height(80.0), Some(Rung::Card));
        assert_eq!(rung_for_px_height(100_000.0), Some(Rung::Card));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p outrider`
Expected: FAIL to compile — `column_scale` etc. not found.

- [ ] **Step 4: Implement**

Prepend to `crates/outrider/src/world.rs` (above the test module):

```rust
use outrider_layout::RATIO;

pub const CELL_ASPECT: f64 = 3.0;
pub const MERGE_PX: f64 = 4.0;
pub const LABEL_PX: f64 = 20.0;
pub const CARD_PX: f64 = 80.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 8^-depth: the size scale of level-`depth` cells relative to level 0.
pub fn column_scale(depth: u8) -> f64 {
    (RATIO as f64).powi(-(depth as i32))
}

/// X_d = CELL_ASPECT * (1 - 8^-d) * 8/7 — where the depth-d column begins.
pub fn column_x(depth: u8) -> f64 {
    let r = RATIO as f64;
    CELL_ASPECT * (1.0 - column_scale(depth)) * r / (r - 1.0)
}

/// Total world width: the columns converge to CELL_ASPECT * 8/7.
pub fn world_width() -> f64 {
    let r = RATIO as f64;
    CELL_ASPECT * r / (r - 1.0)
}

pub fn node_world_rect(depth: u8, abs_start: f64, len: u64) -> WorldRect {
    let s = column_scale(depth);
    WorldRect {
        x: column_x(depth),
        y: abs_start * s,
        w: CELL_ASPECT * s,
        h: len as f64 * s,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    Dot,
    Label,
    Card,
}

pub fn rung_for_px_height(h: f64) -> Option<Rung> {
    if h < MERGE_PX {
        None
    } else if h < LABEL_PX {
        Some(Rung::Dot)
    } else if h < CARD_PX {
        Some(Rung::Label)
    } else {
        Some(Rung::Card)
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p outrider`
Expected: 3 passed. (`dead_code` warnings are expected until Task 5.)

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/Cargo.toml crates/outrider/src/main.rs crates/outrider/src/world.rs Cargo.lock
git commit -m "feat: world geometry and fidelity-rung selection for render"
```

---

### Task 2: Camera (`camera.rs`)

**Files:**
- Modify: `crates/outrider/src/main.rs` (add `mod camera;`)
- Create: `crates/outrider/src/camera.rs`

**Interfaces:**
- Consumes: nothing (pure).
- Produces: `Camera { center_x: f64, center_y: f64, zoom: f64 }` with `world_to_screen(wx, wy, vw, vh) -> (f64, f64)`, `screen_to_world(sx, sy, vw, vh) -> (f64, f64)`, `pan(&mut self, dx_px, dy_px)`, `zoom_about(&mut self, sx, sy, vw, vh, factor, min_zoom, max_zoom)`, `Camera::frame(world_w, world_h, vw, vh) -> Camera`

- [ ] **Step 1: Write the failing tests**

Create `crates/outrider/src/camera.rs` with the test module, and add `mod camera;` to `main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn screen_world_round_trip() {
        let c = Camera { center_x: 1.5, center_y: 0.5, zoom: 200.0 };
        let (sx, sy) = c.world_to_screen(2.0, 0.75, 800.0, 600.0);
        close(sx, 500.0); // (2.0-1.5)*200 + 400
        close(sy, 350.0); // (0.75-0.5)*200 + 300
        let (wx, wy) = c.screen_to_world(sx, sy, 800.0, 600.0);
        close(wx, 2.0);
        close(wy, 0.75);
    }

    #[test]
    fn pan_moves_center_against_drag() {
        let mut c = Camera { center_x: 1.0, center_y: 1.0, zoom: 2.0 };
        c.pan(10.0, -4.0); // dragging content right/up moves center left/down
        close(c.center_x, -4.0); // 1.0 - 10.0/2.0
        close(c.center_y, 3.0);  // 1.0 - (-4.0)/2.0
    }

    #[test]
    fn zoom_about_fixes_cursor_point() {
        for &(sx, sy) in &[(0.0, 0.0), (400.0, 300.0), (799.0, 599.0), (123.0, 456.0)] {
            for &f in &[0.5, 0.9, 1.1, 2.0, 7.3] {
                let mut c = Camera { center_x: 1.7, center_y: 0.4, zoom: 222.0 };
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
        let mut c = Camera { center_x: 0.0, center_y: 0.0, zoom: 100.0 };
        c.zoom_about(400.0, 300.0, 800.0, 600.0, 1e9, 50.0, 400.0);
        close(c.zoom, 400.0);
        c.zoom_about(400.0, 300.0, 800.0, 600.0, 1e-9, 50.0, 400.0);
        close(c.zoom, 50.0);
    }

    #[test]
    fn frame_fits_world_with_margin() {
        // world 24/7 x 1.0 in an 800x600 viewport
        let c = Camera::frame(24.0 / 7.0, 1.0, 800.0, 600.0);
        close(c.center_x, 12.0 / 7.0);
        close(c.center_y, 0.5);
        close(c.zoom, 800.0 / ((24.0 / 7.0) * 1.05)); // width-limited here
        // framed world fits the viewport
        assert!((24.0 / 7.0) * c.zoom <= 800.0 + 1e-9);
        assert!(1.0 * c.zoom <= 600.0 + 1e-9);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider`
Expected: FAIL to compile — `Camera` not found.

- [ ] **Step 3: Implement**

Prepend to `crates/outrider/src/camera.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World point at the viewport center.
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

    /// Drag by (dx, dy) pixels: content follows the cursor.
    pub fn pan(&mut self, dx_px: f64, dy_px: f64) {
        self.center_x -= dx_px / self.zoom;
        self.center_y -= dy_px / self.zoom;
    }

    /// Multiply zoom by `factor`, keeping the world point under (sx, sy) fixed.
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

    /// Frame a world extent of (world_w x world_h) with a 5% margin.
    pub fn frame(world_w: f64, world_h: f64, vw: f64, vh: f64) -> Camera {
        let zoom = (vw / (world_w * 1.05)).min(vh / (world_h * 1.05));
        Camera { center_x: world_w / 2.0, center_y: world_h / 2.0, zoom }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider`
Expected: 8 passed (3 world + 5 camera).

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/main.rs crates/outrider/src/camera.rs
git commit -m "feat: camera with pan, cursor-anchored zoom, and framing"
```

---

### Task 3: Culling walk (`visible_nodes` in `world.rs`)

**Files:**
- Modify: `crates/outrider/src/world.rs`

**Interfaces:**
- Consumes: `Camera` (Task 2), `outrider_index::{SymbolNode, SymbolTree}`, `outrider_layout::WorldLayout`.
- Produces: `PxRect { x, y, w, h: f64 }`, `DrawItem<'a> { node: &'a SymbolNode, px: PxRect, rung: Rung }`, `visible_nodes<'a>(tree: &'a SymbolTree, layout: &WorldLayout, camera: &Camera, vw: f64, vh: f64) -> Vec<DrawItem<'a>>` (pre-order = painter's order).

- [ ] **Step 1: Write the failing tests**

Append inside the existing `mod tests` in `world.rs` (add the new imports at the top of `mod tests`):

```rust
    use crate::camera::Camera;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    fn n(kind: SymbolKind, qp: &str, name: &str, measure: u64, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
            name: name.into(),
            byte_range: None,
            measure,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    /// The Phase 2 worked example: root{0,0,1}; a.rs{1,0,4}; b.rs{1,5,1}; f{2,0,3}; g{2,4,1}.
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
                        10,
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

    #[test]
    fn culling_home_view_prunes_submerge_nodes() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        let cam = Camera::frame(world_width(), 1.0, 800.0, 600.0); // zoom ≈ 222.22
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        // g is ~3.47px tall at home zoom -> merged into b.rs
        assert_eq!(names, vec!["", "a.rs", "b.rs", "f"]);
        let rungs: Vec<Rung> = items.iter().map(|i| i.rung).collect();
        assert_eq!(rungs, vec![Rung::Card, Rung::Card, Rung::Label, Rung::Dot]);
    }

    #[test]
    fn culling_offscreen_y_is_empty() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        let mut cam = Camera::frame(world_width(), 1.0, 800.0, 600.0);
        cam.center_y = 100.0; // world is y ∈ [0,1]
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        assert!(items.is_empty());
    }

    #[test]
    fn culling_recurses_past_offscreen_left_parent() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // Zoomed onto b.rs's children: root column entirely off-screen left,
        // a.rs off-screen top, but b.rs/f/g visible.
        let cam = Camera { center_x: 3.4, center_y: 0.69, zoom: 2000.0 };
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["b.rs", "f", "g"]);
        // hand-computed rect for b.rs: x=(3.0-3.4)*2000+400, y=(0.625-0.69)*2000+300, w=0.375*2000, h=0.125*2000
        let b = &items[0].px;
        assert!((b.x - -400.0).abs() < 1e-6, "{}", b.x);
        assert!((b.y - 170.0).abs() < 1e-6, "{}", b.y);
        assert!((b.w - 750.0).abs() < 1e-6, "{}", b.w);
        assert!((b.h - 250.0).abs() < 1e-6, "{}", b.h);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider`
Expected: FAIL to compile — `visible_nodes` not found.

- [ ] **Step 3: Implement**

Add to `world.rs` (below `rung_for_px_height`, above `mod tests`):

```rust
use outrider_index::{SymbolNode, SymbolTree};
use outrider_layout::WorldLayout;

use crate::camera::Camera;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PxRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug)]
pub struct DrawItem<'a> {
    pub node: &'a SymbolNode,
    pub px: PxRect,
    pub rung: Rung,
}

/// Cull the tree against the viewport and the 4px merge rule.
/// Returns visible nodes in pre-order (parents before children = painter's order).
pub fn visible_nodes<'a>(
    tree: &'a SymbolTree,
    layout: &WorldLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
) -> Vec<DrawItem<'a>> {
    let mut out = Vec::new();
    walk(&tree.root, layout, camera, vw, vh, 0.0, &mut out);
    out
}

fn walk<'a>(
    node: &'a SymbolNode,
    layout: &WorldLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
    parent_abs: f64,
    out: &mut Vec<DrawItem<'a>>,
) {
    let Some(nl) = layout.nodes.get(&node.id) else { return };
    let depth = nl.cells.level;
    let abs = parent_abs * outrider_layout::RATIO as f64 + nl.cells.start as f64;
    debug_assert!(abs < 2f64.powi(53), "cell address exceeds exact f64 range");
    let rect = node_world_rect(depth, abs, nl.cells.len);
    let (px_x, px_y) = camera.world_to_screen(rect.x, rect.y, vw, vh);
    let px_w = rect.w * camera.zoom;
    let px_h = rect.h * camera.zoom;

    // Below the merge threshold: this node merges into its parent's tile,
    // and children (8x smaller) are below it too. Stop.
    let Some(rung) = rung_for_px_height(px_h) else { return };
    // Children's y-ranges are contained in the parent's: off-screen y prunes the subtree.
    if px_y > vh || px_y + px_h < 0.0 {
        return;
    }
    // Deeper columns are further right: past the right edge prunes the subtree.
    if px_x > vw {
        return;
    }
    // The node's own column may be off-screen left while children are visible:
    // skip drawing but keep recursing.
    if px_x + px_w > 0.0 {
        out.push(DrawItem { node, px: PxRect { x: px_x, y: px_y, w: px_w, h: px_h }, rung });
    }
    for child in &node.children {
        walk(child, layout, camera, vw, vh, abs, out);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider`
Expected: 11 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/world.rs
git commit -m "feat: viewport culling walk producing the per-frame draw list"
```

---

### Task 4: Theme (`theme.rs`)

**Files:**
- Modify: `crates/outrider/src/main.rs` (add `mod theme;`)
- Create: `crates/outrider/src/theme.rs`

**Interfaces:**
- Consumes: nothing (pure; colors are `u32` `0xRRGGBB` — GPUI conversion happens in `treemap.rs` via `rgb()`).
- Produces: `BG`, `FILL_COLD`, `FILL_HOT`, `TEXT_PRIMARY`, `TEXT_SECONDARY: u32`, `FONT_FAMILY: &str`, `churn_fill(churn: f32) -> u32`, `border_for(fill: u32) -> u32`

- [ ] **Step 1: Write the failing tests**

Create `crates/outrider/src/theme.rs` with the test module; add `mod theme;` to `main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn churn_endpoints_and_clamp() {
        assert_eq!(churn_fill(0.0), FILL_COLD);
        assert_eq!(churn_fill(1.0), FILL_HOT);
        assert_eq!(churn_fill(-0.5), FILL_COLD);
        assert_eq!(churn_fill(2.0), FILL_HOT);
    }

    #[test]
    fn churn_midpoint_is_channelwise() {
        // 0x2a2a2e -> 0xb03030 at t=0.5: r=(0x2a+0xb0)/2=0x6d, g=(0x2a+0x30)/2=0x2d, b=(0x2e+0x30)/2=0x2f
        assert_eq!(churn_fill(0.5), 0x6d2d2f);
    }

    #[test]
    fn border_is_lighter_than_fill() {
        let f = churn_fill(0.3);
        let b = border_for(f);
        assert!((b >> 16) & 0xff >= (f >> 16) & 0xff);
        assert!((b >> 8) & 0xff >= (f >> 8) & 0xff);
        assert!(b & 0xff >= f & 0xff);
        assert_ne!(b, f);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Prepend to `theme.rs`:

```rust
pub const BG: u32 = 0x1a1a1c;
pub const FILL_COLD: u32 = 0x2a2a2e;
pub const FILL_HOT: u32 = 0xb03030;
pub const TEXT_PRIMARY: u32 = 0xd8d8d8;
pub const TEXT_SECONDARY: u32 = 0x9a9a9a;
/// Adjust if this family is absent under WSLg (`fc-list | grep -i mono`).
pub const FONT_FAMILY: &str = "DejaVu Sans Mono";

fn lerp_channel(a: u32, b: u32, t: f32) -> u32 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u32 & 0xff
}

fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let r = lerp_channel((a >> 16) & 0xff, (b >> 16) & 0xff, t);
    let g = lerp_channel((a >> 8) & 0xff, (b >> 8) & 0xff, t);
    let bl = lerp_channel(a & 0xff, b & 0xff, t);
    (r << 16) | (g << 8) | bl
}

/// Neutral gray -> red, linear per-channel in sRGB.
pub fn churn_fill(churn: f32) -> u32 {
    lerp_rgb(FILL_COLD, FILL_HOT, churn.clamp(0.0, 1.0))
}

/// Border: fill lightened 12% toward white.
pub fn border_for(fill: u32) -> u32 {
    lerp_rgb(fill, 0xffffff, 0.12)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider`
Expected: 14 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/main.rs crates/outrider/src/theme.rs
git commit -m "feat: churn color ramp and theme constants"
```

---

### Task 5: Startup pipeline + quad rendering (`treemap.rs`, `main.rs`)

First pixels: index → layout → window, every visible node painted as a filled, bordered quad (no text yet). This task is GPUI integration; expect compiler iteration against the pinned rev.

**Files:**
- Create: `crates/outrider/src/treemap.rs`
- Modify: `crates/outrider/src/main.rs` (full rewrite below)

**Interfaces:**
- Consumes: `visible_nodes`, `Rung`, `world_width` (Task 1/3), `Camera` (Task 2), `theme` (Task 4), `outrider_index::index_repo`, `outrider_layout::layout`.
- Produces: `TreemapView` (GPUI view) with `TreemapView::new(tree: SymbolTree, layout: WorldLayout, cx: &mut Context<Self>) -> Self`. Tasks 6–7 extend `render`/`paint` and add input handlers to this struct.

- [ ] **Step 1: Rewrite `main.rs`**

```rust
mod camera;
mod theme;
mod treemap;
mod world;

use std::path::PathBuf;

use gpui::{px, size, App, Bounds, WindowBounds, WindowOptions};
use gpui_platform::application;

use crate::treemap::TreemapView;

fn main() {
    let repo = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("no working directory"));
    eprintln!("indexing {}…", repo.display());
    let tree = match outrider_index::index_repo(&repo) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    };
    let layout = outrider_layout::layout(&tree);
    eprintln!("{} symbols laid out", layout.nodes.len());

    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| TreemapView::new(tree, layout, cx)),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
```

Note: the closure passed to `open_window` moves `tree`/`layout`; if the borrow checker objects about `FnOnce`, wrap them in `Option`/`take` or build the view before `open_window` — keep the boot pattern otherwise identical to Phase 0.

- [ ] **Step 2: Write `treemap.rs`**

```rust
use gpui::{
    canvas, div, fill, point, prelude::*, px, quad, rgb, size, App, Bounds, BorderStyle, Context,
    FocusHandle, Pixels, Window,
};
use outrider_index::SymbolTree;
use outrider_layout::WorldLayout;

use crate::camera::Camera;
use crate::theme;
use crate::world::{self, Rung};

pub struct TreemapView {
    tree: SymbolTree,
    layout: WorldLayout,
    /// None until the first render supplies a viewport; then Home-framed.
    camera: Option<Camera>,
    home_zoom: f64,
    drag_last: Option<gpui::Point<Pixels>>,
    focus_handle: FocusHandle,
}

/// Owned, GPUI-free paint instruction — built in render (which may borrow
/// self), moved into the 'static canvas closure.
struct PaintItem {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fill: u32,
    border: u32,
    rung: Rung,
    name: String,
    meta: String,
}

impl TreemapView {
    pub fn new(tree: SymbolTree, layout: WorldLayout, cx: &mut Context<Self>) -> Self {
        Self {
            tree,
            layout,
            camera: None,
            home_zoom: 1.0,
            drag_last: None,
            focus_handle: cx.focus_handle(),
        }
    }

    fn root_world_height(&self) -> f64 {
        self.layout
            .nodes
            .get(&self.tree.root.id)
            .map(|nl| nl.cells.len as f64)
            .unwrap_or(1.0)
    }

    fn home_camera(&self, vw: f64, vh: f64) -> Camera {
        Camera::frame(world::world_width(), self.root_world_height(), vw, vh)
    }

    fn paint_items(&mut self, vw: f64, vh: f64) -> Vec<PaintItem> {
        let camera = *self.camera.get_or_insert_with(|| {
            let c = self.home_camera(vw, vh);
            self.home_zoom = c.zoom;
            c
        });
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
    }
}

impl Render for TreemapView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let (vw, vh) = (f64::from(vp.width), f64::from(vp.height));
        let items = self.paint_items(vw, vh);

        div().size_full().bg(rgb(theme::BG)).child(
            canvas(
                |_bounds, _window, _cx| {},
                move |bounds, _prepaint, window, _cx| {
                    let origin = bounds.origin;
                    for item in &items {
                        let b = Bounds::new(
                            point(origin.x + px(item.x), origin.y + px(item.y)),
                            size(px(item.w), px(item.h)),
                        );
                        window.paint_quad(quad(
                            b,
                            px(0.),
                            rgb(item.fill),
                            px(1.),
                            rgb(item.border),
                            BorderStyle::default(),
                        ));
                    }
                },
            )
            .size_full(),
        )
    }
}
```

If `f64::from(Pixels)` doesn't exist at this rev, use `vp.width.0 as f64`. If `quad(...)`'s `Into` conversions reject `px(0.)`/`px(1.)`, use `gpui::Corners::all(px(0.))` / `gpui::Edges::all(px(1.))`. Sub-1px borders on Dot-rung boxes are fine; if borders overwhelm at Dot rung, drop the border for `Rung::Dot` by painting `fill(b, rgb(item.fill))` instead — decide by eye in Step 4.

- [ ] **Step 3: Compile and fix**

Run: `cargo check -p outrider` then `cargo clippy -p outrider --all-targets`
Expected: compiles; remaining warnings only for not-yet-used items (e.g., `drag_last`, `focus_handle`, `home_zoom`, `meta`, `rung`, text colors — all consumed by Tasks 6–7).

- [ ] **Step 4: Manual verification (requires WSLg display)**

Run: `cargo run -p outrider -- .`
Expected: window opens showing the outrider repo as nested gray boxes — a wide root column on the left, folder/file columns to its right. Report what you see in the task report; a screenshot is not required.

- [ ] **Step 5: Run tests**

Run: `cargo test -p outrider`
Expected: 14 passed (no regressions).

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/main.rs crates/outrider/src/treemap.rs
git commit -m "feat: startup pipeline and quad rendering of the treemap"
```

---

### Task 6: Label and Card text

**Files:**
- Modify: `crates/outrider/src/treemap.rs`

**Interfaces:**
- Consumes: `PaintItem` fields `rung`/`name`/`meta` (Task 5), `theme::{FONT_FAMILY, TEXT_PRIMARY, TEXT_SECONDARY}`.
- Produces: no new public interface — extends the canvas paint closure.

- [ ] **Step 1: Add a text-truncation helper with tests**

Add to `treemap.rs` (module scope), plus a test module at the bottom of the file:

```rust
/// Approximate char budget for a column `w_px` wide at `font_px` monospace.
/// 0.62 ≈ advance-width/em for common monospace faces; exactness is not
/// required — worst case the ellipsis lands a character early.
fn truncate_to_width(name: &str, w_px: f32, font_px: f32) -> Option<String> {
    let budget = ((w_px - 12.0) / (font_px * 0.62)).floor() as isize;
    if budget < 2 {
        return None; // no room for any text
    }
    let budget = budget as usize;
    if name.chars().count() <= budget {
        Some(name.to_string())
    } else {
        let cut: String = name.chars().take(budget - 1).collect();
        Some(format!("{cut}…"))
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_to_width;

    #[test]
    fn truncation() {
        // 12 + 10*0.62*12 = wide enough for exactly 10 chars at 12px
        let w = 12.0 + 10.0 * 0.62 * 12.0;
        assert_eq!(truncate_to_width("short.rs", w, 12.0), Some("short.rs".into()));
        assert_eq!(
            truncate_to_width("a_very_long_file_name.rs", w, 12.0),
            Some("a_very_lo…".into())
        );
        assert_eq!(truncate_to_width("anything", 10.0, 12.0), None);
        // multi-byte chars must not panic
        assert_eq!(truncate_to_width("ééééééééééééé", w, 12.0), Some("ééééééééé…".into()));
    }
}
```

- [ ] **Step 2: Run tests to verify the helper passes**

Run: `cargo test -p outrider`
Expected: 15 passed.

- [ ] **Step 3: Paint text in the canvas closure**

Extend the paint closure in `render` — after each quad is painted, add:

```rust
                        if item.rung == Rung::Dot || item.h < 14.0 {
                            continue;
                        }
                        let font_px = 12.0_f32;
                        let line_height = px(font_px * 1.3);
                        let run = |len: usize, color: u32| gpui::TextRun {
                            len,
                            font: gpui::font(theme::FONT_FAMILY),
                            color: rgb(color).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        if let Some(name) = truncate_to_width(&item.name, item.w, font_px) {
                            let line = window.text_system().shape_line(
                                name.clone().into(),
                                px(font_px),
                                &[run(name.len(), theme::TEXT_PRIMARY)],
                                None,
                            );
                            let ty = if item.rung == Rung::Label {
                                // vertically centered in the box
                                item.y + (item.h - font_px * 1.3) / 2.0
                            } else {
                                item.y + 4.0
                            };
                            let _ = line.paint(
                                point(origin.x + px(item.x + 6.0), origin.y + px(ty)),
                                line_height,
                                gpui::TextAlign::Left,
                                None,
                                window,
                                _cx,
                            );
                        }
                        if item.rung == Rung::Card {
                            if let Some(meta) = truncate_to_width(&item.meta, item.w, font_px) {
                                let line = window.text_system().shape_line(
                                    meta.clone().into(),
                                    px(font_px),
                                    &[run(meta.len(), theme::TEXT_SECONDARY)],
                                    None,
                                );
                                let _ = line.paint(
                                    point(
                                        origin.x + px(item.x + 6.0),
                                        origin.y + px(item.y + 4.0 + font_px * 1.4),
                                    ),
                                    line_height,
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
                            }
                        }
```

The loop must become `for item in &items { ... }` with `continue` valid; rename the paint-closure's `_cx` binding to match usage. If `ShapedLine::paint` at this rev returns `Result`, the `let _ =` swallows it deliberately (a failed text paint must not crash the frame); if it returns `()`, drop the `let _ =`.

- [ ] **Step 4: Compile, then manual verification**

Run: `cargo check -p outrider`, then `cargo run -p outrider -- .`
Expected: file/folder names appear on boxes ≥20px tall; larger boxes (≥80px) additionally show a dimmer `N · pM · KL` line. Names truncate with `…` rather than overflowing their column. If no text renders at all, check the font family exists (`fc-list | grep -i "dejavu sans mono"`) and adjust `theme::FONT_FAMILY` to an installed monospace family.

- [ ] **Step 5: Run tests**

Run: `cargo test -p outrider`
Expected: 15 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: label and card text rungs with width truncation"
```

---

### Task 7: Input — drag pan, wheel zoom, Home; exit-gate check

**Files:**
- Modify: `crates/outrider/src/treemap.rs`

**Interfaces:**
- Consumes: `Camera::{pan, zoom_about}`, `home_camera`, `home_zoom`, `drag_last`, `focus_handle` (all present from Tasks 2/5).
- Produces: the complete Phase 3 interaction surface. No API for later phases beyond `TreemapView` itself.

- [ ] **Step 1: Wire input handlers**

In `render`, replace the outer `div().size_full().bg(...)` chain with:

```rust
        let max_zoom = vh * 8f64.powi(15);
        let min_zoom = self.home_zoom * 0.5;

        div()
            .size_full()
            .bg(rgb(theme::BG))
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, _w, _cx| {
                    this.drag_last = Some(e.position);
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _e: &gpui::MouseUpEvent, _w, _cx| {
                    this.drag_last = None;
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _w, cx| {
                if e.pressed_button != Some(gpui::MouseButton::Left) {
                    return;
                }
                let Some(last) = this.drag_last else { return };
                let (dx, dy) = (f64::from(e.position.x - last.x), f64::from(e.position.y - last.y));
                if let Some(cam) = this.camera.as_mut() {
                    cam.pan(dx, dy);
                }
                this.drag_last = Some(e.position);
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(move |this, e: &gpui::ScrollWheelEvent, w, cx| {
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
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, w, cx| {
                if e.keystroke.key == "home" {
                    let vp = w.viewport_size();
                    let c = this.home_camera(f64::from(vp.width), f64::from(vp.height));
                    this.home_zoom = c.zoom;
                    this.camera = Some(c);
                    cx.notify();
                }
            }))
            .child(/* the canvas child from Task 5/6, unchanged */)
```

`render`'s signature gains use of `cx`: change `_cx: &mut Context<Self>` to `cx: &mut Context<Self>`. For key events to arrive, the view must hold focus: add at the top of `render`:

```rust
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle);
        }
```

(This is a single-view app; unconditional focus-grab is acceptable this phase. If `window.focus(&handle)` doesn't exist at this rev, use `self.focus_handle.focus(window)` — check `gpui/src/window.rs` for the rev's focus API.)

- [ ] **Step 2: Compile, clippy clean**

Run: `cargo check -p outrider && cargo clippy --workspace --all-targets`
Expected: compiles, zero warnings (everything is now wired; fix any leftover dead code by using or removing it).

- [ ] **Step 3: Run full test suite**

Run: `cargo test --workspace`
Expected: all green (26 from Phases 1–2 + 15 from this phase = 41 total; report exact count).

- [ ] **Step 4: Manual exit-gate verification (requires WSLg display)**

Run: `cargo run -p outrider -- .` and verify each:
1. Drag pans — content follows the cursor (if inverted, negate `pan` args and re-check).
2. Wheel zooms about the cursor — the box under the cursor stays put (if inverted, negate the `0.002` constant).
3. Zooming in reveals deeper structure (files → items) as rungs step Dot → Label → Card; zooming out merges them away; no flicker at rung boundaries.
4. Home reframes the whole repo instantly.
5. Boxes never move except in response to camera input.
6. Note pan/zoom smoothness (llvmpipe watch-item): report jank if any — a finding, not a failure.

Record the results of all six checks in the task report. Fix sign inversions (points 1–2) before committing; anything else observed goes in the report for the human's exit-gate review.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: mouse pan, cursor-anchored wheel zoom, and Home reframing"
```

---

## Notes for the executor

- Tasks 1–4 are pure transcription with complete code above. Tasks 5–7 are GPUI integration: the code above was written against the API reference for the pinned rev, but exact `Into`/`Result` shapes may need compiler-driven adjustment — the escape hatches are noted inline. Never bump the GPUI rev to resolve a compile error.
- Manual verification steps (5.4, 6.4, 7.4) need the WSLg display; they run `cargo run` and report observations. They cannot be skipped, but they also cannot block on aesthetic judgment — that's the human's exit-gate review after the branch lands.
- `lines_per_cell` tuning (spec §5) is explicitly deferred to the human's exit-gate session; do not tune it in this plan's tasks.
