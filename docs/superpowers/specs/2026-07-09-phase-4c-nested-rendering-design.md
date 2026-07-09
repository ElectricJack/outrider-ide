# Phase 4c: Nested Rendering + Natural-Size Framing — Design

- Date: 2026-07-09
- Parent: `2026-07-05-outrider-walking-skeleton-design.md`; revises the
  rendering of `2026-07-08-screen-space-columns-design.md` and the framing
  of `2026-07-08-phase-4a-structural-navigation-design.md` (as amended).
- Motivation: the Phase 4b exit gate. Four findings from manual passes:
  1. End framed a 4-line method into a 760px box — mostly empty. Framing
     should be sized to content, with siblings visible around it.
  2. Code never appears without an explicit dive: the Full rung is gated
     at an absolute 700px, but a 3-line method's code fits in ~80px.
  3. Solid churn-red fills are hostile as a code background and carry the
     whole visual load; and columns read as *beside* their parent, not
     *inside* it — containment is invisible on the x-axis.
  4. Square corners; requested small corner rounding.

## 1. Goal

Boxes visibly nest inside their ancestors; focusing a method frames it at
its natural content size with code visible whenever the box is big enough
to hold it; churn becomes a compact left-edge heat stripe on neutral
depth-shaded backgrounds; corners round slightly.

## 2. Nested containment rendering (render-level, world math untouched)

The world/column model is unchanged — x is still fully determined by zoom
via `column_table`, y by the world bands. Only the drawn rectangle of each
item changes:

- A node's rect starts at its own column's left edge (as today) and
  extends **rightward to the right edge of the deepest column occupied by
  its visible descendants, plus `NEST_PAD` per level of depth
  difference**. A node with no visible descendants keeps its own column's
  right edge.
  - `NEST_PAD = 6.0` px. Each ancestor extends `NEST_PAD` further right
    than its children, giving an even nesting margin.
- Paint order is the existing DFS order (parents before children), so
  children draw on top of the parent's fill — containment reads as nested
  boxes. The parent's label/meta stays in its left strip (its own column
  width), exactly where it renders today.
- `visible_nodes` computes the extension during its walk: the recursion
  returns the deepest drawn level in each subtree; the parent widens its
  `PxRect` accordingly. `DrawItem` is otherwise unchanged.
- Hit-testing: rects now overlap ancestors, but `hit_test` already takes
  the **last** (deepest, DFS order) containing item — deepest-wins is
  preserved. Update its doc comment (columns are no longer disjoint).
- Zoomed-past gutter ancestors extend over nearly the whole viewport and
  paint first — their fills become the backdrop of everything inside
  them, which is the correct containment read.

## 3. Palette: depth-shaded fills, churn stripe, code background

`theme.rs` changes; all values tunable at the exit gate:

- **Depth fill**: `depth_fill(level) -> u32` — linear ramp from `0x17171B`
  (level 0) to `0x3C3C46` (level ≥ 8), interpolated per channel at
  `min(level, 8) / 8`. Replaces `churn_fill` as the box background.
  Borders stay 1px `border_for(fill)` (existing lighten rule); the focus
  border stays `FOCUS_BORDER` at 2px.
- **Churn stripe**: churn moves to a vertical heat stripe at the box's
  left edge — `STRIPE_W = 3.0` px wide, inset 1px from the border, full
  box height, colored by the existing churn ramp (rename `churn_fill` →
  `churn_heat`). Zero-churn nodes draw no stripe. The stripe survives
  every rung down to Dot.
- **Code background**: `CODE_BG = 0x101014`. Leaf items at Full (the
  boxes that render code) fill with `CODE_BG` instead of `depth_fill`.
  Containers at Full (file inventories) keep the depth fill.

## 4. Rounded corners

All box quads get `CORNER_RADIUS = 4.0` px (the `px(0.)` radius argument
in the paint call today). The churn stripe stays square; at 3px wide
under a 4px corner the overlap is negligible (exit-gate judgment call).

## 5. Natural-size framing (replaces the sticky step fraction)

The sticky `step_fraction` from the 4b tuning pass is **removed**.

- `content.rs` gains the shared constants (moved from `treemap.rs`:
  `FONT_PX = 12.0`, `LINE_STEP = FONT_PX * 1.3`, `HEADER = 4.0 +
  FONT_PX * 1.4`) and:

  ```rust
  /// Natural pixel height of a leaf item's box: header + signature row +
  /// one row per code line + bottom pad.
  pub fn natural_px(node: &SymbolNode) -> f64
  // = HEADER + (1 + node.measure as f64) * LINE_STEP + 6.0
  ```

  "Leaf item" everywhere below = the existing `build_body` predicate:
  `byte_range.is_some() && children.is_empty()` and kind is not
  File/Folder.

- **Arrow step onto a leaf item**: tween to the camera whose zoom is the
  smallest value ≥ the natural-height zoom at which the leaf's column is
  code-wide:
  - `z_nat = min(natural_px(node), END_FRACTION * vh) / world_h`
  - `z_end = END_FRACTION * vh / world_h`
  - Search: starting at `z_nat`, multiply by 1.25 until
    `column_table(z, vw, leaf_level)[leaf_level].w >= CODE_MIN_W` or
    `z >= z_end`; cap at `z_end`. (The width-starved long-method case —
    e.g. a 212-line fn — accepts Detail at `z_end`, as today.)
  - `center_y` = band center, as `frame_band`; the final zoom is clamped
    to `[min_zoom, max_zoom]` exactly as `frame_band` clamps (the clamp
    may prevent exact framing — accepted). New pure helper in `world.rs`
    (it owns the width math):

    ```rust
    /// Camera framing a leaf item at its natural content height,
    /// zoomed further in only as needed to make its column code-wide;
    /// capped at END_FRACTION framing.
    pub fn frame_leaf(node: &SymbolNode, layout: &WorldLayout,
                      vw: f64, vh: f64, min_zoom: f64, max_zoom: f64)
        -> Option<Camera>
    ```

- **Arrow step onto a container**: `frame_band` at `FOCUS_FRACTION`
  (the original 4a behavior).
- **End**: `frame_band` at `END_FRACTION` of the current focus — any
  node, no stickiness. **Home**: unchanged.

## 6. Legibility-based Full (leaf items)

`rung_for` gains the leaf's natural height: for leaf items,

- **Full** when `px_h >= min(natural_px(node), FULL_PX)` **and**
  `px_w >= CODE_MIN_W`;
- otherwise the existing height ladder (with the existing width
  downgrades).

Containers keep the existing ladder unchanged. Consequence: a small
method shows its code the moment its box can hold it — arrived at by
keyboard, mouse wheel, or drag — with no explicit dive. Signature:
`rung_for(px_h, px_w, natural_px: Option<f64>)`, `None` for containers
(callers other than `visible_nodes` pass `None`).

## 7. Module changes

- `world.rs`: rect extension in `visible_nodes` (+ deepest-level return),
  `rung_for` third parameter, `frame_leaf`, `hit_test` doc update.
- `content.rs`: constants move here; `natural_px`.
- `theme.rs`: `depth_fill`, `churn_heat` (rename), `CODE_BG`; delete the
  churn-fill-as-background path.
- `camera.rs`: unchanged (framing constants stay; `frame_band` stays).
- `treemap.rs`: key handler uses `frame_leaf` / `frame_band` per focus
  kind, sticky `step_fraction` field deleted; paint: depth/code fills,
  churn stripe quad, corner radius; constants imported from `content.rs`.

## 8. Testing

Headless, on the Phase 2 worked example plus targeted cases:

1. **Rect extension**: parent's right edge = deepest visible descendant
   column's right edge + `NEST_PAD × Δlevel`; a leaf keeps its column
   edge; a parent whose children are all culled keeps its column edge.
2. **Paint order / hit-test**: with overlapping rects, `hit_test` returns
   the deepest item containing the point (existing test updated for the
   new widths).
3. **natural_px**: exact arithmetic for a 3-line and a 200-line node.
4. **Legibility rung**: a leaf with `natural_px = 90` at `px_h = 100,
   px_w = 400` → Full (below FULL_PX but fits content); same box for a
   container → Card; leaf at `px_w = 250` → Detail (width gate); leaf
   with `natural_px = 3000` needs `px_h >= 700` (FULL_PX cap).
5. **frame_leaf**: a short method's zoom lands `natural_px` box height
   when its column is already wide enough; a case where the width floor
   forces zoom above `z_nat`; cap at `z_end`.
6. **depth_fill**: endpoints exact (`0x17171B`, `0x3C3C46`), level 4
   midpoint per-channel, level 12 clamps to the level-8 color.

Feel — nesting read, stripe legibility, corner radius, "code appears when
close enough" — is the manual exit gate (re-run of Bet #1).

## 9. Out of scope

True padded nesting in world space (option B — revisit only if the
render-level read fails the gate), churn percentile copy changes, font
scaling with zoom, Enter/Esc descend (Phase 5), any focus persistence.
