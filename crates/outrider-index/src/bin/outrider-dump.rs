use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .map_or_else(std::env::current_dir, Ok)?;
    let tree = outrider_index::index_repo(&root)?;
    print!("{}", outrider_index::dump::render(&tree));
    Ok(())
}
