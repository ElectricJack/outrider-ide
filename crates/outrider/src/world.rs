//! World model: zoom-rung thresholds, draw-mode classification, and the
//! visible-node walk that projects the packed layout into per-frame DrawItems.
//! Entry points are `visible_nodes` (paint) and `hit_test` (pointer events).

use outrider_index::{SymbolNode, SymbolTree};

use crate::camera::Camera;
use crate::content;

/// Minimum on-screen box height (px) before a node merges into its parent.
pub const MERGE_PX: f64 = 4.0;
/// Minimum height (px) for a Label-rung box; below this it falls back to Dot.
pub const LABEL_PX: f64 = 20.0;
/// Minimum height (px) for Card rung; below this Label is used.
pub const CARD_PX: f64 = 80.0;
/// Minimum height (px) for Detail rung; below this Card is used.
pub const DETAIL_PX: f64 = 250.0;
/// Minimum height (px) for Full rung; below this Detail is used.
pub const FULL_PX: f64 = 700.0;
/// Full is useless in a sliver column; below this width it downgrades to Detail.
pub const CODE_MIN_W: f64 = 300.0;
/// Columns narrower than this render fill + border only (forced Dot).
pub const LABEL_MIN_W: f64 = 60.0;

/// Leaf page width in world units (= natural pixels).
pub const PAGE_W: f64 = 640.0;
/// World-px gap between siblings and container inner margin.
pub const PACK_GAP: f64 = 8.0;
/// Target container width/height ratio (≈ square).
pub const PACK_ASPECT: f64 = 1.0;

/// The app's packing configuration: leaf pages sized by the content
/// module's row metrics, so a page at zoom 1.0 is exactly natural size.
pub fn pack_config(gap: f64) -> outrider_layout::PackConfig {
    outrider_layout::PackConfig {
        page_w: PAGE_W,
        line_step: content::LINE_STEP,
        header: content::HEADER,
        container_header: content::HEADER,
        bottom_pad: content::BOTTOM_PAD,
        gap,
        aspect: PACK_ASPECT,
    }
}

/// Level-of-detail ladder for container boxes, driven by on-screen pixel height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    /// Solid fill only — box too small for any text.
    Dot,
    /// Pinned name row only.
    Label,
    /// Name + one meta line (churn/size).
    Card,
    /// Name + multi-line summary (doc, kind counts).
    Detail,
    /// Full content: doc, inventory, and code for leaf items.
    Full,
}

/// Container rung by pixel height, downgraded to Dot when the column is too
/// narrow for text and from Full to Detail when too narrow for code.
/// Heights below MERGE_PX merge into the parent. Leaf items do NOT use this
/// — they go through `leaf_draw` (spec §3).
pub fn rung_for(px_h: f64, px_w: f64) -> Option<Rung> {
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
    let rung = if px_w < LABEL_MIN_W {
        Rung::Dot
    } else {
        by_height
    };
    Some(if rung == Rung::Full && px_w < CODE_MIN_W {
        Rung::Detail
    } else {
        rung
    })
}

/// Draw mode for a leaf page, chosen by on-screen box size (spec §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafDraw {
    Dot,
    Label,
    Minimap,
    Text,
}

/// Leaf LOD ladder. `None` => merged away (below MERGE_PX). First match wins:
/// tiny → Dot, short → Label (pinned name), then Text once the font clears
/// MIN_TEXT_FONT_PX and the column clears CODE_MIN_W, else Minimap.
pub fn leaf_draw(ph: f64, pw: f64, natural_px: f64) -> Option<LeafDraw> {
    if ph < MERGE_PX {
        return None;
    }
    if pw < LABEL_MIN_W || ph < LABEL_PX {
        return Some(LeafDraw::Dot);
    }
    if ph < CARD_PX {
        return Some(LeafDraw::Label);
    }
    let font = content::FONT_PX * ph / natural_px;
    if font >= content::MIN_TEXT_FONT_PX && pw >= CODE_MIN_W {
        Some(LeafDraw::Text)
    } else {
        Some(LeafDraw::Minimap)
    }
}

/// The chosen draw mode for a visible node: containers keep the `Rung`
/// ladder, leaf pages get a `LeafDraw` tier (spec §3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Draw {
    Container(Rung),
    Leaf(LeafDraw),
}

/// Screen-space rectangle in f64 pixels (may be clipped to the viewport).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PxRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// One visible node ready for painting: screen geometry, LOD mode, and source node.
#[derive(Debug)]
pub struct DrawItem<'a> {
    pub node: &'a SymbolNode,
    /// Containment rect clipped to the viewport.
    pub px: PxRect,
    /// The node's own box width — text lives in this strip.
    pub label_w: f64,
    /// Nesting depth from the root (0 = root).
    pub level: u8,
    pub draw: Draw,
    /// UNclipped screen-x of the box left (`px.x` is clipped to the viewport).
    pub left: f64,
    /// UNclipped screen-y of the box top (`px.y` is clipped to the viewport).
    pub top: f64,
    /// UNclipped pixel height (`px.h` is clipped) — drives the code scale.
    pub full_h: f64,
}

/// Cull the tree against the viewport using packed absolute rects.
/// Returns visible nodes in pre-order (parents before children =
/// painter's order). Children are strictly inside their parents, so an
/// off-screen or sub-merge node prunes its whole subtree.
///
/// `has_thumbnail` reports whether a container's folder texture is cached.
/// Card-rung containers only prune their subtree when the thumbnail is
/// ready, avoiding a blank flash before the texture appears.
pub fn visible_nodes<'a>(
    tree: &'a SymbolTree,
    pack: &outrider_layout::PackLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
    has_thumbnail: impl Fn(&outrider_index::SymbolId) -> bool,
) -> Vec<DrawItem<'a>> {
    let mut out = Vec::new();
    walk(
        &tree.root,
        pack,
        camera,
        vw,
        vh,
        0,
        false,
        &has_thumbnail,
        &mut out,
    );
    out
}

/// Recursive DFS helper for `visible_nodes`: project, cull, classify, clip.
///
/// `parent_has_thumb` is true when the immediate parent already has a cached
/// folder thumbnail. When false, children that would normally merge away
/// (below `MERGE_PX`) are kept as Dot fills so they remain visible until
/// the parent's thumbnail is ready to replace the whole subtree.
#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    node: &'a SymbolNode,
    pack: &outrider_layout::PackLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
    level: u8,
    parent_has_thumb: bool,
    has_thumbnail: &dyn Fn(&outrider_index::SymbolId) -> bool,
    out: &mut Vec<DrawItem<'a>>,
) {
    let Some(r) = pack.rects.get(&node.id) else {
        return;
    };
    let (sx, sy) = camera.world_to_screen(r.x, r.y, vw, vh);
    let (pw, ph) = (r.w * camera.zoom, r.h * camera.zoom);
    if sx > vw || sx + pw < 0.0 || sy > vh || sy + ph < 0.0 {
        return;
    }
    let draw = if content::is_leaf_item(node) {
        match leaf_draw(ph, pw, content::natural_px(node)) {
            Some(ld) => Draw::Leaf(ld),
            None if !parent_has_thumb && ph >= 1.0 => Draw::Leaf(LeafDraw::Dot),
            None => return,
        }
    } else {
        match rung_for(ph, pw) {
            Some(r) => Draw::Container(r),
            None if !parent_has_thumb && ph >= 1.0 => Draw::Container(Rung::Dot),
            None => return,
        }
    };
    let x0 = sx.max(-2.0);
    let x1 = (sx + pw).min(vw + 2.0);
    let y0 = sy.max(-2.0);
    let y1 = (sy + ph).min(vh + 2.0);
    out.push(DrawItem {
        node,
        px: PxRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        },
        label_w: pw,
        level,
        draw,
        top: sy,
        left: sx,
        full_h: ph,
    });
    // Containers only prune their subtree when a folder thumbnail is cached,
    // so children stay visible until the thumbnail is ready to replace them.
    let this_has_thumb = has_thumbnail(&node.id);
    let prune = match draw {
        Draw::Container(Rung::Dot | Rung::Label | Rung::Card) => this_has_thumb,
        _ => false,
    };
    if !prune {
        for child in &node.children {
            walk(
                child,
                pack,
                camera,
                vw,
                vh,
                level.saturating_add(1),
                this_has_thumb,
                has_thumbnail,
                out,
            );
        }
    }
}

/// Visible node containing the point. Children sit strictly inside their
/// parents, so a point inside a node is inside every ancestor; the last
/// hit in DFS order is the deepest node.
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

    fn n(
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
                            n(
                                SymbolKind::Item { label: "fn".into() },
                                "b.rs::f",
                                "f",
                                10,
                                vec![],
                            ),
                            n(
                                SymbolKind::Item { label: "fn".into() },
                                "b.rs::g",
                                "g",
                                1,
                                vec![],
                            ),
                        ],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    #[test]
    fn rung_for_thresholds_and_downgrade() {
        assert_eq!(rung_for(3.9, 400.0), None);
        assert_eq!(rung_for(4.0, 400.0), Some(Rung::Dot));
        assert_eq!(rung_for(19.9, 400.0), Some(Rung::Dot));
        assert_eq!(rung_for(20.0, 400.0), Some(Rung::Label));
        assert_eq!(rung_for(79.9, 400.0), Some(Rung::Label));
        assert_eq!(rung_for(80.0, 400.0), Some(Rung::Card));
        assert_eq!(rung_for(249.9, 400.0), Some(Rung::Card));
        assert_eq!(rung_for(250.0, 400.0), Some(Rung::Detail));
        assert_eq!(rung_for(699.9, 400.0), Some(Rung::Detail));
        assert_eq!(rung_for(700.0, 400.0), Some(Rung::Full));
        // narrow boxes are forced to Dot regardless of height
        assert_eq!(rung_for(100_000.0, 59.9), Some(Rung::Dot));
        // Full downgrades to Detail when too narrow for code (spec §4.2)
        assert_eq!(rung_for(100_000.0, 60.0), Some(Rung::Detail));
        assert_eq!(rung_for(100_000.0, 299.9), Some(Rung::Detail));
        assert_eq!(rung_for(100_000.0, 300.0), Some(Rung::Full));
        // the CODE_MIN_W downgrade applies only to Full
        assert_eq!(rung_for(100.0, 60.0), Some(Rung::Card));
        // the merge rule wins over everything
        assert_eq!(rung_for(3.9, 24.0), None);
    }

    fn pack_cfg() -> outrider_layout::PackConfig {
        outrider_layout::PackConfig {
            page_w: 640.0,
            line_step: 15.6,
            header: 20.8,
            container_header: 52.0,
            bottom_pad: 6.0,
            gap: 8.0,
            aspect: 1.6,
        }
    }

    /// Worked example with measures matching the Task 1 pack fixtures and
    /// byte ranges making f and g leaf items.
    fn packed_example() -> (SymbolTree, outrider_layout::PackLayout) {
        let mut t = worked_example();
        t.root.children[0].measure = 100; // a.rs
        t.root.children[1].measure = 40; // b.rs
        t.root.children[1].children[0].measure = 10; // f
        t.root.children[1].children[1].measure = 1; // g
        t.root.children[1].children[0].byte_range = Some(0..10);
        t.root.children[1].children[1].byte_range = Some(10..20);
        let p = outrider_layout::pack(&t, &pack_cfg());
        (t, p)
    }

    #[test]
    fn packed_walk_zoom_one_clips_and_keeps_unclipped_fields() {
        let (tree, p) = packed_example();
        // zoom 1.0 centered on g's page center (984, 355.4)
        let cam = Camera {
            center_x: 984.0,
            center_y: 355.4,
            zoom: 1.0,
        };
        let items = visible_nodes(&tree, &p, &cam, 800.0, 600.0, |_| false);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["", "a.rs", "b.rs", "f", "g"]);
        use LeafDraw::{Label, Text};
        let draws: Vec<Draw> = items.iter().map(|i| i.draw).collect();
        assert_eq!(
            draws,
            vec![
                Draw::Container(Rung::Full),   // root 1670px
                Draw::Container(Rung::Full),   // a.rs 1602px (no byte_range → container)
                Draw::Container(Rung::Detail), // b.rs 332px
                Draw::Leaf(Text),              // f: 198.4px page, font 12, wide
                Draw::Leaf(Label),             // g: 58px page (< CARD_PX)
            ]
        );
        assert_eq!(
            items.iter().map(|i| i.level).collect::<Vec<_>>(),
            vec![0, 1, 1, 2, 2]
        );
        // a.rs hangs off the left edge: clipped x/w, unclipped left/full_h
        let a = &items[1];
        close(a.px.x, -2.0);
        close(a.left, -576.0);
        close(a.px.w, 66.0);
        close(a.px.y, 4.6);
        close(a.top, 4.6);
        close(a.px.h, 597.4);
        close(a.full_h, 1602.4);
        assert!((a.label_w - 640.0).abs() < 1e-9);
        // root's top is above the viewport: top unclipped, px.y clipped
        close(items[0].top, -55.4);
        close(items[0].left, -584.0);
        close(items[0].px.x, -2.0);
        // g fully on-screen: nothing clipped
        let g = &items[4];
        close(g.px.x, 80.0);
        close(g.px.y, 271.0);
        close(g.px.w, 640.0);
        close(g.px.h, 58.0);
        close(g.full_h, 58.0);
        // hit-test picks the deepest node under the point
        assert_eq!(hit_test(&items, 400.0, 290.0).unwrap().node.name, "g");
        assert_eq!(hit_test(&items, 400.0, 100.0).unwrap().node.name, "f");
    }

    #[test]
    fn packed_walk_shows_children_until_thumbnail_ready() {
        let (tree, p) = packed_example();
        // Zoomed far out: root at Dot rung. Without a cached thumbnail,
        // all children remain visible (sub-MERGE_PX nodes kept as Dots).
        let cam = Camera {
            center_x: 500.0,
            center_y: 819.6,
            zoom: 0.03,
        };
        let items = visible_nodes(&tree, &p, &cam, 800.0, 600.0, |_| false);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert!(names.contains(&""));
        assert!(names.contains(&"a.rs"));
        assert!(names.contains(&"b.rs"));
        assert!(names.contains(&"f"));
        assert!(matches!(items[0].draw, Draw::Container(Rung::Dot)));

        // With a thumbnail cached for root, children are pruned.
        let root_id = tree.root.id.clone();
        let items = visible_nodes(&tree, &p, &cam, 800.0, 600.0, |id| *id == root_id);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec![""]);
    }

    #[test]
    fn packed_walk_prunes_offscreen_subtrees() {
        let (tree, p) = packed_example();
        let cam = Camera {
            center_x: 100_000.0,
            center_y: 100_000.0,
            zoom: 1.0,
        };
        assert!(visible_nodes(&tree, &p, &cam, 800.0, 600.0, |_| false).is_empty());
        // panned right so only b.rs's column of the map remains: a.rs's
        // right edge (648) is left of the viewport's world-left edge
        // (1060 − 400 = 660) → a.rs pruned, b.rs subtree survives
        let cam = Camera {
            center_x: 1060.0,
            center_y: 293.0,
            zoom: 1.0,
        };
        let items = visible_nodes(&tree, &p, &cam, 800.0, 600.0, |_| false);
        let names: Vec<&str> = items.iter().map(|i| i.node.name.as_str()).collect();
        assert_eq!(names, vec!["", "b.rs", "f", "g"]);
    }

    #[test]
    fn leaf_draw_tiers_at_their_boundaries() {
        use LeafDraw::*;
        // merge
        assert_eq!(leaf_draw(3.9, 400.0, 100.0), None);
        // Dot: below LABEL_PX height, or below LABEL_MIN_W width
        assert_eq!(leaf_draw(4.0, 400.0, 100.0), Some(Dot));
        assert_eq!(leaf_draw(19.9, 400.0, 100.0), Some(Dot));
        assert_eq!(leaf_draw(1000.0, 59.9, 100.0), Some(Dot));
        // Label: [LABEL_PX, CARD_PX) height, wide enough
        assert_eq!(leaf_draw(20.0, 400.0, 100.0), Some(Label));
        assert_eq!(leaf_draw(79.9, 400.0, 100.0), Some(Label));
        // Text: font ≥ 4 (ph/natural ≥ 4/12) AND pw ≥ CODE_MIN_W
        assert_eq!(leaf_draw(80.0, 400.0, 100.0), Some(Text)); // font 9.6
        assert_eq!(leaf_draw(80.0, 400.0, 200.0), Some(Text)); // font 4.8
                                                                // Minimap: tall page, font sub-4
        assert_eq!(leaf_draw(80.0, 400.0, 300.0), Some(Minimap)); // font 3.2
                                                                   // width gate forces Minimap even when font clears 4
        assert_eq!(leaf_draw(80.0, 299.9, 100.0), Some(Minimap));
    }

    #[test]
    fn tall_leaf_steps_minimap_then_text_as_it_grows() {
        use LeafDraw::*;
        let natural = 3000.0; // ~190-line page
                              // low zoom: box 200px tall → font 0.8 → Minimap
        assert_eq!(leaf_draw(200.0, 400.0, natural), Some(Minimap));
        // zoom until font ≥ 4 → ph ≥ 4/12·natural = 1000
        assert_eq!(leaf_draw(1000.0, 400.0, natural), Some(Text));
        assert_eq!(leaf_draw(999.0, 400.0, natural), Some(Minimap));
    }

    #[test]
    fn short_leaf_never_enters_minimap() {
        use LeafDraw::*;
        // natural ≤ ~137 → at CARD_PX height font already ≥ 7, so a short
        // leaf steps Label → Text with no Minimap tier.
        let natural = 100.0;
        assert_eq!(leaf_draw(79.9, 400.0, natural), Some(Label));
        assert_eq!(leaf_draw(80.0, 400.0, natural), Some(Text));
    }
}
