# Focused Node Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Widen leaf page boxes from 480→640 world units, and expand/wrap text in focused nodes instead of truncating.

**Architecture:** Two changes: (1) bump the `PAGE_W` constant and fix all tests, (2) render-time expansion of focused leaf nodes with content-driven width + line wrapping for both focused leaves and containers. Packed layout is never modified — expansion is purely visual.

**Tech Stack:** Rust 2021, `outrider-layout` (pack algorithm), `outrider` (render/paint), `gpui` (UI framework)

**Source spec:** `docs/superpowers/specs/2026-07-12-focused-node-expansion-design.md`

## Global Constraints

- Layout is pure and deterministic — `pack()` is called once, `PackLayout` is never modified at render time.
- Focused expansion is render-time only: wider `PaintItem` quads + wrapped text, no layout changes.
- Expansion width clamped to `[PAGE_W, 2.0 * PAGE_W]` (640–1280 world units).
- Focused leaf paints last (on top of overlapped neighbors).
- `HighlightSpan` has fields `range: Range<usize>` and `kind: HighlightKind`.
- Existing tests for `wrap_doc` use the `wrap_w(budget)` helper: `12.0 + budget as f64 * 0.62 * 12.0`.

---

### Task 1: PAGE_W bump (480 → 640)

**Files:**
- Modify: `crates/outrider/src/world.rs:26` — constant
- Modify: `crates/outrider-layout/src/pack.rs:231` — test `cfg()` helper
- Modify: `crates/outrider-layout/src/pack.rs:297-441` — test assertions
- Modify: `crates/outrider/src/world.rs:318` — test `pack_cfg()` helper
- Modify: `crates/outrider/src/world.rs:345-427` — test assertions
- Modify: `crates/outrider/src/treemap.rs:2469-2622` — test label_w values and fixture width

**Interfaces:**
- Produces: `world::PAGE_W = 640.0` — used by `pack_config()`, `leaf_tex_rect()`, and Task 2's `focused_width()`.

- [ ] **Step 1: Change `PAGE_W` constant**

In `crates/outrider/src/world.rs`, line 26:

```rust
pub const PAGE_W: f64 = 640.0;
```

- [ ] **Step 2: Update `pack.rs` test config helper**

In `crates/outrider-layout/src/pack.rs`, the test `cfg()` function at line 231:

```rust
fn cfg() -> PackConfig {
    PackConfig {
        page_w: 640.0,
        line_step: 15.6,
        header: 20.8,
        container_header: 52.0,
        bottom_pad: 6.0,
        gap: 8.0,
        aspect: 1.6,
    }
}
```

- [ ] **Step 3: Update `pack.rs` test `worked_example_exact_rects`**

Recomputed values with `page_w=640` (heights unchanged, widths and x-offsets shift):

```rust
#[test]
fn worked_example_exact_rects() {
    let p = pack(&worked_example(), &cfg());
    assert_eq!(p.rects.len(), 5);
    assert_rect(rect(&p, "a.rs"), 8.0, 60.0, 640.0, 1602.4);
    assert_rect(rect(&p, "b.rs::f"), 664.0, 120.0, 640.0, 198.4);
    assert_rect(rect(&p, "b.rs::g"), 664.0, 326.4, 640.0, 58.0);
    assert_rect(rect(&p, "b.rs"), 656.0, 60.0, 656.0, 332.4);
    assert_rect(rect(&p, ""), 0.0, 0.0, 1320.0, 1670.4);
}
```

- [ ] **Step 4: Update `pack.rs` test `children_placed_tallest_first_names_break_ties`**

Only `a.x` changes (second column x-offset):

```rust
close(a.x, 656.0);
```

- [ ] **Step 5: Update `pack.rs` test `sibling_subtree_stable_under_edit`**

After editing f to measure=50, f and g repack inside b.rs:

```rust
assert_rect(rect(&after, "b.rs::f"), 664.0, 120.0, 640.0, 822.4);
assert_rect(rect(&after, "b.rs"), 656.0, 60.0, 1304.0, 890.4);
let g = rect(&after, "b.rs::g");
close(g.x, 1312.0);
close(g.y, 120.0);
```

- [ ] **Step 6: Update `pack.rs` test `wide_child_sets_the_floor_for_target_width`**

Leaf width and root width change:

```rust
assert_rect(rect(&p, "one.rs"), 8.0, 60.0, 640.0, 58.0);
assert_rect(rect(&p, ""), 0.0, 0.0, 656.0, 126.0);
```

Update the comment too: `// single 640×58 child: content 640×58 → root 656 × 126.0`

- [ ] **Step 7: Update `pack.rs` test `columns_fill_down_then_wrap_right`**

All four leaf widths change, c4 and root x-offsets shift:

```rust
assert_rect(rect(&p, "c1.rs"), 8.0, 60.0, 640.0, 120.4);
assert_rect(rect(&p, "c2.rs"), 8.0, 188.4, 640.0, 120.4);
assert_rect(rect(&p, "c3.rs"), 8.0, 316.8, 640.0, 120.4);
assert_rect(rect(&p, "c4.rs"), 656.0, 60.0, 640.0, 120.4);
assert_rect(rect(&p, ""), 0.0, 0.0, 1304.0, 445.2);
```

- [ ] **Step 8: Update `pack.rs` test `doc_file_sinks_below_source_in_folder`**

README.md wraps to second column at new offset:

```rust
close(r.x, 656.0);
```

- [ ] **Step 9: Run pack.rs tests**

Run: `cargo test -p outrider-layout`

Expected: all tests pass.

- [ ] **Step 10: Update `world.rs` test config helper**

In `crates/outrider/src/world.rs`, the test `pack_cfg()` at line 318:

```rust
fn pack_cfg() -> outrider_layout::PackConfig {
    outrider_layout::PackConfig {
        page_w: 640.0,
        line_step: 15.6,
        header: 20.8,
        container_header: 52.0,
        bottom_pad: 6.0,
        gap: 8.0,
        aspect: 1.6,
    }
}
```

- [ ] **Step 11: Update `world.rs` test `packed_walk_zoom_one_clips_and_keeps_unclipped_fields`**

Camera must center on g's new position. g center: (664 + 320, 326.4 + 29) = (984, 355.4).

```rust
let cam = Camera { center_x: 984.0, center_y: 355.4, zoom: 1.0 };
```

Update a.rs assertions (further left due to wider layout):

```rust
let a = &items[1];
close(a.px.x, -2.0);
close(a.left, -576.0);
close(a.px.w, 66.0);
close(a.px.y, 4.6);
close(a.top, 4.6);
close(a.px.h, 597.4);
close(a.full_h, 1602.4);
assert!((a.label_w - 640.0).abs() < 1e-9);
```

Update root left:

```rust
close(items[0].top, -55.4);
close(items[0].left, -584.0);
close(items[0].px.x, -2.0);
```

Update g assertions:

```rust
let g = &items[4];
close(g.px.x, 80.0);
close(g.px.y, 271.0);
close(g.px.w, 640.0);
close(g.px.h, 58.0);
close(g.full_h, 58.0);
```

Hit-test assertions stay unchanged (400, 290 and 400, 100 are still inside g and f respectively).

- [ ] **Step 12: Update `world.rs` test `packed_walk_prunes_offscreen_subtrees`**

a.rs right edge is now 8+640=648. Push camera right so viewport left > 648:

```rust
// panned right so only b.rs's column of the map remains: a.rs's
// right edge (648) is left of the viewport's world-left edge
// (1060 − 400 = 660) → a.rs pruned, b.rs subtree survives
let cam = Camera { center_x: 1060.0, center_y: 293.0, zoom: 1.0 };
```

- [ ] **Step 13: Update `treemap.rs` test label_w values**

In `crates/outrider/src/treemap.rs`:

Line 2469 — `leaf_text_body_paints_code_without_duplicate_signature`:
```rust
let body =
    leaf_text_body(&leaf, 0.0, 0.0, natural, 640.0, 600.0, &mut mgr, &file_symbols);
```

Line 2490 — `leaf_text_body_scales_uniformly_past_one` (zoom 2×):
```rust
let body = leaf_text_body(
    &leaf, 0.0, 0.0, 2.0 * natural, 1280.0, 100_000.0, &mut mgr, &file_symbols,
);
```

Line 2497 — same test, broken buffer case:
```rust
let body =
    leaf_text_body(&leaf, 0.0, 0.0, natural, 640.0, 600.0, &mut broken, &BTreeMap::new());
```

Line 2622 — `stack_fixture` leaf rect width:
```rust
rects.insert(focus.clone(), Rect { x: 30.0, y: 0.0, w: 640.0, h: 200.0 });
```

- [ ] **Step 14: Run all tests**

Run: `cargo test -p outrider-layout && cargo test -p outrider`

Expected: all tests pass.

- [ ] **Step 15: Commit**

```bash
git add crates/outrider/src/world.rs crates/outrider-layout/src/pack.rs crates/outrider/src/treemap.rs
git commit -m "feat: bump PAGE_W from 480 to 640 for wider leaf pages"
```

---

### Task 2: Focused node expansion and line wrapping

**Files:**
- Modify: `crates/outrider/src/treemap.rs:29-103` — add helpers, refactor `wrap_doc`
- Modify: `crates/outrider/src/treemap.rs:329-427` — `container_body` and `leaf_text_body` signatures + wrapping
- Modify: `crates/outrider/src/treemap.rs:719-943` — `paint_items` expansion logic
- Modify: `crates/outrider/src/treemap.rs:2388-2700` — test updates + new tests

**Interfaces:**
- Consumes: `world::PAGE_W` (640.0 from Task 1), `content::FONT_PX`, `content::is_leaf_item`, `BufferManager::get`, `runs_from_spans`
- Produces: focused leaf expansion behavior (no new public API)

- [ ] **Step 1: Write tests for new helper functions**

Add to the `#[cfg(test)] mod tests` block in `crates/outrider/src/treemap.rs`. Import the new functions in the test module's `use super::` block (add `char_budget, wrap_to_budget, wrap_code_line, focused_width`).

```rust
#[test]
fn char_budget_exact_boundary() {
    // 12 + 10*0.62*12 = 86.4 → budget of 10
    let w = 12.0 + 10.0 * 0.62 * 12.0;
    assert_eq!(char_budget(w as f32, 12.0), 10);
    assert_eq!(char_budget(10.0, 12.0), 0); // too narrow
}

#[test]
fn wrap_to_budget_short_text_one_row() {
    assert_eq!(wrap_to_budget("hello", 10), vec!["hello"]);
}

#[test]
fn wrap_to_budget_word_wraps() {
    assert_eq!(
        wrap_to_budget("alpha beta gamma", 10),
        vec!["alpha beta", "gamma"]
    );
}

#[test]
fn wrap_to_budget_hard_splits_long_word() {
    assert_eq!(
        wrap_to_budget("abcdefghijklmno", 10),
        vec!["abcdefghij", "klmno"]
    );
}

#[test]
fn wrap_code_line_short_no_wrap() {
    let lines = wrap_code_line("hello", &[], 100.0, 12.0);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].0, "hello");
    assert_eq!(lines[0].1.iter().map(|r| r.0).sum::<usize>(), 5);
}

#[test]
fn wrap_code_line_wraps_preserving_runs() {
    use outrider_index::buffer::{HighlightKind, HighlightSpan};
    // "fn frobnicate()" = 16 chars, budget of 10
    let w = 12.0 + 10.0 * 0.62 * 12.0;
    let spans = vec![
        HighlightSpan { range: 0..2, kind: HighlightKind::Keyword },
    ];
    let lines = wrap_code_line("fn frobnicate()", &spans, w as f32, 12.0);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].0, "fn frobnic");
    assert_eq!(lines[1].0, "ate()");
    // runs cover each segment exactly
    assert_eq!(lines[0].1.iter().map(|r| r.0).sum::<usize>(), lines[0].0.len());
    assert_eq!(lines[1].1.iter().map(|r| r.0).sum::<usize>(), lines[1].0.len());
}

#[test]
fn wrap_code_line_returns_empty_when_no_room() {
    assert!(wrap_code_line("x", &[], 10.0, 12.0).is_empty());
}

#[test]
fn focused_width_clamps_to_range() {
    use crate::world::PAGE_W;
    use crate::content::FONT_PX;
    // 10 chars → needed = 10*12*0.62 + 12 = 86.4 → below PAGE_W → clamp to PAGE_W
    assert!((focused_width(10) - PAGE_W).abs() < 1e-9);
    // 200 chars → needed = 200*12*0.62 + 12 = 1500 → above 2*PAGE_W → clamp to 2*PAGE_W
    assert!((focused_width(200) - 2.0 * PAGE_W).abs() < 1e-9);
    // 120 chars → needed = 120*12*0.62 + 12 = 904.8 → between PAGE_W and 2*PAGE_W
    assert!((focused_width(120) - 904.8).abs() < 1e-9);
}
```

- [ ] **Step 2: Run the new tests to confirm they fail**

Run: `cargo test -p outrider -- char_budget wrap_to_budget wrap_code_line focused_width`

Expected: compilation errors (functions don't exist yet).

- [ ] **Step 3: Add `char_budget` helper**

In `crates/outrider/src/treemap.rs`, add after the `truncate_to_width` function (after line 46):

```rust
/// Character budget for a column `w_px` wide at `font_px` monospace.
fn char_budget(w_px: f32, font_px: f32) -> usize {
    let b = ((w_px - 12.0) / (font_px * 0.62) + 1e-6).floor() as isize;
    if b < 2 { 0 } else { b as usize }
}
```

- [ ] **Step 4: Add `wrap_to_budget` and refactor `wrap_doc`**

Add after `char_budget`:

```rust
/// Word-wrap a single line to `budget` characters. Words longer than
/// the budget are hard-split. Returns one or more display lines.
fn wrap_to_budget(text: &str, budget: usize) -> Vec<String> {
    if budget == 0 {
        return vec![];
    }
    if text.chars().count() <= budget {
        return vec![text.to_string()];
    }
    let mut rows = Vec::new();
    let mut line = String::new();
    let mut line_len = 0usize;
    for word in text.split(' ') {
        let mut word = word;
        let mut wlen = word.chars().count();
        while wlen > budget {
            if line_len > 0 {
                rows.push(std::mem::take(&mut line));
                line_len = 0;
            }
            let cut = word
                .char_indices()
                .nth(budget)
                .map_or(word.len(), |(i, _)| i);
            rows.push(word[..cut].to_string());
            word = &word[cut..];
            wlen = word.chars().count();
        }
        if wlen == 0 {
            continue;
        }
        let need = if line_len == 0 { wlen } else { line_len + 1 + wlen };
        if need > budget {
            rows.push(std::mem::take(&mut line));
            line.push_str(word);
            line_len = wlen;
        } else {
            if line_len > 0 {
                line.push(' ');
            }
            line.push_str(word);
            line_len = need;
        }
    }
    if line_len > 0 {
        rows.push(line);
    }
    rows
}
```

Then refactor `wrap_doc` to use it. Replace the function body:

```rust
fn wrap_doc(text: &str, w_px: f64, font_px: f64) -> Vec<String> {
    let budget = ((w_px - 12.0) / (font_px * 0.62) + 1e-6).floor() as isize;
    if budget < 2 {
        return Vec::new();
    }
    let budget = budget as usize;
    let mut rows = Vec::new();
    for para in text.split("\n\n") {
        let joined = para.split_whitespace().collect::<Vec<_>>().join(" ");
        if joined.is_empty() {
            continue;
        }
        rows.extend(wrap_to_budget(&joined, budget));
    }
    rows
}
```

- [ ] **Step 5: Add `wrap_code_line`**

Add after `wrap_to_budget`:

```rust
/// Wrap a code line into multiple display lines at the character budget,
/// splitting syntax-highlight runs at wrap boundaries. Returns
/// `Vec<(text, runs)>` — one entry per display line.
fn wrap_code_line(
    text: &str,
    spans: &[HighlightSpan],
    w: f32,
    font_px: f32,
) -> Vec<(String, Vec<(usize, u32)>)> {
    let budget = char_budget(w, font_px);
    if budget == 0 {
        return vec![];
    }
    if text.chars().count() <= budget {
        return vec![(text.to_string(), runs_from_spans(text.len(), spans))];
    }
    let full_runs = runs_from_spans(text.len(), spans);
    let mut result = Vec::new();
    let mut text_off = 0usize;
    let mut run_idx = 0usize;
    let mut run_off = 0usize;
    while text_off < text.len() {
        let rest = &text[text_off..];
        let take_chars = budget.min(rest.chars().count());
        let take_bytes = rest
            .char_indices()
            .nth(take_chars)
            .map_or(rest.len(), |(i, _)| i);
        let seg = rest[..take_bytes].to_string();
        let mut seg_runs = Vec::new();
        let mut left = take_bytes;
        while left > 0 && run_idx < full_runs.len() {
            let (rlen, color) = full_runs[run_idx];
            let avail = rlen - run_off;
            if avail <= left {
                seg_runs.push((avail, color));
                left -= avail;
                run_idx += 1;
                run_off = 0;
            } else {
                seg_runs.push((left, color));
                run_off += left;
                left = 0;
            }
        }
        result.push((seg, seg_runs));
        text_off += take_bytes;
    }
    result
}
```

- [ ] **Step 6: Add `focused_width`**

Add after `wrap_code_line`:

```rust
/// World-space width for a focused node given the longest line's char count.
/// Clamped to `[PAGE_W, 2·PAGE_W]` so the expansion is bounded.
fn focused_width(max_chars: usize) -> f64 {
    let needed = max_chars as f64 * FONT_PX * 0.62 + 2.0 * BODY_PAD;
    needed.clamp(world::PAGE_W, 2.0 * world::PAGE_W)
}
```

- [ ] **Step 7: Run helper tests**

Run: `cargo test -p outrider -- char_budget wrap_to_budget wrap_code_line focused_width`

Expected: all new tests pass. Also run existing `wrap_doc` tests to verify the refactor:

Run: `cargo test -p outrider -- wrap_doc`

Expected: all existing wrap_doc tests still pass.

- [ ] **Step 8: Add `max_line_chars` helper**

Add after `focused_width`:

```rust
/// Longest line (in characters) across a node's body text and source code.
/// Used to compute the content-driven expansion width.
fn max_line_chars(
    node: &SymbolNode,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> usize {
    let mut max = 0usize;
    for line in content::body_lines(node, Rung::Full) {
        let text = match line {
            BodyLine::Plain(t) | BodyLine::Dim(t) => t,
        };
        max = max.max(text.chars().count());
    }
    if content::is_leaf_item(node) {
        let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
        let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
        if let Some(m) = buffers.get(&rel, syms) {
            if let Some(start) = m.symbol_start_line(&node.id) {
                let count =
                    (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
                for j in 0..count {
                    if let Some((text, _)) = m.buffer.line(start + j) {
                        max = max.max(text.chars().count());
                    }
                }
            }
        }
    }
    max
}
```

- [ ] **Step 9: Modify `container_body` — add `focused` parameter with wrapping**

Change the signature to add `focused: bool`:

```rust
fn container_body(
    node: &SymbolNode,
    rung: Rung,
    px: &world::PxRect,
    label_w: f64,
    vh: f64,
    pin_y: f64,
    max_h: f64,
    focused: bool,
) -> Vec<BodyText> {
```

Replace the body-line loop (the `for (k, line)` section) with a row-counted version that wraps when focused:

```rust
    let font = FONT_PX as f32;
    let mut out = Vec::new();
    let mut row = 0usize;
    for line in content::body_lines(node, rung) {
        let y = pin_y + HEADER + row as f64 * LINE_STEP;
        if y + LINE_STEP > pin_y + max_h || y + LINE_STEP > px.y + px.h || y > vh {
            break;
        }
        let (text, color) = match line {
            BodyLine::Plain(t) => (t, theme::TEXT_PRIMARY),
            BodyLine::Dim(t) => (t, theme::TEXT_SECONDARY),
        };
        if focused {
            let budget = char_budget(label_w as f32, font);
            if budget > 0 {
                for seg in wrap_to_budget(&text, budget) {
                    let wy = pin_y + HEADER + row as f64 * LINE_STEP;
                    if wy + LINE_STEP > pin_y + max_h || wy + LINE_STEP > px.y + px.h || wy > vh {
                        break;
                    }
                    let len = seg.len();
                    out.push(BodyText {
                        x: (px.x + BODY_PAD) as f32,
                        y: wy as f32,
                        text: seg,
                        runs: vec![(len, color)],
                    });
                    row += 1;
                }
            }
        } else {
            if let Some(shown) = truncate_to_width(&text, label_w as f32, font) {
                let len = shown.len();
                out.push(BodyText {
                    x: (px.x + BODY_PAD) as f32,
                    y: y as f32,
                    text: shown,
                    runs: vec![(len, color)],
                });
            }
            row += 1;
        }
    }
    out
```

- [ ] **Step 10: Modify `leaf_text_body` — add `focused` parameter with wrapping**

Change the signature to add `focused: bool` and return `(Vec<BodyText>, usize)` for extra row count:

```rust
fn leaf_text_body(
    node: &SymbolNode,
    left: f64,
    top: f64,
    full_h: f64,
    label_w: f64,
    vh: f64,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
    focused: bool,
) -> (Vec<BodyText>, usize) {
```

Replace the function body with a version that tracks display rows and wraps when focused:

```rust
    let scale = full_h / content::natural_px(node);
    let font = (FONT_PX * scale) as f32;
    let step = LINE_STEP * scale;
    let x = (left + BODY_PAD * scale) as f32;
    let content_y0 = HEADER.max(HEADER * scale);
    let mut out = Vec::new();
    let mut display_row = 0usize;
    let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
    let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
    if let Some(m) = buffers.get(&rel, syms) {
        if let Some(start) = m.symbol_start_line(&node.id) {
            let count = (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
            for j in 0..count {
                let y = top + content_y0 + display_row as f64 * step;
                if y > vh {
                    break;
                }
                if let Some((text, spans)) = m.buffer.line(start + j) {
                    if focused {
                        let wrapped = wrap_code_line(&text, spans, label_w as f32, font);
                        for (seg, runs) in wrapped {
                            let wy = top + content_y0 + display_row as f64 * step;
                            if wy > vh {
                                break;
                            }
                            if wy + step >= 0.0 {
                                out.push(BodyText { x, y: wy as f32, text: seg, runs });
                            }
                            display_row += 1;
                        }
                    } else {
                        if y + step >= 0.0 {
                            if let Some((shown, runs)) = code_line(&text, spans, label_w as f32, font) {
                                out.push(BodyText { x, y: y as f32, text: shown, runs });
                            }
                        }
                        display_row += 1;
                    }
                } else {
                    display_row += 1;
                }
            }
            let extra = display_row.saturating_sub(count);
            return (out, extra);
        }
    }
    let lines = content::body_lines(node, Rung::Full);
    let source_count = lines.len();
    for line in lines {
        let y = top + content_y0 + display_row as f64 * step;
        if y > vh {
            break;
        }
        let (text, color) = match line {
            BodyLine::Plain(t) => (t, theme::TEXT_PRIMARY),
            BodyLine::Dim(t) => (t, theme::TEXT_SECONDARY),
        };
        if focused {
            let budget = char_budget(label_w as f32, font);
            if budget > 0 {
                for seg in wrap_to_budget(&text, budget) {
                    let wy = top + content_y0 + display_row as f64 * step;
                    if wy > vh {
                        break;
                    }
                    if wy + step >= 0.0 {
                        let len = seg.len();
                        out.push(BodyText { x, y: wy as f32, text: seg, runs: vec![(len, color)] });
                    }
                    display_row += 1;
                }
            }
        } else {
            if y + step >= 0.0 {
                if let Some(shown) = truncate_to_width(&text, label_w as f32, font) {
                    let len = shown.len();
                    out.push(BodyText { x, y: y as f32, text: shown, runs: vec![(len, color)] });
                }
            }
            display_row += 1;
        }
    }
    let extra = display_row.saturating_sub(source_count);
    (out, extra)
```

- [ ] **Step 11: Update all callers of `container_body` and `leaf_text_body`**

In `paint_items()` (`crates/outrider/src/treemap.rs`), update the `container_body` call (around line 784):

```rust
body = container_body(
    item.node, rung, &item.px, item.label_w, vh, pin_y, ch_px,
    is_focused,
);
```

Note: `is_focused` must be computed BEFORE this point. Move the `is_focused` computation (currently at ~line 852) to before the `match item.draw` block:

```rust
let is_focused = item.node.id == focus_id;
let is_leaf = matches!(item.draw, Draw::Leaf(_));
```

Update the `leaf_text_body` call (around line 822). Replace the existing call with one that computes the expanded width and passes `focused`:

```rust
if use_text {
    let effective_label_w = if is_focused {
        let max = max_line_chars(
            item.node, &mut self.buffers, &self.file_symbols,
        );
        focused_width(max) * camera.zoom
    } else {
        item.label_w
    };
    tex_opacity = 0.0;
    let (text_body, extra) = leaf_text_body(
        item.node,
        item.left,
        item.top,
        item.full_h,
        effective_label_w,
        vh,
        &mut self.buffers,
        &self.file_symbols,
        is_focused,
    );
    body = text_body;
    if is_focused && extra > 0 {
        let step = LINE_STEP * (item.full_h / content::natural_px(item.node));
        focused_extra_h = extra as f64 * step;
    }
}
```

Declare `focused_extra_h` near the top of the loop body (after `let mut tex`):

```rust
let mut focused_extra_h = 0.0f64;
```

And a variable to track the effective expanded width for the PaintItem:

```rust
let mut expanded_w = 0.0f32;
```

Set `expanded_w` in the leaf text path (inside the `if is_focused` block above):

```rust
expanded_w = effective_label_w as f32;
```

- [ ] **Step 12: Apply expansion to PaintItem and reorder paint**

In the PaintItem construction (around line 873), override `w` and `h` for focused leaves:

```rust
out.push(PaintItem {
    x: item.px.x as f32,
    y: item.px.y as f32,
    w: if is_focused && is_leaf && expanded_w > 0.0 {
        expanded_w
    } else {
        item.px.w as f32
    },
    h: if is_focused && is_leaf {
        (item.full_h + focused_extra_h) as f32
    } else {
        item.px.h as f32
    },
    fill,
    border: theme::border_for(fill),
    // ... rest unchanged ...
```

Remove the duplicate `is_focused` computation that was previously at ~line 852 (it's now computed earlier).

After the loop, move the focused leaf's PaintItem to the end for paint-on-top. Add before the `let doc_panel = ...` line:

```rust
if let Some(idx) = out.iter().position(|p| p.focused) {
    let is_leaf_paint = out[idx].tex.is_some()
        || out[idx].body_font_px != FONT_PX as f32
        || out[idx].body.iter().any(|_| true);
    if is_leaf_paint || expanded_w > 0.0 {
        let item = out.remove(idx);
        out.push(item);
    }
}
```

Wait — `expanded_w` is local to the loop body. We need a different way to detect focused leaves. Simpler approach: just always move the focused item to the end when it has an expanded width. Track it with a separate variable:

Add before the loop:
```rust
let mut focused_paint_idx: Option<usize> = None;
```

Inside the loop, after pushing the PaintItem:
```rust
if is_focused && is_leaf && expanded_w > 0.0 {
    focused_paint_idx = Some(out.len() - 1);
}
```

After the loop:
```rust
if let Some(idx) = focused_paint_idx {
    let item = out.remove(idx);
    out.push(item);
}
```

- [ ] **Step 13: Update existing tests for new signatures**

In the test module of `crates/outrider/src/treemap.rs`:

Update test imports (line 2394-2396):
```rust
use super::{
    char_budget, code_line, container_body, focused_width, leaf_tex_rect,
    leaf_text_body, runs_from_spans, truncate_to_width, wrap_code_line,
    wrap_doc, wrap_to_budget, HEADER, LINE_STEP,
};
```

Update `container_body_positions_detail_lines` (line 2452) — add `false`:
```rust
let body = container_body(&f, Rung::Detail, &px, 400.0, 600.0, px.y, 300.0, false);
```

Update `leaf_text_body_paints_code_without_duplicate_signature` (line 2468-2469) — add `false`, destructure tuple:
```rust
let (body, _extra) =
    leaf_text_body(&leaf, 0.0, 0.0, natural, 640.0, 600.0, &mut mgr, &file_symbols, false);
```

Update `leaf_text_body_scales_uniformly_past_one` (line 2489-2490) — add `false`, destructure:
```rust
let (body, _extra) = leaf_text_body(
    &leaf, 0.0, 0.0, 2.0 * natural, 1280.0, 100_000.0, &mut mgr, &file_symbols, false,
);
```

Same test, broken buffer path (line 2497):
```rust
let (body, _extra) =
    leaf_text_body(&leaf, 0.0, 0.0, natural, 640.0, 600.0, &mut broken, &BTreeMap::new(), false);
```

- [ ] **Step 14: Run all tests**

Run: `cargo test -p outrider`

Expected: all tests pass (existing + new helper tests).

- [ ] **Step 15: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: expand focused nodes and wrap text instead of truncating"
```
