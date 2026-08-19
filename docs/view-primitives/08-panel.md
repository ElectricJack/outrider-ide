# 08 — Panel: keyboard-navigable row lists (palette, call-graph columns, metric tables)

**Parent:** [../view-primitives.md](../view-primitives.md) §2.8 (definition), §3.2 (text channel), §3.4 (`selection` live set), §4 (call-graph and palette decompositions), §6.2 (`panel` verb), §8.8 (component spec), §9 milestones 4–5, §10 open question 5 ("topmost panel wins").
**Framework:** [00-framework.md](00-framework.md) — `LayerSpec::Panel`, `SetRef`, `Deps::SELECTION`, `SessionState.selection`, `ResolveCtx`, `ResolvedView::panels`, `ViewCommand::{PushLayer, RemoveLayer, DefineSet, Camera(..)}`, `apply_view_command`, dirty wiring.
**Related:** 02-set.md (`fuzzy`, `kind`, `not`, `$focus`), 03-fill.md (`MetricRef`, `MetricRegistry`, `MetricValue`), 05-edges.md (`RelationRegistry`, `calls` provider — the EdgeGroups source), 06-marks.md (range mark on the selected call site), 07-notes.md (`MetricReadoutRegistry` for the preview pane), 09-camera.md (`CameraCommand::{Focus, Frame}`).


> **Reconciliation note (post-review).** Where this file says `RelationProvider::edges_detailed(id, dir, ctx) -> Option<Vec<EdgeDetail>>`, read it as the 05-edges.md contract: `lookup(id, dir, ctx) -> Lookup::{Ready(Vec<(SymbolId, f64)>), Pending}` for the row list plus `edge_detail(from, to, ctx) -> Option<EdgeDetail{ raw_name, site, weight }>` for `raw_name`/`call_site`. The async-completion signal is `Deps::RELATIONS` (adopted in 00-framework §6). `fuzzy_match` lives in `outrider-index/src/search.rs` (02-set.md), not in `outrider-view`.

---

## 1. Purpose and scope

A Panel is a screen-space rectangle listing rows that are linked to symbols. It is the *only* primitive that owns UI focus: while a panel is open Up/Down move its row, Enter frames/focuses the row's symbol, Esc closes it, and the current row is published as the live set `selection` (`SessionState.selection`, `Deps::SELECTION`). Two existing features are Panels in disguise and are migrated here:

- **Search palette** (Ctrl+P files, Ctrl+T symbols; `palette.rs`, `render_palette`) → one floating Panel over the reserved set `__palette` = `fuzzy(query) ∩ kind(...)`, with the query held in app-side panel state and re-`DefineSet` on every keystroke; the preview pane becomes doc/metric notes for the selected row.
- **Call-graph columns** (`render_call_graph`, `CgScrollState`, `on_call_graph_key`, `CgEdgeGroup`) → two docked EdgeGroups Panels (callers left, callees right) over `of: $focus`, `relation: calls`. Row grouping by `raw_name`, one active edge per group, and the Up/Down/Left/Right semantics are preserved exactly. The scrim, the call edges and the selected-call-site highlight are **not** this spec's job (04-mask, 05-edges, 06-marks) — this spec deletes only the column code.

`Matrix` rows are declared and validated so documents parse, but resolution returns an empty panel with a warning until the `matrix` space exists (parent §8.1).

---

## 2. Ground truth: existing code touched

| file | symbol | approx line | what it does today | what changes |
|---|---|---|---|---|
| `crates/outrider/src/palette.rs` | `Palette { open, mode, query, results, selection, candidates }`, `open/close/is_open/type_char/backspace/move_selection/confirm/name_of/refilter/collect_candidates`, `MAX_RESULTS = 12` | 5–116 | palette state; results = candidates sorted by `(name.len, name)`, filtered by `fuzzy_match`, first 12 | **deleted** (state moves to `PanelInstance` + the `__palette` set); `PaletteMode` deleted; `MAX_RESULTS` moves to `PanelSpec.limit` default |
| " | `fuzzy_match(query, name)` | 118–130 | case-insensitive subsequence match | **kept**; moved to `crates/outrider-index/src/search.rs` per 02-set.md (02-set's `SetExpr::Fuzzy` uses it) with a `pub use outrider_index::search::fuzzy_match;` left in `palette.rs` until the file is deleted |
| " | tests | 132–360 | palette behaviour | `fuzzy_match_*` tests move with the fn; palette tests are replaced by §9's `palette_golden` |
| `crates/outrider/src/treemap.rs` | `TreemapView.palette` | 537 | field | replaced by `panels: crate::view::panel_view::PanelState` |
| " | `render_palette` | 1821–1960 | list div + preview div, centered at top 60 | **deleted**; replaced by `panel_view::render_panels` |
| " | `on_key_down` palette branch, `on_palette_key` | 2209–2266 | Esc/Enter/Up/Down/Backspace/char | replaced by `on_panel_key` (§5.4) |
| " | `OpenFilePalette` / `OpenSymbolPalette` actions | 4260–4277 | `palette.open(mode, &tree)` | push the palette panel layer (§5.5) |
| " | `has_overlays` (`palette.is_open()`), `palette_overlay` | 4132, 4120 | toolbar hide + overlay build | `panels.has_float()` / `render_panels` |
| " | `start_loading` (`self.palette.close()`), `ToggleSettings`/`ToggleProjectSettings` (`palette.close()`) | 2796, 4239, 4253 | close palette on mode changes | `self.close_all_panels()` |
| " | `CallGraphMode { center, caller_groups, callee_groups, selection, loading, scroll }`, `CgEdgeGroup`, `group_edges`, `CG_*` consts, `CgScrollState`, `cg_scroll_target`, `cg_card_top`, `cg_card_height`, `CallGraphSelection`, `CgColumnItem`, `cg_parent_name` | 635–751 | call-graph column model | `CallGraphMode/CgScrollState/CgEdgeGroup/CallGraphSelection/CgColumnItem` and the `cg_*` layout fns **deleted**; `group_edges` moves to `outrider-view/src/layers/panel.rs` (same algorithm, generic over `(SymbolId, raw_name, call_site)`); `cg_parent_name` moves to `panel_view.rs` |
| " | `CallGraphResolver`, `InflightResolve`, `call_graph_cache`, `cg_resolver`, `poll_call_graph`, `maybe_precompute_call_graph` | 753–816, 558–559, 2472–2509 | async `resolve_calls` + cache | **owned by 05-edges** (`calls` provider). This spec assumes 05-edges' provider exposes `pending(&SymbolId) -> bool`; until then the resolver stays and `panel_view` polls it |
| " | `enter_call_graph`, `exit_call_graph` | 2430–2470, 2511–2525 | build `CallGraphMode` on Tab; refocus + frame on exit | rewritten as `push_call_graph_panels` / handled by generic Esc (§5.5) |
| " | `on_call_graph_key` | 2617–2784 | Tab/Esc exit; Left/Right cycle-or-switch; Up/Down move; Enter re-center | semantics moved into `on_panel_key` (§5.4 table) |
| " | `on_key_down` `if self.call_graph.is_some()` | 2146–2149 | route to `on_call_graph_key` | route to `on_panel_key` when `panels.is_open()` |
| " | Tab in `on_nav_key` | 2308–2312 | `enter_call_graph` | `push_call_graph_panels` |
| " | `call_graph_column_lefts` | 924–927 | column x for left/right of the focus box | **kept**, moved to `panel_view.rs` |
| " | `cg_source_lines` | 3191–3220 | up to N highlighted source lines of a symbol | **kept**, renamed `panel_view::source_lines` (same body) |
| " | `render_call_graph`, `render_cg_column` | 3222–3554 | build the two columns | **deleted**; `render_panels` reproduces the card look (§5.3) |
| " | `paint_items` L1805, `Render` L4083/L4104–4107/L4155/L4612 | | `cg_scrim`, `cg_animating`, `call_graph_overlay` | `cg_scrim` → 04-mask; `cg_animating` → `panels.is_animating()`; `call_graph_overlay` → `panels_overlay` |
| " | `on_mouse_move/on_scroll/on_right_press/on_left_release/mouse_down` guards `\|\| self.call_graph.is_some()` | 1968, 1991, 2030, 2063, 4288 | freeze the map in call-graph mode | `\|\| self.panels.captures_mouse()` (true for docked EdgeGroups panels, false for float — the palette never froze the map) |
| `crates/outrider/src/overlays.rs` | `centered_panel`, `backdrop`, `context_menu_row_dynamic` | 212, 203, 168 | GPUI div builders | reused; add `panel_row(id, selected)` builder (§5.3) |
| `crates/outrider/src/buffers.rs` | `BufferManager::get`, `symbol_start_line`, `buffer.line` | | | used by `source_lines` |
| `crates/outrider-index/src/call_graph.rs` | `CallEdge { target, raw_name, call_site }` | 9–13 | | shape mirrored by `RowAlternate` |

---

## 3. Spec types

In `crates/outrider-view/src/spec.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PanelSpec {
    pub rows: PanelRows,
    #[serde(default)] pub columns: Vec<MetricRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub sort_by: Option<SortKey>,
    #[serde(default)] pub dock: Dock,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub title: Option<String>,
    /// Stable identity for the app/CLI to find and remove this panel; reserved: "__palette", "__callers", "__callees".
    #[serde(default, skip_serializing_if = "Option::is_none")] pub id: Option<String>,
    /// Max rows after sorting (palette uses 12). None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub limit: Option<usize>,
}
impl Default for PanelSpec { /* rows: Set(Inline(Ids([]))), dock: Float, others empty */ }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum PanelRows {
    Set(SetRef),                                          // {"set": "affected"} | {"set": {"fuzzy": "auth"}}
    EdgeGroups { of: SetRef, relation: String, direction: EdgeDirection },
    Matrix { space: String },                             // validated ("matrix" only), unsupported at resolve
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EdgeDirection { In, Out }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Dock { Left, Right, Bottom, #[default] Float }

/// "name" | "nameLength" | any MetricRef string. Untagged so it stays a plain string on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SortKey { Builtin(BuiltinSort), Metric(MetricRef) }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuiltinSort { Name, NameLength /* (name.chars().count(), name) — today's palette order */ }
```
Serde note: `SortKey` must try `Builtin` first (a `MetricRef` newtype accepts any string). `MetricRef` is 03-fill's `pub struct MetricRef(pub String)`.

JSON:
```jsonc
// Metric table docked right (parent §5 example)
{ "panel": { "rows": { "set": "affected" }, "columns": ["churn", "coverage"], "sortBy": "churn", "dock": "right", "title": "Affected" } }

// The palette (pushed by Ctrl+P; the app keeps redefining set "__palette")
{ "panel": { "id": "__palette", "rows": { "set": "__palette" }, "sortBy": "nameLength", "limit": 12, "dock": "float", "title": "File" } }

// Call-graph columns (pushed by Tab)
{ "panel": { "id": "__callers", "rows": { "edgeGroups": { "of": { "ids": ["$focus"] }, "relation": "calls", "direction": "in"  } }, "dock": "left",  "title": "Callers" } }
{ "panel": { "id": "__callees", "rows": { "edgeGroups": { "of": { "ids": ["$focus"] }, "relation": "calls", "direction": "out" } }, "dock": "right", "title": "Callees" } }
```

Validation (`validate.rs`): unknown `SetRef::Name` in `rows`/`of` (framework rule); `EdgeGroups.relation` unknown → hard; `columns[i]`/`sortBy` metric unknown → hard; `Matrix{space}` ≠ `"matrix"` → hard; `Matrix` at all → soft warning `"panel.matrix is not supported yet"`; duplicate `id` among layers → hard (`"panel-id-duplicate"`); `limit == Some(0)` → hard.

---

## 4. Resolution (`crates/outrider-view/src/layers/panel.rs`)

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPanel {
    pub layer: usize,                       // index in spec.layers (for RemoveLayer)
    pub id: Option<String>,
    pub title: Option<String>,
    pub dock: Dock,
    pub columns: Vec<MetricRef>,            // header labels
    pub rows: Vec<Row>,
    pub pending: bool,                      // relation data not ready yet ("Resolving..."), see §4.2
    pub deps: Deps,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: SymbolId,                       // for EdgeGroups: target of alternates[0]
    pub label: String,                      // node.name (or raw_name when the target is not in the tree)
    pub sublabel: String,                   // node.id.qualified_path
    pub cells: Vec<Option<MetricValue>>,    // one per `columns`
    pub group: Option<String>,              // EdgeGroups: raw_name
    pub alternates: Vec<RowAlternate>,      // EdgeGroups: every edge in the group, in provider order; else empty
}
#[derive(Debug, Clone, PartialEq)]
pub struct RowAlternate { pub id: SymbolId, pub call_site: Option<Range<usize>> }
```
The *active* alternate per group is interaction state, not resolution state, so it lives in the app (`PanelInstance.group_active`, §5.1); the resolver emits every alternate. `Row.id` is the effective id only when `group_active == 0`; the app computes `effective_id(row, active)`.

### 4.1 Algorithm

```rust
pub fn resolve_panel(layer_idx: usize, spec: &PanelSpec, ctx: &ResolveCtx,
                     sets: &BTreeMap<String, ResolvedSet>, warnings: &mut Vec<String>) -> ResolvedPanel
```
1. **Rows.**
   - `Set(r)`: `ids = resolve_set_ref(r)` (02-set; inline expressions resolved through the same memo). One `Row` per id that exists in `ctx.index` (missing → counted into a warning); `alternates = []`, `group = None`.
   - `EdgeGroups { of, relation, direction }`: for each id in `of` (normally one: `$focus`), `edges = relations.get(relation).{in_edges|out_edges}(id)` **with call-site info** — 05-edges' `RelationProvider` returns `Vec<(SymbolId, f64)>` per 00-framework; EdgeGroups additionally needs `raw_name` and `call_site`, so this spec requires `RelationProvider::edges_detailed(&self, from: &SymbolId, dir: EdgeDirection, ctx) -> Option<Vec<EdgeDetail{ target, raw_name, call_site, weight }>>` (`None` = pending; §11). Then `group_edges(edges)`: identical to treemap.rs L649–664 — first-seen order of `raw_name`, edges appended in provider order:
     ```rust
     pub fn group_edges(edges: Vec<EdgeDetail>) -> Vec<(String, Vec<EdgeDetail>)> {
         let mut groups: Vec<(String, Vec<EdgeDetail>)> = Vec::new();
         let mut seen: HashMap<String, usize> = HashMap::new();
         for e in edges { match seen.get(&e.raw_name) {
             Some(&i) => groups[i].1.push(e),
             None => { seen.insert(e.raw_name.clone(), groups.len()); groups.push((e.raw_name.clone(), vec![e])); } } }
         groups
     }
     ```
     One `Row` per group: `id = group[0].target`, `label = index.node(id).map(name).unwrap_or(raw_name)`, `group = Some(raw_name)`, `alternates = group.iter().map(|e| RowAlternate{ id: e.target, call_site: e.call_site })`. If `of` has several ids the groups of each are concatenated in `of` iteration order (sorted `SymbolId` order of the set).
   - `Matrix` → empty rows + warning, `pending = false`.
2. **Cells.** For each row and each `MetricRef` in `columns`: `ctx.metrics.evaluate(&m, node) -> Option<MetricValue>` (03-fill; percentile over all laid-out nodes, cached per metric). Missing node → `None`.
3. **Sort.** `Set` rows only (EdgeGroups keep provider/group order — today's columns are unsorted):
   - `None` → set iteration order (sorted `SymbolId`, i.e. `(kind, qualified_path, ordinal)`).
   - `Name` → `label` then id; `NameLength` → `(label.chars().count(), label)` then id — reproduces `Palette::open`'s sort.
   - `Metric(m)` → descending `cells[col_of(m)].raw`, `None` last, then id; if `m ∉ columns` evaluate it once for sorting without emitting a cell.
   Sort is stable.
4. **Limit.** `rows.truncate(limit)` after sort (palette: 12 after fuzzy filter — same as `refilter().take(12)` because the candidate order equals the sort order).
5. **Deps.** `set/of deps ∪ relation deps ∪ TREE ∪ (METRICS|GIT if columns non-empty)`. **Not** `SELECTION`: the highlight is app-side; re-resolving on every Up/Down would be wasteful.

`ResolvedView.panels` is `Vec<ResolvedPanel>` in layer order; the resolver recomputes a panel when its deps intersect `dirty` or `SPEC` changed the layer.

### 4.2 Pending relations

`enter_call_graph` today shows "Resolving..." while `CallGraphResolver` works. With 05-edges' `calls` provider (async behind a cache), `edges_detailed` returns `None` when the cache has no entry and a resolve is in flight; the panel resolves to `rows: [], pending: true` and the app requests a re-resolve when the provider reports completion (05-edges sets a `Deps::METRICS`-like bit — it should reuse `Deps::TREE`? No: it defines the app-side poll that ORs `Deps::SPEC` into `view_dirty` when a result lands, which re-resolves the panel; see §11). Until 05-edges lands, `panel_view` keeps polling `cg_resolver` and stores results into `call_graph_cache`, and `RelationRegistry::builtin()` reads that cache.

---

## 5. App integration

### 5.1 New file `crates/outrider/src/view/panel_view.rs`

```rust
pub(crate) struct PanelState {
    /// Open panels in push order; `open.last()` with `dock == Float` or the `active` index gets the keys.
    pub open: Vec<PanelInstance>,
    /// Which open panel receives Up/Down/Enter (index into `open`). Topmost = last pushed unless Left/Right moved it.
    pub active: usize,
    /// (from, target, started) vertical scroll tween for docked columns — replaces CgScrollState per panel.
    /// (kept inside PanelInstance.scroll)
}
pub(crate) struct PanelInstance {
    pub id: String,                       // PanelSpec.id or "panel@<layer idx>"
    pub layer: usize,                     // index into view_spec.layers (kept in sync on RemoveLayer)
    pub selection: usize,                 // row index
    pub group_active: Vec<usize>,         // per row, active alternate (EdgeGroups); resized on rows change
    pub scroll: ScrollTween,              // { from: f32, target: f32, started: Instant } — CG_SCROLL_SECS ease_in_out_cubic
    pub query: Option<String>,            // Some for the palette (typing edits it)
    pub set_name: Option<String>,         // set redefined on typing ("__palette")
    pub kind_filter: Option<SetExpr>,     // palette: {"kind":"file"} or {"not":{"kind":"folder"}}
    pub preview: bool,                    // palette: show the notes preview pane
    pub captures_mouse: bool,             // docked EdgeGroups panels freeze the map like call-graph mode did
    last_row_ids: Vec<SymbolId>,          // to detect rows change → reset selection/group_active
}
pub(crate) struct ScrollTween { from: f32, target: f32, started: std::time::Instant }
impl ScrollTween { fn current(&self) -> f32; fn is_animating(&self) -> bool; fn retarget(&mut self, t: f32); }

pub(crate) const PANEL_COL_W: f32 = 320.0;   // was col_w
pub(crate) const PANEL_CARD_H: f32 = 120.0;  // CG_CARD_H
pub(crate) const PANEL_SELECTED_H: f32 = 300.0; // CG_SELECTED_H
pub(crate) const PANEL_CARD_GAP: f32 = 6.0;  // CG_CARD_GAP
pub(crate) const PANEL_SCROLL_SECS: f64 = 0.20;
pub(crate) const PALETTE_W: f32 = 500.0; pub(crate) const PREVIEW_W: f32 = 300.0;

impl PanelState {
    pub fn is_open(&self) -> bool; pub fn has_float(&self) -> bool; pub fn captures_mouse(&self) -> bool;
    pub fn is_animating(&self) -> bool;
    /// Called every frame after resolve: sync instances with `resolved.panels` (create/drop by layer idx,
    /// reset selection+group_active when row ids changed, apply the "active = last-pushed non-empty" rule).
    pub fn sync(&mut self, panels: &[ResolvedPanel]) -> bool /* selection changed → caller sets SELECTION */;
    pub fn selected(&self, panels: &[ResolvedPanel]) -> Option<SymbolId>;          // effective id of active panel's row
    pub fn selected_call_site(&self, panels: &[ResolvedPanel]) -> Option<(SymbolId, Range<usize>)>; // for 06-marks
    pub fn effective_id(row: &Row, active: usize) -> &SymbolId { row.alternates.get(active).map(|a| &a.id).unwrap_or(&row.id) }
}
```
`TreemapView` gains `panels: PanelState` (replacing `palette` and `call_graph`), and `fn panel_selection(&self) -> Option<&SymbolId>` (00-framework §3.2 already calls it for `SessionState.selection`) returns `self.panels.selected(..)`. Framework's `SessionState.selection: Option<&SymbolId>` — cache the effective id in `PanelState.selected_cache: Option<SymbolId>` in `sync` so the borrow is trivial.

### 5.2 Layout

```rust
pub(crate) fn call_graph_column_lefts(focus_left: f32, focus_right: f32, column_width: f32) -> (f32, f32)  // moved verbatim
pub(crate) fn panel_rect(dock: Dock, vw: f64, vh: f64, focus_screen: Option<(f32, f32)>) -> (f32 /*left*/, f32 /*top*/, f32 /*w*/, f32 /*h*/)
```
- `Left`/`Right`: `w = PANEL_COL_W`, `top = 48`, `h = (vh - 96).max(200)`; `left` from `call_graph_column_lefts(focus_left, focus_right, w)` where `focus_left/right` are computed exactly as `render_call_graph` L3230–3246 (packed rect → `world_to_screen`, `focused_width(max_line_chars)` for leaf items); fallback `(12, vw - w - 12)`.
- `Bottom`: `left = 12`, `w = vw - 24`, `h = 200`, `top = vh - h - 12`.
- `Float`: `top = 60`, `w = PALETTE_W (+ GAP + PREVIEW_W when preview)`, `left = ((vw - total_w)/2).max(0)` — `render_palette` L1835–1840.

Docked panels' vertical layout is `cg_card_top/cg_card_height` verbatim (renamed `card_top/card_height`, selected card 300 px, others 120, gap 6, list centred on the first card as in `render_cg_column` L3452–3455) with `scroll.current()` subtracted. Float panels are a simple flex column of 13 px rows (`render_palette` L1863–1879), no scroll (limit 12).

### 5.3 Rendering — `render_panels(&mut self, vw, vh, cx) -> Option<gpui::Div>`

Returns one absolute `div().top_0().left_0().size_full()` containing every open panel (docked and float), replacing both `palette_overlay` and `call_graph_overlay` in `Render` (`.children(panels_overlay)` at the position of L4608, drop L4612).

Per panel:
- **Frame.** Docked: header row `"{title} ({rows.len()})"` 11 px sans `TEXT_SECONDARY` (L3403–3412), then cards. Float: `overlays::centered_panel`-style shell (`CODE_BG`, `FOCUS_BORDER`, rounded 4, `overflow_hidden`) — factor a `pub(crate) fn panel_shell(left, top, w) -> Div` into `overlays.rs` (= `centered_panel` without the 24/20 padding) and use it for both.
- **Query line** (float with `query`): `"[{title}] {query}│"` 14 px (L1854–1862).
- **Row.** New `overlays::panel_row(id: ElementId, selected: bool) -> Stateful<Div>` = `context_menu_row_dynamic` styling but `px 8 / py 4 / 13px / FONT_FAMILY`, `bg 0x2a2d32` when selected (L1867–1878); child text `"{label}  {sublabel}"` plus, when `columns` non-empty, one right-aligned 11 px `TEXT_SECONDARY` cell per column formatted `format_cell(&MetricValue)` (`"{raw:.0}"` for counts, `"{raw:.1}"` otherwise, `"—"` for None). Docked cards: name row (11 px sans) + `"{active+1}/{total}"` when `alternates.len() > 1` + parent (`cg_parent_name(qualified_path)`, 9 px mono) + file (8 px mono) + up to 15 `source_lines` (10 px mono via `code_line`) for rows within ±3 of the selection — L3471–3548 verbatim modulo names.
- **Empty / pending.** `"Resolving..."` when `pending`, `"No {title.lowercase()}"` when empty (L3424–3450).
- **Preview pane** (float, `preview == true`): a `PREVIEW_W` shell to the right showing, for the selected effective id, the notes `panel_view::preview_notes(node, readouts) -> Vec<ResolvedNote>` = `[Metric(kind label upper), Metric(signature)?, Doc(doc)?, Metric("{measure} lines · {churn_count} commits (p{churn*100})")]` — the "palettePreview" readout registered in 07-notes' registry — rendered 12/12/12/11 px sans, colours `TEXT_SECONDARY/TEXT_PRIMARY/DOC_COLOR/TEXT_SECONDARY` (L1892–1946). Pane omitted when the node has none of signature/doc/churn (L1832–1833). This is how "the preview pane as metric Notes" is satisfied without pushing a Notes layer that would also paint on the map.
- **Mouse.** Each row `.on_click(cx.listener(move |this, _, w, cx| { this.panel_click(panel_idx, row_idx, w, cx) }))` = set `active = panel_idx`, `selection = row_idx`, then the Enter action (§5.4). Float rows are clickable today only implicitly (they aren't) — this is additive.

### 5.4 Keyboard — `on_panel_key(&mut self, e, window, cx)`

Called from `on_key_down` where L2146–2149 (`call_graph`) and L2209–2212 (`palette`) were; **before** `settings_draft`/`project_setup` checks stays as today for call-graph (L2146 precedes them) — keep that order: `if self.panels.is_open() { self.on_panel_key(..); return; }` at L2146. Rule: keys go to `open[active]`; when several panels are open the last pushed is active initially ("topmost wins", parent §10.5).

| key | active panel kind | action (today's line) |
|---|---|---|
| Esc | any | `RemoveLayer(layer)` for **every** open panel pushed by the same gesture (palette: 1 layer; call-graph: 2 layers) → `apply_view_command`; for call-graph panels also the exit re-focus/frame of L2511–2525 (`focus.set(center)`, `nav_history.push`, `frame_focus`, `start_tween`) where `center` = the `$focus` at push time (kept in `PanelInstance.center: Option<SymbolId>`); Tab does the same for call-graph panels (L2628) |
| Up / Down | any | `selection ± 1`; docked: clamp (L2684–2727), retarget `scroll` to `card_top(selection)`; float: wrap (`Palette::move_selection` rem_euclid, L68–74). Then `view_dirty |= SELECTION`, `cx.notify()` |
| Enter | Set rows | `apply_view_command(Camera(Focus(id)))` then frame — i.e. `focus.set(id)`, `nav_history.push`, `maybe_precompute_call_graph`, `frame_focus`+`start_tween` (L2222–2237); palette additionally closes (`RemoveLayer`) |
| Enter | EdgeGroups | `focus.set(effective_id)`, `nav_history.push`, `frame_focus`+`start_tween` (L2739–2749). The panels are live on `$focus`, so `FOCUS` re-resolves both columns around the new centre; `sync` detects the row change → `selection = 0`, `active` = callees panel if non-empty else callers (L2754–2758). `PanelInstance.center = new id` |
| Left | EdgeGroups, dock Right (callees) | if callers panel non-empty: `active = callers`, its `selection = 0`, retarget its scroll to 0 (L2634–2647); else no-op |
| Left | EdgeGroups, dock Left (callers) | cycle `group_active[selection]` backwards mod `alternates.len()` when > 1 (L2648–2654) |
| Right | EdgeGroups, dock Left (callers) | if callees non-empty: `active = callees`, `selection = 0`, scroll 0 (L2660–2673) |
| Right | EdgeGroups, dock Right (callees) | cycle `group_active[selection]` forwards (L2674–2681) |
| Left/Right | Set rows | no-op |
| Backspace | `query.is_some()` | pop char, `redefine_palette_set()` (L2247–2250) |
| printable char | `query.is_some()` | push char, `redefine_palette_set()` (L2251–2264) |
| other | any | swallowed (call-graph) / swallowed (palette) — as today |

Generic rule for Left/Right on docked EdgeGroups panels: the arrow pointing *toward the other docked panel* switches `active`; the arrow pointing *away* cycles the group. This is exactly the four cases above and generalises to any pair of Left/Right-docked panels.

`redefine_palette_set()`:
```rust
let expr = SetExpr::Intersect(vec![SetExpr::Fuzzy(query.clone()), kind_filter.clone()]);
self.apply_view_command(ViewCommand::DefineSet { name: "__palette".into(), expr });   // sets SPEC → panel re-resolves
```
`sync` keeps `selection` in range (`Palette::refilter` L99–101 clamps to `len-1`) — clamp, don't reset, when the query changes; reset only when the panel's *layer* changes.

Every selection change: `view_dirty |= Deps::SELECTION` and `SessionState.selection` (via `panel_selection()`) reports the new effective id next frame; 07-notes' `$selection` anchors and 02-set's `selection` live set pick it up.

### 5.5 Presets (replacing the palette open and Tab)

```rust
fn open_palette(&mut self, files_only: bool) {
    let kind_filter = if files_only { SetExpr::Kind("file".into()) } else { SetExpr::Not(Box::new(SetExpr::Kind("folder".into()))) };
    self.apply_view_command(ViewCommand::DefineSet { name: "__palette".into(), expr: SetExpr::Intersect(vec![SetExpr::Fuzzy(String::new()), kind_filter.clone()]) });
    self.apply_view_command(ViewCommand::PushLayer(LayerSpec::Panel(PanelSpec {
        id: Some("__palette".into()), rows: PanelRows::Set(SetRef::Name("__palette".into())),
        sort_by: Some(SortKey::Builtin(BuiltinSort::NameLength)), limit: Some(12), dock: Dock::Float,
        title: Some(if files_only { "File" } else { "Symbol" }.into()), columns: vec![] })));
    self.panels.configure_last(|p| { p.query = Some(String::new()); p.set_name = Some("__palette".into()); p.kind_filter = Some(kind_filter); p.preview = true; p.captures_mouse = false; });
    self.settings_draft = None; self.context_menu = None;               // L4265–4266
}
fn push_call_graph_panels(&mut self) {   // Tab; same is_fn gate as enter_call_graph L2432–2439
    let of = SetRef::Inline(Box::new(SetExpr::Ids(vec![WireSymbolId::live("$focus")])));
    for (id, dir, dock, title) in [("__callers", EdgeDirection::In, Dock::Left, "Callers"), ("__callees", EdgeDirection::Out, Dock::Right, "Callees")] {
        self.apply_view_command(ViewCommand::PushLayer(LayerSpec::Panel(PanelSpec { id: Some(id.into()),
            rows: PanelRows::EdgeGroups { of: of.clone(), relation: "calls".into(), direction: dir }, dock, title: Some(title.into()), ..Default::default() })));
        self.panels.configure_last(|p| { p.captures_mouse = true; p.center = Some(self.focus.current.clone()); });
    }
    // active = callees if non-empty else callers — applied by sync() on the first resolve (L2444–2448)
}
```
`OpenFilePalette`/`OpenSymbolPalette` (L4260–4277) call `open_palette(true/false)`; the guard `map_interaction_enabled()` stays. Tab in `on_nav_key` (L2308) calls `push_call_graph_panels`. `close_all_panels()` = `RemoveLayer` for every open panel (used at L2796/L4239/L4253 and by Esc).

`RemoveLayer(usize)` shifts later indices; `PanelState::sync` re-derives `layer` from `resolved.panels[i].layer` by matching `id` (or by position for id-less panels), so instances survive removals of *other* layers.

---

## 6. Commands

| verb / command | effect |
|---|---|
| `outrider panel <set> [--columns churn,fanin] [--sort churn\|name] [--dock right] [--title T] [--limit n]` | `PushLayer(Panel{ rows: Set(Name(set)), .. })` |
| `outrider layer rm <i>` / `pop` | closes the panel (`RemoveLayer`/`PopLayer`) |
| `outrider query selection` | returns `SessionState.selection` (`panel_selection()`) |
| `outrider frame`/`focus` from a panel row | app issues `Camera(Focus(id))` (09-camera) on Enter |
| app: Ctrl+P / Ctrl+T | `DefineSet("__palette", …)` + `PushLayer(Panel{id:"__palette"…})`; typing → `DefineSet` |
| app: Tab | two `PushLayer(Panel{ EdgeGroups })`; Esc/Tab → two `RemoveLayer` |
| app: mouse click on a row | select + Enter |

Because everything goes through `apply_view_command`, an agent can open the palette by pushing the same document, and `query view` shows the panels a user has open.

---

## 7. Invalidation

| bit | set by | consumed |
|---|---|---|
| `SPEC` | `PushLayer/RemoveLayer/DefineSet` (palette typing) | panel (re)resolution |
| `FOCUS` | Enter on an EdgeGroups row, any focus move | `of: $focus` panels |
| `SELECTION` | `on_panel_key` Up/Down, `sync` resets, mouse click | **not** by panels themselves (highlight is app state); by 02-set `selection`, 07-notes `$selection`, 06-marks `selection` kind |
| `TREE` | loader | all panels (ids re-looked-up); `close_all_panels()` also runs in `start_loading` |
| `METRICS`/`GIT` | import / git watcher | panels with `columns` or metric `sortBy` |
| relation completion | 05-edges' provider poll (ORs `SPEC` or its own bit) | `pending` panels |
| `CAMERA` | — | none: docked column x is computed at render from the camera, not at resolve |

---

## 8. Migration steps

Behaviour-preserving; each step builds and passes the existing tests until they are replaced.

1. **Types + validation** (§3). Fixture `tests/fixtures/panel-*.json` for the three JSON examples. Test `panel_json_roundtrip`, `panel_matrix_warns`, `panel_duplicate_id_rejected`.
2. **`fuzzy_match` → `outrider-index/src/search.rs`** (with its tests; this is 02-set.md's decision — do not duplicate it in `outrider-view`); `palette.rs` re-exports it. `SetExpr::Fuzzy` in 02-set uses it over `node.name` (the palette matched on `name`, not path — keep that).
3. **`layers/panel.rs`**: `group_edges` (moved), `resolve_panel`, `ResolvedPanel/Row/RowAlternate`, sorting, limit; wired into `ViewResolver::resolve` step (7). Tests §9 (`sort_by_metric`, `edge_grouping_equals_group_edges`, `name_length_sort_matches_palette`).
4. **`view/panel_view.rs` skeleton**: `PanelState/PanelInstance/ScrollTween`, `panel_rect`, `call_graph_column_lefts` (moved), `source_lines` (moved from `cg_source_lines`), `cg_parent_name` (moved), `render_panels` for **Float + Set rows only**, `on_panel_key` for the Set-rows column of the table, `open_palette`, `close_all_panels`. Add `TreemapView.panels`; keep `TreemapView.palette` and `call_graph` alive in parallel.
5. **Palette cut-over**: `OpenFilePalette/OpenSymbolPalette` → `open_palette`; `on_key_down` L2209 → `on_panel_key`; `has_overlays` → `panels.has_float()`; `palette_overlay` → `render_panels`; delete `render_palette`, `on_palette_key`, field `palette`, and `palette.rs` except the `fuzzy_match` re-export (or delete the file once 02-set owns it). Acceptance: `palette_golden` (§9) + manual: Ctrl+P, type, Up/Down wrap, Enter frames, Esc closes, preview identical.
6. **Call-graph columns cut-over** (after 05-edges' `calls` provider exists — or with the interim `cg_resolver` bridge of §4.2): `render_panels` docked EdgeGroups cards; `on_panel_key` EdgeGroups rows; `push_call_graph_panels`; Tab/Esc; `captures_mouse` guards replace `self.call_graph.is_some()` in the five mouse handlers; `cg_animating` → `panels.is_animating()`. Delete `CallGraphMode`, `CgEdgeGroup`, `group_edges` (app copy), `CG_*` consts, `CgScrollState`, `cg_scroll_target`, `cg_card_top`, `cg_card_height`, `CallGraphSelection`, `CgColumnItem`, `enter_call_graph`, `exit_call_graph`, `on_call_graph_key`, `render_call_graph`, `render_cg_column`. `cg_scrim` (paint_items L1805 and Render L4499–4508/4532/4560) is deleted by **04-mask** when the `dimExcept` mask layer is pushed alongside the two panels; until then `cg_scrim = self.panels.captures_mouse()` keeps the visual. `cg_highlight_lines` (L1458–1478) is fed by `panels.selected_call_site()` until 06-marks turns it into a range Mark.
7. **`panel` CLI verb** with 10-rpc-and-watcher.md.

---

## 9. Tests

`outrider-view` (`crates/outrider-view/src/layers/panel.rs` and `tests/panel.rs`, fixture `mini_repo`):
- `panel_json_roundtrip` — the three §3 documents; `sortBy: "churn"` parses as `Metric`, `"nameLength"` as `Builtin`.
- `panel_matrix_warns`, `panel_duplicate_id_rejected`, `panel_unknown_relation_rejected`, `panel_limit_zero_rejected`.
- `sort_by_metric` — set of 5 file ids with distinct `churn_count`; `columns: ["churnCount"]`, `sortBy: "churnCount"` → rows descending, cell raw equals `node.churn_count`; a node with `None` metric sorts last; ties broken by id.
- `name_length_sort_matches_palette` — build `Palette` (kept in the test as `legacy_palette` copied from palette.rs) over the palette test tree; for queries `""`, `"t"`, `"tk"`, `"prs"` assert `resolve_panel(Set(__palette), sortBy NameLength, limit 12).rows.map(id) == legacy.results`. This is the **palette golden**.
- `edge_grouping_equals_group_edges` — feed a fixed `Vec<EdgeDetail>` (targets a,b,a,c with raw names x,y,x,z) → groups `[x:[a,a], y:[b], z:[c]]`, and `resolve_panel(EdgeGroups)` rows `= [a(group x, 2 alternates), b, c]`; compare against the legacy `group_edges` copied into the test.
- `edge_groups_pending` — provider returns `None` → `rows.is_empty() && pending`.
- `panel_deps_exclude_selection` — `deps` of a `Set` panel does not contain `SELECTION`; a `$focus` EdgeGroups panel contains `FOCUS`.

App crate (`crates/outrider/src/view/panel_view.rs` `#[cfg(test)]`; pure functions, no GPUI):
- `keyboard_selection_updates_selection` — `PanelState` with one Set panel of 3 rows: `key("down")` → `selection == 1`, returns `Deps::SELECTION`, `selected() == rows[1].id`; `key("up")` at 0 on a float panel wraps to 2 (palette semantics), on a docked panel stays 0 (call-graph semantics). Implement `on_panel_key` as a thin wrapper around a pure `fn panel_key(state: &mut PanelState, panels: &[ResolvedPanel], key: &str, ch: Option<char>) -> PanelKeyEffect` (`enum PanelKeyEffect { None, SelectionChanged, Enter(SymbolId), Close(Vec<usize>), QueryChanged(String) }`) so this is testable.
- `left_right_switch_and_cycle` — two docked EdgeGroups panels (callers Left, callees Right), active = callees, row 0 has 2 alternates: `right` → `group_active[0] == 1`; `left` → active = callers, its selection 0; `left` again on callers row with 3 alternates → `group_active == 2` (backwards wrap); `right` → back to callees.
- `sync_resets_on_rows_change` — after rows change, `selection == 0`, `active` = last-pushed non-empty panel; when the query only narrows the palette, `selection` clamps instead.
- `panel_rect_matches_call_graph_columns` — `panel_rect(Left/Right, ..)` equals `call_graph_column_lefts(..)` outputs and `(48, vh-96)`; `Float` equals `render_palette` centring for `PALETTE_W + GAP + PREVIEW_W`.
- `card_geometry_golden` — `card_top/card_height` equal the deleted `cg_card_top/cg_card_height` for `selected ∈ {None, 0, 2}` over 5 cards.

Manual acceptance: Ctrl+P/T look and behave identically (12 results, same order, preview pane); Tab on a fn opens the two columns at the same positions, "Resolving..." then cards, Up/Down/Left/Right/Enter/Esc/Tab as before, map frozen while open, selected callee's call site still highlighted; both open at once (palette over call-graph) → palette gets the keys; Esc closes the palette only.

---

## 10. Open questions / risks

- **05-edges dependency.** EdgeGroups needs `raw_name` and `call_site`, which the framework's `RelationProvider` doesn't expose. 05-edges.md provides this as `RelationProvider::edge_detail(from, to, ctx) -> Option<EdgeDetail{ raw_name, site, weight }>` plus `lookup() -> Lookup::{Ready, Pending}`; use those names (the earlier `edges_detailed` name in this file is superseded). If 05-edges lands later than this spec, the interim bridge (`RelationRegistry` reading `call_graph_cache` + `cg_resolver` polling in `panel_view`) keeps the call-graph columns working; it is throwaway.
- **`selection` when the palette narrows** — clamping keeps today's feel but means `SessionState.selection` can change without a key press; `sync` returns `true` so `SELECTION` is set.
- **Float rows are now clickable** and docked cards are too; today only keys work. Additive; keep.
- **Topmost-wins with a docked pair + palette**: Esc closes only the palette layer (its own gesture group), leaving the columns — is that desired, or should Esc close everything? Chosen: per-gesture group; revisit.
- **`limit` truncation happens after sort** — for large `fuzzy("")` sets this sorts everything each keystroke (today's palette pre-sorts candidates once). If it shows up in profiles, cache the sorted candidate order per `(kind_filter)` in the set resolver's memo (02-set) — the fuzzy filter is a subsequence check, cheap.
- **`Bottom` dock** has no consumer yet; geometry is specified so documents render, but no keyboard special-casing.

---

## 11. Framework deltas

1. **`RelationProvider::edges_detailed`** (05-edges): `fn edges_detailed(&self, id: &SymbolId, dir: EdgeDirection, ctx: &ProviderCtx) -> Option<Vec<EdgeDetail { target: SymbolId, raw_name: String, call_site: Option<Range<usize>>, weight: f64 }>>` with `None` = pending; plus an app-side hook so a completed async resolve marks the view dirty (proposal: the provider owns a `pending_ready: AtomicBool` the app polls each frame and ORs `Deps::SPEC` — or a new `Deps::RELATIONS` bit `1 << 8`, which is the cleaner choice; `deps.rs` has room).
2. **`SessionState.selection`** is already in 00-framework; this spec only pins *who* writes it (`PanelState`, via `TreemapView::panel_selection`).
3. **`MetricRegistry::evaluate(&MetricRef, &SymbolNode) -> Option<MetricValue>`** assumed from 03-fill (percentile over laid-out nodes, cached). If 03-fill exposes only `MetricProvider::value`, `resolve_panel` computes the percentile table itself over `ctx.index.iter()`.
4. **`WireSymbolId::live("$focus")`** / `SetExpr::Ids` accepting live pseudo-ids is 02-set's; used here for `of`.
5. **`ViewCommand`** — no new variants. `RemoveLayer(usize)` is enough given `PanelSpec.id` + `PanelState::sync` re-deriving indices. (A `RemoveLayerById(String)` would be nicer for the CLI; optional.)
6. `PanelSpec.id` and `PanelSpec.limit` are schema additions beyond parent §2.8/§8.8 (both `Option`, backwards compatible).
