use std::time::{Duration, Instant};

use outrider_layout::{PackLayout, Rect};

pub(crate) const PACK_LAYOUT_TWEEN: Duration = Duration::from_millis(160);

pub(crate) struct LayoutTransition {
    from: PackLayout,
    to: PackLayout,
    started_at: Instant,
}

impl LayoutTransition {
    pub(crate) fn new(from: PackLayout, to: PackLayout, now: Instant) -> Self {
        Self {
            from,
            to,
            started_at: now,
        }
    }

    pub(crate) fn sample(&self, now: Instant) -> PackLayout {
        if self.from.rects.keys().ne(self.to.rects.keys()) {
            return self.to.clone();
        }
        if now <= self.started_at {
            return self.from.clone();
        }
        if self.is_complete(now) {
            return self.to.clone();
        }

        let t = now.duration_since(self.started_at).as_secs_f64() / PACK_LAYOUT_TWEEN.as_secs_f64();
        PackLayout {
            rects: self
                .from
                .rects
                .iter()
                .map(|(id, from)| {
                    let to = self.to.rects[id];
                    (
                        id.clone(),
                        Rect {
                            x: ease_out_lerp(from.x, to.x, t),
                            y: ease_out_lerp(from.y, to.y, t),
                            w: ease_out_lerp(from.w, to.w, t),
                            h: ease_out_lerp(from.h, to.h, t),
                        },
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn is_complete(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) >= PACK_LAYOUT_TWEEN
    }

    pub(crate) fn retarget(self, target: PackLayout, now: Instant) -> Self {
        Self::new(self.sample(now), target, now)
    }
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn ease_out_lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * ease_out_cubic(t)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use outrider_index::{SymbolId, SymbolKind};
    use outrider_layout::{PackLayout, Rect};

    use super::{ease_out_lerp, LayoutTransition};

    fn id(path: &str) -> SymbolId {
        SymbolId {
            kind: SymbolKind::File,
            qualified_path: path.into(),
            ordinal: 0,
        }
    }

    fn layout(entries: &[(&SymbolId, Rect)]) -> PackLayout {
        PackLayout {
            rects: entries
                .iter()
                .map(|(id, rect)| ((*id).clone(), *rect))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn samples_exact_endpoints_and_midpoint_geometry() {
        let id = id("src/main.rs");
        let from = layout(&[(
            &id,
            Rect {
                x: 0.0,
                y: 10.0,
                w: 20.0,
                h: 30.0,
            },
        )]);
        let to = layout(&[(
            &id,
            Rect {
                x: 100.0,
                y: 50.0,
                w: 60.0,
                h: 70.0,
            },
        )]);
        let now = Instant::now();
        let transition = LayoutTransition::new(from.clone(), to.clone(), now);

        assert_eq!(transition.sample(now), from);
        assert_eq!(transition.sample(now + Duration::from_millis(160)), to);
        let halfway = transition.sample(now + Duration::from_millis(80));
        assert_eq!(halfway.rects[&id].x, ease_out_lerp(0.0, 100.0, 0.5));
        assert_eq!(halfway.rects[&id].y, ease_out_lerp(10.0, 50.0, 0.5));
        assert_eq!(halfway.rects[&id].w, ease_out_lerp(20.0, 60.0, 0.5));
        assert_eq!(halfway.rects[&id].h, ease_out_lerp(30.0, 70.0, 0.5));
    }

    #[test]
    fn mismatched_ids_fall_back_to_the_complete_target() {
        let old_id = id("old.rs");
        let new_id = id("new.rs");
        let from = layout(&[(
            &old_id,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        )]);
        let to = layout(&[(
            &new_id,
            Rect {
                x: 10.0,
                y: 10.0,
                w: 2.0,
                h: 2.0,
            },
        )]);
        let now = Instant::now();

        assert_eq!(LayoutTransition::new(from, to.clone(), now).sample(now), to);
    }

    #[test]
    fn retargeting_starts_from_the_current_sample() {
        let id = id("src/lib.rs");
        let from = layout(&[(
            &id,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
        )]);
        let first_target = layout(&[(
            &id,
            Rect {
                x: 100.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
        )]);
        let second_target = layout(&[(
            &id,
            Rect {
                x: 200.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
        )]);
        let now = Instant::now();
        let retarget_at = now + Duration::from_millis(80);
        let expected_start =
            LayoutTransition::new(from.clone(), first_target.clone(), now).sample(retarget_at);
        let retargeted = LayoutTransition::new(from, first_target, now)
            .retarget(second_target.clone(), retarget_at);

        assert_eq!(retargeted.sample(retarget_at), expected_start);
        assert_eq!(
            retargeted.sample(retarget_at + Duration::from_millis(160)),
            second_target
        );
    }
}
