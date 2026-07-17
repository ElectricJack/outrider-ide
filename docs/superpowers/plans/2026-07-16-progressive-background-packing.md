# Progressive Background Packing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reset loading progress for a distinct packing phase and show a complete, progressively refined tree while optimized semantic-zone skyline packing continues in the background.

**Architecture:** Refactor the layout crate around shared per-node local layouts, add a deterministic progressive packer that emits a cheap draft and at most 30 complete hybrid snapshots, then split the app loader into guaranteed preview/final events plus a nonblocking latest-only snapshot mailbox. The treemap installs project state once from the preview, animates geometry-only updates under the existing dimmed overlay, and enables map interaction only after finalization or valid-layout fallback.

**Tech Stack:** Rust 2021, `std::sync` channels/atomics/mutexes, GPUI, existing `outrider-index` and `outrider-layout` crates.

## Global Constraints

- Design authority: `docs/superpowers/specs/2026-07-16-progressive-background-packing-design.md`.
- Preserve the exact final geometry and public behavior of `pack(&SymbolTree, &PackConfig) -> PackLayout`.
- The app passes `max_snapshots = 30`; this cap includes the draft and final layouts.
- Every draft/intermediate/final snapshot must contain every tree node and remain finite, contained, and sibling-gap-separated.
- Packing progress emits every integer value from `0` through the exact symbol-node total and never regresses.
- Intermediate snapshots are latest-only and must never block the packing worker; preview and terminal events are ordered and lossless.
- Map interaction remains disabled during packing; only the global Open Folder path may supersede/cancel the load.
- The final event carries exact final geometry even if all intermediate snapshots are dropped.
- Automated tests cover layout and state behavior, not screenshot or pixel-timing behavior; the user provides UI acceptance testing.
- Follow strict RED/GREEN TDD for every behavior change and commit each task independently.

---

## File structure

- `crates/outrider-layout/src/pack.rs`: shared exact local-layout calculation used by synchronous and progressive packing.
- `crates/outrider-layout/src/progressive.rs`: draft layout, post-order refinement, milestone scheduling, hybrid snapshot materialization, cancellation, and focused tests.
- `crates/outrider-layout/src/lib.rs`: exports progressive packing types and entry point.
- `crates/outrider/src/project_loader.rs`: explicit load phases, preview/final control events, latest-only snapshot mailbox, cancellation, and loader tests.
- `crates/outrider/src/layout_transition.rs`: pure rectangle/layout interpolation and 160 ms transition state.
- `crates/outrider/src/main.rs`: registers the transition module.
- `crates/outrider/src/treemap.rs`: one-time preview installation, geometry-only updates, animation advancement, camera/cache invalidation, and interaction gating.
- `crates/outrider/src/overlays.rs`: packing copy and reset progress fraction.

---

### Task 1: Shared exact local-layout engine

**Files:**
- Modify: `crates/outrider-layout/src/pack.rs`

**Interfaces:**
- Consumes: existing `SymbolTree`, `SymbolNode`, `PackConfig`, semantic profiles, shelf helpers, and skyline helpers.
- Produces: crate-private `LocalLayout`, `ExactLayouts`, `build_exact_layouts`, and `absolute_from_layouts` for Task 2.

- [ ] **Step 1: Add a focused characterization test before refactoring**

Temporarily rename the current recursive implementation to `legacy_pack_oracle`, then add exact comparisons covering the worked example, a semantic-role tree, a source-ordered file tree, and the large mixed-role fixture:

```rust
#[test]
fn exact_local_layout_rebuild_matches_public_pack() {
    let tree = worked_example();
    let expected = legacy_pack_oracle(&tree, &cfg());
    let profiles = build_profiles(&tree.root);
    let locals = build_exact_layouts(&tree.root, &profiles, &cfg());
    let rebuilt = absolute_from_layouts(&tree.root, &locals);
    assert_eq!(rebuilt, expected);
}

#[test]
fn local_layout_matches_legacy_across_packing_branches() {
    for tree in [
        semantic_inheritance_example(),
        source_order_example(),
        large_role_varied_example(),
    ] {
        let expected = legacy_pack_oracle(&tree, &cfg());
        let profiles = build_profiles(&tree.root);
        let locals = build_exact_layouts(&tree.root, &profiles, &cfg());
        assert_eq!(absolute_from_layouts(&tree.root, &locals), expected);
    }
}
```

Add the three named fixture helpers beside the tests. They must respectively exercise inherited semantic roles, C++/Markdown source order, and the existing 540-child role-varied shape.

- [ ] **Step 2: Run the test and verify RED**

Run:

```powershell
cargo test -p outrider-layout pack::tests::exact_local_layout_rebuild_matches_public_pack -- --exact
```

Expected: compile failure because `build_exact_layouts` and `absolute_from_layouts` do not exist.

- [ ] **Step 3: Introduce shared local-layout types**

Create crate-private representations in `pack.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalLayout {
    pub(crate) size: (f64, f64),
    pub(crate) children: BTreeMap<SymbolId, (f64, f64)>,
}

pub(crate) type ExactLayouts = BTreeMap<SymbolId, LocalLayout>;
```

Refactor the existing recursive `size` implementation so one exact node arrangement is calculated from already-computed child `LocalLayout::size` values. Preserve the existing vector construction, sorting, semantic role order, candidate order, and floating-point operation order. Do not reimplement the packing rules in a second function.

Extract profile-free natural leaf sizing for the draft path:

```rust
pub(crate) fn leaf_local_layout(node: &SymbolNode, cfg: &PackConfig) -> LocalLayout;
```

Expose these crate-private helpers for `progressive.rs`:

```rust
pub(crate) fn build_exact_layouts(
    root: &SymbolNode,
    profiles: &RoleProfiles,
    cfg: &PackConfig,
) -> ExactLayouts;

pub(crate) fn exact_local_layout(
    node: &SymbolNode,
    inherited_role: SemanticRole,
    profiles: &RoleProfiles,
    cfg: &PackConfig,
    child_sizes: &BTreeMap<SymbolId, (f64, f64)>,
) -> LocalLayout;

pub(crate) fn absolute_from_layouts(
    root: &SymbolNode,
    layouts: &ExactLayouts,
) -> PackLayout;
```

`pack()` becomes:

```rust
pub fn pack(tree: &SymbolTree, cfg: &PackConfig) -> PackLayout {
    let profiles = build_profiles(&tree.root);
    let layouts = build_exact_layouts(&tree.root, &profiles, cfg);
    absolute_from_layouts(&tree.root, &layouts)
}
```

Run the temporary oracle comparisons through every packing branch during the GREEN verification below. Keep the oracle only until those comparisons pass.

- [ ] **Step 4: Verify GREEN and regression coverage**

Run:

```powershell
cargo test -p outrider-layout pack::tests::exact_local_layout_rebuild_matches_public_pack -- --exact
cargo test -p outrider-layout pack::tests::local_layout_matches_legacy_across_packing_branches -- --exact
```

Expected: both temporary exact oracle comparisons pass. Then remove `legacy_pack_oracle`, its duplicate recursive path, and the temporary oracle-only tests. Keep the existing exact-coordinate, semantic, source-order, determinism, and large-tree tests as permanent coverage, and run:

```powershell
cargo test -p outrider-layout
cargo fmt -p outrider-layout -- --check
```

Expected: all permanent layout tests pass with unchanged exact geometry; formatting passes; no duplicate legacy implementation remains.

- [ ] **Step 5: Commit Task 1**

```powershell
git add crates/outrider-layout/src/pack.rs
git commit -m "refactor: share exact local packing layouts"
```

---

### Task 2: Deterministic progressive packer

**Files:**
- Create: `crates/outrider-layout/src/progressive.rs`
- Modify: `crates/outrider-layout/src/pack.rs`
- Modify: `crates/outrider-layout/src/lib.rs`

**Interfaces:**
- Consumes: `LocalLayout`, `ExactLayouts`, `exact_local_layout`, `absolute_from_layouts`, `PackLayout`, and `PackConfig` from Task 1.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PackProgress {
    pub completed: usize,
    pub total: usize,
    pub snapshot: Option<PackLayout>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackCancelled;

pub fn pack_progressive<C, E>(
    tree: &SymbolTree,
    cfg: &PackConfig,
    max_snapshots: usize,
    is_cancelled: C,
    emit: E,
) -> Result<PackLayout, PackCancelled>
where
    C: Fn() -> bool,
    E: FnMut(PackProgress);
```

Add a cancellable exact-node boundary:

```rust
pub(crate) fn exact_local_layout_cancellable<C>(
    node: &SymbolNode,
    inherited_role: SemanticRole,
    profiles: &RoleProfiles,
    cfg: &PackConfig,
    child_sizes: &BTreeMap<SymbolId, (f64, f64)>,
    is_cancelled: &C,
) -> Result<LocalLayout, PackCancelled>
where
    C: Fn() -> bool;
```

The ordinary `exact_local_layout` wraps this with a never-cancel predicate.

- [ ] **Step 1: Write RED tests for draft, progress, milestones, and final equality**

In `progressive.rs`, reuse or move the recursive geometry validator from `pack.rs`. Wire `mod progressive;` and the public re-exports in `lib.rs` as part of the RED test edit, leaving the referenced production items absent so compilation fails for the intended missing-feature reason. Add tests whose assertions include:

```rust
let mut events = Vec::new();
let progressive = pack_progressive(&tree, &cfg(), 30, || false, |event| {
    events.push(event)
})
.unwrap();

let total = count_nodes(&tree.root);
assert_eq!(events.iter().map(|e| e.completed).collect::<Vec<_>>(), (0..=total).collect::<Vec<_>>());
assert_eq!(events.first().unwrap().completed, 0);
assert!(events.first().unwrap().snapshot.is_some());
assert_eq!(events.last().unwrap().completed, total);
assert_eq!(events.last().unwrap().snapshot.as_ref(), Some(&progressive));
assert_eq!(progressive, pack(&tree, &cfg()));
let effective_cap = 30_usize.max(2).min(total + 1);
assert!(events.iter().filter(|e| e.snapshot.is_some()).count() <= effective_cap);
for event in events.iter().filter_map(|event| event.snapshot.as_ref()) {
    assert_complete_valid_geometry(&tree.root, event, &cfg());
}
```

Cover `max_snapshots` values `0`, `1`, `2`, `30`, and greater than the node count. Values `0` and `1` have an effective cap of two and therefore still emit the required draft and final snapshots. Also cover a one-node tree, a deep tree, source-ordered C++/Markdown nodes, an ordinary weak folder nested beneath an explicitly named `tests` folder, and the existing large role-varied folder fixture.

- [ ] **Step 2: Run the progressive tests and verify RED**

Run:

```powershell
cargo test -p outrider-layout progressive::tests -- --nocapture
```

Expected: compile failure because the wired module's public API does not exist. Confirm the command does not report `0 tests`.

- [ ] **Step 3: Implement deterministic milestones and the linear draft**

Add private helpers with these responsibilities:

```rust
fn count_nodes(root: &SymbolNode) -> usize;
fn postorder_with_roles<'a>(root: &'a SymbolNode, profiles: &RoleProfiles, out: &mut Vec<(&'a SymbolNode, SemanticRole)>);
fn snapshot_milestones(total: usize, max_snapshots: usize) -> Vec<usize>;
fn draft_local_layout(node: &SymbolNode, child_sizes: &BTreeMap<SymbolId, (f64, f64)>, cfg: &PackConfig) -> LocalLayout;
fn build_draft_layouts(root: &SymbolNode, cfg: &PackConfig, is_cancelled: &impl Fn() -> bool) -> Result<ExactLayouts, PackCancelled>;
```

Milestones are deterministic, deduplicated, contain `0` and `total`, and never exceed the effective cap. For an effective snapshot count `S >= 2`, use integer-safe ceiling division for `ceil(k * total / (S - 1))` and avoid overflow by using `u128` intermediates.

The draft arranges every container in one vertical column beneath `container_header + gap`, uses `leaf_local_layout`, and preserves child vector order. It must neither build semantic profiles nor invoke shelf candidate/skyline searches.

- [ ] **Step 4: Implement post-order refinement and hybrid snapshots**

Build semantic profiles once, propagate effective inherited roles top-down, then process the deterministic `(node, inherited_role)` post-order vector. Store exact locals only after `exact_local_layout_cancellable` and its trailing cancellation checkpoint succeed. At each event:

```rust
emit(PackProgress {
    completed,
    total,
    snapshot: should_materialize.then(|| materialize_hybrid(...)),
});
```

Crossing a milestone sets `snapshot_pending`; it does not materialize on a leaf-only step. The next successfully committed geometry-changing container consumes that flag and creates the snapshot. Draft zero and exact final total always contain snapshots.

`materialize_hybrid` walks post-order once. It uses an exact stored local for completed nodes and recomputes unfinished containers with `draft_local_layout` using the newest child sizes, then calls `absolute_from_layouts`. It checks cancellation before and during the O(N) materialization.

The final `completed == total` event contains the exact layout produced solely from exact locals, and the returned layout is the same value. Keep `pack()` on the non-progressive path so synchronous callers do not pay draft/snapshot overhead.

- [ ] **Step 5: Add cancellation RED/GREEN coverage**

Write tests that cancel before draft work, after a chosen completed count, and during a large folder refinement. Assert `Err(PackCancelled)`, no progress after the cancellation point, and no partial invalid snapshot.

Add cancellation checkpoints to semantic profile traversal, role propagation, folder sorting/group creation, shelf candidate/placement loops, skyline width/candidate/placement loops, and snapshot materialization through crate-private cancellable variants returning `Result<_, PackCancelled>`. The ordinary helpers remain wrappers that pass `|| false`, preserving current APIs and tests.

- [ ] **Step 6: Verify Task 2**

Run:

```powershell
cargo test -p outrider-layout progressive::tests -- --nocapture
cargo test -p outrider-layout
cargo fmt -p outrider-layout -- --check
cargo clippy -p outrider-layout --all-targets --all-features --no-deps -- -D warnings
```

Expected: all progressive and existing layout tests pass; final layouts compare exactly; fmt and strict changed-crate clippy pass.

- [ ] **Step 7: Commit Task 2**

```powershell
git add crates/outrider-layout/src/lib.rs crates/outrider-layout/src/pack.rs crates/outrider-layout/src/progressive.rs crates/outrider-layout/src/skyline.rs crates/outrider-layout/src/zones.rs
git commit -m "feat: stream deterministic progressive packing"
```

---

### Task 3: Staged project loader and latest-only geometry mailbox

**Files:**
- Modify: `crates/outrider/src/project_loader.rs`
- Modify: `crates/outrider/src/overlays.rs`

**Interfaces:**
- Consumes: `outrider_layout::pack_progressive`, `PackProgress`, `PackLayout`, existing indexing progress, cancellation token, and project metadata preparation.
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPhase {
    Scanning,
    Parsing,
    BuildingTree,
    Packing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadProgress {
    pub folder_name: String,
    pub phase: LoadPhase,
    pub completed: usize,
    pub total: usize,
}

pub struct ProjectPreview {
    pub generation: u64,
    pub project_root: PathBuf,
    pub tree: SymbolTree,
    pub layout: PackLayout,
    pub warnings: Vec<String>,
    pub source_fingerprints: BTreeMap<String, u64>,
    pub disk_cache_bytes: u64,
    pub project_namespace: Result<ProjectTextureNamespace, String>,
}

pub enum LoaderPoll {
    Idle,
    Loading(LoadProgress),
    Preview(Box<ProjectPreview>),
    Snapshot { generation: u64, layout: PackLayout },
    Complete { generation: u64, layout: PackLayout },
    Failed { generation: u64, message: String, preview_delivered: bool },
}
```

The ordered worker control stream also contains an internal `PackingStarted { generation, total }` event. `ProjectLoader::poll` converts it into exactly one `LoaderPoll::Loading` value with `phase = Packing` and `completed = 0` before it may return Preview, Snapshot, Complete, or Failed for that generation.

- [ ] **Step 1: Write loader lifecycle RED tests**

Using barriers and channels rather than sleeps, add tests that prove:

```rust
assert_eq!(packing_progress.phase, LoadPhase::Packing);
assert_eq!((packing_progress.completed, packing_progress.total), (0, expected_nodes));
assert!(matches!(first_control_event, LoaderPoll::Preview(_)));
assert!(matches!(terminal_event, LoaderPoll::Complete { .. }));
```

Also test that three snapshot writes before a poll yield only the newest layout, terminal completion remains available when the snapshot slot is occupied, a new generation rejects every old event type, and superseding a worker cancels progressive packing.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```powershell
cargo test -p outrider project_loader::tests -- --nocapture
```

Expected: compile failures because `LoadPhase`, staged events, and the snapshot mailbox do not exist.

- [ ] **Step 3: Add coherent loader-owned progress state**

Keep `IndexProgress` unchanged. Add loader-owned packing counters for updates after the ordered start latch. Map raw index phases to `LoadPhase` only before `PackingStarted` has been delivered.

Avoid a transient user-visible `Done` between building the tree and packing.

- [ ] **Step 4: Split guaranteed control events from snapshots**

Use an ordered channel for packing-start, preview, and terminal events. Add a per-generation overwrite mailbox whose worker-side publication never waits:

```rust
#[derive(Default)]
struct SnapshotMailbox(Mutex<Option<LayoutSnapshot>>);

impl SnapshotMailbox {
    fn publish(&self, snapshot: LayoutSnapshot) {
        if let Ok(mut pending) = self.0.try_lock() {
            *pending = Some(snapshot);
        }
    }

    fn take(&self) -> Option<LayoutSnapshot> {
        self.0.lock().ok()?.take()
    }
}
```

Do not put intermediate snapshots on `sync_channel(1)`. Contention or mutex poisoning drops only the intermediate snapshot; lossless Preview and Complete events still provide valid layouts. `poll()` must deliver `PackingStarted` and then queued Preview before any snapshot or terminal event, even if the worker completes between UI frames. After Preview, a queued Complete or Failed event wins over and clears the mailbox; no Snapshot may be returned after a terminal event.

- [ ] **Step 5: Stage the worker pipeline**

After indexing:

1. count nodes and send the ordered `PackingStarted { total }` latch;
2. call `pack_progressive(..., 30, cancellation, callback)`;
3. on the callback's `completed == 0` snapshot, clone the tree once on the worker and send `ProjectPreview` with all project metadata;
4. publish later nonterminal snapshots to the mailbox and store progress atomically;
5. send `Complete` with the returned exact layout;
6. turn cancellation into silent stale-worker termination, and other errors/panics into `Failed` with whether preview was sent.

- [ ] **Step 6: Update overlay phase mapping**

Extract a pure helper and test it without rendering pixels:

```rust
fn loading_copy(state: &LoadProgress) -> (String, String, f32);
```

For packing, it returns title `Packing {folder_name}...`, detail `Packing {completed}/{total} nodes...`, and `completed / total` (zero when total is zero). Preserve existing scanning/parsing/building-tree copy and the full-screen `0x00000088` dim layer.

- [ ] **Step 7: Verify Task 3**

Run:

```powershell
cargo test -p outrider project_loader::tests -- --nocapture
cargo test -p outrider overlays::tests -- --nocapture
cargo fmt -p outrider -- --check
```

Expected: staged lifecycle, latest-only mailbox, cancellation, generation fencing, and copy tests pass.

- [ ] **Step 8: Commit Task 3**

```powershell
git add crates/outrider/src/project_loader.rs crates/outrider/src/overlays.rs
git commit -m "feat: stage project loading through packing"
```

---

### Task 4: Live treemap preview, animated geometry, and input gating

**Files:**
- Create: `crates/outrider/src/layout_transition.rs`
- Modify: `crates/outrider/src/main.rs`
- Modify: `crates/outrider/src/treemap.rs`

**Interfaces:**
- Consumes: staged `LoaderPoll` variants from Task 3 and complete `PackLayout` snapshots from Task 2.
- Produces: one-time preview installation, geometry-only snapshot application, 160 ms ease-out interpolation, finalization/fallback, and map input guards.

- [ ] **Step 1: Write interpolation RED tests**

Create `layout_transition.rs` tests for exact endpoints, midpoint rectangle interpolation, missing-ID rejection, and retargeting from the current sample:

```rust
let transition = LayoutTransition::new(from.clone(), to.clone(), now);
assert_eq!(transition.sample(now), from);
assert_eq!(transition.sample(now + Duration::from_millis(160)), to);
let halfway = transition.sample(now + Duration::from_millis(80));
assert_eq!(halfway.rects[&id].x, ease_out_lerp(0.0, 100.0, 0.5));
```

The production interface is:

```rust
pub(crate) const PACK_LAYOUT_TWEEN: Duration = Duration::from_millis(160);

pub(crate) struct LayoutTransition {
    from: PackLayout,
    to: PackLayout,
    started_at: Instant,
}

impl LayoutTransition {
    pub(crate) fn new(from: PackLayout, to: PackLayout, now: Instant) -> Self;
    pub(crate) fn sample(&self, now: Instant) -> PackLayout;
    pub(crate) fn is_complete(&self, now: Instant) -> bool;
    pub(crate) fn retarget(self, target: PackLayout, now: Instant) -> Self;
}
```

- [ ] **Step 2: Run the transition test and verify RED**

Run:

```powershell
cargo test -p outrider layout_transition::tests -- --nocapture
```

Expected: compile failure because the module does not exist.

- [ ] **Step 3: Implement the pure transition helper**

Interpolate `x`, `y`, `w`, and `h` for every ID using ease-out cubic:

```rust
fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}
```

All progressive snapshots contain the same ID set. If a mismatch nevertheless reaches the helper, return the target layout rather than displaying missing geometry. Register `mod layout_transition;` in `main.rs`.

- [ ] **Step 4: Split preview installation from geometry updates**

Replace the current terminal-only `install_project` path with:

```rust
fn install_project_preview(&mut self, preview: ProjectPreview);
fn apply_packing_snapshot(&mut self, generation: u64, layout: PackLayout, now: Instant);
fn finish_packing(&mut self, generation: u64, layout: PackLayout);
fn fail_packing(&mut self, generation: u64, message: String, preview_delivered: bool);
```

Preview installation performs the existing one-time tree/buffer/focus/history/palette/texture initialization. Snapshot application changes only displayed/target layout transition state, clears `neighbors` and `hover_id`, and resets/refits the camera. It must not recreate buffers, focus, navigation history, palette, texture cache, warnings, fingerprints, or namespace.

Completion installs the exact final layout, clears transition state and progress, and enables interaction. Failure before preview preserves the old project. Failure after preview keeps the last complete layout, removes the overlay, enables interaction, and posts a warning.

- [ ] **Step 5: Advance animation in the existing render loop**

Add a field:

```rust
layout_transition: Option<LayoutTransition>,
packing_target_layout: Option<PackLayout>,
```

While loading, sample the transition at the start of `render`, assign the sampled complete layout to displayed `self.layout`, retain the newest authoritative target in `packing_target_layout`, clear geometry-derived caches, and request another frame. New snapshots retarget from the current displayed sample. Set `camera = None` while geometry changes so the root is refitted. The final event assigns exact final geometry directly and clears both target and transition before interaction is enabled.

- [ ] **Step 6: Guard map interaction during packing**

Create one predicate:

```rust
fn map_interaction_enabled(&self) -> bool {
    !self.loader.is_loading()
}
```

Apply it to mouse press/release/move, scroll/zoom, keyboard navigation, palette/context-menu actions, rename/delete, focus actions, and map toolbar actions. Leave the global Open Folder action enabled so it can supersede the worker. On platforms using the in-window File menu, keep only Open Folder enabled while loading; Clear Cache and other project/map actions remain guarded. Do not rely on overlay hit testing as the guard.

- [ ] **Step 7: Add minimal state tests**

Avoid GPUI screenshots. Test pure or extracted state helpers proving that:

- applying a snapshot changes layout and invalidates `neighbors`/hover/camera without replacing preview-owned resources;
- finalization clears packing progress and installs exact final geometry;
- failure after preview retains a complete layout and clears loading state;
- `map_interaction_enabled` is false while loading and true after terminal completion.

- [ ] **Step 8: Verify Task 4 and the workspace**

Run:

```powershell
cargo test -p outrider layout_transition::tests -- --nocapture
cargo test -p outrider project_loader::tests -- --nocapture
cargo test --workspace
cargo fmt -p outrider-layout -p outrider -- --check
cargo clippy -p outrider-layout --all-targets --all-features --no-deps -- -D warnings
git diff --check
```

Expected: focused and workspace tests pass; changed crates format cleanly; strict layout clippy passes; no whitespace errors. Existing unrelated workspace warnings may remain documented but no new warning is allowed.

- [ ] **Step 9: Commit Task 4**

```powershell
git add crates/outrider/src/main.rs crates/outrider/src/layout_transition.rs crates/outrider/src/treemap.rs
git commit -m "feat: show live tree during background packing"
```

---

## Manual acceptance handoff

After automated verification and review, report these manual checks to the user without claiming they were performed:

1. Open a project large enough for skyline packing to take visibly longer than indexing.
2. Confirm the completed indexing bar is replaced by an empty `Packing 0/N nodes...` bar.
3. Confirm the complete tree appears behind the existing full-screen dim overlay.
4. Confirm the tree rearranges through multiple smooth, coherent layouts.
5. Confirm map interaction is blocked during packing while Open Folder can supersede it.
6. Confirm the final map matches the optimized semantic-zone skyline result and interaction resumes.

## Plan self-review

- Spec coverage: all progress, draft, refinement, snapshot, loader, animation, cancellation, fallback, and interaction requirements map to Tasks 1-4.
- Placeholder scan: every implementation and error path is concrete; no deferred steps remain.
- Type consistency: Task 2's `PackProgress` feeds Task 3; Task 3's staged `LoaderPoll` feeds Task 4; all use `PackLayout` and generation IDs consistently.
- Scope: the work changes one layout subsystem and one existing loader/UI integration path; no independent feature needs a separate plan.
