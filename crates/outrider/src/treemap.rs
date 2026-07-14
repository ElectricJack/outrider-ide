//! Main GPUI view for the outrider treemap — drives the render loop, handles
//! all input (mouse drag/zoom/click, keyboard navigation), and translates the
//! world-space layout from `outrider-layout` into per-frame paint instructions
//! (quads, text runs, and baked texture quads) via a static canvas closure.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, rgba, size, transparent_black, App, BorderStyle,
    Bounds, ContentMask, Context, Corners, ElementId, FocusHandle, Pixels, TextAlign, TextRun,
    Window,
};
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::{PackLayout, Rect};

use crate::buffers::{collect_file_symbols, BufferManager};
use crate::camera::{self, Camera, CameraTween};
use crate::chrome;
use crate::content::{self, BodyLine, FONT_PX, HEADER, LINE_STEP};
use crate::focus::{self, Focus, TreeIndex};
use crate::interaction::InteractionAction;
use crate::navigation::NavigationHistory;
use crate::overlays::{ContextMenu, Notification, Notifications};
use crate::paint_model::{
    code_line, runs_from_spans, truncate_to_width, wrap_doc, BodyText, DocPanel, NameRow,
    PaintItem, TexQuad,
};
use crate::palette;
use crate::project_loader::{LoadProgress, LoadResult, LoaderPoll, ProjectLoader};
use crate::rasterize::{self, DiskState, TextureCache};
use crate::settings;
use crate::theme;
use crate::world::{self, Draw, LeafDraw, Rung};

/// Left text inset shared by name rows and body rows.
pub(crate) const BODY_PAD: f64 = 6.0;

/// Width of the floating doc panel shown to the right of the focused leaf.
const DOC_PANEL_W: f64 = 280.0;

#[derive(Clone, Copy, PartialEq)]
enum SettingsField {
    Extensions,
    Folders,
    CacheMb,
    DiskCacheGb,
}

struct SettingsDraft {
    filter_extensions: String,
    filter_folders: String,
    cache_mb: String,
    disk_cache_gb: String,
    notification: Option<String>,
    active: SettingsField,
}

impl SettingsDraft {
    fn from_settings(s: &settings::Settings, project: &std::path::Path) -> Self {
        Self {
            filter_extensions: s.filter_extensions.join(", "),
            filter_folders: s.filter_folders.join(", "),
            cache_mb: s.cache_mb.to_string(),
            disk_cache_gb: format_gibibytes(s.disk_cache_bytes(project)),
            notification: None,
            active: SettingsField::Extensions,
        }
    }

    fn active_text_mut(&mut self) -> &mut String {
        match self.active {
            SettingsField::Extensions => &mut self.filter_extensions,
            SettingsField::Folders => &mut self.filter_folders,
            SettingsField::CacheMb => &mut self.cache_mb,
            SettingsField::DiskCacheGb => &mut self.disk_cache_gb,
        }
    }

    fn apply_to(
        &self,
        settings: &mut settings::Settings,
        project: &std::path::Path,
    ) -> Result<(), String> {
        let parse_list = |s: &str| -> Vec<String> {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        };
        let cache_mb =
            self.cache_mb.trim().parse::<u32>().map_err(|_| {
                "Texture cache must be a whole number of MB within range".to_string()
            })?;
        if cache_mb == 0 || cache_mb > settings::MAX_CACHE_MB {
            return Err(format!(
                "Texture cache must be between 1 and {} MB",
                settings::MAX_CACHE_MB
            ));
        }
        let disk_cache_bytes = parse_gibibytes(&self.disk_cache_gb)?;
        settings.filter_extensions = parse_list(&self.filter_extensions);
        settings.filter_folders = parse_list(&self.filter_folders);
        settings.show_welcome = false;
        settings.cache_mb = cache_mb;
        settings.set_disk_cache_bytes(project, disk_cache_bytes);
        Ok(())
    }
}

fn format_gibibytes(bytes: u64) -> String {
    let whole = bytes / settings::DEFAULT_DISK_CACHE_BYTES;
    let remainder = bytes % settings::DEFAULT_DISK_CACHE_BYTES;
    if remainder == 0 {
        return whole.to_string();
    }
    let fraction = u128::from(remainder) * 5_u128.pow(30);
    let fraction = format!("{fraction:030}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

fn parse_gibibytes(input: &str) -> Result<u64, String> {
    let input = input.trim();
    let mut parts = input.split('.');
    let whole = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| "Disk cache must be a decimal number of GiB".to_string())?;
    let fraction = parts.next();
    if parts.next().is_some()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction
            .is_some_and(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("Disk cache must be a decimal number of GiB".into());
    }

    let whole = whole
        .parse::<u64>()
        .map_err(|_| "Disk cache size is too large".to_string())?;
    let mut bytes = whole
        .checked_mul(settings::DEFAULT_DISK_CACHE_BYTES)
        .ok_or_else(|| "Disk cache size is too large".to_string())?;
    if let Some(fraction) = fraction {
        let fraction = fraction.trim_end_matches('0');
        if !fraction.is_empty() {
            if fraction.len() > 30 {
                return Err("Disk cache size is too precise".into());
            }
            let numerator = fraction
                .parse::<u128>()
                .map_err(|_| "Disk cache size is too large".to_string())?;
            let denominator = 10_u128
                .checked_pow(fraction.len() as u32)
                .ok_or_else(|| "Disk cache size is too precise".to_string())?;
            let mut left = denominator;
            let mut right = u128::from(settings::DEFAULT_DISK_CACHE_BYTES);
            while right != 0 {
                (left, right) = (right, left % right);
            }
            let common_factor = left;
            let fractional_bytes = numerator
                .checked_mul(u128::from(settings::DEFAULT_DISK_CACHE_BYTES) / common_factor)
                .ok_or_else(|| "Disk cache size is too large".to_string())?
                / (denominator / common_factor);
            let fractional_bytes = u64::try_from(fractional_bytes)
                .map_err(|_| "Disk cache size is too large".to_string())?;
            bytes = bytes
                .checked_add(fractional_bytes)
                .ok_or_else(|| "Disk cache size is too large".to_string())?;
        }
    }
    if bytes == 0 {
        return Err("Disk cache must be greater than zero GiB".into());
    }
    Ok(bytes)
}

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
    textures: Option<TextureCache>,
    bake_pending: bool,
    /// The four beam-cast arrow targets of the focused node (Left, Right,
    /// Up, Down), cached because layout is immutable per session.
    neighbors: Option<(SymbolId, [Option<SymbolId>; 4])>,
    /// Leaf node currently under the mouse cursor (for doc tooltip).
    hover_id: Option<SymbolId>,
    /// Browser-style history of explicit focus visits (not arrow-key movement).
    nav_history: NavigationHistory,
    /// Search palette (Ctrl+P = file mode, Ctrl+T = symbol mode).
    palette: palette::Palette,
    /// Persisted user preferences.
    settings: settings::Settings,
    /// Whether to show the welcome overlay this session.
    show_welcome: bool,
    /// Working copy of settings while the settings panel is open.
    settings_draft: Option<SettingsDraft>,
    /// Recoverable settings load/save and validation feedback.
    notifications: Notifications,
    /// Right-click context menu, if currently open.
    context_menu: Option<ContextMenu>,
    /// Background indexing controller (Open Folder, startup, or re-index).
    loader: ProjectLoader,
    load_progress: Option<LoadProgress>,
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
                        out.push(BodyText {
                            x,
                            y: y as f32,
                            text: shown,
                            runs,
                        });
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
            out.push(BodyText {
                x,
                y: y as f32,
                text: shown,
                runs: vec![(len, color)],
            });
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

/// Rendered container-header height for a positive [`Camera`] zoom.
fn container_header_px(zoom: f64) -> f64 {
    ((HEADER + 2.0 * LINE_STEP) * zoom.min(1.0)).max(HEADER)
}

fn container_header_bg_h(body_len: usize, max_h: f64) -> f64 {
    (HEADER + body_len as f64 * LINE_STEP).min(max_h)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ContainerHeaderLayout {
    pin_y: f64,
    max_h: f64,
}

/// Computes pinned-header geometry within the clipped container. `zoom` is a
/// positive [`Camera`] zoom. Card-or-higher headers require one complete line
/// after ancestor stacking and never extend past the container's trailing edge.
fn container_header_layout(
    rung: Rung,
    clipped_y: f64,
    clipped_h: f64,
    stack_bottom: f64,
    zoom: f64,
) -> Option<ContainerHeaderLayout> {
    let pin_y = clipped_y.max(stack_bottom);
    match rung {
        Rung::Dot => None,
        Rung::Label if clipped_h >= 14.0 => Some(ContainerHeaderLayout {
            pin_y,
            max_h: clipped_h,
        }),
        Rung::Label => None,
        Rung::Card | Rung::Detail | Rung::Full => {
            let available = clipped_y + clipped_h - pin_y;
            (available >= HEADER).then(|| ContainerHeaderLayout {
                pin_y,
                max_h: container_header_px(zoom).min(available),
            })
        }
    }
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
    let hdr = container_header_px(cam.zoom);
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
        let Some(r) = layout.rects.get(anc) else {
            continue;
        };
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

fn resolve_fs_path(id: &SymbolId, repo_root: &std::path::Path) -> std::path::PathBuf {
    let rel = match id.kind {
        SymbolKind::Folder => id.qualified_path.as_str(),
        _ => crate::buffers::BufferManager::file_path_of(&id.qualified_path),
    };
    repo_root.join(rel)
}

fn open_in_file_manager(path: &std::path::Path) {
    use std::process::Command;

    if cfg!(target_os = "windows") {
        if path.is_dir() {
            let _ = Command::new("explorer.exe").arg(path).spawn();
        } else {
            let _ = Command::new("explorer.exe")
                .arg(format!("/select,{}", path.display()))
                .spawn();
        }
    } else if std::path::Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists() {
        if let Ok(output) = Command::new("wslpath").arg("-w").arg(path).output() {
            let win_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if path.is_dir() {
                let _ = Command::new("explorer.exe").arg(&win_path).spawn();
            } else {
                let _ = Command::new("explorer.exe")
                    .arg(format!("/select,{win_path}"))
                    .spawn();
            }
        }
    } else {
        let dir = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        let _ = Command::new("xdg-open").arg(&dir).spawn();
    }
}

fn loading_texture_cache() -> Option<TextureCache> {
    None
}

fn format_cache_status(memory_bytes: usize, memory_max_mb: u64, disk_state: DiskState) -> String {
    let memory_mb = memory_bytes as f64 / (1024.0 * 1024.0);
    let disk = match disk_state {
        DiskState::Preparing => "Disk preparing…".to_owned(),
        DiskState::Ready { used_bytes } => {
            format!("Disk {:.0} MB", used_bytes as f64 / (1024.0 * 1024.0))
        }
        DiskState::Unavailable => "Disk unavailable".to_owned(),
    };
    format!("Memory {memory_mb:.0} / {memory_max_mb} MB · {disk}")
}

/// Construction, camera helpers, and the per-frame paint pipeline.
impl TreemapView {
    fn apply_action(&mut self, action: InteractionAction) {
        match action {
            InteractionAction::DismissNotification => self.notifications.dismiss_visible(),
        }
    }

    /// Construct a responsive shell and begin indexing only after GPUI has
    /// entered its application callback.
    pub fn loading_shell(
        project_root: PathBuf,
        loaded_settings: settings::SettingsLoad,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_name = project_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| project_root.to_string_lossy().into_owned());
        let root = SymbolNode {
            id: SymbolId {
                kind: SymbolKind::Folder,
                qualified_path: String::new(),
                ordinal: 0,
            },
            name: project_name,
            byte_range: None,
            signature: None,
            doc: None,
            measure: 0,
            churn: 0.0,
            churn_count: 0,
            children: Vec::new(),
        };
        let tree = SymbolTree {
            root,
            repo_root: project_root.clone(),
        };
        let layout = outrider_layout::pack(&tree, &world::pack_config());
        let (settings, settings_notification) = loaded_settings.into_parts();
        let mut view = Self::from_parts(
            tree,
            layout,
            settings,
            settings_notification,
            loading_texture_cache(),
            cx,
        );
        let show_welcome = view.show_welcome;
        view.start_loading(project_root);
        view.show_welcome = show_welcome;
        view
    }

    fn from_parts(
        tree: SymbolTree,
        layout: PackLayout,
        settings: settings::Settings,
        settings_notification: Option<String>,
        textures: Option<TextureCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let root_id = tree.root.id.clone();
        let file_symbols = collect_file_symbols(&tree);
        let buffers = BufferManager::new(tree.repo_root.clone());
        let show_welcome = settings.show_welcome;
        let mut notifications = Notifications::default();
        if let Some(message) = settings_notification {
            notifications.push(Notification::warning(message));
        }
        Self {
            tree,
            layout,
            camera: None,
            home_zoom: 1.0,
            drag_last: None,
            press_origin: None,
            focus: Focus::new(root_id.clone()),
            tween: None,
            focus_handle: cx.focus_handle(),
            buffers,
            file_symbols,
            textures,
            bake_pending: false,
            neighbors: None,
            hover_id: None,
            nav_history: NavigationHistory::new(root_id, 64),
            palette: palette::Palette::new(),
            settings,
            show_welcome,
            settings_draft: None,
            notifications,
            context_menu: None,
            loader: ProjectLoader::new(),
            load_progress: None,
        }
    }

    /// World-space rect of the root node, used for Home framing.
    fn root_rect(&self) -> Rect {
        self.layout
            .rects
            .get(&self.tree.root.id)
            .copied()
            .unwrap_or(Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            })
    }

    fn memory_status(&self) -> String {
        let memory_bytes = self
            .textures
            .as_ref()
            .map(TextureCache::used_bytes)
            .unwrap_or(0);
        let disk_state = self
            .textures
            .as_ref()
            .map(TextureCache::disk_state)
            .unwrap_or(DiskState::Preparing);
        format_cache_status(memory_bytes, u64::from(self.settings.cache_mb), disk_state)
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
        (
            f64::from(vp.width),
            f64::from(vp.height) - chrome::TITLEBAR_H,
        )
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
        let leaf = index
            .node(&self.focus.current)
            .is_some_and(content::is_leaf_item);
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
        Some(NameRow {
            x: (item.px.x + BODY_PAD) as f32,
            y: y as f32,
            font_px: font,
            text,
        })
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
            self.neighbors = Some((
                focus_id.clone(),
                focus::neighbors(&focus_id, &self.layout, &index),
            ));
        }
        let (_, neighbor_ids) = self.neighbors.clone().unwrap();
        let items = world::visible_nodes(&self.tree, &self.layout, &camera, vw, vh, |id| {
            self.textures
                .as_ref()
                .is_some_and(|textures| textures.contains(id))
        });
        let mut out = Vec::with_capacity(items.len());
        let mut header_stack: Vec<(u8, f64)> = Vec::new();
        let mut panel_doc: Option<(String, f32, f32, f32, f32)> = None;
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
            let mut name = None;
            let mut body = Vec::new();
            let mut tex: Option<TexQuad> = None;
            match item.draw {
                Draw::Container(rung) => {
                    let stack_bottom = header_stack.last().map(|&(_, b)| b).unwrap_or(item.px.y);
                    if let Some(header) = container_header_layout(
                        rung,
                        item.px.y,
                        item.px.h,
                        stack_bottom,
                        camera.zoom,
                    ) {
                        name = Self::pinned_name(&item, rung == Rung::Label, header.pin_y);
                        body = container_body(
                            item.node,
                            rung,
                            &item.px,
                            item.label_w,
                            vh,
                            header.pin_y,
                            header.max_h,
                        );
                        if name.is_some() && !matches!(rung, Rung::Dot | Rung::Label) {
                            header_bg_h = container_header_bg_h(body.len(), header.max_h) as f32;
                            header_bg_y = header.pin_y as f32;
                            header_stack.push((item.level, header.pin_y + header_bg_h as f64));
                        }
                    }
                    if matches!(rung, Rung::Dot | Rung::Label | Rung::Card)
                        && !item.node.children.is_empty()
                    {
                        let area = item.label_w * item.full_h;
                        if let Some(textures) = self.textures.as_mut() {
                            if let Some(t) = textures.get(&item.node.id, area) {
                                if let Some(img) = &t.image {
                                    tex = Some(TexQuad {
                                        x: item.left as f32,
                                        y: item.top as f32,
                                        w: item.label_w as f32,
                                        h: item.full_h as f32,
                                        image: img.clone(),
                                    });
                                }
                            } else if textures.needs_dependency_pass(&item.node.id) {
                                let mut dependencies_ready = true;
                                for child in &item.node.children {
                                    let child_area = self
                                        .layout
                                        .rects
                                        .get(&child.id)
                                        .map(|rect| rect.w * rect.h * camera.zoom * camera.zoom)
                                        .unwrap_or(0.0);
                                    if textures.get(&child.id, area + child_area).is_none() {
                                        dependencies_ready = false;
                                    }
                                }
                                if !dependencies_ready {
                                    textures.defer_request_once(&item.node.id);
                                }
                            }
                        }
                    }
                }
                Draw::Leaf(tier) => {
                    let scale = item.full_h / content::natural_px(item.node);
                    let font = FONT_PX * scale;
                    body_font_px = (FONT_PX * scale) as f32;
                    if tier != LeafDraw::Dot && item.px.h >= 14.0 {
                        name = Self::pinned_name(&item, false, item.px.y);
                    }
                    let use_text =
                        font >= content::MIN_TEXT_FONT_PX && item.label_w >= world::CODE_MIN_W;
                    if use_text {
                        tex_opacity = 0.0;
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
                    } else {
                        let (tx, ty, tw, th) =
                            leaf_tex_rect(item.node, item.left, item.top, item.full_h);
                        if tw >= 1.0 && th >= 1.0 && ty < vh && ty + th > 0.0 {
                            if let Some(textures) = self.textures.as_mut() {
                                if let Some(t) = textures.get(&item.node.id, tw * th) {
                                    if let Some(img) = &t.image {
                                        tex = Some(TexQuad {
                                            x: tx as f32,
                                            y: ty as f32,
                                            w: tw as f32,
                                            h: th as f32,
                                            image: img.clone(),
                                        });
                                        body_opacity = 0.0;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let is_focused = item.node.id == focus_id;
            let is_hovered = self.hover_id.as_ref() == Some(&item.node.id);
            if item.node.doc.is_some() && (is_hovered || (is_focused && panel_doc.is_none())) {
                panel_doc = Some((
                    item.node.doc.clone().unwrap(),
                    item.px.x as f32,
                    item.px.y as f32,
                    item.px.w as f32,
                    item.px.h as f32,
                ));
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
                neighbor: !is_focused && neighbor_ids.iter().flatten().any(|n| *n == item.node.id),
                body_font_px,
                header_bg_h,
                header_bg_y,
                body_opacity,
                tex_opacity,
                name,
                body,
                tex,
            });
        }
        let doc_panel = panel_doc.and_then(|(doc, fx, fy, fw, _fh)| {
            let panel_w = fw.max(DOC_PANEL_W as f32);
            let wrapped = wrap_doc(&doc, (panel_w as f64) - 2.0 * BODY_PAD, FONT_PX);
            if wrapped.is_empty() {
                return None;
            }
            let row_count = wrapped.len() as f32;
            let panel_h = BODY_PAD as f32 + row_count * LINE_STEP as f32 + BODY_PAD as f32;
            let panel_y = fy - panel_h - 4.0;
            let mut rows = Vec::new();
            let mut y = panel_y + BODY_PAD as f32;
            for text in wrapped {
                let runs = vec![(text.len(), theme::DOC_COLOR)];
                rows.push(BodyText {
                    x: fx + BODY_PAD as f32,
                    y,
                    text,
                    runs,
                });
                y += LINE_STEP as f32;
            }
            Some(DocPanel {
                x: fx,
                y: panel_y,
                w: panel_w,
                h: panel_h,
                rows,
            })
        });
        self.bake_pending = if let Some(textures) = self.textures.as_mut() {
            if textures.has_queued() {
                let index = TreeIndex::new(&self.tree);
                let direct_child_bytes: HashMap<_, _> = textures
                    .next_request_ids()
                    .into_iter()
                    .filter_map(|id| {
                        let node = index.node(&id)?;
                        (!content::is_leaf_item(node))
                            .then(|| (id, textures.direct_child_bytes(node)))
                    })
                    .collect();
                let buffers = &mut self.buffers;
                let file_symbols = &self.file_symbols;
                let layout = &self.layout;
                textures.process_requests(|id, rasterizer| {
                    let node = index.node(id)?;
                    if !content::is_leaf_item(node) {
                        let rect = layout.rects.get(id)?;
                        let level = index.depth(id).unwrap_or(0) as u8;
                        let child_tex = |cid: &outrider_index::SymbolId| {
                            direct_child_bytes
                                .get(id)
                                .and_then(|children| children.get(cid))
                                .cloned()
                        };
                        return Some(rasterize::bake_container(
                            node, *rect, layout, level, &child_tex,
                        ));
                    }
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
                    if lines.is_empty() {
                        None
                    } else {
                        Some(rasterizer.bake(&lines))
                    }
                })
            } else {
                false
            }
        } else {
            false
        };
        (out, doc_panel)
    }

    /// Find a node in the tree by its ID (recursive depth-first search).
    /// Returns a reference to the node if found, None otherwise.
    fn find_node<'a>(root: &'a SymbolNode, id: &SymbolId) -> Option<&'a SymbolNode> {
        if root.id == *id {
            return Some(root);
        }
        root.children.iter().find_map(|c| Self::find_node(c, id))
    }

    /// Build the palette overlay div (absolutely positioned, centered horizontally).
    /// `map_w` is the map viewport width in logical pixels, used for centering.
    /// Includes an optional preview panel to the right showing metadata for the selected node.
    fn render_palette(&self, map_w: f64) -> gpui::Div {
        const PALETTE_W: f32 = 500.0;
        const PREVIEW_W: f32 = 300.0;
        const GAP: f32 = 4.0;

        let selected_node = self
            .palette
            .results
            .get(self.palette.selection)
            .and_then(|id| Self::find_node(&self.tree.root, id));

        let has_preview = selected_node
            .is_some_and(|n| n.signature.is_some() || n.doc.is_some() || n.churn_count > 0);

        let total_w = if has_preview {
            PALETTE_W + GAP + PREVIEW_W
        } else {
            PALETTE_W
        };
        let left_offset = ((map_w as f32 - total_w) / 2.0).max(0.0);

        let mode_label = match self.palette.mode {
            palette::PaletteMode::File => "File",
            palette::PaletteMode::Symbol => "Symbol",
        };

        let list_div = div()
            .w(px(PALETTE_W))
            .bg(rgb(theme::CODE_BG))
            .border_1()
            .border_color(rgb(theme::FOCUS_BORDER))
            .rounded(px(4.0))
            .overflow_hidden()
            .child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_size(px(14.0))
                    .font_family(theme::FONT_FAMILY)
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child(format!("[{mode_label}] {}│", self.palette.query)),
            )
            .children(self.palette.results.iter().enumerate().map(|(i, id)| {
                let name = self.palette.name_of(id);
                let path = &id.qualified_path;
                let selected = i == self.palette.selection;
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .text_size(px(13.0))
                    .font_family(theme::FONT_FAMILY)
                    .text_color(if selected {
                        rgb(theme::TEXT_PRIMARY)
                    } else {
                        rgb(theme::TEXT_SECONDARY)
                    })
                    .when(selected, |d| d.bg(rgb(0x2a2d32_u32)))
                    .child(format!("{name}  {path}"))
            }));

        let preview_div = selected_node.filter(|_| has_preview).map(|node| {
            let mut preview = div()
                .w(px(PREVIEW_W))
                .bg(rgb(theme::CODE_BG))
                .border_1()
                .border_color(rgb(theme::FOCUS_BORDER))
                .rounded(px(4.0))
                .overflow_hidden()
                .px(px(10.0))
                .py(px(8.0));

            // Kind label
            preview = preview.child(
                div()
                    .text_size(px(12.0))
                    .font_family(theme::FONT_FAMILY)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .pb(px(4.0))
                    .child(node.id.kind.label().to_uppercase()),
            );

            // Signature
            if let Some(sig) = &node.signature {
                preview = preview.child(
                    div()
                        .text_size(px(12.0))
                        .font_family(theme::FONT_FAMILY)
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .pb(px(6.0))
                        .child(sig.clone()),
                );
            }

            // Doc
            if let Some(doc) = &node.doc {
                preview = preview.child(
                    div()
                        .text_size(px(12.0))
                        .font_family(theme::FONT_FAMILY)
                        .text_color(rgb(theme::DOC_COLOR))
                        .pb(px(6.0))
                        .child(doc.clone()),
                );
            }

            // Stats line: lines + churn
            let mut stats = Vec::new();
            if node.measure > 0 {
                stats.push(format!("{} lines", node.measure));
            }
            if node.churn_count > 0 {
                stats.push(format!(
                    "{} commits (p{})",
                    node.churn_count,
                    (node.churn * 100.0).round() as u32
                ));
            }
            if !stats.is_empty() {
                preview = preview.child(
                    div()
                        .text_size(px(11.0))
                        .font_family(theme::FONT_FAMILY)
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(stats.join(" · ")),
                );
            }

            preview
        });

        div()
            .absolute()
            .top(px(60.0))
            .left(px(left_offset))
            .flex()
            .flex_row()
            .gap(px(GAP))
            .child(list_div)
            .children(preview_div)
    }

    fn render_file_menu(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .h_full()
            .ml(px(12.0))
            .gap(px(2.0))
            .child(
                div()
                    .id(ElementId::Name("menu-open-folder".into()))
                    .flex()
                    .items_center()
                    .h_full()
                    .px(px(8.0))
                    .cursor_pointer()
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .text_size(px(12.))
                    .hover(|s| {
                        s.text_color(rgb(theme::TEXT_PRIMARY))
                            .bg(rgb(chrome::MENU_HOVER))
                    })
                    .child("Open Folder")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        if let Some(folder) = rfd::FileDialog::new()
                            .set_title("Open Project Folder")
                            .pick_folder()
                        {
                            let (settings, warning) =
                                crate::settings::Settings::load().into_parts();
                            this.settings = settings;
                            if let Some(message) = warning {
                                this.notifications.push(Notification::warning(message));
                            }
                            this.start_loading(folder);
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(ElementId::Name("menu-clear-project-disk-cache".into()))
                    .flex()
                    .items_center()
                    .h_full()
                    .px(px(8.0))
                    .cursor_pointer()
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .text_size(px(12.))
                    .hover(|s| {
                        s.text_color(rgb(theme::TEXT_PRIMARY))
                            .bg(rgb(chrome::MENU_HOVER))
                    })
                    .child("Clear Project Disk Cache")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        if let Some(textures) = this.textures.as_mut() {
                            textures.request_clear_disk_cache();
                            this.bake_pending = true;
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(ElementId::Name("menu-settings".into()))
                    .flex()
                    .items_center()
                    .h_full()
                    .px(px(8.0))
                    .cursor_pointer()
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .text_size(px(12.))
                    .hover(|s| {
                        s.text_color(rgb(theme::TEXT_PRIMARY))
                            .bg(rgb(chrome::MENU_HOVER))
                    })
                    .child("Settings")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        if this.settings_draft.is_some() {
                            this.settings_draft = None;
                        } else {
                            this.settings_draft = Some(SettingsDraft::from_settings(
                                &this.settings,
                                &this.tree.repo_root,
                            ));
                            this.palette.close();
                            this.show_welcome = false;
                            this.context_menu = None;
                        }
                        cx.notify();
                    })),
            )
    }

    fn on_right_press(
        &mut self,
        e: &gpui::MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cam) = self.camera else { return };
        let (vw, vh) = Self::map_viewport(window);
        let items = world::visible_nodes(&self.tree, &self.layout, &cam, vw, vh, |id| {
            self.textures
                .as_ref()
                .is_some_and(|textures| textures.contains(id))
        });
        let (mx, my) = (
            f64::from(e.position.x),
            f64::from(e.position.y) - chrome::TITLEBAR_H,
        );
        if let Some(hit) = world::hit_test(&items, mx, my) {
            self.context_menu = Some(ContextMenu {
                position: e.position,
                target: hit.node.id.clone(),
            });
        } else {
            self.context_menu = None;
        }
        cx.notify();
    }

    fn on_left_release(&mut self, e: &gpui::MouseUpEvent, window: &Window, cx: &mut Context<Self>) {
        self.drag_last = None;
        if self.context_menu.is_some() {
            self.context_menu = None;
            cx.notify();
            return;
        }
        let Some(origin) = self.press_origin.take() else {
            return;
        };
        let slop = f64::from(e.position.x - origin.x)
            .abs()
            .max(f64::from(e.position.y - origin.y).abs());
        if slop > 4.0 {
            return;
        }
        let Some(cam) = self.camera else { return };
        let (vw, vh) = Self::map_viewport(window);
        let items = world::visible_nodes(&self.tree, &self.layout, &cam, vw, vh, |id| {
            self.textures
                .as_ref()
                .is_some_and(|textures| textures.contains(id))
        });
        let (mx, my) = (
            f64::from(e.position.x),
            f64::from(e.position.y) - chrome::TITLEBAR_H,
        );
        let hit = world::hit_test(&items, mx, my).map(|i| i.node.id.clone());
        drop(items);
        if let Some(id) = hit {
            let index = TreeIndex::new(&self.tree);
            if self.focus.set(id, &index) {
                self.nav_history.push(self.focus.current.clone());
            }
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, e: &gpui::MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        if e.pressed_button == Some(gpui::MouseButton::Left) {
            let Some(last) = self.drag_last else { return };
            self.cancel_tween();
            let dx = f64::from(e.position.x - last.x);
            let dy = f64::from(e.position.y - last.y);
            if let Some(cam) = self.camera.as_mut() {
                cam.pan(dx, dy);
            }
            self.drag_last = Some(e.position);
            cx.notify();
        } else {
            let Some(cam) = self.camera else { return };
            let (vw, vh) = Self::map_viewport(window);
            let items = world::visible_nodes(&self.tree, &self.layout, &cam, vw, vh, |id| {
                self.textures
                    .as_ref()
                    .is_some_and(|textures| textures.contains(id))
            });
            let (mx, my) = (
                f64::from(e.position.x),
                f64::from(e.position.y) - chrome::TITLEBAR_H,
            );
            let hit = world::hit_test(&items, mx, my)
                .filter(|i| i.node.doc.is_some())
                .map(|i| i.node.id.clone());
            if hit != self.hover_id {
                self.hover_id = hit;
                cx.notify();
            }
        }
    }

    fn on_scroll(&mut self, e: &gpui::ScrollWheelEvent, window: &Window, cx: &mut Context<Self>) {
        self.cancel_tween();
        let dy = match e.delta {
            gpui::ScrollDelta::Pixels(p) => f64::from(p.y),
            gpui::ScrollDelta::Lines(l) => l.y as f64 * 40.0,
        };
        let (vw, vh) = Self::map_viewport(window);
        let max_zoom = camera::MAX_ZOOM;
        let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
        if let Some(cam) = self.camera.as_mut() {
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
    }

    fn on_key_down(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.context_menu.is_some() && e.keystroke.key.as_str() == "escape" {
            self.context_menu = None;
            cx.notify();
            return;
        }
        if self.show_welcome {
            if e.keystroke.key.as_str() == "escape" {
                self.show_welcome = false;
                cx.notify();
            }
            return;
        }
        if e.keystroke.modifiers.control && !e.keystroke.modifiers.shift {
            match e.keystroke.key.as_str() {
                "p" => {
                    self.palette.open(palette::PaletteMode::File, &self.tree);
                    self.settings_draft = None;
                    self.context_menu = None;
                    cx.notify();
                    return;
                }
                "t" => {
                    self.palette.open(palette::PaletteMode::Symbol, &self.tree);
                    self.settings_draft = None;
                    self.context_menu = None;
                    cx.notify();
                    return;
                }
                "," => {
                    if self.settings_draft.is_some() {
                        self.settings_draft = None;
                    } else {
                        self.settings_draft = Some(SettingsDraft::from_settings(
                            &self.settings,
                            &self.tree.repo_root,
                        ));
                        self.palette.close();
                        self.show_welcome = false;
                        self.context_menu = None;
                    }
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }
        if let Some(draft) = &mut self.settings_draft {
            match e.keystroke.key.as_str() {
                "escape" => self.settings_draft = None,
                "tab" => {
                    draft.active = match draft.active {
                        SettingsField::Extensions => SettingsField::Folders,
                        SettingsField::Folders => SettingsField::CacheMb,
                        SettingsField::CacheMb => SettingsField::DiskCacheGb,
                        SettingsField::DiskCacheGb => SettingsField::Extensions,
                    };
                }
                "backspace" => {
                    draft.active_text_mut().pop();
                }
                _ => {
                    if let Some(ch) = e.keystroke.key_char.as_ref().and_then(|s| {
                        let mut chars = s.chars();
                        let c = chars.next()?;
                        if chars.next().is_none() {
                            Some(c)
                        } else {
                            None
                        }
                    }) {
                        if matches!(
                            draft.active,
                            SettingsField::CacheMb | SettingsField::DiskCacheGb
                        ) {
                            if ch.is_ascii_digit()
                                || (draft.active == SettingsField::DiskCacheGb && ch == '.')
                            {
                                draft.active_text_mut().push(ch);
                            }
                        } else {
                            draft.active_text_mut().push(ch);
                        }
                    }
                }
            }
            cx.notify();
            return;
        }
        if e.keystroke.modifiers.control
            && e.keystroke.modifiers.shift
            && e.keystroke.key.as_str() == "e"
        {
            let path = resolve_fs_path(&self.focus.current, &self.tree.repo_root);
            open_in_file_manager(&path);
            return;
        }
        if self.palette.is_open() {
            self.on_palette_key(e, window, cx);
            return;
        }
        self.on_nav_key(e, window, cx);
    }

    fn on_palette_key(&mut self, e: &gpui::KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "escape" => {
                self.palette.close();
                cx.notify();
            }
            "enter" => {
                if let Some(id) = self.palette.confirm() {
                    self.palette.close();
                    let index = TreeIndex::new(&self.tree);
                    if self.focus.set(id, &index) {
                        self.nav_history.push(self.focus.current.clone());
                    }
                    let (vw, vh) = Self::map_viewport(window);
                    let max_zoom = camera::MAX_ZOOM;
                    let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
                    if let Some(to) = self.frame_focus(&index, vw, vh, min_zoom, max_zoom) {
                        self.start_tween(to);
                    }
                }
                cx.notify();
            }
            "up" => {
                self.palette.move_selection(-1);
                cx.notify();
            }
            "down" => {
                self.palette.move_selection(1);
                cx.notify();
            }
            "backspace" => {
                self.palette.backspace(&self.tree);
                cx.notify();
            }
            _ => {
                if let Some(ch) = e.keystroke.key_char.as_ref().and_then(|s| {
                    let mut chars = s.chars();
                    let c = chars.next()?;
                    if chars.next().is_none() {
                        Some(c)
                    } else {
                        None
                    }
                }) {
                    self.palette.type_char(ch, &self.tree);
                    cx.notify();
                }
            }
        }
    }

    fn on_nav_key(&mut self, e: &gpui::KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        if self.camera.is_none() {
            return;
        }
        let (vw, vh) = Self::map_viewport(window);
        let max_zoom = camera::MAX_ZOOM;
        let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
        let index = TreeIndex::new(&self.tree);
        let target = match e.keystroke.key.as_str() {
            "enter" => {
                if !self.focus.step_in(&index) {
                    return;
                }
                self.nav_history.push(self.focus.current.clone());
                self.frame_focus(&index, vw, vh, min_zoom, max_zoom)
            }
            "escape" => {
                if !self.focus.step_out(&index) {
                    return;
                }
                self.nav_history.push(self.focus.current.clone());
                self.frame_focus(&index, vw, vh, min_zoom, max_zoom)
            }
            "end" => self
                .layout
                .rects
                .get(&self.focus.current)
                .copied()
                .map(|r| {
                    self.frame_below_headers(&index, r, vw, vh, |vh_eff| {
                        camera::frame_rect(r, vw, vh_eff, camera::END_FRACTION, min_zoom, max_zoom)
                    })
                }),
            "home" => {
                let c = Camera::fit(self.root_rect(), vw, vh);
                self.home_zoom = c.zoom;
                Some(c)
            }
            "left" if e.keystroke.modifiers.alt => {
                let Some(id) = self.nav_history.back().cloned() else {
                    return;
                };
                self.focus.current = id;
                self.focus.record_visit(&index);
                self.neighbors = None;
                self.frame_focus(&index, vw, vh, min_zoom, max_zoom)
            }
            "right" if e.keystroke.modifiers.alt => {
                let Some(id) = self.nav_history.forward().cloned() else {
                    return;
                };
                self.focus.current = id;
                self.focus.record_visit(&index);
                self.neighbors = None;
                self.frame_focus(&index, vw, vh, min_zoom, max_zoom)
            }
            "up" | "down" | "left" | "right" => {
                let dir = match e.keystroke.key.as_str() {
                    "up" => focus::Dir::Up,
                    "down" => focus::Dir::Down,
                    "left" => focus::Dir::Left,
                    _ => focus::Dir::Right,
                };
                let Some(next) =
                    focus::spatial_step(&self.focus.current, dir, &self.layout, &index)
                else {
                    return;
                };
                if !self.focus.set(next, &index) {
                    return;
                }
                self.frame_focus(&index, vw, vh, min_zoom, max_zoom)
            }
            _ => return,
        };
        if let Some(to) = target {
            self.start_tween(to);
            cx.notify();
        }
    }

    /// Re-run indexing in the background after settings change.
    fn reindex(&mut self) {
        let repo = self.tree.repo_root.clone();
        self.start_loading(repo);
    }

    /// Spawn a background thread to index `folder` and compute its layout.
    fn start_loading(&mut self, folder: std::path::PathBuf) {
        self.loader.start(folder, self.settings.clone());
        self.palette.close();
        self.show_welcome = false;
        self.settings_draft = None;
        self.context_menu = None;
        self.load_progress = None;
    }

    /// Check if background indexing completed; if so, apply the result.
    fn poll_loading(&mut self) -> bool {
        match self.loader.poll() {
            LoaderPoll::Idle => false,
            LoaderPoll::Loading(progress) => {
                let changed = self.load_progress.as_ref() != Some(&progress);
                self.load_progress = Some(progress);
                changed
            }
            LoaderPoll::Ready(result) => match *result {
                Ok(project) => {
                    self.install_project(project);
                    self.load_progress = None;
                    true
                }
                Err(error) => {
                    self.load_progress = None;
                    self.notifications.push(Notification::warning(format!(
                        "Could not open project: {error}"
                    )));
                    true
                }
            },
        }
    }

    fn install_project(&mut self, project: LoadResult) {
        let LoadResult {
            generation,
            project_root,
            tree,
            layout,
            warnings,
            source_fingerprints,
            disk_cache_bytes,
            project_namespace,
        } = project;
        debug_assert!(self.loader.accepts(generation));
        self.file_symbols = collect_file_symbols(&tree);
        self.buffers = BufferManager::new(project_root.clone());
        let root_id = tree.root.id.clone();
        self.focus = Focus::new(root_id.clone());
        self.nav_history = NavigationHistory::new(root_id, 64);
        self.neighbors = None;
        self.hover_id = None;
        self.camera = None;
        self.context_menu = None;
        self.palette = palette::Palette::new();
        self.tree = tree;
        self.layout = layout;
        self.textures = Some(TextureCache::new_prepared(
            project_namespace,
            source_fingerprints,
            self.settings.cache_mb as usize * 1024 * 1024,
            disk_cache_bytes,
        ));
        if !warnings.is_empty() {
            self.notifications
                .push(Notification::warning(warnings.join("; ")));
        }
    }

    /// Build the right-click context menu popup positioned at the click site.
    /// Returns None if no context menu is open.
    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let menu = self.context_menu.as_ref()?;
        let x = f32::from(menu.position.x);
        let y = f32::from(menu.position.y) - chrome::TITLEBAR_H as f32;
        let target = menu.target.clone();

        // Resolve display name and path for this target.
        let node_name = Self::find_node(&self.tree.root, &target)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| target.qualified_path.clone());
        let fs_path = resolve_fs_path(&target, &self.tree.repo_root);
        let rel_path = fs_path
            .strip_prefix(&self.tree.repo_root)
            .unwrap_or(&fs_path)
            .to_string_lossy()
            .into_owned();
        let copy_rel_str = rel_path.clone();
        let copy_abs_str = fs_path.to_string_lossy().into_owned();
        let copy_name_str = node_name.clone();
        let open_path = fs_path.clone();

        let menu_div =
            crate::overlays::context_menu_shell(x, y)
                .child(
                    crate::overlays::context_menu_row("ctx-open-fm", "Open File Location")
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            open_in_file_manager(&open_path);
                            this.context_menu = None;
                            cx.notify();
                        })),
                )
                .child(
                    crate::overlays::context_menu_row("ctx-copy-rel", "Copy Relative Path")
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                copy_rel_str.clone(),
                            ));
                            this.context_menu = None;
                            cx.notify();
                        })),
                )
                .child(
                    crate::overlays::context_menu_row("ctx-copy-abs", "Copy Absolute Path")
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                copy_abs_str.clone(),
                            ));
                            this.context_menu = None;
                            cx.notify();
                        })),
                )
                .child(
                    crate::overlays::context_menu_row("ctx-copy-name", "Copy Name").on_click(
                        cx.listener(move |this, _e, _w, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                copy_name_str.clone(),
                            ));
                            this.context_menu = None;
                            cx.notify();
                        }),
                    ),
                );

        Some(menu_div)
    }

    /// Build the settings overlay div (absolutely positioned, centered).
    /// Shows current filter settings read-only with action buttons.
    fn render_settings_window(&self, map_w: f64, cx: &mut Context<Self>) -> gpui::Div {
        let draft = self.settings_draft.as_ref().unwrap();
        let field = |kind: SettingsField, text: String| {
            let (id, label) = match kind {
                SettingsField::Extensions => {
                    ("field-extensions", "Filtered Extensions (comma-separated):")
                }
                SettingsField::Folders => ("field-folders", "Filtered Folders (comma-separated):"),
                SettingsField::CacheMb => ("field-cache-mb", "Texture Cache (MB):"),
                SettingsField::DiskCacheGb => ("field-disk-cache-gb", "Project Disk Cache (GiB):"),
            };
            let active = draft.active == kind;
            let text = if active { format!("{text}|") } else { text };
            crate::overlays::labeled_field(
                label,
                crate::overlays::settings_input(id, text, active).on_click(cx.listener(
                    move |this, _event, _window, cx| {
                        if let Some(draft) = &mut this.settings_draft {
                            draft.active = kind;
                        }
                        cx.notify();
                    },
                )),
            )
        };
        let fields = vec![
            field(SettingsField::Extensions, draft.filter_extensions.clone()),
            field(SettingsField::Folders, draft.filter_folders.clone()),
            field(SettingsField::CacheMb, draft.cache_mb.clone()),
            field(SettingsField::DiskCacheGb, draft.disk_cache_gb.clone()),
        ];
        let validation = draft.notification.clone();
        let save = crate::overlays::action_button("settings-save", "Save & Close", true).on_click(
            cx.listener(|this, _event, _window, cx| {
                if let Some(mut draft) = this.settings_draft.take() {
                    let mut candidate = this.settings.clone();
                    let result = draft
                        .apply_to(&mut candidate, &this.tree.repo_root)
                        .and_then(|()| candidate.save());
                    match result {
                        Ok(()) => {
                            this.settings = candidate;
                            this.reindex();
                        }
                        Err(message) => {
                            draft.notification = Some(message);
                            this.settings_draft = Some(draft);
                        }
                    }
                }
                cx.notify();
            }),
        );
        let reset = crate::overlays::action_button("settings-reset", "Reset to Defaults", false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                let defaults = settings::Settings::default();
                match defaults.save() {
                    Ok(()) => {
                        this.settings = defaults;
                        this.settings_draft = None;
                        this.reindex();
                    }
                    Err(message) => {
                        if let Some(draft) = &mut this.settings_draft {
                            draft.notification = Some(message);
                        }
                    }
                }
                cx.notify();
            }));
        let cancel = crate::overlays::action_button("settings-cancel", "Cancel", false).on_click(
            cx.listener(|this, _event, _window, cx| {
                this.settings_draft = None;
                cx.notify();
            }),
        );
        crate::overlays::settings_element(map_w, fields, validation, vec![save, reset, cancel])
    }
    /// Build the welcome screen overlay div (absolutely positioned, centered).
    /// `map_w` is the map viewport width in logical pixels.
    fn render_welcome(&self, map_w: f64, cx: &mut Context<Self>) -> gpui::Div {
        let got_it = crate::overlays::action_button("welcome-got-it", "Got it", true).on_click(
            cx.listener(|this, _event, _window, cx| {
                this.show_welcome = false;
                cx.notify();
            }),
        );
        let no_show = crate::overlays::action_button("welcome-no-show", "Don't show again", false)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.show_welcome = false;
                this.settings.show_welcome = false;
                if let Err(message) = this.settings.save() {
                    this.notifications.push(Notification::warning(message));
                }
                cx.notify();
            }));
        crate::overlays::welcome_element(map_w, vec![got_it, no_show])
    }
}

/// GPUI render entry point: wires input handlers onto the map canvas and
/// composes the titlebar + canvas into the window element tree.
impl Render for TreemapView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window, cx);
        }

        if self.poll_loading() {
            cx.notify();
        }

        let (vw, vh) = Self::map_viewport(window);
        let is_loading = self.loader.is_loading();

        let (items, doc_panel) = self.paint_items(vw, vh);

        // Disk results are applied while building paint items. Drain their
        // diagnostics afterwards so a terminal worker failure is visible in
        // this frame even when no further animation frame is needed.
        if let Some(textures) = self.textures.as_mut() {
            for diagnostic in textures.drain_disk_diagnostics() {
                self.notifications
                    .push(Notification::warning(diagnostic.to_string()));
            }
        }

        if let Some(textures) = self.textures.as_mut() {
            for img in textures.take_retired() {
                let _ = window.drop_image(img);
            }
        }
        if self.tween.is_some() || self.bake_pending || is_loading {
            window.request_animation_frame();
        }

        // Build the palette overlay before the map div (while &self is free).
        let palette_overlay = self.palette.is_open().then(|| self.render_palette(vw));

        // Build the settings overlay (needs cx for click listeners).
        let settings_overlay = self
            .settings_draft
            .is_some()
            .then(|| self.render_settings_window(vw, cx));

        // Build the welcome overlay (needs cx for click listeners).
        let welcome_overlay = self.show_welcome.then(|| self.render_welcome(vw, cx));

        // Build the context menu overlay (needs cx for click listeners).
        let context_menu_overlay = self.render_context_menu(cx);

        // Build the loading overlay if indexing in background.
        let loading_overlay = self
            .load_progress
            .as_ref()
            .filter(|_| is_loading)
            .map(|progress| crate::overlays::loading_element(progress, vw));
        let notification_overlay = self.notifications.visible().map(|notification| {
            crate::overlays::notification_element(notification).on_click(cx.listener(
                |this, _event, _window, cx| {
                    this.apply_action(InteractionAction::DismissNotification);
                    cx.notify();
                },
            ))
        });

        let title = self.window_title();
        let file_menu = self.render_file_menu(cx);
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
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(|this, e, w, cx| this.on_right_press(e, w, cx)),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, e, w, cx| this.on_left_release(e, w, cx)),
            )
            .on_mouse_move(cx.listener(|this, e, w, cx| this.on_mouse_move(e, w, cx)))
            .on_scroll_wheel(cx.listener(|this, e, w, cx| this.on_scroll(e, w, cx)))
            .on_key_down(cx.listener(|this, e, w, cx| this.on_key_down(e, w, cx)))
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
                                let runs: Vec<TextRun> = bt
                                    .runs
                                    .iter()
                                    .map(|&(len, color)| {
                                        let mut r = run(len, color);
                                        if item.body_opacity < 1.0 {
                                            r.color = r.color.opacity(item.body_opacity);
                                        }
                                        r
                                    })
                                    .collect();
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
                                let runs: Vec<TextRun> = bt
                                    .runs
                                    .iter()
                                    .map(|&(len, color)| run(len, color))
                                    .collect();
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
            )
            .children(palette_overlay)
            .children(settings_overlay)
            .children(welcome_overlay)
            .children(context_menu_overlay)
            .children(loading_overlay)
            .children(notification_overlay);

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::BG))
            .child(chrome::titlebar(
                title,
                file_menu,
                self.memory_status(),
                window,
            ))
            .child(map)
            .children(chrome::resize_rim(window))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_gibibytes, SettingsDraft};

    #[test]
    fn cache_status_reports_memory_and_current_project_disk_usage() {
        assert_eq!(
            super::format_cache_status(
                3 * 1024 * 1024,
                128,
                crate::rasterize::DiskState::Ready {
                    used_bytes: 5 * 1024 * 1024,
                },
            ),
            "Memory 3 / 128 MB · Disk 5 MB"
        );
        assert_eq!(
            super::format_cache_status(0, 128, crate::rasterize::DiskState::Preparing),
            "Memory 0 / 128 MB · Disk preparing…"
        );
        assert_eq!(
            super::format_cache_status(0, 128, crate::rasterize::DiskState::Unavailable),
            "Memory 0 / 128 MB · Disk unavailable"
        );
    }

    #[test]
    fn loading_shell_texture_cache_is_lazy() {
        assert!(super::loading_texture_cache().is_none());
    }

    #[test]
    fn decimal_gibibytes_are_converted_without_overflow() {
        assert_eq!(parse_gibibytes("1").unwrap(), 1_073_741_824);
        assert_eq!(parse_gibibytes("1.5").unwrap(), 1_610_612_736);
        assert!(parse_gibibytes("0.999999999999999999").is_ok());
        assert!(parse_gibibytes("18446744073709551616").is_err());
        assert!(parse_gibibytes("0").is_err());
        assert!(parse_gibibytes("1.2.3").is_err());
    }

    #[test]
    fn settings_draft_rejects_overflow_without_mutating_settings() {
        let project = std::path::Path::new("D:/repo");
        let mut settings = crate::settings::Settings::default();
        let mut draft = SettingsDraft::from_settings(&settings, project);
        draft.cache_mb = "4294967296".into();

        assert!(draft.apply_to(&mut settings, project).is_err());
        assert_eq!(settings.cache_mb, 256);
        assert_eq!(
            settings.disk_cache_bytes(project),
            crate::settings::DEFAULT_DISK_CACHE_BYTES
        );
    }

    #[test]
    fn settings_draft_updates_only_the_current_project_disk_limit() {
        let one = std::path::Path::new("D:/one");
        let two = std::path::Path::new("D:/two");
        let mut settings = crate::settings::Settings::default();
        settings.set_disk_cache_bytes(two, 2 * crate::settings::DEFAULT_DISK_CACHE_BYTES);
        let mut draft = SettingsDraft::from_settings(&settings, one);
        draft.disk_cache_gb = "0.5".into();

        draft.apply_to(&mut settings, one).unwrap();

        assert_eq!(
            settings.disk_cache_bytes(one),
            crate::settings::DEFAULT_DISK_CACHE_BYTES / 2
        );
        assert_eq!(
            settings.disk_cache_bytes(two),
            2 * crate::settings::DEFAULT_DISK_CACHE_BYTES
        );
    }

    #[test]
    fn unchanged_disk_text_round_trips_exact_stored_bytes() {
        let project = std::path::Path::new("D:/exact");
        for bytes in [1, crate::settings::DEFAULT_DISK_CACHE_BYTES, u64::MAX] {
            let mut settings = crate::settings::Settings::default();
            settings.set_disk_cache_bytes(project, bytes);
            let draft = SettingsDraft::from_settings(&settings, project);

            draft.apply_to(&mut settings, project).unwrap();

            assert_eq!(settings.disk_cache_bytes(project), bytes);
        }
    }

    #[test]
    fn saving_settings_still_disables_the_welcome_screen() {
        let project = std::path::Path::new("D:/repo");
        let mut settings = crate::settings::Settings::default();
        assert!(settings.show_welcome);
        let draft = SettingsDraft::from_settings(&settings, project);

        draft.apply_to(&mut settings, project).unwrap();

        assert!(!settings.show_welcome);
    }

    use std::collections::BTreeMap;

    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    use super::{
        code_line, container_body, container_header_bg_h, container_header_layout,
        container_header_px, leaf_tex_rect, leaf_text_body, runs_from_spans, truncate_to_width,
        wrap_doc, HEADER, LINE_STEP,
    };
    use crate::buffers::BufferManager;
    use crate::world::{self, PxRect, Rung};

    #[test]
    fn truncation() {
        // 12 + 10*0.62*12 = wide enough for exactly 10 chars at 12px
        let w = 12.0 + 10.0 * 0.62 * 12.0;
        assert_eq!(
            truncate_to_width("short.rs", w, 12.0),
            Some("short.rs".into())
        );
        assert_eq!(
            truncate_to_width("a_very_long_file_name.rs", w, 12.0),
            Some("a_very_lo…".into())
        );
        assert_eq!(truncate_to_width("anything", 10.0, 12.0), None);
        // multi-byte chars must not panic
        assert_eq!(
            truncate_to_width("ééééééééééééé", w, 12.0),
            Some("ééééééééé…".into())
        );
    }

    fn node(
        kind: SymbolKind,
        qual: &str,
        byte_range: Option<std::ops::Range<usize>>,
        measure: u64,
        signature: Option<&str>,
        doc: Option<&str>,
    ) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind,
                qualified_path: qual.into(),
                ordinal: 0,
            },
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
            HighlightSpan {
                range: 0..2,
                kind: HighlightKind::Keyword,
            },
            HighlightSpan {
                range: 3..7,
                kind: HighlightKind::Function,
            },
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
        let f = node(
            SymbolKind::File,
            "a.rs",
            Some(0..24),
            2,
            None,
            Some("Doc line."),
        );
        let px = PxRect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 300.0,
        };
        let body = container_body(&f, Rung::Detail, &px, 400.0, 600.0, px.y, 300.0);
        // churn readout only (doc shown via hover panel, no children → no kinds)
        assert_eq!(body.len(), 1);
        assert!((f64::from(body[0].y) - HEADER).abs() < 1e-3);
    }

    #[test]
    fn leaf_text_body_paints_code_without_duplicate_signature() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let leaf = node(
            SymbolKind::Item { label: "fn".into() },
            "a.rs::two",
            Some(12..23),
            1,
            Some("fn two()"),
            None,
        );
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 12)]);
        let natural = crate::content::natural_px(&leaf);
        // scale 1.0: full_h == natural
        let body = leaf_text_body(
            &leaf,
            0.0,
            0.0,
            natural,
            480.0,
            600.0,
            &mut mgr,
            &file_symbols,
        );
        // code only — no separate signature row (the code line IS the signature)
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].text, "fn two() {}");
        assert!(body[0].runs.len() > 1, "code rows carry colored runs");
        assert_eq!(
            body[0].runs.iter().map(|r| r.0).sum::<usize>(),
            body[0].text.len()
        );
        // code row 0 at natural-y HEADER
        assert!((f64::from(body[0].y) - HEADER).abs() < 1e-3);
    }

    #[test]
    fn leaf_text_body_scales_uniformly_past_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let leaf = node(
            SymbolKind::Item { label: "fn".into() },
            "a.rs::two",
            Some(12..23),
            1,
            Some("fn two()"),
            None,
        );
        let mut mgr = BufferManager::new(dir.path().to_path_buf());
        let mut file_symbols = BTreeMap::new();
        file_symbols.insert("a.rs".to_string(), vec![(leaf.id.clone(), 12)]);
        let natural = crate::content::natural_px(&leaf);
        // zoom 2× (full_h = 2·natural): code row y doubles, still no clip
        let body = leaf_text_body(
            &leaf,
            0.0,
            0.0,
            2.0 * natural,
            960.0,
            100_000.0,
            &mut mgr,
            &file_symbols,
        );
        assert_eq!(body.len(), 1);
        assert!((f64::from(body[0].y) - 2.0 * HEADER).abs() < 1e-3);
        // buffer unavailable → signature only, no code
        let mut broken = BufferManager::new(std::path::PathBuf::from("/nonexistent"));
        let body = leaf_text_body(
            &leaf,
            0.0,
            0.0,
            natural,
            480.0,
            600.0,
            &mut broken,
            &BTreeMap::new(),
        );
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].text, "fn two()");
    }

    fn make_node(kind: SymbolKind, name: &str) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind,
                qualified_path: name.into(),
                ordinal: 0,
            },
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
            assert_eq!(
                classify_tint(&n),
                BoxTint::DocsFolder,
                "expected DocsFolder for {name}"
            );
        }
    }

    #[test]
    fn classify_tint_test_folder() {
        use super::classify_tint;
        use crate::theme::BoxTint;
        for name in &["test", "tests", "spec", "specs", "__tests__"] {
            let n = make_node(SymbolKind::Folder, name);
            assert_eq!(
                classify_tint(&n),
                BoxTint::TestFolder,
                "expected TestFolder for {name}"
            );
        }
    }

    #[test]
    fn classify_tint_typedef_items() {
        use super::classify_tint;
        use crate::theme::BoxTint;
        for label in &[
            "struct",
            "enum",
            "trait",
            "class",
            "interface",
            "type",
            "typedef",
        ] {
            let n = make_node(
                SymbolKind::Item {
                    label: label.to_string(),
                },
                "Foo",
            );
            assert_eq!(
                classify_tint(&n),
                BoxTint::TypeDef,
                "expected TypeDef for {label}"
            );
        }
    }

    #[test]
    fn classify_tint_normal_cases() {
        use super::classify_tint;
        use crate::theme::BoxTint;
        // Unrecognized folder name
        assert_eq!(
            classify_tint(&make_node(SymbolKind::Folder, "src")),
            BoxTint::Normal
        );
        // Non-typedef item label
        assert_eq!(
            classify_tint(&make_node(SymbolKind::Item { label: "fn".into() }, "foo")),
            BoxTint::Normal
        );
        // File and Chunk always Normal
        assert_eq!(
            classify_tint(&make_node(SymbolKind::File, "main.rs")),
            BoxTint::Normal
        );
        assert_eq!(
            classify_tint(&make_node(SymbolKind::Chunk, "chunk")),
            BoxTint::Normal
        );
    }

    #[test]
    fn leaf_tex_rect_covers_the_line_area() {
        // 10-line leaf drawn at half its natural height.
        let leaf = node(
            SymbolKind::Item { label: "fn".into() },
            "a.rs::f",
            Some(0..100),
            10,
            Some("fn f()"),
            None,
        );
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

    #[test]
    fn container_header_never_collapses_below_one_line() {
        let natural = HEADER + 2.0 * LINE_STEP;
        assert!((container_header_px(1.0) - natural).abs() < 1e-9);
        assert!((container_header_px(0.5) - natural * 0.5).abs() < 1e-9);
        assert!((container_header_px(0.1) - HEADER).abs() < 1e-9);

        // Camera zoom is positive. Check the adjacent representable values
        // around the exact clamp transition rather than implying zero or
        // negative zoom support.
        let transition = HEADER / natural;
        let below = f64::from_bits(transition.to_bits() - 1);
        let above = f64::from_bits(transition.to_bits() + 1);
        assert!((container_header_px(below) - HEADER).abs() < 1e-9);
        assert!((container_header_px(transition) - HEADER).abs() < 1e-9);
        assert!(container_header_px(above) > HEADER);
    }

    #[test]
    fn container_header_layout_respects_trailing_edge_and_ancestor_stack() {
        for rung in [Rung::Card, Rung::Detail, Rung::Full] {
            assert!(container_header_layout(rung, 597.0, 3.0, 597.0, 1.0).is_none());
            assert!(container_header_layout(rung, 600.0, 2.0, 600.0, 1.0).is_none());

            let exact = container_header_layout(rung, 100.0, HEADER, 100.0, 1.0).unwrap();
            assert!((exact.pin_y - 100.0).abs() < 1e-9);
            assert!((exact.max_h - HEADER).abs() < 1e-9);

            let available = HEADER + 5.0;
            let capped = container_header_layout(rung, 100.0, available, 100.0, 1.0).unwrap();
            assert!((capped.max_h - available).abs() < 1e-9);
            assert!(capped.pin_y + capped.max_h <= 100.0 + available);

            let stacked =
                container_header_layout(rung, 100.0, 2.0 * HEADER, 100.0 + HEADER - 2.0, 1.0)
                    .unwrap();
            assert!((stacked.max_h - (HEADER + 2.0)).abs() < 1e-9);
            assert!(
                container_header_layout(rung, 100.0, 2.0 * HEADER, 100.0 + HEADER + 1.0, 1.0,)
                    .is_none()
            );
        }
    }

    #[test]
    fn container_header_layout_preserves_label_and_dot_policy() {
        assert!(container_header_layout(Rung::Dot, 0.0, 100.0, 0.0, 1.0).is_none());
        assert!(container_header_layout(Rung::Label, 0.0, 13.99, 0.0, 1.0).is_none());
        assert!(container_header_layout(Rung::Label, 0.0, 14.0, 0.0, 1.0).is_some());
    }

    #[test]
    fn named_container_header_background_keeps_one_line_without_body_rows() {
        let h = container_header_bg_h(0, container_header_px(0.1));
        assert!((h - HEADER).abs() < 1e-9);
    }

    fn screen_y(cam: &Camera, wy: f64, vh: f64) -> f64 {
        (wy - cam.center_y) * cam.zoom + vh / 2.0
    }

    #[test]
    fn inset_top_centers_rect_in_the_band_below_the_inset() {
        let r = Rect {
            x: 0.0,
            y: 7.0,
            w: 100.0,
            h: 20.0,
        };
        let cam = Camera {
            center_x: 0.0,
            center_y: 0.0,
            zoom: 2.0,
        };
        // band [20, 100], rect 20·2 = 40 tall → top at 20 + (80 − 40)/2 = 40
        let c = inset_top(cam, r, 20.0, 100.0);
        assert!((screen_y(&c, r.y, 100.0) - 40.0).abs() < 1e-9);
        assert_eq!(c.zoom, cam.zoom); // vertical shift only
    }

    #[test]
    fn inset_top_pins_to_band_top_when_rect_is_taller_than_the_band() {
        let r = Rect {
            x: 0.0,
            y: 7.0,
            w: 100.0,
            h: 90.0,
        };
        let cam = Camera {
            center_x: 0.0,
            center_y: 0.0,
            zoom: 1.0,
        };
        let c = inset_top(cam, r, 20.0, 100.0);
        assert!((screen_y(&c, r.y, 100.0) - 20.0).abs() < 1e-9);
    }

    fn named(kind: SymbolKind, qual: &str, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            name: name.into(),
            children,
            ..node(kind, qual, None, 1, None, None)
        }
    }

    /// root { mid { anon(unnamed) { f } } } with rects far above the viewport.
    fn stack_fixture() -> (SymbolTree, PackLayout, SymbolId) {
        let leaf = named(
            SymbolKind::Item { label: "fn".into() },
            "r/m/a/f",
            "f",
            vec![],
        );
        let focus = leaf.id.clone();
        let anon = named(SymbolKind::Folder, "r/m/a", "", vec![leaf]);
        let anon_id = anon.id.clone();
        let mid = named(SymbolKind::Folder, "r/m", "mid", vec![anon]);
        let mid_id = mid.id.clone();
        let root = named(SymbolKind::Folder, "r", "root", vec![mid]);
        let mut rects = BTreeMap::new();
        rects.insert(
            root.id.clone(),
            Rect {
                x: 0.0,
                y: -1000.0,
                w: 4000.0,
                h: 4000.0,
            },
        );
        rects.insert(
            mid_id,
            Rect {
                x: 10.0,
                y: -900.0,
                w: 3000.0,
                h: 3000.0,
            },
        );
        rects.insert(
            anon_id,
            Rect {
                x: 20.0,
                y: -800.0,
                w: 2000.0,
                h: 2000.0,
            },
        );
        rects.insert(
            focus.clone(),
            Rect {
                x: 30.0,
                y: 0.0,
                w: 480.0,
                h: 200.0,
            },
        );
        let tree = SymbolTree {
            root,
            repo_root: std::path::PathBuf::from("/x"),
        };
        (tree, PackLayout { rects }, focus)
    }

    #[test]
    fn pinned_stack_h_stacks_named_offscreen_ancestors_and_skips_unnamed() {
        let (tree, layout, focus) = stack_fixture();
        let index = TreeIndex::new(&tree);
        // Both named ancestors' tops are above the viewport → each pins at
        // the top and stacks; the unnamed folder contributes nothing.
        let cam = Camera {
            center_x: 0.0,
            center_y: 0.0,
            zoom: 1.0,
        };
        let h = pinned_stack_h(&focus, &layout, &index, &cam, 800.0, 600.0);
        let hdr = HEADER + 2.0 * LINE_STEP;
        assert!((h - 2.0 * hdr).abs() < 1e-9);
        // Header height scales with zoom below 1.
        let cam = Camera {
            center_x: 0.0,
            center_y: 0.0,
            zoom: 0.5,
        };
        let h = pinned_stack_h(&focus, &layout, &index, &cam, 800.0, 600.0);
        assert!((h - hdr).abs() < 1e-9);
        let cam = Camera {
            center_x: 0.0,
            center_y: 3000.0,
            zoom: 0.1,
        };
        let h = pinned_stack_h(&focus, &layout, &index, &cam, 800.0, 600.0);
        assert!((h - 2.0 * HEADER).abs() < 1e-9);
    }

    #[test]
    fn pinned_stack_h_pins_on_screen_ancestors_at_their_own_top() {
        let (tree, layout, focus) = stack_fixture();
        let index = TreeIndex::new(&tree);
        // root top on screen at 50, mid top at 150 (clear of root's header)
        // → stack bottom is mid's top plus one header.
        let cam = Camera {
            center_x: 0.0,
            center_y: -750.0,
            zoom: 1.0,
        };
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
        assert_eq!(
            wrap_doc("hello world", wrap_w(11), 12.0),
            vec!["hello world"]
        );
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
    fn resolve_fs_path_file_node() {
        let root = std::path::Path::new("/home/user/project");
        let id = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/main.rs".into(),
            ordinal: 0,
        };
        let path = super::resolve_fs_path(&id, root);
        assert_eq!(
            path,
            std::path::PathBuf::from("/home/user/project/src/main.rs")
        );
    }

    #[test]
    fn resolve_fs_path_item_node() {
        let root = std::path::Path::new("/home/user/project");
        let id = SymbolId {
            kind: SymbolKind::Item { label: "fn".into() },
            qualified_path: "src/lib.rs::Point::norm".into(),
            ordinal: 0,
        };
        let path = super::resolve_fs_path(&id, root);
        assert_eq!(
            path,
            std::path::PathBuf::from("/home/user/project/src/lib.rs")
        );
    }

    #[test]
    fn resolve_fs_path_chunk_node() {
        let root = std::path::Path::new("/repo");
        let id = SymbolId {
            kind: SymbolKind::Chunk,
            qualified_path: "BIG.md#2".into(),
            ordinal: 0,
        };
        let path = super::resolve_fs_path(&id, root);
        assert_eq!(path, std::path::PathBuf::from("/repo/BIG.md"));
    }

    #[test]
    fn resolve_fs_path_folder_node() {
        let root = std::path::Path::new("/repo");
        let id = SymbolId {
            kind: SymbolKind::Folder,
            qualified_path: "src/utils".into(),
            ordinal: 0,
        };
        let path = super::resolve_fs_path(&id, root);
        assert_eq!(path, std::path::PathBuf::from("/repo/src/utils"));
    }
}
