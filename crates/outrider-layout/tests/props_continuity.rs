mod common;

use std::collections::BTreeSet;

use common::{g_folder, to_tree, FRESH_NAMES};
use outrider_index::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::{layout, lines_per_cell, WorldLayout};
use proptest::prelude::*;

/// Index paths (through `children`) of all leaves.
fn leaf_paths(node: &SymbolNode, prefix: Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if node.children.is_empty() {
        out.push(prefix);
        return;
    }
    for (i, c) in node.children.iter().enumerate() {
        let mut p = prefix.clone();
        p.push(i);
        leaf_paths(c, p, out);
    }
}

/// Index paths of all folders (insert targets).
fn folder_paths(node: &SymbolNode, prefix: Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if node.id.kind == SymbolKind::Folder {
        out.push(prefix.clone());
    }
    for (i, c) in node.children.iter().enumerate() {
        let mut p = prefix.clone();
        p.push(i);
        folder_paths(c, p, out);
    }
}

fn node_at<'a>(root: &'a SymbolNode, path: &[usize]) -> &'a SymbolNode {
    path.iter().fold(root, |n, &i| &n.children[i])
}

fn node_at_mut<'a>(root: &'a mut SymbolNode, path: &[usize]) -> &'a mut SymbolNode {
    path.iter().fold(root, |n, &i| &mut n.children[i])
}

/// The exact allowed-changed set (see task header). `path` addresses the
/// perturbed node in `t` (the *perturbed* tree); ancestor ids are identical
/// in both trees. A node absent from `w1` (freshly inserted) counts as
/// "len changed", so the walk continues upward past it.
fn allowed_changed(
    t: &SymbolTree,
    path: &[usize],
    w1: &WorldLayout,
    w2: &WorldLayout,
) -> BTreeSet<SymbolId> {
    let mut allowed = BTreeSet::new();
    for k in (0..=path.len()).rev() {
        let node = node_at(&t.root, &path[..k]);
        allowed.insert(node.id.clone());
        if k > 0 {
            // later siblings, by (name, ordinal) order
            let parent = node_at(&t.root, &path[..k - 1]);
            let mut order: Vec<&SymbolNode> = parent.children.iter().collect();
            order.sort_by(|a, b| {
                a.name
                    .as_bytes()
                    .cmp(b.name.as_bytes())
                    .then(a.id.ordinal.cmp(&b.id.ordinal))
            });
            let pos = order.iter().position(|c| c.id == node.id).unwrap();
            for later in &order[pos + 1..] {
                allowed.insert(later.id.clone());
            }
        }
        let len_changed = match (w1.nodes.get(&node.id), w2.nodes.get(&node.id)) {
            (Some(a), Some(b)) => a.cells.len != b.cells.len,
            _ => true,
        };
        if !len_changed {
            break;
        }
    }
    allowed
}

fn assert_only_allowed_changed(
    t2: &SymbolTree,
    path: &[usize],
    w1: &WorldLayout,
    w2: &WorldLayout,
) {
    let allowed = allowed_changed(t2, path, w1, w2);
    for (id, n1) in &w1.nodes {
        if let Some(n2) = w2.nodes.get(id) {
            if n1 != n2 {
                assert!(
                    allowed.contains(id),
                    "unexpectedly changed: {id:?}\n  before {n1:?}\n  after  {n2:?}"
                );
            }
        }
    }
}

proptest! {
    /// Spec §8.1 property 2: grow one leaf by one cell-worth of lines.
    #[test]
    fn continuity_grow(g in g_folder(), sel in any::<prop::sample::Index>()) {
        let t1 = to_tree(&g);
        let mut paths = Vec::new();
        leaf_paths(&t1.root, Vec::new(), &mut paths);
        let path = paths[sel.index(paths.len())].clone();

        let mut t2 = t1.clone();
        {
            let leaf = node_at_mut(&mut t2.root, &path);
            leaf.measure += lines_per_cell(leaf.id.kind); // exactly +1 cell
        }
        let w1 = layout(&t1);
        let w2 = layout(&t2);
        prop_assert_eq!(w1.nodes.len(), w2.nodes.len());
        // the leaf itself must have grown by exactly one cell
        let leaf_id = &node_at(&t2.root, &path).id;
        prop_assert_eq!(w2.nodes[leaf_id].cells.len, w1.nodes[leaf_id].cells.len + 1);
        assert_only_allowed_changed(&t2, &path, &w1, &w2);
    }

    /// Spec §8.1 property 3: insert one new file into a folder.
    #[test]
    fn continuity_insert(
        g in g_folder(),
        sel in any::<prop::sample::Index>(),
        name_sel in any::<prop::sample::Index>(),
        lines in 1u64..3000,
    ) {
        let t1 = to_tree(&g);
        let mut folders = Vec::new();
        folder_paths(&t1.root, Vec::new(), &mut folders);
        let fpath = folders[sel.index(folders.len())].clone();

        let mut t2 = t1.clone();
        let new_id = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "n-inserted".into(),
            ordinal: 0,
        };
        {
            let folder = node_at_mut(&mut t2.root, &fpath);
            folder.children.push(SymbolNode {
                id: new_id.clone(),
                // fresh name: never collides with NAMES, so existing
                // siblings keep their ordinals (and therefore their ids)
                name: FRESH_NAMES[name_sel.index(FRESH_NAMES.len())].to_string(),
                byte_range: None,
                signature: None,
                doc: None,
                measure: lines,
                churn: 0.0,
                churn_count: 0,
                children: vec![],
            });
            finalize_children(&mut folder.children);
        }
        let w1 = layout(&t1);
        let w2 = layout(&t2);
        prop_assert_eq!(w2.nodes.len(), w1.nodes.len() + 1);

        // path to the inserted node in t2
        let folder = node_at(&t2.root, &fpath);
        let idx = folder.children.iter().position(|c| c.id == new_id).unwrap();
        let mut path = fpath.clone();
        path.push(idx);
        assert_only_allowed_changed(&t2, &path, &w1, &w2);
    }
}
