use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};
use outrider_layout::PackLayout;

/// Lookup maps over an immutable SymbolTree, built once per use.
pub struct TreeIndex<'a> {
    nodes: BTreeMap<&'a SymbolId, &'a SymbolNode>,
    parents: BTreeMap<&'a SymbolId, &'a SymbolId>,
}

impl<'a> TreeIndex<'a> {
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

    pub fn node(&self, id: &SymbolId) -> Option<&'a SymbolNode> {
        self.nodes.get(id).copied()
    }

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

impl Focus {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Spatial arrow step (spec §6): among all nodes at the same tree depth
/// as `current`, pick the candidate whose center lies strictly in `dir`,
/// scored by primary distance + 2·|orthogonal offset|; SymbolId breaks
/// exact ties. No wrap: no candidate → None.
pub fn spatial_step(
    current: &SymbolId,
    dir: Dir,
    pack: &PackLayout,
    index: &TreeIndex,
) -> Option<SymbolId> {
    let cur = pack.rects.get(current)?;
    let (cx, cy) = (cur.x + cur.w / 2.0, cur.y + cur.h / 2.0);
    let depth = index.depth(current)?;
    let mut best: Option<(f64, &SymbolId)> = None;
    for (id, r) in &pack.rects {
        if id == current || index.depth(id) != Some(depth) {
            continue;
        }
        let (nx, ny) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (primary, ortho) = match dir {
            Dir::Right => (nx - cx, (ny - cy).abs()),
            Dir::Left => (cx - nx, (ny - cy).abs()),
            Dir::Down => (ny - cy, (nx - cx).abs()),
            Dir::Up => (cy - ny, (nx - cx).abs()),
        };
        if primary <= 0.0 {
            continue;
        }
        let score = primary + 2.0 * ortho;
        let better = match best {
            None => true,
            Some((s, b)) => score < s || (score == s && id < b),
        };
        if better {
            best = Some((score, id));
        }
    }
    best.map(|(_, id)| id.clone())
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
                            n(SymbolKind::Fn, "b.rs::f", "f", vec![]),
                            n(SymbolKind::Fn, "b.rs::g", "g", vec![]),
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
        f.set(id(SymbolKind::Fn, "b.rs::f"), &idx);
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
        f.set(id(SymbolKind::Fn, "b.rs::g"), &idx);
        f.step_out(&idx); // b.rs
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::Fn, "b.rs::g")); // last visited, not first
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
            bottom_pad: 6.0,
            gap: 8.0,
            aspect: 1.6,
        }
    }

    /// root { a.rs { x }, b.rs { f, g } } — two files whose fns stack in
    /// one vertical column, so Up/Down cross file boundaries.
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
                        vec![n(SymbolKind::Fn, "a.rs::x", "x", vec![])],
                    ),
                    n(
                        SymbolKind::File,
                        "b.rs",
                        "b.rs",
                        vec![
                            n(SymbolKind::Fn, "b.rs::f", "f", vec![]),
                            n(SymbolKind::Fn, "b.rs::g", "g", vec![]),
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
        assert_eq!(idx.depth(&id(SymbolKind::Fn, "b.rs::g")), Some(2));
        assert_eq!(idx.depth(&id(SymbolKind::Fn, "nope")), None);
    }

    #[test]
    fn spatial_step_crosses_parent_boundaries_at_same_depth() {
        let t = two_files();
        let idx = TreeIndex::new(&t);
        let p = outrider_layout::pack(&t, &cfg());
        // packed geometry: x (16, 57.6), f (16, 160.4), g (16, 226.4) —
        // one column of depth-2 pages spanning two files
        let x = id(SymbolKind::Fn, "a.rs::x");
        let f = id(SymbolKind::Fn, "b.rs::f");
        let g = id(SymbolKind::Fn, "b.rs::g");
        assert_eq!(spatial_step(&x, Dir::Down, &p, &idx), Some(f.clone())); // into b.rs
        assert_eq!(spatial_step(&f, Dir::Up, &p, &idx), Some(x.clone())); // back into a.rs
        assert_eq!(spatial_step(&g, Dir::Up, &p, &idx), Some(f.clone())); // nearest, not x
        assert_eq!(spatial_step(&g, Dir::Down, &p, &idx), None); // no wrap
        assert_eq!(spatial_step(&f, Dir::Right, &p, &idx), None); // same x-center → not "right of"
        // depth 1: the two files stack vertically
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
                    n(SymbolKind::Fn, "c", "c", vec![]),
                    n(SymbolKind::Fn, "p", "p", vec![]),
                    n(SymbolKind::Fn, "q", "q", vec![]),
                ],
            ),
            repo_root: std::path::PathBuf::from("/x"),
        }
    }

    #[test]
    fn spatial_step_penalizes_orthogonal_offset() {
        let t = scoring_tree();
        let idx = TreeIndex::new(&t);
        let (c, p, q) =
            (id(SymbolKind::Fn, "c"), id(SymbolKind::Fn, "p"), id(SymbolKind::Fn, "q"));
        // p: straight right, farther (primary 20, ortho 0 → 20);
        // q: nearer in x but 20 off-axis (primary 12, ortho 20 → 52)
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -10.0, y: -30.0, w: 100.0, h: 100.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 0.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 12.0, y: 20.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
        // exact tie (both primary 20, ortho 20): lesser SymbolId wins → p
        let lay = hand_layout(&[
            (t.root.id.clone(), Rect { x: -10.0, y: -30.0, w: 100.0, h: 100.0 }),
            (c.clone(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }),
            (p.clone(), Rect { x: 20.0, y: 20.0, w: 10.0, h: 10.0 }),
            (q.clone(), Rect { x: 20.0, y: -20.0, w: 10.0, h: 10.0 }),
        ]);
        assert_eq!(spatial_step(&c, Dir::Right, &lay, &idx), Some(p.clone()));
        // a node missing from the layout steps nowhere
        assert_eq!(spatial_step(&c, Dir::Right, &hand_layout(&[]), &idx), None);
    }
}
