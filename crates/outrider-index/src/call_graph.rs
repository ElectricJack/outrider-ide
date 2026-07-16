use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::types::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

#[derive(Debug, Clone)]
pub struct CallEdge {
    pub target: SymbolId,
    pub raw_name: String,
    pub call_site: Option<Range<usize>>,
}

#[derive(Debug, Clone, Default)]
pub struct CallGraphData {
    pub callers: Vec<CallEdge>,
    pub callees: Vec<CallEdge>,
}

pub fn resolve_calls(center: &SymbolId, tree: &SymbolTree) -> CallGraphData {
    let center_node = find_node(&tree.root, center);
    let center_node = match center_node {
        Some(n) => n,
        None => return CallGraphData::default(),
    };
    let byte_range = match &center_node.byte_range {
        Some(r) => r.clone(),
        None => return CallGraphData::default(),
    };

    let file_rel = file_path_of(&center.qualified_path);
    let file_path = tree.repo_root.join(file_rel);
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let language = match language_for(ext) {
        Some(l) => l,
        None => return CallGraphData::default(),
    };

    let source = match std::fs::read(&file_path) {
        Ok(s) => s,
        Err(_) => return CallGraphData::default(),
    };

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return CallGraphData::default();
    }
    let parsed = match parser.parse(&source, None) {
        Some(t) => t,
        None => return CallGraphData::default(),
    };

    let raw_calls = extract_calls_from_tree(parsed.root_node(), &source, byte_range.clone(), ext);
    let all_fns = collect_all_functions(&tree.root);

    let type_env = crate::type_resolve::resolver_for(ext).map(|resolver| {
        let class_names = collect_class_names(&tree.root);
        let enclosing_class = find_enclosing_class(center);
        let fn_return_types: std::collections::HashMap<String, String> = all_fns
            .iter()
            .filter_map(|f| {
                f.return_type
                    .as_ref()
                    .map(|rt| (f.name.clone(), rt.clone()))
            })
            .collect();
        resolver.build_scope_types(
            &source,
            &parsed,
            byte_range,
            enclosing_class.as_deref(),
            &class_names,
            &fn_return_types,
        )
    });
    let callees = match_calls_to_symbols(&raw_calls, &all_fns, center, type_env.as_ref());

    let center_name = &center_node.name;
    let callers = find_callers(center_name, center, &all_fns, tree, ext);

    CallGraphData { callers, callees }
}

fn find_node<'a>(root: &'a SymbolNode, id: &SymbolId) -> Option<&'a SymbolNode> {
    if root.id == *id {
        return Some(root);
    }
    root.children.iter().find_map(|c| find_node(c, id))
}

fn file_path_of(qualified_path: &str) -> &str {
    let s = qualified_path.split("::").next().unwrap_or(qualified_path);
    s.split('#').next().unwrap_or(s)
}

fn language_for(ext: &str) -> Option<tree_sitter::Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "js" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "cs" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        _ => None,
    }
}

fn is_call_node(kind: &str, ext: &str) -> bool {
    match ext {
        "py" => kind == "call",
        "cs" => kind == "invocation_expression",
        _ => kind == "call_expression",
    }
}

struct RawCall {
    name: String,
    site: Range<usize>,
    receiver: Option<String>,
}

#[cfg(test)]
fn extract_calls(
    source: &[u8],
    language: tree_sitter::Language,
    byte_range: Range<usize>,
    ext: &str,
) -> Vec<RawCall> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return Vec::new(),
    };
    extract_calls_from_tree(tree.root_node(), source, byte_range, ext)
}

fn extract_calls_from_tree(
    root: Node,
    source: &[u8],
    byte_range: Range<usize>,
    ext: &str,
) -> Vec<RawCall> {
    let mut calls = Vec::new();
    let mut seen = HashSet::new();
    walk_for_calls(root, source, &byte_range, ext, &mut calls, &mut seen);
    calls
}

#[cfg(test)]
fn extract_call_names(
    source: &[u8],
    language: tree_sitter::Language,
    byte_range: Range<usize>,
    ext: &str,
) -> Vec<String> {
    extract_calls(source, language, byte_range, ext)
        .into_iter()
        .map(|c| c.name)
        .collect()
}

fn extract_call_names_from_tree(
    root: Node,
    source: &[u8],
    byte_range: Range<usize>,
    ext: &str,
) -> Vec<String> {
    extract_calls_from_tree(root, source, byte_range, ext)
        .into_iter()
        .map(|c| c.name)
        .collect()
}

fn extract_receiver(call_node: Node, source: &[u8], ext: &str) -> Option<String> {
    let func_node = if ext == "cs" {
        call_node.child(0)?
    } else {
        call_node.child_by_field_name("function")?
    };
    match func_node.kind() {
        "field_expression" | "member_expression" | "attribute" | "member_access_expression" => {
            let obj = func_node
                .child_by_field_name("object")
                .or_else(|| func_node.child_by_field_name("value"));
            match obj {
                Some(o) if o.kind() == "identifier" || o.kind() == "this" => {
                    Some(node_text(o, source))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn walk_for_calls(
    node: Node,
    source: &[u8],
    range: &Range<usize>,
    ext: &str,
    calls: &mut Vec<RawCall>,
    seen: &mut HashSet<String>,
) {
    if node.end_byte() <= range.start || node.start_byte() >= range.end {
        return;
    }
    if is_call_node(node.kind(), ext) && node.start_byte() >= range.start {
        if let Some(name) = callee_name(node, source, ext) {
            if seen.insert(name.clone()) {
                let receiver = extract_receiver(node, source, ext);
                calls.push(RawCall {
                    name,
                    site: node.byte_range(),
                    receiver,
                });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_for_calls(child, source, range, ext, calls, seen);
    }
}

fn callee_name(call_node: Node, source: &[u8], ext: &str) -> Option<String> {
    let func_node = if ext == "cs" {
        call_node.child(0)?
    } else {
        call_node.child_by_field_name("function")?
    };
    Some(extract_name_from_expr(func_node, source))
}

fn extract_name_from_expr(node: Node, source: &[u8]) -> String {
    match node.kind() {
        "identifier" | "type_identifier" => node_text(node, source),
        "field_expression" | "member_expression" | "attribute" | "member_access_expression" => {
            let field = node
                .child_by_field_name("field")
                .or_else(|| node.child_by_field_name("name"))
                .or_else(|| node.child_by_field_name("attribute"));
            match field {
                Some(f) => node_text(f, source),
                None => {
                    if let Some(last) = node.child(node.child_count().saturating_sub(1) as u32) {
                        node_text(last, source)
                    } else {
                        node_text(node, source)
                    }
                }
            }
        }
        "scoped_identifier" | "qualified_identifier" => {
            let name = node.child_by_field_name("name");
            match name {
                Some(n) => {
                    let scope = node.child_by_field_name("path");
                    match scope {
                        Some(s) => format!("{}::{}", node_text(s, source), node_text(n, source)),
                        None => node_text(n, source),
                    }
                }
                None => node_text(node, source),
            }
        }
        _ => node_text(node, source),
    }
}

fn node_text(node: Node, source: &[u8]) -> String {
    std::str::from_utf8(&source[node.byte_range()])
        .unwrap_or("")
        .to_string()
}

struct FnEntry {
    id: SymbolId,
    name: String,
    file_rel: String,
    byte_range: Range<usize>,
    return_type: Option<String>,
}

fn extract_return_type(signature: &str, ext: &str) -> Option<String> {
    match ext {
        "ts" | "tsx" | "js" | "jsx" => {
            let paren_close = signature.rfind(')')?;
            let after = &signature[paren_close + 1..];
            let colon = after.find(':')?;
            let ty = after[colon + 1..].trim().trim_end_matches('{').trim();
            let word = ty
                .split(|c: char| c.is_whitespace() || c == '{' || c == '<')
                .next()
                .unwrap_or(ty);
            if word.is_empty() {
                None
            } else {
                Some(word.to_string())
            }
        }
        _ => {
            let arrow = signature.find(" -> ")?;
            let after = &signature[arrow + 4..];
            let ty = after.trim().trim_end_matches(':').trim();
            let word = ty
                .split(|c: char| c.is_whitespace() || c == '{' || c == ':' || c == '<')
                .next()
                .unwrap_or(ty);
            if word.is_empty() {
                None
            } else {
                Some(word.to_string())
            }
        }
    }
}

fn collect_all_functions(root: &SymbolNode) -> Vec<FnEntry> {
    let mut out = Vec::new();
    collect_fns_recursive(root, &mut out);
    out
}

fn collect_fns_recursive(node: &SymbolNode, out: &mut Vec<FnEntry>) {
    if let SymbolKind::Item { ref label } = node.id.kind {
        if label == "fn" {
            if let Some(ref range) = node.byte_range {
                let file_rel = file_path_of(&node.id.qualified_path).to_string();
                let ext = file_rel.rsplit('.').next().unwrap_or("");
                let return_type = node
                    .signature
                    .as_deref()
                    .and_then(|sig| extract_return_type(sig, ext));
                out.push(FnEntry {
                    id: node.id.clone(),
                    name: node.name.clone(),
                    file_rel,
                    byte_range: range.clone(),
                    return_type,
                });
            }
        }
    }
    for child in &node.children {
        collect_fns_recursive(child, out);
    }
}

fn name_matches_fn(name: &str, func: &FnEntry) -> bool {
    let simple = name.split("::").last().unwrap_or(name);
    if name.contains("::") {
        let parts: Vec<&str> = name.split("::").collect();
        if parts.len() == 2 {
            let parent_name = parts[0];
            let fn_name = parts[1];
            func.name == fn_name
                && func
                    .id
                    .qualified_path
                    .contains(&format!("::{parent_name}::{fn_name}"))
        } else {
            func.name == simple
        }
    } else {
        func.name == *name
    }
}

fn collect_class_names(root: &SymbolNode) -> HashSet<String> {
    let mut out = HashSet::new();
    collect_class_names_recursive(root, &mut out);
    out
}

fn collect_class_names_recursive(node: &SymbolNode, out: &mut HashSet<String>) {
    if let SymbolKind::Item { ref label } = node.id.kind {
        if label == "class" || label == "struct" || label == "impl" {
            out.insert(node.name.clone());
        }
    }
    for child in &node.children {
        collect_class_names_recursive(child, out);
    }
}

fn find_enclosing_class(center: &SymbolId) -> Option<String> {
    let parts: Vec<&str> = center.qualified_path.split("::").collect();
    if parts.len() >= 3 {
        Some(parts[parts.len() - 2].to_string())
    } else {
        None
    }
}

fn is_method_on_class(func: &FnEntry, class_name: &str) -> bool {
    func.id
        .qualified_path
        .contains(&format!("::{class_name}::"))
}

fn match_calls_to_symbols(
    calls: &[RawCall],
    all_fns: &[FnEntry],
    exclude: &SymbolId,
    type_env: Option<&crate::type_resolve::TypeEnv>,
) -> Vec<CallEdge> {
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    for call in calls {
        let resolved_class = call
            .receiver
            .as_deref()
            .and_then(|r| type_env.and_then(|env| env.resolve(r)));

        for func in all_fns {
            if func.id == *exclude {
                continue;
            }
            if !name_matches_fn(&call.name, func) {
                continue;
            }
            if let Some(class) = resolved_class {
                if !is_method_on_class(func, class) {
                    continue;
                }
            }
            if seen.insert(func.id.clone()) {
                edges.push(CallEdge {
                    target: func.id.clone(),
                    raw_name: call.name.clone(),
                    call_site: Some(call.site.clone()),
                });
            }
        }
    }
    edges
}

#[cfg(test)]
fn match_names_to_symbols(
    names: &[String],
    all_fns: &[FnEntry],
    exclude: &SymbolId,
) -> Vec<CallEdge> {
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    for name in names {
        for func in all_fns {
            if func.id == *exclude {
                continue;
            }
            if name_matches_fn(name, func) && seen.insert(func.id.clone()) {
                edges.push(CallEdge {
                    target: func.id.clone(),
                    raw_name: name.clone(),
                    call_site: None,
                });
            }
        }
    }
    edges
}

fn find_callers(
    target_name: &str,
    target_id: &SymbolId,
    all_fns: &[FnEntry],
    tree: &SymbolTree,
    _ext: &str,
) -> Vec<CallEdge> {
    let target_bytes = target_name.as_bytes();
    let mut callers = Vec::new();

    struct FileData {
        source: Vec<u8>,
        parsed: tree_sitter::Tree,
        ext: String,
    }

    let mut file_cache: std::collections::HashMap<String, Option<FileData>> =
        std::collections::HashMap::new();

    for func in all_fns {
        if func.id == *target_id {
            continue;
        }
        let file_data = file_cache.entry(func.file_rel.clone()).or_insert_with(|| {
            let path = tree.repo_root.join(&func.file_rel);
            let bytes = std::fs::read(&path).ok()?;
            if memchr::memmem::find(&bytes, target_bytes).is_none() {
                return None;
            }
            let ext = func.file_rel.rsplit('.').next().unwrap_or("").to_string();
            let language = language_for(&ext)?;
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&language).ok()?;
            let parsed = parser.parse(&bytes, None)?;
            Some(FileData {
                source: bytes,
                parsed,
                ext,
            })
        });
        let fd = match file_data {
            Some(fd) => fd,
            None => continue,
        };

        let call_names = extract_call_names_from_tree(
            fd.parsed.root_node(),
            &fd.source,
            func.byte_range.clone(),
            &fd.ext,
        );
        let is_caller = call_names.iter().any(|n| {
            let simple = n.split("::").last().unwrap_or(n);
            simple == target_name
        });
        if is_caller {
            callers.push(CallEdge {
                target: func.id.clone(),
                raw_name: func.name.clone(),
                call_site: None,
            });
        }
    }
    callers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree(source: &str, ext: &str) -> (Vec<u8>, tree_sitter::Tree) {
        let bytes = source.as_bytes().to_vec();
        let lang = language_for(ext).unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(&bytes, None).unwrap();
        (bytes, tree)
    }

    #[test]
    fn rust_extracts_direct_calls() {
        let src = r#"
fn main() {
    foo();
    Bar::baz();
    obj.method();
}
fn foo() {}
"#;
        let (bytes, _tree) = make_tree(src, "rs");
        let names = extract_call_names(&bytes, language_for("rs").unwrap(), 1..src.len(), "rs");
        assert!(names.contains(&"foo".to_string()));
        assert!(names.contains(&"Bar::baz".to_string()));
        assert!(names.contains(&"method".to_string()));
    }

    #[test]
    fn python_extracts_calls() {
        let src = r#"
def main():
    foo()
    obj.method()
"#;
        let (bytes, _tree) = make_tree(src, "py");
        let names = extract_call_names(&bytes, language_for("py").unwrap(), 1..src.len(), "py");
        assert!(names.contains(&"foo".to_string()));
        assert!(names.contains(&"method".to_string()));
    }

    #[test]
    fn js_extracts_calls() {
        let src = r#"
function main() {
    foo();
    obj.method();
}
"#;
        let (bytes, _tree) = make_tree(src, "js");
        let names = extract_call_names(&bytes, language_for("js").unwrap(), 1..src.len(), "js");
        assert!(names.contains(&"foo".to_string()));
        assert!(names.contains(&"method".to_string()));
    }

    #[test]
    fn call_names_are_deduplicated() {
        let src = r#"
fn main() {
    foo();
    foo();
    foo();
}
"#;
        let (bytes, _tree) = make_tree(src, "rs");
        let names = extract_call_names(&bytes, language_for("rs").unwrap(), 1..src.len(), "rs");
        assert_eq!(names.iter().filter(|n| *n == "foo").count(), 1);
    }

    #[test]
    fn match_names_resolves_simple_name() {
        let fns = vec![FnEntry {
            id: SymbolId {
                kind: SymbolKind::Item {
                    label: "fn".to_string(),
                },
                qualified_path: "src/lib.rs::foo".to_string(),
                ordinal: 0,
            },
            name: "foo".to_string(),
            file_rel: "src/lib.rs".to_string(),
            byte_range: 0..10,
            return_type: None,
        }];
        let exclude = SymbolId {
            kind: SymbolKind::Item {
                label: "fn".to_string(),
            },
            qualified_path: "src/main.rs::main".to_string(),
            ordinal: 0,
        };
        let edges = match_names_to_symbols(&["foo".to_string()], &fns, &exclude);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target.qualified_path, "src/lib.rs::foo");
    }

    #[test]
    fn match_names_resolves_qualified_name() {
        let fns = vec![FnEntry {
            id: SymbolId {
                kind: SymbolKind::Item {
                    label: "fn".to_string(),
                },
                qualified_path: "src/lib.rs::Bar::baz".to_string(),
                ordinal: 0,
            },
            name: "baz".to_string(),
            file_rel: "src/lib.rs".to_string(),
            byte_range: 0..10,
            return_type: None,
        }];
        let exclude = SymbolId {
            kind: SymbolKind::Item {
                label: "fn".to_string(),
            },
            qualified_path: "src/main.rs::main".to_string(),
            ordinal: 0,
        };
        let edges = match_names_to_symbols(&["Bar::baz".to_string()], &fns, &exclude);
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn python_receiver_filtering_with_type_env() {
        let src = "class Dog:\n    def speak(self):\n        pass\n\nclass Cat:\n    def speak(self):\n        pass\n\ndef main():\n    d = Dog()\n    d.speak()\n";
        let (bytes, tree) = make_tree(src, "py");

        let main_start = src.find("def main").unwrap();
        let calls = extract_calls_from_tree(tree.root_node(), &bytes, main_start..src.len(), "py");
        assert_eq!(calls.len(), 2);
        let speak_call = calls.iter().find(|c| c.name == "speak").unwrap();
        assert_eq!(speak_call.receiver.as_deref(), Some("d"));

        let mut env = crate::type_resolve::TypeEnv::default();
        env.bind("d".to_string(), "Dog".to_string());

        let dog_speak = FnEntry {
            id: SymbolId {
                kind: SymbolKind::Item {
                    label: "fn".to_string(),
                },
                qualified_path: "test.py::Dog::speak".to_string(),
                ordinal: 0,
            },
            name: "speak".to_string(),
            file_rel: "test.py".to_string(),
            byte_range: 0..30,
            return_type: None,
        };
        let cat_speak = FnEntry {
            id: SymbolId {
                kind: SymbolKind::Item {
                    label: "fn".to_string(),
                },
                qualified_path: "test.py::Cat::speak".to_string(),
                ordinal: 0,
            },
            name: "speak".to_string(),
            file_rel: "test.py".to_string(),
            byte_range: 31..60,
            return_type: None,
        };
        let main_id = SymbolId {
            kind: SymbolKind::Item {
                label: "fn".to_string(),
            },
            qualified_path: "test.py::main".to_string(),
            ordinal: 0,
        };

        let all_fns = &[dog_speak, cat_speak];
        let edges_without_env = match_calls_to_symbols(&calls, all_fns, &main_id, None);
        let speak_edges: Vec<_> = edges_without_env
            .iter()
            .filter(|e| e.raw_name == "speak")
            .collect();
        assert_eq!(speak_edges.len(), 2);

        let edges_with_env = match_calls_to_symbols(&calls, all_fns, &main_id, Some(&env));
        let speak_edges: Vec<_> = edges_with_env
            .iter()
            .filter(|e| e.raw_name == "speak")
            .collect();
        assert_eq!(speak_edges.len(), 1);
        assert_eq!(speak_edges[0].target.qualified_path, "test.py::Dog::speak");
    }

    #[test]
    fn excludes_self_from_matches() {
        let id = SymbolId {
            kind: SymbolKind::Item {
                label: "fn".to_string(),
            },
            qualified_path: "src/lib.rs::foo".to_string(),
            ordinal: 0,
        };
        let fns = vec![FnEntry {
            id: id.clone(),
            name: "foo".to_string(),
            file_rel: "src/lib.rs".to_string(),
            byte_range: 0..10,
            return_type: None,
        }];
        let edges = match_names_to_symbols(&["foo".to_string()], &fns, &id);
        assert!(edges.is_empty());
    }
}
