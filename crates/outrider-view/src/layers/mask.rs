//! Mask layer resolution: attention control (spotlight / dim).
//!
//! A mask dims everything except a set (spotlight, `dimExcept`) or dims a
//! set (`dim`). Resolution produces a per-symbol `light` value in `0..=1`
//! that the renderer multiplies into fill/border/stripe/text/ring alpha.
//! See `docs/view-primitives/04-mask.md` for the full algorithm writeup.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use outrider_index::{SymbolId, SymbolNode, SymbolTree};

use crate::deps::Deps;
use crate::set::ResolvedSet;
use crate::spec::{MaskSpec, SetRef};

/// The resolved result of one or more (AND-combined) mask layers.
///
/// Stored as a baseline light plus per-symbol exceptions. A `dimExcept`
/// mask dims almost the whole tree, so storing the dimmed nodes would
/// clone tens of thousands of ids per resolve; storing the (few) lit
/// exceptions against a dimmed baseline keeps resolution proportional to
/// the spotlit set instead of the tree.
#[derive(Debug, Clone)]
pub struct MaskTable {
    /// Per-symbol light values in `0..=1` that differ from the baseline.
    /// Shared: a table can hold tens of thousands of entries and is cloned
    /// into the layer memo, the spec cache and the combined mask each
    /// resolve — those clones must not walk it.
    light: Arc<HashMap<SymbolId, f32>>,
    /// Light for every symbol not listed in `light`.
    default_light: f32,
    pub deps: Deps,
    /// Whether any mask layer is present (even if it dims nothing, e.g.
    /// `strength: 0`).
    pub active: bool,
}

impl Default for MaskTable {
    fn default() -> Self {
        MaskTable {
            light: Arc::new(HashMap::new()),
            default_light: 1.0,
            deps: Deps::NONE,
            active: false,
        }
    }
}

impl MaskTable {
    /// The light value for a symbol: the baseline unless listed.
    #[inline]
    pub fn light(&self, id: &SymbolId) -> f32 {
        self.light.get(id).copied().unwrap_or(self.default_light)
    }

    /// True when this table dims nothing at all.
    pub fn is_noop(&self) -> bool {
        self.light.is_empty() && self.default_light >= 1.0
    }

    /// Combine two mask tables by ANDing their light values
    /// (`final_light = light1 * light2`). A node absent from both gets the
    /// product of the baselines.
    pub fn and(&self, other: &MaskTable) -> MaskTable {
        // A no-op side contributes nothing: share the other table's map
        // instead of re-walking it (the first layer always ANDs into the
        // identity table).
        let merged_deps = self.deps.union(other.deps);
        let merged_active = self.active || other.active;
        if self.is_noop() {
            return MaskTable {
                light: Arc::clone(&other.light),
                default_light: other.default_light,
                deps: merged_deps,
                active: merged_active,
            };
        }
        if other.is_noop() {
            return MaskTable {
                light: Arc::clone(&self.light),
                default_light: self.default_light,
                deps: merged_deps,
                active: merged_active,
            };
        }
        let default_light = self.default_light * other.default_light;
        let mut light: HashMap<SymbolId, f32> =
            HashMap::with_capacity(self.light.len() + other.light.len());
        for id in self.light.keys().chain(other.light.keys()) {
            let v = self.light(id) * other.light(id);
            if v != default_light {
                light.insert(id.clone(), v);
            }
        }
        MaskTable {
            light: Arc::new(light),
            default_light,
            deps: merged_deps,
            active: merged_active,
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
    default_light: f32,
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
        desc_max = desc_max.max(walk(
            c,
            member,
            set,
            dim_except,
            strength,
            default_light,
            out,
        ));
    }

    let g = if node.children.is_empty() {
        f
    } else {
        f.max(0.5 * desc_max)
    };

    // Only exceptions to the baseline are stored (see `MaskTable`).
    if g != default_light {
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

    // Baseline: whichever value covers more of the tree, so the exception
    // map stays small. `dim` always lists the dimmed set. `dimExcept`
    // usually dims the world and lists the lit spotlight — but when the
    // spotlight itself is most of the tree (e.g. "everything but the
    // attic"), a lit baseline listing the dimmed remainder is far smaller.
    let spotlight_is_majority = resolved_set.ids.len() > 8192;
    let default_light = if dim_except && !spotlight_is_majority {
        1.0 - strength
    } else {
        1.0
    };
    let mut light = HashMap::new();
    walk(
        &tree.root,
        false,
        &resolved_set.ids,
        dim_except,
        strength,
        default_light,
        &mut light,
    );

    MaskTable {
        light: Arc::new(light),
        default_light,
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
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
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
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
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
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
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
        assert!(!table.is_noop());
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
