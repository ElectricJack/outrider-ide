//! Keyboard focus and spatial navigation: Enter/Esc tree traversal, last-child
//! memory, and arrow-key beam-cast stepping over packed layout rects.
//! `Focus` is persistent view state; `TreeIndex` is a per-use lookup built
//! over the immutable symbol tree.

use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};
use outrider_layout::PackLayout;

/// Lookup maps over an immutable SymbolTree, built once per use.
pub struct TreeIndex<'a> {
    nodes: BTreeMap<&'a SymbolId, &'a SymbolNode>,
    parents: BTreeMap<&'a SymbolId, &'a SymbolId>,
}

/// Build and query the two-way lookup maps over a borrowed SymbolTree.
impl<'a> TreeIndex<'a> {
    /// Walk the whole tree once and populate node and parent maps.
    pub fn new(tree: &'a SymbolTree) -> Self {
        fn walk<'a>(node: &'a SymbolNode, idx: &mut TreeIndex<'a>) {
            idx.nodes.insert(&node.id, node);
            for c in &node.children {
                idx.parents.insert(&c.id, &node.id);
                walk(c, idx);
            }
        }
        let mut idx = TreeIndex { nodes: BTreeMap::new(), parents: BTreeMap::new() };
        walk(&tree.root, &mut idx);
        idx
    }

    /// Borrow the node for `id`, or None if not in this tree.
    pub fn node(&self, id: &SymbolId) -> Option<&'a SymbolNode> {
        self.nodes.get(id).copied()
    }

    /// Structural parent of `id`, or None for the root.
    pub fn parent(&self, id: &SymbolId) -> Option<&'a SymbolId> {
        self.parents.get(id).copied()
    }

    /// Number of ancestors above `id`; None if the id is unknown.
    pub fn depth(&self, id: &SymbolId) -> Option<usize> {
        if !self.nodes.contains_key(id) {
            return None;
        }
        let mut d = 0;
        let mut cur = id;
        while let Some(p) = self.parents.get(cur) {
            d += 1;
            cur = p;
        }
        Some(d)
    }
}

/// Keyboard focus with last-visited child memory. Focus never dangles —
/// the tree is immutable at runtime until live reload (Phase 6).
pub struct Focus {
    pub current: SymbolId,
    last_child: BTreeMap<SymbolId, SymbolId>,
}

/// Enter/Esc navigation and direct-set operations on the focus cursor.
impl Focus {
    /// Create focus starting at `root`; last-child memory is empty.
    pub fn new(root: SymbolId) -> Self {
        Focus { current: root, last_child: BTreeMap::new() }
    }

    /// Land on `next`, recording last-visited on the parent. Landing on the
    /// current focus is a no-op (returns false).
    fn land(&mut self, next: SymbolId, index: &TreeIndex) -> bool {
        if next == self.current {
            return false;
        }
        self.current = next;
        self.record_visit(index);
        true
    }

    /// Update `last_child` for the current node's parent after a move.
    fn record_visit(&mut self, index: &TreeIndex) {
        if let Some(p) = index.parent(&self.current) {
            self.last_child.insert(p.clone(), self.current.clone());
        }
    }

    /// Enter: last-visited child if still valid, else first child.
    pub fn step_in(&mut self, index: &TreeIndex) -> bool {
        let Some(node) = index.node(&self.current) else { return false };
        if node.children.is_empty() {
            return false;
        }
        let next = self
            .last_child
            .get(&self.current)
            .filter(|lc| node.children.iter().any(|c| &c.id == *lc))
            .cloned()
            .unwrap_or_else(|| node.children[0].id.clone());
        self.land(next, index)
    }

    /// Esc: move to the structural parent (no-op at the root).
    pub fn step_out(&mut self, index: &TreeIndex) -> bool {
        let Some(p) = index.parent(&self.current) else { return false };
        self.land(p.clone(), index)
    }

    /// Click: set focus directly. The caller must not move the camera.
    pub fn set(&mut self, id: SymbolId, index: &TreeIndex) -> bool {
        self.land(id, index)
    }
}

/// Cardinal direction for a spatial arrow-key step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Spatial arrow step: a beam cast. Slide `current`'s rect in `dir`; a
/// candidate qualifies iff its span on the orthogonal axis strictly
/// overlaps the current rect's span (corner contact does not count) and
/// its near edge lies at or beyond the current rect's leading edge.
/// Nearest edge-to-edge distance wins; ties break by orthogonal center
/// misalignment, then SymbolId. When `current` is a leaf page
/// (`content::is_leaf_item`), candidates are all other leaf pages at any
/// tree depth; otherwise candidates are the nodes at `current`'s own
/// depth. No wrap and no fallback: no qualifier → None (dead key).
pub fn spatial_step(
    current: &SymbolId,
    dir: Dir,
    pack: &PackLayout,
    index: &TreeIndex,
) -> Option<SymbolId> {
    let cur = pack.rects.get(current)?;
    let depth = index.depth(current)?;
    let leaf_mode = index.node(current).is_some_and(crate::content::is_leaf_item);
    let mut best: Option<(f64, f64, &SymbolId)> = None;
    for (id, r) in &pack.rects {
        if id == current {
            continue;
        }
        let eligible = if leaf_mode {
            index.node(id).is_some_and(crate::content::is_leaf_item)
        } else {
            index.depth(id) == Some(depth)
        };
        if !eligible {
            continue;
        }
        let (overlap, primary, misalign) = match dir {
            Dir::Left | Dir::Right => (
                r.y < cur.y + cur.h && r.y + r.h > cur.y,
                if dir == Dir::Left { cur.x - (r.x + r.w) } else { r.x - (cur.x + cur.w) },
                ((r.y + r.h / 2.0) - (cur.y + cur.h / 2.0)).abs(),
            ),
            Dir::Up | Dir::Down => (
                r.x < cur.x + cur.w && r.x + r.w > cur.x,
                if dir == Dir::Up { cur.y - (r.y + r.h) } else { r.y - (cur.y + cur.h) },
                ((r.x + r.w / 2.0) - (cur.x + cur.w / 2.0)).abs(),
            ),
        };
        if !overlap || primary < 0.0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((bp, bm, bid)) => {
                primary < bp || (primary == bp && (misalign < bm || (misalign == bm && id < bid)))
            }
        };
        if better {
            best = Some((primary, misalign, id));
        }
    }
    best.map(|(_, _, id)| id.clone())
}

/// The four beam-cast arrow targets of `current`, indexed Left, Right,
/// Up, Down. `None` entries are dead directions (and get no highlight).
pub fn neighbors(
    current: &SymbolId,
    pack: &PackLayout,
    index: &TreeIndex,
) -> [Option<SymbolId>; 4] {
    [Dir::Left, Dir::Right, Dir::Up, Dir::Down].map(|d| spatial_step(current, d, pack, index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolKind, SymbolTree};
    use outrider_layout::{PackConfig, PackLayout, Rect};

    fn n(
        kind: SymbolKind,
        qp: &str,
        name: &str,
        children: Vec<outrider_index::SymbolNode>,
    ) -> outrider_index::SymbolNode {
        outrider_index::SymbolNode {
            id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
            name: name.into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    /// The Phase 2 worked example: root { a.rs, b.rs { f, g } }.
    fn tree() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                vec![
                    n(SymbolKind::File, "a.rs", "a.rs", vec![]),
                    n(
                        SymbolKind::File,
                        "b.rs",
                        "b.rs",
                        vec![
                            n(SymbolKind::Item { label: "fn".into() }, "b.rs::f", "f", vec![]),
                            n(SymbolKind::Item { label: "fn".into() }, "b.rs::g", "g", vec![]),
                        ],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    fn id(kind: SymbolKind, qp: &str) -> SymbolId {
        SymbolId { kind, qualified_path: qp.into(), ordinal: 0 }
    }

    #[test]
    fn enter_steps_into_first_child_and_leaf_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "a.rs"));
        assert!(!f.step_in(&idx)); // a.rs is a leaf
        assert_eq!(f.current, id(SymbolKind::File, "a.rs"));
    }

    #[test]
    fn esc_moves_to_parent_and_root_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.set(id(SymbolKind::Item { label: "fn".into() }, "b.rs::f"), &idx);
        assert!(f.step_out(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "b.rs"));
        assert!(f.step_out(&idx));
        assert_eq!(f.current, t.root.id);
        assert!(!f.step_out(&idx)); // root has no parent
    }

    #[test]
    fn esc_then_enter_returns_to_the_same_child() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.set(id(SymbolKind::Item { label: "fn".into() }, "b.rs::g"), &idx);
        f.step_out(&idx); // b.rs
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::Item { label: "fn".into() }, "b.rs::g")); // last visited, not first
    }

    #[test]
    fn enter_remembers_last_visited_child() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.set(id(SymbolKind::File, "b.rs"), &idx); // last_child[root] = b.rs
        assert!(f.set(t.root.id.clone(), &idx)); // click back to root
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "b.rs")); // not a.rs
    }

    #[test]
    fn set_to_current_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        assert!(!f.set(t.root.id.clone(), &idx));
        assert_eq!(f.current, t.root.id);
    }

    fn cfg() -> PackConfig {
        PackConfig {
            page_w: 480.0,
            line_step: 15.6,
            header: 20.8,
            container_header: 52.0,
            bottom_pad: 6.0,
            gap: 8.0,
            aspect: 1.0,
        }
    }

    /// root { a.rs { x(measure=6) }, b.rs { f, g } } — two files whose fns
    /// stack in one vertical column, so Up/Down cross file boundaries.
    ///
    /// Geometry under kind-grouped + tallest-first packing (aspect=1.0):
    ///   x has measure=6 → h=136, making a.rs h=204 > b.rs h=192.
    ///   Root sorts a.rs first (tallest File), then b.rs below.
    ///   x (16, 120, 480, 136), f (16, 332, 480, 58), g (16, 398, 480, 58) —
    ///   one column of depth-2 pages spanning two files, a.rs on top.
    fn two_files() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                vec![
                    n(
                        SymbolKind::File,
                        "a.rs",
                        "a.rs",
                        vec![outrider_index::SymbolNode {
                            measure: 6,
                            ..n(SymbolKind::Item { label: "fn".into() }, "a.rs::x", "x", vec![])
                        }],
                    ),
                    n(
                        SymbolKind::File,
                        "b.rs",
                        "b.rs",
                        vec![
                            n(SymbolKind::Item { label: "fn".into() }, "b.rs::f", "f", vec![]),
                            n(SymbolKind::Item { label: "fn".into() }, "b.rs::g", "g", vec![]),
                        ],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    #[test]
    fn depth_counts_ancestors() {
        let t = two_files();
        let idx = TreeIndex::new(&t);
        assert_eq!(idx.depth(&t.root.id), Some(0));
        assert_eq!(idx.depth(&id(SymbolKind::File, "b.rs")), Some(1));
        assert_eq!(idx.depth(&id(SymbolKind::Item { label: "fn".into() }, "b.rs::g")), Some(2));
        assert_eq!(idx.depth(&id(SymbolKind::Item { label: "fn".into() }, "nope")), None);
    }

    #[test]
    fn spatial_step_crosses_parent_boundaries_at_same_depth() {
        let t = two_files();
        let idx = TreeIndex::new(&t);
        let p = outrider_layout::pack(&t, &cfg());
        // Packed geometry (kind-grouped + tallest-first, aspect=1.0):
        //   x (16, 120, 480, 136), f (16, 332, 480, 58), g (16, 398, 480, 58)
        // a.rs (h=204) packs before b.rs (h=192) because it is taller;
        // all three depth-2 fns land in one column spanning two files.
        let x = id(SymbolKind::Item { label: "fn".into() }, "a.rs::x");
        let f = id(SymbolKind::Item { label: "fn".into() }, "b.rs::f");
        let g = id(SymbolKind::Item { label: "fn".into() }, "b.rs::g");
        assert_eq!(spatial_step(&x, Dir::Down, &p, &idx), Some(f.clone())); // into b.rs
        assert_eq!(spatial_step(&f, Dir::Up, &p, &idx), Some(x.clone())); // back into a.rs
        assert_eq!(spatial_step(&g, Dir::Up, &p, &idx), Some(f.clone())); // nearest, not x
        assert_eq!(spatial_step(&g, Dir::Down, &p, &idx), None); // no wrap
        assert_eq!(spatial_step(&f, Dir::Right, &p, &idx), None); // nothing beyond the right edge → dead key
        // depth 1: the two files stack vertically (a.rs above b.rs)
        let a = id(SymbolKind::File, "a.rs");
        let b = id(SymbolKind::File, "b.rs");
        assert_eq!(spatial_step(&a, Dir::Down, &p, &idx), Some(b.clone()));
        assert_eq!(spatial_step(&b, Dir::Up, &p, &idx), Some(a.clone()));
        // the root has no same-depth peers
        assert_eq!(spatial_step(&t.root.id, Dir::Down, &p, &idx), None);
    }

    fn hand_layout(entries: &[(SymbolId, Rect)]) -> PackLayout {
        PackLayout { rects: entries.iter().cloned().collect() }
    }

    /// root { c, p, q } with hand-placed rects to probe the scoring rule.
    fn scoring_tree() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                vec![
                    n(SymbolKind::Item { label: "fn".into() }, "c", "c", vec![]),
                    n(SymbolKind::Item { label: "fn".into() }, "p", "p", vec![]),
                    n(SymbolKind::Item { label: "fn".into() }, "q", "q", vec![]),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    #[test]
    fn spatial_step_requires_beam_overlap() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // q is nearest by center but sits entirely below the beam (y-span
        // 11..21 vs c's 0..10); p is farther but on-beam. Old center
        // scoring picked q for Left — the "Left moves down" bug.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: -60.0, y: 2.0, w: 10.0, h: 6.0 }),
            (q.clone(), Rect { x: -15.0, y: 11.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Left, &lay, &idx), Some(p.clone()));
        // Only the off-beam candidate remains → Left is a dead key.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: -15.0, y: 11.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Left, &lay, &idx), None);
        // A node missing from the layout steps nowhere.
        assert_eq!(spatial_step(&c, Dir::Left, &hand_layout(&[]), &idx), None);
    }

    #[test]
    fn spatial_step_nearest_edge_wins() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // Both overlap the beam. q's near edge (x=14, distance 4) beats
        // p's (x=20, distance 10) even though p is perfectly centered
        // and q's center is 10 off-axis.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 14.0, y: -30.0, w: 10.0, h: 50.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(q.clone()));
    }

    #[test]
    fn spatial_step_ties_break_on_misalignment_then_id() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // Equal edge distance (both at x=20). p center-y 11 → misalign 6;
        // q center-y -4 → misalign 9. Better-centered p wins.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 6.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 20.0, y: -9.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
        // Exact tie on distance AND misalignment (6 each): lesser SymbolId
        // wins → p.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 6.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 20.0, y: -6.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
    }

    #[test]
    fn spatial_step_shared_edge_qualifies_but_corner_touch_does_not() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // p shares c's right edge (distance 0) → reachable. q touches only
        // at the bottom-right corner → not reachable Down or Right.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 10.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 10.0, y: 10.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
        assert_eq!(spatial_step(&c, Dir::Down, &lay, &idx), None);
    }

    #[test]
    fn neighbors_returns_left_right_up_down() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let c = id(SymbolKind::Item { label: "fn".into() }, "c");
        let p = id(SymbolKind::Item { label: "fn".into() }, "p");
        let q = id(SymbolKind::Item { label: "fn".into() }, "q");
        // p directly right, q directly below; nothing left or up.
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -100.0, y: -100.0, w: 300.0, h: 300.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 0.0, y: 20.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(
            neighbors(&c, &lay, &idx),
            [None, Some(p.clone()), None, Some(q.clone())]
        );
    }

    /// A leaf page with source bytes (unlike `n`, which leaves byte_range None).
    fn leaf(
        kind: SymbolKind,
        qp: &str,
        name: &str,
        children: Vec<outrider_index::SymbolNode>,
    ) -> outrider_index::SymbolNode {
        outrider_index::SymbolNode { byte_range: Some(0..1), ..n(kind, qp, name, children) }
    }

    /// root { a.md (leaf, d1), dir (empty folder, d1), b.rs (container, d1) { f (leaf, d2) } }
    fn leaf_depth_tree() -> SymbolTree {
        SymbolTree {
            root: n(
                SymbolKind::Folder,
                "",
                "",
                vec![
                    leaf(SymbolKind::File, "a.md", "a.md", vec![]),
                    n(SymbolKind::Folder, "dir", "dir", vec![]),
                    leaf(
                        SymbolKind::File,
                        "b.rs",
                        "b.rs",
                        vec![leaf(SymbolKind::Item { label: "fn".into() }, "b.rs::f", "f", vec![])],
                    ),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    #[test]
    fn spatial_step_leaf_mode_crosses_depth_and_skips_non_leaves() {
        let t = leaf_depth_tree();
        let idx = TreeIndex::new(&t);
        let a_md = id(SymbolKind::File, "a.md");
        let f = id(SymbolKind::Item { label: "fn".into() }, "b.rs::f");
        // Column top→bottom: a.md (leaf d1), dir (empty folder d1),
        // b.rs (container d1), f (leaf d2). Only a.md and f are leaf pages.
        let lay = hand_layout(&[
            (a_md.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (id(SymbolKind::Folder, "dir"), Rect { x: 0.0, y: 15.0, w: 10.0, h: 10.0 }),
            (id(SymbolKind::File, "b.rs"), Rect { x: 0.0, y: 30.0, w: 10.0, h: 10.0 }),
            (f.clone(), Rect { x: 0.0, y: 45.0, w: 10.0, h: 10.0 }),
        ]);
        // Down from the shallow leaf skips the nearer folder+container and
        // lands on the deeper leaf (crosses depth 1 → 2).
        assert_eq!(spatial_step(&a_md, Dir::Down, &lay, &idx), Some(f.clone()));
        // Up from the deep leaf returns to the shallow leaf (depth 2 → 1).
        assert_eq!(spatial_step(&f, Dir::Up, &lay, &idx), Some(a_md.clone()));
        // No leaf below the bottom leaf → no wrap.
        assert_eq!(spatial_step(&f, Dir::Down, &lay, &idx), None);
    }
}
