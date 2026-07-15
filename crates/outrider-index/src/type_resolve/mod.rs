mod python;
mod typescript;

use std::collections::{HashMap, HashSet};
use std::ops::Range;

#[derive(Debug, Default)]
pub struct TypeEnv {
    bindings: HashMap<String, String>,
}

impl TypeEnv {
    pub fn bind(&mut self, var: String, ty: String) {
        self.bindings.entry(var).or_insert(ty);
    }

    pub fn resolve(&self, var_name: &str) -> Option<&str> {
        self.bindings.get(var_name).map(|s| s.as_str())
    }
}

pub trait TypeResolver {
    fn build_scope_types(
        &self,
        source: &[u8],
        tree: &tree_sitter::Tree,
        scope_range: Range<usize>,
        enclosing_class: Option<&str>,
        class_names: &HashSet<String>,
        fn_return_types: &HashMap<String, String>,
    ) -> TypeEnv;
}

pub fn resolver_for(ext: &str) -> Option<Box<dyn TypeResolver>> {
    match ext {
        "py" => Some(Box::new(python::PythonTypeResolver)),
        "ts" | "tsx" | "js" | "jsx" => Some(Box::new(typescript::TypeScriptTypeResolver)),
        _ => None,
    }
}
