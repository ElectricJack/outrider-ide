# Window Chrome & Leaf-to-Leaf Navigation — Design

- Date: 2026-07-10
- Parent: `2026-07-09-spatial-treemap-pivot-design.md`,
  `2026-07-10-text-pages-design.md` (built on the merged text-pages
  main). No layout, content, or camera-math changes.
- Motivation: (1) under WSLg the window is a bare rectangle — no title,
  no min/max/close buttons, no way to drag-move or edge-resize (the
  compositor ignores GPUI's server-decoration request). (2) Arrow keys
  step between nodes at the same tree depth; from a focused leaf this
  skips over visually adjacent leaves that happen to sit at a different
  depth — a jarring jump. Verified against the pinned GPUI rev
  (`029bf2f`): `start_window_move`, `start_window_resize(ResizeEdge)`,
  `zoom_window`, `minimize_window`, `is_maximized`,
  `WindowDecorations::Client`, and `MouseDownEvent::click_count` all
  exist.

## 1. Goals

- A client-side titlebar: title text, minimize / maximize-restore /
  close buttons, drag-to-move, double-click-to-maximize, plus invisible
  resize hit-zones on all window edges and corners.
- Arrow navigation from a focused **leaf page** jumps to the nearest
  leaf page in that direction at **any** depth; container focus keeps
  today's same-depth stepping.

## 2. Leaf-to-leaf arrow step (`focus.rs`)

`spatial_step` splits its candidate filter on the focused node's kind:

```rust
let leaf_mode = index.node(current).is_some_and(content::is_leaf_item);
for (id, r) in &pack.rects {
    if id == current {
        continue;
    }
    let candidate = if leaf_mode {
        index.node(id).is_some_and(content::is_leaf_item)
    } else {
        index.depth(id) == Some(depth)
    };
    if !candidate {
        continue;
    }
    // scoring unchanged
}
```

- **Leaf mode** (current node satisfies `content::is_leaf_item` — has
  source bytes, no children, not a Folder): candidates are all other
  leaf pages, at any depth. Containers and empty folders are never
  candidates.
- **Container mode**: same-depth filter, byte-for-byte today's
  behavior. `depth` is only needed in this branch; the existing
  `index.depth(current)?` early-return stays (harmless in leaf mode).
- Scoring, tie-break, and no-wrap semantics unchanged: strict half-plane
  (`primary > 0`), `score = primary + 2·|ortho|` on rect centers,
  lesser `SymbolId` on exact ties, `None` when nothing qualifies.
- Camera follow unchanged — the arrow handler already re-frames the
  landed node through `frame_focus`, so leaf jumps across files use the
  same tween as today.
- Judgment call: scoring stays center-to-center even though leaf pages
  vary in size; revisit only if it feels wrong in practice.
- `content.rs` is GPUI-free, so `focus.rs` gains no GPUI dependency and
  the new paths are fully unit-testable. The doc comment on
  `spatial_step` is updated to describe both modes.

## 3. Window options (`main.rs`)

```rust
WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(bounds)),
    titlebar: None,
    window_decorations: Some(WindowDecorations::Client),
    app_id: Some("outrider".into()),
    window_min_size: Some(size(px(480.), px(320.))),
    ..Default::default()
}
```

`titlebar: None` drops the (ignored) server titlebar request;
`WindowDecorations::Client` declares we draw our own chrome; `app_id`
labels the WSLg taskbar entry; the min size keeps resize from
collapsing the window below usability.

## 4. Chrome module (`crates/outrider/src/chrome.rs`)

New module holding all window furniture; GPUI's footprint grows from
{`main.rs`, `treemap.rs`} to include `chrome.rs`. No world/content
logic. Public surface:

```rust
pub const TITLEBAR_H: f64 = 32.0;
pub fn titlebar(title: &str, window: &Window) -> impl IntoElement;
pub fn resize_rim(window: &Window) -> Option<impl IntoElement>;
```

### Titlebar

A 32px flex row, `theme::BG` background with a 1px bottom border of
`theme::border_for(theme::BG)`:

- **Left:** title text `outrider — {root name}` (the SymbolTree root's
  name, i.e. the repo directory name), dim foreground, 13px, padded,
  non-interactive (`TITLE_FG = 0x9a9aa4`, matching the existing dim
  text ramp).
- **Right:** three 46×32 button divs — minimize `–`, maximize/restore
  (`□` when windowed, `❐` when `window.is_maximized()`), close `✕`.
  Hover fill `0x2a2a30`; close hover `0xc42b1c` with white glyph.
  Click actions: `window.minimize_window()`, `window.zoom_window()`,
  `cx.quit()`.
- **Bar body** (the flex-grow spacer and title area, not the buttons):
  left-mouse-down with `click_count >= 2` → `window.zoom_window()`,
  otherwise `window.start_window_move()`.
- The bar takes no keyboard focus; arrows/Enter/Esc/End/Home still go
  to the treemap's focus handle.

### Resize rim

Eight absolutely-positioned transparent strips over the window
perimeter, 6px thick (corners 12×12), rendered above all other
content. Each sets its cursor and on left-mouse-down calls
`window.start_window_resize(edge)`:

| strip | `ResizeEdge` | cursor |
|---|---|---|
| top, bottom | `Top`, `Bottom` | `ResizeUpDown` |
| left, right | `Left`, `Right` | `ResizeLeftRight` |
| top-left, bottom-right | `TopLeft`, `BottomRight` | `ResizeUpLeftDownRight` |
| top-right, bottom-left | `TopRight`, `BottomLeft` | `ResizeUpRightDownLeft` |

- Corners paint after (above) edges so diagonal grabs win.
- `resize_rim` returns `None` when `window.is_maximized()` — no rim on
  a maximized window (restore via `❐` or double-click first).
- The top strip overlaps the titlebar's top 6px and wins there:
  grabbing the very top edge resizes, standard CSD behavior.
- No shadows, rounded window corners, or tiling-inset handling — the
  window stays a plain rectangle.

## 5. Treemap integration & coordinate offset (`treemap.rs`)

`TreemapView::render`'s root becomes a relative column: titlebar on
top, the map (with all existing listeners) filling the rest, resize
rim overlaid last. Two assumptions in the current code break and are
fixed:

- **Viewport:** every `window.viewport_size()` use for camera math
  (render, mouse-up hit test, scroll zoom, key handler) computes
  `vh = f64::from(vp.height) - chrome::TITLEBAR_H` (width unchanged).
- **Mouse coords:** listeners receive window coordinates; the canvas
  origin is now `(0, TITLEBAR_H)`. Hit-testing and `zoom_about` use
  `my = f64::from(e.position.y) - chrome::TITLEBAR_H`. Pan deltas are
  differences, so they need no offset. The stale comment "window
  coords == canvas coords" is replaced.

A single private helper on `TreemapView` (e.g.
`fn map_viewport(window: &Window) -> (f64, f64)`) centralizes the
subtraction so the offset lives in one place.

The map div keeps `track_focus`; the view focuses itself on render as
today, so keyboard behavior is unaffected by the new siblings.

## 6. Testing

Navigation — headless in `focus.rs`:

- Leaf-focused step reaches a nearer leaf page at a **different**
  depth (fixture: leaf at depth 2, nearest same-direction leaf at
  depth 3; old code returned the same-depth node or `None`, new code
  returns the deeper leaf).
- Leaf-focused step never lands on a container or an empty folder even
  when one is spatially nearest.
- Container-focused step is unchanged (existing
  `spatial_step_crosses_parent_boundaries_at_same_depth` and
  `spatial_step_penalizes_orthogonal_offset` keep passing — their
  moving nodes are leaf items at equal depth, where both modes agree,
  or containers).
- Leaf mode with no leaf in the half-plane → `None`.

Chrome — window-system-bound, so manual exit gate under WSLg
(`cargo run -p outrider -- .`):

- Title reads `outrider — <repo dir>`; taskbar entry says `outrider`.
- Drag bar moves; double-click toggles maximize; `–`/`□`/`✕` minimize,
  maximize-restore, and quit.
- All 8 edges/corners resize with the right cursors; rim absent while
  maximized; window won't shrink below 480×320.
- Map interactions still line up: clicking a box focuses that box (no
  32px offset error), scroll zooms about the pointer, arrows/Enter/Esc
  behave.

Plus the standing gate: `cargo test --workspace` green,
`cargo clippy --workspace --all-targets -- -D warnings` clean.

## 7. Out of scope

Window shadows/rounded corners, tiling insets, `show_window_menu`,
keyboard shortcuts for window management, remembering window geometry
across runs, macOS/Windows-specific titlebar variants, changing
spatial-step scoring.
