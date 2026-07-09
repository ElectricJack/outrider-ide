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

/// Default arrow-step framing: the focus band lands at half the viewport
/// height. The view's sticky step fraction resets to this on Home.
pub const FOCUS_FRACTION: f64 = 0.5;
/// End-key framing: the focus band fills the viewport. End makes this the
/// sticky step fraction, so subsequent arrow steps stay at Full.
pub const END_FRACTION: f64 = 0.95;
/// Camera-follow tween duration, seconds (spec: ~250 ms, interruptible).
pub const TWEEN_SECS: f64 = 0.25;

/// Camera showing world band (y, h) at `fraction` of the viewport height,
/// centered. The zoom clamp may prevent exact framing (accepted).
pub fn frame_band(y: f64, h: f64, vh: f64, fraction: f64, min_zoom: f64, max_zoom: f64) -> Camera {
    Camera {
        center_y: y + h / 2.0,
        zoom: (fraction * vh / h).clamp(min_zoom, max_zoom),
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
/// seconds. center_y interpolates linearly; zoom geometrically (log-space)
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

    #[test]
    fn frame_band_centers_at_fraction() {
        // b.rs::g worked example: band (0.6875, 0.015625), vh 600, fraction ½
        let c = frame_band(0.6875, 0.015625, 600.0, FOCUS_FRACTION, 1e-9, 1e18);
        close(c.zoom, 19200.0); // 0.5·600/0.015625
        close(c.center_y, 0.6953125);
        // clamp may prevent exact framing
        let c = frame_band(0.6875, 0.015625, 600.0, FOCUS_FRACTION, 1e-9, 100.0);
        close(c.zoom, 100.0);
    }

    #[test]
    fn tween_endpoints_exact_and_done() {
        let from = Camera { center_y: 0.0, zoom: 100.0 };
        let to = Camera { center_y: 1.0, zoom: 6400.0 };
        let tw = CameraTween::new(from, to);
        close(tw.duration, TWEEN_SECS);
        assert_eq!(tw.sample(0.0), from);
        assert_eq!(tw.sample(TWEEN_SECS), to); // exact, not approximate
        assert_eq!(tw.sample(TWEEN_SECS * 2.0), to);
        assert!(!tw.done(TWEEN_SECS - 1e-6));
        assert!(tw.done(TWEEN_SECS));
    }

    #[test]
    fn tween_midpoint_linear_y_geometric_zoom() {
        let from = Camera { center_y: 0.0, zoom: 100.0 };
        let to = Camera { center_y: 1.0, zoom: 6400.0 };
        let tw = CameraTween::new(from, to);
        let mid = tw.sample(TWEEN_SECS / 2.0); // ease(½) = ½
        close(mid.center_y, 0.5);
        close(mid.zoom, 800.0); // √(100·6400)
    }

    #[test]
    fn tween_monotonic() {
        let from = Camera { center_y: 0.0, zoom: 100.0 };
        let to = Camera { center_y: 1.0, zoom: 6400.0 };
        let tw = CameraTween::new(from, to);
        let mut last = tw.sample(0.0);
        for i in 1..=100 {
            let c = tw.sample(TWEEN_SECS * i as f64 / 100.0);
            assert!(c.center_y >= last.center_y - 1e-12);
            assert!(c.zoom >= last.zoom - 1e-9);
            last = c;
        }
    }

    #[test]
    fn retarget_is_continuous() {
        let tw = CameraTween::new(
            Camera { center_y: 0.0, zoom: 100.0 },
            Camera { center_y: 1.0, zoom: 6400.0 },
        );
        let other = Camera { center_y: -3.0, zoom: 50.0 };
        let t = 0.1;
        let re = tw.retarget(t, other);
        assert_eq!(re.sample(0.0), tw.sample(t)); // no jump at the splice
        assert_eq!(re.to, other);
    }
}
