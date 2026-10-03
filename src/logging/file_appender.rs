/// Log file appender helpers — delegates to `aleph-logging` crate
use std::path::PathBuf;

use crate::logging::LoggingError;

/// Get the log directory path: `~/.aleph/logs/`
pub fn get_log_directory() -> Result<PathBuf, LoggingError> {
    aleph_logging::get_log_directory().map_err(|e| LoggingError::LogDirectory(e.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_log_directory() {
        // `AlephHomeEnvGuard` locks the same mutex every other ALEPH_HOME
        // test uses and restores the prior value on drop, so we can't race a
        // sibling pointing `$ALEPH_HOME` at a different directory while we
        // read it.
        let (_scratch, scratch) = crate::utils::scratch::scratch_root();
        let tmp = scratch.join(".aleph");
        std::fs::create_dir_all(&tmp).unwrap();
        let _restore = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(&tmp);

        let log_dir = get_log_directory().unwrap();
        assert!(log_dir.to_string_lossy().contains("aleph"));
        assert!(log_dir.to_string_lossy().contains("logs"));
    }
}
