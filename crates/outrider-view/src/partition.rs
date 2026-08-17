//! Partition registry: maps `SymbolId`s to named groups.

use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

use crate::deps::Deps;

/// A partition maps every member `SymbolId` to a named group string.
pub struct Partition {
    pub name: String,
    /// member -> group name
    pub groups: BTreeMap<SymbolId, String>,
    /// Ordered group names (meaningful for layer partitions).
    pub order: Option<Vec<String>>,
    /// Provenance description: how this partition was derived.
    pub basis: String,
    /// Which session inputs this partition depends on.
    pub deps: Deps,
}

impl Partition {
    /// Returns the group name that `id` belongs to, if any.
    pub fn group_of(&self, id: &SymbolId) -> Option<&str> {
        self.groups.get(id).map(|s| s.as_str())
    }

    /// Iterates over all member ids that belong to `group`.
    pub fn members<'a>(&'a self, group: &'a str) -> impl Iterator<Item = &'a SymbolId> + 'a {
        self.groups
            .iter()
            .filter_map(move |(id, g)| if g == group { Some(id) } else { None })
    }
}

/// Registry of named partitions, with a version counter for change detection.
#[derive(Default)]
pub struct PartitionRegistry {
    parts: BTreeMap<String, Partition>,
    pub version: u64,
}

impl PartitionRegistry {
    /// Look up a partition by name.
    pub fn get(&self, name: &str) -> Option<&Partition> {
        self.parts.get(name)
    }

    /// Insert (or replace) a partition, bumping the version counter.
    pub fn insert(&mut self, p: Partition) {
        self.parts.insert(p.name.clone(), p);
        self.version += 1;
    }

    /// Build the default partitions from a `SymbolTree`.
    ///
    /// Currently creates a single `"topFolder"` partition that maps every
    /// `File` node to its first path component (e.g. `"src/lib.rs"` -> `"src"`,
    /// `"README.md"` -> `"(root)"`).
    pub fn builtin(tree: &SymbolTree) -> PartitionRegistry {
        let mut groups = BTreeMap::new();
        Self::walk_top_folder(&tree.root, &mut groups);
        let mut reg = PartitionRegistry::default();
        reg.insert(Partition {
            name: "topFolder".into(),
            groups,
            order: None,
            basis: "first path component of each file".into(),
            deps: Deps::NONE,
        });
        reg
    }

    fn walk_top_folder(node: &SymbolNode, groups: &mut BTreeMap<SymbolId, String>) {
        if node.id.kind == SymbolKind::File {
            let path = &node.id.qualified_path;
            let top = path.split('/').next().unwrap_or("");
            let group = if top == path {
                // No slash -- file lives at root level.
                "(root)".to_string()
            } else {
                top.to_string()
            };
            groups.insert(node.id.clone(), group);
        }
        for child in &node.children {
            Self::walk_top_folder(child, groups);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_node(qp: &str, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::File,
                qualified_path: qp.to_string(),
                ordinal: 0,
            },
            name: name.to_string(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    fn folder_node(qp: &str, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Folder,
                qualified_path: qp.to_string(),
                ordinal: 0,
            },
            name: name.to_string(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 0,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    /// Build a tree with a flat list of files under a root folder.
    fn make_tree(paths: &[&str]) -> SymbolTree {
        // Group files by first path component to build folder structure.
        let mut top_folders: BTreeMap<String, Vec<SymbolNode>> = BTreeMap::new();
        let mut root_files: Vec<SymbolNode> = Vec::new();

        for &path in paths {
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            if parts.len() == 1 {
                // Root-level file.
                root_files.push(file_node(path, path, vec![]));
            } else {
                let folder = parts[0].to_string();
                let file_name = parts[1];
                top_folders
                    .entry(folder)
                    .or_default()
                    .push(file_node(path, file_name, vec![]));
            }
        }

        let mut children = Vec::new();
        for (folder_name, files) in top_folders {
            children.push(folder_node(&folder_name, &folder_name, files));
        }
        children.extend(root_files);

        let root = folder_node("", "root", children);
        SymbolTree {
            root,
            repo_root: std::path::PathBuf::from("."),
        }
    }

    #[test]
    fn top_folder_maps_files_to_first_component() {
        let tree = make_tree(&["src/lib.rs", "src/util.rs", "README.md"]);
        let reg = PartitionRegistry::builtin(&tree);
        let p = reg.get("topFolder").expect("topFolder partition missing");

        let src_lib = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/lib.rs".to_string(),
            ordinal: 0,
        };
        let src_util = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/util.rs".to_string(),
            ordinal: 0,
        };
        let readme = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "README.md".to_string(),
            ordinal: 0,
        };

        assert_eq!(p.group_of(&src_lib), Some("src"));
        assert_eq!(p.group_of(&src_util), Some("src"));
        assert_eq!(p.group_of(&readme), Some("(root)"));
    }

    #[test]
    fn partition_members_query() {
        let tree = make_tree(&["src/lib.rs", "src/util.rs", "README.md"]);
        let reg = PartitionRegistry::builtin(&tree);
        let p = reg.get("topFolder").unwrap();

        let src_members: Vec<&SymbolId> = p.members("src").collect();
        assert_eq!(src_members.len(), 2);
        for id in &src_members {
            assert!(id.qualified_path.starts_with("src/"));
        }

        let root_members: Vec<&SymbolId> = p.members("(root)").collect();
        assert_eq!(root_members.len(), 1);
        assert_eq!(root_members[0].qualified_path, "README.md");
    }

    #[test]
    fn insert_bumps_version() {
        let mut reg = PartitionRegistry::default();
        assert_eq!(reg.version, 0);

        reg.insert(Partition {
            name: "a".into(),
            groups: BTreeMap::new(),
            order: None,
            basis: "test".into(),
            deps: Deps::NONE,
        });
        assert_eq!(reg.version, 1);

        reg.insert(Partition {
            name: "b".into(),
            groups: BTreeMap::new(),
            order: None,
            basis: "test".into(),
            deps: Deps::NONE,
        });
        assert_eq!(reg.version, 2);
    }
}
