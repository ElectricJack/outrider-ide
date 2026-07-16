# GLSL and HLSL Parsing and Highlighting Design

**Date:** 2026-07-16

**Scope:** Add structural parsing and syntax highlighting for the GLSL and HLSL shader families.

## Goals

- Retain and structurally index recognized GLSL and HLSL source files.
- Show functions, structs, and shader-specific containers in the treemap hierarchy.
- Syntax-highlight both languages through Outrider's existing `HighlightKind` palette.
- Keep language selection deterministic and avoid extensions that conflict with existing languages.

## Language detection

Language selection remains path-based and case-insensitive. GLSL owns `.glsl`, `.vert`, `.frag`, `.geom`, `.comp`, `.tesc`, and `.tese`. HLSL owns `.hlsl`, `.fx`, and `.fxh`.

Ambiguous short extensions are intentionally excluded. In particular, `.cs` remains C#, and `.vs` is not assigned because it is used for both GLSL and HLSL. Compound names such as `shader.vert.hlsl` select HLSL from the final extension.

The path-to-language decision must be shared by structural indexing and buffer highlighting so the two features cannot drift. If the in-progress Makefile work introduces `SourceLanguage::for_path`, shader variants will extend that classifier; otherwise this feature will introduce the equivalent shared boundary without overwriting unrelated work.

## Parsing architecture

Use dedicated Tree-sitter grammars for GLSL and HLSL rather than treating shader code as C or C++. Each language receives a parser entry point following the existing functions in `parse.rs` and returning `RawItem` values.

GLSL extraction includes:

- function definitions;
- struct specifiers with names;
- named interface or uniform blocks, including their nested declarations when the grammar exposes stable ranges.

HLSL extraction includes:

- function definitions;
- structs and classes where accepted by the grammar;
- constant and texture buffers (`cbuffer` and `tbuffer`);
- named technique/pass-style containers exposed by the selected grammar.

Items use the complete syntax-node byte range, a concise source-derived signature, and labels consistent with existing `SymbolKind::Item` conventions (`fn`, `struct`, `interface`, `cbuffer`, `tbuffer`, `technique`, and `pass`). Anonymous declarations are skipped unless they contain named structural children that can be safely promoted.

Tree-sitter error nodes are tolerated. Valid structural nodes surrounding malformed code are still returned; a parse failure at the API boundary remains an error with language-specific context.

## Highlighting

`FileBuffer` uses the same language classifier as indexing. Each grammar's maintained highlight query is preferred. If a Rust grammar crate does not export a complete query, Outrider will own a small query derived from stable named nodes in that grammar.

Capture names flow through the current `kind_for` mapping. Shader-specific captures such as attributes, built-ins, preprocessor directives, semantics, and variables are mapped only when they have a natural existing palette category; punctuation and unmapped captures remain default-colored. No theme changes are required unless testing demonstrates that a required capture category has no sensible existing representation.

## Dependencies

Add compatible Rust grammar crates for GLSL and HLSL to `outrider-index`. Exact versions are selected during implementation against the repository's pinned `tree-sitter` runtime and recorded in `Cargo.lock`. A grammar is accepted only if its language ABI loads under the current runtime and representative GLSL/HLSL snippets compile their highlight queries.

## Data flow

1. The scanner discovers a shader path.
2. The shared classifier maps the path to GLSL or HLSL.
3. Index materialization retains the bounded file contents and invokes the matching structural parser.
4. Parsed `RawItem` values become ordinary `SymbolNode` children.
5. On display, `FileBuffer` selects the same grammar and produces per-line highlight spans and minimap colors.
6. Unsupported or ambiguous extensions continue through the existing plain/unsupported behavior.

## Testing

- Table-driven classifier tests cover every accepted extension, uppercase variants, compound HLSL names, `.cs`, `.vs`, and unknown paths.
- GLSL parser tests cover functions, structs, interface/uniform blocks, nesting, byte ranges, and malformed surrounding source.
- HLSL parser tests cover functions, structs, `cbuffer`, `tbuffer`, technique/pass containers when supported by the grammar, byte ranges, and malformed surrounding source.
- Highlight tests assert representative keywords, comments, types, functions, strings/numbers, and preprocessor directives where the query exposes them; all spans must remain sorted, non-overlapping, and within line bounds.
- End-to-end index tests confirm recognized shader files are retained and expose structural children.
- Existing C#, C/C++, and other language tests guard against extension regressions.
- The full workspace test suite and formatting checks run before completion.

## Non-goals

- WGSL, Metal Shading Language, CUDA, OpenCL, Godot shaders, or shader binary formats.
- Semantic compilation, include resolution, macro expansion, entry-point validation, or cross-file symbol resolution.
- Heuristic content sniffing for ambiguous extensions.
- New syntax colors or shader-specific UI.

