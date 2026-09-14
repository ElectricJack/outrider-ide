//! Relation providers and registry (edges between symbols).

pub mod calls;
pub mod inherits;

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use outrider_index::tree_index::TreeIndex;
use outrider_index::{SymbolId, SymbolTree};

use crate::deps::Deps;

use calls::{CallsProvider, CallsWorker};
use inherits::InheritsProvider;

/// What providers see when asked to look up edges.
pub struct ProviderCtx<'a> {
    pub tree: &'a SymbolTree,
    pub index: &'a TreeIndex<'a>,
    pub repo_root: &'a Path,
}

/// Async lookup result.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup<T> {
    Ready(T),
    Pending,
}

/// Direction of edge lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDir {
    Out,
    In,
}

/// Provider-specific facts about one edge (e.g. call site range for the calls provider).
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeDetail {
    pub label: String,
    pub site: Option<Range<usize>>,
}

/// A provider of edges between symbols (parent §7.3).
pub trait RelationProvider: Send + Sync {
    fn id(&self) -> &str;
    fn out_edges(&self, from: &SymbolId, ctx: &ProviderCtx) -> Vec<(SymbolId, f64)>;
    fn in_edges(&self, to: &SymbolId, ctx: &ProviderCtx) -> Vec<(SymbolId, f64)>;
    fn deps(&self) -> Deps;

    fn lookup(
        &self,
        sym: &SymbolId,
        dir: EdgeDir,
        ctx: &ProviderCtx,
    ) -> Lookup<Vec<(SymbolId, f64)>> {
        let edges = match dir {
            EdgeDir::Out => self.out_edges(sym, ctx),
            EdgeDir::In => self.in_edges(sym, ctx),
        };
        Lookup::Ready(edges)
    }

    fn edge_detail(
        &self,
        _from: &SymbolId,
        _to: &SymbolId,
        _ctx: &ProviderCtx,
    ) -> Option<EdgeDetail> {
        None
    }

    fn prefetch(&self, _sym: &SymbolId, _ctx: &ProviderCtx) {}

    fn poll(&self) -> bool {
        false
    }

    fn is_busy(&self) -> bool {
        false
    }
}

/// Registry of available relation providers.
pub struct RelationRegistry {
    providers: Vec<Box<dyn RelationProvider>>,
}

impl RelationRegistry {
    pub fn empty() -> Self {
        RelationRegistry {
            providers: Vec::new(),
        }
    }

    /// Registry pre-populated with the built-in providers.
    pub fn builtin(tree: Arc<SymbolTree>) -> Self {
        let mut reg = RelationRegistry::empty();
        let worker = CallsWorker::spawn(Arc::clone(&tree));
        reg.register(Box::new(CallsProvider::new(worker)));
        reg.register(Box::new(InheritsProvider::new(Arc::clone(&tree))));
        reg
    }

    pub fn register(&mut self, p: Box<dyn RelationProvider>) {
        self.providers.push(p);
    }

    pub fn get(&self, id: &str) -> Option<&dyn RelationProvider> {
        self.providers.iter().find(|p| p.id() == id).map(|p| &**p)
    }

    pub fn has(&self, name: &str) -> bool {
        self.providers.iter().any(|p| p.id() == name)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.providers.iter().map(|p| p.id())
    }

    pub fn poll(&self) -> bool {
        let mut any = false;
        for p in &self.providers {
            if p.poll() {
                any = true;
            }
        }
        any
    }

    pub fn is_busy(&self) -> bool {
        self.providers.iter().any(|p| p.is_busy())
    }

    pub fn prefetch(&self, id: &str, sym: &SymbolId, ctx: &ProviderCtx) {
        if let Some(p) = self.get(id) {
            p.prefetch(sym, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StaticProvider;
    impl RelationProvider for StaticProvider {
        fn id(&self) -> &str {
            "test"
        }
        fn out_edges(&self, _from: &SymbolId, _ctx: &ProviderCtx) -> Vec<(SymbolId, f64)> {
            vec![]
        }
        fn in_edges(&self, _to: &SymbolId, _ctx: &ProviderCtx) -> Vec<(SymbolId, f64)> {
            vec![]
        }
        fn deps(&self) -> Deps {
            Deps::TREE
        }
    }

    #[test]
    fn registry_get_and_has() {
        let mut reg = RelationRegistry::empty();
        assert!(!reg.has("test"));
        reg.register(Box::new(StaticProvider));
        assert!(reg.has("test"));
        assert!(reg.get("test").is_some());
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn registry_ids() {
        let mut reg = RelationRegistry::empty();
        reg.register(Box::new(StaticProvider));
        let ids: Vec<_> = reg.ids().collect();
        assert_eq!(ids, vec!["test"]);
    }

    #[test]
    fn default_lookup_delegates_to_out_in() {
        let p = StaticProvider;
        let tree = outrider_index::SymbolTree {
            repo_root: std::path::PathBuf::from("."),
            root: outrider_index::SymbolNode {
                id: outrider_index::SymbolId {
                    kind: outrider_index::SymbolKind::Folder,
                    qualified_path: String::new(),
                    ordinal: 0,
                },
                name: String::new(),
                doc: None,
                signature: None,
                measure: 0,
                byte_range: None,
                churn: 0.0,
                churn_count: 0,
                diff_status: None,
                diff_hunks: Vec::new(),
                deleted_lines: Vec::new(),
                visibility: None,
                children: vec![],
            },
        };
        let index = TreeIndex::new(&tree);
        let ctx = ProviderCtx {
            tree: &tree,
            index: &index,
            repo_root: tree.repo_root.as_path(),
        };
        let id = tree.root.id.clone();
        match p.lookup(&id, EdgeDir::Out, &ctx) {
            Lookup::Ready(v) => assert!(v.is_empty()),
            Lookup::Pending => panic!("static provider should never be Pending"),
        }
    }

    #[test]
    fn poll_and_is_busy_defaults() {
        let p = StaticProvider;
        assert!(!p.poll());
        assert!(!p.is_busy());
    }
}
