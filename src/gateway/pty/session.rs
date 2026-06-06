//! A single embedded pseudo-terminal session.
//!
//! Wraps a [`portable_pty`] master/child pair behind a `Send + Sync` handle so
//! the gateway can drive an interactive shell over the existing JSON-RPC /
//! event-bus transport. The blocking PTY reader runs on a dedicated OS thread
//! (it would otherwise pin a tokio blocking-pool slot for the session's whole
//! lifetime); each output chunk is base64-framed and republished on the
//! `pty.output` topic so any subscribed operator connection receives it through
//! the normal `events.subscribe` path — no second socket, no second port.
//!
//! This is the Rust mapping of hermes-agent's Python `pty_bridge.py`: where
//! hermes used `ptyprocess` + `select` + a per-connection WebSocket, Aleph uses
//! a cross-platform `portable-pty` abstraction (Unix + Windows ConPTY), a typed
//! session handle, and the gateway's topic broadcaster.

use std::io::{Read, Write};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
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
    /// Writer into the child's stdin (the PTY master write side).
    writer: std::sync::Mutex<Box<dyn Write + Send>>,
    /// Master handle, retained for resize (`TIOCSWINSZ` equivalent).
    master: std::sync::Mutex<Box<dyn MasterPty + Send>>,
    /// Independent killer split from the child so `close` can terminate it
    /// without racing the reader thread that owns the `Child`.
    killer: std::sync::Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// Flipped once the child exits or the session is closed.
    closed: AtomicBool,
}

impl PtySession {
    /// Spawn `opts` behind a fresh PTY and start streaming its output to `bus`
    /// (when attached). Returns the shared session handle; the reader thread
    /// owns the child and self-removes from the manager on exit.
    pub fn spawn(
        id: String,
        opts: &SpawnOptions,
        bus: Option<Arc<GatewayEventBus>>,
    ) -> Result<Arc<PtySession>, String> {
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
        let reader = master
            .try_clone_reader()
            .map_err(|e| format!("clone_reader failed: {e}"))?;
        let writer = master
            .take_writer()
            .map_err(|e| format!("take_writer failed: {e}"))?;

        let session = Arc::new(PtySession {
            id: id.clone(),
            shell: label,
            created_at: chrono::Utc::now().timestamp(),
            writer: std::sync::Mutex::new(writer),
            master: std::sync::Mutex::new(master),
            killer: std::sync::Mutex::new(killer),
            closed: AtomicBool::new(false),
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
        let m = self.master.lock().unwrap_or_else(|e| e.into_inner());
        m.resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("resize failed: {e}"))
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

/// Start the dedicated reader thread for a session. Pumps master output to the
/// `pty.output` topic, then waits for child exit and emits `pty.exit` before
/// removing the session from the manager.
fn spawn_reader(
    session: Arc<PtySession>,
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    bus: Option<Arc<GatewayEventBus>>,
) {
    std::thread::Builder::new()
        .name(format!("pty-reader-{}", session.id))
        .spawn(move || {
            let mut buf = [0u8; READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF: child closed the PTY
                    Ok(n) => {
                        if let Some(bus) = &bus {
                            let encoded = BASE64.encode(&buf[..n]);
                            let ev = TopicEvent::new(
                                "pty.output",
                                json!({ "session_id": session.id, "data": encoded }),
                            );
                            let _ = bus.publish(serde_json::to_string(&ev).unwrap_or_default());
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }

            let exit_code = child.wait().map(|s| s.exit_code()).unwrap_or(0);
            session.closed.store(true, Ordering::SeqCst);
            if let Some(bus) = &bus {
                let ev = TopicEvent::new(
                    "pty.exit",
                    json!({ "session_id": session.id, "exit_code": exit_code }),
                );
                let _ = bus.publish(serde_json::to_string(&ev).unwrap_or_default());
            }
            super::manager().remove(&session.id);
        })
        .ok();
}
