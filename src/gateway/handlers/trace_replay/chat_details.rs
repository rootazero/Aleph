//! Addressed, presentation-only tool history for the V1 chat panel.

use std::collections::{HashMap, HashSet};

use aleph_protocol::chat_transcript::{
    ChatRunDetail, ChatToolChange, ChatToolDelta, ChatTranscriptDetails,
    ChatTranscriptDetailsRequest,
};
use serde_json::json;

use super::{visible_history_runs, JsonRpcRequest, JsonRpcResponse, SessionKey, SessionStore};
use crate::gateway::handlers::chat_presentation::{
    project_tool_finish, project_tool_start, safe_tool_address,
};
use crate::gateway::{protocol::INVALID_PARAMS, visibility};
use crate::resilience::{StateDatabase, TaskStatus};
use crate::session::store::StoredToolPresentation;
use crate::sync_primitives::Arc;

const MAX_RUNS: usize = 12;
const MAX_DELTAS: usize = 80;
const MAX_RESPONSE_BYTES: usize = 384 * 1024;
// The projector applies its independent wire budget after masking.
const MAX_STORED_PRESENTATION_BYTES: usize = 96 * 1024;
// Keep space for the response envelope and at most 12 bounded run IDs.
const MAX_DELTA_BYTES: usize = MAX_RESPONSE_BYTES - 8 * 1024;

pub async fn handle_transcript_details(
    request: JsonRpcRequest,
    db: Arc<StateDatabase>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    // A JSON-RPC id is caller-controlled and otherwise could consume the
    // entire presentation frame despite the budget on the result.
    if serde_json::to_vec(&request.id).map_or(true, |bytes| bytes.len() > 1024) {
        return JsonRpcResponse::error(None, INVALID_PARAMS, "Request id too large");
    }
    let Some(params) = request
        .params
        .and_then(|value| serde_json::from_value::<ChatTranscriptDetailsRequest>(value).ok())
    else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid params");
    };
    let Some(session_key) = SessionKey::from_key_string(&params.session_key) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key");
    };
    let Some(owned) = visible_history_runs(sessions.as_ref(), &session_key).await else {
        return visibility::not_found_response(request.id);
    };

    let mut details = ChatTranscriptDetails {
        runs: Vec::new(),
        limited: params.run_ids.len() > MAX_RUNS,
    };
    let mut seen_runs = HashSet::new();
    let mut runs = Vec::new();
    let canonical_session = session_key.to_key_string();
    for (index, run_id) in params.run_ids.into_iter().take(MAX_RUNS).enumerate() {
        if !safe_tool_address(&run_id)
            || !owned.contains(&run_id)
            || !seen_runs.insert(run_id.clone())
        {
            continue;
        }
        let Ok(Some(task)) = db.get_agent_task(&run_id).await else {
            details.limited = true;
            continue;
        };
        // A history stamp alone cannot prove the task trace belongs to this
        // session (for example, after a split or older projector repair).
        if task.parent_session_id != canonical_session {
            details.limited = true;
            continue;
        }
        let terminal = matches!(
            task.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Interrupted
        );
        runs.push((index, run_id, terminal));
    }
    // Under pressure, retain settled work first and favor recent runs. The
    // reply itself keeps request order, and every run keeps its trace order.
    runs.sort_by_key(|(index, _, terminal)| (!terminal, std::cmp::Reverse(*index)));
    let mut candidates = Vec::new();
    let mut completed_calls = Vec::new();
    let mut remaining = MAX_DELTAS;
    for (index, run_id, terminal) in runs {
        if remaining == 0 {
            details.limited = true;
            break;
        }
        let Ok(mut facts) = db.tool_trace_facts(&run_id, MAX_DELTAS + 1).await else {
            details.limited = true;
            continue;
        };
        let bounded = facts.len() > MAX_DELTAS;
        facts.truncate(MAX_DELTAS);
        let mut limited = bounded;
        let mut calls = HashMap::<&str, (usize, usize, usize)>::new();
        for (position, fact) in facts.iter().enumerate() {
            let Some(id) = fact.call_id.as_deref().filter(|id| safe_tool_address(id)) else {
                limited = true;
                continue;
            };
            let (starts, finishes, first) = calls.entry(id).or_insert((0, 0, position));
            if fact.finished {
                if fact.success.is_some() && fact.duration_ms.is_some() {
                    *finishes += 1;
                } else {
                    // Reject the whole call, not just the unusable finish.
                    *finishes += 2;
                }
            } else {
                *starts += 1;
                *first = (*first).min(position);
            }
        }
        let mut chosen = HashSet::new();
        let mut ordered_calls: Vec<_> = calls.into_iter().collect();
        ordered_calls.sort_by_key(|(_, (_, _, first))| *first);
        for (id, (starts, finishes, _)) in ordered_calls {
            if starts > 1
                || finishes > 1
                || (finishes == 0 && (terminal || bounded))
                || starts + finishes > remaining
            {
                limited = true;
                continue;
            }
            chosen.insert(id);
            remaining -= starts + finishes;
            if finishes == 1 {
                completed_calls.push((run_id.clone(), id.to_owned()));
            }
        }
        let tools = facts
            .iter()
            .filter_map(|fact| {
                let id = fact.call_id.as_deref().filter(|id| chosen.contains(id))?;
                let change = if fact.finished {
                    project_tool_finish(fact.success?, fact.duration_ms?, None)
                } else {
                    project_tool_start(
                        fact.tool_name.as_deref().unwrap_or_default(),
                        &json!({"path": fact.path}),
                    )
                };
                Some(ChatToolDelta {
                    tool_call_id: id.to_owned(),
                    change,
                })
            })
            .collect::<Vec<_>>();
        details.limited |= limited;
        if !tools.is_empty() {
            candidates.push((
                index,
                ChatRunDetail {
                    run_id,
                    tools,
                    limited,
                },
            ));
        }
    }

    let presentations = if completed_calls.is_empty() {
        Some(HashMap::new())
    } else if let Some(store) = crate::session::store::global_session_event_store() {
        store
            .load_tool_presentations(
                &session_key,
                &completed_calls,
                MAX_STORED_PRESENTATION_BYTES,
            )
            .await
            .ok()
    } else {
        None
    };
    let mut presentations = presentations.unwrap_or_default();
    // Reserve all selected statuses before spending bytes on optional diffs.
    let lean_bytes = candidates
        .iter()
        .flat_map(|(_, run)| &run.tools)
        .fold(0usize, |total, delta| {
            total.saturating_add(encoded_delta_bytes(delta))
        });
    let mut remaining_bytes = MAX_DELTA_BYTES.saturating_sub(lean_bytes);
    for (_, run) in &mut candidates {
        for delta in &mut run.tools {
            let ChatToolChange::Finished {
                success,
                duration_ms,
                ..
            } = delta.change
            else {
                continue;
            };
            let stored = presentations.remove(&(run.run_id.clone(), delta.tool_call_id.clone()));
            let (presentation, unavailable) = match stored {
                Some(StoredToolPresentation::Present(body)) => (Some(body), false),
                Some(StoredToolPresentation::Absent) => (None, false),
                Some(StoredToolPresentation::Unavailable) | None => (None, true),
            };
            let mut enriched = ChatToolDelta {
                tool_call_id: delta.tool_call_id.clone(),
                change: project_tool_finish(success, duration_ms, presentation),
            };
            let extra = encoded_delta_bytes(&enriched).saturating_sub(encoded_delta_bytes(delta));
            if let ChatToolChange::Finished {
                presentation,
                presentation_omitted,
                ..
            } = &mut enriched.change
            {
                *presentation_omitted |= unavailable;
                if extra > remaining_bytes {
                    *presentation_omitted |= presentation.take().is_some();
                } else {
                    remaining_bytes -= extra;
                }
                run.limited |= *presentation_omitted;
            }
            *delta = enriched;
        }
        details.limited |= run.limited;
    }
    candidates.sort_by_key(|(index, _)| *index);
    details.runs = candidates.into_iter().map(|(_, run)| run).collect();
    let response = JsonRpcResponse::success(
        request.id,
        serde_json::to_value(details).unwrap_or_default(),
    );
    if serde_json::to_vec(&response).map_or(true, |bytes| bytes.len() > MAX_RESPONSE_BYTES) {
        return JsonRpcResponse::success(response.id, json!({ "runs": [], "limited": true }));
    }
    response
}

fn encoded_delta_bytes(delta: &ChatToolDelta) -> usize {
    serde_json::to_vec(delta).map_or(usize::MAX, |bytes| bytes.len().saturating_add(1))
}
