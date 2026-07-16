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
    let root_folder_name = repo_root.file_name();
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
        // Skip files inside filtered folders. Filters without a path separator
        // match any single component (e.g. "target" excludes any target/ at any
        // depth). Filters containing '/' are treated as a relative path prefix.
        // If the filter starts with the project root's folder name, also try
        // matching without it (the treemap labels the root, so users naturally
        // type e.g. "src/scripts/game_repos" when the relative path is
        // "scripts/game_repos").
        if filter_folders.iter().any(|f| {
            if f.contains('/') {
                let filter_path = Path::new(f.as_str());
                if rel_path.starts_with(filter_path) {
                    return true;
                }
                if let Some(root_name) = root_folder_name {
                    if let Ok(stripped) = filter_path.strip_prefix(root_name) {
                        if !stripped.as_os_str().is_empty() {
                            return rel_path.starts_with(stripped);
                        }
                    }
                }
                false
            } else {
                rel_path
                    .components()
                    .any(|c| c.as_os_str().to_string_lossy() == f.as_str())
            }
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

/// Per-extension or per-folder aggregate stats from a pre-scan.
#[derive(Debug, Clone, Default)]
pub struct ExtensionStats {
    pub count: usize,
    pub bytes: u64,
}

/// Result of a lightweight pre-scan: extension and folder stats without reading content.
#[derive(Debug, Clone)]
pub struct PreScanResult {
    pub extensions: BTreeMap<String, ExtensionStats>,
    /// Folder stats keyed by full relative path (e.g. `"src"`, `"src/lib"`,
    /// `"src/lib/utils"`). Stats are recursive — a parent includes all
    /// descendant files.
    pub folders: BTreeMap<String, ExtensionStats>,
    /// Top-level directory names that exist on disk but were excluded by
    /// gitignore or hidden-file rules (e.g. `"node_modules"`, `"target"`).
    pub gitignored_folders: Vec<String>,
    pub total_files: usize,
    pub total_bytes: u64,
}

/// Lightweight pre-scan: walks the repo honoring .gitignore, collects file
/// extension counts/bytes and folder counts/bytes at every depth via
/// `fs::metadata`. No file content is read.
pub fn pre_scan(repo_root: &Path) -> anyhow::Result<PreScanResult> {
    let mut extensions: BTreeMap<String, ExtensionStats> = BTreeMap::new();
    let mut folders: BTreeMap<String, ExtensionStats> = BTreeMap::new();
    let mut total_files: usize = 0;
    let mut total_bytes: u64 = 0;

    let walker = WalkBuilder::new(repo_root).require_git(false).build();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry.path().extension().is_some_and(|e| e == "lock") {
            continue;
        }
        let file_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);

        let ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();

        let ext_stats = extensions.entry(ext).or_default();
        ext_stats.count += 1;
        ext_stats.bytes += file_bytes;

        if let Ok(rel) = entry.path().strip_prefix(repo_root) {
            let comps: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            if comps.len() > 1 {
                let mut path = String::new();
                for comp in &comps[..comps.len() - 1] {
                    if !path.is_empty() {
                        path.push('/');
                    }
                    path.push_str(comp);
                    let folder_stats = folders.entry(path.clone()).or_default();
                    folder_stats.count += 1;
                    folder_stats.bytes += file_bytes;
                }
            }
        }

        total_files += 1;
        total_bytes += file_bytes;
    }

    let mut gitignored_folders = Vec::new();
    if let Ok(entries) = std::fs::read_dir(repo_root) {
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if !folders.contains_key(&name) {
                gitignored_folders.push(name);
            }
        }
        gitignored_folders.sort();
    }

    Ok(PreScanResult {
        extensions,
        folders,
        gitignored_folders,
        total_files,
        total_bytes,
    })
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
