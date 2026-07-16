# Semantic Zones and Skyline Packing Design

**Status:** Approved in conversation on 2026-07-16

## 1. Goal

Reduce unused space in repository and folder layouts while improving logical locality. Immediate children of each real folder are grouped into invisible role-based zones, skyline-packed within each zone, and then packed as non-overlapping zone blocks. The real folder hierarchy, symbol identities, navigation model, and rendering model remain unchanged.

## 2. Scope

This change applies semantic zoning and skyline packing only when the node being arranged is a `SymbolKind::Folder`.

Non-folder nodes retain the existing multi-candidate shelf algorithm. In particular, C/C++, Markdown, chunk, and other source-ordered file contents retain their current byte-order behavior. No rectangle is rotated.

The public API remains:

```rust
pub fn pack(tree: &SymbolTree, cfg: &PackConfig) -> PackLayout
```

There are no new dependencies, configuration fields, serialized values, renderer nodes, navigation nodes, or user-facing zone controls.

## 3. Structural Invariants

1. A node keeps its existing `SymbolId`, parent, and children.
2. No node crosses a real folder boundary.
3. A semantic zone is temporary layout state, never a `SymbolNode` or `PackLayout` entry.
4. Zone blocks cannot receive focus, render labels, affect hit testing, or appear in navigation.
5. Every child rectangle remains contained by its real parent and cannot overlap a sibling.
6. Packing is pure and deterministic. It performs no I/O and uses no randomness, clocks, environment values, or hash iteration order.
7. A content-only edit cannot change semantic classification. Names, paths, extensions, and tree membership may change classification.

## 4. Semantic Roles

The private role enum is ordered as follows:

```rust
enum SemanticRole {
    Source,
    Test,
    Example,
    ShaderAsset,
    Docs,
    Generated,
}
```

This order is the zone insertion order. `Source` also absorbs ordinary build/configuration files and mixed content so that weak classifications do not create miscellaneous micro-zones.

### 4.1 Normalization

Names are matched case-insensitively. A basename is split into lowercase alphanumeric tokens at punctuation, whitespace, underscores, and hyphens. Classification checks both those tokens and a collapsed alphanumeric basename, so `third_party`, `third-party`, and `thirdparty` match the same signal. File extension matching is also case-insensitive.

### 4.2 Strong classification

An explicit signal produces a strong classification. If several signals match, precedence is:

1. `Generated`
2. `Test`
3. `Example`
4. `ShaderAsset`
5. `Docs`
6. `Source`

Folder token signals are:

- `Generated`: `generated`, `vendor`, `vendors`, `thirdparty`, `external`, `extern`, `deps`, `dependencies`
- `Test`: `test`, `tests`, `testing`, `spec`, `specs`
- `Example`: `example`, `examples`, `demo`, `demos`, `sample`, `samples`
- `ShaderAsset`: `shader`, `shaders`, `asset`, `assets`, `resource`, `resources`, `texture`, `textures`, `model`, `models`, `media`
- `Docs`: `doc`, `docs`, `documentation`
- `Source`: `src`, `source`, `sources`, `include`, `includes`, `lib`, `libs`, `core`

File signals are:

- `Generated`: a `generated`, `vendor`, or `thirdparty` name token
- `Test`: a `test`, `tests`, `spec`, or `specs` token, including common names such as `foo_test.cpp`, `foo.tests.rs`, and `bar.spec.ts`
- `Example`: an `example`, `demo`, or `sample` token
- `ShaderAsset`: shader extensions `glsl`, `vert`, `frag`, `geom`, `comp`, `tesc`, `tese`, `rgen`, `rchit`, `rmiss`, `rahit`, `rint`, `rcall`, `mesh`, `task`, `hlsl`, `fx`, `fxh`, `metal`, `wgsl`, `spv`; image/model/media extensions `png`, `jpg`, `jpeg`, `gif`, `bmp`, `tga`, `hdr`, `exr`, `dds`, `ktx`, `ktx2`, `svg`, `ico`, `obj`, `fbx`, `gltf`, `glb`, `dae`, `ply`, `stl`, `wav`, `mp3`, `ogg`, `flac`, `mp4`, `mov`, `webm`
- `Docs`: extensions `md`, `markdown`, `txt`, and `rst`
- `Source`: no explicit signal

The shader/asset extension helper is private to zoning and does not change language detection or filtering.

### 4.3 Descendant profiles and inheritance

A bottom-up analysis pass records descendant file-role counts for every folder.

- An explicitly named folder uses its strong classification regardless of its descendants.
- Otherwise, if one non-`Source` role satisfies `role_file_count * 10 >= total_file_count * 7`, the folder receives that role strongly. Two different roles cannot both meet this threshold. An ordinary source-file majority is deliberately not strong: a generic `unit` folder below an explicit `tests` folder must inherit `Test`, not override its context with default `Source`.
- An empty or mixed folder has weak `Source` classification.
- A file without an explicit signal has weak `Source` classification.

When arranging a folder, a weak child inherits the effective role of its parent folder. This keeps ordinary files inside a `tests` folder in the test zone while still allowing an explicit `docs` or `shaders` child to form its own zone. The root context is `Source`.

Profiles are computed once for the full tree. The layout pass does not repeatedly walk descendant subtrees to classify siblings.

## 5. Folder Layout Pipeline

For every real folder, after its children have been recursively measured:

1. Determine each immediate child's effective role from its profile and the folder context.
2. Partition children into non-empty role groups in `SemanticRole` order.
3. Within each role, retain the existing folder ordering: height descending, then name bytes, then ordinal.
4. Skyline-pack each role independently into a local invisible block.
5. Skyline-pack the resulting role blocks in `SemanticRole` order.
6. Offset each real child's relative position by its role block's position.
7. Add the existing real container header, inner margin, and outer padding.

A one-child role is placed directly at `(0, 0)` with its natural dimensions. Empty roles create no block.

Because role blocks are treated as solid rectangles during the second packing stage, a later role cannot backfill a hole inside an earlier role. Each semantic role therefore remains spatially contiguous even though it has no visible boundary or label.

The normal sibling `gap` applies between children inside a role and between role blocks. There is no additional zone-specific spacing.

## 6. Skyline Algorithm

### 6.1 Inputs and output

The private skyline primitive accepts an ordered slice of stable item keys and fixed `(width, height)` rectangles plus `gap` and target `aspect`. It returns deterministic relative positions and actual occupied bounds.

Rectangles are never rotated or resized.

### 6.2 Candidate widths

Let:

```text
baseline_width = max(widest_child, sqrt(padded_child_area * aspect))
```

`padded_child_area` uses `(width + gap) * (height + gap)` so the estimate accounts for sibling separation. The baseline is evaluated first. A constant list of multipliers around the baseline is then evaluated, reusing the multi-candidate philosophy of the shelf packer:

```text
0.5, 0.625, 0.75, 0.875, 1.0, 1.125, 1.5, 2.0, 3.0
```

Every width is clamped to the widest padded rectangle. A candidate width equal to any previously evaluated width, including the baseline, is skipped.

### 6.3 Placement within one candidate

The skyline is a sorted vector of horizontal segments covering the candidate width. For each rectangle in input order:

1. Consider `x = 0` and every current skyline segment boundary where the padded rectangle fits within the candidate width.
2. For each `x`, compute `y` as the maximum skyline height across the padded rectangle's horizontal span.
3. Select the placement with the lexicographically smallest `(y + padded_height, y, x)` tuple.
4. Raise the skyline over the occupied span to `y + padded_height`.
5. Split segments at the rectangle edges and merge adjacent segments with equal heights.

Real child positions use `(x, y)`. Candidate occupied bounds use the real, unpadded right and bottom edges, so trailing gap is not counted as content.

### 6.4 Candidate scoring and ties

For occupied bounds `(w, h)` and target aspect `a`, score the candidate by the area of the smallest aspect-`a` envelope that contains it:

```text
envelope_width = max(w, h * a)
score = envelope_width * (envelope_width / a)
```

The baseline candidate wins equal scores. Otherwise the first multiplier candidate with a strictly lower `f64::total_cmp` score wins. Placement ties use the lowest/leftmost tuple above. Stable input order is the final implicit tie-breaker.

## 7. Stability and Reflow

The same tree and configuration always produce byte-for-byte equal rectangle values.

Growing or shrinking a node may change skyline placement or the winning candidate width for its containing folder. That folder and its ancestor chain may therefore reflow. Unrelated subtrees retain their internal relative geometry, although their absolute world position may move when an ancestor repacks.

Adding, removing, or renaming files may change a folder's descendant majority and move that folder between roles. This is intentional. Editing file contents without changing tree structure or names cannot do so.

## 8. Code Organization

The layout crate gains two private modules:

- `crates/outrider-layout/src/zones.rs`: `SemanticRole`, normalized token matching, strong/weak classification, descendant profiles, and effective-role inheritance.
- `crates/outrider-layout/src/skyline.rs`: candidate-width generation, skyline segment placement, scoring, and packed bounds.

`crates/outrider-layout/src/pack.rs` continues to own recursive sizing, ordering, non-folder shelf layout, folder-zone orchestration, and absolute positioning.

`crates/outrider-layout/src/lib.rs` declares the private modules and keeps its existing public exports unchanged.

## 9. Validation

Unit and integration coverage must include:

1. Table-driven explicit folder and file classification for every role.
2. Classification precedence when a name contains tokens from multiple roles.
3. Seventy-percent descendant inheritance, mixed-folder fallback, empty folders, and weak-child parent-role inheritance.
4. Proof that the `SymbolTree` and output ID set are unchanged by zoning.
5. Skyline placement that fills a cavity the current next-fit shelf cannot fill.
6. No overlap and exact containment for varied fixed rectangles.
7. Role-block contiguity: later roles never occupy space inside an earlier role's block.
8. Determinism across repeated runs and explicit equal-score behavior.
9. Exact preservation of representative source-ordered file layouts.
10. A screenshot-shaped fixture containing one very large project and many small projects whose aspect-envelope footprint is strictly smaller than the multi-candidate shelf result.
11. A large synthetic folder that completes and satisfies containment/no-overlap, guarding against accidental explosive allocation or iteration. The test does not impose a brittle wall-clock threshold.
12. All existing layout and workspace tests.

## 10. Acceptance Criteria

The change is accepted when:

- Folder hierarchy and `PackLayout` identity membership are unchanged.
- Folder children are grouped by the specified implicit semantic roles.
- Role blocks and their children never overlap.
- The screenshot-shaped fixture is measurably denser than the current shelf implementation.
- Non-folder layout behavior and source-order invariants remain intact.
- Results are deterministic.
- The public API and dependency graph are unchanged.
- Layout-crate formatting and strict Clippy pass, and the full workspace test suite passes.

## 11. Non-Goals

- Dependency-, call-graph-, or Git-co-change clustering
- Pairing individual tests with the source symbols they exercise
- Moving nodes across real folder boundaries
- Visible zone headers, labels, borders, tinting, or extra gutters
- Rectangle rotation or source-code reflow
- User-configurable zone names, thresholds, or ordering in this iteration
- Replacing non-folder source-order shelf layout
