# X launch drafts

Context: Rik Arends (@rikarends) posted the Makepad Studio code atlas on ~Sep 11 2026.
Yoann Padioleau (@yoann_padioleau) replied Sep 12 with aryx/codemap (89 likes / 9.6K views),
prior art from ~2010-2013 at Facebook via pfff. Rik replied: "I would never have been able
to do this 15 years ago. Im maxxing out an m3 max."

Thread lineage now on the table: SeeSoft (Bell Labs, 1992) -> Linux kernel treemap (2002)
-> MS Code Thumbnails (2006) -> codemap (2010-13) -> Makepad code atlas (2026).

## Ground rules

- Do NOT claim originality. Three people have already claimed it in this thread and the
  substrate is 34 years old.
- Do NOT imply I built on prior art I'd seen. I hadn't. I found all of it after this thread.
  Independent convergence is the true story and the better one.
- Started Outrider July 2026. Say so plainly if asked; the repo is public and checkable.
- The differentiator is WHY, not WHAT: seeing what agents changed, with structural signals
  owning all color/geometry and the LLM allowed only to narrate, never to assess.
- Don't position as the crude-but-cheap option. The LOD ladder converged on the same shape
  as his; the divergence is symbols-and-change vs lines-and-surface.

## Post 1 — standalone (post first, pin it)

> Everyone who wants this badly enough eventually builds it.
>
> I started Outrider in July because I wanted to see what agents actually changed in a
> large codebase. When a repo gets rewritten overnight, the only fast explanation available
> is the agent's own account of its homework — and that's exactly where confident-wrong hides.
>
> So: the whole codebase as a treemap down to individual symbols. Structural signals — churn,
> call graph, inheritance, diff — own every color and every bit of geometry, computed from the
> repo and inspectable down to the number. The LLM is only ever allowed to narrate. It never
> drives an assessment, a color, or a border.
>
> The flip side: agents author the views. Here's Claude writing a 15-step guided tour of a
> 44,000-file C++ engine it had just read, and the app driving it — camera moves, spotlight
> masks, narration anchored to the thing it describes.
>
> [CLIP 1]
>
> Native Rust, GPU-rendered on GPUI. MIT. Links below.

## Post 2 — reply into Rik's thread (~2h after post 1)

> Beautiful work. This thread has been an education — I've been building something in the
> same shape since July and had no idea about any of the prior art until yesterday. SeeSoft
> in '92, @yoann_padioleau's codemap with semantic sizing and coloring. Apparently everyone
> who wants this badly enough just builds it.
>
> Mine came from a different itch: I wanted to see what agents change in a large codebase.
> When a repo gets rewritten overnight the only fast explanation available is the agent's own
> account of its homework. So structural signals — churn, call graph, diff — own all the color
> and geometry, and the LLM is only ever allowed to narrate, never to assess.
>
> The flip side is that agents *author* the views. Here's Claude writing a guided tour of a
> 44k-file C++ codebase it just read, and the app driving it:
>
> [CLIP 1]
>
> Re: maxxing the M3 — a data point from the cheap end. Same idea of a semantic LOD ladder,
> but the tiers are dot -> label -> baked texture -> live shaped glyphs, crossing over at
> 4px/line, and the far tier is a cached texture rather than anything resolution-independent.
> ~Ns to index the 44k-file repo and it stays interactive on ordinary hardware. What I give
> up is holding the entire surface live — I merge boxes below 4px and pack by symbol instead
> of keeping every line addressable. Curious whether the vector tier earns its cost for a
> comprehension-only view, or whether a glyph atlas gets you there.

  MEASURE the index time fresh before posting and replace ~Ns. A number in a reply to this
  thread will be checked.

## Post 3 — reply to Yoann (separate, same day)

> codemap is the closest thing to what I built that I've found, and I found it an hour ago.
> Semantic sizing and coloring, thumbnails, zoom-to-source — you had the whole shape in
> OCaml before I'd written a line. Going to spend the weekend reading it.
>
> The one thing I'd add for 2026: when the code is being written by agents faster than anyone
> can read it, the map stops being a navigation aid and starts being an oversight instrument.
> Different job for the same picture.

## Follow-up clips, one per day, each as its own post

- Day 1: overnight diff + churn heat — "what the agent actually touched, from git, not from its summary"
- Day 2: cold start on the 44k-file repo, progressive packing streaming in
- Day 3: Ctrl+T symbol search -> zoom to live glyphs (shows the LOD ladder end to end)
- Day 4: graph space — inheritance class diagram and the layer-cake dependency view
- Day 5: the comment loop — user presses `c`, agent reads `comments list`, answers with a new view

## Lines to keep

- "Apparently everyone who wants this badly enough just builds it."
- "The author is narrating its own homework."
- "Structural signals assess. The LLM explains."
