# 01 — Space: geometry, exclusion, regroup

**Parent:** [../view-primitives.md](../view-primitives.md) §2.1, §2.10, §8.1
**Depends on:** [00-framework.md](00-framework.md) (crate, `ViewSpec`, `Deps`, `ResolveCtx`, `apply`), [02-set.md](02-set.md) (`SetExpr`, `SetResolver`, `PartitionRegistry`, `PartitionRef`)
**Deliverable:** `SpaceSpec` in `spec.rs`; `outrider-view/src/space.rs` (space planning: exclusion + regroup → the tree that is packed, and the space id); an app-side repack path (`ProjectLoader::start_repack`) that swaps geometry through the existing `LayoutTransition` machinery; validation that exclusion sets are static. After this spec lands nothing looks different until a view document sets `space.exclude`, `space.regroup`, or `space.pack`.

---

## 1. Purpose and scope

The Space is the one primitive that owns geometry (parent §1.2, §2.1). This spec covers:

- `SpaceSpec { kind, regroup, exclude, pack }` — the document form (parent §5 `"space"`).
- **Layout-time exclusion**: symbols in `exclude` get no `Rect`. Index-time filters (`filter_extensions/folders/files`, `settings.rs`) are *unchanged* and stay in `ProjectLoader::start`; exclusion is the additional, view-driven mechanism an agent uses (parent §4 row 2).
- **Regroup**: pack the same leaves under a partition instead of folders. Space id becomes `treemap@<partition>` (parent §2.1 table, §10 q4).
- The `callgraph` / `matrix` kinds: parse and validate today, resolve to "unsupported" (parent §9 milestone 8).
- What the Space explicitly does **not** own: fidelity is computed by `world::rung_for` / `world::leaf_draw` and stays read-only for layers; the focused-leaf widening (`focused_width`, `deferred_overlay`) is the parent §2.10 exception and is not expressible in `SpaceSpec`.

Out of scope: edges/marks/etc. reading geometry (they read `PackLayout` through `ResolveCtx.layout`, nothing here changes that), the `callgraph`/`matrix` layouts themselves.

---

## 2. Ground truth: existing code touched

| File | Symbol | ~Line | What it does today | What changes |
|---|---|---|---|---|
| `crates/outrider-layout/src/pack.rs` | `pack(tree, cfg) -> PackLayout`, `PackConfig { page_w, line_step, header, container_header, bottom_pad, gap, aspect, max_display_lines }` | 27–70 | Pure bottom-up pack over `tree.root`. Every child loop indexes `layouts[&child.id]` / `child_sizes[&child.id]` (L108, L159, L276). | **Unchanged.** Exclusion is done by pruning the tree before calling it (§4.2). |
| `crates/outrider-layout/src/progressive.rs` | `pack_progressive(tree, cfg, max_snapshots, is_cancelled, emit)` | 21–115 | Same algorithm with draft snapshots; also indexes `exact[&child.id]` (L71). | Unchanged; receives the pruned/regrouped tree. |
| `crates/outrider-layout/src/zones.rs` | `build_profiles`, `effective_role` | 219–300 | Role zones keyed by `SymbolId`, names classified per node. | Unchanged; synthetic group folders get the default `Source/weak` profile from their name. |
| `crates/outrider/src/world.rs` | `pack_config(gap, max_display_lines)` | 34–45 | Builds `PackConfig` from `settings.node_padding` / `settings.max_display_lines`. | Called with the *effective* values after `PackOverrides` are applied (§4.4). |
| `crates/outrider/src/world.rs` | `walk` in `visible_nodes` | 207–209 | `let Some(r) = pack.rects.get(&node.id) else { return; }` — a node without a rect prunes its whole subtree. | Unchanged. This is what makes "no rect" == "not laid out" free for the renderer. |
| `crates/outrider/src/treemap.rs` | `TreemapView { tree, layout, layout_transition, packing_target_layout, settings, loader, .. }` | 512–567 | Owns geometry. | Add `space_key: SpaceKey` (§5.1). |
| `crates/outrider/src/treemap.rs` | `PackingGeometryState::{apply_snapshot, finish, fail_after_preview}` | 569–615 | Retargets/creates a `LayoutTransition` toward each packing snapshot; `invalidate()` clears camera/neighbors/hover. | `apply_snapshot` uses `LayoutTransition::bridged` when key sets differ (§5.3); both set `view_dirty |= TREE` (00-framework §3.4). |
| `crates/outrider/src/treemap.rs` | `paint_items` | 1433–1807 | Builds `PaintItem`s from `visible_nodes`. | No change for this spec (fidelity stays here). |
| `crates/outrider/src/treemap.rs` | `reindex`, `merge_project_settings`, `hide_folder/file/extension` | 2360–2428 | Filters → `ProjectSettings` → full re-index. | Unchanged (index-time filters stay). |
| `crates/outrider/src/treemap.rs` | settings-panel save handler | 3987–3999 | Any settings change → `reindex()`. | Optional: `node_padding` / `max_display_lines`-only changes go through `start_repack` (§8 step 7). |
| `crates/outrider/src/treemap.rs` | `start_loading`, `poll_loading`, `install_project_preview`, `apply_packing_snapshot`, `finish_packing`, `fail_packing` | 2787–2974 | Loader lifecycle. | `poll_loading` also drives repack results; `install_project_preview` builds the space (§5.2). |
| `crates/outrider/src/project_loader.rs` | `ProjectLoader::{start, start_worker, poll}`, `load_project_cancellable` | 193–345, 456–555 | Index → `pack_progressive` on a worker thread; snapshots gated on `preview_delivered`. | Add `start_repack` (§5.2); `load_project_cancellable` packs the *space tree* not the raw tree. |
| `crates/outrider/src/layout_transition.rs` | `LayoutTransition::{new, sample, retarget}` | 7–62 | `sample` **snaps to `to`** when `from.rects.keys() != to.rects.keys()` (L23). | Add `bridged(from, to, now)` (§5.3). |
| `crates/outrider/src/focus.rs` | `Focus::step_in`, `spatial_step`, `neighbors` | 96–220 | `step_in` picks a child from `node.children` (tree); `spatial_step` iterates `pack.rects` (layout). | `step_in` must skip children without a rect (§5.4). |
| `crates/outrider/src/settings.rs` | `Settings { filter_*, node_padding, max_display_lines, .. }` | 35–51 | Persistent knobs. | Unchanged; `PackOverrides` layer on top per view. |
| `crates/outrider/src/rasterize.rs` | `bake_container` | 247 | `layout.rects.get(&child.id)` — tolerates missing rects. | Unchanged. |

Every consumer of `PackLayout` in the app already uses `.get` (world.rs:207, focus.rs:158, rasterize.rs:247, treemap.rs:1079/1323/1768/3231); only `LayoutTransition::sample` indexes (`self.to.rects[id]`, L40) and it guards with the key-set check first. So a layout that simply lacks rects for excluded nodes is safe everywhere.

---

## 3. Spec types

In `crates/outrider-view/src/spec.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceSpec {
    #[serde(default)] pub kind: SpaceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub regroup: Option<PartitionRef>,   // 02-set.md §3.4
    #[serde(default, skip_serializing_if = "Option::is_none")] pub exclude: Option<SetRef>,        // 00-framework §2.2
    #[serde(default)] pub pack: PackOverrides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SpaceKind { #[default] Treemap, Callgraph, Matrix }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackOverrides {
    /// World-px sibling gap; `None` → `settings.node_padding`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub gap: Option<f64>,
    /// Cap on the measure used for sizing. Absent/`null` → inherit `settings.max_display_lines`;
    /// a number → that cap; `"none"` → force no cap even if settings has one. `0` is invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub max_display_lines: Option<LineCap>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LineCap { Lines(u64), None(NoCap) }               // `NoCap` is a unit enum serialising as "none"
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")] pub enum NoCap { None }
```

JSON (parent §5 form, all fields optional):

```jsonc
"space": { "kind": "treemap" }                                            // == default
"space": { "kind": "treemap", "exclude": "hidden", "pack": { "gap": 8 } }
"space": { "kind": "treemap", "exclude": { "union": [ { "glob": "**/*.lock" }, { "glob": "target/**" } ] } }
"space": { "kind": "treemap", "regroup": { "partition": "communities" }, "pack": { "maxDisplayLines": 400 } }
"space": { "kind": "matrix" }                                             // validates; resolves Unsupported (§4.5)
```

`PartitionRef` (defined in 02-set.md §3.4, owned by `spec.rs`): `#[serde(deny_unknown_fields)] pub struct PartitionRef { pub partition: String }`.

Validation rules added to `validate.rs` (00-framework §2.6):

| rule | severity | message |
|---|---|---|
| `space.exclude` refers to a static set (§4.6): `static_deps(expr) ∩ (FOCUS|HOVER|CAMERA|SELECTION) == ∅` | hard | `"space.exclude: exclusion sets must be static; '<name>' depends on <bits>"` |
| `space.exclude` depends on `GIT` or `METRICS` | soft | `"space.exclude: resolved once at pack time; changes to <bit> do not repack"` |
| `space.pack.gap` outside `0.0..=64.0` (mirrors `SettingsDraft::apply_to`, treemap.rs:131) | hard | `"space.pack.gap must be within 0–64"` |
| `space.pack.maxDisplayLines == 0` | hard | `"space.pack.maxDisplayLines must be ≥ 1 or \"none\""` |
| `space.regroup` names a partition not in the registry | soft (resolve-time warning; validate has no registry) | `"space.regroup: unknown partition '<p>'; using folders"` |
| `space.kind` is `callgraph`/`matrix` | soft | `"space.kind '<k>' is not implemented; treemap is used"` |

---

## 4. Resolution

New module `crates/outrider-view/src/space.rs`. The Space is *not* resolved by `ViewResolver::resolve` (that produces paint tables per frame); it is resolved on the loader thread when geometry is (re)built, because its output is the tree handed to `pack_progressive`.

### 4.1 Types

```rust
/// Everything the packer needs, derived from a ViewSpec + tree. Pure; no GPUI.
pub struct SpacePlan<'a> {
    /// "treemap" | "treemap@<partition>"; the continuity domain (parent §2.1).
    pub space_id: String,
    /// The tree to pack. Same object as `source` when nothing is excluded/regrouped
    /// (Cow avoids the clone in the default case).
    pub tree: std::borrow::Cow<'a, SymbolTree>,
    /// Ids that were excluded (subtree roots and all their descendants), for warnings/`query`.
    pub excluded: HashSet<SymbolId>,
    pub pack: EffectivePack,           // gap, max_display_lines after overrides
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub struct EffectivePack { pub gap: f64, pub max_display_lines: Option<u64> }

/// Cheap identity of a plan; the app repacks iff this changes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SpaceKey { pub space_id: String, pub exclude_hash: u64, pub partition_version: u64, pub gap_bits: u64, pub max_display_lines: Option<u64> }

pub enum SpaceOutcome<'a> { Ready(SpacePlan<'a>), Unsupported { kind: SpaceKind } }

pub fn plan_space<'a>(
    spec: &SpaceSpec,
    sets: &BTreeMap<String, SetExpr>,
    tree: &'a SymbolTree,
    partitions: &PartitionRegistry,        // 02-set.md §4.7
    defaults: EffectivePack,               // from Settings
    repo_root: &Path,
) -> SpaceOutcome<'a>;
pub fn space_key(spec: &SpaceSpec, sets: &BTreeMap<String, SetExpr>, partitions: &PartitionRegistry, defaults: EffectivePack) -> SpaceKey;
```

`plan_space` builds a throw-away `ResolveCtx`-less `SetResolver` (02-set.md §4.3 exposes `SetResolver::static_only(tree, index, repo_root)`; live leaves resolve empty with a warning — validation already forbids them here) and does, in order: exclusion (4.2), regroup (4.3), pack overrides (4.4).

### 4.2 Exclusion: filtered tree, not a packer predicate

Two options were evaluated against the real packer:

**(a) Predicate on `PackConfig`** (`exclude: Option<&dyn Fn(&SymbolId) -> bool>`, as parent §8.1 suggests). Every child loop in the packer would have to skip excluded children *consistently*: `pack.rs` `build_exact_layouts::build` (L97, L105), `exact_local_layout_cancellable` (L150), `absolute_from_layouts::absolute` (L275); `progressive.rs` `count_nodes` (L118), `postorder_with_roles` (L135), `draft_local_layout_cancellable` (L192), `build_draft_layouts::build` (L227, L232), `materialize_hybrid::build` (L266, L274), `absolute_from_layouts_cancellable` (L319); `zones.rs` `collect_profiles`. Eleven sites, each followed by an indexing `[&child.id]` that panics if any one is missed. `PackConfig` is `Copy` today; a `&dyn Fn` field breaks that and forces a lifetime through `pack_progressive`'s worker closure. Saves one tree clone.

**(b) Prune a copy of the tree** and pack it unchanged. One function, no packer change, no panic surface, `pack()` stays a pure function of its input (which is what makes "geometry depends only on tree + config + exclusion" checkable — the *pruned tree* is that input). Cost: an O(n) `SymbolNode` clone; the loader already clones the whole tree for the preview (`project_loader.rs:520 tree.clone()`), and the pruned tree is dropped as soon as the layout exists.

**Decision: (b).** Implement in `space.rs`:

```rust
/// Remove every node in `excluded` and its subtree. Folders left with no children are removed
/// too (a folder whose files were all excluded should not pack as an empty box); the root is
/// never removed. Files with no remaining items stay (they are leaves with a byte_range).
pub fn prune_tree(tree: &SymbolTree, excluded: &HashSet<SymbolId>) -> SymbolTree;
```

`excluded` is the resolved exclusion set **closed under descendants** (`SetResolver` returns exact members; `plan_space` adds descendants via `TreeIndex`). Nothing else in the app needs to know: `visible_nodes` skips rect-less nodes (world.rs:207), `spatial_step` only sees rects (focus.rs:164), textures use `.get`. The full `self.tree` stays in `TreemapView` — palette, buffers, `TreeIndex`, sets all still see excluded symbols (a set may name them; a mask over them is a no-op; `not` is relative to `layout.rects` per 02-set.md §4.5).

Index-time filters (`Settings.filter_*`, applied in `discover_files`, scan.rs:83–150) are untouched. `default_view` does **not** mirror them into `space.exclude` (00-framework §3.3).

### 4.3 Regroup: synthetic tree, original leaf ids

```rust
/// Rebuild the hierarchy: root → one Folder per partition group → the *original file subtrees*
/// (cloned, ids untouched) of that group's members. Files not assigned to any group go under
/// "@<partition>/(unassigned)". Original folder nodes disappear from the packed tree.
pub fn regroup_tree(tree: &SymbolTree, partition: &Partition) -> SymbolTree;
```

- Group folder id: `SymbolId { kind: Folder, qualified_path: format!("@{}/{}", partition.name, group), ordinal: 0 }`; `name = group`; `measure = Σ member measure`; `churn = 0`, `doc = None`. Group order for packing is decided by the packer (role, height, name) — same as real folders. The `@` prefix cannot collide with a real path (paths never start with `@`) and makes the id recognisable to `PaintOverrides`/`query`.
- Members: a partition maps `SymbolId → group` (02-set.md §4.7). Regroup uses **file-level** membership: for each `File` node in the tree, `partition.group_of(file_id)`; if a partition assigns items rather than files, the file's group is the majority group of its items (ties → first by name), and a warning lists split files. Folders in the partition map are ignored for regroup.
- The root keeps its original id (so `Focus::new(root_id)` and `nav_history` are unaffected). Root `children` = group folders sorted by name (`finalize_children` is *not* rerun — group names are unique by construction).
- Every original **leaf** id (files, items, chunks) is present in the regrouped tree exactly once → every layer keyed on symbols keeps working; only original `Folder` ids lose their rects (they exist in `self.tree` but not in `layout`).

Regroup + exclusion compose: prune first, then regroup (an excluded file never reaches a group; an empty group is dropped).

### 4.4 Effective pack config

`EffectivePack { gap: spec.pack.gap.unwrap_or(settings.node_padding), max_display_lines: match spec.pack.max_display_lines { None => settings.max_display_lines, Some(Lines(n)) => Some(n), Some(NoCap) => None } }` → `world::pack_config(gap, max_display_lines)` (world.rs:34).

### 4.5 `callgraph` / `matrix`

`plan_space` returns `SpaceOutcome::Unsupported { kind }`; the app keeps the current layout and pushes `Notification::warning("space kind 'matrix' is not implemented; keeping treemap")`. `space_key` for an unsupported kind equals the treemap key so no repack is triggered. The enum variants exist so saved documents from later milestones validate today (parent §8.1 "Commands").

### 4.6 Why exclusion must be static

Exclusion changes geometry; geometry changes trigger a repack (worker thread, `LayoutTransition`, camera reset in `PackingGeometryState::invalidate`). A FOCUS-dependent exclusion would repack on every arrow key — violating determinism/continuity (parent §1.3, design doc §9.1–2) and thrashing the loader. Hence the hard rule in §3: `SetResolver::static_deps(&expr, &sets)` (02-set.md §4.2) must not intersect `FOCUS|HOVER|CAMERA|SELECTION`. `visible()` implies CAMERA and is thereby forbidden. `neighbors`, `ids(["$focus"])` etc. are forbidden. `GIT`/`METRICS`-dependent exclusions (`changed(HEAD~1)`, `where(churn > p90)`) are allowed and **frozen at pack time**: the plan resolves them once; a later GIT tick does not repack (soft warning). Re-applying the view (`view apply/patch` touching `space`) re-plans.

### 4.7 Fidelity and the focus exception

`Rung`/`LeafDraw` (world.rs:49–120) are computed from on-screen size only. Layers receive them via `DrawItem.draw` (read-only) — 05-edges uses it for aggregation, 06-marks for glyph visibility. Nothing in `SpaceSpec` alters thresholds. `focused_width`/`deferred_overlay` (treemap.rs:911, 1588, 1694) remain the only geometry override, apply only to `focus.current`, and never enter `PackLayout` (parent §2.10).

---

## 5. App integration

### 5.1 New state on `TreemapView`

```rust
/// Identity of the geometry currently installed (or being packed). Compared after every
/// apply_view_command; mismatch → start_repack.
space_key: outrider_view::space::SpaceKey,
/// Registry consumed by regroup and by community()/layer() sets (02-set.md §4.7).
partitions: outrider_view::PartitionRegistry,
```

Initialise `space_key` from `default_view` + settings in `from_parts`; `partitions = PartitionRegistry::builtin(&tree)` (contains only `"topFolder"` until real providers exist) in the loader completion path next to `metrics`/`relations` (00-framework §3.5).

### 5.2 Loader: plan once at load, and a repack path

`load_project_cancellable` (project_loader.rs:456) currently packs `tree`. Change: after indexing, `let plan = plan_space(&spec.space, &spec.sets, &tree, &partitions, defaults, project_root)`; on `Ready(plan)` call `pack_progressive(&plan.tree, &pack_config(plan.pack.gap, plan.pack.max_display_lines), …)`; the `ProjectPreview` still carries the **full** `tree` (the app keeps it) plus `space_id: String` and `excluded: HashSet<SymbolId>` (new fields). `ProjectLoader::start(project_root, settings)` gains a third argument `space: SpaceInputs { space: SpaceSpec, sets: BTreeMap<String,SetExpr>, partitions: PartitionRegistry /* Clone, cheap while builtin */ }` built by `TreemapView::start_loading` from `self.view_spec`.

New:
```rust
impl ProjectLoader {
    /// Repack an already-indexed tree under a new SpacePlan. No indexing, no Preview event:
    /// the LoadingState starts with `preview_delivered: true` so snapshots flow immediately
    /// (poll(), L324). Emits PackingStarted → Snapshot* → Complete exactly like a load.
    pub fn start_repack(&mut self, tree: SymbolTree /* clone of self.tree */, inputs: SpaceInputs, settings: Settings) -> u64;
}
```
Implementation reuses `start_worker` with a closure that runs the `plan_space` + `pack_progressive` tail of `load_project_cancellable` (factor that tail into `fn pack_space_cancellable(tree: &SymbolTree, inputs, settings, worker) -> Result<PackLayout, WorkerError>`). `Complete`/`Snapshot` events are unchanged so `poll_loading` (treemap.rs:2840) needs no new arm; the `Complete` handler additionally stores `space_key`/`space_id` from the generation's inputs (keep them in `LoadingState`, return them in `LoaderPoll::Complete { .., space: Option<SpaceResult> }`).

Trigger: in `apply_view_command` (00-framework §3.6), after `apply` succeeds:
```rust
let key = space_key(&self.view_spec.space, &self.view_spec.sets, &self.partitions, self.pack_defaults());
if key != self.space_key && !self.loader.is_loading() {   // if loading, start_loading already uses the new spec
    self.space_key = key.clone();
    self.loader.start_repack(self.tree.clone(), self.space_inputs(), self.settings.clone());
    self.load_progress = None;
}
```
While a repack runs, `map_interaction_enabled()` is false (treemap.rs:2835) exactly as during a load — acceptable (repack of a large repo is well under the index time; the progressive snapshots keep the map live).

### 5.3 `LayoutTransition` across a key-set change

`sample()` snaps when key sets differ (layout_transition.rs:23), so an exclusion or regroup would pop instead of tween. Add:

```rust
impl LayoutTransition {
    /// `from` restricted/extended to `to`'s key set: ids only in `from` are dropped (they vanish
    /// immediately — the excluded nodes), ids only in `to` start at their target rect (new group
    /// folders appear in place). Then a normal tween runs for the survivors.
    pub(crate) fn bridged(from: PackLayout, to: PackLayout, now: Instant) -> Self;
}
```
`PackingGeometryState::apply_snapshot` (treemap.rs:586): use `LayoutTransition::bridged` when `self.layout.rects.keys().ne(target.rects.keys())` (both the fresh-transition branch and `retarget`, which should call `bridged` internally). Also set `view_dirty |= TREE` in `apply_snapshot`, `finish`, `fail_after_preview` (00-framework §3.4 already lists this) — the resolver's `not`, `visible`, and any layout-reading layer must recompute.

### 5.4 Focus and the laid-out set

- After `apply_snapshot`/`finish` (i.e. inside `apply_packing_snapshot`/`finish_packing`, treemap.rs:2948–2960): if `!self.layout.rects.contains_key(&self.focus.current)`, walk `TreeIndex::parent` until a laid-out ancestor (root always is) and `self.focus.set(that, &index)`; set `view_dirty |= FOCUS`.
- `Focus::step_in(&mut self, index)` (focus.rs:97) → `step_in(&mut self, index, layout: &PackLayout)`; candidate children filtered by `layout.rects.contains_key(&c.id)`; `last_child` validity check likewise. Three call sites in `on_key_down` (grep `step_in(`).
- Palette results / `Camera(Focus(id))` commands targeting an excluded id: `frame_focus` returns `None` (treemap.rs:1323, `.get`) so nothing moves; add a `Notification::warning("<id> is excluded by the current view")` in the palette confirm path. Under regroup, original folder ids are not laid out; the palette's File/Symbol modes never list folders, so only `Camera` commands can hit this — same warning.

### 5.5 Files

- `crates/outrider-view/src/space.rs` — §4 (new; add `pub mod space;` to `lib.rs`).
- `crates/outrider/src/project_loader.rs` — `SpaceInputs`, `start_repack`, `pack_space_cancellable`, `ProjectPreview.{space_id, excluded}`.
- `crates/outrider/src/layout_transition.rs` — `bridged`.
- `crates/outrider/src/treemap.rs` — fields (5.1), trigger (5.2), focus reconcile (5.4), notification on unsupported kind (4.5).
- `crates/outrider/src/focus.rs` — `step_in` signature.
- No new file under `crates/outrider/src/view/` for this spec.

---

## 6. Commands

- `ViewCommand::Apply(spec)` / `Patch(ViewPatch { space: Some(..), .. })` are the only ways to change the space (00-framework §2.5). `apply` treats `space` like any other field; the repack decision is the app's (§5.2) by comparing `SpaceKey`, so an `Apply` that restates the same space is a no-op geometrically (idempotent, parent §6.3).
- CLI (parent §6.2): `outrider view apply|patch <file.json>` carrying `"space"`. There is no dedicated `space` verb in §6.2; 11-cli may add sugar `outrider space [--exclude <set>] [--regroup <partition>] [--gap n] [--max-lines n|none]` compiling to a `ViewPatch { space }`. `outrider set hidden --glob '…'` then `outrider view patch '{"space":{"exclude":"hidden"}}'` is the walking-skeleton form.
- Query: `outrider query view` shows the space; `outrider query set <name>` on an exclusion set lists members; add to 11-cli `query space` → `{ spaceId, excludedCount, pack }` (reads `SpacePlan` fields the app stores from the last `Complete`).

---

## 7. Invalidation

| event | bits | who |
|---|---|---|
| repack snapshot / finish / fail-after-preview | `TREE` (+ `view_resolver.invalidate_all()` on finish) | `PackingGeometryState` (treemap.rs:586–615) |
| focus reconciled after repack (§5.4) | `FOCUS` | `apply_packing_snapshot`/`finish_packing` |
| `apply_view_command` changed `space` | `SPEC` (from `Applied.changed`) + repack side effect | `TreemapView::apply_view_command` |
| exclusion set with `GIT`/`METRICS` deps | none — frozen (§4.6) | — |
| `PartitionRegistry` content changed (later: community provider finishes) | bump `partition_version` → `SpaceKey` differs → repack iff regroup names it; sets over it get `TREE`-like `PARTITIONS`? No: use `METRICS` bit for partition reloads (they are imported/derived data, same cadence) | provider completion path |

---

## 8. Migration steps

1. `spec.rs`: add `SpaceSpec`, `SpaceKind`, `PackOverrides`, `LineCap`, `PartitionRef` (if 02-set hasn't yet); `ViewSpec::default().space == SpaceSpec::default()`; JSON round-trip test. Behaviour: none.
2. `validate.rs`: rules from §3 (needs `SetResolver::static_deps` from 02-set.md; land 02-set §"Milestone-0 subset" first or stub `static_deps` for `Ids/Neighbors/Union`).
3. `space.rs`: `prune_tree`, `regroup_tree`, `plan_space`, `space_key`, `EffectivePack`; unit tests (§9). Behaviour: none (nothing calls it).
4. `project_loader.rs`: `SpaceInputs`; `load_project_cancellable` calls `plan_space` (default spec → `Cow::Borrowed`, identical layout — assert with the existing loader tests); `ProjectPreview.{space_id, excluded}`. Behaviour-preserving: golden = layouts equal before/after for the default view.
5. `layout_transition.rs`: `bridged` + tests; `PackingGeometryState::apply_snapshot` uses it on key-set mismatch. Behaviour-preserving for equal key sets.
6. `treemap.rs`: `space_key`, `partitions`, `start_repack` trigger in `apply_view_command`, focus reconcile, `Focus::step_in(.., layout)`. Behaviour-preserving until a document sets `space`.
7. Optional: settings-panel save (treemap.rs:3987) — if only `node_padding`/`max_display_lines` changed, `start_repack` instead of `reindex()`. Keep `filter_*` → `reindex()`.
8. Manual acceptance: `.outrider/views/hide-tests.json` with `sets.hidden = {glob:"**/tests/**"}`, `space.exclude = "hidden"` → tests vanish with a 160 ms tween, everything else re-flows minimally (hierarchical stability, pack.rs:1097 test); remove the file → they return. `space.regroup = {partition:"topFolder"}` on a repo with nested folders → flat groups, all files present, focus survives.

Nothing is deleted in this spec; index-time filters remain the persistent user-facing hide mechanism.

---

## 9. Tests

`outrider-view` (`crates/outrider-view/tests/space_test.rs`, fixture `mini_repo` via `outrider_index::index_repo(dir, &[], &[])` as in `index_test.rs`; `PackConfig` copied from `world::pack_config` constants: `page_w 640, line_step 15.6, header 20.8, container_header 52, bottom_pad 6, gap 8, aspect 1.0`):

- `prune_removes_subtree_and_empty_folders`: exclude `generated/junk.rs` → `generated` folder gone; every other id present; `pack(pruned)` has no rect for either.
- `prune_equals_index_filter`: `index_repo(dir, &[], &["generated"])` vs `prune_tree(index_repo(dir,&[],&[]), {generated})` → same id set (parent §8.1 "exclusion equals current filter behaviour"). Item ordinals must match — assert on full `SymbolId`s.
- `prune_keeps_hierarchical_stability`: rect of `src/util.rs` unchanged after excluding `generated/**` (sibling subtree untouched, mirrors pack.rs `sibling_subtree_stable_under_edit`).
- `regroup_keeps_every_leaf_id`: builtin `topFolder` partition on mini_repo → set of leaf ids equal; folder ids in regrouped tree all start with `@topFolder/`; root id unchanged.
- `regroup_is_deterministic`: `pack(regroup(t)) == pack(regroup(t))`.
- `regroup_then_prune_compose`: excluded file absent from its group; empty group dropped.
- `space_key_stable_and_sensitive`: same spec → equal key; changing `gap`, `exclude`, `regroup` → different key; `kind: matrix` → same key as treemap.
- `plan_space_unsupported_kinds`: `callgraph`/`matrix` → `Unsupported`.
- `validate_rejects_live_exclusion`: `exclude: {ids:["$focus"]}`, `{neighbors:"focus"}`, `{visible:true}` → hard violation with path `space.exclude`; `{changed:"HEAD~1"}` → soft only; `gap: 100` → hard.

App crate:

- `layout_transition.rs`: `bridged_drops_from_only_ids_and_starts_new_ids_at_target` (halfway sample: survivors interpolated, new ids at `to`, dropped ids absent).
- `treemap.rs` tests module (has `PackingGeometryState` tests at ~L4681–4802): `apply_snapshot_with_different_keys_uses_bridged` (no snap: survivor rect at t=80 ms differs from both endpoints).
- `focus.rs`: `step_in_skips_children_without_rects`.
- `project_loader.rs`: `start_repack_emits_snapshots_then_complete_without_preview` (uses the existing loader test harness ~L860).

Manual: §8 step 8; plus arrow keys never land on an excluded node; Enter on a folder whose largest child is excluded picks the largest *laid-out* child.

---

## 10. Open questions / risks

1. **Repack blocks interaction** (`map_interaction_enabled` false while `loader.is_loading()`). Fine for a walking skeleton; if agents patch `space` often, consider letting pan/zoom continue during a repack (the progressive snapshots already keep drawing).
2. **Regroup churn/textures.** Container textures are keyed by `SymbolId`; group folders get fresh ids so their thumbnails bake from scratch on every regroup. Acceptable; a partition-content hash in the id (`@p/g#hash`) would make them cacheable but breaks "same id across sessions". Left as is.
3. **Camera reset on repack.** `PackingGeometryState::invalidate` sets `camera = None` → re-fit to root. For a small exclusion the user probably wants to stay put. Option: keep the camera when `focus.current` survives and its rect moved < N%; decide after using it.
4. **`Cow` lifetime in `SpacePlan`.** The plan borrows `tree` on the worker thread — fine since the worker owns the tree; if it becomes awkward, return `Option<SymbolTree>` (None = pack the original).
5. **Partition versioning.** `partition_version` is a counter bumped by whoever mutates the registry; a real community provider (M8) must bump it exactly once per completed computation.
6. **Exclusion by `glob` on items** excludes item subtrees but leaves the file box; the file's `measure` still counts those lines, so the leaf page keeps its size. Document, don't fix (the measure is the file's real size).

---

## 11. Framework deltas

1. **`ResolveCtx` gains `partitions: &'a PartitionRegistry`** (needed by 02-set `community/layer` and here by `space_key`/`plan_space`). Add to 00-framework §2.4 and to the `ResolveCtx` construction in `paint_items` (§3.2).
2. **Module layout (00-framework §2.1):** add `space.rs` (this spec) and `partition.rs` (02-set §4.7) to `crates/outrider-view/src/`; `lib.rs` re-exports `space::{plan_space, space_key, SpaceKey, SpacePlan}` and `PartitionRegistry`.
3. **`ProjectLoader::start` signature** (00-framework §1 table row "Settings … `ProjectLoader::start(folder, settings)`") gains `SpaceInputs`; `TreemapView::start_loading` builds it from `view_spec`. Mention in §3.4 row "start_loading".
4. **00-framework §3.4 dirty wiring:** add row "focus reconciled after repack → `FOCUS`".
5. **`Focus::step_in`** takes `&PackLayout` (00-framework §1 table row "Focus"). No change to the `TreeIndex` move (§2.8).
6. No change to `Deps` bits, `ViewCommand`, `ViewPatch`, `SessionState`, or `PaintOverrides`.
