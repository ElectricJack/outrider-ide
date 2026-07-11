# Texture-Tier Doc Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** At the blurred-texture zoom tier, overlay each documented leaf box with its crisp 12px name (already present) plus its `///` doc description, word-wrapped, on a translucent dark panel.

**Architecture:** All changes live in `crates/outrider/src/treemap.rs`. Two new pure helpers (`wrap_doc`, `doc_overlay`) build screen-space doc rows; `PaintItem` carries them (`doc_rows`, `doc_panel_h`); the canvas closure paints panel + rows in Pass 1 right after the texture quad, reusing `tex_opacity` for the fade.

**Tech Stack:** Rust, GPUI (canvas painting), existing outrider crates.

**Spec:** `docs/superpowers/specs/2026-07-11-texture-doc-overlay-design.md`

## Global Constraints

- `cargo test --workspace` must pass. NOTE (environment quirk): piped cargo output is invisible — verify with `cargo test --workspace >/dev/null 2>&1 && echo WORKSPACE_TESTS_OK`, or redirect to a file and read it.
- `cargo clippy --workspace --all-targets` must produce no NEW warnings (one pre-existing `type_complexity` warning in `crates/outrider-index/src/parse.rs` is allowed).
- Char-budget formula must match `truncate_to_width` exactly: `((w_px - 12.0) / (font_px * 0.62) + 1e-6).floor()`; budget < 2 means no text.
- Doc rows render at screen-space `FONT_PX` (12.0), color `theme::TEXT_PRIMARY`, starting at `px.y + HEADER`, stepping `LINE_STEP`; a row that would end below `px.y + px.h` is dropped (not clipped).
- Backdrop panel: `theme::CODE_BG` at alpha `0.85 * tex_opacity`, inner box width, from box top to last doc row bottom.
- Overlay (panel + rows) opacity is exactly `tex_opacity` (existing field) — no new fade math.
- Overlay only for leaves at texture tier (`font < content::TEXT_FADE_HI`) whose node has `doc: Some(_)`. Doc-less leaves and all containers are unchanged.
- Commit messages: `feat: ...` / `docs: ...` style with trailing `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>` line.
- Every new `fn`/`struct` gets a short `///` doc comment (repo convention; these comments feed the app's own UI).

---

### Task 1: `wrap_doc` word-wrap helper

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (add helper directly below `truncate_to_width`, which ends at ~line 44; add tests in the existing `mod tests` at the end of the file)

**Interfaces:**
- Consumes: nothing new.
- Produces: `fn wrap_doc(text: &str, w_px: f64, font_px: f64) -> Vec<String>` — used by Task 2's `doc_overlay`.

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` block at the end of `crates/outrider/src/treemap.rs` (it already has `use super::{truncate_to_width, HEADER, LINE_STEP, ...}` — extend the import list with `wrap_doc`):

```rust
/// w_px giving exactly `budget` chars: budget = (w - 12) / (0.62 * 12).
fn wrap_w(budget: usize) -> f64 {
    12.0 + budget as f64 * 0.62 * 12.0
}

#[test]
fn wrap_doc_fits_short_text_on_one_row() {
    assert_eq!(wrap_doc("hello world", wrap_w(11), 12.0), vec!["hello world"]);
}

#[test]
fn wrap_doc_greedy_wraps_at_word_boundaries() {
    assert_eq!(
        wrap_doc("alpha beta gamma", wrap_w(10), 12.0),
        vec!["alpha beta", "gamma"]
    );
}

#[test]
fn wrap_doc_hard_splits_over_budget_words() {
    assert_eq!(
        wrap_doc("abcdefghijklmnopqrstuvwxy", wrap_w(10), 12.0),
        vec!["abcdefghij", "klmnopqrst", "uvwxy"]
    );
}

#[test]
fn wrap_doc_joins_lines_within_a_paragraph() {
    assert_eq!(
        wrap_doc("first line\nsecond line", wrap_w(40), 12.0),
        vec!["first line second line"]
    );
}

#[test]
fn wrap_doc_breaks_paragraphs_on_blank_lines() {
    assert_eq!(
        wrap_doc("para one\n\npara two", wrap_w(40), 12.0),
        vec!["para one", "para two"]
    );
}

#[test]
fn wrap_doc_returns_nothing_when_no_room() {
    assert!(wrap_doc("anything", 13.0, 12.0).is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider wrap_doc > /tmp/wrapdoc.txt 2>&1; grep -c "cannot find" /tmp/wrapdoc.txt > /tmp/wrapdoc-check.txt; cat /tmp/wrapdoc-check.txt`
Expected: compile error, `cannot find function wrap_doc` (read /tmp/wrapdoc.txt with the Read tool if grep output is invisible).

- [ ] **Step 3: Implement `wrap_doc`**

Insert directly after the closing brace of `truncate_to_width` (~line 44):

```rust
/// Reflow a `///` doc block for the texture-tier overlay: source lines are
/// joined into paragraphs (a blank line is a paragraph break), then each
/// paragraph is greedy word-wrapped to the same char budget as
/// `truncate_to_width`. Words longer than the budget are hard-split.
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
        let mut line = String::new();
        let mut line_len = 0usize;
        for word in joined.split(' ') {
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
    }
    rows
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider wrap_doc >/dev/null 2>&1 && echo WRAP_OK`
Expected: `WRAP_OK`

- [ ] **Step 5: Full gates and commit**

Run: `cargo test --workspace >/dev/null 2>&1 && echo WORKSPACE_TESTS_OK`
Run: `cargo clippy --workspace --all-targets 2>/tmp/clippy.txt; grep -c "^warning" /tmp/clippy.txt` (only the pre-existing type_complexity warning allowed; read /tmp/clippy.txt if unsure)

```bash
git add crates/outrider/src/treemap.rs
git commit -m "$(cat <<'EOF'
feat: add wrap_doc paragraph reflow helper for the doc overlay

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `doc_overlay` rows + `PaintItem` plumbing

**Files:**
- Modify: `crates/outrider/src/treemap.rs`:
  - `struct PaintItem` (~line 99): two new fields.
  - New helper `doc_overlay` after `wrap_doc`.
  - `paint_items` leaf branch (`Draw::Leaf(tier)` arm, ~lines 554–599) and the `PaintItem` literal (~line 601).
  - Tests in `mod tests`.

**Interfaces:**
- Consumes: `wrap_doc(text, w_px, font_px) -> Vec<String>` from Task 1; the test helper `fn wrap_w(budget: usize) -> f64` that Task 1 added to `mod tests` (`12.0 + budget as f64 * 0.62 * 12.0`); existing `struct BodyText { x: f32, y: f32, text: String, runs: Vec<(usize, u32)> }`; `world::PxRect { x, y, w, h }` (all `f64`, all pub); constants `HEADER`, `LINE_STEP`, `FONT_PX`, `BODY_PAD`; `theme::TEXT_PRIMARY`.
- Produces:
  - `fn doc_overlay(doc: &str, px: &world::PxRect) -> (Vec<BodyText>, f32)` — rows plus panel height.
  - `PaintItem.doc_rows: Vec<BodyText>` and `PaintItem.doc_panel_h: f32` — consumed by Task 3's paint pass.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` (extend the `use super::...` import with `doc_overlay, BODY_PAD` as needed; `crate::world::PxRect` is constructible from tests):

```rust
#[test]
fn doc_overlay_lays_rows_under_the_name_and_drops_overflow() {
    // Box fits exactly 2 rows: h = HEADER + 2*LINE_STEP.
    let px = crate::world::PxRect {
        x: 100.0,
        y: 50.0,
        w: wrap_w(10) + 2.0 * BODY_PAD,
        h: HEADER + 2.0 * LINE_STEP,
    };
    let (rows, panel_h) = doc_overlay("alpha beta gamma delta epsilon", &px);
    assert_eq!(rows.len(), 2); // 3+ wrapped rows, only 2 fit
    assert_eq!(rows[0].x, (100.0 + BODY_PAD) as f32);
    assert_eq!(rows[0].y, (50.0 + HEADER) as f32);
    assert_eq!(rows[1].y, (50.0 + HEADER + LINE_STEP) as f32);
    assert_eq!(panel_h, (HEADER + 2.0 * LINE_STEP) as f32);
    assert_eq!(rows[0].runs, vec![(rows[0].text.len(), crate::theme::TEXT_PRIMARY)]);
}

#[test]
fn doc_overlay_is_empty_when_no_row_fits() {
    let px = crate::world::PxRect { x: 0.0, y: 0.0, w: 200.0, h: HEADER };
    let (rows, panel_h) = doc_overlay("some description", &px);
    assert!(rows.is_empty());
    assert_eq!(panel_h, 0.0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p outrider doc_overlay > /tmp/docoverlay.txt 2>&1; echo done` then Read /tmp/docoverlay.txt.
Expected: compile error, `cannot find function doc_overlay`.

- [ ] **Step 3: Implement `doc_overlay` and plumb `PaintItem`**

Insert after `wrap_doc`:

```rust
/// Screen-space doc-overlay rows for a texture-tier leaf: the item's `///`
/// doc wrapped to the box's inner width, one crisp 12px row per line
/// starting under the pinned name row; rows that would leave the box are
/// dropped. Also returns the backdrop-panel height (box top → last row
/// bottom); (empty, 0.0) when nothing fits.
fn doc_overlay(doc: &str, px: &world::PxRect) -> (Vec<BodyText>, f32) {
    let mut rows = Vec::new();
    let mut y = px.y + HEADER;
    for text in wrap_doc(doc, px.w - 2.0 * BODY_PAD, FONT_PX) {
        if y + LINE_STEP > px.y + px.h {
            break;
        }
        let runs = vec![(text.len(), theme::TEXT_PRIMARY)];
        rows.push(BodyText { x: (px.x + BODY_PAD) as f32, y: y as f32, text, runs });
        y += LINE_STEP;
    }
    let panel_h = if rows.is_empty() { 0.0 } else { (y - px.y) as f32 };
    (rows, panel_h)
}
```

Add two fields to `struct PaintItem` (after `tex_opacity`):

```rust
    /// Crisp 12px doc-description rows overlaid on a texture-tier leaf.
    doc_rows: Vec<BodyText>,
    /// Height of the translucent backdrop panel behind name + doc rows
    /// (0.0 = no overlay).
    doc_panel_h: f32,
```

In `paint_items`, alongside `let mut tex: Option<TexQuad> = None;` add:

```rust
            let mut doc_rows = Vec::new();
            let mut doc_panel_h = 0.0f32;
```

In the `Draw::Leaf(tier)` arm, inside the existing `if font < content::TEXT_FADE_HI {` block, after the texture-quad `if` and before the `if font > content::TEXT_FADE_LO {` fade computation, add:

```rust
                        if let Some(doc) = &item.node.doc {
                            (doc_rows, doc_panel_h) = doc_overlay(doc, &item.px);
                        }
```

In the `PaintItem` literal at the bottom of the loop, after `tex_opacity,` add:

```rust
                doc_rows,
                doc_panel_h,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p outrider doc_overlay >/dev/null 2>&1 && echo OVERLAY_OK`
Expected: `OVERLAY_OK`

- [ ] **Step 5: Full gates and commit**

Run: `cargo test --workspace >/dev/null 2>&1 && echo WORKSPACE_TESTS_OK`
Run clippy as in Task 1 (only the pre-existing type_complexity warning allowed).

```bash
git add crates/outrider/src/treemap.rs
git commit -m "$(cat <<'EOF'
feat: build doc-overlay rows for documented texture-tier leaves

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Paint the backdrop panel and doc rows

**Files:**
- Modify: `crates/outrider/src/treemap.rs`, Pass 1 of the canvas closure (~lines 821–884). The texture-quad block (`if let Some(t) = &item.tex { ... }`) ends at ~line 883; the new code goes right after it, still inside the Pass 1 `for item in &items` loop.

**Interfaces:**
- Consumes: `PaintItem.doc_rows` / `doc_panel_h` / `tex_opacity` from Task 2; the closure-local `run` helper (`let run = |len, color| TextRun { ... }`, defined just above Pass 1); `theme::CODE_BG`; `FONT_PX`.
- Produces: rendering only; no new interfaces.

- [ ] **Step 1: Add the paint code**

Insert immediately after the closing brace of the `if let Some(t) = &item.tex { ... }` block (~line 883), inside the Pass 1 loop:

```rust
                            // Doc overlay: translucent panel + crisp 12px doc
                            // rows over the blurred texture. Fades out with
                            // tex_opacity as real code text fades in. Painted
                            // in Pass 1 so pinned headers and focus rings
                            // (later passes) stay on top; the name row paints
                            // over the panel in Pass 2a.
                            if !item.doc_rows.is_empty() {
                                let pc = rgb(theme::CODE_BG).opacity(0.85 * item.tex_opacity);
                                let pb = Bounds::new(
                                    point(
                                        origin.x + px(item.x + 1.0),
                                        origin.y + px(item.y + 1.0),
                                    ),
                                    size(
                                        px((item.w - 2.0).max(0.0)),
                                        px((item.doc_panel_h - 1.0).max(0.0)),
                                    ),
                                );
                                window.paint_quad(quad(
                                    pb,
                                    px(0.),
                                    pc,
                                    px(0.),
                                    pc,
                                    BorderStyle::default(),
                                ));
                                for bt in &item.doc_rows {
                                    let runs: Vec<TextRun> = bt
                                        .runs
                                        .iter()
                                        .map(|&(len, color)| {
                                            let mut r = run(len, color);
                                            r.color = r.color.opacity(item.tex_opacity);
                                            r
                                        })
                                        .collect();
                                    let line = window.text_system().shape_line(
                                        bt.text.clone().into(),
                                        px(FONT_PX as f32),
                                        &runs,
                                        None,
                                    );
                                    let _ = line.paint(
                                        point(origin.x + px(bt.x), origin.y + px(bt.y)),
                                        px(FONT_PX as f32 * 1.3),
                                        TextAlign::Left,
                                        None,
                                        window,
                                        _cx,
                                    );
                                }
                            }
```

Note: the `run` closure and `_cx` names must match what the surrounding closure already uses (Pass 2a at ~lines 887–934 uses both — copy those exact identifiers).

- [ ] **Step 2: Verify it compiles and all tests pass**

Run: `cargo test --workspace >/dev/null 2>&1 && echo WORKSPACE_TESTS_OK`
Expected: `WORKSPACE_TESTS_OK`

- [ ] **Step 3: Clippy gate**

Run: `cargo clippy --workspace --all-targets 2>/tmp/clippy3.txt; echo done` then Read /tmp/clippy3.txt.
Expected: only the pre-existing `type_complexity` warning at `crates/outrider-index/src/parse.rs`.

- [ ] **Step 4: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "$(cat <<'EOF'
feat: paint crisp doc-description overlay on texture-tier leaves

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Manual visual gate (human)

After all tasks: `cargo run -p outrider -- .` — zoom out to the blurred-texture tier over `crates/` and verify: crisp white name + wrapped doc on a dim panel for documented leaves; smooth fade across the Text↔Texture band; doc-less leaves unchanged; pinned headers and focus rings still render on top.
