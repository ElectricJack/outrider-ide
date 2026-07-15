//! Owned paint instructions and pure text helpers for the treemap canvas.

use std::sync::Arc;

use gpui::RenderImage;
use outrider_index::buffer::HighlightSpan;

use crate::theme;

pub(crate) struct BodyText {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) text: String,
    pub(crate) runs: Vec<(usize, u32)>,
    pub(crate) highlighted: bool,
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

pub(crate) struct DocPanel {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
    pub(crate) rows: Vec<BodyText>,
}

pub(crate) struct PaintItem {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
    pub(crate) fill: u32,
    pub(crate) border: u32,
    pub(crate) stripe: Option<u32>,
    pub(crate) focused: bool,
    pub(crate) deferred_overlay: bool,
    pub(crate) neighbor: bool,
    pub(crate) body_font_px: f32,
    pub(crate) header_bg_h: f32,
    pub(crate) header_bg_y: f32,
    pub(crate) body_opacity: f32,
    pub(crate) tex_opacity: f32,
    pub(crate) name: Option<NameRow>,
    pub(crate) body: Vec<BodyText>,
    pub(crate) tex: Option<TexQuad>,
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
