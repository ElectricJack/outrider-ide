use outrider_layout::RATIO;

pub const CELL_ASPECT: f64 = 3.0;
pub const MERGE_PX: f64 = 4.0;
pub const LABEL_PX: f64 = 20.0;
pub const CARD_PX: f64 = 80.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 8^-depth: the size scale of level-`depth` cells relative to level 0.
pub fn column_scale(depth: u8) -> f64 {
    (RATIO as f64).powi(-(depth as i32))
}

/// X_d = CELL_ASPECT * (1 - 8^-d) * 8/7 — where the depth-d column begins.
pub fn column_x(depth: u8) -> f64 {
    let r = RATIO as f64;
    CELL_ASPECT * (1.0 - column_scale(depth)) * r / (r - 1.0)
}

/// Total world width: the columns converge to CELL_ASPECT * 8/7.
pub fn world_width() -> f64 {
    let r = RATIO as f64;
    CELL_ASPECT * r / (r - 1.0)
}

pub fn node_world_rect(depth: u8, abs_start: f64, len: u64) -> WorldRect {
    let s = column_scale(depth);
    WorldRect {
        x: column_x(depth),
        y: abs_start * s,
        w: CELL_ASPECT * s,
        h: len as f64 * s,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    Dot,
    Label,
    Card,
}

pub fn rung_for_px_height(h: f64) -> Option<Rung> {
    if h < MERGE_PX {
        None
    } else if h < LABEL_PX {
        Some(Rung::Dot)
    } else if h < CARD_PX {
        Some(Rung::Label)
    } else {
        Some(Rung::Card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn column_geometry() {
        close(column_scale(0), 1.0);
        close(column_scale(1), 0.125);
        close(column_scale(2), 0.015625);
        close(column_x(0), 0.0);
        close(column_x(1), 3.0);
        close(column_x(2), 3.375);
        close(world_width(), 24.0 / 7.0);
    }

    #[test]
    fn worked_example_rects() {
        // root {0,0,1}
        let r = node_world_rect(0, 0.0, 1);
        close(r.x, 0.0);
        close(r.y, 0.0);
        close(r.w, 3.0);
        close(r.h, 1.0);
        // b.rs::g — depth 2, abs cell 44, len 1 (Phase 2 worked example)
        let g = node_world_rect(2, 44.0, 1);
        close(g.x, 3.375);
        close(g.y, 0.6875);
        close(g.w, 0.046875);
        close(g.h, 0.015625);
    }

    #[test]
    fn rung_thresholds() {
        assert_eq!(rung_for_px_height(3.9), None);
        assert_eq!(rung_for_px_height(4.0), Some(Rung::Dot));
        assert_eq!(rung_for_px_height(19.9), Some(Rung::Dot));
        assert_eq!(rung_for_px_height(20.0), Some(Rung::Label));
        assert_eq!(rung_for_px_height(79.9), Some(Rung::Label));
        assert_eq!(rung_for_px_height(80.0), Some(Rung::Card));
        assert_eq!(rung_for_px_height(100_000.0), Some(Rung::Card));
    }
}
