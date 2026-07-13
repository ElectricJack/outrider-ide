//! Pluggable file chunking: split an over-threshold text file into ordered,
//! contiguous, covering sub-pages. Pure functions of `&str` — no GPUI, no
//! filesystem.

/// One contiguous slice of a file. `start_line`/`end_line` are 0-based
/// (start inclusive, end exclusive); `start_byte`/`end_byte` cover the same
/// span including each line's trailing newline, so adjacent chunks meet
/// exactly and the last chunk ends at `text.len()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub label: String,
}

/// Splits a file's text into ordered, contiguous, covering `Chunk`s.
pub trait ChunkStrategy {
    /// Ordered, contiguous, covering chunks. Returns a single whole-file
    /// chunk when the file is under threshold; the caller treats `len == 1`
    /// as "do not chunk".
    fn chunks(&self, text: &str) -> Vec<Chunk>;
}

/// Soft cap / slice size in lines.
pub const CHUNK_MAX_LINES: usize = 60;

/// `"md" | "markdown"` → semantic Markdown splits; everything else → line
/// slices.
pub fn strategy_for(ext: &str) -> Box<dyn ChunkStrategy> {
    match ext {
        "md" | "markdown" => Box::new(MarkdownChunker),
        _ => Box::new(LineChunker),
    }
}

/// (start_byte, end_byte) of each line, newline included in end_byte. Matches
/// `buffer::line_bounds` line counting: a trailing newline does not add an
/// empty final line, and `""` yields zero lines.
fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for seg in text.split_inclusive('\n') {
        out.push((start, start + seg.len()));
        start += seg.len();
    }
    out
}

/// Line content (newline/CR trimmed) for each line.
fn line_contents(text: &str) -> Vec<&str> {
    text.split_inclusive('\n')
        .map(|s| s.trim_end_matches(['\n', '\r']))
        .collect()
}

/// Build a Chunk over lines `[a, b)` with the given label.
fn chunk_of(spans: &[(usize, usize)], a: usize, b: usize, label: String) -> Chunk {
    Chunk {
        start_line: a,
        end_line: b,
        start_byte: spans[a].0,
        end_byte: spans[b - 1].1,
        label,
    }
}

/// Human-readable 1-based inclusive line range label (e.g. "1–60").
fn range_label(a: usize, b: usize) -> String {
    format!("{}–{}", a + 1, b) // 1-based inclusive, en dash
}

/// Fixed-size line-slice strategy: slices at every `CHUNK_MAX_LINES` boundary.
pub struct LineChunker;

/// Uniform line-slice chunking for non-Markdown files.
impl ChunkStrategy for LineChunker {
    fn chunks(&self, text: &str) -> Vec<Chunk> {
        let spans = line_spans(text);
        let n = spans.len();
        if n == 0 {
            return Vec::new();
        }
        if n <= CHUNK_MAX_LINES {
            return vec![chunk_of(&spans, 0, n, range_label(0, n))];
        }
        let mut out = Vec::new();
        let mut a = 0;
        while a < n {
            let b = (a + CHUNK_MAX_LINES).min(n);
            out.push(chunk_of(&spans, a, b, range_label(a, b)));
            a = b;
        }
        out
    }
}

/// Heading-aware strategy: splits at ATX headings, with blank-line fallback
/// for oversized heading-less sections.
pub struct MarkdownChunker;

/// `^\s{0,3}#{1,6}\s` — up to 3 leading spaces, 1–6 `#`, then a space/tab.
fn is_heading(line: &str) -> bool {
    let leading = line.len() - line.trim_start_matches(' ').len();
    if leading > 3 {
        return false;
    }
    let rest = &line[leading..];
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    rest[hashes..].starts_with([' ', '\t'])
}

/// Heading text with markers and surrounding whitespace stripped.
fn heading_label(line: &str) -> String {
    line.trim_start_matches(' ')
        .trim_start_matches('#')
        .trim()
        .to_string()
}

/// Semantic Markdown chunking: heading splits with preamble-merge and oversized-section fallback.
impl ChunkStrategy for MarkdownChunker {
    fn chunks(&self, text: &str) -> Vec<Chunk> {
        let spans = line_spans(text);
        let content = line_contents(text);
        let n = spans.len();
        if n == 0 {
            return Vec::new();
        }
        // Chunk-start line indices.
        let mut bounds = vec![0usize];
        let mut cur_start = 0usize;
        // Whether the current chunk has already started on/absorbed a heading;
        // lets a leading preamble merge into the first heading's section.
        let mut seen_heading = is_heading(content[0]);
        for (i, line) in content.iter().enumerate().skip(1) {
            let start_new = if is_heading(line) {
                if seen_heading {
                    true
                } else {
                    seen_heading = true; // merge preamble into this section
                    false
                }
            } else {
                line.trim().is_empty() && (i - cur_start) >= CHUNK_MAX_LINES
            };
            if start_new {
                bounds.push(i);
                cur_start = i;
                seen_heading = seen_heading || is_heading(line);
            }
        }
        bounds
            .iter()
            .enumerate()
            .map(|(k, &a)| {
                let b = bounds.get(k + 1).copied().unwrap_or(n);
                let label = if is_heading(content[a]) {
                    heading_label(content[a])
                } else {
                    range_label(a, b)
                };
                chunk_of(&spans, a, b, label)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build "L1\nL2\n...Ln\n" with n lines.
    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("L{i}\n")).collect()
    }

    fn assert_contiguous(chunks: &[Chunk], text: &str) {
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].start_line, 0);
        assert_eq!(chunks[0].start_byte, 0);
        assert_eq!(chunks.last().unwrap().end_byte, text.len());
        for w in chunks.windows(2) {
            assert_eq!(w[0].end_line, w[1].start_line);
            assert_eq!(w[0].end_byte, w[1].start_byte);
        }
    }

    #[test]
    fn line_chunker_short_file_is_one_chunk() {
        let text = numbered(3);
        let cs = LineChunker.chunks(&text);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].label, "1–3");
        assert_eq!((cs[0].start_line, cs[0].end_line), (0, 3));
        assert_contiguous(&cs, &text);
    }

    #[test]
    fn line_chunker_splits_into_60_line_slices() {
        let text = numbered(150);
        let cs = LineChunker.chunks(&text);
        let labels: Vec<&str> = cs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["1–60", "61–120", "121–150"]);
        assert_eq!(
            cs.iter()
                .map(|c| (c.start_line, c.end_line))
                .collect::<Vec<_>>(),
            vec![(0, 60), (60, 120), (120, 150)]
        );
        assert_contiguous(&cs, &text);
    }

    #[test]
    fn markdown_chunker_splits_at_headings_with_heading_labels() {
        let text = "# Alpha\none\ntwo\n## Beta\nthree\n";
        let cs = MarkdownChunker.chunks(text);
        let labels: Vec<&str> = cs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["Alpha", "Beta"]);
        assert_eq!(
            cs.iter()
                .map(|c| (c.start_line, c.end_line))
                .collect::<Vec<_>>(),
            vec![(0, 3), (3, 5)]
        );
        assert_contiguous(&cs, text);
    }

    #[test]
    fn markdown_chunker_merges_preamble_into_first_chunk() {
        let text = "intro line\n# Alpha\nbody\n## Beta\nend\n";
        let cs = MarkdownChunker.chunks(text);
        // preamble (line 0) merges with the Alpha section: one chunk 0..3,
        // then Beta 3..5. The merged first chunk begins on a non-heading
        // line, so it uses the range label.
        assert_eq!(cs.len(), 2);
        assert_eq!((cs[0].start_line, cs[0].end_line), (0, 3));
        assert_eq!(cs[0].label, "1–3");
        assert_eq!(cs[1].label, "Beta");
        assert_contiguous(&cs, text);
    }

    #[test]
    fn markdown_chunker_splits_long_section_at_a_blank_line() {
        // Heading + 65 non-blank lines + blank + tail. The blank at index 66
        // is the first blank once the running chunk exceeds 60 lines.
        let mut text = String::from("# Head\n");
        for _ in 0..65 {
            text.push_str("x\n");
        }
        text.push('\n'); // blank line, index 66
        text.push_str("tail\n");
        let cs = MarkdownChunker.chunks(&text);
        assert!(
            cs.len() >= 2,
            "expected a blank-line split, got {}",
            cs.len()
        );
        // no chunk boundary lands inside the paragraph of x's (lines 1..=65)
        for c in &cs {
            assert!(
                c.start_line == 0 || c.start_line >= 66,
                "boundary at {} broke the paragraph",
                c.start_line
            );
        }
        assert_contiguous(&cs, &text);
    }

    #[test]
    fn markdown_chunker_short_doc_is_one_chunk() {
        let text = "just a paragraph\nwith two lines\n";
        let cs = MarkdownChunker.chunks(text);
        assert_eq!(cs.len(), 1);
        assert_contiguous(&cs, text);
    }

    #[test]
    fn strategy_for_selects_by_extension() {
        // 150 non-markdown lines chunk by slices; a markdown file with no
        // headings/blank-splits stays one chunk.
        assert_eq!(strategy_for("txt").chunks(&numbered(150)).len(), 3);
        assert_eq!(strategy_for("rs").chunks(&numbered(150)).len(), 3);
        let md = "# A\ntext\n# B\nmore\n";
        assert_eq!(strategy_for("md").chunks(md).len(), 2);
        assert_eq!(strategy_for("markdown").chunks(md).len(), 2);
    }

    #[test]
    fn markdown_chunker_heading_after_blank_split_starts_new_chunk() {
        // Section 1 heading + 65 body lines forces a blank-line split (blank at
        // index 66); the heading at index 67 must start its OWN chunk, not merge
        // into the post-blank tail.
        let mut text = String::from("# One\n");
        for _ in 0..65 {
            text.push_str("x\n");
        }
        text.push('\n'); // blank, index 66
        text.push_str("# Two\n"); // heading, index 67
        text.push_str("tail\n");
        let cs = MarkdownChunker.chunks(&text);
        let labels: Vec<&str> = cs.iter().map(|c| c.label.as_str()).collect();
        // Some chunk must be labeled by the second heading "Two" (i.e. it began
        // on that heading, not merged into a range-labeled chunk).
        assert!(
            labels.contains(&"Two"),
            "heading after blank split should start its own chunk; got {labels:?}"
        );
        assert!(!cs.is_empty());
        assert_eq!(cs[0].start_line, 0);
        assert_eq!(cs.last().unwrap().end_byte, text.len());
        for w in cs.windows(2) {
            assert_eq!(w[0].end_line, w[1].start_line);
            assert_eq!(w[0].end_byte, w[1].start_byte);
        }
    }
}
