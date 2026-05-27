//! Coordinate space contract for GUI agent input.
//!
//! UI-TARS-finetuned VLMs emit clicks/drags in a normalized [0, factor_w] ×
//! [0, factor_h] space (factor defaults to 1000×1000), so the same model can
//! drive any display regardless of physical resolution or DPR. Aleph's desktop
//! capability layer expects pixel coordinates, so callers must convert before
//! dispatch.
//!
//! This module provides the type-level distinction and the pure conversion
//! math. The actual viewport lookup (which display, what dimensions) is the
//! caller's responsibility — typically resolved against
//! [`crate::DisplayInfo`] from [`ScreenCapability::display_list`].

use serde::{Deserialize, Serialize};

/// The coordinate space a caller is using to refer to on-screen positions.
///
/// `Pixel` is Aleph's native space (top-left origin, physical pixels).
/// `Normalized` mirrors UI-TARS: the model emits values in `[0, factor_w] ×
/// [0, factor_h]` and the runtime rescales to pixels at execution time.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoordinateSpace {
    /// Physical pixels, top-left origin. Default for direct/JSON callers.
    #[default]
    Pixel,
    /// Normalized [0, factor_w] × [0, factor_h]. UI-TARS uses (1000, 1000).
    Normalized { factor_w: u32, factor_h: u32 },
}

impl CoordinateSpace {
    /// UI-TARS V1.0/V1.5 default factor space.
    pub const UI_TARS_DEFAULT: Self = Self::Normalized {
        factor_w: 1000,
        factor_h: 1000,
    };

    /// Map a single (x, y) into pixel coordinates.
    ///
    /// `viewport_w` / `viewport_h` are the physical pixel dimensions of the
    /// target display. Returns the input unchanged for `Pixel` space.
    pub fn to_pixels(&self, x: f64, y: f64, viewport_w: u32, viewport_h: u32) -> (f64, f64) {
        match *self {
            Self::Pixel => (x, y),
            Self::Normalized { factor_w, factor_h } => {
                let fw = factor_w.max(1) as f64;
                let fh = factor_h.max(1) as f64;
                ((x / fw) * viewport_w as f64, (y / fh) * viewport_h as f64)
            }
        }
    }
}

/// A point in some [`CoordinateSpace`]. Newtype prevents pixel-vs-normalized
/// mix-ups at the type level.
#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub space: CoordinateSpace,
}

impl Point {
    pub fn pixel(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            space: CoordinateSpace::Pixel,
        }
    }

    pub fn normalized(x: f64, y: f64, factor_w: u32, factor_h: u32) -> Self {
        Self {
            x,
            y,
            space: CoordinateSpace::Normalized { factor_w, factor_h },
        }
    }

    /// Resolve to (pixel_x, pixel_y). For `Pixel` points the viewport is
    /// ignored and the values pass through.
    pub fn to_pixels(&self, viewport_w: u32, viewport_h: u32) -> (f64, f64) {
        self.space.to_pixels(self.x, self.y, viewport_w, viewport_h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_passthrough_ignores_viewport() {
        let cs = CoordinateSpace::Pixel;
        assert_eq!(cs.to_pixels(123.0, 456.0, 1920, 1080), (123.0, 456.0));
        assert_eq!(cs.to_pixels(123.0, 456.0, 1, 1), (123.0, 456.0));
    }

    #[test]
    fn ui_tars_default_factor_is_thousand() {
        let (x, y) = CoordinateSpace::UI_TARS_DEFAULT.to_pixels(500.0, 500.0, 1920, 1080);
        assert!((x - 960.0).abs() < 0.001);
        assert!((y - 540.0).abs() < 0.001);
    }

    #[test]
    fn normalized_scales_per_axis_independently() {
        let cs = CoordinateSpace::Normalized {
            factor_w: 1000,
            factor_h: 1000,
        };
        let (x, y) = cs.to_pixels(250.0, 750.0, 1920, 1080);
        assert!((x - 480.0).abs() < 0.001, "x got {x}");
        assert!((y - 810.0).abs() < 0.001, "y got {y}");
    }

    #[test]
    fn normalized_handles_non_square_factors() {
        let cs = CoordinateSpace::Normalized {
            factor_w: 2000,
            factor_h: 1500,
        };
        let (x, y) = cs.to_pixels(1000.0, 750.0, 1920, 1080);
        assert!((x - 960.0).abs() < 0.001);
        assert!((y - 540.0).abs() < 0.001);
    }

    #[test]
    fn zero_factor_clamps_to_one_no_div_by_zero() {
        let cs = CoordinateSpace::Normalized {
            factor_w: 0,
            factor_h: 0,
        };
        let (x, y) = cs.to_pixels(10.0, 20.0, 100, 200);
        assert_eq!((x, y), (1000.0, 4000.0));
    }

    #[test]
    fn point_pixel_roundtrip() {
        let p = Point::pixel(42.0, 84.0);
        assert_eq!(p.to_pixels(1920, 1080), (42.0, 84.0));
    }

    #[test]
    fn point_normalized_to_pixels() {
        let p = Point::normalized(500.0, 500.0, 1000, 1000);
        let (x, y) = p.to_pixels(1280, 720);
        assert!((x - 640.0).abs() < 0.001);
        assert!((y - 360.0).abs() < 0.001);
    }

    #[test]
    fn coordinate_space_serde_round_trip() {
        let cases = [
            CoordinateSpace::Pixel,
            CoordinateSpace::UI_TARS_DEFAULT,
            CoordinateSpace::Normalized {
                factor_w: 1280,
                factor_h: 720,
            },
        ];
        for cs in cases {
            let json = serde_json::to_string(&cs).expect("serialize");
            let back: CoordinateSpace = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, cs, "round trip for {cs:?}");
        }
    }
}
