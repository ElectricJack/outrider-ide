# 04 — Mask: attention control (spotlight / dim)

**Parent:** [../view-primitives.md](../view-primitives.md) §2.4 (definition), §3.2 (box/body opacity ownership: masks AND, fill opacity multiplies), §4 (call-graph mode = Mask + Edges + Panel), §8.4 (component spec), §9 milestone 2.
**Framework:** [00-framework.md](00-framework.md) — uses `MaskSpec`, `LayerSpec::Mask`, `SetRef`, `Deps`, `ResolveCtx` (`index: &TreeIndex`), `ResolvedView.mask: MaskTable`, `ViewResolver`, `PaintOverrides::light`, `PaintItem.light`, `ViewCommand::{PushLayer, RemoveLayer}`, `apply_view_command`, `validate`. Deltas in §11.
**Related:** [03-fill.md](03-fill.md) (`PaintOverrides::opacity` — fill opacity multiplies *before* light), [06-marks.md](06-marks.md) (nav rings are dimmed by light, not skipped), [07-notes.md](07-notes.md) (notes on dimmed nodes), [05-edges.md](05-edges.md)/[08-panel.md](08-panel.md) (call-graph migration that finishes deleting `CallGraphMode`).

---

## 1. Purpose and scope

A Mask is boolean attention control: dim everything except a set (**spotlight**) or dim a set. It owns dimming only — body/texture opacity, fill desaturation, border and name-row alpha, ring alpha. It never changes a fill *value* (parent §2.4) and never geometry.

In scope: `MaskSpec` + serde, `MaskTable` resolution with AND semantics and container inheritance, `theme::dim_toward`, the per-item paint changes in the canvas closure, replacing the full-screen `cg_scrim` quad with a mask layer pushed by call-graph mode, texture interaction, performance rules, tests. Out of scope: the rest of the call-graph migration (edges, panel — milestone 4).

## 2. Ground truth: existing code touched

| file | symbol | ~line | what it does today | what changes |
|---|---|---|---|---|
| `crates/outrider/src/treemap.rs` | `paint_items` return | 1433, 1805–1806 | Returns `(Vec<PaintItem>, Option<DocPanel>, bool /*cg_scrim*/)`; `cg_scrim = self.call_graph.is_some()`. | Bool renamed `in_call_graph`; only used to suppress the doc panel until 07-notes.md. Scrim quad deleted. |
| | `paint_items` opacities | 1512–1513, 1595, 1649 | `body_opacity`, `tex_opacity` set by draw mode. | Multiplied by `light = ov.light(&id)`; `PaintItem.light` filled. |
| | `PaintItem` construction | 1682–1704 | `fill`, `border`, `stripe`, `neighbor`, `focused` … | `fill`/`border`/`stripe` passed through `theme::dim_toward(c, light)`; `light` stored. |
| | canvas `paint_surface` | 4325–4387 | Box quad + stripe + texture (+ CODE_BG fade quad when `tex_opacity < 1`). | Adds a `theme::DIM` overlay quad at alpha `1-light` over the texture rect (§5.2). |
| | canvas `paint_text` name row | 4389–4403 | Name shaped with `run(len, TEXT_PRIMARY)` — never dimmed. | `run` color `.opacity(item.light)`. |
| | canvas `paint_text` body | 4426–4436 | Body runs get `.opacity(body_opacity)`. | Unchanged (body_opacity already carries light). |
| | canvas scrim | 4499–4508 | `if cg_scrim { paint_quad(bounds, rgba(0x000000cc)) }` — global dim of everything painted so far. | **Deleted**. |
| | canvas `paint_ring` | 4509–4527 | Focus ring `FOCUS_BORDER` 2px, neighbor ring `NEIGHBOR_BORDER` 1px. | Ring color alpha × `item.light`. |
| | canvas pass 2c | 4528–4541 | Neighbor rings skipped when `cg_scrim`. | Guard deleted; rings paint dimmed. |
| | canvas pass 4 doc panel | 4560 | Skipped when `cg_scrim`. | `.filter(\|_\| !in_call_graph)` (same behaviour, new name). |
| | `enter_call_graph` / `exit_call_graph` | 2430–2470, 2511–2525 | Sets/clears `self.call_graph`. | Push / remove `mask { dimExcept: "focusSet", strength: 0.8 }` (§5.4). |
| | `CallGraphMode`, `render_call_graph` | 635–642, 3222+ | Overlay columns (GPUI divs). | Untouched here (milestone 4). |
| `crates/outrider/src/theme.rs` | `BG`, `CODE_BG`, `lerp_rgb`, `NEIGHBOR_BORDER`, `FOCUS_BORDER` | 9, 38, 163, 281, 19 | Colors. | Adds `DIM`, `dim_toward`, `with_alpha`; `fingerprint()` extended. |
| `crates/outrider/src/paint_model.rs` | `PaintItem` | 41–62 | — | `pub(crate) light: f32` (declared by framework §3.2; consumed here). |
| `crates/outrider/src/rasterize.rs` | `bake_container` | 245–312 | Textures are baked without any dimming. | Unchanged; dimming is a live overlay quad. |
| `crates/outrider/src/focus.rs` → `outrider_index::TreeIndex` | `TreeIndex::{node, parent, iter}` | 12–60 | Parent/depth lookup. | Used for ancestor-closure and inheritance (§4.2). |

## 3. Spec types (`spec.rs`)

serde's `deny_unknown_fields` cannot combine with `#[serde(flatten)]`, so the two targets are optional fields validated to be exactly-one, not an enum:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaskSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub dim_except: Option<SetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub dim: Option<SetRef>,
    #[serde(default = "MaskSpec::default_strength")] pub strength: f32,        // 0..=1, default 0.8
}
impl Default for MaskSpec { fn default() -> Self { Self { dim_except: None, dim: None, strength: 0.8 } } } // invalid until validated
impl MaskSpec {
    pub const fn default_strength() -> f32 { 0.8 }
    pub fn target(&self) -> Option<MaskTarget<'_>>;   // Some(DimExcept(r)) | Some(Dim(r)) | None when 0 or 2 set
}
pub enum MaskTarget<'a> { DimExcept(&'a SetRef), Dim(&'a SetRef) }
```
JSON (exactly the parent §5 form):
```jsonc
{ "mask": { "dimExcept": "affected", "strength": 0.8 } }
{ "mask": { "dim": { "glob": "**/*_test.rs" }, "strength": 0.5 } }
{ "mask": { "dimExcept": "focusSet" } }                       // strength defaults to 0.8
```
Validation (hard): neither or both of `dimExcept`/`dim`; `strength` outside `0..=1` or NaN; unknown set name. `strength: 0` is legal and a no-op (§9).

## 4. Resolution (`outrider-view/src/layers/mask.rs`)

### 4.1 `MaskTable`

```rust
#[derive(Debug, Clone, Default)]
pub struct MaskTable {
    light: HashMap<SymbolId, f32>,   // only nodes with light < 1.0; absent ⇒ 1.0
    pub deps: Deps,
    pub active: bool,                // any mask layer present (even if it dims nothing)
}
impl MaskTable {
    #[inline] pub fn light(&self, id: &SymbolId) -> f32 { self.light.get(id).copied().unwrap_or(1.0) }
    pub fn is_noop(&self) -> bool { self.light.is_empty() }
    pub fn dimmed_count(&self) -> usize;
}
```

### 4.2 Algorithm — one mask

Inputs: `target` (resolved `ResolvedSet.ids` = `S`), `strength s`, `ctx.tree`, `ctx.index`. Definitions over the whole tree (not just laid-out nodes — a container's light depends on descendants that may be culled):

1. **Ancestor-closed membership.** `member(n) = n ∈ S ∨ member(parent(n))`. Spotlighting a file lights its items; dimming a folder dims its files. Computed in one pre-order recursion over `tree.root` carrying an `inherited: bool`.
2. **Direct factor.**
   `DimExcept`: `f(n) = 1.0 if member(n) else 1 − s`.
   `Dim`:       `f(n) = 1 − s if member(n) else 1.0`.
3. **Container inheritance (upward, one level of halving, not recursive).**
   `desc_max(n) = max over all descendants d of f(d)` (post-order; `0` for leaves).
   `g(n) = max(f(n), 0.5 · desc_max(n))` when `n` has children, else `f(n)`.
   Consequence: spotlighting `fn verify` (f=1) gives `login.rs` g=0.5, `src/auth` g=0.5, root g=0.5 — every ancestor legible at half light, everything unrelated at `1−s`. Because `desc_max` takes the max over *all* descendants (not children's `g`), the halving does not compound up the tree. A container that is itself a member has `f=1 ⇒ g=1`.
4. Emit `light[n] = g(n)` for every `n` with `g(n) < 1.0`.

Reference implementation shape (one recursion does both passes: membership goes down, `desc_max` comes back up):
```rust
fn walk(n: &SymbolNode, inherited: bool, set: &HashSet<SymbolId>, dim_except: bool, s: f32,
        out: &mut HashMap<SymbolId, f32>) -> f32 /* max direct factor in this subtree incl. n */ {
    let member = inherited || set.contains(&n.id);
    let f = if member == dim_except { 1.0 } else { 1.0 - s };
    let mut desc_max = 0.0f32;
    for c in &n.children { desc_max = desc_max.max(walk(c, member, set, dim_except, s, out)); }
    let g = if n.children.is_empty() { f } else { f.max(0.5 * desc_max) };
    if g < 1.0 { out.insert(n.id.clone(), g); }
    f.max(desc_max)
}
```
(`member == dim_except` reads: for `DimExcept` members are lit; for `Dim` non-members are lit.)

This is a single recursion over `SymbolNode.children` (the resolver has `ctx.tree`; `TreeIndex` is used only for `parent` in tests and for `iter()` when materialising). Cost: **O(N)** time, `O(dimmed)` memory per mask; for a 200 k-node tree with a small spotlight that is ~200 k `SymbolId` clones (~10–20 MB transient, a few ms) — acceptable because it runs on resolve, never per frame (§7). If it shows up in profiles, key the map by a pre-order position (`TreeIndex` would need `pub fn position(&id) -> usize`; not required now).

### 4.3 Combining masks — AND

```rust
pub(crate) fn resolve_masks(layers: impl Iterator<Item=(&MaskSpec, &ResolvedSet)>, ctx: &ResolveCtx) -> MaskTable
```
Start from an empty table; for each mask compute its `light_m` map (§4.2) and multiply: `light[n] = Π_m g_m(n)`. Two spotlights on disjoint sets therefore dim everything (each lights only its own set; the product of `1` and `1−s` is `1−s`), which is the parent's AND rule ("a node is fully lit only if every active mask lights it"). `deps` = union of the target sets' deps ∪ `SPEC`; `active = any layer`.

Clamp: `light` is stored in `[0, 1]`; `strength 1.0` gives 0.0 (fully invisible bodies, fills at `theme::DIM`).

## 5. App integration

### 5.1 `paint_items` (`treemap.rs` ~1512–1513 and ~1682–1704)

```rust
let light = ov.light(&item.node.id);
…                                        // after the draw-mode match and after the fill-opacity multiply (03-fill.md §5.1)
body_opacity *= light;
tex_opacity  *= light;
out.push(PaintItem {
    fill:   theme::dim_toward(fill, light),
    border: theme::dim_toward(theme::border_for(base_fill), light),
    stripe: ov.stripe(&item.node.id).map(|c| theme::dim_toward(c, light)),
    light,
    body_opacity, tex_opacity, …
});
```
Order matters: `fill` here is already the fill-layer override (03-fill.md) — the mask dims the *presented* color, never the metric value.

`theme.rs`:
```rust
/// Target every dimmed color converges to. Sits between the old scrim (pure black) and BG so
/// dimmed boxes still separate from the window background.
pub const DIM: u32 = 0x0c0c0e;
pub fn dim_toward(color: u32, light: f32) -> u32 { lerp_rgb(color, DIM, 1.0 - light.clamp(0.0, 1.0)) }
/// 0xRRGGBB + alpha → 0xRRGGBBAA for `gpui::rgba` (used for rings).
pub fn with_alpha(rgb: u32, a: f32) -> u32 { (rgb << 8) | ((a.clamp(0.0,1.0) * 255.0).round() as u32) }
```
Add `DIM` to `fingerprint()`.

### 5.2 Canvas closure (`treemap.rs` ~4307–4604)

- **Name row** (4389–4403): `&[run(n.text.len(), theme::TEXT_PRIMARY)]` → build the run then `r.color = r.color.opacity(item.light)` (same pattern the body already uses at 4431–4433). Body rows need no change: `body_opacity` carries light.
- **Textures** (4352–4386): after the existing `CODE_BG` fade quad, add
  ```rust
  if item.light < 1.0 {
      let dc = rgb(theme::DIM).opacity(1.0 - item.light);
      window.paint_quad(quad(tb, px(0.), dc, px(0.), dc, BorderStyle::default()));
  }
  ```
  inside the same `with_content_mask(Some(ContentMask { bounds: b }))`, so a dimmed folder thumbnail (which was baked at full brightness, `rasterize.rs:245+`) reads as dimmed exactly like a live box would. This is the same trick as the tex fade at 4372.
- **Scrim** (4499–4508): delete the block and the `cg_scrim` capture. `paint_items` still returns a bool, renamed `in_call_graph`, consumed only by pass 4 (doc panel skip) until 07-notes.md moves the doc panel to Notes.
- **Rings** (4509–4527): focus ring `(2.0, rgba(theme::with_alpha(theme::FOCUS_BORDER, item.light)))`; neighbor ring `(1.0, rgba(theme::scale_alpha(theme::NEIGHBOR_BORDER, item.light)))` — `NEIGHBOR_BORDER` is already RGBA `0xffffff80`, so only its alpha byte is scaled. Add `pub fn scale_alpha(rgba: u32, k: f32) -> u32 { (rgba & !0xff) | (((rgba & 0xff) as f32 * k.clamp(0.0,1.0)).round() as u32) }` to `theme.rs`.
- **Pass 2c** (4528–4541): remove `if !cg_scrim`. **Decision:** neighbor rings are dimmed like any other paint, not skipped. Under a `strength 0.8` spotlight a neighbor ring is `0x80 × 0.2 ≈ 0x1a` alpha — effectively invisible, which matches the old skip visually, but stays consistent (a `strength 0.4` mask keeps faint rings, which is what "dim" means). If the call-graph migration wants them gone entirely it pops the `neighbors` marks layer (06-marks.md) — a layer decision, not a special case in the canvas.
- **Pass 2d / 3** (deferred focused leaf + focus ring): unchanged. The focused leaf's `light` is 1.0 under `dimExcept focusSet`, and it is painted after everything else — so it stays crisp without any global quad. Under a `dim` mask that includes the focus, the focused leaf is dimmed too (correct: the user asked to dim it).
- **Doc panel** (4560): `doc_panel.as_ref().filter(|_| !in_call_graph)`.

**Why per-item beats the scrim.** The scrim dims everything below it including edges (05), marks (06), and notes (07) drawn later — or forces them above the scrim, where they can't be dimmed selectively. Per-item light lets the focused node *and its edges/callers* stay crisp while unrelated boxes recede, gives containers the 0.5 legibility rule (impossible with one quad), and lets `strength` be a real parameter. It also removes a special-cased pass ordering (`!cg_scrim` guards) from the canvas.

### 5.3 `view/paint_resolver.rs`

```rust
impl PaintOverrides<'_> {
    #[inline] pub fn light(&self, id: &SymbolId) -> f32 { self.resolved.mask.light(id) }
}
```

### 5.4 Call-graph mode pushes the mask (`treemap.rs` `enter_call_graph` 2430, `exit_call_graph` 2511)

```rust
fn cg_mask_layer() -> LayerSpec { LayerSpec::Mask(MaskSpec { dim_except: Some(SetRef::Name("focusSet".into())), dim: None, strength: 0.8 }) }
fn cg_mask_index(&self) -> Option<usize> { self.view_spec.layers.iter().position(|l| matches!(l, LayerSpec::Mask(m) if m.dim_except == Some(SetRef::Name("focusSet".into())))) }
// enter_call_graph, in both branches where `self.call_graph = Some(..)` is set:
if self.cg_mask_index().is_none() { self.apply_view_command(ViewCommand::PushLayer(cg_mask_layer())); }
// exit_call_graph, after `self.call_graph.take()`:
if let Some(i) = self.cg_mask_index() { self.apply_view_command(ViewCommand::RemoveLayer(i)); }
```
`focusSet = ids(["$focus"])` is in the session default view (framework §3.3), so the mask follows the focus for free — but the call-graph columns keep their own `center`; when milestone 4 replaces them with `Mask + Edges + Panel` the same layer stays and the columns' code goes. Also remove the mask in every path that clears `call_graph` outside `exit_call_graph` (`self.call_graph = None` at ~2802 during reload) — put both in a `fn clear_call_graph_mode(&mut self)`.

Golden equivalence (§9): old look = every earlier paint composited under black at α 0.8 ⇒ `c·0.2`; new look = `lerp(c, 0x0c0c0e, 0.8) = c·0.2 + 0x0a` per channel (rounding aside), so within 10/255 per channel; text alpha 0.2 in both.

### 5.5 Files
`crates/outrider-view/src/layers/mask.rs` (new), `crates/outrider/src/view/paint_resolver.rs` (`light`), `theme.rs` additions. No new app files.

## 6. Commands

| driver | ViewCommand | notes |
|---|---|---|
| `outrider mask --dim-except <set> \| --dim <set> [--strength 0.8]` | `PushLayer(LayerSpec::Mask(..))` | appends; masks AND, so pushing a second spotlight narrows further. `--replace` (CLI sugar) = `RemoveLayer(last mask index)` + `PushLayer`. |
| `outrider layer rm <i>` / `pop` / `view clear --layers` | `RemoveLayer` / `PopLayer` / `Clear(Layers)` | |
| Tab (enter call-graph mode) / Esc (exit) | `PushLayer(cg_mask_layer())` / `RemoveLayer(i)` via `apply_view_command` (§5.4) | |
| tour steps (09-camera.md) | `push: [{mask: …}]`, `pop: n` | the intended way to spotlight per step |

No `ViewCommand` is added; `mask` verbs are `patch` sugar (parent §6.3).

## 7. Invalidation

| Deps | who sets | effect |
|---|---|---|
| target set deps (`FOCUS` for `focusSet`, `SELECTION`, `GIT` for `changed(..)`, …) | framework §3.4 sites | `MaskTable` recomputed (O(N)) when the set changes — for `focusSet` that is every focus move; fine at a few ms, but see §10.1 |
| `TREE` | loader / packing | full recompute |
| `SPEC` | any `apply` | recompute (mask layers changed) |

Not `CAMERA`, not `HOVER`. `PaintOverrides::light` is one `HashMap` probe per `DrawItem` per frame — no allocation, no tree walk (§4.2 precomputes everything).

## 8. Migration steps

1. **Types + validation:** `MaskSpec`, `MaskTarget`, rules in §3. Round-trip tests.
2. **`layers/mask.rs`:** `MaskTable`, `resolve_masks` (§4.2–4.3); wire into `ViewResolver::resolve` step (3). Unit tests (§9) on `mini_repo`.
3. **theme.rs:** `DIM`, `dim_toward`, `with_alpha`, `scale_alpha`; fingerprint. Tests.
4. **paint_resolver.rs:** `light`.
5. **paint_items:** `light` multiply, `dim_toward` on fill/border/stripe, `PaintItem.light`. Behaviour-preserving: with no mask layer every light is 1.0 and `dim_toward(c, 1.0) == c`.
6. **Canvas:** name-row alpha, texture dim quad, ring alpha. Still no-op without masks.
7. **Call-graph:** `cg_mask_layer` push/remove in `enter_/exit_call_graph` + `clear_call_graph_mode`; **delete** the scrim quad (4499–4508) and the `!cg_scrim` guard on pass 2c; rename the returned bool to `in_call_graph` (doc-panel skip only). This is the one visible change and it is covered by the golden tolerance test.
8. Later (milestone 4, 05/08): `CallGraphMode` columns → Panel, `in_call_graph` bool → gone once the doc panel is a Note (07).

## 9. Tests

**`outrider-view` (fixture `mini_repo`, `index_repo`, layout via `outrider_layout::pack`):**
- `mask_dim_except_lights_set_and_ancestors_at_half` — `dimExcept ids([fn:src/lib.rs::Point::new])`, s=0.8: `light(new)=1.0`; `light(impl Point)=0.5`, `light(src/lib.rs)=0.5`, `light(src)=0.5`, `light(root)=0.5`; `light(fn free)=0.2` (approx, f32); `light(src/util.rs)=0.2`; sibling method `norm`=0.2.
- `mask_membership_is_ancestor_closed` — `dimExcept ids([file:src/lib.rs])`: every item in `lib.rs` is 1.0; `util.rs` 0.2. `dim ids([folder:src])`: `lib.rs`, its items, `util.rs` all 0.2; `README.md` 1.0; root 1.0 (has a lit descendant with f=1 ⇒ g=1).
- `mask_and_semantics` — masks A=`dimExcept {file:src/lib.rs}` s=0.5 and B=`dimExcept {file:src/util.rs}` s=0.5. Per mask: the lit file 1.0, the other file 0.5, `README.md` 0.5, `src` and root `max(0.5, 0.5·1.0) = 0.5`. Products: `lib.rs` 0.5, `util.rs` 0.5, `README.md` 0.25, `src` 0.25, root 0.25, items of `lib.rs` 0.5. Also assert order independence (B then A gives the same table).
- `mask_strength_zero_is_noop` — `is_noop()` true, `active` true, `light` 1.0 everywhere.
- `mask_strength_one_reaches_zero` — dimmed nodes 0.0, containers of lit nodes 0.5.
- `mask_no_layer_absent` — `ResolvedView.mask.active == false`, `light == 1.0`.
- `validate_mask_requires_exactly_one_target`, `validate_mask_strength_range`.
- `mask_deps_follow_target_set` — `dimExcept focusSet` ⇒ `deps ∋ FOCUS`; recomputed on `dirty=FOCUS`, not on `dirty=CAMERA`.

**App crate:**
- `dim_toward_endpoints` — `dim_toward(c,1.0)==c`, `dim_toward(c,0.0)==DIM`, monotone per channel.
- `paint_decisions_mask` (extends the framework golden helper): resolved view with `dimExcept focusSet` s=0.8 → focused item `light 1.0`, its file 0.5, unrelated 0.2; `body_opacity`/`tex_opacity` multiplied; `fill == dim_toward(base_or_override, light)`, `border == dim_toward(border_for(base), light)`.
- **Golden vs. scrim:** for the same fixture and a fixed camera, capture per-item `(fill, border, stripe)` produced by the *old* path composited under `0x000000cc` (compute `c·0.2` per channel in the test) and compare with the new mask path: max per-channel delta ≤ 12/255 for every dimmed item; focused leaf identical; neighbor ring alpha ≤ 0x20.
- `scrim_quad_gone` — grep-level guard: no `rgba(0x000000cc)` in `treemap.rs` (or simply the compile: `cg_scrim` identifier removed).

**Manual acceptance:** Tab on a fn — everything but the focused leaf and its ancestor headers dims to near-black; ancestor headers legible at half; Esc restores; pushing `{"mask":{"dimExcept":{"glob":"src/auth/**"}}}` via a view file lights the folder, its files, and their items, and thumbnails of other folders are dimmed by the overlay quad; `strength 0.3` shows faint neighbor rings.

## 10. Open questions / risks

1. **Recompute per focus move.** `dimExcept focusSet` recomputes O(N) on every arrow key. Fine to ~200 k nodes; beyond that, cache `desc_max` per mask target hash and only redo the pre-order pass, or key the table by pre-order index. Measure before optimising.
2. **Rounding vs. the old scrim.** `DIM = 0x0c0c0e` is a taste decision; pure black (`0x000000`) reproduces the scrim exactly but makes dimmed boxes vanish into `BG`. Tolerance in the golden is set at 12/255 to allow either.
3. **Textures baked with stripes/fills.** The overlay quad dims the whole thumbnail uniformly; children inside a dimmed folder that are *lit* (a spotlight on one method inside a Card-rung folder) cannot be shown lit inside the texture — the folder is at 0.5 (inheritance) so the user sees "something in here is lit" and zooming in reveals it. Acceptable; a per-child bake would need the mask hash in the texture key.
4. **Notes/edges on dimmed nodes.** 05/07 must multiply their alpha by `light` of their anchor symbol; edges use `min(light(from), light(to))`. Stated here so the rule is one place.
5. **`Default for MaskSpec` is invalid** (no target). Framework wants `Default` on every spec type; validation catches it, and `ViewSpec::default()` has no layers so it stays valid.

## 11. Framework deltas

- **None** to types or commands. `MaskTable` (00-framework `ResolvedView.mask`) is defined here as `{ light: HashMap<SymbolId,f32>, deps, active }` with `light(&id) -> f32`.
- **Additive theme API** (app crate, not framework): `DIM`, `dim_toward`, `with_alpha`, `scale_alpha`.
- `paint_items`' third return value is kept but renamed `in_call_graph` and narrowed to the doc-panel skip; it disappears with 07-notes.md.
