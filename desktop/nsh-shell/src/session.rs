// Session state management for NSH Desktop.
//
// The shell owns a projection of canonical Gateway state. It never invents
// session ids or assistant text; the Gateway remains the source of truth for
// history, accepted runs and stream events.

use aleph_client::{AlephClient, CliConfig, TopicEvent, DEFAULT_GATEWAY_URL};
use aleph_protocol::events::StreamEvent;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Role of a message participant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// A single message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub message_id: String,
    pub role: Role,
    pub content: String,
    pub timestamp: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

/// Tool call within a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub tool_name: String,
    pub arguments: String,
    pub result: Option<String>,
}

/// A canonical session artifact row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    pub origin: String,
    #[serde(default)]
    pub run_id: Option<String>,
    pub created_at: i64,
    pub url: String,
}

/// Events emitted by the canonical session projection.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    SessionReady { session_id: String },
    Queued { run_id: String, ahead: u16 },
    RunStarted { run_id: String },
    HistoryLoaded(Vec<Message>),
    ArtifactsLoaded(Vec<Artifact>),
    MessageAdded(Message),
    MessageDelta { delta: String },
    MessageComplete,
    GenerationStarted,
    GenerationComplete,
    Error(String),
}

/// Session manager state.
pub struct SessionManager {
    client: Option<Arc<AlephClient>>,
    events_rx: std::sync::Mutex<Option<mpsc::Receiver<StreamEvent>>>,
    topic_rx: std::sync::Mutex<Option<mpsc::Receiver<TopicEvent>>>,
    session_id: Arc<RwLock<Option<String>>>,
    messages: Arc<RwLock<Vec<Message>>>,
    artifacts: Arc<RwLock<Vec<Artifact>>>,
    is_generating: Arc<RwLock<bool>>,
    event_tx: Option<mpsc::Sender<SessionEvent>>,
}

impl SessionManager {
    /// Create a disconnected manager for shell state and unit tests.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: None,
            events_rx: std::sync::Mutex::new(None),
            topic_rx: std::sync::Mutex::new(None),
            session_id: Arc::new(RwLock::new(None)),
            messages: Arc::new(RwLock::new(Vec::new())),
            artifacts: Arc::new(RwLock::new(Vec::new())),
            is_generating: Arc::new(RwLock::new(false)),
            event_tx: None,
        }
    }

    /// Connect to the canonical Gateway and subscribe to its stream plane.
    pub async fn connect() -> Result<Self, String> {
        let (client, events_rx) = AlephClient::connect(DEFAULT_GATEWAY_URL, &CliConfig::default())
            .await
            .map_err(|error| format!("session connection failed: {error}"))?;
        let client = Arc::new(client);
        client
            .call::<_, Value>("events.subscribe", Some(json!({ "topics": ["**"] })))
            .await
            .map_err(|error| format!("session event subscription failed: {error}"))?;
        let topic_rx = client.take_topic_events();
        Ok(Self {
            client: Some(client),
            events_rx: std::sync::Mutex::new(Some(events_rx)),
            topic_rx: std::sync::Mutex::new(topic_rx),
            ..Self::new()
        })
    }

    /// Attach to an existing canonical session, if one is available.
    pub async fn create_session(&self) -> Result<String, String> {
        let client = self.client.as_ref().ok_or("Gateway is not connected")?;
        let result: Value = client
            .call("sessions.list", None::<Value>)
            .await
            .map_err(|error| format!("sessions.list failed: {error}"))?;
        let rows = result
            .get("sessions")
            .cloned()
            .unwrap_or_else(|| result.clone());
        let session_id = rows
            .as_array()
            .and_then(|sessions| {
                sessions.iter().find_map(|session| {
                    session
                        .get("key")
                        .and_then(Value::as_str)
                        .filter(|key| !key.is_empty())
                })
            })
            .ok_or("No canonical session exists yet; send a message to create one")?
            .to_owned();
        *self.session_id.write().await = Some(session_id.clone());
        self.load_messages().await?;
        Ok(session_id)
    }

    /// Get the current canonical session key.
    pub async fn session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Load message history for the current session.
    pub async fn load_messages(&self) -> Result<Vec<Message>, String> {
        let client = self.client.as_ref().ok_or("Gateway is not connected")?;
        let session_id = self
            .session_id
            .read()
            .await
            .clone()
            .ok_or("No active session")?;
        let result: Value = client
            .call(
                "chat.history",
                Some(json!({ "session_key": session_id, "limit": 100 })),
            )
            .await
            .map_err(|error| format!("chat.history failed: {error}"))?;
        let messages: Vec<Message> = serde_json::from_value(
            result
                .get("messages")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        )
        .map_err(|error| format!("invalid chat.history messages: {error}"))?;
        *self.messages.write().await = messages.clone();
        Ok(messages)
    }

    /// Send a message to the canonical session, creating one server-side when
    /// the request has no session key.
    pub async fn send_message(&self, content: String) -> Result<(), String> {
        if content.trim().is_empty() {
            return Ok(());
        }
        let client = self.client.as_ref().ok_or("Gateway is not connected")?;
        let session_id = self.session_id.read().await.clone();
        let result: Value = client
            .call(
                "chat.send",
                Some(json!({
                    "message": content,
                    "session_key": session_id,
                    "channel": "gui:chat",
                    "stream": true,
                    "attachments": [],
                    "agent_id": Value::Null,
                    "project_root": Value::Null,
                    "project_id": Value::Null,
                    "model_override": Value::Null,
                    "exec_tier": Value::Null,
                    "mode": Value::Null,
                    "thinking": Value::Null,
                    "memory": Value::Null,
                    "voice_input": false,
                })),
            )
            .await
            .map_err(|error| format!("chat.send failed: {error}"))?;
        let canonical_session = result
            .get("session_key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
            .ok_or("chat.send returned no canonical session_key")?
            .to_owned();
        *self.session_id.write().await = Some(canonical_session.clone());
        if let Ok(messages) = self.load_messages().await {
            if let Some(tx) = &self.event_tx {
                let _ = tx
                    .send(SessionEvent::SessionReady {
                        session_id: canonical_session,
                    })
                    .await;
                let _ = tx.send(SessionEvent::HistoryLoaded(messages)).await;
            }
        }
        Ok(())
    }

    /// Subscribe to canonical stream events and project them into shell state.
    pub fn subscribe_events(&mut self) -> mpsc::Receiver<SessionEvent> {
        let (tx, rx) = mpsc::channel(128);
        self.event_tx = Some(tx.clone());
        let Some(mut events_rx) = self
            .events_rx
            .lock()
            .expect("events receiver mutex poisoned")
            .take()
        else {
            return rx;
        };
        let Some(client) = self.client.clone() else {
            return rx;
        };
        let session_id = self.session_id.clone();
        let messages = self.messages.clone();
        let artifacts = self.artifacts.clone();
        let is_generating = self.is_generating.clone();
        let mut topic_rx = self
            .topic_rx
            .lock()
            .expect("topic receiver mutex poisoned")
            .take();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    event = events_rx.recv() => {
                        let Some(event) = event else {
                            break;
                        };
                        match event {
                    StreamEvent::RunAccepted {
                        run_id,
                        session_key,
                        ..
                    } => {
                        *session_id.write().await = Some(session_key.clone());
                        let _ = tx
                            .send(SessionEvent::SessionReady {
                                session_id: session_key.clone(),
                            })
                            .await;
                        let _ = tx.send(SessionEvent::RunStarted { run_id }).await;
                        if let Ok(items) = load_artifacts(&client, &session_key).await {
                            *artifacts.write().await = items.clone();
                            let _ = tx.send(SessionEvent::ArtifactsLoaded(items)).await;
                        }
                        *is_generating.write().await = true;
                        let _ = tx.send(SessionEvent::GenerationStarted).await;
                    }
                    StreamEvent::RunQueued { run_id, ahead, .. } => {
                        let _ = tx.send(SessionEvent::Queued { run_id, ahead }).await;
                    }
                    StreamEvent::ResponseChunk { content, .. } => {
                        let _ = tx.send(SessionEvent::MessageDelta { delta: content }).await;
                    }
                    StreamEvent::SessionUserMessage {
                        session_key,
                        content,
                        timestamp,
                        seq,
                        ..
                    } => {
                        if session_id.read().await.as_deref() != Some(session_key.as_str()) {
                            continue;
                        }
                        let message = Message {
                            message_id: format!("session-user-{seq}"),
                            role: Role::User,
                            content,
                            timestamp,
                            tool_calls: Vec::new(),
                        };
                        messages.write().await.push(message.clone());
                        let _ = tx.send(SessionEvent::MessageAdded(message)).await;
                    }
                    StreamEvent::RunComplete { .. } => {
                        *is_generating.write().await = false;
                        if let Some(current_session) = session_id.read().await.clone() {
                            if let Ok(history) = load_history(&client, &current_session).await {
                                *messages.write().await = history.clone();
                                let _ = tx.send(SessionEvent::HistoryLoaded(history)).await;
                            }
                            if let Ok(items) = load_artifacts(&client, &current_session).await {
                                *artifacts.write().await = items.clone();
                                let _ = tx.send(SessionEvent::ArtifactsLoaded(items)).await;
                            }
                        }
                        let _ = tx.send(SessionEvent::MessageComplete).await;
                        let _ = tx.send(SessionEvent::GenerationComplete).await;
                    }
                    StreamEvent::RunError { error, .. } => {
                        *is_generating.write().await = false;
                        let _ = tx.send(SessionEvent::Error(error)).await;
                        let _ = tx.send(SessionEvent::GenerationComplete).await;
                    }
                    _ => {}
                }
                    }
                    topic = recv_topic(&mut topic_rx) => {
                        let Some(topic) = topic else {
                            break;
                        };
                        if topic.topic != aleph_protocol::artifact::TOPIC {
                            continue;
                        }
                        let Some(topic_session) = topic
                            .data
                            .get(aleph_protocol::artifact::TOPIC_SESSION_KEY)
                            .and_then(Value::as_str)
                        else {
                            continue;
                        };
                        if session_id.read().await.as_deref() != Some(topic_session) {
                            continue;
                        }
                        if let Ok(items) = load_artifacts(&client, topic_session).await {
                            *artifacts.write().await = items.clone();
                            let _ = tx.send(SessionEvent::ArtifactsLoaded(items)).await;
                        }
                    }
                    else => break,
                }
            }
            *is_generating.write().await = false;
        });
        rx
    }
}

async fn recv_topic(receiver: &mut Option<mpsc::Receiver<TopicEvent>>) -> Option<TopicEvent> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn load_history(client: &AlephClient, session_id: &str) -> Result<Vec<Message>, String> {
    let result: Value = client
        .call(
            "chat.history",
            Some(json!({ "session_key": session_id, "limit": 100 })),
        )
        .await
        .map_err(|error| format!("chat.history failed: {error}"))?;
    serde_json::from_value(
        result
            .get("messages")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .map_err(|error| format!("invalid chat.history messages: {error}"))
}

async fn load_artifacts(client: &AlephClient, session_id: &str) -> Result<Vec<Artifact>, String> {
    let result: Value = client
        .call("artifacts.list", Some(json!({ "session_key": session_id })))
        .await
        .map_err(|error| format!("artifacts.list failed: {error}"))?;
    serde_json::from_value(
        result
            .get("items")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .map_err(|error| format!("invalid artifacts.list items: {error}"))
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disconnected_manager_has_no_local_session() {
        let manager = SessionManager::new();
        assert_eq!(manager.session_id().await, None);
    }

    #[tokio::test]
    async fn send_message_requires_a_real_gateway_connection() {
        let manager = SessionManager::new();
        assert_eq!(
            manager.send_message("test".to_string()).await,
            Err("Gateway is not connected".into())
        );
    }

    #[tokio::test]
    async fn create_session_does_not_invent_a_local_id() {
        let manager = SessionManager::new();
        assert_eq!(
            manager.create_session().await,
            Err("Gateway is not connected".into())
        );
    }
}
