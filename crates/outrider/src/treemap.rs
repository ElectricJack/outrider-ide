//! Main GPUI view for the outrider treemap — drives the render loop, handles
//! all input (mouse drag/zoom/click, keyboard navigation), and translates the
//! world-space layout from `outrider-layout` into per-frame paint instructions
//! (quads, text runs, and baked texture quads) via a static canvas closure.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    canvas, div, point, prelude::*, px, quad, rgb, rgba, size, transparent_black, App, BorderStyle,
    Bounds, ContentMask, Context, Corners, FocusHandle, PathBuilder, Pixels, TextAlign, TextRun,
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
        OpenCommandPalette,
        RevealInFileManager,
        Quit,
        NextViewTab,
        PrevViewTab,
        BaseViewTab,
        ViewTab1,
        ViewTab2,
        ViewTab3,
        ViewTab4,
        ViewTab5,
        ViewTab6,
        ViewTab7,
        ViewTab8,
        ViewTab9,
    ]
);

use outrider_index::call_graph::{CallEdge, CallGraphData};

/// Target of a view-tab keyboard action.
#[derive(Clone, Copy)]
enum TabJump {
    Next,
    Prev,
    Index(usize),
}

use crate::buffers::{collect_file_symbols, BufferManager};
use crate::camera::{self, Camera, CameraTween};
use crate::content::{self, FONT_PX, HEADER, LINE_STEP};
use crate::focus::{self, Focus, TreeIndex};
use crate::interaction::InteractionAction;
use crate::layout_transition::LayoutTransition;
use crate::navigation::NavigationHistory;
use crate::overlays::{ContextMenu, Notification, Notifications};
use crate::paint_model::{
    code_line, runs_from_spans, truncate_to_width, wrap_code_line, BarStrip, BodyText,
    NameRow, PaintItem, RowStyle, TexQuad,
};

use crate::project_loader::{
    LoadProgress, LoaderPoll, PreScanPoll, PreScanner, ProjectLoader, ProjectPreview,
};
use crate::project_settings::{self, ExtensionCategory, ProjectSettings};
use crate::rasterize::{self, TextureCache};
use crate::settings;
use crate::theme;
use crate::world::{self, Draw, LeafDraw, Rung};

/// Left text inset shared by name rows and body rows.
pub(crate) const BODY_PAD: f64 = 6.0;


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
                !ps.filter_extensions
                    .iter()
                    .any(|f| f.eq_ignore_ascii_case(ext))
            } else {
                project_settings::categorize_extension(ext).default_enabled()
            };
            extension_enabled.insert(ext.clone(), enabled);
        }

        let default_excluded: &[&str] = &[
            "target",
            "node_modules",
            "dist",
            "build",
            "__pycache__",
            ".next",
            ".nuxt",
            "out",
            "pkg",
            "vendor",
        ];

        let mut folder_enabled = BTreeMap::new();
        for folder in pre_scan.folders.keys() {
            let enabled = if let Some(ps) = existing {
                !ps.filter_folders.iter().any(|f| f == folder)
            } else {
                let first = folder.split('/').next().unwrap_or(folder);
                !default_excluded
                    .iter()
                    .any(|d| d.eq_ignore_ascii_case(first))
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
                *parent != *folder
                    && folder.starts_with(parent.as_str())
                    && folder.as_bytes().get(parent.len()) == Some(&b'/')
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
            filter_files: vec![],
            max_display_lines: None,
        }
    }

    fn sorted_categories(
        &self,
    ) -> Vec<(
        ExtensionCategory,
        Vec<(&String, &outrider_index::scan::ExtensionStats)>,
    )> {
        let mut by_cat: BTreeMap<
            ExtensionCategory,
            Vec<(&String, &outrider_index::scan::ExtensionStats)>,
        > = BTreeMap::new();
        for (ext, stats) in &self.pre_scan.extensions {
            let cat = project_settings::categorize_extension(ext);
            by_cat.entry(cat).or_default().push((ext, stats));
        }
        let mut cats: Vec<_> = by_cat.into_iter().collect();
        cats.sort_by_key(|(cat, _)| cat.sort_order());
        cats
    }

    fn is_category_all_enabled(
        &self,
        exts: &[(&String, &outrider_index::scan::ExtensionStats)],
    ) -> bool {
        exts.iter()
            .all(|(ext, _)| self.extension_enabled.get(*ext).copied().unwrap_or(true))
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
            let a_bytes = self
                .pre_scan
                .folders
                .get(&a.1)
                .map(|s| s.bytes)
                .unwrap_or(0);
            let b_bytes = self
                .pre_scan
                .folders
                .get(&b.1)
                .map(|s| s.bytes)
                .unwrap_or(0);
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
            if self
                .folder_expanded
                .get(&full_path)
                .copied()
                .unwrap_or(false)
            {
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
    pub(crate) tree: SymbolTree,
    layout: PackLayout,
    layout_transition: Option<LayoutTransition>,
    packing_target_layout: Option<PackLayout>,
    /// Alternate display scaffold when `space.kind == "graph"`: a synthetic
    /// flat tree + graph-positioned layout replacing the treemap pair for
    /// culling, hit testing, navigation, camera, and edge projection.
    graph_scaffold: Option<crate::view::graph_scaffold::GraphScaffold>,
    /// None until the first render supplies a viewport; then Home-framed.
    pub(crate) camera: Option<Camera>,
    home_zoom: f64,
    drag_last: Option<gpui::Point<Pixels>>,
    press_origin: Option<gpui::Point<Pixels>>,
    pub(crate) focus: Focus,
    tween: Option<(CameraTween, std::time::Instant)>,
    focus_handle: FocusHandle,
    buffers: BufferManager,
    /// Line-bar profiles (see `line_bars`): the no-bake code representation.
    line_profiles: crate::line_bars::LineProfiles,
    /// Profiles were scanned this frame (items drew placeholder bars) or
    /// scans are still outstanding: paint again next frame.
    bars_pending: bool,
    file_symbols: BTreeMap<String, Vec<(SymbolId, usize)>>,
    textures: Option<TextureCache>,
    bake_pending: bool,
    /// The four beam-cast arrow targets of the focused node (Left, Right,
    /// Up, Down), cached because layout is immutable per session.
    neighbors: Option<(SymbolId, [Option<SymbolId>; 4])>,
    /// Leaf node currently under the mouse cursor (for doc tooltip).
    hover_id: Option<SymbolId>,
    /// Browser-style history of explicit focus visits (not arrow-key movement).
    pub(crate) nav_history: NavigationHistory,
    /// Persisted user preferences (effective = global merged with project).
    settings: settings::Settings,
    /// Unmodified global settings for the Settings window (Cmd+,).
    global_settings: settings::Settings,
    /// Whether to show the welcome overlay this session.
    show_welcome: bool,
    /// Working copy of settings while the settings panel is open.
    settings_draft: Option<SettingsDraft>,
    /// Recoverable settings load/save and validation feedback.
    pub(crate) notifications: Notifications,
    /// Right-click context menu, if currently open.
    context_menu: Option<ContextMenu>,
    /// Client-side File menu used where GPUI has no native application menu.
    file_menu_open: bool,
    /// Confirmation dialog before moving a file/folder to trash.
    delete_confirm: Option<std::path::PathBuf>,
    /// Inline rename input state.
    rename_state: Option<RenameState>,
    /// Unified panel state (palette + call-graph + future panels).
    panels: crate::view::panel_view::PanelState,
    /// Command palette (Ctrl+Shift+P).
    cmd_palette: crate::view::command_palette::CommandPaletteState,
    /// Call graph exploration mode.
    call_graph: Option<CallGraphMode>,
    call_graph_cache: HashMap<SymbolId, CallGraphData>,
    cg_resolver: CallGraphResolver,
    /// The current view document; starts as the session default.
    pub(crate) view_spec: outrider_view::ViewSpec,
    /// Switchable views: the base treemap plus one tab per view file.
    view_tabs: crate::view::tabs::ViewTabs,
    /// Set on tab switch: after the next scaffold rebuild, reframe the
    /// camera on the retained focus (or the root if it isn't in the view).
    pending_focus_reframe: bool,
    /// Guided-tour playback over the active view's `camera.steps`.
    tour: crate::view::tour::TourState,
    /// User comments on symbols (the feedback half of the agentic loop).
    pub(crate) comments: crate::view::comments::CommentList,
    /// Open comment composer: the in-progress text plus the captured
    /// target (wire id + label). None while closed.
    comment_draft: Option<CommentDraft>,
    /// Deferred tour camera move: enacted in paint_items once the step's
    /// layers have been resolved (frame targets need resolved sets).
    pending_tour_camera: Option<outrider_view::spec::StepTarget>,
    /// Bumped whenever `layout`, `tree`, or the graph scaffold changes, so
    /// the pre-order rect cache knows to rebuild.
    layout_generation: u64,
    /// Pre-order rect cache for the active scaffold (see world::PreorderRects).
    preorder_rects: Option<crate::world::PreorderRects>,
    /// Bumped only when `tree` itself is replaced (not on layout morphs), so
    /// the structural index below survives layout transitions.
    tree_generation: u64,
    /// Owned structural index of `tree` (see world::TreeShape); replaces the
    /// per-frame `TreeIndex::new` rebuilds on the paint path.
    tree_shape: Option<crate::world::TreeShape>,
    /// When the last frame was built; a frame that arrives long after it
    /// is an idle one, where deferred cache work can run without being
    /// noticed (see `ViewResolver::idle_work`).
    last_paint_at: Option<Instant>,
    /// Union rect of a named set: name -> (set revision, layout generation,
    /// rect). A tour frame over a 20k-id set costs ~8ms to union; the tour
    /// camera and its callout both need it on the same frame.
    set_rect_cache: HashMap<String, (u64, u64, Option<Rect>)>,
    /// Screen geometry of the last painted tour callout: card rect and
    /// anchor point. Lets clicks on the card/anchor act on the referenced
    /// item even though the callout is canvas-painted, not a GPUI element.
    tour_callout_hit: Option<((f32, f32, f32, f32), (f32, f32))>,
    /// True when the callout collapsed to its one-line pill because no
    /// placement avoided the target; the panel then carries the prose.
    tour_callout_minimized: bool,
    /// Memoised world rect of the live tour step's target, keyed by
    /// (step index, layout rect count, active tab). A `frame` target unions
    /// every rect in a set — thousands of lookups — so it must not run per
    /// frame.
    tour_target_cache: Option<((usize, usize, usize), Option<Rect>)>,
    view_resolver: outrider_view::ViewResolver,
    pub(crate) view_dirty: outrider_view::Deps,
    pub(crate) metrics: outrider_view::metric::MetricRegistry,
    relations: outrider_view::relation::RelationRegistry,
    partitions: outrider_view::partition::PartitionRegistry,
    /// Background indexing controller (Open Folder, startup, or re-index).
    loader: ProjectLoader,
    load_progress: Option<LoadProgress>,
    /// Lightweight pre-scan for the project setup screen.
    pre_scanner: PreScanner,
    /// Working copy of the project setup screen while open.
    project_setup: Option<ProjectSetupDraft>,
    /// RPC server for external tool integration.
    rpc: Option<crate::view::rpc::RpcServer>,
    /// File watcher for `.outrider/views/*.json`.
    view_watch: Option<crate::view::watch::ViewWatcher>,
    git_watch: Option<crate::view::git_watch::GitWatcher>,
    /// Tracking state for the view file watcher.
    watch_state: crate::view::watch::WatchState,
    /// Shared wake flag between RPC/watcher threads and the GPUI pump task.
    wake: std::sync::Arc<crate::view::rpc::Wake>,
    /// Background pump task that polls the wake flag and notifies GPUI.
    _view_pump: Option<gpui::Task<()>>,
    /// Keeps the folder-picker task alive while the platform dialog is up.
    _folder_prompt: Option<gpui::Task<()>>,
    /// Last known viewport size for camera commands from RPC.
    pub(crate) last_viewport: Option<(f64, f64)>,
}

struct PackingGeometryState<'a> {
    layout: &'a mut PackLayout,
    transition: &'a mut Option<LayoutTransition>,
    target: &'a mut Option<PackLayout>,
    camera: &'a mut Option<Camera>,
    neighbors: &'a mut Option<(SymbolId, [Option<SymbolId>; 4])>,
    hover: &'a mut Option<SymbolId>,
    progress: &'a mut Option<LoadProgress>,
}

impl PackingGeometryState<'_> {
    fn invalidate(&mut self) {
        *self.camera = None;
        *self.neighbors = None;
        *self.hover = None;
    }

    fn apply_snapshot(&mut self, target: PackLayout, now: Instant) {
        let transition = if let Some(transition) = self.transition.take() {
            let retargeted = transition.retarget(target.clone(), now);
            *self.layout = retargeted.sample(now);
            retargeted
        } else {
            LayoutTransition::new(self.layout.clone(), target.clone(), now)
        };
        *self.transition = Some(transition);
        *self.target = Some(target);
        self.invalidate();
    }

    fn finish(&mut self, final_layout: PackLayout) {
        *self.layout = final_layout;
        *self.transition = None;
        *self.target = None;
        *self.progress = None;
        self.invalidate();
    }

    fn fail_after_preview(&mut self) {
        if let Some(target) = self.target.take() {
            *self.layout = target;
        }
        *self.transition = None;
        *self.progress = None;
        self.invalidate();
    }
}

fn map_interaction_enabled_for(loader: &ProjectLoader) -> bool {
    !loader.is_loading()
}

fn project_setup_available_for(has_setup: bool, loader: &ProjectLoader) -> bool {
    has_setup && !loader.is_loading()
}

fn clear_project_setup_before_load<T>(project_setup: &mut Option<T>, pre_scanner: &mut PreScanner) {
    *project_setup = None;
    pre_scanner.cancel();
}

struct RenameState {
    path: std::path::PathBuf,
    input: String,
}

/// In-progress comment: target captured when the composer opened.
struct CommentDraft {
    /// Wire id of the commented symbol; None for a general comment.
    target: Option<String>,
    target_label: String,
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
        y += if selected == Some(j) {
            CG_SELECTED_H
        } else {
            CG_CARD_H
        };
        y += CG_CARD_GAP;
    }
    y
}

fn cg_card_height(i: usize, selected: Option<usize>) -> f32 {
    if selected == Some(i) {
        CG_SELECTED_H
    } else {
        CG_CARD_H
    }
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
    let content_y0 = content::leaf_content_y0(node, scale);
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
                                    style: RowStyle::Code,
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
                                    style: RowStyle::Code,
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

fn call_graph_column_lefts(focus_left: f32, focus_right: f32, column_width: f32) -> (f32, f32) {
    const GAP: f32 = 12.0;
    (focus_left - column_width - GAP, focus_right + GAP)
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
    let content_y0 = content::leaf_content_y0(node, scale);
    (
        left,
        top + content_y0,
        world::PAGE_W * scale,
        node.measure as f64 * LINE_STEP * scale,
    )
}

/// Line-bar geometry for a leaf: the same line area a texture would cover
/// (so bars → texture → text stay registered), one bar per source line at
/// the page's uniform scale. Below this many unclipped pixels a box cannot
/// hold even one bar row and keeps its flat fill.
const MIN_BAR_BOX_PX: f64 = 3.0;

fn leaf_bar_strip(
    node: &SymbolNode,
    left: f64,
    top: f64,
    full_h: f64,
    rows: Option<crate::line_bars::Profile>,
) -> BarStrip {
    let scale = full_h / content::natural_px(node);
    let (x, y, w, _) = leaf_tex_rect(node, left, top, full_h);
    let pad = BODY_PAD * scale;
    BarStrip {
        x: (x + pad) as f32,
        y: y as f32,
        w: (w - 2.0 * pad).max(1.0) as f32,
        pitch: (LINE_STEP * scale) as f32,
        char_w: (FONT_PX * 0.62 * scale) as f32,
        rows,
        n_lines: node.measure as u32,
    }
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
/// Canvas pixels covered by the menu button and tab strip overlays at the
/// top of the window. A header pinned to the viewport top would sit under
/// them, so pinned headers start here instead.
pub(crate) const HEADER_TOP_INSET: f64 = 44.0;

fn container_header_layout(
    rung: Rung,
    clipped_y: f64,
    clipped_h: f64,
    stack_bottom: f64,
    zoom: f64,
) -> Option<ContainerHeaderLayout> {
    // Pinned (box top scrolled past the viewport top): keep clear of the
    // toolbar overlays. A box that simply starts near the top is left alone.
    let pinned = clipped_y <= 0.0;
    let pin_y = if pinned {
        clipped_y.max(stack_bottom).max(HEADER_TOP_INSET.min(clipped_y + clipped_h))
    } else {
        clipped_y.max(stack_bottom)
    };
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

fn descendant_paint_clip(y: f64, h: f64, stack_bottom: f64) -> Option<(f64, f64)> {
    let clip_y = y.max(stack_bottom);
    let bottom = y + h;
    (clip_y < bottom).then_some((clip_y, bottom - clip_y))
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
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
            children: Vec::new(),
        };
        let tree = SymbolTree {
            root,
            repo_root: project_root.clone(),
        };
        let (settings, settings_notification) = loaded_settings.into_parts();
        let layout = outrider_layout::pack(&tree, &world::pack_config(settings.node_padding, settings.max_display_lines));
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
        view.start_view_services(cx);
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
        let buffers = BufferManager::with_background_loading(tree.repo_root.clone());
        let line_profiles = crate::line_bars::LineProfiles::new(tree.repo_root.clone());
        let show_welcome = settings.show_welcome;
        let mut notifications = Notifications::default();
        if let Some(message) = settings_notification {
            notifications.push(Notification::warning(message));
        }
        let global_settings = settings.clone();
        let view_spec = crate::view::session::default_view(&settings);
        let view_tabs = crate::view::tabs::ViewTabs::new(view_spec.clone());
        let comments = crate::view::comments::CommentList::load(&tree.repo_root);
        Self {
            tree,
            layout,
            layout_transition: None,
            packing_target_layout: None,
            graph_scaffold: None,
            camera: None,
            home_zoom: 1.0,
            drag_last: None,
            press_origin: None,
            focus: Focus::new(root_id.clone()),
            tween: None,
            focus_handle: cx.focus_handle(),
            buffers,
            line_profiles,
            bars_pending: false,
            file_symbols,
            textures,
            bake_pending: false,
            neighbors: None,
            hover_id: None,
            nav_history: NavigationHistory::new(root_id, 64),

            settings,
            global_settings,
            show_welcome,
            settings_draft: None,
            notifications,
            context_menu: None,
            file_menu_open: false,
            delete_confirm: None,
            rename_state: None,
            view_spec,
            view_tabs,
            pending_focus_reframe: false,
            tour: crate::view::tour::TourState::default(),
            comments,
            comment_draft: None,
            pending_tour_camera: None,
            tour_target_cache: None,
            tour_callout_hit: None,
            tour_callout_minimized: false,
            layout_generation: 1,
            preorder_rects: None,
            tree_generation: 1,
            tree_shape: None,
            last_paint_at: None,
            set_rect_cache: HashMap::new(),
            view_resolver: outrider_view::ViewResolver::new(),
            view_dirty: outrider_view::Deps::SPEC,
            metrics: outrider_view::metric::MetricRegistry::builtin(),
            relations: outrider_view::relation::RelationRegistry::empty(),
            partitions: outrider_view::partition::PartitionRegistry::default(),
            panels: crate::view::panel_view::PanelState::new(),
            cmd_palette: crate::view::command_palette::CommandPaletteState::new(),
            call_graph: None,
            call_graph_cache: HashMap::new(),
            cg_resolver: CallGraphResolver::new(),
            loader: ProjectLoader::new(),
            load_progress: None,
            pre_scanner: PreScanner::new(),
            project_setup: None,
            rpc: None,
            view_watch: None,
            git_watch: None,
            watch_state: crate::view::watch::WatchState::new(),
            wake: std::sync::Arc::new(crate::view::rpc::Wake::new()),
            _view_pump: None,
            _folder_prompt: None,
            last_viewport: None,
        }
    }

    /// The display scaffold: the (tree, layout) pair every geometric
    /// consumer reads. Treemap mode uses the index tree + packed layout;
    /// graph mode substitutes the synthetic graph scaffold.
    fn active_pair(&self) -> (&SymbolTree, &PackLayout) {
        match &self.graph_scaffold {
            Some(s) => (&s.tree, &s.layout),
            None => (&self.tree, &self.layout),
        }
    }

    fn active_tree(&self) -> &SymbolTree {
        self.active_pair().0
    }

    fn active_layout(&self) -> &PackLayout {
        self.active_pair().1
    }

    /// World-space rect of the root node, used for Home framing.
    fn root_rect(&self) -> Rect {
        let (tree, layout) = self.active_pair();
        layout
            .rects
            .get(&tree.root.id)
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
        let layout = match &self.graph_scaffold {
            Some(s) => &s.layout,
            None => &self.layout,
        };
        let stack = pinned_stack_h(&self.focus.current, layout, index, &c0, vw, vh);
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
        let (frame_tree, frame_layout) = match &self.graph_scaffold {
            Some(s) => (&s.tree, &s.layout),
            None => (&self.tree, &self.layout),
        };
        let packed = *frame_layout.rects.get(&self.focus.current)?;
        let index = TreeIndex::new(frame_tree);
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

    pub(crate) fn enact_camera(&mut self, cmd: &outrider_view::command::CameraCommand, vw: f64, vh: f64) {
        use outrider_view::command::CameraCommand;
        match cmd {
            CameraCommand::Frame(set) => self.enact_frame(set, vw, vh),
            CameraCommand::Home => {
                let root = self.root_rect();
                let c = Camera::fit(root, vw, vh);
                self.home_zoom = c.zoom;
                self.start_tween(c);
            }
            CameraCommand::Focus(_) => {}
            CameraCommand::Follow(_) => {}
        }
    }

    fn enact_frame(&mut self, set: &outrider_view::spec::SetRef, vw: f64, vh: f64) {
        let ids: Vec<outrider_index::SymbolId> = match set {
            outrider_view::spec::SetRef::Name(name) => {
                match self.view_resolver.current() {
                    Some(rv) => match rv.sets.get(name) {
                        Some(s) => s.ids.iter().cloned().collect(),
                        None => {
                            self.notifications
                                .push(Notification::warning(format!("frame: unknown set '{name}'")));
                            return;
                        }
                    },
                    None => {
                        self.notifications
                            .push(Notification::warning("frame: no resolved view yet".to_string()));
                        return;
                    }
                }
            }
            outrider_view::spec::SetRef::Inline(_) => {
                self.notifications
                    .push(Notification::warning("frame: inline set expressions not yet supported".to_string()));
                return;
            }
        };

        let layout = self.packing_target_layout.as_ref().unwrap_or(&self.layout);
        match outrider_view::camera::union_rect(ids.iter(), layout) {
            None => {
                self.notifications
                    .push(Notification::warning("frame: set has no laid-out members".to_string()));
            }
            Some(r) => {
                let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
                let to = camera::frame_rect(
                    r,
                    vw,
                    vh,
                    camera::FRAME_FRACTION,
                    min_zoom,
                    camera::MAX_ZOOM,
                );
                self.start_tween(to);
            }
        }
    }

    fn start_view_services(&mut self, cx: &mut Context<Self>) {
        let root = self.tree.repo_root.clone();
        if self.settings.rpc_enabled {
            match crate::view::rpc::RpcServer::start(&root, std::sync::Arc::clone(&self.wake)) {
                Ok(s) => self.rpc = Some(s),
                Err(e) => self
                    .notifications
                    .push(Notification::warning(format!("RPC disabled: {e}"))),
            }
        }
        self.view_watch = Some(crate::view::watch::ViewWatcher::new(
            &root,
            std::sync::Arc::clone(&self.wake),
        ));
        self.git_watch = crate::view::git_watch::GitWatcher::new(
            &root,
            std::sync::Arc::clone(&self.wake),
        );
        if self._view_pump.is_none() {
            let wake = std::sync::Arc::clone(&self.wake);
            self._view_pump = Some(cx.spawn(
                async move |this: gpui::WeakEntity<TreemapView>,
                            cx: &mut gpui::AsyncApp| {
                    loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(50))
                            .await;
                        if !wake.take() {
                            continue;
                        }
                        if this.update(cx, |_view, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                },
            ));
        }
    }

    pub(crate) fn apply_view_command(
        &mut self,
        cmd: outrider_view::command::ViewCommand,
    ) -> outrider_view::command::Applied {
        // Focus/neighbor rings are session navigation chrome, not view
        // content: a user-authored spec that omits them still needs to show
        // the selected node. Inject the defaults unless the spec declares
        // its own focusRing layer.
        let cmd = match cmd {
            outrider_view::command::ViewCommand::Apply(mut spec) => {
                crate::view::session::ensure_navigation_marks(&mut spec);
                outrider_view::command::ViewCommand::Apply(spec)
            }
            other => other,
        };
        let applied = outrider_view::command::apply(cmd, &mut self.view_spec);
        self.view_dirty |= applied.changed;
        for v in &applied.violations {
            self.notifications.push(Notification::warning(format!(
                "{}: {} ({})",
                v.path, v.message, v.rule
            )));
        }
        applied
    }

    /// Drop every `__palette` panel layer still in the spec.
    ///
    /// The panel table is the usual owner of those layers, but a palette that
    /// went away by another route (a spec swap, a tab change) can leave one
    /// behind. The id is fixed, so a survivor makes the next push a duplicate
    /// and spec validation rejects it — the palette then refuses to open at
    /// all. Remove by index, high to low, so earlier indices stay valid.
    fn remove_stale_palette_layers(&mut self) {
        use outrider_view::spec::LayerSpec;
        let stale: Vec<usize> = self
            .view_spec
            .layers
            .iter()
            .enumerate()
            .filter(|(_, layer)| {
                matches!(layer, LayerSpec::Panel(p) if p.id.as_deref() == Some("__palette"))
            })
            .map(|(i, _)| i)
            .collect();
        for idx in stale.into_iter().rev() {
            self.apply_view_command(outrider_view::command::ViewCommand::RemoveLayer(idx));
        }
    }

    /// Ask the platform for a project folder and load it when one comes back.
    ///
    /// `rfd`'s blocking `pick_folder()` spun a modal dialog from inside GPUI's
    /// event loop and took the process down with it; `prompt_for_paths` hands
    /// the request to the platform and relays the answer over a channel.
    fn prompt_open_folder(&mut self, cx: &mut gpui::Context<Self>) {
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Project Folder".into()),
        });
        self._folder_prompt = Some(cx.spawn(
            async move |this: gpui::WeakEntity<TreemapView>, cx: &mut gpui::AsyncApp| {
                // Cancelled, dismissed, or the platform refused it.
                let Ok(Ok(Some(mut selected))) = prompt.await else {
                    return;
                };
                let Some(folder) = selected.pop() else {
                    return;
                };
                let _ = this.update(cx, |this, cx| {
                    let (settings, warning) = crate::settings::Settings::load().into_parts();
                    this.global_settings = settings.clone();
                    this.settings = settings;
                    if let Some(message) = warning {
                        this.notifications.push(Notification::warning(message));
                    }
                    this.start_loading(folder);
                    cx.notify();
                });
            },
        ));
    }

    fn clear_disk_cache(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.map_interaction_enabled() {
            return;
        }
        if let Some(textures) = self.textures.as_mut() {
            textures.request_clear_disk_cache();
            self.bake_pending = true;
        }
        cx.notify();
    }

    fn open_palette(&mut self, files_only: bool) {
        use outrider_view::command::ViewCommand;
        use outrider_view::spec::*;

        // A palette is singular: Ctrl+T while the file palette is open must
        // replace it, not stack a second one under the same id.
        self.close_all_panels();
        self.remove_stale_palette_layers();

        let kind_filter = if files_only {
            SetExpr::Kind("file".into())
        } else {
            SetExpr::Not(Box::new(SetExpr::Kind("folder".into())))
        };
        let expr = SetExpr::Intersect(vec![
            SetExpr::Fuzzy(String::new()),
            kind_filter.clone(),
        ]);
        self.apply_view_command(ViewCommand::DefineSet {
            name: "__palette".into(),
            expr,
        });
        self.apply_view_command(ViewCommand::PushLayer(LayerSpec::Panel(PanelSpec {
            id: Some("__palette".into()),
            rows: PanelRows::Set(SetRef::Name("__palette".into())),
            sort_by: Some(SortKey::Builtin(BuiltinSort::NameLength)),
            limit: Some(12),
            dock: Dock::Float,
            title: Some(
                if files_only { "File" } else { "Symbol" }.into(),
            ),
            columns: vec![],
        })));
        self.panels.configure_last(|p| {
            p.query = Some(String::new());
            p.set_name = Some("__palette".into());
            p.kind_filter = Some(kind_filter);
            p.preview = true;
            p.captures_mouse = false;
        });
        self.settings_draft = None;
        self.context_menu = None;
    }

    fn open_command_palette(&mut self) {
        let names = self.metrics.names();
        self.cmd_palette.open(&names);
        self.close_all_panels();
        self.settings_draft = None;
        self.context_menu = None;
    }

    fn on_cmd_palette_key(&mut self, e: &gpui::KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        let ch = e.keystroke.key_char.as_ref().and_then(|s| {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_none() { Some(c) } else { None }
        });
        let effect = crate::view::command_palette::cmd_palette_key(
            &mut self.cmd_palette,
            e.keystroke.key.as_str(),
            ch,
        );
        match effect {
            crate::view::command_palette::CmdPaletteEffect::None => {}
            crate::view::command_palette::CmdPaletteEffect::SelectionChanged
            | crate::view::command_palette::CmdPaletteEffect::QueryChanged => {
                cx.notify();
            }
            crate::view::command_palette::CmdPaletteEffect::Close => {
                cx.notify();
            }
            crate::view::command_palette::CmdPaletteEffect::Execute(idx) => {
                if let Some(entry) = self.cmd_palette.entries.get(idx) {
                    match &entry.action {
                        crate::view::command_palette::CommandAction::View(cmd) => {
                            let cmd = cmd.clone();
                            // Special case: Tour::SetSteps with empty vec means "load history"
                            if matches!(&cmd, outrider_view::command::ViewCommand::Tour(
                                outrider_view::command::TourCommand::SetSteps(v)
                            ) if v.is_empty()) {
                                let steps = self.nav_history.to_steps();
                                self.apply_view_command(
                                    outrider_view::command::ViewCommand::Tour(
                                        outrider_view::command::TourCommand::SetSteps(steps),
                                    ),
                                );
                            } else if matches!(&cmd, outrider_view::command::ViewCommand::Camera(
                                outrider_view::command::CameraCommand::Home
                            )) {
                                let cmd_c = outrider_view::command::CameraCommand::Home;
                                self.apply_view_command(outrider_view::command::ViewCommand::Camera(cmd_c.clone()));
                                let (vw, vh) = Self::map_viewport(window);
                                self.enact_camera(&cmd_c, vw, vh);
                            } else if let outrider_view::command::ViewCommand::Tour(tc) = &cmd {
                                if !self.dispatch_tour_command(tc) {
                                    self.apply_view_command(cmd);
                                }
                            } else {
                                self.apply_view_command(cmd);
                            }
                        }
                        crate::view::command_palette::CommandAction::OpenFilePalette => {
                            self.open_palette(true);
                        }
                        crate::view::command_palette::CommandAction::OpenSymbolPalette => {
                            self.open_palette(false);
                        }
                    }
                }
                cx.notify();
            }
        }
    }

    fn render_command_palette(&self, map_w: f64) -> Option<gpui::Div> {
        if !self.cmd_palette.open {
            return None;
        }
        use crate::view::panel_view::PALETTE_W;
        use gpui::{div, px, rgb, IntoElement};

        let left = ((map_w as f32 - PALETTE_W) / 2.0).max(0.0);

        let query_text = format!("[Command] {}│", self.cmd_palette.query);

        let mut list = div()
            .w(px(PALETTE_W))
            .ml(px(left))
            .mt(px(60.0))
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
                    .child(query_text),
            );

        for (vi, &entry_idx) in self.cmd_palette.filtered.iter().enumerate().take(14) {
            let entry = &self.cmd_palette.entries[entry_idx];
            let selected = vi == self.cmd_palette.selection;
            list = list.child(
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
                    .child(format!("{}: {}", entry.category, entry.label)),
            );
        }

        Some(div().absolute().top_0().left_0().size_full().child(list))
    }

    fn redefine_palette_set(&mut self) {
        use outrider_view::command::ViewCommand;
        use outrider_view::spec::SetExpr;

        let inst = match self.panels.open.get(self.panels.active) {
            Some(inst) if inst.query.is_some() && inst.set_name.is_some() => inst,
            _ => return,
        };
        let query = inst.query.clone().unwrap();
        let kind_filter = inst.kind_filter.clone().unwrap_or(SetExpr::Kind("file".into()));
        let set_name = inst.set_name.clone().unwrap();

        let expr = SetExpr::Intersect(vec![SetExpr::Fuzzy(query), kind_filter]);
        self.apply_view_command(ViewCommand::DefineSet {
            name: set_name,
            expr,
        });
    }

    fn close_all_panels(&mut self) {
        let layers = self.panels.close_all();
        for layer_idx in layers.into_iter().rev() {
            self.apply_view_command(outrider_view::command::ViewCommand::RemoveLayer(layer_idx));
        }
    }

    fn poll_rpc(&mut self, window: &gpui::Window, cx: &mut gpui::Context<Self>) -> bool {
        let Some(rpc) = self.rpc.as_ref() else {
            return false;
        };
        let reqs = rpc.drain();
        if reqs.is_empty() {
            return false;
        }
        for req in reqs {
            let result = self.dispatch_rpc(&req, window, cx);
            let _ = req.reply.send(crate::view::rpc::Outbound::Response(
                crate::view::rpc::RpcResponse {
                    id: req.id.clone(),
                    result,
                },
            ));
        }
        true
    }

    fn poll_view_watch(&mut self) -> bool {
        let Some(w) = self.view_watch.as_ref() else {
            return false;
        };
        let events = w.drain();
        if events.is_empty() {
            return false;
        }
        for e in &events {
            self.watch_state.record(e);
        }
        // Every view file is a tab. Removed files drop their tab; changed
        // files refresh theirs. Then switch to the newest changed file so
        // an agent-authored view pops up, but only re-apply in place when
        // the edited file is already the active tab (hot reload).
        let mut removed_active = false;
        for e in &events {
            if let crate::view::watch::WatchEvent::Removed(p) = e {
                removed_active |= self.view_tabs.remove_file(p);
            }
        }
        // Initial scan (several files arrive at once, none tracked before):
        // open the FIRST view alphabetically so a numbered lesson series
        // starts at chapter 1. Later single-file changes switch to that file
        // (an agent just wrote it).
        let initial_scan = self.view_tabs.len() == 1
            && events
                .iter()
                .filter(|e| matches!(e, crate::view::watch::WatchEvent::Changed(_)))
                .count()
                > 1;
        let mut switch_to: Option<usize> = None;
        let newest = self.watch_state.newest().cloned();
        let mut changed_paths: Vec<&std::path::PathBuf> = events
            .iter()
            .filter_map(|e| match e {
                crate::view::watch::WatchEvent::Changed(p) => Some(p),
                _ => None,
            })
            .collect();
        changed_paths.sort();
        for path in changed_paths {
            match std::fs::read_to_string(path)
                .map_err(|e| e.to_string())
                .and_then(|s| {
                    serde_json::from_str::<outrider_view::ViewSpec>(&s).map_err(|e| e.to_string())
                }) {
                Ok(spec) => {
                    let idx = self.view_tabs.upsert_file(path, spec);
                    let pick = if initial_scan {
                        switch_to.is_none()
                    } else {
                        newest.as_ref() == Some(path)
                    };
                    if pick {
                        switch_to = Some(idx);
                    }
                }
                Err(msg) => {
                    if !self.watch_state.retry_once(path) {
                        self.notifications.push(Notification::warning(format!(
                            "{}: {msg}",
                            path.display()
                        )));
                    }
                }
            }
        }
        if let Some(idx) = switch_to {
            if idx == self.view_tabs.active_index() {
                // Hot reload of the active view.
                self.apply_active_tab_spec();
            } else {
                self.switch_to_tab(idx);
            }
        } else if removed_active {
            self.apply_active_tab_spec();
        }
        true
    }

    /// Make tab `idx` active: stash the live spec on the outgoing tab,
    /// apply the incoming tab's spec, and keep the focused symbol — reframed
    /// in the new layout if it exists there, else fall back to the root.
    fn switch_to_tab(&mut self, idx: usize) {
        self.switch_to_tab_inner(idx, false);
    }

    /// `keep_tour`: the switch is driven by the tour itself (a step's `tab`),
    /// so playback must survive. A user-driven switch ends the tour first so
    /// its pushed layers are never stashed into the outgoing tab.
    fn switch_to_tab_inner(&mut self, idx: usize, keep_tour: bool) {
        if idx == self.view_tabs.active_index() {
            return;
        }
        if !keep_tour {
            self.tour_stop();
        }
        let live = self.view_spec.clone();
        self.view_tabs.store_active_spec(live);
        if !self.view_tabs.activate(idx) {
            return;
        }
        self.apply_active_tab_spec_inner(keep_tour);
    }

    /// Apply the active tab's spec to the session and reframe the focus.
    fn apply_active_tab_spec(&mut self) {
        self.apply_active_tab_spec_inner(false);
    }

    fn apply_active_tab_spec_inner(&mut self, in_tour: bool) {
        let tab = self.view_tabs.active().clone();
        let result =
            self.apply_view_command(outrider_view::command::ViewCommand::Apply(tab.spec));
        if result.changed.is_none() {
            let msgs: Vec<_> = result.violations.iter().map(|v| v.message.clone()).collect();
            self.notifications.push(Notification::warning(format!(
                "View '{}' rejected: {}",
                tab.label,
                msgs.join("; ")
            )));
            return;
        }
        // Layout may change (treemap <-> graph). Let paint_items rebuild
        // the scaffold, then reframe on the retained focus next frame.
        self.pending_focus_reframe = true;
        self.neighbors = None;
        self.context_menu = None;
        // An authored walkthrough starts playing as soon as its tab opens —
        // unless a tour is already driving this switch.
        if !in_tour && !self.view_spec.camera.steps.is_empty() {
            self.tour_goto(0);
        }
    }

    // ── Guided tour ──

    /// Resolve a step's `tab` label (title or file stem) to a tab index.
    fn tab_index_for_label(&self, label: &str) -> Option<usize> {
        let want = label.trim().to_lowercase();
        self.view_tabs.tabs().iter().position(|t| {
            t.label.trim().to_lowercase() == want
                || t
                    .path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|st| st.to_string_lossy().to_lowercase() == want)
                    .unwrap_or(false)
        })
    }

    /// Truncate the active spec to the tour base for the active tab, then
    /// push `layers`. Records the base the first time a tab is visited.
    fn tour_apply_layers(&mut self, layers: &[outrider_view::spec::LayerSpec]) {
        let tab = self.view_tabs.active_index();
        self.tour.note_base(tab, self.view_spec.layers.len());
        let base = self.tour.base_layers(tab).unwrap_or(self.view_spec.layers.len());
        while self.view_spec.layers.len() > base {
            let top = self.view_spec.layers.len() - 1;
            self.apply_view_command(outrider_view::command::ViewCommand::RemoveLayer(top));
        }
        for layer in layers {
            self.apply_view_command(outrider_view::command::ViewCommand::PushLayer(layer.clone()));
        }
    }

    /// Jump the tour to step `index`: switch tab if the step asks for one,
    /// rebuild that tab's tour layer stack by replaying the same-tab run,
    /// record the step, and queue the camera move. Starts the tour from the
    /// active view if not yet active.
    fn tour_goto(&mut self, index: usize) {
        if !self.tour.is_active() {
            let steps = self.view_spec.camera.steps.clone();
            if steps.is_empty() {
                return;
            }
            self.tour.start(self.view_tabs.active_index(), steps);
        }
        let Some(plan) = self.tour.plan_goto(index) else {
            return;
        };
        // Which tab does this step play on?
        let want_tab = match plan.tab.as_deref() {
            Some(label) => match self.tab_index_for_label(label) {
                Some(i) => i,
                None => {
                    self.notifications.push(Notification::warning(format!(
                        "Tour step {}: no tab named '{label}'",
                        index + 1
                    )));
                    self.tour.origin_tab
                }
            },
            None => self.tour.origin_tab,
        };
        if want_tab != self.view_tabs.active_index() {
            // Leaving a tab: strip its tour layers so the stash is clean.
            let leaving = self.view_tabs.active_index();
            if let Some(base) = self.tour.base_layers(leaving) {
                while self.view_spec.layers.len() > base {
                    let top = self.view_spec.layers.len() - 1;
                    self.apply_view_command(outrider_view::command::ViewCommand::RemoveLayer(top));
                }
            }
            self.switch_to_tab_inner(want_tab, true);
        }
        self.tour_apply_layers(&plan.tour_layers);
        // Mirror the step index into the origin view's spec for RPC/status
        // (only meaningful while on the origin tab).
        if self.view_tabs.active_index() == self.tour.origin_tab {
            self.apply_view_command(outrider_view::command::ViewCommand::Tour(
                outrider_view::command::TourCommand::Goto(index),
            ));
        }
        self.tour.commit(&plan);
        // Focus targets move keyboard focus too, so the ring, neighbors, and
        // the focused node's doc note all follow the tour.
        if let outrider_view::spec::StepTarget::Focus(wire) = &plan.target {
            if let Ok(id) = outrider_view::symbol_id::parse_wire(wire) {
                let tree = match &self.graph_scaffold {
                    Some(s) => &s.tree,
                    None => &self.tree,
                };
                let index = TreeIndex::new(tree);
                if index.node(&id).is_some() && self.focus.set(id.clone(), &index) {
                    self.view_dirty |= outrider_view::Deps::FOCUS;
                    self.nav_history.push(id);
                    self.neighbors = None;
                }
            }
        }
        self.pending_tour_camera = Some(plan.target);
        self.context_menu = None;
    }

    fn tour_next(&mut self) {
        use crate::view::tour::Advance;
        match self.tour.next_move() {
            Some(Advance::Step(i)) => self.tour_goto(i),
            Some(Advance::Part(p)) => self.tour_goto_part(Some(p)),
            Some(Advance::Part0Back) => self.tour_goto_part(None),
            Some(Advance::StepAtPart(i, p)) => {
                self.tour_goto(i);
                self.tour_goto_part(Some(p));
            }
            None => {}
        }
    }

    fn tour_prev(&mut self) {
        use crate::view::tour::Advance;
        match self.tour.prev_move() {
            Some(Advance::Step(i)) => self.tour_goto(i),
            Some(Advance::Part(p)) => self.tour_goto_part(Some(p)),
            Some(Advance::Part0Back) => self.tour_goto_part(None),
            Some(Advance::StepAtPart(i, p)) => {
                self.tour_goto(i);
                self.tour_goto_part(Some(p));
            }
            None => {}
        }
    }

    /// Move within the live step: to part `part` (or back to the step's own
    /// target with None). Layers and tab are the step's; only the camera,
    /// focus, and narration change.
    fn tour_goto_part(&mut self, part: Option<usize>) {
        if !self.tour.is_active() {
            return;
        }
        if let Some(p) = part {
            if p >= self.tour.part_count() {
                return;
            }
        }
        self.tour.set_part(part);
        let Some((target, _note)) = self.tour.live_target() else {
            return;
        };
        if let outrider_view::spec::StepTarget::Focus(wire) = &target {
            if let Ok(id) = outrider_view::symbol_id::parse_wire(wire) {
                let tree = match &self.graph_scaffold {
                    Some(s) => &s.tree,
                    None => &self.tree,
                };
                let index = TreeIndex::new(tree);
                if index.node(&id).is_some() && self.focus.set(id.clone(), &index) {
                    self.view_dirty |= outrider_view::Deps::FOCUS;
                    self.nav_history.push(id);
                    self.neighbors = None;
                }
            }
        }
        self.pending_tour_camera = Some(target);
        self.tour_target_cache = None;
        self.context_menu = None;
    }

    /// Leave the tour: drop tour-pushed layers from every tab it touched and
    /// clear the step marker. The reader keeps the current tab and camera.
    fn tour_stop(&mut self) {
        if !self.tour.is_active() {
            return;
        }
        let active = self.view_tabs.active_index();
        for (tab, base) in self.tour.touched_tabs() {
            if tab == active {
                while self.view_spec.layers.len() > base {
                    let top = self.view_spec.layers.len() - 1;
                    self.apply_view_command(outrider_view::command::ViewCommand::RemoveLayer(top));
                }
            } else {
                // Tabs we left were stripped on exit; nothing stashed above base.
            }
        }
        if active == self.tour.origin_tab {
            self.apply_view_command(outrider_view::command::ViewCommand::Tour(
                outrider_view::command::TourCommand::Stop,
            ));
        }
        self.tour.clear();
        self.pending_tour_camera = None;
    }

    /// Start (or restart) the active view's tour from step 0.
    fn tour_play(&mut self) {
        if self.view_spec.camera.steps.is_empty() {
            self.notifications
                .push(Notification::warning("This view has no tour steps".to_string()));
            return;
        }
        if self.tour.is_active() {
            self.tour_stop();
        }
        self.tour_goto(0);
    }

    /// Route a playback command (palette / RPC) into the tour engine.
    /// Returns false for commands that only mutate the spec (SetSteps, Add),
    /// which the caller should apply normally.
    pub(crate) fn dispatch_tour_command(
        &mut self,
        cmd: &outrider_view::command::TourCommand,
    ) -> bool {
        use outrider_view::command::TourCommand;
        match cmd {
            TourCommand::Play => self.tour_play(),
            TourCommand::Next => {
                if self.tour.is_active() {
                    self.tour_next();
                } else {
                    self.tour_play();
                }
            }
            TourCommand::Prev => self.tour_prev(),
            TourCommand::Stop => self.tour_stop(),
            TourCommand::Goto(i) => {
                if !self.tour.is_active() && self.view_spec.camera.steps.is_empty() {
                    return true;
                }
                self.tour_goto(*i);
            }
            TourCommand::SetSteps(_) | TourCommand::Add(_) => return false,
        }
        true
    }

    /// Select the live step's referenced item and zoom the camera onto it
    /// (Enter, or a click on the callout card / anchor dot). Focus targets
    /// select the symbol; frame targets zoom the whole set; home refits.
    fn tour_zoom_target(&mut self, vw: f64, vh: f64) {
        let Some((target, _)) = self.tour.live_target() else {
            return;
        };
        // Resolve the target to a world rect (and, for symbols, select it).
        let rect: Option<Rect> = match &target {
            outrider_view::spec::StepTarget::Focus(wire) => self.select_wire(wire),
            outrider_view::spec::StepTarget::Frame(outrider_view::spec::SetRef::Name(n)) => {
                let layout = match &self.graph_scaffold {
                    Some(s) => &s.layout,
                    None => &self.layout,
                };
                self.view_resolver
                    .current()
                    .and_then(|rv| rv.sets.get(n))
                    .and_then(|set| outrider_view::camera::union_rect(set.ids.iter(), layout))
            }
            _ => {
                let (tree, layout) = match &self.graph_scaffold {
                    Some(s) => (&s.tree, &s.layout),
                    None => (&self.tree, &self.layout),
                };
                layout.rects.get(&tree.root.id).copied()
            }
        };
        let Some(r) = rect else { return };
        self.frame_rect_beside_panel(r, vw, vh);
    }

    /// Select `wire`'s symbol (focus ring + nav history) in the active
    /// scaffold, returning its rect there when it has one.
    fn select_wire(&mut self, wire: &str) -> Option<Rect> {
        let id = outrider_view::symbol_id::parse_wire(wire).ok()?;
        let (tree, layout) = match &self.graph_scaffold {
            Some(s) => (&s.tree, &s.layout),
            None => (&self.tree, &self.layout),
        };
        let r = layout.rects.get(&id).copied();
        let index = TreeIndex::new(tree);
        if index.node(&id).is_some() && self.focus.set(id.clone(), &index) {
            self.view_dirty |= outrider_view::Deps::FOCUS;
            self.nav_history.push(id);
            self.neighbors = None;
        }
        r
    }

    /// Frame `r` close (END fraction) into the band left of the right-hand
    /// panel so the panel doesn't cover it.
    fn frame_rect_beside_panel(&mut self, r: Rect, vw: f64, vh: f64) {
        let usable_w = (vw - crate::view::tour_panel::PANEL_W as f64 - 16.0).max(vw * 0.5);
        let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
        let mut to =
            camera::frame_rect(r, usable_w, vh, camera::END_FRACTION, min_zoom, camera::MAX_ZOOM);
        to.center_x += (vw - usable_w) / 2.0 / to.zoom;
        self.start_tween(to);
    }

    /// Open the comment composer on the current selection (hotkey `c`).
    /// Root focus (or an unresolvable id) becomes a general comment.
    fn open_comment_composer(&mut self) {
        let focus_id = self.focus.current.clone();
        let (tree, _) = match &self.graph_scaffold {
            Some(s) => (&s.tree, &s.layout),
            None => (&self.tree, &self.layout),
        };
        let is_root = focus_id == tree.root.id || focus_id == self.tree.root.id;
        let (target, target_label) = if is_root {
            (None, "this view".to_string())
        } else {
            let index = TreeIndex::new(tree);
            let label = index
                .node(&focus_id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| {
                    focus_id
                        .qualified_path
                        .rsplit("::")
                        .next()
                        .unwrap_or(&focus_id.qualified_path)
                        .to_string()
                });
            (Some(outrider_view::symbol_id::to_wire(&focus_id)), label)
        };
        self.comment_draft = Some(CommentDraft {
            target,
            target_label,
            input: String::new(),
        });
    }

    /// Save the open composer as a comment (Enter). Empty text cancels.
    fn commit_comment(&mut self) {
        let Some(d) = self.comment_draft.take() else {
            return;
        };
        let text = d.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        let view = self
            .view_tabs
            .tabs()
            .get(self.view_tabs.active_index())
            .map(|t| t.label.clone())
            .unwrap_or_default();
        let tour_step = self.tour.step.map(|s| {
            let title = self
                .view_tabs
                .tabs()
                .get(self.tour.origin_tab)
                .map(|t| t.label.clone())
                .unwrap_or_default();
            format!("{title} \u{00B7} step {}", s + 1)
        });
        self.comments.add(d.target, d.target_label, text, view, tour_step);
        self.comments.save(&self.tree.repo_root);
    }

    /// The agent prompt for the current comment list, with per-target
    /// file/signature context resolved against the live index.
    pub(crate) fn build_comment_prompt(&self) -> String {
        let index = TreeIndex::new(&self.tree);
        crate::view::comments::build_prompt(
            &self.tree.root.name,
            &self.comments.comments,
            |wire| {
                let id = outrider_view::symbol_id::parse_wire(wire).ok()?;
                let node = index.node(&id)?;
                let file = BufferManager::file_path_of(&id.qualified_path).to_string();
                Some(crate::view::comments::TargetContext {
                    file: (!file.is_empty()).then_some(file),
                    signature: node.signature.clone(),
                })
            },
        )
    }

    /// Keys consumed while a tour is playing. Returns true if handled.
    fn on_tour_key(&mut self, e: &gpui::KeyDownEvent) -> bool {
        if !self.tour.is_active() {
            return false;
        }
        let m = &e.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return false;
        }
        if e.keystroke.key.as_str() == "enter" {
            if let Some((vw, vh)) = self.last_viewport {
                self.tour_zoom_target(vw, vh);
            }
            return true;
        }
        match e.keystroke.key.as_str() {
            "right" | "space" | "n" | "pagedown" => {
                self.tour_next();
                true
            }
            "left" | "p" | "pageup" => {
                self.tour_prev();
                true
            }
            "escape" => {
                self.tour_stop();
                true
            }
            _ => false,
        }
    }

    /// Keyboard tab switching (bound in main.rs): Ctrl+Tab / Ctrl+Shift+Tab
    /// cycle, Ctrl+1..9 jump, Ctrl+` returns to the base treemap.
    fn tab_action(&mut self, jump: TabJump) {
        if !self.map_interaction_enabled() {
            return;
        }
        let n = self.view_tabs.len();
        let cur = self.view_tabs.active_index();
        let target = match jump {
            TabJump::Next if n > 1 => Some((cur + 1) % n),
            TabJump::Prev if n > 1 => Some((cur + n - 1) % n),
            TabJump::Index(i) if i < n => Some(i),
            _ => None,
        };
        if let Some(t) = target {
            self.switch_to_tab(t);
        }
    }

    fn poll_git_watch(&mut self) -> bool {
        let Some(w) = self.git_watch.as_ref() else {
            return false;
        };
        if w.drain().is_some() {
            self.view_dirty |= outrider_view::Deps::GIT;
            true
        } else {
            false
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

    /// Take the cached structural index of `tree`, building it if the tree
    /// changed. Callers put it back via `self.tree_shape = Some(..)`.
    fn ensure_tree_shape(&mut self) -> world::TreeShape {
        world::TreeShape::for_generation(self.tree_shape.take(), &self.tree, self.tree_generation)
    }

    /// Union rect of a resolved set over `layout`, memoized per set revision
    /// and layout generation (see `set_rect_cache`).
    fn set_union_rect(
        cache: &mut HashMap<String, (u64, u64, Option<Rect>)>,
        name: &str,
        set: &outrider_view::set::ResolvedSet,
        layout: &PackLayout,
        layout_generation: u64,
    ) -> Option<Rect> {
        if let Some((rev, gen, rect)) = cache.get(name) {
            if *rev == set.revision && *gen == layout_generation {
                return *rect;
            }
        }
        let rect = outrider_view::camera::union_rect(set.ids.iter(), layout);
        cache.insert(name.to_string(), (set.revision, layout_generation, rect));
        rect
    }

    /// Advance the tween, materialize buffers/textures, and build the
    /// `PaintItem` list + optional focused-leaf doc panel for the current
    /// frame; also kicks off queued bakes.
    fn paint_items(&mut self, vw: f64, vh: f64) -> crate::paint_model::PaintFrame {
        let mut prof = crate::frame_profile::FrameProfile::begin();
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
        self.last_viewport = Some((vw, vh));
        let focus_id = self.focus.current.clone();
        // Neighbor targets are a full-layout scan; while a layout morph is
        // animating the rects are transient, so recomputing per frame both
        // stutters and produces throwaway answers. Wait for it to settle.
        let stale = !matches!(&self.neighbors, Some((k, _)) if k == &focus_id);
        // Entering a graph tab: the scaffold is built later this frame, so
        // computing neighbors now would scan the full treemap layout only
        // to be thrown away. Wait for the scaffold.
        let scaffold_pending = self.view_spec.space.kind
            == outrider_view::spec::SpaceKind::Graph
            && self.graph_scaffold.is_none();
        if stale && self.layout_transition.is_none() && !scaffold_pending {
            let n = match &self.graph_scaffold {
                Some(s) => {
                    // Diagram scaffolds are small: a throwaway index is cheap.
                    let index = TreeIndex::new(&s.tree);
                    focus::neighbors(&focus_id, &s.layout, &index)
                }
                None => {
                    // The base tree is large (30k+ nodes): scan the cached
                    // pre-order structure instead of hashing every rect.
                    let shape = self.ensure_tree_shape();
                    let pre = world::PreorderRects::for_generation(
                        self.preorder_rects.take(),
                        &self.tree,
                        &self.layout,
                        self.layout_generation,
                    );
                    let found = focus::neighbors_by_position(&focus_id, &shape, &pre)
                        .map(|slots| {
                            slots.map(|p| p.and_then(|p| shape.node_at(&self.tree, p)).map(|n| n.id.clone()))
                        })
                        .unwrap_or([None, None, None, None]);
                    self.preorder_rects = Some(pre);
                    self.tree_shape = Some(shape);
                    found
                }
            };
            self.neighbors = Some((focus_id.clone(), n));
        }
        crate::frame_profile::profile_phase!(prof, "neighbors");

        let session = outrider_view::SessionState {
            focus: &focus_id,
            hover: self.hover_id.as_ref(),
            selection: self.panels.cached_selection(),
            visible: None,
            head: None,
            neighbors: self.neighbors.as_ref().map(|(_, n)| n),
        };
        let ctx = outrider_view::ResolveCtx {
            tree: &self.tree,
            layout: &self.layout,
            metrics: &self.metrics,
            relations: &self.relations,
            partitions: &self.partitions,
            session,
            repo_root: &self.tree.repo_root,
        };
        let dirty = std::mem::take(&mut self.view_dirty);
        // Idle frame (nothing animating, no frame in the last 40ms — a
        // wheel zoom or drag repaints far more often): let the resolver
        // take its deferred snapshot or pre-warm a fill the tour will
        // push, one unit per frame, so tour steps don't pay for it.
        let idle = self.tween.is_none()
            && self.layout_transition.is_none()
            && dirty.is_none()
            && self
                .last_paint_at
                .is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(40));
        if idle {
            let warm: Vec<outrider_view::spec::FillSpec> = self
                .tour
                .steps
                .iter()
                .filter(|s| s.tab.is_none())
                .flat_map(|s| s.push.iter())
                .filter_map(|l| match l {
                    outrider_view::spec::LayerSpec::Fill(f) => Some(f.clone()),
                    _ => None,
                })
                .collect();
            self.view_resolver.idle_work(&ctx, &warm);
        }
        self.last_paint_at = Some(Instant::now());
        crate::frame_profile::profile_phase!(prof, "pre");
        let resolved = self.view_resolver.resolve(&self.view_spec, &ctx, dirty);
        crate::frame_profile::profile_phase!(prof, "resolve");
        self.panels.sync(&resolved.panels);
        crate::frame_profile::profile_phase!(prof, "panels");
        let ov = crate::view::paint_resolver::PaintOverrides::new(resolved);

        // Maintain the graph scaffold when the spec requests graph space.
        let graph_mode =
            self.view_spec.space.kind == outrider_view::spec::SpaceKind::Graph;
        let mut scaffold_changed = false;
        if graph_mode {
            let stale = self.graph_scaffold.is_none()
                || dirty.intersects(
                    outrider_view::Deps::SPEC
                        | outrider_view::Deps::TREE
                        | outrider_view::Deps::RELATIONS,
                );
            if stale {
                let old_count = self
                    .graph_scaffold
                    .as_ref()
                    .map(|s| s.tree.root.children.len());
                self.layout_generation += 1;
                self.graph_scaffold = Some(crate::view::graph_scaffold::build(
                    &self.tree,
                    resolved,
                    self.view_spec.space.members.as_ref(),
                    self.view_spec.space.direction == outrider_view::spec::GraphDirection::Lr,
                ));
                let new_count = self
                    .graph_scaffold
                    .as_ref()
                    .map(|s| s.tree.root.children.len());
                scaffold_changed = old_count != new_count;
            }
        } else if self.graph_scaffold.take().is_some() {
            self.layout_generation += 1;
            scaffold_changed = true;
        }
        let (active_tree, active_layout): (&SymbolTree, &PackLayout) =
            match &self.graph_scaffold {
                Some(s) => (&s.tree, &s.layout),
                None => (&self.tree, &self.layout),
            };
        // Reframe after a scaffold change or tab switch. Keep the focused
        // symbol if the new layout has it (frame it), else fall back to the
        // root fit. The base tab never rebuilds a scaffold, so the tab
        // switch flag is what drives treemap-side reframing.
        // A queued tour move supersedes the tab-switch reframe (both can be
        // armed by the same switch when the new tab auto-starts its tour).
        if self.pending_tour_camera.is_some() {
            self.pending_focus_reframe = false;
        }
        // While a tour is playing, any scaffold growth (async edges arriving)
        // re-enacts the live step's target rather than focus-framing: the
        // step said where to look.
        if scaffold_changed && self.pending_tour_camera.is_none() {
            if let Some((target, _)) = self.tour.live_target() {
                self.pending_tour_camera = Some(target);
            }
        }
        let reframe = (scaffold_changed || std::mem::take(&mut self.pending_focus_reframe))
            && self.pending_tour_camera.is_none();
        let camera = if reframe {
            let root_rect = active_layout
                .rects
                .get(&active_tree.root.id)
                .copied()
                .unwrap_or(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 });
            let home = Camera::fit(root_rect, vw, vh);
            self.home_zoom = home.zoom;
            // A view that declares `camera.frame` (a set) or `camera.focus`
            // (a symbol) is stating where it wants the reader to look — an
            // authored view's framing wins over the retained focus. Without
            // either, keep the focused symbol if the new layout has it.
            let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
            let declared_frame: Option<Rect> = match &self.view_spec.camera.frame {
                Some(outrider_view::spec::SetRef::Name(name)) => {
                    resolved.sets.get(name).and_then(|s| {
                        Self::set_union_rect(
                            &mut self.set_rect_cache,
                            name,
                            s,
                            active_layout,
                            self.layout_generation,
                        )
                    })
                }
                _ => None,
            };
            let declared_focus: Option<Rect> = self
                .view_spec
                .camera
                .focus
                .as_deref()
                .and_then(|w| outrider_view::symbol_id::parse_wire(w).ok())
                .and_then(|id| active_layout.rects.get(&id).copied());
            let focus_rect = if self.focus.current != active_tree.root.id {
                active_layout.rects.get(&self.focus.current).copied()
            } else {
                None
            };
            let target = if let Some(r) = declared_frame {
                camera::frame_rect(r, vw, vh, camera::FRAME_FRACTION, min_zoom, camera::MAX_ZOOM)
            } else if let Some(r) = declared_focus {
                camera::frame_rect(r, vw, vh, camera::FOCUS_FRACTION, min_zoom, camera::MAX_ZOOM)
            } else if let Some(r) = focus_rect {
                camera::frame_rect(r, vw, vh, camera::FOCUS_FRACTION, min_zoom, camera::MAX_ZOOM)
            } else {
                home
            };
            // Snap on scaffold change (layout is discontinuous); tween when
            // only the tab's framing changed within the same layout.
            if scaffold_changed {
                self.camera = Some(target);
                self.tween = None;
                target
            } else {
                // Inline start_tween: `resolved` still borrows the resolver.
                let tw = match self.tween.take() {
                    Some((tw, started)) => tw.retarget(started.elapsed().as_secs_f64(), target),
                    None => CameraTween::new(camera, target),
                };
                self.camera = Some(tw.from);
                self.tween = Some((tw, std::time::Instant::now()));
                camera
            }
        } else {
            camera
        };

        // Tour step camera: enacted here (not in tour_goto) because frame
        // targets need this frame's resolved sets. A tour move supersedes
        // the tab-switch reframe above when both fire on the same frame.
        // In graph mode the map is narrower by the tour panel: frame into
        // the remaining width so the callout and panel don't cover the target.
        crate::frame_profile::profile_phase!(prof, "scaffold");
        let camera = if let Some(target) = self.pending_tour_camera.take() {
            let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
            let usable_w = (vw - crate::view::tour_panel::PANEL_W as f64).max(vw * 0.5);
            let rect: Option<(Rect, f64)> = match &target {
                outrider_view::spec::StepTarget::Frame(outrider_view::spec::SetRef::Name(name)) => {
                    resolved
                        .sets
                        .get(name)
                        .and_then(|s| {
                            Self::set_union_rect(
                                &mut self.set_rect_cache,
                                name,
                                s,
                                active_layout,
                                self.layout_generation,
                            )
                        })
                        .map(|r| (r, camera::FRAME_FRACTION))
                }
                outrider_view::spec::StepTarget::Frame(_) => None,
                outrider_view::spec::StepTarget::Focus(wire) => {
                    outrider_view::symbol_id::parse_wire(wire)
                        .ok()
                        .and_then(|id| active_layout.rects.get(&id).copied())
                        .map(|r| (r, camera::FOCUS_FRACTION))
                }
                outrider_view::spec::StepTarget::Home(_) => None,
            };
            let to = match rect {
                Some((r, frac)) => {
                    // Frame within the left `usable_w` pixels, then shift the
                    // center so the rect sits in that band.
                    let mut c = camera::frame_rect(r, usable_w, vh, frac, min_zoom, camera::MAX_ZOOM);
                    // frame_rect centers on `usable_w/2`; the real viewport is
                    // `vw` wide, so shift world-center right by the slack.
                    c.center_x += (vw - usable_w) / 2.0 / c.zoom;
                    c
                }
                None => {
                    let root_rect = active_layout
                        .rects
                        .get(&active_tree.root.id)
                        .copied()
                        .unwrap_or(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 });
                    let mut c = Camera::fit(root_rect, usable_w, vh);
                    c.center_x += (vw - usable_w) / 2.0 / c.zoom;
                    c
                }
            };
            if scaffold_changed {
                // The layout is discontinuous (new tab / graph rebuilt):
                // snap rather than tween from unrelated coordinates.
                self.camera = Some(to);
                self.tween = None;
                self.home_zoom = Camera::fit(
                    active_layout
                        .rects
                        .get(&active_tree.root.id)
                        .copied()
                        .unwrap_or(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }),
                    vw,
                    vh,
                )
                .zoom;
                to
            } else {
                let tw = match self.tween.take() {
                    Some((tw, started)) => tw.retarget(started.elapsed().as_secs_f64(), to),
                    None => CameraTween::new(camera, to),
                };
                self.camera = Some(tw.from);
                self.tween = Some((tw, std::time::Instant::now()));
                camera
            }
        } else {
            camera
        };

        let cg_highlight_lines: Option<std::ops::Range<usize>> =
            self.call_graph.as_ref().and_then(|mode| {
                let site = match &mode.selection {
                    CallGraphSelection::Callee(i) => {
                        let g = mode.callee_groups.get(*i)?;
                        g.edges[g.active].call_site.as_ref()?
                    }
                    _ => return None,
                };
                let rel = crate::buffers::BufferManager::file_path_of(&mode.center.qualified_path)
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

        crate::frame_profile::profile_phase!(prof, "tour_cam");
        // Buffers materialized on the worker since last frame become
        // visible now, so a leaf that painted bars while waiting gets its
        // text this frame.
        let buffers_arrived = self.buffers.poll();
        if let Some(textures) = self.textures.as_mut() {
            textures.begin_visibility_frame();
        }
        crate::frame_profile::profile_phase!(prof, "begin_vis");
        // In graph mode boxes are text-only UML nodes: report no thumbnails
        // so member subtrees are never pruned behind a code texture.
        let pre = world::PreorderRects::for_generation(
            self.preorder_rects.take(),
            active_tree,
            active_layout,
            self.layout_generation,
        );
        let items = world::visible_nodes_cached(active_tree, &pre, &camera, vw, vh, |id| {
            !graph_mode
                && self
                    .textures
                    .as_ref()
                    .is_some_and(|textures| textures.has_image(id))
        });
        self.preorder_rects = Some(pre);
        crate::frame_profile::profile_phase!(prof, "visible_nodes");
        let n_items = items.len();
        let n_dots = if prof.is_some() {
            items
                .iter()
                .filter(|i| matches!(i.draw, Draw::Leaf(LeafDraw::Dot) | Draw::Container(Rung::Dot)))
                .count()
        } else {
            0
        };
        let mut out = Vec::with_capacity(items.len());
        let mut focused_paint_idx = None;
        let mut header_stack: Vec<(u8, f64)> = Vec::new();
        // Levels of Label-rung containers on the current DFS path: a
        // Label-rung child (or Label-tier leaf) of a Label-rung container
        // draws no name — both names would be centered in nearly the same
        // box and collide; the parent's coarser name wins at this zoom.
        let mut label_stack: Vec<u8> = Vec::new();
        let mut panel_doc: Option<(Vec<outrider_view::layers::notes::ResolvedNote>, f32, f32, f32, f32)> = None;
        for item in items {
            while let Some(&(lvl, _)) = header_stack.last() {
                if lvl >= item.level {
                    header_stack.pop();
                } else {
                    break;
                }
            }
            let ancestor_stack_bottom = header_stack
                .last()
                .map(|&(_, bottom)| bottom)
                .unwrap_or(item.px.y);
            while label_stack.last().is_some_and(|&lvl| lvl >= item.level) {
                label_stack.pop();
            }
            let parent_is_label = label_stack.last() == Some(&item.level.saturating_sub(1))
                && item.level > 0;
            if matches!(item.draw, Draw::Container(Rung::Label)) {
                label_stack.push(item.level);
            }
            let is_leaf = matches!(item.draw, Draw::Leaf(_));
            let is_focused = item.node.id == focus_id;

            // Fast path: a Dot is a sub-label fill with no text, texture,
            // note, or badge — at a wide zoom thousands of them dominate the
            // item list, so build their PaintItem with the minimum lookups.
            if matches!(item.draw, Draw::Leaf(LeafDraw::Dot) | Draw::Container(Rung::Dot))
                && !is_focused
            {
                let Some((clip_y, clip_h)) =
                    descendant_paint_clip(item.px.y, item.px.h, ancestor_stack_bottom)
                else {
                    continue;
                };
                let box_kind = theme::node_box_kind(is_leaf, &item.node.id.kind);
                let tint = theme::node_box_tint(item.node);
                let base_fill = theme::box_fill(box_kind, item.level, tint);
                let light = ov.light(&item.node.id);
                let effective_fill = if item.level == 0 {
                    base_fill
                } else {
                    ov.fill(&item.node.id).unwrap_or(base_fill)
                };
                // Never a flat box where code can be shown: a resident
                // image (real text, GPU-scaled) if there is one — no load or
                // bake is queued at this tier — else line bars for a leaf.
                let mut tex: Option<TexQuad> = None;
                let mut bars: Option<BarStrip> = None;
                if item.full_h >= MIN_BAR_BOX_PX && !graph_mode {
                    if is_leaf {
                        let (tx, ty, tw, th) =
                            leaf_tex_rect(item.node, item.left, item.top, item.full_h);
                        if let Some(img) = self
                            .textures
                            .as_mut()
                            .and_then(|t| t.peek_image(&item.node.id))
                        {
                            tex = Some(TexQuad {
                                x: tx as f32,
                                y: ty as f32,
                                w: tw as f32,
                                h: th as f32,
                                image: img,
                            });
                        } else if item.node.measure > 0 {
                            let rows = self.line_profiles.get(&item.node.id);
                            if rows.is_none() {
                                self.line_profiles
                                    .request(&item.node.id, item.label_w * item.full_h);
                            }
                            bars = Some(leaf_bar_strip(
                                item.node,
                                item.left,
                                item.top,
                                item.full_h,
                                rows,
                            ));
                        }
                    } else if !item.node.children.is_empty() {
                        if let Some(img) = self
                            .textures
                            .as_mut()
                            .and_then(|t| t.peek_image(&item.node.id))
                        {
                            tex = Some(TexQuad {
                                x: item.left as f32,
                                y: item.top as f32,
                                w: item.label_w as f32,
                                h: item.full_h as f32,
                                image: img,
                            });
                        }
                    }
                }
                out.push(PaintItem {
                    x: item.px.x as f32,
                    y: item.px.y as f32,
                    w: item.px.w as f32,
                    h: item.px.h as f32,
                    clip_y: clip_y as f32,
                    clip_h: clip_h as f32,
                    fill: theme::dim_toward(effective_fill, light),
                    border: theme::dim_toward(theme::border_for(effective_fill), light),
                    stripe: ov.stripe(&item.node.id).map(|c| theme::dim_toward(c, light)),
                    focused: false,
                    deferred_overlay: false,
                    neighbor: ov.is_neighbor(&item.node.id),
                    light,
                    body_font_px: FONT_PX as f32,
                    header_bg_h: 0.0,
                    header_bg_y: item.px.y as f32,
                    body_opacity: light,
                    tex_opacity: light,
                    name: None,
                    body: Vec::new(),
                    tex,
                    bars,
                    badge: None,
                });
                continue;
            }
            let box_kind = theme::node_box_kind(is_leaf, &item.node.id.kind);
            let tint = theme::node_box_tint(item.node);
            let fill = theme::box_fill(box_kind, item.level, tint);
            let mut body_font_px = FONT_PX as f32;
            let mut header_bg_h = 0.0f32;
            let mut header_bg_y = item.px.y as f32;
            let mut body_opacity = 1.0f32;
            let mut tex_opacity = 1.0f32;
            let mut name = None;
            let mut body = Vec::new();
            let mut tex: Option<TexQuad> = None;
            let mut bars: Option<BarStrip> = None;
            let mut focused_extra_h = 0.0f64;
            let mut expanded_w = 0.0f32;
            match item.draw {
                Draw::Container(rung) => {
                    if let Some(header) = container_header_layout(
                        rung,
                        item.px.y,
                        item.px.h,
                        ancestor_stack_bottom,
                        camera.zoom,
                    ) {
                        name = if rung == Rung::Label && parent_is_label {
                            None
                        } else {
                            Self::pinned_name(
                                &item,
                                rung == Rung::Label,
                                header.pin_y,
                                rung != Rung::Label,
                            )
                        };
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
                        && !graph_mode
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
                    if tier != LeafDraw::Dot
                        && item.px.h >= 14.0
                        && content::leaf_has_header(item.node)
                        && !(tier == LeafDraw::Label && parent_is_label)
                    {
                        let pin_y = if item.px.y <= 0.0 {
                            item.px
                                .y
                                .max(HEADER_TOP_INSET.min(item.px.y + item.px.h - 14.0))
                        } else {
                            item.px.y
                        };
                        name = Self::pinned_name(&item, false, pin_y, false);
                    }
                    // The tier already encodes the font/width gates
                    // (`world::leaf_draw`); a second, stricter gate here
                    // would send legible short members to the texture path.
                    let use_text = tier == LeafDraw::Text;
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
                                // Field-level borrows only: `items` still
                                // borrows the tree.
                                let shape = world::TreeShape::for_generation(
                                    self.tree_shape.take(),
                                    &self.tree,
                                    self.tree_generation,
                                );
                                for id in shape.ancestor_ids(&self.tree, &item.node.id) {
                                    if let Some(textures) = self.textures.as_mut() {
                                        textures.invalidate(&id);
                                    }
                                }
                                self.tree_shape = Some(shape);
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
                            // Texture not resident (queued, loading, baking
                            // or evicted): draw the code as line bars now
                            // rather than leaving the body blank.
                            if tex.is_none() && item.node.measure > 0 {
                                let rows = self.line_profiles.get(&item.node.id);
                                if rows.is_none() {
                                    self.line_profiles
                                        .request(&item.node.id, item.label_w * item.full_h);
                                }
                                bars = Some(leaf_bar_strip(
                                    item.node,
                                    item.left,
                                    item.top,
                                    item.full_h,
                                    rows,
                                ));
                            }
                        }
                    }
                }
            }
            let is_hovered = self.hover_id.as_ref() == Some(&item.node.id);
            let light = ov.light(&item.node.id);
            body_opacity *= light;
            tex_opacity *= light;
            let paint_w = if is_focused && is_leaf && expanded_w > 0.0 {
                expanded_w
            } else {
                item.px.w as f32
            };
            let paint_h = if is_focused && is_leaf && expanded_w > 0.0 {
                (item.full_h + focused_extra_h) as f32
            } else {
                item.px.h as f32
            };
            let Some((clip_y, clip_h)) =
                descendant_paint_clip(item.px.y, f64::from(paint_h), ancestor_stack_bottom)
            else {
                continue;
            };
            {
                let notes = ov.notes(&item.node.id);
                let rung_ok = crate::view::note_pass::note_rung_ok(&item.draw);
                if rung_ok && !notes.is_empty() {
                    let panel_notes: Vec<_> = notes
                        .iter()
                        .filter(|n| n.range.is_none() && n.lines.is_none())
                        .cloned()
                        .collect();
                    if !panel_notes.is_empty()
                        && (is_hovered || (is_focused && panel_doc.is_none()))
                    {
                        panel_doc = Some((
                            panel_notes,
                            item.px.x as f32,
                            item.px.y as f32,
                            paint_w,
                            paint_h,
                        ));
                    }
                }
            }
            // The root container is the canvas, not a datum: a metric colour
            // on it carries no information and washes out everything inside.
            // Keep it neutral in both treemap and graph modes.
            let effective_fill = if item.level == 0 {
                fill
            } else {
                ov.fill(&item.node.id).unwrap_or(fill)
            };
            if let Some(op) = ov.opacity(&item.node.id) {
                body_opacity *= op;
                tex_opacity *= op;
            }
            out.push(PaintItem {
                x: item.px.x as f32,
                y: item.px.y as f32,
                w: paint_w,
                h: paint_h,
                clip_y: clip_y as f32,
                clip_h: clip_h as f32,
                fill: theme::dim_toward(effective_fill, light),
                border: theme::dim_toward(theme::border_for(effective_fill), light),
                stripe: ov.stripe(&item.node.id).map(|c| theme::dim_toward(c, light)),
                focused: ov.is_focus_ring(&item.node.id),
                deferred_overlay: defer_leaf_to_overlay(is_focused, is_leaf),
                neighbor: ov.is_neighbor(&item.node.id),
                light,
                body_font_px,
                header_bg_h,
                header_bg_y,
                body_opacity,
                tex_opacity,
                name,
                body,
                tex,
                bars,
                badge: ov.badge(&item.node.id),
            });
            if is_focused && is_leaf && expanded_w > 0.0 {
                focused_paint_idx = Some(out.len() - 1);
            }
        }
        if let Some(index) = focused_paint_idx {
            let focused = out.remove(index);
            out.push(focused);
        }
        crate::frame_profile::profile_phase!(prof, "items");
        let n_bars = if prof.is_some() {
            out.iter().filter(|i| i.bars.is_some()).count()
        } else {
            0
        };
        // Scan source files for the line-bar profiles this frame asked for,
        // largest on-screen area first, within a wall-clock slice.
        self.bars_pending = false;
        if self.line_profiles.has_work() {
            let shape = world::TreeShape::for_generation(
                self.tree_shape.take(),
                &self.tree,
                self.tree_generation,
            );
            let tree = &self.tree;
            let budget = if self.tween.is_some() {
                crate::line_bars::SCAN_BUDGET_TWEEN
            } else {
                crate::line_bars::SCAN_BUDGET_IDLE
            };
            let scanned = self.line_profiles.process(budget, &self.file_symbols, &|id| {
                shape.node(tree, id).map(|n| n.measure)
            });
            self.tree_shape = Some(shape);
            // Items drew placeholder bars this frame: repaint with real
            // rows, and keep going while requests are still outstanding.
            self.bars_pending = scanned > 0;
        }
        // Same for source buffers still materializing in the background.
        self.bars_pending |= buffers_arrived > 0 || self.buffers.has_pending();
        crate::frame_profile::profile_phase!(prof, "line_bars");
        let doc_panel = panel_doc.and_then(|(notes, fx, fy, fw, fh)| {
            crate::view::note_pass::build_doc_panel(&notes, fx, fy, fw, fh)
        });
        self.bake_pending = if let Some(textures) = self.textures.as_mut() {
            if textures.has_queued() && !textures.has_bake_work() {
                // Only disk bookkeeping outstanding: pump it without the
                // per-frame index and closures a bake batch would need.
                // This must not bake: a disk miss collected here lands in
                // the bake queue for the next frame.
                textures.pump_disk()
            } else if textures.has_queued() {
                let shape = world::TreeShape::for_generation(
                    self.tree_shape.take(),
                    &self.tree,
                    self.tree_generation,
                );
                let tree = &self.tree;
                crate::frame_profile::profile_phase!(prof, "tex_index");
                let direct_child_bytes: HashMap<_, _> = textures
                    .next_request_ids()
                    .into_iter()
                    .filter_map(|id| {
                        let node = shape.node(tree, &id)?;
                        (!content::is_leaf_item(node))
                            .then(|| (id, textures.direct_child_bytes(node)))
                    })
                    .collect();
                let buffers = &mut self.buffers;
                let file_symbols = &self.file_symbols;
                let layout = &self.layout;
                // Leaves whose source is still materializing on the worker:
                // put back on the queue after the batch rather than caching
                // an empty texture.
                let deferred: std::cell::RefCell<Vec<(outrider_index::SymbolId, f64)>> =
                    std::cell::RefCell::new(Vec::new());
                // Leaves whose lines were handed to the bake worker this
                // batch: marked in flight after the batch returns.
                let submitted: std::cell::RefCell<Vec<outrider_index::SymbolId>> =
                    std::cell::RefCell::new(Vec::new());
                let bake_tx = textures.bake_sender();
                // Bound main-thread bake time per frame: a tween frame gets a
                // slice small enough to keep 60fps; a static frame can spend
                // more since nothing else is moving.
                let budget = if self.tween.is_some() {
                    rasterize::BAKE_BUDGET_TWEEN
                } else {
                    rasterize::BAKE_BUDGET_IDLE
                };
                let pending = textures.process_requests_grouped_within(
                    budget,
                    |id| {
                        shape
                            .node(tree, id)
                            .filter(|node| content::is_leaf_item(node))
                            .map(|_| BufferManager::file_path_of(&id.qualified_path).to_string())
                    },
                    |id, rasterizer| {
                        let node = shape.node(tree, id)?;
                        if !content::is_leaf_item(node) {
                            let rect = layout.rects.get(id)?;
                            let level = shape.depth(id).unwrap_or(0) as u8;
                            let children = direct_child_bytes.get(id);
                            // Composite off-thread when the worker is up: a
                            // 1024px thumbnail of a big folder is 10–15ms.
                            if let Some(tx) = &bake_tx {
                                let job = rasterize::ContainerBake {
                                    node: node.clone(),
                                    rect: *rect,
                                    rects: rasterize::subtree_rects(node, layout),
                                    level,
                                    child_tex: children.cloned().unwrap_or_default(),
                                };
                                if tx.send((id.clone(), rasterize::BakeJob::Container(job))).is_ok() {
                                    submitted.borrow_mut().push(id.clone());
                                    return None;
                                }
                            }
                            let child_tex = |cid: &outrider_index::SymbolId| {
                                children.and_then(|c| c.get(cid)).cloned()
                            };
                            return Some(rasterize::bake_container(
                                node, *rect, &layout.rects, level, &child_tex,
                            ));
                        }
                        let rel = BufferManager::file_path_of(&id.qualified_path).to_string();
                        let syms = file_symbols.get(&rel).map(|v| v.as_slice()).unwrap_or(&[]);
                        let Some(m) = buffers.get(&rel, syms) else {
                            if buffers.is_pending(&rel) {
                                let area = layout
                                    .rects
                                    .get(id)
                                    .map_or(1.0, |r| r.w * r.h);
                                deferred.borrow_mut().push((id.clone(), area));
                            } else {
                                crate::frame_profile::debug_log(|| {
                                    format!("bake: no buffer for {rel}")
                                });
                            }
                            return None;
                        };
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
                            return None;
                        }
                        // Rasterize off-thread when the worker is up; the
                        // leaf keeps its bars until the texture lands.
                        let lines = match &bake_tx {
                            Some(tx) => match tx.send((id.clone(), rasterize::BakeJob::Leaf(lines))) {
                                Ok(()) => {
                                    submitted.borrow_mut().push(id.clone());
                                    return None;
                                }
                                Err(std::sync::mpsc::SendError((_, rasterize::BakeJob::Leaf(lines)))) => lines,
                                Err(_) => return None,
                            },
                            None => lines,
                        };
                        Some(rasterizer.bake(&lines))
                    },
                );
                self.tree_shape = Some(shape);
                for id in submitted.into_inner() {
                    textures.mark_bake_inflight(id);
                }
                for (id, area) in deferred.into_inner() {
                    textures.requeue(id, area);
                }
                pending || textures.has_queued()
            } else {
                false
            }
        } else {
            false
        };
        crate::frame_profile::profile_phase!(prof, "textures");
        let cg_scrim = self.call_graph.is_some();
        let edge_frame = crate::view::edge_pass::aggregate(
            &resolved.edges,
            active_layout,
            &camera,
            vw,
            vh,
        );
        crate::frame_profile::profile_phase!(prof, "edges");
        // Guided-tour callout: narration anchored to the live step's target.
        let live = self.tour.live_target();
        let tour_step_rect: Option<Rect> = match (self.tour.step, &live) {
            (Some(si), Some((target, _))) => {
                // Key includes the part so moving within a step recomputes.
                let key = (
                    si * 1000 + self.tour.part.map(|p| p + 1).unwrap_or(0),
                    active_layout.rects.len(),
                    self.view_tabs.active_index(),
                );
                match &self.tour_target_cache {
                    Some((k, r)) if *k == key => *r,
                    _ => {
                        let rect = match target {
                            outrider_view::spec::StepTarget::Frame(
                                outrider_view::spec::SetRef::Name(n),
                            ) => resolved.sets.get(n).and_then(|s| {
                                Self::set_union_rect(
                                    &mut self.set_rect_cache,
                                    n,
                                    s,
                                    active_layout,
                                    self.layout_generation,
                                )
                            }),
                            outrider_view::spec::StepTarget::Frame(_) => None,
                            outrider_view::spec::StepTarget::Focus(w) => {
                                outrider_view::symbol_id::parse_wire(w)
                                    .ok()
                                    .and_then(|id| active_layout.rects.get(&id).copied())
                            }
                            outrider_view::spec::StepTarget::Home(_) => {
                                active_layout.rects.get(&active_tree.root.id).copied()
                            }
                        };
                        self.tour_target_cache = Some((key, rect));
                        rect
                    }
                }
            }
            _ => {
                self.tour_target_cache = None;
                None
            }
        };
        let tour_callout = self.tour.step.and_then(|si| {
            let (_, note) = live.as_ref()?;
            let note = note.as_deref().unwrap_or("");
            let r = tour_step_rect?;
            let (sx, sy) = camera.world_to_screen(r.x, r.y, vw, vh);
            let target = (
                sx as f32,
                sy as f32,
                (r.w * camera.zoom) as f32,
                (r.h * camera.zoom) as f32,
            );
            let total = self.tour.steps.len();
            let label = match self.tour.part {
                Some(p) => format!(
                    "Step {} of {} \u{00B7} {}/{}",
                    si + 1,
                    total,
                    p + 1,
                    self.tour.part_count()
                ),
                None if self.tour.part_count() > 0 => format!(
                    "Step {} of {} \u{00B7} overview ({} parts)",
                    si + 1,
                    total,
                    self.tour.part_count()
                ),
                None => format!("Step {} of {}", si + 1, total),
            };
            crate::view::note_pass::build_tour_callout(
                crate::view::tour::headline(note),
                crate::view::tour::body(note),
                target,
                (vw - crate::view::tour_panel::PANEL_W as f64 - 16.0).max(vw * 0.5) as f32,
                vh as f32,
                &label,
            )
        });
        crate::frame_profile::profile_phase!(prof, "callout");
        if let Some(p) = prof {
            let mut slow: Vec<&(String, u128)> = self
                .view_resolver
                .last_timings
                .iter()
                .filter(|(n, us)| *us > 500 || n.starts_with("maskmiss"))
                .collect();
            slow.sort_by(|a, b| b.1.cmp(&a.1));
            let slow: Vec<String> = slow
                .into_iter()
                .take(12)
                .map(|(n, us)| format!("{n}={us}us"))
                .collect();
            p.finish(&format!(
                "items={n_items} dots={n_dots} bars={n_bars} profiles={} rects={} dirty={:?} graph={} lt={} tween={} slow[{}]",
                self.line_profiles.len(),
                active_layout.rects.len(),
                dirty,
                graph_mode,
                self.layout_transition.is_some(),
                self.tween.is_some(),
                slow.join(" ")
            ));
        }
        let (tour_callout, callout_minimized) = match tour_callout {
            Some((co, minimized)) => (Some(co), minimized),
            None => (None, false),
        };
        self.tour_callout_minimized = callout_minimized;
        self.tour_callout_hit = tour_callout
            .as_ref()
            .map(|co| ((co.x, co.y, co.w, co.h), co.anchor));
        crate::paint_model::PaintFrame {
            items: out,
            doc_panel,
            cg_scrim,
            edges: edge_frame,
            tour_callout,
        }
    }

    /// Find a node in the tree by its ID (recursive depth-first search).
    /// Returns a reference to the node if found, None otherwise.
    fn find_node<'a>(root: &'a SymbolNode, id: &SymbolId) -> Option<&'a SymbolNode> {
        if root.id == *id {
            return Some(root);
        }
        root.children.iter().find_map(|c| Self::find_node(c, id))
    }



    fn render_panels(&self, map_w: f64) -> Option<gpui::Div> {
        let resolved = self.view_resolver.current()?;
        if self.panels.open.is_empty() {
            return None;
        }

        let mut container = div().absolute().top_0().left_0().size_full();

        for (inst_idx, inst) in self.panels.open.iter().enumerate() {
            let rp = resolved.panels.iter().find(|rp| match &rp.id {
                Some(rp_id) => inst.id == *rp_id,
                None => inst.layer == rp.layer,
            });
            let rp = match rp {
                Some(rp) => rp,
                None => continue,
            };

            match rp.dock {
                outrider_view::spec::Dock::Float => {
                    container = container.child(self.render_float_panel(
                        inst, rp, inst_idx, map_w,
                    ));
                }
                _ => {
                    // Docked panels (call-graph) are rendered by render_call_graph for now.
                }
            }
        }

        Some(container)
    }

    fn render_float_panel(
        &self,
        inst: &crate::view::panel_view::PanelInstance,
        rp: &outrider_view::layers::panel::ResolvedPanel,
        _inst_idx: usize,
        map_w: f64,
    ) -> gpui::Div {
        use crate::view::panel_view::{PALETTE_W, PREVIEW_GAP, PREVIEW_W};

        let effective_id = if !rp.rows.is_empty() {
            let active_alt = inst.group_active.get(inst.selection).copied().unwrap_or(0);
            Some(crate::view::panel_view::effective_id(
                &rp.rows[inst.selection.min(rp.rows.len() - 1)],
                active_alt,
            ))
        } else {
            None
        };

        let selected_node = effective_id.and_then(|id| Self::find_node(&self.tree.root, id));

        let has_preview = inst.preview
            && selected_node
                .is_some_and(|n| n.signature.is_some() || n.doc.is_some() || n.churn_count > 0);

        let total_w = if has_preview {
            PALETTE_W + PREVIEW_GAP + PREVIEW_W
        } else {
            PALETTE_W
        };
        let left_offset = ((map_w as f32 - total_w) / 2.0).max(0.0);

        let title = rp
            .title
            .as_deref()
            .unwrap_or("Panel");

        let query_text = inst
            .query
            .as_ref()
            .map(|q| format!("[{title}] {q}│"))
            .unwrap_or_else(|| title.to_string());

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
                    .child(query_text),
            )
            .children(rp.rows.iter().enumerate().map(|(i, row)| {
                let selected = i == inst.selection;
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
                    .child(format!("{}  {}", row.label, row.sublabel))
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

            preview = preview.child(
                div()
                    .text_size(px(12.0))
                    .font_family(theme::FONT_FAMILY)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .pb(px(4.0))
                    .child(node.id.kind.label().to_uppercase()),
            );

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
            .gap(px(PREVIEW_GAP))
            .child(list_div)
            .children(preview_div)
    }

    fn on_right_press(
        &mut self,
        e: &gpui::MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.map_interaction_enabled() || self.call_graph.is_some() {
            return;
        }
        let Some(cam) = self.camera else { return };
        let (vw, vh) = Self::map_viewport(window);
        let (hit_tree, hit_layout) = match &self.graph_scaffold {
            Some(s) => (&s.tree, &s.layout),
            None => (&self.tree, &self.layout),
        };
        let items = world::visible_nodes(hit_tree, hit_layout, &cam, vw, vh, |id| {
            self.graph_scaffold.is_none()
                && self
                    .textures
                    .as_ref()
                    .is_some_and(|textures| textures.contains(id))
        });
        let (mx, my) = (f64::from(e.position.x), f64::from(e.position.y));
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
        if !self.map_interaction_enabled() || self.call_graph.is_some() {
            return;
        }
        self.drag_last = None;
        if self.context_menu.is_some() {
            self.context_menu = None;
            cx.notify();
            return;
        }
        let Some(origin) = self.press_origin.take() else {
            return;
        };
        // Click on the tour callout (or its anchor dot): act on the item the
        // note references — select it and zoom the camera onto it.
        if self.tour.is_active() {
            if let Some(((cx_, cy_, cw, ch), (ax, ay))) = self.tour_callout_hit {
                let (mx, my) = (f64::from(e.position.x) as f32, f64::from(e.position.y) as f32);
                let in_card =
                    mx >= cx_ && mx <= cx_ + cw && my >= cy_ && my <= cy_ + ch;
                let near_anchor = (mx - ax).abs() <= 10.0 && (my - ay).abs() <= 10.0;
                if in_card || near_anchor {
                    let (vw, vh) = Self::map_viewport(window);
                    self.tour_zoom_target(vw, vh);
                    cx.notify();
                    return;
                }
            }
        }
        let slop = f64::from(e.position.x - origin.x)
            .abs()
            .max(f64::from(e.position.y - origin.y).abs());
        if slop > 4.0 {
            return;
        }
        let Some(cam) = self.camera else { return };
        let (vw, vh) = Self::map_viewport(window);
        let (hit_tree, hit_layout) = match &self.graph_scaffold {
            Some(s) => (&s.tree, &s.layout),
            None => (&self.tree, &self.layout),
        };
        let items = world::visible_nodes(hit_tree, hit_layout, &cam, vw, vh, |id| {
            self.graph_scaffold.is_none()
                && self
                    .textures
                    .as_ref()
                    .is_some_and(|textures| textures.contains(id))
        });
        let (mx, my) = (f64::from(e.position.x), f64::from(e.position.y));
        let hit = world::hit_test(&items, mx, my).map(|i| i.node.id.clone());
        drop(items);
        if let Some(id) = hit {
            let index = TreeIndex::new(hit_tree);
            if self.focus.set(id, &index) {
                self.view_dirty |= outrider_view::Deps::FOCUS;
                self.nav_history.push(self.focus.current.clone());
            }
            self.maybe_precompute_call_graph();
            self.prefetch_relations();
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, e: &gpui::MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        if !self.map_interaction_enabled() || self.call_graph.is_some() {
            return;
        }
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
            let (hit_tree, hit_layout) = match &self.graph_scaffold {
                Some(s) => (&s.tree, &s.layout),
                None => (&self.tree, &self.layout),
            };
            let items = world::visible_nodes(hit_tree, hit_layout, &cam, vw, vh, |id| {
                self.graph_scaffold.is_none()
                    && self
                        .textures
                        .as_ref()
                        .is_some_and(|textures| textures.contains(id))
            });
            let (mx, my) = (f64::from(e.position.x), f64::from(e.position.y));
            let hit = world::hit_test(&items, mx, my)
                .map(|i| i.node.id.clone());
            if hit != self.hover_id {
                self.hover_id = hit;
                self.view_dirty |= outrider_view::Deps::HOVER;
                cx.notify();
            }
        }
    }

    fn on_scroll(&mut self, e: &gpui::ScrollWheelEvent, window: &Window, cx: &mut Context<Self>) {
        if !self.map_interaction_enabled() || self.call_graph.is_some() {
            return;
        }
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
        if !self.map_interaction_enabled() {
            return;
        }
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
        if self.cmd_palette.open {
            self.on_cmd_palette_key(e, window, cx);
            return;
        }
        // Comment composer captures all keys while open.
        if self.comment_draft.is_some() {
            match e.keystroke.key.as_str() {
                "escape" => self.comment_draft = None,
                "enter" if e.keystroke.modifiers.shift => {
                    if let Some(d) = &mut self.comment_draft {
                        d.input.push('\n');
                    }
                }
                "enter" => self.commit_comment(),
                "backspace" => {
                    if let Some(d) = &mut self.comment_draft {
                        d.input.pop();
                    }
                }
                _ => {
                    if let Some(ch) = e.keystroke.key_char.as_ref().and_then(|s| {
                        let mut chars = s.chars();
                        let c = chars.next()?;
                        if chars.next().is_none() { Some(c) } else { None }
                    }) {
                        if let Some(d) = &mut self.comment_draft {
                            d.input.push(ch);
                        }
                    }
                }
            }
            cx.notify();
            return;
        }
        if self.on_tour_key(e) {
            cx.notify();
            return;
        }
        if self.panels.is_open() {
            self.on_panel_key(e, window, cx);
            return;
        }
        // `c`: comment on the current selection. After the tour handler (so
        // tour keys win nothing here — the tour ignores `c`) and after open
        // panels, whose search inputs need the character.
        let m = &e.keystroke.modifiers;
        if e.keystroke.key.as_str() == "c" && !m.control && !m.alt && !m.platform && !m.shift {
            self.open_comment_composer();
            cx.notify();
            return;
        }
        self.on_nav_key(e, window, cx);
    }

    fn on_panel_key(
        &mut self,
        e: &gpui::KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let resolved = self.view_resolver.current();
        let panels = resolved.map(|r| &r.panels[..]).unwrap_or(&[]);
        let ch = e.keystroke.key_char.as_ref().and_then(|s| {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_none() { Some(c) } else { None }
        });
        let effect = crate::view::panel_view::panel_key(
            &mut self.panels,
            panels,
            e.keystroke.key.as_str(),
            ch,
        );
        match effect {
            crate::view::panel_view::PanelKeyEffect::None => {}
            crate::view::panel_view::PanelKeyEffect::SelectionChanged => {
                self.view_dirty |= outrider_view::Deps::SELECTION;
                cx.notify();
            }
            crate::view::panel_view::PanelKeyEffect::Enter(id) => {
                let index = outrider_index::TreeIndex::new(&self.tree);
                if self.focus.set(id, &index) {
                    self.view_dirty |= outrider_view::Deps::FOCUS;
                    self.nav_history.push(self.focus.current.clone());
                }
                self.maybe_precompute_call_graph();
                self.prefetch_relations();
                let (vw, vh) = Self::map_viewport(window);
                let max_zoom = camera::MAX_ZOOM;
                let min_zoom = (self.home_zoom * 0.5).min(camera::MAX_ZOOM);
                if let Some(to) = self.frame_focus(vw, vh, min_zoom, max_zoom) {
                    self.start_tween(to);
                }
                cx.notify();
            }
            crate::view::panel_view::PanelKeyEffect::Close(layers) => {
                for layer_idx in layers.into_iter().rev() {
                    self.apply_view_command(
                        outrider_view::command::ViewCommand::RemoveLayer(layer_idx),
                    );
                }
                cx.notify();
            }
            crate::view::panel_view::PanelKeyEffect::QueryChanged(_) => {
                self.redefine_palette_set();
                cx.notify();
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
        let (nav_tree, nav_layout) = match &self.graph_scaffold {
            Some(s) => (&s.tree, &s.layout),
            None => (&self.tree, &self.layout),
        };
        let index = TreeIndex::new(nav_tree);
        let target = match e.keystroke.key.as_str() {
            "enter" => {
                if !self.focus.step_in(&index) {
                    return;
                }
                self.view_dirty |= outrider_view::Deps::FOCUS;
                self.nav_history.push(self.focus.current.clone());
                self.maybe_precompute_call_graph();
                self.prefetch_relations();
                self.frame_focus(vw, vh, min_zoom, max_zoom)
            }
            "escape" => {
                if !self.focus.step_out(&index) {
                    return;
                }
                self.view_dirty |= outrider_view::Deps::FOCUS;
                self.nav_history.push(self.focus.current.clone());
                self.maybe_precompute_call_graph();
                self.prefetch_relations();
                self.frame_focus(vw, vh, min_zoom, max_zoom)
            }
            "end" => nav_layout
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
                self.view_dirty |= outrider_view::Deps::FOCUS;
                self.maybe_precompute_call_graph();
                self.prefetch_relations();
                self.frame_focus(vw, vh, min_zoom, max_zoom)
            }
            "right" if e.keystroke.modifiers.alt => {
                let Some(id) = self.nav_history.forward().cloned() else {
                    return;
                };
                self.focus.current = id;
                self.focus.record_visit(&index);
                self.neighbors = None;
                self.view_dirty |= outrider_view::Deps::FOCUS;
                self.maybe_precompute_call_graph();
                self.prefetch_relations();
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
                    focus::spatial_step(&self.focus.current, dir, nav_layout, &index)
                else {
                    return;
                };
                if !self.focus.set(next, &index) {
                    return;
                }
                self.view_dirty |= outrider_view::Deps::FOCUS;
                self.maybe_precompute_call_graph();
                self.prefetch_relations();
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
        self.settings.filter_files = ps.filter_files.clone();
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
        self.settings.max_display_lines = ps.max_display_lines;
    }

    fn hide_folder(&mut self, rel_path: &str) {
        let mut ps = self.load_or_default_project_settings();
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

    fn load_or_default_project_settings(&self) -> ProjectSettings {
        ProjectSettings::load(&self.tree.repo_root).unwrap_or(ProjectSettings {
            filter_extensions: vec![],
            filter_folders: vec![],
            filter_files: vec![],
            max_display_lines: None,
        })
    }

    fn hide_file(&mut self, rel_path: &str) {
        let mut ps = self.load_or_default_project_settings();
        if !ps.filter_files.iter().any(|f| f == rel_path) {
            ps.filter_files.push(rel_path.to_string());
        }
        if let Err(e) = ps.save(&self.tree.repo_root) {
            self.notifications.push(Notification::warning(e));
            return;
        }
        self.merge_project_settings(&ps);
        self.reindex();
    }

    fn hide_extension(&mut self, ext: &str) {
        let mut ps = self.load_or_default_project_settings();
        if !ps.filter_extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            ps.filter_extensions.push(ext.to_string());
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

    fn poll_relations(&mut self) -> bool {
        if self.relations.poll() {
            self.view_dirty |= outrider_view::Deps::RELATIONS;
            true
        } else {
            false
        }
    }

    fn prefetch_relations(&self) {
        let index = TreeIndex::new(&self.tree);
        let ctx = outrider_view::relation::ProviderCtx {
            tree: &self.tree,
            index: &index,
            repo_root: self.tree.repo_root.as_path(),
        };
        self.relations
            .prefetch("calls", &self.focus.current, &ctx);
    }

    fn exit_call_graph(&mut self, window: &Window) {
        if let Some(mode) = self.call_graph.take() {
            let index = TreeIndex::new(&self.tree);
            if self.focus.set(mode.center, &index) {
                self.view_dirty |= outrider_view::Deps::FOCUS;
                self.nav_history.push(self.focus.current.clone());
            }
            self.maybe_precompute_call_graph();
            self.prefetch_relations();
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
                    CallGraphSelection::Caller(i) if *i > 0 => CallGraphSelection::Caller(i - 1),
                    CallGraphSelection::Callee(i) if *i > 0 => CallGraphSelection::Callee(i - 1),
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
                    CallGraphSelection::Caller(i) => mode
                        .caller_groups
                        .get(*i)
                        .map(|g| g.edges[g.active].target.clone()),
                    CallGraphSelection::Callee(i) => mode
                        .callee_groups
                        .get(*i)
                        .map(|g| g.edges[g.active].target.clone()),
                };
                if let Some(new_center) = target {
                    let index = TreeIndex::new(&self.tree);
                    if self.focus.set(new_center.clone(), &index) {
                        self.view_dirty |= outrider_view::Deps::FOCUS;
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
        clear_project_setup_before_load(&mut self.project_setup, &mut self.pre_scanner);
        self.layout_transition = None;
        self.packing_target_layout = None;
        self.drag_last = None;
        self.press_origin = None;
        self.tween = None;
        self.hover_id = None;
        self.close_all_panels();
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

    fn packing_geometry(&mut self) -> PackingGeometryState<'_> {
        PackingGeometryState {
            layout: &mut self.layout,
            transition: &mut self.layout_transition,
            target: &mut self.packing_target_layout,
            camera: &mut self.camera,
            neighbors: &mut self.neighbors,
            hover: &mut self.hover_id,
            progress: &mut self.load_progress,
        }
    }

    fn advance_layout_transition(&mut self, now: Instant) -> bool {
        if self.layout_transition.is_some() {
            self.layout_generation += 1;
        }
        let Some(transition) = self.layout_transition.take() else {
            return false;
        };
        self.layout = transition.sample(now);
        let complete = transition.is_complete(now);
        if !complete {
            self.layout_transition = Some(transition);
        }
        self.camera = None;
        // Recomputing arrow neighbors is a full-layout scan; doing it on
        // every animation frame makes the morph stutter. Keep the stale
        // targets while rects are in flight and rebuild once, at the end.
        if complete {
            self.neighbors = None;
        }
        self.hover_id = None;
        true
    }

    fn map_interaction_enabled(&self) -> bool {
        map_interaction_enabled_for(&self.loader)
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
            LoaderPoll::Preview(preview) => {
                self.install_project_preview(*preview);
                true
            }
            LoaderPoll::Snapshot { generation, layout } => {
                self.apply_packing_snapshot(generation, layout, Instant::now());
                true
            }
            LoaderPoll::Complete { generation, layout } => {
                self.finish_packing(generation, layout);
                true
            }
            LoaderPoll::Failed {
                generation,
                message,
                preview_delivered,
            } => {
                self.fail_packing(generation, message, preview_delivered);
                true
            }
        }
    }

    fn poll_pre_scan(&mut self) -> bool {
        if self.loader.is_loading() {
            self.pre_scanner.cancel();
            return false;
        }
        match self.pre_scanner.poll() {
            PreScanPoll::Idle | PreScanPoll::Scanning => self.pre_scanner.is_scanning(),
            PreScanPoll::Ready(result) => match result {
                Ok(scan) => {
                    let existing = ProjectSettings::load(&self.tree.repo_root);
                    self.project_setup =
                        Some(ProjectSetupDraft::from_pre_scan(scan, existing.as_ref()));
                    self.show_welcome = false;
                    true
                }
                Err(error) => {
                    self.notifications
                        .push(Notification::warning(format!("Pre-scan failed: {error}")));
                    self.start_loading(self.tree.repo_root.clone());
                    true
                }
            },
        }
    }

    fn confirm_project_setup(&mut self) {
        if !project_setup_available_for(self.project_setup.is_some(), &self.loader) {
            return;
        }
        if let Some(draft) = self.project_setup.take() {
            let ps = draft.to_project_settings();
            if let Err(e) = ps.save(&self.tree.repo_root) {
                self.notifications.push(Notification::warning(e));
            }
            self.merge_project_settings(&ps);
            self.start_loading(self.tree.repo_root.clone());
        }
    }

    fn install_project_preview(&mut self, project: ProjectPreview) {
        let ProjectPreview {
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
        self.buffers = BufferManager::with_background_loading(project_root.clone());
        self.line_profiles = crate::line_bars::LineProfiles::new(project_root.clone());
        let root_id = tree.root.id.clone();
        self.focus = Focus::new(root_id.clone());
        self.nav_history = NavigationHistory::new(root_id, 64);
        self.neighbors = None;
        self.hover_id = None;
        self.camera = None;
        self.context_menu = None;
        self.close_all_panels();
        self.tree = tree;
        self.tree_generation += 1;
        self.tree_shape = None;
        self.set_rect_cache.clear();
        self.comments = crate::view::comments::CommentList::load(&self.tree.repo_root);
        self.comment_draft = None;
        self.layout = layout;
        self.layout_generation += 1;
        self.layout_transition = None;
        self.packing_target_layout = Some(self.layout.clone());
        self.view_dirty |= outrider_view::Deps::TREE;
        self.view_resolver.invalidate_all();
        self.metrics = outrider_view::metric::MetricRegistry::builtin();
        self.relations =
            outrider_view::relation::RelationRegistry::builtin(Arc::new(self.tree.clone()));
        self.partitions = outrider_view::partition::PartitionRegistry::builtin(&self.tree);
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

    fn apply_packing_snapshot(&mut self, generation: u64, layout: PackLayout, now: Instant) {
        if !self.loader.accepts(generation) {
            return;
        }
        self.layout_generation += 1;
        self.packing_geometry().apply_snapshot(layout, now);
    }

    fn finish_packing(&mut self, generation: u64, layout: PackLayout) {
        if !self.loader.accepts(generation) {
            return;
        }
        self.layout_generation += 1;
        self.packing_geometry().finish(layout);
        // Final geometry is in: an authored view's declared framing can now
        // be honoured against real rects (earlier passes saw the skeleton).
        // A running tour re-enacts its live step instead (the step's target
        // wins over the view-level frame).
        if let Some((target, _)) = self.tour.live_target() {
            self.pending_tour_camera = Some(target);
        } else if self.view_tabs.active_index() != crate::view::tabs::BASE_TAB {
            self.pending_focus_reframe = true;
        }
    }

    fn fail_packing(&mut self, generation: u64, message: String, preview_delivered: bool) {
        if !self.loader.accepts(generation) {
            return;
        }
        if preview_delivered {
            self.packing_geometry().fail_after_preview();
        } else {
            self.layout_transition = None;
            self.packing_target_layout = None;
            self.load_progress = None;
        }
        self.notifications.push(Notification::warning(message));
    }

    /// Build the right-click context menu popup positioned at the click site.
    /// Returns None if no context menu is open.
    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.map_interaction_enabled() {
            return None;
        }
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
        let is_file = matches!(target.kind, SymbolKind::File);
        let file_ext = if is_file {
            std::path::Path::new(&rel_path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_string())
        } else {
            None
        };

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

        let menu_div = if is_file {
            let hide_file_path = rel_path.clone();
            let mut menu_div = menu_div
                .child(crate::overlays::context_menu_separator())
                .child(
                    crate::overlays::context_menu_row("ctx-hide-file", "Hide this File")
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.hide_file(&hide_file_path);
                            this.context_menu = None;
                            cx.notify();
                        })),
                );
            if let Some(ext) = file_ext {
                let ext_label = format!("Hide *.{ext} Files");
                let ext_clone = ext.clone();
                menu_div = menu_div.child(
                    crate::overlays::context_menu_row_dynamic("ctx-hide-ext", &ext_label)
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.hide_extension(&ext_clone);
                            this.context_menu = None;
                            cx.notify();
                        })),
                );
            }
            menu_div
        } else {
            menu_div
        };

        Some(menu_div)
    }

    /// Right-hand guided-tour panel: step list, live narration, controls.
    /// The tour block of the right column (header, step list, optional
    /// narration, controls). None when no tour is active.
    fn render_tour_sections(&self, vh: f64, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let step_index = self.tour.step?;
        let steps = &self.tour.steps;
        if steps.is_empty() {
            return None;
        }
        let title = self
            .view_tabs
            .tabs()
            .get(self.tour.origin_tab)
            .map(|t| t.label.clone())
            .unwrap_or_default();
        let mut m =
            crate::view::tour_panel::build_model(&title, steps, step_index, self.tour.part);
        if self.tour_callout_minimized {
            if let Some((_, Some(note))) = self.tour.live_target() {
                m.headline = crate::view::tour::headline(&note).to_string();
                m.paragraphs =
                    crate::view::tour_panel::md_paragraphs(crate::view::tour::body(&note));
            }
        }
        use crate::view::tour_panel::RowState;

        let sans = theme::FONT_FAMILY_SANS;
        let label = |text: String, size: f32, color: u32| {
            div()
                .text_size(px(size))
                .font_family(sans)
                .text_color(rgb(color))
                .child(text)
        };

        // Header: view title + "Step i of n".
        let header = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .pb(px(10.0))
            .border_b_1()
            .border_color(rgb(theme::NARRATION_BORDER))
            .child(label(m.view_title.clone(), 11.0, theme::TEXT_SECONDARY))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(label("Guided tour".into(), 15.0, theme::NARRATION_TEXT))
                    .child(label(
                        format!("{} / {}", m.step_index + 1, m.total),
                        12.0,
                        theme::TEXT_SECONDARY,
                    )),
            );

        // Step list: scrolls inside a bounded height so the controls below
        // are always reachable however many steps/parts a tour has.
        let list_max_h = (vh as f32 - 48.0 - 64.0 - 150.0).max(120.0);
        let mut list = div()
            .id("tour-step-list")
            .flex()
            .flex_col()
            .gap(px(1.0))
            .py(px(8.0))
            .max_h(px(list_max_h))
            .overflow_y_scroll();
        for row in &m.rows {
            let (bg, fg, marker) = match row.state {
                RowState::Done => (rgba(0x00000000), theme::TEXT_SECONDARY, "\u{2713}"),
                RowState::Current => (rgba(0x2a3040ff), theme::NARRATION_TEXT, "\u{25B6}"),
                RowState::Upcoming => (rgba(0x00000000), theme::TEXT_SECONDARY, "\u{00B7}"),
            };
            let i = row.index;
            list = list.child(
                div()
                    .id(("tour-step", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(bg)
                    .cursor_pointer()
                    .hover(|el| el.bg(rgb(0x232a38_u32)))
                    .child(
                        div()
                            .w(px(14.0))
                            .text_size(px(11.0))
                            .font_family(sans)
                            .text_color(rgb(fg))
                            .child(marker),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_family(sans)
                                    .text_color(rgb(fg))
                                    .child(format!("{}. {}", i + 1, row.title)),
                            )
                            .children(row.tab.clone().map(|t| {
                                div()
                                    .text_size(px(10.0))
                                    .font_family(sans)
                                    .text_color(rgb(theme::DOC_COLOR))
                                    .child(format!("\u{21B3} {t}"))
                            })),
                    )
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.press_origin = None;
                        this.drag_last = None;
                        this.tour_goto(i);
                        cx.notify();
                    })),
            );
            // Sub-steps of the live step, indented under it.
            if row.state == RowState::Current {
                for part in &m.parts {
                    let (pbg, pfg, pmark) = if part.current {
                        (rgba(0x2a3040ff), theme::NARRATION_TEXT, "\u{25B8}")
                    } else if part.done {
                        (rgba(0x00000000), theme::TEXT_SECONDARY, "\u{2713}")
                    } else {
                        (rgba(0x00000000), theme::TEXT_SECONDARY, "\u{00B7}")
                    };
                    let pi = part.index;
                    list = list.child(
                        div()
                            .id(("tour-part", pi))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .pl(px(30.0))
                            .pr(px(8.0))
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .bg(pbg)
                            .cursor_pointer()
                            .hover(|el| el.bg(rgb(0x232a38_u32)))
                            .child(
                                div()
                                    .w(px(12.0))
                                    .text_size(px(10.0))
                                    .font_family(sans)
                                    .text_color(rgb(pfg))
                                    .child(pmark),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_family(sans)
                                    .text_color(rgb(pfg))
                                    .child(format!("{}.{} {}", i + 1, pi + 1, part.title)),
                            )
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _e, _w, cx| {
                                this.press_origin = None;
                                this.drag_last = None;
                                this.tour_goto_part(Some(pi));
                                cx.notify();
                            })),
                    );
                }
            }
        }

        // Controls.
        let button = |id: &'static str, text: &'static str, enabled: bool| {
            div()
                .id(id)
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .bg(if enabled { rgb(0x2a3040_u32) } else { rgb(0x1c1f26_u32) })
                .text_size(px(12.0))
                .font_family(sans)
                .text_color(rgb(if enabled { theme::TEXT_PRIMARY } else { 0x55585f }))
                .cursor_pointer()
                .child(text)
        };
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pt(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.0))
                    .child(
                        button("tour-prev", "\u{2190} Prev", m.has_prev)
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _e, _w, cx| {
                                this.press_origin = None;
                                this.tour_prev();
                                cx.notify();
                            })),
                    )
                    .child(
                        button("tour-next", "Next \u{2192}", m.has_next)
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _e, _w, cx| {
                                this.press_origin = None;
                                this.tour_next();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                button("tour-exit", "Exit", true)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.press_origin = None;
                        this.tour_stop();
                        cx.notify();
                    })),
            )
            .child(label("\u{2190} \u{2192} keys \u{00B7} Esc".into(), 10.0, theme::TEXT_SECONDARY));

        // When the in-view card collapsed to a pill (it would have covered
        // the target), the panel carries the narration instead.
        let narration = self.tour_callout_minimized.then(|| {
            let mut n = div()
                .id("tour-panel-narration")
                .flex()
                .flex_col()
                .gap(px(8.0))
                .pt(px(10.0))
                .max_h(px(220.0))
                .overflow_y_scroll()
                .border_t_1()
                .border_color(rgb(theme::NARRATION_BORDER))
                .child(label(m.headline.clone(), 13.5, theme::NARRATION_TEXT));
            for para in &m.paragraphs {
                n = n.child(
                    div()
                        .text_size(px(12.0))
                        .font_family(sans)
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .line_height(px(17.0))
                        .child(para.clone()),
                );
            }
            n
        });
        Some(
            div()
                .flex()
                .flex_col()
                .child(header)
                .child(list)
                .children(narration)
                .child(controls),
        )
    }

    /// The right-hand column: tour navigator (when a tour is active) and
    /// the comment list / composer (when comments exist or one is being
    /// written). None when there is nothing to show.
    fn render_right_column(&self, vh: f64, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let tour_block = self.render_tour_sections(vh, cx);
        let comments_block = self.render_comments_section(tour_block.is_some(), cx);
        if tour_block.is_none() && comments_block.is_none() {
            return None;
        }
        Some(
            div()
                .absolute()
                .top(px(48.0))
                .right(px(8.0))
                .w(px(crate::view::tour_panel::PANEL_W))
                .max_h(px((vh - 64.0).max(200.0) as f32))
                .flex()
                .flex_col()
                .p(px(14.0))
                .rounded(px(6.0))
                .bg(rgba(0x1c1b1ef0))
                .border_1()
                .border_color(rgb(theme::NARRATION_BORDER))
                .shadow_lg()
                .overflow_hidden()
                // Swallow map interaction under the panel.
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .children(tour_block)
                .children(comments_block),
        )
    }

    /// The accumulated comment list with per-row delete, Clear all, and
    /// the Copy-prompt button that turns the list into an agent prompt.
    fn render_comments_section(
        &self,
        below_tour: bool,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        if self.comments.is_empty() {
            return None;
        }
        let sans = theme::FONT_FAMILY_SANS;
        let label = |text: String, size: f32, color: u32| {
            div()
                .text_size(px(size))
                .font_family(sans)
                .text_color(rgb(color))
                .child(text)
        };
        let n = self.comments.comments.len();
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pb(px(6.0))
            .when(below_tour, |el| {
                el.pt(px(12.0)).border_t_1().border_color(rgb(theme::NARRATION_BORDER))
            })
            .child(label(format!("Comments ({n})"), 13.0, theme::NARRATION_TEXT))
            .child(
                div()
                    .id("comments-clear")
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .text_size(px(11.0))
                    .font_family(sans)
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .cursor_pointer()
                    .hover(|el| el.bg(rgb(0x232a38_u32)).text_color(rgb(theme::TEXT_PRIMARY)))
                    .child("Clear all")
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.press_origin = None;
                        this.comments.clear();
                        this.comments.save(&this.tree.repo_root);
                        cx.notify();
                    })),
            );
        let mut list = div()
            .id("comments-list")
            .flex()
            .flex_col()
            .gap(px(2.0))
            .max_h(px(220.0))
            .overflow_y_scroll();
        for (i, c) in self.comments.comments.iter().enumerate() {
            let cid = c.id;
            let target = c.target.clone();
            let clickable = target.is_some();
            let title = if c.target.is_some() {
                c.target_label.clone()
            } else {
                format!("General \u{00B7} {}", c.view)
            };
            list = list.child(
                div()
                    .id(("comment-row", i))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .when(clickable, |el| el.cursor_pointer())
                    .hover(|el| el.bg(rgb(0x232a38_u32)))
                    .child(
                        div()
                            .flex_grow(1.0)
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(label(title, 11.5, theme::NARRATION_TEXT))
                            .children(
                                crate::view::tour_panel::md_paragraphs(&c.text).into_iter().map(
                                    |para| {
                                        div()
                                            .text_size(px(11.5))
                                            .font_family(sans)
                                            .text_color(rgb(theme::TEXT_SECONDARY))
                                            .line_height(px(15.0))
                                            .child(para)
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .id(("comment-del", i))
                            .px(px(5.0))
                            .rounded(px(3.0))
                            .text_size(px(11.0))
                            .font_family(sans)
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .cursor_pointer()
                            .hover(|el| el.text_color(rgb(theme::TEXT_PRIMARY)))
                            .child("\u{2715}")
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _e, _w, cx| {
                                this.press_origin = None;
                                this.comments.remove(cid);
                                this.comments.save(&this.tree.repo_root);
                                cx.notify();
                            })),
                    )
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.press_origin = None;
                        this.drag_last = None;
                        if let Some(wire) = &target {
                            let rect = this.select_wire(wire);
                            if let (Some(r), Some((vw, vh))) = (rect, this.last_viewport) {
                                this.frame_rect_beside_panel(r, vw, vh);
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pt(px(8.0))
            .child(
                div()
                    .id("comments-copy-prompt")
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .bg(rgb(0x2a3040_u32))
                    .text_size(px(12.0))
                    .font_family(sans)
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .cursor_pointer()
                    .hover(|el| el.bg(rgb(0x353d50_u32)))
                    .child("Copy prompt")
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.press_origin = None;
                        let prompt = this.build_comment_prompt();
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(prompt));
                        this.notifications.push(Notification::info(
                            "Prompt copied \u{2014} paste it to your agent.",
                        ));
                        cx.notify();
                    })),
            )
            .child(label("c = comment".into(), 10.0, theme::TEXT_SECONDARY));
        Some(
            div()
                .flex()
                .flex_col()
                .child(header)
                .child(list)
                .child(footer),
        )
    }

    /// The floating comment composer (opened with `c` on a selection).
    /// Anchored beside the selected item's on-screen rect — right of it
    /// when there is room before the panel band, otherwise left — and
    /// re-anchored every frame so it tracks camera motion. Supports
    /// multi-line input (Shift+Enter inserts a newline).
    fn render_comment_composer_overlay(&self, vw: f64, vh: f64) -> Option<gpui::Div> {
        let d = self.comment_draft.as_ref()?;
        const W: f64 = 300.0;
        let camera = self.camera;
        let layout = match &self.graph_scaffold {
            Some(s) => &s.layout,
            None => &self.layout,
        };
        let rect = layout.rects.get(&self.focus.current).copied();
        let (bx, by) = match (rect, camera) {
            (Some(r), Some(c)) => {
                let (sx0, sy0) = c.world_to_screen(r.x, r.y, vw, vh);
                let (sx1, _) = c.world_to_screen(r.x + r.w, r.y + r.h, vw, vh);
                // The tour/comments panel owns the far right edge; prefer
                // the gap right of the item, fall back to its left.
                let panel_edge = vw - f64::from(crate::view::tour_panel::PANEL_W) - 16.0;
                let x = if sx1 + 12.0 + W <= panel_edge {
                    sx1 + 12.0
                } else {
                    (sx0 - 12.0 - W).max(8.0)
                };
                (x.min(panel_edge - W).max(8.0), sy0)
            }
            // General comment or item without a rect: near the top center.
            _ => ((vw - W) / 2.0, 80.0),
        };
        let by = by.clamp(8.0, (vh - 180.0).max(8.0));
        let sans = theme::FONT_FAMILY_SANS;
        let mut input_box = div()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .bg(rgb(0x14161c_u32))
            .border_1()
            .border_color(rgb(0x2a3040_u32))
            .min_h(px(48.0))
            .max_h(px(220.0))
            .flex()
            .flex_col()
            .text_size(px(12.0))
            .font_family(sans)
            .text_color(rgb(theme::TEXT_PRIMARY))
            .line_height(px(16.0));
        let lines: Vec<&str> = d.input.split('\n').collect();
        let last = lines.len().saturating_sub(1);
        for (i, line) in lines.iter().enumerate() {
            let shown = if i == last {
                format!("{line}\u{258F}")
            } else if line.is_empty() {
                "\u{00A0}".to_string()
            } else {
                (*line).to_string()
            };
            input_box = input_box.child(div().child(shown));
        }
        Some(
            div()
                .absolute()
                .left(px(bx as f32))
                .top(px(by as f32))
                .w(px(W as f32))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(10.0))
                .rounded(px(6.0))
                .bg(rgba(0x1c1b1ef0))
                .border_1()
                .border_color(rgb(theme::NARRATION_BORDER))
                .shadow_lg()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_family(sans)
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(format!("Comment on {}", d.target_label)),
                )
                .child(input_box)
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(sans)
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(
                            "Enter to save \u{00B7} Shift+Enter for newline \u{00B7} Esc to cancel",
                        ),
                ),
        )
    }

    /// Top-center view tab strip. Hidden while only the base tab exists.
    fn render_tab_bar(&self, vw: f64, cx: &mut Context<Self>) -> gpui::Div {
        let tabs = self.view_tabs.tabs();
        if tabs.len() < 2 {
            return div();
        }
        let active = self.view_tabs.active_index();
        let mut strip = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .p(px(3.0))
            .rounded(px(6.0))
            .bg(rgba(0x000000a0));
        for (i, tab) in tabs.iter().enumerate() {
            let is_active = i == active;
            let (bg, fg) = if is_active {
                (rgb(0x2a3040_u32), rgb(theme::TEXT_PRIMARY))
            } else {
                (rgba(0x00000000), rgb(theme::TEXT_SECONDARY))
            };
            let hint = if i < 9 { format!("{}", i + 1) } else { String::new() };
            let label = tab.label.clone();
            strip = strip.child(
                div()
                    .id(("view-tab", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(bg)
                    .cursor_pointer()
                    .hover(|el| el.bg(rgb(0x232a38_u32)))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_family(theme::FONT_FAMILY_SANS)
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child(hint),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_family(theme::FONT_FAMILY_SANS)
                            .text_color(fg)
                            .child(label),
                    )
                    // Swallow the press so the map beneath doesn't treat the
                    // tab click as a click-to-focus on whatever box is there.
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.press_origin = None;
                        this.drag_last = None;
                        this.switch_to_tab(i);
                        this.context_menu = None;
                        this.file_menu_open = false;
                        cx.notify();
                    })),
            );
        }
        // Center the strip: absolute at top, full width flex container.
        div()
            .absolute()
            .top(px(8.0))
            .left(px(0.0))
            .w(px(vw as f32))
            .flex()
            .flex_row()
            .justify_center()
            .child(strip)
    }

    fn render_file_menu(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if crate::uses_native_application_menu(std::env::consts::OS) {
            return None;
        }

        let button = div()
            .id("file-menu-button")
            .px(px(12.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .bg(rgb(theme::CODE_BG))
            .text_color(rgb(theme::TEXT_PRIMARY))
            .text_size(px(13.0))
            .font_family(theme::FONT_FAMILY_SANS)
            .cursor_pointer()
            .hover(|element| element.bg(rgb(0x2a3040_u32)))
            .child("File")
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.file_menu_open = !this.file_menu_open;
                this.context_menu = None;
                cx.notify();
            }));

        let popup = self.file_menu_open.then(|| {
            let popup = crate::overlays::context_menu_shell(8.0, 42.0).child(
                crate::overlays::context_menu_row("file-menu-open", "Open Folder...").on_click(
                    cx.listener(|this, _event, window, cx| {
                        this.file_menu_open = false;
                        // Call the work directly. Dispatching the action to
                        // ourselves re-enters this entity while the click's own
                        // update still holds it, which GPUI treats as a
                        // non-unwinding panic and the process aborts — and
                        // deferring does not help, because the deferred closure
                        // is itself run inside an update of this entity.
                        this.prompt_open_folder(cx);
                    }),
                ),
            );
            if self.map_interaction_enabled() {
                popup
                    .child(crate::overlays::context_menu_separator())
                    .child(
                        crate::overlays::context_menu_row(
                            "file-menu-clear-cache",
                            "Clear Project Disk Cache",
                        )
                        .on_click(cx.listener(
                            |this, _event, window, cx| {
                                this.file_menu_open = false;
                                this.clear_disk_cache(cx);
                            },
                        )),
                    )
            } else {
                popup
            }
        });

        Some(
            div()
                .absolute()
                .top(px(8.0))
                .left(px(8.0))
                .child(button)
                .children(popup),
        )
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
        vw: f64,
        vh: f64,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        // Nothing to draw without an open call graph. This check must come
        // before the column geometry below: it indexes the whole tree, and
        // doing that on every frame (as it once did) cost ~24ms per frame
        // on a 30k-node project — the single largest steady-state cost.
        self.call_graph.as_ref()?;
        let col_w = 320.0_f32;
        let col_h = (vh as f32 - 96.0).max(200.0);
        let shape = self.ensure_tree_shape();
        let column_lefts = self.camera.and_then(|camera| {
            let packed = *self.layout.rects.get(&self.focus.current)?;
            let node = shape.node(&self.tree, &self.focus.current)?;
            let expanded_w = if content::is_leaf_item(node) {
                focused_width(max_line_chars(node, &mut self.buffers, &self.file_symbols))
            } else {
                packed.w
            };
            let (focus_left, _) = camera.world_to_screen(packed.x, packed.y, vw, vh);
            let focus_right = focus_left + expanded_w * camera.zoom;
            Some(call_graph_column_lefts(
                focus_left as f32,
                focus_right as f32,
                col_w,
            ))
        });
        self.tree_shape = Some(shape);
        let (callers_left, callees_left) = column_lefts.unwrap_or((12.0, vw as f32 - col_w - 12.0));
        let mode = self.call_graph.as_ref()?;
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
                let name = node
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| snap.raw_name.clone());
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
                let name = node
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| snap.raw_name.clone());
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

        let callers_col = Self::render_cg_column(
            &caller_items,
            "Callers",
            true,
            callers_left,
            col_w,
            col_h,
            loading,
            caller_scroll,
            caller_sel,
        );
        let callees_col = Self::render_cg_column(
            &callee_items,
            "Callees",
            false,
            callees_left,
            col_w,
            col_h,
            loading,
            callee_scroll,
            callee_sel,
        );

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
        left: f32,
        width: f32,
        col_h: f32,
        loading: bool,
        scroll_pos: f32,
        selected_idx: Option<usize>,
    ) -> gpui::Stateful<gpui::Div> {
        let col_id = if is_callers {
            "cg-callers"
        } else {
            "cg-callees"
        };
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
            .left(px(left))
            .w(px(width))
            .h(px(col_h))
            .overflow_hidden()
            .child(header);

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

            let mut name_row = div().flex().flex_row().items_center().gap(px(4.0)).child(
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

    fn render_delete_confirm(&self, map_w: f64, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.map_interaction_enabled() {
            return None;
        }
        let path = self.delete_confirm.as_ref()?;
        let display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let delete_path = path.clone();

        let cancel = crate::overlays::action_button("del-cancel", "Cancel", false).on_click(
            cx.listener(|this, _e, _w, cx| {
                this.delete_confirm = None;
                cx.notify();
            }),
        );
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

    fn render_rename(&self, map_w: f64, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.map_interaction_enabled() {
            return None;
        }
        let state = self.rename_state.as_ref()?;
        let rename_path = state.path.clone();
        let new_name = state.input.clone();

        let cancel = crate::overlays::action_button("ren-cancel", "Cancel", false).on_click(
            cx.listener(|this, _e, _w, cx| {
                this.rename_state = None;
                cx.notify();
            }),
        );
        let confirm = crate::overlays::action_button("ren-confirm", "Rename", true).on_click(
            cx.listener(move |this, _e, _w, cx| {
                let new_path = rename_path.parent().unwrap_or(&rename_path).join(&new_name);
                if let Err(err) = std::fs::rename(&rename_path, &new_path) {
                    this.notifications
                        .push(Notification::warning(format!("Rename failed: {err}")));
                }
                this.rename_state = None;
                this.reindex();
                cx.notify();
            }),
        );

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
                    .child(crate::overlays::settings_input(
                        "ren-input",
                        state.input.clone(),
                        true,
                    ))
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
        use crate::overlays::{
            action_button, category_header, checkbox_row, project_setup_element,
        };
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
            let detail = format!(
                "({cat_count} files · {})",
                project_settings::format_bytes(cat_bytes)
            );
            let is_sel =
                draft.active_panel == SetupPanel::Extensions && draft.ext_cursor == flat_ext_idx;

            let cat_for_expand = *cat;
            let cat_for_toggle = *cat;
            let ext_keys: Vec<String> = exts.iter().map(|(e, _)| (*e).clone()).collect();
            let expand_listener = cx.listener(move |this, _, _, cx| {
                if let Some(d) = &mut this.project_setup {
                    let exp = d
                        .category_expanded
                        .get(&cat_for_expand)
                        .copied()
                        .unwrap_or(false);
                    d.category_expanded.insert(cat_for_expand, !exp);
                }
                cx.notify();
            });
            let toggle_listener = cx.listener(move |this, _, _, cx| {
                if let Some(d) = &mut this.project_setup {
                    let all_on = ext_keys
                        .iter()
                        .all(|e| d.extension_enabled.get(e).copied().unwrap_or(true));
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
                    let detail = format!(
                        "{} · {}",
                        stats.count,
                        project_settings::format_bytes(stats.bytes)
                    );
                    let is_sel = draft.active_panel == SetupPanel::Extensions
                        && draft.ext_cursor == flat_ext_idx;
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
                                let v =
                                    d.extension_enabled.get(&ext_owned).copied().unwrap_or(true);
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
        self.build_folder_rows(
            draft,
            "",
            0,
            &visible_folders,
            &mut flat_idx,
            &mut folder_rows,
            cx,
        );

        let actions = vec![
            action_button("setup-confirm", "Start Indexing", true).on_click(cx.listener(
                |this, _, _, cx| {
                    this.confirm_project_setup();
                    cx.notify();
                },
            )),
            action_button("setup-cancel", "Cancel", false).on_click(cx.listener(
                |this, _, _, cx| {
                    this.project_setup = None;
                    cx.notify();
                },
            )),
        ];

        project_setup_element(
            map_w,
            map_h,
            title,
            stats_line,
            ext_rows,
            folder_rows,
            actions,
        )
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
            let enabled = draft
                .folder_enabled
                .get(&full_path)
                .copied()
                .unwrap_or(true);
            let stats = draft.pre_scan.folders.get(&full_path);
            let detail = if let Some(s) = stats {
                format!(
                    "{} files · {}",
                    s.count,
                    project_settings::format_bytes(s.bytes)
                )
            } else {
                "gitignored".into()
            };
            let is_sel =
                draft.active_panel == SetupPanel::Folders && draft.folder_cursor == *flat_idx;
            let has_kids = draft.folder_has_children(&full_path);
            let expanded = draft
                .folder_expanded
                .get(&full_path)
                .copied()
                .unwrap_or(false);
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
                        let exp = d
                            .folder_expanded
                            .get(&path_for_expand)
                            .copied()
                            .unwrap_or(false);
                        d.folder_expanded.insert(path_for_expand.clone(), !exp);
                    }
                    cx.notify();
                });
                let toggle_listener = cx.listener(move |this, _, _, cx| {
                    if let Some(d) = &mut this.project_setup {
                        let v = d
                            .folder_enabled
                            .get(&path_for_toggle)
                            .copied()
                            .unwrap_or(true);
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
                self.build_folder_rows(
                    draft,
                    &full_path,
                    depth + 1,
                    visible_folders,
                    flat_idx,
                    out,
                    cx,
                );
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

        let mut needs_notify = self.advance_layout_transition(Instant::now());
        needs_notify |= self.poll_loading();
        needs_notify |= self.poll_call_graph(window);
        needs_notify |= self.poll_relations();
        needs_notify |= self.poll_pre_scan();
        needs_notify |= self.poll_view_watch();
        needs_notify |= self.poll_git_watch();
        needs_notify |= self.poll_rpc(window, cx);
        if needs_notify {
            cx.notify();
        }

        let (vw, vh) = Self::map_viewport(window);
        let is_loading = self.loader.is_loading();
        let project_setup_available =
            project_setup_available_for(self.project_setup.is_some(), &self.loader);

        let skip_treemap = project_setup_available;
        let frame = if skip_treemap {
            crate::paint_model::PaintFrame {
                items: Vec::new(),
                doc_panel: None,
                tour_callout: None,
                cg_scrim: false,
                edges: crate::view::edge_pass::EdgeFrame { edges: Vec::new() },
            }
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
        let cg_animating = self
            .call_graph
            .as_ref()
            .is_some_and(|cg| cg.scroll.is_animating());
        let scanning = self.pre_scanner.is_scanning();
        // Outstanding disk-cache traffic (loads in flight, saves pending)
        // has nothing for the main thread to do until the worker answers:
        // poll it from the 50ms pump instead of re-rendering every frame —
        // at a wide zoom each spun frame is a full paint of 10k items.
        let disk_only = self.bake_pending
            && self
                .textures
                .as_ref()
                .is_some_and(|t| !t.has_bake_work());
        if disk_only {
            self.wake.raise();
        }
        let wants_frame = self.tween.is_some()
            || (self.bake_pending && !disk_only)
            || self.bars_pending
            || self.buffers.has_pending()
            || is_loading
            || self.cg_resolver.is_active()
            || cg_animating
            || scanning;
        if let Some(mut p) = crate::frame_profile::FrameProfile::begin() {
            p.phase("overlays");
            let tex_state = self
                .textures
                .as_ref()
                .map(|t| t.queue_state())
                .unwrap_or_default();
            p.finish(&format!(
                "RENDER wants_frame={wants_frame} tween={} bake={} loading={is_loading} cg={} scan={scanning} notify={needs_notify} active={} tex[{tex_state}]",
                self.tween.is_some(),
                self.bake_pending,
                self.cg_resolver.is_active(),
                window.is_window_active(),
            ));
        }
        if wants_frame {
            window.request_animation_frame();
        }
        let mut oprof = crate::frame_profile::FrameProfile::begin();

        // Build the panels overlay (palette + future docked panels).
        let panels_overlay = self.render_panels(vw);
        crate::frame_profile::profile_phase!(oprof, "panels");

        // Build the command palette overlay.
        let cmd_palette_overlay = self.render_command_palette(vw);
        crate::frame_profile::profile_phase!(oprof, "palette");

        // Build the settings overlay (needs cx for click listeners).
        let settings_overlay = self
            .settings_draft
            .is_some()
            .then(|| self.render_settings_window(vw, cx));

        // Build the welcome overlay (needs cx for click listeners).
        let welcome_overlay = self.show_welcome.then(|| self.render_welcome(vw, cx));

        // Build the toolbar overlay.
        let has_overlays = self.panels.has_float()
            || self.settings_draft.is_some()
            || self.show_welcome
            || project_setup_available;
        let toolbar_overlay = (!has_overlays && self.map_interaction_enabled()).then(|| {
            let show_churn = self.settings.show_churn;
            div().absolute().top(px(8.0)).right(px(8.0)).child(
                crate::overlays::toolbar_toggle("churn-toggle", "Git Churn", show_churn).on_click(
                    cx.listener(|this, _event, _window, cx| {
                        this.settings.show_churn = !this.settings.show_churn;
                        this.global_settings.show_churn = this.settings.show_churn;
                        let _ = this.global_settings.save();
                        cx.notify();
                    }),
                ),
            )
        });

        // Build the context menu overlay (needs cx for click listeners).
        let context_menu_overlay = self.render_context_menu(cx);
        let file_menu_overlay = self.render_file_menu(cx);
        crate::frame_profile::profile_phase!(oprof, "menus");
        let tab_bar_overlay =
            (!has_overlays && self.map_interaction_enabled()).then(|| self.render_tab_bar(vw, cx));
        crate::frame_profile::profile_phase!(oprof, "tab_bar");
        let tour_overlay = (!has_overlays && self.map_interaction_enabled())
            .then(|| self.render_right_column(vh, cx))
            .flatten();
        let composer_overlay = (!has_overlays && self.map_interaction_enabled())
            .then(|| self.render_comment_composer_overlay(vw, vh))
            .flatten();

        // Build the call graph overlay.
        crate::frame_profile::profile_phase!(oprof, "tour+composer");
        let call_graph_overlay = self.render_call_graph(vw, vh, cx);
        crate::frame_profile::profile_phase!(oprof, "call_graph");

        // Build the delete-confirmation overlay.
        let delete_overlay = self.render_delete_confirm(vw, cx);

        // Build the rename overlay.
        let rename_overlay = self.render_rename(vw, cx);

        // Build the project setup overlay.
        let project_setup_overlay =
            project_setup_available.then(|| self.render_project_setup(vw, vh, cx));

        // Build the pre-scan loading spinner.
        let pre_scan_overlay = (!is_loading
            && self.pre_scanner.is_scanning()
            && self.project_setup.is_none())
        .then(|| {
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

        crate::frame_profile::profile_phase!(oprof, "misc_overlays");
        window.set_window_title(&self.window_title());
        if let Some(p) = oprof {
            p.finish("OVERLAYS");
        }
        let map = div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(theme::BG))
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &OpenFolder, _w, cx| {
                this.prompt_open_folder(cx);
            }))
            .on_action(cx.listener(|this, _: &ClearDiskCache, _w, cx| {
                this.clear_disk_cache(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSettings, _w, cx| {
                if !this.map_interaction_enabled() {
                    return;
                }
                if this.settings_draft.is_some() {
                    this.settings_draft = None;
                } else {
                    this.settings_draft = Some(SettingsDraft::from_settings(
                        &this.global_settings,
                        &this.tree.repo_root,
                    ));
                    this.close_all_panels();
                    this.show_welcome = false;
                    this.context_menu = None;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleProjectSettings, _w, cx| {
                if !this.map_interaction_enabled() {
                    return;
                }
                if this.project_setup.is_some() {
                    this.project_setup = None;
                } else {
                    this.pre_scanner.start(this.tree.repo_root.clone());
                    this.close_all_panels();
                    this.show_welcome = false;
                    this.settings_draft = None;
                    this.context_menu = None;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenFilePalette, _w, cx| {
                if !this.map_interaction_enabled() {
                    return;
                }
                this.open_palette(true);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenSymbolPalette, _w, cx| {
                if !this.map_interaction_enabled() {
                    return;
                }
                this.open_palette(false);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenCommandPalette, _w, cx| {
                if !this.map_interaction_enabled() {
                    return;
                }
                this.open_command_palette();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RevealInFileManager, _w, _cx| {
                if !this.map_interaction_enabled() {
                    return;
                }
                let path = resolve_fs_path(&this.focus.current, &this.tree.repo_root);
                open_in_file_manager(&path);
            }))
            .on_action(cx.listener(|this, _: &NextViewTab, _w, cx| {
                this.tab_action(TabJump::Next);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PrevViewTab, _w, cx| {
                this.tab_action(TabJump::Prev);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BaseViewTab, _w, cx| {
                this.tab_action(TabJump::Index(crate::view::tabs::BASE_TAB));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTab1, _w, cx| {
                this.tab_action(TabJump::Index(0));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTab2, _w, cx| {
                this.tab_action(TabJump::Index(1));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTab3, _w, cx| {
                this.tab_action(TabJump::Index(2));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTab4, _w, cx| {
                this.tab_action(TabJump::Index(3));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTab5, _w, cx| {
                this.tab_action(TabJump::Index(4));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTab6, _w, cx| {
                this.tab_action(TabJump::Index(5));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTab7, _w, cx| {
                this.tab_action(TabJump::Index(6));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTab8, _w, cx| {
                this.tab_action(TabJump::Index(7));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTab9, _w, cx| {
                this.tab_action(TabJump::Index(8));
                cx.notify();
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, _w, _cx| {
                    if !this.map_interaction_enabled() || this.call_graph.is_some() {
                        return;
                    }
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
                        let paint_prof = crate::frame_profile::FrameProfile::begin();
                        let origin = bounds.origin;
                        let run = |len: usize, color: u32| TextRun {
                            len,
                            font: gpui::font(theme::FONT_FAMILY),
                            color: rgb(color).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let item_content_mask = |item: &PaintItem| ContentMask {
                            bounds: Bounds::new(
                                point(origin.x + px(item.x), origin.y + px(item.clip_y)),
                                size(px(item.w), px(item.clip_h)),
                            ),
                        };
                        let bar_quads = std::cell::Cell::new(0usize);
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
                            if let Some(strip) = &item.bars {
                                // Bars stay inside the box's visible band, so
                                // no content mask is needed for them.
                                let x0 = item.x + 1.0;
                                let x1 = item.x + item.w - 1.0;
                                let y0 = item.y.max(item.clip_y) + 1.0;
                                let y1 = (item.y + item.h).min(item.clip_y + item.clip_h) - 1.0;
                                let wanted = strip.bars_in_band(y0, y1);
                                if wanted > 0
                                    && bar_quads.get() + wanted <= crate::paint_model::MAX_BAR_QUADS
                                {
                                    let step = strip.bar_step();
                                    let bar_h = strip.bar_h();
                                    let first = ((y0 - strip.y) / step).floor().max(0.0) as usize;
                                    let mut drawn = 0usize;
                                    for b in first..first + wanted {
                                        let y = strip.y + b as f32 * step;
                                        if y < y0 {
                                            continue;
                                        }
                                        if y + bar_h > y1 {
                                            break;
                                        }
                                        let Some((indent, len, class)) = strip.bar_silhouette(b)
                                        else {
                                            continue;
                                        };
                                        let bx = (strip.x + indent as f32 * strip.char_w).max(x0);
                                        let bw = (len as f32 * strip.char_w).max(1.0);
                                        let bx1 = (bx + bw).min(strip.x + strip.w).min(x1);
                                        if bx1 <= bx {
                                            continue;
                                        }
                                        // Half-transparent so bars read as
                                        // distant text texture rather than
                                        // popping against baked pages.
                                        let color = rgba(theme::with_alpha(
                                            theme::dim_toward(theme::bar_color(class), item.light),
                                            theme::BAR_ALPHA,
                                        ));
                                        window.paint_quad(quad(
                                            Bounds::new(
                                                point(origin.x + px(bx), origin.y + px(y)),
                                                size(px(bx1 - bx), px(bar_h)),
                                            ),
                                            px(0.),
                                            color,
                                            px(0.),
                                            color,
                                            BorderStyle::default(),
                                        ));
                                        drawn += 1;
                                    }
                                    bar_quads.set(bar_quads.get() + drawn);
                                }
                            }
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
                                        // Baked textures (folder thumbnails) are
                                        // rendered at full brightness; overlay a
                                        // dim-colored quad so masked-out content
                                        // still reads as dimmed.
                                        if item.light < 1.0 {
                                            let dc = rgb(theme::DIM).opacity(1.0 - item.light);
                                            window.paint_quad(quad(
                                                tb,
                                                px(0.),
                                                dc,
                                                px(0.),
                                                dc,
                                                BorderStyle::default(),
                                            ));
                                        }
                                    },
                                );
                            }
                        };
                        let paint_text = |item: &PaintItem, window: &mut Window, cx: &mut App| {
                            if let Some(n) = &item.name {
                                let mut name_run = run(n.text.len(), theme::TEXT_PRIMARY);
                                if item.light < 1.0 {
                                    name_run.color = name_run.color.opacity(item.light);
                                }
                                let line = window.text_system().shape_line(
                                    n.text.clone().into(),
                                    px(n.font_px),
                                    &[name_run],
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
                        //
                        // Every primitive painted outside a layer costs a
                        // bounds-tree insert in GPUI (an R-tree search for the
                        // topmost overlapping order); at a wide zoom that is
                        // ~10k inserts and ~14ms per frame. Runs of texture-
                        // less items are painted inside one `paint_layer`, so
                        // they share a single order (insertion order still
                        // decides draw order within it — the item list is in
                        // DFS order, so children stay above parents). Texture
                        // items stay outside so their fade/dim quads keep
                        // ordering against the image.
                        let mut i = 0;
                        let n_items = frame.items.len();
                        while i < n_items {
                            let item = &frame.items[i];
                            if item.deferred_overlay {
                                i += 1;
                                continue;
                            }
                            if item.tex.is_some() {
                                window.with_content_mask(Some(item_content_mask(item)), |window| {
                                    paint_surface(item, window)
                                });
                                i += 1;
                                continue;
                            }
                            let start = i;
                            let (mut x0, mut y0, mut x1, mut y1) =
                                (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                            while i < n_items {
                                let it = &frame.items[i];
                                if it.deferred_overlay || it.tex.is_some() {
                                    break;
                                }
                                x0 = x0.min(it.x);
                                y0 = y0.min(it.clip_y);
                                x1 = x1.max(it.x + it.w);
                                y1 = y1.max(it.clip_y + it.clip_h);
                                i += 1;
                            }
                            let run = &frame.items[start..i];
                            let layer_bounds = Bounds::new(
                                point(origin.x + px(x0), origin.y + px(y0)),
                                size(px((x1 - x0).max(0.0)), px((y1 - y0).max(0.0))),
                            );
                            window.paint_layer(layer_bounds, |window| {
                                for item in run {
                                    let clipped =
                                        item.clip_y != item.y || item.clip_h != item.h;
                                    if clipped {
                                        window.with_content_mask(
                                            Some(item_content_mask(item)),
                                            |window| paint_surface(item, window),
                                        );
                                    } else {
                                        paint_surface(item, window);
                                    }
                                }
                            });
                        }
                        // Pass 2a: leaf / non-header text (rendered under
                        // pinned headers so code doesn't bleed through).
                        for item in &frame.items {
                            if item.header_bg_h > 0.0 || item.deferred_overlay {
                                continue;
                            }
                            window.with_content_mask(Some(item_content_mask(item)), |window| {
                                paint_text(item, window, _cx)
                            });
                        }
                        // Pass 2b: headers, background + text interleaved per
                        // item in DFS order, so a later (right/below) header's
                        // opaque background covers earlier headers' text.
                        for item in &frame.items {
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
                        if frame.cg_scrim {
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
                                (2.0, rgba(theme::with_alpha(theme::FOCUS_BORDER, item.light)))
                            } else {
                                (
                                    1.0,
                                    rgba(theme::scale_alpha(theme::NEIGHBOR_BORDER, item.light)),
                                )
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
                        if !frame.cg_scrim {
                            for item in &frame.items {
                                if item.neighbor {
                                    window.with_content_mask(
                                        Some(item_content_mask(item)),
                                        |window| paint_ring(item, window),
                                    );
                                }
                            }
                        }
                        // Pass 2c2: structural/agent mark rings and corner
                        // badges (hotspot, custom labels, agent flags).
                        if !frame.cg_scrim {
                            for item in &frame.items {
                                let Some(badge) = item.badge.as_ref() else { continue };
                                if item.w < 28.0 || item.h < 14.0 {
                                    continue;
                                }
                                let b = Bounds::new(
                                    point(origin.x + px(item.x), origin.y + px(item.y)),
                                    size(px(item.w), px(item.h)),
                                );
                                window.with_content_mask(
                                    Some(item_content_mask(item)),
                                    |window| {
                                        window.paint_quad(quad(
                                            b,
                                            px(theme::CORNER_RADIUS),
                                            transparent_black(),
                                            px(1.5),
                                            rgba((badge.color << 8) | 0xcc),
                                            BorderStyle::default(),
                                        ));
                                    },
                                );
                                // Tag at the top-right corner, only when the box
                                // is wide enough to read it.
                                if item.w >= 72.0 && item.h >= 22.0 {
                                    let font_px = 10.0f32;
                                    let text_w = badge.text.chars().count() as f32 * font_px * 0.62 + 10.0;
                                    let tag_w = text_w.min(item.w - 8.0);
                                    let tag_h = 14.0f32;
                                    let tx = item.x + item.w - tag_w - 4.0;
                                    let ty = item.y + 3.0;
                                    let tb = Bounds::new(
                                        point(origin.x + px(tx), origin.y + px(ty)),
                                        size(px(tag_w), px(tag_h)),
                                    );
                                    window.paint_quad(quad(
                                        tb,
                                        px(3.0),
                                        rgba((badge.color << 8) | 0xe0),
                                        px(0.),
                                        transparent_black(),
                                        BorderStyle::default(),
                                    ));
                                    let run = TextRun {
                                        len: badge.text.len(),
                                        font: gpui::font(theme::FONT_FAMILY_SANS),
                                        color: rgb(0x0c0c0e).into(),
                                        background_color: None,
                                        underline: None,
                                        strikethrough: None,
                                    };
                                    let line = window.text_system().shape_line(
                                        badge.text.clone().into(),
                                        px(font_px),
                                        &[run],
                                        None,
                                    );
                                    let _ = line.paint(
                                        point(origin.x + px(tx + 5.0), origin.y + px(ty + 1.0)),
                                        px(font_px * 1.2),
                                        TextAlign::Left,
                                        None,
                                        window,
                                        _cx,
                                    );
                                }
                            }
                        }
                        // Pass 2d: selected leaf surface and text above every
                        // regular box, texture, body row, header, and neighbor.
                        if let Some(item) = frame.items.iter().find(|item| item.deferred_overlay) {
                            window.with_content_mask(Some(item_content_mask(item)), |window| {
                                paint_surface(item, window);
                                paint_text(item, window, _cx);
                            });
                        }
                        // Pass 3: only the selected focus ring paints above it.
                        for item in &frame.items {
                            if ring_paints_after_leaf_overlay(item.focused, item.neighbor) {
                                window.with_content_mask(Some(item_content_mask(item)), |window| {
                                    paint_ring(item, window)
                                });
                            }
                        }
                        // Pass 3b: edge lines with arrowheads.
                        for edge in &frame.edges.edges {
                            let x1 = origin.x + px(edge.from.0);
                            let y1 = origin.y + px(edge.from.1);
                            let x2 = origin.x + px(edge.to.0);
                            let y2 = origin.y + px(edge.to.1);

                            let dx = edge.to.0 - edge.from.0;
                            let dy = edge.to.1 - edge.from.1;
                            let len = (dx * dx + dy * dy).sqrt();
                            if len < 1.0 {
                                continue;
                            }

                            let color = rgba((edge.color << 8) | 0xCC);
                            let line_w = 1.5 + edge.weight;

                            // Stem line
                            let mut stem = PathBuilder::stroke(px(line_w));
                            stem.move_to(point(x1, y1));
                            stem.line_to(point(x2, y2));
                            if edge.dashed {
                                stem = stem.dash_array(&[px(6.0), px(4.0)]);
                            }
                            if let Ok(path) = stem.build() {
                                window.paint_path(path, color);
                            }

                            // Arrowhead (filled triangle at the "to" end)
                            let nx = dx / len;
                            let ny = dy / len;
                            let arrow_len: f32 = 8.0 + line_w * 2.0;
                            let arrow_w: f32 = 4.0 + line_w;
                            let tip_x = edge.to.0;
                            let tip_y = edge.to.1;
                            let base_x = tip_x - nx * arrow_len;
                            let base_y = tip_y - ny * arrow_len;
                            let wing1_x = base_x + ny * arrow_w;
                            let wing1_y = base_y - nx * arrow_w;
                            let wing2_x = base_x - ny * arrow_w;
                            let wing2_y = base_y + nx * arrow_w;

                            let mut arrow = PathBuilder::fill();
                            arrow.move_to(point(origin.x + px(tip_x), origin.y + px(tip_y)));
                            arrow.line_to(point(
                                origin.x + px(wing1_x),
                                origin.y + px(wing1_y),
                            ));
                            arrow.line_to(point(
                                origin.x + px(wing2_x),
                                origin.y + px(wing2_y),
                            ));
                            arrow.close();
                            if let Ok(path) = arrow.build() {
                                window.paint_path(path, color);
                            }
                        }

                        // Pass 5: guided-tour callout — narration card + leader
                        // line to the step's target. Above everything on the map.
                        if let Some(co) = frame.tour_callout.as_ref() {
                            // Leader line from the card's nearest edge midpoint to
                            // the target anchor.
                            let card_cx = co.x + co.w / 2.0;
                            let card_cy = co.y + co.h / 2.0;
                            let (ax, ay) = co.anchor;
                            // Start the leader at the card edge facing the anchor.
                            let (lx, ly) = if (ay - card_cy).abs() * co.w > (ax - card_cx).abs() * co.h {
                                (card_cx, if ay < card_cy { co.y } else { co.y + co.h })
                            } else {
                                (if ax < card_cx { co.x } else { co.x + co.w }, card_cy)
                            };
                            let mut leader = PathBuilder::stroke(px(1.5));
                            leader.move_to(point(origin.x + px(lx), origin.y + px(ly)));
                            leader.line_to(point(origin.x + px(ax), origin.y + px(ay)));
                            if let Ok(path) = leader.build() {
                                window.paint_path(path, rgba((theme::NARRATION_BORDER << 8) | 0xff));
                            }
                            // Anchor dot.
                            let dot = Bounds::new(
                                point(origin.x + px(ax - 3.0), origin.y + px(ay - 3.0)),
                                size(px(6.0), px(6.0)),
                            );
                            window.paint_quad(quad(
                                dot,
                                px(3.0),
                                rgb(theme::NARRATION_TEXT),
                                px(0.),
                                transparent_black(),
                                BorderStyle::default(),
                            ));
                            // Card.
                            let cb = Bounds::new(
                                point(origin.x + px(co.x), origin.y + px(co.y)),
                                size(px(co.w), px(co.h)),
                            );
                            window.paint_quad(quad(
                                cb,
                                px(theme::CORNER_RADIUS),
                                rgba((theme::NARRATION_BG << 8) | 0xf2),
                                px(1.0),
                                rgb(theme::NARRATION_BORDER),
                                BorderStyle::default(),
                            ));
                            let note_run = |len: usize, color: u32| TextRun {
                                len,
                                font: gpui::font(theme::FONT_FAMILY_SANS),
                                color: rgb(color).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            for bt in &co.rows {
                                let runs: Vec<TextRun> = bt
                                    .runs
                                    .iter()
                                    .map(|&(len, color)| note_run(len, color))
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
                        // Pass 4: focused-leaf doc panel (floats to the right).
                        // Skipped in call-graph mode.
                        if let Some(dp) = frame.doc_panel.as_ref().filter(|_| !frame.cg_scrim) {
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
                        if let Some(p) = paint_prof {
                            p.finish("PAINT");
                        }
                    },
                )
                .size_full(),
            )
            .children(toolbar_overlay)
            .children(tab_bar_overlay)
            .children(tour_overlay)
            .children(composer_overlay)
            .children(panels_overlay)
            .children(cmd_palette_overlay)
            .children(settings_overlay)
            .children(welcome_overlay)
            .children(context_menu_overlay)
            .children(call_graph_overlay)
            .children(delete_overlay)
            .children(rename_overlay)
            .children(loading_overlay)
            .children(file_menu_overlay)
            .children(pre_scan_overlay)
            .children(project_setup_overlay)
            .children(notification_overlay);

        map
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use outrider_index::{SymbolId, SymbolKind, SymbolNode};
    use outrider_layout::{PackLayout, Rect};

    use super::{parse_gibibytes, SettingsDraft};
    use crate::camera::Camera;

    fn packing_layout(x: f64) -> PackLayout {
        let id = SymbolId {
            kind: SymbolKind::File,
            qualified_path: "src/main.rs".into(),
            ordinal: 0,
        };
        PackLayout {
            rects: std::collections::HashMap::from([(
                id,
                Rect {
                    x,
                    y: 0.0,
                    w: 10.0,
                    h: 20.0,
                },
            )]),
        }
    }

    #[test]
    fn packing_snapshot_only_retargets_geometry_and_invalidates_derived_state() {
        use std::time::Instant;

        let mut layout = packing_layout(0.0);
        let target = packing_layout(100.0);
        let mut transition = None;
        let mut packing_target = None;
        let mut camera = Some(Camera {
            center_x: 1.0,
            center_y: 2.0,
            zoom: 3.0,
        });
        let id = layout.rects.keys().next().unwrap().clone();
        let mut neighbors = Some((id.clone(), [None, None, None, None]));
        let mut hover = Some(id);
        let mut progress = None;

        super::PackingGeometryState {
            layout: &mut layout,
            transition: &mut transition,
            target: &mut packing_target,
            camera: &mut camera,
            neighbors: &mut neighbors,
            hover: &mut hover,
            progress: &mut progress,
        }
        .apply_snapshot(target.clone(), Instant::now());

        assert_eq!(packing_target, Some(target));
        assert!(transition.is_some());
        assert!(camera.is_none());
        assert!(neighbors.is_none());
        assert!(hover.is_none());
    }

    #[test]
    fn packing_finalization_installs_exact_geometry_and_clears_progress() {
        use std::time::Instant;

        let mut layout = packing_layout(0.0);
        let final_layout = packing_layout(321.0);
        let mut transition = Some(crate::layout_transition::LayoutTransition::new(
            layout.clone(),
            packing_layout(100.0),
            Instant::now(),
        ));
        let mut packing_target = Some(packing_layout(100.0));
        let mut camera = None;
        let mut neighbors = None;
        let mut hover = None;
        let mut progress = Some(crate::project_loader::LoadProgress {
            folder_name: "project".into(),
            phase: crate::project_loader::LoadPhase::Packing,
            completed: 1,
            total: 2,
        });

        super::PackingGeometryState {
            layout: &mut layout,
            transition: &mut transition,
            target: &mut packing_target,
            camera: &mut camera,
            neighbors: &mut neighbors,
            hover: &mut hover,
            progress: &mut progress,
        }
        .finish(final_layout.clone());

        assert_eq!(layout, final_layout);
        assert!(transition.is_none());
        assert!(packing_target.is_none());
        assert!(progress.is_none());
    }

    #[test]
    fn packing_snapshot_retargets_from_the_exact_current_sample() {
        use std::time::{Duration, Instant};

        let now = Instant::now();
        let mut layout = packing_layout(0.0);
        let first_target = packing_layout(100.0);
        let expected = crate::layout_transition::LayoutTransition::new(
            layout.clone(),
            first_target.clone(),
            now,
        )
        .sample(now + Duration::from_millis(80));
        let mut transition = Some(crate::layout_transition::LayoutTransition::new(
            layout.clone(),
            first_target,
            now,
        ));
        let mut packing_target = Some(packing_layout(100.0));
        let mut camera = None;
        let mut neighbors = None;
        let mut hover = None;
        let mut progress = None;

        super::PackingGeometryState {
            layout: &mut layout,
            transition: &mut transition,
            target: &mut packing_target,
            camera: &mut camera,
            neighbors: &mut neighbors,
            hover: &mut hover,
            progress: &mut progress,
        }
        .apply_snapshot(packing_layout(200.0), now + Duration::from_millis(80));

        assert_eq!(layout, expected);
    }

    #[test]
    fn packing_failure_keeps_complete_geometry_and_clears_packing_state() {
        use std::time::{Duration, Instant};

        let now = Instant::now();
        let initial = packing_layout(0.0);
        let retained = packing_layout(100.0);
        let transition =
            crate::layout_transition::LayoutTransition::new(initial, retained.clone(), now);
        let mut layout = transition.sample(now + Duration::from_millis(80));
        assert_ne!(
            layout, retained,
            "fixture must fail from an in-flight sample"
        );
        let mut transition = Some(transition);
        let mut packing_target = Some(retained.clone());
        let mut camera = None;
        let mut neighbors = None;
        let mut hover = None;
        let mut progress = Some(crate::project_loader::LoadProgress {
            folder_name: "project".into(),
            phase: crate::project_loader::LoadPhase::Packing,
            completed: 1,
            total: 2,
        });

        super::PackingGeometryState {
            layout: &mut layout,
            transition: &mut transition,
            target: &mut packing_target,
            camera: &mut camera,
            neighbors: &mut neighbors,
            hover: &mut hover,
            progress: &mut progress,
        }
        .fail_after_preview();

        assert_eq!(layout, retained);
        assert!(transition.is_none());
        assert!(packing_target.is_none());
        assert!(progress.is_none());
    }

    #[test]
    fn beginning_a_load_discards_project_setup_and_invalidates_pre_scan() {
        let project = tempfile::tempdir().unwrap();
        let mut pre_scanner = crate::project_loader::PreScanner::new();
        pre_scanner.start(project.path().to_path_buf());
        let mut project_setup = Some("draft");

        super::clear_project_setup_before_load(&mut project_setup, &mut pre_scanner);

        assert!(project_setup.is_none());
        assert!(!pre_scanner.is_scanning());
        assert!(matches!(
            pre_scanner.poll(),
            crate::project_loader::PreScanPoll::Idle
        ));
    }

    #[test]
    fn project_setup_is_hidden_until_the_real_loader_reaches_terminal_state() {
        use std::time::{Duration, Instant};

        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut loader = crate::project_loader::ProjectLoader::new();
        loader.start(
            project.path().to_path_buf(),
            crate::settings::Settings::default(),
        );

        assert!(!super::project_setup_available_for(true, &loader));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match loader.poll() {
                crate::project_loader::LoaderPoll::Complete { .. }
                | crate::project_loader::LoaderPoll::Failed { .. } => break,
                _ if Instant::now() < deadline => std::thread::yield_now(),
                _ => panic!("loader did not reach a terminal state"),
            }
        }
        assert!(super::project_setup_available_for(true, &loader));
        assert!(!super::project_setup_available_for(false, &loader));
    }

    #[test]
    fn packing_interaction_gate_tracks_the_real_loader_lifecycle() {
        use std::time::{Duration, Instant};

        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut loader = crate::project_loader::ProjectLoader::new();
        loader.start(
            project.path().to_path_buf(),
            crate::settings::Settings::default(),
        );

        assert!(!super::map_interaction_enabled_for(&loader));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match loader.poll() {
                crate::project_loader::LoaderPoll::Complete { .. }
                | crate::project_loader::LoaderPoll::Failed { .. } => break,
                _ if Instant::now() < deadline => std::thread::yield_now(),
                _ => panic!("loader did not reach a terminal state"),
            }
        }
        assert!(super::map_interaction_enabled_for(&loader));
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

    use super::{
        call_graph_column_lefts, container_body, container_children_have_images,
        container_header_bg_h, container_header_layout, container_header_px, descendant_paint_clip,
        focused_width, header_bg_paint_h, header_paint_y, leaf_tex_rect, leaf_text_body,
        leaf_texture_is_visible, max_line_chars, BODY_PAD, HEADER, LINE_STEP,
    };
    use crate::buffers::BufferManager;
    use crate::paint_model::{
        char_budget, code_line, runs_from_spans, truncate_to_width, wrap_code_line, wrap_doc,
        wrap_to_budget,
    };
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
    fn call_graph_columns_are_anchored_to_the_focused_leaf() {
        let (callers_left, callees_left) = call_graph_column_lefts(1520.0, 2320.0, 320.0);

        assert_eq!(callers_left, 1188.0);
        assert_eq!(callees_left, 2332.0);
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
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
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
        // code row 0 at the leaf's natural content offset (a one-line item
        // leaf has no name row: its code starts under the short-leaf pad)
        let y0 = crate::content::leaf_content_y0(&leaf, 1.0);
        assert!((y0 - crate::content::SHORT_LEAF_TOP_PAD).abs() < 1e-9);
        assert!((f64::from(body[0].y) - y0).abs() < 1e-3);
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
        let y0 = crate::content::leaf_content_y0(&leaf, 2.0);
        assert!((f64::from(body[0].y) - y0).abs() < 1e-3);
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
            crate::content::leaf_content_y0(&leaf, 1.0) + 0.1,
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
            diff_status: None,
            diff_hunks: Vec::new(),
            deleted_lines: Vec::new(),
            visibility: None,
            children: vec![],
        }
    }

    #[test]
    fn classify_tint_docs_folder() {
        use crate::theme::BoxTint;
        for name in &["docs", "doc", "documentation"] {
            let n = make_node(SymbolKind::Folder, name);
            assert_eq!(
                crate::theme::node_box_tint(&n),
                BoxTint::DocsFolder,
                "expected DocsFolder for {name}"
            );
        }
    }

    #[test]
    fn classify_tint_test_folder() {
        use crate::theme::BoxTint;
        for name in &["test", "tests", "spec", "specs", "__tests__"] {
            let n = make_node(SymbolKind::Folder, name);
            assert_eq!(
                crate::theme::node_box_tint(&n),
                BoxTint::TestFolder,
                "expected TestFolder for {name}"
            );
        }
    }

    #[test]
    fn classify_tint_items_use_file_extension() {
        use crate::theme::BoxTint;
        let mut n = make_node(
            SymbolKind::Item {
                label: "struct".to_string(),
            },
            "Foo",
        );
        n.id.qualified_path = "src/main.rs::Foo".into();
        assert_eq!(
            crate::theme::node_box_tint(&n),
            BoxTint::FileType(crate::theme::extension_tint("rs"))
        );
        n.id.qualified_path = "app.ts::Bar".into();
        assert_eq!(
            crate::theme::node_box_tint(&n),
            BoxTint::FileType(crate::theme::extension_tint("ts"))
        );
    }

    #[test]
    fn classify_tint_normal_cases() {
        use crate::theme::BoxTint;
        // Unrecognized folder name
        assert_eq!(
            crate::theme::node_box_tint(&make_node(SymbolKind::Folder, "src")),
            BoxTint::Normal
        );
        // Item without file extension in qualified_path
        assert_eq!(
            crate::theme::node_box_tint(&make_node(SymbolKind::Item { label: "fn".into() }, "foo")),
            BoxTint::Normal
        );
        // File gets FileType tint based on extension
        assert_eq!(
            crate::theme::node_box_tint(&make_node(SymbolKind::File, "main.rs")),
            BoxTint::FileType(crate::theme::extension_tint("rs"))
        );
        // Chunk without extension falls back to Normal
        assert_eq!(
            crate::theme::node_box_tint(&make_node(SymbolKind::Chunk, "chunk")),
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
    use crate::focus::TreeIndex;
    use outrider_index::SymbolTree;

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
    fn descendant_paint_clip_starts_below_ancestor_headers() {
        assert_eq!(
            descendant_paint_clip(100.0, 80.0, 140.0),
            Some((140.0, 40.0))
        );
        assert_eq!(
            descendant_paint_clip(150.0, 30.0, 140.0),
            Some((150.0, 30.0))
        );
    }

    #[test]
    fn descendant_paint_clip_omits_nodes_hidden_by_ancestor_headers() {
        assert_eq!(descendant_paint_clip(100.0, 40.0, 140.0), None);
        assert_eq!(descendant_paint_clip(100.0, 20.0, 141.0), None);
    }

    #[test]
    fn descendant_paint_clip_uses_the_full_nested_header_stack() {
        let root_bottom = 100.0 + HEADER;
        let nested_bottom = root_bottom + HEADER;
        assert_eq!(
            descendant_paint_clip(100.0, 100.0, nested_bottom),
            Some((nested_bottom, 100.0 - 2.0 * HEADER)),
        );
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
        let mut rects = std::collections::HashMap::new();
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
    fn format_note_rows_lists_and_code_spans() {
        use crate::paint_model::format_note_rows;
        let text = "Intro line.\n\n- first item\n- second `code` item\n\nOutro.";
        let rows = format_note_rows(text, 400.0, 12.0, 0xffffff);
        let texts: Vec<&str> = rows.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "Intro line.",
                "",
                "- first item",
                "- second code item",
                "",
                "Outro."
            ]
        );
        // The `code` span is recolored and the backticks are gone.
        let (_, runs) = &rows[3];
        assert!(runs.iter().any(|(_, c)| *c == crate::theme::CODE_SPAN));
        assert_eq!(runs.iter().map(|(l, _)| l).sum::<usize>(), rows[3].0.len());
    }

    #[test]
    fn format_note_rows_hanging_indent_on_wrapped_items() {
        use crate::paint_model::format_note_rows;
        let text = "1. a numbered item whose text is long enough to wrap onto more rows";
        let rows = format_note_rows(text, 200.0, 12.0, 0xffffff);
        assert!(rows.len() >= 2);
        assert!(rows[0].0.starts_with("1. "));
        assert!(rows[1].0.starts_with("   "), "continuation is indented: {:?}", rows[1].0);
    }

    #[test]
    fn md_paragraphs_splits_items_and_strips_backticks() {
        use crate::view::tour_panel::md_paragraphs;
        let body = "First para\ncontinues.\n\n- item `one`\n- item two\n\nLast.";
        assert_eq!(
            md_paragraphs(body),
            vec!["First para continues.", "- item one", "- item two", "Last."]
        );
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
