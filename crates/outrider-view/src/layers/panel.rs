//! Panel resolution: spec -> resolved panel with rows, sorting, and limit.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use outrider_index::{SymbolId, TreeIndex};

use crate::deps::Deps;
use crate::relation::{EdgeDir, Lookup, ProviderCtx};
use crate::resolve::ResolveCtx;
use crate::set::ResolvedSet;
use crate::spec::*;

/// A fully resolved panel ready for the app to render.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPanel {
    pub layer: usize,
    pub id: Option<String>,
    pub title: Option<String>,
    pub dock: Dock,
    pub columns: Vec<MetricRef>,
    pub rows: Vec<Row>,
    pub pending: bool,
    pub deps: Deps,
}

/// One row in a resolved panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: SymbolId,
    pub label: String,
    pub sublabel: String,
    pub cells: Vec<Option<f64>>,
    pub group: Option<String>,
    pub alternates: Vec<RowAlternate>,
}

/// An alternate edge target within a group (EdgeGroups panels).
#[derive(Debug, Clone, PartialEq)]
pub struct RowAlternate {
    pub id: SymbolId,
    pub call_site: Option<Range<usize>>,
}

/// Intermediate type for edge details used in grouping.
#[derive(Debug, Clone)]
pub struct EdgeDetailRow {
    pub target: SymbolId,
    pub raw_name: String,
    pub call_site: Option<Range<usize>>,
    pub weight: f64,
}

/// Group edge details by their `raw_name`, preserving first-seen order.
pub fn group_edges(edges: Vec<EdgeDetailRow>) -> Vec<(String, Vec<EdgeDetailRow>)> {
    let mut groups: Vec<(String, Vec<EdgeDetailRow>)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for e in edges {
        match seen.get(&e.raw_name) {
            Some(&i) => groups[i].1.push(e),
            None => {
                seen.insert(e.raw_name.clone(), groups.len());
                groups.push((e.raw_name.clone(), vec![e]));
            }
        }
    }
    groups
}

/// Look up a `SetRef` against the pre-resolved sets.
/// Returns the set's symbol IDs and dependency flags.
fn resolve_set_ref_ids(
    r: &SetRef,
    sets: &BTreeMap<String, ResolvedSet>,
    warnings: &mut Vec<String>,
) -> (Vec<SymbolId>, Deps) {
    match r {
        SetRef::Name(name) => match sets.get(name) {
            Some(s) => (s.ids.iter().cloned().collect(), s.deps),
            None => {
                warnings.push(format!("panel: unknown set '{name}'"));
                (vec![], Deps::NONE)
            }
        },
        SetRef::Inline(_) => {
            warnings.push(
                "panel: inline set expressions are not resolved for panel rows".to_string(),
            );
            (vec![], Deps::NONE)
        }
    }
}

/// Remove SELECTION from a Deps bitset. Panels should not re-resolve on
/// selection changes.
fn strip_selection(deps: Deps) -> Deps {
    let mask = Deps::TREE
        .union(Deps::FOCUS)
        .union(Deps::CAMERA)
        .union(Deps::HOVER)
        .union(Deps::GIT)
        .union(Deps::SPEC)
        .union(Deps::METRICS)
        .union(Deps::RELATIONS);
    deps & mask
}

/// Resolve a `PanelSpec` into a `ResolvedPanel`.
pub fn resolve_panel(
    layer_idx: usize,
    spec: &PanelSpec,
    ctx: &ResolveCtx,
    sets: &BTreeMap<String, ResolvedSet>,
    warnings: &mut Vec<String>,
) -> ResolvedPanel {
    let mut deps = Deps::TREE;
    let mut pending = false;
    let index = TreeIndex::new(ctx.tree);

    let (mut rows, is_edge_groups) = match &spec.rows {
        // ── Set rows ──
        PanelRows::Set(set_ref) => {
            let (ids, set_deps) = resolve_set_ref_ids(set_ref, sets, warnings);
            deps = deps.union(set_deps);
            let rows: Vec<Row> = ids
                .iter()
                .filter_map(|id| {
                    let node = index.node(id)?;
                    Some(Row {
                        id: id.clone(),
                        label: node.name.clone(),
                        sublabel: id.qualified_path.clone(),
                        cells: vec![],
                        group: None,
                        alternates: vec![],
                    })
                })
                .collect();
            (rows, false)
        }

        // ── EdgeGroups rows ──
        PanelRows::EdgeGroups {
            of,
            relation,
            direction,
        } => {
            let (source_ids, set_deps) = resolve_set_ref_ids(of, sets, warnings);
            deps = deps.union(set_deps).union(Deps::RELATIONS);

            let provider = match ctx.relations.get(relation) {
                Some(p) => p,
                None => {
                    warnings.push(format!(
                        "panel[{layer_idx}]: unknown relation '{relation}'"
                    ));
                    return empty_panel(layer_idx, spec, deps);
                }
            };

            let pctx = ProviderCtx {
                tree: ctx.tree,
                index: &index,
                repo_root: ctx.repo_root,
            };

            let dir = match direction {
                EdgeDirection::In => EdgeDir::In,
                EdgeDirection::Out => EdgeDir::Out,
            };

            let mut all_details: Vec<EdgeDetailRow> = Vec::new();

            for src_id in &source_ids {
                match provider.lookup(src_id, dir, &pctx) {
                    Lookup::Pending => {
                        pending = true;
                    }
                    Lookup::Ready(edges) => {
                        for (target_id, weight) in edges {
                            let detail = provider.edge_detail(src_id, &target_id, &pctx);
                            let (raw_name, call_site) = match detail {
                                Some(ed) => (ed.label, ed.site),
                                None => {
                                    let name = index
                                        .node(&target_id)
                                        .map(|n| n.name.clone())
                                        .unwrap_or_else(|| target_id.qualified_path.clone());
                                    (name, None)
                                }
                            };
                            all_details.push(EdgeDetailRow {
                                target: target_id,
                                raw_name,
                                call_site,
                                weight,
                            });
                        }
                    }
                }
            }

            if pending {
                return ResolvedPanel {
                    layer: layer_idx,
                    id: spec.id.clone(),
                    title: spec.title.clone(),
                    dock: spec.dock,
                    columns: spec.columns.clone(),
                    rows: vec![],
                    pending: true,
                    deps: strip_selection(deps),
                };
            }

            let groups = group_edges(all_details);
            let rows: Vec<Row> = groups
                .into_iter()
                .map(|(group_name, edges)| {
                    let first = &edges[0];
                    let label = index
                        .node(&first.target)
                        .map(|n| n.name.clone())
                        .unwrap_or_else(|| group_name.clone());
                    let alternates: Vec<RowAlternate> = edges
                        .iter()
                        .map(|e| RowAlternate {
                            id: e.target.clone(),
                            call_site: e.call_site.clone(),
                        })
                        .collect();
                    Row {
                        id: first.target.clone(),
                        label,
                        sublabel: first.target.qualified_path.clone(),
                        cells: vec![],
                        group: Some(group_name),
                        alternates,
                    }
                })
                .collect();
            (rows, true)
        }

        // ── Matrix (stub) ──
        PanelRows::Matrix { .. } => {
            warnings.push(format!(
                "panel[{layer_idx}]: panel.matrix is not supported yet"
            ));
            (vec![], false)
        }
    };

    // ── Cells ──
    if !spec.columns.is_empty() {
        deps = deps.union(Deps::METRICS);
        for row in &mut rows {
            row.cells = spec
                .columns
                .iter()
                .map(|col| {
                    let node = index.node(&row.id)?;
                    let provider = ctx.metrics.get(&col.0)?;
                    provider.value(node)
                })
                .collect();
        }
    }

    // ── Sort (Set rows only, never EdgeGroups) ──
    if !is_edge_groups {
        if let Some(sort_key) = &spec.sort_by {
            match sort_key {
                SortKey::Builtin(BuiltinSort::Name) => {
                    rows.sort_by(|a, b| (&a.label, &a.id).cmp(&(&b.label, &b.id)));
                }
                SortKey::Builtin(BuiltinSort::NameLength) => {
                    rows.sort_by(|a, b| {
                        (a.label.chars().count(), &a.label, &a.id).cmp(&(
                            b.label.chars().count(),
                            &b.label,
                            &b.id,
                        ))
                    });
                }
                SortKey::Metric(metric_ref) => {
                    let metric = ctx.metrics.get(&metric_ref.0);
                    rows.sort_by(|a, b| {
                        let va =
                            metric.and_then(|p| index.node(&a.id).and_then(|n| p.value(n)));
                        let vb =
                            metric.and_then(|p| index.node(&b.id).and_then(|n| p.value(n)));
                        match (va, vb) {
                            (Some(x), Some(y)) => y
                                .partial_cmp(&x)
                                .unwrap_or(std::cmp::Ordering::Equal)
                                .then_with(|| a.id.cmp(&b.id)),
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => a.id.cmp(&b.id),
                        }
                    });
                }
            }
        }
    }

    // ── Limit ──
    if let Some(limit) = spec.limit {
        rows.truncate(limit);
    }

    ResolvedPanel {
        layer: layer_idx,
        id: spec.id.clone(),
        title: spec.title.clone(),
        dock: spec.dock,
        columns: spec.columns.clone(),
        rows,
        pending,
        deps: strip_selection(deps),
    }
}

fn empty_panel(layer_idx: usize, spec: &PanelSpec, deps: Deps) -> ResolvedPanel {
    ResolvedPanel {
        layer: layer_idx,
        id: spec.id.clone(),
        title: spec.title.clone(),
        dock: spec.dock,
        columns: spec.columns.clone(),
        rows: vec![],
        pending: false,
        deps: strip_selection(deps),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metric::MetricRegistry;
    use crate::relation::{EdgeDir, Lookup, ProviderCtx, RelationProvider, RelationRegistry};
    use crate::resolve::SessionState;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
    use outrider_layout::PackLayout;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn make_id(kind: SymbolKind, path: &str) -> SymbolId {
        SymbolId {
            kind,
            qualified_path: path.to_string(),
            ordinal: 0,
        }
    }

    fn leaf(path: &str, name: &str, measure: u64) -> SymbolNode {
        SymbolNode {
            id: make_id(SymbolKind::File, path),
            name: name.to_string(),
            doc: None,
            signature: None,
            measure,
            byte_range: None,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        }
    }

    fn sample_tree(children: Vec<SymbolNode>) -> SymbolTree {
        SymbolTree {
            root: SymbolNode {
                id: make_id(SymbolKind::Folder, ""),
                name: "root".to_string(),
                doc: None,
                signature: None,
                measure: 0,
                byte_range: None,
                churn: 0.0,
                churn_count: 0,
                children,
            },
            repo_root: PathBuf::from("."),
        }
    }

    fn make_ctx<'a>(
        tree: &'a SymbolTree,
        layout: &'a PackLayout,
        metrics: &'a MetricRegistry,
        relations: &'a RelationRegistry,
        focus: &'a SymbolId,
    ) -> ResolveCtx<'a> {
        static EMPTY_PARTS: std::sync::LazyLock<crate::partition::PartitionRegistry> =
            std::sync::LazyLock::new(crate::partition::PartitionRegistry::default);
        ResolveCtx {
            tree,
            layout,
            metrics,
            relations,
            partitions: &EMPTY_PARTS,
            session: SessionState {
                focus,
                hover: None,
                selection: None,
                visible: None,
                head: None,
                neighbors: None,
            },
            repo_root: tree.repo_root.as_path(),
        }
    }

    // ── group_edges ──

    #[test]
    fn group_edges_preserves_first_seen_order() {
        let id_x = make_id(SymbolKind::File, "x");
        let id_y = make_id(SymbolKind::File, "y");
        let id_x2 = make_id(SymbolKind::File, "x2");
        let id_z = make_id(SymbolKind::File, "z");

        let edges = vec![
            EdgeDetailRow {
                target: id_x.clone(),
                raw_name: "x".into(),
                call_site: None,
                weight: 1.0,
            },
            EdgeDetailRow {
                target: id_y.clone(),
                raw_name: "y".into(),
                call_site: None,
                weight: 1.0,
            },
            EdgeDetailRow {
                target: id_x2.clone(),
                raw_name: "x".into(),
                call_site: Some(10..20),
                weight: 1.0,
            },
            EdgeDetailRow {
                target: id_z.clone(),
                raw_name: "z".into(),
                call_site: None,
                weight: 1.0,
            },
        ];
        let groups = group_edges(edges);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].0, "x");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "y");
        assert_eq!(groups[1].1.len(), 1);
        assert_eq!(groups[2].0, "z");
        assert_eq!(groups[2].1.len(), 1);
    }

    // ── resolve_panel: Set rows ──

    #[test]
    fn resolve_panel_set_rows() {
        let a = leaf("a.rs", "alpha", 10);
        let b = leaf("b.rs", "bravo", 20);
        let c = leaf("c.rs", "charlie", 30);
        let ids: HashSet<SymbolId> = [a.id.clone(), b.id.clone(), c.id.clone()]
            .into_iter()
            .collect();

        let tree = sample_tree(vec![a, b, c]);
        let layout = PackLayout {
            rects: Default::default(),
        };
        let metrics = MetricRegistry::builtin();
        let relations = RelationRegistry::empty();
        let focus = make_id(SymbolKind::Folder, "");
        let ctx = make_ctx(&tree, &layout, &metrics, &relations, &focus);

        let mut sets = BTreeMap::new();
        sets.insert(
            "test".to_string(),
            ResolvedSet {
                ids,
                ranges: BTreeMap::new(),
                deps: Deps::TREE,
            },
        );

        let spec = PanelSpec {
            rows: PanelRows::Set(SetRef::Name("test".into())),
            columns: vec![],
            sort_by: None,
            dock: Dock::Float,
            title: None,
            id: None,
            limit: None,
        };

        let mut warnings = Vec::new();
        let panel = resolve_panel(0, &spec, &ctx, &sets, &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(panel.rows.len(), 3);

        let labels: HashSet<&str> = panel.rows.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains("alpha"));
        assert!(labels.contains("bravo"));
        assert!(labels.contains("charlie"));

        for row in &panel.rows {
            assert!(row.cells.is_empty());
            assert!(row.group.is_none());
            assert!(row.alternates.is_empty());
        }
    }

    // ── sort_by ──

    #[test]
    fn sort_by_name_length() {
        let a = leaf("a.rs", "ab", 10);
        let b = leaf("b.rs", "abcde", 20);
        let c = leaf("c.rs", "abc", 30);
        let d = leaf("d.rs", "a", 40);
        let ids: HashSet<SymbolId> =
            [a.id.clone(), b.id.clone(), c.id.clone(), d.id.clone()]
                .into_iter()
                .collect();

        let tree = sample_tree(vec![a, b, c, d]);
        let layout = PackLayout {
            rects: Default::default(),
        };
        let metrics = MetricRegistry::builtin();
        let relations = RelationRegistry::empty();
        let focus = make_id(SymbolKind::Folder, "");
        let ctx = make_ctx(&tree, &layout, &metrics, &relations, &focus);

        let mut sets = BTreeMap::new();
        sets.insert(
            "test".to_string(),
            ResolvedSet {
                ids,
                ranges: BTreeMap::new(),
                deps: Deps::TREE,
            },
        );

        let spec = PanelSpec {
            rows: PanelRows::Set(SetRef::Name("test".into())),
            columns: vec![],
            sort_by: Some(SortKey::Builtin(BuiltinSort::NameLength)),
            dock: Dock::Float,
            title: None,
            id: None,
            limit: None,
        };

        let mut warnings = Vec::new();
        let panel = resolve_panel(0, &spec, &ctx, &sets, &mut warnings);
        let names: Vec<&str> = panel.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(names, vec!["a", "ab", "abc", "abcde"]);
    }

    // ── limit ──

    #[test]
    fn limit_truncates_after_sort() {
        let nodes: Vec<SymbolNode> = (0..5)
            .map(|i| leaf(&format!("{i}.rs"), &format!("n{i}"), i as u64 * 10))
            .collect();
        let ids: HashSet<SymbolId> = nodes.iter().map(|n| n.id.clone()).collect();

        let tree = sample_tree(nodes);
        let layout = PackLayout {
            rects: Default::default(),
        };
        let metrics = MetricRegistry::builtin();
        let relations = RelationRegistry::empty();
        let focus = make_id(SymbolKind::Folder, "");
        let ctx = make_ctx(&tree, &layout, &metrics, &relations, &focus);

        let mut sets = BTreeMap::new();
        sets.insert(
            "test".to_string(),
            ResolvedSet {
                ids,
                ranges: BTreeMap::new(),
                deps: Deps::TREE,
            },
        );

        let spec = PanelSpec {
            rows: PanelRows::Set(SetRef::Name("test".into())),
            columns: vec![],
            sort_by: Some(SortKey::Builtin(BuiltinSort::Name)),
            dock: Dock::Float,
            title: None,
            id: None,
            limit: Some(3),
        };

        let mut warnings = Vec::new();
        let panel = resolve_panel(0, &spec, &ctx, &sets, &mut warnings);
        assert_eq!(panel.rows.len(), 3);
    }

    // ── EdgeGroups pending ──

    #[test]
    fn edge_groups_pending() {
        struct PendingProvider;
        impl RelationProvider for PendingProvider {
            fn id(&self) -> &str {
                "pending_rel"
            }
            fn out_edges(&self, _: &SymbolId, _: &ProviderCtx) -> Vec<(SymbolId, f64)> {
                vec![]
            }
            fn in_edges(&self, _: &SymbolId, _: &ProviderCtx) -> Vec<(SymbolId, f64)> {
                vec![]
            }
            fn deps(&self) -> Deps {
                Deps::RELATIONS
            }
            fn lookup(
                &self,
                _sym: &SymbolId,
                _dir: EdgeDir,
                _ctx: &ProviderCtx,
            ) -> Lookup<Vec<(SymbolId, f64)>> {
                Lookup::Pending
            }
        }

        let a = leaf("a.rs", "alpha", 10);
        let src_id = a.id.clone();
        let tree = sample_tree(vec![a]);
        let layout = PackLayout {
            rects: Default::default(),
        };
        let metrics = MetricRegistry::builtin();
        let mut relations = RelationRegistry::empty();
        relations.register(Box::new(PendingProvider));
        let focus = make_id(SymbolKind::Folder, "");
        let ctx = make_ctx(&tree, &layout, &metrics, &relations, &focus);

        let mut sets = BTreeMap::new();
        sets.insert(
            "sources".to_string(),
            ResolvedSet {
                ids: [src_id].into_iter().collect(),
                ranges: BTreeMap::new(),
                deps: Deps::TREE,
            },
        );

        let spec = PanelSpec {
            rows: PanelRows::EdgeGroups {
                of: SetRef::Name("sources".into()),
                relation: "pending_rel".into(),
                direction: EdgeDirection::Out,
            },
            columns: vec![],
            sort_by: None,
            dock: Dock::Float,
            title: None,
            id: None,
            limit: None,
        };

        let mut warnings = Vec::new();
        let panel = resolve_panel(0, &spec, &ctx, &sets, &mut warnings);
        assert!(panel.pending);
        assert!(panel.rows.is_empty());
    }

    // ── deps ──

    #[test]
    fn panel_deps_exclude_selection() {
        let a = leaf("a.rs", "alpha", 10);
        let ids: HashSet<SymbolId> = [a.id.clone()].into_iter().collect();
        let tree = sample_tree(vec![a]);
        let layout = PackLayout {
            rects: Default::default(),
        };
        let metrics = MetricRegistry::builtin();
        let relations = RelationRegistry::empty();
        let focus = make_id(SymbolKind::Folder, "");
        let ctx = make_ctx(&tree, &layout, &metrics, &relations, &focus);

        let mut sets = BTreeMap::new();
        sets.insert(
            "test".to_string(),
            ResolvedSet {
                ids,
                ranges: BTreeMap::new(),
                // Deliberately include SELECTION in the source set's deps.
                deps: Deps::TREE.union(Deps::SELECTION),
            },
        );

        let spec = PanelSpec {
            rows: PanelRows::Set(SetRef::Name("test".into())),
            columns: vec![],
            sort_by: None,
            dock: Dock::Float,
            title: None,
            id: None,
            limit: None,
        };

        let mut warnings = Vec::new();
        let panel = resolve_panel(0, &spec, &ctx, &sets, &mut warnings);
        assert!(
            !panel.deps.intersects(Deps::SELECTION),
            "panel deps must not include SELECTION"
        );
        assert!(panel.deps.intersects(Deps::TREE));
    }
}
