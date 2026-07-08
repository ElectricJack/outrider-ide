use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use ignore::WalkBuilder;

use crate::types::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub rel_path: PathBuf,
    pub lines: u64,
    pub bytes: u64,
}

/// Walk the repo honoring .gitignore / standard ignore files (spec §5.1).
/// `require_git(false)` so ignore rules also apply in non-git dirs (fixtures).
/// Hidden files (dotfiles, .git) are skipped by the walker's default.
pub fn scan_files(repo_root: &Path) -> anyhow::Result<Vec<ScannedFile>> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(repo_root).require_git(false).build();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel_path = entry
            .path()
            .strip_prefix(repo_root)
            .context("walker yielded path outside repo root")?
            .to_path_buf();
        let bytes = std::fs::read(entry.path())
            .with_context(|| format!("reading {}", entry.path().display()))?;
        files.push(ScannedFile {
            rel_path,
            lines: count_lines(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(files)
}

fn count_lines(bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    if bytes.ends_with(b"\n") {
        newlines
    } else {
        newlines + 1
    }
}

/// Build the folder/file skeleton. `rs_children` maps a file's rel_path to its
/// parsed item nodes (empty map until Task 4 wires in the parser).
pub fn build_tree(
    repo_root: &Path,
    files: &[ScannedFile],
    rs_children: &BTreeMap<PathBuf, Vec<SymbolNode>>,
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
    let root = build_folder(&root_name, "", &decomposed, rs_children);
    SymbolTree {
        root,
        repo_root: repo_root.to_path_buf(),
    }
}

fn build_folder(
    name: &str,
    qualified: &str,
    entries: &[(Vec<String>, &ScannedFile)],
    rs_children: &BTreeMap<PathBuf, Vec<SymbolNode>>,
) -> SymbolNode {
    let mut children: Vec<SymbolNode> = Vec::new();
    let mut by_subfolder: BTreeMap<String, Vec<(Vec<String>, &ScannedFile)>> = BTreeMap::new();

    for (comps, file) in entries {
        match comps.as_slice() {
            [file_name] => {
                let qual = join_path(qualified, file_name);
                let file_children = rs_children
                    .get(&file.rel_path)
                    .cloned()
                    .unwrap_or_default();
                children.push(SymbolNode {
                    id: SymbolId {
                        kind: SymbolKind::File,
                        qualified_path: qual,
                        ordinal: 0,
                    },
                    name: file_name.clone(),
                    byte_range: Some(0..file.bytes as usize),
                    measure: file.lines,
                    churn: 0.0,
                    churn_count: 0,
                    children: file_children,
                });
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
        children.push(build_folder(folder_name, &qual, sub_entries, rs_children));
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
        measure,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}
