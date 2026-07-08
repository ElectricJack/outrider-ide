use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::process::Command;

use outrider_index::{finalize_children, SymbolId, SymbolKind, SymbolNode, SymbolTree};
use outrider_layout::layout;

fn node(
    kind: SymbolKind,
    qp: &str,
    name: &str,
    measure: u64,
    mut children: Vec<SymbolNode>,
) -> SymbolNode {
    finalize_children(&mut children);
    SymbolNode {
        id: SymbolId {
            kind,
            qualified_path: qp.into(),
            ordinal: 0,
        },
        name: name.into(),
        byte_range: None,
        measure,
        churn: 0.0,
        churn_count: 0,
        children,
    }
}

/// Worked example plus one extra folder level — fixed by hand, not generated.
fn canonical_tree() -> SymbolTree {
    let b = node(
        SymbolKind::File,
        "src/b.rs",
        "b.rs",
        40,
        vec![
            node(SymbolKind::Fn, "src/b.rs::f", "f", 10, vec![]),
            node(SymbolKind::Fn, "src/b.rs::g", "g", 1, vec![]),
        ],
    );
    let src = node(
        SymbolKind::Folder,
        "src",
        "src",
        140,
        vec![node(SymbolKind::File, "src/a.rs", "a.rs", 100, vec![]), b],
    );
    let root = node(
        SymbolKind::Folder,
        "",
        "",
        141,
        vec![src, node(SymbolKind::File, "README.md", "README.md", 30, vec![])],
    );
    SymbolTree {
        root,
        repo_root: "/canon".into(),
    }
}

fn layout_hash() -> u64 {
    let w = layout(&canonical_tree());
    let mut h = DefaultHasher::new(); // fixed keys — stable across processes
    format!("{w:?}").hash(&mut h);
    h.finish()
}

#[test]
fn layout_hash_identical_across_processes() {
    if std::env::var("OUTRIDER_DET_CHILD").is_ok() {
        println!("LAYOUT_HASH={}", layout_hash());
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let run_child = || {
        let out = Command::new(&exe)
            .args([
                "layout_hash_identical_across_processes",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("OUTRIDER_DET_CHILD", "1")
            .output()
            .expect("failed to spawn child test process");
        assert!(out.status.success(), "child process failed");
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .find_map(|l| {
                if let Some(start) = l.find("LAYOUT_HASH=") {
                    Some(l[start + "LAYOUT_HASH=".len()..].to_string())
                } else {
                    None
                }
            })
            .expect("child printed no LAYOUT_HASH line")
    };
    let h1 = run_child();
    let h2 = run_child();
    assert_eq!(h1, h2, "two child processes disagree");
    assert_eq!(h1, layout_hash().to_string(), "parent disagrees with children");
}
