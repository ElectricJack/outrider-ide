use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLanguage {
    Rust,
    C,
    Cpp,
    Markdown,
    Toml,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    CSharp,
    Make,
    Glsl,
    Hlsl,
}

impl SourceLanguage {
    pub fn for_path(path: &Path) -> Option<Self> {
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("Makefile" | "makefile" | "GNUmakefile")
        ) {
            return Some(Self::Make);
        }

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
            "mk" => Self::Make,
            "glsl" | "vert" | "frag" | "geom" | "comp" | "tesc" | "tese" | "rgen"
            | "rchit" => Self::Glsl,
            "hlsl" | "fx" | "fxh" => Self::Hlsl,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::SourceLanguage;

    #[test]
    fn recognizes_make_paths() {
        for path in ["Makefile", "makefile", "GNUmakefile", "build/rules.mk"] {
            assert_eq!(
                SourceLanguage::for_path(Path::new(path)),
                Some(SourceLanguage::Make),
                "expected {path} to be recognized as Make"
            );
        }
    }

    #[test]
    fn rejects_make_lookalikes() {
        for path in ["Makefile.txt", "GNUMakefile", "MAKEFILE", "rules.mk.bak"] {
            assert_ne!(
                SourceLanguage::for_path(Path::new(path)),
                Some(SourceLanguage::Make),
                "expected {path} not to be recognized as Make"
            );
        }
    }

    #[test]
    fn shader_extensions_are_deterministic_and_case_insensitive() {
        for ext in [
            "glsl", "vert", "frag", "geom", "comp", "tesc", "tese", "rgen", "rchit",
        ] {
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
