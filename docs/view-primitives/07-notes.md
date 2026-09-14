# 07 — Notes: doc / metric / agent text attached to symbols and ranges

**Parent:** [../view-primitives.md](../view-primitives.md) §2.7 (definition), §3.2 (text channel, Card+ only), §3.3 (provenance), §4 (hover-doc / meta-row decomposition), §8.7 (component spec), §9 milestone 3.
**Framework:** [00-framework.md](00-framework.md) — uses `ViewSpec`/`LayerSpec::Notes`, `SetRef`, `Deps` (`HOVER`, `FOCUS`, `SELECTION`, `TREE`), `SessionState { hover, focus, selection }`, `ResolveCtx`, `ResolvedView::notes`, `validate` ("agent note needs text"), `PaintOverrides::notes`, `default_view` (`{ at: "$hover", source: "doc" }`, `{ at: "$focus", source: "doc" }`), `apply_view_command`.
**Design doc:** [../code-comprehension-viewer-design.md](../code-comprehension-viewer-design.md) §5.2 (fidelity ladder; "anchor-attached inline summary boxes" at Full), §5.4 (narration layer is text-only, visually marked, never drives colour).
**Related:** 03-fill.md (inspect note "metric · value · basis"), 06-marks.md (glyph hover note), 05-edges.md / 08-panel.md (call-site range → line mapping is shared).

---

## 1. Purpose and scope

The Notes primitive owns every piece of *text* that is attached to a symbol and is not the symbol's own name or source code. Three provenances, one type, three renderings:

| source | ground truth | today | after this spec |
|---|---|---|---|
| `doc` | `SymbolNode.doc` (leading `//!` block) | hover tooltip / focused-file `DocPanel` built ad hoc in `paint_items` | a `Notes` layer in the session default view; the same `DocPanel` painted from `PaintOverrides::notes` |
| `metric` | a computed readout (`"142L · 12 commits · p87"`) | `content::card_meta/churn_readout/kind_counts/inventory` exist but are `#[cfg(test)]`; `container_body` returns `Vec::new()` — **no metric rows are drawn today** | a `MetricReadout` registry that un-`cfg`s those functions; consumed by 03-fill's inspect note, 06-marks' glyph hover note, and 08-panel's palette preview |
| `agent` | none — narration | does not exist | the marked narration style (`theme::NARRATION_*`), in-box at Detail/Full, floating callout otherwise, range-anchored callouts at Full/Text |

Hard invariants (parent §3.2/§3.3, design doc §5.4): a Note never changes `fill`, `border`, `stripe`, opacity or geometry; an `agent` note without text is rejected by `validate`; notes render at Card fidelity and finer only. Out of scope: editing notes in the UI, persistence beyond the view document, LLM generation.

---

## 2. Ground truth: existing code touched

| file | symbol | approx line | what it does today | what changes |
|---|---|---|---|---|
| `crates/outrider/src/treemap.rs` | `TreemapView::paint_items` | 1433–1807 | builds `PaintItem`s and the optional `DocPanel` | reads notes from `PaintOverrides::notes`; `panel_doc` logic replaced (§5.1) |
| " | `panel_doc: Option<(String,f32,f32,f32,f32)>` | 1491 | accumulator for the doc panel (doc text + box rect) | type becomes `Option<(Vec<ResolvedNote>, f32, f32, f32, f32)>`; still one panel per frame |
| " | `is_hovered` + `if item.node.doc.is_some() && (is_hovered \|\| (is_focused && panel_doc.is_none()))` | 1657, 1673–1681 | picks hover box, else focus box, if it has a doc | replaced by `ov.notes(&id)` non-empty check (§5.1) |
| " | `let doc_panel = panel_doc.and_then(..)` | 1713–1742 | wraps doc with `wrap_doc`, builds `DocPanel` above the box | moved to `view/note_pass.rs::build_doc_panel`, generalised to a `Vec<ResolvedNote>` |
| " | `DOC_PANEL_W`, `BODY_PAD` | 60, 57 | doc panel width floor, text inset | `DOC_PANEL_W` moves to `note_pass.rs` (pub(crate)); `BODY_PAD` stays |
| " | `hover_id`, `on_mouse_move` | 533, 2029–2060 | hover hit-test filtered to nodes with `doc` | filter dropped (`.filter(|i| i.node.doc.is_some())` removed) so agent/metric notes on `$hover` work; sets `view_dirty |= HOVER` (00-framework §3.4 already requires this) |
| " | `cg_highlight_lines` | 1458–1478 | byte range → line range via `BufferManager` for the selected call site | the byte→line mapping is factored into `view/note_pass.rs::byte_range_to_lines` and reused by range-anchored notes (and by 06-marks) |
| " | `container_body` | 819–830 | returns `Vec::new()` (descriptions removed) | unchanged in this milestone; §5.4 describes the optional metric-row restoration hook |
| " | `leaf_text_body` | 837–909 | leaf source rows; `highlight_lines` for the call site | unchanged; note pass reads the row geometry it produces (`BodyText.y`) |
| " | canvas closure Pass 4 (doc panel) | 4558–4602 | paints `DocPanel` quad + sans rows | unchanged code; followed by the new **Pass 5** note callouts (§5.3) |
| " | `paint_text` closure | 4388–4452 | paints `PaintItem.body` rows in mono | gains a per-row font selection so narration rows use `NARRATION_FONT` (§5.2) |
| `crates/outrider/src/paint_model.rs` | `DocPanel`, `BodyText`, `wrap_doc`, `wrap_to_budget` | 33–39, 10–16, 139–154 | doc panel model + wrapping | `BodyText` gains `style: RowStyle` (default `Code`); new `NoteCallout` struct |
| `crates/outrider/src/content.rs` | `card_meta`, `churn_readout`, `kind_counts`, `inventory`, `body_lines`, `BodyLine` | 39–130 | `#[cfg(test)]` readouts | `card_meta`, `churn_readout`, `kind_counts`, `inventory` lose `#[cfg(test)]` and are wrapped by the readout registry; `body_lines`/`BodyLine` stay test-only |
| `crates/outrider/src/theme.rs` | `DOC_COLOR`, `FONT_FAMILY_SANS`, `CODE_BG`, `FOCUS_BORDER`, `fingerprint()` | 33, 27–31, 38, 19, 41 | colours/fonts used by the doc panel | add `NARRATION_*` tokens (§5.2); add them to `fingerprint()` |
| `crates/outrider/src/buffers.rs` | `BufferManager::get`, `Materialized.buffer.byte_to_line`, `symbol_start_line` | 59, 25 | retained source, byte→line | unchanged; used by `byte_range_to_lines` |
| `crates/outrider-index/src/types.rs` | `SymbolNode.doc`, `.signature`, `.measure`, `.churn`, `.churn_count` | 56–63 | fields the readouts read | unchanged |
| `crates/outrider-view/src/spec.rs` (00-framework) | `NotesSpec` placeholder | — | declared but undefined | defined in §3 |
| `crates/outrider-view/src/validate.rs` | rule "agent note without text" | — | listed in 00-framework §2.6 | implemented here (§4.4) |

---

## 3. Spec types

All in `crates/outrider-view/src/spec.rs` (00-framework §2.2 rule: one file owns every serde type).

```rust
/// One Notes layer: a list of notes. `{ "notes": [ ... ] }` on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct NotesSpec(pub Vec<NoteSpec>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NoteSpec {
    pub at: NoteAnchor,
    pub source: NoteSource,
    /// Required for `agent`; optional for `doc`/`metric` (generated when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Pin offset *relative to the symbol's own box*: (0,0)=top-left, (1,1)=bottom-right.
    /// Never a world/screen coordinate. Only affects where the callout hangs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<(f32, f32)>,
    /// Which readout to use for `metric` notes with no text (default "auto", see §4.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readout: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoteSource { Doc, Metric, Agent }

/// Where a note attaches. Untagged so all of these parse:
///   "fn:src/a.rs::verify"                       → Symbol
///   "$focus" | "$hover" | "$selection"          → Live
///   { "symbol": "…", "range": [40, 92] }        → Range (byte range in the symbol's file)
///   { "symbol": "…", "lines": [40, 52] }        → Range (1-based inclusive lines → bytes at resolve time)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NoteAnchor {
    Live(LiveAnchor),                 // must be tried before Symbol: "$focus" is not a valid WireSymbolId
    Symbol(WireSymbolId),
    Range { symbol: WireSymbolId, #[serde(flatten)] span: AnchorSpan },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveAnchor {
    #[serde(rename = "$focus")] Focus,
    #[serde(rename = "$hover")] Hover,
    #[serde(rename = "$selection")] Selection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum AnchorSpan { Range([usize; 2]), Lines([usize; 2]) }   // externally tagged: {"range":[a,b]} | {"lines":[a,b]}
```

`WireSymbolId` is the newtype from 00-framework §2.7. `LiveAnchor` mirrors the `$focus/$hover/$selection` pseudo-ids of 02-set.md so `{ "at": "$hover" }` and `{ "ids": ["$hover"] }` mean the same thing.

JSON examples:

```jsonc
// Session default (00-framework §3.3)
{ "notes": [ { "at": "$hover", "source": "doc" }, { "at": "$focus", "source": "doc" } ] }

// Agent narration on a symbol
{ "notes": [ { "at": "fn:src/auth/login.rs::verify", "source": "agent",
               "text": "This now calls into billing; previously auth had no edge to billing." } ] }

// Range-anchored agent note (Full/Text only), pinned near the top-right of the box
{ "notes": [ { "at": { "symbol": "fn:src/auth/login.rs::verify", "lines": [40, 52] },
               "source": "agent", "text": "new cross-module call", "pin": [1.0, 0.0] } ] }

// Metric readout with an explicit readout id (03-fill's inspect note uses "inspect:<metric>")
{ "notes": [ { "at": "$focus", "source": "metric", "readout": "inventory" } ] }
```

---

## 4. Resolution (`crates/outrider-view/src/layers/notes.rs`)

### 4.1 Output types

```rust
use std::{collections::HashMap, ops::Range};

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedNote {
    pub source: NoteSource,
    pub text: String,                    // never empty after resolution
    pub range: Option<Range<usize>>,     // byte range within the symbol's file (None = whole symbol)
    pub pin: Option<(f32, f32)>,
    pub layer: usize,                    // index into spec.layers, for `layer rm` and stable ordering
}

/// SymbolId → notes, in layer order then note order. Layers accumulate (parent §3.2).
#[derive(Debug, Default, Clone)]
pub struct NoteTable {
    pub by_symbol: HashMap<SymbolId, Vec<ResolvedNote>>,
    pub deps: Deps,
}
impl NoteTable {
    pub fn get(&self, id: &SymbolId) -> &[ResolvedNote];   // &[] when absent
    pub fn is_empty(&self) -> bool;
}
```

### 4.2 The `MetricReadout` registry

Lives in `crates/outrider-view/src/layers/notes.rs` (GPUI-free; only needs `SymbolNode`), so 03-fill and 06-marks can register readouts from the same crate and the app can add its own.

```rust
pub type ReadoutFn = Box<dyn Fn(&SymbolNode, &ReadoutCtx) -> Option<String> + Send + Sync>;
pub struct ReadoutCtx<'a> { pub metrics: &'a MetricRegistry, pub tree: &'a SymbolTree }

#[derive(Default)]
pub struct MetricReadoutRegistry { by_id: BTreeMap<String, ReadoutFn> }
impl MetricReadoutRegistry {
    pub fn builtin() -> Self;                                    // registers the four below + "auto"
    pub fn register(&mut self, id: impl Into<String>, f: ReadoutFn);
    pub fn readout(&self, id: &str, node: &SymbolNode, ctx: &ReadoutCtx) -> Option<String>;
}
```

Built-in ids and their bodies (moved verbatim out of `content.rs`, `#[cfg(test)]` removed; `content.rs` keeps thin `pub fn` wrappers delegating to `outrider_view::layers::notes::readouts::*` so existing tests keep passing):

| id | function | output |
|---|---|---|
| `cardMeta` | `card_meta(node)` | `"{churn_count} · p{churn*100:.0} · {measure}L"` |
| `churn` | `churn_readout(node)` | `"{measure}L · {churn_count} commits · p{churn*100:.0}"` |
| `kindCounts` | `kind_counts(node)` | `"3 files · 1 folder"` / `"2 fns · 1 struct"` |
| `inventory` | `inventory(node)` | `kind_counts + " · " + churn_readout` (or just churn when no kinds) |
| `auto` (default) | leaf item → `churn`; container → `inventory` | what a Card/Detail meta row would show |
| `inspect:<metric>` | registered by **03-fill**: `"{metric} {raw} · p{percentile} · {basis}"` via `ctx.metrics` | the parent §1.3 inspect string |
| `mark:<kind>` | registered by **06-marks**: `"{label} · {basis}"` | glyph hover text |

`MetricReadoutRegistry` is a field on `ResolveCtx`? **No** — 00-framework fixes `ResolveCtx`; instead the registry is a field of `MetricRegistry` (`MetricRegistry::readouts(&self) -> &MetricReadoutRegistry`, `readouts_mut`). This is the one addition to 03-fill's type; see §11.

### 4.3 Algorithm

```rust
pub fn resolve_notes(
    layer_idx: usize, spec: &NotesSpec, ctx: &ResolveCtx, sets: &BTreeMap<String, ResolvedSet>,
    out: &mut NoteTable, warnings: &mut Vec<String>,
)
```
For each `NoteSpec` in order:
1. **Anchor → (SymbolId, Option<Range>)**.
   - `Live(Focus)` → `ctx.session.focus`; `deps |= FOCUS`.
   - `Live(Hover)` → `ctx.session.hover` or skip; `deps |= HOVER`.
   - `Live(Selection)` → `ctx.session.selection` or skip; `deps |= SELECTION`.
   - `Symbol(id)` → `id`; skip with a warning `"notes[i]: unknown symbol <wire>"` when `ctx.index.node(&id)` is `None`. Also accept a bare path via `SetResolver::lookup_bare` (02-set.md).
   - `Range{symbol, span}` → as `Symbol`, plus `range`: `AnchorSpan::Range([a,b])` → `a..b`; `AnchorSpan::Lines([a,b])` → **kept as lines** in `ResolvedNote.range`? No: `outrider-view` has no buffer access, so `Lines` is converted to bytes only when the app supplies a line map. Store `range: Some(a..b)` for byte spans and put line spans in a second field `lines: Option<(usize,usize)>`; the app's `byte_range_to_lines` short-circuits when `lines` is already present. (Add `pub lines: Option<(usize, usize)>` to `ResolvedNote`.)
   - `deps |= TREE` always (ids are tree-relative).
2. **Text**.
   - `Doc`: `text.clone()` or `node.doc.clone()`; if both `None` → skip silently (this is exactly today's `item.node.doc.is_some()` guard).
   - `Metric`: `text.clone()` or `readouts.readout(readout.as_deref().unwrap_or("auto"), node, &ReadoutCtx{..})`; `None`/empty → skip.
   - `Agent`: `text` (validated present, §4.4); empty after `trim()` → skip with warning.
3. Push `ResolvedNote { source, text, range, lines, pin, layer: layer_idx }` onto `out.by_symbol[id]`.

Sets are not consulted (`at` is a single anchor, not a `SetRef`) — this keeps a note unambiguous about *which* symbol it decorates. Multi-symbol narration is a Panel or several notes.

Caching: the whole `NoteTable` is recomputed when `dirty ∩ (SPEC | TREE | table.deps) ≠ ∅`; it is O(#notes) so no per-note cache.

### 4.4 Validation (`validate.rs`)

```rust
// in validate():
for (i, layer) in spec.layers.iter().enumerate() {
    if let LayerSpec::Notes(NotesSpec(notes)) = layer {
        for (j, n) in notes.iter().enumerate() {
            if n.source == NoteSource::Agent && n.text.as_deref().map_or(true, |t| t.trim().is_empty()) {
                out.push(Violation { path: format!("layers[{i}].notes[{j}].text"),
                    rule: "agent-note-needs-text",
                    message: "notes with source \"agent\" must carry text".into() });
            }
            if let Some((px, py)) = n.pin { if !(0.0..=1.0).contains(&px) || !(0.0..=1.0).contains(&py) {
                out.push(Violation { path: format!("layers[{i}].notes[{j}].pin"), rule: "pin-out-of-range",
                    message: "pin is a relative offset in 0..=1".into() }); } }
            if let NoteAnchor::Range { span: AnchorSpan::Lines([a, b]) | AnchorSpan::Range([a, b]), .. } = &n.at {
                if a > b { /* Violation "anchor-range-inverted" */ }
            }
        }
    }
}
```
`readout` ids are validated softly (unknown → warning at resolve time, note skipped), because 03-fill/06-marks register theirs at runtime.

---

## 5. App integration

### 5.1 `paint_items` — replace the doc-panel logic

Add to `PaintOverrides` (00-framework §3.2): `pub fn notes(&self, id: &SymbolId) -> &[ResolvedNote] { self.resolved.notes.get(id) }`.

**Replace L1491** `let mut panel_doc: Option<(String, f32, f32, f32, f32)> = None;` with
```rust
let mut panel_doc: Option<(Vec<ResolvedNote>, f32, f32, f32, f32)> = None; // notes, x, y, w, h of the anchor box
let mut callouts: Vec<NoteCallout> = Vec::new();                          // §5.3
```

**Replace L1657 + L1673–1681** (`is_hovered` and the `if item.node.doc.is_some() && …` block) with:
```rust
let notes = ov.notes(&item.node.id);
let rung_ok = note_rung_ok(&item.draw);                       // Card+ rule, §5.5
if rung_ok && !notes.is_empty() {
    // Panel notes: doc + metric notes on this box go into the single floating panel
    // (today: hover wins, else focus — preserved because the hover layer is resolved
    // for the hovered id only and the focus layer for the focused id only; when they
    // coincide the notes concatenate).
    let panel_notes: Vec<ResolvedNote> = notes.iter()
        .filter(|n| n.range.is_none() && n.lines.is_none() && n.source != NoteSource::Agent)
        .cloned().collect();
    let is_hovered = self.hover_id.as_ref() == Some(&item.node.id);
    if !panel_notes.is_empty() && (is_hovered || panel_doc.is_none()) {
        panel_doc = Some((panel_notes, item.px.x as f32, item.px.y as f32, paint_w, paint_h));
    }
    // Agent notes: in-box rows or callouts (§5.3).
    crate::view::note_pass::place_agent_notes(
        notes, &item, paint_w, paint_h, &mut body, body_font_px, header_bg_h,
        &mut self.buffers, &self.file_symbols, vw, vh, &mut callouts,
    );
}
```
The `filter` at `on_mouse_move` L2053 (`.filter(|i| i.node.doc.is_some())`) is removed so `$hover` notes of any source resolve; hovering a box with no notes now sets `hover_id` too, which is harmless (the `HOVER` bit is set only when the id changes, as today).

**Replace L1713–1742** (`let doc_panel = panel_doc.and_then(..)`) with:
```rust
let doc_panel = panel_doc.and_then(|(notes, fx, fy, fw, fh)|
    crate::view::note_pass::build_doc_panel(&notes, fx, fy, fw, fh));
```
and change the return type of `paint_items` to `(Vec<PaintItem>, Option<DocPanel>, Vec<NoteCallout>, bool)`; the `Render` impl destructures accordingly (L4083).

`build_doc_panel` is the moved body of L1713–1742 with two generalisations: (a) rows come from every note in `notes` in order, each wrapped with `wrap_doc(&n.text, panel_w - 2*BODY_PAD, FONT_PX)`, and metric rows are coloured `theme::TEXT_SECONDARY` instead of `DOC_COLOR`; (b) `panel_w = fw.max(DOC_PANEL_W)` and `panel_y = fy - panel_h - 4.0` are unchanged, so for a doc-only note the produced `DocPanel` is byte-identical to today's (golden test §9).

### 5.2 `paint_model.rs` and `theme.rs`

```rust
// paint_model.rs
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RowStyle { #[default] Code, Doc, Metric, Narration }
pub(crate) struct BodyText { /* existing fields */ pub(crate) style: RowStyle }   // add; every existing constructor sets `style: RowStyle::Code`

/// A floating text block anchored beside a box or a line range. Screen space, this frame only.
pub(crate) struct NoteCallout {
    pub(crate) x: f32, pub(crate) y: f32, pub(crate) w: f32, pub(crate) h: f32,
    pub(crate) rows: Vec<BodyText>,          // style = Narration for agent, Doc/Metric otherwise
    pub(crate) anchor: (f32, f32),           // where the leader line starts (box edge or line's right edge)
    pub(crate) style: RowStyle,
}
```

```rust
// theme.rs — narration tokens (design doc §5.4: distinct face, tint, marker)
/// Narration text: warm off-white on the neutral callout so it never reads as a metric colour.
pub const NARRATION_TEXT: u32 = 0xe8dcc0;
/// Neutral callout background (slightly lighter than CODE_BG; never a heat colour).
pub const NARRATION_BG: u32 = 0x1c1b1e;
/// Callout border / leader line.
pub const NARRATION_BORDER: u32 = 0x5a5560;
/// Leading marker glyph on the first row of every agent note.
pub const NARRATION_MARK: &str = "✎ ";
/// Narration uses the sans face (code is mono; docs are sans+DOC_COLOR; narration is sans+NARRATION_TEXT+marker).
pub const NARRATION_FONT: &str = FONT_FAMILY_SANS;
```
Add the three colours to `fingerprint()` (textures never contain narration, but the fingerprint enumerates every theme colour by convention). Font families are `&str` and cannot be `const`-concatenated across cfgs; if that is awkward, make `NARRATION_FONT` a `pub fn narration_font() -> &'static str`.

In the canvas closure `paint_text` (L4388): pick the font per row — `bt.style == RowStyle::Narration → gpui::font(theme::NARRATION_FONT)`, else `theme::FONT_FAMILY` — by turning the `run` helper into `run_with(font: &str, len, color)`. Nothing else in the closure changes.

### 5.3 New file `crates/outrider/src/view/note_pass.rs`

```rust
pub(crate) const DOC_PANEL_W: f64 = 280.0;          // moved from treemap.rs L60
pub(crate) const CALLOUT_W: f64 = 260.0;
pub(crate) const CALLOUT_GAP: f32 = 8.0;
pub(crate) const CALLOUT_MAX_ROWS: usize = 6;

/// L1713–1742, generalised. Doc rows: DOC_COLOR; metric rows: TEXT_SECONDARY. Returns None if nothing wraps.
pub(crate) fn build_doc_panel(notes: &[ResolvedNote], fx: f32, fy: f32, fw: f32, fh: f32) -> Option<DocPanel>;

/// True iff the box is at Card fidelity or finer (parent §3.2 "Notes at Card+ only").
pub(crate) fn note_rung_ok(draw: &Draw) -> bool {
    match draw {
        Draw::Container(r) => matches!(r, Rung::Card | Rung::Detail | Rung::Full),
        Draw::Leaf(t)      => matches!(t, LeafDraw::Text | LeafDraw::Minimap),  // Minimap/Text ≥ CARD_PX by world::leaf_draw
    }
}

/// Byte range in `id`'s file → half-open display-line range, exactly the arithmetic of treemap.rs L1467–1477.
pub(crate) fn byte_range_to_lines(
    id: &SymbolId, range: &Range<usize>, buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> Option<Range<usize>> {
    let rel = BufferManager::file_path_of(&id.qualified_path).to_string();
    let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
    let m = buffers.get(&rel, syms)?;
    let start = m.buffer.byte_to_line(range.start);
    let end = m.buffer.byte_to_line(range.end.saturating_sub(1)) + 1;
    Some(start..end)
}

/// Decide, per agent note on one box, between an in-box row block and a callout.
#[allow(clippy::too_many_arguments)]
pub(crate) fn place_agent_notes(
    notes: &[ResolvedNote], item: &DrawItem, paint_w: f32, paint_h: f32,
    body: &mut Vec<BodyText>, body_font_px: f32, header_bg_h: f32,
    buffers: &mut BufferManager, file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
    vw: f64, vh: f64, callouts: &mut Vec<NoteCallout>,
);
```

Placement rules inside `place_agent_notes` (all agent notes; doc/metric never come here):

1. **Symbol-anchored, Detail/Full container or Text leaf, fits** — "fits" = `rows.len() * LINE_STEP + BODY_PAD ≤ paint_h - header_bg_h - existing_body_extent`, where rows = `wrap_doc(&text, paint_w - 2*BODY_PAD, FONT_PX)` capped at `CALLOUT_MAX_ROWS` (last row gets `…`). Push rows into `body` **below the header and below any existing body rows** at `x = item.px.x + BODY_PAD`, `y = header_bottom + existing_extent + BODY_PAD + i*LINE_STEP`, `style: Narration`, first row prefixed with `theme::NARRATION_MARK`, `runs = [(len, NARRATION_TEXT)]`, `highlighted: false`. For a Text leaf the rows go *after the last source row* (leaf pages are unclipped, so this extends the page — same mechanism as `focused_extra_h`; return the extra row count so the caller can add it to `focused_extra_h` when focused; when not focused, rows past the box are clipped by `clip_h`, which is acceptable).
2. **Symbol-anchored, Card, or doesn't fit** — a callout: `x = box_right + CALLOUT_GAP` (or `box_left - CALLOUT_W - CALLOUT_GAP` when the right side is off-screen), `y = box_top + pin.1 * paint_h` (default `pin = (1.0, 0.0)` → top-right); `w = CALLOUT_W`; rows wrapped to `CALLOUT_W - 2*BODY_PAD`; `anchor = (box_right, y)`. Pin `(px, py)` maps to `anchor = (box_left + px*paint_w, box_top + py*paint_h)`; the callout is placed to the right of the anchor when `px ≥ 0.5`, else to the left.
3. **Range-anchored (`range`/`lines`)** — only when the leaf is drawn as `LeafDraw::Text` (design doc §5.2 Full "anchor-attached inline summary boxes"); at any other rung the note is **dropped for this frame** (not turned into a symbol callout — a range note without visible lines would mislead). Lines = `lines` if set, else `byte_range_to_lines(..)`; then `symbol_start_line` offset gives the display row `r = first_line - symbol_start_line`; `y = item.top + HEADER*scale + r * LINE_STEP*scale` (the same `content_y0 + display_row*step` formula as `leaf_text_body`); `x = box_right + CALLOUT_GAP`; `anchor = (box_right, y + LINE_STEP*scale/2)`. This is the "call-site summary box" the design doc describes.
4. **Overlap avoidance (simple)** — callouts are appended per box in note order; before pushing, if the new callout's `y` overlaps the previous callout *of the same box*, set `y = prev.y + prev.h + CALLOUT_GAP`. Callouts fully outside `[0,vw]×[0,vh]` are skipped. No global packing.

Painting — add **Pass 5** in the canvas closure immediately after Pass 4 (L4602):
```rust
// Pass 5: note callouts (agent narration and off-box doc/metric callouts).
if !cg_scrim {                                   // until 04-mask replaces cg_scrim; then unconditional
    for c in &callouts {
        window.paint_quad(quad(bounds_of(c), px(theme::CORNER_RADIUS), rgb(theme::NARRATION_BG),
                               px(1.0), rgb(theme::NARRATION_BORDER), BorderStyle::default()));
        // leader: a 1px quad from c.anchor to the callout's left edge at c.y + FONT_PX
        for bt in &c.rows { /* shape_line with NARRATION_FONT / DOC font per bt.style, FONT_PX, paint at (bt.x, bt.y) */ }
    }
}
```
The pass paints **only** text, one neutral quad, and a leader line. It reads `PaintItem`s for nothing and writes nothing back — the guarantee that a note cannot touch fill/border/stripe holds by construction: `ResolvedNote` has no colour field, `PaintOverrides::notes` is consulted only in this block and §5.1, and `paint_decisions` (00-framework §4) never receives the note table.

### 5.4 `container_body` (metric rows) — deferred hook

`container_body` returns nothing today and Card/Detail readouts are test-only, so **behaviour-preserving** means: do not draw metric rows by default. Provide the hook so a view can opt in: when `ov.notes(id)` contains `source: Metric` notes on a container at Card/Detail, `place_metric_rows(..)` (same shape as rule 1 above, `style: Metric`, colour `TEXT_SECONDARY`, no marker) pushes them into `body` under the header. The session default view carries no metric notes, so nothing changes visually until a user/agent adds `{ at: "$focus", source: "metric" }` or 03-fill's inspect note fires.

### 5.5 Rung gating and dependencies

`note_rung_ok` is the single place the Card+ rule lives; both the panel branch and `place_agent_notes` go through it. `paint_items` sets nothing; the `HOVER`/`FOCUS`/`SELECTION` bits are already wired by 00-framework §3.4 and 08-panel.

---

## 6. Commands

| verb / command | effect |
|---|---|
| `outrider note <symbol[:a-b]> "text"` (parent §6.2) | `ViewCommand::PushLayer(LayerSpec::Notes(NotesSpec(vec![NoteSpec{ at, source: Agent, text: Some(..), pin: None, readout: None }])))`; `<symbol>:40-52` → `AnchorSpan::Lines([40,52])`. The CLI **always** forces `source: agent`; `doc`/`metric` notes come only from documents (`view apply/patch`) or app presets. |
| `outrider layer rm <i>` / `layer pop` | removes the layer; `ResolvedNote.layer` lets the app map a painted callout back to its layer (future "dismiss" affordance). |
| `view apply/patch` | any `notes` layer in the document. |
| app: hover / focus | no command; the default view's `$hover`/`$focus` notes re-resolve on `HOVER`/`FOCUS`. |
| app: 03-fill inspect click, 06-marks glyph hover | `PushLayer(Notes([{ at: "$hover"|"$selection", source: Metric, readout: "inspect:churn" }]))` (they push and pop their own layers). |

---

## 7. Invalidation

| bit | set by | consumed here |
|---|---|---|
| `TREE` | loader / packing snapshot (00-framework §3.4) | ids re-looked-up; readouts recomputed |
| `FOCUS` | focus changes | `$focus` anchors |
| `HOVER` | `on_mouse_move` when `hover_id` changes | `$hover` anchors |
| `SELECTION` | 08-panel row moves | `$selection` anchors |
| `SPEC` | `apply_view_command` | notes layers added/removed |
| `GIT` / `METRICS` | git watcher / import | only via `metric` readouts that read `ctx.metrics` (`inspect:*`); the table records `Deps::GIT | Deps::METRICS` when any metric note is present |
| `CAMERA` | — | **not** a resolution dependency: rung gating and callout placement are per-frame paint decisions, not resolution |

`NoteTable.deps` = union of the above for the notes actually present, so a view with only `agent` notes on explicit ids re-resolves only on `TREE|SPEC`.

---

## 8. Migration steps

1. **Types + validation.** Add §3 types to `spec.rs`; implement §4.4 in `validate.rs`; unit tests `notes_json_roundtrip`, `agent_note_without_text_rejected`. No app change.
2. **Readout registry.** Un-`cfg` `card_meta/churn_readout/kind_counts/inventory` in `content.rs` (keep `body_lines`/`BodyLine` test-only), move bodies to `outrider-view/src/layers/notes.rs::readouts`, leave `content.rs` wrappers; add `MetricReadoutRegistry::builtin()`; hang it off `MetricRegistry` (§11). Tests `readout_auto_leaf_is_churn`, `readout_auto_container_is_inventory` (assert equality with the `content.rs` wrappers on the existing `content::tests::file()`/`folder()` fixtures).
3. **Resolver.** `resolve_notes` + `NoteTable` wired into `ViewResolver::resolve` step (6); `PaintOverrides::notes`. Test `doc_note_equals_node_doc` on `mini_repo`.
4. **note_pass.rs (panel half).** Create the file with `DOC_PANEL_W`, `build_doc_panel`, `note_rung_ok`, `byte_range_to_lines`. Replace L1491 / L1657+1673–1681 / L1713–1742 as in §5.1; remove the `doc.is_some()` hover filter at L2053; extend `paint_items`' return tuple (callouts empty for now). **Acceptance: hover tooltip and focused-file panel look identical** (golden §9). Delete `DOC_PANEL_W` from treemap.rs.
5. **Narration.** `RowStyle`/`NoteCallout` in `paint_model.rs`; `NARRATION_*` in `theme.rs`; `place_agent_notes` + Pass 5; per-row font in `paint_text`. Manual acceptance §9.
6. **Metric rows hook (§5.4)** — optional, same milestone if cheap.
7. **CLI `note` verb** lands with 10-rpc-and-watcher.md; nothing here blocks it.
8. Refactor `cg_highlight_lines` (L1458–1478) to call `byte_range_to_lines` — happens in 06-marks/05-edges when the call-site highlight becomes a range Mark; not required here but the helper is ready.

Nothing is deleted before step 4, and step 4 deletes only the two replaced blocks and the constant.

---

## 9. Tests

`outrider-view` (in `crates/outrider-view/src/layers/notes.rs` `#[cfg(test)]` and `tests/notes.rs`):
- `notes_json_roundtrip` — the four JSON examples in §3 parse and re-serialise equal (`NoteAnchor` untagged order: `$focus` parses as `Live`, `fn:…` as `Symbol`, object as `Range`).
- `agent_note_without_text_rejected` — `validate` returns `rule == "agent-note-needs-text"` at path `layers[0].notes[0].text`; `text: "   "` also rejected; `doc` without text accepted.
- `pin_out_of_range_rejected`, `anchor_range_inverted_rejected`.
- `doc_note_equals_node_doc` — fixture `mini_repo` (via `outrider_index::index_repo`), view `{ notes: [{ at: "$focus", source: "doc" }] }` with `session.focus` = a file that has a `//!` doc: `table.get(id)[0].text == node.doc.unwrap()`; a file with no doc → no entry.
- `hover_note_resolves_only_hovered` — `$hover` note present iff `session.hover.is_some()`, keyed on that id; `deps` contains `HOVER`.
- `metric_note_auto_readout` — leaf fn → equals `churn_readout(node)`; folder → equals `inventory(node)`; explicit `readout: "cardMeta"` → `card_meta`.
- `unknown_readout_skips_with_warning`.
- `layers_accumulate_in_order` — two notes layers on the same id → two entries with `layer` 0 and 1.

App crate (`crates/outrider/src/view/note_pass.rs` tests, pure functions):
- `doc_panel_golden` — capture today's `DocPanel` output for a fixed `(doc, fx, fy, fw, fh)` (`doc = "Doc first.\n\nDoc second …"` long enough to wrap; `fw = 120`) *before* step 4 by copying L1713–1742 into the test as `legacy_doc_panel`; assert `build_doc_panel(&[doc_note], ..)` yields equal `x,y,w,h` and equal `rows[i].{x,y,text,runs}`.
- `wrapping_golden` — `wrap_doc("✎ " + text, CALLOUT_W - 2*BODY_PAD, FONT_PX)` on a fixed 200-char narration → fixed row texts (asserts the marker stays on row 0 and no row exceeds `char_budget`).
- `rung_gate` — `note_rung_ok(Draw::Container(Rung::Label)) == false`, `Card/Detail/Full == true`, `Leaf(Dot|Label) == false`, `Leaf(Text|Minimap) == true`.
- `byte_range_to_lines_matches_cg` — on a temp file materialised through `BufferManager`, `byte_range_to_lines` equals the inline arithmetic copied from L1467–1477.
- `callout_stacks_per_box` — two agent notes on one Card box → second callout `y == first.y + first.h + CALLOUT_GAP`.
- `range_note_dropped_below_text` — range note on a `LeafDraw::Minimap` item produces no callout and no body rows.
- `notes_never_touch_paint_decisions` — 00-framework's `paint_decisions` test extended: a `ResolvedView` with agent+metric+doc notes on every id yields the same `(fill, border, stripe, focused, neighbor, light)` as one with an empty `NoteTable`.

Manual acceptance: build; hover a documented file → identical tooltip; focus it → identical panel; `outrider note src/main.rs "hello"` (once RPC lands) or a `.outrider/views/x.json` with an agent note → "✎ hello" appears inside the box at Detail/Full and as a callout at Card; zoom out to Label → gone; fill/stripe unchanged with and without notes; range note on a focused fn shows a callout beside the lines.

---

## 10. Open questions / risks

- **Panel vs. callout for doc notes on `$hover` when hovering a Card-sized box far right of the screen** — today the panel can go off-screen too; unchanged. A later polish could route off-screen panels through the callout side-flip in rule 2.
- **Leaf pages extended by in-box narration** (rule 1) change `paint_h` for the focused leaf only via `focused_extra_h`; for non-focused Text leaves rows are clipped. Alternative: always callout for leaves. Decide by feel.
- **`Lines` anchors need a buffer at paint time**; a range note on a file that failed to materialise silently drops. Acceptable; a warning surfaces via `ResolvedView.warnings` only for id-level failures.
- **`hover_id` now set for any hovered node** (filter removed) — the `HOVER` bit fires more often. Cost is one `NoteTable` recompute (O(#notes)); fine.
- The `readout` field is an extension beyond parent §2.7's parameter list; it exists so 03-fill/06-marks can plug in without new note sources. Flagged in §11.

---

## 11. Framework deltas

1. **`MetricRegistry` gains a readout registry** (`readouts()` / `readouts_mut()` returning `MetricReadoutRegistry`) so `ResolveCtx` stays as defined in 00-framework §2.4. 03-fill.md should own the field; this spec defines the type. If 03-fill prefers a separate `ResolveCtx.readouts` field, that is a one-line change here.
2. **`ResolvedNote` carries `layer: usize` and `lines: Option<(usize,usize)>`** — additions to the shape sketched in the task brief; no other spec depends on the shape.
3. **`NoteSpec.readout: Option<String>`** — schema extension (parent §5 doesn't list it). Backwards compatible (`default`).
4. `paint_items` return tuple gains `Vec<NoteCallout>`; `TreemapView` gains no new fields.
