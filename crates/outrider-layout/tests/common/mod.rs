// Shared across multiple test binaries; not every binary uses every item.
#![allow(dead_code)]

use outrider_index::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
use proptest::prelude::*;

/// Small pool → frequent same-name siblings → ordinals exercised.
pub const NAMES: &[&str] = &["a", "aa", "b", "c", "x"];
/// Disjoint from NAMES: inserting one of these never re-shuffles existing
/// ordinals (used by the continuity-insert property).
pub const FRESH_NAMES: &[&str] = &["ab", "bz", "zz"];

#[derive(Debug, Clone)]
pub struct GNode {
    pub kind: SymbolKind,
    pub name_idx: usize,
    pub lines: u64,
    pub children: Vec<GNode>,
}

fn g_item() -> impl Strategy<Value = GNode> {
    let leaf = (0..NAMES.len(), 1u64..300).prop_map(|(name_idx, lines)| GNode {
        kind: SymbolKind::Fn,
        name_idx,
        lines,
        children: vec![],
    });
    leaf.prop_recursive(3, 20, 4, |inner| {
        (0..NAMES.len(), 1u64..300, prop::collection::vec(inner, 0..4)).prop_map(
            |(name_idx, lines, children)| GNode {
                kind: SymbolKind::Impl,
                name_idx,
                lines,
                children,
            },
        )
    })
}

fn g_file() -> impl Strategy<Value = GNode> {
    (0..NAMES.len(), 1u64..3000, prop::collection::vec(g_item(), 0..4)).prop_map(
        |(name_idx, lines, children)| GNode {
            kind: SymbolKind::File,
            name_idx,
            lines,
            children,
        },
    )
}

pub fn g_folder() -> impl Strategy<Value = GNode> {
    let base = prop::collection::vec(g_file(), 1..4).prop_map(|children| GNode {
        kind: SymbolKind::Folder,
        name_idx: 0,
        lines: 0,
        children,
    });
    base.prop_recursive(3, 40, 3, |inner| {
        (
            0..NAMES.len(),
            prop::collection::vec(inner, 1..3),
            prop::collection::vec(g_file(), 0..3),
        )
            .prop_map(|(name_idx, subs, files)| {
                let mut children = subs;
                children.extend(files);
                GNode {
                    kind: SymbolKind::Folder,
                    name_idx,
                    lines: 0,
                    children,
                }
            })
    })
}

pub fn to_tree(g: &GNode) -> SymbolTree {
    let mut counter = 0u64;
    let mut root = convert(g, &mut counter);
    root.name = String::new(); // root folder is named "" (spec §4.1)
    SymbolTree {
        root,
        repo_root: "/generated".into(),
    }
}

fn convert(g: &GNode, counter: &mut u64) -> SymbolNode {
    let qp = format!("n{}", *counter);
    *counter += 1;
    let mut children: Vec<SymbolNode> = g.children.iter().map(|c| convert(c, counter)).collect();
    finalize_children(&mut children);
    SymbolNode {
        id: SymbolId {
            kind: g.kind,
            qualified_path: qp,
            ordinal: 0,
        },
        name: NAMES[g.name_idx].to_string(),
        byte_range: None,
        signature: None,
        doc: None,
        measure: g.lines,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}
