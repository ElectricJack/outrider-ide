use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Folder,
    File,
    Module,
    Struct,
    Enum,
    Trait,
    Impl,
    Fn,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolId {
    pub kind: SymbolKind,
    pub qualified_path: String,
    pub ordinal: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolNode {
    pub id: SymbolId,
    pub name: String,
    /// Byte range within the containing file. `None` for folders.
    pub byte_range: Option<Range<usize>>,
    pub measure: u64,
    /// Within-repo churn percentile, 0.0–1.0.
    pub churn: f32,
    /// Raw commit count behind `churn` (inspectability, spec §5.4).
    pub churn_count: u64,
    pub children: Vec<SymbolNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolTree {
    pub root: SymbolNode,
    pub repo_root: PathBuf,
}

/// Sort children byte-wise by name; assign ordinals within same-name runs in
/// prior (source) order. Final order is (name, ordinal) — spec §4.1, §6.3.
pub fn finalize_children(children: &mut [SymbolNode]) {
    children.sort_by(|a, b| a.name.cmp(&b.name)); // stable sort keeps source order on ties
    let mut i = 0;
    while i < children.len() {
        let mut j = i + 1;
        while j < children.len() && children[j].name == children[i].name {
            j += 1;
        }
        for (ord, child) in children[i..j].iter_mut().enumerate() {
            child.id.ordinal = ord as u16;
        }
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(name: &str) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Impl,
                qualified_path: format!("f.rs::{name}"),
                ordinal: 0,
            },
            name: name.to_string(),
            byte_range: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        }
    }

    #[test]
    fn finalize_children_sorts_by_name_and_assigns_ordinals() {
        let mut kids = vec![mk("beta"), mk("alpha"), mk("alpha")];
        finalize_children(&mut kids);
        let got: Vec<(&str, u16)> = kids
            .iter()
            .map(|c| (c.name.as_str(), c.id.ordinal))
            .collect();
        assert_eq!(got, vec![("alpha", 0), ("alpha", 1), ("beta", 0)]);
    }

    #[test]
    fn symbol_tree_serde_roundtrip() {
        let tree = SymbolTree {
            root: mk("root"),
            repo_root: std::path::PathBuf::from("/tmp/x"),
        };
        let json = serde_json::to_string(&tree).unwrap();
        let back: SymbolTree = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
    }
}
