//! Map a free-form `category` string to a CSS color expression for the node stripe.
//! Also provides `category_rgb` for WebGL renderers that need `[f32;3]` linear RGB.

use crate::memory_graph::fnv1a::fnv1a_32;

/// Returns a CSS color string. Well-known categories → curated variable;
/// anything else → deterministic `hsl(hue, 55%, 65%)`.
#[must_use]
pub fn category_color(category: &str) -> String {
    match category {
        "feedback" => "var(--cat-feedback)".to_string(),
        "project" => "var(--cat-project)".to_string(),
        "reference" => "var(--cat-reference)".to_string(),
        "user" => "var(--cat-user)".to_string(),
        "error" | "broken" | "contradiction" => "var(--cat-error)".to_string(),
        other => {
            let hue = fnv1a_32(other.as_bytes()) % 360;
            format!("hsl({hue}, 55%, 65%)")
        }
    }
}

/// Returns a linear RGB `[f32;3]` colour for WebGL renderers.
///
/// Five curated hex constants mirror the CSS `--cat-*` theme values.
/// Unknown categories use the same `hsl(hue, 55%, 65%)` rule as
/// `category_color`, keyed on the FNV-1a hash of the category string.
#[must_use]
pub fn category_rgb(category: &str) -> [f32; 3] {
    // Hex literals → sRGB [0,1] (no gamma correction — matches GPU pipeline).
    match category {
        "feedback" => hex_rgb(0xa7, 0x8b, 0xfa),  // #a78bfa
        "project" => hex_rgb(0x34, 0xd3, 0x99),   // #34d399
        "reference" => hex_rgb(0x60, 0xa5, 0xfa), // #60a5fa
        "user" => hex_rgb(0xfb, 0xbf, 0x24),      // #fbbf24
        "error" | "broken" | "contradiction" => hex_rgb(0xf4, 0x43, 0x36), // #f44336
        other => {
            // Same rule as category_color: hsl(hue, 55%, 65%).
            let hue = (fnv1a_32(other.as_bytes()) % 360) as f32;
            hsl_to_rgb(hue, 0.55, 0.65)
        }
    }
}

/// HDR boost: brighter base color → stronger bloom corona (ref: 1.2 + brightness*0.8).
#[must_use]
pub fn hdr_boost(rgb: [f32; 3]) -> [f32; 3] {
    let b = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    let k = 1.2 + b * 0.8;
    [rgb[0] * k, rgb[1] * k, rgb[2] * k]
}

#[inline]
fn hex_rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_categories_map_to_vars() {
        assert_eq!(category_color("feedback"), "var(--cat-feedback)");
        assert_eq!(category_color("project"), "var(--cat-project)");
        assert_eq!(category_color("reference"), "var(--cat-reference)");
        assert_eq!(category_color("user"), "var(--cat-user)");
        assert_eq!(category_color("error"), "var(--cat-error)");
        assert_eq!(category_color("broken"), "var(--cat-error)");
    }

    #[test]
    fn unknown_categories_use_deterministic_hsl() {
        let a = category_color("custom-xyz");
        let b = category_color("custom-xyz");
        assert_eq!(a, b);
        assert!(a.starts_with("hsl("));
    }

    #[test]
    fn category_rgb_known_values() {
        let fb = category_rgb("feedback");
        // #a78bfa → ~[0.655, 0.545, 0.980]
        assert!((fb[0] - 0xa7 as f32 / 255.0).abs() < 0.001);
        assert!((fb[1] - 0x8b as f32 / 255.0).abs() < 0.001);
        assert!((fb[2] - 0xfa as f32 / 255.0).abs() < 0.001);
    }

    #[test]
    fn category_rgb_fallback_is_gray() {
        // Kept for backward-compat reference; unknown now uses hsl→rgb (not gray).
        // The hsl path is tested in rgb_tests below.
        let c = category_rgb("unknown-xyz");
        for ch in c {
            assert!((0.0..=1.0).contains(&ch));
        }
    }
}

#[cfg(test)]
mod rgb_tests {
    use super::*;
    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 0.01, "{a} vs {b}");
    }

    #[test]
    fn known_category_matches_theme_hex() {
        // --cat-feedback: #a78bfa  → (0.655, 0.545, 0.980)
        let c = category_rgb("feedback");
        approx(c[0], 0.655);
        approx(c[1], 0.545);
        approx(c[2], 0.980);
    }

    #[test]
    fn unknown_category_is_deterministic_and_in_range() {
        let a = category_rgb("custom-xyz");
        let b = category_rgb("custom-xyz");
        assert_eq!(a, b);
        for ch in a {
            assert!((0.0..=1.0).contains(&ch));
        }
    }

    #[test]
    fn boost_brighter_for_whiter() {
        let red = hdr_boost([1.0, 0.0, 0.0]);
        let white = hdr_boost([1.0, 1.0, 1.0]);
        assert!(white.iter().sum::<f32>() / 3.0 > red.iter().sum::<f32>());
        assert!(red[0] >= 1.2); // ref floor
    }
}
