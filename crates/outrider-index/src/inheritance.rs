use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use crate::types::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

#[derive(Debug, Clone)]
pub struct InheritanceEdge {
    pub child: SymbolId,
    pub parent: SymbolId,
    pub parent_name: String,
}

#[derive(Debug, Clone, Default)]
pub struct InheritanceData {
    pub edges: Vec<InheritanceEdge>,
}

fn language_for(ext: &str) -> Option<tree_sitter::Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        // C has no inheritance, and C++ projects overwhelmingly use `.h`
        // for C++ headers, so always parse C-family files with the C++
        // grammar here (a superset for the declarations we inspect).
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => {
            Some(tree_sitter_cpp::LANGUAGE.into())
        }
        "js" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "cs" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        _ => None,
    }
}

fn node_text<'a>(node: Node<'a>, src: &'a [u8]) -> String {
    node.utf8_text(src).unwrap_or("").to_string()
}

struct RawInheritance {
    child_name: String,
    child_byte_range: Range<usize>,
    parent_names: Vec<String>,
}

fn extract_base_names(node: Node, src: &[u8], ext: &str) -> Vec<RawInheritance> {
    let mut out = Vec::new();
    extract_recursive(node, src, ext, &mut out);
    out
}

fn extract_recursive(node: Node, src: &[u8], ext: &str, out: &mut Vec<RawInheritance>) {
    match ext {
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" | "c" | "h" => {
            extract_cpp(node, src, out);
        }
        "rs" => {
            extract_rust(node, src, out);
        }
        "py" => {
            extract_python(node, src, out);
        }
        "ts" | "tsx" | "js" | "jsx" => {
            extract_typescript(node, src, out);
        }
        "cs" => {
            extract_csharp(node, src, out);
        }
        _ => {}
    }
}

fn extract_cpp(node: Node, src: &[u8], out: &mut Vec<RawInheritance>) {
    let kind = node.kind();
    if kind == "class_specifier" || kind == "struct_specifier" {
        let name = node
            .child_by_field_name("name")
            .map(|n| node_text(n, src))
            .unwrap_or_default();
        if !name.is_empty() {
            let mut parents = Vec::new();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "base_class_clause" {
                    let mut inner = child.walk();
                    for base in child.children(&mut inner) {
                        let base_name = extract_type_name(base, src);
                        if !base_name.is_empty()
                            && base_name != "public"
                            && base_name != "private"
                            && base_name != "protected"
                            && base_name != "virtual"
                        {
                            parents.push(base_name);
                        }
                    }
                }
            }
            if !parents.is_empty() {
                out.push(RawInheritance {
                    child_name: name,
                    child_byte_range: node.byte_range(),
                    parent_names: parents,
                });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_cpp(child, src, out);
    }
}

fn extract_rust(node: Node, src: &[u8], out: &mut Vec<RawInheritance>) {
    if node.kind() == "impl_item" {
        let trait_node = node.child_by_field_name("trait");
        let type_node = node.child_by_field_name("type");
        if let (Some(trait_n), Some(type_n)) = (trait_node, type_node) {
            let trait_name = extract_type_name(trait_n, src);
            let type_name = extract_type_name(type_n, src);
            if !trait_name.is_empty() && !type_name.is_empty() {
                out.push(RawInheritance {
                    child_name: type_name,
                    child_byte_range: node.byte_range(),
                    parent_names: vec![trait_name],
                });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_rust(child, src, out);
    }
}

fn extract_python(node: Node, src: &[u8], out: &mut Vec<RawInheritance>) {
    if node.kind() == "class_definition" {
        let name = node
            .child_by_field_name("name")
            .map(|n| node_text(n, src))
            .unwrap_or_default();
        if let Some(superclasses) = node.child_by_field_name("superclasses") {
            let mut parents = Vec::new();
            let mut cursor = superclasses.walk();
            for child in superclasses.children(&mut cursor) {
                let base = extract_type_name(child, src);
                if !base.is_empty() && base != "," && base != "(" && base != ")" {
                    parents.push(base);
                }
            }
            if !name.is_empty() && !parents.is_empty() {
                out.push(RawInheritance {
                    child_name: name,
                    child_byte_range: node.byte_range(),
                    parent_names: parents,
                });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_python(child, src, out);
    }
}

fn extract_typescript(node: Node, src: &[u8], out: &mut Vec<RawInheritance>) {
    let kind = node.kind();
    if kind == "class_declaration" || kind == "abstract_class_declaration" {
        let name = node
            .child_by_field_name("name")
            .map(|n| node_text(n, src))
            .unwrap_or_default();
        let mut parents = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "class_heritage" {
                let mut inner = child.walk();
                for clause in child.children(&mut inner) {
                    if clause.kind() == "extends_clause" || clause.kind() == "implements_clause" {
                        let mut c2 = clause.walk();
                        for val in clause.children(&mut c2) {
                            let base = extract_type_name(val, src);
                            if !base.is_empty()
                                && base != "extends"
                                && base != "implements"
                                && base != ","
                            {
                                parents.push(base);
                            }
                        }
                    }
                }
            }
        }
        if !name.is_empty() && !parents.is_empty() {
            out.push(RawInheritance {
                child_name: name,
                child_byte_range: node.byte_range(),
                parent_names: parents,
            });
        }
    }
    if kind == "interface_declaration" {
        let name = node
            .child_by_field_name("name")
            .map(|n| node_text(n, src))
            .unwrap_or_default();
        let mut parents = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "extends_type_clause" {
                if let Some(ty) = child.child_by_field_name("type") {
                    let base = extract_type_name(ty, src);
                    if !base.is_empty() {
                        parents.push(base);
                    }
                }
            }
        }
        if !name.is_empty() && !parents.is_empty() {
            out.push(RawInheritance {
                child_name: name,
                child_byte_range: node.byte_range(),
                parent_names: parents,
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_typescript(child, src, out);
    }
}

fn extract_csharp(node: Node, src: &[u8], out: &mut Vec<RawInheritance>) {
    let kind = node.kind();
    if kind == "class_declaration"
        || kind == "struct_declaration"
        || kind == "interface_declaration"
        || kind == "record_declaration"
    {
        let name = node
            .child_by_field_name("name")
            .map(|n| node_text(n, src))
            .unwrap_or_default();
        let mut parents = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "base_list" {
                let mut inner = child.walk();
                for base in child.children(&mut inner) {
                    let base_name = extract_type_name(base, src);
                    if !base_name.is_empty() && base_name != ":" && base_name != "," {
                        parents.push(base_name);
                    }
                }
            }
        }
        if !name.is_empty() && !parents.is_empty() {
            out.push(RawInheritance {
                child_name: name,
                child_byte_range: node.byte_range(),
                parent_names: parents,
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_csharp(child, src, out);
    }
}

fn extract_type_name(node: Node, src: &[u8]) -> String {
    match node.kind() {
        "type_identifier" | "identifier" => node_text(node, src),
        "qualified_identifier" | "scoped_type_identifier" | "scoped_identifier"
        | "nested_type_identifier" | "member_expression" => node_text(node, src),
        "generic_type" | "template_type" => {
            if let Some(name) = node.child_by_field_name("name") {
                node_text(name, src)
            } else if let Some(first) = node.named_child(0) {
                node_text(first, src)
            } else {
                String::new()
            }
        }
        _ => {
            let text = node_text(node, src).trim().to_string();
            if text.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ':') {
                text
            } else {
                String::new()
            }
        }
    }
}

struct TypeEntry {
    id: SymbolId,
    name: String,
}

fn collect_types(root: &SymbolNode) -> Vec<TypeEntry> {
    let mut out = Vec::new();
    collect_types_recursive(root, &mut out);
    out
}

fn collect_types_recursive(node: &SymbolNode, out: &mut Vec<TypeEntry>) {
    if let SymbolKind::Item { ref label } = node.id.kind {
        if matches!(
            label.as_str(),
            "class" | "struct" | "trait" | "interface" | "enum" | "impl" | "typedef" | "record"
        ) {
            out.push(TypeEntry {
                id: node.id.clone(),
                name: node.name.clone(),
            });
        }
    }
    for child in &node.children {
        collect_types_recursive(child, out);
    }
}

fn file_path_of(qualified_path: &str) -> &str {
    qualified_path.split("::").next().unwrap_or(qualified_path)
}

pub fn resolve_inheritance(tree: &SymbolTree) -> InheritanceData {
    let all_types = collect_types(&tree.root);
    let name_to_ids: HashMap<&str, Vec<&SymbolId>> = {
        let mut map: HashMap<&str, Vec<&SymbolId>> = HashMap::new();
        for t in &all_types {
            map.entry(&t.name).or_default().push(&t.id);
        }
        map
    };

    let mut edges = Vec::new();
    let mut files_seen: HashMap<String, Option<(Vec<u8>, tree_sitter::Tree)>> = HashMap::new();

    for ty in &all_types {
        let file_rel = file_path_of(&ty.id.qualified_path).to_string();
        let ext = file_rel.rsplit('.').next().unwrap_or("").to_string();

        let file_data = files_seen.entry(file_rel.clone()).or_insert_with(|| {
            let path = tree.repo_root.join(&file_rel);
            let bytes = std::fs::read(&path).ok()?;
            let lang = language_for(&ext)?;
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&lang).ok()?;
            let parsed = parser.parse(&bytes, None)?;
            Some((bytes, parsed))
        });

        let (source, parsed) = match file_data {
            Some((s, p)) => (s.as_slice(), p),
            None => continue,
        };

        let raw = extract_base_names(parsed.root_node(), source, &ext);
        for ri in &raw {
            if ri.child_name != ty.name {
                continue;
            }
            for parent_name in &ri.parent_names {
                // Base names may be namespace-qualified (`live_edit::Baker`,
                // `detail::PropertyBase`); indexed type names are bare. Match
                // on the final segment, then prefer candidates whose
                // qualified path ends with the written qualifier.
                let simple = parent_name.rsplit("::").next().unwrap_or(parent_name);
                let Some(targets) = name_to_ids.get(simple) else {
                    continue;
                };
                let preferred: Vec<&SymbolId> = if parent_name.contains("::") {
                    targets
                        .iter()
                        .copied()
                        .filter(|id| id.qualified_path.ends_with(parent_name.as_str()))
                        .collect()
                } else {
                    Vec::new()
                };
                let chosen: &[&SymbolId] = if preferred.is_empty() {
                    targets
                } else {
                    &preferred
                };
                for &target_id in chosen {
                    if *target_id == ty.id {
                        continue;
                    }
                    edges.push(InheritanceEdge {
                        child: ty.id.clone(),
                        parent: target_id.clone(),
                        parent_name: parent_name.clone(),
                    });
                }
            }
        }
    }

    InheritanceData { edges }
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
    fn cpp_base_classes() {
        let src = r#"
class Base {};
class Derived : public Base {};
class Multi : public Base, private Other {};
"#;
        let (bytes, tree) = make_tree(src, "cpp");
        let raw = extract_base_names(tree.root_node(), &bytes, "cpp");
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[0].child_name, "Derived");
        assert!(raw[0].parent_names.contains(&"Base".to_string()));
        assert_eq!(raw[1].child_name, "Multi");
        assert!(raw[1].parent_names.contains(&"Base".to_string()));
    }

    #[test]
    fn cpp_qualified_and_final_bases_resolve_end_to_end() {
        // Real-world shapes that used to be missed: namespace-qualified
        // bases, `final` before the colon, and a `.h` header (C++ sniffed).
        let dir = std::env::temp_dir().join(format!("outrider-inh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/ifaces.h"),
            "namespace live_edit {\nclass Baker {\npublic:\n  virtual ~Baker() = default;\n};\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/prod.h"),
            "#include \"ifaces.h\"\nclass ProdBaker final : public live_edit::Baker {};\nstruct Other : live_edit::Baker {};\n",
        )
        .unwrap();
        let tree = crate::index_repo(&dir, &[], &[]).unwrap();
        let data = resolve_inheritance(&tree);
        let mut pairs: Vec<(String, String)> = data
            .edges
            .iter()
            .map(|e| {
                let last = |p: &str| p.rsplit("::").next().unwrap_or(p).to_string();
                (last(&e.child.qualified_path), last(&e.parent.qualified_path))
            })
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("Other".to_string(), "Baker".to_string()),
                ("ProdBaker".to_string(), "Baker".to_string()),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rust_impl_trait() {
        let src = r#"
trait Drawable {}
struct Circle {}
impl Drawable for Circle {}
"#;
        let (bytes, tree) = make_tree(src, "rs");
        let raw = extract_base_names(tree.root_node(), &bytes, "rs");
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].child_name, "Circle");
        assert!(raw[0].parent_names.contains(&"Drawable".to_string()));
    }

    #[test]
    fn python_inheritance() {
        let src = r#"
class Animal:
    pass
class Dog(Animal):
    pass
"#;
        let (bytes, tree) = make_tree(src, "py");
        let raw = extract_base_names(tree.root_node(), &bytes, "py");
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].child_name, "Dog");
        assert!(raw[0].parent_names.contains(&"Animal".to_string()));
    }

    #[test]
    fn typescript_extends() {
        let src = r#"
class Base {}
class Derived extends Base {}
"#;
        let (bytes, tree) = make_tree(src, "ts");
        let raw = extract_base_names(tree.root_node(), &bytes, "ts");
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].child_name, "Derived");
        assert!(raw[0].parent_names.contains(&"Base".to_string()));
    }

    #[test]
    fn csharp_base_list() {
        let src = r#"
class Base {}
class Derived : Base {}
"#;
        let (bytes, tree) = make_tree(src, "cs");
        let raw = extract_base_names(tree.root_node(), &bytes, "cs");
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].child_name, "Derived");
        assert!(raw[0].parent_names.contains(&"Base".to_string()));
    }
}
