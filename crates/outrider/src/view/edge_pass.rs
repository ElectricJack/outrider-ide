//! Edge aggregation: resolved edge layers -> screen-space paint instructions.

use std::collections::HashMap;

use outrider_index::SymbolId;
use outrider_layout::PackLayout;
use outrider_view::layers::edges::ResolvedEdgeLayer;
use outrider_view::spec::EdgeStyle;

use crate::camera::Camera;

/// Maximum number of edges to keep after deduplication.
const MAX_EDGES: usize = 2000;

/// Screen-space data for one edge curve.
pub(crate) struct EdgePaint {
    pub(crate) from: (f32, f32),
    pub(crate) to: (f32, f32),
    pub(crate) color: u32,
    pub(crate) weight: f32,
    pub(crate) dashed: bool,
}

/// All edges for one resolved view frame.
pub(crate) struct EdgeFrame {
    pub(crate) edges: Vec<EdgePaint>,
}

/// Aggregate resolved edge layers into an `EdgeFrame` of screen-space paint
/// instructions. Edges whose endpoints are not present in the layout are
/// skipped; self-loops (both endpoints resolve to the same rect) are skipped;
/// duplicates are deduplicated by keeping the highest-weight edge; and the
/// result is capped at `MAX_EDGES`.
pub(crate) fn aggregate(
    resolved: &[ResolvedEdgeLayer],
    layout: &PackLayout,
    camera: &Camera,
    vw: f64,
    vh: f64,
) -> EdgeFrame {
    // Collect (from_id, to_id) -> best edge info, deduplicated by max weight.
    let mut best: HashMap<(SymbolId, SymbolId), (f64, u32, bool)> = HashMap::new();

    for layer in resolved {
        let color = parse_color(&layer.color);
        let dashed = layer.style == EdgeStyle::Dashed;

        for edge in &layer.edges {
            // Both endpoints must be present in the layout.
            if layout.rects.get(&edge.from).is_none() || layout.rects.get(&edge.to).is_none() {
                continue;
            }
            // Skip self-loops.
            if edge.from == edge.to {
                continue;
            }

            let key = (edge.from.clone(), edge.to.clone());
            let entry = best.entry(key).or_insert((f64::MIN, color, dashed));
            if edge.weight > entry.0 {
                *entry = (edge.weight, color, dashed);
            }
        }
    }

    // Collect into a Vec and cap at MAX_EDGES (sorted by weight descending).
    let mut entries: Vec<_> = best.into_iter().collect();
    if entries.len() > MAX_EDGES {
        entries.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap_or(std::cmp::Ordering::Equal));
        entries.truncate(MAX_EDGES);
    }

    // Convert to screen-space EdgePaint.
    let edges = entries
        .into_iter()
        .filter_map(|((from_id, to_id), (weight, color, dashed))| {
            let from_rect = layout.rects.get(&from_id)?;
            let to_rect = layout.rects.get(&to_id)?;

            let from_cx = from_rect.x + from_rect.w / 2.0;
            let from_cy = from_rect.y + from_rect.h / 2.0;
            let to_cx = to_rect.x + to_rect.w / 2.0;
            let to_cy = to_rect.y + to_rect.h / 2.0;

            // Clip the center-to-center segment to each rect's border so
            // the line starts and ends at the box edges, not the centers.
            let (dx, dy) = (to_cx - from_cx, to_cy - from_cy);
            let (wx1, wy1) = exit_point(from_rect, from_cx, from_cy, dx, dy);
            let (wx2, wy2) = exit_point(to_rect, to_cx, to_cy, -dx, -dy);
            // If the clipped segment inverted (overlapping/nested rects),
            // fall back to the raw centers.
            let ((wx1, wy1), (wx2, wy2)) =
                if (wx2 - wx1) * dx + (wy2 - wy1) * dy > 0.0 {
                    ((wx1, wy1), (wx2, wy2))
                } else {
                    ((from_cx, from_cy), (to_cx, to_cy))
                };

            let (sx1, sy1) = camera.world_to_screen(wx1, wy1, vw, vh);
            let (sx2, sy2) = camera.world_to_screen(wx2, wy2, vw, vh);

            let w = (weight as f32).clamp(0.0, 1.0);

            Some(EdgePaint {
                from: (sx1 as f32, sy1 as f32),
                to: (sx2 as f32, sy2 as f32),
                color,
                weight: w,
                dashed,
            })
        })
        .collect();

    EdgeFrame { edges }
}

/// Point where the ray from (cx, cy) along (dx, dy) exits rect `r`.
/// Returns the origin when the direction is zero.
fn exit_point(r: &outrider_layout::Rect, cx: f64, cy: f64, dx: f64, dy: f64) -> (f64, f64) {
    let mut t = f64::INFINITY;
    if dx > 0.0 {
        t = t.min((r.x + r.w - cx) / dx);
    } else if dx < 0.0 {
        t = t.min((r.x - cx) / dx);
    }
    if dy > 0.0 {
        t = t.min((r.y + r.h - cy) / dy);
    } else if dy < 0.0 {
        t = t.min((r.y - cy) / dy);
    }
    if !t.is_finite() || t < 0.0 {
        return (cx, cy);
    }
    (cx + dx * t, cy + dy * t)
}

/// Parse a color string (e.g. "#8a8a8a" or "0xRRGGBB") into a u32.
/// Falls back to 0x8a8a8a on parse failure.
fn parse_color(s: &str) -> u32 {
    let stripped = s
        .trim_start_matches('#')
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    u32::from_str_radix(stripped, 16).unwrap_or(0x8a8a8a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_color() {
        assert_eq!(parse_color("#ff0000"), 0xff0000);
        assert_eq!(parse_color("0xFF00FF"), 0xFF00FF);
        assert_eq!(parse_color("8a8a8a"), 0x8a8a8a);
        assert_eq!(parse_color("not-a-color"), 0x8a8a8a);
    }
}
