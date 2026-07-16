//! Bottom-up sizing pass followed by a top-down absolute-position pass.
//! Real folders skyline-pack semantic role blocks; other containers retain
//! kind/height or source-ordered shelf placement. Layout stays deterministic.

use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

use crate::skyline::{skyline_pack, SkylineLayout};
use crate::zones::{build_profiles, effective_role, RoleProfiles, SemanticRole};

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
    /// Name-row strip height at the top of a leaf page.
    pub header: f64,
    /// Reserved height at the top of a container for the name row plus body
    /// text (inventory, kind counts, etc.). Children are placed below this.
    pub container_header: f64,
    pub bottom_pad: f64,
    /// Space between siblings, both axes; also the container's inner margin.
    pub gap: f64,
    /// Target container width/height ratio for column wrapping.
    pub aspect: f64,
}

/// Output of a full layout pass: world-space rectangles for every symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct PackLayout {
    /// Absolute rects for every node; the root sits at (0, 0).
    pub rects: BTreeMap<SymbolId, Rect>,
}

/// Pack the tree bottom-up (spec §3). Pure and deterministic; a
/// container's internal layout depends only on its own children's sizes,
/// so an edit repacks only its ancestor chain (hierarchical stability).
pub fn pack(tree: &SymbolTree, cfg: &PackConfig) -> PackLayout {
    let profiles = build_profiles(&tree.root);
    let mut rel = BTreeMap::new();
    size(&tree.root, SemanticRole::Source, &profiles, cfg, &mut rel);
    let mut rects = BTreeMap::new();
    absolute(&tree.root, 0.0, 0.0, &rel, &mut rects);
    PackLayout { rects }
}

/// Packing group for a sibling (spec: types → loose fns → classes/impls
/// → modules → unknown). Files/folders don't group — all rank 0.
fn kind_rank(kind: &SymbolKind) -> u8 {
    match kind {
        SymbolKind::Item { label } => match label.as_str() {
            "struct" | "enum" | "trait" | "interface" | "type" => 0,
            "fn" => 1,
            "class" | "impl" => 2,
            "module" | "namespace" => 3,
            _ => 4,
        },
        _ => 0,
    }
}

/// Documentation file extensions: these sink below source siblings when
/// packing a folder.
fn is_doc_ext(ext: &str) -> bool {
    matches!(ext, "md" | "markdown" | "txt" | "rst")
}

/// Extensions whose files read top-to-bottom: prose plus declare-before-
/// use languages (C/C++). Their children pack in source order at every
/// nesting level instead of kind/size order.
fn is_source_ordered_ext(ext: &str) -> bool {
    is_doc_ext(ext)
        || matches!(
            ext,
            "c" | "h" | "cpp" | "hpp" | "cc" | "hh" | "cxx" | "hxx" | "inl"
        )
}

/// Extension of the file part of a qualified path: everything before the
/// first `::` (and before any `#` chunk suffix), then after the last `.`.
fn file_ext(qualified_path: &str) -> Option<&str> {
    let file = qualified_path.split("::").next().unwrap_or(qualified_path);
    let file = file.split('#').next().unwrap_or(file);
    file.rfind('.').map(|dot| &file[dot + 1..])
}

const TARGET_HEIGHT_FACTORS: [f64; 9] = [0.5, 0.625, 0.75, 0.875, 1.0, 1.125, 1.5, 2.0, 3.0];

struct FolderChild<'a> {
    node: &'a SymbolNode,
    size: (f64, f64),
    role: SemanticRole,
}

struct FolderArrangement {
    positions: Vec<(SymbolId, f64, f64)>,
    bounds: (f64, f64),
}

fn arrange_folder(children: &mut [FolderChild<'_>], gap: f64, aspect: f64) -> FolderArrangement {
    children.sort_by(|a, b| {
        a.role
            .cmp(&b.role)
            .then(b.size.1.total_cmp(&a.size.1))
            .then(a.node.name.as_bytes().cmp(b.node.name.as_bytes()))
            .then(a.node.id.ordinal.cmp(&b.node.id.ordinal))
    });

    let mut groups: Vec<(SemanticRole, Vec<&FolderChild<'_>>, SkylineLayout)> = Vec::new();
    for role in [
        SemanticRole::Source,
        SemanticRole::Test,
        SemanticRole::Example,
        SemanticRole::ShaderAsset,
        SemanticRole::Docs,
        SemanticRole::Generated,
    ] {
        let members: Vec<_> = children.iter().filter(|child| child.role == role).collect();
        if !members.is_empty() {
            let sizes: Vec<_> = members.iter().map(|child| child.size).collect();
            let layout = skyline_pack(&sizes, gap, aspect);
            groups.push((role, members, layout));
        }
    }

    let block_sizes: Vec<_> = groups.iter().map(|(_, _, layout)| layout.bounds).collect();
    let blocks = skyline_pack(&block_sizes, gap, aspect);
    let mut positions = Vec::with_capacity(children.len());
    for ((_, members, layout), (block_x, block_y)) in groups.iter().zip(blocks.positions) {
        for ((child_x, child_y), child) in layout.positions.iter().zip(members) {
            positions.push((child.node.id.clone(), block_x + child_x, block_y + child_y));
        }
    }
    FolderArrangement {
        positions,
        bounds: blocks.bounds,
    }
}

fn shelf_bounds(sizes: &[(f64, f64)], gap: f64, target_h: f64) -> (f64, f64) {
    let (mut x, mut y, mut col_w, mut content_h) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    for &(w, h) in sizes {
        if y > 0.0 && y + h > target_h {
            x += col_w + gap;
            y = 0.0;
            col_w = 0.0;
        }
        col_w = col_w.max(w);
        content_h = content_h.max(y + h);
        y += h + gap;
    }
    (x + col_w, content_h)
}

fn aspect_envelope_area((w, h): (f64, f64), aspect: f64) -> f64 {
    let envelope_w = w.max(h * aspect);
    envelope_w * (envelope_w / aspect)
}

fn choose_target_height(sizes: &[(f64, f64)], gap: f64, aspect: f64) -> f64 {
    if let [(_, height)] = sizes {
        return *height;
    }
    let area: f64 = sizes.iter().map(|&(w, h)| w * h).sum();
    let tallest = sizes.iter().map(|&(_, h)| h).fold(0.0, f64::max);
    let baseline = tallest.max((area / aspect).sqrt());
    let mut best_height = baseline;
    let mut best_score = aspect_envelope_area(shelf_bounds(sizes, gap, baseline), aspect);
    let mut previous: Option<f64> = None;

    for factor in TARGET_HEIGHT_FACTORS {
        let candidate = tallest.max(baseline * factor);
        if previous == Some(candidate) {
            continue;
        }
        previous = Some(candidate);
        let score = aspect_envelope_area(shelf_bounds(sizes, gap, candidate), aspect);
        if score.total_cmp(&best_score).is_lt() {
            best_height = candidate;
            best_score = score;
        }
    }
    best_height
}

/// Bottom-up size pass: returns (w, h) and records each node's position
/// relative to its parent's origin in `rel` (x, y, w, h). The root's
/// relative position stays (0, 0).
fn size(
    node: &SymbolNode,
    inherited_role: SemanticRole,
    profiles: &RoleProfiles,
    cfg: &PackConfig,
    rel: &mut BTreeMap<SymbolId, (f64, f64, f64, f64)>,
) -> (f64, f64) {
    if node.children.is_empty() {
        let h = cfg.header + (1.0 + node.measure as f64) * cfg.line_step + cfg.bottom_pad;
        rel.insert(node.id.clone(), (0.0, 0.0, cfg.page_w, h));
        return (cfg.page_w, h);
    }
    let folder = matches!(node.id.kind, SymbolKind::Folder);
    let measured: Vec<(&SymbolNode, (f64, f64), SemanticRole)> = node
        .children
        .iter()
        .map(|child| {
            let role = if folder {
                effective_role(&child.id, inherited_role, profiles)
            } else {
                SemanticRole::Source
            };
            let dimensions = size(child, role, profiles, cfg, rel);
            (child, dimensions, role)
        })
        .collect();

    if folder {
        let mut children: Vec<_> = measured
            .into_iter()
            .map(|(node, size, role)| FolderChild { node, size, role })
            .collect();
        let arrangement = arrange_folder(&mut children, cfg.gap, cfg.aspect);
        for (id, x, y) in &arrangement.positions {
            let entry = rel.get_mut(id).expect("child sized above");
            entry.0 = cfg.gap + x;
            entry.1 = cfg.container_header + cfg.gap + y;
        }
        let wh = (
            arrangement.bounds.0 + 2.0 * cfg.gap,
            cfg.container_header + arrangement.bounds.1 + 2.0 * cfg.gap,
        );
        rel.insert(node.id.clone(), (0.0, 0.0, wh.0, wh.1));
        return wh;
    }

    // Re-derive the ordering invariant locally; never trust input Vec order.
    let mut order = measured;
    // Chunk children, and all descendants of source-ordered files (prose,
    // declare-before-use C/C++), pack in source order: reorganizing them
    // would break top-to-bottom reading.
    let source_ordered = order.first().map(|(c, ..)| &c.id.kind) == Some(&SymbolKind::Chunk)
        || (!matches!(node.id.kind, SymbolKind::Folder)
            && file_ext(&node.id.qualified_path).is_some_and(is_source_ordered_ext));
    if source_ordered {
        order.sort_by(|(a, ..), (b, ..)| {
            let ka = a.byte_range.as_ref().map(|r| r.start).unwrap_or(0);
            let kb = b.byte_range.as_ref().map(|r| r.start).unwrap_or(0);
            ka.cmp(&kb).then(a.id.ordinal.cmp(&b.id.ordinal))
        });
    } else {
        // Kind groups (types → fns → classes → modules), tallest first within
        // a group so greedy column fill becomes FFD; name then ordinal keep
        // equal-height runs alphabetical and deterministic.
        order.sort_by(|(a, sa, _), (b, sb, _)| {
            kind_rank(&a.id.kind)
                .cmp(&kind_rank(&b.id.kind))
                .then(sb.1.total_cmp(&sa.1))
                .then(a.name.as_bytes().cmp(b.name.as_bytes()))
                .then(a.id.ordinal.cmp(&b.id.ordinal))
        });
    }
    let sizes: Vec<(f64, f64)> = order.iter().map(|(_, size, _)| *size).collect();
    let target_h = choose_target_height(&sizes, cfg.gap, cfg.aspect);
    let (mut x, mut y, mut col_w, mut content_h) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for &(child, (w, h), _) in &order {
        if y > 0.0 && y + h > target_h {
            x += col_w + cfg.gap;
            y = 0.0;
            col_w = 0.0;
        }
        let e = rel.get_mut(&child.id).expect("child sized above");
        e.0 = cfg.gap + x;
        e.1 = cfg.container_header + cfg.gap + y;
        col_w = col_w.max(w);
        content_h = content_h.max(y + h);
        y += h + cfg.gap;
    }
    let wh = (
        x + col_w + 2.0 * cfg.gap,
        cfg.container_header + content_h + 2.0 * cfg.gap,
    );
    rel.insert(node.id.clone(), (0.0, 0.0, wh.0, wh.1));
    wh
}

/// Top-down pass: converts relative positions from `size` into absolute
/// world-space `Rect`s by accumulating parent offsets recursively.
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
    use crate::zones::{build_profiles, effective_role, SemanticRole};
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    fn cfg() -> PackConfig {
        PackConfig {
            page_w: 640.0,
            line_step: 15.6,
            header: 20.8,
            container_header: 52.0,
            bottom_pad: 6.0,
            gap: 8.0,
            aspect: 1.6,
        }
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
    fn folder_semantic_zones_preserve_hierarchy_and_do_not_interleave() {
        let examples = n(
            SymbolKind::Folder,
            "examples",
            "examples",
            0,
            vec![n(
                SymbolKind::File,
                "examples/demo.rs",
                "demo.rs",
                8,
                vec![],
            )],
        );
        let tree = SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                0,
                vec![
                    n(SymbolKind::File, "src/tall.rs", "tall.rs", 120, vec![]),
                    n(SymbolKind::File, "src/small.rs", "small.rs", 4, vec![]),
                    n(
                        SymbolKind::File,
                        "tests/large_test.rs",
                        "large_test.rs",
                        90,
                        vec![],
                    ),
                    n(
                        SymbolKind::File,
                        "tests/tiny_test.rs",
                        "tiny_test.rs",
                        1,
                        vec![],
                    ),
                    examples,
                    n(
                        SymbolKind::File,
                        "assets/lighting.frag",
                        "lighting.frag",
                        12,
                        vec![],
                    ),
                    n(SymbolKind::File, "README.md", "README.md", 40, vec![]),
                ],
            ),
            repo_root: "/x".into(),
        };

        fn collect_ids(node: &SymbolNode, ids: &mut std::collections::BTreeSet<SymbolId>) {
            ids.insert(node.id.clone());
            for child in &node.children {
                collect_ids(child, ids);
            }
        }

        let packed = pack(&tree, &cfg());
        let mut input_ids = std::collections::BTreeSet::new();
        collect_ids(&tree.root, &mut input_ids);
        assert_eq!(
            packed
                .rects
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            input_ids
        );

        let profiles = build_profiles(&tree.root);
        let mut role_bounds = BTreeMap::<SemanticRole, Rect>::new();
        for child in &tree.root.children {
            let role = effective_role(&child.id, SemanticRole::Source, &profiles);
            let child_rect = packed.rects[&child.id];
            role_bounds
                .entry(role)
                .and_modify(|bounds| {
                    let min_x = bounds.x.min(child_rect.x);
                    let min_y = bounds.y.min(child_rect.y);
                    let max_x = (bounds.x + bounds.w).max(child_rect.x + child_rect.w);
                    let max_y = (bounds.y + bounds.h).max(child_rect.y + child_rect.h);
                    bounds.x = min_x;
                    bounds.y = min_y;
                    bounds.w = max_x - min_x;
                    bounds.h = max_y - min_y;
                })
                .or_insert(child_rect);
        }

        let test_bounds = role_bounds[&SemanticRole::Test];
        for path in ["tests/large_test.rs", "tests/tiny_test.rs"] {
            let child = rect(&packed, path);
            assert!(
                child.x >= test_bounds.x
                    && child.y >= test_bounds.y
                    && child.x + child.w <= test_bounds.x + test_bounds.w
                    && child.y + child.h <= test_bounds.y + test_bounds.h,
                "{path} must remain inside the test role block"
            );
        }

        let bounds: Vec<_> = role_bounds.into_iter().collect();
        for (index, (left_role, left)) in bounds.iter().enumerate() {
            for (right_role, right) in bounds.iter().skip(index + 1) {
                let separated = left.x + left.w <= right.x
                    || right.x + right.w <= left.x
                    || left.y + left.h <= right.y
                    || right.y + right.h <= left.y;
                assert!(separated, "{left_role:?} and {right_role:?} interleave");
            }
        }
    }

    #[test]
    fn worked_example_exact_rects() {
        let p = pack(&worked_example(), &cfg());
        assert_eq!(p.rects.len(), 5);
        assert_rect(rect(&p, "a.rs"), 8.0, 60.0, 640.0, 1602.4);
        assert_rect(rect(&p, "b.rs::f"), 664.0, 120.0, 640.0, 198.4);
        assert_rect(rect(&p, "b.rs::g"), 664.0, 326.4, 640.0, 58.0);
        assert_rect(rect(&p, "b.rs"), 656.0, 60.0, 656.0, 332.4);
        assert_rect(rect(&p, ""), 0.0, 0.0, 1320.0, 1670.4);
    }

    #[test]
    fn deterministic() {
        let a = pack(&worked_example(), &cfg());
        let b = pack(&worked_example(), &cfg());
        assert_eq!(a.rects, b.rects);
    }

    #[test]
    fn candidate_height_reduces_aspect_envelope_for_mixed_child_shapes() {
        let sizes = vec![
            (1400.0, 700.0),
            (640.0, 100.0),
            (640.0, 100.0),
            (640.0, 100.0),
            (640.0, 100.0),
        ];
        let area: f64 = sizes.iter().map(|(w, h)| w * h).sum();
        let baseline = 700.0_f64.max((area / 1.6).sqrt());
        let selected = choose_target_height(&sizes, 8.0, 1.6);
        let baseline_bounds = shelf_bounds(&sizes, 8.0, baseline);
        let selected_bounds = shelf_bounds(&sizes, 8.0, selected);

        assert!(selected > baseline);
        assert!(
            aspect_envelope_area(selected_bounds, 1.6) < aspect_envelope_area(baseline_bounds, 1.6)
        );
    }

    #[test]
    fn single_child_uses_its_height_as_target() {
        close(choose_target_height(&[(640.0, 58.0)], 8.0, 1.6), 58.0);
    }

    #[test]
    fn candidate_height_never_falls_below_tallest_child() {
        let sizes = [(640.0, 800.0), (640.0, 10.0), (640.0, 10.0)];
        assert!(choose_target_height(&sizes, 8.0, 1.6) >= 800.0);
    }

    #[test]
    fn candidate_height_selection_is_repeatable() {
        let sizes = [(1400.0, 700.0), (640.0, 100.0), (640.0, 100.0)];
        let first = choose_target_height(&sizes, 8.0, 1.6);
        let second = choose_target_height(&sizes, 8.0, 1.6);
        assert_eq!(first, second);
    }

    #[test]
    fn equal_score_keeps_the_baseline_candidate() {
        let sizes = [(640.0, 700.0), (640.0, 700.0)];
        let area: f64 = sizes.iter().map(|(w, h)| w * h).sum();
        let baseline = 700.0_f64.max((area / 1.6).sqrt());
        close(choose_target_height(&sizes, 8.0, 1.6), baseline);
    }

    #[test]
    fn children_placed_tallest_first_names_break_ties() {
        // "zeta" is huge, "alpha" tiny — zeta packs first now (size-aware).
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
        // zeta is placed first: top-left of the content area
        close(z.x, 8.0);
        close(z.y, 60.0);
        // alpha wraps to the second column (zeta alone fills target_h)
        close(a.x, 656.0);
        close(a.y, 60.0);
    }

    #[test]
    fn kind_groups_beat_size_types_first_modules_last() {
        // Scrambled input: huge loose fn, small module, tiny struct, small
        // class. Group rank wins over height: the tiny struct still packs
        // first; the module packs last despite the fn being far taller.
        let item = |label: &str, qp: &str, name: &str, measure: u64| {
            n(
                SymbolKind::Item {
                    label: label.into(),
                },
                qp,
                name,
                measure,
                vec![],
            )
        };
        let file = n(
            SymbolKind::File,
            "m.rs",
            "m.rs",
            0,
            vec![
                item("fn", "m.rs::big", "big", 200),
                item("module", "m.rs::sub", "sub", 3),
                item("struct", "m.rs::S", "S", 2),
                item("class", "m.rs::C", "C", 3),
            ],
        );
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, vec![file]),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        let (s, big, c, sub) = (
            rect(&p, "m.rs::S"),
            rect(&p, "m.rs::big"),
            rect(&p, "m.rs::C"),
            rect(&p, "m.rs::sub"),
        );
        // struct is first: top-left of m.rs's content area
        assert!(s.x < big.x && s.y <= big.y, "struct before fn");
        // big fn wraps to its own column right of the struct
        assert!(big.x > s.x, "fn in a later column than the struct");
        // class after fn, module after class (later column or lower in same)
        assert!(
            c.x > big.x || (c.x == big.x && c.y > big.y),
            "class after fn"
        );
        assert!(
            sub.x > c.x || (sub.x == c.x && sub.y > c.y),
            "module after class"
        );
    }

    #[test]
    fn sibling_subtree_stable_under_edit() {
        // Grow f (10 → 50 lines): b.rs reflows internally and root resizes,
        // but a.rs — a sibling subtree — keeps its exact position. Under
        // column-first packing f fills the first column and g wraps to the
        // second column of b.rs.
        let before = pack(&worked_example(), &cfg());
        let mut edited = worked_example();
        edited.root.children[1].children[0].measure = 50;
        let after = pack(&edited, &cfg());
        assert_eq!(rect(&before, "a.rs"), rect(&after, "a.rs"));
        // f: 640 × 822.4; b.rs grows wide (two columns): 1304 × 890.4
        assert_rect(rect(&after, "b.rs::f"), 664.0, 120.0, 640.0, 822.4);
        assert_rect(rect(&after, "b.rs"), 656.0, 60.0, 1304.0, 890.4);
        // g wraps to b.rs's second column
        let g = rect(&after, "b.rs::g");
        close(g.x, 1312.0);
        close(g.y, 120.0);
    }

    #[test]
    fn wide_child_sets_the_floor_for_target_width() {
        // A single child never wraps alone: target_h = max(tallest child,
        // √(area/aspect)) floors the column height to fit it.
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
        // single 640×58 child: content 640×58 → root 656 × 126.0
        assert_rect(rect(&p, "one.rs"), 8.0, 60.0, 640.0, 58.0);
        assert_rect(rect(&p, ""), 0.0, 0.0, 656.0, 126.0);
    }

    #[test]
    fn multi_candidate_height_can_keep_equal_pages_in_one_column() {
        // Four equal 640×120.4 pages fit in one column. Its 640×505.6
        // content bounds have a smaller 1.6-aspect envelope than the old
        // three-plus-one, two-column result.
        let files: Vec<SymbolNode> = (1..=4)
            .map(|i| {
                n(
                    SymbolKind::File,
                    &format!("c{i}.rs"),
                    &format!("c{i}.rs"),
                    5,
                    vec![],
                )
            })
            .collect();
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, files),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        assert_rect(rect(&p, "c1.rs"), 8.0, 60.0, 640.0, 120.4);
        assert_rect(rect(&p, "c2.rs"), 8.0, 188.4, 640.0, 120.4);
        assert_rect(rect(&p, "c3.rs"), 8.0, 316.8, 640.0, 120.4);
        assert_rect(rect(&p, "c4.rs"), 8.0, 445.2, 640.0, 120.4);
        assert_rect(rect(&p, ""), 0.0, 0.0, 656.0, 573.6);
    }

    #[test]
    fn chunk_children_pack_in_source_order_not_label_order() {
        // Three chunks whose labels sort reverse to their byte order; the
        // packer must order them by byte_range.start, not by name.
        let chunk = |label: &str, start: usize, ord: u16| SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Chunk,
                qualified_path: format!("f.rs#{ord}"),
                ordinal: ord,
            },
            name: label.into(),
            byte_range: Some(start..start + 10),
            signature: None,
            doc: None,
            measure: 2,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        };
        let mut file = n(
            SymbolKind::File,
            "f.rs",
            "f.rs",
            12,
            vec![
                chunk("zzz", 0, 0_u16),
                chunk("mmm", 60, 1_u16),
                chunk("aaa", 120, 2_u16),
            ],
        );
        file.byte_range = Some(0..200);
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, vec![file]),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        let z = rect(&p, "f.rs#0"); // "zzz", byte 0
        let m = rect(&p, "f.rs#1"); // "mmm", byte 60
        let a = rect(&p, "f.rs#2"); // "aaa", byte 120
                                    // one column (same x); source order sets the vertical order
        close(z.x, m.x);
        close(m.x, a.x);
        assert!(
            z.y < m.y && m.y < a.y,
            "chunks stack zzz(0) < mmm(60) < aaa(120)"
        );
    }

    #[test]
    fn kind_rank_groups_types_fns_classes_modules() {
        let item = |l: &str| SymbolKind::Item { label: l.into() };
        for l in ["struct", "enum", "trait", "interface", "type"] {
            assert_eq!(kind_rank(&item(l)), 0, "{l} is a type");
        }
        assert_eq!(kind_rank(&item("fn")), 1);
        assert_eq!(kind_rank(&item("class")), 2);
        assert_eq!(kind_rank(&item("impl")), 2);
        assert_eq!(kind_rank(&item("module")), 3);
        assert_eq!(kind_rank(&item("namespace")), 3);
        // unknown labels pack last
        assert_eq!(kind_rank(&item("macro")), 4);
        // files/folders have no kind grouping: all rank 0
        assert_eq!(kind_rank(&SymbolKind::File), 0);
        assert_eq!(kind_rank(&SymbolKind::Folder), 0);
    }

    #[test]
    fn file_ext_takes_file_part_of_qualified_path() {
        assert_eq!(file_ext("src/m.c"), Some("c"));
        assert_eq!(file_ext("src/m.c::s::field"), Some("c"));
        assert_eq!(file_ext("BIG.md#3"), Some("md"));
        assert_eq!(file_ext("README.markdown"), Some("markdown"));
        assert_eq!(file_ext("Makefile"), None);
    }

    #[test]
    fn source_ordered_exts_cover_docs_and_c_family() {
        for e in ["md", "markdown", "txt", "rst"] {
            assert!(is_doc_ext(e), "{e} is doc");
            assert!(is_source_ordered_ext(e), "{e} is source-ordered");
        }
        for e in ["c", "h", "cpp", "hpp", "cc", "hh", "cxx", "hxx", "inl"] {
            assert!(!is_doc_ext(e), "{e} is not doc");
            assert!(is_source_ordered_ext(e), "{e} is source-ordered");
        }
        for e in ["rs", "py", "ts"] {
            assert!(!is_doc_ext(e), "{e} is not doc");
            assert!(!is_source_ordered_ext(e), "{e} reorganizes");
        }
    }

    #[test]
    fn folder_skyline_does_not_change_cpp_file_layout() {
        // Scrambled: the tall struct is declared LAST. Kind/size order would
        // place it first (rank 0, tallest); a .cpp file must keep byte order.
        let item = |label: &str, qp: &str, name: &str, measure: u64, start: usize| {
            let mut it = n(
                SymbolKind::Item {
                    label: label.into(),
                },
                qp,
                name,
                measure,
                vec![],
            );
            it.byte_range = Some(start..start + 10);
            it
        };
        let mut file = n(
            SymbolKind::File,
            "src/m.cpp",
            "m.cpp",
            60,
            vec![
                item("struct", "src/m.cpp::S", "S", 50, 200),
                item("fn", "src/m.cpp::zebra", "zebra", 2, 0),
                item("fn", "src/m.cpp::mid", "mid", 5, 100),
            ],
        );
        file.byte_range = Some(0..300);
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, vec![file]),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        let file = rect(&p, "src/m.cpp");
        let z = rect(&p, "src/m.cpp::zebra"); // byte 0
        let m = rect(&p, "src/m.cpp::mid"); // byte 100
        let s = rect(&p, "src/m.cpp::S"); // byte 200
                                          // zebra and mid stack in the first column in byte order; the struct —
                                          // which kind/size order would have placed first — packs last (wraps)
        close(z.x, m.x);
        assert!(z.y < m.y, "zebra(0) above mid(100)");
        assert!(s.x > m.x, "S(200) last despite kind rank 0 and max height");
        assert_eq!(
            [
                (z.x - file.x, z.y - file.y),
                (m.x - file.x, m.y - file.y),
                (s.x - file.x, s.y - file.y),
            ],
            [(8.0, 60.0), (8.0, 141.6), (656.0, 60.0)]
        );
    }

    #[test]
    fn nested_markdown_container_keeps_source_order() {
        // A section inside a .md file: its children pack by byte offset even
        // though tallest-first would reverse them.
        let item = |qp: &str, name: &str, measure: u64, start: usize| {
            let mut it = n(
                SymbolKind::Item { label: "h2".into() },
                qp,
                name,
                measure,
                vec![],
            );
            it.byte_range = Some(start..start + 10);
            it
        };
        let mut sec = n(
            SymbolKind::Item { label: "h1".into() },
            "g.md::Sec",
            "Sec",
            0,
            vec![
                item("g.md::Sec::zz", "zz", 2, 0),
                item("g.md::Sec::aa", "aa", 30, 100),
            ],
        );
        sec.byte_range = Some(0..200);
        let mut file = n(SymbolKind::File, "g.md", "g.md", 40, vec![sec]);
        file.byte_range = Some(0..200);
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, vec![file]),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        let zz = rect(&p, "g.md::Sec::zz"); // byte 0, short
        let aa = rect(&p, "g.md::Sec::aa"); // byte 100, tall
                                            // byte order beats tallest-first: zz is placed first (reading order)
        assert!(
            zz.x < aa.x || (zz.x == aa.x && zz.y < aa.y),
            "zz(0) before aa(100)"
        );
    }
}
