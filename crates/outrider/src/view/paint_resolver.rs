use outrider_index::SymbolId;
use outrider_view::layers::notes::ResolvedNote;
use outrider_view::ResolvedView;

use crate::theme;

pub(crate) struct PaintOverrides<'a> {
    resolved: &'a ResolvedView,
}

impl<'a> PaintOverrides<'a> {
    pub fn new(resolved: &'a ResolvedView) -> Self {
        PaintOverrides { resolved }
    }

    pub fn fill(&self, id: &SymbolId) -> Option<u32> {
        let fill = self.resolved.fill.as_ref()?;
        let &value = fill.values.get(id)?;
        Some(theme::churn_heat(value))
    }

    pub fn opacity(&self, id: &SymbolId) -> Option<f32> {
        let fill = self.resolved.opacity.as_ref()?;
        let &value = fill.values.get(id)?;
        Some(value.clamp(0.0, 1.0))
    }

    pub fn stripe(&self, id: &SymbolId) -> Option<u32> {
        let fill = self.resolved.stripe.as_ref()?;
        let &value = fill.values.get(id)?;
        if value <= 0.0 {
            return None;
        }
        Some(theme::churn_heat(value))
    }

    pub fn is_focus_ring(&self, id: &SymbolId) -> bool {
        self.resolved.marks.is_focus_ring(id)
    }

    pub fn is_neighbor(&self, id: &SymbolId) -> bool {
        self.resolved.marks.is_neighbor(id)
    }

    /// First structural/agent mark on a symbol, as a badge: custom marks
    /// use their label; built-in kinds use a short name.
    pub fn badge(&self, id: &SymbolId) -> Option<crate::paint_model::MarkBadge> {
        use outrider_view::spec::MarkKind;
        let marks = self.resolved.marks.by_symbol.get(id)?;
        let m = marks.iter().find(|m| {
            matches!(
                m.kind,
                MarkKind::Hotspot
                    | MarkKind::Cycle
                    | MarkKind::LayeringViolation
                    | MarkKind::AgentFlag
                    | MarkKind::Custom
            )
        })?;
        let (text, color) = match m.kind {
            MarkKind::Custom => (
                m.label.clone().unwrap_or_else(|| "mark".into()),
                crate::theme::EDGE_COCHANGE,
            ),
            MarkKind::Hotspot => ("hot".to_string(), crate::theme::FILL_HOT),
            MarkKind::Cycle => ("cycle".to_string(), crate::theme::EDGE_VIOLATION),
            MarkKind::LayeringViolation => ("layering".to_string(), crate::theme::EDGE_VIOLATION),
            MarkKind::AgentFlag => (
                m.label.clone().unwrap_or_else(|| "flag".into()),
                crate::theme::NARRATION_TEXT,
            ),
            _ => unreachable!(),
        };
        Some(crate::paint_model::MarkBadge { text, color })
    }

    pub fn light(&self, id: &SymbolId) -> f32 {
        self.resolved.mask.light(id)
    }

    pub fn notes(&self, id: &SymbolId) -> &[ResolvedNote] {
        self.resolved.notes.get(id)
    }
}
