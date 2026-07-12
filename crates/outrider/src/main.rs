//! Application entry point for the outrider treemap visualizer.
//! Indexes the target repository, runs the shelf-pack layout pass, then
//! opens a GPUI window hosting `TreemapView` with client-side decorations.

mod buffers;
mod camera;
mod chrome;
mod content;
mod focus;
mod rasterize;
mod palette;
mod settings;
mod theme;
mod treemap;
mod world;

use std::path::PathBuf;

use gpui::{
    px, size, App, AppContext as _, Bounds, WindowBounds, WindowDecorations, WindowOptions,
};
use gpui_platform::application;

use crate::treemap::TreemapView;

/// Index the repo passed as `argv[1]` (or cwd), pack the layout, and open
/// the main treemap window.
fn main() {
    let repo = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("no working directory"));
    let settings = settings::Settings::load();
    eprintln!("indexing {}…", repo.display());
    let tree = match outrider_index::index_repo(&repo, &settings.filter_extensions, &settings.filter_folders) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    };
    let layout = outrider_layout::pack(&tree, &world::pack_config());
    eprintln!("{} symbols packed", layout.rects.len());

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
            |_, cx| cx.new(|cx| TreemapView::new(tree, layout, cx)),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
