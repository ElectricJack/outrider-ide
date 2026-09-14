# 05 — Edges: relation providers, the `edges` layer, and the call-graph migration

**Parent:** [../view-primitives.md](../view-primitives.md) §2.5, §3.2 (lines channel), §4 (call-graph row), §7.3 (`RelationProvider`), §8.5, §9 milestone 4
**Framework:** [00-framework.md](00-framework.md) — every type name below (`ViewSpec`, `LayerSpec`, `SetRef`, `Deps`, `SessionState`, `ResolveCtx`, `ResolvedView`, `ViewResolver`, `ViewCommand`, `validate`, `PaintOverrides`, `PaintItem`, `RelationRegistry::builtin`) is the framework's; this file only adds to them. Deltas are listed in §11.
**Depends on:** 02-set.md (`SetRef` resolution, `reach`, `$focus` pseudo-id), 04-mask.md (`dimExcept`, used by the call-graph preset), 06-marks.md (`selection` range mark), 08-panel.md (`EdgeGroups` rows).

---

## 1. Purpose and scope

The **Edges** primitive draws relations between symbols as curves with optional arrowheads (parent §2.5). It owns the *lines* channel and nothing else (§3.2): weight, per-relation color, dashed/solid, arrows. It never moves a box.

This spec covers three things:

1. **Relation infrastructure** — `crates/outrider-view/src/relation.rs`: the `RelationProvider` trait (parent §7.3) and `RelationRegistry`, with the built-in `calls` provider wrapping `outrider_index::call_graph::resolve_calls` behind a per-symbol cache fed by a background worker (today's `CallGraphResolver` moved out of `treemap.rs`), plus interface-only `imports` and `cochange` providers.
2. **The `edges` layer** — `EdgesSpec` (spec.rs), `layers/edges.rs` resolution to symbol-level `ResolvedEdgeLayer`s, and the app-side `view/edge_pass.rs` that aggregates to visible ancestors each frame and paints with GPUI paths.
3. **The call-graph mode migration** — today's `CallGraphMode` becomes a preset of Mask + Edges + two Panels + a range Mark. Behaviour-preserving: same callers/callees, same keys, same call-site highlight.

Out of scope: the `callgraph` *Space* (parent §2.1, milestone 8), hover on an edge, and edge labels.

---

## 2. Ground truth: existing code touched

| File | Symbol | ~Line | What it does today | What changes |
|---|---|---|---|---|
| `crates/outrider-index/src/call_graph.rs` | `resolve_calls(&SymbolId, &SymbolTree) -> CallGraphData` | 21–84 | Synchronous. Reads + tree-sitter-parses the center's file, extracts call names in the center's `byte_range`, matches against every `fn` in the tree (`collect_all_functions`), optional type-env receiver filtering; then `find_callers` (471–535) reads and parses **every file whose bytes contain the center's name** (memmem prefilter, per-call `file_cache`) — O(files) parses per call. | Unchanged. Wrapped by `CallsProvider`; never called on the UI thread. |
| same | `CallEdge { target, raw_name, call_site: Option<Range<usize>> }`, `CallGraphData { callers, callees }` | 8–19 | Callees carry `call_site` (byte range in the center's file); callers carry `raw_name = caller name`, no site. | Unchanged; `raw_name`/`call_site` surface through `RelationProvider::edge_detail`. |
| `crates/outrider-index/src/churn.rs` | `commit_counts_from_log`, `ChurnCache { head, counts }` | 18–33, 51–55 | Parses `git log --numstat` into **path → commit count only**. Commit→files sets are *not* retained anywhere (cache stores counts). | Unchanged here. `cochange` provider (§4.6) needs a second parse producing `Vec<Vec<path>>` per commit; interface only in this spec. |
| `crates/outrider-index/src/types.rs` | `SymbolId`, `SymbolNode.byte_range` | 38–65 | Ids keyed on `(kind, qualified_path, ordinal)`; item byte ranges are file-relative. | Unchanged. |
| `crates/outrider-layout/src/pack.rs` | `PackLayout { rects: BTreeMap<SymbolId, Rect> }`, `Rect { x, y, w, h }` | 17–52 | World rects for every laid-out symbol. | Read by edge aggregation for off-screen endpoint projection. |
| `crates/outrider/src/treemap.rs` | fields `call_graph: Option<CallGraphMode>`, `call_graph_cache: HashMap<SymbolId, CallGraphData>`, `cg_resolver: CallGraphResolver` | 556–559 | Mode state, per-symbol cache, one-inflight background resolver. | All three **deleted** (§8). Cache + worker move into `outrider_view::relation::calls`. |
| same | `CallGraphMode`, `CgEdgeGroup`, `group_edges`, `CG_*` consts, `CgScrollState`, `cg_scroll_target`, `cg_card_top`, `cg_card_height`, `CallGraphSelection`, `CgColumnItem`, `cg_parent_name` | 635–751 | Column model + scroll animation + selection. | Deleted; grouping moves to `outrider_view::layers::panel::group_edges` (08-panel.md), scroll/selection to `view/panel_view.rs`. |
| same | `InflightResolve`, `CallGraphResolver { request, poll, cancel, is_active }` | 753–816 | Spawns one `std::thread` per request with a **full `SymbolTree` clone**, generation counter for cancel, `sync_channel(1)`. | Moved to `outrider_view::relation::calls::CallsWorker`; becomes a persistent worker thread over `Arc<SymbolTree>` with a FIFO request queue (§4.3). |
| same | `call_graph_column_lefts` | 924–927 | Column x positions beside the focused (widened) leaf. | Moves to `view/panel_view.rs` (08-panel.md `dock: left/right` beside focus). |
| same | `paint_items` — `cg_highlight_lines` | 1458–1478 | Byte range of the selected callee's `call_site` → line range via `BufferManager`. | Replaced by the `selection` range mark (06-marks.md §5.4). |
| same | `paint_items` return `cg_scrim`; canvas scrim quad; ring/doc-panel skips | 1805, 4499–4508, 4532, 4560 | Global 80 % black quad over everything except the deferred focused leaf. | Deleted; the preset's `mask dimExcept` (04-mask.md) dims per item. |
| same | `on_key_down` "tab" → `enter_call_graph`; `on_call_graph_key` | 2308–2312, 2146–2149, 2617–2784 | Tab enters; Tab/Esc exit; Left/Right switch column or cycle group; Up/Down move rows; Enter re-centres. | Tab/Esc → push/pop preset layers; the rest → panel key handling (08-panel.md) (§5.5). |
| same | `enter_call_graph`, `maybe_precompute_call_graph`, `poll_call_graph`, `exit_call_graph` | 2430–2525 | Enter mode from cache or start resolve; prefetch on every focus change; poll worker; exit → refocus centre. | Replaced by `enter_call_graph_preset`, `prefetch_calls`, `relations.poll()`, `leave_call_graph_preset` (§5.5). |
| same | `start_loading` clears `call_graph`, `call_graph_cache`, `cg_resolver` | 2802–2804 | | Replaced by rebuilding `RelationRegistry` at load (00-framework §3.5). |
| same | `cg_source_lines`, `render_call_graph`, `render_cg_column` | 3191–3554 | Builds the two GPUI column `div`s with cards + code preview. | Deleted; `view/panel_view.rs` renders `ResolvedPanel`s (08-panel.md). |
| same | `render()`: `poll_call_graph`, `cg_animating`, `cg_resolver.is_active()` → `request_animation_frame` | 4071, 4104–4117 | | `self.relations.poll()` and `self.relations.is_busy()`; panel animation flag from 08. |
| same | `on_left_release` / `on_mouse_move` / `on_scroll` guards `self.call_graph.is_some()` | 1991, 2030, 2063 | Mode blocks pan/zoom/hover/click. | Guard becomes `self.panel_captures_input()` (08-panel.md), so behaviour is preserved while a docked panel is open. |
| `crates/outrider/src/world.rs` | `DrawItem { node, px, label_w, level, draw, left, top, full_h }`, `visible_nodes` (pre-order) | 141–269 | On-screen culled nodes with LOD (`Draw::Container(Rung)` / `Draw::Leaf(LeafDraw)`); `left/top/full_h` are unclipped. | Read-only input to aggregation. |
| `crates/outrider/src/camera.rs` | `Camera::world_to_screen(wx, wy, vw, vh)` | 22–27 | | Used to project off-screen endpoints. |
| `crates/outrider/src/theme.rs` | palette consts | 9–40, 281 | | Adds `EDGE_*` colours + `edge_color(key)` (§5.3). |
| `crates/outrider/src/paint_model.rs` | `PaintItem` | 41–62 | | No new fields for edges (edges are a separate frame product, §5.1). |
| vendored gpui `029bf2f` | `Window::paint_path(Path<Pixels>, impl Into<Background>)`, `PathBuilder::{stroke(px), fill(), dash_array(&[px]), move_to, line_to, curve_to(to, ctrl), close, build() -> Result<Path<Pixels>,_>}` | `window.rs:3770`, `path_builder.rs:88–250` | Lyon-backed stroked/filled paths, dash arrays supported. | Used by `edge_pass` (§5.3). No quad-polyline fallback needed. |

---

## 3. Spec types (`crates/outrider-view/src/spec.rs`)

```rust
/// One `{ "edges": { ... } }` layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgesSpec {
    /// Exactly one of `relation` / `pairs` must be set (validated; not a serde enum because
    /// `#[serde(flatten)]` is incompatible with `deny_unknown_fields`).
    #[serde(default, skip_serializing_if = "Option::is_none")] pub relation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub pairs: Option<Vec<EdgePair>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub within: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub incident_to: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub cross_boundary: Option<Boundary>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub direction: Option<Direction>,
    #[serde(default)] pub min_weight: f64,           // 0.0
    #[serde(default)] pub style: EdgeStyle,          // Solid
    /// Theme colour key; defaults to the relation id ("calls", "imports", "cochange") or "pairs".
    #[serde(default, skip_serializing_if = "Option::is_none")] pub color: Option<String>,
}
impl Default for EdgesSpec { /* relation: Some("calls"), everything else default */ }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgePair {
    pub from: WireSymbolId,
    pub to: WireSymbolId,
    #[serde(default = "one_f64")] pub weight: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub basis: Option<String>,
}

/// `"folder"` | `{ "partition": "layers" }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Boundary { Folder, Partition(String) }

/// Ranked against a partition's declared group order (01-space.md `regroup`); see §4.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Direction { Up, Down, #[default] Any }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum EdgeStyle { #[default] Solid, Dashed }

impl EdgesSpec {
    /// Ok(Relation(id)) | Ok(Pairs(&[..])) | Err(rule) when neither or both are set.
    pub fn source(&self) -> Result<EdgeSource<'_>, &'static str>;
}
pub enum EdgeSource<'a> { Relation(&'a str), Pairs(&'a [EdgePair]) }
```

JSON examples (all valid `LayerSpec::Edges`):

```jsonc
{ "edges": { "relation": "calls", "incidentTo": "focusSet" } }
{ "edges": { "relation": "calls", "within": "affected", "minWeight": 2, "style": "dashed" } }
{ "edges": { "relation": "calls", "crossBoundary": "folder" } }
{ "edges": { "relation": "imports", "crossBoundary": { "partition": "layers" }, "direction": "up",
             "color": "violation" } }
{ "edges": { "pairs": [ { "from": "fn:src/a.rs::alloc", "to": "fn:src/b.rs::grow", "weight": 4096,
                          "basis": "heaptrack run 3" } ] } }
```

`validate` rules added (00-framework §2.6): `relation` and `pairs` both set or both absent → hard (`"edges.source"`); unknown relation id → hard (already listed there); `direction: up|down` without `crossBoundary: {partition}` → hard (`"edges.direction"`, message "direction needs a partition order"); `minWeight < 0` → hard; unknown `color` key → soft warning (falls back to relation colour at paint time).

---

## 4. Resolution (`outrider-view` side)

### 4.1 `relation.rs` — provider trait and registry

```rust
use outrider_index::{SymbolId, SymbolTree, TreeIndex};

/// What providers get to see. If 03-fill.md has already defined `ProviderCtx` in `metric.rs`,
/// use that one — its fields must be a superset of these; otherwise define it here.
pub struct ProviderCtx<'a> { pub tree: &'a SymbolTree, pub index: &'a TreeIndex<'a>, pub repo_root: &'a Path }

#[derive(Debug, Clone, PartialEq)]
pub enum Lookup<T> { Ready(T), Pending }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDir { Out, In }

/// Extra, provider-specific facts about one edge (the call-graph panel needs `raw_name`
/// and `call_site`; other providers return None).
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeDetail { pub label: String, pub site: Option<Range<usize>> /* in `from`'s file */ }

/// Parent §7.3 verbatim (id / out_edges / in_edges / deps) plus three additive methods.
pub trait RelationProvider: Send + Sync {
    fn id(&self) -> &str;                                            // "calls" | "imports" | "cochange" | imported
    fn out_edges(&self, from: &SymbolId, ctx: &ProviderCtx) -> Vec<(SymbolId, f64 /*weight*/)>;
    fn in_edges(&self, to: &SymbolId, ctx: &ProviderCtx) -> Vec<(SymbolId, f64)>;
    fn deps(&self) -> Deps;
    // ---- additive (see §11) ----
    /// Non-blocking form. Default: `Ready(self.out_edges/in_edges(..))`.
    fn lookup(&self, sym: &SymbolId, dir: EdgeDir, ctx: &ProviderCtx) -> Lookup<Vec<(SymbolId, f64)>>;
    /// Default None.
    fn edge_detail(&self, from: &SymbolId, to: &SymbolId, ctx: &ProviderCtx) -> Option<EdgeDetail>;
    /// Enqueue work for `sym` without needing the answer now. Default no-op.
    fn prefetch(&self, sym: &SymbolId, ctx: &ProviderCtx);
    /// Drain finished background work into the cache. Returns true if any lookup that was
    /// `Pending` is now `Ready`. Default false. Called once per frame by the app.
    fn poll(&self) -> bool;
    /// True while background work is queued or running (app keeps animating). Default false.
    fn is_busy(&self) -> bool;
}
```

`out_edges`/`in_edges` for an async provider are `lookup(..)` with `Pending → vec![]` — that keeps parent §7.3 callers (02-set.md `reach`) compiling, but they must treat `is_busy()` as "my answer may be incomplete" (see §10).

```rust
pub struct RelationRegistry { providers: Vec<Box<dyn RelationProvider>> }
impl RelationRegistry {
    /// Built at load (00-framework §3.5). `tree` is shared with the calls worker thread.
    pub fn builtin(tree: Arc<SymbolTree>) -> Self;          // registers CallsProvider only (imports/cochange when they exist)
    pub fn register(&mut self, p: Box<dyn RelationProvider>);
    pub fn get(&self, id: &str) -> Option<&dyn RelationProvider>;
    pub fn ids(&self) -> impl Iterator<Item = &str>;
    pub fn poll(&self) -> bool { self.providers.iter().any(|p| p.poll()) }   // OR, not short-circuit
    pub fn is_busy(&self) -> bool;
    pub fn prefetch(&self, id: &str, sym: &SymbolId, ctx: &ProviderCtx);
}
```

`validate`'s `known_relations` closure is `|id| relations.get(id).is_some()`.

### 4.2 `relation/calls.rs` — the built-in `calls` provider

Cache semantics moved from `TreemapView.call_graph_cache` (`HashMap<SymbolId, CallGraphData>`): one entry per **centre** symbol holding both directions, filled only by `resolve_calls(centre)`. Because `resolve_calls` self-excludes and dedupes by target, weight is always `1.0` per `(from, to)`.

```rust
pub struct CallsProvider {
    cache: Mutex<HashMap<SymbolId, CallGraphData>>,     // interior mutability: trait methods take &self
    queued: Mutex<HashSet<SymbolId>>,                    // requested, not yet in cache (dedup)
    worker: CallsWorker,
}
impl RelationProvider for CallsProvider {
    fn id(&self) -> &str { "calls" }
    fn deps(&self) -> Deps { Deps::TREE.union(Deps::RELATIONS) }
    fn lookup(&self, sym, dir, ctx) -> Lookup<Vec<(SymbolId, f64)>> {
        if let Some(d) = self.cache.lock().get(sym) {
            let v = match dir { EdgeDir::Out => &d.callees, EdgeDir::In => &d.callers };
            return Lookup::Ready(v.iter().map(|e| (e.target.clone(), 1.0)).collect());
        }
        if !is_fn_leaf(sym, ctx) { return Lookup::Ready(vec![]) }   // same guard as enter_call_graph L2433
        self.request(sym);                                           // no-op if already queued
        Lookup::Pending
    }
    fn edge_detail(&self, from, to, _ctx) -> Option<EdgeDetail> {
        // Callee edge: from = centre. label = raw_name, site = call_site.
        // Caller edge: to = centre; look up cache[to].callers where target == from; label = raw_name, site None.
    }
    fn prefetch(&self, sym, ctx) { if is_fn_leaf(sym, ctx) && !cached { self.request(sym) } }
    fn poll(&self) -> bool { /* drain worker results into cache; remove from queued; return !drained.is_empty() */ }
    fn is_busy(&self) -> bool { !self.queued.lock().is_empty() }
}
```

`CallGraphData` is a `Clone` type from `outrider_index::call_graph`; the mini_repo golden test in §9 asserts the provider returns exactly what `resolve_calls` returns.

### 4.3 `CallsWorker` — the moved `CallGraphResolver`

Today: one detached thread per request, each with a **full `SymbolTree` clone** (`treemap.rs:772–786`), and one inflight slot; a second request while one is running is dropped in `enter_call_graph` (L2458) or supersedes via generation (`request` bumps `generation`, `poll` discards stale results). New: one persistent worker thread, FIFO queue, no clones.

```rust
pub(crate) struct CallsWorker {
    tx: mpsc::Sender<SymbolId>,                                   // requests
    results: Arc<Mutex<Vec<(SymbolId, CallGraphData)>>>,          // finished, drained by poll()
    generation: Arc<AtomicU64>,                                   // bumped by cancel(); worker tags results
}
impl CallsWorker {
    pub fn spawn(tree: Arc<SymbolTree>) -> Self;   // thread loop: for id in rx { let d = resolve_calls(&id, &tree); results.push((id, d)) }
    /// Test-only: resolves synchronously on the calling thread (no `Pending` ever observed).
    #[cfg(any(test, feature = "inline-worker"))] pub fn inline(tree: Arc<SymbolTree>) -> Self;
    pub fn request(&self, id: SymbolId);
    pub fn drain(&self) -> Vec<(SymbolId, CallGraphData)>;
    pub fn cancel(&self);      // bumps generation; results tagged with an older generation are dropped in drain()
}
```

The thread exits when the `Sender` drops (registry rebuilt at load, or app exit). Requests already queued for a dropped registry are simply never read. Priority: `request` for a symbol that is *the current focus* should jump the queue — implement by keeping the queue as `Mutex<VecDeque>` + `Condvar` instead of `mpsc`, and `request_front(id)` used by `prefetch`; plain `request` pushes back. (One thread is deliberate: `resolve_calls` is I/O + parse heavy and today's UX is one resolve at a time; raise to 2 threads only if measured.)

**Async contract, end to end (the frame never blocks):**
1. Layer resolution calls `provider.lookup(sym, dir, ctx)`; a miss enqueues and returns `Pending`. The layer records `pending += 1` and includes no edges for that symbol.
2. `TreemapView::render` calls `self.relations.poll()` every frame (replaces `poll_call_graph`, treemap.rs:4071). If it returns true → `self.view_dirty |= Deps::RELATIONS; cx.notify()`.
3. `ViewResolver::resolve(.., dirty)` recomputes every layer whose `deps` contain `RELATIONS` (edge layers over async providers, `reach` sets, `EdgeGroups` panels). Sets/layers over static providers are untouched.
4. While `self.relations.is_busy()` the app calls `window.request_animation_frame()` (replaces `self.cg_resolver.is_active()` at treemap.rs:4112).
5. `Deps::RELATIONS` is a new bit (§11).

### 4.4 `layers/edges.rs` — resolving one layer

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Edge { pub from: SymbolId, pub to: SymbolId, pub weight: f64, pub basis: Option<String> }

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEdgeLayer {
    pub layer_index: usize,        // position in spec.layers (for warnings / "showing N of M" attribution)
    pub relation: String,          // provider id, or "pairs"
    pub edges: Vec<Edge>,          // symbol level, deduped by (from,to)
    pub directed: bool,            // true for calls/imports/pairs; false for cochange
    pub style: EdgeStyle,
    pub color: String,             // theme key (spec.color or relation id)
    pub pending: usize,            // symbols whose lookup returned Pending this resolve
    pub deps: Deps,
}

pub fn resolve_edges(spec: &EdgesSpec, layer_index: usize, sets: &BTreeMap<String, ResolvedSet>,
                     ctx: &ResolveCtx, warnings: &mut Vec<String>) -> ResolvedEdgeLayer;
```

Algorithm:

1. **Source set.** `within` → its `ids`; else `incident_to` → its `ids`; else *all* `fn`-labelled leaf items from `ctx.index.iter()` (unscoped; see cost note). Deps: `SPEC ∪ within.deps ∪ incident_to.deps`.
2. **Enumerate.**
   - `Relation(id)`: `p = ctx.relations.get(id)`. For each `s` in source: `lookup(s, Out)` → edges `s→t`; if `incident_to` is set (or unscoped) also `lookup(s, In)` → `u→s`. `Pending` → `pending += 1`. Dedup by `(from,to)`, keeping `max(weight)` (the same edge can arrive from `s`'s out list and `t`'s in list; never sum here — summing happens only in render-time aggregation). Deps `∪= p.deps()`. `directed = id != "cochange"`.
   - `Pairs(v)`: each `from`/`to` parsed with `symbol_id::parse_wire`, then looked up in `ctx.index` (bare-path form allowed, 02-set.md); a missing endpoint → warning `"edges[i]: pair 3: unknown symbol …"` and the pair is skipped. Weight/basis copied. `directed = true`. Deps `SPEC`.
3. **Filters**, in this order: `within` (both endpoints ∈ set) → `incident_to` (≥ 1 endpoint ∈ set; both filters may be present, ANDed) → `cross_boundary` → `direction` → `weight >= min_weight`.
   - `Boundary::Folder`: `folder_of(id)` = nearest ancestor with `SymbolKind::Folder` via `ctx.index.parent` chain; keep iff `folder_of(from) != folder_of(to)`.
   - `Boundary::Partition(name)`: `group = ctx.partitions.group(name, id)` (01-space.md regroup registry). Keep iff groups differ. If `ctx` has no partition registry yet → warning `"edges[i]: partition 'name' unavailable; crossBoundary ignored"` and the filter is a no-op.
   - `Direction`: `rank = ctx.partitions.rank(name, group)`; layers are ordered top = 0. `Down` keeps `rank(from) < rank(to)`; `Up` keeps `rank(from) > rank(to)`; `Any` keeps all. Same fallback as above.
4. **Output** with `color = spec.color.clone().unwrap_or(relation)`. If `pending > 0` push warning `"edges[i]: resolving {pending} symbols"` (this string is what the on-screen count in §5.4 shows).

**Cost note.** Unscoped `calls` enqueues every fn in the repo; each `resolve_calls` parses the centre file plus every file containing the name. This is the price of parent §4's "boundary-edge view" and it runs entirely on the worker; the layer fills in progressively and the count says so. A whole-repo single-pass resolver in `outrider-index` (parse each file once, emit all edges) is the follow-up that makes this fast; it plugs in behind the same `CallsProvider` (a `warm_all()` method) without touching this layer.

### 4.5 `ResolvedView.edges: Vec<ResolvedEdgeLayer>` and caching

`ViewResolver` recomputes an edge layer when: `dirty ∩ layer.deps ≠ ∅` (so `RELATIONS` results, `FOCUS` when its set is focus-live, `SPEC`) — per 00-framework §2.4. Aggregation is *not* here: it depends on `CAMERA` and this frame's `DrawItem`s, so it lives in the app (§5.2) and runs every frame; that is cheap (O(edges) hash lookups) and keeps `ResolvedView` camera-independent.

### 4.6 `imports` and `cochange` — interfaces only

```rust
/// relation/imports.rs — file→file (and item→item for `use`/`import` lines) edges. Needs a new
/// `outrider_index::imports::resolve_imports(file: &SymbolId, tree) -> Vec<(SymbolId, Range<usize>)>`
/// (tree-sitter `use_declaration` / `import_statement` / `#include`). Same CallsWorker shape
/// (generic `RelationWorker<T>` once a second async provider exists). deps: TREE | RELATIONS.
pub struct ImportsProvider { /* cache: Mutex<HashMap<SymbolId /*file*/, Vec<SymbolId>>>, worker */ }

/// relation/cochange.rs — undirected file↔file edges weighted by the number of commits touching
/// both. Requires commit→files sets, which churn.rs does NOT retain (it reduces the numstat log to
/// per-path counts, `commit_counts_from_log`). Add `churn::commit_file_sets(repo_root) ->
/// Vec<Vec<String>>` (parse the same `git log --numstat --format=%H` output, split at hash lines,
/// cached next to `churn-cache.json` keyed by HEAD) and build the pair counts lazily on first
/// lookup on the worker (O(Σ files-per-commit²), cap files-per-commit at 50 to skip mass renames).
/// deps: GIT | RELATIONS. Item-level lookups map to their file. Weight = shared commit count.
pub struct CochangeProvider { /* pairs: OnceLock<HashMap<(String,String), u64>>, worker */ }
```

Both register through `RelationRegistry::register`; nothing in the layer or edge pass is provider-specific.

---

## 5. App integration (`crates/outrider/src/view/edge_pass.rs`, treemap.rs, theme.rs)

### 5.1 Frame products

`paint_items` today returns `(Vec<PaintItem>, Option<DocPanel>, bool /*cg_scrim*/)`. Change it to return a struct (also consumed by 06/07):

```rust
pub(crate) struct PaintFrame {
    pub items: Vec<PaintItem>,
    pub doc_panel: Option<DocPanel>,
    pub edges: EdgeFrame,          // this spec
    // 06-marks.md adds glyph_hits; 07-notes.md adds notes. `cg_scrim` is gone.
}
```

### 5.2 `edge_pass::aggregate` — render-time aggregation

```rust
pub(crate) struct EdgePaint {
    pub x0: f32, pub y0: f32, pub cx: f32, pub cy: f32, pub x1: f32, pub y1: f32,  // screen px, quadratic
    pub width: f32, pub color: u32 /*0xRRGGBBAA*/, pub dashed: bool, pub arrow: bool,
    pub weight: f64, pub merged: u32,                                                // for a future hover readout
}
pub(crate) struct EdgeFrame {
    pub edges: Vec<EdgePaint>,
    pub shown: usize, pub total: usize,     // after aggregation, before/after MAX_EDGES cap
    pub collapsed: usize,                   // edges whose lifted endpoints coincide (drawn as nothing)
    pub pending: usize,                     // Σ layer.pending
}
pub(crate) const MAX_EDGES: usize = 2000;

pub(crate) fn aggregate(layers: &[ResolvedEdgeLayer], items: &[DrawItem], index: &TreeIndex,
                        layout: &PackLayout, camera: &Camera, vw: f64, vh: f64) -> EdgeFrame;
```

Called inside `paint_items` right after `visible_nodes` (treemap.rs:1483) while `items: Vec<DrawItem>` is alive; the result is stored in `PaintFrame.edges`. Steps:

1. **Anchor table.** `anchor: HashMap<&SymbolId, usize>` over `items` where the item is Label-or-better: `Draw::Container(r) if r != Rung::Dot` or `Draw::Leaf(t) if t != LeafDraw::Dot`. (`Rung::Label`/`LeafDraw::Label` count — parent §2.5 "nearest visible ancestor (Label rung or better)".)
2. **Endpoint lift.** For a symbol `id`:
   - if `anchor[id]` → `Endpoint::Item(idx)`;
   - else project `layout.rects[id]` with `camera.world_to_screen`; if the projected rect misses the viewport entirely → `Endpoint::Off(rect_px)` (no lifting: the symbol is genuinely elsewhere);
   - else walk `index.parent(id)` upward until an anchored ancestor → `Endpoint::Item(idx)`; none → `Endpoint::Hidden` (drop edge, count in `collapsed`).
   The rule "lift only when the child is on-screen but too small" is what makes bundles honest: an off-screen callee is drawn as a curve leaving the viewport, not as an edge to its (visible) file.
3. **Merge.** Key `(lifted_from_id, lifted_to_id, layer_index)`; sum `weight`, count `merged`. Drop keys with `from == to` after lifting (two functions in one file at zoom-out) into `collapsed`. Drop `Off/Off` pairs (both off-screen).
4. **Cap.** Sort by `weight` desc, truncate to `MAX_EDGES`; `total` = count before truncation, `shown` = after. Never silent: §5.4 shows the count whenever `shown < total || pending > 0`.
5. **Geometry** (per merged edge, using the unclipped `left/top/label_w/full_h` of the anchored `DrawItem`, or the projected rect for `Off`):
   ```
   from_cx = left + label_w/2;  to_cx likewise
   if to_cx >= from_cx { (x0,y0) = (from.right, from.mid_y); (x1,y1) = (to.left,  to.mid_y) }   // deeper = right (parent §8.5)
   else                { (x0,y0) = (from.left,  from.mid_y); (x1,y1) = (to.right, to.mid_y) }   // target is left: face it
   bulge = clamp(0.2 * |x1-x0|, 8, 160);  (cx,cy) = ((x0+x1)/2, min(y0,y1) - bulge)             // arcs bow upward
   width = clamp(1 + log2(max(weight,1)), 1, 6)
   arrow = layer.directed
   ```
   `color = theme::edge_color(&layer.color)` (0xRRGGBB) with alpha `0xd9`; dashed from `layer.style`.

### 5.3 `edge_pass::paint` — GPUI primitives

Runs in the canvas closure (`treemap.rs` ~4453 ff.) **after pass 2b (headers) and before pass 2c (neighbor rings)** — i.e. exactly where the `cg_scrim` quad is painted today (L4499), which this replaces. Edges are above box surfaces and text so they are legible over code, below rings and the deferred focused leaf so the focus stays on top.

```rust
pub(crate) fn paint(frame: &EdgeFrame, origin: Point<Pixels>, window: &mut Window) {
    // Batch: one stroked path per (color, width, dashed) — lyon paths hold many subpaths
    // (each move_to starts a contour), so 2000 edges is a handful of paint_path calls.
    let mut buckets: HashMap<(u32, u32 /*width*1000 as int*/, bool), PathBuilder> = ..;
    for e in &frame.edges {
        let pb = buckets.entry(key(e)).or_insert_with(|| {
            let b = PathBuilder::stroke(px(e.width));
            if e.dashed { b.dash_array(&[px(6.0), px(4.0)]) } else { b }
        });
        pb.move_to(origin + point(px(e.x0), px(e.y0)));
        pb.curve_to(origin + point(px(e.x1), px(e.y1)), origin + point(px(e.cx), px(e.cy)));  // (to, ctrl)
    }
    for ((color, _, _), pb) in buckets { if let Ok(path) = pb.build() { window.paint_path(path, rgba(color)); } }
    // Arrowheads: filled triangles, one PathBuilder::fill() per colour.
    for e in frame.edges.iter().filter(|e| e.arrow) {
        // tangent at t=1 of a quadratic is (x1-cx, y1-cy); size = 4 + 1.5*width; half-angle 25°.
        let (tx, ty) = normalize(e.x1 - e.cx, e.y1 - e.cy);
        let (px_, py_) = (-ty, tx);
        let s = 4.0 + 1.5 * e.width;
        let tip = (e.x1, e.y1); let base = (e.x1 - tx * s, e.y1 - ty * s);
        fill.move_to(tip); fill.line_to(base + perp*s*0.47); fill.line_to(base - perp*s*0.47); fill.close();
    }
}
```

`PathBuilder`, `paint_path` and `dash_array` exist at the pinned rev (`path_builder.rs:88–120`, `window.rs:3770`); no quad-polyline fallback is required. If profiling shows lyon tessellation dominates at MAX_EDGES, the fallback is a per-frame cache of the built `Path<Pixels>` per bucket keyed on the camera (paths only change when the camera or the resolved layer changes) — note in §10.

`theme.rs` additions:
```rust
pub const EDGE_CALLS: u32 = 0x4da6ff;      // same hue family as FOCUS_BORDER
pub const EDGE_IMPORTS: u32 = 0x8fd18f;
pub const EDGE_COCHANGE: u32 = 0xd9a441;
pub const EDGE_PAIRS: u32 = 0xc08ee0;      // imported/agent-supplied pairs (neutral, distinct)
pub const EDGE_VIOLATION: u32 = 0xe06060;
pub fn edge_color(key: &str) -> u32 { match key { "calls" => EDGE_CALLS, "imports" => EDGE_IMPORTS, "cochange" => EDGE_COCHANGE, "violation" => EDGE_VIOLATION, _ => EDGE_PAIRS } }
```

### 5.4 The count readout ("no silent caps")

`TreemapView` keeps `edge_status: Option<(usize /*shown*/, usize /*total*/, usize /*pending*/)>` written from `PaintFrame.edges` in `render`. When `shown < total || pending > 0`, `render` adds a small `div` overlay (bottom-left, `theme::TEXT_SECONDARY`, 11 px sans, like the toolbar toggle at L4136) reading `"showing 2,000 of 5,412 edges"` and/or `" · resolving 37 symbols"`. Not a Note (it isn't attached to a symbol); not painted in the canvas so it needs no text layout code.

### 5.5 Call-graph mode → preset

`view/presets.rs`:
```rust
/// Layers pushed by Tab on a fn leaf. All sets are focus-live so re-centring is just `Focus`.
pub(crate) fn call_graph_layers() -> Vec<LayerSpec>;   // in this order:
```
```jsonc
// sets (defined once in default_view, 00-framework §3.3, alongside focusSet/neighbors):
"cgCallers": { "reach": { "from": { "ref": "focusSet" }, "relation": "calls", "direction": "in",  "depth": 1 } },
"cgCallees": { "reach": { "from": { "ref": "focusSet" }, "relation": "calls", "direction": "out", "depth": 1 } },
"cgHood":    { "union": [ { "ref": "focusSet" }, { "ref": "cgCallers" }, { "ref": "cgCallees" } ] },
"cgSite":    { "ids": [ "$selectionSite" ] }          // pseudo-id → the selected panel row's call site range (§11)
// layers pushed as a group:
{ "mask":  { "dimExcept": "cgHood", "strength": 0.8 } },                                   // 04-mask.md; replaces cg_scrim (0xcc alpha ≈ 0.8)
{ "edges": { "relation": "calls", "incidentTo": "focusSet" } },                             // new: real curves to callers/callees
{ "panel": { "rows": { "edgeGroups": { "of": "focusSet", "relation": "calls", "direction": "in"  } }, "dock": "left",  "title": "Callers" } },
{ "panel": { "rows": { "edgeGroups": { "of": "focusSet", "relation": "calls", "direction": "out" } }, "dock": "right", "title": "Callees" } },
{ "marks": { "on": "cgSite", "kind": "selection" } }                                        // 06-marks.md range mark = today's cg_highlight_lines
```

`TreemapView` state: `cg_preset: Option<Range<usize>>` — the layer indices pushed, so leaving removes exactly those (`RemoveLayer` from the top down). Guard: same `is_fn` predicate as `enter_call_graph` L2432–2439.

| Key (today, `on_call_graph_key`) | Today | After |
|---|---|---|
| Tab (map) | `enter_call_graph` | `enter_call_graph_preset`: `apply_view_command(PushLayer(..))` × 5; `relations.prefetch("calls", focus)`. |
| Tab / Esc (in mode) | `exit_call_graph`: refocus centre, frame | `leave_call_graph_preset`: `RemoveLayer` × 5, then the same `frame_focus` tween. Esc reaches here only if no panel consumed it (08: Esc closes the topmost panel; the preset's panels forward Esc to `leave_call_graph_preset` since they are preset-owned). |
| Up / Down | move row in the active column (`CgScrollState` animation) | 08-panel.md: topmost focused panel moves its row; sets `SELECTION` + `selection_range` = the row edge's `edge_detail().site`. |
| Left / Right | switch column, or cycle `CgEdgeGroup.active` | 08-panel.md: Left/Right move focus between the two docked panels; within a group row they cycle `active` (row exposes `group: (active,total)`, same "1/3" label). |
| Enter | refocus to the row target, keep mode | 08: `ViewCommand::Camera(Focus(row.id))` + `nav_history.push`; the preset layers stay (sets are focus-live, so callers/callees/edges re-resolve — `FOCUS` dirty). Also `prefetch("calls", new focus)`. |
| any other | ignored | as today. |

`maybe_precompute_call_graph` (called at L2024, 2229, 2282, 2290, 2320, 2330, 2348, 2517) → `fn prefetch_calls(&mut self) { self.relations.prefetch("calls", &self.focus.current, &ctx) }` at the same call sites; the `is_fn` check moves into `CallsProvider::prefetch`. This keeps today's "Tab is instant after you land on a fn" property.

`poll_call_graph` (L4071) → `if self.relations.poll() { self.view_dirty |= Deps::RELATIONS; needs_notify = true; }`.

Rendering differences after migration, all intentional: (a) callers/callees are also drawn as curves to their real boxes; (b) the dim is per-item via mask, so hood members keep full brightness including their headers (today only the focused leaf escaped the scrim); (c) neighbor rings of dimmed nodes paint at `light` alpha (06-marks.md §5.3) instead of being skipped outright.

---

## 6. Commands

- `ViewCommand::PushLayer(LayerSpec::Edges(..))` / `PopLayer` / `RemoveLayer(i)` — the only mutating path (`edges` layers are ordinary layers; they accumulate, parent §3.1).
- CLI `outrider edges <relation> [--within <set>] [--incident-to <set>] [--cross-boundary folder|partition:<name>] [--direction up|down] [--min-weight n] [--dashed] [--color key]` (parent §6.2) → `ViewPatch { layers: [Edges(..)] }`. `outrider edges --pairs <file.json>` for explicit pairs (a JSON array of `EdgePair`).
- `outrider query callers|callees <symbol> [--depth n]` (parent §6.2) → RPC `query.callers` implemented over `RelationRegistry` (offline mode calls `resolve_calls` directly).
- Keyboard: Tab / Esc as §5.5.

---

## 7. Invalidation

| bit | set by | effect on this component |
|---|---|---|
| `SPEC` | `apply_view_command` | edge layer re-resolved |
| `FOCUS` | focus changes | edge layers whose `within`/`incident_to` set is focus-live (`focusSet`, `cgHood`) |
| `RELATIONS` (new) | `TreemapView::render` after `relations.poll()` returns true | every layer/set whose deps include `RELATIONS` (edge layers over `calls`, `reach` sets, `EdgeGroups` panels) |
| `TREE` | loader / packing (00 §3.4) | `RelationRegistry` rebuilt (cache dropped), everything re-resolved |
| `GIT` | git watcher (02-set) | `cochange` layers only |
| `CAMERA` | pan/zoom/tween | not consumed by the resolver; aggregation runs unconditionally each frame in `paint_items` |

---

## 8. Migration steps (behaviour-preserving; each step builds and passes tests)

1. **relation.rs skeleton.** Add `Deps::RELATIONS`; `ProviderCtx` (or reuse 03's), `Lookup`, `EdgeDir`, `EdgeDetail`, `RelationProvider` (with default impls), `RelationRegistry`. `RelationRegistry::builtin(tree)` returns an empty registry for now. Unit tests for defaults.
2. **Move the worker.** Copy `CallGraphResolver`/`InflightResolve` (treemap.rs 753–816) into `outrider-view/src/relation/calls.rs` as `CallsWorker` (persistent thread, `Arc<SymbolTree>`, queue, `inline()` for tests). Implement `CallsProvider`. `builtin(tree)` registers it. Provider tests (§9) green. `treemap.rs` untouched.
3. **App plumbing.** In the loader-completion path build `self.relations = RelationRegistry::builtin(Arc::new(self.tree.clone()))` (one clone per load instead of one per request; later make `TreemapView.tree` an `Arc<SymbolTree>` and drop the clone — separate cleanup). Add `relations.poll()`/`is_busy()` to `render` alongside the existing `poll_call_graph`/`cg_resolver` (both coexist for one step). Add `prefetch_calls` next to `maybe_precompute_call_graph` at every call site.
4. **EdgesSpec + layers/edges.rs.** Spec types, validation rules, `resolve_edges`, wire into `ViewResolver` (`ResolvedView.edges`). Tests: crossBoundary, filters, pairs, pending.
5. **edge_pass.rs.** `PaintFrame` struct replacing the tuple; `aggregate` after `visible_nodes`; `paint` in the closure after pass 2b; `edge_status` overlay; theme colours. Manual check with `outrider edges calls --incident-to focusSet` (or a `.outrider/views` file) — curves appear, count appears when capped. Old call-graph mode still works untouched (its scrim paints after edges; fine for one step).
6. **Preset + panels + mark.** Requires 04-mask.md, 06-marks.md `selection` range kind, 08-panel.md `EdgeGroups`. Add `presets.rs`, `enter_/leave_call_graph_preset`, key routing (§5.5), `cgCallers/cgCallees/cgHood/cgSite` sets in `default_view`. Behind a temporary `const NEW_CALL_GRAPH: bool` so both paths can be A/B'd for one commit; golden test §9.
7. **Delete.** Remove: fields `call_graph`, `call_graph_cache`, `cg_resolver` (L556–559); types/fns L635–751 (`CallGraphMode` … `cg_parent_name`), L753–816 (resolver), `call_graph_column_lefts` (924, moved), `cg_highlight_lines` block (1458–1478) and its `hl` plumbing (1596–1611, now `ov.range_marks`), `cg_scrim` (1805; closure 4499–4508, 4532, 4560 → unconditional), `enter_call_graph`/`maybe_precompute_call_graph`/`poll_call_graph`/`exit_call_graph` (2430–2525), `on_call_graph_key` (2617–2784) and its dispatch (2146–2149), `start_loading` lines 2802–2804, `cg_source_lines`/`render_call_graph`/`render_cg_column` (3191–3554), `call_graph_overlay` (4155, 4612), `cg_animating`/`cg_resolver.is_active()` (4104–4113), the three `self.call_graph.is_some()` input guards (→ `panel_captures_input()`), and the `use outrider_index::call_graph::{CallEdge, CallGraphData}` import if now unused. Remove `NEW_CALL_GRAPH`.
8. **Docs.** Update 00-framework §1 table row for `CallGraphMode` and parent §4 row status.

---

## 9. Tests

**`outrider-view` unit tests** (`crates/outrider-view/tests/edges.rs`, fixture `outrider-index/tests/fixtures/mini_repo` via `index_repo`, packed with `PackConfig` copied from `world::pack_config`; `CallsWorker::inline` so lookups are never `Pending`):

- `provider_matches_resolve_calls_for_free` — `fn:src/lib.rs::free`: `lookup(Out)` == `resolve_calls(free).callees` targets (= `[fn:src/lib.rs::Point::new]`), `lookup(In)` == its callers (empty). And `fn:src/lib.rs::Point::new` `lookup(In)` == `[free]`.
- `provider_caches` — with a counting worker (`CallsWorker::counting()` test double): two `lookup`s for the same symbol → one `resolve_calls`; `prefetch` then `lookup` → one; non-fn symbol → `Ready([])` and zero requests.
- `provider_pending_then_ready` — with the real threaded worker: first `lookup` is `Pending`, `is_busy()` true; loop `poll()` until true; second `lookup` is `Ready` with the same edges as the inline worker.
- `edge_detail_carries_call_site` — `edge_detail(free, Point::new).site` == the `call_site` from `resolve_calls`.
- `cross_boundary_folder_filter` — hand-built tree: `a/x.rs::f → a/y.rs::g` (same folder) and `a/x.rs::f → b/z.rs::h`; pairs layer with `crossBoundary: folder` keeps only the second.
- `within_and_incident_to` — pairs `1→2, 2→3, 3→4`, set `{2,3}`: `within` → `{2→3}`; `incidentTo` → `{1→2, 2→3, 3→4}`; both → `{2→3}`.
- `dedup_keeps_max_not_sum` — `incidentTo {a,b}` where a's out and b's in both report `a→b`: one edge, weight 1.
- `min_weight_and_direction_validation` — `minWeight: 2` drops weight-1 pairs; `direction: up` without partition → `Violation{path:"layers[0].edges.direction"}`.
- `source_xor` — `{relation, pairs}` both / neither → violation `"edges.source"`.
- `edges_spec_json_roundtrip` — the five JSON examples in §3.
- `pending_reported` — a `RelationProvider` test double returning `Pending` for one of two symbols → `pending == 1`, warning text `"resolving 1 symbols"`.

**App-side tests** (`crates/outrider/src/view/edge_pass.rs` `#[cfg(test)]`, pure functions over hand-built `DrawItem`s — `DrawItem` needs a `&SymbolNode`; build a two-file tree in the test):

- `aggregation_sums_and_lifts` — items: folder F (Card), file A (Label), file B (Label); leaves a1,a2 ∈ A and b1 ∈ B not in items (below merge). Edges a1→b1 (2), a2→b1 (3) → one `EdgePaint` A→B with `weight 5, merged 2`. a1→a2 → `collapsed == 1`, not drawn.
- `off_screen_endpoint_not_lifted` — b1's rect projects outside the viewport, B is not in items: edge a1→b1 draws from A to the projected off-screen point (`Endpoint::Off`), not to F.
- `both_off_screen_culled`.
- `cap_reports_dropped` — 2,500 distinct edges → `shown == 2000`, `total == 2500`, heaviest kept.
- `geometry_faces_target` — from right of to → uses `from.left`/`to.right`.

**Migration golden** (`crates/outrider/src/view/presets.rs` `#[cfg(test)]`, mini_repo, inline worker): resolve `default_view() + call_graph_layers()` with focus `fn:src/lib.rs::free`; assert `panels[1].rows` (Callees) ids == `group_edges(resolve_calls(free).callees)` active targets in order (`[Point::new]`), `panels[0].rows` (Callers) empty; `mask.light(free) == 1.0`, `light(Point::new) == 1.0`, `light(fn:src/util.rs::clamp) ≈ 0.2`; `edges[0].edges == [free→Point::new]`; after `selection = Point::new` and `selection_range = call_site`, `marks.ranges[free]` has one `Selection` range equal to `call_site`. Then repeat with focus `Point::new`: Callers `[free]`.

**Manual acceptance:** open a repo, focus a fn, Tab → columns as before + curves to callers/callees, dimming everywhere else, selected callee's call site highlighted; Up/Down/Left/Right/Enter behave as before; Esc restores; pan/zoom blocked while the panels are open; `outrider edges calls --cross-boundary folder` on a large repo shows a "resolving N symbols" count that ticks down and never stalls the frame; zooming out bundles curves between folders with thicker strokes.

---

## 10. Open questions / risks

1. **`reach` over an async provider** (02-set.md) sees `Pending` as "no edges"; the resulting `ResolvedSet` should carry `incomplete: bool` and the resolver should re-run it on `RELATIONS`. This spec assumes `SetResolver` uses `lookup` and honours `Pending`; confirm in 02-set.md.
2. **Unscoped `calls` cost** — whole-repo enqueue is correct but slow (minutes on large repos, one parse per referencing file per symbol). The single-pass `warm_all` resolver in `outrider-index` is the real fix; until then the count readout is the honesty mechanism.
3. **Lyon tessellation at MAX_EDGES** — dashed strokes are the expensive case. Fallback: cache built `Path<Pixels>` per bucket keyed on `(camera, resolved layer generation)`; or drop dashes below 1 px width.
4. **Panel/mode parity details** live in 08-panel.md (card heights, scroll easing `CG_SCROLL_SECS`, code preview lines = 15, `dock` beside the widened focus leaf via `call_graph_column_lefts`). This spec only fixes the key mapping and the sets/layers.
5. **`SymbolTree` clone at load** for the worker's `Arc` — acceptable (one per load); moving `TreemapView.tree` to `Arc<SymbolTree>` is a small follow-up touching `poll_loading`.
6. Edge hover/inspect (`metric · value · basis` for a bundle) is deferred; `EdgePaint.weight/merged` are already there for it.

---

## 11. Framework deltas

1. **`Deps::RELATIONS = Deps(1 << 8)`** in `deps.rs` — "an async relation provider's cache gained entries". Set by the app after `relations.poll()`.
2. **`RelationProvider`** keeps parent §7.3's four methods verbatim and adds `lookup`, `edge_detail`, `prefetch`, `poll`, `is_busy` with default impls (§4.1). Additive; static providers implement only the four.
3. **`RelationRegistry::builtin(tree: Arc<SymbolTree>)`** takes the tree (00-framework §3.5 wrote `builtin()`); adds `poll()`, `is_busy()`, `prefetch()`.
4. **`ProviderCtx`** is defined in `relation.rs` unless 03-fill.md already put it in `metric.rs`; either way one definition, fields `{ tree, index, repo_root }` at minimum.
5. **`SessionState.selection_range: Option<(&SymbolId, Range<usize>)>`** and the **`$selectionSite`** pseudo-id in 02-set.md (resolves to `ResolvedSet { ids: {sym}, ranges: {sym: [range]} }`), set by the panel when the current row is an edge-group row (08-panel.md).
6. **`paint_items` returns `PaintFrame`** (§5.1) instead of the `(items, doc_panel, cg_scrim)` tuple; `cg_scrim` is removed.
7. `crates/outrider-view/src/relation/` becomes a directory module (`mod.rs`, `calls.rs`, later `imports.rs`, `cochange.rs`); 00-framework's module list said `relation.rs`.
