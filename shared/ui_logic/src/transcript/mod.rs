//! Transcript presentation logic shared by the Panel (Leptos) and the TUI
//! (ratatui). Everything here is `data → data`: no rendering, no signals, no
//! terminal. Each surface paints what these functions decide, so the two
//! can never disagree about a fold, a summary, a hunk or a colour role.
//!
//! Compiles under `default-features = false` (the TUI build) — nothing in
//! this module may be feature-gated or reach for leptos / web-sys.

mod theme_tokens;

pub use theme_tokens::{
    mix_rgb, SemanticColor, ALL_SEMANTIC_COLORS, DIFF_EMPHASIS_MIX, DIFF_ROW_MIX,
};

mod fold;
pub use fold::{
    fold, wrap_physical, FoldAnchor, FoldBody, FoldPolicy, Folded, DEFAULT_COLLAPSED_ROWS,
    FALLBACK_WIDTH,
};
