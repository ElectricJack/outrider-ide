#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SkylineLayout {
    pub positions: Vec<(f64, f64)>,
    pub bounds: (f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    x: f64,
    width: f64,
    height: f64,
}

impl Segment {
    fn end(self) -> f64 {
        self.x + self.width
    }
}

const WIDTH_FACTORS: [f64; 9] = [0.5, 0.625, 0.75, 0.875, 1.0, 1.125, 1.5, 2.0, 3.0];

pub(crate) fn skyline_pack(sizes: &[(f64, f64)], gap: f64, aspect: f64) -> SkylineLayout {
    if sizes.len() <= 1 {
        return SkylineLayout {
            positions: sizes.iter().map(|_| (0.0, 0.0)).collect(),
            bounds: sizes.first().copied().unwrap_or((0.0, 0.0)),
        };
    }
    let widest = sizes
        .iter()
        .map(|(width, _)| width + gap)
        .fold(0.0, f64::max);
    let padded_area: f64 = sizes
        .iter()
        .map(|(width, height)| (width + gap) * (height + gap))
        .sum();
    let baseline = widest.max((padded_area * aspect).sqrt());
    let mut widths = vec![baseline];
    for factor in WIDTH_FACTORS {
        let candidate = widest.max(baseline * factor);
        if !widths.contains(&candidate) {
            widths.push(candidate);
        }
    }
    let mut best = pack_at_width(sizes, gap, widths[0]);
    let mut best_score = aspect_envelope_area(best.bounds, aspect);
    for width in widths.into_iter().skip(1) {
        let candidate = pack_at_width(sizes, gap, width);
        let score = aspect_envelope_area(candidate.bounds, aspect);
        if score.total_cmp(&best_score).is_lt() {
            best = candidate;
            best_score = score;
        }
    }
    best
}

fn aspect_envelope_area((width, height): (f64, f64), aspect: f64) -> f64 {
    let envelope_width = width.max(height * aspect);
    envelope_width * (envelope_width / aspect)
}

fn pack_at_width(sizes: &[(f64, f64)], gap: f64, bin_width: f64) -> SkylineLayout {
    if sizes.is_empty() {
        return SkylineLayout {
            positions: vec![],
            bounds: (0.0, 0.0),
        };
    }
    let mut skyline = vec![Segment {
        x: 0.0,
        width: bin_width,
        height: 0.0,
    }];
    let mut positions = Vec::with_capacity(sizes.len());
    let (mut used_w, mut used_h) = (0.0_f64, 0.0_f64);

    for &(width, height) in sizes {
        let padded_w = width + gap;
        let padded_h = height + gap;
        let (x, y) = skyline
            .iter()
            .map(|segment| segment.x)
            .filter(|x| *x + padded_w <= bin_width)
            .map(|x| {
                let y = skyline
                    .iter()
                    .filter(|segment| segment.x < x + padded_w && segment.end() > x)
                    .map(|segment| segment.height)
                    .fold(0.0, f64::max);
                (x, y)
            })
            .min_by(|a, b| {
                (a.1 + padded_h)
                    .total_cmp(&(b.1 + padded_h))
                    .then(a.1.total_cmp(&b.1))
                    .then(a.0.total_cmp(&b.0))
            })
            .expect("candidate width is clamped to the widest padded rectangle");

        raise_skyline(&mut skyline, x, padded_w, y + padded_h);
        positions.push((x, y));
        used_w = used_w.max(x + width);
        used_h = used_h.max(y + height);
    }
    SkylineLayout {
        positions,
        bounds: (used_w, used_h),
    }
}

fn raise_skyline(skyline: &mut Vec<Segment>, x: f64, width: f64, height: f64) {
    let end = x + width;
    let mut next = Vec::with_capacity(skyline.len() + 2);
    let mut inserted = false;
    for segment in skyline.iter().copied() {
        if segment.end() <= x {
            next.push(segment);
            continue;
        }
        if segment.x >= end {
            if !inserted {
                next.push(Segment { x, width, height });
                inserted = true;
            }
            next.push(segment);
            continue;
        }
        if segment.x < x {
            next.push(Segment {
                x: segment.x,
                width: x - segment.x,
                height: segment.height,
            });
        }
        if !inserted {
            next.push(Segment { x, width, height });
            inserted = true;
        }
        if segment.end() > end {
            next.push(Segment {
                x: end,
                width: segment.end() - end,
                height: segment.height,
            });
        }
    }
    if !inserted {
        next.push(Segment { x, width, height });
    }

    let mut merged: Vec<Segment> = Vec::with_capacity(next.len());
    for segment in next {
        if let Some(last) = merged.last_mut() {
            if last.end() == segment.x && last.height == segment.height {
                last.width += segment.width;
                continue;
            }
        }
        merged.push(segment);
    }
    *skyline = merged;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_width_skyline_fills_a_right_hand_cavity() {
        let packed = pack_at_width(&[(6.0, 4.0), (4.0, 2.0), (4.0, 2.0)], 0.0, 10.0);
        assert_eq!(packed.positions, vec![(0.0, 0.0), (6.0, 0.0), (6.0, 2.0)]);
        assert_eq!(packed.bounds, (10.0, 4.0));
    }

    #[test]
    fn gap_separates_horizontal_and_vertical_neighbors() {
        let packed = pack_at_width(&[(6.0, 4.0), (4.0, 2.0), (4.0, 2.0)], 1.0, 12.0);
        assert_eq!(packed.positions[1], (7.0, 0.0));
        assert_eq!(packed.positions[2], (7.0, 3.0));
    }

    #[test]
    fn raise_skyline_splits_and_replaces_unequal_segments() {
        let mut skyline = vec![
            Segment {
                x: 0.0,
                width: 3.0,
                height: 1.0,
            },
            Segment {
                x: 3.0,
                width: 4.0,
                height: 2.0,
            },
            Segment {
                x: 7.0,
                width: 3.0,
                height: 1.0,
            },
        ];

        raise_skyline(&mut skyline, 2.0, 6.0, 5.0);

        assert_eq!(
            skyline,
            vec![
                Segment {
                    x: 0.0,
                    width: 2.0,
                    height: 1.0,
                },
                Segment {
                    x: 2.0,
                    width: 6.0,
                    height: 5.0,
                },
                Segment {
                    x: 8.0,
                    width: 2.0,
                    height: 1.0,
                },
            ]
        );
    }

    #[test]
    fn public_skyline_pack_is_repeatable_and_preserves_index_order() {
        let sizes = [(10.0, 8.0), (4.0, 2.0), (4.0, 2.0), (2.0, 6.0)];
        let first = skyline_pack(&sizes, 1.0, 1.6);
        let second = skyline_pack(&sizes, 1.0, 1.6);
        assert_eq!(first, second);
        assert_eq!(first.positions.len(), sizes.len());
    }

    #[test]
    fn single_item_keeps_natural_bounds() {
        assert_eq!(
            skyline_pack(&[(640.0, 58.0)], 8.0, 1.6),
            SkylineLayout {
                positions: vec![(0.0, 0.0)],
                bounds: (640.0, 58.0),
            }
        );
    }

    #[test]
    fn candidate_width_is_clamped_to_widest_padded_rectangle() {
        let packed = skyline_pack(&[(100.0, 1.0), (1.0, 1.0)], 5.0, 0.000_001);
        assert_eq!(packed.positions[0], (0.0, 0.0));
        assert!(packed.bounds.0 >= 100.0);
    }

    #[test]
    fn baseline_candidate_wins_equal_scores() {
        let sizes = [(4.0, 4.0), (4.0, 4.0)];
        let baseline = (32.0_f64).sqrt();
        assert_eq!(
            skyline_pack(&sizes, 0.0, 1.0),
            pack_at_width(&sizes, 0.0, baseline)
        );
    }

    #[test]
    fn large_layout_is_finite_contained_and_gap_separated() {
        let gap = 2.0;
        let sizes: Vec<_> = (0..512)
            .map(|index| {
                let width = 3.0 + ((index * 37) % 41) as f64;
                let height = 2.0 + ((index * 19) % 29) as f64;
                (width, height)
            })
            .collect();

        let packed = skyline_pack(&sizes, gap, 1.6);

        assert_eq!(packed.positions.len(), sizes.len());
        assert!(packed.bounds.0.is_finite());
        assert!(packed.bounds.1.is_finite());
        for (index, (&(x, y), &(width, height))) in packed.positions.iter().zip(&sizes).enumerate()
        {
            assert!(x.is_finite() && y.is_finite(), "item {index} is not finite");
            assert!(
                x >= 0.0 && y >= 0.0,
                "item {index} has negative coordinates"
            );
            assert!(
                x + width <= packed.bounds.0 && y + height <= packed.bounds.1,
                "item {index} lies outside the packed bounds"
            );
        }

        for left in 0..sizes.len() {
            let (left_x, left_y) = packed.positions[left];
            let (left_width, left_height) = sizes[left];
            for (right, &(right_width, right_height)) in sizes.iter().enumerate().skip(left + 1) {
                let (right_x, right_y) = packed.positions[right];
                let separated = left_x + left_width + gap <= right_x
                    || right_x + right_width + gap <= left_x
                    || left_y + left_height + gap <= right_y
                    || right_y + right_height + gap <= left_y;
                assert!(separated, "items {left} and {right} overlap their gap");
            }
        }
    }
}
