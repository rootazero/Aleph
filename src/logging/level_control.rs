/// Dynamic log level control
///
/// This module provides runtime control over the global log level.
/// It uses an atomic variable to track the current level and allows
/// dynamic modification without restarting the application.
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Once;

use crate::logging::LoggingError;

/// Log level enumeration.
///
/// `#[repr(u8)]` matches the `AtomicU8` storage backing — `#[repr(C)]` would
/// compile to whatever the platform ABI picks (typically `c_int` = 4 bytes),
/// which would be misleading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
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

    /// Parse from string (case-insensitive, trims surrounding whitespace and
    /// surrounding quotes). Numeric strings (`"0"`..`"4"`) are accepted to
    /// match `tracing-subscriber::EnvFilter`'s accepted form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_matches('\"');
        if let Ok(n) = s.parse::<u8>() {
            // 0..=4 are the five known variants; anything else is out of
            // range (e.g. `"9"`) and should parse as `None` rather than
            // silently fall back to `Info` via `from_u8`.
            return match n {
                0 => Some(Self::Error),
                1 => Some(Self::Warn),
                2 => Some(Self::Info),
                3 => Some(Self::Debug),
                4 => Some(Self::Trace),
                _ => None,
            };
        }
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

    /// Convert from u8. Out-of-range values (memory corruption, ABI mismatch
    /// across hot-reload) fall back to `Info` and emit a single warn.
    ///
    /// On fallback, the atomic is also rewritten to `Info` so the stored
    /// value and the reported value agree; otherwise subsequent `get_log_level`
    /// calls would re-hit the corrupt byte, re-warn (suppressed by the
    /// once-guard) and keep returning `Info` while the atomic remained
    /// permanently out of range — masking the corruption indefinitely.
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Error,
            1 => Self::Warn,
            2 => Self::Info,
            3 => Self::Debug,
            4 => Self::Trace,
            _ => {
                warn_invalid_level_once(value);
                CURRENT_LOG_LEVEL.store(LogLevel::Info.to_u8(), Ordering::Release);
                Self::Info
            }
        }
    }
}

/// Global log level storage
static CURRENT_LOG_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info.to_u8());

/// Initialization guard for log level
static INIT: Once = Once::new();

/// Once-guard for the invalid-level warn so a corrupt atomic does not spam
/// the log on every `from_u8` read.
static INVALID_LEVEL_WARNED: AtomicBool = AtomicBool::new(false);

fn warn_invalid_level_once(value: u8) {
    if INVALID_LEVEL_WARNED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        tracing::warn!(value, "Invalid LogLevel u8 value, falling back to Info");
    }
}

/// Initialize the log level from environment or default.
///
/// `RUST_LOG` parsing: walk all comma-separated directives; the last match
/// wins, where a match is either (a) a plain-level directive (no `target=`
/// prefix), or (b) a `target=value` whose target names us (see
/// [`is_alephcore_target`]). Per-target entries for non-`alephcore` crates
/// (e.g. `h2=warn`) are skipped. Falls back to `Info` when nothing parses.
pub(crate) fn init_log_level() {
    INIT.call_once(|| {
        let Ok(rust_log) = std::env::var("RUST_LOG") else {
            return;
        };
        // Pick the last directive that has no `target=` prefix (the simple
        // global-level entry). Per-target entries (`h2=warn`) are skipped so
        // a global `info` from a per-crate override doesn't silently win.
        let mut chosen: Option<LogLevel> = None;
        for directive in rust_log.split(',') {
            let Some((maybe_target, maybe_value)) = directive.split_once('=') else {
                // Plain level directive.
                if let Some(lvl) = LogLevel::parse(directive) {
                    chosen = Some(lvl);
                }
                continue;
            };
            // `target=value` — only adopt it when the target names us.
            let target = maybe_target.trim();
            if !is_alephcore_target(target) {
                continue;
            }
            if let Some(lvl) = LogLevel::parse(maybe_value) {
                chosen = Some(lvl);
            }
        }
        if let Some(level) = chosen {
            CURRENT_LOG_LEVEL.store(level.to_u8(), Ordering::Release);
        }
    });
}

/// Heuristic: a directive's target refers to `alephcore` when it is the
/// crate name, the crate's library alias, an absolute path under it, or a
/// binary with the `aleph-` prefix.
fn is_alephcore_target(target: &str) -> bool {
    matches!(target, "alephcore" | "aleph" | "alephcore_lib")
        || target.starts_with("alephcore::")
        || target.starts_with("aleph_server")
        || target.starts_with("aleph-cli")
        || target.starts_with("aleph-")
}

/// Get the current log level
pub fn get_log_level() -> LogLevel {
    // Ensure the level is seeded from RUST_LOG before the first read, so the
    // reported level matches the EnvFilter the logging backend actually uses.
    // `init_log_level` is idempotent (guarded by `Once`).
    init_log_level();
    LogLevel::from_u8(CURRENT_LOG_LEVEL.load(Ordering::Acquire))
}

/// Set the log level dynamically.
///
/// Atomically swaps the reported level into the global atomic (CAS loop so
/// concurrent `set_log_level` callers cannot lose intermediate writes to
/// the audit log), and asks the shared logging backend to apply the same
/// level to the live `EnvFilter`.
///
/// Returns `Ok(())` if both the atomic and the filter were updated. If the
/// filter update fails (e.g. shared logging was never installed), returns
/// [`LoggingError::FilterUnavailable`]. The atomic is updated in both cases —
/// the contract is "reported level matches the stored value", with filter
/// availability surfaced separately so the RPC layer can warn rather than
/// silently lying about it.
pub fn set_log_level(level: LogLevel) -> Result<(), LoggingError> {
    init_log_level();
    // CAS loop: read-modify-write so concurrent setters do not interleave
    // reads/stores between themselves and lose audit context.
    let mut current = CURRENT_LOG_LEVEL.load(Ordering::Acquire);
    let next = level.to_u8();
    let old_u8 = loop {
        if current == next {
            break current;
        }
        match CURRENT_LOG_LEVEL.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(prev) => break prev,
            Err(observed) => current = observed,
        }
    };
    let old_level = LogLevel::from_u8(old_u8);
    if old_u8 != next {
        if let Err(error) = aleph_logging::set_log_level(level.to_filter_string()) {
            tracing::warn!(%error, "Runtime log filter is unavailable");
            return Err(LoggingError::FilterUnavailable(error));
        }
        tracing::info!(
            old_level = ?old_level,
            new_level = ?level,
            "Log level changed"
        );
    } else {
        tracing::trace!(
            old_level = ?old_level,
            new_level = ?level,
            "log level set was a no-op"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate the global log-level atomic. Without this
    /// guard, `cargo test`'s parallel test runner would let one test's
    /// `set_log_level` leak into the next test's `get_log_level` assertion.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

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
    fn test_log_level_parse_edge_cases() {
        // Whitespace and quoting tolerance.
        assert_eq!(LogLevel::parse("  "), None);
        assert_eq!(LogLevel::parse(" debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::parse("debug  "), Some(LogLevel::Debug));
        assert_eq!(LogLevel::parse("\"debug\""), Some(LogLevel::Debug));
        // Numeric forms (mirror tracing-subscriber).
        assert_eq!(LogLevel::parse("0"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("4"), Some(LogLevel::Trace));
        assert_eq!(LogLevel::parse("9"), None);
        // Empty string.
        assert_eq!(LogLevel::parse(""), None);
        // Compound directive must be rejected by the bare parser.
        assert_eq!(LogLevel::parse("info=debug"), None);
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
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = get_log_level();
        // The atomic is updated unconditionally; the live filter update is
        // best-effort and may report `FilterUnavailable` when the shared
        // backend was never installed (the test environment). Both outcomes
        // are correct — what we pin here is that `get_log_level` reflects
        // every successful `set_log_level` call.
        for target in [LogLevel::Debug, LogLevel::Error, LogLevel::Info] {
            let _ = set_log_level(target);
            assert_eq!(get_log_level(), target);
        }

        // Restore to whatever was set before this test ran, so we don't
        // pollute parallel tests' assumptions about the default level.
        let _ = set_log_level(prev);
    }

    #[test]
    fn test_set_log_level_always_updates_atomic() {
        // The atomic is updated unconditionally by `set_log_level`, regardless
        // of whether the live filter update succeeds. This pins the contract
        // that `get_log_level` always reflects the last `set_log_level` call
        // (the RPC layer surfaces filter-availability failures separately via
        // `LoggingError::FilterUnavailable`, which this unit test deliberately
        // does not assert — that contract is exercised in
        // `gateway/handlers/logs.rs::tests`).
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = get_log_level();
        let _ = set_log_level(LogLevel::Warn);
        assert_eq!(get_log_level(), LogLevel::Warn, "atomic updated regardless");
        let _ = set_log_level(prev);
    }

    #[test]
    fn test_default_log_level() {
        assert_eq!(LogLevel::default(), LogLevel::Info);
    }
}
