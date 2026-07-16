//! Visual design tokens and color-derivation functions for the treemap renderer.
//! All colors are 24-bit sRGB (`0xRRGGBB`) unless noted; higher-level modules
//! call the functions here rather than hard-coding palette values.

use outrider_index::buffer::HighlightKind;
use outrider_index::{SymbolKind, SymbolNode};

/// Window and panel background color.
pub const BG: u32 = 0x1a1a1c;
/// Churn-stripe base (neutral, zero activity).
pub const FILL_COLD: u32 = 0x2a2a2e;
/// Churn-stripe top (saturated red, maximum activity).
pub const FILL_HOT: u32 = 0xb03030;
/// Primary label and code text color.
pub const TEXT_PRIMARY: u32 = 0xd8d8d8;
/// Dimmed text for secondary labels, button glyphs, and hints.
pub const TEXT_SECONDARY: u32 = 0x9a9a9a;
/// Focused-node border accent (clearly distinct from churn fills/borders).
pub const FOCUS_BORDER: u32 = 0x4da6ff;
#[cfg(target_os = "windows")]
pub const FONT_FAMILY: &str = "Consolas";
#[cfg(target_os = "macos")]
pub const FONT_FAMILY: &str = "Menlo";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const FONT_FAMILY: &str = "DejaVu Sans Mono";
#[cfg(target_os = "windows")]
pub const FONT_FAMILY_SANS: &str = "Arial";
#[cfg(target_os = "macos")]
pub const FONT_FAMILY_SANS: &str = "Helvetica Neue";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const FONT_FAMILY_SANS: &str = "Liberation Sans";
/// Light blue for doc-description overlays (matches the focus accent family).
pub const DOC_COLOR: u32 = 0x7cb8e4;
/// Depth-shaded box fill: darker outside, lighter inside, clamped at 8.
const DEPTH_FILL_0: u32 = 0x17171B;
const DEPTH_FILL_8: u32 = 0x3C3C46;
/// Editor background for boxes that render code (Full leaf items).
pub const CODE_BG: u32 = 0x101014;

/// Deterministic identity of every theme input used by texture rendering.
pub fn fingerprint() -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    let mut update = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    for color in [
        BG,
        FILL_COLD,
        FILL_HOT,
        TEXT_PRIMARY,
        TEXT_SECONDARY,
        FOCUS_BORDER,
        DOC_COLOR,
        DEPTH_FILL_0,
        DEPTH_FILL_8,
        CODE_BG,
        TINT_DOCS,
        TINT_TEST,
        TINT_TYPEDEF,
        FILE_TINT,
        0xc586c0,
        0xdcdcaa,
        0x4ec9b0,
        0xce9178,
        0x6a9955,
        0xb5cea8,
        0x9cdcfe,
    ] {
        update(&color.to_le_bytes());
    }
    update(FONT_FAMILY.as_bytes());
    update(FONT_FAMILY_SANS.as_bytes());
    update(&TINT_BLEND.to_bits().to_le_bytes());
    update(&FILE_BLEND.to_bits().to_le_bytes());
    update(&STRIPE_W.to_bits().to_le_bytes());
    hash
}

/// Semantic tint targets (blended at TINT_BLEND toward the base fill).
const TINT_DOCS: u32 = 0x3060a0;
const TINT_TEST: u32 = 0x306030;
const TINT_TYPEDEF: u32 = 0x206060;
const TINT_BLEND: f32 = 0.12;

/// File containers get a subtle warm shift so they're visually distinct
/// from the cool-gray folder depth ramp.
const FILE_TINT: u32 = 0x443828;
const FILE_BLEND: f32 = 0.25;

/// Semantic category for box background tinting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoxTint {
    /// No tint; use the base depth/kind fill unchanged.
    Normal,
    /// Type-definition items (struct, enum, trait, interface, type alias).
    #[allow(dead_code)]
    TypeDef,
    /// Folder whose contents are predominantly documentation files.
    DocsFolder,
    /// Folder whose contents are predominantly test files.
    TestFolder,
    /// File-type tint derived from the source extension.
    FileType(u32),
}

const FILE_TYPE_BLEND: f32 = 0.35;
const ITEM_TYPE_BLEND: f32 = 0.25;
const LEAF_TYPE_BLEND: f32 = 0.18;

pub fn extension_tint(ext: &str) -> u32 {
    match ext {
        "rs" => 0xc06030,
        "js" | "mjs" | "cjs" => 0xc0b030,
        "jsx" => 0xd0a030,
        "ts" | "mts" | "cts" => 0x3080d0,
        "tsx" => 0x4070c0,
        "py" => 0x30a060,
        "rb" => 0xc03040,
        "go" => 0x30b0b0,
        "java" => 0xb04030,
        "kt" | "kts" => 0xa050c0,
        "swift" => 0xe06030,
        "c" | "h" => 0x5070a0,
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => 0x5060c0,
        "cs" => 0x7050b0,
        "html" | "htm" => 0xd05030,
        "css" => 0xc040a0,
        "scss" | "sass" | "less" => 0xb050a0,
        "vue" => 0x40b070,
        "svelte" => 0xd04020,
        "json" => 0x908040,
        "yaml" | "yml" => 0x906050,
        "toml" => 0x807060,
        "xml" => 0x808060,
        "md" | "markdown" => 0x4080c0,
        "txt" | "rst" => 0x808080,
        "sh" | "bash" | "zsh" | "fish" => 0x50a040,
        "sql" => 0xc08030,
        "lua" => 0x3040c0,
        "php" => 0x7060b0,
        "r" => 0x3060c0,
        "ex" | "exs" => 0x6040a0,
        "zig" => 0xd0a020,
        "dart" => 0x40a0c0,
        "scala" => 0xc03020,
        _ => 0x606068,
    }
}
/// Churn heat stripe width at the box's left edge.
pub const STRIPE_W: f32 = 3.0;
/// Corner radius for all box quads.
pub const CORNER_RADIUS: f32 = 4.0;

/// Linear interpolation between two 8-bit channel values.
fn lerp_channel(a: u32, b: u32, t: f32) -> u32 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u32 & 0xff
}

/// Per-channel linear blend between two packed `0xRRGGBB` colors.
fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let r = lerp_channel((a >> 16) & 0xff, (b >> 16) & 0xff, t);
    let g = lerp_channel((a >> 8) & 0xff, (b >> 8) & 0xff, t);
    let bl = lerp_channel(a & 0xff, b & 0xff, t);
    (r << 16) | (g << 8) | bl
}

/// Churn heat for the left-edge stripe: neutral gray -> red, linear per-channel in sRGB.
pub fn churn_heat(churn: f32) -> u32 {
    lerp_rgb(FILL_COLD, FILL_HOT, churn.clamp(0.0, 1.0))
}

/// Box background by nesting depth (containment read): linear ramp,
/// clamped at level 8.
pub fn depth_fill(level: u8) -> u32 {
    lerp_rgb(DEPTH_FILL_0, DEPTH_FILL_8, level.min(8) as f32 / 8.0)
}

/// Whether a box is a leaf page, a file/item container, or a folder.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoxKind {
    /// A leaf symbol page that renders source code lines.
    Leaf,
    /// A file container; uses the warm depth ramp.
    File,
    /// An item container (class, module, impl block); darker than File.
    Item,
    /// A folder container; uses the cool depth ramp.
    Folder,
}

/// Map canonical leaf status and symbol kind to a rendering tier.
pub fn node_box_kind(is_leaf: bool, symbol_kind: &SymbolKind) -> BoxKind {
    if is_leaf {
        BoxKind::Leaf
    } else {
        match symbol_kind {
            SymbolKind::Folder => BoxKind::Folder,
            SymbolKind::Item { .. } => BoxKind::Item,
            SymbolKind::File | SymbolKind::Chunk => BoxKind::File,
        }
    }
}

fn file_extension_tint(path: &str) -> BoxTint {
    let file = path.split("::").next().unwrap_or(path);
    let ext = file.rsplit('.').next().unwrap_or("");
    if ext == file {
        BoxTint::Normal
    } else {
        BoxTint::FileType(extension_tint(ext))
    }
}

/// Derive the semantic tint shared by live and cached rendering.
pub fn node_box_tint(node: &SymbolNode) -> BoxTint {
    match &node.id.kind {
        SymbolKind::Folder => match node.name.as_str() {
            "docs" | "doc" | "documentation" => BoxTint::DocsFolder,
            "test" | "tests" | "spec" | "specs" | "__tests__" => BoxTint::TestFolder,
            _ => BoxTint::Normal,
        },
        SymbolKind::Item { .. } => file_extension_tint(&node.id.qualified_path),
        SymbolKind::File | SymbolKind::Chunk => file_extension_tint(&node.name),
    }
}

/// File-container depth fill: the folder ramp shifted warm.
pub fn file_fill(level: u8) -> u32 {
    lerp_rgb(depth_fill(level), FILE_TINT, FILE_BLEND)
}

/// Item-container fill: halfway between CODE_BG and file_fill (darker than file).
pub fn item_fill(level: u8) -> u32 {
    lerp_rgb(CODE_BG, file_fill(level), 0.45)
}

/// Leaf code fill: just above CODE_BG (darkest tier).
pub fn leaf_fill(level: u8) -> u32 {
    lerp_rgb(CODE_BG, file_fill(level), 0.15)
}

/// Box background: three tiers of brightness for file-type coloring.
/// File containers are brightest, item containers (classes) are darker,
/// leaf code pages are darkest. Folder containers use the cool depth ramp.
pub fn box_fill(kind: BoxKind, level: u8, tint: BoxTint) -> u32 {
    let base = match kind {
        BoxKind::Leaf => CODE_BG,
        BoxKind::Item => item_fill(level),
        BoxKind::File => file_fill(level),
        BoxKind::Folder => depth_fill(level),
    };
    match tint {
        BoxTint::Normal => base,
        BoxTint::TypeDef => lerp_rgb(base, TINT_TYPEDEF, TINT_BLEND),
        BoxTint::DocsFolder => lerp_rgb(base, TINT_DOCS, TINT_BLEND),
        BoxTint::TestFolder => lerp_rgb(base, TINT_TEST, TINT_BLEND),
        BoxTint::FileType(color) => {
            let blend = match kind {
                BoxKind::File => FILE_TYPE_BLEND,
                BoxKind::Item => ITEM_TYPE_BLEND,
                BoxKind::Leaf => LEAF_TYPE_BLEND,
                BoxKind::Folder => FILE_TYPE_BLEND,
            };
            lerp_rgb(base, color, blend)
        }
    }
}

/// Border: fill lightened 12% toward white.
pub fn border_for(fill: u32) -> u32 {
    lerp_rgb(fill, 0xffffff, 0.12)
}

/// Ring around the four arrow-key neighbor targets, painted on top of all
/// content: translucent white (0xRRGGBBAA, use with `gpui::rgba`).
pub const NEIGHBOR_BORDER: u32 = 0xffffff80;

/// Syntax palette for Full-rung code: one color per HighlightKind,
/// legible on BG (0x1a1a1c). Default falls back to TEXT_PRIMARY.
pub fn syntax_color(kind: HighlightKind) -> u32 {
    match kind {
        HighlightKind::Keyword => 0xc586c0,
        HighlightKind::Function => 0xdcdcaa,
        HighlightKind::Type => 0x4ec9b0,
        HighlightKind::String => 0xce9178,
        HighlightKind::Comment => 0x6a9955,
        HighlightKind::Number => 0xb5cea8,
        HighlightKind::Property => 0x9cdcfe,
        HighlightKind::Default => TEXT_PRIMARY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider_index::{SymbolId, SymbolKind, SymbolNode};

    fn node(kind: SymbolKind, name: &str, qualified_path: &str) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind,
                qualified_path: qualified_path.into(),
                ordinal: 0,
            },
            name: name.into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 0,
            churn: 0.0,
            churn_count: 0,
            children: Vec::new(),
        }
    }

    #[test]
    fn python_nodes_share_extension_tint_and_use_three_brightness_tiers() {
        let file = node(SymbolKind::File, "main.py", "main.py");
        let mut item = node(
            SymbolKind::Item {
                label: "class".into(),
            },
            "Widget",
            "main.py::Widget",
        );
        item.children.push(node(
            SymbolKind::Item { label: "fn".into() },
            "method",
            "main.py::Widget::method",
        ));
        let mut leaf = node(
            SymbolKind::Item { label: "fn".into() },
            "method",
            "main.py::Widget::method",
        );
        leaf.byte_range = Some(0..10);

        let expected_tint = BoxTint::FileType(extension_tint("py"));
        assert_eq!(node_box_tint(&file), expected_tint);
        assert_eq!(node_box_tint(&item), expected_tint);
        assert_eq!(node_box_tint(&leaf), expected_tint);
        assert_eq!(node_box_kind(false, &file.id.kind), BoxKind::File);
        assert_eq!(node_box_kind(false, &item.id.kind), BoxKind::Item);
        assert_eq!(node_box_kind(true, &leaf.id.kind), BoxKind::Leaf);

        let file_fill = box_fill(node_box_kind(false, &file.id.kind), 2, node_box_tint(&file));
        let item_fill = box_fill(node_box_kind(false, &item.id.kind), 2, node_box_tint(&item));
        let leaf_fill = box_fill(node_box_kind(true, &leaf.id.kind), 2, node_box_tint(&leaf));
        assert!(brightness(file_fill) > brightness(item_fill));
        assert!(brightness(item_fill) > brightness(leaf_fill));
    }

    #[test]
    fn renderer_theme_fingerprint_is_stable_and_nonzero() {
        assert_eq!(fingerprint(), fingerprint());
        assert_ne!(fingerprint(), 0);
    }

    #[test]
    fn churn_endpoints_and_clamp() {
        assert_eq!(churn_heat(0.0), FILL_COLD);
        assert_eq!(churn_heat(1.0), FILL_HOT);
        assert_eq!(churn_heat(-0.5), FILL_COLD);
        assert_eq!(churn_heat(2.0), FILL_HOT);
    }

    #[test]
    fn churn_midpoint_is_channelwise() {
        // 0x2a2a2e -> 0xb03030 at t=0.5: r=(0x2a+0xb0)/2=0x6d, g=(0x2a+0x30)/2=0x2d, b=(0x2e+0x30)/2=0x2f
        assert_eq!(churn_heat(0.5), 0x6d2d2f);
    }

    #[test]
    fn border_is_lighter_than_fill() {
        let f = churn_heat(0.3);
        let b = border_for(f);
        assert!((b >> 16) & 0xff >= (f >> 16) & 0xff);
        assert!((b >> 8) & 0xff >= (f >> 8) & 0xff);
        assert!(b & 0xff >= f & 0xff);
        assert_ne!(b, f);
    }

    #[test]
    fn syntax_default_is_text_primary() {
        use outrider_index::buffer::HighlightKind;
        assert_eq!(syntax_color(HighlightKind::Default), TEXT_PRIMARY);
    }

    #[test]
    fn depth_fill_ramp_endpoints_midpoint_and_clamp() {
        assert_eq!(depth_fill(0), 0x17171B);
        assert_eq!(depth_fill(8), 0x3C3C46);
        assert_eq!(depth_fill(12), 0x3C3C46); // clamps at level 8
                                              // t = 0.5 per channel: r,g 23+18.5→42 (0x2a); b 27+21.5→49 (0x31)
        assert_eq!(depth_fill(4), 0x2a2a31);
    }

    #[test]
    fn box_fill_leaf_pages_are_editor_black_at_every_depth() {
        assert_eq!(box_fill(BoxKind::Leaf, 0, BoxTint::Normal), CODE_BG);
        assert_eq!(box_fill(BoxKind::Leaf, 5, BoxTint::Normal), CODE_BG);
        assert_eq!(box_fill(BoxKind::Folder, 0, BoxTint::Normal), depth_fill(0));
        assert_eq!(box_fill(BoxKind::Folder, 5, BoxTint::Normal), depth_fill(5));
    }

    #[test]
    fn box_fill_files_differ_from_folders() {
        for level in [0, 3, 8] {
            let folder = box_fill(BoxKind::Folder, level, BoxTint::Normal);
            let file = box_fill(BoxKind::File, level, BoxTint::Normal);
            let item = box_fill(BoxKind::Item, level, BoxTint::Normal);
            assert_ne!(
                file, folder,
                "file and folder fills must differ at level {level}"
            );
            assert_eq!(file, file_fill(level));
            assert_eq!(item, item_fill(level));
        }
    }

    #[test]
    fn box_fill_tints_produce_different_colors_than_normal() {
        // Folder at level 0; each non-Normal tint shifts the color.
        let normal = box_fill(BoxKind::Folder, 0, BoxTint::Normal);
        assert_ne!(box_fill(BoxKind::Folder, 0, BoxTint::TypeDef), normal);
        assert_ne!(box_fill(BoxKind::Folder, 0, BoxTint::DocsFolder), normal);
        assert_ne!(box_fill(BoxKind::Folder, 0, BoxTint::TestFolder), normal);
        // Leaf page; same contract.
        let leaf_normal = box_fill(BoxKind::Leaf, 0, BoxTint::Normal);
        assert_ne!(box_fill(BoxKind::Leaf, 0, BoxTint::TypeDef), leaf_normal);
        assert_ne!(box_fill(BoxKind::Leaf, 0, BoxTint::DocsFolder), leaf_normal);
        assert_ne!(box_fill(BoxKind::Leaf, 0, BoxTint::TestFolder), leaf_normal);
    }

    #[test]
    fn tinted_fill_border_contract() {
        // border_for must remain lighter than the tinted fill on every channel.
        for kind in [BoxKind::Folder, BoxKind::File, BoxKind::Item] {
            for tint in [
                BoxTint::TypeDef,
                BoxTint::DocsFolder,
                BoxTint::TestFolder,
                BoxTint::FileType(extension_tint("rs")),
                BoxTint::FileType(extension_tint("ts")),
                BoxTint::FileType(extension_tint("py")),
            ] {
                let fill = box_fill(kind, 2, tint);
                let border = border_for(fill);
                assert!((border >> 16) & 0xff >= (fill >> 16) & 0xff);
                assert!((border >> 8) & 0xff >= (fill >> 8) & 0xff);
                assert!(border & 0xff >= fill & 0xff);
                assert_ne!(border, fill);
            }
        }
    }

    fn brightness(color: u32) -> u32 {
        ((color >> 16) & 0xff) + ((color >> 8) & 0xff) + (color & 0xff)
    }

    #[test]
    fn file_type_three_tier_brightness() {
        for ext in ["rs", "ts", "py", "js", "go"] {
            let tint = BoxTint::FileType(extension_tint(ext));
            let file = box_fill(BoxKind::File, 2, tint);
            let item = box_fill(BoxKind::Item, 2, tint);
            let leaf = box_fill(BoxKind::Leaf, 2, tint);
            assert!(
                brightness(file) > brightness(item),
                "{ext}: file ({file:#x}) must be brighter than item ({item:#x})"
            );
            assert!(
                brightness(item) > brightness(leaf),
                "{ext}: item ({item:#x}) must be brighter than leaf ({leaf:#x})"
            );
        }
    }
}
