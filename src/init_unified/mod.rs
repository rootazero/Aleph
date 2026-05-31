//! Unified initialization module
//!
//! Handles first-time setup including:
//! - Directory structure creation
//! - Default configuration generation
//! - Memory database initialization
//! - Runtime ledger setup
//! - Built-in skills installation

mod coordinator;
mod error;

pub use coordinator::{
    InitPhase, InitProgressHandler, InitializationCoordinator, InitializationResult,
};
pub use error::InitError;

use crate::error::Result;
use crate::utils::paths::get_config_dir;

/// Check if this is a fresh installation requiring initialization.
///
/// **Note**: This is a best-effort check. The filesystem state may change
/// between the individual `exists()` calls (TOCTOU). Callers should not
/// rely on this for strict synchronization — use [`InitializationCoordinator`]
/// for atomic initialization with proper mutual exclusion.
pub fn needs_initialization() -> Result<bool> {
    let config_dir = get_config_dir()?;

    // Check essential markers
    let has_config = config_dir.join("config.toml").exists();
    let has_manifest = config_dir.join("runtimes").join("manifest.json").exists()
        || config_dir.join("runtimes").join("ledger.json").exists();
    let has_database = config_dir.join("memory.db").exists();

    Ok(!has_config || !has_manifest || !has_database)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_phase_name_returns_correct_str() {
        assert_eq!(InitPhase::Directories.name(), "directories");
        assert_eq!(InitPhase::Config.name(), "config");
        assert_eq!(InitPhase::Database.name(), "database");
        assert_eq!(InitPhase::Runtimes.name(), "runtimes");
        assert_eq!(InitPhase::Skills.name(), "skills");
    }

    #[test]
    fn init_phase_display_name_returns_human_readable() {
        assert_eq!(
            InitPhase::Directories.display_name(),
            "Creating directories"
        );
        assert_eq!(InitPhase::Config.display_name(), "Generating configuration");
        assert_eq!(InitPhase::Database.display_name(), "Initializing database");
        assert_eq!(InitPhase::Runtimes.display_name(), "Installing runtimes");
        assert_eq!(InitPhase::Skills.display_name(), "Installing skills");
    }

    #[test]
    fn init_error_new_defaults_to_retryable() {
        let err = InitError::new("test_phase", "something went wrong");
        assert_eq!(err.phase, "test_phase");
        assert_eq!(err.message, "something went wrong");
        assert!(err.is_retryable);
    }

    #[test]
    fn init_error_non_retryable_sets_flag_correctly() {
        let err = InitError::non_retryable("setup", "fatal");
        assert_eq!(err.phase, "setup");
        assert_eq!(err.message, "fatal");
        assert!(!err.is_retryable);
    }

    #[test]
    fn init_error_display_format() {
        let err = InitError::new("config", "missing field");
        let display = format!("{}", err);
        assert_eq!(display, "[config] missing field");
    }
}
