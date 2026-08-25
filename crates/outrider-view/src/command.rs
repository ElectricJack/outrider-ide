//! View commands: mutations on a ViewSpec.

use crate::deps::Deps;
use crate::spec::*;
use crate::validate::{self, Violation};

/// Camera intent commands enacted by the app (pure apply is a no-op).
#[derive(Debug, Clone, PartialEq)]
pub enum CameraCommand {
    Frame(SetRef),
    Focus(String),
    Follow(FollowMode),
    Home,
}

/// Tour commands for guided walkthroughs.
#[derive(Debug, Clone, PartialEq)]
pub enum TourCommand {
    Add(Step),
    SetSteps(Vec<Step>),
    Play,
    Next,
    Prev,
    Goto(usize),
    Stop,
}

/// All mutations the UI, RPC, or CLI can apply to a ViewSpec.
#[derive(Debug, Clone)]
pub enum ViewCommand {
    Apply(ViewSpec),
    Patch(ViewPatch),
    Clear(ClearScope),
    DefineSet { name: String, expr: SetExpr },
    PushLayer(LayerSpec),
    PopLayer,
    RemoveLayer(usize),
    ImportMetric { name: String, metric: ImportedMetric },
    Camera(CameraCommand),
    Tour(TourCommand),
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewPatch {
    pub space: Option<SpaceSpec>,
    pub sets: std::collections::BTreeMap<String, SetExpr>,
    pub metrics: std::collections::BTreeMap<String, ImportedMetric>,
    pub layers: Vec<LayerSpec>,
    pub camera: Option<CameraSpec>,
}

#[derive(Debug, Clone, Copy)]
pub enum ClearScope {
    Layers,
    Sets,
    All,
}

/// Result of applying a command.
pub struct Applied {
    pub changed: Deps,
    pub violations: Vec<Violation>,
}

/// Apply a command to a view spec. Validates the result; if hard violations
/// exist, the spec is left untouched.
pub fn apply(cmd: ViewCommand, spec: &mut ViewSpec) -> Applied {
    let mut candidate = spec.clone();
    let changed = match cmd {
        ViewCommand::Apply(new_spec) => {
            let old_metrics = std::mem::take(&mut candidate.metrics);
            candidate = new_spec;
            // Preserve existing imported metrics not redefined
            for (k, v) in old_metrics {
                candidate.metrics.entry(k).or_insert(v);
            }
            Deps::SPEC
        }
        ViewCommand::Patch(patch) => {
            if let Some(space) = patch.space {
                candidate.space = space;
            }
            for (k, v) in patch.sets {
                candidate.sets.insert(k, v);
            }
            for (k, v) in patch.metrics {
                candidate.metrics.insert(k, v);
            }
            candidate.layers.extend(patch.layers);
            if let Some(camera) = patch.camera {
                candidate.camera = camera;
            }
            Deps::SPEC
        }
        ViewCommand::Clear(scope) => {
            match scope {
                ClearScope::Layers => candidate.layers.clear(),
                ClearScope::Sets => candidate.sets.clear(),
                ClearScope::All => {
                    candidate.layers.clear();
                    candidate.sets.clear();
                    candidate.camera = CameraSpec::default();
                    candidate.space = SpaceSpec::default();
                }
            }
            Deps::SPEC
        }
        ViewCommand::DefineSet { name, expr } => {
            candidate.sets.insert(name, expr);
            Deps::SPEC
        }
        ViewCommand::PushLayer(layer) => {
            candidate.layers.push(layer);
            Deps::SPEC
        }
        ViewCommand::PopLayer => {
            candidate.layers.pop();
            Deps::SPEC
        }
        ViewCommand::RemoveLayer(idx) => {
            if idx < candidate.layers.len() {
                candidate.layers.remove(idx);
            }
            Deps::SPEC
        }
        ViewCommand::ImportMetric { name, metric } => {
            candidate.metrics.insert(name, metric);
            Deps::SPEC.union(Deps::METRICS)
        }
        ViewCommand::Camera(CameraCommand::Follow(m)) => {
            candidate.camera.follow = m;
            Deps::SPEC
        }
        ViewCommand::Camera(_) => Deps::NONE,
        ViewCommand::Tour(TourCommand::Add(s)) => {
            candidate.camera.steps.push(s);
            Deps::SPEC
        }
        ViewCommand::Tour(TourCommand::SetSteps(v)) => {
            candidate.camera.steps = v;
            candidate.camera.step = None;
            Deps::SPEC
        }
        ViewCommand::Tour(TourCommand::Goto(n)) => {
            if n >= candidate.camera.steps.len() {
                return Applied {
                    changed: Deps::NONE,
                    violations: vec![Violation {
                        path: "camera.step".to_string(),
                        rule: "tour.goto_out_of_bounds",
                        message: format!(
                            "step index {n} out of bounds ({})",
                            candidate.camera.steps.len()
                        ),
                        hard: true,
                    }],
                };
            }
            candidate.camera.step = Some(n);
            Deps::SPEC
        }
        ViewCommand::Tour(TourCommand::Stop) => {
            candidate.camera.step = None;
            Deps::SPEC
        }
        // Play, Next, Prev are rewritten by the app before reaching apply.
        ViewCommand::Tour(TourCommand::Play | TourCommand::Next | TourCommand::Prev) => {
            Deps::NONE
        }
    };

    let violations = validate::validate(&candidate, &|_| true, &|_| true);
    let hard = violations.iter().any(|v| v.hard);
    if hard {
        return Applied {
            changed: Deps::NONE,
            violations,
        };
    }

    *spec = candidate;
    Applied { changed, violations }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn define_set_adds_to_spec() {
        let mut spec = ViewSpec::default();
        let applied = apply(
            ViewCommand::DefineSet {
                name: "focusSet".to_string(),
                expr: SetExpr::Ids(vec!["$focus".to_string()]),
            },
            &mut spec,
        );
        assert!(applied.violations.iter().all(|v| !v.hard));
        assert!(spec.sets.contains_key("focusSet"));
    }

    #[test]
    fn push_layer_referencing_unknown_set_is_rejected() {
        let mut spec = ViewSpec::default();
        let applied = apply(
            ViewCommand::PushLayer(LayerSpec::Marks(MarksSpec {
                on: MarkTarget::Set(SetRef::Name("noSuchSet".to_string())),
                kind: MarkKind::FocusRing,
                label: None,
                basis: None,
            })),
            &mut spec,
        );
        assert!(applied.violations.iter().any(|v| v.hard));
        assert!(spec.layers.is_empty());
    }

    fn test_step() -> Step {
        Step {
            target: StepTarget::Home(HomeFlag),
            push: vec![],
            pop: 0,
            note: None,
            tab: None,
            parts: Vec::new(),
        }
    }

    #[test]
    fn tour_add_appends_step() {
        let mut spec = ViewSpec::default();
        let applied = apply(ViewCommand::Tour(TourCommand::Add(test_step())), &mut spec);
        assert_eq!(applied.changed, Deps::SPEC);
        assert_eq!(spec.camera.steps.len(), 1);
    }

    #[test]
    fn tour_set_steps_replaces() {
        let mut spec = ViewSpec::default();
        spec.camera.steps.push(test_step());
        spec.camera.step = Some(0);
        let new_steps = vec![test_step(), test_step()];
        let applied = apply(
            ViewCommand::Tour(TourCommand::SetSteps(new_steps)),
            &mut spec,
        );
        assert_eq!(applied.changed, Deps::SPEC);
        assert_eq!(spec.camera.steps.len(), 2);
        assert_eq!(spec.camera.step, None);
    }

    #[test]
    fn tour_goto_sets_step() {
        let mut spec = ViewSpec::default();
        spec.camera.steps.push(test_step());
        let applied = apply(ViewCommand::Tour(TourCommand::Goto(0)), &mut spec);
        assert_eq!(applied.changed, Deps::SPEC);
        assert_eq!(spec.camera.step, Some(0));
    }

    #[test]
    fn tour_goto_out_of_bounds_violation() {
        let mut spec = ViewSpec::default();
        spec.camera.steps.push(test_step());
        let applied = apply(ViewCommand::Tour(TourCommand::Goto(5)), &mut spec);
        assert!(applied.violations.iter().any(|v| v.hard));
        assert_eq!(applied.changed, Deps::NONE);
    }

    #[test]
    fn tour_stop_clears_step() {
        let mut spec = ViewSpec::default();
        spec.camera.steps.push(test_step());
        spec.camera.step = Some(0);
        let applied = apply(ViewCommand::Tour(TourCommand::Stop), &mut spec);
        assert_eq!(applied.changed, Deps::SPEC);
        assert_eq!(spec.camera.step, None);
    }

    #[test]
    fn camera_follow_sets_mode() {
        let mut spec = ViewSpec::default();
        let applied = apply(
            ViewCommand::Camera(CameraCommand::Follow(FollowMode::None)),
            &mut spec,
        );
        assert_eq!(applied.changed, Deps::SPEC);
        assert_eq!(spec.camera.follow, FollowMode::None);
    }

    #[test]
    fn camera_frame_reports_none() {
        let mut spec = ViewSpec::default();
        let applied = apply(
            ViewCommand::Camera(CameraCommand::Frame(SetRef::Name("x".to_string()))),
            &mut spec,
        );
        assert_eq!(applied.changed, Deps::NONE);
    }
}
