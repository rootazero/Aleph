//! Small, pure helpers shared by [`super`] modules.

use crate::error::AlephError;

pub(super) fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
        .as_secs()
}

/// Store-internal error taxonomy: a DOMAIN rejection is not a STORAGE
/// failure.
///
/// The trait's public error type (`AlephError`) is crate-wide and its variant
/// list is pinned by `crates/aleph-slot-api/src/error.rs`, so the store cannot
/// widen it without a semver proposal. What the store CAN do is stop
/// conflating two failures that mean opposite things to a caller:
///
/// * [`CoordStoreError::NotFound`] / [`CoordStoreError::Locked`] — domain
///   rejections. The row is not there, or another agent holds the lock.
///   Retrying changes nothing; the caller must give up or wait.
/// * [`CoordStoreError::Storage`] — a genuine storage failure (the SQLite
///   call or a serialisation blew up). This one may be transient.
///
/// The distinction survives the conversion to the public type as a stable
/// message prefix (`not found:` / `locked:` / `storage:`), so a caller that
/// only sees `AlephError` can still branch on the KIND of failure without an
/// API change. The `AlephError` variant itself stays `ConfigError` — the
/// public surface of the trait methods is unchanged (no semver impact).
#[derive(Debug, thiserror::Error)]
pub(super) enum CoordStoreError {
    /// The addressed row does not exist.
    #[error("not found: {what}")]
    NotFound { what: String },
    /// The row exists but is held by another claimer's lock.
    #[error("locked: {what} is held by {holder}")]
    Locked { what: String, holder: String },
    /// The storage layer itself failed.
    #[error("storage: {message}")]
    Storage { message: String },
}

impl From<CoordStoreError> for AlephError {
    fn from(err: CoordStoreError) -> Self {
        AlephError::ConfigError {
            message: format!("CoordTaskStore: {err}"),
            suggestion: None,
        }
    }
}

/// A storage-layer failure (SQLite error, serialisation failure, …).
pub(super) fn db_err(e: impl std::fmt::Display) -> AlephError {
    CoordStoreError::Storage {
        message: e.to_string(),
    }
    .into()
}

/// A domain rejection: the addressed row does not exist.
pub(super) fn not_found_err(what: &str) -> AlephError {
    CoordStoreError::NotFound {
        what: what.to_string(),
    }
    .into()
}

/// A domain rejection: the row exists but another claimer holds its lock.
pub(super) fn locked_err(what: &str, holder: &str) -> AlephError {
    CoordStoreError::Locked {
        what: what.to_string(),
        holder: holder.to_string(),
    }
    .into()
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
