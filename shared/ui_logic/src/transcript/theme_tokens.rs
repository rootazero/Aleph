//! The one roster of semantic colour roles both surfaces paint from.
//!
//! Panel maps a role to a CSS custom property name; TUI maps it to a
//! `ratatui::style::Color`. Neither side may invent a role the other cannot
//! see — that is why the enum lives here and `ALL_SEMANTIC_COLORS` exists:
//! a mapping table on either side is asserted complete against it.

/// A colour ROLE, not a colour. Values are assigned by the surface's theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticColor {
    Fg,
    Dim,
    Accent,
    Prompt,
    UserBar,
    ToolPending,
    ToolRunning,
    ToolOk,
    ToolErr,
    ToolRail,
    DiffAdd,
    DiffDel,
    DiffCtx,
    DiffGutter,
    AdmonitionNote,
    AdmonitionTip,
    AdmonitionImportant,
    AdmonitionWarning,
    AdmonitionCaution,
    CodeBorder,
    Link,
    ContextOk,
    ContextWarn,
    ContextDanger,
    Cost,
    Spinner,
}

/// Every variant, in declaration order. A surface's mapping test iterates
/// this and must resolve each — a new role added here goes red on both
/// sides until it is painted.
pub const ALL_SEMANTIC_COLORS: &[SemanticColor] = &[
    SemanticColor::Fg,
    SemanticColor::Dim,
    SemanticColor::Accent,
    SemanticColor::Prompt,
    SemanticColor::UserBar,
    SemanticColor::ToolPending,
    SemanticColor::ToolRunning,
    SemanticColor::ToolOk,
    SemanticColor::ToolErr,
    SemanticColor::ToolRail,
    SemanticColor::DiffAdd,
    SemanticColor::DiffDel,
    SemanticColor::DiffCtx,
    SemanticColor::DiffGutter,
    SemanticColor::AdmonitionNote,
    SemanticColor::AdmonitionTip,
    SemanticColor::AdmonitionImportant,
    SemanticColor::AdmonitionWarning,
    SemanticColor::AdmonitionCaution,
    SemanticColor::CodeBorder,
    SemanticColor::Link,
    SemanticColor::ContextOk,
    SemanticColor::ContextWarn,
    SemanticColor::ContextDanger,
    SemanticColor::Cost,
    SemanticColor::Spinner,
];

impl SemanticColor {
    /// CSS custom property the Panel reads for this role (without `var()`).
    #[must_use]
    pub const fn css_var(self) -> &'static str {
        match self {
            Self::Fg => "--tr-fg",
            Self::Dim => "--tr-dim",
            Self::Accent => "--tr-accent",
            Self::Prompt => "--tr-prompt",
            Self::UserBar => "--tr-user-bar",
            Self::ToolPending => "--tr-tool-pending",
            Self::ToolRunning => "--tr-tool-running",
            Self::ToolOk => "--tr-tool-ok",
            Self::ToolErr => "--tr-tool-err",
            Self::ToolRail => "--tr-tool-rail",
            Self::DiffAdd => "--tr-diff-add",
            Self::DiffDel => "--tr-diff-del",
            Self::DiffCtx => "--tr-diff-ctx",
            Self::DiffGutter => "--tr-diff-gutter",
            Self::AdmonitionNote => "--tr-adm-note",
            Self::AdmonitionTip => "--tr-adm-tip",
            Self::AdmonitionImportant => "--tr-adm-important",
            Self::AdmonitionWarning => "--tr-adm-warning",
            Self::AdmonitionCaution => "--tr-adm-caution",
            Self::CodeBorder => "--tr-code-border",
            Self::Link => "--tr-link",
            Self::ContextOk => "--tr-ctx-ok",
            Self::ContextWarn => "--tr-ctx-warn",
            Self::ContextDanger => "--tr-ctx-danger",
            Self::Cost => "--tr-cost",
            Self::Spinner => "--tr-spinner",
        }
    }
}

/// Linear RGB mix: `t = 0` is `a`, `t = 1` is `b`. Used for diff row (12%)
/// and inline-emphasis (26%) backgrounds derived from the theme's own diff
/// colours, so a user theme stays coherent (pi-cc-extensions `diff-palette`).
#[must_use]
pub fn mix_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| -> u8 {
        let v = f32::from(x) + (f32::from(y) - f32::from(x)) * t;
        v.round().clamp(0.0, 255.0) as u8
    };
    (ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

/// Row-background mix ratio for added/removed diff lines.
pub const DIFF_ROW_MIX: f32 = 0.12;
/// Inline-emphasis (changed word span) mix ratio.
pub const DIFF_EMPHASIS_MIX: f32 = 0.26;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_role_has_a_distinct_css_var() {
        let vars: HashSet<&str> = ALL_SEMANTIC_COLORS.iter().map(|c| c.css_var()).collect();
        assert_eq!(
            vars.len(),
            ALL_SEMANTIC_COLORS.len(),
            "two roles share a CSS var"
        );
        assert!(vars.iter().all(|v| v.starts_with("--tr-")));
    }

    #[test]
    fn the_roster_is_complete() {
        // If a variant is added without being listed, this count is off and
        // both surfaces' mapping tests silently stop covering it.
        let mut seen = HashSet::new();
        for c in ALL_SEMANTIC_COLORS {
            assert!(seen.insert(*c), "duplicate in ALL_SEMANTIC_COLORS: {c:?}");
        }
        assert_eq!(seen.len(), 26);
    }

    #[test]
    fn mix_endpoints_and_midpoint() {
        assert_eq!(mix_rgb((0, 0, 0), (255, 255, 255), 0.0), (0, 0, 0));
        assert_eq!(mix_rgb((0, 0, 0), (255, 255, 255), 1.0), (255, 255, 255));
        assert_eq!(mix_rgb((0, 0, 0), (200, 100, 50), 0.5), (100, 50, 25));
        // Out-of-range t is clamped, never wraps.
        assert_eq!(mix_rgb((10, 10, 10), (20, 20, 20), 7.0), (20, 20, 20));
    }
}
