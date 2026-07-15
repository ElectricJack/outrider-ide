use std::collections::{HashMap, HashSet};
use std::ops::Range;

use tree_sitter::Node;

use super::{TypeEnv, TypeResolver};

pub(super) struct PythonTypeResolver;

impl TypeResolver for PythonTypeResolver {
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

        if let Some(params) = func.child_by_field_name("parameters") {
            bind_parameters(&mut env, params, source, enclosing_class);
        }

        walk_body_for_bindings(tree.root_node(), source, &scope_range, class_names, fn_return_types, &mut env);
        env
    }
}

fn find_function_at<'a>(node: Node<'a>, range: &Range<usize>) -> Option<Node<'a>> {
    if node.kind() == "function_definition"
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

fn bind_parameters(env: &mut TypeEnv, params: Node, source: &[u8], enclosing_class: Option<&str>) {
    let mut first = true;
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        match param.kind() {
            "identifier" => {
                if first {
                    if let Some(class) = enclosing_class {
                        env.bind(node_text(param, source), class.to_string());
                    }
                }
            }
            "typed_parameter" => {
                let name_node = param
                    .named_children(&mut param.walk())
                    .find(|c| c.kind() == "identifier");
                if first {
                    if let Some(class) = enclosing_class {
                        if let Some(n) = name_node {
                            env.bind(node_text(n, source), class.to_string());
                        }
                    }
                }
                if let (Some(n), Some(t)) = (name_node, param.child_by_field_name("type")) {
                    env.bind(node_text(n, source), node_text(t, source));
                }
            }
            "typed_default_parameter" => {
                if let (Some(n), Some(t)) = (
                    param.child_by_field_name("name"),
                    param.child_by_field_name("type"),
                ) {
                    if first {
                        if let Some(class) = enclosing_class {
                            env.bind(node_text(n, source), class.to_string());
                        }
                    }
                    env.bind(node_text(n, source), node_text(t, source));
                }
            }
            _ => {}
        }
        first = false;
    }
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
    if node.kind() == "assignment" && node.start_byte() >= range.start {
        let left = node.child_by_field_name("left");
        let right = node.child_by_field_name("right");
        let type_ann = node.child_by_field_name("type");

        if let Some(left) = left {
            if left.kind() == "identifier" {
                let var = node_text(left, source);

                if let Some(ty) = type_ann {
                    env.bind(var.clone(), node_text(ty, source));
                }

                if let Some(right) = right {
                    if right.kind() == "call" {
                        if let Some(func) = right.child_by_field_name("function") {
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

    fn parse(source: &str) -> (Vec<u8>, tree_sitter::Tree) {
        let bytes = source.as_bytes().to_vec();
        let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(&bytes, None).unwrap();
        (bytes, tree)
    }

    fn class_names(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn self_binds_to_enclosing_class() {
        let src = "class Dog:\n    def bark(self):\n        self.speak()\n";
        let (bytes, tree) = parse(src);
        let func_start = src.find("def bark").unwrap();
        let range = func_start..src.len();
        let resolver = PythonTypeResolver;
        let env =
            resolver.build_scope_types(&bytes, &tree, range, Some("Dog"), &HashSet::new(), &HashMap::new());
        assert_eq!(env.resolve("self"), Some("Dog"));
    }

    #[test]
    fn typed_parameter_binds() {
        let src = "def f(x: Foo):\n    x.bar()\n";
        let (bytes, tree) = parse(src);
        let range = 0..src.len();
        let resolver = PythonTypeResolver;
        let env = resolver.build_scope_types(&bytes, &tree, range, None, &HashSet::new(), &HashMap::new());
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn typed_default_parameter_binds() {
        let src = "def f(x: Foo = None):\n    x.bar()\n";
        let (bytes, tree) = parse(src);
        let range = 0..src.len();
        let resolver = PythonTypeResolver;
        let env = resolver.build_scope_types(&bytes, &tree, range, None, &HashSet::new(), &HashMap::new());
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn constructor_assignment_binds() {
        let src = "def f():\n    x = Foo()\n    x.bar()\n";
        let (bytes, tree) = parse(src);
        let range = 0..src.len();
        let resolver = PythonTypeResolver;
        let env = resolver.build_scope_types(&bytes, &tree, range, None, &class_names(&["Foo"]), &HashMap::new());
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn annotated_assignment_binds() {
        let src = "def f():\n    x: Foo = get_foo()\n";
        let (bytes, tree) = parse(src);
        let range = 0..src.len();
        let resolver = PythonTypeResolver;
        let env = resolver.build_scope_types(&bytes, &tree, range, None, &HashSet::new(), &HashMap::new());
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn first_binding_wins() {
        let src = "def f():\n    x = Foo()\n    x = Bar()\n";
        let (bytes, tree) = parse(src);
        let range = 0..src.len();
        let resolver = PythonTypeResolver;
        let env =
            resolver.build_scope_types(&bytes, &tree, range, None, &class_names(&["Foo", "Bar"]), &HashMap::new());
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn non_class_call_does_not_bind() {
        let src = "def f():\n    x = helper()\n";
        let (bytes, tree) = parse(src);
        let range = 0..src.len();
        let resolver = PythonTypeResolver;
        let env = resolver.build_scope_types(&bytes, &tree, range, None, &HashSet::new(), &HashMap::new());
        assert_eq!(env.resolve("x"), None);
    }

    #[test]
    fn no_enclosing_class_no_self_binding() {
        let src = "def f(self):\n    pass\n";
        let (bytes, tree) = parse(src);
        let range = 0..src.len();
        let resolver = PythonTypeResolver;
        let env = resolver.build_scope_types(&bytes, &tree, range, None, &HashSet::new(), &HashMap::new());
        assert_eq!(env.resolve("self"), None);
    }

    fn fn_returns(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn fn_return_type_binds() {
        let src = "def f():\n    x = get_dog()\n    x.bark()\n";
        let (bytes, tree) = parse(src);
        let range = 0..src.len();
        let resolver = PythonTypeResolver;
        let env = resolver.build_scope_types(
            &bytes, &tree, range, None, &HashSet::new(),
            &fn_returns(&[("get_dog", "Dog")]),
        );
        assert_eq!(env.resolve("x"), Some("Dog"));
    }

    #[test]
    fn fn_return_type_does_not_override_class_constructor() {
        let src = "def f():\n    x = Foo()\n";
        let (bytes, tree) = parse(src);
        let range = 0..src.len();
        let resolver = PythonTypeResolver;
        let env = resolver.build_scope_types(
            &bytes, &tree, range, None, &class_names(&["Foo"]),
            &fn_returns(&[("Foo", "SomethingElse")]),
        );
        assert_eq!(env.resolve("x"), Some("Foo"));
    }

    #[test]
    fn self_typed_param_class_wins_over_annotation() {
        let src = "class Dog:\n    def bark(self: Any):\n        pass\n";
        let (bytes, tree) = parse(src);
        let func_start = src.find("def bark").unwrap();
        let range = func_start..src.len();
        let resolver = PythonTypeResolver;
        let env =
            resolver.build_scope_types(&bytes, &tree, range, Some("Dog"), &HashSet::new(), &HashMap::new());
        assert_eq!(env.resolve("self"), Some("Dog"));
    }
}
