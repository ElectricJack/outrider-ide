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
use crate::scan::{build_tree, discover_files};
use crate::types::{
    dedupe_ids, finalize_children, IndexedFile, ParsedFile, SymbolId, SymbolKind, SymbolNode,
    SymbolTree,
};

/// Upper bound for retaining a complete source/text file in memory.
pub const MAX_RETAINED_FILE_BYTES: usize = 8 * 1024 * 1024;

/// Result of a bounded retained read. `bytes` is absent when the file crossed
/// the retention limit, but metrics still cover the complete stream.
pub struct FileRead {
    pub bytes: Option<Vec<u8>>,
    pub lines: u64,
    pub byte_count: u64,
}

/// Minimal file I/O seam used by the materialization phase.
pub trait FileSource: Sync {
    fn open(&self, path: &Path) -> anyhow::Result<Box<dyn BufRead + Send>>;

    fn len(&self, _path: &Path) -> anyhow::Result<Option<u64>> {
        Ok(None)
    }

    fn read(&self, path: &Path, max_bytes: usize) -> anyhow::Result<FileRead> {
        read_retained(self.open(path)?, max_bytes)
    }
}

/// Filesystem-backed source rooted at the repository being indexed.
pub struct FsFileSource<'a> {
    root: &'a Path,
}

impl<'a> FsFileSource<'a> {
    pub fn new(root: &'a Path) -> Self {
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
    if let Some(progress) = progress {
        progress.phase.store(2, Ordering::Relaxed);
    }
    let mut tree = build_tree(repo_root, &files, &BTreeMap::new());
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
pub fn index_discovered_files(
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
        let read = source
            .read(path, MAX_RETAINED_FILE_BYTES)
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

fn parser_for(ext: &str) -> Option<fn(&[u8]) -> anyhow::Result<Vec<RawItem>>> {
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
    let mut line = Vec::new();
    let mut lines = 0;
    let mut bytes = 0;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        lines += 1;
        bytes += read as u64;
    }
    Ok((lines, bytes))
}

fn read_retained(
    mut reader: Box<dyn BufRead + Send>,
    max_bytes: usize,
) -> anyhow::Result<FileRead> {
    let mut retained = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut line = Vec::new();
    let mut lines = 0;
    let mut byte_count = 0;
    let mut overflowed = false;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        lines += 1;
        byte_count += read as u64;
        if !overflowed && retained.len().saturating_add(read) <= max_bytes {
            retained.extend_from_slice(&line);
        } else {
            retained.clear();
            overflowed = true;
        }
    }
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
