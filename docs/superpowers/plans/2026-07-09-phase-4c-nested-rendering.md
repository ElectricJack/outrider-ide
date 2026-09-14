# Phase 4c: Nested Rendering + Natural-Size Framing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Boxes visibly nest inside ancestors; focusing a method frames it at natural content size with code visible whenever the box fits it; churn becomes a left-edge heat stripe on depth-shaded fills; corners round.

**Architecture:** Render-level containment only — the world/column math is untouched. `visible_nodes` extends each ancestor's drawn rect rightward over its visible descendants; `rung_for` gains a legibility upgrade for leaf items; a new `frame_leaf` computes natural-size framing with a code-width floor; `theme.rs` swaps churn fills for depth-shaded fills plus a churn stripe; `treemap.rs` wires framing and paints stripe/radius/code background.

**Tech Stack:** Rust workspace (`outrider` bin crate), GPUI pinned at rev 029bf2f284b4e59f20175d78443e630468f3a3e5. Spec: `docs/superpowers/specs/2026-07-09-phase-4c-nested-rendering-design.md`.

## Global Constraints

- Every cargo command needs `export PATH="$HOME/.cargo/bin:$PATH" && ` first.
- `cargo clippy --workspace -- -D warnings` must stay clean after every task.
- Work on the existing branch `phase-4b-tuning`; do not create branches.
- Exact constants: `NEST_PAD = 6.0`, `STRIPE_W: f32 = 3.0`, `CORNER_RADIUS: f32 = 4.0`, `CODE_BG = 0x101014`, depth ramp `0x17171B` → `0x3C3C46` over levels 0..=8, `BOTTOM_PAD = 6.0`, width-search factor `1.25`. `CODE_MIN_W = 300.0`, `FULL_PX = 700.0`, `FOCUS_FRACTION = 0.5`, `END_FRACTION = 0.95` unchanged.
- `natural_px(node) = HEADER + (1 + measure) · LINE_STEP + BOTTOM_PAD` where `FONT_PX = 12.0`, `LINE_STEP = FONT_PX · 1.3`, `HEADER = 4.0 + FONT_PX · 1.4`.
- "Leaf item" everywhere = `byte_range.is_some() && children.is_empty()` and kind is not File/Folder.
- GPUI-free modules (`world.rs`, `content.rs`, `camera.rs`, `focus.rs`, `theme.rs`) must not import gpui.

---

### Task 1: content.rs — shared constants, `is_leaf_item`, `natural_px`

**Files:**
- Modify: `crates/outrider/src/content.rs` (add constants + two fns + tests)
- Modify: `crates/outrider/src/treemap.rs` (constants move out; use shared predicate)

**Interfaces:**
- Consumes: nothing new.
- Produces: `content::FONT_PX: f64`, `content::LINE_STEP: f64`, `content::HEADER: f64`, `content::BOTTOM_PAD: f64`, `content::is_leaf_item(&SymbolNode) -> bool`, `content::natural_px(&SymbolNode) -> f64`. Tasks 3, 5, 6 call these.

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `content.rs`:

```rust
    #[test]
    fn natural_px_arithmetic() {
        // HEADER 20.8 + (1 + measure)·15.6 + BOTTOM_PAD 6
        let three = node(SymbolKind::Fn, "a.rs::f", 3, 0.0, 0, Some("fn f()"), None, vec![]);
        assert!((natural_px(&three) - 89.2).abs() < 1e-9);
        let long = node(SymbolKind::Fn, "a.rs::g", 200, 0.0, 0, Some("fn g()"), None, vec![]);
        assert!((natural_px(&long) - 3162.4).abs() < 1e-9);
    }

    #[test]
    fn leaf_item_predicate() {
        let mut f = node(SymbolKind::Fn, "a.rs::f", 3, 0.0, 0, None, None, vec![]);
        assert!(!is_leaf_item(&f)); // no byte_range
        f.byte_range = Some(0..10);
        assert!(is_leaf_item(&f));
        let mut file = node(SymbolKind::File, "a.rs", 3, 0.0, 0, None, None, vec![]);
        file.byte_range = Some(0..10);
        assert!(!is_leaf_item(&file)); // files are never leaf items
        let parent = node(SymbolKind::Impl, "a.rs::I", 3, 0.0, 0, None, None,
            vec![node(SymbolKind::Fn, "a.rs::I::m", 1, 0.0, 0, None, None, vec![])]);
        assert!(!is_leaf_item(&parent)); // has children
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider content:: 2>&1 | tail -5`
Expected: FAIL to compile — `natural_px`, `is_leaf_item` not found.

- [ ] **Step 3: Implement** — in `content.rs`, directly under the `use` lines add:

```rust
/// Monospace body font size (px); shared by content math and the paint path.
pub const FONT_PX: f64 = 12.0;
pub const LINE_STEP: f64 = FONT_PX * 1.3;
/// Name-row height: text top padding (4) plus one meta-line offset.
pub const HEADER: f64 = 4.0 + FONT_PX * 1.4;
/// Padding below the last body row inside a leaf box.
pub const BOTTOM_PAD: f64 = 6.0;

/// A code-bearing leaf: has source bytes, no children, and is an item
/// (not a file/folder). These are the boxes that render code at Full.
pub fn is_leaf_item(node: &SymbolNode) -> bool {
    node.byte_range.is_some()
        && node.children.is_empty()
        && !matches!(node.id.kind, SymbolKind::File | SymbolKind::Folder)
}

/// Natural pixel height of a leaf item's box: header + signature row +
/// one row per code line + bottom pad.
pub fn natural_px(node: &SymbolNode) -> f64 {
    HEADER + (1.0 + node.measure as f64) * LINE_STEP + BOTTOM_PAD
}
```

Then in `treemap.rs`:
1. Delete the three constant definitions (`const FONT_PX: f64 = 12.0;`, `const LINE_STEP: f64 = FONT_PX * 1.3;`, the `/// Name-row height…` comment + `const HEADER: f64 = 4.0 + FONT_PX * 1.4;`).
2. Change `use crate::content::{self, BodyLine};` to `use crate::content::{self, BodyLine, FONT_PX, HEADER, LINE_STEP};`
3. In `build_body`, replace the inline leaf predicate

```rust
    let is_leaf_item = node.byte_range.is_some()
        && node.children.is_empty()
        && !matches!(node.id.kind, SymbolKind::File | SymbolKind::Folder);
    if rung == Rung::Full && is_leaf_item {
```

with

```rust
    if rung == Rung::Full && content::is_leaf_item(node) {
```

(The treemap tests import `HEADER`/`LINE_STEP` via `use super::{…}` — re-imports resolve, no test edits needed.)

- [ ] **Step 4: Verify**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings && cargo test -p outrider 2>&1 | grep "^test result"`
Expected: clippy clean; all tests pass (2 new).

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/content.rs crates/outrider/src/treemap.rs
git commit -m "feat(app): shared text constants, is_leaf_item, natural_px in content.rs"
```

---

### Task 2: theme.rs — depth fills, churn stripe color, code background

**Files:**
- Modify: `crates/outrider/src/theme.rs`
- Modify: `crates/outrider/src/treemap.rs` (one mechanical rename at the call site)

**Interfaces:**
- Consumes: nothing new.
- Produces: `theme::depth_fill(level: u8) -> u32`, `theme::churn_heat(churn: f32) -> u32` (rename of `churn_fill`), `theme::CODE_BG: u32`, `theme::STRIPE_W: f32`, `theme::CORNER_RADIUS: f32`. Task 6 uses all of these.

- [ ] **Step 1: Write the failing tests** — in `theme.rs` `mod tests`, rename `churn_fill` → `churn_heat` inside the two existing churn tests, and append:

```rust
    #[test]
    fn depth_fill_ramp_endpoints_midpoint_and_clamp() {
        assert_eq!(depth_fill(0), 0x17171B);
        assert_eq!(depth_fill(8), 0x3C3C46);
        assert_eq!(depth_fill(12), 0x3C3C46); // clamps at level 8
        // t = 0.5 per channel: r,g 23+18.5→42 (0x2a); b 27+21.5→49 (0x31)
        assert_eq!(depth_fill(4), 0x2a2a31);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider theme:: 2>&1 | tail -5`
Expected: FAIL to compile — `depth_fill`, `churn_heat` not found.

- [ ] **Step 3: Implement** — in `theme.rs`:
1. Rename `pub fn churn_fill` → `pub fn churn_heat` and update its doc comment to `/// Churn heat for the left-edge stripe: neutral gray -> red, linear per-channel in sRGB.`
2. Add after the color constants at the top:

```rust
/// Depth-shaded box fill: darker outside, lighter inside, clamped at 8.
const DEPTH_FILL_0: u32 = 0x17171B;
const DEPTH_FILL_8: u32 = 0x3C3C46;
/// Editor background for boxes that render code (Full leaf items).
pub const CODE_BG: u32 = 0x101014;
/// Churn heat stripe width at the box's left edge.
pub const STRIPE_W: f32 = 3.0;
/// Corner radius for all box quads.
pub const CORNER_RADIUS: f32 = 4.0;
```

3. Add after `churn_heat`:

```rust
/// Box background by nesting depth (containment read): linear ramp,
/// clamped at level 8.
pub fn depth_fill(level: u8) -> u32 {
    lerp_rgb(DEPTH_FILL_0, DEPTH_FILL_8, level.min(8) as f32 / 8.0)
}
```

4. In `treemap.rs`, change `let f = theme::churn_fill(item.node.churn);` to `let f = theme::churn_heat(item.node.churn);` (fill semantics change lands in Task 6).

- [ ] **Step 4: Verify**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings && cargo test -p outrider 2>&1 | grep "^test result"`
Expected: clippy clean; all pass (1 new).

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/theme.rs crates/outrider/src/treemap.rs
git commit -m "feat(app): depth-shaded fills, churn_heat rename, CODE_BG/stripe/radius constants"
```

---

### Task 3: world.rs — legibility-based Full in `rung_for`

**Files:**
- Modify: `crates/outrider/src/world.rs`

**Interfaces:**
- Consumes: `content::is_leaf_item`, `content::natural_px` (Task 1).
- Produces: `rung_for(px_h: f64, px_w: f64, natural_px: Option<f64>) -> Option<Rung>` — third arg is `Some(natural)` only for leaf items; `visible_nodes` passes it internally. Task 6 has no direct `rung_for` calls.

- [ ] **Step 1: Write the failing tests** — in `world.rs` tests, append to `rung_for_thresholds_and_downgrade` (after updating every existing call in that test to take `None` as a third argument):

```rust
        // Leaf legibility (spec 4c §6): Full as soon as the box fits the
        // content, even below FULL_PX
        assert_eq!(rung_for(100.0, 400.0, Some(90.0)), Some(Rung::Full));
        assert_eq!(rung_for(100.0, 400.0, None), Some(Rung::Card)); // container ladder
        assert_eq!(rung_for(100.0, 250.0, Some(90.0)), Some(Rung::Detail)); // width gate holds
        assert_eq!(rung_for(100.0, 59.0, Some(90.0)), Some(Rung::Dot)); // narrow gate holds
        assert_eq!(rung_for(80.0, 400.0, Some(90.0)), Some(Rung::Card)); // below content → ladder
        assert_eq!(rung_for(699.0, 400.0, Some(3000.0)), Some(Rung::Detail)); // long fn: FULL_PX cap
        assert_eq!(rung_for(700.0, 400.0, Some(3000.0)), Some(Rung::Full));
```

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider world:: 2>&1 | tail -5`
Expected: FAIL to compile — `rung_for` takes 2 arguments.

- [ ] **Step 3: Implement** — replace `rung_for` with:

```rust
/// Rung by pixel height, downgraded to Dot when the column is too narrow
/// for text (gutter strips) and from Full to Detail when too narrow for
/// code. Heights below MERGE_PX merge into the parent. For leaf items,
/// pass `natural_px`: the box is Full as soon as it fits its content
/// (capped at FULL_PX for long methods) — code appears when close enough,
/// no explicit dive required (spec 4c §6).
pub fn rung_for(px_h: f64, px_w: f64, natural_px: Option<f64>) -> Option<Rung> {
    let by_height = if px_h < MERGE_PX {
        return None;
    } else if px_h < LABEL_PX {
        Rung::Dot
    } else if px_h < CARD_PX {
        Rung::Label
    } else if px_h < DETAIL_PX {
        Rung::Card
    } else if px_h < FULL_PX {
        Rung::Detail
    } else {
        Rung::Full
    };
    let by_height = match natural_px {
        Some(n) if px_h >= n.min(FULL_PX) => Rung::Full,
        _ => by_height,
    };
    let rung = if px_w < LABEL_MIN_W { Rung::Dot } else { by_height };
    Some(if rung == Rung::Full && px_w < CODE_MIN_W { Rung::Detail } else { rung })
}
```

In `walk`, change the call site to:

```rust
    let natural = content::is_leaf_item(node).then(|| content::natural_px(node));
    let Some(rung) = rung_for(px_h, px_w, natural) else { return };
```

and add `use crate::content;` next to `use crate::camera::Camera;`. (Worked-example test nodes have `byte_range: None`, so all existing `visible_nodes` expectations are unchanged.)

- [ ] **Step 4: Verify**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings && cargo test -p outrider 2>&1 | grep "^test result"`
Expected: clippy clean; all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/world.rs
git commit -m "feat(app): legibility-based Full — leaf boxes show code once they fit it"
```

---

### Task 4: world.rs — nested containment rects

**Files:**
- Modify: `crates/outrider/src/world.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub const NEST_PAD: f64 = 6.0`; `DrawItem` gains `pub level: u8` and `pub label_w: f64` (own-column width; `px.w` becomes the extended containment width). Task 6 reads both new fields.

- [ ] **Step 1: Write the failing test** — append to `world.rs` tests:

```rust
    #[test]
    fn nested_rects_extend_over_descendants() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // zoomed-past scene: cols w = 36.19, 144.76, 579.05; stack right = 760
        let cam = Camera { center_y: 0.6875, zoom: 256000.0 / 7.0 };
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["", "b.rs", "g"]);
        // root (level 0) encloses g (level 2): right = 760 + 2·NEST_PAD
        assert!((items[0].px.w - 772.0).abs() < 1e-6, "{}", items[0].px.w);
        // b.rs (level 1): right = 760 + 1·NEST_PAD → w = 766 − 36.190…
        assert!((items[1].px.w - 729.8095238).abs() < 1e-6, "{}", items[1].px.w);
        // g is a leaf: keeps its own column width
        assert!((items[2].px.w - 579.0476190).abs() < 1e-6, "{}", items[2].px.w);
        // label_w stays the own-column width for text layout
        assert!((items[0].label_w - 36.1904762).abs() < 1e-6);
        assert!((items[1].label_w - 144.7619048).abs() < 1e-6);
        assert!((items[2].label_w - 579.0476190).abs() < 1e-6);
        assert_eq!((items[0].level, items[1].level, items[2].level), (0, 1, 2));
        // a parent whose children are all culled keeps its column edge:
        // the x-prune scene (40px viewport) draws root+b.rs only
        let cam = Camera { center_y: 0.6875, zoom: 1e9 };
        let items = visible_nodes(&tree, &layout, &cam, 40.0, 600.0);
        assert!((items[1].px.w - 24.0).abs() < 1e-6); // b.rs: g x-pruned
        assert!((items[0].px.w - 54.0).abs() < 1e-6); // root: 24+24 + NEST_PAD
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider world:: 2>&1 | tail -5`
Expected: FAIL to compile — no `label_w`/`level` fields on `DrawItem`.

- [ ] **Step 3: Implement**

1. Add near the other constants: 

```rust
/// Horizontal nesting margin: each ancestor's box extends this much
/// further right than its children's boxes.
pub const NEST_PAD: f64 = 6.0;
```

2. Extend `DrawItem`:

```rust
#[derive(Debug)]
pub struct DrawItem<'a> {
    pub node: &'a SymbolNode,
    /// Containment rect: extends rightward over visible descendants
    /// (+ NEST_PAD per level of depth difference). Overlaps ancestors.
    pub px: PxRect,
    /// The node's own column width — text lives in this left strip.
    pub label_w: f64,
    pub level: u8,
    pub rung: Rung,
    /// UNclipped screen-y of the box top (`px.y` is clipped to the viewport).
    pub top: f64,
}
```

3. Make `walk` return the deepest drawn level and patch the parent's width; replace the tail of `walk` (from `let y0 = …` to the end) with:

```rust
    let y0 = px_y.max(-2.0);
    let y1 = (px_y + px_h).min(vh + 2.0);
    let idx = out.len();
    out.push(DrawItem {
        node,
        px: PxRect { x: px_x, y: y0, w: px_w, h: y1 - y0 },
        label_w: px_w,
        level: depth,
        rung,
        top: px_y,
    });
    let mut deepest = depth;
    for child in &node.children {
        if let Some(d) = walk(child, layout, camera, cols, vw, vh, abs, out) {
            deepest = deepest.max(d);
        }
    }
    if deepest > depth {
        let dc = &cols[deepest as usize];
        out[idx].px.w = dc.x + dc.w + NEST_PAD * f64::from(deepest - depth) - px_x;
    }
    Some(deepest)
```

and change the signature/early-returns to match: `fn walk<'a>(…) -> Option<u8>` with every bare `return;` becoming `return None;` (three sites: missing layout node, merge rule, y-prune; plus the x-prune and the `cols.get` miss).

4. Update `visible_nodes`'s call: `walk(&tree.root, layout, camera, &cols, vw, vh, 0.0, &mut out);` (return value ignored — add `let _ = …` only if clippy complains; plain call is fine since `Option<u8>` is not `#[must_use]`).

5. Update `hit_test`'s doc comment:

```rust
/// Visible node containing the point. Rects nest (ancestors extend over
/// descendants), so take the last hit in DFS order — the deepest node.
```

6. Update the two existing tests the extension changes:
   - `culling_x_prune_stops_recursion`: `assert!((items[0].px.w - 24.0).abs() < 1e-6);` becomes `assert!((items[0].px.w - 54.0).abs() < 1e-6);` and add `assert!((items[0].label_w - 24.0).abs() < 1e-6);`.
   - `zoomed_past_ancestors_clip_and_compress`: root width assertion `(root.w - 36.1904762)` becomes `(root.w - 772.0)`; add `assert!((items[0].label_w - 36.1904762).abs() < 1e-6);`. The g assertions are unchanged (leaf).
   - `hit_test_picks_the_column_under_the_point`: `assert!(hit_test(&items, 400.0, 100.0).is_none());` becomes `assert_eq!(hit_test(&items, 400.0, 100.0).unwrap().node.name, "b.rs");` with comment `// above g's band, inside b.rs's extended rect`. The `(790.0, 300.0)` None case still holds (root right edge 772).

- [ ] **Step 4: Verify**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep "^test result"`
Expected: clippy clean; all pass. (`treemap.rs` compiles unchanged — it doesn't construct `DrawItem`s and doesn't read the new fields yet.)

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/world.rs
git commit -m "feat(app): nested containment rects — ancestors extend over visible descendants"
```

---

### Task 5: world.rs — `frame_leaf` natural-size framing

**Files:**
- Modify: `crates/outrider/src/world.rs`

**Interfaces:**
- Consumes: `content::natural_px` (Task 1), `camera::{Camera, END_FRACTION}`, `world_band`, `column_table`.
- Produces: `world::frame_leaf(node, layout, vw, vh, min_zoom, max_zoom) -> Option<Camera>`. Task 6 calls it from the key handler.

- [ ] **Step 1: Write the failing tests** — append to `world.rs` tests:

```rust
    /// worked example with byte ranges so f and g are leaf items
    fn leafy_example() -> SymbolTree {
        let mut t = worked_example();
        t.root.children[1].children[0].byte_range = Some(0..10); // f, measure 10
        t.root.children[1].children[1].byte_range = Some(10..20); // g, measure 1
        t
    }

    #[test]
    fn frame_leaf_natural_size_and_width_floor() {
        let tree = leafy_example();
        let layout = outrider_layout::layout(&tree);
        let (vw, vh) = (800.0, 600.0);
        // f: measure 10 → natural = 20.8 + 11·15.6 + 6 = 198.4; its cell
        // height at z_nat is ~198 ≈ PEAK_CELL_PX, so the column is already
        // code-wide: zoom lands exactly at natural height
        let f = &tree.root.children[1].children[0];
        let (fy, fh) = world_band(&f.id, &layout).unwrap();
        let cam = frame_leaf(f, &layout, vw, vh, 1e-9, 1e18).unwrap();
        assert!((cam.zoom * fh - 198.4).abs() < 1e-6); // box = natural px
        assert!((cam.center_y - (fy + fh / 2.0)).abs() < 1e-12);
        let cols = column_table(cam.zoom, vw, 2);
        assert!(cols[2].w >= CODE_MIN_W);
        // g: measure 1 → natural = 58; at that zoom the leaf column is
        // narrower than CODE_MIN_W, so the width floor zooms further in:
        // result is the smallest 1.25-step ≥ natural-height zoom that is
        // code-wide (and its 1.25-times-smaller neighbor is not)
        let g = &tree.root.children[1].children[1];
        let (gy, gh) = world_band(&g.id, &layout).unwrap();
        let z_nat = 58.0 / gh;
        let cam = frame_leaf(g, &layout, vw, vh, 1e-9, 1e18).unwrap();
        assert!(cam.zoom > z_nat);
        assert!((cam.center_y - (gy + gh / 2.0)).abs() < 1e-12);
        assert!(column_table(cam.zoom, vw, 2)[2].w >= CODE_MIN_W);
        assert!(column_table(cam.zoom / 1.25, vw, 2)[2].w < CODE_MIN_W);
        // cap: a viewport too short for natural height caps at END framing
        let cam = frame_leaf(g, &layout, vw, 60.0, 1e-9, 1e18).unwrap();
        assert!((cam.zoom - crate::camera::END_FRACTION * 60.0 / gh).abs() < 1e-9);
        // clamp: min_zoom wins over the search result
        let cam = frame_leaf(g, &layout, vw, vh, 1e9, 1e18).unwrap();
        assert!((cam.zoom - 1e9).abs() < 1.0);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider world:: 2>&1 | tail -5`
Expected: FAIL to compile — `frame_leaf` not found.

- [ ] **Step 3: Implement** — in `world.rs`:

1. Extract the max-level computation `visible_nodes` already does into a helper, and use it in both places:

```rust
/// Deepest level that exists in the layout (capped at MAX_DEPTH) — the
/// column-table normalization domain. Framing must use the same domain
/// as rendering or widths won't match.
fn tree_max_level(layout: &WorldLayout) -> usize {
    layout
        .nodes
        .values()
        .map(|nl| nl.cells.level as usize)
        .max()
        .unwrap_or(0)
        .min(MAX_DEPTH)
}
```

In `visible_nodes`, replace the inline `max_level` computation with `let cols = column_table(camera.zoom, vw, tree_max_level(layout));`.

2. Add (after `world_band`):

```rust
/// Camera framing a leaf item at its natural content height (spec 4c §5):
/// zoom starts at min(natural_px, END_FRACTION·vh) of box height and
/// steps up by 1.25× only as needed to make the leaf's column code-wide,
/// capped at END_FRACTION framing; the result is clamped like frame_band.
pub fn frame_leaf(
    node: &SymbolNode,
    layout: &WorldLayout,
    vw: f64,
    vh: f64,
    min_zoom: f64,
    max_zoom: f64,
) -> Option<Camera> {
    let (y, h) = world_band(&node.id, layout)?;
    let level = layout.nodes.get(&node.id)?.cells.level as usize;
    let max_level = tree_max_level(layout);
    let z_end = crate::camera::END_FRACTION * vh / h;
    let mut z = content::natural_px(node).min(crate::camera::END_FRACTION * vh) / h;
    while z < z_end && column_table(z, vw, max_level)[level].w < CODE_MIN_W {
        z = (z * 1.25).min(z_end);
    }
    Some(Camera { center_y: y + h / 2.0, zoom: z.clamp(min_zoom, max_zoom) })
}
```

- [ ] **Step 4: Verify**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep "^test result"`
Expected: clippy clean; all pass. If the f-case assertion fails because the column at `z_nat` is unexpectedly narrower than 300 (waterfill nuance), print `column_table(z_nat, 800.0, 2)` in the test, report the numbers in your report file, and adjust only the *expected zoom* line to `z_nat · 1.25^k` for the k the search actually needs — the minimality assertions (`≥ CODE_MIN_W` at result, `< CODE_MIN_W` one step below) are the contract and must stay.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/world.rs
git commit -m "feat(app): frame_leaf — natural-size framing with a code-width floor"
```

---

### Task 6: treemap.rs — wiring and paint (framing, fills, stripe, radius)

**Files:**
- Modify: `crates/outrider/src/treemap.rs`
- Modify: `docs/superpowers/specs/2026-07-08-phase-4a-structural-navigation-design.md` (supersession notes)

**Interfaces:**
- Consumes: everything produced by Tasks 1–5.
- Produces: end-user behavior only; no new API.

- [ ] **Step 1: Update the two `build_body` tests to the new signature** (they gain a `label_w` argument, value `400.0`, right after the `&px` argument):

```rust
        let body = build_body(&f, Rung::Detail, &px, 400.0, 0.0, 600.0, &mut mgr, &BTreeMap::new());
```

(and the equivalent three other `build_body(…)` calls in `build_body_positions_detail_lines` / `build_body_full_leaf_appends_windowed_code`).

- [ ] **Step 2: Run to verify failure**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider treemap:: 2>&1 | tail -5`
Expected: FAIL to compile — `build_body` takes 7 arguments.

- [ ] **Step 3: Implement** — all in `treemap.rs`:

1. **`build_body`**: add `label_w: f64` parameter after `px: &world::PxRect`; replace both `px.w as f32` truncation calls (`truncate_to_width(&text, px.w as f32, …)` and `code_line(&text, spans, px.w as f32, …)`) with `label_w as f32`. Add `#[allow(clippy::too_many_arguments)]` above the fn if clippy demands it.

2. **`TreemapView`**: delete the `step_fraction` field, its doc comment block, and its `new()` initializer line.

3. **`PaintItem`**: add fields `label_w: f32` and `stripe: Option<u32>`.

4. **`paint_items`**: replace the per-item block

```rust
            let f = theme::churn_heat(item.node.churn);
            let body = build_body(
                item.node,
                item.rung,
                &item.px,
                item.top,
                vh,
                &mut self.buffers,
                &self.file_symbols,
            );
            out.push(PaintItem {
                x: item.px.x as f32,
                y: item.px.y as f32,
                w: item.px.w as f32,
                h: item.px.h as f32,
                fill: f,
                border: theme::border_for(f),
                focused: item.node.id == focus_id,
                rung: item.rung,
                name: item.node.name.clone(),
                body,
            });
```

with

```rust
            let is_code = item.rung == Rung::Full && content::is_leaf_item(item.node);
            let fill = if is_code { theme::CODE_BG } else { theme::depth_fill(item.level) };
            let body = build_body(
                item.node,
                item.rung,
                &item.px,
                item.label_w,
                item.top,
                vh,
                &mut self.buffers,
                &self.file_symbols,
            );
            out.push(PaintItem {
                x: item.px.x as f32,
                y: item.px.y as f32,
                w: item.px.w as f32,
                h: item.px.h as f32,
                label_w: item.label_w as f32,
                fill,
                border: theme::border_for(fill),
                stripe: (item.node.churn > 0.0).then(|| theme::churn_heat(item.node.churn)),
                focused: item.node.id == focus_id,
                rung: item.rung,
                name: item.node.name.clone(),
                body,
            });
```

5. **Key handler**: the arrow/end/home target match becomes (the `vh`, `max_zoom`, `min_zoom`, `index`, `moved` lines above it are unchanged; add `let vw = f64::from(w.viewport_size().width);` next to the existing `vh` line):

```rust
                let target = match key {
                    "right" | "left" | "up" | "down" => {
                        if !moved {
                            return;
                        }
                        match index.node(&this.focus.current) {
                            Some(n) if content::is_leaf_item(n) => {
                                world::frame_leaf(n, &this.layout, vw, vh, min_zoom, max_zoom)
                            }
                            _ => world::world_band(&this.focus.current, &this.layout).map(
                                |(y, h)| {
                                    camera::frame_band(
                                        y,
                                        h,
                                        vh,
                                        camera::FOCUS_FRACTION,
                                        min_zoom,
                                        max_zoom,
                                    )
                                },
                            ),
                        }
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
```

(Note: `"end"` and `"home"` no longer touch a sticky fraction — the field is gone.)

6. **Paint closure**: replace the main quad call and add the stripe right after it:

```rust
                            window.paint_quad(quad(
                                b,
                                px(theme::CORNER_RADIUS),
                                rgb(item.fill),
                                px(bw),
                                rgb(bc),
                                BorderStyle::default(),
                            ));
                            if let Some(heat) = item.stripe {
                                let sb = Bounds::new(
                                    point(origin.x + px(item.x + 1.0), origin.y + px(item.y + 1.0)),
                                    size(px(theme::STRIPE_W), px((item.h - 2.0).max(0.0))),
                                );
                                window.paint_quad(quad(
                                    sb,
                                    px(0.),
                                    rgb(heat),
                                    px(0.),
                                    rgb(heat),
                                    BorderStyle::default(),
                                ));
                            }
```

7. **Text width**: in the paint closure, `truncate_to_width(&item.name, item.w, font_px)` becomes `truncate_to_width(&item.name, item.label_w, font_px)`.

8. **4a spec supersession** — in `docs/superpowers/specs/2026-07-08-phase-4a-structural-navigation-design.md`, replace the §3 FOCUS_FRACTION amendment sentence beginning `*Amended after the 4b exit gate (second pass):* the step fraction is **sticky**` (through the end of that bullet) with:

```
*Superseded by Phase 4c
(`2026-07-09-phase-4c-nested-rendering-design.md` §5):* arrow steps onto
leaf items use natural-size framing (`world::frame_leaf`); containers
frame at FOCUS_FRACTION; there is no sticky fraction.
```

and in §6, replace the key-handler bullet's description of `step_fraction` (the sentence from `where` through `resets the sticky fraction and`) so the bullet reads:

```
- Key handlers: Right/Left/Up/Down mutate `Focus` then tween to
  `world::frame_leaf(focus, …)` for leaf items or `frame_band(…,
  FOCUS_FRACTION, …)` for containers (superseded framing — Phase 4c §5);
  End tweens to `END_FRACTION` framing of the current focus (no focus
  change); Home tweens to `Camera::frame(root_world_height(), vh)`.
```

- [ ] **Step 4: Verify**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep "^test result"`
Expected: clippy clean; all pass. Then `cargo build -p outrider` — must succeed (GPUI paths compile; actual rendering is verified manually at the exit gate).

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/treemap.rs docs/superpowers/specs/2026-07-08-phase-4a-structural-navigation-design.md
git commit -m "feat(app): nested paint — depth fills, churn stripe, rounded corners, natural-size step framing"
```

---

## Manual exit gate (after all tasks — human)

`cargo run -p outrider -- .`: nesting read (files inside folders), stripe legibility, corner radius, natural-size method framing with visible siblings, code appearing without End, code on the editor background. Record the Bet #1 verdict in the ledger.
