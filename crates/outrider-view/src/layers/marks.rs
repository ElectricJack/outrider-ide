//! Marks layer resolution.

use std::collections::{BTreeMap, HashMap, HashSet};

use outrider_index::SymbolId;

use crate::deps::Deps;
use crate::set::ResolvedSet;
use crate::spec::{MarkKind, MarkStyle, MarkTarget, MarksSpec, SetRef};

/// A single resolved mark on a symbol.
#[derive(Debug, Clone)]
pub struct Mark {
    pub kind: MarkKind,
    pub style: MarkStyle,
    pub label: Option<String>,
    pub basis: Option<String>,
}

/// The merged result of all marks layers.
#[derive(Debug, Clone, Default)]
pub struct MarkTable {
    /// Marks per symbol (for corner glyphs and ring decisions).
    pub by_symbol: HashMap<SymbolId, Vec<Mark>>,
    /// Focus ring set.
    pub focus_ring: HashSet<SymbolId>,
    /// Neighbor ring set.
    pub neighbors: HashSet<SymbolId>,
    /// Selection ring set.
    pub selection: HashSet<SymbolId>,
    pub deps: Deps,
}

impl MarkTable {
    pub fn is_focus_ring(&self, id: &SymbolId) -> bool {
        self.focus_ring.contains(id)
    }

    pub fn is_neighbor(&self, id: &SymbolId) -> bool {
        self.neighbors.contains(id)
    }

    pub fn is_selection(&self, id: &SymbolId) -> bool {
        self.selection.contains(id)
    }
}

fn mark_style(kind: MarkKind) -> MarkStyle {
    match kind {
        MarkKind::FocusRing | MarkKind::Neighbor | MarkKind::Selection => MarkStyle::Nav,
        MarkKind::Hotspot | MarkKind::Cycle | MarkKind::LayeringViolation | MarkKind::Custom => {
            MarkStyle::Structural
        }
        MarkKind::AgentFlag => MarkStyle::Agent,
    }
}

/// Resolve a single marks layer and merge into the table.
pub fn resolve_marks(
    spec: &MarksSpec,
    named_sets: &BTreeMap<String, ResolvedSet>,
    warnings: &mut Vec<String>,
    table: &mut MarkTable,
) {
    let resolved_set = match &spec.on {
        MarkTarget::Set(set_ref) => match set_ref {
            SetRef::Name(name) => {
                if let Some(set) = named_sets.get(name) {
                    set.clone()
                } else {
                    warnings.push(format!("marks: unknown set '{name}'"));
                    return;
                }
            }
            SetRef::Inline(_) => {
                warnings
                    .push("marks: inline set expressions in MarkTarget not resolved here".to_string());
                return;
            }
        },
    };

    let style = mark_style(spec.kind);
    let mark = Mark {
        kind: spec.kind,
        style,
        label: spec.label.clone(),
        basis: spec.basis.clone(),
    };

    for id in &resolved_set.ids {
        // Add to nav ring sets
        match spec.kind {
            MarkKind::FocusRing => {
                table.focus_ring.insert(id.clone());
            }
            MarkKind::Neighbor => {
                table.neighbors.insert(id.clone());
            }
            MarkKind::Selection => {
                table.selection.insert(id.clone());
            }
            _ => {}
        }
        table.by_symbol.entry(id.clone()).or_insert_with(Vec::new).push(mark.clone());
    }

    table.deps = table.deps.union(resolved_set.deps);
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::SymbolKind;

    #[test]
    fn focus_ring_mark_populates_focus_ring_set() {
        let id = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "a.rs".to_string(),
            ordinal: 0,
        };
        let mut named = BTreeMap::new();
        let mut set = ResolvedSet::default();
        set.ids.insert(id.clone());
        set.deps = Deps::FOCUS;
        named.insert("focusSet".to_string(), set);

        let spec = MarksSpec {
            on: MarkTarget::Set(SetRef::Name("focusSet".to_string())),
            kind: MarkKind::FocusRing,
            label: None,
            basis: None,
        };
        let mut warnings = Vec::new();
        let mut table = MarkTable::default();
        resolve_marks(&spec, &named, &mut warnings, &mut table);

        assert!(table.is_focus_ring(&id));
        assert!(warnings.is_empty());
        assert!(table.deps.intersects(Deps::FOCUS));
    }
}
