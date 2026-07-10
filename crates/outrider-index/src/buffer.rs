use std::ops::Range;

use anyhow::Context;
use ropey::Rope;
use tree_sitter::{Query, QueryCursor, StreamingIterator, Tree};

/// Handle to a tracked byte position (spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnchorId(usize);

/// A buffer mutation: `range` replaced by `new_len` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub new_len: usize,
}

#[derive(Debug, Default)]
pub struct AnchorList {
    positions: Vec<usize>,
}

impl AnchorList {
    pub fn create(&mut self, offset: usize) -> AnchorId {
        self.positions.push(offset);
        AnchorId(self.positions.len() - 1)
    }

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

pub struct FileBuffer {
    rope: Rope,
    /// Held for Phase 6 incremental re-parse; unused until then. `None`
    /// in plain mode (no grammar for the extension).
    #[allow(dead_code)]
    tree: Option<Tree>,
    /// Per-line spans, computed once at materialization (spec §3.2).
    lines: Vec<Vec<HighlightSpan>>,
    anchors: AnchorList,
}

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

impl FileBuffer {
    /// `ext` is the bare lowercase file extension (no dot). Known
    /// extensions parse and highlight; anything else is plain mode —
    /// no parse, every line's span list empty.
    pub fn new(text: String, ext: &str) -> anyhow::Result<Self> {
        let lang: Option<(tree_sitter::Language, &str)> = match ext {
            "rs" => Some((tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY)),
            "md" => Some((tree_sitter_md::LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_BLOCK)),
            "toml" => Some((tree_sitter_toml_ng::LANGUAGE.into(), TOML_HIGHLIGHTS)),
            _ => None,
        };
        let (tree, lines) = match lang {
            Some((language, query_src)) => {
                let mut parser = tree_sitter::Parser::new();
                parser.set_language(&language).context("loading tree-sitter grammar")?;
                let tree = parser.parse(&text, None).context("tree-sitter parse failed")?;
                let lines = highlight_lines(&text, &tree, &language, query_src)?;
                (Some(tree), lines)
            }
            None => (None, vec![Vec::new(); line_bounds(&text).len()]),
        };
        Ok(Self { rope: Rope::from(text), tree, lines, anchors: AnchorList::default() })
    }

    /// Content lines (the empty final line after a trailing newline is not counted).
    pub fn len_lines(&self) -> usize {
        self.lines.len()
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

    pub fn byte_to_line(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte.min(self.rope.len_bytes()))
    }

    pub fn create_anchor(&mut self, offset: usize) -> AnchorId {
        self.anchors.create(offset)
    }

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
        "keyword" => Some(HighlightKind::Keyword),
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
                    lines[l].push(HighlightSpan { range: s - b.start..e - b.start, kind });
                }
            }
        }
    }
    for spans in &mut lines {
        spans.sort_by(|a, b| a.range.start.cmp(&b.range.start).then(b.range.end.cmp(&a.range.end)));
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
        a.remap(&Edit { range: 50..50, new_len: 10 }); // insert before → shifts
        assert_eq!(a.resolve(id), 110);
        a.remap(&Edit { range: 200..200, new_len: 7 }); // insert after → unchanged
        assert_eq!(a.resolve(id), 110);
        a.remap(&Edit { range: 105..120, new_len: 3 }); // replace spanning → clamp
        assert_eq!(a.resolve(id), 105);
    }

    #[test]
    fn anchor_delete_spanning_clamps_to_start() {
        let mut a = AnchorList::default();
        let id = a.create(30);
        a.remap(&Edit { range: 20..40, new_len: 0 });
        assert_eq!(a.resolve(id), 20);
    }

    #[test]
    fn anchor_at_edit_boundaries() {
        let mut a = AnchorList::default();
        let id = a.create(50);
        a.remap(&Edit { range: 50..60, new_len: 1 }); // edit starts at anchor → stays
        assert_eq!(a.resolve(id), 50);
        a.remap(&Edit { range: 50..50, new_len: 4 }); // insertion at anchor → shifts
        assert_eq!(a.resolve(id), 54);
    }

    #[test]
    fn multi_anchor_ordering_preserved() {
        let mut a = AnchorList::default();
        let x = a.create(10);
        let y = a.create(20);
        let z = a.create(30);
        a.remap(&Edit { range: 15..25, new_len: 2 }); // y clamps to 15; z shifts −8
        assert_eq!((a.resolve(x), a.resolve(y), a.resolve(z)), (10, 15, 22));
        assert!(a.resolve(x) <= a.resolve(y) && a.resolve(y) <= a.resolve(z));
    }

    const SNIPPET: &str =
        "// a comment line\nfn free() -> i32 {\n    let s = \"hi\";\n    42\n}\n";

    #[test]
    fn highlight_kinds_and_bounds() {
        let buf = FileBuffer::new(SNIPPET.to_string(), "rs").unwrap();
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
    fn byte_to_line_and_anchor_roundtrip() {
        let mut buf = FileBuffer::new(SNIPPET.to_string(), "rs").unwrap();
        assert_eq!(buf.byte_to_line(0), 0);
        assert_eq!(buf.byte_to_line(18), 1); // first byte of "fn free…"
        let a = buf.create_anchor(18);
        assert_eq!(buf.resolve_anchor(a), 18);
        assert_eq!(buf.byte_to_line(buf.resolve_anchor(a)), 1);
    }

    #[test]
    fn plain_mode_has_lines_but_no_spans() {
        let text = "alpha beta\n\ngamma\n";
        let buf = FileBuffer::new(text.to_string(), "txt").unwrap();
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
        let mut buf = FileBuffer::new(text.to_string(), "").unwrap();
        let a = buf.create_anchor(12);
        assert_eq!(buf.byte_to_line(buf.resolve_anchor(a)), 2);
    }

    const MD_SNIPPET: &str = "# Title\n\nplain text\n\n```\nlet x = 1;\n```\n";

    #[test]
    fn markdown_headings_and_fences_highlight() {
        let buf = FileBuffer::new(MD_SNIPPET.to_string(), "md").unwrap();
        assert_eq!(buf.len_lines(), 7);
        // heading content is Type ("text.title")
        let (t0, s0) = buf.line(0).unwrap();
        assert!(
            s0.iter().any(|s| s.kind == HighlightKind::Type && &t0[s.range.clone()] == "Title"),
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
        let buf = FileBuffer::new(TOML_SNIPPET.to_string(), "toml").unwrap();
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
}
