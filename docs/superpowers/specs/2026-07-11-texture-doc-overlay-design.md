# Texture-Tier Doc Overlay — Design

**Date:** 2026-07-11
**Status:** Approved (user: "looks good")

## Goal

At the zoom level where leaf boxes render blurred baked-texture code (the
"texture tier"), show a crisp, readable overlay: the item's pinned name row
plus its `///` doc description below it, white-on-dark, line-wrapped to fill
the box width. Leaves without a doc are unchanged.

## When it applies

- Leaf items only, at texture tier: on-screen body font
  `font = FONT_PX * (full_h / natural_px) < TEXT_FADE_HI` (i.e. whenever a
  texture quad is drawn).
- Only when the node has `doc: Some(_)` (populated from `///` blocks by
  outrider-index).
- **Opacity:** the overlay (backdrop panel + doc rows) uses the existing
  `tex_opacity` — fully opaque below `TEXT_FADE_LO`, fading out across the
  `TEXT_FADE_LO..TEXT_FADE_HI` band exactly as the texture does, so the
  overlay disappears as real code text fades in. The name row itself already
  exists at all tiers and keeps its current behavior.

## What it renders

Under the existing pinned 12px name row:

- Doc text in screen-space `FONT_PX` (12px) `TEXT_PRIMARY`, **not** scaled
  with the page — crisp at any zoom.
- **Reflow:** consecutive source doc lines are joined into paragraphs (a
  blank line is a paragraph break), then each paragraph is greedy
  word-wrapped to the inner width `px.w - 2 * BODY_PAD` using the existing
  char-budget heuristic (`(w_px - 12.0) / (font_px * 0.62)`). Words longer
  than the budget are hard-split. Trade-off (accepted): deliberate source
  line breaks are lost in favor of filling the box.
- **Layout:** rows start at `px.y + HEADER`, step `LINE_STEP`, and stop when
  the next row's bottom would leave the box (`row_bottom > px.y + px.h`).
  Rows are dropped, not clipped mid-glyph.

## Backdrop ("dim behind text only")

One translucent `CODE_BG` quad behind just the name + doc block:

- Full inner box width (box x..x+w).
- Vertical extent: from box top to the bottom of the last rendered doc row.
- Alpha: 0.85 × overlay opacity (`tex_opacity`), so the blurred texture
  stays visible around/below the text block.

## Implementation shape

- `PaintItem` gains `doc_rows: Vec<BodyText>` and `doc_panel_h: f32`;
  overlay alpha reuses the existing `tex_opacity` field.
- New pure helper `wrap_doc(text: &str, w_px: f64, font_px: f64) ->
  Vec<String>` next to `truncate_to_width` in treemap.rs: paragraph joining,
  greedy wrap, hard split.
- Populated in the leaf branch of `paint_items` only when a texture quad is
  built and `node.doc` is Some.
- **Paint order (Pass 1):** texture quad → backdrop panel → doc rows. This
  keeps pinned container headers (Pass 2b) and focus/neighbor rings (Pass 3)
  on top.

## Testing

- Unit tests for `wrap_doc`: budget math matches `truncate_to_width`'s
  heuristic, greedy wrap at word boundaries, hard split of over-budget
  words, blank-line paragraph breaks.
- Unit test for row layout: row count limited by box height (next row that
  would overflow is dropped), panel height ends at last row.
- Fade needs no new tests — it reuses `tex_opacity` math already covered.
- Manual visual gate: `cargo run -p outrider -- .`, zoom to texture tier,
  verify crisp name + wrapped doc with dim panel, smooth fade across the
  Text↔Texture band, and unchanged rendering for doc-less leaves.
