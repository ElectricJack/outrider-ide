//! Pre-built lookup tables over an immutable `SymbolTree`.
//! `TreeIndex` maps every `SymbolId` to its node and parent for O(1) access.

use std::collections::BTreeMap;

use crate::{SymbolId, SymbolNode, SymbolTree};

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
        let mut idx = TreeIndex {
            nodes: BTreeMap::new(),
            parents: BTreeMap::new(),
        };
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

    /// Pre-order iterator over all nodes in the tree.
    pub fn iter(&self) -> impl Iterator<Item = &'a SymbolNode> + '_ {
        self.nodes.values().copied()
    }

    /// Number of nodes in the index.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
