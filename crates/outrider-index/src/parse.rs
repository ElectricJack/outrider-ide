use std::ops::Range;

use anyhow::Context;
use tree_sitter::Node;

use crate::types::SymbolKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawItem {
    pub kind: SymbolKind,
    pub name: String,
    pub signature: String,
    pub byte_range: Range<usize>,
    pub line_count: u64,
    pub children: Vec<RawItem>,
}

/// Extract mod/struct/enum/trait/impl/fn items, nested per the syntax tree
/// (spec §5.2). Items are returned in source order.
pub fn parse_rust_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("loading tree-sitter-rust grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    Ok(collect_items(tree.root_node(), source))
}

fn collect_items(node: Node, src: &[u8]) -> Vec<RawItem> {
    let mut items = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(kind) = item_kind(child.kind()) {
            items.push(RawItem {
                kind,
                name: item_name(child, src),
                signature: item_signature(child, src),
                byte_range: child.byte_range(),
                line_count: (child.end_position().row - child.start_position().row + 1) as u64,
                children: collect_items(child, src),
            });
        } else {
            items.extend(collect_items(child, src));
        }
    }
    items
}

fn item_kind(node_kind: &str) -> Option<SymbolKind> {
    match node_kind {
        "mod_item" => Some(SymbolKind::Module),
        "struct_item" => Some(SymbolKind::Struct),
        "enum_item" => Some(SymbolKind::Enum),
        "trait_item" => Some(SymbolKind::Trait),
        "impl_item" => Some(SymbolKind::Impl),
        "function_item" => Some(SymbolKind::Fn),
        _ => None,
    }
}

fn node_text(node: Node, src: &[u8]) -> String {
    String::from_utf8_lossy(&src[node.byte_range()]).into_owned()
}

fn item_name(node: Node, src: &[u8]) -> String {
    if node.kind() == "impl_item" {
        let ty = node
            .child_by_field_name("type")
            .map(|n| node_text(n, src))
            .unwrap_or_default();
        return match node.child_by_field_name("trait") {
            Some(tr) => format!("{} for {}", node_text(tr, src), ty),
            None => ty,
        };
    }
    node.child_by_field_name("name")
        .map(|n| node_text(n, src))
        .unwrap_or_else(|| "<anon>".to_string())
}

/// Declaration text up to (excluding) the body `{` or a terminating `;`,
/// whitespace collapsed to one line.
fn item_signature(node: Node, src: &[u8]) -> String {
    let text = node_text(node, src);
    let end = text.find(['{', ';']).unwrap_or(text.len());
    text[..end].split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Leading `//!` block: skip blank lines, collect consecutive `//!` lines,
/// strip the marker plus one following space. None when there is no block.
pub fn file_doc(source: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(source);
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if lines.is_empty() && t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("//!") {
            lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else {
            break;
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use crate::types::SymbolKind;
    use super::{parse_rust_items};

    const SRC: &str = r#"mod inner {
    pub fn helper() {
        println!("help");
    }
}

struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn new() -> Self {
        Point { x: 0, y: 0 }
    }

    fn norm(&self) -> f64 {
        ((self.x * self.x + self.y * self.y) as f64).sqrt()
    }
}

fn free() {
    let _ = Point::new();
}
"#;

    #[test]
    fn extracts_nested_items_with_names_kinds_measures() {
        let items = parse_rust_items(SRC.as_bytes()).unwrap();
        let summary: Vec<(SymbolKind, &str, usize)> = items
            .iter()
            .map(|i| (i.kind, i.name.as_str(), i.children.len()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (SymbolKind::Module, "inner", 1),
                (SymbolKind::Struct, "Point", 0),
                (SymbolKind::Impl, "Point", 2),
                (SymbolKind::Fn, "free", 0),
            ]
        );

        // nested fn inside mod
        assert_eq!(items[0].children[0].name, "helper");
        assert_eq!(items[0].children[0].kind, SymbolKind::Fn);

        // methods inside impl, in source order at this stage
        let methods: Vec<&str> = items[2].children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(methods, vec!["new", "norm"]);

        // line-count measures: `mod inner { ... }` spans lines 1-5
        assert_eq!(items[0].line_count, 5);
        // `fn free() { ... }` spans 3 lines
        assert_eq!(items[3].line_count, 3);
    }

    #[test]
    fn trait_impl_name_includes_trait() {
        let src = b"trait Show {}\nimpl Show for i32 {}\n";
        let items = parse_rust_items(src).unwrap();
        assert_eq!(items[0].kind, SymbolKind::Trait);
        assert_eq!(items[0].name, "Show");
        assert_eq!(items[1].kind, SymbolKind::Impl);
        assert_eq!(items[1].name, "Show for i32");
    }

    #[test]
    fn signatures_cut_before_body_and_collapse_whitespace() {
        let items = parse_rust_items(SRC.as_bytes()).unwrap();
        assert_eq!(items[0].signature, "mod inner");
        assert_eq!(items[1].signature, "struct Point");
        assert_eq!(items[2].signature, "impl Point");
        assert_eq!(items[2].children[1].signature, "fn norm(&self) -> f64");
        assert_eq!(items[3].signature, "fn free()");
        // multi-line declarations collapse to one line; `;` terminators cut too
        let src = b"fn multi(\n    a: i32,\n    b: i32,\n) -> i32 { a + b }\nstruct Unit;\n";
        let items = parse_rust_items(src).unwrap();
        assert_eq!(items[0].signature, "fn multi( a: i32, b: i32, ) -> i32");
        assert_eq!(items[1].signature, "struct Unit");
    }

    #[test]
    fn file_doc_extracts_leading_bang_comments() {
        use super::file_doc;
        assert_eq!(
            file_doc(b"//! First line.\n//!\n//! Third.\nfn x() {}\n"),
            Some("First line.\n\nThird.".to_string())
        );
        assert_eq!(file_doc(b"\n\n//! After blanks.\nfn x() {}\n"), Some("After blanks.".to_string()));
        assert_eq!(file_doc(b"fn x() {}\n"), None);
        assert_eq!(file_doc(b"// plain comment\n//! not leading\n"), None);
    }
}
