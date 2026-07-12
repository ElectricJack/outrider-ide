use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub filter_extensions: Vec<String>,
    pub filter_folders: Vec<String>,
    pub show_welcome: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            filter_extensions: vec![
                "exe", "dll", "obj", "o", "so", "dylib", "a", "lib", "pdb",
                "class", "pyc", "wasm", "bin", "dat", "db", "sqlite",
                "png", "jpg", "jpeg", "gif", "ico", "bmp", "svg",
                "mp3", "mp4", "wav", "zip", "tar", "gz", "7z", "rar",
                "pdf", "ttf", "otf", "woff", "woff2",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            filter_folders: vec![
                "target", "node_modules", "dist", "build", "__pycache__",
                ".next", ".nuxt", "out", "pkg", "vendor",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            show_welcome: true,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("outrider").join("settings.json"))
}

impl Settings {
    /// Return the path to the settings file, if determinable.
    pub fn path() -> Option<PathBuf> {
        settings_path()
    }

    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}
