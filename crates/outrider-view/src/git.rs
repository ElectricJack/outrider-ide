//! Git diff parsing and repository change detection.
//!
//! Provides [`changed_files`] for identifying which files (and line ranges)
//! were modified, and [`GitProbe`] for cheaply polling whether the repository
//! state has changed without spawning git processes.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// ChangedSpec — what range of commits to diff
// ---------------------------------------------------------------------------

/// Specifies which git diff to compute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedSpec {
    /// Unstaged + staged changes relative to HEAD.
    Worktree,
    /// A single rev compared against the worktree (e.g. `HEAD~1`).
    Rev(String),
    /// An explicit revision range (e.g. `a..b`).
    Range(String, String),
}

/// Parse a user-facing changed spec string into a [`ChangedSpec`].
pub fn parse_changed(s: &str) -> ChangedSpec {
    if s == "worktree" {
        return ChangedSpec::Worktree;
    }
    if let Some((a, b)) = s.split_once("..") {
        return ChangedSpec::Range(a.to_string(), b.to_string());
    }
    ChangedSpec::Rev(s.to_string())
}

// ---------------------------------------------------------------------------
// changed_files — run `git diff -U0` and parse hunks
// ---------------------------------------------------------------------------

/// Run `git diff -U0` according to `spec` and parse hunks to find changed
/// files and their new-side line ranges.
///
/// Returns relative paths (with `/` separators) mapped to a vector of
/// 1-based line ranges on the new side. For `Worktree` mode, untracked files
/// (`??` in porcelain output) are included with an empty range vec (meaning
/// the whole file changed).
pub fn changed_files(
    repo_root: &Path,
    spec: &ChangedSpec,
) -> Result<BTreeMap<String, Vec<Range<usize>>>, String> {
    let diff_output = match spec {
        ChangedSpec::Worktree => {
            let diff = git_stdout_str(
                repo_root,
                &["diff", "-U0", "--no-color", "--no-ext-diff", "HEAD"],
            )?;
            let status = git_stdout_str(
                repo_root,
                &["status", "--porcelain", "--untracked-files=all"],
            )?;
            let mut result = parse_diff_output(&diff);
            // Add untracked files from porcelain status.
            for line in status.lines() {
                if let Some(path) = line.strip_prefix("?? ") {
                    let path = path.trim().replace('\\', "/");
                    result.entry(path).or_insert_with(Vec::new);
                }
            }
            return Ok(result);
        }
        ChangedSpec::Rev(r) => {
            git_stdout_str(repo_root, &["diff", "-U0", "--no-color", "--no-ext-diff", r])?
        }
        ChangedSpec::Range(a, b) => {
            git_stdout_str(repo_root, &["diff", "-U0", "--no-color", "--no-ext-diff", a, b])?
        }
    };
    Ok(parse_diff_output(&diff_output))
}

/// Wrapper around `outrider_index::churn::git_stdout` that maps errors to
/// `String` for our public API.
fn git_stdout_str(repo_root: &Path, args: &[&str]) -> Result<String, String> {
    outrider_index::churn::git_stdout(repo_root, args).map_err(|e| format!("{e:#}"))
}

/// Parse unified diff output (with `-U0`) into a map of file paths to
/// new-side line ranges.
fn parse_diff_output(diff: &str) -> BTreeMap<String, Vec<Range<usize>>> {
    let mut result: BTreeMap<String, Vec<Range<usize>>> = BTreeMap::new();
    let mut current_file: Option<String> = None;

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            current_file = Some(rest.replace('\\', "/"));
        } else if line.starts_with("+++ /dev/null") {
            // File was deleted entirely; no new-side path.
            current_file = None;
        } else if line.starts_with("@@ ") {
            if let Some(ref file) = current_file {
                if let Some(range) = parse_hunk_header(line) {
                    result
                        .entry(file.clone())
                        .or_insert_with(Vec::new)
                        .push(range);
                }
            }
        }
    }
    result
}

/// Parse a `@@ -a[,b] +c[,d] @@` hunk header and return the new-side range
/// as a 1-based `Range<usize>`.
///
/// When `d` is 0 (pure deletion), the range covers the single context line
/// `c` so the hunk is still associated with a location.
fn parse_hunk_header(line: &str) -> Option<Range<usize>> {
    // Format: "@@ -old_start[,old_count] +new_start[,new_count] @@..."
    let after_at = line.strip_prefix("@@ ")?;
    let hunk_info = after_at.split(" @@").next()?;

    // Split into old and new parts.
    let mut parts = hunk_info.split_whitespace();
    let _old = parts.next()?; // -a[,b]
    let new = parts.next()?; // +c[,d]

    let new = new.strip_prefix('+')?;
    let (start_str, count_str) = if let Some((s, c)) = new.split_once(',') {
        (s, c)
    } else {
        (new, "1")
    };

    let start: usize = start_str.parse().ok()?;
    let count: usize = count_str.parse().ok()?;
    let effective_count = count.max(1); // pure deletion (count=0) -> mark line `start`
    Some(start..start + effective_count)
}

// ---------------------------------------------------------------------------
// GitStamp / GitProbe — cheap filesystem polling for repo changes
// ---------------------------------------------------------------------------

/// Snapshot of lightweight git metadata used to detect changes without
/// spawning git processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStamp {
    /// Content of `.git/HEAD` (e.g. `ref: refs/heads/main\n`).
    pub head_content: String,
    /// Mtime (seconds since epoch) of the ref file HEAD points to.
    pub ref_mtime: Option<u64>,
    /// Mtime (seconds since epoch) of `.git/index`.
    pub index_mtime: Option<u64>,
    /// Length of `.git/index` in bytes.
    pub index_len: u64,
}

/// Polls a git repository for changes using only file-system metadata (no git
/// processes). Suitable for driving a "changed" invalidation loop.
pub struct GitProbe {
    git_dir: PathBuf,
    last: Option<GitStamp>,
}

impl GitProbe {
    /// Open a probe for the repository at `repo_root`.
    ///
    /// Returns `None` if `repo_root` does not appear to be a git repository
    /// (no `.git` directory or file).
    pub fn open(repo_root: &Path) -> Option<GitProbe> {
        let dot_git = repo_root.join(".git");
        let git_dir = resolve_git_dir(&dot_git)?;
        Some(GitProbe {
            git_dir,
            last: None,
        })
    }

    /// Poll for changes. Returns `Some(stamp)` if the repository state
    /// differs from the last poll (or on the first call), `None` otherwise.
    pub fn poll(&mut self) -> Option<GitStamp> {
        let stamp = self.read_stamp()?;
        if self.last.as_ref() == Some(&stamp) {
            return None;
        }
        self.last = Some(stamp.clone());
        Some(stamp)
    }

    /// Read the current HEAD sha (40-char hex) by inspecting the ref chain.
    pub fn head_sha(&self) -> Option<String> {
        let head_content = std::fs::read_to_string(self.git_dir.join("HEAD")).ok()?;
        let trimmed = head_content.trim();
        if let Some(ref_path) = trimmed.strip_prefix("ref: ") {
            // Symbolic ref — read the target file first.
            let ref_file = self.git_dir.join(ref_path);
            if let Ok(sha) = std::fs::read_to_string(&ref_file) {
                let sha = sha.trim();
                if sha.len() >= 40 {
                    return Some(sha[..40].to_string());
                }
            }
            // Fall back to packed-refs.
            return self.find_in_packed_refs(ref_path);
        }
        // Detached HEAD — the content is the sha itself.
        if trimmed.len() >= 40 {
            Some(trimmed[..40].to_string())
        } else {
            None
        }
    }

    // -- private helpers --

    fn read_stamp(&self) -> Option<GitStamp> {
        let head_content = std::fs::read_to_string(self.git_dir.join("HEAD")).ok()?;
        let trimmed = head_content.trim();

        let ref_mtime = if let Some(ref_path) = trimmed.strip_prefix("ref: ") {
            let ref_file = self.git_dir.join(ref_path);
            if ref_file.exists() {
                file_mtime(&ref_file)
            } else {
                // Ref doesn't exist on disk; use packed-refs mtime.
                file_mtime(&self.git_dir.join("packed-refs"))
            }
        } else {
            // Detached HEAD — use HEAD's own mtime.
            file_mtime(&self.git_dir.join("HEAD"))
        };

        let index_path = self.git_dir.join("index");
        let (index_mtime, index_len) = if index_path.exists() {
            (file_mtime(&index_path), file_len(&index_path))
        } else {
            (None, 0)
        };

        Some(GitStamp {
            head_content: head_content.clone(),
            ref_mtime,
            index_mtime,
            index_len,
        })
    }

    fn find_in_packed_refs(&self, ref_name: &str) -> Option<String> {
        let packed = std::fs::read_to_string(self.git_dir.join("packed-refs")).ok()?;
        for line in packed.lines() {
            if line.starts_with('#') {
                continue;
            }
            // Format: "<sha> <ref>"
            let mut parts = line.split_whitespace();
            let sha = parts.next()?;
            let name = parts.next()?;
            if name == ref_name && sha.len() >= 40 {
                return Some(sha[..40].to_string());
            }
        }
        None
    }
}

/// Resolve a `.git` path which may be a directory or a gitdir file
/// (used by worktrees: contains `gitdir: <path>`).
fn resolve_git_dir(dot_git: &Path) -> Option<PathBuf> {
    if dot_git.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    if dot_git.is_file() {
        let content = std::fs::read_to_string(dot_git).ok()?;
        let target = content.trim().strip_prefix("gitdir: ")?;
        let target_path = if Path::new(target).is_absolute() {
            PathBuf::from(target)
        } else {
            dot_git.parent()?.join(target)
        };
        if target_path.is_dir() {
            return Some(target_path);
        }
    }
    None
}

fn file_mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// GitCache — memoize changed_files results keyed by spec + stamp
// ---------------------------------------------------------------------------

/// Caches the raw file-to-hunks mapping from [`changed_files`], keyed by
/// the spec text and validated against a [`GitStamp`].
pub struct GitCache {
    entries: HashMap<String, (GitStamp, BTreeMap<String, Vec<Range<usize>>>)>,
}

impl GitCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Retrieve a cached result if the stamp still matches.
    pub fn get(
        &self,
        spec_text: &str,
        current_stamp: &GitStamp,
    ) -> Option<&BTreeMap<String, Vec<Range<usize>>>> {
        self.entries
            .get(spec_text)
            .and_then(|(stamp, map)| if stamp == current_stamp { Some(map) } else { None })
    }

    /// Insert (or overwrite) a cached result.
    pub fn insert(
        &mut self,
        spec_text: String,
        stamp: GitStamp,
        map: BTreeMap<String, Vec<Range<usize>>>,
    ) {
        self.entries.insert(spec_text, (stamp, map));
    }

    /// Discard all cached entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for GitCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_changed ---------------------------------------------------------

    #[test]
    fn parse_changed_worktree() {
        assert_eq!(parse_changed("worktree"), ChangedSpec::Worktree);
    }

    #[test]
    fn parse_changed_rev() {
        assert_eq!(
            parse_changed("HEAD~1"),
            ChangedSpec::Rev("HEAD~1".to_string())
        );
    }

    #[test]
    fn parse_changed_range() {
        assert_eq!(
            parse_changed("abc123..def456"),
            ChangedSpec::Range("abc123".to_string(), "def456".to_string())
        );
    }

    // -- parse_diff_hunks / parse_hunk_header ----------------------------------

    #[test]
    fn parse_diff_hunks() {
        let diff = "\
diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdef0 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,5 @@ fn main() {
@@ -30 +32,0 @@ fn helper() {
diff --git a/README.md b/README.md
index 0000000..1111111 100644
--- /dev/null
+++ b/README.md
@@ -0,0 +1,20 @@
";
        let result = parse_diff_output(diff);
        // src/main.rs: two hunks
        let main_hunks = result.get("src/main.rs").expect("src/main.rs");
        assert_eq!(main_hunks.len(), 2);
        // @@ -10,3 +10,5 @@ -> range 10..15
        assert_eq!(main_hunks[0], 10..15);
        // @@ -30 +32,0 @@ -> pure deletion, mark line 32 -> 32..33
        assert_eq!(main_hunks[1], 32..33);

        // README.md: one hunk
        let readme_hunks = result.get("README.md").expect("README.md");
        assert_eq!(readme_hunks.len(), 1);
        // @@ -0,0 +1,20 @@ -> range 1..21
        assert_eq!(readme_hunks[0], 1..21);
    }

    #[test]
    fn parse_hunk_header_single_line() {
        // @@ -5 +5 @@ means 1 line changed at line 5
        assert_eq!(parse_hunk_header("@@ -5 +5 @@"), Some(5..6));
    }

    #[test]
    fn parse_hunk_header_with_context_suffix() {
        // Some diffs include function context after the second @@
        assert_eq!(
            parse_hunk_header("@@ -1,2 +3,4 @@ fn main() {"),
            Some(3..7)
        );
    }

    // -- GitProbe --------------------------------------------------------------

    #[test]
    fn git_probe_open_non_repo() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        assert!(GitProbe::open(tmp.path()).is_none());
    }

    // -- GitCache --------------------------------------------------------------

    #[test]
    fn git_cache_miss_on_different_stamp() {
        let mut cache = GitCache::new();
        let stamp_a = GitStamp {
            head_content: "ref: refs/heads/main\n".to_string(),
            ref_mtime: Some(1000),
            index_mtime: Some(2000),
            index_len: 500,
        };
        let stamp_b = GitStamp {
            head_content: "ref: refs/heads/main\n".to_string(),
            ref_mtime: Some(1001),
            index_mtime: Some(2000),
            index_len: 500,
        };
        let map = BTreeMap::new();
        cache.insert("worktree".to_string(), stamp_a.clone(), map);

        assert!(cache.get("worktree", &stamp_a).is_some());
        assert!(cache.get("worktree", &stamp_b).is_none());
    }

    #[test]
    fn git_cache_clear() {
        let mut cache = GitCache::new();
        let stamp = GitStamp {
            head_content: "abc".to_string(),
            ref_mtime: None,
            index_mtime: None,
            index_len: 0,
        };
        cache.insert("x".to_string(), stamp.clone(), BTreeMap::new());
        assert!(cache.get("x", &stamp).is_some());
        cache.clear();
        assert!(cache.get("x", &stamp).is_none());
    }
}
