//! Graph space scaffold: a synthetic (tree, layout) pair for
//! `space.kind == "graph"` views. The symbols incident to the view's edge
//! layers become flat children of the root, positioned by a layered graph
//! layout instead of the treemap packer. Downstream infrastructure
//! (culling, hit testing, spatial nav, camera, edge projection) consumes
//! the pair exactly like the treemap scaffold. Class members render as
//! rows inside each box (UML style), filtered by `space.members`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree, Visibility};
use outrider_layout::{layout_graph, GraphConfig, GraphNode, PackLayout, Rect};
use outrider_view::spec::MembersSpec;
use outrider_view::ResolvedView;

/// World-space height of one member row inside a node box. Rows must
/// clear the Label rung threshold (20px) at zoom 1 so names render.
const MEMBER_ROW_H: f64 = 24.0;
/// Title strip height reserved above the member list.
const BOX_HEADER_H: f64 = 26.0;
/// Horizontal inset of member rows from the box edges.
const MEMBER_PAD_X: f64 = 8.0;
/// Bottom padding under the last member row.
const MEMBER_PAD_BOTTOM: f64 = 8.0;

pub(crate) struct GraphScaffold {
    pub(crate) tree: SymbolTree,
    pub(crate) layout: PackLayout,
}

/// Container-like item labels that are nested types, not listable members.
fn is_nested_type(label: &str) -> bool {
    matches!(
        label,
        "class" | "struct" | "enum" | "namespace" | "impl" | "interface" | "trait" | "mod"
    )
}

fn vis_name(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

fn vis_marker(v: Option<Visibility>) -> &'static str {
    match v {
        Some(Visibility::Public) => "+ ",
        Some(Visibility::Protected) => "# ",
        Some(Visibility::Private) => "- ",
        None => "",
    }
}

/// Longest member row we render before eliding, in characters.
const MEMBER_MAX_CHARS: usize = 46;

/// Display text for a member row: its typed signature when the index has
/// one (access keywords stripped — the UML marker already carries them),
/// otherwise the bare name. Long rows are elided.
fn member_display(m: &SymbolNode) -> String {
    let base = match m.signature.as_deref().map(str::trim) {
        Some(sig) if !sig.is_empty() => {
            let mut s = sig;
            loop {
                let word = s.split_whitespace().next().unwrap_or("");
                let strip = matches!(word, "pub" | "public" | "private" | "protected" | "export")
                    || word.starts_with("pub(");
                if strip {
                    s = s[word.len()..].trim_start();
                } else {
                    break;
                }
            }
            s.to_string()
        }
        _ => {
            let suffix = if m.id.kind.label() == "fn" { "()" } else { "" };
            format!("{}{}", m.name, suffix)
        }
    };
    if base.chars().count() > MEMBER_MAX_CHARS {
        let truncated: String = base.chars().take(MEMBER_MAX_CHARS - 1).collect();
        format!("{truncated}…")
    } else {
        base
    }
}

/// Direct children of `class_node` that pass the member filter, sorted
/// fields-before-methods then by name (UML convention).
fn filter_members<'a>(
    class_node: &'a SymbolNode,
    spec: Option<&MembersSpec>,
) -> Vec<&'a SymbolNode> {
    let mut members: Vec<&SymbolNode> = class_node
        .children
        .iter()
        .filter(|m| {
            let label = match &m.id.kind {
                SymbolKind::Item { label } => label.as_str(),
                _ => return false,
            };
            if is_nested_type(label) {
                return false;
            }
            if let Some(spec) = spec {
                if let Some(kinds) = &spec.kinds {
                    if !kinds.iter().any(|k| k == label) {
                        return false;
                    }
                }
                if let Some(show) = &spec.show {
                    match m.visibility {
                        Some(v) => {
                            if !show.iter().any(|s| s == vis_name(v)) {
                                return false;
                            }
                        }
                        // Unknown visibility stays visible unless the
                        // filter is an explicit empty list.
                        None => {
                            if show.is_empty() {
                                return false;
                            }
                        }
                    }
                }
            }
            true
        })
        .collect();
    members.sort_by_key(|m| {
        let is_fn = m.id.kind.label() == "fn";
        (is_fn, m.name.clone())
    });
    members
}

/// Build the graph scaffold from the resolved view's edge layers.
/// Returns a scaffold containing only the root when no edges resolved yet
/// (async relation providers may still be pending).
pub(crate) fn build(
    index_tree: &SymbolTree,
    resolved: &ResolvedView,
    members_spec: Option<&MembersSpec>,
) -> GraphScaffold {
    // 1. Collect edge endpoints across every resolved edge layer, plus all
    //    members of each layer's `incidentTo` set so unconnected symbols
    //    still show up as isolated boxes.
    let mut ids: BTreeSet<SymbolId> = BTreeSet::new();
    let mut id_edges: Vec<(SymbolId, SymbolId)> = Vec::new();
    for layer in &resolved.edges {
        for e in &layer.edges {
            if e.from == e.to {
                continue;
            }
            ids.insert(e.from.clone());
            ids.insert(e.to.clone());
            id_edges.push((e.from.clone(), e.to.clone()));
        }
        if let Some(set) = layer
            .incident_set
            .as_ref()
            .and_then(|name| resolved.sets.get(name))
        {
            for id in &set.ids {
                if id.kind != SymbolKind::Folder {
                    ids.insert(id.clone());
                }
            }
        }
    }

    // 2. Look up the real nodes so boxes carry name/signature/churn.
    let mut real: HashMap<&SymbolId, &SymbolNode> = HashMap::new();
    collect(&index_tree.root, &ids, &mut real);

    let ordered: Vec<SymbolId> = ids.into_iter().collect();
    let index_of: HashMap<&SymbolId, usize> = ordered
        .iter()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect();

    let member_lists: Vec<Vec<&SymbolNode>> = ordered
        .iter()
        .map(|id| {
            real.get(id)
                .map(|n| filter_members(n, members_spec))
                .unwrap_or_default()
        })
        .collect();

    let graph_nodes: Vec<GraphNode> = ordered
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let name_len = real
                .get(id)
                .map(|n| n.name.chars().count())
                .unwrap_or_else(|| id.qualified_path.chars().count().min(24));
            // Widen boxes so the longest member row fits too.
            let member_len = member_lists[i]
                .iter()
                .map(|m| member_display(m).chars().count() + 3)
                .max()
                .unwrap_or(0);
            let extra_h = if member_lists[i].is_empty() {
                0.0
            } else {
                member_lists[i].len() as f64 * MEMBER_ROW_H + MEMBER_PAD_BOTTOM
            };
            GraphNode {
                label_len: name_len.max(member_len),
                extra_h,
            }
        })
        .collect();
    let edges: Vec<(usize, usize)> = id_edges
        .iter()
        .filter_map(|(f, t)| Some((*index_of.get(f)?, *index_of.get(t)?)))
        .collect();

    // 3. Layered layout.
    let gl = layout_graph(&graph_nodes, &edges, &GraphConfig::default());

    // 4. Assemble the synthetic tree and rect map. Each box is a container
    //    whose children are its member rows; both carry real SymbolIds so
    //    selection, fill, and marks apply to actual symbols.
    let mut rects: BTreeMap<SymbolId, Rect> = BTreeMap::new();
    let mut children: Vec<SymbolNode> = Vec::with_capacity(ordered.len());
    for (i, id) in ordered.iter().enumerate() {
        let box_rect = gl.rects[i];
        rects.insert(id.clone(), box_rect);

        let mut member_nodes: Vec<SymbolNode> = Vec::with_capacity(member_lists[i].len());
        for (j, m) in member_lists[i].iter().enumerate() {
            member_nodes.push(SymbolNode {
                id: m.id.clone(),
                name: format!("{}{}", vis_marker(m.visibility), member_display(m)),
                byte_range: None,
                signature: m.signature.clone(),
                doc: m.doc.clone(),
                measure: m.measure,
                churn: m.churn,
                churn_count: m.churn_count,
                diff_status: None,
                diff_hunks: Vec::new(),
                deleted_lines: Vec::new(),
                visibility: m.visibility,
                children: Vec::new(),
            });
            rects.insert(
                m.id.clone(),
                Rect {
                    x: box_rect.x + MEMBER_PAD_X,
                    y: box_rect.y + BOX_HEADER_H + j as f64 * MEMBER_ROW_H,
                    w: (box_rect.w - 2.0 * MEMBER_PAD_X).max(1.0),
                    h: MEMBER_ROW_H - 2.0,
                },
            );
        }

        let node = match real.get(id) {
            Some(n) => SymbolNode {
                id: id.clone(),
                name: n.name.clone(),
                byte_range: None,
                signature: n.signature.clone(),
                doc: n.doc.clone(),
                measure: n.measure,
                churn: n.churn,
                churn_count: n.churn_count,
                diff_status: None,
                diff_hunks: Vec::new(),
                deleted_lines: Vec::new(),
                visibility: n.visibility,
                children: member_nodes,
            },
            None => SymbolNode {
                id: id.clone(),
                name: id
                    .qualified_path
                    .rsplit(['/', ':', '.'])
                    .next()
                    .unwrap_or(&id.qualified_path)
                    .to_string(),
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
                children: member_nodes,
            },
        };
        children.push(node);
    }

    let root_id = index_tree.root.id.clone();
    rects.insert(root_id.clone(), gl.bounds);
    let root = SymbolNode {
        id: root_id,
        name: index_tree.root.name.clone(),
        byte_range: None,
        signature: None,
        doc: None,
        measure: index_tree.root.measure,
        churn: 0.0,
        churn_count: 0,
        diff_status: None,
        diff_hunks: Vec::new(),
        deleted_lines: Vec::new(),
        visibility: None,
        children,
    };

    GraphScaffold {
        tree: SymbolTree {
            root,
            repo_root: index_tree.repo_root.clone(),
        },
        layout: PackLayout { rects },
    }
}

fn collect<'a>(
    node: &'a SymbolNode,
    wanted: &BTreeSet<SymbolId>,
    out: &mut HashMap<&'a SymbolId, &'a SymbolNode>,
) {
    if wanted.contains(&node.id) {
        out.insert(&node.id, node);
    }
    for child in &node.children {
        collect(child, wanted, out);
    }
}
