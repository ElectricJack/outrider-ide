use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use outrider_index::{IndexProgress, SymbolTree};
use outrider_layout::{pack_progressive, PackLayout};

use crate::settings::Settings;
use crate::texture_store::ProjectTextureNamespace;

/// Project data prepared off the UI thread. Textures are intentionally absent:
/// the view creates an empty, project-scoped cache after accepting this result.
pub struct ProjectPreview {
    pub generation: u64,
    pub project_root: PathBuf,
    pub tree: SymbolTree,
    pub layout: PackLayout,
    pub warnings: Vec<String>,
    pub source_fingerprints: BTreeMap<String, u64>,
    /// Per-project disk allowance resolved on the loader thread so project
    /// installation never canonicalizes the project path on the UI thread.
    pub disk_cache_bytes: u64,
    /// Canonical project cache identity prepared off the UI thread. Failure is
    /// non-fatal so the tree and layout remain usable without disk caching.
    pub project_namespace: Result<ProjectTextureNamespace, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPhase {
    Scanning,
    Parsing,
    BuildingTree,
    Packing,
}

/// A cheap snapshot of loader-owned progress state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadProgress {
    pub folder_name: String,
    pub phase: LoadPhase,
    pub completed: usize,
    pub total: usize,
}

pub enum LoaderPoll {
    Idle,
    Loading(LoadProgress),
    Preview(Box<ProjectPreview>),
    Snapshot {
        generation: u64,
        layout: PackLayout,
    },
    Complete {
        generation: u64,
        layout: PackLayout,
    },
    Failed {
        generation: u64,
        message: String,
        preview_delivered: bool,
    },
}

enum ControlEvent {
    PackingStarted {
        generation: u64,
        total: usize,
    },
    Preview(Box<ProjectPreview>),
    Complete {
        generation: u64,
        layout: PackLayout,
    },
    Failed {
        generation: u64,
        message: String,
        preview_delivered: bool,
    },
}

impl ControlEvent {
    fn generation(&self) -> u64 {
        match self {
            Self::PackingStarted { generation, .. }
            | Self::Complete { generation, .. }
            | Self::Failed { generation, .. } => *generation,
            Self::Preview(preview) => preview.generation,
        }
    }
}

struct LayoutSnapshot {
    generation: u64,
    layout: PackLayout,
}

#[derive(Default)]
struct SnapshotMailbox(Mutex<Option<LayoutSnapshot>>);

impl SnapshotMailbox {
    fn publish(&self, snapshot: LayoutSnapshot) {
        if let Ok(mut pending) = self.0.try_lock() {
            *pending = Some(snapshot);
        }
    }

    fn take(&self) -> Option<LayoutSnapshot> {
        self.0.lock().ok()?.take()
    }
}

struct LoadingState {
    generation: u64,
    folder_name: String,
    index_progress: Arc<IndexProgress>,
    packing_completed: Arc<AtomicUsize>,
    packing_total: Arc<AtomicUsize>,
    packing_started_delivered: bool,
    preview_delivered: bool,
    control: Receiver<ControlEvent>,
    snapshots: Arc<SnapshotMailbox>,
    cancellation: CancellationToken,
}

#[cfg(test)]
impl LoadingState {
    fn for_test(
        generation: u64,
        folder_name: String,
        control: Receiver<ControlEvent>,
        snapshots: Arc<SnapshotMailbox>,
    ) -> Self {
        Self {
            generation,
            folder_name,
            index_progress: Arc::new(IndexProgress::new()),
            packing_completed: Arc::new(AtomicUsize::new(0)),
            packing_total: Arc::new(AtomicUsize::new(0)),
            packing_started_delivered: false,
            preview_delivered: false,
            control,
            snapshots,
            cancellation: CancellationToken::new(),
        }
    }
}

#[derive(Clone)]
struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
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
        self.start_worker(project_root, move |worker| {
            load_project_cancellable(&worker_root, &settings, &worker)
        })
    }

    fn start_worker<F>(&mut self, project_root: PathBuf, load: F) -> u64
    where
        F: FnOnce(WorkerContext) -> Result<(), WorkerError> + Send + 'static,
    {
        if let Some(loading) = &self.loading {
            loading.cancellation.cancel();
        }
        let generation = self.begin_generation();
        let folder_name = project_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| project_root.to_string_lossy().into_owned());
        let index_progress = Arc::new(IndexProgress::new());
        let packing_completed = Arc::new(AtomicUsize::new(0));
        let packing_total = Arc::new(AtomicUsize::new(0));
        let (control_sender, control) = mpsc::channel();
        let snapshots = Arc::new(SnapshotMailbox::default());
        let preview_sent = Arc::new(AtomicBool::new(false));
        let cancellation = CancellationToken::new();
        let worker = WorkerContext {
            generation,
            index_progress: Arc::clone(&index_progress),
            packing_completed: Arc::clone(&packing_completed),
            packing_total: Arc::clone(&packing_total),
            control: control_sender.clone(),
            snapshots: Arc::clone(&snapshots),
            cancellation: cancellation.clone(),
            preview_sent: Arc::clone(&preview_sent),
        };
        let panic_cancellation = cancellation.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load(worker)));
            let failure = match result {
                Ok(Ok(())) | Ok(Err(WorkerError::Cancelled)) => None,
                Ok(Err(WorkerError::Failed(message))) => Some(message),
                Err(panic) => Some(format!(
                    "project loading worker panicked: {}",
                    panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic")
                )),
            };
            if let Some(message) = failure {
                if !panic_cancellation.is_cancelled() {
                    let _ = control_sender.send(ControlEvent::Failed {
                        generation,
                        message,
                        preview_delivered: preview_sent.load(Ordering::Acquire),
                    });
                }
            }
        });
        self.loading = Some(LoadingState {
            generation,
            folder_name,
            index_progress,
            packing_completed,
            packing_total,
            packing_started_delivered: false,
            preview_delivered: false,
            control,
            snapshots,
            cancellation,
        });
        generation
    }

    pub fn poll(&mut self) -> LoaderPoll {
        let Some(loading) = self.loading.as_mut() else {
            return LoaderPoll::Idle;
        };

        loop {
            match loading.control.try_recv() {
                Ok(event) if event.generation() != loading.generation => continue,
                Ok(ControlEvent::PackingStarted { total, .. }) => {
                    loading.packing_started_delivered = true;
                    loading.packing_total.store(total, Ordering::Relaxed);
                    return LoaderPoll::Loading(LoadProgress {
                        folder_name: loading.folder_name.clone(),
                        phase: LoadPhase::Packing,
                        completed: 0,
                        total,
                    });
                }
                Ok(ControlEvent::Preview(preview)) => {
                    loading.preview_delivered = true;
                    return LoaderPoll::Preview(preview);
                }
                Ok(ControlEvent::Complete { generation, layout }) => {
                    loading.snapshots.take();
                    self.loading = None;
                    return LoaderPoll::Complete { generation, layout };
                }
                Ok(ControlEvent::Failed {
                    generation,
                    message,
                    preview_delivered,
                }) => {
                    loading.snapshots.take();
                    self.loading = None;
                    return LoaderPoll::Failed {
                        generation,
                        message,
                        preview_delivered,
                    };
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let generation = loading.generation;
                    let preview_delivered = loading.preview_delivered;
                    loading.snapshots.take();
                    self.loading = None;
                    return LoaderPoll::Failed {
                        generation,
                        message: "project loading worker disconnected before completion".into(),
                        preview_delivered,
                    };
                }
            }
        }

        if loading.preview_delivered {
            while let Some(snapshot) = loading.snapshots.take() {
                if snapshot.generation == loading.generation {
                    return LoaderPoll::Snapshot {
                        generation: snapshot.generation,
                        layout: snapshot.layout,
                    };
                }
            }
        }

        if loading.packing_started_delivered {
            LoaderPoll::Loading(LoadProgress {
                folder_name: loading.folder_name.clone(),
                phase: LoadPhase::Packing,
                completed: loading.packing_completed.load(Ordering::Relaxed),
                total: loading.packing_total.load(Ordering::Relaxed),
            })
        } else {
            index_load_progress(&loading.folder_name, &loading.index_progress)
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }
}

fn index_load_progress(folder_name: &str, progress: &IndexProgress) -> LoaderPoll {
    let files_total = progress.files_total.load(Ordering::Relaxed);
    let files_parsed = progress.files_parsed.load(Ordering::Relaxed);
    let (phase, completed, total) = match progress.phase.load(Ordering::Relaxed) {
        0 => (LoadPhase::Scanning, 0, 0),
        1 => (LoadPhase::Parsing, files_parsed, files_total),
        _ => (LoadPhase::BuildingTree, files_total, files_total),
    };
    LoaderPoll::Loading(LoadProgress {
        folder_name: folder_name.to_owned(),
        phase,
        completed,
        total,
    })
}

enum WorkerError {
    Cancelled,
    Failed(String),
}

struct WorkerContext {
    generation: u64,
    index_progress: Arc<IndexProgress>,
    packing_completed: Arc<AtomicUsize>,
    packing_total: Arc<AtomicUsize>,
    control: Sender<ControlEvent>,
    snapshots: Arc<SnapshotMailbox>,
    cancellation: CancellationToken,
    preview_sent: Arc<AtomicBool>,
}

impl Default for ProjectLoader {
    fn default() -> Self {
        Self::new()
    }
}

use outrider_index::scan::PreScanResult;

pub enum PreScanPoll {
    Idle,
    Scanning,
    Ready(Result<PreScanResult, String>),
}

pub struct PreScanner {
    result: Option<Receiver<Result<PreScanResult, String>>>,
}

impl PreScanner {
    pub fn new() -> Self {
        Self { result: None }
    }

    pub fn start(&mut self, repo_root: PathBuf) {
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let res = outrider_index::scan::pre_scan(&repo_root).map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
        });
        self.result = Some(rx);
    }

    pub fn poll(&mut self) -> PreScanPoll {
        let Some(rx) = &self.result else {
            return PreScanPoll::Idle;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.result = None;
                PreScanPoll::Ready(result)
            }
            Err(TryRecvError::Empty) => PreScanPoll::Scanning,
            Err(TryRecvError::Disconnected) => {
                self.result = None;
                PreScanPoll::Ready(Err("pre-scan worker disconnected".into()))
            }
        }
    }

    pub fn is_scanning(&self) -> bool {
        self.result.is_some()
    }

    pub fn cancel(&mut self) {
        self.result = None;
    }
}

impl Default for PreScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ProjectLoader {
    fn drop(&mut self) {
        if let Some(loading) = &self.loading {
            loading.cancellation.cancel();
        }
    }
}

fn load_project_cancellable(
    project_root: &Path,
    settings: &Settings,
    worker: &WorkerContext,
) -> Result<(), WorkerError> {
    worker.checkpoint()?;
    let project_namespace =
        ProjectTextureNamespace::prepare(project_root).map_err(|error| error.to_string());
    let disk_cache_bytes = settings.disk_cache_bytes(project_root);
    worker.checkpoint()?;
    let outcome = outrider_index::index_repo_with_progress_outcome_cancellable(
        project_root,
        &settings.filter_extensions,
        &settings.filter_folders,
        &settings.filter_files,
        &worker.index_progress,
        &|| worker.cancellation.is_cancelled(),
    )
    .map_err(|error| worker.failure_or_cancelled(format!("{error:#}")))?;
    worker.checkpoint()?;
    let tree = outcome.tree;
    let total = count_nodes(&tree.root);
    worker.packing_total.store(total, Ordering::Relaxed);
    worker
        .control
        .send(ControlEvent::PackingStarted {
            generation: worker.generation,
            total,
        })
        .map_err(|_| WorkerError::Cancelled)?;

    let mut preview_metadata = Some((
        project_root.to_path_buf(),
        outcome.warnings,
        outcome.source_fingerprints,
        disk_cache_bytes,
        project_namespace,
    ));
    let layout = pack_progressive(
        &tree,
        &crate::world::pack_config(settings.node_padding, settings.max_display_lines),
        30,
        || worker.cancellation.is_cancelled(),
        |progress| {
            worker
                .packing_completed
                .store(progress.completed, Ordering::Relaxed);
            if progress.completed == 0 {
                let Some(layout) = progress.snapshot else {
                    return;
                };
                let Some((
                    project_root,
                    warnings,
                    source_fingerprints,
                    disk_cache_bytes,
                    project_namespace,
                )) = preview_metadata.take()
                else {
                    return;
                };
                let preview = ProjectPreview {
                    generation: worker.generation,
                    project_root,
                    tree: tree.clone(),
                    layout,
                    warnings,
                    source_fingerprints,
                    disk_cache_bytes,
                    project_namespace,
                };
                if worker
                    .control
                    .send(ControlEvent::Preview(Box::new(preview)))
                    .is_ok()
                {
                    worker.preview_sent.store(true, Ordering::Release);
                }
            } else if progress.completed < progress.total {
                if let Some(layout) = progress.snapshot {
                    worker.snapshots.publish(LayoutSnapshot {
                        generation: worker.generation,
                        layout,
                    });
                }
            }
        },
    )
    .map_err(|_| WorkerError::Cancelled)?;
    worker.checkpoint()?;
    worker
        .control
        .send(ControlEvent::Complete {
            generation: worker.generation,
            layout,
        })
        .map_err(|_| WorkerError::Cancelled)
}

impl WorkerContext {
    fn checkpoint(&self) -> Result<(), WorkerError> {
        if self.cancellation.is_cancelled() {
            Err(WorkerError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn failure_or_cancelled(&self, message: String) -> WorkerError {
        if self.cancellation.is_cancelled() {
            WorkerError::Cancelled
        } else {
            WorkerError::Failed(message)
        }
    }
}

fn count_nodes(node: &outrider_index::SymbolNode) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::{
        ControlEvent, LayoutSnapshot, LoadPhase, LoaderPoll, LoadingState, ProjectLoader,
        SnapshotMailbox, WorkerError,
    };
    use crate::settings::Settings;

    #[test]
    fn loader_has_only_the_staged_project_pipeline() {
        let source = include_str!("project_loader.rs");

        assert!(
            !source.contains("\nfn load_project(\n"),
            "a duplicate synchronous project-loading pipeline remains"
        );
    }

    #[test]
    fn newer_load_supersedes_older_result() {
        let mut loader = ProjectLoader::new();
        let first = loader.begin_generation();
        let second = loader.begin_generation();

        assert!(!loader.accepts(first));
        assert!(loader.accepts(second));
    }

    #[test]
    fn successful_indexed_preview_retains_project_metadata_and_layout() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut loader = ProjectLoader::new();

        let generation = loader.start(repo.path().to_path_buf(), Settings::default());
        let preview = await_preview(&mut loader);

        assert_eq!(preview.generation, generation);
        assert_eq!(preview.project_root, repo.path());
        assert!(!preview.tree.root.children.is_empty());
        assert!(!preview.layout.rects.is_empty());
        assert!(preview.warnings.is_empty());
        assert!(preview.source_fingerprints.contains_key("main.rs"));
        assert_eq!(
            preview.disk_cache_bytes,
            crate::settings::DEFAULT_DISK_CACHE_BYTES
        );
        assert!(preview.project_namespace.is_ok());
    }

    #[test]
    fn packing_start_preview_and_completion_are_observed_in_order() {
        let mut loader = ProjectLoader::new();
        let generation = loader.begin_generation();
        let (control_tx, control_rx) = std::sync::mpsc::channel();
        let snapshots = std::sync::Arc::new(SnapshotMailbox::default());
        let preview = test_preview(generation, 1.0);
        let final_layout = preview.layout.clone();
        control_tx
            .send(ControlEvent::PackingStarted {
                generation,
                total: 1,
            })
            .unwrap();
        control_tx
            .send(ControlEvent::Preview(Box::new(preview)))
            .unwrap();
        control_tx
            .send(ControlEvent::Complete {
                generation,
                layout: final_layout,
            })
            .unwrap();
        loader.loading = Some(test_loading_state(
            generation,
            control_rx,
            std::sync::Arc::clone(&snapshots),
        ));

        let LoaderPoll::Loading(packing_progress) = loader.poll() else {
            panic!("packing start was not observable");
        };
        assert_eq!(packing_progress.phase, LoadPhase::Packing);
        assert_eq!((packing_progress.completed, packing_progress.total), (0, 1));
        assert!(matches!(loader.poll(), LoaderPoll::Preview(_)));
        assert!(matches!(loader.poll(), LoaderPoll::Complete { .. }));
    }

    #[test]
    fn snapshot_mailbox_keeps_only_the_newest_layout() {
        let mailbox = SnapshotMailbox::default();
        mailbox.publish(LayoutSnapshot {
            generation: 7,
            layout: test_layout(1.0),
        });
        mailbox.publish(LayoutSnapshot {
            generation: 7,
            layout: test_layout(2.0),
        });
        mailbox.publish(LayoutSnapshot {
            generation: 7,
            layout: test_layout(3.0),
        });

        let snapshot = mailbox.take().expect("newest snapshot missing");
        assert_eq!(snapshot.layout, test_layout(3.0));
        assert!(mailbox.take().is_none());
    }

    #[test]
    fn terminal_event_wins_over_and_clears_an_occupied_snapshot_mailbox() {
        let mut loader = ProjectLoader::new();
        let generation = loader.begin_generation();
        let (control_tx, control_rx) = std::sync::mpsc::channel();
        let snapshots = std::sync::Arc::new(SnapshotMailbox::default());
        snapshots.publish(LayoutSnapshot {
            generation,
            layout: test_layout(2.0),
        });
        control_tx
            .send(ControlEvent::Complete {
                generation,
                layout: test_layout(3.0),
            })
            .unwrap();
        loader.loading = Some(test_loading_state(
            generation,
            control_rx,
            std::sync::Arc::clone(&snapshots),
        ));

        let LoaderPoll::Complete { layout, .. } = loader.poll() else {
            panic!("completion did not take priority over a snapshot");
        };
        assert_eq!(layout, test_layout(3.0));
        assert!(snapshots.take().is_none());
        assert!(matches!(loader.poll(), LoaderPoll::Idle));
    }

    fn test_loading_state(
        generation: u64,
        control: std::sync::mpsc::Receiver<ControlEvent>,
        snapshots: std::sync::Arc<SnapshotMailbox>,
    ) -> LoadingState {
        LoadingState::for_test(generation, "project".into(), control, snapshots)
    }

    fn await_preview(loader: &mut ProjectLoader) -> super::ProjectPreview {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(std::time::Instant::now() < deadline, "load timed out");
            match loader.poll() {
                LoaderPoll::Preview(preview) => return *preview,
                LoaderPoll::Loading(_) => std::thread::yield_now(),
                LoaderPoll::Failed { message, .. } => panic!("load failed: {message}"),
                LoaderPoll::Idle | LoaderPoll::Snapshot { .. } | LoaderPoll::Complete { .. } => {}
            }
        }
    }

    fn test_preview(generation: u64, width: f64) -> super::ProjectPreview {
        super::ProjectPreview {
            generation,
            project_root: std::path::PathBuf::from("fixture-project"),
            tree: test_tree(),
            layout: test_layout(width),
            warnings: Vec::new(),
            source_fingerprints: std::collections::BTreeMap::new(),
            disk_cache_bytes: 0,
            project_namespace: Err("fixture has no texture namespace".into()),
        }
    }

    fn test_tree() -> outrider_index::SymbolTree {
        use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

        SymbolTree {
            root: SymbolNode {
                id: SymbolId {
                    kind: SymbolKind::Folder,
                    qualified_path: String::new(),
                    ordinal: 0,
                },
                name: "fixture".into(),
                byte_range: None,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                children: Vec::new(),
            },
            repo_root: std::path::PathBuf::from("fixture-project"),
        }
    }

    fn test_layout(width: f64) -> outrider_layout::PackLayout {
        use outrider_index::{SymbolId, SymbolKind};
        use outrider_layout::Rect;
        use std::collections::BTreeMap;

        let id = SymbolId {
            kind: SymbolKind::Folder,
            qualified_path: String::new(),
            ordinal: 0,
        };
        outrider_layout::PackLayout {
            rects: BTreeMap::from([(
                id,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: width,
                    h: 1.0,
                },
            )]),
        }
    }

    #[test]
    fn snapshot_publication_drops_on_contention_without_replacing_pending_state() {
        let mailbox = SnapshotMailbox::default();
        let guard = mailbox.0.lock().unwrap();
        mailbox.publish(LayoutSnapshot {
            generation: 1,
            layout: test_layout(2.0),
        });
        drop(guard);

        assert!(mailbox.take().is_none());
    }

    #[test]
    fn stale_generation_rejects_every_control_event_and_snapshot() {
        let mut loader = ProjectLoader::new();
        let stale = loader.begin_generation();
        let current = loader.begin_generation();
        let (control_tx, control_rx) = std::sync::mpsc::channel();
        let snapshots = std::sync::Arc::new(SnapshotMailbox::default());
        control_tx
            .send(ControlEvent::PackingStarted {
                generation: stale,
                total: 1,
            })
            .unwrap();
        control_tx
            .send(ControlEvent::Preview(Box::new(test_preview(stale, 1.0))))
            .unwrap();
        control_tx
            .send(ControlEvent::Complete {
                generation: stale,
                layout: test_layout(2.0),
            })
            .unwrap();
        control_tx
            .send(ControlEvent::Failed {
                generation: stale,
                message: "stale".into(),
                preview_delivered: true,
            })
            .unwrap();
        snapshots.publish(LayoutSnapshot {
            generation: stale,
            layout: test_layout(3.0),
        });
        let mut loading = test_loading_state(current, control_rx, snapshots);
        loading.preview_delivered = true;
        loader.loading = Some(loading);

        let LoaderPoll::Loading(progress) = loader.poll() else {
            panic!("a stale event escaped generation fencing");
        };
        assert_eq!(progress.phase, LoadPhase::Scanning);
        assert!(loader.is_loading());
        drop(control_tx);
    }

    #[test]
    fn superseding_a_worker_cancels_progressive_packing() {
        let tree = test_tree();
        let (draft_tx, draft_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let mut loader = ProjectLoader::new();

        loader.start_worker(std::path::PathBuf::from("old-project"), move |worker| {
            let mut release_rx = Some(release_rx);
            let result = outrider_layout::pack_progressive(
                &tree,
                &crate::world::pack_config(4.0, None),
                30,
                || worker.cancellation.is_cancelled(),
                |progress| {
                    if progress.completed == 0 {
                        draft_tx.send(()).unwrap();
                        release_rx.take().unwrap().recv().unwrap();
                    }
                },
            );
            exit_tx.send(result.is_err()).unwrap();
            result.map(|_| ()).map_err(|_| WorkerError::Cancelled)
        });
        draft_rx.recv().unwrap();
        loader.start_worker(std::path::PathBuf::from("new-project"), |_| {
            Err(WorkerError::Failed("new load stopped for test".into()))
        });
        release_tx.send(()).unwrap();

        assert!(exit_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("superseded packer did not exit"));
    }

    #[test]
    fn worker_panic_becomes_a_failed_terminal_event() {
        let mut loader = ProjectLoader::new();
        loader.start_worker(std::path::PathBuf::from("panic-project"), |_| {
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
                LoaderPoll::Failed { message, .. } => {
                    assert!(message.contains("panicked"));
                    break;
                }
                _ => panic!("panicked worker returned a non-failure event"),
            }
        }
    }

    #[test]
    fn real_worker_exposes_packing_zero_preview_then_exact_completion() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut loader = ProjectLoader::new();
        let generation = loader.start(repo.path().to_path_buf(), Settings::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut saw_packing_start = false;
        let mut preview_layout = None;

        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "background load timed out"
            );
            match loader.poll() {
                LoaderPoll::Loading(progress) if progress.phase == LoadPhase::Packing => {
                    if !saw_packing_start {
                        assert_eq!(progress.completed, 0);
                        assert!(progress.total > 0);
                        saw_packing_start = true;
                    }
                }
                LoaderPoll::Loading(_) => {}
                LoaderPoll::Preview(preview) => {
                    assert!(saw_packing_start, "preview preceded packing start");
                    assert_eq!(preview.generation, generation);
                    preview_layout = Some(preview.layout.clone());
                }
                LoaderPoll::Snapshot { .. } => {
                    assert!(preview_layout.is_some(), "snapshot preceded preview");
                }
                LoaderPoll::Complete {
                    generation: completed_generation,
                    layout,
                } => {
                    assert_eq!(completed_generation, generation);
                    assert!(preview_layout.is_some(), "completion preceded preview");
                    assert!(!layout.rects.is_empty());
                    break;
                }
                LoaderPoll::Failed { message, .. } => panic!("load failed: {message}"),
                LoaderPoll::Idle => panic!("loader became idle before completion"),
            }
            std::thread::yield_now();
        }
    }

    #[test]
    fn disconnected_worker_becomes_a_failed_terminal_event() {
        let mut loader = ProjectLoader::new();
        let generation = loader.begin_generation();
        let (sender, control) = std::sync::mpsc::channel();
        drop(sender);
        loader.loading = Some(test_loading_state(
            generation,
            control,
            std::sync::Arc::new(SnapshotMailbox::default()),
        ));

        let LoaderPoll::Failed {
            generation: failed_generation,
            message,
            preview_delivered,
        } = loader.poll()
        else {
            panic!("disconnect was not surfaced as failure");
        };
        assert_eq!(failed_generation, generation);
        assert!(message.contains("disconnected"));
        assert!(!preview_delivered);
    }

    #[test]
    fn background_load_prepares_same_namespace_for_canonical_aliases() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join("alias-segment")).unwrap();
        std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
        let alias = repo.path().join("alias-segment").join("..");

        let mut canonical_loader = ProjectLoader::new();
        canonical_loader.start(repo.path().to_path_buf(), Settings::default());
        let canonical = await_preview(&mut canonical_loader);
        let mut alias_loader = ProjectLoader::new();
        alias_loader.start(alias, Settings::default());
        let aliased = await_preview(&mut alias_loader);

        assert!(canonical.project_namespace == aliased.project_namespace);
    }
}
