# Phase 4a: Structural Navigation + Camera-Follow — Design

- Date: 2026-07-08
- Parent: `docs/superpowers/specs/2026-07-05-outrider-walking-skeleton-design.md`
  §7.1 (focus rows) and the Phase 4 entry of
  `docs/superpowers/plans/2026-07-08-walking-skeleton-roadmap.md`.
- Scope split: Phase 4 is delivered as two sub-projects. **4a (this spec):**
  focus model + camera-follow — the motion half of Bet #1. **4b (separate
  spec):** Detail/Full rungs, rope materialization, anchors, highlighting.
- Builds on: screen-space columns
  (`2026-07-08-screen-space-columns-design.md`) — camera is
  `{ center_y, zoom }`; x is fully determined by zoom, and a focus node's
  ancestors are always the columns to its left.

## 1. Goal

Keyboard-primary navigation over the treemap: arrow keys step focus through
the structure, and the camera follows with a short eased animation. This is
the half of Bet #1 ("moving through a place, not slides changing") that can
be felt before any Detail/Full content exists.

## 2. Focus model

New GPUI-free module `crates/outrider/src/focus.rs`.

```rust
pub struct TreeIndex<'a> {
    // built once from &SymbolTree; SymbolId is Ord
    nodes:   BTreeMap<&'a SymbolId, &'a SymbolNode>,
    parents: BTreeMap<&'a SymbolId, &'a SymbolId>, // root absent
}

pub struct Focus {
    pub current: SymbolId,
    last_child: BTreeMap<SymbolId, SymbolId>,
}
// Amended after the 4b exit gate: the history stack is gone — Left moves
// to the structural parent instead of popping history.
```

Initial focus = root. Stepping semantics (children are already name-sorted
by `outrider-index`):

| Input | Behavior |
|---|---|
| **Right** | Step into `last_child[current]` if it is still a child of `current`, else the first child. Leaf → no-op. |
| **Up / Down** | Cycle name-ordered siblings, wrapping at the ends. Root → no-op. |
| **Left** | Move to the structural parent. Root → no-op. *(Amended after the 4b exit gate — was history-pop.)* |
| **Click** | Hit-test and set focus. Camera does **not** move (spec §7.1). |
| **Tab** | Disabled — no handler. |

- Whenever focus lands on node `N` with parent `P`, record
  `last_child[P] = N` — Right remembers the last-visited child no matter
  how you left it (Right, Up/Down, or click).
- Focus never becomes invalid in 4a: the tree is immutable at runtime until
  Phase 6 (live reload).

Click hit-testing: re-run `world::visible_nodes` at the click's viewport
size and take the item whose `PxRect` contains the cursor. Columns are
horizontally disjoint, so at most one item contains any point (take the
last hit for robustness). Recomputing the cull walk on click is stateless
and avoids storing borrowed `DrawItem`s in the view.

## 3. World band and framing

`world.rs` gains the y-band composition (it owns world math):

```rust
/// Absolute world-y band of a node: walk root→node accumulating
/// abs = abs·8 + start (same composition as the render walk), then
/// y = abs · 8^-level, h = len · 8^-level.
pub fn world_band(id: &SymbolId, index: &TreeIndex, layout: &WorldLayout)
    -> Option<(f64, f64)>  // (y, h); None if id unknown
```

`camera.rs` gains pure framing:

```rust
/// Camera whose viewport shows `band` at `fraction` of the viewport
/// height, centered. zoom clamped to [min_zoom, max_zoom]; the clamp may
/// prevent exact framing (accepted).
pub fn frame_band(y: f64, h: f64, vh: f64, fraction: f64,
                  min_zoom: f64, max_zoom: f64) -> Camera
// zoom = (fraction · vh / h).clamp(min_zoom, max_zoom)
// center_y = y + h / 2
```

- `FOCUS_FRACTION = 0.5` — arrow steps land the focus band at half the
  viewport height (Card territory, siblings and parent visible around it —
  "frame focus plus its parent" in the column model, where ancestors are
  always on-screen as the columns to the left). *Amended after the 4b exit
  gate (second pass):* the step fraction is **sticky** — End sets it to
  `END_FRACTION`, Home resets it to `FOCUS_FRACTION`, and every arrow step
  re-frames the new focus at the sticky fraction. Re-framing per node keeps
  the fidelity rung constant across different-sized siblings (a first-pass
  zoom floor did not: box height is zoom × node height, so a smaller
  sibling dropped below `FULL_PX`), and Left zooms out to frame the parent.
- `END_FRACTION = 0.95` — **End** frames the focus to fill the viewport.
- **Home** keeps its current framing (`Camera::frame`) but now animates.
- Constants live in `camera.rs`, tunable at the exit gate.

## 4. Camera tween

`camera.rs`, pure and clock-free:

```rust
pub struct CameraTween {
    pub from: Camera,
    pub to: Camera,
    pub duration: f64, // seconds; TWEEN_SECS = 0.25
}
impl CameraTween {
    /// t in seconds since start. Ease-in-out cubic. center_y interpolates
    /// linearly; zoom geometrically (log-space) so zoom speed feels
    /// uniform. t ≥ duration returns `to` exactly.
    pub fn sample(&self, t: f64) -> Camera;
    pub fn done(&self, t: f64) -> bool;
}
```

- Ease: `e(t) = if t < ½ { 4t³ } else { 1 − (−2t+2)³ / 2 }` on normalized t.
- **Retargeting:** a focus/camera key while a tween is live starts a new
  tween whose `from` is the **current sampled camera** — motion is
  continuous, never restarted from the old origin (spec: interruptible,
  ~250 ms).
- **Mouse cancels:** drag or wheel drops any live tween and applies the
  manual camera op from the current sampled state (keyboard is
  snapped-to-structure, mouse is free — parent §7.7).

The view (`treemap.rs`) owns `Option<(CameraTween, Instant)>`; while it is
`Some`, each render samples the tween into the camera, requests another
animation frame, and clears it when done.

## 5. Rendering

- `theme.rs` gains `FOCUS_BORDER` (accent color, clearly distinct from the
  churn fills and their computed borders).
- The focused node's quad, when it survives culling, is drawn with the
  accent border (slightly thicker than the standard 1px). When the focused
  node is culled or merged (mouse zoomed far away), nothing extra is drawn —
  focus remains valid and the next arrow key both steps and re-frames it.
- No other visual changes; rungs and fills are untouched.

## 6. Input wiring (`treemap.rs`)

- Key handlers: Right/Left/Up/Down mutate `Focus` then tween to
  `frame_band(world_band(focus), vh, step_fraction, …)`, where
  `step_fraction` is the view's sticky fraction (FOCUS_FRACTION by
  default); End sets the sticky fraction to `END_FRACTION` and tweens to
  that framing of the current focus (no focus change); Home resets the
  sticky fraction and tweens to `Camera::frame(root_world_height(), vh)`.
- Click: hit-test → focus change only.
- Zoom clamps stay as computed today (min = 0.5 × home zoom,
  max = vh · 8¹⁵); framing targets are clamped through the same values.

## 7. Testing

Headless, in `focus.rs` / `camera.rs` / `world.rs`, on the Phase 2 worked
example (root / a.rs / b.rs{f, g}):

1. **Stepping:** Right from root → a.rs (first child); Right at leaf no-op;
   Down/Up cycle and wrap (a.rs ↔ b.rs); root Up/Down no-op.
2. **Last-visited:** Right → a.rs, Down → b.rs (records
   `last_child[root] = b.rs`), click root (click never touches the
   clicked node's own `last_child` entry), Right → **b.rs**, not a.rs.
   Note the landing rule means any step onto a child updates its parent's
   `last_child` — "last visited" is literally the child most recently
   occupied, however you got there.
3. **Left:** moves to the structural parent; root no-op; Left-then-Right
   returns to the child you came from (via `last_child`).
4. **world_band:** `b.rs::g` → y = 44/64, h = 1/64 (abs cell 44, the
   worked example); root → (0, 1).
5. **frame_band:** band (0.6875, 0.015625), vh = 600, fraction 0.5 →
   zoom = 300/0.015625 = 19200, center_y = 0.6953125; clamped when the
   target exceeds max_zoom.
6. **Tween:** sample(0) = from, sample(duration) = to exactly; eased
   midpoint between endpoints and monotonic in t; zoom interpolates
   geometrically (sample(½·duration).zoom = √(z₀·z₁) for symmetric ease);
   done() flips at duration.
7. **Retarget continuity:** starting a new tween from a mid-flight sample
   produces no camera jump (from == sampled camera by construction —
   asserted at the wiring seam by unit test on the helper that builds it).
8. **Hit-test:** click point inside g's rect at the gutter-zoom worked
   example returns g (deepest), not b.rs or root.

Feel — eased motion, interruption, "place vs slideshow" — is manual.

## 8. Exit gate (Bet #1, informal)

Arrow-step through Outrider's own repo with camera-follow; End and Home
behave; mouse still free and cancels animation cleanly. Then the informal
Bet #1 check (parent §8.3 question 1): does arrow-stepping read as *moving
through a place* or as *slides changing*? Record the read in the ledger —
a "slideshow" verdict is a design finding to surface before Phase 4b/5
investment, not a failure to hide.

## 9. Out of scope (Phase 4b or later)

Detail/Full rungs, rope/anchor materialization, tree-sitter highlighting,
Enter/Esc descend transition, tree-history, Tab/call-graph, any focus
persistence across runs.
