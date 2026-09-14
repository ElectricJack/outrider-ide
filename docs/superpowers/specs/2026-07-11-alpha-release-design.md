# Alpha Release Readiness Design

**Goal:** Prepare outrider-ide for initial alpha distribution — add settings persistence, file filtering, folder selection, welcome screen, right-click menu, and a README.

## 1. Settings Persistence

A `Settings` struct serialized as JSON to the user config directory.

**Location:** `dirs::config_dir() / "outrider" / "settings.json"`
- Linux: `~/.config/outrider/settings.json`
- Windows: `%APPDATA%\outrider\settings.json`
- macOS: `~/Library/Application Support/outrider/settings.json`

**Schema:**
```rust
pub struct Settings {
    pub filter_extensions: Vec<String>,  // e.g. ["exe", "dll", "png"]
    pub filter_folders: Vec<String>,     // e.g. ["target", "node_modules"]
    pub show_welcome: bool,             // default true, set false when dismissed
}
```

**Behavior:**
- Loaded at startup. If missing or malformed, use defaults silently.
- Saved on mutation (welcome dismiss, settings window save).
- Creates parent dirs on first save.

**New crate dependency:** `dirs` for cross-platform config path.

## 2. File Filtering

Apply settings-based filter patterns during the filesystem scan in `scan.rs`.

**Default exclusions — extensions:**
`exe, dll, obj, o, so, dylib, a, lib, pdb, class, pyc, wasm, bin, dat, db, sqlite, png, jpg, jpeg, gif, ico, bmp, svg, mp3, mp4, wav, zip, tar, gz, 7z, rar, pdf, ttf, otf, woff, woff2`

**Default exclusions — folders:**
`target, node_modules, dist, build, __pycache__, .next, .nuxt, out, pkg, vendor`

**Implementation:** In `scan_files()`, after the `ignore` crate walker filters, additionally skip files whose extension matches `filter_extensions` and directories whose name matches `filter_folders`. The `Settings` struct is passed into `scan_files()` (and by extension `index_repo()`).

## 3. Folder Select Dialog

Replace the current "use CLI arg or cwd" launch with a folder picker when no path is given.

**Dependency:** `rfd` crate (Rust File Dialog — native cross-platform).

**Flow:**
1. If CLI arg provided, use it (existing behavior).
2. If no arg, call `rfd::FileDialog::new().pick_folder()`.
3. If user cancels, exit with code 0 (no error).
4. Proceed with selected path as `repo_root`.

All of this happens synchronously in `main()` before `index_repo`.

## 4. Welcome Screen

A GPUI overlay shown on first launch, listing all keybindings.

**Content — keybinding reference grid:**

| Key | Action |
|-----|--------|
| Arrow keys | Navigate between nodes |
| Enter | Zoom into selected node |
| Esc | Zoom out to parent |
| Ctrl+P | Search files |
| Ctrl+T | Search symbols |
| Ctrl+Shift+E | Open in file manager |
| Alt+Left/Right | Navigation history |
| Home | Frame entire project |
| Scroll wheel | Zoom in/out |
| Click + drag | Pan |
| Right-click | Context menu |

**Behavior:**
- Shown as a centered overlay on startup when `settings.show_welcome` is true.
- Dismissed by pressing Esc or clicking a "Don't show again" button.
- "Don't show again" sets `show_welcome = false` in settings and saves.
- Pressing Esc just dismisses for this session without changing the setting.

**Rendering:** Similar to the palette — an absolutely positioned div, centered, with the same background/border styling.

## 5. Settings Window

A GPUI overlay toggled with Ctrl+Comma.

**Content:**
- Two text areas: one for filtered extensions (one per line), one for filtered folders (one per line).
- A "Save" button and a "Cancel" button.
- A "Reset to defaults" link.

**Behavior:**
- Toggled open/closed with Ctrl+Comma. Esc also closes (cancels).
- On save: parse the text areas back into `Vec<String>`, update `Settings`, write to disk, and re-index the repo with the new filters.
- Re-indexing: call `index_repo` again with the new settings, rebuild the layout and `TreemapView` state. This is the simplest correct approach — the scan is fast enough for alpha.

## 6. Right-Click Context Menu

A popup menu shown on right-click over any node.

**Commands:**
- **Open in File Manager** — existing `open_in_file_manager` logic
- **Copy Path** — copy the file's relative path to clipboard
- **Copy Name** — copy the node's display name to clipboard

**Behavior:**
- Appears at mouse cursor position on right-click.
- Each item is a clickable div row.
- Dismissed on click-outside, Esc, or selecting an item.
- Only shown when right-clicking on a leaf or file node (not empty space).

**State:** A `ContextMenu` struct with `open: bool`, `position: Point`, `target: SymbolId`.

## 7. README

A `README.md` at the project root.

**Content:**
- Project name and one-line description
- What outrider-ide is — a spatial code visualization tool that renders your codebase as an interactive treemap
- Key features: treemap layout, syntax highlighting, fuzzy search, git churn visualization, keyboard navigation
- Build instructions (`cargo build --release`)
- Controls reference (same table as welcome screen)
- License placeholder
