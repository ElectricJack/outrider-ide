mod buffers;
mod camera;
mod focus;
mod theme;
mod treemap;
mod world;

use std::path::PathBuf;

use gpui::{px, size, App, AppContext as _, Bounds, WindowBounds, WindowOptions};
use gpui_platform::application;

use crate::treemap::TreemapView;

fn main() {
    let repo = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("no working directory"));
    eprintln!("indexing {}…", repo.display());
    let tree = match outrider_index::index_repo(&repo) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    };
    let layout = outrider_layout::layout(&tree);
    eprintln!("{} symbols laid out", layout.nodes.len());

    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| TreemapView::new(tree, layout, cx)),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
