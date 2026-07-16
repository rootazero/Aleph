//! Tree token accounting for a budgeted goal (tree budget v1 — codex
//! `RolloutBudget` mapped onto A3's single persistent source).
//!
//! codex shares one in-memory `Arc<RolloutBudget>` across an agent tree;
//! Aleph deliberately has no per-tree control handle — the durable source of
//! token truth is `SessionStore::get_total_tokens`, so the tree total is
//! *derived*: the goal session's own live total plus each enrolled
//! delegation member's spend since it joined (`tokens_at_join` baseline,
//! stamped by `session_send` at enrollment). The sum feeds the EXISTING
//! `Goal::over_budget` channel unchanged, so enforcement stays at the
//! run-granular continuation hook and the delegation seam — never mid-turn
//! (a mid-sampling hard abort would need the think loop, R10).
//!
//! Consumers: `goal_continuation::live_tokens` (budget enforcement per
//! claim) and `session_send` (refuse NEW delegations once the shared budget
//! is spent, F9-style compact refusal).

use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::goal::Goal;
use crate::sync_primitives::Arc;
use tracing::warn;

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
