/// Dynamic log level control
///
/// This module provides runtime control over the global log level.
/// It uses an atomic variable to track the current level and allows
/// dynamic modification without restarting the application.
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Once;
use tracing::Level;

/// Log level enumeration (matches aether.udl)
///
/// Note: This type is defined in aether.udl for UniFFI code generation.
/// The Rust definition must match the UDL enum exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// Convert to tracing::Level
    pub fn to_tracing_level(&self) -> Level {
        match self {
            LogLevel::Error => Level::ERROR,
            LogLevel::Warn => Level::WARN,
            LogLevel::Info => Level::INFO,
            LogLevel::Debug => Level::DEBUG,
            LogLevel::Trace => Level::TRACE,
        }
    }

    /// Convert to EnvFilter string
    pub fn to_filter_string(&self) -> String {
        match self {
            LogLevel::Error => "error".to_string(),
            LogLevel::Warn => "warn".to_string(),
            LogLevel::Info => "info".to_string(),
            LogLevel::Debug => "debug".to_string(),
            LogLevel::Trace => "trace".to_string(),
        }
    }

    /// Parse from string (case-insensitive)
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "error" => Some(LogLevel::Error),
            "warn" | "warning" => Some(LogLevel::Warn),
            "info" => Some(LogLevel::Info),
            "debug" => Some(LogLevel::Debug),
            "trace" => Some(LogLevel::Trace),
            _ => None,
        }
    }

    /// Convert to u8 for atomic storage
    fn to_u8(&self) -> u8 {
        match self {
            LogLevel::Error => 0,
            LogLevel::Warn => 1,
            LogLevel::Info => 2,
            LogLevel::Debug => 3,
            LogLevel::Trace => 4,
        }
    }

    /// Convert from u8
    fn from_u8(value: u8) -> Self {
        match value {
            0 => LogLevel::Error,
            1 => LogLevel::Warn,
            2 => LogLevel::Info,
            3 => LogLevel::Debug,
            4 => LogLevel::Trace,
            _ => LogLevel::Info, // Default fallback
        }
    }
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

/// Global log level storage
static CURRENT_LOG_LEVEL: AtomicU8 = AtomicU8::new(2); // Default to Info (2)

/// Initialization guard for log level
static INIT: Once = Once::new();

/// Initialize the log level from environment or default
pub fn init_log_level() {
    INIT.call_once(|| {
        // Try to read from RUST_LOG environment variable
        if let Ok(rust_log) = std::env::var("RUST_LOG") {
            // Parse the log level from RUST_LOG
            // Format can be "debug", "aethecore=debug", etc.
            let level_str = rust_log
                .split(',')
                .next()
                .and_then(|s| s.split('=').last())
                .unwrap_or("info");

            if let Some(level) = LogLevel::from_str(level_str) {
                CURRENT_LOG_LEVEL.store(level.to_u8(), Ordering::SeqCst);
                tracing::debug!(level = ?level, "Initialized log level from RUST_LOG");
            }
        }
    });
}

/// Get the current log level
pub fn get_log_level() -> LogLevel {
    LogLevel::from_u8(CURRENT_LOG_LEVEL.load(Ordering::SeqCst))
}

/// Set the log level dynamically
///
/// This updates the global log level setting. Note that this affects
/// the filter directive, but the actual filtering is still controlled
/// by the EnvFilter set during initialization.
///
/// For full dynamic control, the logging system should be reinitialized
/// with the new level, or use a reload::Layer.
pub fn set_log_level(level: LogLevel) {
    let old_level = get_log_level();
    CURRENT_LOG_LEVEL.store(level.to_u8(), Ordering::SeqCst);

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
    fn test_log_level_from_str() {
        assert_eq!(LogLevel::from_str("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_str("ERROR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_str("warn"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str("warning"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::from_str("debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::from_str("trace"), Some(LogLevel::Trace));
        assert_eq!(LogLevel::from_str("invalid"), None);
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
