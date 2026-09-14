# Source-Ordered Files and Doc-Rank Folder Packing

**Date:** 2026-07-11
**Status:** Approved

## Problem

Kind-grouped, size-aware packing (spec
`2026-07-11-packing-order-design.md`) reorganizes a container's children.
That is wrong for two cases:

1. **Prose and declaration-order languages.** Markdown/text files read
   top-to-bottom; C/C++ requires declare-before-use. Reorganizing their
   sections/symbols makes the map unreadable. Today `.c`/`.h` files parse
   into symbol items and get reorganized; markdown/`.cpp` currently fall
   back to source-ordered chunks, but the rule must be extension-keyed so
   adding parsers later doesn't regress them.
2. **Documentation sinks.** Within a folder, doc trees (`docs/`, big
   READMEs) currently compete with source purely by size. Source should
   pack first.

## Design

All changes live in `crates/outrider-layout/src/pack.rs`.

### Extension sets

- `DOC_EXTS`: `md`, `markdown`, `txt`, `rst`
- `SOURCE_ORDERED_EXTS`: `DOC_EXTS` ∪ `c`, `h`, `cpp`, `hpp`, `cc`, `hh`,
  `cxx`, `hxx`, `inl`

The extension comes from the file part of a qualified path: everything
before the first `::` (and before any `#` chunk suffix), then after the
last `.`.

### 1. Source-ordered files (every nesting level)

In `size()`, if the packing parent is **not** a `Folder` and its
qualified path's file extension is in `SOURCE_ORDERED_EXTS`, children
sort by `(byte_range.start, ordinal)` — the same rule `Chunk` children
already use (that branch is untouched and takes precedence). This applies
at every nesting level: C struct members, markdown subsections, anything
whose qualified path roots in such a file. Missing `byte_range` sorts as
start 0, then ordinal breaks the tie.

The greedy column fill is unchanged, so source-ordered children read
top-to-bottom, wrapping right — reading order.

### 2. Doc rank for folder children

- A `File` child is doc iff its name's extension is in `DOC_EXTS`.
- A `Folder` child is doc iff **more than 70%** of the files under it
  (recursively) are doc files: `doc_files * 10 > total_files * 7`.
  Empty folders are not doc.
- Other kinds rank 0.

The non-source-ordered sort key becomes
`(doc_rank, kind_rank, height desc, name bytes, ordinal)`. Doc ranks are
computed once per child before sorting — no tree walks inside the
comparator. `kind_rank` is 0 for all files/folders, so within a folder
this reads: source (rank 0) size-aware first, doc (rank 1) size-aware
last.

## Invariant amendment

`docs/code-comprehension-viewer-design.md` invariant #3: folder layout
now also depends on descendant file *extensions* (a doc-only folder
gaining a source file can float above doc siblings). Editing file
*contents* still never changes classification.

## Trade-off

Source-ordered files can pack with more empty space than size-aware
order. Accepted: readability wins for prose and declaration-order code.

## Testing

- `.c` file: scrambled-size fn items pack by byte offset, not kind/size.
- Nested level: a container inside a `.md` file packs its children by
  byte offset.
- Folder: a large doc file sinks below smaller source files.
- Folder classification: 3-of-4 doc files (75%) sinks; 1-of-2 (50%)
  does not.
- Existing packing tests (kind grouping, tallest-first, chunks,
  determinism) keep passing unchanged.
