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

use crate::runtimes::ledger::{CapabilityLedger, CapabilityStatus};
use crate::runtimes::specs::EnvFromConfig;
use crate::runtimes::{ensure_capability, find_spec, supported_on_current_os, SPECS};
use crate::sync_primitives::Arc;

/// The one capability this tool installs that the ledger does not model.
const CHROMIUM: &str = "chromium";

/// The subcommand that supplies it — the SAME argv the ledger's post-install
/// action runs (`runtimes::specs`, the `playwright-cli` entry). Written once so
/// the two paths cannot drift into installing different things.
const CHROMIUM_INSTALL_ARGS: &[&str] = &["install-browser", CHROMIUM];

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
            spawner: Arc::new(RegistrySpawner),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_locator(locator: Arc<dyn ChromiumLocator>) -> Self {
        Self {
            locator,
            spawner: Arc::new(RegistrySpawner),
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
                let mut out = Self::list(locator).await;
                out.message = format!(
                    "Installing {name} in the background as job {job_id}. It downloads over the \
                     network and will not finish inside a tool call, so poll it with \
                     `bash{{process_action:\"wait\", process_id:{job_id}}}` (or \
                     `process_action:\"poll\"` for a non-blocking peek). This tool's `list` \
                     action also reports the job while it runs."
                );
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

/// The production spawner: the same registry `bash {background: true}` uses.
pub(crate) struct RegistrySpawner;

impl RegistrySpawner {
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
    fn register<F>(command: String, job: F) -> Result<u64, String>
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
        Self::register(format!("{INSTALL_JOB_PREFIX}{capability}"), async move {
            run_install(&cap).await
        })
    }
}

/// The install itself, running detached. Returns the shape the registry stores
/// for `poll` / `wait` to hand back.
async fn run_install(capability: &str) -> crate::builtin_tools::code_exec::CodeExecOutput {
    if capability == CHROMIUM {
        install_chromium().await
    } else {
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
async fn install_chromium() -> crate::builtin_tools::code_exec::CodeExecOutput {
    let cli = tokio::task::spawn_blocking(crate::tools::probes::browser::managed_cli_path)
        .await
        .unwrap_or(None);
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
    match cmd.no_window().output().await {
        // Exit 0 is not the claim. The claim is that the NEXT browser call
        // works, and the resolver is one await away — so ask it (判据 §4:
        // assert the effect arrived, not that the call happened). This CLI has
        // produced exit-0-and-nothing-happened before: appendix D.9.11 records
        // `browser_pdf` answering "Saved PDF to <path>" over a file it had been
        // refused permission to write.
        Ok(out) if out.status.success() => match chromium_row().await {
            row if row.path.is_some() => install_output(
                true,
                format!(
                    "chromium installed and resolves at {}.",
                    row.path.unwrap_or_default()
                ),
            ),
            row => install_output(
                false,
                format!(
                    "`install-browser chromium` exited 0 but no browser resolves afterwards ({}). \
                     Check [browser.runtime] binary_path and download_host.",
                    row.status
                ),
            ),
        },
        Ok(out) => install_output(
            false,
            format!(
                "chromium install failed (exit {}): {}. If this network blocks Playwright's \
                 CDN, set [browser.runtime] download_host to a mirror and try again.",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ),
        Err(e) => install_output(false, format!("chromium install could not run: {e}")),
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

    /// **The install must not sit on the tool call.** A ~150 MB download cannot
    /// fit the 180 s per-tool budget, and `bash_exec::WAIT_MAX_TIMEOUT_SECS`
    /// (170 s) is the constraint CLAUDE.md forbids extending — the very fact
    /// this plan cites three times to justify not installing from
    /// `resolve_binary`. So the answer comes back with a job id, immediately,
    /// and names the verb that polls it.
    ///
    /// The wall-clock bound is the assertion: a stub spawner returns instantly,
    /// so anything slow here means the tool awaited the installer.
    #[tokio::test]
    async fn install_returns_a_job_id_immediately_instead_of_awaiting_the_download() {
        let spawner = StubSpawner::ok(42);
        let started = std::time::Instant::now();
        let out = tool(spawner.clone())
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
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "install blocked for {:?} — it awaited the installer",
            started.elapsed()
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
                let id =
                    RegistrySpawner::register(format!("{INSTALL_JOB_PREFIX}chromium"), async {
                        install_output(true, "stub".into())
                    })
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
}
