//! Debug renderer for a `SymbolTree`: produces an indented text outline of the
//! full tree with kind, name, line count, and churn for each node.
//! Used by the `outrider-dump` CLI binary.

use std::fmt::Write;

use crate::types::{SymbolKind, SymbolNode, SymbolTree};

/// Renders the entire `SymbolTree` as an indented text outline.
pub fn render(tree: &SymbolTree) -> String {
    let mut out = String::new();
    render_node(&tree.root, 0, &mut out);
    out
}

/// Maps a `SymbolKind` to its capitalized display label for the dump output.
fn kind_label(kind: &SymbolKind) -> String {
    match kind {
        SymbolKind::Folder => "Folder".into(),
        SymbolKind::File => "File".into(),
        SymbolKind::Chunk => "Chunk".into(),
        SymbolKind::Item { label } => label.clone(),
    }
}

/// Appends one line per node (indented by `depth`) and recurses into children.
fn render_node(node: &SymbolNode, depth: usize, out: &mut String) {
    writeln!(
        out,
        "{:indent$}{} {} [{} lines, churn {} · p{:.0}]",
        "",
        kind_label(&node.id.kind),
        node.name,
        node.measure,
        node.churn_count,
        node.churn * 100.0,
        indent = depth * 2
    )
    .expect("string write");
    for child in &node.children {
        render_node(child, depth + 1, out);
    }
}
