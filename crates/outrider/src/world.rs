use outrider_layout::RATIO;

pub const CELL_ASPECT: f64 = 3.0;
pub const MERGE_PX: f64 = 4.0;
pub const LABEL_PX: f64 = 20.0;
pub const CARD_PX: f64 = 80.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 8^-depth: the size scale of level-`depth` cells relative to level 0.
pub fn column_scale(depth: u8) -> f64 {
    (RATIO as f64).powi(-(depth as i32))
}

/// X_d = CELL_ASPECT * (1 - 8^-d) * 8/7 — where the depth-d column begins.
pub fn column_x(depth: u8) -> f64 {
    let r = RATIO as f64;
    CELL_ASPECT * (1.0 - column_scale(depth)) * r / (r - 1.0)
}

/// Total world width: the columns converge to CELL_ASPECT * 8/7.
pub fn world_width() -> f64 {
    let r = RATIO as f64;
    CELL_ASPECT * r / (r - 1.0)
}

pub fn node_world_rect(depth: u8, abs_start: f64, len: u64) -> WorldRect {
    let s = column_scale(depth);
    WorldRect {
        x: column_x(depth),
        y: abs_start * s,
        w: CELL_ASPECT * s,
        h: len as f64 * s,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    Dot,
    Label,
    Card,
}

pub fn rung_for_px_height(h: f64) -> Option<Rung> {
    if h < MERGE_PX {
        None
    } else if h < LABEL_PX {
        Some(Rung::Dot)
    } else if h < CARD_PX {
        Some(Rung::Label)
    } else {
        Some(Rung::Card)
    }
}

use outrider_index::{SymbolNode, SymbolTree};
use outrider_layout::WorldLayout;

use crate::camera::Camera;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PxRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug)]
pub struct DrawItem<'a> {
    pub node: &'a SymbolNode,
    pub px: PxRect,
    pub rung: Rung,
}

/// Cull the tree against the viewport and the 4px merge rule.
/// Returns visible nodes in pre-order (parents before children = painter's order).
pub fn visible_nodes<'a>(
    tree: &'a SymbolTree,
    layout: &WorldLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
) -> Vec<DrawItem<'a>> {
    let mut out = Vec::new();
    walk(&tree.root, layout, camera, vw, vh, 0.0, &mut out);
    out
}

fn walk<'a>(
    node: &'a SymbolNode,
    layout: &WorldLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
    parent_abs: f64,
    out: &mut Vec<DrawItem<'a>>,
) {
    let Some(nl) = layout.nodes.get(&node.id) else { return };
    let depth = nl.cells.level;
    let abs = parent_abs * outrider_layout::RATIO as f64 + nl.cells.start as f64;
    debug_assert!(abs < 2f64.powi(53), "cell address exceeds exact f64 range");
    let rect = node_world_rect(depth, abs, nl.cells.len);
    let (px_x, px_y) = camera.world_to_screen(rect.x, rect.y, vw, vh);
    let px_w = rect.w * camera.zoom;
    let px_h = rect.h * camera.zoom;

    // Below the merge threshold: this node merges into its parent's tile,
    // and children (8x smaller) are below it too. Stop.
    let Some(rung) = rung_for_px_height(px_h) else { return };
    // Children's y-ranges are contained in the parent's: off-screen y prunes the subtree.
    if px_y > vh || px_y + px_h < 0.0 {
        return;
    }
    // Deeper columns are further right: past the right edge prunes the subtree.
    if px_x > vw {
        return;
    }
    // The node's own column may be off-screen left while children are visible:
    // skip drawing but keep recursing.
    if px_x + px_w > 0.0 {
        out.push(DrawItem { node, px: PxRect { x: px_x, y: px_y, w: px_w, h: px_h }, rung });
    }
    for child in &node.children {
        walk(child, layout, camera, vw, vh, abs, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    fn n(kind: SymbolKind, qp: &str, name: &str, measure: u64, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
            name: name.into(),
            byte_range: None,
            measure,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    /// The Phase 2 worked example: root{0,0,1}; a.rs{1,0,4}; b.rs{1,5,1}; f{2,0,3}; g{2,4,1}.
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
                        10,
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

    #[test]
    fn culling_home_view_prunes_submerge_nodes() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        let cam = Camera::frame(world_width(), 1.0, 800.0, 600.0); // zoom ≈ 222.22
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        // g is ~3.47px tall at home zoom -> merged into b.rs
        assert_eq!(names, vec!["", "a.rs", "b.rs", "f"]);
        let rungs: Vec<Rung> = items.iter().map(|i| i.rung).collect();
        assert_eq!(rungs, vec![Rung::Card, Rung::Card, Rung::Label, Rung::Dot]);
    }

    #[test]
    fn culling_offscreen_y_is_empty() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        let mut cam = Camera::frame(world_width(), 1.0, 800.0, 600.0);
        cam.center_y = 100.0; // world is y ∈ [0,1]
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        assert!(items.is_empty());
    }

    #[test]
    fn culling_recurses_past_offscreen_left_parent() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // Zoomed onto b.rs's children: root column entirely off-screen left,
        // a.rs off-screen top, but b.rs/f/g visible.
        let cam = Camera { center_x: 3.4, center_y: 0.69, zoom: 2000.0 };
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["b.rs", "f", "g"]);
        // hand-computed rect for b.rs: x=(3.0-3.4)*2000+400, y=(0.625-0.69)*2000+300, w=0.375*2000, h=0.125*2000
        let b = &items[0].px;
        assert!((b.x - -400.0).abs() < 1e-6, "{}", b.x);
        assert!((b.y - 170.0).abs() < 1e-6, "{}", b.y);
        assert!((b.w - 750.0).abs() < 1e-6, "{}", b.w);
        assert!((b.h - 250.0).abs() < 1e-6, "{}", b.h);
    }

    #[test]
    fn column_geometry() {
        close(column_scale(0), 1.0);
        close(column_scale(1), 0.125);
        close(column_scale(2), 0.015625);
        close(column_x(0), 0.0);
        close(column_x(1), 3.0);
        close(column_x(2), 3.375);
        close(world_width(), 24.0 / 7.0);
    }

    #[test]
    fn worked_example_rects() {
        // root {0,0,1}
        let r = node_world_rect(0, 0.0, 1);
        close(r.x, 0.0);
        close(r.y, 0.0);
        close(r.w, 3.0);
        close(r.h, 1.0);
        // b.rs::g — depth 2, abs cell 44, len 1 (Phase 2 worked example)
        let g = node_world_rect(2, 44.0, 1);
        close(g.x, 3.375);
        close(g.y, 0.6875);
        close(g.w, 0.046875);
        close(g.h, 0.015625);
    }

    #[test]
    fn rung_thresholds() {
        assert_eq!(rung_for_px_height(3.9), None);
        assert_eq!(rung_for_px_height(4.0), Some(Rung::Dot));
        assert_eq!(rung_for_px_height(19.9), Some(Rung::Dot));
        assert_eq!(rung_for_px_height(20.0), Some(Rung::Label));
        assert_eq!(rung_for_px_height(79.9), Some(Rung::Label));
        assert_eq!(rung_for_px_height(80.0), Some(Rung::Card));
        assert_eq!(rung_for_px_height(100_000.0), Some(Rung::Card));
    }
}
