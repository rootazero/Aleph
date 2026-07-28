//! Workflow Canvas (DAG <-> Obsidian JSON Canvas) + teams.chat.send handlers.

use serde::Deserialize;
use serde_json::json;
use tracing::{debug, warn};

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStore};
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;

use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};

// =============================================================================
// Workflow Canvas — DAG ↔ Obsidian JSON Canvas bridge
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ExportCanvasParams {
    pub team_id: String,
    /// Optional status filter — same vocabulary as `teams.list_tasks`.
    /// When unset, every coord-task in the team is rendered.
    #[serde(default)]
    pub status: Option<String>,
    /// Optional owner filter (`agent_id`).
    #[serde(default)]
    pub owner: Option<String>,
}

/// Handle `teams.workflow.export_canvas` — render the team's `CoordTask` DAG as
/// an Obsidian JSON Canvas 1.0 document. The panel's canvas engine accepts
/// the same wire shape, so this is what powers the "Workflow" tab.
pub async fn handle_workflow_export_canvas(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.workflow.export_canvas request");

    let params: ExportCanvasParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    use crate::agents::swarm::tasks::CoordTaskStatus;
    // Single-source filter vocabulary (all 10 variants); unknown strings are
    // an explicit error, not a silently unfiltered board — mirrors
    // `handle_list_tasks` and the `team_workflow_canvas` tool.
    let status_filter = match params.status.as_deref() {
        None => None,
        Some(s) => match CoordTaskStatus::from_filter_str(s) {
            Some(status) => Some(status),
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!(
                        "Unknown status filter '{s}'. Expected one of: pending, blocked, \
                         in_progress, waiting_review, completed, failed, cancelled, skipped, \
                         paused, unsatisfiable."
                    ),
                );
            }
        },
    };
    let filter = CoordTaskFilter {
        team_id: Some(params.team_id.clone()),
        status: status_filter,
        owner: params.owner.clone(),
    };

    let tasks = match coord_store.list_tasks(filter).await {
        Ok(t) => t,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to list tasks for team '{}': {}", params.team_id, e),
            )
        }
    };

    let doc = crate::teams::workflow_canvas::tasks_to_canvas(&tasks);
    JsonRpcResponse::success(
        request.id,
        json!({
            "team_id": params.team_id,
            "canvas": doc,
            "node_count": doc.nodes.len(),
            "edge_count": doc.edges.len(),
        }),
    )
}

#[derive(Debug, Deserialize)]
pub struct ImportCanvasParams {
    pub team_id: String,
    /// Parsed JSON Canvas document. Any node kind other than `text` is
    /// silently skipped; edges connecting non-text nodes are also dropped.
    pub canvas: aleph_protocol::canvas_format::Document,
    /// When `true`, return the list of `NewCoordTask` that *would* be created
    /// without actually inserting them. Defaults to live import.
    #[serde(default)]
    pub dry_run: bool,
}

/// Handle `teams.workflow.import_canvas` — turn a JSON Canvas document into
/// `CoordTask` rows under `team_id`. The first non-blank line of each text
/// node becomes the task subject; the rest becomes the description. Edges
/// become `blocked_by` dependencies. Returns the created (or projected, in
/// dry-run) task list.
pub async fn handle_workflow_import_canvas(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.workflow.import_canvas request");

    let params: ImportCanvasParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let planned =
        crate::teams::workflow_canvas::canvas_to_new_tasks(&params.canvas, &params.team_id);

    if params.dry_run {
        // Surface the planned tasks without mutating state.
        return JsonRpcResponse::success(
            request.id,
            json!({
                "team_id": params.team_id,
                "planned": planned.len(),
                "tasks": planned
                    .into_iter()
                    .map(|t| json!({
                        "subject": t.subject,
                        "description": t.description,
                        "blocked_by": t.blocked_by,
                        "metadata": t.metadata,
                    }))
                    .collect::<Vec<_>>(),
            }),
        );
    }

    // Create tasks in two passes so dependency IDs can be rewritten from
    // canvas-node IDs to freshly-minted coord-task IDs.
    let mut node_to_task: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // First pass: stash each canvas node's original ID so we can map it after
    // create_task returns.
    let canvas_node_ids: Vec<String> = planned
        .iter()
        .map(|t| {
            crate::teams::workflow_canvas::extract_canvas_task_id(&t.metadata)
                .unwrap_or("")
                .to_string()
        })
        .collect();

    // Set of canvas-node IDs in this import batch — needed to distinguish
    // a "deferred" blocker (still in this batch, not yet created) from a
    // "passthrough" blocker (a real existing coord_task_id stitched in by
    // the caller).
    let batch_canvas_ids: std::collections::HashSet<String> = canvas_node_ids
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect();

    // Pair each task with its canvas node id so we keep the mapping after
    // the task value is moved into `create_task`.
    let mut work: Vec<(String, crate::agents::swarm::tasks::NewCoordTask)> =
        canvas_node_ids.into_iter().zip(planned).collect();

    let mut created: Vec<serde_json::Value> = Vec::with_capacity(work.len());
    // Topological insert with deferral: tasks whose blocked_by still points
    // at not-yet-created canvas nodes get held until a later pass. Cap at
    // `n + 1` passes to halt on cyclic input.
    let max_passes = work.len() + 1;
    let mut passes = 0;
    while !work.is_empty() && passes < max_passes {
        passes += 1;
        let mut remaining: Vec<(String, crate::agents::swarm::tasks::NewCoordTask)> = Vec::new();
        for (canvas_id, mut task) in work.into_iter() {
            let mut ready = true;
            let mut rewritten = Vec::with_capacity(task.blocked_by.len());
            for b in &task.blocked_by {
                if let Some(live) = node_to_task.get(b) {
                    // Already created in an earlier pass → rewrite to live ID.
                    rewritten.push(live.clone());
                } else if batch_canvas_ids.contains(b) {
                    // Still pending in this batch → defer.
                    ready = false;
                    break;
                } else {
                    // Not part of this batch → assume caller-supplied real
                    // coord_task_id (or harmless dangling reference).
                    rewritten.push(b.clone());
                }
            }
            if !ready {
                remaining.push((canvas_id, task));
                continue;
            }
            task.blocked_by = rewritten;
            let subject_for_err = task.subject.clone();
            match coord_store.create_task(task).await {
                Ok(t) => {
                    if !canvas_id.is_empty() {
                        node_to_task.insert(canvas_id, t.id.clone());
                    }
                    created.push(json!({
                        "id": t.id,
                        "subject": t.subject,
                    }));
                }
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to create task '{subject_for_err}': {e}"),
                    );
                }
            }
        }
        work = remaining;
    }
    // Re-bind for the post-loop emptiness check below.
    let planned: Vec<crate::agents::swarm::tasks::NewCoordTask> =
        work.into_iter().map(|(_, t)| t).collect();

    if !planned.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Canvas contains {} unresolvable dependency cycles or stranded edges",
                planned.len()
            ),
        );
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "team_id": params.team_id,
            "created": created.len(),
            "tasks": created,
        }),
    )
}

// =============================================================================
// teams.chat.send — spawn the team leader on a single harness run
//
// Registered in agent_init/mod.rs (needs GatewayContext + ExecutionAdapter),
// not in register_teams_handlers (which is store-only).
// =============================================================================

#[derive(Debug, serde::Deserialize)]
pub struct ChatSendParams {
    pub team_id: String,
    pub message: String,
}

/// teams.chat.send — equal-broadcast group chat (telegram-style multiagent).
/// Stores the user message into the shared transcript, then fans out to each
/// @mentioned agent (no @ → leader fallback) via the `GroupChatBroadcaster`.
/// Agents reply into the same transcript and may @ each other to continue,
/// bounded by chain_depth + fan-out width guards. R10: routing/fan-out is
/// deterministic scaffolding; all "what to say / whether to reply" reasoning
/// lives in each agent's own LLM loop.
pub async fn handle_chat_send(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    msg_store: Option<Arc<dyn crate::teams::messages::MessageStore>>,
    context: Arc<crate::gateway::context::GatewayContext>,
    topic_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    team_planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    coord_task_store: Option<Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>>,
    broadcast_cfg: crate::teams::broadcast::BroadcastConfig,
) -> JsonRpcResponse {
    let params: ChatSendParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    debug!(team_id = %params.team_id, "Handling teams.chat.send request");
    if params.team_id.trim().is_empty() || params.message.trim().is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "team_id and message are required".to_string(),
        );
    }

    // Team must exist (404 for an unknown id). We don't bind it here — the
    // broadcaster re-reads team + roster per dispatch round.
    match store.get_team(&params.team_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Team '{}' not found", params.team_id),
            )
        }
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    };

    // Group-chat broadcast needs the message store for the shared transcript.
    let Some(msg_store) = msg_store else {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "team message store unavailable; group chat cannot persist transcript".to_string(),
        );
    };

    // Persist the user message into the shared transcript (single source of
    // truth). Long TTL: the transcript is a durable record, not short-lived
    // inbox traffic (recipient-less messages would otherwise expire in 30 min).
    // A persistence failure must be visible — the fan-out below still runs,
    // but the triggering message would be a silent hole in the durable
    // transcript (P7).
    if let Err(e) = msg_store
        .send_message_with_ttl(
            crate::teams::messages::NewMessage {
                team_id: params.team_id.clone(),
                from_agent: crate::teams::broadcast::RESERVED_USER_HANDLE.to_string(),
                msg_type: crate::teams::messages::MessageType::Message,
                subject: String::new(),
                content: params.message.clone(),
                recipients: Vec::new(),
                reply_to: None,
                attachments: Vec::new(),
            },
            chrono::Duration::days(3650),
        )
        .await
    {
        warn!(team_id = %params.team_id, error = %e,
            "team chat: failed to persist user message into the team transcript");
    }

    // First-message auto-name: if this team was created with a blank name
    // (auto_name flag set), generate an LLM topic now that we have content.
    // The flag is a one-shot gate (atomic take-and-clear), so this fires
    // exactly once — on the first send — and never overrides explicit names.
    if let (Some(provider), Some(bus)) = (topic_provider.as_ref(), event_bus.as_ref()) {
        if store
            .take_auto_name_flag(&params.team_id)
            .await
            .unwrap_or(false)
        {
            let provider = Arc::clone(provider);
            let bus = Arc::clone(bus);
            let store = Arc::clone(&store);
            let team_id = params.team_id.clone();
            let first_message = params.message.clone();
            tokio::spawn(async move {
                let topic = crate::gateway::execution_engine::topic::generate_conversation_topic(
                    &provider,
                    &first_message,
                )
                .await;
                match store.rename_team(&team_id, &topic).await {
                    Ok(()) => {
                        let _ = bus.publish_frame(
                            &crate::gateway::events::GatewayEventFrame::TeamChanged {
                                team_id: team_id.clone(),
                                change: crate::gateway::events::ChangeKind::Updated,
                            },
                        );
                    }
                    Err(e) => {
                        warn!(team_id = %team_id, error = %e, "team auto-name: rename failed")
                    }
                }
            });
        }
    }

    // Equal broadcast: fan out by @mention (no @ → leader fallback); agents may
    // @ each other to continue the thread, bounded by the chain_depth guard.
    let broadcaster = crate::teams::broadcast::GroupChatBroadcaster::new(
        Arc::clone(&context),
        Arc::clone(&store),
        Arc::clone(&msg_store),
        team_planner_provider,
        coord_task_store,
        broadcast_cfg,
    );
    // The minted run_id is the fan-out TREE identity. Register the tree
    // BEFORE spawning the dispatch (and before this RPC returns), so the id
    // the Panel receives is cancellable the instant it is visible —
    // `teams.chat.cancel` can never race the registration into NOT_FOUND.
    let run_id = uuid::Uuid::new_v4().to_string();
    let fanout = crate::teams::broadcast::GroupChatBroadcaster::register_fanout(
        run_id.clone(),
        &params.team_id,
    );
    let team_id = params.team_id.clone();
    let message = params.message.clone();
    tokio::spawn(async move {
        broadcaster.dispatch_user(team_id, message, fanout).await;
    });

    JsonRpcResponse::success(request.id, serde_json::json!({ "run_id": run_id }))
}

#[derive(Debug, serde::Deserialize)]
pub struct ChatCancelParams {
    pub run_id: String,
}

/// teams.chat.cancel — stop an in-flight `teams.chat.send` fan-out tree.
///
/// `run_id` is the id `teams.chat.send` returned (the tree's tracker node,
/// registered by `dispatch_user`). Order matters: poison FIRST so the
/// recursive fan-out spawns no new members while we walk, THEN abort every
/// in-flight member run through the engine's per-run CancellationToken path —
/// the same `ExecutionAdapter::cancel` the `agent.cancel` RPC uses. Pure I/O
/// plumbing (R4): no reasoning, no new registry — the tree lives in the
/// process-global `BackgroundAgentTracker`.
pub async fn handle_chat_cancel(
    request: JsonRpcRequest,
    context: Arc<crate::gateway::context::GatewayContext>,
) -> JsonRpcResponse {
    let params: ChatCancelParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    debug!(run_id = %params.run_id, "Handling teams.chat.cancel request");
    if params.run_id.trim().is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "run_id is required".to_string(),
        );
    }

    let tracker = crate::agents::background_tracker::BackgroundAgentTracker::global();
    // 1. Poison the tree: fires the tree-level token stored at registration,
    //    so dispatch/run_member spawn no further members.
    let tree_found = tracker.cancel(&params.run_id);
    if !tree_found {
        return JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            format!(
                "No running team-chat fan-out with run_id '{}' (already settled or unknown)",
                params.run_id
            ),
        );
    }
    // 2. Walk the tree: every still-running member registered under this
    //    run_id gets its engine run aborted (fires the per-run token). A
    //    member whose engine run already finished (or has not started yet)
    //    yields a cancel error — counted, not fatal; the tree poison covers
    //    the not-yet-started ones.
    let members = tracker.running_children_of(&params.run_id);
    let mut signalled = 0usize;
    for member_run_id in &members {
        if context
            .execution_adapter()
            .cancel(member_run_id)
            .await
            .is_ok()
        {
            signalled += 1;
        }
    }
    JsonRpcResponse::success(
        request.id,
        json!({
            "run_id": params.run_id,
            "cancelled": true,
            "members_running": members.len(),
            "members_signalled": signalled,
        }),
    )
}
