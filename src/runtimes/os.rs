//! OS detection and cross-OS install selection.

/// Target operating system for runtime install strategies.
///
/// Concrete variants (`MacOs`, `Linux`, `Windows`) are returned by `current()`.
/// Wildcard variants (`AnyUnix`, `AnyOs`) are only valid inside `OsInstall`
/// spec entries — they match multiple concrete OSes for DRY specs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    MacOs,
    Linux,
    Windows,
    /// Matches MacOs or Linux
    AnyUnix,
    /// Matches any concrete OS
    AnyOs,
}

impl TargetOs {
    /// Detect the current OS at runtime.
    ///
    /// Panics on unsupported OSes at compile time via `cfg!` gating.
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            panic!("unsupported OS for Aleph runtimes")
        }
    }

    /// Check whether this (possibly wildcard) target matches a concrete OS.
    pub fn matches(&self, current: TargetOs) -> bool {
        match (*self, current) {
            (Self::AnyOs, _) => true,
            (Self::AnyUnix, Self::MacOs | Self::Linux) => true,
            (a, b) => a == b,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_concrete_matches_self() {
        assert!(TargetOs::MacOs.matches(TargetOs::MacOs));
        assert!(TargetOs::Linux.matches(TargetOs::Linux));
        assert!(TargetOs::Windows.matches(TargetOs::Windows));
    }

    #[test]
    fn test_concrete_does_not_match_other_concrete() {
        assert!(!TargetOs::MacOs.matches(TargetOs::Linux));
        assert!(!TargetOs::Linux.matches(TargetOs::Windows));
        assert!(!TargetOs::Windows.matches(TargetOs::MacOs));
    }

    #[test]
    fn test_any_unix_matches_mac_and_linux() {
        assert!(TargetOs::AnyUnix.matches(TargetOs::MacOs));
        assert!(TargetOs::AnyUnix.matches(TargetOs::Linux));
        assert!(!TargetOs::AnyUnix.matches(TargetOs::Windows));
    }

    #[test]
    fn test_any_os_matches_all() {
        assert!(TargetOs::AnyOs.matches(TargetOs::MacOs));
        assert!(TargetOs::AnyOs.matches(TargetOs::Linux));
        assert!(TargetOs::AnyOs.matches(TargetOs::Windows));
    }

    #[test]
    fn test_current_returns_concrete() {
        let os = TargetOs::current();
        assert!(matches!(
            os,
            TargetOs::MacOs | TargetOs::Linux | TargetOs::Windows
        ));
    }
}
