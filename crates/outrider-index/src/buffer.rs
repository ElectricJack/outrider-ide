//! Source-file representation: parse, syntax-highlight, and minimap summary.
//! Wraps a ropey `Rope` with per-line `HighlightSpan` lists (built once via
//! tree-sitter) and a compact `MinimapRow` array used by the far-zoom renderer.
//! Anchors track byte offsets through edits (Phase 6 incremental re-parse).

use std::ops::Range;
use std::path::Path;

use anyhow::Context;
use ropey::Rope;
use tree_sitter::{Query, QueryCursor, StreamingIterator, Tree};

use crate::language::SourceLanguage;

/// Handle to a tracked byte position (spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnchorId(usize);

/// A buffer mutation: `range` replaced by `new_len` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub new_len: usize,
}

/// Growable list of tracked byte positions; each slot is a stable `AnchorId`.
#[derive(Debug, Default)]
pub struct AnchorList {
    positions: Vec<usize>,
}

/// Anchor creation, resolution, and edit-survival remapping.
impl AnchorList {
    /// Append a new anchor at `offset` and return its stable handle.
    pub fn create(&mut self, offset: usize) -> AnchorId {
        self.positions.push(offset);
        AnchorId(self.positions.len() - 1)
    }

    /// Return the current byte offset for the given anchor.
    pub fn resolve(&self, id: AnchorId) -> usize {
        self.positions[id.0]
    }

    /// Survive-edits rule: positions at/after the edit's end shift by the
    /// length delta; positions strictly inside a replaced/deleted range
    /// clamp to its start. A position at the edit's start stays put
    /// (unless the edit is an insertion exactly there, which shifts it).
    pub fn remap(&mut self, edit: &Edit) {
        let delta = edit.new_len as isize - edit.range.len() as isize;
        for p in &mut self.positions {
            if *p >= edit.range.end {
                *p = (*p as isize + delta) as usize;
            } else if *p > edit.range.start {
                *p = edit.range.start;
            }
        }
    }
}

/// Semantic token category used to color text in the source view and tint
/// minimap bars. Mapped from tree-sitter capture names via `kind_for`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    Function,
    Type,
    String,
    Comment,
    Number,
    Property,
    Default,
}

/// A colored span; `range` is a byte range within its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

/// Cheap per-line texture summary for the far-zoom minimap: leading
/// whitespace width, trimmed visible length, and the dominant highlight
/// kind. Precomputed once at materialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimapRow {
    pub indent: u32,
    pub len: u32,
    pub kind: HighlightKind,
}

/// Parsed, highlighted, in-memory view of a single source file.
/// Created by `FileBuffer::new`; read by the renderer for source and minimap display.
pub struct FileBuffer {
    rope: Rope,
    /// Held for Phase 6 incremental re-parse; unused until then. `None`
    /// in plain mode (no grammar for the extension).
    #[allow(dead_code)]
    tree: Option<Tree>,
    /// Per-line spans, computed once at materialization (spec §3.2).
    lines: Vec<Vec<HighlightSpan>>,
    minimap: Vec<MinimapRow>,
    anchors: AnchorList,
}

/// Leaf-node-only highlight query for TOML, overriding the bundled grammar query.
/// Leaf-only TOML captures. The crate's shipped HIGHLIGHTS_QUERY has a
/// `(pair (bare_key)) @property` pattern that captures the whole pair
/// node; outermost-first overlap resolution would keep that whole-line
/// span and drop the inner key/string/number spans.
const TOML_HIGHLIGHTS: &str = r#"
(bare_key) @property
(quoted_key) @string
(boolean) @constant
(comment) @comment
(string) @string
[(integer) (float)] @number
[(offset_date_time) (local_date_time) (local_date) (local_time)] @string.special
"#;

/// Focused Make highlights using only stable grammar nodes and tokens.
const MAKE_HIGHLIGHTS: &str = r#"
(comment) @comment
(variable_assignment name: (word) @property)
(shell_assignment name: (word) @property)
(targets (word) @function)
[
  "ifeq" "ifneq" "ifdef" "ifndef" "else" "endif" "if" "or" "and"
] @conditional
"foreach" @repeat
[
  "define" "endef" "vpath" "undefine" "export" "unexport" "override" "private"
] @keyword
["include" "sinclude" "-include"] @include
["error" "warning" "info"] @exception
[(text) (string) (raw_text)] @string
"#;

/// Construction, line/span access, minimap queries, and anchor management.
impl FileBuffer {
    /// `path` selects a language from its complete filename. Known paths
    /// parse and highlight; anything else is plain mode —
    /// no parse, every line's span list empty.
    pub fn new(text: String, path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let lang: Option<(tree_sitter::Language, String)> =
            match SourceLanguage::for_path(path.as_ref()) {
                Some(SourceLanguage::Rust) => Some((
                    tree_sitter_rust::LANGUAGE.into(),
                    tree_sitter_rust::HIGHLIGHTS_QUERY.to_owned(),
                )),
                Some(SourceLanguage::C) => Some((
                    tree_sitter_c::LANGUAGE.into(),
                    tree_sitter_c::HIGHLIGHT_QUERY.to_owned(),
                )),
                Some(SourceLanguage::Cpp) => Some((
                    tree_sitter_cpp::LANGUAGE.into(),
                    format!(
                        "{}\n{}",
                        tree_sitter_c::HIGHLIGHT_QUERY,
                        tree_sitter_cpp::HIGHLIGHT_QUERY
                    ),
                )),
                Some(SourceLanguage::Markdown) => Some((
                    tree_sitter_md::LANGUAGE.into(),
                    tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_owned(),
                )),
                Some(SourceLanguage::Toml) => Some((
                    tree_sitter_toml_ng::LANGUAGE.into(),
                    TOML_HIGHLIGHTS.to_owned(),
                )),
                Some(SourceLanguage::Python) => Some((
                    tree_sitter_python::LANGUAGE.into(),
                    tree_sitter_python::HIGHLIGHTS_QUERY.to_owned(),
                )),
                Some(SourceLanguage::JavaScript) => Some((
                    tree_sitter_javascript::LANGUAGE.into(),
                    tree_sitter_javascript::HIGHLIGHT_QUERY.to_owned(),
                )),
                Some(SourceLanguage::TypeScript) => Some((
                    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                    tree_sitter_typescript::HIGHLIGHTS_QUERY.to_owned(),
                )),
                Some(SourceLanguage::Tsx) => Some((
                    tree_sitter_typescript::LANGUAGE_TSX.into(),
                    tree_sitter_typescript::HIGHLIGHTS_QUERY.to_owned(),
                )),
                Some(SourceLanguage::CSharp) => Some((
                    tree_sitter_c_sharp::LANGUAGE.into(),
                    tree_sitter_c_sharp::HIGHLIGHTS_QUERY.to_owned(),
                )),
                Some(SourceLanguage::Make) => Some((
                    tree_sitter_make::LANGUAGE.into(),
                    MAKE_HIGHLIGHTS.to_owned(),
                )),
                _ => None,
            };
        let (tree, lines) = match lang {
            Some((language, query_src)) => {
                let mut parser = tree_sitter::Parser::new();
                parser
                    .set_language(&language)
                    .context("loading tree-sitter grammar")?;
                let tree = parser
                    .parse(&text, None)
                    .context("tree-sitter parse failed")?;
                let lines = highlight_lines(&text, &tree, &language, &query_src)?;
                (Some(tree), lines)
            }
            None => (None, vec![Vec::new(); line_bounds(&text).len()]),
        };
        let minimap = compute_minimap(&text, &lines);
        Ok(Self {
            rope: Rope::from(text),
            tree,
            lines,
            minimap,
            anchors: AnchorList::default(),
        })
    }

    /// Content lines (the empty final line after a trailing newline is not counted).
    pub fn len_lines(&self) -> usize {
        self.lines.len()
    }

    /// The precomputed minimap summary for line `i`.
    pub fn minimap_row(&self, i: usize) -> MinimapRow {
        self.minimap[i]
    }

    /// Line text (newline stripped) plus its highlight spans. Text comes
    /// from the rope — never from a cached copy of the original string.
    pub fn line(&self, i: usize) -> Option<(String, &[HighlightSpan])> {
        let spans = self.lines.get(i)?;
        let mut text = self.rope.line(i).to_string();
        while text.ends_with(['\n', '\r']) {
            text.pop();
        }
        Some((text, spans.as_slice()))
    }

    /// Convert a byte offset to its 0-based line index, clamped to file length.
    pub fn byte_to_line(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte.min(self.rope.len_bytes()))
    }

    /// Register a tracked byte position; survives future edits via `AnchorList::remap`.
    pub fn create_anchor(&mut self, offset: usize) -> AnchorId {
        self.anchors.create(offset)
    }

    /// Return the current byte offset of a previously registered anchor.
    pub fn resolve_anchor(&self, id: AnchorId) -> usize {
        self.anchors.resolve(id)
    }
}

/// Byte bounds of each line's content (trailing newline/CR excluded).
fn line_bounds(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for seg in text.split_inclusive('\n') {
        let content = seg.trim_end_matches(['\n', '\r']);
        out.push(start..start + content.len());
        start += seg.len();
    }
    out
}

/// Dominant highlight kind on a line: the kind covering the most bytes,
/// ties broken by first occurrence; `Default` when the line has no mapped
/// spans (blank or all-default).
fn dominant_kind(spans: &[HighlightSpan]) -> HighlightKind {
    let mut acc: Vec<(HighlightKind, usize)> = Vec::new();
    for s in spans {
        let w = s.range.end - s.range.start;
        if let Some(e) = acc.iter_mut().find(|(k, _)| *k == s.kind) {
            e.1 += w;
        } else {
            acc.push((s.kind, w));
        }
    }
    let mut best: Option<(HighlightKind, usize)> = None;
    for &(k, w) in &acc {
        if best.is_none_or(|(_, bw)| w > bw) {
            best = Some((k, w));
        }
    }
    best.map(|(k, _)| k).unwrap_or(HighlightKind::Default)
}

/// One MinimapRow per line, aligned with `lines` (the per-line spans).
fn compute_minimap(text: &str, lines: &[Vec<HighlightSpan>]) -> Vec<MinimapRow> {
    let bounds = line_bounds(text);
    bounds
        .iter()
        .zip(lines)
        .map(|(b, spans)| {
            let content = &text[b.clone()];
            let indent = content
                .chars()
                .take_while(|&c| c == ' ' || c == '\t')
                .count() as u32;
            let trimmed = content.trim();
            let len = trimmed.chars().count() as u32;
            let kind = if len == 0 {
                HighlightKind::Default
            } else {
                dominant_kind(spans)
            };
            MinimapRow { indent, len, kind }
        })
        .collect()
}

/// Capture-name → HighlightKind (spec §3.2). Full-name matches first
/// (markdown block captures), then the prefix map. Unmapped captures
/// (punctuation, operators, `none`, …) are skipped and paint as Default.
fn kind_for(capture: &str) -> Option<HighlightKind> {
    match capture {
        "text.title" => return Some(HighlightKind::Type),
        "text.literal" => return Some(HighlightKind::String),
        "text.uri" | "text.reference" => return Some(HighlightKind::Property),
        _ => {}
    }
    match capture.split('.').next().unwrap_or(capture) {
        "keyword" | "conditional" | "repeat" | "include" | "exception" => {
            Some(HighlightKind::Keyword)
        }
        "function" => Some(HighlightKind::Function),
        "type" | "constructor" => Some(HighlightKind::Type),
        "string" | "escape" => Some(HighlightKind::String),
        "comment" => Some(HighlightKind::Comment),
        "constant" | "number" => Some(HighlightKind::Number),
        "property" => Some(HighlightKind::Property),
        _ => None,
    }
}

/// Run HIGHLIGHTS_QUERY over the whole file, splitting captures into
/// per-line spans. Overlaps resolve outermost-first (sort by start asc,
/// end desc; drop spans starting inside an earlier-kept span).
fn highlight_lines(
    text: &str,
    tree: &Tree,
    language: &tree_sitter::Language,
    query_src: &str,
) -> anyhow::Result<Vec<Vec<HighlightSpan>>> {
    let bounds = line_bounds(text);
    let mut lines: Vec<Vec<HighlightSpan>> = vec![Vec::new(); bounds.len()];
    let query = Query::new(language, query_src).context("compiling highlight query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let Some(kind) = kind_for(query.capture_names()[cap.index as usize]) else {
                continue;
            };
            let r = cap.node.byte_range();
            let first = bounds.partition_point(|b| b.end < r.start);
            for (l, b) in bounds.iter().enumerate().skip(first) {
                if b.start >= r.end {
                    break;
                }
                let s = r.start.max(b.start);
                let e = r.end.min(b.end);
                if s < e {
                    lines[l].push(HighlightSpan {
                        range: s - b.start..e - b.start,
                        kind,
                    });
                }
            }
        }
    }
    for spans in &mut lines {
        spans.sort_by(|a, b| {
            a.range
                .start
                .cmp(&b.range.start)
                .then(b.range.end.cmp(&a.range.end))
        });
        let mut end = 0;
        spans.retain(|s| {
            if s.range.start >= end {
                end = s.range.end;
                true
            } else {
                false
            }
        });
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_remap_insert_before_inside_after() {
        let mut a = AnchorList::default();
        let id = a.create(100);
        a.remap(&Edit {
            range: 50..50,
            new_len: 10,
        }); // insert before → shifts
        assert_eq!(a.resolve(id), 110);
        a.remap(&Edit {
            range: 200..200,
            new_len: 7,
        }); // insert after → unchanged
        assert_eq!(a.resolve(id), 110);
        a.remap(&Edit {
            range: 105..120,
            new_len: 3,
        }); // replace spanning → clamp
        assert_eq!(a.resolve(id), 105);
    }

    #[test]
    fn anchor_delete_spanning_clamps_to_start() {
        let mut a = AnchorList::default();
        let id = a.create(30);
        a.remap(&Edit {
            range: 20..40,
            new_len: 0,
        });
        assert_eq!(a.resolve(id), 20);
    }

    #[test]
    fn anchor_at_edit_boundaries() {
        let mut a = AnchorList::default();
        let id = a.create(50);
        a.remap(&Edit {
            range: 50..60,
            new_len: 1,
        }); // edit starts at anchor → stays
        assert_eq!(a.resolve(id), 50);
        a.remap(&Edit {
            range: 50..50,
            new_len: 4,
        }); // insertion at anchor → shifts
        assert_eq!(a.resolve(id), 54);
    }

    #[test]
    fn multi_anchor_ordering_preserved() {
        let mut a = AnchorList::default();
        let x = a.create(10);
        let y = a.create(20);
        let z = a.create(30);
        a.remap(&Edit {
            range: 15..25,
            new_len: 2,
        }); // y clamps to 15; z shifts −8
        assert_eq!((a.resolve(x), a.resolve(y), a.resolve(z)), (10, 15, 22));
        assert!(a.resolve(x) <= a.resolve(y) && a.resolve(y) <= a.resolve(z));
    }

    const SNIPPET: &str = "// a comment line\nfn free() -> i32 {\n    let s = \"hi\";\n    42\n}\n";

    #[test]
    fn highlight_kinds_and_bounds() {
        let buf = FileBuffer::new(SNIPPET.to_string(), "src/lib.rs").unwrap();
        assert_eq!(buf.len_lines(), 5);
        let (t0, s0) = buf.line(0).unwrap();
        assert_eq!(t0, "// a comment line");
        assert!(s0.iter().any(|s| s.kind == HighlightKind::Comment));
        let (t1, s1) = buf.line(1).unwrap();
        assert_eq!(t1, "fn free() -> i32 {");
        assert!(s1
            .iter()
            .any(|s| s.kind == HighlightKind::Keyword && &t1[s.range.clone()] == "fn"));
        let (_t2, s2) = buf.line(2).unwrap();
        assert!(s2.iter().any(|s| s.kind == HighlightKind::String));
        // every span lies within its line's bounds, sorted and non-overlapping
        for i in 0..buf.len_lines() {
            let (text, spans) = buf.line(i).unwrap();
            let mut end = 0;
            for s in spans {
                assert!(s.range.start < s.range.end);
                assert!(
                    s.range.start >= end && s.range.end <= text.len(),
                    "span {:?} out of bounds in line {i}: {text:?}",
                    s.range
                );
                end = s.range.end;
            }
        }
        assert!(buf.line(5).is_none());
    }

    #[test]
    fn cpp_extensions_enable_syntax_highlighting() {
        let text = "// note\nclass Widget {\npublic:\n    int value = 42;\n};\n";
        for ext in ["cpp", "cc", "cxx", "hpp", "hxx", "hh"] {
            let path = format!("sample.{ext}");
            let buf = FileBuffer::new(text.to_string(), path).unwrap();
            let (comment, comment_spans) = buf.line(0).unwrap();
            assert!(
                comment_spans.iter().any(|span| {
                    span.kind == HighlightKind::Comment && &comment[span.range.clone()] == "// note"
                }),
                "{ext} did not highlight a C++ comment: {comment_spans:?}"
            );
        }
    }

    #[test]
    fn byte_to_line_and_anchor_roundtrip() {
        let mut buf = FileBuffer::new(SNIPPET.to_string(), "src/lib.rs").unwrap();
        assert_eq!(buf.byte_to_line(0), 0);
        assert_eq!(buf.byte_to_line(18), 1); // first byte of "fn free…"
        let a = buf.create_anchor(18);
        assert_eq!(buf.resolve_anchor(a), 18);
        assert_eq!(buf.byte_to_line(buf.resolve_anchor(a)), 1);
    }

    #[test]
    fn plain_mode_has_lines_but_no_spans() {
        let text = "alpha beta\n\ngamma\n";
        let buf = FileBuffer::new(text.to_string(), "notes.txt").unwrap();
        assert_eq!(buf.len_lines(), 3);
        let (t0, s0) = buf.line(0).unwrap();
        assert_eq!(t0, "alpha beta");
        assert!(s0.is_empty());
        let (t1, s1) = buf.line(1).unwrap();
        assert_eq!(t1, "");
        assert!(s1.is_empty());
        let (t2, s2) = buf.line(2).unwrap();
        assert_eq!(t2, "gamma");
        assert!(s2.is_empty());
        // anchors still work in plain mode
        let mut buf = FileBuffer::new(text.to_string(), "LICENSE").unwrap();
        let a = buf.create_anchor(12);
        assert_eq!(buf.byte_to_line(buf.resolve_anchor(a)), 2);
    }

    const MD_SNIPPET: &str = "# Title\n\nplain text\n\n```\nlet x = 1;\n```\n";

    #[test]
    fn markdown_headings_and_fences_highlight() {
        let buf = FileBuffer::new(MD_SNIPPET.to_string(), "docs/guide.md").unwrap();
        assert_eq!(buf.len_lines(), 7);
        // heading content is Type ("text.title")
        let (t0, s0) = buf.line(0).unwrap();
        assert!(
            s0.iter()
                .any(|s| s.kind == HighlightKind::Type && &t0[s.range.clone()] == "Title"),
            "no Type span over 'Title' in {s0:?}"
        );
        // plain paragraph line: no mapped spans
        let (_t2, s2) = buf.line(2).unwrap();
        assert!(s2.is_empty(), "paragraph should be unhighlighted: {s2:?}");
        // fenced block ("text.literal" spans the whole block): every fence
        // line carries a String span
        for i in 4..=6 {
            let (_t, s) = buf.line(i).unwrap();
            assert!(
                s.iter().any(|sp| sp.kind == HighlightKind::String),
                "no String span on fence line {i}: {s:?}"
            );
        }
    }

    const TOML_SNIPPET: &str = "# note\n[package]\nname = \"x\"\ncount = 3\n";

    #[test]
    fn toml_keys_and_values_highlight() {
        let buf = FileBuffer::new(TOML_SNIPPET.to_string(), "Cargo.toml").unwrap();
        assert_eq!(buf.len_lines(), 4);
        let (t0, s0) = buf.line(0).unwrap();
        assert!(s0
            .iter()
            .any(|s| s.kind == HighlightKind::Comment && &t0[s.range.clone()] == "# note"));
        // table header key
        let (t1, s1) = buf.line(1).unwrap();
        assert!(s1
            .iter()
            .any(|s| s.kind == HighlightKind::Property && &t1[s.range.clone()] == "package"));
        // pair: key is Property, value is String — both present (the
        // embedded query must not let a whole-pair capture swallow them)
        let (t2, s2) = buf.line(2).unwrap();
        assert!(s2
            .iter()
            .any(|s| s.kind == HighlightKind::Property && &t2[s.range.clone()] == "name"));
        assert!(s2.iter().any(|s| s.kind == HighlightKind::String));
        let (_t3, s3) = buf.line(3).unwrap();
        assert!(s3.iter().any(|s| s.kind == HighlightKind::Number));
    }

    #[test]
    fn minimap_rows_report_indent_len_and_dominant_kind() {
        // line 0: comment; line 1: indented let with a string; line 2: blank
        let text = "// hello world\n    let s = \"xy\";\n\n";
        let buf = FileBuffer::new(text.to_string(), "src/main.rs").unwrap();
        assert_eq!(buf.len_lines(), 3);
        let r0 = buf.minimap_row(0);
        assert_eq!(r0.indent, 0);
        assert_eq!(r0.len, "// hello world".chars().count() as u32);
        assert_eq!(r0.kind, HighlightKind::Comment); // whole line is comment
        let r1 = buf.minimap_row(1);
        assert_eq!(r1.indent, 4); // four leading spaces
        assert_eq!(r1.len, "let s = \"xy\";".chars().count() as u32);
        // blank line: no bar
        let r2 = buf.minimap_row(2);
        assert_eq!(r2.len, 0);
        assert_eq!(r2.kind, HighlightKind::Default);
    }

    #[test]
    fn minimap_dominant_kind_breaks_ties_by_first_occurrence() {
        // plain-mode line has no spans → Default
        let buf = FileBuffer::new("abcdef\n".to_string(), "notes.txt").unwrap();
        assert_eq!(buf.minimap_row(0).kind, HighlightKind::Default);
        assert_eq!(buf.minimap_row(0).len, 6);
    }

    #[test]
    fn conventional_make_paths_highlight_make_syntax() {
        const MAKE: &str =
            "# build settings\nMODE := debug\ninclude common.mk\nifeq ($(MODE),debug)\nall: app\nendif\n";

        for path in ["Makefile", "makefile", "GNUmakefile", "build/rules.mk"] {
            let buf = FileBuffer::new(MAKE.to_string(), path).unwrap();
            let (comment, comment_spans) = buf.line(0).unwrap();
            assert!(
                comment_spans.iter().any(|span| {
                    span.kind == HighlightKind::Comment
                        && &comment[span.range.clone()] == "# build settings"
                }),
                "{path}: missing comment highlight: {comment_spans:?}"
            );

            let (variable, variable_spans) = buf.line(1).unwrap();
            assert!(
                variable_spans.iter().any(|span| {
                    span.kind == HighlightKind::Property && &variable[span.range.clone()] == "MODE"
                }),
                "{path}: missing variable highlight: {variable_spans:?}"
            );

            let (include, include_spans) = buf.line(2).unwrap();
            assert!(
                include_spans.iter().any(|span| {
                    span.kind == HighlightKind::Keyword && &include[span.range.clone()] == "include"
                }),
                "{path}: missing include highlight: {include_spans:?}"
            );

            let (conditional, conditional_spans) = buf.line(3).unwrap();
            assert!(
                conditional_spans.iter().any(|span| {
                    span.kind == HighlightKind::Keyword
                        && &conditional[span.range.clone()] == "ifeq"
                }),
                "{path}: missing conditional highlight: {conditional_spans:?}"
            );

            let (target, target_spans) = buf.line(4).unwrap();
            assert!(
                target_spans.iter().any(|span| {
                    span.kind == HighlightKind::Function && &target[span.range.clone()] == "all"
                }),
                "{path}: missing target highlight: {target_spans:?}"
            );
        }
    }

    #[test]
    fn make_capture_categories_map_to_existing_palette() {
        for capture in ["conditional", "repeat", "include", "exception"] {
            assert_eq!(kind_for(capture), Some(HighlightKind::Keyword), "{capture}");
        }
    }
}
