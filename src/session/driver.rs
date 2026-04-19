//! `SessionDriver` — the seam between a session and whatever drives its
//! Think→Act turns. Phase 4 introduces one implementation (`AgentHarness`
//! via blanket impl); Phase 5 will add others (Orchestrator, legacy
//! `AgentLoop` adapter, etc.) behind the same trait.
//!
//! Why this exists: when the runtime has picked a SessionId and handed it
//! to a driver, the driver's contract is simply "drive this session until
//! it returns to idle." Anything else (user input injection, streaming
//! deltas, result projection) happens at orchestration layers above.

use async_trait::async_trait;

use crate::error::Result;
use crate::session::service::SessionId;

#[async_trait]
pub trait SessionDriver: Send + Sync {
    /// Drive the session forward until it reaches an idle state.
    ///
    /// For `AgentHarness`, this delegates to `Harness::run` which loops
    /// `run_turn` until `TurnState::Done`. The session event log is the
    /// authoritative state — callers read it via `SessionService::get_events`
    /// after `drive` returns.
    async fn drive(&self, session_id: &SessionId) -> Result<()>;
}
