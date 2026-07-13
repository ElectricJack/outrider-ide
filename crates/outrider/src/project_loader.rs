use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use outrider_index::{IndexProgress, SymbolTree};
use outrider_layout::PackLayout;

use crate::settings::Settings;

/// Project data prepared off the UI thread. Textures are intentionally absent:
/// the view creates an empty, project-scoped cache after accepting this result.
pub struct LoadResult {
    pub generation: u64,
    pub project_root: PathBuf,
    pub tree: SymbolTree,
    pub layout: PackLayout,
    pub warnings: Vec<String>,
    pub source_fingerprints: BTreeMap<String, u64>,
}

/// A cheap snapshot of the indexer's atomic progress counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadProgress {
    pub folder_name: String,
    pub phase: u8,
    pub files_total: usize,
    pub files_parsed: usize,
}

pub enum LoaderPoll {
    Idle,
    Loading(LoadProgress),
    Ready(Box<Result<LoadResult, String>>),
}

struct LoadingState {
    generation: u64,
    folder_name: String,
    progress: Arc<IndexProgress>,
    result: Arc<Mutex<Option<Result<LoadResult, String>>>>,
}

/// Owns background project loads and rejects results from superseded generations.
pub struct ProjectLoader {
    generation: u64,
    loading: Option<LoadingState>,
}

impl ProjectLoader {
    pub fn new() -> Self {
        Self {
            generation: 0,
            loading: None,
        }
    }

    pub fn begin_generation(&mut self) -> u64 {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("project load generation exhausted");
        self.generation
    }

    pub fn accepts(&self, generation: u64) -> bool {
        generation == self.generation
    }

    pub fn start(&mut self, project_root: PathBuf, settings: Settings) -> u64 {
        let generation = self.begin_generation();
        let folder_name = project_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| project_root.to_string_lossy().into_owned());
        let progress = Arc::new(IndexProgress::new());
        let result = Arc::new(Mutex::new(None));
        let worker_progress = Arc::clone(&progress);
        let worker_result = Arc::clone(&result);
        std::thread::spawn(move || {
            let loaded = load_project(&project_root, &settings, &worker_progress, generation);
            *worker_result.lock().unwrap() = Some(loaded);
        });
        self.loading = Some(LoadingState {
            generation,
            folder_name,
            progress,
            result,
        });
        generation
    }

    pub fn poll(&mut self) -> LoaderPoll {
        let Some(loading) = &self.loading else {
            return LoaderPoll::Idle;
        };
        let completed = loading.result.lock().unwrap().take();
        if let Some(result) = completed {
            let generation = loading.generation;
            self.loading = None;
            if self.accepts(generation) {
                LoaderPoll::Ready(Box::new(result))
            } else {
                LoaderPoll::Idle
            }
        } else {
            LoaderPoll::Loading(LoadProgress {
                folder_name: loading.folder_name.clone(),
                phase: loading.progress.phase.load(Ordering::Relaxed),
                files_total: loading.progress.files_total.load(Ordering::Relaxed),
                files_parsed: loading.progress.files_parsed.load(Ordering::Relaxed),
            })
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }
}

impl Default for ProjectLoader {
    fn default() -> Self {
        Self::new()
    }
}

pub fn load_project(
    project_root: &Path,
    settings: &Settings,
    progress: &IndexProgress,
    generation: u64,
) -> Result<LoadResult, String> {
    let outcome = outrider_index::index_repo_with_progress_outcome(
        project_root,
        &settings.filter_extensions,
        &settings.filter_folders,
        progress,
    )
    .map_err(|error| format!("{error:#}"))?;
    let tree = outcome.tree;
    let layout = outrider_layout::pack(&tree, &crate::world::pack_config());
    Ok(LoadResult {
        generation,
        project_root: project_root.to_path_buf(),
        tree,
        layout,
        warnings: outcome.warnings,
        source_fingerprints: outcome.source_fingerprints,
    })
}

#[cfg(test)]
mod tests {
    use super::{load_project, LoadResult, LoaderPoll, ProjectLoader};
    use crate::settings::Settings;

    #[test]
    fn newer_load_supersedes_older_result() {
        let mut loader = ProjectLoader::new();
        let first = loader.begin_generation();
        let second = loader.begin_generation();

        assert!(!loader.accepts(first));
        assert!(loader.accepts(second));
    }

    #[test]
    fn successful_load_contains_tree_and_layout_without_textures() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
        let progress = outrider_index::IndexProgress::new();

        let result = load_project(repo.path(), &Settings::default(), &progress, 7).unwrap();
        let LoadResult {
            generation,
            project_root,
            tree,
            layout,
            warnings,
            source_fingerprints,
        } = result;

        assert_eq!(generation, 7);
        assert_eq!(project_root, repo.path());
        assert!(!tree.root.children.is_empty());
        assert!(!layout.rects.is_empty());
        assert!(warnings.is_empty());
        assert!(source_fingerprints.contains_key("main.rs"));
    }

    #[test]
    fn background_start_returns_the_current_generation() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut loader = ProjectLoader::new();
        let generation = loader.start(repo.path().to_path_buf(), Settings::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);

        loop {
            match loader.poll() {
                LoaderPoll::Loading(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "background load timed out"
                    );
                    std::thread::yield_now();
                }
                LoaderPoll::Ready(result) => {
                    assert_eq!(result.unwrap().generation, generation);
                    break;
                }
                LoaderPoll::Idle => panic!("loader became idle before returning its result"),
            }
        }
    }
}
