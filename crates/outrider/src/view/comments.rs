//! User comments on selected symbols, and the prompt generated from them.
//!
//! Comments are the user's half of the agentic loop: an agent presents a
//! view or tour, the user annotates symbols with questions and change
//! requests (hotkey `c`), and the accumulated list is turned into a prompt
//! for the agent — either copied to the clipboard from the panel, or read
//! directly by the agent via `outrider-cli comments list`. Persisted to
//! `<project>/.outrider/comments.json` so they survive restarts and are
//! visible on disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: u64,
    /// Wire symbol id the comment is attached to; None for a general
    /// comment about the current view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Display name captured at creation, so the row stays readable even
    /// if a re-index renames or drops the symbol.
    pub target_label: String,
    pub text: String,
    /// Active view tab label when the comment was written.
    pub view: String,
    /// Tour step (1-based, with the tour's title) if one was active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tour_step: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentList {
    pub next_id: u64,
    pub comments: Vec<Comment>,
}

impl CommentList {
    fn file_path(repo_root: &Path) -> PathBuf {
        repo_root.join(".outrider").join("comments.json")
    }

    /// Load the project's comment list; a missing or unreadable file is an
    /// empty list.
    pub fn load(repo_root: &Path) -> CommentList {
        std::fs::read_to_string(Self::file_path(repo_root))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist to `.outrider/comments.json`. Failures are ignored — the
    /// in-memory list is the working copy.
    pub fn save(&self, repo_root: &Path) {
        let path = Self::file_path(repo_root);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn add(
        &mut self,
        target: Option<String>,
        target_label: String,
        text: String,
        view: String,
        tour_step: Option<String>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.comments.push(Comment {
            id,
            target,
            target_label,
            text,
            view,
            tour_step,
        });
        id
    }

    /// Remove one comment by id; true when something was removed.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.comments.len();
        self.comments.retain(|c| c.id != id);
        self.comments.len() != before
    }

    pub fn clear(&mut self) {
        self.comments.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }
}

/// Per-comment context the prompt builder needs beyond the stored fields —
/// resolved against the live index at generation time.
pub struct TargetContext {
    pub file: Option<String>,
    pub signature: Option<String>,
}

/// Build the agent prompt from the comment list. `context_fn` resolves a
/// wire id to its current file/signature (None when the symbol is gone).
pub fn build_prompt(
    project_name: &str,
    comments: &[Comment],
    context_fn: impl Fn(&str) -> Option<TargetContext>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("## Outrider feedback — {project_name}\n\n"));
    let views: Vec<&str> = {
        let mut seen = Vec::new();
        for c in comments {
            if !seen.contains(&c.view.as_str()) {
                seen.push(c.view.as_str());
            }
        }
        seen
    };
    if !views.is_empty() {
        out.push_str(&format!("Context: viewed in {}.\n\n", views.join(", ")));
    }
    for (i, c) in comments.iter().enumerate() {
        let n = i + 1;
        match &c.target {
            Some(wire) => {
                out.push_str(&format!("{n}. `{wire}`"));
                if let Some(ctx) = context_fn(wire) {
                    if let Some(file) = ctx.file {
                        out.push_str(&format!(" ({file})"));
                    }
                    if let Some(sig) = ctx.signature {
                        out.push_str(&format!("\n   Signature: `{sig}`"));
                    }
                } else {
                    out.push_str(&format!(" ({})", c.target_label));
                }
            }
            None => out.push_str(&format!("{n}. General ({})", c.view)),
        }
        if let Some(step) = &c.tour_step {
            out.push_str(&format!("\n   During: {step}"));
        }
        out.push('\n');
        for line in c.text.lines() {
            out.push_str(&format!("   > {line}\n"));
        }
        out.push('\n');
    }
    out.push_str(
        "Respond through outrider (run-outrider skill): answer questions with a \
         focused view or tour step, and propose code changes as diffs referencing \
         these symbols. Run `outrider-cli comments clear` once this feedback is \
         addressed so stale comments don't linger.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CommentList {
        let mut l = CommentList::default();
        l.add(
            Some("fn:src/a.rs::foo".into()),
            "foo".into(),
            "Why is this here?".into(),
            "Treemap".into(),
            Some("Matter Engine 101 · step 3".into()),
        );
        l.add(None, "".into(), "Overall: too complex".into(), "Seams".into(), None);
        l
    }

    #[test]
    fn add_remove_clear() {
        let mut l = sample();
        assert_eq!(l.comments.len(), 2);
        assert!(l.remove(0));
        assert!(!l.remove(0));
        assert_eq!(l.comments.len(), 1);
        l.clear();
        assert!(l.is_empty());
        // Ids keep advancing after a clear.
        let id = l.add(None, "".into(), "x".into(), "v".into(), None);
        assert_eq!(id, 2);
    }

    #[test]
    fn roundtrips_through_json() {
        let l = sample();
        let json = serde_json::to_string(&l).unwrap();
        let back: CommentList = serde_json::from_str(&json).unwrap();
        assert_eq!(back.comments.len(), 2);
        assert_eq!(back.comments[0].target.as_deref(), Some("fn:src/a.rs::foo"));
        assert_eq!(back.next_id, 2);
    }

    #[test]
    fn prompt_includes_targets_context_and_instructions() {
        let l = sample();
        let prompt = build_prompt("matter-engine-cpp", &l.comments, |wire| {
            (wire == "fn:src/a.rs::foo").then(|| TargetContext {
                file: Some("src/a.rs".into()),
                signature: Some("fn foo()".into()),
            })
        });
        assert!(prompt.contains("## Outrider feedback — matter-engine-cpp"));
        assert!(prompt.contains("`fn:src/a.rs::foo` (src/a.rs)"));
        assert!(prompt.contains("Signature: `fn foo()`"));
        assert!(prompt.contains("During: Matter Engine 101 · step 3"));
        assert!(prompt.contains("> Why is this here?"));
        assert!(prompt.contains("2. General (Seams)"));
        assert!(prompt.contains("> Overall: too complex"));
        assert!(prompt.contains("comments clear"));
    }

    #[test]
    fn prompt_falls_back_to_stored_label_when_symbol_gone() {
        let l = sample();
        let prompt = build_prompt("p", &l.comments, |_| None);
        assert!(prompt.contains("`fn:src/a.rs::foo` (foo)"));
    }
}
