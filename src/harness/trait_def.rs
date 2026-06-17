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
use tokio_util::sync::CancellationToken;

use crate::harness::callback::HarnessCallback;
use crate::session::service::{SessionError, SessionId};
use crate::tools::service::ToolError;

// `AiProvider` uses `crate::error::AlephError` (not a dedicated LlmError).
// We wrap it under the `Llm` variant name to match the spec intent.
use crate::error::AlephError;

#[async_trait]
pub trait Harness: Send + Sync {
    /// One Think→Act turn; returns whether the session should continue.
    ///
    /// The `callback` receives `on_delta` / `on_tool_call` events as the turn
    /// runs. `on_complete` is fired by [`Harness::run`] when the outer loop
    /// transitions to `TurnState::Done`, not here.
    async fn run_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
    ) -> Result<TurnState, HarnessError>;

    /// Position of this harness instance in the subagent call chain.
    /// Default `None` keeps non-`AgentHarness` impls (test mocks, future
    /// alternative drivers) ergonomic. `AgentHarness` overrides to return
    /// `Some(&self.deps.chain_context)`. Stage 4 seam (#11).
    fn chain_context(&self) -> Option<&crate::harness::chain_context::ChainContext> {
        None
    }

    /// Loop `run_turn` until `Done`, firing `callback.on_complete()` on exit.
    ///
    /// `cancel` is checked before every `run_turn`; a cancelled token aborts
    /// with [`HarnessError::Cancelled`] without firing `on_complete` — the
    /// orchestrator needs to distinguish cooperative abort from natural
    /// completion.
    async fn run(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        cancel: &CancellationToken,
    ) -> Result<(), HarnessError> {
        loop {
            if cancel.is_cancelled() {
                return Err(HarnessError::Cancelled);
            }
            match self.run_turn(session_id, callback).await? {
                TurnState::Continue => continue,
                TurnState::Done => {
                    callback.on_complete();
                    return Ok(());
                }
            }
        }
    }
}

/// `#[must_use]` because dropping a `TurnState` silently loses the
/// loop-control signal — Continue and Done are not interchangeable;
/// every Think→Act caller must observe the result.
#[must_use = "TurnState carries loop-control signal; dropping it silently loses Continue/Done"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Continue,
    Done,
}

/// Outcome of one `run_turn_internal` cycle.
///
/// Replaces the former anonymous `(TurnState, usize, bool, Option<SessionId>)`
/// 4-tuple, whose bare `usize`/`bool` were destructured positionally across
/// `think.rs` and `agent.rs` — the kind of primitive obsession Rust's type
/// system lets us delete outright. Naming the fields makes the loop driver's
/// `match` arms self-documenting and stops a future fifth signal from being
/// bolted on as `.4`. Pure scaffolding: it records what the turn did, it does
/// not decide anything (R10-safe).
#[derive(Debug)]
#[must_use = "TurnStep carries the loop-control signal; dropping it loses the turn outcome"]
pub struct TurnStep {
    /// Whether the loop continues or the model finished this turn.
    pub state: TurnState,
    /// Number of tool calls actually executed this turn.
    pub executed: usize,
    /// `true` when a verifier vetoed the turn (forces Continue + retry).
    pub vetoed: bool,
    /// A forked child session id when the turn split into a sub-session.
    pub split_child: Option<SessionId>,
}

impl TurnStep {
    /// Terminal turn: the model finished, nothing further executed, no fork.
    pub const fn done() -> Self {
        Self {
            state: TurnState::Done,
            executed: 0,
            vetoed: false,
            split_child: None,
        }
    }

    /// Continue the loop after executing `executed` tool calls (no veto/fork).
    pub const fn cont(executed: usize) -> Self {
        Self {
            state: TurnState::Continue,
            executed,
            vetoed: false,
            split_child: None,
        }
    }
}

/// Identifies which sub-phase of a turn was hung when a per-turn timeout fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnPhase {
    /// LLM `process()` call was hung.
    Think,
    /// A specific tool's `execute()` call was hung.
    Act { tool_name: String },
}

impl std::fmt::Display for TurnPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Think => write!(f, "Think"),
            Self::Act { tool_name } => write!(f, "Act({tool_name})"),
        }
    }
}

/// Harness-level errors.
///
/// `ToolError` has named-field struct variants so `#[from]` is not usable;
/// `AlephError` is the crate-wide error (covering LLM + provider failures).
/// Both are wrapped via explicit `From` impls below.
///
/// `#[must_use]` (inherited from `std::error::Error` via thiserror is not
/// automatic — declare explicitly): a dropped harness error silently
/// loses both the failure cause and the `ErrorClass` classification,
/// which downstream `SessionDriver::drive` consumers need to branch on.
#[must_use = "HarnessError carries the failure cause + ErrorClass; dropping silently loses both"]
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
    /// A single Think or Act phase exceeded `turn_timeout`. The cross-turn
    /// stall watchdog (`StallTracker`) does not raise an error — it sets
    /// `TerminateReason::StallTimeout` and exits with `hit_limit=true`.
    #[error("turn stalled in {phase} after {elapsed:?}")]
    StalledTurn {
        phase: TurnPhase,
        elapsed: std::time::Duration,
    },
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

impl HarnessError {
    /// Map this error to a stable [`crate::error::ErrorClass`] for cross-cutting
    /// decisions (trace dispatch in `agent.rs`, future Guardrail / Verification
    /// consumers in Stage 5 / 6).
    ///
    /// The match is intentionally exhaustive **without** a wildcard arm so
    /// adding a new `HarnessError` variant fails at compile time until it is
    /// consciously classified. Do not "fix" a `non_exhaustive_patterns` error
    /// by adding `_ => ErrorClass::Unexpected` — pick the variant's true class
    /// instead.
    #[must_use]
    pub fn class(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;
        match self {
            Self::Llm(inner) => inner.class(),
            Self::Tool(_) => ErrorClass::Fixable,
            Self::Session(_) => ErrorClass::Unexpected,
            Self::Cancelled => ErrorClass::Recoverable,
            Self::StalledTurn { .. } => ErrorClass::Transient,
        }
    }
}

#[cfg(test)]
mod harness_error_class_tests {
    use super::{HarnessError, TurnPhase};
    use crate::error::{AlephError, ErrorClass};

    #[test]
    fn cancelled_is_recoverable() {
        assert_eq!(HarnessError::Cancelled.class(), ErrorClass::Recoverable);
    }

    #[test]
    fn stalled_turn_is_transient() {
        let e = HarnessError::StalledTurn {
            phase: TurnPhase::Think,
            elapsed: std::time::Duration::from_secs(60),
        };
        assert_eq!(e.class(), ErrorClass::Transient);
    }

    #[test]
    fn llm_delegates_to_inner() {
        let e = HarnessError::Llm(AlephError::network("net blip"));
        assert_eq!(e.class(), ErrorClass::Transient);
        let e = HarnessError::Llm(AlephError::authentication("anthropic", "bad key"));
        assert_eq!(e.class(), ErrorClass::Fixable);
    }
}
