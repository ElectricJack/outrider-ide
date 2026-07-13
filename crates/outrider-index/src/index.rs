//! Top-level indexing pipeline: scans, parses (in parallel), assembles the
//! symbol tree, deduplicates IDs, and annotates with git churn.
//! Entry point is `index_repo`, re-exported from the crate root.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use anyhow::Context;
use rayon::prelude::*;

use crate::parse::{
    parse_c_items, parse_cpp_items, parse_csharp_items, parse_js_items, parse_python_items,
    parse_rust_items, parse_ts_items, parse_tsx_items, RawItem,
};
use crate::scan::{build_tree, scan_files, ParsedFile, ScannedFile};
use crate::types::{dedupe_ids, finalize_children, SymbolId, SymbolNode, SymbolTree};

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

/// Full indexing pipeline: scan → parse → assemble → dedupe → churn annotate.
pub fn index_repo(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
) -> anyhow::Result<SymbolTree> {
    let files = scan_files(repo_root, filter_extensions, filter_folders)?;
    let parsed_children = parse_all(repo_root, &files, None)?;
    let mut tree = build_tree(repo_root, &files, &parsed_children);
    dedupe_ids(&mut tree.root);
    let counts = crate::churn::churn_counts(repo_root)?;
    crate::churn::annotate(&mut tree, &counts);
    Ok(tree)
}

/// Full indexing pipeline with atomic progress reporting.
pub fn index_repo_with_progress(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
    progress: &IndexProgress,
) -> anyhow::Result<SymbolTree> {
    progress.phase.store(0, Ordering::Relaxed);
    let files = scan_files(repo_root, filter_extensions, filter_folders)?;
    progress.files_total.store(files.len(), Ordering::Relaxed);
    progress.phase.store(1, Ordering::Relaxed);
    let parsed_children = parse_all(repo_root, &files, Some(progress))?;
    progress.phase.store(2, Ordering::Relaxed);
    let mut tree = build_tree(repo_root, &files, &parsed_children);
    dedupe_ids(&mut tree.root);
    let counts = crate::churn::churn_counts(repo_root)?;
    crate::churn::annotate(&mut tree, &counts);
    progress.phase.store(3, Ordering::Relaxed);
    Ok(tree)
}

/// Parse source files in parallel. Dispatches to the correct parser based on
/// file extension. When `progress` is provided, atomically increments
/// `files_parsed` after each file completes.
fn parse_all(
    repo_root: &Path,
    files: &[ScannedFile],
    progress: Option<&IndexProgress>,
) -> anyhow::Result<BTreeMap<PathBuf, ParsedFile>> {
    files
        .par_iter()
        .filter_map(|f| {
            let ext = f.rel_path.extension()?.to_str()?;
            let parser: fn(&[u8]) -> anyhow::Result<Vec<RawItem>> = match ext {
                "rs" => parse_rust_items,
                "c" | "h" => parse_c_items,
                "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => parse_cpp_items,
                "py" => parse_python_items,
                "js" | "jsx" => parse_js_items,
                "ts" => parse_ts_items,
                "tsx" => parse_tsx_items,
                "cs" => parse_csharp_items,
                _ => return None,
            };
            Some((f, parser))
        })
        .map(|(f, parser)| {
            let source = std::fs::read(repo_root.join(&f.rel_path))
                .with_context(|| format!("reading {}", f.rel_path.display()))?;
            let items =
                parser(&source).with_context(|| format!("parsing {}", f.rel_path.display()))?;
            let file_qual = f.rel_path.to_string_lossy().replace('\\', "/");
            let mut children: Vec<SymbolNode> = items
                .into_iter()
                .map(|item| to_symbol_node(item, &file_qual))
                .collect();
            finalize_children(&mut children);
            let doc = if f.rel_path.extension().is_some_and(|e| e == "rs") {
                crate::parse::file_doc(&source)
            } else {
                None
            };
            if let Some(p) = progress {
                p.files_parsed.fetch_add(1, Ordering::Relaxed);
            }
            Ok((
                f.rel_path.clone(),
                ParsedFile {
                    items: children,
                    doc,
                },
            ))
        })
        .collect()
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
