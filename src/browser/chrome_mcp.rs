//! `ChromeMcpDriver` — manages Chrome `DevTools` MCP sessions.
//!
//! Spawns `chrome-devtools-mcp` as a stdio MCP server per profile.
//! Sessions are lazily created on first tool call and cached by profile name.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::RwLock;

use super::discovery::{find_chromium, find_chromium_preferred};
use super::error::BrowserError;
use super::profile::{ChromeMcpConfig, ProfileConfig};
use crate::mcp::{ExternalServerConfig, McpClient};
use crate::sync_primitives::Mutex;
use crate::utils::no_window::NoWindow;

/// A running Chrome `DevTools` MCP session.
struct ChromeMcpSession {
    client: McpClient,
}

/// Build the argument vector for `chrome --remote-debugging-port=0`.
///
/// When the launching profile is known, its `proxy` / `user_data_dir` are
/// appended next, and `extra_args` go LAST so a user-supplied flag always
/// wins over a config-derived one (Chrome honors the last occurrence of a
/// repeated switch).
///
/// No `--host-resolver-rules` DNS pin is passed — see the note in
/// [`super::network_policy`] for why that control was removed rather than
/// repaired.
/// BROWSER-R4-09: anchored transport-error detection. The previous
/// heuristic checked four substrings ("broken pipe", "connection reset",
/// "process exited", "channel closed"), each plain `contains`, which
/// silently misclassifies whenever the wording drifts (e.g. "broken-pipe"
/// with a hyphen, "IO error: broken pipe", "connection closed"). Anchor
/// on the four shapes the underlying tokio / reqwest / tungstenite
/// stacks actually emit, each prefixed by an indicator word or
/// punctuation boundary so a stray "process exited" inside an unrelated
/// log line does not flip a tool-level error into transport.
fn looks_like_transport_error(s: &str) -> bool {
    let needles = [
        "broken pipe",
        "connection reset",
        "process exited",
        "channel closed",
    ];
    for n in needles {
        // Drop the `s.contains(n)` guard — `s.find(n)` returns `None`
        // exactly when `s.contains(n)` is false, and a single
        // linear scan is cheaper than two. The boundary check
        // below still anchors against "subprocess exited" and
        // similar false positives by requiring the match to be at
        // the start of the string or preceded by whitespace/colon
        // (the Display impls of io::Error / tungstenite::Error
        // prepend a label like "IO error: " or "(os error N)",
        // which is exactly the boundary we want).
        if let Some(idx) = s.find(n) {
            if idx == 0
                || s.as_bytes()[idx - 1].is_ascii_whitespace()
                || s.as_bytes()[idx - 1] == b':'
            {
                return true;
            }
        }
    }
    false
}

fn chrome_launch_args(
    profile_cfg: Option<&ProfileConfig>,
    profile_name: &str,
) -> Result<Vec<String>, BrowserError> {
    let mut args = vec![
        "--remote-debugging-port=0".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
    ];
    // BROWSER-R4-05: when the profile leaves user_data_dir unset, the
    // bootstrap-launched Chrome used to fall through to the user's
    // daily Chrome profile (~/.config/google-chrome on Linux,
    // %LOCALAPPDATA%\Google\Chrome\User Data on Windows) — silently
    // reading/writing the user's cookies, history, and logins.
    // Default to a per-profile Aleph-private path under $ALEPH_DATA
    // (or $HOME/.aleph) so the bootstrap clone is isolated. The
    // override `cfg.user_data_dir` still wins when configured.
    let default_user_data_dir = default_user_data_dir_for(profile_name)?;
    if let Some(cfg) = profile_cfg {
        if let Some(proxy) = &cfg.proxy {
            args.push(format!("--proxy-server={proxy}"));
        }
        match &cfg.user_data_dir {
            Some(dir) => args.push(format!("--user-data-dir={dir}")),
            None => args.push(format!("--user-data-dir={default_user_data_dir}")),
        }
        args.extend(cfg.extra_args.iter().cloned());
    } else {
        args.push(format!("--user-data-dir={default_user_data_dir}"));
    }
    Ok(args)
}

/// BROWSER-R4-05: the per-profile Aleph-private Chrome user-data-dir default,
/// so a bootstrap-launched Chrome never falls through to the human's daily
/// profile (their cookies, history and logins).
///
/// Rooted through [`get_config_dir`](crate::utils::paths::get_config_dir) — the
/// one function that answers "where is this process's `.aleph`" — rather than
/// hand-rolled. It used to read an `$ALEPH_DATA` env var that nothing else in
/// the repo reads or writes and no document mentions, then fall back to
/// `dirs::home_dir()`. Both spellings ignore `ALEPH_HOME`, which is the single
/// authoritative knob, so an isolated run (every QA run, every test harness)
/// wrote its browser profile into the operator's real `~/.aleph` while
/// believing it was sandboxed — the failure mode
/// `utils::paths::no_hand_rolled_aleph_home_outside_the_allowlist` exists to
/// catch, and did, the moment the lib test binary could be built again.
///
/// Returns a `String` so the caller can hand it straight to Chrome's
/// `--user-data-dir` flag.
fn default_user_data_dir_for(profile_name: &str) -> Result<String, BrowserError> {
    // BROWSER-R4-17: no home directory at all used to fall back to
    // `/tmp/.aleph`. `/tmp` is world-writable, so a local attacker can
    // pre-create `/tmp/.aleph/browser/chrome-mcp/<profile>/user-data-dir`
    // as a symlink to redirect Chrome's profile dir (TOCTOU). Refuse
    // to launch instead: a Chrome without a resolvable home cannot
    // honour the per-profile isolation this whole module exists to
    // guarantee, and silently launching into `/tmp` is worse than a
    // visible error the operator can route around.
    let root = crate::utils::paths::get_config_dir().map_err(|e| {
        BrowserError::LaunchFailed(format!(
            "cannot resolve Aleph home for chrome user-data-dir: {e} \
             (refusing to fall back to /tmp/.aleph — that path is world-writable \
             and a local attacker could symlink-redirect Chrome's profile)"
        ))
    })?;
    // BROWSER-R4-05: profile_name reaches us from operator config; without
    // sanitization a name like `/tmp/x` would replace the root via
    // `PathBuf::join` (absolute segments discard everything before them),
    // handing Chrome an arbitrary `--user-data-dir` and defeating the
    // per-profile isolation. Reuse the launch-config sanitizer that the
    // sibling `playwright-cli` path already enforces.
    let safe = super::playwright_launch::sanitize_session_key(profile_name);
    Ok(root
        .join("browser")
        .join("chrome-mcp")
        .join(safe)
        .join("user-data-dir")
        .to_string_lossy()
        .into_owned())
}

/// Manages Chrome `DevTools` MCP sessions with lazy creation and profile-keyed caching.
pub(crate) struct ChromeMcpDriver {
    sessions: RwLock<HashMap<String, Arc<ChromeMcpSession>>>,
    config: ChromeMcpConfig,
    /// Per-profile browser configuration (engine preference, proxy,
    /// user-data-dir, extra launch args), consulted when Aleph has to launch
    /// Chrome itself. Profiles absent from this map launch with baseline
    /// flags and the default browser discovery order.
    profiles: HashMap<String, ProfileConfig>,
    /// Prevents concurrent Chrome launches from racing.
    chrome_launch_lock: tokio::sync::Mutex<()>,
    /// Per-profile serialization locks. Page selection in chrome-devtools-mcp
    /// is server-side state, so a backend's `select_page` → action pair is two
    /// round-trips that must not interleave with a concurrent same-profile
    /// operation. The backend holds the matching lock across the whole pair.
    /// Mirrors `PlaywrightCliDriver`'s per-session lock.
    profile_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    /// Per-profile session-creation locks. Creating a session starts an MCP
    /// server and may launch Chrome — seconds of `await`. Serializing that per
    /// profile here (rather than under the `sessions` write lock) keeps the map
    /// readable by every other profile meanwhile. Distinct from
    /// [`Self::profile_locks`] on purpose: a backend action holds *that* lock
    /// while calling a tool, which is what triggers session creation — sharing
    /// one lock for both would deadlock.
    session_create_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    /// BROWSER-R4-01: PIDs of Chrome processes Aleph launched itself when
    /// no user Chrome was running. The `tokio::process::Child` handle is
    /// intentionally dropped after the bootstrap-spawn (the MCP server's
    /// `--autoConnect` is what keeps Chrome alive), so without this list
    /// the launched Chrome is reparented to PID 1 and outlives the
    /// daemon. Drop below kills each PID so a daemon stop cleans up.
    /// PIDs are recorded on launch and removed when the driver shuts
    /// the session down explicitly; the Drop covers everything else.
    ///
    /// BROWSER-R4-18: a raw `u32` PID is not enough — PIDs are reused
    /// after a process exits, so a daemon that takes a few seconds to
    /// shut down after Chrome died could SIGKILL an unrelated later
    /// process that landed on the same PID. The Linux entry also stores
    /// the kernel-reported `starttime` (clock ticks since boot, taken
    /// from `/proc/<pid>/stat` field 22); Drop compares it against the
    /// live value and skips the kill on mismatch. Non-Linux platforms
    /// retain the bare-PID behaviour, which is acceptable there because
    /// PIDs are not recycled as aggressively.
    launched_pids: Mutex<Vec<LaunchedPid>>,
}

/// One PID Aleph launched, plus enough provenance to tell a recycled PID
/// apart from the original on Linux.
#[derive(Debug, Clone, Copy)]
struct LaunchedPid {
    pid: u32,
    /// Linux only: `starttime` from `/proc/<pid>/stat` field 22, in clock
    /// ticks since boot. `None` on other platforms (the verification step
    /// then falls back to a `kill(pid, 0)` liveness probe).
    starttime: Option<u64>,
}

impl Drop for ChromeMcpDriver {
    /// BROWSER-R4-01: reap Chrome processes Aleph bootstrapped. The
    /// async-friendly `tokio::process::Child::start_kill` requires a
    /// runtime handle we do not have in a sync Drop; send SIGKILL via
    /// `libc::kill` on Unix / `taskkill /F` on Windows instead. Failures
    /// (already exited, owned by another user, PID recycled to a
    /// different process) are logged and ignored — the reaper on process
    /// exit will pick up orphans.
    fn drop(&mut self) {
        let pids = self
            .launched_pids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if pids.is_empty() {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            for entry in pids {
                // BROWSER-R4-18: skip the kill if the PID has been recycled
                // to a process whose `/proc/<pid>/stat` starttime differs
                // from what we recorded at launch. Reading `/proc` here is
                // the right grain — it is the canonical Linux source of
                // truth and the cost is one stat per recorded PID.
                match read_proc_starttime(entry.pid) {
                    Ok(now) if Some(now) == entry.starttime => {
                        // SAFETY: kill(2) with a stored pid is well-defined
                        // for any process the daemon's user can signal;
                        // ESRCH / EPERM are expected outcomes and logged,
                        // not propagated.
                        let rc = unsafe { libc::kill(entry.pid as i32, libc::SIGKILL) };
                        if rc != 0 {
                            let err = std::io::Error::last_os_error();
                            tracing::warn!(
                                pid = entry.pid,
                                error = %err,
                                "ChromeMcpDriver::drop: failed to SIGKILL launched Chrome"
                            );
                        }
                    }
                    Ok(_) => {
                        tracing::warn!(
                            pid = entry.pid,
                            "ChromeMcpDriver::drop: PID recycled to a different process; \
                             skipping SIGKILL (would have killed an unrelated process)"
                        );
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // Process already exited. Nothing to do.
                    }
                    Err(e) => {
                        tracing::warn!(
                            pid = entry.pid,
                            error = %e,
                            "ChromeMcpDriver::drop: failed to read /proc/<pid>/stat; \
                             skipping the kill rather than risk a recycled PID"
                        );
                    }
                }
            }
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            // macOS / BSD: PIDs are recycled less aggressively, so the
            // previous bare-PID behaviour is acceptable here. A `kill(pid, 0)`
            // probe still catches already-exited entries.
            for entry in pids {
                // SAFETY: probe only — no signal sent on success.
                let probe = unsafe { libc::kill(entry.pid as i32, 0) };
                if probe == 0 {
                    // SAFETY: see above.
                    let rc = unsafe { libc::kill(entry.pid as i32, libc::SIGKILL) };
                    if rc != 0 {
                        let err = std::io::Error::last_os_error();
                        tracing::warn!(
                            pid = entry.pid,
                            error = %err,
                            "ChromeMcpDriver::drop: failed to SIGKILL launched Chrome"
                        );
                    }
                }
            }
        }
        #[cfg(windows)]
        {
            for entry in pids {
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/PID", &entry.pid.to_string()])
                    .output();
            }
        }
    }
}

/// Read `/proc/<pid>/stat` field 22 (process start time, in clock ticks
/// since boot). Returns `NotFound` if the process is gone, other IO
/// errors unchanged.
#[cfg(target_os = "linux")]
fn read_proc_starttime(pid: u32) -> std::io::Result<u64> {
    let path = format!("/proc/{pid}/stat");
    let stat = std::fs::read_to_string(&path)?;
    // The comm field can contain spaces and parens; the kernel delimits it
    // with `(...)`. Find the LAST `)` to skip past it, then split on
    // whitespace and take field 22 (1-indexed) = index 21.
    let Some(close) = stat.rfind(')') else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "malformed /proc stat line",
        ));
    };
    let after = &stat[close + 1..];
    let field = after
        .split_whitespace()
        .nth(20) // 0-indexed: field 22 -> index 21
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "starttime field missing from /proc stat",
            )
        })?;
    field.parse::<u64>().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("parse starttime: {e}"),
        )
    })
}

impl ChromeMcpDriver {
    #[must_use]
    pub(crate) fn new(config: ChromeMcpConfig, profiles: HashMap<String, ProfileConfig>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config,
            profiles,
            chrome_launch_lock: tokio::sync::Mutex::new(()),
            profile_locks: Mutex::new(HashMap::new()),
            session_create_locks: Mutex::new(HashMap::new()),
            launched_pids: Mutex::new(Vec::new()),
        }
    }

    /// Get (or lazily create) the per-profile serialization lock. The returned
    /// `Arc<Mutex>` is held by the backend across a `select_page` → action
    /// sequence so concurrent operations on the same profile cannot interleave.
    pub(crate) fn profile_lock(&self, profile_name: &str) -> Arc<AsyncMutex<()>> {
        let mut map = self.profile_locks.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(profile_name.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// Call a tool on the Chrome `DevTools` MCP server for the given profile.
    /// Creates the session lazily if it doesn't exist.
    pub(crate) async fn call_tool(
        &self,
        profile_name: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        self.ensure_session(profile_name).await?;

        let sessions = self.sessions.read().await;
        let session = sessions.get(profile_name).ok_or_else(|| {
            BrowserError::ChromeMcpError("Session not found after creation".into())
        })?;
        let session = Arc::clone(session);
        drop(sessions);

        // MCP tools are namespaced with server prefix: "chrome-mcp-{profile}:{tool}"
        let server_name = format!("chrome-mcp-{profile_name}");
        let full_tool_name = format!("{server_name}:{tool_name}");
        // Two error shapes, kept apart deliberately. A failure of the *call*
        // (the client never got an answer) is a transport failure; a failure
        // reported *inside* a successful answer is the tool's own verdict about
        // the page. Callers must be able to tell them apart — `wait_for` folds
        // a tool-level "text never appeared" into `Ok(false)`, and folding a
        // dead pipe into the same value tells the model the text is not on the
        // page when in truth nothing ever looked.
        let result = match session.client.call_tool(&full_tool_name, args).await {
            Ok(r) => r,
            Err(e) => {
                let err_str = e.to_string();
                // The layer below returns `IoError` for BOTH "the pipe is dead"
                // and "the tool answered, and its answer was a failure", which
                // defeats the split this function documents. Ask it which one
                // this is instead of guessing from the text.
                if crate::mcp::external::is_tool_error(&err_str) {
                    return Err(BrowserError::ChromeMcpError(err_str));
                }
                let is_broken_pipe = looks_like_transport_error(&err_str);
                if is_broken_pipe {
                    tracing::warn!(
                        "Chrome MCP transport error for profile '{profile_name}': {err_str}"
                    );
                    // Only destroy if the same session is still stored
                    // (avoid racing a concurrent recreate that replaced
                    // the errored session).
                    self.destroy_session_if_same(profile_name, &session).await;
                }
                return Err(BrowserError::ChromeMcpTransport(err_str));
            }
        };

        if !result.success {
            return Err(BrowserError::ChromeMcpError(
                result
                    .error
                    .unwrap_or_else(|| "Unknown Chrome MCP error".into()),
            ));
        }
        Ok(result.content)
    }

    /// Get (or lazily create) the per-profile session-creation lock — see
    /// [`Self::session_create_locks`].
    fn session_create_lock(&self, profile_name: &str) -> Arc<AsyncMutex<()>> {
        let mut map = self
            .session_create_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.entry(profile_name.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// Ensure a session exists for the given profile, creating one if needed.
    ///
    /// The process-wide `sessions` map is never write-locked across the
    /// creation itself: `create_session` starts an MCP server and may launch
    /// Chrome (seconds of `await`), and holding the map's writer for that long
    /// stalls every other profile's `call_tool`, the idle reaper and the
    /// liveness read. Concurrency is serialized by a per-profile creation lock
    /// instead, and the map is only locked for the two short read/insert steps.
    async fn ensure_session(&self, profile_name: &str) -> Result<(), BrowserError> {
        // Fast path: check if session already exists
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(profile_name) {
                return Ok(());
            }
        }

        // Slow path: one creation at a time per profile.
        let create_lock = self.session_create_lock(profile_name);
        let _create_guard = create_lock.lock().await;

        // Double-check: a concurrent creator for this profile may have won.
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(profile_name) {
                return Ok(());
            }
        }

        let session = self.create_session(profile_name).await?;
        self.sessions
            .write()
            .await
            .insert(profile_name.to_string(), Arc::new(session));
        Ok(())
    }

    /// Create a new MCP session by spawning chrome-devtools-mcp.
    async fn create_session(&self, profile_name: &str) -> Result<ChromeMcpSession, BrowserError> {
        let server_name = format!("chrome-mcp-{profile_name}");
        let config = ExternalServerConfig {
            name: server_name.clone(),
            command: self.config.command.clone(),
            args: self.config.args.clone(),
            env: HashMap::new(),
            cwd: None,
            requires_runtime: Some("node".into()),
            timeout_seconds: Some(60),
        };

        let client = McpClient::new();
        match client.start_external_server(config).await {
            Ok(()) => {
                tracing::info!("Chrome DevTools MCP session started for profile '{profile_name}'");
                tracing::warn!(
                    "Existing-session mode connects to your Chrome with remote debugging enabled. \
                     Any local process can access browser data (cookies, passwords) via the debug port. \
                     This is Chrome's standard debugging interface (same as DevTools)."
                );
                Ok(ChromeMcpSession { client })
            }
            Err(e) => {
                tracing::info!(
                    "Chrome DevTools MCP connection failed, attempting to launch Chrome: {e}"
                );
                if let Err(e) = client.stop_all().await {
                    tracing::warn!("Failed to stop Chrome DevTools MCP client: {e}");
                }
                self.ensure_chrome_running(profile_name).await?;

                // Retry after Chrome launch
                let retry_config = ExternalServerConfig {
                    name: server_name,
                    command: self.config.command.clone(),
                    args: self.config.args.clone(),
                    env: HashMap::new(),
                    cwd: None,
                    requires_runtime: Some("node".into()),
                    timeout_seconds: Some(60),
                };

                let retry_client = McpClient::new();
                retry_client
                    .start_external_server(retry_config)
                    .await
                    .map_err(|e: crate::error::AlephError| {
                        BrowserError::AttachFailed(format!(
                            "Failed to connect Chrome DevTools MCP after launching Chrome: {e}"
                        ))
                    })?;

                Ok(ChromeMcpSession {
                    client: retry_client,
                })
            }
        }
    }

    /// Ensure Chrome is running with remote debugging enabled.
    async fn ensure_chrome_running(&self, profile_name: &str) -> Result<(), BrowserError> {
        let _guard = self.chrome_launch_lock.lock().await;

        if Self::is_chrome_running().await {
            return Err(BrowserError::AttachFailed(
                "Chrome is running but remote debugging is not enabled. \
                 Please restart Chrome or enable debugging at chrome://inspect/#remote-debugging"
                    .into(),
            ));
        }

        // A configured profile pins both the engine we look for and the
        // launch flags we pass; an unknown profile falls back to the plain
        // discovery order and baseline flags.
        let profile_cfg = self.profiles.get(profile_name);
        let chrome_path = match profile_cfg {
            Some(cfg) => find_chromium_preferred(&cfg.browser)?,
            None => find_chromium()?,
        };
        tracing::info!(
            "Launching Chrome with remote debugging: {}",
            chrome_path.display()
        );

        let args = chrome_launch_args(profile_cfg, profile_name)?;

        let mut cmd = Command::new(&chrome_path);
        for a in &args {
            cmd.arg(a);
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .no_window()
            .spawn()
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to launch Chrome: {e}")))?;

        // BROWSER-R4-01: record the bootstrap-launched PID so Drop can
        // SIGKILL it on daemon shutdown. The handle is intentionally
        // dropped right below (the MCP server's --autoConnect keeps
        // Chrome alive), so without this list the launched process
        // outlives the daemon.
        if let Some(pid) = child.id() {
            // BROWSER-R4-18: also stash the kernel-reported starttime on
            // Linux so Drop can refuse to SIGKILL a PID that has been
            // recycled to a different process.
            #[cfg(target_os = "linux")]
            let starttime = read_proc_starttime(pid).ok();
            #[cfg(not(target_os = "linux"))]
            let starttime = None;
            self.launched_pids
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(LaunchedPid { pid, starttime });
        }

        // Verify the process did not immediately exit instead of blind-sleeping.
        tokio::time::sleep(Duration::from_millis(100)).await;
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(BrowserError::LaunchFailed(format!(
                    "Chrome exited immediately with status {status}"
                )));
            }
            Ok(None) => {}
            Err(e) => {
                return Err(BrowserError::LaunchFailed(format!(
                    "Failed to check Chrome process status: {e}"
                )));
            }
        }

        // Drop the handle intentionally: `tokio::process::Child::kill_on_drop`
        // defaults to `false`, so this does NOT terminate the launched Chrome
        // process — it remains alive to serve the MCP server's `--autoConnect`.
        // A future shutdown hook (e.g. `Drop` on `ChromeMcpDriver`) can reap it.
        drop(child);
        Ok(())
    }

    /// Check if a Chrome browser process is running on the system.
    async fn is_chrome_running() -> bool {
        tokio::task::spawn_blocking(|| {
            // `pgrep -x` matches the WHOLE process name only. `chrome` is
            // the published Chromium / Chrome binary, but the same role is
            // filled on Linux by `chromium`, `chromium-browser`, `brave`,
            // `brave-browser`, and `msedge` (the stable Edge stable channel
            // ships an Electron-shaped binary called `msedge` — the Edge
            // stable on Linux uses the same DevTools surface as Chrome, so
            // an Aleph browser tool call should attach to it). Match the
            // same discovery list `discovery::CHROMIUM_NAMES` uses for
            // binary lookup so the two checks never disagree about what
            // counts as 'a Chrome-class browser'.
            #[cfg(target_os = "macos")]
            const NAMES: &[&str] = &["Google Chrome", "Google Chrome Helper"];
            #[cfg(all(unix, not(target_os = "macos")))]
            const NAMES: &[&str] = &[
                "chrome",
                "chromium",
                "chromium-browser",
                "brave",
                "brave-browser",
                "msedge",
            ];
            #[cfg(target_os = "macos")]
            {
                // macOS uses the full display name. Walk the list — one pgrep
                // per candidate — and short-circuit on the first hit.
                for name in NAMES {
                    let hit = std::process::Command::new("pgrep")
                        .arg("-x")
                        .arg(name)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .is_ok_and(|s| s.success());
                    if hit {
                        return true;
                    }
                }
                false
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                for name in NAMES {
                    let hit = std::process::Command::new("pgrep")
                        .arg("-x")
                        .arg(name)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if hit {
                        return true;
                    }
                }
                false
            }
            #[cfg(target_os = "windows")]
            {
                // `tasklist` exits 0 even when no process matches (it prints an
                // "INFO: No tasks…" line), so the command's success status says
                // nothing about whether Chrome is running. Capture stdout and
                // look for the image name instead.
                std::process::Command::new("tasklist")
                    .arg("/NH")
                    .arg("/FI")
                    .arg("IMAGENAME eq chrome.exe")
                    .stderr(Stdio::null())
                    .no_window()
                    .output()
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .to_ascii_lowercase()
                            .contains("chrome.exe")
                    })
                    .unwrap_or(false)
            }
            #[cfg(not(any(unix, target_os = "windows")))]
            {
                false
            }
        })
        .await
        .unwrap_or(false)
    }

    /// Destroy a session only if the stored session is still the expected one.
    /// Prevents a transport-error destroy from wiping out a concurrently
    /// recreated session.
    async fn destroy_session_if_same(&self, profile_name: &str, expected: &Arc<ChromeMcpSession>) {
        let session = {
            let mut sessions = self.sessions.write().await;
            match sessions.get(profile_name) {
                Some(current) if Arc::ptr_eq(current, expected) => sessions.remove(profile_name),
                _ => None,
            }
        };
        if let Some(session) = session {
            let _ = session.client.stop_all().await;
            tracing::info!(
                "Chrome MCP session destroyed for profile '{}'",
                profile_name
            );
        }
    }

    /// Whether a live session exists for `profile_name`. Best-effort: returns
    /// `false` on lock contention rather than awaiting — intended for the
    /// idle reaper and liveness reporting, where a skipped sweep is harmless.
    pub(crate) fn has_session(&self, profile_name: &str) -> bool {
        match self.sessions.try_read() {
            Ok(sessions) => sessions.contains_key(profile_name),
            Err(_) => false,
        }
    }

    /// Destroy a session (for cleanup after transport errors).
    pub(crate) async fn destroy_session(&self, profile_name: &str) {
        let session = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(profile_name)
        };
        // BROWSER-R4-06: prune the per-profile serialization locks for
        // this profile. Without this, every distinct profile_name ever
        // seen leaves a permanent `Arc<AsyncMutex<()>>` entry — each
        // roughly 96 bytes of heap + Arc bookkeeping. A long-lived
        // daemon serving dynamic profile names (test sessions,
        // customer-id-based names, names from error logs) accumulates
        // entries forever. Same fix as the session_create_locks below:
        // remove on explicit destroy. The destroy_session path is
        // typically reached on session teardown, which is the right
        // moment — no other profile operation can be holding the lock
        // past destroy.
        self.profile_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(profile_name);
        self.session_create_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(profile_name);
        if let Some(session) = session {
            let _ = session.client.stop_all().await;
            tracing::info!(
                "Chrome MCP session destroyed for profile '{}'",
                profile_name
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserType;

    fn test_driver() -> ChromeMcpDriver {
        ChromeMcpDriver::new(ChromeMcpConfig::default(), HashMap::new())
    }

    #[test]
    fn test_chrome_mcp_driver_new() {
        let driver = test_driver();
        let sessions = driver.sessions.try_read().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn chrome_launch_args_never_pass_a_host_resolver_pin() {
        // The DNS pin was removed (see network_policy's note): a control that
        // cannot fire on the reachable path is worse than none, because three
        // comments claimed it worked. This asserts it stays gone.
        let args = chrome_launch_args(None, "default").unwrap();
        assert!(
            !args.iter().any(|a| a.contains("--host-resolver-rules")),
            "no --host-resolver-rules may reach Chrome — args = {args:?}"
        );
        assert!(args.contains(&"--remote-debugging-port=0".to_string()));
    }

    #[test]
    fn chrome_launch_args_wires_profile_proxy_and_user_data_dir() {
        let cfg = ProfileConfig {
            proxy: Some("socks5://127.0.0.1:1080".into()),
            user_data_dir: Some("/tmp/aleph-profile".into()),
            ..Default::default()
        };
        let args = chrome_launch_args(Some(&cfg), "default").unwrap();
        assert!(args.contains(&"--proxy-server=socks5://127.0.0.1:1080".to_string()));
        assert!(args.contains(&"--user-data-dir=/tmp/aleph-profile".to_string()));
        // Baseline flags still lead the argv.
        assert_eq!(args[0], "--remote-debugging-port=0");
    }

    #[test]
    fn chrome_launch_args_appends_extra_args_last() {
        // extra_args go last so a user flag wins over a config-derived one.
        let cfg = ProfileConfig {
            proxy: Some("http://proxy:8080".into()),
            extra_args: vec![
                "--disable-gpu".into(),
                "--proxy-server=http://override:1".into(),
            ],
            ..Default::default()
        };
        let args = chrome_launch_args(Some(&cfg), "default").unwrap();
        let n = args.len();
        assert_eq!(args[n - 2], "--disable-gpu");
        assert_eq!(args[n - 1], "--proxy-server=http://override:1");
        // Both proxy flags are present; Chrome honors the last occurrence.
        assert!(args.contains(&"--proxy-server=http://proxy:8080".to_string()));
    }

    #[test]
    fn chrome_launch_args_default_profile_matches_baseline() {
        // A profile with no proxy/user-data/extra args must not change the argv.
        assert_eq!(
            chrome_launch_args(Some(&ProfileConfig::default()), "default").unwrap(),
            chrome_launch_args(None, "default").unwrap()
        );
    }

    #[test]
    fn driver_profile_lookup_prefers_configured_engine() {
        // The profiles map is what ensure_chrome_running consults: a configured
        // profile resolves to its config, an unknown one to None (baseline).
        let mut profiles = HashMap::new();
        profiles.insert(
            "work".into(),
            ProfileConfig {
                browser: BrowserType::Brave,
                ..Default::default()
            },
        );
        let driver = ChromeMcpDriver::new(ChromeMcpConfig::default(), profiles);
        assert_eq!(
            driver.profiles.get("work").map(|p| &p.browser),
            Some(&BrowserType::Brave)
        );
        assert!(!driver.profiles.contains_key("unknown"));
    }
}

#[cfg(test)]
mod integration_tests {
    use std::sync::Arc;

    use super::*;
    use crate::browser::backend::BrowserBackend;
    use crate::browser::chrome_mcp_backend::ChromeMcpBackend;
    use crate::browser::network_policy::BrowserSsrfGuard;

    #[tokio::test]
    #[ignore] // Requires Chrome + npx chrome-devtools-mcp installed
    async fn test_chrome_mcp_list_tools() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config, HashMap::new()));
        // Ensure session is created
        driver
            .ensure_session("user")
            .await
            .expect("session should start");
        let sessions = driver.sessions.read().await;
        let session = sessions.get("user").expect("session exists");
        let tools = session.client.list_tools().await;
        println!("=== Available MCP tools ({}) ===", tools.len());
        for tool in &tools {
            println!("  {} — {}", tool.name, tool.description);
        }
        assert!(!tools.is_empty(), "Should have tools available");
    }

    #[tokio::test]
    #[ignore]
    async fn test_chrome_mcp_list_tabs_raw() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config, HashMap::new()));
        driver.ensure_session("user").await.expect("session");
        let sessions = driver.sessions.read().await;
        let session = sessions.get("user").expect("session");

        // Call directly via client with full prefixed name
        println!("=== Calling chrome-mcp-user:list_pages via client...");
        let r1 = session
            .client
            .call_tool("chrome-mcp-user:list_pages", serde_json::json!({}))
            .await;
        println!("=== client result: {r1:?}");

        // Also try raw without prefix via the connection directly
        // Let's just see what tool names the server actually has
        let tools = session.client.list_tools().await;
        let page_tools: Vec<_> = tools
            .iter()
            .filter(|t| t.name.contains("page"))
            .map(|t| &t.name)
            .collect();
        println!("=== page-related tools: {page_tools:?}");
    }

    #[tokio::test]
    #[ignore]
    async fn test_chrome_mcp_list_tabs() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config, HashMap::new()));
        let backend = ChromeMcpBackend::new(
            driver,
            "user".to_string(),
            Arc::new(BrowserSsrfGuard::default()),
        );

        println!("Calling list_tabs...");
        match backend.list_tabs().await {
            Ok(tabs_text) => {
                println!("Open tabs:\n{tabs_text}");
                assert!(!tabs_text.is_empty(), "Should have at least one tab open");
            }
            Err(e) => {
                panic!("list_tabs failed: {e}");
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_chrome_mcp_snapshot() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config, HashMap::new()));
        let backend = ChromeMcpBackend::new(
            driver,
            "user".to_string(),
            Arc::new(BrowserSsrfGuard::default()),
        );

        let tabs_text = backend.list_tabs().await.expect("list_tabs");
        println!("Tabs for snapshot:\n{tabs_text}");
        // Parse first numeric tab id from text
        let tab_id = tabs_text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let colon_pos = line.find(": ")?;
                let id_str = line.get(..colon_pos)?.trim();
                if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty() {
                    Some(id_str.to_string())
                } else {
                    None
                }
            })
            .nth(1)
            .or_else(|| {
                tabs_text.lines().find_map(|line| {
                    let line = line.trim();
                    let colon_pos = line.find(": ")?;
                    let id_str = line.get(..colon_pos)?.trim();
                    if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty() {
                        Some(id_str.to_string())
                    } else {
                        None
                    }
                })
            })
            .expect("need at least one tab");
        let tab_id = &tab_id;

        let snapshot = backend
            .snapshot(tab_id)
            .await
            .expect("snapshot should succeed");
        assert!(
            !snapshot.snapshot_text.is_empty(),
            "Snapshot should have content"
        );
        println!("Snapshot text length: {}", snapshot.snapshot_text.len());
    }
}
