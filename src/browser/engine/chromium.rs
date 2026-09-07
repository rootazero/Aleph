//! Aleph launches Chromium; `playwright-cli` only attaches to what it finds.
//!
//! Why this module exists at all is a measurement, not a preference. The Chrome
//! spike (`docs/superpowers/specs/2026-09-05-browser-live-view-evidence/`)
//! established that a CLI-launched Chrome *does* open a debug port — and that
//! the port is useless as a contract: it is random per launch, a user-supplied
//! `--remote-debugging-port` loses to Playwright's own (Chrome takes the last
//! occurrence), no `DevToolsActivePort` file is written into Playwright's
//! profile dir, and `playwright-cli list` prints no endpoint. The only
//! discovery route left was scraping `ps`. Launching it ourselves replaces all
//! of that with a file Chrome writes on purpose.
//!
//! The second consequence is ownership: under `attach --cdp`, `playwright-cli
//! close` disconnects and leaves the browser running (measured: 9 Chrome
//! processes before and after, endpoint still serving, page still on its URL).
//! So the browser's life is ours to end, which is what [`ChromiumChild`] and
//! [`reap_orphans`] are for.
//!
//! Moved here from `chromium_launch.rs` when the second engine arrived. What
//! stayed behind in `super::process` is what both engines need; what is here
//! is Chromium-shaped and nothing else: the `DevToolsActivePort` file, the
//! `--use-mock-keychain` argv, the owner-only user-data-dir.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::process::{
    sidecar_path, terminate, write_sidecar_record, CdpEndpoint, EngineProcess, LaunchRequest,
    Launched,
};
use super::Engine;
use crate::browser::error::BrowserError;
use crate::browser::profile::BrowserRuntimeConfig;
use crate::utils::no_window::NoWindow;

/// How long the `DevToolsActivePort` file may take to appear before the launch
/// is called failed.
///
/// A cold Chrome on a loaded machine is slow, and the spike never measured this
/// window (it read the file after the fact) — so the number is chosen to match
/// the repo's existing answer to "how long may bringing up a browser take":
/// `playwright_cli::SESSION_START_TIMEOUT_SECS` and `chrome_mcp`'s
/// `create_session` both say 60 s. Half of it is the budget for the *port*,
/// which appears well before the browser is usable.
pub(crate) const DEVTOOLS_PORT_DEADLINE: Duration = Duration::from_secs(30);

/// How often the port file is polled while waiting.
const PORT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Chrome's own file, written into the user-data-dir. Name fixed by Chrome.
const DEVTOOLS_PORT_FILE: &str = "DevToolsActivePort";

/// Everything the Chromium process needs at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChromiumLaunchSpec {
    pub binary: PathBuf,
    pub user_data_dir: PathBuf,
    pub headless: bool,
    pub proxy: Option<String>,
    pub extra_args: Vec<String>,
}

impl ChromiumLaunchSpec {
    /// The full argv, operator args first.
    ///
    /// Order is the contract. Chrome resolves a duplicated switch to its LAST
    /// occurrence — that is precisely how Playwright's own
    /// `--remote-debugging-port=58419` beat a caller-supplied `=0` in the spike.
    /// So `extra_args` lead and every switch this launch depends on follows
    /// them, where an operator's duplicate cannot displace it. The URL is last
    /// because it is positional.
    pub(crate) fn argv(&self) -> Vec<String> {
        let mut argv = self.extra_args.clone();
        argv.push("--no-first-run".to_string());
        argv.push("--no-default-browser-check".to_string());
        // Chrome asks the OS credential store for the key that encrypts its
        // profile, and on macOS that call NEVER RETURNS when HOME has no
        // usable login Keychain (a launchd service, a service account, a
        // sandboxed host, this repo's own scratch-HOME QA). The browser still
        // answers /json/version, so it looks healthy; what dies is the first
        // navigation in each page — Chrome emits requestWillBeSent, the origin
        // server never sees the request, and Page.navigate is never answered.
        //
        // These are not a preference: while `playwright-cli` launched the
        // browser its own launcher always passed them, so moving the launch
        // here (4c208760a) silently dropped them. Measured on macOS: the mock
        // keychain is the load-bearing one, `--password-store=basic` alone
        // changes nothing. The latter is kept for the Linux twin, where the
        // blocking store is gnome-keyring/kwallet rather than the Keychain.
        //
        // AFTER `extra_args`, like every other switch this launch depends on:
        // Chrome resolves a duplicated switch to its last occurrence, so an
        // operator cannot displace it.
        argv.push("--use-mock-keychain".to_string());
        argv.push("--password-store=basic".to_string());
        if self.headless {
            argv.push("--headless=new".to_string());
        }
        if let Some(proxy) = &self.proxy {
            argv.push(format!("--proxy-server={proxy}"));
        }
        argv.push(format!("--user-data-dir={}", self.user_data_dir.display()));
        argv.push("--remote-debugging-port=0".to_string());
        // `about:blank` keeps the launch out of the SSRF guard's way; the
        // caller navigates afterwards through the guarded path. Same reasoning
        // the deleted `open_argv` carried.
        argv.push("about:blank".to_string());
        argv
    }
}

/// Parse Chrome's two-line `DevToolsActivePort`: the port, then the browser
/// websocket path.
///
/// Returns `None` for every shape that is not both lines — which is the normal
/// state while Chrome is still writing the file. That `None` means "not yet",
/// and the poll loop is the only thing allowed to spend it; nothing may read it
/// as "failed" (判据 §8).
pub(crate) fn parse_devtools_active_port(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    if port == 0 {
        return None;
    }
    let path = lines.next()?.trim();
    if !path.starts_with('/') {
        return None;
    }
    Some((port, path.to_string()))
}

/// [`parse_devtools_active_port`] plus the pid, as one endpoint.
pub(crate) fn endpoint_from_port_file(text: &str, pid: u32) -> Option<CdpEndpoint> {
    let (port, path) = parse_devtools_active_port(text)?;
    Some(CdpEndpoint {
        http_url: format!("http://127.0.0.1:{port}"),
        ws_url: format!("ws://127.0.0.1:{port}{path}"),
        pid,
    })
}

/// Restrict a chromium user-data-dir to owner-only, if the platform has a
/// bit for it.
///
/// `argv()` passes `--use-mock-keychain` (load-bearing — see its own doc
/// comment), so every cookie/credential Chrome writes under here is
/// encrypted with a constant, publicly-known key rather than one the OS
/// keychain guards. The directory permission bit is the ONLY thing standing
/// between another local account and that plaintext-equivalent data, for a
/// udd that — unlike the old per-launch tmp profile — now persists across
/// restarts for every Managed profile (Final Review M7).
///
/// Re-asserted unconditionally on every `spawn`, not just on first create: a
/// udd from before this fix existed would otherwise keep whatever mode
/// `create_dir_all` gave it (umask-dependent, not owner-only) forever.
///
/// Windows has no equivalent bit this crate sets today (ACLs are a separate,
/// larger mechanism) — deliberately a no-op there, not an omission.
async fn restrict_udd_to_owner(dir: &Path) -> Result<(), BrowserError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|e| BrowserError::LaunchFailed {
                stage: "spawn",
                detail: format!(
                    "cannot restrict the chromium user-data-dir {} to owner-only: {e}",
                    dir.display()
                ),
            })?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// One Chromium process owned by this Aleph.
pub(crate) struct ChromiumChild {
    child: Child,
    endpoint: CdpEndpoint,
    user_data_dir: PathBuf,
    session_key: String,
}

impl ChromiumChild {
    /// Launch Chromium and wait for it to publish its debug port.
    ///
    /// `session_key` is the profile name, and it is taken here rather than
    /// derived, because it is what names this browser's record in the sidecar
    /// registry — the only thing that can find the process again after a crash.
    pub(crate) async fn spawn(
        spec: &ChromiumLaunchSpec,
        session_key: &str,
        deadline: Duration,
    ) -> Result<Self, BrowserError> {
        tokio::fs::create_dir_all(&spec.user_data_dir)
            .await
            .map_err(|e| BrowserError::LaunchFailed {
                stage: "spawn",
                detail: format!(
                    "cannot create the chromium user-data-dir {}: {e}",
                    spec.user_data_dir.display()
                ),
            })?;
        restrict_udd_to_owner(&spec.user_data_dir).await?;
        // A leftover file from the PREVIOUS launch would be read as this one's
        // endpoint — a port that is either closed or, worse, somebody else's.
        let port_file = spec.user_data_dir.join(DEVTOOLS_PORT_FILE);
        let _ = tokio::fs::remove_file(&port_file).await;

        let mut cmd = Command::new(&spec.binary);
        cmd.args(spec.argv())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Same discipline as the CLI child (`playwright_cli::spawn`): the
        // browser never needs the parent's credentials, and over-stripping is
        // safe.
        for (name, _) in std::env::vars() {
            if crate::security::secret_env::is_secret_env(&name) {
                cmd.env_remove(&name);
            }
        }
        let mut child = cmd
            .no_window()
            .spawn()
            .map_err(|e| BrowserError::LaunchFailed {
                stage: "spawn",
                detail: format!("{}: {e}", spec.binary.display()),
            })?;
        let pid = child.id();
        // Record intent BEFORE the port is known (判据 §15): if this future is
        // dropped at any await below (an outer timeout, a `select!`, a
        // cancelled task) or Aleph crashes before the loop returns, this is
        // the only trace that a Chromium process exists and needs reaping —
        // `std::process::Child` does not kill on drop (round-1 review finding
        // F2). The reaper decides on pid + user_data_dir alone, so an
        // endpoint-less record is fully reapable.
        write_sidecar_record(
            Engine::Chromium,
            session_key,
            pid,
            &spec.user_data_dir,
            None,
        )
        .await;

        let started = Instant::now();
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&port_file).await {
                if let Some(endpoint) = endpoint_from_port_file(&text, pid) {
                    let me = Self {
                        child,
                        endpoint,
                        user_data_dir: spec.user_data_dir.clone(),
                        session_key: session_key.to_string(),
                    };
                    me.write_sidecar().await;
                    tracing::info!(pid, endpoint = %me.endpoint.http_url, "chromium launched");
                    return Ok(me);
                }
            }
            // Chrome died before publishing: a different fact from "the file is
            // late", and the operator's fix is different too (a missing shared
            // library, a bad `--user-data-dir`, a crashed sandbox).
            if let Ok(Some(status)) = child.try_wait() {
                return Err(BrowserError::LaunchFailed {
                    stage: "chromium-exit",
                    detail: format!(
                        "{} exited with {status} before writing {DEVTOOLS_PORT_FILE}",
                        spec.binary.display()
                    ),
                });
            }
            if started.elapsed() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BrowserError::LaunchFailed {
                    stage: "devtools-port",
                    detail: format!(
                        "no {DEVTOOLS_PORT_FILE} under {} after {}s",
                        spec.user_data_dir.display(),
                        deadline.as_secs()
                    ),
                });
            }
            tokio::time::sleep(PORT_POLL_INTERVAL).await;
        }
    }

    /// Build a `ChromiumChild` around a process this module did not spawn.
    ///
    /// The test seam for everything downstream of a launch. `spawn` is the only
    /// other constructor and it launches a real browser, so without this every
    /// caller that reasons about a live or dead child — `PlaywrightCliDriver`'s
    /// whole relaunch path — is untestable, and an untestable path is where the
    /// review found a `run` that killed live browsers while a full mutation
    /// sweep of the pure predicate beside it stayed green.
    ///
    /// A test passes any process it can observe (`sleep` for alive, an exited
    /// one for dead); nothing here assumes the child is a browser.
    ///
    /// `#[cfg(test)]` on purpose: this is `--lib` unit-test scaffolding, not an
    /// injection point production code may use — a second way to obtain a
    /// `ChromiumChild` that skips the port file would be a second answer to
    /// "where does an endpoint come from". **Task 6's reaper tests consume this
    /// same seam**; do not add a second one.
    #[cfg(test)]
    pub(crate) fn from_parts(
        child: Child,
        endpoint: CdpEndpoint,
        user_data_dir: PathBuf,
        session_key: &str,
    ) -> Self {
        Self {
            child,
            endpoint,
            user_data_dir,
            session_key: session_key.to_string(),
        }
    }

    pub(crate) const fn endpoint(&self) -> &CdpEndpoint {
        &self.endpoint
    }

    /// Give up the `Child` handle so a [`Launched`] can carry it.
    ///
    /// The ONLY way the handle leaves a `ChromiumChild`, and it consumes
    /// `self`, so there is never a moment when two values could both `wait()`
    /// the same process. Used once, by `ChromiumLauncher::launch`; the
    /// `playwright-cli` driver keeps holding whole `ChromiumChild`s and is
    /// untouched.
    pub(crate) fn into_child_handle(self) -> Child {
        self.child
    }

    /// Whether the browser is still running.
    ///
    /// `Err` from `try_wait` is answered **`true`**, deliberately. "I could not
    /// tell" is not "it is dead", and killing on an unknown would orphan a live
    /// browser. The attach that follows settles it for free: a dead endpoint
    /// answers `ECONNREFUSED` and the driver's retry path forgets the child.
    pub(crate) fn alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) | Err(_) => true,
            Ok(Some(_)) => false,
        }
    }

    /// Kill the process only — do NOT touch the by-key sidecar record.
    ///
    /// For exactly one caller: `ensure_chromium`'s "cannot happen while the
    /// per-session lock is held" arm, which fires only if the map already
    /// held a child for this SAME session key at the moment a brand-new one
    /// was inserted. [`Self::shutdown`]'s sidecar deletion is keyed by
    /// `session_key`, not by pid — calling it there would delete the record
    /// the just-inserted, live child wrote for itself one line above,
    /// making the NEW browser unreapable across a crash forever (Final
    /// Review M1). This kills the stale process and leaves every record
    /// alone.
    pub(crate) fn kill_only(mut self) {
        let pid = self.endpoint.pid;
        match self.child.kill() {
            Ok(()) => {
                let _ = self.child.wait();
                tracing::info!(
                    pid,
                    "stale chromium (replaced under its own session key) killed"
                );
            }
            Err(e) => {
                tracing::warn!(pid, error = %e, "could not kill stale chromium; leaving it")
            }
        }
    }

    /// Kill the browser and clear its registry record.
    ///
    /// `wait()` runs **only after a successful `kill()`**. It is a blocking
    /// call and every production caller is inside async code: after a kill the
    /// reap is immediate, but `kill()` can fail (EPERM, or the child was
    /// already reaped) and then `wait()` would park a tokio worker until the
    /// process happened to exit on its own.
    pub(crate) fn shutdown(mut self) {
        let pid = self.endpoint.pid;
        match self.child.kill() {
            Ok(()) => {
                let _ = self.child.wait();
                tracing::info!(pid, "chromium shut down");
            }
            // Say which of the two happened rather than logging "shut down"
            // over a process that may still be running: an untrue log line is
            // the thing a reader would spend as evidence.
            Err(e) => tracing::warn!(pid, error = %e, "could not kill chromium; leaving it"),
        }
        match sidecar_path(&self.session_key) {
            Ok(path) => {
                let _ = std::fs::remove_file(path);
            }
            Err(e) => tracing::warn!(error = %e, "cannot resolve the chromium sidecar to remove"),
        }
    }

    /// Rewrite this profile's sidecar now that the endpoint is known — see
    /// [`write_sidecar_record`].
    async fn write_sidecar(&self) {
        write_sidecar_record(
            Engine::Chromium,
            &self.session_key,
            self.endpoint.pid,
            &self.user_data_dir,
            Some(self.endpoint.http_url.clone()),
        )
        .await;
    }
}

/// The [`EngineProcess`] implementation for Chromium.
///
/// Stateless apart from the runtime configuration it resolves the binary
/// from. **R43**: resolution is deferred to [`Self::launch`] rather than
/// pinned at construction time — `chromium_resolve::resolve_binary` is
/// `async` and may install ~150 MB on first use, and `chromium_resolve.rs`'s
/// own doc says it never installs anything inside this tool's 180 s budget. A
/// `binary: PathBuf` field would force resolution at boot (`ProfileManager::new`
/// is sync), making `binary_path` / `prefer_system_browser` / `download_host`
/// unreachable on this path.
///
/// The launched process's handle lives in the [`Launched`] this returns, not
/// in a map here: the caller (Task 9's `EngineHandle`) already keeps that
/// value for the connection's whole life, and a second, pid-keyed copy here
/// would be two owners of one `Child` — where whichever one `wait()`s first
/// leaves the other holding a handle to a pid the OS may already have
/// reissued.
///
/// **Not a second way to obtain an endpoint.** Every launch still goes through
/// [`ChromiumChild::spawn`] and the `DevToolsActivePort` file; this type
/// re-packages the result and nothing else. (Compare `ChromiumChild::from_parts`'
/// doc, which refuses a second endpoint source.)
///
/// **Which** Chromium is the request's to say, not this type's (R68).
/// [`Self::launch`] resolves for `req.browser`, so a profile configured for
/// Chrome, Brave or Edge reaches `chromium_resolve::resolve_binary` as itself
/// — the same way `playwright_cli.rs:421` passes `&launch.browser`. Resolving
/// `BrowserType::default()` here instead would make the profile's `browser`
/// field unreachable on this path: an Edge profile would launch Chromium and
/// nothing would report that the setting had been ignored, which is a no-op
/// that reports success (判据 §11) in a user-facing setting. Pinned by
/// `the_launch_source_still_names_the_requests_own_browser_type`.
pub struct ChromiumLauncher {
    runtime: BrowserRuntimeConfig,
}

impl ChromiumLauncher {
    #[must_use]
    pub const fn new(runtime: BrowserRuntimeConfig) -> Self {
        Self { runtime }
    }
}

#[async_trait::async_trait]
impl EngineProcess for ChromiumLauncher {
    fn engine(&self) -> Engine {
        Engine::Chromium
    }

    async fn launch(&self, req: LaunchRequest) -> Result<Launched, BrowserError> {
        if req.stealth {
            // Named, not dropped: a launch that silently discards a requested
            // mode is a no-op that reports success (判据 §11).
            tracing::warn!(
                profile = %req.profile,
                "stealth was requested but is an obscura-only launch mode; the chromium \
                 launch ignores it — switch the profile's engine to obscura for it"
            );
        }
        // `allow_private_network` has no Chromium switch by design: Aleph's
        // own SSRF guard is the gate on that side, and obscura's flag only
        // relaxes obscura's built-in floor (spec §6.2). Not logged, unlike
        // `stealth`, because it is not a mode Chromium could have honoured —
        // the profile's network policy is the gate here either way, so there
        // is no discarded intent to report.

        // R43: resolve the binary now, mirroring `playwright_cli.rs:421`'s
        // `ensure_chromium` call. The CLI binary is looked up the same cheap
        // way the doctor and `runtime_manage` do it (`managed_cli_path`: a
        // `which` PATH walk plus a ledger read, off the async worker) —
        // never provisioned here, since `resolve_binary` promises never to
        // install anything.
        let cli = tokio::task::spawn_blocking(crate::tools::probes::browser::managed_cli_path)
            .await
            .unwrap_or(None)
            .ok_or_else(|| {
                crate::browser::error::engine_unavailable(
                    Engine::Chromium,
                    "no playwright-cli found on PATH or in the runtime ledger",
                )
            })?;
        // R68: the REQUEST's browser, never `BrowserType::default()` — see the
        // type's doc comment. This is the only place a profile's `browser`
        // field can reach the resolver on this path.
        let resolved =
            crate::browser::chromium_resolve::resolve_binary(&self.runtime, &req.browser, &cli)
                .await?;
        let spec = ChromiumLaunchSpec {
            binary: resolved.path,
            user_data_dir: req.data_dir.clone(),
            headless: req.headless,
            proxy: req.proxy.clone(),
            extra_args: req.extra_args.clone(),
        };
        let child = ChromiumChild::spawn(&spec, &req.session_key, DEVTOOLS_PORT_DEADLINE).await?;
        let endpoint = child.endpoint().clone();
        let sidecar_path = sidecar_path(&req.session_key)?;
        Ok(Launched {
            pid: endpoint.pid,
            endpoint,
            sidecar_path,
            child: std::sync::Arc::new(crate::sync_primitives::Mutex::new(Some(
                child.into_child_handle(),
            ))),
        })
    }

    /// R69: delegated, not re-implemented. The kill/reap contract lives in
    /// [`terminate`] (`super::process`) so that this launcher and Task 16's
    /// obscura launcher cannot drift about when a browser may be reported
    /// dead. Nothing Chromium-specific happens on this path — the signal is
    /// SIGKILL and the evidence is the reap, both of which are true of any
    /// child process — so there is nothing left here to override.
    async fn kill(&self, launched: &Launched, grace: Duration) -> Result<bool, BrowserError> {
        terminate(launched, grace).await
    }
}

// What the sidecar record does here. `ChromiumChild::shutdown` also deletes
// the by-key sidecar; `ChromiumLauncher::kill` does not, and that is
// deliberate: the record's job is orphan reclamation across a crash, a
// record whose pid is gone is dropped by the next sweep's `ArgvProbe::Absent`
// arm, and deleting it on a kill that returned `false` would destroy the
// only way that still-live browser could be found again
// (`super::process::reap_orphans`'s rule). Task 9's `EngineHandle::shutdown`
// removes `launched.sidecar_path` **after** a `kill` that answered `true`,
// which is where the ordering belongs.

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ChromiumLaunchSpec {
        ChromiumLaunchSpec {
            binary: PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            user_data_dir: PathBuf::from("/tmp/udd"),
            headless: true,
            proxy: Some("socks5://127.0.0.1:1080".into()),
            extra_args: vec!["--disable-gpu".into()],
        }
    }

    /// Golden vector. The ORDER is the contract, not decoration: Chrome takes
    /// the LAST occurrence of a duplicated switch, which is exactly how
    /// Playwright's own `--remote-debugging-port` beat a user-supplied `=0` in
    /// the spike. So the operator's `extra_args` go FIRST and every switch this
    /// launch depends on goes after them, where a duplicate cannot displace it.
    #[test]
    fn argv_puts_the_contract_switches_after_the_operator_args() {
        assert_eq!(
            spec().argv(),
            vec![
                "--disable-gpu",
                "--no-first-run",
                "--no-default-browser-check",
                "--use-mock-keychain",
                "--password-store=basic",
                "--headless=new",
                "--proxy-server=socks5://127.0.0.1:1080",
                "--user-data-dir=/tmp/udd",
                "--remote-debugging-port=0",
                "about:blank",
            ]
        );
    }

    /// M7: the udd directory must end up owner-only, whatever mode it
    /// started with — `--use-mock-keychain` (below) means the directory
    /// permission bit is the only thing standing between another local
    /// account and the plaintext-equivalent cookie/credential data Chrome
    /// writes under it.
    #[cfg(unix)]
    #[tokio::test]
    async fn restrict_udd_to_owner_locks_down_a_directory_created_wide_open() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("udd");
        std::fs::create_dir(&dir).expect("create dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("chmod wide open");

        restrict_udd_to_owner(&dir)
            .await
            .expect("restrict succeeds");

        let mode = std::fs::metadata(&dir).expect("stat").permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "udd must end up owner-only regardless of the mode create_dir_all gave it"
        );
    }

    /// The hang-rootcause report's own falsifier: `--use-mock-keychain` is
    /// what stops Chrome's first navigation in every page from hanging
    /// forever on a HOME with no usable login Keychain (measured: 60 s vs
    /// 0.6 s with the flag). Presence alone is not the whole contract —
    /// **position** is, because Chrome resolves a duplicated switch to its
    /// LAST occurrence, so an assertion on presence alone would stay green if
    /// someone moved the push to the front, where an operator's `extra_args`
    /// could displace it and silently reproduce the hang.
    #[test]
    fn the_keychain_switches_sit_after_extra_args_so_an_operator_cannot_displace_them() {
        let with_extra = ChromiumLaunchSpec {
            extra_args: vec!["--a".into(), "--b".into(), "--c".into()],
            ..spec()
        };
        let extra_len = with_extra.extra_args.len();
        let argv = with_extra.argv();
        // `rposition`, not `position`: Chrome resolves a duplicated switch to
        // its LAST occurrence, so the occurrence that decides behaviour is
        // the one this assertion must be about. With no duplicate present
        // (this test's `extra_args` has none) the two agree — the duplicate
        // case that actually distinguishes them lives in the next test.
        let keychain_idx = argv
            .iter()
            .rposition(|a| a == "--use-mock-keychain")
            .expect("--use-mock-keychain must be present");
        let store_idx = argv
            .iter()
            .rposition(|a| a == "--password-store=basic")
            .expect("--password-store=basic must be present");
        assert!(
            keychain_idx >= extra_len,
            "--use-mock-keychain came before extra_args ended: {argv:?}"
        );
        assert!(
            store_idx >= extra_len,
            "--password-store=basic came before extra_args ended: {argv:?}"
        );
    }

    /// The teeth `rposition` exists for: swapping `position` → `rposition`
    /// above changes nothing when `extra_args` carries no duplicate (there is
    /// only one occurrence, so both functions agree) — that guard alone would
    /// be trivially true. Here `extra_args` ALREADY contains
    /// `--use-mock-keychain`, simulating an operator's own duplicate, and the
    /// assertion must still hold: the disjunction must find OUR occurrence —
    /// the one Chrome actually honours, being last — not the operator's
    /// earlier one. `position` would find the operator's copy here (an index
    /// inside `extra_args`) and this test would wrongly redden even though
    /// the launch is correct; that is the false failure `rposition` fixes.
    #[test]
    fn an_operators_duplicate_use_mock_keychain_does_not_hide_ours() {
        let with_extra = ChromiumLaunchSpec {
            extra_args: vec!["--use-mock-keychain".into()],
            ..spec()
        };
        let extra_len = with_extra.extra_args.len();
        let argv = with_extra.argv();
        assert_eq!(
            argv.iter().filter(|a| *a == "--use-mock-keychain").count(),
            2,
            "expected the operator's duplicate plus ours: {argv:?}"
        );
        let winning_idx = argv
            .iter()
            .rposition(|a| a == "--use-mock-keychain")
            .expect("--use-mock-keychain must be present");
        assert!(
            winning_idx >= extra_len,
            "the operator's duplicate in extra_args, not ours, would win: {argv:?}"
        );
    }

    #[test]
    fn a_headed_launch_omits_the_headless_switch_and_a_proxyless_one_the_proxy() {
        let argv = ChromiumLaunchSpec {
            headless: false,
            proxy: None,
            extra_args: Vec::new(),
            ..spec()
        }
        .argv();
        assert!(!argv.iter().any(|a| a.starts_with("--headless")));
        assert!(!argv.iter().any(|a| a.starts_with("--proxy-server")));
        assert_eq!(argv.last().map(String::as_str), Some("about:blank"));
        assert!(argv.contains(&"--remote-debugging-port=0".to_string()));
    }

    /// The real two-line file, verbatim from the Chrome spike
    /// (`docs/superpowers/specs/2026-09-05-browser-live-view-evidence/chrome-spike-findings.md`
    /// STEP 1): a port on line 1, the browser path on line 2.
    #[test]
    fn the_real_port_file_parses_into_a_port_and_a_browser_path() {
        let text = "58363\n/devtools/browser/ac5f508a-1111-2222-3333-444455556666\n";
        assert_eq!(
            parse_devtools_active_port(text),
            Some((
                58363,
                "/devtools/browser/ac5f508a-1111-2222-3333-444455556666".to_string()
            ))
        );
        let ep = endpoint_from_port_file(text, 4242).expect("endpoint");
        assert_eq!(ep.http_url, "http://127.0.0.1:58363");
        assert_eq!(
            ep.ws_url,
            "ws://127.0.0.1:58363/devtools/browser/ac5f508a-1111-2222-3333-444455556666"
        );
        assert_eq!(ep.pid, 4242);
    }

    /// A half-written file is the NORMAL state during the poll — Chrome creates
    /// it and fills it in. Every partial shape must read as "not yet", never as
    /// an endpoint: `Option::None` here is the "I do not know yet" answer the
    /// poll loop is allowed to spend, and a `Some` built from half a file would
    /// hand `attach --cdp` a URL that cannot connect.
    #[test]
    fn every_partial_or_malformed_port_file_reads_as_not_yet() {
        for bad in [
            "",
            "\n",
            "58363",                     // port written, path not yet
            "58363\n",                   // ditto, with the newline
            "58363\ndevtools/browser/x", // path must be absolute
            "notaport\n/devtools/browser/x",
            "0\n/devtools/browser/x", // port 0 is never a listening port
            "99999999\n/devtools/browser/x",
        ] {
            assert_eq!(parse_devtools_active_port(bad), None, "accepted {bad:?}");
            assert!(
                endpoint_from_port_file(bad, 1).is_none(),
                "accepted {bad:?}"
            );
        }
    }

    /// Final Review M1: `kill_only` exists precisely because `shutdown`'s
    /// sidecar deletion is keyed by `session_key`, not by pid — and the one
    /// caller (`ensure_chromium`'s "cannot happen" arm) fires only when a
    /// DIFFERENT, brand-new child was just inserted under that SAME key,
    /// one line before. `shutdown` there would delete the record the new,
    /// live child just wrote for itself. Modelled directly: write a sidecar
    /// under `session_key` (standing in for "the new child's own record"),
    /// kill a stand-in stale child under that same key, and require the
    /// sidecar to survive.
    #[cfg(unix)]
    #[test]
    fn kill_only_kills_the_process_and_never_touches_the_sidecar() {
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let session = "kill-only-guard";
        let sidecar = sidecar_path(session).expect("home resolves");
        std::fs::create_dir_all(sidecar.parent().expect("sidecar has a parent"))
            .expect("create sidecar dir");
        std::fs::write(&sidecar, br#"{"pretend":"the NEW child's own record"}"#)
            .expect("write sidecar fixture");

        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn the stand-in stale browser");
        let pid = child.id();
        let stale = ChromiumChild::from_parts(
            child,
            CdpEndpoint {
                http_url: "http://127.0.0.1:1".into(),
                ws_url: "ws://127.0.0.1:1/devtools/browser/x".into(),
                pid,
            },
            std::path::PathBuf::from("/tmp/udd-does-not-matter"),
            session,
        );

        stale.kill_only();

        let still_there = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("kill -0");
        assert!(
            !still_there.success(),
            "kill_only must actually kill the stale process"
        );
        assert!(
            sidecar.exists(),
            "kill_only must not delete the by-key sidecar — that record belongs to \
             whatever child is CURRENTLY installed under this session key, not to \
             the stale one being killed"
        );
    }

    /// A `Launched` around a process this test owns, so the kill path can be
    /// driven without a real browser.
    #[cfg(unix)]
    fn launched_around(child: std::process::Child) -> Launched {
        let pid = child.id();
        Launched {
            pid,
            endpoint: CdpEndpoint {
                http_url: "http://127.0.0.1:1".into(),
                ws_url: "ws://127.0.0.1:1/devtools/browser/x".into(),
                pid,
            },
            sidecar_path: PathBuf::from("/tmp/sidecar-does-not-matter"),
            child: std::sync::Arc::new(crate::sync_primitives::Mutex::new(Some(child))),
        }
    }

    /// `kill` must report the EFFECT, and the effect is "exited AND reaped".
    ///
    /// `kill -0` is the assertion precisely because it succeeds on a
    /// **zombie**: a child killed without a `wait()` still holds its
    /// process-table entry, so this assertion fails unless the reap really
    /// happened. That is the whole reason `Launched` carries the handle.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_launcher_kills_its_child_and_reaps_it_before_saying_so() {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = child.id();
        let launched = launched_around(child);
        let launcher = ChromiumLauncher::new(BrowserRuntimeConfig::default());
        assert_eq!(launcher.engine(), Engine::Chromium);

        assert!(
            launcher
                .kill(&launched, std::time::Duration::from_secs(5))
                .await
                .expect("kill"),
            "a child we killed and reaped must be reported as gone"
        );
        let still_there = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("kill -0");
        assert!(
            !still_there.success(),
            "the process is still in the table — it was signalled but never \
             waited on, i.e. it is a zombie for the rest of this daemon's life"
        );
        assert!(
            launched
                .child
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none(),
            "a successful kill must leave the handle settled"
        );
    }

    /// Killing twice is safe and answers `true` both times.
    ///
    /// The second call finds an emptied slot and must NOT signal anything: by
    /// then the pid may have been reissued to an unrelated process, and a
    /// second `Child::kill()` on a reaped handle is exactly how a daemon kills
    /// a stranger. `EngineHandle` shutdown racing an explicit close is the
    /// real caller.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_second_kill_is_a_no_op_that_still_answers_gone() {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn the stand-in browser");
        let launched = launched_around(child);
        let launcher = ChromiumLauncher::new(BrowserRuntimeConfig::default());

        assert!(launcher
            .kill(&launched, std::time::Duration::from_secs(5))
            .await
            .expect("first kill"));
        assert!(
            launcher
                .kill(&launched, std::time::Duration::ZERO)
                .await
                .expect("second kill"),
            "an already-reaped launch is gone, and a zero grace must not turn \
             that determinate answer into a false"
        );
    }

    /// R68: the launch SOURCE still names the request's `BrowserType`.
    ///
    /// The name says `source` and `names` rather than `resolves` because
    /// the verb is the half that travels: a reader who greps a failure
    /// sees the name long before this doc, and `resolves` would promise an
    /// effect this cannot observe. Mirrors
    /// `manager.rs`'s `the_boot_hook_still_calls_the_orphan_sweep`, which
    /// is the same instrument for the same reason.
    ///
    /// **A SOURCE pin, and the report says so rather than implying more.**
    /// `launch` reaches `resolve_binary` only after `managed_cli_path` finds a
    /// real `playwright-cli`, and `resolve_binary` then walks the filesystem
    /// for an installed browser — so no unit test can observe the argument
    /// arriving without a provisioned toolchain and a browser on disk. Same
    /// shape and the same justification as
    /// `manager.rs`'s `the_boot_hook_still_calls_the_orphan_sweep`.
    ///
    /// What this therefore proves is that the argument EXPRESSION is
    /// `&req.browser`, not that the resolver honoured it. That is exactly the
    /// regression being guarded: the defect it replaces was a literal
    /// `&BrowserType::default()` at this call, which no runtime assertion in
    /// this crate could ever have caught either.
    ///
    /// Comment lines are stripped before the negative assertion, because both
    /// the type's doc comment and the call's own comment name
    /// `BrowserType::default()` in order to say "never this" — a census that
    /// read them would be reading the explanation as if it were the code
    /// (判据 §1: the comment is the side that lies).
    #[test]
    fn the_launch_source_still_names_the_requests_own_browser_type() {
        let src = include_str!("chromium.rs").replace('\r', "");
        let production = crate::utils::source_scan::production_prefix(&src);
        assert!(
            production.len() < src.len(),
            "the #[cfg(test)] bound matched nothing — this test would then be \
             reading its own source"
        );

        let code: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");

        assert!(
            flat.contains("resolve_binary(&self.runtime, &req.browser, &cli)"),
            "the chromium launch must resolve the request's own BrowserType — \
             without it a profile set to Brave or Edge silently launches \
             Chromium and nothing reports the setting was ignored"
        );
        assert!(
            !flat.contains("BrowserType::default()"),
            "a BrowserType::default() survives in production code; the \
             profile's browser field is unreachable again"
        );
    }
}
