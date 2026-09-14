# Leaf-Page Scaling, File Chunking & Square Packing — Design

- Date: 2026-07-10
- Parent: `2026-07-10-text-pages-design.md`,
  `2026-07-10-window-chrome-leaf-nav-design.md` (built on the merged main
  with client chrome + leaf-to-leaf nav).
- Motivation, from a smoke-test session. Four coupled problems with how
  leaf pages render and lay out:
  1. **Text does not scale with its box.** `code_scale` is
     `clamp(zoom, 7/12, 1.0)`: below zoom 0.58 the font floors and the
     line-window *clips* lines; above 1.0 the font caps and the box grows
     past the text (empty space). Text should stay put — the same line at
     the same fractional position in the box at every zoom.
  2. **Large files are unreadable.** A childless file is one page as tall
     as its line count (a 2000-line file is 480 × ~31000 world px);
     framing it (`frame_page`) zooms out until nothing is legible.
  3. **In-file packing is not square.** The shelf packer fills rows
     left-to-right; tall pages produce awkward, wide-and-short or very tall
     containers.
  4. **No distant representation.** Zoomed out, every visible code line is
     live-shaped — slow and illegible — instead of a cheap "code from a
     distance" texture that resolves smoothly into text as you zoom in.

The four ship together as one change because they all reshape the leaf
page: uniform scaling (A) defines how a page paints, the far-zoom LOD (B)
defines how it paints when small, chunking (C) bounds page size so scaling
stays readable, and square packing (D) arranges the now-bounded pages.

## 1. Goals

- A leaf page scales as one unit with its box: text at `FONT_PX · zoom`,
  the whole page always painted, no clipping and no empty box. Position is
  proportional at every zoom.
- Zoomed far out, a leaf paints a **minimap** — one colored bar per source
  line — that resolves seamlessly into live text as it grows.
- Large text files split into ordered, readable sub-pages via a
  per-file-type strategy (line slices by default, semantic splits for
  Markdown).
- Containers pack column-first toward a roughly square aspect.

Out of scope stays in §9. The maximize-to-quadrant WSLg bug is **not** in
this change.

## 2. Uniform leaf-page scaling (`content.rs`, `world.rs`, `treemap.rs`)

A leaf page's on-screen height is `ph = natural_px(node) · zoom` (the
packer sizes a leaf's rect height to exactly `natural_px`). So the page's
uniform scale is simply

```
scale = ph / natural_px(node)   // == camera zoom, uncapped
```

Everything inside the page — the header/name row, the signature/readout
row, every code line, the bottom pad — is drawn at this one scale. Font is
`FONT_PX · scale`, row step is `LINE_STEP · scale`, row `r` (0-based over
the whole page, row 0 = header) sits at `top + r-offset · scale`. Because
the box height is `natural_px · scale`, the page fills the box exactly at
every zoom: never clipped, never surrounded by empty space.

Changes:

- **`content.rs`:** `code_scale` is deleted. The clamp constants
  `MIN_CODE_FONT_PX`, `MIN_CODE_SCALE`, and `LEAF_CODE_MIN_PX` are removed
  (the last is superseded by the leaf tiers in §3). A new
  `MIN_TEXT_FONT_PX: f64 = 7.0` marks the text/minimap boundary (§3).
- **`treemap.rs` paint:** the leaf-item body path stops calling
  `code_scale` and stops windowing/clipping. It computes
  `scale = item.full_h / natural_px(node)` and paints **all** of the
  page's rows at that scale (subject to viewport culling of individual
  off-screen rows for cost, which changes nothing visible — a row is drawn
  iff its scaled y-band intersects the viewport). The pinned-header logic
  (`min_y`/`max_y` window) is removed for leaf pages; the header is row 0
  and scales with the page.
- Containers are untouched: their name header stays pinned and readable at
  a fixed size, and they keep the `Rung` ladder (§3 leaves it intact for
  non-leaf nodes).

The camera is unchanged. `frame_page` still caps a framed leaf at zoom 1.0
(font 12), and scroll-zoom past 1.0 now grows the text (read up close) up
to `MAX_ZOOM = 8` (font 96) instead of leaving an empty box.

## 3. Far-zoom LOD and leaf tiers (`world.rs`, `buffer.rs`, `treemap.rs`, `theme.rs`)

Leaf items stop using `rung_for`. A leaf's draw mode is chosen by its
on-screen box size:

```rust
pub enum LeafDraw { Dot, Label, Minimap, Text }

/// None => merged away (below MERGE_PX).
pub fn leaf_draw(ph: f64, pw: f64, natural_px: f64) -> Option<LeafDraw>
```

with, in order:

| condition (first match wins)                     | mode      |
|--------------------------------------------------|-----------|
| `ph < MERGE_PX` (4)                              | `None`    |
| `pw < LABEL_MIN_W` (60) or `ph < LABEL_PX` (20) | `Dot`     |
| `ph < CARD_PX` (80)                             | `Label`   |
| font `≥ MIN_TEXT_FONT_PX` (7) and `pw ≥ CODE_MIN_W` (300) | `Text` |
| otherwise                                       | `Minimap` |

where `font = FONT_PX · ph / natural_px`. So: tiny boxes show a dot; short
boxes show the pinned name (`Label`); taller boxes show the page as a
`Minimap` while the font would be sub-7px or the column is too narrow for
code, and as live `Text` once it clears both gates. A short leaf steps
`Label → Text`; a tall leaf steps `Label → Minimap → Text` as it grows —
the minimap bars occupy the exact rows the glyphs later occupy, so the
transition is seamless.

**Walk/DrawItem (`world.rs`):** `walk` dispatches on `is_leaf_item`: leaves
get `leaf_draw`, containers keep `rung_for` (whose `natural_px` special
case and the now-unused param are removed — containers never had one). The
draw mode carried on `DrawItem` becomes an enum
`Draw { Container(Rung), Leaf(LeafDraw) }`; `rung_for` loses its
`natural_px: Option<f64>` argument.

**Minimap rows (`outrider-index/src/buffer.rs`):** the minimap needs, per
line, the leading-whitespace width, the trimmed length, and a dominant
color — all cheap to precompute once at materialization from the spans
already stored per line:

```rust
pub struct MinimapRow { pub indent: u32, pub len: u32, pub kind: HighlightKind }
impl FileBuffer {
    pub fn minimap_row(&self, i: usize) -> MinimapRow; // cached Vec, one per line
}
```

`indent` = count of leading spaces/tabs (tab = 1 col); `len` = trimmed
visible length in chars; `kind` = the `HighlightKind` covering the most
bytes on the line, ties broken by first occurrence, `Default` when the
line is all-default or blank. A blank line yields `len == 0` (no bar).

**Minimap paint (`treemap.rs`):** for a `Minimap` leaf, per source line
`r` draw one `paint_quad` (no text shaping):

- y-band identical to the text row: `y = top + (HEADER + (1 + r)·LINE_STEP)·scale`,
  bar height `= LINE_STEP · scale · 0.7`, vertically centered in the row.
- x `= left + BODY_PAD·scale + indent·CHAR_ADV·scale`,
  width `= min(len·CHAR_ADV, PAGE_W − BODY_PAD − indent·CHAR_ADV)·scale`,
  where `CHAR_ADV = 0.6·FONT_PX = 7.2` is the monospace advance and
  `BODY_PAD` is the existing left text inset (6px). `len == 0` draws
  nothing.
- color `= theme::syntax_color(kind)`, dimmed toward the page background
  (a new `theme::minimap_color(kind) = lerp(syntax_color(kind), CODE_BG,
  0.15)`) so the minimap reads as texture, not full-brightness code.

The same left/top/`full_h`/`label_w` geometry the text path uses drives
the minimap, so bars and glyphs are pixel-aligned across the tier switch.

## 4. File chunking (`outrider-index`: new `chunk.rs`, `types.rs`, `scan.rs`; `buffers.rs`, `content.rs`)

### 4.1 Strategy interface (`outrider-index/src/chunk.rs`, new)

```rust
pub struct Chunk {
    pub start_line: usize, // 0-based, inclusive
    pub end_line: usize,   // 0-based, exclusive
    pub start_byte: usize,
    pub end_byte: usize,
    pub label: String,     // "61–120" or a Markdown heading
}

pub trait ChunkStrategy {
    /// Ordered, contiguous, covering chunks. Returns a single whole-file
    /// chunk when the file is under threshold (caller treats len==1 as
    /// "do not chunk").
    fn chunks(&self, text: &str) -> Vec<Chunk>;
}

pub fn strategy_for(ext: &str) -> Box<dyn ChunkStrategy>;

pub const CHUNK_MAX_LINES: usize = 60; // soft cap / slice size
```

- `strategy_for`: `"md" | "markdown"` → `MarkdownChunker`; everything else
  (txt, toml, unparsed rs, no ext) → `LineChunker`.
- **`LineChunker`** (default): if `≤ CHUNK_MAX_LINES` lines, one chunk
  spanning the file. Otherwise fixed slices of `CHUNK_MAX_LINES` lines;
  the final slice is the remainder. Labels are `"{start+1}–{end}"`
  (1-based inclusive, e.g. `"1–60"`, `"61–120"`).
- **`MarkdownChunker`**: scan lines, starting a new chunk at every heading
  line (`^\s{0,3}#{1,6}\s`) and, within a heading's section, at a blank
  line once the running chunk would exceed `CHUNK_MAX_LINES` (never split
  a non-blank run). Merge a leading pre-heading preamble into the first
  chunk. A chunk that begins on a heading is labeled with that heading's
  text (markers and surrounding whitespace stripped); other chunks use the
  `"{start+1}–{end}"` range label. A document that yields one chunk is not
  split.

Both strategies are pure functions of `&str`, fully unit-testable, with no
GPUI or filesystem dependency.

### 4.2 Chunk nodes in the tree (`types.rs`, `scan.rs`)

- **`SymbolKind`** gains a trailing `Chunk` variant (appended after `Fn`
  so existing `Ord`/serialized values are unchanged). `is_leaf_item`
  already admits it (bytes present, no children, not a Folder). The
  `count` match in `content.rs::kind_counts` gets a `Chunk` arm (see 4.4).
- **`build_tree`/`build_folder` (`scan.rs`)** thread `repo_root` down. When
  a File node would be a **childless leaf** (`parsed.items` empty) and its
  `lines > CHUNK_MAX_LINES`, read the file text
  (`repo_root.join(rel_path)`; on read error or non-UTF-8, skip chunking —
  the file stays a single page) and run `strategy_for(ext).chunks(text)`.
  If it returns more than one chunk, replace the File node's empty
  `children` with one `SymbolNode` per chunk:
  - `kind: SymbolKind::Chunk`,
  - `qualified_path: "{file_qual}#{i}"` (i = chunk index, unique),
  - `name: chunk.label`,
  - `byte_range: Some(start_byte..end_byte)`,
  - `signature: None`, `doc: None`,
  - `measure: (end_line − start_line) as u64`,
  - `churn: 0.0`, `churn_count: 0`, `children: vec![]`.

  The parent File node keeps `byte_range: Some(0..bytes)` and its total
  `measure`; now that it has children it is a container (renders name +
  inventory, §4.4), not a page. `finalize_children`/`dedupe_ids` run as
  today; chunk ordering at paint time comes from the packer sorting on
  `byte_range.start` (§5), so the `finalize_children` name-sort of chunk
  labels is harmless.

### 4.3 Chunk anchors (`buffers.rs`)

`collect_file_symbols` today pushes a childless file's own id at offset 0.
For a **chunked** file it pushes one `(chunk.id, chunk.byte_range.start)`
per chunk child, so `symbol_start_line(chunk_id)` resolves to the chunk's
first rope line. Files with real parsed items are unchanged.

### 4.4 Chunk content (`content.rs`)

- A `Chunk` leaf renders like a text page: `body_lines(chunk, Full)` =
  `[Dim(churn_readout(chunk))]` — one readout row; the paint path appends
  the chunk's own lines (`start_line ..= start_line + measure`) from the
  buffer. `natural_px` (via `measure`) sizes it to its line count.
- The chunked **File** container: `kind_counts` counts `Chunk` children as
  `"{n} parts"` (e.g. `"5 parts"`), so its inventory reads
  `"5 parts · 480L · 47 commits · p96"`. All other kinds unchanged.

## 5. Column-first square packing (`outrider-layout/src/pack.rs`, `world.rs`)

`PACK_ASPECT` changes `1.6 → 1.0` (target width/height ratio ≈ square).
The `size` pass fills **columns top-to-bottom, wrapping right**, instead of
shelves:

```
area   = Σ child w·h
tallest= max child h
target_h = tallest.max((area / aspect).sqrt())   // aspect = 1.0 → √area
x = 0; y = 0; col_w = 0; content_h = 0
for child (w, h) in order:
    if y > 0 and y + h > target_h:      // wrap to next column
        x += col_w + gap; y = 0; col_w = 0
    place child at (gap + x, header + gap + y)
    col_w = col_w.max(w)
    content_h = content_h.max(y + h)
    y += h + gap
content_w = x + col_w
container = (content_w + 2·gap, header + content_h + 2·gap)
```

- `tallest.max(...)` guarantees no child is ever forced to wrap alone
  (mirrors today's `widest.max(...)` width floor). Because leaf pages share
  `PAGE_W`, columns are uniform-width and pages stack like newspaper
  columns; ordered chunks read top-to-bottom then rightward.
- **Ordering:** `size` still sorts a container's children before placing.
  Default order stays `(name, ordinal)`. When the children are chunks
  (first child `kind == Chunk`), it orders by `byte_range.start` so chunks
  pack in source order regardless of their heading labels.
- Hierarchical stability is preserved: a container's layout still depends
  only on its own children's sizes.

`world.rs` re-exports `PACK_ASPECT = 1.0`; `pack_config()` is otherwise
unchanged.

## 6. Interaction & camera

No camera-math changes. `frame_page`'s 1.0 cap keeps a freshly-framed leaf
at natural size; uniform scaling means scroll-zoom past 1.0 grows text.
Chunk leaves are ordinary leaf pages, so leaf-to-leaf arrow nav
(`content::is_leaf_item`) already steps across chunks and across files with
no change. Hit-testing is unchanged (chunks are the deepest nodes under a
pointer inside a chunked file).

## 7. File/module map

- `outrider-index/src/chunk.rs` — **new**: `Chunk`, `ChunkStrategy`,
  `LineChunker`, `MarkdownChunker`, `strategy_for`, `CHUNK_MAX_LINES`.
- `outrider-index/src/types.rs` — `SymbolKind::Chunk`.
- `outrider-index/src/scan.rs` — chunk injection in `build_folder`
  (threads `repo_root`, reads text for over-threshold childless files).
- `outrider-index/src/buffer.rs` — `MinimapRow` + `FileBuffer::minimap_row`
  (cached per line).
- `outrider/src/buffers.rs` — chunk anchors in `collect_file_symbols`.
- `outrider/src/content.rs` — delete `code_scale`/clamp constants, add
  `MIN_TEXT_FONT_PX`, `Chunk` body + `kind_counts` "parts".
- `outrider/src/world.rs` — `LeafDraw`, `leaf_draw`, `Draw` enum, trimmed
  `rung_for`, `PACK_ASPECT` re-export.
- `outrider/src/treemap.rs` — uniform-scale text paint, minimap paint,
  leaf/container draw dispatch.
- `outrider/src/theme.rs` — `minimap_color`.
- `outrider-layout/src/pack.rs` — column-first `size`, chunk ordering.

## 8. Testing

Headless unit tests (the standing gate: `cargo test --workspace` green,
`cargo clippy --workspace --all-targets -- -D warnings` clean):

- **Uniform scale (`content.rs`/`world.rs`):** `leaf_draw` returns each
  mode at its boundary (Dot/Label/Minimap/Text thresholds, width gates,
  merge). A tall leaf at low zoom → `Minimap`; the same leaf zoomed until
  `font ≥ 7` → `Text`; a short leaf never enters `Minimap`. No code_scale
  clamp remains (font grows unbounded with zoom, shrinks unbounded until
  the minimap tier takes over).
- **Minimap rows (`buffer.rs`):** indent/len/kind for an indented,
  highlighted line; blank line → `len == 0`; dominant-kind tie broken by
  first occurrence.
- **Chunking (`chunk.rs`):** `LineChunker` — ≤60-line text → one chunk;
  150 lines → 60/60/30 with labels `1–60`, `61–120`, `121–150`.
  `MarkdownChunker` — splits at each heading, labels chunks by heading
  text; an over-long section splits at a blank line without breaking a
  paragraph; a short doc → one chunk. Byte/line ranges are contiguous and
  cover the whole text.
- **Tree (`scan.rs`):** a childless >60-line `.md` file becomes a File
  container of `Chunk` children with contiguous byte ranges and
  source-order `byte_range.start`; a ≤60-line file stays a single page; a
  `.rs` file with parsed items is untouched.
- **Anchors (`buffers.rs`):** each chunk's `symbol_start_line` resolves to
  its first line; a non-chunked childless file still anchors at line 0.
- **Content (`content.rs`):** `Chunk` Full body = one readout row;
  chunked-file `kind_counts` = `"{n} parts"`; existing body_lines cases
  unchanged.
- **Packing (`pack.rs`):** column-fill worked example with exact rects
  (recomputed in the plan); `tallest` floor prevents lone wraps; chunk
  container orders by `byte_range.start` even when labels sort otherwise;
  existing determinism / sibling-stability tests updated to the square
  aspect.

Manual WSLg exit gate (`cargo run -p outrider -- .`): a large README/
Markdown file shows ordered, readable chunk pages; zooming a chunk in
resolves minimap bars into highlighted text with the text staying put (no
clip, no empty box); text grows past natural size when zoomed in; folders
and chunked files read roughly square.

## 9. Out of scope

- True offscreen texture caching of pages (the minimap is the LOD; a
  rasterized-glyph cache stays a possible later optimization).
- Chunking parsed code items (over-long functions stay single pages).
- Word-wrap for long lines (truncation/telescoping as today).
- The maximize→quadrant WSLg bug (separate investigation).
- Inline-Markdown grammar, languages beyond the current rs/md/toml set,
  editing pages.
