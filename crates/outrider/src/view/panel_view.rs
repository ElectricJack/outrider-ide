//! Panel state and keyboard logic for the app side.
//! GPUI-free pure types and functions — rendering lives in treemap.rs.

use std::ops::Range;

use outrider_index::SymbolId;
use outrider_view::layers::panel::{ResolvedPanel, Row};
use outrider_view::spec::Dock;

// ── Constants ──

pub(crate) const PANEL_COL_W: f32 = 320.0;
pub(crate) const PANEL_CARD_H: f32 = 120.0;
pub(crate) const PANEL_SELECTED_H: f32 = 300.0;
pub(crate) const PANEL_CARD_GAP: f32 = 6.0;
pub(crate) const PANEL_SCROLL_SECS: f64 = 0.20;
pub(crate) const PALETTE_W: f32 = 500.0;
pub(crate) const PREVIEW_W: f32 = 300.0;
pub(crate) const PREVIEW_GAP: f32 = 8.0;

// ── ScrollTween ──

pub(crate) struct ScrollTween {
    from: f32,
    target: f32,
    started: std::time::Instant,
}

impl ScrollTween {
    pub fn new(pos: f32) -> Self {
        ScrollTween {
            from: pos,
            target: pos,
            started: std::time::Instant::now(),
        }
    }

    pub fn current(&self) -> f32 {
        let t = (self.started.elapsed().as_secs_f64() / PANEL_SCROLL_SECS).min(1.0);
        let e = crate::camera::ease_in_out_cubic(t) as f32;
        self.from + (self.target - self.from) * e
    }

    pub fn is_animating(&self) -> bool {
        self.started.elapsed().as_secs_f64() < PANEL_SCROLL_SECS
    }

    pub fn retarget(&mut self, t: f32) {
        self.from = self.current();
        self.target = t;
        self.started = std::time::Instant::now();
    }
}

// ── PanelInstance ──

pub(crate) struct PanelInstance {
    pub id: String,
    pub layer: usize,
    pub selection: usize,
    pub group_active: Vec<usize>,
    pub scroll: ScrollTween,
    pub query: Option<String>,
    pub set_name: Option<String>,
    pub kind_filter: Option<outrider_view::spec::SetExpr>,
    pub preview: bool,
    pub captures_mouse: bool,
    pub center: Option<SymbolId>,
    last_row_ids: Vec<SymbolId>,
}

impl PanelInstance {
    pub fn new(id: String, layer: usize) -> Self {
        PanelInstance {
            id,
            layer,
            selection: 0,
            group_active: Vec::new(),
            scroll: ScrollTween::new(0.0),
            query: None,
            set_name: None,
            kind_filter: None,
            preview: false,
            captures_mouse: false,
            center: None,
            last_row_ids: Vec::new(),
        }
    }
}

// ── PanelState ──

pub(crate) struct PanelState {
    pub open: Vec<PanelInstance>,
    pub active: usize,
    selected_cache: Option<SymbolId>,
}

impl PanelState {
    pub fn new() -> Self {
        PanelState {
            open: Vec::new(),
            active: 0,
            selected_cache: None,
        }
    }

    pub fn is_open(&self) -> bool {
        !self.open.is_empty()
    }

    pub fn has_float(&self) -> bool {
        // A float panel has no captures_mouse and typically has query
        self.open.iter().any(|p| !p.captures_mouse)
    }

    pub fn captures_mouse(&self) -> bool {
        self.open.iter().any(|p| p.captures_mouse)
    }

    pub fn is_animating(&self) -> bool {
        self.open.iter().any(|p| p.scroll.is_animating())
    }

    /// Sync instances with resolved panels. Returns true if the selection changed.
    pub fn sync(&mut self, panels: &[ResolvedPanel]) -> bool {
        let mut selection_changed = false;

        // Match open instances to resolved panels by id (or layer index).
        // Remove instances whose layer no longer has a panel.
        self.open.retain(|inst| {
            panels.iter().any(|rp| match (&inst.id, &rp.id) {
                (id, Some(rp_id)) => id == rp_id,
                _ => inst.layer == rp.layer,
            })
        });

        // For each open instance, sync with its resolved panel.
        for inst in &mut self.open {
            let resolved = panels.iter().find(|rp| match &rp.id {
                Some(rp_id) => inst.id == *rp_id,
                None => inst.layer == rp.layer,
            });
            let Some(resolved) = resolved else { continue };

            // Update layer index (may have shifted after RemoveLayer).
            inst.layer = resolved.layer;

            // Detect row change.
            let new_ids: Vec<SymbolId> = resolved.rows.iter().map(|r| r.id.clone()).collect();
            if new_ids != inst.last_row_ids {
                let had_query = inst.query.is_some();
                if had_query && !new_ids.is_empty() {
                    // Palette narrowing: clamp rather than reset.
                    inst.selection = inst.selection.min(new_ids.len().saturating_sub(1));
                } else {
                    inst.selection = 0;
                }
                // Resize group_active.
                inst.group_active.resize(new_ids.len(), 0);
                inst.last_row_ids = new_ids;
                selection_changed = true;
            }
        }

        // Update active index.
        if !self.open.is_empty() {
            self.active = self.active.min(self.open.len() - 1);
        }

        // Update selection cache.
        self.selected_cache = self.selected(panels);

        selection_changed
    }

    /// The effective id of the active panel's currently selected row.
    pub fn selected(&self, panels: &[ResolvedPanel]) -> Option<SymbolId> {
        let inst = self.open.get(self.active)?;
        let resolved = panels.iter().find(|rp| match &rp.id {
            Some(rp_id) => inst.id == *rp_id,
            None => inst.layer == rp.layer,
        })?;
        let row = resolved.rows.get(inst.selection)?;
        let active_alt = inst.group_active.get(inst.selection).copied().unwrap_or(0);
        Some(effective_id(row, active_alt).clone())
    }

    /// The call site range for the active selection (for 06-marks highlight).
    pub fn selected_call_site(
        &self,
        panels: &[ResolvedPanel],
    ) -> Option<(SymbolId, Range<usize>)> {
        let inst = self.open.get(self.active)?;
        let resolved = panels.iter().find(|rp| match &rp.id {
            Some(rp_id) => inst.id == *rp_id,
            None => inst.layer == rp.layer,
        })?;
        let row = resolved.rows.get(inst.selection)?;
        let active_alt = inst.group_active.get(inst.selection).copied().unwrap_or(0);
        let alt = row.alternates.get(active_alt)?;
        let site = alt.call_site.clone()?;
        Some((alt.id.clone(), site))
    }

    /// Cached selection (avoids borrow issues in TreemapView).
    pub fn cached_selection(&self) -> Option<&SymbolId> {
        self.selected_cache.as_ref()
    }

    /// Configure the last-pushed panel instance.
    pub fn configure_last(&mut self, f: impl FnOnce(&mut PanelInstance)) {
        if let Some(inst) = self.open.last_mut() {
            f(inst);
        }
    }

    /// Push a new panel instance for a layer.
    pub fn push(&mut self, id: String, layer: usize) {
        let inst = PanelInstance::new(id, layer);
        self.open.push(inst);
        self.active = self.open.len() - 1;
    }

    /// Remove all open panels, returning their layer indices.
    pub fn close_all(&mut self) -> Vec<usize> {
        let layers: Vec<usize> = self.open.iter().map(|p| p.layer).collect();
        self.open.clear();
        self.active = 0;
        self.selected_cache = None;
        layers
    }

    /// Remove panels matching a predicate, returning their layer indices.
    pub fn close_where(&mut self, pred: impl Fn(&PanelInstance) -> bool) -> Vec<usize> {
        let mut layers = Vec::new();
        self.open.retain(|p| {
            if pred(p) {
                layers.push(p.layer);
                false
            } else {
                true
            }
        });
        if !self.open.is_empty() {
            self.active = self.active.min(self.open.len() - 1);
        } else {
            self.active = 0;
        }
        self.selected_cache = None;
        layers
    }

    /// Find an open panel by id.
    pub fn find(&self, id: &str) -> Option<usize> {
        self.open.iter().position(|p| p.id == id)
    }
}

/// The effective symbol id for a row given the active alternate index.
pub(crate) fn effective_id(row: &Row, active: usize) -> &SymbolId {
    row.alternates
        .get(active)
        .map(|a| &a.id)
        .unwrap_or(&row.id)
}

// ── PanelKeyEffect ──

/// Result of processing a key press on the panel — pure data, no GPUI.
pub(crate) enum PanelKeyEffect {
    None,
    SelectionChanged,
    Enter(SymbolId),
    Close(Vec<usize>),
    QueryChanged(String),
}

/// Process a key press on the active panel. Returns an effect for the caller to apply.
pub(crate) fn panel_key(
    state: &mut PanelState,
    panels: &[ResolvedPanel],
    key: &str,
    ch: Option<char>,
) -> PanelKeyEffect {
    if state.open.is_empty() {
        return PanelKeyEffect::None;
    }

    match key {
        "escape" | "tab" => {
            // Close panels from the same gesture group.
            // Float panels (palette) close alone; docked pairs close together.
            let active_inst = match state.open.get(state.active) {
                Some(inst) => inst,
                None => return PanelKeyEffect::None,
            };
            let is_docked = active_inst.captures_mouse;
            let center = active_inst.center.clone();
            let layers = if is_docked {
                // Close all docked panels (the call-graph pair).
                state.close_where(|p| p.captures_mouse)
            } else {
                // Close just the active float panel.
                let layer = active_inst.layer;
                let id = active_inst.id.clone();
                state.close_where(|p| p.id == id || p.layer == layer)
            };
            if is_docked {
                // For call-graph panels, return the center id so the caller
                // can re-focus on it.
                if let Some(center_id) = center {
                    return PanelKeyEffect::Enter(center_id);
                }
            }
            PanelKeyEffect::Close(layers)
        }

        "up" => {
            let inst = match state.open.get_mut(state.active) {
                Some(inst) => inst,
                None => return PanelKeyEffect::None,
            };
            let row_count = find_resolved(&inst.id, inst.layer, panels)
                .map(|rp| rp.rows.len())
                .unwrap_or(0);
            if row_count == 0 {
                return PanelKeyEffect::None;
            }
            if inst.captures_mouse {
                // Docked: clamp at 0.
                if inst.selection > 0 {
                    inst.selection -= 1;
                    inst.scroll
                        .retarget(scroll_target(inst.selection));
                } else {
                    return PanelKeyEffect::None;
                }
            } else {
                // Float: wrap.
                if inst.selection == 0 {
                    inst.selection = row_count - 1;
                } else {
                    inst.selection -= 1;
                }
            }
            PanelKeyEffect::SelectionChanged
        }

        "down" => {
            let inst = match state.open.get_mut(state.active) {
                Some(inst) => inst,
                None => return PanelKeyEffect::None,
            };
            let row_count = find_resolved(&inst.id, inst.layer, panels)
                .map(|rp| rp.rows.len())
                .unwrap_or(0);
            if row_count == 0 {
                return PanelKeyEffect::None;
            }
            if inst.captures_mouse {
                // Docked: clamp at len-1.
                if inst.selection + 1 < row_count {
                    inst.selection += 1;
                    inst.scroll
                        .retarget(scroll_target(inst.selection));
                } else {
                    return PanelKeyEffect::None;
                }
            } else {
                // Float: wrap.
                inst.selection = (inst.selection + 1) % row_count;
            }
            PanelKeyEffect::SelectionChanged
        }

        "enter" => {
            let inst = match state.open.get(state.active) {
                Some(inst) => inst,
                None => return PanelKeyEffect::None,
            };
            let resolved = match find_resolved(&inst.id, inst.layer, panels) {
                Some(rp) => rp,
                None => return PanelKeyEffect::None,
            };
            let row = match resolved.rows.get(inst.selection) {
                Some(r) => r,
                None => return PanelKeyEffect::None,
            };
            let active_alt = inst.group_active.get(inst.selection).copied().unwrap_or(0);
            let id = effective_id(row, active_alt).clone();

            if !inst.captures_mouse {
                // Float (palette): close on Enter.
                let layer = inst.layer;
                let inst_id = inst.id.clone();
                state.close_where(|p| p.id == inst_id || p.layer == layer);
            }
            PanelKeyEffect::Enter(id)
        }

        "left" => handle_left_right(state, panels, true),
        "right" => handle_left_right(state, panels, false),

        "backspace" => {
            let inst = match state.open.get_mut(state.active) {
                Some(inst) if inst.query.is_some() => inst,
                _ => return PanelKeyEffect::None,
            };
            let q = inst.query.as_mut().unwrap();
            q.pop();
            let new_query = q.clone();
            PanelKeyEffect::QueryChanged(new_query)
        }

        _ => {
            // Printable character → query.
            if let Some(ch) = ch {
                let inst = match state.open.get_mut(state.active) {
                    Some(inst) if inst.query.is_some() => inst,
                    _ => return PanelKeyEffect::None,
                };
                let q = inst.query.as_mut().unwrap();
                q.push(ch);
                let new_query = q.clone();
                PanelKeyEffect::QueryChanged(new_query)
            } else {
                PanelKeyEffect::None
            }
        }
    }
}

fn handle_left_right(
    state: &mut PanelState,
    panels: &[ResolvedPanel],
    is_left: bool,
) -> PanelKeyEffect {
    let inst = match state.open.get(state.active) {
        Some(inst) => inst,
        None => return PanelKeyEffect::None,
    };

    if !inst.captures_mouse {
        // Float panels: left/right is no-op.
        return PanelKeyEffect::None;
    }

    let resolved = match find_resolved(&inst.id, inst.layer, panels) {
        Some(rp) => rp,
        None => return PanelKeyEffect::None,
    };

    // Determine if we're pointing toward the other docked panel or cycling alternates.
    let dock = resolved.dock;
    let pointing_toward_other = match dock {
        Dock::Left => !is_left,   // Right arrow on left panel → toward other
        Dock::Right => is_left,   // Left arrow on right panel → toward other
        _ => false,
    };

    if pointing_toward_other {
        // Switch to the other docked panel.
        let target_dock = if dock == Dock::Left { Dock::Right } else { Dock::Left };
        let other_idx = state.open.iter().position(|p| {
            p.captures_mouse
                && find_resolved(&p.id, p.layer, panels)
                    .map(|rp| rp.dock == target_dock)
                    .unwrap_or(false)
        });
        if let Some(idx) = other_idx {
            let other_rp = find_resolved(&state.open[idx].id, state.open[idx].layer, panels);
            if other_rp.map(|rp| !rp.rows.is_empty()).unwrap_or(false) {
                state.active = idx;
                state.open[idx].selection = 0;
                state.open[idx].scroll.retarget(0.0);
                return PanelKeyEffect::SelectionChanged;
            }
        }
        PanelKeyEffect::None
    } else {
        // Cycle group_active on the current row.
        let inst = match state.open.get_mut(state.active) {
            Some(inst) => inst,
            None => return PanelKeyEffect::None,
        };
        let row = match resolved.rows.get(inst.selection) {
            Some(r) if r.alternates.len() > 1 => r,
            _ => return PanelKeyEffect::None,
        };
        let ga = inst
            .group_active
            .get_mut(inst.selection)
            .expect("group_active out of sync");
        let n = row.alternates.len();
        if is_left {
            *ga = (*ga + n - 1) % n;
        } else {
            *ga = (*ga + 1) % n;
        }
        PanelKeyEffect::SelectionChanged
    }
}

fn find_resolved<'a>(id: &str, layer: usize, panels: &'a [ResolvedPanel]) -> Option<&'a ResolvedPanel> {
    panels.iter().find(|rp| match &rp.id {
        Some(rp_id) => id == rp_id.as_str(),
        None => rp.layer == layer,
    })
}

// ── Layout helpers ──

/// Column x positions for left/right docked panels relative to the focus rect.
pub(crate) fn call_graph_column_lefts(
    focus_left: f32,
    focus_right: f32,
    column_width: f32,
) -> (f32, f32) {
    const GAP: f32 = 12.0;
    (focus_left - column_width - GAP, focus_right + GAP)
}

/// Scroll target for the nth card.
pub(crate) fn scroll_target(selected_idx: usize) -> f32 {
    selected_idx as f32 * (PANEL_CARD_H + PANEL_CARD_GAP)
}

/// Y position of the ith card given the current selection.
pub(crate) fn card_top(i: usize, selected: Option<usize>) -> f32 {
    let mut y = 0.0_f32;
    for j in 0..i {
        y += if selected == Some(j) {
            PANEL_SELECTED_H
        } else {
            PANEL_CARD_H
        };
        y += PANEL_CARD_GAP;
    }
    y
}

/// Height of the ith card given the current selection.
pub(crate) fn card_height(i: usize, selected: Option<usize>) -> f32 {
    if selected == Some(i) {
        PANEL_SELECTED_H
    } else {
        PANEL_CARD_H
    }
}

/// Bounding rect for a panel given its dock and viewport.
pub(crate) fn panel_rect(
    dock: Dock,
    vw: f64,
    vh: f64,
    focus_screen: Option<(f32, f32, f32)>,
) -> (f32, f32, f32, f32) {
    match dock {
        Dock::Left | Dock::Right => {
            let w = PANEL_COL_W;
            let top = 48.0_f32;
            let h = (vh as f32 - 96.0).max(200.0);
            let (callers_left, callees_left) = match focus_screen {
                Some((fl, fr, _cw)) => call_graph_column_lefts(fl, fr, w),
                None => (12.0, vw as f32 - w - 12.0),
            };
            let left = if dock == Dock::Left {
                callers_left
            } else {
                callees_left
            };
            (left, top, w, h)
        }
        Dock::Bottom => {
            let left = 12.0_f32;
            let w = vw as f32 - 24.0;
            let h = 200.0_f32;
            let top = vh as f32 - h - 12.0;
            (left, top, w, h)
        }
        Dock::Float => {
            let top = 60.0_f32;
            let w = PALETTE_W;
            let total_w = w + PREVIEW_GAP + PREVIEW_W;
            let left = ((vw as f32 - total_w) / 2.0).max(0.0);
            (left, top, w, vh as f32 - top - 12.0)
        }
    }
}

// ── Helpers moved from treemap.rs ──

/// Extract the parent symbol name from a qualified path (e.g. "file.rs::Foo::bar" → "Foo").
pub(crate) fn parent_name(qualified_path: &str) -> Option<String> {
    let after_file = qualified_path.split("::").skip(1).collect::<Vec<_>>();
    if after_file.len() >= 2 {
        Some(after_file[..after_file.len() - 1].join("::"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind};
    use outrider_view::layers::panel::{ResolvedPanel, Row, RowAlternate};
    use outrider_view::spec::{Dock, MetricRef};

    fn make_id(path: &str) -> SymbolId {
        SymbolId {
            kind: SymbolKind::File,
            qualified_path: path.to_string(),
            ordinal: 0,
        }
    }

    fn make_panel(layer: usize, id: &str, dock: Dock, rows: Vec<Row>) -> ResolvedPanel {
        ResolvedPanel {
            layer,
            id: Some(id.to_string()),
            title: Some(id.to_string()),
            dock,
            columns: vec![],
            rows,
            pending: false,
            deps: outrider_view::Deps::TREE,
        }
    }

    fn make_row(path: &str, label: &str) -> Row {
        Row {
            id: make_id(path),
            label: label.to_string(),
            sublabel: path.to_string(),
            cells: vec![],
            group: None,
            alternates: vec![],
        }
    }

    fn make_edge_row(path: &str, label: &str, alternates: Vec<(&str, Option<Range<usize>>)>) -> Row {
        let alts: Vec<RowAlternate> = alternates
            .into_iter()
            .map(|(p, cs)| RowAlternate {
                id: make_id(p),
                call_site: cs,
            })
            .collect();
        Row {
            id: make_id(path),
            label: label.to_string(),
            sublabel: path.to_string(),
            cells: vec![],
            group: Some(label.to_string()),
            alternates: alts,
        }
    }

    // ── keyboard_selection_updates_selection ──

    #[test]
    fn keyboard_selection_set_panel() {
        let rows = vec![
            make_row("a.rs", "alpha"),
            make_row("b.rs", "bravo"),
            make_row("c.rs", "charlie"),
        ];
        let panels = vec![make_panel(0, "test", Dock::Float, rows)];

        let mut state = PanelState::new();
        state.push("test".into(), 0);
        state.sync(&panels);

        // Down moves selection.
        let eff = panel_key(&mut state, &panels, "down", None);
        assert!(matches!(eff, PanelKeyEffect::SelectionChanged));
        assert_eq!(state.open[0].selection, 1);
        assert_eq!(state.selected(&panels).unwrap(), make_id("b.rs"));

        // Up wraps on float.
        state.open[0].selection = 0;
        let eff = panel_key(&mut state, &panels, "up", None);
        assert!(matches!(eff, PanelKeyEffect::SelectionChanged));
        assert_eq!(state.open[0].selection, 2); // wrapped
    }

    #[test]
    fn keyboard_selection_docked_clamps() {
        let rows = vec![
            make_row("a.rs", "alpha"),
            make_row("b.rs", "bravo"),
        ];
        let panels = vec![make_panel(0, "callers", Dock::Left, rows)];

        let mut state = PanelState::new();
        state.push("callers".into(), 0);
        state.configure_last(|p| p.captures_mouse = true);
        state.sync(&panels);

        // Up at 0 on docked panel: no-op.
        let eff = panel_key(&mut state, &panels, "up", None);
        assert!(matches!(eff, PanelKeyEffect::None));
        assert_eq!(state.open[0].selection, 0);
    }

    // ── left_right_switch_and_cycle ──

    #[test]
    fn left_right_switch_and_cycle() {
        let caller_rows = vec![
            make_edge_row("x.rs", "x_fn", vec![("x.rs", None), ("x2.rs", Some(10..20)), ("x3.rs", Some(30..40))]),
        ];
        let callee_rows = vec![
            make_edge_row("y.rs", "y_fn", vec![("y.rs", None), ("y2.rs", Some(50..60))]),
        ];
        let panels = vec![
            make_panel(0, "__callers", Dock::Left, caller_rows),
            make_panel(1, "__callees", Dock::Right, callee_rows),
        ];

        let mut state = PanelState::new();
        state.push("__callers".into(), 0);
        state.configure_last(|p| p.captures_mouse = true);
        state.push("__callees".into(), 1);
        state.configure_last(|p| p.captures_mouse = true);
        state.sync(&panels);
        // active = 1 (callees, last pushed)

        // Right on callees → cycle alternate.
        let eff = panel_key(&mut state, &panels, "right", None);
        assert!(matches!(eff, PanelKeyEffect::SelectionChanged));
        assert_eq!(state.open[1].group_active[0], 1);

        // Left on callees → switch to callers.
        let eff = panel_key(&mut state, &panels, "left", None);
        assert!(matches!(eff, PanelKeyEffect::SelectionChanged));
        assert_eq!(state.active, 0); // now on callers

        // Left on callers → cycle alternate backwards.
        let eff = panel_key(&mut state, &panels, "left", None);
        assert!(matches!(eff, PanelKeyEffect::SelectionChanged));
        assert_eq!(state.open[0].group_active[0], 2); // wraps from 0 to len-1

        // Right on callers → switch back to callees.
        let eff = panel_key(&mut state, &panels, "right", None);
        assert!(matches!(eff, PanelKeyEffect::SelectionChanged));
        assert_eq!(state.active, 1);
    }

    // ── sync_resets_on_rows_change ──

    #[test]
    fn sync_resets_on_rows_change() {
        let rows1 = vec![
            make_row("a.rs", "alpha"),
            make_row("b.rs", "bravo"),
            make_row("c.rs", "charlie"),
        ];
        let panels1 = vec![make_panel(0, "test", Dock::Float, rows1)];

        let mut state = PanelState::new();
        state.push("test".into(), 0);
        state.open[0].query = Some("".into());
        state.sync(&panels1);

        state.open[0].selection = 2;

        // Rows change (narrowed query) but query present → clamp.
        let rows2 = vec![make_row("a.rs", "alpha")];
        let panels2 = vec![make_panel(0, "test", Dock::Float, rows2)];
        let changed = state.sync(&panels2);
        assert!(changed);
        assert_eq!(state.open[0].selection, 0); // clamped
    }

    // ── layout ──

    #[test]
    fn panel_rect_matches_call_graph_columns() {
        let (left_l, _, w_l, _) = panel_rect(Dock::Left, 1920.0, 1080.0, Some((500.0, 900.0, 320.0)));
        let (left_r, _, w_r, _) = panel_rect(Dock::Right, 1920.0, 1080.0, Some((500.0, 900.0, 320.0)));
        let (cl, cr) = call_graph_column_lefts(500.0, 900.0, 320.0);
        assert_eq!(left_l, cl);
        assert_eq!(left_r, cr);
        assert_eq!(w_l, PANEL_COL_W);
        assert_eq!(w_r, PANEL_COL_W);
    }

    #[test]
    fn card_geometry_golden() {
        // 5 cards, selected=None.
        assert_eq!(card_top(0, None), 0.0);
        assert_eq!(card_top(1, None), PANEL_CARD_H + PANEL_CARD_GAP);
        assert_eq!(card_height(0, None), PANEL_CARD_H);

        // selected=0: first card is taller.
        assert_eq!(card_height(0, Some(0)), PANEL_SELECTED_H);
        assert_eq!(card_top(1, Some(0)), PANEL_SELECTED_H + PANEL_CARD_GAP);

        // selected=2: cards 0,1 are short, card 2 is tall.
        let t2 = card_top(2, Some(2));
        assert_eq!(t2, 2.0 * (PANEL_CARD_H + PANEL_CARD_GAP));
        assert_eq!(card_height(2, Some(2)), PANEL_SELECTED_H);
    }

    // ── parent_name ──

    #[test]
    fn parent_name_extracts_parent() {
        assert_eq!(parent_name("file.rs::Foo::bar"), Some("Foo".to_string()));
        assert_eq!(parent_name("file.rs::bar"), None);
        assert_eq!(parent_name("file.rs"), None);
    }

    // ── enter closes float ──

    #[test]
    fn enter_closes_float_panel() {
        let rows = vec![make_row("a.rs", "alpha")];
        let panels = vec![make_panel(0, "pal", Dock::Float, rows)];

        let mut state = PanelState::new();
        state.push("pal".into(), 0);
        state.sync(&panels);

        let eff = panel_key(&mut state, &panels, "enter", None);
        match eff {
            PanelKeyEffect::Enter(id) => assert_eq!(id, make_id("a.rs")),
            _ => panic!("expected Enter"),
        }
        assert!(state.open.is_empty());
    }

    // ── deps ──

    #[test]
    fn panel_deps_exclude_selection() {
        let rows = vec![make_row("a.rs", "alpha")];
        let panels = vec![make_panel(0, "test", Dock::Float, rows)];
        assert!(!panels[0].deps.intersects(outrider_view::Deps::SELECTION));
    }
}
