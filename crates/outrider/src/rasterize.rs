//! Bakes far-zoom leaf source text into low-res BGRA images with a CPU
//! mip chain (spec: docs/superpowers/specs/2026-07-11-texture-leaf-rendering-design.md).

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

pub struct Rasterizer {
    font_system: FontSystem,
    swash: SwashCache,
}

impl Rasterizer {
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

use std::collections::HashMap;

use outrider_index::SymbolId;

/// Bakes per frame; keeps zoom-out pop-in bounded without stalling a frame.
pub const BAKES_PER_FRAME: usize = 4;
/// Total texture budget across all levels of all cached leaves.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

struct Entry {
    tex: LeafTexture,
    last_used: u64,
}

/// Per-leaf texture cache: misses queue during the item pass, then
/// `bake_queued` bakes the largest few and LRU-evicts past the budget.
pub struct TextureCache {
    raster: Rasterizer,
    entries: HashMap<SymbolId, Entry>,
    clock: u64,
    bytes: usize,
    max_bytes: usize,
    queue: Vec<(SymbolId, f64)>,
    retired: Vec<Arc<RenderImage>>,
}

impl TextureCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            raster: Rasterizer::new(),
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
            queue: Vec::new(),
            retired: Vec::new(),
        }
    }

    /// Cache lookup. A hit refreshes LRU recency; a miss queues the leaf
    /// for `bake_queued` at the end of the frame.
    pub fn get(&mut self, id: &SymbolId, screen_area: f64) -> Option<&LeafTexture> {
        self.clock += 1;
        if self.entries.contains_key(id) {
            let e = self.entries.get_mut(id).unwrap();
            e.last_used = self.clock;
            Some(&e.tex)
        } else {
            self.queue.push((id.clone(), screen_area));
            None
        }
    }

    pub fn has_queued(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Bake up to BAKES_PER_FRAME queued leaves, largest on screen first,
    /// then evict LRU entries past the byte budget. Returns whether misses
    /// remain (the caller schedules a repaint so they bake next frame).
    pub fn bake_queued(
        &mut self,
        mut lines_for: impl FnMut(&SymbolId) -> Option<Vec<Line>>,
    ) -> bool {
        self.queue.sort_by(|a, b| b.1.total_cmp(&a.1));
        let queue = std::mem::take(&mut self.queue);
        let mut it = queue.into_iter();
        for (id, _) in it.by_ref().take(BAKES_PER_FRAME) {
            // None → empty texture: negative-cached so a leaf without a
            // buffer doesn't re-queue (and repaint) forever.
            let tex = match lines_for(&id) {
                Some(lines) => self.raster.bake(&lines),
                None => LeafTexture { levels: Vec::new(), bytes: 0 },
            };
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

    fn some_lines(n: usize) -> Option<Vec<Line>> {
        Some((0..n).map(|_| plain("let x = 1;")).collect())
    }

    #[test]
    fn cache_bakes_largest_first_within_budget() {
        let mut cache = TextureCache::new(MAX_BYTES);
        for i in 0..6 {
            // areas 10, 20, .. 60 — misses enqueue
            assert!(cache.get(&sid(&format!("l{i}")), (i + 1) as f64 * 10.0).is_none());
        }
        assert!(cache.has_queued());
        let remaining = cache.bake_queued(|_| some_lines(4));
        assert!(remaining, "6 queued, budget 4 — misses remain");
        // The 4 largest (l2..l5) are now hits; the 2 smallest are not.
        for i in 2..6 {
            assert!(cache.get(&sid(&format!("l{i}")), 1.0).is_some());
        }
        assert!(cache.get(&sid("l0"), 1.0).is_none());
        assert!(cache.get(&sid("l1"), 1.0).is_none());
        // Next frame the rest bake and nothing remains.
        assert!(!cache.bake_queued(|_| some_lines(4)));
        assert!(cache.get(&sid("l0"), 1.0).is_some());
    }

    #[test]
    fn cache_negative_caches_leaves_without_lines() {
        let mut cache = TextureCache::new(MAX_BYTES);
        assert!(cache.get(&sid("nofile"), 1.0).is_none());
        assert!(!cache.bake_queued(|_| None));
        // Cached as empty: a hit (no re-queue), but paints nothing.
        let tex = cache.get(&sid("nofile"), 1.0).expect("negative-cached");
        assert!(tex.levels.is_empty());
        assert!(!cache.has_queued());
    }

    #[test]
    fn cache_evicts_lru_and_retires_images() {
        // Identical lines → identical bytes per texture, so a budget of
        // exactly one texture forces exactly one eviction.
        let mut cache = TextureCache::new(usize::MAX);
        cache.get(&sid("a"), 1.0);
        cache.bake_queued(|_| some_lines(4));
        let one = cache.get(&sid("a"), 1.0).unwrap().bytes;
        cache.set_max_bytes_for_test(one); // room for exactly one texture
        cache.get(&sid("b"), 1.0);
        cache.bake_queued(|_| some_lines(4)); // 2×one > one → evict LRU (a)
        assert!(cache.get(&sid("b"), 1.0).is_some());
        assert!(cache.get(&sid("a"), 1.0).is_none());
        let retired = cache.take_retired();
        assert!(!retired.is_empty(), "evicted levels are retired");
        assert!(cache.take_retired().is_empty(), "drained");
    }

    #[test]
    fn cache_hit_refreshes_lru_order() {
        let mut cache = TextureCache::new(usize::MAX);
        cache.get(&sid("a"), 1.0);
        cache.bake_queued(|_| some_lines(4));
        cache.get(&sid("b"), 1.0);
        cache.bake_queued(|_| some_lines(4));
        let one = cache.get(&sid("a"), 1.0).unwrap().bytes; // touch a
        cache.set_max_bytes_for_test(2 * one); // room for two textures
        cache.get(&sid("c"), 1.0);
        cache.bake_queued(|_| some_lines(4)); // 3×one > 2×one → evict one
        // b (older touch than a) is the LRU victim.
        assert!(cache.get(&sid("a"), 1.0).is_some());
        assert!(cache.get(&sid("b"), 1.0).is_none());
        assert!(cache.get(&sid("c"), 1.0).is_some());
    }
}
