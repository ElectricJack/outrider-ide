# Outrider Walking Skeleton — Implementation Roadmap

**Status:** Roadmap v1.0
**Source spec:** `docs/superpowers/specs/2026-07-05-outrider-walking-skeleton-design.md` (approved, v1.0)
**Parent design:** `docs/code-comprehension-viewer-design.md` (v0.1)

This roadmap decomposes the skeleton spec into six phase plans. Each phase is
independently plannable (one detailed TDD plan per phase), produces working,
testable software on its own, and ends at a reviewable gate. Phases follow the
spec's §9 milestones but make the dependency structure explicit — the two
headless crates do not depend on the platform gate and can proceed in parallel
with it or ahead of it.

---

## Dependency graph

```
Phase 0: Platform gate (GPUI + WSLg) ──────────────┐
                                                   │
Phase 1: outrider-index ──► Phase 2: outrider-layout ──► Phase 3: Render + camera
                                                              │
                                                    Phase 4: Structural navigation   [BET #1 GATE]
                                                              │
                                                    Phase 5: Descend transition      [BET #2 GATE]
                                                              │
                                                    Phase 6: Live reload + verdict   [PROJECT GATE]
```

- **Critical path:** 0 → 3 → 4 → 5 → 6.
- **Parallel track:** Phases 1 and 2 are pure Rust with no GPUI types (spec §4)
  and can be built headless regardless of Phase 0's outcome. Start Phase 0 and
  Phase 1 together; if WSLg proves flaky, index/layout work continues unblocked
  while the Windows-native fallback is set up.
- Phase 3 is the first point where all three tracks join.

---

## Phase 0 — Platform gate: GPUI hello-world under WSLg

*Spec §9 milestone 0, §10 risk 1–2.*

**Deliverable:** Cargo workspace (`outrider-index`, `outrider-layout`,
`outrider`) with the `outrider` binary opening a GPUI window that renders a
quad and text under WSLg/Vulkan. GPUI pinned to a specific git revision.

**Contents:**
- Workspace scaffold, `rust-toolchain.toml`, `.gitignore` (including
  `.outrider/`), CI skeleton (fmt + clippy + test).
- GPUI dependency pinned by revision (spec §10: upgrade deliberately, never
  passively).
- Minimal render: window, one colored quad, one text run — enough to prove the
  Vulkan path on this driver.

**Exit gate (decision point):** window renders and resizes without artifacts
under WSLg. **If it fails:** switch the app target to Windows-native (spec's
named fallback) before writing any app code. This gate exists precisely so the
platform bet is settled before the expensive phases.

**Test story:** manual verification (GPU rendering is not unit-testable);
CI runs headless `cargo test` for the workspace.

---

## Phase 1 — `outrider-index`: repo → SymbolTree

*Spec §5, §9 milestone 1. No GPUI dependency.*

**Deliverable:** `outrider-index` crate producing a `SymbolTree` (with churn
percentiles) from a real repository, plus a CLI dump binary for inspection.
All fixture tests pass.

**Contents, in task order:**
1. Core types: `SymbolId`, `SymbolKind`, `SymbolNode`, `SymbolTree`
   (spec §4.1) — including ordinal disambiguation and name-sorted children.
2. Scan: `ignore`-crate walk; all non-ignored files contribute size to folder
   measure; only `.rs` files descend (spec §5.1).
3. Parse: `tree-sitter-rust` extraction of `mod`/`struct`/`enum`/`trait`/
   `impl`/`fn` with byte ranges and nesting; `rayon` parallel; line-count
   measures (spec §5.2).
4. Churn: one `git log --numstat --no-renames` subprocess pass → commit counts
   → within-repo percentiles (files and folders ranked separately, methods
   inherit file churn); cache keyed by HEAD hash in
   `.outrider/churn-cache.json` (spec §5.4).
5. CLI dump binary (`outrider-index-dump` or a subcommand) printing the tree
   with measures and churn — the milestone-1 acceptance artifact.
6. Substrate seams honored at interface level: byte ranges stored per node so
   Phase 4's rope/anchor materialization plugs in without reshaping the tree.

**Tests (spec §8.2):**
- Fixture mini-repo in `tests/fixtures/` → exact expected `SymbolTree`
  (names, kinds, nesting, measures).
- Churn percentile math on synthetic history.
- `.gitignore`d files contribute nothing.

**Exit gate:** fixture tests pass; CLI dump of Outrider's own repo looks
correct by inspection.

---

## Phase 2 — `outrider-layout`: SymbolTree → WorldLayout

*Spec §6, §9 milestone 2. Pure function, no GPUI dependency. Depends on
Phase 1's types only (can start once §4.1 types exist, before churn lands).*

**Deliverable:** `outrider-layout` crate mapping any `SymbolTree` to a
`WorldLayout`; all five property tests pass headless; cross-process
determinism verified in CI.

**Contents, in task order:**
1. Types: `CellRange`, `NodeLayout`, `WorldLayout` with `ratio = 8`
   (spec §4.1, §6.1).
2. Measure pass (post-order): leaf cells from `lines_per_cell` per level
   (initial: 32 lines/cell at file level, 4 at method level); parent cells
   `ceil((Σ child + slack) / r)` with 15% slack; round-up-only monotonicity
   (spec §6.2).
3. Arrange pass (pre-order): name-order placement, slack distributed as
   per-child gaps + remainder at end, parent-relative `CellRange.start`
   (hierarchical addressing / floating origin, spec §6.3).
4. Determinism discipline enforced structurally: `BTreeMap`/sorted `Vec` only,
   integer-only cell math (spec §6.4).

**Tests (spec §8.1, proptest):**
1. Determinism — byte-identical serialized layout across runs *and* across two
   processes in CI.
2. Continuity/grow — bounded displacement on leaf growth.
3. Continuity/insert — bounded displacement on file insertion into slack.
4. Stable ordering — sibling order invariant under size permutation.
5. Containment/no-overlap.

**Exit gate:** all five property tests pass. Per spec §10: if a continuity
bound proves wrong, tighten the algorithm, not the test.

---

## Phase 3 — Render + camera

*Spec §7.1 (camera rows only), §7.2 (Dot/Label/Card rungs), §9 milestone 3.
Requires Phases 0, 1, 2.*

**Deliverable:** the treemap of a real repo on screen; mouse pan and
continuous zoom; Home frames root; Dot → Label → Card fidelity rungs driven by
on-screen pixel height; churn fill color.

**Contents:**
1. Startup pipeline: index → layout → render, one-way data flow (spec §4).
2. World-coordinate composition at render time from ancestors near the camera
   (floating origin — f32 sees only local deltas, spec §6.3).
3. Visibility as a range query on cell ranges (the grid is the spatial index,
   spec §6.1).
4. Camera: mouse drag pan, wheel zoom, Home; camera state owned by the app,
   never by layout.
5. Rungs: Dot (fill only, <4px merge to parent tile), Label (truncated name),
   Card (name + `47 · p96` churn readout + line count). Tune `lines_per_cell`
   here per spec §6.2.

**Exit gate:** navigate Outrider's own repo by mouse; boxes never move unless
data changes; rungs switch by pixel height without flicker.

**Test story:** rung-selection and world-composition math unit-tested
headless; rendering itself verified manually (this phase begins the
manual-feel territory the skeleton exists for).

---

## Phase 4 — Structural navigation + Detail/Full rungs  ⟶ **Bet #1 assessable**

*Spec §7.1 (focus rows), §7.2 (Detail/Full), §5.3 substrate doors 1–3,
§9 milestone 4.*

**Deliverable:** keyboard-primary navigation with camera-follow; the Detail
and Full rungs, with Full rendering tree-sitter-highlighted code from a rope.

**Contents:**
1. Focus model: click sets focus (camera holds); Right steps into first/
   last-visited child; Up/Down cycle name-ordered siblings; Left pops a simple
   linear history stack of `SymbolId` (spec §7.1 — tree-history is explicitly
   post-skeleton).
2. Camera-follow policy: frame focus + parent, pan right as focus deepens,
   ~250 ms eased, interruptible/retargetable mid-flight (spec §7.1). End
   frames focus at Full. Tab explicitly disabled.
3. Substrate doors 1–3 (spec §5.3): `ropey::Rope` materialization for
   Full-fidelity files, LRU-capped (~64); anchor-shaped marker interface (not
   sum_tree); Full rung reads only from the rope, never a cached string.
4. Detail rung: file shows method names as sub-labels; method shows full
   signature. Full rung: tree-sitter-highlighted code.

**Exit gate — Bet #1 assessable:** arrow-step through a real repo with
camera-follow. Informal check of spec §8.3 question 1 (place vs. slideshow)
before investing in Phase 5. A "slideshow" read here is a design finding —
surface it immediately rather than proceeding.

---

## Phase 5 — Descend transition  ⟶ **Bet #2 assessable**

*Spec §7.3, §9 milestone 5.*

**Deliverable:** Enter/Esc mode transition with a genuinely frozen file layer.

**Contents:**
1. Enter (on method, or file → its first method): file layer **frozen** —
   retained with exact box positions and camera state, dimmed + blurred
   beneath the method plane; never torn down or regenerated (parent
   invariant 8). Blur is cosmetic; dim-only is the sanctioned fallback if GPUI
   blur is awkward (spec §10) — *frozen* is the requirement.
2. Method plane: focused method at Detail/Full; peers = the file's other
   methods in name order (real data, no faked call edges); arrows operate on
   that peer set.
3. Motion vocabulary: push-in zoom + blur (depth), reversed exactly by Esc.
4. Esc lands on the file with last-selected method focused; file-layer camera
   and scroll state bit-identical to the moment of descent.

**Exit gate — Bet #2 assessable:** round-trip fidelity is the acceptance test
(spec §7.3). Enter/Esc/Enter/Esc repeatedly: zero drift in the frozen layer.

---

## Phase 6 — Live reload + validation protocol  ⟶ **Project gate**

*Spec §7.4, §5.3 door 4, §8.3, §9 milestone 6.*

**Deliverable:** disk-change → re-index → re-layout → re-render with unchanged
nodes motionless; the written verdict on both bets.

**Contents:**
1. `notify` file watching; on change: incremental tree-sitter re-parse
   (`InputEdit`) for materialized files, full re-parse for unmaterialized —
   the same code path a future user-edit will take (substrate door 4).
2. Rebuild affected `SymbolTree` subtree → re-layout → diff `WorldLayout` →
   re-render. Unchanged nodes must not move (invariant 2, live).
3. Manual validation protocol (spec §8.3): 30 minutes keyboard-primary on
   (a) Outrider's own repo, (b) one large foreign Rust repo (`zed` or
   `ripgrep`); answer the four judgment questions; record a short written
   verdict in `docs/`.

**Exit gate:** the verdict. Both bets pass → proceed per spec §12
(import-graph space, signal layer, tree-history, Tab). Either fails →
back to the parent document, cheaply — that is the skeleton doing its job.

---

## Cross-cutting constraints (apply to every phase plan)

Copied from the spec; every detailed phase plan inherits these as global
constraints:

- **Crate hygiene:** no GPUI types in `outrider-index` or `outrider-layout`;
  data flows one way; the app mutates neither tree nor layout (spec §4).
- **Determinism:** `BTreeMap`/sorted collections only in layout; no float
  accumulation in cell math; floats at render composition only (spec §6.4).
- **Stable identity:** `SymbolId` (qualified path + ordinal), never byte
  offsets, as the key for layout and history (spec §4.1).
- **Naming:** binary `outrider`, crates `outrider-*` (spec §2).
- **GPUI pinned** by git revision; upgrades are deliberate (spec §10).
- **Out of scope — reject if it creeps in:** Tab/call-graph space, call edges
  real or faked, LLM narration, ACP, editing, minimap, jump-to-symbol search,
  signals beyond churn, languages beyond Rust, persistence beyond the churn
  cache, multi-repo, config UI (spec §3).

## Sequencing recommendation

1. Kick off **Phase 0 and Phase 1 in parallel** (Phase 0 is small and
   risk-gating; Phase 1 is pure Rust and unblocked by it).
2. Phase 2 immediately after Phase 1's types exist.
3. Phases 3 → 4 → 5 → 6 strictly sequential on the critical path, with the
   informal bet-#1 check at the end of Phase 4 as an early-warning gate.

Each phase gets its own detailed TDD implementation plan
(`docs/superpowers/plans/`) written just-in-time — Phase 0/1 plans now;
Phase 3+ plans only after the platform gate settles, since a Windows-native
fallback would change their setup details.
