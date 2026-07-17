# Progressive Background Packing Design

## Summary

Outrider currently indexes and packs a project on a background thread, but it does not publish the indexed tree until the complete layout is ready. The loading bar therefore appears to stop after indexing while the new semantic-zone skyline packer works, and the map remains empty or shows the previous project.

This change adds a distinct packing phase. When indexing finishes, progress resets to zero, a cheap complete draft layout makes the entire new tree visible behind the existing dimmed loading overlay, and the worker progressively replaces draft container arrangements with final semantic-zone/skyline arrangements. The UI remains non-interactive until packing finishes.

## Goals

- Show a second, clearly labeled packing progress phase whose bar starts at zero.
- Show the complete indexed tree while optimized packing continues off the UI thread.
- Reflow the visible tree through coherent intermediate layouts rather than making one final jump.
- Keep every published layout finite, contained, non-overlapping, and structurally faithful to the symbol tree.
- Preserve the exact final geometry produced by the existing `outrider_layout::pack` behavior.
- Keep indexing, packing, snapshot publication, and cancellation off the UI thread.
- Avoid allowing a slow UI to delay the packing worker.

## Non-goals

- Changing semantic roles, folder hierarchy, skyline scoring, or final packing geometry.
- Allowing navigation, selection, editing, palette actions, or other map interaction while packing.
- Persisting partial layouts between launches.
- Pixel-perfect or screenshot-based automated UI tests. Manual acceptance testing is the UI/animation gate.

## User experience

Indexing retains the current full-screen dim layer and centered loading card. After the symbol tree is built, the same card changes to `Packing <folder>...`. Its detail line reads `Packing X/N nodes...`, and its progress bar visibly returns to empty at `0/N`.

A cheap draft layout is then installed, so every indexed node is visible behind the overlay. As optimized containers finish, the worker publishes bounded layout snapshots and the tree animates toward them. The overlay continues to consume input for the entire packing phase. The camera keeps the changing root bounds fitted in the viewport.

The worker publishes no more than 30 layout snapshots total, including the initial draft and final layout. Geometry transitions use a 160 ms ease-out. If a new snapshot arrives during a transition, the transition retargets from the geometry currently on screen rather than from the previous target.

When packing reaches `N/N`, the exact final layout is installed, the overlay disappears, and normal interaction resumes.

## Loading stages and progress

`LoadProgress` will expose an explicit stage rather than requiring the overlay to interpret the indexer's raw numeric phase. The stages are:

- scanning: indeterminate, zero fraction;
- parsing: `files_parsed / files_total`;
- building tree: completed indexing bar;
- packing: `nodes_packed / nodes_total`, starting at zero;
- complete: terminal state, normally not rendered because the overlay is removed.

The loader owns packing-stage atomics separate from `outrider_index::IndexProgress`. This avoids extending the indexing crate with layout-specific state. The worker sets the packing total and stores zero before it computes or publishes the draft, so polling can observe a real phase reset even if draft creation takes more than one UI frame.

The packing total is the number of symbol nodes in the immutable indexed tree. Progress is monotonic and counts each post-order node exactly once. Leaves advance progress even though their natural size is already known by the draft pass; containers advance progress after their final child arrangement has been committed.

## Loader event model

The worker and UI exchange lifecycle events separately from replaceable layout snapshots.

Lifecycle events are ordered and lossless:

- `PreviewReady`: generation, immutable tree, complete draft layout, project metadata, cache namespace, warnings, and source fingerprints;
- `Complete`: generation and exact final layout;
- `Failed`: generation and error text.

The worker retains the indexed tree for packing and sends one deep clone in `PreviewReady`. The clone happens on the worker thread, never the UI thread. This avoids a repository-wide conversion of tree ownership to `Arc<SymbolTree>` while still ensuring the tree is sent only once and project state is installed only once.

Intermediate geometry uses a separate latest-only slot guarded by a short-lived mutex. Publishing overwrites the previous pending snapshot and never blocks. `ProjectLoader::poll` takes the newest available snapshot. A final event includes the final layout itself, so completion never depends on the UI first consuming a pending intermediate snapshot.

Every lifecycle event and snapshot carries the load generation. Starting another load cancels the current token, clears pending preview/snapshot state, and causes stale generations to be ignored. Packing checks cancellation between post-order nodes and before expensive folder arrangements and snapshot materialization.

## Draft layout

The draft pass is deterministic and linear in the number of nodes. It computes normal leaf dimensions, then vertically stacks each container's children with the configured gap and header inset. It does not build semantic profiles, evaluate shelf candidates, or run skyline searches.

The draft contains a rectangle for every symbol. It preserves the real parent/child hierarchy, uses the same configured leaf measurements and container insets as the final packer, and satisfies the same finite/containment/gap invariants. Its shape is intentionally temporary; speed and a coherent complete preview matter more than density.

## Progressive refinement

The progressive engine maintains final local layout state for every node:

- final measured size once known;
- final child offsets once known;
- whether the node has received its final arrangement;
- deterministic post-order position.

The draft pass supplies the initial complete layout. The engine then walks nodes in deterministic post-order. When a node is finalized, all of its children already have final sizes, so it can use the same source-order shelf or semantic-zone skyline arrangement as `pack()`.

Intermediate work does not mutate absolute rectangles or eagerly repair every ancestor. At a snapshot checkpoint, a hybrid materialization walks from the root: finalized containers use their exact local arrangement, while unfinished containers are temporarily stacked with the draft rule using the newest available child sizes. This makes every published root layout complete and valid without paying an ancestor-repair cost after every node. An unfinished ancestor's temporary arrangement is replaced by its exact final arrangement when post-order processing reaches it.

Snapshot checkpoints are deterministic and count-based. With the app's `max_snapshots` value of 30, the engine divides the node total into at most 30 milestones and materializes a complete absolute `PackLayout` only after crossing a milestone and committing at least one geometry-changing container. The cap includes the draft at zero and final layout at the node total. Progress atomics still update for every node even when no geometry snapshot is materialized.

The layout crate adds these public types and entry point:

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

`max_snapshots` is clamped to the meaningful range for the tree and counts the draft and final snapshots. With a nonempty tree, the engine emits progress events for every value from zero through the node total. Only deterministic milestone events contain a layout snapshot.

The existing `pack(tree, cfg) -> PackLayout` API remains available and uses the same final-arrangement helpers as the progressive engine. `pack()` does not pay for the draft pass. The progressive final result must compare exactly equal to `pack()`; there is one source of truth for role classification, ordering, skyline placement, shelf placement, and absolute coordinates.

## UI installation and animation

`PreviewReady` installs project-owned state exactly once: tree, buffers, symbol lookup, focus root, navigation history, empty texture cache, warnings, and the draft layout. Later snapshots replace only layout geometry and transition state. They do not recreate buffers, textures, focus, navigation, or metadata.

During packing, the renderer uses an interpolated display layout between the current displayed geometry and the newest target. All node IDs exist in both layouts because the draft and every intermediate snapshot are complete. Retargeting first samples the current interpolation, then uses that sample as the next transition's source.

The authoritative target layout remains separate from the transient display layout so hit testing and focus cannot accidentally observe half-interpolated state. Map mouse handlers, keyboard navigation, palette actions, context menus, and focus actions explicitly guard on loading state; the visual overlay alone is not treated as an input guarantee. The global Open Folder action remains available so a user can supersede and cancel a slow load. Because map interaction is disabled, the camera may continuously refit the interpolated root without preserving user pan or zoom.

On completion, the final target is installed and may finish its short transition before or as the overlay is removed; the authoritative final layout is available immediately. No project-state installation is repeated.

## Failure and cancellation

If indexing fails before a preview is published, the current project remains installed and Outrider shows the existing warning behavior.

If packing fails or panics after `PreviewReady`, the draft or latest valid intermediate layout remains installed. The loader removes the packing overlay, enables interaction, and shows a warning explaining that optimized packing failed. A valid complete layout is therefore always available; the map never becomes blank.

Cancellation is cooperative. A superseded worker stops at checkpoints and may leave an obsolete snapshot in its local path, but generation checks prevent that snapshot or terminal event from mutating the active project.

Mutex poisoning or disconnected worker channels become recoverable load errors. Snapshot-slot failure cannot invalidate an already published preview or final layout.

## Automated testing

Automated tests focus on deterministic model and state behavior rather than visual appearance.

Layout-crate tests will verify:

- the draft contains exactly one finite rectangle per node;
- draft and every emitted snapshot satisfy recursive containment and configured sibling gaps;
- progress starts at zero, is monotonic, and ends at the exact node total;
- snapshot count is capped and checkpoint selection is deterministic;
- progressive final geometry exactly equals `pack()`;
- cancellation stops before additional expensive work or snapshots;
- mixed semantic roles, source-ordered files, deep trees, and large synthetic folders remain valid.

Loader tests will verify:

- indexing completion transitions to an observable `Packing 0/N` phase;
- preview is delivered before completion;
- intermediate snapshots are latest-only and do not block the worker;
- project metadata is delivered once;
- final completion carries exact layout geometry;
- superseded generations, cancellation, disconnects, and worker panics remain recoverable.

Small treemap state tests may verify that preview installation and geometry replacement are separate operations and that loading continues to gate interaction. No screenshot, pixel-timing, or animation-appearance tests are required; those are covered by manual acceptance testing.

## Acceptance criteria

- On a project whose optimized packing is visibly slow, the indexing bar completes and a separate packing bar starts from empty.
- The complete new tree becomes visible behind the current full-screen dimmed overlay before optimized packing completes.
- The tree visibly rearranges through multiple coherent states and ends in the same layout produced before this feature.
- No mouse or keyboard interaction affects the map until packing completes or fails into the valid-layout fallback.
- Opening another project during loading never installs stale tree or layout state.
- Packing does not block the UI thread, and a slow UI does not block the packing worker.
