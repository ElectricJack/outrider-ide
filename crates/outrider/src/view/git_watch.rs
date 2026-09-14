//! Lightweight git repository change watcher using `GitProbe` (no git processes).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use outrider_view::git::{GitProbe, GitStamp};

pub struct GitWatcher {
    rx: mpsc::Receiver<GitStamp>,
    stop: Arc<AtomicBool>,
}

impl GitWatcher {
    pub fn new(repo_root: &Path, wake: Arc<super::rpc::Wake>) -> Option<GitWatcher> {
        let mut probe = GitProbe::open(repo_root)?;
        // Consume the initial stamp so the first real change triggers.
        probe.poll();

        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        std::thread::Builder::new()
            .name("git-watch".into())
            .spawn(move || {
                loop {
                    if thread_stop.load(Ordering::Acquire) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    if thread_stop.load(Ordering::Acquire) {
                        break;
                    }
                    if let Some(stamp) = probe.poll() {
                        if tx.send(stamp).is_err() {
                            return;
                        }
                        wake.raise();
                    }
                }
            })
            .ok();

        Some(GitWatcher { rx, stop })
    }

    /// Drain pending change events (non-blocking). Returns the latest stamp
    /// if any changes occurred since last drain.
    pub fn drain(&self) -> Option<GitStamp> {
        let mut last = None;
        for stamp in self.rx.try_iter() {
            last = Some(stamp);
        }
        last
    }
}

impl Drop for GitWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
