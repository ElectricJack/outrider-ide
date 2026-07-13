mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use outrider_index::churn::churn_counts_with_cache;
use outrider_index::{index_repo, index_repo_outcome_with_cache};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn git_fixture() -> tempfile::TempDir {
    let dir = common::copy_fixture("mini_repo");
    let p = dir.path();
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "test@test"]);
    git(p, &["config", "user.name", "test"]);
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "one"]);
    // touch lib.rs twice more so it out-churns everything
    fs::write(
        p.join("src/lib.rs"),
        fs::read_to_string(p.join("src/lib.rs")).unwrap() + "\n// x\n",
    )
    .unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "two"]);
    fs::write(
        p.join("src/lib.rs"),
        fs::read_to_string(p.join("src/lib.rs")).unwrap() + "// y\n",
    )
    .unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "three"]);
    dir
}

#[test]
fn churn_counts_are_correct_and_reuse_head_keyed_cache() {
    let dir = git_fixture();
    let p = dir.path();
    let cache_root = tempfile::tempdir().unwrap();

    let outcome = churn_counts_with_cache(p, cache_root.path()).unwrap();
    assert_eq!(outcome.counts.get("src/lib.rs"), Some(&3));
    assert_eq!(outcome.counts.get("README.md"), Some(&1));
    assert_eq!(outcome.warning, None);

    let cache = fs::read_dir(cache_root.path())
        .unwrap()
        .next()
        .expect("project cache directory")
        .unwrap()
        .path()
        .join("churn-cache.json");
    assert!(cache.exists(), "cache written outside the repository");

    // Poison a count while retaining the valid HEAD to prove the cache is reused.
    let mut cached: serde_json::Value = serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
    cached["counts"]["src/lib.rs"] = 99.into();
    fs::write(&cache, serde_json::to_vec_pretty(&cached).unwrap()).unwrap();
    let again = churn_counts_with_cache(p, cache_root.path()).unwrap();
    assert_eq!(again.counts.get("src/lib.rs"), Some(&99));
    assert_eq!(again.warning, None);
}

#[test]
fn churn_cache_is_not_written_inside_repository() {
    let repo = git_fixture();
    let cache = tempfile::tempdir().unwrap();

    let outcome = churn_counts_with_cache(repo.path(), cache.path()).unwrap();

    assert!(!outcome.counts.is_empty());
    assert!(!repo.path().join(".outrider").exists());
}

#[test]
fn unwritable_cache_still_returns_counts() {
    let repo = git_fixture();
    let invalid_cache_root = repo.path().join("regular-file");
    fs::write(&invalid_cache_root, b"x").unwrap();

    let outcome = churn_counts_with_cache(repo.path(), &invalid_cache_root).unwrap();

    assert!(!outcome.counts.is_empty());
    assert!(outcome.warning.is_some());
}

#[test]
fn unexpected_git_metadata_failure_returns_warning() {
    let repo = git_fixture();
    let cache = tempfile::tempdir().unwrap();
    fs::write(repo.path().join(".git/config"), b"[invalid").unwrap();

    let outcome = churn_counts_with_cache(repo.path(), cache.path()).unwrap();

    assert!(outcome.counts.is_empty());
    assert!(outcome.warning.is_some());
}

#[test]
fn index_outcome_retains_churn_warning_after_successful_load() {
    let repo = git_fixture();
    let invalid_cache_root = repo.path().join("regular-file");
    fs::write(&invalid_cache_root, b"x").unwrap();

    let outcome = index_repo_outcome_with_cache(repo.path(), &[], &[], &invalid_cache_root)
        .expect("cache failure must not fail indexing");

    assert!(outcome.tree.root.churn_count > 0);
    assert_eq!(outcome.warnings.len(), 1);
    assert!(outcome.warnings[0].contains("churn cache"));
}

#[test]
fn index_repo_annotates_percentiles_and_inherits_to_methods() {
    let dir = git_fixture();
    let tree = index_repo(dir.path(), &[], &[]).unwrap();

    fn find<'a>(
        n: &'a outrider_index::SymbolNode,
        qual: &str,
    ) -> Option<&'a outrider_index::SymbolNode> {
        if n.id.qualified_path == qual {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, qual))
    }

    let lib = find(&tree.root, "src/lib.rs").unwrap();
    let util = find(&tree.root, "src/util.rs").unwrap();
    assert_eq!(lib.churn_count, 3);
    assert_eq!(util.churn_count, 1);
    // lib.rs is the most-churned of 3 files -> percentile 1.0
    assert_eq!(lib.churn, 1.0);
    assert_eq!(util.churn, 0.0);

    // methods inherit the file's values (spec §5.4)
    let norm = find(&tree.root, "src/lib.rs::Point::norm").unwrap();
    assert_eq!(norm.churn, lib.churn);
    assert_eq!(norm.churn_count, lib.churn_count);

    // folder churn = sum of descendants, ranked among folders
    let src = find(&tree.root, "src").unwrap();
    assert_eq!(src.churn_count, 4); // 3 + 1
}

#[test]
fn non_git_dir_yields_zero_churn_not_error() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path(), &[], &[]).unwrap();
    assert_eq!(tree.root.churn_count, 0);
}
