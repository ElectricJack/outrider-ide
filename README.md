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

Requires Rust 1.89+.

```bash
cargo build --release
```

The binary is at `target/release/outrider`.

## Usage

```bash
# Open a folder picker
outrider

# Open a specific project
outrider /path/to/project
```

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

### Cache behavior

- The in-memory texture cache limit is global across projects.
- The disk texture cache limit is configured per project and defaults to 1 GB for each project.
- Texture and Git churn caches live under the operating system's cache directory.
- Texture work prioritizes nodes currently visible in the viewport so useful project content appears sooner.
- Outrider never writes cache files into repositories that it analyzes.

## License

MIT
