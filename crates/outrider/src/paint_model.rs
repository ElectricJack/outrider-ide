//! Owned paint instructions and pure text helpers for the treemap canvas.

use std::sync::Arc;

use gpui::RenderImage;
use outrider_index::buffer::HighlightSpan;

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RowStyle {
    #[default]
    Code,
    Doc,
    Metric,
    Narration,
}

pub(crate) struct BodyText {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) text: String,
    pub(crate) runs: Vec<(usize, u32)>,
    pub(crate) highlighted: bool,
    pub(crate) style: RowStyle,
}

pub(crate) struct NameRow {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) font_px: f32,
    pub(crate) text: String,
}

pub(crate) struct TexQuad {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
    pub(crate) image: Arc<RenderImage>,
}

/// Procedural line bars for a leaf whose code is not shown as text or as a
/// resident texture: the geometry of its line area (unclipped screen px)
/// plus the cached row silhouettes. `rows` is `None` until the file has
/// been scanned; uniform bars stand in for that frame.
pub(crate) struct BarStrip {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    /// Vertical distance between consecutive source lines at this scale.
    pub(crate) pitch: f32,
    /// Horizontal advance per source column at this scale.
    pub(crate) char_w: f32,
    pub(crate) rows: Option<crate::line_bars::Profile>,
    pub(crate) n_lines: u32,
}

/// Cap on bar quads per frame: past it, remaining strips fall back to the
/// box fill. ~40k quads is well under a millisecond of CPU inside a paint
/// layer and keeps the GPU instance count bounded at whole-repo zoom.
pub(crate) const MAX_BAR_QUADS: usize = 40_000;

impl BarStrip {
    /// Drawn bars: source lines are grouped so no bar is denser than one
    /// per two pixel rows (a bar row and a gap row).
    pub(crate) fn lines_per_bar(&self) -> usize {
        if self.pitch >= 2.0 {
            1
        } else {
            (2.0 / self.pitch.max(0.01)).ceil() as usize
        }
    }

    pub(crate) fn bar_step(&self) -> f32 {
        self.pitch * self.lines_per_bar() as f32
    }

    pub(crate) fn bar_h(&self) -> f32 {
        if self.pitch >= 2.0 {
            (self.pitch * 0.6).clamp(1.0, 10.0)
        } else {
            1.0
        }
    }

    /// Number of bars that intersect the vertical band `[y0, y1)`.
    pub(crate) fn bars_in_band(&self, y0: f32, y1: f32) -> usize {
        let step = self.bar_step();
        if step <= 0.0 || self.n_lines == 0 {
            return 0;
        }
        let n_bars = (self.n_lines as usize).div_ceil(self.lines_per_bar());
        let first = ((y0 - self.y) / step).floor().max(0.0) as usize;
        let last = (((y1 - self.y) / step).ceil().max(0.0) as usize).min(n_bars);
        last.saturating_sub(first)
    }

    /// Silhouette of bar `b`: (indent, len, class) — the longest non-blank
    /// line of the group, or `None` when every line in it is blank.
    pub(crate) fn bar_silhouette(
        &self,
        b: usize,
    ) -> Option<(u8, u8, crate::line_bars::LineClass)> {
        let lpb = self.lines_per_bar();
        let Some(rows) = &self.rows else {
            // Unknown yet: a plain, uniform bar (real rows land next frame).
            return Some((1, 28, crate::line_bars::LineClass::Code));
        };
        let start = b * lpb;
        let end = (start + lpb).min(rows.len());
        rows.get(start..end)?
            .iter()
            .filter(|r| r.class != crate::line_bars::LineClass::Blank)
            .max_by_key(|r| r.len)
            .map(|r| (r.indent, r.len, r.class))
    }
}

pub(crate) struct DocPanel {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
    pub(crate) rows: Vec<BodyText>,
}

pub(crate) struct NoteCallout {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
    pub(crate) rows: Vec<BodyText>,
    pub(crate) anchor: (f32, f32),
    pub(crate) style: RowStyle,
}

pub(crate) struct PaintItem {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
    pub(crate) clip_y: f32,
    pub(crate) clip_h: f32,
    pub(crate) fill: u32,
    pub(crate) border: u32,
    pub(crate) stripe: Option<u32>,
    pub(crate) focused: bool,
    pub(crate) deferred_overlay: bool,
    pub(crate) neighbor: bool,
    pub(crate) light: f32,
    pub(crate) body_font_px: f32,
    pub(crate) header_bg_h: f32,
    pub(crate) header_bg_y: f32,
    pub(crate) body_opacity: f32,
    pub(crate) tex_opacity: f32,
    pub(crate) name: Option<NameRow>,
    pub(crate) body: Vec<BodyText>,
    pub(crate) tex: Option<TexQuad>,
    /// Line bars for a leaf with no text rows and no resident texture.
    pub(crate) bars: Option<BarStrip>,
    /// Structural/agent mark: draw an accent ring and, if present, a small
    /// corner badge with this text (custom mark label, or the kind name).
    pub(crate) badge: Option<MarkBadge>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MarkBadge {
    pub(crate) text: String,
    pub(crate) color: u32,
}

pub(crate) struct PaintFrame {
    pub(crate) items: Vec<PaintItem>,
    pub(crate) doc_panel: Option<DocPanel>,
    pub(crate) cg_scrim: bool,
    pub(crate) edges: crate::view::edge_pass::EdgeFrame,
    /// Guided-tour narration card anchored to the live step's target.
    pub(crate) tour_callout: Option<NoteCallout>,
}

pub(crate) fn truncate_to_width(name: &str, w_px: f32, font_px: f32) -> Option<String> {
    let budget = ((w_px - 12.0) / (font_px * 0.62) + 1e-6).floor() as isize;
    if budget < 2 {
        return None;
    }
    let budget = budget as usize;
    if name.chars().count() <= budget {
        Some(name.to_string())
    } else {
        let cut: String = name.chars().take(budget - 1).collect();
        Some(format!("{cut}…"))
    }
}

pub(crate) fn char_budget(w_px: f32, font_px: f32) -> usize {
    let budget = ((w_px - 12.0) / (font_px * 0.62) + 1e-6).floor() as isize;
    if budget < 2 {
        0
    } else {
        budget as usize
    }
}

pub(crate) fn wrap_to_budget(text: &str, budget: usize) -> Vec<String> {
    if budget == 0 {
        return Vec::new();
    }
    if text.chars().count() <= budget {
        return vec![text.to_string()];
    }
    let mut rows = Vec::new();
    let mut line = String::new();
    let mut line_len = 0usize;
    for word in text.split(' ') {
        let mut word = word;
        let mut word_len = word.chars().count();
        while word_len > budget {
            if line_len > 0 {
                rows.push(std::mem::take(&mut line));
                line_len = 0;
            }
            let cut = word
                .char_indices()
                .nth(budget)
                .map_or(word.len(), |(i, _)| i);
            rows.push(word[..cut].to_string());
            word = &word[cut..];
            word_len = word.chars().count();
        }
        if word_len == 0 {
            continue;
        }
        let need = if line_len == 0 {
            word_len
        } else {
            line_len + 1 + word_len
        };
        if need > budget {
            rows.push(std::mem::take(&mut line));
            line.push_str(word);
            line_len = word_len;
        } else {
            if line_len > 0 {
                line.push(' ');
            }
            line.push_str(word);
            line_len = need;
        }
    }
    if line_len > 0 {
        rows.push(line);
    }
    rows
}

pub(crate) fn wrap_doc(text: &str, w_px: f64, font_px: f64) -> Vec<String> {
    let budget = ((w_px - 12.0) / (font_px * 0.62) + 1e-6).floor() as isize;
    if budget < 2 {
        return Vec::new();
    }
    let budget = budget as usize;
    let mut rows = Vec::new();
    for para in text.split("\n\n") {
        let joined = para.split_whitespace().collect::<Vec<_>>().join(" ");
        if joined.is_empty() {
            continue;
        }
        rows.extend(wrap_to_budget(&joined, budget));
    }
    rows
}

/// True for a markdown-style list-item line: `- `, `* `, `• `, or `1. `.
fn is_list_item(t: &str) -> bool {
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("\u{2022} ") {
        return true;
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && matches!(t[digits..].chars().next(), Some('.') | Some(')'))
}

/// Hanging indent for a list item's continuation rows: the marker width.
fn list_hang(t: &str) -> usize {
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("\u{2022} ") {
        return 2;
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    (digits + 2).min(6)
}

/// Strip inline backticks from a row and return the text plus color runs:
/// `base` prose with `code` spans recolored CODE_SPAN. An unpaired backtick
/// colors the remainder of the row.
fn code_runs(text: &str, base: u32) -> (String, Vec<(usize, u32)>) {
    if !text.contains('`') {
        return (text.to_string(), vec![(text.len(), base)]);
    }
    let mut out = String::with_capacity(text.len());
    let mut runs: Vec<(usize, u32)> = Vec::new();
    let mut push = |s: &str, color: u32, out: &mut String, runs: &mut Vec<(usize, u32)>| {
        if s.is_empty() {
            return;
        }
        out.push_str(s);
        match runs.last_mut() {
            Some((len, c)) if *c == color => *len += s.len(),
            _ => runs.push((s.len(), color)),
        }
    };
    let mut rest = text;
    let mut in_code = false;
    while let Some(pos) = rest.find('`') {
        push(&rest[..pos], if in_code { theme::CODE_SPAN } else { base }, &mut out, &mut runs);
        rest = &rest[pos + 1..];
        in_code = !in_code;
    }
    push(rest, if in_code { theme::CODE_SPAN } else { base }, &mut out, &mut runs);
    if runs.is_empty() {
        runs.push((0, base));
    }
    (out, runs)
}

/// Markdown-lite formatting for note and doc prose, replacing the flat
/// wrap: paragraphs separated by spacer rows, list items (`- `, `1. `, …)
/// on their own rows with a hanging indent, and inline `code` spans
/// recolored with the backticks stripped. Returns (text, color runs) rows;
/// spacer rows are empty strings.
pub(crate) fn format_note_rows(
    text: &str,
    w_px: f64,
    font_px: f64,
    base: u32,
) -> Vec<(String, Vec<(usize, u32)>)> {
    let budget = ((w_px - 12.0) / (font_px * 0.62) + 1e-6).floor() as isize;
    if budget < 4 {
        return Vec::new();
    }
    let budget = budget as usize;

    // Group lines into blocks: blank lines separate paragraphs, and every
    // list item is its own block so it keeps its own row(s).
    enum Piece {
        Gap,
        Block { hang: usize, text: String },
    }
    let mut pieces: Vec<Piece> = Vec::new();
    let mut cur: Option<(usize, String)> = None;
    let mut saw_blank = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            if let Some((h, s)) = cur.take() {
                pieces.push(Piece::Block { hang: h, text: s });
            }
            saw_blank = true;
            continue;
        }
        if is_list_item(t) || cur.is_none() || saw_blank {
            if let Some((h, s)) = cur.take() {
                pieces.push(Piece::Block { hang: h, text: s });
            }
            if saw_blank && !pieces.is_empty() {
                pieces.push(Piece::Gap);
            }
            saw_blank = false;
            let hang = if is_list_item(t) { list_hang(t) } else { 0 };
            cur = Some((hang, t.to_string()));
        } else if let Some((_, s)) = &mut cur {
            s.push(' ');
            s.push_str(t);
        }
    }
    if let Some((h, s)) = cur.take() {
        pieces.push(Piece::Block { hang: h, text: s });
    }

    let mut out: Vec<(String, Vec<(usize, u32)>)> = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Gap => out.push((String::new(), vec![(0, base)])),
            Piece::Block { hang, text } => {
                let wrapped = wrap_to_budget(&text, budget.saturating_sub(hang).max(4));
                for (i, row) in wrapped.into_iter().enumerate() {
                    let prefixed = if i == 0 || hang == 0 {
                        row
                    } else {
                        format!("{}{row}", " ".repeat(hang))
                    };
                    out.push(code_runs(&prefixed, base));
                }
            }
        }
    }
    out
}

pub(crate) fn runs_from_spans(len: usize, spans: &[HighlightSpan]) -> Vec<(usize, u32)> {
    let mut runs = Vec::new();
    let mut pos = 0;
    for span in spans {
        let start = span.range.start.min(len);
        let end = span.range.end.min(len);
        if start > pos {
            runs.push((start - pos, theme::TEXT_PRIMARY));
        }
        if end > start {
            runs.push((end - start, theme::syntax_color(span.kind)));
        }
        pos = pos.max(end);
    }
    if pos < len {
        runs.push((len - pos, theme::TEXT_PRIMARY));
    }
    runs
}

pub(crate) fn code_line(
    text: &str,
    spans: &[HighlightSpan],
    width: f32,
    font_px: f32,
) -> Option<(String, Vec<(usize, u32)>)> {
    let shown = truncate_to_width(text, width, font_px)?;
    let truncated = shown != text;
    let kept = if truncated {
        shown.len() - '…'.len_utf8()
    } else {
        shown.len()
    };
    let mut runs = runs_from_spans(kept, spans);
    if truncated {
        runs.push(('…'.len_utf8(), theme::TEXT_PRIMARY));
    }
    Some((shown, runs))
}

pub(crate) fn wrap_code_line(
    text: &str,
    spans: &[HighlightSpan],
    width: f32,
    font_px: f32,
) -> Vec<(String, Vec<(usize, u32)>)> {
    let budget = char_budget(width, font_px);
    if budget == 0 {
        return Vec::new();
    }
    if text.chars().count() <= budget {
        return vec![(text.to_string(), runs_from_spans(text.len(), spans))];
    }
    let full_runs = runs_from_spans(text.len(), spans);
    let mut result = Vec::new();
    let mut text_off = 0usize;
    let mut run_idx = 0usize;
    let mut run_off = 0usize;
    while text_off < text.len() {
        let rest = &text[text_off..];
        let take_chars = budget.min(rest.chars().count());
        let take_bytes = rest
            .char_indices()
            .nth(take_chars)
            .map_or(rest.len(), |(i, _)| i);
        let segment = rest[..take_bytes].to_string();
        let mut segment_runs = Vec::new();
        let mut left = take_bytes;
        while left > 0 && run_idx < full_runs.len() {
            let (run_len, color) = full_runs[run_idx];
            let available = run_len - run_off;
            let used = available.min(left);
            segment_runs.push((used, color));
            left -= used;
            if used == available {
                run_idx += 1;
                run_off = 0;
            } else {
                run_off += used;
            }
        }
        result.push((segment, segment_runs));
        text_off += take_bytes;
    }
    result
}
