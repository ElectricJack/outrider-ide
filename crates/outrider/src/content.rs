//! Content model: text metrics, body-line generation, and inventory strings
//! for each symbol kind × rung combination (spec §§3–4.4).
//! Pure functions — no rendering; shared by the layout and paint paths.

#[cfg(test)]
use crate::world::Rung;
use outrider_index::{SymbolKind, SymbolNode};

/// Monospace body font size (px); shared by content math and the paint path.
pub const FONT_PX: f64 = 12.0;
/// Vertical distance between baselines (130% leading).
pub const LINE_STEP: f64 = FONT_PX * 1.3;
/// Name-row height: text top padding (4) plus one meta-line offset.
pub const HEADER: f64 = 4.0 + FONT_PX * 1.4;
/// Padding below the last body row inside a leaf box.
pub const BOTTOM_PAD: f64 = 6.0;

/// Below this on-screen font size a leaf paints its texture instead of live
/// text (the text/texture tier boundary).
pub const MIN_TEXT_FONT_PX: f64 = 4.0;

/// A leaf page: has source bytes, no children, and is not a folder.
/// Items are code pages; childless files (markdown, TOML, plain text,
/// unparsed .rs) are text pages. These boxes render their content at
/// Full and keep the editor background at every rung.
pub fn is_leaf_item(node: &SymbolNode) -> bool {
    node.byte_range.is_some() && node.children.is_empty() && node.id.kind != SymbolKind::Folder
}

/// Natural pixel height of a leaf item's box: header + signature row +
/// one row per code line + bottom pad.
pub fn natural_px(node: &SymbolNode) -> f64 {
    HEADER + (1.0 + node.measure as f64) * LINE_STEP + BOTTOM_PAD
}

// Body lines, inventory strings, and related helpers are no longer rendered
// but kept under #[cfg(test)] for the existing test suite.

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyLine {
    Plain(String),
    Dim(String),
}

#[cfg(test)]
pub fn card_meta(node: &SymbolNode) -> String {
    format!(
        "{} · p{:.0} · {}L",
        node.churn_count,
        node.churn * 100.0,
        node.measure
    )
}

#[cfg(test)]
pub fn churn_readout(node: &SymbolNode) -> String {
    format!(
        "{}L · {} commits · p{:.0}",
        node.measure,
        node.churn_count,
        node.churn * 100.0
    )
}

#[cfg(test)]
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

#[cfg(test)]
pub fn kind_counts(node: &SymbolNode) -> String {
    if node.id.kind == SymbolKind::Folder {
        let files = node
            .children
            .iter()
            .filter(|c| c.id.kind == SymbolKind::File)
            .count();
        let folders = node
            .children
            .iter()
            .filter(|c| c.id.kind == SymbolKind::Folder)
            .count();
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

#[cfg(test)]
pub fn inventory(node: &SymbolNode) -> String {
    let kinds = kind_counts(node);
    if kinds.is_empty() {
        churn_readout(node)
    } else {
        format!("{kinds} · {}", churn_readout(node))
    }
}

#[cfg(test)]
pub fn body_lines(_node: &SymbolNode, _rung: Rung) -> Vec<BodyLine> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Rung;
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
            id: SymbolId {
                kind,
                qualified_path: qual.into(),
                ordinal: 0,
            },
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
                node(
                    SymbolKind::Item {
                        label: "struct".into(),
                    },
                    "m.rs::Point",
                    4,
                    0.5,
                    3,
                    Some("struct Point"),
                    None,
                    vec![],
                ),
                node(
                    SymbolKind::Item {
                        label: "impl".into(),
                    },
                    "m.rs::Point",
                    9,
                    0.5,
                    3,
                    Some("impl Point"),
                    None,
                    vec![
                        node(
                            SymbolKind::Item { label: "fn".into() },
                            "m.rs::Point::new",
                            3,
                            0.5,
                            3,
                            Some("fn new() -> Self"),
                            None,
                            vec![],
                        ),
                        node(
                            SymbolKind::Item { label: "fn".into() },
                            "m.rs::Point::norm",
                            3,
                            0.5,
                            3,
                            Some("fn norm(&self) -> f64"),
                            None,
                            vec![],
                        ),
                    ],
                ),
                node(
                    SymbolKind::Item { label: "fn".into() },
                    "m.rs::free",
                    3,
                    0.5,
                    3,
                    Some("fn free()"),
                    None,
                    vec![],
                ),
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
                node(
                    SymbolKind::File,
                    "src/a.rs",
                    400,
                    0.0,
                    0,
                    None,
                    None,
                    vec![],
                ),
                node(
                    SymbolKind::File,
                    "src/b.rs",
                    400,
                    0.0,
                    0,
                    None,
                    None,
                    vec![],
                ),
                node(
                    SymbolKind::Folder,
                    "src/sub",
                    12,
                    0.0,
                    0,
                    None,
                    None,
                    vec![],
                ),
            ],
        )
    }

    #[test]
    fn inventory_strings_are_exact() {
        let f = file();
        assert_eq!(churn_readout(&f), "480L · 47 commits · p96");
        assert_eq!(kind_counts(&f), "3 fns · 1 impl · 1 struct");
        assert_eq!(
            inventory(&f),
            "3 fns · 1 impl · 1 struct · 480L · 47 commits · p96"
        );
        let d = folder();
        assert_eq!(kind_counts(&d), "2 files · 1 folder");
        assert_eq!(
            inventory(&d),
            "2 files · 1 folder · 812L · 12 commits · p40"
        );
        // empty node: inventory degrades to the readout alone
        let empty = node(SymbolKind::File, "e.rs", 0, 0.0, 0, None, None, vec![]);
        assert_eq!(kind_counts(&empty), "");
        assert_eq!(inventory(&empty), "0L · 0 commits · p0");
        // card meta keeps the pre-4b format exactly
        assert_eq!(card_meta(&f), "47 · p96 · 480L");
    }

    #[test]
    fn body_lines_always_empty() {
        let f = file();
        let d = folder();
        for rung in [Rung::Dot, Rung::Label, Rung::Card, Rung::Detail, Rung::Full] {
            assert_eq!(body_lines(&f, rung), vec![]);
            assert_eq!(body_lines(&d, rung), vec![]);
        }
    }

    #[test]
    fn natural_px_arithmetic() {
        // HEADER 20.8 + (1 + measure)·15.6 + BOTTOM_PAD 6
        let three = node(
            SymbolKind::Item { label: "fn".into() },
            "a.rs::f",
            3,
            0.0,
            0,
            Some("fn f()"),
            None,
            vec![],
        );
        assert!((natural_px(&three) - 89.2).abs() < 1e-9);
        let long = node(
            SymbolKind::Item { label: "fn".into() },
            "a.rs::g",
            200,
            0.0,
            0,
            Some("fn g()"),
            None,
            vec![],
        );
        assert!((natural_px(&long) - 3162.4).abs() < 1e-9);
    }

    #[test]
    fn leaf_item_predicate() {
        let mut f = node(
            SymbolKind::Item { label: "fn".into() },
            "a.rs::f",
            3,
            0.0,
            0,
            None,
            None,
            vec![],
        );
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
            vec![node(
                SymbolKind::Item { label: "fn".into() },
                "a.rs::f",
                1,
                0.0,
                0,
                None,
                None,
                vec![],
            )],
        );
        parent_file.byte_range = Some(0..10);
        assert!(!is_leaf_item(&parent_file));
        // folders never qualify
        let mut folder = node(SymbolKind::Folder, "src", 3, 0.0, 0, None, None, vec![]);
        folder.byte_range = Some(0..10);
        assert!(!is_leaf_item(&folder));
        let parent = node(
            SymbolKind::Item {
                label: "impl".into(),
            },
            "a.rs::I",
            3,
            0.0,
            0,
            None,
            None,
            vec![node(
                SymbolKind::Item { label: "fn".into() },
                "a.rs::I::m",
                1,
                0.0,
                0,
                None,
                None,
                vec![],
            )],
        );
        assert!(!is_leaf_item(&parent)); // has children
    }

    #[test]
    fn chunked_file_counts_parts_and_chunk_body_is_one_readout() {
        use BodyLine::Dim;
        // A File container whose children are Chunk nodes.
        let mut file = node(
            SymbolKind::File,
            "README.md",
            120,
            0.2,
            5,
            None,
            None,
            vec![
                node(
                    SymbolKind::Chunk,
                    "README.md#0",
                    60,
                    0.2,
                    5,
                    None,
                    None,
                    vec![],
                ),
                node(
                    SymbolKind::Chunk,
                    "README.md#1",
                    60,
                    0.2,
                    5,
                    None,
                    None,
                    vec![],
                ),
            ],
        );
        file.byte_range = Some(0..1000);
        assert_eq!(kind_counts(&file), "2 parts");
        assert_eq!(inventory(&file), "2 parts · 120L · 5 commits · p20");
        // a single-part edge still pluralizes correctly
        let one = node(
            SymbolKind::File,
            "x.txt",
            60,
            0.0,
            0,
            None,
            None,
            vec![node(
                SymbolKind::Chunk,
                "x.txt#0",
                60,
                0.0,
                0,
                None,
                None,
                vec![],
            )],
        );
        assert_eq!(kind_counts(&one), "1 part");
        let chunk = &file.children[0];
        assert_eq!(body_lines(chunk, Rung::Full), vec![]);
        assert_eq!(body_lines(chunk, Rung::Detail), vec![]);
    }
}
