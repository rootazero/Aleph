//! Team CRUD + membership handlers (list / get / disband / delete / create / rename / agents.teams).

use serde::Deserialize;
use serde_json::json;
use tracing::{debug, warn};

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStore};
use crate::sync_primitives::Arc;
use crate::teams::{NewTeam, NewTeamMember, TeamStore};

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::{ChangeKind, GatewayEventFrame};
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};

/// Emit a `team.changed` frame so every subscribed surface re-fetches: the
/// group-chat sidebar (`agents.teams`) and the teams tab (`teams.list`) both
/// listen on this topic. Without it the two views drift apart until a manual
/// refresh. Best-effort — a serialization failure must not fail the RPC.
fn notify_team_changed(event_bus: &Arc<GatewayEventBus>, team_id: &str, change: ChangeKind) {
    let _ = event_bus.publish_frame(&GatewayEventFrame::TeamChanged {
        team_id: team_id.to_string(),
        change,
    });
}

#[derive(Debug, Deserialize)]
pub struct TeamIdParams {
    pub team_id: String,
}

#[derive(Debug, Deserialize)]
pub struct AgentIdParams {
    pub agent_id: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// Handle teams.list — list all teams as lightweight summaries
pub async fn handle_list(request: JsonRpcRequest, store: Arc<dyn TeamStore>) -> JsonRpcResponse {
    debug!("Handling teams.list request");

    match store.list_teams().await {
        Ok(teams) => JsonRpcResponse::success(request.id, json!({ "teams": teams })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list teams: {e}"),
        ),
    }
}

/// Handle teams.get — get full team detail: team record, members, and tasks
pub async fn handle_get(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.get request");

    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let team = match store.get_team(&params.team_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Team '{}' not found", params.team_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get team '{}': {}", params.team_id, e),
            )
        }
    };

    let members = match store.get_members(&params.team_id).await {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get members for team '{}': {}", params.team_id, e),
            )
        }
    };

    let tasks = match coord_store
        .list_tasks(CoordTaskFilter {
            team_id: Some(params.team_id.clone()),
            ..Default::default()
        })
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get tasks for team '{}': {}", params.team_id, e),
            )
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({ "team": team, "members": members, "tasks": tasks }),
    )
}

/// Handle teams.disband — mark a team as disbanded
pub async fn handle_disband(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling teams.disband request");

    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.disband_team(&params.team_id).await {
        Ok(()) => {
            // Victory-claim trigger, identical to the `team_disband` tool arm.
            // A disband is once-only: whichever face performs it is the ONLY
            // chance the governance graph gets to review the claim, so a face
            // that skips this makes `loop_graph(action='pair', to_id='team:…')`
            // silently untrue for every Panel-driven disband.
            let _ = crate::loop_graph::service::notify_team_settled(&params.team_id).await;
            notify_team_changed(&event_bus, &params.team_id, ChangeKind::Updated);
            JsonRpcResponse::success(request.id, json!({ "success": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to disband team '{}': {}", params.team_id, e),
        ),
    }
}

/// Handle teams.delete — permanently delete a disbanded team with cascade cleanup.
///
/// Deletes the team row first (authoritative gate; disbanded check lives inside
/// `delete_team`). On success, best-effort orphan cleanup runs across all five
/// subordinate stores; individual failures emit a `warn!` but do not fail the
/// overall response. Use `handle_delete_basic` as a fallback when the subordinate
/// stores are not configured.
pub async fn handle_delete(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    msg_store: Arc<dyn crate::teams::messages::MessageStore>,
    event_store: Arc<dyn crate::teams::events::EventLogStore>,
    artifact_store: Arc<dyn crate::teams::artifacts::ArtifactStore>,
    snapshot_store: Arc<crate::teams::SqliteSnapshotStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling teams.delete request (cascade)");
    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let team_id = params.team_id;

    // 1) Authoritative gate + remove team row (disbanded check inside delete_team).
    //    Fail immediately on error — do not cascade to subordinate stores.
    if let Err(e) = store.delete_team(&team_id).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete team '{team_id}': {e}"),
        );
    }

    // 2) Best-effort cleanup of orphaned data in subordinate stores.
    //    Each failure is logged with warn! but does not affect the success response.
    let task_ids: Vec<String> = match coord_store
        .list_tasks(CoordTaskFilter {
            team_id: Some(team_id.clone()),
            ..Default::default()
        })
        .await
    {
        Ok(tasks) => tasks.into_iter().map(|t| t.id).collect(),
        Err(e) => {
            warn!("teams.delete: list tasks for artifact cleanup failed: {e}");
            Vec::new()
        }
    };
    if let Err(e) = artifact_store.delete_artifacts_for_tasks(&task_ids).await {
        warn!("teams.delete: artifact cleanup failed for {team_id}: {e}");
    }
    if let Err(e) = coord_store.delete_team_tasks(&team_id).await {
        warn!("teams.delete: task cleanup failed for {team_id}: {e}");
    }
    if let Err(e) = snapshot_store.delete_team_snapshots(&team_id).await {
        warn!("teams.delete: snapshot cleanup failed for {team_id}: {e}");
    }
    if let Err(e) = msg_store.delete_team_messages(&team_id).await {
        warn!("teams.delete: message cleanup failed for {team_id}: {e}");
    }
    if let Err(e) = event_store.delete_team_events(&team_id).await {
        warn!("teams.delete: event cleanup failed for {team_id}: {e}");
    }

    notify_team_changed(&event_bus, &team_id, ChangeKind::Deleted);
    JsonRpcResponse::success(request.id, json!({ "success": true }))
}

/// Fallback used when subordinate stores are not all configured: removes the
/// team row only (legacy behavior). Cascade cleanup is skipped.
pub async fn handle_delete_basic(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match store.delete_team(&params.team_id).await {
        Ok(()) => {
            notify_team_changed(&event_bus, &params.team_id, ChangeKind::Deleted);
            JsonRpcResponse::success(request.id, json!({ "success": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete team '{}': {}", params.team_id, e),
        ),
    }
}

// =============================================================================
// teams.create — create a persistent team with explicit leader + members
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateMemberSpec {
    pub agent_id: String,
    #[serde(default = "default_member_role")]
    pub role: String,
}

fn default_member_role() -> String {
    "member".to_string()
}

#[derive(Debug, Deserialize)]
pub struct CreateTeamParams {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub leader_id: String,
    #[serde(default)]
    pub members: Vec<CreateMemberSpec>,
    /// When true, the team was created with a blank name from the Panel; the
    /// first `teams.chat.send` will replace the provisional name with an
    /// LLM-generated topic. Defaults false (explicit names are respected).
    #[serde(default)]
    pub auto_name: bool,
}

/// teams.create — create a persistent team with explicit leader_id + members.
///
/// Thin I/O: only wraps `TeamStore::create_team` + `add_member`, no orchestration
/// or business logic (R4/R10). Leader is auto-enrolled with role="leader";
/// members duplicating the leader are skipped.
pub async fn handle_create(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling teams.create request");

    let params: CreateTeamParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if params.name.trim().is_empty() || params.leader_id.trim().is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "name and leader_id are required".to_string(),
        );
    }

    // Duplicate-name check intentionally omitted at this thin I/O layer; see
    // TeamCreateTool for LLM-facing dup-name validation.
    let team = match store
        .create_team(NewTeam {
            name: params.name.clone(),
            description: params.description.clone(),
            leader_id: params.leader_id.clone(),
        })
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to create team: {e}"),
            )
        }
    };

    // Auto-enroll the leader with role="leader" (mirrors team_create tool semantics).
    if let Err(e) = store
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: params.leader_id.clone(),
            role: "leader".to_string(),
            ..Default::default()
        })
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to enroll leader: {e}"),
        );
    }

    for spec in params.members {
        if spec.agent_id == params.leader_id {
            continue; // leader already enrolled
        }
        if let Err(e) = store
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: spec.agent_id.clone(),
                role: spec.role.clone(),
                ..Default::default()
            })
            .await
        {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to enroll member '{}': {e}", spec.agent_id),
            );
        }
    }

    // Blank-name teams from the Panel carry the auto-name flag so the first
    // message can replace the provisional name with an LLM topic.
    if params.auto_name {
        if let Err(e) = store.set_name_auto(&team.id, true).await {
            tracing::warn!(team_id = %team.id, error = %e, "failed to set name_auto flag");
        }
    }

    notify_team_changed(&event_bus, &team.id, ChangeKind::Created);
    JsonRpcResponse::success(
        request.id,
        json!({ "team_id": team.id, "name": team.name, "leader_id": team.leader_id }),
    )
}

// =============================================================================
// teams.rename — rename a team
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct RenameTeamParams {
    pub team_id: String,
    pub name: String,
}

/// teams.rename — rename a team. Thin I/O: validates non-blank name, delegates
/// to `TeamStore::rename_team`. Used by the Panel sidebar inline-edit.
pub async fn handle_rename(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling teams.rename request");
    let params: RenameTeamParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = params.name.trim();
    if params.team_id.trim().is_empty() || name.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "team_id and a non-blank name are required".to_string(),
        );
    }
    match store.rename_team(&params.team_id, name).await {
        Ok(()) => {
            notify_team_changed(&event_bus, &params.team_id, ChangeKind::Updated);
            JsonRpcResponse::success(request.id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::error(request.id, RESOURCE_NOT_FOUND, format!("{e}")),
    }
}

/// Handle agents.teams — list all teams an agent belongs to (as leader or member).
///
/// When `agent_manager` and `msg_store` are supplied, each team summary is enriched
/// with `members_preview` (up to 4 members with name + emoji) and `last_message`
/// (most recent transcript content, truncated to 60 chars). Both are best-effort:
/// per-team failures degrade to empty/null rather than failing the whole list.
pub async fn handle_agent_teams(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    agent_manager: Option<Arc<crate::config::agent_manager::AgentManager>>,
    msg_store: Option<Arc<dyn crate::teams::messages::MessageStore>>,
) -> JsonRpcResponse {
    debug!("Handling agents.teams request");

    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let summaries = match store.get_agent_teams(&params.agent_id).await {
        Ok(s) => s,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get teams for agent '{}': {}", params.agent_id, e),
            )
        }
    };

    // Fast path: no enrichment stores available — return raw summaries as before.
    if agent_manager.is_none() && msg_store.is_none() {
        return JsonRpcResponse::success(request.id, json!({ "teams": summaries }));
    }

    let mgr = agent_manager.as_deref();

    let mut enriched: Vec<serde_json::Value> = Vec::with_capacity(summaries.len());
    for summary in summaries {
        let team_id = &summary.id;

        // members_preview: up to 4 members with name + emoji (best-effort).
        let members_preview: Vec<serde_json::Value> = match store.get_members(team_id).await {
            Ok(members) => members
                .into_iter()
                .take(4)
                .map(|mem| {
                    let def = mgr.and_then(|m| m.get(&mem.agent_id).ok());
                    let name = def
                        .as_ref()
                        .and_then(|d| d.name.clone())
                        .unwrap_or_else(|| mem.agent_id.clone());
                    let emoji = def
                        .as_ref()
                        .and_then(|d| d.identity.as_ref().and_then(|i| i.emoji.clone()));
                    json!({ "id": mem.agent_id, "name": name, "emoji": emoji })
                })
                .collect(),
            Err(_) => vec![],
        };

        // last_message + last_message_at: most recent transcript entry (content
        // truncated to 60 chars) and its Unix-epoch-seconds timestamp. The panel
        // sorts group chats newest-first on last_message_at (falling back to
        // created_at). Both best-effort: null on per-team failure.
        // list_team_messages returns oldest-first; .pop() gives the newest.
        let (last_message, last_message_at): (Option<String>, Option<i64>) =
            match msg_store.as_deref() {
                Some(ms) => ms
                    .list_team_messages(team_id, 100)
                    .await
                    .ok()
                    .and_then(|mut v| v.pop())
                    .map(|m| {
                        (
                            Some(m.content.chars().take(60).collect::<String>()),
                            Some(m.created_at.timestamp()),
                        )
                    })
                    .unwrap_or((None, None)),
                None => (None, None),
            };

        let mut obj = serde_json::to_value(&summary).unwrap_or_else(|_| json!({}));
        if let Some(map) = obj.as_object_mut() {
            map.insert("members_preview".to_string(), json!(members_preview));
            map.insert("last_message".to_string(), json!(last_message));
            map.insert("last_message_at".to_string(), json!(last_message_at));
        }
        enriched.push(obj);
    }

    JsonRpcResponse::success(request.id, json!({ "teams": enriched }))
}
