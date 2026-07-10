# Phase 4d: Density + Scaled, Clipped Code — Design

- Date: 2026-07-09
- Parent: `2026-07-05-outrider-walking-skeleton-design.md`; revises
  `2026-07-09-phase-4c-nested-rendering-design.md` §6 (legibility-based
  Full) and the layout gap of the walking-skeleton layout spec.
- Motivation: Phase 4c exit-gate findings. Three requests from the manual
  pass: siblings sit too far apart vertically; a leaf's code should stay
  visible (clipped) well below its natural size instead of collapsing to
  a signature summary; and code text should shrink with the box so it
  stays legible longer while zooming out.

## 1. Goal

Sibling boxes stack densely; a code-bearing leaf shows code — scaled
down to a 7px floor, then clipped — whenever the box can hold about
three legible rows; framing and all world x-math are untouched.

## 2. Sibling gap: 15% → 3% (`outrider-layout`)

`measure.rs::gap_cells` becomes `(len * 3).div_ceil(100)`. The gap
formula is used identically by the measure and arrange passes, so the
change is one constant; determinism and the position-independence
property (per-child slack, not pooled) are unchanged.

- Consequence: every cell address below the root shifts. The layout
  worked-example fixtures and every `world.rs` test that hard-codes band
  positions (`world_band`, the zoomed-past/x-prune scenes, `frame_leaf`
  cases) get recomputed expectations. Column tables and all x-math are
  gap-free and untouched.
- `gap_cells(1)` is still 1 (integer round-up): tiny nodes keep a
  minimum 1-cell gap.

## 3. Leaf code persists, clipped (`world.rs::rung_for`)

The 4c leaf legibility rule — Full when `px_h >= min(natural_px,
FULL_PX)` — is replaced by:

- **Full** when `px_h >= min(natural_px, LEAF_CODE_MIN_PX)` and
  `px_w >= CODE_MIN_W` (existing width downgrade unchanged), where

  ```rust
  /// Shortest leaf box that still shows code: header + three code rows
  /// at the floor font + bottom pad.
  pub const LEAF_CODE_MIN_PX: f64 =
      HEADER + 3.0 * LINE_STEP * MIN_CODE_SCALE + BOTTOM_PAD; // 54.1
  ```

- `FULL_PX` no longer participates in the leaf arm (54.1 < 700 always);
  it remains in the container ladder, which is unchanged.
- No `build_body` window changes are needed for clipping itself: the
  Full body already emits only rows that fit the box.
- A leaf whose natural height is below the threshold (measure 0,
  natural 42.4px) still needs only its natural height: the `min` keeps
  the smallest leaves appearing as soon as they fit.

## 4. Code font scales with box height (`content.rs`, `treemap.rs`)

- `content.rs` gains:

  ```rust
  /// Floor for scaled code text.
  pub const MIN_CODE_FONT_PX: f64 = 7.0;
  pub const MIN_CODE_SCALE: f64 = MIN_CODE_FONT_PX / FONT_PX; // 7/12

  /// Per-box text scale for a Full leaf: 1.0 when the box fits the whole
  /// method, shrinking with the box down to the floor, after which the
  /// window clips.
  pub fn code_scale(node: &SymbolNode, px_h: f64) -> f64 {
      (px_h / natural_px(node)).clamp(MIN_CODE_SCALE, 1.0)
  }
  ```

- **Scope of scaling**: only the Full-leaf body (signature row + code
  rows). The name/header row, Detail/Card summaries, and container
  inventories stay at 12px. `HEADER` stays a fixed 12px-row height, so
  the body still starts at the same y.
- `build_body`'s Full-leaf branch computes `scale = code_scale(node,
  px.h)` and uses `LINE_STEP * scale` as the row step everywhere in the
  signature/code window math (`code_y0`, `min_y`, `max_y`, row y), and
  `FONT_PX * scale` for the char-budget calls (`truncate_to_width`,
  `code_line`) — smaller text fits more characters per line.
- `PaintItem` gains `body_font_px: f32` (12.0 for everything except Full
  leaves, where it is `(FONT_PX * scale) as f32`). The paint closure
  shapes body rows at `px(item.body_font_px)` with line height
  `item.body_font_px * 1.3` instead of the fixed 12px values.
- `natural_px` stays defined at scale 1.0 and `frame_leaf` is untouched:
  arrow-step framing still targets the comfortable full-size box.

## 5. Module changes

- `outrider-layout/src/measure.rs`: gap constant; test updates.
- `outrider-layout` arrange/measure tests + `crates/outrider/src/world.rs`
  band-dependent test fixtures: recomputed expectations.
- `crates/outrider/src/content.rs`: `MIN_CODE_FONT_PX`, `MIN_CODE_SCALE`,
  `LEAF_CODE_MIN_PX`, `code_scale`.
- `crates/outrider/src/world.rs`: `rung_for` leaf arm uses
  `LEAF_CODE_MIN_PX`.
- `crates/outrider/src/treemap.rs`: `build_body` scaled step/font;
  `PaintItem.body_font_px`; paint closure uses it.
- `camera.rs`, `focus.rs`, column math: untouched.

## 6. Testing

Headless:

1. **gap_cells**: `(1, 4, 7, 20, 34, 100) → (1, 1, 1, 1, 2, 3)`.
2. **Recomputed fixtures**: layout worked example cell starts/lens;
   `world_band`; the zoomed-past and x-prune scenes; `frame_leaf` cases
   (exact values derived at plan time by running the new layout).
3. **Leaf rung boundary**: `(55, 400, Some(3000)) → Full`,
   `(54, 400, Some(3000)) → Label` (below LEAF_CODE_MIN_PX);
   `(43, 400, Some(42.4)) → Full`, `(42, 400, Some(42.4)) → Label`
   (a leaf smaller than the threshold needs only natural height); width gate
   `(100, 250, Some(90)) → Detail` unchanged.
4. **code_scale**: exact 1.0 at `px_h = natural_px` and above; exact
   `7/12` floor for tiny boxes; mid value `px_h = 0.8·natural` → 0.8.
5. **Scaled window**: `build_body` on a Full leaf at half natural height
   emits rows at the scaled step and stops at the box edge.

Feel — density, 7px legibility, "code everywhere while moving" — is the
manual exit gate (re-run of Bet #1).

## 7. Out of scope

Scaling the name row or container text, scaling `CODE_MIN_W` with the
font, gap changes driven by depth or kind, any framing changes
(`frame_leaf`, fractions, tween).
