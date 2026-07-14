//! Bakes node textures: leaf text via cosmic-text, containers by compositing
//! cached child textures. One texture per node, GPU handles scaling.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

use outrider_index::{SymbolId, SymbolKind, SymbolNode};
use outrider_layout::{PackLayout, Rect};

use crate::buffers::BufferManager;
use crate::content::LINE_STEP;
use crate::texture_store::{TextureKey, TexturePayload, TextureStore};
use crate::theme;
use crate::treemap::BODY_PAD;
use crate::world;

/// Master texture line height, px.
pub const MASTER_LINE_PX: f64 = 4.0;
/// Master texture height cap; taller leaves stride rows to fit.
pub const MAX_TEX_H: usize = 1024;
/// Maximum pixel dimension (longer side) for a container thumbnail.
const CONTAINER_TEX_MAX: f64 = 1024.0;
/// Increment whenever rasterization semantics change incompatibly.
pub const RENDER_SCHEMA_VERSION: u64 = 1;

/// One source line: text plus colored runs (byte length, 0xRRGGBB).
pub type Line = (String, Vec<(usize, u32)>);

/// A baked node texture: single image, no mip chain. GPU handles scaling.
pub struct NodeTexture {
    pub image: Option<Arc<RenderImage>>,
    pub bytes: usize,
}

impl NodeTexture {
    pub fn empty() -> Self {
        Self {
            image: None,
            bytes: 0,
        }
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
        Self {
            font_system: FontSystem::new(),
            swash: SwashCache::new(),
        }
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
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(font_size, l as f32));
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
        let img =
            ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, rgba).expect("buffer sized to w*h*4");
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
    child_tex: &impl Fn(&SymbolId) -> Option<(u32, u32, Vec<u8>)>,
) -> NodeTexture {
    if container_rect.w < 1.0 || container_rect.h < 1.0 {
        return NodeTexture::empty();
    }
    let aspect = container_rect.w / container_rect.h;
    let (tw, th) = if aspect >= 1.0 {
        (
            CONTAINER_TEX_MAX as u32,
            (CONTAINER_TEX_MAX / aspect).ceil().max(1.0) as u32,
        )
    } else {
        (
            (CONTAINER_TEX_MAX * aspect).ceil().max(1.0) as u32,
            CONTAINER_TEX_MAX as u32,
        )
    };
    let sx = tw as f64 / container_rect.w;
    let sy = th as f64 / container_rect.h;

    let mut rgba = vec![0u8; (tw as usize) * (th as usize) * 4];
    container_fill(
        node,
        &container_rect,
        layout,
        sx,
        sy,
        tw,
        th,
        &mut rgba,
        base_level + 1,
        child_tex,
    );

    for p in rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let bytes = rgba.len();
    let img =
        ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(tw, th, rgba).expect("buffer sized to tw*th*4");
    NodeTexture {
        image: Some(Arc::new(RenderImage::new(vec![Frame::new(img)]))),
        bytes,
    }
}

/// Recursively fill descendant rectangles into the RGBA buffer, compositing
/// cached child textures on top of themed fills.
#[allow(clippy::too_many_arguments)]
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
    child_tex: &impl Fn(&SymbolId) -> Option<(u32, u32, Vec<u8>)>,
) {
    for child in &node.children {
        let Some(r) = layout.rects.get(&child.id) else {
            continue;
        };
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

        if let Some((sw, sh, src_bgra)) = child_tex(&child.id) {
            composite_bgra(
                &src_bgra, sw, sh, pw as u32, ph as u32, px, py, tw, th, rgba,
            );
        } else if !child.children.is_empty() {
            container_fill(
                child,
                root,
                layout,
                sx,
                sy,
                tw,
                th,
                rgba,
                level.saturating_add(1),
                child_tex,
            );
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
#[allow(clippy::too_many_arguments)]
fn composite_bgra(
    src_bgra: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dst_x: i32,
    dst_y: i32,
    buf_w: u32,
    buf_h: u32,
    rgba: &mut [u8],
) {
    if src_w == 0 || src_h == 0 || src_bgra.len() < 4 {
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
                src_bgra[si], // R→B
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
const DISK_QUEUE_CAPACITY: usize = 64;
const UNKNOWN_DISK_USAGE: u64 = u64::MAX;
#[cfg(test)]
const DEFAULT_CACHE_MB: u32 = 256;

struct Entry {
    tex: NodeTexture,
    last_used: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiskOperation {
    Open,
    Load,
    Save,
    Clear,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskDiagnostic {
    pub operation: DiskOperation,
    pub message: String,
}

impl fmt::Display for DiskDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "texture cache {:?} failed: {}",
            self.operation, self.message
        )
    }
}

enum DiskCommand {
    Load {
        id: SymbolId,
        key: TextureKey,
    },
    Save {
        key: TextureKey,
        payload: TexturePayload,
    },
    Clear,
}

enum DiskResult {
    OpenComplete,
    Loaded {
        id: SymbolId,
        payload: TexturePayload,
    },
    Miss {
        id: SymbolId,
    },
    SaveComplete,
    ClearComplete,
    Diagnostic(DiskDiagnostic),
}

struct TextureDiskWorker {
    commands: mpsc::SyncSender<DiskCommand>,
    results: mpsc::Receiver<DiskResult>,
    used_bytes: Arc<AtomicU64>,
}

impl TextureDiskWorker {
    fn spawn(project_root: PathBuf, max_bytes: u64) -> Self {
        Self::spawn_with_opener(move || TextureStore::open(&project_root, max_bytes))
    }

    fn spawn_with_opener(
        opener: impl FnOnce() -> io::Result<TextureStore> + Send + 'static,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let (result_tx, result_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let used_bytes = Arc::new(AtomicU64::new(UNKNOWN_DISK_USAGE));
        let worker_used_bytes = Arc::clone(&used_bytes);
        thread::Builder::new()
            .name("outrider-texture-cache".into())
            .spawn(move || run_disk_worker(opener, command_rx, result_tx, worker_used_bytes))
            .expect("failed to spawn texture cache worker");
        Self {
            commands: command_tx,
            results: result_rx,
            used_bytes,
        }
    }
}

fn run_disk_worker(
    opener: impl FnOnce() -> io::Result<TextureStore>,
    commands: mpsc::Receiver<DiskCommand>,
    results: mpsc::SyncSender<DiskResult>,
    used_bytes: Arc<AtomicU64>,
) {
    let mut reported = HashSet::new();
    let mut store = match opener() {
        Ok(store) => {
            used_bytes.store(store.used_bytes(), Ordering::Release);
            Some(store)
        }
        Err(error) => {
            send_disk_diagnostic(
                &results,
                &mut reported,
                DiskOperation::Open,
                error.to_string(),
            );
            None
        }
    };
    let _ = results.send(DiskResult::OpenComplete);
    while let Ok(command) = commands.recv() {
        match command {
            DiskCommand::Load { id, key } => {
                let loaded = store.as_mut().map(|store| store.load(&key));
                match loaded {
                    Some(Ok(Some(payload))) => {
                        let _ = results.send(DiskResult::Loaded { id, payload });
                    }
                    Some(Ok(None)) | None => {
                        let _ = results.send(DiskResult::Miss { id });
                    }
                    Some(Err(error)) => {
                        send_disk_diagnostic(
                            &results,
                            &mut reported,
                            DiskOperation::Load,
                            error.to_string(),
                        );
                        let _ = results.send(DiskResult::Miss { id });
                    }
                }
                if let Some(store) = &store {
                    used_bytes.store(store.used_bytes(), Ordering::Release);
                }
            }
            DiskCommand::Save { key, payload } => {
                if let Some(store) = &mut store {
                    if let Err(error) = store.save(&key, &payload) {
                        send_disk_diagnostic(
                            &results,
                            &mut reported,
                            DiskOperation::Save,
                            error.to_string(),
                        );
                    }
                    used_bytes.store(store.used_bytes(), Ordering::Release);
                }
                let _ = results.send(DiskResult::SaveComplete);
            }
            DiskCommand::Clear => {
                if let Some(store) = &mut store {
                    if let Err(error) = store.clear() {
                        send_disk_diagnostic(
                            &results,
                            &mut reported,
                            DiskOperation::Clear,
                            error.to_string(),
                        );
                    }
                    used_bytes.store(store.used_bytes(), Ordering::Release);
                }
                let _ = results.send(DiskResult::ClearComplete);
            }
        }
    }
}

fn send_disk_diagnostic(
    results: &mpsc::SyncSender<DiskResult>,
    reported: &mut HashSet<(DiskOperation, String)>,
    operation: DiskOperation,
    message: String,
) {
    if reported.insert((operation, message.clone())) {
        let _ = results.send(DiskResult::Diagnostic(DiskDiagnostic {
            operation,
            message,
        }));
    }
}

fn texture_from_payload(payload: TexturePayload) -> Option<NodeTexture> {
    let bytes = payload.bytes.len();
    let img =
        ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(payload.width, payload.height, payload.bytes)?;
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
    queue: HashMap<SymbolId, f64>,
    deferred_once: HashSet<SymbolId>,
    retired: Vec<Arc<RenderImage>>,
    disk_worker: Option<TextureDiskWorker>,
    waiting_disk: HashMap<SymbolId, f64>,
    disk_inflight: HashSet<SymbolId>,
    disk_worker_starting: bool,
    clear_disk_pending: bool,
    clear_disk_inflight: usize,
    pending_disk_saves: usize,
    disk_diagnostics: Vec<DiskDiagnostic>,
    source_fingerprints: BTreeMap<String, u64>,
}

impl TextureCache {
    pub fn new(
        project_root: &Path,
        source_fingerprints: BTreeMap<String, u64>,
        max_bytes: usize,
        disk_max_bytes: u64,
    ) -> Self {
        Self {
            raster: Rasterizer::new(),
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
            queue: HashMap::new(),
            deferred_once: HashSet::new(),
            retired: Vec::new(),
            disk_worker: Some(TextureDiskWorker::spawn(
                project_root.to_path_buf(),
                disk_max_bytes,
            )),
            waiting_disk: HashMap::new(),
            disk_inflight: HashSet::new(),
            disk_worker_starting: true,
            clear_disk_pending: false,
            clear_disk_inflight: 0,
            pending_disk_saves: 0,
            disk_diagnostics: Vec::new(),
            source_fingerprints,
        }
    }

    fn disk_key(&self, id: &SymbolId) -> Option<TextureKey> {
        let relative_path = BufferManager::file_path_of(&id.qualified_path).replace('\\', "/");
        let source_fingerprint = *self.source_fingerprints.get(&relative_path)?;
        Some(TextureKey::new(
            &relative_path,
            source_fingerprint,
            id,
            RENDER_SCHEMA_VERSION,
            theme::fingerprint(),
        ))
    }

    pub fn used_bytes(&self) -> usize {
        self.bytes
    }

    pub fn disk_used_bytes(&self) -> Option<u64> {
        let bytes = self
            .disk_worker
            .as_ref()?
            .used_bytes
            .load(Ordering::Acquire);
        (bytes != UNKNOWN_DISK_USAGE).then_some(bytes)
    }

    pub fn request_clear_disk_cache(&mut self) {
        self.clear_disk_pending = true;
    }

    pub fn drain_disk_diagnostics(&mut self) -> Vec<DiskDiagnostic> {
        std::mem::take(&mut self.disk_diagnostics)
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
        if self.disk_key(id).is_some() && self.disk_worker.is_some() {
            let priority = self.waiting_disk.entry(id.clone()).or_insert(screen_area);
            *priority = priority.max(screen_area);
            return None;
        }
        self.request(id.clone(), screen_area);
        None
    }

    /// Queue a texture bake, retaining only the largest visible screen area.
    pub fn request(&mut self, id: SymbolId, screen_area: f64) {
        if self.entries.contains_key(&id) {
            return;
        }
        if let Some(priority) = self.waiting_disk.get_mut(&id) {
            *priority = priority.max(screen_area);
            return;
        }
        self.queue
            .entry(id)
            .and_modify(|priority| *priority = priority.max(screen_area))
            .or_insert(screen_area);
    }

    /// Give a container's dependencies one bounded processing pass.
    pub fn defer_request_once(&mut self, id: &SymbolId) -> bool {
        if self.deferred_once.insert(id.clone()) {
            self.queue.remove(id);
            true
        } else {
            false
        }
    }

    pub fn needs_dependency_pass(&self, id: &SymbolId) -> bool {
        !self.deferred_once.contains(id)
    }

    pub fn has_queued(&self) -> bool {
        !self.queue.is_empty()
            || !self.waiting_disk.is_empty()
            || self.disk_worker_starting
            || self.clear_disk_pending
            || self.clear_disk_inflight > 0
            || self.pending_disk_saves > 0
    }

    /// Highest-priority requests that can be processed this frame.
    pub fn next_request_ids(&self) -> Vec<SymbolId> {
        let mut queued: Vec<_> = self.queue.iter().collect();
        queued.sort_by(|a, b| b.1.total_cmp(a.1));
        queued
            .into_iter()
            .take(BAKES_PER_FRAME)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// BGRA bytes snapshot with dimensions for compositing into parent textures.
    pub fn direct_child_bytes(&self, node: &SymbolNode) -> HashMap<SymbolId, (u32, u32, Vec<u8>)> {
        node.children
            .iter()
            .filter_map(|child| {
                let id = &child.id;
                let e = self.entries.get(id)?;
                let img = e.tex.image.as_ref()?;
                let sz = img.size(0);
                let w = sz.width.0 as u32;
                let h = sz.height.0 as u32;
                let bytes = img.as_bytes(0)?;
                Some((id.clone(), (w, h, bytes.to_vec())))
            })
            .collect()
    }

    /// Bake up to BAKES_PER_FRAME queued items, evict past budget.
    pub fn process_requests(
        &mut self,
        mut bake_fn: impl FnMut(&SymbolId, &mut Rasterizer) -> Option<NodeTexture>,
    ) -> bool {
        self.apply_disk_results();
        self.dispatch_clear_request();
        self.dispatch_disk_loads();
        let mut queue: Vec<_> = std::mem::take(&mut self.queue).into_iter().collect();
        queue.sort_by(|a, b| b.1.total_cmp(&a.1));
        let remaining = queue.split_off(queue.len().min(BAKES_PER_FRAME));
        self.queue.extend(remaining);
        for (id, _) in queue {
            let tex = bake_fn(&id, &mut self.raster).unwrap_or_else(NodeTexture::empty);
            self.insert(id, tex);
        }
        self.has_queued()
    }

    /// Insert a texture while preserving disk and memory-cache invariants.
    pub fn insert(&mut self, id: SymbolId, tex: NodeTexture) {
        self.save_to_disk(&id, &tex);
        self.insert_entry(id, tex);
    }

    fn insert_entry(&mut self, id: SymbolId, tex: NodeTexture) {
        self.deferred_once.remove(&id);
        if let Some(replaced) = self.entries.remove(&id) {
            self.bytes -= replaced.tex.bytes;
            if let Some(image) = replaced.tex.image {
                self.retired.push(image);
            }
        }
        self.bytes += tex.bytes;
        self.clock += 1;
        self.entries.insert(
            id,
            Entry {
                tex,
                last_used: self.clock,
            },
        );
        self.evict();
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

    fn save_to_disk(&mut self, id: &SymbolId, tex: &NodeTexture) {
        let Some(key) = self.disk_key(id) else { return };
        let Some(image) = &tex.image else { return };
        let size = image.size(0);
        let Some(bytes) = image.as_bytes(0) else {
            return;
        };
        let payload = TexturePayload {
            width: size.width.0 as u32,
            height: size.height.0 as u32,
            bytes: bytes.to_vec(),
        };
        if let Some(worker) = &self.disk_worker {
            if worker
                .commands
                .try_send(DiskCommand::Save { key, payload })
                .is_ok()
            {
                self.pending_disk_saves = self.pending_disk_saves.saturating_add(1);
            }
        }
    }

    fn apply_disk_results(&mut self) {
        for _ in 0..BAKES_PER_FRAME {
            let result = self
                .disk_worker
                .as_ref()
                .and_then(|worker| worker.results.try_recv().ok());
            let Some(result) = result else { break };
            match result {
                DiskResult::OpenComplete => self.disk_worker_starting = false,
                DiskResult::Loaded { id, payload } => {
                    self.disk_inflight.remove(&id);
                    let priority = self.waiting_disk.remove(&id).unwrap_or_default();
                    if let Some(texture) = texture_from_payload(payload) {
                        self.insert_entry(id, texture);
                    } else {
                        self.request(id, priority);
                    }
                }
                DiskResult::Miss { id } => {
                    self.disk_inflight.remove(&id);
                    let priority = self.waiting_disk.remove(&id).unwrap_or_default();
                    self.request(id, priority);
                }
                DiskResult::SaveComplete => {
                    self.pending_disk_saves = self.pending_disk_saves.saturating_sub(1);
                }
                DiskResult::ClearComplete => {
                    self.clear_disk_inflight = self.clear_disk_inflight.saturating_sub(1);
                }
                DiskResult::Diagnostic(diagnostic) => {
                    self.disk_diagnostics.push(diagnostic);
                }
            }
        }
    }

    fn dispatch_disk_loads(&mut self) {
        let Some(worker) = &self.disk_worker else {
            return;
        };
        let mut candidates: Vec<_> = self
            .waiting_disk
            .iter()
            .filter(|(id, _)| !self.disk_inflight.contains(*id))
            .map(|(id, priority)| (id.clone(), *priority))
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut failed = Vec::new();
        for (id, _) in candidates.into_iter().take(BAKES_PER_FRAME) {
            let Some(key) = self.disk_key(&id) else {
                continue;
            };
            match worker.commands.try_send(DiskCommand::Load {
                id: id.clone(),
                key,
            }) {
                Ok(()) => {
                    self.disk_inflight.insert(id);
                }
                Err(mpsc::TrySendError::Full(_)) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => failed.push(id),
            }
        }
        for id in failed {
            let priority = self.waiting_disk.remove(&id).unwrap_or_default();
            self.request(id, priority);
        }
    }

    fn dispatch_clear_request(&mut self) {
        if !self.clear_disk_pending {
            return;
        }
        let Some(worker) = &self.disk_worker else {
            self.clear_disk_pending = false;
            return;
        };
        match worker.commands.try_send(DiskCommand::Clear) {
            Ok(()) => {
                self.clear_disk_pending = false;
                self.clear_disk_inflight = self.clear_disk_inflight.saturating_add(1);
            }
            Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.clear_disk_pending = false;
                self.disk_diagnostics.push(DiskDiagnostic {
                    operation: DiskOperation::Clear,
                    message: "texture cache worker disconnected".into(),
                });
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
            queue: HashMap::new(),
            deferred_once: HashSet::new(),
            retired: Vec::new(),
            disk_worker: None,
            waiting_disk: HashMap::new(),
            disk_inflight: HashSet::new(),
            disk_worker_starting: false,
            clear_disk_pending: false,
            clear_disk_inflight: 0,
            pending_disk_saves: 0,
            disk_diagnostics: Vec::new(),
            source_fingerprints: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    fn set_max_bytes_for_test(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
    }

    #[cfg(test)]
    fn queued_ids(&self) -> Vec<SymbolId> {
        let mut queued: Vec<_> = self.queue.iter().collect();
        queued.sort_by(|a, b| b.1.total_cmp(a.1));
        queued.into_iter().map(|(id, _)| id.clone()).collect()
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

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
        let covered: Vec<&[u8]> = bytes.chunks_exact(4).filter(|p| p[3] > 0).collect();
        assert!(!covered.is_empty(), "no glyph coverage — font not found?");
        assert!(covered.iter().all(|p| p[2] >= p[0]), "not BGRA red");
    }

    #[test]
    fn bake_is_deterministic() {
        let lines: Vec<Line> = (0..5).map(|_| plain("let x = 1;")).collect();
        let a = Rasterizer::new().bake(&lines);
        let b = Rasterizer::new().bake(&lines);
        assert_eq!(a.image.unwrap().as_bytes(0), b.image.unwrap().as_bytes(0),);
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

    fn texture(bytes: usize) -> NodeTexture {
        NodeTexture { image: None, bytes }
    }

    #[test]
    fn repeated_request_is_deduplicated_and_priority_is_upgraded() {
        let mut cache = TextureCache::new_memory_only(1024);
        cache.request(sid("a"), 10.0);
        cache.request(sid("a"), 100.0);
        cache.request(sid("b"), 50.0);

        assert_eq!(cache.queued_ids(), vec![sid("a"), sid("b")]);
    }

    #[test]
    fn disk_worker_startup_does_not_wait_for_store_open() {
        let (allow_open, wait_for_open) = mpsc::channel();
        let (started, worker_started) = mpsc::channel();
        let worker = TextureDiskWorker::spawn_with_opener(move || {
            started.send(()).unwrap();
            wait_for_open.recv().unwrap();
            Err(std::io::Error::other("injected open failure"))
        });

        worker_started.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(worker);
        allow_open.send(()).unwrap();
    }

    #[test]
    fn worker_open_lifecycle_keeps_empty_cache_pending_until_completion() {
        let (command_tx, _command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let (result_tx, result_rx) = mpsc::channel();
        let mut cache = TextureCache::new_memory_only(1024);
        cache.disk_worker = Some(TextureDiskWorker {
            commands: command_tx,
            results: result_rx,
            used_bytes: Arc::new(AtomicU64::new(UNKNOWN_DISK_USAGE)),
        });
        cache.disk_worker_starting = true;

        assert!(cache.has_queued());
        result_tx.send(DiskResult::OpenComplete).unwrap();
        cache.process_requests(|_, _| panic!("no texture should bake"));
        assert!(!cache.has_queued());
    }

    #[test]
    fn clear_keeps_cache_pending_until_worker_completion() {
        let (command_tx, command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let (result_tx, result_rx) = mpsc::channel();
        let mut cache = TextureCache::new_memory_only(1024);
        cache.disk_worker = Some(TextureDiskWorker {
            commands: command_tx,
            results: result_rx,
            used_bytes: Arc::new(AtomicU64::new(UNKNOWN_DISK_USAGE)),
        });

        cache.request_clear_disk_cache();
        cache.process_requests(|_, _| panic!("no texture should bake"));
        assert!(matches!(command_rx.recv().unwrap(), DiskCommand::Clear));
        assert!(cache.has_queued(), "queued clear is still in flight");

        result_tx.send(DiskResult::ClearComplete).unwrap();
        cache.process_requests(|_, _| panic!("no texture should bake"));
        assert!(!cache.has_queued());
    }

    #[test]
    fn save_keeps_cache_pending_until_worker_completion() {
        let (command_tx, command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let (result_tx, result_rx) = mpsc::channel();
        let mut cache = TextureCache::new_memory_only(1024 * 1024);
        cache.source_fingerprints.insert("a".into(), 1);
        cache.disk_worker = Some(TextureDiskWorker {
            commands: command_tx,
            results: result_rx,
            used_bytes: Arc::new(AtomicU64::new(UNKNOWN_DISK_USAGE)),
        });
        let texture = some_tex(1, &mut Rasterizer::new()).unwrap();

        cache.insert(sid("a"), texture);
        assert!(matches!(
            command_rx.recv().unwrap(),
            DiskCommand::Save { .. }
        ));
        assert!(cache.has_queued(), "queued save is still in flight");

        result_tx.send(DiskResult::SaveComplete).unwrap();
        cache.process_requests(|_, _| panic!("no texture should bake"));
        assert!(!cache.has_queued());
    }

    #[test]
    fn disk_requests_are_deduplicated_and_keep_upgraded_priority() {
        let (command_tx, command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let (result_tx, result_rx) = mpsc::channel();
        let mut cache = TextureCache::new_memory_only(1024);
        cache.source_fingerprints.insert("a".into(), 1);
        cache.disk_worker = Some(TextureDiskWorker {
            commands: command_tx,
            results: result_rx,
            used_bytes: Arc::new(AtomicU64::new(UNKNOWN_DISK_USAGE)),
        });

        assert!(cache.get(&sid("a"), 10.0).is_none());
        assert!(cache.get(&sid("a"), 100.0).is_none());
        assert!(command_rx.try_recv().is_err(), "paint submitted disk I/O");
        cache.process_requests(|_, _| panic!("disk candidate baked before miss"));
        let DiskCommand::Load { id, .. } = command_rx.recv().unwrap() else {
            panic!("expected load request")
        };
        assert_eq!(id, sid("a"));
        assert!(command_rx.try_recv().is_err(), "duplicate disk load queued");

        cache.request(sid("b"), 50.0);
        result_tx.send(DiskResult::Miss { id: sid("a") }).unwrap();
        let mut baked = Vec::new();
        cache.process_requests(|id, _| {
            baked.push(id.clone());
            Some(texture(1))
        });
        assert_eq!(baked, vec![sid("a"), sid("b")]);
    }

    #[test]
    fn disconnected_disk_worker_falls_back_to_baking() {
        let (command_tx, command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        drop(command_rx);
        let (_result_tx, result_rx) = mpsc::channel();
        let mut cache = TextureCache::new_memory_only(1024);
        cache.source_fingerprints.insert("a".into(), 1);
        cache.disk_worker = Some(TextureDiskWorker {
            commands: command_tx,
            results: result_rx,
            used_bytes: Arc::new(AtomicU64::new(UNKNOWN_DISK_USAGE)),
        });
        cache.get(&sid("a"), 10.0);

        cache.process_requests(|_, _| Some(texture(1)));

        assert!(cache.contains(&sid("a")));
    }

    #[test]
    fn parent_is_deferred_once_without_losing_dependency_requests() {
        let mut cache = TextureCache::new_memory_only(100);
        for i in 0..6 {
            cache.insert(sid(&format!("child-{i}")), texture(80));
        }

        cache.request(sid("parent"), 100.0);
        for i in 0..5 {
            cache.request(sid(&format!("child-{i}")), 110.0);
        }

        assert!(cache.defer_request_once(&sid("parent")));
        assert!(cache.process_requests(|_, _| Some(texture(80))));
        assert!(!cache.needs_dependency_pass(&sid("parent")));

        cache.request(sid("parent"), 100.0);
        assert!(!cache.defer_request_once(&sid("parent")));
        assert!(!cache.process_requests(|_, _| Some(texture(80))));
        assert!(
            cache.contains(&sid("parent")),
            "parent gets a reserved turn"
        );
    }

    #[test]
    fn replacing_an_entry_does_not_double_count_bytes() {
        let mut cache = TextureCache::new_memory_only(1024);
        cache.insert(sid("a"), texture(100));
        cache.insert(sid("a"), texture(60));

        assert_eq!(cache.used_bytes(), 60);
    }

    #[test]
    fn insertion_obeys_memory_limit() {
        let mut cache = TextureCache::new_memory_only(100);
        cache.insert(sid("a"), texture(80));
        cache.insert(sid("b"), texture(80));

        assert!(cache.used_bytes() <= 100);
    }

    #[test]
    fn disk_promotion_obeys_memory_limit() {
        let dir = tempfile::tempdir().unwrap();
        let fingerprints = BTreeMap::from([("a".into(), 1), ("b".into(), 1)]);
        let payload = TexturePayload {
            width: 5,
            height: 4,
            bytes: vec![0; 80],
        };
        let disk_limit = 1024 * 1024;
        let mut cache = TextureCache::new_memory_only(100);
        cache.source_fingerprints = fingerprints;
        let a_key = cache.disk_key(&sid("a")).unwrap();
        let b_key = cache.disk_key(&sid("b")).unwrap();
        let mut store = TextureStore::open_at(dir.path(), "promotion-test", disk_limit).unwrap();
        store.save(&a_key, &payload).unwrap();
        store.save(&b_key, &payload).unwrap();
        cache.disk_worker = Some(TextureDiskWorker::spawn_with_opener(move || Ok(store)));
        assert!(cache.get(&sid("a"), 1.0).is_none());
        assert!(cache.get(&sid("b"), 1.0).is_none());
        for _ in 0..100 {
            cache.process_requests(|_, _| None);
            if cache.contains(&sid("a")) || cache.contains(&sid("b")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(cache.contains(&sid("a")) || cache.contains(&sid("b")));

        assert!(cache.used_bytes() <= 100);
    }

    #[test]
    fn cache_bakes_largest_first_within_budget() {
        let mut cache = TextureCache::new_memory_only(DEFAULT_CACHE_MB as usize * 1024 * 1024);
        for i in 0..6 {
            assert!(cache
                .get(&sid(&format!("l{i}")), (i + 1) as f64 * 10.0)
                .is_none());
        }
        assert!(cache.has_queued());
        let remaining = cache.process_requests(|_, r| some_tex(4, r));
        assert!(remaining, "6 queued, budget 4 — misses remain");
        for i in 2..6 {
            assert!(cache.get(&sid(&format!("l{i}")), 1.0).is_some());
        }
        assert!(cache.get(&sid("l0"), 1.0).is_none());
        assert!(cache.get(&sid("l1"), 1.0).is_none());
        assert!(!cache.process_requests(|_, r| some_tex(4, r)));
        assert!(cache.get(&sid("l0"), 1.0).is_some());
    }

    #[test]
    fn cache_negative_caches_leaves_without_lines() {
        let mut cache = TextureCache::new_memory_only(DEFAULT_CACHE_MB as usize * 1024 * 1024);
        assert!(cache.get(&sid("nofile"), 1.0).is_none());
        assert!(!cache.process_requests(|_, _| None));
        let tex = cache.get(&sid("nofile"), 1.0).expect("negative-cached");
        assert!(tex.image.is_none());
        assert!(!cache.has_queued());
    }

    #[test]
    fn cache_evicts_lru_and_retires_images() {
        let mut cache = TextureCache::new_memory_only(usize::MAX);
        cache.get(&sid("a"), 1.0);
        cache.process_requests(|_, r| some_tex(4, r));
        let one = cache.get(&sid("a"), 1.0).unwrap().bytes;
        cache.set_max_bytes_for_test(one);
        cache.get(&sid("b"), 1.0);
        cache.process_requests(|_, r| some_tex(4, r));
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
        cache.process_requests(|_, r| some_tex(4, r));
        cache.get(&sid("b"), 1.0);
        cache.process_requests(|_, r| some_tex(4, r));
        let one = cache.get(&sid("a"), 1.0).unwrap().bytes;
        cache.set_max_bytes_for_test(2 * one);
        cache.get(&sid("c"), 1.0);
        cache.process_requests(|_, r| some_tex(4, r));
        assert!(cache.get(&sid("a"), 1.0).is_some());
        assert!(cache.get(&sid("b"), 1.0).is_none());
        assert!(cache.get(&sid("c"), 1.0).is_some());
    }

    #[test]
    fn chunk_disk_key_uses_its_source_file_fingerprint() {
        let mut cache = TextureCache::new_memory_only(usize::MAX);
        cache.source_fingerprints.insert("BIG.md".into(), 42);
        let chunk = SymbolId {
            qualified_path: "BIG.md#2".into(),
            kind: SymbolKind::Chunk,
            ordinal: 0,
        };
        assert!(cache.disk_key(&chunk).is_some());

        let missing = SymbolId {
            qualified_path: "MISSING.md#2".into(),
            kind: SymbolKind::Chunk,
            ordinal: 0,
        };
        assert!(cache.disk_key(&missing).is_none());
    }
}
