/// Dynamic log level control
///
/// This module provides runtime control over the global log level.
/// It uses an atomic variable to track the current level and allows
/// dynamic modification without restarting the application.
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Once;
use tracing::Level;

/// Log level enumeration (matches aleph.udl)
///
/// Note: This type is defined in aleph.udl for `UniFFI` code generation.
/// The Rust definition must match the UDL enum exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// Convert to `tracing::Level`
    #[must_use]
    pub const fn to_tracing_level(&self) -> Level {
        match self {
            Self::Error => Level::ERROR,
            Self::Warn => Level::WARN,
            Self::Info => Level::INFO,
            Self::Debug => Level::DEBUG,
            Self::Trace => Level::TRACE,
        }
    }

    /// Convert to `EnvFilter` string
    #[must_use]
    pub const fn to_filter_string(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    /// Parse from string (case-insensitive)
    #[must_use]
    pub const fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("error") {
            Some(Self::Error)
        } else if s.eq_ignore_ascii_case("warn") || s.eq_ignore_ascii_case("warning") {
            Some(Self::Warn)
        } else if s.eq_ignore_ascii_case("info") {
            Some(Self::Info)
        } else if s.eq_ignore_ascii_case("debug") {
            Some(Self::Debug)
        } else if s.eq_ignore_ascii_case("trace") {
            Some(Self::Trace)
        } else {
            None
        }
    }

    /// Convert to u8 for atomic storage
    const fn to_u8(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warn => 1,
            Self::Info => 2,
            Self::Debug => 3,
            Self::Trace => 4,
        }
    }

    /// Convert from u8
    const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Error,
            1 => Self::Warn,
            2 => Self::Info,
            3 => Self::Debug,
            4 => Self::Trace,
            _ => {
                // Static message only: `from_u8` is a `const fn`, and a
                // formatted `debug_assert!` ({value}) uses non-const format
                // machinery (E0015). The out-of-range byte is unrepresentable
                // via the public API anyway (round-trips `to_u8`).
                debug_assert!(false, "Invalid LogLevel u8 value");
                Self::Info
            }
        }
    }
}

/// Global log level storage
static CURRENT_LOG_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info.to_u8());

/// Initialization guard for log level
static INIT: Once = Once::new();

/// Initialize the log level from environment or default.
pub(crate) fn init_log_level() {
    INIT.call_once(|| {
        // Try to read from RUST_LOG environment variable
        if let Ok(rust_log) = std::env::var("RUST_LOG") {
            // Parse the log level from RUST_LOG
            // Format can be "debug", "alephcore=debug", etc.
            let level_str = rust_log
                .split(',')
                .next()
                .and_then(|s| s.split('=').next_back())
                .unwrap_or("info");

            if let Some(level) = LogLevel::parse(level_str) {
                CURRENT_LOG_LEVEL.store(level.to_u8(), Ordering::SeqCst);
            }
        }
    });
}

/// Get the current log level
pub fn get_log_level() -> LogLevel {
    // Ensure the level is seeded from RUST_LOG before the first read, so the
    // reported level matches the EnvFilter the logging backend actually uses.
    // `init_log_level` is idempotent (guarded by `Once`).
    init_log_level();
    LogLevel::from_u8(CURRENT_LOG_LEVEL.load(Ordering::SeqCst))
}

/// Set the log level dynamically
///
/// This updates both the reported level and the active subscriber filter when
/// shared logging has been initialized.
pub fn set_log_level(level: LogLevel) {
    // Run the one-time RUST_LOG seed before applying the explicit override, so
    // an early `set` is never clobbered by a later lazy env seed (the `Once`
    // fires here first, then the explicit store below wins).
    init_log_level();
    let old_level = get_log_level();
    CURRENT_LOG_LEVEL.store(level.to_u8(), Ordering::SeqCst);
    if let Err(error) = aleph_logging::set_log_level(level.to_filter_string()) {
        tracing::debug!(%error, "Runtime log filter is unavailable");
    }

    tracing::info!(
        old_level = ?old_level,
        new_level = ?level,
        "Log level changed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_to_tracing_level() {
        assert_eq!(LogLevel::Error.to_tracing_level(), Level::ERROR);
        assert_eq!(LogLevel::Warn.to_tracing_level(), Level::WARN);
        assert_eq!(LogLevel::Info.to_tracing_level(), Level::INFO);
        assert_eq!(LogLevel::Debug.to_tracing_level(), Level::DEBUG);
        assert_eq!(LogLevel::Trace.to_tracing_level(), Level::TRACE);
    }

    #[test]
    fn test_log_level_to_filter_string() {
        assert_eq!(LogLevel::Error.to_filter_string(), "error");
        assert_eq!(LogLevel::Warn.to_filter_string(), "warn");
        assert_eq!(LogLevel::Info.to_filter_string(), "info");
        assert_eq!(LogLevel::Debug.to_filter_string(), "debug");
        assert_eq!(LogLevel::Trace.to_filter_string(), "trace");
    }

    #[test]
    fn test_log_level_parse() {
        assert_eq!(LogLevel::parse("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("ERROR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("warn"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("warning"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse("debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::parse("trace"), Some(LogLevel::Trace));
        assert_eq!(LogLevel::parse("invalid"), None);
    }

    #[test]
    fn test_log_level_roundtrip() {
        for level in &[
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            let u8_val = level.to_u8();
            let recovered = LogLevel::from_u8(u8_val);
            assert_eq!(*level, recovered);
        }
    }

    #[test]
    fn test_get_set_log_level() {
        // Set to Debug
        set_log_level(LogLevel::Debug);
        assert_eq!(get_log_level(), LogLevel::Debug);

        // Set to Error
        set_log_level(LogLevel::Error);
        assert_eq!(get_log_level(), LogLevel::Error);

        // Set back to Info (default)
        set_log_level(LogLevel::Info);
        assert_eq!(get_log_level(), LogLevel::Info);
    }

    #[test]
    fn test_default_log_level() {
        assert_eq!(LogLevel::default(), LogLevel::Info);
    }
}
