use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use crate::a2a::domain::{
    A2AError, A2AMessage, A2ARole, A2ATask, AgentCard, AgentInterface, Artifact,
    TaskArtifactUpdateEvent, TaskStatus, TaskStatusUpdateEvent, TransportProtocol, UpdateEvent,
};
use crate::a2a::port::{append_path, http_url, A2AResult, ResolvedEndpoint, RpcProtocol};

impl ResolvedEndpoint {
    pub(crate) fn method(&self) -> &'static str {
        match (self.protocol(), self.streaming()) {
            (RpcProtocol::Legacy | RpcProtocol::V03, false) => "message/send",
            (RpcProtocol::Legacy | RpcProtocol::V03, true) => "message/stream",
            (RpcProtocol::V1, false) => "SendMessage",
            (RpcProtocol::V1, true) => "SendStreamingMessage",
        }
    }

    pub(crate) fn params(
        &self,
        task_id: &str,
        message: &A2AMessage,
        session_id: Option<&str>,
    ) -> Value {
        match self.protocol() {
            RpcProtocol::Legacy => {
                let mut params = json!({"taskId": task_id, "message": message});
                if let Some(session_id) = session_id {
                    params["sessionId"] = json!(session_id);
                }
                params
            }
            RpcProtocol::V03 => json!({
                "message": {
                    "kind": "message", "role": "user", "messageId": message.message_id,
                    "parts": [{"kind": "text", "text": message.text_content()}],
                },
                "configuration": {"blocking": true, "acceptedOutputModes": ["text/plain"]},
            }),
            RpcProtocol::V1 => json!({
                "message": {
                    "role": "ROLE_USER", "messageId": message.message_id,
                    "parts": [{"text": message.text_content()}],
                },
                "configuration": {"acceptedOutputModes": ["text/plain"]},
            }),
        }
    }
}

pub(crate) fn discover_urls(base: &Url) -> Vec<Url> {
    const WELL_KNOWN: &str = ".well-known/agent-card.json";
    let path = base.path();
    if path.contains("/.well-known/") || path.ends_with(".json") || base.query().is_some() {
        return vec![base.clone()];
    }
    let conventional = append_path(base, WELL_KNOWN);
    if path.ends_with('/') || path.trim_end_matches('/') == "/a2a/legacy" {
        return vec![conventional];
    }
    // An unmarked path could be a direct card URL or a reverse-proxy root.
    vec![conventional, base.clone()]
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StandardCard {
    name: String,
    version: String,
    description: Option<String>,
    provider: Option<crate::a2a::domain::AgentProvider>,
    documentation_url: Option<String>,
    #[serde(default)]
    skills: Vec<crate::a2a::domain::AgentSkill>,
    #[serde(default)]
    default_input_modes: Vec<String>,
    #[serde(default)]
    default_output_modes: Vec<String>,
}

pub(crate) fn resolve_card(
    value: Value,
    agent_id: &str,
    discovery: &Url,
    legacy_root: Option<&Url>,
    pin: Option<&Url>,
) -> A2AResult<(AgentCard, ResolvedEndpoint)> {
    let streaming = value
        .pointer("/capabilities/streaming")
        .and_then(Value::as_bool)
        == Some(true);
    let mut candidates = Vec::new();
    let standard = value.get("supportedInterfaces").is_some()
        || value.get("url").is_some()
        || value.get("protocolVersion").is_some();
    let card = if standard {
        if let Some(interfaces) = value.get("supportedInterfaces").and_then(Value::as_array) {
            for interface in interfaces {
                if interface.get("protocolBinding").and_then(Value::as_str) != Some("JSONRPC")
                    || interface
                        .get("tenant")
                        .and_then(Value::as_str)
                        .is_some_and(|tenant| !tenant.is_empty())
                {
                    continue;
                }
                let protocol = match interface.get("protocolVersion").and_then(Value::as_str) {
                    Some("1.0" | "1.0.0") => RpcProtocol::V1,
                    Some("0.3" | "0.3.0") => RpcProtocol::V03,
                    _ => continue,
                };
                if let Some(url) = interface.get("url").and_then(Value::as_str) {
                    candidates.push((protocol, url.to_owned()));
                }
            }
        }
        if matches!(
            value.get("protocolVersion").and_then(Value::as_str),
            None | Some("0.3" | "0.3.0")
        ) {
            if let Some(url) = value.get("url").and_then(Value::as_str) {
                if matches!(
                    value.get("preferredTransport").and_then(Value::as_str),
                    None | Some("JSONRPC")
                ) {
                    candidates.push((RpcProtocol::V03, url.to_owned()));
                }
            }
            if let Some(interfaces) = value.get("additionalInterfaces").and_then(Value::as_array) {
                for interface in interfaces {
                    if interface.get("transport").and_then(Value::as_str) == Some("JSONRPC") {
                        if let Some(url) = interface.get("url").and_then(Value::as_str) {
                            candidates.push((RpcProtocol::V03, url.to_owned()));
                        }
                    }
                }
            }
        }
        let standard: StandardCard = serde_json::from_value(value).map_err(parse_error)?;
        AgentCard {
            id: agent_id.to_owned(),
            name: standard.name,
            version: standard.version,
            description: standard.description,
            provider: standard.provider,
            documentation_url: standard.documentation_url,
            skills: standard.skills,
            default_input_modes: standard.default_input_modes,
            default_output_modes: standard.default_output_modes,
            security: vec![],
            extensions: vec![],
            interfaces: vec![],
        }
    } else {
        let legacy: AgentCard = serde_json::from_value(value).map_err(parse_error)?;
        for interface in &legacy.interfaces {
            if matches!(interface.protocol, TransportProtocol::JsonRpc) {
                candidates.push((RpcProtocol::Legacy, interface.url.clone()));
            }
        }
        if legacy.interfaces.is_empty() {
            if let Some(root) = legacy_root {
                candidates.push((RpcProtocol::Legacy, append_path(root, "a2a").into()));
            } else if let Some(pin) = pin {
                candidates.push((RpcProtocol::Legacy, pin.as_str().to_owned()));
            }
        }
        legacy
    };

    let mut candidates: Vec<_> = candidates
        .into_iter()
        .filter_map(|(protocol, raw)| http_url(&raw).ok().map(|url| (protocol, url)))
        .collect();
    candidates.sort_by_key(|(protocol, url)| {
        (
            pin.is_some_and(|pin| pin != url),
            *protocol != RpcProtocol::V1,
        )
    });
    let (protocol, url) = candidates
        .into_iter()
        .find(|(_, url)| pin.is_some() || url.origin() == discovery.origin())
        .ok_or_else(|| {
            A2AError::InvalidParams(
                "No supported JSON-RPC endpoint on the configured A2A origin".into(),
            )
        })?;
    let url = pin.cloned().unwrap_or(url);
    let streaming = streaming
        && (protocol != RpcProtocol::Legacy || (pin.is_none() && url.path().ends_with("/a2a")));
    let endpoint = if streaming && protocol == RpcProtocol::Legacy {
        ResolvedEndpoint::new(append_path(&url, "stream"), protocol, true)
    } else {
        ResolvedEndpoint::new(url.clone(), protocol, streaming)
    };
    let mut card = card;
    card.interfaces = vec![AgentInterface {
        url: url.into(),
        protocol: TransportProtocol::JsonRpc,
    }];
    Ok((card, endpoint))
}

fn parse_error(error: serde_json::Error) -> A2AError {
    A2AError::ParseError(error.to_string())
}

fn invalid_result() -> A2AError {
    A2AError::ParseError("Invalid A2A result shape".into())
}

fn text_message(value: &Value) -> A2AMessage {
    let mut message = A2AMessage::text(A2ARole::Agent, text_parts(value));
    if let Some(id) = value.get("messageId").and_then(Value::as_str) {
        message.message_id = id.to_owned();
    }
    message
}

fn text_parts(value: &Value) -> String {
    value
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn text_artifact(value: &Value) -> A2AResult<Artifact> {
    let id = string_field(value, "artifactId")?;
    Ok(Artifact {
        artifact_id: id.to_owned(),
        kind: "artifact".into(),
        parts: text_message(value).parts,
        metadata: None,
    })
}

fn string_field<'a>(value: &'a Value, key: &str) -> A2AResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid_result)
}

fn task_status(value: &Value) -> A2AResult<TaskStatus> {
    let state = string_field(value, "state")?;
    let state = state
        .strip_prefix("TASK_STATE_")
        .map(|s| s.to_ascii_lowercase().replace('_', "-"))
        .unwrap_or_else(|| state.to_owned());
    Ok(TaskStatus {
        state: serde_json::from_value(Value::String(state)).map_err(parse_error)?,
        message: value
            .get("message")
            .filter(|msg| !msg.is_null())
            .map(text_message),
        timestamp: value
            .get("timestamp")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(parse_error)?
            .unwrap_or_else(chrono::Utc::now),
    })
}

pub(crate) fn decode_task(protocol: RpcProtocol, result: Value) -> A2AResult<A2ATask> {
    if protocol == RpcProtocol::Legacy {
        return serde_json::from_value(result).map_err(parse_error);
    }
    let (value, message_only) = if protocol == RpcProtocol::V1 {
        match (result.get("task"), result.get("message")) {
            (Some(task), None) => (task, false),
            (None, Some(message)) => (message, true),
            _ => return Err(invalid_result()),
        }
    } else {
        match result.get("kind").and_then(Value::as_str) {
            Some("task") => (&result, false),
            Some("message") => (&result, true),
            _ => return Err(invalid_result()),
        }
    };
    if message_only {
        let mut task = A2ATask::new(
            string_field(value, "messageId")?,
            value
                .get("contextId")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        task.status.state = crate::a2a::domain::TaskState::Completed;
        task.status.message = Some(text_message(value));
        return Ok(task);
    }
    let mut task = A2ATask::new(
        string_field(value, "id")?,
        string_field(value, "contextId")?,
    );
    task.status = task_status(&value["status"])?;
    task.artifacts = value
        .get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(text_artifact)
        .collect::<A2AResult<_>>()?;
    task.history = value
        .get("history")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|msg| {
            matches!(
                msg.get("role").and_then(Value::as_str),
                Some("agent" | "ROLE_AGENT")
            )
        })
        .map(text_message)
        .collect();
    Ok(task)
}

pub(crate) fn decode_stream_result(
    protocol: RpcProtocol,
    result: Value,
) -> A2AResult<Vec<UpdateEvent>> {
    let (kind, event) = if protocol == RpcProtocol::V1 {
        if let Some(event) = result.get("statusUpdate") {
            ("status-update", event)
        } else if let Some(event) = result.get("artifactUpdate") {
            ("artifact-update", event)
        } else {
            ("response", &result)
        }
    } else {
        (
            result
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            &result,
        )
    };
    Ok(match kind {
        "status-update" => {
            let status = task_status(&event["status"])?;
            vec![UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
                task_id: string_field(event, "taskId")?.to_owned(),
                context_id: string_field(event, "contextId")?.to_owned(),
                is_final: status.state.is_terminal()
                    || event.get("final").and_then(Value::as_bool) == Some(true),
                status,
                metadata: None,
            })]
        }
        "artifact-update" => vec![UpdateEvent::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: string_field(event, "taskId")?.to_owned(),
            context_id: string_field(event, "contextId")?.to_owned(),
            artifact: text_artifact(&event["artifact"])?,
            append: event
                .get("append")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
            last_chunk: event
                .get("lastChunk")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
            metadata: None,
        })],
        "response" | "task" | "message" => {
            let mut task = decode_task(protocol, result)?;
            if task.status.message.is_none() {
                task.status.message = task.history.pop();
            }
            let mut events = task
                .artifacts
                .into_iter()
                .map(|artifact| {
                    UpdateEvent::ArtifactUpdate(TaskArtifactUpdateEvent {
                        task_id: task.id.clone(),
                        context_id: task.context_id.clone(),
                        artifact,
                        append: false,
                        last_chunk: true,
                        metadata: None,
                    })
                })
                .collect::<Vec<_>>();
            events.push(UpdateEvent::StatusUpdate(TaskStatusUpdateEvent::new(
                task.id,
                task.context_id,
                task.status,
                None,
            )));
            events
        }
        _ => return Err(invalid_result()),
    })
}
