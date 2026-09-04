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
/// record of this id" (never existed, or aged out of [`OWNER_RETENTION`]),
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

/// The global PTY session registry.
pub struct PtyManager {
    inner: Mutex<Inner>,
    bus: Mutex<Option<Arc<GatewayEventBus>>>,
    /// The current concurrent session ceiling — see [`DEFAULT_MAX_SESSIONS`]
    /// and [`Self::set_max_sessions`].
    max_sessions: AtomicUsize,
}

/// One flush iteration for one session: take the diff frame, probe the
/// foreground process if the gate allows, then sample the agent state off the
/// same screen.
///
/// Extracted from [`PtyManager::start_flush_loop`]'s loop body rather than
/// written inline so the wire between the PTY and `gateway::runtime` has a
/// caller a test can drive. The sampler still starts no clock of its own
/// (判据 §12) — it runs on the flush tick, and only when that tick found
/// something new.
///
/// Two things count as new, and the second one had to be added after the
/// end-to-end guard caught its absence:
/// - the screen produced a frame, the ordinary case; or
/// - the foreground PROGRAM changed with the screen standing still, which is
///   what an agent that starts, paints once and goes quiet looks like. Without
///   this arm the probe identified it and the table never heard.
///
/// The probe itself runs unconditionally, above both. Gated on frames it could
/// never fire rule 3 of [`crate::gateway::pty::foreground::probe_due`] — the
/// recheck for a session that has gone silent — and that rule is the only
/// thing that can notice an agent EXITING. It would also have made
/// `frame_produced` true at the only call site, a predicate that cannot vary
/// (判据 §2).
///
/// The frame is taken first (one screen lock, released), the probe runs
/// holding no screen lock at all, then [`PtySession::with_screen`] takes it
/// again for the length of one sample. The sample therefore sees a screen at
/// least as new as the frame, never older, and the grid is never cloned.
///
/// `now` is unix millis, taken ONCE per tick by the caller and shared with
/// [`crate::gateway::runtime::RuntimeAgents::release_expired`] and
/// [`crate::gateway::runtime::RuntimeAgents::mark_quiet`] — every entry
/// touched in one pass carries the same instant, and the sampler never reads
/// a clock of its own (判据 §12).
///
/// The second element of the return is `RuntimeAgents::sample`'s own
/// `changed` bool (task 6): whether anything OBSERVABLE — state, agent,
/// program, label, cwd, or a quiet mark clearing — differs from the entry's
/// last sample. `start_flush_loop` keys `runtime.agents.changed` on it, so it
/// is surfaced here rather than discarded, exactly like the frame it travels
/// with.
pub(crate) fn flush_session(session: &PtySession, now: i64) -> FlushOutcome {
    let agents = crate::gateway::runtime::agents();
    let frame = session.feed_and_take_frame();
    let program_changed =
        session.maybe_probe_foreground(now, frame.is_some(), agents.agent_known(&session.id));
    // A frame is the usual reason to re-sample; a changed foreground program
    // is the other one, and leaving it out is how an agent that paints once
    // and goes quiet gets identified by the probe and then never published
    // (measured on the first real run of
    // `a_real_agent_started_after_spawn_is_identified`: the probe saw
    // `/bin/sh …/claude` while the table still said `program: "sh",
    // agent: None`, 521 ms stale). The screen lock is taken for that case too,
    // but only when the program actually moved.
    if frame.is_none() && !program_changed {
        return FlushOutcome {
            frame: None,
            agent_changed: false,
        };
    }

    let foreground = session.foreground_fact();
    // The live cwd, in a fixed order of authority, so no reader has to guess
    // which source won (判据 §12).
    //
    // 1. OSC 7 — the shell TELLING us where it is. Stream B adds
    //    `Screen::cwd()`; until it lands there is nothing to read here, and
    //    this comment is the marker Task M attaches it to. It goes FIRST when
    //    it arrives.
    // 2. the foreground process's own cwd, from the probe.
    // 3. the spawn directory, which never changes.
    let cwd = foreground
        .as_ref()
        .and_then(|f| f.cwd.clone())
        .unwrap_or_else(|| session.cwd.clone());

    let changed = session.with_screen(|screen| {
        agents.sample(crate::gateway::runtime::SampleInput {
            session_id: &session.id,
            shell: &session.shell,
            program: foreground.as_ref().map(|f| f.name.as_str()),
            argv0: foreground.as_ref().and_then(|f| f.argv0.as_deref()),
            cmdline: foreground.as_ref().and_then(|f| f.cmdline.as_deref()),
            cwd: &cwd,
            screen,
            process_exited: session.is_closed(),
            // Genuinely varies: the other reason to be here is a
            // foreground-program change with the screen standing still, and
            // only a real frame may end a quiet mark.
            frame_produced: frame.is_some(),
            now,
        })
    });
    FlushOutcome {
        frame,
        agent_changed: changed,
    }
}

/// What one [`flush_session`] pass produced.
///
/// Two independent facts, which is why this is a struct and not the old
/// `Option<(frame, bool)>`: a tick can produce a screen frame, an agent-table
/// change, both, or neither, and the pair `(None, true)` — the foreground
/// program moved while the screen stood still — is exactly the case the old
/// shape could not express.
pub(crate) struct FlushOutcome {
    /// The screen diff to publish on `pty.screen`, when the screen changed.
    pub frame: Option<aleph_protocol::pty::PtyScreenFrame>,
    /// Whether the agent table changed — `RuntimeAgents::sample`'s own
    /// `changed`. `start_flush_loop` folds this across sessions and publishes
    /// at most one `runtime.agents.changed` per tick.
    pub agent_changed: bool,
}

/// Publish `runtime.agents.changed` on `bus` iff `changed`. Payload is
/// deliberately empty (`json!({})`) — the event means "the table changed,
/// re-fetch via `runtime.agents.list`", which is already filtered per
/// caller (R6-3), so the event itself carries no session id or other
/// content to leak (see `event_scope.rs`'s `runtime.` rule doc for why that
/// needs no `session_identity_of` arm).
///
/// The `if` lives HERE, not at each call site: [`PtyManager::start_flush_loop`]
/// folds every session's [`flush_session`] outcome plus
/// [`crate::gateway::runtime::RuntimeAgents::release_expired`]'s non-empty
/// check into ONE `bool` per tick before calling this — N sessions changing
/// in one 16 ms tick still produce at most one event, the same reasoning
/// `release_expired`'s own single-event-per-batch already used, extended
/// across sessions (fix round 1, review Minor 8): a second event tells a
/// subscriber nothing a first one didn't already say. The exit-path call in
/// `session.rs` passes `true` unconditionally — a removed row is always a
/// change.
///
/// One function, one decision, so a test can drive the EXACT decision
/// `start_flush_loop` makes (not just the publish beneath it) without
/// needing the process-global loop itself (that loop cannot be driven from
/// a test — see `the_flush_loop_body_calls_the_sampler_and_the_release`'s
/// doc in `gateway::runtime`).
pub(crate) fn publish_agents_changed_if(changed: bool, bus: &GatewayEventBus) {
    if !changed {
        return;
    }
    let ev = TopicEvent::new(
        aleph_protocol::runtime::RUNTIME_AGENTS_CHANGED_TOPIC,
        serde_json::json!({}),
    );
    let _ = bus.publish(serde_json::to_string(&ev).unwrap_or_default());
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

    /// The current visible screen as plain text (no scrollback) — the same
    /// input [`crate::gateway::runtime::flush_session`]'s sampler reads off
    /// [`super::screen::Screen::visible_text`].
    ///
    /// Deliberately narrower than [`Self::attach_snapshot`]: that response
    /// carries a diff-encoded [`aleph_protocol::pty::PtyScreenPatch`] meant
    /// for a terminal renderer, not a string a model can read. Same posture
    /// as `attach_snapshot` otherwise — an unknown session is an error, never
    /// an empty screen, because a blank grid would read as "the terminal is
    /// idle".
    pub(crate) fn visible_text(&self, session_id: &str) -> Result<String, String> {
        self.with_session(session_id, |s| {
            Ok(s.with_screen(super::screen::Screen::visible_text))
        })
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

    /// Who created a session — the ownership input for every `pty.*` face.
    ///
    /// Answers for sessions that are ALREADY GONE, up to [`OWNER_RETENTION`].
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
            None => Err(no_such_session(session_id)),
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
                let now = chrono::Utc::now().timestamp_millis();
                // Folded across BOTH triggers and every session touched this
                // tick, then handed to `publish_agents_changed_if` ONCE below
                // — not sent on every frame, and not one event per session
                // either (fix round 1, review Minor 8): an event per
                // unchanged frame, or a duplicate for a second session
                // changing in the same tick, is noise clients learn to
                // ignore, and then they ignore the real ones too (R6-4).
                let mut any_agent_changed = false;
                for session in sessions {
                    let outcome = flush_session(&session, now);
                    any_agent_changed |= outcome.agent_changed;
                    let Some(frame) = outcome.frame else {
                        continue;
                    };
                    let Ok(data) = serde_json::to_value(&frame) else {
                        continue;
                    };
                    let ev = TopicEvent::new(aleph_protocol::pty::PTY_SCREEN_TOPIC, data);
                    let _ = bus.publish(serde_json::to_string(&ev).unwrap_or_default());
                }
                // Sessions that went quiet produce no frame, so the loop above
                // cannot reach them — and a finished agent going quiet is
                // exactly what the idle hold is waiting on. This runs on the
                // tick, not on a frame, and it is the SAME tick: no second
                // clock (判据 §12). A held working->idle observation released
                // is folded into the same one-event-per-tick decision (R6-4b).
                any_agent_changed |= !crate::gateway::runtime::agents()
                    .release_expired(now)
                    .is_empty();
                // Same argument, second fact: a session that went quiet
                // produces no frame, so the loop above cannot reach it either.
                // This publishes the SILENCE and never touches `state` — time
                // alone must not turn Working into Idle (spec R2-3).
                any_agent_changed |= !crate::gateway::runtime::agents().mark_quiet(now).is_empty();
                publish_agents_changed_if(any_agent_changed, &bus);
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
}
