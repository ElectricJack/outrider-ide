//! Client-side window chrome: titlebar with controls and invisible resize strips.
//! Provides `titlebar` and `resize_rim` for use by the root render function;
//! both are no-ops when the OS manages decorations or the window is maximized.

use gpui::{
    div, prelude::*, px, rgb, App, CursorStyle, MouseButton, ResizeEdge, SharedString, Window,
};

use crate::theme;

/// Height of the client-side titlebar, in pixels.
pub const TITLEBAR_H: f64 = 32.0;

/// Thickness of the invisible window-resize rim along each edge, in pixels.
const RIM: f64 = 6.0;
/// Square size of each corner resize hit-zone, in pixels.
const CORNER: f64 = 12.0;
/// Width of each window-control button, in pixels.
const BTN_W: f64 = 46.0;

const TITLE_FG: u32 = 0x9a9aa4;
const BTN_HOVER: u32 = 0x2a2a30;
const CLOSE_HOVER: u32 = 0xc42b1c;

/// The client-side titlebar: title text on the left, optional menu items in
/// the middle, minimize / maximize / close buttons on the right. Dragging
/// the body moves the window; double-clicking toggles maximize.
pub fn titlebar(
    title: impl Into<SharedString>,
    menu: impl IntoElement,
    status: impl Into<SharedString>,
    window: &Window,
) -> impl IntoElement {
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
                .child(menu)
                .on_mouse_down(MouseButton::Left, |e, window, _cx| {
                    if e.click_count >= 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .h_full()
                .px(px(8.0))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .text_size(px(11.))
                .child(status.into()),
        )
        .child(control_btn("–", BTN_HOVER, |window, _cx| window.minimize_window()))
        .child(control_btn(maximize_glyph, BTN_HOVER, |window, _cx| window.zoom_window()))
        .child(control_btn("✕", CLOSE_HOVER, |_window, cx| cx.quit()))
}

/// Hover background for titlebar menu items — re-exported so the main
/// view can style inline menu buttons consistently.
pub const MENU_HOVER: u32 = BTN_HOVER;

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

/// One of the eight resize zones around the window perimeter.
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

/// Maps `Edge` variants to GPUI resize/cursor types.
impl Edge {
    /// Returns the GPUI `ResizeEdge` that triggers a compositor resize drag.
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

    /// Returns the directional cursor shown when the pointer enters this zone.
    fn cursor(self) -> CursorStyle {
        match self {
            Edge::Top | Edge::Bottom => CursorStyle::ResizeUpDown,
            Edge::Left | Edge::Right => CursorStyle::ResizeLeftRight,
            Edge::TopLeft | Edge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            Edge::TopRight | Edge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        }
    }
}

/// Builds a single absolutely-positioned hit strip for one resize edge or corner.
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
