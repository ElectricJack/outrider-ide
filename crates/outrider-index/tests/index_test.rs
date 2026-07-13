mod common;

use std::collections::BTreeMap;
use std::io::{BufReader, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use outrider_index::index::{
    index_discovered_files, FileRead, FileSource, MAX_RETAINED_FILE_BYTES,
};
use outrider_index::{index_repo, SymbolKind, SymbolNode};

struct CountingSource {
    files: BTreeMap<PathBuf, Vec<u8>>,
    opens: Mutex<BTreeMap<PathBuf, usize>>,
    full_reads: Mutex<BTreeMap<PathBuf, usize>>,
}

impl CountingSource {
    fn with_file(path: &str, contents: &[u8]) -> Self {
        Self {
            files: BTreeMap::from([(PathBuf::from(path), contents.to_vec())]),
            opens: Mutex::new(BTreeMap::new()),
            full_reads: Mutex::new(BTreeMap::new()),
        }
    }

    fn with_repeated_file(path: &str, byte: u8, len: usize) -> Self {
        Self::with_file(path, &vec![byte; len])
    }

    fn count(&self, counts: &Mutex<BTreeMap<PathBuf, usize>>, path: &str) -> usize {
        counts
            .lock()
            .unwrap()
            .get(&PathBuf::from(path))
            .copied()
            .unwrap_or(0)
    }
}

impl FileSource for CountingSource {
    fn open(&self, path: &Path) -> anyhow::Result<Box<dyn std::io::BufRead + Send>> {
        *self
            .opens
            .lock()
            .unwrap()
            .entry(path.to_path_buf())
            .or_default() += 1;
        Ok(Box::new(BufReader::new(Cursor::new(
            self.files.get(path).expect("fixture path").clone(),
        ))))
    }

    fn len(&self, path: &Path) -> anyhow::Result<Option<u64>> {
        Ok(Some(
            self.files.get(path).expect("fixture path").len() as u64
        ))
    }

    fn read(&self, path: &Path, max_bytes: usize) -> anyhow::Result<FileRead> {
        *self
            .full_reads
            .lock()
            .unwrap()
            .entry(path.to_path_buf())
            .or_default() += 1;
        let reader = self.open(path)?;
        let mut bytes = Vec::new();
        reader
            .take((max_bytes + 1) as u64)
            .read_to_end(&mut bytes)?;
        Ok(FileRead {
            lines: if bytes.is_empty() {
                0
            } else {
                bytes.iter().filter(|&&b| b == b'\n').count() as u64
                    + u64::from(!bytes.ends_with(b"\n"))
            },
            byte_count: bytes.len() as u64,
            bytes: (bytes.len() <= max_bytes).then_some(bytes),
        })
    }
}

#[test]
fn supported_file_is_opened_once_for_metrics_parse_and_chunks() {
    let source = CountingSource::with_file("src/lib.rs", b"fn one() {}\n");
    let indexed = index_discovered_files(&source, &[PathBuf::from("src/lib.rs")], None).unwrap();

    assert_eq!(source.count(&source.opens, "src/lib.rs"), 1);
    assert_eq!(indexed[0].lines, 1);
    assert_eq!(indexed[0].parsed.items.len(), 1);
    assert!(indexed[0].source_fingerprint.is_some());
}

#[test]
fn oversized_unsupported_file_is_stream_counted_without_full_read() {
    let source = CountingSource::with_repeated_file("data", b'x', MAX_RETAINED_FILE_BYTES + 1);
    let indexed = index_discovered_files(&source, &[PathBuf::from("data")], None).unwrap();

    assert_eq!(source.count(&source.opens, "data"), 1);
    assert_eq!(source.count(&source.full_reads, "data"), 0);
    assert_eq!(indexed[0].bytes, (MAX_RETAINED_FILE_BYTES + 1) as u64);
    assert_eq!(indexed[0].source_fingerprint, None);
}

fn find<'a>(node: &'a SymbolNode, qual: &str) -> Option<&'a SymbolNode> {
    if node.id.qualified_path == qual {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, qual))
}

#[test]
fn index_repo_parses_rust_files_into_items() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path(), &[], &[]).unwrap();

    let lib = find(&tree.root, "src/lib.rs").expect("src/lib.rs node");
    assert_eq!(lib.id.kind, SymbolKind::File);

    // file children are name-sorted (spec §4.1), not source-ordered:
    // Point (impl), Point (struct), free, inner  -> sorted byte-wise:
    // "Point"(impl? struct?) ties resolved by source order via ordinal
    let kids: Vec<(&str, SymbolKind, u16)> = lib
        .children
        .iter()
        .map(|c| (c.name.as_str(), c.id.kind.clone(), c.id.ordinal))
        .collect();
    assert_eq!(
        kids,
        vec![
            (
                "Point",
                SymbolKind::Item {
                    label: "struct".into()
                },
                0
            ), // struct appears before impl in source
            (
                "Point",
                SymbolKind::Item {
                    label: "impl".into()
                },
                1
            ),
            ("free", SymbolKind::Item { label: "fn".into() }, 0),
            (
                "inner",
                SymbolKind::Item {
                    label: "module".into()
                },
                0
            ),
        ]
    );

    // nesting + qualified paths
    let helper = find(&tree.root, "src/lib.rs::inner::helper").expect("nested fn");
    assert_eq!(helper.id.kind, SymbolKind::Item { label: "fn".into() });
    assert!(helper.byte_range.is_some());

    let norm = find(&tree.root, "src/lib.rs::Point::norm").expect("method");
    assert_eq!(norm.id.kind, SymbolKind::Item { label: "fn".into() });
    assert_eq!(norm.measure, 3); // 3-line method body span

    // Phase 4b metadata: signature + doc (spec §3.1)
    assert_eq!(
        lib.doc.as_deref(),
        Some("Mini fixture library.\nExercises doc extraction.")
    );
    assert_eq!(lib.signature, None);
    assert_eq!(norm.signature.as_deref(), Some("fn norm(&self) -> f64"));
    let util = find(&tree.root, "src/util.rs").expect("util.rs node");
    assert_eq!(util.doc, None);
    assert_eq!(tree.root.signature, None);
    assert_eq!(tree.root.doc, None);

    // ignored file contributed nothing (spec §8.2)
    assert!(find(&tree.root, "generated/junk.rs").is_none());

    // util.rs has its free fn
    let clamp = find(&tree.root, "src/util.rs::clamp").expect("clamp fn");
    assert_eq!(clamp.measure, 3);
}

#[test]
fn symbol_ids_are_unique_tree_wide() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path(), &[], &[]).unwrap();
    fn walk(n: &outrider_index::SymbolNode, out: &mut Vec<outrider_index::SymbolId>) {
        out.push(n.id.clone());
        for c in &n.children {
            walk(c, out);
        }
    }
    let mut ids = Vec::new();
    walk(&tree.root, &mut ids);
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), total, "duplicate SymbolIds in indexed tree");
}
