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

    for delay_secs in RETRY_DELAYS_SECS {
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

        let request = RunRequest {
            run_id: uuid::Uuid::new_v4().to_string(),
            input: input.clone(),
            session_key: session_key.clone(),
            timeout_secs: None,
            metadata: metadata.clone(),
            attachments: Vec::new(),
            pending_media: crate::gateway::media::PendingMedia::default(),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
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
                continue;
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
