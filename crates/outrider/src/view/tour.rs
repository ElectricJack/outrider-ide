//! Guided tours: step-by-step walkthroughs declared in a view's
//! `camera.steps`. Steps are a *script* that builds up layer state as it
//! goes: each step may `push` layers and `pop` layers off the top of the
//! stack the previous steps built. Jumping to step N (forward, back, or
//! direct) replays the contiguous run of steps *on the same tab* ending at
//! N, from that tab's base layers, so the result is deterministic however
//! the reader got there.
//!
//! A tour may span several tabs (`step.tab`): the steps belong to the
//! originating view, and each tab visited gets its own base-layer mark so
//! tour-pushed layers never leak into a tab's saved spec.
//!
//! The engine owns playback state and computes *what* a step needs; the
//! app switches tabs, applies layers, and moves the camera.

use std::collections::BTreeMap;

use outrider_view::spec::{LayerSpec, Step, StepTarget};

/// Playback state for the active tour, if any.
#[derive(Debug, Default)]
pub(crate) struct TourState {
    /// Index of the live step, or None when no tour is playing.
    pub(crate) step: Option<usize>,
    /// Index into the live step's `parts`, or None when on the step itself
    /// (its own target/note, before any sub-step).
    pub(crate) part: Option<usize>,
    /// Tab the tour was started from; it owns the steps.
    pub(crate) origin_tab: usize,
    /// The steps, snapshotted at start so tab switches can't lose them.
    pub(crate) steps: Vec<Step>,
    /// Base layer count per tab index, recorded the first time the tour
    /// lands on that tab. Everything above the base belongs to the tour.
    base_layers: BTreeMap<usize, usize>,
}

/// What the app must do to realise a jump.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StepPlan {
    /// Tab the step plays on (None = the origin tab).
    pub(crate) tab: Option<String>,
    /// Layers the active tab's spec should have above its base after the jump.
    pub(crate) tour_layers: Vec<LayerSpec>,
    pub(crate) target: StepTarget,
    pub(crate) note: Option<String>,
    pub(crate) index: usize,
    pub(crate) total: usize,
}

impl TourState {
    pub(crate) fn is_active(&self) -> bool {
        self.step.is_some()
    }

    /// Begin a tour over `steps`, started from `origin_tab`.
    pub(crate) fn start(&mut self, origin_tab: usize, steps: Vec<Step>) {
        self.origin_tab = origin_tab;
        self.steps = steps;
        self.base_layers.clear();
        self.step = None;
    }

    /// Record a tab's base layer count the first time the tour lands there.
    pub(crate) fn note_base(&mut self, tab: usize, layer_count: usize) {
        self.base_layers.entry(tab).or_insert(layer_count);
    }

    pub(crate) fn base_layers(&self, tab: usize) -> Option<usize> {
        self.base_layers.get(&tab).copied()
    }

    /// Tabs this tour has touched (for unwinding on stop).
    pub(crate) fn touched_tabs(&self) -> Vec<(usize, usize)> {
        self.base_layers.iter().map(|(t, b)| (*t, *b)).collect()
    }

    /// Effective tab label of step `i` (None = origin tab).
    fn tab_of(&self, i: usize) -> Option<&str> {
        self.steps.get(i).and_then(|s| s.tab.as_deref())
    }

    /// Replay the contiguous same-tab run ending at `index` to compute the
    /// tour layer stack for that tab.
    pub(crate) fn plan_goto(&self, index: usize) -> Option<StepPlan> {
        let step = self.steps.get(index)?;
        let tab = self.tab_of(index);
        // Walk back to the start of this tab's run.
        let mut run_start = index;
        while run_start > 0 && self.tab_of(run_start - 1) == tab {
            run_start -= 1;
        }
        let mut stack: Vec<LayerSpec> = Vec::new();
        for s in &self.steps[run_start..=index] {
            for _ in 0..s.pop {
                stack.pop();
            }
            stack.extend(s.push.iter().cloned());
        }
        Some(StepPlan {
            tab: step.tab.clone(),
            tour_layers: stack,
            target: step.target.clone(),
            note: step.note.clone(),
            index,
            total: self.steps.len(),
        })
    }

    pub(crate) fn commit(&mut self, plan: &StepPlan) {
        self.step = Some(plan.index);
        self.part = None;
    }

    pub(crate) fn clear(&mut self) {
        self.step = None;
        self.part = None;
        self.steps.clear();
        self.base_layers.clear();
    }

    /// Number of parts on the live step.
    pub(crate) fn part_count(&self) -> usize {
        self.step
            .and_then(|i| self.steps.get(i))
            .map(|s| s.parts.len())
            .unwrap_or(0)
    }

    /// The live target and note: the current part's if one is active, else
    /// the step's own.
    pub(crate) fn live_target(&self) -> Option<(StepTarget, Option<String>)> {
        let step = self.steps.get(self.step?)?;
        match self.part.and_then(|p| step.parts.get(p)) {
            Some(part) => Some((part.target.clone(), part.note.clone())),
            None => Some((step.target.clone(), step.note.clone())),
        }
    }

    /// What "next" means from here: advance within the step's parts, or
    /// move to the next step.
    pub(crate) fn next_move(&self) -> Option<Advance> {
        let total = self.steps.len();
        match self.step {
            None => (total > 0).then_some(Advance::Step(0)),
            Some(i) => {
                let parts = self.part_count();
                let next_part = match self.part {
                    None => 0,
                    Some(p) => p + 1,
                };
                if next_part < parts {
                    Some(Advance::Part(next_part))
                } else if i + 1 < total {
                    Some(Advance::Step(i + 1))
                } else {
                    None
                }
            }
        }
    }

    /// What "prev" means: back through parts, then to the previous step —
    /// landing on its *last* part if it has any, so ← is the exact inverse
    /// of →.
    pub(crate) fn prev_move(&self) -> Option<Advance> {
        let i = self.step?;
        match self.part {
            Some(0) => Some(Advance::Part0Back),
            Some(p) => Some(Advance::Part(p - 1)),
            None => {
                if i == 0 {
                    return None;
                }
                let prev_parts = self.steps.get(i - 1).map(|s| s.parts.len()).unwrap_or(0);
                if prev_parts > 0 {
                    Some(Advance::StepAtPart(i - 1, prev_parts - 1))
                } else {
                    Some(Advance::Step(i - 1))
                }
            }
        }
    }

    pub(crate) fn set_part(&mut self, part: Option<usize>) {
        self.part = part;
    }
}

/// A navigation move computed by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Advance {
    /// Go to step `i` (its own target, before its parts).
    Step(usize),
    /// Go to part `p` of the live step.
    Part(usize),
    /// Leave the live step's first part back to the step itself.
    Part0Back,
    /// Go to step `i`, landing directly on its part `p`.
    StepAtPart(usize, usize),
}

/// First line of a note, for compact callouts and list rows.
pub(crate) fn headline(note: &str) -> &str {
    note.lines().next().unwrap_or("").trim()
}

/// Everything after the first line, trimmed; empty if the note is one line.
pub(crate) fn body(note: &str) -> &str {
    match note.split_once('\n') {
        Some((_, rest)) => rest.trim(),
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_view::spec::{HomeFlag, MaskSpec, SetRef};

    fn mask(name: &str) -> LayerSpec {
        LayerSpec::Mask(MaskSpec {
            dim_except: Some(SetRef::Name(name.into())),
            dim: None,
            strength: 0.5,
        })
    }

    fn step(push: &[&str], pop: usize) -> Step {
        Step {
            target: StepTarget::Home(HomeFlag),
            push: push.iter().map(|n| mask(n)).collect(),
            pop,
            note: Some("Title\nbody text".into()),
            tab: None,
            parts: Vec::new(),
        }
    }

    fn step_on(tab: &str, push: &[&str], pop: usize) -> Step {
        let mut s = step(push, pop);
        s.tab = Some(tab.into());
        s
    }

    fn names(layers: &[LayerSpec]) -> Vec<String> {
        layers
            .iter()
            .map(|l| match l {
                LayerSpec::Mask(m) => match &m.dim_except {
                    Some(SetRef::Name(n)) => n.clone(),
                    _ => "?".into(),
                },
                _ => "?".into(),
            })
            .collect()
    }

    fn started(steps: Vec<Step>) -> TourState {
        let mut t = TourState::default();
        t.start(0, steps);
        t
    }

    #[test]
    fn pushes_accumulate_and_pop_unwinds_from_top() {
        let t = started(vec![
            step(&["a", "b"], 0), // stack: a b
            step(&[], 0),         // stack: a b
            step(&["c"], 1),      // stack: a c
            step(&[], 2),         // stack: (empty)
        ]);
        assert_eq!(names(&t.plan_goto(0).unwrap().tour_layers), ["a", "b"]);
        assert_eq!(names(&t.plan_goto(1).unwrap().tour_layers), ["a", "b"]);
        assert_eq!(names(&t.plan_goto(2).unwrap().tour_layers), ["a", "c"]);
        assert!(t.plan_goto(3).unwrap().tour_layers.is_empty());
        assert!(t.plan_goto(4).is_none());
    }

    #[test]
    fn replay_is_independent_of_path() {
        let t = started(vec![step(&["a"], 0), step(&["b"], 0), step(&["c"], 1)]);
        assert_eq!(names(&t.plan_goto(2).unwrap().tour_layers), ["a", "c"]);
    }

    #[test]
    fn layer_stack_resets_when_tab_changes() {
        let t = started(vec![
            step(&["a"], 0),            // origin tab: a
            step_on("Seams", &["x"], 0), // Seams: x   (a does not carry over)
            step_on("Seams", &["y"], 0), // Seams: x y
            step(&["b"], 0),            // back on origin: b (fresh run, not a b)
        ]);
        assert_eq!(names(&t.plan_goto(0).unwrap().tour_layers), ["a"]);
        assert_eq!(names(&t.plan_goto(1).unwrap().tour_layers), ["x"]);
        assert_eq!(names(&t.plan_goto(2).unwrap().tour_layers), ["x", "y"]);
        assert_eq!(names(&t.plan_goto(3).unwrap().tour_layers), ["b"]);
        assert_eq!(t.plan_goto(1).unwrap().tab.as_deref(), Some("Seams"));
        assert_eq!(t.plan_goto(3).unwrap().tab, None);
    }

    #[test]
    fn bases_recorded_once_per_tab() {
        let mut t = started(vec![step(&[], 0)]);
        t.note_base(0, 3);
        t.note_base(0, 99);
        t.note_base(2, 1);
        assert_eq!(t.base_layers(0), Some(3));
        assert_eq!(t.base_layers(2), Some(1));
        assert_eq!(t.base_layers(5), None);
        let mut touched = t.touched_tabs();
        touched.sort();
        assert_eq!(touched, vec![(0, 3), (2, 1)]);
    }

    #[test]
    fn next_prev_bounds() {
        let mut t = started(vec![step(&[], 0), step(&[], 0), step(&[], 0)]);
        assert_eq!(t.next_move(), Some(Advance::Step(0)));
        assert_eq!(t.prev_move(), None);
        t.step = Some(2);
        assert_eq!(t.next_move(), None);
        assert_eq!(t.prev_move(), Some(Advance::Step(1)));
    }

    #[test]
    fn parts_are_walked_before_the_next_step_and_reversed_exactly() {
        use outrider_view::spec::StepPart;
        let mut s1 = step(&[], 0);
        s1.parts = vec![
            StepPart { target: StepTarget::Home(HomeFlag), note: Some("p0".into()) },
            StepPart { target: StepTarget::Home(HomeFlag), note: Some("p1".into()) },
        ];
        let mut t = started(vec![step(&[], 0), s1, step(&[], 0)]);
        // Forward: step0 -> step1 -> p0 -> p1 -> step2
        t.step = Some(0);
        assert_eq!(t.next_move(), Some(Advance::Step(1)));
        t.step = Some(1);
        assert_eq!(t.next_move(), Some(Advance::Part(0)));
        t.set_part(Some(0));
        assert_eq!(t.next_move(), Some(Advance::Part(1)));
        assert_eq!(t.live_target().unwrap().1.as_deref(), Some("p0"));
        t.set_part(Some(1));
        assert_eq!(t.next_move(), Some(Advance::Step(2)));
        // Backward from step2 lands on step1's LAST part.
        t.step = Some(2);
        t.set_part(None);
        assert_eq!(t.prev_move(), Some(Advance::StepAtPart(1, 1)));
        t.step = Some(1);
        t.set_part(Some(1));
        assert_eq!(t.prev_move(), Some(Advance::Part(0)));
        t.set_part(Some(0));
        assert_eq!(t.prev_move(), Some(Advance::Part0Back));
        t.set_part(None);
        assert_eq!(t.prev_move(), Some(Advance::Step(0)));
        assert_eq!(t.part_count(), 2);
    }

    #[test]
    fn headline_and_body_split() {
        assert_eq!(headline("Title\nbody\nmore"), "Title");
        assert_eq!(body("Title\nbody\nmore"), "body\nmore");
        assert_eq!(body("only"), "");
    }
}
