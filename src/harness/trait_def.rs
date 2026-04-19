//! Harness: the Think→Act loop driver.
//!
//! Stateless; all state lives in `SessionService`. Dependencies injected
//! at construction. One call to `run_turn` produces one Think→Act cycle.
//!
//! ```
//! use alephcore::harness::{Harness, TurnState};
//! fn _assert_object_safe(_: Box<dyn Harness>) {}
//! ```

use async_trait::async_trait;

use crate::session::service::{SessionError, SessionId};
use crate::tools::service::ToolError;

// `AiProvider` uses `crate::error::AlephError` (not a dedicated LlmError).
// We wrap it under the `Llm` variant name to match the spec intent.
use crate::error::AlephError;

#[async_trait]
pub trait Harness: Send + Sync {
    /// One Think→Act turn; returns whether the session should continue.
    async fn run_turn(&self, session_id: &SessionId) -> Result<TurnState, HarnessError>;

    /// Loop `run_turn` until Done.
    async fn run(&self, session_id: &SessionId) -> Result<(), HarnessError> {
        loop {
            match self.run_turn(session_id).await? {
                TurnState::Continue => continue,
                TurnState::Done => return Ok(()),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Continue,
    Done,
}

/// Harness-level errors.
///
/// `ToolError` has named-field struct variants so `#[from]` is not usable;
/// `AlephError` is the crate-wide error (covering LLM + provider failures).
/// Both are wrapped via explicit `From` impls below.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// LLM / provider failure (wraps `AlephError`).
    #[error("llm error: {0}")]
    Llm(AlephError),
    /// Tool dispatch failure.
    #[error("tool error: {0}")]
    Tool(ToolError),
    /// Session storage / actor failure.
    #[error("session error: {0}")]
    Session(#[from] SessionError),
    /// The run loop was externally cancelled.
    #[error("cancelled")]
    Cancelled,
}

impl From<AlephError> for HarnessError {
    fn from(e: AlephError) -> Self {
        Self::Llm(e)
    }
}

impl From<ToolError> for HarnessError {
    fn from(e: ToolError) -> Self {
        Self::Tool(e)
    }
}
