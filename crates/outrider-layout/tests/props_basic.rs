mod common;

use common::{g_folder, to_tree};
use outrider_index::SymbolNode;
use outrider_layout::{layout, WorldLayout};
use proptest::prelude::*;

/// Property 4 helper (spec §8.1 #4, strengthened): in every layout, sibling
/// start-order equals (name, ordinal) order — order is a function of names
/// alone, so *no* size permutation can ever change it.
fn assert_sibling_order(node: &SymbolNode, w: &WorldLayout) {
    let mut by_name: Vec<&SymbolNode> = node.children.iter().collect();
    by_name.sort_by(|a, b| {
        a.name
            .as_bytes()
            .cmp(b.name.as_bytes())
            .then(a.id.ordinal.cmp(&b.id.ordinal))
    });
    let starts: Vec<u64> = by_name.iter().map(|c| w.nodes[&c.id].cells.start).collect();
    for pair in starts.windows(2) {
        assert!(pair[0] < pair[1], "sibling starts out of (name, ordinal) order");
    }
    for c in &node.children {
        assert_sibling_order(c, w);
    }
}

/// Property 5 helper (spec §8.1 #5): every child's absolute range lies
/// within its parent's; siblings never overlap.
fn assert_containment(node: &SymbolNode, w: &WorldLayout) {
    let p = &w.nodes[&node.id];
    let p_abs = w.absolute_start(&node.id).unwrap();
    let sub_lo = p_abs * w.ratio as u64;
    let sub_hi = (p_abs + p.cells.len) * w.ratio as u64;

    let mut ranges: Vec<(u64, u64)> = node
        .children
        .iter()
        .map(|c| {
            let abs = w.absolute_start(&c.id).unwrap();
            let len = w.nodes[&c.id].cells.len;
            assert!(abs >= sub_lo && abs + len <= sub_hi, "child escapes parent");
            assert!(len >= 1, "zero-cell node");
            (abs, abs + len)
        })
        .collect();
    ranges.sort();
    for pair in ranges.windows(2) {
        assert!(pair[0].1 <= pair[1].0, "sibling overlap");
    }
    for c in &node.children {
        assert_containment(c, w);
    }
}

proptest! {
    /// Spec §8.1 property 1 (in-process half; cross-process is Task 7).
    #[test]
    fn determinism_byte_identical(g in g_folder()) {
        let t = to_tree(&g);
        let a = layout(&t);
        let b = layout(&t);
        prop_assert_eq!(format!("{:?}", a), format!("{:?}", b));
    }

    /// Spec §8.1 property 4.
    #[test]
    fn stable_ordering_never_by_size(g in g_folder()) {
        let t = to_tree(&g);
        let w = layout(&t);
        assert_sibling_order(&t.root, &w);
    }

    /// Spec §8.1 property 5.
    #[test]
    fn containment_no_overlap(g in g_folder()) {
        let t = to_tree(&g);
        let w = layout(&t);
        assert_containment(&t.root, &w);
    }
}
