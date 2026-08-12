//! Per-turn plan-phase resolution — the fourth twin of
//! [`super::turn_permissions`] / [`super::turn_mode`] / [`super::turn_thinking`].
//!
//! Same session-metadata carrier, same "requested > stored" precedence, one
//! deliberate difference from all three: **there is no global rung.** The other
//! knobs have a `[policies]` default because an operator can sensibly say "this
//! install is an `Ask` install" or "this install is a chat install". Nobody can
//! sensibly say "this install is always planning": the phase is a position
//! inside one piece of work, not a posture, and a global default would put every
//! cron tick and every heartbeat into a read-only phase whose only exit needs a
//! human. So an unstamped session is [`PlanPhase::Building`], full stop.
//!
//! ## Entering, and who may
//!
//! Entering is unprivileged on purpose: the phase only ever *subtracts*, so a
//! member, a guest channel, and the model itself may all enter it, exactly as
//! any of them may pick a stricter [`ExecTier`](crate::config::types::policies::ExecTier).
//! Leaving is the guarded direction, and it is not guarded here — it is guarded
//! at the approval card (`GateRule::PlanHandoff`).
//!
//! ## Why the request rung exists at all
//!
//! A brand-new conversation has no session to stamp, and the FIRST turn is
//! precisely the one a "plan this before you touch anything" gesture is for. So
//! the phase rides on the message (`chat.send`'s `plan_phase`, same shape as
//! `exec_tier` / `mode` / `project_root`) and the server stamps it onto the
//! session it creates. The Panel carries it **only on the first message** —
//! see `MODE_SYSTEM.md`'s carriage discipline: a client that re-sends a cached
//! value on every message silently rolls back a phase the *approval* just
//! changed, which is the one rollback this feature cannot survive.

use tracing::warn;

use super::engine::ExecutionEngine;
use super::RunRequest;
use crate::config::types::policies::{PlanPhase, PLAN_PHASE_SESSION_KEY};
use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

/// Pure precedence: requested > stored > [`PlanPhase::Building`]. Split out
/// (mirroring `resolve_session_mode` / `resolve_exec_tier`) so the contract is
/// pinned by tests without a live engine.
///
/// No channel clamp and no non-operator ceiling: both exist to stop a caller
/// resolving *looser* than the install allows, and this axis has no looser
/// direction to resolve in — `Building` is where every session already is.
#[must_use]
pub(super) fn resolve_plan_phase(
    requested: Option<PlanPhase>,
    stored: Option<PlanPhase>,
) -> PlanPhase {
    requested.or(stored).unwrap_or_default()
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Resolve the plan phase for this turn, persisting a request-carried
    /// choice onto the session so it sticks across turns and reloads.
    pub(super) async fn resolve_turn_plan_phase(&self, request: &RunRequest) -> PlanPhase {
        let requested = request
            .metadata
            .get(PLAN_PHASE_SESSION_KEY)
            .map(String::as_str)
            .and_then(PlanPhase::from_id);
        let stored = self.session_plan_phase(&request.session_key).await;

        if let Some(phase) = requested.filter(|p| stored != Some(*p)) {
            self.persist_session_plan_phase(&request.session_key, phase)
                .await;
        }
        resolve_plan_phase(requested, stored)
    }

    /// Read the phase previously stamped on the session.
    ///
    /// A malformed or unknown value is ignored — but note which way that
    /// fails: it falls back to `Building`, i.e. **open**. That is the correct
    /// direction here even though it is the unusual one, because the phase is
    /// not a security boundary: it is a workflow position whose whole purpose
    /// is to be left. A session wedged read-only by a typo in its metadata,
    /// with the exit behind a card the model can no longer reach, would be a
    /// worse failure than one that resumes working.
    async fn session_plan_phase(
        &self,
        session_key: &crate::gateway::router::SessionKey,
    ) -> Option<PlanPhase> {
        let store = self.session_manager.as_ref()?;
        let meta = match store.get_metadata(session_key).await {
            Ok(meta) => meta?,
            Err(e) => {
                warn!(error = %e, "Failed to read session metadata — plan phase skipped");
                return None;
            }
        };
        let raw = meta
            .identity_meta?
            .custom
            .get(PLAN_PHASE_SESSION_KEY)?
            .as_str()?
            .to_string();
        match PlanPhase::from_id(&raw) {
            Some(phase) => Some(phase),
            None => {
                warn!(
                    value = %raw,
                    "Unknown plan phase — turn falls back to building"
                );
                None
            }
        }
    }

    /// Stamp a phase onto the session.
    ///
    /// Best-effort for the request-carried case (the phase for THIS turn is
    /// already resolved either way), and **not** best-effort for the approval
    /// case, which goes through [`PlanPhaseWriter`] instead and propagates its
    /// error — that write is the durable record of a human decision.
    async fn persist_session_plan_phase(
        &self,
        session_key: &crate::gateway::router::SessionKey,
        phase: PlanPhase,
    ) {
        if let Err(e) = write_plan_phase(self.session_manager.as_ref(), session_key, phase).await {
            warn!(error = %e, phase = phase.id(), "Failed to persist plan phase");
        }
    }
}

/// The one place a plan phase is written to a session.
///
/// Both writers go through it — the request-carried stamp above and the
/// approval-driven [`PlanPhaseWriter`] below — so "how is the phase persisted"
/// has one answer and a future third writer cannot invent a second key or a
/// second shape.
async fn write_plan_phase(
    store: Option<&Arc<dyn crate::gateway::session_store::SessionStore>>,
    session_key: &crate::gateway::router::SessionKey,
    phase: PlanPhase,
) -> Result<(), String> {
    use crate::gateway::session_store::types::SessionPatch;

    let Some(store) = store else {
        return Err("no session store is attached to this run".to_string());
    };
    let patch = SessionPatch {
        metadata: Some(serde_json::json!({ PLAN_PHASE_SESSION_KEY: phase.id() })),
        ..Default::default()
    };
    store
        .patch_session(session_key, &patch)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

use crate::sync_primitives::Arc;

/// Records an approved handoff onto the session it belongs to.
///
/// The gateway's implementation of [`PlanPhaseSink`], handed to the run's
/// [`PlanGate`] at tool-service construction. Lives here, next to the reader,
/// because a writer that does not use the same key as the reader is the
/// classic version of this bug (判据: `~/.aleph` 下的任何路径，写它的和读它的必须是同一个函数).
///
/// [`PlanPhaseSink`]: crate::tools::scoped::PlanPhaseSink
/// [`PlanGate`]: crate::tools::scoped::PlanGate
pub struct PlanPhaseWriter {
    store: Arc<dyn crate::gateway::session_store::SessionStore>,
    session_key: crate::gateway::router::SessionKey,
}

impl std::fmt::Debug for PlanPhaseWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanPhaseWriter")
            .field("session_key", &self.session_key)
            .finish_non_exhaustive()
    }
}

impl PlanPhaseWriter {
    #[must_use]
    pub fn new(
        store: Arc<dyn crate::gateway::session_store::SessionStore>,
        session_key: crate::gateway::router::SessionKey,
    ) -> Self {
        Self { store, session_key }
    }
}

#[async_trait::async_trait]
impl crate::tools::scoped::PlanPhaseSink for PlanPhaseWriter {
    async fn record_build_approved(&self) -> Result<(), String> {
        write_plan_phase(Some(&self.store), &self.session_key, PlanPhase::Building).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unstamped_sessions_are_building() {
        assert_eq!(resolve_plan_phase(None, None), PlanPhase::Building);
    }

    #[test]
    fn the_request_wins_over_the_session() {
        assert_eq!(
            resolve_plan_phase(Some(PlanPhase::Planning), Some(PlanPhase::Building)),
            PlanPhase::Planning
        );
        // And in the other direction: a client that carries `building` on a
        // planning session leaves it. That is a real gesture (the composer's
        // toggle going off), not an accident — which is exactly why the Panel
        // must not carry a cached value on every message.
        assert_eq!(
            resolve_plan_phase(Some(PlanPhase::Building), Some(PlanPhase::Planning)),
            PlanPhase::Building
        );
    }

    #[test]
    fn a_stored_phase_survives_a_message_that_carries_nothing() {
        assert_eq!(
            resolve_plan_phase(None, Some(PlanPhase::Planning)),
            PlanPhase::Planning
        );
    }
}
