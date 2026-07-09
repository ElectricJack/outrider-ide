use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};

use crate::measure::{gap_cells, node_cells};
use crate::types::{CellRange, NodeLayout, WorldLayout, RATIO};

/// Map a `SymbolTree` to a `WorldLayout` (spec §6). Pure function:
/// deterministic, no I/O, integer-only cell math.
pub fn layout(tree: &SymbolTree) -> WorldLayout {
    let mut lens = BTreeMap::new();
    node_cells(&tree.root, &mut lens);
    let mut nodes = BTreeMap::new();
    arrange(&tree.root, None, 0, 0, &lens, &mut nodes);
    debug_assert_eq!(
        nodes.len(),
        count(&tree.root),
        "duplicate SymbolId in input tree (index must run dedupe_ids)"
    );
    WorldLayout { nodes, ratio: RATIO }
}

/// Pre-order arrange pass (spec §6.3): children in (name, ordinal) order,
/// each followed by its own gap; round-up remainder accumulates at the end.
/// `start` is relative to the parent's range.
fn arrange(
    node: &SymbolNode,
    parent: Option<&SymbolId>,
    level: u8,
    start: u64,
    lens: &BTreeMap<SymbolId, u64>,
    out: &mut BTreeMap<SymbolId, NodeLayout>,
) {
    let len = lens[&node.id];
    out.insert(
        node.id.clone(),
        NodeLayout {
            id: node.id.clone(),
            parent: parent.cloned(),
            cells: CellRange { level, start, len },
        },
    );
    // Re-derive the ordering invariant locally; never trust input Vec order
    // for placement (plan decision #5).
    let mut order: Vec<&SymbolNode> = node.children.iter().collect();
    order.sort_by(|a, b| {
        a.name
            .as_bytes()
            .cmp(b.name.as_bytes())
            .then(a.id.ordinal.cmp(&b.id.ordinal))
    });
    let mut cursor = 0u64;
    for child in order {
        let child_len = lens[&child.id];
        arrange(child, Some(&node.id), level + 1, cursor, lens, out);
        cursor += child_len + gap_cells(child_len);
    }
    debug_assert!(
        cursor <= len * RATIO as u64,
        "children overflow parent allocation"
    );
}

fn count(node: &SymbolNode) -> usize {
    1 + node.children.iter().map(count).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::SymbolKind;

    fn node(
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
    fn worked_example_layout_exact() {
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
        let tree = SymbolTree {
            root,
            repo_root: "/ex".into(),
        };
        let w = layout(&tree);
        assert_eq!(w.ratio, RATIO);
        assert_eq!(w.nodes.len(), 5);

        let get = |qp: &str| {
            w.nodes
                .iter()
                .find(|(id, _)| id.qualified_path == qp)
                .map(|(_, n)| n)
                .unwrap()
        };
        let root_l = get("");
        assert_eq!(root_l.parent, None);
        assert_eq!(root_l.cells, CellRange { level: 0, start: 0, len: 1 });

        assert_eq!(get("a.rs").cells, CellRange { level: 1, start: 0, len: 4 });
        assert_eq!(get("b.rs").cells, CellRange { level: 1, start: 5, len: 1 });
        assert_eq!(get("b.rs").parent.as_ref().unwrap().qualified_path, "");

        assert_eq!(get("b.rs::f").cells, CellRange { level: 2, start: 0, len: 3 });
        assert_eq!(get("b.rs::g").cells, CellRange { level: 2, start: 4, len: 1 });

        // absolute composition (worked example)
        let g_id = w.nodes.keys().find(|id| id.qualified_path == "b.rs::g").unwrap().clone();
        assert_eq!(w.absolute_start(&g_id), Some(44));
    }

    #[test]
    fn children_placed_by_name_then_ordinal_never_size() {
        // "zeta" is huge, "alpha" tiny — alpha still comes first.
        let root = node(
            SymbolKind::Folder,
            "",
            "",
            0,
            vec![
                node(SymbolKind::File, "zeta.rs", "zeta.rs", 5000, vec![]),
                node(SymbolKind::File, "alpha.rs", "alpha.rs", 1, vec![]),
            ],
        );
        let tree = SymbolTree {
            root,
            repo_root: "/ex".into(),
        };
        let w = layout(&tree);
        let start = |qp: &str| {
            w.nodes
                .iter()
                .find(|(id, _)| id.qualified_path == qp)
                .map(|(_, n)| n.cells.start)
                .unwrap()
        };
        assert!(start("alpha.rs") < start("zeta.rs"));
        assert_eq!(start("alpha.rs"), 0);
    }
}
