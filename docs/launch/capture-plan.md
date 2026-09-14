# Capture plan

Everything below is recorded from the Windows build driving `matter-engine-cpp`
(43,916 C++/h files), which already has nine authored view files in `.outrider/views/`,
including a 15-step tour across four tabs.

Recorder: ShareX, 60fps, MP4. Region = the app window only, no desktop, no taskbar.
Post MP4 natively to X. Convert to GIF only for the README (cap ~8MB, 15fps, 800px wide).

Golden rules for every clip:
- First frame is already interesting. No empty map, no loading spinner, no cursor hunting.
- No dead air. If indexing takes 20s, that's its own clip (#3), not the opening of another.
- Drive with the keyboard or the CLI, not the mouse, wherever possible — mouse drift reads as amateur.
- Record at a fixed window size so every clip in the set matches. 1600x1000 is a good default
  (crops to 16:10, legible on a phone).
- Dark theme throughout. Same theme in every clip and on the website.

---

## CLIP 1 — the agent-authored tour  [HERO — blocks everything else]

The only clip that is genuinely unavailable to anyone else in this conversation.
Target length 50-70s. This is the one that goes in post 1, the reply to Rik, and the
website hero.

Structure:
1. (0-8s) Split or cut-in: the terminal where Claude writes `.outrider/views/00-matter-engine-101.json`.
   Show the JSON scrolling past — sets, layers, camera.steps. Just enough that the viewer
   understands a *file* is being authored, not a chat reply.
2. (8s) Cut to the app. The watcher has opened the tab and auto-started the tour.
   Step 1 "Welcome to Matter Engine" — whole 44k-file repo framed, narration card anchored.
3. Walk the tour with -> . Do NOT show all 15 steps. Pick 6 that escalate:
   - 1  Welcome to Matter Engine            (whole repo)
   - 3  Foundation: five independent libraries  (has 5 parts — show 2 of them zooming)
   - 6  The kernel pipeline: script -> bake -> world
   - 10 The Baker seam                      (focus on one symbol, zooms to readable code)
   - 11 Every interface, as a class diagram (TAB HOP to graph space — the surprise beat)
   - 15 Where the work is happening         (churn fill + hotspot marks, whole repo again)
   The shape is: wide -> narrow -> one symbol -> different representation -> wide again.
4. End on step 15, hold 2s on the churn heat.

Drive it deterministically over RPC so takes are repeatable:
    $CLI $P tour goto 0 ; (record) ; $CLI $P tour goto 2 ; ...
Use `tour goto <n>` between beats rather than mashing ->, so the pacing is yours.

Caption to pair with it: "I asked Claude to explain this codebase. It didn't write me a
paragraph — it drove the map."

---

## CLIP 2 — overnight diff + churn        [the thesis clip]

20-25s. This is the one that proves the *why* rather than the *wow*.

- Open on the treemap with a diff view active — boxes an agent touched, marked by diff type.
- Then push the churn fill (`06-hotspots.json` already does churn + hotspot marks).
- One deliberate zoom into a single changed symbol so the live glyphs come up and you can
  read the actual changed code.

The point being made: this is computed from git and the AST. Nothing on screen came from
a model's opinion. Say that in the post text, not in an overlay.

Needs: a real agent session against matter-engine-cpp first, so the diff is genuine.
If none is recent, run one — do not fake it.

---

## CLIP 3 — cold start                    [proof of scale]

15s, no cuts, real time, with a visible clock or just obviously uncut.
- Launch on matter-engine-cpp from nothing.
- Progressive packing streams the live tree in while indexing continues.
- Ends with the whole 44k-file repo framed and interactive.

MEASURE the wall-clock time here and use that exact number in post 2. This clip is the
evidence for the claim, so the number and the video have to agree.

---

## CLIP 4 — the LOD ladder end to end     [answers Rik directly]

15-20s, one continuous zoom, no cuts. This is the clip that speaks to the rendering crowd.
- Start fully zoomed out: 44k files as dots/fills.
- One smooth continuous zoom into a single function until glyphs are live and crisp.
- The whole ladder should be visible in one motion: Dot -> Label -> Minimap texture ->
  live shaped text at the 4px/line crossover.
- Optionally Ctrl+T first, type a symbol name, and let the camera fly there instead of
  manual scroll — faster and shows off search.

Do not narrate the tiers in an overlay. Let the motion do it, name the tiers in the post text.

---

## CLIP 5 — graph space                   [breadth]

15s. Tab hop between representations with the focused symbol preserved.
- Base treemap, symbol focused.
- Ctrl+Tab to `07-layer-cake.json` (dependency graph) — same symbol reframed.
- Ctrl+Tab to `04-bake-seams.json` or the class diagram — UML-ish boxes with members.
The beat that lands: the focus *follows you across representations*.

---

## CLIP 6 — the comment loop              [the differentiated workflow]

25s. Lowest priority for X, highest value for the website's "how it works" section.
- Mid-tour, press `c`, type a comment on a symbol.
- Comment appears in the right-hand column.
- Cut to terminal: `outrider-cli comments list` -> agent reads it -> authors a new focused view.
- Cut back: new tab opens answering the comment.

This is the full oversight loop and nobody else has it. It's just slower to read than clip 1,
so it belongs on the site rather than in the launch post.

---

## Stills needed (website + README + repo social preview)

1. Hero still: whole 44k-file repo framed, churn fill on. 2560px wide.
2. Zoomed-in still: one function, live glyphs, doc panel visible. Proves it's readable.
3. Class diagram still from graph space.
4. Tour still: narration card anchored next to its target, navigator panel visible.
5. Repo social preview (1280x640) — crop of #1 with the name overlaid.

---

## Repo gaps to close before any of this goes out

Every click from X lands on github.com/ElectricJack/outrider-ide. Right now it has:
- [ ] no description
- [ ] no topics
- [ ] no images in the README
- [ ] no social preview image
- [ ] releases exist (v0.1.1, Jul 16) — CONFIRM they carry mac/win/linux binaries.
      If the only install path is "cargo build --release, Rust 1.89+", most of the
      audience bounces.
