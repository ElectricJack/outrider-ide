mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use outrider_index::churn::churn_counts;
use outrider_index::index_repo;

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
    fs::write(p.join("src/lib.rs"), fs::read_to_string(p.join("src/lib.rs")).unwrap() + "\n// x\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "two"]);
    fs::write(p.join("src/lib.rs"), fs::read_to_string(p.join("src/lib.rs")).unwrap() + "// y\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-qm", "three"]);
    dir
}

#[test]
fn churn_counts_real_repo_writes_and_reuses_cache() {
    let dir = git_fixture();
    let p = dir.path();

    let counts = churn_counts(p).unwrap();
    assert_eq!(counts.get("src/lib.rs"), Some(&3));
    assert_eq!(counts.get("README.md"), Some(&1));

    let cache = p.join(".outrider/churn-cache.json");
    assert!(cache.exists(), "cache written");

    // poison the cache counts but keep the HEAD key valid? No — prove reuse
    // by asserting a second call returns identical data with the cache intact.
    let mtime = fs::metadata(&cache).unwrap().modified().unwrap();
    let again = churn_counts(p).unwrap();
    assert_eq!(again, counts);
    assert_eq!(fs::metadata(&cache).unwrap().modified().unwrap(), mtime, "cache not rewritten");
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
