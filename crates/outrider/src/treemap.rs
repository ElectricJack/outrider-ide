use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, size, App, Bounds, BorderStyle, Context,
    FocusHandle, Pixels, TextAlign, TextRun, Window,
};
use outrider_index::SymbolTree;
use outrider_layout::WorldLayout;

use crate::camera::{self, Camera, CameraTween};
use crate::focus::{Focus, TreeIndex};
use crate::theme;
use crate::world::{self, Rung};

/// Approximate char budget for a column `w_px` wide at `font_px` monospace.
/// 0.62 ≈ advance-width/em for common monospace faces; exactness is not
/// required — worst case the ellipsis lands a character early.
fn truncate_to_width(name: &str, w_px: f32, font_px: f32) -> Option<String> {
    // Add a small epsilon before flooring to absorb f32 rounding at exact integer
    // boundaries (e.g. 10.0 * 0.62 * 12.0 in f32 is 9.999999 without it).
    let budget = ((w_px - 12.0) / (font_px * 0.62) + 1e-6).floor() as isize;
    if budget < 2 {
        return None; // no room for any text
    }
    let budget = budget as usize;
    if name.chars().count() <= budget {
        Some(name.to_string())
    } else {
        let cut: String = name.chars().take(budget - 1).collect();
        Some(format!("{cut}…"))
    }
}

pub struct TreemapView {
    tree: SymbolTree,
    layout: WorldLayout,
    /// None until the first render supplies a viewport; then Home-framed.
    camera: Option<Camera>,
    home_zoom: f64,
    drag_last: Option<gpui::Point<Pixels>>,
    press_origin: Option<gpui::Point<Pixels>>,
    focus: Focus,
    tween: Option<(CameraTween, std::time::Instant)>,
    focus_handle: FocusHandle,
}

/// Owned, GPUI-free paint instruction — built in render (which may borrow
/// self), moved into the 'static canvas closure.
struct PaintItem {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fill: u32,
    border: u32,
    focused: bool,
    rung: Rung,
    name: String,
    meta: String,
}

impl TreemapView {
    pub fn new(tree: SymbolTree, layout: WorldLayout, cx: &mut Context<Self>) -> Self {
        let root_id = tree.root.id.clone();
        Self {
            tree,
            layout,
            camera: None,
            home_zoom: 1.0,
            drag_last: None,
            press_origin: None,
            focus: Focus::new(root_id),
            tween: None,
            focus_handle: cx.focus_handle(),
        }
    }

    fn root_world_height(&self) -> f64 {
        self.layout
            .nodes
            .get(&self.tree.root.id)
            .map(|nl| nl.cells.len as f64)
            .unwrap_or(1.0)
    }

    fn home_camera(&self, vh: f64) -> Camera {
        Camera::frame(self.root_world_height(), vh)
    }

    /// Start (or retarget) the camera-follow tween from the current sample.
    /// Retargeting goes through CameraTween::retarget, whose continuity is
    /// unit-tested (spec §7 item 7): from == sampled camera by construction.
    fn start_tween(&mut self, to: Camera) {
        let tw = match self.tween.take() {
            Some((tw, started)) => tw.retarget(started.elapsed().as_secs_f64(), to),
            None => match self.camera {
                Some(c) => CameraTween::new(c, to),
                None => return, // no viewport yet; ignore keys until first render
            },
        };
        self.camera = Some(tw.from);
        self.tween = Some((tw, std::time::Instant::now()));
    }

    /// Mouse is free (spec §4): manual camera ops drop any live tween,
    /// continuing from the current sampled state.
    fn cancel_tween(&mut self) {
        if let Some((tw, started)) = self.tween.take() {
            self.camera = Some(tw.sample(started.elapsed().as_secs_f64()));
        }
    }

    fn paint_items(&mut self, vw: f64, vh: f64) -> Vec<PaintItem> {
        if let Some((tw, started)) = self.tween {
            let t = started.elapsed().as_secs_f64();
            self.camera = Some(tw.sample(t));
            if tw.done(t) {
                self.tween = None;
            }
        }
        if self.camera.is_none() {
            let c = self.home_camera(vh);
            self.home_zoom = c.zoom;
            self.camera = Some(c);
        }
        let camera = *self.camera.as_ref().unwrap();
        let focus_id = self.focus.current.clone();
        world::visible_nodes(&self.tree, &self.layout, &camera, vw, vh)
            .into_iter()
            .map(|item| {
                let f = theme::churn_fill(item.node.churn);
                PaintItem {
                    x: item.px.x as f32,
                    y: item.px.y as f32,
                    w: item.px.w as f32,
                    h: item.px.h as f32,
                    fill: f,
                    border: theme::border_for(f),
                    focused: item.node.id == focus_id,
                    rung: item.rung,
                    name: item.node.name.clone(),
                    meta: format!(
                        "{} · p{:.0} · {}L",
                        item.node.churn_count,
                        item.node.churn * 100.0,
                        item.node.measure
                    ),
                }
            })
            .collect()
    }
}

impl Render for TreemapView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window, cx);
        }

        let vp = window.viewport_size();
        let (vw, vh) = (f64::from(vp.width), f64::from(vp.height));
        let items = self.paint_items(vw, vh);

        if self.tween.is_some() {
            window.request_animation_frame();
        }

        let max_zoom = vh * 8f64.powi(15);
        let min_zoom = self.home_zoom * 0.5;

        div()
            .size_full()
            .bg(rgb(theme::BG))
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, _w, _cx| {
                    this.drag_last = Some(e.position);
                    this.press_origin = Some(e.position);
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseUpEvent, w, cx| {
                    this.drag_last = None;
                    let Some(origin) = this.press_origin.take() else { return };
                    let slop = f64::from(e.position.x - origin.x)
                        .abs()
                        .max(f64::from(e.position.y - origin.y).abs());
                    if slop > 4.0 {
                        return; // drag, not click
                    }
                    let Some(cam) = this.camera else { return };
                    let vp = w.viewport_size();
                    let items = world::visible_nodes(
                        &this.tree,
                        &this.layout,
                        &cam,
                        f64::from(vp.width),
                        f64::from(vp.height),
                    );
                    // view fills the window, so window coords == canvas coords
                    let (mx, my) = (f64::from(e.position.x), f64::from(e.position.y));
                    let hit = world::hit_test(&items, mx, my).map(|i| i.node.id.clone());
                    drop(items);
                    if let Some(id) = hit {
                        let index = TreeIndex::new(&this.tree);
                        // click sets focus; camera does NOT move (spec §2)
                        this.focus.set(id, &index);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _w, cx| {
                if e.pressed_button != Some(gpui::MouseButton::Left) {
                    return;
                }
                let Some(last) = this.drag_last else { return };
                this.cancel_tween();
                let dy = f64::from(e.position.y - last.y);
                if let Some(cam) = this.camera.as_mut() {
                    cam.pan(dy);
                }
                this.drag_last = Some(e.position);
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(move |this, e: &gpui::ScrollWheelEvent, w, cx| {
                this.cancel_tween();
                let dy = match e.delta {
                    gpui::ScrollDelta::Pixels(p) => f64::from(p.y),
                    gpui::ScrollDelta::Lines(l) => l.y as f64 * 40.0,
                };
                let vh = f64::from(w.viewport_size().height);
                if let Some(cam) = this.camera.as_mut() {
                    // scroll up (positive dy) zooms in; flip the sign here if
                    // manual testing shows it inverted on this platform
                    let factor = (dy * 0.002).exp();
                    cam.zoom_about(f64::from(e.position.y), vh, factor, min_zoom, max_zoom);
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, w, cx| {
                if this.camera.is_none() {
                    return;
                }
                let vh = f64::from(w.viewport_size().height);
                let max_zoom = vh * 8f64.powi(15);
                let min_zoom = this.home_zoom * 0.5;
                let index = TreeIndex::new(&this.tree);
                let key = e.keystroke.key.as_str();
                let moved = match key {
                    "right" => this.focus.step_in(&index),
                    "left" => this.focus.step_back(&index),
                    "up" => this.focus.step_sibling(-1, &index),
                    "down" => this.focus.step_sibling(1, &index),
                    _ => false,
                };
                let target = match key {
                    "right" | "left" | "up" | "down" => {
                        if !moved {
                            return;
                        }
                        world::world_band(&this.focus.current, &this.layout).map(|(y, h)| {
                            camera::frame_band(y, h, vh, camera::FOCUS_FRACTION, min_zoom, max_zoom)
                        })
                    }
                    "end" => world::world_band(&this.focus.current, &this.layout).map(|(y, h)| {
                        camera::frame_band(y, h, vh, camera::END_FRACTION, min_zoom, max_zoom)
                    }),
                    "home" => {
                        let c = this.home_camera(vh);
                        this.home_zoom = c.zoom;
                        Some(c)
                    }
                    _ => return, // Tab included: explicitly no handler
                };
                if let Some(to) = target {
                    this.start_tween(to);
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    |_bounds, _window, _cx: &mut App| {},
                    move |bounds, _prepaint, window, _cx: &mut App| {
                        let origin = bounds.origin;
                        for item in &items {
                            let b = Bounds::new(
                                point(origin.x + px(item.x), origin.y + px(item.y)),
                                size(px(item.w), px(item.h)),
                            );
                            let (bw, bc) = if item.focused {
                                (2.0, theme::FOCUS_BORDER)
                            } else {
                                (1.0, item.border)
                            };
                            window.paint_quad(quad(
                                b,
                                px(0.),
                                rgb(item.fill),
                                px(bw),
                                rgb(bc),
                                BorderStyle::default(),
                            ));
                            if item.rung == Rung::Dot || item.h < 14.0 {
                                continue;
                            }
                            let font_px = 12.0_f32;
                            let line_height = px(font_px * 1.3);
                            let run = |len: usize, color: u32| TextRun {
                                len,
                                font: gpui::font(theme::FONT_FAMILY),
                                color: rgb(color).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            if let Some(name) = truncate_to_width(&item.name, item.w, font_px) {
                                let line = window.text_system().shape_line(
                                    name.clone().into(),
                                    px(font_px),
                                    &[run(name.len(), theme::TEXT_PRIMARY)],
                                    None,
                                );
                                let ty = if item.rung == Rung::Label {
                                    // vertically centered in the box
                                    item.y + (item.h - font_px * 1.3) / 2.0
                                } else {
                                    item.y + 4.0
                                };
                                let _ = line.paint(
                                    point(origin.x + px(item.x + 6.0), origin.y + px(ty)),
                                    line_height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
                            }
                            if item.rung == Rung::Card {
                                if let Some(meta) = truncate_to_width(&item.meta, item.w, font_px) {
                                    let line = window.text_system().shape_line(
                                        meta.clone().into(),
                                        px(font_px),
                                        &[run(meta.len(), theme::TEXT_SECONDARY)],
                                        None,
                                    );
                                    let _ = line.paint(
                                        point(
                                            origin.x + px(item.x + 6.0),
                                            origin.y + px(item.y + 4.0 + font_px * 1.4),
                                        ),
                                        line_height,
                                        TextAlign::Left,
                                        None,
                                        window,
                                        _cx,
                                    );
                                }
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_to_width;

    #[test]
    fn truncation() {
        // 12 + 10*0.62*12 = wide enough for exactly 10 chars at 12px
        let w = 12.0 + 10.0 * 0.62 * 12.0;
        assert_eq!(truncate_to_width("short.rs", w, 12.0), Some("short.rs".into()));
        assert_eq!(
            truncate_to_width("a_very_long_file_name.rs", w, 12.0),
            Some("a_very_lo…".into())
        );
        assert_eq!(truncate_to_width("anything", 10.0, 12.0), None);
        // multi-byte chars must not panic
        assert_eq!(truncate_to_width("ééééééééééééé", w, 12.0), Some("ééééééééé…".into()));
    }
}
