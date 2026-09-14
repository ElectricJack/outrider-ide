# Kind-Grouped, Size-Aware Packing

**Date:** 2026-07-11
**Status:** Approved

## Problem

The packer (`crates/outrider-layout/src/pack.rs`) orders children strictly
alphabetically and fills columns greedily. When the next child in
alphabetical order doesn't fit in the current column, the column ends
early — leaving large empty strips (see `ManagePlugins.tsx` screenshots:
short columns beside one giant function, small type boxes floating in dead
space). Alphabetical order also interleaves types, functions, and classes,
so related kinds don't read as groups.

## Goals

- Same-level items group by kind: types first, then loose functions, then
  classes/impls, then modules.
- Columns fill evenly: tallest-first placement within each group
  (first-fit-decreasing), sharply reducing empty space.
- Keep determinism and hierarchical stability: a container's layout depends
  only on its own children's kinds, names, and sizes.

## Non-Goals

- Best-fit/backfill placement that scatters a group across columns
  (rejected: dilutes the grouping). May revisit as a follow-up if
  group-boundary gaps still look bad in practice.
- Renderer changes — `treemap.rs` consumes rects and is order-agnostic.

## Design

All changes live in `crates/outrider-layout/src/pack.rs`.

### 1. Kind rank

A local helper maps a child's `SymbolKind` to a group rank:

| Rank | Kinds |
|------|-------|
| 0 | `Item` labels `struct`, `enum`, `trait`, `interface`, `type` |
| 1 | `fn` |
| 2 | `class`, `impl` |
| 3 | `module`, `namespace` |
| 4 | any other `Item` label |

`File` and `Folder` children all get rank 0 — folders have no kind
grouping, just size-aware order. `Chunk` children keep the existing
source-order branch, untouched.

### 2. Sort

In `size()`, children are currently sized *after* sorting. That flips:
recurse to get each child's `(w, h)` first, then sort by

```
(rank, height desc via f64::total_cmp, name bytes, ordinal)
```

Name remains the tiebreak so equal-height runs (e.g., a wall of small type
boxes) stay alphabetical, and the layout stays fully deterministic.

### 3. Placement

The greedy column-fill loop is unchanged. Tallest-first input turns it
into first-fit-decreasing: each wrap leaves only a small gap, and the
tallest child (which sets `target_h`) lands in the first column of its
group with everything smaller filling evenly to its right.

## Testing

- Rewrite `children_placed_by_name_then_ordinal_never_size` to assert the
  new contract: tallest first, name breaks height ties.
- Add a kind-grouping test: a small `struct` still packs before a large
  loose `fn`.
- Re-derive expected rects in existing tests whose order changes; chunk
  source-order and determinism tests must keep passing as-is.

## Risks

- Positions shift when a child's size changes relative to siblings (items
  can swap columns after an edit). Accepted: names are pinned and readable
  at every zoom, so spatial memory matters less than packing density.
