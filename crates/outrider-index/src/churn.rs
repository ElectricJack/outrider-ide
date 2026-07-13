//! Git churn analysis: commit-frequency per file, normalized to percentiles,
//! annotated onto the `SymbolTree`. Results are cached in `.outrider/churn-cache.json`
//! keyed by HEAD so repeated opens skip the `git log` invocation (spec §5.4).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::types::{SymbolKind, SymbolNode, SymbolTree};

/// Parse `git log --numstat --no-renames --format=%H` output into
/// commit-count-per-path (spec §5.4).
pub fn commit_counts_from_log(log: &str) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for line in log.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue; // commit-hash line or blank
        };
        let is_stat =
            |s: &str| s == "-" || (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
        if is_stat(added) && is_stat(deleted) {
            *counts.entry(path.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// Percentile of each value among the slice: fraction of values strictly
/// below, over (n - 1). Ties share a value; single element -> 0.0.
pub fn percentiles(values: &[u64]) -> Vec<f32> {
    let n = values.len();
    if n <= 1 {
        return vec![0.0; n];
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    values
        .iter()
        .map(|v| sorted.partition_point(|x| x < v) as f32 / (n - 1) as f32)
        .collect()
}

/// On-disk cache record: HEAD SHA plus the full commit-count map.
#[derive(Serialize, Deserialize)]
struct ChurnCache {
    head: String,
    counts: BTreeMap<String, u64>,
}

/// Commit counts per current path, cached in .outrider/churn-cache.json keyed
/// by HEAD (spec §5.4). Non-git dirs yield an empty map.
pub fn churn_counts(repo_root: &Path) -> anyhow::Result<BTreeMap<String, u64>> {
    let Ok(head) = git_stdout(repo_root, &["rev-parse", "HEAD"]) else {
        return Ok(BTreeMap::new()); // not a git repo, or no commits yet
    };
    let head = head.trim().to_string();

    let cache_path = repo_root.join(".outrider/churn-cache.json");
    if let Ok(bytes) = std::fs::read(&cache_path) {
        if let Ok(cache) = serde_json::from_slice::<ChurnCache>(&bytes) {
            if cache.head == head {
                return Ok(cache.counts);
            }
        }
    }

    let log = git_stdout(
        repo_root,
        &["log", "--numstat", "--no-renames", "--format=%H"],
    )?;
    let counts = commit_counts_from_log(&log);

    std::fs::create_dir_all(cache_path.parent().expect("cache path has parent"))?;
    std::fs::write(
        &cache_path,
        serde_json::to_vec_pretty(&ChurnCache {
            head,
            counts: counts.clone(),
        })?,
    )?;
    Ok(counts)
}

/// Run a git subcommand in `repo_root` and return its UTF-8 stdout, or error on non-zero exit.
fn git_stdout(repo_root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .context("spawning git")?;
    if !out.status.success() {
        bail!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8(out.stdout).context("git output not utf-8")
}

/// Annotate the tree: file counts -> percentile among files; folder counts
/// (sum of descendants) -> percentile among folders; items inherit their
/// file's values (spec §5.4).
pub fn annotate(tree: &mut SymbolTree, counts: &BTreeMap<String, u64>) {
    set_counts(&mut tree.root, counts);

    let mut file_counts = Vec::new();
    let mut folder_counts = Vec::new();
    collect_counts(&tree.root, &mut file_counts, &mut folder_counts);
    let file_pcts: BTreeMap<u64, f32> = zip_pct(&file_counts);
    let folder_pcts: BTreeMap<u64, f32> = zip_pct(&folder_counts);

    set_percentiles(&mut tree.root, &file_pcts, &folder_pcts);
}

/// Post-order traversal: assign raw commit counts to file nodes and sum them up to folder nodes.
fn set_counts(node: &mut SymbolNode, counts: &BTreeMap<String, u64>) -> u64 {
    match node.id.kind {
        SymbolKind::File => {
            node.churn_count = counts.get(&node.id.qualified_path).copied().unwrap_or(0);
        }
        SymbolKind::Folder => {
            node.churn_count = node
                .children
                .iter_mut()
                .map(|c| set_counts(c, counts))
                .sum();
            return node.churn_count;
        }
        _ => {} // items are filled from their file in set_percentiles
    }
    // descend into file's items only to keep recursion uniform
    for child in &mut node.children {
        set_counts(child, counts);
    }
    node.churn_count
}

/// Collect raw counts into separate file and folder slices for percentile computation.
fn collect_counts(node: &SymbolNode, files: &mut Vec<u64>, folders: &mut Vec<u64>) {
    match node.id.kind {
        SymbolKind::File => files.push(node.churn_count),
        SymbolKind::Folder => folders.push(node.churn_count),
        _ => return, // items don't rank
    }
    for child in &node.children {
        collect_counts(child, files, folders);
    }
}

/// Map each distinct count to its percentile. Duplicate counts overwrite
/// each other in the map, which is safe because `percentiles` assigns
/// identical values to ties.
fn zip_pct(counts: &[u64]) -> BTreeMap<u64, f32> {
    let pcts = percentiles(counts);
    counts.iter().copied().zip(pcts).collect()
}

/// Write percentile values onto the tree; file nodes also push their values down to child items.
fn set_percentiles(
    node: &mut SymbolNode,
    file_pcts: &BTreeMap<u64, f32>,
    folder_pcts: &BTreeMap<u64, f32>,
) {
    match node.id.kind {
        SymbolKind::File => {
            node.churn = file_pcts.get(&node.churn_count).copied().unwrap_or(0.0);
            let (pct, count) = (node.churn, node.churn_count);
            inherit(&mut node.children, pct, count);
            return;
        }
        SymbolKind::Folder => {
            node.churn = folder_pcts.get(&node.churn_count).copied().unwrap_or(0.0);
        }
        _ => {}
    }
    for child in &mut node.children {
        set_percentiles(child, file_pcts, folder_pcts);
    }
}

/// Recursively propagate a file's churn values to all its descendant item nodes.
fn inherit(children: &mut [SymbolNode], pct: f32, count: u64) {
    for child in children {
        child.churn = pct;
        child.churn_count = count;
        inherit(&mut child.children, pct, count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_commits_per_path_from_numstat_log() {
        // format=%H then numstat lines; binary files show "-\t-"
        let log = "aaaa1111\n\
                   10\t2\tsrc/main.rs\n\
                   3\t0\tREADME.md\n\
                   bbbb2222\n\
                   5\t5\tsrc/main.rs\n\
                   -\t-\tlogo.png\n\
                   cccc3333\n\
                   1\t1\tsrc/main.rs\n";
        let counts = commit_counts_from_log(log);
        assert_eq!(counts.get("src/main.rs"), Some(&3));
        assert_eq!(counts.get("README.md"), Some(&1));
        assert_eq!(counts.get("logo.png"), Some(&1));
        assert_eq!(counts.get("aaaa1111"), None);
    }

    #[test]
    fn percentiles_are_fraction_strictly_below_over_n_minus_1() {
        assert_eq!(
            percentiles(&[10, 20, 30, 20]),
            vec![0.0, 1.0 / 3.0, 1.0, 1.0 / 3.0]
        );
        assert_eq!(percentiles(&[7]), vec![0.0]);
        assert_eq!(percentiles(&[]), Vec::<f32>::new());
        // all equal -> everyone at 0.0
        assert_eq!(percentiles(&[4, 4, 4]), vec![0.0, 0.0, 0.0]);
    }
}
