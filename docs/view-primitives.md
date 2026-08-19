# View Primitives — Design and Implementation Spec

**Status:** v0.1 draft
**Companion to:** [code-comprehension-viewer-design.md](code-comprehension-viewer-design.md) (the "design doc"). Section references of the form §N.M without a prefix point into this document; "design doc §N.M" points into the companion.
**Scope:** The small, fixed vocabulary of view building blocks that every Outrider view is composed from; how they fit the pan/zoom world; how the current features decompose into them; the data schema; the command surface an agent drives them through; and the implementation of the framework and each component.

---

## 0. How to read this document

Part I (§1–§6) is design: what the primitives are and why exactly these. Part II (§7–§9) is the implementation overview: types, module boundaries, render passes, invalidation, per-component sketches, and the milestone order. §10 lists open questions.

**Detailed implementation specs** live alongside this file in [`view-primitives/`](view-primitives/), one per component, each grounded in the current code and written so an implementer can work from it directly. Read [00-framework.md](view-primitives/00-framework.md) first; it is the contract every other spec builds on.

| Spec | Covers | Parent §s |
|---|---|---|
| [00-framework.md](view-primitives/00-framework.md) | `outrider-view` crate, `ViewSpec`, resolver, `Deps`, commands, validation, `PaintOverrides`, session default view, app wiring | §7 |
| [01-space.md](view-primitives/01-space.md) | Space: treemap, exclusion, regroup, callgraph/matrix stubs | §2.1, §8.1 |
| [02-set.md](view-primitives/02-set.md) | Set: `SetExpr` grammar, resolver, live sets, `changed`, git watcher, partitions | §2.2, §8.2 |
| [03-fill.md](view-primitives/03-fill.md) | Fill + metric registry, scales, imported metrics, inspect | §2.3, §8.3 |
| [04-mask.md](view-primitives/04-mask.md) | Mask: dim/spotlight, container inheritance, scrim replacement | §2.4, §8.4 |
| [05-edges.md](view-primitives/05-edges.md) | Edges + relation registry, aggregation, edge pass, call-graph migration | §2.5, §8.5 |
| [06-marks.md](view-primitives/06-marks.md) | Marks: glyphs, range marks, nav rings | §2.6, §8.6 |
| [07-notes.md](view-primitives/07-notes.md) | Notes: doc/metric/agent provenance, narration style, callouts | §2.7, §8.7 |
| [08-panel.md](view-primitives/08-panel.md) | Panel: docked/floating lists, palette + call-graph column migration | §2.8, §8.8 |
| [09-camera.md](view-primitives/09-camera.md) | Camera: frame/focus/follow, steps and tours | §2.9, §8.9 |
| [10-rpc-and-watcher.md](view-primitives/10-rpc-and-watcher.md) | JSON-RPC server, instance file, `.outrider/views/` watcher | §6.1, §6.4, §8.10 |
| [11-cli.md](view-primitives/11-cli.md) | `outrider-cli` verbs, flag grammar, offline query | §6.2–6.3, §8.11 |

The single sentence to keep in mind:

> **A view is a Space plus an ordered stack of Layers plus a Camera, all expressed as data keyed on symbols and anchors, never on coordinates. Layers paint; only the Space owns geometry.**

Everything below is a consequence of that sentence.

---

# Part I — Design

## 1. Thesis

### 1.1 Why a fixed vocabulary

The earlier brainstorm produced a long list of views (boundary edges, dependency matrix, community overlay, entry-point ribbon, coverage overlay, memory hotspots, diff-flow, guided tours, …). Programming each one is open-ended work and each is a bespoke code path. The alternative is to identify the handful of things every one of those views actually *does* to the canvas and expose those as composable primitives. Then a view is a document, not a feature; and an agent — or a user, or a script — composes new views without anyone writing Rust.

Nine primitives are enough. §4 demonstrates it by decomposing both the brainstormed views and every view the app has today.

### 1.2 The three roles

- **Space** owns geometry: which deterministic layout places each symbol in world space, and how much fidelity a box gets at a given on-screen size. There is exactly one active space.
- **Layers** paint over the space: color, dimming, lines, glyphs, text, side panels. Layers may read geometry; they may never write it.
- **Camera** owns the viewport: what is framed, what is focused, and recorded/authored sequences of viewpoints.

This is the same separation the design doc makes between geometry (design doc §5.4 layer 1), structural overlays (layer 2), and narration (layer 3) — restated so that an external agent can only ever touch layers 2 and 3, and inside layer 2 only in ways that decompose to a number.

### 1.3 Invariants inherited from the design doc

Every primitive must respect design doc §9. The ones that bite here:

- **Determinism / continuity (§9.1–2):** layers cannot move boxes. The one exception is focus emphasis (§2.10), which is a screen-space render override and never enters `PackLayout`.
- **Anchors everywhere (§9.4):** a view document refers to `SymbolId`s and byte ranges within a symbol, never to world or screen coordinates.
- **Assess/explain split (§9.6):** structural channels (fill, heat, edge weight, glyphs) are driven only by numbers with an inspectable basis; narration is text-only and visually marked. An agent may *choose* which metric drives fill and may *import* measured data, but cannot assign a color to a symbol by opinion.
- **Every signal inspectable (§9.7):** any painted value must be able to answer `metric · value · basis · threshold`.

### 1.4 What "agent-constructed views" buys

With the primitives in place the app gains a command surface (§6). An agent, prompted with a question, can query ground truth from the same index the map is drawn from, build sets, paint them, frame them, and narrate — the closed loop the design doc calls the ACP layer (design doc §3), but reachable first through a plain CLI, and later through MCP/ACP over the same internal command API.

---

## 2. The nine primitives

Each entry: definition · what it may own · what it may not do · the canonical parameters.

### 2.1 Space

**Definition.** A deterministic function from `(SymbolTree, config) → world rect per symbol`, plus the level-of-detail ladder that decides how much of a symbol to draw at a given on-screen size, plus an exclusion set of symbols not laid out at all.

**Owns.** Geometry (`PackLayout`), fidelity (`Rung` / `LeafDraw`), textures/minimaps, layout transitions (`LayoutTransition`), and `exclude`.

**May not.** Read layers. A space's output depends only on the tree, its config, and its exclusion set.

**Kinds.**
| id | status | places symbols by |
|---|---|---|
| `treemap` | exists | folder hierarchy, stable-ordered packing (`outrider_layout::pack`) |
| `treemap` + `regroup` | planned | same packer, but the hierarchy is a supplied partition (community, layer, owner) instead of folders. A regrouped treemap is a *distinct* space id (`treemap@<partition>`), so continuity within a space is never violated. |
| `callgraph` | planned | dependency neighbourhood of the focus, depth on the x axis (design doc §4.3, §7.4) |
| `matrix` | planned | dependency structure matrix; symbols on both axes in treemap order |

**Parameters.** `{ kind, regroup?: PartitionRef, exclude?: SetRef, pack?: { gap, max_display_lines } }`.

### 2.2 Set

**Definition.** A named collection of symbols (and optionally anchor ranges within them), defined by an expression, resolved to `HashSet<SymbolId>` (+ ranges) on demand. Sets are the nouns; every other layer takes a set.

**Owns.** Nothing visual.

**Live sets.** A set expression may reference session state (`focus`, `camera`, `selection`, `HEAD`). Such sets are re-resolved when their dependency changes. `neighbors(focus)` and `callers(focus)` are live; an explicit id list is static.

**Expression grammar.** (JSON forms in §5.)
- `ids([...])` — explicit
- `glob(pattern)`, `kind(fn|struct|class|file|folder|...)`, `fuzzy(query)`
- `where(metric op value)` — e.g. `where(churn > p90)`
- `reach(from: SetExpr, relation: calls|imports|cochange, direction: out|in|both, depth: n|∞)`
- `changed(rev | rev..rev | worktree)`
- `community(partition, id)`, `layer(partition, name)`
- `children(set)`, `ancestors(set)`, `descendants(set)`, `file_of(set)`
- `neighbors(focus)` — the four arrow targets
- `visible()` — nodes currently drawn (camera-live)
- algebra: `union`, `intersect`, `minus`, `not` (relative to the space's laid-out set)

### 2.3 Fill

**Definition.** Bind a numeric metric to a continuous visual channel through a scale.

**Owns.** `fill` color, `heat`/`stripe`, `opacity`. **Exactly one Fill may drive `fill` at a time**; the stripe channel is separate and may carry a second metric (this is what churn does today). Which fill is active is a hotkey-cyclable view state.

**May not.** Take a color-per-symbol map. It takes a *metric* and a *scale*; color comes from the theme's ramp.

**Metrics.** Built-in providers (churn, LOC/measure, entity count, complexity, fan-in, fan-out, instability, community disagreement, …) and **imported columns**: `symbol → number` supplied by the agent or a tool, with a mandatory `basis` string (`"lcov 2026-08-14"`, `"heaptrack run 3 · peak bytes"`). Imported metrics render exactly like built-in ones and inspect as `metric · value · basis · threshold` with `basis` shown.

**Scale.** `percentile` (default, repo-relative — design doc §6.8), `linear(min,max)`, `log`, `threshold([t0,t1,...])`, `categorical` (for partitions painted as fill).

**Parameters.** `{ metric: MetricRef, channel: fill|stripe|opacity, scale, domain?: SetRef, ramp?: theme ramp id }`.

### 2.4 Mask

**Definition.** Boolean attention control: `dim` everything except a set (spotlight), or `dim` a set.

**Owns.** Dimming / desaturation / body opacity.

**Composition.** Multiple masks AND together (a node is fully lit only if every active mask lights it). Masks never affect fill *values*, only their presentation.

**Parameters.** `{ dimExcept: SetRef } | { dim: SetRef }, strength?: 0..1`.

### 2.5 Edges

**Definition.** Draw relations between symbols as lines/arcs with optional arrowheads.

**Owns.** Line weight, line color (from a small palette keyed by relation), style (solid/dashed), arrows.

**Sources.** Graph relations (`calls`, `imports`, `cochange`, `alloc` (imported), …) or explicit pairs (with an optional weight and `basis`).

**Filters.** `within: SetRef`, `incidentTo: SetRef`, `crossBoundary: folder|partition`, `direction: up|down|any` (against a layer order), `minWeight`.

**Aggregation.** When both endpoints are below the fidelity threshold (Dot/Label), edges roll up to the nearest visible ancestor pair and their weights sum — bundles between folders at zoom-out, individual call edges at zoom-in. Aggregation is a function of the space's current fidelity per node, so it is automatic and consistent with LOD.

**Parameters.** `{ relation | pairs, within?, incidentTo?, crossBoundary?, direction?, minWeight?, style? }`.

### 2.6 Marks

**Definition.** Discrete glyphs: a corner badge on a symbol, or an outline/underline on an anchor range inside a symbol's body at Full fidelity.

**Owns.** Corner glyph slot(s), range highlight.

**Kinds.** Absolute-threshold structural flags (`cycle`, `layering-violation`, `hotspot`) — styled as assessment; `agent-flag` — a neutral, distinct style meaning "an agent pointed here"; `focus-ring`, `neighbor`, `selection` — navigation marks (§4). Every mark carries `label` and optional `basis`.

**Parameters.** `{ on: SetRef | [{symbol, range}], kind, label?, basis? }`.

### 2.7 Notes

**Definition.** Text attached to a symbol, to an anchor range, or pinned *relative to a symbol* (offset in the symbol's own box), rendered from Card fidelity and finer.

**Owns.** Text only. Never a color, heat, or border.

**Provenance.** `source: doc | metric | agent`. `doc` = leading doc comment (ground truth from the file); `metric` = a computed readout (`"142 lines · churn 12 · p87"`); `agent` = narration, rendered in the marked narration style (design doc §5.4 layer 3). One primitive, three styles; the assess/explain guarantee is a field.

**Parameters.** `{ at: SymbolId | {symbol, range}, source, text, pin?: relative offset }`. For `doc` and `metric` the `text` may be omitted and is generated.

### 2.8 Panel

**Definition.** A non-spatial ordered list/table of rows linked to symbols: keyboard-navigable, click/enter frames the row's symbol, and the current row is exposed as the live set `selection`.

**Owns.** A docked or floating rectangle in *screen* space (not world space); its rows.

**Sources.** A `SetRef` (rows = members, sorted by a metric or name), an edge group list (the call-graph columns), a matrix (`matrix` space renders into a Panel), or a fuzzy query (the palette).

**Parameters.** `{ rows: SetRef | EdgeGroups | Matrix, columns?: [MetricRef], sortBy?, dock: left|right|bottom|float, title? }`.

### 2.9 Camera

**Definition.** Viewport control: framing, focus, follow behaviour, and **steps** — an ordered list of `{ camera target, layer diffs }` that is played forward/back. A tour is a step list; navigation history is a recorded step list.

**Owns.** `Camera` (position/zoom), `Focus`, tween.

**Operations.** `frame(SetRef | rect-of-set, padding)`, `focus(SymbolId)`, `home`, `follow: focus|none`, `steps: [Step]`, `step: n`.

### 2.10 The one geometry exception: focus emphasis

The focused leaf is widened on screen (`focused_width`, `deferred_overlay` in `treemap.rs`) so its code is readable at the current zoom. This is a **screen-space render override on the focus only**; it never enters `PackLayout` and no layer may request it for anything else. Naming it here means an agent can't ask for "make this box bigger" — that verb doesn't exist.

---

## 3. Composition model

### 3.1 The view document

```
View := { space: Space, sets: {name → SetExpr}, layers: [Layer], camera: Camera, meta }
Layer := Fill | Mask | Edges | Marks | Notes | Panel
```

Layers form an **ordered stack**. Order matters only where channels overlap: later Fills of the same channel replace earlier ones (only one active), Masks AND, Edges/Marks/Notes/Panels accumulate. A tour step pushes/pops layers rather than restating the whole view.

### 3.2 Channel ownership (conflict rules)

| channel | owner | rule |
|---|---|---|
| geometry, fidelity, exclusion | Space | one space active |
| fill color | Fill | last Fill with `channel: fill` wins; hotkey cycles |
| stripe | Fill | last Fill with `channel: stripe` wins |
| box/body opacity | Mask (and Fill `channel: opacity`) | Masks AND; Fill opacity multiplies |
| lines | Edges | accumulate; per-relation palette |
| corner glyphs | Marks | up to 2 slots; structural before agent; overflow shows `+n` |
| range highlight | Marks | accumulate |
| text | Notes, Panel | accumulate; Notes at Card+ only |
| viewport | Camera | one |

### 3.3 Provenance and the assess/explain guarantee

Every value that reaches a structural channel carries `{ metric, value, basis, threshold }`. Every text carries `source`. The paint resolver refuses a Fill without a metric, a Mark of structural kind without a basis, and a Note without a source. This is what makes "the agent can't paint opinions" a property of the code rather than of the prompt.

### 3.4 Live resolution and invalidation

Sets and layers are resolved lazily and cached against a dependency key:

| dependency | invalidates |
|---|---|
| tree / layout changed (re-index, packing) | everything |
| focus changed | sets referencing `focus`; layers over them; camera `follow` |
| camera moved | `visible()`; edge aggregation level; note visibility |
| selection (panel row) changed | sets referencing `selection` |
| git HEAD / worktree changed | `changed(...)`, cochange edges, churn metric |
| view document edited | the touched layer(s) |
| imported metric reloaded | Fills over it |

---

## 4. Current features decomposed

Every feature the app has today maps onto the nine primitives; several mappings *sharpened* the primitives (live sets, note provenance, Panel promoted from escape hatch to core).

| Current feature (where) | Decomposition | Notes |
|---|---|---|
| Treemap boxes, `PackLayout`, `Rung`/`LeafDraw`, textures, `LayoutTransition` (`world.rs`, `treemap.rs`, `outrider-layout`) | **Space** `treemap` | Fidelity, textures, and transitions are properties of the space, not layers. |
| `filter_extensions` / `filter_folders` / `filter_files` / `max_display_lines` (`settings.rs`, `project_settings.rs`) | **Set** → `space.exclude`, `space.pack.max_display_lines` | Filters become a set expression the space consumes; the same mechanism an agent uses to hide tests. |
| Churn stripe + "Git Churn" toggle (`treemap.rs:1691`, toolbar) | **Fill** `{metric: churn, channel: stripe, scale: percentile}` | Toggle becomes "active fill" state. |
| Focus ring, focused-leaf widening (`focused_width`, `deferred_overlay`) | **Camera.focus** + **Mark** `focus-ring` on `ids([focus])`; widening is §2.10 | |
| Arrow neighbor highlights (`neighbors: [Option<SymbolId>;4]`, `focus::neighbors`) | **Mark** `neighbor` on live **Set** `neighbors(focus)` | Motivated live sets. |
| Hover doc tooltip, focused-file `DocPanel` (`hover_id`, `paint_model::DocPanel`) | **Notes** `{source: doc}` on `ids([hover])` / `ids([focus])` | Motivated note provenance. |
| Card/Detail meta rows (`content::card_meta`, `churn_readout`, `kind_counts`, `inventory`) | **Notes** `{source: metric}` | Per-view swappable readouts. |
| Call-graph mode (`CallGraphMode`, `CgEdgeGroup`, `cg_scrim`, caller/callee columns, up/down cycling) | **Mask** `dimExcept(focus ∪ callers(focus) ∪ callees(focus))` + **Edges** `calls incidentTo focus` + **Panel** (edge groups, dock left/right) + **Mark** `range` on the selected call site | Today's mode is treemap space + three layers. Drawing edges to callers' real positions is an upgrade; the `callgraph` *Space* is a separate later item. |
| Search palette Ctrl+P / Ctrl+T (`palette.rs`) + preview | **Set** `fuzzy(q) ∩ kind(...)` + **Panel** (float, rows sorted) + **Camera.frame(selection)** + **Notes** `{source: metric}` for the preview | |
| Alt+Left/Right history, Enter/Esc, `Focus.last_child` (`navigation.rs`, `focus.rs`) | **Camera** (focus + recorded steps) | A tour is the same structure authored ahead of time. |
| Context menu, settings UI, welcome, project setup, rename/delete | app chrome | Not views. |

**Migration order** (each is a behaviour-preserving refactor with the existing feature as its acceptance test): churn → Fill; neighbor ring → Mark over live Set; hover doc → Notes(doc); call-graph mode → Mask + Edges + Panel; palette → Panel + Set. After the last step the CLI can do everything the keyboard can.

**Brainstormed views as compositions** (for the record):

| View | Composition |
|---|---|
| Boundary-edge view | `edges(calls, crossBoundary: folder)` |
| Diff-flow | `A = changed(HEAD~1) ∪ reach(A, calls, in, 2)`; `mask dimExcept A`; `edges(calls, within A)`; `frame A` |
| Entry-point ribbon | `R = reach(ids([main]), calls, out, ∞)`; `mask dimExcept R`; `edges(calls, within R)` |
| Misplaced-file | `fill(metric: community-disagreement)` |
| Memory hotspots | `fill(metric: import heaptrack.peak_bytes, scale: percentile)`; `edges(pairs: alloc stacks)` |
| Coverage | `fill(metric: import lcov, scale: threshold [0, 50, 100])` |
| Oversight alarm | `H = where(complexity > p90) ∩ where(churn > p90) ∩ where(fanin > p90)`; `marks(H, hotspot)`; `frame H` |
| Layer view | `space treemap regroup: layers`; `edges(imports, direction: up)` |
| DSM | `space matrix` (renders through Panel) |
| Guided tour | `camera.steps[...]` |

---

## 5. Schema

Version-stamped JSON. All symbol references are `SymbolId` in its string form `"<kind>:<qualified_path>#<ordinal>"` (ordinal omitted when 0). Anchor ranges are byte ranges relative to the symbol's file, or `{ "lines": [a, b] }` (1-based inclusive) which the loader converts.

```jsonc
{
  "outriderView": 1,
  "meta": { "title": "What this PR touched", "author": "agent|user", "createdAt": "…" },

  "space": {
    "kind": "treemap",                       // treemap | callgraph | matrix
    "regroup": null,                         // or { "partition": "communities" }
    "exclude": "hidden",                     // SetRef (name) or inline SetExpr
    "pack": { "gap": 8, "maxDisplayLines": null }
  },

  "sets": {
    "hidden":   { "union": [ { "glob": "**/*.lock" }, { "glob": "target/**" } ] },
    "changed":  { "changed": "HEAD~1" },
    "affected": { "union": [ { "ref": "changed" },
                             { "reach": { "from": { "ref": "changed" }, "relation": "calls",
                                          "direction": "in", "depth": 2 } } ] },
    "hot":      { "intersect": [ { "where": { "metric": "complexity", "op": ">", "value": "p90" } },
                                 { "where": { "metric": "churn",      "op": ">", "value": "p90" } } ] }
  },

  "metrics": {                                // imported columns (optional)
    "coverage": { "basis": "lcov 2026-08-14", "unit": "%",
                  "values": { "file:src/auth/login.rs": 12.5, "fn:src/auth/login.rs::verify": 0 } },
    "peakBytes": { "basis": "heaptrack run 3", "unit": "B", "file": ".outrider/metrics/heap.json" }
  },

  "layers": [
    { "fill":  { "metric": "churn", "channel": "stripe", "scale": "percentile" } },
    { "fill":  { "metric": "coverage", "channel": "fill",
                 "scale": { "threshold": [0, 50, 100] } } },
    { "mask":  { "dimExcept": "affected", "strength": 0.8 } },
    { "edges": { "relation": "calls", "within": "affected", "minWeight": 1 } },
    { "edges": { "relation": "calls", "crossBoundary": "folder", "style": "dashed" } },
    { "marks": { "on": "hot", "kind": "hotspot", "label": "complexity×churn",
                 "basis": "p90 both" } },
    { "marks": { "on": [ { "symbol": "fn:src/auth/login.rs::verify", "lines": [40, 52] } ],
                 "kind": "agent-flag", "label": "new cross-module call" } },
    { "notes": [ { "at": "fn:src/auth/login.rs::verify", "source": "agent",
                   "text": "This now calls into billing; previously auth had no edge to billing." } ] },
    { "panel": { "rows": "affected", "columns": ["churn", "coverage"], "sortBy": "churn",
                 "dock": "right", "title": "Affected" } }
  ],

  "camera": {
    "frame": "affected",
    "follow": "none",
    "steps": [
      { "frame": "changed", "push": [ { "notes": [ /* … */ ] } ] },
      { "focus": "fn:src/auth/login.rs::verify", "pop": 1, "push": [ /* … */ ] }
    ]
  }
}
```

**SetExpr forms** (exactly one key per object): `ref`, `ids`, `glob`, `kind`, `fuzzy`, `where`, `reach`, `changed`, `community`, `layer`, `children`, `ancestors`, `descendants`, `fileOf`, `neighbors` (`"focus"`), `visible` (`true`), `union`, `intersect`, `minus`, `not`.

**MetricRef:** a built-in id (`churn`, `measure`, `entities`, `complexity`, `nesting`, `params`, `fanin`, `fanout`, `instability`, `communityDisagreement`, …) or a key from `metrics`.

**Validation rules** (loader rejects): fill without metric; structural mark kind without basis; note without source; any coordinate field; unknown set ref; a set with an unresolvable live dependency in a saved (non-session) view is allowed but reported.

---

## 6. Command surface

### 6.1 Three clients, one API

All of these speak the same internal `ViewCommand` enum (§7.4):

1. **File watch (zero infrastructure).** `.outrider/views/*.json` in the project. The app watches the folder; the newest/selected file is applied. Views listed in a Panel. This is enough for a Claude Code skill to work with no live connection.
2. **CLI `outrider view …`** talking to the running instance over local JSON-RPC (§6.4). Imperative verbs, for interactive agent use.
3. **MCP / ACP tools** — later; the same commands exposed as tools. Not in scope for the walking skeleton but the API is shaped for it.

### 6.2 CLI verbs

Mutating:
```
outrider view apply <file.json>              # replace the whole view (idempotent)
outrider view patch <file.json>              # merge: add/replace named sets, append layers
outrider view clear [--layers|--sets|--all]
outrider set <name> <SetExpr-flags…>         # e.g. --changed HEAD~1 --reach calls:in:2
outrider fill <metric> [--channel fill|stripe] [--scale percentile|log|threshold=a,b,c]
outrider mask --dim-except <set> | --dim <set> [--strength 0.8]
outrider edges <relation> [--within <set>] [--incident-to <set>] [--cross-boundary folder]
outrider mark <set|symbol[:lines]> --kind hotspot|agent-flag|… [--label …] [--basis …]
outrider note <symbol[:lines]> "text"        # source=agent always from the CLI
outrider panel <set> [--columns churn,fanin] [--dock right]
outrider frame <set> | outrider focus <symbol> | outrider home
outrider tour add|play|next|prev|save <file>
outrider metric import <name> <file.json|csv> --basis "…" [--key path|file:line]
outrider layer list|pop|rm <index>
```

Querying (reads ground truth from the running index; also works offline via `outrider-dump`-style direct load):
```
outrider query symbols [--glob] [--kind] [--in <set>] [--json]
outrider query set <name>                    # resolved members
outrider query metrics <symbol>              # every metric with value/basis/percentile
outrider query callers|callees <symbol> [--depth n]
outrider query changed [rev]
outrider query view                          # current view document
outrider query focus|camera|selection
```

### 6.3 Design rules for the CLI

- Every mutating verb is expressible as a `patch` document; the CLI is sugar.
- Verbs are idempotent where possible (`set` replaces a name; `fill` replaces the channel's fill).
- Output is JSON with `--json`, human text otherwise. Errors name the invariant violated ("fill requires a metric; got a color").
- No verb takes coordinates.

### 6.4 Transport

- App listens on `127.0.0.1:<ephemeral>`; writes `{ pid, port, token, project_root }` to `<cache_dir>/outrider/instances/<hash(project_root)>.json`; removes it on exit.
- CLI resolves the instance by project root (`--project` or cwd walk-up), connects, sends JSON-RPC 2.0 with the token in the first frame.
- Line-delimited JSON. Methods: `view.apply`, `view.patch`, `view.get`, `set.define`, `layer.push`, `layer.pop`, `camera.frame`, `camera.focus`, `query.*`, `metric.import`, `tour.*`. Notifications app→client: `focus.changed`, `selection.changed`, `view.changed` (for a future live agent).
- Windows-friendly (TCP loopback), no extra deps beyond `std::net` + `serde_json`. Named pipes / Unix sockets can be added later behind the same trait.

### 6.5 The skill contract (for later; shapes the CLI now)

A Claude Code skill answering "how does X work" should be able to: `query symbols`/`callers` to ground itself → write a view file with sets, mask, edges, notes, and steps → `view apply` (or drop the file in `.outrider/views/`) → optionally `tour play`. Nothing in that loop needs a live agent connection.

---

# Part II — Implementation spec

## 7. Framework

> Detailed spec: [view-primitives/00-framework.md](view-primitives/00-framework.md)

### 7.1 Crate layout

> The authoritative module list is [00-framework.md §2.1 + §6.1](view-primitives/00-framework.md); the sketch below predates the component specs.

New crate **`crates/outrider-view`** (no GPUI dependency; depends on `outrider-index`, `outrider-layout`, `serde`, `serde_json`):

```
outrider-view/src/
  lib.rs          — re-exports
  spec.rs         — ViewSpec + serde types (the schema of §5)
  set.rs          — SetExpr, SetResolver, resolved sets, dependency keys
  metric.rs       — MetricRef, MetricProvider trait, registry, imported columns, Scale
  relation.rs     — RelationProvider trait (calls/imports/cochange), edge aggregation
  layers/{fill,mask,edges,marks,notes,panel}.rs — per-layer resolution into paint-ready data
  resolve.rs      — ViewResolver: (spec, tree, layout, session state) → ResolvedView
  command.rs      — ViewCommand enum + apply/patch semantics
  validate.rs     — schema/invariant validation
```

Reasons: the CLI binary needs the spec/query types without GPUI; resolution logic is pure and unit-testable against fixture trees (`outrider-index/tests/fixtures/mini_repo`); the app crate keeps only rendering and interaction.

In **`crates/outrider`** (app): a new module `view/` with `paint_resolver.rs` (ResolvedView → per-DrawItem paint overrides), `edge_pass.rs`, `mark_pass.rs`, `note_pass.rs`, `panel_view.rs`, `rpc.rs` (server), `view_watch.rs` (file watcher).

New binary **`crates/outrider-cli`** (`outrider view …`, `outrider query …`); shares `outrider-view` for types and offline query.

### 7.2 Core types

```rust
// outrider-view/src/spec.rs
pub struct ViewSpec {
    pub version: u32,
    pub meta: Meta,
    pub space: SpaceSpec,
    pub sets: BTreeMap<String, SetExpr>,
    pub metrics: BTreeMap<String, ImportedMetric>,
    pub layers: Vec<LayerSpec>,
    pub camera: CameraSpec,
}
pub enum LayerSpec { Fill(FillSpec), Mask(MaskSpec), Edges(EdgesSpec), Marks(MarksSpec), Notes(NotesSpec), Panel(PanelSpec) }

// outrider-view/src/resolve.rs
pub struct SessionState<'a> {          // what live sets/camera read
    pub focus: &'a SymbolId,
    pub selection: Option<&'a SymbolId>,
    pub visible: Option<&'a [SymbolId]>,
    pub head: Option<&'a str>,
}
pub struct ResolvedView {
    pub sets: BTreeMap<String, ResolvedSet>,
    pub fill: Option<ResolvedFill>,          // channel: fill
    pub stripe: Option<ResolvedFill>,        // channel: stripe
    pub mask: MaskTable,                     // SymbolId → light factor 0..1 (AND of masks)
    pub edges: Vec<ResolvedEdgeLayer>,
    pub marks: MarkTable,                    // SymbolId → Vec<Mark>; range marks by symbol
    pub notes: NoteTable,                    // SymbolId → Vec<Note>
    pub panels: Vec<ResolvedPanel>,
    pub camera: CameraSpec,
    pub deps: DependencyKey,                 // for invalidation
}
pub struct ResolvedSet { pub ids: HashSet<SymbolId>, pub ranges: BTreeMap<SymbolId, Vec<Range<usize>>>, pub deps: Deps }
pub struct ResolvedFill { pub metric: MetricId, pub values: BTreeMap<SymbolId, MetricValue>, pub scale: ScaleFn, pub basis: String }
pub struct MetricValue { pub raw: f64, pub scaled: f32 /* 0..1 or class index */, pub percentile: f32 }
```

`Deps` is a bitset `{ TREE, FOCUS, CAMERA, SELECTION, GIT, METRIC(name) }`; a resolved artifact is dropped when the session invalidates any bit it carries.

### 7.3 Providers

```rust
pub trait MetricProvider {
    fn id(&self) -> &str;
    fn basis(&self) -> &str;
    fn value(&self, node: &SymbolNode, ctx: &ProviderCtx) -> Option<f64>;
    fn deps(&self) -> Deps;                  // e.g. churn → GIT
}
pub trait RelationProvider {
    fn id(&self) -> &str;                    // "calls" | "imports" | "cochange" | imported
    fn out_edges(&self, from: &SymbolId, ctx: &ProviderCtx) -> Vec<(SymbolId, f64 /*weight*/)>;
    fn in_edges(&self, to: &SymbolId, ctx: &ProviderCtx) -> Vec<(SymbolId, f64)>;
    fn deps(&self) -> Deps;
}
```

Built-in metric providers wrap fields already on `SymbolNode` (`churn`, `churn_count`, `measure`, child counts). AST metrics (complexity, nesting, params) get a provider over the retained buffer via `outrider_index::buffer` + tree-sitter. `calls` wraps `outrider_index::call_graph::resolve_calls` behind a per-symbol cache (the app already has `call_graph_cache`; move it behind the provider). `imports` and `cochange` are new providers (cochange from the churn cache's commit→files data).

Imported metrics are a `MetricProvider` over a `BTreeMap<SymbolId, f64>`; the importer accepts keys `path`, `path::item`, or `file:line` (line → enclosing symbol via `byte_range`).

### 7.4 Commands

```rust
pub enum ViewCommand {
    Apply(ViewSpec), Patch(ViewPatch), Clear(ClearScope),
    DefineSet { name: String, expr: SetExpr },
    PushLayer(LayerSpec), PopLayer, RemoveLayer(usize),
    Frame(SetExpr), Focus(SymbolId), Home,
    ImportMetric { name: String, metric: ImportedMetric },
    Tour(TourCommand),
}
```

`apply(spec, &mut ViewSpec)` and `patch` are pure functions in `command.rs`; validation runs before mutation and returns a list of `Violation { path, rule }`.

### 7.5 Where it plugs into `TreemapView`

Add to `TreemapView`:
```rust
view_spec: ViewSpec,                 // current document (session default is built at startup, §7.6)
view_resolved: Option<ResolvedView>, // cache
view_deps_dirty: Deps,               // bits set by focus/camera/selection/git/tree changes
rpc: Option<RpcServer>,              // §6.4
view_watch: Option<ViewWatcher>,     // §6.1 (1)
```

Render frame (`paint_items`, `treemap.rs:1433`):

1. If `view_deps_dirty` intersects `view_resolved.deps`, re-resolve the affected parts (sets first, then layers that depend on them).
2. `visible_nodes` runs as today (space).
3. **PaintResolver** maps each `DrawItem` → `PaintItem` consulting: `fill`/`stripe` tables (replacing the `stripe: churn` special case at `treemap.rs:1691`), `mask` table → `body_opacity`/`tex_opacity`/desaturated fill, `marks` → new `PaintItem.glyphs: Vec<Glyph>` and `range_marks`, `notes` → `PaintItem.body` rows at Card+ (replacing hard-coded `content::body_lines` for the meta rows once migrated).
4. New passes after quads: **edge pass** (world rects from `PackLayout` → screen via `Camera::world_to_screen`, aggregated per §2.5), **mark pass** (glyphs at box corners; range outlines using the same line geometry the code renderer uses), **note pass** (agent/doc notes as floating text blocks anchored to the symbol box).
5. **Panels** render as GPUI `div`s alongside the palette overlay, using the resolved rows.

The existing `cg_scrim`, `neighbors`, `hover_id` doc, and `show_churn` branches are deleted as each migration step lands (§4).

### 7.6 The session default view

At startup the app builds a `ViewSpec` that reproduces today's behaviour:

```
sets:   hidden = union(glob(filter_folders…), glob(filter_extensions…), ids(filter_files))
        neighbors = neighbors(focus)
        hover = ids([hover])                          (session-internal live set)
space:  treemap { exclude: hidden, pack.maxDisplayLines }
layers: fill  { churn, stripe, percentile }           (present iff show_churn)
        marks { on: ids([focus]), kind: focus-ring }
        marks { on: neighbors, kind: neighbor }
        notes { at: hover, source: doc }
        notes { at: focus,  source: doc }
camera: follow: focus
```

User actions mutate this spec through the same `ViewCommand`s the CLI uses. This is the mechanism that guarantees "the CLI can do everything the keyboard can."

### 7.7 Invalidation wiring

| event (existing code) | sets bits |
|---|---|
| `Focus::set/step_*` | FOCUS |
| camera pan/zoom/tween tick | CAMERA |
| panel selection move | SELECTION |
| loader finished / packing snapshot (`apply_snapshot`, `finish`) | TREE |
| git watcher (new: poll `.git/HEAD` + index mtime, 1 s) | GIT |
| rpc / file-watch command | as computed by `patch` |

CAMERA-dependent artifacts (`visible()`, edge aggregation) are recomputed at most once per frame; everything else on demand.

### 7.8 Testing strategy

- `outrider-view` unit tests against `mini_repo` fixture: set algebra, reach, changed (with a temp git repo as in `churn_test.rs`), scales, mask AND, edge aggregation, validation violations, apply/patch idempotence.
- Golden tests: session default view resolves to the same `PaintItem` fields the current code produces for a fixed camera (captured once before migration).
- CLI tests: spawn app headless? Not yet — test the RPC server with a fake `TreemapView`-free harness holding a `ViewSpec` + resolver.

---

## 8. Component specs

Each: data → resolution → render → invalidation → commands → tests.

### 8.1 Space

> Detailed spec: [01-space.md](view-primitives/01-space.md)

- **Data.** `SpaceSpec { kind, regroup: Option<PartitionRef>, exclude: Option<SetExpr>, pack: PackOverrides }`.
- **Resolution.** `exclude` resolves to a set; the tree passed to `outrider_layout::pack` is a filtered view (a `SymbolTree` clone with excluded subtrees removed, or — better — a `pack` variant taking a predicate; add `PackConfig.exclude: Option<&dyn Fn(&SymbolId)->bool>` to avoid the clone). `regroup` builds a synthetic `SymbolTree` whose folders are partition groups and whose leaves are the original files/items; ids stay the original `SymbolId`s so layers keyed on symbols keep working. Space id = `treemap` or `treemap@<partition>`; changing space id triggers a `LayoutTransition` exactly like a re-pack does today.
- **Render.** Unchanged (`visible_nodes`, `Rung`, `LeafDraw`, textures).
- **Invalidation.** TREE, and the exclusion set's deps.
- **Commands.** `view.apply/patch` with `space`; `set hidden …` then reference. `callgraph`/`matrix` are separate milestones; the enum has their variants from day one so documents validate.
- **Tests.** exclusion equals current filter behaviour; regroup keeps every original id; determinism (`pack` twice → equal).

### 8.2 Set

> Detailed spec: [02-set.md](view-primitives/02-set.md)

- **Data.** `SetExpr` enum mirroring §5; `Deps` per node.
- **Resolution.** `SetResolver { tree, index: TreeIndex, relations, metrics, git, session }`. Recursive eval with memo by structural hash. `reach` is BFS over the relation provider with a visited set; depth ∞ bounded by node count. `where` uses `MetricProvider` + a percentile table computed once per (metric, domain). `changed` shells out to git (`git diff --name-only`, `git diff -U0` for ranges → enclosing symbols) with a cache keyed by GIT deps. `visible()` reads `session.visible`. `neighbors(focus)` calls `focus::neighbors` (move it into `outrider-view` or expose via `SessionState`).
- **Render.** none.
- **Invalidation.** Union of leaves' deps.
- **Commands.** `set <name> …`, `query set <name>`.
- **Tests.** algebra laws; reach depth; changed on a temp repo; live set re-resolves on focus change; unknown ref → violation.

### 8.3 Fill

> Detailed spec: [03-fill.md](view-primitives/03-fill.md)

- **Data.** `FillSpec { metric: MetricRef, channel: Fill|Stripe|Opacity, scale: Scale, domain: Option<SetExpr>, ramp: Option<String> }`.
- **Resolution.** For each laid-out node in `domain` (default all): `raw = provider.value(node)`; percentile table over the domain; `scaled = scale(raw)`. Store `basis` from provider/imported metric.
- **Render.** PaintResolver: `channel: Fill` → replaces `PaintItem.fill` (theme ramp keyed by `scaled`; kind tint preserved as a border so structure stays legible); `Stripe` → `PaintItem.stripe = Some(theme::churn_heat(scaled))` generalised to `theme::heat(ramp, scaled)`; `Opacity` → multiplies `body_opacity`. Inspect: hover/click on a filled node adds a `metric` Note `"<metric> <raw> · p<percentile> · <basis>"`.
- **Invalidation.** provider deps ∪ domain deps ∪ METRIC(name).
- **Commands.** `fill <metric> …`, `metric import`, hotkey cycle (`view_spec` mutation that reorders which Fill is last for the channel).
- **Tests.** churn stripe golden; threshold scale classes; imported metric by `file:line` resolves to enclosing symbol; fill without metric rejected.

### 8.4 Mask

> Detailed spec: [04-mask.md](view-primitives/04-mask.md)

- **Data.** `MaskSpec { DimExcept(SetExpr) | Dim(SetExpr), strength: f32 }`.
- **Resolution.** `MaskTable: HashMap<SymbolId, f32>` light factor; start 1.0; each mask multiplies `1 - strength` onto dimmed nodes. Containers: a container is lit if any descendant is lit (so spotlighting a method keeps its file/folder legible), at `max(strength of children) * 0.5`.
- **Render.** PaintResolver: `body_opacity *= light`, `tex_opacity *= light`, fill desaturated toward the theme's dim color by `(1-light)`. Replaces `cg_scrim` (which was a global dim-except-focus).
- **Invalidation.** set deps.
- **Commands.** `mask --dim-except|--dim`.
- **Tests.** AND semantics; container inheritance; strength 0 = no-op.

### 8.5 Edges

> Detailed spec: [05-edges.md](view-primitives/05-edges.md)

- **Data.** `EdgesSpec { source: Relation(String) | Pairs(Vec<{from,to,weight,basis}>), within, incidentTo, crossBoundary: Option<Folder|Partition>, direction, minWeight, style }`.
- **Resolution.** Enumerate candidate edges: for `Relation` over `within` (or `incidentTo`) via provider; apply filters. Output `Vec<Edge{from,to,weight}>` at symbol level.
- **Aggregation (render-time, CAMERA-dependent).** For each edge, lift each endpoint to its nearest ancestor whose `Draw` is Label or better (using this frame's `DrawItem`s); merge edges with equal lifted endpoints, summing weight. Cull edges with both endpoints off-screen. Cap: if > `MAX_EDGES` (e.g. 2000) after aggregation, keep the heaviest and add a Note-free on-screen count "showing 2000 of 5,412 edges" (no silent caps).
- **Render.** New `edge_pass` after quads and before marks: quadratic curves from the right edge of `from` to the left edge of `to` (matches deeper=right), width `1 + log2(weight)` px clamped, arrowhead if directed, per-relation color, dashed style flag. GPUI: `paint_path` with stroked path; if perf demands, batch into a single path per color.
- **Invalidation.** relation deps ∪ set deps; aggregation on CAMERA.
- **Commands.** `edges <relation> …`.
- **Tests.** cross-boundary filter; aggregation sums; cap reports dropped count.

### 8.6 Marks

> Detailed spec: [06-marks.md](view-primitives/06-marks.md)

- **Data.** `MarksSpec { on: SetExpr | Vec<{symbol, range|lines}>, kind: MarkKind, label, basis }`; `MarkKind` = structural (`cycle`, `layeringViolation`, `hotspot`, `custom(name)` requiring basis) | `agentFlag` | nav (`focusRing`, `neighbor`, `selection`).
- **Resolution.** `MarkTable { by_symbol: HashMap<SymbolId, Vec<Mark>>, ranges: HashMap<SymbolId, Vec<(Range<usize>, Mark)>> }`. Validate structural kinds have basis.
- **Render.** Corner glyphs at Label rung and above (top-right, up to 2 slots, then `+n`); range marks at Full/Text fidelity as an outline over the affected lines (reuse `cg_highlight_lines` machinery — the `BufferManager` line mapping — generalised to N ranges); nav kinds reuse the existing focus ring / neighbor styles. Hover on a glyph shows `label · basis` as a metric Note.
- **Invalidation.** set deps.
- **Commands.** `mark …`.
- **Tests.** glyph slot overflow; range→line mapping; structural mark without basis rejected.

### 8.7 Notes

> Detailed spec: [07-notes.md](view-primitives/07-notes.md)

- **Data.** `NotesSpec(Vec<Note{ at: SymbolId | {symbol, range}, source: Doc|Metric|Agent, text: Option<String>, pin: Option<(f32,f32)> }>)`.
- **Resolution.** `doc` with no text → `node.doc`; `metric` with no text → the readouts from `content.rs` (`card_meta`, `churn_readout`, …) via a `MetricReadout` registry; `agent` requires text.
- **Render.** Card+ only. `doc`/`metric` render as today's body rows and DocPanel. `agent` renders in the narration style: distinct type face/tint and a leading marker (design doc §5.4 layer 3), inside the box below the header at Detail/Full, or as a floating callout beside the box at Card when it doesn't fit; range-anchored agent notes at Full render as callouts to the right of the range (the design doc's "anchor-attached inline summary boxes", §5.2 Full).
- **Invalidation.** target set deps; TREE.
- **Commands.** `note <symbol[:lines]> "text"` (source forced to `agent` from the CLI).
- **Tests.** source styling flags; missing text on agent note rejected; doc note equals current tooltip text.

### 8.8 Panel

> Detailed spec: [08-panel.md](view-primitives/08-panel.md)

- **Data.** `PanelSpec { rows: Set(SetExpr) | EdgeGroups{ of: SetExpr, relation, direction } | Matrix{ space: matrix }, columns: Vec<MetricRef>, sortBy, dock, title }`.
- **Resolution.** `ResolvedPanel { rows: Vec<Row{ id, label, cells: Vec<MetricValue> }>, dock, title }`. EdgeGroups reproduces `group_edges` (by `raw_name`) for the call-graph columns.
- **Render.** GPUI `div` overlay (like the palette). Keyboard: Up/Down move the row (sets SELECTION and the live `selection` set), Enter frames/focuses, Esc closes. When a panel is open, Up/Down are captured by it (this is how call-graph mode already behaves).
- **Invalidation.** rows' set deps; SELECTION for highlight only.
- **Commands.** `panel <set> …`, palette and call-graph mode become panel presets.
- **Tests.** sort by metric; edge grouping equals current `group_edges`.

### 8.9 Camera

> Detailed spec: [09-camera.md](view-primitives/09-camera.md)

- **Data.** `CameraSpec { frame: Option<SetExpr>, focus: Option<SymbolId>, follow: Focus|None, steps: Vec<Step>, step: usize }`, `Step { frame|focus|home, push: Vec<LayerSpec>, pop: usize, note: Option<String> }`.
- **Resolution.** `frame(set)` → union rect of members from `PackLayout` (+ padding) → `camera::frame_rect` → `CameraTween`. `focus` → `Focus::set`. Steps: applying step *n* pops/pushes on the layer stack relative to step *n-1* and then frames.
- **Render.** existing tween/`Camera`.
- **Invalidation.** n/a (it *causes* FOCUS/CAMERA).
- **Commands.** `frame`, `focus`, `home`, `tour add|play|next|prev|save`. Navigation history (`NavigationHistory`) records steps; `tour save` can export it as a starting point for authoring.
- **Tests.** frame of an empty set is a no-op with a warning; steps push/pop are balanced.

### 8.10 RPC server and file watcher

> Detailed spec: [10-rpc-and-watcher.md](view-primitives/10-rpc-and-watcher.md)

- `rpc.rs`: `std::net::TcpListener` on a background thread; each connection is line-delimited JSON-RPC; commands are queued into an `mpsc` channel drained on the GPUI foreground (`cx.spawn` / `background_executor` + `notify`). Instance file per §6.4; token = 32 random bytes hex (from `getrandom` via `std` `RandomState` hashing if we avoid a new dep, or the `rand` crate).
- `view_watch.rs`: poll `.outrider/views/` every 500 ms (avoid a `notify` dependency initially); newest mtime file is applied if it parses; parse errors surface as a `Notification` (existing `Notifications`).

### 8.11 CLI (`crates/outrider-cli`)

> Detailed spec: [11-cli.md](view-primitives/11-cli.md)

- `clap`-based; subcommands per §6.2. Connects via instance file; falls back to offline mode for `query` (loads the index directly through `outrider-index`, the way `outrider-dump` does).
- Set flags compile to `SetExpr`; every mutating verb builds a `ViewPatch` and calls `view.patch`.
- `--json` everywhere; exit code non-zero on violation, printing the violation list.

---

## 9. Milestones

1. **Framework skeleton** — `outrider-view` crate with `ViewSpec`, `SetExpr` (ids/glob/kind/union/intersect/minus/neighbors), `MetricProvider` for churn/measure, `ViewResolver`, validation, apply/patch. Session default view (§7.6) built at startup; churn stripe routed through **Fill** (golden test passes); neighbor ring and focus ring routed through **Marks** over live sets. No CLI yet.
2. **Mask + Frame + RPC + CLI** — `mask`, `frame`, `set`, `query symbols|set`, file watcher. Acceptance: `outrider set s --glob 'src/auth/**' && outrider mask --dim-except s && outrider frame s` works against a running app; the same as a `.outrider/views/*.json` file. *This is the walking skeleton for agent-built views.*
3. **Notes** — hover/focus doc and meta rows through Notes(doc/metric); `note` verb for agent notes with the narration style.
4. **Edges + call-graph migration** — `calls` relation provider over the existing resolver/cache; edge pass with aggregation; call-graph mode rebuilt as Mask + Edges + Panel (+ range Mark for the selected call site). Delete `cg_scrim` and the bespoke column code.
5. **Panel generalisation + palette migration** — palette as Panel over `fuzzy`; `panel` verb.
6. **Sets: reach/changed/where** + AST metric providers (complexity, nesting, params, fan-in/out) → diff-flow, entry-point ribbon, oversight alarm become pure documents. `metric import` for coverage/heap columns.
7. **Camera steps / tours** — `tour` verbs; save history as a tour.
8. **Space extensions** — `regroup`; then `callgraph` and `matrix` spaces.

Each milestone ends with the corresponding rows of the §4 table green.

---

## 10. Open questions

1. **Stale ids in saved views.** `SymbolId` is path-based; renames orphan members. Decision so far: tolerate silently in resolution, report via `query set --missing`. Revisit if agents need stronger identity (content-hash aliases).
2. **Imported metric keys.** `file:line` requires the buffer to be materialised for the line→symbol map; for large imports do it lazily per file. Confirm the map is cheap enough at import time or move it into the index.
3. **Edge rendering cost.** `paint_path` per edge may not scale past a few thousand; batching or an instanced quad approach (edges as thin rotated quads) is the fallback.
4. **Regroup as a distinct space id vs. a parameter** — chosen distinct id for continuity; confirm the transition feels like a space switch, not a re-pack.
5. **Panels stealing Up/Down** — the existing call-graph behaviour; check it doesn't fight the palette when both are open (rule: topmost panel wins).
6. **Transport** — TCP loopback chosen for zero deps and Windows parity; if multi-user machines matter, the token file's permissions need care.
7. **Live agent notifications** (`focus.changed` etc.) are specified but unused until MCP/ACP; keep them behind a flag so the walking skeleton stays small.

---

*End of v0.1. The nine primitives and the channel-ownership table (§3.2) are the contract; everything in Part II is the current best route to implementing that contract on the existing code and may change as the migration steps land.*
