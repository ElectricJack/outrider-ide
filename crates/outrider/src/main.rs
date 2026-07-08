mod world;
mod camera;

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;

struct HelloWorld;

impl Render for HelloWorld {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .justify_center()
            .items_center()
            .bg(rgb(0x1e2430))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(240.))
                            .h(px(120.))
                            .rounded_md()
                            .bg(rgb(0x2e7d32)),
                    )
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(0xffffff))
                            .child("outrider — milestone 0"),
                    ),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| HelloWorld),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
