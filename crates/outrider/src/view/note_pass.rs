use outrider_view::layers::notes::ResolvedNote;
use outrider_view::spec::NoteSource;

use crate::content::{FONT_PX, LINE_STEP};
use crate::paint_model::{wrap_doc, BodyText, DocPanel, RowStyle};
use crate::theme;
use crate::treemap::BODY_PAD;
use crate::world::{Draw, LeafDraw, Rung};

pub(crate) const DOC_PANEL_W: f64 = 280.0;

/// True iff the box is at Card fidelity or finer (notes at Card+ only).
pub(crate) fn note_rung_ok(draw: &Draw) -> bool {
    match draw {
        Draw::Container(r) => matches!(r, Rung::Card | Rung::Detail | Rung::Full),
        Draw::Leaf(t) => matches!(t, LeafDraw::Text | LeafDraw::Minimap),
    }
}

/// Build the floating doc panel from resolved notes.
/// Generalised from the old inline doc panel logic in treemap.rs:
/// rows come from every note in order, doc rows are DOC_COLOR,
/// metric rows are TEXT_SECONDARY.
pub(crate) fn build_doc_panel(
    notes: &[ResolvedNote],
    fx: f32,
    fy: f32,
    fw: f32,
    _fh: f32,
) -> Option<DocPanel> {
    let panel_w = fw.max(DOC_PANEL_W as f32);
    let wrap_w = (panel_w as f64) - 2.0 * BODY_PAD;

    let mut all_rows: Vec<(String, Vec<(usize, u32)>)> = Vec::new();
    // A symbol that is both focused and hovered resolves the same note
    // once per anchor — show each distinct text only once.
    let mut seen: Vec<&str> = Vec::new();
    for note in notes {
        if seen.contains(&note.text.as_str()) {
            continue;
        }
        seen.push(&note.text);
        let color = match note.source {
            NoteSource::Doc => theme::DOC_COLOR,
            NoteSource::Metric => theme::TEXT_SECONDARY,
            NoteSource::Agent => theme::NARRATION_TEXT,
        };
        if !all_rows.is_empty() {
            all_rows.push((String::new(), vec![(0, color)]));
        }
        all_rows.extend(crate::paint_model::format_note_rows(
            &note.text, wrap_w, FONT_PX, color,
        ));
    }

    if all_rows.is_empty() {
        return None;
    }

    // The panel sits above the symbol; a long doc must not run off the top
    // of the window (under the tab bar). Keep the head of the doc and
    // truncate with an ellipsis row.
    const PANEL_TOP_MARGIN: f32 = 44.0;
    let max_rows = (((fy - PANEL_TOP_MARGIN - 4.0) - 2.0 * BODY_PAD as f32)
        / LINE_STEP as f32)
        .floor()
        .max(3.0) as usize;
    if all_rows.len() > max_rows {
        all_rows.truncate(max_rows.saturating_sub(1));
        all_rows.push(("\u{2026}".to_string(), vec![(3, theme::TEXT_SECONDARY)]));
    }

    let row_count = all_rows.len() as f32;
    let panel_h = BODY_PAD as f32 + row_count * LINE_STEP as f32 + BODY_PAD as f32;
    let panel_y = (fy - panel_h - 4.0).max(PANEL_TOP_MARGIN);

    let mut rows = Vec::new();
    let mut y = panel_y + BODY_PAD as f32;
    for (text, runs) in all_rows {
        rows.push(BodyText {
            x: fx + BODY_PAD as f32,
            y,
            text,
            runs,
            highlighted: false,
            style: RowStyle::Doc,
        });
        y += LINE_STEP as f32;
    }

    Some(DocPanel {
        x: fx,
        y: panel_y,
        w: panel_w,
        h: panel_h,
        rows,
    })
}

/// Width of the tour callout card, in screen px.
pub(crate) const CALLOUT_W: f64 = 360.0;
/// Screen rows reserved at the top for the tab bar and menus.
pub(crate) const CALLOUT_TOP_MARGIN: f32 = 84.0;

/// Build the guided-tour callout: a narration card placed just outside the
/// target's screen rect (preferring above, else below, else beside), with a
/// leader line anchor at the rect's nearest edge midpoint. `headline` is
/// bold-ish (NARRATION_TEXT); `body` rows use TEXT_PRIMARY. Returns None
/// when there is no text.
pub(crate) fn build_tour_callout(
    headline: &str,
    body: &str,
    target: (f32, f32, f32, f32), // screen x, y, w, h of the step target
    vw: f32,
    vh: f32,
    step_label: &str,
) -> Option<(crate::paint_model::NoteCallout, bool)> {
    use crate::paint_model::NoteCallout;
    use crate::theme;

    // The callout is the reader: step label, headline, then every
    // paragraph of the note, anchored to what it describes. The side panel
    // is just the navigator, so nothing is said twice.
    let wrap_w = CALLOUT_W - 2.0 * BODY_PAD;
    let mut rows: Vec<(String, Vec<(usize, u32)>)> = Vec::new();
    if !step_label.is_empty() {
        rows.push((
            step_label.to_string(),
            vec![(step_label.len(), theme::TEXT_SECONDARY)],
        ));
    }
    if !headline.trim().is_empty() {
        for t in wrap_doc(headline.trim(), wrap_w, FONT_PX + 1.0) {
            let len = t.len();
            rows.push((t, vec![(len, theme::NARRATION_TEXT)]));
        }
    }
    let body_rows =
        crate::paint_model::format_note_rows(body, wrap_w, FONT_PX, theme::TEXT_PRIMARY);
    if !body_rows.is_empty() && !rows.is_empty() {
        // Spacer between the headline and the body.
        rows.push((String::new(), vec![(0, theme::TEXT_PRIMARY)]));
    }
    rows.extend(body_rows);
    if rows.is_empty() {
        return None;
    }

    let (tx, ty, tw, th) = target;
    let w = CALLOUT_W as f32;
    let h = (BODY_PAD as f32) * 2.0 + rows.len() as f32 * LINE_STEP as f32;
    let gap = 14.0f32;
    // The top strip holds the tab bar / file menu; keep the card below it.
    let top = CALLOUT_TOP_MARGIN;
    let cx = tx + tw / 2.0;
    let cy = ty + th / 2.0;
    let slide_y = |y: f32| y.clamp(top, (vh - h - 4.0).max(top));
    let slide_x = |x: f32| x.clamp(4.0, (vw - w - 4.0).max(4.0));
    // Candidate placements. Side cards may slide along the free band so
    // they stay beside the target rather than over it.
    let above = (slide_x(cx - w / 2.0), ty - gap - h, (cx, ty));
    let below = (slide_x(cx - w / 2.0), ty + th + gap, (cx, ty + th));
    let right = (tx + tw + gap, slide_y(cy - h / 2.0), (tx + tw, cy));
    let left = (tx - gap - w, slide_y(cy - h / 2.0), (tx, cy));
    let fits = |x: f32, y: f32| x >= 4.0 && y >= top && x + w <= vw - 4.0 && y + h <= vh - 4.0;
    let overlaps_target =
        |x: f32, y: f32| x < tx + tw && x + w > tx && y < ty + th && y + h > ty;
    let candidates = [above, below, right, left];
    let placed = candidates
        .iter()
        .copied()
        .find(|(x, y, _)| fits(*x, *y) && !overlaps_target(*x, *y));
    let (mut x, mut y, anchor, minimized) = match placed {
        Some((x, y, a)) => (x, y, a, false),
        None => {
            // No placement avoids the target: the reader has zoomed into
            // the thing the note describes. The note must never obstruct
            // it — collapse to a one-line pill hugging the top edge of the
            // usable area. Full prose stays available by stepping back out.
            rows.truncate(0);
            let mut pill = String::new();
            if !step_label.is_empty() {
                pill.push_str(step_label);
            }
            if !headline.trim().is_empty() {
                if !pill.is_empty() {
                    pill.push_str(" \u{00B7} ");
                }
                pill.push_str(headline.trim());
            }
            let len = pill.len();
            rows.push((pill, vec![(len, theme::NARRATION_TEXT)]));
            (8.0, top, (cx.clamp(8.0, vw - 8.0), ty.max(top)), true)
        }
    };
    let (w, h) = if minimized {
        let longest = rows[0].0.chars().count() as f64 * FONT_PX * 0.62;
        (
            ((longest + 2.0 * BODY_PAD) as f32).min(vw - 16.0),
            (BODY_PAD * 2.0 + LINE_STEP) as f32,
        )
    } else {
        (w, h)
    };
    x = x.clamp(4.0, (vw - w - 4.0).max(4.0));
    y = y.clamp(top, (vh - h - 4.0).max(top));

    let mut out_rows = Vec::with_capacity(rows.len());
    let mut ry = y + BODY_PAD as f32;
    for (text, runs) in rows {
        out_rows.push(BodyText {
            x: x + BODY_PAD as f32,
            y: ry,
            text,
            runs,
            highlighted: false,
            style: RowStyle::Doc,
        });
        ry += LINE_STEP as f32;
    }
    Some((
        NoteCallout {
            x,
            y,
            w,
            h,
            rows: out_rows,
            anchor,
            style: RowStyle::Doc,
        },
        minimized,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Draw, LeafDraw, Rung};

    #[test]
    fn rung_gate() {
        assert!(!note_rung_ok(&Draw::Container(Rung::Dot)));
        assert!(!note_rung_ok(&Draw::Container(Rung::Label)));
        assert!(note_rung_ok(&Draw::Container(Rung::Card)));
        assert!(note_rung_ok(&Draw::Container(Rung::Detail)));
        assert!(note_rung_ok(&Draw::Container(Rung::Full)));
        assert!(!note_rung_ok(&Draw::Leaf(LeafDraw::Dot)));
        assert!(!note_rung_ok(&Draw::Leaf(LeafDraw::Label)));
        assert!(note_rung_ok(&Draw::Leaf(LeafDraw::Text)));
        assert!(note_rung_ok(&Draw::Leaf(LeafDraw::Minimap)));
    }

    #[test]
    fn build_doc_panel_single_doc_note() {
        let note = ResolvedNote {
            source: NoteSource::Doc,
            text: "Hello world".into(),
            range: None,
            lines: None,
            pin: None,
            layer: 0,
        };
        let panel = build_doc_panel(&[note], 100.0, 200.0, 120.0, 50.0).unwrap();
        assert_eq!(panel.x, 100.0);
        assert!(panel.w >= DOC_PANEL_W as f32);
        assert_eq!(panel.rows.len(), 1);
        assert_eq!(panel.rows[0].text, "Hello world");
        assert_eq!(panel.rows[0].runs[0].1, theme::DOC_COLOR);
    }

    #[test]
    fn build_doc_panel_empty_notes() {
        assert!(build_doc_panel(&[], 0.0, 0.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn build_doc_panel_metric_note_color() {
        let note = ResolvedNote {
            source: NoteSource::Metric,
            text: "42L".into(),
            range: None,
            lines: None,
            pin: None,
            layer: 0,
        };
        let panel = build_doc_panel(&[note], 0.0, 100.0, 300.0, 50.0).unwrap();
        assert_eq!(panel.rows[0].runs[0].1, theme::TEXT_SECONDARY);
    }

    #[test]
    fn build_doc_panel_position_above_box() {
        let note = ResolvedNote {
            source: NoteSource::Doc,
            text: "Test".into(),
            range: None,
            lines: None,
            pin: None,
            layer: 0,
        };
        let fy = 200.0f32;
        let panel = build_doc_panel(&[note], 50.0, fy, 300.0, 100.0).unwrap();
        assert!(panel.y < fy, "panel should float above the box");
        let expected_y = fy - panel.h - 4.0;
        assert!((panel.y - expected_y).abs() < 0.01);
    }
}
