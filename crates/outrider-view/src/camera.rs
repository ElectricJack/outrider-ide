//! Camera resolution helpers: union rects for set framing.

use outrider_index::SymbolId;
use outrider_layout::{PackLayout, Rect};

use crate::spec::{LayerSpec, Step};

// ── Tour state & helpers ──

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TourState {
    pub active: bool,
    pub index: usize,
    pub base: Vec<LayerSpec>,
}

/// Layer stack that should be active at step `n` (inclusive), starting from `base`
/// (the stack snapshotted at Play). For each step 0..=n: truncate by `pop` (saturating), then extend by `push`.
pub fn layers_at_step(base: &[LayerSpec], steps: &[Step], n: usize) -> Vec<LayerSpec> {
    let mut stack = base.to_vec();
    if steps.is_empty() {
        return stack;
    }
    for step in &steps[..=n.min(steps.len() - 1)] {
        let new_len = stack.len().saturating_sub(step.pop);
        stack.truncate(new_len);
        stack.extend(step.push.iter().cloned());
    }
    stack
}

/// Minimal pop/push edit from `cur` to `target` (longest common prefix).
pub fn diff_layers(cur: &[LayerSpec], target: &[LayerSpec]) -> (usize, Vec<LayerSpec>) {
    let common = cur.iter().zip(target.iter()).take_while(|(a, b)| a == b).count();
    let pops = cur.len() - common;
    let pushes = target[common..].to_vec();
    (pops, pushes)
}

/// Union rect of every member with a layout rect. Members with no rect are
/// skipped. Returns `None` iff no member has a rect.
pub fn union_rect<'a>(
    ids: impl IntoIterator<Item = &'a SymbolId>,
    layout: &PackLayout,
) -> Option<Rect> {
    let mut result: Option<Rect> = None;
    for id in ids {
        if let Some(r) = layout.rects.get(id) {
            result = Some(match result {
                None => *r,
                Some(acc) => {
                    let x = acc.x.min(r.x);
                    let y = acc.y.min(r.y);
                    let x2 = (acc.x + acc.w).max(r.x + r.w);
                    let y2 = (acc.y + acc.h).max(r.y + r.h);
                    Rect {
                        x,
                        y,
                        w: x2 - x,
                        h: y2 - y,
                    }
                }
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{FillChannel, FillSpec, MaskSpec, Scale, SetRef, Step, StepTarget};
    use outrider_index::SymbolKind;
    use std::collections::BTreeMap;

    fn sid(path: &str) -> SymbolId {
        SymbolId {
            kind: SymbolKind::File,
            qualified_path: path.to_string(),
            ordinal: 0,
        }
    }

    #[test]
    fn empty_iterator_returns_none() {
        let layout = PackLayout {
            rects: BTreeMap::new(),
        };
        assert!(union_rect(std::iter::empty(), &layout).is_none());
    }

    #[test]
    fn missing_ids_returns_none() {
        let layout = PackLayout {
            rects: BTreeMap::new(),
        };
        let ids = vec![sid("a.rs")];
        assert!(union_rect(ids.iter(), &layout).is_none());
    }

    #[test]
    fn single_id_returns_its_rect() {
        let a = sid("a.rs");
        let r = Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 50.0,
        };
        let mut rects = BTreeMap::new();
        rects.insert(a.clone(), r);
        let layout = PackLayout { rects };
        let u = union_rect([&a], &layout).unwrap();
        assert!((u.x - 10.0).abs() < 1e-9);
        assert!((u.y - 20.0).abs() < 1e-9);
        assert!((u.w - 100.0).abs() < 1e-9);
        assert!((u.h - 50.0).abs() < 1e-9);
    }

    #[test]
    fn two_ids_returns_envelope() {
        let a = sid("a.rs");
        let b = sid("b.rs");
        let mut rects = BTreeMap::new();
        rects.insert(
            a.clone(),
            Rect {
                x: 10.0,
                y: 20.0,
                w: 100.0,
                h: 50.0,
            },
        );
        rects.insert(
            b.clone(),
            Rect {
                x: 200.0,
                y: 5.0,
                w: 80.0,
                h: 30.0,
            },
        );
        let layout = PackLayout { rects };
        let u = union_rect([&a, &b], &layout).unwrap();
        assert!((u.x - 10.0).abs() < 1e-9);
        assert!((u.y - 5.0).abs() < 1e-9);
        assert!((u.w - 270.0).abs() < 1e-9); // 280 - 10
        assert!((u.h - 65.0).abs() < 1e-9); // 70 - 5
    }

    #[test]
    fn skips_ids_without_rects() {
        let a = sid("a.rs");
        let b = sid("b.rs");
        let mut rects = BTreeMap::new();
        rects.insert(
            a.clone(),
            Rect {
                x: 10.0,
                y: 20.0,
                w: 100.0,
                h: 50.0,
            },
        );
        let layout = PackLayout { rects };
        let u = union_rect([&a, &b], &layout).unwrap();
        assert!((u.w - 100.0).abs() < 1e-9);
    }

    // ── Tour helpers ──

    fn fill_layer(metric: &str) -> LayerSpec {
        LayerSpec::Fill(FillSpec {
            metric: metric.to_string(),
            ..FillSpec::default()
        })
    }

    fn mask_layer_spec(set: &str) -> LayerSpec {
        LayerSpec::Mask(MaskSpec {
            dim_except: Some(SetRef::Name(set.to_string())),
            dim: None,
            strength: 0.7,
        })
    }

    fn frame_step(set: &str, push: Vec<LayerSpec>, pop: usize) -> Step {
        Step {
            target: StepTarget::Frame(SetRef::Name(set.to_string())),
            push,
            pop,
            note: None,
        }
    }

    // ── layers_at_step tests ──

    #[test]
    fn layers_at_step_no_steps() {
        let base = vec![fill_layer("F")];
        let result = layers_at_step(&base, &[], 0);
        assert_eq!(result, vec![fill_layer("F")]);
    }

    #[test]
    fn layers_at_step_push_only() {
        let base = vec![fill_layer("F")];
        let steps = vec![
            frame_step("s1", vec![fill_layer("A"), fill_layer("B")], 0),
            frame_step("s2", vec![fill_layer("C")], 0),
        ];
        let at0 = layers_at_step(&base, &steps, 0);
        assert_eq!(at0, vec![fill_layer("F"), fill_layer("A"), fill_layer("B")]);
        let at1 = layers_at_step(&base, &steps, 1);
        assert_eq!(
            at1,
            vec![fill_layer("F"), fill_layer("A"), fill_layer("B"), fill_layer("C")]
        );
    }

    #[test]
    fn layers_at_step_pop_and_push() {
        let base = vec![fill_layer("F")];
        let steps = vec![
            frame_step("s1", vec![fill_layer("A"), fill_layer("B")], 0),
            frame_step("s2", vec![fill_layer("C")], 1),
            frame_step("s3", vec![], 3),
        ];
        let at0 = layers_at_step(&base, &steps, 0);
        assert_eq!(at0, vec![fill_layer("F"), fill_layer("A"), fill_layer("B")]);
        let at1 = layers_at_step(&base, &steps, 1);
        assert_eq!(at1, vec![fill_layer("F"), fill_layer("A"), fill_layer("C")]);
        let at2 = layers_at_step(&base, &steps, 2);
        assert_eq!(at2, Vec::<LayerSpec>::new());
    }

    #[test]
    fn layers_at_step_saturating_pop() {
        let base = vec![fill_layer("F")];
        let steps = vec![frame_step("s1", vec![], 99)];
        let result = layers_at_step(&base, &steps, 0);
        assert_eq!(result, Vec::<LayerSpec>::new());
    }

    // ── diff_layers tests ──

    #[test]
    fn diff_layers_identical() {
        let layers = vec![fill_layer("A"), fill_layer("B")];
        let (pops, pushes) = diff_layers(&layers, &layers);
        assert_eq!(pops, 0);
        assert!(pushes.is_empty());
    }

    #[test]
    fn diff_layers_append() {
        let cur = vec![fill_layer("A")];
        let target = vec![fill_layer("A"), fill_layer("B")];
        let (pops, pushes) = diff_layers(&cur, &target);
        assert_eq!(pops, 0);
        assert_eq!(pushes, vec![fill_layer("B")]);
    }

    #[test]
    fn diff_layers_replace_tail() {
        let cur = vec![fill_layer("A"), fill_layer("B")];
        let target = vec![fill_layer("A"), fill_layer("C")];
        let (pops, pushes) = diff_layers(&cur, &target);
        assert_eq!(pops, 1);
        assert_eq!(pushes, vec![fill_layer("C")]);
    }

    #[test]
    fn diff_layers_full_replace() {
        let cur = vec![fill_layer("A"), fill_layer("B")];
        let target = vec![fill_layer("C")];
        let (pops, pushes) = diff_layers(&cur, &target);
        assert_eq!(pops, 2);
        assert_eq!(pushes, vec![fill_layer("C")]);
    }

    #[test]
    fn diff_layers_empty_target() {
        let cur = vec![fill_layer("A"), fill_layer("B")];
        let target: Vec<LayerSpec> = vec![];
        let (pops, pushes) = diff_layers(&cur, &target);
        assert_eq!(pops, 2);
        assert!(pushes.is_empty());
    }

    // ── TourState tests ──

    #[test]
    fn tour_state_default() {
        let ts = TourState::default();
        assert!(!ts.active);
        assert_eq!(ts.index, 0);
        assert!(ts.base.is_empty());
    }
}
