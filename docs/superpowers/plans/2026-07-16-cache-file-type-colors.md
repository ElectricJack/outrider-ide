# Cache File-Type Color Consistency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure live treemap nodes and cached folder textures use identical file-type background colors and invalidate textures baked with the old semantics.

**Architecture:** Centralize node tint and box-kind classification in the theme layer so both live painting and container rasterization consume the same decisions. Cover the behavior with regression tests that compare Python file, item, and leaf classifications across both paths, then bump the render schema for disk-cache invalidation.

**Tech Stack:** Rust, GPUI, Cargo unit tests.

## Global Constraints

- Preserve the existing extension palette and `File`/`Item`/`Leaf` blend strengths.
- Folder semantic tints remain unchanged.
- Live and cached rendering must use one shared classification implementation, not duplicated match blocks.
- Increment `RENDER_SCHEMA_VERSION` from `2` to `3`.
- Make no unrelated refactors or behavior changes.

---

### Task 1: Shared Classification and Cache Invalidation

**Files:**
- Modify: `crates/outrider/src/theme.rs`
- Modify: `crates/outrider/src/treemap.rs`
- Modify: `crates/outrider/src/rasterize.rs`
- Test: unit tests in the files above

**Interfaces:**
- Consumes: `SymbolNode`, `SymbolKind`, `BoxKind`, `BoxTint`, `extension_tint`.
- Produces: shared theme helpers for deriving a node's `BoxKind` and `BoxTint`, used by both live and cached rendering.

- [ ] **Step 1: Write failing regression tests**

Add tests proving Python file nodes, Python item containers, and Python leaf nodes resolve to the same extension tint and correct brightness tier in the shared path. Add a rasterizer-facing test that would fail while its old local classifier is retained. Assert the render schema is `3`.

- [ ] **Step 2: Run focused tests and verify RED**

Run: `cargo test -p outrider theme::tests rasterize::tests treemap::tests`

Expected: the new shared-classification or schema assertions fail for the missing implementation.

- [ ] **Step 3: Implement the minimal shared classification**

Move extension extraction and node classification into `theme.rs`. Use the shared helpers from `treemap.rs` and `rasterize.rs`; delete their duplicated classifiers. Ensure cached container rasterization classifies item containers as `BoxKind::Item`. Change `RENDER_SCHEMA_VERSION` to `3`.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p outrider theme::tests rasterize::tests treemap::tests`

Expected: all focused tests pass.

- [ ] **Step 5: Verify the crate**

Run: `cargo test -p outrider`

Run: `cargo check -p outrider`

Expected: both commands exit successfully with no new warnings.

- [ ] **Step 6: Self-review**

Inspect `git diff --check` and `git diff`, confirming only the three scoped source files and tests changed (aside from this plan), classification is not duplicated, and no unrelated user changes were overwritten. Do not commit unless explicitly requested by the user.
