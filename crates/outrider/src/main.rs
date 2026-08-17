//! Application entry point for the outrider treemap visualizer.
//! Opens a GPUI window immediately, then indexes the target repository on a
//! background thread while the loading shell remains responsive.

mod buffers;
mod camera;
mod content;
mod focus;
mod interaction;
mod layout_transition;
mod navigation;
mod overlays;
mod paint_model;
mod project_loader;
mod project_settings;
mod rasterize;
mod settings;
mod texture_store;
mod theme;
mod treemap;
mod view;
mod world;

use std::path::PathBuf;

use gpui::{px, size, App, AppContext as _, Bounds, Menu, MenuItem, WindowBounds, WindowOptions};
use gpui_platform::application;

use crate::treemap::{
    ClearDiskCache, OpenFilePalette, OpenFolder, OpenSymbolPalette, Quit, RevealInFileManager,
    ToggleProjectSettings, ToggleSettings, TreemapView,
};

pub(crate) const fn uses_native_application_menu(target_os: &str) -> bool {
    matches!(target_os.as_bytes(), b"macos")
}

#[cfg(test)]
mod tests {
    use super::uses_native_application_menu;

    #[test]
    fn windows_needs_an_in_window_application_menu() {
        assert!(!uses_native_application_menu("windows"));
    }

    #[test]
    fn macos_uses_the_native_application_menu() {
        assert!(uses_native_application_menu("macos"));
    }
}

const CLI_VERBS: &[&str] = &[
    "view", "set", "fill", "mask", "edges", "mark", "note", "panel",
    "frame", "focus", "home", "follow", "tour", "metric", "layer",
    "query", "status",
];

fn maybe_dispatch_cli() {
    let mut args = std::env::args_os().skip(1);
    let Some(first) = args.next() else { return };
    let Some(verb) = first.to_str() else { return };
    if !CLI_VERBS.contains(&verb) || std::path::Path::new(verb).is_dir() {
        return;
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| {
            d.join(if cfg!(windows) { "outrider-cli.exe" } else { "outrider-cli" })
        }))
        .filter(|p| p.exists())
        .unwrap_or_else(|| "outrider-cli".into());
    match std::process::Command::new(exe).arg(verb).args(args).status() {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("outrider: cannot run outrider-cli: {e}");
            std::process::exit(2);
        }
    }
}

fn main() {
    maybe_dispatch_cli();
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
            gpui::KeyBinding::new("secondary-shift-,", ToggleProjectSettings, None),
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
                    MenuItem::action("Project Settings...", ToggleProjectSettings),
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
