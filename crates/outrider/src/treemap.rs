//! Main GPUI view for the outrider treemap — drives the render loop, handles
//! all input (mouse drag/zoom/click, keyboard navigation), and translates the
//! world-space layout from `outrider-layout` into per-frame paint instructions
//! (quads, text runs, and baked texture quads) via a static canvas closure.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, rgba, size, transparent_black, App, BorderStyle,
    Bounds, ContentMask, Context, Corners, FocusHandle, Pixels, TextAlign, TextRun,
    Window,
};
use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::{PackLayout, Rect};

use gpui::actions;

actions!(
    outrider,
    [
        OpenFolder,
        ClearDiskCache,
        ToggleSettings,
        ToggleProjectSettings,
        OpenFilePalette,
        OpenSymbolPalette,
        RevealInFileManager,
        Quit,
    ]
);

use outrider_index::call_graph::{CallEdge, CallGraphData};

use crate::buffers::{collect_file_symbols, BufferManager};
use crate::camera::{self, Camera, CameraTween};
use crate::content::{self, FONT_PX, HEADER, LINE_STEP};
use crate::focus::{self, Focus, TreeIndex};
use crate::interaction::InteractionAction;
use crate::navigation::NavigationHistory;
use crate::overlays::{ContextMenu, Notification, Notifications};
use crate::paint_model::{
    code_line, runs_from_spans, truncate_to_width, wrap_code_line, wrap_doc, BodyText, DocPanel,
    NameRow, PaintItem, TexQuad,
};
use crate::palette;
use crate::project_loader::{LoadProgress, LoadResult, LoaderPoll, PreScanPoll, PreScanner, ProjectLoader};
use crate::project_settings::{self, ExtensionCategory, ProjectSettings};
use crate::rasterize::{self, TextureCache};
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
    NodePadding,
}

struct SettingsDraft {
    filter_extensions: String,
    filter_folders: String,
    cache_mb: String,
    disk_cache_gb: String,
    node_padding: String,
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
            node_padding: s.node_padding.to_string(),
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
            SettingsField::NodePadding => &mut self.node_padding,
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
        let node_padding = self
            .node_padding
            .trim()
            .parse::<f64>()
            .map_err(|_| "Node padding must be a number".to_string())?;
        if !(0.0..=64.0).contains(&node_padding) {
            return Err("Node padding must be between 0 and 64".to_string());
        }
        settings.filter_extensions = parse_list(&self.filter_extensions);
        settings.filter_folders = parse_list(&self.filter_folders);
        settings.show_welcome = false;
        settings.cache_mb = cache_mb;
        settings.node_padding = node_padding;
        settings.set_disk_cache_bytes(project, disk_cache_bytes);
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SetupPanel {
    Extensions,
    Folders,
}

struct ProjectSetupDraft {
    pre_scan: outrider_index::scan::PreScanResult,
    extension_enabled: BTreeMap<String, bool>,
    folder_enabled: BTreeMap<String, bool>,
    folder_expanded: BTreeMap<String, bool>,
    gitignored_set: std::collections::HashSet<String>,
    category_expanded: BTreeMap<ExtensionCategory, bool>,
    filtered_files: usize,
    filtered_bytes: u64,
    active_panel: SetupPanel,
    ext_cursor: usize,
    folder_cursor: usize,
}

impl ProjectSetupDraft {
    fn from_pre_scan(
        pre_scan: outrider_index::scan::PreScanResult,
        existing: Option<&ProjectSettings>,
    ) -> Self {
        let mut extension_enabled = BTreeMap::new();
        for ext in pre_scan.extensions.keys() {
            let enabled = if let Some(ps) = existing {
                !ps.filter_extensions.iter().any(|f| f.eq_ignore_ascii_case(ext))
            } else {
                project_settings::categorize_extension(ext).default_enabled()
            };
            extension_enabled.insert(ext.clone(), enabled);
        }

        let default_excluded: &[&str] = &[
            "target", "node_modules", "dist", "build", "__pycache__",
            ".next", ".nuxt", "out", "pkg", "vendor",
        ];

        let mut folder_enabled = BTreeMap::new();
        for folder in pre_scan.folders.keys() {
            let enabled = if let Some(ps) = existing {
                !ps.filter_folders.iter().any(|f| f == folder)
            } else {
                let first = folder.split('/').next().unwrap_or(folder);
                !default_excluded.iter().any(|d| d.eq_ignore_ascii_case(first))
            };
            folder_enabled.insert(folder.clone(), enabled);
        }

        let mut gitignored_set = std::collections::HashSet::new();
        for name in &pre_scan.gitignored_folders {
            folder_enabled.insert(name.clone(), false);
            gitignored_set.insert(name.clone());
        }

        let mut draft = Self {
            pre_scan,
            extension_enabled,
            folder_enabled,
            folder_expanded: BTreeMap::new(),
            gitignored_set,
            category_expanded: BTreeMap::new(),
            filtered_files: 0,
            filtered_bytes: 0,
            active_panel: SetupPanel::Extensions,
            ext_cursor: 0,
            folder_cursor: 0,
        };
        draft.recompute_stats();
        draft
    }

    fn recompute_stats(&mut self) {
        let mut files = 0usize;
        let mut bytes = 0u64;
        for (ext, stats) in &self.pre_scan.extensions {
            if self.extension_enabled.get(ext).copied().unwrap_or(true) {
                files += stats.count;
                bytes += stats.bytes;
            }
        }
        // Collect disabled folder paths, then only subtract top-level
        // (non-nested) disabled paths so child counts aren't double-subtracted.
        let disabled: Vec<&String> = self
            .folder_enabled
            .iter()
            .filter(|(_, &v)| !v)
            .map(|(k, _)| k)
            .collect();
        for folder in &disabled {
            let is_child_of_disabled = disabled.iter().any(|parent| {
                *parent != *folder && folder.starts_with(parent.as_str()) && folder.as_bytes().get(parent.len()) == Some(&b'/')
            });
            if is_child_of_disabled {
                continue;
            }
            if let Some(stats) = self.pre_scan.folders.get(*folder) {
                files = files.saturating_sub(stats.count);
                bytes = bytes.saturating_sub(stats.bytes);
            }
        }
        self.filtered_files = files;
        self.filtered_bytes = bytes;
    }

    fn to_project_settings(&self) -> ProjectSettings {
        let filter_extensions: Vec<String> = self
            .extension_enabled
            .iter()
            .filter(|(_, &v)| !v)
            .map(|(k, _)| k.clone())
            .collect();
        let filter_folders: Vec<String> = self
            .folder_enabled
            .iter()
            .filter(|(_, &v)| !v)
            .map(|(k, _)| k.clone())
            .collect();
        ProjectSettings {
            filter_extensions,
            filter_folders,
        }
    }

    fn sorted_categories(&self) -> Vec<(ExtensionCategory, Vec<(&String, &outrider_index::scan::ExtensionStats)>)> {
        let mut by_cat: BTreeMap<ExtensionCategory, Vec<(&String, &outrider_index::scan::ExtensionStats)>> =
            BTreeMap::new();
        for (ext, stats) in &self.pre_scan.extensions {
            let cat = project_settings::categorize_extension(ext);
            by_cat.entry(cat).or_default().push((ext, stats));
        }
        let mut cats: Vec<_> = by_cat.into_iter().collect();
        cats.sort_by_key(|(cat, _)| cat.sort_order());
        cats
    }

    fn is_category_all_enabled(&self, exts: &[(&String, &outrider_index::scan::ExtensionStats)]) -> bool {
        exts.iter().all(|(ext, _)| self.extension_enabled.get(*ext).copied().unwrap_or(true))
    }

    fn flat_ext_count(&self) -> usize {
        let categories = self.sorted_categories();
        let mut count = 0;
        for (cat, exts) in &categories {
            count += 1;
            if self.category_expanded.get(cat).copied().unwrap_or(false) {
                count += exts.len();
            }
        }
        count
    }

    fn toggle_ext_at_cursor(&mut self) {
        let categories = self.sorted_categories();
        let mut idx = 0usize;
        for (cat, exts) in &categories {
            if idx == self.ext_cursor {
                let all_on = self.is_category_all_enabled(&exts);
                let keys: Vec<String> = exts.iter().map(|(e, _)| (*e).clone()).collect();
                for ext in keys {
                    self.extension_enabled.insert(ext, !all_on);
                }
                return;
            }
            idx += 1;
            if self.category_expanded.get(cat).copied().unwrap_or(false) {
                for (ext, _) in exts {
                    if idx == self.ext_cursor {
                        let key = (*ext).clone();
                        let v = self.extension_enabled.get(&key).copied().unwrap_or(true);
                        self.extension_enabled.insert(key, !v);
                        return;
                    }
                    idx += 1;
                }
            }
        }
    }

    fn toggle_folder_recursive(&mut self, path: &str, new_val: bool) {
        self.folder_enabled.insert(path.to_string(), new_val);
        let prefix = format!("{path}/");
        let children: Vec<String> = self
            .folder_enabled
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for child in children {
            self.folder_enabled.insert(child, new_val);
        }
    }

    /// Direct children of `prefix` in the folder tree. Returns (name, full_path)
    /// sorted by bytes descending.
    fn folder_children(&self, prefix: &str) -> Vec<(String, String)> {
        let mut children: BTreeMap<String, String> = BTreeMap::new();
        for path in self.pre_scan.folders.keys() {
            let child_name = if prefix.is_empty() {
                if !path.contains('/') {
                    Some(path.as_str())
                } else {
                    None
                }
            } else if let Some(rest) = path.strip_prefix(prefix).and_then(|r| r.strip_prefix('/')) {
                if !rest.contains('/') {
                    Some(rest)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(name) = child_name {
                children.insert(name.to_string(), path.clone());
            }
        }
        // Also include gitignored folders at root level
        if prefix.is_empty() {
            for name in &self.pre_scan.gitignored_folders {
                children.entry(name.clone()).or_insert_with(|| name.clone());
            }
        }
        let mut result: Vec<_> = children.into_iter().collect();
        result.sort_by(|a, b| {
            let a_bytes = self.pre_scan.folders.get(&a.1).map(|s| s.bytes).unwrap_or(0);
            let b_bytes = self.pre_scan.folders.get(&b.1).map(|s| s.bytes).unwrap_or(0);
            b_bytes.cmp(&a_bytes)
        });
        result
    }

    fn folder_has_children(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        self.pre_scan.folders.keys().any(|k| k.starts_with(&prefix))
    }

    /// Build the flat visible folder list for cursor navigation.
    fn visible_folder_paths(&self) -> Vec<String> {
        let mut result = Vec::new();
        self.collect_visible_folders("", &mut result);
        result
    }

    fn collect_visible_folders(&self, prefix: &str, out: &mut Vec<String>) {
        for (_, full_path) in self.folder_children(prefix) {
            out.push(full_path.clone());
            if self.folder_expanded.get(&full_path).copied().unwrap_or(false) {
                self.collect_visible_folders(&full_path, out);
            }
        }
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
    /// Persisted user preferences (effective = global merged with project).
    settings: settings::Settings,
    /// Unmodified global settings for the Settings window (Cmd+,).
    global_settings: settings::Settings,
    /// Whether to show the welcome overlay this session.
    show_welcome: bool,
    /// Working copy of settings while the settings panel is open.
    settings_draft: Option<SettingsDraft>,
    /// Recoverable settings load/save and validation feedback.
    notifications: Notifications,
    /// Right-click context menu, if currently open.
    context_menu: Option<ContextMenu>,
    /// Confirmation dialog before moving a file/folder to trash.
    delete_confirm: Option<std::path::PathBuf>,
    /// Inline rename input state.
    rename_state: Option<RenameState>,
    /// Call graph exploration mode.
    call_graph: Option<CallGraphMode>,
    call_graph_cache: HashMap<SymbolId, CallGraphData>,
    cg_resolver: CallGraphResolver,
    /// Background indexing controller (Open Folder, startup, or re-index).
    loader: ProjectLoader,
    load_progress: Option<LoadProgress>,
    /// Lightweight pre-scan for the project setup screen.
    pre_scanner: PreScanner,
    /// Working copy of the project setup screen while open.
    project_setup: Option<ProjectSetupDraft>,
}

struct RenameState {
    path: std::path::PathBuf,
    input: String,
}

struct CallGraphMode {
    center: SymbolId,
    caller_groups: Vec<CgEdgeGroup>,
    callee_groups: Vec<CgEdgeGroup>,
    selection: CallGraphSelection,
    loading: bool,
    scroll: CgScrollState,
}

struct CgEdgeGroup {
    edges: Vec<CallEdge>,
    active: usize,
}

fn group_edges(edges: Vec<CallEdge>) -> Vec<CgEdgeGroup> {
    let mut groups: Vec<CgEdgeGroup> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for edge in edges {
        if let Some(&idx) = seen.get(&edge.raw_name) {
            groups[idx].edges.push(edge);
        } else {
            seen.insert(edge.raw_name.clone(), groups.len());
            groups.push(CgEdgeGroup {
                edges: vec![edge],
                active: 0,
            });
        }
    }
    groups
}

const CG_SCROLL_SECS: f64 = 0.20;
const CG_CARD_H: f32 = 120.0;
const CG_SELECTED_H: f32 = 300.0;
const CG_CARD_GAP: f32 = 6.0;

struct CgScrollState {
    caller_from: f32,
    callee_from: f32,
    caller_target: f32,
    callee_target: f32,
    started: std::time::Instant,
}

impl CgScrollState {
    fn new_at(caller: f32, callee: f32) -> Self {
        Self {
            caller_from: caller,
            callee_from: callee,
            caller_target: caller,
            callee_target: callee,
            started: std::time::Instant::now(),
        }
    }

    fn is_animating(&self) -> bool {
        self.started.elapsed().as_secs_f64() < CG_SCROLL_SECS
    }

    fn current_offsets(&self) -> (f32, f32) {
        let t = (self.started.elapsed().as_secs_f64() / CG_SCROLL_SECS).min(1.0);
        let e = camera::ease_in_out_cubic(t) as f32;
        (
            self.caller_from + (self.caller_target - self.caller_from) * e,
            self.callee_from + (self.callee_target - self.callee_from) * e,
        )
    }
}

fn cg_scroll_target(selected_idx: usize) -> f32 {
    selected_idx as f32 * (CG_CARD_H + CG_CARD_GAP)
}

fn cg_card_top(i: usize, selected: Option<usize>) -> f32 {
    let mut y = 0.0_f32;
    for j in 0..i {
        y += if selected == Some(j) { CG_SELECTED_H } else { CG_CARD_H };
        y += CG_CARD_GAP;
    }
    y
}

fn cg_card_height(i: usize, selected: Option<usize>) -> f32 {
    if selected == Some(i) { CG_SELECTED_H } else { CG_CARD_H }
}

#[derive(Clone, PartialEq)]
enum CallGraphSelection {
    Caller(usize),
    Callee(usize),
}

struct CgColumnItem {
    name: String,
    parent: Option<String>,
    file: String,
    lines: Vec<(String, Vec<outrider_index::buffer::HighlightSpan>)>,
    selected: bool,
    group_info: Option<(usize, usize)>,
}

fn cg_parent_name(qualified_path: &str) -> Option<String> {
    let after_file = qualified_path.split("::").skip(1).collect::<Vec<_>>();
    if after_file.len() >= 2 {
        Some(after_file[..after_file.len() - 1].join("::"))
    } else {
        None
    }
}

struct InflightResolve {
    generation: u64,
    target: SymbolId,
    rx: std::sync::mpsc::Receiver<CallGraphData>,
}

struct CallGraphResolver {
    generation: u64,
    inflight: Option<InflightResolve>,
}

impl CallGraphResolver {
    fn new() -> Self {
        Self {
            generation: 0,
            inflight: None,
        }
    }

    fn request(&mut self, center: SymbolId, tree: SymbolTree) {
        self.generation = self.generation.wrapping_add(1);
        let gen = self.generation;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let id = center.clone();
        std::thread::spawn(move || {
            let data = outrider_index::call_graph::resolve_calls(&id, &tree);
            let _ = tx.send(data);
        });
        self.inflight = Some(InflightResolve {
            generation: gen,
            target: center,
            rx,
        });
    }

    fn poll(&mut self) -> Option<(SymbolId, CallGraphData)> {
        let inflight = self.inflight.as_ref()?;
        match inflight.rx.try_recv() {
            Ok(data) if inflight.generation == self.generation => {
                let target = inflight.target.clone();
                self.inflight = None;
                Some((target, data))
            }
            Ok(_) => {
                self.inflight = None;
                None
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.inflight = None;
                None
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
        }
    }

    fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.inflight = None;
    }

    fn is_active(&self) -> bool {
        self.inflight.is_some()
    }
}

/// Container headers have no body rows (descriptions were removed).
fn container_body(
    _node: &SymbolNode,
    _rung: Rung,
    _px: &world::PxRect,
    _label_w: f64,
    _vh: f64,
    _pin_y: f64,
    _max_h: f64,
    _focused: bool,
) -> Vec<BodyText> {
    Vec::new()
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
    focused: bool,
    highlight_lines: Option<std::ops::Range<usize>>,
) -> (Vec<BodyText>, usize) {
    let scale = full_h / content::natural_px(node);
    let font = (FONT_PX * scale) as f32;
    let step = LINE_STEP * scale;
    let x = (left + BODY_PAD * scale) as f32;
    let content_y0 = HEADER.max(HEADER * scale);
    let mut out = Vec::new();
    let mut display_row = 0usize;
    let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
    let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
    if let Some(m) = buffers.get(&rel, syms) {
        if let Some(start) = m.symbol_start_line(&node.id) {
            let count = (node.measure as usize).min(m.buffer.len_lines().saturating_sub(start));
            for j in 0..count {
                let file_line = start + j;
                let hl = highlight_lines
                    .as_ref()
                    .is_some_and(|r| file_line >= r.start && file_line < r.end);
                let y = top + content_y0 + display_row as f64 * step;
                if y > vh && !focused {
                    break;
                }
                if let Some((text, spans)) = m.buffer.line(file_line) {
                    if focused {
                        for (shown, runs) in wrap_code_line(&text, spans, label_w as f32, font) {
                            let y = top + content_y0 + display_row as f64 * step;
                            if y <= vh && y + step >= 0.0 {
                                out.push(BodyText {
                                    x,
                                    y: y as f32,
                                    text: shown,
                                    runs,
                                    highlighted: hl,
                                });
                            }
                            display_row += 1;
                        }
                    } else {
                        if y + step >= 0.0 {
                            if let Some((shown, runs)) =
                                code_line(&text, spans, label_w as f32, font)
                            {
                                out.push(BodyText {
                                    x,
                                    y: y as f32,
                                    text: shown,
                                    runs,
                                    highlighted: hl,
                                });
                            }
                        }
                        display_row += 1;
                    }
                } else {
                    display_row += 1;
                }
            }
            return (out, display_row.saturating_sub(count));
        }
    }
    (out, 0)
}

fn focused_width(max_chars: usize) -> f64 {
    let needed = max_chars as f64 * FONT_PX * 0.62 + 2.0 * BODY_PAD;
    needed.clamp(world::PAGE_W, 2.0 * world::PAGE_W)
}

fn expanded_leaf_bounds(packed: Rect, expanded_w: f64, extra_rows: usize) -> Rect {
    Rect {
        w: expanded_w,
        h: packed.h + extra_rows as f64 * LINE_STEP,
        ..packed
    }
}

fn defer_leaf_to_overlay(is_focused: bool, is_leaf: bool) -> bool {
    is_focused && is_leaf
}

fn ring_paints_after_leaf_overlay(is_focused: bool, is_neighbor: bool) -> bool {
    is_focused && !is_neighbor
}

fn max_line_chars(
    node: &SymbolNode,
    buffers: &mut BufferManager,
    file_symbols: &BTreeMap<String, Vec<(SymbolId, usize)>>,
) -> usize {
    if content::is_leaf_item(node) {
        let rel = BufferManager::file_path_of(&node.id.qualified_path).to_string();
        let symbols = file_symbols.get(&rel).map(Vec::as_slice).unwrap_or(&[]);
        if let Some(materialized) = buffers.get(&rel, symbols) {
            if let Some(start) = materialized.symbol_start_line(&node.id) {
                let count = (node.measure as usize)
                    .min(materialized.buffer.len_lines().saturating_sub(start));
                return (0..count)
                    .filter_map(|offset| materialized.buffer.line(start + offset))
                    .map(|(text, _)| text.chars().count())
                    .max()
                    .unwrap_or(0);
            }
        }
    }
    0
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

/// Texture painting only needs the quad to intersect the viewport. GPU image
/// quads continue to render correctly when their projected size is subpixel.
fn leaf_texture_is_visible(
    left: f64,
    width: f64,
    top: f64,
    height: f64,
    viewport_w: f64,
    viewport_h: f64,
) -> bool {
    left < viewport_w && left + width > 0.0 && top < viewport_h && top + height > 0.0
}

/// A composite texture is valid only when every direct child already has a
/// renderable texture image. This prevents color-only placeholders in folders.
fn container_children_have_images(
    node: &SymbolNode,
    mut has_image: impl FnMut(&SymbolId) -> bool,
) -> bool {
    node.children.iter().all(|child| has_image(&child.id))
}

/// Rendered container-header height: always one name row.
fn container_header_px(_zoom: f64) -> f64 {
    HEADER
}

fn container_header_bg_h(_body_len: usize, max_h: f64) -> f64 {
    HEADER.min(max_h)
}

fn header_bg_paint_h(logical_h: f32) -> f32 {
    logical_h
}

/// Screen-space paint offset shared by container header background and text.
fn header_paint_y(y: f64) -> f64 {
    y - 1.0
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

/// The smallest zoom at which a focused leaf remains in the live Text tier:
/// both the font and its code column must clear their rendering thresholds.
fn leaf_text_zoom_floor(r: Rect) -> f64 {
    (content::MIN_TEXT_FONT_PX / FONT_PX).max(world::CODE_MIN_W / r.w)
}

/// Map a symbol node to the semantic tint for its box background.
fn file_ext_tint(path: &str) -> theme::BoxTint {
    let file = path.split("::").next().unwrap_or(path);
    let ext = file.rsplit('.').next().unwrap_or("");
    if ext == file {
        return theme::BoxTint::Normal;
    }
    theme::BoxTint::FileType(theme::extension_tint(ext))
}

fn classify_tint(node: &SymbolNode) -> theme::BoxTint {
    match &node.id.kind {
        SymbolKind::Folder => match node.name.as_str() {
            "docs" | "doc" | "documentation" => theme::BoxTint::DocsFolder,
            "test" | "tests" | "spec" | "specs" | "__tests__" => theme::BoxTint::TestFolder,
            _ => theme::BoxTint::Normal,
        },
        SymbolKind::Item { .. } => file_ext_tint(&node.id.qualified_path),
        SymbolKind::File | SymbolKind::Chunk => file_ext_tint(&node.name),
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

    if cfg!(target_os = "macos") {
        if path.is_dir() {
            let _ = Command::new("open").arg(path).spawn();
        } else {
            let _ = Command::new("open").arg("-R").arg(path).spawn();
        }
    } else if cfg!(target_os = "windows") {
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
        let (settings, settings_notification) = loaded_settings.into_parts();
        let layout = outrider_layout::pack(&tree, &world::pack_config(settings.node_padding));
        let mut view = Self::from_parts(
            tree,
            layout,
            settings,
            settings_notification,
            loading_texture_cache(),
            cx,
        );
        let show_welcome = view.show_welcome;
        if ProjectSettings::exists(&project_root) {
            if let Some(ps) = ProjectSettings::load(&project_root) {
                view.merge_project_settings(&ps);
            }
            view.start_loading(project_root);
        } else {
            view.pre_scanner.start(project_root);
        }
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
        let global_settings = settings.clone();
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
            global_settings,
            show_welcome,
            settings_draft: None,
            notifications,
            context_menu: None,
            delete_confirm: None,
            rename_state: None,
            call_graph: None,
            call_graph_cache: HashMap::new(),
            cg_resolver: CallGraphResolver::new(),
            loader: ProjectLoader::new(),
            load_progress: None,
            pre_scanner: PreScanner::new(),
            project_setup: None,
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

    fn window_title(&self) -> String {
        let name = self
            .tree
            .repo_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "outrider".into());
        format!("outrider — {name}")
    }

    fn map_viewport(window: &Window) -> (f64, f64) {
        let vp = window.viewport_size();
        (f64::from(vp.width), f64::from(vp.height))
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
    fn frame_focus(&mut self, vw: f64, vh: f64, min_zoom: f64, max_zoom: f64) -> Option<Camera> {
        let packed = *self.layout.rects.get(&self.focus.current)?;
        let index = TreeIndex::new(&self.tree);
        let node = index.node(&self.focus.current)?;
        let leaf = content::is_leaf_item(node);
        let framed = if leaf {
            let max_chars = max_line_chars(node, &mut self.buffers, &self.file_symbols);
            let expanded_w = focused_width(max_chars);
            let zoom_floor = min_zoom.max(leaf_text_zoom_floor(packed));
            let mut bounds = expanded_leaf_bounds(packed, expanded_w, 0);
            for _ in 0..2 {
                let provisional = camera::frame_page(bounds, vw, vh, zoom_floor, max_zoom);
                let (_, extra_rows) = leaf_text_body(
                    node,
                    0.0,
                    0.0,
                    packed.h * provisional.zoom,
                    expanded_w * provisional.zoom,
                    f64::INFINITY,
                    &mut self.buffers,
                    &self.file_symbols,
                    true,
                    None,
                );
                let next = expanded_leaf_bounds(packed, expanded_w, extra_rows);
                if next.h == bounds.h {
                    break;
                }
                bounds = next;
            }
            bounds
        } else {
            packed
        };
        Some(self.frame_below_headers(&index, framed, vw, vh, |vh_eff| {
            if leaf {
                camera::frame_page(
                    framed,
                    vw,
                    vh_eff,
                    min_zoom.max(leaf_text_zoom_floor(packed)),
                    max_zoom,
                )
            } else {
                camera::frame_rect(
                    framed,
                    vw,
                    vh_eff,
                    camera::FOCUS_FRACTION,
                    min_zoom,
                    max_zoom,
                )
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
    fn pinned_name(
        item: &world::DrawItem,
        center: bool,
        pin_y: f64,
        shift_as_header: bool,
    ) -> Option<NameRow> {
        let font = FONT_PX as f32;
        let text = truncate_to_width(&item.node.name, item.label_w as f32, font)?;
        let y = if center {
            item.px.y + (item.px.h - f64::from(font) * 1.3) / 2.0
        } else {
            pin_y + 4.0
        };
        let y = if shift_as_header {
            header_paint_y(y)
        } else {
            y
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
    fn paint_items(&mut self, vw: f64, vh: f64) -> (Vec<PaintItem>, Option<DocPanel>, bool) {
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

        let cg_highlight_lines: Option<std::ops::Range<usize>> = self
            .call_graph
            .as_ref()
            .and_then(|mode| {
                let site = match &mode.selection {
                    CallGraphSelection::Callee(i) => {
                        let g = mode.callee_groups.get(*i)?;
                        g.edges[g.active].call_site.as_ref()?
                    }
                    _ => return None,
                };
                let rel =
                    crate::buffers::BufferManager::file_path_of(&mode.center.qualified_path)
                        .to_string();
                let syms = self
                    .file_symbols
                    .get(&rel)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let m = self.buffers.get(&rel, syms)?;
                let start_line = m.buffer.byte_to_line(site.start);
                let end_line = m.buffer.byte_to_line(site.end.saturating_sub(1)) + 1;
                Some(start_line..end_line)
            });

        if let Some(textures) = self.textures.as_mut() {
            textures.begin_visibility_frame();
        }
        let items = world::visible_nodes(&self.tree, &self.layout, &camera, vw, vh, |id| {
            self.textures
                .as_ref()
                .is_some_and(|textures| textures.contains(id))
        });
        let mut out = Vec::with_capacity(items.len());
        let mut focused_paint_idx = None;
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
            let is_focused = item.node.id == focus_id;
            let box_kind = if is_leaf {
                theme::BoxKind::Leaf
            } else if item.node.id.kind == SymbolKind::Folder {
                theme::BoxKind::Folder
            } else if matches!(item.node.id.kind, SymbolKind::Item { .. }) {
                theme::BoxKind::Item
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
            let mut focused_extra_h = 0.0f64;
            let mut expanded_w = 0.0f32;
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
                        name = Self::pinned_name(
                            &item,
                            rung == Rung::Label,
                            header.pin_y,
                            rung != Rung::Label,
                        );
                        body = container_body(
                            item.node,
                            rung,
                            &item.px,
                            item.label_w,
                            vh,
                            header.pin_y,
                            header.max_h,
                            is_focused,
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
                            if textures.has_image(&item.node.id) {
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
                                }
                            } else if container_children_have_images(item.node, |id| {
                                textures.has_image(id)
                            }) {
                                textures.request(item.node.id.clone(), area);
                            }
                        }
                    }
                }
                Draw::Leaf(tier) => {
                    let scale = item.full_h / content::natural_px(item.node);
                    let font = FONT_PX * scale;
                    body_font_px = (FONT_PX * scale) as f32;
                    if tier != LeafDraw::Dot && item.px.h >= 14.0 {
                        name = Self::pinned_name(&item, false, item.px.y, false);
                    }
                    let use_text =
                        font >= content::MIN_TEXT_FONT_PX && item.label_w >= world::CODE_MIN_W;
                    if use_text {
                        let effective_label_w = if is_focused {
                            let max_chars =
                                max_line_chars(item.node, &mut self.buffers, &self.file_symbols);
                            focused_width(max_chars) * camera.zoom
                        } else {
                            item.label_w
                        };
                        if is_focused {
                            expanded_w = effective_label_w as f32;
                        }
                        tex_opacity = 0.0;
                        let hl = if is_focused {
                            cg_highlight_lines.clone()
                        } else {
                            None
                        };
                        let (text_body, extra_rows) = leaf_text_body(
                            item.node,
                            item.left,
                            item.top,
                            item.full_h,
                            effective_label_w,
                            vh,
                            &mut self.buffers,
                            &self.file_symbols,
                            is_focused,
                            hl,
                        );
                        body = text_body;
                        if is_focused && extra_rows > 0 {
                            focused_extra_h = extra_rows as f64 * LINE_STEP * scale;
                        }
                        if !body.is_empty() {
                            let refreshed = self.textures.as_mut().is_some_and(|textures| {
                                textures.refresh_from_live_text(
                                    &item.node.id,
                                    item.label_w * item.full_h,
                                )
                            });
                            if refreshed {
                                let index = TreeIndex::new(&self.tree);
                                let mut parent = index.parent(&item.node.id);
                                while let Some(id) = parent {
                                    if let Some(textures) = self.textures.as_mut() {
                                        textures.invalidate(id);
                                    }
                                    parent = index.parent(id);
                                }
                            }
                        }
                    } else {
                        let (tx, ty, tw, th) =
                            leaf_tex_rect(item.node, item.left, item.top, item.full_h);
                        if leaf_texture_is_visible(tx, tw, ty, th, vw, vh) {
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
            let is_hovered = self.hover_id.as_ref() == Some(&item.node.id);
            if item.node.doc.is_some() && (is_hovered || (is_focused && panel_doc.is_none())) {
                panel_doc = Some((
                    item.node.doc.clone().unwrap(),
                    item.px.x as f32,
                    item.px.y as f32,
                    if expanded_w > 0.0 {
                        expanded_w
                    } else {
                        item.px.w as f32
                    },
                    if expanded_w > 0.0 {
                        (item.full_h + focused_extra_h) as f32
                    } else {
                        item.px.h as f32
                    },
                ));
            }
            out.push(PaintItem {
                x: item.px.x as f32,
                y: item.px.y as f32,
                w: if is_focused && is_leaf && expanded_w > 0.0 {
                    expanded_w
                } else {
                    item.px.w as f32
                },
                h: if is_focused && is_leaf && expanded_w > 0.0 {
                    (item.full_h + focused_extra_h) as f32
                } else {
                    item.px.h as f32
                },
                fill,
                border: theme::border_for(fill),
                stripe: (self.settings.show_churn && item.node.churn > 0.0)
                    .then(|| theme::churn_heat(item.node.churn)),
                focused: is_focused,
                deferred_overlay: defer_leaf_to_overlay(is_focused, is_leaf),
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
            if is_focused && is_leaf && expanded_w > 0.0 {
                focused_paint_idx = Some(out.len() - 1);
            }
        }
        if let Some(index) = focused_paint_idx {
            let focused = out.remove(index);
            out.push(focused);
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
                    highlighted: false,
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
                textures.process_requests_grouped(
                    |id| {
                        index
                            .node(id)
                            .filter(|node| content::is_leaf_item(node))
                            .map(|_| BufferManager::file_path_of(&id.qualified_path).to_string())
                    },
                    |id, rasterizer| {
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
                    },
                )
            } else {
                false
            }
        } else {
            false
        };
        let cg_scrim = self.call_graph.is_some();
        (out, doc_panel, cg_scrim)
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

    fn on_right_press(
        &mut self,
        e: &gpui::MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.call_graph.is_some() { return; }
        let Some(cam) = self.camera else { return };
        let (vw, vh) = Self::map_viewport(window);
        let items = world::visible_nodes(&self.tree, &self.layout, &cam, vw, vh, |id| {
            self.textures
                .as_ref()
                .is_some_and(|textures| textures.contains(id))
        });
        let (mx, my) = (
            f64::from(e.position.x),
            f64::from(e.position.y),
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
        if self.call_graph.is_some() { return; }
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
            f64::from(e.position.y),
        );
        let hit = world::hit_test(&items, mx, my).map(|i| i.node.id.clone());
        drop(items);
        if let Some(id) = hit {
            let index = TreeIndex::new(&self.tree);
            if self.focus.set(id, &index) {
                self.nav_history.push(self.focus.current.clone());
            }
            self.maybe_precompute_call_graph();
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, e: &gpui::MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        if self.call_graph.is_some() { return; }
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
                f64::from(e.position.y),
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
        if self.call_graph.is_some() { return; }
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
                f64::from(e.position.y),
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
        if self.delete_confirm.is_some() {
            if e.keystroke.key.as_str() == "escape" {
                self.delete_confirm = None;
                cx.notify();
            }
            return;
        }
        if self.rename_state.is_some() {
            match e.keystroke.key.as_str() {
                "escape" => self.rename_state = None,
                "enter" => {
                    if let Some(state) = self.rename_state.take() {
                        let new_path = state
                            .path
                            .parent()
                            .unwrap_or(&state.path)
                            .join(&state.input);
                        if let Err(err) = std::fs::rename(&state.path, &new_path) {
                            self.notifications
                                .push(Notification::warning(format!("Rename failed: {err}")));
                        }
                        self.reindex();
                    }
                }
                "backspace" => {
                    if let Some(s) = &mut self.rename_state {
                        s.input.pop();
                    }
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
                        if let Some(s) = &mut self.rename_state {
                            s.input.push(ch);
                        }
                    }
                }
            }
            cx.notify();
            return;
        }
        if self.call_graph.is_some() {
            self.on_call_graph_key(e, window, cx);
            return;
        }
        if self.project_setup.is_some() {
            self.on_project_setup_key(e, cx);
            return;
        }
        if self.show_welcome {
            if e.keystroke.key.as_str() == "escape" {
                self.show_welcome = false;
                cx.notify();
            }
            return;
        }
        if let Some(draft) = &mut self.settings_draft {
            match e.keystroke.key.as_str() {
                "escape" => self.settings_draft = None,
                "tab" => {
                    draft.active = match draft.active {
                        SettingsField::Extensions => SettingsField::Folders,
                        SettingsField::Folders => SettingsField::CacheMb,
                        SettingsField::CacheMb => SettingsField::DiskCacheGb,
                        SettingsField::DiskCacheGb => SettingsField::NodePadding,
                        SettingsField::NodePadding => SettingsField::Extensions,
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
                            SettingsField::CacheMb
                                | SettingsField::DiskCacheGb
                                | SettingsField::NodePadding
                        ) {
                            if ch.is_ascii_digit()
                                || (matches!(
                                    draft.active,
                                    SettingsField::DiskCacheGb | SettingsField::NodePadding
                                ) && ch == '.')
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
                    self.maybe_precompute_call_graph();
                    let (vw, vh) = Self::map_viewport(window);
                    let max_zoom = camera::MAX_ZOOM;
                    let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
                    if let Some(to) = self.frame_focus(vw, vh, min_zoom, max_zoom) {
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
                self.maybe_precompute_call_graph();
                self.frame_focus(vw, vh, min_zoom, max_zoom)
            }
            "escape" => {
                if !self.focus.step_out(&index) {
                    return;
                }
                self.nav_history.push(self.focus.current.clone());
                self.maybe_precompute_call_graph();
                self.frame_focus(vw, vh, min_zoom, max_zoom)
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
            "tab" => {
                self.enter_call_graph(window);
                cx.notify();
                return;
            }
            "left" if e.keystroke.modifiers.alt => {
                let Some(id) = self.nav_history.back().cloned() else {
                    return;
                };
                self.focus.current = id;
                self.focus.record_visit(&index);
                self.neighbors = None;
                self.maybe_precompute_call_graph();
                self.frame_focus(vw, vh, min_zoom, max_zoom)
            }
            "right" if e.keystroke.modifiers.alt => {
                let Some(id) = self.nav_history.forward().cloned() else {
                    return;
                };
                self.focus.current = id;
                self.focus.record_visit(&index);
                self.neighbors = None;
                self.maybe_precompute_call_graph();
                self.frame_focus(vw, vh, min_zoom, max_zoom)
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
                self.maybe_precompute_call_graph();
                self.frame_focus(vw, vh, min_zoom, max_zoom)
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

    fn merge_project_settings(&mut self, ps: &ProjectSettings) {
        self.settings.filter_extensions = self.global_settings.filter_extensions.clone();
        self.settings.filter_folders = self.global_settings.filter_folders.clone();
        for ext in &ps.filter_extensions {
            if !self.settings.filter_extensions.iter().any(|e| e == ext) {
                self.settings.filter_extensions.push(ext.clone());
            }
        }
        for folder in &ps.filter_folders {
            if !self.settings.filter_folders.iter().any(|f| f == folder) {
                self.settings.filter_folders.push(folder.clone());
            }
        }
    }

    fn hide_folder(&mut self, rel_path: &str) {
        let mut ps = ProjectSettings::load(&self.tree.repo_root).unwrap_or(ProjectSettings {
            filter_extensions: vec![],
            filter_folders: vec![],
        });
        if !ps.filter_folders.iter().any(|f| f == rel_path) {
            ps.filter_folders.push(rel_path.to_string());
        }
        if let Err(e) = ps.save(&self.tree.repo_root) {
            self.notifications.push(Notification::warning(e));
            return;
        }
        self.merge_project_settings(&ps);
        self.reindex();
    }

    fn enter_call_graph(&mut self, _window: &Window) {
        let center = self.focus.current.clone();
        let node = Self::find_node(&self.tree.root, &center);
        let is_fn = node.is_some_and(|n| {
            n.children.is_empty()
                && matches!(&n.id.kind, SymbolKind::Item { label } if label == "fn")
        });
        if !is_fn {
            return;
        }
        let initial_offset = cg_scroll_target(0);
        if let Some(data) = self.call_graph_cache.get(&center).cloned() {
            let callee_groups = group_edges(data.callees);
            let caller_groups = group_edges(data.callers);
            let selection = if !callee_groups.is_empty() {
                CallGraphSelection::Callee(0)
            } else {
                CallGraphSelection::Caller(0)
            };
            self.call_graph = Some(CallGraphMode {
                center,
                caller_groups,
                callee_groups,
                selection,
                loading: false,
                scroll: CgScrollState::new_at(initial_offset, initial_offset),
            });
        } else {
            if !self.cg_resolver.is_active() {
                self.cg_resolver.request(center.clone(), self.tree.clone());
            }
            self.call_graph = Some(CallGraphMode {
                center,
                caller_groups: Vec::new(),
                callee_groups: Vec::new(),
                selection: CallGraphSelection::Callee(0),
                loading: true,
                scroll: CgScrollState::new_at(initial_offset, initial_offset),
            });
        }
    }

    fn maybe_precompute_call_graph(&mut self) {
        let id = &self.focus.current;
        if self.call_graph_cache.contains_key(id) {
            return;
        }
        let node = Self::find_node(&self.tree.root, id);
        let is_fn = node.is_some_and(|n| {
            n.children.is_empty()
                && matches!(&n.id.kind, SymbolKind::Item { label } if label == "fn")
        });
        if !is_fn {
            return;
        }
        self.cg_resolver.request(id.clone(), self.tree.clone());
    }

    fn poll_call_graph(&mut self, _window: &Window) -> bool {
        if let Some((id, data)) = self.cg_resolver.poll() {
            self.call_graph_cache.insert(id.clone(), data.clone());
            if let Some(mode) = &mut self.call_graph {
                if mode.loading && mode.center == id {
                    mode.callee_groups = group_edges(data.callees);
                    mode.caller_groups = group_edges(data.callers);
                    mode.loading = false;
                    mode.selection = if !mode.callee_groups.is_empty() {
                        CallGraphSelection::Callee(0)
                    } else {
                        CallGraphSelection::Caller(0)
                    };
                    let initial_offset = cg_scroll_target(0);
                    mode.scroll = CgScrollState::new_at(initial_offset, initial_offset);
                }
            }
            true
        } else {
            false
        }
    }

    fn exit_call_graph(&mut self, window: &Window) {
        if let Some(mode) = self.call_graph.take() {
            let index = TreeIndex::new(&self.tree);
            if self.focus.set(mode.center, &index) {
                self.nav_history.push(self.focus.current.clone());
            }
            self.maybe_precompute_call_graph();
            let (vw, vh) = Self::map_viewport(window);
            let max_zoom = camera::MAX_ZOOM;
            let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
            if let Some(to) = self.frame_focus(vw, vh, min_zoom, max_zoom) {
                self.start_tween(to);
            }
        }
    }

    fn on_project_setup_key(&mut self, e: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let draft = match &mut self.project_setup {
            Some(d) => d,
            None => return,
        };
        match e.keystroke.key.as_str() {
            "escape" => {
                self.project_setup = None;
            }
            "enter" => {
                self.confirm_project_setup();
            }
            "tab" => {
                draft.active_panel = match draft.active_panel {
                    SetupPanel::Extensions => SetupPanel::Folders,
                    SetupPanel::Folders => SetupPanel::Extensions,
                };
            }
            "up" => match draft.active_panel {
                SetupPanel::Extensions => {
                    if draft.ext_cursor > 0 {
                        draft.ext_cursor -= 1;
                    }
                }
                SetupPanel::Folders => {
                    if draft.folder_cursor > 0 {
                        draft.folder_cursor -= 1;
                    }
                }
            },
            "down" => match draft.active_panel {
                SetupPanel::Extensions => {
                    let max = draft.flat_ext_count().saturating_sub(1);
                    if draft.ext_cursor < max {
                        draft.ext_cursor += 1;
                    }
                }
                SetupPanel::Folders => {
                    let visible = draft.visible_folder_paths();
                    let max = visible.len().saturating_sub(1);
                    if draft.folder_cursor < max {
                        draft.folder_cursor += 1;
                    }
                }
            },
            "right" => {
                if draft.active_panel == SetupPanel::Folders {
                    let visible = draft.visible_folder_paths();
                    if let Some(path) = visible.get(draft.folder_cursor) {
                        if draft.folder_has_children(path) {
                            draft.folder_expanded.insert(path.clone(), true);
                        }
                    }
                }
            }
            "left" => {
                if draft.active_panel == SetupPanel::Folders {
                    let visible = draft.visible_folder_paths();
                    if let Some(path) = visible.get(draft.folder_cursor) {
                        if draft.folder_expanded.get(path).copied().unwrap_or(false) {
                            draft.folder_expanded.insert(path.clone(), false);
                        } else if let Some(parent_end) = path.rfind('/') {
                            let parent = &path[..parent_end];
                            if let Some(idx) = visible.iter().position(|p| p == parent) {
                                draft.folder_cursor = idx;
                            }
                        }
                    }
                }
            }
            "space" => {
                match draft.active_panel {
                    SetupPanel::Extensions => {
                        draft.toggle_ext_at_cursor();
                    }
                    SetupPanel::Folders => {
                        let visible = draft.visible_folder_paths();
                        if let Some(path) = visible.get(draft.folder_cursor).cloned() {
                            let v = draft.folder_enabled.get(&path).copied().unwrap_or(true);
                            draft.toggle_folder_recursive(&path, !v);
                        }
                    }
                }
                draft.recompute_stats();
            }
            _ => {}
        }
        cx.notify();
    }

    fn on_call_graph_key(
        &mut self,
        e: &gpui::KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let mode = match &mut self.call_graph {
            Some(m) => m,
            None => return,
        };
        match e.keystroke.key.as_str() {
            "tab" | "escape" => {
                self.exit_call_graph(window);
                cx.notify();
            }
            "left" => {
                match &mode.selection {
                    CallGraphSelection::Callee(_) => {
                        if mode.caller_groups.is_empty() {
                            return;
                        }
                        mode.selection = CallGraphSelection::Caller(0);
                        let (cur_caller, cur_callee) = mode.scroll.current_offsets();
                        mode.scroll = CgScrollState {
                            caller_from: cur_caller,
                            callee_from: cur_callee,
                            caller_target: cg_scroll_target(0),
                            callee_target: cur_callee,
                            started: std::time::Instant::now(),
                        };
                    }
                    CallGraphSelection::Caller(i) => {
                        let g = match mode.caller_groups.get_mut(*i) {
                            Some(g) if g.edges.len() > 1 => g,
                            _ => return,
                        };
                        g.active = (g.active + g.edges.len() - 1) % g.edges.len();
                    }
                }
                cx.notify();
            }
            "right" => {
                match &mode.selection {
                    CallGraphSelection::Caller(_) => {
                        if mode.callee_groups.is_empty() {
                            return;
                        }
                        mode.selection = CallGraphSelection::Callee(0);
                        let (cur_caller, cur_callee) = mode.scroll.current_offsets();
                        mode.scroll = CgScrollState {
                            caller_from: cur_caller,
                            callee_from: cur_callee,
                            caller_target: cur_caller,
                            callee_target: cg_scroll_target(0),
                            started: std::time::Instant::now(),
                        };
                    }
                    CallGraphSelection::Callee(i) => {
                        let g = match mode.callee_groups.get_mut(*i) {
                            Some(g) if g.edges.len() > 1 => g,
                            _ => return,
                        };
                        g.active = (g.active + 1) % g.edges.len();
                    }
                }
                cx.notify();
            }
            "up" => {
                mode.selection = match &mode.selection {
                    CallGraphSelection::Caller(i) if *i > 0 => {
                        CallGraphSelection::Caller(i - 1)
                    }
                    CallGraphSelection::Callee(i) if *i > 0 => {
                        CallGraphSelection::Callee(i - 1)
                    }
                    _ => return,
                };
                let (cur_caller, cur_callee) = mode.scroll.current_offsets();
                let (new_caller_t, new_callee_t) = match &mode.selection {
                    CallGraphSelection::Caller(i) => (cg_scroll_target(*i), cur_callee),
                    CallGraphSelection::Callee(i) => (cur_caller, cg_scroll_target(*i)),
                };
                mode.scroll = CgScrollState {
                    caller_from: cur_caller,
                    callee_from: cur_callee,
                    caller_target: new_caller_t,
                    callee_target: new_callee_t,
                    started: std::time::Instant::now(),
                };
                cx.notify();
            }
            "down" => {
                mode.selection = match &mode.selection {
                    CallGraphSelection::Caller(i) if *i + 1 < mode.caller_groups.len() => {
                        CallGraphSelection::Caller(i + 1)
                    }
                    CallGraphSelection::Callee(i) if *i + 1 < mode.callee_groups.len() => {
                        CallGraphSelection::Callee(i + 1)
                    }
                    _ => return,
                };
                let (cur_caller, cur_callee) = mode.scroll.current_offsets();
                let (new_caller_t, new_callee_t) = match &mode.selection {
                    CallGraphSelection::Caller(i) => (cg_scroll_target(*i), cur_callee),
                    CallGraphSelection::Callee(i) => (cur_caller, cg_scroll_target(*i)),
                };
                mode.scroll = CgScrollState {
                    caller_from: cur_caller,
                    callee_from: cur_callee,
                    caller_target: new_caller_t,
                    callee_target: new_callee_t,
                    started: std::time::Instant::now(),
                };
                cx.notify();
            }
            "enter" => {
                let target = match &mode.selection {
                    CallGraphSelection::Caller(i) => {
                        mode.caller_groups.get(*i).map(|g| g.edges[g.active].target.clone())
                    }
                    CallGraphSelection::Callee(i) => {
                        mode.callee_groups.get(*i).map(|g| g.edges[g.active].target.clone())
                    }
                };
                if let Some(new_center) = target {
                    let index = TreeIndex::new(&self.tree);
                    if self.focus.set(new_center.clone(), &index) {
                        self.nav_history.push(self.focus.current.clone());
                    }
                    let (vw, vh) = Self::map_viewport(window);
                    let max_zoom = camera::MAX_ZOOM;
                    let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
                    if let Some(to) = self.frame_focus(vw, vh, min_zoom, max_zoom) {
                        self.start_tween(to);
                    }
                    let initial_offset = cg_scroll_target(0);
                    if let Some(data) = self.call_graph_cache.get(&new_center).cloned() {
                        let callee_groups = group_edges(data.callees);
                        let caller_groups = group_edges(data.callers);
                        let selection = if !callee_groups.is_empty() {
                            CallGraphSelection::Callee(0)
                        } else {
                            CallGraphSelection::Caller(0)
                        };
                        self.call_graph = Some(CallGraphMode {
                            center: new_center,
                            caller_groups,
                            callee_groups,
                            selection,
                            loading: false,
                            scroll: CgScrollState::new_at(initial_offset, initial_offset),
                        });
                    } else {
                        self.cg_resolver
                            .request(new_center.clone(), self.tree.clone());
                        self.call_graph = Some(CallGraphMode {
                            center: new_center,
                            caller_groups: Vec::new(),
                            callee_groups: Vec::new(),
                            selection: CallGraphSelection::Callee(0),
                            loading: true,
                            scroll: CgScrollState::new_at(initial_offset, initial_offset),
                        });
                    }
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Spawn a background thread to index `folder` and compute its layout.
    fn start_loading(&mut self, folder: std::path::PathBuf) {
        self.loader.start(folder, self.settings.clone());
        self.palette.close();
        self.show_welcome = false;
        self.settings_draft = None;
        self.context_menu = None;
        self.delete_confirm = None;
        self.rename_state = None;
        self.call_graph = None;
        self.call_graph_cache.clear();
        self.cg_resolver.cancel();
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

    fn poll_pre_scan(&mut self) -> bool {
        match self.pre_scanner.poll() {
            PreScanPoll::Idle | PreScanPoll::Scanning => self.pre_scanner.is_scanning(),
            PreScanPoll::Ready(result) => match result {
                Ok(scan) => {
                    let existing = ProjectSettings::load(&self.tree.repo_root);
                    self.project_setup = Some(ProjectSetupDraft::from_pre_scan(scan, existing.as_ref()));
                    self.show_welcome = false;
                    true
                }
                Err(error) => {
                    self.notifications.push(Notification::warning(format!(
                        "Pre-scan failed: {error}"
                    )));
                    self.start_loading(self.tree.repo_root.clone());
                    true
                }
            },
        }
    }

    fn confirm_project_setup(&mut self) {
        if let Some(draft) = self.project_setup.take() {
            let ps = draft.to_project_settings();
            if let Err(e) = ps.save(&self.tree.repo_root) {
                self.notifications.push(Notification::warning(e));
            }
            self.merge_project_settings(&ps);
            self.start_loading(self.tree.repo_root.clone());
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
        let y = f32::from(menu.position.y);
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
                )
                .child(crate::overlays::context_menu_separator())
                .child(
                    crate::overlays::context_menu_row("ctx-rename", "Rename").on_click({
                        let rename_path = fs_path.clone();
                        let rename_name = node_name.clone();
                        cx.listener(move |this, _e, _w, cx| {
                            this.rename_state = Some(RenameState {
                                path: rename_path.clone(),
                                input: rename_name.clone(),
                            });
                            this.context_menu = None;
                            cx.notify();
                        })
                    }),
                )
                .child(
                    crate::overlays::context_menu_row("ctx-delete", "Move to Trash").on_click({
                        let delete_path = fs_path.clone();
                        cx.listener(move |this, _e, _w, cx| {
                            this.delete_confirm = Some(delete_path.clone());
                            this.context_menu = None;
                            cx.notify();
                        })
                    }),
                );

        let is_folder = target.kind == SymbolKind::Folder && !target.qualified_path.is_empty();
        let menu_div = if is_folder {
            let hide_folder = rel_path.clone();
            menu_div
                .child(crate::overlays::context_menu_separator())
                .child(
                    crate::overlays::context_menu_row("ctx-hide-folder", "Hide this Folder")
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.hide_folder(&hide_folder);
                            this.context_menu = None;
                            cx.notify();
                        })),
                )
        } else {
            menu_div
        };

        Some(menu_div)
    }

    fn cg_source_lines(
        &mut self,
        id: &SymbolId,
        max_lines: usize,
    ) -> Vec<(String, Vec<outrider_index::buffer::HighlightSpan>)> {
        let rel = crate::buffers::BufferManager::file_path_of(&id.qualified_path).to_string();
        let syms = self
            .file_symbols
            .get(&rel)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let Some(m) = self.buffers.get(&rel, syms) else {
            return Vec::new();
        };
        let Some(start) = m.symbol_start_line(id) else {
            return Vec::new();
        };
        let node = Self::find_node(&self.tree.root, id);
        let count = node
            .map(|n| (n.measure as usize).min(m.buffer.len_lines().saturating_sub(start)))
            .unwrap_or(0)
            .min(max_lines);
        (0..count)
            .filter_map(|j| {
                m.buffer
                    .line(start + j)
                    .map(|(text, spans)| (text, spans.to_vec()))
            })
            .collect()
    }

    fn render_call_graph(
        &mut self,
        vh: f64,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let mode = self.call_graph.as_ref()?;
        let col_w = 320.0_f32;
        let col_h = (vh as f32 - 96.0).max(200.0);
        let loading = mode.loading;
        let (caller_scroll, callee_scroll) = mode.scroll.current_offsets();

        let caller_sel = match &mode.selection {
            CallGraphSelection::Caller(i) => Some(*i),
            _ => None,
        };
        let callee_sel = match &mode.selection {
            CallGraphSelection::Callee(i) => Some(*i),
            _ => None,
        };

        struct CgGroupSnap {
            target: SymbolId,
            raw_name: String,
            active: usize,
            total: usize,
        }
        let caller_snaps: Vec<CgGroupSnap> = mode
            .caller_groups
            .iter()
            .map(|g| CgGroupSnap {
                target: g.edges[g.active].target.clone(),
                raw_name: g.edges[g.active].raw_name.clone(),
                active: g.active,
                total: g.edges.len(),
            })
            .collect();
        let callee_snaps: Vec<CgGroupSnap> = mode
            .callee_groups
            .iter()
            .map(|g| CgGroupSnap {
                target: g.edges[g.active].target.clone(),
                raw_name: g.edges[g.active].raw_name.clone(),
                active: g.active,
                total: g.edges.len(),
            })
            .collect();

        let max_code_lines = 15;
        let caller_items: Vec<CgColumnItem> = caller_snaps
            .iter()
            .enumerate()
            .map(|(i, snap)| {
                let node = Self::find_node(&self.tree.root, &snap.target);
                let name = node.map(|n| n.name.clone()).unwrap_or_else(|| snap.raw_name.clone());
                let parent = cg_parent_name(&snap.target.qualified_path);
                let file = crate::buffers::BufferManager::file_path_of(&snap.target.qualified_path)
                    .to_string();
                let lines = if caller_sel.map_or(false, |s| i.abs_diff(s) <= 3) {
                    self.cg_source_lines(&snap.target, max_code_lines)
                } else {
                    Vec::new()
                };
                let group_info = if snap.total > 1 {
                    Some((snap.active + 1, snap.total))
                } else {
                    None
                };
                CgColumnItem {
                    name,
                    parent,
                    file,
                    lines,
                    selected: caller_sel == Some(i),
                    group_info,
                }
            })
            .collect();
        let callee_items: Vec<CgColumnItem> = callee_snaps
            .iter()
            .enumerate()
            .map(|(i, snap)| {
                let node = Self::find_node(&self.tree.root, &snap.target);
                let name = node.map(|n| n.name.clone()).unwrap_or_else(|| snap.raw_name.clone());
                let parent = cg_parent_name(&snap.target.qualified_path);
                let file = crate::buffers::BufferManager::file_path_of(&snap.target.qualified_path)
                    .to_string();
                let lines = if callee_sel.map_or(false, |s| i.abs_diff(s) <= 3) {
                    self.cg_source_lines(&snap.target, max_code_lines)
                } else {
                    Vec::new()
                };
                let group_info = if snap.total > 1 {
                    Some((snap.active + 1, snap.total))
                } else {
                    None
                };
                CgColumnItem {
                    name,
                    parent,
                    file,
                    lines,
                    selected: callee_sel == Some(i),
                    group_info,
                }
            })
            .collect();

        let callers_col =
            Self::render_cg_column(&caller_items, "Callers", true, col_w, col_h, loading, caller_scroll, caller_sel);
        let callees_col =
            Self::render_cg_column(&callee_items, "Callees", false, col_w, col_h, loading, callee_scroll, callee_sel);

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(callers_col)
                .child(callees_col),
        )
    }

    fn render_cg_column(
        items: &[CgColumnItem],
        title: &str,
        is_callers: bool,
        width: f32,
        col_h: f32,
        loading: bool,
        scroll_pos: f32,
        selected_idx: Option<usize>,
    ) -> gpui::Stateful<gpui::Div> {
        let col_id = if is_callers { "cg-callers" } else { "cg-callees" };
        let header_h: f32 = 28.0;
        let header = div()
            .absolute()
            .top(px(10.0))
            .left(px(8.0))
            .right(px(8.0))
            .h(px(header_h))
            .text_size(px(11.0))
            .font_family(theme::FONT_FAMILY_SANS)
            .text_color(rgb(theme::TEXT_SECONDARY))
            .child(format!("{title} ({})", items.len()));

        let mut col = div()
            .id(col_id)
            .absolute()
            .top(px(48.0))
            .w(px(width))
            .h(px(col_h))
            .overflow_hidden()
            .child(header);

        if is_callers {
            col = col.left(px(12.0));
        } else {
            col = col.right(px(12.0));
        }

        if loading {
            col = col.child(
                div()
                    .absolute()
                    .top(px(header_h + 20.0))
                    .left(px(8.0))
                    .text_size(px(12.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .child("Resolving..."),
            );
            return col;
        }

        if items.is_empty() {
            col = col.child(
                div()
                    .absolute()
                    .top(px(header_h + 20.0))
                    .left(px(8.0))
                    .text_size(px(12.0))
                    .font_family(theme::FONT_FAMILY_SANS)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .child(format!("No {}", title.to_lowercase())),
            );
            return col;
        }

        let content_top = header_h + 10.0;
        let first_card_h = cg_card_height(0, selected_idx);
        let avail_h = col_h - content_top;
        let center_y = content_top + (avail_h / 2.0 - first_card_h / 2.0).max(0.0);

        for (i, item) in items.iter().enumerate() {
            let card_h = cg_card_height(i, selected_idx);
            let card_y = center_y + cg_card_top(i, selected_idx) - scroll_pos;

            if card_y + card_h < 0.0 || card_y > col_h {
                continue;
            }

            let border_color = if item.selected {
                rgb(theme::FOCUS_BORDER)
            } else {
                rgb(theme::border_for(theme::CODE_BG))
            };

            let mut name_row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_family(theme::FONT_FAMILY_SANS)
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .child(item.name.clone()),
                );
            if let Some((current, total)) = item.group_info {
                name_row = name_row.child(
                    div()
                        .text_size(px(9.0))
                        .font_family(theme::FONT_FAMILY_SANS)
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(format!("{current}/{total}")),
                );
            }

            let mut card = div()
                .absolute()
                .top(px(card_y))
                .left(px(8.0))
                .right(px(8.0))
                .h(px(card_h))
                .overflow_hidden()
                .px(px(8.0))
                .py(px(6.0))
                .bg(rgb(theme::CODE_BG))
                .border_1()
                .border_color(border_color)
                .rounded(px(4.0))
                .child(name_row);

            if let Some(parent) = &item.parent {
                card = card.child(
                    div()
                        .text_size(px(9.0))
                        .font_family(theme::FONT_FAMILY)
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(parent.clone()),
                );
            }

            card = card.child(
                div()
                    .text_size(px(8.0))
                    .font_family(theme::FONT_FAMILY)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .pb(px(4.0))
                    .child(item.file.clone()),
            );

            if !item.lines.is_empty() {
                let mut code_div = div()
                    .border_color(rgb(theme::border_for(theme::CODE_BG)))
                    .border_t_1()
                    .pt(px(4.0));
                for (text, spans) in &item.lines {
                    if let Some((shown, runs)) = code_line(text, spans, width - 32.0, 10.0) {
                        let mut line_div = div()
                            .flex()
                            .flex_row()
                            .text_size(px(10.0))
                            .font_family(theme::FONT_FAMILY);
                        let mut byte_pos = 0;
                        for (len, color) in &runs {
                            let fragment = &shown[byte_pos..(byte_pos + len).min(shown.len())];
                            if !fragment.is_empty() {
                                line_div = line_div.child(
                                    div().text_color(rgb(*color)).child(fragment.to_string()),
                                );
                            }
                            byte_pos += len;
                        }
                        code_div = code_div.child(line_div);
                    }
                }
                card = card.child(code_div);
            }

            col = col.child(card);
        }

        col
    }

    fn render_delete_confirm(
        &self,
        map_w: f64,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let path = self.delete_confirm.as_ref()?;
        let display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let delete_path = path.clone();

        let cancel = crate::overlays::action_button("del-cancel", "Cancel", false)
            .on_click(cx.listener(|this, _e, _w, cx| {
                this.delete_confirm = None;
                cx.notify();
            }));
        let confirm = crate::overlays::action_button("del-confirm", "Move to Trash", true)
            .on_click(cx.listener(move |this, _e, _w, cx| {
                if let Err(err) = trash::delete(&delete_path) {
                    this.notifications
                        .push(Notification::warning(format!("Delete failed: {err}")));
                }
                this.delete_confirm = None;
                this.reindex();
                cx.notify();
            }));

        const WIDTH: f32 = 420.0;
        let left = ((map_w as f32 - WIDTH) / 2.0).max(0.0);
        Some(
            crate::overlays::backdrop().child(
                crate::overlays::centered_panel(80.0, left, WIDTH)
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_family(theme::FONT_FAMILY_SANS)
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .pb(px(14.0))
                            .child(format!("Move \"{display_name}\" to trash?")),
                    )
                    .child(div().h(px(1.0)).mb(px(14.0)).bg(rgb(0x2a2d32_u32)))
                    .child(div().flex().flex_row().gap(px(10.0)).child(cancel).child(confirm)),
            ),
        )
    }

    fn render_rename(
        &self,
        map_w: f64,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let state = self.rename_state.as_ref()?;
        let rename_path = state.path.clone();
        let new_name = state.input.clone();

        let cancel = crate::overlays::action_button("ren-cancel", "Cancel", false)
            .on_click(cx.listener(|this, _e, _w, cx| {
                this.rename_state = None;
                cx.notify();
            }));
        let confirm = crate::overlays::action_button("ren-confirm", "Rename", true)
            .on_click(cx.listener(move |this, _e, _w, cx| {
                let new_path = rename_path.parent().unwrap_or(&rename_path).join(&new_name);
                if let Err(err) = std::fs::rename(&rename_path, &new_path) {
                    this.notifications
                        .push(Notification::warning(format!("Rename failed: {err}")));
                }
                this.rename_state = None;
                this.reindex();
                cx.notify();
            }));

        const WIDTH: f32 = 420.0;
        let left = ((map_w as f32 - WIDTH) / 2.0).max(0.0);
        Some(
            crate::overlays::backdrop().child(
                crate::overlays::centered_panel(80.0, left, WIDTH)
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_family(theme::FONT_FAMILY_SANS)
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .pb(px(14.0))
                            .child("Rename"),
                    )
                    .child(
                        crate::overlays::settings_input("ren-input", state.input.clone(), true),
                    )
                    .child(div().h(px(1.0)).mb(px(14.0)).bg(rgb(0x2a2d32_u32)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(10.0))
                            .child(cancel)
                            .child(confirm),
                    ),
            ),
        )
    }

    fn render_project_setup(&self, map_w: f64, map_h: f64, cx: &mut Context<Self>) -> gpui::Div {
        use crate::overlays::{action_button, category_header, checkbox_row, project_setup_element};
        use gpui::ElementId;

        let draft = self.project_setup.as_ref().unwrap();
        let project_name = self
            .tree
            .repo_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into());
        let title = format!("Project Setup — {project_name}");
        let stats_line = format!(
            "{} files · {} will be indexed",
            draft.filtered_files,
            project_settings::format_bytes(draft.filtered_bytes),
        );

        let categories = draft.sorted_categories();
        let mut ext_rows: Vec<gpui::AnyElement> = Vec::new();
        let mut flat_ext_idx: usize = 0;

        for (cat, exts) in &categories {
            let expanded = draft.category_expanded.get(cat).copied().unwrap_or(false);
            let all_on = draft.is_category_all_enabled(exts);
            let cat_count: usize = exts.iter().map(|(_, s)| s.count).sum();
            let cat_bytes: u64 = exts.iter().map(|(_, s)| s.bytes).sum();
            let detail = format!("({cat_count} files · {})", project_settings::format_bytes(cat_bytes));
            let is_sel = draft.active_panel == SetupPanel::Extensions && draft.ext_cursor == flat_ext_idx;

            let cat_for_expand = *cat;
            let cat_for_toggle = *cat;
            let ext_keys: Vec<String> = exts.iter().map(|(e, _)| (*e).clone()).collect();
            let expand_listener = cx.listener(move |this, _, _, cx| {
                if let Some(d) = &mut this.project_setup {
                    let exp = d.category_expanded.get(&cat_for_expand).copied().unwrap_or(false);
                    d.category_expanded.insert(cat_for_expand, !exp);
                }
                cx.notify();
            });
            let toggle_listener = cx.listener(move |this, _, _, cx| {
                if let Some(d) = &mut this.project_setup {
                    let all_on = ext_keys.iter().all(|e| d.extension_enabled.get(e).copied().unwrap_or(true));
                    for ext in &ext_keys {
                        d.extension_enabled.insert(ext.clone(), !all_on);
                    }
                    d.recompute_stats();
                }
                cx.notify();
            });
            ext_rows.push(
                category_header(
                    ElementId::Name(format!("cat-arrow-{}", cat_for_toggle.label()).into()),
                    ElementId::Name(format!("cat-cb-{}", cat_for_toggle.label()).into()),
                    format!("{} ({})", cat.label(), exts.len()),
                    detail,
                    all_on,
                    expanded,
                    is_sel,
                    |arrow| arrow.on_click(expand_listener),
                    |cb| cb.on_click(toggle_listener),
                )
                .into_any_element(),
            );
            flat_ext_idx += 1;

            if expanded {
                for (ext, stats) in exts {
                    let enabled = draft.extension_enabled.get(*ext).copied().unwrap_or(true);
                    let detail = format!("{} · {}", stats.count, project_settings::format_bytes(stats.bytes));
                    let is_sel = draft.active_panel == SetupPanel::Extensions && draft.ext_cursor == flat_ext_idx;
                    let ext_owned = (*ext).clone();
                    ext_rows.push(
                        checkbox_row(
                            ElementId::Name(format!("ext-{ext}").into()),
                            format!(".{ext}"),
                            detail,
                            enabled,
                            is_sel,
                            24.0,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = &mut this.project_setup {
                                let v = d.extension_enabled.get(&ext_owned).copied().unwrap_or(true);
                                d.extension_enabled.insert(ext_owned.clone(), !v);
                                d.recompute_stats();
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                    flat_ext_idx += 1;
                }
            }
        }

        let visible_folders = draft.visible_folder_paths();
        let mut folder_rows: Vec<gpui::AnyElement> = Vec::new();
        let mut flat_idx = 0usize;
        self.build_folder_rows(draft, "", 0, &visible_folders, &mut flat_idx, &mut folder_rows, cx);

        let actions = vec![
            action_button("setup-confirm", "Start Indexing", true).on_click(
                cx.listener(|this, _, _, cx| {
                    this.confirm_project_setup();
                    cx.notify();
                }),
            ),
            action_button("setup-cancel", "Cancel", false).on_click(
                cx.listener(|this, _, _, cx| {
                    this.project_setup = None;
                    cx.notify();
                }),
            ),
        ];

        project_setup_element(map_w, map_h, title, stats_line, ext_rows, folder_rows, actions)
    }

    /// Build the settings overlay div (absolutely positioned, centered).
    /// Shows current filter settings read-only with action buttons.
    fn build_folder_rows(
        &self,
        draft: &ProjectSetupDraft,
        prefix: &str,
        depth: usize,
        visible_folders: &[String],
        flat_idx: &mut usize,
        out: &mut Vec<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) {
        use crate::overlays::{category_header, checkbox_row};
        use gpui::ElementId;

        let children = draft.folder_children(prefix);
        let indent = 8.0 + depth as f32 * 16.0;

        for (name, full_path) in children {
            let enabled = draft.folder_enabled.get(&full_path).copied().unwrap_or(true);
            let stats = draft.pre_scan.folders.get(&full_path);
            let detail = if let Some(s) = stats {
                format!("{} files · {}", s.count, project_settings::format_bytes(s.bytes))
            } else {
                "gitignored".into()
            };
            let is_sel = draft.active_panel == SetupPanel::Folders && draft.folder_cursor == *flat_idx;
            let has_kids = draft.folder_has_children(&full_path);
            let expanded = draft.folder_expanded.get(&full_path).copied().unwrap_or(false);
            let is_gitignored = draft.gitignored_set.contains(&full_path);
            let path_owned = full_path.clone();

            let label = if is_gitignored {
                format!("{name}/ (gitignored)")
            } else {
                format!("{name}/")
            };

            if has_kids {
                let path_for_expand = full_path.clone();
                let path_for_toggle = full_path.clone();
                let expand_listener = cx.listener(move |this, _, _, cx| {
                    if let Some(d) = &mut this.project_setup {
                        let exp = d.folder_expanded.get(&path_for_expand).copied().unwrap_or(false);
                        d.folder_expanded.insert(path_for_expand.clone(), !exp);
                    }
                    cx.notify();
                });
                let toggle_listener = cx.listener(move |this, _, _, cx| {
                    if let Some(d) = &mut this.project_setup {
                        let v = d.folder_enabled.get(&path_for_toggle).copied().unwrap_or(true);
                        d.toggle_folder_recursive(&path_for_toggle, !v);
                        d.recompute_stats();
                    }
                    cx.notify();
                });
                out.push(
                    category_header(
                        ElementId::Name(format!("folder-arrow-{full_path}").into()),
                        ElementId::Name(format!("folder-cb-{full_path}").into()),
                        label,
                        detail,
                        enabled,
                        expanded,
                        is_sel,
                        |arrow| arrow.on_click(expand_listener),
                        |cb| cb.on_click(toggle_listener),
                    )
                    .ml(px(indent - 8.0))
                    .into_any_element(),
                );
            } else {
                out.push(
                    checkbox_row(
                        ElementId::Name(format!("folder-{full_path}").into()),
                        label,
                        detail,
                        enabled,
                        is_sel,
                        indent,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = &mut this.project_setup {
                            let v = d.folder_enabled.get(&path_owned).copied().unwrap_or(true);
                            d.folder_enabled.insert(path_owned.clone(), !v);
                            d.recompute_stats();
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
                );
            }

            *flat_idx += 1;

            if has_kids && expanded {
                self.build_folder_rows(draft, &full_path, depth + 1, visible_folders, flat_idx, out, cx);
            }
        }
    }

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
                SettingsField::NodePadding => ("field-node-padding", "Node Padding (px):"),
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
            field(SettingsField::NodePadding, draft.node_padding.clone()),
        ];
        let validation = draft.notification.clone();
        let save = crate::overlays::action_button("settings-save", "Save & Close", true).on_click(
            cx.listener(|this, _event, _window, cx| {
                if let Some(mut draft) = this.settings_draft.take() {
                    let mut candidate = this.global_settings.clone();
                    let result = draft
                        .apply_to(&mut candidate, &this.tree.repo_root)
                        .and_then(|()| candidate.save());
                    match result {
                        Ok(()) => {
                            this.global_settings = candidate.clone();
                            this.settings = candidate;
                            if let Some(ps) = ProjectSettings::load(&this.tree.repo_root) {
                                this.merge_project_settings(&ps);
                            }
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
                        this.global_settings = defaults.clone();
                        this.settings = defaults;
                        if let Some(ps) = ProjectSettings::load(&this.tree.repo_root) {
                            this.merge_project_settings(&ps);
                        }
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

        let mut needs_notify = self.poll_loading();
        needs_notify |= self.poll_call_graph(window);
        needs_notify |= self.poll_pre_scan();
        if needs_notify {
            cx.notify();
        }

        let (vw, vh) = Self::map_viewport(window);
        let is_loading = self.loader.is_loading();

        let skip_treemap = self.project_setup.is_some();
        let (items, doc_panel, cg_scrim) = if skip_treemap {
            (Vec::new(), None, false)
        } else {
            self.paint_items(vw, vh)
        };

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
        let cg_animating = self.call_graph.as_ref().is_some_and(|cg| cg.scroll.is_animating());
        let scanning = self.pre_scanner.is_scanning();
        if self.tween.is_some() || self.bake_pending || is_loading || self.cg_resolver.is_active() || cg_animating || scanning
        {
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

        // Build the toolbar overlay.
        let has_overlays = self.palette.is_open()
            || self.settings_draft.is_some()
            || self.show_welcome
            || self.project_setup.is_some();
        let toolbar_overlay = (!has_overlays).then(|| {
            let show_churn = self.settings.show_churn;
            div()
                .absolute()
                .top(px(8.0))
                .right(px(8.0))
                .child(
                    crate::overlays::toolbar_toggle("churn-toggle", "Git Churn", show_churn)
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.settings.show_churn = !this.settings.show_churn;
                            this.global_settings.show_churn = this.settings.show_churn;
                            let _ = this.global_settings.save();
                            cx.notify();
                        })),
                )
        });

        // Build the context menu overlay (needs cx for click listeners).
        let context_menu_overlay = self.render_context_menu(cx);

        // Build the call graph overlay.
        let call_graph_overlay = self.render_call_graph(vh, cx);

        // Build the delete-confirmation overlay.
        let delete_overlay = self.render_delete_confirm(vw, cx);

        // Build the rename overlay.
        let rename_overlay = self.render_rename(vw, cx);

        // Build the project setup overlay.
        let project_setup_overlay = self
            .project_setup
            .is_some()
            .then(|| self.render_project_setup(vw, vh, cx));

        // Build the pre-scan loading spinner.
        let pre_scan_overlay = (self.pre_scanner.is_scanning() && self.project_setup.is_none()).then(|| {
            let folder_name = self
                .tree
                .repo_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "project".into());
            crate::overlays::pre_scan_loading_element(vw, &folder_name)
        });

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

        window.set_window_title(&self.window_title());
        let map = div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(theme::BG))
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &OpenFolder, _w, cx| {
                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Open Project Folder")
                    .pick_folder()
                {
                    let (settings, warning) =
                        crate::settings::Settings::load().into_parts();
                    this.global_settings = settings.clone();
                    this.settings = settings;
                    if let Some(message) = warning {
                        this.notifications.push(Notification::warning(message));
                    }
                    this.start_loading(folder);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ClearDiskCache, _w, cx| {
                if let Some(textures) = this.textures.as_mut() {
                    textures.request_clear_disk_cache();
                    this.bake_pending = true;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleSettings, _w, cx| {
                if this.settings_draft.is_some() {
                    this.settings_draft = None;
                } else {
                    this.settings_draft = Some(SettingsDraft::from_settings(
                        &this.global_settings,
                        &this.tree.repo_root,
                    ));
                    this.palette.close();
                    this.show_welcome = false;
                    this.context_menu = None;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleProjectSettings, _w, cx| {
                if this.project_setup.is_some() {
                    this.project_setup = None;
                } else {
                    this.pre_scanner.start(this.tree.repo_root.clone());
                    this.palette.close();
                    this.show_welcome = false;
                    this.settings_draft = None;
                    this.context_menu = None;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenFilePalette, _w, cx| {
                this.palette.open(palette::PaletteMode::File, &this.tree);
                this.settings_draft = None;
                this.context_menu = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenSymbolPalette, _w, cx| {
                this.palette.open(palette::PaletteMode::Symbol, &this.tree);
                this.settings_draft = None;
                this.context_menu = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RevealInFileManager, _w, _cx| {
                let path = resolve_fs_path(&this.focus.current, &this.tree.repo_root);
                open_in_file_manager(&path);
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, _w, _cx| {
                    if this.call_graph.is_some() { return; }
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
                        let paint_surface = |item: &PaintItem, window: &mut Window| {
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
                        };
                        let paint_text = |item: &PaintItem, window: &mut Window, cx: &mut App| {
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
                                    cx,
                                );
                            }
                            let body_line_height = px(item.body_font_px * 1.3);
                            for bt in &item.body {
                                if bt.text.is_empty() {
                                    continue;
                                }
                                if bt.highlighted {
                                    let char_w = item.body_font_px * 0.62;
                                    let hw = char_w * bt.text.len() as f32 + 12.0;
                                    let hh = item.body_font_px * 1.3;
                                    window.paint_quad(quad(
                                        Bounds::new(
                                            point(origin.x + px(bt.x - 4.0), origin.y + px(bt.y)),
                                            size(px(hw), px(hh)),
                                        ),
                                        px(2.0),
                                        rgba(0x4488ff30),
                                        px(0.),
                                        transparent_black(),
                                        BorderStyle::default(),
                                    ));
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
                                    cx,
                                );
                            }
                        };
                        // Pass 1: quads, stripes, texture quads (back to front).
                        for item in &items {
                            if !item.deferred_overlay {
                                paint_surface(item, window);
                            }
                        }
                        // Pass 2a: leaf / non-header text (rendered under
                        // pinned headers so code doesn't bleed through).
                        for item in &items {
                            if item.header_bg_h > 0.0 || item.deferred_overlay {
                                continue;
                            }
                            paint_text(item, window, _cx);
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
                                    origin.y
                                        + px(header_paint_y(f64::from(item.header_bg_y)) as f32),
                                ),
                                size(
                                    px((item.w - 2.0).max(0.0)),
                                    px(header_bg_paint_h(item.header_bg_h)),
                                ),
                            );
                            window.paint_quad(quad(
                                hb,
                                px(0.),
                                rgb(item.fill),
                                px(0.),
                                rgb(item.fill),
                                BorderStyle::default(),
                            ));
                            paint_text(item, window, _cx);
                        }
                        if cg_scrim {
                            window.paint_quad(quad(
                                bounds,
                                px(0.),
                                rgba(0x000000cc),
                                px(0.),
                                transparent_black(),
                                BorderStyle::default(),
                            ));
                        }
                        let paint_ring = |item: &PaintItem, window: &mut Window| {
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
                        };
                        // Pass 2c: neighbor rings remain above regular content
                        // but below the selected leaf when their bounds overlap.
                        // Skipped in call-graph mode — only the focused node is
                        // above the scrim.
                        if !cg_scrim {
                            for item in &items {
                                if item.neighbor {
                                    paint_ring(item, window);
                                }
                            }
                        }
                        // Pass 2d: selected leaf surface and text above every
                        // regular box, texture, body row, header, and neighbor.
                        if let Some(item) = items.iter().find(|item| item.deferred_overlay) {
                            paint_surface(item, window);
                            paint_text(item, window, _cx);
                        }
                        // Pass 3: only the selected focus ring paints above it.
                        for item in &items {
                            if ring_paints_after_leaf_overlay(item.focused, item.neighbor) {
                                paint_ring(item, window);
                            }
                        }
                        // Pass 4: focused-leaf doc panel (floats to the right).
                        // Skipped in call-graph mode.
                        if let Some(dp) = doc_panel.as_ref().filter(|_| !cg_scrim) {
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
            .children(toolbar_overlay)
            .children(palette_overlay)
            .children(settings_overlay)
            .children(welcome_overlay)
            .children(context_menu_overlay)
            .children(call_graph_overlay)
            .children(delete_overlay)
            .children(rename_overlay)
            .children(loading_overlay)
            .children(pre_scan_overlay)
            .children(project_setup_overlay)
            .children(notification_overlay);

        map
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_gibibytes, SettingsDraft};

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
        container_body, container_children_have_images,
        container_header_bg_h, container_header_layout, container_header_px, focused_width,
        header_bg_paint_h, header_paint_y, leaf_tex_rect, leaf_text_body, leaf_texture_is_visible,
        max_line_chars, HEADER, LINE_STEP, BODY_PAD,
    };
    use crate::paint_model::{
        char_budget, code_line, runs_from_spans, truncate_to_width, wrap_code_line, wrap_doc,
        wrap_to_budget,
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

    #[test]
    fn focused_text_width_and_wrapping_are_bounded() {
        let width = 12.0 + 10.0 * 0.62 * 12.0;
        assert_eq!(char_budget(width as f32, 12.0), 10);
        assert_eq!(
            wrap_to_budget("alpha beta gamma", 10),
            vec!["alpha beta", "gamma"]
        );
        assert_eq!(
            wrap_to_budget("abcdefghijklmno", 10),
            vec!["abcdefghij", "klmno"]
        );
        assert!((focused_width(10) - world::PAGE_W).abs() < 1e-9);
        assert!((focused_width(200) - 2.0 * world::PAGE_W).abs() < 1e-9);
    }

    #[test]
    fn expanded_leaf_bounds_move_the_center_to_the_rendered_dimensions() {
        use super::expanded_leaf_bounds;

        let packed = Rect {
            x: 10.0,
            y: 20.0,
            w: 640.0,
            h: 100.0,
        };
        let expanded = expanded_leaf_bounds(packed, 1_000.0, 3);

        assert_eq!(expanded.w, 1_000.0);
        assert!((expanded.h - (100.0 + 3.0 * LINE_STEP)).abs() < 1e-9);
        assert_eq!(expanded.x + expanded.w / 2.0, 510.0);
        assert_eq!(expanded.y + expanded.h / 2.0, 20.0 + expanded.h / 2.0);
    }

    #[test]
    fn only_focused_leaves_are_deferred_to_the_overlay_pass() {
        use super::defer_leaf_to_overlay;

        assert!(defer_leaf_to_overlay(true, true));
        assert!(!defer_leaf_to_overlay(true, false));
        assert!(!defer_leaf_to_overlay(false, true));
    }

    #[test]
    fn only_focus_ring_paints_after_the_leaf_overlay() {
        use super::ring_paints_after_leaf_overlay;

        assert!(ring_paints_after_leaf_overlay(true, false));
        assert!(!ring_paints_after_leaf_overlay(false, true));
    }

    #[test]
    fn focused_code_wrap_preserves_run_coverage() {
        use outrider_index::buffer::{HighlightKind, HighlightSpan};

        let width = 12.0 + 10.0 * 0.62 * 12.0;
        let spans = vec![HighlightSpan {
            range: 0..2,
            kind: HighlightKind::Keyword,
        }];
        let lines = wrap_code_line("fn frobnicate()", &spans, width as f32, 12.0);
        assert_eq!(
            lines.iter().map(|line| line.0.as_str()).collect::<Vec<_>>(),
            vec!["fn frobnic", "ate()"]
        );
        assert!(lines
            .iter()
            .all(|(text, runs)| { runs.iter().map(|run| run.0).sum::<usize>() == text.len() }));
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
        let body = container_body(&f, Rung::Detail, &px, 400.0, 600.0, px.y, 300.0, false);
        assert_eq!(body.len(), 0);
    }

    #[test]
    fn container_texture_waits_for_each_child_image() {
        let mut folder = node(SymbolKind::Folder, "src", None, 2, None, None);
        let first = node(SymbolKind::File, "src/a.rs", Some(0..1), 1, None, None);
        let second = node(SymbolKind::File, "src/b.rs", Some(0..1), 1, None, None);
        folder.children = vec![first.clone(), second.clone()];

        assert!(!container_children_have_images(&folder, |id| id == &first.id));
        assert!(container_children_have_images(&folder, |_| true));
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
        let (body, _extra) = leaf_text_body(
            &leaf,
            0.0,
            0.0,
            natural,
            640.0,
            600.0,
            &mut mgr,
            &file_symbols,
            false,
            None,
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
    fn focused_width_uses_rendered_source_lines_not_a_hidden_signature() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "let short = 1;\n").unwrap();
        let signature = format!("fn {}()", "very_long_".repeat(40));
        let leaf = node(
            SymbolKind::Item { label: "fn".into() },
            "a.rs::short",
            Some(0..15),
            1,
            Some(&signature),
            None,
        );
        let mut manager = BufferManager::new(dir.path().to_path_buf());
        let file_symbols = BTreeMap::from([("a.rs".to_string(), vec![(leaf.id.clone(), 0)])]);

        let max_chars = max_line_chars(&leaf, &mut manager, &file_symbols);

        assert_eq!(max_chars, "let short = 1;".chars().count());
        assert!((focused_width(max_chars) - world::PAGE_W).abs() < 1e-9);
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
        let (body, _extra) = leaf_text_body(
            &leaf,
            0.0,
            0.0,
            2.0 * natural,
            1280.0,
            100_000.0,
            &mut mgr,
            &file_symbols,
            false,
            None,
        );
        assert_eq!(body.len(), 1);
        assert!((f64::from(body[0].y) - 2.0 * HEADER).abs() < 1e-3);
        // buffer unavailable → no body lines
        let mut broken = BufferManager::new(std::path::PathBuf::from("/nonexistent"));
        let (body, _extra) = leaf_text_body(
            &leaf,
            0.0,
            0.0,
            natural,
            640.0,
            600.0,
            &mut broken,
            &BTreeMap::new(),
            false,
            None,
        );
        assert_eq!(body.len(), 0);
    }

    #[test]
    fn focused_leaf_height_counts_wrapped_rows_below_viewport() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "abcdefghijklmno\npqrstuvwxyzabcd\n",
        )
        .unwrap();
        let leaf = node(
            SymbolKind::Item { label: "fn".into() },
            "a.rs::two",
            Some(0..31),
            2,
            None,
            None,
        );
        let mut manager = BufferManager::new(dir.path().to_path_buf());
        let file_symbols = BTreeMap::from([("a.rs".to_string(), vec![(leaf.id.clone(), 0)])]);
        let natural = crate::content::natural_px(&leaf);
        let ten_chars_wide = 12.0 + 10.0 * 0.62 * 12.0;

        let (body, extra_rows) = leaf_text_body(
            &leaf,
            0.0,
            0.0,
            natural,
            ten_chars_wide,
            HEADER + 0.1,
            &mut manager,
            &file_symbols,
            true,
            None,
        );

        assert_eq!(body.len(), 1, "only the first wrapped row is visible");
        assert_eq!(extra_rows, 2, "both source lines wrap below the viewport");
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
    fn classify_tint_items_use_file_extension() {
        use super::classify_tint;
        use crate::theme::BoxTint;
        let mut n = make_node(
            SymbolKind::Item {
                label: "struct".to_string(),
            },
            "Foo",
        );
        n.id.qualified_path = "src/main.rs::Foo".into();
        assert_eq!(
            classify_tint(&n),
            BoxTint::FileType(crate::theme::extension_tint("rs"))
        );
        n.id.qualified_path = "app.ts::Bar".into();
        assert_eq!(
            classify_tint(&n),
            BoxTint::FileType(crate::theme::extension_tint("ts"))
        );
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
        // Item without file extension in qualified_path
        assert_eq!(
            classify_tint(&make_node(SymbolKind::Item { label: "fn".into() }, "foo")),
            BoxTint::Normal
        );
        // File gets FileType tint based on extension
        assert_eq!(
            classify_tint(&make_node(SymbolKind::File, "main.rs")),
            BoxTint::FileType(crate::theme::extension_tint("rs"))
        );
        // Chunk without extension falls back to Normal
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

    #[test]
    fn cached_leaf_texture_remains_visible_at_subpixel_size() {
        assert!(leaf_texture_is_visible(
            120.0, 0.5, 120.0, 0.5, 800.0, 600.0
        ));
    }

    use super::{inset_top, leaf_text_zoom_floor, pinned_stack_h};
    use crate::camera::Camera;
    use crate::focus::TreeIndex;
    use outrider_index::SymbolTree;
    use outrider_layout::{PackLayout, Rect};

    #[test]
    fn container_header_is_always_one_line() {
        assert!((container_header_px(1.0) - HEADER).abs() < 1e-9);
        assert!((container_header_px(0.5) - HEADER).abs() < 1e-9);
        assert!((container_header_px(0.1) - HEADER).abs() < 1e-9);
        assert!((container_header_px(2.0) - HEADER).abs() < 1e-9);
    }

    #[test]
    fn focused_leaf_zoom_floor_keeps_live_text_and_code_width() {
        let normal_page = Rect {
            x: 0.0,
            y: 0.0,
            w: world::PAGE_W,
            h: 2_000.0,
        };
        // For PAGE_W (640), CODE_MIN_W/w = 300/640 ≈ 0.469 dominates over 4/12 ≈ 0.333
        assert!((leaf_text_zoom_floor(normal_page) - 300.0 / world::PAGE_W).abs() < 1e-9);

        let narrow_page = Rect {
            w: 200.0,
            ..normal_page
        };
        assert!((leaf_text_zoom_floor(narrow_page) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn inset_top_pins_a_tall_leaf_below_headers() {
        let page = Rect {
            x: 10.0,
            y: 200.0,
            w: 480.0,
            h: 2_000.0,
        };
        let vh = 600.0;
        let inset = 48.0;
        let cam = inset_top(
            Camera {
                center_x: page.x + page.w / 2.0,
                center_y: page.y + page.h / 2.0,
                zoom: leaf_text_zoom_floor(page),
            },
            page,
            inset,
            vh,
        );
        assert!((screen_y(&cam, page.y, vh) - inset).abs() < 1e-9);
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
            assert!((capped.max_h - HEADER).abs() < 1e-9);
            assert!(capped.pin_y + capped.max_h <= 100.0 + available);

            let stacked =
                container_header_layout(rung, 100.0, 2.0 * HEADER, 100.0 + HEADER - 2.0, 1.0)
                    .unwrap();
            assert!((stacked.max_h - HEADER).abs() < 1e-9);
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

    #[test]
    fn header_paint_group_is_shifted_up_without_losing_height() {
        let logical_h = HEADER as f32;
        let paint_y = header_paint_y(1.0) as f32;
        assert_eq!(paint_y, 0.0);
        assert_eq!(paint_y + header_bg_paint_h(logical_h), logical_h);
        assert_eq!(header_bg_paint_h(0.0), 0.0);
        assert_eq!(header_bg_paint_h(0.5), 0.5);
        assert_eq!(header_paint_y(104.0), 103.0);
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
                w: 640.0,
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
        assert!((h - 2.0 * HEADER).abs() < 1e-9);
        // Header height is constant regardless of zoom.
        let cam = Camera {
            center_x: 0.0,
            center_y: 0.0,
            zoom: 0.5,
        };
        let h = pinned_stack_h(&focus, &layout, &index, &cam, 800.0, 600.0);
        assert!((h - 2.0 * HEADER).abs() < 1e-9);
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
        assert!((h - (150.0 + HEADER)).abs() < 1e-9);
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
