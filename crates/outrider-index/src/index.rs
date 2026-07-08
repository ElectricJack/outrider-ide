use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rayon::prelude::*;

use crate::parse::{parse_rust_items, RawItem};
use crate::scan::{build_tree, scan_files, ScannedFile};
use crate::types::{finalize_children, SymbolId, SymbolNode, SymbolTree};

pub fn index_repo(repo_root: &Path) -> anyhow::Result<SymbolTree> {
    let files = scan_files(repo_root)?;
    let rs_children = parse_all_rust(repo_root, &files)?;
    let mut tree = build_tree(repo_root, &files, &rs_children);
    let counts = crate::churn::churn_counts(repo_root)?;
    crate::churn::annotate(&mut tree, &counts);
    Ok(tree)
}

/// Parse every .rs file in parallel (spec §5.2: rayon, whole repo at startup).
fn parse_all_rust(
    repo_root: &Path,
    files: &[ScannedFile],
) -> anyhow::Result<BTreeMap<PathBuf, Vec<SymbolNode>>> {
    files
        .par_iter()
        .filter(|f| f.rel_path.extension().is_some_and(|e| e == "rs"))
        .map(|f| {
            let source = std::fs::read(repo_root.join(&f.rel_path))
                .with_context(|| format!("reading {}", f.rel_path.display()))?;
            let items = parse_rust_items(&source)
                .with_context(|| format!("parsing {}", f.rel_path.display()))?;
            let file_qual = f.rel_path.to_string_lossy().replace('\\', "/");
            let mut children: Vec<SymbolNode> = items
                .into_iter()
                .map(|item| to_symbol_node(item, &file_qual))
                .collect();
            finalize_children(&mut children);
            Ok((f.rel_path.clone(), children))
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
        measure: item.line_count,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}
