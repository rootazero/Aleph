//! A single embedded pseudo-terminal session.
//!
//! Wraps a [`portable_pty`] master/child pair behind a `Send + Sync` handle so
//! the gateway can drive an interactive shell over the existing JSON-RPC /
//! event-bus transport. The blocking PTY reader runs on a dedicated OS thread
//! (it would otherwise pin a tokio blocking-pool slot for the session's whole
//! lifetime); each output chunk is fed into the session's server-held screen
//! (see [`super::screen::Screen`]), which is drained into bounded per-frame
//! diffs published on the `pty.screen` topic so any subscribed operator
//! connection receives them through the normal `events.subscribe` path — no
//! second socket, no second port.
//!
//! This is the Rust mapping of hermes-agent's Python `pty_bridge.py`: where
//! hermes used `ptyprocess` + `select` + a per-connection WebSocket, Aleph uses
//! a cross-platform `portable-pty` abstraction (Unix + Windows `ConPTY`), a typed
//! session handle, and the gateway's topic broadcaster.

use std::io::{Read, Write};

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::json;

use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::sync_primitives::{Arc, AtomicBool, Ordering};

/// Bytes read from the PTY master per syscall. Matches hermes' 64 KiB ceiling
/// scaled down to a single page-friendly chunk; the broadcaster coalesces.
const READ_CHUNK: usize = 8192;

/// Options for spawning a new PTY session. All fields optional — an empty
/// request spawns the user's default login shell at 80x24.
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// Program to run. `None` → the platform default shell
    /// (`$SHELL` / `/bin/sh` on Unix, `cmd.exe` on Windows).
    pub command: Option<String>,
    /// Arguments passed to `command` (ignored when `command` is `None`).
    pub args: Vec<String>,
    /// Working directory for the child.
    pub cwd: Option<String>,
    /// Extra environment variables layered on top of the inherited env.
    pub env: Vec<(String, String)>,
    /// Initial terminal rows (default 24).
    pub rows: u16,
    /// Initial terminal columns (default 80).
    pub cols: u16,
    /// Scrollback ceiling for this session's ring, from
    /// `[policies.terminal] scrollback_lines`. `None` keeps the grid's
    /// built-in default.
    ///
    /// It lives here, and not in a `PtyManager::spawn_with_scrollback`
    /// wrapper that sets it after the fact, for two reasons. It closes a
    /// window: `spawn` starts the reader thread before any caller could
    /// reach back in, so a post-hoc setter races the child's first output.
    /// And it removes a second spawn path: the wrapper it replaces
    /// (`PtyManager::spawn_with_scrollback`) looked the session up AGAIN
    /// after `spawn` returned and propagated the lookup error, so a child
    /// already reaped by its own reader thread (`spawn_reader` calls
    /// `manager().remove` at EOF) would have been reported as a spawn
    /// FAILURE for a process that in fact ran.
    ///
    /// ⚠️ Honest about the strength of that second reason: it is real by
    /// construction and unreachable in practice. Restoring the wrapper's
    /// shape and spawning `sh -c "exit 0"` twenty times in a row did NOT
    /// reproduce it — thread start-up plus process teardown is orders of
    /// magnitude longer than the gap between the insert and the second
    /// lookup. A test asserting it stayed green under that mutation, so it
    /// was deleted rather than kept: a guard that cannot go red advertises
    /// coverage it does not have. What remains is the first reason and one
    /// path instead of two.
    pub scrollback_lines: Option<usize>,
    /// Who asked for this session, for `pty.list`'s accountability column.
    /// `None` = not attributable (a spawn that did not come through a
    /// caller-identified face). Carried here rather than through a
    /// `spawn_as` wrapper for the same reason `scrollback_lines` is — see
    /// that field's doc.
    ///
    /// Sourced from `visibility::ambient_actor()`, which on `pty.spawn`'s one
    /// production call site (a bare RPC, never a run under a `TurnContext`)
    /// degenerates to `ambient_owner()` — a person, always. That is currently
    /// moot, not a settled design: `ambient_actor()` has a third arm reading
    /// an AGENT id (`turn_context::current_agent_id()`), so if this face is
    /// ever reached from inside a run (a future spawn-a-terminal tool, say),
    /// this column starts holding two kinds of identity under a doc that
    /// says "who asked". Worth re-deciding at that point, not before.
    pub created_by: Option<String>,
}

/// A live PTY session handle. Cloneable via `Arc`; all mutating operations are
/// internally synchronized so the handler layer can stay stateless.
pub struct PtySession {
    /// Opaque session id (uuid v4).
    pub id: String,
    /// Human-readable shell/command label (for `pty.list`).
    pub shell: String,
    /// Unix-epoch seconds at spawn time.
    pub created_at: i64,
    /// Who asked for this session — see [`SpawnOptions::created_by`].
    pub created_by: Option<String>,
    /// The directory the child was SPAWNED in ([`SpawnOptions::cwd`]), or
    /// empty when the spawn inherited the server's.
    ///
    /// NOT the live cwd: a shell that has since `cd`'d is not tracked,
    /// because that needs PID probing (a phase 0-A gap). Retained rather than
    /// dropped after `CommandBuilder` because `RuntimeAgentEntry.cwd` has no
    /// other producer, and a field that is empty for every session is a
    /// predicate that cannot vary (判据 §2).
    pub cwd: String,
    /// Writer into the child's stdin (the PTY master write side).
    writer: crate::sync_primitives::Mutex<Box<dyn Write + Send>>,
    /// Master handle, retained for resize (`TIOCSWINSZ` equivalent).
    ///
    /// `None` once the child has exited AND its reader was still parked when
    /// [`settle_exit`] looked — never for a live session, and not on the
    /// platforms that end the read on their own.
    ///
    /// That conditional take is not tidiness, it is the only lever that ends
    /// a parked reader: ConPTY does not close the pseudoconsole's output pipe
    /// when the child dies, so the reader stays blocked in `read` until the
    /// master is DROPPED. Measured on this machine 2026-09-05 — the reader
    /// was still blocked 3 s after the child exited, and unblocked with EOF
    /// **1.9 ms** after the master went away.
    ///
    /// Every reader of this field must therefore answer "the session is
    /// over", never "resize failed": `closed` is set before the take, so the
    /// `None` arm is unreachable for a live session and its message says
    /// exactly that.
    master: crate::sync_primitives::Mutex<Option<Box<dyn MasterPty + Send>>>,
    /// Independent killer split from the child so `close` can terminate it
    /// without racing the reader thread that owns the `Child`.
    killer: crate::sync_primitives::Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// Flipped once the child exits or the session is closed.
    closed: AtomicBool,
    /// The server-held screen. Fed by the reader thread, drained by the flush
    /// task. A `Mutex` rather than a channel because both halves want the
    /// latest state, not every intermediate one.
    screen: crate::sync_primitives::Mutex<super::screen::Screen>,
    /// Monotonic per-session frame counter. Advances only when a frame is
    /// actually published, so a client's gap detection means what it says.
    seq: crate::sync_primitives::Mutex<u64>,
    /// The pid of the process this session spawned — the shell, normally.
    ///
    /// `None` when `portable-pty` would not say (it returns `Option`).
    /// Captured at spawn because the reader thread takes ownership of the
    /// `Child` immediately afterwards and is the only other holder of this
    /// fact; asking it later would need a channel for something that never
    /// changes.
    shell_pid: Option<u32>,
    /// The foreground-process probe's bookkeeping. Its own lock, held for
    /// microseconds and NEVER around the `sysinfo` refresh — see
    /// [`super::foreground`]'s Locks section.
    foreground: crate::sync_primitives::Mutex<super::foreground::ForegroundState>,
}

impl PtySession {
    /// Spawn `opts` behind a fresh PTY and start streaming its output to `bus`
    /// (when attached). Returns the shared session handle; the reader thread
    /// owns the child and self-removes from the manager on exit.
    pub fn spawn(
        id: String,
        opts: &SpawnOptions,
        bus: Option<Arc<GatewayEventBus>>,
    ) -> Result<Arc<Self>, String> {
        let rows = if opts.rows == 0 { 24 } else { opts.rows };
        let cols = if opts.cols == 0 { 80 } else { opts.cols };
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(size)
            .map_err(|e| format!("openpty failed: {e}"))?;

        // Build the command (explicit program or the platform default shell).
        let (mut cmd, label) = match &opts.command {
            Some(prog) => (CommandBuilder::new(prog), prog.clone()),
            None => (CommandBuilder::new_default_prog(), default_shell_label()),
        };
        if opts.command.is_some() {
            cmd.args(&opts.args);
        }
        if let Some(cwd) = &opts.cwd {
            cmd.cwd(cwd);
        }
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        // Give curses/full-screen apps a sane terminal type, mirroring hermes.
        cmd.env("TERM", "xterm-256color");

        let portable_pty::PtyPair { master, slave } = pair;
        let child = slave
            .spawn_command(cmd)
            .map_err(|e| format!("spawn_command failed: {e}"))?;
        // Drop the slave so the kernel propagates EOF to the master read side
        // once the child closes its descriptors.
        drop(slave);

        let killer = child.clone_killer();
        // Read BEFORE `spawn_reader` moves the child onto the reader thread.
        let shell_pid = child.process_id();
        let reader = master
            .try_clone_reader()
            .map_err(|e| format!("clone_reader failed: {e}"))?;
        let writer = master
            .take_writer()
            .map_err(|e| format!("take_writer failed: {e}"))?;

        // Built and bounded BEFORE `spawn_reader` below, so the ceiling is in
        // force for the child's very first byte. See `SpawnOptions::scrollback_lines`.
        let mut screen = super::screen::Screen::new(rows, cols);
        if let Some(lines) = opts.scrollback_lines {
            screen.set_scrollback_limit(lines);
        }

        let session = Arc::new(Self {
            id: id.clone(),
            shell: label,
            cwd: opts.cwd.clone().unwrap_or_default(),
            created_at: chrono::Utc::now().timestamp(),
            created_by: opts.created_by.clone(),
            writer: crate::sync_primitives::Mutex::new(writer),
            master: crate::sync_primitives::Mutex::new(Some(master)),
            killer: crate::sync_primitives::Mutex::new(killer),
            closed: AtomicBool::new(false),
            screen: crate::sync_primitives::Mutex::new(screen),
            seq: crate::sync_primitives::Mutex::new(0),
            shell_pid,
            foreground: crate::sync_primitives::Mutex::default(),
        });

        spawn_reader(session.clone(), reader, child, bus);
        Ok(session)
    }

    /// Write raw bytes to the child's stdin.
    pub fn write_input(&self, data: &[u8]) -> Result<(), String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("session closed".into());
        }
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(data)
            .map_err(|e| format!("write failed: {e}"))?;
        w.flush().map_err(|e| format!("flush failed: {e}"))
    }

    /// Resize the terminal window (forwarded to the kernel via the master).
    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("session closed".into());
        }
        let guard = self.master.lock().unwrap_or_else(|e| e.into_inner());
        let m = guard.as_ref().ok_or("session closed")?;
        m.resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("resize failed: {e}"))?;
        drop(guard);
        self.screen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .resize(rows.max(1), cols.max(1));
        Ok(())
    }

    /// Terminate the child process. Idempotent — the reader thread also marks
    /// the session closed on EOF.
    pub fn kill(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut k = self.killer.lock().unwrap_or_else(|e| e.into_inner());
        let _ = k.kill();
    }

    /// Whether the child has exited or the session was closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    // A `pub fn shell_pid(&self) -> Option<u32>` accessor lived here with zero
    // callers crate-wide (R10 / YAGNI — CUT 2026-09-04). Its doc named "the
    // only consumer, the non-Unix foreground heuristic", and that consumer
    // reads the FIELD directly in `maybe_probe_foreground` below — so the
    // accessor was both dead and describing someone else's call site (判据 §1).
    // Re-add it when a reader outside this type exists, not before.

    /// Probe the terminal's foreground process, if this tick is due. Returns
    /// whether the believed foreground process CHANGED.
    ///
    /// `frame_produced` and `agent_known` are the gate's two inputs; see
    /// [`super::foreground::probe_due`] for the three rules. Calling this on
    /// every tick — not only on ticks that produced a frame — is what makes
    /// the recheck rule reachable, and the recheck rule is the only thing
    /// that can notice an agent EXITING.
    ///
    /// The return value is what lets `flush_session` re-sample a session whose
    /// screen did not change but whose foreground program did; without it, a
    /// program that starts, paints once and goes quiet is identified here and
    /// never published.
    ///
    /// # Locks
    ///
    /// Three acquisitions, none overlapping, and the screen lock is not among
    /// them:
    /// 1. the foreground lock, to ask the gate;
    /// 2. the master lock, inside [`Self::terminal_leader`], for one
    ///    `tcgetpgrp` ioctl and nothing else;
    /// 3. the foreground lock again, to fold the outcome in.
    ///
    /// **Every process-table read happens between 2 and 3, holding nothing** —
    /// both the descendant walk (a full refresh) and the single-pid read. An
    /// earlier version composed the walk into the locked step, so on Windows
    /// every probe scanned the whole process table while `PtySession::resize`
    /// waited behind the same lock. `terminal_leader` exists to make that
    /// impossible to reintroduce by accident, and
    /// `foreground::tests::no_process_table_read_happens_under_the_master_lock`
    /// pins it.
    pub fn maybe_probe_foreground(
        &self,
        now: i64,
        frame_produced: bool,
        agent_known: bool,
    ) -> bool {
        let due = {
            let mut state = self.foreground.lock().unwrap_or_else(|e| e.into_inner());
            // Remember the frame even when the gate is about to say no: a
            // frame that lands inside the rate limit's shadow still means the
            // screen changed, and dropping it is how a program that paints
            // once and goes quiet stays unidentified forever (see
            // `foreground::probe_due`).
            state.note_frame(frame_produced);
            super::foreground::probe_due(
                state.last_probe_at(),
                now,
                state.frame_budget_left(),
                agent_known,
            )
        };
        if !due {
            return false;
        }
        // The master lock is taken and released inside `terminal_leader`.
        // Everything below this line reads the process table and holds
        // nothing.
        let observed = match self.terminal_leader() {
            Some(leader) => super::foreground::fact_for_pid(leader),
            // The terminal would not say. On Windows that is EVERY probe.
            None => self
                .shell_pid
                .and_then(super::foreground::foreground_fact_for_shell),
        };
        self.foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .observe(now, observed)
    }

    /// The pgid the terminal itself reports, under the master lock.
    ///
    /// The whole body is the lock plus one ioctl, and it is a separate
    /// function so that "what may run under the master lock" is a question
    /// with a one-line answer rather than a promise in a comment. Anything
    /// that reads the process table goes in the caller, after this returns.
    ///
    /// A settled session has no master, and the `None` that produces means
    /// what every other `None` here means — "I could not look" (判据 §8), not
    /// "nothing is running there". The caller's fallback answers next, which
    /// is the same thing that happens on Windows on every probe.
    fn terminal_leader(&self) -> Option<u32> {
        let master = self.master.lock().unwrap_or_else(|e| e.into_inner());
        super::foreground::leader_from_terminal(&**master.as_ref()?)
    }

    /// How many probes this session has performed — the cost guard's
    /// instrument. See [`super::foreground::ForegroundState::probes`].
    #[must_use]
    pub fn probe_count(&self) -> u64 {
        self.foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .probes()
    }

    /// The foreground process this session is currently believed to be
    /// running, after the miss hysteresis.
    ///
    /// `None` means the probe has not answered — never that the shell is what
    /// is running. Those are different facts and the wire keeps them apart
    /// (`RuntimeAgentEntry::program`'s doc).
    #[must_use]
    pub fn foreground_fact(&self) -> Option<super::foreground::ForegroundFact> {
        self.foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .current()
            .cloned()
    }

    /// The diff since the last call, already in wire form, with a fresh `seq`.
    /// `None` when nothing changed — that is what makes a quiet terminal free.
    pub fn feed_and_take_frame(&self) -> Option<aleph_protocol::pty::PtyScreenFrame> {
        let (patch, rows, cols) = {
            let mut screen = self.screen.lock().unwrap_or_else(|e| e.into_inner());
            let patch = screen.take_patch()?;
            let (rows, cols) = screen.grid.dims();
            (patch, rows, cols)
        };
        let seq = {
            let mut s = self.seq.lock().unwrap_or_else(|e| e.into_inner());
            *s += 1;
            *s
        };
        Some(aleph_protocol::pty::PtyScreenFrame {
            session_id: self.id.clone(),
            seq,
            rows,
            cols,
            patch: super::screen::convert::patch(&patch),
        })
    }

    /// Run `f` against the server-held screen under ONE lock acquisition.
    ///
    /// The agent sampler needs the visible text *and* the OSC title from the
    /// same screen; two accessors would be two acquisitions and two
    /// observations of a screen the reader thread mutates in between. Handing
    /// the borrow out instead of returning `(String, Option<String>)` also
    /// keeps the title read inside `RuntimeAgents::sample`, which is the line
    /// the falsification guard has to be able to cut.
    pub(crate) fn with_screen<R>(&self, f: impl FnOnce(&super::screen::Screen) -> R) -> R {
        f(&self.screen.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Override the scrollback ceiling for this session. Called at spawn
    /// from `[policies.terminal] scrollback_lines`; without this the field
    /// would be settable and inert (`Grid::set_scrollback_limit`'s doc).
    pub fn set_scrollback_limit(&self, lines: usize) {
        self.screen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_scrollback_limit(lines);
    }

    /// The scrollback ceiling currently in effect — read back by tests and
    /// diagnostics to confirm a configured value actually reached the grid.
    #[must_use]
    pub fn scrollback_limit(&self) -> usize {
        self.screen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .scrollback_limit()
    }

    /// One snapshot for `pty.attach`: the whole screen plus the seq it was
    /// taken at, so the client knows which live frames to discard.
    pub fn attach_snapshot(&self) -> aleph_protocol::pty::PtyAttachResponse {
        let screen = self.screen.lock().unwrap_or_else(|e| e.into_inner());
        let (rows, cols) = screen.grid.dims();
        let seq = *self.seq.lock().unwrap_or_else(|e| e.into_inner());
        aleph_protocol::pty::PtyAttachResponse {
            seq,
            rows,
            cols,
            patch: super::screen::convert::patch(&screen.full_patch()),
            scrollback_len: screen.grid.scrollback_len(),
        }
    }
}

/// The platform default shell label for display purposes only (the actual
/// spawn uses `CommandBuilder::new_default_prog`).
fn default_shell_label() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

/// Start a session's two OS threads: one that feeds the screen, and one that
/// waits for the child and settles the session when it exits.
///
/// # Why two, and why the settle is on the WAITER
///
/// "The child exited" and "the terminal reached EOF" are two facts, and this
/// used to derive the first from the second: one thread read until `Ok(0)`,
/// and only then ran `child.wait()` and everything downstream of it. On Unix
/// that works, because the pty master reports EOF when the last slave fd
/// closes. **On Windows it never fires at all**: ConPTY does not close the
/// pseudoconsole's output pipe when the child dies, so the reader stayed
/// blocked in `read` forever and `pty.exit`, `manager().remove` and
/// `runtime::agents().remove` never ran — every terminal whose program
/// exited stayed listed for the life of the process, and `manager.rs`'s
/// `owner_of` already spelled out the consequence: "a client that never
/// learns its shell died shows a live terminal forever." Measured before the
/// fix: `pty.exit` after a child that exits in ~2 s arrived **never**, not
/// late (20 s budget). One fact, one deriver (判据 §6) — and the deriver has
/// to be the one the platform actually supplies.
///
/// Both halves of the current shape were measured on this machine on
/// 2026-09-05 rather than assumed about ConPTY:
///
/// * `child.wait()` returns **promptly** on Windows — 2.07 s for a child that
///   exits at ~2 s. So waiting on the child is a real signal there even
///   though EOF is not.
/// * the reader is still blocked 3 s after the child exits, and unblocks with
///   EOF **1.9 ms** after the master is dropped. So [`settle_exit`] taking
///   the master out of the session is not tidiness — it is the only thing
///   that ends a parked reader thread and lets the session's screen be
///   freed. It does that only for a reader that is still parked after
///   [`READER_DRAIN_GRACE`]; where EOF works, nothing is taken and the
///   sequence is byte-for-byte what it always was.
///
/// # Exactly once
///
/// The reader no longer settles anything; it only feeds and exits. So there
/// is one settle site and it needs no latch. A child that closes its fds
/// without exiting (EOF but still running) used to block in `child.wait()`
/// anyway, so nothing regresses: the reader leaves, the waiter stays until
/// the process really is gone.
///
/// The cost is one extra OS thread per session, parked in `child.wait()` and
/// gone the moment the child is. Named because it is a real cost: PTY
/// sessions are terminals a person opened, not a pool.
fn spawn_reader(
    session: Arc<PtySession>,
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    bus: Option<Arc<GatewayEventBus>>,
) {
    // Never sent on — the reader's exit is signalled by DROPPING the sender,
    // so the waiter's `recv_timeout` tells "reader gone" (`Disconnected`)
    // from "reader still parked" (`Timeout`) without either side polling.
    let (reader_done, reader_gone) = std::sync::mpsc::channel::<()>();
    let reading = Arc::clone(&session);
    std::thread::Builder::new()
        .name(format!("pty-reader-{}", session.id))
        .spawn(move || {
            let _reader_done = reader_done;
            let mut buf = [0u8; READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF: the terminal is gone
                    Ok(n) => {
                        reading
                            .screen
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .feed(&buf[..n]);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
        .ok();

    std::thread::Builder::new()
        .name(format!("pty-waiter-{}", session.id))
        .spawn(move || {
            let exit_code = child.wait().map_or(0, |s| s.exit_code());
            settle_exit(&session, exit_code, bus.as_ref(), &reader_gone);
        })
        .ok();
}

/// How long a settling session waits for its reader to drain and end BEFORE
/// reaching for the master.
///
/// This window is the whole reason the fix is not a regression on the
/// platforms that already worked. `child.wait()` returns the moment the child
/// dies, which is EARLIER than "the reader has fed the child's last bytes
/// into the screen" — and the settle removes the session from the manager,
/// which is what stops frames being published at all. Announcing first would
/// therefore drop the tail of a program's output on Unix, where nothing was
/// broken. So the waiter yields to the reader first, and only a reader that
/// is still parked after this gets the master pulled out from under it.
///
/// Generous on purpose: on Unix the reader is already unblocked when the
/// child exits and this costs microseconds, so the number only has to be
/// larger than "drain a pipe buffer through an 8 KiB loop", which it is by
/// orders of magnitude. On a platform that never EOFs it is paid once per
/// session exit, and half a second of latency on a terminal disappearing is
/// not a thing anyone can see.
const READER_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// How long to then wait for the reader to notice the master is gone.
/// Measured at 1.9 ms on Windows (see [`spawn_reader`]); this is that number
/// with room, not a guess, and missing it only costs the thread its exit —
/// the announcements below happen either way.
const READER_UNBLOCK_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// Everything that must happen exactly once when a session's child exits.
///
/// Ordering is load-bearing and reads top to bottom:
///
/// 1. `closed` first, so nothing accepts new input for a dead child and the
///    `master.take()` below cannot be mistaken for a resize failure.
/// 2. Give the reader [`READER_DRAIN_GRACE`] to finish and end on its own.
///    On every platform that reports EOF this is where the story stops: the
///    reader is already gone, the master is never touched, and the tail of
///    the child's output reached the screen before anything below ran. That
///    is the pre-existing behaviour, preserved deliberately.
/// 3. Only if the reader is STILL parked — the ConPTY case, and equally a
///    Unix session whose slave fd a grandchild is holding open — take the
///    master. That is the lever that ends the read (~2 ms, measured), and it
///    is applied as a remedy for a terminal that demonstrably did not EOF,
///    not as routine teardown.
/// 4. `pty.exit`, then the manager removal. The manager keeps the last
///    `OWNER_RETENTION` sessions' owners precisely so this frame can still be
///    addressed to the client whose shell just died — see `owner_of`.
/// 5. The runtime row, then its change edge.
fn settle_exit(
    session: &Arc<PtySession>,
    exit_code: u32,
    bus: Option<&Arc<GatewayEventBus>>,
    reader_gone: &std::sync::mpsc::Receiver<()>,
) {
    use std::sync::mpsc::RecvTimeoutError;

    session.closed.store(true, Ordering::SeqCst);
    // Nothing is ever sent, so `Ok` is unreachable and `Disconnected` is the
    // reader's goodbye. Only `Timeout` means "still parked".
    if matches!(
        reader_gone.recv_timeout(READER_DRAIN_GRACE),
        Err(RecvTimeoutError::Timeout)
    ) {
        // Taken under the lock, DROPPED outside it. The destructor is
        // `ClosePseudoConsole` (or the platform's equivalent) and it is the
        // one call here that can take its time; running it under the master
        // lock would park `maybe_probe_foreground`'s `terminal_leader` on the
        // flush thread behind a teardown, which is the shape this module's
        // Locks doctrine exists to keep out.
        let taken = session
            .master
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        drop(taken);
        let _ = reader_gone.recv_timeout(READER_UNBLOCK_GRACE);
    }
    if let Some(bus) = bus {
        let ev = TopicEvent::new(
            aleph_protocol::pty::PTY_EXIT_TOPIC,
            json!({ "session_id": session.id, "exit_code": exit_code }),
        );
        let _ = bus.publish(serde_json::to_string(&ev).unwrap_or_default());
    }
    super::manager().remove(&session.id);
    // Spec §5: the PTY session is gone, so its agent entry is gone.
    // Here and not as a prune inside `RuntimeAgents::snapshot` —
    // two mechanisms would be two answers to "is this session
    // alive" (判据 §6), and only an explicit removal gives task 6 an
    // edge to emit `runtime.agents.changed` on.
    crate::gateway::runtime::agents().remove(&session.id);
    // Task 6: the edge above IS the change — a removed row is
    // exactly what `runtime.agents.list`'s next fetch will no longer
    // show. Same helper `start_flush_loop` uses, called with `true`
    // unconditionally: a removed row is always a change.
    if let Some(bus) = bus {
        super::manager::publish_agents_changed_if(true, bus);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A child that exits must settle the session — on a platform whose
    /// terminal never reports EOF.
    ///
    /// This is the guard for the defect described on [`spawn_reader`]: the
    /// settle used to hang off the read loop breaking, and on Windows that
    /// break never comes, so `pty.exit` was published **never** (measured:
    /// not once in 20 s for a child that exits in ~2 s). It asserts the two
    /// halves separately because they fail separately:
    ///
    /// 1. **`pty.exit` is published.** Red on the old shape on Windows, green
    ///    on Unix — the asymmetry is the point, and it is why this could sit
    ///    broken behind a green suite.
    /// 2. **The session is not still held by a thread.** `settle_exit` takes
    ///    the master, the blocked reader gets EOF ~2 ms later and drops its
    ///    `Arc`, so the count falls back to this test's own. Delete the take
    ///    and assertion 1 still passes while the reader thread — and the
    ///    session's whole screen and scrollback — leaks for the life of the
    ///    process. A guard on the announcement alone would not see that.
    ///
    /// Both halves have been shown to go red, and by DIFFERENT mutations —
    /// which is what says they are two guards and not one written twice
    /// (measured 2026-09-05): delete the take and this one reports
    /// `pty.exit at 3.13s, strong_count=2` — assertion 1 green, assertion 2
    /// red; make the take unconditional and this one stays green while
    /// `settle_leaves_the_master_alone_when_the_reader_already_ended` is the
    /// one that fails.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn a_child_that_exits_settles_the_session_without_needing_terminal_eof() {
        let bus =
            crate::sync_primitives::Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
        let mut rx = bus.subscribe();
        let opts = SpawnOptions {
            command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
            args: if cfg!(windows) {
                vec!["/c".into(), "echo BYE & ping -n 3 127.0.0.1 >nul".into()]
            } else {
                vec!["-c".into(), "printf BYE; sleep 1".into()]
            },
            rows: 6,
            cols: 40,
            ..Default::default()
        };
        let id = "t-settle-without-eof";
        let t0 = std::time::Instant::now();
        let session = PtySession::spawn(id.into(), &opts, Some(bus.clone())).expect("spawn");

        let mut exit_at: Option<std::time::Duration> = None;
        for _ in 0..1500 {
            while let Ok(raw) = rx.try_recv() {
                if raw.contains(aleph_protocol::pty::PTY_EXIT_TOPIC) && raw.contains(id) {
                    exit_at = Some(t0.elapsed());
                    break;
                }
            }
            if exit_at.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            exit_at.is_some(),
            "a child that exited must publish {} within 15s -- deriving \"the child \
             exited\" from \"the terminal reached EOF\" makes this unreachable on any \
             platform that does not supply the second fact (判据 §6)",
            aleph_protocol::pty::PTY_EXIT_TOPIC
        );

        // The reader is unblocked by the master being taken, not by the exit
        // announcement, so give it its own window rather than folding it into
        // the one above.
        let mut released = false;
        for _ in 0..500 {
            if crate::sync_primitives::Arc::strong_count(&session) == 1 {
                released = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        eprintln!(
            "settle: pty.exit at {exit_at:?}, strong_count={}",
            crate::sync_primitives::Arc::strong_count(&session)
        );
        super::super::manager().remove(id);
        crate::gateway::runtime::agents().remove(id);
        assert!(
            released,
            "no thread may still hold the session after it settles: the reader is \
             parked in a read that this platform may never end on its own, and the \
             lever that ends it is `settle_exit` taking the master. Still held by {} \
             other handle(s)",
            crate::sync_primitives::Arc::strong_count(&session) - 1
        );
    }

    /// The OTHER arm of [`settle_exit`], pinned on a machine that cannot
    /// reach it by running a program.
    ///
    /// On Windows the reader is ALWAYS still parked when the child exits, so
    /// the "reader already ended" branch — the one that leaves the master
    /// alone and so keeps the tail of a program's output reaching the screen
    /// on every platform that reports EOF — is unreachable here by any
    /// command. Leaving it to "the platform that has EOF will cover it" is
    /// the exact shape this round was sent to fix: a branch no machine in the
    /// loop executes is a branch nobody can falsify. So it is driven
    /// directly, with a sender dropped up front standing in for a reader that
    /// has already gone.
    ///
    /// It goes red the moment the take stops being conditional — which is the
    /// change that would silently truncate a child's last output everywhere
    /// the old code worked.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn settle_leaves_the_master_alone_when_the_reader_already_ended() {
        let opts = SpawnOptions {
            command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
            args: if cfg!(windows) {
                vec!["/c".into(), "ping -n 31 127.0.0.1 >nul".into()]
            } else {
                vec!["-c".into(), "sleep 30".into()]
            },
            rows: 6,
            cols: 40,
            ..Default::default()
        };
        let id = "t-settle-eof-arm";
        let session = PtySession::spawn(id.into(), &opts, None).expect("spawn");

        // A reader that has already ended: nothing is ever sent on this
        // channel, so dropping the sender is exactly the goodbye the real
        // reader thread gives.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        drop(tx);
        let t0 = std::time::Instant::now();
        settle_exit(&session, 0, None, &rx);
        let took = t0.elapsed();

        let master_kept = session
            .master
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        session.kill();
        super::super::manager().remove(id);
        crate::gateway::runtime::agents().remove(id);

        assert!(
            master_kept,
            "a reader that has already ended needs no lever: taking the master here \
             would close the terminal out from under a drain that had not finished, \
             which is how a program's last lines get lost on every platform that \
             does report EOF"
        );
        assert!(
            took < READER_DRAIN_GRACE,
            "a goodbye from the reader must be waited on, not slept through: \
             `Disconnected` has to return at once or every session exit pays \
             {READER_DRAIN_GRACE:?} of latency it does not owe (took {took:?})"
        );
    }

    /// End-to-end through a real PTY: bytes a child writes must reach the
    /// server's screen, and the snapshot must show them.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_child_write_reaches_the_server_held_screen() {
        let opts = SpawnOptions {
            command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
            args: if cfg!(windows) {
                vec!["/C".into(), "echo ALEPH_SCREEN_OK".into()]
            } else {
                vec!["-c".into(), "printf 'ALEPH_SCREEN_OK'".into()]
            },
            rows: 10,
            cols: 40,
            ..Default::default()
        };
        let session = PtySession::spawn("t-screen".into(), &opts, None).expect("spawn");

        // The reader thread feeds the screen; poll the snapshot rather than
        // sleeping a fixed amount, so a slow machine does not flake.
        let mut found = false;
        for _ in 0..100 {
            let snap = session.attach_snapshot();
            if snap.patch.rows.iter().any(|r| {
                r.runs
                    .iter()
                    .any(|run| run.text.contains("ALEPH_SCREEN_OK"))
            }) {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            found,
            "the child's output must appear on the server-held screen"
        );
        session.kill();
    }

    /// seq must advance only when a frame is actually produced, otherwise a
    /// client's gap detection fires on frames that were never sent.
    #[tokio::test(flavor = "multi_thread")]
    async fn seq_advances_only_when_a_frame_is_produced() {
        let opts = SpawnOptions {
            rows: 5,
            cols: 20,
            ..Default::default()
        };
        let session = PtySession::spawn("t-seq".into(), &opts, None).expect("spawn");
        // Drain whatever the shell printed at startup.
        for _ in 0..20 {
            if session.feed_and_take_frame().is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let before = session.attach_snapshot().seq;
        assert!(
            session.feed_and_take_frame().is_none(),
            "a quiet screen yields no frame"
        );
        assert_eq!(
            session.attach_snapshot().seq,
            before,
            "a no-op must not burn a seq"
        );
        session.kill();
    }
}
