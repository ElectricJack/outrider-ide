# Multi-Language Support Design

**Status:** Approved design
**Date:** 2026-07-10
**Scope:** Add symbol extraction for C, Python, JavaScript, TypeScript, and C#. Refactor `SymbolKind` to a flexible string-label system so adding future languages never touches downstream code.

---

## 1. Purpose

Outrider currently only parses `.rs` files for symbol extraction. All other files appear as flat leaf nodes (or chunked containers for large files). This change adds five languages so the treemap shows meaningful structure for mixed-language repos.

## 2. SymbolKind Refactor

Replace the language-specific enum variants with a string label:

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Folder,
    File,
    Chunk,
    Item { label: String },
}
```

Use `String` (not `&'static str`) because `SymbolTree` derives `Deserialize` — deserialized data doesn't have `'static` lifetime. All labels originate from string literals at parse time, so the allocation cost is negligible (a few bytes per symbol).

`Folder`, `File`, and `Chunk` stay as variants — they have structural meaning in layout, rendering, and chunking. All language-level concepts (`fn`, `struct`, `class`, `interface`, `module`, `namespace`, `enum`, `trait`, `impl`, `type`, `typedef`) become `Item { label }`.

### 2.1 Migration mapping

| Old variant | New representation |
|---|---|
| `SymbolKind::Fn` | `Item { label: "fn" }` |
| `SymbolKind::Struct` | `Item { label: "struct" }` |
| `SymbolKind::Enum` | `Item { label: "enum" }` |
| `SymbolKind::Trait` | `Item { label: "trait" }` |
| `SymbolKind::Impl` | `Item { label: "impl" }` |
| `SymbolKind::Module` | `Item { label: "module" }` |

### 2.2 Downstream changes

**`content.rs` — `kind_counts()`:** Replace the `[usize; 7]` fixed array with `BTreeMap<&str, usize>`. Walk descendants; for each `Item { label }`, increment `counts[label]`. Format: join non-zero entries sorted alphabetically by label. Example output: `"2 class · 3 fn · 1 interface"`. `Chunk` keeps its existing `"part"` label via a dedicated arm.

**`content.rs` — `body_lines()`:** The `Detail`/`Full` match currently names each variant. Replace with: `Folder` arm (unchanged), `File` arm (unchanged), `Chunk` arm (unchanged), `Item { .. }` arm (the current `_` wildcard arm, which already handles the generic case with signature display).

**`pack.rs`:** Uses `SymbolKind::Chunk` for byte-range ordering. `Chunk` stays an enum variant — no change needed.

**`buffers.rs`:** Checks `node.id.kind == SymbolKind::File`. Unchanged — `File` stays a variant.

**All test fixtures:** Update `SymbolKind::Fn` → `SymbolKind::Item { label: "fn" }` etc. Mechanical replacement.

## 3. Parser Architecture

### 3.1 Structure

Each language gets a dedicated parse function in `parse.rs`. They share the existing helpers (`collect_items`, `item_signature`, `node_text`) with a language-specific `item_kind` closure that maps tree-sitter node-type strings to `&'static str` labels.

Generalize `collect_items` to accept the kind-mapping function:

```rust
fn collect_items(
    node: Node,
    src: &[u8],
    kind_fn: &dyn Fn(&str, Node, &[u8]) -> Option<&'static str>,
) -> Vec<RawItem> { ... }
```

Each language's parse function creates the parser with its grammar and calls `collect_items` with its own `kind_fn`.

### 3.2 `item_name` generalization

The current `item_name` special-cases Rust's `impl_item` (combining trait + type names). Generalize: each `kind_fn` receives the full `Node` and source, so language-specific name logic can live inside the closure or in a per-language `item_name` override. The default is `child_by_field_name("name")`.

For most languages, the default name extraction works. Language-specific overrides:

- **Rust `impl_item`**: existing `"Trait for Type"` format (unchanged)
- **Python `decorated_definition`**: unwrap to inner `function_definition` or `class_definition`
- **JS/TS arrow functions**: when a `variable_declarator` holds an `arrow_function`, use the variable name

### 3.3 Language dispatch

`index.rs::parse_all_rust()` becomes `parse_all()`. Dispatch by file extension:

| Extensions | Parser |
|---|---|
| `.rs` | `parse_rust_items()` |
| `.c`, `.h` | `parse_c_items()` |
| `.py` | `parse_python_items()` |
| `.js`, `.jsx` | `parse_js_items()` |
| `.ts`, `.tsx` | `parse_ts_items()` |
| `.cs` | `parse_csharp_items()` |

The `rs_children` parameter in `build_tree` and `scan.rs` is renamed to `parsed_children` since it's no longer Rust-specific.

### 3.4 Node-type-to-label mappings

**C:**
| Node type | Label |
|---|---|
| `function_definition` | `"fn"` |
| `struct_specifier` (with body) | `"struct"` |
| `enum_specifier` (with body) | `"enum"` |
| `type_definition` | `"typedef"` |

**Python:**
| Node type | Label |
|---|---|
| `function_definition` | `"fn"` |
| `class_definition` | `"class"` |
| `decorated_definition` | unwrap inner, use its label |

**JavaScript:**
| Node type | Label |
|---|---|
| `function_declaration` | `"fn"` |
| `class_declaration` | `"class"` |
| `method_definition` | `"fn"` |
| `generator_function_declaration` | `"fn"` |
| `export_statement` | unwrap inner, use its label |

Named arrow functions: when `collect_items` encounters a `lexical_declaration` or `variable_declaration` whose declarator's value is an `arrow_function` or `function`, emit an `Item { label: "fn" }` using the variable name.

**TypeScript:** All JS mappings, plus:
| Node type | Label |
|---|---|
| `interface_declaration` | `"interface"` |
| `enum_declaration` | `"enum"` |
| `type_alias_declaration` | `"type"` |

TS uses `tree-sitter-typescript` (separate grammar from JS). TSX uses `tree-sitter-tsx`.

**C#:**
| Node type | Label |
|---|---|
| `class_declaration` | `"class"` |
| `interface_declaration` | `"interface"` |
| `struct_declaration` | `"struct"` |
| `enum_declaration` | `"enum"` |
| `method_declaration` | `"fn"` |
| `constructor_declaration` | `"fn"` |
| `namespace_declaration` | `"namespace"` |
| `record_declaration` | `"class"` |

## 4. FileBuffer grammar support

`buffer.rs` already dispatches by extension for tree-sitter highlighting grammars. Add entries for the five new languages so Full-rung code rendering gets syntax highlighting. Each tree-sitter grammar crate ships its own `HIGHLIGHTS` query.

## 5. Dependencies

New Cargo.toml dependencies for `outrider-index`:

```toml
tree-sitter-c = "0.23"
tree-sitter-python = "0.23"
tree-sitter-javascript = "0.23"
tree-sitter-typescript = "0.23"
tree-sitter-c-sharp = "0.23"
```

Pin to latest 0.23.x (compatible with the tree-sitter version already in use). Exact version numbers will be resolved at implementation time from crates.io.

## 6. Testing

One test per language in `parse.rs`, same shape as the existing Rust test: a small representative source snippet → assert on extracted items (kinds, names, nesting, children counts).

**C test fixture:** function + struct with fields + enum.
**Python test fixture:** class with methods + standalone function + decorated function.
**JS test fixture:** function declaration + class with methods + named arrow function + export.
**TS test fixture:** JS fixture + interface + enum + type alias.
**C# test fixture:** namespace containing class with methods + interface + enum.

Update the `content.rs` count test to use `Item { label }` syntax.

No new property tests — layout and packing are kind-agnostic.

## 7. Out of Scope

- C++ support (deferred — templates, namespaces, and header/impl split add significant complexity)
- Language-specific theme colors per symbol kind (current depth-ramp coloring is kind-agnostic)
- Language detection beyond file extension (e.g. shebangs, `.h` as C vs C++)
- Header/implementation file association
