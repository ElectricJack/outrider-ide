use std::fmt::Write;

use crate::types::{SymbolKind, SymbolNode, SymbolTree};

pub fn render(tree: &SymbolTree) -> String {
    let mut out = String::new();
    render_node(&tree.root, 0, &mut out);
    out
}

fn kind_label(kind: &SymbolKind) -> String {
    match kind {
        SymbolKind::Folder => "Folder".into(),
        SymbolKind::File => "File".into(),
        SymbolKind::Chunk => "Chunk".into(),
        SymbolKind::Item { label } => label.clone(),
    }
}

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
