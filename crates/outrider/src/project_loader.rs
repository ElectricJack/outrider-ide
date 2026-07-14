use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

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
    result: Receiver<Result<LoadResult, String>>,
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
        let worker_root = project_root.clone();
        self.start_with(project_root, move |progress, generation| {
            load_project(&worker_root, &settings, progress, generation)
        })
    }

    fn start_with<F>(&mut self, project_root: PathBuf, load: F) -> u64
    where
        F: FnOnce(&IndexProgress, u64) -> Result<LoadResult, String> + Send + 'static,
    {
        let generation = self.begin_generation();
        let folder_name = project_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| project_root.to_string_lossy().into_owned());
        let progress = Arc::new(IndexProgress::new());
        let (result_sender, result) = mpsc::sync_channel(1);
        let worker_progress = Arc::clone(&progress);
        std::thread::spawn(move || {
            let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                load(&worker_progress, generation)
            }))
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("unknown panic");
                Err(format!("project loading worker panicked: {message}"))
            });
            let _ = result_sender.send(loaded);
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
        let completed = match loading.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(
                "project loading worker disconnected before returning a result".into(),
            )),
        };
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
    use super::{load_project, LoadResult, LoaderPoll, LoadingState, ProjectLoader};
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

    #[test]
    fn worker_panic_becomes_a_recoverable_error() {
        let mut loader = ProjectLoader::new();
        loader.start_with(std::path::PathBuf::from("panic-project"), |_, _| {
            panic!("simulated worker failure")
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match loader.poll() {
                LoaderPoll::Loading(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "panic was not reported"
                    );
                    std::thread::yield_now();
                }
                LoaderPoll::Ready(result) => {
                    let Err(error) = *result else {
                        panic!("panicked worker returned a successful load")
                    };
                    assert!(error.contains("panicked"));
                    break;
                }
                LoaderPoll::Idle => panic!("panicked worker disconnected without an error"),
            }
        }
    }

    #[test]
    fn disconnected_worker_becomes_a_recoverable_error() {
        let mut loader = ProjectLoader::new();
        let generation = loader.begin_generation();
        let (sender, result) = std::sync::mpsc::sync_channel(1);
        drop(sender);
        loader.loading = Some(LoadingState {
            generation,
            folder_name: "disconnected-project".into(),
            progress: std::sync::Arc::new(outrider_index::IndexProgress::new()),
            result,
        });

        let LoaderPoll::Ready(result) = loader.poll() else {
            panic!("disconnected worker did not return a recoverable result")
        };
        let Err(error) = *result else {
            panic!("disconnected worker returned a successful load")
        };
        assert!(error.contains("disconnected"));
    }

    #[test]
    fn older_completion_cannot_replace_an_overlapping_newer_load() {
        let first_repo = tempfile::tempdir().unwrap();
        let second_repo = tempfile::tempdir().unwrap();
        std::fs::write(first_repo.path().join("first.rs"), "fn first() {}\n").unwrap();
        std::fs::write(second_repo.path().join("second.rs"), "fn second() {}\n").unwrap();
        let first_path = first_repo.path().to_path_buf();
        let second_path = second_repo.path().to_path_buf();
        let (release_first, wait_first) = std::sync::mpsc::channel();
        let (first_finished, first_done) = std::sync::mpsc::channel();
        let (release_second, wait_second) = std::sync::mpsc::channel();
        let settings = Settings::default();
        let first_settings = settings.clone();
        let first_worker_path = first_path.clone();
        let second_worker_path = second_path.clone();
        let second_folder_name = second_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let mut loader = ProjectLoader::new();

        let first_generation = loader.start_with(first_path, move |progress, generation| {
            wait_first.recv().unwrap();
            let result = load_project(&first_worker_path, &first_settings, progress, generation);
            first_finished.send(()).unwrap();
            result
        });
        let second_generation = loader.start_with(second_path, move |progress, generation| {
            wait_second.recv().unwrap();
            load_project(&second_worker_path, &settings, progress, generation)
        });

        release_first.send(()).unwrap();
        first_done.recv().unwrap();
        match loader.poll() {
            LoaderPoll::Loading(progress) => assert_eq!(progress.folder_name, second_folder_name),
            _ => panic!("older completion affected the current load"),
        }
        release_second.send(()).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match loader.poll() {
                LoaderPoll::Loading(_) => {
                    assert!(std::time::Instant::now() < deadline, "newer load timed out");
                    std::thread::yield_now();
                }
                LoaderPoll::Ready(result) => {
                    assert_eq!(result.unwrap().generation, second_generation);
                    assert!(!loader.accepts(first_generation));
                    break;
                }
                LoaderPoll::Idle => panic!("newer load disappeared"),
            }
        }
    }
}
