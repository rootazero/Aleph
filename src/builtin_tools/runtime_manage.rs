//! `runtime_manage` — the R8 face of the `runtimes.*` RPC family.
//!
//! Everything configurable is a tool (R8), and until now the runtime ledger was
//! the exception: `runtimes.list` / `runtimes.refresh` / `runtimes.install`
//! existed only as gateway RPCs, reachable from the Panel and from nothing the
//! model can call. So "Chromium is not installed" was a dead end in
//! conversation — the fail-closed message could name a shell command and
//! nothing else.
//!
//! `chromium` is a member of this tool's installable set WITHOUT being a
//! `RuntimeSpec`, and that is deliberate. The ledger probes PATH
//! (`runtimes::probe::probe_system_path`), and Playwright's Chromium lives in a
//! per-revision cache directory that is never on PATH — a spec for it would sit
//! at `Missing` forever and reinstall on every call. Its supply already exists
//! as `playwright-cli`'s post-install action (`install-browser chromium`), and
//! this tool re-runs exactly that command with exactly that environment.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::runtimes::ledger::{CapabilityLedger, CapabilityStatus};
use crate::runtimes::specs::EnvFromConfig;
use crate::runtimes::{ensure_capability, find_spec, supported_on_current_os, SPECS};
use crate::sandbox::live_tail::LiveTail;
use crate::sync_primitives::Arc;

/// The one capability this tool installs that the ledger does not model.
const CHROMIUM: &str = "chromium";

/// The subcommand that supplies it — the SAME argv the ledger's post-install
/// action runs. Owned by `browser::chromium_resolve` (M8 correction): that
/// module is the lower layer, already depended on through the browser crate,
/// so this tool consumes it from there instead of minting its own copy —
/// `browser/error.rs` reaching UP into this module for it was the wrong
/// direction (`tab_registry.rs`'s "not reach up into `builtin_tools`").
use crate::browser::chromium_resolve::CHROMIUM_INSTALL_ARGS;

// NO install timeout constant lives here, and its absence is the point.
//
// A ~150 MB download does not fit a tool call: the per-tool budget is 180 s and
// `bash_exec::WAIT_MAX_TIMEOUT_SECS = 170` (`src/builtin_tools/bash_exec.rs:60`)
// is the hard constraint CLAUDE.md forbids extending — the very fact this
// module's own fail-closed message cites to justify NOT installing from
// `browser::chromium_resolve::resolve_binary`. Routing the install here and
// then blocking on it would move the problem, not solve it (判据 §1: one fact,
// two answers).
//
// So `install` uses the machinery CLAUDE.md names for exactly this ("长任务
// （>3 min build/install）必须 `background: true`"): register the job in
// `process_registry()` and return its id immediately. The model polls with the
// verb that already exists — `bash{process_action:"wait", process_id}`, itself
// clamped to `WAIT_MAX_TIMEOUT_SECS` (`bash_exec.rs:553`) — and
// `kill_all_running_background` reaps the job on daemon exit for free.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAction {
    /// What is installed, what is missing, and what each one is for.
    List,
    /// Install one capability by name.
    Install,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeManageArgs {
    pub action: RuntimeAction,
    /// Required for `install`; ignored by `list`.
    #[serde(default)]
    pub capability: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeRow {
    pub name: String,
    pub status: String,
    pub path: Option<String>,
    pub version: Option<String>,
    pub purpose: Option<String>,
    pub supported_here: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeManageOutput {
    pub ok: bool,
    pub message: String,
    pub runtimes: Vec<RuntimeRow>,
}

/// Whether `name` is something this tool can install: a ledger spec, or the one
/// capability the ledger deliberately does not model.
#[must_use]
pub fn is_installable(name: &str) -> bool {
    name == CHROMIUM || find_spec(name).is_some()
}

/// Every installable name, for the error that has to say what IS available.
fn installable_names() -> Vec<&'static str> {
    SPECS
        .iter()
        .map(|s| s.name)
        .chain(std::iter::once(CHROMIUM))
        .collect()
}

/// Where the tool learns about Chromium.
///
/// Injected because the production answer shells out to `playwright-cli
/// install-browser --dry-run`, and `cargo test --lib` must not spawn node.
#[async_trait]
pub(crate) trait ChromiumLocator: Send + Sync {
    async fn locate(&self) -> RuntimeRow;
}

/// The production locator: the resolver the browser driver itself uses.
pub(crate) struct RealChromiumLocator;

#[async_trait]
impl ChromiumLocator for RealChromiumLocator {
    async fn locate(&self) -> RuntimeRow {
        chromium_row().await
    }
}

#[derive(Clone)]
pub struct RuntimeManageTool {
    locator: Arc<dyn ChromiumLocator>,
    spawner: Arc<dyn InstallSpawner>,
}

impl Default for RuntimeManageTool {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RuntimeManageTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeManageTool").finish_non_exhaustive()
    }
}

impl RuntimeManageTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            locator: Arc::new(RealChromiumLocator),
            spawner: Arc::new(RegistrySpawner::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_locator(locator: Arc<dyn ChromiumLocator>) -> Self {
        Self {
            locator,
            spawner: Arc::new(RegistrySpawner::new()),
        }
    }

    /// Both seams at once, for the tests that must neither spawn node nor
    /// register a real background job.
    #[cfg(test)]
    pub(crate) fn with_parts(
        locator: Arc<dyn ChromiumLocator>,
        spawner: Arc<dyn InstallSpawner>,
    ) -> Self {
        Self { locator, spawner }
    }

    async fn ledger() -> crate::error::Result<Arc<tokio::sync::RwLock<CapabilityLedger>>> {
        let dir = crate::runtimes::get_runtimes_dir()
            .map_err(|e| crate::error::AlephError::tool(format!("runtimes dir: {e}")))?;
        let path = dir.join("ledger.json");
        let ledger = tokio::task::spawn_blocking(move || CapabilityLedger::load_or_create(path))
            .await
            .map_err(|e| crate::error::AlephError::tool(format!("load capability ledger: {e}")))?;
        Ok(Arc::new(tokio::sync::RwLock::new(ledger)))
    }

    async fn list(locator: &Arc<dyn ChromiumLocator>) -> RuntimeManageOutput {
        let ledger = match Self::ledger().await {
            Ok(l) => l,
            Err(e) => {
                return RuntimeManageOutput {
                    ok: false,
                    message: format!("Cannot read the runtime ledger: {e}"),
                    runtimes: Vec::new(),
                }
            }
        };
        let guard = ledger.read().await;
        let mut runtimes: Vec<RuntimeRow> = SPECS
            .iter()
            .map(|spec| {
                let entry = guard.entries.get(spec.name);
                RuntimeRow {
                    name: spec.name.to_string(),
                    status: format!(
                        "{:?}",
                        entry.map_or(CapabilityStatus::Missing, |e| e.status)
                    ),
                    path: entry
                        .filter(|e| !e.bin_path.as_os_str().is_empty())
                        .map(|e| e.bin_path.to_string_lossy().to_string()),
                    version: entry
                        .filter(|e| !e.version.is_empty())
                        .map(|e| e.version.clone()),
                    purpose: spec.llm_hint.map(str::to_string),
                    supported_here: supported_on_current_os(spec.name),
                }
            })
            .collect();
        // Chromium is not in the ledger (see the module doc), so its row is
        // derived from the resolver the browser driver itself uses. A row that
        // said "Missing" while a system Chrome sat in /Applications would be a
        // lie the model would act on.
        runtimes.push(locator.locate().await);
        // An install this tool started is still running somewhere; `list` is
        // where a model looks next, so it says so rather than showing the
        // pre-install answer and letting the model conclude nothing happened.
        let running = running_install_jobs();
        let message = if running.is_empty() {
            format!("{} runtime(s).", runtimes.len())
        } else {
            format!(
                "{} runtime(s). {} install(s) still running: {} — poll with \
                 `bash{{process_action:\"wait\", process_id:<id>}}`.",
                runtimes.len(),
                running.len(),
                running
                    .iter()
                    .map(|(id, cmd)| format!("{id} ({cmd})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        RuntimeManageOutput {
            ok: true,
            message,
            runtimes,
        }
    }

    /// Start an install and return **immediately** with its background job id.
    ///
    /// Nothing here awaits the installer. Both branches — the chromium download
    /// and `ensure_capability` (npm global installs, `curl … | sh` bootstrap
    /// scripts, which have no timeout of their own at all) — are detached into
    /// the same registry `bash {background: true}` uses, so neither can sit on
    /// a 180 s tool budget and neither can run unbounded and unobservable.
    async fn install(
        capability: Option<String>,
        spawner: &Arc<dyn InstallSpawner>,
        locator: &Arc<dyn ChromiumLocator>,
    ) -> RuntimeManageOutput {
        let Some(name) = capability
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return RuntimeManageOutput {
                ok: false,
                message: format!(
                    "install needs a `capability`. Installable: {}.",
                    installable_names().join(", ")
                ),
                runtimes: Vec::new(),
            };
        };
        if !is_installable(name) {
            return RuntimeManageOutput {
                ok: false,
                message: format!(
                    "unknown capability {name:?}. Installable: {}.",
                    installable_names().join(", ")
                ),
                runtimes: Vec::new(),
            };
        }
        match spawner.spawn(name) {
            Ok(job_id) => {
                // `ok` must come from THIS branch — the spawn succeeded, a job
                // IS running and holding a registry slot — not from `list`'s
                // own verdict. `list` builds the snapshot this response reuses
                // for its `runtimes` field, but if the ledger fails to load
                // inside it, `list` answers `ok:false` with an unrelated
                // message about a job that is, right now, actually running.
                // A caller reading `ok` as the verdict would retry, and now
                // two installs hold slots — the inverse of 判据 §11's "success
                // reported for a no-op": here a REAL effect gets reported as a
                // failure.
                let mut out = Self::list(locator).await;
                // M3: `list`'s OWN verdict, not just its message, must be read
                // before it is overwritten below — `list`'s ledger-failure arm
                // answers `ok:false` with `runtimes: []`, and `[]` is only
                // entitled to mean "I don't know" here, never "there are
                // none" (判据 §8). Silently keeping that `[]` under `ok:true`
                // would render an unreadable ledger as an empty one.
                let list_failed = !out.ok;
                let list_failure_reason = std::mem::take(&mut out.message);
                out.ok = true;
                out.message = if list_failed {
                    format!(
                        "Installing {name} in the background as job {job_id}. It downloads over \
                         the network and will not finish inside a tool call, so poll it with \
                         `bash{{process_action:\"wait\", process_id:{job_id}}}` (or \
                         `process_action:\"poll\"` for a non-blocking peek). The current runtime \
                         list could not also be read ({list_failure_reason}) — the empty \
                         `runtimes` below means UNKNOWN, not \"nothing installed\"."
                    )
                } else {
                    format!(
                        "Installing {name} in the background as job {job_id}. It downloads over \
                         the network and will not finish inside a tool call, so poll it with \
                         `bash{{process_action:\"wait\", process_id:{job_id}}}` (or \
                         `process_action:\"poll\"` for a non-blocking peek). This tool's `list` \
                         action also reports the job while it runs."
                    )
                };
                out
            }
            Err(why) => RuntimeManageOutput {
                ok: false,
                message: format!("could not start the {name} install: {why}"),
                runtimes: Vec::new(),
            },
        }
    }
}

/// This tool's own background jobs, read out of the shared registry.
///
/// Filtered by the command prefix `install` writes, so a `cargo build` the user
/// backgrounded through `bash` does not get reported as a runtime install. The
/// registry is per-caller already; this narrows by verb, not by ownership.
fn running_install_jobs() -> Vec<(u64, String)> {
    let registry = crate::builtin_tools::process_registry::process_registry();
    let caller = crate::builtin_tools::bash_exec::session_label();
    registry
        .list(caller.as_deref())
        .into_iter()
        .filter(|row| row.status == "running" && row.command.starts_with(INSTALL_JOB_PREFIX))
        .map(|row| (row.id, row.command.clone()))
        .collect()
}

/// The command string every install job is registered under. One constant, two
/// readers (the spawner writes it, `running_install_jobs` filters on it) — a
/// second spelling would make the filter silently match nothing.
const INSTALL_JOB_PREFIX: &str = "runtime_manage install ";

/// How an install is started. Injected for the same reason [`ChromiumLocator`]
/// is: a unit test must neither spawn node nor register a real background job.
#[async_trait]
pub(crate) trait InstallSpawner: Send + Sync {
    /// Register the install as a background job and return its id, without
    /// waiting for it.
    fn spawn(&self, capability: &str) -> Result<u64, String>;
}

/// Where [`install_chromium`] finds the `playwright-cli` binary to run
/// `install-browser chromium` with.
///
/// Injected for the same reason [`ChromiumLocator`]/[`InstallSpawner`] are —
/// and its absence was round 1's I2 finding wearing a second costume: the
/// production answer (`probes::browser::managed_cli_path`) does a `which`
/// PATH walk plus a ledger read, with no seam, so a test that wants to prove
/// `install_chromium`'s real effect — the `tokio::select!` tee that makes
/// `bash{process_action:"poll"}` show bytes — had no way to run a REAL
/// subprocess and had to supply its own pushing closure instead, which stays
/// green whether or not the tee itself ever runs (判据 §4). This seam lets a
/// test point `install_chromium` at a trivial, test-written script.
#[async_trait]
pub(crate) trait ChromiumInstallCli: Send + Sync {
    async fn cli_path(&self) -> Option<PathBuf>;
}

/// The production answer: the same PATH walk `chromium_row` and the doctor's
/// twin probe use.
pub(crate) struct RealChromiumInstallCli;

#[async_trait]
impl ChromiumInstallCli for RealChromiumInstallCli {
    async fn cli_path(&self) -> Option<PathBuf> {
        tokio::task::spawn_blocking(crate::tools::probes::browser::managed_cli_path)
            .await
            .unwrap_or(None)
    }
}

/// The production spawner: the same registry `bash {background: true}` uses.
pub(crate) struct RegistrySpawner {
    cli_locator: Arc<dyn ChromiumInstallCli>,
}

impl RegistrySpawner {
    pub(crate) fn new() -> Self {
        Self {
            cli_locator: Arc::new(RealChromiumInstallCli),
        }
    }

    /// The test seam: a `RegistrySpawner` that resolves the chromium install
    /// CLI to a test-controlled path instead of walking the real PATH.
    #[cfg(test)]
    pub(crate) fn with_cli_locator(cli_locator: Arc<dyn ChromiumInstallCli>) -> Self {
        Self { cli_locator }
    }

    /// The registration half, separated from the work.
    ///
    /// Two reasons. **The label must match byte for byte.**
    /// `ProcessRegistry::owns` (`src/builtin_tools/process_registry.rs:671-673`)
    /// is `entry.session_label.as_deref() == caller` — plain equality on
    /// `Option<&str>`, no normalisation — and every lookup `bash` performs
    /// (`poll` / `wait` / `list`, all through `handle_process_action`'s
    /// `let caller = session_label();` at `bash_exec.rs:498`, e.g. the
    /// `registry.poll(id, caller.as_deref())` at `:574`) passes exactly what
    /// `bash_exec::session_label()` (`:449-451`) produces:
    /// `current_session().map(|sid| serde_json::to_string(&sid)…)`. That is
    /// **serde JSON of the `SessionId`**, deliberately not
    /// `SessionKey::to_key_string()` — the inverse's own doc (`:453-464`) warns
    /// that reaching for `from_key_string` "returns `None` for every row, which
    /// reads exactly like 'this job had no session'". Register under any other
    /// derivation and the id this tool hands back is un-pollable: a reported
    /// success that leads nowhere (判据 §11).
    ///
    /// **And it makes that derivation testable** without running an installer:
    /// a test registers a job that does nothing and resolves it through the
    /// same lookup, so the two halves are proven to agree rather than assumed to.
    ///
    /// **I2:** the twin, `bash_exec::spawn_background`, also attaches a
    /// [`LiveTail`] before returning so `poll` shows something for a
    /// still-running job — this tool's own message at [`RuntimeManageTool::install`]
    /// names exactly that verb. `live` is threaded in here rather than
    /// created ad hoc so the registry attach and the tee the job writes into
    /// are provably the same instance.
    fn register<F>(command: String, live: Arc<LiveTail>, job: F) -> Result<u64, String>
    where
        F: std::future::Future<Output = crate::builtin_tools::code_exec::CodeExecOutput>
            + Send
            + 'static,
    {
        use crate::builtin_tools::process_registry::{process_registry, RegisterOutcome};
        let registry = process_registry();
        // The one derivation. Not a local re-implementation.
        let label = crate::builtin_tools::bash_exec::session_label();

        // Same ordering hazard `spawn_background` solves and the same solution:
        // a fast failure could otherwise complete before `register_running`
        // inserts the slot, and the outcome would be dropped. The task waits on
        // a oneshot carrying its own id.
        let (id_tx, id_rx) = tokio::sync::oneshot::channel::<u64>();
        let join = tokio::spawn(async move {
            let Ok(id) = id_rx.await else {
                // Registration failed; there is nothing to report against.
                return;
            };
            let outcome = job.await;
            crate::builtin_tools::process_registry::process_registry().complete(id, outcome);
        });

        match registry.register_running(command, label, join.abort_handle()) {
            RegisterOutcome::Registered(id) => {
                // Same ordering the twin uses: attach before the task learns
                // its id, so nothing can complete and report before `poll`
                // has somewhere to read from.
                registry.attach_live(id, live);
                // The task is parked until it learns its id; send it now that
                // the slot exists.
                let _ = id_tx.send(id);
                Ok(id)
            }
            RegisterOutcome::TooManyRunning { limit } => {
                join.abort();
                Err(format!(
                    "this session already has {limit} background jobs running; \
                     poll or kill one first (`bash{{process_action:\"list\"}}`)"
                ))
            }
        }
    }
}

#[async_trait]
impl InstallSpawner for RegistrySpawner {
    fn spawn(&self, capability: &str) -> Result<u64, String> {
        let cap = capability.to_string();
        let live = Arc::new(LiveTail::new());
        let live_for_job = live.clone();
        let cli_locator = self.cli_locator.clone();
        Self::register(
            format!("{INSTALL_JOB_PREFIX}{capability}"),
            live,
            async move { run_install(&cap, &live_for_job, &cli_locator).await },
        )
    }
}

/// The install itself, running detached. Returns the shape the registry stores
/// for `poll` / `wait` to hand back.
///
/// `live` is NOT re-entered as a context the way `bash_exec::spawn_background`
/// re-enters `SESSION_ID` / `EXEC_WORKSPACE` / `CallIdentity` — it is passed
/// explicitly, because neither branch below runs through the sandboxed
/// executor those task-locals exist for (verified: zero references to any of
/// the three anywhere under `src/runtimes/`). Re-entering them here would be
/// inert scaffolding copied from a caller that does not apply; passing `live`
/// explicitly is the part that has a real reader.
async fn run_install(
    capability: &str,
    live: &Arc<LiveTail>,
    cli_locator: &Arc<dyn ChromiumInstallCli>,
) -> crate::builtin_tools::code_exec::CodeExecOutput {
    if capability == CHROMIUM {
        install_chromium(live, cli_locator).await
    } else {
        // `ensure_capability`'s subprocesses (curl|sh installers, npm global
        // installs, each OS's own bootstrap script) live several layers down
        // in `runtimes::bootstrap`, and none of that call graph accepts a
        // live tail today. Threading one through it is a larger change than
        // this fix round — named here rather than silently dropped: `poll`
        // shows nothing for these while they run, same as before this fix.
        // `chromium` is the 150 MB download the finding names, and it IS
        // wired below.
        let ledger = match RuntimeManageTool::ledger().await {
            Ok(l) => l,
            Err(e) => return install_output(false, format!("{e}")),
        };
        match ensure_capability(capability, &ledger).await {
            Ok(path) => install_output(
                true,
                format!("{capability} is ready at {}.", path.display()),
            ),
            Err(e) => install_output(false, format!("{capability} install failed: {e}")),
        }
    }
}

/// The chromium row, derived from the driver's own resolver.
async fn chromium_row() -> RuntimeRow {
    // Off the async worker, same as the doctor's twin probe: a `which` PATH
    // walk plus a JSON read (判据 §16).
    let cli = tokio::task::spawn_blocking(crate::tools::probes::browser::managed_cli_path)
        .await
        .unwrap_or(None);
    let (status, path) = match cli {
        None => ("Unknown (no playwright-cli)".to_string(), None),
        Some(cli) => match crate::config::Config::load() {
            // A config we cannot read is NOT a config with default settings: a
            // pinned `binary_path` we failed to see would make this row say
            // "Missing" on a host that has a browser. The doctor answers this
            // condition with `unknown`; the tool must say the same thing, or
            // the two faces disagree about the same fact (判据 §16).
            Err(e) => (format!("Unknown (the config could not be read: {e})"), None),
            Ok(cfg) => {
                match crate::browser::chromium_resolve::resolve_binary(
                    &cfg.general.browser.runtime,
                    &crate::browser::profile::BrowserType::default(),
                    &cli,
                )
                .await
                {
                    Ok(r) => (
                        format!("Ready ({})", r.source.label()),
                        Some(r.path.display().to_string()),
                    ),
                    Err(e) => (format!("Missing ({e})"), None),
                }
            }
        },
    };
    RuntimeRow {
        name: CHROMIUM.to_string(),
        status,
        path,
        version: None,
        purpose: Some(
            "The browser the managed browser driver launches. Supplied by \
             `playwright-cli install-browser chromium`, or by any system \
             Chrome/Chromium/Brave/Edge."
                .to_string(),
        ),
        supported_here: true,
    }
}

/// Run the same command the ledger's post-install action runs, with the same
/// environment.
async fn install_chromium(
    live: &Arc<LiveTail>,
    cli_locator: &Arc<dyn ChromiumInstallCli>,
) -> crate::builtin_tools::code_exec::CodeExecOutput {
    let cli = cli_locator.cli_path().await;
    let Some(cli) = cli else {
        return install_output(
            false,
            "Cannot install chromium: playwright-cli is not provisioned yet. Install that \
             first (`runtime_manage{action:\"install\", capability:\"playwright-cli\"}`), \
             whose own post-install step installs chromium too."
                .to_string(),
        );
    };
    let mut cmd = tokio::process::Command::new(&cli);
    cmd.args(CHROMIUM_INSTALL_ARGS)
        .stdin(std::process::Stdio::null())
        // Piped rather than inherited: streamed below into `live`, chunk by
        // chunk, instead of `.output()`'s wait-for-everything. This IS the
        // 150 MB download `bash{process_action:"poll"}` promises something
        // for (I2) — without this, `poll` showed nothing at all while it ran.
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in
        crate::runtimes::post_install::config_env(&[EnvFromConfig::PlaywrightDownloadHost])
    {
        cmd.env(key, value);
    }
    use crate::utils::no_window::NoWindow;
    // No `tokio::time::timeout` here. This runs on the detached registry task,
    // where the bound that matters is the registry's own — the same place a
    // backgrounded `cargo build` lives, and for the same reason. A timeout here
    // would be a second, smaller answer to "how long may this run" than the one
    // the caller can already see and cancel through `bash{process_action:…}`.
    let mut child = match cmd.no_window().spawn() {
        Ok(c) => c,
        Err(e) => return install_output(false, format!("chromium install could not run: {e}")),
    };

    // Tee both streams into `live` as they arrive. Only stderr is also kept
    // whole (the failure message below needs the complete text, which a ring
    // only retains a tail of) — stdout is teed and drained without being
    // retained: this download's stdout can run to the same order of
    // magnitude as the download itself, and nothing here reads it back.
    let stderr_buf = {
        use crate::sandbox::live_tail::LiveStream;
        use tokio::io::AsyncReadExt;
        let mut stdout_pipe = child.stdout.take().expect("stdout was piped above");
        let mut stderr_pipe = child.stderr.take().expect("stderr was piped above");
        let mut stderr_buf = Vec::new();
        let mut out_open = true;
        let mut err_open = true;
        let mut out_chunk = [0u8; 8192];
        let mut err_chunk = [0u8; 8192];
        while out_open || err_open {
            tokio::select! {
                n = stdout_pipe.read(&mut out_chunk), if out_open => {
                    match n {
                        Ok(0) | Err(_) => out_open = false,
                        Ok(n) => live.push(LiveStream::Stdout, &out_chunk[..n]),
                    }
                }
                n = stderr_pipe.read(&mut err_chunk), if err_open => {
                    match n {
                        Ok(0) | Err(_) => err_open = false,
                        Ok(n) => {
                            live.push(LiveStream::Stderr, &err_chunk[..n]);
                            stderr_buf.extend_from_slice(&err_chunk[..n]);
                        }
                    }
                }
            }
        }
        stderr_buf
    };
    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => return install_output(false, format!("chromium install could not run: {e}")),
    };

    // Exit 0 is not the claim. The claim is that the NEXT browser call
    // works, and the resolver is one await away — so ask it (判据 §4:
    // assert the effect arrived, not that the call happened). This CLI has
    // produced exit-0-and-nothing-happened before: appendix D.9.11 records
    // `browser_pdf` answering "Saved PDF to <path>" over a file it had been
    // refused permission to write.
    if status.success() {
        match chromium_row().await {
            row if row.path.is_some() => install_output(
                true,
                format!(
                    "chromium installed and resolves at {}.",
                    row.path.unwrap_or_default()
                ),
            ),
            // `chromium_row()`'s own "Unknown" prefix (判据 §16: the same two
            // conditions — no `playwright-cli` on PATH, or the config could
            // not be read — that make the doctor and `list` answer `unknown`
            // rather than a verdict) is NOT the same fact as a verified
            // "nothing resolves". Collapsing it into the same "no browser
            // resolves" wording would tell the operator to go fix
            // `binary_path`/`download_host` on a host where those were never
            // even read (Final Review M2) — an instruction that answers a
            // question nobody asked and hides the actual, checkable
            // condition (a transient PATH or config read failure).
            row if row.status.starts_with("Unknown") => install_output(
                false,
                format!(
                    "`install-browser chromium` exited 0, but whether it resolves could not be \
                     checked afterwards ({}). This is not the same as a failed install — retry \
                     `runtime_manage{{action:\"list\"}}` once that condition clears.",
                    row.status
                ),
            ),
            row => install_output(
                false,
                format!(
                    "`install-browser chromium` exited 0 but no browser resolves afterwards ({}). \
                     Check [general.browser.runtime] binary_path and download_host.",
                    row.status
                ),
            ),
        }
    } else {
        install_output(
            false,
            format!(
                "chromium install failed (exit {}): {}. If this network blocks Playwright's \
                 CDN, set [general.browser.runtime] download_host to a mirror and try again.",
                status.code().unwrap_or(-1),
                String::from_utf8_lossy(&stderr_buf).trim()
            ),
        )
    }
}

/// One shape for everything the detached install can report, so `poll` / `wait`
/// hand the model the same envelope a background `bash` job does.
///
/// Built as an explicit field literal rather than `..Default::default()`:
/// `CodeExecOutput` derives only `Debug, Clone, Serialize`, not `Default`
/// (`src/builtin_tools/code_exec.rs:210`), so a struct-update base does not
/// exist here.
fn install_output(ok: bool, message: String) -> crate::builtin_tools::code_exec::CodeExecOutput {
    crate::builtin_tools::code_exec::CodeExecOutput {
        success: ok,
        exit_code: if ok { 0 } else { 1 },
        stdout: if ok { message.clone() } else { String::new() },
        stderr: if ok { String::new() } else { message },
        duration_ms: 0,
        language: "runtime_manage".to_string(),
        truncated: None,
        stdout_truncated_bytes: 0,
        stderr_truncated_bytes: 0,
        advisory: None,
    }
}

#[async_trait]
impl crate::tools::AlephTool for RuntimeManageTool {
    const NAME: &'static str = "runtime_manage";
    // Pruned against R9's two rulers (2026-09-06): the first draft measured
    // 569 B, over the 400 B threshold the plan sets before a ceiling raise is
    // accepted. The action semantics ("list shows what's installed", "install
    // takes a capability") are already the `RuntimeAction` enum's own variant
    // docs, so restating them here was the same fact twice (判据 §1) — cut.
    // What stays is what the enum cannot say: which runtimes exist, that
    // `chromium` is the special non-ledger one, and — the one no schema can
    // ever carry — that installs run detached and how to poll them.
    const DESCRIPTION: &'static str =
        "List or install the external runtimes Aleph shells out to (node, uv, cargo, git, \
         playwright-cli, chromium). `install` takes the `capability` a refusal named; \
         `chromium` is the browser the managed driver launches. Installs run in the \
         BACKGROUND, returning a job id at once — a download is minutes, not one call: \
         poll with `bash{process_action:\"wait\", process_id:<id>}`, or `list` again.";

    type Args = RuntimeManageArgs;
    type Output = RuntimeManageOutput;

    async fn call(&self, args: Self::Args) -> crate::error::Result<Self::Output> {
        Ok(match args.action {
            RuntimeAction::List => Self::list(&self.locator).await,
            RuntimeAction::Install => {
                Self::install(args.capability, &self.spawner, &self.locator).await
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AlephTool;

    /// A spawner that registers nothing and starts nothing.
    struct StubSpawner {
        started: std::sync::Mutex<Vec<String>>,
        answer: Result<u64, String>,
    }

    impl StubSpawner {
        fn ok(id: u64) -> Arc<Self> {
            Arc::new(Self {
                started: std::sync::Mutex::new(Vec::new()),
                answer: Ok(id),
            })
        }
        fn started(&self) -> Vec<String> {
            self.started
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl InstallSpawner for StubSpawner {
        fn spawn(&self, capability: &str) -> Result<u64, String> {
            self.started
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(capability.to_string());
            self.answer.clone()
        }
    }

    fn tool(spawner: Arc<dyn InstallSpawner>) -> RuntimeManageTool {
        RuntimeManageTool::with_parts(Arc::new(StubLocator("Ready (stub)")), spawner)
    }

    /// `install` without a capability is not "install everything" — it is a
    /// malformed call, and answering it with a guess would install whichever
    /// spec happens to be first in the table. It must also start nothing.
    #[tokio::test]
    async fn install_without_a_capability_refuses_instead_of_guessing() {
        let spawner = StubSpawner::ok(7);
        let out = tool(spawner.clone())
            .call(RuntimeManageArgs {
                action: RuntimeAction::Install,
                capability: None,
            })
            .await
            .expect("tool answers");
        assert!(!out.ok);
        assert!(out.message.contains("capability"), "{}", out.message);
        assert!(
            spawner.started().is_empty(),
            "a malformed call started an install"
        );
    }

    /// A locator that actually blocks, standing in for the real one's
    /// `chromium_resolve::DRY_RUN_TIMEOUT` (≈ 6 s). `install`'s response DOES
    /// call the locator — it builds on `Self::list`, which appends the
    /// chromium row — so a stub that resolves instantly cannot tell "the
    /// response really composed a live row" from "nothing here ever calls the
    /// locator at all" (判据 §2: a guard that cannot go red for the reason it
    /// names is not a guard). The delay is well under the real ceiling, so the
    /// test still runs fast.
    struct DelayedStubLocator(std::time::Duration);

    #[async_trait]
    impl ChromiumLocator for DelayedStubLocator {
        async fn locate(&self) -> RuntimeRow {
            tokio::time::sleep(self.0).await;
            RuntimeRow {
                name: "chromium".into(),
                status: "Ready (stub)".into(),
                path: None,
                version: None,
                purpose: None,
                supported_here: true,
            }
        }
    }

    /// **The install must not sit on the tool call.** A ~150 MB download cannot
    /// fit the 180 s per-tool budget, and `bash_exec::WAIT_MAX_TIMEOUT_SECS`
    /// (170 s) is the constraint CLAUDE.md forbids extending — the very fact
    /// this plan cites three times to justify not installing from
    /// `resolve_binary`. So the answer comes back with a job id, immediately,
    /// and names the verb that polls it.
    ///
    /// The locator genuinely sleeps (see [`DelayedStubLocator`]), so the wall
    /// clock bound proves two things at once: the response really did reach
    /// the locator (elapsed is at least the delay — a "faster" run would mean
    /// the locator was silently skipped), and nothing ELSE unbounded is on the
    /// path (elapsed stays close to the delay rather than ballooning toward
    /// what awaiting the actual installer would cost).
    #[tokio::test]
    async fn install_returns_a_job_id_immediately_instead_of_awaiting_the_download() {
        // `install`'s success arm calls `Self::list`, which calls the REAL
        // `Self::ledger()` unconditionally before ever consulting the
        // (stubbed) locator below — so without its own isolated
        // `$ALEPH_HOME`, this test's `Self::ledger()` call implicitly shares
        // the ambient environment with whatever else `cargo test` is running
        // concurrently. Measured: green alone, red running the whole module
        // once `a_failing_ledger_still_reports_ok_for_a_real_spawn` started
        // reliably clearing `$HOME`/`$ALEPH_HOME` for its own duration — that
        // test's ledger-failure branch returns `runtimes: []` before ever
        // calling `locator.locate()`, so this test's own locator delay was
        // silently skipped and its elapsed-time assertion went red for a
        // reason that had nothing to do with what it names.
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let locator_delay = std::time::Duration::from_millis(200);
        let spawner = StubSpawner::ok(42);
        let tool = RuntimeManageTool::with_parts(
            Arc::new(DelayedStubLocator(locator_delay)),
            spawner.clone(),
        );
        let started = std::time::Instant::now();
        let out = tool
            .call(RuntimeManageArgs {
                action: RuntimeAction::Install,
                capability: Some("chromium".into()),
            })
            .await
            .expect("tool answers");
        assert!(out.ok, "{}", out.message);
        assert_eq!(spawner.started(), vec!["chromium".to_string()]);
        assert!(
            out.message.contains("42"),
            "the job id must reach the model: {}",
            out.message
        );
        assert!(
            out.message.contains("process_action"),
            "the answer must name the verb that polls it: {}",
            out.message
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed >= locator_delay,
            "elapsed {elapsed:?} is shorter than the locator's own delay \
             ({locator_delay:?}) — the response stopped calling the locator"
        );
        assert!(
            elapsed < locator_delay * 5,
            "install blocked for {elapsed:?}, far past the locator's own \
             {locator_delay:?} delay — it awaited the installer, not just the \
             locator"
        );
    }

    /// I1: `install`'s `ok` must come from the SPAWN outcome, not from
    /// `list`'s own verdict. Before the fix, a ledger that failed to load
    /// inside `list` made `install` answer `ok:false` alongside "Installing …
    /// as job {id}" for a job that is, right now, actually running and
    /// holding a registry slot — the inverse of 判据 §11 (a REAL effect
    /// reported as a failure): a caller reading `ok` as the verdict would
    /// retry, and now two installs hold slots.
    ///
    /// `Self::ledger()` only fails when `get_config_dir()` cannot resolve a
    /// home directory at all — `CapabilityLedger::load_or_create` itself never
    /// errors; a missing or corrupted file degrades to a fresh in-memory
    /// ledger. So the failure is reproduced the way `get_config_dir` actually
    /// produces it: no `$ALEPH_HOME` and no `$HOME`.
    ///
    /// `HomeEnvGuards::acquire_and_clear` (not a bare `HomeEnvGuard` plus an
    /// assertion that `$ALEPH_HOME` happens to be unset, which is what this
    /// test used to do): `cargo test`'s default parallel execution runs this
    /// alongside OTHER tests in this same module that legitimately set
    /// `$ALEPH_HOME` under their own guard, on a separate mutex — measured
    /// red running the whole module, green only in isolation, which is
    /// exactly the ABBA-adjacent race `HomeEnvGuards` exists to close.
    #[tokio::test]
    async fn a_failing_ledger_still_reports_ok_for_a_real_spawn() {
        let _guard = crate::runtimes::post_install::HomeEnvGuards::acquire_and_clear();

        let spawner = StubSpawner::ok(99);
        let out = tool(spawner.clone())
            .call(RuntimeManageArgs {
                action: RuntimeAction::Install,
                capability: Some("chromium".into()),
            })
            .await
            .expect("tool answers");

        assert!(
            out.ok,
            "a successful spawn must report ok:true even when list's own \
             ledger read fails: {}",
            out.message
        );
        assert!(
            out.message.contains("99"),
            "the job id must still reach the model: {}",
            out.message
        );
        // M3: the empty `runtimes` this response carries (inherited from
        // `list`'s own failed ledger read) must not be silently readable as
        // "there is nothing installed" — the message must say it is unknown.
        assert!(out.runtimes.is_empty(), "{:?}", out.runtimes);
        assert!(
            out.message.contains("UNKNOWN") || out.message.contains("could not"),
            "an empty `runtimes` next to a failed ledger read must say it is \
             unknown, not silently pass as \"there are none\": {}",
            out.message
        );
    }

    /// The non-chromium branch takes the SAME path. It is the one that used to
    /// be worse than the download: `ensure_capability` runs npm global installs
    /// and `curl … | sh` bootstrap scripts with no timeout of its own at all.
    /// A plan that backgrounded only the browser would leave the unbounded one
    /// inline (判据 §5: 不在我这张表上的那部分呢).
    #[tokio::test]
    async fn a_ledger_capability_is_backgrounded_the_same_way() {
        let spawner = StubSpawner::ok(9);
        let out = tool(spawner.clone())
            .call(RuntimeManageArgs {
                action: RuntimeAction::Install,
                capability: Some("playwright-cli".into()),
            })
            .await
            .expect("tool answers");
        assert!(out.ok, "{}", out.message);
        assert_eq!(spawner.started(), vec!["playwright-cli".to_string()]);
        assert!(out.message.contains("9"), "{}", out.message);
    }

    /// A refused registration (the per-session cap) is reported, not swallowed:
    /// answering "installing…" over a job that was never started is the
    /// report-success-for-a-no-op shape (判据 §11).
    #[tokio::test]
    async fn a_refused_registration_is_reported_rather_than_claimed_as_started() {
        let spawner: Arc<dyn InstallSpawner> = Arc::new(StubSpawner {
            started: std::sync::Mutex::new(Vec::new()),
            answer: Err("this session already has 5 background jobs running".into()),
        });
        let out = tool(spawner)
            .call(RuntimeManageArgs {
                action: RuntimeAction::Install,
                capability: Some("chromium".into()),
            })
            .await
            .expect("tool answers");
        assert!(!out.ok);
        assert!(
            out.message.contains("background jobs running"),
            "{}",
            out.message
        );
    }

    /// An unknown capability must name what IS installable. "unknown capability:
    /// chrmium" that does not list the alternatives costs the model a whole turn
    /// discovering them.
    #[tokio::test]
    async fn an_unknown_capability_lists_the_ones_that_exist() {
        let out = RuntimeManageTool::new()
            .call(RuntimeManageArgs {
                action: RuntimeAction::Install,
                capability: Some("chrmium".into()),
            })
            .await
            .expect("tool answers");
        assert!(!out.ok);
        assert!(out.message.contains("chromium"), "{}", out.message);
        assert!(out.message.contains("playwright-cli"), "{}", out.message);
    }

    /// `chromium` is installable through this tool even though it is NOT a
    /// ledger spec — the ledger probes PATH, and Playwright's browser is never
    /// on PATH. `find_spec` must not be the gate, or the one capability the
    /// browser subsystem needs would be the one this tool cannot install.
    #[test]
    fn chromium_is_installable_and_is_deliberately_not_a_ledger_spec() {
        assert!(
            crate::runtimes::find_spec("chromium").is_none(),
            "a chromium RuntimeSpec would be probed on PATH and stay Missing forever"
        );
        assert!(is_installable("chromium"));
        assert!(is_installable("playwright-cli"));
        assert!(!is_installable("chrmium"));
    }

    /// A locator that answers from memory. The production one shells out to
    /// `playwright-cli install-browser --dry-run`, and a unit test that reached
    /// it would spawn a real node subprocess inside `cargo test --lib` — the
    /// exact discipline `src/browser/playwright_cli.rs:150-165` seals its own
    /// `provision_binary` to enforce, in its own words "so no future test can
    /// forget to seal itself", because otherwise "their green was a property of
    /// the environment, not of the code". It would also pass on a machine with
    /// no `playwright-cli` for a different reason than on one with it.
    struct StubLocator(&'static str);

    #[async_trait]
    impl ChromiumLocator for StubLocator {
        async fn locate(&self) -> RuntimeRow {
            RuntimeRow {
                name: "chromium".into(),
                status: self.0.into(),
                path: None,
                version: None,
                purpose: None,
                supported_here: true,
            }
        }
    }

    /// The I1 seam: resolves the chromium install CLI to a fixed,
    /// test-written path instead of walking the real PATH.
    struct FixedCliLocator(std::path::PathBuf);

    #[async_trait]
    impl ChromiumInstallCli for FixedCliLocator {
        async fn cli_path(&self) -> Option<std::path::PathBuf> {
            Some(self.0.clone())
        }
    }

    /// I1 (round 2): the `tokio::select!` tee inside `install_chromium` is
    /// what actually makes `bash{process_action:"poll"}` show bytes for a
    /// running chromium install. Round 1's own attempt at proving this
    /// (`a_running_install_jobs_partial_output_is_visible_to_poll`, deleted
    /// under Final Review M4) only proved `RegistrySpawner::register`'s
    /// `attach_live` wiring — its job pushed into the live tail ITSELF, so
    /// that guard stayed green even with the tee inside `install_chromium`
    /// deleted outright (判据 §4: it asserted the call happened, not that the
    /// effect arrived — confirmed by the final reviewer's own mutation, not
    /// merely inferred). Deleting it loses nothing: `register`'s ONLY
    /// production caller for install jobs is `RegistrySpawner::spawn` below,
    /// the exact path THIS test drives, so a regression in `register`'s own
    /// wiring fails this test too. This one runs the REAL `RegistrySpawner` →
    /// `run_install` → `install_chromium` chain against a fake CLI script
    /// that writes a known marker to stdout AND stderr, so the tee is the
    /// only thing that could have put those bytes where `poll` reads them.
    ///
    /// The fake CLI exits 1 (a plain argument-style failure) so the test does
    /// not also have to fake `chromium_row`'s post-install verification
    /// (`Config::load` + the real resolver) — the tee runs identically
    /// whether the child ultimately succeeds or fails.
    #[cfg(unix)]
    #[tokio::test]
    async fn install_chromiums_real_subprocess_output_reaches_poll_through_the_tee() {
        use crate::builtin_tools::process_registry::{process_registry, PollOutcome};
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let cli = tmp.path().join("fake-playwright-cli");
        const MARKER: &str = "AlephI1TeeMarker-c3f1a9";
        // The `sleep` is load-bearing: without it the script exits before the
        // test's poll loop ever runs, and the marker would only ever be
        // observable through the FINISHED result — which proves nothing about
        // the tee, since `stderr_buf` reaches the final message regardless of
        // whether `live.push` is ever called. The sleep holds the job
        // `Running` long enough that the ONLY way to see the marker in that
        // window is through the live tail the tee writes into.
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\necho '{MARKER} stdout'\necho '{MARKER} stderr' >&2\nsleep 1\nexit 1\n"
            ),
        )
        .expect("write fake cli");
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake cli");

        let spawner = RegistrySpawner::with_cli_locator(Arc::new(FixedCliLocator(cli)));
        let job_id = spawner
            .spawn(CHROMIUM)
            .expect("the registry had a free slot");

        // Poll until the tee has pushed the marker into the STILL-RUNNING
        // job's live tail. `Done` is deliberately NOT accepted as proof here:
        // the finished `CodeExecOutput` carries the complete stderr text
        // regardless of whether `live.push` was ever called (it is built from
        // the same accumulated buffer, not from the tail), so treating it as
        // a satisfying answer would let a deleted tee still pass.
        let mut saw_marker_while_running = false;
        let mut finished_without_ever_seeing_it = false;
        for _ in 0..400 {
            match process_registry().poll(job_id, None) {
                PollOutcome::Running {
                    partial: Some(p), ..
                } => {
                    let text = format!(
                        "{}{}",
                        String::from_utf8_lossy(&p.stdout),
                        String::from_utf8_lossy(&p.stderr)
                    );
                    if text.contains(MARKER) {
                        saw_marker_while_running = true;
                        break;
                    }
                }
                PollOutcome::Done(_) => {
                    finished_without_ever_seeing_it = true;
                    break;
                }
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            !finished_without_ever_seeing_it,
            "the job finished before the live tail ever showed the marker — \
             the 1s sleep in the fake CLI should have made that impossible \
             unless the tee itself never ran"
        );
        assert!(
            saw_marker_while_running,
            "poll never showed the fake CLI's own output through the live \
             tail while the job was still running — the tee inside \
             install_chromium did not reach it, or the wrong live tail was \
             attached"
        );
    }

    /// The catalogue face and the RPC face answer from the same table. A tool
    /// that listed a different set than `runtimes.list` would be the second
    /// answer to "what runtimes are there" (判据 §9).
    #[tokio::test]
    async fn list_answers_from_the_same_spec_table_as_the_rpc() {
        // The ledger path is a pure join (`utils::paths.rs:249-251`, no
        // directory creation), and every assertion below is over the static
        // `SPECS` table — but point HOME at a scratch dir anyway so the answer
        // can never become a property of the developer's own ledger.
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        let out = RuntimeManageTool::with_locator(Arc::new(StubLocator("Ready (stub)")))
            .call(RuntimeManageArgs {
                action: RuntimeAction::List,
                capability: None,
            })
            .await
            .expect("tool answers");
        assert!(out.ok, "{}", out.message);
        let names: Vec<&str> = out.runtimes.iter().map(|r| r.name.as_str()).collect();
        for spec in crate::runtimes::SPECS {
            assert!(
                names.contains(&spec.name),
                "{} missing from the tool face",
                spec.name
            );
        }
        assert!(
            names.contains(&"chromium"),
            "chromium is installable, so it must be listable"
        );
    }

    /// **The job id has to be pollable, by the verb the answer names.**
    ///
    /// The registry resolves ownership with plain equality on the session label
    /// (`process_registry.rs:671-673`), and every `bash` lookup passes exactly
    /// what `bash_exec::session_label()` returns. Register under any other
    /// derivation — `SessionKey::to_key_string()` is the tempting wrong one,
    /// and the label's own inverse doc warns about it — and the id comes back
    /// fine, names a real slot, and resolves to `NotFound` for the caller who
    /// was told to poll it: a success that leads nowhere (判据 §11).
    ///
    /// Drives the REAL registration path with a job that does nothing, so the
    /// two label derivations are proven to agree rather than assumed to.
    ///
    /// Both the registration and the lookup run inside a real
    /// `SESSION_ID.scope(...)`. Outside any scope `current_session()` is
    /// `None` for every derivation, so `bash_exec::session_label()` and a
    /// wrong stand-in would collapse to the same `None` and this test would
    /// pass no matter which one `register` used — a falsification actually
    /// run against the no-scope version confirmed exactly that (it stayed
    /// green), which is why the scope is here and not optional.
    #[tokio::test]
    async fn an_install_job_resolves_under_the_label_bash_wait_uses() {
        use crate::builtin_tools::process_registry::{process_registry, PollOutcome};
        use crate::sandbox::context::SESSION_ID;
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        let session = crate::routing::session_key::SessionKey::ephemeral("runtime-manage-label-rt");
        let (id, caller) = SESSION_ID
            .scope(session.clone(), async {
                let id = RegistrySpawner::register(
                    format!("{INSTALL_JOB_PREFIX}chromium"),
                    Arc::new(LiveTail::new()),
                    async { install_output(true, "stub".into()) },
                )
                .expect("the registry had a free slot");
                // The EXACT lookup `bash{process_action:"wait"}` performs
                // (`bash_exec.rs:498` then `:574`), taken inside the SAME
                // scope the registration ran under.
                let caller = crate::builtin_tools::bash_exec::session_label();
                (id, caller)
            })
            .await;

        assert!(
            !matches!(
                process_registry().poll(id, caller.as_deref()),
                PollOutcome::NotFound
            ),
            "the id runtime_manage handed back is not pollable by the verb its \
             own message names"
        );

        // Non-vacuity: the assertion above must be able to fail. A DIFFERENT
        // label has to answer NotFound, or `owns` is not doing what this test
        // claims and the check above would pass for a job registered under
        // anything at all.
        assert!(
            matches!(
                process_registry().poll(id, Some("some-other-session")),
                PollOutcome::NotFound
            ),
            "ownership is not label-scoped — this test proves nothing"
        );
    }

    /// A running install has to be visible where the model looks next. Without
    /// this, `list` after an `install` shows the pre-install answer and the
    /// model concludes nothing happened — a report that is true field by field
    /// and false as a whole.
    ///
    /// Registers a real slot (the registry is in-process and the abort handle
    /// is from a task that parks forever), then asserts the row is named; the
    /// job is aborted at the end so it does not outlive the test.
    #[tokio::test]
    async fn list_names_an_install_that_is_still_running() {
        use crate::builtin_tools::process_registry::{process_registry, RegisterOutcome};
        use crate::sandbox::context::SESSION_ID;
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        // Scoped to a real, unique session: `process_registry()` is a
        // process-global singleton shared by every test in this binary, and
        // an unscoped `session_label()` is `None` for every such test — which
        // would let this test's job collide with any other unscoped test's
        // `list()` call running concurrently (both "own" the same `None`
        // bucket). A unique session id keeps this test's row invisible to
        // everyone else's.
        let session =
            crate::routing::session_key::SessionKey::ephemeral("runtime-manage-list-running");
        let (out, id) = SESSION_ID
            .scope(session, async {
                let parked = tokio::spawn(async { std::future::pending::<()>().await });
                let RegisterOutcome::Registered(id) = process_registry().register_running(
                    format!("{INSTALL_JOB_PREFIX}chromium"),
                    crate::builtin_tools::bash_exec::session_label(),
                    parked.abort_handle(),
                ) else {
                    panic!("the registry refused a slot in a test with no other jobs");
                };

                let out = RuntimeManageTool::with_locator(Arc::new(StubLocator("Ready (stub)")))
                    .call(RuntimeManageArgs {
                        action: RuntimeAction::List,
                        capability: None,
                    })
                    .await
                    .expect("tool answers");
                parked.abort();
                (out, id)
            })
            .await;

        assert!(out.ok, "{}", out.message);
        assert!(
            out.message.contains(&id.to_string()) && out.message.contains("chromium"),
            "a running install must be named by list: {}",
            out.message
        );
    }

    /// The prefix filter in `running_install_jobs` is the only thing standing
    /// between "an install is running" and "something is running": without
    /// it, a `cargo build` the user backgrounded through plain `bash` would be
    /// reported as a runtime install. Falsifying that filter (removing it
    /// entirely, as opposed to inverting it to always-false) left the sibling
    /// test above green — this test is the one that would have caught it
    /// (判据 §5: 不在我这张表上的那部分呢 / 判据 §2: a guard's existence must
    /// be provable by a specific red).
    #[tokio::test]
    async fn list_does_not_mistake_an_unrelated_background_job_for_an_install() {
        use crate::builtin_tools::process_registry::{process_registry, RegisterOutcome};
        use crate::sandbox::context::SESSION_ID;
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());

        // Scoped to a real, unique session for the same reason as the sibling
        // test above: an unscoped label is `None` for every test in this
        // binary, so without a unique session this test's `list()` call could
        // see another concurrently-running test's install job (and its own
        // `assert!(!out.message.contains("install(s) still running"))` would
        // then fail on a job that has nothing to do with this test).
        let session =
            crate::routing::session_key::SessionKey::ephemeral("runtime-manage-unrelated-job");
        let (out, id) = SESSION_ID
            .scope(session, async {
                let parked = tokio::spawn(async { std::future::pending::<()>().await });
                let RegisterOutcome::Registered(id) = process_registry().register_running(
                    "cargo build".to_string(),
                    crate::builtin_tools::bash_exec::session_label(),
                    parked.abort_handle(),
                ) else {
                    panic!("the registry refused a slot in a test with no other jobs");
                };

                let out = RuntimeManageTool::with_locator(Arc::new(StubLocator("Ready (stub)")))
                    .call(RuntimeManageArgs {
                        action: RuntimeAction::List,
                        capability: None,
                    })
                    .await
                    .expect("tool answers");
                parked.abort();
                (out, id)
            })
            .await;

        assert!(out.ok, "{}", out.message);
        assert!(
            !out.message.contains(&id.to_string()),
            "a plain background job must not be reported as a runtime install: {}",
            out.message
        );
        assert!(
            !out.message.contains("install(s) still running"),
            "no install is running; the message must not claim one is: {}",
            out.message
        );
    }

    /// The doctor's fix hint (Task 7) names this tool by string. Nothing
    /// otherwise ties the two together, so a rename would quietly turn that
    /// hint into a lie — the "same fact, two expressions" shape, with the
    /// expensive copy in the text a human is told to act on (判据 §1).
    #[test]
    fn the_doctor_fix_hint_names_a_tool_that_actually_exists() {
        let hint = crate::diagnostics::checks::chromium_missing::missing_finding_for_test()
            .fix_hint
            .expect("the missing finding carries a fix hint");
        assert!(
            hint.contains(<RuntimeManageTool as AlephTool>::NAME),
            "{hint}"
        );
        assert!(
            crate::executor::BUILTIN_TOOL_DEFINITIONS
                .iter()
                .any(|d| d.name == <RuntimeManageTool as AlephTool>::NAME),
            "the tool the doctor points at must be in the catalogue"
        );
    }

    /// M2: an unreadable config makes `chromium_row()` answer `Unknown (the
    /// config could not be read: ...)`, not `Missing`. Before this fix,
    /// `install_chromium`'s post-check only tested `row.path.is_some()`, so
    /// this same row fell into the `else` arm and was reported as "no
    /// browser resolves afterwards" — collapsing "I could not check" into a
    /// verified negative, and sending the operator to fix
    /// `[general.browser.runtime]` keys the resolver never even reached.
    ///
    /// Drives the real `install_chromium` (not a stub): a fake install CLI
    /// that exits 0 immediately, against a scratch `$ALEPH_HOME` whose
    /// `config.toml` is not valid TOML — the same "unreadable config" state
    /// `Config::load()` reports as an `Err`, reached here through the exact
    /// path `chromium_row()` takes, not a mocked-up status string.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_config_that_cannot_be_read_after_install_is_reported_as_unknown_not_missing() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        std::fs::write(home.path().join("config.toml"), b"this is not { valid toml")
            .expect("write malformed config");

        // Precondition: the fixture actually reaches the row this test is
        // about — `chromium_row()`'s own "Unknown" prefix — not some other
        // shape of failure.
        let row = chromium_row().await;
        assert!(
            row.status.starts_with("Unknown"),
            "fixture does not reproduce an Unknown row: {}",
            row.status
        );

        let fake_cli = home.path().join("fake-playwright-cli");
        std::fs::write(&fake_cli, "#!/bin/sh\nexit 0\n").expect("write fake cli");
        std::fs::set_permissions(&fake_cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake cli");

        let out = install_chromium(
            &Arc::new(LiveTail::new()),
            &(Arc::new(FixedCliLocator(fake_cli)) as Arc<dyn ChromiumInstallCli>),
        )
        .await;

        assert!(
            !out.success,
            "an unreadable config cannot be reported as a successful install: {}",
            out.stderr
        );
        assert!(
            out.stderr.contains("could not be checked"),
            "an unreadable-config row must say the post-install check itself was \
             inconclusive, not that the browser is missing: {}",
            out.stderr
        );
        assert!(
            !out.stderr.contains("no browser resolves afterwards"),
            "must not spend 'unknown' as a definite negative: {}",
            out.stderr
        );
        assert!(
            !out.stderr.contains("Check [general.browser.runtime]"),
            "must not send the operator to fix config keys the resolver never \
             reached: {}",
            out.stderr
        );
    }
}
