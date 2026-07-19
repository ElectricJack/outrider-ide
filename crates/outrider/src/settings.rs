use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub const DEFAULT_DISK_CACHE_BYTES: u64 = 1_073_741_824;
pub(crate) const MAX_CACHE_MB: u32 = u32::MAX / (1024 * 1024);

#[derive(Debug)]
pub enum SettingsLoad {
    Loaded(Settings),
    Recovered { settings: Settings, warning: String },
}

impl SettingsLoad {
    pub fn into_parts(self) -> (Settings, Option<String>) {
        match self {
            Self::Loaded(settings) => (settings, None),
            Self::Recovered { settings, warning } => (settings, Some(warning)),
        }
    }
}

impl std::ops::Deref for SettingsLoad {
    type Target = Settings;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Loaded(settings) | Self::Recovered { settings, .. } => settings,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub filter_extensions: Vec<String>,
    pub filter_folders: Vec<String>,
    #[serde(default)]
    pub filter_files: Vec<String>,
    pub show_welcome: bool,
    #[serde(default = "default_cache_mb")]
    pub cache_mb: u32,
    #[serde(default = "default_node_padding")]
    pub node_padding: f64,
    #[serde(default = "default_show_churn")]
    pub show_churn: bool,
    #[serde(default)]
    pub max_display_lines: Option<u64>,
    #[serde(default)]
    pub(crate) disk_cache_bytes: BTreeMap<String, u64>,
}

fn default_node_padding() -> f64 {
    8.0
}

fn default_show_churn() -> bool {
    true
}

fn default_cache_mb() -> u32 {
    256
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            filter_extensions: vec![
                "exe", "dll", "obj", "o", "so", "dylib", "a", "lib", "pdb", "class", "pyc", "wasm",
                "bin", "dat", "db", "sqlite", "png", "jpg", "jpeg", "gif", "ico", "bmp", "svg",
                "mp3", "mp4", "wav", "zip", "tar", "gz", "7z", "rar", "pdf", "ttf", "otf", "woff",
                "woff2",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            filter_folders: vec![
                "target",
                "node_modules",
                "dist",
                "build",
                "__pycache__",
                ".next",
                ".nuxt",
                "out",
                "pkg",
                "vendor",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            filter_files: vec![],
            show_welcome: true,
            cache_mb: default_cache_mb(),
            node_padding: default_node_padding(),
            show_churn: default_show_churn(),
            max_display_lines: None,
            disk_cache_bytes: BTreeMap::new(),
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("outrider").join("settings.json"))
}

impl Settings {
    pub fn disk_cache_bytes(&self, project: &Path) -> u64 {
        self.disk_cache_bytes_for_key(&project_key(project))
    }

    pub fn set_disk_cache_bytes(&mut self, project: &Path, bytes: u64) {
        self.set_disk_cache_bytes_for_key(project_key(project), bytes);
    }

    fn disk_cache_bytes_for_key(&self, key: &str) -> u64 {
        self.disk_cache_bytes
            .get(key)
            .copied()
            .unwrap_or(DEFAULT_DISK_CACHE_BYTES)
    }

    fn set_disk_cache_bytes_for_key(&mut self, key: String, bytes: u64) {
        if bytes == 0 {
            self.disk_cache_bytes.remove(&key);
        } else {
            self.disk_cache_bytes.insert(key, bytes);
        }
    }

    pub fn load() -> SettingsLoad {
        let Some(path) = settings_path() else {
            return SettingsLoad::Recovered {
                settings: Self::default(),
                warning: "Unable to determine the settings directory; using defaults".into(),
            };
        };
        Self::load_from_path(&path)
    }

    fn load_from_path(path: &Path) -> SettingsLoad {
        let json = match std::fs::read_to_string(path) {
            Ok(json) => json,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return SettingsLoad::Loaded(Self::default());
            }
            Err(error) => {
                return SettingsLoad::Recovered {
                    settings: Self::default(),
                    warning: format!("Could not read settings: {error}; using defaults"),
                };
            }
        };
        match serde_json::from_str(&json) {
            Ok(settings) => Self::sanitize_loaded(settings),
            Err(error) => {
                let invalid_path = path.with_file_name("settings.invalid.json");
                let preservation = std::fs::rename(path, &invalid_path)
                    .map(|()| format!(" Preserved the invalid file at {}.", invalid_path.display()))
                    .unwrap_or_else(|rename_error| {
                        format!(" Could not preserve the invalid file: {rename_error}.")
                    });
                SettingsLoad::Recovered {
                    settings: Self::default(),
                    warning: format!(
                        "Settings were invalid ({error}); using defaults.{preservation}"
                    ),
                }
            }
        }
    }

    fn sanitize_loaded(mut settings: Self) -> SettingsLoad {
        let mut warnings = Vec::new();
        if settings.cache_mb == 0 || settings.cache_mb > MAX_CACHE_MB {
            settings.cache_mb = default_cache_mb();
            warnings.push(format!(
                "cache_mb was outside the safe range 1..={MAX_CACHE_MB}; restored the default"
            ));
        }
        let original_disk_entries = settings.disk_cache_bytes.len();
        settings.disk_cache_bytes.retain(|_, bytes| *bytes != 0);
        if settings.disk_cache_bytes.len() != original_disk_entries {
            warnings.push(
                "invalid zero-byte project disk cache values were restored to defaults".into(),
            );
        }
        if warnings.is_empty() {
            SettingsLoad::Loaded(settings)
        } else {
            SettingsLoad::Recovered {
                settings,
                warning: warnings.join("; "),
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = settings_path()
            .ok_or_else(|| "Unable to determine the settings directory".to_string())?;
        self.save_to_path(&path)
    }

    fn save_to_path(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create settings directory: {error}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("Could not serialize settings: {error}"))?;
        let temp_path = path.with_file_name("settings.tmp.json");
        std::fs::write(&temp_path, json)
            .map_err(|error| format!("Could not write temporary settings file: {error}"))?;
        if let Err(error) = std::fs::rename(&temp_path, path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(format!("Could not replace settings file: {error}"));
        }
        Ok(())
    }
}

fn project_key(project: &Path) -> String {
    let canonical = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let key = canonical.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let key = key.to_lowercase();
    let trimmed = key.trim_end_matches('/');
    if trimmed.is_empty() {
        key
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{Settings, SettingsLoad};

    #[test]
    fn new_project_defaults_to_one_gibibyte() {
        let settings = Settings::default();
        assert_eq!(settings.disk_cache_bytes_for_key("D:/repo"), 1_073_741_824);
    }

    #[test]
    fn project_disk_limits_are_independent() {
        let mut settings = Settings::default();
        settings.set_disk_cache_bytes_for_key("D:/one".into(), 512 * 1024 * 1024);
        settings.set_disk_cache_bytes_for_key("D:/two".into(), 2 * 1024 * 1024 * 1024);
        assert_eq!(
            settings.disk_cache_bytes_for_key("D:/one"),
            512 * 1024 * 1024
        );
        assert_eq!(
            settings.disk_cache_bytes_for_key("D:/two"),
            2 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn old_settings_json_receives_disk_defaults() {
        let settings: Settings = serde_json::from_str(
            r#"{"filter_extensions":[],"filter_folders":[],"show_welcome":false,"cache_mb":128}"#,
        )
        .unwrap();
        assert_eq!(settings.disk_cache_bytes_for_key("repo"), 1_073_741_824);
    }

    #[test]
    fn zero_memory_cache_recovers_to_safe_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"filter_extensions":[],"filter_folders":[],"show_welcome":false,"cache_mb":0}"#,
        )
        .unwrap();

        let SettingsLoad::Recovered { settings, warning } = Settings::load_from_path(&path) else {
            panic!("zero cache must be recovered");
        };

        assert_eq!(settings.cache_mb, 256);
        assert!(warning.contains("cache_mb"));
    }

    #[test]
    fn oversized_memory_cache_recovers_to_safe_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"filter_extensions":[],"filter_folders":[],"show_welcome":false,"cache_mb":4294967295}"#,
        )
        .unwrap();

        let SettingsLoad::Recovered { settings, warning } = Settings::load_from_path(&path) else {
            panic!("unsafe cache size must be recovered");
        };

        assert_eq!(settings.cache_mb, 256);
        assert!(warning.contains("cache_mb"));
    }

    #[test]
    fn zero_project_disk_cache_recovers_to_project_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"filter_extensions":[],"filter_folders":[],"show_welcome":false,"cache_mb":128,"disk_cache_bytes":{"repo":0}}"#,
        )
        .unwrap();

        let SettingsLoad::Recovered { settings, warning } = Settings::load_from_path(&path) else {
            panic!("zero disk allowance must be recovered");
        };

        assert_eq!(settings.disk_cache_bytes_for_key("repo"), 1_073_741_824);
        assert!(warning.contains("disk cache"));
    }

    #[test]
    fn malformed_settings_are_preserved_and_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "not json").unwrap();

        let loaded = Settings::load_from_path(&path);

        assert!(matches!(loaded, SettingsLoad::Recovered { .. }));
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.invalid.json")).unwrap(),
            "not json"
        );
    }

    #[test]
    fn newest_malformed_settings_replace_the_previous_invalid_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "new invalid").unwrap();
        std::fs::write(dir.path().join("settings.invalid.json"), "old invalid").unwrap();

        let loaded = Settings::load_from_path(&path);

        assert!(matches!(loaded, SettingsLoad::Recovered { .. }));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.invalid.json")).unwrap(),
            "new invalid"
        );
    }

    #[test]
    fn save_round_trip_preserves_project_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let mut settings = Settings::default();
        settings.set_disk_cache_bytes_for_key("project".into(), 42);

        settings.save_to_path(&path).unwrap();
        settings.set_disk_cache_bytes_for_key("project".into(), 84);
        settings.save_to_path(&path).unwrap();
        let SettingsLoad::Loaded(loaded) = Settings::load_from_path(&path) else {
            panic!("saved settings should load normally");
        };

        assert_eq!(loaded.disk_cache_bytes_for_key("project"), 84);
        assert!(!path.with_file_name("settings.tmp.json").exists());
    }
}
