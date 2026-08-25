//! Fill layer resolution: metric -> scaled value -> theme color.

use std::collections::HashMap;

use outrider_index::{SymbolId, SymbolNode};

use crate::deps::Deps;
use crate::metric::MetricRegistry;
use crate::set::ResolvedSet;
use crate::spec::{FillChannel, FillSpec, Scale};

/// A resolved fill layer: per-symbol scaled values.
#[derive(Debug, Clone)]
pub struct ResolvedFill {
    pub metric: String,
    pub channel: FillChannel,
    pub scale: Scale,
    /// Scaled value 0.0..1.0 for each symbol in the domain.
    pub values: HashMap<SymbolId, f32>,
    pub deps: Deps,
}

/// Domain statistics for percentile computation.
struct DomainStats {
    sorted: Vec<f64>,
}

impl DomainStats {
    fn new(values: &[f64]) -> Self {
        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        DomainStats { sorted }
    }

    fn percentile(&self, value: f64) -> f32 {
        if self.sorted.len() <= 1 {
            return 0.0;
        }
        let below = self.sorted.iter().filter(|&&v| v < value).count();
        below as f32 / (self.sorted.len() - 1) as f32
    }
}

fn scale_value(raw: f64, scale: &Scale, stats: &DomainStats) -> f32 {
    match scale {
        Scale::Percentile => stats.percentile(raw),
        Scale::Linear => {
            if stats.sorted.is_empty() {
                return 0.0;
            }
            let min = stats.sorted[0];
            let max = stats.sorted[stats.sorted.len() - 1];
            if (max - min).abs() < f64::EPSILON {
                0.0
            } else {
                ((raw - min) / (max - min)).clamp(0.0, 1.0) as f32
            }
        }
        Scale::Log => {
            if stats.sorted.is_empty() {
                return 0.0;
            }
            let min = stats.sorted[0].max(1.0);
            let max = stats.sorted[stats.sorted.len() - 1].max(1.0);
            if (max - min).abs() < f64::EPSILON {
                0.0
            } else {
                let log_min = min.ln();
                let log_max = max.ln();
                if (log_max - log_min).abs() < f64::EPSILON {
                    0.0
                } else {
                    ((raw.max(1.0).ln() - log_min) / (log_max - log_min)).clamp(0.0, 1.0) as f32
                }
            }
        }
        Scale::Threshold(thresholds) => {
            if thresholds.is_empty() {
                return 0.0;
            }
            let class = thresholds.iter().filter(|&&t| raw >= t).count();
            class as f32 / (thresholds.len()) as f32
        }
        Scale::Categorical => raw as f32,
    }
}

/// Resolve a fill layer against the tree.
pub fn resolve_fill(
    spec: &FillSpec,
    metrics: &MetricRegistry,
    tree: &outrider_index::SymbolTree,
    domain: Option<&ResolvedSet>,
    warnings: &mut Vec<String>,
) -> Option<ResolvedFill> {
    let provider = match metrics.get(&spec.metric) {
        Some(p) => p,
        None => {
            warnings.push(format!("unknown metric '{}'", spec.metric));
            return None;
        }
    };

    let mut raw_values: Vec<(SymbolId, f64)> = Vec::new();

    fn collect(
        node: &SymbolNode,
        provider: &dyn crate::metric::MetricProvider,
        domain: Option<&ResolvedSet>,
        out: &mut Vec<(SymbolId, f64)>,
    ) {
        let in_domain = domain.map_or(true, |d| d.contains(&node.id));
        if in_domain {
            if let Some(v) = provider.value(node) {
                out.push((node.id.clone(), v));
            }
        }
        for c in &node.children {
            collect(c, provider, domain, out);
        }
    }
    collect(&tree.root, provider, domain, &mut raw_values);

    // For churn with percentile scale and no explicit domain, use the native percentile
    // so the result exactly matches the pre-existing (pre-view-layer) churn stripes.
    let use_native = spec.metric == "churn" && matches!(spec.scale, Scale::Percentile) && domain.is_none();

    let mut values = HashMap::new();
    if use_native {
        fn collect_native(
            node: &SymbolNode,
            provider: &dyn crate::metric::MetricProvider,
            values: &mut HashMap<SymbolId, f32>,
        ) {
            if let Some(p) = provider.native_percentile(node) {
                values.insert(node.id.clone(), p);
            }
            for c in &node.children {
                collect_native(c, provider, values);
            }
        }
        collect_native(&tree.root, provider, &mut values);
    } else {
        let raw_vals: Vec<f64> = raw_values.iter().map(|(_, v)| *v).collect();
        let stats = DomainStats::new(&raw_vals);
        for (id, raw) in &raw_values {
            values.insert(id.clone(), scale_value(*raw, &spec.scale, &stats));
        }
    }

    Some(ResolvedFill {
        metric: spec.metric.clone(),
        channel: spec.channel,
        scale: spec.scale.clone(),
        values,
        deps: Deps::TREE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolKind, SymbolTree};

    fn leaf(name: &str, churn: f32, churn_count: u64) -> SymbolNode {
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
            churn,
            churn_count,
            visibility: None,
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
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
            children: vec![leaf("a.rs", 0.2, 2), leaf("b.rs", 0.8, 8)],
        };
        SymbolTree {
            root,
            repo_root: std::path::PathBuf::from("."),
        }
    }

    #[test]
    fn churn_percentile_uses_native_value() {
        let tree = sample_tree();
        let metrics = MetricRegistry::builtin();
        let mut warnings = Vec::new();
        let spec = FillSpec {
            metric: "churn".to_string(),
            channel: FillChannel::Stripe,
            scale: Scale::Percentile,
            domain: None,
            ramp: None,
        };
        let resolved = resolve_fill(&spec, &metrics, &tree, None, &mut warnings).unwrap();
        assert!(warnings.is_empty());
        let a_id = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "a.rs".to_string(),
            ordinal: 0,
        };
        assert_eq!(resolved.values.get(&a_id).copied(), Some(0.2));
    }

    #[test]
    fn unknown_metric_returns_none_with_warning() {
        let tree = sample_tree();
        let metrics = MetricRegistry::builtin();
        let mut warnings = Vec::new();
        let spec = FillSpec {
            metric: "bogus".to_string(),
            ..Default::default()
        };
        let resolved = resolve_fill(&spec, &metrics, &tree, None, &mut warnings);
        assert!(resolved.is_none());
        assert!(!warnings.is_empty());
    }

    #[test]
    fn opacity_channel_produces_resolved_fill() {
        let tree = sample_tree();
        let metrics = MetricRegistry::builtin();
        let mut warnings = Vec::new();
        let spec = FillSpec {
            metric: "churn".to_string(),
            channel: FillChannel::Opacity,
            scale: Scale::Percentile,
            domain: None,
            ramp: None,
        };
        let resolved = resolve_fill(&spec, &metrics, &tree, None, &mut warnings).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(resolved.channel, FillChannel::Opacity);
        // Should have values for both leaves
        let a_id = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "a.rs".to_string(),
            ordinal: 0,
        };
        let b_id = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "b.rs".to_string(),
            ordinal: 0,
        };
        assert!(resolved.values.contains_key(&a_id));
        assert!(resolved.values.contains_key(&b_id));
    }

    #[test]
    fn log_scale_normalizes_correctly() {
        let stats = DomainStats::new(&[1.0, 10.0, 100.0]);

        // Minimum value should map to 0.0
        let at_min = scale_value(1.0, &Scale::Log, &stats);
        assert!((at_min - 0.0).abs() < 0.001, "min should be ~0.0, got {at_min}");

        // Maximum value should map to 1.0
        let at_max = scale_value(100.0, &Scale::Log, &stats);
        assert!((at_max - 1.0).abs() < 0.001, "max should be ~1.0, got {at_max}");

        // Middle value (10.0) on log scale: ln(10)/ln(100) = 1/2
        let at_mid = scale_value(10.0, &Scale::Log, &stats);
        assert!((at_mid - 0.5).abs() < 0.01, "mid should be ~0.5, got {at_mid}");

        // Values below 1.0 are clamped to 1.0, so they map to min
        let below = scale_value(0.5, &Scale::Log, &stats);
        assert!((below - 0.0).abs() < 0.001, "below-min should be ~0.0, got {below}");
    }

    #[test]
    fn log_scale_single_value_returns_zero() {
        let stats = DomainStats::new(&[5.0]);
        let v = scale_value(5.0, &Scale::Log, &stats);
        assert_eq!(v, 0.0, "single-value domain should return 0.0");
    }

    #[test]
    fn log_scale_empty_domain_returns_zero() {
        let stats = DomainStats::new(&[]);
        let v = scale_value(42.0, &Scale::Log, &stats);
        assert_eq!(v, 0.0, "empty domain should return 0.0");
    }
}
