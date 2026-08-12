//! Process-global registry of live PTY sessions.
//!
//! Modelled on `sandbox::exec_approval::grants`: the registry is a
//! `LazyLock` singleton so the JSON-RPC handlers stay stateless and reach it
//! through a free function (`pty::manager()`), exactly like the user-hooks
//! admin handlers reach the extension manager. The gateway attaches its
//! [`GatewayEventBus`] once at serve time via [`PtyManager::attach_event_bus`]
//! so reader threads can stream output without per-connection wiring.
//!
//! Sessions are bounded (FIFO eviction at [`MAX_SESSIONS`]) so a runaway client
//! cannot exhaust descriptors/threads — the Rust analogue of hermes' implicit
//! per-process `_sessions` dict, but with an explicit ceiling.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};

use serde::Serialize;

use super::session::{PtySession, SpawnOptions};
use crate::gateway::event_bus::GatewayEventBus;
use crate::sync_primitives::Arc;

/// Maximum concurrent PTY sessions. Beyond this the oldest is killed FIFO.
const MAX_SESSIONS: usize = 64;

/// Public summary of a session for `pty.list`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub shell: String,
    pub created_at: i64,
    pub closed: bool,
}

/// Result of a successful `pty.spawn`.
#[derive(Debug, Clone, Serialize)]
pub struct SpawnResult {
    pub session_id: String,
    pub shell: String,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<String, Arc<PtySession>>,
    /// Insertion order for FIFO eviction.
    order: VecDeque<String>,
}

/// The global PTY session registry.
pub struct PtyManager {
    inner: Mutex<Inner>,
    bus: Mutex<Option<Arc<GatewayEventBus>>>,
}

static GLOBAL: LazyLock<PtyManager> = LazyLock::new(PtyManager::new);

/// Access the process-global PTY manager.
#[must_use]
pub fn manager() -> &'static PtyManager {
    &GLOBAL
}

/// Attach the gateway event bus so session output is broadcast on `pty.output`.
/// Called once from `GatewayServer::build_router`. Idempotent.
pub fn attach_event_bus(bus: Arc<GatewayEventBus>) {
    manager().attach_event_bus(bus);
}

impl PtyManager {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            bus: Mutex::new(None),
        }
    }

    fn attach_event_bus(&self, bus: Arc<GatewayEventBus>) {
        *self.bus.lock().unwrap_or_else(|e| e.into_inner()) = Some(bus);
    }

    fn current_bus(&self) -> Option<Arc<GatewayEventBus>> {
        self.bus.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Spawn a new session, evicting the oldest if at capacity.
    pub fn spawn(&self, opts: &SpawnOptions) -> Result<SpawnResult, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let bus = self.current_bus();
        let session = PtySession::spawn(id.clone(), opts, bus)?;
        let result = SpawnResult {
            session_id: session.id.clone(),
            shell: session.shell.clone(),
        };

        // Evict-then-insert under the lock so the map never exceeds the cap.
        let evicted = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let mut evicted: Option<Arc<PtySession>> = None;
            if inner.sessions.len() >= MAX_SESSIONS {
                if let Some(old_id) = inner.order.pop_front() {
                    evicted = inner.sessions.remove(&old_id);
                }
            }
            inner.order.push_back(id.clone());
            inner.sessions.insert(id, session);
            evicted
        };
        if let Some(old) = evicted {
            old.kill();
        }
        Ok(result)
    }

    /// Write bytes to a session's stdin.
    pub fn write(&self, session_id: &str, data: &[u8]) -> Result<(), String> {
        self.with_session(session_id, |s| s.write_input(data))
    }

    /// Resize a session's terminal.
    pub fn resize(&self, session_id: &str, rows: u16, cols: u16) -> Result<(), String> {
        self.with_session(session_id, |s| s.resize(rows, cols))
    }

    /// Terminate and remove a session.
    pub fn close(&self, session_id: &str) -> Result<(), String> {
        let session = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.order.retain(|i| i != session_id);
            inner.sessions.remove(session_id)
        };
        match session {
            Some(s) => {
                s.kill();
                Ok(())
            }
            None => Err(format!("no such session: {session_id}")),
        }
    }

    /// Remove a session from the registry without killing it (called by the
    /// reader thread after the child has already exited).
    pub fn remove(&self, session_id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.order.retain(|i| i != session_id);
        inner.sessions.remove(session_id);
    }

    /// Snapshot of all active sessions.
    pub fn list(&self) -> Vec<SessionInfo> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .order
            .iter()
            .filter_map(|id| inner.sessions.get(id))
            .map(|s| SessionInfo {
                session_id: s.id.clone(),
                shell: s.shell.clone(),
                created_at: s.created_at,
                closed: s.is_closed(),
            })
            .collect()
    }

    fn with_session<F, R>(&self, session_id: &str, f: F) -> Result<R, String>
    where
        F: FnOnce(&Arc<PtySession>) -> Result<R, String>,
    {
        let session = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.sessions.get(session_id).cloned()
        };
        match session {
            Some(s) => f(&s),
            None => Err(format!("no such session: {session_id}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_write_list_close_roundtrip() {
        let mgr = PtyManager::new();
        // No bus attached → output is silently dropped, spawn still works.
        let res = mgr
            .spawn(&SpawnOptions {
                command: Some(if cfg!(windows) { "cmd.exe" } else { "cat" }.to_string()),
                ..Default::default()
            })
            .expect("spawn should succeed");
        assert!(!res.session_id.is_empty());

        let list = mgr.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].session_id, res.session_id);

        // Writing to a live session succeeds.
        mgr.write(&res.session_id, b"hello\n").expect("write ok");
        mgr.resize(&res.session_id, 40, 120).expect("resize ok");

        // Closing removes it from the registry.
        mgr.close(&res.session_id).expect("close ok");
        assert!(mgr.list().is_empty());

        // Operations on an unknown session are errors, not panics.
        assert!(mgr.write("nope", b"x").is_err());
        assert!(mgr.close("nope").is_err());
    }

    #[test]
    fn unknown_session_is_error() {
        let mgr = PtyManager::new();
        assert!(mgr.resize("ghost", 24, 80).is_err());
        assert!(mgr.list().is_empty());
    }
}
