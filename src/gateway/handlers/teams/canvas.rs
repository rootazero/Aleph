//! teams.chat.send handlers.

use serde_json::json;
use tracing::{debug, warn};

use crate::agents::swarm::tasks::CoordTaskStore;
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;

use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};

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
