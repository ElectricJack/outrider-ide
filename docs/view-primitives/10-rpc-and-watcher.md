# 10 — RPC server and view-file watcher

**Parent:** [../view-primitives.md](../view-primitives.md) §6.1 (three clients), §6.4 (transport), §8.10 (component), §10.6–10.7 (open questions: token file perms, live notifications).
**Depends on:** [00-framework.md](00-framework.md) (`ViewSpec`, `ViewCommand`, `ViewPatch`, `apply`, `Applied`, `Violation`, `Deps`, `apply_view_command`, `symbol_id` wire form). Camera verbs depend on `09-camera.md` (`CameraCommand`, `TourCommand`); set resolution for `query.*` depends on `02-set.md` (`SetResolver`). Neither existed when this was written — where a type from those files is used it is named exactly as the framework's `ViewCommand` enum names it, and any further assumption is flagged "(09/02 dep)".
**Consumed by:** [11-cli.md](11-cli.md) (the only client in this milestone).

---

## 1. Purpose and scope

Two ways for something outside the process to drive `ViewCommand`s in a running app:

1. **`crates/outrider/src/view/rpc.rs`** — a loopback TCP JSON-RPC 2.0 server. Clients discover the port and token through an *instance file* in the user cache dir. Requests are queued on an `mpsc` channel and drained on the GPUI foreground inside `impl Render`, exactly like `ProjectLoader`/`CallGraphResolver` results are today.
2. **`crates/outrider/src/view/watch.rs`** — a 500 ms poller of `<project>/.outrider/views/*.json`. When the newest file changes it is applied as `ViewCommand::Apply`. Zero infrastructure: a skill can drop a file and the app picks it up.

Both are behaviour-additive: nothing in the app changes unless a client speaks. Out of scope: MCP/ACP (parent §6.1 (3)), a `views` Panel listing the folder (hook noted in §4.4), named pipes / Unix sockets (parent §6.4 says later, behind the same trait — see §10).

---

## 2. Ground truth: existing code touched

| file | symbol | ~line | what it does today | what changes |
|---|---|---|---|---|
| `crates/outrider/src/main.rs` | `main`, `mod` list | L5–22, L53–75 | Declares modules; `TreemapView::loading_shell(repo, settings, cx)` inside `application().run` | Add `mod view;` (already required by 00-framework §3). No other change; server starts inside `loading_shell`. |
| `crates/outrider/src/treemap.rs` | `TreemapView` fields | L512–567 | Owns tree/layout/camera/loader/… | Add `rpc: Option<view::rpc::RpcServer>`, `view_watch: Option<view::watch::ViewWatcher>`, `wake: Arc<view::Wake>`, `_view_pump: Option<gpui::Task<()>>` (§5.1). |
| `treemap.rs` | `TreemapView::loading_shell` | L1162–1211 | Builds shell, starts loader / pre-scan | Start `RpcServer`, `ViewWatcher`, and the wake pump (`cx.spawn`) after `from_parts` (§5.2). |
| `treemap.rs` | `TreemapView::start_loading` | L2787–2806 | Resets per-project state, calls `loader.start` | If `folder != tree.repo_root` (Open Folder), drop and recreate `rpc`/`view_watch` for the new root (§5.2). |
| `treemap.rs` | `impl Render for TreemapView::render` | L4063–4117 | `advance_layout_transition / poll_loading / poll_call_graph / poll_pre_scan` → `cx.notify()`; `request_animation_frame` while animating/loading | Add `needs_notify \|= self.poll_rpc(window, cx); needs_notify \|= self.poll_view_watch();` (§5.3). No `request_animation_frame` for RPC — the pump handles wake-up. |
| `treemap.rs` | `poll_loading`, `poll_call_graph` | L2840, L2488 | The `try_recv` per-frame drain pattern | Copied, not changed. |
| `treemap.rs` | `on_key_down` `"home"` arm; `frame_focus`; `start_tween` | L2303, L1322, L1381 | Home framing = `Camera::fit(root_rect)`; focus framing + tween | Reused by `camera.home` / `camera.frame` (09 dep: 09-camera owns the app-side handlers; rpc only builds the `ViewCommand`). |
| `treemap.rs` | `TreemapView::apply_view_command` | (added by 00-framework §3.6) | `apply(cmd, &mut view_spec)`, ORs `Applied.changed` into `view_dirty`, pushes violations as `Notification::warning` | Called by `poll_rpc` and `poll_view_watch`; return value is the RPC error payload. |
| `crates/outrider/src/project_loader.rs` | `ProjectLoader::start_worker`, `poll` | L200–320 | `std::thread::spawn` + `mpsc::channel` + `try_recv` loop, generation-guarded | The model for `RpcServer`'s per-connection threads and `ViewWatcher`'s poll thread. |
| `crates/outrider/src/overlays.rs` | `Notification::warning`, `Notifications::push` | L26, L42 | Warning stack | Used for watcher parse/validation errors and RPC bind failure. |
| `crates/outrider/src/texture_store.rs` | `canonical_project_identity`, `namespace_derivation`, `Fnv1a` | L591–618, L791 | Canonical lowercase-on-Windows `/`-separated identity → FNV-1a 64 → 16-hex dir name under `<cache_dir>/outrider/textures/` | Extract `pub(crate) fn project_identity_hash(project_root: &Path) -> String` (16 hex chars) so instances and textures share one identity function. |
| `crates/outrider/src/project_settings.rs` | `project_settings_path` | L14 | `<root>/.outrider/project.json` | Same folder convention: watcher reads `<root>/.outrider/views/`. |
| `crates/outrider/src/settings.rs` | `Settings` | L35 | Persisted prefs | Add `rpc_enabled: bool` (default `true`) and `rpc_notifications: bool` (default `false`, parent §10.7). |
| `crates/outrider/Cargo.toml` | deps | — | `serde`, `serde_json`, `dirs`; no tokio/clap | No new deps. |

---

## 3. Spec types / wire formats

### 3.1 Instance file

Path: `dirs::cache_dir()/outrider/instances/<project_identity_hash>.json`, where `project_identity_hash` = FNV-1a 64 (`Fnv1a::field`) of the canonicalized project root, `\` → `/`, lowercased on Windows, printed `{:016x}` — identical to the texture namespace dir name.

```jsonc
{ "pid": 12345, "port": 51234, "token": "3f9a…(32 hex)", "project_root": "D:/src/foo",
  "started_at": "2026-08-17T10:11:12Z", "version": 1 }
```

```rust
#[derive(Serialize, Deserialize)]
pub struct InstanceFile { pub pid: u32, pub port: u16, pub token: String, pub project_root: String, pub started_at: String, pub version: u32 }
```
`started_at` is RFC-3339 UTC built from `SystemTime` by hand (no chrono). Written with `create_dir_all` + write to `<hash>.json.tmp` + rename (as `ProjectSettings::save` does). Removed in `Drop for RpcServer`. Clients must treat a file whose `pid` is dead as stale (11-cli.md §4.2).

### 3.2 Wire framing

Line-delimited JSON-RPC 2.0 over TCP. One JSON object per `\n`-terminated line, UTF-8, max 16 MiB per line (larger → close). Batches are not supported (array → `-32600`).

```jsonc
// request  →   {"jsonrpc":"2.0","id":1,"method":"view.patch","params":{...}}
// response ←   {"jsonrpc":"2.0","id":1,"result":{...}}
//          ←   {"jsonrpc":"2.0","id":1,"error":{"code":-32001,"message":"2 violation(s)","data":[Violation…]}}
// notif    ←   {"jsonrpc":"2.0","method":"focus.changed","params":{"symbol":"fn:src/a.rs::f"}}
```

First frame on every connection **must** be `{"jsonrpc":"2.0","id":0,"method":"auth","params":{"token":"…"}}`. Wrong/missing token → `{"error":{"code":-32003,"message":"unauthorized"}}` then close.

Error codes:

| code | meaning | `data` |
|---|---|---|
| -32700 | parse error (bad JSON line) | — |
| -32600 | invalid request (not an object, missing `method`, batch) | — |
| -32601 | method not found | — |
| -32602 | invalid params (serde error text in `message`) | — |
| -32001 | violation(s): the command was rejected by `validate` | `Vec<Violation>` serialised `{path, rule, message}` |
| -32002 | not ready: project still loading (`loader.is_loading()`), only for `query.*` and `camera.*` | `{ "phase": "Scanning|Parsing|BuildingTree|Packing" }` |
| -32003 | unauthorized (auth frame) | — |
| -32004 | unknown symbol / set name at query time (e.g. `query.metrics` on an id not in the tree) | `{ "symbol": "…" }` |

`Violation` gets `#[derive(Serialize)]` (framework delta §11, trivial).

### 3.3 Rust types (`crates/outrider/src/view/rpc.rs`)

```rust
use std::sync::{mpsc, Arc, atomic::{AtomicBool, Ordering}};

/// One inbound request, already parsed and authenticated.
pub struct RpcRequest {
    pub id: serde_json::Value,           // number | string | null (notification requests get null; we still answer)
    pub method: String,
    pub params: serde_json::Value,       // Null when absent
    pub reply: mpsc::Sender<Outbound>,   // this connection's writer queue
    pub conn: ConnId,                    // for subscribe bookkeeping
}
pub enum Outbound { Response(RpcResponse), Notification(RpcNotification), Close }
pub struct RpcResponse { pub id: serde_json::Value, pub result: Result<serde_json::Value, RpcError> }
pub struct RpcError { pub code: i32, pub message: String, pub data: Option<serde_json::Value> }
pub struct RpcNotification { pub method: &'static str, pub params: serde_json::Value }
pub type ConnId = u64;

/// Shared "something arrived" flag; the pump task checks it (§4.3).
pub struct Wake { pending: AtomicBool }
impl Wake { pub fn raise(&self); pub fn take(&self) -> bool; }

pub struct RpcServer {
    port: u16,
    token: String,
    instance_path: PathBuf,
    rx: mpsc::Receiver<RpcRequest>,
    subscribers: Arc<Mutex<HashMap<ConnId, (mpsc::Sender<Outbound>, SubscribeMask)>>>,
    shutdown: Arc<AtomicBool>,
    notify_enabled: bool,
}
impl RpcServer {
    /// Binds 127.0.0.1:0, writes the instance file, spawns the accept thread. Never panics; bind
    /// or write failure is returned so the app can `Notification::warning` it and run without RPC.
    pub fn start(project_root: &Path, wake: Arc<Wake>, notify_enabled: bool) -> Result<RpcServer, String>;
    pub fn port(&self) -> u16;
    /// Drains everything currently queued (never blocks).
    pub fn drain(&self) -> Vec<RpcRequest>;             // rx.try_iter().collect()
    /// App → subscribed clients (no-op unless notify_enabled). Cheap when nobody subscribed.
    pub fn publish(&self, n: RpcNotification);
    pub fn subscribe(&self, conn: ConnId, sender: mpsc::Sender<Outbound>, mask: SubscribeMask);
}
impl Drop for RpcServer { fn drop(&mut self) { self.shutdown.store(true, ..); let _ = fs::remove_file(&self.instance_path); /* connect-to-self to unblock accept() */ } }

#[derive(Clone, Copy, Default)] pub struct SubscribeMask { pub focus: bool, pub selection: bool, pub view: bool }
```

Token: 32 hex chars, no new dependency. `std::hash::RandomState::new()` is seeded from OS randomness (std's `hashmap_random_keys`); take two independent `RandomState`s, hash `(pid, SystemTime nanos, &listener as *const _ as usize, counter)` through each, and print both `u64`s as `{:016x}{:016x}`. Documented as "unguessable enough for a loopback-only server"; swapping in `getrandom` later is a one-line change in `fn make_token()`.

### 3.4 Watcher types (`crates/outrider/src/view/watch.rs`)

```rust
pub enum WatchEvent { Changed(PathBuf), Removed(PathBuf) }
pub struct ViewWatcher { rx: mpsc::Receiver<WatchEvent>, stop: Arc<AtomicBool>, dir: PathBuf }
impl ViewWatcher {
    /// Watches `<project_root>/.outrider/views/*.json`. The dir may not exist yet; it is re-checked every tick.
    pub fn new(project_root: &Path, wake: Arc<Wake>) -> ViewWatcher;
    pub fn drain(&self) -> Vec<WatchEvent>;
    pub fn dir(&self) -> &Path;
}
impl Drop for ViewWatcher { /* stop.store(true) */ }
/// Pure helper used by the thread and by tests: diff two mtime maps into events.
pub(crate) fn diff_mtimes(prev: &BTreeMap<PathBuf, SystemTime>, next: &BTreeMap<PathBuf, SystemTime>) -> Vec<WatchEvent>;
/// Newest file by mtime (ties: lexicographically last path) — the one the app applies.
pub(crate) fn newest(map: &BTreeMap<PathBuf, SystemTime>) -> Option<&PathBuf>;
```

---

## 4. Behaviour

### 4.1 Threads (server)

- **Accept thread** (`std::thread::Builder::new().name("outrider-rpc-accept")`): `listener.accept()` loop; on each socket set `TCP_NODELAY`, assign `ConnId` (counter), spawn a **reader thread** and a **writer thread**; stop when `shutdown` is set (Drop connects to its own port to unblock `accept`, then the loop observes the flag).
- **Writer thread** per connection: `for msg in out_rx { serialize + "\n"; write_all; flush; if Close → break }`. All responses and notifications go through it, so writes never interleave.
- **Reader thread** per connection: `BufReader::lines()`. State machine `AwaitingAuth → Authed`. In `AwaitingAuth` anything but a valid `auth` frame gets `-32003` + `Close`. In `Authed`: parse line → on parse error send `-32700` (id null) and continue; validate shape → `-32600`; `subscribe`/`unsubscribe` are handled *on the reader thread* (they touch only `subscribers`), reply `{"ok":true}`; every other method becomes `RpcRequest` sent on the shared `tx`, then `wake.raise()`. Reader exit (EOF/error) → send `Close` to writer, remove from `subscribers`.
- No thread ever touches `TreemapView`. `TreemapView` never blocks on a socket.

### 4.2 `poll_rpc` on the foreground

```rust
// treemap.rs (impl TreemapView)
fn poll_rpc(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
    let Some(rpc) = self.rpc.as_ref() else { return false };
    let reqs = rpc.drain();
    if reqs.is_empty() { return false; }
    for req in reqs {
        let result = self.dispatch_rpc(&req, window, cx);
        let _ = req.reply.send(Outbound::Response(RpcResponse { id: req.id.clone(), result }));
    }
    true
}
```

`dispatch_rpc(&mut self, req, window, cx) -> Result<Value, RpcError>` lives in `view/rpc_dispatch.rs` as `impl TreemapView` (keeps `treemap.rs` from growing) and is a `match req.method.as_str()` over §4.5. Two shapes:

- **Mutating** (`view.*`, `set.define`, `layer.*`, `metric.import`, `camera.*`, `tour.*`): parse params with `serde_json::from_value` (`-32602` on failure) → build the `ViewCommand` → `let violations = self.apply_view_command(cmd);` → empty ⇒ `Ok(json!({"ok":true, "changed": deps_names(applied)}))`, else `Err(RpcError{code:-32001, data: violations})`. `apply_view_command` already ORs `Applied.changed` into `view_dirty` and pushes `Notification::warning`s (00-framework §3.6); the RPC path deliberately reuses it so an agent's rejected command is *also* visible in the UI. `cx.notify()` is called once by `render` because `poll_rpc` returned true.
- **Query** (`query.*`): answered **synchronously from app state inside `poll_rpc`** — the tree, `layout`, `focus`, `camera`, `view_spec` are all `&self`; nothing needs a background thread. Cost is bounded: `query.symbols` is a linear scan of `TreeIndex`; `query.set` resolves one `SetExpr` (02 dep) with the same `ResolveCtx` `paint_items` builds (00-framework §3.2 — factor that construction into `fn resolve_ctx<'a>(&'a self, session: SessionState<'a>) -> ResolveCtx<'a>`); `query.callers/callees` call `outrider_index::call_graph::resolve_calls` through `self.call_graph_cache` (hit) or synchronously (miss — same cost the Tab key pays; acceptable for a CLI, capped by `depth ≤ 4`). If `self.loader.is_loading()` → `-32002` for every query and camera method (spec mutations are allowed while loading; they only touch `view_spec`).

### 4.3 Wake-up when idle

GPUI only re-renders on `cx.notify()`/input/`request_animation_frame`; a request arriving while the window is idle would sit in the channel until the next mouse move. `AsyncApp`/`Context` are `!Send` (they wrap `Rc<AppCell>`), so a std thread cannot call `notify` directly. Options considered:

1. `window.request_animation_frame()` every frame while the server is up — a permanent 60 Hz repaint. Rejected (battery, and `render` would rebuild the whole element tree constantly).
2. Blocking `rx.recv()` inside `cx.background_executor().spawn(..)` — parks a pool thread per server; the pool is shared with everything else GPUI does. Rejected.
3. **A foreground pump task** (chosen): a `cx.spawn` future that sleeps on `background_executor().timer(50 ms)` and, only when `wake.take()` is true, upgrades the `WeakEntity<TreemapView>` and `cx.notify()`s. Idle cost is one timer wake per 50 ms and an atomic load; latency ≤ 50 ms; no repaint unless something arrived.

```rust
// in TreemapView::loading_shell, after from_parts:
let wake = Arc::clone(&view.wake);
view._view_pump = Some(cx.spawn(async move |this: WeakEntity<TreemapView>, cx: &mut AsyncApp| {
    loop {
        cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
        if !wake.take() { continue; }
        // Entity gone → window closed → stop pumping.
        if this.update(cx, |_view, cx| cx.notify()).is_err() { break; }
    }
}));
```
`Task` must be held (`_view_pump` field) or it is cancelled on drop. `render` then runs `poll_rpc`/`poll_view_watch`, which drain the queues. Both `RpcServer` and `ViewWatcher` share the same `Arc<Wake>`; a stray raise with nothing queued costs one empty render — fine.

### 4.4 Watcher

Thread `outrider-view-watch`: every 500 ms (`thread::sleep`, checks `stop` between): `read_dir(dir)`, keep `*.json`, `metadata().modified()`, build `BTreeMap<PathBuf, SystemTime>`, `diff_mtimes(prev, next)` → send events, `wake.raise()` if any. Missing dir ⇒ empty map (so deleting the folder emits `Removed` for each file). Errors reading a single entry are skipped.

App side (`poll_view_watch(&mut self) -> bool`, called from `render`):
```rust
fn poll_view_watch(&mut self) -> bool {
    let Some(w) = self.view_watch.as_ref() else { return false };
    let events = w.drain(); if events.is_empty() { return false; }
    for e in &events { self.watch_state.record(e); }               // keeps its own mtime map
    let Some(path) = self.watch_state.newest().cloned() else { return true };
    if !events.iter().any(|e| matches!(e, WatchEvent::Changed(p) if *p == path)) { return true; } // an older file changed: ignore
    match fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| serde_json::from_str::<ViewSpec>(&s).map_err(|e| e.to_string())) {
        Ok(spec) => { let v = self.apply_view_command(ViewCommand::Apply(spec)); /* violations already notified */
                      if v.is_empty() { self.notifications.push(Notification::info(format!("Applied view {}", path.file_name()...))); } }
        Err(msg) => { if self.watch_state.retry_once(&path) { /* half-written file: try again next tick */ }
                      else { self.notifications.push(Notification::warning(format!("{}: {msg}", path.display()))); } }
    }
    true
}
```
Rules: **the newest-mtime file is the active one**; only a change to *it* (or a new newest) is applied; editing an older file does nothing (it becomes active if its mtime overtakes). Parse/validation errors → `Notification::warning` (a `Notification::info` level is added for the success toast; if 00-framework's `Notification` has no info level, use warning text "Applied view …" — the level is cosmetic). Retry-once handles editors that truncate-then-write. A file with `"outriderView"` ≠ 1 is a validation violation, reported the same way. **Hook for later**: `watch_state.files()` is exactly the row source for a `views` Panel (parent §6.1 (1)); the Panel spec can consume it without changing the watcher.

### 4.5 Method table

`sym` = wire `SymbolId` string (00-framework §2.7); `SetRef` = name string or inline `SetExpr` object; all params objects are `deny_unknown_fields`.

| method | params | result | errors |
|---|---|---|---|
| `auth` | `{token}` | `{ok:true, pid, project_root, version:1}` | -32003 then close |
| `subscribe` | `{focus?:bool, selection?:bool, view?:bool}` (default all true) | `{ok:true, enabled: <notify_enabled>}` | — |
| `unsubscribe` | `{}` | `{ok:true}` | — |
| `view.apply` | `{spec: ViewSpec}` | `{ok, changed:["SPEC","METRICS"]}` | -32602, -32001 |
| `view.patch` | `{patch: ViewPatch}` | same | -32602, -32001 |
| `view.get` / `query.view` | `{}` | `{spec: ViewSpec}` (the live document incl. session default layers) | — |
| `view.clear` | `{scope:"layers"\|"sets"\|"all"}` | `{ok}` | -32602 |
| `set.define` | `{name, expr: SetExpr}` | `{ok}` → `ViewCommand::DefineSet` | -32602, -32001 (unknown ref inside expr) |
| `layer.push` | `{layer: LayerSpec}` | `{ok, index}` (new length−1) | -32602, -32001 |
| `layer.pop` | `{}` | `{ok, remaining}` | -32001 if empty (rule `layers.empty`) |
| `layer.remove` | `{index}` | `{ok}` | -32602 out of range |
| `layer.list` | `{}` | `{layers:[{index, kind:"fill"\|…, summary}]}` | — |
| `camera.frame` | `{set: SetRef, padding?}` | `{ok, framed:n}` → `ViewCommand::Camera(CameraCommand::Frame(..))` (09 dep) | -32002, -32001 empty set (rule `camera.frame.empty`, soft → `ok:true, framed:0`) |
| `camera.focus` | `{symbol: sym}` | `{ok}` → `CameraCommand::Focus` | -32004 unknown |
| `camera.home` | `{}` | `{ok}` → `CameraCommand::Home` | -32002 |
| `camera.follow` | `{follow:"focus"\|"none"}` | `{ok}` → `CameraCommand::Follow` | -32602 |
| `tour.add` | `{step: Step}` | `{ok, steps:n}` → `ViewCommand::Tour(TourCommand::Add)` (09 dep) | -32001 |
| `tour.play` / `tour.next` / `tour.prev` | `{}` | `{ok, step:i, of:n}` | -32002 |
| `tour.save` | `{}` | `{steps:[Step]}` (client writes the file) | — |
| `metric.import` | `{name, metric: ImportedMetric}` | `{ok, matched:n, unmatched:[keys…] (≤50)}` → `ViewCommand::ImportMetric` | -32602, -32001 |
| `query.symbols` | `{glob?, kind?, in?: SetRef, limit?:u32 (default 1000)}` | `{symbols:[{id, name, kind, path, lines?:[a,b], measure, churn}], total}` | -32002 |
| `query.set` | `{set: SetRef, missing?:bool}` | `{ids:[sym], ranges:{sym:[[a,b]]}, missing:[sym] (if requested), deps:["FOCUS",…]}` | -32002, -32001 unknown name |
| `query.metrics` | `{symbol}` | `{symbol, metrics:[{metric, value, percentile, basis, unit?}]}` over `MetricRegistry` | -32004 |
| `query.callers` / `query.callees` | `{symbol, depth?:1..4}` | `{edges:[{from,to,rawName,callSite?:[a,b],depth}]}` | -32004, -32002 |
| `query.changed` | `{rev?: "HEAD~1"\|"a..b"\|"worktree"}` | `{ids:[sym], files:[path]}` (`SetExpr::Changed`, 02 dep) | -32002 |
| `query.focus` | `{}` | `{symbol, path, lines?}` | — |
| `query.camera` | `{}` | `{centerX, centerY, zoom, homeZoom}` — read-only, coordinates *out* only (parent §6.3 forbids taking them) | — |
| `query.selection` | `{}` | `{symbol?: sym, panel?: index}` | — |

Notifications (only if `settings.rpc_notifications` and the connection subscribed):

| method | params | emitted from |
|---|---|---|
| `focus.changed` | `{symbol}` | wherever `view_dirty \|= FOCUS` is set (00-framework §3.4 table) — call `self.rpc_publish(..)` in the same helper |
| `selection.changed` | `{symbol?}` | panel row move (08 dep) |
| `view.changed` | `{source:"rpc"\|"watch"\|"ui", changed:[…]}` | end of `apply_view_command` when `Applied.changed ≠ NONE` |

Coalescing: `publish` is called from the foreground at most once per frame per kind (keep a `pending_notifs: SubscribeMask` on `TreemapView`, flush at the end of `render`).

### 4.6 Security notes

- Loopback only (`127.0.0.1`), never `0.0.0.0`; no TLS. Token in every session's first frame; wrong token closes.
- Instance file lives under the per-user cache dir (`%LOCALAPPDATA%\outrider\instances` / `~/.cache/outrider/instances`). On Unix, create with `0o600` (`OpenOptions::mode`); on Windows the default ACL of `%LOCALAPPDATA%` is user-private — document, don't fight ACLs. Multi-user shared machines are parent §10.6's open question; not solved here.
- Reader caps: 16 MiB line, 64 in-flight requests per connection (further reads block until the queue drains — natural back-pressure via a bounded `sync_channel(64)` for `tx`).
- No method takes a filesystem path the app would read (`metric.import` sends values inline; `view.apply` sends the spec inline). The watcher reads only inside `<root>/.outrider/views/`.

### 4.7 End-to-end sequence: `outrider mask --dim-except s`

```
CLI                                   accept/reader/writer threads             GPUI foreground (TreemapView)
 │ read <cache>/outrider/instances/<h>.json → {port, token}
 │ TcpStream::connect(127.0.0.1:port)
 │ ─── {"id":0,"method":"auth","params":{"token":T}}\n ──▶ reader: verify T   
 │ ◀── {"id":0,"result":{"ok":true,...}}\n ────────────── writer
 │ ─── {"id":1,"method":"layer.push","params":{"layer":{"mask":{"dimExcept":"s","strength":0.8}}}}\n ──▶
 │                                       reader: parse → tx.send(RpcRequest) → wake.raise()
 │                                                                            pump task (≤50 ms): wake.take() → this.update(cx.notify)
 │                                                                            render(): poll_rpc → drain → dispatch_rpc
 │                                                                              → ViewCommand::PushLayer(Mask{..}) → apply_view_command
 │                                                                              → validate: set "s" exists? yes → view_dirty |= SPEC
 │                                                                              → req.reply.send(Response{ok,index:4})
 │ ◀── {"id":1,"result":{"ok":true,"index":4}}\n ────────── writer            render(): cx.notify() → next frame paints mask
 │ print "ok" (or JSON), exit 0
```
If `s` were undefined: `validate` returns `[{path:"layers[4].mask.dimExcept", rule:"set.unknown", message:"unknown set 's'"}]`, spec untouched, response `error -32001` with that array, CLI exits 1.

---

## 5. App integration

### 5.1 New fields (`TreemapView`, after `pre_scanner`)

```rust
/// Loopback JSON-RPC server (parent §6.4). None if bind failed or settings.rpc_enabled == false.
rpc: Option<crate::view::rpc::RpcServer>,
/// `.outrider/views/*.json` poller.
view_watch: Option<crate::view::watch::ViewWatcher>,
watch_state: crate::view::watch::WatchState,      // mtime map + retry flags, foreground-owned
/// Shared wake flag raised by rpc/watch threads, consumed by the pump task.
wake: std::sync::Arc<crate::view::Wake>,
_view_pump: Option<gpui::Task<()>>,
pending_notifs: crate::view::rpc::SubscribeMask,
```

### 5.2 Start / restart

In `loading_shell` (L1162) after `from_parts` and before `start_loading`/`pre_scanner.start`: `view.start_view_services(cx)`:
```rust
fn start_view_services(&mut self, cx: &mut Context<Self>) {
    let root = self.tree.repo_root.clone();
    if self.settings.rpc_enabled {
        match RpcServer::start(&root, Arc::clone(&self.wake), self.settings.rpc_notifications) {
            Ok(s) => self.rpc = Some(s),
            Err(e) => self.notifications.push(Notification::warning(format!("RPC disabled: {e}"))),
        }
    }
    self.view_watch = Some(ViewWatcher::new(&root, Arc::clone(&self.wake)));
    if self._view_pump.is_none() { self._view_pump = Some(/* §4.3 */); }
}
```
In `start_loading(folder)` (L2787): if `folder != self.tree.repo_root` set `self.rpc = None; self.view_watch = None;` (Drop removes the instance file) and call `start_view_services` again *after* `self.tree.repo_root` is updated by `install_project_preview` (L2910) — simplest: do it in `install_project_preview` when `project_root` differs from the server's root (`RpcServer::project_root()`).

### 5.3 `render`

At L4069–4075:
```rust
let mut needs_notify = self.advance_layout_transition(Instant::now());
needs_notify |= self.poll_loading();
needs_notify |= self.poll_call_graph(window);
needs_notify |= self.poll_pre_scan();
needs_notify |= self.poll_view_watch();          // before rpc: a file apply then an rpc patch composes in arrival order
needs_notify |= self.poll_rpc(window, cx);
if needs_notify { cx.notify(); }
```
and at the very end of `render`, `self.flush_rpc_notifications()`.

### 5.4 New files

- `crates/outrider/src/view/mod.rs` — add `pub mod rpc; pub mod rpc_dispatch; pub mod watch; pub struct Wake;`.
- `crates/outrider/src/view/rpc.rs` — §3.3, §4.1, framing, token, instance file (≈350 lines).
- `crates/outrider/src/view/rpc_dispatch.rs` — `impl TreemapView { fn dispatch_rpc(..) }`, param structs (`#[derive(Deserialize)] #[serde(rename_all="camelCase", deny_unknown_fields)]`), result builders.
- `crates/outrider/src/view/watch.rs` — §3.4, §4.4 (`WatchState` included).
- `crates/outrider/src/texture_store.rs` — export `project_identity_hash`.
- `settings.rs` — two bools; Settings window gets a "Allow CLI/agent control (local RPC)" checkbox (optional in this milestone; default true).
- `Cargo.toml` — nothing.

---

## 6. Commands (mapping to `ViewCommand`)

| method | `ViewCommand` |
|---|---|
| `view.apply` | `Apply(spec)` |
| `view.patch` | `Patch(patch)` |
| `view.clear` | `Clear(ClearScope::{Layers,Sets,All})` |
| `set.define` | `DefineSet{name, expr}` |
| `layer.push` / `layer.pop` / `layer.remove` | `PushLayer(l)` / `PopLayer` / `RemoveLayer(i)` |
| `metric.import` | `ImportMetric{name, metric}` |
| `camera.frame/focus/home/follow` | `Camera(CameraCommand::Frame(SetRef)/Focus(id)/Home/Follow(..))` (09) |
| `tour.*` | `Tour(TourCommand::Add(step)/Play/Next/Prev/Save)` (09; `Save` returns steps rather than writing) |
| watcher file change | `Apply(spec)` |
| `query.*`, `view.get`, `layer.list`, `subscribe` | none (read-only) |

Everything mutating goes through `TreemapView::apply_view_command` — no second code path.

---

## 7. Invalidation

RPC and watcher never set `Deps` bits themselves; `apply_view_command` ORs `Applied.changed` (`SPEC`, plus `METRICS` for `metric.import`/`view.apply` with metrics/`view.patch` with metrics) into `view_dirty`. Camera commands set `FOCUS`/`CAMERA` through the existing focus/tween paths (09). `view.get`/`query.*` set nothing. `subscribe` sets nothing. The watcher's `Apply` is `SPEC | METRICS?` like any apply.

---

## 8. Migration steps

1. Extract `project_identity_hash` from `texture_store.rs`; add unit test that it equals the texture namespace dir name.
2. Add `view::Wake`, `view/watch.rs` (`diff_mtimes`, `newest`, `WatchState`, thread) + tests.
3. Add `view/rpc.rs`: framing (`parse_line`, `encode`), token, instance file, accept/reader/writer threads, `drain`, `publish`, Drop.
4. Add `Serialize` to `Violation` (framework delta).
5. Add `settings.rpc_enabled/rpc_notifications`.
6. Add fields (§5.1), `start_view_services`, pump task, `poll_rpc`, `poll_view_watch`, render wiring (§5.3).
7. `rpc_dispatch.rs`: `auth`/`view.*`/`set.define`/`layer.*`/`query.view|focus|camera` first (no 02/09 dependency); then `query.symbols/metrics/callers/callees` (needs `MetricRegistry`, call graph); then `query.set/changed`, `camera.*`, `tour.*` as 02/09 land — return `-32601` until then.
8. Manual acceptance: start app, `type %LOCALAPPDATA%\outrider\instances\*.json`, `nc`/`ncat 127.0.0.1 <port>` and paste an auth + `view.get` line; drop a view file into `.outrider/views/` and see it apply; close app → instance file gone.

---

## 9. Tests

All in-crate (`#[cfg(test)]`), no GPUI:

- **Framing**: `parse_line` accepts the request shape, rejects arrays (`-32600`), bad JSON (`-32700`), missing method; `encode(Response)` round-trips and ends with `\n`; error `data` carries `Vec<Violation>` serialised.
- **Auth + dispatch with a fake handler**: bind `RpcServer::start(tempdir, wake, false)`; connect with `TcpStream`; (a) first frame not auth → `-32003` and EOF; (b) wrong token → same; (c) correct auth then `{"method":"echo"}` → the test drains `server.drain()`, replies via `req.reply` with `result: params`, client reads it back; (d) `wake.take()` was true after the send; (e) two connections interleaved get their own responses (ConnId isolation); (f) 3 requests before the app drains → all delivered in order.
- **Subscribe/publish**: subscribe on conn A only; `publish(focus.changed)` → A receives a notification line, B does not; `notify_enabled=false` → nobody receives.
- **Instance file lifecycle**: after `start`, `<cache>/outrider/instances/<hash>.json` exists with matching `pid`/`port`/`token`; `drop(server)` removes it and the port refuses connections; `project_identity_hash` is stable across `\`/`/` and case on Windows. Use a `cache_root` override (`RpcServer::start_at(cache_root, …)`) so tests don't touch the real cache dir.
- **Watcher**: with a tempdir, `diff_mtimes` yields `Changed` for new/modified and `Removed` for deleted; `newest` picks max mtime, tie → last path; end-to-end: create `a.json`, sleep > 500 ms, create `b.json` → `drain()` yields `Changed(b)` and `WatchState::newest() == b`; touching `a.json` afterwards is *not* applied (assert via `WatchState::should_apply(&events)`); dir absent at start then created → first files reported.
- **Dispatch (pure)**: `dispatch_rpc` is `impl TreemapView` and needs a `Context`; test the pure param parsers and result builders (`parse_params::<ViewPatchParams>`, `symbols_result(..)`) instead, plus one violation path via `outrider_view::command::apply` on a `ViewSpec` to confirm the `-32001` payload shape.

---

## 10. Open questions / risks

- **Latency**: 50 ms pump granularity is fine for a CLI; a live agent streaming commands would prefer a real wake. If needed later, replace the timer with the reader thread calling a platform "post empty event" — GPUI has no public one at the pinned rev, hence the pump.
- **`query.callers` on a cache miss** blocks the frame for a `resolve_calls` (re-parses the file). Acceptable; if it shows up, answer `-32002 {phase:"resolving"}` and let the CLI retry once the `cg_resolver` result lands.
- **Multiple instances on the same root** (two windows): the second overwrites the instance file. Decision: last writer wins; `Drop` only removes the file if its `pid` is still ours.
- **Named pipes / Unix sockets**: keep `Transport` as an internal trait `{ accept() -> Box<dyn Read+Write+Send> }` so `TcpListener` is swappable without touching the reader/writer code.
- **02/09 dependency**: `query.set/changed`, `camera.*`, `tour.*` are stubbed `-32601` until those specs land; the method table above is the contract they must satisfy.

---

## 11. Framework deltas

1. `Violation` needs `#[derive(Serialize)]` (and `Deserialize` for the CLI to print it typed).
2. `TreemapView::apply_view_command` must return the `Vec<Violation>` *and* the `Applied.changed` bits (or make `Applied` `pub` and return it) so `view.apply` can report `changed`. Suggested signature: `fn apply_view_command(&mut self, cmd: ViewCommand) -> outrider_view::command::Applied`.
3. Factor the `ResolveCtx` construction in `paint_items` into `TreemapView::resolve_ctx(&self, session) -> ResolveCtx` so `query.set` can reuse it.
4. `Notification::info` (a second level) — optional; only for the "Applied view x.json" toast.
