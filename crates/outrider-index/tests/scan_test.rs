mod common;

use std::collections::BTreeMap;

use outrider_index::scan::{build_tree, scan_files};
use outrider_index::SymbolKind;

#[test]
fn scan_respects_gitignore_and_builds_sorted_tree() {
    let dir = common::copy_fixture("mini_repo");
    let files = scan_files(dir.path(), &[], &[]).unwrap();

    let paths: Vec<String> = files
        .iter()
        .map(|f| f.rel_path.to_string_lossy().into_owned())
        .collect();
    // generated/ and *.log excluded by .gitignore; .gitignore itself is a
    // dotfile, skipped by the walker's hidden-files default; Cargo.lock
    // excluded by the *.lock filter (generated, not source).
    assert_eq!(paths, vec!["README.md", "src/lib.rs", "src/util.rs"]);

    let tree = build_tree(dir.path(), &files, &BTreeMap::new());
    let root = &tree.root;
    assert_eq!(root.id.kind, SymbolKind::Folder);
    assert_eq!(root.id.qualified_path, "");

    let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["README.md", "src"]);

    let src = &root.children[1];
    assert_eq!(src.id.kind, SymbolKind::Folder);
    assert_eq!(src.id.qualified_path, "src");
    let src_names: Vec<&str> = src.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(src_names, vec!["lib.rs", "util.rs"]);
    assert_eq!(src.children[0].id.qualified_path, "src/lib.rs");
    assert_eq!(src.children[0].id.kind, SymbolKind::File);

    // folder measure = sum of children (spec §5.2)
    assert_eq!(src.measure, src.children.iter().map(|c| c.measure).sum::<u64>());
    assert_eq!(
        root.measure,
        root.children.iter().map(|c| c.measure).sum::<u64>()
    );

    // file measure = line count; util.rs has 3 lines
    assert_eq!(src.children[1].measure, 3);
}
