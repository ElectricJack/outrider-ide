# Semantic Box Tinting

Tint container and leaf backgrounds by semantic category so folder purpose and type definitions are visible from a distance.

## BoxTint enum (theme.rs)

```rust
pub enum BoxTint { Normal, TypeDef, DocsFolder, TestFolder }
```

## Tint constants (theme.rs)

```rust
const TINT_DOCS: u32 = 0x3060a0;
const TINT_TEST: u32 = 0x306030;
const TINT_TYPEDEF: u32 = 0x206060;
const TINT_BLEND: f32 = 0.12;
```

## box_fill signature change (theme.rs)

```rust
pub fn box_fill(is_leaf_page: bool, level: u8, tint: BoxTint) -> u32 {
    let base = if is_leaf_page { CODE_BG } else { depth_fill(level) };
    let target = match tint {
        BoxTint::Normal => return base,
        BoxTint::TypeDef => TINT_TYPEDEF,
        BoxTint::DocsFolder => TINT_DOCS,
        BoxTint::TestFolder => TINT_TEST,
    };
    lerp_rgb(base, target, TINT_BLEND)
}
```

## Classification function (treemap.rs)

```rust
fn classify_tint(node: &SymbolNode) -> theme::BoxTint {
    match &node.id.kind {
        SymbolKind::Folder => {
            match node.name.as_str() {
                "docs" | "doc" | "documentation" => theme::BoxTint::DocsFolder,
                "test" | "tests" | "spec" | "specs" | "__tests__" => theme::BoxTint::TestFolder,
                _ => theme::BoxTint::Normal,
            }
        }
        SymbolKind::Item { label } => {
            match label.as_str() {
                "struct" | "enum" | "trait" | "class" | "interface" | "type" | "typedef"
                    => theme::BoxTint::TypeDef,
                _ => theme::BoxTint::Normal,
            }
        }
        _ => theme::BoxTint::Normal,
    }
}
```

## Call site change (treemap.rs paint_items)

Replace:
```rust
let fill = theme::box_fill(is_leaf, item.level);
```
With:
```rust
let tint = classify_tint(item.node);
let fill = theme::box_fill(is_leaf, item.level, tint);
```

## Tests

- `box_fill` with each tint variant produces a different color than Normal
- `box_fill` with Normal is unchanged from current behavior
- `classify_tint` returns correct variant for each category
- Tinted fills are still darker than their borders (`border_for` contract)

## Global Constraints

- No changes outside theme.rs and treemap.rs
- Existing test assertions for `box_fill(true/false, level)` must be updated to pass `BoxTint::Normal` — behavior unchanged
- Tint blend at 0.12 — subtle, not saturated
