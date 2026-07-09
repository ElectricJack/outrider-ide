use std::collections::BTreeMap;

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, size, App, Bounds, BorderStyle, Context,
    FocusHandle, Pixels, TextAlign, TextRun, Window,
};
use outrider_index::buffer::HighlightSpan;
use outrider_index::{SymbolId, SymbolNode, SymbolTree};
use outrider_layout::WorldLayout;

use crate::buffers::{collect_file_symbols, BufferManager};
use crate::camera::{self, Camera, CameraTween};
use crate::content::{self, BodyLine, FONT_PX, HEADER, LINE_STEP};
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
    /// Sticky framing fraction for arrow steps: FOCUS_FRACTION by default,
    /// END_FRACTION after End, reset by Home. Re-framing every step at this
    /// fraction keeps the fidelity rung constant across different-sized
    /// nodes (a zoom floor cannot: box height = zoom × node height) and
    /// lets Left zoom out to frame the parent.
    step_fraction: f64,
    tween: Option<(CameraTween, std::time::Instant)>,
    focus_handle: FocusHandle,
    buffers: BufferManager,
    file_symbols: BTreeMap<String, Vec<(SymbolId, usize)>>,
}

/// One shaped body line: canvas y plus full-coverage (byte len, color) runs.
struct BodyText {
    y: f32,
    text: String,
    runs: Vec<(usize, u32)>,
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
    body: Vec<BodyText>,
}

/// Full-coverage colored runs for the first `len` bytes of a line from its
/// highlight spans; gaps paint TEXT_PRIMARY. Run lengths sum exactly to `len`.
fn runs_from_spans(len: usize, spans: &[HighlightSpan]) -> Vec<(usize, u32)> {
    let mut runs = Vec::new();
    let mut pos = 0;
    for s in spans {
        let start = s.range.start.min(len);
        let end = s.range.end.min(len);
        if start > pos {
            runs.push((start - pos, theme::TEXT_PRIMARY));
        }
        if end > start {
            runs.push((end - start, theme::syntax_color(s.kind)));
        }
        pos = pos.max(end);
    }
    if pos < len {
        runs.push((len - pos, theme::TEXT_PRIMARY));
    }
    runs
}

/// Truncate a code line to the box width, clipping its runs to the kept
/// bytes; a trailing ellipsis paints TEXT_PRIMARY.
fn code_line(
    text: &str,
    spans: &[HighlightSpan],
    w: f32,
    font_px: f32,
) -> Option<(String, Vec<(usize, u32)>)> {
    let shown = truncate_to_width(text, w, font_px)?;
    let truncated = shown != text;
    let kept = if truncated { shown.len() - '…'.len_utf8() } else { shown.len() };
    let mut runs = runs_from_spans(kept, spans);
    if truncated {
        runs.push(('…'.len_utf8(), theme::TEXT_PRIMARY));
    }
    Some((shown, runs))
}

/// Body content for one box: content-table lines anchored to the CLIPPED
/// top (they pin like the name row), then — for Full leaf items — the
/// symbol's highlighted code laid out from the UNCLIPPED top and
/// line-window culled to the viewport (spec §4.4). Rows that would sit
/// under the pinned name/signature block or off-screen are skipped.
fn build_body(
    node: &SymbolNode,
    rung: Rung,
    px: &world::PxRect,
    top: f64,
    vh: f64,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> Vec<BodyText> {
    if rung == Rung::Dot || rung == Rung::Label {
        return Vec::new();
    }
    let mut out = Vec::new();
    let lines = content::body_lines(node, rung);
    let rows = lines.len();
    for (k, line) in lines.into_iter().enumerate() {
        let y = px.y + HEADER + k as f64 * LINE_STEP;
        if y + LINE_STEP > px.y + px.h || y > vh {
            break;
        }
        let (text, color) = match line {
            BodyLine::Plain(t) => (t, theme::TEXT_PRIMARY),
            BodyLine::Dim(t) => (t, theme::TEXT_SECONDARY),
        };
        if let Some(shown) = truncate_to_width(&text, px.w as f32, FONT_PX as f32) {
            let len = shown.len();
            out.push(BodyText { y: y as f32, text: shown, runs: vec![(len, color)] });
        }
    }
    if rung == Rung::Full && content::is_leaf_item(node) {
        let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
        let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
        if let Some(m) = buffers.get(&rel, syms) {
            if let Some(start) = m.symbol_start_line(&node.id) {
                let count = (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
                let code_y0 = top + HEADER + rows as f64 * LINE_STEP;
                let min_y = px.y + HEADER + rows as f64 * LINE_STEP - 0.5;
                let max_y = (px.y + px.h).min(vh) - LINE_STEP;
                for j in 0..count {
                    let y = code_y0 + j as f64 * LINE_STEP;
                    if y < min_y {
                        continue;
                    }
                    if y > max_y {
                        break;
                    }
                    if let Some((text, spans)) = m.buffer.line(start + j) {
                        if let Some((shown, runs)) =
                            code_line(&text, spans, px.w as f32, FONT_PX as f32)
                        {
                            out.push(BodyText { y: y as f32, text: shown, runs });
                        }
                    }
                }
            }
        }
    }
    out
}

impl TreemapView {
    pub fn new(tree: SymbolTree, layout: WorldLayout, cx: &mut Context<Self>) -> Self {
        let root_id = tree.root.id.clone();
        let file_symbols = collect_file_symbols(&tree);
        let buffers = BufferManager::new(tree.repo_root.clone());
        Self {
            tree,
            layout,
            camera: None,
            home_zoom: 1.0,
            drag_last: None,
            press_origin: None,
            focus: Focus::new(root_id),
            step_fraction: camera::FOCUS_FRACTION,
            tween: None,
            focus_handle: cx.focus_handle(),
            buffers,
            file_symbols,
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
        let items = world::visible_nodes(&self.tree, &self.layout, &camera, vw, vh);
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let f = theme::churn_fill(item.node.churn);
            let body = build_body(
                item.node,
                item.rung,
                &item.px,
                item.top,
                vh,
                &mut self.buffers,
                &self.file_symbols,
            );
            out.push(PaintItem {
                x: item.px.x as f32,
                y: item.px.y as f32,
                w: item.px.w as f32,
                h: item.px.h as f32,
                fill: f,
                border: theme::border_for(f),
                focused: item.node.id == focus_id,
                rung: item.rung,
                name: item.node.name.clone(),
                body,
            });
        }
        out
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
                    "left" => this.focus.step_out(&index),
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
                            camera::frame_band(y, h, vh, this.step_fraction, min_zoom, max_zoom)
                        })
                    }
                    "end" => {
                        this.step_fraction = camera::END_FRACTION;
                        world::world_band(&this.focus.current, &this.layout).map(|(y, h)| {
                            camera::frame_band(y, h, vh, camera::END_FRACTION, min_zoom, max_zoom)
                        })
                    }
                    "home" => {
                        this.step_fraction = camera::FOCUS_FRACTION;
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
                            for bt in &item.body {
                                if bt.text.is_empty() {
                                    continue;
                                }
                                let runs: Vec<TextRun> =
                                    bt.runs.iter().map(|&(len, color)| run(len, color)).collect();
                                let line = window.text_system().shape_line(
                                    bt.text.clone().into(),
                                    px(font_px),
                                    &runs,
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(item.x + 6.0), origin.y + px(bt.y)),
                                    line_height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
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
    use std::collections::BTreeMap;

    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    use super::{build_body, code_line, runs_from_spans, truncate_to_width, HEADER, LINE_STEP};
    use crate::buffers::BufferManager;
    use crate::world::{PxRect, Rung};

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

    fn node(kind: SymbolKind, qual: &str, byte_range: Option<std::ops::Range<usize>>, measure: u64, signature: Option<&str>, doc: Option<&str>) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qual.into(), ordinal: 0 },
            name: qual.to_string(),
            byte_range,
            signature: signature.map(str::to_string),
            doc: doc.map(str::to_string),
            measure,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        }
    }

    #[test]
    fn runs_cover_text_exactly_and_truncate() {
        use outrider_index::buffer::{HighlightKind, HighlightSpan};
        let spans = vec![
            HighlightSpan { range: 0..2, kind: HighlightKind::Keyword },
            HighlightSpan { range: 3..7, kind: HighlightKind::Function },
        ];
        let runs = runs_from_spans(10, &spans);
        assert_eq!(runs.iter().map(|r| r.0).sum::<usize>(), 10);
        assert_eq!(runs.len(), 4); // keyword, gap, function, tail
        // truncated code line: run lengths still cover the shown bytes exactly
        let w = 12.0 + 5.0 * 0.62 * 12.0; // 5-char budget at 12px
        let (shown, runs) = code_line("fn frobnicate()", &spans, w, 12.0).unwrap();
        assert_eq!(shown, "fn f…");
        assert_eq!(runs.iter().map(|r| r.0).sum::<usize>(), shown.len());
        // too narrow for any text → no line
        assert!(code_line("fn x()", &spans, 10.0, 12.0).is_none());
    }

    #[test]
    fn build_body_positions_detail_lines() {
        let f = node(SymbolKind::File, "a.rs", Some(0..24), 2, None, Some("Doc line."));
        let px = PxRect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let mut mgr = BufferManager::new(std::path::PathBuf::from("/nonexistent"));
        let body = build_body(&f, Rung::Detail, &px, 0.0, 600.0, &mut mgr, &BTreeMap::new());
        // churn readout + doc first line (no items → no kind-counts line)
        assert_eq!(body.len(), 2);
        assert_eq!(body[1].text, "Doc line.");
        assert!((f64::from(body[0].y) - HEADER).abs() < 1e-3);
        assert!((f64::from(body[1].y) - (HEADER + LINE_STEP)).abs() < 1e-3);
    }

    #[test]
    fn build_body_full_leaf_appends_windowed_code() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let leaf = node(SymbolKind::Fn, "a.rs::two", Some(12..23), 1, Some("fn two()"), None);
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 12)]);
        let px = PxRect { x: 0.0, y: 0.0, w: 400.0, h: 800.0 };
        let body = build_body(&leaf, Rung::Full, &px, 0.0, 600.0, &mut mgr, &file_symbols);
        // signature row + exactly the symbol's one code line (line-window)
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].text, "fn two()");
        assert_eq!(body[1].text, "fn two() {}");
        assert!(body[1].runs.len() > 1, "code rows carry colored runs");
        assert_eq!(body[1].runs.iter().map(|r| r.0).sum::<usize>(), body[1].text.len());
        assert!((f64::from(body[1].y) - (HEADER + LINE_STEP)).abs() < 1e-3);
        // buffer unavailable → Detail-equivalent content (signature, no code)
        let mut broken = BufferManager::new(std::path::PathBuf::from("/nonexistent"));
        let body = build_body(&leaf, Rung::Full, &px, 0.0, 600.0, &mut broken, &BTreeMap::new());
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].text, "fn two()");
    }
}
