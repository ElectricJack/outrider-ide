#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World y at the viewport's vertical center. X is not a camera concern:
    /// the column stack is left-anchored and fully determined by zoom.
    pub center_y: f64,
    /// Pixels per world unit.
    pub zoom: f64,
}

impl Camera {
    pub fn world_to_screen_y(&self, wy: f64, vh: f64) -> f64 {
        (wy - self.center_y) * self.zoom + vh / 2.0
    }

    pub fn screen_to_world_y(&self, sy: f64, vh: f64) -> f64 {
        (sy - vh / 2.0) / self.zoom + self.center_y
    }

    /// Drag by dy pixels: content follows the cursor. Horizontal drag is ignored.
    pub fn pan(&mut self, dy_px: f64) {
        self.center_y -= dy_px / self.zoom;
    }

    /// Multiply zoom by `factor`, keeping the world y under screen `sy` fixed.
    pub fn zoom_about(&mut self, sy: f64, vh: f64, factor: f64, min_zoom: f64, max_zoom: f64) {
        let wy = self.screen_to_world_y(sy, vh);
        self.zoom = (self.zoom * factor).clamp(min_zoom, max_zoom);
        self.center_y = wy - (sy - vh / 2.0) / self.zoom;
    }

    /// Frame a world height with a 5% margin (Home).
    pub fn frame(world_h: f64, vh: f64) -> Camera {
        Camera { center_y: world_h / 2.0, zoom: vh / (world_h * 1.05) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn screen_world_round_trip_y() {
        let c = Camera { center_y: 0.5, zoom: 200.0 };
        let sy = c.world_to_screen_y(0.75, 600.0);
        close(sy, 350.0); // (0.75-0.5)*200 + 300
        close(c.screen_to_world_y(sy, 600.0), 0.75);
    }

    #[test]
    fn pan_moves_center_against_drag() {
        let mut c = Camera { center_y: 1.0, zoom: 2.0 };
        c.pan(-4.0); // dragging content up moves center down
        close(c.center_y, 3.0); // 1.0 - (-4.0)/2.0
    }

    #[test]
    fn zoom_about_fixes_cursor_y() {
        for &sy in &[0.0, 300.0, 599.0, 456.0] {
            for &f in &[0.5, 0.9, 1.1, 2.0, 7.3] {
                let mut c = Camera { center_y: 0.4, zoom: 222.0 };
                let before = c.screen_to_world_y(sy, 600.0);
                c.zoom_about(sy, 600.0, f, 1e-9, 1e18);
                close(before, c.screen_to_world_y(sy, 600.0));
            }
        }
    }

    #[test]
    fn zoom_about_clamps() {
        let mut c = Camera { center_y: 0.0, zoom: 100.0 };
        c.zoom_about(300.0, 600.0, 1e9, 50.0, 400.0);
        close(c.zoom, 400.0);
        c.zoom_about(300.0, 600.0, 1e-9, 50.0, 400.0);
        close(c.zoom, 50.0);
    }

    #[test]
    fn frame_fits_height_with_margin() {
        let c = Camera::frame(1.0, 600.0);
        close(c.center_y, 0.5);
        close(c.zoom, 600.0 / 1.05);
        assert!(c.zoom * 1.0 <= 600.0 + 1e-9); // framed band fits the viewport
    }
}
