# 09 — Camera: framing, focus, follow, and steps (tours / history)

**Parent:** [../view-primitives.md](../view-primitives.md) §2.9 (definition), §2.10 (focus emphasis is render-only), §3.1 (steps push/pop layers), §4 (Enter/Esc/Alt+Left/Right/`last_child` decomposed as Camera), §5 (`camera` block), §6.2 (`frame` / `focus` / `home` / `tour` verbs), §8.9 (component spec), §9 milestones 2 (`frame`) and 7 (steps/tours).
**Framework:** [00-framework.md](00-framework.md) — `ViewSpec.camera: CameraSpec`, `ResolvedView.camera`, `Deps::{FOCUS, CAMERA, SPEC}`, `SessionState.focus`, `ViewCommand::Camera(CameraCommand)`, `ViewCommand::Tour(TourCommand)`, `apply`, `apply_view_command`.
**Depends on:** 02-set.md (a `SetRef` must resolve to ids before it can be framed).

## 1. Purpose and scope

The Camera primitive owns the viewport and nothing else: `Camera` (center/zoom), `Focus`, the follow tween, and **steps** — an ordered list of `{ target, layer diff }` played forward/back. A tour is an authored step list; navigation history is a recorded one (parent §2.9). Camera **never changes layout**: `PackLayout` is read, never written. The focused-leaf widening (`focused_width`, `expanded_leaf_bounds` in `treemap.rs`) is the render-only exception of parent §2.10 and stays exactly where it is; nothing in this spec adds a "resize" verb.

Design-doc intent this spec preserves (design doc §7.2): *arrows move the focus and the camera follows; Home/End and the mouse move the camera and the focus holds.* "Camera follows" becomes a spec field (`follow: focus`) so an agent can turn it off while it frames a set that is not the focus.

Out of scope: exploration *tree* history (design doc §7.4 — `NavigationHistory` stays linear), call-graph space camera, panels.

## 2. Ground truth: existing code touched

| file | symbol | ~line | what it does today | what changes |
|---|---|---|---|---|
| `crates/outrider/src/camera.rs` | `Camera { center_x, center_y, zoom }`, `world_to_screen`, `screen_to_world`, `pan`, `zoom_about`, `fit` | 9–70 | viewport math; `fit` = Home (5% margin) | unchanged |
| `camera.rs` | `FOCUS_FRACTION=0.5`, `END_FRACTION=0.95`, `TWEEN_SECS`, `MAX_ZOOM`, `frame_rect`, `frame_page` | 74–110 | framing helpers | add `pub const FRAME_FRACTION: f64 = 0.9` (set framing) |
| `camera.rs` | `CameraTween::{new, sample, done, retarget}`, `ease_in_out_cubic` | 125–165 | eased tween | unchanged |
| `crates/outrider/src/focus.rs` | `Focus { current, last_child }`, `land`, `record_visit`, `step_in`, `step_out`, `set` | 63–132 | focus cursor; `set` never moves the camera | unchanged |
| `focus.rs` | `spatial_step`, `neighbors`, `TreeIndex` | 152–220 | arrow targets | unchanged (TreeIndex moves per 00 §2.8) |
| `crates/outrider/src/navigation.rs` | `NavigationHistory { entries, cursor, capacity }`, `new/push/back/forward` | 4–46 | linear browser-style history; `push` truncates forward | add `entries()`, `cursor()`, `to_steps()` (§5.3) |
| `crates/outrider/src/treemap.rs` | `TreemapView.camera: Option<Camera>`, `home_zoom`, `tween`, `focus`, `nav_history` | 518–535 | camera state; `camera == None` means "re-fit Home on next paint" | add `tour: TourState`, `reframe_pending: bool`, `last_viewport: (f64,f64)` |
| `treemap.rs` | `PackingGeometryState::invalidate` | 580 | on repack snapshot/finish sets `camera=None`, `neighbors=None`, `hover=None` | also sets `*self.reframe_pending = true` |
| `treemap.rs` | `root_rect()` | 1270 | root rect for Home | unchanged; used by `CameraCommand::Home` |
| `treemap.rs` | `frame_below_headers`, `frame_focus(vw,vh,min,max) -> Option<Camera>` | 1301–1376 | camera target for the current focus (leaf: `frame_page` on expanded bounds; container: `frame_rect(FOCUS_FRACTION)`) | unchanged; called only from `follow_focus` |
| `treemap.rs` | `start_tween`, `cancel_tween` | 1381–1399 | tween start/retarget; mouse cancels | unchanged |
| `treemap.rs` | `paint_items` head | 1434–1446 | samples tween; if `camera.is_none()` → `Camera::fit(root_rect)` and sets `home_zoom` | after that block: consume `reframe_pending` (§5.5) |
| `treemap.rs` | `on_mouse_up` | 2019–2026 | click: `focus.set` + `nav_history.push`; **no camera move** | route through `set_focus_explicit(id, follow=false)` |
| `treemap.rs` | `on_palette_key` `"enter"` | 2222–2238 | `focus.set` + push + `frame_focus` + `start_tween` | → `set_focus_explicit(id, follow=true)` |
| `treemap.rs` | `on_nav_key` | 2268–2357 | `enter`→`step_in`; `escape`→`step_out`; both push history then `frame_focus`. `end`→`frame_rect(END_FRACTION)` of focus rect (camera only). `home`→`Camera::fit(root_rect)` (camera only). `tab`→call graph. `alt+left/right`→history back/forward, sets `focus.current` directly + `record_visit`, `neighbors=None`, then `frame_focus`. bare arrows→`spatial_step` + `focus.set` (no history push) then `frame_focus` | every `frame_focus(..)`+`start_tween` pair → `self.follow_focus()`; `home` → `self.enact_home()`; add PageUp/PageDown when tour active (§6.3) |
| `treemap.rs` | `exit_call_graph` | 2511–2525 | `focus.set(mode.center)` + push + frame | → `set_focus_explicit(center, follow=true)` |
| `treemap.rs` | `on_call_graph_key` `"enter"` | 2728–2749 | `focus.set(new_center)` + push + frame | → `set_focus_explicit(new_center, follow=true)` |
| `treemap.rs` | `start_loading` / `install_project_preview` | 2790, 2926 | reset `tween`, `focus`, `nav_history`, `camera=None` | also `tour = TourState::default()`, `reframe_pending=false` |
| `crates/outrider/src/layout_transition.rs` | `LayoutTransition::{new, sample, retarget, is_complete}` | 14–60 | rect interpolation during repack | unchanged; framing reads `packing_target_layout` when present (§5.4) |
| `crates/outrider-layout/src/pack.rs` | `Rect { x, y, w, h }`, `PackLayout.rects: BTreeMap<SymbolId, Rect>` | 17, 52 | world rects | read-only |

Existing key bindings found (for §6.3): global actions in `main.rs` L80–86 — `secondary-o` OpenFolder, `secondary-p` file palette, `secondary-t` symbol palette, `secondary-,` settings, `secondary-shift-,` project settings, `secondary-shift-e` reveal, `secondary-q` quit. `on_nav_key`: Enter, Escape, End, Home, Tab, Alt+Left, Alt+Right, Up/Down/Left/Right. Overlays: Escape (context menu, delete confirm, welcome), rename (Escape/Enter/Backspace/chars), settings draft (Escape/Tab/Backspace/chars), palette (Escape/Enter/Up/Down/Backspace/chars), call graph (Tab/Escape/Left/Right/Up/Down/Enter), project setup (Escape/Enter/Tab/Up/Down/Left/Right/Space). **PageUp/PageDown, `[`, `]`, `.` are unbound.**

## 3. Spec types (`crates/outrider-view/src/spec.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraSpec {
    /// One-shot intent: frame this set when the document is applied.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub frame: Option<SetRef>,
    /// One-shot intent: focus this symbol when the document is applied (applied after `frame`).
    #[serde(default, skip_serializing_if = "Option::is_none")] pub focus: Option<WireSymbolId>,
    #[serde(default)] pub follow: FollowMode,                 // default Focus
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub steps: Vec<Step>,
    /// Current step while a tour is active; None = not in a tour.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub step: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FollowMode { #[default] Focus, None }

/// One tour step. `target` is flattened so JSON reads `{ "frame": "changed", "push": [...] }`.
/// NOTE: `deny_unknown_fields` cannot be combined with `flatten`; unknown keys are caught by
/// `validate` (rule "camera.steps[n]: unknown field").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    #[serde(flatten)] pub target: StepTarget,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub push: Vec<LayerSpec>,
    #[serde(default, skip_serializing_if = "is_zero")] pub pop: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepTarget {
    Frame(SetRef),          // {"frame": "name" | {SetExpr}}
    Focus(WireSymbolId),    // {"focus": "fn:src/auth/login.rs::verify"}
    Home(HomeFlag),         // {"home": true}
}
/// Unit marker that (de)serializes as the boolean `true` (so `{"home": true}` works under flatten).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)] pub struct HomeFlag;
```

JSON (parent §5, unchanged shape):
```jsonc
"camera": {
  "frame": "affected", "follow": "none",
  "steps": [
    { "frame": "changed", "push": [ { "notes": [ { "at": "fn:src/a.rs::f", "source": "agent", "text": "start here" } ] } ], "note": "What changed" },
    { "focus": "fn:src/auth/login.rs::verify", "pop": 1, "push": [ { "mask": { "dimExcept": "affected" } } ] },
    { "home": true, "pop": 1 }
  ]
}
```

Commands (`crates/outrider-view/src/command.rs`, referenced from `ViewCommand::Camera` / `::Tour` in 00 §2.5):
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraCommand { Frame(SetRef), Focus(WireSymbolId), Home, Follow(FollowMode) }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TourCommand {
    Add(Step),              // append to camera.steps
    SetSteps(Vec<Step>),    // replace camera.steps (used by LoadHistory, tour file load)
    Play,                   // = snapshot layers, Goto(0)
    Next, Prev,             // Goto(index±1), clamped; Next past the end == Stop
    Goto(usize),
    Stop,                   // restore layer stack, camera.step = None
}
```
**Save / LoadHistory live outside `outrider-view`.** `tour save <file>` is CLI-side: `view.get` → write `camera.steps` (as `{ "outriderView": 1, "camera": { "steps": [...] } }`) to the file. `tour load-history` is an RPC method (`tour.loadHistory`) the app answers by `NavigationHistory::to_steps()` → `apply_view_command(Tour(SetSteps(steps)))`. Neither needs a pure-crate variant.

`validate` additions: `camera.step` ≥ `camera.steps.len()` → hard violation; a step whose `pop` exceeds the reachable stack is *soft* (saturating pop, warning); `Frame` refs must be known set names (same rule as layers).

## 4. Resolution (`crates/outrider-view/src/camera.rs`, new module; `pub mod camera;` in `lib.rs`)

Camera resolution is tiny and pure; the app owns the side effects.

```rust
use outrider_layout::{PackLayout, Rect};

/// Union rect of every member with a rect. Members with no rect (excluded, stale) are skipped.
/// None iff no member has a rect — the caller emits the warning and does nothing.
pub fn union_rect<'a>(ids: impl IntoIterator<Item = &'a SymbolId>, layout: &PackLayout) -> Option<Rect>;

/// Layer stack that should be active at step `n` (inclusive), starting from `base`
/// (the stack snapshotted at Play). For each step 0..=n: truncate by `pop` (saturating), then extend by `push`.
pub fn layers_at_step(base: &[LayerSpec], steps: &[Step], n: usize) -> Vec<LayerSpec>;

/// Minimal pop/push edit from `cur` to `target` (longest common prefix). Lets the app drive
/// the change through `apply(PopLayer)`/`apply(PushLayer)` so validation always runs.
pub fn diff_layers(cur: &[LayerSpec], target: &[LayerSpec]) -> (usize /*pops*/, Vec<LayerSpec> /*pushes*/);

/// Pure tour state machine (stored on TreemapView; see §5.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TourState { pub active: bool, pub index: usize, pub base: Vec<LayerSpec> }
```

`ResolvedView.camera` remains a plain copy of `spec.camera` (00 §2.4); the resolver does not act on it. `deps` for the camera block: `Deps::SPEC` only (it *causes* FOCUS/CAMERA; it depends on nothing live). Framing a `SetRef` reuses the set resolver: `ViewResolver::resolve_set_ref(&mut self, spec, ctx, r: &SetRef) -> &ResolvedSet` (02-set.md; listed under §11 in case that spec names it differently). No caching beyond the set cache; `union_rect` is O(|set|).

`apply` (command.rs) semantics for the pure part:
- `Camera(Follow(m))` → `spec.camera.follow = m`; `changed = SPEC`.
- `Camera(Frame|Focus|Home)` → **no spec mutation** (one-shot); `changed = NONE`. The app enacts (§5.1).
- `Tour(Add(s))` → push; `Tour(SetSteps(v))` → replace, `step = None`; `Tour(Goto(n))` → bounds-check (violation if `n >= steps.len()`), `spec.camera.step = Some(n)`; `Tour(Stop)` → `step = None`. `Play/Next/Prev` are rewritten by the app into `Goto`/`Stop` before reaching `apply` (they need `TourState`). All `changed = SPEC`.
- `Apply(spec)` / `Patch { camera: Some(..) }`: the incoming `frame`/`focus` are enacted once by the app immediately after `apply` returns without violations, then left in the spec (so `query view` shows the intent; re-resolution never re-enacts).

## 5. App integration (`crates/outrider/src/treemap.rs`, `view/camera_ctl.rs`)

New file `crates/outrider/src/view/camera_ctl.rs` holds the `impl TreemapView` block below (keeps `treemap.rs` from growing); it needs `pub(crate)` on `frame_focus`, `frame_below_headers`, `start_tween`, `root_rect`, `set_focus_explicit`.

New fields on `TreemapView`:
```rust
tour: outrider_view::camera::TourState,
/// Set by PackingGeometryState::invalidate; consumed in paint_items (§5.5).
reframe_pending: bool,
/// (vw, vh) of the last render — commands arriving off the key path (RPC) need a viewport.
last_viewport: (f64, f64),
```
`Self::map_viewport(window)` callers stay; `render` writes `self.last_viewport = (vw, vh)` each frame.

### 5.1 One entry point per verb

```rust
fn zoom_bounds(&self) -> (f64, f64) { ((self.home_zoom * 0.5).min(camera::MAX_ZOOM), camera::MAX_ZOOM) }

/// Camera follows focus iff view_spec.camera.follow == Focus. THE only caller of frame_focus.
pub(crate) fn follow_focus(&mut self) {
    if self.view_spec.camera.follow != FollowMode::Focus { return; }
    let (vw, vh) = self.last_viewport; let (lo, hi) = self.zoom_bounds();
    if let Some(to) = self.frame_focus(vw, vh, lo, hi) { self.start_tween(to); }
}

/// Explicit focus visit (click, palette, call graph, RPC focus, tour Focus step, Alt-history):
/// Focus::set, record in history unless `from_history`, refresh call-graph precompute, dirty FOCUS, follow.
pub(crate) fn set_focus_explicit(&mut self, id: SymbolId, follow: bool, from_history: bool) -> bool {
    let index = TreeIndex::new(&self.tree);
    let moved = if from_history { self.focus.current = id; self.focus.record_visit(&index); self.neighbors = None; true }
                else { self.focus.set(id, &index) };
    if !moved { return false; }
    if !from_history { self.nav_history.push(self.focus.current.clone()); }
    self.view_dirty = self.view_dirty.union(Deps::FOCUS);
    self.maybe_precompute_call_graph();
    if follow { self.follow_focus(); }
    true
}

pub(crate) fn enact_home(&mut self) {
    let (vw, vh) = self.last_viewport;
    let c = Camera::fit(self.root_rect(), vw, vh); self.home_zoom = c.zoom; self.start_tween(c);
}

/// Frame a set: resolve → union rect → frame_rect(FRAME_FRACTION) → tween. Empty → warning, no-op.
pub(crate) fn enact_frame(&mut self, set: &SetRef) {
    let ids: Vec<SymbolId> = { /* resolve via self.view_resolver.resolve_set_ref(..) with the same
        ResolveCtx paint_items builds (split borrows as there) */ };
    let layout = self.packing_target_layout.as_ref().unwrap_or(&self.layout);
    match outrider_view::camera::union_rect(ids.iter(), layout) {
        None => self.notifications.push(Notification::warning(format!("frame: set {set:?} has no laid-out members"))),
        Some(r) => { let (vw, vh) = self.last_viewport; let (lo, hi) = self.zoom_bounds();
                     let to = camera::frame_rect(r, vw, vh, camera::FRAME_FRACTION, lo, hi);
                     self.start_tween(to); }
    }
}

pub(crate) fn enact_camera(&mut self, cmd: &CameraCommand) {
    match cmd {
        CameraCommand::Frame(s) => self.enact_frame(s),
        CameraCommand::Focus(id) => { self.set_focus_explicit(id.0.clone(), true, false); }
        CameraCommand::Home => self.enact_home(),
        CameraCommand::Follow(_) => {}      // pure; already applied
    }
}
```
`enact_frame` does not touch focus (that is what `follow: none` + `frame` is for); with `follow: focus` an agent that frames a set and then presses an arrow gets the camera back on the focus — intended (design doc §7.2).

### 5.2 Tour state machine

```rust
pub(crate) fn enact_tour(&mut self, cmd: TourCommand) {
    use TourCommand::*;
    let n_steps = self.view_spec.camera.steps.len();
    match cmd {
        Add(_) | SetSteps(_) => { self.apply_view_command(ViewCommand::Tour(cmd)); }
        Play => { if n_steps == 0 { warn; return; }
                  self.tour = TourState { active: true, index: 0, base: self.view_spec.layers.clone() };
                  self.goto_step(0); }
        Next => if self.tour.active { if self.tour.index + 1 < n_steps { self.goto_step(self.tour.index + 1) } else { self.enact_tour(Stop) } } else { self.enact_tour(Play) },
        Prev => if self.tour.active && self.tour.index > 0 { self.goto_step(self.tour.index - 1) },
        Goto(n) => { if !self.tour.active { self.tour = TourState { active: true, index: 0, base: self.view_spec.layers.clone() }; } self.goto_step(n); }
        Stop => if self.tour.active {
            let base = std::mem::take(&mut self.tour.base);
            self.restore_layers(base);                                   // diff_layers → Pop/Push via apply
            self.apply_view_command(ViewCommand::Tour(Stop));            // step = None
            self.tour = TourState::default();
        },
    }
}

fn goto_step(&mut self, n: usize) {
    let target = layers_at_step(&self.tour.base, &self.view_spec.camera.steps, n);
    self.restore_layers(target);
    self.tour.index = n;
    self.apply_view_command(ViewCommand::Tour(TourCommand::Goto(n)));    // sets camera.step
    let step = self.view_spec.camera.steps[n].clone();
    match step.target {
        StepTarget::Frame(s) => self.enact_frame(&s),
        StepTarget::Focus(id) => { self.set_focus_explicit(id.0, true, false); }
        StepTarget::Home(_) => self.enact_home(),
    }
    self.tour_hud = step.note;      // Option<String>; rendered as a small pill next to the notifications
}

fn restore_layers(&mut self, target: Vec<LayerSpec>) {
    let (pops, pushes) = diff_layers(&self.view_spec.layers, &target);
    for _ in 0..pops { self.apply_view_command(ViewCommand::PopLayer); }
    for l in pushes { self.apply_view_command(ViewCommand::PushLayer(l)); }
}
```
Invariants: (a) `Stop` always returns `view_spec.layers` to the exact stack at `Play` — a *full snapshot* is kept, not just a count, because a step's `pop` may legally pop below the pre-tour stack (`layers_at_step` saturates and `restore_layers` re-pushes). (b) `Prev` is not "undo step n" but "recompute stack for n-1 from base" — so steps need no inverse and are always balanced. (c) A `PushLayer` that fails validation is dropped with a warning; the tour continues. (d) Any `Apply`/`Clear(Layers|All)` arriving mid-tour ends the tour first (`enact_tour(Stop)` before `apply`), so a stale `base` is never restored over a new document.

### 5.3 `NavigationHistory` → steps

`navigation.rs`:
```rust
pub(crate) fn entries(&self) -> &[SymbolId];
pub(crate) fn cursor(&self) -> usize;
/// Every visit becomes a Focus step: [{ focus: e0 }, { focus: e1 }, ...]; no layers, no notes.
pub(crate) fn to_steps(&self) -> Vec<outrider_view::Step> {
    self.entries.iter().map(|id| Step { target: StepTarget::Focus(WireSymbolId(id.clone())), push: vec![], pop: 0, note: None }).collect()
}
```
Consecutive duplicates are not possible (`Focus::land` no-ops on the same id). Bare-arrow moves are *not* in history today and stay out; only explicit visits (Enter/Esc/click/palette/call graph/RPC focus/tour Focus steps) are recorded — unchanged.

### 5.4 Layout transitions

Framing reads `packing_target_layout` when a `LayoutTransition` is in flight so the tween lands on final geometry rather than an interpolated frame (the visible layout catches up within the transition; both are `PackLayout`s so `union_rect` is unchanged). `frame_focus` keeps reading `self.layout` (behaviour-preserving; the follow tween is retargeted by later moves anyway).

### 5.5 Repack (TREE) re-frame

`PackingGeometryState` gets `reframe_pending: &'a mut bool`; `invalidate()` sets it. In `paint_items`, right after the `if self.camera.is_none() { fit … }` block:
```rust
if std::mem::take(&mut self.reframe_pending) && self.tour.active {
    let n = self.tour.index;                       // re-enact the current step's target on the new geometry
    match self.view_spec.camera.steps[n].target.clone() { Frame(s) => self.enact_frame(&s),
        Focus(id) => { let _ = self.set_focus_explicit(id.0, true, false); }, Home(_) => {} }
}
```
Outside a tour today's behaviour (camera re-fits Home after a repack) is preserved. Focus itself never dangles across a repack (`Focus` doc, focus.rs L61) so re-framing a `Focus` step is always valid; a `Frame` step whose set lost all rects degrades to the §5.1 warning.

### 5.6 Where the existing handlers change

- `on_nav_key`: `"enter"`/`"escape"` → after `step_in/step_out` succeed: `nav_history.push`, `view_dirty |= FOCUS`, `maybe_precompute_call_graph`, `self.follow_focus()`, `cx.notify()`, return (drop the `target` plumbing for these arms). `"alt+left/right"` → `set_focus_explicit(id, true, /*from_history*/ true)`. Bare arrows → `focus.set(next)`, dirty FOCUS, precompute, `follow_focus()`. `"home"` → `enact_home()`. `"end"` unchanged (camera-only, not follow). Add `"pageup"`/`"pagedown"` guarded by `self.tour.active` (§6.3).
- `on_palette_key "enter"`, `exit_call_graph`, `on_call_graph_key "enter"` → `set_focus_explicit(id, true, false)` (call-graph enter keeps its `CallGraphMode` rebuild after the call).
- `on_mouse_up` → `set_focus_explicit(id, false, false)` (click never moves the camera — unchanged).
- `apply_view_command` (00 §3.6) gains a post-step: `match &cmd { ViewCommand::Camera(c) => self.enact_camera(c), ViewCommand::Apply(s) => { enact s.camera.frame then s.camera.focus }, ViewCommand::Patch(p) if p.camera.is_some() => same, _ => {} }`. `Tour(*)` commands from RPC/keys enter through `enact_tour`, which calls `apply_view_command` only for the pure sub-steps (so there is no recursion: `apply_view_command` never calls `enact_tour`).

## 6. Commands

### 6.1 `ViewCommand` variants
`ViewCommand::Camera(CameraCommand::{Frame, Focus, Home, Follow})`, `ViewCommand::Tour(TourCommand::{Add, SetSteps, Play, Next, Prev, Goto, Stop})` — exactly the shapes in 00 §2.5.

### 6.2 RPC / CLI (parent §6.2, §6.4)
| CLI | RPC method (params) | app path |
|---|---|---|
| `outrider frame <set>` | `camera.frame { "set": SetRef }` | `apply_view_command(Camera(Frame(set)))` |
| `outrider focus <symbol>` | `camera.focus { "symbol": wire id }` | `apply_view_command(Camera(Focus(id)))` |
| `outrider home` | `camera.home {}` | `apply_view_command(Camera(Home))` |
| `outrider camera follow focus\|none` | `camera.follow { "mode" }` | `apply_view_command(Camera(Follow(m)))` |
| `outrider tour add <step.json>` / `play` / `next` / `prev` / `goto <n>` / `stop` | `tour.add {step}` / `tour.play` / `tour.next` / `tour.prev` / `tour.goto {n}` / `tour.stop` | `enact_tour(..)` |
| `outrider tour save <file>` | `view.get` (query) | CLI writes `camera.steps` to file |
| `outrider tour load <file>` | `tour.setSteps { steps }` | `apply_view_command(Tour(SetSteps))` |
| `outrider tour load-history` | `tour.loadHistory {}` | `Tour(SetSteps(nav_history.to_steps()))` |
| `outrider query camera` | `query.camera` | `{ center, zoom, focus, follow, step, steps.len }` |

The RPC server (10-rpc-and-watcher.md) queues decoded `ViewCommand`s / tour commands; the drain runs on the GPUI foreground inside `render` (or `poll_loading`'s tick), where `last_viewport` is fresh, and calls `apply_view_command` / `enact_tour`. Camera enactment therefore always happens on the GPUI thread with a real viewport; if `self.camera.is_none()` (first frame not yet rendered) `start_tween` is a no-op — enqueue-then-drain-after-first-paint handles it.

### 6.3 Keys (new)
| key | condition | action |
|---|---|---|
| `PageDown` / `PageUp` | `tour.active` (falls through to nothing otherwise, as today) | `Next` / `Prev` |
| `secondary-]` / `secondary-[` (new actions `TourNext`, `TourPrev` in `actions!`, bound in `main.rs`) | always; `Next` when inactive == `Play` | `Next` / `Prev` |
| `secondary-.` (`TourStop`) | `tour.active` | `Stop` |
No collision with the bindings listed in §2. Escape is deliberately not Stop (it is `step_out`); design doc §7.6 reserves PageUp/Down for peer stride "deferred", and while a tour is active stepping *is* the natural meaning.

## 7. Invalidation

Camera sets bits; it depends only on `SPEC`.
| event | bit | who |
|---|---|---|
| `set_focus_explicit`, arrow `focus.set`, `step_in/out` | `FOCUS` | the handler (00 §3.4 already requires this) |
| tween sample in `paint_items`, `enact_home/frame` start | `CAMERA` | `paint_items` / `start_tween` |
| `Camera(Follow)`, `Tour(Add/SetSteps/Goto/Stop)`, step Push/Pop | `SPEC` (from `Applied.changed`) | `apply_view_command` |
| repack (`apply_snapshot`/`finish`) | `TREE` (existing) + `reframe_pending` | `PackingGeometryState` |

## 8. Migration steps

1. `outrider-view`: add `CameraSpec`, `FollowMode`, `Step`, `StepTarget`, `HomeFlag` to `spec.rs`; `CameraCommand`, `TourCommand` to `command.rs` with the pure `apply` arms of §4; new `camera.rs` (`union_rect`, `layers_at_step`, `diff_layers`, `TourState`); validate rules. Unit tests (§9 a–d).
2. `camera.rs` (app): add `FRAME_FRACTION`. `navigation.rs`: `entries/cursor/to_steps`.
3. `treemap.rs`: add fields (`tour`, `reframe_pending`, `last_viewport`, `tour_hud`); write `last_viewport` in `render`; add `view/camera_ctl.rs` with `follow_focus`, `set_focus_explicit`, `enact_home`, `enact_frame`, `enact_camera`, `enact_tour`, `goto_step`, `restore_layers`.
4. **Behaviour-preserving rewire** (one commit, manual acceptance = no visible change): replace the eight `frame_focus`+`start_tween` sites (§2 table) with `follow_focus` / `set_focus_explicit`; `home` → `enact_home`. Session default view already has `follow: focus`.
5. `apply_view_command` post-step for `Camera` / `Apply` / `Patch{camera}`; `PackingGeometryState.reframe_pending`; `paint_items` reframe block.
6. Keys: `PageUp/PageDown` guard, `TourNext/TourPrev/TourStop` actions + bindings; tour HUD pill.
7. RPC/CLI verbs (with 10-rpc-and-watcher.md / 11-cli.md): `camera.*`, `tour.*`, `query.camera`, `tour save/load/load-history`.
8. Reset `tour`/`reframe_pending` in `start_loading` and `install_project_preview`.

Nothing is deleted; `frame_focus`, `frame_below_headers`, `start_tween`, `NavigationHistory` remain the engine.

## 9. Tests

`outrider-view` (in-crate, plus `mini_repo` via `outrider_index::index_repo` + `outrider_layout::pack` as in 00 §4):
- a. **serde**: the §3 JSON round-trips; `{"home": true}` parses to `StepTarget::Home`; `follow` defaults to `focus`; `step` beyond `steps.len()` is a hard violation.
- b. **`union_rect`**: on the `mini_repo` layout, `union_rect([src/lib.rs, src/util.rs])` equals the min/max envelope of the two rects and is contained in the `src` folder rect; empty iterator → `None`; ids with no rect are skipped (a set of one stale id → `None`).
- c. **steps balanced**: base `[F]`, steps `[{push:[A,B]}, {pop:1, push:[C]}, {pop:3}]` → `layers_at_step` gives `[F,A,B]`, `[F,A,C]`, `[]` (saturating); `diff_layers` between consecutive stacks pops/pushes exactly the expected count; `diff_layers(layers_at_step(base, steps, n), base)` returns to `base` for every `n` (this is what `Stop` does).
- d. **`apply`**: `Tour(Goto(n))` sets `camera.step`; `Tour(Stop)` clears it; `Camera(Follow(None))` flips the mode and reports `SPEC`; `Camera(Frame)` leaves the spec untouched and reports `Deps::NONE`.

App crate (`treemap.rs` tests already construct `PackingGeometryState` headlessly — extend those; `TreemapView` itself is not constructible without GPUI, so keep the logic in pure helpers):
- e. **empty-set frame is a no-op with warning**: extract `fn frame_target(ids, layout, vw, vh, lo, hi) -> Result<Camera, String>` used by `enact_frame`; empty → `Err`, non-empty → `frame_rect(FRAME_FRACTION)`.
- f. **follow modes**: `fn should_follow(spec: &CameraSpec) -> bool`; `Focus` → true, `None` → false (trivial but pins the contract), and a doc-test-style check that every handler in §5.6 calls `follow_focus` (grep-based test over `treemap.rs` for `frame_focus(` having exactly one caller — cheap regression guard).
- g. **history → steps**: `NavigationHistory::new(root).push(a).push(b).to_steps()` == `[Focus(root), Focus(a), Focus(b)]`; after `back()` the export still contains all entries (export is the full path, cursor ignored).
- h. **repack sets `reframe_pending`**: extend `packing_snapshot_only_retargets_geometry_and_invalidates_derived_state` (treemap.rs ~L4655) to assert the flag.

Manual acceptance: arrows/Enter/Esc/Alt-history/palette/call-graph behave identically; `outrider camera follow none` then arrows → focus ring moves, camera stays; `outrider frame <set>` tweens to the union; `tour play` on a 3-step file pushes/pops layers and `tour stop` leaves the layer list as before `play`.

## 10. Open questions / risks

1. **Frame padding.** `FRAME_FRACTION = 0.9` with `frame_rect` ignores the pinned-header stack (`frame_below_headers` is focus-relative). Acceptable for sets; revisit if agents frame single files often — could route single-member sets through `frame_focus`-style framing.
2. **`follow: none` and Enter/Esc.** With follow off, Enter/Esc/arrows move focus without moving the camera; the focus ring may go off-screen. Alternative: follow only when the new focus is off-screen. Start with the simple rule; it is what an agent asks for when it frames a set.
3. **`deny_unknown_fields` vs `flatten` on `Step`** — typos in a step are caught by `validate` not serde; error paths must be as good ("camera.steps[2]: unknown field `focu`"). Alternative: give `Step` an explicit `target` key and drop flatten (deviates from parent §5's JSON).
4. **History as a tree** (design doc §7.4) is not attempted; `to_steps` exports the linear path. When the tree lands, export the trunk (root→cursor).
5. **Live tour + RPC `Apply` races**: rule (d) in §5.2 (any `Apply/Clear` stops the tour) — confirm this is what an agent iterating on a tour file wants, or add `tour.reload` that keeps `active` and re-`Goto(index)`.

## 11. Framework deltas

- **New module** `crates/outrider-view/src/camera.rs` (`pub mod camera;`) — 00 §2.1's module list does not include it; add it (it is neither a layer nor a set).
- **`ViewResolver::resolve_set_ref(&mut self, &ViewSpec, &ResolveCtx, &SetRef) -> &ResolvedSet`** must exist (02-set.md); if 02 only exposes named-set resolution, add this method there.
- **`apply` may report `Deps::NONE`** for `Camera(Frame|Focus|Home)` (pure no-ops); 00 §2.5's `Applied.changed` doc says "SPEC, plus METRICS" — extend the comment.
- **`apply_view_command`** gains the post-step of §5.6 (calls `enact_camera` for `Camera`/`Apply`/`Patch{camera}`); signature unchanged. It relies on the new `TreemapView.last_viewport` field.
- 00 §2.5's inline comment lists `Step(..)` under `CameraCommand`; per this spec stepping is `TourCommand::Goto` — update the comment. `CameraSpec.step` is `Option<usize>` (parent §8.9 says `usize`); `frame` is `Option<SetRef>` (parent says `SetExpr`; `SetRef` admits both a name and an inline expression).
