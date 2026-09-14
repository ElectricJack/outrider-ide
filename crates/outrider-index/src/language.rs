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

/// Heuristic for ambiguous `.h` headers: true when the source uses C++-only
/// constructs. Checks a bounded prefix so huge headers stay cheap. Tokens are
/// matched at word boundaries to avoid false hits inside identifiers.
pub fn looks_like_cpp(bytes: &[u8]) -> bool {
    const LIMIT: usize = 64 * 1024;
    let head = &bytes[..bytes.len().min(LIMIT)];
    let text = String::from_utf8_lossy(head);
    const MARKERS: [&str; 8] = [
        "class ", "namespace ", "template<", "template <", "public:", "private:", "protected:",
        "::",
    ];
    for line in text.lines() {
        let t = line.trim_start();
        // Skip comment lines; `::` inside prose is common.
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            continue;
        }
        if t.starts_with("#include") && (t.contains('<') && !t.contains(".h")) {
            // `#include <vector>` style (no extension) is C++ standard library.
            return true;
        }
        for m in MARKERS {
            if let Some(pos) = t.find(m) {
                // Word-boundary check for keyword markers; `::` is an
                // operator and legitimately follows an identifier.
                let boundary_ok = m == "::"
                    || pos == 0
                    || !t.as_bytes()[pos - 1].is_ascii_alphanumeric()
                        && t.as_bytes()[pos - 1] != b'_';
                if boundary_ok {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{looks_like_cpp, SourceLanguage};

    #[test]
    fn sniffs_cpp_headers() {
        assert!(looks_like_cpp(b"namespace foo {\nclass Bar {};\n}\n"));
        assert!(looks_like_cpp(b"#include <vector>\nstruct S { int x; };\n"));
        assert!(looks_like_cpp(b"struct S {\npublic:\n  int x;\n};\n"));
        assert!(looks_like_cpp(b"template <typename T>\nstruct S {};\n"));
        assert!(looks_like_cpp(b"void f(std::string s);\n"));
    }

    #[test]
    fn plain_c_headers_stay_c() {
        assert!(!looks_like_cpp(b"#include <stdio.h>\nstruct S { int x; };\nvoid f(void);\n"));
        assert!(!looks_like_cpp(b"// a class of problems\nint subclass_count;\n"));
        assert!(!looks_like_cpp(b"typedef struct { int a; } my_namespace_t;\n"));
    }

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
