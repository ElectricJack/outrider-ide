//! View resolution: spec + session state -> resolved channels.

use std::collections::BTreeMap;
use std::path::Path;

use outrider_index::{SymbolId, SymbolTree};
use outrider_layout::PackLayout;

use crate::deps::Deps;
use crate::layers::edges::ResolvedEdgeLayer;
use crate::layers::fill::ResolvedFill;
use crate::layers::marks::MarkTable;
use crate::layers::mask::MaskTable;
use crate::layers::notes::NoteTable;
use crate::layers::panel::ResolvedPanel;
use crate::metric::MetricRegistry;
use crate::partition::PartitionRegistry;
use crate::relation::RelationRegistry;
use crate::set::{resolve_set_expr, PathIndex, ResolvedSet, SetCtx};
use crate::spec::{CameraSpec, FillChannel, LayerSpec, SetRef, ViewSpec};

/// Session-transient state visible to the resolver.
pub struct SessionState<'a> {
    pub focus: &'a SymbolId,
    pub hover: Option<&'a SymbolId>,
    pub selection: Option<&'a SymbolId>,
    pub visible: Option<&'a [SymbolId]>,
    pub head: Option<&'a str>,
    pub neighbors: Option<&'a [Option<SymbolId>; 4]>,
}

/// Everything the resolver reads (immutable per frame).
pub struct ResolveCtx<'a> {
    pub tree: &'a SymbolTree,
    pub layout: &'a PackLayout,
    pub metrics: &'a MetricRegistry,
    pub relations: &'a RelationRegistry,
    pub partitions: &'a PartitionRegistry,
    pub session: SessionState<'a>,
    pub repo_root: &'a Path,
}

/// The fully resolved view -- consumed by PaintOverrides.
#[derive(Debug, Clone)]
pub struct ResolvedView {
    pub sets: BTreeMap<String, ResolvedSet>,
    pub fill: Option<ResolvedFill>,
    pub stripe: Option<ResolvedFill>,
    pub opacity: Option<ResolvedFill>,
    pub marks: MarkTable,
    pub mask: MaskTable,
    pub notes: NoteTable,
    pub edges: Vec<ResolvedEdgeLayer>,
    pub panels: Vec<ResolvedPanel>,
    pub camera: CameraSpec,
    pub deps: Deps,
    pub warnings: Vec<String>,
}

impl Default for ResolvedView {
    fn default() -> Self {
        ResolvedView {
            sets: BTreeMap::new(),
            fill: None,
            stripe: None,
            opacity: None,
            marks: MarkTable::default(),
            mask: MaskTable::default(),
            notes: NoteTable::default(),
            edges: Vec::new(),
            panels: Vec::new(),
            camera: CameraSpec::default(),
            deps: Deps::NONE,
            warnings: Vec::new(),
        }
    }
}

/// Names of sets an expression refers to (directly or via nested exprs).
fn collect_refs(expr: &crate::spec::SetExpr, out: &mut Vec<String>) {
    use crate::spec::SetExpr;
    let mut push_ref = |r: &SetRef| {
        match r {
            SetRef::Name(n) => out.push(n.clone()),
            SetRef::Inline(e) => collect_refs(e, out),
        }
    };
    match expr {
        SetExpr::Ref(n) => out.push(n.clone()),
        SetExpr::Union(v) | SetExpr::Intersect(v) => {
            for e in v {
                collect_refs(e, out);
            }
        }
        SetExpr::Diff(pair) => {
            collect_refs(&pair[0], out);
            collect_refs(&pair[1], out);
        }
        SetExpr::Not(e) => collect_refs(e, out),
        SetExpr::Ancestors(r)
        | SetExpr::Descendants(r)
        | SetExpr::Children(r)
        | SetExpr::FileOf(r) => push_ref(r),
        SetExpr::Reach(r) => collect_refs(&r.from, out),
        SetExpr::Ids(_)
        | SetExpr::Glob(_)
        | SetExpr::Kind(_)
        | SetExpr::Neighbors(_)
        | SetExpr::Fuzzy(_)
        | SetExpr::Visible(_)
        | SetExpr::Changed(_)
        | SetExpr::Where(_)
        | SetExpr::Community(_)
        | SetExpr::Layer(_) => {}
    }
}

/// Named sets an edges layer reads from. Used to decide whether a cached
/// edge resolution survives a spec change: it is only valid when every
/// set it references was itself reused.
fn edges_set_refs(spec: &crate::spec::EdgesSpec) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add_ref = |r: &crate::spec::SetRef, out: &mut Vec<String>| match r {
        crate::spec::SetRef::Name(n) => out.push(n.clone()),
        crate::spec::SetRef::Inline(e) => collect_refs(e, out),
    };
    if let Some(r) = &spec.within {
        add_ref(r, &mut out);
    }
    if let Some(r) = &spec.incident_to {
        add_ref(r, &mut out);
    }
    if let Some(pairs) = &spec.pairs {
        for p in pairs {
            out.push(p.from.clone());
            out.push(p.to.clone());
        }
    }
    out
}

/// Incremental resolver -- caches resolved sets and the last resolved view.
pub struct ViewResolver {
    path_index: Option<PathIndex>,
    cached: Option<ResolvedView>,
    /// The spec the cached view was resolved against. Lets a spec change
    /// (tab switch, tour layer push/pop) reuse cached sets and layers
    /// whose definitions are unchanged, instead of a full recompute.
    last_spec: Option<ViewSpec>,
    /// Per-item timings of the last resolve (name, microseconds), only
    /// populated when `OUTRIDER_PROFILE` is set. Profiling aid.
    pub last_timings: Vec<(String, u128)>,
    /// Per-layer mask results from recent resolves, keyed by the layer's
    /// spec. `ResolvedView` only stores the merged mask, so reusing an
    /// individual layer across resolves needs this side table.
    mask_layers: Vec<(crate::spec::MaskSpec, crate::layers::mask::MaskTable)>,
    /// Recently resolved views keyed by their full spec (MRU, bounded).
    /// Lets a switch back to a previously shown view (tab switches, tour
    /// runs across tabs) start from that view's own resolution instead of
    /// recomputing every set from scratch. Session-dependent artifacts are
    /// still recomputed: a cache restore treats every session input as
    /// dirty. Cleared whenever the tree changes.
    spec_cache: Vec<(ViewSpec, ResolvedView)>,
}

/// Cap for `ViewResolver::spec_cache` (one entry per distinct spec).
const SPEC_CACHE_CAP: usize = 4;

/// Every session-data dependency bit — what a spec-cache restore must
/// treat as dirty, since we don't track session changes per cache entry.
const ALL_SESSION_DEPS: Deps = Deps::FOCUS
    .union(Deps::CAMERA)
    .union(Deps::SELECTION)
    .union(Deps::HOVER)
    .union(Deps::GIT)
    .union(Deps::METRICS)
    .union(Deps::RELATIONS);

/// Named sets a mask layer reads from (for cache validity, like
/// `edges_set_refs`).
fn mask_set_refs(spec: &crate::spec::MaskSpec) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in [&spec.dim_except, &spec.dim].into_iter().flatten() {
        match r {
            crate::spec::SetRef::Name(n) => out.push(n.clone()),
            crate::spec::SetRef::Inline(e) => collect_refs(e, &mut out),
        }
    }
    out
}

fn profiling() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OUTRIDER_PROFILE").is_some())
}

impl Default for ViewResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewResolver {
    pub fn new() -> Self {
        ViewResolver {
            path_index: None,
            cached: None,
            last_spec: None,
            last_timings: Vec::new(),
            mask_layers: Vec::new(),
            spec_cache: Vec::new(),
        }
    }

    pub fn invalidate_all(&mut self) {
        self.path_index = None;
        self.cached = None;
        self.last_spec = None;
        self.mask_layers.clear();
        self.spec_cache.clear();
    }

    pub fn resolve(&mut self, spec: &ViewSpec, ctx: &ResolveCtx, dirty: Deps) -> &ResolvedView {
        // Rebuild path index on tree change
        if dirty.intersects(Deps::TREE) || self.path_index.is_none() {
            self.path_index = Some(PathIndex::new(ctx.tree));
        }

        // If nothing is dirty and we have a cached result, return it
        if dirty.is_none() && self.cached.is_some() {
            return self.cached.as_ref().unwrap();
        }

        let path_index = self.path_index.as_ref().unwrap();
        let mut warnings = Vec::new();
        let prof = profiling();
        let mut timings: Vec<(String, u128)> = Vec::new();

        let sctx = SetCtx {
            metrics: ctx.metrics,
            relations: ctx.relations,
            partitions: ctx.partitions,
            repo_root: ctx.repo_root,
        };

        // Incremental path: when the spec and tree are unchanged, a previous
        // resolution is a valid starting point. Anything whose recorded deps
        // don't intersect `dirty` is reused verbatim; only the rest is
        // recomputed. This is what keeps hover/focus/selection ticks cheap
        // on large trees (a full resolve walks every node per fill layer).
        let incremental = !dirty.intersects(Deps::SPEC | Deps::TREE);
        // Spec changes (tab switches, tour layer push/pop) usually share
        // most of their sets and layers with the previous spec. As long
        // as the tree is unchanged, anything whose *definition* is
        // identical to last time is still valid subject to its data deps
        // — reuse it instead of recomputing from scratch.
        let spec_reuse = !incremental && !dirty.intersects(Deps::TREE);
        let prev = if incremental || spec_reuse {
            self.cached.take()
        } else {
            None
        };
        if prev.is_none() {
            self.mask_layers.clear();
            self.spec_cache.clear();
        }
        let mut prev = prev;
        let mut prev_spec = self.last_spec.take().filter(|_| spec_reuse);
        // Deps to test cached artifacts against: on a spec change the SPEC
        // bit is answered by definition equality, not by the deps bitset.
        let mut data_dirty = dirty.without(Deps::SPEC);
        // Switching to a spec we resolved before (tab switch back): start
        // from that spec's own resolution — every definition matches — but
        // treat all session inputs as dirty, since the entry may predate
        // arbitrary focus/hover/selection changes.
        if spec_reuse
            && dirty.intersects(Deps::SPEC)
            && prev_spec.as_ref().map_or(true, |ps| ps.sets != spec.sets)
        {
            // Match on the set collection, not the full spec: tour steps
            // push/pop layers on top of a tab's spec, so the layer list is
            // rarely identical — but the sets (the expensive part) are.
            // Layer reuse below matches by definition, not by index, so a
            // prev with different layers is still a valid starting point.
            if let Some((s, rv)) = self
                .spec_cache
                .iter()
                .find(|(s, _)| s.sets == spec.sets)
                .cloned()
            {
                prev = Some(rv);
                prev_spec = Some(s);
                data_dirty = data_dirty.union(ALL_SESSION_DEPS);
            }
        }

        // 1. Resolve sets. A cached set is reusable when its own deps are
        //    clean, its definition is unchanged, AND every set it
        //    references is itself reusable (a stale upstream would leak
        //    through a `ref`). Reusability is computed transitively up
        //    front — iteration order is alphabetical, so a set may be
        //    visited before the sets it references.
        let mut reused_sets: std::collections::HashSet<String> = spec
            .sets
            .iter()
            .filter_map(|(name, expr)| {
                let cached = prev.as_ref()?.sets.get(name)?;
                if cached.deps.intersects(data_dirty) {
                    return None;
                }
                // On a spec change the cached set is only valid when its
                // definition is unchanged from the previous spec.
                if dirty.intersects(Deps::SPEC)
                    && prev_spec.as_ref().and_then(|ps| ps.sets.get(name)) != Some(expr)
                {
                    return None;
                }
                Some(name.clone())
            })
            .collect();
        loop {
            let stale: Vec<String> = reused_sets
                .iter()
                .filter(|name| {
                    let mut refs = Vec::new();
                    if let Some(expr) = spec.sets.get(*name) {
                        collect_refs(expr, &mut refs);
                    }
                    !refs.iter().all(|r| reused_sets.contains(r))
                })
                .cloned()
                .collect();
            if stale.is_empty() {
                break;
            }
            for s in stale {
                reused_sets.remove(&s);
            }
        }
        let mut resolved_sets: BTreeMap<String, ResolvedSet> = BTreeMap::new();
        for (name, expr) in &spec.sets {
            // `prev` is owned, so a reused set is moved out, not cloned —
            // cloning id sets on every reuse would cost more than some of
            // the recomputes it avoids.
            let reusable = reused_sets
                .contains(name)
                .then(|| prev.as_mut().unwrap().sets.remove(name).unwrap());
            let set = match reusable {
                Some(set) => set,
                None => {
                    let t0 = prof.then(std::time::Instant::now);
                    let mut stack = Vec::new();
                    let set = resolve_set_expr(
                        expr,
                        &resolved_sets,
                        &ctx.session,
                        path_index,
                        &mut warnings,
                        &mut stack,
                        &spec.sets,
                        ctx.tree,
                        &sctx,
                    );
                    if let Some(t0) = t0 {
                        timings.push((format!("set:{name}"), t0.elapsed().as_micros()));
                    }
                    set
                }
            };
            resolved_sets.insert(name.clone(), set);
        }

        // 2. Resolve layers
        let mut fill: Option<ResolvedFill> = None;
        let mut stripe: Option<ResolvedFill> = None;
        let mut opacity: Option<ResolvedFill> = None;
        let mut marks = MarkTable::default();
        let mut mask = MaskTable::default();
        let mut notes = NoteTable::default();
        let mut edges: Vec<ResolvedEdgeLayer> = Vec::new();
        let mut panels: Vec<ResolvedPanel> = Vec::new();
        let mut all_deps = Deps::NONE;
        let mut new_mask_layers: Vec<(crate::spec::MaskSpec, crate::layers::mask::MaskTable)> =
            Vec::new();

        for (layer_idx, layer) in spec.layers.iter().enumerate() {
            let lt0 = prof.then(std::time::Instant::now);
            match layer {
                LayerSpec::Fill(fill_spec) => {
                    // Reuse the previous fill for this channel when its deps
                    // are clean and its domain set (if any) was reused.
                    let domain_name = match &fill_spec.domain {
                        Some(SetRef::Name(n)) => Some(n.as_str()),
                        _ => None,
                    };
                    let domain_clean = domain_name.map_or(true, |n| reused_sets.contains(n));
                    // On a spec change the cached channel is only valid
                    // when the previous spec had an identical fill layer.
                    let def_unchanged = !dirty.intersects(Deps::SPEC)
                        || prev_spec.as_ref().is_some_and(|ps| {
                            ps.layers
                                .iter()
                                .any(|l| matches!(l, LayerSpec::Fill(f) if f == fill_spec))
                        });
                    let cached = prev.as_ref().and_then(|pv| {
                        let c = match fill_spec.channel {
                            FillChannel::Fill => pv.fill.as_ref(),
                            FillChannel::Stripe => pv.stripe.as_ref(),
                            FillChannel::Opacity => pv.opacity.as_ref(),
                        }?;
                        (def_unchanged && domain_clean && !c.deps.intersects(data_dirty))
                            .then(|| c.clone())
                    });
                    let resolved = match cached {
                        Some(c) => Some(c),
                        None => {
                            let domain = domain_name.and_then(|n| resolved_sets.get(n));
                            crate::layers::fill::resolve_fill(
                                fill_spec,
                                ctx.metrics,
                                ctx.tree,
                                domain,
                                &mut warnings,
                            )
                        }
                    };
                    if let Some(resolved) = resolved {
                        all_deps = all_deps.union(resolved.deps);
                        match fill_spec.channel {
                            FillChannel::Fill => fill = Some(resolved),
                            FillChannel::Stripe => stripe = Some(resolved),
                            FillChannel::Opacity => opacity = Some(resolved),
                        }
                    }
                }
                LayerSpec::Marks(marks_spec) => {
                    crate::layers::marks::resolve_marks(
                        marks_spec,
                        &resolved_sets,
                        &mut warnings,
                        &mut marks,
                    );
                    all_deps = all_deps.union(marks.deps);
                }
                LayerSpec::Mask(mask_spec) => {
                    // Reuse the previous resolution of an identical mask
                    // layer when its deps are clean and every set it
                    // reads was itself reused. Mask resolution walks the
                    // whole tree, so this matters on hover/focus ticks.
                    let refs_ok = mask_set_refs(mask_spec)
                        .iter()
                        .all(|n| reused_sets.contains(n));
                    let cached = refs_ok
                        .then(|| {
                            self.mask_layers.iter().find(|(s, t)| {
                                s == mask_spec && !t.deps.intersects(data_dirty)
                            })
                        })
                        .flatten()
                        .map(|(_, t)| t.clone());
                    if prof && cached.is_none() {
                        let why = if !refs_ok {
                            1
                        } else if self.mask_layers.iter().any(|(s, _)| s == mask_spec) {
                            2 // present but deps dirty
                        } else {
                            3 // not in cache
                        };
                        timings.push((format!("maskmiss{layer_idx}"), why));
                    }
                    let resolved_mask = match cached {
                        Some(c) => c,
                        None => crate::layers::mask::resolve_mask(
                            mask_spec,
                            &resolved_sets,
                            ctx.tree,
                            &mut warnings,
                        ),
                    };
                    new_mask_layers.push((mask_spec.clone(), resolved_mask.clone()));
                    all_deps = all_deps.union(resolved_mask.deps);
                    mask = mask.and(&resolved_mask);
                }
                LayerSpec::Notes(notes_spec) => {
                    crate::layers::notes::resolve_notes(
                        layer_idx,
                        notes_spec,
                        ctx,
                        &resolved_sets,
                        &mut notes,
                        &mut warnings,
                    );
                    all_deps = all_deps.union(notes.deps);
                }
                LayerSpec::Edges(edges_spec) => {
                    // Edge layers are keyed by layer index; reuse when clean,
                    // unless a previous lookup was still pending (async
                    // provider) — then retry so the edges fill in. On a spec
                    // change, match by definition instead: find the previous
                    // layer with an identical edges spec and rehome its
                    // resolution under the current index — valid only when
                    // every set it references was itself reused.
                    let cached = prev.as_ref().and_then(|pv| {
                        let prev_idx = if dirty.intersects(Deps::SPEC) {
                            let ps = prev_spec.as_ref()?;
                            let sets_ok = edges_set_refs(edges_spec)
                                .iter()
                                .all(|n| reused_sets.contains(n));
                            if !sets_ok {
                                return None;
                            }
                            ps.layers.iter().enumerate().find_map(|(i, l)| {
                                matches!(l, LayerSpec::Edges(e) if e == edges_spec)
                                    .then_some(i)
                            })?
                        } else {
                            layer_idx
                        };
                        let c = pv.edges.iter().find(|e| e.layer_index == prev_idx)?;
                        (c.pending == 0 && !c.deps.intersects(data_dirty)).then(|| {
                            let mut c = c.clone();
                            c.layer_index = layer_idx;
                            c
                        })
                    });
                    let resolved_edges = match cached {
                        Some(c) => c,
                        None => crate::layers::edges::resolve_edges(
                            edges_spec,
                            layer_idx,
                            &resolved_sets,
                            ctx,
                            &mut warnings,
                        ),
                    };
                    all_deps = all_deps.union(resolved_edges.deps);
                    edges.push(resolved_edges);
                }
                LayerSpec::Panel(panel_spec) => {
                    let resolved = crate::layers::panel::resolve_panel(
                        layer_idx,
                        panel_spec,
                        ctx,
                        &resolved_sets,
                        &mut warnings,
                    );
                    all_deps = all_deps.union(resolved.deps);
                    panels.push(resolved);
                }
            }
            if let Some(lt0) = lt0 {
                let kind = match layer {
                    LayerSpec::Fill(_) => "fill",
                    LayerSpec::Marks(_) => "marks",
                    LayerSpec::Mask(_) => "mask",
                    LayerSpec::Notes(_) => "notes",
                    LayerSpec::Edges(_) => "edges",
                    LayerSpec::Panel(_) => "panel",
                };
                timings.push((format!("layer{layer_idx}:{kind}"), lt0.elapsed().as_micros()));
            }
        }

        // Merge set deps
        for set in resolved_sets.values() {
            all_deps = all_deps.union(set.deps);
        }

        let resolved = ResolvedView {
            sets: resolved_sets,
            fill,
            stripe,
            opacity,
            marks,
            mask,
            notes,
            edges,
            panels,
            camera: spec.camera.clone(),
            deps: all_deps,
            warnings,
        };

        // On spec changes, remember this resolution so switching back to
        // this spec later restarts from it (see `spec_cache`).
        // Store at most one entry per distinct set collection, and only on
        // first sight: the deep clone is not free, and later resolves of
        // the same sets add nothing (a restore treats all session inputs
        // as dirty anyway).
        if dirty.intersects(Deps::SPEC)
            && !self.spec_cache.iter().any(|(s, _)| s.sets == spec.sets)
        {
            self.spec_cache.insert(0, (spec.clone(), resolved.clone()));
            self.spec_cache.truncate(SPEC_CACHE_CAP);
        }
        self.cached = Some(resolved);
        self.last_spec = Some(spec.clone());
        self.last_timings = timings;
        // Keep mask results from other recent specs alive so a tab switch
        // back doesn't rebuild them; new results shadow same-spec entries.
        for (s, t) in std::mem::take(&mut self.mask_layers) {
            if new_mask_layers.len() >= 12 {
                break;
            }
            if !new_mask_layers.iter().any(|(ns, _)| *ns == s) {
                new_mask_layers.push((s, t));
            }
        }
        self.mask_layers = new_mask_layers;
        self.cached.as_ref().unwrap()
    }

    pub fn current(&self) -> Option<&ResolvedView> {
        self.cached.as_ref()
    }

    pub fn needs_git(&self) -> bool {
        self.cached
            .as_ref()
            .map_or(false, |r| r.deps.intersects(Deps::GIT))
    }

    pub fn needs_visible(&self) -> bool {
        self.cached
            .as_ref()
            .map_or(false, |r| r.deps.intersects(Deps::CAMERA))
    }

    /// Re-resolve with the visible list populated.
    ///
    /// The caller runs the first pass via `resolve()` with `visible: None`,
    /// uses the result to determine which nodes are on screen, then calls
    /// this method so that sets using `SetExpr::Visible(true)` get the
    /// actual visible ids.
    pub fn resolve_visible(
        &mut self,
        spec: &ViewSpec,
        ctx: &ResolveCtx,
        visible: &[SymbolId],
    ) -> &ResolvedView {
        let session = SessionState {
            visible: Some(visible),
            focus: ctx.session.focus,
            hover: ctx.session.hover,
            selection: ctx.session.selection,
            head: ctx.session.head,
            neighbors: ctx.session.neighbors,
        };
        let ctx2 = ResolveCtx {
            tree: ctx.tree,
            layout: ctx.layout,
            metrics: ctx.metrics,
            relations: ctx.relations,
            partitions: ctx.partitions,
            session,
            repo_root: ctx.repo_root,
        };
        self.resolve(spec, &ctx2, Deps::CAMERA)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{SetExpr, ViewSpec};
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
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

    fn file_id(name: &str) -> SymbolId {
        SymbolId {
            kind: SymbolKind::File,
            qualified_path: name.to_string(),
            ordinal: 0,
        }
    }

    #[test]
    fn hover_tick_reuses_clean_sets_and_recomputes_hover_sets() {
        let tree = sample_tree();
        let layout = PackLayout {
            rects: std::collections::HashMap::new(),
        };
        let metrics = MetricRegistry::builtin();
        let relations = RelationRegistry::empty();
        let partitions = PartitionRegistry::default();
        let focus = file_id("a.rs");
        let a = file_id("a.rs");
        let b = file_id("b.rs");

        let mut spec = ViewSpec::default();
        spec.sets
            .insert("all".to_string(), SetExpr::Glob("*.rs".to_string()));
        spec.sets
            .insert("hov".to_string(), SetExpr::Ids(vec!["$hover".to_string()]));

        let mut resolver = ViewResolver::new();
        let ctx1 = ResolveCtx {
            tree: &tree,
            layout: &layout,
            metrics: &metrics,
            relations: &relations,
            partitions: &partitions,
            session: SessionState {
                focus: &focus,
                hover: Some(&a),
                selection: None,
                visible: None,
                head: None,
                neighbors: None,
            },
            repo_root: std::path::Path::new("."),
        };
        let r1 = resolver.resolve(&spec, &ctx1, Deps::SPEC);
        assert_eq!(r1.sets["all"].ids.len(), 2);
        assert!(r1.sets["hov"].ids.contains(&a));

        // Hover moves to b: only the hover set changes.
        let ctx2 = ResolveCtx {
            tree: &tree,
            layout: &layout,
            metrics: &metrics,
            relations: &relations,
            partitions: &partitions,
            session: SessionState {
                focus: &focus,
                hover: Some(&b),
                selection: None,
                visible: None,
                head: None,
                neighbors: None,
            },
            repo_root: std::path::Path::new("."),
        };
        let r2 = resolver.resolve(&spec, &ctx2, Deps::HOVER);
        assert_eq!(r2.sets["all"].ids.len(), 2, "glob set survives hover tick");
        assert!(r2.sets["hov"].ids.contains(&b), "hover set follows the hover");
        assert!(!r2.sets["hov"].ids.contains(&a));
        assert!(r2.sets["hov"].deps.intersects(Deps::HOVER));
        assert!(!r2.sets["all"].deps.intersects(Deps::HOVER));
    }

    #[test]
    fn two_pass_visible_resolution() {
        let tree = sample_tree();
        let layout = PackLayout {
            rects: std::collections::HashMap::new(),
        };
        let metrics = MetricRegistry::builtin();
        let relations = RelationRegistry::empty();
        let partitions = PartitionRegistry::default();
        let focus = file_id("a.rs");

        // Build a spec with a set that uses Visible(true)
        let mut spec = ViewSpec::default();
        spec.sets
            .insert("onscreen".to_string(), SetExpr::Visible(true));

        // --- First pass: visible is None ---
        let session = SessionState {
            focus: &focus,
            hover: None,
            selection: None,
            visible: None,
            head: None,
            neighbors: None,
        };
        let ctx = ResolveCtx {
            tree: &tree,
            layout: &layout,
            metrics: &metrics,
            relations: &relations,
            partitions: &partitions,
            session,
            repo_root: std::path::Path::new("."),
        };

        let mut resolver = ViewResolver::new();
        let first = resolver.resolve(&spec, &ctx, Deps::TREE);

        // The set should be empty because visible was None
        assert!(
            first.sets["onscreen"].ids.is_empty(),
            "first pass with visible=None should yield empty set"
        );
        // needs_visible should be true because the set depends on CAMERA
        assert!(
            resolver.needs_visible(),
            "needs_visible should be true when a set uses Visible"
        );

        // --- Second pass: supply visible ids ---
        let visible_ids = vec![file_id("a.rs"), file_id("b.rs")];

        let session2 = SessionState {
            focus: &focus,
            hover: None,
            selection: None,
            visible: None, // resolve_visible will override this
            head: None,
            neighbors: None,
        };
        let ctx2 = ResolveCtx {
            tree: &tree,
            layout: &layout,
            metrics: &metrics,
            relations: &relations,
            partitions: &partitions,
            session: session2,
            repo_root: std::path::Path::new("."),
        };

        let second = resolver.resolve_visible(&spec, &ctx2, &visible_ids);

        assert_eq!(
            second.sets["onscreen"].ids.len(),
            2,
            "second pass should contain the visible ids"
        );
        assert!(second.sets["onscreen"].ids.contains(&file_id("a.rs")));
        assert!(second.sets["onscreen"].ids.contains(&file_id("b.rs")));
    }
}
