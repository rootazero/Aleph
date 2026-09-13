//! Launching obscura, and knowing which process owns the port it answers on.
//!
//! Every measurement below was re-taken on the real `v0.2.2` binary at this
//! task's BASE (`obscura 0.2.2`, `aarch64-macos`), not carried over from the
//! spike — 判据 §18 wants the predicate and the build a number was taken under.
//!
//! # Why Aleph picks the port
//!
//! `obscura serve --port 0` binds an ephemeral port and then reports the
//! *requested* one in both announcements. Measured:
//!
//! ```text
//! $ obscura serve --port=0 --storage-dir=./store     # stdout
//!   CDP server: ws://127.0.0.1:0/devtools/browser
//! $ lsof -a -nP -p <pid> -iTCP -sTCP:LISTEN
//!   obscura … TCP 127.0.0.1:50455 (LISTEN)
//! $ curl -s http://127.0.0.1:50455/json/version
//!   … "webSocketDebuggerUrl": "ws://127.0.0.1:0/devtools/browser"
//! ```
//!
//! The only place the real port exists is the listening socket, so there is no
//! "read the endpoint back" path to write — that URL cannot connect. Aleph
//! reserves an ephemeral port itself, passes it, and the announcements then
//! agree with reality.
//!
//! # Why the stdout banner is never waited on
//!
//! It is printed BEFORE the bind is attempted. Launch obscura on a port
//! somebody else holds and the banner still appears on stdout, then
//! `Error: bind 127.0.0.1:50455: Address already in use (os error 48)` on
//! stderr, then exit 1 (measured). A wait on that line would be a readiness
//! signal that is true even when the server never came up — a 恒真 predicate
//! wearing a readiness signal's clothes (判据 §2). Readiness here is
//! `/json/version` answering AND the child still being alive.
//!
//! # Why the port cannot belong to somebody else
//!
//! TCP bind on `127.0.0.1:<p>` is exclusive, so at most one process holds it.
//! If another process took the port between our reservation and the spawn,
//! obscura fails to bind and exits non-zero with the message above — which is
//! what makes "the child is still alive after `/json/version` answered"
//! evidence rather than hope. [`probe_port_owner`] adds the direct OS-level
//! confirmation on top; when the probing tool itself cannot run that is an
//! absent instrument, not a refutation, and the launch proceeds with a warning
//! (判据 §8, §18).
//!
//! # stdio: a file, not a pipe, and no endpoint line is taken from it
//!
//! stdin null; stdout and stderr to `<data_dir>/serve.log`, truncated per
//! launch. NOT a pipe: there is no endpoint line worth reading, and an
//! undrained pipe blocks the child once the OS buffer fills — obscura is
//! chatty, logging a warning per page script (its own `--quiet` help says so).
//! NOT `/dev/null` either: the bind-failure text above is the single most
//! useful diagnostic this launcher can hand an operator, and dropping it would
//! leave `LaunchFailed` holding an exit code and nothing else. A file has no
//! backpressure and keeps the text.
//!
//! The cost, stated: one log file per profile, truncated on each launch rather
//! than rotated. That is the cheap answer and the honest one to write down.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use super::process::{
    restrict_to_owner, sidecar_path, terminate, write_sidecar_record, CdpEndpoint, ChildHandle,
    EngineProcess, LaunchRequest, Launched,
};
use super::Engine;
use crate::browser::error::BrowserError;
use crate::browser::profile::{ObscuraRuntimeConfig, ObscuraVariant};
use crate::utils::no_window::NoWindow;

/// obscura's own stdout+stderr, inside the profile's data directory.
pub const OBSCURA_LOG: &str = "serve.log";

/// How long to wait for `/json/version` after the spawn.
const ENDPOINT_DEADLINE: Duration = Duration::from_secs(30);
const ENDPOINT_POLL: Duration = Duration::from_millis(50);

/// How long a single readiness probe may hang before the loop re-checks
/// whether the child is even alive.
const ENDPOINT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The exact argv, as an ordered token vector.
///
/// Every valued flag uses the `=` form so it is ONE token. That is not style:
/// the orphan sweep matches whole argv elements
/// ([`super::process::argv_names_dir`] — a substring scan kills the
/// neighbouring profile's browser, and macOS `sysinfo::cmd()` bleeds
/// environment words into argv), so `--storage-dir=<dir>` as one token is what
/// makes an obscura orphan reapable at all.
///
/// **`storage_dir` is `req.data_dir` itself, with nothing appended.** The
/// engine leaf is already in that path — `ProfileManager::launch_request_for`
/// builds it as `browser_state_dir(engine.data_subdir())/<profile>` — and the
/// sweep compares this token against the sidecar's recorded `data_dir`, which
/// is that same value. Appending a second `obscura/` leaf here would compile,
/// launch, and make every obscura orphan permanently unreapable, with nothing
/// anywhere reporting it (判据 §7: two ends complete, no wire between them).
///
/// `--host=127.0.0.1` is passed explicitly even though it is already the
/// default. A CDP endpoint reachable off-loopback is remote code execution on
/// this machine, and inheriting a default for a security boundary is how that
/// boundary moves without anyone editing Aleph.
///
/// `--allow-file-access` is never passed, on any path (spec §7.2). There is no
/// parameter for it, so there is nothing to get wrong.
///
/// `req.headless` has no counterpart: obscura has no window and no `--headless`
/// flag. [`ObscuraLauncher::launch`] warns once when a profile asks for
/// `headless = false`.
#[must_use]
pub fn obscura_argv(
    port: u16,
    storage_dir: &Path,
    req: &LaunchRequest,
    variant: ObscuraVariant,
) -> Vec<String> {
    let mut argv = vec![
        "serve".to_string(),
        "--host=127.0.0.1".to_string(),
        format!("--port={port}"),
        format!(
            "{}={}",
            Engine::Obscura.data_dir_flag(),
            storage_dir.display()
        ),
    ];
    if matches!(variant, ObscuraVariant::Stealth) {
        argv.push("--stealth".to_string());
    }
    if req.allow_private_network {
        argv.push("--allow-private-network".to_string());
    }
    if let Some(proxy) = &req.proxy {
        argv.push(format!("--proxy={proxy}"));
    }
    argv.extend(req.extra_args.iter().cloned());
    argv
}

/// Reserve an ephemeral loopback port by binding it and letting it go.
///
/// The window between the drop and obscura's own bind is real, and it is
/// closed on the far side rather than here: a loser gets `Address already in
/// use` and exits 1, which [`launch_with`] reports as
/// `LaunchFailed{stage: "obscura-exit"}` carrying that text.
pub fn reserve_port() -> Result<u16, BrowserError> {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| BrowserError::LaunchFailed {
            stage: "reserve-port",
            detail: format!("cannot reserve a loopback port for obscura: {e}"),
        })?;
    let port = listener
        .local_addr()
        .map_err(|e| BrowserError::LaunchFailed {
            stage: "reserve-port",
            detail: format!("cannot read the reserved port: {e}"),
        })?
        .port();
    drop(listener);
    Ok(port)
}

/// What an OS-level listener probe learned. Three states, not two, for the
/// same reason [`super::process::ArgvProbe`] has three: "I could not look" and
/// "nobody is there" authorise opposite actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortOwner {
    /// A listener on this port belongs to this pid.
    Pid(u32),
    /// The probe ran and found nothing listening.
    Unbound,
    /// The probe itself could not answer. This is an absent instrument, NOT a
    /// statement about the port.
    Unknown(String),
}

/// Ask the OS who holds `port`.
///
/// unix: `lsof -a -nP -iTCP:<port> -sTCP:LISTEN -Fp`, whose output is `p<pid>`
/// lines — parsed by prefix rather than by column, so a change in lsof's
/// human-readable layout cannot silently produce a wrong pid.
///
/// **The "nothing is listening" reading is `exit 1 with empty stdout`, NOT
/// `empty stderr`.** Measured on this machine at BASE: lsof writes
/// `lsof: WARNING: can't stat() smbfs file system …` to stderr on *every*
/// invocation — success and no-match alike — because one mount cannot be
/// stat'd. A rule that read a non-empty stderr as "the tool complained" would
/// classify every free-port probe here as [`PortOwner::Unknown`], leaving the
/// [`PortOwner::Unbound`] arm unreachable in production: an arm that can never
/// fire is 判据 §2's 恒绿 face. stderr is a diagnostic channel lsof uses even
/// when it succeeds, so it may not be spent as a verdict; it is carried into
/// the `Unknown` text where it is a clue rather than a decision.
///
/// windows: `netstat -ano -p TCP`, matching a `LISTENING` row whose local
/// address ends `:<port>`; the last column is the pid.
#[must_use]
pub fn probe_port_owner(port: u16) -> PortOwner {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("lsof")
            .args(["-a", "-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fp"])
            .output();
        let out = match out {
            Ok(o) => o,
            Err(e) => return PortOwner::Unknown(format!("lsof could not run: {e}")),
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        if let Some(pid) = stdout
            .lines()
            .find_map(|l| l.strip_prefix('p'))
            .and_then(|p| p.trim().parse::<u32>().ok())
        {
            return PortOwner::Pid(pid);
        }
        // lsof's documented "no matches" is exit 1 with nothing on stdout.
        if out.status.code() == Some(1) && stdout.trim().is_empty() {
            return PortOwner::Unbound;
        }
        PortOwner::Unknown(format!(
            "lsof exited with {} and named no listener: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .output();
        let out = match out {
            Ok(o) => o,
            Err(e) => return PortOwner::Unknown(format!("netstat could not run: {e}")),
        };
        if !out.status.success() {
            return PortOwner::Unknown(format!(
                "netstat exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let needle = format!(":{port}");
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            // proto local foreign state pid
            if cols.len() < 5 || !cols[3].eq_ignore_ascii_case("LISTENING") {
                continue;
            }
            if !cols[1].ends_with(&needle) {
                continue;
            }
            if let Ok(pid) = cols[4].parse::<u32>() {
                return PortOwner::Pid(pid);
            }
        }
        PortOwner::Unbound
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = port;
        PortOwner::Unknown("no listener probe on this platform".to_string())
    }
}

/// Turn a probe result into a verdict.
///
/// Split from [`probe_port_owner`] so all three arms are reachable from a test
/// without an OS, and so the *decision* is one function rather than a `match`
/// scattered through the launch.
pub fn settle_port_ownership(port: u16, pid: u32, probe: PortOwner) -> Result<(), BrowserError> {
    match probe {
        PortOwner::Pid(p) if p == pid => Ok(()),
        PortOwner::Pid(other) => Err(BrowserError::LaunchFailed {
            stage: "port-ownership",
            detail: format!(
                "the CDP endpoint on 127.0.0.1:{port} is held by pid {other}, not by the \
                 obscura we launched (pid {pid}). Refusing to drive somebody else's browser."
            ),
        }),
        PortOwner::Unbound => Err(BrowserError::LaunchFailed {
            stage: "port-ownership",
            detail: format!(
                "nothing is listening on 127.0.0.1:{port}, but it answered /json/version a \
                 moment ago — the endpoint moved out from under us (pid {pid})."
            ),
        }),
        // An instrument we do not have says nothing about the port. Exclusive
        // TCP bind plus a child that did not exit with a bind error already
        // establishes ownership; failing here would make obscura unusable in
        // any container without lsof, on the strength of a tool we never ran.
        PortOwner::Unknown(why) => {
            tracing::warn!(
                port,
                pid,
                reason = %why,
                "could not confirm the obscura CDP port's owner at the OS level; \
                 relying on exclusive bind plus a live child"
            );
            Ok(())
        }
    }
}

/// The obscura binary, resolved the same three ways every other runtime is.
///
/// Precedence, and the first arm is deliberately a hard failure: a pin that
/// does not exist is refused rather than fallen back from, because launching a
/// *different* binary than the one the operator named is worse than refusing
/// ([`ObscuraRuntimeConfig::pinned_binary`] states the same rule for the same
/// reason).
///
/// The other two arms mirror `probes::browser::managed_cli_path` exactly —
/// PATH first, then the capability ledger — so "where does a runtime binary
/// come from" has one answer in this codebase rather than two (判据 §16).
fn resolve_obscura_binary(runtime: &ObscuraRuntimeConfig) -> Result<PathBuf, BrowserError> {
    if let Some(pinned) = runtime.pinned_binary() {
        let path = PathBuf::from(pinned);
        if path.is_file() {
            return Ok(path);
        }
        // `engine_unavailable` is the ONE constructor for this error and it
        // owns the remedy sentence; what this call site owns is the `tried`
        // half — "here is where I looked" — which is the only part that can
        // say the pin was the problem. `engine_unavailable_no_launcher` is
        // deliberately NOT used: it is Chromium-only and its remedy is about
        // `playwright-cli`, which has nothing to do with obscura.
        return Err(crate::browser::error::engine_unavailable(
            Engine::Obscura,
            format!(
                "[general.browser.obscura] binary_path points at {pinned}, which is not a \
                 file; a pin is refused rather than fallen back from, because launching some \
                 other obscura than the one you named would be worse than this error"
            ),
        ));
    }
    if let Ok(path) = which::which(crate::runtimes::OBSCURA_RUNTIME) {
        return Ok(path);
    }
    let dir = crate::runtimes::get_runtimes_dir().map_err(|e| {
        crate::browser::error::engine_unavailable(
            Engine::Obscura,
            format!("the runtimes directory could not be resolved: {e}"),
        )
    })?;
    crate::runtimes::CapabilityLedger::load_or_create(dir.join("ledger.json"))
        .executable(crate::runtimes::OBSCURA_RUNTIME)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            crate::browser::error::engine_unavailable(
                Engine::Obscura,
                "not on PATH and not in the runtime ledger",
            )
        })
}

/// One obscura process this Aleph launched.
///
/// Holds the CONFIG rather than a resolved path, the same shape
/// [`super::chromium::ChromiumLauncher`] has: `ProfileManager::new` is
/// synchronous and resolving a binary touches the filesystem, so the
/// resolution has to happen on the launch — which is also the only moment at
/// which "obscura is not installed" is a true statement worth reporting.
pub struct ObscuraLauncher {
    runtime: ObscuraRuntimeConfig,
}

impl ObscuraLauncher {
    #[must_use]
    pub const fn new(runtime: ObscuraRuntimeConfig) -> Self {
        Self { runtime }
    }

    /// Which `serve` mode this launcher runs in.
    ///
    /// **A runtime flag, not a second download.** The upstream release really
    /// does carry `obscura-<arch>-<os>-stealth.tar.gz` (verified against the
    /// real `v0.2.2` release), and the ledger deliberately does not fetch it —
    /// `runtimes::specs::obscura_asset` installs the plain `default` archive
    /// and only that one. So `variant = "stealth"` reaches obscura as
    /// `serve --stealth`.
    ///
    /// The cost, from obscura's own `--help`: on the default build `--stealth`
    /// gives "a consistent browser fingerprint"; TLS impersonation and tracker
    /// blocking need "the `stealth` build feature", which is the archive Aleph
    /// does not install. Half a mode honoured is still a mode honoured, but a
    /// reader expecting TLS impersonation would be wrong, so it is written
    /// down here rather than discovered (判据 §11).
    #[must_use]
    pub const fn variant(&self) -> ObscuraVariant {
        self.runtime.variant
    }
}

#[async_trait]
impl EngineProcess for ObscuraLauncher {
    fn engine(&self) -> Engine {
        Engine::Obscura
    }

    async fn launch(&self, req: LaunchRequest) -> Result<Launched, BrowserError> {
        if !req.headless {
            // obscura has no window and no `--headless`; a profile asking for
            // a headed browser is asking for something this engine cannot do.
            // Warned rather than refused: the useful answer is "you got a
            // headless one", and the escape hatch is a different engine
            // (判据 §11 — a discarded intent is named, not dropped).
            tracing::warn!(
                profile = %req.profile,
                "obscura has no headed mode; headless = false is ignored on this engine \
                 (switch the profile to engine = \"chromium\" for a real window)"
            );
        }
        let runtime = self.runtime.clone();
        let binary = tokio::task::spawn_blocking(move || resolve_obscura_binary(&runtime))
            .await
            .map_err(|e| BrowserError::LaunchFailed {
                stage: "resolve-binary",
                detail: format!("the obscura resolver task failed: {e}"),
            })??;

        tokio::fs::create_dir_all(&req.data_dir)
            .await
            .map_err(|e| BrowserError::LaunchFailed {
                stage: "spawn",
                detail: format!(
                    "cannot create the obscura storage dir {}: {e}",
                    req.data_dir.display()
                ),
            })?;
        // obscura persists cookies and localStorage under this directory with
        // no OS keychain involved, so the owner-only bit Chromium's
        // user-data-dir gets applies here for the same reason — and through
        // the same function, so a future hardening of one is a hardening of
        // both (判据 §16).
        restrict_to_owner(&req.data_dir, "obscura storage dir").await?;

        let port = reserve_port()?;
        let fx = ObscuraEffects {
            binary,
            log_path: req.data_dir.join(OBSCURA_LOG),
            session_key: req.session_key.clone(),
            data_dir: req.data_dir.clone(),
        };
        // One derivation of the record's path, and it is the same one
        // `ChromiumLauncher` uses: `write_sidecar_record` returns `()`, so the
        // path is asked of `sidecar_path` here exactly as it is there. Two
        // launchers deriving it two different ways would be the divergence
        // 判据 §16 warns about; the remaining single-author hazard (the writer
        // and this reader are two callers of one function) is stated in
        // `Launched::sidecar_path`'s own doc.
        let record = sidecar_path(&req.session_key)?;
        let launched = launch_with(&req, self.variant(), port, record, &fx).await?;
        tracing::info!(
            pid = launched.pid,
            endpoint = %launched.endpoint.http_url,
            "obscura launched"
        );
        Ok(launched)
    }

    /// R69: delegated, not re-implemented. The kill/reap contract — SIGKILL
    /// with no handshake, the three answers, what an empty handle means —
    /// lives in [`terminate`] so that this launcher and the Chromium one
    /// cannot drift about when a browser may be reported dead.
    async fn kill(&self, launched: &Launched, grace: Duration) -> Result<bool, BrowserError> {
        terminate(launched, grace).await
    }
}

/// The three effects [`launch_with`] orders, behind one trait.
///
/// `record` and `wait_ready` are `async` **because they are async in
/// production**: one writes a file, the other polls HTTP. A synchronous seam
/// would let a caller satisfy the ordering assertion with a detached
/// `tokio::spawn` or a blocking poll on a tokio worker — a seam whose shape
/// forces the production code to block or detach is the defect, not the
/// convenience (判据 §4, arriving through the test door).
#[async_trait]
pub(crate) trait LaunchEffects: Sync {
    fn spawn(&self, argv: Vec<String>) -> Result<(u32, ChildHandle), BrowserError>;
    /// Must have LANDED when it returns. A caller that detaches this defeats
    /// the ordering the sidecar exists for.
    async fn record(&self, pid: u32, http: Option<String>);
    async fn wait_ready(&self, port: u16, child: &ChildHandle) -> Result<String, BrowserError>;
}

/// The ordering skeleton, with the effects injected.
///
/// Exists so the ORDER — sidecar before the endpoint is known, endpoint before
/// the second sidecar write — is testable without a binary. `ChromiumChild::spawn`
/// states the same rule: if this future is dropped at any await below, the
/// first record is the only trace that a process exists and needs reaping
/// (判据 §15).
pub(crate) async fn launch_with(
    req: &LaunchRequest,
    variant: ObscuraVariant,
    port: u16,
    record: PathBuf,
    fx: &dyn LaunchEffects,
) -> Result<Launched, BrowserError> {
    let argv = obscura_argv(port, &req.data_dir, req, variant);
    let (pid, child) = fx.spawn(argv)?;
    // Intent stamped BEFORE the endpoint is known, and AWAITED: a detached
    // write returns before anything reaches disk, so a cancelled launch or a
    // runtime shutdown in that window leaves an obscura nothing can find
    // again. `ChromiumChild::spawn` awaits its equivalent for the same reason.
    fx.record(pid, None).await;
    // On every early return from here on, the child is still owned by `child`
    // — which is DROPPED, not killed (`std::process::Child` has no
    // kill-on-drop). The record written one line above is what makes it
    // reapable, which is the whole reason that write comes first (判据 §15).
    let ws_url = fx.wait_ready(port, &child).await?;
    let http_url = format!("http://127.0.0.1:{port}");
    fx.record(pid, Some(http_url.clone())).await;
    Ok(Launched {
        pid,
        endpoint: CdpEndpoint {
            http_url,
            ws_url,
            pid,
        },
        sidecar_path: record,
        child,
    })
}

/// The production effects.
struct ObscuraEffects {
    binary: PathBuf,
    log_path: PathBuf,
    session_key: String,
    data_dir: PathBuf,
}

#[async_trait]
impl LaunchEffects for ObscuraEffects {
    fn spawn(&self, argv: Vec<String>) -> Result<(u32, ChildHandle), BrowserError> {
        spawn_obscura(&self.binary, argv, &self.log_path)
    }

    async fn record(&self, pid: u32, http: Option<String>) {
        write_sidecar_record(
            Engine::Obscura,
            &self.session_key,
            pid,
            &self.data_dir,
            http,
        )
        .await;
    }

    async fn wait_ready(&self, port: u16, child: &ChildHandle) -> Result<String, BrowserError> {
        wait_for_endpoint(port, child, &self.log_path).await
    }
}

/// stdio per the module doc: a file, never a pipe, never `/dev/null`.
fn spawn_obscura(
    binary: &Path,
    argv: Vec<String>,
    log_path: &Path,
) -> Result<(u32, ChildHandle), BrowserError> {
    let log = std::fs::File::create(log_path).map_err(|e| BrowserError::LaunchFailed {
        stage: "spawn",
        detail: format!(
            "cannot open {} for obscura's output: {e}",
            log_path.display()
        ),
    })?;
    let errlog = log.try_clone().map_err(|e| BrowserError::LaunchFailed {
        stage: "spawn",
        detail: format!("cannot duplicate the obscura log handle: {e}"),
    })?;
    // `std::process::Command`, not tokio's: `Launched.child` carries a
    // `std::process::Child` so `terminate` can `try_wait`/`wait` it, and that
    // is also the type `ChromiumChild` uses at HEAD.
    let mut cmd = Command::new(binary);
    cmd.args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errlog));
    // Same discipline as `ChromiumChild::spawn` and `playwright_cli::spawn`:
    // the browser never needs the parent's credentials, and over-stripping is
    // safe.
    //
    // It inherits that code's hazard as well, and copying it knowingly is the
    // point: `std::env::vars()` iterates while another thread may be inside
    // `std::env::set_var` (the `AlephHomeEnvGuard` pair does exactly that in
    // tests). `env_clear()` is not the fix — the browser needs PATH — and
    // diverging from the twin here would leave two disciplines where the point
    // was to have one (判据 §16). Named so the next reader does not assume this
    // file introduced it.
    for (name, _) in std::env::vars() {
        if crate::security::secret_env::is_secret_env(&name) {
            cmd.env_remove(&name);
        }
    }
    let child = cmd
        .no_window()
        .spawn()
        .map_err(|e| BrowserError::LaunchFailed {
            stage: "spawn",
            detail: format!("{}: {e}", binary.display()),
        })?;
    let pid = child.id();
    Ok((
        pid,
        std::sync::Arc::new(crate::sync_primitives::Mutex::new(Some(child))),
    ))
}

/// Poll `/json/version` until it answers, then confirm the port's owner.
///
/// The three exits are deliberately distinct, because they are three different
/// operator problems: the child died (its stderr is in the log and goes into
/// the message), the deadline elapsed, or somebody else owns the port.
///
/// **`async`, and the async `reqwest::Client`.** `reqwest::blocking` builds its
/// own runtime and panics when nested inside a tokio worker — this codebase
/// already wrote that down at
/// `src/extension/runtime/wasm/host_functions.rs` and goes through
/// `std::thread::scope` to avoid it. `spawn_blocking` is not the fix either:
/// the `&ChildHandle` this borrows is not `'static`.
async fn wait_for_endpoint(
    port: u16,
    child: &ChildHandle,
    log_path: &Path,
) -> Result<String, BrowserError> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let client = reqwest::Client::builder()
        .timeout(ENDPOINT_PROBE_TIMEOUT)
        .build()
        .map_err(|e| BrowserError::LaunchFailed {
            stage: "cdp-endpoint",
            detail: format!("cannot build an http client for the readiness poll: {e}"),
        })?;
    let started = Instant::now();
    let pid = child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map_or(0, std::process::Child::id);
    loop {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                // Ownership AFTER the endpoint answered: before it, nothing is
                // bound yet and the probe would legitimately say `Unbound`.
                // `probe_port_owner` forks `lsof`/`netstat`, so it goes to a
                // blocking thread rather than onto this worker.
                let probe = tokio::task::spawn_blocking(move || probe_port_owner(port))
                    .await
                    .unwrap_or_else(|e| PortOwner::Unknown(format!("the probe task failed: {e}")));
                settle_port_ownership(port, pid, probe)?;
                // Built here rather than read from `/json/version`: measured on
                // v0.2.2, that field echoes the REQUESTED port, so under
                // `--port 0` it is unusable and under an explicit port it is
                // merely a second author for a string we already know.
                return Ok(format!("ws://127.0.0.1:{port}/devtools/browser"));
            }
        }
        // Liveness from the CHILD HANDLE, not from a pid lookup: `try_wait`
        // reaps the exit status as a side effect, so this loop cannot leave a
        // zombie behind on the failure path, and it cannot be fooled by a
        // recycled pid. An `Err` from `try_wait` is answered "still alive" —
        // "I could not tell" is not "it is dead" (判据 §8).
        let exited = child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten());
        if let Some(status) = exited {
            return Err(BrowserError::LaunchFailed {
                stage: "obscura-exit",
                detail: format!(
                    "obscura (pid {pid}) exited with {status} before answering {url}: {}",
                    tail_of(log_path, LOG_TAIL_BYTES)
                ),
            });
        }
        if started.elapsed() >= ENDPOINT_DEADLINE {
            return Err(BrowserError::LaunchFailed {
                stage: "cdp-endpoint",
                detail: format!(
                    "obscura (pid {pid}) did not answer {url} within {}s: {}",
                    ENDPOINT_DEADLINE.as_secs(),
                    tail_of(log_path, LOG_TAIL_BYTES)
                ),
            });
        }
        tokio::time::sleep(ENDPOINT_POLL).await;
    }
}

/// How much of obscura's log a launch failure carries. The bind error is on
/// the last line, and the banner above it is ~400 B of ASCII art.
const LOG_TAIL_BYTES: usize = 2000;

/// The last `max` bytes of obscura's log, on a char boundary.
///
/// `.get(..)` rather than `&s[..]` (P7): the log is arbitrary UTF-8 and a byte
/// slice through a multi-byte character panics — inside an error path, where a
/// panic replaces a diagnosis with a crash.
fn tail_of(path: &Path, max: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return format!("(no output captured at {})", path.display());
    };
    if text.len() <= max {
        return text.trim().to_string();
    }
    let start = text.len() - max;
    let boundary = text
        .char_indices()
        .map(|(i, _)| i)
        .find(|i| *i >= start)
        .unwrap_or(text.len());
    text.get(boundary..).unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserType;
    use std::sync::{Arc, Mutex};

    fn req(profile: &str, data_dir: &Path) -> LaunchRequest {
        LaunchRequest {
            profile: profile.to_string(),
            session_key: profile.to_string(),
            data_dir: data_dir.to_path_buf(),
            headless: true,
            proxy: None,
            browser: BrowserType::default(),
            allow_private_network: false,
            stealth: false,
            extra_args: Vec::new(),
        }
    }

    /// The effect recorder both ordering tests drive.
    struct Recorder {
        seen: Arc<Mutex<Vec<Option<String>>>>,
        /// Snapshotted the moment `wait_ready` is entered, so the ordering
        /// assertion is about a completed EFFECT and not about a call.
        seen_at_wait: Arc<Mutex<Option<Vec<Option<String>>>>>,
        ws_url: Result<String, ()>,
    }

    impl Recorder {
        fn new(ws_url: Result<String, ()>) -> Self {
            Self {
                seen: Arc::new(Mutex::new(Vec::new())),
                seen_at_wait: Arc::new(Mutex::new(None)),
                ws_url,
            }
        }
    }

    #[async_trait]
    impl LaunchEffects for Recorder {
        fn spawn(&self, _argv: Vec<String>) -> Result<(u32, ChildHandle), BrowserError> {
            Ok((4242, ChildHandle::default()))
        }

        async fn record(&self, _pid: u32, http: Option<String>) {
            // A real await inside the effect, so a caller that forgot to
            // `.await` it (or detached it) cannot pass by racing.
            tokio::task::yield_now().await;
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(http);
        }

        async fn wait_ready(
            &self,
            _port: u16,
            _child: &ChildHandle,
        ) -> Result<String, BrowserError> {
            let snapshot = self
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            *self
                .seen_at_wait
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(snapshot);
            self.ws_url
                .clone()
                .map_err(|()| BrowserError::LaunchFailed {
                    stage: "obscura-exit",
                    detail: "exited with 1: Error: bind 127.0.0.1:41234: Address already in use"
                        .to_string(),
                })
        }
    }

    /// The exact token vector, for each flag combination. A snapshot rather
    /// than a set of `contains` assertions because ORDER and TOKENISATION are
    /// both contracts here: the orphan sweep matches whole argv tokens, so
    /// `--storage-dir=<dir>` as ONE token is what makes an obscura orphan
    /// reapable at all.
    #[test]
    fn the_argv_is_an_exact_ordered_token_vector() {
        let dir = Path::new("/tmp/aleph-p/obscura/default");

        assert_eq!(
            obscura_argv(41234, dir, &req("default", dir), ObscuraVariant::Default),
            vec![
                "serve".to_string(),
                "--host=127.0.0.1".to_string(),
                "--port=41234".to_string(),
                "--storage-dir=/tmp/aleph-p/obscura/default".to_string(),
            ]
        );

        let mut r = req("default", dir);
        r.allow_private_network = true;
        r.proxy = Some("socks5://127.0.0.1:1080".to_string());
        assert_eq!(
            obscura_argv(41234, dir, &r, ObscuraVariant::Stealth),
            vec![
                "serve".to_string(),
                "--host=127.0.0.1".to_string(),
                "--port=41234".to_string(),
                "--storage-dir=/tmp/aleph-p/obscura/default".to_string(),
                "--stealth".to_string(),
                "--allow-private-network".to_string(),
                "--proxy=socks5://127.0.0.1:1080".to_string(),
            ]
        );
    }

    /// **The argv token the orphan sweep will look for is the one the sidecar
    /// records.**
    ///
    /// The two are produced by different lines of `launch_with` — the argv by
    /// `obscura_argv(.., &req.data_dir, ..)`, the record by
    /// `write_sidecar_record(.., &req.data_dir, ..)` — and the sweep compares
    /// them with `argv_names_dir`. Appending a storage leaf to one and not the
    /// other compiles, launches, and makes every obscura orphan permanently
    /// unreapable with nothing reporting it (判据 §7). This is the wire
    /// between those two ends, asserted through the sweep's own matcher rather
    /// than by re-stating the path.
    #[test]
    fn the_storage_dir_token_is_what_the_orphan_sweep_matches_against() {
        let dir = Path::new("/tmp/aleph-p/obscura/default");
        let argv = obscura_argv(41234, dir, &req("default", dir), ObscuraVariant::Default);
        assert!(
            super::super::process::argv_names_dir(&argv, Engine::Obscura.data_dir_flag(), dir),
            "the sweep cannot recognise its own launcher's argv: {argv:?}"
        );
        // And it is not matching something wider: a sibling profile's dir must
        // not be recognised, or one profile's record authorises killing
        // another's browser.
        assert!(!super::super::process::argv_names_dir(
            &argv,
            Engine::Obscura.data_dir_flag(),
            Path::new("/tmp/aleph-p/obscura/other")
        ));
    }

    /// `--allow-file-access` turns the CDP port into an arbitrary local-file
    /// reader (obscura's own `serve --help`: "Off by default so a CDP
    /// connection cannot read arbitrary local files"). The spec says never. A
    /// census over the whole browser subsystem rather than over this function,
    /// because "never" is a property of the subsystem and the next launcher
    /// would be a new place to forget it (判据 §6 — count the producers).
    #[test]
    fn allow_file_access_appears_nowhere_in_the_browser_subsystem() {
        // Split so the detector does not find ITSELF. Spelled whole, this
        // function's own two source lines are the first two hits and the
        // census can never be green — a 恒红 guard is as useless as a 恒绿 one
        // (判据 §2), and the natural "fix" is to skip this file, which is what
        // would make it blind to the one file most likely to grow an argv.
        let needle = concat!("allow-file", "-access");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser");
        let mut offenders = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (n, line) in text.lines().enumerate() {
                    // Comments may (and do) name the flag to say why it is
                    // never passed; code may not.
                    if line.trim_start().starts_with("//") || line.trim_start().starts_with("///") {
                        continue;
                    }
                    if line.contains(needle) {
                        offenders.push(format!("{}:{}", path.display(), n + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "--{needle} must never reach an obscura argv: {offenders:?}"
        );
    }

    /// The census above only means something if it reads files at all. An
    /// empty corpus passes every "no offender" assertion silently
    /// (判据 §2's 恒绿 face), so the walk is made to name what it saw.
    #[test]
    fn the_allow_file_access_census_actually_reads_this_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/browser");
        let mut seen = 0usize;
        let mut found_self = false;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    seen += 1;
                    if path.ends_with("engine/obscura.rs") {
                        found_self = true;
                    }
                }
            }
        }
        assert!(
            seen > 10,
            "the census walked {seen} files; it is not walking"
        );
        assert!(found_self, "the census did not reach obscura.rs itself");
    }

    /// `variant` selects a RUNTIME FLAG, not a different download. Verified
    /// against the real release: `obscura-aarch64-macos-stealth.tar.gz` DOES
    /// exist upstream, and the ledger deliberately fetches the plain one — so
    /// a reader who assumed `variant = "stealth"` changes the asset would be
    /// looking at a real archive Aleph never installs.
    #[test]
    fn the_stealth_variant_is_a_flag_not_a_second_archive() {
        let dir = Path::new("/tmp/aleph-p/obscura/default");
        let argv = obscura_argv(1, dir, &req("default", dir), ObscuraVariant::Stealth);
        assert!(argv.contains(&"--stealth".to_string()));
        let spec = crate::runtimes::find_spec(crate::runtimes::OBSCURA_RUNTIME)
            .expect("obscura has a runtime spec");
        let crate::runtimes::InstallStrategy::GithubRelease { asset, .. } =
            &spec.install[0].strategy
        else {
            panic!("obscura installs from a release");
        };
        assert_eq!(
            asset("macos", "aarch64"),
            Some("obscura-aarch64-macos.tar.gz")
        );
        assert_eq!(
            asset("windows", "x86_64"),
            Some("obscura-x86_64-windows.zip")
        );
    }

    /// The three ownership verdicts. A probe that could not RUN is not a
    /// refutation — the instrument being absent says nothing about the port
    /// (判据 §8, §18) — but a probe that ran and named someone else is.
    #[test]
    fn port_ownership_refuses_a_foreign_pid_and_an_empty_port_but_tolerates_a_missing_tool() {
        settle_port_ownership(41234, 999, PortOwner::Pid(999)).expect("our own pid must pass");

        let err = settle_port_ownership(41234, 999, PortOwner::Pid(1000))
            .unwrap_err()
            .to_string();
        assert!(err.contains("41234"), "{err}");
        assert!(
            err.contains("1000"),
            "must name the pid that actually holds it: {err}"
        );
        assert!(err.contains("999"), "and the one we launched: {err}");

        let err = settle_port_ownership(41234, 999, PortOwner::Unbound)
            .unwrap_err()
            .to_string();
        assert!(err.contains("nothing is listening"), "{err}");

        // The instrument is missing. Exclusive TCP bind plus "the child did
        // not exit with a bind error" already establishes ownership; a hard
        // failure here would make obscura unusable in any container without
        // lsof, on the strength of an instrument we never had.
        settle_port_ownership(41234, 999, PortOwner::Unknown("lsof not found".into()))
            .expect("an unavailable probe must not be read as a refutation");
    }

    /// The probe's own three answers, against a port whose state this test
    /// controls.
    ///
    /// The reason this exists rather than only the `settle_*` table above:
    /// **`Unbound` is the arm a wrong parse rule silently deletes.** lsof on
    /// this machine writes a `WARNING: can't stat() smbfs …` line to stderr on
    /// EVERY invocation, success and no-match alike (measured at BASE), so a
    /// rule keyed on "stderr is empty" answers `Unknown` for every free port
    /// and the `Unbound` arm never fires anywhere in production — 判据 §2's
    /// 恒绿 face. This asserts the two answers a live machine can actually
    /// produce.
    #[cfg(unix)]
    #[test]
    fn the_listener_probe_tells_a_held_port_from_a_free_one_on_this_machine() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind a loopback port");
        let held = listener.local_addr().expect("local addr").port();
        let me = std::process::id();

        match probe_port_owner(held) {
            PortOwner::Pid(p) => assert_eq!(p, me, "the probe named someone else's pid"),
            // lsof may genuinely be absent (a container). That is the one
            // answer this test accepts without a claim about the port.
            PortOwner::Unknown(why) => {
                eprintln!("listener probe unavailable, nothing asserted: {why}");
                return;
            }
            PortOwner::Unbound => panic!("a port this process is holding read as Unbound"),
        }

        drop(listener);
        // The freed port: the arm that a stderr-keyed rule would have deleted.
        assert_eq!(
            probe_port_owner(held),
            PortOwner::Unbound,
            "a port nobody holds must read as Unbound, not Unknown — see this \
             function's doc for the lsof stderr measurement"
        );
    }

    /// `reserve_port` owes exactly two things, and this asserts those two.
    ///
    /// **It reads the port off the SOCKET, not off the request.** `bind(:0)`
    /// returning `0` would be the whole function failing silently — obscura
    /// would then be launched on port 0, announce `ws://127.0.0.1:0/…`, and
    /// nothing would ever connect. That is deterministic and is the first
    /// assertion.
    ///
    /// **It does not keep the port bound.** Checked by re-binding, with
    /// retries, and the retries are the point rather than tidiness: a bind that
    /// fails because `reserve_port` forgot its `drop` fails EVERY time, while
    /// one that fails because another thread in this same test binary grabbed
    /// the port in the window fails once. Distinguishing those two is what the
    /// loop buys.
    ///
    /// ⚠️ **This test previously also asserted `a != b`, and that assertion was
    /// removed after it went red once in a full-suite run and never again in
    /// four repeats.** Two consecutive calls getting different ports is a
    /// property of the kernel's ephemeral allocator under whatever else is
    /// binding at that moment — it is not something this function promises, and
    /// asserting it made a guard that can go red without the code changing. A
    /// guard that cries wolf is more expensive than a missing one, because the
    /// next reader spends it as evidence (判据 §3).
    ///
    /// It also misstated where the safety comes from. Two launches CAN race for
    /// one port; nothing here prevents that. What makes it safe is on the far
    /// side: the loser gets a clean, immediate, non-zero exit with a named
    /// reason — measured on the real v0.2.2 binary, `exit 1` and
    /// `Error: bind 127.0.0.1:<p>: Address already in use (os error 48)` on
    /// stderr — which `wait_for_endpoint` reports as
    /// `LaunchFailed{stage: "obscura-exit"}` carrying that text.
    #[test]
    fn reserve_port_reads_the_socket_and_does_not_keep_it_bound() {
        let a = reserve_port().expect("a loopback ephemeral port must be reservable");
        assert_ne!(
            a, 0,
            "reserve_port echoed the requested port instead of reading the socket"
        );

        let mut last = None;
        for _ in 0..10 {
            match std::net::TcpListener::bind(("127.0.0.1", a)) {
                Ok(_) => return,
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        panic!(
            "127.0.0.1:{a} was unbindable on all 10 attempts, so reserve_port is \
             holding its listener rather than dropping it: {last:?}"
        );
    }

    /// The sidecar is written from the pid, BEFORE the endpoint is known — the
    /// same rule `ChromiumChild::spawn` states: if this future is dropped at
    /// any await below, the record is the only trace that a process exists and
    /// needs reaping. Asserted by ORDER, not by "the function was called": the
    /// recorder fails the test if the endpoint is already known when the first
    /// record lands.
    #[tokio::test]
    async fn the_sidecar_record_has_landed_before_the_endpoint_is_waited_for() {
        let dir = Path::new("/tmp/aleph-p/obscura/default");
        let fx = Recorder::new(Ok("ws://127.0.0.1:41234/devtools/browser".to_string()));
        let seen = fx.seen.clone();
        let at_wait = fx.seen_at_wait.clone();

        let out = launch_with(
            &req("default", dir),
            ObscuraVariant::Default,
            41234,
            PathBuf::from("/tmp/aleph-test-sidecar.json"),
            &fx,
        )
        .await
        .expect("the fake launch must succeed");

        // THE assertion: the intent stamp must have LANDED by the time the
        // endpoint wait begins — not merely have been called. A detached
        // `tokio::spawn` passes a call-ordering check and fails this one
        // (判据 §4).
        assert_eq!(
            at_wait
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            Some(vec![None]),
            "the endpoint-less sidecar record must be on disk before anything is \
             awaited that could drop this future; a fire-and-forget write is not"
        );
        assert_eq!(
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            vec![None, Some("http://127.0.0.1:41234".to_string())],
            "first record carries no endpoint (it is not known yet), second one does"
        );
        assert_eq!(out.pid, 4242);
        assert_eq!(out.endpoint.ws_url, "ws://127.0.0.1:41234/devtools/browser");
        assert_eq!(out.endpoint.http_url, "http://127.0.0.1:41234");
        assert_eq!(
            out.sidecar_path,
            PathBuf::from("/tmp/aleph-test-sidecar.json"),
            "the record's path is the one the launcher derived, not a third derivation"
        );
    }

    /// A child that exits before answering is a DIFFERENT operator problem
    /// from a slow one, and its stage string must say which — the same reason
    /// `LaunchFailed.stage` exists. The measured text for the commonest case is
    /// on stderr, so the detail must carry it.
    #[tokio::test]
    async fn a_child_that_dies_before_the_endpoint_reports_the_exit_stage_and_its_stderr() {
        let dir = Path::new("/tmp/aleph-p/obscura/default");
        let fx = Recorder::new(Err(()));
        let seen = fx.seen.clone();
        let err = launch_with(
            &req("default", dir),
            ObscuraVariant::Default,
            41234,
            PathBuf::from("/tmp/aleph-test-sidecar.json"),
            &fx,
        )
        .await
        .err()
        .map(|e| e.to_string())
        .expect("the launch must fail when the endpoint never answers");
        // The intent stamp survives the failure: that record is the only thing
        // that can find this process again (判据 §15).
        assert_eq!(
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            vec![None],
            "the pre-endpoint record must have landed even though the launch failed"
        );
        assert!(err.contains("obscura-exit"), "{err}");
        assert!(
            err.contains("Address already in use"),
            "the stderr text must survive: {err}"
        );
    }

    /// `kill` must leave no zombie. A pid-only kill cannot `wait()`, so the
    /// child sits in the process table as a defunct entry until the daemon
    /// exits — which is why the trait takes `&Launched` and the launcher keeps
    /// the `Child`. Driven with a real short-lived process rather than obscura:
    /// nothing here is browser-specific, and a test that needed a browser would
    /// not run in `--lib`.
    #[cfg(unix)]
    #[tokio::test]
    async fn kill_reaps_the_child_and_leaves_no_zombie() {
        let child = std::process::Command::new("sleep")
            .arg("120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("`sleep` must be spawnable");
        let pid = child.id();
        let launched = Launched {
            pid,
            endpoint: CdpEndpoint {
                http_url: "http://127.0.0.1:1".to_string(),
                ws_url: "ws://127.0.0.1:1/devtools/browser".to_string(),
                pid,
            },
            sidecar_path: PathBuf::from("/dev/null"),
            child: std::sync::Arc::new(crate::sync_primitives::Mutex::new(Some(child))),
        };

        let launcher = ObscuraLauncher::new(ObscuraRuntimeConfig::default());
        let died = launcher
            .kill(&launched, Duration::from_secs(5))
            .await
            .expect("the kill itself must not error");
        assert!(died, "a `sleep` must not survive a kill");

        // The EFFECT, twice over: the handle was reaped (so no zombie), and the
        // pid is gone from the process table. Asserting only the return value
        // would pass over a `kill()` that never `wait()`ed.
        assert!(
            launched
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none(),
            "the child must have been waited and taken, not merely signalled"
        );
        let ps = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .expect("ps must run");
        let stat = String::from_utf8_lossy(&ps.stdout);
        assert!(
            stat.trim().is_empty() || !stat.contains('Z'),
            "pid {pid} is still in the table as {stat:?}"
        );
    }

    /// A pinned `binary_path` that is not a file is REFUSED, never fallen back
    /// from — and the refusal names the path, because "obscura is not
    /// installed" would send the operator to install a binary they already
    /// pinned (判据 §17: a wrong label costs more than a missing one).
    #[test]
    fn a_pin_that_is_not_a_file_is_refused_by_name_rather_than_fallen_back_from() {
        let cfg = ObscuraRuntimeConfig {
            binary_path: Some("/nonexistent/obscura-pin".to_string()),
            ..ObscuraRuntimeConfig::default()
        };
        let err = resolve_obscura_binary(&cfg)
            .expect_err("a missing pin must not resolve to something else")
            .to_string();
        assert!(err.contains("/nonexistent/obscura-pin"), "{err}");
        assert!(
            err.contains("is not a file"),
            "must say WHY the pin was refused, not just that obscura is missing — \
             the generic hint mentions `binary_path` either way, so asserting on \
             that word alone would be true even with this arm deleted: {err}"
        );
    }

    /// A blank pin is not a pin. A cleared Panel field posts `""`, and
    /// `Some("")` spent as a path resolves to the current directory and then
    /// fails naming nothing — so the resolver must fall THROUGH to the normal
    /// search rather than refuse.
    #[test]
    fn a_blank_pin_falls_through_instead_of_refusing_a_path_it_cannot_name() {
        let cfg = ObscuraRuntimeConfig {
            binary_path: Some("   ".to_string()),
            ..ObscuraRuntimeConfig::default()
        };
        // On a machine with no obscura this is the ledger's refusal, on one
        // with obscura on PATH it is a path — either way it must NOT be the
        // pin refusal above.
        match resolve_obscura_binary(&cfg) {
            Ok(p) => assert!(p.is_file(), "{}", p.display()),
            Err(e) => {
                let text = e.to_string();
                assert!(
                    !text.contains("is not a file"),
                    "a blank pin was treated as a pin: {text}"
                );
            }
        }
    }

    /// `tail_of` must not panic on a multi-byte boundary — it runs inside an
    /// error path, where a panic replaces a diagnosis with a crash.
    #[test]
    fn the_log_tail_cuts_on_a_char_boundary_and_never_panics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("serve.log");
        // Every character is 3 bytes, so almost every byte offset is mid-char.
        std::fs::write(&path, "。".repeat(400)).expect("write");
        let tail = tail_of(&path, 100);
        assert!(!tail.is_empty());
        assert!(tail.len() <= 100, "{}", tail.len());
        assert!(tail.chars().all(|c| c == '。'), "{tail}");

        // A log that is not there is an absence, reported as such rather than
        // as an empty string that reads like "obscura said nothing".
        let missing = tail_of(&dir.path().join("nope.log"), 100);
        assert!(missing.contains("no output captured"), "{missing}");
    }
}
