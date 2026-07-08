use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, size, App, Bounds, BorderStyle, Context,
    FocusHandle, Pixels, Window,
};
use outrider_index::SymbolTree;
use outrider_layout::WorldLayout;

use crate::camera::Camera;
use crate::theme;
use crate::world::{self, Rung};

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
                    }
                },
            )
            .size_full(),
        )
    }
}
