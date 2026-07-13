mod common;

use outrider_index::{index_repo, index_repo_outcome, SymbolKind, SymbolNode};

fn find<'a>(node: &'a SymbolNode, qual: &str) -> Option<&'a SymbolNode> {
    if node.id.qualified_path == qual {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, qual))
}

#[test]
fn index_repo_parses_rust_files_into_items() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path(), &[], &[]).unwrap();

    let lib = find(&tree.root, "src/lib.rs").expect("src/lib.rs node");
    assert_eq!(lib.id.kind, SymbolKind::File);

    // file children are name-sorted (spec §4.1), not source-ordered:
    // Point (impl), Point (struct), free, inner  -> sorted byte-wise:
    // "Point"(impl? struct?) ties resolved by source order via ordinal
    let kids: Vec<(&str, SymbolKind, u16)> = lib
        .children
        .iter()
        .map(|c| (c.name.as_str(), c.id.kind.clone(), c.id.ordinal))
        .collect();
    assert_eq!(
        kids,
        vec![
            (
                "Point",
                SymbolKind::Item {
                    label: "struct".into()
                },
                0
            ), // struct appears before impl in source
            (
                "Point",
                SymbolKind::Item {
                    label: "impl".into()
                },
                1
            ),
            ("free", SymbolKind::Item { label: "fn".into() }, 0),
            (
                "inner",
                SymbolKind::Item {
                    label: "module".into()
                },
                0
            ),
        ]
    );

    // nesting + qualified paths
    let helper = find(&tree.root, "src/lib.rs::inner::helper").expect("nested fn");
    assert_eq!(helper.id.kind, SymbolKind::Item { label: "fn".into() });
    assert!(helper.byte_range.is_some());

    let norm = find(&tree.root, "src/lib.rs::Point::norm").expect("method");
    assert_eq!(norm.id.kind, SymbolKind::Item { label: "fn".into() });
    assert_eq!(norm.measure, 3); // 3-line method body span

    // Phase 4b metadata: signature + doc (spec §3.1)
    assert_eq!(
        lib.doc.as_deref(),
        Some("Mini fixture library.\nExercises doc extraction.")
    );
    assert_eq!(lib.signature, None);
    assert_eq!(norm.signature.as_deref(), Some("fn norm(&self) -> f64"));
    let util = find(&tree.root, "src/util.rs").expect("util.rs node");
    assert_eq!(util.doc, None);
    assert_eq!(tree.root.signature, None);
    assert_eq!(tree.root.doc, None);

    // ignored file contributed nothing (spec §8.2)
    assert!(find(&tree.root, "generated/junk.rs").is_none());

    // util.rs has its free fn
    let clamp = find(&tree.root, "src/util.rs::clamp").expect("clamp fn");
    assert_eq!(clamp.measure, 3);
}

#[test]
fn symbol_ids_are_unique_tree_wide() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path(), &[], &[]).unwrap();
    fn walk(n: &outrider_index::SymbolNode, out: &mut Vec<outrider_index::SymbolId>) {
        out.push(n.id.clone());
        for c in &n.children {
            walk(c, out);
        }
    }
    let mut ids = Vec::new();
    walk(&tree.root, &mut ids);
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), total, "duplicate SymbolIds in indexed tree");
}

#[test]
fn index_outcome_preserves_normalized_retained_source_fingerprints() {
    let dir = common::copy_fixture("mini_repo");
    std::fs::write(dir.path().join("opaque"), b"not retained").unwrap();
    let outcome = index_repo_outcome(dir.path(), &[], &[]).unwrap();

    assert!(outcome.source_fingerprints.contains_key("src/lib.rs"));
    assert!(outcome.source_fingerprints.contains_key("README.md"));
    assert!(outcome
        .source_fingerprints
        .keys()
        .all(|path| !path.contains('\\')));
    assert!(!outcome.source_fingerprints.contains_key("opaque"));
}
