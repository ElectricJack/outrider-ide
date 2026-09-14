# Reliability, Cache, and Indexing Design

**Date:** 2026-07-13

**Status:** Approved

## Objective

Make Outrider show useful project content sooner, keep cache resource use within explicit limits, avoid mutating opened repositories, and establish cross-platform quality gates. Existing navigation, layout, and visual behavior remain compatible unless this specification explicitly changes them.

## Cache Architecture

Outrider will use a viewport-first, two-tier LRU texture cache.

The memory cache has one global byte limit configured in application settings. Every insertion path, including disk promotion and replacement, updates accounting correctly and immediately evicts least-recently-used entries until usage is within the configured limit. Empty or negative cache entries consume only their actual recorded memory.

The disk cache is partitioned by canonical project identity. Each project has its own byte limit, stored in application settings outside the project repository. Newly opened projects default to 1 GB. Disk writes, reads, eviction, usage reporting, and clearing operate only inside the current project's namespace.

Disk cache keys include:

- Canonical project identity
- Normalized relative source path and symbol identity
- A source-content fingerprint
- Rendering schema version
- Theme or palette fingerprint

Changing source, project, theme, or rendering format therefore produces a different key. Old entries are reclaimed by per-project LRU eviction.

Disk entries use atomic replacement and a versioned header containing validated dimensions and payload length. Invalid, truncated, oversized, or incompatible entries are deleted or ignored without allocating untrusted payload sizes.

## Texture Scheduling

The application does not pre-bake the entire project before presenting it.

Once indexing and layout finish, the project becomes interactive immediately. Texture work is scheduled in this order:

1. Visible nodes, ordered by descending screen area
2. Visible containers' direct children and ancestors needed for compositing
3. Nodes near the viewport that are likely navigation targets
4. Other nodes only when requested by later viewport changes

The queue deduplicates node requests and updates priority when a node is requested again. Each frame performs a bounded amount of work. Replacing an existing texture subtracts its old size before inserting the new value. Container compositing obtains only required direct-child image data and never clones the full cache.

The title bar or status area reports global memory usage and current-project disk usage. Clearing disk cache affects only the current project.

## Loading and Indexing

GPUI opens immediately into a loading shell. Initial open, folder switching, and settings-triggered reindexing use the same background pipeline.

Each load has an identity and cancellation/supersession signal. Starting another load makes all older results stale. Stale workers may finish safe in-flight work, but their results cannot replace the current project.

The pipeline is:

1. Discover eligible paths and inexpensive filesystem metadata.
2. Read each supported source file once in a worker.
3. From that byte buffer, calculate line count, parse symbols and documentation, and generate chunks when needed.
4. Stream unsupported files for line counts without retaining their full contents.
5. Assemble and deduplicate the symbol tree.
6. Add optional Git churn metadata.
7. Compute layout and publish the project to the UI.
8. Begin viewport-prioritized texture scheduling.

File-size safeguards prevent unexpectedly large binary or extensionless files from monopolizing memory. Filtered folders and extensions continue to work as they do today. Internal and serialized symbol paths use normalized `/` separators; filesystem operations use `Path` and `PathBuf` values.

## Churn Cache and Repository Integrity

Opening a project must not create or modify files inside that project.

Git churn data moves from `.outrider/churn-cache.json` to the operating system cache directory. Its namespace is derived from the canonical project identity, and records remain keyed by Git HEAD. A cache miss or corrupt record triggers recomputation.

Git command failures, missing Git, repositories without commits, read-only cache directories, and churn-cache write failures degrade to missing churn metadata. They produce a non-blocking warning and never prevent the project from opening.

## Settings and Error Handling

Application settings contain:

- Global memory-cache allowance in MB
- A map from canonical project identity to disk-cache allowance in bytes
- Existing filters and welcome-screen preferences

The current project's disk allowance is editable in the settings UI and defaults to 1 GB when absent. Numeric values are validated and bounded before multiplication or allocation.

Malformed settings are backed up and replaced with defaults. Read, parse, validation, directory-creation, and save failures are represented as user-visible non-blocking notifications. Reindex failures leave the existing project active and display the error. Console logging may supplement but not replace in-app feedback.

## Application Boundaries

`TreemapView` remains the top-level coordinator, but responsibilities move out of `treemap.rs` into focused modules:

- `project_loader`: background pipeline state, progress, cancellation, and result application
- `interaction`: mouse and keyboard event handling
- `navigation`: navigation history and focus transitions
- `overlays`: welcome, settings, context-menu, loading, and notification views
- `paint_model`: conversion from visible world nodes to renderable paint data
- `rasterize`: texture generation, scheduling, and bounded memory/disk cache behavior

The split follows behavior boundaries rather than arbitrary file size. Public interfaces remain minimal, and existing controls and layout semantics remain unchanged.

## Verification and Release Quality

Regression coverage includes:

- Cross-project disk-cache isolation
- Source, theme, and schema invalidation
- Corrupt/truncated cache entries
- Memory and disk LRU enforcement
- Deduplicated viewport-priority scheduling
- Replacement accounting
- Read-only repositories and cache directories
- Load cancellation and stale-result rejection
- Portable relative-path handling
- Existing layout, navigation, parsing, and rendering contracts

Continuous integration runs on Windows, Linux, and macOS:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The existing Windows path assertion becomes platform-neutral without weakening the normalized internal-path contract. The workspace is formatted and all Clippy warnings are resolved. An MIT `LICENSE` file is added to match the README. Documentation explains cache locations, defaults, limits, eviction, and the guarantee that Outrider does not write into opened repositories.

## Success Criteria

- The application window appears before repository indexing finishes.
- The project becomes usable after tree and layout completion, without waiting for full texture generation.
- Visible content receives texture priority over off-screen content.
- Global memory and per-project disk limits are enforced after every cache operation.
- A new project receives a 1 GB disk-cache allowance.
- Cache entries cannot leak across projects or survive relevant source/render changes.
- Opening and analyzing a repository creates no files inside it.
- Background failures are visible without discarding a previously loaded project.
- Formatting, Clippy, and workspace tests pass locally and in the three-platform CI matrix.

## Non-Goals

- Editing source files inside Outrider
- File watching or incremental tree-sitter edits
- Persistent pre-baking of an entire project
- Changing treemap packing, navigation semantics, syntax colors, or supported languages
