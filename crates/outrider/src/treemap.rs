//! Main GPUI view for the outrider treemap — drives the render loop, handles
//! all input (mouse drag/zoom/click, keyboard navigation), and translates the
//! world-space layout from `outrider-layout` into per-frame paint instructions
//! (quads, text runs, and baked texture quads) via a static canvas closure.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, rgba, size, transparent_black, App, Bounds,
    BorderStyle, ContentMask, Context, Corners, FocusHandle, Pixels, RenderImage, TextAlign,
    TextRun, Window,
};
use outrider_index::buffer::HighlightSpan;
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::{PackLayout, Rect};

use crate::buffers::{collect_file_symbols, BufferManager};
use crate::camera::{self, Camera, CameraTween};
use crate::chrome;
use crate::content::{self, BodyLine, FONT_PX, HEADER, LINE_STEP};
use crate::focus::{self, Focus, TreeIndex};
use crate::rasterize::{self, TextureCache};
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

/// Reflow a `///` doc block for the texture-tier overlay: source lines are
/// joined into paragraphs (a blank line is a paragraph break), then each
/// paragraph is greedy word-wrapped to the same char budget as
/// `truncate_to_width`. Words longer than the budget are hard-split.
fn wrap_doc(text: &str, w_px: f64, font_px: f64) -> Vec<String> {
    let budget = ((w_px - 12.0) / (font_px * 0.62) + 1e-6).floor() as isize;
    if budget < 2 {
        return Vec::new();
    }
    let budget = budget as usize;
    let mut rows = Vec::new();
    for para in text.split("\n\n") {
        let joined = para.split_whitespace().collect::<Vec<_>>().join(" ");
        if joined.is_empty() {
            continue;
        }
        let mut line = String::new();
        let mut line_len = 0usize;
        for word in joined.split(' ') {
            let mut word = word;
            let mut wlen = word.chars().count();
            while wlen > budget {
                if line_len > 0 {
                    rows.push(std::mem::take(&mut line));
                    line_len = 0;
                }
                let cut = word
                    .char_indices()
                    .nth(budget)
                    .map_or(word.len(), |(i, _)| i);
                rows.push(word[..cut].to_string());
                word = &word[cut..];
                wlen = word.chars().count();
            }
            if wlen == 0 {
                continue;
            }
            let need = if line_len == 0 { wlen } else { line_len + 1 + wlen };
            if need > budget {
                rows.push(std::mem::take(&mut line));
                line.push_str(word);
                line_len = wlen;
            } else {
                if line_len > 0 {
                    line.push(' ');
                }
                line.push_str(word);
                line_len = need;
            }
        }
        if line_len > 0 {
            rows.push(line);
        }
    }
    rows
}

/// Screen-space doc-overlay rows for a texture-tier leaf: the item's `///`
/// doc wrapped to the box's inner width, one crisp 12px row per line
/// starting under the pinned name row. If the wrapped text overflows the
/// box, the entire overlay is hidden (returns empty) — partial descriptions
/// at small sizes are unreadable.
fn doc_overlay(doc: &str, px: &world::PxRect) -> (Vec<BodyText>, f32) {
    let wrapped = wrap_doc(doc, px.w - 2.0 * BODY_PAD, FONT_PX);
    let mut rows = Vec::new();
    let mut y = px.y + HEADER;
    for text in &wrapped {
        if y + LINE_STEP > px.y + px.h {
            return (Vec::new(), 0.0);
        }
        let runs = vec![(text.len(), theme::DOC_COLOR)];
        rows.push(BodyText { x: (px.x + BODY_PAD) as f32, y: y as f32, text: text.clone(), runs });
        y += LINE_STEP;
    }
    let panel_h = if rows.is_empty() { 0.0 } else { (y - px.y) as f32 };
    (rows, panel_h)
}

/// Left text inset shared by name rows and body rows.
pub(crate) const BODY_PAD: f64 = 6.0;

/// Width of the floating doc panel shown to the right of the focused leaf.
const DOC_PANEL_W: f64 = 280.0;

/// Root GPUI view: owns the symbol tree, pack layout, camera state, buffer
/// cache, and texture cache; produces a full-screen canvas each frame.
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
    textures: TextureCache,
    bake_pending: bool,
    /// The four beam-cast arrow targets of the focused node (Left, Right,
    /// Up, Down), cached because layout is immutable per session.
    neighbors: Option<(SymbolId, [Option<SymbolId>; 4])>,
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

/// One baked-texture quad for a far-zoom leaf (replaces minimap bars).
struct TexQuad {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    image: Arc<RenderImage>,
}

/// Floating doc-description panel shown to the right of the focused leaf.
struct DocPanel {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rows: Vec<BodyText>,
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
    /// One of the four arrow-key targets — painted with a dimmed focus border.
    neighbor: bool,
    /// Font size for body rows: FONT_PX·scale for a Text leaf, else 12.0.
    body_font_px: f32,
    /// Height of an opaque header-background band painted behind container
    /// name + body text so children's boxes don't occlude the header.
    header_bg_h: f32,
    /// Y coordinate for the header background (may differ from `y` when
    /// nested pinned headers are stacked).
    header_bg_y: f32,
    /// Opacity for body text (0..1); used for the Texture→Text fade.
    body_opacity: f32,
    /// Opacity for the baked texture quad (0..1); inverse of body_opacity in the fade zone.
    tex_opacity: f32,
    /// Crisp 12px doc-description rows overlaid on a texture-tier leaf.
    doc_rows: Vec<BodyText>,
    /// Height of the translucent backdrop panel behind name + doc rows
    /// (0.0 = no overlay).
    doc_panel_h: f32,
    name: Option<NameRow>,
    body: Vec<BodyText>,
    tex: Option<TexQuad>,
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

/// Unclipped screen rect of a leaf's line area: full page width, rows
/// starting under the header band — the same rows leaf_text_body fills,
/// so the Text↔Texture crossfade is seamless.
fn leaf_tex_rect(node: &SymbolNode, left: f64, top: f64, full_h: f64) -> (f64, f64, f64, f64) {
    let scale = full_h / content::natural_px(node);
    let content_y0 = HEADER.max(HEADER * scale);
    (
        left,
        top + content_y0,
        world::PAGE_W * scale,
        node.measure as f64 * LINE_STEP * scale,
    )
}

/// Predicted height of the pinned ancestor-header stack above `focus` under
/// camera `cam`, mirroring paint_items' stacking: each named ancestor's
/// header pins at max(its screen top clamped to the viewport, the previous
/// header's bottom). Header height uses the 2-body-line cap, exact for
/// zoom ≤ 1 (leaf framing) and a close estimate above it.
fn pinned_stack_h(
    focus: &SymbolId,
    layout: &PackLayout,
    index: &TreeIndex,
    cam: &Camera,
    vw: f64,
    vh: f64,
) -> f64 {
    let hdr = (HEADER + 2.0 * LINE_STEP) * cam.zoom.min(1.0);
    let mut chain = Vec::new();
    let mut id = focus;
    while let Some(p) = index.parent(id) {
        chain.push(p);
        id = p;
    }
    let mut bottom = 0.0f64;
    for anc in chain.into_iter().rev() {
        if index.node(anc).is_none_or(|n| n.name.is_empty()) {
            continue;
        }
        let Some(r) = layout.rects.get(anc) else { continue };
        let (_, sy) = cam.world_to_screen(r.x, r.y, vw, vh);
        bottom = sy.max(0.0).max(bottom) + hdr;
    }
    bottom
}

/// Re-center `cam` vertically so `r` starts below `inset`: centered in the
/// `[inset, vh]` band, or pinned to the band top when taller than the band.
fn inset_top(mut cam: Camera, r: Rect, inset: f64, vh: f64) -> Camera {
    let top = inset + ((vh - inset - r.h * cam.zoom) / 2.0).max(0.0);
    cam.center_y = r.y - (top - vh / 2.0) / cam.zoom;
    cam
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

/// Construction, camera helpers, and the per-frame paint pipeline.
impl TreemapView {
    /// Construct from a fully-indexed `SymbolTree` and its `PackLayout`;
    /// camera is deferred until the first render supplies a viewport.
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
            textures: TextureCache::new(rasterize::MAX_BYTES),
            bake_pending: false,
            neighbors: None,
        }
    }

    /// World-space rect of the root node, used for Home framing.
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

    /// Frame the focused rect below the pinned ancestor-header stack: frame
    /// normally, predict the stack height under that camera, and if the rect
    /// would start under the stack, reframe into the `[stack, vh]` band.
    fn frame_below_headers(
        &self,
        index: &TreeIndex,
        r: Rect,
        vw: f64,
        vh: f64,
        frame: impl Fn(f64) -> Camera,
    ) -> Camera {
        let c0 = frame(vh);
        let stack = pinned_stack_h(&self.focus.current, &self.layout, index, &c0, vw, vh);
        let top0 = (vh - r.h * c0.zoom) / 2.0;
        if stack <= top0 {
            return c0;
        }
        let inset = stack.min(vh / 2.0);
        inset_top(frame(vh - inset), r, inset, vh)
    }

    /// Framing target for the current focus: leaf pages at natural size
    /// (capped END fit), containers at FOCUS_FRACTION — both nudged below
    /// any pinned ancestor headers so the focus is never underlapped.
    fn frame_focus(
        &self,
        index: &TreeIndex,
        vw: f64,
        vh: f64,
        min_zoom: f64,
        max_zoom: f64,
    ) -> Option<Camera> {
        let r = *self.layout.rects.get(&self.focus.current)?;
        let leaf = index.node(&self.focus.current).is_some_and(content::is_leaf_item);
        Some(self.frame_below_headers(index, r, vw, vh, |vh_eff| {
            if leaf {
                camera::frame_page(r, vw, vh_eff, min_zoom, max_zoom)
            } else {
                camera::frame_rect(r, vw, vh_eff, camera::FOCUS_FRACTION, min_zoom, max_zoom)
            }
        }))
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

    /// Advance the tween, materialize buffers/textures, and build the
    /// `PaintItem` list + optional focused-leaf doc panel for the current
    /// frame; also kicks off queued bakes.
    fn paint_items(&mut self, vw: f64, vh: f64) -> (Vec<PaintItem>, Option<DocPanel>) {
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
        let stale = !matches!(&self.neighbors, Some((k, _)) if k == &focus_id);
        if stale {
            let index = TreeIndex::new(&self.tree);
            self.neighbors =
                Some((focus_id.clone(), focus::neighbors(&focus_id, &self.layout, &index)));
        }
        let (_, neighbor_ids) = self.neighbors.clone().unwrap();
        let items = world::visible_nodes(&self.tree, &self.layout, &camera, vw, vh);
        let mut out = Vec::with_capacity(items.len());
        let mut header_stack: Vec<(u8, f64)> = Vec::new();
        let mut focus_doc: Option<(String, f32, f32, f32, f32)> = None;
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
            let mut tex_opacity = 1.0f32;
            let mut doc_rows = Vec::new();
            let mut doc_panel_h = 0.0f32;
            let mut name = None;
            let mut body = Vec::new();
            let mut tex: Option<TexQuad> = None;
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
                Draw::Leaf(tier) => {
                    let scale = item.full_h / content::natural_px(item.node);
                    let font = FONT_PX * scale;
                    body_font_px = (FONT_PX * scale) as f32;
                    if tier != LeafDraw::Dot && item.px.h >= 14.0 {
                        name = Self::pinned_name(&item, false, item.px.y);
                    }
                    if font < content::TEXT_FADE_HI {
                        let (tx, ty, tw, th) =
                            leaf_tex_rect(item.node, item.left, item.top, item.full_h);
                        if tw >= 1.0 && th >= 1.0 && ty < vh && ty + th > 0.0 {
                            if let Some(t) = self.textures.get(&item.node.id, tw * th) {
                                tex = t.level_for(th as f32).map(|img| TexQuad {
                                    x: tx as f32,
                                    y: ty as f32,
                                    w: tw as f32,
                                    h: th as f32,
                                    image: img.clone(),
                                });
                            }
                        }
                        if let Some(doc) = &item.node.doc {
                            (doc_rows, doc_panel_h) = doc_overlay(doc, &item.px);
                        }
                        if font > content::TEXT_FADE_LO {
                            tex_opacity = 1.0
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
            let is_focused = item.node.id == focus_id;
            if is_focused && is_leaf {
                if let Some(doc) = &item.node.doc {
                    focus_doc = Some((
                        doc.clone(),
                        item.px.x as f32,
                        item.px.y as f32,
                        item.px.w as f32,
                        item.px.h as f32,
                    ));
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
                focused: is_focused,
                neighbor: !is_focused
                    && neighbor_ids.iter().flatten().any(|n| *n == item.node.id),
                body_font_px,
                header_bg_h,
                header_bg_y,
                body_opacity,
                tex_opacity,
                doc_rows,
                doc_panel_h,
                name,
                body,
                tex,
            });
        }
        let doc_panel = focus_doc.and_then(|(doc, fx, fy, fw, fh)| {
            let panel_x = fx + fw + 4.0;
            let panel_w = DOC_PANEL_W as f32;
            let wrapped = wrap_doc(&doc, (panel_w as f64) - 2.0 * BODY_PAD, FONT_PX);
            if wrapped.is_empty() {
                return None;
            }
            let mut rows = Vec::new();
            let mut y = fy + BODY_PAD as f32;
            for text in wrapped {
                let runs = vec![(text.len(), theme::DOC_COLOR)];
                rows.push(BodyText { x: panel_x + BODY_PAD as f32, y, text, runs });
                y += LINE_STEP as f32;
            }
            let panel_h = y - fy + BODY_PAD as f32;
            Some(DocPanel { x: panel_x, y: fy, w: panel_w, h: panel_h.min(fh), rows })
        });
        self.bake_pending = if self.textures.has_queued() {
            let index = TreeIndex::new(&self.tree);
            let buffers = &mut self.buffers;
            let file_symbols = &self.file_symbols;
            self.textures.bake_queued(|id| {
                let node = index.node(id)?;
                let rel = BufferManager::file_path_of(&id.qualified_path).to_string();
                let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
                let m = buffers.get(&rel, syms)?;
                let start = m.symbol_start_line(id)?;
                let count =
                    (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
                let mut lines: Vec<rasterize::Line> = Vec::with_capacity(count);
                for j in 0..count {
                    let (text, spans) = m.buffer.line(start + j)?;
                    let runs = runs_from_spans(text.len(), spans);
                    lines.push((text, runs));
                }
                (!lines.is_empty()).then_some(lines)
            })
        } else {
            false
        };
        (out, doc_panel)
    }
}

/// GPUI render entry point: wires input handlers onto the map canvas and
/// composes the titlebar + canvas into the window element tree.
impl Render for TreemapView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window, cx);
        }

        let (vw, vh) = Self::map_viewport(window);
        let (items, doc_panel) = self.paint_items(vw, vh);

        for img in self.textures.take_retired() {
            let _ = window.drop_image(img);
        }
        if self.tween.is_some() || self.bake_pending {
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
                    "end" => this.layout.rects.get(&this.focus.current).copied().map(|r| {
                        this.frame_below_headers(&index, r, vw, vh, |vh_eff| {
                            camera::frame_rect(
                                r, vw, vh_eff, camera::END_FRACTION, min_zoom, max_zoom,
                            )
                        })
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
                        // Pass 1: quads, stripes, texture quads (back to front).
                        for item in &items {
                            let b = Bounds::new(
                                point(origin.x + px(item.x), origin.y + px(item.y)),
                                size(px(item.w), px(item.h)),
                            );
                            window.paint_quad(quad(
                                b,
                                px(theme::CORNER_RADIUS),
                                rgb(item.fill),
                                px(1.0),
                                rgb(item.border),
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
                            if let Some(t) = &item.tex {
                                let tb = Bounds::new(
                                    point(origin.x + px(t.x), origin.y + px(t.y)),
                                    size(px(t.w), px(t.h)),
                                );
                                // The texture rect is unclipped (it scales with
                                // the whole page); mask it to the box so it
                                // can't spill past the bottom edge at far zoom.
                                window.with_content_mask(
                                    Some(ContentMask { bounds: b }),
                                    |window| {
                                        let _ = window.paint_image(
                                            tb,
                                            Corners::default(),
                                            t.image.clone(),
                                            0,
                                            false,
                                        );
                                        // Fade out the texture as text fades in by
                                        // overlaying a semi-transparent bg-colored quad.
                                        if item.tex_opacity < 1.0 {
                                            let fade = 1.0 - item.tex_opacity;
                                            let oc = rgb(theme::CODE_BG).opacity(fade);
                                            window.paint_quad(quad(
                                                tb,
                                                px(0.),
                                                oc,
                                                px(0.),
                                                oc,
                                                BorderStyle::default(),
                                            ));
                                        }
                                    },
                                );
                            }
                            if !item.doc_rows.is_empty() {
                                let pc = rgb(theme::CODE_BG).opacity(0.85 * item.tex_opacity);
                                let pb = Bounds::new(
                                    point(
                                        origin.x + px(item.x + 1.0),
                                        origin.y + px(item.y + 1.0),
                                    ),
                                    size(
                                        px((item.w - 2.0).max(0.0)),
                                        px((item.doc_panel_h - 1.0).max(0.0)),
                                    ),
                                );
                                window.paint_quad(quad(
                                    pb,
                                    px(0.),
                                    pc,
                                    px(0.),
                                    pc,
                                    BorderStyle::default(),
                                ));
                                let doc_run = |len: usize, color: u32| TextRun {
                                    len,
                                    font: gpui::font(theme::FONT_FAMILY_SANS),
                                    color: rgb(color).into(),
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                for bt in &item.doc_rows {
                                    let runs: Vec<TextRun> = bt
                                        .runs
                                        .iter()
                                        .map(|&(len, color)| doc_run(len, color))
                                        .collect();
                                    let line = window.text_system().shape_line(
                                        bt.text.clone().into(),
                                        px(FONT_PX as f32),
                                        &runs,
                                        None,
                                    );
                                    let _ = line.paint(
                                        point(origin.x + px(bt.x), origin.y + px(bt.y)),
                                        px(FONT_PX as f32 * 1.3),
                                        TextAlign::Left,
                                        None,
                                        window,
                                        _cx,
                                    );
                                }
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
                        // Pass 3: focus + neighbor rings on top of everything,
                        // so child boxes, text, and headers never occlude them.
                        for item in &items {
                            if !item.focused && !item.neighbor {
                                continue;
                            }
                            let b = Bounds::new(
                                point(origin.x + px(item.x), origin.y + px(item.y)),
                                size(px(item.w), px(item.h)),
                            );
                            let (bw, bc) = if item.focused {
                                (2.0, rgb(theme::FOCUS_BORDER))
                            } else {
                                (1.0, rgba(theme::NEIGHBOR_BORDER))
                            };
                            window.paint_quad(quad(
                                b,
                                px(theme::CORNER_RADIUS),
                                transparent_black(),
                                px(bw),
                                bc,
                                BorderStyle::default(),
                            ));
                        }
                        // Pass 4: focused-leaf doc panel (floats to the right).
                        if let Some(dp) = &doc_panel {
                            let pb = Bounds::new(
                                point(origin.x + px(dp.x), origin.y + px(dp.y)),
                                size(px(dp.w), px(dp.h)),
                            );
                            window.paint_quad(quad(
                                pb,
                                px(theme::CORNER_RADIUS),
                                rgb(theme::CODE_BG),
                                px(1.0),
                                rgb(theme::FOCUS_BORDER),
                                BorderStyle::default(),
                            ));
                            let doc_run = |len: usize, color: u32| TextRun {
                                len,
                                font: gpui::font(theme::FONT_FAMILY_SANS),
                                color: rgb(color).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            for bt in &dp.rows {
                                let runs: Vec<TextRun> = bt
                                    .runs
                                    .iter()
                                    .map(|&(len, color)| doc_run(len, color))
                                    .collect();
                                let line = window.text_system().shape_line(
                                    bt.text.clone().into(),
                                    px(FONT_PX as f32),
                                    &runs,
                                    None,
                                );
                                let _ = line.paint(
                                    point(origin.x + px(bt.x), origin.y + px(bt.y)),
                                    px(FONT_PX as f32 * 1.3),
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
        code_line, container_body, doc_overlay, leaf_tex_rect, leaf_text_body, runs_from_spans,
        truncate_to_width, wrap_doc, BODY_PAD, HEADER, LINE_STEP,
    };
    use crate::buffers::BufferManager;
    use crate::world::{self, PxRect, Rung};

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
    fn leaf_tex_rect_covers_the_line_area() {
        // 10-line leaf drawn at half its natural height.
        let leaf = node(SymbolKind::Item { label: "fn".into() }, "a.rs::f", Some(0..100), 10, Some("fn f()"), None);
        let natural = crate::content::natural_px(&leaf);
        let full_h = natural * 0.5;
        let (x, y, w, h) = leaf_tex_rect(&leaf, 100.0, 50.0, full_h);
        assert!((x - 100.0).abs() < 1e-9);
        // Scale < 1 → the content starts below the unscaled header band,
        // exactly where leaf_text_body puts row 0.
        assert!((y - (50.0 + HEADER)).abs() < 1e-9);
        assert!((w - world::PAGE_W * 0.5).abs() < 1e-9);
        assert!((h - 10.0 * LINE_STEP * 0.5).abs() < 1e-9);
    }

    use super::{inset_top, pinned_stack_h};
    use crate::camera::Camera;
    use crate::focus::TreeIndex;
    use outrider_index::SymbolTree;
    use outrider_layout::{PackLayout, Rect};

    fn screen_y(cam: &Camera, wy: f64, vh: f64) -> f64 {
        (wy - cam.center_y) * cam.zoom + vh / 2.0
    }

    #[test]
    fn inset_top_centers_rect_in_the_band_below_the_inset() {
        let r = Rect { x: 0.0, y: 7.0, w: 100.0, h: 20.0 };
        let cam = Camera { center_x: 0.0, center_y: 0.0, zoom: 2.0 };
        // band [20, 100], rect 20·2 = 40 tall → top at 20 + (80 − 40)/2 = 40
        let c = inset_top(cam, r, 20.0, 100.0);
        assert!((screen_y(&c, r.y, 100.0) - 40.0).abs() < 1e-9);
        assert_eq!(c.zoom, cam.zoom); // vertical shift only
    }

    #[test]
    fn inset_top_pins_to_band_top_when_rect_is_taller_than_the_band() {
        let r = Rect { x: 0.0, y: 7.0, w: 100.0, h: 90.0 };
        let cam = Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 };
        let c = inset_top(cam, r, 20.0, 100.0);
        assert!((screen_y(&c, r.y, 100.0) - 20.0).abs() < 1e-9);
    }

    fn named(kind: SymbolKind, qual: &str, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode { name: name.into(), children, ..node(kind, qual, None, 1, None, None) }
    }

    /// root { mid { anon(unnamed) { f } } } with rects far above the viewport.
    fn stack_fixture() -> (SymbolTree, PackLayout, SymbolId) {
        let leaf = named(SymbolKind::Item { label: "fn".into() }, "r/m/a/f", "f", vec![]);
        let focus = leaf.id.clone();
        let anon = named(SymbolKind::Folder, "r/m/a", "", vec![leaf]);
        let anon_id = anon.id.clone();
        let mid = named(SymbolKind::Folder, "r/m", "mid", vec![anon]);
        let mid_id = mid.id.clone();
        let root = named(SymbolKind::Folder, "r", "root", vec![mid]);
        let mut rects = BTreeMap::new();
        rects.insert(root.id.clone(), Rect { x: 0.0, y: -1000.0, w: 4000.0, h: 4000.0 });
        rects.insert(mid_id, Rect { x: 10.0, y: -900.0, w: 3000.0, h: 3000.0 });
        rects.insert(anon_id, Rect { x: 20.0, y: -800.0, w: 2000.0, h: 2000.0 });
        rects.insert(focus.clone(), Rect { x: 30.0, y: 0.0, w: 480.0, h: 200.0 });
        let tree = SymbolTree { root, repo_root: std::path::PathBuf::from("/x") };
        (tree, PackLayout { rects }, focus)
    }

    #[test]
    fn pinned_stack_h_stacks_named_offscreen_ancestors_and_skips_unnamed() {
        let (tree, layout, focus) = stack_fixture();
        let index = TreeIndex::new(&tree);
        // Both named ancestors' tops are above the viewport → each pins at
        // the top and stacks; the unnamed folder contributes nothing.
        let cam = Camera { center_x: 0.0, center_y: 0.0, zoom: 1.0 };
        let h = pinned_stack_h(&focus, &layout, &index, &cam, 800.0, 600.0);
        let hdr = HEADER + 2.0 * LINE_STEP;
        assert!((h - 2.0 * hdr).abs() < 1e-9);
        // Header height scales with zoom below 1.
        let cam = Camera { center_x: 0.0, center_y: 0.0, zoom: 0.5 };
        let h = pinned_stack_h(&focus, &layout, &index, &cam, 800.0, 600.0);
        assert!((h - hdr).abs() < 1e-9);
    }

    #[test]
    fn pinned_stack_h_pins_on_screen_ancestors_at_their_own_top() {
        let (tree, layout, focus) = stack_fixture();
        let index = TreeIndex::new(&tree);
        // root top on screen at 50, mid top at 150 (clear of root's header)
        // → stack bottom is mid's top plus one header.
        let cam = Camera { center_x: 0.0, center_y: -750.0, zoom: 1.0 };
        let vh = 600.0;
        assert!((screen_y(&cam, -1000.0, vh) - 50.0).abs() < 1e-9);
        let h = pinned_stack_h(&focus, &layout, &index, &cam, 800.0, vh);
        let hdr = HEADER + 2.0 * LINE_STEP;
        assert!((h - (150.0 + hdr)).abs() < 1e-9);
    }

    /// w_px giving exactly `budget` chars: budget = (w - 12) / (0.62 * 12).
    fn wrap_w(budget: usize) -> f64 {
        12.0 + budget as f64 * 0.62 * 12.0
    }

    #[test]
    fn wrap_doc_fits_short_text_on_one_row() {
        assert_eq!(wrap_doc("hello world", wrap_w(11), 12.0), vec!["hello world"]);
    }

    #[test]
    fn wrap_doc_greedy_wraps_at_word_boundaries() {
        assert_eq!(
            wrap_doc("alpha beta gamma", wrap_w(10), 12.0),
            vec!["alpha beta", "gamma"]
        );
    }

    #[test]
    fn wrap_doc_hard_splits_over_budget_words() {
        assert_eq!(
            wrap_doc("abcdefghijklmnopqrstuvwxy", wrap_w(10), 12.0),
            vec!["abcdefghij", "klmnopqrst", "uvwxy"]
        );
    }

    #[test]
    fn wrap_doc_joins_lines_within_a_paragraph() {
        assert_eq!(
            wrap_doc("first line\nsecond line", wrap_w(40), 12.0),
            vec!["first line second line"]
        );
    }

    #[test]
    fn wrap_doc_breaks_paragraphs_on_blank_lines() {
        assert_eq!(
            wrap_doc("para one\n\npara two", wrap_w(40), 12.0),
            vec!["para one", "para two"]
        );
    }

    #[test]
    fn wrap_doc_returns_nothing_when_no_room() {
        assert!(wrap_doc("anything", 13.0, 12.0).is_empty());
    }

    #[test]
    fn doc_overlay_shows_rows_when_text_fits() {
        // Box fits exactly 2 rows and text wraps to exactly 2 rows.
        let px = crate::world::PxRect {
            x: 100.0,
            y: 50.0,
            w: wrap_w(10) + 2.0 * BODY_PAD,
            h: HEADER + 2.0 * LINE_STEP,
        };
        let (rows, panel_h) = doc_overlay("alpha beta gamma", &px);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].x, (100.0 + BODY_PAD) as f32);
        assert_eq!(rows[0].y, (50.0 + HEADER) as f32);
        assert_eq!(rows[1].y, (50.0 + HEADER + LINE_STEP) as f32);
        assert_eq!(panel_h, (HEADER + 2.0 * LINE_STEP) as f32);
        assert_eq!(rows[0].runs, vec![(rows[0].text.len(), crate::theme::DOC_COLOR)]);
    }

    #[test]
    fn doc_overlay_hides_entirely_when_text_overflows() {
        // Box fits 2 rows but text wraps to 3+: overlay is hidden.
        let px = crate::world::PxRect {
            x: 100.0,
            y: 50.0,
            w: wrap_w(10) + 2.0 * BODY_PAD,
            h: HEADER + 2.0 * LINE_STEP,
        };
        let (rows, panel_h) = doc_overlay("alpha beta gamma delta epsilon", &px);
        assert!(rows.is_empty());
        assert_eq!(panel_h, 0.0);
    }

    #[test]
    fn doc_overlay_is_empty_when_no_row_fits() {
        let px = crate::world::PxRect { x: 0.0, y: 0.0, w: 200.0, h: HEADER };
        let (rows, panel_h) = doc_overlay("some description", &px);
        assert!(rows.is_empty());
        assert_eq!(panel_h, 0.0);
    }
}
