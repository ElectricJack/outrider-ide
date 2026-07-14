# Focused Node Expansion & Base Width Bump

## Problem

Leaf page width (`PAGE_W = 480`) is too narrow for most code and prose lines,
causing pervasive text truncation. Even when a node is focused and the user is
actively reading it, lines are clipped with ellipsis. There is no mechanism for
the focused node to show more content.

## Changes

### 1. Base width bump: PAGE_W 480 → 640

Increase the leaf page width constant from 480 to 640 world units. This reduces
clipping across the board with a single constant change.

**Files touched:**
- `crates/outrider/src/world.rs` — `PAGE_W` constant
- `crates/outrider-layout/src/pack.rs` — test assertions (hardcoded 480 values)
- `crates/outrider/src/world.rs` — test assertions

### 2. Render-time focused node expansion

When a node is focused, its painted box grows wider to fit its content. This is
purely render-time — the packed layout (`PackLayout`) is never modified.

**Width calculation:**
- Measure the longest visible line (code or body text) in character count.
- Convert to pixel width: `chars * font_px * 0.62 + 12.0` (inverse of
  `truncate_to_width`'s budget formula).
- Clamp to `[PAGE_W, 2.0 * PAGE_W]` — minimum is the base width (640),
  maximum is 1280 world units.
- Multiply by camera zoom for screen-space width.

**PaintItem changes for the focused node:**
- `w` — set to expanded screen width.
- `label_w` — set to expanded screen width (text uses the wider budget).
- `x` — unchanged (box grows rightward).

**Paint order:** The focused node's `PaintItem` is moved to the end of the list
so it paints on top of any neighbors it overlaps.

**Not affected:** Packed layout, camera framing, texture cache, hit testing.

### 3. Line wrapping for focused nodes

When a node is focused, lines that exceed the expanded width wrap instead of
truncating with ellipsis.

**Code lines (leaf text body):**
- In `leaf_text_body()`, when focused and a line exceeds the width, wrap it
  into multiple `BodyText` entries.
- Each continuation shifts subsequent lines down by `step`.
- The box height grows to accommodate extra wrapped rows.

**Body text (container body):**
- In `container_body()`, when focused, use `wrap_doc()`-style word wrapping
  instead of `truncate_to_width()`.

**Interface change:**
- `leaf_text_body()` and `container_body()` receive a `focused: bool` parameter.
- When `true`, they wrap instead of truncate.

**Height adjustment:**
- `PaintItem.h` grows to fit extra wrapped rows.
- The focused box can grow both wider (expansion) and taller (wrapping).

**Hard-wrap cap:** Lines wider than the expanded width (2x PAGE_W = 1280 world
units) hard-wrap at that boundary, matching `wrap_doc()`'s existing behavior.

## Approach

Render-time-only expansion. The packed layout stays unchanged — expansion is
purely visual, applied when building `PaintItem`s in `paint_items()`. The
focused node overlaps neighbors, which is natural since it's the foreground
element the user is actively reading.

## What does not change

- `pack()` algorithm and `PackLayout` — deterministic, computed once
- Camera framing logic — frames the original layout rect
- Texture cache — textures are baked at base PAGE_W
- Hit testing — uses original `DrawItem` rects
- Non-focused nodes — render exactly as before at 640px width
