use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use outrider_index::inheritance::{resolve_inheritance, InheritanceEdge};
use outrider_index::{SymbolId, SymbolTree};

use crate::deps::Deps;

use super::{EdgeDir, RelationProvider};

pub struct InheritsProvider {
    cache: Mutex<Option<InheritsCache>>,
    tree: Arc<SymbolTree>,
}

struct InheritsCache {
    out: HashMap<SymbolId, Vec<(SymbolId, String)>>,
    inc: HashMap<SymbolId, Vec<(SymbolId, String)>>,
}

impl InheritsCache {
    fn build(edges: &[InheritanceEdge]) -> Self {
        let mut out: HashMap<SymbolId, Vec<(SymbolId, String)>> = HashMap::new();
        let mut inc: HashMap<SymbolId, Vec<(SymbolId, String)>> = HashMap::new();
        for e in edges {
            out.entry(e.child.clone())
                .or_default()
                .push((e.parent.clone(), e.parent_name.clone()));
            inc.entry(e.parent.clone())
                .or_default()
                .push((e.child.clone(), e.parent_name.clone()));
        }
        InheritsCache { out, inc }
    }
}

impl InheritsProvider {
    pub fn new(tree: Arc<SymbolTree>) -> Self {
        InheritsProvider {
            cache: Mutex::new(None),
            tree,
        }
    }

    fn ensure_cache(&self) {
        let mut guard = self.cache.lock().unwrap();
        if guard.is_none() {
            let data = resolve_inheritance(&self.tree);
            *guard = Some(InheritsCache::build(&data.edges));
        }
    }

    fn edges_for(&self, sym: &SymbolId, dir: EdgeDir) -> Vec<(SymbolId, f64)> {
        self.ensure_cache();
        let guard = self.cache.lock().unwrap();
        let cache = guard.as_ref().unwrap();
        let map = match dir {
            EdgeDir::Out => &cache.out,
            EdgeDir::In => &cache.inc,
        };
        map.get(sym)
            .map(|v| v.iter().map(|(id, _)| (id.clone(), 1.0)).collect())
            .unwrap_or_default()
    }
}

impl RelationProvider for InheritsProvider {
    fn id(&self) -> &str {
        "inherits"
    }

    fn out_edges(
        &self,
        from: &SymbolId,
        _ctx: &super::ProviderCtx,
    ) -> Vec<(SymbolId, f64)> {
        self.edges_for(from, EdgeDir::Out)
    }

    fn in_edges(
        &self,
        to: &SymbolId,
        _ctx: &super::ProviderCtx,
    ) -> Vec<(SymbolId, f64)> {
        self.edges_for(to, EdgeDir::In)
    }

    fn deps(&self) -> Deps {
        Deps::TREE
    }

    fn edge_detail(
        &self,
        from: &SymbolId,
        to: &SymbolId,
        _ctx: &super::ProviderCtx,
    ) -> Option<super::EdgeDetail> {
        self.ensure_cache();
        let guard = self.cache.lock().unwrap();
        let cache = guard.as_ref()?;
        let edges = cache.out.get(from)?;
        let (_, label) = edges.iter().find(|(id, _)| id == to)?;
        Some(super::EdgeDetail {
            label: format!("extends {label}"),
            site: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolKind, SymbolNode};

    fn mock_tree() -> Arc<SymbolTree> {
        Arc::new(SymbolTree {
            repo_root: std::path::PathBuf::from("."),
            root: SymbolNode {
                id: SymbolId {
                    kind: SymbolKind::Folder,
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
        })
    }

    #[test]
    fn provider_id() {
        let p = InheritsProvider::new(mock_tree());
        assert_eq!(p.id(), "inherits");
    }

    #[test]
    fn empty_tree_no_edges() {
        let tree = mock_tree();
        let p = InheritsProvider::new(tree.clone());
        let id = tree.root.id.clone();
        let ctx = super::super::ProviderCtx {
            tree: &tree,
            index: &outrider_index::TreeIndex::new(&tree),
            repo_root: tree.repo_root.as_path(),
        };
        assert!(p.out_edges(&id, &ctx).is_empty());
        assert!(p.in_edges(&id, &ctx).is_empty());
    }
}
