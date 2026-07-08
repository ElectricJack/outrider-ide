use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, size, App, Bounds, BorderStyle, Context,
    FocusHandle, Pixels, TextAlign, TextRun, Window,
};
use outrider_index::SymbolTree;
use outrider_layout::WorldLayout;

use crate::camera::Camera;
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
    rung: Rung,
    name: String,
    meta: String,
}

impl TreemapView {
    pub fn new(tree: SymbolTree, layout: WorldLayout, cx: &mut Context<Self>) -> Self {
        Self {
            tree,
            layout,
            camera: None,
            home_zoom: 1.0,
            drag_last: None,
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

    fn home_camera(&self, vw: f64, vh: f64) -> Camera {
        Camera::frame(world::world_width(), self.root_world_height(), vw, vh)
    }

    fn paint_items(&mut self, vw: f64, vh: f64) -> Vec<PaintItem> {
        if self.camera.is_none() {
            let c = self.home_camera(vw, vh);
            self.home_zoom = c.zoom;
            self.camera = Some(c);
        }
        let camera = *self.camera.as_ref().unwrap();
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
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let (vw, vh) = (f64::from(vp.width), f64::from(vp.height));
        let items = self.paint_items(vw, vh);

        div().size_full().bg(rgb(theme::BG)).child(
            canvas(
                |_bounds, _window, _cx: &mut App| {},
                move |bounds, _prepaint, window, _cx: &mut App| {
                    let origin = bounds.origin;
                    for item in &items {
                        let b = Bounds::new(
                            point(origin.x + px(item.x), origin.y + px(item.y)),
                            size(px(item.w), px(item.h)),
                        );
                        window.paint_quad(quad(
                            b,
                            px(0.),
                            rgb(item.fill),
                            px(1.),
                            rgb(item.border),
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
