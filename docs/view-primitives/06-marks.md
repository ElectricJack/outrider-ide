# 06 — Marks: corner glyphs, range highlights, and the navigation rings

**Parent:** [../view-primitives.md](../view-primitives.md) §2.6, §3.2 (corner glyphs / range highlight channels), §3.3 (structural mark needs basis), §4 (focus ring / neighbor rows, call-graph call-site row), §8.6, §9 milestone 1 (nav kinds) and 4 (call-site range)
**Framework:** [00-framework.md](00-framework.md) — uses `ViewSpec`, `LayerSpec::Marks`, `SetRef`, `Deps`, `SessionState`, `ResolveCtx`, `ResolvedView.marks: MarkTable`, `ViewResolver`, `ViewCommand`, `validate` (structural mark needs basis is already a hard rule there), `PaintOverrides::{glyphs, range_marks, is_focus_ring, is_neighbor}`, `PaintItem.glyphs / range_marks`, `default_view` (focusRing / neighbor layers). Deltas in §11.
**Depends on:** 02-set.md (`SetRef`, `$focus`, `neighbors(focus)`, `$selectionSite`), 03-fill.md (shares the line-table loader, §4.3), 07-notes.md (glyph hover text), 05-edges.md (`selection` range mark in the call-graph preset).

---

## 1. Purpose and scope

**Marks** are the discrete channel (parent §2.6): a corner badge on a symbol's box, or an outline over an anchor range inside a symbol's body at text fidelity. Structural kinds (`cycle`, `layeringViolation`, `hotspot`, `custom`) are assessments and must carry a `basis` (§3.3); `agentFlag` is the neutral "an agent pointed here" style; the navigation kinds `focusRing`, `neighbor`, `selection` are what today's focus ring, neighbor rings, and call-graph call-site highlight become (parent §4).

This spec defines `MarksSpec`/`MarkKind` and their JSON, `MarkTable` and its resolution (including `lines → bytes`), the glyph slot rules, the app-side `mark_pass.rs`, the generalisation of `cg_highlight_lines` to N ranges per symbol, and the **Milestone-0 subset** (nav kinds only) that 00-framework §5 step 1 needs, kept in its own section (§8.1) so it can land first.

---

## 2. Ground truth: existing code touched

| File | Symbol | ~Line | What it does today | What changes |
|---|---|---|---|---|
| `crates/outrider/src/treemap.rs` | `TreemapView.neighbors: Option<(SymbolId, [Option<SymbolId>;4])>` | 531 | Cached `focus::neighbors` result, keyed by focus; cleared on focus change (2319, 2329, …) and by `PackingGeometryState::invalidate` (582). | **Kept** (arrow-key navigation, and it feeds `SessionState.neighbors`). Only its use as a paint boolean goes. |
| same | `paint_items` — `neighbor_ids` local; `PaintItem { focused: is_focused, neighbor: !is_focused && neighbor_ids…any(..) }` | 1456, 1693–1695 | Boolean ring flags computed inline. | `focused: ov.is_focus_ring(&id)`, `neighbor: ov.is_neighbor(&id)`; `neighbor_ids` local deleted. `is_focused` local stays for §2.10 widening / `deferred_overlay` (1585–1594, 1694). |
| same | `paint_items` — `cg_highlight_lines: Option<Range<usize>>` | 1458–1478 | Selected callee's `call_site` bytes → line range via `BufferManager::get` + `byte_to_line`; passed only for the focused leaf (1596–1600). | Deleted; every Text-tier leaf gets `ov.range_marks(&id)` (§5.4). |
| same | `leaf_text_body(.., focused: bool, highlight_lines: Option<Range<usize>>) -> (Vec<BodyText>, usize)` | 837–909 | Per file line, `hl = highlight_lines.contains(file_line)` → `BodyText.highlighted`. | Signature takes `marks: &[(Range<usize>, MarkStyle)]`; converts each byte range to a line range once, sets `BodyText.mark: Option<MarkStyle>` (§5.4). |
| same | `Self::pinned_name(item, center, pin_y, shift_as_header)` | 1404–1428 | Truncates the name to `item.label_w`. | Gains `reserved_right: f32` so glyph slots never overlap the name (§5.2). |
| same | canvas closure `paint_text` highlight quad | 4410–4425 | `rgba(0x4488ff30)` rounded quad behind a highlighted row. | Colour from `theme::range_mark_bg(style)`; condition `bt.mark.is_some()`. |
| same | `paint_ring`, pass 2c (neighbor rings), pass 3 (focus ring), `ring_paints_after_leaf_overlay` | 4509–4557, 933–935 | 2 px `FOCUS_BORDER` for `focused`, 1 px `NEIGHBOR_BORDER` otherwise; 2c skipped under `cg_scrim`. | Unchanged geometry; inputs come from overrides; 2c is unconditional (scrim gone, 05-edges); ring alpha × `item.light` (04-mask). Adds `selection` ring (§5.3). |
| same | `on_mouse_move` hover hit test | 2029–2060 | `hover_id` = hit node with a doc. | Adds glyph hit test → `hover_glyph` (§5.5). |
| `crates/outrider/src/paint_model.rs` | `BodyText.highlighted: bool` | 15 | | → `mark: Option<MarkStyle>`. |
| same | `PaintItem { focused, neighbor, .. }` | 41–62 | | Adds `glyphs: Vec<Glyph>`, `range_marks: Vec<(Range<usize>, MarkStyle)>` (00 §3.2), `selection: bool`. |
| `crates/outrider/src/theme.rs` | `FOCUS_BORDER 0x4da6ff`, `NEIGHBOR_BORDER 0xffffff80`, `CORNER_RADIUS 4.0`, `TEXT_PRIMARY/SECONDARY`, `CODE_BG` | 19, 281, 155 | | Adds `MARK_*` colours, `range_mark_bg`, `glyph_bg` (§5.2). |
| `crates/outrider/src/world.rs` | `Draw::Container(Rung)`, `Draw::Leaf(LeafDraw)`, `Rung::{Dot,Label,Card,Detail,Full}`, `LeafDraw::{Dot,Label,Minimap,Text}` | 49–128 | LOD ladder. | Glyphs painted iff Label-or-better; range marks iff `LeafDraw::Text` (the `use_text` branch, treemap.rs 1582). |
| `crates/outrider/src/buffers.rs` | `BufferManager::get(rel, syms) -> Option<&Materialized>`, `Materialized.buffer.byte_to_line`, `symbol_start_line`, `file_path_of` | 50–77, 25 | Rope-backed line mapping used at paint time. | Unchanged; used by `leaf_text_body` for byte→display-row mapping. `outrider-view` cannot use it (GPUI-free crate, and it's app-side) — hence the small line table in §4.3. |
| `crates/outrider/src/focus.rs` | `neighbors(current, pack, index) -> [Option<SymbolId>;4]` | 214–221 | | Unchanged; result reaches the resolver via `SessionState.neighbors` (00 §2.4). |
| `crates/outrider-index/src/types.rs` | `SymbolNode.byte_range: Option<Range<usize>>` (file-relative) | 53 | | Range marks are validated/clipped against it. |

---

## 3. Spec types (`crates/outrider-view/src/spec.rs`)

```rust
/// `{ "marks": { "on": ..., "kind": ..., "label"?: ..., "basis"?: ... } }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarksSpec {
    pub on: MarkTarget,
    pub kind: MarkKind,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub basis: Option<String>,
}
impl Default for MarksSpec { /* on: Anchors(vec![]), kind: AgentFlag */ }

/// A set reference (name or inline expr) or an explicit anchor list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MarkTarget { Set(SetRef), Anchors(Vec<MarkAnchor>) }
// untagged order matters: SetRef::Name (string) → SetRef::Inline (object) → Anchors (array). No overlap.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkAnchor {
    pub symbol: WireSymbolId,
    /// Byte range relative to the symbol's FILE (parent §5), half-open.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub range: Option<[usize; 2]>,
    /// 1-based inclusive line pair; converted by the loader (§4.3). Mutually exclusive with `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub lines: Option<[usize; 2]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MarkKind {
    // structural — basis required (validate.rs, 00 §2.6)
    Cycle,
    #[serde(alias = "layering-violation")] LayeringViolation,
    Hotspot,
    Custom(String),                          // {"custom": "dead-code"}
    // agent
    #[serde(alias = "agent-flag")] AgentFlag,
    // navigation
    #[serde(alias = "focus-ring")] FocusRing,
    Neighbor,
    Selection,
}
impl MarkKind {
    pub fn is_structural(&self) -> bool;    // Cycle | LayeringViolation | Hotspot | Custom
    pub fn is_nav(&self) -> bool;           // FocusRing | Neighbor | Selection
    pub fn style(&self) -> MarkStyle;       // structural → Structural, AgentFlag → Agent, nav → Nav
    /// 1–2 char glyph text for corner slots; None for nav kinds (they draw rings, not glyphs).
    pub fn glyph_text(&self) -> Option<String>;   // Cycle "↻", LayeringViolation "↑", Hotspot "!", Custom(n) → first char of n uppercased, AgentFlag "◆"
}

/// Style class shared by glyphs, range highlights, and rings. Lives in outrider-view (no colours here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MarkStyle { Structural, Agent, Nav, Overflow }
```

The `alias`es accept the kebab-case spellings used in the parent §5 example (`"agent-flag"`); serialisation always emits camelCase.

JSON examples:

```jsonc
{ "marks": { "on": "hot", "kind": "hotspot", "label": "complexity×churn", "basis": "p90 both" } }
{ "marks": { "on": { "intersect": [ { "ref": "cyclic" }, { "kind": "file" } ] }, "kind": "cycle",
             "label": "import cycle", "basis": "tarjan over imports @HEAD" } }
{ "marks": { "on": [ { "symbol": "fn:src/auth/login.rs::verify", "lines": [40, 52] } ],
             "kind": "agentFlag", "label": "new cross-module call" } }
{ "marks": { "on": [ { "symbol": "src/auth/login.rs::verify", "range": [1220, 1287] } ],
             "kind": { "custom": "unsafe" }, "label": "unsafe block", "basis": "grep unsafe @HEAD" } }
{ "marks": { "on": "focusSet",  "kind": "focusRing" } }      // session default view (00 §3.3)
{ "marks": { "on": "neighbors", "kind": "neighbor" } }
{ "marks": { "on": "cgSite",    "kind": "selection" } }      // call-graph preset (05-edges §5.5)
```

Validation (`validate.rs`): structural kind with `basis: None` → hard `Violation { path: "layers[i].marks.basis", rule: "structural mark needs basis" }` (already listed in 00 §2.6); anchor with both `range` and `lines`, or neither → hard `"marks.on[j]"`; `lines[0] == 0 || lines[0] > lines[1]` → hard; `range[0] >= range[1]` → hard; nav kind with `label`/`basis` → soft warning (ignored); `on: Anchors([])` → soft (empty layer).

---

## 4. Resolution (`crates/outrider-view/src/layers/marks.rs`)

### 4.1 Types

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub kind: MarkKind,
    pub label: Option<String>,
    pub basis: Option<String>,
    pub layer_index: usize,          // spec.layers position — order within a slot list and hover attribution
}

/// One corner badge, computed at resolution so the app just paints it.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub text: String,                // 1–2 chars ("!" / "↻" / "+3")
    pub style: MarkStyle,
    pub note: String,                // "label · basis" (basis omitted if None) — hover text (07-notes.md `metric` source)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarkTable {
    pub by_symbol: HashMap<SymbolId, Vec<Mark>>,                       // whole-symbol marks (all kinds)
    pub ranges: HashMap<SymbolId, Vec<(Range<usize>, Mark)>>,          // range marks, file-relative bytes
    pub glyphs: HashMap<SymbolId, Vec<Glyph>>,                         // ≤ 3 entries: up to 2 slots + "+n"
    pub deps: Deps,
}
impl MarkTable {
    pub fn has_kind(&self, id: &SymbolId, kind: &MarkKind) -> bool;
    pub fn glyphs(&self, id: &SymbolId) -> &[Glyph];
    pub fn range_marks(&self, id: &SymbolId) -> impl Iterator<Item = (Range<usize>, MarkStyle)> + '_;
}
pub const GLYPH_SLOTS: usize = 2;
```

### 4.2 Algorithm — `resolve_marks(specs: &[(usize, &MarksSpec)], sets, ctx, lines: &mut LineTables, warnings) -> MarkTable`

All `marks` layers resolve into **one** `MarkTable` (they accumulate, parent §3.2), in `spec.layers` order:

1. For `on: Set(r)`: `set = sets[r]` (inline exprs were resolved by the set pass, 02-set.md). For each `id ∈ set.ids` push `Mark` to `by_symbol[id]`; for each `(id, ranges) ∈ set.ranges` push `(range, mark)` to `ranges[id]` (this is how `$selectionSite` becomes the call-site range mark). Deps `∪= set.deps`.
2. For `on: Anchors(list)`: each anchor → `id = ctx.index` lookup of `symbol` (wire or bare path); missing → warning `"marks[i].on[j]: unknown symbol"` and skip. If neither `range` nor `lines` → whole-symbol mark. `range` → bytes as given. `lines: [a,b]` → `lines.get(repo_root, file_of(id))?.byte_range_of_lines(a, b)` (§4.3); unreadable file → warning, skip. Then clip to `node.byte_range` (warning `"range outside symbol; clipped"` if it had to clip; drop if empty after clipping). Deps `SPEC` (`TREE` implicitly: the whole view is dropped on TREE).
3. **Glyph slots** per symbol (`compute_glyphs(&[Mark]) -> Vec<Glyph>`): take marks with `kind.glyph_text().is_some()`; stable-sort structural before agent, then by `layer_index`; first `GLYPH_SLOTS` become `Glyph { text, style, note }`; if `n > GLYPH_SLOTS`, the **last slot** is replaced by `Glyph { text: format!("+{}", n - GLYPH_SLOTS + 1), style: Overflow, note: <all remaining notes joined by " / "> }` — so 3 marks show `[m1, +2]`, 2 marks show `[m1, m2]`. Nav kinds never occupy a slot. Range-only marks (from step 2 with a range, or set ranges) do **not** produce a corner glyph on the symbol; they are visible only at text fidelity — but a whole-symbol mark of the same layer does.
4. `note = match (label, basis) { (Some(l), Some(b)) => "l · b", (Some(l), None) => l, (None, Some(b)) => "kind · b", (None, None) => kind name }`.

Cache: `ViewResolver` recomputes the whole `MarkTable` when `dirty ∩ deps ≠ ∅` — cheap (it's a few hash inserts) so no per-layer caching.

### 4.3 `lines.rs` — the shared line-offset table (also used by 03-fill's `file:line` importer)

```rust
// crates/outrider-view/src/lines.rs
pub struct LineTable { starts: Vec<usize>, len: usize }        // starts[0] = 0; one entry per line
impl LineTable {
    pub fn from_bytes(bytes: &[u8]) -> Self;                     // split on '\n'; '\r' left in place (ranges are bytes)
    pub fn line_count(&self) -> usize;
    /// 1-based line → byte offset of its first byte; None if out of range.
    pub fn line_start(&self, line1: usize) -> Option<usize>;
    /// 0-based line index containing `byte` (clamped), same contract as FileBuffer::byte_to_line.
    pub fn byte_to_line(&self, byte: usize) -> usize;
    /// [a,b] 1-based inclusive → half-open byte range covering those lines incl. the trailing '\n' of b.
    pub fn byte_range_of_lines(&self, a: usize, b: usize) -> Option<Range<usize>>;
}
/// Lazy per-file loader; lives in ViewResolver (dropped on TREE, and by an explicit `clear()` on GIT).
#[derive(Default)]
pub struct LineTables { cache: HashMap<PathBuf, Option<LineTable>> }
impl LineTables {
    pub fn get(&mut self, repo_root: &Path, rel: &str) -> Option<&LineTable>;   // std::fs::read on miss
    pub fn clear(&mut self);
}
```

03-fill.md's importer (`file:line` keys → enclosing symbol) must call `LineTables::get(..).line_start(line)` and then find the deepest item whose `byte_range` contains it — same table, one loader instance on `ViewResolver` (`resolver.lines`). Not `BufferManager`: that is app-side, GPUI-adjacent, LRU-bounded, and materialises highlight spans — far heavier than a `Vec<usize>` per file.

### 4.4 `PaintOverrides` (00 §3.2) — the four accessors

```rust
pub fn glyphs(&self, id) -> &[Glyph]                          { self.resolved.marks.glyphs(id) }
pub fn range_marks(&self, id) -> Vec<(Range<usize>, MarkStyle)> { self.resolved.marks.range_marks(id).collect() }  // 00 wrote `&[..]`; a Vec is fine (tiny; only Text-tier leaves ask)
pub fn is_focus_ring(&self, id) -> bool                        { self.resolved.marks.has_kind(id, &MarkKind::FocusRing) }
pub fn is_neighbor(&self, id) -> bool                          { !self.is_focus_ring(id) && self.resolved.marks.has_kind(id, &MarkKind::Neighbor) }
pub fn is_selection(&self, id) -> bool                         { self.resolved.marks.has_kind(id, &MarkKind::Selection) }   // whole-symbol selection → ring
```

`is_neighbor` excludes the focus (today's `!is_focused && …` at L1695): the live `neighbors(focus)` set never contains the focus, but a user-defined `neighbor` layer might.

---

## 5. App integration

### 5.1 `paint_model.rs`

```rust
pub(crate) struct BodyText { x, y, text, runs, pub(crate) mark: Option<MarkStyle> }   // replaces `highlighted: bool`
pub(crate) struct PaintItem {
    ..existing..,
    pub(crate) focused: bool,      // now = ov.is_focus_ring
    pub(crate) neighbor: bool,     // now = ov.is_neighbor
    pub(crate) selection: bool,    // new: whole-symbol `selection` mark → thin ring
    pub(crate) glyphs: Vec<Glyph>,                          // empty below Label
    pub(crate) range_marks: Vec<(Range<usize>, MarkStyle)>, // kept for tests/hover; consumed by leaf_text_body
}
```

Every `BodyText { .., highlighted: false }` literal (doc panel rows L1731; tests) becomes `mark: None`.

### 5.2 `view/mark_pass.rs` — corner glyphs

```rust
pub(crate) const GLYPH_W: f32 = 14.0;  pub(crate) const GLYPH_H: f32 = 14.0;
pub(crate) const GLYPH_GAP: f32 = 2.0; pub(crate) const GLYPH_PAD: f32 = 3.0;
pub(crate) const GLYPH_FONT_PX: f32 = 10.0;

/// Screen rects of the slots for one item, right-aligned in the top-right corner. Slot 0 is rightmost.
pub(crate) fn glyph_rects(item: &PaintItem) -> Vec<(f32, f32, f32, f32)>;   // (x, y, w, h)
/// Width the name row must leave free: n * GLYPH_W + (n-1) * GLYPH_GAP + 2 * GLYPH_PAD, or 0.
pub(crate) fn reserved_right(n_glyphs: usize) -> f32;
/// Paints background quad + text for every item's glyphs. Called from the canvas closure.
pub(crate) fn paint(items: &[PaintItem], origin: Point<Pixels>, window: &mut Window, cx: &mut App);
/// Hit test for hover: (item index, glyph index).
pub(crate) fn hit(items: &[PaintItem], mx: f32, my: f32) -> Option<(usize, usize)>;
```

`paint`: for each item with `!glyphs.is_empty()`, for each `(rect, glyph)`: `window.paint_quad(quad(rect, px(3.0), rgb(theme::glyph_bg(glyph.style)), px(0.), transparent_black(), ..))`, then `window.text_system().shape_line(glyph.text, px(GLYPH_FONT_PX), &[TextRun{ font: gpui::font(theme::FONT_FAMILY_SANS), color: rgb(theme::glyph_fg(style)) }], None).paint(rect.origin + (centered), px(GLYPH_FONT_PX*1.3), TextAlign::Center, ..)`. Wrapped in `window.with_content_mask(Some(item_content_mask(item)), ..)` so glyphs clip with their box. Alpha × `item.light` (04-mask.md) so dimmed boxes have dimmed badges.

Where in the closure: **after pass 2b (headers) and after the edge pass (05-edges §5.3), before pass 2c (rings)** — glyphs must sit above header backgrounds (headers paint opaque quads at 2b) and above edges, below the focus ring and the deferred focused leaf. Items with `deferred_overlay` paint their glyphs again in pass 2d after `paint_text` so the widened focus leaf keeps its badges on top.

Population in `paint_items`: `glyphs: if label_or_better(item.draw) && item.px.w >= 3.0 * GLYPH_W { ov.glyphs(&id).to_vec() } else { vec![] }` where `label_or_better` = `Container(r) if r != Rung::Dot` or `Leaf(t) if t != LeafDraw::Dot`. `pinned_name(.., reserved_right(glyphs.len()))` subtracts that width from `item.label_w` before `truncate_to_width` (treemap.rs:1411).

`theme.rs`:
```rust
pub const MARK_STRUCTURAL: u32 = 0xd9822b;   // amber — assessment
pub const MARK_AGENT: u32 = 0x8e7cc3;        // violet — narration-adjacent, distinct from any metric ramp
pub const MARK_OVERFLOW: u32 = 0x3a3a40;
pub const MARK_SELECTION_BG: u32 = 0x4488ff30;   // extracted from treemap.rs:4420
pub fn glyph_bg(style: MarkStyle) -> u32 { Structural → MARK_STRUCTURAL, Agent → MARK_AGENT, Overflow → MARK_OVERFLOW, Nav → FOCUS_BORDER }
pub fn glyph_fg(style: MarkStyle) -> u32 { Overflow → TEXT_PRIMARY, _ → CODE_BG }
pub fn range_mark_bg(style: MarkStyle) -> u32 { Nav → MARK_SELECTION_BG, Structural → 0xd9822b30, Agent → 0x8e7cc330, Overflow → MARK_SELECTION_BG }
```

### 5.3 Nav rings — reuse passes 2c / 3 unchanged

`paint_ring` (L4509–4527) stays as is, reading `item.focused` / `item.neighbor`, with two edits: (a) `bc = bc.opacity(item.light)` (04-mask.md; identity until a mask exists); (b) a third branch: `if item.selection && !item.focused { (1.0, rgba(theme::FOCUS_BORDER << 8 | 0x99)) }` painted in pass 2c alongside neighbors. Pass 2c loses its `if !cg_scrim` guard (05-edges deletes the scrim). `ring_paints_after_leaf_overlay(item.focused, item.neighbor)` unchanged.

In `paint_items` (L1693–1695):
```rust
focused:  ov.is_focus_ring(&item.node.id),
deferred_overlay: defer_leaf_to_overlay(is_focused, is_leaf),   // is_focused = (id == focus_id) stays: §2.10 geometry, not a mark
neighbor: ov.is_neighbor(&item.node.id),
selection: ov.is_selection(&item.node.id),
```
and delete `let (_, neighbor_ids) = self.neighbors.clone().unwrap();` (L1456). The `stale`/refresh block (L1448–1455) stays and its output goes into `SessionState.neighbors` (00 §3.2).

### 5.4 Range marks — generalising `cg_highlight_lines`

```rust
fn leaf_text_body(node, left, top, full_h, label_w, vh, buffers, file_symbols, focused: bool,
                  marks: &[(Range<usize>, MarkStyle)]) -> (Vec<BodyText>, usize)
```
Inside, after `let Some(m) = buffers.get(&rel, syms)`: convert once —
```rust
let line_marks: Vec<(Range<usize>, MarkStyle)> = marks.iter().map(|(r, s)| {
    let a = m.buffer.byte_to_line(r.start);
    let b = m.buffer.byte_to_line(r.end.saturating_sub(1)) + 1;     // identical to treemap.rs:1475–1476
    (a..b, *s)
}).collect();
```
and per row `mark: line_marks.iter().find(|(lr, _)| lr.contains(&file_line)).map(|(_, s)| *s)` (first match wins; the resolver ordered structural before agent within `ranges[id]` — sort there by `style` then `layer_index`). Caller (treemap.rs:1596–1611): `let hl = ov.range_marks(&item.node.id);` for **every** Text-tier leaf, not only the focused one — structural/agent range marks must show wherever code is readable. Delete the `cg_highlight_lines` block (1458–1478).

Paint (L4410–4425): `if let Some(style) = bt.mark { … rgba(theme::range_mark_bg(style)) … }` — same quad geometry (`char_w * len + 12`, `-4.0` x inset, 2 px radius). Result for the call-graph preset is pixel-identical to today: same range, same colour, same rows.

### 5.5 Glyph hover → note

`TreemapView.hover_glyph: Option<(SymbolId, usize /*slot*/)>`. In `on_mouse_move` (L2043–2059) after `hit_test`: run `mark_pass::hit(&self.last_glyph_rects, mx, my)` where `last_glyph_rects: Vec<(SymbolId, usize, PxRect)>` is written by `paint_items` each frame from `glyph_rects(item)` (a few dozen entries; avoids re-running LOD in the mouse handler). On change set `self.view_dirty |= Deps::HOVER; cx.notify()`. Until 07-notes.md lands, the app shows the note text through the existing doc-panel path: in `paint_items`, `if let Some((id, slot)) = &self.hover_glyph { panel_doc = Some((glyph.note.clone(), x, y, w, h)) }` (same tuple as L1674) — the tooltip reads e.g. `"complexity×churn · p90 both"`. With 07: `SessionState.hover_glyph` and a `Notes { at: $hoverGlyph, source: metric }` entry in the default view render the same text as a `metric` Note; the doc-panel shim is then deleted.

### 5.6 `PaintOverrides` construction site

`paint_items` already builds `ov` (00 §3.2). No new fields on `TreemapView` beyond `hover_glyph`, `last_glyph_rects`.

---

## 6. Commands

- `ViewCommand::PushLayer(LayerSpec::Marks(..))`, `PopLayer`, `RemoveLayer(i)`; marks layers accumulate.
- CLI (parent §6.2): `outrider mark <set|symbol[:lines]> --kind hotspot|cycle|layering-violation|custom=<name>|agent-flag [--label …] [--basis …]` → `ViewPatch { layers: [Marks(..)] }`. `symbol:40-52` builds `on: [{symbol, lines:[40,52]}]`; a bare set name builds `on: "name"`. `--kind` structural without `--basis` → CLI exits non-zero with the validate message.
- Nav kinds are pushed by the app only: `default_view` (focusRing, neighbor), the call-graph preset (selection), and 08-panel.md (`selection` on `$selection` for any open panel row).

---

## 7. Invalidation

| bit | who sets it | effect |
|---|---|---|
| `FOCUS` | focus changes (00 §3.4) | `focusSet` / `neighbors` sets → `MarkTable` recomputed (nav rings move) |
| `SELECTION` | panel row move (08) | `$selection` / `$selectionSite` sets → selection ring / range mark |
| `HOVER` | `on_mouse_move` (`hover_id` **and** `hover_glyph`) | glyph note (07) |
| `SPEC` | `apply_view_command` | affected marks layers |
| `TREE` | loader / packing | everything, incl. `LineTables` dropped |
| `GIT` | git watcher | `LineTables::clear()` (line→byte tables of edited files go stale); anchor-based marks recomputed |
| `RELATIONS` (05) | relation worker results | only via sets that depend on it (e.g. a `reach`-based mark set) |

---

## 8. Migration steps

### 8.1 Milestone-0 subset (00-framework §5 step 1 — nav kinds only)

Goal: focus ring and neighbor rings produced by `marks` layers, pixel-identical to today. No glyphs, no ranges, no `mark_pass.rs`, no `lines.rs`.

1. `spec.rs`: `MarksSpec`, `MarkTarget`, `MarkAnchor`, `MarkKind`, `MarkStyle` exactly as §3 (full enum, so documents validate now). `validate`: structural-needs-basis + anchor shape rules.
2. `layers/marks.rs`: `Mark`, `Glyph`, `MarkTable` (all fields), `resolve_marks` implementing **only** step 1 of §4.2 for `on: Set(..)` and whole-symbol marks (`by_symbol`), plus set `ranges` → `ranges` (needed for `$selectionSite` later, trivial now). `on: Anchors(..)` → warning `"marks[i]: explicit anchors not yet supported"` and skip. `compute_glyphs` implemented and unit-tested (pure), but `glyphs` map is filled and simply unused by the app.
3. `PaintOverrides::is_focus_ring / is_neighbor / is_selection` (§4.4). `glyphs()`/`range_marks()` return empty.
4. `treemap.rs`: §5.3 edits (`focused:`/`neighbor:` from `ov`, delete `neighbor_ids`), add `selection` field defaulting from `ov.is_selection` (always false in this milestone), keep everything else. `default_view` has the two nav layers (00 §3.3).
5. Golden test (§9 "nav rings identical") green; manual: arrows still ring the four neighbors, focus ring intact, call-graph mode unaffected.

### 8.2 Full marks (milestone 3–4 alongside notes/edges)

6. `lines.rs` + `LineTables` on `ViewResolver`; anchors resolution (§4.2 step 2) with clipping and warnings; tests.
7. `paint_model.rs`: `BodyText.mark`, `PaintItem.glyphs / range_marks`; `theme.rs` `MARK_*` + helpers; `leaf_text_body` signature (§5.4) and the paint colour switch — at this step pass `&[]` from the caller except the focused leaf which passes today's `cg_highlight_lines` converted to `[(site, MarkStyle::Nav)]`. Behaviour unchanged.
8. `mark_pass.rs` (§5.2): `glyph_rects`, `reserved_right`, `paint`, `hit`; `pinned_name` reserved width; call from the closure after headers/edges; populate `PaintItem.glyphs`. `hover_glyph` + doc-panel shim (§5.5).
9. Route range marks through `ov.range_marks` for every Text leaf; when 05-edges §5.5 step 6 lands (`cgSite` set + `selection` layer), delete `cg_highlight_lines` (treemap.rs 1458–1478) and the `hl`-only-when-focused branch (1596–1600).
10. With 07-notes.md: replace the doc-panel shim with a `metric` Note over `$hoverGlyph`.

---

## 9. Tests

**`outrider-view` unit tests** (`crates/outrider-view/tests/marks.rs`; hand-built trees for slot/anchor tests, `mini_repo` for symbol lookups):

- `marks_spec_json_roundtrip` — the seven JSON examples in §3, including kebab aliases parse and emit camelCase.
- `structural_without_basis_rejected` — `{marks:{on:"x", kind:"hotspot"}}` → `Violation{path:"layers[0].marks.basis"}`; with `basis` → none; `custom` likewise; `agentFlag` without basis → none.
- `anchor_shape_rules` — both `range` and `lines` → violation; `lines:[0,3]` → violation; `lines:[5,4]` → violation.
- `slot_overflow` — `compute_glyphs`: `[hotspot, cycle]` → `["!", "↻"]`; `[hotspot, cycle, agentFlag]` → `["!", "+2"]` with overflow note joining cycle + agent notes; `[agentFlag, hotspot]` → `["!", "◆"]` (structural first regardless of layer order); `[agentFlag ×3]` → `["◆", "+2"]`; `[focusRing, hotspot]` → `["!"]`; `[]` → `[]`.
- `range_to_line_mapping` — `LineTable::from_bytes(b"a\nbb\nccc\n")`: `line_start(1)=0, (2)=2, (3)=5, (4)=None`; `byte_range_of_lines(2,3) == 2..9`; `byte_to_line(4) == 1`; CRLF input keeps `\r` inside the range.
- `lines_anchor_resolves_and_clips` — mini_repo `fn:src/lib.rs::free`, `lines` covering the whole function → `ranges[free]` one entry equal to the intersection with `node.byte_range`; lines entirely outside → warning and no entry.
- `set_ranges_become_range_marks` — a `ResolvedSet` with `ranges: {sym: [10..20]}` and `kind: selection` → `ranges[sym] == [(10..20, Selection mark)]`, `by_symbol[sym]` also has the mark, no glyph.
- `nav_kinds_no_glyphs` — focusRing/neighbor/selection produce empty `glyphs`.
- `note_text` — `(Some("l"), Some("b")) → "l · b"`, `(None, Some("b")) → "hotspot · b"`, `(None, None) → "hotspot"`.

**App-side tests** (`crates/outrider/src/view/`, pure functions):

- `paint_decisions` golden (00 §4): default view + `focus = X`, `neighbors = [A, None, C, D]` → `focused` true only for X; `neighbor` true for A, C, D; `selection` false everywhere; `light == 1.0`. This is the "focus/neighbor rings identical to today" acceptance: the ring booleans equal the pre-migration expressions `id == focus` and `!is_focused && neighbor_ids.contains(id)` for every id in a small tree.
- `glyph_rects_right_aligned` — item `x=100, w=200, y=50`, 2 glyphs → slot 0 at `x = 300 - 3 - 14`, slot 1 at `x = 300 - 3 - 14 - 2 - 14`; `reserved_right(2) == 36`; `reserved_right(0) == 0`.
- `leaf_text_body_marks_rows` — extend `leaf_text_body_paints_code_without_duplicate_signature` (treemap.rs:5148): pass one byte range covering source lines 2–3 → those two `BodyText`s have `mark == Some(Nav)`, others `None`; two overlapping ranges (Structural, Agent) → the row shows `Structural`.
- `mark_hit` — point inside slot 1 rect → `Some((item_idx, 1))`; outside → `None`.

**Manual acceptance:** (M0) no visible change; arrows / focus ring as before. (Full) `outrider mark 'src/**' --kind hotspot --label t --basis b` shows amber "!" badges top-right of every visible box at Label+; names truncate before the badges; hovering a badge shows "t · b"; three marks on one symbol show `[!, +2]`; `outrider mark src/x.rs::f:10-12 --kind agent-flag --label note` highlights those rows in violet when zoomed to code; Tab call-graph preset still highlights the selected callee's call site in blue exactly as before.

---

## 10. Open questions / risks

1. **Glyph font coverage** — `↻` (U+21BB) and `◆` (U+25C6) render via GPUI font fallback if `FONT_FAMILY_SANS` lacks them; if fallback looks wrong on some platform, swap `glyph_text` to ASCII (`"C"`, `"*"`) — a one-line table change.
2. **`range_marks` return type** — 00 §3.2 wrote `&[(Range<usize>, MarkStyle)]`; the resolver stores `(Range, Mark)` so a borrowed slice would need a parallel vector. Returning `Vec` is simpler and only Text-tier leaves ask (tens per frame). Flagged in §11.
3. **Range marks and the minimap/texture path** — at `LeafDraw::Minimap` the code is a baked texture; range marks are not painted there (spec says Full/Text fidelity). A future stripe in the texture margin is possible; not now.
4. **Glyphs on the widened focus leaf** — the leaf's `paint_w` is `expanded_w` (§2.10), so slots follow the expanded right edge; confirm this reads well against the doc panel that floats above it (L1721).
5. **`GIT` invalidating `LineTables`** is coarse (drops every table). Fine at 1 s poll granularity; refine to per-file mtime if it shows up.

---

## 11. Framework deltas

1. **`PaintOverrides::range_marks`** returns `Vec<(Range<usize>, MarkStyle)>` rather than `&[..]` (00 §3.2); adds **`is_selection(&self, id) -> bool`**.
2. **`PaintItem.selection: bool`** (app-side, alongside `focused`/`neighbor`); **`BodyText.highlighted: bool` → `mark: Option<MarkStyle>`**.
3. **New module `crates/outrider-view/src/lines.rs`** (`LineTable`, `LineTables`) and a **`lines: LineTables` field on `ViewResolver`**, shared with 03-fill.md's importer (00's module list did not include it).
4. **`Glyph`, `Mark`, `MarkStyle`** live in `outrider-view` (`spec.rs` for `MarkStyle`, `layers/marks.rs` for `Mark`/`Glyph`); the app re-exports them from `view/mod.rs`. `MarkTable` gains a `glyphs` map and `deps`.
5. **`SessionState.hover_glyph: Option<(&SymbolId, usize)>`** and the `$hoverGlyph` pseudo-id are needed only when 07-notes.md replaces the doc-panel shim (§5.5); until then no framework change.
6. `Deps::HOVER` is now also set by glyph hover, not only `hover_id` (00 §3.4 wording).
