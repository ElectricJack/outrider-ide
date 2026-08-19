# 11 — CLI: `outrider-cli`

**Parent:** [../view-primitives.md](../view-primitives.md) §6.2 (verbs), §6.3 (design rules), §6.4 (transport), §6.5 (skill contract), §8.11 (component).
**Depends on:** [00-framework.md](00-framework.md) (`ViewSpec`, `ViewPatch`, `ViewCommand`, `SetExpr`, `SetRef`, `LayerSpec`, `ImportedMetric`, `Violation`, `symbol_id::{to_wire, parse_wire}`), [10-rpc-and-watcher.md](10-rpc-and-watcher.md) (instance file, framing, method table, error codes). Set flags depend on the `SetExpr` variants of `02-set.md`; camera/tour verbs on `09-camera.md` — both absent at time of writing; the JSON forms used below are the parent §5 forms, which those specs mirror.

---

## 1. Purpose and scope

A thin, GPUI-free binary that (a) turns imperative verbs into `ViewPatch`/`ViewCommand` JSON and sends them to the running app over the RPC of spec 10, and (b) answers `query …` either from the running app or — with no instance — by indexing the repo directly, the way `outrider-dump` does. It is the first client of parent §6.1 and the tool a Claude Code skill shells out to (§6.5).

**Where the binary lives.** Parent §6.2 writes `outrider view …`. The `outrider` binary links GPUI, cosmic-text, image, rfd — a CLI must not pay that link time or pull a window system on a headless CI box. Decision:

- New crate **`crates/outrider-cli`**, binary **`outrider-cli`**. Verbs are the parent's verbs without the leading word: `outrider-cli set s --glob 'src/auth/**'`, `outrider-cli view apply f.json`, `outrider-cli query symbols`.
- **The `outrider` app binary dispatches** to it: in `main.rs`, before `application().run`, if `argv[1]` is one of the CLI verbs (`view set fill mask edges mark note panel frame focus home tour metric layer query`) **and** `Path::new(argv[1]).is_dir()` is false, exec the sibling `outrider-cli` (same directory as `current_exe()`, or `PATH`) with `argv[1..]` and exit with its status. Then `outrider mask --dim-except s` works as the parent shows, GPUI is never initialised, and a folder literally named `set` still opens with `outrider ./set`. Shell users who only install the CLI can `alias outrider=outrider-cli`.

Out of scope: MCP/ACP wrappers; TUI; anything that reads screen coordinates.

---

## 2. Ground truth: existing code touched

| file | symbol | ~line | today | change |
|---|---|---|---|---|
| `Cargo.toml` (workspace) | `members` | L3–7 | index, layout, outrider | add `"crates/outrider-view"` (00-framework) and `"crates/outrider-cli"` |
| `crates/outrider/src/main.rs` | `main` | L53–63 | `argv[1]` = repo path or `rfd` picker | insert the verb dispatch (§5.2) before the path/picker logic |
| `crates/outrider-index/src/bin/outrider-dump.rs` | `main` | L7–15 | `index_repo(&root, &[], &[])` → `dump::render` | pattern copied for offline mode; binary untouched |
| `crates/outrider-index/src/index.rs` | `index_repo`, `index_repo_outcome` | L104–127 | full pipeline with extension/folder filters | called offline; filters read from `.outrider/project.json` |
| `crates/outrider-index/src/call_graph.rs` | `resolve_calls(&SymbolId,&SymbolTree)` | L21 | per-symbol caller/callee resolution | offline `query callers/callees` |
| `crates/outrider/src/project_settings.rs` | `ProjectSettings {filter_extensions, filter_folders, filter_files, max_display_lines}` | L4–12 | app-crate serde struct | CLI re-declares a `serde(default)` mirror `ProjectFilters` (three fields) — the app crate cannot be a dependency; note in §10 to move it into `outrider-index` later |
| `crates/outrider/src/world.rs`, `content.rs` | `pack_config`, `PAGE_W=640`, `LINE_STEP=15.6`, `HEADER=20.8`, `BOTTOM_PAD=6`, `PACK_ASPECT=1.0` | world.rs L26–45, content.rs L10–16 | app-owned `PackConfig` constants | offline mode needs a `PackLayout` for `ResolveCtx`; add `outrider_layout::PackConfig::outrider_default(gap: f64, max_display_lines: Option<u64>)` with these constants and make `world::pack_config` call it (framework delta §11) |
| `crates/outrider/src/texture_store.rs` | `project_identity_hash` (added by spec 10) | — | 16-hex FNV-1a of canonical root | CLI re-implements the *same* function (`instance::project_identity_hash`) — 20 lines; a shared test fixture (`docs/…/fixtures/identity-hash.txt`: `input → hash`) keeps them equal |

---

## 3. Spec types / wire formats

### 3.1 Crate

```toml
# crates/outrider-cli/Cargo.toml
[package] name = "outrider-cli"  version = "0.1.0"  edition = "2021"
license.workspace = true  repository.workspace = true  rust-version.workspace = true
[[bin]] name = "outrider-cli"  path = "src/main.rs"
[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
globset = "0.4"
dirs = "6"
anyhow = "1.0"
outrider-view = { path = "../outrider-view" }
outrider-index = { path = "../outrider-index" }
outrider-layout = { path = "../outrider-layout" }
[dev-dependencies]
tempfile = "3"
assert_cmd = "2"        # optional; tests can also call `run(args) -> Exit` directly
```
No tokio, no reqwest: transport is `std::net::TcpStream` + `BufReader::lines()`.

Modules:
```
crates/outrider-cli/src/
  main.rs        clap tree → run(Cli) -> ExitCode
  cli.rs         #[derive(Parser)] types (§4.1)
  instance.rs    discovery + InstanceFile + project_identity_hash + pid liveness
  rpc.rs         Client { call(method, params) -> Result<Value, RpcFailure> }
  compile.rs     verbs → SetExpr / LayerSpec / ViewPatch / ViewCommand (§4.2–4.4)   ← pure, tested
  offline.rs     index_repo + pack + SetResolver for query verbs without an instance (§4.6)
  output.rs      human vs --json printers; Violation printer
  exit.rs        ExitCode consts
```

### 3.2 Exit codes and failure type

```rust
pub const OK: u8 = 0; pub const VIOLATION: u8 = 1; pub const NO_INSTANCE: u8 = 2; pub const BAD_ARGS: u8 = 3;
pub const RPC_ERROR: u8 = 4;      // any other JSON-RPC error (-32002 not ready, -32004 unknown symbol, -32601 …)

pub enum RpcFailure { NoInstance(String), Violations(Vec<Violation>), Rpc { code: i32, message: String, data: Option<Value> }, Io(std::io::Error) }
```
`clap` reports usage errors with exit 2 by default; use `Cli::try_parse()` and map `clap::Error` → print to stderr → `BAD_ARGS (3)` so 2 stays "no instance" (§4.5).

### 3.3 Output contract

- Without `--json`: one human line per result (`ok`, `ok · layer 4`, `applied 3 layers, 2 sets`); tables for `query`; violations as `error: layers[4].mask.dimExcept — unknown set 's' (set.unknown)`.
- With `--json` (global flag, also `OUTRIDER_JSON=1`): exactly one JSON document on stdout. Success = the RPC `result` verbatim. Violation = `{"error":"violation","violations":[Violation…]}` on stdout, exit 1. No instance = `{"error":"no_instance","message":"…"}` on stderr, exit 2.
- Errors always name the invariant (`Violation.rule` + `message`), per parent §6.3.

---

## 4. Behaviour

### 4.1 Verb table (clap tree)

Global flags: `--project <dir>`, `--json`, `--offline` (force offline for `query`), `--timeout <ms>` (default 5000), `-q/--quiet`.

| verb | args / flags | compiles to (§6) | RPC method |
|---|---|---|---|
| `view apply <file>` | `-` = stdin | `ViewSpec` parsed locally (`serde_json`), then sent | `view.apply {spec}` |
| `view patch <file>` | | `ViewPatch` | `view.patch {patch}` |
| `view clear [--layers\|--sets\|--all]` | default `--layers` | scope | `view.clear {scope}` |
| `view get [-o file]` | | — | `view.get` → prints `spec` pretty (this is how a session becomes a `.outrider/views/x.json`) |
| `set <name> <set-flags…>` | §4.2 | `SetExpr` | `set.define {name, expr}` |
| `fill <metric> [--channel fill\|stripe\|opacity] [--scale percentile\|log\|linear=a,b\|threshold=a,b,c] [--domain <set>] [--ramp <id>]` | default channel `fill`, scale `percentile` | `LayerSpec::Fill` | `layer.push` |
| `mask --dim-except <set> \| --dim <set> [--strength 0.8]` | exactly one of the two | `LayerSpec::Mask` | `layer.push` |
| `edges <relation> [--within <set>] [--incident-to <set>] [--cross-boundary folder\|partition] [--direction up\|down\|any] [--min-weight n] [--style solid\|dashed]` | | `LayerSpec::Edges` | `layer.push` |
| `mark <target> --kind <k> [--label] [--basis]` | target = set name \| `sym` \| `sym:40-52` (lines) | `LayerSpec::Marks` (`on` = `SetRef::Name` or `[{symbol, lines}]`) | `layer.push` |
| `note <target> <text>` | target = `sym` \| `sym:a-b`; `-` reads text from stdin | `LayerSpec::Notes([{at, source:"agent", text}])` — source is **always** `agent` | `layer.push` |
| `panel <set> [--columns churn,fanin] [--sort-by m] [--dock left\|right\|bottom\|float] [--title t]` | | `LayerSpec::Panel` | `layer.push` |
| `frame <set> [--padding px]` | | | `camera.frame {set}` |
| `focus <symbol>` | | | `camera.focus {symbol}` |
| `home` | | | `camera.home` |
| `follow focus\|none` | | | `camera.follow` |
| `tour add [--frame <set>\|--focus <sym>\|--home] [--push <layer.json>…] [--pop n] [--note t]` | | `Step` | `tour.add {step}` |
| `tour play\|next\|prev` | | | `tour.play/next/prev` |
| `tour save <file>` | | writes `{steps}` result to file | `tour.save` |
| `metric import <name> <file> --basis "…" [--key path\|item\|file:line] [--unit u]` | json or csv (§4.4) | `ImportedMetric` | `metric.import {name, metric}` |
| `layer list` | | | `layer.list` |
| `layer pop` / `layer rm <index>` | | | `layer.pop` / `layer.remove {index}` |
| `query symbols [--glob g] [--kind k] [--in <set>] [--limit n]` | | | `query.symbols` / offline |
| `query set <name-or-expr-flags> [--missing]` | `<name>` or the §4.2 flags for an ad-hoc set | | `query.set` / offline |
| `query metrics <symbol>` | | | `query.metrics` / offline (built-in metrics only) |
| `query callers\|callees <symbol> [--depth n]` | | | `query.callers/callees` / offline |
| `query changed [rev]` | | | `query.changed` / offline (needs git) |
| `query view` | | | `query.view` (online only) |
| `query focus\|camera\|selection` | | | online only |
| `status` | | prints instance file fields + `auth` result | `auth` |

### 4.2 `set` flag grammar → `SetExpr`

Flags are grouped into **primaries** (each yields one `SetExpr`), **expanders**, and **algebra**. Composition rule, in order:

1. `base = intersect(primaries…)`; a single primary is used as-is; zero primaries ⇒ `--from <name>` is required (then `base = {"ref": name}`) or error `BAD_ARGS`.
2. Each `--reach REL:DIR:DEPTH` (repeatable) → `base = union(base, reach{from: base, relation, direction, depth})`. `--reach-only` replaces instead of unions. `DEPTH` = integer or `inf`.
3. `--union a,b` → `union(base, ref a, ref b)`; `--intersect a,b`; `--minus a,b` → `minus(base, union(refs))` (`SetExpr::Minus` takes `[a, b]`: `a \ b`).
4. `--not` → `not(base)`.

| flag | `SetExpr` JSON |
|---|---|
| `--glob <pat>` (repeatable → union of globs) | `{"glob": "src/auth/**"}` |
| `--kind <k>` (repeatable → union) | `{"kind": "fn"}` |
| `--fuzzy <q>` | `{"fuzzy": "login"}` |
| `--where '<metric> <op> <value>'` (repeatable → intersect) | `{"where": {"metric":"churn","op":">","value":"p90"}}`; ops `> >= < <= == !=`; value `pNN` or number |
| `--changed [rev]` (`HEAD~1`, `a..b`, `worktree`; bare flag = `worktree`) | `{"changed": "HEAD~1"}` |
| `--ids a,b,c` or repeated | `{"ids": ["fn:src/a.rs::f", "src/b.rs"]}` (bare paths allowed, 00-framework §2.7) |
| `--ref <name>` / `--from <name>` | `{"ref": "name"}` |
| `--community <partition>:<id>` / `--layer <partition>:<name>` | `{"community": {"partition":"…","id":"…"}}` |
| `--children <name>` / `--ancestors` / `--descendants` / `--file-of` | `{"children": {"ref": "name"}}` … |
| `--neighbors` | `{"neighbors": "focus"}` |
| `--visible` | `{"visible": true}` |
| `--reach calls:in:2` | `{"reach": {"from": <base>, "relation":"calls", "direction":"in", "depth":2}}` |
| `--union a,b` / `--intersect a,b` / `--minus a` / `--not` | `{"union":[…]}` / `{"intersect":[…]}` / `{"minus":[base, …]}` / `{"not": base}` |
| `--expr '<json>'` | verbatim `SetExpr` (escape hatch; mutually exclusive with everything else) |

Examples: `set A --changed HEAD~1 --reach calls:in:2` ⇒ `{"union":[{"changed":"HEAD~1"},{"reach":{"from":{"changed":"HEAD~1"},"relation":"calls","direction":"in","depth":2}}]}` (the diff-flow set of parent §4). `set tests --glob '**/tests/**' --glob '**/*_test.rs'` ⇒ `{"union":[{"glob":…},{"glob":…}]}`. `set hot --where 'complexity > p90' --where 'churn > p90'` ⇒ `{"intersect":[…]}`.

`compile::set_expr(&SetArgs) -> Result<SetExpr, String>` is pure and unit-tested (§9).

### 4.3 Symbol targets

`compile::target(s) -> Result<Target, String>`: `name` with no `:` and not containing `/` or `.` → `SetRef::Name`; otherwise `symbol_id::parse_wire` (or bare path); optional `:a-b` or `:a` suffix → `{"symbol": sym, "lines": [a, b]}`. Ambiguity (`--kind`-less bare word that is also a file) is resolved by the app at resolution time; the CLI does not consult the tree.

### 4.4 `metric import` parsing

`--key` selects how the first column is interpreted (`ImportedMetric` values are keyed by string; the app's importer maps `path`, `path::item`, `file:line` — parent §7.3):

- **JSON**: `{"src/a.rs": 12.5, "src/a.rs::f": 0}` or `[{"key":"src/a.rs","value":12.5}]` or `{"values":{…},"basis":"…"}` (basis inside the file is overridden by `--basis`).
- **CSV**: header optional; two columns `key,value`; `#` lines skipped; numbers parsed as `f64`; a bad row aborts with `BAD_ARGS` naming the line.
- Result: `ImportedMetric { basis, unit, values: BTreeMap<String,f64>, key: "path"|"item"|"file:line" (if 03-fill defines the field; else the CLI normalises `file:line` keys itself only when it is online-with-tree, otherwise passes through) }`. `--basis` is mandatory (parent §2.3); missing ⇒ `BAD_ARGS` with message "metric import requires --basis (every imported metric must carry its provenance)".

### 4.5 Instance discovery and connection (`instance.rs`, `rpc.rs`)

1. `root = --project` or walk up from `cwd` until a dir containing `.outrider/` or `.git/` (prefer the first `.outrider/`; else the first `.git/`; else cwd). Canonicalize.
2. `path = dirs::cache_dir()/outrider/instances/<project_identity_hash(root)>.json` (spec 10 §3.1). Missing ⇒ `NoInstance("no running outrider for <root>; start the app or use --offline")`.
3. Parse `InstanceFile`; if `pid` is not alive (Windows: `OpenProcess`-free check = try `TcpStream::connect` first; if refused, treat as stale, delete the file, `NoInstance("stale instance file removed")`), connect with `--timeout`.
4. Send `auth`; a `-32003` ⇒ `NoInstance("token rejected — instance file is stale")`.
5. `Client::call(method, params) -> Result<Value, RpcFailure>`: write one line, read lines until a response with the matching `id` (skip notifications), map `error.code == -32001` → `Violations`, else `Rpc{..}`.

Environment overrides for CI: `OUTRIDER_INSTANCE=<path-to-instance.json>`, `OUTRIDER_RPC=127.0.0.1:port` + `OUTRIDER_TOKEN`.

### 4.6 Offline `query` (`offline.rs`)

Used when there is no instance (or `--offline`). Only `query symbols|set|metrics|callers|callees|changed`.

```rust
pub struct Offline { tree: SymbolTree, layout: PackLayout, index: TreeIndex, metrics: MetricRegistry, relations: RelationRegistry, root: PathBuf }
impl Offline {
    pub fn load(root: &Path) -> anyhow::Result<Offline> {
        let f: ProjectFilters = read `.outrider/project.json` if present (serde default otherwise);
        let tree = outrider_index::index_repo(root, &f.filter_extensions, &f.filter_folders)?;   // like outrider-dump.rs L12
        let layout = outrider_layout::pack(&tree, &PackConfig::outrider_default(8.0, f.max_display_lines));
        let index = TreeIndex::new(&tree); let metrics = MetricRegistry::builtin(&tree); let relations = RelationRegistry::builtin();
        Ok(..)
    }
    pub fn ctx(&self, head: Option<&str>) -> ResolveCtx<'_> {
        ResolveCtx { tree, index, layout, metrics, relations, repo_root: &self.root,
            session: SessionState { focus: &self.tree.root.id, hover: None, selection: None, visible: None, head, neighbors: None } }
    }
}
```
`head` = `git rev-parse HEAD` if git is available. **Unavailable offline** (documented in `--help` and returned as `{"error":"offline_unsupported"}` exit 4): `query view/focus/camera/selection`; any set using `neighbors`, `visible`, `$focus/$hover/$selection` pseudo-ids (resolves to empty with a warning on stderr — 00-framework §2.6 soft rule); imported metrics (`query metrics` lists built-ins only); `filter_files` (index-time file filter is honoured only if `ProjectFilters` carries it — it does, pass through `index_repo_outcome_impl` if a public entry exists, else note the difference in output: `"filters": "extensions+folders"`). Every offline result includes `"source": "offline"` under `--json` so a skill can tell.

Offline load of a large repo takes seconds; print `indexing <root>…` to stderr unless `--json`/`-q`.

---

## 5. App integration

### 5.1 Workspace / Cargo

- Root `Cargo.toml` members: `+ "crates/outrider-view", + "crates/outrider-cli"`.
- No change to `crates/outrider/Cargo.toml` (the app does not depend on the CLI).
- `crates/outrider-layout`: add `impl PackConfig { pub fn outrider_default(gap: f64, max_display_lines: Option<u64>) -> Self }` with the constants from `world.rs`/`content.rs` (framework delta).

### 5.2 `main.rs` dispatch (≈20 lines, before L54)

```rust
const CLI_VERBS: &[&str] = &["view","set","fill","mask","edges","mark","note","panel","frame","focus","home","follow","tour","metric","layer","query","status"];
fn maybe_dispatch_cli() {
    let mut args = std::env::args_os().skip(1);
    let Some(first) = args.next() else { return };
    let Some(verb) = first.to_str() else { return };
    if !CLI_VERBS.contains(&verb) || std::path::Path::new(verb).is_dir() { return; }
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(if cfg!(windows) {"outrider-cli.exe"} else {"outrider-cli"})))
        .filter(|p| p.exists()).unwrap_or_else(|| "outrider-cli".into());
    match std::process::Command::new(exe).arg(verb).args(args).status() {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => { eprintln!("outrider: cannot run outrider-cli: {e}"); std::process::exit(2); }
    }
}
```
Called as the first line of `main()`. `scripts/build-windows.sh` (and the release packaging) must ship `outrider-cli(.exe)` next to `outrider(.exe)`.

### 5.3 New files

All of `crates/outrider-cli/` (§3.1). Nothing else in the app.

---

## 6. Commands (mapping to `ViewCommand`)

Parent §6.3: every mutating verb is expressible as a `patch`. The CLI therefore has exactly two payload shapes for layer verbs and one for the rest:

| verb | payload | app-side `ViewCommand` |
|---|---|---|
| `view apply` | `ViewSpec` | `Apply` |
| `view patch` | `ViewPatch` | `Patch` |
| `view clear` | scope | `Clear(scope)` |
| `set` | `{name, expr}` | `DefineSet` |
| `fill` / `mask` / `edges` / `mark` / `note` / `panel` | one `LayerSpec` via `layer.push` | `PushLayer(layer)` |
| `layer pop` / `layer rm` | — / index | `PopLayer` / `RemoveLayer` |
| `metric import` | `{name, metric}` | `ImportMetric` |
| `frame` / `focus` / `home` / `follow` | set / symbol / — | `Camera(CameraCommand::…)` (09) |
| `tour add/play/next/prev/save` | step / — | `Tour(TourCommand::…)` (09) |
| `--patch-only` (global) | *any* mutating verb prints its `ViewPatch` JSON to stdout instead of sending — the "CLI is sugar" guarantee made checkable, and how a skill writes a `.outrider/views/*.json` incrementally | none |

`fill` uses `layer.push` (append; last-wins in resolution) rather than replacing an existing fill layer, so `layer pop` restores the previous fill (00-framework §2.5).

---

## 7. Invalidation

The CLI sets no `Deps`; the app's `apply_view_command` does (spec 10 §7). The CLI *reads* the `changed` list from `view.apply/patch` results and prints it in `--json` output so a skill can see whether it invalidated `METRICS`.

---

## 8. Migration steps

1. Add `PackConfig::outrider_default` to `outrider-layout`; make `world::pack_config` delegate.
2. Create `crates/outrider-cli` with `cli.rs` (full clap tree, all verbs — camera/tour verbs may map to methods that return `-32601` until 09 lands), `exit.rs`, `output.rs`.
3. `compile.rs`: `set_expr`, `target`, `layer_spec_for(verb)`, `metric_import`; tests (§9).
4. `instance.rs` + `rpc.rs`: discovery, auth, `call`; test against a fake server (a `TcpListener` thread that speaks the framing of spec 10 §3.2 — reuse the same test helper shape as spec 10 §9 or a copy).
5. `offline.rs`: `Offline::load/ctx`; `query symbols/callers/callees` first (no 02 dep), then `query set/changed/metrics` as `SetResolver`/`MetricRegistry` land.
6. `main.rs` dispatch in the app; packaging script ships both binaries.
7. Docs: `outrider-cli --help` text conventions — verb summary line ≤ 60 chars, every flag's help names the primitive it maps to (`--dim-except <SET>  Mask: dim everything not in SET`), examples section per verb, `EXIT CODES` section in the top-level help.

---

## 9. Tests

`crates/outrider-cli/src/compile.rs` (unit, no I/O):

| args | expected `SetExpr` JSON |
|---|---|
| `--glob 'src/**'` | `{"glob":"src/**"}` |
| `--glob a --glob b` | `{"union":[{"glob":"a"},{"glob":"b"}]}` |
| `--glob 'src/**' --kind fn` | `{"intersect":[{"glob":"src/**"},{"kind":"fn"}]}` |
| `--changed HEAD~1 --reach calls:in:2` | union of changed and reach-from-changed (§4.2 example) |
| `--from A --reach calls:out:inf --reach-only` | `{"reach":{"from":{"ref":"A"},"relation":"calls","direction":"out","depth":null}}` (∞ = `null`, or whatever 02-set chooses — one place to change) |
| `--where 'churn > p90'` | `{"where":{"metric":"churn","op":">","value":"p90"}}` |
| `--where 'churn >> 3'` | `Err` mentioning the op |
| `--ids a,b --minus C --not` | `{"not":{"minus":[{"ids":["a","b"]},{"ref":"C"}]}}` |
| `--neighbors --visible` | `{"intersect":[{"neighbors":"focus"},{"visible":true}]}` |
| (no primaries, no `--from`) | `Err("set needs at least one selector")` |
| `--expr '{"kind":"fn"}' --glob x` | `Err` (exclusive) |

Also: `target("s")` → Name; `target("fn:src/a.rs::f:40-52")` → `{symbol, lines:[40,52]}`; `note` always yields `source:"agent"`; `mask` with both `--dim` and `--dim-except` → `BAD_ARGS`; `fill --scale threshold=0,50,100` → `{"threshold":[0,50,100]}`; `metric import` CSV with a bad row → error naming line 3; `--basis` missing → error text contains "provenance".

`tests/offline_query.rs` (integration, uses `crates/outrider-index/tests/fixtures/mini_repo`, copied to a tempdir):
- `query symbols --kind fn --json` lists `fn:src/lib.rs::free`, `fn:src/lib.rs::inner::helper`, `fn:src/util.rs::clamp`, `fn:src/lib.rs::Point::new`, `fn:src/lib.rs::Point::norm` (exact set = whatever `index_test.rs` asserts; reuse its expectations), each with `source:"offline"`.
- `query symbols --glob 'src/util.rs'` → the file + `clamp`.
- `query callees fn:src/lib.rs::free` → `fn:src/lib.rs::Point::new` (matches `call_graph` tests).
- `query set --glob 'src/**' --kind fn --json` (02 dep) → same ids as the symbols query.
- `query focus` offline → exit 4 with `offline_unsupported`.

`tests/exit_codes.rs` (fake server on `TcpListener` in-process, instance file written into a temp `cache_root` via `OUTRIDER_INSTANCE`):
- Server answers `mask --dim-except nope` with `-32001` + one `Violation` → CLI exit **1**, stdout `--json` has `violations[0].rule == "set.unknown"`, human mode prints `error: … (set.unknown)`.
- No instance file → exit **2**, message names the root.
- `mask --dim a --dim-except b` → exit **3** (clap `try_parse` path).
- `query view` when server returns `-32002` → exit **4**, message "not ready (Packing)".
- `--patch-only mask --dim-except s` prints `{"layers":[{"mask":{"dimExcept":"s","strength":0.8}}]}` and never connects.

Worked transcripts (kept in `docs/view-primitives/examples/cli-diff-flow.md`, checked into docs not tests):

```
$ outrider set changed --changed HEAD~1
ok · set "changed"
$ outrider set affected --from changed --reach calls:in:2
ok · set "affected"
$ outrider mask --dim-except affected --strength 0.8
ok · layer 4 (mask)
$ outrider edges calls --within affected
ok · layer 5 (edges)
$ outrider frame affected
ok · framed 17 symbols
$ outrider note fn:src/auth/login.rs::verify:40-52 "This now calls into billing; previously auth had no edge to billing."
ok · layer 6 (notes)
$ outrider view get -o .outrider/views/what-this-pr-touched.json
wrote .outrider/views/what-this-pr-touched.json (6 layers, 4 sets)
$ outrider query set affected --json | jq '.ids | length'
17
$ outrider mask --dim-except nope
error: layers[7].mask.dimExcept — unknown set 'nope' (set.unknown)
$ echo $?
1
```
The saved file is exactly the parent §5 shape and re-applies through the watcher (spec 10 §4.4) or `outrider view apply`.

---

## 10. Open questions / risks

- **`outrider` dispatch vs a directory named like a verb** — resolved by `is_dir()`; document `outrider ./query` for the pathological case.
- **Windows PID liveness** without `winapi`: we rely on "connect refused ⇒ stale". A live *other* process on a reused port would fail at `auth` (token mismatch) ⇒ also reported as stale. Good enough.
- **Duplicated `ProjectFilters` and `project_identity_hash`** in the CLI crate: move `ProjectSettings` and the identity hash into `outrider-index` (no GPUI there) in a follow-up; both are ≤ 40 lines and covered by the shared fixture.
- **Offline `changed`/`where` cost** — full index per invocation. Fine for a skill that runs a handful of queries; if it becomes a loop, add `outrider_index`'s disk cache (`index_repo_outcome_with_cache`) keyed by the same cache root the app uses.
- **02/09 dependency**: `set` flag → `SetExpr` variant names and the `depth: ∞` encoding must match 02-set.md; camera/tour param names must match 09-camera.md. The tables here are the CLI's contract; adjust field names there, not the verb grammar.
- **`--json` and stderr progress**: skills parse stdout only; all progress/notes go to stderr.

---

## 11. Framework deltas

1. `outrider_layout::PackConfig::outrider_default(gap, max_display_lines)` — move the five layout constants out of the app crate so the CLI can build the same `PackLayout` (`world::pack_config` becomes a one-line delegate). Purely additive.
2. `Violation: Serialize + Deserialize` (also requested by spec 10).
3. `SetExpr`, `LayerSpec`, `ViewPatch`, `ImportedMetric` must be constructible from the CLI crate without a tree (they are plain serde types per 00-framework §2.2 — confirm no `#[serde(skip)]` fields with non-`Default` types).
4. Optional: `MetricRegistry::builtin(&tree)` and `RelationRegistry::builtin()` must not require app-crate types (00-framework §3.5 places them in `outrider-view`; keep it so).
