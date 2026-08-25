//! The guided-tour side panel: a right-hand column listing every step of
//! the active tour, the live step's narration, and Prev/Next/Exit controls.
//! Pure layout over `TourPanelModel` so it can be unit-tested without GPUI.

use outrider_view::spec::{Step, StepTarget};

use crate::view::tour::{body, headline};

/// Width reserved for the panel on the right edge, in px.
pub(crate) const PANEL_W: f32 = 300.0;
/// Longest step-list row title before eliding.
const ROW_TITLE_MAX: usize = 44;

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}\u{2026}", cut.trim_end())
    }
}

/// One row in the step list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StepRow {
    pub(crate) index: usize,
    pub(crate) title: String,
    pub(crate) state: RowState,
    /// Tab this step plays on, when it differs from the origin view.
    pub(crate) tab: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowState {
    Done,
    Current,
    Upcoming,
}

/// A sub-step row under the live step.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PartRow {
    pub(crate) index: usize,
    pub(crate) title: String,
    pub(crate) current: bool,
    pub(crate) done: bool,
}

/// Everything the renderer needs for one frame of the panel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TourPanelModel {
    pub(crate) view_title: String,
    pub(crate) step_index: usize,
    pub(crate) total: usize,
    pub(crate) rows: Vec<StepRow>,
    /// Sub-steps of the live step (empty when it has none).
    pub(crate) parts: Vec<PartRow>,
    /// Headline of the live step (first note line), or a target label.
    pub(crate) headline: String,
    /// Narration body paragraphs (blank-line separated).
    pub(crate) paragraphs: Vec<String>,
    pub(crate) has_prev: bool,
    pub(crate) has_next: bool,
}

/// Markdown-lite paragraph split for GPUI text: blank lines separate
/// paragraphs, list items (`- `, `* `, `• `, `1. `) each become their own
/// entry (GPUI wraps within an entry), and inline backticks are stripped.
pub(crate) fn md_paragraphs(body: &str) -> Vec<String> {
    fn is_item(t: &str) -> bool {
        if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("\u{2022} ") {
            return true;
        }
        let d = t.chars().take_while(|c| c.is_ascii_digit()).count();
        d > 0 && matches!(t[d..].chars().next(), Some('.') | Some(')'))
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() || is_item(t) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if is_item(t) {
                cur = t.to_string();
            }
            continue;
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(t);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    for p in &mut out {
        *p = p.replace('`', "");
    }
    out
}

/// Human label for a step with no note, derived from its target.
fn target_label(t: &StepTarget) -> String {
    match t {
        StepTarget::Frame(outrider_view::spec::SetRef::Name(n)) => format!("Frame '{n}'"),
        StepTarget::Frame(_) => "Frame set".into(),
        StepTarget::Focus(w) => {
            // Show the last path segment only.
            let tail = w.rsplit("::").next().unwrap_or(w);
            let tail = tail.rsplit('/').next().unwrap_or(tail);
            let tail = tail.split('#').next().unwrap_or(tail);
            format!("Focus {tail}")
        }
        StepTarget::Home(_) => "Overview".into(),
    }
}

pub(crate) fn build_model(
    view_title: &str,
    steps: &[Step],
    step_index: usize,
    part_index: Option<usize>,
) -> TourPanelModel {
    let rows = steps
        .iter()
        .enumerate()
        .map(|(i, s)| StepRow {
            index: i,
            title: s
                .note
                .as_deref()
                .map(headline)
                .filter(|h| !h.is_empty())
                .map(|h| elide(h, ROW_TITLE_MAX))
                .unwrap_or_else(|| target_label(&s.target)),
            state: if i < step_index {
                RowState::Done
            } else if i == step_index {
                RowState::Current
            } else {
                RowState::Upcoming
            },
            tab: s.tab.clone(),
        })
        .collect::<Vec<_>>();
    let live = steps.get(step_index);
    let headline_text = live
        .and_then(|s| s.note.as_deref())
        .map(headline)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .or_else(|| live.map(|s| target_label(&s.target)))
        .unwrap_or_default();
    let paragraphs: Vec<String> = live
        .and_then(|s| s.note.as_deref())
        .map(body)
        .filter(|b| !b.is_empty())
        .map(md_paragraphs)
        .unwrap_or_default();
    let parts: Vec<PartRow> = live
        .map(|s| {
            s.parts
                .iter()
                .enumerate()
                .map(|(pi, p)| PartRow {
                    index: pi,
                    title: p
                        .note
                        .as_deref()
                        .map(headline)
                        .filter(|h| !h.is_empty())
                        .map(|h| elide(h, ROW_TITLE_MAX - 4))
                        .unwrap_or_else(|| target_label(&p.target)),
                    current: part_index == Some(pi),
                    done: part_index.map_or(false, |cur| pi < cur),
                })
                .collect()
        })
        .unwrap_or_default();
    let n_parts = parts.len();
    TourPanelModel {
        view_title: view_title.to_string(),
        step_index,
        total: steps.len(),
        rows,
        parts,
        headline: headline_text,
        paragraphs,
        has_prev: step_index > 0 || part_index.is_some(),
        has_next: step_index + 1 < steps.len()
            || part_index.map_or(n_parts > 0, |p| p + 1 < n_parts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_view::spec::{HomeFlag, SetRef};

    fn step(note: Option<&str>, target: StepTarget) -> Step {
        Step {
            target,
            push: vec![],
            pop: 0,
            note: note.map(String::from),
            tab: None,
            parts: Vec::new(),
        }
    }

    #[test]
    fn rows_reflect_progress_and_titles() {
        let steps = vec![
            step(Some("First\nBody one.\n\nBody two."), StepTarget::Home(HomeFlag)),
            step(None, StepTarget::Frame(SetRef::Name("kernel".into()))),
            step(None, StepTarget::Focus("struct:a/b.h::ns::Baker#0".into())),
        ];
        let m = build_model("Lesson", &steps, 1, None);
        assert_eq!(m.rows[0].state, RowState::Done);
        assert_eq!(m.rows[1].state, RowState::Current);
        assert_eq!(m.rows[2].state, RowState::Upcoming);
        assert_eq!(m.rows[0].title, "First");
        assert_eq!(m.rows[1].title, "Frame 'kernel'");
        assert_eq!(m.rows[2].title, "Focus Baker");
        assert!(m.has_prev && m.has_next);
        assert!(m.paragraphs.is_empty());
    }

    #[test]
    fn paragraphs_split_on_blank_lines_and_reflow() {
        let steps = vec![step(
            Some("Title\nline one\ncontinues here.\n\nSecond para."),
            StepTarget::Home(HomeFlag),
        )];
        let m = build_model("L", &steps, 0, None);
        assert_eq!(m.headline, "Title");
        assert_eq!(m.paragraphs, vec!["line one continues here.", "Second para."]);
        assert!(!m.has_prev && !m.has_next);
    }
}
