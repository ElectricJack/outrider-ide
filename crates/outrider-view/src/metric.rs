//! Metric providers and the metric registry.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

use crate::deps::Deps;

/// A single metric provider.
pub trait MetricProvider: Send + Sync {
    fn id(&self) -> &str;
    fn basis(&self) -> &str {
        "builtin"
    }
    fn unit(&self) -> &str;
    fn value(&self, node: &SymbolNode) -> Option<f64>;
    /// If this metric has a pre-computed percentile on the node, return it.
    /// Churn returns `node.churn` so the golden test is exact.
    fn native_percentile(&self, _node: &SymbolNode) -> Option<f32> {
        None
    }
    fn deps(&self) -> Deps {
        Deps::NONE
    }
}

struct ChurnProvider;
impl MetricProvider for ChurnProvider {
    fn id(&self) -> &str {
        "churn"
    }
    fn unit(&self) -> &str {
        "commits"
    }
    fn value(&self, node: &SymbolNode) -> Option<f64> {
        Some(node.churn_count as f64)
    }
    fn native_percentile(&self, node: &SymbolNode) -> Option<f32> {
        Some(node.churn)
    }
}

struct ChurnCountProvider;
impl MetricProvider for ChurnCountProvider {
    fn id(&self) -> &str {
        "churnCount"
    }
    fn unit(&self) -> &str {
        "commits"
    }
    fn value(&self, node: &SymbolNode) -> Option<f64> {
        Some(node.churn_count as f64)
    }
}

struct MeasureProvider;
impl MetricProvider for MeasureProvider {
    fn id(&self) -> &str {
        "measure"
    }
    fn unit(&self) -> &str {
        "lines"
    }
    fn value(&self, node: &SymbolNode) -> Option<f64> {
        Some(node.measure as f64)
    }
}

struct EntitiesProvider;
impl MetricProvider for EntitiesProvider {
    fn id(&self) -> &str {
        "entities"
    }
    fn unit(&self) -> &str {
        "items"
    }
    fn value(&self, node: &SymbolNode) -> Option<f64> {
        Some(count_entities(node) as f64)
    }
}

fn count_entities(node: &SymbolNode) -> usize {
    if node.children.is_empty() {
        1
    } else {
        node.children.iter().map(count_entities).sum()
    }
}

// ── Imported metric provider ──

struct ImportedProvider {
    id: String,
    basis: String,
    unit: String,
    values: BTreeMap<SymbolId, f64>,
}

impl MetricProvider for ImportedProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn basis(&self) -> &str {
        &self.basis
    }
    fn unit(&self) -> &str {
        &self.unit
    }
    fn value(&self, node: &SymbolNode) -> Option<f64> {
        self.values.get(&node.id).copied()
    }
    fn native_percentile(&self, _node: &SymbolNode) -> Option<f32> {
        None
    }
    fn deps(&self) -> Deps {
        Deps::METRICS
    }
}

// ── AST metric provider ──

struct AstMetricProvider {
    id: String,
    extract: fn(&outrider_index::ast_metrics::NodeMetrics) -> u32,
    cache: Arc<BTreeMap<String, outrider_index::ast_metrics::NodeMetrics>>,
}

impl MetricProvider for AstMetricProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn unit(&self) -> &str {
        ""
    }
    fn value(&self, node: &SymbolNode) -> Option<f64> {
        self.cache
            .get(&node.id.qualified_path)
            .map(|m| (self.extract)(m) as f64)
    }
}

// ── LineTable ──

/// Byte-offset table for resolving `file:line` import keys.
pub struct LineTable {
    starts: Vec<usize>,
    pub len: usize,
}

impl LineTable {
    pub fn load(repo_root: &Path, rel: &str) -> std::io::Result<LineTable> {
        let path = repo_root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let data = std::fs::read(&path)?;
        let mut starts = vec![0usize];
        for (i, &b) in data.iter().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Ok(LineTable {
            len: data.len(),
            starts,
        })
    }

    pub fn byte_of_line(&self, line1: usize) -> Option<usize> {
        if line1 == 0 || line1 > self.starts.len() {
            return None;
        }
        Some(self.starts[line1 - 1])
    }
}

// ── ImportReport ──

pub struct ImportReport {
    pub resolved: usize,
    pub unresolved: Vec<String>,
    pub warnings: Vec<String>,
}

// ── MetricReadout ──

pub struct MetricReadout {
    pub metric: String,
    pub raw: f64,
    pub unit: String,
    pub percentile: f32,
    pub basis: String,
}

impl MetricReadout {
    pub fn text(&self) -> String {
        let unit_str = if self.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", self.unit)
        };
        format!(
            "{} {:.0}{} · p{:.0} · {}",
            self.metric,
            self.raw,
            unit_str,
            self.percentile * 100.0,
            self.basis
        )
    }
}

// ── Resolution helpers ──

fn kind_label(kind: &SymbolKind) -> &str {
    match kind {
        SymbolKind::Folder => "folder",
        SymbolKind::File => "file",
        SymbolKind::Chunk => "chunk",
        SymbolKind::Item { label } => label.as_str(),
    }
}

fn find_node_by_id<'a>(node: &'a SymbolNode, target: &SymbolId) -> Option<&'a SymbolNode> {
    if &node.id == target {
        return Some(node);
    }
    for child in &node.children {
        if let Some(n) = find_node_by_id(child, target) {
            return Some(n);
        }
    }
    None
}

fn collect_by_path<'a>(node: &'a SymbolNode, path: &str) -> Vec<&'a SymbolNode> {
    let mut result = Vec::new();
    if node.id.qualified_path == path {
        result.push(node);
    }
    for child in &node.children {
        result.extend(collect_by_path(child, path));
    }
    result
}

fn deepest_at<'a>(node: &'a SymbolNode, byte: usize) -> Option<&'a SymbolNode> {
    for child in &node.children {
        if let Some(range) = &child.byte_range {
            if range.contains(&byte) {
                return Some(deepest_at(child, byte).unwrap_or(child));
            }
        }
    }
    None
}

/// Registry of all available metrics (built-in + imported).
pub struct MetricRegistry {
    providers: Vec<Box<dyn MetricProvider>>,
}

impl MetricRegistry {
    pub fn builtin() -> Self {
        MetricRegistry {
            providers: vec![
                Box::new(ChurnProvider),
                Box::new(ChurnCountProvider),
                Box::new(MeasureProvider),
                Box::new(EntitiesProvider),
            ],
        }
    }

    pub fn get(&self, name: &str) -> Option<&dyn MetricProvider> {
        self.providers.iter().find(|p| p.id() == name).map(|p| p.as_ref())
    }

    pub fn has(&self, name: &str) -> bool {
        self.providers.iter().any(|p| p.id() == name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.id()).collect()
    }

    pub fn register_imported(
        &mut self,
        name: &str,
        m: &crate::spec::ImportedMetric,
        tree: &SymbolTree,
        repo_root: &Path,
    ) -> ImportReport {
        let mut values = BTreeMap::new();
        let mut unresolved = Vec::new();
        let mut warnings = Vec::new();
        let mut resolved = 0usize;

        for (key, &val) in &m.values {
            // 1. Wire SymbolId
            if let Ok(id) = crate::symbol_id::parse_wire(key) {
                if find_node_by_id(&tree.root, &id).is_some() {
                    values.insert(id, val);
                    resolved += 1;
                    continue;
                }
            }

            // 2. Bare qualified path
            let matches = collect_by_path(&tree.root, key);
            if matches.len() == 1 {
                values.insert(matches[0].id.clone(), val);
                resolved += 1;
                continue;
            } else if matches.len() > 1 {
                let mut sorted = matches;
                sorted.sort_by_key(|n| kind_label(&n.id.kind).to_string());
                let chosen = sorted[0];
                warnings.push(format!(
                    "ambiguous path {:?}: {} matches, picked {}",
                    key,
                    sorted.len(),
                    kind_label(&chosen.id.kind)
                ));
                values.insert(chosen.id.clone(), val);
                resolved += 1;
                continue;
            }

            // 3. file:line — split at the last ':'
            if let Some(colon_pos) = key.rfind(':') {
                let prefix = &key[..colon_pos];
                let suffix = &key[colon_pos + 1..];
                if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(line) = suffix.parse::<usize>() {
                        let file_matches = collect_by_path(&tree.root, prefix);
                        let file_node =
                            file_matches.iter().find(|n| n.id.kind == SymbolKind::File);
                        if let Some(file_node) = file_node {
                            if let Ok(lt) = LineTable::load(repo_root, prefix) {
                                if let Some(byte) = lt.byte_of_line(line) {
                                    if let Some(deepest) = deepest_at(file_node, byte) {
                                        values.insert(deepest.id.clone(), val);
                                        resolved += 1;
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            unresolved.push(key.clone());
        }

        let provider = ImportedProvider {
            id: name.to_string(),
            basis: m.basis.clone().unwrap_or_default(),
            unit: m.unit.clone().unwrap_or_default(),
            values,
        };
        self.providers.push(Box::new(provider));

        ImportReport {
            resolved,
            unresolved,
            warnings,
        }
    }

    pub fn readouts(&self, node: &SymbolNode) -> Vec<MetricReadout> {
        self.providers
            .iter()
            .filter_map(|p| {
                let raw = p.value(node)?;
                let percentile = p.native_percentile(node).unwrap_or(0.0);
                Some(MetricReadout {
                    metric: p.id().to_string(),
                    raw,
                    unit: p.unit().to_string(),
                    percentile,
                    basis: p.basis().to_string(),
                })
            })
            .collect()
    }

    /// Register AST-derived metrics (complexity, maxNesting, params) from a
    /// pre-computed cache keyed by qualified path.
    pub fn register_ast_metrics(
        &mut self,
        cache: BTreeMap<String, outrider_index::ast_metrics::NodeMetrics>,
    ) {
        let cache = Arc::new(cache);
        self.providers.push(Box::new(AstMetricProvider {
            id: "complexity".to_string(),
            extract: |m| m.complexity,
            cache: cache.clone(),
        }));
        self.providers.push(Box::new(AstMetricProvider {
            id: "maxNesting".to_string(),
            extract: |m| m.max_nesting,
            cache: cache.clone(),
        }));
        self.providers.push(Box::new(AstMetricProvider {
            id: "params".to_string(),
            extract: |m| m.params,
            cache: cache.clone(),
        }));
    }

    /// Pre-compute fan-in and fan-out edge counts for a given relation and
    /// register them as imported-style metric providers.
    pub fn register_fan_metrics(
        &mut self,
        relation_id: &str,
        relations: &crate::relation::RelationRegistry,
        tree: &SymbolTree,
    ) {
        use outrider_index::tree_index::TreeIndex;

        let provider = match relations.get(relation_id) {
            Some(p) => p,
            None => return,
        };
        let index = TreeIndex::new(tree);
        let pctx = crate::relation::ProviderCtx {
            tree,
            index: &index,
            repo_root: tree.repo_root.as_path(),
        };

        let mut fan_out: BTreeMap<SymbolId, f64> = BTreeMap::new();
        let mut fan_in: BTreeMap<SymbolId, f64> = BTreeMap::new();

        fn walk(node: &SymbolNode, f: &mut impl FnMut(&SymbolNode)) {
            f(node);
            for c in &node.children {
                walk(c, f);
            }
        }

        walk(&tree.root, &mut |node| {
            let out = provider.out_edges(&node.id, &pctx);
            if !out.is_empty() {
                fan_out.insert(node.id.clone(), out.len() as f64);
            }
            let in_e = provider.in_edges(&node.id, &pctx);
            if !in_e.is_empty() {
                fan_in.insert(node.id.clone(), in_e.len() as f64);
            }
        });

        self.providers.push(Box::new(ImportedProvider {
            id: format!("fanOut_{relation_id}"),
            basis: format!("{relation_id} out-degree"),
            unit: "edges".to_string(),
            values: fan_out,
        }));
        self.providers.push(Box::new(ImportedProvider {
            id: format!("fanIn_{relation_id}"),
            basis: format!("{relation_id} in-degree"),
            unit: "edges".to_string(),
            values: fan_in,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_id(kind: SymbolKind, path: &str) -> SymbolId {
        SymbolId {
            kind,
            qualified_path: path.to_string(),
            ordinal: 0,
        }
    }

    fn make_node(id: SymbolId, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id,
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

    fn sample_tree() -> SymbolTree {
        let foo = SymbolNode {
            byte_range: Some(0..20),
            ..make_node(
                make_id(
                    SymbolKind::Item {
                        label: "fn".to_string(),
                    },
                    "src/a.rs::foo",
                ),
                "foo",
                vec![],
            )
        };
        let file_a = make_node(make_id(SymbolKind::File, "src/a.rs"), "a.rs", vec![foo]);
        let root = make_node(make_id(SymbolKind::Folder, ""), "root", vec![file_a]);
        SymbolTree {
            root,
            repo_root: PathBuf::from("."),
        }
    }

    #[test]
    fn builtin_registry_has_expected_metrics() {
        let reg = MetricRegistry::builtin();
        assert!(reg.has("churn"));
        assert!(reg.has("churnCount"));
        assert!(reg.has("measure"));
        assert!(reg.has("entities"));
        assert!(!reg.has("bogus"));
    }

    #[test]
    fn register_imported_wire_key() {
        let tree = sample_tree();
        let mut reg = MetricRegistry::builtin();
        let mut values = BTreeMap::new();
        values.insert("fn:src/a.rs::foo".to_string(), 42.0);
        let im = crate::spec::ImportedMetric {
            basis: Some("test".to_string()),
            unit: Some("bugs".to_string()),
            values,
        };
        let report = reg.register_imported("coverage", &im, &tree, &PathBuf::from("."));
        assert_eq!(report.resolved, 1);
        assert!(report.unresolved.is_empty());
        assert!(reg.has("coverage"));
    }

    #[test]
    fn register_imported_bare_path() {
        let tree = sample_tree();
        let mut reg = MetricRegistry::builtin();
        let mut values = BTreeMap::new();
        values.insert("src/a.rs::foo".to_string(), 7.5);
        let im = crate::spec::ImportedMetric {
            basis: None,
            unit: None,
            values,
        };
        let report = reg.register_imported("complexity", &im, &tree, &PathBuf::from("."));
        assert_eq!(report.resolved, 1);
        assert!(report.unresolved.is_empty());
    }

    #[test]
    fn register_imported_unresolved_key() {
        let tree = sample_tree();
        let mut reg = MetricRegistry::builtin();
        let mut values = BTreeMap::new();
        values.insert("nonexistent::symbol".to_string(), 1.0);
        let im = crate::spec::ImportedMetric {
            basis: None,
            unit: None,
            values,
        };
        let report = reg.register_imported("bad", &im, &tree, &PathBuf::from("."));
        assert_eq!(report.resolved, 0);
        assert_eq!(report.unresolved, vec!["nonexistent::symbol"]);
    }

    #[test]
    fn line_table_byte_of_line() {
        // Simulate "ab\ncd\nef" — line 1 starts at 0, line 2 at 3, line 3 at 6
        let lt = LineTable {
            starts: vec![0, 3, 6],
            len: 8,
        };
        assert_eq!(lt.byte_of_line(1), Some(0));
        assert_eq!(lt.byte_of_line(2), Some(3));
        assert_eq!(lt.byte_of_line(3), Some(6));
        assert_eq!(lt.byte_of_line(0), None);
        assert_eq!(lt.byte_of_line(4), None);
    }

    #[test]
    fn readout_text_format() {
        let r = MetricReadout {
            metric: "churn".to_string(),
            raw: 42.0,
            unit: "commits".to_string(),
            percentile: 0.95,
            basis: "builtin".to_string(),
        };
        assert_eq!(r.text(), "churn 42 commits · p95 · builtin");

        let r2 = MetricReadout {
            metric: "score".to_string(),
            raw: 7.0,
            unit: String::new(),
            percentile: 0.5,
            basis: "imported".to_string(),
        };
        assert_eq!(r2.text(), "score 7 · p50 · imported");
    }

    // ── AST metric tests ──

    #[test]
    fn ast_metric_complexity() {
        let mut cache = BTreeMap::new();
        cache.insert(
            "src/a.rs::foo".to_string(),
            outrider_index::ast_metrics::NodeMetrics {
                complexity: 5,
                max_nesting: 2,
                params: 3,
            },
        );
        let mut reg = MetricRegistry::builtin();
        reg.register_ast_metrics(cache);

        let tree = sample_tree();
        // Find the foo node
        let foo = &tree.root.children[0].children[0];
        assert_eq!(foo.id.qualified_path, "src/a.rs::foo");

        let complexity = reg.get("complexity").unwrap();
        assert_eq!(complexity.value(foo), Some(5.0));

        let nesting = reg.get("maxNesting").unwrap();
        assert_eq!(nesting.value(foo), Some(2.0));

        let params = reg.get("params").unwrap();
        assert_eq!(params.value(foo), Some(3.0));
    }

    #[test]
    fn ast_metric_missing_node() {
        let cache = BTreeMap::new(); // empty cache
        let mut reg = MetricRegistry::builtin();
        reg.register_ast_metrics(cache);

        let tree = sample_tree();
        let foo = &tree.root.children[0].children[0];

        let complexity = reg.get("complexity").unwrap();
        assert_eq!(complexity.value(foo), None);
    }

    #[test]
    fn fan_metrics_empty_relations() {
        let mut reg = MetricRegistry::builtin();
        let relations = crate::relation::RelationRegistry::empty();
        let tree = sample_tree();
        let before = reg.names().len();
        reg.register_fan_metrics("nonexistent", &relations, &tree);
        // No providers added when the relation doesn't exist
        assert_eq!(reg.names().len(), before);
    }
}
