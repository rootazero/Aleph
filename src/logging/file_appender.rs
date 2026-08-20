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

    /// RAII guard that restores an env var on drop (even when the body panics).
    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prev = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn test_get_log_directory() {
        // ALEPH_HOME is process-global; lock so other tests don't point it at a
        // temp directory without "aleph" in the path while we read it.
        let _lock = crate::utils::paths::ALEPH_HOME_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_scratch, scratch) = crate::utils::scratch::scratch_root();
        let tmp = scratch.join(".aleph");
        std::fs::create_dir_all(&tmp).unwrap();
        let _restore = EnvGuard::set("ALEPH_HOME", &tmp);

        let log_dir = get_log_directory().unwrap();
        assert!(log_dir.to_string_lossy().contains("aleph"));
        assert!(log_dir.to_string_lossy().contains("logs"));
    }
}