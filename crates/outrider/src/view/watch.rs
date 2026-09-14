//! File watcher for `.outrider/views/*.json` view spec files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::SystemTime;

/// An event from the view watcher.
pub enum WatchEvent {
    Changed(PathBuf),
    Removed(PathBuf),
}

/// Polls `.outrider/views/` for JSON files and sends change/removal events.
pub struct ViewWatcher {
    rx: mpsc::Receiver<WatchEvent>,
    stop: Arc<AtomicBool>,
}

impl ViewWatcher {
    pub fn new(project_root: &Path, wake: Arc<super::rpc::Wake>) -> ViewWatcher {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let watch_dir = project_root.join(".outrider").join("views");
        let thread_stop = Arc::clone(&stop);

        std::thread::Builder::new()
            .name("view-watch".into())
            .spawn(move || {
                let mut prev: BTreeMap<PathBuf, SystemTime> = BTreeMap::new();
                loop {
                    if thread_stop.load(Ordering::Acquire) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    if thread_stop.load(Ordering::Acquire) {
                        break;
                    }

                    let next = scan_dir(&watch_dir);
                    let events = diff_mtimes(&prev, &next);
                    if !events.is_empty() {
                        for event in events {
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                        wake.raise();
                    }
                    prev = next;
                }
            })
            .ok();

        ViewWatcher { rx, stop }
    }

    /// Drain all pending events (non-blocking).
    pub fn drain(&self) -> Vec<WatchEvent> {
        self.rx.try_iter().collect()
    }
}

impl Drop for ViewWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Tracking state for the most recently applied view file.
pub struct WatchState {
    mtimes: BTreeMap<PathBuf, SystemTime>,
    retry: Option<PathBuf>,
}

impl WatchState {
    pub fn new() -> Self {
        WatchState {
            mtimes: BTreeMap::new(),
            retry: None,
        }
    }

    /// Record an event (update or remove the tracked mtime).
    pub fn record(&mut self, event: &WatchEvent) {
        match event {
            WatchEvent::Changed(path) => {
                let mtime = std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::now());
                self.mtimes.insert(path.clone(), mtime);
            }
            WatchEvent::Removed(path) => {
                self.mtimes.remove(path);
            }
        }
    }

    /// Return the path with the newest mtime, or the lexicographically last
    /// path on tie.
    pub fn newest(&self) -> Option<&PathBuf> {
        newest(&self.mtimes)
    }

    /// Returns true the first time called for a given path; false on repeated
    /// calls for the same path. Resets when a different path is retried.
    pub fn retry_once(&mut self, path: &PathBuf) -> bool {
        if self.retry.as_ref() == Some(path) {
            false
        } else {
            self.retry = Some(path.clone());
            true
        }
    }
}

fn scan_dir(dir: &Path) -> BTreeMap<PathBuf, SystemTime> {
    let mut result = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return result,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    result.insert(path, mtime);
                }
            }
        }
    }
    result
}

/// Compare two mtime snapshots and produce change/removal events.
pub(crate) fn diff_mtimes(
    prev: &BTreeMap<PathBuf, SystemTime>,
    next: &BTreeMap<PathBuf, SystemTime>,
) -> Vec<WatchEvent> {
    let mut events = Vec::new();
    // New or modified files.
    for (path, &mtime) in next {
        match prev.get(path) {
            None => events.push(WatchEvent::Changed(path.clone())),
            Some(&old_mtime) if old_mtime != mtime => {
                events.push(WatchEvent::Changed(path.clone()))
            }
            _ => {}
        }
    }
    // Removed files.
    for path in prev.keys() {
        if !next.contains_key(path) {
            events.push(WatchEvent::Removed(path.clone()));
        }
    }
    events
}

/// Return the path with the newest mtime. On tie, pick the lexicographically
/// last path.
pub(crate) fn newest(map: &BTreeMap<PathBuf, SystemTime>) -> Option<&PathBuf> {
    map.iter()
        .max_by(|(path_a, time_a), (path_b, time_b)| {
            time_a.cmp(time_b).then_with(|| path_a.cmp(path_b))
        })
        .map(|(path, _)| path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn diff_mtimes_new_file() {
        let prev = BTreeMap::new();
        let mut next = BTreeMap::new();
        next.insert(PathBuf::from("a.json"), t(100));

        let events = diff_mtimes(&prev, &next);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], WatchEvent::Changed(p) if p == &PathBuf::from("a.json")));
    }

    #[test]
    fn diff_mtimes_modified_file() {
        let mut prev = BTreeMap::new();
        prev.insert(PathBuf::from("a.json"), t(100));
        let mut next = BTreeMap::new();
        next.insert(PathBuf::from("a.json"), t(200));

        let events = diff_mtimes(&prev, &next);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], WatchEvent::Changed(p) if p == &PathBuf::from("a.json")));
    }

    #[test]
    fn diff_mtimes_removed_file() {
        let mut prev = BTreeMap::new();
        prev.insert(PathBuf::from("a.json"), t(100));
        let next = BTreeMap::new();

        let events = diff_mtimes(&prev, &next);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], WatchEvent::Removed(p) if p == &PathBuf::from("a.json")));
    }

    #[test]
    fn diff_mtimes_unchanged() {
        let mut prev = BTreeMap::new();
        prev.insert(PathBuf::from("a.json"), t(100));
        let next = prev.clone();

        let events = diff_mtimes(&prev, &next);
        assert!(events.is_empty());
    }

    #[test]
    fn newest_picks_max_mtime() {
        let mut map = BTreeMap::new();
        map.insert(PathBuf::from("old.json"), t(100));
        map.insert(PathBuf::from("new.json"), t(200));

        assert_eq!(newest(&map), Some(&PathBuf::from("new.json")));
    }

    #[test]
    fn newest_tie_picks_lexicographic_last() {
        let mut map = BTreeMap::new();
        map.insert(PathBuf::from("a.json"), t(100));
        map.insert(PathBuf::from("b.json"), t(100));

        assert_eq!(newest(&map), Some(&PathBuf::from("b.json")));
    }

    #[test]
    fn retry_once_returns_true_first_false_second() {
        let mut state = WatchState::new();
        let path = PathBuf::from("test.json");
        assert!(state.retry_once(&path));
        assert!(!state.retry_once(&path));
    }

    #[test]
    fn retry_once_resets_for_different_path() {
        let mut state = WatchState::new();
        let path1 = PathBuf::from("a.json");
        let path2 = PathBuf::from("b.json");
        assert!(state.retry_once(&path1));
        assert!(state.retry_once(&path2));
    }
}
