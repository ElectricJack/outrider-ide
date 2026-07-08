#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World point at the viewport center.
    pub center_x: f64,
    pub center_y: f64,
    /// Pixels per world unit.
    pub zoom: f64,
}

impl Camera {
    pub fn world_to_screen(&self, wx: f64, wy: f64, vw: f64, vh: f64) -> (f64, f64) {
        (
            (wx - self.center_x) * self.zoom + vw / 2.0,
            (wy - self.center_y) * self.zoom + vh / 2.0,
        )
    }

    pub fn screen_to_world(&self, sx: f64, sy: f64, vw: f64, vh: f64) -> (f64, f64) {
        (
            (sx - vw / 2.0) / self.zoom + self.center_x,
            (sy - vh / 2.0) / self.zoom + self.center_y,
        )
    }

    /// Drag by (dx, dy) pixels: content follows the cursor.
    pub fn pan(&mut self, dx_px: f64, dy_px: f64) {
        self.center_x -= dx_px / self.zoom;
        self.center_y -= dy_px / self.zoom;
    }

    /// Multiply zoom by `factor`, keeping the world point under (sx, sy) fixed.
    pub fn zoom_about(
        &mut self,
        sx: f64,
        sy: f64,
        vw: f64,
        vh: f64,
        factor: f64,
        min_zoom: f64,
        max_zoom: f64,
    ) {
        let (wx, wy) = self.screen_to_world(sx, sy, vw, vh);
        self.zoom = (self.zoom * factor).clamp(min_zoom, max_zoom);
        self.center_x = wx - (sx - vw / 2.0) / self.zoom;
        self.center_y = wy - (sy - vh / 2.0) / self.zoom;
    }

    /// Frame a world extent of (world_w x world_h) with a 5% margin.
    pub fn frame(world_w: f64, world_h: f64, vw: f64, vh: f64) -> Camera {
        let zoom = (vw / (world_w * 1.05)).min(vh / (world_h * 1.05));
        Camera { center_x: world_w / 2.0, center_y: world_h / 2.0, zoom }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn screen_world_round_trip() {
        let c = Camera { center_x: 1.5, center_y: 0.5, zoom: 200.0 };
        let (sx, sy) = c.world_to_screen(2.0, 0.75, 800.0, 600.0);
        close(sx, 500.0); // (2.0-1.5)*200 + 400
        close(sy, 350.0); // (0.75-0.5)*200 + 300
        let (wx, wy) = c.screen_to_world(sx, sy, 800.0, 600.0);
        close(wx, 2.0);
        close(wy, 0.75);
    }

    #[test]
    fn pan_moves_center_against_drag() {
        let mut c = Camera { center_x: 1.0, center_y: 1.0, zoom: 2.0 };
        c.pan(10.0, -4.0); // dragging content right/up moves center left/down
        close(c.center_x, -4.0); // 1.0 - 10.0/2.0
        close(c.center_y, 3.0);  // 1.0 - (-4.0)/2.0
    }

    #[test]
    fn zoom_about_fixes_cursor_point() {
        for &(sx, sy) in &[(0.0, 0.0), (400.0, 300.0), (799.0, 599.0), (123.0, 456.0)] {
            for &f in &[0.5, 0.9, 1.1, 2.0, 7.3] {
                let mut c = Camera { center_x: 1.7, center_y: 0.4, zoom: 222.0 };
                let before = c.screen_to_world(sx, sy, 800.0, 600.0);
                c.zoom_about(sx, sy, 800.0, 600.0, f, 1e-9, 1e18);
                let after = c.screen_to_world(sx, sy, 800.0, 600.0);
                close(before.0, after.0);
                close(before.1, after.1);
            }
        }
    }

    #[test]
    fn zoom_about_clamps() {
        let mut c = Camera { center_x: 0.0, center_y: 0.0, zoom: 100.0 };
        c.zoom_about(400.0, 300.0, 800.0, 600.0, 1e9, 50.0, 400.0);
        close(c.zoom, 400.0);
        c.zoom_about(400.0, 300.0, 800.0, 600.0, 1e-9, 50.0, 400.0);
        close(c.zoom, 50.0);
    }

    #[test]
    fn frame_fits_world_with_margin() {
        // world 24/7 x 1.0 in an 800x600 viewport
        let c = Camera::frame(24.0 / 7.0, 1.0, 800.0, 600.0);
        close(c.center_x, 12.0 / 7.0);
        close(c.center_y, 0.5);
        close(c.zoom, 800.0 / ((24.0 / 7.0) * 1.05)); // width-limited here
        // framed world fits the viewport
        assert!((24.0 / 7.0) * c.zoom <= 800.0 + 1e-9);
        assert!(1.0 * c.zoom <= 600.0 + 1e-9);
    }
}
