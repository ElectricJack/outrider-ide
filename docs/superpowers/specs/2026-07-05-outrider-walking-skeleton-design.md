# Outrider — Walking Skeleton Implementation Spec

**Status:** Approved design, v1.0
**Date:** 2026-07-05
**Parent document:** `docs/code-comprehension-viewer-design.md` (v0.1)
**Scope:** The walking skeleton defined in §10.2 of the parent design, and nothing else.

---

## 1. Purpose

The parent design is internally consistent on paper except for two bets that only a running prototype can settle:

1. **Does deterministic treemap layout + camera-follow *feel* like navigation, or like a slideshow?**
2. **Does the frozen-layer descend transition read as "same thing, deeper," or as a teleport?**

The skeleton's only job is to answer those two questions on a real repository. Every scope decision below either serves that job or exercises one of the parent design's four one-way-door substrate decisions (§2.4) that are cheap now and brutal to retrofit.

## 2. Decisions settled during spec review

| Question | Decision |
|---|---|
| Spec scope | Walking skeleton only (parent §10.2) |
| Platform | Linux under WSL2 (WSLg + Vulkan); Windows-native as fallback if WSLg GPU proves flaky |
| First language | Rust (`tree-sitter-rust`); dogfood on Outrider's own repo |
| Name | **Outrider** — binary `outrider`, crates `outrider-*` |
| Dependency graph strategy (post-skeleton note) | Import/module graph from tree-sitter first; call-level resolution via LSP/SCIP deferred. Not in skeleton scope, recorded here because it shaped what the skeleton does *not* attempt (no call edges, no faked call edges) |
| Fill metric | Git churn (commit count per file), percentile-scaled |

## 3. Scope

### In scope

- Cargo workspace: `outrider-index`, `outrider-layout`, `outrider` (GPUI app).
- Whole-repo scan + parse of Rust files; symbol tree of folders → files → items → methods.
- Churn metric from git history; percentile-scaled fill color.
- Deterministic icicle/grid layout with stable name ordering, slack gaps, hierarchical addressing.
- Camera-follow navigation: arrows move focus, mouse moves camera, Home/End, simple linear history for Left.
- Five-rung fidelity ladder with minimal content per rung; tree-sitter-highlighted code at Full.
- Rope buffers + anchors for files materialized at Full fidelity; incremental re-parse on disk change.
- Descend transition (Enter/Esc) with a genuinely frozen file layer.
- Property tests for layout invariants; fixture tests for indexing.

### Out of scope (explicitly)

Call-graph space and the Tab switch (key disabled), any call edges real or faked, LLM narration, ACP, editing of any kind, minimap, jump-to-symbol search, all signals except churn fill, all languages except Rust, persistence beyond a churn cache, multi-repo, configuration UI.

## 4. Architecture

```
outrider-index    repo scan → SymbolTree + metrics        (no GPUI types)
outrider-layout   SymbolTree → WorldLayout                (no GPUI types, pure)
outrider          GPUI app: camera, input, fidelity, transitions
```

Data flows one way. `outrider-index` produces a `SymbolTree`; `outrider-layout` maps it to a `WorldLayout`; the app renders `WorldLayout` through a camera and mutates neither. A change on disk triggers rebuild → relayout → render; the app owns only ephemeral state (camera, focus, history stack, transition state, materialized buffers).

`outrider-layout` is a pure function of its input. This is what converts parent-design invariants 1–3 (determinism, continuity, stable ordering) from promises into property tests.

### 4.1 Core types

```rust
// outrider-index
struct SymbolId { kind: SymbolKind, qualified_path: String, ordinal: u16 }
enum SymbolKind { Folder, File, Module, Struct, Enum, Trait, Impl, Fn }

struct SymbolNode {
    id: SymbolId,
    name: String,
    byte_range: Option<Range<usize>>,   // None for folders
    measure: u64,                       // line count (leaf) / aggregate (container)
    churn: f32,                         // percentile 0.0–1.0
    children: Vec<SymbolNode>,          // sorted by name, always
}

struct SymbolTree { root: SymbolNode, repo_root: PathBuf }

// outrider-layout
struct CellRange { level: u8, start: u64, len: u64 }   // y-cells within parent
struct NodeLayout {
    id: SymbolId,
    parent: Option<SymbolId>,
    cells: CellRange,        // offset relative to parent's range (hierarchical address)
}
struct WorldLayout { nodes: BTreeMap<SymbolId, NodeLayout>, ratio: u32 /* = 8 */ }
```

`SymbolId.qualified_path` example: `src/auth/session.rs::validate`. Two same-named symbols in one scope (e.g. multiple `impl` blocks) disambiguate by `ordinal` in name order. This ID is the stable identity used by layout keys and history entries; it is deliberately *not* a byte offset (parent invariant 4).

## 5. `outrider-index`

### 5.1 Scan

Walk the repo with the `ignore` crate so `.gitignore` and standard ignore files define what is "real" code (parent §4.5 — never hardcode exclusions). Every non-ignored file contributes its size to folder measure. Only `.rs` files parse and descend further.

### 5.2 Parse

- `tree-sitter` + `tree-sitter-rust`, parallelized with `rayon`, whole repo at startup.
- Extract per file: `mod`, `struct`, `enum`, `trait`, `impl`, `fn` items with byte ranges, nested per the syntax tree. Free functions and methods inside `impl` blocks both become `Fn` nodes.
- Leaf measure = line count of the item's range. File measure = file line count. Folder measure = sum of children.
- Scale assumption: ~2k files / ~500k LOC parses in low single-digit seconds on the dev machine. Larger repos may be slow at startup; acceptable for the skeleton, noted as a post-skeleton concern (lazy/background parse).

### 5.3 Substrate — the four one-way doors

The skeleton honors all four (parent §2.4) at skeleton cost:

1. **Rope buffer:** files rendered at Full fidelity materialize a `ropey::Rope`. Unmaterialized files keep only parse-derived byte ranges. Materialization is capped (LRU, ~64 files) and transparent.
2. **Anchors:** method boxes within a materialized file hold anchors (position markers that survive edits) into its rope, not raw offsets. Skeleton anchor implementation: a lightweight marker list remapped on buffer change; it does not need Zed's `sum_tree` sophistication yet, but the *interface* is anchor-shaped so the implementation can be swapped.
3. **Single buffer:** Full-fidelity rendering reads from the rope — never from a cached string copy.
4. **Incremental parse:** a disk-change event (via `notify` file watching) produces a tree-sitter `InputEdit` + incremental re-parse for materialized files, full re-parse for unmaterialized ones. This is the same code path a future user-edit will take.

### 5.4 Churn

One `git log --numstat --no-renames` pass at startup (subprocess, not `git2` bindings — one invocation, trivially parsed) → commit count per current file path. Convert to within-repo percentile (parent §6.8: continuous fill channels are percentile-relative). Folder churn = sum of descendant counts, then percentile-ranked among folders. Methods inherit their file's churn value. Cache the raw counts keyed by HEAD commit hash in `.outrider/churn-cache.json` (gitignored); invalidate when HEAD moves.

Inspectability (parent invariant 7): the focused node's readout shows `churn: 47 commits · 96th percentile`.

## 6. `outrider-layout`

### 6.1 World model — icicle on a grid

Parent §4.7: depth = horizontal axis, extent ∝ size, expand right = deeper. Therefore packing is **1D per parent** on the y-axis; the x-axis is structural depth. Concretely:

- Subdivision ratio `r = 8`.
- Depth-*d* nodes occupy x-band `[X_d, X_{d+1})` where column width `w_d = w₀ / rᵈ`, and y-cells of height `h_d = h₀ / rᵈ`.
- A node occupies a contiguous run of level-*d* cells (its `CellRange`) inside its parent's range. A parent's run of *n* level-*d* cells subdivides into `n·r` level-*(d+1)* cells for its children.
- Zooming by ~r× brings the next level's cells to screen scale: **the grid is the LOD ladder** and the spatial index (visibility = range query on cell ranges).

### 6.2 Measure pass (post-order)

- Leaf: `cells = max(1, ceil(measure / lines_per_cell(level)))` where `lines_per_cell` is a per-level constant chosen so a typical method ≈ 1–4 cells. Initial values: 32 lines/cell at file level, 4 lines/cell at method level; tune during milestone 3.
- Parent: `cells = ceil((Σ child cells + slack) / r)` with `slack = ceil(0.15 × Σ child cells)`.
- Rounding only ever rounds **up** → sizes are monotonic → the pass terminates in one post-order sweep, no iteration to convergence (parent §4.9).

### 6.3 Arrange pass (pre-order)

- Children placed in **name order** (byte-wise on `name`, ties by `ordinal`) — never by size (parent invariant 3).
- The parent's slack is distributed as gaps: one gap slot after each child, remainder appended at the end. Gaps are the continuity budget: a newly inserted or grown child consumes adjacent slack and displaces only its immediate neighbors; displacement propagates past the parent only when the parent's own total (including slack) overflows its allocation.
- Positions are stored **relative to the parent** (`CellRange.start` is an offset within the parent's range). This is the hierarchical address (parent invariant 10): world coordinates are composed at render time from only the ancestors near the camera, so f32 sees only local deltas — floating origin by construction.

### 6.4 Determinism discipline

- `BTreeMap` / sorted `Vec` only; no `HashMap` iteration anywhere in this crate.
- No floating-point accumulation in cell math — cell arithmetic is integer; floats appear only at render-time composition.
- Property-tested: same `SymbolTree` → byte-identical `WorldLayout`, across runs and processes.

## 7. `outrider` (GPUI app)

### 7.1 Camera and input

| Input | Effect | Class |
|---|---|---|
| Mouse drag / wheel | Pan / continuous zoom | camera |
| Click on box | Set focus (camera does not move) | focus |
| **Right** | Step into first (or last-visited) child; camera follows | focus |
| **Left** | Pop the history stack; camera follows | focus |
| **Up / Down** | Cycle name-ordered siblings; camera follows | focus |
| **Home** | Frame root (overview) | camera |
| **End** | Frame focus at Full fidelity | camera |
| **Enter** | Descend file → method-space (push-in transition) | mode |
| **Esc** | Ascend back to the frozen file layer | mode |
| **Tab** | Disabled (call-graph space out of scope) | — |

- Camera-follow policy (parent §4.6): frame focus plus its parent, panning right as focus deepens. Eased animation ~250 ms; interruptible (a new key retargets mid-flight).
- History for Left is a **simple linear stack** of `SymbolId` (parent §10.2 explicitly permits this simplification; the tree-history of §7.4 is post-skeleton).
- Arrow-stepping and mouse zoom drive the **same camera**; keyboard motion is snapped-to-structure, mouse is free (parent §7.7).

### 7.2 Fidelity ladder (minimal rung content)

Rung selected purely by the node's on-screen pixel height:

| Rung | Pixels | Skeleton content |
|---|---|---|
| Dot | 4–20 | Churn fill color only; below 4px merge into parent tile |
| Label | 20–80 | Truncated name + fill |
| Card | 80–250 | Name + churn readout (`47 · p96`) + measure (lines) |
| Detail | 250–700 | File: method names as sub-labels. Method: full signature |
| Full | >700 | Tree-sitter-highlighted code rendered from the rope |

No sparklines, no glyphs, no LLM text at any rung (all post-skeleton).

### 7.3 Descend transition (bet #2)

- **Enter** (focus on a method, or on a file — then its first method): the file layer **freezes**. Frozen means: the rendered layer is retained with exact box positions and camera state, drawn dimmed and blurred beneath the method plane — never torn down, never re-generated (parent invariant 8).
- The focused method lifts onto the method plane at Detail/Full fidelity; its **peers are the file's other methods** in name order (real data — the skeleton fakes nothing here; call-edge traversal is simply absent). Left/Right/Up/Down operate on that peer set.
- Motion vocabulary: **push-in zoom + blur** (depth). There is no Tab/lateral motion in the skeleton, so the two-vocabulary distinction (parent §7.5) is honored trivially.
- **Esc** reverses the motion exactly and lands on the file with the last-selected method focused; the file layer's camera and scroll state are bit-identical to the moment of descent. Round-trip fidelity is the acceptance test.

### 7.4 Live reload

`notify` watches the repo. On change: re-index the changed file (incremental parse if materialized), rebuild the affected `SymbolTree` subtree, re-run layout, diff `WorldLayout`, and re-render. Unchanged nodes must not move (this is invariant 2 exercised live and the seed of the future buffer-change invalidation machinery, parent §2.5).

## 8. Testing

### 8.1 `outrider-layout` property tests (proptest)

1. **Determinism:** any generated `SymbolTree` laid out twice → byte-identical `WorldLayout` (serialize + compare). Also verified across two separate processes in CI.
2. **Continuity — grow:** grow one leaf's measure by one cell-worth → only that leaf and (at most) its within-parent successors change `CellRange`; nodes outside the parent are untouched unless the parent itself overflowed its slack.
3. **Continuity — insert:** insert one new file into a folder with available slack → only the folder's children between the insertion point and the nearest gap change; everything outside the parent untouched.
4. **Stable ordering:** permuting the sizes of siblings never changes their relative order.
5. **Containment / no overlap:** every child's absolute cell range lies within its parent's; siblings never overlap.

### 8.2 `outrider-index` tests

- Fixture mini-repo (checked into `tests/fixtures/`) → exact expected `SymbolTree` (names, kinds, nesting, measures).
- Churn percentile math on a synthetic history.
- Ignore handling: files matched by `.gitignore` contribute nothing.

### 8.3 The two bets — manual validation protocol

Not automatable; the skeleton exists for this. Protocol: 30 minutes navigating (a) Outrider's own repo, (b) one large foreign Rust repo (`zed` or `ripgrep`), using keyboard-primary navigation. Judgment questions, recorded in a short written verdict:

1. Does arrow-stepping with camera-follow read as *moving through a place* or as *slides changing*?
2. After 10 minutes, can you point (without searching) at where you were three steps ago?
3. Does Enter read as *going deeper into the same thing*? Does Esc read as *coming back to where you were*?
4. Does anything reflow or jump when a file is touched on disk mid-session?

A "slideshow" or "teleport" verdict is a design-level finding and goes back to the parent document before further investment — that is the skeleton doing its job.

## 9. Milestones

0. **GPUI hello-world under WSLg** — verify Vulkan renders on this driver before anything else. Fallback: Windows-native build. Pin the GPUI git revision.
1. **Index:** scan + parse + churn → `SymbolTree` for a real repo (CLI dump, no UI). Fixture tests pass.
2. **Layout:** `SymbolTree` → `WorldLayout`; all five property tests pass headless.
3. **Render + camera:** treemap on screen, dot→label→card rungs, mouse pan/zoom, Home.
4. **Structural navigation:** arrows + history stack + camera-follow; Detail/Full rungs with rope + tree-sitter highlighting. **Bet #1 assessable.**
5. **Descend transition:** Enter/Esc with frozen layer. **Bet #2 assessable.**
6. **Live reload** + the manual validation protocol; written verdict on both bets.

## 10. Risks

| Risk | Mitigation |
|---|---|
| WSLg Vulkan flaky on this driver | Milestone 0 tests it first; Windows-native fallback |
| GPUI API instability (git dependency) | Pin revision; upgrade deliberately, not passively |
| Blur of a frozen layer is expensive or awkward in GPUI | Blur is cosmetic — dim-only is an acceptable fallback; *frozen* (not regenerated) is the requirement |
| Startup parse too slow on huge repos | Out of skeleton scope; noted for post-skeleton lazy/background indexing |
| Continuity property tests hard to specify exactly | The displacement bounds in §8.1 are the spec; if a bound proves wrong, tighten the algorithm, not the test |

## 11. Parent invariants exercised by the skeleton

Of the parent design's §9 checklist: invariants **1, 2, 3** (determinism, continuity, stable ordering — property-tested), **4** (anchors — for materialized files), **5** (single buffer — Full rung renders from the rope), **8** (frozen layers — the descend transition), **9** (world-absolute layout, camera navigation), **10** (floating origin — hierarchical addresses). Invariants 6 and 7 are exercised only trivially (one signal, no LLM); invariant 11 is out of scope with the call-graph space.

## 12. What comes after (non-binding pointer)

If both bets pass: import-graph space (tree-sitter import resolution — reliable without semantic analysis), the signal layer beyond churn, tree-history for Left, and the Tab switch. If either bet fails: back to the parent document, cheaply.
