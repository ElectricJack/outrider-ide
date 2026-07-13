//! Application entry point for the outrider treemap visualizer.
//! Opens a GPUI window immediately, then indexes the target repository on a
//! background thread while the loading shell remains responsive.

mod buffers;
mod camera;
mod chrome;
mod content;
mod focus;
mod palette;
mod project_loader;
mod rasterize;
mod settings;
mod texture_store;
mod theme;
mod treemap;
mod world;

use std::path::PathBuf;

use gpui::{
    px, size, App, AppContext as _, Bounds, WindowBounds, WindowDecorations, WindowOptions,
};
use gpui_platform::application;

use crate::treemap::TreemapView;

/// Resolve the repo passed as `argv[1]` (or chosen interactively) and open the
/// main treemap window before starting background indexing.
fn main() {
    let repo = match std::env::args().nth(1).map(PathBuf::from) {
        Some(path) => path,
        None => match rfd::FileDialog::new()
            .set_title("Open Project Folder")
            .pick_folder()
        {
            Some(path) => path,
            None => return,
        },
    };
    let settings = settings::Settings::load();

    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("outrider".into()),
                window_min_size: Some(size(px(480.), px(320.))),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| TreemapView::loading_shell(repo, settings, cx)),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
