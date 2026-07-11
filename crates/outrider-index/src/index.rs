use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rayon::prelude::*;

use crate::parse::{parse_rust_items, parse_c_items, parse_python_items, parse_js_items, parse_ts_items, parse_tsx_items, parse_csharp_items, RawItem};
use crate::scan::{build_tree, scan_files, ParsedFile, ScannedFile};
use crate::types::{dedupe_ids, finalize_children, SymbolId, SymbolNode, SymbolTree};

pub fn index_repo(repo_root: &Path) -> anyhow::Result<SymbolTree> {
    let files = scan_files(repo_root)?;
    let parsed_children = parse_all(repo_root, &files)?;
    let mut tree = build_tree(repo_root, &files, &parsed_children);
    dedupe_ids(&mut tree.root);
    let counts = crate::churn::churn_counts(repo_root)?;
    crate::churn::annotate(&mut tree, &counts);
    Ok(tree)
}

/// Parse source files in parallel (spec §5.2: rayon, whole repo at startup).
/// Dispatches to the correct parser based on file extension.
fn parse_all(
    repo_root: &Path,
    files: &[ScannedFile],
) -> anyhow::Result<BTreeMap<PathBuf, ParsedFile>> {
    files
        .par_iter()
        .filter_map(|f| {
            let ext = f.rel_path.extension()?.to_str()?;
            let parser: fn(&[u8]) -> anyhow::Result<Vec<RawItem>> = match ext {
                "rs" => parse_rust_items,
                "c" | "h" => parse_c_items,
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
            let items = parser(&source)
                .with_context(|| format!("parsing {}", f.rel_path.display()))?;
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
            Ok((f.rel_path.clone(), ParsedFile { items: children, doc }))
        })
        .collect()
}

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
        doc: None,
        measure: item.line_count,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}
