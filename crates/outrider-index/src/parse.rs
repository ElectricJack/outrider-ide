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
    let kind_fn = |node_kind: &str, _node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "mod_item" => Some("module"),
            "struct_item" => Some("struct"),
            "enum_item" => Some("enum"),
            "trait_item" => Some("trait"),
            "impl_item" => Some("impl"),
            "function_item" => Some("fn"),
            _ => None,
        }
    };
    Ok(collect_items(tree.root_node(), source, &kind_fn, &rust_item_name))
}

/// Extract class/fn items from Python source, including decorated definitions.
pub fn parse_python_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .context("loading tree-sitter-python grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "function_definition" => {
                // Skip inner definition when it is directly owned by a decorated_definition;
                // the decorated_definition node itself will be the top-level item.
                if node.parent().is_some_and(|p| p.kind() == "decorated_definition") {
                    None
                } else {
                    Some("fn")
                }
            }
            "class_definition" => {
                if node.parent().is_some_and(|p| p.kind() == "decorated_definition") {
                    None
                } else {
                    Some("class")
                }
            }
            "decorated_definition" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    match child.kind() {
                        "function_definition" => return Some("fn"),
                        "class_definition" => return Some("class"),
                        _ => {}
                    }
                }
                None
            }
            _ => None,
        }
    };
    Ok(collect_items(tree.root_node(), source, &kind_fn, &python_item_name))
}

/// Python-specific name extraction: unwraps decorated_definition to find the inner name.
fn python_item_name(node: Node, src: &[u8]) -> String {
    if node.kind() == "decorated_definition" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "function_definition" || child.kind() == "class_definition" {
                return item_name_default(child, src);
            }
        }
    }
    item_name_default(node, src)
}

/// Extract struct/enum/typedef/fn items from C source.
pub fn parse_c_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_c::LANGUAGE.into())
        .context("loading tree-sitter-c grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "function_definition" => Some("fn"),
            "struct_specifier" if node.child_by_field_name("body").is_some() => Some("struct"),
            "enum_specifier" if node.child_by_field_name("body").is_some() => Some("enum"),
            "type_definition" => Some("typedef"),
            _ => None,
        }
    };
    Ok(collect_items(tree.root_node(), source, &kind_fn, &c_item_name))
}

/// C-specific name extraction.
/// - `struct_specifier` / `enum_specifier`: `name` field.
/// - `type_definition`: the last named child before `;` is the type alias
///   (`type_identifier`).
/// - `function_definition`: the `declarator` field is a `function_declarator`
///   whose own `declarator` field is the identifier.
fn c_item_name(node: Node, src: &[u8]) -> String {
    match node.kind() {
        "struct_specifier" | "enum_specifier" => {
            node.child_by_field_name("name")
                .map(|n| node_text(n, src))
                .unwrap_or_else(|| "<anon>".to_string())
        }
        "type_definition" => {
            // The alias identifier is the last type_identifier child.
            let mut cursor = node.walk();
            let mut last_ident = None;
            for child in node.named_children(&mut cursor) {
                if child.kind() == "type_identifier" {
                    last_ident = Some(node_text(child, src));
                }
            }
            last_ident.unwrap_or_else(|| "<anon>".to_string())
        }
        "function_definition" => {
            // declarator field → function_declarator → declarator field (the name).
            let declarator = node.child_by_field_name("declarator");
            if let Some(decl) = declarator {
                // function_declarator has a `declarator` field that is the identifier.
                if let Some(inner) = decl.child_by_field_name("declarator") {
                    return node_text(inner, src);
                }
                // fallback: first named child
                if let Some(first) = decl.named_child(0) {
                    return node_text(first, src);
                }
            }
            "<anon>".to_string()
        }
        _ => item_name_default(node, src),
    }
}

pub fn js_kind_fn(node_kind: &str, node: Node, _src: &[u8]) -> Option<&'static str> {
    match node_kind {
        "function_declaration" | "generator_function_declaration" => Some("fn"),
        "class_declaration" => Some("class"),
        "method_definition" => Some("fn"),
        "lexical_declaration" | "variable_declaration" => {
            // Named arrow functions: const add = (a, b) => ...
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "variable_declarator" {
                    if let Some(value) = child.child_by_field_name("value") {
                        if value.kind() == "arrow_function" || value.kind() == "function" {
                            return Some("fn");
                        }
                    }
                }
            }
            None
        }
        "export_statement" => None, // recurse into inner declaration
        _ => None,
    }
}

pub fn js_item_name(node: Node, src: &[u8]) -> String {
    if node.kind() == "lexical_declaration" || node.kind() == "variable_declaration" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "variable_declarator" {
                return child
                    .child_by_field_name("name")
                    .map(|n| node_text(n, src))
                    .unwrap_or_else(|| "<anon>".to_string());
            }
        }
    }
    item_name_default(node, src)
}

fn ts_kind_fn(node_kind: &str, node: Node, src: &[u8]) -> Option<&'static str> {
    match node_kind {
        "interface_declaration" => Some("interface"),
        "enum_declaration" => Some("enum"),
        "type_alias_declaration" => Some("type"),
        _ => js_kind_fn(node_kind, node, src),
    }
}

pub fn parse_ts_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .context("loading tree-sitter-typescript grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    Ok(collect_items(tree.root_node(), source, &ts_kind_fn, &js_item_name))
}

pub fn parse_tsx_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
        .context("loading tree-sitter-tsx grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    Ok(collect_items(tree.root_node(), source, &ts_kind_fn, &js_item_name))
}

pub fn parse_js_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .context("loading tree-sitter-javascript grammar")?;
    let tree = parser.parse(source, None).context("tree-sitter parse failed")?;
    Ok(collect_items(tree.root_node(), source, &js_kind_fn, &js_item_name))
}

fn collect_items(
    node: Node,
    src: &[u8],
    kind_fn: &dyn Fn(&str, Node, &[u8]) -> Option<&'static str>,
    name_fn: &dyn Fn(Node, &[u8]) -> String,
) -> Vec<RawItem> {
    let mut items = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(label) = kind_fn(child.kind(), child, src) {
            items.push(RawItem {
                kind: SymbolKind::Item { label: label.into() },
                name: name_fn(child, src),
                signature: item_signature(child, src),
                byte_range: child.byte_range(),
                line_count: (child.end_position().row - child.start_position().row + 1) as u64,
                children: collect_items(child, src, kind_fn, name_fn),
            });
        } else {
            items.extend(collect_items(child, src, kind_fn, name_fn));
        }
    }
    items
}

fn node_text(node: Node, src: &[u8]) -> String {
    String::from_utf8_lossy(&src[node.byte_range()]).into_owned()
}

/// Default name extraction: uses the `name` field of the node.
fn item_name_default(node: Node, src: &[u8]) -> String {
    node.child_by_field_name("name")
        .map(|n| node_text(n, src))
        .unwrap_or_else(|| "<anon>".to_string())
}

/// Rust-specific name extraction: handles `impl` blocks specially.
fn rust_item_name(node: Node, src: &[u8]) -> String {
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
    item_name_default(node, src)
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
    use super::{parse_rust_items, parse_c_items, parse_python_items, parse_js_items, parse_ts_items};

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
            .map(|i| (i.kind.clone(), i.name.as_str(), i.children.len()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (SymbolKind::Item { label: "module".into() }, "inner", 1),
                (SymbolKind::Item { label: "struct".into() }, "Point", 0),
                (SymbolKind::Item { label: "impl".into() }, "Point", 2),
                (SymbolKind::Item { label: "fn".into() }, "free", 0),
            ]
        );

        // nested fn inside mod
        assert_eq!(items[0].children[0].name, "helper");
        assert_eq!(items[0].children[0].kind, SymbolKind::Item { label: "fn".into() });

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
        assert_eq!(items[0].kind, SymbolKind::Item { label: "trait".into() });
        assert_eq!(items[0].name, "Show");
        assert_eq!(items[1].kind, SymbolKind::Item { label: "impl".into() });
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

    #[test]
    fn extracts_python_items() {
        let src = br#"
class Animal:
    def speak(self):
        pass

    def eat(self):
        pass

def standalone():
    pass

@staticmethod
def decorated():
    pass
"#;
        let items = parse_python_items(src).unwrap();
        let summary: Vec<(&str, &str, usize)> = items
            .iter()
            .map(|i| (i.kind.label(), i.name.as_str(), i.children.len()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("class", "Animal", 2),
                ("fn", "standalone", 0),
                ("fn", "decorated", 0),
            ]
        );
        assert_eq!(items[0].children[0].name, "speak");
        assert_eq!(items[0].children[1].name, "eat");
    }

    #[test]
    fn extracts_js_items() {
        let src = br#"
function greet(name) {
    return "hello " + name;
}

class Greeter {
    constructor(name) {
        this.name = name;
    }
    greet() {
        return "hello " + this.name;
    }
}

const add = (a, b) => a + b;

export function exported() {}
"#;
        let items = parse_js_items(src).unwrap();
        let summary: Vec<(&str, &str, usize)> = items
            .iter()
            .map(|i| (i.kind.label(), i.name.as_str(), i.children.len()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("fn", "greet", 0),
                ("class", "Greeter", 2),
                ("fn", "add", 0),
                ("fn", "exported", 0),
            ]
        );
    }

    #[test]
    fn extracts_ts_items() {
        let src = br#"
function greet(name: string): string {
    return "hello " + name;
}

class Greeter {
    greet(): string {
        return "hello";
    }
}

interface Printable {
    print(): void;
}

enum Direction {
    Up,
    Down,
}

type UserId = string;

const add = (a: number, b: number): number => a + b;
"#;
        let items = parse_ts_items(src).unwrap();
        let summary: Vec<(&str, &str)> = items
            .iter()
            .map(|i| (i.kind.label(), i.name.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("fn", "greet"),
                ("class", "Greeter"),
                ("interface", "Printable"),
                ("enum", "Direction"),
                ("type", "UserId"),
                ("fn", "add"),
            ]
        );
    }

    #[test]
    fn extracts_c_items() {
        let src = br#"
struct Point {
    int x;
    int y;
};

enum Color { RED, GREEN, BLUE };

typedef unsigned long ulong;

void draw(struct Point p) {
    // body
}
"#;
        let items = parse_c_items(src).unwrap();
        let summary: Vec<(&str, &str)> = items
            .iter()
            .map(|i| (i.kind.label(), i.name.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("struct", "Point"),
                ("enum", "Color"),
                ("typedef", "ulong"),
                ("fn", "draw"),
            ]
        );
    }
}

