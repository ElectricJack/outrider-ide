//! Top-level indexing pipeline: scans, parses (in parallel), assembles the
//! symbol tree, deduplicates IDs, and annotates with git churn.
//! Entry point is `index_repo`, re-exported from the crate root.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use anyhow::Context;
use rayon::prelude::*;

use crate::chunk::{strategy_for, CHUNK_MAX_LINES};
use crate::parse::{
    parse_c_items, parse_cpp_items, parse_csharp_items, parse_js_items, parse_python_items,
    parse_rust_items, parse_ts_items, parse_tsx_items, RawItem,
};
use crate::scan::{build_indexed_tree, discover_files};
use crate::types::{
    dedupe_ids, finalize_children, IndexedFile, ParsedFile, SymbolId, SymbolKind, SymbolNode,
    SymbolTree,
};

/// Upper bound for retaining a complete source/text file in memory.
pub(crate) const MAX_RETAINED_FILE_BYTES: usize = 8 * 1024 * 1024;

/// Result of a bounded retained read. `bytes` is absent when the file crossed
/// the retention limit, but metrics still cover the complete stream.
struct FileRead {
    bytes: Option<Vec<u8>>,
    lines: u64,
    byte_count: u64,
}

/// Minimal file I/O seam used by the materialization phase.
pub(crate) trait FileSource: Sync {
    fn open(&self, path: &Path) -> anyhow::Result<Box<dyn BufRead + Send>>;

    fn len(&self, _path: &Path) -> anyhow::Result<Option<u64>> {
        Ok(None)
    }
}

/// Filesystem-backed source rooted at the repository being indexed.
pub(crate) struct FsFileSource<'a> {
    root: &'a Path,
}

impl<'a> FsFileSource<'a> {
    pub(crate) fn new(root: &'a Path) -> Self {
        Self { root }
    }
}

impl FileSource for FsFileSource<'_> {
    fn open(&self, path: &Path) -> anyhow::Result<Box<dyn BufRead + Send>> {
        Ok(Box::new(BufReader::new(File::open(self.root.join(path))?)))
    }

    fn len(&self, path: &Path) -> anyhow::Result<Option<u64>> {
        Ok(Some(std::fs::metadata(self.root.join(path))?.len()))
    }
}

/// Atomic progress counters for non-blocking UI updates during indexing.
pub struct IndexProgress {
    /// 0 = scanning, 1 = parsing, 2 = building tree, 3 = done
    pub phase: AtomicU8,
    pub files_total: AtomicUsize,
    pub files_parsed: AtomicUsize,
}

impl IndexProgress {
    pub fn new() -> Self {
        Self {
            phase: AtomicU8::new(0),
            files_total: AtomicUsize::new(0),
            files_parsed: AtomicUsize::new(0),
        }
    }
}

impl Default for IndexProgress {
    fn default() -> Self {
        Self::new()
    }
}

/// A successfully indexed tree plus non-fatal warnings collected while loading it.
pub struct IndexOutcome {
    pub tree: SymbolTree,
    pub warnings: Vec<String>,
    /// Stable fingerprints keyed by normalized repo-relative source path.
    /// Missing entries are intentionally uncacheable.
    pub source_fingerprints: BTreeMap<String, u64>,
}

/// Full indexing pipeline: scan → parse → assemble → dedupe → churn annotate.
pub fn index_repo(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
) -> anyhow::Result<SymbolTree> {
    Ok(index_repo_outcome(repo_root, filter_extensions, filter_folders)?.tree)
}

/// Full indexing pipeline that retains non-fatal churn warnings.
pub fn index_repo_outcome(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
) -> anyhow::Result<IndexOutcome> {
    index_repo_outcome_impl(repo_root, filter_extensions, filter_folders, None, None)
}

/// Full indexing pipeline with an explicit application cache root.
pub fn index_repo_outcome_with_cache(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
    cache_root: &Path,
) -> anyhow::Result<IndexOutcome> {
    index_repo_outcome_impl(
        repo_root,
        filter_extensions,
        filter_folders,
        None,
        Some(cache_root),
    )
}

fn index_repo_outcome_impl(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
    progress: Option<&IndexProgress>,
    cache_root: Option<&Path>,
) -> anyhow::Result<IndexOutcome> {
    if let Some(progress) = progress {
        progress.phase.store(0, Ordering::Relaxed);
    }
    let paths = discover_files(repo_root, filter_extensions, filter_folders)?;
    if let Some(progress) = progress {
        progress.files_total.store(paths.len(), Ordering::Relaxed);
        progress.phase.store(1, Ordering::Relaxed);
    }
    let files = index_discovered_files(&FsFileSource::new(repo_root), &paths, progress)?;
    let source_fingerprints = files
        .iter()
        .filter_map(|file| {
            file.source_fingerprint.map(|fingerprint| {
                (
                    file.rel_path.to_string_lossy().replace('\\', "/"),
                    fingerprint,
                )
            })
        })
        .collect();
    if let Some(progress) = progress {
        progress.phase.store(2, Ordering::Relaxed);
    }
    let mut tree = build_indexed_tree(repo_root, &files);
    dedupe_ids(&mut tree.root);
    let churn = match cache_root {
        Some(cache_root) => crate::churn::churn_counts_with_cache(repo_root, cache_root)?,
        None => crate::churn::churn_outcome(repo_root)?,
    };
    crate::churn::annotate(&mut tree, &churn.counts);
    if let Some(progress) = progress {
        progress.phase.store(3, Ordering::Relaxed);
    }
    Ok(IndexOutcome {
        tree,
        warnings: churn.warning.into_iter().collect(),
        source_fingerprints,
    })
}

/// Full indexing pipeline with atomic progress reporting.
pub fn index_repo_with_progress(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
    progress: &IndexProgress,
) -> anyhow::Result<SymbolTree> {
    Ok(
        index_repo_with_progress_outcome(repo_root, filter_extensions, filter_folders, progress)?
            .tree,
    )
}

/// Full indexing pipeline with progress reporting and retained non-fatal warnings.
pub fn index_repo_with_progress_outcome(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
    progress: &IndexProgress,
) -> anyhow::Result<IndexOutcome> {
    index_repo_outcome_impl(
        repo_root,
        filter_extensions,
        filter_folders,
        Some(progress),
        None,
    )
}

/// Materialize metrics, parse products, fingerprints, and chunks. Each
/// retained file is opened once; unsupported or oversized files are streamed.
pub(crate) fn index_discovered_files(
    source: &dyn FileSource,
    paths: &[PathBuf],
    progress: Option<&IndexProgress>,
) -> anyhow::Result<Vec<IndexedFile>> {
    let mut indexed: Vec<IndexedFile> = paths
        .par_iter()
        .map(|path| materialize_file(source, path, progress))
        .collect::<anyhow::Result<_>>()?;
    indexed.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(indexed)
}

fn materialize_file(
    source: &dyn FileSource,
    path: &Path,
    progress: Option<&IndexProgress>,
) -> anyhow::Result<IndexedFile> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let parser = parser_for(ext);
    let retain = parser.is_some() || is_retained_text(ext);
    let known_len = source
        .len(path)
        .with_context(|| format!("reading metadata for {}", path.display()))?;

    if retain && known_len.is_none_or(|len| len <= MAX_RETAINED_FILE_BYTES as u64) {
        let read = read_retained(
            source
                .open(path)
                .with_context(|| format!("reading {}", path.display()))?,
            MAX_RETAINED_FILE_BYTES,
        )
        .with_context(|| format!("reading {}", path.display()))?;
        if let Some(bytes) = read.bytes {
            let lines = read.lines;
            let mut parsed = ParsedFile::default();
            if let Some(parser) = parser {
                let items =
                    parser(&bytes).with_context(|| format!("parsing {}", path.display()))?;
                let file_qual = path.to_string_lossy().replace('\\', "/");
                parsed.items = items
                    .into_iter()
                    .map(|item| to_symbol_node(item, &file_qual))
                    .collect();
                finalize_children(&mut parsed.items);
                if ext == "rs" {
                    parsed.doc = crate::parse::file_doc(&bytes);
                }
                if let Some(p) = progress {
                    p.files_parsed.fetch_add(1, Ordering::Relaxed);
                }
            }
            let chunks = if parsed.items.is_empty() && lines > CHUNK_MAX_LINES as u64 {
                std::str::from_utf8(&bytes).ok().and_then(|text| {
                    let chunks = strategy_for(ext).chunks(text);
                    (chunks.len() > 1).then(|| chunk_nodes(path, chunks))
                })
            } else {
                None
            };
            return Ok(IndexedFile {
                rel_path: path.to_path_buf(),
                lines,
                bytes: read.byte_count,
                source_fingerprint: Some(source_fingerprint(&bytes)),
                parsed,
                chunks,
            });
        }
        return Ok(IndexedFile {
            rel_path: path.to_path_buf(),
            lines: read.lines,
            bytes: read.byte_count,
            source_fingerprint: None,
            parsed: ParsedFile::default(),
            chunks: None,
        });
    }

    let mut reader = source
        .open(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let (lines, bytes) = stream_metrics(&mut reader)?;
    Ok(IndexedFile {
        rel_path: path.to_path_buf(),
        lines,
        bytes,
        source_fingerprint: None,
        parsed: ParsedFile::default(),
        chunks: None,
    })
}

type ParserFn = fn(&[u8]) -> anyhow::Result<Vec<RawItem>>;

fn parser_for(ext: &str) -> Option<ParserFn> {
    match ext {
        "rs" => Some(parse_rust_items),
        "c" | "h" => Some(parse_c_items),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Some(parse_cpp_items),
        "py" => Some(parse_python_items),
        "js" | "jsx" => Some(parse_js_items),
        "ts" => Some(parse_ts_items),
        "tsx" => Some(parse_tsx_items),
        "cs" => Some(parse_csharp_items),
        _ => None,
    }
}

fn is_retained_text(ext: &str) -> bool {
    matches!(
        ext,
        "md" | "markdown" | "txt" | "toml" | "json" | "yaml" | "yml" | "css" | "html" | "xml"
    )
}

fn stream_metrics(reader: &mut dyn BufRead) -> anyhow::Result<(u64, u64)> {
    let mut newlines = 0;
    let mut byte_count = 0;
    let mut ends_with_newline = false;
    loop {
        let (len, chunk_newlines, last_is_newline) = {
            let chunk = reader.fill_buf()?;
            if chunk.is_empty() {
                break;
            }
            (
                chunk.len(),
                chunk.iter().filter(|&&byte| byte == b'\n').count() as u64,
                chunk.last() == Some(&b'\n'),
            )
        };
        newlines += chunk_newlines;
        byte_count += len as u64;
        ends_with_newline = last_is_newline;
        reader.consume(len);
    }
    let lines = if byte_count == 0 {
        0
    } else {
        newlines + u64::from(!ends_with_newline)
    };
    Ok((lines, byte_count))
}

fn read_retained(
    mut reader: Box<dyn BufRead + Send>,
    max_bytes: usize,
) -> anyhow::Result<FileRead> {
    let mut retained = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut newlines = 0;
    let mut byte_count = 0;
    let mut ends_with_newline = false;
    let mut overflowed = false;
    loop {
        let (len, chunk_newlines, last_is_newline) = {
            let chunk = reader.fill_buf()?;
            if chunk.is_empty() {
                break;
            }
            if !overflowed && retained.len().saturating_add(chunk.len()) <= max_bytes {
                retained.extend_from_slice(chunk);
            } else {
                retained.clear();
                overflowed = true;
            }
            (
                chunk.len(),
                chunk.iter().filter(|&&byte| byte == b'\n').count() as u64,
                chunk.last() == Some(&b'\n'),
            )
        };
        newlines += chunk_newlines;
        byte_count += len as u64;
        ends_with_newline = last_is_newline;
        reader.consume(len);
    }
    let lines = if byte_count == 0 {
        0
    } else {
        newlines + u64::from(!ends_with_newline)
    };
    Ok(FileRead {
        bytes: (!overflowed).then_some(retained),
        lines,
        byte_count,
    })
}

fn source_fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn chunk_nodes(path: &Path, chunks: Vec<crate::chunk::Chunk>) -> Vec<SymbolNode> {
    let qual = path.to_string_lossy().replace('\\', "/");
    let mut nodes = chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Chunk,
                qualified_path: format!("{qual}#{i}"),
                ordinal: 0,
            },
            name: chunk.label,
            byte_range: Some(chunk.start_byte..chunk.end_byte),
            signature: None,
            doc: None,
            measure: (chunk.end_line - chunk.start_line) as u64,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        })
        .collect::<Vec<_>>();
    finalize_children(&mut nodes);
    nodes
}

/// Converts a `RawItem` (tree-sitter output) into a `SymbolNode`, recursively
/// processing children and finalizing their ordinals.
fn to_symbol_node(item: RawItem, parent_qual: &str) -> SymbolNode {
    let qual = format!("{parent_qual}::{}", item.name);
    let mut children: Vec<SymbolNode> = item
        .children
        .into_iter()
        .map(|c| to_symbol_node(c, &qual))
        .collect();
    finalize_children(&mut children);
    SymbolNode {
        id: SymbolId {
            kind: item.kind,
            qualified_path: qual,
            ordinal: 0,
        },
        name: item.name,
        byte_range: Some(item.byte_range),
        signature: Some(item.signature),
        doc: item.doc,
        measure: item.line_count,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Read};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    struct ChunkProbeReader {
        bytes: Vec<u8>,
        position: usize,
        chunk_size: usize,
        max_inspected: Arc<AtomicUsize>,
    }

    impl Read for ChunkProbeReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let available = self.fill_buf()?;
            let len = available.len().min(output.len());
            output[..len].copy_from_slice(&available[..len]);
            self.consume(len);
            Ok(len)
        }
    }

    impl BufRead for ChunkProbeReader {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            let end = (self.position + self.chunk_size).min(self.bytes.len());
            let chunk = &self.bytes[self.position..end];
            self.max_inspected.fetch_max(chunk.len(), Ordering::Relaxed);
            Ok(chunk)
        }

        fn consume(&mut self, amount: usize) {
            self.position = (self.position + amount).min(self.bytes.len());
        }

        fn read_until(&mut self, _byte: u8, _buf: &mut Vec<u8>) -> io::Result<usize> {
            panic!("streaming metrics must not accumulate a whole line")
        }
    }

    struct ProbeSource {
        bytes: Vec<u8>,
        max_inspected: Arc<AtomicUsize>,
        opens: Arc<AtomicUsize>,
    }

    impl FileSource for ProbeSource {
        fn open(&self, _path: &Path) -> anyhow::Result<Box<dyn BufRead + Send>> {
            self.opens.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(ChunkProbeReader {
                bytes: self.bytes.clone(),
                position: 0,
                chunk_size: 1024,
                max_inspected: Arc::clone(&self.max_inspected),
            }))
        }

        fn len(&self, _path: &Path) -> anyhow::Result<Option<u64>> {
            Ok(Some(self.bytes.len() as u64))
        }
    }

    #[test]
    fn newline_free_oversized_file_is_inspected_in_bounded_chunks() {
        let max_inspected = Arc::new(AtomicUsize::new(0));
        let source = ProbeSource {
            bytes: vec![b'x'; MAX_RETAINED_FILE_BYTES + 1],
            max_inspected: Arc::clone(&max_inspected),
            opens: Arc::new(AtomicUsize::new(0)),
        };

        let indexed = index_discovered_files(&source, &[PathBuf::from("data")], None).unwrap();

        assert_eq!(indexed[0].lines, 1);
        assert_eq!(indexed[0].bytes, (MAX_RETAINED_FILE_BYTES + 1) as u64);
        assert!(max_inspected.load(Ordering::Relaxed) <= 1024);
    }

    #[test]
    fn supported_file_is_opened_once_for_metrics_parse_and_fingerprint() {
        let opens = Arc::new(AtomicUsize::new(0));
        let source = ProbeSource {
            bytes: b"fn one() {}\n".to_vec(),
            max_inspected: Arc::new(AtomicUsize::new(0)),
            opens: Arc::clone(&opens),
        };

        let indexed =
            index_discovered_files(&source, &[PathBuf::from("src/lib.rs")], None).unwrap();

        assert_eq!(opens.load(Ordering::Relaxed), 1);
        assert_eq!(indexed[0].lines, 1);
        assert_eq!(indexed[0].parsed.items.len(), 1);
        assert!(indexed[0].source_fingerprint.is_some());
    }

    #[test]
    fn bounded_stream_metrics_preserve_line_count_semantics() {
        for (bytes, expected_lines) in [
            (b"".as_slice(), 0),
            (b"one".as_slice(), 1),
            (b"one\n".as_slice(), 1),
            (b"one\ntwo".as_slice(), 2),
            (b"one\ntwo\n".as_slice(), 2),
        ] {
            let mut reader = BufReader::with_capacity(2, std::io::Cursor::new(bytes));
            let (lines, byte_count) = stream_metrics(&mut reader).unwrap();
            assert_eq!(lines, expected_lines, "bytes={bytes:?}");
            assert_eq!(byte_count, bytes.len() as u64);
        }
    }
}
