# Alpha Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prepare outrider-ide for alpha distribution — settings persistence, file filtering, folder dialog, welcome screen, settings window, right-click menu, and README.

**Architecture:** New `settings` module for persistence. `scan_files` gains a filter parameter. `main.rs` gains folder dialog via `rfd`. Three new overlays in `TreemapView` (welcome, settings window, context menu). README at project root.

**Tech Stack:** Rust, GPUI, `dirs` crate, `rfd` crate

## Global Constraints

- New dependencies: `dirs` (outrider crate), `rfd` (outrider crate). `serde`/`serde_json` already in outrider-index; add to outrider crate too.
- Settings path: `dirs::config_dir() / "outrider" / "settings.json"`.
- All overlays use the existing style: `theme::CODE_BG` background, `theme::FOCUS_BORDER` border, `theme::FONT_FAMILY` font.
- Overlay precedence: only one overlay active at a time (welcome, palette, settings, context menu). Opening one closes others.
- No changes to the layout algorithm or indexing pipeline structure.

---

### Task 1: Settings Module

Create `crates/outrider/src/settings.rs` — a `Settings` struct with serde JSON persistence.

**Files:**
- Create: `crates/outrider/src/settings.rs`
- Modify: `crates/outrider/src/main.rs` (add `mod settings;`)
- Modify: `crates/outrider/Cargo.toml` (add `dirs`, `serde`, `serde_json`)

**Interfaces:**
- Produces: `Settings` struct with `filter_extensions: Vec<String>`, `filter_folders: Vec<String>`, `show_welcome: bool`
- Produces: `Settings::load() -> Settings` (loads from disk or returns defaults)
- Produces: `Settings::save(&self)` (writes to disk, creates parent dirs)
- Produces: `Settings::default()` with the standard filter lists

- [ ] **Step 1: Add dependencies to Cargo.toml**

Add to `crates/outrider/Cargo.toml` under `[dependencies]`:
```toml
dirs = "6"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
rfd = "0.15"
```

- [ ] **Step 2: Create settings.rs**

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub filter_extensions: Vec<String>,
    pub filter_folders: Vec<String>,
    pub show_welcome: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            filter_extensions: vec![
                "exe", "dll", "obj", "o", "so", "dylib", "a", "lib", "pdb",
                "class", "pyc", "wasm", "bin", "dat", "db", "sqlite",
                "png", "jpg", "jpeg", "gif", "ico", "bmp", "svg",
                "mp3", "mp4", "wav", "zip", "tar", "gz", "7z", "rar",
                "pdf", "ttf", "otf", "woff", "woff2",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            filter_folders: vec![
                "target", "node_modules", "dist", "build", "__pycache__",
                ".next", ".nuxt", "out", "pkg", "vendor",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            show_welcome: true,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("outrider").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}
```

- [ ] **Step 3: Register module in main.rs**

Add `mod settings;` to the module declarations in `crates/outrider/src/main.rs`.

- [ ] **Step 4: Build and test**

Run: `cargo build -p outrider 2>&1 | head -20`
Run: `cargo test -p outrider 2>&1 | tail -10`

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/settings.rs crates/outrider/src/main.rs crates/outrider/Cargo.toml Cargo.lock
git commit -m "feat: add settings module with JSON persistence"
```

---

### Task 2: File Filtering in Scanner

Pass filter settings into `scan_files` to skip binary/build files.

**Files:**
- Modify: `crates/outrider-index/src/scan.rs` (add filter params to `scan_files`)
- Modify: `crates/outrider-index/src/index.rs` (pass filters through `index_repo`)
- Modify: `crates/outrider-index/src/lib.rs` (update re-export)
- Modify: `crates/outrider/src/main.rs` (pass settings to `index_repo`)

**Interfaces:**
- Consumes: `Settings.filter_extensions`, `Settings.filter_folders` from Task 1
- Produces: `index_repo(repo_root, filter_extensions, filter_folders)` updated signature
- Produces: `scan_files(repo_root, filter_extensions, filter_folders)` updated signature

- [ ] **Step 1: Update scan_files signature**

In `crates/outrider-index/src/scan.rs`, change `scan_files` to accept filter parameters:

```rust
pub fn scan_files(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
) -> anyhow::Result<Vec<ScannedFile>> {
    let mut files = Vec::new();
    let mut walker_builder = WalkBuilder::new(repo_root);
    walker_builder.require_git(false);
    for folder in filter_folders {
        walker_builder.filter_entry(move |entry| {
            // This closure approach won't work with WalkBuilder — see step below
            true
        });
    }
    // ... (see actual implementation below)
```

The `ignore` crate's `WalkBuilder` supports custom ignore via `.add_custom_ignore_filename()` but the simplest approach is to filter in the loop body:

```rust
pub fn scan_files(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
) -> anyhow::Result<Vec<ScannedFile>> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(repo_root).require_git(false).build();
    for entry in walker {
        let entry = entry?;
        // Skip filtered folders
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            // The `ignore` walker won't let us skip mid-iteration easily,
            // but folder entries appear before their children, and we can
            // filter files by checking path components below.
            continue;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry.path().extension().is_some_and(|e| e == "lock") {
            continue;
        }
        // Skip files with filtered extensions
        if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
            if filter_extensions.iter().any(|f| f.eq_ignore_ascii_case(ext)) {
                continue;
            }
        }
        // Skip files inside filtered folders
        let rel = entry.path().strip_prefix(repo_root).unwrap_or(entry.path());
        if rel.components().any(|c| {
            filter_folders.iter().any(|f| c.as_os_str().to_string_lossy() == *f)
        }) {
            continue;
        }
        let rel_path = rel.to_path_buf();
        let bytes = std::fs::read(entry.path())
            .with_context(|| format!("reading {}", entry.path().display()))?;
        files.push(ScannedFile {
            rel_path,
            lines: count_lines(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(files)
}
```

- [ ] **Step 2: Update index_repo signature**

In `crates/outrider-index/src/index.rs`:

```rust
pub fn index_repo(
    repo_root: &Path,
    filter_extensions: &[String],
    filter_folders: &[String],
) -> anyhow::Result<SymbolTree> {
    let files = scan_files(repo_root, filter_extensions, filter_folders)?;
    // ... rest unchanged
}
```

- [ ] **Step 3: Update main.rs to pass settings**

In `crates/outrider/src/main.rs`, load settings and pass filters:

```rust
let settings = settings::Settings::load();
let tree = match outrider_index::index_repo(
    &repo,
    &settings.filter_extensions,
    &settings.filter_folders,
) {
    // ...
};
```

- [ ] **Step 4: Fix existing tests**

Update any tests calling `scan_files` or `index_repo` to pass empty filter vecs `&[], &[]`.

- [ ] **Step 5: Build and test**

Run: `cargo build 2>&1 | head -20`
Run: `cargo test 2>&1 | tail -20`

- [ ] **Step 6: Commit**

```bash
git add crates/outrider-index/src/scan.rs crates/outrider-index/src/index.rs crates/outrider/src/main.rs
git commit -m "feat: filter binary and build files via settings"
```

---

### Task 3: Folder Select Dialog

Show a native folder picker when no CLI argument is provided.

**Files:**
- Modify: `crates/outrider/src/main.rs`

**Interfaces:**
- Consumes: `rfd` crate (added in Task 1's Cargo.toml)
- Produces: folder selection flow before `index_repo`

- [ ] **Step 1: Replace CLI arg fallback with folder dialog**

In `main()`, replace the current repo resolution:

```rust
fn main() {
    let repo = match std::env::args().nth(1).map(PathBuf::from) {
        Some(p) => p,
        None => match rfd::FileDialog::new()
            .set_title("Open Project Folder")
            .pick_folder()
        {
            Some(p) => p,
            None => return, // user cancelled
        },
    };
    // ... rest unchanged
}
```

- [ ] **Step 2: Build and test**

Run: `cargo build -p outrider 2>&1 | head -20`

- [ ] **Step 3: Commit**

```bash
git add crates/outrider/src/main.rs
git commit -m "feat: show folder picker when no CLI argument provided"
```

---

### Task 4: Welcome Screen Overlay

Show a keybinding reference overlay on first launch. `TreemapView` needs access to `Settings` so it can check `show_welcome` and persist the dismissal.

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (add welcome state, render method, key handler, pass Settings through)
- Modify: `crates/outrider/src/main.rs` (pass settings to TreemapView::new)

**Interfaces:**
- Consumes: `Settings` from Task 1
- Produces: `render_welcome()` method, welcome overlay in render loop

- [ ] **Step 1: Add settings field to TreemapView**

Add `settings: settings::Settings` field to the `TreemapView` struct. Update `TreemapView::new` to accept and store it. Update `main.rs` to pass `settings` (which was loaded in Task 2's changes).

The `new` signature becomes:
```rust
pub fn new(tree: SymbolTree, layout: PackLayout, settings: settings::Settings, cx: &mut Context<Self>) -> Self
```

Add a `show_welcome: bool` field to `TreemapView` initialized from `settings.show_welcome`.

- [ ] **Step 2: Add render_welcome method**

Add a method to `TreemapView` that renders the welcome overlay. Use the same absolute positioning pattern as `render_palette`. Center it, ~600px wide.

Content: a title "Welcome to Outrider", then a two-column grid of key→action rows (same table from the spec), then two buttons at the bottom: "Got it" (dismiss for session) and "Don't show again" (sets setting).

Since GPUI click handlers on divs use `cx.listener`, wire:
- "Got it" → sets `self.show_welcome = false`, `cx.notify()`
- "Don't show again" → sets `self.show_welcome = false`, `self.settings.show_welcome = false`, `self.settings.save()`, `cx.notify()`

- [ ] **Step 3: Wire into render loop and key handler**

In `TreemapView::render()`, after the palette overlay line, add:
```rust
let welcome_overlay = self.show_welcome.then(|| self.render_welcome(vw));
```

Add it as a child of the map div, like palette_overlay.

In the `on_key_down` handler, before the palette check, add:
```rust
if this.show_welcome {
    if e.keystroke.key.as_str() == "escape" {
        this.show_welcome = false;
        cx.notify();
    }
    return; // block all other keys while welcome is open
}
```

- [ ] **Step 4: Build and test**

Run: `cargo build -p outrider 2>&1 | head -20`
Run: `cargo test -p outrider 2>&1 | tail -10`

- [ ] **Step 5: Commit**

```bash
git add crates/outrider/src/treemap.rs crates/outrider/src/main.rs
git commit -m "feat: add welcome screen overlay with keybinding reference"
```

---

### Task 5: Settings Window Overlay

A Ctrl+Comma overlay to edit filter patterns. On save, re-index the repo.

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (settings window state, render, key handler, re-index)

**Interfaces:**
- Consumes: `Settings` field from Task 4
- Produces: settings window overlay toggled by Ctrl+Comma

- [ ] **Step 1: Add settings window state**

Add fields to `TreemapView`:
```rust
settings_open: bool,
settings_ext_text: String,   // newline-separated extension list for editing
settings_folder_text: String, // newline-separated folder list for editing
```

- [ ] **Step 2: Add render_settings_window method**

Render a centered overlay (~600px wide) with:
- Title: "Settings"
- Label "Filtered Extensions:" followed by a text display showing extensions one per line
- Label "Filtered Folders:" followed by a text display showing folders one per line
- Since GPUI doesn't have a native text input widget, present the filters as editable through the palette-style character input: when the settings window is open, typed characters append to the active text field, backspace deletes, Tab switches fields.
- Alternatively, simpler approach: show the current filters as a list, with buttons to "Reset to Defaults" and "Close". The user edits `~/.config/outrider/settings.json` directly for now. This is alpha — a full text-editing settings UI is a lot of GPUI work for limited value.

**Recommended approach for alpha:** Display current filters read-only, with a "Reset to Defaults" button and a "Open Settings File" button that opens the JSON in the system editor. Add a note showing the path.

```rust
fn render_settings_window(&self, map_w: f64) -> gpui::Div {
    // centered 600px overlay showing filter lists (read-only) and action buttons
}
```

- [ ] **Step 3: Wire Ctrl+Comma and key handling**

In the `on_key_down` handler, add Ctrl+Comma detection (alongside Ctrl+P/T):
```rust
"," if e.keystroke.modifiers.control => {
    this.settings_open = !this.settings_open;
    cx.notify();
    return;
}
```

When settings_open, Esc closes it. The "Open Settings File" button opens `settings_path()` with the system default editor. The "Reset to Defaults" button resets and saves settings, then re-indexes.

- [ ] **Step 4: Implement re-index on settings change**

Add a helper method to `TreemapView` that re-runs `index_repo` and rebuilds layout:
```rust
fn reindex(&mut self) {
    let repo = self.tree.repo_root.clone();
    match outrider_index::index_repo(&repo, &self.settings.filter_extensions, &self.settings.filter_folders) {
        Ok(tree) => {
            let layout = outrider_layout::pack(&tree, &world::pack_config());
            self.file_symbols = collect_file_symbols(&tree);
            self.buffers = BufferManager::new(tree.repo_root.clone());
            self.textures = TextureCache::new(rasterize::MAX_BYTES);
            let root_id = tree.root.id.clone();
            self.focus = Focus::new(root_id.clone());
            self.nav_history = vec![root_id];
            self.nav_cursor = 0;
            self.neighbors = None;
            self.hover_id = None;
            self.camera = None;
            self.palette = palette::Palette::new();
            self.tree = tree;
            self.layout = layout;
        }
        Err(e) => eprintln!("reindex failed: {e:#}"),
    }
}
```

- [ ] **Step 5: Build and test**

Run: `cargo build -p outrider 2>&1 | head -20`
Run: `cargo test -p outrider 2>&1 | tail -10`

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: add settings window with Ctrl+Comma"
```

---

### Task 6: Right-Click Context Menu

Show a popup menu on right-click with available commands.

**Files:**
- Modify: `crates/outrider/src/treemap.rs` (context menu state, mouse handler, render, clipboard)

**Interfaces:**
- Consumes: `resolve_fs_path`, `open_in_file_manager` from existing code
- Produces: context menu popup on right-click

- [ ] **Step 1: Add context menu state**

Add fields to `TreemapView`:
```rust
context_menu: Option<ContextMenu>,
```

```rust
struct ContextMenu {
    position: gpui::Point<Pixels>,
    target: SymbolId,
}
```

- [ ] **Step 2: Add right-click mouse handler**

Add `on_mouse_down(MouseButton::Right, ...)` to the map div:
```rust
.on_mouse_down(
    gpui::MouseButton::Right,
    cx.listener(|this, e: &gpui::MouseDownEvent, w, _cx| {
        let Some(cam) = this.camera else { return };
        let (vw, vh) = Self::map_viewport(w);
        let items = world::visible_nodes(&this.tree, &this.layout, &cam, vw, vh);
        let (mx, my) = (f64::from(e.position.x), f64::from(e.position.y) - chrome::TITLEBAR_H);
        if let Some(hit) = world::hit_test(&items, mx, my) {
            this.context_menu = Some(ContextMenu {
                position: e.position,
                target: hit.node.id.clone(),
            });
        }
        cx.notify();
    }),
)
```

- [ ] **Step 3: Add render_context_menu method**

Render a small popup at `context_menu.position` with three clickable rows:
- "Open in File Manager" → calls `open_in_file_manager`
- "Copy Path" → copies relative path to clipboard via `cx.write_to_clipboard()`
- "Copy Name" → copies node name to clipboard

Each row is a div with hover highlight and a click handler. Use `cx.listener` for each.

```rust
fn render_context_menu(&self, cx: &Context<Self>) -> Option<gpui::Div> {
    let menu = self.context_menu.as_ref()?;
    // ... render popup at menu.position
}
```

- [ ] **Step 4: Wire dismiss behavior**

Close the context menu on:
- Left-click anywhere (add to existing `on_mouse_down` left handler)
- Esc key (add to key handler)
- Right-click elsewhere (opens new menu or closes if empty space)

In the left-click handler, at the top:
```rust
if this.context_menu.is_some() {
    this.context_menu = None;
    cx.notify();
}
```

- [ ] **Step 5: Build and test**

Run: `cargo build -p outrider 2>&1 | head -20`
Run: `cargo test -p outrider 2>&1 | tail -10`

- [ ] **Step 6: Commit**

```bash
git add crates/outrider/src/treemap.rs
git commit -m "feat: add right-click context menu with commands"
```

---

### Task 7: README

Write a README.md at the project root.

**Files:**
- Create: `README.md`

**Interfaces:**
- None — standalone documentation

- [ ] **Step 1: Write README.md**

```markdown
# Outrider IDE

A spatial code visualization tool that renders your entire codebase as an interactive treemap. Navigate, search, and understand large codebases at a glance.

## What is Outrider?

Outrider displays your project's source code as a nested treemap where every file and symbol is a visible, navigable box. The size of each box corresponds to its line count, and colors encode structure: folders form the outer containers, files sit inside them, and individual functions, structs, and classes are the innermost leaves — each rendered with syntax-highlighted source code.

Git churn is visualized as a heat stripe on each box, so you can immediately spot the most actively changed parts of your codebase.

## Features

- **Treemap layout** — entire codebase visible at once, zoom in to read code
- **Syntax highlighting** — Rust, Python, C/C++, JavaScript, TypeScript, TSX, C#
- **Fuzzy search** — find files (Ctrl+P) or symbols (Ctrl+T) instantly
- **Git churn visualization** — heat stripes show commit frequency
- **Keyboard navigation** — spatial arrow-key movement through the code map
- **Cross-platform** — Linux, macOS, Windows

## Build

Requires Rust 1.80+.

\```bash
cargo build --release
\```

The binary is at `target/release/outrider`.

## Usage

\```bash
# Open a folder picker
outrider

# Open a specific project
outrider /path/to/project
\```

## Controls

| Key | Action |
|-----|--------|
| Arrow keys | Navigate between nodes |
| Enter | Zoom into selected node |
| Esc | Zoom out to parent |
| Ctrl+P | Search files |
| Ctrl+T | Search symbols |
| Ctrl+, | Open settings |
| Ctrl+Shift+E | Open in file manager |
| Alt+Left/Right | Navigation history |
| Home | Frame entire project |
| Scroll wheel | Zoom in/out |
| Click + drag | Pan |
| Right-click | Context menu |

## Settings

Settings are stored in:
- Linux: `~/.config/outrider/settings.json`
- macOS: `~/Library/Application Support/outrider/settings.json`
- Windows: `%APPDATA%\outrider\settings.json`

You can configure which file extensions and folders are filtered out of the treemap.

## License

MIT
```

Note: escape the triple-backtick fences in the actual file (the backslashes above are just for plan formatting — write real fences).

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add README with project overview and controls"
```
