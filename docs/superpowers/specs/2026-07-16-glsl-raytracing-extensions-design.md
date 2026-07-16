# GLSL Ray-Tracing Extension Design

**Date:** 2026-07-16

Add `.rgen` and `.rchit` to the existing GLSL source-language classification. Both extensions reuse the current GLSL Tree-sitter parser, structural symbol extraction, highlight query, and palette mapping; no new grammar or rendering behavior is introduced.

Update classifier and highlighting coverage for both extensions, and classify them as source code in project settings so they are enabled by default. Preserve `.vs` ambiguity handling and all existing shader mappings.

Verification consists of focused language/highlighting/settings tests followed by the full workspace test suite and build.
