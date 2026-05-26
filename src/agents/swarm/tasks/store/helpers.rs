//! Small, pure helpers shared by [`super`] modules.

use crate::error::AlephError;

pub(super) fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
        .as_secs()
}

pub(super) fn db_err(e: impl std::fmt::Display) -> AlephError {
    AlephError::ConfigError {
        message: format!("CoordTaskStore: {e}"),
        suggestion: None,
    }
}

/// Truncate `s` on a char boundary to at most `max` chars, appending `…` when
/// truncated. Used to bound the size of result summaries embedded in events.
pub(super) fn summarize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}
