use outrider_index::{SymbolKind, SymbolNode};

use crate::world::Rung;

/// Monospace body font size (px); shared by content math and the paint path.
pub const FONT_PX: f64 = 12.0;
pub const LINE_STEP: f64 = FONT_PX * 1.3;
/// Name-row height: text top padding (4) plus one meta-line offset.
pub const HEADER: f64 = 4.0 + FONT_PX * 1.4;
/// Padding below the last body row inside a leaf box.
pub const BOTTOM_PAD: f64 = 6.0;

/// Below this on-screen font size a leaf paints its texture instead of live
/// text (the text/texture tier boundary).
pub const MIN_TEXT_FONT_PX: f64 = 7.0;

/// Font-size range for the Texture↔Text crossfade: text fades in from
/// FADE_LO to FADE_HI while the baked texture fades out over the same range.
pub const TEXT_FADE_LO: f64 = 5.0;
pub const TEXT_FADE_HI: f64 = 9.0;

/// A leaf page: has source bytes, no children, and is not a folder.
/// Items are code pages; childless files (markdown, TOML, plain text,
/// unparsed .rs) are text pages. These boxes render their content at
/// Full and keep the editor background at every rung.
pub fn is_leaf_item(node: &SymbolNode) -> bool {
    node.byte_range.is_some()
        && node.children.is_empty()
        && node.id.kind != SymbolKind::Folder
}

/// Natural pixel height of a leaf item's box: header + signature row +
/// one row per code line + bottom pad.
pub fn natural_px(node: &SymbolNode) -> f64 {
    HEADER + (1.0 + node.measure as f64) * LINE_STEP + BOTTOM_PAD
}

/// One rendered body line under a box's name row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyLine {
    /// TEXT_PRIMARY
    Plain(String),
    /// TEXT_SECONDARY
    Dim(String),
}

/// Card meta line — format unchanged from the pre-4b render (spec §4.4).
pub fn card_meta(node: &SymbolNode) -> String {
    format!("{} · p{:.0} · {}L", node.churn_count, node.churn * 100.0, node.measure)
}

/// e.g. "480L · 47 commits · p96"
pub fn churn_readout(node: &SymbolNode) -> String {
    format!("{}L · {} commits · p{:.0}", node.measure, node.churn_count, node.churn * 100.0)
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Item counts by kind: all descendants for files/items ("3 fns · 1 struct");
/// direct child files/folders for folders ("2 files · 1 folder"). Empty
/// string when there is nothing to count.
pub fn kind_counts(node: &SymbolNode) -> String {
    if node.id.kind == SymbolKind::Folder {
        let files = node.children.iter().filter(|c| c.id.kind == SymbolKind::File).count();
        let folders = node.children.iter().filter(|c| c.id.kind == SymbolKind::Folder).count();
        let mut parts = Vec::new();
        if files > 0 {
            parts.push(plural(files, "file"));
        }
        if folders > 0 {
            parts.push(plural(folders, "folder"));
        }
        return parts.join(" · ");
    }
    fn count(node: &SymbolNode, counts: &mut std::collections::BTreeMap<String, usize>) {
        for k in &node.children {
            match &k.id.kind {
                SymbolKind::Item { label } => *counts.entry(label.clone()).or_insert(0) += 1,
                SymbolKind::Chunk => *counts.entry("part".to_string()).or_insert(0) += 1,
                SymbolKind::File | SymbolKind::Folder => {}
            }
            count(k, counts);
        }
    }
    let mut counts = std::collections::BTreeMap::new();
    count(node, &mut counts);
    counts
        .iter()
        .filter(|(_, &n)| n > 0)
        .map(|(w, &n)| plural(n, w))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The full inventory line (spec §4.3): kind counts + churn readout,
/// e.g. "4 fns · 2 structs · 480L · 47 commits · p96".
pub fn inventory(node: &SymbolNode) -> String {
    let kinds = kind_counts(node);
    if kinds.is_empty() {
        churn_readout(node)
    } else {
        format!("{kinds} · {}", churn_readout(node))
    }
}

/// Non-code body lines by node type and rung — the spec §4.3 content table.
/// Full leaf items return only their signature; the paint path appends the
/// highlighted code (or leaves this Detail-equivalent content when the
/// buffer is unavailable).
pub fn body_lines(node: &SymbolNode, rung: Rung) -> Vec<BodyLine> {
    match rung {
        Rung::Dot | Rung::Label => vec![],
        Rung::Card => vec![BodyLine::Dim(card_meta(node))],
        Rung::Detail | Rung::Full => match node.id.kind {
            SymbolKind::Folder => {
                if rung == Rung::Detail {
                    let mut out = vec![BodyLine::Dim(churn_readout(node))];
                    let kinds = kind_counts(node);
                    if !kinds.is_empty() {
                        out.push(BodyLine::Dim(kinds));
                    }
                    out
                } else {
                    vec![BodyLine::Dim(inventory(node))]
                }
            }
            SymbolKind::File => {
                if rung == Rung::Detail {
                    let mut out = vec![BodyLine::Dim(churn_readout(node))];
                    if let Some(first) = node.doc.as_deref().and_then(|d| d.lines().next()) {
                        out.push(BodyLine::Plain(first.to_string()));
                    }
                    let kinds = kind_counts(node);
                    if !kinds.is_empty() {
                        out.push(BodyLine::Dim(kinds));
                    }
                    out
                } else if node.children.is_empty() {
                    // Text page: one signature-equivalent row; the paint
                    // path appends the file text from row 1 (spec §3).
                    vec![BodyLine::Dim(churn_readout(node))]
                } else {
                    let mut out: Vec<BodyLine> = node
                        .doc
                        .as_deref()
                        .map(|d| d.lines().map(|l| BodyLine::Plain(l.to_string())).collect())
                        .unwrap_or_default();
                    out.push(BodyLine::Dim(inventory(node)));
                    out
                }
            }
            SymbolKind::Chunk => vec![BodyLine::Dim(churn_readout(node))],
            SymbolKind::Item { .. } => {
                let mut out = Vec::new();
                if let Some(sig) = &node.signature {
                    out.push(BodyLine::Plain(sig.clone()));
                }
                if rung == Rung::Full && !node.children.is_empty() {
                    out.push(BodyLine::Dim(inventory(node)));
                }
                out
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    #[allow(clippy::too_many_arguments)]
    fn node(
        kind: SymbolKind,
        qual: &str,
        measure: u64,
        churn: f32,
        churn_count: u64,
        signature: Option<&str>,
        doc: Option<&str>,
        children: Vec<SymbolNode>,
    ) -> SymbolNode {
        SymbolNode {
            id: SymbolId { kind, qualified_path: qual.into(), ordinal: 0 },
            name: qual.rsplit(['/', ':']).next().unwrap_or(qual).to_string(),
            byte_range: None,
            signature: signature.map(str::to_string),
            doc: doc.map(str::to_string),
            measure,
            churn,
            churn_count,
            children,
        }
    }

    /// File m.rs: struct Point, impl Point { fn new, fn norm }, fn free —
    /// 480L, 47 commits, p96, two-line doc.
    fn file() -> SymbolNode {
        node(
            SymbolKind::File,
            "m.rs",
            480,
            0.96,
            47,
            None,
            Some("Doc first.\nDoc second."),
            vec![
                node(SymbolKind::Item { label: "struct".into() }, "m.rs::Point", 4, 0.5, 3, Some("struct Point"), None, vec![]),
                node(
                    SymbolKind::Item { label: "impl".into() },
                    "m.rs::Point",
                    9,
                    0.5,
                    3,
                    Some("impl Point"),
                    None,
                    vec![
                        node(SymbolKind::Item { label: "fn".into() }, "m.rs::Point::new", 3, 0.5, 3, Some("fn new() -> Self"), None, vec![]),
                        node(SymbolKind::Item { label: "fn".into() }, "m.rs::Point::norm", 3, 0.5, 3, Some("fn norm(&self) -> f64"), None, vec![]),
                    ],
                ),
                node(SymbolKind::Item { label: "fn".into() }, "m.rs::free", 3, 0.5, 3, Some("fn free()"), None, vec![]),
            ],
        )
    }

    fn folder() -> SymbolNode {
        node(
            SymbolKind::Folder,
            "src",
            812,
            0.4,
            12,
            None,
            None,
            vec![
                node(SymbolKind::File, "src/a.rs", 400, 0.0, 0, None, None, vec![]),
                node(SymbolKind::File, "src/b.rs", 400, 0.0, 0, None, None, vec![]),
                node(SymbolKind::Folder, "src/sub", 12, 0.0, 0, None, None, vec![]),
            ],
        )
    }

    #[test]
    fn inventory_strings_are_exact() {
        let f = file();
        assert_eq!(churn_readout(&f), "480L · 47 commits · p96");
        assert_eq!(kind_counts(&f), "3 fns · 1 impl · 1 struct");
        assert_eq!(inventory(&f), "3 fns · 1 impl · 1 struct · 480L · 47 commits · p96");
        let d = folder();
        assert_eq!(kind_counts(&d), "2 files · 1 folder");
        assert_eq!(inventory(&d), "2 files · 1 folder · 812L · 12 commits · p40");
        // empty node: inventory degrades to the readout alone
        let empty = node(SymbolKind::File, "e.rs", 0, 0.0, 0, None, None, vec![]);
        assert_eq!(kind_counts(&empty), "");
        assert_eq!(inventory(&empty), "0L · 0 commits · p0");
        // card meta keeps the pre-4b format exactly
        assert_eq!(card_meta(&f), "47 · p96 · 480L");
    }

    #[test]
    fn body_lines_follow_the_content_table() {
        use BodyLine::{Dim, Plain};
        let f = file();
        let leaf = &f.children[2]; // fn free
        let container = &f.children[1]; // impl Point (2 children)
        let d = folder();

        // leaf item: signature at Detail AND Full (code appended by paint)
        assert_eq!(body_lines(leaf, Rung::Detail), vec![Plain("fn free()".into())]);
        assert_eq!(body_lines(leaf, Rung::Full), vec![Plain("fn free()".into())]);
        // container item: signature; Full adds the inventory
        assert_eq!(body_lines(container, Rung::Detail), vec![Plain("impl Point".into())]);
        assert_eq!(
            body_lines(container, Rung::Full),
            vec![Plain("impl Point".into()), Dim(inventory(container))]
        );
        // file Detail: churn readout + doc first line + kind counts
        assert_eq!(
            body_lines(&f, Rung::Detail),
            vec![
                Dim("480L · 47 commits · p96".into()),
                Plain("Doc first.".into()),
                Dim("3 fns · 1 impl · 1 struct".into()),
            ]
        );
        // file Full: whole doc block + inventory
        assert_eq!(
            body_lines(&f, Rung::Full),
            vec![
                Plain("Doc first.".into()),
                Plain("Doc second.".into()),
                Dim(inventory(&f)),
            ]
        );
        // folder Detail: readout + counts; Full: inventory only
        assert_eq!(
            body_lines(&d, Rung::Detail),
            vec![Dim("812L · 12 commits · p40".into()), Dim("2 files · 1 folder".into())]
        );
        assert_eq!(body_lines(&d, Rung::Full), vec![Dim(inventory(&d))]);
        // file without docs
        let nodoc = node(SymbolKind::File, "n.rs", 9, 0.0, 0, None, None, vec![]);
        assert_eq!(body_lines(&nodoc, Rung::Detail), vec![Dim("9L · 0 commits · p0".into())]);
        assert_eq!(body_lines(&nodoc, Rung::Full), vec![Dim("9L · 0 commits · p0".into())]);
        // Card keeps the legacy meta; Dot/Label have no body
        assert_eq!(body_lines(&f, Rung::Card), vec![Dim("47 · p96 · 480L".into())]);
        assert_eq!(body_lines(&f, Rung::Dot), vec![]);
        assert_eq!(body_lines(&f, Rung::Label), vec![]);
    }

    #[test]
    fn childless_file_full_body_is_one_readout_row() {
        use BodyLine::{Dim, Plain};
        // even with a doc comment, Full is exactly one row: the paint
        // path appends the file text (which contains the doc) from row 1,
        // keeping natural_px = HEADER + (1+measure)·LINE_STEP + BOTTOM_PAD
        let f = node(
            SymbolKind::File,
            "README.md",
            12,
            0.2,
            5,
            None,
            Some("# Readme\nIntro."),
            vec![],
        );
        assert_eq!(body_lines(&f, Rung::Full), vec![Dim("12L · 5 commits · p20".into())]);
        // Detail is unchanged: readout + doc first line (no kinds — childless)
        assert_eq!(
            body_lines(&f, Rung::Detail),
            vec![Dim("12L · 5 commits · p20".into()), Plain("# Readme".into())]
        );
    }

    #[test]
    fn natural_px_arithmetic() {
        // HEADER 20.8 + (1 + measure)·15.6 + BOTTOM_PAD 6
        let three = node(SymbolKind::Item { label: "fn".into() }, "a.rs::f", 3, 0.0, 0, Some("fn f()"), None, vec![]);
        assert!((natural_px(&three) - 89.2).abs() < 1e-9);
        let long = node(SymbolKind::Item { label: "fn".into() }, "a.rs::g", 200, 0.0, 0, Some("fn g()"), None, vec![]);
        assert!((natural_px(&long) - 3162.4).abs() < 1e-9);
    }

    #[test]
    fn leaf_item_predicate() {
        let mut f = node(SymbolKind::Item { label: "fn".into() }, "a.rs::f", 3, 0.0, 0, None, None, vec![]);
        assert!(!is_leaf_item(&f)); // no byte_range
        f.byte_range = Some(0..10);
        assert!(is_leaf_item(&f));
        // childless file WITH bytes is a leaf page now
        let mut file = node(SymbolKind::File, "a.md", 3, 0.0, 0, None, None, vec![]);
        assert!(!is_leaf_item(&file)); // no byte_range
        file.byte_range = Some(0..10);
        assert!(is_leaf_item(&file));
        // file with children is a container, not a page
        let mut parent_file = node(
            SymbolKind::File,
            "a.rs",
            3,
            0.0,
            0,
            None,
            None,
            vec![node(SymbolKind::Item { label: "fn".into() }, "a.rs::f", 1, 0.0, 0, None, None, vec![])],
        );
        parent_file.byte_range = Some(0..10);
        assert!(!is_leaf_item(&parent_file));
        // folders never qualify
        let mut folder = node(SymbolKind::Folder, "src", 3, 0.0, 0, None, None, vec![]);
        folder.byte_range = Some(0..10);
        assert!(!is_leaf_item(&folder));
        let parent = node(SymbolKind::Item { label: "impl".into() }, "a.rs::I", 3, 0.0, 0, None, None,
            vec![node(SymbolKind::Item { label: "fn".into() }, "a.rs::I::m", 1, 0.0, 0, None, None, vec![])]);
        assert!(!is_leaf_item(&parent)); // has children
    }

    #[test]
    fn chunked_file_counts_parts_and_chunk_body_is_one_readout() {
        use BodyLine::Dim;
        // A File container whose children are Chunk nodes.
        let mut file = node(SymbolKind::File, "README.md", 120, 0.2, 5, None, None, vec![
            node(SymbolKind::Chunk, "README.md#0", 60, 0.2, 5, None, None, vec![]),
            node(SymbolKind::Chunk, "README.md#1", 60, 0.2, 5, None, None, vec![]),
        ]);
        file.byte_range = Some(0..1000);
        assert_eq!(kind_counts(&file), "2 parts");
        assert_eq!(inventory(&file), "2 parts · 120L · 5 commits · p20");
        // a single-part edge still pluralizes correctly
        let one = node(SymbolKind::File, "x.txt", 60, 0.0, 0, None, None, vec![
            node(SymbolKind::Chunk, "x.txt#0", 60, 0.0, 0, None, None, vec![]),
        ]);
        assert_eq!(kind_counts(&one), "1 part");
        // a Chunk leaf's Full body is exactly its churn readout row
        let chunk = &file.children[0];
        assert_eq!(body_lines(chunk, Rung::Full), vec![Dim("60L · 5 commits · p20".into())]);
        assert_eq!(body_lines(chunk, Rung::Detail), vec![Dim("60L · 5 commits · p20".into())]);
    }
}
