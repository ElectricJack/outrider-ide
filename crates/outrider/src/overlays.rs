use gpui::{div, prelude::*, px, rgb, rgba, transparent_black, ElementId, Pixels};
use outrider_index::SymbolId;

use crate::project_loader::LoadProgress;
use crate::theme;

pub(crate) struct ContextMenu {
    pub(crate) position: gpui::Point<Pixels>,
    pub(crate) target: SymbolId,
}

/// Severity controls the visual treatment of a transient notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NotificationLevel {
    Warning,
}

/// User-visible recoverable feedback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Notification {
    pub(crate) message: String,
    pub(crate) level: NotificationLevel,
}

impl Notification {
    pub(crate) fn warning(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            level: NotificationLevel::Warning,
        }
    }
}

/// Notification stack; the newest entry is visible until dismissed. The
/// oldest warning is evicted when the stack grows beyond 64 entries.
#[derive(Default)]
pub(crate) struct Notifications {
    entries: Vec<Notification>,
}

impl Notifications {
    pub(crate) fn push(&mut self, notification: Notification) {
        self.entries.push(notification);
        if self.entries.len() > 64 {
            self.entries.remove(0);
        }
    }

    pub(crate) fn visible(&self) -> Option<&Notification> {
        self.entries.last()
    }

    pub(crate) fn dismiss_visible(&mut self) {
        self.entries.pop();
    }
}

/// Build the visual shell for the currently visible notification. Event
/// handling remains at the `TreemapView` composition boundary.
pub(crate) fn notification_element(notification: &Notification) -> gpui::Stateful<gpui::Div> {
    div()
        .id("notification")
        .absolute()
        .top(px(12.0))
        .left(px(12.0))
        .right(px(12.0))
        .px(px(12.0))
        .py(px(8.0))
        .bg(rgb(0x3a2020_u32))
        .border_1()
        .border_color(rgb(0xff8a80_u32))
        .rounded(px(4.0))
        .text_size(px(12.0))
        .font_family(theme::FONT_FAMILY_SANS)
        .text_color(rgb(theme::TEXT_PRIMARY))
        .cursor_pointer()
        .child(notification.message.clone())
}

pub(crate) fn loading_element(state: &LoadProgress, viewport_width: f64) -> gpui::Div {
    let (status_text, fraction) = match state.phase {
        0 => ("Scanning files…".to_string(), 0.0_f32),
        1 if state.files_total > 0 => (
            format!(
                "Parsing {}/{} files…",
                state.files_parsed, state.files_total
            ),
            state.files_parsed as f32 / state.files_total as f32,
        ),
        1 => ("Parsing…".to_string(), 0.0),
        2 => ("Building symbol tree…".to_string(), 1.0),
        _ => ("Done".to_string(), 1.0),
    };
    let bar_width = 300.0_f32.min(viewport_width as f32 - 80.0);

    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x00000088))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(16.0))
                .px(px(40.0))
                .py(px(32.0))
                .bg(rgb(theme::CODE_BG))
                .border_1()
                .border_color(rgb(theme::border_for(theme::CODE_BG)))
                .rounded(px(8.0))
                .text_color(rgb(theme::TEXT_PRIMARY))
                .font_family(theme::FONT_FAMILY_SANS)
                .child(
                    div()
                        .text_size(px(16.0))
                        .child(format!("Indexing {}…", state.folder_name)),
                )
                .child(
                    div()
                        .w(px(bar_width))
                        .h(px(6.0))
                        .rounded(px(3.0))
                        .bg(rgb(0x333340))
                        .child(
                            div()
                                .h_full()
                                .w(px(bar_width * fraction))
                                .rounded(px(3.0))
                                .bg(rgb(theme::FOCUS_BORDER)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(status_text),
                ),
        )
}

pub(crate) fn context_menu_row(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(7.0))
        .text_size(px(13.0))
        .font_family(theme::FONT_FAMILY_SANS)
        .text_color(rgb(theme::TEXT_PRIMARY))
        .cursor_pointer()
        .hover(|element| element.bg(rgb(0x2a3040_u32)))
        .child(label)
}

pub(crate) fn context_menu_separator() -> gpui::Div {
    div()
        .h(px(1.0))
        .mx(px(8.0))
        .my(px(4.0))
        .bg(rgb(0x2a2d32_u32))
}

pub(crate) fn context_menu_shell(x: f32, y: f32) -> gpui::Div {
    div()
        .absolute()
        .top(px(y))
        .left(px(x))
        .w(px(210.0))
        .bg(rgb(theme::CODE_BG))
        .border_1()
        .border_color(rgb(theme::FOCUS_BORDER))
        .rounded(px(4.0))
        .overflow_hidden()
        .shadow_lg()
}

pub(crate) fn backdrop() -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(rgba(0x00000066))
}

pub(crate) fn centered_panel(top: f32, left: f32, width: f32) -> gpui::Div {
    div()
        .absolute()
        .top(px(top))
        .left(px(left))
        .w(px(width))
        .bg(rgb(theme::CODE_BG))
        .border_1()
        .border_color(rgb(theme::FOCUS_BORDER))
        .rounded(px(6.0))
        .overflow_hidden()
        .px(px(24.0))
        .py(px(20.0))
}

pub(crate) fn settings_input(
    id: &'static str,
    text: String,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(ElementId::Name(id.into()))
        .px(px(8.0))
        .py(px(6.0))
        .mb(px(12.0))
        .max_h(px(80.0))
        .overflow_y_scroll()
        .bg(rgb(0x1a1d21_u32))
        .border_1()
        .border_color(rgb(if active {
            theme::FOCUS_BORDER
        } else {
            0x333340
        }))
        .rounded(px(3.0))
        .cursor_pointer()
        .text_size(px(12.0))
        .font_family(theme::FONT_FAMILY)
        .text_color(rgb(if active {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        }))
        .child(text)
}

pub(crate) fn labeled_field(label: &'static str, input: impl gpui::IntoElement) -> gpui::Div {
    div()
        .child(
            div()
                .text_size(px(13.0))
                .font_family(theme::FONT_FAMILY_SANS)
                .text_color(rgb(theme::FOCUS_BORDER))
                .pb(px(4.0))
                .child(label),
        )
        .child(input)
}

pub(crate) fn action_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let button = div()
        .id(ElementId::Name(id.into()))
        .px(px(14.0))
        .py(px(7.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .text_size(px(13.0))
        .font_family(theme::FONT_FAMILY_SANS)
        .child(label);
    if primary {
        button
            .bg(rgb(theme::FOCUS_BORDER))
            .text_color(rgb(0x000000_u32))
    } else {
        button
            .border_1()
            .border_color(rgb(theme::TEXT_SECONDARY))
            .text_color(rgb(theme::TEXT_SECONDARY))
    }
}

pub(crate) fn settings_element(
    map_width: f64,
    fields: Vec<gpui::Div>,
    validation: Option<String>,
    actions: Vec<gpui::Stateful<gpui::Div>>,
) -> gpui::Div {
    const WIDTH: f32 = 600.0;
    let left = ((map_width as f32 - WIDTH) / 2.0).max(0.0);
    backdrop().child(
        centered_panel(80.0, left, WIDTH)
            .child(
                div()
                    .text_size(px(18.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .pb(px(14.0))
                    .child("Settings"),
            )
            .children(fields)
            .children(validation.map(|message| {
                div()
                    .text_size(px(12.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(0xff8a80_u32))
                    .pb(px(12.0))
                    .child(message)
            }))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_family(theme::FONT_FAMILY)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .pb(px(14.0))
                    .child("Tab to switch fields. Type to edit. Esc to cancel."),
            )
            .child(div().h(px(1.0)).mb(px(14.0)).bg(rgb(0x2a2d32_u32)))
            .child(div().flex().flex_row().gap(px(10.0)).children(actions)),
    )
}

pub(crate) fn welcome_element(
    map_width: f64,
    actions: Vec<gpui::Stateful<gpui::Div>>,
) -> gpui::Div {
    const WIDTH: f32 = 600.0;
    let left = ((map_width as f32 - WIDTH) / 2.0).max(0.0);
    #[cfg(target_os = "macos")]
    const M: &str = "Cmd";
    #[cfg(not(target_os = "macos"))]
    const M: &str = "Ctrl";
    let keybindings: Vec<(String, &str)> = vec![
        ("Enter / Esc".into(), "Step into / out of focused node"),
        ("Arrow keys".into(), "Move focus spatially"),
        ("Alt+Left / Alt+Right".into(), "Navigate history back / forward"),
        ("Home".into(), "Reset camera to fit all nodes"),
        ("End".into(), "Frame the focused node"),
        ("Scroll".into(), "Zoom in / out at cursor"),
        ("Click".into(), "Set focus"),
        ("Drag".into(), "Pan the view"),
        (format!("{M}+O"), "Open folder"),
        (format!("{M}+P"), "Go to file"),
        (format!("{M}+T"), "Go to symbol"),
        (format!("{M}+,"), "Settings"),
        (format!("{M}+Shift+E"), "Reveal in file manager"),
    ];
    let rows = keybindings.into_iter().map(|(key, action)| {
        div()
            .flex()
            .flex_row()
            .py(px(3.0))
            .child(
                div()
                    .w(px(200.0))
                    .text_size(px(12.0))
                    .font_family(theme::FONT_FAMILY)
                    .text_color(rgb(theme::FOCUS_BORDER))
                    .child(key),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child(action),
            )
    });
    centered_panel(80.0, left, WIDTH)
        .child(
            div()
                .text_size(px(18.0))
                .font_family(theme::FONT_FAMILY_SANS)
                .text_color(rgb(theme::TEXT_PRIMARY))
                .pb(px(14.0))
                .child("Welcome to Outrider"),
        )
        .children(rows)
        .child(
            div()
                .h(px(1.0))
                .mt(px(14.0))
                .mb(px(14.0))
                .bg(rgb(0x2a2d32_u32)),
        )
        .child(div().flex().flex_row().gap(px(10.0)).children(actions))
}

fn checkbox_icon(checked: bool) -> gpui::Div {
    let base = div()
        .w(px(14.0))
        .h(px(14.0))
        .rounded(px(3.0))
        .border_1()
        .flex()
        .items_center()
        .justify_center();
    if checked {
        base.bg(rgb(theme::FOCUS_BORDER))
            .border_color(rgb(theme::FOCUS_BORDER))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(0x000000_u32))
                    .child("✓"),
            )
    } else {
        base.bg(transparent_black())
            .border_color(rgb(theme::TEXT_SECONDARY))
    }
}

pub(crate) fn toolbar_toggle(id: impl Into<ElementId>, label: &str, checked: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .bg(rgba(0x00000080))
        .child(checkbox_icon(checked))
        .child(
            div()
                .text_size(px(12.0))
                .font_family(theme::FONT_FAMILY_SANS)
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(label.to_string()),
        )
}

pub(crate) fn checkbox_row(
    id: ElementId,
    label: String,
    detail: String,
    checked: bool,
    selected: bool,
    indent_px: f32,
) -> gpui::Stateful<gpui::Div> {
    let bg = if selected { 0x2a3040_u32 } else { 0x00000000 };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .pl(px(indent_px))
        .pr(px(8.0))
        .py(px(3.0))
        .gap(px(6.0))
        .bg(rgb(bg))
        .rounded(px(2.0))
        .cursor_pointer()
        .child(checkbox_icon(checked))
        .child(
            div()
                .flex_grow(1.0)
                .text_size(px(12.0))
                .font_family(theme::FONT_FAMILY_SANS)
                .text_color(rgb(theme::TEXT_PRIMARY))
                .child(label),
        )
        .child(
            div()
                .text_size(px(11.0))
                .font_family(theme::FONT_FAMILY_SANS)
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(detail),
        )
}

pub(crate) fn category_header(
    arrow_id: ElementId,
    checkbox_id: ElementId,
    label: String,
    detail: String,
    checked: bool,
    expanded: bool,
    selected: bool,
    on_arrow: impl FnOnce(gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div>,
    on_checkbox: impl FnOnce(gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div>,
) -> gpui::Div {
    let arrow_char = if expanded { "▾" } else { "▸" };
    let bg = if selected { 0x2a3040_u32 } else { 0x00000000 };
    let arrow = div()
        .id(arrow_id)
        .cursor_pointer()
        .text_size(px(11.0))
        .font_family(theme::FONT_FAMILY_SANS)
        .text_color(rgb(theme::TEXT_SECONDARY))
        .w(px(14.0))
        .child(arrow_char);
    let checkbox = div()
        .id(checkbox_id)
        .cursor_pointer()
        .child(checkbox_icon(checked));
    div()
        .flex()
        .flex_row()
        .items_center()
        .px(px(8.0))
        .py(px(4.0))
        .gap(px(6.0))
        .bg(rgb(bg))
        .rounded(px(2.0))
        .child(on_arrow(arrow))
        .child(on_checkbox(checkbox))
        .child(
            div()
                .flex_grow(1.0)
                .text_size(px(13.0))
                .font_family(theme::FONT_FAMILY_SANS)
                .text_color(rgb(theme::TEXT_PRIMARY))
                .child(label),
        )
        .child(
            div()
                .text_size(px(11.0))
                .font_family(theme::FONT_FAMILY_SANS)
                .text_color(rgb(theme::TEXT_SECONDARY))
                .child(detail),
        )
}

pub(crate) fn project_setup_element(
    map_width: f64,
    map_height: f64,
    title: String,
    stats_line: String,
    ext_rows: Vec<gpui::AnyElement>,
    folder_rows: Vec<gpui::AnyElement>,
    actions: Vec<gpui::Stateful<gpui::Div>>,
) -> gpui::Div {
    const WIDTH: f32 = 750.0;
    let left = ((map_width as f32 - WIDTH) / 2.0).max(0.0);
    let max_panel_h = (map_height as f32 - 120.0).max(300.0);
    let col_h = (max_panel_h - 180.0).max(150.0);

    backdrop().child(
        centered_panel(60.0, left, WIDTH)
            .max_h(px(max_panel_h))
            .child(
                div()
                    .text_size(px(18.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .pb(px(6.0))
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .pb(px(14.0))
                    .child(stats_line),
            )
            .child(div().h(px(1.0)).mb(px(12.0)).bg(rgb(0x2a2d32_u32)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex_grow(1.0)
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_family(theme::FONT_FAMILY_SANS)
                                    .text_color(rgb(theme::FOCUS_BORDER))
                                    .pb(px(6.0))
                                    .child("File Types"),
                            )
                            .child(
                                div()
                                    .id("ext-scroll")
                                    .h(px(col_h))
                                    .overflow_y_scroll()
                                    .children(ext_rows),
                            ),
                    )
                    .child(
                        div()
                            .w(px(1.0))
                            .h(px(col_h + 20.0))
                            .bg(rgb(0x2a2d32_u32)),
                    )
                    .child(
                        div()
                            .flex_grow(1.0)
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_family(theme::FONT_FAMILY_SANS)
                                    .text_color(rgb(theme::FOCUS_BORDER))
                                    .pb(px(6.0))
                                    .child("Folders"),
                            )
                            .child(
                                div()
                                    .id("folder-scroll")
                                    .h(px(col_h))
                                    .overflow_y_scroll()
                                    .children(folder_rows),
                            ),
                    ),
            )
            .child(div().h(px(1.0)).mt(px(12.0)).mb(px(10.0)).bg(rgb(0x2a2d32_u32)))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_family(theme::FONT_FAMILY)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .pb(px(10.0))
                    .child("Tab: switch panels  |  Space: toggle  |  Enter: start indexing  |  Esc: cancel"),
            )
            .child(div().flex().flex_row().gap(px(10.0)).children(actions)),
    )
}

pub(crate) fn pre_scan_loading_element(map_width: f64, folder_name: &str) -> gpui::Div {
    const WIDTH: f32 = 400.0;
    let left = ((map_width as f32 - WIDTH) / 2.0).max(0.0);
    backdrop().child(
        centered_panel(200.0, left, WIDTH)
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .text_size(px(16.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .pb(px(10.0))
                    .child(format!("Scanning {folder_name}…")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .child("Discovering files and folders"),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::{Notification, Notifications};

    #[test]
    fn newest_notification_is_visible_and_dismissible() {
        let mut notifications = Notifications::default();
        notifications.push(Notification::warning("cache unavailable"));
        assert_eq!(
            notifications.visible().unwrap().message,
            "cache unavailable"
        );
        notifications.dismiss_visible();
        assert!(notifications.visible().is_none());
    }

    #[test]
    fn dismissing_newest_reveals_previous_notification() {
        let mut notifications = Notifications::default();
        notifications.push(Notification::warning("first"));
        notifications.push(Notification::warning("second"));
        notifications.dismiss_visible();
        assert_eq!(notifications.visible().unwrap().message, "first");
    }

    #[test]
    fn notification_stack_evicts_the_oldest_beyond_64_entries() {
        let mut notifications = Notifications::default();
        for index in 0..=64 {
            notifications.push(Notification::warning(index.to_string()));
        }
        assert_eq!(notifications.visible().unwrap().message, "64");
        for _ in 0..63 {
            notifications.dismiss_visible();
        }
        assert_eq!(notifications.visible().unwrap().message, "1");
        notifications.dismiss_visible();
        assert!(notifications.visible().is_none());
    }
}
