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

Revised post-exit-gate: the original raw profile (`w = 3h` rising, `1/h`
decay, both 8×/octave) made the total stack width dip mid-octave — as the
big column shrank, the incoming one didn't grow fast enough — and 8×
between adjacent columns made the tail vanish too fast. Widths are now
**normalized weights** with a gentler falloff.

Let `h_d = zoom · 8^-d` — the pixel height of one level-`d` cell. Each
depth gets a dimensionless weight, peaked where cells are most readable:

```
u(h) = (h / PEAK_CELL_PX)^α    if h ≤ PEAK_CELL_PX      (rising side)
     = (PEAK_CELL_PX / h)^α    otherwise                (decay side)

α = log_8(WIDTH_RATIO) = ⅔
```

- `PEAK_CELL_PX = 200`, `WIDTH_RATIO = 4`, `STACK_FRACTION = 0.95`,
  `GUTTER_PX = 24` — tunable constants in `world.rs`.
- One depth step changes `h` by 8× but weight by only `WIDTH_RATIO` (4×),
  so the peak's neighbor columns stay comparable instead of vanishing.
- **Normalization:** per frame, weights over depths `0..=max_level` (the
  tree's actual depth, capped at MAX_DEPTH) are scaled so the stack sums to
  `STACK_FRACTION · viewport_width` — total width is constant under zoom,
  no mid-octave breathing.
- **Gutter floor:** decay-side (zoomed-past) columns are floored at
  `GUTTER_PX`; flooring re-scales the rest (waterfill, continuous at the
  boundary so widths never pop). The rising tail has no floor. With 4×
  falloff a passed column compresses gradually over ~2.4 octaves.

The weight profile still moves one depth per 8× zoom, so it is
**self-similar**: adjacent-column width *ratios* at zoom `8z` equal those
at zoom `z` shifted one depth right (absolute widths shift slightly as
accumulating gutters eat budget). On very deep zooms accumulated gutters
(24 px per passed level) can exceed the target on narrow windows — the
free columns squeeze, and x-pruning clips the rest; accepted for the
skeleton.

Column x is the prefix sum: `x_d = Σ_{d' < d} w_{d'}`, computed once per
frame into a per-depth table. Screen x = `x_d` directly — no offset, no
camera term.

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

1. **Weight profile:** peak weight 1 at `PEAK_CELL_PX`; one depth step
   (8× in h) is `WIDTH_RATIO`× in weight on both sides; symmetric in log-h.
2. **Normalization:** total stack width = `STACK_FRACTION · vw` at every
   zoom; gutter floor reached exactly on far-decay columns while nearer
   ancestors stay free.
3. **Self-similarity:** adjacent-width ratios at zoom `8z` = ratios at `z`
   shifted one depth (free columns only; gutters shift the budget).
4. **Prefix sums:** widths positive, x = running sum.
5. **Rung downgrade:** tall-but-narrow → Dot; tall-and-wide → by height.
6. **`zoom_about` y-invariant:** world-y under the cursor fixed
   (property-style).
7. **Home round-trip:** root band fits `vh` with the 5% margin.
8. **Worked-example culling** re-derived at hand-picked zooms (the Phase 2
   worked example: `b.rs::g` abs cell 44).

Feel — smooth ramping, gutter appearance, no popping — is verified manually
at the exit gate.

## 7. Exit gate

Zoom from home into a deep symbol in Outrider's own repo: the stack fills
`STACK_FRACTION` of the window at every zoom with no mid-octave breathing;
passed ancestors compress gradually to gutters; widths change smoothly with
no popping; y behavior identical to before.
