use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

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
        || matches!(ext, "c" | "h" | "cpp" | "hpp" | "cc" | "hh" | "cxx" | "hxx" | "inl")
}

/// Extension of the file part of a qualified path: everything before the
/// first `::` (and before any `#` chunk suffix), then after the last `.`.
fn file_ext(qualified_path: &str) -> Option<&str> {
    let file = qualified_path.split("::").next().unwrap_or(qualified_path);
    let file = file.split('#').next().unwrap_or(file);
    file.rfind('.').map(|dot| &file[dot + 1..])
}

fn name_is_doc(name: &str) -> bool {
    name.rfind('.').is_some_and(|dot| is_doc_ext(&name[dot + 1..]))
}

/// (doc files, total files) under a folder, recursively. Symbol items
/// inside files are not files and don't count.
fn doc_stats(node: &SymbolNode) -> (u64, u64) {
    let (mut doc, mut total) = (0, 0);
    for c in &node.children {
        match c.id.kind {
            SymbolKind::File => {
                total += 1;
                doc += name_is_doc(&c.name) as u64;
            }
            SymbolKind::Folder => {
                let (d, t) = doc_stats(c);
                doc += d;
                total += t;
            }
            _ => {}
        }
    }
    (doc, total)
}

/// 1 if this folder child is documentation — a doc file, or a folder
/// whose files are more than 70% doc — else 0. Doc children pack after
/// source children so source never competes with docs purely by size.
fn doc_rank(node: &SymbolNode) -> u8 {
    match node.id.kind {
        SymbolKind::File => name_is_doc(&node.name) as u8,
        SymbolKind::Folder => {
            let (doc, total) = doc_stats(node);
            (doc * 10 > total * 7) as u8
        }
        _ => 0,
    }
}

/// Bottom-up size pass: returns (w, h) and records each node's position
/// relative to its parent's origin in `rel` (x, y, w, h). Children fill
/// columns top-to-bottom, wrapping right toward a square aspect (spec §5).
/// The root's relative position stays (0, 0).
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
    let mut order: Vec<(&SymbolNode, (f64, f64), u8)> = node
        .children
        .iter()
        .map(|c| (c, size(c, cfg, rel), doc_rank(c)))
        .collect();
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
        // Docs sink last; then kind groups (types → fns → classes →
        // modules), tallest first within a group so greedy column fill
        // becomes FFD; name then ordinal keep equal-height runs
        // alphabetical/deterministic. Doc ranks were precomputed above —
        // no tree walks inside the comparator.
        order.sort_by(|(a, sa, da), (b, sb, db)| {
            da.cmp(db)
                .then(kind_rank(&a.id.kind).cmp(&kind_rank(&b.id.kind)))
                .then(sb.1.total_cmp(&sa.1))
                .then(a.name.as_bytes().cmp(b.name.as_bytes()))
                .then(a.id.ordinal.cmp(&b.id.ordinal))
        });
    }
    let area: f64 = order.iter().map(|(_, (w, h), _)| w * h).sum();
    let tallest = order.iter().map(|&(_, (_, h), _)| h).fold(0.0, f64::max);
    // tallest.max(...) guarantees no child is ever forced to wrap alone.
    let target_h = tallest.max((area / cfg.aspect).sqrt());
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
    let wh = (x + col_w + 2.0 * cfg.gap, cfg.container_header + content_h + 2.0 * cfg.gap);
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
            container_header: 52.0,
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
                            n(SymbolKind::Item { label: "fn".into() }, "b.rs::f", "f", 10, vec![]),
                            n(SymbolKind::Item { label: "fn".into() }, "b.rs::g", "g", 1, vec![]),
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
        assert_rect(rect(&p, "a.rs"), 8.0, 60.0, 480.0, 1602.4);
        // b.rs: f fills the first column, g stacks under it (one column)
        assert_rect(rect(&p, "b.rs::f"), 504.0, 120.0, 480.0, 198.4);
        assert_rect(rect(&p, "b.rs::g"), 504.0, 326.4, 480.0, 58.0);
        assert_rect(rect(&p, "b.rs"), 496.0, 60.0, 496.0, 332.4);
        // root: a.rs fills column 1 (tall), b.rs wraps to column 2
        assert_rect(rect(&p, ""), 0.0, 0.0, 1000.0, 1670.4);
    }

    #[test]
    fn deterministic() {
        let a = pack(&worked_example(), &cfg());
        let b = pack(&worked_example(), &cfg());
        assert_eq!(a.rects, b.rects);
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
        close(a.x, 496.0);
        close(a.y, 60.0);
    }

    #[test]
    fn kind_groups_beat_size_types_first_modules_last() {
        // Scrambled input: huge loose fn, small module, tiny struct, small
        // class. Group rank wins over height: the tiny struct still packs
        // first; the module packs last despite the fn being far taller.
        let item = |label: &str, qp: &str, name: &str, measure: u64| {
            n(SymbolKind::Item { label: label.into() }, qp, name, measure, vec![])
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
        assert!(c.x > big.x || (c.x == big.x && c.y > big.y), "class after fn");
        assert!(sub.x > c.x || (sub.x == c.x && sub.y > c.y), "module after class");
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
        // f: 480 × 822.4; b.rs grows wide (two columns): 984 × 890.4
        assert_rect(rect(&after, "b.rs::f"), 504.0, 120.0, 480.0, 822.4);
        assert_rect(rect(&after, "b.rs"), 496.0, 60.0, 984.0, 890.4);
        // g wraps to b.rs's second column
        let g = rect(&after, "b.rs::g");
        close(g.x, 992.0);
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
        // single 480×58 child: content 480×58 → root 496 × 126.0
        assert_rect(rect(&p, "one.rs"), 8.0, 60.0, 480.0, 58.0);
        assert_rect(rect(&p, ""), 0.0, 0.0, 496.0, 126.0);
    }

    #[test]
    fn columns_fill_down_then_wrap_right() {
        // Four equal 480×120.4 pages, aspect 1.6 (test cfg): target_h ≈ 380
        // holds three per column, the fourth wraps to a second column.
        let files: Vec<SymbolNode> = (1..=4)
            .map(|i| n(SymbolKind::File, &format!("c{i}.rs"), &format!("c{i}.rs"), 5, vec![]))
            .collect();
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, files),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        assert_rect(rect(&p, "c1.rs"), 8.0, 60.0, 480.0, 120.4);
        assert_rect(rect(&p, "c2.rs"), 8.0, 188.4, 480.0, 120.4);
        assert_rect(rect(&p, "c3.rs"), 8.0, 316.8, 480.0, 120.4);
        assert_rect(rect(&p, "c4.rs"), 496.0, 60.0, 480.0, 120.4);
        assert_rect(rect(&p, ""), 0.0, 0.0, 984.0, 445.2);
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
            vec![chunk("zzz", 0, 0_u16), chunk("mmm", 60, 1_u16), chunk("aaa", 120, 2_u16)],
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
        assert!(z.y < m.y && m.y < a.y, "chunks stack zzz(0) < mmm(60) < aaa(120)");
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
    fn doc_rank_files_by_name_folders_by_recursive_share() {
        let f = |name: &str| n(SymbolKind::File, name, name, 1, vec![]);
        assert_eq!(doc_rank(&f("README.md")), 1);
        assert_eq!(doc_rank(&f("main.rs")), 0);
        // 3 of 4 files doc (75% > 70%) — doc, counted through a subfolder
        let d75 = n(
            SymbolKind::Folder,
            "d",
            "d",
            0,
            vec![
                n(SymbolKind::Folder, "d/sub", "sub", 0, vec![f("a.md"), f("b.md")]),
                f("c.md"),
                f("x.rs"),
            ],
        );
        assert_eq!(doc_rank(&d75), 1);
        // 1 of 2 (50%, not > 70%) — not doc
        let mixed = n(SymbolKind::Folder, "m", "m", 0, vec![f("a.md"), f("x.rs")]);
        assert_eq!(doc_rank(&mixed), 0);
        // empty folder — not doc
        assert_eq!(doc_rank(&n(SymbolKind::Folder, "e", "e", 0, vec![])), 0);
        // non-file/folder kinds never rank
        let it = n(SymbolKind::Item { label: "fn".into() }, "a.md::x", "x", 1, vec![]);
        assert_eq!(doc_rank(&it), 0);
    }

    #[test]
    fn c_file_children_pack_in_source_order_not_kind_or_size() {
        // Scrambled: the tall struct is declared LAST. Kind/size order would
        // place it first (rank 0, tallest); a .c file must keep byte order.
        let item = |label: &str, qp: &str, name: &str, measure: u64, start: usize| {
            let mut it =
                n(SymbolKind::Item { label: label.into() }, qp, name, measure, vec![]);
            it.byte_range = Some(start..start + 10);
            it
        };
        let mut file = n(
            SymbolKind::File,
            "src/m.c",
            "m.c",
            60,
            vec![
                item("struct", "src/m.c::S", "S", 50, 200),
                item("fn", "src/m.c::zebra", "zebra", 2, 0),
                item("fn", "src/m.c::mid", "mid", 5, 100),
            ],
        );
        file.byte_range = Some(0..300);
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, vec![file]),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        let z = rect(&p, "src/m.c::zebra"); // byte 0
        let m = rect(&p, "src/m.c::mid"); // byte 100
        let s = rect(&p, "src/m.c::S"); // byte 200
        // zebra and mid stack in the first column in byte order; the struct —
        // which kind/size order would have placed first — packs last (wraps)
        close(z.x, m.x);
        assert!(z.y < m.y, "zebra(0) above mid(100)");
        assert!(s.x > m.x, "S(200) last despite kind rank 0 and max height");
    }

    #[test]
    fn nested_markdown_container_keeps_source_order() {
        // A section inside a .md file: its children pack by byte offset even
        // though tallest-first would reverse them.
        let item = |qp: &str, name: &str, measure: u64, start: usize| {
            let mut it =
                n(SymbolKind::Item { label: "h2".into() }, qp, name, measure, vec![]);
            it.byte_range = Some(start..start + 10);
            it
        };
        let mut sec = n(
            SymbolKind::Item { label: "h1".into() },
            "g.md::Sec",
            "Sec",
            0,
            vec![item("g.md::Sec::zz", "zz", 2, 0), item("g.md::Sec::aa", "aa", 30, 100)],
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
        assert!(zz.x < aa.x || (zz.x == aa.x && zz.y < aa.y), "zz(0) before aa(100)");
    }

    #[test]
    fn doc_file_sinks_below_source_in_folder() {
        // README.md is far taller; size order would place it first, but doc
        // rank sinks it below the source file.
        let tree = SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                0,
                vec![
                    n(SymbolKind::File, "README.md", "README.md", 500, vec![]),
                    n(SymbolKind::File, "main.rs", "main.rs", 5, vec![]),
                ],
            ),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        let (r, m) = (rect(&p, "README.md"), rect(&p, "main.rs"));
        // main.rs first: top-left of the content area
        close(m.x, 8.0);
        close(m.y, 60.0);
        // README wraps to the second column
        close(r.x, 496.0);
        close(r.y, 60.0);
    }

    #[test]
    fn folder_doc_share_over_70_percent_sinks() {
        let f = |qp: &str, name: &str| n(SymbolKind::File, qp, name, 1, vec![]);
        // "a_docs" (3/4 doc, recursive through sub) sinks after "mixed"
        // (1/2 doc, not doc) even though a_docs wins BOTH fallback keys:
        // it is taller (more children) and alphabetically first.
        let docs = n(
            SymbolKind::Folder,
            "a_docs",
            "a_docs",
            0,
            vec![
                n(
                    SymbolKind::Folder,
                    "a_docs/sub",
                    "sub",
                    0,
                    vec![f("a_docs/sub/a.md", "a.md"), f("a_docs/sub/b.md", "b.md")],
                ),
                f("a_docs/c.md", "c.md"),
                f("a_docs/x.rs", "x.rs"),
            ],
        );
        let mixed = n(
            SymbolKind::Folder,
            "mixed",
            "mixed",
            0,
            vec![f("mixed/a.md", "a.md"), f("mixed/x.rs", "x.rs")],
        );
        let tree = SymbolTree {
            root: n(SymbolKind::Folder, "", "", 0, vec![docs, mixed]),
            repo_root: "/x".into(),
        };
        let p = pack(&tree, &cfg());
        let (d, m) = (rect(&p, "a_docs"), rect(&p, "mixed"));
        assert!(m.x < d.x || (m.x == d.x && m.y < d.y), "mixed before a_docs");
    }
}
