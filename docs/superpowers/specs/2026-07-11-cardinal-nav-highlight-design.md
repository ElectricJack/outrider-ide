# Beam-Cast Arrow Navigation and Neighbor Highlights

**Date:** 2026-07-11
**Status:** Approved

## Problem

`spatial_step` (`crates/outrider/src/focus.rs:119`) scores every candidate
in the direction's half-plane by center distance `primary + 2·ortho`. A
box directly below the focus but 1px to the left qualifies for **Left**
with `primary = 1`, and its score can beat the true left neighbor whose
primary distance is large — pressing Left sometimes moves down. There is
also no visual cue for where the four arrow keys will land.

## Design

### 1. Beam-cast scoring (`focus.rs::spatial_step`)

Signature, eligibility (leaf mode: all leaf items at any depth; otherwise
same tree depth), and the no-wrap contract are unchanged. The
qualification and scoring change to a beam cast — slide the focused rect
in `dir` and see what it hits:

- **Beam overlap.** The candidate's orthogonal span must strictly overlap
  the focused rect's span. For Left/Right:
  `cand.y < cur.y + cur.h && cand.y + cand.h > cur.y`. For Up/Down the
  same on x. Corner-touching (equality) does not count.
- **Beyond the leading edge.** Edge-to-edge distance in `dir` must be
  ≥ 0. For Left: `primary = cur.x − (cand.x + cand.w)`; Right:
  `cand.x − (cur.x + cur.w)`; Up: `cur.y − (cand.y + cand.h)`; Down:
  `cand.y − (cur.y + cur.h)`. Shared edges (`primary == 0`) qualify.

Among qualifiers, lowest `primary` wins. Ties break by orthogonal center
misalignment `|cand_center − cur_center|` (y-centers for Left/Right,
x-centers for Up/Down), then `SymbolId` ordering for determinism.

**No fallback.** No qualifier → `None` → the key is dead and that
direction shows no highlight. (Requested behavior: a raycast that hits
nothing does nothing.)

The doc comment on `spatial_step` is rewritten to describe beam-cast
semantics.

### 2. Neighbor cache (`treemap.rs`)

`TreemapView` gains:

```rust
neighbors: Option<(SymbolId, [Option<SymbolId>; 4])>
```

In `paint_items`, after `focus_id` is read: if `self.neighbors` is `None`
or its key ≠ `focus_id`, recompute all four via `spatial_step` (Dir order:
Left, Right, Up, Down) and store. Lazy recompute keyed by focus id covers
every focus-mutation path (click, Enter/Esc, arrows, initial focus) with
one code site; layout is immutable per session so there is no other
invalidation. Cache hit costs one comparison per frame.

### 3. Highlight paint (`treemap.rs`, `theme.rs`)

- `PaintItem` gains `neighbor: bool`, set in `paint_items` where `focused`
  is set: true iff the item's id equals any `Some` entry of the cached
  neighbor array (and the item is not the focused one).
- `theme.rs` gains:

```rust
pub fn neighbor_border(border: u32) -> u32 {
    lerp_rgb(FOCUS_BORDER, border, 0.5)
}
```

- Canvas border resolution (currently `treemap.rs:728`) becomes:
  focused → `(2.0, FOCUS_BORDER)`; else neighbor →
  `(1.0, theme::neighbor_border(item.border))`; else
  `(1.0, item.border)`.

Because the highlight is literally the cached `spatial_step` results, the
four highlighted boxes are exactly where the arrows go.

## Testing

Unit tests in `focus.rs`:

- Off-beam candidate rejected: nearest-by-center box with no span overlap
  loses to a farther overlapping box (the reported Left-goes-down bug).
- Dead key: no span-overlapping candidate in `dir` → `None`.
- Edge distance wins: nearer edge beats farther edge among overlapping
  candidates, regardless of center distances.
- Misalignment tiebreak: equal `primary`, better-centered candidate wins.
- Shared edge qualifies: `primary == 0` candidate is reachable.
- Leaf mode still crosses depths and skips non-leaves (existing test kept,
  geometry adjusted if needed).
- `spatial_step_penalizes_orthogonal_offset` is rewritten to the new
  semantics; `spatial_step_crosses_parent_boundaries_at_same_depth` keeps
  passing (its geometry uses aligned rows/columns, which beam-cast also
  resolves).

Unit test in `theme.rs`: `neighbor_border` output differs from both
`FOCUS_BORDER` and the input border (a genuine blend).

Manual: run `cargo run -p outrider -- .`, verify the four highlights track
focus, arrows land exactly on highlighted boxes, and directions without a
highlight are dead keys.

## Trade-off

Strict beam overlap means sparse or heavily staggered layouts can have
dead directions where the old scoring would have jumped diagonally.
Accepted: predictability wins, and the highlight makes dead directions
visible instead of surprising.
