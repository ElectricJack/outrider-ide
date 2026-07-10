use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolKind, SymbolNode};

use crate::types::RATIO;

/// Initial spec §6.2 constants: 32 lines/cell at file level, 4 at method
/// level; keyed by kind because files occur at varying depths. Tune during
/// milestone 3.
pub fn lines_per_cell(kind: SymbolKind) -> u64 {
    match kind {
        SymbolKind::Folder | SymbolKind::File => 32,
        _ => 4,
    }
}

pub(crate) fn leaf_cells(measure: u64, kind: SymbolKind) -> u64 {
    std::cmp::max(1, measure.div_ceil(lines_per_cell(kind)))
}

/// Per-child slack: ceil(0.03 · len), in integer math (3% — spec 4d §2;
/// was 15% before the density pass). Per-child (not pooled per parent) so
/// a child's position never depends on its successors. The round-up keeps
/// a minimum 1-cell gap for tiny nodes.
pub(crate) fn gap_cells(len: u64) -> u64 {
    (len * 3).div_ceil(100)
}

/// Post-order measure pass (spec §6.2). Fills `lens` for every node in the
/// subtree; returns this node's length in cells at its own level. Round-up
/// only — one sweep, no convergence iteration.
pub(crate) fn node_cells(node: &SymbolNode, lens: &mut BTreeMap<SymbolId, u64>) -> u64 {
    let len = if node.children.is_empty() {
        leaf_cells(node.measure, node.id.kind)
    } else {
        let total: u64 = node
            .children
            .iter()
            .map(|c| {
                let l = node_cells(c, lens);
                l + gap_cells(l)
            })
            .sum();
        total.div_ceil(RATIO as u64)
    };
    lens.insert(node.id.clone(), len);
    len
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    pub(crate) fn node(
        kind: SymbolKind,
        qp: &str,
        name: &str,
        measure: u64,
        children: Vec<SymbolNode>,
    ) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind,
                qualified_path: qp.into(),
                ordinal: 0,
            },
            name: name.into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    #[test]
    fn gap_is_three_percent_rounded_up() {
        assert_eq!(gap_cells(1), 1); // round-up floor keeps a minimum gap
        assert_eq!(gap_cells(4), 1);
        assert_eq!(gap_cells(7), 1);
        assert_eq!(gap_cells(20), 1);
        assert_eq!(gap_cells(34), 2); // ceil(1.02)
        assert_eq!(gap_cells(100), 3);
    }

    #[test]
    fn leaf_cells_by_kind_with_floor_of_one() {
        assert_eq!(leaf_cells(100, SymbolKind::File), 4); // ceil(100/32)
        assert_eq!(leaf_cells(1, SymbolKind::File), 1);
        assert_eq!(leaf_cells(10, SymbolKind::Fn), 3); // ceil(10/4)
        assert_eq!(leaf_cells(1, SymbolKind::Fn), 1);
        assert_eq!(leaf_cells(0, SymbolKind::Fn), 1); // floor of one
    }

    #[test]
    fn worked_example_measures() {
        // Plan-header worked example.
        let b = node(
            SymbolKind::File,
            "b.rs",
            "b.rs",
            40,
            vec![
                node(SymbolKind::Fn, "b.rs::f", "f", 10, vec![]),
                node(SymbolKind::Fn, "b.rs::g", "g", 1, vec![]),
            ],
        );
        let root = node(
            SymbolKind::Folder,
            "",
            "",
            140,
            vec![node(SymbolKind::File, "a.rs", "a.rs", 100, vec![]), b],
        );
        let mut lens = BTreeMap::new();
        let root_len = node_cells(&root, &mut lens);
        assert_eq!(root_len, 1);
        let get = |qp: &str| {
            lens.iter()
                .find(|(id, _)| id.qualified_path == qp)
                .map(|(_, l)| *l)
                .unwrap()
        };
        assert_eq!(get("b.rs::f"), 3);
        assert_eq!(get("b.rs::g"), 1);
        assert_eq!(get("b.rs"), 1); // ceil(((3+1)+(1+1))/8)
        assert_eq!(get("a.rs"), 4);
        assert_eq!(get(""), 1); // ceil(((4+1)+(1+1))/8)
        assert_eq!(lens.len(), 5);
    }
}
