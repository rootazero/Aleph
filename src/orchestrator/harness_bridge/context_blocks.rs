//! Free functions that compute per-turn runtime/context prompt fragments
//! (execution plan, standing goal, tool health) for the context layers.

use crate::sync_primitives::Arc;

/// Look up the session's active scratchpad execution list and render a
/// compact, judgment-free progress snapshot for `ExecutionPlanLayer` to
/// inject into the per-turn system prompt. Returns `None` (→ the layer
/// emits nothing) when the session has no bound scratchpad, the file is
/// missing/unreadable, or every plan item is already done — the same
/// `has_pending_work()` gate the stop-verifier uses, so an empty or
/// finished plan never adds noise.
///
/// Free async function so it can be unit-tested without a full
/// `AgentHarnessRunner`, mirroring `compute_runtime_state_blocks`.
/// Fail-soft on any I/O error: a transient scratchpad read must never
/// wedge prompt assembly.
pub async fn active_execution_plan(session_key: &str) -> Option<String> {
    let project_id = crate::builtin_tools::scratchpad_registry::active(session_key)?;
    let manager = crate::memory::scratchpad::ScratchpadManager::new(&project_id, "harness");
    if !manager.exists() {
        return None;
    }
    let snapshot = manager.snapshot().await.ok()?;
    snapshot
        .has_pending_work()
        .then(|| snapshot.render_progress())
}

/// Fetch the session's active standing goal as a compact, judgment-free
/// summary for `StandingGoalLayer`. Returns `None` (→ layer emits nothing)
/// when the goal subsystem is uninitialized, the session has no goal, or the
/// goal is not `Active`. Fail-soft on store error. Mirrors `active_execution_plan`.
pub async fn active_standing_goal(session_key: &str) -> Option<String> {
    let store = crate::goal::global()?;
    let goal = store.get(session_key).ok().flatten()?;
    if !goal.is_active() {
        return None;
    }
    // Stamp the wall-clock once so the rendered deadline matches the same
    // instant the autonomous loop's deadline check (`should_continue`) uses.
    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    Some(render_goal_summary(&goal, now_ms))
}

/// Fetch the session's welded Strategy for the prompt weld. Returns the
/// `Strategy` struct (the caller renders both the `<strategy>` body and the
/// guardrail echo from it). Resolution mirrors the StraTA composite key: try
/// `goal_key(session)` first (a `/goal` Strategy takes precedence), else
/// `loop_key(session)` (a `/loop` Strategy). Returns `None` (→ both Strategy
/// layers emit nothing) when the strategy subsystem is uninitialized or no
/// Strategy is stored for either key. Fail-soft on store error. Mirrors
/// `active_standing_goal`.
pub async fn active_strategy(session_key: &str) -> Option<crate::strategy::Strategy> {
    let store = crate::strategy::global()?;
    resolve_active_strategy(&store, session_key)
}

/// Resolve the welded Strategy for a session key against an explicit store
/// (sync; the global accessor lives in `active_strategy`, keeping this
/// unit-testable). Precedence: goal → loop → **team** → session. The team tier
/// fires only for a `team_chat` Task key — it recovers the (already-normalized)
/// team id and reads the leader-minted team-wide row, so every member welds the
/// same plan (strategy round 2). Above `session_key` so a member's own
/// `/goal`/`/loop` still wins; below `loop_key` for the same reason.
fn resolve_active_strategy(
    store: &crate::strategy::StrategyStore,
    session_key: &str,
) -> Option<crate::strategy::Strategy> {
    if let Some(s) = store.get(&crate::strategy::goal_key(session_key)).ok().flatten() {
        return Some(s);
    }
    if let Some(s) = store.get(&crate::strategy::loop_key(session_key)).ok().flatten() {
        return Some(s);
    }
    if let Some(crate::routing::session_key::SessionKey::Task { task_type, task_id, .. }) =
        crate::routing::session_key::SessionKey::parse(session_key)
    {
        if task_type == "team_chat" {
            if let Some(s) = store.get(&crate::strategy::team_key(&task_id)).ok().flatten() {
                return Some(s);
            }
        }
    }
    store
        .get(&crate::strategy::session_key(session_key))
        .ok()
        .flatten()
}

/// Format the active-goal summary line injected as `<standing_goal>`. Pure:
/// takes the goal plus the current wall-clock (Unix epoch ms) so it is
/// unit-testable without the process-global `GoalStore`. Surfaces the
/// objective plus every structural backstop the autonomous loop enforces —
/// token budget, wall-clock deadline, and iteration pace — so the model can
/// pace itself against each one (R9 — intelligence in the prompt). A goal with
/// no caps renders just `"{objective} (status=active)"`, byte-identical to the
/// pre-deadline output for the common case.
pub(crate) fn render_goal_summary(goal: &crate::goal::Goal, now_ms: u64) -> String {
    let budget = match goal.token_budget {
        Some(b) => format!(", budget={b}"),
        None => String::new(),
    };
    // The wall-clock deadline is a hard stop the loop enforces (it Blocks the
    // goal once exceeded), yet the model was never told it existed — surfacing
    // the remaining time lets it triage instead of being cut off mid-thought.
    let deadline = match goal.deadline_ms {
        Some(d) => format!(", {}", render_deadline(d, now_ms)),
        None => String::new(),
    };
    let pursuit = match goal.pursuit {
        crate::goal::PursuitMode::Active { max_iterations } => {
            format!(
                ", autonomous iteration {}/{}",
                goal.continuations_used, max_iterations
            )
        }
        crate::goal::PursuitMode::Passive => String::new(),
    };
    format!(
        "{} (status=active{budget}{deadline}{pursuit})",
        goal.objective
    )
}

/// Render a wall-clock deadline (absolute Unix epoch ms) as a compact
/// remaining-time phrase relative to `now_ms`. `now_ms == 0` (no clock
/// available, mirroring the loop's clock-less convention) degrades to a bare
/// "deadline set" rather than a misleading countdown; an already-passed
/// deadline reads "deadline passed" (the loop Blocks the goal on its next hook).
pub(crate) fn render_deadline(deadline_ms: u64, now_ms: u64) -> String {
    if now_ms == 0 {
        return "deadline set".to_string();
    }
    if now_ms >= deadline_ms {
        return "deadline passed".to_string();
    }
    let remaining_s = (deadline_ms - now_ms) / 1000;
    if remaining_s < 60 {
        format!("deadline in ~{remaining_s}s")
    } else if remaining_s < 3600 {
        format!("deadline in ~{}m", remaining_s / 60)
    } else {
        format!(
            "deadline in ~{}h{}m",
            remaining_s / 3600,
            (remaining_s % 3600) / 60
        )
    }
}

/// Snapshot the tool catalog's `ToolHealthCache` and convert every
/// currently-cached `Unhealthy` entry into a `RuntimeStateFragment` for
/// `ToolRuntimeStateLayer` to render. Returns `vec![]` when
/// `tool_catalog` is `None` (test / early-boot).
///
/// Free function so unit tests can exercise the conversion without
/// constructing a full `AgentHarnessRunner`.
#[must_use]
pub fn compute_runtime_state_blocks(
    tool_catalog: Option<&Arc<crate::tool_metadata::ToolCatalog>>,
) -> Vec<crate::tools::runtime_state::RuntimeStateFragment> {
    let Some(registry) = tool_catalog else {
        return Vec::new();
    };
    let snapshot = registry.health().snapshot();
    // Coalesce unhealthy tools by reason: a single downed dependency — an MCP
    // server exposing many tools, or the whole `browser_*` family when no
    // browser runtime exists — collapses to ONE hint instead of flooding the
    // prompt with a near-identical line per tool. Groups are keyed by the
    // reason's short label (server-id-qualified for MCP, capability-specific
    // for generation, so genuinely distinct dependencies stay separate) and
    // sorted for deterministic output. A single-tool group keeps its exact
    // name, so existing one-tool-per-reason behaviour is byte-identical.
    let mut by_reason: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for (name, reason) in snapshot.unhealthy_iter() {
        by_reason
            .entry(reason.short_label())
            .or_default()
            .push(name);
    }
    by_reason
        .into_iter()
        .map(|(reason, mut tools)| {
            tools.sort_unstable();
            let label = match tools.as_slice() {
                [] => String::new(),
                [single] => (*single).to_string(),
                many => format!("{} (+{} more)", many[0], many.len() - 1),
            };
            crate::tools::runtime_state::RuntimeStateFragment::unavailable(label, reason)
        })
        .collect()
}

#[cfg(test)]
mod active_strategy_tests {
    use super::*;

    #[tokio::test]
    async fn returns_none_when_store_uninitialized() {
        let out = active_strategy("session-with-no-store").await;
        assert!(out.is_none());
    }

    fn mk_strategy(objective: &str) -> crate::strategy::Strategy {
        crate::strategy::Strategy {
            objective: objective.into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["g".into()],
            success_criteria: "s".into(),
            goal_id: None,
        }
    }

    #[test]
    fn resolve_active_strategy_team_tier_and_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::strategy::StrategyStore::open(&dir.path().join("s.db")).unwrap();
        // A team-chat member session key: agent:alice:team_chat:squad
        let sk = crate::routing::session_key::SessionKey::task("alice", "team_chat", "squad")
            .to_key_string();

        // team tier resolves the team-wide row
        store.put(&crate::strategy::team_key("squad"), &mk_strategy("team-obj")).unwrap();
        assert_eq!(
            resolve_active_strategy(&store, &sk).map(|s| s.objective),
            Some("team-obj".to_string())
        );

        // a member's own /goal still wins over the team frame
        store.put(&crate::strategy::goal_key(&sk), &mk_strategy("goal-obj")).unwrap();
        assert_eq!(
            resolve_active_strategy(&store, &sk).map(|s| s.objective),
            Some("goal-obj".to_string())
        );

        // a non-team session never hits the team tier
        assert!(resolve_active_strategy(&store, "agent:bob:main").is_none());
    }
}
