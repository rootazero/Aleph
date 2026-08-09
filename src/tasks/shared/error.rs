//! Typed errors for the task services (cron, heartbeat).
//!
//! # Why this exists
//!
//! Both services used to return `Result<_, String>`, and both gateway faces
//! did the only thing a `String` allows: fold every failure into
//! `INTERNAL_ERROR`. So "you chained this job to one that does not exist",
//! "there is no job by that id" and "the disk is full" arrived at the client
//! as the same `-32603 Internal error` — a code that means *the server broke*,
//! invites a retry that will fail identically, and hides the fact that the
//! caller is the one who can fix it. The conversational face (`cron_manage`)
//! showed the model the real message all along, so the two faces of the same
//! verb disagreed about whose fault a rejection was.
//!
//! # The three answers
//!
//! The variants are not a taxonomy of what went wrong; they are the set of
//! distinct answers a caller can act on:
//!
//! - [`TaskError::NotFound`] — the thing you addressed is not there. Nothing
//!   to retry; check the id.
//! - [`TaskError::Invalid`] — the request is well-formed but describes
//!   something the scheduler refuses (a chain to a missing job, a cycle, a
//!   schedule that can never fire). Nothing to retry; change the request.
//! - [`TaskError::Internal`] — our side failed. A retry may work.
//!
//! There is deliberately **no `Conflict`** variant for "the job is disabled" /
//! "the job is already running", even though "retry later" is arguably a
//! fourth answer: no caller in the repo branches on it today, and an
//! enum variant with no consumer is the abstraction R10 says to withhold until
//! someone needs it. Those two land in `Invalid` — the request named a legal
//! job in a state that refuses it, and the message says which.
//!
//! # There is deliberately no `From<String>`
//!
//! A blanket `impl From<String> for TaskError` would make `?` work on every
//! `String`-returning helper in the subsystem, and would classify all of them
//! as whichever variant it picked. That is the bug this module exists to
//! remove, re-introduced as a language feature: the next caller error to grow
//! a `?` would silently become an internal one. Every conversion is written
//! out with [`TaskError::internal`] / [`TaskError::invalid`] at the call site,
//! so classifying is an act rather than a default.

use std::fmt;

/// A failure from a task service (`CronService`, `HeartbeatService`).
///
/// The `Display` text is exactly what the pre-typed `String` carried, so
/// callers that only interpolate the error (`format!("...: {e}")` — the tool
/// faces, the CLI, log lines) are unaffected by the type change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    /// The addressed job/task does not exist.
    NotFound(String),
    /// The request is well-formed but the scheduler refuses what it describes.
    Invalid(String),
    /// Persistence, I/O or configuration failed on our side.
    Internal(String),
}

impl TaskError {
    /// "no such job/task by that id" — the addressed object is absent.
    pub fn not_found(kind: &str, id: &str) -> Self {
        Self::NotFound(format!("{kind} not found: {id}"))
    }

    /// The caller described something the scheduler will not accept.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    /// Our side failed — a store write, a config load, an I/O call.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    /// Whether the caller can fix this by changing the request.
    ///
    /// The one question the transport mapping asks; kept here so the answer
    /// lives with the variants rather than being re-derived per surface.
    #[must_use]
    pub const fn is_caller_error(&self) -> bool {
        matches!(self, Self::NotFound(_) | Self::Invalid(_))
    }

    /// The message, without the variant.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(m) | Self::Invalid(m) | Self::Internal(m) => m,
        }
    }
}

impl fmt::Display for TaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for TaskError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every consumer outside the gateway interpolates the error into a
    /// sentence. If `Display` ever grew a `NotFound: ` prefix those sentences
    /// would read "Failed to delete job: NotFound: job not found: x".
    #[test]
    fn display_is_the_bare_message() {
        assert_eq!(
            TaskError::not_found("job", "abc").to_string(),
            "job not found: abc"
        );
        assert_eq!(
            TaskError::invalid("chain target not found: b").to_string(),
            "chain target not found: b"
        );
        assert_eq!(TaskError::internal("disk full").to_string(), "disk full");
    }

    #[test]
    fn only_internal_is_ours() {
        assert!(TaskError::not_found("job", "x").is_caller_error());
        assert!(TaskError::invalid("nope").is_caller_error());
        assert!(!TaskError::internal("nope").is_caller_error());
    }
}
