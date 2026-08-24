//! teams.chat.send handlers.

use serde_json::json;
use tracing::{debug, warn};

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
    security_store: Arc<crate::gateway::security::SecurityStore>,
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

    // Team must exist AND belong to the caller (404 for either — same
    // response). We don't bind it here: the broadcaster re-reads team + roster
    // per dispatch round, through the same scoped store.
    if let Err(resp) = crate::gateway::handlers::teams::visibility::gate_team(
        request.id.clone(),
        &store,
        &params.team_id,
    )
    .await
    {
        return resp;
    }

    // Group-chat broadcast needs the message store for the shared transcript.
    let Some(msg_store) = msg_store else {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "team message store unavailable; group chat cannot persist transcript".to_string(),
        );
    };

    // The speaker (spec §6.2 humanization): the gateway caller's identity, or
    // (in a project room) the room's actual speaker rather than the room's
    // creator — see `visibility::ambient_actor`'s doc for why `ambient_owner`
    // alone would answer with the wrong person here. `None` for an
    // unattributed/internal caller (legacy behavior — the message is stored
    // without an author, same as before this field existed).
    let author = crate::gateway::visibility::ambient_actor();

    // Persist the user message into the shared transcript (single source of
    // truth). Long TTL: the transcript is a durable record, not short-lived
    // inbox traffic (recipient-less messages would otherwise expire in 30 min).
    // A persistence failure must be visible — the fan-out below still runs,
    // but the triggering message would be a silent hole in the durable
    // transcript (P7). The persisted row's id becomes `message_id` in the
    // response and the `.message` event below; a failed persist still lets
    // the turn proceed (existing warn-and-continue), just with no id to hand
    // back.
    let persisted_id: Option<String> = match msg_store
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
                author_user_id: author.clone(),
            },
            chrono::Duration::days(3650),
        )
        .await
    {
        Ok(msg) => Some(msg.id),
        Err(e) => {
            warn!(team_id = %params.team_id, error = %e,
                "team chat: failed to persist user message into the team transcript");
            None
        }
    };

    // Multi-human activation gate (spec §6.2): with exactly one distinct
    // human author ever seen in this transcript, every message still
    // activates — byte-identical to the single-user history. Once a SECOND
    // human has spoken, a message only activates the roster when it
    // @-mentions a member or @all/@everyone; otherwise it is observed
    // (persisted + broadcast, no run minted). A store `Err` on either read
    // below must not let this new mechanism swallow a message into observe
    // mode — `unwrap_or_default()` keeps `humans.len()` from exceeding 1 on
    // a failed author count (so the gate falls through to "activates"), and
    // an empty roster on a failed member read only suppresses recognizing an
    // explicit @-mention, never an `@all`/`@everyone` broadcast.
    let mut humans: std::collections::HashSet<String> = msg_store
        .distinct_human_authors(&params.team_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    if let Some(a) = &author {
        humans.insert(a.clone());
    }
    let roster_ids: Vec<String> = store
        .get_members(&params.team_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.agent_id)
        .collect();
    let observe = humans.len() > 1
        && !crate::teams::broadcast::targets::has_activation_mention(&params.message, &roster_ids);

    // Real-time fan-out to the OTHER humans in the thread — both modes emit
    // this; an observed message is still live-visible to the room, it just
    // does not mint a run. `display_name` is UI sugar (never persisted): a
    // failed/missing user lookup falls back to the raw id, same as every
    // other display-name resolution in this crate.
    let display_name: Option<String> = author.as_ref().map(|uid| {
        security_store
            .get_user(uid)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .unwrap_or_else(|| uid.clone())
    });
    crate::gateway::event_emitter::team_fanout::publish_team_event(
        &params.team_id,
        "message",
        serde_json::json!({
            "text": params.message, "message_id": persisted_id,
            "author_user_id": author, "author_display_name": display_name,
        }),
    );

    if observe {
        // Observed: no roster member was addressed, so no run is minted —
        // skip auto-name/broadcaster/fanout/spawn entirely. The message is
        // already durable (above) and already broadcast (above); there is
        // nothing else for this turn to do.
        return JsonRpcResponse::success(
            request.id,
            serde_json::json!({ "run_id": null, "observed": true, "message_id": persisted_id }),
        );
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
    // `teams.` is member-reachable, and this spawn happens inside a gateway
    // dispatch where the caller's scope IS live — so a bare `tokio::spawn`
    // throws away a real attribution. Everything downstream then ran with
    // `current_scope() == None`: member session rows written NULL/NULL (adopted
    // by the operator), and every memory write filed into the BASE partition
    // that `session_read_ids` unions into every other principal's turn.
    //
    // `with_speaker(author)` overrides the carried room-author task-local
    // with the speaker this handler already resolved above. Without it, a
    // spawn here that never had `scope::with_room_author` seeded around it
    // (nothing on this RPC path seeds it) carries `room_author: None`, and
    // `ambient_room_author()` inside the fan-out then falls back to the
    // scope's `owner_user_id` — the ROOM'S CREATOR, identically for every
    // member. A message from Bob in Alice's room would resolve its actor to
    // Alice: confidently wrong, not merely missing (see
    // `CarriedAttribution`'s doc and `with_speaker`'s doc for the full
    // shape). `with_speaker` only overwrites when `author.is_some()`, so an
    // unattributed call keeps whatever `capture()` read — never widens,
    // never blanks.
    let carried = crate::scope::CarriedAttribution::capture().with_speaker(author.clone());
    tokio::spawn(carried.reestablish(async move {
        broadcaster.dispatch_user(team_id, message, fanout).await;
    }));

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({ "run_id": run_id, "observed": false, "message_id": persisted_id }),
    )
}

#[derive(Debug, serde::Deserialize)]
pub struct ChatCancelParams {
    pub run_id: String,
}

/// The response an uncancellable `run_id` produces — whether no such fan-out
/// was ever minted here, it already settled, or it belongs to a team the caller
/// cannot reach.
///
/// One constructor so the three causes stay byte-identical. Ownership is
/// resolved BEFORE the tracker is consulted, so a caller holding someone else's
/// tree id learns nothing from the reply — not even that the run exists. Same
/// no-existence-oracle contract as
/// [`visibility::team_not_found`](crate::gateway::handlers::teams::visibility::team_not_found),
/// keyed on the id the caller supplied, and the wording is the one this handler
/// already returned for an unknown id so a single-user deployment sees
/// byte-identical output to before.
#[must_use]
fn fanout_not_cancellable(id: Option<serde_json::Value>, run_id: &str) -> JsonRpcResponse {
    JsonRpcResponse::error(
        id,
        RESOURCE_NOT_FOUND,
        format!("No running team-chat fan-out with run_id '{run_id}' (already settled or unknown)"),
    )
}

/// teams.chat.cancel — stop an in-flight `teams.chat.send` fan-out tree.
///
/// `run_id` is the id `teams.chat.send` returned (the tree's tracker node,
/// registered by `register_fanout`).
///
/// **Ownership gate (2026-08-07).** The tracker is process-global and keyed by
/// run id, so the run → team mapping this needs comes from
/// [`broadcast::team_of_fanout_run`](crate::teams::broadcast::team_of_fanout_run),
/// recorded at the single point a tree run id is minted. The team is then run
/// through the same [`gate_team`](super::visibility::gate_team) every other
/// addressed `teams.*` method uses, so this handler is enforced by the one
/// ownership predicate rather than a second derivation of it. A team deleted
/// mid-fan-out therefore refuses even its owner — the same fail-closed reading
/// `gate_task` gives an orphaned task, and the fan-out's own recursion already
/// stops at the next level when `get_team` comes back empty.
///
/// It was previously open, on the recorded reasoning that a run id is an
/// unguessable capability obtainable only from one's own `teams.chat.send`
/// response or a `team.<id>.fanout` event one was already entitled to. Both
/// halves of that are the wrong kind of guarantee: the entitlement clause made
/// this handler's safety depend on another subsystem's event-classification
/// table with no compile-time link to it, and that table did not in fact hold —
/// the whole `team.*` plane classified as `Global`, so every connected user
/// received every other user's tree run ids until the plane was owner-scoped in
/// the same round as this gate.
///
/// Order matters twice. The gate runs BEFORE the poison: a refused caller must
/// not be able to kill a fan-out on their way to a `not_found`, which is the
/// difference between a gate and a decoration. Then poison FIRST so the
/// recursive fan-out spawns no new members while we walk, THEN abort every
/// in-flight member run through the engine's per-run CancellationToken path —
/// the same `ExecutionAdapter::cancel` the `agent.cancel` RPC uses. Pure I/O
/// plumbing (R4): no reasoning — the tree lives in the process-global
/// `BackgroundAgentTracker`.
pub async fn handle_chat_cancel(
    request: JsonRpcRequest,
    context: Arc<crate::gateway::context::GatewayContext>,
    store: Arc<dyn TeamStore>,
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

    // 0. Ownership. An id this process never minted is unattributable and is
    //    refused for the same reason a foreign team is: fail closed, and say
    //    the same thing either way.
    let Some(team_id) = crate::teams::broadcast::team_of_fanout_run(&params.run_id) else {
        return fanout_not_cancellable(request.id, &params.run_id);
    };
    if crate::gateway::handlers::teams::visibility::gate_team(request.id.clone(), &store, &team_id)
        .await
        .is_err()
    {
        // Deliberately NOT the gate's own team-shaped response: it names the
        // team id, which is precisely the identifier a caller who could not
        // see the team must not learn.
        return fanout_not_cancellable(request.id, &params.run_id);
    }

    let tracker = crate::agents::background_tracker::BackgroundAgentTracker::global();
    // 1. Poison the tree: fires the tree-level token stored at registration,
    //    so dispatch/run_member spawn no further members.
    let tree_found = tracker.cancel(&params.run_id);
    if !tree_found {
        return fanout_not_cancellable(request.id, &params.run_id);
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
