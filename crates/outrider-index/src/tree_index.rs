//! Pre-built lookup tables over an immutable `SymbolTree`.
//! `TreeIndex` maps every `SymbolId` to its node, parent, and depth for
//! O(1) access. Hash maps (not ordered maps) — `SymbolId` keys carry long
//! qualified paths, and ordered-map comparisons on those dominate build
//! and lookup cost on large trees. Iteration order is preserved
//! separately as an explicit pre-order list.

use std::collections::HashMap;

use crate::{SymbolId, SymbolNode, SymbolTree};

/// Lookup maps over a borrowed SymbolTree, built once per use.
pub struct TreeIndex<'a> {
    nodes: HashMap<&'a SymbolId, (&'a SymbolNode, u32)>,
    parents: HashMap<&'a SymbolId, &'a SymbolId>,
    preorder: Vec<&'a SymbolNode>,
}

/// Build and query the lookup maps over a borrowed SymbolTree.
impl<'a> TreeIndex<'a> {
    /// Walk the whole tree once and populate node, parent, and depth maps.
    pub fn new(tree: &'a SymbolTree) -> Self {
        fn walk<'a>(node: &'a SymbolNode, depth: u32, idx: &mut TreeIndex<'a>) {
            idx.nodes.insert(&node.id, (node, depth));
            idx.preorder.push(node);
            for c in &node.children {
                idx.parents.insert(&c.id, &node.id);
                walk(c, depth + 1, idx);
            }
        }
        let mut idx = TreeIndex {
            nodes: HashMap::new(),
            parents: HashMap::new(),
            preorder: Vec::new(),
        };
        walk(&tree.root, 0, &mut idx);
        idx
    }

    /// Borrow the node for `id`, or None if not in this tree.
    pub fn node(&self, id: &SymbolId) -> Option<&'a SymbolNode> {
        self.nodes.get(id).map(|(n, _)| *n)
    }

    /// Structural parent of `id`, or None for the root.
    pub fn parent(&self, id: &SymbolId) -> Option<&'a SymbolId> {
        self.parents.get(id).copied()
    }

    /// Number of ancestors above `id`; None if the id is unknown.
    pub fn depth(&self, id: &SymbolId) -> Option<usize> {
        self.nodes.get(id).map(|(_, d)| *d as usize)
    }

    /// Pre-order iterator over all nodes in the tree.
    pub fn iter(&self) -> impl Iterator<Item = &'a SymbolNode> + '_ {
        self.preorder.iter().copied()
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
