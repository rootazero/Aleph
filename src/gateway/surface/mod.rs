//! Surface identity — what *kind* of I/O surface a connection is.
//!
//! This is distinct from `ChannelClass` (lane priority, see
//! `server::handler`): `SurfaceKind` names the attachment for tiering and
//! delivery routing, not scheduling. Phase 1's `DeliverySurface` trait will
//! live alongside this enum.

pub mod delivery;
pub mod desktop;
pub mod r5_router;

/// The kind of I/O surface a gateway connection presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    /// Native desktop shell (Tauri).
    Desktop,
    /// Web Panel running in a browser.
    Browser,
    /// Command-line client.
    Cli,
    /// Unspecified or legacy client that declared no kind.
    Unknown,
}

impl SurfaceKind {
    /// Parse a client-declared `channel_kind` string (case-insensitive,
    /// trimmed). Anything unrecognised or absent → `Unknown` (fail-safe: an
    /// unknown surface gets no special treatment).
    #[must_use]
    pub fn from_opt_str(s: Option<&str>) -> Self {
        match s.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("desktop") => SurfaceKind::Desktop,
            Some("browser") => SurfaceKind::Browser,
            Some("cli") => SurfaceKind::Cli,
            _ => SurfaceKind::Unknown,
        }
    }

    /// Stable wire/log string for this kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SurfaceKind::Desktop => "desktop",
            SurfaceKind::Browser => "browser",
            SurfaceKind::Cli => "cli",
            SurfaceKind::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_kinds_case_insensitively() {
        assert_eq!(
            SurfaceKind::from_opt_str(Some("desktop")),
            SurfaceKind::Desktop
        );
        assert_eq!(
            SurfaceKind::from_opt_str(Some("  Browser ")),
            SurfaceKind::Browser
        );
        assert_eq!(SurfaceKind::from_opt_str(Some("CLI")), SurfaceKind::Cli);
    }

    #[test]
    fn unknown_and_absent_map_to_unknown() {
        assert_eq!(SurfaceKind::from_opt_str(None), SurfaceKind::Unknown);
        assert_eq!(SurfaceKind::from_opt_str(Some("")), SurfaceKind::Unknown);
        assert_eq!(
            SurfaceKind::from_opt_str(Some("telegram")),
            SurfaceKind::Unknown
        );
    }

    #[test]
    fn as_str_round_trips() {
        for k in [
            SurfaceKind::Desktop,
            SurfaceKind::Browser,
            SurfaceKind::Cli,
            SurfaceKind::Unknown,
        ] {
            assert_eq!(SurfaceKind::from_opt_str(Some(k.as_str())), k);
        }
    }
}
