# Phase 0 — Platform Gate (Workspace + GPUI Hello-World) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove GPUI renders under WSLg/Vulkan on this machine (spec milestone 0) inside a properly scaffolded Cargo workspace, before any app code is written.

**Architecture:** A three-crate Cargo workspace (`outrider-index`, `outrider-layout` as libs; `outrider` as the GPUI binary). This phase touches only scaffolding and the `outrider` crate. GPUI is a git dependency pinned by revision.

**Tech Stack:** Rust (edition 2021), Cargo workspace, GPUI (pinned git rev from zed-industries/zed), WSLg + Vulkan.

**Source spec:** `docs/superpowers/specs/2026-07-05-outrider-walking-skeleton-design.md` §2, §9 (milestone 0), §10.
**Roadmap:** `docs/superpowers/plans/2026-07-08-walking-skeleton-roadmap.md` (Phase 0).

## Global Constraints

- Binary is named `outrider`; crates are named `outrider-*` (spec §2).
- GPUI pinned to an exact git revision; never a floating branch (spec §10: "Pin revision; upgrade deliberately, not passively").
- Platform: Linux under WSL2 (WSLg + Vulkan). Fallback if WSLg GPU is flaky: Windows-native build (spec §2) — that fallback is a **decision point for the user**, not something this plan executes.
- No GPUI types may ever appear in `outrider-index` or `outrider-layout` (spec §4). This phase leaves both as empty lib crates.
- `.outrider/` must be gitignored (spec §5.4).

**The exit gate of this plan is manual:** a human confirms the window renders and resizes cleanly under WSLg. GPU output is not unit-testable; do not fake a test for it.

---

### Task 1: Cargo workspace scaffold

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/outrider-index/Cargo.toml`, `crates/outrider-index/src/lib.rs`
- Create: `crates/outrider-layout/Cargo.toml`, `crates/outrider-layout/src/lib.rs`
- Create: `crates/outrider/Cargo.toml`, `crates/outrider/src/main.rs`
- Create: `.gitignore`

**Interfaces:**
- Consumes: nothing (first code in the repo).
- Produces: the workspace layout every later phase builds inside. Crate names `outrider-index`, `outrider-layout`, `outrider` and the `crates/` directory layout are load-bearing for all other plans.

- [ ] **Step 1: Create the workspace root `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = [
    "crates/outrider-index",
    "crates/outrider-layout",
    "crates/outrider",
]
```

- [ ] **Step 2: Create `.gitignore`**

```gitignore
/target
.outrider/
```

- [ ] **Step 3: Create the two lib crates**

`crates/outrider-index/Cargo.toml`:

```toml
[package]
name = "outrider-index"
version = "0.1.0"
edition = "2021"

[dependencies]
```

`crates/outrider-index/src/lib.rs`:

```rust
```

(empty file — Phase 1 fills it)

`crates/outrider-layout/Cargo.toml`:

```toml
[package]
name = "outrider-layout"
version = "0.1.0"
edition = "2021"

[dependencies]
```

`crates/outrider-layout/src/lib.rs`:

```rust
```

(empty file — Phase 2 fills it)

- [ ] **Step 4: Create the app crate**

`crates/outrider/Cargo.toml`:

```toml
[package]
name = "outrider"
version = "0.1.0"
edition = "2021"

[dependencies]
```

`crates/outrider/src/main.rs`:

```rust
fn main() {
    println!("outrider: milestone 0 scaffold");
}
```

- [ ] **Step 5: Verify the workspace builds and tests run**

Run: `cargo build && cargo test`
Expected: build succeeds; test run reports `0 passed` for each crate with no failures.

Run: `cargo run -p outrider`
Expected: prints `outrider: milestone 0 scaffold`

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore crates/
git commit -m "feat: scaffold outrider cargo workspace (index, layout, app crates)"
```

---

### Task 2: System dependencies and Vulkan sanity check

**Files:**
- No repo files. Host-machine setup only. (Findings get recorded in the Task 4 verdict.)

**Interfaces:**
- Consumes: nothing.
- Produces: a working Vulkan device under WSLg, required by Task 4.

- [ ] **Step 1: Install build and runtime dependencies**

GPUI on Linux needs Wayland/X11, xkbcommon, Vulkan, fontconfig, and audio dev headers to compile. Run:

```bash
sudo apt-get update && sudo apt-get install -y \
  build-essential clang pkg-config \
  libfontconfig-dev libasound2-dev libssl-dev libzstd-dev \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libx11-xcb-dev \
  libvulkan1 mesa-vulkan-drivers vulkan-tools
```

Note: if the Task 4 build fails on a missing system library, install its `-dev` package; the authoritative dependency list is `script/linux` in the Zed repository at the pinned revision.

- [ ] **Step 2: Verify a Vulkan device exists under WSLg**

Run: `vulkaninfo --summary`
Expected: at least one `GPU0` entry. Under WSLg this is typically the Dozen driver (`Microsoft Direct3D12 (...)` via mesa). If **no device** is listed, stop: WSLg Vulkan is not working, and that finding goes straight to the Task 4 gate (fallback decision) without writing the hello-world.

- [ ] **Step 3: Record the driver line**

Copy the GPU name/driver line from `vulkaninfo --summary` output — it goes in the Task 4 verdict note.

(No commit — no repo changes.)

---

### Task 3: Pin GPUI as a git dependency

**Files:**
- Modify: `crates/outrider/Cargo.toml`
- Modify: `Cargo.lock` (generated)

**Interfaces:**
- Consumes: workspace from Task 1.
- Produces: a buildable `gpui` dependency at a pinned rev — the rev every later phase compiles against.

- [ ] **Step 1: Resolve the revision to pin**

Run: `git ls-remote https://github.com/zed-industries/zed.git refs/heads/main`
Expected: one line, `<40-char-sha>\trefs/heads/main`. That SHA is the pin.

- [ ] **Step 2: Add the pinned dependency**

In `crates/outrider/Cargo.toml`, replace the `[dependencies]` section with (substituting the real SHA):

```toml
[dependencies]
gpui = { git = "https://github.com/zed-industries/zed", rev = "<SHA-FROM-STEP-1>" }
```

- [ ] **Step 3: Build**

Run: `cargo build -p outrider`
Expected: success. **First build compiles GPUI and takes on the order of 10–20 minutes**; run it in the background and check on completion, don't assume failure from silence. If it fails on a missing system header, see Task 2 Step 1's note, install, retry.

- [ ] **Step 4: Commit**

```bash
git add crates/outrider/Cargo.toml Cargo.lock
git commit -m "feat: pin gpui git dependency for milestone 0"
```

---

### Task 4: Hello-world window + the manual platform gate

**Files:**
- Modify: `crates/outrider/src/main.rs`
- Create: `docs/superpowers/plans/2026-07-08-phase-0-verdict.md` (gate record)

**Interfaces:**
- Consumes: pinned gpui from Task 3.
- Produces: the platform verdict (WSLg OK / fallback needed) that gates Phases 3–6 planning.

- [ ] **Step 1: Write the hello-world app**

Replace `crates/outrider/src/main.rs` with:

```rust
use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, Window, WindowBounds,
    WindowOptions,
};

struct HelloWorld;

impl Render for HelloWorld {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .justify_center()
            .items_center()
            .bg(rgb(0x1e2430))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(240.))
                            .h(px(120.))
                            .rounded_md()
                            .bg(rgb(0x2e7d32)),
                    )
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(0xffffff))
                            .child("outrider — milestone 0"),
                    ),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| HelloWorld),
        )
        .expect("failed to open window");
    });
}
```

**API-drift note:** GPUI's API moves. If this does not compile against the pinned rev, adapt from `crates/gpui/examples/hello_world.rs` *in the Zed checkout at the pinned revision* (cargo puts it under `~/.cargo/git/checkouts/zed-*/<rev>/`). The requirement is a window with one colored quad and one text run — not this exact code.

- [ ] **Step 2: Build and run**

Run: `cargo run -p outrider`
Expected: an 800×600 window opens under WSLg showing a green rectangle and the text "outrider — milestone 0" on a dark background.

- [ ] **Step 3: Manual gate checklist (human at the keyboard)**

Verify each:
1. Window opens and renders (quad color correct, text crisp).
2. Resize the window: content re-lays-out, no black frames, no artifacts, no crash.
3. Drag the window around: no smearing or stalls.
4. Close the window: process exits cleanly (exit code 0).

- [ ] **Step 4: Record the verdict**

Create `docs/superpowers/plans/2026-07-08-phase-0-verdict.md`:

```markdown
# Milestone 0 verdict — GPUI under WSLg

- Date: <date>
- GPUI rev: <pinned SHA>
- Vulkan device (from `vulkaninfo --summary`): <driver line from Task 2>
- Checklist: render <pass/fail> · resize <pass/fail> · drag <pass/fail> · clean exit <pass/fail>
- **Verdict: WSLg viable / fallback to Windows-native required**
- Notes: <anything flaky, workarounds applied>
```

**If the verdict is "fallback required": STOP.** Do not proceed to Phase 3+ planning; surface the verdict to the user — the Windows-native fallback changes the setup assumptions of every rendering phase (roadmap sequencing note).

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/main.rs docs/superpowers/plans/2026-07-08-phase-0-verdict.md
git commit -m "feat: gpui hello-world window; record milestone 0 platform verdict"
```

---

## Post-implementation notes (2026-07-08)

- At the pinned rev (029bf2f2), `gpui::Application::new()` no longer exists; the entry point is `gpui_platform::application()`. Task 3's dependency block therefore gained a second dep: `gpui_platform = { git = "https://github.com/zed-industries/zed", rev = "<same SHA>", features = ["wayland"] }`. Any future rev bump must move **both** deps in lockstep.
- The `wayland` feature on `gpui_platform` is required on Linux; without it the build has no platform backend.
- Verdict details (llvmpipe-only Vulkan, borderless window under WSLg) are in `2026-07-08-phase-0-verdict.md`.
