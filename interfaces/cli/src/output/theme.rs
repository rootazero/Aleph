//! ANSI theme palette for Aleph CLI rendering.
//!
//! Pure ANSI escape strings — no extra dependency. Honors:
//! - `NO_COLOR` env var (<https://no-color.org>/)
//! - `ALEPH_COLOR=always|never|auto`
//! - stdout `IsTerminal` (no escapes when piped)
//!
//! The colour gate is cached once via `OnceLock` so `paint()` is cheap.

use std::io::IsTerminal;
use std::sync::OnceLock;

/// Semantic style tokens; mapped to ANSI sequences in [`paint`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Default body text (no escapes emitted).
    Default,
    /// Section / column headers.
    Header,
    /// Success status (✓ / ok / completed).
    Success,
    /// Error status (✗ / failed).
    Error,
    /// Warning status (⚠ / degraded).
    Warning,
    /// Informational / running / pending.
    Info,
    /// Muted / secondary detail.
    Muted,
    /// Bold emphasis (no colour).
    Bold,
}

impl Style {
    const fn sgr(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Header => "\x1b[1;36m", // bold cyan
            Self::Success => "\x1b[32m",  // green
            Self::Error => "\x1b[31m",    // red
            Self::Warning => "\x1b[33m",  // yellow
            Self::Info => "\x1b[36m",     // cyan
            Self::Muted => "\x1b[90m",    // bright black / grey
            Self::Bold => "\x1b[1m",      // bold
        }
    }
}

const RESET: &str = "\x1b[0m";

/// One-time detection of whether ANSI escapes should be emitted.
///
/// Detection order:
/// 1. `ALEPH_COLOR=always` ⇒ true
/// 2. `ALEPH_COLOR=never` or `NO_COLOR` non-empty ⇒ false
/// 3. otherwise ⇒ stdout `is_terminal()`
pub fn use_color() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(detect_color)
}

fn detect_color() -> bool {
    if let Ok(v) = std::env::var("ALEPH_COLOR") {
        match v.to_ascii_lowercase().as_str() {
            "always" | "true" | "1" => return true,
            "never" | "false" | "0" => return false,
            _ => {}
        }
    }
    if std::env::var("NO_COLOR").is_ok_and(|v| !v.is_empty()) {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// Wrap `s` in the SGR escape for `style`, or return it unchanged if colour
/// is disabled or `style == Default`.
pub fn paint(style: Style, s: &str) -> String {
    if !use_color() || style == Style::Default {
        return s.to_string();
    }
    let prefix = style.sgr();
    if prefix.is_empty() {
        return s.to_string();
    }
    format!("{prefix}{s}{RESET}")
}

/// Best-effort classification of a status word to a [`Style`].
///
/// Used by [`crate::output::print_table`] to colour status columns.
pub fn status_style(value: &str) -> Style {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "ok" | "success" | "succeeded" | "active" | "ready" | "connected" | "true" | "yes"
        | "completed" | "passed" | "online" => Style::Success,
        "error" | "failed" | "failure" | "down" | "false" | "no" | "offline" | "disconnected"
        | "crashed" => Style::Error,
        "warning" | "warn" | "degraded" | "stale" | "timeout" | "throttled" => Style::Warning,
        "running" | "pending" | "starting" | "queued" | "in_progress" | "in-progress"
        | "connecting" | "syncing" => Style::Info,
        _ => Style::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_default_is_passthrough() {
        // Default style never wraps, regardless of color gate.
        assert_eq!(paint(Style::Default, "hi"), "hi");
    }

    #[test]
    fn status_style_classifies_known_words() {
        assert_eq!(status_style("ok"), Style::Success);
        assert_eq!(status_style("FAILED"), Style::Error);
        assert_eq!(status_style("running"), Style::Info);
        assert_eq!(status_style("warn"), Style::Warning);
        assert_eq!(status_style("unknown-thing"), Style::Default);
    }

    #[test]
    fn status_style_trims_whitespace() {
        assert_eq!(status_style("  Ok  "), Style::Success);
    }

    #[test]
    fn paint_when_color_disabled_returns_input() {
        // Force ALEPH_COLOR=never via the public env. We can't reset the
        // OnceLock once seeded, so this test calls detect_color directly.
        std::env::set_var("ALEPH_COLOR", "never");
        assert!(!detect_color());
        std::env::remove_var("ALEPH_COLOR");
    }

    #[test]
    fn paint_when_color_always_emits_sgr() {
        std::env::set_var("ALEPH_COLOR", "always");
        assert!(detect_color());
        std::env::remove_var("ALEPH_COLOR");
    }

    #[test]
    fn no_color_env_disables_color() {
        std::env::set_var("NO_COLOR", "1");
        std::env::remove_var("ALEPH_COLOR");
        assert!(!detect_color());
        std::env::remove_var("NO_COLOR");
    }
}
