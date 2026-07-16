# GLSL and HLSL Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Structurally index and syntax-highlight recognized GLSL and HLSL shader files.

**Architecture:** A shared `SourceLanguage::for_path` classifier will be the sole path-to-language decision for indexing and buffer highlighting. Dedicated Tree-sitter GLSL/HLSL grammars will feed focused structural extractors; GLSL uses its bundled highlight query and HLSL uses a project-owned query because its crate exports none.

**Tech Stack:** Rust 2021, tree-sitter 0.26.10, tree-sitter-glsl 0.2.0, tree-sitter-hlsl 0.2.0, anyhow, ropey, Cargo tests.

## Global Constraints

- GLSL extensions are `.glsl`, `.vert`, `.frag`, `.geom`, `.comp`, `.tesc`, and `.tese`, matched case-insensitively.
- HLSL extensions are `.hlsl`, `.fx`, and `.fxh`, matched case-insensitively.
- `.cs` remains C# and `.vs` remains unsupported; do not add content sniffing.
- Parse functions tolerate Tree-sitter error nodes and retain valid surrounding symbols.
- Do not add shader-specific colors, semantic compilation, include resolution, macro expansion, or cross-file resolution.
- Preserve unrelated changes on `main`; all implementation occurs in `D:\Dev\outrider-ide\.worktrees\glsl-hlsl-support`.

---

### Task 1: Shared source-language classification and grammar dependencies

**Files:**
- Create: `crates/outrider-index/src/language.rs`
- Modify: `crates/outrider-index/src/lib.rs`
- Modify: `crates/outrider-index/Cargo.toml`
- Modify: `Cargo.lock`

**Interfaces:**
- Produces: `pub enum SourceLanguage` and `pub fn SourceLanguage::for_path(path: &Path) -> Option<Self>`.
- Produces: grammar crates used by parsing and highlighting tasks.

- [ ] **Step 1: Write classifier tests**

Create `language.rs` with a test-only expectation for the complete enum:

```rust
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLanguage {
    Rust, C, Cpp, Python, JavaScript, TypeScript, Tsx, CSharp,
    Markdown, Toml, Glsl, Hlsl,
}

impl SourceLanguage {
    pub fn for_path(_path: &Path) -> Option<Self> { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_extensions_are_deterministic_and_case_insensitive() {
        for ext in ["glsl", "vert", "frag", "geom", "comp", "tesc", "tese"] {
            assert_eq!(SourceLanguage::for_path(Path::new(&format!("shader.{ext}"))), Some(SourceLanguage::Glsl));
            assert_eq!(SourceLanguage::for_path(Path::new(&format!("shader.{}", ext.to_uppercase()))), Some(SourceLanguage::Glsl));
        }
        for ext in ["hlsl", "fx", "fxh"] {
            assert_eq!(SourceLanguage::for_path(Path::new(&format!("shader.{ext}"))), Some(SourceLanguage::Hlsl));
        }
        assert_eq!(SourceLanguage::for_path(Path::new("shader.vert.hlsl")), Some(SourceLanguage::Hlsl));
        assert_eq!(SourceLanguage::for_path(Path::new("shader.cs")), Some(SourceLanguage::CSharp));
        assert_eq!(SourceLanguage::for_path(Path::new("shader.vs")), None);
    }
}
```

- [ ] **Step 2: Run the classifier test and verify failure**

Run: `cargo test -p outrider-index language::tests::shader_extensions_are_deterministic_and_case_insensitive -- --exact`

Expected: FAIL because `for_path` returns `None`.

- [ ] **Step 3: Implement the classifier and export it**

Implement lowercase final-extension matching:

```rust
pub fn for_path(path: &Path) -> Option<Self> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
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
```

Add `pub mod language; pub use language::SourceLanguage;` to `lib.rs`. Add `tree-sitter-glsl = "0.2.0"` and `tree-sitter-hlsl = "0.2.0"` beside existing grammar dependencies.

- [ ] **Step 4: Verify classifier and grammar ABI compatibility**

Run: `cargo test -p outrider-index language::tests -- --nocapture`

Run: `cargo check -p outrider-index`

Expected: both exit 0; `Cargo.lock` records both 0.2.0 crates.

- [ ] **Step 5: Commit**

```powershell
git add crates/outrider-index/src/language.rs crates/outrider-index/src/lib.rs crates/outrider-index/Cargo.toml Cargo.lock
git commit -m "feat(index): classify GLSL and HLSL paths"
```

### Task 2: GLSL and HLSL structural parsers

**Files:**
- Modify: `crates/outrider-index/src/parse.rs`

**Interfaces:**
- Produces: `pub fn parse_glsl_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>`.
- Produces: `pub fn parse_hlsl_items(source: &[u8]) -> anyhow::Result<Vec<RawItem>>`.
- Consumes: `tree_sitter_glsl::LANGUAGE_GLSL`, `tree_sitter_hlsl::LANGUAGE_HLSL`, and existing `collect_items` helpers.

- [ ] **Step 1: Add failing GLSL parser tests**

Add a representative source test containing `struct Light`, `uniform Scene { ... } scene;`, `out VertexData { ... } vertex;`, and `void main()`. Assert item labels/names include `struct:Light`, container names `Scene` and `VertexData`, and `fn:main`; assert ranges slice to the corresponding declarations. Add malformed text before `void recover() {}` and assert `recover` remains present.

- [ ] **Step 2: Run GLSL tests and verify failure**

Run: `cargo test -p outrider-index parse::tests::glsl -- --nocapture`

Expected: FAIL because `parse_glsl_items` does not exist.

- [ ] **Step 3: Implement GLSL extraction**

Load `LANGUAGE_GLSL`; classify `function_definition` as `fn`, named `struct_specifier` as `struct`, and declarations containing a named block with `uniform`, `buffer`, `in`, or `out` as `interface`. Name functions through the declarator's identifier, structs through their `name` field/type identifier, and interface blocks through the first type identifier preceding the field list. Reuse `collect_items`; skip an accepted node when its extracted name is empty.

- [ ] **Step 4: Verify GLSL parser tests pass**

Run: `cargo test -p outrider-index parse::tests::glsl -- --nocapture`

Expected: all GLSL parser tests pass.

- [ ] **Step 5: Add failing HLSL parser tests**

Add source containing `struct VSInput`, `cbuffer Camera : register(b0) { ... };`, and `float4 main(VSInput input) : SV_Target { ... }`. Assert `struct:VSInput`, `cbuffer:Camera`, and `fn:main`, including source ranges. Add malformed text before `float4 recover() : SV_Target { return 0; }` and assert recovery. Add a grammar-capability test documenting that 0.2.0 does not expose `tbuffer`, `technique`, or `pass` named nodes; this keeps those approved containers conditional as specified.

- [ ] **Step 6: Run HLSL tests and verify failure**

Run: `cargo test -p outrider-index parse::tests::hlsl -- --nocapture`

Expected: FAIL because `parse_hlsl_items` does not exist.

- [ ] **Step 7: Implement HLSL extraction**

Load `LANGUAGE_HLSL`; classify `function_definition` as `fn`, named `struct_specifier` as `struct`, and `cbuffer_specifier` as `cbuffer`. Extract the identifier owned by each declarator/specifier, retain complete syntax-node byte ranges, and skip anonymous accepted nodes. Do not invent text scanning for containers absent from the grammar.

- [ ] **Step 8: Verify parser module**

Run: `cargo test -p outrider-index parse::tests -- --nocapture`

Expected: all parser tests pass.

- [ ] **Step 9: Commit**

```powershell
git add crates/outrider-index/src/parse.rs
git commit -m "feat(index): parse GLSL and HLSL symbols"
```

### Task 3: Index dispatch and end-to-end retention

**Files:**
- Modify: `crates/outrider-index/src/index.rs`
- Modify: `crates/outrider-index/tests/index_test.rs`

**Interfaces:**
- Consumes: `SourceLanguage::for_path`, `parse_glsl_items`, and `parse_hlsl_items`.
- Produces: shader `IndexedFile` entries with ordinary `SymbolNode` children.

- [ ] **Step 1: Add failing end-to-end tests**

Create temporary `.vert`, `.comp`, `.hlsl`, and `.fx` files with the Task 2 snippets. Run `index_repo`, locate each file node, and assert GLSL `main`/interface children and HLSL `main`/`Camera` children. Add `.vs` and verify it remains an unsupported flat file; add `.cs` and verify C# parsing remains active.

- [ ] **Step 2: Run end-to-end tests and verify failure**

Run: `cargo test -p outrider-index --test index_test shader -- --nocapture`

Expected: shader files have no parsed children.

- [ ] **Step 3: Replace extension parser dispatch with shared language dispatch**

In `materialize_file`, compute `let language = SourceLanguage::for_path(path);`. Change `parser_for` to accept `Option<SourceLanguage>` and match all existing parser languages plus `Glsl` and `Hlsl`. Retain Markdown/TOML and the existing plain-text extensions without parsing. Key Rust/Python documentation extraction from the enum rather than raw extension.

- [ ] **Step 4: Verify indexing and existing language regressions**

Run: `cargo test -p outrider-index index::tests -- --nocapture`

Run: `cargo test -p outrider-index --test index_test -- --nocapture`

Expected: both commands exit 0.

- [ ] **Step 5: Commit**

```powershell
git add crates/outrider-index/src/index.rs crates/outrider-index/tests/index_test.rs
git commit -m "feat(index): index GLSL and HLSL files"
```

### Task 4: Shader syntax highlighting through shared dispatch

**Files:**
- Modify: `crates/outrider-index/src/buffer.rs`
- Modify: `crates/outrider/src/buffers.rs`

**Interfaces:**
- Changes: `FileBuffer::new(text: String, path: &Path) -> anyhow::Result<Self>`.
- Consumes: `SourceLanguage::for_path`, GLSL bundled `HIGHLIGHTS_QUERY`, and project-owned `HLSL_HIGHLIGHTS`.

- [ ] **Step 1: Add failing highlight and path tests**

Add table-driven GLSL tests for all seven extensions and HLSL tests for `.hlsl`, `.fx`, and `.fxh`. Assert comments, numeric literals, types, function names, and preprocessor directives receive mapped spans; assert every span is in bounds and non-overlapping. Assert `.vs` produces no spans and `.cs` still highlights as C#. Update app buffer-manager tests to pass a path and verify a `.frag` buffer materializes with spans.

- [ ] **Step 2: Run shader highlight tests and verify failure**

Run: `cargo test -p outrider-index buffer::tests::shader -- --nocapture`

Expected: FAIL because shader extensions are not dispatched and `FileBuffer::new` accepts an extension.

- [ ] **Step 3: Add the HLSL project highlight query**

Define `HLSL_HIGHLIGHTS` over stable HLSL 0.2.0 nodes: comments, string/char/number literals, preprocessor directives, primitive/type identifiers, function declarators/calls, field identifiers, semantics, and keywords including `cbuffer`, `struct`, control flow, storage qualifiers, and register-related syntax. Use capture names already accepted by `kind_for`; add `variable`/`label` mappings only if tests require a natural existing palette category.

- [ ] **Step 4: Move `FileBuffer` to shared path classification**

Match `SourceLanguage` to existing grammar/query pairs, adding GLSL with `LANGUAGE_GLSL` plus its bundled query and HLSL with `LANGUAGE_HLSL` plus `HLSL_HIGHLIGHTS`. Change `BufferManager::get` to pass the repository-relative path instead of extracting an extension. Update all `FileBuffer::new` call sites and tests to use `Path::new("file.rs")`-style inputs.

- [ ] **Step 5: Verify highlighting and buffer integration**

Run: `cargo test -p outrider-index buffer::tests -- --nocapture`

Run: `cargo test -p outrider buffers::tests -- --nocapture`

Expected: both commands exit 0; shader spans are sorted, non-overlapping, and mapped to existing palette kinds.

- [ ] **Step 6: Commit**

```powershell
git add crates/outrider-index/src/buffer.rs crates/outrider/src/buffers.rs
git commit -m "feat: highlight GLSL and HLSL source"
```

### Task 5: Documentation and full verification

**Files:**
- Modify: `README.md`

**Interfaces:**
- Produces: user-visible supported-language documentation matching the classifier.

- [ ] **Step 1: Update supported-language documentation**

Add GLSL and HLSL to the syntax-highlighting feature list and document the accepted extensions. State that `.cs` remains C# and ambiguous `.vs` is unsupported.

- [ ] **Step 2: Format and inspect the final diff**

Run: `cargo fmt --all -- --check`

Run: `git diff --check`

Expected: both exit 0.

- [ ] **Step 3: Run focused shader verification**

Run: `cargo test -p outrider-index shader -- --nocapture`

Expected: all classifier, parser, indexing, and highlighting shader tests pass.

- [ ] **Step 4: Run full workspace verification**

Run: `cargo test --workspace`

Run: `cargo build --workspace`

Expected: both exit 0 with no test failures; pre-existing warnings may remain.

- [ ] **Step 5: Commit documentation**

```powershell
git add README.md
git commit -m "docs: document shader language support"
```

- [ ] **Step 6: Review requirements against the approved spec**

Confirm every supported extension, both structural parsers, shader containers supported by the grammars, malformed-source recovery, highlight categories, ambiguous-extension behavior, end-to-end indexing, and full regression verification are represented by passing tests.

