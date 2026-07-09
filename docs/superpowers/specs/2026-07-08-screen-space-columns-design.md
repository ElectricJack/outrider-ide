# Screen-Space Column Widths — Design

- Date: 2026-07-08
- Amends: `docs/superpowers/specs/2026-07-08-phase-3-render-camera-design.md`
  (§5 world geometry x-axis, §6 camera, §7 culling). The y-axis model, the
  8× grid, rung heights, theme, and the render pipeline are unchanged.
- Status: replaces the world-space x-axis (`COLUMN_SHRINK`) shipped at the
  Phase 3 exit gate.

## 1. Problem

Under uniform zoom, zooming 8× descends one grid level. For column widths to
look the same after each descent, the width falloff per depth must equal the
height falloff (8×) — but 8× width falloff makes deep columns unreadably
narrow (the original exit-gate complaint), and any gentler constant
(`COLUMN_SHRINK = 0.5`) makes on-screen widths grow every zoom-in (the
follow-up complaint). No world-space constant satisfies both.

## 2. Decision

Decouple x from world space entirely. Column widths become **pixel** values
computed per frame as a pure function of `(depth, zoom)`; the y-axis stays
pure world × zoom, so the tested Phase 2 layout math is untouched.

Settled during brainstorming:

1. **Ancestor treatment:** zoomed-past ancestors compress into ~24 px colored
   gutter strips (churn fill + border, no text). Labels/polish are
   post-skeleton.
2. **Width function:** continuous peaked profile (below) — smooth under
   zoom, no popping, headless-testable.
3. **Horizontal camera:** none. The column stack is left-anchored at the
   viewport's left edge; x is fully determined by zoom. Drag pans y only.
   Phase 4 camera-follow drives x from the focused node anyway.

## 3. Width profile

Let `h_d = zoom · 8^-d` — the pixel height of one level-`d` cell.

```
w(h) = 3·h                                    if h ≤ MAX_COLUMN_PX / 3
     = max(GUTTER_PX, MAX_COLUMN_PX² / (3·h)) otherwise
```

- `MAX_COLUMN_PX = 400`, `GUTTER_PX = 24` — tunable constants in `world.rs`
  (revisit at the exit gate, esp. `MAX_COLUMN_PX` on wide monitors).
- **Rising side** is today's 3:1 cell aspect: approaching columns grow
  naturally with zoom.
- **Peak** at `h = MAX_COLUMN_PX / 3 ≈ 133 px` — cells comfortably in Card
  rung when their column is widest.
- **Decay side** falls off as 1/h, floored at the gutter. A passed column
  shrinks from 400 px to 24 px over ~1.2 zoom octaves.

Both sides move 8× per octave, so the profile is **self-similar**: the width
table at zoom `8z` equals the table at zoom `z` shifted one depth right.
Widths are bounded — at any zoom, roughly: ancestors at ≤ 24 px each, one or
two columns near the peak, then a 3·h tail shrinking 8× per depth (its sum
converges). Total stack width stays in the ~1.3 kpx worst case; deep sliver
columns may clip on narrow windows, accepted for the skeleton.

Column x is the prefix sum: `x_d = Σ_{d' < d} w(h_{d'})`, computed once per
frame into a per-depth table (depth ≤ ~20 entries). Screen x = `x_d`
directly — no offset, no camera term.

One visible consequence, accepted by design: at home view the root column
sits on the decay side (~90 px), and depth 1 gets the widest column. The
peak follows whichever level is at readable cell height.

## 4. Camera

```
Camera { center_y: f64, zoom: f64 }   // center_x deleted
```

| Op | Behavior |
|---|---|
| `pan(screen_dy)` | `center_y -= dy / zoom`. Horizontal drag ignored. |
| `zoom_about(cursor_y, factor)` | exponential; the world-y under the cursor stays fixed. No x term. |
| `home(vh)` | fit the root band's height (1.0 world unit) with 5% margin: `zoom = vh / 1.05`, `center_y = 0.5`. |

Zoom clamps unchanged (min = 0.5 × home-zoom; max = level-15 cell spans the
viewport height). The f64 floating-origin contract applies to y only; x is
bounded pixels and needs no such care.

## 5. Culling and rungs

The recursive walk keeps its shape; per-node:

1. `px_y`/`px_h` from the y pipeline as before; `px_x`/`px_w` from the
   per-frame width table.
2. **Prune** (unchanged): `px_h < MERGE_PX` merges the subtree;
   y-range off-screen prunes the subtree.
3. **X-prune**: `x_d > viewport width` prunes the subtree (deeper columns
   are always further right). The old off-screen-left skip-draw case is
   gone — nothing is ever left of x = 0.
4. **Rung** (one new clause): select by pixel height as today, then
   **downgrade to Dot when `px_w < LABEL_MIN_W = 60.0`** — gutter strips and
   sliver columns render fill + border only, never text.

Zoomed-past ancestors are drawn as their (viewport-clipped) quad in the
gutter column — no special-case node type; the width profile and the rung
downgrade produce the gutter appearance by themselves.

## 6. Testing

Headless, in `world.rs` / `camera.rs`:

1. **Width profile:** rising side `w = 3h`; `w(MAX/3) = MAX`; decay side
   values; gutter floor reached and held.
2. **Self-similarity:** width table at zoom `8z` = table at `z` shifted one
   depth (property-style over random zooms).
3. **Prefix sums:** widths positive, x non-decreasing (deep columns are
   below f64 resolution of the running sum — strictness is not achievable);
   total bounded.
4. **Rung downgrade:** tall-but-narrow → Dot; tall-and-wide → by height.
5. **`zoom_about` y-invariant:** world-y under the cursor fixed
   (property-style).
6. **Home round-trip:** root band fits `vh` with the 5% margin.
7. **Worked-example culling** re-derived at hand-picked zooms (the Phase 2
   worked example: `b.rs::g` abs cell 44).

Feel — smooth ramping, gutter appearance, no popping — is verified manually
at the exit gate.

## 7. Exit gate

Zoom from home into a deep symbol in Outrider's own repo: no column ever
grows past `MAX_COLUMN_PX`; passed ancestors compress to gutters; widths
change smoothly with no popping; y behavior identical to before.
