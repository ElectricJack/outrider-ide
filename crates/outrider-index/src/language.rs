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
}

impl SourceLanguage {
    pub fn for_path(path: &Path) -> Option<Self> {
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("Makefile" | "makefile" | "GNUmakefile")
        ) {
            return Some(Self::Make);
        }

        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "rs" => Some(Self::Rust),
            "c" | "h" => Some(Self::C),
            "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Some(Self::Cpp),
            "md" => Some(Self::Markdown),
            "toml" => Some(Self::Toml),
            "py" => Some(Self::Python),
            "js" | "jsx" => Some(Self::JavaScript),
            "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "cs" => Some(Self::CSharp),
            "mk" => Some(Self::Make),
            _ => None,
        }
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
}
