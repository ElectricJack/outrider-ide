//! Search palette: fuzzy file/symbol search with filtered results list.

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

const MAX_RESULTS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    File,
    Symbol,
}

pub struct Palette {
    open: bool,
    pub mode: PaletteMode,
    pub query: String,
    pub results: Vec<SymbolId>,
    pub selection: usize,
    candidates: Vec<(SymbolId, String)>,
}

impl Palette {
    pub fn new() -> Self {
        Self {
            open: false,
            mode: PaletteMode::File,
            query: String::new(),
            results: Vec::new(),
            selection: 0,
            candidates: Vec::new(),
        }
    }

    pub fn open(&mut self, mode: PaletteMode, tree: &SymbolTree) {
        self.open = true;
        self.mode = mode;
        self.query.clear();
        self.selection = 0;
        self.candidates.clear();
        Self::collect_candidates(&tree.root, mode, &mut self.candidates);
        self.candidates.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| a.1.cmp(&b.1)));
        self.refilter();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.results.clear();
        self.candidates.clear();
        self.selection = 0;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn type_char(&mut self, ch: char, _tree: &SymbolTree) {
        self.query.push(ch);
        self.refilter();
    }

    pub fn backspace(&mut self, _tree: &SymbolTree) {
        self.query.pop();
        self.refilter();
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.results.is_empty() {
            return;
        }
        let len = self.results.len() as i32;
        self.selection = ((self.selection as i32 + delta).rem_euclid(len)) as usize;
    }

    pub fn confirm(&self) -> Option<SymbolId> {
        if !self.open {
            return None;
        }
        self.results.get(self.selection).cloned()
    }

    pub fn name_of(&self, id: &SymbolId) -> &str {
        self.candidates
            .iter()
            .find(|(cid, _)| cid == id)
            .map(|(_, name)| name.as_str())
            .unwrap_or("?")
    }

    fn refilter(&mut self) {
        self.results = self
            .candidates
            .iter()
            .filter(|(_, name)| fuzzy_match(&self.query, name))
            .take(MAX_RESULTS)
            .map(|(id, _)| id.clone())
            .collect();
        if self.selection >= self.results.len() {
            self.selection = self.results.len().saturating_sub(1);
        }
    }

    fn collect_candidates(node: &SymbolNode, mode: PaletteMode, out: &mut Vec<(SymbolId, String)>) {
        let include = match mode {
            PaletteMode::File => node.id.kind == SymbolKind::File,
            PaletteMode::Symbol => node.id.kind != SymbolKind::Folder,
        };
        if include {
            out.push((node.id.clone(), node.name.clone()));
        }
        for c in &node.children {
            Self::collect_candidates(c, mode, out);
        }
    }
}

pub fn fuzzy_match(query: &str, name: &str) -> bool {
    let mut name_chars = name.chars().flat_map(|c| c.to_lowercase());
    for qc in query.chars().flat_map(|c| c.to_lowercase()) {
        loop {
            match name_chars.next() {
                Some(nc) if nc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    fn test_tree() -> SymbolTree {
        fn node(kind: SymbolKind, qp: &str, name: &str, children: Vec<SymbolNode>) -> SymbolNode {
            SymbolNode {
                id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
                name: name.into(),
                byte_range: None,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                children,
            }
        }
        SymbolTree {
            root: node(
                SymbolKind::Folder,
                "",
                "",
                vec![
                    node(SymbolKind::File, "parse.rs", "parse.rs", vec![
                        node(SymbolKind::Item { label: "fn".into() }, "parse.rs::parse_item", "parse_item", vec![]),
                        node(SymbolKind::Item { label: "fn".into() }, "parse.rs::tokenize", "tokenize", vec![]),
                    ]),
                    node(SymbolKind::File, "main.rs", "main.rs", vec![
                        node(SymbolKind::Item { label: "fn".into() }, "main.rs::main", "main", vec![]),
                    ]),
                    node(SymbolKind::Folder, "utils", "utils", vec![
                        node(SymbolKind::File, "utils/helpers.rs", "helpers.rs", vec![]),
                    ]),
                ],
            ),
            repo_root: std::path::PathBuf::from("/tmp"),
        }
    }

    #[test]
    fn palette_file_mode_lists_files() {
        let tree = test_tree();
        let mut p = Palette::new();
        p.open(PaletteMode::File, &tree);
        assert!(p.is_open());
        // 3 files: parse.rs, main.rs, helpers.rs
        assert_eq!(p.results.len(), 3);
    }

    #[test]
    fn palette_symbol_mode_lists_non_folders() {
        let tree = test_tree();
        let mut p = Palette::new();
        p.open(PaletteMode::Symbol, &tree);
        // all non-Folder: 3 files + 3 items = 6
        assert_eq!(p.results.len(), 6);
    }

    #[test]
    fn palette_filters_on_type() {
        let tree = test_tree();
        let mut p = Palette::new();
        p.open(PaletteMode::Symbol, &tree);
        p.type_char('t', &tree);
        p.type_char('k', &tree);
        // "tk" matches "tokenize" only
        assert_eq!(p.results.len(), 1);
        assert_eq!(p.results[0].qualified_path, "parse.rs::tokenize");
    }

    #[test]
    fn palette_backspace_widens_results() {
        let tree = test_tree();
        let mut p = Palette::new();
        p.open(PaletteMode::Symbol, &tree);
        p.type_char('t', &tree);
        p.type_char('k', &tree);
        assert_eq!(p.results.len(), 1);
        p.backspace(&tree);
        // "t" matches tokenize, parse_item (has 't' at position 8) — more than 1
        assert!(p.results.len() > 1);
    }

    #[test]
    fn palette_selection_wraps() {
        let tree = test_tree();
        let mut p = Palette::new();
        p.open(PaletteMode::File, &tree);
        assert_eq!(p.selection, 0);
        p.move_selection(1);
        assert_eq!(p.selection, 1);
        p.move_selection(-1);
        assert_eq!(p.selection, 0);
        // wraps at bottom
        p.move_selection(-1);
        assert_eq!(p.selection, p.results.len() - 1);
    }

    #[test]
    fn palette_confirm_returns_selected() {
        let tree = test_tree();
        let mut p = Palette::new();
        p.open(PaletteMode::File, &tree);
        p.move_selection(1);
        let id = p.confirm().unwrap();
        assert_eq!(id, p.results[1]);
    }

    #[test]
    fn palette_close_clears_state() {
        let tree = test_tree();
        let mut p = Palette::new();
        p.open(PaletteMode::File, &tree);
        p.close();
        assert!(!p.is_open());
        assert!(p.confirm().is_none());
    }

    #[test]
    fn palette_caps_results_at_12() {
        // Build a tree with 20 files
        fn node(kind: SymbolKind, qp: &str, name: &str) -> SymbolNode {
            SymbolNode {
                id: SymbolId { kind, qualified_path: qp.into(), ordinal: 0 },
                name: name.into(),
                byte_range: None,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                children: vec![],
            }
        }
        let files: Vec<SymbolNode> = (0..20)
            .map(|i| node(SymbolKind::File, &format!("f{i}.rs"), &format!("f{i}.rs")))
            .collect();
        let tree = SymbolTree {
            root: SymbolNode {
                id: SymbolId { kind: SymbolKind::Folder, qualified_path: "".into(), ordinal: 0 },
                name: "".into(),
                byte_range: None,
                signature: None,
                doc: None,
                measure: 1,
                churn: 0.0,
                churn_count: 0,
                children: files,
            },
            repo_root: std::path::PathBuf::from("/tmp"),
        };
        let mut p = Palette::new();
        p.open(PaletteMode::File, &tree);
        assert_eq!(p.results.len(), 12);
    }

    #[test]
    fn fuzzy_match_exact() {
        assert!(fuzzy_match("parse", "parse"));
    }

    #[test]
    fn fuzzy_match_subsequence() {
        assert!(fuzzy_match("prs", "parse"));
        assert!(fuzzy_match("fmn", "file_manager_new"));
    }

    #[test]
    fn fuzzy_match_case_insensitive() {
        assert!(fuzzy_match("PRS", "parse"));
        assert!(fuzzy_match("prs", "PARSE"));
    }

    #[test]
    fn fuzzy_match_no_match() {
        assert!(!fuzzy_match("xyz", "parse"));
        assert!(!fuzzy_match("srp", "parse")); // wrong order
    }

    #[test]
    fn fuzzy_match_empty_query_matches_all() {
        assert!(fuzzy_match("", "anything"));
    }
}
