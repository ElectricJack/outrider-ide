use std::collections::HashMap;
use std::ops::Range;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};

use crate::deps::Deps;
use crate::resolve::ResolveCtx;
use crate::set::ResolvedSet;
use crate::spec::{AnchorSpan, LiveAnchor, NoteAnchor, NoteSource, NotesSpec};

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedNote {
    pub source: NoteSource,
    pub text: String,
    pub range: Option<Range<usize>>,
    pub lines: Option<(usize, usize)>,
    pub pin: Option<(f32, f32)>,
    pub layer: usize,
}

#[derive(Debug, Default, Clone)]
pub struct NoteTable {
    pub by_symbol: HashMap<SymbolId, Vec<ResolvedNote>>,
    pub deps: Deps,
}

impl NoteTable {
    pub fn get(&self, id: &SymbolId) -> &[ResolvedNote] {
        self.by_symbol.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.by_symbol.is_empty()
    }
}

pub fn resolve_notes(
    layer_idx: usize,
    spec: &NotesSpec,
    ctx: &ResolveCtx,
    _sets: &std::collections::BTreeMap<String, ResolvedSet>,
    out: &mut NoteTable,
    warnings: &mut Vec<String>,
) {
    for (i, note) in spec.0.iter().enumerate() {
        let (id, range, lines) = match &note.at {
            NoteAnchor::Live(live) => {
                let id = match live {
                    LiveAnchor::Focus => {
                        out.deps |= Deps::FOCUS;
                        Some(ctx.session.focus.clone())
                    }
                    LiveAnchor::Hover => {
                        out.deps |= Deps::HOVER;
                        ctx.session.hover.cloned()
                    }
                    LiveAnchor::Selection => {
                        out.deps |= Deps::SELECTION;
                        ctx.session.selection.cloned()
                    }
                };
                match id {
                    Some(id) => (id, None, None),
                    None => continue,
                }
            }
            NoteAnchor::Symbol(wire) => {
                out.deps |= Deps::TREE;
                match resolve_wire_id(wire, ctx.tree) {
                    Some(id) => (id, None, None),
                    None => {
                        warnings.push(format!("notes[{i}]: unknown symbol \"{wire}\""));
                        continue;
                    }
                }
            }
            NoteAnchor::Range { symbol, span } => {
                out.deps |= Deps::TREE;
                match resolve_wire_id(symbol, ctx.tree) {
                    Some(id) => {
                        let (range, lines) = match span {
                            AnchorSpan::Range([a, b]) => (Some(*a..*b), None),
                            AnchorSpan::Lines([a, b]) => (None, Some((*a, *b))),
                        };
                        (id, range, lines)
                    }
                    None => {
                        warnings.push(format!("notes[{i}]: unknown symbol \"{symbol}\""));
                        continue;
                    }
                }
            }
        };

        let text = match note.source {
            NoteSource::Doc => {
                if let Some(t) = note.text.as_ref().filter(|t| !t.is_empty()) {
                    t.clone()
                } else {
                    match find_node(ctx.tree, &id).and_then(|n| n.doc.clone()) {
                        Some(d) if !d.is_empty() => d,
                        _ => continue,
                    }
                }
            }
            NoteSource::Metric => {
                if let Some(t) = note.text.as_ref().filter(|t| !t.is_empty()) {
                    t.clone()
                } else {
                    match find_node(ctx.tree, &id) {
                        Some(node) => format!(
                            "{}L · {} commits · p{:.0}",
                            node.measure,
                            node.churn_count,
                            node.churn * 100.0
                        ),
                        None => continue,
                    }
                }
            }
            NoteSource::Agent => match note.text.as_ref().filter(|t| !t.trim().is_empty()) {
                Some(t) => t.clone(),
                None => {
                    warnings.push(format!("notes[{i}]: agent note has no text"));
                    continue;
                }
            },
        };

        out.by_symbol.entry(id).or_default().push(ResolvedNote {
            source: note.source,
            text,
            range,
            lines,
            pin: note.pin,
            layer: layer_idx,
        });
    }

    out.deps |= Deps::TREE;
}

fn resolve_wire_id(wire: &str, tree: &SymbolTree) -> Option<SymbolId> {
    let parsed = crate::symbol_id::parse_wire(wire).ok()?;

    fn walk<'a>(node: &'a SymbolNode, target: &SymbolId) -> Option<&'a SymbolId> {
        if &node.id == target {
            return Some(&node.id);
        }
        for child in &node.children {
            if let Some(id) = walk(child, target) {
                return Some(id);
            }
        }
        None
    }
    if let Some(id) = walk(&tree.root, &parsed) {
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

fn find_node<'a>(tree: &'a SymbolTree, id: &SymbolId) -> Option<&'a SymbolNode> {
    fn walk<'a>(node: &'a SymbolNode, target: &SymbolId) -> Option<&'a SymbolNode> {
        if &node.id == target {
            return Some(node);
        }
        for child in &node.children {
            if let Some(n) = walk(child, target) {
                return Some(n);
            }
        }
        None
    }
    walk(&tree.root, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::SessionState;
    use crate::spec::{LayerSpec, NoteSpec, ViewSpec};
    use crate::validate;
    use outrider_index::SymbolKind;
    use outrider_layout::PackLayout;

    fn leaf(path: &str, doc: Option<&str>) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::File,
                qualified_path: path.to_string(),
                ordinal: 0,
            },
            name: path.to_string(),
            byte_range: None,
            signature: None,
            doc: doc.map(|d| d.to_string()),
            measure: 42,
            churn: 0.5,
            churn_count: 7,
            children: vec![],
        }
    }

    fn sample_tree(children: Vec<SymbolNode>) -> SymbolTree {
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
            children,
        };
        SymbolTree {
            root,
            repo_root: std::path::PathBuf::from("."),
        }
    }

    fn make_ctx<'a>(
        tree: &'a SymbolTree,
        layout: &'a PackLayout,
        metrics: &'a crate::metric::MetricRegistry,
        relations: &'a crate::relation::RelationRegistry,
        session: SessionState<'a>,
        repo_root: &'a std::path::Path,
    ) -> ResolveCtx<'a> {
        static EMPTY_PARTS: std::sync::LazyLock<crate::partition::PartitionRegistry> =
            std::sync::LazyLock::new(crate::partition::PartitionRegistry::default);
        ResolveCtx {
            tree,
            layout,
            metrics,
            relations,
            partitions: &EMPTY_PARTS,
            session,
            repo_root,
        }
    }

    #[test]
    fn notes_json_roundtrip() {
        let cases = [
            r#"{"notes": [{"at": "$hover", "source": "doc"}]}"#,
            r#"{"notes": [{"at": "fn:src/auth/login.rs::verify", "source": "agent", "text": "This now calls into billing."}]}"#,
            r#"{"notes": [{"at": {"symbol": "fn:src/a.rs::verify", "lines": [40, 52]}, "source": "agent", "text": "new cross-module call", "pin": [1.0, 0.0]}]}"#,
            r#"{"notes": [{"at": "$focus", "source": "metric", "readout": "inventory"}]}"#,
        ];
        for case in cases {
            let value: serde_json::Value = serde_json::from_str(case).unwrap();
            let spec: NotesSpec = serde_json::from_value(value["notes"].clone()).unwrap();
            let round = serde_json::to_value(&spec).unwrap();
            let back: NotesSpec = serde_json::from_value(round.clone()).unwrap();
            assert_eq!(spec, back, "roundtrip mismatch for {case}");
        }
    }

    #[test]
    fn agent_note_without_text_rejected() {
        let spec = ViewSpec {
            layers: vec![LayerSpec::Notes(NotesSpec(vec![NoteSpec {
                at: NoteAnchor::Live(LiveAnchor::Focus),
                source: NoteSource::Agent,
                text: None,
                pin: None,
                readout: None,
            }]))],
            ..ViewSpec::default()
        };
        let violations = validate::validate(&spec, &|_| true, &|_| true);
        assert!(violations.iter().any(|v| v.rule == "agent-note-needs-text"));

        let spec_ws = ViewSpec {
            layers: vec![LayerSpec::Notes(NotesSpec(vec![NoteSpec {
                at: NoteAnchor::Live(LiveAnchor::Focus),
                source: NoteSource::Agent,
                text: Some("   ".to_string()),
                pin: None,
                readout: None,
            }]))],
            ..ViewSpec::default()
        };
        let violations = validate::validate(&spec_ws, &|_| true, &|_| true);
        assert!(violations.iter().any(|v| v.rule == "agent-note-needs-text"));

        let spec_doc = ViewSpec {
            layers: vec![LayerSpec::Notes(NotesSpec(vec![NoteSpec {
                at: NoteAnchor::Live(LiveAnchor::Focus),
                source: NoteSource::Doc,
                text: None,
                pin: None,
                readout: None,
            }]))],
            ..ViewSpec::default()
        };
        let violations = validate::validate(&spec_doc, &|_| true, &|_| true);
        assert!(!violations.iter().any(|v| v.rule == "agent-note-needs-text"));
    }

    #[test]
    fn pin_out_of_range_rejected() {
        let spec = ViewSpec {
            layers: vec![LayerSpec::Notes(NotesSpec(vec![NoteSpec {
                at: NoteAnchor::Live(LiveAnchor::Focus),
                source: NoteSource::Doc,
                text: Some("hi".to_string()),
                pin: Some((1.5, 0.5)),
                readout: None,
            }]))],
            ..ViewSpec::default()
        };
        let violations = validate::validate(&spec, &|_| true, &|_| true);
        assert!(violations.iter().any(|v| v.rule == "pin-out-of-range"));
    }

    #[test]
    fn anchor_range_inverted_rejected() {
        let spec = ViewSpec {
            layers: vec![LayerSpec::Notes(NotesSpec(vec![NoteSpec {
                at: NoteAnchor::Range {
                    symbol: "fn:src/a.rs::verify".to_string(),
                    span: AnchorSpan::Range([50, 10]),
                },
                source: NoteSource::Doc,
                text: Some("hi".to_string()),
                pin: None,
                readout: None,
            }]))],
            ..ViewSpec::default()
        };
        let violations = validate::validate(&spec, &|_| true, &|_| true);
        assert!(violations.iter().any(|v| v.rule == "anchor-range-inverted"));
    }

    #[test]
    fn doc_note_resolves_from_node_doc() {
        let tree = sample_tree(vec![leaf("src/a.rs", Some("Hello docs"))]);
        let focus = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/a.rs".to_string(),
            ordinal: 0,
        };
        let layout = PackLayout {
            rects: Default::default(),
        };
        let metrics = crate::metric::MetricRegistry::builtin();
        let relations = crate::relation::RelationRegistry::empty();
        let session = SessionState {
            focus: &focus,
            hover: None,
            selection: None,
            visible: None,
            head: None,
            neighbors: None,
        };
        let repo_root = std::path::Path::new(".");
        let resolve_ctx = make_ctx(&tree, &layout, &metrics, &relations, session, repo_root);

        let notes_spec = NotesSpec(vec![NoteSpec {
            at: NoteAnchor::Live(LiveAnchor::Focus),
            source: NoteSource::Doc,
            text: None,
            pin: None,
            readout: None,
        }]);
        let mut table = NoteTable::default();
        let mut warnings = Vec::new();
        let sets = std::collections::BTreeMap::new();
        resolve_notes(0, &notes_spec, &resolve_ctx, &sets, &mut table, &mut warnings);

        assert!(warnings.is_empty());
        let notes = table.get(&focus);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "Hello docs");
    }

    #[test]
    fn hover_note_resolves_only_when_hovered() {
        let tree = sample_tree(vec![leaf("src/a.rs", Some("Hello docs"))]);
        let focus = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/a.rs".to_string(),
            ordinal: 0,
        };
        let layout = PackLayout {
            rects: Default::default(),
        };
        let metrics = crate::metric::MetricRegistry::builtin();
        let relations = crate::relation::RelationRegistry::empty();
        let repo_root = std::path::Path::new(".");

        let notes_spec = NotesSpec(vec![NoteSpec {
            at: NoteAnchor::Live(LiveAnchor::Hover),
            source: NoteSource::Doc,
            text: None,
            pin: None,
            readout: None,
        }]);

        {
            let session = SessionState {
                focus: &focus,
                hover: None,
                selection: None,
                visible: None,
                head: None,
                neighbors: None,
            };
            let resolve_ctx = make_ctx(&tree, &layout, &metrics, &relations, session, repo_root);
            let mut table = NoteTable::default();
            let mut warnings = Vec::new();
            let sets = std::collections::BTreeMap::new();
            resolve_notes(0, &notes_spec, &resolve_ctx, &sets, &mut table, &mut warnings);
            assert!(table.is_empty());
            assert!(table.deps.intersects(Deps::HOVER));
        }

        {
            let session = SessionState {
                focus: &focus,
                hover: Some(&focus),
                selection: None,
                visible: None,
                head: None,
                neighbors: None,
            };
            let resolve_ctx = make_ctx(&tree, &layout, &metrics, &relations, session, repo_root);
            let mut table = NoteTable::default();
            let mut warnings = Vec::new();
            let sets = std::collections::BTreeMap::new();
            resolve_notes(0, &notes_spec, &resolve_ctx, &sets, &mut table, &mut warnings);
            assert_eq!(table.get(&focus).len(), 1);
            assert!(table.deps.intersects(Deps::HOVER));
        }
    }

    #[test]
    fn layers_accumulate_in_order() {
        let tree = sample_tree(vec![leaf("src/a.rs", Some("Hello docs"))]);
        let focus = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/a.rs".to_string(),
            ordinal: 0,
        };
        let layout = PackLayout {
            rects: Default::default(),
        };
        let metrics = crate::metric::MetricRegistry::builtin();
        let relations = crate::relation::RelationRegistry::empty();
        let repo_root = std::path::Path::new(".");
        let session = SessionState {
            focus: &focus,
            hover: None,
            selection: None,
            visible: None,
            head: None,
            neighbors: None,
        };
        let resolve_ctx = make_ctx(&tree, &layout, &metrics, &relations, session, repo_root);

        let layer0 = NotesSpec(vec![NoteSpec {
            at: NoteAnchor::Live(LiveAnchor::Focus),
            source: NoteSource::Agent,
            text: Some("first".to_string()),
            pin: None,
            readout: None,
        }]);
        let layer1 = NotesSpec(vec![NoteSpec {
            at: NoteAnchor::Live(LiveAnchor::Focus),
            source: NoteSource::Agent,
            text: Some("second".to_string()),
            pin: None,
            readout: None,
        }]);

        let mut table = NoteTable::default();
        let mut warnings = Vec::new();
        let sets = std::collections::BTreeMap::new();
        resolve_notes(0, &layer0, &resolve_ctx, &sets, &mut table, &mut warnings);
        resolve_notes(1, &layer1, &resolve_ctx, &sets, &mut table, &mut warnings);

        let notes = table.get(&focus);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].layer, 0);
        assert_eq!(notes[1].layer, 1);
    }
}
