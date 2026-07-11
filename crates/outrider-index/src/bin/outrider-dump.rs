//! CLI binary: indexes a repo and prints a full indented symbol-tree outline.
//! Usage: `outrider-dump [PATH]` — defaults to the current directory.

use std::path::PathBuf;

/// Entry point: resolve the repo root, index it, and print the dump.
fn main() -> anyhow::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .map_or_else(std::env::current_dir, Ok)?;
    let tree = outrider_index::index_repo(&root)?;
    print!("{}", outrider_index::dump::render(&tree));
    Ok(())
}
