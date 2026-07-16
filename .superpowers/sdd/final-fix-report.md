# Final Review Fix Report

## Scope

- Static-pattern rules now take their display name from the actual `targets` syntax child before falling back to the static `target` pattern field.
- Make file children retain parser/source byte order while same-name ordinals are assigned in encounter order. Other languages continue to use `finalize_children` name sorting.

## RED evidence

1. `cargo test -p outrider-index make_static_pattern_rule_uses_actual_targets_for_name -- --nocapture`
   - Failed as expected: `left: "%.o"`, `right: "objects"`.
2. `cargo test -p outrider-index --test index_test index_repo_keeps_make_children_in_source_byte_order -- --exact --nocapture`
   - Failed as expected in the unsorted coverage assertion: first child began at byte 24 instead of byte 0.

## GREEN evidence

1. `cargo test -p outrider-index make_static_pattern_rule_uses_actual_targets_for_name -- --nocapture`
   - Passed: 1 focused parser regression test.
2. `cargo test -p outrider-index --test index_test index_repo_keeps_make_children_in_source_byte_order -- --exact --nocapture`
   - Passed: 1 focused integration regression test.
3. `cargo test -p outrider-index`
   - Passed: 95 unit tests, 9 churn tests, 1 dump test, 5 index tests, 3 scan tests, and doc tests; 0 failures.
4. `cargo test -p outrider buffers::tests`
   - Passed: 8 tests, 0 failures. Existing unrelated compiler warnings remain (unused/dead code and future-incompatibility notice).
5. `git diff --check`
   - Passed with no whitespace errors. Git emitted only existing LF-to-CRLF checkout notices.

## Self-review

- Changes are limited to the Make parser/index ordering implementation and focused tests.
- Unique sibling IDs remain deterministic: same-name Make siblings receive monotonically increasing ordinals in source order, and tree-wide `dedupe_ids` remains unchanged.
- Non-Make parsing still calls the original name-sorting `finalize_children` path.
- `cargo fmt --all -- --check` is not clean because the pre-existing branch contains numerous unrelated unformatted files; none were modified to avoid expanding scope. The four touched Rust files are formatted in the displayed diff.
