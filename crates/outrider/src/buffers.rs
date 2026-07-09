use std::collections::BTreeMap;
use std::path::PathBuf;

use outrider_index::buffer::{AnchorId, FileBuffer};
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

#[allow(dead_code)]
pub const MAX_BUFFERS: usize = 64;

/// A materialized file: rope-backed buffer plus one anchor per symbol,
/// created at materialization (spec §3.3).
#[allow(dead_code)]
pub struct Materialized {
    pub buffer: FileBuffer,
    anchors: BTreeMap<SymbolId, AnchorId>,
}

#[allow(dead_code)]
impl Materialized {
    /// Rope line index of the symbol's start, via its anchor — the Full
    /// render never reads raw `byte_range` offsets.
    pub fn symbol_start_line(&self, id: &SymbolId) -> Option<usize> {
        let a = self.anchors.get(id)?;
        Some(self.buffer.byte_to_line(self.buffer.resolve_anchor(*a)))
    }
}

/// LRU cache of materialized buffers, keyed by relative file path.
/// Most-recently-used entry is last (spec §4.1).
#[allow(dead_code)]
pub struct BufferManager {
    repo_root: PathBuf,
    entries: Vec<(String, Materialized)>,
}

#[allow(dead_code)]
impl BufferManager {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root, entries: Vec::new() }
    }

    /// The file-path portion of a qualified_path: everything before the
    /// first `::` (the whole path when there is none, as on File nodes).
    pub fn file_path_of(qualified_path: &str) -> &str {
        qualified_path.split("::").next().unwrap_or(qualified_path)
    }

    /// Materialize from disk on first access, creating one anchor per
    /// symbol; refresh recency on hits (no disk re-read); LRU-evict beyond
    /// MAX_BUFFERS. None if the file cannot be read or parsed — the box
    /// falls back to Detail content.
    pub fn get(&mut self, rel_path: &str, symbols: &[(SymbolId, usize)]) -> Option<&Materialized> {
        if let Some(i) = self.entries.iter().position(|(p, _)| p == rel_path) {
            let e = self.entries.remove(i);
            self.entries.push(e);
        } else {
            let text = std::fs::read_to_string(self.repo_root.join(rel_path)).ok()?;
            let mut buffer = FileBuffer::new(text).ok()?;
            let anchors = symbols
                .iter()
                .map(|(id, start)| (id.clone(), buffer.create_anchor(*start)))
                .collect();
            self.entries.push((rel_path.to_string(), Materialized { buffer, anchors }));
            if self.entries.len() > MAX_BUFFERS {
                self.entries.remove(0);
            }
        }
        self.entries.last().map(|(_, m)| m)
    }
}

/// rel file path → (id, byte_range.start) of every item inside that file,
/// from the tree. Built once at view construction; `get` uses it to create
/// anchors at materialization.
#[allow(dead_code)]
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
            items(node, &mut v);
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
    use super::{BufferManager, MAX_BUFFERS, collect_file_symbols};
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    fn write_file(dir: &std::path::Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn fn_id(qual: &str) -> SymbolId {
        SymbolId { kind: SymbolKind::Fn, qualified_path: qual.into(), ordinal: 0 }
    }

    #[test]
    fn file_path_of_splits_at_first_colons() {
        assert_eq!(BufferManager::file_path_of("src/lib.rs::Point::norm"), "src/lib.rs");
        assert_eq!(BufferManager::file_path_of("src/lib.rs"), "src/lib.rs");
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
    fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        assert!(mgr.get("nope.rs", &[]).is_none());
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
        fn node(kind: SymbolKind, qual: &str, byte_range: Option<std::ops::Range<usize>>, children: Vec<SymbolNode>) -> SymbolNode {
            SymbolNode {
                id: SymbolId { kind, qualified_path: qual.into(), ordinal: 0 },
                name: qual.rsplit("::").next().unwrap_or(qual).to_string(),
                byte_range,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
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
                        SymbolKind::Impl,
                        "a.rs::T",
                        Some(0..30),
                        vec![node(SymbolKind::Fn, "a.rs::T::m", Some(10..25), vec![])],
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
            .map(|(id, s)| (id.qualified_path.as_str(), id.kind, *s))
            .collect();
        assert_eq!(
            got,
            vec![("a.rs::T", SymbolKind::Impl, 0), ("a.rs::T::m", SymbolKind::Fn, 10)]
        );
    }
}
