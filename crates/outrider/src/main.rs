//! Application entry point for the outrider treemap visualizer.
//! Opens a GPUI window immediately, then indexes the target repository on a
//! background thread while the loading shell remains responsive.

mod buffers;
mod camera;
mod content;
mod focus;
mod interaction;
mod navigation;
mod overlays;
mod paint_model;
mod palette;
mod project_loader;
mod rasterize;
mod settings;
mod texture_store;
mod theme;
mod treemap;
mod world;

use std::path::PathBuf;

use gpui::{px, size, App, AppContext as _, Bounds, Menu, MenuItem, WindowBounds, WindowOptions};
use gpui_platform::application;

use crate::treemap::{
    ClearDiskCache, OpenFilePalette, OpenFolder, OpenSymbolPalette, Quit, RevealInFileManager,
    ToggleSettings, TreemapView,
};

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
                app_id: Some("outrider".into()),
                window_min_size: Some(size(px(480.), px(320.))),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| TreemapView::loading_shell(repo, settings, cx)),
        )
        .expect("failed to open window");

        cx.bind_keys([
            gpui::KeyBinding::new("secondary-o", OpenFolder, None),
            gpui::KeyBinding::new("secondary-p", OpenFilePalette, None),
            gpui::KeyBinding::new("secondary-t", OpenSymbolPalette, None),
            gpui::KeyBinding::new("secondary-,", ToggleSettings, None),
            gpui::KeyBinding::new("secondary-shift-e", RevealInFileManager, None),
            gpui::KeyBinding::new("secondary-q", Quit, None),
        ]);

        cx.on_action(|_: &Quit, cx| cx.quit());

        cx.set_menus(vec![
            Menu {
                name: "Outrider".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("Settings...", ToggleSettings),
                    MenuItem::separator(),
                    MenuItem::action("Quit outrider", Quit),
                ],
            },
            Menu {
                name: "File".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("Open Folder...", OpenFolder),
                    MenuItem::separator(),
                    MenuItem::action("Clear Project Disk Cache", ClearDiskCache),
                ],
            },
            Menu {
                name: "Navigate".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("Go to File...", OpenFilePalette),
                    MenuItem::action("Go to Symbol...", OpenSymbolPalette),
                    MenuItem::separator(),
                    MenuItem::action("Reveal in File Manager", RevealInFileManager),
                ],
            },
        ]);

        cx.activate(true);
    });
}
