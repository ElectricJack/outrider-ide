# 03 — Fill: metric → visual channel, and the metric infrastructure

**Parent:** [../view-primitives.md](../view-primitives.md) §2.3 (definition), §3.2 (channel ownership), §3.3 (provenance), §5 (`metrics` import block, `MetricRef`), §7.3 (`MetricProvider`), §8.3 (component spec).
**Framework:** [00-framework.md](00-framework.md) — this spec uses `FillSpec`, `LayerSpec::Fill`, `SetRef`, `Deps`, `ResolveCtx`, `ResolvedView { fill, stripe, opacity }`, `ViewResolver`, `ViewCommand::{PushLayer, RemoveLayer, ImportMetric}`, `validate`, `PaintOverrides::{stripe, fill}`, `default_view`, `MetricRegistry::builtin` exactly as defined there. Anything this spec needs beyond that is listed in §11.
**Related:** [02-set.md](02-set.md) (`where(metric op value)` uses the same percentile tables), [04-mask.md](04-mask.md) (mask multiplies the opacities this layer sets), [07-notes.md](07-notes.md) (renders the inspect readout defined in §4.7).

---

## 1. Purpose and scope

Fill binds one **numeric metric** to one **continuous visual channel** through a **scale**. It owns three channels: `fill` (box background), `stripe` (the left-edge heat bar churn uses today), and `opacity` (body/texture alpha). It never takes a color per symbol; color comes from a theme ramp keyed by the scaled value (parent §2.3, §3.3).

This file also owns `crates/outrider-view/src/metric.rs`: `MetricProvider`, `MetricRegistry`, the built-in providers, imported metrics (`ViewSpec.metrics`), `Scale`, and the percentile tables that `where(...)` sets (02-set.md) reuse.

In scope: types, resolution, ramps, the app-side paint hookup (replacing the churn special case at `treemap.rs:1691`), the churn toolbar toggle as `PushLayer`/`RemoveLayer`, the fill-cycling hotkey, the inspect readout data, and the loader for imported metrics keyed by `file:line`. Out of scope: per-language node-type lists for AST metrics (interface and caching only), fan-in/fan-out (needs the `calls` relation provider, 05-edges.md), rendering of the inspect Note (07-notes.md).

## 2. Ground truth: existing code touched

| file | symbol | ~line | what it does today | what changes |
|---|---|---|---|---|
| `crates/outrider-index/src/types.rs` | `SymbolNode { measure, churn, churn_count, children, byte_range }` | 49–65 | `churn` is a 0..1 percentile (files ranked among files, folders among folders, items inherit the file's), `churn_count` the raw commit count, `measure` = lines, `byte_range` = file-relative bytes (`Some(0..len)` for files, `None` for folders). | Read-only. Built-in providers wrap these fields. |
| `crates/outrider-index/src/churn.rs` | `percentiles(&[u64]) -> Vec<f32>`, `annotate` | 37–48, 256–341 | Percentile = fraction strictly below / (n−1); ties share; single element → 0. | Semantics copied for the f64 percentile table in `metric.rs` (§4.3); `churn` provider reports `node.churn` as its *native* percentile so the golden holds. |
| `crates/outrider-index/src/buffer.rs` | `FileBuffer::new` grammar table | 161–221 | Maps `SourceLanguage` → tree-sitter `Language` + highlight query. | AST providers need the same grammar lookup; factor `SourceLanguage::grammar(self) -> Option<tree_sitter::Language>` into `language.rs` (§4.5, §11). |
| `crates/outrider-index/src/language.rs` | `SourceLanguage::for_path` | 21–31 | Extension/filename → language. | Reused by the AST provider. |
| `crates/outrider-index/src/parse.rs` | `parse_rust_items` etc., `collect_items` | 196–222, 841 | Item extraction; the per-language `kind_fn` closures are the pattern the AST metric walkers will copy. | Not modified in this spec. |
| `crates/outrider/src/treemap.rs` | `paint_items` fill | 1506–1508 | `fill = theme::box_fill(node_box_kind, level, node_box_tint)` | Becomes `base_fill`; `fill = ov.fill(&id, base_fill)`. |
| | `paint_items` stripe | 1691–1692 | `stripe: (settings.show_churn && node.churn > 0.0).then(\|\| churn_heat(node.churn))` | `stripe: ov.stripe(&id)`. |
| | `paint_items` border | 1690 | `border: theme::border_for(fill)` | `border_for(base_fill)` — border keeps kind structure when fill is overridden. |
| | `paint_items` opacities | 1512–1513, 1595, 1649 | `body_opacity`/`tex_opacity` start 1.0; text mode sets `tex_opacity=0`, texture mode sets `body_opacity=0`. | Both multiplied by `ov.opacity(&id)` (fill `channel: opacity`) after the match, before the mask multiply (04-mask.md). |
| | canvas: stripe quad | 4338–4351 | Paints `item.stripe` as a `STRIPE_W` bar. | Unchanged. |
| | canvas: header bg quad (pass 2b) | 4489–4496 | Fills the pinned header with `item.fill`. | Unchanged — it already uses `item.fill`, so an overridden fill carries into the header. |
| | toolbar churn toggle | 4136–4148 | Flips `settings.show_churn`, saves, `cx.notify()`. | Calls `self.toggle_churn_layer(cx)` (§5.4). |
| | `on_key_down` map | 2276–2352 | enter/escape/end/home/tab/alt-arrows/arrows. | Adds `"f"` / `shift-f` → `ViewCommand::CycleFill` (§6). |
| | `on_mouse_move`, `on_left_release` | 2029–2060, 1990–2027 | Hover sets `hover_id` (doc nodes only); click sets focus. | Hover filter widened so filled nodes are hoverable for inspect (§5.5). |
| `crates/outrider/src/theme.rs` | `churn_heat`, `FILL_COLD/HOT`, `lerp_rgb`, `box_fill`, `border_for` | 171–173, 11–13, 163, 250, 275 | Churn ramp cold→hot. | `heat(Ramp, t)` generalisation, `categorical(i)`, `Ramp` enum; `churn_heat` becomes a wrapper (kept for `rasterize.rs:299`). |
| `crates/outrider/src/paint_model.rs` | `PaintItem { fill, border, stripe, body_opacity, tex_opacity }` | 41–62 | Owned paint instructions. | No new fields for this spec (`light` is 04-mask.md). |
| `crates/outrider/src/content.rs` | `card_meta`, `churn_readout` | 47–64 (`#[cfg(test)]`) | `"{count} · p{pct} · {lines}L"` — no longer rendered. | Readout format reused by `MetricReadout::text()` (§4.7). |
| `crates/outrider/src/settings.rs` | `Settings.show_churn` | 45–46 | Persisted toggle default `true`. | Kept as the *startup default only*; runtime truth is the presence of the churn fill layer. |
| `crates/outrider/src/rasterize.rs` | `container_fill` | 255–310 | Bakes children with `box_fill` and **always** a churn stripe (ignores `show_churn`). | Not changed here; see §10 (textures don't see fill layers). |

## 3. Spec types (`spec.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FillSpec {
    pub metric: String,                          // MetricRef: built-in id or a key of ViewSpec.metrics
    #[serde(default)] pub channel: FillChannel,  // default Fill
    #[serde(default)] pub scale: Scale,          // default Percentile
    #[serde(default, skip_serializing_if = "Option::is_none")] pub domain: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub ramp: Option<String>,
}
impl Default for FillSpec { /* metric: String::new() — invalid until validated (§4.8) */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FillChannel { #[default] Fill, Stripe, Opacity }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Scale {
    #[default] Percentile,        // "percentile"
    Log,                          // "log"
    Linear([f64; 2]),             // {"linear": [min, max]}
    Threshold(Vec<f64>),          // {"threshold": [t0, t1, ...]}  (ascending; validated)
    Categorical,                  // "categorical"  (raw is a class index)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedMetric {
    pub basis: String,                                        // mandatory, non-empty
    #[serde(default)] pub unit: String,                       // "%", "B", "ms", "" …
    #[serde(default)] pub values: BTreeMap<String, f64>,      // key → value (keys: §4.4)
    #[serde(default, skip_serializing_if = "Option::is_none")] pub file: Option<PathBuf>, // relative to repo_root
}
```
`Scale` is externally tagged by serde's default, which yields exactly the wire forms in the comments. `values` and `file` may both be present; `file` is loaded first and `values` override per key.

JSON:
```jsonc
{ "fill": { "metric": "churn",    "channel": "stripe", "scale": "percentile" } }
{ "fill": { "metric": "coverage", "channel": "fill",   "scale": { "threshold": [0, 50, 100] }, "ramp": "redGreen" } }
{ "fill": { "metric": "measure",  "channel": "opacity","scale": "log", "domain": "affected" } }
{ "fill": { "metric": "community","channel": "fill",   "scale": "categorical" } }
// ViewSpec.metrics
"metrics": {
  "coverage":  { "basis": "lcov 2026-08-14", "unit": "%",
                 "values": { "file:src/auth/login.rs": 12.5, "src/auth/login.rs::verify": 0, "src/auth/login.rs:41": 0 } },
  "peakBytes": { "basis": "heaptrack run 3", "unit": "B", "file": ".outrider/metrics/heap.json" }
}
```

## 4. Resolution (`outrider-view`)

### 4.1 `metric.rs` — provider trait and registry

```rust
pub struct ProviderCtx<'a> { pub tree: &'a SymbolTree, pub index: &'a TreeIndex<'a>, pub repo_root: &'a Path }

pub trait MetricProvider: Send + Sync {
    fn id(&self) -> &str;
    fn basis(&self) -> &str;                    // human-readable provenance, shown by inspect
    fn unit(&self) -> &str;                     // "" | "commits" | "lines" | "%" | "B" | …
    fn value(&self, node: &SymbolNode, ctx: &ProviderCtx) -> Option<f64>;   // None = no data for this node
    fn deps(&self) -> Deps;
    /// A percentile the provider already knows (e.g. `SymbolNode.churn`). When `Some`, the
    /// resolver uses it for `Scale::Percentile` instead of the domain table, unless the fill
    /// sets an explicit `domain`. Default `None`.
    fn native_percentile(&self, _node: &SymbolNode) -> Option<f32> { None }
}

pub struct MetricRegistry { providers: BTreeMap<String, Box<dyn MetricProvider>> }
impl MetricRegistry {
    pub fn builtin(tree: &SymbolTree) -> Self;                     // §4.2; `tree` reserved for AST provider seeding
    pub fn register(&mut self, p: Box<dyn MetricProvider>);       // replaces same id
    pub fn register_imported(&mut self, name: &str, m: &ImportedMetric, tree: &SymbolTree, repo_root: &Path) -> ImportReport;
    pub fn get(&self, id: &str) -> Option<&dyn MetricProvider>;
    pub fn contains(&self, id: &str) -> bool;                      // what `validate` is given as `known_metrics`
    pub fn ids(&self) -> impl Iterator<Item = &str>;
    pub fn readouts(&self, node: &SymbolNode, ctx: &ProviderCtx) -> Vec<MetricReadout>;   // `query metrics <symbol>`
}
pub struct ImportReport { pub resolved: usize, pub unresolved: Vec<String>, pub warnings: Vec<String> }
```

### 4.2 Built-in providers (`metric.rs`, one struct each)

| id | raw `value` | unit | native percentile | basis | deps |
|---|---|---|---|---|---|
| `churn` | `Some(node.churn_count as f64)` | `commits` | `Some(node.churn)` | `"git log --numstat · files ranked among files, folders among folders, items inherit their file"` | `GIT \| TREE` |
| `churnCount` | `Some(node.churn_count as f64)` | `commits` | `None` (plain rank over the domain) | `"git log --numstat commit count"` | `GIT \| TREE` |
| `measure` | `Some(node.measure as f64)` | `lines` | `None` | `"line count from index"` | `TREE` |
| `entities` | `Some(node.children.len() as f64)` | `children` | `None` | `"direct child count"` | `TREE` |

`churn` vs `churnCount`: identical raw, different percentile semantics. `churn` reproduces today's stripe exactly (the golden); `churnCount` gives a whole-domain rank for `where(churnCount > p90)`.

**AST metrics (`complexity`, `nesting`, `params`)** — one `AstMetricProvider` per id sharing an `Arc<AstCache>`; **not registered until implemented**, so `fill {metric: complexity}` is rejected by validation with `unknown metric 'complexity'` rather than silently painting nothing. Interface (§4.5) and caching are specified now; per-language node lists are a follow-up. **`fanin`/`fanout`** are `RelationMetricProvider { relation: "calls", dir }` = `in_edges/out_edges(...).len()`; they land with 05-edges.md and register through the same `register()`.

### 4.3 `layers/fill.rs` — resolving a `FillSpec`

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricValue { pub raw: f64, pub scaled: f32 /* 0..1, or class index for Categorical */, pub percentile: f32 }

pub struct ResolvedFill {
    pub metric: String, pub channel: FillChannel, pub scale: Scale,
    pub values: BTreeMap<SymbolId, MetricValue>,      // only nodes where value() was Some and ∈ domain
    pub basis: String, pub unit: String, pub ramp: Option<String>,
    pub deps: Deps,
}

pub(crate) fn resolve_fill(spec: &FillSpec, ctx: &ResolveCtx, sets: &BTreeMap<String, ResolvedSet>,
                           tables: &mut PercentileCache) -> Result<ResolvedFill, String>;
```
Algorithm:
1. `provider = ctx.metrics.get(&spec.metric)` (validated earlier; `Err` if missing).
2. Domain = ids in `spec.domain` (resolved set) ∩ laid-out nodes (`ctx.layout.rects` keys); with no `domain`, every laid-out node. Iterate the tree in pre-order (`ctx.index.iter()`) and skip nodes not in the domain.
3. `raw = provider.value(node, &pctx)`; `None` → skip.
4. Percentile: if `spec.domain.is_none()` and `provider.native_percentile(node)` is `Some(p)` → `p`; else `tables.get(&spec.metric, domain_hash, || collect raws)`.of(raw)`.
5. `scaled = scale_value(&spec.scale, raw, percentile, &stats)`:
   - `Percentile` → `percentile`
   - `Log` → `ln(1+max(raw,0)) / ln(1+stats.max)` (0 if `max ≤ 0`)
   - `Linear([lo,hi])` → `((raw−lo)/(hi−lo)).clamp(0,1)` (0 if `hi ≤ lo`)
   - `Threshold(ts)` → class `k = ts.partition_point(|t| *t <= raw)` (d3 convention: `< t0` → 0, `[t0,t1)` → 1, …, `≥ t_last` → n); `scaled = k / ts.len()`
   - `Categorical` → `raw` (index; must be a small non-negative integer — non-integers are truncated, negatives → 0)
6. `deps = provider.deps() | domain set deps | (imported ? METRICS : NONE) | SPEC`.

**Percentile table** (`metric.rs`):
```rust
pub struct DomainStats { sorted: Vec<f64>, pub min: f64, pub max: f64 }
impl DomainStats { pub fn of(&self, v: f64) -> f32 /* partition_point(x < v) / (n-1); n≤1 → 0 */ }
pub struct PercentileCache { map: HashMap<(String /*metric*/, u64 /*domain hash*/), (Arc<DomainStats>, Deps)> }
```
Semantics match `churn::percentiles` (ties share, strictly-below over n−1). The cache lives inside `ViewResolver` and is shared with `where(...)`; entries drop when `dirty ∩ deps ≠ ∅` (provider deps ∪ TREE ∪ domain deps).

**Combination across layers** (framework §2.4): iterate `spec.layers`; for `channel: fill` keep the last `ResolvedFill` in `ResolvedView.fill`; `stripe` likewise into `.stripe`; every `opacity` fill is pushed to `.opacity` (they multiply). Later layers *replace* earlier ones per channel — no merging of `values`.

### 4.4 Imported metrics — key resolution and the line-table loader

`register_imported` materialises `ImportedProvider { id, basis, unit, values: BTreeMap<SymbolId, f64>, deps: METRICS }`. Keys are accepted in three forms, tried in order:
1. **Wire `SymbolId`** — `symbol_id::parse_wire(key)` succeeds *and* `index.node(&id)` is `Some`.
2. **Bare qualified path** — `key` equals `qualified_path` of a node of any kind (`src/auth/login.rs`, `src/auth/login.rs::verify`). Lookup through a `HashMap<&str, Vec<&SymbolId>>` built once per import from `index.iter()`. Ambiguity (e.g. `struct Point` and `impl Point`) → pick the lowest `(kind label, ordinal)` and push a warning `"key 'x' is ambiguous (n symbols); using <wire id>"`.
3. **`file:line`** — split at the *last* `:`; suffix all digits → `line` (1-based); prefix must be a `SymbolKind::File` node's qualified path. `byte = LineTable::for_file(repo_root, path).byte_of_line(line)?`; then descend from the file node: repeatedly pick the child whose `byte_range` contains `byte` (children's ranges are disjoint and cover the file, `index_test.rs::assert_children_cover_source`); the deepest match is the target. If the file node has no children the file itself is the target.

Otherwise the key goes to `ImportReport.unresolved` (never an error — the rest of the import proceeds).

```rust
pub struct LineTable { starts: Vec<usize>, pub len: usize }
impl LineTable {
    /// Reads the file with std::fs::read (outrider-view has no BufferManager); `\n`-terminated lines,
    /// CRLF tolerated because we only need line *starts*.
    pub fn load(repo_root: &Path, rel: &str) -> std::io::Result<LineTable>;
    pub fn byte_of_line(&self, line1: usize) -> Option<usize>;   // start byte of the line, None if out of range
}
struct ImportSession { tables: HashMap<String, Option<LineTable>> }   // per-file memo for one import
```
If `table.len != file_node.byte_range.end`, warn `"src/x.rs changed since index (N vs M bytes); line keys may be off"`. `file:` loading: JSON object `{key: number}` or CSV `key,value` (header row optional; parse errors are `Err` for the whole import). Both are read relative to `repo_root`.

The import runs at `ImportMetric`/`Apply` time (app side, §5.3), not per frame; the resulting provider is keyed by `SymbolId` so resolution is a map lookup.

### 4.5 AST provider interface (follow-up, interface fixed here)

```rust
// outrider-index/src/ast_metrics.rs (new; keeps tree-sitter out of outrider-view)
pub struct NodeMetrics { pub complexity: u32, pub nesting: u32, pub params: u32 }
pub fn compute(source: &[u8], lang: SourceLanguage) -> anyhow::Result<Vec<(Range<usize>, NodeMetrics)>>;
//   one entry per item node the parser would emit (same byte ranges as `parse_*_items`), so a
//   SymbolNode is matched by `byte_range` equality; falls back to the smallest containing range.
// per language: `branch_kinds`, `block_kinds`, `param_list_kind` tables — out of scope here.

// outrider-view/src/metric.rs
pub struct AstCache { files: Mutex<HashMap<String /*rel*/, Arc<FileAst>>> }
struct FileAst { len: usize, entries: Vec<(Range<usize>, NodeMetrics)> }
pub struct AstMetricProvider { which: AstMetric /*Complexity|Nesting|Params*/, cache: Arc<AstCache> }
```
`value(node)`: `rel = qualified_path.split("::").next()`; load/parse once per file (`std::fs::read`), memoised in `AstCache` for the session; `deps = TREE`. Cache invalidates on TREE (re-index). Files whose byte length differs from the file node's `byte_range.end` are parsed anyway but flagged in `basis` ("source changed since index").

### 4.6 Ramps (`theme.rs`)

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Ramp { #[default] Heat /*FILL_COLD→FILL_HOT*/, Cool /*FILL_COLD→0x3070c0*/, RedGreen /*0xb03030→0x3a8a3a*/, Gray /*0x2a2a2e→0xb8b8b8*/ }
impl Ramp { pub fn parse(id: &str) -> Option<Ramp> /* "heat"|"cool"|"redGreen"|"gray" */ }
pub fn heat(ramp: Ramp, t: f32) -> u32;          // lerp_rgb(lo, hi, t.clamp(0,1)); heat(Heat, t) == churn_heat(t)
pub fn churn_heat(t: f32) -> u32 { heat(Ramp::Heat, t) }   // kept for rasterize.rs
pub fn categorical(i: usize) -> u32;             // 8-entry palette, i % 8: 0x4da6ff 0xd08a3a 0x5fb35f 0xc45c8a 0x8f7ad6 0x3fb3b3 0xc9c04a 0x9a9a9a
```
Default ramp per channel when `FillSpec.ramp` is `None`: `stripe`/`fill` → `Heat`; `Categorical` scale ignores the ramp and uses `categorical(scaled as usize)`. Unknown ramp id → validation *warning* + default. Add the new constants to `theme::fingerprint()` (textures embed theme colors).

### 4.7 Inspect readout (data; rendering in 07-notes.md)

```rust
pub struct MetricReadout { pub metric: String, pub raw: f64, pub unit: String, pub percentile: f32,
                           pub scaled: f32, pub scale: Scale, pub channel: Option<FillChannel>, pub basis: String }
impl MetricReadout {
    /// "churn 12 commits · p87 · git log --numstat …"; Threshold adds "· class 2/3"; unit "" is omitted.
    pub fn text(&self) -> String;
}
```
`ResolvedView` gains nothing; the app's `PaintOverrides::inspect(&id) -> Vec<MetricReadout>` (§5.2) collects one readout per active channel (`fill`, `stripe`, each `opacity`) that has a value for `id`. The note pass (07-notes.md) turns them into a `Note { source: Metric, text }` at `$hover` (and at `$focus` when a click on a filled node happens). `MetricRegistry::readouts(node)` (all registered metrics, `channel: None`) backs `outrider query metrics <symbol>`.

### 4.8 Validation (`validate.rs`, additions)

Hard: `fill.metric` empty or unknown (`known_metrics(id) == false`) → `"fill requires a metric"` / `"unknown metric '<id>'"`; `Threshold` not strictly ascending or empty; `Linear` with `hi <= lo`; `domain` unknown set name; `metrics.<name>.basis` empty; `metrics.<name>` with neither `values` nor `file`. Soft: unknown `ramp`; a `Categorical` scale on a non-integer metric (checked at resolve time → warning). Names in `ViewSpec.metrics` that collide with a built-in id are hard errors (`"metric 'churn' is built-in"`).

## 5. App integration (`crates/outrider`)

### 5.1 `paint_items` (`treemap.rs` ~1506–1513, ~1595, ~1649, ~1689–1699)

```rust
let base_fill = theme::box_fill(box_kind, item.level, tint);      // was `fill`
let fill = ov.fill(&item.node.id, base_fill);
…
// after the `match item.draw {}` block, before building PaintItem:
let fill_opacity = ov.opacity(&item.node.id);
body_opacity *= fill_opacity;
tex_opacity  *= fill_opacity;
…
PaintItem { fill, border: theme::border_for(base_fill), stripe: ov.stripe(&item.node.id), … }
```
The header quad in pass 2b (4489–4496) already paints `item.fill`, so an overridden fill covers the pinned header too — required, otherwise a lit fill would show a base-colored header. `border_for(base_fill)` keeps the kind/depth structure readable under a solid heat fill (parent §8.3).

### 5.2 `view/paint_resolver.rs`

```rust
impl PaintOverrides<'_> {
    pub fn stripe(&self, id: &SymbolId) -> Option<u32> {
        let f = self.resolved.stripe.as_ref()?;
        let v = f.values.get(id)?;
        // stripe is an accent: zero heat paints no bar. This is exactly today's `churn > 0.0` rule.
        (v.scaled > 0.0).then(|| color_for(f, v))
    }
    pub fn fill(&self, id: &SymbolId, base: u32) -> u32 {
        match self.resolved.fill.as_ref().and_then(|f| f.values.get(id).map(|v| color_for(f, v))) { Some(c) => c, None => base }
    }
    pub fn opacity(&self, id: &SymbolId) -> f32 {           // product over opacity fills; floor keeps text findable
        self.resolved.opacity.iter().filter_map(|f| f.values.get(id)).map(|v| v.scaled.max(OPACITY_FLOOR)).product()
    }
    pub fn inspect(&self, id: &SymbolId) -> Vec<MetricReadout>;   // §4.7
}
const OPACITY_FLOOR: f32 = 0.15;
fn color_for(f: &ResolvedFill, v: &MetricValue) -> u32 {
    if f.scale == Scale::Categorical { theme::categorical(v.scaled as usize) }
    else { theme::heat(f.ramp.as_deref().and_then(theme::Ramp::parse).unwrap_or_default(), v.scaled) }
}
```
Nodes without a value keep the base fill / no stripe / opacity 1.0 (a fill over `coverage` leaves un-instrumented files looking normal — inspect says "no data").

### 5.3 Registries and imports (`treemap.rs` loader completion path; `apply_view_command`)

- On load: `self.metrics = MetricRegistry::builtin(&self.tree)`, then `for (name, m) in &self.view_spec.metrics { report = self.metrics.register_imported(name, m, &self.tree, &self.tree.repo_root); notify warnings/unresolved count }`.
- In `apply_view_command`, when `Applied.changed` contains `METRICS`: re-register every `view_spec.metrics` entry (cheap; imports change rarely) and push a `Notification::warning` per `ImportReport.warnings` plus `"<name>: k of n keys unresolved"` when `unresolved` is non-empty. `validate` is given `|id| self.metrics.contains(id) || spec.metrics.contains_key(id)` so a document may reference a metric it imports itself.

### 5.4 Churn toolbar toggle (`treemap.rs` ~4136–4148)

```rust
fn churn_stripe_layer() -> LayerSpec { LayerSpec::Fill(FillSpec { metric: "churn".into(), channel: FillChannel::Stripe, scale: Scale::Percentile, domain: None, ramp: None }) }
fn churn_layer_index(&self) -> Option<usize> {
    self.view_spec.layers.iter().position(|l| matches!(l, LayerSpec::Fill(f) if f.metric == "churn" && f.channel == FillChannel::Stripe))
}
fn toggle_churn_layer(&mut self, cx: &mut Context<Self>) {
    let cmd = match self.churn_layer_index() { Some(i) => ViewCommand::RemoveLayer(i), None => ViewCommand::PushLayer(churn_stripe_layer()) };
    self.apply_view_command(cmd);
    self.settings.show_churn = self.churn_layer_index().is_some();      // persist as startup default
    self.global_settings.show_churn = self.settings.show_churn; let _ = self.global_settings.save();
    cx.notify();
}
```
The toggle's checked state is `self.churn_layer_index().is_some()` (not `settings.show_churn`), so a CLI `outrider layer rm` is reflected in the toolbar. `default_view(settings)` still seeds the layer iff `settings.show_churn` (framework §3.3).

### 5.5 Inspect affordance

`on_mouse_move` (~2052–2054) currently only records hover for nodes with a doc: change the filter to `i.node.doc.is_some() || ov_has_metric(&i.node.id)` where `ov_has_metric` checks `resolved.fill/stripe/opacity` maps (cheap; `view_resolver` cache is available on `self`). Rendering of the readout is 07-notes.md's `metric` note at `$hover`; until 07 lands, `PaintOverrides::inspect` is only exercised by tests and `query metrics`.

### 5.6 New files
`crates/outrider/src/view/paint_resolver.rs` (methods above; framework already declares the struct). No other new app files for this spec.

## 6. Commands

| driver | ViewCommand | effect |
|---|---|---|
| `outrider fill <metric> [--channel fill\|stripe\|opacity] [--scale percentile\|log\|linear=a,b\|threshold=a,b,c\|categorical] [--domain set] [--ramp id]` | `PushLayer(LayerSpec::Fill(..))` | appends; last-wins makes it active; `PopLayer` restores the previous one (framework §2.5 — no dedupe) |
| `outrider metric import <name> <file.json\|csv> --basis "…" [--unit u]` | `ImportMetric { name, metric: ImportedMetric { file: Some(path), … } }` | stored in `spec.metrics`; `Applied.changed ∋ METRICS`; app re-registers (§5.3) |
| `outrider layer rm <i>` / `pop` | `RemoveLayer(i)` / `PopLayer` | as framework |
| toolbar "Git Churn" | `PushLayer`/`RemoveLayer` (§5.4) | |
| hotkey **`f`** / **`shift-f`** | `CycleFill { channel: Fill }` / `CycleFill { channel: Stripe }` (§11 delta) | rotates the fill layers of that channel so the previously-last becomes first and the next one becomes last (= active). ≤1 such layer → no-op. Not bound while palette/settings/call-graph overlays are open (`on_key_down` early returns already handle that). |
| `outrider query metrics <symbol>` | (query) | `MetricRegistry::readouts` → JSON `[MetricReadout]` |

`CycleFill` semantics in `command.rs`: collect indices of `LayerSpec::Fill` with the given channel in order; if `n ≥ 2`, remove the layer at the last index and insert it at the first index (all other layers keep relative order); `Applied.changed = SPEC`.

## 7. Invalidation

| Deps bit | set by (framework §3.4) | drops |
|---|---|---|
| `TREE` | loader completion, packing snapshot | every `ResolvedFill`, `PercentileCache`, `AstCache`, imported providers are re-registered against the new tree |
| `GIT` | git watcher tick (02-set.md) | fills over `churn`/`churnCount` and their percentile tables |
| `METRICS` | `apply(ImportMetric)`, `Apply` with a different `metrics` block | fills over imported metrics |
| `SPEC` | any `apply` | fills whose layer changed (resolver recomputes all fills — they are cheap: one map per laid-out node) |
| set bits (`FOCUS`, …) | via `domain` | fills with a live `domain` |

Nothing here depends on `CAMERA`.

## 8. Migration steps

1. **Types.** Add `FillSpec`, `FillChannel`, `Scale`, `ImportedMetric` to `spec.rs`; add fill rules to `validate.rs` (§4.8). Test JSON round-trip of every `Scale` form.
2. **metric.rs.** `MetricProvider` (+ `native_percentile`), `MetricRegistry::{builtin, register, get, contains, ids, readouts}`, four built-in providers, `DomainStats`, `PercentileCache`, `MetricReadout`. Unit-test percentile equivalence with `churn::percentiles`.
3. **layers/fill.rs.** `resolve_fill`, `MetricValue`, `ResolvedFill`; wire into `ViewResolver::resolve` step (2) with last-wins/opacity-append combination.
4. **theme.rs.** `Ramp`, `heat`, `categorical`; `churn_heat` delegates; extend `fingerprint()`. Existing theme tests unchanged (`churn_heat(0.5) == 0x6d2d2f` still holds).
5. **paint_resolver.rs.** `stripe`, `fill`, `opacity`, `inspect`.
6. **treemap.rs paint_items.** `base_fill`/`fill`/`border_for(base_fill)`/`stripe`/opacity multiply (§5.1). **Delete** the `self.settings.show_churn && item.node.churn > 0.0` expression at 1691. Behaviour-preserving given `default_view` seeds the churn stripe iff `show_churn`.
7. **Toolbar toggle** → `toggle_churn_layer` (§5.4). Delete the direct `settings.show_churn = !…` flip.
8. **Imports.** `ImportedProvider`, key resolution, `LineTable`, `register_imported`, app-side re-register (§5.3). *(May be scheduled with milestone 6; the types from step 1 already validate documents that carry `metrics`.)*
9. **Hotkey + `CycleFill`** (§6). **Inspect** hover widening (§5.5) — lands with 07-notes.md.
10. **AST providers** — `outrider_index::ast_metrics` + `AstMetricProvider`; register in `builtin` only once `compute` exists for at least Rust.

Steps 1–7 minus imports/hotkey are the **Milestone-0 subset** (next section).

### Milestone-0 subset (what 00-framework.md §5 step 1 needs)

- `FillSpec`/`FillChannel`/`Scale`/`ImportedMetric` types with serde (full, they are small) and validation "fill without metric / unknown metric".
- `MetricProvider` + `MetricRegistry::builtin` with `churn`, `churnCount`, `measure`, `entities`; `native_percentile` on `churn`.
- `resolve_fill` for `channel: stripe` and `channel: fill`, all `Scale` variants (pure functions), percentile table (needed even in M0 for `measure`); `opacity` channel may be resolved but `PaintOverrides::opacity` can be deferred (framework §3.2 lists only `stripe`/`fill`).
- `theme::heat(Ramp::Heat, t)`; `PaintOverrides::{stripe, fill}`; `paint_items` rewiring; toolbar toggle as `PushLayer`/`RemoveLayer`.
- **Not** in M0: imports (`register_imported`, `LineTable`), AST providers, `CycleFill` hotkey, inspect hover widening, ramps other than `Heat`/`categorical`.

## 9. Tests

**`outrider-view` (fixture `crates/outrider-index/tests/fixtures/mini_repo`, indexed via `index_repo` as in `index_test.rs`; churn via the temp-git helper from `churn_test.rs::git_fixture`; layout via `outrider_layout::pack` with `world::pack_config` constants copied):**
- `fill_churn_stripe_matches_node_churn` — for every laid-out node, `stripe.values[id].scaled == node.churn` (f32 exact) and present iff the node exists; with `git_fixture`, `src/lib.rs` has the highest percentile.
- `fill_percentile_table_matches_churn_percentiles` — `DomainStats` over `[10,20,30,20]` gives `[0, 1/3, 1, 1/3]`; `[7]` → 0; empty → none.
- `scale_threshold_classes` — `Threshold([0,50,100])`: −1→0/3, 0→1/3, 12.5→1/3, 50→2/3, 100→1, 150→1. `scale_linear_clamps`, `scale_log_max_is_one`, `scale_categorical_passthrough`.
- `fill_last_wins_per_channel` — two `channel: fill` layers (`measure`, `entities`): `resolved.fill.metric == "entities"`; `PopLayer` → `"measure"`.
- `fill_domain_restricts_and_reranks` — `domain: ids([src/lib.rs, src/util.rs])` with `metric: measure`: only two entries; percentiles `0` and `1`.
- `import_by_wire_path_and_file_line` — `ImportedMetric { values: {"fn:src/lib.rs::free": 1, "src/lib.rs::Point": 2, "src/lib.rs:16": 3, "src/lib.rs:999": 4, "nope": 5} }` on a copied fixture: `free` and `Point` (struct, ordinal 0, with an ambiguity warning) resolve; `src/lib.rs:16` (inside `impl Point` → `fn new`) resolves to `fn:src/lib.rs::Point::new`; `:999` and `nope` are in `unresolved`.
- `import_file_json_and_csv` — same values from `.outrider/metrics/x.json` and `x.csv` under a tempdir repo root.
- `validate_fill_without_metric_rejected`, `validate_unknown_metric_rejected`, `validate_threshold_must_ascend`, `validate_import_requires_basis`, `validate_builtin_name_collision`.
- `cycle_fill_rotates_channel_layers` — three fill layers `[A(fill), S(stripe), B(fill)]` → after `CycleFill{Fill}`: `[B, A(fill), S]`… precisely: fill-channel order `[A,B]` becomes `[B,A]`, `S` keeps its position relative to the remaining layers; a second cycle restores; `CycleFill{Stripe}` is a no-op.
- `readout_text_format` — `"churn 12 commits · p87 · git log …"`, `"coverage 12.5% · p40 · class 1/3 · lcov 2026-08-14"`.

**App crate:**
- `paint_decisions_golden` (framework §4): with `default_view(show_churn=true)` resolved over the fixture, `stripe == Some(churn_heat(node.churn))` iff `node.churn > 0`, `fill == box_fill(..)`, `border == border_for(fill)`; with `show_churn=false` stripes are all `None`.
- `toggle_churn_layer_round_trip` — pure test on `ViewSpec`: toggle twice returns the same layer list; the toolbar checked state follows `churn_layer_index()`.
- `theme::heat_default_ramp_equals_churn_heat`, `categorical_wraps`.

**Manual acceptance:** open a repo — no visible change; toggle Git Churn — stripes come and go, toolbar state correct; `.outrider/views/x.json` with `fill {measure, fill, log}` (once the watcher from 10 exists, or via a test hook) — folders and files fill on the heat ramp, borders keep depth structure, pinned headers match the fill; hover a filled node — (07) readout shows `measure 480 lines · p96 · line count from index`.

## 10. Open questions / risks

1. **Textures don't see fill layers.** `rasterize::container_fill` (255–310) bakes children with `box_fill` and always paints a churn stripe (`rasterize.rs:298` ignores `show_churn` — pre-existing). At Card/Label rungs the folder texture will show base fills while live boxes show the fill layer. Fix later by passing `fill_of`/`stripe_of` closures into `bake_container` and adding the fill layer's hash to the texture key; until then a `channel: fill` view is only fully correct at Detail+ rungs. Flagging, not fixing, in this spec.
2. **Solid heat fills under code text.** `Ramp::Heat` at `t→1` is `0xb03030`; `TEXT_PRIMARY` stays legible but the page loses its "editor black" identity. If it reads badly, blend for `BoxKind::Leaf` (`lerp(CODE_BG, heat, 0.5)`) — that needs the box kind in `PaintOverrides::fill`, an additive signature change.
3. **`file:line` against edited files.** The line table reads the working tree, the index may be older; the byte-length check only warns. Parent §10.2 already tracks this.
4. **Stripe channel hides class 0.** By the "accent" rule (§5.2) a `threshold` scale's lowest class paints no stripe. Documented; use `channel: fill` when zero must be visible.
5. **`native_percentile` and `domain`.** With an explicit `domain`, `churn` re-ranks over the domain (folders and files together), which differs from `node.churn`. Intended, but inspect must show which one — `MetricReadout.percentile` is whatever was used.
6. `MetricRegistry::builtin(tree)` doesn't currently need `tree`; kept for the framework signature and the AST seeding.

## 11. Framework deltas

- **Additive:** `PaintOverrides::opacity(&self, id) -> f32` and `PaintOverrides::inspect(&self, id) -> Vec<MetricReadout>` (00-framework §3.2 lists only `stripe`/`fill`/`light`/…).
- **Additive:** `ViewCommand::CycleFill { channel: FillChannel }` in `command.rs` (§6); pure, `Applied.changed = SPEC`.
- **Additive:** `MetricProvider::native_percentile` default method on top of parent §7.3's four methods, plus `unit()`.
- **Additive:** `ViewResolver` owns a `PercentileCache` shared by fills and `where(...)` sets.
- **Outside outrider-view (index crate):** `SourceLanguage::grammar(self) -> Option<tree_sitter::Language>` factored out of `buffer.rs`; new module `outrider_index::ast_metrics` (follow-up).
- No changes to `ResolvedView`, `Deps`, `SessionState`, `ResolveCtx`, or `apply` semantics.
