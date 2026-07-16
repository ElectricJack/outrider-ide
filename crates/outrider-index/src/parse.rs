//! Tree-sitter-based source parsers for Rust, Python, C, C++, JS/TS/TSX, and C#.
//! Each `parse_*_items` function returns a nested `RawItem` tree that mirrors
//! the language's structural hierarchy. `file_doc` extracts `//!` module docs;
//! each item carries the `///` block found directly above it.

use std::ops::Range;

use anyhow::Context;
use tree_sitter::Node;

use crate::types::SymbolKind;

/// Raw output of a single tree-sitter parse pass: one structural item with its
/// metadata and nested children, before `SymbolId` assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawItem {
    pub kind: SymbolKind,
    pub name: String,
    pub signature: String,
    /// `///` block directly above the item, marker stripped; None without one.
    pub doc: Option<String>,
    pub byte_range: Range<usize>,
    pub line_count: u64,
    pub children: Vec<RawItem>,
}

/// Extract Make rules while retaining all non-rule bytes as adjacent sections.
pub fn parse_make_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    if source.is_empty() {
        return Ok(Vec::new());
    }

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_make::LANGUAGE.into())
        .context("loading tree-sitter-make grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse failed")?;

    let mut rules = Vec::new();
    collect_make_rules(tree.root_node(), &mut rules);
    rules.sort_by_key(|node| (node.start_byte(), node.end_byte()));
    let mut end = 0;
    rules.retain(|node| {
        if node.start_byte() < end {
            false
        } else {
            end = node.end_byte();
            true
        }
    });

    let mut items = Vec::with_capacity(rules.len() * 2 + 1);
    let mut cursor = 0;
    for rule in rules {
        if cursor < rule.start_byte() {
            items.push(make_section(
                cursor..rule.start_byte(),
                tree.root_node(),
                source,
                cursor == 0,
            ));
        }
        items.push(make_target(rule, source));
        cursor = rule.end_byte();
    }
    if cursor < source.len() {
        items.push(make_section(
            cursor..source.len(),
            tree.root_node(),
            source,
            cursor == 0,
        ));
    }
    Ok(items)
}

fn collect_make_rules<'tree>(node: Node<'tree>, rules: &mut Vec<Node<'tree>>) {
    if node.kind() == "rule" {
        rules.push(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_make_rules(child, rules);
    }
}

fn make_target(node: Node<'_>, source: &[u8]) -> RawItem {
    let target = node
        .child_by_field_name("targets")
        .or_else(|| {
            let mut cursor = node.walk();
            let target = node
                .named_children(&mut cursor)
                .find(|child| child.kind() == "targets");
            target
        })
        .or_else(|| node.child_by_field_name("target"))
        .map(|target| node_text(target, source).trim().to_owned())
        .unwrap_or_default();
    let recipe_start = {
        let mut cursor = node.walk();
        let start = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "recipe")
            .map(|recipe| recipe.start_byte());
        start
    };
    let header_end = recipe_start.unwrap_or_else(|| node.end_byte());
    let signature = String::from_utf8_lossy(&source[node.start_byte()..header_end])
        .trim()
        .trim_end_matches(';')
        .trim_end()
        .to_owned();
    let range = node.byte_range();
    let line_count = source[range.clone()]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
        + usize::from(!source[range.clone()].ends_with(b"\n"));
    RawItem {
        kind: SymbolKind::Item {
            label: "target".into(),
        },
        name: target,
        signature,
        doc: None,
        byte_range: range,
        line_count: line_count as u64,
        children: Vec::new(),
    }
}

fn make_section(range: Range<usize>, root: Node<'_>, source: &[u8], is_preamble: bool) -> RawItem {
    let priorities = [
        (
            "Definitions",
            &["define_directive", "undefine_directive"][..],
        ),
        ("Conditionals", &["conditional"][..]),
        ("Includes", &["include_directive"][..]),
        (
            "Variables",
            &[
                "variable_assignment",
                "shell_assignment",
                "RECIPEPREFIX_assignment",
                "VPATH_assignment",
            ][..],
        ),
    ];
    let name = priorities
        .iter()
        .find(|(_, kinds)| make_range_contains_kind(root, &range, kinds))
        .map(|(label, _)| *label)
        .unwrap_or(if is_preamble { "Preamble" } else { "Section" });
    let line_count = source[range.clone()]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
        + usize::from(!source[range.clone()].ends_with(b"\n"));
    RawItem {
        kind: SymbolKind::Item {
            label: "section".into(),
        },
        name: name.to_owned(),
        signature: String::new(),
        doc: None,
        byte_range: range,
        line_count: line_count as u64,
        children: Vec::new(),
    }
}

fn make_range_contains_kind(node: Node<'_>, range: &Range<usize>, kinds: &[&str]) -> bool {
    if node.end_byte() <= range.start || node.start_byte() >= range.end {
        return false;
    }
    if kinds.contains(&node.kind()) {
        return true;
    }
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|child| make_range_contains_kind(child, range, kinds));
    found
}

type KindClassifier = dyn for<'tree> Fn(&str, Node<'tree>, &[u8]) -> Option<&'static str>;
type NameExtractor = dyn for<'tree> Fn(Node<'tree>, &[u8]) -> String;
type DocExtractor = dyn for<'tree> Fn(Node<'tree>, &[u8]) -> Option<String>;

/// Extract mod/struct/enum/trait/impl/fn items, nested per the syntax tree
/// (spec §5.2). Items are returned in source order.
pub fn parse_rust_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("loading tree-sitter-rust grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse failed")?;
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
    Ok(collect_items(
        tree.root_node(),
        source,
        &kind_fn,
        &rust_item_name,
    ))
}

/// Extract class/fn items from Python source, including decorated definitions.
pub fn parse_python_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .context("loading tree-sitter-python grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "function_definition" => {
                // Skip inner definition when it is directly owned by a decorated_definition;
                // the decorated_definition node itself will be the top-level item.
                if node
                    .parent()
                    .is_some_and(|p| p.kind() == "decorated_definition")
                {
                    None
                } else {
                    Some("fn")
                }
            }
            "class_definition" => {
                if node
                    .parent()
                    .is_some_and(|p| p.kind() == "decorated_definition")
                {
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
    Ok(collect_items_with_doc(
        tree.root_node(),
        source,
        &kind_fn,
        &python_item_name,
        Some(&python_docstring),
    ))
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

fn python_docstring(node: Node, src: &[u8]) -> Option<String> {
    let inner;
    let def = if node.kind() == "decorated_definition" {
        let mut cursor = node.walk();
        inner = node
            .children(&mut cursor)
            .find(|c| c.kind() == "function_definition" || c.kind() == "class_definition")?;
        inner
    } else {
        node
    };
    let body = def.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let first = body.named_children(&mut cursor).next()?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let string_node = first.named_child(0)?;
    if string_node.kind() != "string" {
        return None;
    }
    let raw = node_text(string_node, src);
    clean_docstring(&raw)
}

fn clean_docstring(raw: &str) -> Option<String> {
    let body = raw
        .strip_prefix("\"\"\"")
        .and_then(|s| s.strip_suffix("\"\"\""))
        .or_else(|| raw.strip_prefix("'''").and_then(|s| s.strip_suffix("'''")))?;
    let lines: Vec<&str> = body.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let first = lines[0].trim();
    let rest = &lines[1..];
    let min_indent = rest
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    out.push(first);
    for line in rest {
        if line.trim().is_empty() {
            out.push("");
        } else {
            out.push(&line[min_indent..]);
        }
    }
    while out.last() == Some(&"") {
        out.pop();
    }
    while out.first() == Some(&"") {
        out.remove(0);
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("\n"))
    }
}

pub fn python_file_doc(source: &[u8]) -> Option<String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    let root = tree.root_node();
    let mut cursor = root.walk();
    let first = root.named_children(&mut cursor).next()?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let string_node = first.named_child(0)?;
    if string_node.kind() != "string" {
        return None;
    }
    let raw = node_text(string_node, source);
    clean_docstring(&raw)
}

/// Extract struct/enum/typedef/fn items from C source.
pub fn parse_c_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_c::LANGUAGE.into())
        .context("loading tree-sitter-c grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "function_definition" => Some("fn"),
            "struct_specifier" if node.child_by_field_name("body").is_some() => Some("struct"),
            "enum_specifier" if node.child_by_field_name("body").is_some() => Some("enum"),
            "type_definition" => Some("typedef"),
            _ => None,
        }
    };
    Ok(collect_items(
        tree.root_node(),
        source,
        &kind_fn,
        &c_item_name,
    ))
}

/// C-specific name extraction.
/// - `struct_specifier` / `enum_specifier`: `name` field.
/// - `type_definition`: the last named child before `;` is the type alias
///   (`type_identifier`).
/// - `function_definition`: the `declarator` field is a `function_declarator`
///   whose own `declarator` field is the identifier.
fn c_item_name(node: Node, src: &[u8]) -> String {
    match node.kind() {
        "struct_specifier" | "enum_specifier" => node
            .child_by_field_name("name")
            .map(|n| node_text(n, src))
            .unwrap_or_else(|| "<anon>".to_string()),
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

/// Extract class/struct/enum/namespace/fn items from C++ source.
pub fn parse_cpp_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .context("loading tree-sitter-cpp grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "function_definition" => Some("fn"),
            "class_specifier" if node.child_by_field_name("body").is_some() => Some("class"),
            "struct_specifier" if node.child_by_field_name("body").is_some() => Some("struct"),
            "enum_specifier" if node.child_by_field_name("body").is_some() => Some("enum"),
            "namespace_definition" => Some("namespace"),
            "type_definition" => Some("typedef"),
            "template_declaration" => None,
            _ => None,
        }
    };
    Ok(collect_items(
        tree.root_node(),
        source,
        &kind_fn,
        &cpp_item_name,
    ))
}

/// C++-specific name extraction.
/// Extends the C name logic with support for qualified identifiers
/// (`Foo::bar`), destructors (`~Foo`), and operator overloads.
fn cpp_item_name(node: Node, src: &[u8]) -> String {
    match node.kind() {
        "class_specifier" | "struct_specifier" | "enum_specifier" => node
            .child_by_field_name("name")
            .map(|n| node_text(n, src))
            .unwrap_or_else(|| "<anon>".to_string()),
        "namespace_definition" => node
            .child_by_field_name("name")
            .map(|n| node_text(n, src))
            .unwrap_or_else(|| "<anon>".to_string()),
        "type_definition" => {
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
            let declarator = node.child_by_field_name("declarator");
            if let Some(decl) = declarator {
                return cpp_declarator_name(decl, src);
            }
            "<anon>".to_string()
        }
        _ => item_name_default(node, src),
    }
}

/// Walks through nested C++ declarators to extract the function/method name,
/// handling `function_declarator`, `qualified_identifier`, `destructor_name`,
/// and `operator_name`.
fn cpp_declarator_name(node: Node, src: &[u8]) -> String {
    match node.kind() {
        "function_declarator" => {
            if let Some(inner) = node.child_by_field_name("declarator") {
                return cpp_declarator_name(inner, src);
            }
            node.named_child(0)
                .map(|n| node_text(n, src))
                .unwrap_or_else(|| "<anon>".to_string())
        }
        "qualified_identifier" => node_text(node, src),
        "destructor_name" | "operator_name" => node_text(node, src),
        "reference_declarator" | "pointer_declarator" => {
            if let Some(inner) = node.named_child(0) {
                return cpp_declarator_name(inner, src);
            }
            node_text(node, src)
        }
        _ => node_text(node, src),
    }
}

/// Classifies a JS/JSX syntax node into a symbol kind label, handling arrow functions.
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

/// Extracts the display name for a JS item, unwrapping `const foo = ...` declarators.
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

/// Extends `js_kind_fn` with TypeScript-only node kinds (interface, enum, type alias).
fn ts_kind_fn(node_kind: &str, node: Node, src: &[u8]) -> Option<&'static str> {
    match node_kind {
        "interface_declaration" => Some("interface"),
        "enum_declaration" => Some("enum"),
        "type_alias_declaration" => Some("type"),
        _ => js_kind_fn(node_kind, node, src),
    }
}

/// Extract TypeScript items using the TS grammar (class/fn/interface/enum/type).
pub fn parse_ts_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .context("loading tree-sitter-typescript grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse failed")?;
    Ok(collect_items(
        tree.root_node(),
        source,
        &ts_kind_fn,
        &js_item_name,
    ))
}

/// Extract TypeScript/JSX items using the TSX grammar for `.tsx` files.
pub fn parse_tsx_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
        .context("loading tree-sitter-tsx grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse failed")?;
    Ok(collect_items(
        tree.root_node(),
        source,
        &ts_kind_fn,
        &js_item_name,
    ))
}

/// Extract JavaScript items (function/class/arrow-fn declarations).
pub fn parse_js_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .context("loading tree-sitter-javascript grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse failed")?;
    Ok(collect_items(
        tree.root_node(),
        source,
        &js_kind_fn,
        &js_item_name,
    ))
}

/// Extract C# items (namespace/class/record/interface/struct/enum/method).
pub fn parse_csharp_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .context("loading tree-sitter-c-sharp grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse failed")?;
    let kind_fn = |node_kind: &str, _node: Node, _src: &[u8]| -> Option<&'static str> {
        match node_kind {
            "class_declaration" | "record_declaration" => Some("class"),
            "interface_declaration" => Some("interface"),
            "struct_declaration" => Some("struct"),
            "enum_declaration" => Some("enum"),
            "method_declaration" | "constructor_declaration" => Some("fn"),
            "namespace_declaration" => Some("namespace"),
            _ => None,
        }
    };
    Ok(collect_items(
        tree.root_node(),
        source,
        &kind_fn,
        &item_name_default,
    ))
}

/// Extract functions, structs, and named interface blocks from GLSL source.
pub fn parse_glsl_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_glsl::LANGUAGE_GLSL.into())
        .context("loading tree-sitter-glsl grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter GLSL parse failed")?;
    Ok(collect_shader_items(
        tree.root_node(),
        source,
        ShaderLanguage::Glsl,
    ))
}

/// Extract functions, structs, and constant buffers from HLSL source.
pub fn parse_hlsl_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_hlsl::LANGUAGE_HLSL.into())
        .context("loading tree-sitter-hlsl grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter HLSL parse failed")?;
    let mut items = collect_shader_items(tree.root_node(), source, ShaderLanguage::Hlsl);
    items.extend(scan_hlsl_cbuffers(source));
    items.sort_by_key(|item| item.byte_range.start);
    Ok(items)
}

#[derive(Clone, Copy)]
enum ShaderLanguage {
    Glsl,
    Hlsl,
}

fn collect_shader_items(node: Node, src: &[u8], language: ShaderLanguage) -> Vec<RawItem> {
    let mut items = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = node_text(child, src);
        let label = match child.kind() {
            "function_definition" => Some("fn"),
            "struct_specifier" => Some("struct"),
            "cbuffer_specifier" if matches!(language, ShaderLanguage::Hlsl) => Some("cbuffer"),
            "declaration"
                if matches!(language, ShaderLanguage::Glsl)
                    && text.contains('{')
                    && ["uniform ", "buffer ", "in ", "out "]
                        .iter()
                        .any(|qualifier| text.contains(qualifier)) =>
            {
                Some("interface")
            }
            _ => None,
        };
        if let Some(label) = label {
            if let Some(name) = shader_item_name(child, src, label) {
                items.push(RawItem {
                    kind: SymbolKind::Item {
                        label: label.into(),
                    },
                    name,
                    signature: item_signature(child, src),
                    doc: item_doc(src, child.byte_range().start),
                    byte_range: child.byte_range(),
                    line_count: (child.end_position().row - child.start_position().row + 1) as u64,
                    children: collect_shader_items(child, src, language),
                });
                continue;
            }
        }
        items.extend(collect_shader_items(child, src, language));
    }
    items
}

fn shader_item_name(node: Node, src: &[u8], label: &str) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name") {
        return Some(node_text(name, src));
    }
    if label == "interface" {
        let mut cursor = node.walk();
        return node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "identifier")
            .map(|name| node_text(name, src));
    }
    if label == "fn" {
        let declarator = find_descendant(node, "function_declarator")?;
        return find_descendant(declarator, "identifier").map(|name| node_text(name, src));
    }
    find_descendant(node, "identifier").map(|name| node_text(name, src))
}

fn find_descendant<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
        if let Some(found) = find_descendant(child, kind) {
            return Some(found);
        }
    }
    None
}

fn scan_hlsl_cbuffers(src: &[u8]) -> Vec<RawItem> {
    let text = String::from_utf8_lossy(src);
    let mut items = Vec::new();
    let mut search = 0;
    while let Some(relative) = text[search..].find("cbuffer") {
        let start = search + relative;
        let before_ok = start == 0 || !text.as_bytes()[start - 1].is_ascii_alphanumeric();
        let after = start + "cbuffer".len();
        let after_ok = text
            .as_bytes()
            .get(after)
            .is_some_and(|byte| byte.is_ascii_whitespace());
        if !before_ok || !after_ok {
            search = after;
            continue;
        }
        let name_start = after
            + text[after..]
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(text.len() - after);
        let name_end = name_start
            + text[name_start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(text.len() - name_start);
        let Some(open_rel) = text[name_end..].find('{') else {
            break;
        };
        let open = name_end + open_rel;
        let mut depth = 0usize;
        let mut end = text.len();
        for (offset, byte) in text.as_bytes()[open..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + offset + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if name_start < name_end && end > open {
            let range = start..end;
            items.push(RawItem {
                kind: SymbolKind::Item {
                    label: "cbuffer".into(),
                },
                name: text[name_start..name_end].to_string(),
                signature: text[range.clone()]
                    .split('{')
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                doc: item_doc(src, start),
                byte_range: range,
                line_count: text[start..end].lines().count() as u64,
                children: Vec::new(),
            });
        }
        search = end.max(after);
    }
    items
}

/// Recursively walks a syntax tree, collecting nodes accepted by `kind_fn` into
/// `RawItem`s; unrecognized nodes are transparent (children are still visited).
fn collect_items(
    node: Node,
    src: &[u8],
    kind_fn: &KindClassifier,
    name_fn: &NameExtractor,
) -> Vec<RawItem> {
    collect_items_with_doc(node, src, kind_fn, name_fn, None)
}

fn collect_items_with_doc(
    node: Node,
    src: &[u8],
    kind_fn: &KindClassifier,
    name_fn: &NameExtractor,
    doc_fn: Option<&DocExtractor>,
) -> Vec<RawItem> {
    let mut items = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(label) = kind_fn(child.kind(), child, src) {
            let doc = item_doc(src, child.byte_range().start)
                .or_else(|| doc_fn.and_then(|f| f(child, src)));
            items.push(RawItem {
                kind: SymbolKind::Item {
                    label: label.into(),
                },
                name: name_fn(child, src),
                signature: item_signature(child, src),
                doc,
                byte_range: child.byte_range(),
                line_count: (child.end_position().row - child.start_position().row + 1) as u64,
                children: collect_items_with_doc(child, src, kind_fn, name_fn, doc_fn),
            });
        } else {
            items.extend(collect_items_with_doc(child, src, kind_fn, name_fn, doc_fn));
        }
    }
    items
}

/// Returns the source slice for a tree-sitter node as a UTF-8 string.
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

/// Consecutive `///` lines directly above an item, scanned backwards from
/// its start: the marker plus one following space is stripped and lines are
/// joined like `file_doc`. Attribute/decorator lines (`#[…]`, `[…]`, `@…`)
/// and blanks between the block and the item are skipped; `////` separators
/// and any other content stop the scan. None when there is no block.
fn item_doc(src: &[u8], item_start: usize) -> Option<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut end = item_start;
    loop {
        let start = src[..end]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |p| p + 1);
        let line = String::from_utf8_lossy(&src[start..end]);
        let t = line.trim();
        match t.strip_prefix("///") {
            Some(rest) if !rest.starts_with('/') => {
                collected.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
            _ => {
                let gap =
                    t.is_empty() || t.starts_with("#[") || t.starts_with('[') || t.starts_with('@');
                if !collected.is_empty() || !gap {
                    break;
                }
            }
        }
        if start == 0 {
            break;
        }
        end = start - 1;
    }
    if collected.is_empty() {
        None
    } else {
        collected.reverse();
        Some(collected.join("\n"))
    }
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
    use super::{
        parse_c_items, parse_cpp_items, parse_csharp_items, parse_js_items, parse_make_items,
        parse_glsl_items, parse_hlsl_items, parse_python_items, parse_rust_items, parse_ts_items,
    };
    use crate::types::SymbolKind;

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

    fn assert_make_coverage(source: &[u8], items: &[super::RawItem]) {
        if source.is_empty() {
            assert!(items.is_empty());
            return;
        }
        assert_eq!(items.first().unwrap().byte_range.start, 0);
        assert_eq!(items.last().unwrap().byte_range.end, source.len());
        for item in items {
            assert!(item.byte_range.start < item.byte_range.end);
        }
        for pair in items.windows(2) {
            assert!(pair[0].byte_range.start < pair[1].byte_range.start);
            assert_eq!(pair[0].byte_range.end, pair[1].byte_range.start);
        }
    }

    #[test]
    fn make_extracts_explicit_pattern_multi_target_and_recipe_rules() {
        let src = b"all: build\n\t@echo done\n\n%.o: %.c\n\t$(CC) -c $<\n\nclean install: prep\n\trm -f *.o\n";
        let items = parse_make_items(src).unwrap();
        let targets: Vec<_> = items
            .iter()
            .filter(|item| item.kind.label() == "target")
            .collect();
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].name, "all");
        assert_eq!(targets[0].signature, "all: build");
        assert_eq!(targets[0].line_count, 3);
        assert_eq!(targets[1].name, "%.o");
        assert_eq!(targets[1].signature, "%.o: %.c");
        assert_eq!(targets[2].name, "clean install");
        assert_eq!(targets[2].signature, "clean install: prep");
        assert_eq!(targets[2].children, vec![]);
        assert_eq!(targets[2].doc, None);
        assert_make_coverage(src, &items);
    }

    #[test]
    fn make_static_pattern_rule_uses_actual_targets_for_name() {
        let items = parse_make_items(b"objects: %.o: %.c\n\t$(CC) -c $<\n").unwrap();
        let target = items
            .iter()
            .find(|item| item.kind.label() == "target")
            .unwrap();

        assert_eq!(target.name, "objects");
        assert_eq!(target.signature, "objects: %.o: %.c");
    }

    #[test]
    fn make_signature_includes_every_physical_line_of_a_continued_header() {
        let src = concat!(
            "bundle: first \\\n",
            "    second \\\n",
            "    third\n",
            "\t@echo bundled\n",
        )
        .as_bytes();
        let items = parse_make_items(src).unwrap();
        let target = items
            .iter()
            .find(|item| item.kind.label() == "target")
            .unwrap();
        assert_eq!(
            target.signature,
            concat!("bundle: first \\\n", "    second \\\n", "    third")
        );
        assert_make_coverage(src, &items);
    }

    #[test]
    fn make_preserves_and_labels_non_target_constructs() {
        let src = b"# heading\nCC := cc\ninclude common.mk\n\nifeq ($(DEBUG),1)\ndebug: ; @echo debug\nendif\n\ndefine banner\nhello\nendef\n";
        let items = parse_make_items(src).unwrap();
        assert_eq!(
            items
                .iter()
                .filter(|item| item.kind.label() == "target")
                .count(),
            1
        );
        let section_names: Vec<_> = items
            .iter()
            .filter(|item| item.kind.label() == "section")
            .map(|item| item.name.as_str())
            .collect();
        assert!(section_names.contains(&"Definitions"));
        assert!(section_names.contains(&"Conditionals"));
        assert_make_coverage(src, &items);

        for (construct, expected) in [
            (b"CC := cc\n".as_slice(), "Variables"),
            (b"include common.mk\n".as_slice(), "Includes"),
            (b"# heading\n".as_slice(), "Preamble"),
        ] {
            let section = parse_make_items(construct).unwrap();
            assert_eq!(section[0].name, expected);
        }
    }

    #[test]
    fn make_target_free_and_malformed_sources_preserve_every_byte() {
        for src in [
            b"VAR = value\ninclude config.mk\n# comment\n".as_slice(),
            b"broken: target\n\tunterminated $(value\nifeq (x,y\ntrailing".as_slice(),
        ] {
            let items = parse_make_items(src).unwrap();
            assert_make_coverage(src, &items);
            if !src.starts_with(b"broken") {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].kind.label(), "section");
            }
        }
        assert!(parse_make_items(b"").unwrap().is_empty());
    }

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
                (
                    SymbolKind::Item {
                        label: "module".into()
                    },
                    "inner",
                    1
                ),
                (
                    SymbolKind::Item {
                        label: "struct".into()
                    },
                    "Point",
                    0
                ),
                (
                    SymbolKind::Item {
                        label: "impl".into()
                    },
                    "Point",
                    2
                ),
                (SymbolKind::Item { label: "fn".into() }, "free", 0),
            ]
        );

        // nested fn inside mod
        assert_eq!(items[0].children[0].name, "helper");
        assert_eq!(
            items[0].children[0].kind,
            SymbolKind::Item { label: "fn".into() }
        );

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
        assert_eq!(
            items[0].kind,
            SymbolKind::Item {
                label: "trait".into()
            }
        );
        assert_eq!(items[0].name, "Show");
        assert_eq!(
            items[1].kind,
            SymbolKind::Item {
                label: "impl".into()
            }
        );
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
        assert_eq!(
            file_doc(b"\n\n//! After blanks.\nfn x() {}\n"),
            Some("After blanks.".to_string())
        );
        assert_eq!(file_doc(b"fn x() {}\n"), None);
        assert_eq!(file_doc(b"// plain comment\n//! not leading\n"), None);
    }

    #[test]
    fn item_docs_attach_to_rust_items() {
        let src = b"/// Adds numbers.\n/// Second line.\n#[inline]\npub fn add() {}\n\nfn bare() {}\n\n/// Struct doc.\n#[derive(Debug)]\nstruct S { x: i32 }\n\n//// separator, not a doc\nfn sep() {}\n";
        let items = super::parse_rust_items(src).unwrap();
        assert_eq!(items[0].doc.as_deref(), Some("Adds numbers.\nSecond line."));
        assert_eq!(items[1].doc, None);
        assert_eq!(items[2].doc.as_deref(), Some("Struct doc."));
        assert_eq!(items[3].doc, None);
    }

    #[test]
    fn item_docs_attach_to_nested_methods_and_ignore_file_docs() {
        let src = b"//! File doc.\n\nstruct P;\n\nimpl P {\n    /// Frobs the P.\n    fn frob(&self) {}\n\n    fn plain(&self) {}\n}\n";
        let items = super::parse_rust_items(src).unwrap();
        // the leading //! block must not attach to the first item
        assert_eq!(items[0].doc, None);
        let imp = &items[1];
        assert_eq!(imp.children[0].doc.as_deref(), Some("Frobs the P."));
        assert_eq!(imp.children[1].doc, None);
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
    fn python_docstrings_on_functions_and_classes() {
        let src = br#"
def greet(name):
    """Say hello to someone."""
    print(f"Hello {name}")

class Dog:
    """A good dog.

    Dogs are the best.
    """
    def bark(self):
        """Make noise."""
        pass

    def sit(self):
        pass
"#;
        let items = parse_python_items(src).unwrap();
        assert_eq!(items[0].doc.as_deref(), Some("Say hello to someone."));
        assert_eq!(
            items[1].doc.as_deref(),
            Some("A good dog.\n\nDogs are the best.")
        );
        assert_eq!(items[1].children[0].doc.as_deref(), Some("Make noise."));
        assert_eq!(items[1].children[1].doc, None);
    }

    #[test]
    fn python_docstring_single_quotes() {
        let src = br#"
def f():
    '''Single-quoted docstring.'''
    pass
"#;
        let items = parse_python_items(src).unwrap();
        assert_eq!(items[0].doc.as_deref(), Some("Single-quoted docstring."));
    }

    #[test]
    fn python_docstring_on_decorated_definition() {
        let src = br#"
@staticmethod
def helper():
    """Decorated helper."""
    pass
"#;
        let items = parse_python_items(src).unwrap();
        assert_eq!(items[0].doc.as_deref(), Some("Decorated helper."));
    }

    #[test]
    fn python_triple_slash_takes_precedence_over_docstring() {
        let src = br#"
/// Triple-slash comment.
def f():
    """Docstring."""
    pass
"#;
        let items = parse_python_items(src).unwrap();
        assert_eq!(items[0].doc.as_deref(), Some("Triple-slash comment."));
    }

    #[test]
    fn python_file_doc_extracts_module_docstring() {
        use super::python_file_doc;
        assert_eq!(
            python_file_doc(b"\"\"\"Module doc.\"\"\"\n\ndef f():\n    pass\n"),
            Some("Module doc.".to_string())
        );
        assert_eq!(
            python_file_doc(b"'''Multi-line\nmodule doc.'''\n\ndef f():\n    pass\n"),
            Some("Multi-line\nmodule doc.".to_string())
        );
        assert_eq!(python_file_doc(b"def f():\n    pass\n"), None);
        assert_eq!(python_file_doc(b"import os\n\"\"\"Not first.\"\"\""), None);
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

    #[test]
    fn extracts_csharp_items() {
        let src = br#"
namespace MyApp {
    class Greeter {
        public Greeter() {
        }
        public string Greet() {
            return "hello";
        }
    }

    interface IPrintable {
        void Print();
    }

    enum Color {
        Red,
        Green,
        Blue
    }

    struct Point {
        public int X;
        public int Y;
    }

    record Person(string Name, int Age);
}
"#;
        let items = parse_csharp_items(src).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind.label(), "namespace");
        assert_eq!(items[0].name, "MyApp");
        let inner: Vec<(&str, &str)> = items[0]
            .children
            .iter()
            .map(|i| (i.kind.label(), i.name.as_str()))
            .collect();
        assert_eq!(
            inner,
            vec![
                ("class", "Greeter"),
                ("interface", "IPrintable"),
                ("enum", "Color"),
                ("struct", "Point"),
                ("class", "Person"),
            ]
        );
        // Greeter has constructor + method
        assert_eq!(items[0].children[0].children.len(), 2);
        assert_eq!(items[0].children[0].children[0].kind.label(), "fn");
        assert_eq!(items[0].children[0].children[0].name, "Greeter");
        assert_eq!(items[0].children[0].children[1].name, "Greet");
    }

    #[test]
    fn extracts_cpp_items() {
        let src = br#"
namespace geometry {

class Shape {
public:
    virtual ~Shape() {}
    virtual double area() const = 0;
};

class Circle : public Shape {
public:
    Circle(double r) : radius(r) {}
    ~Circle() {}
    double area() const override {
        return 3.14159 * radius * radius;
    }
private:
    double radius;
};

struct Point {
    double x;
    double y;
};

enum Color { RED, GREEN, BLUE };

}

typedef unsigned long ulong;

void standalone() {
    // body
}
"#;
        let items = parse_cpp_items(src).unwrap();
        assert_eq!(items[0].kind.label(), "namespace");
        assert_eq!(items[0].name, "geometry");
        let inner: Vec<(&str, &str)> = items[0]
            .children
            .iter()
            .map(|i| (i.kind.label(), i.name.as_str()))
            .collect();
        assert_eq!(
            inner,
            vec![
                ("class", "Shape"),
                ("class", "Circle"),
                ("struct", "Point"),
                ("enum", "Color"),
            ]
        );
        // Shape has destructor only (pure virtual decl is not a definition)
        assert_eq!(items[0].children[0].children.len(), 1);
        // Circle has constructor + destructor + method
        assert_eq!(items[0].children[1].children.len(), 3);

        // Top-level items after namespace
        let top: Vec<(&str, &str)> = items[1..]
            .iter()
            .map(|i| (i.kind.label(), i.name.as_str()))
            .collect();
        assert_eq!(top, vec![("typedef", "ulong"), ("fn", "standalone"),]);
    }

    #[test]
    fn extracts_cpp_templates_and_qualified_names() {
        let src = br#"
template <typename T>
class Container {
public:
    void add(T item) {}
};

class Foo {
public:
    void bar();
};

void Foo::bar() {
    // out-of-line definition
}
"#;
        let items = parse_cpp_items(src).unwrap();
        // Template declaration is transparent, so we see the inner class
        assert_eq!(items[0].kind.label(), "class");
        assert_eq!(items[0].name, "Container");
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].name, "add");

        // Foo class with declared method
        assert_eq!(items[1].kind.label(), "class");
        assert_eq!(items[1].name, "Foo");

        // Out-of-line Foo::bar
        assert_eq!(items[2].kind.label(), "fn");
        assert_eq!(items[2].name, "Foo::bar");
    }

    #[test]
    fn glsl_extracts_structs_interfaces_functions_and_recovers() {
        let src = br#"#version 450
struct Light { vec3 position; };
uniform Scene { mat4 view; } scene;
out VertexData { vec3 normal; } vertex;
void main() {}
@broken
void recover() {}
"#;
        let items = parse_glsl_items(src).unwrap();
        let flat: Vec<(&str, &str)> = items
            .iter()
            .map(|item| (item.kind.label(), item.name.as_str()))
            .collect();
        assert!(flat.contains(&("struct", "Light")), "{flat:?}");
        assert!(flat.contains(&("interface", "Scene")), "{flat:?}");
        assert!(flat.contains(&("interface", "VertexData")), "{flat:?}");
        assert!(flat.contains(&("fn", "main")), "{flat:?}");
        assert!(flat.contains(&("fn", "recover")), "{flat:?}");
        for item in &items {
            assert!(!&src[item.byte_range.clone()].is_empty());
        }
    }

    #[test]
    fn hlsl_extracts_structs_cbuffers_functions_and_recovers() {
        let src = br#"struct VSInput { float3 position : POSITION; };
cbuffer Camera : register(b0) { float4x4 view; };
float4 main(VSInput input) : SV_Target { return 1; }
@broken
float4 recover() : SV_Target { return 0; }
"#;
        let items = parse_hlsl_items(src).unwrap();
        let flat: Vec<(&str, &str)> = items
            .iter()
            .map(|item| (item.kind.label(), item.name.as_str()))
            .collect();
        assert!(flat.contains(&("struct", "VSInput")), "{flat:?}");
        assert!(flat.contains(&("cbuffer", "Camera")), "{flat:?}");
        assert!(flat.contains(&("fn", "main")), "{flat:?}");
        assert!(flat.contains(&("fn", "recover")), "{flat:?}");
    }
}
