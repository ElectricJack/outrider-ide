//! Bakes node textures: leaf text via cosmic-text, containers by compositing
//! cached child textures. One texture per node, GPU handles scaling.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::io;
#[cfg(test)]
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

use outrider_index::{SymbolId, SymbolNode};
use outrider_layout::{PackLayout, Rect};

use crate::buffers::BufferManager;
use crate::content;
use crate::content::LINE_STEP;
use crate::texture_store::{ProjectTextureNamespace, TextureKey, TexturePayload, TextureStore};
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
// Version 2 invalidates textures baked with the former 480-world-unit page width.
pub const RENDER_SCHEMA_VERSION: u64 = 3;

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

        let kind = theme::node_box_kind(content::is_leaf_item(child), &child.id.kind);
        let tint = theme::node_box_tint(child);
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
const DISK_RESULT_CAPACITY: usize = 16;
const DISK_START_TIMEOUT: Duration = Duration::from_secs(2);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskState {
    Preparing,
    Ready { used_bytes: u64 },
    Unavailable,
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
    Shutdown,
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

#[allow(dead_code)]
struct TextureDiskWorker {
    commands: mpsc::SyncSender<DiskCommand>,
    results: mpsc::Receiver<DiskResult>,
    state: Arc<Mutex<DiskState>>,
    shared_usage: Arc<Mutex<Option<Arc<AtomicU64>>>>,
    cancelled: Arc<AtomicBool>,
    control: Option<Arc<WorkerControl>>,
}

struct PendingDiskStart {
    prepared: crate::texture_store::PreparedTextureStore,
    claimant: u64,
    deadline: Instant,
}

struct WorkerControl {
    cancelled: Arc<AtomicBool>,
    retiring: AtomicBool,
    wake: Mutex<Option<mpsc::SyncSender<DiskCommand>>>,
}

impl WorkerControl {
    fn retire(&self) {
        self.retiring.store(true, Ordering::Release);
        self.cancelled.store(true, Ordering::Release);
        if let Some(wake) = self
            .wake
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
        {
            let _ = wake.try_send(DiskCommand::Shutdown);
        }
    }
}

struct WorkerSlot {
    current: Mutex<Option<Weak<WorkerControl>>>,
    next_claimant: AtomicU64,
    newest_claimant: AtomicU64,
    started: AtomicUsize,
    retired: AtomicUsize,
}

impl WorkerSlot {
    fn new() -> Self {
        Self {
            current: Mutex::new(None),
            next_claimant: AtomicU64::new(0),
            newest_claimant: AtomicU64::new(0),
            started: AtomicUsize::new(0),
            retired: AtomicUsize::new(0),
        }
    }

    fn designate_successor(&self) -> u64 {
        let claimant = self.next_claimant.fetch_add(1, Ordering::AcqRel) + 1;
        self.newest_claimant.fetch_max(claimant, Ordering::AcqRel);
        let current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.is_newest(claimant) {
            if let Some(worker) = current.as_ref().and_then(Weak::upgrade) {
                worker.retire();
            }
        }
        claimant
    }

    fn is_newest(&self, claimant: u64) -> bool {
        self.newest_claimant.load(Ordering::Acquire) == claimant
    }

    fn try_claim(&self, claimant: u64) -> SlotClaim {
        if !self.is_newest(claimant) {
            return SlotClaim::Superseded;
        }
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !self.is_newest(claimant) {
            return SlotClaim::Superseded;
        }
        if current.as_ref().and_then(Weak::upgrade).is_some() {
            return SlotClaim::Deferred;
        }
        let worker = Arc::new(WorkerControl {
            cancelled: Arc::new(AtomicBool::new(false)),
            retiring: AtomicBool::new(false),
            wake: Mutex::new(None),
        });
        *current = Some(Arc::downgrade(&worker));
        SlotClaim::Granted(worker)
    }

    fn finish(&self, worker: &Arc<WorkerControl>) {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if current
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|current| Arc::ptr_eq(&current, worker))
        {
            *current = None;
        }
        self.retired.fetch_add(1, Ordering::AcqRel);
    }
}

enum SlotClaim {
    Granted(Arc<WorkerControl>),
    Deferred,
    Superseded,
}

trait RetryClock: Send + Sync {
    fn now(&self) -> Instant;
}

struct SystemRetryClock;

impl RetryClock for SystemRetryClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

fn global_worker_slot() -> Arc<WorkerSlot> {
    static SLOT: OnceLock<Arc<WorkerSlot>> = OnceLock::new();
    Arc::clone(SLOT.get_or_init(|| Arc::new(WorkerSlot::new())))
}

enum WorkerStart {
    Started(TextureDiskWorker),
    Deferred(PendingDiskStart),
    Failed(io::Error),
    Superseded,
}

impl TextureDiskWorker {
    fn start(prepared: PendingDiskStart, slot: Arc<WorkerSlot>) -> WorkerStart {
        let control = match slot.try_claim(prepared.claimant) {
            SlotClaim::Granted(control) => control,
            SlotClaim::Deferred => return WorkerStart::Deferred(prepared),
            SlotClaim::Superseded => return WorkerStart::Superseded,
        };
        let opener = move || prepared.prepared.open();
        match Self::try_spawn_with_opener_and_control(opener, Some((control, slot))) {
            Ok(worker) => WorkerStart::Started(worker),
            Err(error) => WorkerStart::Failed(error),
        }
    }

    #[cfg(test)]
    fn spawn_with_opener(
        opener: impl FnOnce() -> io::Result<TextureStore> + Send + 'static,
    ) -> Self {
        Self::try_spawn_with_opener(opener).expect("failed to spawn test texture cache worker")
    }

    #[cfg(test)]
    fn spawn_with_opener_and_slot(
        opener: impl FnOnce() -> io::Result<TextureStore> + Send + 'static,
        slot: Arc<WorkerSlot>,
    ) -> Self {
        let claimant = slot.designate_successor();
        let SlotClaim::Granted(control) = slot.try_claim(claimant) else {
            panic!("test worker slot was occupied")
        };
        Self::try_spawn_with_opener_and_control(opener, Some((control, slot)))
            .expect("failed to spawn gated test texture cache worker")
    }

    #[cfg(test)]
    fn try_spawn_with_opener(
        opener: impl FnOnce() -> io::Result<TextureStore> + Send + 'static,
    ) -> io::Result<Self> {
        Self::try_spawn_with_opener_and_control(opener, None)
    }

    fn try_spawn_with_opener_and_control(
        opener: impl FnOnce() -> io::Result<TextureStore> + Send + 'static,
        lifecycle: Option<(Arc<WorkerControl>, Arc<WorkerSlot>)>,
    ) -> io::Result<Self> {
        let (command_tx, command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let (result_tx, result_rx) = disk_result_channel();
        let state = Arc::new(Mutex::new(DiskState::Preparing));
        let worker_state = Arc::clone(&state);
        let shared_usage = Arc::new(Mutex::new(None));
        let worker_shared_usage = Arc::clone(&shared_usage);
        let cancelled = lifecycle
            .as_ref()
            .map(|(control, _)| Arc::clone(&control.cancelled))
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let worker_cancelled = Arc::clone(&cancelled);
        let control = lifecycle.as_ref().map(|(control, _)| Arc::clone(control));
        if let Some(control) = &control {
            *control
                .wake
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(command_tx.clone());
        }
        thread::Builder::new()
            .name("outrider-texture-cache".into())
            .spawn(move || {
                if let Some((_, slot)) = &lifecycle {
                    slot.started.fetch_add(1, Ordering::AcqRel);
                }
                run_disk_worker(
                    opener,
                    command_rx,
                    result_tx,
                    worker_state,
                    worker_shared_usage,
                    worker_cancelled,
                );
                if let Some((control, slot)) = lifecycle {
                    slot.finish(&control);
                }
            })?;
        Ok(Self {
            commands: command_tx,
            results: result_rx,
            state,
            shared_usage,
            cancelled,
            control,
        })
    }
}

impl Drop for TextureDiskWorker {
    fn drop(&mut self) {
        if let Some(control) = &self.control {
            control.retire();
        } else {
            self.cancelled.store(true, Ordering::Release);
            let _ = self.commands.try_send(DiskCommand::Shutdown);
        }
    }
}

fn disk_result_channel() -> (mpsc::SyncSender<DiskResult>, mpsc::Receiver<DiskResult>) {
    mpsc::sync_channel(DISK_RESULT_CAPACITY)
}

fn run_disk_worker(
    opener: impl FnOnce() -> io::Result<TextureStore>,
    commands: mpsc::Receiver<DiskCommand>,
    results: mpsc::SyncSender<DiskResult>,
    state: Arc<Mutex<DiskState>>,
    shared_usage: Arc<Mutex<Option<Arc<AtomicU64>>>>,
    cancelled: Arc<AtomicBool>,
) {
    let mut reported = HashSet::new();
    let mut store = match opener() {
        Ok(store) => {
            *shared_usage
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(store.shared_usage());
            *state.lock().unwrap_or_else(|error| error.into_inner()) = DiskState::Ready {
                used_bytes: store.used_bytes(),
            };
            Some(store)
        }
        Err(error) => {
            send_disk_diagnostic(
                &results,
                &mut reported,
                DiskOperation::Open,
                error.to_string(),
            );
            *state.lock().unwrap_or_else(|error| error.into_inner()) = DiskState::Unavailable;
            None
        }
    };
    let _ = results.send(DiskResult::OpenComplete);
    while let Ok(command) = commands.recv() {
        if matches!(command, DiskCommand::Shutdown) {
            break;
        }
        if cancelled.load(Ordering::Acquire) {
            continue;
        }
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
                    *state.lock().unwrap_or_else(|error| error.into_inner()) = DiskState::Ready {
                        used_bytes: store.used_bytes(),
                    };
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
                    *state.lock().unwrap_or_else(|error| error.into_inner()) = DiskState::Ready {
                        used_bytes: store.used_bytes(),
                    };
                }
                let _ = results.send(DiskResult::SaveComplete);
            }
            DiskCommand::Clear => {
                if let Some(store) = &mut store {
                    if let Err(error) = store.clear() {
                        let _ = results.send(DiskResult::Diagnostic(DiskDiagnostic {
                            operation: DiskOperation::Clear,
                            message: error.to_string(),
                        }));
                    }
                    *state.lock().unwrap_or_else(|error| error.into_inner()) = DiskState::Ready {
                        used_bytes: store.used_bytes(),
                    };
                } else {
                    let _ = results.send(DiskResult::Diagnostic(DiskDiagnostic {
                        operation: DiskOperation::Clear,
                        message: "texture cache unavailable".into(),
                    }));
                }
                let _ = results.send(DiskResult::ClearComplete);
            }
            DiskCommand::Shutdown => unreachable!(),
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
    refreshed_from_live_text: HashSet<SymbolId>,
    retired: Vec<Arc<RenderImage>>,
    disk_worker: Option<TextureDiskWorker>,
    pending_disk_start: Option<PendingDiskStart>,
    worker_slot: Arc<WorkerSlot>,
    worker_claimant: u64,
    retry_clock: Arc<dyn RetryClock>,
    waiting_disk: HashMap<SymbolId, f64>,
    disk_inflight: HashSet<SymbolId>,
    disk_worker_starting: bool,
    clear_disk_pending: bool,
    clear_disk_inflight: usize,
    pending_disk_saves: usize,
    disk_diagnostics: Vec<DiskDiagnostic>,
    disk_superseded_reported: bool,
    source_fingerprints: BTreeMap<String, u64>,
}

impl TextureCache {
    pub fn new_prepared(
        namespace: Result<ProjectTextureNamespace, String>,
        source_fingerprints: BTreeMap<String, u64>,
        max_bytes: usize,
        disk_max_bytes: u64,
    ) -> Self {
        Self::new_prepared_with(
            namespace,
            source_fingerprints,
            max_bytes,
            disk_max_bytes,
            global_worker_slot(),
            Arc::new(SystemRetryClock),
            DISK_START_TIMEOUT,
        )
    }

    #[cfg(test)]
    fn new_with_slot(
        project_root: &Path,
        source_fingerprints: BTreeMap<String, u64>,
        max_bytes: usize,
        disk_max_bytes: u64,
        worker_slot: Arc<WorkerSlot>,
    ) -> Self {
        Self::new_prepared_with(
            ProjectTextureNamespace::prepare(project_root).map_err(|error| error.to_string()),
            source_fingerprints,
            max_bytes,
            disk_max_bytes,
            worker_slot,
            Arc::new(SystemRetryClock),
            DISK_START_TIMEOUT,
        )
    }

    fn new_prepared_with(
        namespace: Result<ProjectTextureNamespace, String>,
        source_fingerprints: BTreeMap<String, u64>,
        max_bytes: usize,
        disk_max_bytes: u64,
        worker_slot: Arc<WorkerSlot>,
        retry_clock: Arc<dyn RetryClock>,
        retry_timeout: Duration,
    ) -> Self {
        let worker_claimant = worker_slot.designate_successor();
        let pending = namespace.map(|namespace| PendingDiskStart {
            prepared: namespace.claim(disk_max_bytes),
            claimant: worker_claimant,
            deadline: retry_clock.now() + retry_timeout,
        });
        let start =
            pending.map(|pending| TextureDiskWorker::start(pending, Arc::clone(&worker_slot)));
        let (disk_worker, pending_disk_start, disk_worker_starting, disk_diagnostics) = match start
        {
            Ok(WorkerStart::Started(worker)) => (Some(worker), None, true, Vec::new()),
            Ok(WorkerStart::Deferred(pending)) => (None, Some(pending), false, Vec::new()),
            Ok(WorkerStart::Superseded) => (None, None, false, Vec::new()),
            Ok(WorkerStart::Failed(error)) => (
                None,
                None,
                false,
                vec![DiskDiagnostic {
                    operation: DiskOperation::Open,
                    message: error.to_string(),
                }],
            ),
            Err(message) => (
                None,
                None,
                false,
                vec![DiskDiagnostic {
                    operation: DiskOperation::Open,
                    message,
                }],
            ),
        };
        Self {
            raster: Rasterizer::new(),
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
            queue: HashMap::new(),
            refreshed_from_live_text: HashSet::new(),
            retired: Vec::new(),
            disk_worker,
            pending_disk_start,
            worker_slot,
            worker_claimant,
            retry_clock,
            waiting_disk: HashMap::new(),
            disk_inflight: HashSet::new(),
            disk_worker_starting,
            clear_disk_pending: false,
            clear_disk_inflight: 0,
            pending_disk_saves: 0,
            disk_diagnostics,
            disk_superseded_reported: false,
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

    #[allow(dead_code)]
    pub fn used_bytes(&self) -> usize {
        self.bytes
    }

    #[allow(dead_code)]
    pub fn disk_state(&self) -> DiskState {
        if !self.worker_slot.is_newest(self.worker_claimant) {
            return DiskState::Unavailable;
        }
        let Some(worker) = &self.disk_worker else {
            return if self.pending_disk_start.is_some() {
                DiskState::Preparing
            } else {
                DiskState::Unavailable
            };
        };
        let state = *worker
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if matches!(state, DiskState::Ready { .. }) {
            if let Some(usage) = worker
                .shared_usage
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                return DiskState::Ready {
                    used_bytes: usage.load(Ordering::Acquire),
                };
            }
        }
        state
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

    /// Whether the cache contains a renderable image, rather than an empty
    /// placeholder produced while its source buffer was unavailable.
    pub fn has_image(&self, id: &SymbolId) -> bool {
        self.entries
            .get(id)
            .is_some_and(|entry| entry.tex.image.is_some())
    }

    /// Start a new visibility pass. Queued work remains available, but its
    /// old screen-area scores no longer outrank nodes in the current view.
    pub fn begin_visibility_frame(&mut self) {
        self.queue.values_mut().for_each(|priority| *priority = 0.0);
        self.waiting_disk
            .values_mut()
            .for_each(|priority| *priority = 0.0);
    }

    /// Cache lookup. Hit refreshes LRU; miss checks disk, then queues.
    pub fn get(&mut self, id: &SymbolId, screen_area: f64) -> Option<&NodeTexture> {
        self.clock += 1;
        if self.entries.contains_key(id) {
            let e = self.entries.get_mut(id).unwrap();
            e.last_used = self.clock;
            return Some(&e.tex);
        }
        if self.worker_slot.is_newest(self.worker_claimant)
            && self.disk_key(id).is_some()
            && self.disk_worker.is_some()
        {
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

    /// Queue one replacement bake after live text proves the source buffer is
    /// available. A fresh cache is created for each project load, so one
    /// refresh per node keeps scrolling responsive without rebaking every frame.
    pub fn refresh_from_live_text(&mut self, id: &SymbolId, screen_area: f64) -> bool {
        if !self.refreshed_from_live_text.insert(id.clone()) {
            if let Some(priority) = self.queue.get_mut(id) {
                *priority = priority.max(screen_area);
            }
            return false;
        }
        self.queue
            .entry(id.clone())
            .and_modify(|priority| *priority = priority.max(screen_area))
            .or_insert(screen_area);
        true
    }

    pub fn has_queued(&self) -> bool {
        !self.queue.is_empty()
            || !self.waiting_disk.is_empty()
            || self.disk_worker_starting
            || self.pending_disk_start.is_some()
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
    #[allow(dead_code)]
    pub fn process_requests(
        &mut self,
        bake_fn: impl FnMut(&SymbolId, &mut Rasterizer) -> Option<NodeTexture>,
    ) -> bool {
        self.process_requests_grouped(|_| None, bake_fn)
    }

    /// Bake visible work in priority order, using the highest-priority item's
    /// group to finish other visible requests backed by the same source file.
    pub fn process_requests_grouped(
        &mut self,
        mut group_fn: impl FnMut(&SymbolId) -> Option<String>,
        mut bake_fn: impl FnMut(&SymbolId, &mut Rasterizer) -> Option<NodeTexture>,
    ) -> bool {
        self.handle_disk_superseded();
        self.retry_disk_worker_start();
        self.apply_disk_results();
        self.dispatch_clear_request();
        self.dispatch_disk_loads();
        let mut queue: Vec<_> = std::mem::take(&mut self.queue).into_iter().collect();
        queue.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut selected = Vec::with_capacity(BAKES_PER_FRAME);
        while selected.len() < BAKES_PER_FRAME && !queue.is_empty() {
            let first = queue.remove(0);
            let group = group_fn(&first.0);
            selected.push(first);
            let Some(group) = group else { continue };
            while selected.len() < BAKES_PER_FRAME {
                let Some(index) = queue.iter().position(|(id, priority)| {
                    *priority > 0.0 && group_fn(id).as_deref() == Some(group.as_str())
                }) else {
                    break;
                };
                selected.push(queue.remove(index));
            }
        }
        self.queue.extend(queue);
        for (id, _) in selected {
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

    /// Remove a composited texture so it is rebuilt from its refreshed child
    /// images. The next paint keeps rendering its visible children meanwhile.
    pub fn invalidate(&mut self, id: &SymbolId) {
        let Some(replaced) = self.entries.remove(id) else {
            return;
        };
        self.bytes -= replaced.tex.bytes;
        if let Some(image) = replaced.tex.image {
            self.retired.push(image);
        }
    }

    fn insert_entry(&mut self, id: SymbolId, tex: NodeTexture) {
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
        let Some(worker) = &self.disk_worker else {
            return;
        };
        if !self.worker_slot.is_newest(self.worker_claimant)
            || worker.cancelled.load(Ordering::Acquire)
        {
            return;
        }
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
        if worker
            .commands
            .try_send(DiskCommand::Save { key, payload })
            .is_ok()
        {
            self.pending_disk_saves = self.pending_disk_saves.saturating_add(1);
        }
    }

    fn retry_disk_worker_start(&mut self) {
        let Some(pending) = self.pending_disk_start.take() else {
            return;
        };
        if self.retry_clock.now() >= pending.deadline {
            self.disk_diagnostics.push(DiskDiagnostic {
                operation: DiskOperation::Open,
                message: "timed out waiting for the previous texture cache worker to retire".into(),
            });
            return;
        }
        match TextureDiskWorker::start(pending, Arc::clone(&self.worker_slot)) {
            WorkerStart::Started(worker) => {
                self.disk_worker = Some(worker);
                self.disk_worker_starting = true;
            }
            WorkerStart::Deferred(pending) => self.pending_disk_start = Some(pending),
            WorkerStart::Superseded => self.handle_disk_superseded(),
            WorkerStart::Failed(error) => {
                self.disk_diagnostics.push(DiskDiagnostic {
                    operation: DiskOperation::Open,
                    message: error.to_string(),
                });
            }
        }
    }

    fn handle_disk_superseded(&mut self) {
        if self.worker_slot.is_newest(self.worker_claimant) {
            return;
        }
        self.pending_disk_start = None;
        self.disk_worker = None;
        let waiting = std::mem::take(&mut self.waiting_disk);
        self.disk_inflight.clear();
        for (id, priority) in waiting {
            self.request(id, priority);
        }
        self.disk_worker_starting = false;
        self.clear_disk_pending = false;
        self.clear_disk_inflight = 0;
        self.pending_disk_saves = 0;
        if !self.disk_superseded_reported {
            self.disk_superseded_reported = true;
            self.disk_diagnostics.push(DiskDiagnostic {
                operation: DiskOperation::Open,
                message: "texture cache superseded by a newer project".into(),
            });
        }
    }

    fn apply_disk_results(&mut self) {
        for _ in 0..BAKES_PER_FRAME {
            let received = self
                .disk_worker
                .as_ref()
                .map(|worker| worker.results.try_recv());
            let result = match received {
                Some(Ok(result)) => result,
                Some(Err(mpsc::TryRecvError::Empty)) | None => break,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.handle_disk_disconnect();
                    break;
                }
            };
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

    fn handle_disk_disconnect(&mut self) {
        let waiting = std::mem::take(&mut self.waiting_disk);
        self.disk_inflight.clear();
        for (id, priority) in waiting {
            self.request(id, priority);
        }
        if self.clear_disk_pending || self.clear_disk_inflight > 0 {
            self.disk_diagnostics.push(DiskDiagnostic {
                operation: DiskOperation::Clear,
                message: "texture cache worker disconnected".into(),
            });
        }
        self.disk_worker_starting = false;
        self.clear_disk_pending = false;
        self.clear_disk_inflight = 0;
        self.pending_disk_saves = 0;
        self.disk_worker = None;
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
            if self.pending_disk_start.is_some() {
                return;
            }
            self.clear_disk_pending = false;
            self.disk_diagnostics.push(DiskDiagnostic {
                operation: DiskOperation::Clear,
                message: "texture cache unavailable".into(),
            });
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
        let worker_slot = Arc::new(WorkerSlot::new());
        let worker_claimant = worker_slot.designate_successor();
        Self {
            raster: Rasterizer::new(),
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
            queue: HashMap::new(),
            refreshed_from_live_text: HashSet::new(),
            retired: Vec::new(),
            disk_worker: None,
            pending_disk_start: None,
            worker_slot,
            worker_claimant,
            retry_clock: Arc::new(SystemRetryClock),
            waiting_disk: HashMap::new(),
            disk_inflight: HashSet::new(),
            disk_worker_starting: false,
            clear_disk_pending: false,
            clear_disk_inflight: 0,
            pending_disk_saves: 0,
            disk_diagnostics: Vec::new(),
            disk_superseded_reported: false,
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
    fn cached_python_item_bakes_with_file_type_item_fill() {
        let child = SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Item {
                    label: "class".into(),
                },
                qualified_path: "main.py::Widget".into(),
                ordinal: 0,
            },
            name: "Widget".into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 0,
            churn: 0.0,
            churn_count: 0,
            children: Vec::new(),
        };
        let root = SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Folder,
                qualified_path: "src".into(),
                ordinal: 0,
            },
            name: "src".into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 0,
            churn: 0.0,
            churn_count: 0,
            children: vec![child.clone()],
        };
        let root_rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let child_rect = Rect {
            x: 10.0,
            y: 10.0,
            w: 80.0,
            h: 80.0,
        };
        let layout = PackLayout {
            rects: [(root.id.clone(), root_rect), (child.id.clone(), child_rect)]
                .into_iter()
                .collect(),
        };

        let texture = bake_container(&root, root_rect, &layout, 0, &|_| None);
        let image = texture.image.expect("container texture");
        let bytes = image.as_bytes(0).expect("BGRA pixels");
        let center = (512 * 1024 + 512) * 4;
        let fill = theme::box_fill(
            theme::BoxKind::Item,
            1,
            theme::BoxTint::FileType(theme::extension_tint("py")),
        );
        let expected_bgra = [fill as u8, (fill >> 8) as u8, (fill >> 16) as u8, 255];
        assert_eq!(&bytes[center..center + 4], &expected_bgra);
    }

    #[test]
    fn render_schema_tracks_shared_file_type_classification() {
        assert_eq!(RENDER_SCHEMA_VERSION, 3);
    }

    #[test]
    fn bake_dimensions_single_image() {
        let lines: Vec<Line> = (0..10).map(|_| plain("fn foo() {}")).collect();
        let tex = Rasterizer::new().bake(&lines);
        let img = tex.image.as_ref().unwrap();
        assert_eq!(img.size(0).width.0, 164);
        assert_eq!(img.size(0).height.0, 40);
        assert_eq!(tex.bytes, 164 * 40 * 4);
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
    fn visibility_frame_replaces_stale_priorities_with_current_areas() {
        let mut cache = TextureCache::new_memory_only(1024);
        cache.request(sid("was-large"), 1_000.0);
        cache.request(sid("now-large"), 10.0);
        cache.request(sid("offscreen"), 500.0);

        cache.begin_visibility_frame();
        cache.request(sid("was-large"), 5.0);
        cache.request(sid("now-large"), 100.0);

        assert_eq!(
            cache.queued_ids(),
            vec![sid("now-large"), sid("was-large"), sid("offscreen")]
        );
    }

    #[test]
    fn grouped_bakes_finish_visible_leaves_from_the_first_file() {
        let mut cache = TextureCache::new_memory_only(DEFAULT_CACHE_MB as usize * 1024 * 1024);
        cache.request(sid("a.rs::first"), 100.0);
        cache.request(sid("b.rs::first"), 90.0);
        cache.request(sid("a.rs::second"), 80.0);
        cache.request(sid("a.rs::third"), 70.0);
        cache.request(sid("b.rs::second"), 60.0);
        let mut baked = Vec::new();

        let remaining = cache.process_requests_grouped(
            |id| id.qualified_path.split("::").next().map(str::to_owned),
            |id, _| {
                baked.push(id.clone());
                Some(texture(1))
            },
        );

        assert!(remaining);
        assert_eq!(
            baked,
            vec![
                sid("a.rs::first"),
                sid("a.rs::second"),
                sid("a.rs::third"),
                sid("b.rs::first"),
            ]
        );
    }

    #[test]
    fn image_readiness_rejects_empty_cached_texture() {
        let mut cache = TextureCache::new_memory_only(1024 * 1024);
        cache.insert(sid("empty"), texture(0));
        assert!(!cache.has_image(&sid("empty")));

        cache.insert(
            sid("rendered"),
            some_tex(1, &mut Rasterizer::new()).unwrap(),
        );
        assert!(cache.has_image(&sid("rendered")));
    }

    #[test]
    fn live_text_refresh_queues_cached_texture_once() {
        let mut cache = TextureCache::new_memory_only(1024);
        cache.insert(sid("leaf"), some_tex(1, &mut Rasterizer::new()).unwrap());

        assert!(cache.refresh_from_live_text(&sid("leaf"), 100.0));
        assert_eq!(cache.queued_ids(), vec![sid("leaf")]);
        assert!(!cache.refresh_from_live_text(&sid("leaf"), 100.0));
    }

    #[test]
    fn invalidating_a_texture_removes_its_renderable_image() {
        let mut cache = TextureCache::new_memory_only(1024 * 1024);
        cache.insert(sid("folder"), some_tex(1, &mut Rasterizer::new()).unwrap());

        cache.invalidate(&sid("folder"));

        assert!(!cache.contains(&sid("folder")));
        assert!(!cache.has_image(&sid("folder")));
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
        allow_open.send(()).unwrap();
        drop(worker);
    }

    #[test]
    fn repeated_replacements_bound_retirement_and_deferred_cache_auto_starts() {
        let project = tempfile::tempdir().unwrap();
        let slot = Arc::new(WorkerSlot::new());
        let fingerprints = BTreeMap::from([("a".into(), 1)]);
        let (allow_retirement, retirement_blocked) = mpsc::channel();

        let first_worker = TextureDiskWorker::spawn_with_opener_and_slot(
            move || {
                retirement_blocked.recv().unwrap();
                Err(std::io::Error::other("retired test worker"))
            },
            Arc::clone(&slot),
        );
        let mut first = TextureCache::new_memory_only(1024 * 1024);
        first.source_fingerprints.insert("a".into(), 1);
        first.disk_worker = Some(first_worker);
        wait_for_counter_for_test(&slot.started, 1);

        let mut middle = TextureCache::new_with_slot(
            project.path(),
            fingerprints.clone(),
            1024 * 1024,
            1024,
            Arc::clone(&slot),
        );
        first.insert(sid("a"), some_tex(1, &mut Rasterizer::new()).unwrap());
        assert_eq!(first.pending_disk_saves, 0);
        drop(first);
        let mut current = TextureCache::new_with_slot(
            project.path(),
            fingerprints,
            1024 * 1024,
            1024,
            Arc::clone(&slot),
        );
        middle.insert(sid("a"), some_tex(1, &mut Rasterizer::new()).unwrap());
        current.insert(sid("a"), some_tex(1, &mut Rasterizer::new()).unwrap());

        assert_eq!(slot.started.load(Ordering::Acquire), 1);
        assert_eq!(slot.retired.load(Ordering::Acquire), 0);
        assert!(middle.disk_worker.is_none());
        assert!(middle.pending_disk_start.is_some());
        assert_eq!(middle.pending_disk_saves, 0);
        assert!(current.disk_worker.is_none());
        assert!(current.pending_disk_start.is_some());
        assert_eq!(current.pending_disk_saves, 0);
        assert_eq!(current.disk_state(), DiskState::Preparing);
        middle.request(sid("memory-only"), 100.0);
        middle.process_requests(|_, _| Some(texture(1)));
        assert!(middle.contains(&sid("memory-only")));
        assert_eq!(middle.disk_state(), DiskState::Unavailable);
        assert!(middle.pending_disk_start.is_none());
        drop(middle);

        allow_retirement.send(()).unwrap();
        wait_for_counter_for_test(&slot.retired, 1);
        for _ in 0..100 {
            current.process_requests(|_, _| None);
            if current.disk_worker.is_some() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(current.disk_worker.is_some());
        wait_for_counter_for_test(&slot.started, 2);
        assert_eq!(slot.started.load(Ordering::Acquire), 2);
        assert!(!current
            .disk_worker
            .as_ref()
            .unwrap()
            .cancelled
            .load(Ordering::Acquire));
    }

    struct FakeRetryClock {
        base: Instant,
        elapsed_millis: AtomicU64,
    }

    impl FakeRetryClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                elapsed_millis: AtomicU64::new(0),
            }
        }

        fn advance(&self, duration: Duration) {
            self.elapsed_millis
                .fetch_add(duration.as_millis() as u64, Ordering::AcqRel);
        }
    }

    impl RetryClock for FakeRetryClock {
        fn now(&self) -> Instant {
            self.base + Duration::from_millis(self.elapsed_millis.load(Ordering::Acquire))
        }
    }

    #[test]
    fn deferred_worker_times_out_once_and_future_cache_can_claim() {
        let project = tempfile::tempdir().unwrap();
        let slot = Arc::new(WorkerSlot::new());
        let clock = Arc::new(FakeRetryClock::new());
        let (release_retiree, blocked_retiree) = mpsc::channel();
        let retiree = TextureDiskWorker::spawn_with_opener_and_slot(
            move || {
                blocked_retiree.recv().unwrap();
                Err(std::io::Error::other("released retiree"))
            },
            Arc::clone(&slot),
        );
        wait_for_counter_for_test(&slot.started, 1);

        let namespace = ProjectTextureNamespace::prepare(project.path()).unwrap();
        let mut deferred = TextureCache::new_prepared_with(
            Ok(namespace.clone()),
            BTreeMap::new(),
            1024,
            1024,
            Arc::clone(&slot),
            clock.clone(),
            Duration::from_secs(10),
        );
        assert_eq!(deferred.disk_state(), DiskState::Preparing);
        assert!(deferred.has_queued());

        clock.advance(Duration::from_secs(11));
        deferred.process_requests(|_, _| None);
        assert_eq!(deferred.disk_state(), DiskState::Unavailable);
        assert!(!deferred.has_queued());
        assert_eq!(deferred.drain_disk_diagnostics().len(), 1);
        deferred.process_requests(|_, _| None);
        assert!(deferred.drain_disk_diagnostics().is_empty());

        drop(retiree);
        release_retiree.send(()).unwrap();
        wait_for_counter_for_test(&slot.retired, 1);
        let mut future = TextureCache::new_prepared_with(
            Ok(namespace),
            BTreeMap::new(),
            1024,
            1024,
            Arc::clone(&slot),
            clock,
            Duration::from_secs(10),
        );
        for _ in 0..100 {
            future.process_requests(|_, _| None);
            if future.disk_worker.is_some() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(future.disk_worker.is_some());
    }

    fn wait_for_counter_for_test(counter: &AtomicUsize, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while counter.load(Ordering::Acquire) < expected {
            assert!(
                std::time::Instant::now() < deadline,
                "counter did not advance"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn disk_result_channel_applies_backpressure() {
        let (results, _receiver) = disk_result_channel();
        for ordinal in 0..DISK_RESULT_CAPACITY {
            results
                .try_send(DiskResult::Loaded {
                    id: sid(&format!("result-{ordinal}")),
                    payload: TexturePayload {
                        width: 1,
                        height: 1,
                        bytes: vec![0; 4],
                    },
                })
                .unwrap();
        }
        assert!(matches!(
            results.try_send(DiskResult::Loaded {
                id: sid("full"),
                payload: TexturePayload {
                    width: 1,
                    height: 1,
                    bytes: vec![0; 4],
                },
            }),
            Err(mpsc::TrySendError::Full(_))
        ));
    }

    #[test]
    fn dropping_worker_returns_while_open_is_blocked() {
        let (allow_open, wait_for_open) = mpsc::channel();
        let worker = TextureDiskWorker::spawn_with_opener(move || {
            wait_for_open.recv().unwrap();
            Err(std::io::Error::other("open released after drop"))
        });

        let (dropped_tx, dropped_rx) = mpsc::channel();
        std::thread::spawn(move || {
            drop(worker);
            dropped_tx.send(()).unwrap();
        });
        dropped_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        allow_open.send(()).unwrap();
    }

    #[test]
    fn failed_store_open_transitions_disk_state_to_unavailable() {
        let worker = TextureDiskWorker::spawn_with_opener(|| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected open failure",
            ))
        });

        loop {
            if matches!(
                worker.results.recv_timeout(Duration::from_secs(1)).unwrap(),
                DiskResult::OpenComplete
            ) {
                break;
            }
        }
        assert_eq!(
            *worker
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            DiskState::Unavailable
        );
    }

    #[test]
    fn every_explicit_clear_failure_emits_a_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let store = TextureStore::open_at(dir.path(), "project", 1024).unwrap();
        let namespace = store.dir_for_test().to_path_buf();
        std::fs::remove_dir(&namespace).unwrap();
        std::fs::write(&namespace, b"not a directory").unwrap();
        let worker = TextureDiskWorker::spawn_with_opener(move || Ok(store));

        while !matches!(
            worker.results.recv_timeout(Duration::from_secs(1)).unwrap(),
            DiskResult::OpenComplete
        ) {}
        worker.commands.send(DiskCommand::Clear).unwrap();
        worker.commands.send(DiskCommand::Clear).unwrap();

        let mut clear_diagnostics = 0;
        while clear_diagnostics < 2 {
            if let DiskResult::Diagnostic(diagnostic) =
                worker.results.recv_timeout(Duration::from_secs(1)).unwrap()
            {
                if diagnostic.operation == DiskOperation::Clear {
                    clear_diagnostics += 1;
                }
            }
        }
        assert_eq!(clear_diagnostics, 2);
    }

    #[test]
    fn replacement_worker_observes_usage_written_by_older_worker() {
        let dir = tempfile::tempdir().unwrap();
        let limit = 1024;
        let older_store = TextureStore::open_at(dir.path(), "project", limit).unwrap();
        let replacement_store = TextureStore::open_at(dir.path(), "project", limit).unwrap();
        let older = TextureDiskWorker::spawn_with_opener(move || Ok(older_store));
        let replacement = TextureDiskWorker::spawn_with_opener(move || Ok(replacement_store));
        for worker in [&older, &replacement] {
            while !matches!(
                worker.results.recv_timeout(Duration::from_secs(1)).unwrap(),
                DiskResult::OpenComplete
            ) {}
        }
        let payload = TexturePayload {
            width: 4,
            height: 1,
            bytes: vec![0; 16],
        };
        let key = TextureKey::new("a.rs", 1, &sid("a.rs"), 1, 1);
        older
            .commands
            .send(DiskCommand::Save { key, payload })
            .unwrap();
        while !matches!(
            older.results.recv_timeout(Duration::from_secs(1)).unwrap(),
            DiskResult::SaveComplete
        ) {}

        let mut cache = TextureCache::new_memory_only(1024);
        cache.disk_worker = Some(replacement);
        assert_eq!(
            cache.disk_state(),
            DiskState::Ready {
                used_bytes: (crate::texture_store::HEADER_LEN + 16) as u64,
            }
        );
    }

    #[test]
    fn worker_open_lifecycle_keeps_empty_cache_pending_until_completion() {
        let (command_tx, _command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let (result_tx, result_rx) = mpsc::channel();
        let mut cache = TextureCache::new_memory_only(1024);
        cache.disk_worker = Some(TextureDiskWorker {
            commands: command_tx,
            results: result_rx,
            state: Arc::new(Mutex::new(DiskState::Preparing)),
            shared_usage: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            control: None,
        });
        cache.disk_worker_starting = true;

        assert!(cache.has_queued());
        result_tx.send(DiskResult::OpenComplete).unwrap();
        cache.process_requests(|_, _| panic!("no texture should bake"));
        assert!(!cache.has_queued());
    }

    #[test]
    fn result_disconnect_requeues_inflight_loads_and_clears_lifecycle() {
        let (command_tx, _command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let (result_tx, result_rx) = mpsc::sync_channel(DISK_RESULT_CAPACITY);
        drop(result_tx);
        let mut cache = TextureCache::new_memory_only(1024);
        cache.disk_worker = Some(TextureDiskWorker {
            commands: command_tx,
            results: result_rx,
            state: Arc::new(Mutex::new(DiskState::Preparing)),
            shared_usage: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            control: None,
        });
        cache.waiting_disk.insert(sid("a"), 100.0);
        cache.waiting_disk.insert(sid("b"), 50.0);
        cache.disk_inflight.insert(sid("a"));
        cache.disk_worker_starting = true;
        cache.clear_disk_inflight = 1;
        cache.pending_disk_saves = 1;

        cache.process_requests(|_, _| Some(texture(1)));

        assert!(cache.disk_worker.is_none());
        assert!(cache.contains(&sid("a")));
        assert!(cache.contains(&sid("b")));
        assert!(!cache.has_queued());
    }

    #[test]
    fn every_clear_without_a_worker_emits_a_diagnostic() {
        let mut cache = TextureCache::new_memory_only(1024);
        for _ in 0..2 {
            cache.request_clear_disk_cache();
            cache.process_requests(|_, _| panic!("no texture should bake"));
        }

        let diagnostics = cache.drain_disk_diagnostics();
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.operation == DiskOperation::Clear));
    }

    #[test]
    fn clear_keeps_cache_pending_until_worker_completion() {
        let (command_tx, command_rx) = mpsc::sync_channel(DISK_QUEUE_CAPACITY);
        let (result_tx, result_rx) = mpsc::channel();
        let mut cache = TextureCache::new_memory_only(1024);
        cache.disk_worker = Some(TextureDiskWorker {
            commands: command_tx,
            results: result_rx,
            state: Arc::new(Mutex::new(DiskState::Preparing)),
            shared_usage: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            control: None,
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
            state: Arc::new(Mutex::new(DiskState::Preparing)),
            shared_usage: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            control: None,
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
            state: Arc::new(Mutex::new(DiskState::Preparing)),
            shared_usage: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            control: None,
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
            state: Arc::new(Mutex::new(DiskState::Preparing)),
            shared_usage: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            control: None,
        });
        cache.get(&sid("a"), 10.0);

        cache.process_requests(|_, _| Some(texture(1)));

        assert!(cache.contains(&sid("a")));
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
