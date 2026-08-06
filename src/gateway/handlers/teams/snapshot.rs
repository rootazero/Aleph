//! Snapshot lifecycle + per-team usage handlers.

use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::agents::swarm::tasks::CoordTaskStore;
use crate::resilience::{AgentUsageTotal, StateDatabase};
use crate::sync_primitives::Arc;
use crate::teams::snapshots::{capture_snapshot, restore_snapshot, SqliteSnapshotStore};
use crate::teams::TeamStore;

use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::teams::visibility::{gate_team, snapshot_team_visible};
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};

/// The response an unreachable snapshot produces — missing, orphaned, or owned
/// by another user. One constructor so the three stay indistinguishable.
fn snapshot_not_found(id: Option<serde_json::Value>, snapshot_id: &str) -> JsonRpcResponse {
    JsonRpcResponse::error(
        id,
        RESOURCE_NOT_FOUND,
        format!("snapshot '{snapshot_id}' not found"),
    )
}

#[derive(Debug, Deserialize)]
pub struct SnapshotCreateParams {
    pub team_id: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SnapshotListParams {
    #[serde(default)]
    pub team_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SnapshotIdParams {
    pub snapshot_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SnapshotRestoreParams {
    pub snapshot_id: String,
    /// `false` (default) → dry-run, returns the diff without mutation.
    /// `true` → applies the snapshot, including dependency-edge restoration.
    #[serde(default)]
    pub apply: bool,
}

/// teams.snapshot.create — capture a new snapshot of `team_id`'s current state.
pub async fn handle_snapshot_create(
    request: JsonRpcRequest,
    team_store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.create request");
    let params: SnapshotCreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_team(request.id.clone(), &team_store, &params.team_id).await {
        return resp;
    }
    let tag = params.tag.unwrap_or_default();
    let note = params.note.unwrap_or_default();
    match capture_snapshot(
        snapshot_store.as_ref(),
        team_store.as_ref(),
        coord_store.as_ref(),
        &params.team_id,
        &tag,
        &note,
    )
    .await
    {
        Ok(out) => JsonRpcResponse::success(request.id, json!(out)),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot create failed: {e}"),
        ),
    }
}

/// teams.snapshot.list — newest-first metadata listing, optionally filtered.
pub async fn handle_snapshot_list(
    request: JsonRpcRequest,
    team_store: Arc<dyn TeamStore>,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.list request");
    // List is the only snapshot RPC that allows missing params (no filter).
    let params: SnapshotListParams = match request.params.as_ref() {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("invalid params: {e}"),
                )
            }
        },
        None => SnapshotListParams::default(),
    };
    // An explicit `team_id` is gated up front; the unfiltered form (the Panel's
    // "all snapshots" view) is filtered per row below. Doing only the former
    // would leave the no-argument call listing every user's snapshots — the
    // list-shaped hole P1's `sessions.list` filter exists to prevent.
    if let Some(ref team_id) = params.team_id {
        if let Err(resp) = gate_team(request.id.clone(), &team_store, team_id).await {
            return resp;
        }
    }
    match snapshot_store.list(params.team_id.as_deref()).await {
        Ok(metas) => {
            let mut visible = Vec::with_capacity(metas.len());
            for meta in metas {
                if snapshot_team_visible(&team_store, &meta.team_id).await {
                    visible.push(meta);
                }
            }
            JsonRpcResponse::success(request.id, json!({ "snapshots": visible }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot list failed: {e}"),
        ),
    }
}

/// teams.snapshot.get — load the full payload by id.
pub async fn handle_snapshot_get(
    request: JsonRpcRequest,
    team_store: Arc<dyn TeamStore>,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.get request");
    let params: SnapshotIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match snapshot_store.get(&params.snapshot_id).await {
        Ok(Some((meta, payload))) => {
            // The payload embeds the full team record, members and tasks, so
            // the ownership check has to happen before it is serialized — this
            // is the snapshot-shaped twin of P1 Task 6's checkpoint-branch read
            // compromise.
            if !snapshot_team_visible(&team_store, &meta.team_id).await {
                return snapshot_not_found(request.id, &params.snapshot_id);
            }
            JsonRpcResponse::success(request.id, json!({ "meta": meta, "payload": payload }))
        }
        Ok(None) => snapshot_not_found(request.id, &params.snapshot_id),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot get failed: {e}"),
        ),
    }
}

/// teams.snapshot.restore — apply or dry-run; returns the diff either way.
pub async fn handle_snapshot_restore(
    request: JsonRpcRequest,
    team_store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.restore request");
    let params: SnapshotRestoreParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    // Restore writes the snapshot's team + tasks back into the live stores, so
    // it is gated on the SNAPSHOT's team, not on any caller-supplied team id.
    match snapshot_store.get(&params.snapshot_id).await {
        Ok(Some((meta, _))) => {
            if !snapshot_team_visible(&team_store, &meta.team_id).await {
                return snapshot_not_found(request.id, &params.snapshot_id);
            }
        }
        Ok(None) => return snapshot_not_found(request.id, &params.snapshot_id),
        Err(e) => {
            tracing::warn!(
                snapshot_id = %params.snapshot_id,
                error = %e,
                "teams.snapshot.restore: ownership gate failed closed"
            );
            return snapshot_not_found(request.id, &params.snapshot_id);
        }
    }
    match restore_snapshot(
        snapshot_store.as_ref(),
        team_store.as_ref(),
        coord_store.as_ref(),
        &params.snapshot_id,
        !params.apply,
    )
    .await
    {
        Ok(diff) => JsonRpcResponse::success(request.id, json!(diff)),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot restore failed: {e}"),
        ),
    }
}

// =============================================================================
// teams.usage — per-team provider token aggregation (F5)
// =============================================================================

/// Params for `teams.usage`. `since` / `until` are optional epoch-second
/// bounds. They default to "all time" when absent.
#[derive(Debug, Deserialize)]
pub struct UsageParams {
    pub team_id: String,
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub until: Option<i64>,
}

/// teams.usage — returns provider-token usage aggregated across all members
/// of `team_id`, plus a per-agent breakdown. Pure read-side: derived from
/// `task_traces` `ProviderUsage` events that are already persisted by the
/// gateway execution engine.
///
/// Cost is intentionally not computed here — tokens are factual, cost is
/// rate-card policy that lives in the provider config / UI layer.
pub async fn handle_usage(
    request: JsonRpcRequest,
    team_store: Arc<dyn TeamStore>,
    state_db: Arc<StateDatabase>,
) -> JsonRpcResponse {
    debug!("Handling teams.usage request");
    let params: UsageParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Step 1 — resolve team members. Both "team not found" and "team exists
    // but is empty" are distinguishable here so the response can disambiguate;
    // "team exists but is someone else's" collapses into the first, which is
    // the point of routing the existence check through the gate.
    if let Err(resp) = gate_team(request.id.clone(), &team_store, &params.team_id).await {
        return resp;
    }

    let members = match team_store.get_members(&params.team_id).await {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to load members for '{}': {}", params.team_id, e),
            );
        }
    };
    let agent_ids: Vec<String> = members.iter().map(|m| m.agent_id.clone()).collect();

    // Step 2 — aggregate usage across the team's members.
    let per_agent: Vec<AgentUsageTotal> = match state_db
        .aggregate_usage_by_agents(&agent_ids, params.since, params.until)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Usage aggregation failed for '{}': {}", params.team_id, e),
            );
        }
    };

    // Step 3 — fold into team totals. Saturating sums match the harness
    // `TokenBreakdown::total()` semantics.
    let mut total_calls: u64 = 0;
    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;
    let mut total_cache_read: u64 = 0;
    let mut total_cache_creation: u64 = 0;
    let mut total_reasoning: u64 = 0;
    for row in &per_agent {
        total_calls = total_calls.saturating_add(row.call_count);
        total_input = total_input.saturating_add(row.input_tokens);
        total_output = total_output.saturating_add(row.output_tokens);
        total_cache_read = total_cache_read.saturating_add(row.cache_read_tokens);
        total_cache_creation = total_cache_creation.saturating_add(row.cache_creation_tokens);
        total_reasoning = total_reasoning.saturating_add(row.reasoning_tokens);
    }
    // Reuse the per-row method so the rollup denominator matches what each
    // AgentUsageTotal would report on its own.
    let cache_hit_ratio = AgentUsageTotal {
        agent_id: String::new(),
        call_count: total_calls,
        input_tokens: total_input,
        output_tokens: total_output,
        cache_read_tokens: total_cache_read,
        cache_creation_tokens: total_cache_creation,
        reasoning_tokens: total_reasoning,
    }
    .cache_hit_ratio();

    // Step 4 — MoA advisor spend (round-2 B6). Advisors are metered under
    // synthetic `moa:<idx>:<provider>:<model>` agent ids that never appear in
    // a team's member list, so `aggregate_usage_by_agents` above can't see
    // them. Roll them into one dedicated bucket instead of leaving advisor
    // spend invisible. Errors are treated the same as "no advisor usage" —
    // this is a supplementary field, not load-bearing for the response.
    let moa_advisors = state_db
        .aggregate_moa_advisor_usage(params.since, params.until)
        .await
        .ok()
        .flatten();

    JsonRpcResponse::success(
        request.id,
        json!({
            "team_id": params.team_id,
            "since": params.since,
            "until": params.until,
            "member_count": agent_ids.len(),
            "total": {
                "call_count": total_calls,
                "input_tokens": total_input,
                "output_tokens": total_output,
                "cache_read_tokens": total_cache_read,
                "cache_creation_tokens": total_cache_creation,
                "reasoning_tokens": total_reasoning,
                "cache_hit_ratio": cache_hit_ratio,
            },
            "per_agent": per_agent,
            "moa_advisors": moa_advisors,
        }),
    )
}

/// teams.snapshot.delete — idempotent removal. Returns `existed: bool`.
pub async fn handle_snapshot_delete(
    request: JsonRpcRequest,
    team_store: Arc<dyn TeamStore>,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.delete request");
    let params: SnapshotIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    // Idempotent by contract (`existed: false` for an unknown id), so a
    // foreign snapshot must produce that same shape rather than an error —
    // otherwise delete becomes the existence oracle the reads are denied.
    match snapshot_store.get(&params.snapshot_id).await {
        Ok(Some((meta, _))) if snapshot_team_visible(&team_store, &meta.team_id).await => {}
        Ok(_) => {
            return JsonRpcResponse::success(
                request.id,
                json!({ "snapshot_id": params.snapshot_id, "existed": false }),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("snapshot delete failed: {e}"),
            )
        }
    }
    match snapshot_store.delete(&params.snapshot_id).await {
        Ok(existed) => JsonRpcResponse::success(
            request.id,
            json!({ "snapshot_id": params.snapshot_id, "existed": existed }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot delete failed: {e}"),
        ),
    }
}
