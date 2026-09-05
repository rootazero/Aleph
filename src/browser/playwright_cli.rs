//! `PlaywrightCliDriver` — manages per-session `playwright-cli` subprocesses.
//!
//! Each tool call spawns a fresh process with `-s=<session_key>`; the CLI
//! keeps browser state in memory across invocations under the same key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Mutex;

// Only the production `provision_binary` provisions a runtime; the sealed
// test twin must not so much as name the installer.
#[cfg(not(test))]
use crate::runtimes::{ensure_capability, ledger::CapabilityLedger};
use crate::security::secret_env::is_secret_env;
use crate::sync_primitives::RwLock;
use crate::utils::no_window::NoWindow;

use super::chromium_launch::{
    CdpEndpoint, ChromiumChild, ChromiumLaunchSpec, DEVTOOLS_PORT_DEADLINE,
};
use super::error::BrowserError;
use super::playwright_launch::{attach_argv, write_launch_config, LaunchPolicy, SessionLaunch};
use super::profile::{BrowserRuntimeConfig, PlaywrightCliConfig};

/// How long bringing up a browser session may take.
///
/// Mirrors the existing-session driver's answer to the same question
/// (`chrome_mcp.rs::create_session`, `timeout_seconds: Some(60)`) — the two
/// drivers are twins and should not disagree about how slow a cold browser is.
const SESSION_START_TIMEOUT_SECS: u64 = 60;

/// Output of a single `playwright-cli` invocation.
#[derive(Debug, Clone)]
pub(crate) struct CliOutput {
    pub stdout: String,
    pub page_meta: Option<PageMeta>,
}

/// Metadata extracted from the `### Page / URL / Title / Snapshot` header.
#[derive(Debug, Clone, Default)]
pub(crate) struct PageMeta {
    pub url: String,
    pub title: String,
    pub snapshot_file: Option<PathBuf>,
}

/// Lazily resolves + caches the `playwright-cli` binary path, then serializes
/// concurrent invocations per session key.
pub struct PlaywrightCliDriver {
    binary_path: RwLock<Option<PathBuf>>,
    config: PlaywrightCliConfig,
    runtime: BrowserRuntimeConfig,
    per_session_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    binary_resolve_lock: tokio::sync::Mutex<()>,
    /// The Chromium this driver launched, per session key.
    ///
    /// It lives HERE and not on `ProfileManager` because the lazy attach — the
    /// only place a browser comes into existence — runs under
    /// `per_session_locks`, three lines away. A map on the manager would have
    /// to re-derive that serialization, and two callers racing into an unopened
    /// session would each spawn a Chromium.
    ///
    /// A `std` mutex, never held across an `await`: the resolve-and-spawn
    /// happens outside it, and the per-session lock is what keeps that safe.
    chromium: crate::sync_primitives::Mutex<HashMap<String, ChromiumChild>>,
}

impl PlaywrightCliDriver {
    #[must_use]
    pub fn new(config: PlaywrightCliConfig, runtime: BrowserRuntimeConfig) -> Self {
        Self {
            binary_path: RwLock::new(None),
            config,
            runtime,
            per_session_locks: RwLock::new(HashMap::new()),
            binary_resolve_lock: tokio::sync::Mutex::new(()),
            chromium: crate::sync_primitives::Mutex::new(HashMap::new()),
        }
    }

    /// Resolve (or re-resolve) the CLI binary path. Caches on success.
    pub async fn resolve_binary(&self) -> Result<PathBuf, BrowserError> {
        if let Some(p) = self
            .binary_path
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(p);
        }

        let _guard = self.binary_resolve_lock.lock().await;

        if let Some(p) = self
            .binary_path
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(p);
        }

        if let Some(explicit) = self.config.binary_path.as_deref() {
            let p = PathBuf::from(explicit);
            if !p.exists() {
                return Err(BrowserError::PlaywrightCliNotInstalled);
            }
            *self.binary_path.write().unwrap_or_else(|e| e.into_inner()) = Some(p.clone());
            return Ok(p);
        }
        self.provision_binary().await
    }

    /// The slow path of [`Self::resolve_binary`]: ensure the full chain
    /// (fnm → node → playwright-cli + chromium + skills), **installing it over
    /// the network** if it is not there, and cache the result.
    #[cfg(not(test))]
    async fn provision_binary(&self) -> Result<PathBuf, BrowserError> {
        let runtimes_dir = crate::runtimes::get_runtimes_dir()
            .map_err(|e| BrowserError::PlaywrightCliError(format!("runtimes dir: {e}")))?;
        let ledger_path = runtimes_dir.join("ledger.json");
        let ledger =
            tokio::task::spawn_blocking(move || CapabilityLedger::load_or_create(ledger_path))
                .await
                .map_err(|e| {
                    BrowserError::PlaywrightCliError(format!("load capability ledger: {e}"))
                })?;
        let ledger = Arc::new(tokio::sync::RwLock::new(ledger));

        let resolved = tokio::time::timeout(
            // BROWSER-R4-10: cap the install path. Without the timeout,
            // ensure_capability could run for minutes on a captive
            // portal, offline host, or slow mirror. The first browser
            // tool call would block for the full duration before any
            // diagnostic surfaced. 5 minutes is generous for a fresh
            // download and well below the operator's patience horizon.
            std::time::Duration::from_secs(300),
            ensure_capability("playwright-cli", &ledger),
        )
        .await
        .map_err(|_| {
            BrowserError::PlaywrightCliError(
                "playwright-cli install timed out after 300s; \
                 check network connectivity and the runtimes mirror"
                    .to_string(),
            )
        })?
        .map_err(|e| BrowserError::PlaywrightCliError(format!("ensure playwright-cli: {e}")))?;

        *self.binary_path.write().unwrap_or_else(|e| e.into_inner()) = Some(resolved.clone());
        Ok(resolved)
    }

    /// Under test an unconfigured driver refuses instead of reaching outside
    /// the process — it neither installs a runtime nor launches a browser.
    ///
    /// The production twin above *installs things over the network*, and now
    /// that the driver can open a browser, a test that reached it would launch
    /// a real Chrome. One already did: a unit test asserting "degrades without
    /// a running browser" instead navigated a live browser to a public site.
    /// Those assertions were green because the machine happened to have no
    /// reachable browser — their green was a property of the environment, not
    /// of the code.
    ///
    /// Sealing here rather than in each test is deliberate: this is the one
    /// boundary where the process reaches out, so no future test can forget to
    /// seal itself. A test that genuinely wants a live browser opts in by
    /// setting `binary_path`, which [`Self::resolve_binary`] honors before
    /// calling this.
    ///
    /// ⚠️ `cfg(test)` covers `--lib` unit tests only; integration tests under
    /// `tests/` link the library built without it and are not sealed.
    #[cfg(test)]
    #[allow(clippy::unused_async)]
    async fn provision_binary(&self) -> Result<PathBuf, BrowserError> {
        Err(BrowserError::PlaywrightCliNotInstalled)
    }

    fn session_lock(&self, session_key: &str) -> Arc<Mutex<()>> {
        let mut map = self
            .per_session_locks
            .write()
            .unwrap_or_else(|e| e.into_inner());
        map.entry(session_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Run one `playwright-cli` subcommand for a session, giving the session a
    /// browser first if the CLI says it has none.
    ///
    /// Serializes concurrent calls within the same `session_key`, which is
    /// also what makes the lazy launch safe: the attach-and-retry happens under
    /// the same per-session lock as every other call, so two callers racing
    /// into an unattached session cannot both spawn a Chromium for it.
    ///
    /// One mutation of the child map is **outside** this lock and it is worth
    /// naming rather than leaving the claim above sounding total:
    /// [`Self::shutdown_chromium`], whose caller is the idle reaper and is not
    /// inside `run`. See its doc for what that does and does not cost.
    ///
    /// **Why lazily, off the CLI's own refusal, rather than eagerly:** the CLI
    /// is not the thing that owns the browser any more, but it is still the
    /// only thing that knows whether *this session* is attached. Attaching only
    /// when it says it is not makes a redundant attach structurally impossible,
    /// whereas an eager attach would have to be right about every path that
    /// obtains a backend. (Under `open` the same shape was load-bearing for a
    /// harder reason: a second `open` relaunched the browser and dropped every
    /// tab. A second `attach` is merely wasteful — but the browser it would
    /// attach to is now Aleph's, and re-deriving "is it alive" per call is how
    /// two answers to that question get created.)
    ///
    /// `policy` comes from the caller for two reasons: the driver is shared
    /// across sessions and does not know which profile a session key belongs
    /// to, and only some calls are entitled to create a browser at all — see
    /// [`LaunchPolicy`].
    pub(crate) async fn run(
        &self,
        session_key: &str,
        policy: LaunchPolicy<'_>,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CliOutput, BrowserError> {
        let bin = self.resolve_binary().await?;
        let lock = self.session_lock(session_key);
        let _guard = lock.lock().await;

        // Before the verb: if this profile HAD a browser and it has since
        // exited, no CLI subcommand can succeed and the error it returns is
        // not guaranteed to be one of the phrasings below. `chromium_died` is
        // a `try_wait` on a child we own — cheap enough to ask every time, and
        // it is the only thing that closes spec §6.2's "Chrome 中途死" row for
        // the verbs whose failure text says something else entirely.
        if self.chromium_died(session_key) {
            if let Some(launch) = policy.launch() {
                tracing::info!(session = %session_key, "chromium exited; relaunching before the verb");
                self.attach_session(&bin, session_key, launch).await?;
            }
        }

        let first = self.spawn(&bin, session_key, args, timeout).await;
        let Err(err) = first else {
            return first;
        };
        if !needs_relaunch(&err, self.chromium_alive(session_key)) {
            return Err(err);
        }
        let Some(launch) = policy.launch() else {
            return Err(err);
        };
        // NOT `forget_chromium` first. Tearing the browser down here would kill
        // a browser that is very often perfectly alive: `needs_relaunch` says
        // true for `NoSession` **regardless of liveness**, and `NoSession` is
        // exactly what a `playwright-cli` daemon restart produces while Aleph's
        // Chromium keeps running — so killing here would drop every tab to
        // recover a CLI session, which is D.9.10's double-`open` wearing this
        // round's costume. `ensure_chromium` already decides correctly and is
        // the ONLY place that decides: it re-uses a live child's endpoint, and
        // removes-and-kills a dead one before respawning.
        self.attach_session(&bin, session_key, launch).await?;
        // One retry only. If the verb still fails after a successful attach,
        // that is a real failure and must surface rather than loop.
        self.spawn(&bin, session_key, args, timeout).await
    }

    /// Give this session a browser: make sure Aleph's Chromium for the profile
    /// is alive, then `attach --cdp` to it.
    ///
    /// Calls [`Self::spawn`] directly rather than [`Self::run`]: the lock is
    /// already held by the caller, and going through `run` would make the
    /// attach-on-`NoSession` path re-entrant.
    ///
    /// One retry, and only for [`BrowserError::AttachFailed`]. That is the
    /// answer to the one race this design has: the liveness check said the
    /// child was there (or could not tell — see `ChromiumChild::alive`) and the
    /// endpoint refused the connection a moment later. Forgetting the child and
    /// attaching once more relaunches it. Bounded at one so a genuinely
    /// unreachable endpoint surfaces instead of looping.
    async fn attach_session(
        &self,
        bin: &Path,
        session_key: &str,
        launch: &SessionLaunch,
    ) -> Result<(), BrowserError> {
        match self.attach_once(bin, session_key, launch).await {
            Err(BrowserError::AttachFailed(detail)) => {
                tracing::warn!(session = %session_key, %detail, "attach refused; relaunching chromium");
                self.forget_chromium(session_key);
                self.attach_once(bin, session_key, launch).await
            }
            other => other,
        }
    }

    async fn attach_once(
        &self,
        bin: &Path,
        session_key: &str,
        launch: &SessionLaunch,
    ) -> Result<(), BrowserError> {
        let endpoint = self.ensure_chromium(bin, session_key, launch).await?;
        let config_path = write_launch_config(session_key).await?;
        let argv = attach_argv(&endpoint, &config_path);
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        // Attaching is not a navigation and must not borrow the navigation
        // budget. Same reasoning, and the same number, the `open` path used.
        let timeout =
            Duration::from_secs(self.config.nav_timeout_secs.max(SESSION_START_TIMEOUT_SECS));
        self.spawn(bin, session_key, &args, timeout).await?;
        tracing::info!(
            session = %session_key,
            endpoint = %endpoint.http_url,
            "playwright-cli attached to Aleph's chromium"
        );
        Ok(())
    }

    /// The endpoint of this session's Chromium, launching one if there is none
    /// (or the one there is has exited).
    ///
    /// Safe without its own lock because every caller holds the per-session
    /// lock from [`Self::run`]. The `chromium` mutex is taken twice, briefly,
    /// and never across the `await`s in between.
    ///
    /// `bin` is passed in rather than re-resolved: `run` already resolved it
    /// three lines up, and a second call site for the same fact is how two
    /// answers get created even when both are cached.
    async fn ensure_chromium(
        &self,
        bin: &Path,
        session_key: &str,
        launch: &SessionLaunch,
    ) -> Result<CdpEndpoint, BrowserError> {
        {
            let mut map = self.chromium.lock().unwrap_or_else(|e| e.into_inner());
            // Taken OUT and put back rather than inspected in place. The
            // alternative — `get_mut`, then `remove` inside the dead branch —
            // needs a second lookup whose `None` arm the surrounding branch has
            // already proved impossible, i.e. a predicate that can never go red
            // (判据 §2). The lock is held throughout, so the brief absence of a
            // live child from the map is not observable.
            if let Some(mut child) = map.remove(session_key) {
                if child.alive() {
                    let endpoint = child.endpoint().clone();
                    map.insert(session_key.to_string(), child);
                    return Ok(endpoint);
                }
                // Exited. It is already out of the map; `shutdown` reaps it and
                // clears the sidecar so the next boot does not try to kill this
                // pid.
                child.shutdown();
            }
        }

        let resolved =
            super::chromium_resolve::resolve_binary(&self.runtime, &launch.browser, bin).await?;
        // The replacement for the boot-time `unhonored_managed_fields` warning
        // this round deletes. `find_chromium_preferred` degrades SILENTLY when
        // the requested engine is not installed — it merely reorders candidates
        // and logs the fallback at `debug!` — so without this line "asked for
        // Brave, got Chrome" is reported nowhere. The predicate lives in
        // `chromium_resolve::engine_mismatch` rather than being spelled out
        // here: a bare `resolved != Some(requested)` fires on essentially every
        // launch (`BrowserType::default()` is `Chromium` and the resolved
        // engine is Chrome or Edge), and a warning that is always red is not a
        // warning (判据 §2). `None` — an unidentifiable path — is not evidence
        // that the request was honoured, so it warns too (判据 §8).
        if super::chromium_resolve::engine_mismatch(&launch.browser, resolved.engine.as_ref()) {
            tracing::warn!(
                requested = ?launch.browser,
                resolved = ?resolved.engine,
                path = %resolved.path.display(),
                "the managed profile asked for one engine and got another"
            );
        }
        let user_data_dir = chromium_user_data_dir(launch, session_key)?;
        let spec = ChromiumLaunchSpec {
            binary: resolved.path,
            user_data_dir,
            headless: launch.headless,
            proxy: launch.proxy.clone(),
            extra_args: launch.extra_args.clone(),
        };
        tracing::info!(
            session = %session_key,
            binary = %spec.binary.display(),
            source = resolved.source.label(),
            "launching chromium for the managed profile"
        );
        let child = ChromiumChild::spawn(&spec, session_key, DEVTOOLS_PORT_DEADLINE).await?;
        let endpoint = child.endpoint().clone();
        let mut map = self.chromium.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = map.insert(session_key.to_string(), child) {
            // Cannot happen while the per-session lock is held; if it ever
            // does, the previous browser is leaked unless it is killed here.
            previous.shutdown();
        }
        Ok(endpoint)
    }

    /// Kill and forget this session's Chromium. Returns whether there was one.
    fn forget_chromium(&self, session_key: &str) -> bool {
        let taken = self
            .chromium
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_key);
        match taken {
            Some(child) => {
                child.shutdown();
                true
            }
            None => false,
        }
    }

    /// This session's live CDP endpoint, if it has one — the accessor spec §3.2
    /// asks for. **Task 6 (`manager.rs`) is its first caller**; what the
    /// endpoint is ultimately *for* is Plan 2's live view, which reaches it
    /// through the manager rather than through this driver.
    ///
    /// `None` is "**this driver** launched no browser for this key", which is
    /// the same sentence as "there is no browser" only once Task 6's boot sweep
    /// has run: a Chromium orphaned by a previous process is recorded in the
    /// sidecar registry and not in this map, so before the sweep a caller that
    /// reads `None` as "nothing is running" is reading an absent record as an
    /// absent process (判据 §8). The same caveat applies to
    /// [`Self::chromium_alive`].
    // TODO(plan-1 task 6): remove this allow when Task 6 (manager.rs) calls
    // this. Until then `-D warnings` on `--lib` (which does not compile
    // `#[cfg(test)]`) sees no consumer.
    #[allow(dead_code)]
    pub(crate) fn endpoint(&self, session_key: &str) -> Option<CdpEndpoint> {
        self.chromium
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_key)
            .map(|c| c.endpoint().clone())
    }

    /// Whether this session's Chromium is running. The authoritative answer to
    /// "does this managed profile have a browser" now that Aleph owns it.
    pub(crate) fn chromium_alive(&self, session_key: &str) -> bool {
        self.chromium
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(session_key)
            .is_some_and(ChromiumChild::alive)
    }

    /// Whether this session HAD a browser and it has since exited.
    ///
    /// Deliberately not `!chromium_alive`: "there is no browser" and "the
    /// browser died" are different facts and only the second one is a reason to
    /// tear down and relaunch before a verb. Reading the first as the second
    /// would make the pre-verb check fire on every cold profile.
    pub(crate) fn chromium_died(&self, session_key: &str) -> bool {
        let mut map = self.chromium.lock().unwrap_or_else(|e| e.into_inner());
        map.get_mut(session_key).is_some_and(|c| !c.alive())
    }

    /// Public face of [`Self::forget_chromium`], for the idle reaper.
    ///
    /// ⚠️ Unlike every other mutation of the child map, this one does **not**
    /// run under the per-session lock — the reaper is not inside [`Self::run`].
    /// The map operation is atomic on its own mutex, so nothing corrupts; what
    /// is not serialized is the reaper against a concurrent lazy attach. There
    /// are two windows and they do **not** cost the same:
    ///
    /// * killed **before** the attach — the *system* recovers: the next verb
    ///   sees `chromium_died` and relaunches, at the cost of one wasted launch;
    /// * killed **during** the verb, after a successful attach — the *system*
    ///   still recovers on the next call, but **this request does not**.
    ///   `needs_relaunch` is false for whatever the CLI says about a connection
    ///   that vanished mid-command, so the in-flight tool call returns that
    ///   error as-is. Aleph has no measured transcript of that failure, and
    ///   guessing an anchor for it is how an over-broad anchor gets written, so
    ///   it is named here rather than classified (Task 9 scenario A is what
    ///   would measure it).
    ///
    /// The fix for both, if it ever matters, is for this to take `session_lock`
    /// and become `async`; it is stated here rather than done because the
    /// reaper that will call it does not exist yet.
    // TODO(plan-1 task 6): remove this allow. The idle reaper in Task 6
    // (manager.rs) is the first non-test caller.
    #[allow(dead_code)]
    pub(crate) fn shutdown_chromium(&self, session_key: &str) -> bool {
        self.forget_chromium(session_key)
    }

    /// Kill and forget **every** Chromium this driver launched. Returns how
    /// many there were.
    ///
    /// For a driver that does not live as long as the process. `ChromiumChild`
    /// wraps a `std::process::Child` with **no `Drop`** — deliberately, because
    /// a browser that outlives Aleph is the whole reason the sidecar registry
    /// exists — so a driver that is constructed per call and dropped at the end
    /// of it leaks one browser per launch unless it says so explicitly. That is
    /// not merely untidy: the orphan keeps the profile lock on
    /// `chromium-udd/<key>`, so the *next* launch on that key loses to it and
    /// fails permanently rather than transiently.
    /// `builtin_tools::pdf_generate::browser_engine` is exactly that caller.
    ///
    /// Task 6 consumes this too, for the process-wide shutdown path; it is here
    /// rather than there because the map it drains is private to this file.
    pub(crate) fn shutdown_all_chromium(&self) -> usize {
        let taken: Vec<ChromiumChild> = self
            .chromium
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, child)| child)
            .collect();
        let count = taken.len();
        for child in taken {
            child.shutdown();
        }
        count
    }

    /// Record a `ChromiumChild` for `session_key` as though this driver had
    /// launched it.
    ///
    /// The seam that makes the lazy-attach wiring testable at all: a unit test
    /// cannot launch a Chromium (and [`Self::provision_binary`]'s test twin
    /// exists to stop it trying), so without this every line of [`Self::run`]
    /// that touches a real child is unreachable — which is how a `run` that
    /// killed live browsers passed a full mutation sweep of `needs_relaunch`.
    ///
    /// Pairs with [`ChromiumChild::from_parts`]. **Task 6 consumes this same
    /// seam** for the reaper's tests; do not add a second one.
    #[cfg(test)]
    fn insert_test_child(&self, session_key: &str, child: ChromiumChild) {
        self.chromium
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_key.to_string(), child);
    }

    /// Spawn one `playwright-cli -s=<session_key> <args>` process and capture
    /// its output. Assumes the per-session lock is already held.
    async fn spawn(
        &self,
        bin: &Path,
        session_key: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CliOutput, BrowserError> {
        let session_flag = format!("-s={session_key}");
        let mut cmd = Command::new(bin);
        cmd.arg(&session_flag)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Kill the child if its owning future is dropped — e.g. on the
            // timeout branch below, where `wait_with_output` has consumed
            // `child` so it can no longer be killed explicitly.
            .kill_on_drop(true);
        // Strip secret-bearing env vars from the browser child process. The
        // browser never needs the parent's credentials; over-stripping is safe.
        for (name, _) in std::env::vars() {
            if is_secret_env(&name) {
                cmd.env_remove(&name);
            }
        }

        let child = cmd.no_window().spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => BrowserError::PlaywrightCliNotInstalled,
            _ => BrowserError::Io(e),
        })?;

        let output_fut = child.wait_with_output();
        let output = match tokio::time::timeout(timeout, output_fut).await {
            Ok(res) => res.map_err(BrowserError::Io)?,
            Err(_) => {
                // `output_fut` (owning `child`) is dropped here; `kill_on_drop`
                // set above terminates the process on timeout.
                return Err(BrowserError::Timeout(timeout.as_millis() as u64));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        if !output.status.success() {
            return Err(classify_failure(
                &stdout,
                &stderr,
                exit_code,
                session_key,
                timeout.as_millis() as u64,
            ));
        }
        // A clean exit status is not a success claim on this CLI — see
        // [`parse_error_section`]. Routed through the same classifier as a
        // non-zero exit so "the browser is not open" keeps producing
        // `NoSession` (and therefore the lazy launch) no matter which channel
        // the CLI chose to say it on.
        if let Some(err) = parse_error_section(&stdout) {
            return Err(classify_failure(
                &stdout,
                &err,
                exit_code,
                session_key,
                timeout.as_millis() as u64,
            ));
        }

        let page_meta = parse_page_meta(&stdout);
        Ok(CliOutput { stdout, page_meta })
    }

    pub const fn config(&self) -> &PlaywrightCliConfig {
        &self.config
    }
}

/// Classify a non-zero-exit invocation.
///
/// **Reads stdout as well as stderr.** `playwright-cli` writes its "the
/// browser is not open" refusal to *stdout* and leaves stderr empty (exit 1),
/// so a stderr-only classifier could never produce [`BrowserError::NoSession`]
/// — which is exactly what happened: the variant was constructed nowhere
/// reachable and had no consumers at all.
///
/// The not-open match is deliberately anchored on the CLI's full phrase rather
/// than a loose word: this runs on failure output that can include page text.
///
/// There are **two** such phrases, and they differ by more than wording:
///
/// * an *unknown* session prints to **stdout** —
///   `The browser 'x' is not open, please run open first`;
/// * a session the CLI still has a record of but whose browser is gone — which
///   is every profile with a `user_data_dir`, since those survive `close` as
///   `status: closed` — throws to **stderr** —
///   `Error: Browser 'x' is not open. Run … to start the browser session`.
///
/// Only the first was matched. The consequence was not cosmetic: a persistent
/// profile closed by the idle reaper never produced [`BrowserError::NoSession`],
/// so the lazy launch never fired and the profile stayed unusable for the life
/// of the process — the reaper's own housekeeping bricked the thing it reclaimed.
/// `is not open` is the substring both phrasings share; the older, narrower
/// anchors are kept because a third phrasing is likelier to resemble one of them
/// than to be predicted here.
///
/// A **third** phrasing joined the two "not open" ones when the driver stopped
/// launching browsers: a refused `attach --cdp`. Measured, not guessed —
/// `playwright-cli -s=x attach --cdp http://127.0.0.1:1` exits 1 with an EMPTY
/// stdout and a node exception on stderr (`Error: connect ECONNREFUSED …` plus
/// a `Call log:` line `- <ws preparing> retrieving websocket url from …`). It
/// shares no substring with either not-open phrase, so the anchors do not
/// interact.
///
/// Unlike the not-open anchors, this one is a **conjunction**. The not-open
/// pair could each stand alone because both are sentences only this CLI writes;
/// `ECONNREFUSED` is a sentence any program writes, and this function is
/// documented as running on output that can include page text. Requiring
/// playwright's own call-log line beside it is what a page cannot supply by
/// accident. A fourth wording is handled the same way it always was: add it,
/// do not widen an existing anchor.
///
/// `detail` is the CLI's own account of the failure: stderr on a non-zero exit,
/// the `### Error` body when the exit status was clean.
fn classify_failure(
    stdout: &str,
    detail: &str,
    exit_code: i32,
    session_key: &str,
    timeout_ms: u64,
) -> BrowserError {
    let s = format!("{stdout}\n{detail}").to_lowercase();
    // A refused attach, before the not-open anchors: it is neither "no browser
    // for this session" (which would attach again against the same dead
    // endpoint) nor a page-level failure.
    //
    // BOTH phrases are required, and that is the point. This function runs on
    // output that can contain page text — its own doc says so, and
    // `snapshot`/`console` echo the page under `### Result` — so a single
    // anchor on `econnrefused` would let a developer's own error page talk the
    // driver into relaunching a browser. The node error and playwright's own
    // call-log line appear together in the real transcript and not, by
    // accident, in a page.
    if s.contains("econnrefused") && s.contains("retrieving websocket url from") {
        return BrowserError::AttachFailed(detail.trim().to_string());
    }
    if s.contains("please run open first")
        || s.contains("is not open")
        || s.contains("no session")
        || s.contains("browser not open")
    {
        BrowserError::NoSession(session_key.to_string())
    } else if contains_timeout_phrase(&s) {
        BrowserError::Timeout(timeout_ms)
    } else if s.contains("element not found")
        || s.contains("no element")
        || s.contains("does not match any elements")
    {
        BrowserError::ActionFailed(format!("element not found ({})", detail.trim()))
    } else if exit_code == 0 {
        // Reported in-band with a clean status: printing "exit 0" alongside a
        // failure would be its own small lie.
        BrowserError::ActionFailed(detail.trim().to_string())
    } else {
        BrowserError::PlaywrightCliError(format!("exit {exit_code}: {detail}"))
    }
}

/// Whether this failure means "the browser needs relaunching", given whether
/// the browser is currently alive.
///
/// * [`BrowserError::NoSession`] — the CLI has no session for this key. Attach,
///   whatever the browser is doing; that is the lazy launch this driver has
///   always had.
/// * [`BrowserError::AttachFailed`] — the endpoint refused a connection. Only a
///   reason to relaunch when the browser is **not** alive. Relaunching over a
///   live browser is appendix D.9.10's hazard in a new costume: there it was a
///   second `open` dropping every tab, here it is a second Chromium writing the
///   same `DevToolsActivePort`.
/// * everything else — the model's error to read. The harness does not pick a
///   recovery strategy on its behalf (R10 第 5 不).
#[must_use]
fn needs_relaunch(err: &BrowserError, chromium_alive: bool) -> bool {
    match err {
        BrowserError::NoSession(_) => true,
        BrowserError::AttachFailed(_) => !chromium_alive,
        _ => false,
    }
}

/// BROWSER-R4-03: anchored timeout detection. Previously the classifier
/// folded on any substring containing the literal "timeout", which
/// mis-classified unrelated error text like "the timeout parameter was
/// rejected" or "previous request hit a timeout". Anchor on the common
/// playwright-cli timeout phrasings (boundary-padded), so a stray
/// "timeout" inside a longer word or a debug log does not flip a
/// non-timeout failure into [`BrowserError::Timeout`] (and thereby
/// trigger the tool-layer's retry-against-broken-state path).
fn contains_timeout_phrase(s: &str) -> bool {
    // Accept the four phrasings playwright-cli itself uses at the four
    // timeout sites (action / navigation / waitFor / expect). The
    // pattern matches when the phrase is preceded by whitespace or
    // start-of-string and followed by whitespace, punctuation, or
    // end-of-string — the rough "word boundary" check that handles
    // "timeout" without anchoring to the regex crate.
    for needle in [
        " timeout ",
        " timeout.",
        " timeout:",
        "timeout exceeded",
        "timed out",
    ] {
        if s.contains(needle) {
            return true;
        }
    }
    false
}

/// The `### Error` section, when the invocation reported a failure that its
/// **exit code did not**.
///
/// `playwright-cli` exits 0 for runtime failures and says so only in stdout:
/// a thrown `eval`, an element that matches nothing, an unhandled modal state,
/// and every `File access denied` refusal all leave the process status clean.
/// Deciding success on the exit code alone therefore reported each of them to
/// the model as a success — `browser_pdf` answered "Saved PDF to <path>" for a
/// file the CLI had refused to write, and `browser_upload` answered "Uploaded
/// 1 file(s)" having attached nothing.
///
/// Only a transcript whose **first** `### ` header is `### Error` counts. The
/// looser rule (any such line anywhere) would let untrusted page text decide:
/// `snapshot` and `console` echo page content under `### Result`, so a page
/// carrying a line `### Error` could make every read of itself fail. The CLI
/// writes its own header first, which the page cannot precede.
#[must_use]
pub(crate) fn parse_error_section(stdout: &str) -> Option<String> {
    let mut body = String::new();
    let mut in_error = false;
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("### ") {
            if in_error {
                break;
            }
            if trimmed.trim_end() == "### Error" {
                in_error = true;
                continue;
            }
            // Some other section came first — this is not an error transcript.
            return None;
        }
        if in_error {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
    }
    in_error.then(|| body.trim().to_string())
}

/// Parse stdout for `### Page / URL / Title / Snapshot [path]` header.
#[must_use]
pub(crate) fn parse_page_meta(stdout: &str) -> Option<PageMeta> {
    let mut meta = PageMeta::default();
    let mut in_page_section = false;
    let mut found_any = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed == "### Page" {
            in_page_section = true;
            continue;
        }
        if in_page_section {
            if let Some(rest) = trimmed.strip_prefix("- Page URL:") {
                meta.url = rest.trim().to_string();
                found_any = true;
            } else if let Some(rest) = trimmed.strip_prefix("- Page Title:") {
                meta.title = rest.trim().to_string();
                found_any = true;
            }
        }
        // "### Snapshot" is followed by: [Snapshot](<path>)
        if let Some(path) = trimmed
            .strip_prefix("[Snapshot](")
            .and_then(|s| s.strip_suffix(')'))
        {
            meta.snapshot_file = Some(PathBuf::from(path.trim()));
            found_any = true;
        }
    }
    if found_any {
        Some(meta)
    } else {
        None
    }
}

/// Extract the value an `eval` produced, out of the CLI's transcript.
///
/// `playwright-cli eval` answers with a transcript, not a value:
///
/// ````text
/// ### Result
/// "absent"
/// ### Ran Playwright code
/// ```js
/// await page.evaluate('() => (...) ? "ALEPH_WAIT_FOUND" : \'absent\'');
/// ```
/// ````
///
/// The second section echoes **the script that was run**, so any caller that
/// searches the raw stdout for a token is searching a channel that contains its
/// own question. That is not hypothetical: `wait_probe`'s sentinel is a literal
/// inside every probe it builds, so `out.contains(WAIT_PROBE_FOUND)` was true on
/// the first poll of every wait — `browser_wait_for` and `browser_exec`'s `wait`
/// step reported "found" instantly for conditions that never held, on the
/// default driver, with no error anywhere. Its doc even asserted the opposite
/// ("the probe's result value is the only thing echoed back"), which was true of
/// the fake backend the unit tests used and false of the CLI.
///
/// Returns `None` when there is no `### Result` section — an `### Error`
/// transcript, notably, which carries no echoed source either and so is safe for
/// the caller to hand on raw.
pub(crate) fn parse_result_value(stdout: &str) -> Option<String> {
    let mut lines = stdout.lines();
    lines.by_ref().find(|l| l.trim() == "### Result")?;
    let mut value = String::new();
    for line in lines {
        // Any following `### ` header ends the value — `### Ran Playwright code`
        // in practice, but the rule is the section, not that one heading.
        if line.trim_start().starts_with("### ") {
            break;
        }
        if !value.is_empty() {
            value.push('\n');
        }
        value.push_str(line);
    }
    Some(value.trim().to_string())
}

/// Where this session's Chromium keeps its profile.
///
/// The profile's own `user_data_dir` when it has one; otherwise a directory
/// derived under `~/.aleph/data/browser/chromium-udd/<key>`.
///
/// **A managed profile can no longer be "in memory".** `DevToolsActivePort` is
/// written into the user-data-dir, so a browser with no profile directory has
/// no discoverable endpoint — the file IS the contract. That is a behaviour
/// change for a default profile and it is stated here rather than left to be
/// discovered: browsing state (cookies, localStorage) now survives a restart
/// for every managed profile, not only for the ones that asked.
pub(crate) fn chromium_user_data_dir(
    launch: &SessionLaunch,
    session_key: &str,
) -> Result<PathBuf, BrowserError> {
    if let Some(dir) = &launch.user_data_dir {
        return Ok(PathBuf::from(dir));
    }
    Ok(super::playwright_launch::browser_state_dir("chromium-udd")?
        .join(super::playwright_launch::sanitize_session_key(session_key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both refusals, copied verbatim from `playwright-cli 0.1.8` — one per
    /// channel, because which one you get depends on whether the CLI still has
    /// a record of the session, and only one of them was ever recognised.
    #[test]
    fn both_not_open_phrasings_classify_as_no_session() {
        // Unknown session: stdout, exit 1.
        let stdout = "The browser 'p' is not open, please run open first\n\n  playwright-cli -s=p open [params]\n";
        assert!(matches!(
            classify_failure(stdout, "", 1, "p", 1000),
            BrowserError::NoSession(_)
        ));

        // Known-but-closed session (any profile with a user_data_dir): a raw
        // node throw on stderr, no "please run open first" anywhere in it.
        let stderr = "Error: Browser 'p' is not open. Run\n\n  playwright-cli -s=p open\n\nto start the browser session.\n    at Session.run (…/cli-client/session.js:61:13)\n";
        assert!(
            !stderr.contains("please run open first"),
            "the whole point is that this message does not carry the old anchor"
        );
        assert!(matches!(
            classify_failure("", stderr, 1, "p", 1000),
            BrowserError::NoSession(_)
        ));
    }

    /// The anchor must not swallow ordinary action failures — a `NoSession`
    /// verdict triggers a relaunch, and relaunching drops every open tab.
    #[test]
    fn an_ordinary_failure_is_not_mistaken_for_a_closed_browser() {
        assert!(matches!(
            classify_failure(
                "### Error\nError: \"#nope\" does not match any elements.\n",
                "",
                0,
                "p",
                1000
            ),
            BrowserError::ActionFailed(_)
        ));
    }

    /// Shapes taken verbatim from a real `playwright-cli` 0.1.8 run — the
    /// parser exists because the transcript was assumed to be the value, so a
    /// hand-imagined format here would reproduce the original mistake.
    #[test]
    fn parse_result_value_takes_the_value_and_drops_the_echoed_script() {
        let stdout = "### Result\n\"absent\"\n### Ran Playwright code\n```js\nawait page.evaluate('() => 1');\n```\n";
        assert_eq!(parse_result_value(stdout).as_deref(), Some("\"absent\""));

        // A non-JSON value (`undefined`) is still a value.
        let stdout = "### Result\nundefined\n### Ran Playwright code\n```js\nx\n```\n";
        assert_eq!(parse_result_value(stdout).as_deref(), Some("undefined"));

        // A multi-line value keeps its interior newlines.
        let stdout = "### Result\n{\n  \"a\": 1\n}\n### Ran Playwright code\n";
        assert_eq!(
            parse_result_value(stdout).as_deref(),
            Some("{\n  \"a\": 1\n}")
        );
    }

    #[test]
    fn parse_result_value_declines_an_error_transcript() {
        // `eval` exits 0 on a thrown script and prints `### Error` with NO
        // echoed source, so returning `None` here hands the caller the
        // diagnostic intact without reintroducing the echo.
        let stdout = "### Error\nError: boom\n    at eval (<anonymous>:1:16)\n";
        assert_eq!(parse_result_value(stdout), None);
        assert_eq!(parse_result_value(""), None);
    }

    #[test]
    fn test_parse_page_meta_full() {
        let stdout = "\
### Page
- Page URL: https://example.com/
- Page Title: Example Domain
### Snapshot
[Snapshot](.playwright-cli/page-2026-04-12T00-00-00Z.yml)
";
        let meta = parse_page_meta(stdout).unwrap();
        assert_eq!(meta.url, "https://example.com/");
        assert_eq!(meta.title, "Example Domain");
        assert_eq!(
            meta.snapshot_file.as_ref().unwrap().to_string_lossy(),
            ".playwright-cli/page-2026-04-12T00-00-00Z.yml"
        );
    }

    #[test]
    fn test_parse_page_meta_none_for_empty() {
        assert!(parse_page_meta("").is_none());
        assert!(parse_page_meta("just some unrelated output").is_none());
    }

    /// The `cfg(test)` seal is itself asserted, not assumed: an unconfigured
    /// driver must refuse to resolve a binary rather than probe the machine.
    /// If this ever regresses, every "degrades without a running browser" test
    /// silently becomes a live-browser test again.
    #[tokio::test]
    async fn an_unconfigured_driver_cannot_reach_the_machine_under_test() {
        let driver = PlaywrightCliDriver::new(
            PlaywrightCliConfig::default(),
            BrowserRuntimeConfig::default(),
        );
        assert!(
            matches!(
                driver.resolve_binary().await,
                Err(BrowserError::PlaywrightCliNotInstalled)
            ),
            "an unconfigured driver must not resolve a real binary in tests"
        );
    }

    /// …and the opt-in still works, so a deliberate live test can set a path.
    /// (Pointed at a path that does not exist, so this test stays hermetic
    /// too — what it proves is that `binary_path` is consulted before the
    /// seal, not that any particular binary is present.)
    #[tokio::test]
    async fn an_explicit_binary_path_is_still_consulted_first() {
        let driver = PlaywrightCliDriver::new(
            PlaywrightCliConfig {
                binary_path: Some("/nonexistent/aleph-playwright-cli".into()),
                ..PlaywrightCliConfig::default()
            },
            BrowserRuntimeConfig::default(),
        );
        // Missing file → NotInstalled, same variant, so assert on the branch
        // by using a path that DOES exist instead.
        let exists = std::env::current_exe().expect("test binary path");
        let driver2 = PlaywrightCliDriver::new(
            PlaywrightCliConfig {
                binary_path: Some(exists.to_string_lossy().into_owned()),
                ..PlaywrightCliConfig::default()
            },
            BrowserRuntimeConfig::default(),
        );
        assert!(driver.resolve_binary().await.is_err());
        assert_eq!(
            driver2.resolve_binary().await.ok(),
            Some(exists),
            "an explicit, existing binary_path must win over the test seal"
        );
    }

    #[test]
    fn test_classify_stderr_no_session() {
        let err = classify_failure("", "Error: no session found for -s=foo", 1, "foo", 5000);
        assert!(matches!(err, BrowserError::NoSession(_)));
    }

    /// The refusal `playwright-cli 0.1.8` actually emits, copied verbatim —
    /// and it arrives on **stdout** with stderr empty. A classifier that read
    /// only stderr called this a generic `PlaywrightCliError`, so the lazy
    /// launch could never trigger and the managed driver could never reach a
    /// browser at all.
    #[test]
    fn the_real_not_open_refusal_arrives_on_stdout_and_still_classifies() {
        let stdout = "The browser 'default' is not open, please run open first\n\n                        playwright-cli -s=default open [params]\n";
        let err = classify_failure(stdout, "", 1, "default", 5000);
        assert!(
            matches!(err, BrowserError::NoSession(_)),
            "stdout-only refusal must classify as NoSession, got {err:?}"
        );
    }

    #[test]
    fn test_classify_stderr_timeout() {
        let err = classify_failure("", "Error: action timeout 5000ms", 1, "foo", 5000);
        assert!(matches!(err, BrowserError::Timeout(5000)));
    }

    #[test]
    fn test_classify_stderr_element_not_found() {
        let err = classify_failure("", "element not found: #missing", 1, "foo", 5000);
        assert!(matches!(err, BrowserError::ActionFailed(_)));
    }

    #[test]
    fn test_classify_stderr_generic() {
        let err = classify_failure("", "something else", 2, "foo", 5000);
        assert!(matches!(err, BrowserError::PlaywrightCliError(_)));
    }

    #[test]
    fn test_is_secret_env_exact_matches() {
        assert!(is_secret_env("ANTHROPIC_API_KEY"));
        assert!(is_secret_env("ALEPH_VAULT_KEY"));
        assert!(is_secret_env("AWS_SECRET_ACCESS_KEY"));
        assert!(is_secret_env("DATABASE_URL"));
        // Case-insensitive
        assert!(is_secret_env("anthropic_api_key"));
    }

    #[test]
    fn test_is_secret_env_suffix_heuristic() {
        assert!(is_secret_env("ACME_API_KEY"));
        assert!(is_secret_env("SOME_SERVICE_TOKEN"));
        assert!(is_secret_env("MY_DB_PASSWORD"));
        assert!(is_secret_env("APP_PRIVATE_KEY"));
    }

    #[test]
    fn test_is_secret_env_allows_normal_vars() {
        assert!(!is_secret_env("PATH"));
        assert!(!is_secret_env("HOME"));
        assert!(!is_secret_env("LANG"));
        assert!(!is_secret_env("ALEPH_CHROME_PATH"));
    }
}

#[cfg(test)]
mod attach_tests {
    use super::*;

    /// The verbatim stderr of a real `attach --cdp` against a dead port
    /// (playwright-cli 0.1.8 / node 24.14.1), trimmed of the stack frames that
    /// carry absolute paths.
    const ATTACH_REFUSED: &str = "\
Error: connect ECONNREFUSED 127.0.0.1:1
Call log:
  - <ws preparing> retrieving websocket url from http://127.0.0.1:1
";

    /// A refused attach is its own outcome. It is NOT `NoSession` (that would
    /// loop straight back into another attach against the same dead endpoint)
    /// and it is NOT a generic CLI error (that would surface to the model as
    /// "exit 1: <node stack trace>" for a browser that merely needs
    /// relaunching).
    #[test]
    fn a_refused_attach_classifies_as_attach_failed() {
        let err = classify_failure("", ATTACH_REFUSED, 1, "default", 10_000);
        assert!(
            matches!(err, BrowserError::AttachFailed(_)),
            "expected AttachFailed, got {err:?}"
        );
    }

    /// The anchor is the **pair**, not either phrase alone, and that is not
    /// fussiness: `classify_failure` runs on output that can contain page text
    /// (its own doc says so, and `snapshot`/`console` echo the page under
    /// `### Result`). A developer's own error page carrying the word
    /// `ECONNREFUSED` must not be able to talk the driver into relaunching a
    /// browser. Requiring both the node error AND playwright's call-log line is
    /// what a page cannot supply by accident.
    #[test]
    fn one_half_of_the_attach_signature_is_not_enough() {
        for half in [
            "Error: connect ECONNREFUSED 127.0.0.1:8080",
            "  - <ws preparing> retrieving websocket url from http://127.0.0.1:1",
        ] {
            let err = classify_failure(half, "", 1, "default", 10_000);
            assert!(
                !matches!(err, BrowserError::AttachFailed(_)),
                "half the signature was enough: {half:?} -> {err:?}"
            );
        }
    }

    /// The two "not open" phrasings (appendix D.9.13) must keep producing
    /// `NoSession` — that is what makes the lazy attach fire at all. Adding the
    /// attach anchors must not shadow either of them.
    #[test]
    fn both_not_open_phrasings_still_produce_no_session() {
        for (stdout, stderr) in [
            (
                "The browser 'default' is not open, please run open first",
                "",
            ),
            (
                "",
                "Error: Browser 'default' is not open. Run open to start the browser session",
            ),
        ] {
            assert!(
                matches!(
                    classify_failure(stdout, stderr, 1, "default", 10_000),
                    BrowserError::NoSession(_)
                ),
                "lost the lazy-attach trigger for {stdout:?}/{stderr:?}"
            );
        }
    }

    /// An ordinary Playwright failure keeps its own class.
    #[test]
    fn an_unrelated_failure_is_not_read_as_a_refused_attach() {
        let err = classify_failure(
            "",
            "Error: strict mode violation: locator resolved to 3 elements",
            1,
            "default",
            10_000,
        );
        assert!(
            !matches!(err, BrowserError::AttachFailed(_)),
            "over-broad attach anchor: {err:?}"
        );
    }

    /// The user-data-dir is where `DevToolsActivePort` lands, so the managed
    /// driver can no longer keep a browser "in memory": a profile that
    /// configures none gets one derived under `~/.aleph/data/browser`. The
    /// containment property is the same one `config_path_for` has — one
    /// component under the state dir, whatever the session key looks like.
    #[test]
    fn every_profile_gets_a_user_data_dir_and_it_cannot_escape() {
        let configured = chromium_user_data_dir(
            &SessionLaunch {
                user_data_dir: Some("/tmp/explicit".into()),
                ..SessionLaunch::headless_default()
            },
            "default",
        )
        .expect("home resolves");
        assert_eq!(configured, std::path::PathBuf::from("/tmp/explicit"));

        let derived = chromium_user_data_dir(&SessionLaunch::headless_default(), "default")
            .expect("home resolves");
        let dir = derived.parent().expect("has a parent").to_path_buf();
        for hostile in ["../../etc", "/etc", "..", "", "a/b"] {
            let p = chromium_user_data_dir(&SessionLaunch::headless_default(), hostile)
                .expect("home resolves");
            assert_eq!(p.parent(), Some(dir.as_path()), "escaped with {hostile:?}");
            assert_eq!(
                p.components().count(),
                dir.components().count() + 1,
                "not a single component for {hostile:?}"
            );
        }
    }

    /// spec §6.2 row "Chrome 中途死 → 下次工具调用惰性重启" — and the two ways it
    /// can present, which the first draft covered only one of.
    ///
    /// `NoSession` is the CLI saying it has no session: attach, whatever the
    /// browser is doing. `AttachFailed` is the endpoint refusing a connection,
    /// and it must trigger a relaunch **only when the browser is not alive**.
    /// Relaunching over a live browser is the D.9.10 hazard in a new costume:
    /// the old one was a second `open` dropping every tab, the new one is a
    /// second Chromium writing the same `DevToolsActivePort`. Everything else
    /// is the model's error to read, not the driver's to route (R10 第 5 不).
    #[test]
    fn only_a_dead_browser_earns_a_relaunch() {
        // The CLI has no session: attach regardless of the browser's state.
        assert!(needs_relaunch(&BrowserError::NoSession("d".into()), true));
        assert!(needs_relaunch(&BrowserError::NoSession("d".into()), false));
        // A refused endpoint with the browser gone: relaunch.
        assert!(needs_relaunch(
            &BrowserError::AttachFailed("econnrefused".into()),
            false
        ));
        // A refused endpoint while the browser is ALIVE: do not. Something else
        // is wrong and a second Chromium would make it worse.
        assert!(!needs_relaunch(
            &BrowserError::AttachFailed("econnrefused".into()),
            true
        ));
        // Ordinary failures are the model's to read.
        for other in [
            BrowserError::ActionFailed("element not found".into()),
            BrowserError::Timeout(1000),
            BrowserError::PlaywrightCliError("exit 1: boom".into()),
        ] {
            assert!(
                !needs_relaunch(&other, false),
                "{other:?} must not relaunch"
            );
            assert!(!needs_relaunch(&other, true), "{other:?} must not relaunch");
        }
    }

    /// The cheap pre-verb check. `chromium_alive` is a `try_wait` on a child we
    /// own — no syscall storm, no process-table scan — so asking before every
    /// verb costs nothing and closes the gap where the CLI's error text does
    /// not happen to be one of the phrasings above.
    ///
    /// With no child recorded it must answer `false`: "there is no browser" is
    /// not "the browser is dead", and the lazy attach already handles the
    /// first case.
    #[tokio::test]
    async fn a_profile_with_no_child_is_not_reported_as_a_dead_one() {
        let driver = PlaywrightCliDriver::new(
            PlaywrightCliConfig::default(),
            crate::browser::profile::BrowserRuntimeConfig::default(),
        );
        assert!(!driver.chromium_alive("default"));
        assert!(!driver.chromium_died("default"));
    }

    /// **The C1 guard.** A `playwright-cli` daemon restart loses the CLI session
    /// while Aleph's Chromium keeps running, so `NoSession` arrives over a
    /// perfectly live browser. The relaunch arm used to `forget_chromium` first,
    /// which kills that browser and every tab in it to recover a CLI session —
    /// D.9.10's double-`open` in this round's costume, in the one path no test
    /// could reach. `needs_relaunch`'s own mutation sweep could not catch it:
    /// that predicate's contract is "a relaunch is needed", and the bug was in
    /// what the wiring did on the way to the relaunch.
    ///
    /// The browser here is a `sleep`, and the CLI is a shell script that refuses
    /// once and then succeeds — nothing reaches the network or a real browser.
    /// The runtime config pins a Chromium that does not exist **on purpose**: if
    /// the child is ever torn down, `ensure_chromium` has to resolve a binary,
    /// and that pin makes the resulting failure instant and loud instead of a
    /// real Chrome launch inside a unit test.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_lost_cli_session_does_not_cost_a_live_browser_its_tabs() {
        use std::os::unix::fs::PermissionsExt;

        let session = "aleph-unit-c1-guard";
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let marker = tmp.path().join("verb-was-refused-once");
        let cli = tmp.path().join("fake-playwright-cli");

        // Refuses the first verb the way playwright-cli 0.1.8 does (stdout,
        // exit 1), answers `attach` with a clean exit, then serves the verb.
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\n\
                 case \" $* \" in\n\
                 *\" attach \"*) exit 0 ;;\n\
                 esac\n\
                 if [ -e {marker:?} ]; then\n\
                 echo '### Page'\n\
                 echo '- Page URL: about:blank'\n\
                 exit 0\n\
                 fi\n\
                 : > {marker:?}\n\
                 echo \"The browser '{session}' is not open, please run open first\"\n\
                 exit 1\n",
                marker = marker.to_string_lossy(),
            ),
        )
        .expect("write fake cli");
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake cli");

        // The "browser": a process we can observe, that outlives the call.
        let sleeper = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = sleeper.id();
        let driver = PlaywrightCliDriver::new(
            PlaywrightCliConfig {
                binary_path: Some(cli.to_string_lossy().into_owned()),
                ..PlaywrightCliConfig::default()
            },
            BrowserRuntimeConfig {
                binary_path: Some("/nonexistent/aleph-test-chromium".into()),
                ..BrowserRuntimeConfig::default()
            },
        );
        driver.insert_test_child(
            session,
            ChromiumChild::from_parts(
                sleeper,
                CdpEndpoint {
                    http_url: "http://127.0.0.1:1".into(),
                    ws_url: "ws://127.0.0.1:1/devtools/browser/x".into(),
                    pid,
                },
                tmp.path().join("udd"),
                session,
            ),
        );

        let launch = SessionLaunch::headless_default();
        let out = driver
            .run(
                session,
                LaunchPolicy::OpenIfNeeded(&launch),
                &["tab-list"],
                Duration::from_secs(10),
            )
            .await;
        assert!(
            out.is_ok(),
            "the verb must succeed by re-attaching to the live browser, got {:?}",
            out.err()
        );

        // The three facts, in increasing order of how hard they are to fake:
        // the record survived, the driver still calls it alive, and the OS
        // still has that pid (`shutdown` kills AND reaps, so a torn-down child
        // is gone rather than a zombie `kill -0` would still accept).
        assert_eq!(
            driver.endpoint(session).map(|e| e.pid),
            Some(pid),
            "the live browser was torn down and replaced"
        );
        assert!(driver.chromium_alive(session));
        let still_there = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("kill -0");
        assert!(still_there.success(), "pid {pid} is gone: it was killed");

        assert_eq!(
            driver.shutdown_all_chromium(),
            1,
            "cleanup killed the child"
        );
        // `attach_once` writes a real `--config` under the aleph home; this
        // session key exists only for this test, so take its state with it.
        let _ = tokio::fs::remove_file(
            super::super::playwright_launch::config_path_for(session).expect("home resolves"),
        )
        .await;
        let _ = tokio::fs::remove_dir_all(
            super::super::playwright_launch::output_dir_for(session).expect("home resolves"),
        )
        .await;
    }

    /// The sealed test twin must stay sealed: a unit test may not install a
    /// runtime, and now it may not launch a Chromium either.
    #[tokio::test]
    async fn an_unconfigured_driver_still_refuses_to_reach_outside_the_process() {
        let driver = PlaywrightCliDriver::new(
            PlaywrightCliConfig::default(),
            crate::browser::profile::BrowserRuntimeConfig::default(),
        );
        assert!(matches!(
            driver.resolve_binary().await,
            Err(BrowserError::PlaywrightCliNotInstalled)
        ));
        assert!(driver.endpoint("default").is_none());
        assert!(!driver.chromium_alive("default"));
        assert!(!driver.shutdown_chromium("default"));
    }
}
