//! The delivery ladder every proactive-arrival announce shares.
//!
//! Two subsystems finish work that outlives the run which started it —
//! background sub-agents ([`super::subagent_announce`]) and background `bash`
//! jobs ([`super::process_announce`]) — and both owe the same thing when they
//! do: *somebody has to look*. The failure they close is one failure, recorded
//! in the sub-agent module's own words: "If the parent's run had already ended,
//! nobody would ever look — the user never heard back (an R5 violation)."
//!
//! The ladder itself is four decisions, and neither subsystem gets to answer
//! them differently:
//!
//! - **already collected?** — the model that polled the result itself has been
//!   told just as surely as one that received an announce; delivering again
//!   spends a whole parent turn re-stating what it already folded in. Rechecked
//!   after every retry sleep, not once up front: the reason a parent is busy is
//!   very often that it is parked in the very `wait` that collects this result.
//! - **is the parent addressable?** — an unparseable session key or an
//!   unregistered agent is a skip, never a panic and never a broadcast to
//!   whoever happens to be listening. The work stays poll-able.
//! - **idle → a fresh run; mid-run → steering.** Both are `adapter.execute`:
//!   `ExecutionEngine::execute`'s busy-input path absorbs the notice into the
//!   live run at its next turn boundary rather than spawning a second one.
//! - **busy elsewhere → bounded retries**, then give up quietly and leave the
//!   result reachable through the tool that produced it.
//! - **may its owner still act?** — fire-time authority (round 11), re-asked
//!   on every attempt; refused ends the ladder, unknown waits on its own
//!   bounded budget and never spends a busy retry.
//!
//! ⚠️ **Known gap — ATTRIBUTION only: the person checked is the parent
//! session's OWNER, not the initiator** (R-b / R-d). An announce run carries
//! no `AUTHOR_USER_KEY`: the completion events (`SubAgentCompletionEvent`,
//! `ProcessCompletionEvent`) and the orphan sidecar
//! (`agents::background_persistence`'s `state.json`) record no author. In a
//! project room the owner is the room's CREATOR, so a member's finished
//! sub-agent or bash job is judged by the creator's STATUS: refused if the
//! creator was deactivated, delivered if only the member was, and spend and
//! the ledger are charged to the creator. On a multi-user server its ROLE is
//! no longer the creator's: `fire_gate::authorize_session_run` caps a room
//! run with no author and no carried role at `member` (final review I1), and
//! the browser face composes no one's profile for a room run with no speaker
//! (`visibility::run_principal`). On a single-user install (nobody but the
//! machine owner in the users table) the floor does not apply: there is no
//! member whose work the creator's grant could carry. Closing the
//! rest means capturing `scope::current_room_author()` at the two spawn sites
//! (`agents::subagent_tool::spawn`, `builtin_tools::bash_exec` →
//! `builtin_tools::process_completion`), carrying it on both event types and
//! on the sidecar record (serde-default fields — no database migration), and
//! stamping it into the base metadata below. Personal sessions (one human)
//! are not affected.
//!
//! What stays with each caller is exactly what differs: which event it listens
//! for, what the notice says, what "already collected" means for its own
//! bookkeeping, and where the durable "the parent knows" stamp lives. This
//! module owns the ladder; it owns no vocabulary of either subsystem.
//!
//! No reasoning happens here (R10): the harness only delivers. What to do with
//! the result is decided by the parent agent's own turn.

use std::collections::HashMap;

use tracing::{debug, info, warn};

use crate::event::{EventFilter, EventType, GlobalBus, GlobalEvent};
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::event_emitter::{EventEmitter, GatewayEventEmitter};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;

/// Busy-retry schedule (seconds before each attempt). The first attempt is
/// immediate; later ones give a busy parent time to free its run slot.
const RETRY_DELAYS_SECS: [u64; 3] = [0, 30, 120];

/// Wait before re-asking an authority that could not be established. An
/// unknown answer is not a busy parent: it spends none of
/// [`RETRY_DELAYS_SECS`] (ruling a), and has this budget of its own instead.
const AUTHORITY_UNKNOWN_WAIT_SECS: u64 = 30;

/// How many times an unknown authority is re-asked before the ladder gives up
/// and leaves the result where [`Announcement::fallback`] says it is — five
/// minutes at [`AUTHORITY_UNKNOWN_WAIT_SECS`]. A bound so a users store that
/// stays down does not pin one waiting task per finished unit forever.
const AUTHORITY_UNKNOWN_MAX_WAITS: u32 = 10;

/// One completion, ready to be delivered into the session that owns it.
pub(crate) struct Announcement {
    /// Identifier of the finished unit — logged, and stamped on the driven run
    /// under [`metadata_key`](Self::metadata_key).
    pub key: String,
    /// Run-metadata key for the announce (e.g. `"subagent_announce"`), so the
    /// engine can tag the turn it opens.
    pub metadata_key: &'static str,
    /// `GlobalEvent::source_session_id` — the parent session's key string.
    pub session_id: String,
    /// The `[system]` notice the parent's turn reads.
    pub input: String,
    /// Short label for logs (`"subagent announce"`).
    pub kind: &'static str,
    /// Where the result remains reachable when delivery is skipped or fails.
    /// Named in the log lines, because "we gave up" without "and here is where
    /// it still is" is how a recoverable state reads as a lost one.
    pub fallback: &'static str,
    /// "The parent already knows" — checked before **every** attempt.
    pub already_delivered: Box<dyn Fn() -> bool + Send + Sync>,
    /// Durable "the parent knows now", run once on a successful delivery.
    /// Without it a restart inside the retry ladder re-delivers forever, or
    /// (worse) withdraws the announcement promised at spawn in silence.
    pub on_delivered: Box<dyn Fn() + Send + Sync>,
}

/// Register a global-bus subscriber for one announce family.
///
/// **Awaited, not spawned**, and that is the whole reason this is shared: boot
/// broadcasts reconciled completions of its own right after registration, and a
/// subscriber that is merely *scheduled* is a subscriber that is not listening
/// yet (§9 — a mechanism that builds state from an event stream must reconcile
/// *after* subscribing). Both callers inherit that ordering instead of each
/// re-deriving it.
pub(crate) async fn subscribe<F>(event_type: EventType, kind: &'static str, on_event: F)
where
    F: Fn(GlobalEvent) + Send + Sync + 'static,
{
    // The returned id is a handle, not a guard: dropping it does not
    // unsubscribe (see `GlobalBus::unsubscribe`), so this subscription lives
    // for the process.
    let _sub = GlobalBus::global()
        .subscribe_async(EventFilter::new(vec![event_type]), on_event)
        .await;
    info!(announce = kind, "Announce subscriber registered");
}

/// One announce attempt's `RunRequest`, built FROM the metadata the fire-time
/// grant admitted for the parent session `row` (round 11, N9; ruling b):
/// `deliver` executes exactly the request returned here, so there is no
/// second copy of `base` a stamp could miss. Asked afresh on every attempt.
/// `multi_user` is the room floor's mode (`fire_gate::authorize_session_run`).
pub(crate) fn admit_announce<R>(
    resolve: R,
    multi_user: impl FnOnce() -> bool,
    row: Option<&crate::gateway::session_store::types::SessionMetadata>,
    base: &HashMap<String, String>,
    input: &str,
    session_key: &SessionKey,
) -> Result<RunRequest, crate::gateway::fire_gate::FireStop>
where
    R: FnOnce(crate::scope::authority::FireSubject<'_>) -> crate::scope::authority::FireAuthority,
{
    let metadata =
        crate::gateway::fire_gate::admit_session_metadata(resolve, multi_user, row, base.clone())?;
    Ok(RunRequest {
        run_id: uuid::Uuid::new_v4().to_string(),
        input: input.to_string(),
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata,
        attachments: Vec::new(),
        pending_media: crate::gateway::media::PendingMedia::default(),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    })
}

/// Drive one completion into its parent session.
pub(crate) async fn deliver(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    event_bus: Arc<GatewayEventBus>,
    announcement: Announcement,
) {
    let Announcement {
        key,
        metadata_key,
        session_id,
        input,
        kind,
        fallback,
        already_delivered,
        on_delivered,
    } = announcement;

    // Dedup with the on-demand paths: if the parent already saw this result by
    // asking for it, skip rather than spending a fresh parent turn re-delivering
    // what the model has already folded in. Pure data check — no reasoning,
    // R7/R10 clean.
    if already_delivered() {
        debug!(
            announce = kind,
            key = %key,
            "result already collected on demand; skipping proactive delivery"
        );
        return;
    }

    let Some(session_key) = SessionKey::from_key_string(&session_id) else {
        debug!(
            announce = kind,
            session = %session_id,
            key = %key,
            "parent session key not parseable; {fallback}"
        );
        return;
    };

    let agent_id = session_key.agent_id().to_string();
    let Some(agent) = registry.get(&agent_id).await else {
        warn!(
            announce = kind,
            agent_id = %agent_id,
            key = %key,
            "parent agent not registered; {fallback}"
        );
        return;
    };

    // Mirror handlers::agent — Panel stream as base, fan the final reply out
    // to the session's origin channel when one is bound.
    let base: Arc<dyn EventEmitter + Send + Sync> =
        Arc::new(GatewayEventEmitter::new(event_bus.clone()));
    let emitter: Arc<dyn EventEmitter + Send + Sync> = match (
        agent.origin_route(&session_key).await,
        crate::gateway::event_emitter::origin_fanout::channel_registry(),
    ) {
        (Some((origin_channel, origin_conversation)), Some(channel_registry)) => Arc::new(
            crate::gateway::event_emitter::origin_fanout::OriginFanoutEmitter::new(
                base,
                channel_registry,
                origin_channel,
                origin_conversation,
            ),
        ),
        _ => base,
    };

    let mut metadata = HashMap::new();
    metadata.insert(metadata_key.to_string(), key.clone());
    // Unattended, like cron (round 11, N9): an announce run fires precisely
    // when the person has walked away, so an approval-gated tool must fail
    // closed instead of parking on a card nobody answers.
    metadata.insert(
        crate::gateway::execution_engine::UNATTENDED_KEY.to_string(),
        "true".to_string(),
    );

    // `slot` walks `RETRY_DELAYS_SECS` and advances ONLY on a busy parent. An
    // unknown authority retries the same slot after its own wait (M5).
    let mut slot = 0usize;
    let mut delay_secs = RETRY_DELAYS_SECS[0];
    let mut unknown_waits = 0u32;
    loop {
        if delay_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
            // Re-check the dedup guard after every wait, not just once up front.
            // The retry schedule spans over two minutes, and the reason the
            // parent is busy is very often that it is parked in the very `wait`
            // that returns this result and marks it collected. Checking only
            // before the loop meant the announce woke up minutes later and spent
            // a fresh parent turn re-delivering something the model already had.
            if already_delivered() {
                debug!(
                    announce = kind,
                    key = %key,
                    "result collected while waiting to retry; skipping delivery"
                );
                return;
            }
        }

        // Fire-time authority, asked on EVERY attempt (round 11, N9): the
        // parent session's owner may be deactivated or demoted during the
        // two-minute retry schedule. Granted stamps the scope pair (the run
        // used to carry none) and the member ceiling; Unknown waits and asks
        // again without spending a busy retry (R-a). The person checked is
        // the row's OWNER — see the module doc's known gap: no initiator
        // rides an announce.
        let admitted = match agent.session_store().get_metadata(&session_key).await {
            Err(e) => Err(crate::gateway::fire_gate::FireStop::Unknown(format!(
                "parent session row unreadable: {e}"
            ))),
            Ok(row) => admit_announce(
                |subject| crate::scope::authority::resolve(&subject),
                crate::gateway::security::store::slot::multi_user,
                row.as_ref(),
                &metadata,
                &input,
                &session_key,
            ),
        };
        let request = match admitted {
            Ok(request) => request,
            Err(crate::gateway::fire_gate::FireStop::Refused(reason)) => {
                warn!(
                    announce = kind,
                    key = %key,
                    session = %session_id,
                    reason = %reason,
                    "announce refused: the parent session's owner may no longer act; {fallback}"
                );
                return;
            }
            Err(crate::gateway::fire_gate::FireStop::Unknown(reason)) => {
                unknown_waits += 1;
                if unknown_waits > AUTHORITY_UNKNOWN_MAX_WAITS {
                    warn!(
                        announce = kind,
                        key = %key,
                        session = %session_id,
                        reason = %reason,
                        waits = AUTHORITY_UNKNOWN_MAX_WAITS,
                        "announce authority stayed unknown; {fallback}"
                    );
                    return;
                }
                warn!(
                    announce = kind,
                    key = %key,
                    session = %session_id,
                    reason = %reason,
                    "announce authority unknown; asking again without spending a busy retry"
                );
                delay_secs = AUTHORITY_UNKNOWN_WAIT_SECS;
                continue;
            }
        };

        match adapter
            .execute(request, agent.clone(), emitter.clone())
            .await
        {
            // Ok covers both "fresh announce run completed" and "absorbed by
            // the live run as steering" — either way the parent saw it.
            Ok(()) => {
                on_delivered();
                debug!(
                    announce = kind,
                    key = %key,
                    session = %session_id,
                    "announce delivered to parent session"
                );
                return;
            }
            Err(ExecutionError::AgentBusy(_)) => {
                slot += 1;
                match RETRY_DELAYS_SECS.get(slot) {
                    Some(&next) => {
                        delay_secs = next;
                        continue;
                    }
                    None => break,
                }
            }
            Err(e) => {
                warn!(
                    announce = kind,
                    key = %key,
                    session = %session_id,
                    error = %e,
                    "announce run failed; {fallback}"
                );
                return;
            }
        }
    }

    warn!(
        announce = kind,
        key = %key,
        session = %session_id,
        "parent stayed busy through all retries; {fallback}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::{SecurityStore, UserRole};
    use crate::scope::authority::resolve_with;

    /// Ruling (b) at the announce site: the request `admit_announce` hands
    /// back — the one `deliver` executes — carries the grant resolved for the
    /// parent row, plus the base keys. A member's PERSONAL session, so the
    /// member ceiling (not the room floor) supplies `caller_role`.
    #[test]
    fn the_announce_request_carries_the_resolved_grant() {
        let users = SecurityStore::in_memory().unwrap();
        users.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        let row = crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some("u-bob".into()),
            scope_id: Some("personal:u-bob".into()),
            ..Default::default()
        };
        let key = SessionKey::Main {
            agent_id: "main".to_string(),
            main_key: crate::routing::session_key::DEFAULT_MAIN_KEY.to_string(),
            epoch: 0,
        };
        let mut base = HashMap::new();
        base.insert("subagent_announce".to_string(), "sub-1".to_string());
        let request = admit_announce(
            |s| resolve_with(Some(&users), &s),
            || true,
            Some(&row),
            &base,
            "[system] done",
            &key,
        )
        .expect("an active member's announce is admitted");
        assert_eq!(
            request.metadata.get("caller_role").map(String::as_str),
            Some("member")
        );
        assert_eq!(
            request
                .metadata
                .get("subagent_announce")
                .map(String::as_str),
            Some("sub-1")
        );
        assert_eq!(request.input, "[system] done");
    }
}
