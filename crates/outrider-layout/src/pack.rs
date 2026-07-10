use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};

/// An absolute world rectangle. World units are natural pixels: a leaf
/// page at zoom 1.0 renders at exactly this size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Sizing knobs, passed in by the app so this crate stays independent of
/// the render-side content constants.
#[derive(Debug, Clone, Copy)]
pub struct PackConfig {
    /// Leaf page width (world px).
    pub page_w: f64,
    /// Per-code-line height; leaf h = header + (1+measure)·line_step + bottom_pad.
    pub line_step: f64,
    /// Name-row strip height, reserved at the top of every container too.
    pub header: f64,
    pub bottom_pad: f64,
    /// Space between siblings, both axes; also the container's inner margin.
    pub gap: f64,
    /// Target container width/height ratio for shelf wrapping.
    pub aspect: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackLayout {
    /// Absolute rects for every node; the root sits at (0, 0).
    pub rects: BTreeMap<SymbolId, Rect>,
}

/// Shelf-pack the tree bottom-up (spec §3). Pure and deterministic; a
/// container's internal layout depends only on its own children's sizes,
/// so an edit repacks only its ancestor chain (hierarchical stability).
pub fn pack(tree: &SymbolTree, cfg: &PackConfig) -> PackLayout {
    let mut rel = BTreeMap::new();
    size(&tree.root, cfg, &mut rel);
    let mut rects = BTreeMap::new();
    absolute(&tree.root, 0.0, 0.0, &rel, &mut rects);
    PackLayout { rects }
}

/// Bottom-up size pass: returns (w, h) and records each node's position
/// relative to its parent's origin in `rel` (x, y, w, h). The root's
/// relative position stays (0, 0).
fn size(
    node: &SymbolNode,
    cfg: &PackConfig,
    rel: &mut BTreeMap<SymbolId, (f64, f64, f64, f64)>,
) -> (f64, f64) {
    if node.children.is_empty() {
        let h = cfg.header + (1.0 + node.measure as f64) * cfg.line_step + cfg.bottom_pad;
        rel.insert(node.id.clone(), (0.0, 0.0, cfg.page_w, h));
        return (cfg.page_w, h);
    }
    // Re-derive the ordering invariant locally; never trust input Vec order.
    let mut order: Vec<&SymbolNode> = node.children.iter().collect();
    order.sort_by(|a, b| {
        a.name.as_bytes().cmp(b.name.as_bytes()).then(a.id.ordinal.cmp(&b.id.ordinal))
    });
    let sizes: Vec<(f64, f64)> = order.iter().map(|c| size(c, cfg, rel)).collect();
    let area: f64 = sizes.iter().map(|(w, h)| w * h).sum();
    let widest = sizes.iter().map(|&(w, _)| w).fold(0.0, f64::max);
    let target_w = widest.max((area * cfg.aspect).sqrt());
    let (mut x, mut y, mut shelf_h, mut content_w) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (child, &(w, h)) in order.iter().zip(&sizes) {
        if x > 0.0 && x + w > target_w {
            x = 0.0;
            y += shelf_h + cfg.gap;
            shelf_h = 0.0;
        }
        let e = rel.get_mut(&child.id).expect("child sized above");
        e.0 = cfg.gap + x;
        e.1 = cfg.header + cfg.gap + y;
        shelf_h = shelf_h.max(h);
        content_w = content_w.max(x + w);
        x += w + cfg.gap;
    }
    let wh = (content_w + 2.0 * cfg.gap, cfg.header + y + shelf_h + 2.0 * cfg.gap);
    rel.insert(node.id.clone(), (0.0, 0.0, wh.0, wh.1));
    wh
}

fn absolute(
    node: &SymbolNode,
    ox: f64,
    oy: f64,
    rel: &BTreeMap<SymbolId, (f64, f64, f64, f64)>,
    out: &mut BTreeMap<SymbolId, Rect>,
) {
    let &(rx, ry, w, h) = &rel[&node.id];
    let (x, y) = (ox + rx, oy + ry);
    out.insert(node.id.clone(), Rect { x, y, w, h });
    for c in &node.children {
        absolute(c, x, y, rel, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    fn cfg() -> PackConfig {
        PackConfig {
            page_w: 480.0,
            line_step: 15.6,
            header: 20.8,
            bottom_pad: 6.0,
            gap: 8.0,
            aspect: 1.6,
        }
    }

    fn n(kind: SymbolKind, qp: &str, name: &str, measure: u64, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
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

    /// The worked example: root { a.rs(100), b.rs(40) { f(10), g(1) } }.
    fn worked_example() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                0,
                vec![
                    n(SymbolKind::File, "a.rs", "a.rs", 100, vec![]),
                    n(
                        SymbolKind::File,
                        "b.rs",
                        "b.rs",
                        40,
                        vec![
                            n(SymbolKind::Fn, "b.rs::f", "f", 10, vec![]),
                            n(SymbolKind::Fn, "b.rs::g", "g", 1, vec![]),
                        ],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    fn rect(p: &PackLayout, qp: &str) -> Rect {
        *p.rects
            .iter()
            .find(|(id, _)| id.qualified_path == qp)
            .map(|(_, r)| r)
            .unwrap()
    }

    fn assert_rect(r: Rect, x: f64, y: f64, w: f64, h: f64) {
        close(r.x, x);
        close(r.y, y);
        close(r.w, w);
        close(r.h, h);
    }

    #[test]
    fn worked_example_exact_rects() {
        let p = pack(&worked_example(), &cfg());
        assert_eq!(p.rects.len(), 5);
        // leaf pages: w = page_w, h = header + (1+measure)·line_step + bottom_pad
        assert_rect(rect(&p, "a.rs"), 8.0, 28.8, 480.0, 1602.4);
        // b.rs: f and g don't fit one 480-wide shelf → two shelves
        assert_rect(rect(&p, "b.rs::f"), 504.0, 57.6, 480.0, 198.4);
        assert_rect(rect(&p, "b.rs::g"), 504.0, 264.0, 480.0, 58.0);
        assert_rect(rect(&p, "b.rs"), 496.0, 28.8, 496.0, 301.2);
        // root: a.rs and b.rs share one shelf (984 ≤ target_w ≈ 1212.3)
        assert_rect(rect(&p, ""), 0.0, 0.0, 1000.0, 1639.2);
    }

    #[test]
    fn deterministic() {
        let a = pack(&worked_example(), &cfg());
        let b = pack(&worked_example(), &cfg());
        assert_eq!(a.rects, b.rects);
    }

    #[test]
    fn children_placed_by_name_then_ordinal_never_size() {
        // "zeta" is huge, "alpha" tiny — alpha still comes first.
        let tree = SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                0,
                vec![
                    n(SymbolKind::File, "zeta.rs", "zeta.rs", 5000, vec![]),
                    n(SymbolKind::File, "alpha.rs", "alpha.rs", 1, vec![]),
                ],
            ),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        let (a, z) = (rect(&p, "alpha.rs"), rect(&p, "zeta.rs"));
        // alpha is placed first: top-left of the content area
        close(a.x, 8.0);
        close(a.y, 28.8);
        assert!(z.y > a.y, "zeta wraps below alpha (or right of it)");
    }

    #[test]
    fn sibling_subtree_stable_under_edit() {
        // Grow f (10 → 50 lines): b.rs reflows internally and root resizes,
        // but a.rs — a sibling subtree — keeps its exact position, and g
        // only shifts along y inside b.rs.
        let before = pack(&worked_example(), &cfg());
        let mut edited = worked_example();
        edited.root.children[1].children[0].measure = 50;
        let after = pack(&edited, &cfg());
        assert_eq!(rect(&before, "a.rs"), rect(&after, "a.rs"));
        // f: 480 × 822.4 now; b.rs: 496 × 925.2; g slides down, same x
        assert_rect(rect(&after, "b.rs::f"), 504.0, 57.6, 480.0, 822.4);
        assert_rect(rect(&after, "b.rs"), 496.0, 28.8, 496.0, 925.2);
        let g = rect(&after, "b.rs::g");
        close(g.x, 504.0);
        close(g.y, 888.0);
    }

    #[test]
    fn wide_child_sets_the_floor_for_target_width() {
        // A container whose packed width exceeds √(area·aspect) still fits:
        // target_w = max(widest child, …) — no child is ever split.
        let tree = SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                0,
                vec![n(SymbolKind::File, "one.rs", "one.rs", 1, vec![])],
            ),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        // single 480×58 child: content 480×58 → root 496 × 94.8
        assert_rect(rect(&p, "one.rs"), 8.0, 28.8, 480.0, 58.0);
        assert_rect(rect(&p, ""), 0.0, 0.0, 496.0, 94.8);
    }
}
