# Code Comprehension Viewer — Design Document

**Status:** Initial comprehensive design (v0.1)
**Working title:** TBD
**Substrate:** Standalone GPUI application (Rust)
**Scope of this document:** Everything settled during design; ends with the one thing that can only be settled by building.

---

## 0. How to read this document

Every section states a decision and the reasoning behind it. The reasoning is load-bearing: many decisions look like arbitrary preferences until you see the constraint they satisfy, and several constraints are *silent* — violating them doesn't produce an error, it produces a tool that feels subtly wrong. Section 9 consolidates those invariants; treat it as the checklist that no implementation choice may violate.

---

## 1. Thesis and mission

### 1.1 The problem

Codebases are increasingly written and rewritten by AI agents, fast. The practical way people understand such a codebase today is to **ask the agent** — which means your only fast window onto the code is the account given by the same model that wrote it. The author is narrating its own homework. That is precisely where confident-wrong hides, and it is a *second-hand* view: you are trusting a paraphrase, not inspecting the thing.

### 1.2 The mission

Fast, **first-hand** understanding of unfamiliar and rapidly-changing codebases — architecture, changes, and code smell — so a maintainer can provide real oversight of agent-driven work: approve, correct, and improve changes they did not write, with clear sight of what actually happened rather than what the agent says happened.

### 1.3 The founding principle: assess vs. explain

The tool is an **instrument rendered from ground truth** — the AST, the dependency graph, the diff, and git history — not from model prose. This splits cleanly and the split is enforced *mechanically* throughout the design:

- **Structural signals assess.** Whether code is large, tangled, central, churning, or architecturally misplaced is computed from the repo and is inspectable down to the number. Structural signals own color and geometry.
- **The LLM explains.** "What does this do" narration is genuinely useful but is a convenience layer, always visually marked as the model's account, and **never** allowed to drive an assessment, a color, a heat value, or a border.

The moment the tool's judgment about whether code is okay comes from an LLM, it has rebuilt the second-hand view inside itself. The paint-channel separation (Section 5.4) is what makes this principle something the user can *see*, not merely something we promised.

### 1.4 Product posture

**Comprehension-first, editing-in-mind.** The tool is a read-mostly comprehension instrument. Editing is not ruled out; the substrate is built editing-ready from the first commit (Section 3.2) so that adding it later is wiring, not a rewrite. The roadmap is not binary read-only → full editor; it is:

> **read → apply diffs / surgical edits → full authoring**

The middle stage (rename-across, accept an agent's proposed change, edit a single line inline) needs none of the hard editor machinery and is exactly what the agent layer produces. A comprehension-plus-agent tool can live happily in that middle for a long time.

---

## 2. Substrate and platform

### 2.1 Standalone GPUI application

**Decision:** Build a standalone native application on **GPUI** (Zed's open-source GPU-accelerated Rust UI framework). Not a VS Code extension. Not a Zed extension for the core view.

**Why not a VS Code extension:** The floating-box / semantic-zoom view needs control of the render surface. VS Code confines custom rendering to a sandboxed webview and won't expose its pipeline; deep integration and a custom high-performance renderer pull apart.

**Why not a Zed extension (today):** Zed extensions are WASM sandboxes providing language servers, themes, slash commands, and context servers. They **cannot render custom UI**. A visual-extension API (custom panels/views via GPUI) exists only as a draft RFC with open discussions and no ship date. Betting the core interaction on that surface is betting on an unshipped API.

**Why standalone is better anyway:** It maps precisely onto the comprehension-companion posture — an alt-tab window that owns its rendering, syncs with the repo and git, and pops to the real editor for edits. No host pipeline to fight, no RFC to wait on. Precedent is broad: the GPUI ecosystem already includes native database clients, agentic-workflow tools, terminals, and launchers.

**Licensing:** The GPUI framework is Apache/MIT; the Zed editor is GPL. A standalone GPUI app is clean. If we vendor Zed's text crates (`rope`, `text`, `sum_tree`), confirm their license first; if it doesn't suit, the *pattern* ports to any rope library (`ropey`, `crop`), which is the part that matters.

### 2.2 Why GPUI over an immediate-mode (ImGui) approach

The instinct to reach for immediate-mode 60fps rendering was directionally right but for the wrong mechanism. Constant redraw is a battery/thermal tax that buys nothing on a static screen, and rendering boxes and lines was never the bottleneck — a GPU eats thousands of quads without noticing. The real costs are graph computation, layout stability, summary latency, and marshaling.

GPUI is already the better-shaped version of the goal: a **reactive entity model** with Element trees, Taffy layout, a scene graph diffed between frames, and GPU submission — **retained state, redraw on demand**, hitting high framerates during actual motion (pan/zoom/dive) and idling at zero otherwise. That is exactly "60fps only during transitions," achieved by construction.

### 2.3 GPUI is the framework, not Zed's editor

**Consequence to plan around:** GPUI provides `div`, text, `uniform_list`, layout, and input dispatch. It does **not** provide Zed's rich code-editing component (multi-cursor, selection, syntax highlighting, LSP completion) — that lives in Zed's *app* crates. So we do not get a free editor.

This is exactly why comprehension-first earns its keep: boxes are read-mostly rendered code with **tree-sitter** syntax highlighting (trivial to pull in standalone), and "edit for real" pops to the actual editor. If in-box editing is ever needed, the fallback is reusing Zed's editor crate — heavy, and a reason to keep that need at bay.

### 2.4 Editing-ready substrate (four one-way doors)

"Add editing later" fails in two opposite ways: build read-only in a way that bakes in assumptions requiring a rewrite, or over-build for editing now and drag that weight through the MVP. The escape is to separate **substrate** (how text and positions are represented) from **features** (user-facing editing capabilities). Keep the substrate editing-ready — cheap now, a rewrite later. Defer every feature — all additive if the substrate is right.

Four substrate decisions are one-way doors and all four are cheap to do correctly now:

1. **Rope buffer from day one.** Not immutable strings with byte offsets. Editing retrofitted onto a string substrate is a ground-up replacement; with a rope, "add editing" becomes "wire input to mutate the buffer you already have."
2. **Anchors, not offsets, for every stored position.** Every spatial reference — a method's box, a call site, a summary attachment — points to a *stable anchor* into the buffer that auto-adjusts as text changes. With anchors, an edit shifts text and the box layout survives untouched. With offsets, every edit is a cache-invalidation nightmare across the whole spatial structure. Nearly free now (resolve anchor→offset at render time); brutal to retrofit.
3. **Single buffer as the single source of truth.** Render *from* the buffer even in read-only mode, rather than from a separate parsed snapshot. One path, exercised in one direction for now, so that when edits arrive they mutate the one buffer and everything downstream already reacts. Avoid a separate read-path and write-path.
4. **Feed tree-sitter incremental edits, not full re-parses.** The "code changed on disk, re-highlight" path in comprehension mode becomes the *same code path* as "user edited, re-highlight" later. Faster live-reload now, editing-readiness for free.

**Deferred features (two-way doors):** multi-cursor, selection models, IME, undo/redo, completion, dirty-state and save management. These are what actually make editing hard; all are additive once the substrate is in place. Building any of them now is the over-engineering failure mode.

**One-line discipline:** *substrate editing-ready, features comprehension-only.*

### 2.5 Summary and signal invalidation

Summaries and derived signals are keyed to **anchors and tree-sitter nodes**, and invalidated on **buffer-change events**, not file-watch events. The same invalidation machinery then serves "file changed on disk," "agent committed," and "user typed" — one more payoff from doing the substrate the editing-ready way. (Live/batch tiering in Section 6.7.)

---

## 3. Agent integration

**Rail:** The **Agent Client Protocol (ACP)** — the open standard (Zed + JetBrains, Jan 2026) for connecting AI agents to editors. The standalone app speaks ACP and drives its own view from the agent's tool calls, rather than inventing a bespoke protocol.

**Sequencing:** The structural view and the ground-truth signal layer come first; LLM summarization comes next as infrastructure for semantic zoom; the conversational ACP layer ("show me how auth works" → it navigates and highlights) rides on top of both.

**Convergence with editing:** Agents propose **diffs**. Applying a diff to the buffer and reflecting it in the view is 90% of the "editing" a comprehension tool needs, and it falls out of the rope+anchor substrate nearly for free — which is why the apply-diffs stage (Section 1.4) is both the agent-output path and the first editing capability.

---

## 4. Spatial model

### 4.1 Determinism replaces memory

Durable, cross-session spatial memory ("auth is over in the top-left") is **dead** for this use case: you cannot build muscle memory of a map that redraws itself as agents rewrite the code. Killing it is correct, and it *frees* automatic layout (its original sin was jumpiness destroying durable memory, which we no longer bank on). The user's stated preference for automatic (not manual) layout is therefore honored.

Two things survive and must be preserved:

- **Intra-session coherence** — not getting lost while navigating *now*. The codebase isn't changing during the 30 minutes you spend on a PR; within a session you build a working map of "I'm here, I dove into that, it's off to the right." Matters *more* when comprehending something unfamiliar fast.
- **Diff legibility** — when reviewing what an agent changed, unchanged structure must stay put so the change pops.

The requirement is therefore rescoped from *persistent* to **deterministic and continuous**:

> Same structure always produces the same layout. A small structural change produces a small visual change.

Determinism replaces memory. Bonus: two people reviewing the same commit see the same picture — a shared reference to point at. Nobody memorizes anything.

### 4.2 Orientation model

Not spatial recall. Three replacements, all better suited to churn than memory was:

- **Query / symbol as entry point.** You don't remember where auth is; you jump to it and the view composes around that focus, freshly, from current structure.
- **Structural landmarks, not coordinates.** You orient by "this is in the API layer," which stays stable as internals churn.
- **The diff as a first-class navigational object.** "What changed since I last looked" is how you *enter*, not a place you navigate back to.

The view is always generated on demand around a focus from the current AST — never a persistent map.

### 4.3 Two decoupled spaces

There are two spaces, deliberately **decoupled in geometry**, switchable (tabs), coupled only through **focus**:

- **Treemap space** — architecture-as-declared. Where symbols live in the folder hierarchy.
- **Call-graph space** — architecture-as-built. What the focused symbol depends on / who depends on it.

Switching keeps the symbol and changes the question (Section 7.5 covers the transition).

### 4.4 Folder tree as the spine; declared vs. built as the headline

The folder structure is a spatial system that mostly **survives the churn argument**: agents rewrite the contents of `src/auth/` nightly, but `src/auth/` stays `src/auth/` across hundreds of commits. It is already in the user's head for free, and it is ground truth (in the repo, deterministic, not a paraphrase).

- Folder tree = **architecture-as-declared** (a human's stated decomposition).
- Import/call graph = **architecture-as-built** (how the code actually depends).
- **The gap between them is the highest-value view in the tool.** Every structural smell lives there: the `utils/` folder that says "utility" while the graph says "god module"; the feature folder that reaches into three others; the layer that imports upward against its own direction.

Mechanically, folder membership is a **stable coordinate system**: cluster and position graph nodes by their folder and the force-directed hairball gets a deterministic spine for free. Intra-folder edges = cohesion; cross-folder edges = coupling, drawn prominently. Determinism and continuity come almost free because folder membership barely moves.

**Caveat:** the tree is *declared*, sometimes arbitrary (package-by-layer conventions scatter one domain concept across five folders). So the folder tree is the **default** spine, not a blind trust: the tool can also re-cluster by actual coupling (community detection) and surface where the two clusterings disagree — the disagreement is itself signal (Section 6.4).

**Precedent:** CodeCity (hierarchy-as-space worked for orientation; buildings-encode-metrics got gimmicky — the lighter treemap is the right weight).

### 4.5 The treemap

- Root far left; expand **right** = deeper. One left→right axis for *every* "deeper" (folders → files → classes/methods → call graph). This single consistent gesture is what keeps the mental model intact across scale, and it aligns "deeper in the tree" with "deeper in the call graph" on one axis.
- **Area ∝ code size.** Metric is swappable (file bytes / lines / AST-node count / churn) — each renders a different truth on the same stable shape. Purpose: *see where the majority of your software lives.*
- Exclude generated/cache folders by reading `.gitignore` and language-native ignores — **not** hardcoded — so existing ignore rules define what's "real" code.
- The treemap is **not an intro screen you graduate from**; it is the persistent substrate the whole ground-truth layer paints onto (recolor by fan-in → hub; by churn → hot spots; by divergence → misplaced modules).

### 4.6 Infinite canvas + camera (Finder × Google Maps)

The canvas is a single infinitely scrollable/zoomable 2D world (Google-Maps style, no boundary constraints). This is in tension with Finder-column navigation, and resolves in our favor:

- **Google Maps is the general case:** content lives at **world-absolute** positions; the camera moves over it. A box never relocates because you scrolled.
- **Finder columns become a camera policy** layered on top: layout stays world-absolute (deeper = further right, positions fixed), and "columns" describe how the camera *frames* — follow the focus, keep current-plus-parent in view, pan right as you descend. "Parents scroll off the left" becomes "the camera leaves parents behind, but they persist in world space" — strictly better than Finder, because zooming out reveals the entire descended spine as one ribbon.

**Sibling-collapse rule (Miller-column discipline, softened):** the active path stays expanded and detailed; siblings you stepped past **collapse back to their treemap box** — still visible as a stable landmark, reclaimed to a single tile. On the infinite canvas the reason shifts from "screen width is scarce" (it isn't) to "managing attention and camera travel." The collapsed-but-visible siblings *are* the breadcrumb trail — the intra-session working map.

### 4.7 Flame-graph layout

Depth = horizontal position, box extent ∝ size, dive by expanding right — this is a **flame/icicle graph**, one of the most battle-tested ways to render a size-weighted hierarchy readably. Strong evidence it reads well at a glance.

**Critical refinement:** a normal flame graph *reflows* on zoom (clicked frame stretches to full width). **We do not.** Hold world positions fixed and move the camera instead. We give up the reflow animation to keep "a frame is always in the same place" — the entire diff-legibility argument. *Flame-graph layout, camera navigation instead of reflow navigation.*

### 4.8 Getting lost, and the antidote

An unbounded canvas without re-grounding is a desert — the defining failure mode of infinite-canvas tools. The antidote is already built in: **determinism makes re-grounding reliable.** Because every symbol has a fixed world position, "jump to root," "frame this symbol," and "recenter on the diff" always work and always land the same place. Budget the concrete affordances: a **minimap**, **jump-to-symbol search** that recenters the camera, and a **home** that frames root. The canvas also *forces* a real semantic-zoom LOD system (Section 5) — zoomed-out cannot be tiny illegible rectangles; it must drop detail and merge, the way Maps turns cities into dots.

### 4.9 The hierarchical grid

Boxes snap to a **hierarchical grid** that exists in world space (rendered or not). Deeper zoom levels use a finer grid (≈10× per level). This converts determinism and continuity from a *discipline* into *structure*: quantized positions can't jitter, and every element's location is derivable from its path (root cell → subcell → subcell), so **position is an address** — reproducible across sessions, machines, and two reviewers on the same commit.

**Grid model — slippy-map lattice, not fixed-count subdivision.** Fixed-count subdivision (each parent a fixed 10×10, one child per cell) would destroy area∝size and overflow past 100 children. Instead: a single global multi-resolution lattice where level *k* has cells of size `base / ratio^k` *everywhere*, and a box occupies a variable **range** of cells sized to its content. Decouple **grid resolution** (global, per level) from **box extent** (per node, content-driven). Leave empty cells — breathing room, and the source of stability (a box never has to move to fill a gap).

**The grid *is* the LOD ladder.** It's a map-tile pyramid: zoom level → grid level → detail level. When level-*k* cells are roughly screen-sized, render level-*k* boxes as tiles and stop descending; zoom in and the next finer level materializes. Culling and level-of-detail both fall out. The lattice is also the **spatial index** (a quadtree — "what's on screen" is a range query).

**Stable ordering (do not order by size).** The classic squarified treemap sorts children by size to optimize aspect ratio — and that sort is the enemy: grow one file and its rank shifts, reshuffling every sibling (a small code change → a large visual change). Order children by a **stable key** (name/path) so a box's slot is a function of identity, not size; a growing file spans more cells *in place* while neighbors hold still. This is the known ordered/stable-treemap fix; take that variant, not the pretty one.

**Layout passes — measure up, arrange down.** Sizing propagates **post-order** from leaves (the *measure pass*): you can't size a folder until its contents are sized. Then, once root knows its extent, positions flow back **down** (the *arrange pass*), each parent placing children within its allocated region. This is Taffy's two-pass model, though the treemap packing itself is custom (neither flexbox nor grid). Because grid rounding only ever rounds *up*, sizes are monotonic and the measure pass terminates in a single sweep — no iterate-to-convergence.

**Floating origin (numerical survival at depth).** With ≈10× finer per level, depth *d* sits at 10⁻ᵈ of root scale; f32 shaders (~7 digits) start to shimmer by depth 6–7 — the wall large-world game engines hit. The fix is theirs and is the *same idea as the addressing scheme*: store positions **hierarchically** (each level relative to its parent) and compose only the few levels near the camera at render time, feeding f32 nothing but the local delta. The hierarchical address is what keeps us numerically alive at depth.

---

## 5. Level-of-detail (LOD) system

### 5.1 Two orthogonal axes

"Level" hides two independent axes; separating them collapses a big matrix into one short ladder:

- **Entity granularity** — which grid level you're on: folder / file / class / method / line. Discrete, set by camera depth crossing grid levels.
- **Render fidelity** — how much of a thing you draw at its current on-screen size: dot / label / card / detail / full. Continuous, set by zoom *within* a level.

A folder can be a dot or a full district; a method can be a dot or its full highlighted body. So we define **one universal fidelity ladder** and a small mapping of what fills each rung per entity type — reused at every grid level and in both spaces.

### 5.2 The universal fidelity ladder

By on-screen pixel size of the cell:

| Rung | Size | Content |
|---|---|---|
| **Dot** | ~4–20px | Fill color only, no text — color *is* the content (driven by active structural overlay). Below ~4px: cull or merge into parent tile. |
| **Label** | ~20–80px | Truncated name + fill color + at most one glyph for a critical flag (cycle member, divergence hit). |
| **Card** | ~80–250px | Name + one-line signature (method) or member/size readout (container) + primary metric as a number + churn sparkline. **First rung an LLM one-liner may appear — marked as narration.** |
| **Detail** | ~250–700px | Full signature + member skeleton (a file shows its methods as sub-labels; a method shows its control structure) + inline metric annotations + one LLM summary sentence, marked. |
| **Full** | >700px | Real tree-sitter-highlighted code off the rope + **anchor-attached inline call-site summary boxes** floating to the right of each call (the original core idea's home) + line-level affordances + (later) editing. |

### 5.3 Fractal chaining and leaf convergence

The ladder is **fractal**: as you zoom into a file box, *it* climbs its own ladder (tile → card → detail); as it reaches full size, its methods (the next grid level) appear at ~200px and begin climbing *theirs*. The handoff between grid levels is just one entity finishing its ladder as its children start theirs — continuous zoom, discrete entities, no seams.

Both spaces run the **identical** ladder; they differ only in which entity sits at each grid level (treemap: folder→file→class→method→body; call-graph: module-node→file-node→method-node→body) and in what "full" arranges *around* the focus (folder siblings vs. callers-and-callees). The ladders **converge at the leaf**: "full fidelity on a method body" is the *same render* in both spaces. That shared leaf is the referent that makes the tab-switch cohere (Section 7.5).

### 5.4 Three paint layers, three permission rules

This is where the assess/explain principle is enforced mechanically:

1. **Geometry layer** (boxes, areas, edges) — ground truth, always on, every rung.
2. **Structural-overlay layer** (fan-in, churn, complexity, divergence, cycles, layering violations) — also ground truth (all from the AST/graph/git). Drives **color, heat, glow, edge-weight** at every rung; the *relevant* signal changes with grid level (macro smells at coarse levels: god-module by fan-in, dependency cycle as a red loop, divergent folder glowing; micro smells at fine levels: over-long method, deep nesting, high fan-out on a call site). **Owns color and geometry; never speaks in prose.**
3. **LLM-narration layer** — **text only**, appears **only at card fidelity and finer**, **always visually distinct** (different type, a marker, a reserved tint), and **never** drives a color, heat value, or border. *LLM for explain, structural for assess.*

When a box is red, it's red because a metric crossed an inspectable threshold; when there's a sentence, it's clearly the model talking. That visible separation *is* the oversight guarantee.

**Modal, swappable fill.** Fan-in, churn, and divergence can't all drive the fill channel at once — they'd fight. One primary signal drives fill at a time, hotkey-cycled (like the treemap size-metric swap). A couple of critical signals get reserved always-on non-fill channels (cycles as edge color, divergence as a corner glyph).

---

## 6. Ground-truth signal layer

### 6.1 The inspectability rule

**Every signal is a number computed from the repo, never a model's opinion.** When a box is hot, it decomposes: click it and see the metric, its value, its basis, and its threshold. This inspectability is what separates this layer from the second-hand view, and it is a hard constraint on every signal below — nothing paints that can't explain itself in a number.

**Precedent:** CodeScene / Adam Tornhill's behavioral-code-analysis work — validates that churn-driven structural signals carry real weight.

### 6.2 Three data sources

- **AST / structure** — sizes, shapes, complexity. Local, from the parse tree.
- **Graph / coupling** — who calls and imports whom. Relational, from resolved references.
- **Git / history** — churn, recency, co-change. Deterministic ground truth, and for this mission arguably co-headline: in an agent-churn world, "where is the change landing" is the whole question.

### 6.3 Micro signals (method / class / file — mostly AST, live-cheap)

- **Method:** LOC; max nesting depth; cyclomatic complexity (decision points + 1); **cognitive complexity** (nesting-weighted — the better "hard to read" metric); parameter count; fan-out (distinct callees).
- **Class:** method count; **LCOM4** (lack of cohesion — connected components of the method-shares-a-field graph; >1 component = a god-class that should split); inheritance depth.
- **File:** total size; entity count; afferent/efferent coupling (Ca/Ce).

### 6.4 Macro signals (folder / module — graph + history, mostly batch)

- Aggregate size (the treemap area); folder-boundary Ca/Ce; **instability** `I = Ce / (Ca + Ce)` (0 = stable, 1 = unstable).
- **Circular dependencies:** Tarjan's SCC on the module graph; any strongly-connected component larger than one node *is* a cycle — the serious architectural smell — drawn as a red loop between districts.
- **Hub / god-module:** afferent coupling above the repo's high percentile — the `utils/` everything imports.

### 6.5 The headline — declared vs. built divergence

The folder tree (declared) measured against the coupling graph (built):

- **Cross-boundary ratio** per folder: fraction of edges that leave vs. stay inside. High external ratio = the declared boundary isn't a real unit.
- **Misplaced file:** a file whose modal coupling target is a *sibling* folder — living in the wrong neighborhood, and the metric says where it should move.
- **Partition disagreement:** run **Leiden** community detection on the coupling graph, then score agreement (normalized mutual information) between that partition and the folder partition. Low score = folders orthogonal to real architecture (the package-by-layer case); per-node disagreement highlights *which* files the two views place differently. Expensive, whole-repo, batch.
- **Layering violation:** given a declared or inferred layer order, any dependency edge pointing "up" against it.

### 6.6 Agent-oversight deltas (the mission-critical signals)

These are *changes*, not static readings, and they are the signals the agent will never volunteer because it doesn't know it did anything wrong:

- **New cross-boundary edge introduced by a diff** — architectural drift as a reviewable event. The agent didn't just change code; it coupled two previously-independent modules. Catch it at introduction, not months later.
- **Temporal coupling gap** (from git co-change): "the agent edited A, but A and B have co-changed in 40 of their last 42 commits, and it didn't touch B" — a **missing-change detector** the static graph structurally cannot produce.

### 6.7 The composite alarm (the reason the tool exists)

Signals compose into the thing that motivated the project:

> **complexity × churn = hotspot** (Tornhill's crime-scene insight: pain concentrates where hard-to-change meets changes-often). Add a third axis — is that hot, churning code also high-coupling / high-divergence? — and you get the **oversight alarm**: *an agent just made a heavy change to a load-bearing, already-fragile, architecturally-central piece.*

No single metric says that; the product of three does, and every factor is inspectable. This composite is the first-hand instrument — what you want lit up before approving a PR you didn't write.

### 6.8 Thresholds

- **Continuous fill/heat channel:** **percentile-relative within the repo** — the top few percent by the active metric glow; self-calibrating; always shows *this* codebase's tail regardless of absolute values.
- **Discrete flags/glyphs:** **absolute** — a cycle is a cycle, a layering violation is binary, cyclomatic past a hard line is objectively a lot.
- Every threshold is inspectable and configurable: a hot box reads `cognitive complexity 34 · 98th percentile · flag at 25`. You never trust the color; the color always decomposes.

### 6.9 Live / batch cadence (three tiers)

Maps directly onto the anchor-keyed, buffer-change invalidation from Section 2.5:

- **Keystroke:** purely local method metrics (LOC, nesting, complexity, params) — recompute only the edited method. Instant.
- **Edge-commit** (the edited method's call set changed): update that node's graph edges; recompute fan-in/out and coupling for it and immediate neighbors. Local-ish, fast.
- **Idle / save / commit** (debounced, on GPUI's `BackgroundExecutor`): the genuinely global computations — SCC/cycles, Leiden community detection, divergence NMI. Cached, invalidated by buffer-change events, never on the interaction thread.

The graph is incrementally maintainable (an edit touches a bounded edge set); a few derived globals (communities especially) aren't cheaply incremental and are recomputed whole in the background. The same invalidation machinery serves disk-change, agent-commit, and user-typed.

---

## 7. Navigation and controls

### 7.1 Arrows navigate structure, not pixels

On a Google-Maps canvas the naïve reading of arrow keys is spatial (move to the nearest box in that direction). That's wrong here: the user's intent is always structural — *out to the parent, in to what this calls, over to the next sibling* — never "the box over there." So **arrows navigate the graph/structure, not pixels.** Because layout is deterministic, a structural move renders as a consistent spatial motion, so structural navigation *feels* spatial without being spatial.

Modality split: **keyboard = precise structural; mouse = loose spatial (free pan/zoom); search = teleport (recenter camera).** Each does what it's best at.

### 7.2 The core distinction

> **Arrows move the *focus*, and the camera follows. Home/End and the mouse move the *camera*, and the focus holds.**

Every key sorts into one of those two.

### 7.3 The arrow grammar (identical in both spaces)

- **Left** — step out. **This is a history operation, not a graph operation** (see 7.4).
- **Right** — step in (into the focused child / dive into the selected callee).
- **Up / Down** — cycle the current peer set. Which set is **directional** (see 7.4): after Right, cycle callees; after Left, cycle alternate callers.

This is **Miller columns generalized to the call graph** — out / in / choose-among-peers — achievable because depth is mapped to the horizontal axis on purpose.

### 7.4 History-based Left and the exploration tree

**Left backs up your traversal path, not "the" graph parent.** The primary navigational axis is the **path you walked**, not the call graph itself: the graph is the *space*, the path is the *thread* through it. The caller you came from isn't necessarily special in the graph (it may be one of thirty), but it's special to *you*. Alternate callers are listed collapsed **above and below** the history-caller as branch points.

**Why this is the right model:** comprehension *is* a path — you follow a thread of reasoning, and walking it backward matters more than any graph-theoretic "parent." And a path has **no diamonds**: the graph has fan-in, diamonds, and cycles that made "which caller is up?" unanswerable, but your *walk* is linear by construction — you arrived by exactly one route, so "back" is unambiguous even at thirty callers. This *dissolves* the fan-out problem rather than patching it.

**Forking preserves history (tree, not stack).** Picking an *alternate* caller forks the path — you step onto a road you haven't walked. Browser-back semantics (new branch clobbers forward history) are **wrong** here, because exploration is the whole point: "I traced A→B→C, now let me back up and see who else calls B, then return to my C thread." So history is a **tree** — a trunk with alternate-caller picks as branches you can walk out on and back from without losing the trunk. A *map of where you've been*, not a back button.

**Visited state is visible.** Explored branch points look different from unexplored ones — the caller column becomes a cross-section of your exploration tree at that node (breadcrumbs made visible). This falls out of state you're already tracking; don't discard it.

**Up/Down asymmetry resolved by directionality:** the peer set Up/Down cycles is "the relevant set in the direction you last moved," and history disambiguates — callees after Right, alternate callers after Left. One grammar, no mode confusion.

**Churn-safe:** the path is stored as **anchors / stable symbol IDs** (the same anchor system as the substrate), so exploration history survives the code being rewritten mid-session.

**Session-scoped:** the traversal tree is session state, not persistent (reviving durable memory is not the goal). It dies on close, with an optional "pin this path into the working set" escape hatch (ties to Insert/pin, 7.6).

### 7.5 Transitions — two distinct motions

There are **two** blur/transition types and they must have **distinct motion vocabularies**, or the user can't tell what just happened:

- **Descending** (Enter): file backdrop blurs as you drop from file into method-traversal — same space, going deeper. Reads as **depth: zoom-and-blur, push in.** The folder container dissolves so methods detach into the shared plane and a call-graph traversal can span files; call edges grow out of the held focus. **Hard requirement:** file-space persists **frozen** underneath method-space (a Z-layer you lift off of and drop back onto, holding exact position and scroll) — *not* regenerated on return. Escape lands you back on the file for the last-selected method.
- **Switching** (Tab): treemap ↔ call-graph on the held focus — same focus, lateral move. Reads as **rotation / cross-dissolve.** The **shared referent is the focused symbol**: it stays put through the cut while the surrounding context swaps from spatial-neighborhood to dependency-neighborhood. On switch, each space **re-frames its camera onto the current focus** — the rule that keeps decoupling safe. *Decoupled layout, shared focus.*

If both were "a blur," "I went deeper" and "I changed views" would be indistinguishable.

### 7.6 Full keymap

| Key | Action | Class |
|---|---|---|
| **Left** | Step out / back up the traversal path (history-caller); alternate callers listed above/below | focus |
| **Right** | Step in / dive into focused child or callee | focus |
| **Up / Down** | Cycle the current (directional) peer set | focus |
| **Page Up / Down** | Same axis as Up/Down, bigger stride (leap through a long list). *Boring version first; skip-the-subtree is the tempting-but-less-obvious alternative, deferred.* | focus |
| **Home** | Re-ground: frame root, overview. *The single most valuable key on an unbounded canvas.* | camera |
| **End** | Frame current focus at full fidelity. *Home → the map, End → the code.* (Softest assignment; let usage confirm.) | camera |
| **Enter** | Commit / cross: descend file → method-traversal (push-in transition) | mode |
| **Esc** | Step out of the current space/mode (blur-back / layer-lift) — distinct from Left's level-hop | mode |
| **Tab** | Switch treemap ↔ call-graph on held focus (lateral transition) | mode |
| **Insert / Delete** | Editing: Insert = "bring me in to edit this" (today pops to the real editor; later, inline edit on the same buffer, same key). *Pin/unpin working-set on a letter key.* (Real fork — decided in favor of editing owning Ins/Del.) | action |

### 7.7 Two connections worth keeping in mind

- **Arrow-stepping is quantized zoom.** Left/Right are discrete zoom steps snapped to structural levels, driving the *same* camera and LOD fidelity ladder as the mouse wheel's continuous zoom. Keyboard = snapped-to-structure motion; mouse = free motion; one camera underneath both.
- **Keyboard navigation is where determinism earns its keep most.** Because layout is fixed, "Left" slides the camera the same way every time, and that repetition builds the intra-session working map that replaces durable spatial memory. The muscle memory you *can't* build for *where things are* (churn erased that) you *can* build for *how motion behaves* — and deterministic layout is what makes the motion learnable.

---

## 8. Prior art and lineage (reference)

- **Code Bubbles** (Brown, Bragdon, ~2010) — functions pulled out of files as draggable fragments on a canvas; the direct ancestor of the floating-methods idea and the working-set/pin concept. Its durable-spatial-workspace model is the part we deliberately drop.
- **Light Table** (Granger, 2012) — pull-functions-out-of-files with live eval; instructive that it didn't stick.
- **Sourcetrail** (discontinued, open-sourced 2021) — the usage-graph view; couldn't reach sustainability.
- **Code Canvas / Debugger Canvas** (Microsoft Research, DeLine & Rowan) — semantic-zoom code on an infinite surface; our zoom-out ladder, nearly exactly.
- **Smalltalk / Pharo** — never had files; method-at-a-time browsing. Evidence the file-as-unit is a Unix artifact, not a law.
- **CodeCity** (Wettel & Lanza) — package hierarchy as spatial districts/buildings; hierarchy-as-space worked, metric-encoded-buildings got gimmicky.
- **CodeScene** (Tornhill) — behavioral code analysis; complexity × churn hotspots; the closest precedent for the signal layer.
- **Miller columns** (Finder) — the out/in/sibling navigation grammar, generalized here to the call graph.
- **Flame / icicle graphs** — battle-tested size-weighted hierarchy layout; our treemap-with-depth-on-x.
- **Slippy map tile pyramids** — the multi-resolution lattice, LOD, and floating-origin model.
- **ZUI / Pad++** (Perlin & Bederson) — semantic zoom; the layout-stability lesson that motivates determinism.

---

## 9. Cross-cutting invariants (the silent-failure checklist)

No implementation choice may violate these. Each one, broken, degrades the tool without throwing an error.

1. **Determinism.** Same structure → same layout, always. Same commit renders identically on any machine, for any reviewer.
2. **Continuity.** Small structural change → small visual change. This is diff legibility; it is the whole review use case.
3. **Stable ordering.** Order children by name/path, never by size. Size changes extent in place; it must never change *slot*.
4. **Anchors everywhere.** Every stored position — layout, call site, summary, traversal history — is an anchor/stable ID into the buffer, never a raw offset.
5. **Single buffer, one path.** Render from the buffer; don't build a separate read-path and write-path.
6. **Assess/explain paint split.** Structural signals own color and geometry and never speak prose. The LLM owns text only, at card fidelity and finer, always marked, and never drives color/heat/border.
7. **Every signal is inspectable.** No box paints hot from anything that can't decompose into metric · value · basis · threshold.
8. **Frozen layers, not regenerated.** File-space persists frozen under method-space; transitions lift and drop Z-layers, never tear down and rebuild.
9. **World-absolute layout, camera-relative view.** Boxes live at fixed world positions; navigation moves the camera. Nothing reflows on zoom.
10. **Floating origin.** Store coordinates hierarchically; compose only near-camera levels for f32. Never a single giant absolute coordinate.
11. **Focus is the connective tissue.** The two spaces are decoupled in geometry and coupled only through the focused symbol; every space switch re-frames onto focus.

---

## 10. Open questions and the next step

### 10.1 Soft spots to resolve through use
- **End key** meaning (frame-focus) is the least certain assignment; confirm or reassign with real usage.
- **Insert/Delete → editing vs. pin** was a genuine fork, decided for editing; revisit if pinning proves more frequent than inline edit early on.
- **Page Up/Down**: boring "bigger stride" first; consider skip-the-subtree later.
- **Layer order** for layering-violation detection: declared vs. inferred — needs a heuristic.

### 10.2 The one thing a whiteboard can't settle: the walking skeleton

Every decision in this document is internally consistent on paper. Exactly two are bets that only a running prototype can settle, and both are load-bearing for everything downstream:

1. **Does deterministic treemap layout + camera-follow *feel* like navigation, or like a slideshow?** (Arrow-stepping with the camera following, on a real repo.)
2. **Does the frozen-layer blur read as "same thing, new context," or as a teleport?** (The descend transition and its frozen file-space underneath.)

**The skeleton's job** is to stress-test those two and nothing else. It should:
- Load a real repo, compute code-size and the folder tree, and render the **stable, name-ordered treemap on the grid** (Sections 4.5–4.9).
- Implement **world-absolute layout + camera-follow** with Left/Right/Up/Down structural stepping and mouse pan/zoom (Sections 7.1–7.3), Left using simple history for now.
- Implement the **descend transition** with a genuinely frozen file-layer underneath (Section 7.5).

**What it's allowed to fake:** the call-graph space (stub it), all LLM narration, most signals (a single fill metric is enough to prove color-on-stable-shape), editing (none), and ACP (none). The point is the *feel* of deterministic navigation and the frozen-layer transition — the two unfalsifiable-in-the-abstract risks — before committing to the rest.

---

*End of v0.1. This document captures decisions and their reasoning; the reasoning is the guardrail. When a future change tempts you to break something here, check Section 9 first.*
