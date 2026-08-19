---
description: Build and launch the outrider desktop app (native GPUI treemap visualizer)
user_invocable: true
---

# Run Outrider

Build and launch the outrider desktop application. This is a native Rust/GPUI desktop app (not a web app) — it opens its own window.

## Steps

1. **Release build** from the repo root:

```bash
cd "D:/Shared With Desktop/AI/outrider-ide" && cargo build --release 2>&1 | tail -10
```

The binary lands at `target/release/outrider.exe` (Windows) or `target/release/outrider` (macOS/Linux). Build takes ~25s incremental, ~3min clean.

2. **Launch** on the current working directory (the repo Claude is operating in):

```bash
start "" "D:/Shared With Desktop/AI/outrider-ide/target/release/outrider.exe" "$PWD"
```

If the user provides a specific project path as an argument to the skill, use that instead of `$PWD`.

3. **Apply a view** (optional): If the user wants a specific visualization, write a view spec JSON to `<project>/.outrider/views/<name>.json`. The app's file watcher picks it up in ~500ms and opens it as a **new tab** (named from `meta.title`, else the filename), switching to it automatically. The base **Treemap** tab is always tab 1 and is never removed, so the user can always flip back. Saving the file again hot-reloads that tab in place; deleting the file removes the tab. Alternatively, use the CLI to send RPC commands to the running app:

```bash
"D:/Shared With Desktop/AI/outrider-ide/target/release/outrider-cli.exe" view apply --file .outrider/views/<name>.json
```

4. **Verify launch**: The app is a native window — it cannot be driven via browser tools. After launching, report to the user that the app is running. If the build fails, show the compiler errors.

## View spec reference

View specs are JSON files in `<project>/.outrider/views/`. Key features:

- **sets**: Named groups of files/symbols using glob, kind, union, intersect, diff, where, changed, reach, fuzzy
- **layers**: Stack of visual effects — fill (color by metric), mask (dim/highlight), edges (relation lines), marks (rings/dots), notes (annotations), panel (data tables)
- **camera**: focus, follow, home, tour steps
- **space**: layout mode. `{"kind": "treemap"}` (default, with regroup and pack overrides) or `{"kind": "graph"}` — a relationship diagram: the symbols incident to the view's edges layers render as simple labeled boxes positioned by a layered graph layout (parents above children, arrows child→parent). Pan/zoom/click/arrow-key navigation work the same in both modes.

### Available relations for edges layers
- `"calls"` — function call graph (caller/callee)
- `"inherits"` — class/trait inheritance and impl (child extends parent)

### Available metrics for fill layers
- `churn` — git commit frequency (has native percentile)
- `churnCount` — raw commit count
- `measure` — line count (unit: lines)
- `entities` — leaf symbol count
- `complexity` — cyclomatic complexity (AST-derived)
- `maxNesting` — max nesting depth
- `params` — parameter count
- `fanOut_calls` / `fanIn_calls` — call graph out/in degree
- `fanOut_inherits` / `fanIn_inherits` — inheritance out/in degree
- Custom imported metrics via `metrics` field in the spec

### Available mark kinds
- `focusRing`, `neighbor`, `selection`, `hotspot`, `cycle`, `layeringViolation`, `agentFlag`, `custom`
- Structural marks (`hotspot`, `cycle`, `layeringViolation`) require a `"basis"` field (metric name)

### Example: inheritance diagram (graph layout)
```json
{
  "outriderView": 1,
  "meta": { "title": "Inheritance" },
  "space": { "kind": "graph" },
  "sets": {
    "types": { "union": [{"kind": "class"}, {"kind": "struct"}, {"kind": "trait"}, {"kind": "interface"}] }
  },
  "layers": [
    { "fill": { "metric": "churn", "channel": "fill", "scale": "percentile" } },
    { "edges": { "relation": "inherits", "within": "types", "style": "solid" } }
  ]
}
```
With `"kind": "graph"` the edge endpoints are shown as boxes in a UML-style diagram: each
connected family (e.g. one base class and its subclasses) is laid out as its own compact
cluster with parents above children, and clusters are packed largest-first into a
roughly square canvas. Drop the `space` line to overlay the same edges on the regular treemap.

**Which classes appear** is controlled by the edges layer's scoping:
- `"within": "types"` → only classes that actually have an inheritance edge (both ends in
  the set). Best for a readable inheritance diagram.
- `"incidentTo": "types"` → every member of the set, including unconnected classes, which
  are packed after the hierarchies as single boxes. Use when you want to see *all* types
  and spot which ones don't inherit. Can be huge (1000+ boxes on big C++ projects).

Graph boxes list class members (fields first, then methods) as rows inside each box, with
UML visibility markers (`+` public, `#` protected, `-` private). Member names render once
zoomed in enough (~20px row height). Filter with `space.members`:

```json
"space": {
  "kind": "graph",
  "members": { "show": ["public", "protected"], "kinds": ["fn", "field"] }
}
```

- `show`: visibilities to include (omit = all; `[]` = hide members entirely)
- `kinds`: member kind labels (omit = all): `fn`, `field`, etc.
- Omit `members` completely to list every member.
- Visibility comes from C++ access sections (class defaults private, struct public),
  Rust `pub`, and TS/C# modifiers. Members with unknown visibility always show unless
  `show` is `[]`.
- Member rows show the typed declaration signature (e.g. `+ virtual bool connect(WorldManifest& out)`),
  elided past ~46 chars. Each row is a real symbol: click or arrow-key onto it to select it.

Selection feedback (focus ring + neighbor rings) is injected into every applied view
automatically — specs do not need to declare `focusRing` marks themselves.

## Screenshot (see what the user sees)

To capture the outrider window and inspect it visually:

```powershell
& "D:/Shared With Desktop/AI/outrider-ide/scripts/screenshot-outrider.ps1" "$env:TEMP\outrider-screenshot.png"
```

Then read the PNG with the Read tool. This uses Win32 PrintWindow to capture the native GPUI window. Use it to verify views are displaying correctly, read error notifications, and debug visual issues.

**Caveat:** PrintWindow occasionally returns a solid-black capture even though the app renders fine on screen (GPU swapchain timing). On a black capture, wait a few seconds and retry, or ask the user what they see — do not conclude rendering is broken.

## Drive the app with keystrokes

```powershell
& "D:/Shared With Desktop/AI/outrider-ide/scripts/send-keys-outrider.ps1" '{HOME}'
```

Uses .NET SendKeys syntax. Useful sequences: `{HOME}` (fit whole view), `{ENTER}` (step into first child / focus), `{ESC}` (step out), `{END}` (frame focused node), `{UP}{DOWN}{LEFT}{RIGHT}` (spatial navigation between boxes), `%{LEFT}` (Alt+Left, history back), `^+p` (Ctrl+Shift+P, command palette). Chain a few calls then screenshot to walk the user through a specific class.

## View tabs

Every view file is a tab in a strip at the top-center of the window (hidden while only the
Treemap tab exists). Switching tabs **keeps the focused symbol**: select a class in an
inheritance graph, jump to the Treemap tab, and the camera lands on that same class in the
treemap (and vice versa). When the focused symbol isn't present in the target view, the
camera falls back to the view's home framing.

- Click a tab, or: **Ctrl+Tab / Ctrl+Shift+Tab** cycle, **Ctrl+1..9** jump to tab N,
  **Ctrl+`** jump to the base Treemap tab. (`Cmd` on macOS.)
- Agent workflow: write `<project>/.outrider/views/<topic>.json` → it appears as a tab and
  becomes active → walk the user through it → they press Ctrl+` to return to the treemap
  at the same symbol. Write several files to build up a set of explanatory views the user
  can flip between.

## Notes

- The app indexes the target repo on a background thread; the treemap appears after indexing completes (a few seconds for most repos).
- The CLI companion binary `outrider-cli.exe` is also built alongside and can be used for RPC commands while the app is running.
- Key shortcut: **Ctrl+Shift+P** opens the in-app command palette.
- View specs in `<project>/.outrider/views/*.json` are auto-detected by a file watcher (500ms poll).
- Edit a view JSON and save — the app hot-reloads within 500ms.
