# Spatial Treemap Pivot — Design

- Date: 2026-07-09
- Parent: `2026-07-05-outrider-walking-skeleton-design.md`. Replaces the
  icicle/column layout model (`2026-07-08-screen-space-columns-design.md`,
  layout spec §6) wholesale. The 4b–4d content stack — fidelity rungs,
  rope/anchor materialization, highlighting, density, scaled code — is
  reused unchanged.
- Motivation: the left/right column system reads as depth-slices, not a
  place. The pivot makes the whole project visible spatially when zoomed
  out: a real 2D treemap packed bottom-up from leaf nodes.

## 1. Goal

Zoomed out, the whole repo is one spatial map of nested boxes; zooming in
smoothly reaches real code at natural size. Mouse is the primary
navigation; keyboard arrows step between spatial neighbors; Enter/Esc
move down/up the hierarchy with camera-follow.

## 2. World model: natural pixels

World units are **pixels at zoom 1.0**. A leaf's box at zoom 1.0 is
exactly its natural rendered size, so all the 4b–4d pixel math transfers
unchanged: `px = world · zoom`, `rung_for` thresholds keep their meaning,
and `code_scale(node, full_h)` degenerates to `zoom.clamp(7/12, 1.0)` for
leaves.

- **Leaf box** (any childless node): `w = PAGE_W = 480.0`,
  `h = header + (1 + measure) · line_step + bottom_pad` — the
  `content::natural_px` formula, so height ∝ lines (code-page shaped).
- **Container box**: its packed children's bounding box plus a header
  strip and padding (§3).

## 3. Shelf packing (`outrider-layout/src/pack.rs`)

New module; the old cell system (`arrange.rs`, `measure.rs`, `types.rs`)
is deleted at the end of the pivot.

```rust
pub struct Rect { pub x: f64, pub y: f64, pub w: f64, pub h: f64 }

pub struct PackConfig {
    pub page_w: f64,     // 480.0
    pub line_step: f64,  // content::LINE_STEP (15.6)
    pub header: f64,     // content::HEADER (20.8)
    pub bottom_pad: f64, // content::BOTTOM_PAD (6.0)
    pub gap: f64,        // 8.0 — world px between siblings, both axes
    pub aspect: f64,     // 1.6 — target container w/h
}

pub struct PackLayout { pub rects: BTreeMap<SymbolId, Rect> } // absolute

pub fn pack(tree: &SymbolTree, cfg: &PackConfig) -> PackLayout
```

Per container, children in `(name bytes, ordinal)` order (same order the
old arrange pass used):

1. Size each child first (bottom-up recursion): leaf formula or packed
   container size.
2. `target_w = max(widest child w, sqrt(Σ child area · aspect))`.
3. Fill shelves left→right; a child that would cross `target_w` starts a
   new shelf (never wrap an empty shelf). Shelf height = tallest child;
   `gap` separates children horizontally and shelves vertically.
4. Container size: `w = content_w + 2·gap`, `h = header + content_h +
   2·gap`; children sit at `(gap + x, header + gap + y)` relative to the
   container's origin.

Absolute rects come from a second walk composing offsets; the root sits
at (0, 0).

**Properties (tested):** deterministic; position-independent — a child
subtree's internal layout depends only on its own contents, so an edit
repacks only its ancestor chain and shifts (never reflows) siblings.

## 4. Camera goes 2D (`camera.rs`)

`Camera { center_x, center_y, zoom }`. Extensions of the existing 1D
API, same shapes:

- `world_to_screen(wx, wy, vw, vh)` / `screen_to_world(sx, sy, vw, vh)`.
- `pan(dx_px, dy_px)` — content follows the cursor, both axes.
- `zoom_about(sx, sy, vw, vh, factor, min, max)` — cursor-fixed 2D zoom.
- `fit(rect, vw, vh)` — Home: rect fits with 5% margin,
  `zoom = min(vw/w, vh/h) / 1.05`, centered.
- `frame_rect(rect, vw, vh, fraction, min, max)` — focus framing:
  `zoom = fraction · min(vw/w, vh/h)`, clamped; centered.
- `frame_page(rect, vw, vh, min, max)` — leaf framing:
  `zoom = min(1.0, END_FRACTION · min(vw/w, vh/h))` — never zooms past
  natural size.
- `CameraTween` interpolates `center_x` and `center_y` linearly, zoom
  geometrically; retarget continuity unchanged.
- Zoom clamps: `min_zoom = 0.5 · home zoom` (fit zoom);
  `MAX_ZOOM = 8.0` (constant — the old `vh · 8^15` was cell-math).

## 5. Rendering (`world.rs`, `treemap.rs`)

`visible_nodes(tree, &PackLayout, camera, vw, vh)` walks the tree
pre-order (painter's order):

- Screen rect = camera transform of the node's world rect. Children are
  strictly inside parents, so an off-screen node prunes its subtree; a
  node below `MERGE_PX` (4px, either axis via `rung_for`) prunes too.
- Rects are clipped to the viewport ±2px in **both** axes before f32.
  `DrawItem` keeps `top`/`full_h` (unclipped y/h — drives code scale and
  the code line window) and gains `left` (unclipped x — Full-leaf code
  moves with its box; the name row stays pinned at the clipped corner).
- `label_w` = unclipped box width (text truncates to the box, not the
  viewport).
- `rung_for(px_h, px_w, natural)` is untouched: containers climb the
  Dot→Full ladder by pixel size; leaves show scaled, clipped code per
  4d. `hit_test` (last hit in DFS order = deepest) is untouched.
- `build_body`, `PaintItem.body_font_px`, highlighting, churn stripes,
  focus border: unchanged except `PaintItem` gains `body_x` (unclipped).

Deleted from `world.rs`: the entire column stack (`column_table`,
`column_weight`, `column_scale`, `cell_px_height`, `ColPx`,
`STACK_FRACTION`, `PEAK_CELL_PX`, `WIDTH_RATIO`, `GUTTER_PX`,
`MAX_DEPTH`, `NEST_PAD`), `world_band`, `frame_leaf`, `tree_max_level`.

## 6. Navigation (map-first)

- **Mouse (primary):** drag pans both axes; wheel zooms about the
  cursor; click sets focus without moving the camera. All unchanged in
  spirit, extended to 2D.
- **Arrows:** spatial neighbors. `spatial_step(current, dir, rects,
  index)` picks among all nodes at the **same tree depth**: candidates
  whose center lies strictly in the arrow's direction, scored by
  `primary distance + 2 · |orthogonal offset|`, ties broken by
  `SymbolId`. No wrap; no candidate → no-op. Camera-follows (tween) to
  the target's framing.
- **Enter** = descend (last-visited child, else first — the existing
  `step_in`); **Esc** = parent (`step_out`); `step_sibling` is deleted.
- **Framing targets:** leaves → `frame_page`; containers →
  `frame_rect(…, FOCUS_FRACTION)`. **End** → `frame_rect(…,
  END_FRACTION)` of the focus. **Home** → `fit(root)`.
- `Focus`, `TreeIndex`, last-visited memory, click-does-not-move-camera:
  all unchanged.

## 7. Module changes

- `outrider-layout`: new `pack.rs`; `arrange.rs`/`measure.rs`/`types.rs`
  and their re-exports deleted (with their tests).
- `camera.rs`: 2D fields/methods; `MAX_ZOOM`; y-only methods deleted at
  switchover.
- `world.rs`: packed-rect walk; column machinery deleted.
- `focus.rs`: `spatial_step`; `step_sibling` deleted.
- `treemap.rs`: 2D pan/zoom, new key map, `body_x`; `main.rs`: builds
  `PackConfig` from `content` constants and calls `pack`.
- Untouched: `content.rs`, `buffers.rs`, `theme.rs`, `outrider-index`.

## 8. Testing

Headless, on the worked example root{a.rs(100), b.rs(40){f(10), g(1)}}
with the §3 config: exact packed rects (b.rs content wraps to two
shelves; root packs a.rs and b.rs side by side); determinism; sibling
stability under a subtree edit; 2D camera round-trips, cursor-fixed
zoom, fit/frame math; tween continuity with center_x; cull/clip walk
(subtree prune, ±2 clip, unclipped top/left/full_h); spatial stepping
(direction filter, orthogonal penalty, same-depth candidates, no-wrap);
Enter/Esc reuse of existing focus tests. Feel — "one place, whole
project" — is the manual exit gate (Bet #1 re-run, superseding 4d's
unrun gate).

## 9. Out of scope

Squarified/space-optimal packing, size-proportional container fonts,
minimap, animation of layout changes (live reload is Phase 6), call
graph, persistence.
