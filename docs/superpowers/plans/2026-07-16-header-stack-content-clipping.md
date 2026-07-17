# Header Stack Content Clipping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep descendant nodes from painting through fixed-height stacked ancestor headers at any zoom level.

**Architecture:** Compute a pure screen-space intersection between each projected node and the bottom of its active ancestor-header stack. Store the resulting clip rectangle on the owned paint instruction and apply it as a content mask to surfaces, text, textures, and rings while leaving header painting outside that mask.

**Tech Stack:** Rust, GPUI canvas painting, Cargo unit tests

## Global Constraints

- Preserve fixed screen-space header height and existing header order.
- Do not change world-space layout, camera behavior, focus geometry, or hit-testing.
- Limit automated verification to unit tests; visual acceptance belongs to the user.
- Do not add screenshot or end-to-end tests.

---

### Task 1: Clip Descendant Paint Below Ancestor Headers

**Files:**
- Modify: `crates/outrider/src/paint_model.rs`
- Modify: `crates/outrider/src/treemap.rs:929-979`
- Modify: `crates/outrider/src/treemap.rs:1415-1625`
- Modify: `crates/outrider/src/treemap.rs:4047-4267`
- Test: `crates/outrider/src/treemap.rs:4936-4984`

**Interfaces:**
- Consumes: the active `header_stack: Vec<(u8, f64)>`, projected item `y` and `h`, and existing GPUI `ContentMask` support.
- Produces: `fn descendant_paint_clip(y: f64, h: f64, stack_bottom: f64) -> Option<(f64, f64)>` and `PaintItem { clip_y: f32, clip_h: f32, ... }`.

- [ ] **Step 1: Write failing unit tests for screen-space clipping**

Add tests alongside the existing container-header tests:

```rust
#[test]
fn descendant_paint_clip_starts_below_ancestor_headers() {
    assert_eq!(descendant_paint_clip(100.0, 80.0, 140.0), Some((140.0, 40.0)));
    assert_eq!(descendant_paint_clip(150.0, 30.0, 140.0), Some((150.0, 30.0)));
}

#[test]
fn descendant_paint_clip_omits_nodes_hidden_by_ancestor_headers() {
    assert_eq!(descendant_paint_clip(100.0, 40.0, 140.0), None);
    assert_eq!(descendant_paint_clip(100.0, 20.0, 141.0), None);
}

#[test]
fn descendant_paint_clip_uses_the_full_nested_header_stack() {
    let root_bottom = 100.0 + HEADER;
    let nested_bottom = root_bottom + HEADER;
    assert_eq!(
        descendant_paint_clip(100.0, 100.0, nested_bottom),
        Some((nested_bottom, 100.0 - 2.0 * HEADER)),
    );
}
```

- [ ] **Step 2: Run the focused tests and confirm RED**

Run: `cargo test -p outrider --bin outrider descendant_paint_clip`

Expected: compilation fails because `descendant_paint_clip` does not exist.

- [ ] **Step 3: Add the minimal pure clipping helper**

Add near `container_header_layout`:

```rust
fn descendant_paint_clip(y: f64, h: f64, stack_bottom: f64) -> Option<(f64, f64)> {
    let clip_y = y.max(stack_bottom);
    let bottom = y + h;
    (clip_y < bottom).then_some((clip_y, bottom - clip_y))
}
```

- [ ] **Step 4: Run the focused tests and confirm GREEN**

Run: `cargo test -p outrider --bin outrider descendant_paint_clip`

Expected: all three clipping tests pass.

- [ ] **Step 5: Carry the clip rectangle into each paint instruction**

Add these fields to `PaintItem`:

```rust
pub(crate) clip_y: f32,
pub(crate) clip_h: f32,
```

In `paint_items`, capture the ancestor stack bottom after pruning and before the current item can push its own header. After calculating the item's final paint height, call `descendant_paint_clip`. Skip the paint instruction when it returns `None`; otherwise store the returned `clip_y` and `clip_h`. Use the final expanded focused-leaf height when applicable so clipping does not truncate valid content at the bottom.

- [ ] **Step 6: Apply the clip mask to all node-owned painting**

Build a mask per item:

```rust
let item_mask = ContentMask {
    bounds: Bounds::new(
        point(origin.x + px(item.x), origin.y + px(item.clip_y)),
        size(px(item.w), px(item.clip_h)),
    ),
};
```

Wrap node surface painting, non-header text painting, texture painting, and focus/neighbor ring painting with `window.with_content_mask(Some(item_mask), ...)`. Keep the pinned-header background/text pass outside the descendant mask so headers remain fully visible and stacked. The selected deferred overlay must use the same mask when repainted above regular content.

- [ ] **Step 7: Run unit tests and formatting checks**

Run: `cargo test -p outrider --bin outrider`

Expected: all `outrider` unit tests pass.

Run: `cargo fmt --all -- --check`

Expected: exits successfully with no diff.

- [ ] **Step 8: Commit the implementation**

```bash
git add crates/outrider/src/paint_model.rs crates/outrider/src/treemap.rs docs/superpowers/plans/2026-07-16-header-stack-content-clipping.md
git commit -m "fix: clip nodes below stacked headers"
```
