use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub filter_extensions: Vec<String>,
    pub filter_folders: Vec<String>,
}

fn project_settings_path(project_root: &Path) -> PathBuf {
    project_root.join(".outrider").join("project.json")
}

impl ProjectSettings {
    pub fn exists(project_root: &Path) -> bool {
        project_settings_path(project_root).exists()
    }

    pub fn load(project_root: &Path) -> Option<Self> {
        let path = project_settings_path(project_root);
        let json = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&json).ok()
    }

    pub fn save(&self, project_root: &Path) -> Result<(), String> {
        let path = project_settings_path(project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create .outrider directory: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Could not serialize project settings: {e}"))?;
        let temp_path = path.with_file_name("project.tmp.json");
        std::fs::write(&temp_path, json)
            .map_err(|e| format!("Could not write project settings: {e}"))?;
        if let Err(e) = std::fs::rename(&temp_path, &path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(format!("Could not replace project settings file: {e}"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExtensionCategory {
    Code,
    Config,
    Docs,
    Styles,
    Markup,
    Binary,
    Media,
    Archive,
    Font,
    Generated,
    Other,
}

impl ExtensionCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Config => "Config",
            Self::Docs => "Docs",
            Self::Styles => "Styles",
            Self::Markup => "Markup",
            Self::Binary => "Binary",
            Self::Media => "Media",
            Self::Archive => "Archive",
            Self::Font => "Font",
            Self::Generated => "Generated",
            Self::Other => "Other",
        }
    }

    pub fn default_enabled(self) -> bool {
        match self {
            Self::Code | Self::Config | Self::Docs | Self::Styles | Self::Markup | Self::Other => {
                true
            }
            Self::Binary | Self::Media | Self::Archive | Self::Font | Self::Generated => false,
        }
    }

    pub fn sort_order(self) -> u8 {
        match self {
            Self::Code => 0,
            Self::Config => 1,
            Self::Docs => 2,
            Self::Styles => 3,
            Self::Markup => 4,
            Self::Other => 5,
            Self::Binary => 6,
            Self::Media => 7,
            Self::Archive => 8,
            Self::Font => 9,
            Self::Generated => 10,
        }
    }
}

pub fn categorize_extension(ext: &str) -> ExtensionCategory {
    match ext {
        "rs" | "py" | "js" | "jsx" | "ts" | "tsx" | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp"
        | "hxx" | "hh" | "cs" | "java" | "kt" | "kts" | "go" | "rb" | "php" | "swift" | "m"
        | "mm" | "r" | "R" | "scala" | "clj" | "cljs" | "erl" | "ex" | "exs" | "hs" | "lua"
        | "pl" | "pm" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" | "zig" | "nim"
        | "v" | "d" | "f90" | "f95" | "jl" | "ml" | "mli" | "fs" | "fsx" | "dart" | "groovy"
        | "vb" | "vbs" | "asm" | "s" | "S" => ExtensionCategory::Code,

        "toml" | "json" | "yaml" | "yml" | "xml" | "ini" | "cfg" | "conf" | "env"
        | "properties" | "plist" | "editorconfig" | "eslintrc" | "prettierrc" | "babelrc"
        | "nvmrc" | "dockerignore" | "gitattributes" => ExtensionCategory::Config,

        "md" | "markdown" | "txt" | "rst" | "adoc" | "org" | "tex" | "latex" | "rdoc" | "pod" => {
            ExtensionCategory::Docs
        }

        "css" | "scss" | "sass" | "less" | "styl" | "stylus" | "pcss" => ExtensionCategory::Styles,

        "html" | "htm" | "vue" | "svelte" | "erb" | "ejs" | "hbs" | "handlebars" | "pug"
        | "jade" | "slim" | "haml" | "twig" | "jinja" | "jinja2" | "mustache" | "njk" | "astro" => {
            ExtensionCategory::Markup
        }

        "exe" | "dll" | "obj" | "o" | "so" | "dylib" | "a" | "lib" | "pdb" | "class" | "pyc"
        | "pyo" | "wasm" | "bin" | "dat" | "db" | "sqlite" | "mdb" => ExtensionCategory::Binary,

        "png" | "jpg" | "jpeg" | "gif" | "ico" | "bmp" | "tiff" | "tif" | "webp" | "avif"
        | "mp3" | "mp4" | "wav" | "flac" | "ogg" | "avi" | "mkv" | "mov" | "wmv" | "webm"
        | "aac" | "m4a" => ExtensionCategory::Media,

        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "lz" | "lzma" | "zst" | "cab"
        | "dmg" | "iso" | "jar" | "war" | "ear" => ExtensionCategory::Archive,

        "ttf" | "otf" | "woff" | "woff2" | "eot" => ExtensionCategory::Font,

        "pdf" | "map" | "min" | "bundle" | "chunk" => ExtensionCategory::Generated,

        _ => ExtensionCategory::Other,
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorizes_known_extensions() {
        assert_eq!(categorize_extension("rs"), ExtensionCategory::Code);
        assert_eq!(categorize_extension("json"), ExtensionCategory::Config);
        assert_eq!(categorize_extension("md"), ExtensionCategory::Docs);
        assert_eq!(categorize_extension("css"), ExtensionCategory::Styles);
        assert_eq!(categorize_extension("html"), ExtensionCategory::Markup);
        assert_eq!(categorize_extension("exe"), ExtensionCategory::Binary);
        assert_eq!(categorize_extension("png"), ExtensionCategory::Media);
        assert_eq!(categorize_extension("zip"), ExtensionCategory::Archive);
        assert_eq!(categorize_extension("ttf"), ExtensionCategory::Font);
        assert_eq!(categorize_extension("pdf"), ExtensionCategory::Generated);
        assert_eq!(categorize_extension("xyz"), ExtensionCategory::Other);
    }

    #[test]
    fn code_defaults_enabled_binary_disabled() {
        assert!(ExtensionCategory::Code.default_enabled());
        assert!(!ExtensionCategory::Binary.default_enabled());
        assert!(!ExtensionCategory::Media.default_enabled());
        assert!(ExtensionCategory::Config.default_enabled());
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let settings = ProjectSettings {
            filter_extensions: vec!["exe".into(), "png".into()],
            filter_folders: vec!["target".into()],
        };
        settings.save(dir.path()).unwrap();
        let loaded = ProjectSettings::load(dir.path()).expect("should load");
        assert_eq!(loaded.filter_extensions, settings.filter_extensions);
        assert_eq!(loaded.filter_folders, settings.filter_folders);
    }

    #[test]
    fn exists_false_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!ProjectSettings::exists(dir.path()));
    }

    #[test]
    fn exists_true_after_save() {
        let dir = tempfile::tempdir().unwrap();
        let settings = ProjectSettings {
            filter_extensions: vec![],
            filter_folders: vec![],
        };
        settings.save(dir.path()).unwrap();
        assert!(ProjectSettings::exists(dir.path()));
    }
}
