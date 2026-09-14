# Readable Zoomed Headers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep every rendered container header at least one readable screen-space line tall while zooming out.

**Architecture:** Add one pure `container_header_px` helper in `treemap.rs` and use it everywhere that computes the zoomed container-header height. The helper clamps the existing three-line natural header height to `HEADER`, keeping paint preparation and pinned-stack prediction consistent without changing draw tiers or text size.

**Tech Stack:** Rust, GPUI, existing Outrider treemap unit tests, Cargo.

## Global Constraints

- Card, Detail, and Full container headers never render below the natural one-line `HEADER` height.
- Header names retain the existing 12 px font.
- Metadata rows may be clipped as zoom decreases; only the name line is guaranteed.
- Label-tier behavior remains a single centered name.
- Dot-tier and merged nodes remain text-free.
- Paint preparation and pinned ancestor-stack prediction must use the same height calculation.
- No new dependencies and no unrelated rendering or layout changes.

---

### Task 1: Clamp Rendered Container Headers to One Line

**Files:**
- Modify: `crates/outrider/src/treemap.rs`
- Test: `crates/outrider/src/treemap.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `content::HEADER`, `content::LINE_STEP`, and camera zoom as `f64`.
- Produces: `fn container_header_px(zoom: f64) -> f64`, used by `paint_items` and `pinned_stack_h`.

- [ ] **Step 1: Write the failing helper regression**

Add this test to the existing `treemap::tests` module and import `container_header_px` through the module's `use super::{...}` list:

```rust
#[test]
fn container_header_never_collapses_below_one_line() {
    let natural = HEADER + 2.0 * LINE_STEP;
    assert!((container_header_px(1.0) - natural).abs() < 1e-9);
    assert!((container_header_px(0.5) - natural * 0.5).abs() < 1e-9);
    assert!((container_header_px(0.1) - HEADER).abs() < 1e-9);
    assert!((container_header_px(0.0) - HEADER).abs() < 1e-9);
}
```

- [ ] **Step 2: Run the helper test and verify RED**

Run:

```powershell
cargo test -p outrider treemap::tests::container_header_never_collapses_below_one_line
```

Expected: compilation fails because `container_header_px` does not exist.

- [ ] **Step 3: Add the minimal shared height helper**

Place this pure helper immediately above `pinned_stack_h`:

```rust
fn container_header_px(zoom: f64) -> f64 {
    ((HEADER + 2.0 * LINE_STEP) * zoom.min(1.0)).max(HEADER)
}
```

Replace the height calculation in `pinned_stack_h`:

```rust
let hdr = container_header_px(cam.zoom);
```

Replace the height calculation in `TreemapView::paint_items`:

```rust
let ch_px = container_header_px(camera.zoom);
```

Do not change the existing `header_bg_h` clipping expression, header font size, rung selection, or Dot/Label behavior.

- [ ] **Step 4: Add the pinned-stack regression**

Extend `pinned_stack_h_stacks_named_offscreen_ancestors_and_skips_unnamed` after its zoom-0.5 assertion:

```rust
let cam = Camera {
    center_x: 0.0,
    center_y: 0.0,
    zoom: 0.1,
};
let h = pinned_stack_h(&focus, &layout, &index, &cam, 800.0, 600.0);
assert!((h - 2.0 * HEADER).abs() < 1e-9);
```

This must fail before Step 3's helper is wired into `pinned_stack_h` and pass afterward.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run:

```powershell
cargo test -p outrider treemap::tests::container_header_never_collapses_below_one_line
cargo test -p outrider treemap::tests::pinned_stack_h
```

Expected: all selected tests pass.

- [ ] **Step 6: Run the complete verification gate**

Run:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git diff --check
```

Expected: every command exits 0.

- [ ] **Step 7: Commit the implementation**

```powershell
git add crates/outrider/src/treemap.rs
git commit -m "fix: preserve readable headers while zooming out"
```
