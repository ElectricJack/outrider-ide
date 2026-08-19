# 00 — Framework: `outrider-view` crate, resolver, and app integration

**Parent:** [../view-primitives.md](../view-primitives.md) §7
**Depends on:** nothing (this is the base every component spec builds on)
**Deliverable:** a new GPUI-free crate `crates/outrider-view`, the app-side `view/` module in `crates/outrider`, and the plumbing that routes today's paint decisions through a `ResolvedView`. After this spec lands the app behaves exactly as before; the difference is that the churn stripe, focus ring, and neighbor rings are produced by view layers (see [03-fill.md](03-fill.md), [06-marks.md](06-marks.md)) rather than by hard-coded branches.

This document is written so an implementer can work from it without the parent doc open, but the parent's §2–§3 (primitive definitions, channel ownership) are the authority when anything here is ambiguous.

---

## 1. Ground truth: what exists today

Read these before starting; the spec below refers to them by name.

| Thing | Where | Role |
|---|---|---|
| `SymbolId { kind, qualified_path, ordinal }`, `SymbolNode`, `SymbolTree` | `crates/outrider-index/src/types.rs` | Stable ids and the tree every view is keyed on. `SymbolNode` already carries `measure`, `churn` (percentile 0..1), `churn_count`, `doc`, `signature`, `byte_range`, `children`. |
| `PackLayout { rects: BTreeMap<SymbolId, Rect> }`, `pack(tree, cfg)` | `crates/outrider-layout/src/pack.rs` | World-absolute rects. Immutable per session (re-packed only on re-index / settings change). |
| `Camera` (`world_to_screen`, `screen_to_world`, `fit`, `frame_rect`, `CameraTween`) | `crates/outrider/src/camera.rs` | Viewport. |
| `world::visible_nodes(tree, layout, camera, vw, vh, has_thumbnail) -> Vec<DrawItem>` | `crates/outrider/src/world.rs` | Culling + LOD (`Draw::Container(Rung)` / `Draw::Leaf(LeafDraw)`); pre-order. |
| `PaintItem`, `NameRow`, `BodyText`, `TexQuad`, `DocPanel` | `crates/outrider/src/paint_model.rs` | Owned paint instructions consumed by the canvas closure. |
| `TreemapView::paint_items(&mut self, vw, vh) -> (Vec<PaintItem>, Option<DocPanel>, bool /*cg_scrim*/)` | `crates/outrider/src/treemap.rs` ~L1433–1807 | The single place `DrawItem` becomes `PaintItem`. Today it hard-codes: `stripe` from `settings.show_churn && node.churn > 0` (L1691), `focused` (L1693), `neighbor` from `self.neighbors` (L1695), doc-panel from `hover_id`/focus (L1673), call-graph highlight lines (L1458). |
| Canvas closure in `impl Render for TreemapView` | `treemap.rs` ~L4307–4604 | Pass 1 surfaces/stripes/textures; 2a leaf text; 2b headers; scrim if `cg_scrim`; 2c neighbor rings; 2d deferred focused leaf; 3 focus ring; 4 doc panel. |
| `Focus { current, last_child }`, `focus::neighbors(id, layout, index) -> [Option<SymbolId>;4]`, `TreeIndex` | `crates/outrider/src/focus.rs` | Focus cursor and arrow targets. |
| `NavigationHistory` | `crates/outrider/src/navigation.rs` | Alt+Left/Right. |
| `Palette`, `fuzzy_match` | `crates/outrider/src/palette.rs` | Ctrl+P/T search. |
| `CallGraphMode`, `CgEdgeGroup`, `render_call_graph`, `on_call_graph_key`, `call_graph_cache`, `cg_resolver` | `treemap.rs` ~L635–700, L2430–2500, L3230+ | Current call-graph overlay. |
| `Settings { filter_extensions, filter_folders, filter_files, show_churn, max_display_lines, node_padding, .. }` | `crates/outrider/src/settings.rs`; `project_settings.rs` | Filters are applied at **index time** (`ProjectLoader::start(folder, settings)`), not at layout time. |
| `content::{card_meta, churn_readout, kind_counts, inventory, body_lines}` | `crates/outrider/src/content.rs` | Text readouts shown at Card/Detail rungs. |
| `BufferManager::get(rel_path, symbols) -> Option<&Materialized>`; `Materialized.buffer.byte_to_line`, `.line(i)`, `symbol_start_line` | `crates/outrider/src/buffers.rs` | Retained source + highlight spans; how byte ranges become line ranges. |
| `outrider_index::call_graph::resolve_calls(&SymbolId, &SymbolTree) -> CallGraphData { callers, callees: Vec<CallEdge{target, raw_name, call_site}> }` | `crates/outrider-index/src/call_graph.rs` | The only relation resolver today. Synchronous, per-symbol, re-parses the file. |
| `theme::{box_fill, border_for, churn_heat, node_box_kind, node_box_tint, FOCUS_BORDER, NEIGHBOR_BORDER, ..}` | `crates/outrider/src/theme.rs` | Colors. |
| `outrider-dump` binary, `dump::render` | `crates/outrider-index/src/bin`, `src/dump.rs` | Offline text dump of the tree — the seed of offline `query`. |

Workspace: `Cargo.toml` at root lists `crates/outrider-index`, `crates/outrider-layout`, `crates/outrider`. Rust 1.89. Deps available in the app crate: `serde`, `serde_json`, `gpui`, `cosmic-text`, `image`, `dirs`, `rfd`, `trash`.

---

## 2. What to build

### 2.1 New crate `crates/outrider-view`

`Cargo.toml`:
```toml
[package]
name = "outrider-view"
version = "0.1.0"
edition = "2021"
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[dependencies]
outrider-index = { path = "../outrider-index" }
outrider-layout = { path = "../outrider-layout" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
globset = "0.4"          # for SetExpr::Glob (see 02-set.md)

[dev-dependencies]
tempfile = "3"
```
Add `"crates/outrider-view"` to the workspace `members`. **No `gpui` dependency, ever** — this crate must build for the CLI and for headless tests.

Module layout:
```
crates/outrider-view/src/
  lib.rs        pub mod spec; pub mod set; pub mod metric; pub mod relation;
                pub mod layers; pub mod resolve; pub mod command; pub mod validate; pub mod deps;
                pub use spec::*; pub use resolve::{ResolvedView, ViewResolver, SessionState};
  deps.rs       Deps bitset (2.3)
  spec.rs       ViewSpec + all *Spec types, serde (2.2)
  set.rs        SetExpr, SetResolver, ResolvedSet            → 02-set.md
  metric.rs     MetricRef, MetricProvider, MetricRegistry, Scale, ImportedMetric → 03-fill.md
  relation.rs   RelationProvider, RelationRegistry           → 05-edges.md
  layers/mod.rs pub mod fill; mask; edges; marks; notes; panel;
  layers/*.rs   per-layer resolution                         → 03..08
  resolve.rs    ViewResolver, ResolvedView, SessionState (2.4)
  command.rs    ViewCommand, ViewPatch, apply/patch (2.5)
  validate.rs   Violation, validate(&ViewSpec) (2.6)
  symbol_id.rs  string form <-> SymbolId (2.7)
```

### 2.2 `spec.rs` — the document types

These are the serde mirror of the JSON schema in the parent doc §5. Field names are `camelCase` on the wire (`#[serde(rename_all = "camelCase")]` on every struct/enum). Unknown fields are an error (`#[serde(deny_unknown_fields)]`) so an agent gets told about typos.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewSpec {
    #[serde(rename = "outriderView", default = "one")] pub version: u32,
    #[serde(default)] pub meta: Meta,
    #[serde(default)] pub space: SpaceSpec,
    #[serde(default)] pub sets: BTreeMap<String, SetExpr>,
    #[serde(default)] pub metrics: BTreeMap<String, ImportedMetric>,
    #[serde(default)] pub layers: Vec<LayerSpec>,
    #[serde(default)] pub camera: CameraSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Meta { pub title: Option<String>, pub author: Option<String>, pub created_at: Option<String> }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum LayerSpec {
    Fill(FillSpec), Mask(MaskSpec), Edges(EdgesSpec), Marks(MarksSpec), Notes(NotesSpec), Panel(PanelSpec),
}
// serde: externally tagged → {"fill": {...}} exactly as in the schema.

/// A reference to a set: by name, or an inline expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SetRef { Name(String), Inline(Box<SetExpr>) }
```

`SpaceSpec`, `SetExpr`, `FillSpec`, `MaskSpec`, `EdgesSpec`, `MarksSpec`, `NotesSpec`, `PanelSpec`, `CameraSpec`, `ImportedMetric` are defined in their component specs; **`spec.rs` owns them all** (one file so serde derives are in one place), and the component files describe their fields. Keep `Default` for every spec type so `ViewSpec::default()` is a valid empty view (`space` defaults to `treemap`).

### 2.3 `deps.rs` — dependency bits

```rust
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Deps(u32);
impl Deps {
    pub const NONE: Deps = Deps(0);
    pub const TREE: Deps = Deps(1 << 0);       // tree/layout replaced (re-index, packing snapshot)
    pub const FOCUS: Deps = Deps(1 << 1);
    pub const CAMERA: Deps = Deps(1 << 2);
    pub const SELECTION: Deps = Deps(1 << 3);  // panel row
    pub const HOVER: Deps = Deps(1 << 4);
    pub const GIT: Deps = Deps(1 << 5);        // HEAD / worktree changed
    pub const SPEC: Deps = Deps(1 << 6);       // the ViewSpec itself changed
    pub const METRICS: Deps = Deps(1 << 7);    // any imported metric (re)loaded
    pub fn union(self, o: Deps) -> Deps; pub fn intersects(self, o: Deps) -> bool; pub fn contains(...)
}
```
Every resolved artifact records the `Deps` it was computed from. `TreemapView` accumulates a `dirty: Deps` and re-resolves anything whose deps intersect it. Keep it a bitset — no per-metric bits; imported metrics are all-or-nothing under `METRICS` (they change rarely).

### 2.4 `resolve.rs` — the resolver

```rust
pub struct SessionState<'a> {
    pub focus: &'a SymbolId,
    pub hover: Option<&'a SymbolId>,
    pub selection: Option<&'a SymbolId>,       // current Panel row
    pub visible: Option<&'a [SymbolId]>,       // this frame's DrawItems, if a set needs them
    pub head: Option<&'a str>,                 // git HEAD sha, if known
    pub neighbors: Option<&'a [Option<SymbolId>; 4]>, // focus::neighbors output (computed by the app)
}

pub struct ResolveCtx<'a> {
    pub tree: &'a SymbolTree,
    pub index: &'a outrider_index::TreeIndex,  // see 2.8: TreeIndex moves to outrider-index
    pub layout: &'a PackLayout,
    pub metrics: &'a MetricRegistry,
    pub relations: &'a RelationRegistry,
    pub session: SessionState<'a>,
    pub repo_root: &'a Path,
}

pub struct ResolvedView {
    pub sets: BTreeMap<String, ResolvedSet>,
    pub fill: Option<ResolvedFill>,      // channel: fill  (last wins)
    pub stripe: Option<ResolvedFill>,    // channel: stripe (last wins)
    pub opacity: Vec<ResolvedFill>,      // channel: opacity (multiply)
    pub mask: MaskTable,
    pub edges: Vec<ResolvedEdgeLayer>,
    pub marks: MarkTable,
    pub notes: NoteTable,
    pub panels: Vec<ResolvedPanel>,
    pub camera: CameraSpec,              // copied through; app acts on it
    pub deps: Deps,                      // union of everything above
    pub warnings: Vec<String>,           // e.g. "set 'affected': 3 ids not found"
}

pub struct ViewResolver { /* caches keyed by (spec hash, deps) — see below */ }
impl ViewResolver {
    pub fn new() -> Self;
    /// Full or partial re-resolution. `dirty` says which session bits changed since the
    /// last call; artifacts whose deps don't intersect `dirty` are reused from cache.
    pub fn resolve(&mut self, spec: &ViewSpec, ctx: &ResolveCtx, dirty: Deps) -> &ResolvedView;
    pub fn invalidate_all(&mut self);
}
```

Resolution order inside `resolve`: (1) sets, in dependency order (a `SetRef::Name` may reference another named set; detect cycles → warning + empty set); (2) fill/stripe/opacity; (3) mask; (4) edges; (5) marks; (6) notes; (7) panels. Layers are resolved in `spec.layers` order and *combined* by the channel rules (parent §3.2): for `fill`/`stripe` the last layer wins, masks AND into one `MaskTable`, everything else appends.

Caching: `ViewResolver` keeps `HashMap<u64 /*structural hash of SetExpr*/, (ResolvedSet, Deps)>` and one cached `ResolvedView`. On `resolve`, a set is recomputed if `dirty ∩ set.deps ≠ ∅` or `dirty ∋ SPEC` and its hash isn't in the cache; a layer is recomputed if any set it references was recomputed, or its own provider deps intersect `dirty`. Simple and good enough — the tree is immutable per session, so most frames recompute nothing.

`ResolvedSet`, `ResolvedFill`, `MaskTable`, `ResolvedEdgeLayer`, `MarkTable`, `NoteTable`, `ResolvedPanel` are defined in the component specs.

### 2.5 `command.rs`

```rust
pub enum ViewCommand {
    Apply(ViewSpec),
    Patch(ViewPatch),
    Clear(ClearScope),                       // Layers | Sets | All
    DefineSet { name: String, expr: SetExpr },
    PushLayer(LayerSpec),
    PopLayer,
    RemoveLayer(usize),
    ImportMetric { name: String, metric: ImportedMetric },
    Camera(CameraCommand),                   // Frame(SetRef) | Focus(SymbolId) | Home | Follow(..) | Step(..)  → 09-camera.md
    Tour(TourCommand),                       // → 09-camera.md
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewPatch {
    pub space: Option<SpaceSpec>,
    #[serde(default)] pub sets: BTreeMap<String, SetExpr>,          // insert/replace by name
    #[serde(default)] pub metrics: BTreeMap<String, ImportedMetric>,
    #[serde(default)] pub layers: Vec<LayerSpec>,                   // appended
    pub camera: Option<CameraSpec>,
}

pub struct Applied { pub changed: Deps /* SPEC, plus METRICS if metrics changed */ , pub violations: Vec<Violation> }
pub fn apply(cmd: ViewCommand, spec: &mut ViewSpec) -> Applied;
```
`apply` validates the *result* first (clone → mutate → `validate`); if there are hard violations the spec is left untouched and they are returned. Semantics: `Apply` replaces everything except `metrics` (imported data survives a view swap unless the new spec redefines the same name); `PushLayer` for a `Fill` with the same channel *appends* (resolution's last-wins makes it active) — do not dedupe, so `PopLayer` restores the previous fill; `DefineSet` replaces the name.

### 2.6 `validate.rs`

```rust
pub struct Violation { pub path: String /* e.g. "layers[2].fill.metric" */, pub rule: &'static str, pub message: String }
pub fn validate(spec: &ViewSpec, known_metrics: &dyn Fn(&str)->bool, known_relations: &dyn Fn(&str)->bool) -> Vec<Violation>;
```
Rules (hard = reject): unknown `SetRef::Name`; `Fill` with unknown metric; `Marks` with a structural kind and no `basis`; `Notes` entry with `source: agent` and no text; `Edges` with unknown relation; `SetExpr::Ref` cycles; `version != 1`. Soft (warning, allowed): a saved view referencing `focus`/`selection`/`visible` (fine at runtime, meaningless offline). There is no coordinate field anywhere in the schema, so "no coordinates" is enforced by the types.

### 2.7 `symbol_id.rs`

Wire form used everywhere outside Rust: `"<kind>:<qualified_path>"` with `"#<ordinal>"` appended when `ordinal != 0`. `kind` is `folder | file | chunk | <item label>` (e.g. `fn`, `struct`, `class` — whatever `SymbolKind::Item{label}` holds). Provide `pub fn to_wire(&SymbolId) -> String` and `pub fn parse_wire(&str) -> Result<SymbolId, String>`; implement `Serialize`/`Deserialize` for a newtype `WireSymbolId(SymbolId)` used in the spec types so JSON stays readable. Also accept a bare path with no kind prefix and resolve it against the tree at resolution time (agents will type `src/auth/login.rs::verify`); that lookup lives in `SetResolver` (02-set.md) not here.

### 2.8 Move `TreeIndex` into `outrider-index`

`crates/outrider/src/focus.rs` defines `TreeIndex<'a>` (id → node, parent, depth). `outrider-view` needs it. Move the struct and its `new/node/parent/depth` methods to `crates/outrider-index/src/tree_index.rs`, `pub use` it from `outrider_index`, and leave a `pub use outrider_index::TreeIndex;` in `focus.rs` so no call sites change. Also add `pub fn iter(&self) -> impl Iterator<Item=&SymbolNode>` (pre-order over `nodes`) and `pub fn is_leaf_item(&SymbolNode) -> bool` (mirror of `content::is_leaf_item` — move the predicate to the index crate and re-export it in `content.rs`).

---

## 3. App integration (`crates/outrider/src/view/`)

New module `view/mod.rs` with submodules `paint_resolver.rs`, `session.rs`, and (later specs) `edge_pass.rs`, `mark_pass.rs`, `note_pass.rs`, `panel_view.rs`, `rpc.rs`, `watch.rs`. Register `mod view;` in `main.rs`.

### 3.1 New fields on `TreemapView`

```rust
/// The current view document. Starts as the session default (3.3); mutated only via ViewCommand.
view_spec: outrider_view::ViewSpec,
view_resolver: outrider_view::ViewResolver,
/// Session bits changed since the last resolve.
view_dirty: outrider_view::Deps,
metrics: outrider_view::MetricRegistry,        // built at load (3.5)
relations: outrider_view::RelationRegistry,    // built at load
```
Remove **nothing** yet; the old fields (`neighbors`, `hover_id`, `call_graph*`, `settings.show_churn`) stay until each component spec's migration step deletes them.

### 3.2 `paint_items` changes (the integration point)

At the top of `paint_items`, after the camera is settled and `neighbors` is refreshed:

```rust
let session = SessionState { focus: &focus_id, hover: self.hover_id.as_ref(), selection: self.panel_selection(), visible: None, head: self.git_head.as_deref(), neighbors: self.neighbors.as_ref().map(|(_, n)| n) };
let ctx = ResolveCtx { tree: &self.tree, index: &index, layout: &self.layout, metrics: &self.metrics, relations: &self.relations, session, repo_root: &self.tree.repo_root };
let resolved = self.view_resolver.resolve(&self.view_spec, &ctx, std::mem::take(&mut self.view_dirty));
```
(`visible` is `None` here; sets that need `visible()` are resolved in a second, cheap pass after `visible_nodes` runs — see 02-set.md. Borrow-checker note: `resolve` needs `&self.view_spec` and `&mut self.view_resolver` simultaneously with `&self.tree`; split borrows by destructuring `self` fields into locals at the top of the function as `packing_geometry()` already does.)

Then introduce `view::paint_resolver::PaintOverrides`:

```rust
pub(crate) struct PaintOverrides<'a> { resolved: &'a ResolvedView }
impl PaintOverrides<'_> {
    pub fn stripe(&self, id: &SymbolId) -> Option<u32>;                 // 03-fill.md
    pub fn fill(&self, id: &SymbolId, base: u32) -> u32;               // 03-fill.md (base = today's theme fill)
    pub fn light(&self, id: &SymbolId) -> f32;                          // 04-mask.md, 1.0 = unmasked
    pub fn glyphs(&self, id: &SymbolId) -> &[Glyph];                    // 06-marks.md
    pub fn range_marks(&self, id: &SymbolId) -> &[(Range<usize>, MarkStyle)];
    pub fn is_focus_ring(&self, id) -> bool; pub fn is_neighbor(&self, id) -> bool; // 06-marks.md nav kinds
    pub fn notes(&self, id: &SymbolId) -> &[ResolvedNote];              // 07-notes.md
}
```
and replace in the `PaintItem` construction:
- `stripe: (self.settings.show_churn && ..)` → `stripe: ov.stripe(&item.node.id)`
- `fill` → `ov.fill(&id, fill)` (identity until a fill layer with `channel: fill` exists)
- `focused: is_focused` → `focused: ov.is_focus_ring(&id)`; `neighbor: …` → `ov.is_neighbor(&id)`
- `body_opacity`, `tex_opacity` → multiplied by `ov.light(&id)`

Add to `PaintItem` (paint_model.rs): `pub glyphs: Vec<Glyph>`, `pub range_marks: Vec<RangeMark>`, `pub light: f32`. Their rendering is specified in 06-marks.md / 04-mask.md; in this spec they are populated but the canvas ignores them.

### 3.3 The session default view (`view/session.rs`)

`pub(crate) fn default_view(settings: &Settings) -> ViewSpec` builds:

```
sets:
  neighbors  = { "neighbors": "focus" }
  focusSet   = { "ids": ["$focus"] }          # "$focus" / "$hover" / "$selection" are session pseudo-ids (02-set.md)
  hoverSet   = { "ids": ["$hover"] }
layers:
  { fill:  { metric: "churn", channel: "stripe", scale: "percentile" } }   # only if settings.show_churn
  { marks: { on: "focusSet",  kind: "focusRing" } }
  { marks: { on: "neighbors", kind: "neighbor" } }
  { notes: [ { at: "$hover", source: "doc" }, { at: "$focus", source: "doc" } ] }   # 07-notes.md
camera: { follow: "focus" }
```
`space.exclude` is **not** populated from `filter_*` in this milestone: those filters run at index time (`ProjectLoader::start(folder, settings)`) and stay there (see 01-space.md for the layout-time exclusion that *is* added). Rebuild the default view whenever settings change (`show_churn` toggle → `ViewCommand::Apply(default_view(&settings))` — or, better, the toggle becomes `PushLayer`/`RemoveLayer` of the churn fill so any user-added layers survive; do the latter).

### 3.4 Dirty-bit wiring

Set `self.view_dirty |= …` at these existing sites:
| site | bit |
|---|---|
| every `self.focus.set/step_in/step_out` call and arrow-key moves in `on_key_down` | `FOCUS` |
| `on_mouse_move` when `hover_id` changes | `HOVER` |
| camera pan/zoom/tween sample (`on_scroll`, drag in `on_mouse_move`, tween in `paint_items`) | `CAMERA` |
| `PackingGeometryState::apply_snapshot/finish`, loader completion (`poll_loading` success path), `start_loading` | `TREE` (+ `view_resolver.invalidate_all()`) |
| after `apply(ViewCommand)` | whatever `Applied.changed` says |
| git watcher tick (added in 02-set.md) | `GIT` |

`CAMERA` is set nearly every frame during motion; nothing in this milestone depends on it, so it costs nothing. Later specs (edges aggregation, `visible()`) consume it.

### 3.5 Registries built at load

In the loader completion path (where `self.tree` is replaced), build:
```rust
self.metrics = MetricRegistry::builtin(&self.tree);   // churn, churnCount, measure, entities (03-fill.md)
self.relations = RelationRegistry::builtin();         // "calls" wrapping resolve_calls with a cache (05-edges.md)
```
and keep imported metrics from `view_spec.metrics` registered (re-register after rebuild).

### 3.6 Applying commands from the UI

Add `fn apply_view_command(&mut self, cmd: ViewCommand) -> Vec<Violation>` on `TreemapView`: calls `outrider_view::command::apply`, ORs `Applied.changed` into `view_dirty`, pushes each violation as a `Notification::warning`, and `cx.notify()`s (caller's responsibility). All UI paths that change view state (churn toggle now; palette/call-graph/panels later) go through this one function. This is what later lets RPC (10-rpc-and-watcher.md) reuse the exact same path.

---

## 4. Testing

- `outrider-view` unit tests (in-crate, no fixtures needed for the framework itself): `ViewSpec` round-trips through JSON for the parent doc §5 example (check the example into `crates/outrider-view/tests/fixtures/example-view.json`); `deny_unknown_fields` rejects a typo; `apply(Apply)` preserves `metrics`; `PushLayer(Fill)` then `PopLayer` restores the previous stripe fill; validation catches unknown set ref / metric / structural mark without basis / agent note without text; `Deps` algebra.
- Resolver tests use `crates/outrider-index/tests/fixtures/mini_repo` via `outrider_index::index_repo` (see `index_test.rs` for how) and `outrider_layout::pack` with a small `PackConfig` (copy the constants from `world::pack_config`).
- **Golden test in the app crate** (the acceptance criterion for this spec): before touching `paint_items`, add a `#[cfg(test)]` helper that constructs `TreemapView` state headlessly is *not* feasible (needs GPUI `Context`). Instead: extract the per-item decision block into a pure function `fn paint_decisions(item: &DrawItem, ov: &PaintOverrides, ..) -> (fill, border, stripe, focused, neighbor, light)` and test *that* against a hand-built `ResolvedView` for the session default: stripe present iff churn > 0 and show_churn; focused iff id == focus; neighbor iff id ∈ neighbors. Assert `light == 1.0` everywhere.
- Manual acceptance: build, open a repo, verify no visible change; toggle Git Churn → stripes come and go; arrow keys still show neighbor rings; focus ring intact.

---

## 5. Migration checklist for this spec

1. Create `outrider-view` crate with `spec.rs`, `deps.rs`, `symbol_id.rs`, `validate.rs`, `command.rs`, `resolve.rs` skeleton (empty resolvers returning empty tables), and the two minimal layer implementations this milestone needs: `layers/fill.rs` stripe path (03-fill.md §"Milestone-0 subset") and `layers/marks.rs` nav kinds (06-marks.md §"Milestone-0 subset"), plus `set.rs` with `Ids`, `Neighbors`, `Union` and the `$focus/$hover` pseudo-ids (02-set.md §"Milestone-0 subset").
2. Move `TreeIndex` to `outrider-index` (2.8).
3. Add fields (3.1), `default_view` (3.3), `apply_view_command` (3.6), dirty wiring (3.4), registries (3.5).
4. Route stripe/focused/neighbor through `PaintOverrides` (3.2); delete the `settings.show_churn` check in `paint_items` (keep the setting; the toggle now pushes/removes the fill layer).
5. Tests (§4). Do not touch `cg_scrim`, `hover_id` doc panel, palette, or call-graph code in this milestone.

Everything else — real set expressions, metrics beyond churn, masks, edges, notes, panels, camera commands, RPC, CLI — is specified in the numbered files alongside this one and builds on these types, subject to the adopted amendments in §6.

---

## 6. Consolidated framework deltas (adopted)

The component specs (01–11) were written against §1–§5 and each reported changes the framework must absorb. These are **adopted** and override the corresponding text above; where a component spec's own §11 lists something not repeated here, it was either folded into an item below or is local to that component. Implement §1–§5 *with* these amendments.

### 6.1 Module layout (amends §2.1)

`outrider-view/src/` additionally contains:
- `space.rs` — `SpaceKey`, `prune_tree`, `regroup_tree`, `SpaceOutcome` (01-space)
- `partition.rs` — `PartitionRef`, `Partition`, `PartitionRegistry` with builtin `topFolder` (02-set; consumed by 01-space `regroup`)
- `git.rs` — `GitProbe`, `GitCache`, `changed(...)` evaluation via `std::process::Command` (02-set)
- `lines.rs` — `LineTable`/`LineTables` (byte↔line per file, loaded with `std::fs::read`), shared by 03-fill's `file:line` importer and 06-marks' `lines:` anchors
- `camera.rs` — `union_rect`, `layers_at_step`, `diff_layers`, `TourState` (09-camera)
- `relation/` becomes a directory module: `relation/mod.rs` (`RelationProvider`, `RelationRegistry`, `Lookup`, `EdgeDetail`, `ProviderCtx`), `relation/calls.rs` (`CallsProvider` + `CallsWorker`, the moved `CallGraphResolver`) (05-edges)

App crate `crates/outrider/src/view/` additionally contains `git_watch.rs` (02-set), `camera_ctl.rs` (09-camera), `edge_pass.rs`, `mark_pass.rs`, `note_pass.rs`, `panel_view.rs`, `rpc.rs`, `watch.rs`.

`outrider-index` gains `search.rs` (`fuzzy_match`, moved from `palette.rs`), `tree_index.rs` (§2.8), and `pub` `churn::git_command/git_stdout`; `ast_metrics.rs` is a follow-up (03-fill). `outrider-layout` gains `PackConfig::outrider_default(gap, max_display_lines)` holding the five layout constants so the CLI can pack offline; `world::pack_config` delegates to it (11-cli).

### 6.2 `Deps` (amends §2.3)

Add `pub const RELATIONS: Deps = Deps(1 << 8);` — "an async relation provider's cache gained entries". Set by the app after `self.relations.poll()` returns true each frame (05-edges, 08-panel).

### 6.3 `SessionState` / `ResolveCtx` (amends §2.4)

```rust
pub struct SessionState<'a> {
    pub focus: &'a SymbolId,
    pub hover: Option<&'a SymbolId>,
    pub hover_glyph: Option<(&'a SymbolId, usize)>,     // 06-marks: glyph slot under the cursor
    pub selection: Option<&'a SymbolId>,
    pub selection_range: Option<Range<usize>>,          // 05-edges/08-panel: `$selectionSite` = call site of the selected row
    pub visible: Option<&'a [SymbolId]>,
    pub head: Option<&'a str>,                          // sourced from GitProbe (02-set); no separate `git_head` field
    pub neighbors: Option<&'a [Option<SymbolId>; 4]>,
}
pub struct ResolveCtx<'a> {
    /* as §2.4, plus: */
    pub partitions: &'a PartitionRegistry,
}
```
Pseudo-ids accepted inside `SetExpr::Ids`: `$focus`, `$hover`, `$selection`, `$selectionSite` (the last yields a set with one member *and* a range).

Factor `ResolveCtx` construction out of `paint_items` into `TreemapView::resolve_ctx(&self, index, session) -> ResolveCtx` so `query.set` (10-rpc) can reuse it.

### 6.4 `ViewResolver` (amends §2.4)

`ViewResolver` owns: the set memo (`HashMap<u64, (ResolvedSet, Deps)>`), a `PathIndex` (bare path → SymbolId), `GitCache`, `PercentileCache` (per `(metric, domain)` — shared by Fill scales and `where(...)`), and `LineTables`. Additional methods:
```rust
pub fn resolve_set_ref(&mut self, spec: &ViewSpec, ctx: &ResolveCtx, r: &SetRef) -> &ResolvedSet;   // 09-camera frame()
pub fn needs_visible(&self) -> bool;                       // 02-set: any set uses visible()
pub fn resolve_visible(&mut self, spec, ctx_with_visible);  // second pass after visible_nodes
pub fn needs_git(&self) -> bool;
pub fn current(&self) -> &ResolvedView;
```

### 6.5 Providers (amends §2.4 / parent §7.3)

`MetricProvider` gains `fn unit(&self) -> &str` and a default `fn native_percentile(&self, node) -> Option<f32>` (churn returns `node.churn` so the golden test is exact). `MetricRegistry` gains `register_imported`, `evaluate(metric, node)`, and `readouts()/readouts_mut() -> &MetricReadoutRegistry` (07-notes) so `ResolveCtx` stays unchanged.

`RelationProvider` keeps parent §7.3's four methods and adds, with default impls: `lookup(id, dir, ctx) -> Lookup::{Ready(Vec<(SymbolId,f64)>), Pending}`, `edge_detail(from, to, ctx) -> Option<EdgeDetail{ raw_name, site, weight }>`, `prefetch(ids)`, `poll() -> bool`, `is_busy() -> bool`. `RelationRegistry::builtin(tree: Arc<SymbolTree>)` (takes the tree; the `calls` provider owns the moved `call_graph_cache` and background worker), plus `poll()`/`is_busy()`/`prefetch()` fan-out.

### 6.6 Commands (amends §2.5)

- `ViewCommand::CycleFill { channel: FillChannel }` (03-fill; hotkey `f` / `shift-f`).
- `CameraCommand { Frame(SetRef), Focus(WireSymbolId), Home, Follow(FollowMode) }`; `TourCommand { Add(Step), SetSteps(Vec<Step>), Play, Next, Prev, Goto(usize), Stop }` (09-camera; the earlier `Step(..)` mention under `CameraCommand` is `TourCommand::Goto`). `tour save` is CLI-side (`view.get` → file); `tour load-history` is an RPC method answered by the app.
- `apply` may return `Applied.changed == Deps::NONE` for one-shot `Camera(Frame|Focus|Home)`; the app still enacts them (09-camera `enact_camera`).
- `Violation` derives `Serialize, Deserialize` (10-rpc/11-cli).

### 6.7 App integration (amends §3)

- `TreemapView::apply_view_command(&mut self, cmd) -> Applied` (returns violations **and** `changed`), and after `apply` it calls `view::camera_ctl::enact_camera` for camera/tour commands (09-camera). Add `last_viewport: (f64, f64)` so commands can frame outside `paint_items`.
- `paint_items` returns a `PaintFrame { items: Vec<PaintItem>, doc_panel: Option<DocPanel>, edges: EdgeFrame, callouts: Vec<NoteCallout>, in_call_graph: bool }` struct instead of the `(items, doc_panel, cg_scrim)` tuple. `cg_scrim` is deleted by 04-mask (per-item mask replaces the global quad); `in_call_graph` survives only until 07-notes moves the doc panel to Notes; `edges` is filled by 05-edges; `callouts` by 07-notes. Introduce the struct in this milestone with `edges`/`callouts` empty so later specs only add fields.
- `PaintOverrides` additionally exposes `opacity(id) -> f32` (03), `inspect(id) -> Option<MetricReadout>` (03), `is_selection(id)` (06), and `range_marks(id) -> Vec<(Range<usize>, MarkStyle)>` (owned, since ranges are clipped per frame) (06). `PaintItem` gains `selection: bool`; `BodyText.highlighted: bool` becomes `mark: Option<MarkStyle>` (06).
- Dirty table (§3.4) gains: "focus reconciled after repack → `FOCUS`" (01-space), "`relations.poll()` → `RELATIONS`" (05), "`GitProbe` tick (1 s GPUI timer in `view/git_watch.rs`) → `GIT`" (02), "glyph hover changed → `HOVER`" (06), "panel row moved → `SELECTION`" (08).
- `ProjectLoader::start(folder, settings)` becomes `start(folder, SpaceInputs { settings, space: SpaceSpec, exclude: Option<ResolvedSet> })` and gains `start_repack` for space-id changes (01-space); `Focus::step_in` takes `&PackLayout` so it can skip pruned children (01-space).
- `Notification` gains an `info` level for non-warning toasts such as "Applied view foo.json" (10-rpc). Optional.
- Wake-up: `_view_pump: Option<Task<()>>` — a 50 ms foreground pump started with `cx.spawn` that calls `cx.notify()` when the RPC/watcher `Wake` flag is set (10-rpc). Only started when the RPC server or watcher is active.

### 6.8 Milestone-0 subsets

The "Milestone-0 subset" sections referenced from §5 step 1 are: 02-set §"Milestone-0 subset" (`ids` + pseudo-ids, `neighbors`, `union`, `ref`), 03-fill §"Milestone-0 subset" (churn stripe only), 06-marks §8.1 (nav kinds `focusRing`/`neighbor` only). Nothing else from 01–11 is required for step 1.
