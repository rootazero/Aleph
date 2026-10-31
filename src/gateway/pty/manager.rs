//! Process-global registry of live PTY sessions.
//!
//! The OnceLock remains untouched on disabled boot. The gateway activates
//! this source owner with its existing event bus only when live Config admits
//! Terminal. One ProviderHandle fences operations and frame commits; one
//! retained driver handle proves stop/join without duplicating session owners.
//!
//! Sessions are bounded (FIFO eviction at `MAX_SESSIONS`) so a runaway client
//! cannot exhaust descriptors/threads — the Rust analogue of hermes' implicit
//! per-process `_sessions` dict, but with an explicit ceiling.

use std::collections::{HashMap, VecDeque};
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use serde::Serialize;
use tokio::sync::Notify;

use super::session::{PtySession, SpawnOptions};
use crate::builtin_tools::process_journal::{self, Verdict};
use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderState, ProviderTransport,
};
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

/// How many `session_id -> created_by` stamps are kept for the ownership
/// filter, INCLUDING sessions that are already gone.
///
/// The retention is the point, not a leak. `pty.exit` is published by the
/// reader thread and `PtyManager::remove` is called on the very next line,
/// while delivery to a subscriber is asynchronous — so an ownership filter
/// that consulted live sessions alone would fail closed on the one frame
/// that tells a client its shell died. `EventVisibilityIndex`'s module doc
/// rules on the same shape for run->session seeds: retire by CAPACITY, never
/// at end-of-life, because "the entry is gone" and "the caller may not have
/// it" are different answers and only one of them is true here.
///
/// Comfortably above any plausible `[policies.terminal] max_sessions` (the
/// default is 64) so a stamp cannot age out while its session is still
/// live; each entry is two short strings.
const OWNER_RETENTION: usize = 1024;

/// Publish cadence. 16 ms ≈ 60 Hz: fast enough that no human sees the delay,
/// slow enough that a process writing megabytes per second still costs one
/// bounded frame per tick. This coalescing *is* the backpressure design.
const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// Internal summary of a session, from which the wire row
/// ([`aleph_protocol::pty::PtySessionInfo`]) is built.
///
/// This type is NOT the wire shape and deliberately does not derive
/// `Serialize`: it carries `created_by`, which no client may see, and a
/// serialisable server struct is exactly how that stamp would reach the wire
/// the next time someone reached for `json!({ "sessions": … })`. Convert with
/// the `From` impl below and let the compiler enforce the difference.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub shell: String,
    /// The directory the child was SPAWNED in, empty when it inherited the
    /// server's. See [`PtySession::cwd`] for why this is not the live cwd.
    pub cwd: String,
    pub created_at: i64,
    pub closed: bool,
    /// Who asked for this session — see [`SpawnOptions::created_by`]. `None`
    /// for a spawn that did not come through a caller-identified face.
    ///
    /// Read by [`handle_list`](crate::gateway::handlers::pty::handle_list),
    /// which drops every row this caller does not own; the same stamp answers
    /// the four addressed methods and the `pty.screen`/`pty.exit` delivery
    /// filter through [`PtyManager::owner_of`].
    pub created_by: Option<String>,
}

/// The one place a session becomes a wire row. Every face that answers
/// "which sessions are there" — the `pty.list` handler and the `terminal`
/// tool — goes through here, so the key set has a single author and
/// `created_by` has no way out.
impl From<&SessionInfo> for aleph_protocol::pty::PtySessionInfo {
    fn from(s: &SessionInfo) -> Self {
        Self {
            session_id: s.session_id.clone(),
            shell: s.shell.clone(),
            cwd: s.cwd.clone(),
            created_at: s.created_at,
            closed: s.closed,
        }
    }
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
    /// `session_id -> created_by`, deliberately OUTLIVING the session — see
    /// [`OWNER_RETENTION`]. Written once at spawn and never rewritten: a
    /// session's creator does not change, and a second writer would be a
    /// second answer to "whose is this".
    owners: HashMap<String, Option<String>>,
    /// Insertion order for [`OWNER_RETENTION`] eviction.
    owner_order: VecDeque<String>,
}

impl Inner {
    /// Record who created a session, evicting the oldest stamp past
    /// [`OWNER_RETENTION`]. Called under the same lock that inserts the
    /// session itself.
    fn remember_owner(&mut self, session_id: &str, created_by: Option<String>) {
        if self
            .owners
            .insert(session_id.to_string(), created_by)
            .is_none()
        {
            self.owner_order.push_back(session_id.to_string());
        }
        while self.owner_order.len() > OWNER_RETENTION {
            if let Some(old) = self.owner_order.pop_front() {
                self.owners.remove(&old);
            }
        }
    }
}

/// The answer to "who created this session" — the input to the ONE ownership
/// predicate every `pty.*` face shares ([`owner_admits`]).
///
/// Two variants rather than `Option<Option<String>>` because the outer layer
/// is a different question from the inner one: `Unknown` is "there is no
/// record of this id" (never existed, or aged out of `OWNER_RETENTION`),
/// while `Known(None)` is "this session exists and was spawned through a face
/// that resolved no caller". Folding them together is how a fail-closed answer
/// gets consumed as a value — `CLAUDE.md` §0 rules on exactly that shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOwner {
    /// The session is (or recently was) on record; this is its `created_by`.
    Known(Option<String>),
    /// No record at all.
    Unknown,
}

impl SessionOwner {
    /// Whether `actor` may address — or be delivered frames for — the session
    /// this stamp describes.
    #[must_use]
    pub fn admits(&self, actor: Option<&str>) -> bool {
        match self {
            Self::Known(created_by) => owner_admits(created_by.as_deref(), actor),
            // Never existed, or aged out: fail closed for anyone who has an
            // identity to compare against, unrestricted for an unscoped caller
            // on the same reasoning as `owner_admits`.
            Self::Unknown => actor.is_none(),
        }
    }
}

/// The single derivation of "may this caller have this session", shared by
/// `pty.list`'s filter, the four addressed methods, and the
/// `pty.screen`/`pty.exit` delivery filter.
///
/// One body because a predicate answered once per face is this repo's
/// signature defect: an ownership rule enforced on `pty.list` and not on
/// `pty.input` is not an ownership rule, it is a hidden list.
///
/// `actor: None` is unrestricted, matching every `visibility::*_visible_to`
/// in the repo: it means no caller identity was resolved — internal wiring, a
/// test, or a deployment that resolves none — not "an anonymous stranger". On
/// the delivery face the topic is separately operator-gated by
/// `EventScopeGuard`, so a walled socket never reaches this predicate.
///
/// `created_by: None` with a scoped actor is REFUSED: a session nobody is
/// recorded as owning is not one a particular user may claim. In production
/// this cannot arise — `handle_spawn` stamps `visibility::ambient_actor()`,
/// which resolves through `CALLER_USER` for every gateway dispatch, loopback
/// included (it resolves to `OWNER_USER_ID`).
#[must_use]
pub fn owner_admits(created_by: Option<&str>, actor: Option<&str>) -> bool {
    match actor {
        None => true,
        Some(actor) => created_by == Some(actor),
    }
}

/// A read-only view of the source-owned Terminal lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleSnapshot {
    pub state: ProviderState,
    pub generation: u64,
    pub driver_active: bool,
    pub failure: Option<TerminalResidual>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("terminal generation {generation}: {detail}")]
pub struct TerminalResidual {
    pub generation: u64,
    pub detail: String,
}

struct DriverState {
    generation: u64,
    stop: Arc<Notify>,
    handle: tokio::task::JoinHandle<()>,
}

/// The global PTY session registry and its source-owned lifecycle.
pub struct PtyManager {
    inner: Arc<Mutex<Inner>>,
    bus: Arc<Mutex<Option<Arc<GatewayEventBus>>>>,
    lifecycle: ProviderHandle,
    /// Source serialization, without holding ProviderHandle's mutex over PTY I/O.
    admission: Arc<Mutex<()>>,
    driver: Mutex<Option<DriverState>>,
    stop_wait: tokio::sync::Mutex<()>,
    failure: Mutex<Option<TerminalResidual>>,
    /// The current concurrent session ceiling — see [`DEFAULT_MAX_SESSIONS`]
    /// and [`Self::set_max_sessions`].
    max_sessions: AtomicUsize,
}

/// Take the next incremental frame from a session.
pub(crate) fn flush_session(session: &PtySession) -> Option<aleph_protocol::pty::PtyScreenFrame> {
    session.feed_and_take_frame()
}

static GLOBAL: OnceLock<PtyManager> = OnceLock::new();

/// Access the process-global PTY manager, initializing its source-owned state.
#[must_use]
pub fn manager() -> &'static PtyManager {
    GLOBAL.get_or_init(PtyManager::cold)
}

/// Read the process-global manager without initializing Terminal runtime state.
#[must_use]
pub fn try_manager() -> Option<&'static PtyManager> {
    GLOBAL.get()
}

/// Attach the gateway event bus and activate the source-owned Terminal driver.
pub fn attach_event_bus(bus: Arc<GatewayEventBus>) -> Result<u64, TerminalResidual> {
    manager().activate(bus)
}

/// Reject queued frames from stopped/replaced drivers without initializing the manager.
pub(crate) fn frame_admits(topic: &str, payload: Option<&serde_json::Value>) -> bool {
    if !matches!(
        topic,
        aleph_protocol::pty::PTY_SCREEN_TOPIC | aleph_protocol::pty::PTY_EXIT_TOPIC
    ) {
        return true;
    }
    let Some(generation) = payload
        .and_then(|data| data.get("provider_generation"))
        .and_then(serde_json::Value::as_u64)
    else {
        return false;
    };
    try_manager().is_some_and(|manager| manager.admit_generation(generation).is_ok())
}

/// The one wording for "this session id does not exist", shared by
/// [`PtyManager::with_session`]'s own not-found branch,
/// `gateway::handlers::pty::require_owned`, and `builtin_tools::terminal`'s
/// `read` action.
///
/// Three call sites used to format this literal independently
/// (task-11 review F9). That mattered because two of them have to produce
/// BYTE-IDENTICAL text on purpose: `require_owned` and `terminal::read`
/// both refuse a session the caller does not own with exactly this string,
/// not a distinct "not yours" — an addressed lookup that could tell the two
/// apart would let a caller enumerate other operators' session ids. A
/// wording change to one of three independent literals would have silently
/// re-opened that oracle; a change here changes all three at once.
pub(crate) fn no_such_session(session_id: &str) -> String {
    format!("no such session: {session_id}")
}

impl PtyManager {
    #[cfg(test)]
    fn new() -> TestManager {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let manager = Self::cold();
        let _entered = runtime.enter();
        manager.activate(Arc::new(GatewayEventBus::new())).unwrap();
        drop(_entered);
        TestManager { manager, runtime }
    }

    fn cold() -> Self {
        Self::with_lifecycle(ProviderLifecycle::new(
            terminal_provider_id(),
            ProviderTransport::InProcess,
        ))
    }

    fn with_lifecycle(lifecycle: ProviderLifecycle) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            bus: Arc::new(Mutex::new(None)),
            lifecycle: ProviderHandle::new(lifecycle),
            admission: Arc::new(Mutex::new(())),
            driver: Mutex::new(None),
            stop_wait: tokio::sync::Mutex::new(()),
            failure: Mutex::new(None),
            max_sessions: AtomicUsize::new(DEFAULT_MAX_SESSIONS),
        }
    }

    #[must_use]
    pub fn lifecycle_snapshot(&self) -> LifecycleSnapshot {
        let driver = self.driver.lock().unwrap_or_else(|e| e.into_inner());
        let (state, generation) = self.lifecycle.snapshot();
        LifecycleSnapshot {
            state,
            generation,
            driver_active: driver.as_ref().is_some_and(|d| d.generation == generation),
            failure: self
                .failure
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }

    pub fn ready_generation(&self) -> Result<u64, String> {
        let generation = self.lifecycle.generation();
        self.admit_generation(generation)?;
        Ok(generation)
    }

    /// Check a captured generation; effects use the guarded form below.
    pub fn admit_generation(&self, generation: u64) -> Result<(), String> {
        let driver = self.driver.lock().unwrap_or_else(|e| e.into_inner());
        if self.lifecycle.state() == ProviderState::Ready
            && !driver.as_ref().is_some_and(|d| !d.handle.is_finished())
        {
            let _admission = self.admission.lock().unwrap_or_else(|e| e.into_inner());
            return Err(self
                .fail("Ready provider lost its running driver")
                .to_string());
        }
        self.lifecycle
            .acquire_call(generation)
            .map_err(|e| e.to_string())
    }

    fn residual(&self, detail: impl Into<String>) -> TerminalResidual {
        TerminalResidual {
            generation: self.lifecycle.generation(),
            detail: detail.into(),
        }
    }

    fn fail(&self, detail: impl Into<String>) -> TerminalResidual {
        let residual = self.residual(detail);
        let residual = {
            let mut failure = self.failure.lock().unwrap_or_else(|e| e.into_inner());
            match failure.as_ref() {
                Some(first) if first.generation == residual.generation => first.clone(),
                _ => {
                    *failure = Some(residual.clone());
                    residual
                }
            }
        };
        if matches!(
            self.lifecycle.state(),
            ProviderState::Starting | ProviderState::Ready | ProviderState::Stopping
        ) {
            self.lifecycle
                .transition(ProviderState::Failed)
                .expect("failure transition is valid");
        }
        residual
    }

    /// Start exactly one owned driver. Repeated activation cannot replace its bus.
    pub fn activate(&self, bus: Arc<GatewayEventBus>) -> Result<u64, TerminalResidual> {
        if self.lifecycle.state() == ProviderState::Stopping {
            let _waiter = self
                .stop_wait
                .try_lock()
                .map_err(|_| self.residual("terminal stop is awaiting its driver"))?;
            let generation = self.lifecycle.generation();
            match self.poll_stop(generation, &mut Context::from_waker(Waker::noop())) {
                Poll::Ready(result) => result?,
                Poll::Pending => return Err(self.residual("terminal stop still owns its driver")),
            }
        }
        let mut driver = self.driver.lock().unwrap_or_else(|e| e.into_inner());
        let _admission = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        let (state, generation) = self.lifecycle.snapshot();
        if state == ProviderState::Ready {
            if driver.as_ref().is_some_and(|d| !d.handle.is_finished()) {
                if self
                    .current_bus()
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &bus))
                {
                    return Ok(generation);
                }
                return Err(self.residual("stop the current driver before replacing its event bus"));
            }
            return Err(self.fail("Ready provider has no running owned driver"));
        }
        if driver.is_some() {
            return Err(self.residual("terminal retains an unjoined driver"));
        }
        if state == ProviderState::Discovered {
            self.lifecycle
                .transition(ProviderState::Cold)
                .map_err(|e| self.residual(e.to_string()))?;
        }
        let generation = self
            .lifecycle
            .transition(ProviderState::Starting)
            .map_err(|e| self.residual(e.to_string()))?
            .generation;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| self.fail("activation requires a Tokio runtime"))?;
        *self.bus.lock().unwrap_or_else(|e| e.into_inner()) = Some(bus);
        let stop = Arc::new(Notify::new());
        *driver = Some(DriverState {
            generation,
            handle: runtime.spawn(run_flush_driver(
                self.inner.clone(),
                self.bus.clone(),
                self.lifecycle.clone(),
                self.admission.clone(),
                generation,
                stop.clone(),
            )),
            stop,
        });
        self.lifecycle
            .transition(ProviderState::Ready)
            .map_err(|e| self.fail(e.to_string()))?;
        Ok(generation)
    }

    fn begin_stop(&self) -> Result<u64, TerminalResidual> {
        let driver = self.driver.lock().unwrap_or_else(|e| e.into_inner());
        let _admission = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        let (state, generation) = self.lifecycle.snapshot();
        if driver.is_none() {
            if matches!(
                state,
                ProviderState::Discovered
                    | ProviderState::Cold
                    | ProviderState::Disabled
                    | ProviderState::Stopped
            ) {
                return Ok(generation);
            }
            return Err(self.fail("stop has no owned driver"));
        }
        if matches!(state, ProviderState::Ready | ProviderState::Starting) {
            // Transition serializes against every operation and frame commit.
            self.lifecycle
                .transition(ProviderState::Stopping)
                .map_err(|e| self.fail(e.to_string()))?;
        }
        let cleanup = self.close_all_checked();
        // notify_one retains a permit even if the task has not started yet.
        driver
            .as_ref()
            .expect("driver is present")
            .stop
            .notify_one();
        cleanup?;
        Ok(generation)
    }

    fn poll_stop(
        &self,
        generation: u64,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), TerminalResidual>> {
        let mut driver = self.driver.lock().unwrap_or_else(|e| e.into_inner());
        if self.lifecycle.generation() != generation {
            return Poll::Ready(Err(self.residual("stop belongs to an older generation")));
        }
        let Some(owned) = driver.as_mut() else {
            return Poll::Ready(
                match self
                    .failure
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                {
                    Some(error) if self.lifecycle.state() == ProviderState::Failed => Err(error),
                    _ => Ok(()),
                },
            );
        };
        let joined = std::task::ready!(Pin::new(&mut owned.handle).poll(cx));
        if let Err(error) = joined {
            let residual = self.fail(format!("driver join failed: {error}"));
            // Failed is published and evidence retained BEFORE clearing a completed failed handle.
            *driver = None;
            return Poll::Ready(Err(residual));
        }
        if self.lifecycle.state() == ProviderState::Failed {
            *driver = None;
            return Poll::Ready(Err(self
                .failure
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_else(|| self.residual("driver completed after failure"))));
        }
        self.lifecycle
            .transition(ProviderState::Stopped)
            .map_err(|e| self.fail(e.to_string()))?;
        *driver = None;
        Poll::Ready(Ok(()))
    }

    /// Cancellation leaves the JoinHandle in this manager, with admission revoked.
    pub async fn deactivate(&self) -> Result<(), TerminalResidual> {
        let _waiter = self.stop_wait.lock().await;
        let generation = self.begin_stop()?;
        poll_fn(|cx| self.poll_stop(generation, cx)).await
    }

    /// The synchronous config API reports pending join honestly instead of detaching a task.
    /// A later apply/start or async shutdown polls the same retained handle.
    pub fn request_deactivate(&self) -> Result<bool, TerminalResidual> {
        let generation = self.begin_stop()?;
        let Ok(_waiter) = self.stop_wait.try_lock() else {
            return Ok(false);
        };
        match self.poll_stop(generation, &mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => result.map(|()| true),
            Poll::Pending => Ok(false),
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

    pub(crate) fn current_bus(&self) -> Option<Arc<GatewayEventBus>> {
        self.bus.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Spawn a new session, evicting the oldest if at capacity.
    ///
    /// The session's journal row is written in the same order the bash twin
    /// keeps: intent BEFORE the child exists, pid right after. A daemon that
    /// dies between the two leaves a row the next boot tombstones as
    /// "interrupted, liveness unknown"; one that dies after leaves a row the
    /// boot can ask the OS about. The journal only records — it never kills
    /// a shell a previous daemon left behind (U5).
    ///
    /// The row names the shell the session will actually run
    /// ([`SpawnOptions::shell_label`] — the same derivation `PtySession::spawn`
    /// stamps on `shell`), not a placeholder: the tombstone a later face
    /// renders quotes this string, and a wrong label there reads as a fact.
    pub fn spawn(&self, opts: &SpawnOptions) -> Result<SpawnResult, String> {
        self.spawn_at(self.ready_generation()?, opts)
    }

    pub fn spawn_at(&self, generation: u64, opts: &SpawnOptions) -> Result<SpawnResult, String> {
        self.with_generation(generation, || self.spawn_admitted(generation, opts))
    }

    fn spawn_admitted(&self, generation: u64, opts: &SpawnOptions) -> Result<SpawnResult, String> {
        let id = uuid::Uuid::new_v4().to_string();
        process_journal::record_pty_spawn(
            &id,
            &opts.shell_label(),
            opts.cwd.as_deref().unwrap_or(""),
            opts.created_by.as_deref(),
        );
        let bus = self.current_bus();
        let lifecycle = self.lifecycle.clone();
        let inner = self.inner.clone();
        let admission = self.admission.clone();
        let sink = Arc::new(move |session: &PtySession, exit_code: Option<u32>| {
            // The journal settles before this generation-fenced publication/removal.
            let _admission = admission.lock().unwrap_or_else(|e| e.into_inner());
            if lifecycle.acquire_call(generation).is_err() {
                return;
            }
            {
                if let Some(bus) = &bus {
                    if let Some(frame) = flush_session(session) {
                        if let Ok(mut data) = serde_json::to_value(frame) {
                            data["provider_generation"] = generation.into();
                            let _ = bus.publish_json(&TopicEvent::new(
                                aleph_protocol::pty::PTY_SCREEN_TOPIC,
                                data,
                            ));
                        }
                    }
                    let _ = bus.publish_json(&TopicEvent::new(
                        aleph_protocol::pty::PTY_EXIT_TOPIC,
                        serde_json::json!({"session_id": session.id, "exit_code": exit_code.unwrap_or(0), "provider_generation": generation}),
                    ));
                }
                let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.order.retain(|id| id != &session.id);
                inner.viewports.remove(&session.id);
                inner.sessions.remove(&session.id);
            }
        });
        let session = match PtySession::spawn_with_sink(id.clone(), opts, sink) {
            Ok(session) => session,
            Err(e) => {
                // Nothing ran. Settle the intent row now rather than leave
                // it `Running` for the next boot to read as a shell the
                // restart interrupted — with the error as the screen, so the
                // row says what the spawn said.
                process_journal::record_pty_settled(&id, Verdict::Exited, None, &e);
                return Err(e);
            }
        };
        if let Some(pid) = session.shell_pid() {
            process_journal::record_pty_child(&id, pid);
        }
        let result = SpawnResult {
            session_id: session.id.clone(),
            shell: session.shell.clone(),
        };

        // Evict-then-insert under the lock so the map never exceeds the cap.
        let cap = self.max_sessions.load(Ordering::SeqCst);
        let evicted = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            // Stamp the owner under the SAME lock acquisition that inserts the
            // session, so no face can observe a live session with no ownership
            // record and fall through to whatever its "unknown" arm does.
            inner.remember_owner(&id, opts.created_by.clone());
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
            // The operator's capacity policy ending a shell, not orphan
            // handling: recorded as `killed` before the kill, like `close`.
            Self::record_killed(&old);
            old.kill();
        }
        Ok(result)
    }

    /// The journal's `killed` verdict for a session this manager is about to
    /// `kill()`, with the screen as it stands — after the kill nothing more
    /// arrives, so the screen at this moment IS the final screen, exactly what
    /// `settle_exit` would have written had it won. One body for the three
    /// kill sites (`close`, `close_all`, capacity eviction) so none of them
    /// can pass an empty screen and leave a killed terminal's row with no
    /// trail (the bash twin keeps its last window in `partial.txt`).
    fn record_killed(session: &PtySession) {
        let screen = session.with_screen(super::screen::Screen::visible_text);
        process_journal::record_pty_settled(&session.id, Verdict::Killed, None, &screen);
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
        self.write_at(self.ready_generation()?, session_id, data)
    }

    pub fn write_at(&self, generation: u64, session_id: &str, data: &[u8]) -> Result<(), String> {
        self.with_session_at(generation, session_id, |s| s.write_input(data))
    }

    /// Resize a session's terminal.
    pub fn resize(&self, session_id: &str, rows: u16, cols: u16) -> Result<(), String> {
        self.resize_at(self.ready_generation()?, session_id, rows, cols)
    }

    pub fn resize_at(
        &self,
        generation: u64,
        session_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(), String> {
        self.with_session_at(generation, session_id, |s| s.resize(rows, cols))
    }

    /// Snapshot a session's screen. An unknown session is an error, never an
    /// empty screen — a blank grid would read as "the terminal is idle".
    pub fn attach_snapshot(
        &self,
        session_id: &str,
    ) -> Result<aleph_protocol::pty::PtyAttachResponse, String> {
        self.attach_snapshot_at(self.ready_generation()?, session_id)
    }

    pub fn attach_snapshot_at(
        &self,
        generation: u64,
        session_id: &str,
    ) -> Result<aleph_protocol::pty::PtyAttachResponse, String> {
        self.with_session_at(generation, session_id, |s| Ok(s.attach_snapshot()))
    }

    /// The current visible screen as plain text (no scrollback).
    ///
    /// Deliberately narrower than [`Self::attach_snapshot`]: that response
    /// carries a diff-encoded [`aleph_protocol::pty::PtyScreenPatch`] meant
    /// for a terminal renderer, not a string a model can read. Same posture
    /// as `attach_snapshot` otherwise — an unknown session is an error, never
    /// an empty screen, because a blank grid would read as "the terminal is
    /// idle".
    #[allow(dead_code)] // used by tests only — no production reader since the terminal
                        // herdr removal (5dd337df1) took the agent-detection caller with it
    pub(crate) fn visible_text(&self, session_id: &str) -> Result<String, String> {
        self.with_session(session_id, |s| {
            Ok(s.with_screen(super::screen::Screen::visible_text))
        })
    }

    /// Terminate and remove a session. The journal verdict (`killed`) is
    /// written BEFORE the kill: the reader thread's exit settle follows the
    /// kill and would otherwise record `exited` for a shell the operator
    /// stopped — the journal keeps whichever verdict lands first.
    pub fn close(&self, session_id: &str) -> Result<(), String> {
        self.close_at(self.ready_generation()?, session_id)
    }

    pub fn close_at(&self, generation: u64, session_id: &str) -> Result<(), String> {
        self.with_generation(generation, || self.close_admitted(session_id))
    }

    fn close_admitted(&self, session_id: &str) -> Result<(), String> {
        let session = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.order.retain(|i| i != session_id);
            inner.viewports.remove(session_id);
            inner.sessions.remove(session_id)
        };
        match session {
            Some(s) => {
                Self::record_killed(&s);
                s.kill();
                Ok(())
            }
            None => Err(format!("no such session: {session_id}")),
        }
    }

    /// Terminate every live session, returning how many were killed. Called
    /// wherever the whole set has to end at once: the terminal switch being
    /// turned off (a gate evaluated only at admission leaves the shell that
    /// is already open still open), and the daemon's exit paths in
    /// `aleph-server`'s `start` command — beside the bash registry's reaper
    /// (`bash_exec::kill_all_running_background`), whose twin this is. Each
    /// session is journaled `killed` before its kill, as in [`Self::close`],
    /// which is what keeps a clean stop from leaving every open PTY row
    /// `running` on disk for the next boot to tombstone as a shell the
    /// restart *interrupted*.
    pub fn close_all(&self) -> usize {
        self.with_generation(self.lifecycle.generation(), || {
            self.close_all_checked().map_err(|error| error.to_string())
        })
        .unwrap_or(0)
    }

    fn close_all_checked(&self) -> Result<usize, TerminalResidual> {
        let sessions: Vec<Arc<PtySession>> = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.order.clear();
            inner.viewports.clear();
            inner.sessions.drain().map(|(_, s)| s).collect()
        };
        let n = sessions.len();
        let failures = sessions
            .into_iter()
            .filter_map(|session| {
                Self::record_killed(&session);
                let error = session.try_kill().err()?;
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.order.push_back(session.id.clone());
                inner.sessions.insert(session.id.clone(), session);
                Some(error)
            })
            .collect::<Vec<_>>();
        if !failures.is_empty() {
            return Err(self.fail(failures.join("; ")));
        }
        Ok(n)
    }

    /// Remove a session from the registry without killing it (called by the
    /// reader thread after the child has already exited).
    pub fn remove(&self, session_id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.order.retain(|i| i != session_id);
        inner.viewports.remove(session_id);
        inner.sessions.remove(session_id);
    }

    /// Who created a session — the ownership input for every `pty.*` face.
    ///
    /// Answers for sessions that are ALREADY GONE, up to `OWNER_RETENTION`.
    /// That is not slack, it is the requirement: `pty.exit` is published and
    /// `remove` is called on the next line, so a filter that consulted
    /// `inner.sessions` would deny a client the frame announcing its own
    /// shell's death — and a client that never learns its shell died shows a
    /// live terminal forever.
    #[must_use]
    pub fn owner_of(&self, session_id: &str) -> SessionOwner {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match inner.owners.get(session_id) {
            Some(created_by) => SessionOwner::Known(created_by.clone()),
            None => SessionOwner::Unknown,
        }
    }

    /// Snapshot of all active sessions.
    ///
    /// Deliberately UNFILTERED: the caller-scoped view is
    /// `handle_list`'s, built from [`SessionInfo::created_by`] through
    /// [`owner_admits`]. Filtering here would put the predicate below the
    /// only face that knows who is asking, and `close_all`/`live_apply` need
    /// the whole set.
    pub fn list(&self) -> Vec<SessionInfo> {
        self.with_generation(self.lifecycle.generation(), || Ok(self.list_admitted()))
            .unwrap_or_default()
    }

    fn list_admitted(&self) -> Vec<SessionInfo> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .order
            .iter()
            .filter_map(|id| inner.sessions.get(id))
            .map(|s| SessionInfo {
                session_id: s.id.clone(),
                shell: s.shell.clone(),
                cwd: s.cwd.clone(),
                created_at: s.created_at,
                closed: s.is_closed(),
                created_by: s.created_by.clone(),
            })
            .collect()
    }

    /// Record a client's viewport and re-apply the smallest one. The
    /// existence check and the insert happen under the SAME lock
    /// acquisition — checking via a separate `list()` call first (as the
    /// caller used to) leaves a TOCTOU window where a `close()`/`remove()`
    /// landing between the check and the record creates an orphaned
    /// `viewports` entry for a dead `session_id`, invisible to any caller
    /// (nothing iterates `inner.viewports` outside this lock) and reclaimed
    /// only when the connection eventually disconnects via `release_conn`.
    pub fn note_viewport(
        &self,
        session_id: &str,
        conn_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(), String> {
        self.note_viewport_at(self.ready_generation()?, session_id, conn_id, rows, cols)
    }

    pub fn note_viewport_at(
        &self,
        generation: u64,
        session_id: &str,
        conn_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(), String> {
        self.with_generation(generation, || {
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
            if let Some((rows, cols)) = self.effective_size(session_id) {
                self.with_session_admitted(session_id, |s| s.resize(rows, cols))?;
            }
            Ok(())
        })
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
        let Ok(generation) = self.ready_generation() else {
            return;
        };
        let _ = self.with_generation(generation, || {
            let touched = {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                let touched = inner
                    .viewports
                    .iter_mut()
                    .filter_map(|(sid, map)| map.remove(conn_id).map(|_| sid.clone()))
                    .collect::<Vec<_>>();
                inner.viewports.retain(|_, map| !map.is_empty());
                touched
            };
            for sid in touched {
                if let Some((rows, cols)) = self.effective_size(&sid) {
                    let _ = self.with_session_admitted(&sid, |session| session.resize(rows, cols));
                }
            }
            Ok(())
        });
    }

    /// The size every attached client can display: the per-axis minimum.
    /// `None` when the session has no recorded viewports (nobody has ever
    /// called `pty.resize` on it, or the last client just released it) —
    /// deliberately not a fallback size: viewport cleanup treats that
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

    fn with_session<F, R>(&self, session_id: &str, f: F) -> Result<R, String>
    where
        F: FnOnce(&Arc<PtySession>) -> Result<R, String>,
    {
        self.with_session_at(self.ready_generation()?, session_id, f)
    }

    fn with_session_at<R>(
        &self,
        generation: u64,
        session_id: &str,
        f: impl FnOnce(&Arc<PtySession>) -> Result<R, String>,
    ) -> Result<R, String> {
        self.with_generation(generation, || self.with_session_admitted(session_id, f))
    }

    fn with_generation<R>(
        &self,
        generation: u64,
        effect: impl FnOnce() -> Result<R, String>,
    ) -> Result<R, String> {
        self.admit_generation(generation)?;
        let _admission = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        self.lifecycle
            .acquire_call(generation)
            .map_err(|error| error.to_string())?;
        effect()
    }

    fn with_session_admitted<R>(
        &self,
        session_id: &str,
        f: impl FnOnce(&Arc<PtySession>) -> Result<R, String>,
    ) -> Result<R, String> {
        let session = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.sessions.get(session_id).cloned()
        };
        match session {
            Some(s) => f(&s),
            None => Err(no_such_session(session_id)),
        }
    }
}

fn terminal_provider_id() -> ProviderId {
    ProviderId::new("gateway.pty").expect("static provider id is non-empty")
}

async fn run_flush_driver(
    inner: Arc<Mutex<Inner>>,
    bus: Arc<Mutex<Option<Arc<GatewayEventBus>>>>,
    lifecycle: ProviderHandle,
    admission: Arc<Mutex<()>>,
    generation: u64,
    stop: Arc<Notify>,
) {
    let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.notified() => return,
            _ = ticker.tick() => {}
        }
        let _admission = admission.lock().unwrap_or_else(|e| e.into_inner());
        if lifecycle.acquire_call(generation).is_err() {
            return;
        }
        {
            let Some(bus) = bus.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
                return;
            };
            let sessions: Vec<Arc<PtySession>> = inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .sessions
                .values()
                .cloned()
                .collect();
            for session in sessions {
                let Some(frame) = flush_session(&session) else {
                    continue;
                };
                let Ok(mut data) = serde_json::to_value(&frame) else {
                    continue;
                };
                data["provider_generation"] = generation.into();
                let _ = bus.publish_json(&TopicEvent::new(
                    aleph_protocol::pty::PTY_SCREEN_TOPIC,
                    data,
                ));
            }
        }
    }
}

/// Global-consumer tests keep the driver's runtime alive across individual test runtimes.
#[cfg(test)]
pub(crate) fn activate_test_provider() -> &'static PtyManager {
    static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> = std::sync::LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap()
    });
    static BUS: std::sync::LazyLock<Arc<GatewayEventBus>> =
        std::sync::LazyLock::new(|| Arc::new(GatewayEventBus::new()));
    let manager = GLOBAL.get_or_init(PtyManager::cold);
    let _entered = RUNTIME.enter();
    manager
        .activate(manager.current_bus().unwrap_or_else(|| BUS.clone()))
        .unwrap();
    manager
}

#[cfg(test)]
struct TestManager {
    manager: PtyManager,
    runtime: tokio::runtime::Runtime,
}

#[cfg(test)]
impl std::ops::Deref for TestManager {
    type Target = PtyManager;
    fn deref(&self) -> &Self::Target {
        &self.manager
    }
}

#[cfg(test)]
impl Drop for TestManager {
    fn drop(&mut self) {
        self.runtime.block_on(self.manager.deactivate()).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_write_list_close_roundtrip() {
        let mgr = PtyManager::new();
        // The local fixture owns a real driver; no subscriber is required.
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

    /// The three writes of a PTY session's journal life, driven through a
    /// real child: intent (before the spawn — without it the pid stamp is a
    /// no-op, the same intent-first pin the bash twin has), pid (after), and
    /// the natural exit settling the row with its code. Polled off the
    /// journal rather than the bus because the journal is the thing under
    /// test; the reader thread's `settle_exit` is what writes the verdict.
    #[test]
    fn spawning_a_pty_journals_intent_then_pid_and_its_exit_settles_the_row() {
        use crate::builtin_tools::process_journal as j;
        let _g = j::test_gate();
        let tmp = tempfile::tempdir().unwrap();
        j::enable_for_test(tmp.path().to_path_buf());
        // Keep the child alive until its attached pid has been observed.
        // An immediate `exit 3` can settle the row before the first lookup.
        let (cmd, args) = if cfg!(windows) {
            (
                "cmd.exe",
                vec!["/C".into(), "set /p QA_PTY_RELEASE= & exit /b 3".into()],
            )
        } else {
            ("sh", vec!["-c".into(), "read -r _; exit 3".into()])
        };
        let mgr = PtyManager::new();
        let sid = mgr
            .spawn(&SpawnOptions {
                command: Some(cmd.into()),
                args,
                created_by: Some("u-alice".into()),
                ..Default::default()
            })
            .expect("spawn")
            .session_id;
        struct CloseSessionOnDrop<'a>(&'a PtyManager, &'a str);
        impl Drop for CloseSessionOnDrop<'_> {
            fn drop(&mut self) {
                let _ = self.0.close(self.1);
            }
        }
        let _cleanup = CloseSessionOnDrop(&mgr, &sid);
        let row = j::lookup_pty(&sid).expect("journaled").record;
        assert!(
            row.kind == j::JournalKind::Pty
                && row.phase == j::JobPhase::Running
                && row.pid.is_some(),
            "{row:?}"
        );
        let release: &[u8] = if cfg!(windows) {
            b"release\r\n"
        } else {
            b"release\n"
        };
        mgr.write(&sid, release).expect("release child");
        let settled = (0..100)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let r = j::lookup_pty(&sid)?.record;
                (r.phase == j::JobPhase::Settled).then_some(r)
            })
            .expect("exit reaches the journal within 10 s");
        assert_eq!(
            (settled.exit_code, settled.outcome.as_deref()),
            (Some(3), Some("exited"))
        );
        j::disable_for_test();
    }

    /// The intent row of a spawn that never ran is settled at once — `exited`
    /// with no exit code and no pid, the spawn error as its screen — instead
    /// of staying `Running` for the next boot to tombstone as a shell the
    /// restart interrupted. This is the second writer of `Verdict::Exited`
    /// (its doc names it); the shape `exited ∧ exit_code None ∧ pid None`
    /// cannot come from `settle_exit`, which always passes a code.
    #[test]
    fn a_failed_spawn_settles_its_intent_row_instead_of_leaving_it_running() {
        use crate::builtin_tools::process_journal as j;
        let _g = j::test_gate();
        let tmp = tempfile::tempdir().unwrap();
        j::enable_for_test(tmp.path().to_path_buf());
        const PROGRAM: &str = "aleph-no-such-program-xyz";
        let err = PtyManager::new()
            .spawn(&SpawnOptions {
                command: Some(PROGRAM.into()),
                created_by: Some("u-alice".into()),
                ..Default::default()
            })
            .expect_err("a program that does not exist cannot spawn");
        // The id is minted inside `spawn` and lost with the `Err`, so the row
        // is found by its command: the tempdir may also hold rows a sibling
        // (ungated) PTY test wrote while the journal was pointed here.
        let job = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name();
                let sid = name.to_str()?.strip_prefix("pty-")?.to_string();
                j::lookup_pty(&sid)
            })
            .find(|job| job.record.command.contains(PROGRAM))
            .expect("the failed spawn left its intent row");
        j::disable_for_test();
        let r = &job.record;
        assert_eq!(
            (r.phase, r.outcome.as_deref(), r.exit_code, r.pid),
            (j::JobPhase::Settled, Some("exited"), None, None),
            "{r:?}"
        );
        assert!(
            job.recorded_output.contains(&err),
            "the screen must carry the spawn error: {:?} vs {err:?}",
            job.recorded_output
        );
    }

    /// The intent row of a default-shell spawn names the shell the session
    /// reports — the row is written before the session exists, and the
    /// tombstone a later face renders quotes it, so a placeholder there would
    /// be read as the program (a wrong label costs more than a missing one).
    /// Also the `killed` row of a closed session carries its final screen.
    #[test]
    fn the_intent_row_names_the_resolved_shell_and_a_close_records_the_screen() {
        use crate::builtin_tools::process_journal as j;
        let _g = j::test_gate();
        let tmp = tempfile::tempdir().unwrap();
        j::enable_for_test(tmp.path().to_path_buf());
        let mgr = PtyManager::new();
        let res = mgr
            .spawn(&SpawnOptions {
                created_by: Some("u-alice".into()),
                ..Default::default()
            })
            .expect("spawn");
        let intent = j::lookup_pty(&res.session_id).expect("journaled").record;
        assert!(
            intent.command.starts_with(&res.shell) && !intent.command.contains("<default shell>"),
            "row {:?} vs session shell {:?}",
            intent.command,
            res.shell
        );
        // Put something recognisable on the screen (the PTY echoes typed
        // input, so the marker lands whether or not the shell has run it
        // yet), then kill: the screen at the kill IS the final screen, and
        // the row must carry it rather than an empty trail.
        const MARKER: &str = "aleph-screen-marker";
        mgr.write(&res.session_id, format!("echo {MARKER}\r\n").as_bytes())
            .expect("write");
        (0..100)
            .find(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                mgr.visible_text(&res.session_id)
                    .is_ok_and(|t| t.contains(MARKER))
            })
            .expect("the marker reaches the visible screen within 10 s");
        mgr.close(&res.session_id).expect("close");
        let killed = j::lookup_pty(&res.session_id).expect("journaled");
        j::disable_for_test();
        assert_eq!(
            (killed.record.phase, killed.record.outcome.as_deref()),
            (j::JobPhase::Settled, Some("killed"))
        );
        assert!(
            killed.recorded_output.contains(MARKER),
            "the killed row's [screen] must hold the screen at the kill: {:?}",
            killed.recorded_output
        );
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

    /// The stamp has to answer for a session that is already gone, because
    /// the one frame whose delivery decision needs it — `pty.exit` — is
    /// published a line BEFORE `remove`, and delivery is asynchronous. A
    /// filter keyed on live sessions would deny a client the news of its own
    /// shell's death.
    #[test]
    fn the_owner_stamp_outlives_the_session_it_names() {
        let mgr = PtyManager::new();
        let sid = mgr
            .spawn(&SpawnOptions {
                created_by: Some("u-alice".to_string()),
                ..Default::default()
            })
            .expect("spawn")
            .session_id;

        assert_eq!(
            mgr.owner_of(&sid),
            SessionOwner::Known(Some("u-alice".to_string()))
        );
        mgr.remove(&sid);
        assert!(
            mgr.list().is_empty(),
            "remove must retire the session itself"
        );
        assert_eq!(
            mgr.owner_of(&sid),
            SessionOwner::Known(Some("u-alice".to_string())),
            "and must NOT retire its ownership stamp — the exit frame is \
             published before the removal and delivered after it"
        );
        assert_eq!(
            mgr.owner_of("never-existed"),
            SessionOwner::Unknown,
            "an id nothing ever spawned is Unknown, not Known(None): the two \
             are different answers and only one of them fails closed for a \
             scoped caller"
        );
    }

    /// The retention window is bounded, or a long-lived daemon accumulates a
    /// stamp per shell it ever opened.
    #[test]
    fn owner_stamps_are_evicted_by_capacity() {
        let mgr = PtyManager::new();
        {
            let mut inner = mgr.inner.lock().expect("lock");
            for i in 0..(OWNER_RETENTION + 2) {
                inner.remember_owner(&format!("s-{i}"), Some(format!("u-{i}")));
            }
        }
        assert_eq!(mgr.owner_of("s-0"), SessionOwner::Unknown);
        assert_eq!(mgr.owner_of("s-1"), SessionOwner::Unknown);
        assert_eq!(
            mgr.owner_of("s-2"),
            SessionOwner::Known(Some("u-2".to_string()))
        );
        let inner = mgr.inner.lock().expect("lock");
        assert_eq!(inner.owners.len(), OWNER_RETENTION);
        assert_eq!(inner.owner_order.len(), OWNER_RETENTION);
    }

    /// The one predicate, exhaustively — every other face delegates here, so
    /// a hole in this table is a hole on all four of them.
    #[test]
    fn the_ownership_predicate_covers_every_combination() {
        // Same user: the only admitting scoped case.
        assert!(owner_admits(Some("u-alice"), Some("u-alice")));
        // A different operator is still a different person. Permission
        // equivalence is why they COULD be given access; it is not a reason
        // to hand it over by default, with the scrollback attached.
        assert!(!owner_admits(Some("u-alice"), Some("u-bob")));
        // An unowned session is claimable by nobody who has an identity.
        assert!(!owner_admits(None, Some("u-bob")));
        // An unscoped caller is internal wiring, not a stranger.
        assert!(owner_admits(Some("u-alice"), None));
        assert!(owner_admits(None, None));

        // And through the lookup wrapper, including the fail-closed arm.
        assert!(SessionOwner::Known(Some("u-alice".into())).admits(Some("u-alice")));
        assert!(!SessionOwner::Unknown.admits(Some("u-alice")));
        assert!(SessionOwner::Unknown.admits(None));
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

    #[tokio::test]
    async fn lifecycle_requires_activation_and_fences_restarts() {
        let mgr = PtyManager::cold();
        assert_eq!(
            mgr.lifecycle_snapshot().state,
            crate::capability::provider::ProviderState::Discovered
        );
        assert!(!mgr.lifecycle_snapshot().driver_active);
        assert!(mgr.spawn(&SpawnOptions::default()).is_err());

        let bus = Arc::new(GatewayEventBus::new());
        let first = mgr.activate(bus.clone()).expect("first activation");
        assert_eq!(
            mgr.lifecycle_snapshot().state,
            crate::capability::provider::ProviderState::Ready
        );
        assert!(mgr.lifecycle_snapshot().driver_active);
        let driver_id = mgr.driver.lock().unwrap().as_ref().unwrap().handle.id();
        assert_eq!(mgr.activate(bus.clone()).expect("repeat activation"), first);
        assert_eq!(
            mgr.driver.lock().unwrap().as_ref().unwrap().handle.id(),
            driver_id
        );

        mgr.deactivate().await.expect("stop");
        assert!(!mgr.lifecycle_snapshot().driver_active);
        assert!(mgr.admit_generation(first).is_err());

        let second = mgr.activate(bus).expect("restart");
        assert!(second > first);
        assert!(mgr.admit_generation(first).is_err());
        assert!(mgr.admit_generation(second).is_ok());
        mgr.deactivate().await.expect("final stop");
    }

    #[tokio::test]
    async fn stopping_without_a_owned_driver_retains_failure_evidence() {
        let mgr = PtyManager::cold();
        mgr.activate(Arc::new(GatewayEventBus::new())).unwrap();
        // Exercise a real cancelled driver rather than a cold, idempotent stop.
        mgr.driver.lock().unwrap().as_ref().unwrap().handle.abort();
        let result = mgr.deactivate().await;
        assert!(result.is_err());
        let snapshot = mgr.lifecycle_snapshot();
        assert_eq!(
            snapshot.state,
            crate::capability::provider::ProviderState::Failed
        );
        assert!(snapshot.failure.is_some());
        assert!(!snapshot.driver_active);
    }

    #[tokio::test]
    async fn stop_accepts_reaped_child_while_exit_callback_waits_for_admission() {
        let mgr = PtyManager::cold();
        let bus = Arc::new(GatewayEventBus::new());
        let first = mgr.activate(bus.clone()).unwrap();
        {
            // Keep the source callback from removing the session after wait().
            let _admission = mgr.admission.lock().unwrap();
            let spawned = mgr
                .spawn_admitted(
                    first,
                    &SpawnOptions {
                        command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.into()),
                        args: if cfg!(windows) {
                            vec!["/C".into(), "exit 0".into()]
                        } else {
                            vec!["-c".into(), "exit 0".into()]
                        },
                        ..Default::default()
                    },
                )
                .unwrap();
            let session = mgr.inner.lock().unwrap().sessions[&spawned.session_id].clone();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !session.is_closed() {
                assert!(std::time::Instant::now() < deadline, "child did not exit");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(mgr
                .inner
                .lock()
                .unwrap()
                .sessions
                .contains_key(&spawned.session_id));
            mgr.close_all_checked()
                .expect("observed exit completes cleanup");
        }
        mgr.deactivate()
            .await
            .expect("join driver after natural exit");
        assert_eq!(mgr.lifecycle_snapshot().state, ProviderState::Stopped);
        assert!(mgr.activate(bus).unwrap() > first);
        mgr.deactivate().await.unwrap();
    }

    #[tokio::test]
    async fn failed_session_cleanup_retains_session_driver_and_typed_evidence() {
        #[derive(Debug)]
        struct RefuseKill;
        impl portable_pty::ChildKiller for RefuseKill {
            fn kill(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "cleanup refused",
                ))
            }
            fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
                Box::new(Self)
            }
        }

        let mgr = PtyManager::cold();
        let bus = Arc::new(GatewayEventBus::new());
        let generation = mgr.activate(bus.clone()).unwrap();
        let sid = mgr
            .spawn(&SpawnOptions {
                created_by: Some("u-cleanup".into()),
                ..Default::default()
            })
            .unwrap()
            .session_id;
        let session = mgr.inner.lock().unwrap().sessions[&sid].clone();
        let original = session.replace_killer_for_test(Box::new(RefuseKill));
        let failure = mgr.request_deactivate().unwrap_err();
        assert_eq!(failure.generation, generation);
        assert!(failure.detail.contains("cleanup refused"));
        let snapshot = mgr.lifecycle_snapshot();
        assert_eq!(snapshot.state, ProviderState::Failed);
        assert_eq!(snapshot.failure, Some(failure.clone()));
        assert!(snapshot.driver_active);
        assert!(mgr.inner.lock().unwrap().sessions.contains_key(&sid));
        assert_eq!(
            mgr.owner_of(&sid),
            SessionOwner::Known(Some("u-cleanup".into()))
        );
        assert!(mgr.activate(bus.clone()).is_err());
        assert!(mgr.write_at(generation, &sid, b"refused").is_err());

        session.replace_killer_for_test(original);
        assert!(
            mgr.deactivate().await.is_err(),
            "failed cleanup stays failed after join"
        );
        assert!(mgr.inner.lock().unwrap().sessions.is_empty());
        assert!(!mgr.lifecycle_snapshot().driver_active);
        assert_eq!(mgr.lifecycle_snapshot().failure, Some(failure));
        assert!(mgr.activate(bus).unwrap() > generation);
        mgr.deactivate().await.unwrap();
    }

    #[test]
    fn activation_without_runtime_retains_typed_start_failure() {
        let mgr = PtyManager::cold();
        let failure = mgr.activate(Arc::new(GatewayEventBus::new())).unwrap_err();
        assert_eq!(failure.generation, 1);
        assert!(failure.detail.contains("Tokio runtime"));
        let snapshot = mgr.lifecycle_snapshot();
        assert_eq!(snapshot.state, ProviderState::Failed);
        assert_eq!(snapshot.failure, Some(failure));
        assert!(!snapshot.driver_active);
        assert!(mgr.current_bus().is_none());
        assert!(mgr.inner.lock().unwrap().sessions.is_empty());
    }

    #[tokio::test]
    async fn cancelled_stop_retains_handle_and_blocks_reactivation_until_join() {
        let mgr = PtyManager::cold();
        let bus = Arc::new(GatewayEventBus::new());
        let first = mgr.activate(bus.clone()).unwrap();
        let sid = mgr.spawn(&SpawnOptions::default()).unwrap().session_id;
        {
            let mut stop = std::pin::pin!(mgr.deactivate());
            assert!(stop
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending());
        }
        let snapshot = mgr.lifecycle_snapshot();
        assert_eq!(snapshot.state, ProviderState::Stopping);
        assert!(snapshot.driver_active);
        assert!(mgr.inner.lock().unwrap().sessions.is_empty());
        assert!(mgr.write_at(first, &sid, b"refused").is_err());
        assert!(mgr.activate(bus.clone()).is_err());
        mgr.deactivate().await.unwrap();
        assert_eq!(mgr.lifecycle_snapshot().state, ProviderState::Stopped);
        assert!(!mgr.lifecycle_snapshot().driver_active);
        assert!(mgr.activate(bus).unwrap() > first);
        mgr.deactivate().await.unwrap();
    }

    #[tokio::test]
    async fn all_old_generation_operation_faces_reject_replacement_sessions() {
        let mgr = PtyManager::cold();
        let bus = Arc::new(GatewayEventBus::new());
        let old = mgr.activate(bus.clone()).unwrap();
        mgr.deactivate().await.unwrap();
        let current = mgr.activate(bus).unwrap();
        let sid = mgr
            .spawn_at(current, &SpawnOptions::default())
            .unwrap()
            .session_id;
        assert!(mgr.spawn_at(old, &SpawnOptions::default()).is_err());
        assert!(mgr.write_at(old, &sid, b"wrong").is_err());
        assert!(mgr.resize_at(old, &sid, 10, 20).is_err());
        assert!(mgr.attach_snapshot_at(old, &sid).is_err());
        assert!(mgr.note_viewport_at(old, &sid, "old", 10, 20).is_err());
        assert!(mgr.close_at(old, &sid).is_err());
        assert_eq!(mgr.list().len(), 1);
        assert!(mgr.effective_size(&sid).is_none());
        mgr.deactivate().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_activation_and_stop_preserve_one_owned_driver() {
        let mgr = PtyManager::cold();
        let bus = Arc::new(GatewayEventBus::new());
        let runtime = tokio::runtime::Handle::current();
        let barrier = std::sync::Barrier::new(3);
        let generations = std::thread::scope(|scope| {
            let launch = || {
                let _entered = runtime.enter();
                barrier.wait();
                mgr.activate(bus.clone()).unwrap()
            };
            let a = scope.spawn(launch);
            let b = scope.spawn(launch);
            barrier.wait();
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_eq!(generations.0, generations.1);
        assert!(mgr.lifecycle_snapshot().driver_active);
        let (a, b) = tokio::join!(mgr.deactivate(), mgr.deactivate());
        assert!(a.is_ok());
        assert!(b.is_ok());
        assert_eq!(mgr.lifecycle_snapshot().state, ProviderState::Stopped);
        assert!(!mgr.lifecycle_snapshot().driver_active);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn activation_racing_disable_retains_one_joinable_driver() {
        let mgr = PtyManager::cold();
        let bus = Arc::new(GatewayEventBus::new());
        let first = mgr.activate(bus.clone()).unwrap();
        let runtime = tokio::runtime::Handle::current();
        let barrier = std::sync::Barrier::new(3);
        let activation = std::thread::scope(|scope| {
            let start = scope.spawn(|| {
                let _entered = runtime.enter();
                barrier.wait();
                mgr.activate(bus.clone())
            });
            let stop = scope.spawn(|| {
                barrier.wait();
                mgr.request_deactivate()
            });
            barrier.wait();
            let activation = start.join().unwrap();
            // A stale stop completion may be rejected if activation already advanced.
            let _ = stop.join().unwrap();
            activation
        });
        if let Ok(generation) = activation {
            assert!(generation >= first);
        }
        assert_ne!(mgr.lifecycle_snapshot().state, ProviderState::Failed);
        mgr.deactivate()
            .await
            .expect("join whichever generation won");
        assert_eq!(mgr.lifecycle_snapshot().state, ProviderState::Stopped);
        assert!(!mgr.lifecycle_snapshot().driver_active);
        assert!(mgr.activate(bus).unwrap() > first);
        mgr.deactivate().await.unwrap();
    }

    #[test]
    fn cold_manager_does_not_create_runtime_effects() {
        let mgr = PtyManager::cold();
        let snapshot = mgr.lifecycle_snapshot();
        assert_eq!(
            snapshot.state,
            crate::capability::provider::ProviderState::Discovered
        );
        assert_eq!(snapshot.generation, 0);
        assert!(!snapshot.driver_active);
        assert!(mgr.list().is_empty());
    }
}
