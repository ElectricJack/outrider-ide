//! Source-file materialization layer: loads files from disk into rope-backed
//! `FileBuffer` objects with syntax highlighting, attaches per-symbol anchors
//! for stable line lookup, and caches up to `MAX_BUFFERS` entries LRU-style.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

use outrider_index::buffer::{AnchorId, FileBuffer};
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

/// Maximum simultaneously-cached materialized files; LRU-evicts beyond this.
pub const MAX_BUFFERS: usize = 64;

/// A materialized file: rope-backed buffer plus one anchor per symbol,
/// created at materialization (spec §3.3).
pub struct Materialized {
    pub buffer: FileBuffer,
    anchors: BTreeMap<SymbolId, AnchorId>,
}

/// Stable line-index access for a loaded file's symbols.
impl Materialized {
    /// Rope line index of the symbol's start, via its anchor — the Full
    /// render never reads raw `byte_range` offsets.
    pub fn symbol_start_line(&self, id: &SymbolId) -> Option<usize> {
        let a = self.anchors.get(id)?;
        Some(self.buffer.byte_to_line(self.buffer.resolve_anchor(*a)))
    }
}

/// Background materialization: the request channel to the loader thread
/// and the results it sends back.
struct Loader {
    requests: mpsc::Sender<(String, Vec<(SymbolId, usize)>)>,
    results: mpsc::Receiver<(String, Option<Materialized>)>,
    inflight: HashSet<String>,
}

/// LRU cache of materialized buffers, keyed by relative file path.
/// Most-recently-used entry is last (spec §4.1).
///
/// With background loading (`with_background_loading`) a miss in `get`
/// queues the read + parse on a worker thread and returns `None`; the
/// caller paints its no-buffer representation (line bars) and the entry
/// arrives through `poll` a frame or so later. Parsing a large C++ file
/// takes ~100ms, which is why it is never done on the frame thread.
pub struct BufferManager {
    repo_root: PathBuf,
    entries: Vec<(String, Materialized)>,
    loader: Option<Loader>,
    /// Files that failed to read or parse: `get` answers `None` at once
    /// instead of retrying every frame.
    failed: HashSet<String>,
}

fn materialize(
    repo_root: &std::path::Path,
    rel_path: &str,
    symbols: &[(SymbolId, usize)],
) -> Option<Materialized> {
    let full = repo_root.join(rel_path);
    let text = match std::fs::read_to_string(&full) {
        Ok(t) => t,
        Err(e) => {
            crate::frame_profile::debug_log(|| format!("buffer read failed {}: {e}", full.display()));
            return None;
        }
    };
    let mut buffer = match FileBuffer::new(text, std::path::Path::new(rel_path)) {
        Ok(b) => b,
        Err(e) => {
            crate::frame_profile::debug_log(|| format!("buffer parse failed {rel_path}: {e:#}"));
            return None;
        }
    };
    let anchors = symbols
        .iter()
        .map(|(id, start)| (id.clone(), buffer.create_anchor(*start)))
        .collect();
    Some(Materialized { buffer, anchors })
}

/// Disk I/O, anchor creation, LRU management, and path helpers.
impl BufferManager {
    /// Create a manager rooted at `repo_root`; no files are read yet.
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            entries: Vec::new(),
            loader: None,
            failed: HashSet::new(),
        }
    }

    /// A manager whose misses materialize on a worker thread (see the type
    /// docs). Falls back to synchronous loading if the thread cannot start.
    pub fn with_background_loading(repo_root: PathBuf) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<(String, Vec<(SymbolId, usize)>)>();
        let (res_tx, res_rx) = mpsc::channel();
        let root = repo_root.clone();
        let spawned = std::thread::Builder::new()
            .name("outrider-buffers".into())
            .spawn(move || {
                while let Ok((rel, symbols)) = req_rx.recv() {
                    let m = materialize(&root, &rel, &symbols);
                    if res_tx.send((rel, m)).is_err() {
                        break;
                    }
                }
            });
        let loader = spawned.ok().map(|_| Loader {
            requests: req_tx,
            results: res_rx,
            inflight: HashSet::new(),
        });
        Self {
            repo_root,
            entries: Vec::new(),
            loader,
            failed: HashSet::new(),
        }
    }

    /// Whether `rel_path` is being materialized right now (background
    /// loading only): a `None` from `get` will turn into a buffer later.
    pub fn is_pending(&self, rel_path: &str) -> bool {
        self.loader
            .as_ref()
            .is_some_and(|l| l.inflight.contains(rel_path))
    }

    /// Any materialization outstanding on the worker.
    pub fn has_pending(&self) -> bool {
        self.loader.as_ref().is_some_and(|l| !l.inflight.is_empty())
    }

    /// Collect finished background loads into the cache. Returns how many
    /// arrived (callers repaint when > 0).
    pub fn poll(&mut self) -> usize {
        let Some(loader) = self.loader.as_mut() else {
            return 0;
        };
        let mut arrived = 0;
        while let Ok((rel, m)) = loader.results.try_recv() {
            loader.inflight.remove(&rel);
            match m {
                Some(m) => {
                    self.entries.retain(|(p, _)| p != &rel);
                    self.entries.push((rel, m));
                    if self.entries.len() > MAX_BUFFERS {
                        self.entries.remove(0);
                    }
                }
                None => {
                    self.failed.insert(rel);
                }
            }
            arrived += 1;
        }
        arrived
    }

    /// The file-path portion of a qualified_path: everything before the
    /// first `::` (the whole path when there is none, as on File nodes).
    pub fn file_path_of(qualified_path: &str) -> &str {
        let s = qualified_path.split("::").next().unwrap_or(qualified_path);
        s.split('#').next().unwrap_or(s)
    }

    /// Materialize from disk on first access, creating one anchor per
    /// symbol; refresh recency on hits (no disk re-read); LRU-evict beyond
    /// MAX_BUFFERS. None if the file cannot be read or parsed — the box
    /// falls back to Detail content.
    pub fn get(&mut self, rel_path: &str, symbols: &[(SymbolId, usize)]) -> Option<&Materialized> {
        if let Some(i) = self.entries.iter().position(|(p, _)| p == rel_path) {
            let e = self.entries.remove(i);
            self.entries.push(e);
            return self.entries.last().map(|(_, m)| m);
        }
        if self.failed.contains(rel_path) {
            return None;
        }
        if let Some(loader) = self.loader.as_mut() {
            if !loader.inflight.contains(rel_path) {
                let sent = loader
                    .requests
                    .send((rel_path.to_string(), symbols.to_vec()))
                    .is_ok();
                if sent {
                    loader.inflight.insert(rel_path.to_string());
                    return None;
                }
                // Worker gone: materialize inline below.
                self.loader = None;
            } else {
                return None;
            }
        }
        let Some(m) = materialize(&self.repo_root, rel_path, symbols) else {
            self.failed.insert(rel_path.to_string());
            return None;
        };
        self.entries.push((rel_path.to_string(), m));
        if self.entries.len() > MAX_BUFFERS {
            self.entries.remove(0);
        }
        self.entries.last().map(|(_, m)| m)
    }
}

/// rel file path → (id, byte_range.start) of every item inside that file
/// — or, for a childless file, the file node itself at byte 0. Built once
/// at view construction; `get` uses it to create anchors at
/// materialization.
pub fn collect_file_symbols(tree: &SymbolTree) -> BTreeMap<String, Vec<(SymbolId, usize)>> {
    fn items(node: &SymbolNode, out: &mut Vec<(SymbolId, usize)>) {
        for c in &node.children {
            if let Some(r) = &c.byte_range {
                out.push((c.id.clone(), r.start));
            }
            items(c, out);
        }
    }
    fn walk(node: &SymbolNode, out: &mut BTreeMap<String, Vec<(SymbolId, usize)>>) {
        if node.id.kind == SymbolKind::File {
            let mut v = Vec::new();
            if node.children.is_empty() {
                // Text page: anchor the file itself so its window starts
                // at rope line 0 (spec §4).
                if let Some(r) = &node.byte_range {
                    v.push((node.id.clone(), r.start));
                }
            } else {
                items(node, &mut v);
            }
            out.insert(node.id.qualified_path.clone(), v);
        } else {
            for c in &node.children {
                walk(c, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&tree.root, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::{collect_file_symbols, BufferManager, MAX_BUFFERS};
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    fn write_file(dir: &std::path::Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn fn_id(qual: &str) -> SymbolId {
        SymbolId {
            kind: SymbolKind::Item { label: "fn".into() },
            qualified_path: qual.into(),
            ordinal: 0,
        }
    }

    #[test]
    fn file_path_of_splits_at_first_colons() {
        assert_eq!(
            BufferManager::file_path_of("src/lib.rs::Point::norm"),
            "src/lib.rs"
        );
        assert_eq!(BufferManager::file_path_of("src/lib.rs"), "src/lib.rs");
        assert_eq!(BufferManager::file_path_of("BIG.md#0"), "BIG.md");
        assert_eq!(BufferManager::file_path_of("dir/f.rs#2"), "dir/f.rs");
    }

    #[test]
    fn get_materializes_creates_anchors_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.rs", "fn one() {}\nfn two() {}\n");
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let syms = vec![(fn_id("a.rs::one"), 0), (fn_id("a.rs::two"), 12)];
        let m = mgr.get("a.rs", &syms).unwrap();
        assert_eq!(m.buffer.len_lines(), 2);
        assert_eq!(m.symbol_start_line(&fn_id("a.rs::one")), Some(0));
        assert_eq!(m.symbol_start_line(&fn_id("a.rs::two")), Some(1));
        assert_eq!(m.symbol_start_line(&fn_id("a.rs::absent")), None);
        // cache hit: delete from disk; a second get must NOT re-read
        std::fs::remove_file(dir.path().join("a.rs")).unwrap();
        assert!(mgr.get("a.rs", &[]).is_some());
    }

    #[test]
    fn background_loading_answers_none_then_polls_the_buffer_in() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.rs", "fn one() {}\nfn two() {}\n");
        let mut mgr = BufferManager::with_background_loading(dir.path().to_path_buf());
        let syms = vec![(fn_id("a.rs::one"), 0), (fn_id("a.rs::two"), 12)];
        assert!(mgr.get("a.rs", &syms).is_none());
        assert!(mgr.is_pending("a.rs"));
        assert!(mgr.has_pending());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while mgr.poll() == 0 {
            assert!(std::time::Instant::now() < deadline, "loader never answered");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!mgr.is_pending("a.rs"));
        let m = mgr.get("a.rs", &syms).unwrap();
        assert_eq!(m.symbol_start_line(&fn_id("a.rs::two")), Some(1));
        // A missing file is reported once and then answered without retry.
        assert!(mgr.get("nope.rs", &[]).is_none());
        while mgr.has_pending() {
            mgr.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(mgr.get("nope.rs", &[]).is_none());
        assert!(!mgr.is_pending("nope.rs"));
    }

    #[test]
    fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        assert!(mgr.get("nope.rs", &[]).is_none());
    }

    #[test]
    fn extensionless_makefile_materializes_with_highlights() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "Makefile", "# build\nall:\n\t@echo done\n");
        let mut mgr = BufferManager::new(dir.path().to_path_buf());

        let materialized = mgr.get("Makefile", &[]).unwrap();
        assert!(materialized
            .buffer
            .line(0)
            .unwrap()
            .1
            .iter()
            .any(|span| { span.kind == outrider_index::buffer::HighlightKind::Comment }));
        assert!(materialized
            .buffer
            .line(1)
            .unwrap()
            .1
            .iter()
            .any(|span| { span.kind == outrider_index::buffer::HighlightKind::Function }));
    }

    #[test]
    fn lru_evicts_least_recent_beyond_cap() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..=MAX_BUFFERS {
            write_file(dir.path(), &format!("f{i}.rs"), "fn x() {}\n");
        }
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        for i in 0..MAX_BUFFERS {
            mgr.get(&format!("f{i}.rs"), &[]).unwrap();
        }
        // touch f0 (refresh recency), then insert one past the cap
        mgr.get("f0.rs", &[]).unwrap();
        mgr.get(&format!("f{MAX_BUFFERS}.rs"), &[]).unwrap();
        // f1 is now least-recent and was evicted: with the file gone, a
        // fresh get must fail (re-materialization from disk)
        std::fs::remove_file(dir.path().join("f1.rs")).unwrap();
        assert!(mgr.get("f1.rs", &[]).is_none());
        // f0 survived the eviction (recency was refreshed)
        std::fs::remove_file(dir.path().join("f0.rs")).unwrap();
        assert!(mgr.get("f0.rs", &[]).is_some());
    }

    #[test]
    fn collect_file_symbols_maps_items_by_file() {
        fn node(
            kind: SymbolKind,
            qual: &str,
            byte_range: Option<std::ops::Range<usize>>,
            children: Vec<SymbolNode>,
        ) -> SymbolNode {
            SymbolNode {
                id: SymbolId {
                    kind,
                    qualified_path: qual.into(),
                    ordinal: 0,
                },
                name: qual.rsplit("::").next().unwrap_or(qual).to_string(),
                byte_range,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                diff_status: None,
                diff_hunks: Vec::new(),
                deleted_lines: Vec::new(),
                visibility: None,
                children,
            }
        }
        let tree = SymbolTree {
            root: node(
                SymbolKind::Folder,
                "",
                None,
                vec![node(
                    SymbolKind::File,
                    "a.rs",
                    Some(0..40),
                    vec![node(
                        SymbolKind::Item {
                            label: "impl".into(),
                        },
                        "a.rs::T",
                        Some(0..30),
                        vec![node(
                            SymbolKind::Item { label: "fn".into() },
                            "a.rs::T::m",
                            Some(10..25),
                            vec![],
                        )],
                    )],
                )],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        };
        let map = collect_file_symbols(&tree);
        assert_eq!(map.len(), 1);
        let got: Vec<(&str, SymbolKind, usize)> = map
            .get("a.rs")
            .unwrap()
            .iter()
            .map(|(id, s)| (id.qualified_path.as_str(), id.kind.clone(), *s))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "a.rs::T",
                    SymbolKind::Item {
                        label: "impl".into()
                    },
                    0
                ),
                ("a.rs::T::m", SymbolKind::Item { label: "fn".into() }, 10)
            ]
        );
    }

    #[test]
    fn collect_file_symbols_anchors_childless_files_at_zero() {
        fn node(
            kind: SymbolKind,
            qual: &str,
            byte_range: Option<std::ops::Range<usize>>,
            children: Vec<SymbolNode>,
        ) -> SymbolNode {
            SymbolNode {
                id: SymbolId {
                    kind,
                    qualified_path: qual.into(),
                    ordinal: 0,
                },
                name: qual.rsplit("::").next().unwrap_or(qual).to_string(),
                byte_range,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                diff_status: None,
                diff_hunks: Vec::new(),
                deleted_lines: Vec::new(),
                visibility: None,
                children,
            }
        }
        let tree = SymbolTree {
            root: node(
                SymbolKind::Folder,
                "",
                None,
                vec![
                    node(SymbolKind::File, "README.md", Some(0..120), vec![]),
                    node(
                        SymbolKind::File,
                        "a.rs",
                        Some(0..40),
                        vec![node(
                            SymbolKind::Item { label: "fn".into() },
                            "a.rs::f",
                            Some(5..30),
                            vec![],
                        )],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        };
        let map = collect_file_symbols(&tree);
        // childless file: its own id at byte 0
        let readme = map.get("README.md").unwrap();
        assert_eq!(readme.len(), 1);
        assert_eq!(readme[0].0.kind, SymbolKind::File);
        assert_eq!(readme[0].0.qualified_path, "README.md");
        assert_eq!(readme[0].1, 0);
        // file with children: items only, own id absent
        let a = map.get("a.rs").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].0.qualified_path, "a.rs::f");
    }

    #[test]
    fn collect_file_symbols_anchors_each_chunk_at_its_start() {
        fn node(
            kind: SymbolKind,
            qual: &str,
            byte_range: Option<std::ops::Range<usize>>,
            children: Vec<SymbolNode>,
        ) -> SymbolNode {
            SymbolNode {
                id: SymbolId {
                    kind,
                    qualified_path: qual.into(),
                    ordinal: 0,
                },
                name: qual.rsplit(['#', ':']).next().unwrap_or(qual).to_string(),
                byte_range,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                diff_status: None,
                diff_hunks: Vec::new(),
                deleted_lines: Vec::new(),
                visibility: None,
                children,
            }
        }
        let tree = SymbolTree {
            root: node(
                SymbolKind::Folder,
                "",
                None,
                vec![node(
                    SymbolKind::File,
                    "BIG.md",
                    Some(0..300),
                    vec![
                        node(SymbolKind::Chunk, "BIG.md#0", Some(0..100), vec![]),
                        node(SymbolKind::Chunk, "BIG.md#1", Some(100..300), vec![]),
                    ],
                )],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        };
        let map = collect_file_symbols(&tree);
        let got: Vec<(&str, usize)> = map
            .get("BIG.md")
            .unwrap()
            .iter()
            .map(|(id, s)| (id.qualified_path.as_str(), *s))
            .collect();
        assert_eq!(got, vec![("BIG.md#0", 0), ("BIG.md#1", 100)]);
    }
}
