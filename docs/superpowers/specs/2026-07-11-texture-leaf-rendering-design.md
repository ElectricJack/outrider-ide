# Baked Text Textures for Far-Zoom Leaves

**Date:** 2026-07-11
**Status:** Approved

## Problem

Below the Text tier a leaf currently paints minimap bars: one colored quad
per source line (strided at far zoom). Thousands of on-screen leaves times
dozens of bars each is the dominant quad cost at far zoom, and bars are an
abstract stand-in — they never look like the text they replace. We want the
"map of code" look: real text that degrades naturally as it shrinks, drawn
as **one textured quad per leaf**.

## Goals

- Replace minimap bar rendering entirely: one `paint_image` quad per
  far-zoom leaf instead of N bar quads.
- The texture holds the leaf's real source lines rasterized with real
  glyphs and syntax colors, so zooming shows genuine text shrinking away.
- Calm rendering at every zoom: no shimmer from undersampling, no frame
  stalls from baking.
- No GPUI fork. Work within `paint_image` + bilinear atlas sampling.

## Non-Goals

- Trilinear filtering / GPU mipmaps (needs a GPUI patch — possible
  follow-up; for now nearest-LOD selection only, accepting a small pop at
  level boundaries).
- Blending two LOD levels per leaf (emulated trilinear) — revisit if
  level-boundary pops are visible in practice.
- Texturing container headers or Label/Dot-tier name text — those stay on
  the shaped-text path.

## Design

### 1. Tier change (`world.rs`, `treemap.rs`, `content.rs`)

`LeafDraw::Minimap` becomes the texture tier: same tier boundaries
(`leaf_draw` unchanged), new painter. Deleted outright:

- `leaf_minimap()` and `MinimapBar` in `treemap.rs`
- `PaintFrame::bars` and the bar paint pass
- `BAR_FADE_W_LO` / `BAR_FADE_W_HI` in `content.rs` (bar width fade)
- `theme::minimap_color` if no other caller remains

The existing `TEXT_FADE_LO=5.0 .. TEXT_FADE_HI=9.0` (on-screen font px)
crossfade now blends shaped text ↔ textured quad exactly as it blended
text ↔ bars: texture opacity ramps 1→0 over the band while body text ramps
0→1. Below `TEXT_FADE_LO` the texture paints at full opacity at all deeper
zooms, including the Dot tier (the quad is clipped/scaled by the same
content bounds the bars used). Opacity is applied via
`window.with_element_opacity`.

### 2. Rasterizer (new module `crates/outrider/src/rasterize.rs`)

Direct dependency on `cosmic-text` (already a transitive dep of GPUI on
Linux — no new lockfile entries; pure Rust, works on Windows too). One
`FontSystem` + `SwashCache` created lazily and shared (behind the view or
a `OnceLock`), loading system fonts and matching `theme::FONT_FAMILY`.

Input: the leaf's source lines with highlight spans, exactly what
`leaf_text_body` reads today: `buffers.get(&rel, syms)`,
`symbol_start_line`, `m.buffer.line(start + j) -> (text, spans)`, colored
via `theme::syntax_color(kind)` on a transparent background (the leaf's
box fill shows through, matching how bars sat on the box color).

Master image:

- Line height `L = clamp(1024 / line_count, 1.0, 4.0)` px; font size
  `L / 1.3` (mirrors `LINE_STEP = FONT_PX * 1.3`).
- Height = `line_count * L` (≤ 1024). Leaves with > 1024 lines stride
  rows (like the old bars) so height stays ≤ 1024.
- Width = aspect-correct to the leaf's text column:
  `PAGE_W / LINE_STEP * L` px (≈ 123 px at L=4).
- Layout mirrors the Text tier: `BODY_PAD` left inset, `HEADER` band
  skipped (the pinned name row draws over the quad's top separately, as
  it does today).
- Buffer is RGBA during compositing, then byte-swapped to BGRA
  (`pixel.swap(0, 2)` per 4-byte chunk) and wrapped:
  `Frame::new(ImageBuffer::from_raw(w, h, buf))` →
  `Arc<RenderImage>`.

### 3. Mip chain + nearest-LOD selection

After the master bake, CPU box-downsample (average each 2×2 texel block,
premultiplied-alpha aware) by 2× repeatedly until the height is ≤ 8 px.
Each level is its own `Arc<RenderImage>`; total memory ≈ 1.33× master
(≤ ~0.7 MB worst case per leaf at 123×1024, typical well under 100 KB).

At paint time: screen content height `h_px` → pick the smallest level
whose height ≥ `h_px` (slight downscale under bilinear = clean), then one
`window.paint_image(bounds, no radii, level, 0, false)`.

### 4. Cache, bake budget, eviction (owned by the treemap view)

```
struct LeafTexture { levels: Vec<Arc<RenderImage>>, bytes: usize }
cache: HashMap<SymbolId, LeafTexture> + LRU order
```

- **Budget:** at most 4 bakes per frame. Texture-tier leaves that miss the
  cache enqueue (keyed by on-screen area, largest first) and paint only
  their box fill this frame; a repaint is requested while the queue is
  non-empty, so textures pop in over a few frames.
- **Eviction:** LRU over total `bytes`, cap 64 MB. Evicting calls
  `window.drop_image(image)` for every level so atlas memory is actually
  reclaimed, then removes the entry.
- **Invalidation:** keyed by `SymbolId`; buffers are static per session
  today, so no content-hash invalidation. If/when live editing lands,
  evict on buffer change.

### 5. Paint order

Textured quads paint exactly where each leaf's bars painted: inside
Pass 1, per item, after the box quad and stripe — under all text and
header backgrounds, over box fills.

## Testing

- Rasterizer: output dimensions and clamping (L, width cap, stride for
  huge leaves), BGRA byte order (known glyph → known channel pattern),
  determinism (same input → identical bytes), transparent background.
- Mip chain: level count and sizes (halving to ≤ 8 px), 2×2 box average
  correctness on a synthetic image.
- LOD selection: given a level ladder and a screen height, picks the
  smallest level ≥ screen height; clamps at both ends.
- Cache: bake budget (only N per frame), LRU eviction order, byte
  accounting; `drop_image` calls verified by trait/shim where practical.
- Paint path and visual quality verified manually (crossfade band, Dot
  tier, level-boundary pops).

## Risks

- At L ≤ 2px glyphs are smudges — intended at far zoom, but if the band
  near the 6px threshold looks muddy, raise the master to 6–8px lines
  (2–4× memory) — the design isolates this in one constant.
- Nearest-LOD pops at level boundaries; accepted for now, revisit with
  two-level blend or a GPUI mipmap patch.
- cosmic-text may resolve a different font file than GPUI's text system
  for the same family name; acceptable since the texture tier never sits
  beside crisp text at readable sizes (crossfade band is 5–9px).
