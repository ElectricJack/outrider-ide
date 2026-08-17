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

/// Incremental resolver -- caches resolved sets and the last resolved view.
pub struct ViewResolver {
    path_index: Option<PathIndex>,
    cached: Option<ResolvedView>,
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
        }
    }

    pub fn invalidate_all(&mut self) {
        self.path_index = None;
        self.cached = None;
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

        let sctx = SetCtx {
            metrics: ctx.metrics,
            relations: ctx.relations,
            partitions: ctx.partitions,
            repo_root: ctx.repo_root,
        };

        // 1. Resolve sets in dependency order
        let mut resolved_sets = BTreeMap::new();
        for (name, expr) in &spec.sets {
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

        for (layer_idx, layer) in spec.layers.iter().enumerate() {
            match layer {
                LayerSpec::Fill(fill_spec) => {
                    // Resolve domain set if specified
                    let domain = fill_spec.domain.as_ref().and_then(|d| match d {
                        SetRef::Name(n) => resolved_sets.get(n),
                        SetRef::Inline(_) => None, // TODO: inline domain expressions
                    });
                    if let Some(resolved) = crate::layers::fill::resolve_fill(
                        fill_spec,
                        ctx.metrics,
                        ctx.tree,
                        domain,
                        &mut warnings,
                    ) {
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
                    let resolved_mask = crate::layers::mask::resolve_mask(
                        mask_spec,
                        &resolved_sets,
                        ctx.tree,
                        &mut warnings,
                    );
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
                    let resolved_edges = crate::layers::edges::resolve_edges(
                        edges_spec,
                        layer_idx,
                        &resolved_sets,
                        ctx,
                        &mut warnings,
                    );
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

        self.cached = Some(resolved);
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
    fn two_pass_visible_resolution() {
        let tree = sample_tree();
        let layout = PackLayout {
            rects: std::collections::BTreeMap::new(),
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
