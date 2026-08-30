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
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::sync_primitives::{Arc, AtomicUsize, Ordering};

/// Concurrent PTY session ceiling before any `[policies.terminal]
/// max_sessions` config has been read. Not a hard cap — deliberately not a
/// `const` used directly by `spawn`: `handle_spawn` reads the live config
/// fresh on every `pty.spawn` and stores the configured ceiling via
/// `PtyManager::set_max_sessions`, so a patched value takes effect on the
/// very next spawn instead of needing a restart. This is only the value a
/// freshly constructed `PtyManager` starts with.
const DEFAULT_MAX_SESSIONS: usize = 64;

/// Publish cadence. 16 ms ≈ 60 Hz: fast enough that no human sees the delay,
/// slow enough that a process writing megabytes per second still costs one
/// bounded frame per tick. This coalescing *is* the backpressure design.
const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// Public summary of a session for `pty.list`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub shell: String,
    pub created_at: i64,
    pub closed: bool,
    /// Number of connections currently holding a viewport constraint on this
    /// session — the diagnostic surface for the smallest-wins sizing table
    /// (`PtyManager::note_viewport`/`release_conn`). Not the same thing as
    /// "how many clients are attached": a client that has only ever called
    /// `pty.attach`/`pty.input` without resizing never appears here.
    pub attached_count: usize,
    /// Who asked for this session — see [`SpawnOptions::created_by`]. `None`
    /// for a spawn that did not come through a caller-identified face.
    pub created_by: Option<String>,
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
    /// `session_id -> (conn_id -> viewport)`. Present because a server-held
    /// screen makes multi-client sharing free, and the moment a second client
    /// attaches, something has to decide the one size the PTY gets.
    viewports: HashMap<String, HashMap<String, (u16, u16)>>,
}

/// The global PTY session registry.
pub struct PtyManager {
    inner: Mutex<Inner>,
    bus: Mutex<Option<Arc<GatewayEventBus>>>,
    /// The current concurrent session ceiling — see [`DEFAULT_MAX_SESSIONS`]
    /// and [`Self::set_max_sessions`].
    max_sessions: AtomicUsize,
}

static GLOBAL: LazyLock<PtyManager> = LazyLock::new(PtyManager::new);

/// Access the process-global PTY manager.
#[must_use]
pub fn manager() -> &'static PtyManager {
    &GLOBAL
}

/// Attach the gateway event bus so session output is broadcast on `pty.screen`,
/// and start the flush loop that publishes it. Called once from
/// `GatewayServer::build_router`. Idempotent.
pub fn attach_event_bus(bus: Arc<GatewayEventBus>) {
    manager().attach_event_bus(bus);
    manager().start_flush_loop();
}

impl PtyManager {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            bus: Mutex::new(None),
            max_sessions: AtomicUsize::new(DEFAULT_MAX_SESSIONS),
        }
    }

    /// Set the concurrent session ceiling. Called by `handle_spawn` on every
    /// `pty.spawn` with `[policies.terminal] max_sessions` read fresh from
    /// the live config — deliberately not cached at boot, so a live patch
    /// takes effect on the very next spawn rather than needing a restart.
    /// Clamped to at least 1: a ceiling of 0 would make every spawn evict
    /// the session it just inserted.
    pub fn set_max_sessions(&self, max_sessions: usize) {
        self.max_sessions
            .store(max_sessions.max(1), Ordering::SeqCst);
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
        let cap = self.max_sessions.load(Ordering::SeqCst);
        let evicted = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let mut evicted: Option<Arc<PtySession>> = None;
            if inner.sessions.len() >= cap {
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

    /// The scrollback ceiling currently in effect for a live session — for
    /// diagnostics/tests, so a configured value can be read back rather than
    /// merely trusted to have been passed through.
    #[must_use]
    pub fn scrollback_limit_of(&self, session_id: &str) -> Option<usize> {
        self.with_session(session_id, |s| Ok(s.scrollback_limit()))
            .ok()
    }

    /// Write bytes to a session's stdin.
    pub fn write(&self, session_id: &str, data: &[u8]) -> Result<(), String> {
        self.with_session(session_id, |s| s.write_input(data))
    }

    /// Resize a session's terminal.
    pub fn resize(&self, session_id: &str, rows: u16, cols: u16) -> Result<(), String> {
        self.with_session(session_id, |s| s.resize(rows, cols))
    }

    /// Snapshot a session's screen. An unknown session is an error, never an
    /// empty screen — a blank grid would read as "the terminal is idle".
    pub fn attach_snapshot(
        &self,
        session_id: &str,
    ) -> Result<aleph_protocol::pty::PtyAttachResponse, String> {
        self.with_session(session_id, |s| Ok(s.attach_snapshot()))
    }

    /// Terminate and remove a session.
    pub fn close(&self, session_id: &str) -> Result<(), String> {
        let session = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.order.retain(|i| i != session_id);
            inner.viewports.remove(session_id);
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

    /// Terminate every live session, returning how many were killed. Used
    /// when the terminal switch is turned off: a gate evaluated only at
    /// admission leaves the shell that is already open still open.
    pub fn close_all(&self) -> usize {
        let sessions: Vec<Arc<PtySession>> = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.order.clear();
            inner.viewports.clear();
            inner.sessions.drain().map(|(_, s)| s).collect()
        };
        let n = sessions.len();
        for s in sessions {
            s.kill();
        }
        n
    }

    /// Remove a session from the registry without killing it (called by the
    /// reader thread after the child has already exited).
    pub fn remove(&self, session_id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.order.retain(|i| i != session_id);
        inner.viewports.remove(session_id);
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
                attached_count: inner.viewports.get(&s.id).map_or(0, HashMap::len),
                created_by: s.created_by.clone(),
            })
            .collect()
    }

    /// Record a client's viewport and re-apply the smallest one. The
    /// existence check and the insert happen under the SAME lock
    /// acquisition — checking via a separate `list()` call first (as the
    /// caller used to) leaves a TOCTOU window where a `close()`/`remove()`
    /// landing between the check and the record creates an orphaned
    /// `viewports` entry for a dead `session_id`, invisible to `list()`
    /// (`attached_count` only iterates sessions still in `inner.sessions`)
    /// and reclaimed only when the connection eventually disconnects via
    /// `release_conn`.
    pub fn note_viewport(
        &self,
        session_id: &str,
        conn_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(), String> {
        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !inner.sessions.contains_key(session_id) {
                return Err(format!("no such session: {session_id}"));
            }
            inner
                .viewports
                .entry(session_id.to_string())
                .or_default()
                .insert(conn_id.to_string(), (rows.max(1), cols.max(1)));
        }
        self.apply_effective_size(session_id);
        Ok(())
    }

    /// Drop every viewport constraint held by a departing connection.
    ///
    /// This is what makes "a client's constraint is released when it goes
    /// away" structural rather than something every call site has to
    /// remember: without it, a crashed tab (or one that simply never sends
    /// `pty.close`) pins the shared PTY at whatever size it last requested,
    /// forever, with no surface that shows the zombie row. Called from the
    /// gateway's connection-teardown block, alongside the cleanup for the
    /// other per-connection subsystems (subscriptions, reverse-RPC, presence).
    pub fn release_conn(&self, conn_id: &str) {
        let touched: Vec<String> = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let mut touched = Vec::new();
            for (sid, map) in &mut inner.viewports {
                if map.remove(conn_id).is_some() {
                    touched.push(sid.clone());
                }
            }
            inner.viewports.retain(|_, m| !m.is_empty());
            touched
        };
        for sid in touched {
            self.apply_effective_size(&sid);
        }
    }

    /// The size every attached client can display: the per-axis minimum.
    /// `None` when the session has no recorded viewports (nobody has ever
    /// called `pty.resize` on it, or the last client just released it) —
    /// deliberately not a fallback size: `apply_effective_size` treats that
    /// as "leave the PTY's current size alone" rather than resizing it to
    /// some default, so an empty table can never shrink a still-open
    /// terminal to a nonsense size.
    #[must_use]
    pub fn effective_size(&self, session_id: &str) -> Option<(u16, u16)> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let map = inner.viewports.get(session_id)?;
        map.values()
            .copied()
            .reduce(|(ar, ac), (br, bc)| (ar.min(br), ac.min(bc)))
    }

    fn apply_effective_size(&self, session_id: &str) {
        let Some((rows, cols)) = self.effective_size(session_id) else {
            return;
        };
        let _ = self.resize(session_id, rows, cols);
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

    /// Start the process-global flush loop. Idempotent — safe to call from
    /// every gateway boot path.
    pub fn start_flush_loop(&'static self) {
        static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let Some(bus) = self.current_bus() else {
                    continue;
                };
                let sessions: Vec<Arc<PtySession>> = {
                    let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    inner.sessions.values().cloned().collect()
                };
                for session in sessions {
                    let Some(frame) = session.feed_and_take_frame() else {
                        continue;
                    };
                    let Ok(data) = serde_json::to_value(&frame) else {
                        continue;
                    };
                    let ev = TopicEvent::new(aleph_protocol::pty::PTY_SCREEN_TOPIC, data);
                    let _ = bus.publish(serde_json::to_string(&ev).unwrap_or_default());
                }
            }
        });
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

    /// Two clients with different viewports share one PTY, which has exactly
    /// one size. The smallest wins (tmux's convention for shared sessions):
    /// deterministic, and it never thrashes between two live clients.
    #[test]
    fn the_smallest_attached_viewport_wins() {
        let mgr = PtyManager::new();
        let res = mgr
            .spawn(&SpawnOptions {
                rows: 40,
                cols: 120,
                ..Default::default()
            })
            .expect("spawn");
        let sid = res.session_id;

        mgr.note_viewport(&sid, "conn-a", 40, 120).expect("note ok");
        mgr.note_viewport(&sid, "conn-b", 24, 80).expect("note ok");
        assert_eq!(mgr.effective_size(&sid), Some((24, 80)));

        // The constraint must be released when its client goes away —
        // otherwise a crashed tab pins every other client to its size.
        mgr.release_conn("conn-b");
        assert_eq!(mgr.effective_size(&sid), Some((40, 120)));

        mgr.close(&sid).expect("close");
    }

    #[test]
    fn attached_count_is_visible_for_diagnosis() {
        let mgr = PtyManager::new();
        let sid = mgr
            .spawn(&SpawnOptions::default())
            .expect("spawn")
            .session_id;
        mgr.note_viewport(&sid, "conn-a", 24, 80).expect("note ok");
        mgr.note_viewport(&sid, "conn-b", 24, 80).expect("note ok");
        assert_eq!(mgr.list()[0].attached_count, 2);
        mgr.close(&sid).expect("close");
    }

    /// `note_viewport` must reject an unknown session under the SAME lock
    /// acquisition it uses to record the viewport — not via a separate
    /// `list()` call from the caller, which would leave a TOCTOU window
    /// where a `close()`/`remove()` landing between the check and the
    /// record creates an orphaned `viewports` entry for a dead session_id.
    #[test]
    fn note_viewport_on_unknown_session_is_an_error() {
        let mgr = PtyManager::new();
        assert!(mgr.note_viewport("ghost", "conn-a", 24, 80).is_err());
        // And it must not have recorded anything under that dead id.
        assert_eq!(mgr.effective_size("ghost"), None);
    }

    /// A config field with no consumer is indistinguishable from one nobody
    /// sets: it looks settable and never does anything. `scrollback_lines`
    /// must reach the grid that actually bounds the ring.
    #[test]
    fn the_configured_scrollback_reaches_the_session_grid() {
        let mgr = PtyManager::new();
        let sid = mgr
            .spawn(&SpawnOptions {
                rows: 3,
                cols: 10,
                scrollback_lines: Some(7),
                ..Default::default()
            })
            .expect("spawn")
            .session_id;
        assert_eq!(
            mgr.scrollback_limit_of(&sid),
            Some(7),
            "the configured limit must bound the session's ring, not the built-in default"
        );
        mgr.close(&sid).expect("close");
    }

    /// Accountability names the person, not just the identity: on a
    /// multi-user install "which operator" is the question an audit asks.
    #[test]
    fn a_spawn_records_who_asked_for_it() {
        // A LOCAL PtyManager, so `list()` really is this test's own sessions.
        // Never index the process-global one -- see the handler test below.
        let mgr = PtyManager::new();
        let sid = mgr
            .spawn(&SpawnOptions {
                created_by: Some("u-alice".to_string()),
                ..Default::default()
            })
            .expect("spawn")
            .session_id;
        assert_eq!(mgr.list()[0].created_by.as_deref(), Some("u-alice"));
        mgr.close(&sid).expect("close");
    }

    /// Turning the switch off must kill live sessions, not merely block new
    /// ones: a gate evaluated only at admission leaves the shell that is
    /// already open still open.
    #[test]
    fn close_all_terminates_every_live_session() {
        let mgr = PtyManager::new();
        let a = mgr.spawn(&SpawnOptions::default()).expect("a").session_id;
        let b = mgr.spawn(&SpawnOptions::default()).expect("b").session_id;
        assert_eq!(mgr.list().len(), 2);
        assert_eq!(mgr.close_all(), 2);
        assert!(mgr.list().is_empty());
        assert!(mgr.write(&a, b"x").is_err());
        assert!(mgr.write(&b, b"x").is_err());
    }
}
