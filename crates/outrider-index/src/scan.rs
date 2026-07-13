//! Filesystem scanner and symbol-tree assembler.
//! `scan_files` discovers repo source files (respecting .gitignore); `build_tree`
//! is the legacy pure structural assembler. The normal `index_repo*` pipeline
//! uses materialized `IndexedFile` values for parsing, docs, and chunks.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::Context;
use ignore::WalkBuilder;

pub use crate::types::ParsedFile;
use crate::types::{finalize_children, IndexedFile, SymbolId, SymbolKind, SymbolNode, SymbolTree};

/// A discovered source file and its raw size metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub rel_path: PathBuf,
    pub lines: u64,
    pub bytes: u64,
}

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
    paths
        .into_iter()
        .map(|rel_path| {
            let file = std::fs::File::open(repo_root.join(&rel_path))
                .with_context(|| format!("reading {}", rel_path.display()))?;
            let (lines, bytes) = count_stream(BufReader::new(file))?;
            Ok(ScannedFile {
                rel_path,
                lines,
                bytes,
            })
        })
        .collect()
}

fn count_stream(mut reader: impl BufRead) -> anyhow::Result<(u64, u64)> {
    let mut newlines = 0;
    let mut bytes = 0;
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
        bytes += len as u64;
        ends_with_newline = last_is_newline;
        reader.consume(len);
    }
    let lines = if bytes == 0 {
        0
    } else {
        newlines + u64::from(!ends_with_newline)
    };
    Ok((lines, bytes))
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

/// Build the folder/file skeleton directly from materialized file products.
pub(crate) fn build_indexed_tree(repo_root: &Path, files: &[IndexedFile]) -> SymbolTree {
    let root_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string());
    // decompose rel paths into components once
    let decomposed: Vec<(Vec<String>, &IndexedFile)> = files
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
    let root = build_indexed_folder(&root_name, "", &decomposed);
    SymbolTree {
        root,
        repo_root: repo_root.to_path_buf(),
    }
}

/// Compatibility assembler for callers that separately scan and parse files.
/// This function is pure over its arguments and never accesses the filesystem;
/// use `index_repo*` for the full parse/docs/chunk pipeline.
pub fn build_tree(
    repo_root: &Path,
    files: &[ScannedFile],
    parsed_children: &BTreeMap<PathBuf, ParsedFile>,
) -> SymbolTree {
    let root_name = repo_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string());
    let decomposed: Vec<(Vec<String>, &ScannedFile)> = files
        .iter()
        .map(|file| {
            let components = file
                .rel_path
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect();
            (components, file)
        })
        .collect();
    SymbolTree {
        root: build_legacy_folder(&root_name, "", &decomposed, parsed_children),
        repo_root: repo_root.to_path_buf(),
    }
}

fn build_legacy_folder(
    name: &str,
    qualified: &str,
    entries: &[(Vec<String>, &ScannedFile)],
    parsed_children: &BTreeMap<PathBuf, ParsedFile>,
) -> SymbolNode {
    let mut children = Vec::new();
    let mut by_subfolder: BTreeMap<String, Vec<(Vec<String>, &ScannedFile)>> = BTreeMap::new();
    for (components, file) in entries {
        match components.as_slice() {
            [file_name] => {
                let qualified_path = join_path(qualified, file_name);
                let parsed = parsed_children
                    .get(&file.rel_path)
                    .cloned()
                    .unwrap_or_default();
                let node = SymbolNode {
                    id: SymbolId {
                        kind: SymbolKind::File,
                        qualified_path: qualified_path.clone(),
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
                children.push(node);
            }
            [folder, ..] => by_subfolder
                .entry(folder.clone())
                .or_default()
                .push((components[1..].to_vec(), *file)),
            [] => {}
        }
    }
    for (folder_name, sub_entries) in &by_subfolder {
        let qualified_path = join_path(qualified, folder_name);
        children.push(build_legacy_folder(
            folder_name,
            &qualified_path,
            sub_entries,
            parsed_children,
        ));
    }
    finalize_children(&mut children);
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
        measure: children.iter().map(|child| child.measure).sum(),
        churn: 0.0,
        churn_count: 0,
        children,
    }
}

/// Recursively constructs a `Folder` node from a pre-decomposed file list,
/// injecting parsed items and chunk-splitting large unparsed files.
fn build_indexed_folder(
    name: &str,
    qualified: &str,
    entries: &[(Vec<String>, &IndexedFile)],
) -> SymbolNode {
    let mut children: Vec<SymbolNode> = Vec::new();
    let mut by_subfolder: BTreeMap<String, Vec<(Vec<String>, &IndexedFile)>> = BTreeMap::new();

    for (comps, file) in entries {
        match comps.as_slice() {
            [file_name] => {
                let qual = join_path(qualified, file_name);
                let parsed = file.parsed.clone();
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
        children.push(build_indexed_folder(folder_name, &qual, sub_entries));
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
    fn small_file_stays_a_single_page() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("small.txt"), "one\ntwo\nthree\n").unwrap();
        let tree = scan_tree(dir.path());
        let f = child(&tree.root, "small.txt");
        assert!(f.children.is_empty(), "under threshold: not chunked");
    }
}
