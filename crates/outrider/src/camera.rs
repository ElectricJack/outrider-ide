use outrider_layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World point at the viewport center. World units are natural pixels
    /// (zoom 1.0 = code at natural size).
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

    /// Multiply zoom by `factor`, keeping the world point under screen
    /// (sx, sy) fixed.
    #[allow(clippy::too_many_arguments)]
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

    /// Home: `rect` fits the viewport with a 5% margin.
    pub fn fit(rect: Rect, vw: f64, vh: f64) -> Camera {
        Camera {
            center_x: rect.x + rect.w / 2.0,
            center_y: rect.y + rect.h / 2.0,
            zoom: (vw / rect.w).min(vh / rect.h) / 1.05,
        }
    }

}

/// Enter/Esc framing for containers: the focus rect lands at half the
/// viewport's tighter dimension.
pub const FOCUS_FRACTION: f64 = 0.5;
/// End-key framing: the focus rect fills the viewport.
pub const END_FRACTION: f64 = 0.95;
/// Camera-follow tween duration, seconds (spec: ~250 ms, interruptible).
pub const TWEEN_SECS: f64 = 0.25;
/// World units are natural pixels; 8× natural size is as far as zoom goes.
pub const MAX_ZOOM: f64 = 8.0;

/// Camera showing `rect` at `fraction` of the viewport's tighter
/// dimension, centered. The zoom clamp may prevent exact framing (accepted).
pub fn frame_rect(
    rect: Rect,
    vw: f64,
    vh: f64,
    fraction: f64,
    min_zoom: f64,
    max_zoom: f64,
) -> Camera {
    Camera {
        center_x: rect.x + rect.w / 2.0,
        center_y: rect.y + rect.h / 2.0,
        zoom: (fraction * (vw / rect.w).min(vh / rect.h)).clamp(min_zoom, max_zoom),
    }
}

/// Leaf framing: END_FRACTION fit, capped at natural size (zoom 1.0) —
/// stepping onto a small method never blows its code up past 12px.
pub fn frame_page(rect: Rect, vw: f64, vh: f64, min_zoom: f64, max_zoom: f64) -> Camera {
    Camera {
        center_x: rect.x + rect.w / 2.0,
        center_y: rect.y + rect.h / 2.0,
        zoom: (END_FRACTION * (vw / rect.w).min(vh / rect.h))
            .min(1.0)
            .clamp(min_zoom, max_zoom),
    }
}

fn ease_in_out_cubic(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Eased camera animation, pure and clock-free: the caller supplies elapsed
/// seconds. Centers interpolate linearly; zoom geometrically (log-space)
/// so zoom speed feels uniform across octaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraTween {
    pub from: Camera,
    pub to: Camera,
    pub duration: f64,
}

impl CameraTween {
    pub fn new(from: Camera, to: Camera) -> Self {
        CameraTween { from, to, duration: TWEEN_SECS }
    }

    pub fn sample(&self, t: f64) -> Camera {
        if t >= self.duration {
            return self.to;
        }
        let e = ease_in_out_cubic((t / self.duration).max(0.0));
        Camera {
            center_x: self.from.center_x + (self.to.center_x - self.from.center_x) * e,
            center_y: self.from.center_y + (self.to.center_y - self.from.center_y) * e,
            zoom: self.from.zoom * (self.to.zoom / self.from.zoom).powf(e),
        }
    }

    pub fn done(&self, t: f64) -> bool {
        t >= self.duration
    }

    /// Retarget mid-flight: the new tween starts from the current sample,
    /// so motion is continuous — never restarted from the old origin.
    pub fn retarget(&self, t: f64, to: Camera) -> CameraTween {
        CameraTween::new(self.sample(t), to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn screen_world_round_trip_2d() {
        let c = Camera { center_x: 100.0, center_y: 50.0, zoom: 2.0 };
        let (sx, sy) = c.world_to_screen(150.0, 75.0, 800.0, 600.0);
        close(sx, 500.0); // (150-100)·2 + 400
        close(sy, 350.0); // (75-50)·2 + 300
        let (wx, wy) = c.screen_to_world(sx, sy, 800.0, 600.0);
        close(wx, 150.0);
        close(wy, 75.0);
    }

    #[test]
    fn pan_moves_center_against_drag() {
        let mut c = Camera { center_x: 100.0, center_y: 50.0, zoom: 2.0 };
        c.pan(-4.0, 6.0);
        close(c.center_x, 102.0); // 100 - (-4)/2
        close(c.center_y, 47.0); // 50 - 6/2
    }

    #[test]
    fn zoom_about_fixes_cursor_point() {
        for &(sx, sy) in &[(0.0, 0.0), (400.0, 300.0), (799.0, 1.0), (123.0, 456.0)] {
            for &f in &[0.5, 0.9, 1.1, 2.0, 7.3] {
                let mut c = Camera { center_x: 40.0, center_y: 700.0, zoom: 0.7 };
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
        let mut c = Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 };
        c.zoom_about(400.0, 300.0, 800.0, 600.0, 1e9, 0.5, 4.0);
        close(c.zoom, 4.0);
        c.zoom_about(400.0, 300.0, 800.0, 600.0, 1e-9, 0.5, 4.0);
        close(c.zoom, 0.5);
    }

    #[test]
    fn fit_centers_with_margin() {
        // the Task 1 worked-example root: 1000 × 1639.2 in 800 × 600 —
        // height is the tight side
        let r = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 1639.2 };
        let c = Camera::fit(r, 800.0, 600.0);
        close(c.center_x, 500.0);
        close(c.center_y, 819.6);
        close(c.zoom, 600.0 / 1639.2 / 1.05);
        // framed rect fits the viewport in both axes
        assert!(c.zoom * r.w <= 800.0 + 1e-9);
        assert!(c.zoom * r.h <= 600.0 + 1e-9);
    }

    #[test]
    fn frame_rect_uses_tighter_dimension() {
        // g's page from the Task 1 worked example: width is the tight side
        let r = Rect { x: 504.0, y: 264.0, w: 480.0, h: 58.0 };
        let c = frame_rect(r, 800.0, 600.0, FOCUS_FRACTION, 1e-9, 1e18);
        close(c.center_x, 744.0);
        close(c.center_y, 293.0);
        close(c.zoom, 0.5 * 800.0 / 480.0);
        // clamp may prevent exact framing
        let c = frame_rect(r, 800.0, 600.0, FOCUS_FRACTION, 1e-9, 0.3);
        close(c.zoom, 0.3);
    }

    #[test]
    fn frame_page_caps_at_natural_size() {
        // small page: END framing would be 0.95·800/480 ≈ 1.58 → capped at 1.0
        let small = Rect { x: 504.0, y: 264.0, w: 480.0, h: 58.0 };
        let c = frame_page(small, 800.0, 600.0, 1e-9, 1e18);
        close(c.zoom, 1.0);
        close(c.center_x, 744.0);
        close(c.center_y, 293.0);
        // tall page: fit dominates — 0.95·600/1602.4, below the 1.0 cap
        let tall = Rect { x: 8.0, y: 28.8, w: 480.0, h: 1602.4 };
        let c = frame_page(tall, 800.0, 600.0, 1e-9, 1e18);
        close(c.zoom, 0.95 * 600.0 / 1602.4);
        // clamp still applies
        let c = frame_page(small, 800.0, 600.0, 2.0, 4.0);
        close(c.zoom, 2.0);
    }

    #[test]
    fn tween_endpoints_exact_and_done() {
        let from = Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 };
        let to = Camera { center_x: 300.0, center_y: 900.0, zoom: 64.0 };
        let tw = CameraTween::new(from, to);
        close(tw.duration, TWEEN_SECS);
        assert_eq!(tw.sample(0.0), from);
        assert_eq!(tw.sample(TWEEN_SECS), to); // exact, not approximate
        assert_eq!(tw.sample(TWEEN_SECS * 2.0), to);
        assert!(!tw.done(TWEEN_SECS - 1e-6));
        assert!(tw.done(TWEEN_SECS));
    }

    #[test]
    fn tween_midpoint_linear_centers_geometric_zoom() {
        let from = Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 };
        let to = Camera { center_x: 300.0, center_y: 900.0, zoom: 64.0 };
        let tw = CameraTween::new(from, to);
        let mid = tw.sample(TWEEN_SECS / 2.0); // ease(½) = ½
        close(mid.center_x, 150.0);
        close(mid.center_y, 450.0);
        close(mid.zoom, 8.0); // √(1·64)
    }

    #[test]
    fn retarget_is_continuous() {
        let tw = CameraTween::new(
            Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 },
            Camera { center_x: 300.0, center_y: 900.0, zoom: 64.0 },
        );
        let other = Camera { center_x: -3.0, center_y: 7.0, zoom: 0.5 };
        let t = 0.1;
        let re = tw.retarget(t, other);
        assert_eq!(re.sample(0.0), tw.sample(t)); // no jump at the splice
        assert_eq!(re.to, other);
    }

}
