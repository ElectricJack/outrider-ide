//! Git churn analysis: commit-frequency per file, normalized to percentiles,
//! annotated onto the `SymbolTree`. Results are cached outside the analyzed
//! repository, keyed by repository identity and HEAD so repeated opens skip
//! the `git log` invocation (spec §5.4).

use std::collections::BTreeMap;
use std::io::ErrorKind;
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

/// Churn counts plus an optional non-fatal cache or Git warning.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ChurnOutcome {
    pub counts: BTreeMap<String, u64>,
    pub warning: Option<String>,
}

/// Commit counts per current path, cached in the operating system's cache
/// directory. This compatibility wrapper omits non-fatal warnings.
pub fn churn_counts(repo_root: &Path) -> anyhow::Result<BTreeMap<String, u64>> {
    Ok(churn_outcome(repo_root)?.counts)
}

/// Commit counts and non-fatal warnings using the operating system cache root.
pub fn churn_outcome(repo_root: &Path) -> anyhow::Result<ChurnOutcome> {
    let Some(cache_root) = dirs::cache_dir() else {
        return churn_counts_without_cache(
            repo_root,
            Some("cache directory is unavailable".into()),
        );
    };
    churn_counts_with_cache(repo_root, &cache_root.join("outrider/churn"))
}

/// Commit counts and non-fatal warnings using an explicit application cache root.
pub fn churn_counts_with_cache(
    repo_root: &Path,
    cache_root: &Path,
) -> anyhow::Result<ChurnOutcome> {
    let head = match git_head(repo_root) {
        Ok(Some(head)) => head,
        Ok(None) => return Ok(ChurnOutcome::default()),
        Err(warning) => {
            return Ok(ChurnOutcome {
                counts: BTreeMap::new(),
                warning: Some(warning),
            });
        }
    };

    let cache_path = cache_root
        .join(project_key(repo_root))
        .join("churn-cache.json");
    let mut warnings = Vec::new();
    match std::fs::read(&cache_path) {
        Ok(bytes) => match serde_json::from_slice::<ChurnCache>(&bytes) {
            Ok(cache) if cache.head == head => {
                return Ok(ChurnOutcome {
                    counts: cache.counts,
                    warning: None,
                });
            }
            Ok(_) => {}
            Err(error) => warnings.push(format!("could not read churn cache: {error}")),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => warnings.push(format!("could not read churn cache: {error}")),
    }

    let log = match git_stdout(
        repo_root,
        &["log", "--numstat", "--no-renames", "--format=%H"],
    ) {
        Ok(log) => log,
        Err(error) => {
            warnings.push(format!("could not calculate Git churn: {error:#}"));
            return Ok(ChurnOutcome {
                counts: BTreeMap::new(),
                warning: Some(warnings.join("; ")),
            });
        }
    };
    let counts = commit_counts_from_log(&log);

    let cache_write = std::fs::create_dir_all(cache_path.parent().expect("cache path has parent"))
        .and_then(|()| {
            let bytes = serde_json::to_vec_pretty(&ChurnCache {
                head,
                counts: counts.clone(),
            })
            .expect("serializing churn cache cannot fail");
            std::fs::write(&cache_path, bytes)
        });
    if let Err(error) = cache_write {
        warnings.push(format!("could not write churn cache: {error}"));
    }

    Ok(ChurnOutcome {
        counts,
        warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
    })
}

fn churn_counts_without_cache(
    repo_root: &Path,
    warning: Option<String>,
) -> anyhow::Result<ChurnOutcome> {
    let mut warnings: Vec<String> = warning.into_iter().collect();
    match git_head(repo_root) {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(ChurnOutcome::default()),
        Err(error) => {
            warnings.push(error);
            return Ok(ChurnOutcome {
                counts: BTreeMap::new(),
                warning: Some(warnings.join("; ")),
            });
        }
    };
    let counts = match git_stdout(
        repo_root,
        &["log", "--numstat", "--no-renames", "--format=%H"],
    ) {
        Ok(log) => commit_counts_from_log(&log),
        Err(error) => {
            warnings.push(format!("could not calculate Git churn: {error:#}"));
            return Ok(ChurnOutcome {
                counts: BTreeMap::new(),
                warning: Some(warnings.join("; ")),
            });
        }
    };
    Ok(ChurnOutcome {
        counts,
        warning: Some(warnings.join("; ")),
    })
}

fn project_key(repo_root: &Path) -> String {
    let canonical = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let identity = canonical.to_string_lossy().replace('\\', "/");
    project_hash(&identity)
}

fn project_hash(identity: &str) -> String {
    let hash = identity.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

/// Resolve HEAD while distinguishing normal Git absence from unexpected metadata failures.
fn git_head(repo_root: &Path) -> Result<Option<String>, String> {
    match git_stdout(repo_root, &["rev-parse", "HEAD"]) {
        Ok(head) => Ok(Some(head.trim().to_string())),
        Err(error) if is_git_unavailable(&error) => Ok(None),
        Err(error) => {
            let message = format!("{error:#}");
            if message.contains("not a git repository") {
                return Ok(None);
            }

            // An unborn repository has no HEAD yet, but `rev-list --all`
            // succeeds with empty output. Other failures remain visible.
            if git_stdout(repo_root, &["rev-list", "--all", "--max-count=1"])
                .is_ok_and(|commits| commits.trim().is_empty())
            {
                Ok(None)
            } else {
                Err(format!("could not read Git HEAD: {message}"))
            }
        }
    }
}

fn is_git_unavailable(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|io| io.kind() == ErrorKind::NotFound)
}

fn git_command(repo_root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .arg("-C")
        .arg(repo_root);
    command
}

/// Run a git subcommand in `repo_root` and return its UTF-8 stdout, or error on non-zero exit.
fn git_stdout(repo_root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = git_command(repo_root)
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

    #[test]
    fn project_hash_uses_stable_fnv1a() {
        assert_eq!(project_hash("hello"), "a430d84680aabd0b");
    }

    #[test]
    fn git_subprocess_uses_stable_diagnostic_locale() {
        let command = git_command(Path::new("."));
        let env = |wanted: &str| {
            command
                .get_envs()
                .find_map(|(name, value)| (name == wanted).then_some(value))
                .flatten()
        };

        assert_eq!(env("LC_ALL"), Some(std::ffi::OsStr::new("C")));
        assert_eq!(env("LANG"), Some(std::ffi::OsStr::new("C")));
    }

    #[test]
    fn executable_not_found_is_normal_git_unavailability() {
        let error =
            anyhow::Error::new(std::io::Error::from(ErrorKind::NotFound)).context("spawning git");

        assert!(is_git_unavailable(&error));
    }
}
