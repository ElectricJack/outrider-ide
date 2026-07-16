# GLSL Ray-Tracing Extensions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recognize and syntax-highlight `.rgen` and `.rchit` files as GLSL source.

**Architecture:** Extend the existing centralized `SourceLanguage` mapping; all parsing and highlighting reuse the current GLSL paths. Project settings add both extensions to the default source-code category.

**Tech Stack:** Rust 2021, existing Tree-sitter GLSL integration, Cargo tests.

## Global Constraints

- Work directly on `main` as explicitly approved.
- Preserve unrelated uncommitted edits in `rasterize.rs` and `theme.rs`.
- Do not add a grammar, parser, query, or shader-stage-specific behavior.

---

### Task 1: Recognize ray-tracing extensions

**Files:**
- Modify: `crates/outrider-index/src/language.rs`
- Modify: `crates/outrider-index/src/buffer.rs`
- Modify: `crates/outrider/src/project_settings.rs`

**Interfaces:**
- Consumes: existing `SourceLanguage::Glsl` parser/highlighter dispatch.
- Produces: `.rgen` and `.rchit` classification as GLSL and default-enabled code.

- [ ] Add `.rgen`/`.rchit` expectations to classifier, shader highlighting, and project-settings tests.
- [ ] Run the focused tests and confirm they fail because the extensions are not mapped.
- [ ] Add `rgen` and `rchit` to the GLSL extension match and code-category match.
- [ ] Rerun focused tests and confirm they pass.
- [ ] Run `cargo test --workspace`, `cargo build --workspace`, and `git diff --check`.
- [ ] Commit only the three implementation files and this plan.
