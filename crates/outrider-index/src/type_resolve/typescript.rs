use std::collections::{HashMap, HashSet};
use std::ops::Range;

use tree_sitter::Node;

use super::{TypeEnv, TypeResolver};

pub(super) struct TypeScriptTypeResolver;

impl TypeResolver for TypeScriptTypeResolver {
    fn build_scope_types(
        &self,
        source: &[u8],
        tree: &tree_sitter::Tree,
        scope_range: Range<usize>,
        enclosing_class: Option<&str>,
        class_names: &HashSet<String>,
        fn_return_types: &HashMap<String, String>,
    ) -> TypeEnv {
        let mut env = TypeEnv::default();
        let Some(func) = find_function_at(tree.root_node(), &scope_range) else {
            return env;
        };

        if let Some(class) = enclosing_class {
            env.bind("this".to_string(), class.to_string());
        }

        if let Some(params) = func.child_by_field_name("parameters") {
            bind_parameters(&mut env, params, source);
        }

        walk_body_for_bindings(
            tree.root_node(),
            source,
            &scope_range,
            class_names,
            fn_return_types,
            &mut env,
        );
        env
    }
}

const FUNCTION_KINDS: &[&str] = &[
    "function_declaration",
    "method_definition",
    "arrow_function",
    "function",
    "generator_function_declaration",
    "generator_function",
];

fn find_function_at<'a>(node: Node<'a>, range: &Range<usize>) -> Option<Node<'a>> {
    if node.is_named()
        && FUNCTION_KINDS.contains(&node.kind())
        && node.start_byte() <= range.start
        && node.end_byte() > range.start
    {
        let mut best = Some(node);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(inner) = find_function_at(child, range) {
                best = Some(inner);
            }
        }
        return best;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let result @ Some(_) = find_function_at(child, range) {
            return result;
        }
    }
    None
}

fn bind_parameters(env: &mut TypeEnv, params: Node, source: &[u8]) {
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        match param.kind() {
            "required_parameter" | "optional_parameter" => {
                let name_node = find_child_by_kind(param, "identifier");
                let type_node = find_child_by_kind(param, "type_annotation");
                if let (Some(n), Some(t)) = (name_node, type_node) {
                    if let Some(type_name) = extract_type_name(t, source) {
                        env.bind(node_text(n, source), type_name);
                    }
                }
            }
            _ => {}
        }
    }
}

fn find_child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    for i in 0..node.named_child_count() {
        if let Some(child) = node.named_child(i as u32) {
            if child.kind() == kind {
                return Some(child);
            }
        }
    }
    None
}

fn extract_type_name(type_ann: Node, source: &[u8]) -> Option<String> {
    let mut cursor = type_ann.walk();
    for child in type_ann.named_children(&mut cursor) {
        if child.kind() == "type_identifier" {
            return Some(node_text(child, source));
        }
    }
    None
}

fn walk_body_for_bindings(
    node: Node,
    source: &[u8],
    range: &Range<usize>,
    class_names: &HashSet<String>,
    fn_return_types: &HashMap<String, String>,
    env: &mut TypeEnv,
) {
    if node.end_byte() <= range.start || node.start_byte() >= range.end {
        return;
    }

    if node.kind() == "variable_declarator" && node.start_byte() >= range.start {
        let name = node.child_by_field_name("name");
        let value = node.child_by_field_name("value");
        let type_ann = node.child_by_field_name("type");

        if let Some(name) = name {
            if name.kind() == "identifier" {
                let var = node_text(name, source);

                if let Some(ty) = type_ann {
                    if let Some(type_name) = extract_type_name(ty, source) {
                        env.bind(var.clone(), type_name);
                    }
                }

                if let Some(val) = value {
                    if val.kind() == "new_expression" {
                        if let Some(constructor) = val.child_by_field_name("constructor") {
                            if constructor.kind() == "identifier" {
                                let callee = node_text(constructor, source);
                                if class_names.contains(&callee) {
                                    env.bind(var, callee);
                                }
                            }
                        }
                    } else if val.kind() == "call_expression" {
                        if let Some(func) = val.child_by_field_name("function") {
                            if func.kind() == "identifier" {
                                let callee = node_text(func, source);
                                if class_names.contains(&callee) {
                                    env.bind(var, callee);
                                } else if let Some(ret) = fn_return_types.get(&callee) {
                                    env.bind(var, ret.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_body_for_bindings(child, source, range, class_names, fn_return_types, env);
    }
}

fn node_text(node: Node, source: &[u8]) -> String {
    std::str::from_utf8(&source[node.byte_range()])
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ts(source: &str) -> (Vec<u8>, tree_sitter::Tree) {
        let bytes = source.as_bytes().to_vec();
        let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(&bytes, None).unwrap();
        (bytes, tree)
    }

    fn parse_tsx(source: &str) -> (Vec<u8>, tree_sitter::Tree) {
        let bytes = source.as_bytes().to_vec();
        let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TSX.into();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(&bytes, None).unwrap();
        (bytes, tree)
    }

    fn parse_js(source: &str) -> (Vec<u8>, tree_sitter::Tree) {
        let bytes = source.as_bytes().to_vec();
        let lang: tree_sitter::Language = tree_sitter_javascript::LANGUAGE.into();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(&bytes, None).unwrap();
        (bytes, tree)
    }

    fn class_names(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn this_binds_to_enclosing_class() {
        let src = "class Dog {\n  bark() {\n    this.speak();\n  }\n}\n";
        let (bytes, tree) = parse_ts(src);
        let func_start = src.find("bark()").unwrap();
        let range = func_start..src.find("  }\n}").unwrap() + 3;
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            Some("Dog"),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("this"), Some("Dog"));
    }

    #[test]
    fn no_enclosing_class_no_this_binding() {
        let src = "function foo() {\n  this.bar();\n}\n";
        let (bytes, tree) = parse_ts(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("this"), None);
    }

    #[test]
    fn typed_parameter_binds() {
        let src = "function foo(x: Bar) {\n  x.baz();\n}\n";
        let (bytes, tree) = parse_ts(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("x"), Some("Bar"));
    }

    #[test]
    fn optional_parameter_binds() {
        let src = "function foo(x?: Bar) {\n  x.baz();\n}\n";
        let (bytes, tree) = parse_ts(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("x"), Some("Bar"));
    }

    #[test]
    fn arrow_function_typed_param() {
        let src = "const fn = (svc: Service) => {\n  svc.call();\n};\n";
        let (bytes, tree) = parse_ts(src);
        let arrow_start = src.find("(svc").unwrap();
        let range = arrow_start..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("svc"), Some("Service"));
    }

    #[test]
    fn constructor_assignment_binds() {
        let src = "function foo() {\n  const x = new Foo();\n  x.bar();\n}\n";
        let (bytes, tree) = parse_ts(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &class_names(&["Foo"]),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn type_annotated_variable_binds() {
        let src = "function foo() {\n  const x: Foo = getFoo();\n  x.bar();\n}\n";
        let (bytes, tree) = parse_ts(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn first_binding_wins() {
        let src = "function f() {\n  const x = new Foo();\n  const x = new Bar();\n}\n";
        let (bytes, tree) = parse_ts(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &class_names(&["Foo", "Bar"]),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn non_class_new_does_not_bind() {
        let src = "function f() {\n  const x = new helper();\n}\n";
        let (bytes, tree) = parse_ts(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("x"), None);
    }

    #[test]
    fn tsx_typed_parameter() {
        let src = "function Component(props: MyProps) {\n  props.onClick();\n}\n";
        let (bytes, tree) = parse_tsx(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("props"), Some("MyProps"));
    }

    #[test]
    fn js_constructor_binds() {
        let src = "function foo() {\n  const x = new Foo();\n  x.bar();\n}\n";
        let (bytes, tree) = parse_js(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &class_names(&["Foo"]),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn js_no_type_annotations() {
        let src = "function foo(x) {\n  x.bar();\n}\n";
        let (bytes, tree) = parse_js(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(env.resolve("x"), None);
    }

    fn fn_returns(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn fn_return_type_binds() {
        let src = "function foo() {\n  const x = getDog();\n  x.bark();\n}\n";
        let (bytes, tree) = parse_ts(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &HashSet::new(),
            &fn_returns(&[("getDog", "Dog")]),
        );
        assert_eq!(env.resolve("x"), Some("Dog"));
    }

    #[test]
    fn fn_return_type_does_not_override_class_constructor() {
        let src = "function foo() {\n  const x = new Foo();\n}\n";
        let (bytes, tree) = parse_ts(src);
        let range = 0..src.len();
        let resolver = TypeScriptTypeResolver;
        let env = resolver.build_scope_types(
            &bytes,
            &tree,
            range,
            None,
            &class_names(&["Foo"]),
            &fn_returns(&[("Foo", "SomethingElse")]),
        );
        assert_eq!(env.resolve("x"), Some("Foo"));
    }
}
