mod common;

use outrider_index::index_repo;

#[test]
fn dump_format_shows_kind_name_measure_and_churn_readout() {
    let dir = common::copy_fixture("mini_repo");
    let tree = index_repo(dir.path(), &[], &[]).unwrap();
    let out = outrider_index::dump::render(&tree);

    // spec §5.4 inspectability: raw count and percentile, both visible
    assert!(out.contains("File util.rs"), "out was:\n{out}");
    assert!(out.contains("[3 lines"), "out was:\n{out}");
    assert!(out.contains("churn 0 · p0"), "out was:\n{out}");
    // nesting is indented: method deeper than file
    let file_line = out.lines().find(|l| l.contains("File lib.rs")).unwrap();
    let fn_line = out.lines().find(|l| l.contains("fn norm")).unwrap();
    let indent = |s: &str| s.len() - s.trim_start().len();
    assert!(indent(fn_line) > indent(file_line));
}
