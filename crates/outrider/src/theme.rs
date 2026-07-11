use outrider_index::buffer::HighlightKind;

pub const BG: u32 = 0x1a1a1c;
pub const FILL_COLD: u32 = 0x2a2a2e;
pub const FILL_HOT: u32 = 0xb03030;
pub const TEXT_PRIMARY: u32 = 0xd8d8d8;
pub const TEXT_SECONDARY: u32 = 0x9a9a9a;
/// Focused-node border accent (clearly distinct from churn fills/borders).
pub const FOCUS_BORDER: u32 = 0x4da6ff;
#[cfg(target_os = "windows")]
pub const FONT_FAMILY: &str = "Consolas";
#[cfg(not(target_os = "windows"))]
pub const FONT_FAMILY: &str = "DejaVu Sans Mono";
/// Depth-shaded box fill: darker outside, lighter inside, clamped at 8.
const DEPTH_FILL_0: u32 = 0x17171B;
const DEPTH_FILL_8: u32 = 0x3C3C46;
/// Editor background for boxes that render code (Full leaf items).
pub const CODE_BG: u32 = 0x101014;
/// Churn heat stripe width at the box's left edge.
pub const STRIPE_W: f32 = 3.0;
/// Corner radius for all box quads.
pub const CORNER_RADIUS: f32 = 4.0;

fn lerp_channel(a: u32, b: u32, t: f32) -> u32 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u32 & 0xff
}

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

/// Box background: leaf pages (code or text) keep the editor background
/// at every rung — zooming in never changes a leaf's background —
/// containers use the depth ramp.
pub fn box_fill(is_leaf_page: bool, level: u8) -> u32 {
    if is_leaf_page {
        CODE_BG
    } else {
        depth_fill(level)
    }
}

/// Border: fill lightened 12% toward white.
pub fn border_for(fill: u32) -> u32 {
    lerp_rgb(fill, 0xffffff, 0.12)
}

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

/// Minimap bar color: the syntax color dimmed toward the page background so
/// the far-zoom minimap reads as texture rather than full-brightness code.
pub fn minimap_color(kind: HighlightKind) -> u32 {
    lerp_rgb(syntax_color(kind), CODE_BG, 0.50)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(box_fill(true, 0), CODE_BG);
        assert_eq!(box_fill(true, 5), CODE_BG);
        assert_eq!(box_fill(false, 0), depth_fill(0));
        assert_eq!(box_fill(false, 5), depth_fill(5));
    }

    #[test]
    fn minimap_color_dims_syntax_toward_code_bg() {
        use outrider_index::buffer::HighlightKind;
        let kw = syntax_color(HighlightKind::Keyword);
        assert_eq!(minimap_color(HighlightKind::Keyword), lerp_rgb(kw, CODE_BG, 0.50));
        assert_eq!(
            minimap_color(HighlightKind::Default),
            lerp_rgb(TEXT_PRIMARY, CODE_BG, 0.50)
        );
        // dimming moves the color: never equal to the full-brightness syntax color
        assert_ne!(minimap_color(HighlightKind::Keyword), kw);
    }
}
