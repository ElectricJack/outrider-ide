//! Built-in `calls` relation provider.
//! Wraps outrider_index::call_graph::resolve_calls behind a per-symbol cache
//! fed by a background worker thread.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use outrider_index::call_graph::{resolve_calls, CallGraphData};
use outrider_index::types::is_leaf_item;
use outrider_index::{SymbolId, SymbolTree};

use crate::deps::Deps;

use super::{EdgeDetail, EdgeDir, Lookup, ProviderCtx, RelationProvider};

/// Persistent background worker that resolves call graphs one symbol at a time.
///
/// `spawn` runs a single long-lived thread that drains requests off `tx`/`rx`
/// and pushes finished results into the shared `results` buffer. `inline`
/// (test-only) skips the thread entirely and resolves synchronously inside
/// `request`, so results are available immediately without polling.
pub(crate) struct CallsWorker {
    tx: mpsc::Sender<SymbolId>,
    results: Arc<Mutex<Vec<(SymbolId, CallGraphData)>>>,
    /// Set only in `inline` mode: resolves requests synchronously against this
    /// tree instead of dispatching to a background thread.
    inline_tree: Option<Arc<SymbolTree>>,
}

impl CallsWorker {
    /// Spawn one persistent worker thread that resolves requests as they arrive.
    /// The thread exits cleanly once every `CallsWorker` (and thus every
    /// `Sender`) referencing it is dropped.
    pub(crate) fn spawn(tree: Arc<SymbolTree>) -> Self {
        let (tx, rx) = mpsc::channel::<SymbolId>();
        let results: Arc<Mutex<Vec<(SymbolId, CallGraphData)>>> = Arc::new(Mutex::new(Vec::new()));
        let results_thread = Arc::clone(&results);
        std::thread::spawn(move || {
            while let Ok(id) = rx.recv() {
                let data = resolve_calls(&id, &tree);
                if let Ok(mut guard) = results_thread.lock() {
                    guard.push((id, data));
                }
            }
        });
        CallsWorker {
            tx,
            results,
            inline_tree: None,
        }
    }

    /// Test-only: resolve synchronously with no background thread, so results
    /// are available as soon as `request` returns.
    #[allow(dead_code)]
    pub(crate) fn inline(tree: Arc<SymbolTree>) -> Self {
        // Channel exists only to satisfy the field type; nothing is ever sent
        // through it in inline mode.
        let (tx, _rx) = mpsc::channel::<SymbolId>();
        CallsWorker {
            tx,
            results: Arc::new(Mutex::new(Vec::new())),
            inline_tree: Some(tree),
        }
    }

    /// Ask the worker to resolve `id`'s call graph. Fire-and-forget; results
    /// show up in a later `drain`.
    pub(crate) fn request(&self, id: SymbolId) {
        if let Some(tree) = &self.inline_tree {
            let data = resolve_calls(&id, tree);
            if let Ok(mut guard) = self.results.lock() {
                guard.push((id, data));
            }
        } else {
            let _ = self.tx.send(id);
        }
    }

    /// Take all results finished since the last drain.
    pub(crate) fn drain(&self) -> Vec<(SymbolId, CallGraphData)> {
        match self.results.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(_) => Vec::new(),
        }
    }
}

/// Relation provider backing the `calls` relation: caller/callee edges
/// resolved on demand and cached per symbol.
pub struct CallsProvider {
    cache: Mutex<HashMap<SymbolId, CallGraphData>>,
    queued: Mutex<HashSet<SymbolId>>,
    worker: CallsWorker,
}

impl CallsProvider {
    pub(crate) fn new(worker: CallsWorker) -> Self {
        CallsProvider {
            cache: Mutex::new(HashMap::new()),
            queued: Mutex::new(HashSet::new()),
            worker,
        }
    }

    fn is_fn_leaf(sym: &SymbolId, ctx: &ProviderCtx) -> bool {
        ctx.index.node(sym).map(is_leaf_item).unwrap_or(false)
    }

    /// Request resolution for `sym` unless it's already in flight.
    fn request_if_needed(&self, sym: &SymbolId) {
        let mut queued = self.queued.lock().unwrap();
        if queued.insert(sym.clone()) {
            self.worker.request(sym.clone());
        }
    }

    fn edges_from_cache(data: &CallGraphData, dir: EdgeDir) -> Vec<(SymbolId, f64)> {
        let edges = match dir {
            EdgeDir::Out => &data.callees,
            EdgeDir::In => &data.callers,
        };
        edges.iter().map(|e| (e.target.clone(), 1.0)).collect()
    }
}

impl RelationProvider for CallsProvider {
    fn id(&self) -> &str {
        "calls"
    }

    fn out_edges(&self, from: &SymbolId, _ctx: &ProviderCtx) -> Vec<(SymbolId, f64)> {
        let cache = self.cache.lock().unwrap();
        cache
            .get(from)
            .map(|data| Self::edges_from_cache(data, EdgeDir::Out))
            .unwrap_or_default()
    }

    fn in_edges(&self, to: &SymbolId, _ctx: &ProviderCtx) -> Vec<(SymbolId, f64)> {
        let cache = self.cache.lock().unwrap();
        cache
            .get(to)
            .map(|data| Self::edges_from_cache(data, EdgeDir::In))
            .unwrap_or_default()
    }

    fn deps(&self) -> Deps {
        Deps::TREE | Deps::RELATIONS
    }

    fn lookup(
        &self,
        sym: &SymbolId,
        dir: EdgeDir,
        ctx: &ProviderCtx,
    ) -> Lookup<Vec<(SymbolId, f64)>> {
        {
            let cache = self.cache.lock().unwrap();
            if let Some(data) = cache.get(sym) {
                return Lookup::Ready(Self::edges_from_cache(data, dir));
            }
        }

        if !Self::is_fn_leaf(sym, ctx) {
            return Lookup::Ready(Vec::new());
        }

        self.request_if_needed(sym);
        Lookup::Pending
    }

    fn edge_detail(&self, from: &SymbolId, to: &SymbolId, _ctx: &ProviderCtx) -> Option<EdgeDetail> {
        let cache = self.cache.lock().unwrap();
        let data = cache.get(from)?;
        data.callees
            .iter()
            .chain(data.callers.iter())
            .find(|e| e.target == *to)
            .map(|e| EdgeDetail {
                label: e.raw_name.clone(),
                site: e.call_site.clone(),
            })
    }

    fn prefetch(&self, sym: &SymbolId, ctx: &ProviderCtx) {
        if !Self::is_fn_leaf(sym, ctx) {
            return;
        }
        if self.cache.lock().unwrap().contains_key(sym) {
            return;
        }
        self.request_if_needed(sym);
    }

    fn poll(&self) -> bool {
        let drained = self.worker.drain();
        if drained.is_empty() {
            return false;
        }
        let mut cache = self.cache.lock().unwrap();
        let mut queued = self.queued.lock().unwrap();
        for (id, data) in drained {
            queued.remove(&id);
            cache.insert(id, data);
        }
        true
    }

    fn is_busy(&self) -> bool {
        !self.queued.lock().unwrap().is_empty()
    }
}
