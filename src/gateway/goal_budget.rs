//! Tree token accounting for a budgeted goal (tree budget v1 — codex
//! `RolloutBudget` mapped onto A3's single persistent source).
//!
//! codex shares one in-memory `Arc<RolloutBudget>` across an agent tree;
//! Aleph deliberately has no per-tree control handle — the durable source of
//! token truth is `SessionStore::get_total_tokens`, so the tree total is
//! *derived*: the goal session's own live total plus each enrolled
//! delegation member's spend since it joined (`tokens_at_join` baseline,
//! stamped at enrollment). The sum feeds the EXISTING `Goal::over_budget`
//! channel unchanged, so enforcement stays at the run-granular continuation
//! hook and the delegation seam — never mid-turn (a mid-sampling hard abort
//! would need the think loop, R10).
//!
//! Enrollment covers every delegation surface through one shared seam
//! ([`check_and_enroll_delegation`] / [`tree_budget_refusal`]):
//! - `session_send` and `team_delegate` — interactive, refuse-then-enroll (a
//!   model turn is there to receive the F9 compact refusal);
//! - the autonomous team dispatcher — account-only enrollment, keyed off the
//!   `origin_session` metadata anchor a task-creation tool stamps via
//!   [`with_origin_session`] (no model turn ⇒ no refusal; the leader's own
//!   pursuit budget still stops the tree at the continuation hook).
//!
//! Consumers of the derived total: `goal_continuation::live_tokens` (budget
//! enforcement per claim) and the delegation seam above.

use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::goal::Goal;
use crate::sync_primitives::Arc;
use tracing::warn;

/// Metadata key carrying the session that CREATED a coordination task, so the
/// autonomous dispatcher — which runs with no turn context — can still enroll
/// the task's child run into the creating session's goal tree budget. Stamped
/// at task-creation tool sites (where the turn context is live) via
/// [`with_origin_session`], read back in the dispatcher via
/// [`origin_session_from_metadata`].
pub const ORIGIN_SESSION_METADATA_KEY: &str = "origin_session";

/// Outcome of a delegation's tree-budget check + enrollment.
pub enum DelegationBudget {
    /// Proceed — the child was enrolled (or there is no active budgeted goal on
    /// the delegating session, so nothing to account).
    Proceed,
    /// Refuse — the shared tree budget is exhausted. Carries a model-facing
    /// reason (only returned when `refuse_when_over` is set).
    Refused(String),
}

/// Wall-clock now (Unix epoch ms).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Tree-budget seam for ANY delegation that spawns a child session — the single
/// shared implementation behind `session_send`, `team_delegate`, and the
/// autonomous team dispatcher (mirrors the original `session_send` block).
///
/// `caller_session` is the delegating (goal-owning) session key string;
/// `child_key` is the GATEWAY session key the child run executes and accrues
/// tokens under (never a routing target key, which collapses differently for
/// DM/Group and would leave `tree_tokens` reading the wrong row).
///
/// `refuse_when_over`: `true` for an interactive delegation that has a model
/// turn to receive the refusal (`session_send`, `team_delegate`) — returns
/// [`DelegationBudget::Refused`] when the tree budget is already spent, BEFORE
/// enrolling. `false` for the autonomous dispatcher (no model turn to act on a
/// refusal) — only accounts (always enrolls, never refuses).
///
/// Best-effort throughout: no active budgeted goal, an unparseable caller key,
/// or any store read failure lets the delegation proceed unaccounted rather
/// than blocking real work.
pub async fn check_and_enroll_delegation(
    session_store: &Arc<dyn SessionStore>,
    caller_session: &str,
    child_key: &SessionKey,
    refuse_when_over: bool,
) -> DelegationBudget {
    let Some(goal_store) = crate::goal::global() else {
        return DelegationBudget::Proceed;
    };
    let Ok(Some(goal)) = goal_store.get(caller_session) else {
        return DelegationBudget::Proceed;
    };
    if !(goal.is_active() && goal.token_budget.is_some()) {
        return DelegationBudget::Proceed;
    }
    if refuse_when_over && goal.baseline_captured {
        if let Some(caller_key) = SessionKey::from_key_string(caller_session) {
            let tree_total = tree_tokens(session_store, &goal, &caller_key).await;
            if tree_total.is_some_and(|t| goal.over_budget(t)) {
                return DelegationBudget::Refused(budget_exhausted_reason(&goal));
            }
        }
    }
    // Enroll under the gateway key the child accrues tokens under. First-writer-
    // wins in the store, so a retried task keeps its original join baseline.
    let member_baseline = session_store
        .get_total_tokens(child_key)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    if let Err(e) = goal_store.register_budget_member(
        caller_session,
        &child_key.to_key_string(),
        member_baseline,
        now_ms(),
    ) {
        warn!(error = %e, caller = %caller_session, child = %child_key.to_key_string(),
            "tree budget: delegation member enrollment failed (delegation proceeds unaccounted)");
    }
    DelegationBudget::Proceed
}

/// The model-facing refusal reason when a goal's shared tree budget is spent.
fn budget_exhausted_reason(goal: &Goal) -> String {
    format!(
        "Delegation refused: the standing goal's shared token budget ({} tokens across this \
         session and {} delegated session(s)) is exhausted. Wrap up with the results you have, \
         or raise the budget via goal(action='update', token_budget=…).",
        goal.token_budget.unwrap_or(0),
        goal.budget_members.len(),
    )
}

/// The refusal half of [`check_and_enroll_delegation`], for callers that must
/// decide to refuse BEFORE they have a child session key to enroll (e.g.
/// `team_delegate`, which only mints the child key after it has created the
/// task row). Returns a model-facing reason when the delegating session's
/// active goal has a captured baseline and the shared tree budget is already
/// spent; `None` otherwise (no goal / no budget / within budget). Best-effort:
/// any store read failure reads as "within budget" (proceed).
pub async fn tree_budget_refusal(
    session_store: &Arc<dyn SessionStore>,
    caller_session: &str,
) -> Option<String> {
    let goal_store = crate::goal::global()?;
    let goal = goal_store.get(caller_session).ok().flatten()?;
    if !(goal.is_active() && goal.token_budget.is_some() && goal.baseline_captured) {
        return None;
    }
    let caller_key = SessionKey::from_key_string(caller_session)?;
    let tree_total = tree_tokens(session_store, &goal, &caller_key).await;
    tree_total
        .is_some_and(|t| goal.over_budget(t))
        .then(|| budget_exhausted_reason(&goal))
}

/// Fold the current turn's session key into a coordination task's metadata as
/// the `origin_session` anchor (see [`ORIGIN_SESSION_METADATA_KEY`]). A no-op
/// when there is no turn context (task created outside a tool call) or when the
/// metadata is not a JSON object — the autonomous dispatcher then simply runs
/// the child unaccounted (fail-open), exactly as before this anchor existed.
#[must_use]
pub fn with_origin_session(mut metadata: serde_json::Value) -> serde_json::Value {
    if let Some(session) = crate::tools::turn_context::current_session_key() {
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert(
                ORIGIN_SESSION_METADATA_KEY.to_string(),
                serde_json::Value::String(session),
            );
        }
    }
    metadata
}

/// Read the `origin_session` anchor back from a task's metadata, if present.
#[must_use]
pub fn origin_session_from_metadata(metadata: &serde_json::Value) -> Option<String> {
    metadata
        .get(ORIGIN_SESSION_METADATA_KEY)?
        .as_str()
        .map(str::to_string)
}

/// The goal's tree token total in `over_budget`'s coordinate system: the own
/// session's cumulative total PLUS every member's delta since enrollment
/// (`Goal::tokens_used` later subtracts `tokens_at_start`, yielding
/// own-delta + member-deltas). `None` = the own total is unavailable —
/// budget unenforced this round, same contract as the plain read it
/// replaces. Unreadable members are skipped with a warn (under-counting is
/// the fail-open direction: pursuit continues, never falsely blocked).
pub async fn tree_tokens(
    store: &Arc<dyn SessionStore>,
    goal: &Goal,
    own_key: &SessionKey,
) -> Option<u64> {
    let own = match store.get_total_tokens(own_key).await {
        Ok(total) => total?,
        Err(e) => {
            warn!(error = %e, session = %own_key.to_key_string(),
                "goal tree budget: own session token read failed; budget unenforced this round");
            return None;
        }
    };
    // INVARIANT (single accounting rail): the tree total is own row + each
    // ENROLLED member's disjoint gateway row, counted exactly once. In-process
    // subagents bill a separate child-harness counter (harness/agent.rs) that
    // touches no gateway session row and is never enrolled — so their spend is
    // absent here BY DESIGN. Do not add a second accumulation of the same
    // tokens (e.g. rolling a child's total into the owner row, or enrolling one
    // run under two keys): that is the double-count this rail is built to avoid.
    // Covered by session_manager::tests::goal_tree_budget_sums_own_plus_member_deltas_only.
    let mut total = own;
    for member in &goal.budget_members {
        let Some(key) = SessionKey::from_key_string(&member.session_id) else {
            warn!(member = %member.session_id,
                "goal tree budget: unparseable member session key; skipped");
            continue;
        };
        match store.get_total_tokens(&key).await {
            Ok(Some(member_total)) => {
                total = total.saturating_add(member_total.saturating_sub(member.tokens_at_join));
            }
            Ok(None) => {} // no counted turns yet — zero delta.
            Err(e) => {
                warn!(error = %e, member = %member.session_id,
                    "goal tree budget: member token read failed; its spend uncounted this round");
            }
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_session_anchor_roundtrips_through_metadata() {
        let mut obj = serde_json::Map::new();
        obj.insert(
            ORIGIN_SESSION_METADATA_KEY.to_string(),
            serde_json::Value::String("agent:main:main".to_string()),
        );
        let meta = serde_json::Value::Object(obj);
        assert_eq!(
            origin_session_from_metadata(&meta).as_deref(),
            Some("agent:main:main")
        );
    }

    #[test]
    fn origin_session_absent_or_non_object_reads_none() {
        assert!(origin_session_from_metadata(&serde_json::json!({})).is_none());
        assert!(origin_session_from_metadata(&serde_json::json!({"other": "x"})).is_none());
        // A non-object metadata value carries no anchor.
        assert!(origin_session_from_metadata(&serde_json::Value::Null).is_none());
        assert!(origin_session_from_metadata(&serde_json::json!("scalar")).is_none());
    }

    #[test]
    fn with_origin_session_is_a_noop_without_a_turn_context() {
        // A bare unit test has no ambient turn context, so the anchor is not
        // stamped — the autonomous dispatcher then runs the child unaccounted
        // (fail-open), which is the intended degraded behavior.
        let stamped = with_origin_session(serde_json::json!({"managed": true}));
        assert!(origin_session_from_metadata(&stamped).is_none());
        // Existing keys are preserved untouched.
        assert_eq!(stamped.get("managed"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn with_origin_session_leaves_non_object_metadata_untouched() {
        let v = with_origin_session(serde_json::Value::Null);
        assert!(v.is_null());
    }
}
