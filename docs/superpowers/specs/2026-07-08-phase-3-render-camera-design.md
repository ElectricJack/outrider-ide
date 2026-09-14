# Phase 3 — Render + Camera Design

- Date: 2026-07-08
- Parent spec: `docs/superpowers/specs/2026-07-05-outrider-walking-skeleton-design.md` (§4, §6.1, §6.3, §7.1 camera rows, §7.2 Dot/Label/Card, §9 milestone 3)
- Roadmap: `docs/superpowers/plans/2026-07-08-walking-skeleton-roadmap.md` (Phase 3)
- Requires: Phases 0–2 (GPUI window under WSLg; `outrider-index`; `outrider-layout`)

## 1. Goal

The treemap of a real repo on screen: mouse pan, continuous wheel zoom, Home
frames root; Dot → Label → Card fidelity rungs driven by on-screen pixel
height; churn fill color. First join point of all three crates.

## 2. Scope

In scope: startup pipeline (index → layout → render, one-way), floating-origin
world composition, visibility culling via the cell grid, camera
(drag pan / wheel zoom / Home), rungs Dot/Label/Card, churn fill,
`lines_per_cell` tuning.

Out of scope (later phases): click-to-focus, arrow navigation, camera-follow
animation and easing, Detail/Full rungs, ropes and tree-sitter highlighting
(Phase 4); Enter/Esc descend (Phase 5); live reload (Phase 6). Tab disabled
per parent spec.

## 3. Decisions settled during brainstorming

1. **Repo selection:** CLI argument, defaulting to the current directory
   (`cargo run -p outrider -- <path>`), matching `outrider-dump`.
2. **Startup:** synchronous — index and layout complete before the window
   opens; progress goes to stderr (`indexing <path>…`). No async loading
   state this phase.
3. **Churn ramp:** neutral gray → red (option C): color appears only where
   code is hot.
4. **Render approach:** custom canvas-style GPUI element painting quads and
   shaped text directly (option 1) — not a retained `div` tree, not a hybrid.
   Rationale: thousands-of-quads scale on llvmpipe, direct camera transform,
   and Phases 4–5 (camera-follow, frozen layer) want ownership of the paint
   pass.

## 4. Structure

```
crates/outrider/src/
  main.rs      CLI arg → index_repo → layout → open window
  world.rs     pure math: geometry, composition, culling, rung selection  (NO gpui types)
  camera.rs    Camera + pan/zoom/home ops                                 (NO gpui types)
  theme.rs     churn ramp, borders, text colors, rung thresholds
  treemap.rs   canvas element: paint pass + mouse/key handlers (all gpui here)
```

The app holds `{SymbolTree, WorldLayout, Camera}` and mutates none of the
first two (parent §4 one-way flow). `SymbolTree` is retained because render
needs `name`/`churn`/`measure`/`kind`, which `NodeLayout` deliberately omits;
the paint pass walks the tree and joins to `NodeLayout` by `SymbolId`.

`world.rs` and `camera.rs` import no GPUI types. Every piece of math behind
the exit gate is headless-testable; only `treemap.rs` needs eyeballs.

## 5. World geometry

One level-0 cell is 1.0 world unit tall.

- `CELL_ASPECT = 3.0` — a column is 3× the height of its cell (an 80 px cell
  sits in a ~240 px column). Tunable constant in `world.rs`.
- `COLUMN_SHRINK = 0.5` — width falloff per depth, deliberately gentler than
  the 8× cell-height ratio so deeper columns stay readable (tuned at the
  Phase 3 exit gate; the original 8× width falloff made ancestor columns
  dominate the screen). Heights alone carry the grid.
- Column width at depth d: `w_d = CELL_ASPECT · COLUMN_SHRINK^d`.
- Column x-origin: `X_d = CELL_ASPECT · (1 − COLUMN_SHRINK^d) / (1 −
  COLUMN_SHRINK)`. Total world width converges to `CELL_ASPECT / (1 −
  COLUMN_SHRINK)`; "fully zoomed out" is a finite frame.
- Node world rect (all f64):
  - `x = X_depth`, `width = CELL_ASPECT · COLUMN_SHRINK^depth`
  - `y = abs_start / 8^depth`, `height = len / 8^depth`
  - `abs_start` composed alongside the tree walk by the layout recurrence
    `abs = parent_abs · 8 + start` (same rule as
    `WorldLayout::absolute_start`).

**Floating origin contract:** composition and camera subtraction happen in
f64; only the final screen-relative delta is cast to f32 pixels. f32 never
sees an absolute world coordinate. f64 holds exact integers to 2^53 — safe
past depth 17; a `debug_assert` guards the bound.

**`lines_per_cell` tuning** (parent §6.2 assigns this to milestone 3): the
initial values (Folder/File → 32, items → 4) may be adjusted once real repos
are on screen. The Phase 2 property tests consume `lines_per_cell()` and
survive tuning unchanged; the measure/arrange unit tests hardcode expected
cell counts and must have their constants updated in the same commit as any
tuning change.

## 6. Camera

`Camera { center: (f64, f64) /* world */, zoom: f64 /* px per world unit */ }`

| Op | Behavior |
|---|---|
| `pan(screen_delta)` | `center -= delta / zoom`. No inertia. |
| `zoom_about(cursor, factor)` | exponential wheel zoom; the world point under the cursor stays fixed |
| `home(viewport)` | frame the root band with 5% margin; instant (easing is Phase 4) |

Zoom clamp: min = 0.5 × home-zoom; max = where one level-15 cell spans the
viewport height. Window resize changes only the viewport; camera state is
untouched.

## 7. Culling and fidelity rungs

One recursive walk from the root per frame:

1. Compute the node's world rect → pixel rect through the camera.
2. **Prune** if the pixel rect misses the viewport, or if pixel height <
   `MERGE_PX = 4` — the subtree merges into the parent's tile and recursion
   stops (parent §7.2 "below 4px merge into parent tile").
3. Otherwise select the rung by pixel height and paint.

| Rung | Pixel height | Content |
|---|---|---|
| Dot | 4–20 | churn fill + border only |
| Label | 20–80 | truncated name + fill |
| Card | ≥80 | name + `47 · p96 · 312L` line (churn count · percentile · lines) |

Card is the ceiling this phase (parent §7.2's Detail/Full start at 250 px and
belong to Phase 4; heights ≥250 px still render Card). The 8× grid ratio
bounds the visible set: children are 8× shorter than their parent, so only
~2–3 depth levels sit above 4 px at any zoom — the grid is the spatial index
(parent §6.1).

No rung hysteresis: thresholds are deterministic functions of zoom, and zoom
is continuous, so "no flicker" is a property of input continuity (verified
manually at the exit gate).

## 8. Input

| Input | Effect |
|---|---|
| Mouse drag | pan |
| Wheel | continuous zoom about the cursor |
| Home | frame root, instant |

Trackpad pinch maps onto `zoom_about` if GPUI delivers pinch events under
WSLg — verify during implementation; not a gate item. No other input this
phase.

## 9. Theme (placeholder aesthetics)

- Background `#1a1a1c`.
- Fill: linear sRGB lerp from `#2a2a2e` (churn 0.0) to `#b03030` (churn 1.0).
- Border: fill lightened ~12%, 1 px.
- Text: `#d8d8d8` monospace, `…`-truncated to column width (Label); Card's
  second line in `#9a9a9a`.

The real visual language is post-skeleton; these constants live in `theme.rs`
and nothing else depends on their values.

## 10. Testing

Headless unit tests in `world.rs` / `camera.rs`:

1. **Worked-example composition:** the Phase 2 worked-example tree
   (`b.rs::g` abs cell 44) → exact f64 world rects.
2. **Rung thresholds:** boundary pixel heights (3.9 / 4 / 19.9 / 20 / 80) →
   expected rung.
3. **Culling:** off-viewport nodes excluded; sub-4px subtrees pruned exactly
   at the merge boundary.
4. **`zoom_about` invariant:** the world point under the cursor is fixed
   across the op (property-style over random cursors/factors).
5. **Home round-trip:** after `home()`, the composed root rect fits the
   viewport with the 5% margin.

Rendering itself is verified manually — this phase begins the manual-feel
territory the skeleton exists for.

## 11. Exit gate

Navigate Outrider's own repo by mouse; boxes never move unless data changes;
rungs switch by pixel height without flicker (roadmap Phase 3 gate).

**llvmpipe watch-item** (Phase 0 verdict): if pan/zoom stutters under CPU
rasterization, record the finding and revisit the Dozen driver or the
Windows-native fallback — do not silently accept jank, and do not
prematurely optimize before measuring.
