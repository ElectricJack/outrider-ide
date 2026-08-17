//! Edges layer resolution: relation lookups / explicit pairs -> drawn edges.

use std::collections::{BTreeMap, HashMap};

use outrider_index::tree_index::TreeIndex;
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

use crate::deps::Deps;
use crate::relation::{EdgeDir, Lookup, ProviderCtx};
use crate::resolve::ResolveCtx;
use crate::set::ResolvedSet;
use crate::spec::{Direction, EdgeSource, EdgePair, EdgeStyle, EdgesSpec, SetRef};

/// A single resolved edge between two symbols.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: SymbolId,
    pub to: SymbolId,
    pub weight: f64,
    pub basis: Option<String>,
}

/// The resolved result of one `edges` layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEdgeLayer {
    pub layer_index: usize,
    pub relation: String,
    pub edges: Vec<Edge>,
    pub directed: bool,
    pub style: EdgeStyle,
    pub color: String,
    /// Number of source symbols still waiting on an async provider lookup.
    pub pending: usize,
    pub deps: Deps,
}

fn default_color() -> String {
    "#8a8a8a".to_string()
}

fn empty_layer(layer_index: usize, spec: &EdgesSpec, relation: String) -> ResolvedEdgeLayer {
    ResolvedEdgeLayer {
        layer_index,
        relation,
        edges: Vec::new(),
        directed: true,
        style: spec.style,
        color: spec.color.clone().unwrap_or_else(default_color),
        pending: 0,
        deps: Deps::NONE,
    }
}

/// Resolve a single `edges` layer against the relation registry / explicit pairs.
pub fn resolve_edges(
    spec: &EdgesSpec,
    layer_index: usize,
    sets: &BTreeMap<String, ResolvedSet>,
    ctx: &ResolveCtx,
    warnings: &mut Vec<String>,
) -> ResolvedEdgeLayer {
    let source = match spec.source() {
        Ok(s) => s,
        Err(_) => {
            warnings.push(format!(
                "edges[{layer_index}]: exactly one of relation or pairs must be set"
            ));
            return empty_layer(layer_index, spec, String::new());
        }
    };

    if spec.cross_boundary.is_some() {
        warnings.push(format!(
            "edges[{layer_index}]: crossBoundary is not implemented yet; ignoring"
        ));
    }
    if matches!(spec.direction, Some(Direction::Up) | Some(Direction::Down)) {
        warnings.push(format!(
            "edges[{layer_index}]: direction filtering is not implemented yet; ignoring"
        ));
    }

    let mut layer = match source {
        EdgeSource::Relation(relation_id) => {
            resolve_from_relation(relation_id, layer_index, spec, sets, ctx, warnings)
        }
        EdgeSource::Pairs(pairs) => resolve_from_pairs(pairs, layer_index, spec, ctx, warnings),
    };

    // ── filters ──
    let within = resolve_scope_set(spec.within.as_ref(), sets, warnings, "within");
    let incident_to = resolve_scope_set(spec.incident_to.as_ref(), sets, warnings, "incidentTo");

    layer.edges.retain(|e| {
        if let Some(w) = within {
            if !(w.contains(&e.from) && w.contains(&e.to)) {
                return false;
            }
        }
        if let Some(inc) = incident_to {
            if !(inc.contains(&e.from) || inc.contains(&e.to)) {
                return false;
            }
        }
        e.weight >= spec.min_weight
    });

    layer
}

fn resolve_scope_set<'a>(
    set_ref: Option<&SetRef>,
    sets: &'a BTreeMap<String, ResolvedSet>,
    warnings: &mut Vec<String>,
    what: &str,
) -> Option<&'a ResolvedSet> {
    match set_ref? {
        SetRef::Name(name) => match sets.get(name) {
            Some(s) => Some(s),
            None => {
                warnings.push(format!("edges: unknown set '{name}' in {what}"));
                None
            }
        },
        SetRef::Inline(_) => {
            warnings.push(format!(
                "edges: inline set expressions are not resolved for {what}"
            ));
            None
        }
    }
}

fn resolve_from_relation(
    relation_id: &str,
    layer_index: usize,
    spec: &EdgesSpec,
    sets: &BTreeMap<String, ResolvedSet>,
    ctx: &ResolveCtx,
    warnings: &mut Vec<String>,
) -> ResolvedEdgeLayer {
    let provider = match ctx.relations.get(relation_id) {
        Some(p) => p,
        None => {
            warnings.push(format!("edges: unknown relation '{relation_id}'"));
            return empty_layer(layer_index, spec, relation_id.to_string());
        }
    };

    let index = TreeIndex::new(ctx.tree);
    let provider_ctx = ProviderCtx {
        tree: ctx.tree,
        index: &index,
        repo_root: ctx.repo_root,
    };

    let within = resolve_scope_set(spec.within.as_ref(), sets, warnings, "within");
    let incident_to = resolve_scope_set(spec.incident_to.as_ref(), sets, warnings, "incidentTo");

    // Which direction(s) to query from each source symbol, and which symbols
    // to start from.
    let (source_ids, also_in): (Vec<SymbolId>, bool) = if let Some(w) = within {
        (w.ids.iter().cloned().collect(), false)
    } else if let Some(inc) = incident_to {
        (inc.ids.iter().cloned().collect(), true)
    } else {
        (fn_leaves(ctx.tree), true)
    };

    let mut best: HashMap<(SymbolId, SymbolId), (f64, Option<String>)> = HashMap::new();
    let mut pending = 0usize;

    for sym in &source_ids {
        match provider.lookup(sym, EdgeDir::Out, &provider_ctx) {
            Lookup::Ready(edges) => {
                for (to, w) in edges {
                    let key = (sym.clone(), to);
                    let entry = best.entry(key).or_insert((f64::MIN, None));
                    if w > entry.0 {
                        entry.0 = w;
                    }
                }
            }
            Lookup::Pending => pending += 1,
        }

        if also_in {
            match provider.lookup(sym, EdgeDir::In, &provider_ctx) {
                Lookup::Ready(edges) => {
                    for (from, w) in edges {
                        let key = (from, sym.clone());
                        let entry = best.entry(key).or_insert((f64::MIN, None));
                        if w > entry.0 {
                            entry.0 = w;
                        }
                    }
                }
                Lookup::Pending => pending += 1,
            }
        }
    }

    let edges = best
        .into_iter()
        .map(|((from, to), (weight, basis))| Edge {
            from,
            to,
            weight,
            basis,
        })
        .collect();

    ResolvedEdgeLayer {
        layer_index,
        relation: relation_id.to_string(),
        edges,
        directed: true,
        style: spec.style,
        color: spec.color.clone().unwrap_or_else(default_color),
        pending,
        deps: provider.deps().union(Deps::TREE).union(Deps::RELATIONS),
    }
}

fn resolve_from_pairs(
    pairs: &[EdgePair],
    layer_index: usize,
    spec: &EdgesSpec,
    ctx: &ResolveCtx,
    warnings: &mut Vec<String>,
) -> ResolvedEdgeLayer {
    let mut edges = Vec::new();
    for (i, pair) in pairs.iter().enumerate() {
        let from = resolve_symbol(&pair.from, ctx.tree);
        let to = resolve_symbol(&pair.to, ctx.tree);
        match (from, to) {
            (Some(from), Some(to)) => edges.push(Edge {
                from,
                to,
                weight: pair.weight,
                basis: pair.basis.clone(),
            }),
            _ => {
                warnings.push(format!(
                    "edges[{layer_index}].pairs[{i}]: unresolved endpoint ('{}' -> '{}')",
                    pair.from, pair.to
                ));
            }
        }
    }

    ResolvedEdgeLayer {
        layer_index,
        relation: "pairs".to_string(),
        edges,
        directed: true,
        style: spec.style,
        color: spec.color.clone().unwrap_or_else(default_color),
        pending: 0,
        deps: Deps::TREE,
    }
}

/// Resolve a wire-format symbol id string against the tree, confirming it
/// actually exists (either an exact id match or a matching qualified path).
fn resolve_symbol(wire: &str, tree: &SymbolTree) -> Option<SymbolId> {
    let parsed = crate::symbol_id::parse_wire(wire).ok()?;

    fn walk_by_id<'a>(node: &'a SymbolNode, target: &SymbolId) -> Option<&'a SymbolId> {
        if &node.id == target {
            return Some(&node.id);
        }
        for child in &node.children {
            if let Some(id) = walk_by_id(child, target) {
                return Some(id);
            }
        }
        None
    }
    if let Some(id) = walk_by_id(&tree.root, &parsed) {
        return Some(id.clone());
    }

    fn walk_by_path<'a>(node: &'a SymbolNode, path: &str) -> Option<&'a SymbolId> {
        if node.id.qualified_path == path {
            return Some(&node.id);
        }
        for child in &node.children {
            if let Some(id) = walk_by_path(child, path) {
                return Some(id);
            }
        }
        None
    }
    walk_by_path(&tree.root, &parsed.qualified_path).cloned()
}

/// Every function-item leaf in the tree, pre-order.
fn fn_leaves(tree: &SymbolTree) -> Vec<SymbolId> {
    let mut out = Vec::new();
    fn walk(node: &SymbolNode, out: &mut Vec<SymbolId>) {
        if let SymbolKind::Item { label } = &node.id.kind {
            if label == "fn" {
                out.push(node.id.clone());
            }
        }
        for child in &node.children {
            walk(child, out);
        }
    }
    walk(&tree.root, &mut out);
    out
}
