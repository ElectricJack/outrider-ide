use outrider_layout::RATIO;

pub const MERGE_PX: f64 = 4.0;
pub const LABEL_PX: f64 = 20.0;
pub const CARD_PX: f64 = 80.0;
pub const DETAIL_PX: f64 = 250.0;
pub const FULL_PX: f64 = 700.0;
/// Full is useless in a sliver column; below this width it downgrades to Detail.
pub const CODE_MIN_W: f64 = 300.0;

/// The normalized column stack sums to this fraction of the viewport width
/// at every zoom, eliminating the mid-octave total-width dip of the raw
/// peaked profile.
pub const STACK_FRACTION: f64 = 0.95;
/// Cell height (px) at which a column carries the most weight; the widest
/// column is the one whose cells are nearest this height.
pub const PEAK_CELL_PX: f64 = 200.0;
/// Raw width ratio between adjacent depths. Cell heights differ by RATIO
/// (8x) per depth; widths differ by only this much, so the peak's neighbor
/// columns stay comparable instead of vanishing.
pub const WIDTH_RATIO: f64 = 4.0;
pub const GUTTER_PX: f64 = 24.0;
/// Columns narrower than this render fill + border only (forced Dot).
pub const LABEL_MIN_W: f64 = 60.0;
/// Depths beyond this are sub-merge at any legal zoom (max zoom = vh·8^15).
pub const MAX_DEPTH: usize = 24;
/// Horizontal nesting margin: each ancestor's box extends this much
/// further right than its children's boxes.
pub const NEST_PAD: f64 = 6.0;

/// 8^-depth: the size scale of level-`depth` cells relative to level 0.
pub fn column_scale(depth: u8) -> f64 {
    (RATIO as f64).powi(-(depth as i32))
}

/// Pixel height of one level-`depth` cell at `zoom` (px per world unit).
pub fn cell_px_height(depth: u8, zoom: f64) -> f64 {
    zoom * column_scale(depth)
}

/// Exponent turning the 8x-per-depth cell-height falloff into the
/// WIDTH_RATIO-per-depth width falloff: h^alpha with alpha = log_8(4) = 2/3.
fn width_alpha() -> f64 {
    WIDTH_RATIO.ln() / (RATIO as f64).ln()
}

/// Dimensionless column weight (screen-space-columns spec §3): peaked at
/// h = PEAK_CELL_PX and falling off WIDTH_RATIO× per depth step on both
/// sides, so the profile is self-similar under zoom.
pub fn column_weight(h: f64) -> f64 {
    let a = width_alpha();
    if h <= PEAK_CELL_PX {
        (h / PEAK_CELL_PX).powf(a)
    } else {
        (PEAK_CELL_PX / h).powf(a)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColPx {
    pub x: f64,
    pub w: f64,
}

/// Per-frame column table over depths 0..=max_depth: weights are normalized
/// so the stack sums to STACK_FRACTION·vw at every zoom, with zoomed-past
/// (decay-side) columns floored at GUTTER_PX. Flooring one column re-scales
/// the rest, which may floor more — iterate until stable (waterfill; the
/// re-scale is continuous at the floor boundary, so widths never pop).
/// x is the prefix sum of shallower widths — the stack is left-anchored at
/// x = 0 and fully determined by zoom.
pub fn column_table(zoom: f64, vw: f64, max_depth: usize) -> Vec<ColPx> {
    let n = max_depth + 1;
    let target = STACK_FRACTION * vw;
    let heights: Vec<f64> = (0..n).map(|d| cell_px_height(d as u8, zoom)).collect();
    let weights: Vec<f64> = heights.iter().map(|&h| column_weight(h)).collect();
    let mut floored = vec![false; n];
    let mut scale;
    loop {
        let free_sum: f64 = weights.iter().zip(&floored).filter(|&(_, &f)| !f).map(|(w, _)| w).sum();
        let budget = target - GUTTER_PX * floored.iter().filter(|&&f| f).count() as f64;
        if free_sum <= 0.0 || budget <= 0.0 {
            // Degenerate (gutters alone overflow a tiny window, or every
            // depth is floored): free columns collapse to zero width.
            scale = 0.0;
            break;
        }
        scale = budget / free_sum;
        let mut changed = false;
        for d in 0..n {
            // Only zoomed-past columns get the gutter floor; the deep rising
            // tail is allowed to be arbitrarily thin.
            if !floored[d] && heights[d] > PEAK_CELL_PX && scale * weights[d] < GUTTER_PX {
                floored[d] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = Vec::with_capacity(n);
    let mut x = 0.0;
    for d in 0..n {
        let w = if floored[d] { GUTTER_PX } else { scale * weights[d] };
        out.push(ColPx { x, w });
        x += w;
    }
    out
}

/// Rung by pixel height, downgraded to Dot when the column is too narrow
/// for text (gutter strips) and from Full to Detail when too narrow for
/// code. Heights below MERGE_PX merge into the parent. For leaf items,
/// pass `natural_px`: the box is Full as soon as it holds about three
/// floor-font code rows (LEAF_CODE_MIN_PX) or its whole content, whichever
/// is smaller — code persists, scaled then clipped (spec 4d §3).
pub fn rung_for(px_h: f64, px_w: f64, natural_px: Option<f64>) -> Option<Rung> {
    let by_height = if px_h < MERGE_PX {
        return None;
    } else if px_h < LABEL_PX {
        Rung::Dot
    } else if px_h < CARD_PX {
        Rung::Label
    } else if px_h < DETAIL_PX {
        Rung::Card
    } else if px_h < FULL_PX {
        Rung::Detail
    } else {
        Rung::Full
    };
    let by_height = match natural_px {
        Some(n) if px_h >= n.min(content::LEAF_CODE_MIN_PX) => Rung::Full,
        _ => by_height,
    };
    let rung = if px_w < LABEL_MIN_W { Rung::Dot } else { by_height };
    Some(if rung == Rung::Full && px_w < CODE_MIN_W { Rung::Detail } else { rung })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    Dot,
    Label,
    Card,
    Detail,
    Full,
}

use outrider_index::{SymbolId, SymbolNode, SymbolTree};
use outrider_layout::WorldLayout;

use crate::camera::Camera;
use crate::content;

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
    /// Containment rect: extends rightward over visible descendants
    /// (+ NEST_PAD per level of depth difference). Overlaps ancestors.
    pub px: PxRect,
    /// The node's own column width — text lives in this left strip.
    pub label_w: f64,
    pub level: u8,
    pub rung: Rung,
    /// UNclipped screen-y of the box top (`px.y` is clipped to the viewport).
    pub top: f64,
    /// UNclipped pixel height (`px.h` is clipped) — drives the code scale.
    pub full_h: f64,
}

/// Deepest level that exists in the layout (capped at MAX_DEPTH) — the
/// column-table normalization domain. Framing must use the same domain
/// as rendering or widths won't match.
fn tree_max_level(layout: &WorldLayout) -> usize {
    layout
        .nodes
        .values()
        .map(|nl| nl.cells.level as usize)
        .max()
        .unwrap_or(0)
        .min(MAX_DEPTH)
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
    // Normalize over the depths that actually exist, so phantom deep levels
    // don't steal window width from a shallow tree.
    let cols = column_table(camera.zoom, vw, tree_max_level(layout));
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
) -> Option<u8> {
    let nl = layout.nodes.get(&node.id)?;
    let depth = nl.cells.level;
    let abs = parent_abs * outrider_layout::RATIO as f64 + nl.cells.start as f64;
    debug_assert!(abs < 2f64.powi(53), "cell address exceeds exact f64 range");
    let s = column_scale(depth);
    let px_y = camera.world_to_screen_y(abs * s, vh);
    let px_h = nl.cells.len as f64 * s * camera.zoom;
    let &ColPx { x: px_x, w: px_w } = cols.get(depth as usize)?;

    // Below the merge threshold: this node merges into its parent's tile,
    // and children (8x smaller) are below it too. Stop.
    let natural = content::is_leaf_item(node).then(|| content::natural_px(node));
    let rung = rung_for(px_h, px_w, natural)?;
    // Children's y-ranges are contained in the parent's: off-screen y prunes the subtree.
    if px_y > vh || px_y + px_h < 0.0 {
        return None;
    }
    // Deeper columns are further right: past the right edge prunes the subtree.
    if px_x > vw {
        return None;
    }
    // Zoomed-past ancestors have enormous pixel heights; clip to the viewport
    // (2px slack keeps their borders off-screen) before f32 ever sees them.
    // The rung above is chosen from the UNclipped height.
    let y0 = px_y.max(-2.0);
    let y1 = (px_y + px_h).min(vh + 2.0);
    let idx = out.len();
    out.push(DrawItem {
        node,
        px: PxRect { x: px_x, y: y0, w: px_w, h: y1 - y0 },
        label_w: px_w,
        level: depth,
        rung,
        top: px_y,
        full_h: px_h,
    });
    let mut deepest = depth;
    for child in &node.children {
        if let Some(d) = walk(child, layout, camera, cols, vw, vh, abs, out) {
            deepest = deepest.max(d);
        }
    }
    if deepest > depth {
        let dc = &cols[deepest as usize];
        out[idx].px.w = dc.x + dc.w + NEST_PAD * f64::from(deepest - depth) - px_x;
    }
    Some(deepest)
}

/// Absolute world-y band (y, h) of a node: full ancestor composition via
/// WorldLayout::absolute_start, then y = abs·8^-level, h = len·8^-level.
/// (The render walk composes incrementally; this is for framing targets.)
pub fn world_band(id: &SymbolId, layout: &WorldLayout) -> Option<(f64, f64)> {
    let nl = layout.nodes.get(id)?;
    let abs = layout.absolute_start(id)? as f64;
    debug_assert!(abs < 2f64.powi(53), "cell address exceeds exact f64 range");
    let s = column_scale(nl.cells.level);
    Some((abs * s, nl.cells.len as f64 * s))
}

/// Camera framing a leaf item at its natural content height (spec 4c §5):
/// zoom starts at min(natural_px, END_FRACTION·vh) of box height and
/// steps up by 1.25× only as needed to make the leaf's column code-wide,
/// capped at END_FRACTION framing; the result is clamped like frame_band.
pub fn frame_leaf(
    node: &SymbolNode,
    layout: &WorldLayout,
    vw: f64,
    vh: f64,
    min_zoom: f64,
    max_zoom: f64,
) -> Option<Camera> {
    let (y, h) = world_band(&node.id, layout)?;
    let level = layout.nodes.get(&node.id)?.cells.level as usize;
    let max_level = tree_max_level(layout);
    let z_end = crate::camera::END_FRACTION * vh / h;
    let mut z = content::natural_px(node).min(crate::camera::END_FRACTION * vh) / h;
    while z < z_end && column_table(z, vw, max_level)[level].w < CODE_MIN_W {
        z = (z * 1.25).min(z_end);
    }
    Some(Camera { center_y: y + h / 2.0, zoom: z.clamp(min_zoom, max_zoom) })
}

/// Visible node containing the point. Rects nest (ancestors extend over
/// descendants), so take the last hit in DFS order — the deepest node.
pub fn hit_test<'a>(items: &'a [DrawItem<'a>], x: f64, y: f64) -> Option<&'a DrawItem<'a>> {
    items
        .iter()
        .rev()
        .find(|i| x >= i.px.x && x < i.px.x + i.px.w && y >= i.px.y && y < i.px.y + i.px.h)
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
            signature: None,
            doc: None,
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
    fn weight_profile_peak_and_falloff() {
        // peak weight is 1; one depth step (8x in h) is WIDTH_RATIO (4x) in
        // weight, on both sides of the peak
        close(column_weight(PEAK_CELL_PX), 1.0);
        close(column_weight(PEAK_CELL_PX / 8.0), 0.25);
        close(column_weight(PEAK_CELL_PX / 64.0), 0.0625);
        close(column_weight(PEAK_CELL_PX * 8.0), 0.25);
        // symmetric in log-h around the peak
        close(column_weight(PEAK_CELL_PX / 2.0), column_weight(PEAK_CELL_PX * 2.0));
        // self-similar at arbitrary h: one step away from the peak is /4
        for &h in &[1.0, 37.0, 200.0] {
            close(column_weight(h / 8.0), column_weight(h) / 4.0);
        }
        for &h in &[200.0, 5000.0] {
            close(column_weight(8.0 * h), column_weight(h) / 4.0);
        }
    }

    #[test]
    fn column_table_sums_to_target() {
        // normalization: total stack width = STACK_FRACTION·vw at every zoom
        // (gutter floors included in the budget) — no mid-octave breathing
        for &z in &[10.0, 571.4285714285714, 36571.42857142857, 1e6] {
            let t = column_table(z, 800.0, MAX_DEPTH);
            assert_eq!(t.len(), MAX_DEPTH + 1);
            close(t[0].x, 0.0);
            for d in 1..t.len() {
                close(t[d].x, t[d - 1].x + t[d - 1].w);
                assert!(t[d - 1].w > 0.0, "widths must be positive");
            }
            let total = t[MAX_DEPTH].x + t[MAX_DEPTH].w;
            close(total, 0.95 * 800.0);
        }
    }

    #[test]
    fn column_table_gutter_floor() {
        // two octaves past home on a deep table: depth 0 is floored at
        // exactly GUTTER_PX while nearer ancestors are still free
        let t = column_table(256000.0 / 7.0, 800.0, MAX_DEPTH);
        close(t[0].w, GUTTER_PX);
        assert!(t[1].w > GUTTER_PX);
    }

    #[test]
    fn width_ratios_self_similar() {
        // spec §3: zooming 8x shifts the profile one depth; gutter floors
        // shift the budget, so it's the adjacent-width RATIOS that carry over
        for &z in &[10.0, 127.0, 1000.0, 54321.0] {
            let t1 = column_table(z, 800.0, MAX_DEPTH);
            let t8 = column_table(8.0 * z, 800.0, MAX_DEPTH);
            for d in 0..MAX_DEPTH - 1 {
                let free = [t1[d].w, t1[d + 1].w, t8[d + 1].w, t8[d + 2].w]
                    .iter()
                    .all(|&w| w > GUTTER_PX + 0.5);
                if free {
                    close(t8[d + 1].w / t8[d + 2].w, t1[d].w / t1[d + 1].w);
                }
            }
        }
    }

    #[test]
    fn rung_for_thresholds_and_downgrade() {
        // height thresholds (wide column: no downgrade)
        assert_eq!(rung_for(3.9, 400.0, None), None);
        assert_eq!(rung_for(4.0, 400.0, None), Some(Rung::Dot));
        assert_eq!(rung_for(19.9, 400.0, None), Some(Rung::Dot));
        assert_eq!(rung_for(20.0, 400.0, None), Some(Rung::Label));
        assert_eq!(rung_for(79.9, 400.0, None), Some(Rung::Label));
        assert_eq!(rung_for(80.0, 400.0, None), Some(Rung::Card));
        assert_eq!(rung_for(249.9, 400.0, None), Some(Rung::Card));
        assert_eq!(rung_for(250.0, 400.0, None), Some(Rung::Detail));
        assert_eq!(rung_for(699.9, 400.0, None), Some(Rung::Detail));
        assert_eq!(rung_for(700.0, 400.0, None), Some(Rung::Full));
        // narrow columns are forced to Dot regardless of height (gutters)
        assert_eq!(rung_for(100_000.0, 59.9, None), Some(Rung::Dot));
        // Full downgrades to Detail when too narrow for code (spec §4.2)
        assert_eq!(rung_for(100_000.0, 60.0, None), Some(Rung::Detail));
        assert_eq!(rung_for(100_000.0, 299.9, None), Some(Rung::Detail));
        assert_eq!(rung_for(100_000.0, 300.0, None), Some(Rung::Full));
        // the CODE_MIN_W downgrade applies only to Full
        assert_eq!(rung_for(100.0, 60.0, None), Some(Rung::Card));
        // the merge rule wins over everything
        assert_eq!(rung_for(3.9, 24.0, None), None);

        // Leaf code persistence (spec 4d §3): Full whenever the box holds
        // ~three floor-font rows (LEAF_CODE_MIN_PX = 54.1) — or its whole
        // natural height, if that is smaller.
        assert_eq!(rung_for(100.0, 400.0, Some(90.0)), Some(Rung::Full));
        assert_eq!(rung_for(100.0, 400.0, None), Some(Rung::Card)); // container ladder
        assert_eq!(rung_for(100.0, 250.0, Some(90.0)), Some(Rung::Detail)); // width gate holds
        assert_eq!(rung_for(100.0, 59.0, Some(90.0)), Some(Rung::Dot)); // narrow gate holds
        assert_eq!(rung_for(80.0, 400.0, Some(90.0)), Some(Rung::Full)); // ≥ 54.1 → code, clipped
        assert_eq!(rung_for(55.0, 400.0, Some(3000.0)), Some(Rung::Full)); // long fn, no FULL_PX cap
        assert_eq!(rung_for(54.0, 400.0, Some(3000.0)), Some(Rung::Label)); // just below 54.1
        assert_eq!(rung_for(43.0, 400.0, Some(42.4)), Some(Rung::Full)); // tiny leaf: natural wins
        assert_eq!(rung_for(42.0, 400.0, Some(42.4)), Some(Rung::Label)); // below its natural height
        assert_eq!(rung_for(700.0, 400.0, Some(3000.0)), Some(Rung::Full));
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
        assert_eq!(rungs, vec![Rung::Detail, Rung::Detail, Rung::Label, Rung::Label, Rung::Dot]);
        // externally computed px rect for f (zoom = 4000/7):
        // x = w0+w1, y = 0.125·zoom + 300, w = w2, h = 3·zoom/64
        let f = &items[3].px;
        assert!((f.x - 675.0504578).abs() < 1e-6, "{}", f.x);
        assert!((f.y - 371.4285714).abs() < 1e-6, "{}", f.y);
        assert!((f.w - 84.9495422).abs() < 1e-6, "{}", f.w);
        assert!((f.h - 26.7857143).abs() < 1e-6, "{}", f.h);
    }

    #[test]
    fn culling_x_prune_stops_recursion() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // extreme zoom on a 40px-wide viewport: gutters alone (24+24) exceed
        // the 38px budget, so depths 0/1 floor at 24 and x2 = 48 > 40 →
        // depth 2 pruned; a.rs is above the viewport (y-pruned)
        let cam = Camera { center_y: 0.6875, zoom: 1e9 };
        let items = visible_nodes(&tree, &layout, &cam, 40.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["", "b.rs"]);
        assert!((items[0].px.w - 54.0).abs() < 1e-6);
        assert!((items[0].label_w - 24.0).abs() < 1e-6);
        assert!((items[1].px.w - 24.0).abs() < 1e-6);
    }

    #[test]
    fn zoomed_past_ancestors_clip_and_compress() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // two octaves past home (zoom·64), centered on g: root and b.rs are
        // zoomed-past ancestors, clipped to the viewport. With the 4x
        // falloff they compress gradually: weights (7/1280)^⅔, (7/160)^⅔,
        // (7/20)^⅔ normalized to 760 → widths 36.19, 144.76, 579.05
        let cam = Camera { center_y: 0.6875, zoom: 256000.0 / 7.0 };
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        // a.rs and f are entirely above the viewport (y-pruned)
        assert_eq!(names, vec!["", "b.rs", "g"]);
        let rungs: Vec<Rung> = items.iter().map(|i| i.rung).collect();
        // root narrow (36.2 < LABEL_MIN_W) → Dot; b.rs Full-height but
        // 144.76 < CODE_MIN_W → Detail; g 571.4px → Detail
        assert_eq!(rungs, vec![Rung::Dot, Rung::Detail, Rung::Detail]);
        // root strip: x=0, w extended to 772 (encloses g at level 2), y clipped to [-2, 602]
        let root = &items[0].px;
        assert!((root.x - 0.0).abs() < 1e-6 && (root.w - 772.0).abs() < 1e-6, "{:?}", root);
        assert!((root.y - -2.0).abs() < 1e-6 && (root.h - 604.0).abs() < 1e-6);
        assert!((items[0].label_w - 36.1904762).abs() < 1e-6);
        // g: x = w0 + w1, w = w2 (peak-side), y = 300, h clipped to 302
        let g = &items[2].px;
        assert!((g.x - 180.9523810).abs() < 1e-6, "{}", g.x);
        assert!((g.w - 579.0476190).abs() < 1e-6, "{}", g.w);
        assert!((g.y - 300.0).abs() < 1e-6, "{}", g.y);
        assert!((g.h - 302.0).abs() < 1e-6, "{}", g.h);
        // nothing exceeds the clipped viewport band
        for i in &items {
            assert!(i.px.y >= -2.0 - 1e-9 && i.px.y + i.px.h <= 602.0 + 1e-9);
        }
        // DrawItem.top is the UNclipped screen top (px.y is clipped to -2)
        assert!((items[0].top - (300.0 - 0.6875 * (256000.0 / 7.0))).abs() < 1e-6);
        assert!((items[2].top - 300.0).abs() < 1e-6); // on-screen: top == px.y
    }

    /// worked example with byte ranges so f and g are leaf items
    fn leafy_example() -> SymbolTree {
        let mut t = worked_example();
        t.root.children[1].children[0].byte_range = Some(0..10); // f, measure 10
        t.root.children[1].children[1].byte_range = Some(10..20); // g, measure 1
        t
    }

    #[test]
    fn frame_leaf_natural_size_and_width_floor() {
        let tree = leafy_example();
        let layout = outrider_layout::layout(&tree);
        let (vw, vh) = (800.0, 600.0);
        // f: measure 10 → natural = 20.8 + 11·15.6 + 6 = 198.4; its cell
        // height at z_nat is ~198 ≈ PEAK_CELL_PX, so the column is already
        // code-wide: zoom lands exactly at natural height
        let f = &tree.root.children[1].children[0];
        let (fy, fh) = world_band(&f.id, &layout).unwrap();
        let cam = frame_leaf(f, &layout, vw, vh, 1e-9, 1e18).unwrap();
        assert!((cam.zoom * fh - 198.4).abs() < 1e-6); // box = natural px
        assert!((cam.center_y - (fy + fh / 2.0)).abs() < 1e-12);
        let cols = column_table(cam.zoom, vw, 2);
        assert!(cols[2].w >= CODE_MIN_W);
        // g: measure 1 → natural = 58; at that zoom the leaf column is
        // narrower than CODE_MIN_W, so the width floor zooms further in:
        // result is the smallest 1.25-step ≥ natural-height zoom that is
        // code-wide (and its 1.25-times-smaller neighbor is not)
        let g = &tree.root.children[1].children[1];
        let (gy, gh) = world_band(&g.id, &layout).unwrap();
        let z_nat = 58.0 / gh;
        let cam = frame_leaf(g, &layout, vw, vh, 1e-9, 1e18).unwrap();
        assert!(cam.zoom > z_nat);
        assert!((cam.center_y - (gy + gh / 2.0)).abs() < 1e-12);
        assert!(column_table(cam.zoom, vw, 2)[2].w >= CODE_MIN_W);
        assert!(column_table(cam.zoom / 1.25, vw, 2)[2].w < CODE_MIN_W);
        // cap: a viewport too short for natural height caps at END framing
        let cam = frame_leaf(g, &layout, vw, 60.0, 1e-9, 1e18).unwrap();
        assert!((cam.zoom - crate::camera::END_FRACTION * 60.0 / gh).abs() < 1e-9);
        // clamp: min_zoom wins over the search result
        let cam = frame_leaf(g, &layout, vw, vh, 1e9, 1e18).unwrap();
        assert!((cam.zoom - 1e9).abs() < 1.0);
    }

    #[test]
    fn world_band_composes_ancestors() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // g: depth 2, abs cell 44, len 1 (the Phase 2 worked example)
        let g_id = tree.root.children[1].children[1].id.clone();
        let (y, h) = world_band(&g_id, &layout).unwrap();
        close(y, 0.6875);
        close(h, 0.015625);
        let (y, h) = world_band(&tree.root.id, &layout).unwrap();
        close(y, 0.0);
        close(h, 1.0);
        let unknown = outrider_index::SymbolId {
            kind: SymbolKind::Fn,
            qualified_path: "nope".into(),
            ordinal: 0,
        };
        assert!(world_band(&unknown, &layout).is_none());
    }

    #[test]
    fn hit_test_picks_the_column_under_the_point() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // the zoomed-past-ancestors scene: root [0,36.19), b.rs [36.19,180.95),
        // g [180.95,760) horizontally; g only spans y ∈ [300, 602]
        let cam = Camera { center_y: 0.6875, zoom: 256000.0 / 7.0 };
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        assert_eq!(hit_test(&items, 10.0, 10.0).unwrap().node.name, "");
        assert_eq!(hit_test(&items, 100.0, 500.0).unwrap().node.name, "b.rs");
        assert_eq!(hit_test(&items, 400.0, 450.0).unwrap().node.name, "g");
        assert_eq!(hit_test(&items, 400.0, 100.0).unwrap().node.name, "b.rs"); // above g's band, inside b.rs's extended rect
        assert!(hit_test(&items, 790.0, 300.0).is_none()); // right of the stack
    }

    #[test]
    fn nested_rects_extend_over_descendants() {
        let tree = worked_example();
        let layout = outrider_layout::layout(&tree);
        // zoomed-past scene: cols w = 36.19, 144.76, 579.05; stack right = 760
        let cam = Camera { center_y: 0.6875, zoom: 256000.0 / 7.0 };
        let items = visible_nodes(&tree, &layout, &cam, 800.0, 600.0);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["", "b.rs", "g"]);
        // root (level 0) encloses g (level 2): right = 760 + 2·NEST_PAD
        assert!((items[0].px.w - 772.0).abs() < 1e-6, "{}", items[0].px.w);
        // b.rs (level 1): right = 760 + 1·NEST_PAD → w = 766 − 36.190…
        assert!((items[1].px.w - 729.8095238).abs() < 1e-6, "{}", items[1].px.w);
        // g is a leaf: keeps its own column width
        assert!((items[2].px.w - 579.0476190).abs() < 1e-6, "{}", items[2].px.w);
        // label_w stays the own-column width for text layout
        assert!((items[0].label_w - 36.1904762).abs() < 1e-6);
        assert!((items[1].label_w - 144.7619048).abs() < 1e-6);
        assert!((items[2].label_w - 579.0476190).abs() < 1e-6);
        assert_eq!((items[0].level, items[1].level, items[2].level), (0, 1, 2));
        // a parent whose children are all culled keeps its column edge:
        // the x-prune scene (40px viewport) draws root+b.rs only
        let cam = Camera { center_y: 0.6875, zoom: 1e9 };
        let items = visible_nodes(&tree, &layout, &cam, 40.0, 600.0);
        assert!((items[1].px.w - 24.0).abs() < 1e-6); // b.rs: g x-pruned
        assert!((items[0].px.w - 54.0).abs() < 1e-6); // root: 24+24 + NEST_PAD
    }
}
