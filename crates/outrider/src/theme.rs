pub const BG: u32 = 0x1a1a1c;
pub const FILL_COLD: u32 = 0x2a2a2e;
pub const FILL_HOT: u32 = 0xb03030;
pub const TEXT_PRIMARY: u32 = 0xd8d8d8;
pub const TEXT_SECONDARY: u32 = 0x9a9a9a;
/// Adjust if this family is absent under WSLg (`fc-list | grep -i mono`).
pub const FONT_FAMILY: &str = "DejaVu Sans Mono";

fn lerp_channel(a: u32, b: u32, t: f32) -> u32 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u32 & 0xff
}

fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let r = lerp_channel((a >> 16) & 0xff, (b >> 16) & 0xff, t);
    let g = lerp_channel((a >> 8) & 0xff, (b >> 8) & 0xff, t);
    let bl = lerp_channel(a & 0xff, b & 0xff, t);
    (r << 16) | (g << 8) | bl
}

/// Neutral gray -> red, linear per-channel in sRGB.
pub fn churn_fill(churn: f32) -> u32 {
    lerp_rgb(FILL_COLD, FILL_HOT, churn.clamp(0.0, 1.0))
}

/// Border: fill lightened 12% toward white.
pub fn border_for(fill: u32) -> u32 {
    lerp_rgb(fill, 0xffffff, 0.12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn churn_endpoints_and_clamp() {
        assert_eq!(churn_fill(0.0), FILL_COLD);
        assert_eq!(churn_fill(1.0), FILL_HOT);
        assert_eq!(churn_fill(-0.5), FILL_COLD);
        assert_eq!(churn_fill(2.0), FILL_HOT);
    }

    #[test]
    fn churn_midpoint_is_channelwise() {
        // 0x2a2a2e -> 0xb03030 at t=0.5: r=(0x2a+0xb0)/2=0x6d, g=(0x2a+0x30)/2=0x2d, b=(0x2e+0x30)/2=0x2f
        assert_eq!(churn_fill(0.5), 0x6d2d2f);
    }

    #[test]
    fn border_is_lighter_than_fill() {
        let f = churn_fill(0.3);
        let b = border_for(f);
        assert!((b >> 16) & 0xff >= (f >> 16) & 0xff);
        assert!((b >> 8) & 0xff >= (f >> 8) & 0xff);
        assert!(b & 0xff >= f & 0xff);
        assert_ne!(b, f);
    }
}
