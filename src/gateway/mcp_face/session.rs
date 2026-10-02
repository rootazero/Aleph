//! One row per `Mcp-Session-Id` (Streamable HTTP, revisions ≤ 2025-11-25).
//!
//! A row maps the peer's session id to (a) the Aleph session key every tool
//! call from it runs under — `SessionKey::task("main", "mcp", <id>)`, so the
//! result store, standing grants and `ctx_search` scope to this MCP session
//! and nothing else — and (b) what the peer said it was in `clientInfo`.
//!
//! **A row is not a credential.** `authorize()` (auth.rs) runs on every
//! request; the row only tells the face which Aleph session to use. A leaked
//! id therefore buys nothing without the bearer that minted it.
//!
//! Idle rows are swept opportunistically on `create` (same shape as
//! `ExecApprovalManager::register_pending`'s `cleanup_expired`): no background
//! task, bounded work. A swept session answers 404 to its next request and the
//! SDKs re-`initialize` on 404.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::gateway::protocol::JsonRpcRequest;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Mutex;

/// How long a session may sit without a request (POST, GET stream open, or
/// DELETE) before it is swept. pi-mcp-adapter idle-disconnects at 10 min and
/// re-initializes; dsh's SDK keeps its session for the process lifetime and
/// re-initializes on 404 — 30 min covers both without keeping abandoned rows
/// for hours.
pub const MCP_SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// `SessionKey::task` type for MCP sessions. Not one of the reserved routing
/// markers (`session_key.rs:246-260`).
pub const MCP_SESSION_TASK_TYPE: &str = "mcp";

/// Notifications buffered per open SSE stream. Lossy on purpose: a client
/// that cannot drain `list_changed` events will re-sync on the next one.
pub const SSE_QUEUE_DEPTH: usize = 16;

/// What the peer declared in `initialize.clientInfo`. Display / log / audit
/// text only — never an authority (see module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClient {
    pub client_name: String,
    pub client_version: String,
}

/// A copy of the row's public facts, handed out so callers never hold the
/// table lock across an `.await`.
#[derive(Debug, Clone)]
pub struct SessionView {
    pub id: String,
    pub client: McpClient,
    pub protocol_version: &'static str,
    pub aleph_key: SessionKey,
    /// `notifications/initialized` has arrived.
    pub initialized: bool,
}

struct Row {
    view: SessionView,
    last_seen: Instant,
    notifier: Option<mpsc::Sender<JsonRpcRequest>>,
}

/// The session table. One per face.
pub struct SessionTable {
    rows: Mutex<HashMap<String, Row>>,
    ttl: Duration,
}

impl SessionTable {
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            rows: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Mint a session. Sweeps idle rows first.
    pub fn create(&self, client: McpClient, protocol_version: &'static str) -> SessionView {
        self.create_at(client, protocol_version, Instant::now())
    }

    pub(crate) fn create_at(
        &self,
        client: McpClient,
        protocol_version: &'static str,
        now: Instant,
    ) -> SessionView {
        let id = uuid::Uuid::new_v4().to_string();
        let view = SessionView {
            aleph_key: SessionKey::task("main", MCP_SESSION_TASK_TYPE, &id),
            id: id.clone(),
            client,
            protocol_version,
            initialized: false,
        };
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let ttl = self.ttl;
        rows.retain(|_, row| now.duration_since(row.last_seen) < ttl);
        rows.insert(
            id,
            Row {
                view: view.clone(),
                last_seen: now,
                notifier: None,
            },
        );
        view
    }

    /// The row, with its idle clock reset. `None` for unknown or expired ids
    /// (an expired row is removed here rather than left to the next sweep).
    pub fn touch(&self, id: &str) -> Option<SessionView> {
        self.touch_at(id, Instant::now())
    }

    pub(crate) fn touch_at(&self, id: &str, now: Instant) -> Option<SessionView> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let expired = rows
            .get(id)
            .is_some_and(|row| now.duration_since(row.last_seen) >= self.ttl);
        if expired {
            rows.remove(id);
            return None;
        }
        let row = rows.get_mut(id)?;
        row.last_seen = now;
        Some(row.view.clone())
    }

    /// Record `notifications/initialized`. `false` when the id is unknown.
    pub fn mark_initialized(&self, id: &str) -> bool {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        match rows.get_mut(id) {
            Some(row) => {
                row.view.initialized = true;
                true
            }
            None => false,
        }
    }

    /// `DELETE /mcp`. `false` when the id is unknown.
    pub fn remove(&self, id: &str) -> bool {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .is_some()
    }

    /// Open (or replace) the session's server→client stream. The receiver is
    /// what `GET /mcp` turns into SSE. Replacing drops the previous sender, so
    /// a client that reconnects its stream never leaves a dead one behind.
    pub fn attach_stream(&self, id: &str) -> Option<mpsc::Receiver<JsonRpcRequest>> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let row = rows.get_mut(id)?;
        let (tx, rx) = mpsc::channel(SSE_QUEUE_DEPTH);
        row.notifier = Some(tx);
        row.last_seen = Instant::now();
        Some(rx)
    }

    /// Push one notification to every open stream. Returns how many streams
    /// took it; senders whose receiver is gone are dropped as a side effect.
    pub fn broadcast(&self, notification: &JsonRpcRequest) -> usize {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let mut delivered = 0;
        for row in rows.values_mut() {
            let Some(tx) = row.notifier.as_ref() else {
                continue;
            };
            match tx.try_send(notification.clone()) {
                Ok(()) => delivered += 1,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // Lossy by design (see `SSE_QUEUE_DEPTH`).
                    tracing::debug!(session = %row.view.id, "MCP notification dropped: stream backlog full");
                }
                Err(mpsc::error::TrySendError::Closed(_)) => row.notifier = None,
            }
        }
        delivered
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn client() -> McpClient {
        McpClient {
            client_name: "dsh-mcp-client".to_string(),
            client_version: "0.0.1".to_string(),
        }
    }

    #[test]
    fn create_mints_a_uuid_and_a_task_session_key() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let view = table.create(client(), "2025-03-26");
        assert!(
            uuid::Uuid::parse_str(&view.id).is_ok(),
            "session id must be a uuid: {}",
            view.id
        );
        assert_eq!(view.protocol_version, "2025-03-26");
        assert!(!view.initialized);
        assert_eq!(
            view.aleph_key,
            SessionKey::task("main", MCP_SESSION_TASK_TYPE, &view.id)
        );
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn touch_returns_the_row_and_unknown_ids_are_none() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let view = table.create(client(), "2025-06-18");
        assert_eq!(table.touch(&view.id).map(|v| v.id), Some(view.id.clone()));
        assert!(table.touch("not-a-session").is_none());
    }

    #[test]
    fn an_idle_session_expires_and_a_touched_one_does_not() {
        let ttl = Duration::from_secs(60);
        let table = SessionTable::new(ttl);
        let t0 = Instant::now();
        let idle = table.create_at(client(), "2025-03-26", t0);
        let busy = table.create_at(client(), "2025-03-26", t0);
        // `busy` is used at t0+50s; `idle` never again.
        assert!(table
            .touch_at(&busy.id, t0 + Duration::from_secs(50))
            .is_some());
        // At t0+70s `idle` is past its TTL (last seen t0) and `busy` is not
        // (last seen t0+50s).
        assert!(table
            .touch_at(&idle.id, t0 + Duration::from_secs(70))
            .is_none());
        assert!(table
            .touch_at(&busy.id, t0 + Duration::from_secs(70))
            .is_some());
        assert_eq!(
            table.len(),
            1,
            "the expired row must be removed, not just hidden"
        );
    }

    #[test]
    fn mark_initialized_and_remove_report_whether_the_row_existed() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let view = table.create(client(), "2025-11-25");
        assert!(table.mark_initialized(&view.id));
        assert!(table.touch(&view.id).is_some_and(|v| v.initialized));
        assert!(!table.mark_initialized("nope"));
        assert!(table.remove(&view.id));
        assert!(!table.remove(&view.id));
        assert_eq!(table.len(), 0);
    }

    #[tokio::test]
    async fn broadcast_reaches_only_sessions_with_an_open_stream() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let with_stream = table.create(client(), "2025-03-26");
        let _without_stream = table.create(client(), "2025-03-26");
        let mut rx = table.attach_stream(&with_stream.id).expect("known session");
        assert!(table.attach_stream("nope").is_none());

        let note = JsonRpcRequest::notification("notifications/tools/list_changed", None);
        assert_eq!(table.broadcast(&note), 1, "exactly one stream is open");
        let got = rx.recv().await.expect("the open stream receives it");
        assert_eq!(got.method, "notifications/tools/list_changed");
        assert!(got.id.is_none(), "a notification carries no id");
    }

    #[tokio::test]
    async fn a_closed_stream_is_dropped_on_the_next_broadcast() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let s = table.create(client(), "2025-03-26");
        let rx = table.attach_stream(&s.id).expect("known session");
        drop(rx); // client went away
        let note = JsonRpcRequest::notification("notifications/tools/list_changed", None);
        assert_eq!(table.broadcast(&note), 0);
        // The row survives (the client may POST again); only the stream is gone.
        assert!(table.touch(&s.id).is_some());
    }

    #[tokio::test]
    async fn reattaching_replaces_the_previous_stream() {
        let table = SessionTable::new(MCP_SESSION_IDLE_TTL);
        let s = table.create(client(), "2025-03-26");
        let mut old = table.attach_stream(&s.id).unwrap();
        let mut new = table.attach_stream(&s.id).unwrap();
        let note = JsonRpcRequest::notification("notifications/tools/list_changed", None);
        assert_eq!(table.broadcast(&note), 1);
        assert!(new.recv().await.is_some());
        assert!(
            old.recv().await.is_none(),
            "the replaced sender was dropped"
        );
    }
}
