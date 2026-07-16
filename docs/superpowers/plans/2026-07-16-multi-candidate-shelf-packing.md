# Multi-Candidate Shelf Packing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce empty space while retaining Outrider's deterministic, ordered, column-first shelf layout.

**Architecture:** Keep the existing bottom-up measure and top-down absolute-position passes. Within each non-leaf node, generate a bounded set of candidate shelf heights around the current area-derived estimate, simulate the ordered children for each height, score each resulting bounding box by the area of the smallest rectangle with the configured aspect ratio that can contain it, and place children with the deterministic winner.

**Tech Stack:** Rust 2021, `outrider-layout`, built-in unit tests, Cargo.

## Global Constraints

- Preserve the existing public `pack(tree, cfg) -> PackLayout` API and `PackConfig` fields.
- Preserve semantic, documentation, and source-order sorting before packing.
- Keep the algorithm pure and deterministic; do not use randomness, clocks, I/O, or hash iteration order.
- Keep candidate evaluation bounded at a constant number of simulations per container.
- Never select a target height shorter than the tallest child.
- Break equal-score ties by candidate order so results are reproducible.
- Do not add dependencies.

---

### Task 1: Deterministic candidate selection

**Files:**
- Modify: `crates/outrider-layout/src/pack.rs:139-211`
- Test: `crates/outrider-layout/src/pack.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: ordered child dimensions `&[(f64, f64)]`, `gap: f64`, and configured `aspect: f64`.
- Produces: private `choose_target_height(sizes: &[(f64, f64)], gap: f64, aspect: f64) -> f64` used by `size`.
- Preserves: `pack`, `PackConfig`, `PackLayout`, `Rect`, child ordering, relative-position format, and top-down absolute positioning.

- [x] **Step 1: Write a failing candidate-selection test**

Add a unit test with one wide/tall child and four narrower/shorter children. Assert that `choose_target_height` selects a height above the single-estimate baseline and that simulating the selected shelf produces a smaller aspect-constrained envelope than the baseline. This fixture represents the large-project-plus-small-project gap visible in repository layouts.

```rust
#[test]
fn candidate_height_reduces_aspect_envelope_for_mixed_child_shapes() {
    let sizes = vec![
        (1400.0, 700.0),
        (640.0, 100.0),
        (640.0, 100.0),
        (640.0, 100.0),
        (640.0, 100.0),
    ];
    let area: f64 = sizes.iter().map(|(w, h)| w * h).sum();
    let baseline = 700.0_f64.max((area / 1.6).sqrt());
    let selected = choose_target_height(&sizes, 8.0, 1.6);
    let baseline_bounds = shelf_bounds(&sizes, 8.0, baseline);
    let selected_bounds = shelf_bounds(&sizes, 8.0, selected);

    assert!(selected > baseline);
    assert!(
        aspect_envelope_area(selected_bounds, 1.6)
            < aspect_envelope_area(baseline_bounds, 1.6)
    );
}
```

- [x] **Step 2: Run the test and verify RED**

Run: `cargo test -p outrider-layout candidate_height_reduces_aspect_envelope_for_mixed_child_shapes`

Expected: compilation fails because `choose_target_height`, `shelf_bounds`, and `aspect_envelope_area` do not exist.

- [x] **Step 3: Implement bounded shelf simulation and scoring**

In `pack.rs`, add private helpers:

```rust
const TARGET_HEIGHT_FACTORS: [f64; 9] = [0.5, 0.625, 0.75, 0.875, 1.0, 1.125, 1.5, 2.0, 3.0];

fn shelf_bounds(sizes: &[(f64, f64)], gap: f64, target_h: f64) -> (f64, f64) {
    let (mut x, mut y, mut col_w, mut content_h) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    for &(w, h) in sizes {
        if y > 0.0 && y + h > target_h {
            x += col_w + gap;
            y = 0.0;
            col_w = 0.0;
        }
        col_w = col_w.max(w);
        content_h = content_h.max(y + h);
        y += h + gap;
    }
    (x + col_w, content_h)
}

fn aspect_envelope_area((w, h): (f64, f64), aspect: f64) -> f64 {
    let envelope_w = w.max(h * aspect);
    envelope_w * (envelope_w / aspect)
}

fn choose_target_height(sizes: &[(f64, f64)], gap: f64, aspect: f64) -> f64 {
    if let [(_, height)] = sizes {
        return *height;
    }
    let area: f64 = sizes.iter().map(|&(w, h)| w * h).sum();
    let tallest = sizes.iter().map(|&(_, h)| h).fold(0.0, f64::max);
    let baseline = tallest.max((area / aspect).sqrt());
    let mut best_height = baseline;
    let mut best_score = aspect_envelope_area(shelf_bounds(sizes, gap, baseline), aspect);
    let mut previous = None;

    for factor in TARGET_HEIGHT_FACTORS {
        let candidate = tallest.max(baseline * factor);
        if previous == Some(candidate) {
            continue;
        }
        previous = Some(candidate);
        let score = aspect_envelope_area(shelf_bounds(sizes, gap, candidate), aspect);
        if score.total_cmp(&best_score).is_lt() {
            best_height = candidate;
            best_score = score;
        }
    }
    best_height
}
```

Use actual shelf bounds in scoring, including inter-child gaps but excluding the container header and outer margins because those are constant across candidates. The simulation must follow the exact same `y > 0 && y + h > target_h` wrap condition as placement.

- [x] **Step 4: Connect selection to placement**

Replace the single target calculation in `size` with dimension collection and `choose_target_height`:

```rust
let sizes: Vec<(f64, f64)> = order.iter().map(|(_, size, _)| *size).collect();
let target_h = choose_target_height(&sizes, cfg.gap, cfg.aspect);
```

Leave the existing placement loop intact so candidate simulation and real placement share the established shelf semantics.

- [x] **Step 5: Run the focused test and verify GREEN**

Run: `cargo test -p outrider-layout candidate_height_reduces_aspect_envelope_for_mixed_child_shapes`

Expected: one test passes.

- [x] **Step 6: Add determinism and safety coverage**

Add tests asserting that candidate selection returns the tallest height for one child, returns the same value repeatedly, and never returns less than the tallest child. Retain the existing end-to-end `deterministic` test.

- [x] **Step 7: Update exact-layout expectations only where the improved winner intentionally changes layout**

Run the full layout tests, inspect every changed rectangle, and update exact assertions only when they reflect the winning candidate under the documented score. Do not weaken ordering, containment, or stability assertions.

Run: `cargo test -p outrider-layout`

Expected: all layout tests pass.

- [x] **Step 8: Format and verify the workspace**

Run: `cargo fmt --all -- --check`

Expected: success with no output.

Run: `cargo test --workspace`

Expected: all workspace unit and documentation tests pass.

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Expected: success with no warnings.

Verification note: `cargo test --workspace`, `cargo fmt -p outrider-layout -- --check`,
`cargo clippy -p outrider-layout --all-targets --no-deps -- -D warnings`, and
`git diff --check` pass. Workspace-wide strict formatting and dependency linting
remain blocked by pre-existing issues in `outrider-index/src/language.rs:44` and
`outrider-index/src/call_graph.rs:497`; no unrelated files were changed.

- [x] **Step 9: Commit the feature**

```text
git add crates/outrider-layout/src/pack.rs docs/superpowers/plans/2026-07-16-multi-candidate-shelf-packing.md
git commit -m "feat: evaluate multiple shelf packing heights"
```
