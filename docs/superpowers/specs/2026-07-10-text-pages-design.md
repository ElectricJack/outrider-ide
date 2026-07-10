# Text Pages & Leaf Backgrounds — Design

- Date: 2026-07-10
- Parent: `2026-07-09-spatial-treemap-pivot-design.md` (built on the merged
  treemap). Extends the 4b–4d content stack; no layout or camera changes.
- Motivation: files the scanner picks up but the parser can't decompose
  (markdown, TOML, plain text, unparsed `.rs`) render only a Detail
  summary at Full — their content is invisible. Also, leaf boxes flip
  from `depth_fill` to `CODE_BG` only at the Full rung, a jarring
  background pop when zooming into code.

## 1. Goal

Any childless file leaf renders its full text at the Full rung — syntax
highlighted for Markdown and TOML, plain text otherwise — through the
same buffer/anchor/scale machinery code leaves use. Leaf boxes (code or
text) keep the editor-black background at every rung, so zooming in never
changes a leaf's background.

## 2. Leaf-page predicate (`content.rs`)

`is_leaf_item` currently excludes `File` nodes. New rule — a **leaf
page** is any node with source bytes, no children, and not a Folder:

```rust
pub fn is_leaf_item(node: &SymbolNode) -> bool {
    node.byte_range.is_some()
        && node.children.is_empty()
        && node.id.kind != SymbolKind::Folder
}
```

Childless `File` nodes (every non-`.rs` file, plus `.rs` files that
parse to zero items) now qualify. Everything downstream is automatic:

- `world.rs` (`is_leaf_item(node).then(|| natural_px(node))`) gives file
  leaves natural-size rungs — `natural_px = HEADER + (1 + measure) ·
  LINE_STEP + BOTTOM_PAD`, where `measure` is the file's line count.
- `treemap.rs` framing dispatch sends file leaves through `frame_page`
  (never zooms past natural size).
- `code_scale` and the line-window clipping in `build_body` transfer
  unchanged.

Layout note: file leaves were already sized by the same leaf formula in
`pack.rs` (any childless node), so packed rects do not change.

## 3. File body at Full (`content.rs` `body_lines`)

The `SymbolKind::File` Full arm splits on childlessness:

- **Childless file:** return exactly one row, `Dim(churn_readout(node))`
  — the file page's "signature-equivalent" row. One row keeps the
  `natural_px` math exact: header + (1 + measure) rows means row 0 is the
  readout and rows 1..=measure are the file text appended by the paint
  path. (The previous draft's Detail-equivalent fallback of 2–3 rows
  would clip the file's last lines; the doc first-line row is redundant
  anyway — the text itself starts at line 0.)
- **File with children:** unchanged (doc lines + inventory).

The Detail arm is unchanged for both. When the buffer is unavailable
(binary/non-UTF-8 read failure, missing file), the page shows just the
readout row — same graceful degradation code leaves have when their
buffer fails.

## 4. File-node anchors (`buffers.rs`)

`collect_file_symbols` only pushes children today, so
`symbol_start_line(file_id)` resolves to `None` and the paint path would
skip the text. Change: for a **childless** file node, also push
`(file.id, byte_range.start)` — which is 0 — so the file page's text
window starts at rope line 0. Files with children are untouched (their
own id is never asked for; only their items').

## 5. Multi-grammar buffers (`outrider-index/src/buffer.rs`)

`FileBuffer::new` gains the file extension and selects a language:

```rust
pub fn new(text: String, ext: &str) -> anyhow::Result<Self>
```

| ext    | grammar                          | highlight query           |
|--------|----------------------------------|---------------------------|
| `rs`   | `tree_sitter_rust::LANGUAGE`     | `HIGHLIGHTS_QUERY`        |
| `md`   | `tree_sitter_md::LANGUAGE` (block grammar only) | `HIGHLIGHT_QUERY_BLOCK` |
| `toml` | `tree_sitter_toml_ng::LANGUAGE`  | `HIGHLIGHTS_QUERY`        |
| other  | **plain mode**: no parse, no spans |                         |

- Struct change: `tree: Tree` becomes `tree: Option<Tree>` (still
  `#[allow(dead_code)]`, held for Phase 6); plain mode stores `None` and
  one empty span-vec per line (`line_bounds` still runs so `len_lines`
  and `line()` behave identically).
- Caller: `BufferManager::get` (`buffers.rs`) passes the extension from
  `rel_path` (`Path::extension`, empty string when absent).
- Dependencies (`outrider-index/Cargo.toml`): `tree-sitter-md = "0.5.3"`,
  `tree-sitter-toml-ng = "0.7.0"`. Both verified at runtime against
  `tree-sitter 0.26.10` (parse + query compile succeed; ABI OK), so no
  fallback contingency is needed.
- Markdown uses the **block** grammar only; inline emphasis/links inside
  paragraphs paint as Default. Headings, fenced code blocks, and URIs
  still color. The inline grammar + injections are out of scope.
- `kind_for` extensions (checked before the existing prefix map):
  - `"text.title"` → `Type` (headings)
  - `"text.literal"` → `String` (code spans/fences)
  - `"text.uri"` | `"text.reference"` → `Property`
  - prefix `"boolean"` → `Number` (TOML `true`/`false`)
  - everything else falls through to the existing prefix map
    (TOML's `property`/`string`/`comment`/`number`/`type` already map).

## 6. Leaf background at every rung (`treemap.rs`, `theme.rs`)

The fill decision keys on the predicate, not the rung:

```rust
let fill = if content::is_leaf_item(item.node) {
    theme::CODE_BG
} else {
    theme::depth_fill(item.level)
};
```

Every leaf page — code or text — paints `CODE_BG` (0x101014) from Dot
through Full; containers keep the depth ramp. Borders stay
`border_for(fill)`, churn stripes and focus border unchanged. Zoomed
out, leaves read as dark pages against the lighter depth-shaded
containers; zooming in changes only the content, never the background.

## 7. Testing

Headless throughout:

- Predicate: childless File with bytes → true; File with children →
  false; Folder → false; existing item cases unchanged.
- Plain buffer: `FileBuffer::new(text, "txt")` — `len_lines`, `line()`
  text round-trip, all span lists empty.
- Markdown buffer: `# Title` line carries a `Type` span; fenced code
  carries `String`.
- TOML buffer: key gets `Property`, value gets `String`/`Number` spans.
- `collect_file_symbols`: childless file appears in its own symbol list
  at offset 0; file-with-children lists remain items-only.
- `body_lines`: childless File at Full → exactly `[Dim(churn_readout)]`;
  with-children File arm unchanged.
- Rust buffers: existing highlight/anchor tests updated to the new
  `new(text, "rs")` signature, behavior identical.
- Manual exit gate: `cargo run -p outrider -- .` — README.md and
  Cargo.toml render highlighted text when zoomed in; leaves are dark at
  every zoom level with no background pop.

## 8. Out of scope

Inline-markdown grammar/injections, additional languages beyond
rs/md/toml, word wrap for long prose lines (existing truncation
applies), rendering binary files, editing text pages.
