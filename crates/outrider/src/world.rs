use outrider_layout::RATIO;

pub const CELL_ASPECT: f64 = 3.0;
pub const MERGE_PX: f64 = 4.0;
pub const LABEL_PX: f64 = 20.0;
pub const CARD_PX: f64 = 80.0;

pub const MAX_COLUMN_PX: f64 = 400.0;
pub const GUTTER_PX: f64 = 24.0;
/// Columns narrower than this render fill + border only (forced Dot).
pub const LABEL_MIN_W: f64 = 60.0;
/// Depths beyond this are sub-merge at any legal zoom (max zoom = vh·8^15).
pub const MAX_DEPTH: usize = 24;

/// 8^-depth: the size scale of level-`depth` cells relative to level 0.
pub fn column_scale(depth: u8) -> f64 {
    (RATIO as f64).powi(-(depth as i32))
}

/// Pixel height of one level-`depth` cell at `zoom` (px per world unit).
pub fn cell_px_height(depth: u8, zoom: f64) -> f64 {
    zoom * column_scale(depth)
}

/// Peaked width profile (screen-space-columns spec §3): rises as
/// CELL_ASPECT·h until the peak (h = MAX/CELL_ASPECT, cells comfortably in
/// Card rung), then decays as 1/h — 8× per zoom octave on both sides, so the
/// profile is self-similar — floored at the gutter for zoomed-past ancestors.
pub fn column_px_width(h: f64) -> f64 {
    let peak_h = MAX_COLUMN_PX / CELL_ASPECT;
    if h <= peak_h {
        CELL_ASPECT * h
    } else {
        (MAX_COLUMN_PX * peak_h / h).max(GUTTER_PX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColPx {
    pub x: f64,
    pub w: f64,
}

/// Per-frame column table: x is the prefix sum of shallower widths — the
/// stack is left-anchored at x = 0 and fully determined by zoom.
pub fn column_table(zoom: f64) -> Vec<ColPx> {
    let mut out = Vec::with_capacity(MAX_DEPTH + 1);
    let mut x = 0.0;
    for d in 0..=MAX_DEPTH {
        let w = column_px_width(cell_px_height(d as u8, zoom));
        out.push(ColPx { x, w });
        x += w;
    }
    out
}

/// Rung by pixel height, downgraded to Dot when the column is too narrow
/// for text (gutter strips). Heights below MERGE_PX merge into the parent.
pub fn rung_for(px_h: f64, px_w: f64) -> Option<Rung> {
    let by_height = if px_h < MERGE_PX {
        return None;
    } else if px_h < LABEL_PX {
        Rung::Dot
    } else if px_h < CARD_PX {
        Rung::Label
    } else {
        Rung::Card
    };
    Some(if px_w < LABEL_MIN_W { Rung::Dot } else { by_height })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    Dot,
    Label,
    Card,
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
    let cols = column_table(camera.zoom);
    let mut out = Vec::new();
    walk(&tree.root, layout, camera, &cols, vw, vh, 0.0, &mut out);
    out
}

#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    node: &'a SymbolNode,
    layout: &WorldLayout,
    camera: &Camera,
    cols: &[ColPx],
    vw: f64,
    vh: f64,
    parent_abs: f64,
    out: &mut Vec<DrawItem<'a>>,
) {
    let Some(nl) = layout.nodes.get(&node.id) else { return };
    let depth = nl.cells.level;
    let abs = parent_abs * outrider_layout::RATIO as f64 + nl.cells.start as f64;
    debug_assert!(abs < 2f64.powi(53), "cell address exceeds exact f64 range");
    let s = column_scale(depth);
    let px_y = camera.world_to_screen_y(abs * s, vh);
    let px_h = nl.cells.len as f64 * s * camera.zoom;
    let Some(&ColPx { x: px_x, w: px_w }) = cols.get(depth as usize) else { return };

    // Below the merge threshold: this node merges into its parent's tile,
    // and children (8x smaller) are below it too. Stop.
    let Some(rung) = rung_for(px_h, px_w) else { return };
    // Children's y-ranges are contained in the parent's: off-screen y prunes the subtree.
    if px_y > vh || px_y + px_h < 0.0 {
        return;
    }
    // Deeper columns are further right: past the right edge prunes the subtree.
    if px_x > vw {
        return;
    }
    // Zoomed-past ancestors have enormous pixel heights; clip to the viewport
    // (2px slack keeps their borders off-screen) before f32 ever sees them.
    // The rung above is chosen from the UNclipped height.
    let y0 = px_y.max(-2.0);
    let y1 = (px_y + px_h).min(vh + 2.0);
    out.push(DrawItem { node, px: PxRect { x: px_x, y: y0, w: px_w, h: y1 - y0 }, rung });
    for child in &node.children {
        walk(child, layout, camera, cols, vw, vh, abs, out);
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
    fn culling_offscreen_y_is_empty() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        let mut cam = Camera::frame(1.0, 600.0);
        cam.center_y = 100.0; // world is y ∈ [0,1]
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        assert!(items.is_empty());
    }

    #[test]
    fn width_profile_rising_side() {
        // w = 3h up to the peak
        close(column_px_width(10.0), 30.0);
        close(column_px_width(100.0), 300.0);
        let peak_h = MAX_COLUMN_PX / CELL_ASPECT; // ≈ 133.33 px cells
        close(column_px_width(peak_h), MAX_COLUMN_PX);
    }

    #[test]
    fn width_profile_decay_side() {
        // past the peak, w = MAX² / (3h): halves when h doubles
        let peak_h = MAX_COLUMN_PX / CELL_ASPECT;
        close(column_px_width(2.0 * peak_h), MAX_COLUMN_PX / 2.0);
        close(column_px_width(8.0 * peak_h), MAX_COLUMN_PX / 8.0);
        // gutter floor is reached exactly and held forever
        let floor_h = MAX_COLUMN_PX * peak_h / GUTTER_PX;
        close(column_px_width(floor_h), GUTTER_PX);
        close(column_px_width(floor_h * 100.0), GUTTER_PX);
    }

    #[test]
    fn width_profile_self_similar() {
        // spec §3: the table at zoom 8z equals the table at z shifted one depth right
        for &z in &[10.0, 127.0, 1000.0, 54321.0] {
            let t1 = column_table(z);
            let t8 = column_table(8.0 * z);
            for d in 0..MAX_DEPTH {
                close(t8[d + 1].w, t1[d].w);
            }
        }
    }

    #[test]
    fn column_table_prefix_sums_and_bound() {
        for &z in &[1.0, 571.4285714285714, 36571.42857142857, 1e12] {
            let t = column_table(z);
            assert_eq!(t.len(), MAX_DEPTH + 1);
            close(t[0].x, 0.0);
            for d in 1..t.len() {
                close(t[d].x, t[d - 1].x + t[d - 1].w);
                assert!(t[d - 1].w > 0.0, "widths must be positive");
                assert!(t[d].x >= t[d - 1].x, "x must be non-decreasing");
            }
            // spec §3: total stack width is bounded at any zoom
            let total = t[MAX_DEPTH].x + t[MAX_DEPTH].w;
            assert!(total < 1600.0, "total {total} not bounded at zoom {z}");
        }
    }

    #[test]
    fn rung_for_thresholds_and_downgrade() {
        // height thresholds (wide column: no downgrade)
        assert_eq!(rung_for(3.9, 400.0), None);
        assert_eq!(rung_for(4.0, 400.0), Some(Rung::Dot));
        assert_eq!(rung_for(19.9, 400.0), Some(Rung::Dot));
        assert_eq!(rung_for(20.0, 400.0), Some(Rung::Label));
        assert_eq!(rung_for(79.9, 400.0), Some(Rung::Label));
        assert_eq!(rung_for(80.0, 400.0), Some(Rung::Card));
        // narrow columns are forced to Dot regardless of height (gutters)
        assert_eq!(rung_for(100_000.0, 59.9), Some(Rung::Dot));
        assert_eq!(rung_for(100_000.0, 60.0), Some(Rung::Card));
        // the merge rule wins over everything
        assert_eq!(rung_for(3.9, 24.0), None);
    }

    #[test]
    fn worked_example_bands() {
        // y-composition unchanged from the world-space model:
        // b.rs::g — depth 2, abs cell 44, len 1 → y = 44/64, h = 1/64
        let s = column_scale(2);
        close(44.0 * s, 0.6875);
        close(1.0 * s, 0.015625);
    }

    #[test]
    fn culling_home_view() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // Home: root band (world height 1.0) fits 600px with 5% margin → zoom = 4000/7
        let cam = Camera::frame(1.0, 600.0);
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        // home zoom is now height-only (571.4) — even g (8.9px) is above merge
        assert_eq!(names, vec!["", "a.rs", "b.rs", "f", "g"]);
        let rungs: Vec<Rung> = items.iter().map(|i| i.rung).collect();
        // heights: root 571.4, a.rs 285.7, b.rs 71.4, f 26.8, g 8.9
        // widths:  d0 93.33 (decay side), d1 214.29, d2 26.79 (< LABEL_MIN_W → Dot)
        assert_eq!(rungs, vec![Rung::Card, Rung::Card, Rung::Label, Rung::Dot, Rung::Dot]);
        // hand-computed px rect for f (zoom = 4000/7):
        // x = w0+w1 = 280/3 + 1500/7, y = 0.125·zoom + 300, w = 3·zoom/64, h = 3·zoom/64
        let f = &items[3].px;
        assert!((f.x - 307.6190476).abs() < 1e-6, "{}", f.x);
        assert!((f.y - 371.4285714).abs() < 1e-6, "{}", f.y);
        assert!((f.w - 26.7857143).abs() < 1e-6, "{}", f.w);
        assert!((f.h - 26.7857143).abs() < 1e-6, "{}", f.h);
    }

    #[test]
    fn culling_x_prune_stops_recursion() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        let cam = Camera::frame(1.0, 600.0);
        // viewport only 80px wide: x1 = 93.33 > 80 → depth ≥ 1 pruned
        let items = visible_nodes(&tree, &layout, &cam, 80.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec![""]);
    }

    #[test]
    fn gutters_are_clipped_narrow_dots() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // two octaves past home (zoom·64), centered on g: root and b.rs are
        // zoomed-past ancestors → 24px gutter strips, clipped to the viewport
        let cam = Camera { center_y: 0.6875, zoom: 256000.0 / 7.0 };
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        // a.rs and f are entirely above the viewport (y-pruned)
        assert_eq!(names, vec!["", "b.rs", "g"]);
        let rungs: Vec<Rung> = items.iter().map(|i| i.rung).collect();
        assert_eq!(rungs, vec![Rung::Dot, Rung::Dot, Rung::Card]);
        // root gutter: x=0, w=24, y clipped to [-2, 602]
        let root = &items[0].px;
        assert!((root.x - 0.0).abs() < 1e-6 && (root.w - 24.0).abs() < 1e-6);
        assert!((root.y - -2.0).abs() < 1e-6 && (root.h - 604.0).abs() < 1e-6);
        // g: x = 24+24 = 48, w = 93.33 (decay side), y = 300, h clipped to 302
        let g = &items[2].px;
        assert!((g.x - 48.0).abs() < 1e-6, "{}", g.x);
        assert!((g.w - 93.3333333).abs() < 1e-6, "{}", g.w);
        assert!((g.y - 300.0).abs() < 1e-6, "{}", g.y);
        assert!((g.h - 302.0).abs() < 1e-6, "{}", g.h);
        // nothing exceeds the clipped viewport band
        for i in &items {
            assert!(i.px.y >= -2.0 - 1e-9 && i.px.y + i.px.h <= 602.0 + 1e-9);
        }
    }
}
