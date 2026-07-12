//! Bakes far-zoom leaf source text into low-res BGRA images with a CPU
//! mip chain (spec: docs/superpowers/specs/2026-07-11-texture-leaf-rendering-design.md).

use std::hash::{Hash, Hasher};
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::Arc;

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

use crate::content::LINE_STEP;
use crate::theme;
use crate::treemap::BODY_PAD;
use crate::world;

/// Master mip level line height, px. Raise for crisper near-threshold text.
pub const MASTER_LINE_PX: f64 = 4.0;
/// Master texture height cap; taller leaves stride rows to fit.
pub const MAX_TEX_H: usize = 1024;
/// Downsample until a level is at most this tall.
pub const MIN_LEVEL_H: i32 = 8;

/// One source line: text plus colored runs (byte length, 0xRRGGBB) — the
/// same shape `treemap::runs_from_spans` produces for the Text tier.
pub type Line = (String, Vec<(usize, u32)>);

/// A baked leaf: mip levels ordered largest→smallest, plus byte total for
/// cache accounting. Empty when the leaf had no source lines.
pub struct LeafTexture {
    pub levels: Vec<Arc<RenderImage>>,
    pub bytes: usize,
}

/// Mip-level selection and empty-check helpers.
impl LeafTexture {
    /// The level to paint at `screen_h` on-screen pixels, or None if empty.
    pub fn level_for(&self, screen_h: f32) -> Option<&Arc<RenderImage>> {
        if self.levels.is_empty() {
            return None;
        }
        let heights: Vec<u32> =
            self.levels.iter().map(|l| l.size(0).height.0 as u32).collect();
        Some(&self.levels[pick_level(&heights, screen_h)])
    }
}

/// Index of the smallest level (heights ordered largest→smallest) whose
/// height still covers `screen_h`; clamps to the last level below that.
pub fn pick_level(heights: &[u32], screen_h: f32) -> usize {
    let mut best = 0;
    for (i, &lh) in heights.iter().enumerate() {
        if lh as f32 >= screen_h {
            best = i;
        } else {
            break;
        }
    }
    best
}

/// Alpha-weighted 2×2 box downsample of a straight-alpha RGBA buffer.
/// Weighting by alpha avoids transparent texels dragging colors dark.
#[allow(clippy::manual_checked_ops)] // guard pattern is clearer than checked_div
pub(crate) fn downsample(src: &[u8], w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let nw = (w / 2).max(1);
    let nh = (h / 2).max(1);
    let mut out = vec![0u8; (nw * nh) as usize * 4];
    for oy in 0..nh {
        for ox in 0..nw {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for dy in 0..2 {
                for dx in 0..2 {
                    let sx = (ox * 2 + dx).min(w - 1);
                    let sy = (oy * 2 + dy).min(h - 1);
                    let i = ((sy * w + sx) * 4) as usize;
                    let pa = src[i + 3] as u32;
                    r += src[i] as u32 * pa;
                    g += src[i + 1] as u32 * pa;
                    b += src[i + 2] as u32 * pa;
                    a += pa;
                }
            }
            let o = ((oy * nw + ox) * 4) as usize;
            if a > 0 {
                out[o] = (r / a) as u8;
                out[o + 1] = (g / a) as u8;
                out[o + 2] = (b / a) as u8;
            }
            out[o + 3] = (a / 4) as u8;
        }
    }
    (nw, nh, out)
}

/// Straight-alpha src-over blend of one RGBA pixel.
fn blend(dst: &mut [u8], r: u8, g: u8, b: u8, a: u8) {
    let sa = a as u32;
    let da = dst[3] as u32;
    let oa = sa + da * (255 - sa) / 255;
    if oa == 0 {
        return;
    }
    let src = [r, g, b];
    for i in 0..3 {
        let sc = src[i] as u32;
        let dc = dst[i] as u32;
        dst[i] = ((sc * sa + dc * da * (255 - sa) / 255) / oa) as u8;
    }
    dst[3] = oa as u8;
}

/// CPU rasterizer: holds a `cosmic-text` font system and glyph cache,
/// reused across bake calls to avoid per-frame font loading overhead.
pub struct Rasterizer {
    font_system: FontSystem,
    swash: SwashCache,
}

/// Construction and the `bake` pipeline.
impl Rasterizer {
    /// Create a rasterizer with a fresh font system; loads system fonts lazily.
    pub fn new() -> Self {
        Self { font_system: FontSystem::new(), swash: SwashCache::new() }
    }

    /// Rasterize `lines` at MASTER_LINE_PX per line (strided so the master
    /// never exceeds MAX_TEX_H), then box-downsample the mip chain.
    pub fn bake(&mut self, lines: &[Line]) -> LeafTexture {
        if lines.is_empty() {
            return LeafTexture { levels: Vec::new(), bytes: 0 };
        }
        let stride = lines.len().div_ceil(MAX_TEX_H).max(1);
        let rows: Vec<&Line> = lines.iter().step_by(stride).collect();
        let l = MASTER_LINE_PX.min(MAX_TEX_H as f64 / rows.len() as f64);
        let h = ((rows.len() as f64 * l).ceil() as u32).max(1);
        let w = ((world::PAGE_W / LINE_STEP * l).round() as u32).max(1);
        let pad = (BODY_PAD / LINE_STEP * l).round() as i32;
        let font_size = (l / 1.3) as f32;

        // One cosmic buffer holds every row, newline-separated; runs map
        // 1:1 onto rich-text spans. Runs from runs_from_spans always cover
        // the full line, but clamp defensively for hand-built inputs.
        let mut text = String::new();
        let mut spans: Vec<(usize, usize, Option<u32>)> = Vec::new();
        for (line, runs) in &rows {
            let mut pos = 0;
            for &(len, color) in runs {
                let end = (pos + len).min(line.len());
                if end > pos {
                    let s = text.len() + pos;
                    spans.push((s, s + (end - pos), Some(color)));
                }
                pos = end;
            }
            if pos < line.len() {
                let s = text.len() + pos;
                spans.push((s, s + (line.len() - pos), None));
            }
            text.push_str(line);
            let nl = text.len();
            text.push('\n');
            spans.push((nl, nl + 1, None));
        }

        let attrs = |color: Option<u32>| {
            let a = Attrs::new().family(Family::Name(theme::FONT_FAMILY));
            match color {
                Some(c) => a.color(ct_color(c)),
                None => a,
            }
        };
        let mut buffer =
            Buffer::new(&mut self.font_system, Metrics::new(font_size, l as f32));
        buffer.set_size(Some(w as f32), Some(h as f32));
        buffer.set_wrap(Wrap::None);
        buffer.set_rich_text(
            spans.iter().map(|&(s, e, c)| (&text[s..e], attrs(c))),
            &attrs(None),
            Shaping::Basic,
            None,
        );

        let mut rgba = vec![0u8; (w * h) as usize * 4];
        buffer.draw(
            &mut self.font_system,
            &mut self.swash,
            ct_color(theme::TEXT_PRIMARY),
            |x, y, rw, rh, color| {
                let a = color.a();
                if a == 0 {
                    return;
                }
                let (r, g, b) = (color.r(), color.g(), color.b());
                for yy in y.max(0)..(y + rh as i32).min(h as i32) {
                    for xx in (x + pad).max(0)..(x + pad + rw as i32).min(w as i32) {
                        let i = ((yy as u32 * w + xx as u32) * 4) as usize;
                        blend(&mut rgba[i..i + 4], r, g, b, a);
                    }
                }
            },
        );

        let mut levels = Vec::new();
        let mut bytes = 0usize;
        let (mut cw, mut ch, mut cur) = (w, h, rgba);
        loop {
            let mut bgra = cur.clone();
            for p in bgra.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
            bytes += bgra.len();
            let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(cw, ch, bgra)
                .expect("buffer sized to cw*ch*4");
            levels.push(Arc::new(RenderImage::new(vec![Frame::new(img)])));
            if ch as i32 <= MIN_LEVEL_H || cw <= 1 {
                break;
            }
            (cw, ch, cur) = {
                let (nw, nh, next) = downsample(&cur, cw, ch);
                (nw, nh, next)
            };
        }
        LeafTexture { levels, bytes }
    }
}

/// 0xRRGGBB → cosmic-text Color.
fn ct_color(c: u32) -> Color {
    Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

use outrider_index::{SymbolId, SymbolKind, SymbolNode};
use outrider_layout::{PackLayout, Rect};

use std::collections::HashMap;

/// Maximum pixel dimension (longer side) for a folder thumbnail texture.
const FOLDER_TEX_MAX: f64 = 256.0;

/// Rasterize a container's subtree into a mipped texture. Leaf children
/// with an already-cached text texture get composited at the target
/// resolution; others fall back to themed colored rectangles.
pub fn bake_folder(
    node: &SymbolNode,
    container_rect: Rect,
    layout: &PackLayout,
    base_level: u8,
    leaf_tex: &impl Fn(&SymbolId) -> Option<Vec<u8>>,
) -> LeafTexture {
    if node.children.is_empty() || container_rect.w < 1.0 || container_rect.h < 1.0 {
        return LeafTexture { levels: Vec::new(), bytes: 0 };
    }
    let aspect = container_rect.w / container_rect.h;
    let (tw, th) = if aspect >= 1.0 {
        (FOLDER_TEX_MAX as u32, (FOLDER_TEX_MAX / aspect).ceil().max(1.0) as u32)
    } else {
        ((FOLDER_TEX_MAX * aspect).ceil().max(1.0) as u32, FOLDER_TEX_MAX as u32)
    };
    let sx = tw as f64 / container_rect.w;
    let sy = th as f64 / container_rect.h;

    let mut rgba = vec![0u8; (tw as usize) * (th as usize) * 4];
    folder_fill(node, &container_rect, layout, sx, sy, tw, th, &mut rgba, base_level + 1, leaf_tex);

    for p in rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
    }

    let mut levels = Vec::new();
    let mut bytes = 0usize;
    let (mut cw, mut ch, mut cur) = (tw, th, rgba);
    loop {
        bytes += cur.len();
        let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(cw, ch, cur.clone())
            .expect("buffer sized to cw*ch*4");
        levels.push(Arc::new(RenderImage::new(vec![Frame::new(img)])));
        if ch as i32 <= MIN_LEVEL_H || cw <= 1 {
            break;
        }
        (cw, ch, cur) = downsample(&cur, cw, ch);
    }
    LeafTexture { levels, bytes }
}

/// Recursively fill descendant rectangles into the RGBA buffer.
/// For leaf children with a cached texture, composites the BGRA pixels
/// (scaled to fit the destination rect) on top of the fill.
fn folder_fill(
    node: &SymbolNode,
    root: &Rect,
    layout: &PackLayout,
    sx: f64,
    sy: f64,
    tw: u32,
    th: u32,
    rgba: &mut [u8],
    level: u8,
    leaf_tex: &impl Fn(&SymbolId) -> Option<Vec<u8>>,
) {
    for child in &node.children {
        let Some(r) = layout.rects.get(&child.id) else { continue };
        let px = ((r.x - root.x) * sx) as i32;
        let py = ((r.y - root.y) * sy) as i32;
        let pw = (r.w * sx).max(1.0).ceil() as i32;
        let ph = (r.h * sy).max(1.0).ceil() as i32;

        let is_leaf = child.byte_range.is_some()
            && child.children.is_empty()
            && child.id.kind != SymbolKind::Folder;
        let kind = if is_leaf {
            theme::BoxKind::Leaf
        } else if child.id.kind == SymbolKind::Folder {
            theme::BoxKind::Folder
        } else {
            theme::BoxKind::File
        };
        let tint = match &child.id.kind {
            SymbolKind::Folder => match child.name.as_str() {
                "docs" | "doc" | "documentation" => theme::BoxTint::DocsFolder,
                "test" | "tests" | "spec" | "specs" | "__tests__" => theme::BoxTint::TestFolder,
                _ => theme::BoxTint::Normal,
            },
            SymbolKind::Item { label } => match label.as_str() {
                "struct" | "enum" | "trait" | "class" | "interface" | "type" | "typedef" => {
                    theme::BoxTint::TypeDef
                }
                _ => theme::BoxTint::Normal,
            },
            _ => theme::BoxTint::Normal,
        };
        let fill = theme::box_fill(kind, level, tint);
        let border = theme::border_for(fill);
        let (fr, fg, fb) = rgb_u8(fill);
        let (br, bg, bb) = rgb_u8(border);

        for y in py.max(0)..(py + ph).min(th as i32) {
            for x in px.max(0)..(px + pw).min(tw as i32) {
                let i = (y as u32 * tw + x as u32) as usize * 4;
                let on_border = x == px || x == px + pw - 1 || y == py || y == py + ph - 1;
                if on_border {
                    rgba[i] = br;
                    rgba[i + 1] = bg;
                    rgba[i + 2] = bb;
                } else {
                    rgba[i] = fr;
                    rgba[i + 1] = fg;
                    rgba[i + 2] = fb;
                }
                rgba[i + 3] = 255;
            }
        }

        if is_leaf {
            if let Some(src_bgra) = leaf_tex(&child.id) {
                composite_leaf(
                    &src_bgra, pw as u32, ph as u32,
                    px, py, tw, th, rgba,
                );
            }
        }

        if child.churn > 0.0 {
            let heat = theme::churn_heat(child.churn);
            let (hr, hg, hb) = rgb_u8(heat);
            let sw = ((theme::STRIPE_W as f64 * sx / 4.0).ceil() as i32).max(1);
            for y in (py + 1).max(0)..(py + ph - 1).min(th as i32) {
                for x in (px + 1).max(0)..(px + 1 + sw).min((px + pw - 1).min(tw as i32)) {
                    let i = (y as u32 * tw + x as u32) as usize * 4;
                    rgba[i] = hr;
                    rgba[i + 1] = hg;
                    rgba[i + 2] = hb;
                }
            }
        }

        if !child.children.is_empty() {
            folder_fill(child, root, layout, sx, sy, tw, th, rgba, level.saturating_add(1), leaf_tex);
        }
    }
}

/// Nearest-neighbor scale a BGRA source into the destination RGBA buffer,
/// blending non-transparent pixels via src-over. The source bytes are
/// from a RenderImage which stores BGRA; we swap channels on read.
fn composite_leaf(
    src_bgra: &[u8],
    dst_w: u32,
    dst_h: u32,
    dst_x: i32,
    dst_y: i32,
    buf_w: u32,
    buf_h: u32,
    rgba: &mut [u8],
) {
    if src_bgra.len() < 4 {
        return;
    }
    let src_px = src_bgra.len() / 4;
    let src_w = (src_px as f64).sqrt().ceil() as u32;
    let src_h = if src_w > 0 { src_px as u32 / src_w } else { return };
    if src_w == 0 || src_h == 0 {
        return;
    }
    let inner_x = dst_x + 1;
    let inner_y = dst_y + 1;
    let inner_w = (dst_w as i32 - 2).max(0) as u32;
    let inner_h = (dst_h as i32 - 2).max(0) as u32;
    if inner_w == 0 || inner_h == 0 {
        return;
    }
    for oy in 0..inner_h {
        let dy = inner_y + oy as i32;
        if dy < 0 || dy >= buf_h as i32 {
            continue;
        }
        let sy = (oy as u64 * src_h as u64 / inner_h as u64) as u32;
        for ox in 0..inner_w {
            let dx = inner_x + ox as i32;
            if dx < 0 || dx >= buf_w as i32 {
                continue;
            }
            let sx = (ox as u64 * src_w as u64 / inner_w as u64) as u32;
            let si = (sy * src_w + sx) as usize * 4;
            if si + 3 >= src_bgra.len() {
                continue;
            }
            let a = src_bgra[si + 3];
            if a == 0 {
                continue;
            }
            let di = (dy as u32 * buf_w + dx as u32) as usize * 4;
            blend(
                &mut rgba[di..di + 4],
                src_bgra[si + 2], // B→R (BGRA→RGBA)
                src_bgra[si + 1],
                src_bgra[si],     // R→B
                a,
            );
        }
    }
}

fn rgb_u8(c: u32) -> (u8, u8, u8) {
    ((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Bakes per frame; keeps zoom-out pop-in bounded without stalling a frame.
pub const BAKES_PER_FRAME: usize = 4;
/// Default in-memory texture budget (256 MB), used by tests; production
/// reads from `Settings::cache_mb`.
#[cfg(test)]
const DEFAULT_CACHE_MB: u32 = 256;

/// Single cache slot: baked texture plus a logical clock tick for LRU ordering.
struct Entry {
    tex: LeafTexture,
    last_used: u64,
}

fn disk_key(id: &SymbolId) -> String {
    let mut h = std::hash::DefaultHasher::new();
    id.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn save_to_disk(dir: &std::path::Path, id: &SymbolId, tex: &LeafTexture) {
    if tex.levels.is_empty() {
        return;
    }
    let path = dir.join(format!("{}.tex", disk_key(id)));
    let Ok(mut f) = std::fs::File::create(&path) else { return };
    let n = tex.levels.len() as u32;
    let _ = f.write_all(&n.to_le_bytes());
    for level in &tex.levels {
        let sz = level.size(0);
        let w = sz.width.0 as u32;
        let h = sz.height.0 as u32;
        let _ = f.write_all(&w.to_le_bytes());
        let _ = f.write_all(&h.to_le_bytes());
        if let Some(bytes) = level.as_bytes(0) {
            let len = bytes.len() as u32;
            let _ = f.write_all(&len.to_le_bytes());
            let _ = f.write_all(bytes);
        } else {
            let _ = f.write_all(&0u32.to_le_bytes());
        }
    }
}

fn load_from_disk(dir: &std::path::Path, id: &SymbolId) -> Option<LeafTexture> {
    let path = dir.join(format!("{}.tex", disk_key(id)));
    let mut f = std::fs::File::open(&path).ok()?;
    let mut buf4 = [0u8; 4];
    f.read_exact(&mut buf4).ok()?;
    let n = u32::from_le_bytes(buf4) as usize;
    if n == 0 || n > 20 {
        return None;
    }
    let mut levels = Vec::with_capacity(n);
    let mut bytes = 0usize;
    for _ in 0..n {
        f.read_exact(&mut buf4).ok()?;
        let w = u32::from_le_bytes(buf4);
        f.read_exact(&mut buf4).ok()?;
        let h = u32::from_le_bytes(buf4);
        f.read_exact(&mut buf4).ok()?;
        let len = u32::from_le_bytes(buf4) as usize;
        if len == 0 || w == 0 || h == 0 {
            return None;
        }
        let mut data = vec![0u8; len];
        f.read_exact(&mut data).ok()?;
        bytes += data.len();
        let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, data)?;
        levels.push(Arc::new(RenderImage::new(vec![Frame::new(img)])));
    }
    Some(LeafTexture { levels, bytes })
}

/// Per-leaf texture cache: misses queue during the item pass, then
/// `bake_queued` bakes the largest few and LRU-evicts past the budget.
/// Textures are persisted to disk so evicted entries can be reloaded
/// without re-rendering.
pub struct TextureCache {
    raster: Rasterizer,
    entries: HashMap<SymbolId, Entry>,
    clock: u64,
    bytes: usize,
    max_bytes: usize,
    queue: Vec<(SymbolId, f64)>,
    retired: Vec<Arc<RenderImage>>,
    disk_dir: Option<PathBuf>,
}

/// LRU cache management, miss queuing, and frame-gated baking.
impl TextureCache {
    /// Create a cache with `max_bytes` total budget across all mip levels.
    pub fn new(max_bytes: usize) -> Self {
        let disk_dir = dirs::cache_dir().map(|d| {
            let p = d.join("outrider").join("textures");
            let _ = std::fs::create_dir_all(&p);
            p
        });
        Self {
            raster: Rasterizer::new(),
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
            queue: Vec::new(),
            retired: Vec::new(),
            disk_dir,
        }
    }

    /// True if a texture for `id` is already cached (does not queue).
    pub fn contains(&self, id: &SymbolId) -> bool {
        self.entries.contains_key(id)
    }

    /// Cache lookup. A hit refreshes LRU recency; a miss checks the disk
    /// cache first, then queues the leaf for `bake_queued`.
    pub fn get(&mut self, id: &SymbolId, screen_area: f64) -> Option<&LeafTexture> {
        self.clock += 1;
        if self.entries.contains_key(id) {
            let e = self.entries.get_mut(id).unwrap();
            e.last_used = self.clock;
            return Some(&e.tex);
        }
        if let Some(dir) = &self.disk_dir {
            if let Some(tex) = load_from_disk(dir, id) {
                self.bytes += tex.bytes;
                self.entries.insert(id.clone(), Entry { tex, last_used: self.clock });
                return Some(&self.entries.get(id).unwrap().tex);
            }
        }
        self.queue.push((id.clone(), screen_area));
        None
    }

    /// True when at least one miss is queued and `bake_queued` should be called.
    pub fn has_queued(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Snapshot of the smallest mip-level BGRA bytes for all cached entries.
    /// Used by `bake_folder` to composite leaf text into folder thumbnails.
    pub fn leaf_bytes_snapshot(&self) -> HashMap<SymbolId, Vec<u8>> {
        self.entries
            .iter()
            .filter_map(|(id, e)| {
                let level = e.tex.levels.last()?;
                let bytes = level.as_bytes(0)?;
                Some((id.clone(), bytes.to_vec()))
            })
            .collect()
    }

    /// Bake up to BAKES_PER_FRAME queued items, largest on screen first,
    /// then evict LRU entries past the byte budget. The callback receives
    /// the `SymbolId` and a `&mut Rasterizer` and returns a ready-to-cache
    /// `LeafTexture` (or `None` for negative caching). Returns whether
    /// misses remain (the caller schedules a repaint so they bake next frame).
    pub fn bake_queued(
        &mut self,
        mut bake_fn: impl FnMut(&SymbolId, &mut Rasterizer) -> Option<LeafTexture>,
    ) -> bool {
        self.queue.sort_by(|a, b| b.1.total_cmp(&a.1));
        let queue = std::mem::take(&mut self.queue);
        let mut it = queue.into_iter();
        for (id, _) in it.by_ref().take(BAKES_PER_FRAME) {
            let tex = bake_fn(&id, &mut self.raster)
                .unwrap_or_else(|| LeafTexture { levels: Vec::new(), bytes: 0 });
            if let Some(dir) = &self.disk_dir {
                save_to_disk(dir, &id, &tex);
            }
            self.bytes += tex.bytes;
            self.clock += 1;
            self.entries.insert(id, Entry { tex, last_used: self.clock });
        }
        let remaining = it.next().is_some();
        while self.bytes > self.max_bytes {
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            let e = self.entries.remove(&victim).unwrap();
            self.bytes -= e.tex.bytes;
            self.retired.extend(e.tex.levels);
        }
        remaining
    }

    /// Evicted images, for the caller to hand to `window.drop_image` so
    /// atlas memory is actually reclaimed.
    pub fn take_retired(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.retired)
    }

    /// Wipe the on-disk texture cache (e.g. when opening a new folder).
    pub fn clear_disk_cache(&self) {
        if let Some(dir) = &self.disk_dir {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("tex") {
                        let _ = std::fs::remove_file(p);
                    }
                }
            }
        }
    }

    #[cfg(test)]
    fn new_memory_only(max_bytes: usize) -> Self {
        Self {
            raster: Rasterizer::new(),
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
            queue: Vec::new(),
            retired: Vec::new(),
            disk_dir: None,
        }
    }

    #[cfg(test)]
    fn set_max_bytes_for_test(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Line {
        (text.to_string(), vec![(text.len(), 0xFF0000)])
    }

    #[test]
    fn pick_level_takes_smallest_covering_height() {
        let heights = [40, 20, 10, 5];
        assert_eq!(pick_level(&heights, 50.0), 0); // bigger than all: master
        assert_eq!(pick_level(&heights, 12.0), 1); // 20 covers, 10 doesn't
        assert_eq!(pick_level(&heights, 10.0), 2); // exact cover
        assert_eq!(pick_level(&heights, 3.0), 3);  // smaller than all: last
    }

    #[test]
    fn downsample_is_alpha_weighted_2x2_average() {
        // 2x2 RGBA: one opaque red pixel, three transparent.
        let src = [255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let (w, h, out) = downsample(&src, 2, 2);
        assert_eq!((w, h), (1, 1));
        // Color is alpha-weighted (pure red survives), alpha is the mean.
        assert_eq!(&out, &[255, 0, 0, 63]);
    }

    #[test]
    fn bake_dimensions_and_mip_chain() {
        let lines: Vec<Line> = (0..10).map(|_| plain("fn foo() {}")).collect();
        let tex = Rasterizer::new().bake(&lines);
        // L=4 → 40px tall; width = round(PAGE_W/LINE_STEP*4) = 123.
        let dims: Vec<(i32, i32)> = tex
            .levels
            .iter()
            .map(|l| (l.size(0).width.0, l.size(0).height.0))
            .collect();
        assert_eq!(dims, vec![(123, 40), (61, 20), (30, 10), (15, 5)]);
        assert_eq!(tex.bytes, (123 * 40 + 61 * 20 + 30 * 10 + 15 * 5) * 4);
    }

    #[test]
    fn bake_strides_huge_leaves_to_height_cap() {
        let lines: Vec<Line> = (0..3000).map(|_| plain("x")).collect();
        let tex = Rasterizer::new().bake(&lines);
        // stride = ceil(3000/1024) = 3 → 1000 rows; L = 1024/1000 = 1.024.
        assert_eq!(tex.levels[0].size(0).height.0, 1024);
    }

    #[test]
    fn bake_renders_glyphs_in_bgra() {
        // Red runs; if the font resolved and channels are BGRA, every
        // covered pixel is red-dominant: byte[2] (R) >= byte[0] (B).
        let lines: Vec<Line> = (0..8).map(|_| plain("MMMMMMMMMM")).collect();
        let tex = Rasterizer::new().bake(&lines);
        let bytes = tex.levels[0].as_bytes(0).unwrap();
        let covered: Vec<&[u8]> =
            bytes.chunks_exact(4).filter(|p| p[3] > 0).collect();
        assert!(!covered.is_empty(), "no glyph coverage — font not found?");
        assert!(covered.iter().all(|p| p[2] >= p[0]), "not BGRA red");
    }

    #[test]
    fn bake_is_deterministic() {
        let lines: Vec<Line> = (0..5).map(|_| plain("let x = 1;")).collect();
        let a = Rasterizer::new().bake(&lines);
        let b = Rasterizer::new().bake(&lines);
        assert_eq!(a.levels.len(), b.levels.len());
        for (la, lb) in a.levels.iter().zip(&b.levels) {
            assert_eq!(la.as_bytes(0), lb.as_bytes(0));
        }
    }

    #[test]
    fn empty_lines_produce_empty_texture() {
        let tex = Rasterizer::new().bake(&[]);
        assert!(tex.levels.is_empty());
        assert_eq!(tex.bytes, 0);
    }

    #[test]
    fn level_for_picks_by_screen_height() {
        let lines: Vec<Line> = (0..10).map(|_| plain("y")).collect();
        let tex = Rasterizer::new().bake(&lines);
        assert_eq!(tex.level_for(35.0).unwrap().size(0).height.0, 40);
        assert_eq!(tex.level_for(12.0).unwrap().size(0).height.0, 20);
        assert_eq!(tex.level_for(2.0).unwrap().size(0).height.0, 5);
        assert!(Rasterizer::new().bake(&[]).level_for(10.0).is_none());
    }

    use outrider_index::{SymbolId, SymbolKind};

    fn sid(name: &str) -> SymbolId {
        SymbolId {
            qualified_path: name.to_string(),
            kind: SymbolKind::Item { label: "fn".into() },
            ordinal: 0,
        }
    }

    fn some_tex(n: usize, raster: &mut Rasterizer) -> Option<LeafTexture> {
        let lines: Vec<Line> = (0..n).map(|_| plain("let x = 1;")).collect();
        Some(raster.bake(&lines))
    }

    #[test]
    fn cache_bakes_largest_first_within_budget() {
        let mut cache = TextureCache::new_memory_only(DEFAULT_CACHE_MB as usize * 1024 * 1024);
        for i in 0..6 {
            assert!(cache.get(&sid(&format!("l{i}")), (i + 1) as f64 * 10.0).is_none());
        }
        assert!(cache.has_queued());
        let remaining = cache.bake_queued(|_, r| some_tex(4, r));
        assert!(remaining, "6 queued, budget 4 — misses remain");
        for i in 2..6 {
            assert!(cache.get(&sid(&format!("l{i}")), 1.0).is_some());
        }
        assert!(cache.get(&sid("l0"), 1.0).is_none());
        assert!(cache.get(&sid("l1"), 1.0).is_none());
        assert!(!cache.bake_queued(|_, r| some_tex(4, r)));
        assert!(cache.get(&sid("l0"), 1.0).is_some());
    }

    #[test]
    fn cache_negative_caches_leaves_without_lines() {
        let mut cache = TextureCache::new_memory_only(DEFAULT_CACHE_MB as usize * 1024 * 1024);
        assert!(cache.get(&sid("nofile"), 1.0).is_none());
        assert!(!cache.bake_queued(|_, _| None));
        let tex = cache.get(&sid("nofile"), 1.0).expect("negative-cached");
        assert!(tex.levels.is_empty());
        assert!(!cache.has_queued());
    }

    #[test]
    fn cache_evicts_lru_and_retires_images() {
        let mut cache = TextureCache::new_memory_only(usize::MAX);
        cache.get(&sid("a"), 1.0);
        cache.bake_queued(|_, r| some_tex(4, r));
        let one = cache.get(&sid("a"), 1.0).unwrap().bytes;
        cache.set_max_bytes_for_test(one);
        cache.get(&sid("b"), 1.0);
        cache.bake_queued(|_, r| some_tex(4, r));
        assert!(cache.get(&sid("b"), 1.0).is_some());
        assert!(cache.get(&sid("a"), 1.0).is_none());
        let retired = cache.take_retired();
        assert!(!retired.is_empty(), "evicted levels are retired");
        assert!(cache.take_retired().is_empty(), "drained");
    }

    #[test]
    fn cache_hit_refreshes_lru_order() {
        let mut cache = TextureCache::new_memory_only(usize::MAX);
        cache.get(&sid("a"), 1.0);
        cache.bake_queued(|_, r| some_tex(4, r));
        cache.get(&sid("b"), 1.0);
        cache.bake_queued(|_, r| some_tex(4, r));
        let one = cache.get(&sid("a"), 1.0).unwrap().bytes;
        cache.set_max_bytes_for_test(2 * one);
        cache.get(&sid("c"), 1.0);
        cache.bake_queued(|_, r| some_tex(4, r));
        assert!(cache.get(&sid("a"), 1.0).is_some());
        assert!(cache.get(&sid("b"), 1.0).is_none());
        assert!(cache.get(&sid("c"), 1.0).is_some());
    }
}
