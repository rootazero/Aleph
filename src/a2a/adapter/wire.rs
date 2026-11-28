//! JSON-RPC payloads at the 0.3 and 1.0 A2A boundaries.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::a2a::domain::message::FileContent;
use crate::a2a::domain::{
    A2AError, A2AMessage, A2ARole, A2ATask, AgentCard, Part, TaskState, TaskStatus, UpdateEvent,
};
use crate::a2a::port::A2AResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WireVersion {
    V03,
    V10,
}

impl WireVersion {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "0.3" => Some(Self::V03),
            "1.0" => Some(Self::V10),
            _ => None,
        }
    }

    pub(crate) fn header(self) -> &'static str {
        match self {
            Self::V03 => "0.3",
            Self::V10 => "1.0",
        }
    }

    pub(crate) fn methods(self) -> [&'static str; 5] {
        match self {
            Self::V03 => [
                "message/send",
                "message/stream",
                "tasks/get",
                "tasks/cancel",
                "tasks/resubscribe",
            ],
            Self::V10 => [
                "SendMessage",
                "SendStreamingMessage",
                "GetTask",
                "CancelTask",
                "SubscribeToTask",
            ],
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendParams {
    message: Message,
    #[serde(default)]
    pub(crate) configuration: SendConfiguration,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendConfiguration {
    blocking: Option<bool>,
    return_immediately: Option<bool>,
    #[serde(default)]
    accepted_output_modes: Vec<String>,
    push_notification_config: Option<Value>,
    pub(crate) history_length: Option<usize>,
}

impl SendConfiguration {
    pub(crate) fn immediate(&self, version: WireVersion) -> bool {
        match version {
            WireVersion::V03 => self.blocking == Some(false),
            WireVersion::V10 => self.return_immediately == Some(true),
        }
    }

    pub(crate) fn validate(&self) -> A2AResult<()> {
        if self.push_notification_config.is_some() {
            return Err(A2AError::PushNotSupported);
        }
        if !self.accepted_output_modes.is_empty()
            && !self
                .accepted_output_modes
                .iter()
                .any(|mode| mode == "text/plain" || mode == "text")
        {
            return Err(A2AError::UnsupportedContentType);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message {
    message_id: String,
    kind: Option<String>,
    role: String,
    parts: Vec<Value>,
    task_id: Option<String>,
    context_id: Option<String>,
    #[serde(default)]
    reference_task_ids: Vec<String>,
    metadata: Option<Map<String, Value>>,
}

pub(crate) struct IncomingMessage {
    pub(crate) message: A2AMessage,
    pub(crate) task_id: Option<String>,
    pub(crate) context_id: Option<String>,
    pub(crate) reference_task_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Part03 {
    Text {
        text: String,
        metadata: Option<Map<String, Value>>,
    },
    File {
        file: FileContent,
    },
    Data {
        #[serde(rename = "data")]
        _data: Map<String, Value>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Part10 {
    text: Option<String>,
    raw: Option<String>,
    url: Option<String>,
    data: Option<Value>,
    metadata: Option<Map<String, Value>>,
    filename: Option<String>,
    media_type: Option<String>,
}

fn decode_part(version: WireVersion, value: Value) -> A2AResult<Part> {
    match version {
        WireVersion::V03 => match from_value::<Part03>(value)? {
            Part03::Text { text, metadata } => Ok(Part::Text { text, metadata }),
            Part03::File { file } => {
                file.validate()
                    .map_err(|error| A2AError::InvalidParams(error.into()))?;
                Err(A2AError::UnsupportedContentType)
            }
            Part03::Data { .. } => Err(A2AError::UnsupportedContentType),
        },
        WireVersion::V10 => {
            let part: Part10 = from_value(value)?;
            match (part.text, part.raw, part.url, part.data) {
                (Some(text), None, None, None) => Ok(Part::Text {
                    text,
                    metadata: part.metadata,
                }),
                (None, bytes @ Some(_), uri @ None, None)
                | (None, bytes @ None, uri @ Some(_), None) => {
                    FileContent {
                        name: part.filename,
                        mime_type: part.media_type,
                        bytes,
                        uri,
                    }
                    .validate()
                    .map_err(|error| A2AError::InvalidParams(error.into()))?;
                    Err(A2AError::UnsupportedContentType)
                }
                (None, None, None, Some(_)) => Err(A2AError::UnsupportedContentType),
                _ => Err(A2AError::InvalidParams(
                    "A part must set exactly one content field".into(),
                )),
            }
        }
    }
}

pub(crate) fn from_value<T: serde::de::DeserializeOwned>(value: Value) -> A2AResult<T> {
    serde_json::from_value(value).map_err(|error| A2AError::InvalidParams(error.to_string()))
}

impl SendParams {
    pub(crate) fn into_message(self, version: WireVersion) -> A2AResult<IncomingMessage> {
        let message = self.message;
        let valid_role = match version {
            WireVersion::V03 => {
                message.role == "user" && message.kind.as_deref() == Some("message")
            }
            WireVersion::V10 => message.role == "ROLE_USER",
        };
        if !valid_role || message.message_id.is_empty() {
            return Err(A2AError::InvalidParams(
                "Expected a user message with a messageId".into(),
            ));
        }
        let parts = message
            .parts
            .into_iter()
            .map(|part| decode_part(version, part))
            .collect::<A2AResult<Vec<_>>>()?;
        let input = A2AMessage {
            message_id: message.message_id,
            role: A2ARole::User,
            parts,
            session_id: None,
            timestamp: None,
            metadata: message.metadata,
        };
        if input.text_content().trim().is_empty() {
            return Err(A2AError::InvalidParams(
                "Message contains no text content".into(),
            ));
        }
        Ok(IncomingMessage {
            message: input,
            task_id: message.task_id.filter(|id| !id.is_empty()),
            context_id: message.context_id.filter(|id| !id.is_empty()),
            reference_task_ids: message.reference_task_ids,
        })
    }
}

fn part(version: WireVersion, part: &Part) -> Value {
    let mut value = serde_json::to_value(part).expect("part is serializable");
    let obj = value.as_object_mut().expect("part is an object");
    let kind = obj.remove("type").expect("part has a type");
    match version {
        WireVersion::V03 => {
            obj.insert("kind".into(), kind);
        }
        WireVersion::V10 => {
            if let Some(Value::Object(mut file)) = obj.remove("file") {
                for (from, to) in [
                    ("bytes", "raw"),
                    ("uri", "url"),
                    ("name", "filename"),
                    ("mimeType", "mediaType"),
                ] {
                    if let Some(value) = file.remove(from) {
                        obj.insert(to.into(), value);
                    }
                }
            }
        }
    }
    value
}

fn message(version: WireVersion, message: &A2AMessage) -> Value {
    let parts: Vec<_> = message
        .parts
        .iter()
        .map(|item| part(version, item))
        .collect();
    let mut result = json!({"messageId": message.message_id, "parts": parts});
    result["role"] = match (version, message.role) {
        (WireVersion::V03, A2ARole::User) => json!("user"),
        (WireVersion::V03, A2ARole::Agent) => json!("agent"),
        (WireVersion::V10, A2ARole::User) => json!("ROLE_USER"),
        (WireVersion::V10, A2ARole::Agent) => json!("ROLE_AGENT"),
    };
    if version == WireVersion::V03 {
        result["kind"] = json!("message");
    }
    if let Some(metadata) = &message.metadata {
        result["metadata"] = json!(metadata);
    }
    result
}

pub(crate) fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Submitted => "TASK_STATE_SUBMITTED",
        TaskState::Working => "TASK_STATE_WORKING",
        TaskState::Completed => "TASK_STATE_COMPLETED",
        TaskState::Failed => "TASK_STATE_FAILED",
        TaskState::Canceled => "TASK_STATE_CANCELED",
        TaskState::InputRequired => "TASK_STATE_INPUT_REQUIRED",
        TaskState::Rejected => "TASK_STATE_REJECTED",
        TaskState::AuthRequired => "TASK_STATE_AUTH_REQUIRED",
    }
}

fn status(version: WireVersion, status: &TaskStatus) -> Value {
    let state = match version {
        WireVersion::V03 => json!(status.state),
        WireVersion::V10 => json!(state_name(status.state)),
    };
    let mut result = json!({"state": state, "timestamp": status.timestamp});
    if let Some(item) = &status.message {
        result["message"] = message(version, item);
    }
    result
}

fn artifact(version: WireVersion, artifact: &crate::a2a::domain::Artifact) -> Value {
    let parts: Vec<_> = artifact
        .parts
        .iter()
        .map(|item| part(version, item))
        .collect();
    let mut result = json!({"artifactId": artifact.artifact_id, "parts": parts});
    if let Some(metadata) = &artifact.metadata {
        result["metadata"] = json!(metadata);
    }
    result
}

pub(crate) fn task(version: WireVersion, task: &A2ATask) -> Value {
    let artifacts: Vec<_> = task
        .artifacts
        .iter()
        .map(|item| artifact(version, item))
        .collect();
    let history: Vec<_> = task
        .history
        .iter()
        .map(|item| message(version, item))
        .collect();
    let mut result = json!({"id": task.id, "contextId": task.context_id, "status": status(version, &task.status), "artifacts": artifacts, "history": history});
    if version == WireVersion::V03 {
        result["kind"] = json!("task");
    }
    result
}

pub(crate) fn task_result(version: WireVersion, snapshot: &A2ATask) -> Value {
    let value = task(version, snapshot);
    if version == WireVersion::V10 {
        json!({"task": value})
    } else {
        value
    }
}

pub(crate) fn update(version: WireVersion, event: &UpdateEvent) -> Value {
    match event {
        UpdateEvent::StatusUpdate(event) => {
            let mut value = json!({"taskId": event.task_id, "contextId": event.context_id, "status": status(version, &event.status)});
            if version == WireVersion::V10 {
                return json!({"statusUpdate": value});
            }
            value["kind"] = json!("status-update");
            value["final"] = json!(event.is_final);
            value
        }
        UpdateEvent::ArtifactUpdate(event) => {
            let mut value = json!({"taskId": event.task_id, "contextId": event.context_id, "artifact": artifact(version, &event.artifact), "append": event.append, "lastChunk": event.last_chunk});
            if version == WireVersion::V10 {
                return json!({"artifactUpdate": value});
            }
            value["kind"] = json!("artifact-update");
            value
        }
    }
}

pub(crate) fn agent_card(card: &AgentCard, endpoint: &str) -> Value {
    let skills: Vec<_> = card.skills.iter().map(|skill| json!({
        "id": skill.id, "name": skill.name, "description": skill.description.as_deref().unwrap_or_default(),
        "tags": [], "inputModes": ["text/plain"], "outputModes": ["text/plain"],
    })).collect();
    let mut result = json!({
        "name": card.name, "description": card.description.as_deref().unwrap_or_default(),
        "version": card.version, "protocolVersion": "0.3.0", "url": endpoint,
        "preferredTransport": "JSONRPC", "capabilities": {"streaming": true},
        "defaultInputModes": ["text/plain"], "defaultOutputModes": ["text/plain"], "skills": skills,
        "supportedInterfaces": [
            {"url": endpoint, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"},
            {"url": endpoint, "protocolBinding": "JSONRPC", "protocolVersion": "0.3"},
        ],
    });
    if !card.security.is_empty() {
        result["securitySchemes"] = json!({"bearer": {
            "type": "http", "scheme": "bearer", "httpAuthSecurityScheme": {"scheme": "bearer"},
        }});
        result["security"] = json!([{"bearer": []}]);
        result["securityRequirements"] = json!([{"schemes": {"bearer": {"list": []}}}]);
    }
    result
}
