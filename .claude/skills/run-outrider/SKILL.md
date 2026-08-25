---
description: Build, launch, and drive the outrider desktop app (native GPUI code visualizer) — including authoring guided tours that walk a user through a codebase
user_invocable: true
---

# Run Outrider

Build and launch the outrider desktop application. This is a native Rust/GPUI desktop app (not a web app) — it opens its own window.

## Steps

1. **Release build** from the repo root:

```bash
cd "D:/Shared With Desktop/AI/outrider-ide" && cargo build --release 2>&1 | tail -10
```

The binary lands at `target/release/outrider.exe` (Windows) or `target/release/outrider` (macOS/Linux). Build takes ~25s incremental, ~3min clean. If the build fails with "Access is denied" on `outrider.exe`, the app is running — `taskkill //IM outrider.exe //F` first.

2. **Launch** on a project directory:

```bash
start "" "D:/Shared With Desktop/AI/outrider-ide/target/release/outrider.exe" "<PROJECT_PATH>"
```

If no path is given use `$PWD`. Indexing takes ~20s for a large C++ repo; wait before screenshotting. If the project has `.outrider/project.json`, its `filter_folders`/`filter_extensions` are applied (this is why a probe that indexes the whole repo can be 100× slower than the app).

3. **Verify**: screenshot (below) and read the PNG. The app is native — browser tools cannot drive it.

## Teaching a codebase: the guided-tour workflow

This is the primary agent use-case. A **tour** is a view file whose `camera.steps` walks the user through the code: each step moves the camera, can push/pop visual layers (masks, fills, marks), and carries narration that is rendered **in the view**, anchored to the thing it describes. The user presses → / ← to step (Esc exits), or clicks steps in the right-hand navigator panel.

### 1. Recon first (don't guess)

Read the README/docs, grep the source, and use the CLI to confirm exact symbol ids. **Never hand-write a wire id** — paths and nesting are easy to get wrong (namespaces are path segments; ordinals only appear when `>0`):

```bash
CLI="D:/Shared With Desktop/AI/outrider-ide/target/release/outrider-cli.exe"
P="--project <PROJECT_PATH>"
$CLI $P query symbols Baker --kind struct      # exact > prefix > substring > fuzzy
$CLI $P query symbols ScriptHost --kind class
$CLI $P query symbols world_flatten --kind file
```

Output is one wire id per line, e.g. `struct:MatterEngine3/src/part_graph.h::part_graph::Baker   struct Baker`. Use those ids verbatim in `focus` targets, `notes`, and `pairs`. Folders are only returned when you ask with `--kind folder`. Always pass `--project`, otherwise the CLI resolves the instance from the CLI's cwd.

### 2. Author the tour file

Write `<project>/.outrider/views/NN-<slug>.json`. The watcher opens it as a tab and **auto-starts the tour**. Structure:

```json
{
  "outriderView": 1,
  "meta": { "title": "Matter Engine 101", "author": "Claude" },
  "sets": {
    "kernel": { "glob": "MatterEngine3/**" },
    "bake":   { "union": [ {"glob": "MatterEngine3/src/part_graph*"}, {"glob": "MatterEngine3/src/part_asset_v2.*"} ] },
    "hot":    { "where": { "metric": "churn", "op": ">=", "value": "p90" } }
  },
  "layers": [ { "mask": { "dimExcept": "kernel", "strength": 0.75 } } ],
  "camera": {
    "steps": [
      { "frame": "kernel",
        "note": "Headline line\nFirst paragraph of narration.\n\nSecond paragraph." },
      { "frame": "bake",
        "push": [ { "mask": { "dimExcept": "bake", "strength": 0.85 } } ],
        "note": "Spotlight the bake pipeline\n..." },
      { "focus": "struct:MatterEngine3/src/part_graph.h::part_graph::Baker",
        "note": "The Baker seam\nZooms to one symbol and sets keyboard focus on it." },
      { "home": true, "pop": 1,
        "push": [ { "fill": { "metric": "churn", "channel": "fill", "scale": "percentile" } },
                  { "marks": { "on": "hot", "kind": "hotspot", "basis": "churn" } } ],
        "note": "Where the work is happening\n..." }
    ]
  }
}
```

Step semantics:
- **Target** (exactly one): `"frame": "<set>"` frames a set; `"focus": "<wire id>"` zooms to a symbol *and* focuses it (ring, neighbors, doc note); `"home": true` fits the whole view.
- **`"tab": "<title or file stem>"`** plays the step on another tab (e.g. a class diagram or pipeline view) and returns to the origin tab on the next step without `tab`. The tour keeps running across tabs; the navigator shows `↳ Tab` under such steps. Sets named in `frame` must exist in the *target* tab's spec; `home: true` is the usual choice for a diagram tab.
- **Layers are a script that accumulates within a tab run.** `push` adds layers on top of the tab's base `layers`; `pop: n` removes n from the top of what previous same-tab steps built (pop happens before push). Jumping to any step replays the contiguous same-tab run ending there, so state is deterministic. Switching tabs resets the stack to the new tab's base. Put persistent context (the base mask) in `layers`, per-step spotlights in `push`.
- **`note`**: first line = headline (navigator list + top of the in-view card); the rest = body. The full note renders in a ~360px card in the view, anchored to the target — the navigator panel deliberately shows only the step list, so keep notes to 2–4 short paragraphs.
- **Notes support markdown-lite** (same dialect everywhere prose renders: tour notes, doc comments, user comments): blank lines separate paragraphs; `- ` / `* ` / `1. ` lines render as list items with a hanging indent — use them for stages and enumerations instead of prose runs; `` `inline code` `` is recolored with the backticks stripped — use it for symbol, function, and env-var names. No other markdown (headings, bold, links) is interpreted.
- **`"parts": [ {frame|focus|home, note}, … ]`** — sub-steps inside a step for a closer look at its pieces (e.g. a "Foundation libs" step with one part per lib; a "Renderer" step with parts for core / culling / virtual texturing / sky). Parts inherit the step's tab and layer stack; they only move the camera and change the narration. → / ← walk parts before moving on (← from the next step lands on the previous step's *last* part). The navigator shows parts indented under the live step (`3.1`, `3.2`…), click to jump; the callout reads "Step 3 of 15 · 2/5". Use parts when a step's frame is too wide to read and you want to zoom into 3–6 regions in turn without multiplying top-level steps.
- The camera frames into the area **left of the navigator panel**, so targets aren't hidden under it.
- **The referenced item is interactive.** Clicking the in-view note card (or its anchor dot), or pressing **Enter**, selects the step's referenced symbol and zooms the camera onto it (END framing — close enough to read the code). The note never obstructs the target: when no placement beside the target fits, the card collapses to a one-line pill and the full narration moves into the navigator panel instead.

Good tours: 10–16 steps; start wide (whole repo / big picture), narrow to a subsystem, land on one or two concrete symbols (`focus`), hop to a diagram tab (`tab`) to show the same thing as a relationship picture, then widen again to a cross-cutting metric view (churn, complexity). Tell the reader what to ignore early. Build the diagram tabs first (they're separate view files), then reference them from the tour.

Drive and verify a tour over RPC — more reliable than synthetic keys:

```bash
$CLI $P tour goto 6      # 0-based step index
$CLI $P tour next / prev / stop / status
```

### 3. Verify, then iterate

```powershell
& "D:/Shared With Desktop/AI/outrider-ide/scripts/click-outrider.ps1" 500 450 800    # click the map to give it keyboard focus
& "D:/Shared With Desktop/AI/outrider-ide/scripts/send-keys-outrider.ps1" '{RIGHT}' 3000
& "D:/Shared With Desktop/AI/outrider-ide/scripts/screenshot-outrider.ps1" "$env:TEMP\step.png"
```

Step through a few steps and look: did the camera land where you meant? Are the pushed masks spotlighting the right files (others dimmed)? If a `focus` step shows the whole map instead of zooming, the wire id didn't resolve — re-query it. Edit the JSON and save; the tab hot-reloads. Tour control is also available over RPC: `outrider-cli tour play|next|prev|stop|status`.

## View spec reference

View specs are JSON files in `<project>/.outrider/views/`. Each file is a **tab**; the base Treemap tab is always tab 1.

- **sets**: named groups via `glob`, `kind`, `union`, `intersect`, `diff`, `not`, `where` (metric filter, e.g. `{"metric":"churn","op":">=","value":"p90"}`), `changed`, `reach` (relation traversal), `fuzzy`, `ids`.
- **layers**: `fill` (color by metric), `mask` (`dimExcept`/`dim` + `strength`), `edges` (relation lines), `marks` (rings/dots), `notes`, `panel`.
- **camera**: `frame` (set), `focus` (wire id), `follow`, `steps` (tour). A view's `camera.frame`/`focus` is honoured when its tab opens.
- **space**: `{"kind": "treemap"}` (default) or `{"kind": "graph"}` (relationship diagram — see below).

### Relations (for `edges` / `reach`)
- `calls` — function call graph
- `inherits` — class/trait inheritance and impl (child → parent)

### Metrics (for `fill` / `where`)
`churn` (native percentile), `churnCount`, `measure` (lines), `entities`, `complexity`, `maxNesting`, `params`, `fanOut_calls`/`fanIn_calls`, `fanOut_inherits`/`fanIn_inherits`, plus custom imported metrics.

### Mark kinds
`focusRing`, `neighbor`, `selection`, `hotspot`, `cycle`, `layeringViolation`, `agentFlag`, `custom`. Structural marks (`hotspot`, `cycle`, `layeringViolation`) and `custom` require `"basis"` (a metric name). Structural/agent marks draw an accent ring plus a small corner badge — `custom` marks show their `label` (e.g. `"label": "SP-2 script host"`), built-ins show `hot`/`cycle`/`layering`. Use labelled custom marks to tag groups of files in a tour step. Focus/neighbor rings are injected into every view automatically.

The root container never takes a metric fill colour (it's the canvas, not a datum).

### Diagram views beyond the treemap

`space.kind = "graph"` lays symbols out by their relationships. Three recipes:

1. **Class diagram** (inheritance) — `edges.relation: "inherits"`, default `tb`: parents above, arrows point at the base.
2. **Dependency / layer cake** — `edges.pairs` between **folders** (`folder:MatterEngine3`), `tb`. Author edges `from: dependency → to: dependent` so the top-level app lands on top. Get folder ids with `query symbols <name> --kind folder`.
3. **Pipeline / data flow** — `edges.pairs` between files, `"direction": "lr"`: `from` is placed left of `to`, so write edges in data-flow order (`script_host → part_graph → … → vk_scene_renderer`). Sources end up on the left, sinks on the right. Use `"members": {"show": []}` for plain labelled boxes.

Pairs accept any wire id (folder/file/class/fn). Unknown ids are reported as a warning and skipped — verify with `query symbols` first.

### Graph layout (class diagrams)

```json
{ "space": { "kind": "graph", "members": { "show": ["public", "protected"] } },
  "sets": { "types": { "union": [{"kind": "class"}, {"kind": "struct"}] } },
  "layers": [ { "fill": { "metric": "churn", "channel": "fill", "scale": "percentile" } },
              { "edges": { "relation": "inherits", "within": "types", "style": "solid" } } ] }
```

Each connected family lays out as its own cluster (parents above children, arrows child→parent), clusters packed largest-first. `within` = only connected classes (readable); `incidentTo` = every class in the set incl. unconnected ones (can be 1000+ boxes). Boxes list members UML-style (`+`/`#`/`-`, typed signatures, fields then methods); `space.members.show`/`kinds` filter them and `space.members.max` (default 24) caps rows per box with a "+n more" row. Each member row is a real symbol — click it, or Ctrl+` to jump to it in the treemap.

## Seeing and driving the app

```powershell
& "D:/Shared With Desktop/AI/outrider-ide/scripts/screenshot-outrider.ps1" "$env:TEMP\outrider.png"   # then Read the PNG
& "D:/Shared With Desktop/AI/outrider-ide/scripts/send-keys-outrider.ps1" '{HOME}'                    # SendKeys syntax
& "D:/Shared With Desktop/AI/outrider-ide/scripts/click-outrider.ps1" 545 55                          # screenshot-pixel coords
```

Keys: `{HOME}` fit, `{ENTER}` step in, `{ESC}` step out, `{END}` frame focus, arrows = spatial nav (or tour next/prev while a tour is playing), `%{LEFT}` history back, `^+p` command palette. Ctrl-chords via SendKeys are unreliable — click the tab strip instead to switch tabs.

PrintWindow sometimes returns a black frame even though the app renders fine; wait and retry, or ask the user. Synthetic clicks need the window foregrounded; click the map once before sending keys.

## Tabs

Every view file is a tab (top-center strip). Switching tabs **keeps the focused symbol** and reframes it in the new view. Ctrl+Tab / Ctrl+1..9 / Ctrl+` (base treemap). Opening a tab whose spec has `steps` starts its tour; Esc exits the tour but keeps the view.

## The feedback loop: user comments

While exploring (or mid-tour), the user presses **`c`** to leave a comment on the currently selected symbol (root selection = a general comment about the view). Comments accumulate in the right-hand column — each row jumps back to its symbol on click — and persist in `<project>/.outrider/comments.json`. The column's **Copy prompt** button puts a ready-made agent prompt on the clipboard; **Clear all** and per-row ✕ delete them.

As the agent, close the loop from the CLI:

```bash
outrider-cli --project <PROJECT> comments list      # what the user flagged (wire ids + text)
outrider-cli --project <PROJECT> comments prompt    # the same, formatted as a prompt
outrider-cli --project <PROJECT> comments remove <id>
outrider-cli --project <PROJECT> comments clear
```

Protocol: **author a tour/view → user comments (`c`) → read `comments list` (or the user pastes the copied prompt) → respond with a new focused view, tour step, or code diff referencing the commented wire ids → `comments clear`** so stale feedback doesn't linger once the selection it referred to has been replaced. Clear only after you have actually addressed the comments — the list is the user's queue, not yours.

## Performance profiling

`OUTRIDER_PROFILE=1 outrider.exe <project>` appends one line per frame to `%TEMP%\outrider-profile.log` with per-phase microseconds (`resolve`, `visible_nodes`, `items`, `textures`, `edges+callout`) plus `RENDER wants_frame=…` lines that say *why* an animation frame was requested (tween / bake / loading…). On a 20k-symbol C++ repo the idle `paint_items` cost should be ~2–3 ms and the frame loop should stop (no RENDER lines) once textures finish baking; if it keeps running, read the `bake=`/`tween=` flags. Use it before and after any change touching the paint path.

## Notes

- Indexing runs in the background; wait for it before screenshotting/querying.
- `.h` headers are sniffed for C++ and parsed with the C++ grammar (namespaces, classes, members); inheritance resolution handles namespace-qualified bases.
- **Doc extraction**: Rust `///` + `//!`, Python docstrings, and — for C-family languages (C, C++, C#, JS/TS, GLSL/HLSL) — plain `//` comment blocks directly above an item, plus the file's leading `//` banner as the file doc (banner rule lines like `// ----` are stripped). Well-commented code therefore surfaces its prose in the doc panel automatically; structured comments (numbered stages, `code` names) render with the markdown-lite formatting above.
- The doc panel appears above the focused/hovered symbol (base-view tabs carry the doc-notes layer), shows each distinct doc once even when the symbol is both selected and hovered, and truncates long docs with a `…` row rather than running off the top of the window — zoom in for more room.
- Key shortcut **Ctrl+Shift+P** opens the command palette (has Tour: Play/Next/Prev/Stop).
