//! Bakes node textures: leaf text via cosmic-text, containers by compositing
//! cached child textures. One texture per node, GPU handles scaling.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::Arc;

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::{PackLayout, Rect};

use crate::buffers::{collect_file_symbols, BufferManager};
use crate::content::{self, LINE_STEP};
use crate::theme;
use crate::treemap::BODY_PAD;
use crate::world;

/// Master texture line height, px.
pub const MASTER_LINE_PX: f64 = 4.0;
/// Master texture height cap; taller leaves stride rows to fit.
pub const MAX_TEX_H: usize = 1024;
/// Maximum pixel dimension (longer side) for a container thumbnail.
const CONTAINER_TEX_MAX: f64 = 1024.0;

/// One source line: text plus colored runs (byte length, 0xRRGGBB).
pub type Line = (String, Vec<(usize, u32)>);

/// A baked node texture: single image, no mip chain. GPU handles scaling.
pub struct NodeTexture {
    pub image: Option<Arc<RenderImage>>,
    pub bytes: usize,
}

impl NodeTexture {
    pub fn empty() -> Self {
        Self { image: None, bytes: 0 }
    }
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

/// CPU rasterizer: holds a `cosmic-text` font system and glyph cache.
pub struct Rasterizer {
    font_system: FontSystem,
    swash: SwashCache,
}

impl Rasterizer {
    pub fn new() -> Self {
        Self { font_system: FontSystem::new(), swash: SwashCache::new() }
    }

    /// Rasterize `lines` into a single BGRA texture.
    pub fn bake(&mut self, lines: &[Line]) -> NodeTexture {
        if lines.is_empty() {
            return NodeTexture::empty();
        }
        let stride = lines.len().div_ceil(MAX_TEX_H).max(1);
        let rows: Vec<&Line> = lines.iter().step_by(stride).collect();
        let l = MASTER_LINE_PX.min(MAX_TEX_H as f64 / rows.len() as f64);
        let h = ((rows.len() as f64 * l).ceil() as u32).max(1);
        let w = ((world::PAGE_W / LINE_STEP * l).round() as u32).max(1);
        let pad = (BODY_PAD / LINE_STEP * l).round() as i32;
        let font_size = (l / 1.3) as f32;

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

        for p in rgba.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        let bytes = rgba.len();
        let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, rgba)
            .expect("buffer sized to w*h*4");
        NodeTexture {
            image: Some(Arc::new(RenderImage::new(vec![Frame::new(img)]))),
            bytes,
        }
    }
}

/// 0xRRGGBB → cosmic-text Color.
fn ct_color(c: u32) -> Color {
    Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Rasterize a container's subtree into a single texture. Composites
/// cached child textures where available, falls back to colored rects.
pub fn bake_container(
    node: &SymbolNode,
    container_rect: Rect,
    layout: &PackLayout,
    base_level: u8,
    child_tex: &impl Fn(&SymbolId) -> Option<Vec<u8>>,
) -> NodeTexture {
    if node.children.is_empty() || container_rect.w < 1.0 || container_rect.h < 1.0 {
        return NodeTexture::empty();
    }
    let aspect = container_rect.w / container_rect.h;
    let (tw, th) = if aspect >= 1.0 {
        (CONTAINER_TEX_MAX as u32, (CONTAINER_TEX_MAX / aspect).ceil().max(1.0) as u32)
    } else {
        ((CONTAINER_TEX_MAX * aspect).ceil().max(1.0) as u32, CONTAINER_TEX_MAX as u32)
    };
    let sx = tw as f64 / container_rect.w;
    let sy = th as f64 / container_rect.h;

    let mut rgba = vec![0u8; (tw as usize) * (th as usize) * 4];
    container_fill(node, &container_rect, layout, sx, sy, tw, th, &mut rgba, base_level + 1, child_tex);

    for p in rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let bytes = rgba.len();
    let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(tw, th, rgba)
        .expect("buffer sized to tw*th*4");
    NodeTexture {
        image: Some(Arc::new(RenderImage::new(vec![Frame::new(img)]))),
        bytes,
    }
}

/// Recursively fill descendant rectangles into the RGBA buffer, compositing
/// cached child textures on top of themed fills.
fn container_fill(
    node: &SymbolNode,
    root: &Rect,
    layout: &PackLayout,
    sx: f64,
    sy: f64,
    tw: u32,
    th: u32,
    rgba: &mut [u8],
    level: u8,
    child_tex: &impl Fn(&SymbolId) -> Option<Vec<u8>>,
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
        let tint = classify_tint_node(child);
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

        if let Some(src_bgra) = child_tex(&child.id) {
            composite_bgra(&src_bgra, pw as u32, ph as u32, px, py, tw, th, rgba);
        } else if !child.children.is_empty() {
            container_fill(child, root, layout, sx, sy, tw, th, rgba, level.saturating_add(1), child_tex);
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
    }
}

fn classify_tint_node(node: &SymbolNode) -> theme::BoxTint {
    match &node.id.kind {
        SymbolKind::Folder => match node.name.as_str() {
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
    }
}

/// Nearest-neighbor scale BGRA source into destination RGBA buffer via
/// src-over blend. Insets by 1px to avoid overwriting borders.
fn composite_bgra(
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
            let sxx = (ox as u64 * src_w as u64 / inner_w as u64) as u32;
            let si = (sy * src_w + sxx) as usize * 4;
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

// ── Texture cache ────────────────────────────────────────────────────

/// Bakes per frame for on-demand misses.
pub const BAKES_PER_FRAME: usize = 4;
#[cfg(test)]
const DEFAULT_CACHE_MB: u32 = 256;

struct Entry {
    tex: NodeTexture,
    last_used: u64,
}

fn disk_key(id: &SymbolId) -> String {
    let mut h = std::hash::DefaultHasher::new();
    id.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn save_to_disk(dir: &std::path::Path, id: &SymbolId, tex: &NodeTexture) {
    let Some(img) = &tex.image else { return };
    let path = dir.join(format!("{}.tex", disk_key(id)));
    let Ok(mut f) = std::fs::File::create(&path) else { return };
    let sz = img.size(0);
    let w = sz.width.0 as u32;
    let h = sz.height.0 as u32;
    let _ = f.write_all(&w.to_le_bytes());
    let _ = f.write_all(&h.to_le_bytes());
    if let Some(bytes) = img.as_bytes(0) {
        let len = bytes.len() as u32;
        let _ = f.write_all(&len.to_le_bytes());
        let _ = f.write_all(bytes);
    }
}

fn load_from_disk(dir: &std::path::Path, id: &SymbolId) -> Option<NodeTexture> {
    let path = dir.join(format!("{}.tex", disk_key(id)));
    let mut f = std::fs::File::open(&path).ok()?;
    let mut buf4 = [0u8; 4];
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
    let bytes = data.len();
    let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, data)?;
    Some(NodeTexture {
        image: Some(Arc::new(RenderImage::new(vec![Frame::new(img)]))),
        bytes,
    })
}

/// Per-node texture cache with LRU eviction and disk persistence.
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

impl TextureCache {
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

    pub fn used_bytes(&self) -> usize {
        self.bytes
    }

    pub fn contains(&self, id: &SymbolId) -> bool {
        self.entries.contains_key(id)
    }

    /// Cache lookup. Hit refreshes LRU; miss checks disk, then queues.
    pub fn get(&mut self, id: &SymbolId, screen_area: f64) -> Option<&NodeTexture> {
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

    pub fn has_queued(&self) -> bool {
        !self.queue.is_empty()
    }

    /// BGRA bytes snapshot for compositing into parent textures.
    pub fn child_bytes_snapshot(&self) -> HashMap<SymbolId, Vec<u8>> {
        self.entries
            .iter()
            .filter_map(|(id, e)| {
                let img = e.tex.image.as_ref()?;
                let bytes = img.as_bytes(0)?;
                Some((id.clone(), bytes.to_vec()))
            })
            .collect()
    }

    /// Bake up to BAKES_PER_FRAME queued items, evict past budget.
    pub fn bake_queued(
        &mut self,
        mut bake_fn: impl FnMut(&SymbolId, &mut Rasterizer) -> Option<NodeTexture>,
    ) -> bool {
        self.queue.sort_by(|a, b| b.1.total_cmp(&a.1));
        let queue = std::mem::take(&mut self.queue);
        let mut it = queue.into_iter();
        for (id, _) in it.by_ref().take(BAKES_PER_FRAME) {
            let tex = bake_fn(&id, &mut self.raster).unwrap_or_else(NodeTexture::empty);
            if let Some(dir) = &self.disk_dir {
                save_to_disk(dir, &id, &tex);
            }
            self.bytes += tex.bytes;
            self.clock += 1;
            self.entries.insert(id, Entry { tex, last_used: self.clock });
        }
        let remaining = it.next().is_some();
        self.evict();
        remaining
    }

    /// Insert a pre-baked texture (used by the bulk pre-bake pass).
    pub fn insert(&mut self, id: SymbolId, tex: NodeTexture) {
        if let Some(dir) = &self.disk_dir {
            save_to_disk(dir, &id, &tex);
        }
        self.bytes += tex.bytes;
        self.clock += 1;
        self.entries.insert(id, Entry { tex, last_used: self.clock });
    }

    fn evict(&mut self) {
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
            if let Some(img) = e.tex.image {
                self.retired.push(img);
            }
        }
    }

    pub fn take_retired(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.retired)
    }

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

// ── Pre-bake all textures bottom-up ──────────────────────────────────

use crate::treemap::runs_from_spans;

/// Bake every node in the tree bottom-up: leaves first, then containers
/// compositing their children's cached textures. Called on the background
/// thread at the end of indexing so the view opens fully textured.
pub fn pre_bake_all(
    tree: &SymbolTree,
    layout: &PackLayout,
    cache: &mut TextureCache,
    progress: &outrider_index::IndexProgress,
) {
    progress.phase.store(4, std::sync::atomic::Ordering::Relaxed);
    let file_symbols = collect_file_symbols(tree);
    let mut buffers = BufferManager::new(tree.repo_root.clone());

    let mut order: Vec<(&SymbolNode, u8)> = Vec::new();
    fn collect_nodes<'a>(node: &'a SymbolNode, depth: u8, out: &mut Vec<(&'a SymbolNode, u8)>) {
        for child in &node.children {
            collect_nodes(child, depth.saturating_add(1), out);
        }
        out.push((node, depth));
    }
    collect_nodes(&tree.root, 0, &mut order);

    let total = order.len();
    progress.files_total.store(total, std::sync::atomic::Ordering::Relaxed);
    progress.files_parsed.store(0, std::sync::atomic::Ordering::Relaxed);

    for (i, &(node, depth)) in order.iter().enumerate() {
        if cache.contains(&node.id) {
            progress.files_parsed.store(i + 1, std::sync::atomic::Ordering::Relaxed);
            continue;
        }

        if content::is_leaf_item(node) {
            let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
            let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
            if let Some(m) = buffers.get(&rel, syms) {
                if let Some(start) = m.symbol_start_line(&node.id) {
                    let count = (node.measure as usize)
                        .min(m.buffer.len_lines().saturating_sub(start));
                    let mut lines: Vec<Line> = Vec::with_capacity(count);
                    for j in 0..count {
                        if let Some((text, spans)) = m.buffer.line(start + j) {
                            let runs = runs_from_spans(text.len(), spans);
                            lines.push((text, runs));
                        }
                    }
                    if !lines.is_empty() {
                        let tex = cache.raster.bake(&lines);
                        cache.insert(node.id.clone(), tex);
                    } else {
                        cache.insert(node.id.clone(), NodeTexture::empty());
                    }
                } else {
                    cache.insert(node.id.clone(), NodeTexture::empty());
                }
            } else {
                cache.insert(node.id.clone(), NodeTexture::empty());
            }
        } else if !node.children.is_empty() {
            if let Some(rect) = layout.rects.get(&node.id) {
                let snap = cache.child_bytes_snapshot();
                let child_lookup = |id: &SymbolId| snap.get(id).cloned();
                let tex = bake_container(node, *rect, layout, depth, &child_lookup);
                cache.insert(node.id.clone(), tex);
            }
        }

        progress.files_parsed.store(i + 1, std::sync::atomic::Ordering::Relaxed);
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Line {
        (text.to_string(), vec![(text.len(), 0xFF0000)])
    }

    #[test]
    fn bake_dimensions_single_image() {
        let lines: Vec<Line> = (0..10).map(|_| plain("fn foo() {}")).collect();
        let tex = Rasterizer::new().bake(&lines);
        let img = tex.image.as_ref().unwrap();
        assert_eq!(img.size(0).width.0, 123);
        assert_eq!(img.size(0).height.0, 40);
        assert_eq!(tex.bytes, 123 * 40 * 4);
    }

    #[test]
    fn bake_strides_huge_leaves_to_height_cap() {
        let lines: Vec<Line> = (0..3000).map(|_| plain("x")).collect();
        let tex = Rasterizer::new().bake(&lines);
        assert_eq!(tex.image.unwrap().size(0).height.0, 1024);
    }

    #[test]
    fn bake_renders_glyphs_in_bgra() {
        let lines: Vec<Line> = (0..8).map(|_| plain("MMMMMMMMMM")).collect();
        let tex = Rasterizer::new().bake(&lines);
        let img = tex.image.unwrap();
        let bytes = img.as_bytes(0).unwrap();
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
        assert_eq!(
            a.image.unwrap().as_bytes(0),
            b.image.unwrap().as_bytes(0),
        );
    }

    #[test]
    fn empty_lines_produce_empty_texture() {
        let tex = Rasterizer::new().bake(&[]);
        assert!(tex.image.is_none());
        assert_eq!(tex.bytes, 0);
    }

    use outrider_index::{SymbolId, SymbolKind};

    fn sid(name: &str) -> SymbolId {
        SymbolId {
            qualified_path: name.to_string(),
            kind: SymbolKind::Item { label: "fn".into() },
            ordinal: 0,
        }
    }

    fn some_tex(n: usize, raster: &mut Rasterizer) -> Option<NodeTexture> {
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
        assert!(tex.image.is_none());
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
        assert!(!retired.is_empty(), "evicted image is retired");
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
