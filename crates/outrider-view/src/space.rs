//! Space planning: exclusion + regroup → the tree handed to the packer.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::hash::Hash;

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

use crate::partition::{Partition, PartitionRegistry};
use crate::spec::{PackOverrides, SpaceKind, SpaceSpec};

// ── Types ──

/// Resolved pack config: gap size and optional line cap.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EffectivePack {
    pub gap: f64,
    pub max_display_lines: Option<u64>,
}

/// Cheap identity for repack decisions — two plans with the same key
/// produce identical layout and can share cached geometry.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SpaceKey {
    pub space_id: String,
    pub exclude_hash: u64,
    pub gap_bits: u64,
    pub max_display_lines: Option<u64>,
}

/// Everything the packer needs to lay out one space.
pub struct SpacePlan<'a> {
    pub space_id: String,
    pub tree: Cow<'a, SymbolTree>,
    pub excluded: HashSet<SymbolId>,
    pub pack: EffectivePack,
    pub warnings: Vec<String>,
}

/// Result of `plan_space`: either a ready plan or an unsupported kind.
pub enum SpaceOutcome<'a> {
    Ready(SpacePlan<'a>),
    Unsupported { kind: SpaceKind },
}

// ── Functions ──

/// Apply overrides on top of defaults.
pub fn effective_pack(overrides: Option<&PackOverrides>, defaults: EffectivePack) -> EffectivePack {
    match overrides {
        None => defaults,
        Some(ov) => EffectivePack {
            gap: ov.gap.unwrap_or(defaults.gap),
            max_display_lines: ov.max_display_lines.or(defaults.max_display_lines),
        },
    }
}

/// Compute a cheap identity key for a space spec.
pub fn space_key(spec: &SpaceSpec, defaults: EffectivePack) -> SpaceKey {
    let space_id = match &spec.regroup {
        None => "treemap".to_string(),
        Some(pr) => format!("treemap@{}", pr.partition),
    };
    let ep = effective_pack(spec.pack.as_ref(), defaults);
    SpaceKey {
        space_id,
        exclude_hash: 0,
        gap_bits: ep.gap.to_bits(),
        max_display_lines: ep.max_display_lines,
    }
}

/// Remove every node in `excluded` (and its subtree) from the tree.
///
/// Folders left with no children are removed too.
/// The root is never removed. Files with no remaining items stay (they are leaves).
pub fn prune_tree(tree: &SymbolTree, excluded: &HashSet<SymbolId>) -> SymbolTree {
    let mut pruned = tree.clone();
    pruned.root.children = prune_children(&pruned.root.children, excluded);
    pruned
}

fn prune_children(children: &[SymbolNode], excluded: &HashSet<SymbolId>) -> Vec<SymbolNode> {
    let mut result = Vec::new();
    for child in children {
        if excluded.contains(&child.id) {
            continue;
        }
        let mut node = child.clone();
        node.children = prune_children(&child.children, excluded);
        // Remove folders left with no children, but keep files (they are leaves).
        if node.id.kind == SymbolKind::Folder && node.children.is_empty() {
            continue;
        }
        result.push(node);
    }
    result
}

/// Rebuild hierarchy: root → one Folder per partition group → original file subtrees.
///
/// Files not assigned to any group go under `@<partition>/(unassigned)`.
/// Group measure = sum of member measures, churn = 0.
/// Original leaf ids are preserved exactly.
pub fn regroup_tree(tree: &SymbolTree, partition: &Partition) -> SymbolTree {
    // Collect all file-level nodes from the original tree.
    let mut file_nodes: BTreeMap<SymbolId, SymbolNode> = BTreeMap::new();
    collect_files(&tree.root, &mut file_nodes);

    // Collect unique group names in order, preserving partition order if available.
    let group_names: Vec<String> = {
        if let Some(ref order) = partition.order {
            let mut names: Vec<String> = order.clone();
            // Add any groups present in the data but not in the explicit order.
            let ordered_set: HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
            let mut extra: Vec<String> = partition
                .groups
                .values()
                .filter(|g| !ordered_set.contains(g.as_str()))
                .cloned()
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            extra.sort();
            names.extend(extra);
            names
        } else {
            let mut names: Vec<String> = partition
                .groups
                .values()
                .cloned()
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            names.sort();
            names
        }
    };

    // Build group folders.
    let mut group_children: BTreeMap<String, Vec<SymbolNode>> = BTreeMap::new();
    for name in &group_names {
        group_children.insert(name.clone(), Vec::new());
    }

    let mut assigned: HashSet<SymbolId> = HashSet::new();
    for (file_id, group_name) in &partition.groups {
        if let Some(node) = file_nodes.get(file_id) {
            group_children
                .entry(group_name.clone())
                .or_default()
                .push(node.clone());
            assigned.insert(file_id.clone());
        }
    }

    // Unassigned files.
    let unassigned_name = "(unassigned)".to_string();
    let mut unassigned_nodes: Vec<SymbolNode> = Vec::new();
    for (file_id, node) in &file_nodes {
        if !assigned.contains(file_id) {
            unassigned_nodes.push(node.clone());
        }
    }

    // Assemble group folder nodes.
    let mut root_children: Vec<SymbolNode> = Vec::new();
    for name in &group_names {
        if let Some(members) = group_children.get(name) {
            if members.is_empty() {
                continue;
            }
            let measure: u64 = members.iter().map(|n| n.measure).sum();
            root_children.push(SymbolNode {
                id: SymbolId {
                    kind: SymbolKind::Folder,
                    qualified_path: format!("@{}/{}", partition.name, name),
                    ordinal: 0,
                },
                name: name.clone(),
                byte_range: None,
                signature: None,
                doc: None,
                measure,
                churn: 0.0,
                churn_count: 0,
                children: members.clone(),
            });
        }
    }

    if !unassigned_nodes.is_empty() {
        let measure: u64 = unassigned_nodes.iter().map(|n| n.measure).sum();
        root_children.push(SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Folder,
                qualified_path: format!("@{}/{}", partition.name, unassigned_name),
                ordinal: 0,
            },
            name: unassigned_name,
            byte_range: None,
            signature: None,
            doc: None,
            measure,
            churn: 0.0,
            churn_count: 0,
            children: unassigned_nodes,
        });
    }

    let root_measure: u64 = root_children.iter().map(|n| n.measure).sum();
    SymbolTree {
        root: SymbolNode {
            id: tree.root.id.clone(),
            name: tree.root.name.clone(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: root_measure,
            churn: 0.0,
            churn_count: 0,
            children: root_children,
        },
        repo_root: tree.repo_root.clone(),
    }
}

/// Recursively collect all File nodes from the tree.
fn collect_files(node: &SymbolNode, out: &mut BTreeMap<SymbolId, SymbolNode>) {
    if node.id.kind == SymbolKind::File {
        out.insert(node.id.clone(), node.clone());
    }
    for child in &node.children {
        collect_files(child, out);
    }
}

/// Plan a single space from a spec, tree, partition registry, and
/// pre-resolved exclusion set.
pub fn plan_space<'a>(
    spec: &SpaceSpec,
    tree: &'a SymbolTree,
    partitions: &PartitionRegistry,
    excluded: Option<&HashSet<SymbolId>>,
    defaults: EffectivePack,
) -> SpaceOutcome<'a> {
    match spec.kind {
        SpaceKind::Callgraph | SpaceKind::Matrix => {
            return SpaceOutcome::Unsupported { kind: spec.kind };
        }
        _ => {}
    }

    let mut warnings: Vec<String> = Vec::new();

    // Determine the excluded set.
    let excluded_set = match excluded {
        Some(ex) => ex.clone(),
        None => HashSet::new(),
    };

    // Apply pruning if there are exclusions.
    let pruned: Option<SymbolTree> = if !excluded_set.is_empty() {
        Some(prune_tree(tree, &excluded_set))
    } else {
        None
    };

    let working_tree: Cow<'a, SymbolTree> = match &pruned {
        Some(t) => Cow::Owned(t.clone()),
        None => Cow::Borrowed(tree),
    };

    // Apply regrouping if specified.
    let final_tree: Cow<'a, SymbolTree> = match &spec.regroup {
        Some(pr) => {
            match partitions.get(&pr.partition) {
                Some(partition) => Cow::Owned(regroup_tree(&working_tree, partition)),
                None => {
                    warnings.push(format!("partition '{}' not found", pr.partition));
                    working_tree
                }
            }
        }
        None => working_tree,
    };

    let space_id = match &spec.regroup {
        None => "treemap".to_string(),
        Some(pr) => format!("treemap@{}", pr.partition),
    };

    let pack = effective_pack(spec.pack.as_ref(), defaults);

    SpaceOutcome::Ready(SpacePlan {
        space_id,
        tree: final_tree,
        excluded: excluded_set,
        pack,
        warnings,
    })
}

// ── Helpers ──

/// Collect all leaf ids from a tree (files and items, not folders).
fn collect_leaf_ids(node: &SymbolNode, out: &mut HashSet<SymbolId>) {
    if node.children.is_empty() && node.id.kind != SymbolKind::Folder {
        out.insert(node.id.clone());
    }
    // Also include files that have children (they contain items).
    if node.id.kind == SymbolKind::File {
        out.insert(node.id.clone());
    }
    for child in &node.children {
        collect_leaf_ids(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    fn make_tree(paths: &[&str]) -> SymbolTree {
        let mut top_folders: BTreeMap<String, Vec<SymbolNode>> = BTreeMap::new();
        let mut root_files: Vec<SymbolNode> = Vec::new();

        for &path in paths {
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            if parts.len() == 1 {
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
            repo_root: PathBuf::from("."),
        }
    }

    fn file_id(qp: &str) -> SymbolId {
        SymbolId {
            kind: SymbolKind::File,
            qualified_path: qp.to_string(),
            ordinal: 0,
        }
    }

    fn defaults() -> EffectivePack {
        EffectivePack {
            gap: 2.0,
            max_display_lines: None,
        }
    }

    // ── prune_tree tests ──

    #[test]
    fn prune_removes_excluded_and_descendants() {
        let tree = make_tree(&["src/a.rs", "src/b.rs"]);
        let mut excluded = HashSet::new();
        excluded.insert(file_id("src/a.rs"));
        let pruned = prune_tree(&tree, &excluded);

        // src folder should still exist with only b.rs.
        let src = pruned
            .root
            .children
            .iter()
            .find(|n| n.name == "src")
            .expect("src folder should remain");
        assert_eq!(src.children.len(), 1);
        assert_eq!(src.children[0].id.qualified_path, "src/b.rs");
    }

    #[test]
    fn prune_removes_empty_folders() {
        let tree = make_tree(&["src/a.rs"]);
        let mut excluded = HashSet::new();
        excluded.insert(file_id("src/a.rs"));
        let pruned = prune_tree(&tree, &excluded);

        // src folder should be removed since it has no children left.
        assert!(
            pruned.root.children.is_empty(),
            "root should have no children after pruning only file in folder"
        );
    }

    #[test]
    fn prune_preserves_root() {
        let tree = make_tree(&["src/a.rs", "README.md"]);
        let mut excluded = HashSet::new();
        excluded.insert(file_id("src/a.rs"));
        excluded.insert(file_id("README.md"));
        let pruned = prune_tree(&tree, &excluded);

        // Root should still exist even though all children are gone.
        assert_eq!(pruned.root.id.kind, SymbolKind::Folder);
        assert!(pruned.root.children.is_empty());
    }

    // ── regroup_tree tests ──

    #[test]
    fn regroup_preserves_leaf_ids() {
        let tree = make_tree(&["src/lib.rs", "src/util.rs", "README.md"]);
        let reg = PartitionRegistry::builtin(&tree);
        let partition = reg.get("topFolder").unwrap();

        // Collect leaf ids from original.
        let mut original_ids = HashSet::new();
        collect_leaf_ids(&tree.root, &mut original_ids);

        let regrouped = regroup_tree(&tree, partition);
        let mut regrouped_ids = HashSet::new();
        collect_leaf_ids(&regrouped.root, &mut regrouped_ids);

        assert_eq!(original_ids, regrouped_ids);
    }

    #[test]
    fn regroup_creates_group_folders() {
        let tree = make_tree(&["src/lib.rs", "src/util.rs", "README.md"]);
        let reg = PartitionRegistry::builtin(&tree);
        let partition = reg.get("topFolder").unwrap();

        let regrouped = regroup_tree(&tree, partition);

        // All immediate children of root should be group folders with @topFolder/ prefix.
        for child in &regrouped.root.children {
            assert_eq!(child.id.kind, SymbolKind::Folder);
            assert!(
                child.id.qualified_path.starts_with("@topFolder/"),
                "group folder path '{}' should start with '@topFolder/'",
                child.id.qualified_path
            );
        }
    }

    // ── plan_space tests ──

    #[test]
    fn plan_space_default_is_borrowed() {
        let tree = make_tree(&["src/lib.rs"]);
        let partitions = PartitionRegistry::default();
        let spec = SpaceSpec::default();
        let outcome = plan_space(&spec, &tree, &partitions, None, defaults());

        match outcome {
            SpaceOutcome::Ready(plan) => {
                assert!(
                    matches!(plan.tree, Cow::Borrowed(_)),
                    "default plan should borrow the tree"
                );
            }
            SpaceOutcome::Unsupported { .. } => panic!("default should be supported"),
        }
    }

    #[test]
    fn plan_space_unsupported_returns_unsupported() {
        let tree = make_tree(&["src/lib.rs"]);
        let partitions = PartitionRegistry::default();

        let callgraph_spec = SpaceSpec {
            kind: SpaceKind::Callgraph,
            ..SpaceSpec::default()
        };
        assert!(matches!(
            plan_space(&callgraph_spec, &tree, &partitions, None, defaults()),
            SpaceOutcome::Unsupported {
                kind: SpaceKind::Callgraph
            }
        ));

        let matrix_spec = SpaceSpec {
            kind: SpaceKind::Matrix,
            ..SpaceSpec::default()
        };
        assert!(matches!(
            plan_space(&matrix_spec, &tree, &partitions, None, defaults()),
            SpaceOutcome::Unsupported {
                kind: SpaceKind::Matrix
            }
        ));
    }

    // ── effective_pack tests ──

    #[test]
    fn effective_pack_applies_overrides() {
        let d = EffectivePack {
            gap: 2.0,
            max_display_lines: Some(100),
        };
        let ov = PackOverrides {
            gap: Some(5.0),
            max_display_lines: None,
        };
        let ep = effective_pack(Some(&ov), d);
        assert_eq!(ep.gap, 5.0);
        assert_eq!(ep.max_display_lines, Some(100));
    }

    // ── space_key tests ──

    #[test]
    fn space_key_stable() {
        let spec = SpaceSpec::default();
        let d = defaults();
        let k1 = space_key(&spec, d);
        let k2 = space_key(&spec, d);
        assert_eq!(k1, k2);
    }

    #[test]
    fn space_key_sensitive_to_gap() {
        let spec = SpaceSpec::default();
        let d1 = EffectivePack {
            gap: 2.0,
            max_display_lines: None,
        };
        let d2 = EffectivePack {
            gap: 5.0,
            max_display_lines: None,
        };
        let k1 = space_key(&spec, d1);
        let k2 = space_key(&spec, d2);
        assert_ne!(k1, k2);
    }
}
