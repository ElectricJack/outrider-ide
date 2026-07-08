mod common;

use outrider_index::{index_repo, SymbolKind, SymbolNode};

fn find<'a>(node: &'a SymbolNode, qual: &str) -> Option<&'a SymbolNode> {
    if node.id.qualified_path == qual {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, qual))
}

#[test]
fn index_repo_parses_rust_files_into_items() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path()).unwrap();

    let lib = find(&tree.root, "src/lib.rs").expect("src/lib.rs node");
    assert_eq!(lib.id.kind, SymbolKind::File);

    // file children are name-sorted (spec §4.1), not source-ordered:
    // Point (impl), Point (struct), free, inner  -> sorted byte-wise:
    // "Point"(impl? struct?) ties resolved by source order via ordinal
    let kids: Vec<(&str, SymbolKind, u16)> = lib
        .children
        .iter()
        .map(|c| (c.name.as_str(), c.id.kind, c.id.ordinal))
        .collect();
    assert_eq!(
        kids,
        vec![
            ("Point", SymbolKind::Struct, 0), // struct appears before impl in source
            ("Point", SymbolKind::Impl, 1),
            ("free", SymbolKind::Fn, 0),
            ("inner", SymbolKind::Module, 0),
        ]
    );

    // nesting + qualified paths
    let helper = find(&tree.root, "src/lib.rs::inner::helper").expect("nested fn");
    assert_eq!(helper.id.kind, SymbolKind::Fn);
    assert!(helper.byte_range.is_some());

    let norm = find(&tree.root, "src/lib.rs::Point::norm").expect("method");
    assert_eq!(norm.id.kind, SymbolKind::Fn);
    assert_eq!(norm.measure, 3); // 3-line method body span

    // ignored file contributed nothing (spec §8.2)
    assert!(find(&tree.root, "generated/junk.rs").is_none());

    // util.rs has its free fn
    let clamp = find(&tree.root, "src/util.rs::clamp").expect("clamp fn");
    assert_eq!(clamp.measure, 3);
}
