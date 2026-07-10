# Window Chrome & Leaf-to-Leaf Navigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the GPUI window a client-side titlebar (title, minimize/maximize/close, drag-move, edge-resize) and make arrow keys from a focused leaf jump to the nearest leaf page at any depth.

**Architecture:** Two independent changes. (1) `focus.rs::spatial_step` gains a mode split: when the focused node is a leaf page (`content::is_leaf_item`), candidates become all leaf pages at any depth; otherwise the existing same-depth filter stands. (2) A new GPUI-only `chrome.rs` renders a 32px titlebar plus invisible perimeter resize strips; `main.rs` requests client decorations; `treemap.rs` wraps the map in a column below the titlebar and offsets its camera/mouse math by the titlebar height.

**Tech Stack:** Rust, GPUI (pinned zed rev `029bf2f`, `gpui_platform` wayland under WSLg), tree-sitter (unchanged here).

## Global Constraints

- Every cargo command must be prefixed with `export PATH="$HOME/.cargo/bin:$PATH" && `.
- Gate at each task boundary: `cargo test --workspace` green AND `cargo clippy --workspace --all-targets -- -D warnings` clean.
- GPUI types may only appear in `main.rs`, `treemap.rs`, and the new `chrome.rs`. `focus.rs` and `content.rs` stay GPUI-free.
- Exact values: `app_id = "outrider"`; window title = `outrider — <repo dir name>`; titlebar height `TITLEBAR_H = 32.0`; window min size `480×320`; resize rim thickness `6.0`, corner `12.0`.
- Leaf-page predicate is `content::is_leaf_item` (source bytes + no children + not Folder) — do not introduce a second definition.
- Spatial-step scoring, tie-break, and no-wrap rules are unchanged: strict half-plane `primary > 0`, `score = primary + 2·|ortho|` on rect centers, lesser `SymbolId` on exact ties.

---

### Task 1: Leaf-to-leaf spatial arrow step

**Files:**
- Modify: `crates/outrider/src/focus.rs` (`spatial_step`, its doc comment, and the `#[cfg(test)]` module)

**Interfaces:**
- Consumes: `content::is_leaf_item(&SymbolNode) -> bool` (existing, in `crate::content`); `TreeIndex::node(&SymbolId) -> Option<&SymbolNode>`, `TreeIndex::depth(&SymbolId) -> Option<usize>` (existing).
- Produces: `spatial_step(current, dir, pack, index) -> Option<SymbolId>` — unchanged signature; new behavior when `current` is a leaf page.

- [ ] **Step 1: Write the failing test**

Add this test and its helper to the bottom of the `#[cfg(test)] mod tests` block in `crates/outrider/src/focus.rs` (just before the closing `}` of the module, after `spatial_step_penalizes_orthogonal_offset`):

```rust
    /// A leaf page with source bytes (unlike `n`, which leaves byte_range None).
    fn leaf(
        kind: SymbolKind,
        qp: &str,
        name: &str,
        children: Vec<outrider_index::SymbolNode>,
    ) -> outrider_index::SymbolNode {
        outrider_index::SymbolNode { byte_range: Some(0..1), ..n(kind, qp, name, children) }
    }

    /// root { a.md (leaf, d1), dir (empty folder, d1), b.rs (container, d1) { f (leaf, d2) } }
    fn leaf_depth_tree() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                vec![
                    leaf(SymbolKind::File, "a.md", "a.md", vec![]),
                    n(SymbolKind::Folder, "dir", "dir", vec![]),
                    leaf(
                        SymbolKind::File,
                        "b.rs",
                        "b.rs",
                        vec![leaf(SymbolKind::Fn, "b.rs::f", "f", vec![])],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    #[test]
    fn spatial_step_leaf_mode_crosses_depth_and_skips_non_leaves() {
        let t = leaf_depth_tree();
        let idx = TreeIndex::new(&t);
        let a_md = id(SymbolKind::File, "a.md");
        let f = id(SymbolKind::Fn, "b.rs::f");
        // Column top→bottom: a.md (leaf d1), dir (empty folder d1),
        // b.rs (container d1), f (leaf d2). Only a.md and f are leaf pages.
        let lay = hand_layout(&[
            (a_md.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (id(SymbolKind::Folder, "dir"), Rect { x: 0.0, y: 15.0, w: 10.0, h: 10.0 }),
            (id(SymbolKind::File, "b.rs"), Rect { x: 0.0, y: 30.0, w: 10.0, h: 10.0 }),
            (f.clone(), Rect { x: 0.0, y: 45.0, w: 10.0, h: 10.0 }),
        ]);
        // Down from the shallow leaf skips the nearer folder+container and
        // lands on the deeper leaf (crosses depth 1 → 2).
        assert_eq!(spatial_step(&a_md, Dir::Down, &lay, &idx), Some(f.clone()));
        // Up from the deep leaf returns to the shallow leaf (depth 2 → 1).
        assert_eq!(spatial_step(&f, Dir::Up, &lay, &idx), Some(a_md.clone()));
        // No leaf below the bottom leaf → no wrap.
        assert_eq!(spatial_step(&f, Dir::Down, &lay, &idx), None);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider --lib focus::tests::spatial_step_leaf_mode -- --nocapture`
Expected: FAIL — the first assertion returns `Some(dir)` (current same-depth code picks the nearest depth-1 node) instead of `Some(f)`; the second returns `None`.

- [ ] **Step 3: Rewrite `spatial_step` with the mode split**

Replace the doc comment and body of `spatial_step` in `crates/outrider/src/focus.rs` (currently lines ~113–151) with:

```rust
/// Spatial arrow step. When `current` is a leaf page
/// (`content::is_leaf_item`), candidates are all other leaf pages at any
/// tree depth; otherwise candidates are the nodes at `current`'s own
/// depth. Among candidates whose center lies strictly in `dir`, pick the
/// one scored lowest by primary distance + 2·|orthogonal offset|;
/// SymbolId breaks exact ties. No wrap: no candidate → None.
pub fn spatial_step(
    current: &SymbolId,
    dir: Dir,
    pack: &PackLayout,
    index: &TreeIndex,
) -> Option<SymbolId> {
    let cur = pack.rects.get(current)?;
    let (cx, cy) = (cur.x + cur.w / 2.0, cur.y + cur.h / 2.0);
    let depth = index.depth(current)?;
    let leaf_mode = index.node(current).is_some_and(crate::content::is_leaf_item);
    let mut best: Option<(f64, &SymbolId)> = None;
    for (id, r) in &pack.rects {
        if id == current {
            continue;
        }
        let eligible = if leaf_mode {
            index.node(id).is_some_and(crate::content::is_leaf_item)
        } else {
            index.depth(id) == Some(depth)
        };
        if !eligible {
            continue;
        }
        let (nx, ny) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (primary, ortho) = match dir {
            Dir::Right => (nx - cx, (ny - cy).abs()),
            Dir::Left => (cx - nx, (ny - cy).abs()),
            Dir::Down => (ny - cy, (nx - cx).abs()),
            Dir::Up => (cy - ny, (nx - cx).abs()),
        };
        if primary <= 0.0 {
            continue;
        }
        let score = primary + 2.0 * ortho;
        let better = match best {
            None => true,
            Some((s, b)) => score < s || (score == s && id < b),
        };
        if better {
            best = Some((score, id));
        }
    }
    best.map(|(_, id)| id.clone())
}
```

Note: `depth` is now used only in the container branch, but the `let depth = index.depth(current)?;` line is kept — it preserves the "current must be a known node" guard and is a live read in the `else` arm, so no unused-variable warning.

- [ ] **Step 4: Run the new test and the whole focus module**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p outrider --lib focus::`
Expected: PASS — `spatial_step_leaf_mode_crosses_depth_and_skips_non_leaves` passes, and the existing `spatial_step_crosses_parent_boundaries_at_same_depth` and `spatial_step_penalizes_orthogonal_offset` still pass (their focused nodes have `byte_range: None`, so they exercise container mode unchanged).

- [ ] **Step 5: Full gate**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all tests pass, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/focus.rs
git commit -m "feat(app): arrow keys jump leaf-to-leaf at any depth"
```

---

### Task 2: Client-side window chrome

**Files:**
- Create: `crates/outrider/src/chrome.rs`
- Modify: `crates/outrider/src/main.rs` (module list + `WindowOptions`)
- Modify: `crates/outrider/src/treemap.rs` (imports, two inherent helpers, `render` body)

**Interfaces:**
- Produces: `chrome::TITLEBAR_H: f64`; `chrome::titlebar(title: impl Into<SharedString>, window: &Window) -> impl IntoElement`; `chrome::resize_rim(window: &Window) -> Option<impl IntoElement>`.
- Consumes (from GPUI, verified in rev `029bf2f`): `Window::{is_maximized, zoom_window, minimize_window, start_window_move, start_window_resize}`, `App::quit`, `WindowDecorations::Client`, `ResizeEdge::{Top,Bottom,Left,Right,TopLeft,TopRight,BottomLeft,BottomRight}`, `CursorStyle::{ResizeUpDown,ResizeLeftRight,ResizeUpLeftDownRight,ResizeUpRightDownLeft}`, `MouseDownEvent::click_count`.

This task has no headless test (all behavior is window-system-bound); it is verified by build + clippy + a manual smoke run. Do all three file edits before building so the final state is warning-free (a created-but-unused `chrome.rs` would trip `-D warnings`).

- [ ] **Step 1: Create `crates/outrider/src/chrome.rs`**

```rust
use gpui::{
    div, prelude::*, px, rgb, App, CursorStyle, MouseButton, ResizeEdge, SharedString, Window,
};

use crate::theme;

/// Height of the client-side titlebar, in pixels.
pub const TITLEBAR_H: f64 = 32.0;

/// Thickness of the invisible window-resize rim along each edge.
const RIM: f64 = 6.0;
/// Square size of each corner resize hit-zone.
const CORNER: f64 = 12.0;
/// Width of each window-control button.
const BTN_W: f64 = 46.0;

const TITLE_FG: u32 = 0x9a9aa4;
const BTN_HOVER: u32 = 0x2a2a30;
const CLOSE_HOVER: u32 = 0xc42b1c;

/// The client-side titlebar: title text on the left, minimize / maximize
/// (restore) / close buttons on the right. Dragging the body moves the
/// window; double-clicking the body toggles maximize.
pub fn titlebar(title: impl Into<SharedString>, window: &Window) -> impl IntoElement {
    let maximize_glyph = if window.is_maximized() { "❐" } else { "□" };
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(px(TITLEBAR_H as f32))
        .bg(rgb(theme::BG))
        .border_b_1()
        .border_color(rgb(theme::border_for(theme::BG)))
        .child(
            // Draggable body: title + flex spacer.
            div()
                .flex()
                .flex_grow(1.)
                .items_center()
                .h_full()
                .px_3()
                .text_color(rgb(TITLE_FG))
                .text_size(px(13.))
                .child(title.into())
                .on_mouse_down(MouseButton::Left, |e, window, _cx| {
                    if e.click_count >= 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                }),
        )
        .child(control_btn("–", BTN_HOVER, |window, _cx| window.minimize_window()))
        .child(control_btn(maximize_glyph, BTN_HOVER, |window, _cx| window.zoom_window()))
        .child(control_btn("✕", CLOSE_HOVER, |_window, cx| cx.quit()))
}

/// One window-control button: centered glyph, hover fill, press action.
/// The glyph is chosen by the caller (e.g. maximize vs. restore), so no
/// per-button state is needed here.
fn control_btn(
    glyph: &'static str,
    hover_bg: u32,
    on_press: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(BTN_W as f32))
        .h_full()
        .cursor_pointer()
        .text_color(rgb(theme::TEXT_SECONDARY))
        .text_size(px(13.))
        .hover(move |s| s.bg(rgb(hover_bg)))
        .child(glyph)
        .on_mouse_down(MouseButton::Left, move |_e, window, cx| on_press(window, cx))
}

/// Invisible window-resize strips over the window perimeter — eight
/// absolutely-positioned edges/corners, each starting a compositor-driven
/// resize on left-press. Returns `None` while maximized (no rim then).
pub fn resize_rim(window: &Window) -> Option<impl IntoElement> {
    if window.is_maximized() {
        return None;
    }
    Some(
        div()
            // Full-window, non-interactive container: only the strips have
            // listeners, so mouse events elsewhere fall through to the map.
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(strip(Edge::Top))
            .child(strip(Edge::Bottom))
            .child(strip(Edge::Left))
            .child(strip(Edge::Right))
            // Corners last so diagonal grabs win over the edges beneath them.
            .child(strip(Edge::TopLeft))
            .child(strip(Edge::TopRight))
            .child(strip(Edge::BottomLeft))
            .child(strip(Edge::BottomRight)),
    )
}

#[derive(Clone, Copy)]
enum Edge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Edge {
    fn resize(self) -> ResizeEdge {
        match self {
            Edge::Top => ResizeEdge::Top,
            Edge::Bottom => ResizeEdge::Bottom,
            Edge::Left => ResizeEdge::Left,
            Edge::Right => ResizeEdge::Right,
            Edge::TopLeft => ResizeEdge::TopLeft,
            Edge::TopRight => ResizeEdge::TopRight,
            Edge::BottomLeft => ResizeEdge::BottomLeft,
            Edge::BottomRight => ResizeEdge::BottomRight,
        }
    }

    fn cursor(self) -> CursorStyle {
        match self {
            Edge::Top | Edge::Bottom => CursorStyle::ResizeUpDown,
            Edge::Left | Edge::Right => CursorStyle::ResizeLeftRight,
            Edge::TopLeft | Edge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            Edge::TopRight | Edge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        }
    }
}

fn strip(edge: Edge) -> gpui::Div {
    let base = div()
        .absolute()
        .cursor(edge.cursor())
        .on_mouse_down(MouseButton::Left, move |_e, window, _cx| {
            window.start_window_resize(edge.resize());
        });
    let rim = px(RIM as f32);
    let corner = px(CORNER as f32);
    match edge {
        Edge::Top => base.top_0().left_0().right_0().h(rim),
        Edge::Bottom => base.bottom_0().left_0().right_0().h(rim),
        Edge::Left => base.top_0().bottom_0().left_0().w(rim),
        Edge::Right => base.top_0().bottom_0().right_0().w(rim),
        Edge::TopLeft => base.top_0().left_0().w(corner).h(corner),
        Edge::TopRight => base.top_0().right_0().w(corner).h(corner),
        Edge::BottomLeft => base.bottom_0().left_0().w(corner).h(corner),
        Edge::BottomRight => base.bottom_0().right_0().w(corner).h(corner),
    }
}
```

Note on the maximize button: `maximize_glyph` is computed from `window.is_maximized()` before building the row (`□` windowed / `❐` maximized) and passed straight into `control_btn`, so the button always renders the state-correct glyph without any per-button state.

- [ ] **Step 2: Register the module and request client decorations in `main.rs`**

In `crates/outrider/src/main.rs`, add `mod chrome;` to the module list (keep alphabetical — between `camera` and `content`):

```rust
mod buffers;
mod camera;
mod chrome;
mod content;
mod focus;
mod theme;
mod treemap;
mod world;
```

Update the `use gpui::` line to add `WindowDecorations`:

```rust
use gpui::{
    px, size, App, AppContext as _, Bounds, WindowBounds, WindowDecorations, WindowOptions,
};
```

Replace the `WindowOptions { .. }` passed to `cx.open_window` with:

```rust
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("outrider".into()),
                window_min_size: Some(size(px(480.), px(320.))),
                ..Default::default()
            },
```

- [ ] **Step 3: Wire chrome into `treemap.rs`**

In `crates/outrider/src/treemap.rs`, add `use crate::chrome;` next to the other `use crate::...` lines (e.g. after `use crate::camera::...`).

Add these two helpers to the inherent `impl TreemapView` block (e.g. next to `root_rect`):

```rust
    /// Window title shown in the client titlebar and taskbar.
    fn window_title(&self) -> String {
        let name = self
            .tree
            .repo_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "outrider".into());
        format!("outrider — {name}")
    }

    /// The map's drawable size = the window minus the titlebar. Camera math
    /// and mouse hit-testing both use these; the map canvas is offset down
    /// by `chrome::TITLEBAR_H` in window coordinates.
    fn map_viewport(window: &Window) -> (f64, f64) {
        let vp = window.viewport_size();
        (f64::from(vp.width), f64::from(vp.height) - chrome::TITLEBAR_H)
    }
```

In `render`, replace the opening viewport read:

```rust
        let vp = window.viewport_size();
        let (vw, vh) = (f64::from(vp.width), f64::from(vp.height));
        let items = self.paint_items(vw, vh);
```

with:

```rust
        let (vw, vh) = Self::map_viewport(window);
        let items = self.paint_items(vw, vh);
```

In the mouse-up listener, replace:

```rust
                    let Some(cam) = this.camera else { return };
                    let vp = w.viewport_size();
                    let items = world::visible_nodes(
                        &this.tree,
                        &this.layout,
                        &cam,
                        f64::from(vp.width),
                        f64::from(vp.height),
                    );
                    // view fills the window, so window coords == canvas coords
                    let (mx, my) = (f64::from(e.position.x), f64::from(e.position.y));
```

with:

```rust
                    let Some(cam) = this.camera else { return };
                    let (vw, vh) = Self::map_viewport(w);
                    let items = world::visible_nodes(&this.tree, &this.layout, &cam, vw, vh);
                    // the map canvas sits below the titlebar; shift window
                    // coords up by its height to get canvas coords
                    let (mx, my) =
                        (f64::from(e.position.x), f64::from(e.position.y) - chrome::TITLEBAR_H);
```

In the scroll-wheel listener, replace:

```rust
                let vp = w.viewport_size();
                let (vw, vh) = (f64::from(vp.width), f64::from(vp.height));
                if let Some(cam) = this.camera.as_mut() {
                    // scroll up (positive dy) zooms in; flip the sign here if
                    // manual testing shows it inverted on this platform
                    let factor = (dy * 0.002).exp();
                    cam.zoom_about(
                        f64::from(e.position.x),
                        f64::from(e.position.y),
                        vw,
                        vh,
                        factor,
                        min_zoom,
                        max_zoom,
                    );
                }
```

with:

```rust
                let (vw, vh) = Self::map_viewport(w);
                if let Some(cam) = this.camera.as_mut() {
                    // scroll up (positive dy) zooms in; flip the sign here if
                    // manual testing shows it inverted on this platform
                    let factor = (dy * 0.002).exp();
                    cam.zoom_about(
                        f64::from(e.position.x),
                        f64::from(e.position.y) - chrome::TITLEBAR_H,
                        vw,
                        vh,
                        factor,
                        min_zoom,
                        max_zoom,
                    );
                }
```

In the key-down listener, replace:

```rust
                let vp = w.viewport_size();
                let (vw, vh) = (f64::from(vp.width), f64::from(vp.height));
```

with:

```rust
                let (vw, vh) = Self::map_viewport(w);
```

Finally, restructure the `render` return value. The existing return is a single `div().size_full()...child(canvas(...).size_full())`. Bind that whole map element to a local `map` and change its top-level sizing from `.size_full()` to `.flex_grow(1.).w_full().relative().overflow_hidden()`, then wrap it in a titlebar column with the resize rim on top. Concretely, the tail of `render` becomes:

```rust
        let title = self.window_title();
        let map = div()
            .flex_grow(1.)
            .w_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(theme::BG))
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, _w, _cx| {
                    this.drag_last = Some(e.position);
                    this.press_origin = Some(e.position);
                }),
            )
            // ... KEEP every existing listener (.on_mouse_up, .on_mouse_move,
            //     .on_scroll_wheel, .on_key_down) exactly as edited above ...
            .child(
                canvas(
                    |_bounds, _window, _cx: &mut App| {},
                    move |bounds, _prepaint, window, _cx: &mut App| {
                        // ... KEEP the existing paint closure verbatim ...
                    },
                )
                .size_full(),
            );

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BG))
            .child(chrome::titlebar(title, window))
            .child(map)
            .children(chrome::resize_rim(window))
```

Only two things change on the map element itself: its outer `.size_full()` becomes `.flex_grow(1.).w_full().relative().overflow_hidden()` (the leading `.bg(rgb(theme::BG))` and all listeners/canvas stay). The canvas child keeps its own `.size_full()`. `.children(chrome::resize_rim(window))` spreads the `Option` (0 or 1 element) as the last, top-most sibling.

- [ ] **Step 4: Build and lint**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p outrider && cargo clippy --workspace --all-targets -- -D warnings`
Expected: builds clean, clippy reports no warnings.

- [ ] **Step 5: Full test gate**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace`
Expected: all tests pass (chrome has no unit tests; this confirms nothing else regressed).

- [ ] **Step 6: Manual smoke run (WSLg)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo run -p outrider -- .`
Verify by hand and report what you observe:
- Titlebar shows `outrider — outrider-ide`; taskbar entry reads `outrider`.
- Drag the bar → window moves; double-click the bar → toggles maximize.
- `–` minimizes, `□`/`❐` maximizes/restores (glyph flips), `✕` quits.
- All four edges and four corners resize with the correct cursor; no rim while maximized; window won't shrink below 480×320.
- Map still lines up: clicking a box focuses that exact box (no ~32px vertical offset), scroll zooms about the pointer, arrow/Enter/Esc/End/Home behave. Leaf boxes stay dark at every zoom.

If any GPUI method name mismatches the pinned rev (e.g. a style helper), fix the call site — the semantics above are the contract; report the substitution in your notes.

- [ ] **Step 7: Commit**

```bash
git add crates/outrider/src/chrome.rs crates/outrider/src/main.rs crates/outrider/src/treemap.rs
git commit -m "feat(app): client-side titlebar with move/resize/min/max/close"
```

---

## Self-Review

**Spec coverage** (against `docs/superpowers/specs/2026-07-10-window-chrome-leaf-nav-design.md`):
- §2 leaf-to-leaf step → Task 1 (mode split, scoring preserved, container mode via existing tests).
- §3 window options → Task 2 Step 2.
- §4 chrome module (titlebar + resize rim) → Task 2 Step 1.
- §5 treemap integration & coordinate offset → Task 2 Step 3 (`map_viewport` helper, three listener offsets, column layout).
- §6 testing → Task 1 headless tests; Task 2 Step 6 manual gate.
- §7 out of scope → nothing added (no shadows, no geometry persistence, no window menu).

**Placeholder scan:** no TBD/TODO; every code step shows full code. No placeholder glyphs — the maximize button receives its state-correct glyph directly.

**Type consistency:** `TITLEBAR_H: f64` used consistently in `map_viewport` and offsets; `chrome::titlebar(title, window)`/`chrome::resize_rim(window)` signatures match their call sites; `content::is_leaf_item` is the single leaf predicate in both `spatial_step` and `theme::box_fill` (unchanged). `Self::map_viewport` is an associated fn (no `self`), called as `Self::map_viewport(window/w)` in render and all three listeners.
