//! Authorization and identity for the persisted thought projection.

use super::{visible_history_runs, SessionKey, SessionStore, StateDatabase};
use crate::resilience::{TaskTrace, ThoughtProvenance};
use aleph_protocol::thought::{ThoughtBlockId, ThoughtBlockRef};
use aleph_protocol::AgentTraceEvent;
use sha2::{Digest, Sha256};

/// Resolve exactly the stored text disclosed by `trace.by_runs` to this caller.
///
/// The caller identity must be scoped just as it is for the trace handler.
/// Missing, invisible, unattributed, unrecorded and ineligible blocks all return
/// `None`. A complete provider block can resolve before the whole run finishes,
/// but only once the session's own history actually attributes that run to it.
/// No caller text or generic stream reasoning can supply the source.
pub async fn resolve_thought_source(
    reference: &ThoughtBlockRef,
    db: &StateDatabase,
    sessions: &dyn SessionStore,
) -> Option<String> {
    let session = SessionKey::from_key_string(&reference.session_key)?;
    let owned_runs = visible_history_runs(sessions, &session).await?;
    if !owned_runs.contains(&reference.run_id) {
        return None;
    }
    let traces = db.get_traces_by_task(&reference.run_id).await.ok()?;
    traces.into_iter().find_map(|trace| {
        let source = eligible_source(&trace)?;
        (thought_block_id(&trace) == reference.block_id).then(|| source.to_owned())
    })
}

pub(super) fn reference_for_trace(trace: &TaskTrace, session_key: &str) -> Option<ThoughtBlockRef> {
    eligible_source(trace)?;
    Some(ThoughtBlockRef {
        session_key: session_key.to_owned(),
        run_id: trace.task_id.clone(),
        block_id: thought_block_id(trace),
    })
}

fn eligible_source(trace: &TaskTrace) -> Option<&str> {
    if trace.id <= 0 || trace.thought_provenance != Some(ThoughtProvenance::ProviderResponse) {
        return None;
    }
    match &trace.event {
        AgentTraceEvent::ReasoningEmitted { text, .. } if !text.trim().is_empty() => Some(text),
        _ => None,
    }
}

/// SQLite AUTOINCREMENT gives each persisted row a stable, non-reused identity.
/// Hash the row and run with a versioned domain so clients see no sequence/index
/// and two blocks on the same iteration remain distinct. This is not authority:
/// source resolution always rechecks session/run visibility and provenance.
pub(super) fn thought_block_id(trace: &TaskTrace) -> ThoughtBlockId {
    let mut digest = Sha256::new();
    digest.update(b"aleph:thought-block:v1\0");
    digest.update(trace.task_id.as_bytes());
    digest.update(trace.id.to_be_bytes());
    ThoughtBlockId(format!("thought_{}", hex::encode(digest.finalize())))
}
