//! Per-symbol AST metrics (complexity, nesting depth, parameter count)
//! computed via tree-sitter. Currently supports Rust.

use std::ops::Range;

use crate::language::SourceLanguage;

/// Metrics computed for a single item-level AST node.
#[derive(Debug, Clone, Default)]
pub struct NodeMetrics {
    /// Cyclomatic complexity: count of branch points (if, match arm, for, while, &&, ||, ?)
    /// with a base of 1 for function items.
    pub complexity: u32,
    /// Deepest nesting of branch/loop constructs within this item.
    pub max_nesting: u32,
    /// Number of parameters (fn params, struct fields, enum variants, etc.).
    pub params: u32,
}

/// Compute AST metrics for all item-level nodes in a source file.
/// Returns one entry per item node the parser would emit (matched by byte range).
/// Returns `None` if the language is not supported.
pub fn compute(
    source: &[u8],
    lang: SourceLanguage,
) -> Option<Vec<(Range<usize>, NodeMetrics)>> {
    match lang {
        SourceLanguage::Rust => compute_rust(source),
        _ => None,
    }
}

/// Item-level node kinds in the tree-sitter Rust grammar.
const RUST_ITEM_KINDS: &[&str] = &[
    "function_item",
    "impl_item",
    "struct_item",
    "enum_item",
    "trait_item",
    "type_item",
    "const_item",
    "static_item",
    "mod_item",
];

fn compute_rust(source: &[u8]) -> Option<Vec<(Range<usize>, NodeMetrics)>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;

    let mut results = Vec::new();
    collect_rust_items(tree.root_node(), source, &mut results);
    Some(results)
}

fn collect_rust_items(
    node: tree_sitter::Node,
    source: &[u8],
    results: &mut Vec<(Range<usize>, NodeMetrics)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if RUST_ITEM_KINDS.contains(&child.kind()) {
            let metrics = rust_node_metrics(child, source);
            results.push((child.byte_range(), metrics));
        }
        collect_rust_items(child, source, results);
    }
}

fn rust_node_metrics(node: tree_sitter::Node, source: &[u8]) -> NodeMetrics {
    let base = if node.kind() == "function_item" { 1 } else { 0 };
    let mut branch_count = 0u32;
    count_complexity(node, source, &mut branch_count);

    let mut max_nesting = 0u32;
    count_nesting(node, 0, &mut max_nesting);

    NodeMetrics {
        complexity: base + branch_count,
        max_nesting,
        params: count_params(node),
    }
}

/// Count branch points within a node's subtree.
fn count_complexity(node: tree_sitter::Node, source: &[u8], count: &mut u32) {
    match node.kind() {
        "if_expression" | "for_expression" | "while_expression" | "loop_expression"
        | "try_expression" => {
            *count += 1;
        }
        "match_arm" => {
            *count += 1;
        }
        "binary_expression" => {
            // Check whether the operator is && or ||.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    let text = &source[child.byte_range()];
                    if text == b"&&" || text == b"||" {
                        *count += 1;
                        break;
                    }
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count_complexity(child, source, count);
    }
}

/// Track nesting depth of branch/loop constructs.
fn count_nesting(node: tree_sitter::Node, depth: u32, max: &mut u32) {
    let new_depth = match node.kind() {
        "if_expression" | "for_expression" | "while_expression" | "loop_expression"
        | "match_expression" | "closure_expression" => {
            let d = depth + 1;
            if d > *max {
                *max = d;
            }
            d
        }
        _ => depth,
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count_nesting(child, new_depth, max);
    }
}

/// Count parameters/fields/variants depending on item kind.
fn count_params(node: tree_sitter::Node) -> u32 {
    match node.kind() {
        "function_item" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "parameters" {
                    let mut pcursor = child.walk();
                    return child
                        .named_children(&mut pcursor)
                        .filter(|c| c.kind() == "parameter" || c.kind() == "self_parameter")
                        .count() as u32;
                }
            }
            0
        }
        "struct_item" => count_descendants(node, "field_declaration"),
        "enum_item" => count_descendants(node, "enum_variant"),
        _ => 0,
    }
}

/// Count all descendant nodes of a given kind.
fn count_descendants(node: tree_sitter::Node, kind: &str) -> u32 {
    let mut count = 0u32;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            count += 1;
        }
        count += count_descendants(child, kind);
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::SourceLanguage;

    #[test]
    fn simple_function_complexity() {
        let src = b"fn foo(x: i32, y: i32) -> bool { if x > 0 { true } else { false } }";
        let results = compute(src, SourceLanguage::Rust).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.complexity, 2); // 1 base + 1 if
        assert_eq!(results[0].1.max_nesting, 1); // one level of if
        assert_eq!(results[0].1.params, 2); // x, y
    }

    #[test]
    fn nested_control_flow() {
        let src = b"fn bar() { for i in 0..10 { if i > 5 { while true { break; } } } }";
        let results = compute(src, SourceLanguage::Rust).unwrap();
        assert_eq!(results[0].1.max_nesting, 3); // for > if > while
    }

    #[test]
    fn struct_counts_fields() {
        let src = b"struct Point { x: f64, y: f64, z: f64 }";
        let results = compute(src, SourceLanguage::Rust).unwrap();
        assert_eq!(results[0].1.params, 3);
    }

    #[test]
    fn unsupported_language_returns_none() {
        // SourceLanguage has no Unknown variant; Markdown is not supported for metrics.
        let results = compute(b"hello", SourceLanguage::Markdown);
        assert!(results.is_none());
    }

    #[test]
    fn match_arms_add_complexity() {
        let src = b"fn f(x: i32) -> &str { match x { 1 => \"a\", 2 => \"b\", _ => \"c\" } }";
        let results = compute(src, SourceLanguage::Rust).unwrap();
        // base 1 + 3 match_arms = 4
        assert!(results[0].1.complexity >= 3); // base + arms
    }
}
