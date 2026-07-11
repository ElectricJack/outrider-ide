use std::collections::BTreeMap;

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, size, App, Bounds, BorderStyle, Context,
    FocusHandle, Pixels, TextAlign, TextRun, Window,
};
use outrider_index::buffer::HighlightSpan;
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::{PackLayout, Rect};

use crate::buffers::{collect_file_symbols, BufferManager};
use crate::camera::{self, Camera, CameraTween};
use crate::chrome;
use crate::content::{self, BodyLine, FONT_PX, HEADER, LINE_STEP};
use crate::focus::{self, Focus, TreeIndex};
use crate::theme;
use crate::world::{self, Draw, LeafDraw, Rung};

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

/// Monospace advance width used by the minimap bars (spec §3): 0.6·FONT_PX.
const CHAR_ADV: f64 = 0.6 * content::FONT_PX;
/// Left text inset shared by name rows, body rows, and minimap bars.
const BODY_PAD: f64 = 6.0;

pub struct TreemapView {
    tree: SymbolTree,
    layout: PackLayout,
    /// None until the first render supplies a viewport; then Home-framed.
    camera: Option<Camera>,
    home_zoom: f64,
    drag_last: Option<gpui::Point<Pixels>>,
    press_origin: Option<gpui::Point<Pixels>>,
    focus: Focus,
    tween: Option<(CameraTween, std::time::Instant)>,
    focus_handle: FocusHandle,
    buffers: BufferManager,
    file_symbols: BTreeMap<String, Vec<(SymbolId, usize)>>,
}

/// One shaped body/code line: canvas position, text, and colored runs.
struct BodyText {
    x: f32,
    y: f32,
    text: String,
    runs: Vec<(usize, u32)>,
}

/// A name row — pinned at 12px (containers, Label leaves) or scaled with a
/// leaf page (Text leaves).
struct NameRow {
    x: f32,
    y: f32,
    font_px: f32,
    text: String,
}

/// One minimap bar: a source line drawn as a single colored quad.
struct MinimapBar {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: u32,
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
    stripe: Option<u32>,
    focused: bool,
    /// Font size for body rows: FONT_PX·scale for a Text leaf, else 12.0.
    body_font_px: f32,
    /// Height of an opaque header-background band painted behind container
    /// name + body text so children's boxes don't occlude the header.
    header_bg_h: f32,
    /// Y coordinate for the header background (may differ from `y` when
    /// nested pinned headers are stacked).
    header_bg_y: f32,
    /// Opacity for body text (0..1); used for the Minimap→Text fade.
    body_opacity: f32,
    /// Opacity for minimap bars (0..1); inverse of body_opacity in the fade zone.
    bar_opacity: f32,
    name: Option<NameRow>,
    body: Vec<BodyText>,
    bars: Vec<MinimapBar>,
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

/// Content-table rows for a container, pinned to `pin_y` (which may be
/// stacked below ancestor headers when multiple containers are pinned).
/// `max_h` caps body text to the zoomed container-header area so it
/// never bleeds into the children zone.
fn container_body(
    node: &SymbolNode,
    rung: Rung,
    px: &world::PxRect,
    label_w: f64,
    vh: f64,
    pin_y: f64,
    max_h: f64,
) -> Vec<BodyText> {
    if rung == Rung::Dot || rung == Rung::Label {
        return Vec::new();
    }
    let font = FONT_PX as f32;
    let mut out = Vec::new();
    for (k, line) in content::body_lines(node, rung).into_iter().enumerate() {
        let y = pin_y + HEADER + k as f64 * LINE_STEP;
        if y + LINE_STEP > pin_y + max_h || y + LINE_STEP > px.y + px.h || y > vh {
            break;
        }
        let (text, color) = match line {
            BodyLine::Plain(t) => (t, theme::TEXT_PRIMARY),
            BodyLine::Dim(t) => (t, theme::TEXT_SECONDARY),
        };
        if let Some(shown) = truncate_to_width(&text, label_w as f32, font) {
            let len = shown.len();
            out.push(BodyText {
                x: (px.x + BODY_PAD) as f32,
                y: y as f32,
                text: shown,
                runs: vec![(len, color)],
            });
        }
    }
    out
}

/// A leaf page's rows at uniform scale (spec §2): the signature/readout row
/// then every source line, anchored to the UNCLIPPED top/left so the whole
/// page moves and scales as one unit — no windowing, no clipping. Rows whose
/// scaled y-band leaves the viewport are skipped for cost only.
#[allow(clippy::too_many_arguments)]
fn leaf_text_body(
    node: &SymbolNode,
    left: f64,
    top: f64,
    full_h: f64,
    label_w: f64,
    vh: f64,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> Vec<BodyText> {
    let scale = full_h / content::natural_px(node);
    let font = (FONT_PX * scale) as f32;
    let step = LINE_STEP * scale;
    let x = (left + BODY_PAD * scale) as f32;
    let content_y0 = HEADER.max(HEADER * scale);
    let mut out = Vec::new();
    let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
    let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
    if let Some(m) = buffers.get(&rel, syms) {
        if let Some(start) = m.symbol_start_line(&node.id) {
            let count = (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
            for j in 0..count {
                let y = top + content_y0 + j as f64 * LINE_STEP * scale;
                if y > vh {
                    break;
                }
                if y + step < 0.0 {
                    continue;
                }
                if let Some((text, spans)) = m.buffer.line(start + j) {
                    if let Some((shown, runs)) = code_line(&text, spans, label_w as f32, font) {
                        out.push(BodyText { x, y: y as f32, text: shown, runs });
                    }
                }
            }
            return out;
        }
    }
    let lines = content::body_lines(node, Rung::Full);
    for (k, line) in lines.into_iter().enumerate() {
        let y = top + content_y0 + k as f64 * LINE_STEP * scale;
        if y > vh {
            break;
        }
        if y + step < 0.0 {
            continue;
        }
        let (text, color) = match line {
            BodyLine::Plain(t) => (t, theme::TEXT_PRIMARY),
            BodyLine::Dim(t) => (t, theme::TEXT_SECONDARY),
        };
        if let Some(shown) = truncate_to_width(&text, label_w as f32, font) {
            let len = shown.len();
            out.push(BodyText { x, y: y as f32, text: shown, runs: vec![(len, color)] });
        }
    }
    out
}

/// Minimap bars for a far-zoom leaf (spec §3): one colored quad per source
/// line, pixel-aligned to the rows the glyphs occupy at the Text tier, so
/// the Minimap→Text switch is seamless.
fn leaf_minimap(
    node: &SymbolNode,
    left: f64,
    top: f64,
    full_h: f64,
    vh: f64,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> Vec<MinimapBar> {
    let scale = full_h / content::natural_px(node);
    let step = LINE_STEP * scale;
    let bar_h = (step * 0.7) as f32;
    let content_y0 = HEADER.max(HEADER * scale);
    let mut bars = Vec::new();
    let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
    let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
    if let Some(m) = buffers.get(&rel, syms) {
        if let Some(start) = m.symbol_start_line(&node.id) {
            let count = (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
            for r in 0..count {
                let row_y = top + content_y0 + r as f64 * LINE_STEP * scale;
                if row_y > vh {
                    break;
                }
                if row_y + step < 0.0 {
                    continue;
                }
                let mr = m.buffer.minimap_row(start + r);
                if mr.len == 0 {
                    continue;
                }
                let indent = mr.indent as f64;
                let x = left + (BODY_PAD + indent * CHAR_ADV) * scale;
                let avail = (world::PAGE_W - BODY_PAD - indent * CHAR_ADV).max(0.0);
                let w = (mr.len as f64 * CHAR_ADV).min(avail) * scale;
                bars.push(MinimapBar {
                    x: x as f32,
                    y: (row_y + step * 0.15) as f32,
                    w: w as f32,
                    h: bar_h,
                    color: theme::minimap_color(mr.kind),
                });
            }
        }
    }
    bars
}

/// Map a symbol node to the semantic tint for its box background.
fn classify_tint(node: &SymbolNode) -> theme::BoxTint {
    match &node.id.kind {
        SymbolKind::Folder => {
            match node.name.as_str() {
                "docs" | "doc" | "documentation" => theme::BoxTint::DocsFolder,
                "test" | "tests" | "spec" | "specs" | "__tests__" => theme::BoxTint::TestFolder,
                _ => theme::BoxTint::Normal,
            }
        }
        SymbolKind::Item { label } => {
            match label.as_str() {
                "struct" | "enum" | "trait" | "class" | "interface" | "type" | "typedef"
                    => theme::BoxTint::TypeDef,
                _ => theme::BoxTint::Normal,
            }
        }
        _ => theme::BoxTint::Normal,
    }
}

impl TreemapView {
    pub fn new(tree: SymbolTree, layout: PackLayout, cx: &mut Context<Self>) -> Self {
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
            tween: None,
            focus_handle: cx.focus_handle(),
            buffers,
            file_symbols,
        }
    }

    fn root_rect(&self) -> Rect {
        self.layout
            .rects
            .get(&self.tree.root.id)
            .copied()
            .unwrap_or(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 })
    }

    /// Window title shown in the client titlebar and taskbar.
    fn window_title(&self) -> String {
        let name = self
            .tree
            .repo_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "outrider".into());
        format!("outrider — {name}")
    }

    /// The map's drawable size = the window minus the titlebar. Camera math
    /// and mouse hit-testing both use these; the map canvas is offset down
    /// by `chrome::TITLEBAR_H` in window coordinates.
    fn map_viewport(window: &Window) -> (f64, f64) {
        let vp = window.viewport_size();
        (f64::from(vp.width), f64::from(vp.height) - chrome::TITLEBAR_H)
    }

    /// Framing target for the current focus: leaf pages at natural size
    /// (capped END fit), containers at FOCUS_FRACTION.
    fn frame_focus(
        &self,
        index: &TreeIndex,
        vw: f64,
        vh: f64,
        min_zoom: f64,
        max_zoom: f64,
    ) -> Option<Camera> {
        let r = *self.layout.rects.get(&self.focus.current)?;
        match index.node(&self.focus.current) {
            Some(n) if content::is_leaf_item(n) => {
                Some(camera::frame_page(r, vw, vh, min_zoom, max_zoom))
            }
            _ => Some(camera::frame_rect(r, vw, vh, camera::FOCUS_FRACTION, min_zoom, max_zoom)),
        }
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

    /// A name pinned at 12px to the clipped box corner; `center` vertically
    /// centers it in the box (the Label tier). `pin_y` is the stacked header
    /// y for containers with pinned headers.
    fn pinned_name(item: &world::DrawItem, center: bool, pin_y: f64) -> Option<NameRow> {
        let font = FONT_PX as f32;
        let text = truncate_to_width(&item.node.name, item.label_w as f32, font)?;
        let y = if center {
            item.px.y + (item.px.h - f64::from(font) * 1.3) / 2.0
        } else {
            pin_y + 4.0
        };
        Some(NameRow { x: (item.px.x + BODY_PAD) as f32, y: y as f32, font_px: font, text })
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
            let c = Camera::fit(self.root_rect(), vw, vh);
            self.home_zoom = c.zoom;
            self.camera = Some(c);
        }
        let camera = *self.camera.as_ref().unwrap();
        let focus_id = self.focus.current.clone();
        let items = world::visible_nodes(&self.tree, &self.layout, &camera, vw, vh);
        let mut out = Vec::with_capacity(items.len());
        let mut header_stack: Vec<(u8, f64)> = Vec::new();
        for item in items {
            while let Some(&(lvl, _)) = header_stack.last() {
                if lvl >= item.level {
                    header_stack.pop();
                } else {
                    break;
                }
            }
            let is_leaf = matches!(item.draw, Draw::Leaf(_));
            let box_kind = if is_leaf {
                theme::BoxKind::Leaf
            } else if item.node.id.kind == SymbolKind::Folder {
                theme::BoxKind::Folder
            } else {
                theme::BoxKind::File
            };
            let tint = classify_tint(item.node);
            let fill = theme::box_fill(box_kind, item.level, tint);
            let mut body_font_px = FONT_PX as f32;
            let mut header_bg_h = 0.0f32;
            let mut header_bg_y = item.px.y as f32;
            let mut body_opacity = 1.0f32;
            let mut bar_opacity = 1.0f32;
            let mut name = None;
            let mut body = Vec::new();
            let mut bars = Vec::new();
            match item.draw {
                Draw::Container(rung) => {
                    let stack_bottom =
                        header_stack.last().map(|&(_, b)| b).unwrap_or(item.px.y);
                    let pin_y = item.px.y.max(stack_bottom);
                    let ch_px =
                        (HEADER + 2.0 * LINE_STEP) * camera.zoom;
                    if rung != Rung::Dot && item.px.h >= 14.0 {
                        name = Self::pinned_name(&item, rung == Rung::Label, pin_y);
                    }
                    body = container_body(
                        item.node, rung, &item.px, item.label_w, vh, pin_y, ch_px,
                    );
                    if name.is_some() && !matches!(rung, Rung::Dot | Rung::Label) {
                        header_bg_h = (HEADER + body.len() as f64 * LINE_STEP)
                            .min(ch_px) as f32;
                        header_bg_y = pin_y as f32;
                        header_stack
                            .push((item.level, pin_y + header_bg_h as f64));
                    }
                }
                Draw::Leaf(LeafDraw::Dot) => {}
                Draw::Leaf(LeafDraw::Label | LeafDraw::Minimap | LeafDraw::Text) => {
                    let scale = item.full_h / content::natural_px(item.node);
                    let font = FONT_PX * scale;
                    body_font_px = (FONT_PX * scale) as f32;
                    if item.px.h >= 14.0 {
                        name = Self::pinned_name(&item, false, item.px.y);
                    }
                    let bar_h_fade = ((item.full_h - content::BAR_FADE_LO)
                        / (content::BAR_FADE_HI - content::BAR_FADE_LO))
                        .clamp(0.0, 1.0) as f32;
                    if bar_h_fade > 0.0 && font < content::TEXT_FADE_HI {
                        bars = leaf_minimap(
                            item.node,
                            item.left,
                            item.top,
                            item.full_h,
                            vh,
                            &mut self.buffers,
                            &self.file_symbols,
                        );
                        bar_opacity = bar_h_fade;
                        if font > content::TEXT_FADE_LO {
                            bar_opacity *= 1.0
                                - ((font - content::TEXT_FADE_LO)
                                    / (content::TEXT_FADE_HI - content::TEXT_FADE_LO))
                                    as f32;
                        }
                    }
                    if font >= content::TEXT_FADE_LO
                        && item.label_w >= world::CODE_MIN_W
                    {
                        body = leaf_text_body(
                            item.node,
                            item.left,
                            item.top,
                            item.full_h,
                            item.label_w,
                            vh,
                            &mut self.buffers,
                            &self.file_symbols,
                        );
                        body_opacity = ((font - content::TEXT_FADE_LO)
                            / (content::TEXT_FADE_HI - content::TEXT_FADE_LO))
                            .clamp(0.0, 1.0) as f32;
                    }
                }
            }
            out.push(PaintItem {
                x: item.px.x as f32,
                y: item.px.y as f32,
                w: item.px.w as f32,
                h: item.px.h as f32,
                fill,
                border: theme::border_for(fill),
                stripe: (item.node.churn > 0.0).then(|| theme::churn_heat(item.node.churn)),
                focused: item.node.id == focus_id,
                body_font_px,
                header_bg_h,
                header_bg_y,
                body_opacity,
                bar_opacity,
                name,
                body,
                bars,
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

        let (vw, vh) = Self::map_viewport(window);
        let items = self.paint_items(vw, vh);

        if self.tween.is_some() {
            window.request_animation_frame();
        }

        let max_zoom = camera::MAX_ZOOM;
        let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);

        let title = self.window_title();
        let map = div()
            .flex_grow(1.)
            .w_full()
            .relative()
            .overflow_hidden()
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
                    let (vw, vh) = Self::map_viewport(w);
                    let items = world::visible_nodes(&this.tree, &this.layout, &cam, vw, vh);
                    // the map canvas sits below the titlebar; shift window
                    // coords up by its height to get canvas coords
                    let (mx, my) =
                        (f64::from(e.position.x), f64::from(e.position.y) - chrome::TITLEBAR_H);
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
                let dx = f64::from(e.position.x - last.x);
                let dy = f64::from(e.position.y - last.y);
                if let Some(cam) = this.camera.as_mut() {
                    cam.pan(dx, dy);
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
                let (vw, vh) = Self::map_viewport(w);
                if let Some(cam) = this.camera.as_mut() {
                    // scroll up (positive dy) zooms in; flip the sign here if
                    // manual testing shows it inverted on this platform
                    let factor = (dy * 0.002).exp();
                    cam.zoom_about(
                        f64::from(e.position.x),
                        f64::from(e.position.y) - chrome::TITLEBAR_H,
                        vw,
                        vh,
                        factor,
                        min_zoom,
                        max_zoom,
                    );
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, w, cx| {
                if this.camera.is_none() {
                    return;
                }
                let (vw, vh) = Self::map_viewport(w);
                let max_zoom = camera::MAX_ZOOM;
                let min_zoom = (this.home_zoom * 0.5).min(camera::MAX_ZOOM);
                let index = TreeIndex::new(&this.tree);
                let target = match e.keystroke.key.as_str() {
                    "enter" => {
                        if !this.focus.step_in(&index) {
                            return;
                        }
                        this.frame_focus(&index, vw, vh, min_zoom, max_zoom)
                    }
                    "escape" => {
                        if !this.focus.step_out(&index) {
                            return;
                        }
                        this.frame_focus(&index, vw, vh, min_zoom, max_zoom)
                    }
                    "end" => this.layout.rects.get(&this.focus.current).map(|&r| {
                        camera::frame_rect(r, vw, vh, camera::END_FRACTION, min_zoom, max_zoom)
                    }),
                    "home" => {
                        let c = Camera::fit(this.root_rect(), vw, vh);
                        this.home_zoom = c.zoom;
                        Some(c)
                    }
                    "up" | "down" | "left" | "right" => {
                        let dir = match e.keystroke.key.as_str() {
                            "up" => focus::Dir::Up,
                            "down" => focus::Dir::Down,
                            "left" => focus::Dir::Left,
                            _ => focus::Dir::Right,
                        };
                        let Some(next) =
                            focus::spatial_step(&this.focus.current, dir, &this.layout, &index)
                        else {
                            return;
                        };
                        if !this.focus.set(next, &index) {
                            return;
                        }
                        this.frame_focus(&index, vw, vh, min_zoom, max_zoom)
                    }
                    // Tab stays disabled — no handler.
                    _ => return,
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
                        let run = |len: usize, color: u32| TextRun {
                            len,
                            font: gpui::font(theme::FONT_FAMILY),
                            color: rgb(color).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        // Pass 1: quads, stripes, minimap bars (back to front).
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
                                px(theme::CORNER_RADIUS),
                                rgb(item.fill),
                                px(bw),
                                rgb(bc),
                                BorderStyle::default(),
                            ));
                            if let Some(heat) = item.stripe {
                                let sb = Bounds::new(
                                    point(origin.x + px(item.x + 1.0), origin.y + px(item.y + 1.0)),
                                    size(px(theme::STRIPE_W), px((item.h - 2.0).max(0.0))),
                                );
                                window.paint_quad(quad(
                                    sb,
                                    px(0.),
                                    rgb(heat),
                                    px(0.),
                                    rgb(heat),
                                    BorderStyle::default(),
                                ));
                            }
                            for bar in &item.bars {
                                let bb = Bounds::new(
                                    point(origin.x + px(bar.x), origin.y + px(bar.y)),
                                    size(px(bar.w), px(bar.h)),
                                );
                                let bc = rgb(bar.color).opacity(item.bar_opacity);
                                window.paint_quad(quad(
                                    bb,
                                    px(0.),
                                    bc,
                                    px(0.),
                                    bc,
                                    BorderStyle::default(),
                                ));
                            }
                        }
                        // Pass 2a: leaf / non-header text (rendered under
                        // pinned headers so code doesn't bleed through).
                        for item in &items {
                            if item.header_bg_h > 0.0 {
                                continue;
                            }
                            if let Some(n) = &item.name {
                                let line = window.text_system().shape_line(
                                    n.text.clone().into(),
                                    px(n.font_px),
                                    &[run(n.text.len(), theme::TEXT_PRIMARY)],
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(n.x), origin.y + px(n.y)),
                                    px(n.font_px * 1.3),
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
                            }
                            let body_line_height = px(item.body_font_px * 1.3);
                            for bt in &item.body {
                                if bt.text.is_empty() {
                                    continue;
                                }
                                let runs: Vec<TextRun> = bt.runs.iter().map(|&(len, color)| {
                                    let mut r = run(len, color);
                                    if item.body_opacity < 1.0 {
                                        r.color = r.color.opacity(item.body_opacity);
                                    }
                                    r
                                }).collect();
                                let line = window.text_system().shape_line(
                                    bt.text.clone().into(),
                                    px(item.body_font_px),
                                    &runs,
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(bt.x), origin.y + px(bt.y)),
                                    body_line_height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
                            }
                        }
                        // Pass 2b: headers, background + text interleaved per
                        // item in DFS order, so a later (right/below) header's
                        // opaque background covers earlier headers' text.
                        for item in &items {
                            if item.header_bg_h == 0.0 {
                                continue;
                            }
                            let hb = Bounds::new(
                                point(
                                    origin.x + px(item.x + 1.0),
                                    origin.y + px(item.header_bg_y + 1.0),
                                ),
                                size(px((item.w - 2.0).max(0.0)), px(item.header_bg_h)),
                            );
                            window.paint_quad(quad(
                                hb,
                                px(0.),
                                rgb(item.fill),
                                px(0.),
                                rgb(item.fill),
                                BorderStyle::default(),
                            ));
                            if let Some(n) = &item.name {
                                let line = window.text_system().shape_line(
                                    n.text.clone().into(),
                                    px(n.font_px),
                                    &[run(n.text.len(), theme::TEXT_PRIMARY)],
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(n.x), origin.y + px(n.y)),
                                    px(n.font_px * 1.3),
                                    TextAlign::Left,
                                    None,
                                    window,
                                    _cx,
                                );
                            }
                            let body_line_height = px(item.body_font_px * 1.3);
                            for bt in &item.body {
                                if bt.text.is_empty() {
                                    continue;
                                }
                                let runs: Vec<TextRun> =
                                    bt.runs.iter().map(|&(len, color)| run(len, color)).collect();
                                let line = window.text_system().shape_line(
                                    bt.text.clone().into(),
                                    px(item.body_font_px),
                                    &runs,
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(bt.x), origin.y + px(bt.y)),
                                    body_line_height,
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
            );

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BG))
            .child(chrome::titlebar(title, window))
            .child(map)
            .children(chrome::resize_rim(window))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    use super::{
        code_line, container_body, leaf_minimap, leaf_text_body, runs_from_spans,
        truncate_to_width, HEADER, LINE_STEP,
    };
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
    fn container_body_positions_detail_lines() {
        let f = node(SymbolKind::File, "a.rs", Some(0..24), 2, None, Some("Doc line."));
        let px = PxRect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let body = container_body(&f, Rung::Detail, &px, 400.0, 600.0, px.y, 300.0);
        // churn readout + doc first line (no items → no kind-counts line)
        assert_eq!(body.len(), 2);
        assert_eq!(body[1].text, "Doc line.");
        assert!((f64::from(body[0].y) - HEADER).abs() < 1e-3);
        assert!((f64::from(body[1].y) - (HEADER + LINE_STEP)).abs() < 1e-3);
    }

    #[test]
    fn leaf_text_body_paints_code_without_duplicate_signature() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let leaf = node(SymbolKind::Item { label: "fn".into() }, "a.rs::two", Some(12..23), 1, Some("fn two()"), None);
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 12)]);
        let natural = crate::content::natural_px(&leaf);
        // scale 1.0: full_h == natural
        let body =
            leaf_text_body(&leaf, 0.0, 0.0, natural, 480.0, 600.0, &mut mgr, &file_symbols);
        // code only — no separate signature row (the code line IS the signature)
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].text, "fn two() {}");
        assert!(body[0].runs.len() > 1, "code rows carry colored runs");
        assert_eq!(body[0].runs.iter().map(|r| r.0).sum::<usize>(), body[0].text.len());
        // code row 0 at natural-y HEADER
        assert!((f64::from(body[0].y) - HEADER).abs() < 1e-3);
    }

    #[test]
    fn leaf_text_body_scales_uniformly_past_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let leaf = node(SymbolKind::Item { label: "fn".into() }, "a.rs::two", Some(12..23), 1, Some("fn two()"), None);
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 12)]);
        let natural = crate::content::natural_px(&leaf);
        // zoom 2× (full_h = 2·natural): code row y doubles, still no clip
        let body = leaf_text_body(
            &leaf, 0.0, 0.0, 2.0 * natural, 960.0, 100_000.0, &mut mgr, &file_symbols,
        );
        assert_eq!(body.len(), 1);
        assert!((f64::from(body[0].y) - 2.0 * HEADER).abs() < 1e-3);
        // buffer unavailable → signature only, no code
        let mut broken = BufferManager::new(std::path::PathBuf::from("/nonexistent"));
        let body =
            leaf_text_body(&leaf, 0.0, 0.0, natural, 480.0, 600.0, &mut broken, &BTreeMap::new());
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].text, "fn two()");
    }

    fn make_node(kind: SymbolKind, name: &str) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: name.into(), ordinal: 0 },
            name: name.to_string(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 0,
            churn: 0.0,
            churn_count: 0,
            children: vec![],
        }
    }

    #[test]
    fn classify_tint_docs_folder() {
        use super::classify_tint;
        use crate::theme::BoxTint;
        for name in &["docs", "doc", "documentation"] {
            let n = make_node(SymbolKind::Folder, name);
            assert_eq!(classify_tint(&n), BoxTint::DocsFolder, "expected DocsFolder for {name}");
        }
    }

    #[test]
    fn classify_tint_test_folder() {
        use super::classify_tint;
        use crate::theme::BoxTint;
        for name in &["test", "tests", "spec", "specs", "__tests__"] {
            let n = make_node(SymbolKind::Folder, name);
            assert_eq!(classify_tint(&n), BoxTint::TestFolder, "expected TestFolder for {name}");
        }
    }

    #[test]
    fn classify_tint_typedef_items() {
        use super::classify_tint;
        use crate::theme::BoxTint;
        for label in &["struct", "enum", "trait", "class", "interface", "type", "typedef"] {
            let n = make_node(SymbolKind::Item { label: label.to_string() }, "Foo");
            assert_eq!(classify_tint(&n), BoxTint::TypeDef, "expected TypeDef for {label}");
        }
    }

    #[test]
    fn classify_tint_normal_cases() {
        use super::classify_tint;
        use crate::theme::BoxTint;
        // Unrecognized folder name
        assert_eq!(classify_tint(&make_node(SymbolKind::Folder, "src")), BoxTint::Normal);
        // Non-typedef item label
        assert_eq!(
            classify_tint(&make_node(SymbolKind::Item { label: "fn".into() }, "foo")),
            BoxTint::Normal
        );
        // File and Chunk always Normal
        assert_eq!(classify_tint(&make_node(SymbolKind::File, "main.rs")), BoxTint::Normal);
        assert_eq!(classify_tint(&make_node(SymbolKind::Chunk, "chunk")), BoxTint::Normal);
    }

    #[test]
    fn leaf_minimap_bars_align_to_code_rows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\n    let x = 1;\n").unwrap();
        let leaf = node(SymbolKind::File, "a.rs", Some(0..24), 2, None, None);
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 0)]);
        let natural = crate::content::natural_px(&leaf);
        let bars = leaf_minimap(&leaf, 0.0, 0.0, natural, 600.0, &mut mgr, &file_symbols);
        // two non-blank source lines → two bars
        assert_eq!(bars.len(), 2);
        // bar 0 sits centered in the first code row (HEADER)
        let row_y0 = HEADER;
        assert!((f64::from(bars[0].y) - (row_y0 + LINE_STEP * 0.15)).abs() < 1e-3);
        // second line is indented 4 spaces → its bar starts further right
        assert!(bars[1].x > bars[0].x);
    }
}
