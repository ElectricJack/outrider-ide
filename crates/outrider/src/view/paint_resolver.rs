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

    pub fn light(&self, id: &SymbolId) -> f32 {
        self.resolved.mask.light(id)
    }

    pub fn notes(&self, id: &SymbolId) -> &[ResolvedNote] {
        self.resolved.notes.get(id)
    }
}
