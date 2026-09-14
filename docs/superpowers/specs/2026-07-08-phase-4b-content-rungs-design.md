# Phase 4b: Detail/Full Rungs + Rope Substrate — Design

- Date: 2026-07-08
- Parent: `docs/superpowers/specs/2026-07-05-outrider-walking-skeleton-design.md`
  §5.3 (substrate doors 1–3), §7.2 (fidelity ladder) and the Phase 4 entry of
  `docs/superpowers/plans/2026-07-08-walking-skeleton-roadmap.md`.
- Scope split: Phase 4a (shipped) delivered focus + camera-follow — the
  motion half of Bet #1. **4b (this spec):** the content half — Detail/Full
  rungs, rope materialization, anchors, tree-sitter highlighting.
- Builds on: screen-space columns
  (`2026-07-08-screen-space-columns-design.md`) and Phase 4a navigation
  (`2026-07-08-phase-4a-structural-navigation-design.md`).

## 1. Goal

The closer you get, the more it looks like code. Two new rungs complete the
fidelity ladder: Detail (250–700 px) shows signatures and summaries, Full
(≥700 px) shows tree-sitter-highlighted source rendered from a rope buffer.
This exercises three of the parent's four one-way doors (rope, anchors,
single buffer) and makes Bet #1 fully assessable.

## 2. Decisions settled during brainstorming

1. **Detail content:** item signatures, no method sub-labels inside file
   boxes — the parent spec's "method names as sub-labels" predates the
   column model, where a file's methods are already the adjacent column.
2. **Full content:** leaf items only render code; files and folders render
   summaries (doc header + inventory). Container items (impl/struct with
   children) cap at signature + inventory — their children carry the code.
3. **Summaries:** derived data only (LLM narration is out of skeleton
   scope). Files: the `//!` doc block — the author's own summary — plus an
   inventory. Folders: inventory only.
4. **Anchors:** full remap-on-edit logic lands now, headless-tested against
   synthetic edits. Phase 6 wires real disk edits into an already-tested
   path rather than landing remap and live-reload together.
5. **Placement (approach A):** the buffer substrate lives in
   `outrider-index` (which already owns tree-sitter); the app owns only the
   ephemeral LRU manager and paint code. Only new dependency: `ropey`.

## 3. Index additions (`outrider-index`)

### 3.1 SymbolNode metadata

```rust
pub struct SymbolNode {
    // … existing fields …
    /// Item declaration up to (excluding) the body `{`, whitespace
    /// collapsed to one line. None for folders and files.
    pub signature: Option<String>,
    /// Leading `//!` block, comment markers stripped. File nodes only.
    pub doc: Option<String>,
}
```

Extracted during the existing parse pass. These are index-derived metadata:
refreshed by re-index (Phase 6), not read from the rope. The single-buffer
invariant (parent §5.3 door 3) is scoped explicitly to **code at Full**,
which always renders from the rope.

### 3.2 Buffer module (`outrider-index/src/buffer.rs`, GPUI-free)

```rust
pub struct FileBuffer {
    rope: ropey::Rope,
    tree: tree_sitter::Tree,
    lines: Vec<Vec<HighlightSpan>>,   // per-line, computed at materialization
    anchors: AnchorList,
}
pub struct HighlightSpan { pub range: Range<usize>, pub kind: HighlightKind }
// byte range within the line

pub enum HighlightKind {
    Keyword, Function, Type, String, Comment, Number, Property, Default,
}
```

- `FileBuffer::new(text: String)` parses with `tree-sitter-rust`, then runs
  `tree_sitter_rust::HIGHLIGHTS_QUERY` directly with `Query`/`QueryCursor`
  (no new highlighting dependency), mapping capture-name prefixes to
  `HighlightKind`. Whole-file spans are computed once at materialization —
  fine at skeleton file sizes.
- `line(&self, i) -> Option<(String, &[HighlightSpan])>` and
  `len_lines()` serve the render's line window. Line text comes from the
  rope — never from a cached copy of `text`.

### 3.3 Anchors

```rust
pub struct AnchorId(usize);
pub struct Edit { pub range: Range<usize>, pub new_len: usize }

impl AnchorList {
    pub fn create(&mut self, offset: usize) -> AnchorId;
    pub fn resolve(&self, id: AnchorId) -> usize;
    /// Survive-edits rule: positions after the edit shift by the length
    /// delta; positions inside a replaced/deleted range clamp to its start.
    pub fn remap(&mut self, edit: &Edit);
}
```

When a file materializes, the manager creates one anchor per symbol in that
file (at `byte_range.start`). The Full-rung render resolves anchors — never
raw `byte_range` offsets — so the render path is already anchor-shaped when
Phase 6 starts mutating buffers. In 4b `remap` is exercised only by tests.

## 4. App additions (`outrider`)

### 4.1 BufferManager (`crates/outrider/src/buffers.rs`, GPUI-free)

```rust
pub const MAX_BUFFERS: usize = 64;

pub struct BufferManager { /* LRU keyed by relative file path */ }
impl BufferManager {
    pub fn new(repo_root: PathBuf) -> Self;
    /// Materialize from disk on first access; LRU-evict beyond MAX_BUFFERS.
    /// None if the file cannot be read (box falls back to Detail content).
    pub fn get(&mut self, rel_path: &str) -> Option<&FileBuffer>;
}
```

The file path is the portion of `qualified_path` before the first `::`
(the whole path when there is none, as on File nodes). Repeated `get` refreshes recency without re-reading
disk. `TreemapView` owns the manager — materialized buffers are ephemeral
app state (parent §4).

### 4.2 Rung selection (`world.rs`)

```rust
pub const CARD_PX: f64 = 80.0;    // existing
pub const DETAIL_PX: f64 = 250.0; // new: Card becomes 80–250
pub const FULL_PX: f64 = 700.0;   // new
pub const CODE_MIN_W: f64 = 300.0;
```

`Rung` gains `Detail` and `Full`. Selection stays by pixel height; the
existing `px_w < LABEL_MIN_W → Dot` downgrade stays; new rule: Full
downgrades to Detail when `px_w < CODE_MIN_W` (code in a sliver column is
useless). All tunable at the exit gate.

### 4.3 Content by node type

| Node | Detail (250–700) | Full (≥700) |
|---|---|---|
| Leaf item (`byte_range`, no children) | signature | signature + highlighted code |
| Container item (children) | signature | signature + inventory |
| File | name + churn readout + doc first line + inventory line | doc block + inventory |
| Folder | name + churn readout + inventory line | inventory |

Inventory derives from the `SymbolTree` at paint time: item counts by kind,
total lines, churn readout (e.g. `4 fns · 2 structs · 480L · 47 commits ·
p96`); folders list child file/folder counts and lines.

### 4.4 Painting (`treemap.rs`, `theme.rs`)

- `theme.rs` gains a syntax palette: one color per `HighlightKind`
  (Default = `TEXT_PRIMARY`).
- `paint_items` builds owned per-item content: for Full leaf items it
  resolves the symbol's anchor, converts to a rope line range, and emits
  only the lines whose y intersects the viewport (line-window cull), each
  as text plus colored runs from the highlight spans. Monospace 12 px as
  today; long lines truncate to box width (existing `truncate_to_width`).
- Card content is unchanged. Detail/Full text starts below the name line
  with the existing padding conventions.
- Shaping visible lines per frame is acceptable at skeleton scale (a Full
  box shows ~50 lines; a handful of Full boxes exist at once).

## 5. Testing

Headless:

1. **Anchor remap** (`buffer.rs`): insert before / inside / after an
   anchor; delete spanning an anchor (clamps to edit start); multi-anchor
   ordering preserved across edits.
2. **Highlighting:** fixture snippet → `fn` keyword span is `Keyword`,
   comment line is `Comment`, string literal is `String`; every span lies
   within its line's bounds.
3. **Signature/doc extraction** (index fixtures): known fixture file →
   exact one-line signature strings; `//!` block extracted with markers
   stripped; items without docs → `None`.
4. **Rung thresholds** (`world.rs`): 250 px → Detail, 700 px → Full; Full
   at `px_w < CODE_MIN_W` → Detail; existing Dot/Label/Card cases
   unchanged.
5. **BufferManager:** LRU eviction at cap + 1; missing file → `None`;
   repeated `get` returns the same materialized buffer (cache hit).
6. **Inventory derivation:** worked-example tree → exact inventory strings.

Feel — code legibility, ramp smoothness, summary usefulness — is manual.

## 6. Exit gate (Bet #1, complete)

`End` on a method fills the box with highlighted code; arrow-stepping
between methods at Full reads as moving through the code; file/folder
summaries appear on the way down. Then the parent §8.3 question 1 protocol
on Outrider's own repo — with both the motion (4a) and content (4b) halves
present, Bet #1 gets its full read. Verdict recorded in the ledger; a
"slideshow" verdict goes back to the parent document before Phase 5.

## 7. Out of scope (later phases)

Enter/Esc descend transition and frozen layers (Phase 5), live reload /
`notify` watching / incremental re-parse and real `remap` invocation
(Phase 6), editing of any kind, sparklines/glyphs/LLM text, non-Rust
highlighting, jump-to-symbol, call edges.
