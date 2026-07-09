use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};

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

    /// Right: last-visited child if still valid, else first child.
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

    /// Up (-1) / Down (+1): cycle name-ordered siblings, wrapping.
    pub fn step_sibling(&mut self, delta: isize, index: &TreeIndex) -> bool {
        let Some(parent_id) = index.parent(&self.current) else { return false };
        let parent = index.node(parent_id).expect("parent id is in the index");
        let n = parent.children.len() as isize;
        let i = parent
            .children
            .iter()
            .position(|c| c.id == self.current)
            .expect("focus is a child of its parent") as isize;
        let next = parent.children[(i + delta).rem_euclid(n) as usize].id.clone();
        self.land(next, index)
    }

    /// Left: move to the structural parent (no-op at the root).
    pub fn step_out(&mut self, index: &TreeIndex) -> bool {
        let Some(p) = index.parent(&self.current) else { return false };
        self.land(p.clone(), index)
    }

    /// Click: set focus directly. The caller must not move the camera.
    pub fn set(&mut self, id: SymbolId, index: &TreeIndex) -> bool {
        self.land(id, index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolKind, SymbolTree};

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
    fn right_steps_into_first_child_and_leaf_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "a.rs"));
        assert!(!f.step_in(&idx)); // a.rs is a leaf
        assert_eq!(f.current, id(SymbolKind::File, "a.rs"));
    }

    #[test]
    fn up_down_cycle_and_wrap() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.step_in(&idx); // a.rs
        assert!(f.step_sibling(1, &idx));
        assert_eq!(f.current, id(SymbolKind::File, "b.rs"));
        assert!(f.step_sibling(1, &idx)); // wraps
        assert_eq!(f.current, id(SymbolKind::File, "a.rs"));
        assert!(f.step_sibling(-1, &idx)); // wraps the other way
        assert_eq!(f.current, id(SymbolKind::File, "b.rs"));
    }

    #[test]
    fn sibling_at_root_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        assert!(!f.step_sibling(1, &idx));
        assert!(!f.step_sibling(-1, &idx));
        assert_eq!(f.current, t.root.id);
    }

    #[test]
    fn left_moves_to_parent_and_root_is_noop() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.step_in(&idx); // a.rs
        f.step_sibling(1, &idx); // b.rs
        f.step_in(&idx); // b.rs::f
        assert!(f.step_out(&idx));
        assert_eq!(f.current, id(SymbolKind::File, "b.rs"));
        assert!(f.step_out(&idx));
        assert_eq!(f.current, t.root.id);
        assert!(!f.step_out(&idx)); // root has no parent
    }

    #[test]
    fn left_then_right_returns_to_the_same_child() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.step_in(&idx); // a.rs
        f.step_sibling(1, &idx); // b.rs
        f.step_in(&idx); // b.rs::f
        f.step_sibling(1, &idx); // b.rs::g
        f.step_out(&idx); // b.rs
        assert!(f.step_in(&idx));
        assert_eq!(f.current, id(SymbolKind::Fn, "b.rs::g")); // last visited, not first
    }

    #[test]
    fn right_remembers_last_visited_child() {
        let t = tree();
        let idx = TreeIndex::new(&t);
        let mut f = Focus::new(t.root.id.clone());
        f.step_in(&idx); // a.rs
        f.step_sibling(1, &idx); // b.rs → last_child[root] = b.rs
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
}
