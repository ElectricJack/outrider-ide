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

    let mut all_rows: Vec<(String, u32)> = Vec::new();
    for note in notes {
        let color = match note.source {
            NoteSource::Doc => theme::DOC_COLOR,
            NoteSource::Metric => theme::TEXT_SECONDARY,
            NoteSource::Agent => theme::NARRATION_TEXT,
        };
        let wrapped = wrap_doc(&note.text, wrap_w, FONT_PX);
        for text in wrapped {
            all_rows.push((text, color));
        }
    }

    if all_rows.is_empty() {
        return None;
    }

    let row_count = all_rows.len() as f32;
    let panel_h = BODY_PAD as f32 + row_count * LINE_STEP as f32 + BODY_PAD as f32;
    let panel_y = fy - panel_h - 4.0;

    let mut rows = Vec::new();
    let mut y = panel_y + BODY_PAD as f32;
    for (text, color) in all_rows {
        let runs = vec![(text.len(), color)];
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
