//! Core data types for the symbol tree: `SymbolKind`, `SymbolId`, `SymbolNode`,
//! and `SymbolTree`, plus the helpers that sort, ordinate, and deduplicate nodes.
//! All types are serde-serializable so the tree can be cached to disk.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The structural role of a node: hierarchy level or a language-level item kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    /// A directory in the repo tree.
    Folder,
    /// A source file.
    File,
    /// A line-range slice of a large unparsed file.
    Chunk,
    /// A language-level symbol (e.g. `"fn"`, `"struct"`, `"class"`).
    Item { label: String },
}

/// Stable identity methods shared by all `SymbolKind` variants.
impl SymbolKind {
    /// Returns the short display label used by the renderer and layout keys.
    pub fn label(&self) -> &str {
        match self {
            SymbolKind::Folder => "folder",
            SymbolKind::File => "file",
            SymbolKind::Chunk => "chunk",
            SymbolKind::Item { label } => label,
        }
    }
}

/// Stable, layout-keyed identity for a single node (spec §4.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolId {
    pub kind: SymbolKind,
    /// `/`-separated path from repo root; `::` separates items within a file.
    pub qualified_path: String,
    /// Disambiguates same-name siblings; 0 for unique names.
    pub ordinal: u16,
}

/// A single node in the symbol tree: folder, file, chunk, or language item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolNode {
    pub id: SymbolId,
    pub name: String,
    /// Byte range within the containing file. `None` for folders.
    pub byte_range: Option<Range<usize>>,
    /// Item declaration up to (excluding) the body `{`, whitespace
    /// collapsed to one line. None for folders and files.
    pub signature: Option<String>,
    /// Leading `//!` block, comment markers stripped. File nodes only.
    pub doc: Option<String>,
    pub measure: u64,
    /// Within-repo churn percentile, 0.0–1.0.
    pub churn: f32,
    /// Raw commit count behind `churn` (inspectability, spec §5.4).
    pub churn_count: u64,
    pub children: Vec<SymbolNode>,
}

/// The complete indexed representation of a repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolTree {
    pub root: SymbolNode,
    pub repo_root: PathBuf,
}

/// Parsed products derived from one retained source buffer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedFile {
    pub items: Vec<SymbolNode>,
    pub doc: Option<String>,
}

/// All products needed to assemble one file node without reopening the file.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedFile {
    pub rel_path: PathBuf,
    pub lines: u64,
    pub bytes: u64,
    /// Stable FNV-1a hash of retained source contents. Large or unsupported
    /// files that are only stream-counted intentionally have no fingerprint.
    pub source_fingerprint: Option<u64>,
    pub parsed: ParsedFile,
    pub chunks: Option<Vec<SymbolNode>>,
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

/// Assign same-name ordinals while retaining the caller's source order.
pub(crate) fn finalize_children_in_source_order(children: &mut [SymbolNode]) {
    let mut next_ordinals = BTreeMap::<String, u16>::new();
    for child in children {
        let next = next_ordinals.entry(child.name.clone()).or_default();
        child.id.ordinal = *next;
        *next += 1;
    }
}

/// Enforce tree-wide `SymbolId` uniqueness (spec §4.1: the ID is the stable
/// identity used by layout keys). `finalize_children` disambiguates only
/// within one sibling group; same-named children of same-named containers
/// (e.g. cfg-gated duplicate `mod` blocks) still collide across scopes.
/// Deterministic pre-order walk: on a repeated `(kind, qualified_path)`,
/// bump the ordinal to the next unseen value. Within-scope relative order
/// is preserved because visit order is deterministic and bumps are monotonic.
pub fn dedupe_ids(root: &mut SymbolNode) {
    fn walk(node: &mut SymbolNode, seen: &mut BTreeMap<(SymbolKind, String), u16>) {
        let next = seen
            .entry((node.id.kind.clone(), node.id.qualified_path.clone()))
            .or_insert(0);
        if node.id.ordinal < *next {
            node.id.ordinal = *next;
        }
        *next = node.id.ordinal + 1;
        for child in &mut node.children {
            walk(child, seen);
        }
    }
    walk(root, &mut BTreeMap::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(name: &str) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Item {
                    label: "impl".into(),
                },
                qualified_path: format!("f.rs::{name}"),
                ordinal: 0,
            },
            name: name.to_string(),
            byte_range: None,
            signature: None,
            doc: None,
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

    #[test]
    fn item_kind_serde_roundtrip() {
        let tree = SymbolTree {
            root: SymbolNode {
                id: SymbolId {
                    kind: SymbolKind::Item { label: "fn".into() },
                    qualified_path: "f.rs::main".into(),
                    ordinal: 0,
                },
                name: "main".to_string(),
                byte_range: None,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                children: vec![],
            },
            repo_root: std::path::PathBuf::from("/tmp/x"),
        };
        let json = serde_json::to_string(&tree).unwrap();
        let back: SymbolTree = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
    }

    #[test]
    fn dedupe_ids_disambiguates_cross_scope_duplicates() {
        // Simulates two cfg-gated `mod imp` blocks, each containing `fn connect`.
        let mk_mod = || SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Item {
                    label: "module".into(),
                },
                qualified_path: "net.rs::imp".into(),
                ordinal: 0,
            },
            name: "imp".into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 2,
            churn: 0.0,
            churn_count: 0,
            children: vec![SymbolNode {
                id: SymbolId {
                    kind: SymbolKind::Item { label: "fn".into() },
                    qualified_path: "net.rs::imp::connect".into(),
                    ordinal: 0,
                },
                name: "connect".into(),
                byte_range: None,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                children: vec![],
            }],
        };
        let mut file = mk("net.rs");
        file.children = vec![mk_mod(), mk_mod()];
        finalize_children(&mut file.children);
        // finalize gives the mods ordinals 0,1 — but both `connect` fns still collide
        assert_eq!(
            file.children[0].children[0].id,
            file.children[1].children[0].id
        );

        dedupe_ids(&mut file);
        let a = &file.children[0].children[0].id;
        let b = &file.children[1].children[0].id;
        assert_ne!(a, b);
        assert_eq!((a.ordinal, b.ordinal), (0, 1));
    }
}
