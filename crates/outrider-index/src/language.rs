use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLanguage {
    Rust,
    C,
    Cpp,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    CSharp,
    Markdown,
    Toml,
    Glsl,
    Hlsl,
}

impl SourceLanguage {
    pub fn for_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::for_extension(&ext)
    }

    pub fn for_extension(ext: &str) -> Option<Self> {
        let ext = ext.to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Self::Rust,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Self::Cpp,
            "py" => Self::Python,
            "js" | "jsx" => Self::JavaScript,
            "ts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "cs" => Self::CSharp,
            "md" | "markdown" => Self::Markdown,
            "toml" => Self::Toml,
            "glsl" | "vert" | "frag" | "geom" | "comp" | "tesc" | "tese" => Self::Glsl,
            "hlsl" | "fx" | "fxh" => Self::Hlsl,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_extensions_are_deterministic_and_case_insensitive() {
        for ext in ["glsl", "vert", "frag", "geom", "comp", "tesc", "tese"] {
            assert_eq!(
                SourceLanguage::for_path(Path::new(&format!("shader.{ext}"))),
                Some(SourceLanguage::Glsl)
            );
            assert_eq!(
                SourceLanguage::for_path(Path::new(&format!("shader.{}", ext.to_uppercase()))),
                Some(SourceLanguage::Glsl)
            );
        }
        for ext in ["hlsl", "fx", "fxh"] {
            assert_eq!(
                SourceLanguage::for_path(Path::new(&format!("shader.{ext}"))),
                Some(SourceLanguage::Hlsl)
            );
        }
        assert_eq!(
            SourceLanguage::for_path(Path::new("shader.vert.hlsl")),
            Some(SourceLanguage::Hlsl)
        );
        assert_eq!(
            SourceLanguage::for_path(Path::new("shader.cs")),
            Some(SourceLanguage::CSharp)
        );
        assert_eq!(SourceLanguage::for_path(Path::new("shader.vs")), None);
    }
}
