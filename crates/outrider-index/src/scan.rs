//! Filesystem scanner and symbol-tree assembler.
//! `scan_files` discovers repo source files (respecting .gitignore); `build_tree`
//! wires them together with parsed items into the folder/file/item `SymbolTree`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use ignore::WalkBuilder;

use crate::types::{
    finalize_children, IndexedFile, ParsedFile, SymbolId, SymbolKind, SymbolNode, SymbolTree,
};

/// Compatibility name retained for callers of the original scan/build API.
pub type ScannedFile = IndexedFile;

/// Walk the repo honoring .gitignore / standard ignore files (spec §5.1).
/// `require_git(false)` so ignore rules also apply in non-git dirs (fixtures).
/// Hidden files (dotfiles, .git) are skipped by the walker's default.
/// Generated lock files (Cargo.lock etc.) are skipped: they are not source,
/// and their size dwarfs real files in the treemap.
/// `filter_extensions` lists extensions to skip (e.g. `["exe", "png"]`).
/// `filter_folders` lists folder names to skip (e.g. `["target", "node_modules"]`).
pub fn scan_files(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
) -> anyhow::Result<Vec<ScannedFile>> {
    let paths = discover_files(repo_root, filter_extensions, filter_folders)?;
    crate::index::index_discovered_files(&crate::index::FsFileSource::new(repo_root), &paths, None)
}

/// Discover eligible repo-relative paths without reading file contents.
pub fn discover_files(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(repo_root).require_git(false).build();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry.path().extension().is_some_and(|e| e == "lock") {
            continue;
        }
        // Skip files with filtered extensions
        if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
            if filter_extensions
                .iter()
                .any(|f| f.eq_ignore_ascii_case(ext))
            {
                continue;
            }
        }
        let rel_path = entry
            .path()
            .strip_prefix(repo_root)
            .context("walker yielded path outside repo root")?
            .to_path_buf();
        // Skip files inside filtered folders
        if rel_path.components().any(|c| {
            filter_folders
                .iter()
                .any(|f| c.as_os_str().to_string_lossy() == f.as_str())
        }) {
            continue;
        }
        files.push(rel_path);
    }
    files.sort();
    Ok(files)
}

/// Build the folder/file skeleton. `parsed_children` maps a file's rel_path to its
/// parsed contents (item nodes plus the file's `//!` doc block).
pub fn build_tree(
    repo_root: &Path,
    files: &[ScannedFile],
    parsed_children: &BTreeMap<PathBuf, ParsedFile>,
) -> SymbolTree {
    let root_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string());
    // decompose rel paths into components once
    let decomposed: Vec<(Vec<String>, &ScannedFile)> = files
        .iter()
        .map(|f| {
            let comps = f
                .rel_path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            (comps, f)
        })
        .collect();
    let root = build_folder(&root_name, "", &decomposed, parsed_children);
    SymbolTree {
        root,
        repo_root: repo_root.to_path_buf(),
    }
}

/// Recursively constructs a `Folder` node from a pre-decomposed file list,
/// injecting parsed items and chunk-splitting large unparsed files.
fn build_folder(
    name: &str,
    qualified: &str,
    entries: &[(Vec<String>, &ScannedFile)],
    parsed_children: &BTreeMap<PathBuf, ParsedFile>,
) -> SymbolNode {
    let mut children: Vec<SymbolNode> = Vec::new();
    let mut by_subfolder: BTreeMap<String, Vec<(Vec<String>, &ScannedFile)>> = BTreeMap::new();

    for (comps, file) in entries {
        match comps.as_slice() {
            [file_name] => {
                let qual = join_path(qualified, file_name);
                let parsed = parsed_children
                    .get(&file.rel_path)
                    .cloned()
                    .unwrap_or_else(|| file.parsed.clone());
                let mut node = SymbolNode {
                    id: SymbolId {
                        kind: SymbolKind::File,
                        qualified_path: qual.clone(),
                        ordinal: 0,
                    },
                    name: file_name.clone(),
                    byte_range: Some(0..file.bytes as usize),
                    signature: None,
                    doc: parsed.doc,
                    measure: file.lines,
                    churn: 0.0,
                    churn_count: 0,
                    children: parsed.items,
                };
                if node.children.is_empty() {
                    node.children = file.chunks.clone().unwrap_or_default();
                }
                children.push(node);
            }
            [folder, ..] => {
                by_subfolder
                    .entry(folder.clone())
                    .or_default()
                    .push((comps[1..].to_vec(), *file));
            }
            [] => {}
        }
    }

    for (folder_name, sub_entries) in &by_subfolder {
        let qual = join_path(qualified, folder_name);
        children.push(build_folder(
            folder_name,
            &qual,
            sub_entries,
            parsed_children,
        ));
    }

    finalize_children(&mut children);
    let measure = children.iter().map(|c| c.measure).sum();
    SymbolNode {
        id: SymbolId {
            kind: SymbolKind::Folder,
            qualified_path: qualified.to_string(),
            ordinal: 0,
        },
        name: name.to_string(),
        byte_range: None,
        signature: None,
        doc: None,
        measure,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}

/// Joins a parent qualified path and a child name with `/`, or returns `name` if parent is empty.
fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SymbolKind;

    fn scan_tree(dir: &std::path::Path) -> SymbolTree {
        let files = scan_files(dir, &[], &[]).unwrap();
        build_tree(dir, &files, &BTreeMap::new())
    }

    fn child<'a>(root: &'a SymbolNode, name: &str) -> &'a SymbolNode {
        root.children
            .iter()
            .find(|c| c.name == name)
            .expect("child present")
    }

    #[test]
    fn large_markdown_file_becomes_a_chunk_container() {
        let dir = tempfile::tempdir().unwrap();
        // 3 headed sections, each long enough to be its own chunk.
        let mut text = String::new();
        for h in ["Alpha", "Beta", "Gamma"] {
            text.push_str(&format!("# {h}\n"));
            for i in 0..25 {
                text.push_str(&format!("line {i}\n"));
            }
        }
        std::fs::write(dir.path().join("BIG.md"), &text).unwrap();
        let tree = scan_tree(dir.path());
        let f = child(&tree.root, "BIG.md");
        assert_eq!(f.id.kind, SymbolKind::File);
        assert_eq!(f.children.len(), 3);
        assert!(f.children.iter().all(|c| c.id.kind == SymbolKind::Chunk));
        // byte ranges are contiguous and cover the whole file, in source order
        let mut sorted: Vec<&SymbolNode> = f.children.iter().collect();
        sorted.sort_by_key(|c| c.byte_range.as_ref().unwrap().start);
        assert_eq!(sorted[0].byte_range.as_ref().unwrap().start, 0);
        assert_eq!(
            sorted.last().unwrap().byte_range.as_ref().unwrap().end,
            text.len()
        );
        for w in sorted.windows(2) {
            assert_eq!(
                w[0].byte_range.as_ref().unwrap().end,
                w[1].byte_range.as_ref().unwrap().start
            );
        }
        // chunk qualified_path is "{file}#{i}"
        assert!(f
            .children
            .iter()
            .all(|c| c.id.qualified_path.starts_with("BIG.md#")));
    }

    #[test]
    fn small_file_stays_a_single_page() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("small.txt"), "one\ntwo\nthree\n").unwrap();
        let tree = scan_tree(dir.path());
        let f = child(&tree.root, "small.txt");
        assert!(f.children.is_empty(), "under threshold: not chunked");
    }
}
