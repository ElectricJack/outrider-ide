//! Set expression resolution.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::Path;

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree, TreeIndex};

use crate::deps::Deps;
use crate::metric::MetricRegistry;
use crate::partition::PartitionRegistry;
use crate::relation::RelationRegistry;
use crate::resolve::SessionState;
use crate::spec::{CmpOp, Depth, ReachDirection, SetRef, WhereValue};
use crate::symbol_id;

/// A resolved set of symbol IDs (plus optional byte ranges per symbol).
#[derive(Debug, Clone, Default)]
pub struct ResolvedSet {
    pub ids: HashSet<SymbolId>,
    pub ranges: BTreeMap<SymbolId, Vec<std::ops::Range<usize>>>,
    pub deps: Deps,
}

impl ResolvedSet {
    pub fn contains(&self, id: &SymbolId) -> bool {
        self.ids.contains(id)
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// Index from bare paths to SymbolIds for fast resolution.
pub struct PathIndex {
    by_path: BTreeMap<String, Vec<SymbolId>>,
}

impl PathIndex {
    pub fn new(tree: &outrider_index::SymbolTree) -> Self {
        let mut by_path = BTreeMap::new();
        fn walk(node: &outrider_index::SymbolNode, idx: &mut BTreeMap<String, Vec<SymbolId>>) {
            idx.entry(node.id.qualified_path.clone())
                .or_insert_with(Vec::new)
                .push(node.id.clone());
            for c in &node.children {
                walk(c, idx);
            }
        }
        walk(&tree.root, &mut by_path);
        PathIndex { by_path }
    }

    /// Look up a bare path (no kind prefix). Returns all matching IDs.
    pub fn lookup(&self, path: &str) -> &[SymbolId] {
        self.by_path.get(path).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Additional context for resolving advanced set expressions (where, reach,
/// community, layer, changed).
pub struct SetCtx<'a> {
    pub metrics: &'a MetricRegistry,
    pub relations: &'a RelationRegistry,
    pub partitions: &'a PartitionRegistry,
    pub repo_root: &'a Path,
}

/// Pre-order walk of every node in a subtree, invoking `f` on each node
/// (including `node` itself).
fn walk_tree(node: &SymbolNode, f: &mut impl FnMut(&SymbolNode)) {
    f(node);
    for child in &node.children {
        walk_tree(child, f);
    }
}

/// Subsequence match on `name`, case-insensitive (palette-style fuzzy match).
/// An empty query matches everything.
fn fuzzy_match(query: &str, name: &str) -> bool {
    let mut name_chars = name.chars().flat_map(|c| c.to_lowercase());
    for qc in query.chars().flat_map(|c| c.to_lowercase()) {
        loop {
            match name_chars.next() {
                Some(nc) if nc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// Resolve a single wire-format ID string to a SymbolId.
/// Handles pseudo-ids ($focus, $hover, $selection) and wire-format parsing.
pub fn resolve_wire_id(
    s: &str,
    session: &SessionState,
    path_index: &PathIndex,
    warnings: &mut Vec<String>,
) -> Option<SymbolId> {
    match s {
        "$focus" => Some(session.focus.clone()),
        "$hover" => session.hover.cloned(),
        "$selection" => session.selection.cloned(),
        _ => {
            // Try wire format first (has kind prefix with colon)
            if s.contains(':') {
                match symbol_id::parse_wire(s) {
                    Ok(id) => Some(id),
                    Err(e) => {
                        warnings.push(format!("invalid wire id '{s}': {e}"));
                        None
                    }
                }
            } else {
                // Bare path
                let matches = path_index.lookup(s);
                match matches.len() {
                    0 => {
                        warnings.push(format!("bare path '{s}' not found"));
                        None
                    }
                    1 => Some(matches[0].clone()),
                    _ => {
                        warnings.push(format!(
                            "bare path '{s}' is ambiguous ({} matches)",
                            matches.len()
                        ));
                        Some(matches[0].clone())
                    }
                }
            }
        }
    }
}

/// Resolve a `SetExpr` to a `ResolvedSet`.
#[allow(clippy::too_many_arguments)]
pub fn resolve_set_expr(
    expr: &crate::spec::SetExpr,
    named_sets: &BTreeMap<String, ResolvedSet>,
    session: &SessionState,
    path_index: &PathIndex,
    warnings: &mut Vec<String>,
    resolve_stack: &mut Vec<String>,
    all_sets: &BTreeMap<String, crate::spec::SetExpr>,
    tree: &SymbolTree,
    sctx: &SetCtx<'_>,
) -> ResolvedSet {
    use crate::spec::SetExpr;
    match expr {
        SetExpr::Ids(ids) => {
            let mut result = ResolvedSet::default();
            let mut deps = Deps::NONE;
            for s in ids {
                match s.as_str() {
                    "$focus" => deps = deps.union(Deps::FOCUS),
                    "$hover" => deps = deps.union(Deps::HOVER),
                    "$selection" => deps = deps.union(Deps::SELECTION),
                    _ => {}
                }
                if let Some(id) = resolve_wire_id(s, session, path_index, warnings) {
                    result.ids.insert(id);
                }
            }
            result.deps = deps;
            result
        }
        SetExpr::Neighbors(target) => {
            let mut result = ResolvedSet::default();
            if target == "focus" || target == "$focus" {
                if let Some(neighbors) = session.neighbors {
                    for n in neighbors.iter().flatten() {
                        result.ids.insert(n.clone());
                    }
                }
                result.deps = Deps::FOCUS;
            } else {
                warnings.push(format!(
                    "neighbors: unsupported target '{target}', only 'focus' is supported"
                ));
            }
            result
        }
        SetExpr::Union(exprs) => {
            let mut result = ResolvedSet::default();
            for sub in exprs {
                let resolved = resolve_set_expr(
                    sub,
                    named_sets,
                    session,
                    path_index,
                    warnings,
                    resolve_stack,
                    all_sets,
                    tree,
                    sctx,
                );
                result.ids.extend(resolved.ids);
                result.deps = result.deps.union(resolved.deps);
            }
            result
        }
        SetExpr::Ref(name) => {
            if resolve_stack.contains(name) {
                warnings.push(format!(
                    "set cycle detected: {}",
                    resolve_stack.join(" -> ")
                ));
                return ResolvedSet::default();
            }
            if let Some(resolved) = named_sets.get(name) {
                return resolved.clone();
            }
            if let Some(expr) = all_sets.get(name) {
                resolve_stack.push(name.clone());
                let result = resolve_set_expr(
                    expr,
                    named_sets,
                    session,
                    path_index,
                    warnings,
                    resolve_stack,
                    all_sets,
                    tree,
                    sctx,
                );
                resolve_stack.pop();
                result
            } else {
                warnings.push(format!("unknown set '{name}'"));
                ResolvedSet::default()
            }
        }
        SetExpr::Glob(pattern) => {
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE;
            match globset::GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
            {
                Ok(glob) => {
                    let matcher = glob.compile_matcher();
                    walk_tree(&tree.root, &mut |node| {
                        if matcher.is_match(&node.id.qualified_path) {
                            result.ids.insert(node.id.clone());
                        }
                    });
                }
                Err(e) => {
                    warnings.push(format!("invalid glob '{pattern}': {e}"));
                }
            }
            result
        }
        SetExpr::Kind(kind) => {
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE;
            walk_tree(&tree.root, &mut |node| {
                if node.id.kind.label() == kind.as_str() {
                    result.ids.insert(node.id.clone());
                }
            });
            result
        }
        SetExpr::Fuzzy(query) => {
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE;
            walk_tree(&tree.root, &mut |node| {
                if fuzzy_match(query, &node.name) {
                    result.ids.insert(node.id.clone());
                }
            });
            result
        }
        SetExpr::Intersect(exprs) => {
            let mut result = ResolvedSet::default();
            let mut acc: Option<HashSet<SymbolId>> = None;
            for sub in exprs {
                let resolved = resolve_set_expr(
                    sub,
                    named_sets,
                    session,
                    path_index,
                    warnings,
                    resolve_stack,
                    all_sets,
                    tree,
                    sctx,
                );
                result.deps = result.deps.union(resolved.deps);
                acc = Some(match acc {
                    None => resolved.ids,
                    Some(prev) => prev.intersection(&resolved.ids).cloned().collect(),
                });
            }
            result.ids = acc.unwrap_or_default();
            result
        }
        SetExpr::Diff(pair) => {
            let [base_expr, remove_expr] = pair.as_ref();
            let base = resolve_set_expr(
                base_expr,
                named_sets,
                session,
                path_index,
                warnings,
                resolve_stack,
                all_sets,
                tree,
                sctx,
            );
            let remove = resolve_set_expr(
                remove_expr,
                named_sets,
                session,
                path_index,
                warnings,
                resolve_stack,
                all_sets,
                tree,
                sctx,
            );
            let mut result = ResolvedSet::default();
            result.deps = base.deps.union(remove.deps);
            result.ids = base.ids.difference(&remove.ids).cloned().collect();
            result
        }
        SetExpr::Not(sub) => {
            let resolved = resolve_set_expr(
                sub,
                named_sets,
                session,
                path_index,
                warnings,
                resolve_stack,
                all_sets,
                tree,
                sctx,
            );
            let mut all_ids = HashSet::new();
            walk_tree(&tree.root, &mut |node| {
                all_ids.insert(node.id.clone());
            });
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE.union(resolved.deps);
            result.ids = all_ids.difference(&resolved.ids).cloned().collect();
            result
        }
        SetExpr::Ancestors(set_ref) => {
            let resolved = resolve_set_ref(
                set_ref,
                named_sets,
                session,
                path_index,
                warnings,
                resolve_stack,
                all_sets,
                tree,
                sctx,
            );
            let index = TreeIndex::new(tree);
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE.union(resolved.deps);
            for id in &resolved.ids {
                let mut cur = id;
                while let Some(parent) = index.parent(cur) {
                    result.ids.insert(parent.clone());
                    cur = parent;
                }
            }
            result
        }
        SetExpr::Descendants(set_ref) => {
            let resolved = resolve_set_ref(
                set_ref,
                named_sets,
                session,
                path_index,
                warnings,
                resolve_stack,
                all_sets,
                tree,
                sctx,
            );
            let index = TreeIndex::new(tree);
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE.union(resolved.deps);
            for id in &resolved.ids {
                if let Some(node) = index.node(id) {
                    for child in &node.children {
                        walk_tree(child, &mut |n| {
                            result.ids.insert(n.id.clone());
                        });
                    }
                }
            }
            result
        }
        SetExpr::Children(set_ref) => {
            let resolved = resolve_set_ref(
                set_ref, named_sets, session, path_index, warnings,
                resolve_stack, all_sets, tree, sctx,
            );
            let index = TreeIndex::new(tree);
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE.union(resolved.deps);
            for id in &resolved.ids {
                if let Some(node) = index.node(id) {
                    for child in &node.children {
                        result.ids.insert(child.id.clone());
                    }
                }
            }
            result
        }
        SetExpr::FileOf(set_ref) => {
            let resolved = resolve_set_ref(
                set_ref, named_sets, session, path_index, warnings,
                resolve_stack, all_sets, tree, sctx,
            );
            let index = TreeIndex::new(tree);
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE.union(resolved.deps);
            for id in &resolved.ids {
                if matches!(id.kind, SymbolKind::File) {
                    result.ids.insert(id.clone());
                    continue;
                }
                let mut cur = id;
                while let Some(parent_id) = index.parent(cur) {
                    if matches!(parent_id.kind, SymbolKind::File) {
                        result.ids.insert(parent_id.clone());
                        break;
                    }
                    cur = parent_id;
                }
            }
            result
        }
        SetExpr::Community(pm) | SetExpr::Layer(pm) => {
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE;
            if let Some(partition) = sctx.partitions.get(&pm.partition) {
                for id in partition.members(&pm.id) {
                    result.ids.insert(id.clone());
                }
                result.deps = result.deps.union(partition.deps);
            } else {
                warnings.push(format!("unknown partition '{}'", pm.partition));
            }
            result
        }
        SetExpr::Visible(_) => {
            let mut result = ResolvedSet::default();
            result.deps = Deps::CAMERA;
            if let Some(visible) = session.visible {
                for id in visible {
                    result.ids.insert(id.clone());
                }
            }
            result
        }
        SetExpr::Changed(spec_text) => {
            let mut result = ResolvedSet::default();
            result.deps = Deps::GIT.union(Deps::TREE);
            let spec = crate::git::parse_changed(spec_text);
            match crate::git::changed_files(sctx.repo_root, &spec) {
                Ok(changed) => {
                    let index = TreeIndex::new(tree);
                    for (path, _hunks) in &changed {
                        let file_ids = path_index.lookup(path);
                        for file_id in file_ids {
                            if let Some(file_node) = index.node(file_id) {
                                walk_tree(file_node, &mut |n| {
                                    result.ids.insert(n.id.clone());
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    warnings.push(format!("changed: {e}"));
                }
            }
            result
        }
        SetExpr::Where(w) => {
            let mut result = ResolvedSet::default();
            result.deps = Deps::METRICS.union(Deps::TREE);
            if let Some(provider) = sctx.metrics.get(&w.metric) {
                let threshold = match &w.value {
                    WhereValue::Number(n) => *n,
                    WhereValue::Percentile(p) => {
                        let pct: f64 = p[1..].parse().unwrap_or(50.0) / 100.0;
                        let mut values: Vec<f64> = Vec::new();
                        walk_tree(&tree.root, &mut |node| {
                            if let Some(v) = provider.value(node) {
                                values.push(v);
                            }
                        });
                        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        if values.is_empty() {
                            0.0
                        } else {
                            let idx = ((values.len() as f64 - 1.0) * pct).round() as usize;
                            values[idx.min(values.len() - 1)]
                        }
                    }
                };
                walk_tree(&tree.root, &mut |node| {
                    if let Some(v) = provider.value(node) {
                        let passes = match w.op {
                            CmpOp::Gt => v > threshold,
                            CmpOp::Ge => v >= threshold,
                            CmpOp::Lt => v < threshold,
                            CmpOp::Le => v <= threshold,
                            CmpOp::Eq => (v - threshold).abs() < f64::EPSILON,
                            CmpOp::Ne => (v - threshold).abs() >= f64::EPSILON,
                        };
                        if passes {
                            result.ids.insert(node.id.clone());
                        }
                    }
                });
            } else {
                warnings.push(format!("where: unknown metric '{}'", w.metric));
            }
            result
        }
        SetExpr::Reach(r) => {
            let seed = resolve_set_expr(
                &r.from, named_sets, session, path_index, warnings,
                resolve_stack, all_sets, tree, sctx,
            );
            let mut result = ResolvedSet::default();
            result.deps = Deps::TREE.union(Deps::RELATIONS).union(seed.deps);
            let max_depth = match r.depth {
                Depth::N(n) => n as usize,
                Depth::Inf(_) => usize::MAX,
            };
            if let Some(provider) = sctx.relations.get(&r.relation) {
                let index = TreeIndex::new(tree);
                let pctx = crate::relation::ProviderCtx {
                    tree,
                    index: &index,
                    repo_root: sctx.repo_root,
                };
                let mut visited: HashSet<SymbolId> = HashSet::new();
                let mut frontier: VecDeque<(SymbolId, usize)> = VecDeque::new();
                for id in &seed.ids {
                    visited.insert(id.clone());
                    frontier.push_back((id.clone(), 0));
                }
                while let Some((id, depth)) = frontier.pop_front() {
                    if depth >= max_depth {
                        continue;
                    }
                    let neighbors = match r.direction {
                        ReachDirection::Out => provider.out_edges(&id, &pctx),
                        ReachDirection::In => provider.in_edges(&id, &pctx),
                        ReachDirection::Both => {
                            let mut all = provider.out_edges(&id, &pctx);
                            all.extend(provider.in_edges(&id, &pctx));
                            all
                        }
                    };
                    for (neighbor_id, _weight) in neighbors {
                        if visited.insert(neighbor_id.clone()) {
                            frontier.push_back((neighbor_id, depth + 1));
                        }
                    }
                }
                result.ids = visited;
            } else {
                warnings.push(format!("reach: unknown relation '{}'", r.relation));
            }
            result
        }
    }
}

/// Resolve a `SetRef` (named reference or inline expression) to a `ResolvedSet`.
#[allow(clippy::too_many_arguments)]
fn resolve_set_ref(
    set_ref: &SetRef,
    named_sets: &BTreeMap<String, ResolvedSet>,
    session: &SessionState,
    path_index: &PathIndex,
    warnings: &mut Vec<String>,
    resolve_stack: &mut Vec<String>,
    all_sets: &BTreeMap<String, crate::spec::SetExpr>,
    tree: &SymbolTree,
    sctx: &SetCtx<'_>,
) -> ResolvedSet {
    match set_ref {
        SetRef::Name(name) => resolve_set_expr(
            &crate::spec::SetExpr::Ref(name.clone()),
            named_sets,
            session,
            path_index,
            warnings,
            resolve_stack,
            all_sets,
            tree,
            sctx,
        ),
        SetRef::Inline(expr) => resolve_set_expr(
            expr,
            named_sets,
            session,
            path_index,
            warnings,
            resolve_stack,
            all_sets,
            tree,
            sctx,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::SetExpr;
    use outrider_index::{SymbolKind, SymbolNode, SymbolTree};

    fn leaf(name: &str) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::File,
                qualified_path: name.to_string(),
                ordinal: 0,
            },
            name: name.to_string(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
            children: vec![],
        }
    }

    fn sample_tree() -> SymbolTree {
        let root = SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Folder,
                qualified_path: String::new(),
                ordinal: 0,
            },
            name: "root".to_string(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 0,
            churn: 0.0,
            churn_count: 0,
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
            children: vec![leaf("a.rs"), leaf("b.rs")],
        };
        SymbolTree {
            root,
            repo_root: std::path::PathBuf::from("."),
        }
    }

    fn test_sctx() -> SetCtx<'static> {
        use crate::metric::MetricRegistry;
        use crate::partition::PartitionRegistry;
        use crate::relation::RelationRegistry;
        static REPO: &str = ".";
        SetCtx {
            metrics: Box::leak(Box::new(MetricRegistry::builtin())),
            relations: Box::leak(Box::new(RelationRegistry::empty())),
            partitions: Box::leak(Box::new(PartitionRegistry::default())),
            repo_root: std::path::Path::new(REPO),
        }
    }

    #[test]
    fn resolves_focus_pseudo_id() {
        let tree = sample_tree();
        let path_index = PathIndex::new(&tree);
        let focus = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "a.rs".to_string(),
            ordinal: 0,
        };
        let session = SessionState {
            focus: &focus,
            hover: None,
            selection: None,
            visible: None,
            head: None,
            neighbors: None,
        };
        let mut warnings = Vec::new();
        let expr = SetExpr::Ids(vec!["$focus".to_string()]);
        let named = BTreeMap::new();
        let all_sets = BTreeMap::new();
        let mut stack = Vec::new();
        let sctx = test_sctx();
        let result = resolve_set_expr(
            &expr,
            &named,
            &session,
            &path_index,
            &mut warnings,
            &mut stack,
            &all_sets,
            &tree,
            &sctx,
        );
        assert!(result.ids.contains(&focus));
        assert!(result.deps.intersects(Deps::FOCUS));
    }

    #[test]
    fn resolves_bare_path() {
        let tree = sample_tree();
        let path_index = PathIndex::new(&tree);
        let focus = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "a.rs".to_string(),
            ordinal: 0,
        };
        let session = SessionState {
            focus: &focus,
            hover: None,
            selection: None,
            visible: None,
            head: None,
            neighbors: None,
        };
        let mut warnings = Vec::new();
        let expr = SetExpr::Ids(vec!["b.rs".to_string()]);
        let named = BTreeMap::new();
        let all_sets = BTreeMap::new();
        let mut stack = Vec::new();
        let sctx = test_sctx();
        let result = resolve_set_expr(
            &expr,
            &named,
            &session,
            &path_index,
            &mut warnings,
            &mut stack,
            &all_sets,
            &tree,
            &sctx,
        );
        assert_eq!(result.ids.len(), 1);
        assert!(warnings.is_empty());
    }

    // ── nested fixture for glob/kind/intersect/diff/not/ancestors/descendants/fuzzy ──

    fn item(qp: &str, name: &str, label: &str) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Item {
                    label: label.to_string(),
                },
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
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
            children: vec![],
        }
    }

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
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
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
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
            children,
        }
    }

    fn id_of(node: &SymbolNode) -> SymbolId {
        node.id.clone()
    }

    /// root
    ///  ├─ src/            (folder)
    ///  │   ├─ src/a.rs    (file)
    ///  │   │   └─ src/a.rs::foo  (fn item)
    ///  │   └─ src/b.rs    (file)
    ///  └─ README.md       (file)
    fn nested_tree() -> SymbolTree {
        let foo = item("src/a.rs::foo", "foo", "fn");
        let a_rs = file_node("src/a.rs", "a.rs", vec![foo]);
        let b_rs = file_node("src/b.rs", "b.rs", vec![]);
        let src = folder_node("src", "src", vec![a_rs, b_rs]);
        let readme = file_node("README.md", "README.md", vec![]);
        let root = folder_node("", "root", vec![src, readme]);
        SymbolTree {
            root,
            repo_root: std::path::PathBuf::from("."),
        }
    }

    fn empty_session(focus: &SymbolId) -> SessionState<'_> {
        SessionState {
            focus,
            hover: None,
            selection: None,
            visible: None,
            head: None,
            neighbors: None,
        }
    }

    fn resolve(
        expr: &SetExpr,
        tree: &SymbolTree,
        path_index: &PathIndex,
        session: &SessionState,
    ) -> (ResolvedSet, Vec<String>) {
        let mut warnings = Vec::new();
        let named = BTreeMap::new();
        let all_sets = BTreeMap::new();
        let mut stack = Vec::new();
        let sctx = test_sctx();
        let result = resolve_set_expr(
            expr,
            &named,
            session,
            path_index,
            &mut warnings,
            &mut stack,
            &all_sets,
            tree,
            &sctx,
        );
        (result, warnings)
    }

    #[test]
    fn glob_matches_paths_not_folder_itself() {
        let tree = nested_tree();
        let path_index = PathIndex::new(&tree);
        let focus = id_of(&tree.root);
        let session = empty_session(&focus);
        let expr = SetExpr::Glob("src/**".to_string());
        let (result, warnings) = resolve(&expr, &tree, &path_index, &session);
        assert!(warnings.is_empty());
        let a_rs = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/a.rs".to_string(),
            ordinal: 0,
        };
        let foo = SymbolId {
            kind: SymbolKind::Item {
                label: "fn".to_string(),
            },
            qualified_path: "src/a.rs::foo".to_string(),
            ordinal: 0,
        };
        let b_rs = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/b.rs".to_string(),
            ordinal: 0,
        };
        let src_folder = SymbolId {
            kind: SymbolKind::Folder,
            qualified_path: "src".to_string(),
            ordinal: 0,
        };
        assert_eq!(result.ids.len(), 3);
        assert!(result.ids.contains(&a_rs));
        assert!(result.ids.contains(&foo));
        assert!(result.ids.contains(&b_rs));
        assert!(!result.ids.contains(&src_folder));
        assert!(result.deps.intersects(Deps::TREE));
    }

    #[test]
    fn kind_matches_labels() {
        let tree = nested_tree();
        let path_index = PathIndex::new(&tree);
        let focus = id_of(&tree.root);
        let session = empty_session(&focus);
        let expr = SetExpr::Kind("fn".to_string());
        let (result, _) = resolve(&expr, &tree, &path_index, &session);
        assert_eq!(result.ids.len(), 1);

        let expr = SetExpr::Kind("file".to_string());
        let (result, _) = resolve(&expr, &tree, &path_index, &session);
        assert_eq!(result.ids.len(), 3);
    }

    #[test]
    fn intersect_takes_common_ids() {
        let tree = nested_tree();
        let path_index = PathIndex::new(&tree);
        let focus = id_of(&tree.root);
        let session = empty_session(&focus);
        let expr = SetExpr::Intersect(vec![
            SetExpr::Glob("src/**".to_string()),
            SetExpr::Kind("file".to_string()),
        ]);
        let (result, _) = resolve(&expr, &tree, &path_index, &session);
        // src/a.rs and src/b.rs are both under src/** and are files.
        assert_eq!(result.ids.len(), 2);
    }

    #[test]
    fn diff_removes_second_set() {
        let tree = nested_tree();
        let path_index = PathIndex::new(&tree);
        let focus = id_of(&tree.root);
        let session = empty_session(&focus);
        let expr = SetExpr::Diff(Box::new([
            SetExpr::Kind("file".to_string()),
            SetExpr::Glob("src/**".to_string()),
        ]));
        let (result, _) = resolve(&expr, &tree, &path_index, &session);
        // Only README.md is a file outside src/**.
        assert_eq!(result.ids.len(), 1);
        let readme = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "README.md".to_string(),
            ordinal: 0,
        };
        assert!(result.ids.contains(&readme));
    }

    #[test]
    fn not_is_complement_of_laid_out_tree() {
        let tree = nested_tree();
        let path_index = PathIndex::new(&tree);
        let focus = id_of(&tree.root);
        let session = empty_session(&focus);
        let expr = SetExpr::Not(Box::new(SetExpr::Kind("file".to_string())));
        let (result, _) = resolve(&expr, &tree, &path_index, &session);
        // 6 total nodes - 3 files = 3 (root, src folder, foo item).
        assert_eq!(result.ids.len(), 3);
        assert!(result.deps.intersects(Deps::TREE));
    }

    #[test]
    fn ancestors_walks_up_parent_chain() {
        let tree = nested_tree();
        let path_index = PathIndex::new(&tree);
        let focus = id_of(&tree.root);
        let session = empty_session(&focus);
        let expr = SetExpr::Ancestors(Box::new(SetRef::Inline(Box::new(SetExpr::Kind(
            "fn".to_string(),
        )))));
        let (result, _) = resolve(&expr, &tree, &path_index, &session);
        // Ancestors of `foo`: src/a.rs, src, root.
        assert_eq!(result.ids.len(), 3);
    }

    #[test]
    fn descendants_walks_down_children() {
        let tree = nested_tree();
        let path_index = PathIndex::new(&tree);
        let focus = id_of(&tree.root);
        let session = empty_session(&focus);
        let expr = SetExpr::Descendants(Box::new(SetRef::Inline(Box::new(SetExpr::Glob(
            "src".to_string(),
        )))));
        let (result, _) = resolve(&expr, &tree, &path_index, &session);
        // Descendants of `src`: a.rs, foo, b.rs.
        assert_eq!(result.ids.len(), 3);
    }

    #[test]
    fn fuzzy_matches_subsequence() {
        let tree = nested_tree();
        let path_index = PathIndex::new(&tree);
        let focus = id_of(&tree.root);
        let session = empty_session(&focus);
        let expr = SetExpr::Fuzzy("fo".to_string());
        let (result, _) = resolve(&expr, &tree, &path_index, &session);
        assert_eq!(result.ids.len(), 1);
        let foo = SymbolId {
            kind: SymbolKind::Item {
                label: "fn".to_string(),
            },
            qualified_path: "src/a.rs::foo".to_string(),
            ordinal: 0,
        };
        assert!(result.ids.contains(&foo));
    }
}
