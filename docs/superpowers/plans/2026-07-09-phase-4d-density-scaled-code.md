# Phase 4d: Density + Scaled, Clipped Code Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Siblings stack densely (3% gap), a code-bearing leaf keeps showing code — scaled down to a 7px font floor, then clipped — whenever its box can hold about three legible rows.

**Architecture:** Three GPUI-free changes (layout gap constant, `content.rs` scale math, `world.rs` leaf rung threshold) plus one wiring task in `treemap.rs` that plumbs the unclipped box height into a per-box code scale and a `body_font_px` on `PaintItem`. Framing (`frame_leaf`, `natural_px`) and all x-math are untouched.

**Tech Stack:** Rust workspace (`outrider-layout`, `outrider` bin), GPUI pinned rev, ropey/tree-sitter already wired.

**Spec:** `docs/superpowers/specs/2026-07-09-phase-4d-density-scaled-code-design.md`

## Global Constraints

- Every cargo command needs the prefix `export PATH="$HOME/.cargo/bin:$PATH" && `.
- `cargo clippy --workspace -- -D warnings` must stay clean after every task.
- `cargo test --workspace` must stay green after every task (83 tests before this plan).
- GPUI-free modules (`world.rs`, `content.rs`, `camera.rs`, `focus.rs`, everything in `outrider-layout`) must not import `gpui`. Only `treemap.rs` touches GPUI.
- Exact constants (spec §§2–4): gap = `(len * 3).div_ceil(100)`; `MIN_CODE_FONT_PX: f64 = 7.0`; `MIN_CODE_SCALE = MIN_CODE_FONT_PX / FONT_PX`; `LEAF_CODE_MIN_PX = HEADER + 3.0 * LINE_STEP * MIN_CODE_SCALE + BOTTOM_PAD` (= 54.1).
- Scaling applies ONLY to the Full-leaf body (signature row + code rows). The name/header row, Detail/Card summaries, and container inventories stay at 12px. `HEADER` stays fixed.
- Out of scope (spec §7): scaling the name row or `CODE_MIN_W`, depth/kind-driven gaps, any framing change (`frame_leaf`, fractions, tween), `natural_px` stays defined at scale 1.0.

---

### Task 1: Sibling gap 15% → 3% (`outrider-layout`)

**Files:**
- Modify: `crates/outrider-layout/src/measure.rs:21-26` (constant + doc) and its `gap_is_fifteen_percent_rounded_up` test

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `gap_cells(len) = (len * 3).div_ceil(100)` — used identically by measure and arrange passes; no signature change.

**Fixture note (verified before planning):** the worked-example fixtures do NOT change. All worked-example child lens (1, 3, 4) have gaps that hit the 1-cell round-up floor at both 15% and 3%, so `worked_example_measures` (measure.rs), `worked_example_layout_exact` / `absolute_start(g) == 44` (arrange.rs), and every `world.rs` band-dependent test keep their current expectations. Do not modify them; Step 4 proves they still pass.

- [ ] **Step 1: Update the failing test first**

In `crates/outrider-layout/src/measure.rs`, replace the test `gap_is_fifteen_percent_rounded_up` with:

```rust
    #[test]
    fn gap_is_three_percent_rounded_up() {
        assert_eq!(gap_cells(1), 1); // round-up floor keeps a minimum gap
        assert_eq!(gap_cells(4), 1);
        assert_eq!(gap_cells(7), 1);
        assert_eq!(gap_cells(20), 1);
        assert_eq!(gap_cells(34), 2); // ceil(1.02)
        assert_eq!(gap_cells(100), 3);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider-layout gap_is_three_percent -- --nocapture`
Expected: FAIL — `gap_cells(7)` returns 2 under the 15% formula (and 20 → 3, 34 → 6, 100 → 15).

- [ ] **Step 3: Change the constant**

In `crates/outrider-layout/src/measure.rs`, replace lines 21–26:

```rust
/// Per-child slack: ceil(0.15 · len), in integer math. Per-child (not pooled
/// per parent) so a child's position never depends on its successors — see
/// plan "Interpretation Decisions" #1.
pub(crate) fn gap_cells(len: u64) -> u64 {
    (len * 15).div_ceil(100)
}
```

with:

```rust
/// Per-child slack: ceil(0.03 · len), in integer math (3% — spec 4d §2;
/// was 15% before the density pass). Per-child (not pooled per parent) so
/// a child's position never depends on its successors. The round-up keeps
/// a minimum 1-cell gap for tiny nodes.
pub(crate) fn gap_cells(len: u64) -> u64 {
    (len * 3).div_ceil(100)
}
```

- [ ] **Step 4: Run the full workspace suite**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: all tests PASS (worked-example fixtures unchanged per the fixture note), clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider-layout/src/measure.rs
git commit -m "feat(layout): shrink sibling gap from 15% to 3% for dense stacking"
```

---

### Task 2: Scale constants + `code_scale` (`content.rs`)

**Files:**
- Modify: `crates/outrider/src/content.rs` (constants after `BOTTOM_PAD` at line 11, `code_scale` after `natural_px` at line 25, tests at the end of the `tests` module)

**Interfaces:**
- Consumes: existing `FONT_PX`, `LINE_STEP`, `HEADER`, `BOTTOM_PAD`, `natural_px(node)`.
- Produces (used by Tasks 3 and 4):
  - `pub const MIN_CODE_FONT_PX: f64 = 7.0;`
  - `pub const MIN_CODE_SCALE: f64 = MIN_CODE_FONT_PX / FONT_PX;`
  - `pub const LEAF_CODE_MIN_PX: f64` (= 54.1)
  - `pub fn code_scale(node: &SymbolNode, px_h: f64) -> f64`

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `crates/outrider/src/content.rs`:

```rust
    #[test]
    fn leaf_code_min_px_value() {
        // HEADER 20.8 + 3·15.6·(7/12) + BOTTOM_PAD 6 = 54.1
        assert!((LEAF_CODE_MIN_PX - 54.1).abs() < 1e-9);
        assert!((MIN_CODE_SCALE - 7.0 / 12.0).abs() < 1e-12);
    }

    #[test]
    fn code_scale_clamps_between_floor_and_one() {
        // measure 3 → natural 89.2 (see natural_px_arithmetic). Compare
        // against natural_px itself, not a decimal literal: n/n is exactly
        // 1.0, while a re-typed 89.2 can land one ulp under and miss the top
        // of the clamp.
        let three = node(SymbolKind::Fn, "a.rs::f", 3, 0.0, 0, Some("fn f()"), None, vec![]);
        let n = natural_px(&three);
        // box fits the whole method (and anything taller): exact 1.0
        assert_eq!(code_scale(&three, n), 1.0);
        assert_eq!(code_scale(&three, 500.0), 1.0);
        // mid value: 80% of natural → 0.8
        assert!((code_scale(&three, 0.8 * n) - 0.8).abs() < 1e-9);
        // tiny box: exact 7/12 floor, after which the window clips
        assert_eq!(code_scale(&three, 10.0), 7.0 / 12.0);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider content::tests`
Expected: FAIL to compile — `LEAF_CODE_MIN_PX`, `MIN_CODE_SCALE`, `code_scale` not found.

- [ ] **Step 3: Add the constants and function**

In `crates/outrider/src/content.rs`, insert after the `BOTTOM_PAD` const (line 11):

```rust
/// Floor for scaled code text (spec 4d §4).
pub const MIN_CODE_FONT_PX: f64 = 7.0;
pub const MIN_CODE_SCALE: f64 = MIN_CODE_FONT_PX / FONT_PX;

/// Shortest leaf box that still shows code: header + three code rows at
/// the floor font + bottom pad (≈ 54.1px). Below this a leaf drops to the
/// container ladder (spec 4d §3).
pub const LEAF_CODE_MIN_PX: f64 = HEADER + 3.0 * LINE_STEP * MIN_CODE_SCALE + BOTTOM_PAD;
```

and insert after `natural_px` (line 25):

```rust
/// Per-box text scale for a Full leaf: 1.0 when the box fits the whole
/// method, shrinking with the box down to the floor, after which the
/// window clips. `px_h` must be the UNCLIPPED box height — the clipped
/// height would wrongly shrink zoomed-in giants (spec 4d §4).
pub fn code_scale(node: &SymbolNode, px_h: f64) -> f64 {
    (px_h / natural_px(node)).clamp(MIN_CODE_SCALE, 1.0)
}
```

- [ ] **Step 4: Run tests and clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: all PASS, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/content.rs
git commit -m "feat(app): code_scale and LEAF_CODE_MIN_PX constants for scaled leaf code"
```

---

### Task 3: Leaf rung threshold uses `LEAF_CODE_MIN_PX` (`world.rs`)

**Files:**
- Modify: `crates/outrider/src/world.rs:112-138` (`rung_for` doc + leaf arm) and its `rung_for_thresholds_and_downgrade` test (lines ~456-464)

**Interfaces:**
- Consumes: `content::LEAF_CODE_MIN_PX` from Task 2 (`world.rs` already has `use crate::content;`).
- Produces: `rung_for(px_h, px_w, natural_px)` — signature unchanged; leaf arm now `px_h >= n.min(content::LEAF_CODE_MIN_PX)`. `FULL_PX` remains in the container ladder only.

- [ ] **Step 1: Update the test to the new expectations**

In `rung_for_thresholds_and_downgrade`, the "Leaf legibility" block currently reads:

```rust
        // Leaf legibility (spec 4c §6): Full as soon as the box fits the
        // content, even below FULL_PX
        assert_eq!(rung_for(100.0, 400.0, Some(90.0)), Some(Rung::Full));
        assert_eq!(rung_for(100.0, 400.0, None), Some(Rung::Card)); // container ladder
        assert_eq!(rung_for(100.0, 250.0, Some(90.0)), Some(Rung::Detail)); // width gate holds
        assert_eq!(rung_for(100.0, 59.0, Some(90.0)), Some(Rung::Dot)); // narrow gate holds
        assert_eq!(rung_for(80.0, 400.0, Some(90.0)), Some(Rung::Card)); // below content → ladder
        assert_eq!(rung_for(699.0, 400.0, Some(3000.0)), Some(Rung::Detail)); // long fn: FULL_PX cap
        assert_eq!(rung_for(700.0, 400.0, Some(3000.0)), Some(Rung::Full));
```

Replace it with:

```rust
        // Leaf code persistence (spec 4d §3): Full whenever the box holds
        // ~three floor-font rows (LEAF_CODE_MIN_PX = 54.1) — or its whole
        // natural height, if that is smaller.
        assert_eq!(rung_for(100.0, 400.0, Some(90.0)), Some(Rung::Full));
        assert_eq!(rung_for(100.0, 400.0, None), Some(Rung::Card)); // container ladder
        assert_eq!(rung_for(100.0, 250.0, Some(90.0)), Some(Rung::Detail)); // width gate holds
        assert_eq!(rung_for(100.0, 59.0, Some(90.0)), Some(Rung::Dot)); // narrow gate holds
        assert_eq!(rung_for(80.0, 400.0, Some(90.0)), Some(Rung::Full)); // ≥ 54.1 → code, clipped
        assert_eq!(rung_for(55.0, 400.0, Some(3000.0)), Some(Rung::Full)); // long fn, no FULL_PX cap
        assert_eq!(rung_for(54.0, 400.0, Some(3000.0)), Some(Rung::Label)); // just below 54.1
        assert_eq!(rung_for(43.0, 400.0, Some(42.4)), Some(Rung::Full)); // tiny leaf: natural wins
        assert_eq!(rung_for(42.0, 400.0, Some(42.4)), Some(Rung::Label)); // below its natural height
        assert_eq!(rung_for(700.0, 400.0, Some(3000.0)), Some(Rung::Full));
```

- [ ] **Step 2: Run it to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider rung_for_thresholds`
Expected: FAIL — under the 4c rule (`n.min(FULL_PX)`), `(80, 400, Some(90))` is Card and `(55, 400, Some(3000))` is Label; the first failing assertion is the `(80, …) → Full` line.

- [ ] **Step 3: Change the leaf arm and doc comment**

In `crates/outrider/src/world.rs`, the `rung_for` doc comment currently ends:

```rust
/// code. Heights below MERGE_PX merge into the parent. For leaf items,
/// pass `natural_px`: the box is Full as soon as it fits its content
/// (capped at FULL_PX for long methods) — code appears when close enough,
/// no explicit dive required (spec 4c §6).
```

Replace those lines with:

```rust
/// code. Heights below MERGE_PX merge into the parent. For leaf items,
/// pass `natural_px`: the box is Full as soon as it holds about three
/// floor-font code rows (LEAF_CODE_MIN_PX) or its whole content, whichever
/// is smaller — code persists, scaled then clipped (spec 4d §3).
```

and change the leaf match arm from:

```rust
    let by_height = match natural_px {
        Some(n) if px_h >= n.min(FULL_PX) => Rung::Full,
        _ => by_height,
    };
```

to:

```rust
    let by_height = match natural_px {
        Some(n) if px_h >= n.min(content::LEAF_CODE_MIN_PX) => Rung::Full,
        _ => by_height,
    };
```

- [ ] **Step 4: Run tests and clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: all PASS, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/world.rs
git commit -m "feat(app): leaf code persists to ~3 floor-font rows (LEAF_CODE_MIN_PX)"
```

---

### Task 4: Scaled Full-leaf body — unclipped height, scaled window, `body_font_px` (`world.rs`, `treemap.rs`)

**Files:**
- Modify: `crates/outrider/src/world.rs` (`DrawItem` struct ~line 164, `walk` push ~line 245)
- Modify: `crates/outrider/src/treemap.rs` (`PaintItem` struct line 61, `build_body` lines 116–181, `paint_items` lines 238–284, paint closure body loop lines 470–522, two existing `build_body` test call sites, one new test)

**Interfaces:**
- Consumes: `content::code_scale(node, px_h)` and `content::FONT_PX` from Task 2.
- Produces:
  - `DrawItem` gains `pub full_h: f64` — the UNCLIPPED pixel height (`px.h` is clipped to the viewport).
  - `build_body(node, rung, px, label_w, top, scale: f64, vh, buffers, file_symbols)` — new `scale` param after `top`; callers pass `1.0` for unscaled bodies.
  - `PaintItem` gains `body_font_px: f32` — `(FONT_PX * scale) as f32` (12.0 for everything except Full leaves).

**Why unclipped:** `build_body` receives a viewport-CLIPPED `px.h` (`walk` clamps y0/y1 with 2px slack), so deriving scale from `px.h` would wrongly shrink the font on a zoomed-in method taller than the viewport. `walk` already chooses the rung from the unclipped height; this task carries that height out on `DrawItem` and computes scale in `paint_items`.

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `crates/outrider/src/treemap.rs` (reuses the module's `node` helper and the tempdir pattern of `build_body_full_leaf_appends_windowed_code`):

```rust
    #[test]
    fn build_body_full_leaf_scales_step_and_clips_at_box_edge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\nfn five() {}\n",
        )
        .unwrap();
        // 4-line symbol starting at line 1 (byte 12), natural 104.8px
        let leaf = node(SymbolKind::Fn, "a.rs::two", Some(12..59), 4, Some("fn two()"), None);
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 12)]);
        let px = PxRect { x: 0.0, y: 0.0, w: 400.0, h: 60.0 };
        let scale = 0.8;
        let step = LINE_STEP * scale; // 12.48
        let body =
            build_body(&leaf, Rung::Full, &px, 400.0, 0.0, scale, 600.0, &mut mgr, &file_symbols);
        // signature + two scaled code rows; the third (y 58.24) would cross
        // max_y = 60 − 12.48 = 47.52 and is clipped at the box edge.
        assert_eq!(body.len(), 3);
        assert_eq!(body[0].text, "fn two()");
        assert_eq!(body[1].text, "fn two() {}");
        assert_eq!(body[2].text, "fn three() {}");
        assert!((f64::from(body[1].y) - (HEADER + step)).abs() < 1e-3);
        assert!((f64::from(body[2].y) - (HEADER + 2.0 * step)).abs() < 1e-3);
        // same box at scale 1.0 fits only one code row — scaling shows more
        let body =
            build_body(&leaf, Rung::Full, &px, 400.0, 0.0, 1.0, 600.0, &mut mgr, &file_symbols);
        assert_eq!(body.len(), 2);
    }
```

Also update the two existing `build_body` call sites in tests to the new signature (scale `1.0` inserted after the `top` argument, keeping their expectations unchanged):

- In `build_body_positions_detail_lines`:
  `build_body(&f, Rung::Detail, &px, 400.0, 0.0, 1.0, 600.0, &mut mgr, &BTreeMap::new())`
- In `build_body_full_leaf_appends_windowed_code` (both calls):
  `build_body(&leaf, Rung::Full, &px, 400.0, 0.0, 1.0, 600.0, &mut mgr, &file_symbols)` and
  `build_body(&leaf, Rung::Full, &px, 400.0, 0.0, 1.0, 600.0, &mut broken, &BTreeMap::new())`

- [ ] **Step 2: Run it to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider build_body`
Expected: FAIL to compile — `build_body` has no `scale` parameter yet.

- [ ] **Step 3: Implement — `world.rs` plumbing**

In `crates/outrider/src/world.rs`, add a field to `DrawItem` (after `top`):

```rust
    /// UNclipped screen-y of the box top (`px.y` is clipped to the viewport).
    pub top: f64,
    /// UNclipped pixel height (`px.h` is clipped) — drives the code scale.
    pub full_h: f64,
```

and in `walk`, extend the push:

```rust
    out.push(DrawItem {
        node,
        px: PxRect { x: px_x, y: y0, w: px_w, h: y1 - y0 },
        label_w: px_w,
        level: depth,
        rung,
        top: px_y,
        full_h: px_h,
    });
```

- [ ] **Step 4: Implement — `build_body` scaled window**

In `crates/outrider/src/treemap.rs`, replace `build_body` (lines 116–181) with:

```rust
/// Body content for one box: content-table lines anchored to the CLIPPED
/// top (they pin like the name row), then — for Full leaf items — the
/// symbol's highlighted code laid out from the UNCLIPPED top and
/// line-window culled to the viewport (spec §4.4). Rows that would sit
/// under the pinned name/signature block or off-screen are skipped.
/// `scale` shrinks the row step and font of the whole body (spec 4d §4);
/// callers pass 1.0 for everything except Full leaves.
#[allow(clippy::too_many_arguments)]
fn build_body(
    node: &SymbolNode,
    rung: Rung,
    px: &world::PxRect,
    label_w: f64,
    top: f64,
    scale: f64,
    vh: f64,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> Vec<BodyText> {
    if rung == Rung::Dot || rung == Rung::Label {
        return Vec::new();
    }
    let step = LINE_STEP * scale;
    let font = (FONT_PX * scale) as f32;
    let mut out = Vec::new();
    let lines = content::body_lines(node, rung);
    let rows = lines.len();
    for (k, line) in lines.into_iter().enumerate() {
        let y = px.y + HEADER + k as f64 * step;
        if y + step > px.y + px.h || y > vh {
            break;
        }
        let (text, color) = match line {
            BodyLine::Plain(t) => (t, theme::TEXT_PRIMARY),
            BodyLine::Dim(t) => (t, theme::TEXT_SECONDARY),
        };
        if let Some(shown) = truncate_to_width(&text, label_w as f32, font) {
            let len = shown.len();
            out.push(BodyText { y: y as f32, text: shown, runs: vec![(len, color)] });
        }
    }
    if rung == Rung::Full && content::is_leaf_item(node) {
        let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
        let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
        if let Some(m) = buffers.get(&rel, syms) {
            if let Some(start) = m.symbol_start_line(&node.id) {
                let count = (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
                let code_y0 = top + HEADER + rows as f64 * step;
                let min_y = px.y + HEADER + rows as f64 * step - 0.5;
                let max_y = (px.y + px.h).min(vh) - step;
                for j in 0..count {
                    let y = code_y0 + j as f64 * step;
                    if y < min_y {
                        continue;
                    }
                    if y > max_y {
                        break;
                    }
                    if let Some((text, spans)) = m.buffer.line(start + j) {
                        if let Some((shown, runs)) = code_line(&text, spans, label_w as f32, font) {
                            out.push(BodyText { y: y as f32, text: shown, runs });
                        }
                    }
                }
            }
        }
    }
    out
}
```

(This is the existing function with `step`/`font` replacing `LINE_STEP`/`FONT_PX as f32` throughout — the content loop, `code_y0`, `min_y`, `max_y`, row y, and both truncation calls — plus the new `scale` param and the doc-comment addition. Nothing else changes.)

- [ ] **Step 5: Implement — `PaintItem.body_font_px` and `paint_items`**

In `crates/outrider/src/treemap.rs`, add to the `PaintItem` struct (after `label_w: f32,`):

```rust
    /// Font size for body rows: FONT_PX·scale for Full leaves, else 12.0.
    /// The name row always paints at 12px.
    body_font_px: f32,
```

In `paint_items`, replace the loop body (currently lines 255–282) with:

```rust
        for item in items {
            let is_code = item.rung == Rung::Full && content::is_leaf_item(item.node);
            let scale =
                if is_code { content::code_scale(item.node, item.full_h) } else { 1.0 };
            let fill = if is_code { theme::CODE_BG } else { theme::depth_fill(item.level) };
            let body = build_body(
                item.node,
                item.rung,
                &item.px,
                item.label_w,
                item.top,
                scale,
                vh,
                &mut self.buffers,
                &self.file_symbols,
            );
            out.push(PaintItem {
                x: item.px.x as f32,
                y: item.px.y as f32,
                w: item.px.w as f32,
                h: item.px.h as f32,
                label_w: item.label_w as f32,
                body_font_px: (FONT_PX * scale) as f32,
                fill,
                border: theme::border_for(fill),
                stripe: (item.node.churn > 0.0).then(|| theme::churn_heat(item.node.churn)),
                focused: item.node.id == focus_id,
                rung: item.rung,
                name: item.node.name.clone(),
                body,
            });
        }
```

- [ ] **Step 6: Implement — paint closure shapes body rows at `body_font_px`**

In the paint closure, the body-row loop (currently lines 502–522) shapes and paints at the fixed `font_px`/`line_height`. Change only the body loop — the name row keeps `font_px = 12.0_f32`:

```rust
                            let body_line_height = px(item.body_font_px * 1.3);
                            for bt in &item.body {
                                if bt.text.is_empty() {
                                    continue;
                                }
                                let runs: Vec<TextRun> =
                                    bt.runs.iter().map(|&(len, color)| run(len, color)).collect();
                                let line = window.text_system().shape_line(
                                    bt.text.clone().into(),
                                    px(item.body_font_px),
                                    &runs,
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(item.x + 6.0), origin.y + px(bt.y)),
                                    body_line_height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
                            }
```

(`let body_line_height = …` goes immediately before the `for bt in &item.body` loop; the loop's `px(font_px)` and `line_height` become `px(item.body_font_px)` and `body_line_height`.)

- [ ] **Step 7: Run tests and clippy**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: all PASS (including the new scaled-window test and the two updated call sites), clippy clean.

- [ ] **Step 8: Commit**

```bash
git add crates/outrider/src/world.rs crates/outrider/src/treemap.rs
git commit -m "feat(app): scale Full-leaf code with box height down to the 7px floor"
```

---

## Manual exit gate (after all tasks)

Run the app on the Outrider repo (`export PATH="$HOME/.cargo/bin:$PATH" && cargo run --release`): siblings stack densely, leaf code stays visible (scaled, then clipped) while zooming out, 7px floor still legible, "code everywhere while moving" — the Bet #1 re-run. This is the user's pass, not an automated step.
