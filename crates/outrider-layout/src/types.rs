use std::collections::BTreeMap;

use outrider_index::SymbolId;

/// Subdivision ratio: a parent's n level-d cells subdivide into n·r
/// level-(d+1) cells (spec §6.1).
pub const RATIO: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellRange {
    /// Structural depth: root = 0.
    pub level: u8,
    /// Offset in level-`level` cells, relative to the parent's range
    /// (hierarchical address, spec §6.3).
    pub start: u64,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLayout {
    pub id: SymbolId,
    pub parent: Option<SymbolId>,
    pub cells: CellRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldLayout {
    pub nodes: BTreeMap<SymbolId, NodeLayout>,
    pub ratio: u32,
}

impl WorldLayout {
    /// Absolute level-d cell index of a node's first cell, composed from the
    /// ancestor chain: abs(child) = abs(parent) · r + child.start.
    /// (Render code will compose only near-camera ancestors instead — this
    /// full composition is for tests and tools.)
    pub fn absolute_start(&self, id: &SymbolId) -> Option<u64> {
        let node = self.nodes.get(id)?;
        match &node.parent {
            None => Some(node.cells.start),
            Some(p) => Some(self.absolute_start(p)? * self.ratio as u64 + node.cells.start),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind};

    fn id(kind: SymbolKind, qp: &str) -> SymbolId {
        SymbolId {
            kind,
            qualified_path: qp.into(),
            ordinal: 0,
        }
    }

    #[test]
    fn absolute_start_composes_ancestor_chain() {
        // Worked example from the plan header: root {0,0,1}, b.rs {1,5,1}, f {2,0,3}
        let root = id(SymbolKind::Folder, "");
        let b = id(SymbolKind::File, "b.rs");
        let f = id(SymbolKind::Fn, "b.rs::f");
        let mut nodes = BTreeMap::new();
        nodes.insert(
            root.clone(),
            NodeLayout {
                id: root.clone(),
                parent: None,
                cells: CellRange { level: 0, start: 0, len: 1 },
            },
        );
        nodes.insert(
            b.clone(),
            NodeLayout {
                id: b.clone(),
                parent: Some(root.clone()),
                cells: CellRange { level: 1, start: 5, len: 1 },
            },
        );
        nodes.insert(
            f.clone(),
            NodeLayout {
                id: f.clone(),
                parent: Some(b.clone()),
                cells: CellRange { level: 2, start: 0, len: 3 },
            },
        );
        let world = WorldLayout { nodes, ratio: RATIO };
        assert_eq!(world.absolute_start(&root), Some(0));
        assert_eq!(world.absolute_start(&b), Some(5));
        assert_eq!(world.absolute_start(&f), Some(40));
        assert_eq!(world.absolute_start(&id(SymbolKind::Fn, "missing")), None);
    }
}
