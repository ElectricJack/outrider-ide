//! Mask layer resolution: attention control (spotlight / dim).
//!
//! A mask dims everything except a set (spotlight, `dimExcept`) or dims a
//! set (`dim`). Resolution produces a per-symbol `light` value in `0..=1`
//! that the renderer multiplies into fill/border/stripe/text/ring alpha.
//! See `docs/view-primitives/04-mask.md` for the full algorithm writeup.

use std::collections::{BTreeMap, HashMap, HashSet};

use outrider_index::{SymbolId, SymbolNode, SymbolTree};

use crate::deps::Deps;
use crate::set::ResolvedSet;
use crate::spec::{MaskSpec, SetRef};

/// The resolved result of one or more (AND-combined) mask layers.
#[derive(Debug, Clone, Default)]
pub struct MaskTable {
    /// Per-symbol light value in `0..=1`. Only nodes with `light < 1.0` are
    /// present; an absent node is fully lit (`1.0`).
    light: HashMap<SymbolId, f32>,
    pub deps: Deps,
    /// Whether any mask layer is present (even if it dims nothing, e.g.
    /// `strength: 0`).
    pub active: bool,
}

impl MaskTable {
    /// The light value for a symbol: `1.0` (fully lit) unless dimmed.
    #[inline]
    pub fn light(&self, id: &SymbolId) -> f32 {
        self.light.get(id).copied().unwrap_or(1.0)
    }

    /// True when this table dims nothing at all.
    pub fn is_noop(&self) -> bool {
        self.light.is_empty()
    }

    /// Number of symbols with a light value below 1.0.
    pub fn dimmed_count(&self) -> usize {
        self.light.len()
    }

    /// Combine two mask tables by ANDing their light values
    /// (`final_light = light1 * light2`). A node absent from both stays lit.
    pub fn and(&self, other: &MaskTable) -> MaskTable {
        let mut light: HashMap<SymbolId, f32> = self.light.clone();
        for (id, &v) in &other.light {
            let entry = light.entry(id.clone()).or_insert(1.0);
            *entry *= v;
        }
        // A product landing back at (or above, from fp error) 1.0 should not
        // be treated as "dimmed".
        light.retain(|_, v| *v < 1.0);
        MaskTable {
            light,
            deps: self.deps.union(other.deps),
            active: self.active || other.active,
        }
    }
}

/// Recursive membership + factor + container-inheritance walk (§4.2).
///
/// Returns the maximum direct factor `f` seen anywhere in `node`'s subtree
/// (including `node` itself), which the caller uses as `desc_max` for its
/// own inheritance step.
fn walk(
    node: &SymbolNode,
    inherited: bool,
    set: &HashSet<SymbolId>,
    dim_except: bool,
    strength: f32,
    out: &mut HashMap<SymbolId, f32>,
) -> f32 {
    let member = inherited || set.contains(&node.id);
    let f = if member == dim_except {
        1.0
    } else {
        1.0 - strength
    };

    let mut desc_max = 0.0f32;
    for c in &node.children {
        desc_max = desc_max.max(walk(c, member, set, dim_except, strength, out));
    }

    let g = if node.children.is_empty() {
        f
    } else {
        f.max(0.5 * desc_max)
    };

    if g < 1.0 {
        out.insert(node.id.clone(), g);
    }

    f.max(desc_max)
}

/// Resolve a single `mask` layer against the tree, returning its `MaskTable`.
///
/// Exactly one of `spec.dim_except` / `spec.dim` is expected to be set
/// (validated upstream); if neither is set the mask is a no-op but still
/// marked `active` since a mask layer was present.
pub fn resolve_mask(
    spec: &MaskSpec,
    named_sets: &BTreeMap<String, ResolvedSet>,
    tree: &SymbolTree,
    warnings: &mut Vec<String>,
) -> MaskTable {
    let (set_ref, dim_except) = match (&spec.dim_except, &spec.dim) {
        (Some(r), None) => (r, true),
        (None, Some(r)) => (r, false),
        (Some(r), Some(_)) => {
            warnings.push("mask: both dimExcept and dim specified; using dimExcept".to_string());
            (r, true)
        }
        (None, None) => {
            warnings.push("mask: neither dimExcept nor dim specified; mask is a no-op".to_string());
            return MaskTable {
                active: true,
                ..Default::default()
            };
        }
    };

    let resolved_set = match set_ref {
        SetRef::Name(name) => match named_sets.get(name) {
            Some(s) => s,
            None => {
                warnings.push(format!("mask: unknown set '{name}'"));
                return MaskTable {
                    active: true,
                    ..Default::default()
                };
            }
        },
        SetRef::Inline(_) => {
            warnings.push("mask: inline set expressions are not resolved here".to_string());
            return MaskTable {
                active: true,
                ..Default::default()
            };
        }
    };

    let strength = if spec.strength.is_finite() {
        spec.strength.clamp(0.0, 1.0)
    } else {
        warnings.push(format!("mask: invalid strength {}, clamping to 0", spec.strength));
        0.0
    };

    let mut light = HashMap::new();
    walk(&tree.root, false, &resolved_set.ids, dim_except, strength, &mut light);

    MaskTable {
        light,
        deps: resolved_set.deps,
        active: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::SymbolKind;

    fn item(qualified_path: &str, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Item {
                    label: "fn".to_string(),
                },
                qualified_path: qualified_path.to_string(),
                ordinal: 0,
            },
            name: name.to_string(),
            byte_range: Some(0..1),
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    fn file(path: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind: SymbolKind::File,
                qualified_path: path.to_string(),
                ordinal: 0,
            },
            name: path.to_string(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1,
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    /// root (folder)
    ///  |- a.rs (file)
    ///  |   `- foo (item: fn)
    ///  `- b.rs (file)
    fn sample_tree() -> (SymbolTree, SymbolId, SymbolId, SymbolId, SymbolId) {
        let foo = item("a.rs::foo", "foo", vec![]);
        let foo_id = foo.id.clone();
        let a = file("a.rs", vec![foo]);
        let a_id = a.id.clone();
        let b = file("b.rs", vec![]);
        let b_id = b.id.clone();
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
            children: vec![a, b],
        };
        let root_id = root.id.clone();
        (
            SymbolTree {
                root,
                repo_root: std::path::PathBuf::from("."),
            },
            root_id,
            a_id,
            foo_id,
            b_id,
        )
    }

    fn set_of(ids: impl IntoIterator<Item = SymbolId>) -> ResolvedSet {
        let mut set = ResolvedSet::default();
        set.ids.extend(ids);
        set
    }

    #[test]
    fn spotlight_lights_target_and_dims_the_rest() {
        let (tree, _root, a_id, foo_id, b_id) = sample_tree();
        let mut named = BTreeMap::new();
        named.insert("target".to_string(), set_of([a_id.clone()]));
        let spec = MaskSpec {
            dim_except: Some(SetRef::Name("target".to_string())),
            dim: None,
            strength: 0.8,
        };
        let mut warnings = Vec::new();
        let table = resolve_mask(&spec, &named, &tree, &mut warnings);

        assert!(warnings.is_empty());
        assert!(table.active);
        // a.rs and everything inside it is lit (ancestor-closed membership).
        assert_eq!(table.light(&a_id), 1.0);
        assert_eq!(table.light(&foo_id), 1.0);
        // b.rs is unrelated, fully dimmed.
        assert!((table.light(&b_id) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn dim_dims_target_and_leaves_the_rest_lit() {
        let (tree, _root, a_id, foo_id, b_id) = sample_tree();
        let mut named = BTreeMap::new();
        named.insert("target".to_string(), set_of([a_id.clone()]));
        let spec = MaskSpec {
            dim_except: None,
            dim: Some(SetRef::Name("target".to_string())),
            strength: 0.8,
        };
        let mut warnings = Vec::new();
        let table = resolve_mask(&spec, &named, &tree, &mut warnings);

        assert!(warnings.is_empty());
        // a.rs (and its contents) are dimmed.
        assert!((table.light(&a_id) - 0.2).abs() < 1e-6);
        assert!((table.light(&foo_id) - 0.2).abs() < 1e-6);
        // b.rs is untouched.
        assert_eq!(table.light(&b_id), 1.0);
    }

    #[test]
    fn ancestor_inheritance_lights_container_at_half() {
        let (tree, root_id, a_id, foo_id, _b_id) = sample_tree();
        let mut named = BTreeMap::new();
        named.insert("target".to_string(), set_of([foo_id.clone()]));
        let spec = MaskSpec {
            dim_except: Some(SetRef::Name("target".to_string())),
            dim: None,
            strength: 0.8,
        };
        let mut warnings = Vec::new();
        let table = resolve_mask(&spec, &named, &tree, &mut warnings);

        assert_eq!(table.light(&foo_id), 1.0);
        // a.rs (foo's parent file) is legible at half light.
        assert!((table.light(&a_id) - 0.5).abs() < 1e-6);
        // root, too (it has a lit descendant).
        assert!((table.light(&root_id) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn empty_set_with_dim_except_dims_everything() {
        let (tree, root_id, a_id, foo_id, b_id) = sample_tree();
        let mut named = BTreeMap::new();
        named.insert("target".to_string(), ResolvedSet::default());
        let spec = MaskSpec {
            dim_except: Some(SetRef::Name("target".to_string())),
            dim: None,
            strength: 0.8,
        };
        let mut warnings = Vec::new();
        let table = resolve_mask(&spec, &named, &tree, &mut warnings);

        for id in [&root_id, &a_id, &foo_id, &b_id] {
            assert!((table.light(id) - 0.2).abs() < 1e-6, "{id:?}");
        }
        assert_eq!(table.dimmed_count(), 4);
    }

    #[test]
    fn strength_zero_is_a_noop() {
        let (tree, _root, a_id, _foo_id, _b_id) = sample_tree();
        let mut named = BTreeMap::new();
        named.insert("target".to_string(), set_of([a_id.clone()]));
        let spec = MaskSpec {
            dim_except: Some(SetRef::Name("target".to_string())),
            dim: None,
            strength: 0.0,
        };
        let mut warnings = Vec::new();
        let table = resolve_mask(&spec, &named, &tree, &mut warnings);

        assert!(table.active);
        assert!(table.is_noop());
        assert_eq!(table.light(&a_id), 1.0);
    }

    #[test]
    fn no_target_is_a_warned_noop() {
        let (tree, _root, _a_id, _foo_id, _b_id) = sample_tree();
        let named = BTreeMap::new();
        let spec = MaskSpec {
            dim_except: None,
            dim: None,
            strength: 0.7,
        };
        let mut warnings = Vec::new();
        let table = resolve_mask(&spec, &named, &tree, &mut warnings);

        assert!(!warnings.is_empty());
        assert!(table.is_noop());
    }

    #[test]
    fn and_multiplies_light_values() {
        let (tree, root_id, a_id, foo_id, b_id) = sample_tree();
        let mut named = BTreeMap::new();
        named.insert("a".to_string(), set_of([a_id.clone()]));
        named.insert("b".to_string(), set_of([b_id.clone()]));

        let spec_a = MaskSpec {
            dim_except: Some(SetRef::Name("a".to_string())),
            dim: None,
            strength: 0.5,
        };
        let spec_b = MaskSpec {
            dim_except: Some(SetRef::Name("b".to_string())),
            dim: None,
            strength: 0.5,
        };
        let mut warnings = Vec::new();
        let table_a = resolve_mask(&spec_a, &named, &tree, &mut warnings);
        let table_b = resolve_mask(&spec_b, &named, &tree, &mut warnings);

        let combined = table_a.and(&table_b);
        // a.rs: lit under A (1.0), dimmed under B (0.5) -> 0.5.
        assert!((combined.light(&a_id) - 0.5).abs() < 1e-6);
        // b.rs: dimmed under A (0.5), lit under B (1.0) -> 0.5.
        assert!((combined.light(&b_id) - 0.5).abs() < 1e-6);
        // foo (inside a.rs): same as a.rs.
        assert!((combined.light(&foo_id) - 0.5).abs() < 1e-6);
        // root: 0.5 under each (has one lit child) -> 0.25.
        assert!((combined.light(&root_id) - 0.25).abs() < 1e-6);

        // Order independence.
        let combined_rev = table_b.and(&table_a);
        assert!((combined_rev.light(&a_id) - combined.light(&a_id)).abs() < 1e-6);
        assert!((combined_rev.light(&root_id) - combined.light(&root_id)).abs() < 1e-6);
    }
}
