//! The per-device detail level (ruling R7) and the per-entry override.
//!
//! `Brief` folds every settled step to one line; `Full` opens them. A click
//! or `Enter` on one step records an OVERRIDE for that entry — an XOR against
//! the level's default, the same shape the Panel already uses for tool rows
//! (`WorkspaceState.expanded_events`) — so switching level does not have to
//! rewrite every remembered choice. Persistence is the surface's (Panel
//! `localStorage`, TUI `<aleph_home>/tui-detail`); this module only decides.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailLevel {
    #[default]
    Brief,
    Full,
}

impl DetailLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Brief => "brief",
            Self::Full => "full",
        }
    }

    /// `None` for anything but the two names — an unreadable preference file
    /// falls back to the default at the CALLER, which must not read `None`
    /// as `Brief` silently without saying so.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "brief" => Some(Self::Brief),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    #[must_use]
    pub const fn default_open(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Whether the entry `id` is open under `level`, given the entries the user
/// toggled away from the level's default.
#[must_use]
pub fn effective_open(level: DetailLevel, overrides: &HashSet<String>, id: &str) -> bool {
    level.default_open() ^ overrides.contains(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn brief_is_the_default_and_round_trips_through_its_name() {
        assert_eq!(DetailLevel::default(), DetailLevel::Brief);
        for level in [DetailLevel::Brief, DetailLevel::Full] {
            assert_eq!(DetailLevel::parse(level.as_str()), Some(level));
        }
        assert_eq!(
            DetailLevel::parse("verbose"),
            None,
            "an unknown name is unknown, not Brief"
        );
    }

    #[test]
    fn an_override_flips_the_level_default_for_that_entry_only() {
        let mut overrides = HashSet::new();
        overrides.insert("step-2".to_string());
        assert!(!effective_open(DetailLevel::Brief, &overrides, "step-1"));
        assert!(effective_open(DetailLevel::Brief, &overrides, "step-2"));
        assert!(effective_open(DetailLevel::Full, &overrides, "step-1"));
        assert!(!effective_open(DetailLevel::Full, &overrides, "step-2"));
    }
}
