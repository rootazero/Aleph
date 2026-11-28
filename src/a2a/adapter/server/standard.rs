use std::convert::Infallible;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::a2a::adapter::wire::{self, WireVersion};
use crate::a2a::domain::{A2AError, A2ATask, ListTasksParams, TaskState};
use crate::a2a::port::A2AResult;
use crate::a2a::port::authenticator::{A2AAction, A2AAuthContext, A2AAuthPrincipal};
use crate::a2a::port::streaming::TaskUpdateStream;
use crate::sync_primitives::Arc;

use super::request_processor::{A2AServerState, JsonRpcRequest, JsonRpcResponse};
use super::routes::{PeerAddr, extract_credentials, headers_to_map, serialize_rpc_frame};
use super::task_store::TaskScope;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskParams {
    id: String,
    history_length: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListParams {
    context_id: Option<String>,
    status: Option<String>,
    status_timestamp_after: Option<chrono::DateTime<chrono::Utc>>,
    page_size: Option<usize>,
    history_length: Option<usize>,
    include_artifacts: Option<bool>,
    page_token: Option<String>,
}

enum Operation {
    Send { streaming: bool },
    Get,
    Cancel,
    Subscribe,
    List,
}

impl Operation {
    fn parse(version: WireVersion, method: &str) -> A2AResult<Self> {
        let [send, stream, get, cancel, subscribe] = version.methods();
        match method {
            name if name == send => Ok(Self::Send { streaming: false }),
            name if name == stream => Ok(Self::Send { streaming: true }),
            name if name == get => Ok(Self::Get),
            name if name == cancel => Ok(Self::Cancel),
            name if name == subscribe => Ok(Self::Subscribe),
            "ListTasks" if version == WireVersion::V10 => Ok(Self::List),
            _ => Err(A2AError::MethodNotFound(method.to_owned())),
        }
    }

    fn action(&self) -> A2AAction {
        match self {
            Self::Send { .. } => A2AAction::SendMessage,
            Self::Get => A2AAction::GetTask,
            Self::Cancel => A2AAction::CancelTask,
            Self::Subscribe => A2AAction::Subscribe,
            Self::List => A2AAction::ListTasks,
        }
    }
}

pub(super) async fn handler(
    State(state): State<Arc<A2AServerState>>,
    headers: HeaderMap,
    PeerAddr(remote_addr): PeerAddr,
    Json(request): Json<JsonRpcRequest>,
) -> Response {
    let auth = A2AAuthContext {
        remote_addr,
        headers: headers_to_map(&headers),
        credentials: extract_credentials(&headers),
    };
    let principal = match state.authenticator.authenticate(&auth).await {
        Ok(principal) => principal,
        Err(error) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(rpc_error(request.id, &error)),
            )
                .into_response();
        }
    };
    let version = match headers.get("a2a-version") {
        None => WireVersion::V03,
        Some(value) => match value.to_str().ok().and_then(WireVersion::parse) {
            Some(version) => version,
            None => {
                return Json(JsonRpcResponse::error(
                    request.id,
                    -32009,
                    "A2A version not supported",
                ))
                .into_response();
            }
        },
    };
    let id = request.id.clone();
    match dispatch(&state, &principal, version, request).await {
        Ok(response) => response,
        Err(error) => Json(rpc_error(id, &error)).into_response(),
    }
}

fn rpc_error(id: Option<Value>, error: &A2AError) -> JsonRpcResponse {
    match error {
        A2AError::UnsupportedContentType => {
            JsonRpcResponse::error(id, -32005, "Content type not supported")
        }
        A2AError::Forbidden => JsonRpcResponse::error(id, -32000, "Forbidden"),
        _ => JsonRpcResponse::from_a2a_error(id, error),
    }
}

async fn dispatch(
    state: &A2AServerState,
    principal: &A2AAuthPrincipal,
    version: WireVersion,
    request: JsonRpcRequest,
) -> A2AResult<Response> {
    if request.jsonrpc != "2.0" {
        return Err(A2AError::InvalidRequest("Expected JSON-RPC 2.0".into()));
    }
    let operation = Operation::parse(version, &request.method)?;
    if !state
        .authenticator
        .authorize(principal, &operation.action())
        .await?
    {
        return Err(A2AError::Forbidden);
    }
    let params = if request.params.is_null() {
        json!({})
    } else {
        request.params
    };
    if params
        .get("tenant")
        .is_some_and(|tenant| tenant.as_str() != Some(""))
    {
        return Err(A2AError::InvalidParams(
            "This server has no named tenants".into(),
        ));
    }
    let id = request.id;
    let owner = principal.owner;
    let scope = TaskScope::Standard(owner);
    let result = match operation {
        Operation::Send { streaming } => {
            let send: wire::SendParams = wire::from_value(params)?;
            send.configuration.validate()?;
            let immediate = send.configuration.immediate(version);
            let history_length = send.configuration.history_length;
            let input = send.into_message(version)?;
            let claim = state
                .task_manager
                .claim_standard(
                    owner,
                    input.task_id.as_deref(),
                    input.context_id.as_deref(),
                    &input.reference_task_ids,
                )
                .await?;
            if streaming || immediate {
                let initial = limit_history(claim.task.clone(), history_length);
                let updates = state
                    .message_handler
                    .handle_message_stream(claim, input.message)
                    .await?;
                if streaming {
                    return Ok(sse(id, version, initial, Some(updates)));
                }
                wire::task_result(version, &initial)
            } else {
                let task = state
                    .message_handler
                    .handle_message(claim, input.message)
                    .await?;
                wire::task_result(version, &limit_history(task, history_length))
            }
        }
        Operation::Get => {
            let query: TaskParams = wire::from_value(params)?;
            let task = state
                .task_manager
                .get_owned(scope, &query.id, query.history_length)
                .await?;
            wire::task(version, &task)
        }
        Operation::Cancel => {
            let query: TaskParams = wire::from_value(params)?;
            let task = state.task_manager.cancel_owned(scope, &query.id).await?;
            wire::task(version, &task)
        }
        Operation::Subscribe => {
            let query: TaskParams = wire::from_value(params)?;
            let (task, updates) = state
                .task_manager
                .observe_owned(scope, &query.id, state.streaming.as_ref())
                .await?;
            if version == WireVersion::V10 && task.status.state.is_terminal() {
                return Err(A2AError::UnsupportedOperation("Task has already settled"));
            }
            return Ok(sse(
                id,
                version,
                limit_history(task, query.history_length),
                updates,
            ));
        }
        Operation::List => {
            let query: ListParams = wire::from_value(params)?;
            list_tasks(state, principal, query).await?
        }
    };
    Ok(Json(JsonRpcResponse::success(id, result)).into_response())
}

fn limit_history(mut task: A2ATask, history_length: Option<usize>) -> A2ATask {
    if let Some(limit) = history_length {
        let start = task.history.len().saturating_sub(limit);
        task.history.drain(..start);
    }
    task
}

async fn list_tasks(
    state: &A2AServerState,
    principal: &A2AAuthPrincipal,
    query: ListParams,
) -> A2AResult<Value> {
    const STATES: [TaskState; 8] = [
        TaskState::Submitted,
        TaskState::Working,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Canceled,
        TaskState::InputRequired,
        TaskState::Rejected,
        TaskState::AuthRequired,
    ];
    let state_filter = query
        .status
        .as_deref()
        .map(|status| {
            STATES
                .into_iter()
                .find(|state| wire::state_name(*state) == status)
                .map(|state| vec![state])
                .ok_or_else(|| A2AError::InvalidParams("Unknown task status".into()))
        })
        .transpose()?;
    let cursor = query
        .page_token
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| {
            let (time, id) = value
                .split_once('/')
                .ok_or_else(|| A2AError::InvalidParams("Invalid page token".into()))?;
            let time = time
                .parse::<i64>()
                .map_err(|_| A2AError::InvalidParams("Invalid page token".into()))?;
            Ok::<_, A2AError>((time, id))
        })
        .transpose()?;
    let page_size = query.page_size.unwrap_or(50);
    if !(1..=100).contains(&page_size) {
        return Err(A2AError::InvalidParams(
            "pageSize must be between 1 and 100".into(),
        ));
    }
    let page = state
        .task_manager
        .list_owned(
            TaskScope::Standard(principal.owner),
            ListTasksParams {
                context_id: query.context_id,
                state_filter,
                limit: Some(usize::MAX),
                ..Default::default()
            },
        )
        .await;
    let visible = page.tasks.into_iter().filter(|task| {
        query
            .status_timestamp_after
            .is_none_or(|time| task.status.timestamp >= time)
    });
    let total = visible.clone().count();
    let mut page: Vec<_> = visible
        .filter(|task| {
            cursor.is_none_or(|(time, id)| {
                let timestamp = task
                    .status
                    .timestamp
                    .timestamp_nanos_opt()
                    .unwrap_or_default();
                timestamp < time || (timestamp == time && task.id.as_str() > id)
            })
        })
        .take(page_size + 1)
        .collect();
    let next = if page.len() > page_size {
        page.pop();
        let last = page.last().expect("page has at least one item");
        format!(
            "{}/{}",
            last.status
                .timestamp
                .timestamp_nanos_opt()
                .unwrap_or_default(),
            last.id
        )
    } else {
        String::new()
    };
    let tasks: Vec<_> = page
        .into_iter()
        .map(|task| {
            let mut task = limit_history(task, query.history_length);
            if query.include_artifacts != Some(true) {
                task.artifacts.clear();
            }
            wire::task(WireVersion::V10, &task)
        })
        .collect();
    Ok(json!({"tasks": tasks, "pageSize": page_size, "totalSize": total, "nextPageToken": next}))
}

fn sse(
    id: Option<Value>,
    version: WireVersion,
    snapshot: A2ATask,
    updates: Option<TaskUpdateStream>,
) -> Response {
    let first = JsonRpcResponse::success(id.clone(), wire::task_result(version, &snapshot));
    let events = futures::stream::once(async move { first }).chain(
        futures::stream::iter(updates)
            .flatten()
            .map(move |event| match event {
                Ok(event) => JsonRpcResponse::success(id.clone(), wire::update(version, &event)),
                Err(error) => rpc_error(id.clone(), &error),
            }),
    );
    Sse::new(
        events.map(|response| {
            Ok::<_, Infallible>(Event::default().data(serialize_rpc_frame(&response)))
        }),
    )
    .keep_alive(KeepAlive::new())
    .into_response()
}
