// Browser profile lifecycle manager.
// Manages profile instances: registration, state tracking, idle reclamation.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use arc_swap::ArcSwap;

use crate::sync_primitives::{AtomicBool, Mutex, Ordering, RwLock};

use super::backend::BrowserBackend;
use super::cdp_backend::migration;
use super::chrome_mcp::ChromeMcpDriver;
use super::chrome_mcp_backend::ChromeMcpBackend;
use super::engine;
use super::engine::process::{EngineProcess, LaunchRequest};
use super::engine::registry::EngineRegistry;
use super::engine::{Engine, EngineHandle, EngineLaunch};
use super::error::BrowserError;
use super::network_policy::{BrowserSsrfGuard, PolicyViolation, SsrfConfig};
use super::playwright_cli::PlaywrightCliDriver;
use super::playwright_cli_backend::PlaywrightCliBackend;
use super::playwright_launch::{LaunchPolicy, SessionLaunch};
use super::profile::{
    BrowserDriver, BrowserSystemConfig, BrowserType, PlaywrightCliConfig, ProfileConfig,
};
use super::tab_registry::{tab_ids, TabRegistry};

/// The manager the running daemon actually serves browser tools from, so a
/// config write can reach it (see [`apply_policy_live`]).
///
/// A `Weak` on purpose: the handle must not keep a manager alive past its
/// owner, and a stale entry must fail to upgrade rather than silently apply a
/// policy to a manager nobody uses. Published by [`ProfileManager::spawn_idle_reaper`].
static LIVE_MANAGER: Mutex<Option<Weak<ProfileManager>>> = Mutex::new(None);

/// Hot-apply a new SSRF policy onto the running browser manager.
///
/// Returns `true` when it landed. `false` means no manager is published (a CLI
/// process, a test, or before boot wired one up) and the caller must NOT report
/// the change as live — the same honest-downgrade contract as
/// [`crate::config::live_apply::apply_live_sections`], whose `route` arm this
/// mirrors: a process-global handle, poked from wherever the config write lands,
/// and a no-op that says so when it is absent.
///
/// Scope is the SSRF policy only. Everything else in `BrowserSystemConfig`
/// (per-profile drivers, the Playwright CLI timeouts, the Chrome launch
/// snapshot the MCP driver holds) is still captured at construction and needs a
/// restart; claiming otherwise here would just move the lie.
pub fn apply_policy_live(policy: SsrfConfig) -> bool {
    let handle = LIVE_MANAGER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    apply_policy_to(handle.as_ref(), policy)
}

/// Body of [`apply_policy_live`] against an explicit handle, so both arms —
/// including "the published manager is gone" — are testable without racing
/// whatever else this process published into the global.
fn apply_policy_to(handle: Option<&Weak<ProfileManager>>, policy: SsrfConfig) -> bool {
    match handle.and_then(Weak::upgrade) {
        Some(mgr) => {
            mgr.apply_policy(policy);
            true
        }
        None => {
            tracing::debug!(
                "browser SSRF policy saved but not hot-applied: no live ProfileManager is published"
            );
            false
        }
    }
}

/// Stop the browsers of the manager the running daemon serves.
///
/// Shaped exactly like [`crate::builtin_tools::bash_exec::kill_all_running_background`],
/// for the same reason its comment gives at the shutdown call site: an
/// automatic teardown is best-effort once the runtime itself is being torn
/// down, so the daemon calls this explicitly. Returns 0 — honestly — when no
/// manager is published (a CLI process, a test, or before boot wired one up).
pub async fn shutdown_browsers_global(budget: Duration) -> usize {
    // The `.clone()` drops the `MutexGuard` before the await below: holding a
    // `std::sync::MutexGuard` across a yield point is what this shape avoids,
    // and it became load-bearing the moment `shutdown_browsers` went async.
    let handle = LIVE_MANAGER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    match handle.as_ref().and_then(Weak::upgrade) {
        Some(mgr) => mgr.shutdown_browsers(budget).await,
        None => 0,
    }
}

/// Manages the lifecycle of browser profiles.
/// What a completed [`ProfileManager::switch_engine`] actually moved.
///
/// Counts, not booleans: "the switch succeeded" and "the switch carried your
/// login" are different facts, and only the numbers tell them apart.
#[derive(Debug)]
pub struct SwitchReport {
    pub from: Engine,
    pub to: Engine,
    pub cookies_moved: usize,
    pub tabs_reopened: usize,
    pub local_storage_origins: usize,
    /// Everything the migration could not carry. Rendered to the model
    /// verbatim; empty means nothing was lost, not "nothing was checked".
    pub warnings: Vec<String>,
    /// The new page tree, or `None` when the switch landed but the tree could
    /// not be read. `None` is not a failure of the switch — see
    /// [`ProfileManager::switch_engine`]'s note on the point of no return — and
    /// the reason is in [`Self::warnings`].
    pub snapshot: Option<super::types::SnapshotOutput>,
}

pub struct ProfileManager {
    profiles: RwLock<HashMap<String, ManagedProfile>>,
    /// Live SSRF policy. Swappable because `browser.update` writes a new one at
    /// runtime and every backend is built per call — a boot-time snapshot meant
    /// the RPC reported success while the running guard never changed.
    ssrf_guard: ArcSwap<BrowserSsrfGuard>,
    config: BrowserSystemConfig,
    chrome_mcp_driver: Arc<ChromeMcpDriver>,
    playwright_cli_driver: Arc<PlaywrightCliDriver>,
    idle_reaper_started: AtomicBool,
    /// Per-tab lifecycle tracking for Managed/Cdp profiles (idle reclamation
    /// + cap) AND the tab-identity registry (targetId / last-url per tab).
    ///
    /// An `Arc` because the cdp backend — constructed per call — records the
    /// identities it discovers straight into it.
    tab_registry: Arc<TabRegistry>,
    /// The live CDP engines, for `driver = "cdp"` profiles.
    ///
    /// An `Arc` because [`Self::get_backend`] is SYNCHRONOUS (and the idle
    /// reaper calls it), so a backend cannot be handed a resolved handle at
    /// construction — resolving one means launching, and launching is async.
    /// The registry is what a sync constructor can hand over; the backend
    /// resolves on its own first async call.
    engines: Arc<EngineRegistry>,
}

struct ManagedProfile {
    config: ProfileConfig,
    /// The engine a runtime override has moved this profile onto, if any.
    ///
    /// **A second field rather than writing `config.engine`, and that is the
    /// whole point.** `config.user_data_dir` names a directory in ONE engine's
    /// on-disk format, and [`ProfileManager::request_from`] decides which one
    /// by asking `config.resolved_engine(…)`. If an override wrote that same
    /// field, the answer to "which engine does the operator's directory belong
    /// to" would be derived from a value the override moves — so after one
    /// switch a relaunch would point Chromium at obscura's store, switching
    /// back would abandon the operator's directory, and the warning text would
    /// name the wrong engine. One fact, and only the config may author it
    /// (判据 §1).
    ///
    /// In memory only. A restart puts the profile back on the engine its config
    /// names, and both callers say so in the text the model reads.
    adopted_engine: Option<Engine>,
    last_activity: std::time::Instant,
    /// `Some(principal)` when this entry was materialized for one principal by
    /// [`ProfileManager::principal_profile`] rather than read from config.
    /// Such an entry is never a BASE another principal is derived from, never
    /// answers to its own key typed back in, and never appears in
    /// [`ProfileManager::list_profiles_for`].
    materialized_for: Option<String>,
}

impl ProfileManager {
    #[must_use]
    pub fn new(config: BrowserSystemConfig) -> Self {
        let ssrf_guard = ArcSwap::from_pointee(BrowserSsrfGuard::new(config.policy.clone()));
        let playwright_cli_driver = Arc::new(PlaywrightCliDriver::new(
            config.playwright_cli.clone(),
            config.runtime.clone(),
        ));

        let mut profiles = HashMap::new();

        if config.profiles.is_empty() {
            // The `default` profile, from the type's own defaults: driver =
            // Cdp, engine = None (i.e. follow `default_engine`, which is
            // Obscura). Named nowhere here on purpose — see the sibling
            // injection below.
            profiles.insert(
                "default".into(),
                ManagedProfile {
                    config: ProfileConfig::default(),
                    adopted_engine: None,
                    last_activity: std::time::Instant::now(),
                    materialized_for: None,
                },
            );
        } else {
            for (name, profile_config) in &config.profiles {
                profiles.insert(
                    name.clone(),
                    ManagedProfile {
                        config: profile_config.clone(),
                        adopted_engine: None,
                        last_activity: std::time::Instant::now(),
                        materialized_for: None,
                    },
                );
            }
        }

        // Auto-inject "default" if the operator's config did not name one.
        //
        // `ProfileConfig::default()` outright, with no `driver:` override. The
        // override used to say `Managed` here while the empty-config branch
        // above took the type's default — two sites answering one question,
        // and the only way to see them disagree was to run with an empty
        // config (判据 §1, §6).
        //
        // There is a THIRD writer and it is not in this file: the Panel's PUT
        // handler maps `update.default_driver` and writes it into this profile
        // on every save. It goes through `BrowserDriver::from_wire` and refuses
        // what it does not recognise (`gateway::handlers::browser_config`), so
        // a Panel save no longer coerces this profile back off `Cdp` — that
        // fix is a precondition of this flip, not a consequence of it.
        if !profiles.contains_key("default") {
            profiles.insert(
                "default".into(),
                ManagedProfile {
                    config: ProfileConfig::default(),
                    adopted_engine: None,
                    last_activity: std::time::Instant::now(),
                    materialized_for: None,
                },
            );
        }

        // Auto-inject "user" profile with ExistingSession driver if not already present.
        if !profiles.contains_key("user") {
            profiles.insert(
                "user".into(),
                ManagedProfile {
                    config: ProfileConfig {
                        browser: BrowserType::Chrome,
                        driver: BrowserDriver::ExistingSession,
                        ..Default::default()
                    },
                    adopted_engine: None,
                    last_activity: std::time::Instant::now(),
                    materialized_for: None,
                },
            );
        }

        // The Chrome MCP driver consults the profile map when it has to launch
        // Chrome itself (engine preference, proxy, user-data-dir, extra args).
        // Hand it the merged set — including the auto-injected "default"/"user"
        // entries above — so its view matches what the manager routes on.
        let chrome_mcp_driver = Arc::new(ChromeMcpDriver::new(
            config.chrome_mcp.clone(),
            profiles
                .iter()
                .map(|(name, p)| (name.clone(), p.config.clone()))
                .collect(),
        ));

        // Read out of `config` before it moves into `Self` below.
        let mut processes: HashMap<Engine, Arc<dyn EngineProcess>> = HashMap::new();
        processes.insert(
            Engine::Chromium,
            Arc::new(engine::chromium::ChromiumLauncher::new(
                config.runtime.clone(),
            )),
        );
        processes.insert(
            Engine::Obscura,
            Arc::new(engine::obscura::ObscuraLauncher::new(
                config.obscura.clone(),
            )),
        );

        // ⚠️ `cdp_command_timeout()` is captured HERE, at construction, and a
        // `browser.update` that changes it needs a restart. The SSRF bit two
        // screens down (`launch_request_for`'s `allow_private_network`) is
        // deliberately the opposite — it reads the LIVE guard on every launch.
        // Both are right for what they are: the timeout is handed to the
        // connection when it is opened and cannot be re-read afterwards, while
        // the SSRF answer is written into an argv this manager composes fresh
        // each time. Said out loud because two adjacent values behaving
        // differently with nothing explaining it is how the next reader
        // "fixes" one of them.
        let engines = Arc::new(EngineRegistry::new(
            processes,
            config.cdp_command_timeout(),
            engine::readiness::READY_GATE_BUDGET,
        ));

        Self {
            profiles: RwLock::new(profiles),
            ssrf_guard,
            config,
            chrome_mcp_driver,
            playwright_cli_driver,
            idle_reaper_started: AtomicBool::new(false),
            tab_registry: Arc::new(TabRegistry::new()),
            engines,
        }
    }

    /// Hot-swap the SSRF policy. Backends are constructed per call by
    /// [`Self::get_backend`], so the next browser action — and every direct
    /// `check_*` on this manager — uses the new policy without a restart.
    pub fn apply_policy(&self, policy: SsrfConfig) {
        self.ssrf_guard
            .store(Arc::new(BrowserSsrfGuard::new(policy)));
        tracing::info!("browser SSRF policy hot-applied");
    }

    /// Spawn the idle-profile reaper on a background tokio task, at most once
    /// per `ProfileManager` instance. The reaper sweeps every `interval_secs`
    /// and tears down Chrome MCP sessions whose profile is past its idle
    /// timeout. Idempotent — subsequent calls are no-ops.
    ///
    /// It is also where the previous run's orphaned Chromium processes are
    /// reaped — same argument as the live-config handle below: "the manager
    /// whose reaper runs" is precisely "the manager the daemon serves from".
    ///
    /// Also publishes this manager as the process-global live-config target
    /// (see [`apply_policy_live`]). This is the daemon's one boot hook that
    /// already owns the `Arc` and runs exactly once per served manager — "the
    /// manager whose reaper runs" is precisely "the manager the daemon serves
    /// from", and a `ProfileManager` built ad hoc (tests, CLI) never calls this
    /// and so never claims the handle.
    pub fn spawn_idle_reaper(self: &Arc<Self>, interval_secs: u64) {
        if self.idle_reaper_started.swap(true, Ordering::AcqRel) {
            return;
        }
        // BROWSER-R4-08: claim-or-skip rather than last-write-wins.
        // The previous shape overwrote LIVE_MANAGER unconditionally,
        // so a second ProfileManager invocation (a test that builds
        // its own manager, a re-initialised daemon) would silently
        // steal the handle and the live apply_policy_live arm would
        // hot-apply to whichever manager most recently published.
        // Only install the handle when the slot is empty or the
        // existing weak has been dropped (try_unwrap succeeds).
        let mut slot = LIVE_MANAGER.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            // The previous manager is still alive: refuse to steal the
            // handle. The caller (typically a test) can run its own
            // reaper locally; it does not need the global one.
            Some(existing) if existing.strong_count() > 0 => {
                tracing::warn!(
                    "ProfileManager::spawn_idle_reaper: live manager already installed; \
                     refusing to claim the global reaper slot"
                );
                return;
            }
            // Slot empty OR previous weak handle is already dead — the
            // latter means the prior daemon (or test) has fully torn down.
            // Surface the steal so a future refactor that changes the
            // install order shows up in logs instead of silently shadowing
            // the live-config target.
            Some(_) | None => {
                tracing::info!(
                    "ProfileManager::spawn_idle_reaper: claiming the global reaper slot"
                );
            }
        }
        // Boot hook, and the only one that runs exactly once per SERVED
        // manager (a `ProfileManager` built by a test or a CLI never claims the
        // slot above). Anything Aleph launched before a crash is still running:
        // Chrome does not exit when its parent does, and under `attach` the CLI
        // was never its parent anyway.
        //
        // Detached because boot must not wait for it — nothing downstream
        // reads the count.
        tokio::spawn(async {
            match Self::sweep_orphaned_engines().await {
                Ok(outcome) => {
                    if outcome.reaped > 0 {
                        tracing::info!(
                            "reaped {} orphaned chromium process(es) from a previous run",
                            outcome.reaped
                        );
                    }
                    // Not one-shot: this fires on EVERY boot sweep for as
                    // long as the count stays nonzero, not only the sweep
                    // that first quarantined it (Final Review M6) — the
                    // defect was that nothing ever said this again after
                    // the initial rename.
                    if outcome.corrupt_pending > 0 {
                        tracing::warn!(
                            "{} unparseable chromium sidecar(s) remain quarantined as \
                             .corrupt — a live orphaned browser may be behind one and \
                             unreapable until its own session key is relaunched",
                            outcome.corrupt_pending
                        );
                    }
                    if outcome.corrupt_superseded > 0 {
                        tracing::info!(
                            "cleared {} stale .corrupt sidecar(s), superseded by a fresher \
                             record under the same session key",
                            outcome.corrupt_superseded
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "the orphaned-chromium sweep did not complete")
                }
            }
        });
        *slot = Some(Arc::downgrade(self));
        let weak = Arc::downgrade(self);
        let interval = std::time::Duration::from_secs(interval_secs.max(5));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(mgr) = weak.upgrade() else {
                    tracing::debug!("ProfileManager idle reaper exiting (manager dropped)");
                    break;
                };
                let reaped = mgr.reap_idle().await;
                if reaped > 0 {
                    tracing::info!("Browser idle reaper swept {reaped} profile(s)");
                }
                let tabs = mgr.reap_idle_tabs().await;
                if tabs > 0 {
                    tracing::info!("Browser idle reaper closed {tabs} idle/over-cap tab(s)");
                }
            }
        });
    }

    /// The boot sweep's outward-reaching leaf: read the real sidecar registry
    /// and kill whatever a previous process left running.
    ///
    /// Sealed under `cfg(test)` for the same reason
    /// `PlaywrightCliDriver::provision_binary` is, and the seal is not
    /// theoretical here: [`Self::spawn_idle_reaper`] **has a unit-test caller**
    /// (`gateway::handlers::browser_config`), and this task is detached, so it
    /// outlives the test body — including that test's `AlephHomeEnvGuard`. It
    /// would therefore resolve the *developer's real* `$ALEPH_HOME` after the
    /// guard restored it, and kill the Chromium of an Aleph they have running.
    ///
    /// What the seal costs is only the wire, not the decision: the decision is
    /// covered against injected effects in `engine::process::reap_orphans`, and
    /// the wire is pinned by `the_boot_hook_still_calls_the_orphan_sweep`.
    ///
    /// Off the async worker: the sweep does a `read_dir`, a `sysinfo` refresh
    /// per record and possibly a kill, and `with_process_specifics` is
    /// documented as syscall-heavy.
    #[cfg(not(test))]
    async fn sweep_orphaned_engines(
    ) -> Result<super::engine::process::ReapOutcome, tokio::task::JoinError> {
        tokio::task::spawn_blocking(super::engine::process::reap_orphans_now).await
    }

    /// The sealed twin. See the production one above for why it is sealed.
    #[cfg(test)]
    #[allow(clippy::unused_async)]
    async fn sweep_orphaned_engines(
    ) -> Result<super::engine::process::ReapOutcome, tokio::task::JoinError> {
        Ok(super::engine::process::ReapOutcome::default())
    }

    /// The managed driver's configuration, for the one consumer that runs a
    /// `playwright-cli` session of its own rather than through a profile:
    /// `pdf_generate`'s browser engine.
    ///
    /// Exposed rather than left to `PlaywrightCliConfig::default()` because a
    /// second construction site inherits none of the first's settings — the PDF
    /// engine was resolving its binary as though the operator had pinned
    /// nothing, so an install whose `binary_path` points off `PATH` had working
    /// browser tools and a PDF engine that either fell back to the native
    /// renderer or reached for the network installer.
    #[must_use]
    pub const fn playwright_cli_config(&self) -> &PlaywrightCliConfig {
        &self.config.playwright_cli
    }

    /// The `[browser.runtime]` section — where this Aleph's Chromium comes from.
    ///
    /// The twin of [`Self::playwright_cli_config`], and it exists for the same
    /// reason: `pdf_generate` builds a `PlaywrightCliDriver` of its own, and
    /// that driver now launches a browser, so a construction site that
    /// inherited the CLI settings but not the browser ones would honour an
    /// operator's pin in one half of Aleph and ignore it in the other.
    ///
    /// ⚠️ Task 6 was told to add `ProfileManager` accessors. This is one of
    /// them, added early because `pdf_generate` needed it; do not add a second.
    pub const fn runtime_config(&self) -> &super::profile::BrowserRuntimeConfig {
        &self.config.runtime
    }

    /// Route a profile to its appropriate `BrowserBackend` instance.
    ///
    /// - `BrowserDriver::Managed`         → `PlaywrightCliBackend`
    /// - `BrowserDriver::ExistingSession` → `ChromeMcpBackend`
    /// - `BrowserDriver::Cdp`             → `CdpBackend` (either engine)
    ///
    /// The backend is constructed per call and never stored, which is why it
    /// can hold an `Arc` back to the engine registry without a cycle — and why
    /// a `Cdp` profile's engine is resolved lazily, inside the backend, rather
    /// than here: this function is synchronous, and it is also what
    /// [`Self::reap_idle_tabs`] calls, which must never launch a browser.
    pub fn get_backend(&self, profile_name: &str) -> Result<Arc<dyn BrowserBackend>, BrowserError> {
        let cfg = self
            .get_config(profile_name)
            .ok_or_else(|| BrowserError::ProfileNotFound(profile_name.into()))?;
        match cfg.driver {
            BrowserDriver::Managed => {
                let headless = cfg.headless.unwrap_or(self.config.playwright_cli.headless);
                Ok(Arc::new(PlaywrightCliBackend::new(
                    self.playwright_cli_driver.clone(),
                    profile_name.to_string(),
                    self.ssrf_guard.load_full(),
                    SessionLaunch::from_profile(&cfg, headless),
                )))
            }
            BrowserDriver::ExistingSession => Ok(Arc::new(ChromeMcpBackend::new(
                self.chrome_mcp_driver.clone(),
                profile_name.to_string(),
                self.ssrf_guard.load_full(),
            ))),
            BrowserDriver::Cdp => {
                // ONE source for both halves of the backend's identity:
                // `launch_request_for` writes `req.profile` AND
                // `req.session_key` from the same `profile_name` this call
                // receives (already principal-scoped — the tool layer resolves
                // `principal_profile` before calling in), so the registry key,
                // the sidecar name and the profile agree by construction
                // rather than by coincidence (判据 §1). `CdpBackend::new`
                // takes no second profile argument to disagree with it.
                let (engine, req) = self.launch_request_for(profile_name)?;
                Ok(Arc::new(super::cdp_backend::CdpBackend::new(
                    self.engines.clone(),
                    engine,
                    req,
                    // The LIVE guard, loaded per call: backends are built per
                    // call precisely so a `browser.update` reaches the next
                    // action without a restart.
                    self.ssrf_guard.load_full(),
                    self.cdp_command_timeout(),
                    // The shared identity registry: the backend records every
                    // tab↔targetId mapping it discovers into it, so the answer
                    // to "is this tab still that tab" survives the per-call
                    // backend itself.
                    self.tab_registry.clone(),
                )))
            }
        }
    }

    /// The registry every CDP backend resolves its engine handle through.
    ///
    /// Handed out as an `Arc` so [`Self::get_backend`] — which is synchronous —
    /// can give a backend something that resolves later.
    #[must_use]
    pub const fn engines(&self) -> &Arc<EngineRegistry> {
        &self.engines
    }

    /// The shared tab-identity + activity registry. The cdp backend records
    /// into it; the tool layer reads `resolve_identity` / `last_url` from it.
    #[must_use]
    pub const fn tab_registry(&self) -> &Arc<TabRegistry> {
        &self.tab_registry
    }

    /// The per-command CDP timeout both engines are driven with.
    ///
    /// Delegates to [`BrowserSystemConfig::cdp_command_timeout`] rather than
    /// re-deriving `Duration::from_secs(cdp_command_timeout_secs)`: the
    /// `< 60 s` rule (obscura's own guillotine) is enforced once, at config
    /// load, and a second derivation here is a second place for the two to
    /// disagree the day one of them moves (判据 §1).
    #[must_use]
    pub fn cdp_command_timeout(&self) -> Duration {
        self.config.cdp_command_timeout()
    }

    /// Everything a launch of `profile` needs, and which engine it is for.
    ///
    /// **Synchronous and free of I/O on purpose.** It reads the profile config
    /// and the live SSRF guard, nothing else — so the synchronous
    /// [`Self::get_backend`] can call it, and so "what would we launch" is
    /// answerable without launching anything. A sensor must not create what it
    /// measures, which is the rule `playwright_launch::LaunchPolicy`'s own doc
    /// states.
    ///
    /// The engine it answers is the **live** one — [`Self::live_engine`], which
    /// a runtime override can move — while `request_from`'s `configured`
    /// argument stays the **config's**. Those are two facts and this is the one
    /// place both are needed at once: the request must carry the data dir of
    /// the engine actually being launched, and whether the operator's
    /// `user_data_dir` applies is a question about what they configured.
    pub fn launch_request_for(
        &self,
        profile: &str,
    ) -> Result<(Engine, LaunchRequest), BrowserError> {
        let cfg = self
            .get_config(profile)
            .ok_or_else(|| BrowserError::ProfileNotFound(profile.into()))?;
        let configured = cfg.resolved_engine(self.config.default_engine);
        let engine = self.adopted_engine(profile).unwrap_or(configured);
        let req = self.request_from(&cfg, profile, engine, configured)?;
        Ok((engine, req))
    }

    /// The engine a runtime override has moved `profile` onto, if any.
    ///
    /// `None` is "no override", never "obscura": the fallback belongs to
    /// `resolved_engine`, and answering a default here would give this fact a
    /// second author (判据 §1).
    fn adopted_engine(&self, profile: &str) -> Option<Engine> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles.get(profile).and_then(|p| p.adopted_engine)
    }

    /// [`Self::launch_request_for`] for a NAMED engine, which may not be the
    /// one this profile resolves to.
    ///
    /// Two callers, both of which exist because an engine can be chosen per
    /// call: `browser_open{engine}` on a cold profile, and `switch_engine`'s
    /// target. Both need the data dir of the engine they are about to start,
    /// and `launch_request_for` can only answer for the configured one — hand
    /// its request to the other engine and Chromium is pointed at obscura's
    /// store (or the reverse). Neither engine reports that as a mismatch; each
    /// one just finds a directory it does not understand and starts empty.
    pub fn launch_request_for_engine(
        &self,
        profile: &str,
        engine: Engine,
    ) -> Result<LaunchRequest, BrowserError> {
        let cfg = self
            .get_config(profile)
            .ok_or_else(|| BrowserError::ProfileNotFound(profile.into()))?;
        let configured = cfg.resolved_engine(self.config.default_engine);
        self.request_from(&cfg, profile, engine, configured)
    }

    /// The one derivation both of the above share.
    ///
    /// `configured` is the engine the profile's own config resolves to, and it
    /// decides exactly one thing: whether `user_data_dir` applies. That setting
    /// names a directory in ONE engine's on-disk format, so the other engine
    /// gets the managed path under its own `data_subdir` instead — two
    /// directories per profile is the point, and sharing one would hand each
    /// engine a store it cannot read. [`Self::switch_engine`] states the
    /// substitution in its warnings rather than performing it silently.
    fn request_from(
        &self,
        cfg: &ProfileConfig,
        profile: &str,
        engine: Engine,
        configured: Engine,
    ) -> Result<LaunchRequest, BrowserError> {
        let data_dir = match &cfg.user_data_dir {
            Some(dir) if engine == configured => std::path::PathBuf::from(dir),
            _ => super::playwright_launch::browser_state_dir(engine.data_subdir())?
                .join(super::playwright_launch::sanitize_session_key(profile)),
        };
        Ok(LaunchRequest {
            profile: profile.to_string(),
            session_key: profile.to_string(),
            data_dir,
            headless: cfg.headless.unwrap_or(self.config.playwright_cli.headless),
            proxy: cfg.proxy.clone(),
            browser: cfg.browser.clone(),
            // The LIVE guard, not the boot snapshot: `apply_policy` swaps
            // it at runtime and this argv is written once, so reading the
            // stale copy would hand obscura a permission the running policy
            // has already withdrawn.
            allow_private_network: self.ssrf_guard.load().allows_private_network(),
            stealth: matches!(
                self.config.obscura.variant,
                super::profile::ObscuraVariant::Stealth
            ),
            extra_args: cfg.extra_args.clone(),
        })
    }

    /// The live engine for `profile`, on the engine its config resolves to.
    ///
    /// A thin delegate: the derivation is [`Self::launch_request_for`]'s and
    /// the caching is the registry's. Kept as a method because the tool layer
    /// and `switch_engine` (Task 19) address a profile, not a registry.
    pub async fn engine_handle(
        &self,
        profile: &str,
        gate: EngineLaunch,
    ) -> Result<Arc<EngineHandle>, BrowserError> {
        let (engine, req) = self.launch_request_for(profile)?;
        self.engines.handle(engine, &req, gate).await
    }

    /// [`Self::engine_handle`] with the engine named explicitly — the face
    /// `browser_open{engine}` and `switch_engine` use. A profile already
    /// running a different engine is refused, not swapped (`EngineMismatch`).
    ///
    /// The request comes from [`Self::launch_request_for_engine`], not from
    /// [`Self::launch_request_for`]: the whole point of this entry is that
    /// `engine` may differ from the one the profile resolves to, and the
    /// configured engine's request carries the configured engine's data dir.
    pub async fn engine_handle_for(
        &self,
        profile: &str,
        engine: Engine,
        gate: EngineLaunch,
    ) -> Result<Arc<EngineHandle>, BrowserError> {
        let req = self.launch_request_for_engine(profile, engine)?;
        self.engines.handle(engine, &req, gate).await
    }

    /// What `browser_open` should run on — `None` when this profile's driver
    /// has no engine at all.
    ///
    /// **The driver gate is the FIRST thing, and it is not conditional on
    /// `requested`.** It used to read `requested.is_some() && driver != Cdp`,
    /// i.e. "only an explicit override can be wrong" — but
    /// `ProfileConfig::resolved_engine` answers `Engine::Chromium` for
    /// `Managed` and `ExistingSession` (it must: a legacy driver IS a
    /// Chromium). So a plain `browser_open` on either of them launched a CDP
    /// Chromium that nothing went on to use, and on a default install —
    /// obscura, no playwright chain — it did worse: `ChromiumLauncher::launch`
    /// demanded `managed_cli_path()` before it resolved a binary (it no longer
    /// does — W5 moved that lookup into `chromium_resolve::resolve_binary`'s
    /// third route, whose doc owns the subject), so at the time
    /// `browser_open{profile:"user"}` **failed outright, blaming
    /// playwright-cli**, on a host whose own `existing_session_driver_ready()`
    /// (`find_chromium() && which("npx")`, deliberately WITHOUT
    /// playwright-cli) calls that driver ready. `ProfileManager::new`
    /// auto-injects that `user` profile on every install, so it was not a
    /// configuration anyone had to choose.
    ///
    /// `None` rather than the engine `resolved_engine` would name: there is no
    /// engine process on this path, and a name on `BrowserOpenOutput.engine`
    /// for a browser that does not exist is the wrong label, not a helpful
    /// default (判据 §8, §17).
    ///
    /// Deliberately thin otherwise: "is a different engine already running for
    /// this profile" is a question [`EngineRegistry::handle`] already answers
    /// under its own lock, by returning `EngineMismatch` (the
    /// `existing.engine != engine` arm). Re-deriving it here from a second read
    /// of the map would be the same fact with two answers, and the registry's
    /// is the one that cannot race.
    ///
    /// A successful launch on a NON-configured engine also moves the profile
    /// onto it — see [`Self::adopt_engine`]. Without that, `browser_open
    /// {engine:"chromium"}` on an obscura profile would succeed exactly once
    /// and every verb after it would answer `EngineMismatch`, because every
    /// other verb resolves its engine from the profile (`get_backend` →
    /// `launch_request_for`). That adoption is **sticky for the life of the
    /// server process**, which is why nothing here calls it a "one-shot"
    /// override any more: `browser_open`'s own success message states the
    /// boundary, the way `switch_engine`'s does.
    pub(crate) async fn prepare_engine(
        &self,
        profile: &str,
        requested: Option<Engine>,
    ) -> Result<Option<Engine>, BrowserError> {
        let cfg = self
            .get_config(profile)
            .ok_or_else(|| BrowserError::ProfileNotFound(profile.into()))?;
        if cfg.driver != BrowserDriver::Cdp {
            return match requested {
                // Asking for an engine on a driver that has none is a model
                // mistake worth naming, and it is refused BEFORE any launch.
                Some(_) => Err(BrowserError::ActionFailed(format!(
                    "profile '{profile}' uses driver '{}', which has no engine to choose; \
                     set driver = \"cdp\" on it with `browser_profile` first",
                    cfg.driver.as_wire()
                ))),
                // The ordinary call. Nothing to prepare and nothing to launch;
                // the backend this profile really uses is built by
                // `make_backend` a few lines later in `browser_open`.
                None => Ok(None),
            };
        }
        let configured = cfg.resolved_engine(self.config.default_engine);
        // The LIVE engine is the default, not the configured one: a profile an
        // earlier override moved onto chromium must keep answering on chromium,
        // or a plain `browser_open` would ask the registry for obscura and be
        // told `EngineMismatch` about a browser it is already using.
        let target =
            requested.unwrap_or_else(|| self.adopted_engine(profile).unwrap_or(configured));
        // `EngineLaunch` is payload-free: `launch_request_for_engine` already
        // derived everything a launch needs, so a second `SessionLaunch` here
        // would be that derivation's second author (判据 §1).
        self.engine_handle_for(profile, target, EngineLaunch::Allow)
            .await?;
        self.set_adopted_engine(profile, (target != configured).then_some(target));
        Ok(Some(target))
    }

    /// Record — or clear — the runtime engine override for `profile`.
    ///
    /// **In memory only, and deliberately.** The escape hatch is a decision
    /// about this session, not an edit to the operator's config file; a restart
    /// puts the profile back on the engine its config names, and both callers
    /// say so in the text the model reads.
    ///
    /// It has to happen at all because every verb but the two that take an
    /// `engine` argument resolves its engine from the profile —
    /// [`Self::launch_request_for`], which `get_backend` calls to build a
    /// `CdpBackend`, and `EngineRegistry::handle` refuses a handle that does
    /// not match. Swapping the registry entry without this leaves a browser
    /// nothing can address — a switch that reports success over a profile the
    /// next call cannot use (判据 §11).
    ///
    /// `None` **clears** it, and that arm is live rather than defensive: a
    /// switch back to the configured engine, and a relaunch that lands there
    /// after the override's browser died, both reach it. Leaving a stale
    /// `Some(configured)` behind would be a second, redundant author of a fact
    /// the config already states.
    fn set_adopted_engine(&self, profile: &str, engine: Option<Engine>) {
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = profiles.get_mut(profile) {
            entry.adopted_engine = engine;
        }
    }

    /// Move `profile` onto `to`, carrying the state spec §5.5 names: every
    /// cookie, each open tab's URL and scroll, and per-origin localStorage.
    /// Everything else — the JS heap, in-flight form input, the history stack,
    /// sessionStorage — is gone, and `BrowserSessionTool::DESCRIPTION` is where
    /// that is stated for the model.
    ///
    /// **Human control lease (plan 2's hook point, deliberately absent today).**
    /// When a person holds the control lease on this profile's browser, this
    /// method must refuse rather than pull the browser out from under them —
    /// the check belongs immediately after the `AlreadyOnEngine` guard below,
    /// before anything is exported. There is no lease to check in this round:
    /// the live view that mints one is plan 2, and a `false`-returning stub
    /// here would be a gate that has never once been closed (判据 §2). This
    /// comment is the hook point, placed where the edit will be made.
    ///
    /// The order is the contract, chosen so **every failure before the last
    /// step leaves the user on the browser they already had**:
    ///
    /// 1. find the source (still parked, still serving)
    /// 2. read it (nothing written anywhere)
    /// 3. `launch_detached` the target — deliberately NOT `engine_handle_for`:
    ///    the source is still parked under this profile, which is exactly the
    ///    case `EngineRegistry::handle` refuses with `EngineMismatch`
    /// 4. write the target (a failure here kills the *target*; the source is
    ///    untouched and still the profile's handle)
    /// 5. `replace` — one locked swap, so no window exists in which the profile
    ///    has no engine — and then [`Self::adopt_engine`], so the config and
    ///    the registry agree about which engine this profile is on
    /// 6. stop the displaced source
    ///
    /// Step 4-before-6 is the one an implementation naturally gets wrong: it
    /// reads more cleanly to shut the old engine first, and it is wrong for the
    /// same reason `reap_idle` stopping at `close` was wrong — the observable
    /// state afterwards is "reported success, browser gone".
    pub async fn switch_engine(
        &self,
        profile: &str,
        to: Engine,
        migrate: bool,
    ) -> Result<SwitchReport, BrowserError> {
        self.record_activity(profile);
        let source = self
            .engines()
            .get(profile)
            .await
            .ok_or_else(|| BrowserError::NoSession(profile.into()))?;
        let from = source.engine;
        if from == to {
            return Err(BrowserError::AlreadyOnEngine {
                profile: profile.into(),
                engine: to,
            });
        }

        let mut state = if migrate {
            migration::export_state(&source).await?
        } else {
            migration::MigrationState {
                warnings: vec![
                    "migrate=false: no cookies, tabs or localStorage were carried across".into(),
                ],
                ..migration::MigrationState::default()
            }
        };

        // A `user_data_dir` written by an operator names ONE engine's store, in
        // that engine's format. The target gets its own managed directory, and
        // says so — silently ignoring a directory the operator chose is the
        // no-op that reports success (判据 §11), one level down.
        //
        // It names the **configured** engine, never `from`. `from` is whatever
        // is running right now, which a previous override may already have
        // moved: after one switch this sentence rendered "the obscura store
        // belongs to chromium", which is verifiably false (判据 §1, §17). And
        // the condition is `to != configured` for the same reason — a switch
        // BACK to the configured engine does get the operator's directory, so
        // there is nothing to warn about.
        let cfg = self.get_config(profile);
        let configured = cfg
            .as_ref()
            .map(|c| c.resolved_engine(self.config.default_engine));
        if cfg.is_some_and(|c| c.user_data_dir.is_some()) && configured != Some(to) {
            let owner =
                configured.map_or_else(|| "the configured engine".to_string(), |e| e.to_string());
            state.warnings.push(format!(
                "this profile's configured user_data_dir belongs to {owner} and is not readable \
                 by {to}; {to} was started on its own managed directory instead"
            ));
        }

        let req = self.launch_request_for_engine(profile, to)?;
        let target = self.engines().launch_detached(to, &req).await?;

        let report = match migration::import_state(&target, &state).await {
            Ok(r) => r,
            Err(e) => {
                target.shutdown().await;
                return Err(e);
            }
        };

        let displaced = self.engines().replace(profile, Arc::clone(&target)).await;
        // Immediately after the swap and before anything can read the profile
        // again: between these two lines the registry says `to` while the
        // profile still resolves `from`, so a concurrent verb gets
        // `EngineMismatch` — an error, never the wrong browser.
        //
        // `None` when the switch lands back on the configured engine: the
        // override is over and the config is the authority again.
        self.set_adopted_engine(profile, (configured != Some(to)).then_some(to));
        // `shutdown` answers `true` only when the process actually died, and
        // `stop_launched` has two arms that answer `false` — refused, or the
        // signal could not be sent at all. Dropping that bool reads "I could
        // not kill it" as "it is gone" (判据 §8), and the observable state it
        // hides is the expensive one: the OLD engine still running on this
        // profile's store with the original cookies, the new one running too,
        // and a report that says the switch succeeded. The channel for "this
        // part did not go as intended" is three lines below, so it is said.
        let mut source_survived = false;
        if let Some(old) = displaced {
            source_survived = !old.shutdown().await;
        }

        // ─── the point of no return is ABOVE this line ───────────────────
        //
        // The registry points at the target and the source process is gone.
        // Nothing below may return `Err`: a failure reported over a switch that
        // has already happened sends the model back to `switch_engine`, which
        // now answers `AlreadyOnEngine` — it would be told the escape hatch
        // failed while standing on the other side of it (判据 §15). So the read
        // that follows degrades into a warning, never an error.
        //
        // The model's old refs all died with the old process, so the new tree
        // travels back in the return value rather than being something it must
        // remember to ask for — but "could not read it" is a different fact
        // from "the switch failed", and only one of them is true here.
        let mut warnings = report.warnings;
        if source_survived {
            warnings.push(format!(
                "the {from} process did not exit when it was asked to; it is still running on \
                 this profile's storage directory with the original cookies. Two browsers are \
                 open for this profile until it goes away — the next boot sweep will reap it"
            ));
        }
        let snapshot = match self.snapshot_after_switch(profile, report.active_tab).await {
            Ok(s) => Some(s),
            Err(e) => {
                warnings.push(format!(
                    "the switch completed, but the new page tree could not be read \
                     ({e}); run browser_snapshot to get one"
                ));
                None
            }
        };

        Ok(SwitchReport {
            from,
            to,
            cookies_moved: report.cookies_moved,
            tabs_reopened: report.tabs_reopened,
            local_storage_origins: report.local_storage_origins,
            warnings,
            snapshot,
        })
    }

    /// The post-switch page tree. Fallible, and its caller is the one that
    /// decides a failure here is not a failure of the switch.
    async fn snapshot_after_switch(
        &self,
        profile: &str,
        active_tab: Option<String>,
    ) -> Result<super::types::SnapshotOutput, BrowserError> {
        let backend = self.get_backend(profile)?;
        let tab_id = match active_tab {
            Some(id) => id,
            // No active tab is a fact, not an empty string. Ask the backend the
            // same question every other verb asks rather than handing
            // `snapshot` a `""` it will fail on obscurely.
            None => crate::builtin_tools::browser_tools::get_active_tab(backend.as_ref()).await?,
        };
        backend.snapshot(&tab_id).await
    }
    /// Test-only: build a manager around a registry the test owns.
    ///
    /// The ONE seam that lets a unit test put a browser into this manager
    /// without a real binary — same discipline as [`Self::insert_test_child`]:
    /// `#[cfg(test)]`, not `pub`, and there is exactly one. Construction-time
    /// injection rather than a mutable process map, so nothing can swap a
    /// launcher under a running engine.
    #[cfg(test)]
    pub(crate) fn with_engine_registry(
        config: BrowserSystemConfig,
        engines: Arc<EngineRegistry>,
    ) -> Self {
        let mut manager = Self::new(config);
        manager.engines = engines;
        manager
    }

    /// Sweep idle profiles past their `idle_timeout_secs`, both drivers.
    /// Returns the number of profiles reaped (best-effort; safe to call any time).
    ///
    /// - `ExistingSession` → tear down the Chrome MCP session. Liveness comes
    ///   from the driver's session map (the only place a session exists).
    /// - `Managed` → `playwright-cli close` (which now only disconnects the CLI
    ///   session) **and then** killing Aleph's own Chromium. Under the previous
    ///   arrangement `close` destroyed the browser the CLI had launched; under
    ///   `attach --cdp` it leaves it running, so stopping at `close` would have
    ///   reported a reaped profile over a browser that never went away.
    ///
    /// The close runs under [`LaunchPolicy::Refuse`]: a reaper that opened a
    /// browser in order to close it would be absurd, and the lazy launch makes
    /// that a real possibility rather than a hypothetical one.
    pub async fn reap_idle(&self) -> usize {
        let mut reaped = 0;
        for name in self.idle_existing_session_profiles() {
            self.chrome_mcp_driver.destroy_session(&name).await;
            reaped += 1;
        }
        for name in self.idle_managed_profiles() {
            match self
                .playwright_cli_driver
                .run(
                    &name,
                    LaunchPolicy::Refuse,
                    &["close"],
                    std::time::Duration::from_secs(self.config.playwright_cli.action_timeout_secs),
                )
                .await
            {
                // Already gone is the same outcome as just-closed.
                Ok(_) | Err(BrowserError::NoSession(_)) => {}
                // Best-effort, and deliberately NOT a gate on the kill below.
                // The CLI's session and the browser are two different things
                // now; an error here is the CLI's opinion of its own session
                // and says nothing about the process Aleph owns. Skipping the
                // kill on it would make an unusable CLI — the state most
                // likely to have leaked a browser in the first place — the one
                // state in which Aleph refuses to reclaim it (判据 §8: a
                // fail-closed answer must not be spent as a value).
                Err(e) => {
                    tracing::warn!(profile = %name, error = %e, "reap_idle: could not close the managed cli session; stopping its browser anyway");
                }
            }
            // `close` under `attach --cdp` is a DISCONNECT: the browser, its
            // pages and their state all survive it (measured). So this is the
            // half that actually reclaims anything, and the only half the
            // count may be earned by — reporting a reaped profile over a
            // browser that never went away is the "success reported for a
            // no-op" shape (判据 §11).
            if self.playwright_cli_driver.shutdown_chromium(&name) {
                reaped += 1;
            } else {
                tracing::warn!(profile = %name, "reap_idle: no chromium to stop for an idle managed profile");
            }
            self.tab_registry.clear_profile(&name);
        }
        reaped
    }

    /// `Managed` profiles idle past their timeout that Aleph has a browser
    /// record for — alive **or** dead.
    ///
    /// Exact, not approximate: the browser is Aleph's own child process, so
    /// "does one exist" is `ChromiumChild::alive`. The `close` below still
    /// tolerates `NoSession` because the CLI's session and the browser are now
    /// two different things — the browser can be alive with no session attached.
    ///
    /// The dead half is not a courtesy. A Chromium that exited on its own
    /// leaves three things behind that only this sweep clears while the daemon
    /// runs — the child record in the driver's map, the sidecar file naming its
    /// pid, and the profile's tab entries — so filtering on `alive` alone would
    /// make "the browser died" mean "there is nothing here to reclaim".
    ///
    /// The filter below reads as `chromium_alive(name) || chromium_died(name)`,
    /// and that is NOT because it behaves differently from a bare "does the
    /// driver have a record for this profile at all" check — algebraically it
    /// does not: [`PlaywrightCliDriver::chromium_died`] is defined as
    /// `key_present && !alive`, so the disjunction reduces to exactly
    /// `key_present`, in every state, not merely today's. It is written this
    /// way because `PlaywrightCliDriver` never exposes a bare "has a record"
    /// accessor — `chromium_alive` / `chromium_died` are the only two names it
    /// hands out for this decision, and they are the vocabulary the rest of
    /// this module already reasons in.
    ///
    /// Scoped to browsers **this** process launched, deliberately: a Chromium
    /// left by a crashed daemon has no idle clock here to be past, and is the
    /// boot sweep's to reclaim, not this sweep's.
    fn idle_managed_profiles(&self) -> Vec<String> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        profiles
            .iter()
            .filter(|(name, p)| {
                p.config.driver == BrowserDriver::Managed
                    && is_idle(p.last_activity, now, p.config.idle_timeout_secs)
                    && (self.playwright_cli_driver.chromium_alive(name)
                        || self.playwright_cli_driver.chromium_died(name))
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Whether the profile currently has a live browser session, derived from
    /// the real session-tracking surfaces rather than a state flag:
    /// - `ExistingSession` → a Chrome MCP session exists in the driver.
    /// - `Managed` → Aleph's Chromium for the profile is running. Exact since
    ///   the launch-chain flip; it used to be "the tab registry has tabs",
    ///   which its own doc called an approximation. Exact about **this
    ///   process's** browsers: one orphaned by a previous run reads as
    ///   inactive until [`Self::spawn_idle_reaper`]'s boot sweep disposes of
    ///   it, which is the sweep's job rather than this predicate's.
    pub fn session_active(&self, name: &str) -> bool {
        match self.get_driver(name) {
            Some(BrowserDriver::ExistingSession) => self.chrome_mcp_driver.has_session(name),
            Some(BrowserDriver::Managed) => self.playwright_cli_driver.chromium_alive(name),
            // The engine registry's own answer, on the same terms as the
            // `Managed` arm above: it asks the browser, not the bookkeeping.
            // A hardcoded `false` was correct only while nothing could put an
            // engine in the map; `engine_handle` can, so the constant would
            // have become a lie the moment the first CDP profile launched
            // (判据 §1 — this commit is what falsifies it).
            Some(BrowserDriver::Cdp) => self.engines.is_live(name),
            None => false,
        }
    }

    /// `ExistingSession` profiles that have a live Chrome MCP session AND have
    /// been idle longer than their configured timeout. Only these are reaped —
    /// a session that no longer exists needs no teardown, and recent activity
    /// protects live sessions.
    fn idle_existing_session_profiles(&self) -> Vec<String> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        // One clock read for the whole sweep, so every profile is judged
        // against the same instant.
        let now = std::time::Instant::now();
        profiles
            .iter()
            .filter(|(name, p)| {
                p.config.driver == BrowserDriver::ExistingSession
                    && is_idle(p.last_activity, now, p.config.idle_timeout_secs)
                    && self.chrome_mcp_driver.has_session(name)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Record activity on a specific tab so its idle timer resets.
    ///
    /// Tracked for the two drivers whose browsers Aleph launched and fully
    /// owns — `Managed` and `Cdp`. Never for `ExistingSession`: those are the
    /// user's own tabs and are neither tracked nor reaped (R5: don't disturb
    /// the user).
    ///
    /// ⚠️ **Half of this was fixed at the default flip, and the reason the
    /// other half is not is narrower than this doc used to claim.**
    /// [`Self::reap_idle_tabs`] now covers `Managed | Cdp` — it needed no
    /// liveness predicate, only `list_tabs`, so the `chromium_alive` argument
    /// below never applied to it. That sentence stood while `Cdp` was opt-in
    /// and would have become a false one the day the default profile was a Cdp
    /// profile with dead `max_tabs_per_profile` and `tab_idle_timeout_secs`
    /// settings (判据 §1 — the comment was the lying half).
    ///
    /// What is still deferred is the SESSION-level sweep:
    /// [`Self::idle_managed_profiles`] filters on `driver == Managed`, so a
    /// `Cdp` profile is never selected. **An idle CDP engine is therefore not
    /// torn down until the daemon exits**, and after the flip that is every
    /// install's `default` profile: its `idle_timeout_secs` is a setting with
    /// a writer and no reader — the same shape [`Self::reap_idle_tabs`] was
    /// widened to fix, one level up.
    ///
    /// ⚠️ **The reason this doc used to give for the deferral was false, and
    /// it was cited.** It said the predicate could not answer for a `Cdp`
    /// profile and that "there is no per-profile engine shutdown for it to
    /// call anyway (`EngineRegistry` exposes `shutdown_all`, nothing
    /// narrower)". Both halves are contradicted by modules it names:
    ///
    /// * `EngineRegistry::remove` (`engine/registry.rs:251`) takes one
    ///   profile's handle out of the map — "the caller owns stopping it" —
    ///   and `EngineHandle::shutdown` (`engine/mod.rs:603`) is a `pub async`
    ///   per-handle stop. Together those ARE the per-profile shutdown the
    ///   sentence said did not exist.
    /// * The liveness half is answered forty lines above, in
    ///   [`Self::session_active`], whose `Cdp` arm is `engines.is_live(name)`.
    ///
    /// So the gap is real but the deferral is a scheduling decision, not an
    /// impossibility — carried as a task-level item rather than argued away
    /// here. A written-down reason that does not survive reading the modules
    /// it names is worse than no reason: it is load-bearing the moment
    /// somebody cites it (判据 §1). The QA's `reap` scenario refuses
    /// `ALEPH_QA_DRIVER=cdp` for the same underlying gap.
    ///
    /// It does **not** mean nothing ever removes an entry. [`Self::forget_tab`]
    /// is called when a tab is closed, for every driver — the cheap half, and
    /// the one the first version of this doc failed to name: it said the
    /// sweeper was missing and left a reader to assume removal-on-close still
    /// happened, when `forget`'s only caller was inside that same Managed-only
    /// sweeper. A tab the model explicitly closed kept its entry forever.
    pub fn touch_tab(&self, profile_name: &str, tab_id: &str) {
        if matches!(
            self.get_driver(profile_name),
            Some(BrowserDriver::Managed | BrowserDriver::Cdp)
        ) {
            self.tab_registry.touch(profile_name, tab_id);
        }
    }

    /// Sweep idle / over-cap tabs for every profile whose browser Aleph owns
    /// and that has tracked tabs.
    ///
    /// Reconciles the registry against each profile's live `list_tabs` output,
    /// then closes the selected victims (idle beyond `tab_idle_timeout_secs`, or
    /// LRU overflow beyond `max_tabs_per_profile`). The active (most-recently-
    /// used) tab is always protected. Best-effort: any backend error skips that
    /// profile. Returns the number of tabs closed.
    ///
    /// **`Managed | Cdp`, and the set is the same one [`Self::touch_tab`]
    /// writes.** It filtered on `Managed` alone until the default flip, which
    /// was affordable while `Cdp` was opt-in and became a silent regression the
    /// moment `Cdp` was what a fresh install got: `touch_tab` records a Cdp
    /// profile's tabs, nothing ever selected them, and `max_tabs_per_profile`
    /// and `tab_idle_timeout_secs` were dead settings on the default profile —
    /// a writer with no reader (判据 §7), arrived at by a membership list that
    /// only covered the world of the day it was written (判据 §5).
    ///
    /// Widening this needs no new liveness predicate, which is why it is here
    /// and [`Self::reap_idle`]'s session-level sweep is not: this function asks
    /// the backend (`list_tabs`, through `EngineLaunch::Refuse`, so a sweep can
    /// never LAUNCH a browser) and reads an error as "the browser is gone,
    /// stop re-probing". `idle_managed_profiles` instead judges through
    /// `playwright_cli_driver.chromium_alive`, which cannot answer for a Cdp
    /// profile at all — see [`Self::touch_tab`].
    pub async fn reap_idle_tabs(&self) -> usize {
        // Candidates: profiles whose browser Aleph launched and that were used.
        let candidates: Vec<String> = {
            let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
            profiles
                .iter()
                .filter(|(name, p)| {
                    matches!(p.config.driver, BrowserDriver::Managed | BrowserDriver::Cdp)
                        && self.tab_registry.has_tabs(name)
                })
                .map(|(name, _)| name.clone())
                .collect()
        };

        let mut closed = 0;
        for profile in candidates {
            let (max_tabs, idle_secs) = match self.get_config(&profile) {
                Some(c) => (c.max_tabs_per_profile, c.tab_idle_timeout_secs),
                None => continue,
            };
            let backend = match self.get_backend(&profile) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(profile = %profile, error = %e, "reap_idle_tabs: failed to get backend");
                    continue;
                }
            };
            let tabs = match backend.list_tabs().await {
                Ok(t) => t,
                Err(e) => {
                    // Browser gone — stop re-probing this profile every sweep.
                    tracing::warn!(profile = %profile, error = %e, "reap_idle_tabs: failed to list tabs");
                    self.tab_registry.clear_profile(&profile);
                    continue;
                }
            };
            let live_ids = tab_ids(&tabs);
            // The sweep's `list_tabs` is also the OLD drivers' identity
            // discovery point (the cdp backend records into the same registry
            // itself, from inside the verbs). A listing row knows no targetId,
            // so `None` is recorded for it — and because `record_identity`
            // merges rather than replaces, that `None` never erases a
            // targetId the cdp backend recorded.
            for line in &tabs {
                self.tab_registry
                    .record_identity(&profile, &line.id, None, Some(line.url.clone()));
            }
            let victims = self.tab_registry.select_victims(
                &profile,
                &live_ids,
                max_tabs,
                Duration::from_secs(idle_secs),
            );
            for victim in victims {
                if let Err(e) = backend.close_tab(&victim).await {
                    tracing::warn!(profile = %profile, tab = %victim, error = %e, "reap_idle_tabs: failed to close tab");
                } else {
                    self.tab_registry.forget(&profile, &victim);
                    closed += 1;
                }
            }
        }
        closed
    }

    /// Get the driver mode for a named profile.
    pub fn get_driver(&self, name: &str) -> Option<BrowserDriver> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        let result = profiles.get(name).map(|p| p.config.driver);
        tracing::debug!(
            profile = name,
            driver = ?result,
            available_profiles = ?profiles.keys().collect::<Vec<_>>(),
            "ProfileManager::get_driver"
        );
        result
    }

    /// Every entry with derived session liveness (see [`Self::session_active`]),
    /// INCLUDING principal-materialized ones. Diagnostic / construction-time
    /// use (`tools::probes::browser`); the tool face lists through
    /// [`Self::list_profiles_for`].
    pub fn list_profiles(&self) -> Vec<(String, bool)> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles
            .keys()
            .map(|name| (name.clone(), self.session_active(name)))
            .collect()
    }

    /// Get the configuration of a named profile.
    pub fn get_config(&self, name: &str) -> Option<ProfileConfig> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles.get(name).map(|p| p.config.clone())
    }

    /// The key `name` resolves to for `principal` — THE browser-profile
    /// boundary for a caller-supplied name (r11 N5).
    ///
    /// - A composed name (`default__u-alice`) is refused as not found: it is the
    ///   OUTPUT of this function, never a value a caller was handed to type
    ///   back in — the `memory_scope::read_partitions` rule. Same text as a
    ///   genuinely missing profile, so the refusal is no oracle.
    /// - The owner and an actor-less caller get `name` itself.
    /// - Anyone else gets [`principal_profile_key`], materialized on first use
    ///   from the CONFIGURED `name` with three changes: no `user_data_dir` (a
    ///   configured directory is the operator's browser store, logins
    ///   included), no `--user-data-dir` / `--profile-directory` in the
    ///   inherited `extra_args` (the same store named another way — on the cdp
    ///   engines `extra_args` go first, and Chrome takes the first occurrence),
    ///   and a refusal for `ExistingSession` (that driver attaches to the
    ///   machine owner's own Chrome — there is no per-principal copy).
    ///
    /// Materialized entries live in the same map as configured ones, so the
    /// idle reapers, the tab registry, the engine override and both data-dir
    /// derivations key them exactly like any other profile.
    ///
    /// [`principal_profile_key`]: super::profile::principal_profile_key
    pub fn principal_profile(
        &self,
        name: &str,
        principal: Option<&str>,
    ) -> Result<String, BrowserError> {
        let not_found = || BrowserError::ProfileNotFound(name.to_string());
        if crate::memory::project_scope::is_composed_id(name) {
            return Err(not_found());
        }
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        let base = profiles
            .get(name)
            .filter(|p| p.materialized_for.is_none())
            .map(|p| p.config.clone())
            .ok_or_else(not_found)?;
        let key = super::profile::principal_profile_key(name, principal);
        if key == name {
            return Ok(key);
        }
        if base.driver == BrowserDriver::ExistingSession {
            return Err(BrowserError::ActionFailed(format!(
                "profile '{name}' attaches to the machine owner's own Chrome, which is not \
                 available to other users; use a managed profile such as 'default'"
            )));
        }
        let extra_args = args_without_store_flags(&base.extra_args);
        profiles
            .entry(key.clone())
            .or_insert_with(|| ManagedProfile {
                config: ProfileConfig {
                    user_data_dir: None,
                    extra_args,
                    ..base
                },
                adopted_engine: None,
                last_activity: std::time::Instant::now(),
                materialized_for: principal.map(str::to_string),
            });
        Ok(key)
    }

    /// The profile list as `principal` sees it: every CONFIGURED profile once,
    /// under the name the caller types back in, with the liveness of the
    /// caller's own copy. Never another principal's copy, never a composed key.
    /// `ExistingSession` profiles are omitted for anyone but the owner / an
    /// actor-less caller, because [`Self::principal_profile`] refuses them.
    pub fn list_profiles_for(&self, principal: Option<&str>) -> Vec<(String, bool)> {
        let configured: Vec<(String, BrowserDriver)> = {
            let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
            profiles
                .iter()
                .filter(|(_, p)| p.materialized_for.is_none())
                .map(|(name, p)| (name.clone(), p.config.driver))
                .collect()
        };
        configured
            .into_iter()
            .filter_map(|(name, driver)| {
                let key = super::profile::principal_profile_key(&name, principal);
                if key != name && driver == BrowserDriver::ExistingSession {
                    return None;
                }
                let active = self.session_active(&key);
                Some((name, active))
            })
            .collect()
    }

    /// The live CDP endpoint of a `Managed` profile's browser, if it has one.
    ///
    /// The accessor spec §3.2 asks for. `ExistingSession` answers `None` by
    /// construction: that browser is the user's own, Aleph never launched it,
    /// and the live view is deliberately Managed-only — a Chrome the user
    /// started is already on their screen.
    ///
    /// ⚠️ `None` is "**Aleph** has no browser for this profile", which is the
    /// same sentence as "no browser is running" only after
    /// [`Self::spawn_idle_reaper`]'s boot sweep: a Chromium orphaned by a
    /// previous process lives in the sidecar registry, not in this driver's
    /// map (判据 §8 — an absent record is not an absent process).
    // The one allow this task adds, and it replaces two: `PlaywrightCliDriver`'s
    // `endpoint` and `shutdown_chromium` each carried one naming Task 6, and
    // both are consumed here now. This one is the head of that chain — nothing
    // in this crate reads a live endpoint until Plan 2's live view does, and
    // `--lib` (which does not compile `#[cfg(test)]`) sees only that. Delete it
    // with the first non-test caller; if Plan 2 does not land, this is a CUT,
    // not a permanent allow.
    #[allow(dead_code)]
    pub(crate) fn live_endpoint(&self, profile: &str) -> Option<super::CdpEndpoint> {
        match self.get_driver(profile) {
            Some(BrowserDriver::Managed) => self.playwright_cli_driver.endpoint(profile),
            Some(BrowserDriver::ExistingSession | BrowserDriver::Cdp) | None => None,
        }
    }

    /// Kill every browser this manager launched. Returns how many were stopped.
    ///
    /// spec §3.6「退出时杀」. `std::process::Child` does not kill on drop, and
    /// under `attach --cdp` the CLI was never the browser's parent — so without
    /// this every restart leaves a Chromium running until the next boot sweep.
    ///
    /// **Must finish inside `SHUTDOWN_FAILSAFE` (`start/helpers.rs`)** — both
    /// call sites, not just one. The wedged-shutdown watchdog IS that failsafe
    /// and the `std::process::exit(0)` after it waits for nobody; the orderly
    /// path races the same watchdog, because its `JoinHandle` is dropped and
    /// nothing cancels it. That is why `budget` is a parameter and why the
    /// orderly caller derives its own from the failsafe rather than from a
    /// constant in this crate: for a while it threaded 35.5 s through here,
    /// against a doc that said 5 s and was never re-read because this task
    /// never edited it (判据 §1's fourth form — a doc that stayed true until
    /// someone changed its subject). What that buys
    /// this function is a rule, not a budget: SIGKILL plus a bounded reap per
    /// child, and **never a graceful handshake**. No SIGTERM-then-wait, no CDP
    /// `Browser.close`, no round trip to a browser that may be the reason the
    /// shutdown wedged in the first place. `Child::kill()` is immediate and
    /// `Child::wait()` after a successful kill returns as soon as the kernel
    /// reaps — microseconds, not a negotiation (see `ChromiumChild::shutdown`,
    /// which skips the wait entirely when the kill did not succeed, precisely
    /// so a child we could not signal cannot park this loop).
    ///
    /// Covers BOTH families of browser this manager owns: the playwright-owned
    /// Chromiums and the CDP engines in [`Self::engines`]. One function, one
    /// count — a second global for the engines would make each number a
    /// half-truth about "did we stop the browsers".
    pub async fn shutdown_browsers(&self, budget: Duration) -> usize {
        // The playwright-owned Chromiums first, unchanged and synchronous:
        // this half has always had to fit inside SHUTDOWN_FAILSAFE and still
        // does, and it costs the engine half nothing to run first. Note it is
        // NOT covered by `budget`: the caller's budget bounds the engine half
        // below, and this synchronous call runs before the clock starts.
        let mut stopped = self.playwright_cli_driver.shutdown_all_chromium();
        // Still hard-bounded, but by the CALLER's budget and inside
        // `shutdown_all`. A `timeout` wrapped around the call
        // here drops the future and takes the count with it, so a run that
        // stopped two engines and then hit the wall added 0 to this total —
        // a number that under-reports work done is the same family as a no-op
        // that reports success (判据 §11). The registry owns both the budget
        // and the count because only it can still see what died.
        //
        // The bound covers `shutdown_all`'s **lock acquisition** as well as the
        // stops. It briefly did not — the deadline started after the lock, so a
        // launch in flight could hold this call for ~35 s against a stated 1 s.
        // Worth knowing here rather than only at the definition, because this
        // is the call site both daemon exit paths reach: the wedged-exit
        // failsafe AND the orderly teardown, which race the same watchdog and
        // pass different budgets for exactly that reason.
        stopped += self.engines.shutdown_all(budget).await;
        stopped
    }

    /// Validate a URL against the SSRF policy.
    pub async fn check_url(&self, url: &str) -> Result<(), PolicyViolation> {
        self.ssrf_guard.load().check_url(url).await
    }

    /// Validate an agent-initiated navigation target: SSRF policy plus
    /// secret-exfiltration scanning. Use this for `goto`/`open`.
    pub async fn check_navigation(&self, url: &str) -> Result<(), PolicyViolation> {
        self.ssrf_guard.load().check_navigation(url).await
    }

    /// Scan `text` about to be typed into a page form for an embedded
    /// credential (the form-input leg of the secret-egress boundary; mirrors
    /// [`Self::redact_content`]'s delegation pattern). Returns the matched rule
    /// name when the input must be refused, `None` when clean or the flag is off.
    pub fn check_input_secret(&self, text: &str) -> Option<String> {
        self.ssrf_guard.load().check_input(text)
    }

    /// Redact embedded credentials from page-derived `text` before it is
    /// returned to the LLM (the OUT half of the secret-egress boundary). Used by
    /// the content-read tools (snapshot / console / network / evaluate) via the
    /// shared `redact_and_wrap` egress chokepoint. Zero-copy when redaction is
    /// disabled or the text carries no secret.
    pub fn redact_content<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        // The returned `Cow` borrows from `text`, never from the guard, so the
        // loaded policy generation can be dropped at the end of the statement
        // and the zero-copy path survives the swap to `ArcSwap`.
        self.ssrf_guard.load().redact_content(text)
    }

    /// Record activity on a profile to reset its idle timer.
    pub fn record_activity(&self, profile_name: &str) {
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        if let Some(profile) = profiles.get_mut(profile_name) {
            profile.last_activity = std::time::Instant::now();
        }
    }

    /// Stop tracking a tab that is gone.
    ///
    /// The twin of [`Self::touch_tab`] and deliberately NOT driver-filtered:
    /// `touch_tab` decides what gets tracked, and anything tracked must be
    /// removable by the same event that ends it. Filtering here would leak
    /// exactly the entries the other filter admitted. A profile that never
    /// tracked the tab is a no-op, which is the honest answer to "forget
    /// something I was not remembering".
    pub fn forget_tab(&self, profile_name: &str, tab_id: &str) {
        self.tab_registry.forget(profile_name, tab_id);
    }

    /// Test-only: whether any tabs are tracked for a profile.
    #[cfg(test)]
    pub(crate) fn has_tracked_tabs(&self, profile: &str) -> bool {
        self.tab_registry.has_tabs(profile)
    }

    /// Test-only: hand the managed driver a browser it did not launch, so the
    /// reaper and the shutdown path can be exercised against a REAL pid without
    /// a real Chromium.
    ///
    /// One line of forwarding onto [`PlaywrightCliDriver::insert_test_child`]
    /// and [`ChromiumChild::from_parts`] — both of which say "do not add a
    /// second one", and this is the one. Not `pub`, and `#[cfg(test)]`: a door
    /// that lets something outside put a browser into the driver is a door that
    /// goes around the launch chain.
    #[cfg(test)]
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn insert_test_child(&self, profile: &str, child: std::process::Child) {
        let endpoint = super::CdpEndpoint {
            http_url: "http://127.0.0.1:1".into(),
            ws_url: "ws://127.0.0.1:1/devtools/browser/test".into(),
            pid: child.id(),
        };
        self.playwright_cli_driver.insert_test_child(
            profile,
            super::engine::chromium::ChromiumChild::from_parts(
                child,
                endpoint,
                std::path::PathBuf::from("/tmp/aleph-test-udd"),
                profile,
            ),
        );
    }
}

/// Whether `last_activity` is older than `timeout_secs` as of `now`. Pure
/// helper so the reaper's timeout filter is unit-testable without a live
/// session.
///
/// `now` is taken explicitly rather than read from the clock inside: it makes
/// the helper a total function of its inputs, and lets a test express "long
/// ago" by moving `now` *forward* instead of subtracting from `Instant::now()`.
/// That subtraction panics wherever the monotonic clock's origin is more recent
/// than the offset — routine on a freshly booted CI VM, where Windows counts
/// `Instant` from system boot.
fn is_idle(last_activity: std::time::Instant, now: std::time::Instant, timeout_secs: u64) -> bool {
    now.saturating_duration_since(last_activity).as_secs() > timeout_secs
}

/// `args` minus every flag that names a browser store: `user-data-dir` and
/// `profile-directory`, behind either prefix Chromium's switch parser accepts
/// (`--` and `-`; re-review N7), in the `=value` spelling and in the
/// separate-argument spelling (whose value is dropped with it, unless the
/// next argument is itself a flag) — i.e. `^-{1,2}(user-data-dir|
/// profile-directory)(=|$)`. A principal's copy of a profile must not
/// inherit the operator's store through `extra_args` (final review M10).
/// Not stripped: the `/flag` prefix Chromium also accepts on Windows.
fn args_without_store_flags(args: &[String]) -> Vec<String> {
    let mut kept = Vec::with_capacity(args.len());
    let mut drop_value = false;
    for arg in args {
        if std::mem::take(&mut drop_value) && !arg.starts_with('-') {
            continue;
        }
        match store_flag(arg) {
            Some(StoreFlagValue::Separate) => drop_value = true,
            Some(StoreFlagValue::Inline) => {}
            None => kept.push(arg.clone()),
        }
    }
    kept
}

/// Where a store flag's value is, for [`args_without_store_flags`].
enum StoreFlagValue {
    /// `-{1,2}flag=value`.
    Inline,
    /// `-{1,2}flag`, the value in the next argument.
    Separate,
}

/// Whether `arg` names a browser store, and how its value is spelled.
fn store_flag(arg: &str) -> Option<StoreFlagValue> {
    const STORE_FLAGS: [&str; 2] = ["user-data-dir", "profile-directory"];
    let name = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-'))?;
    STORE_FLAGS.iter().find_map(|flag| {
        let rest = name.strip_prefix(flag)?;
        if rest.is_empty() {
            Some(StoreFlagValue::Separate)
        } else if rest.starts_with('=') {
            Some(StoreFlagValue::Inline)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::testkit::{engine_peer, FakeEngineProcess};
    use crate::utils::paths::AlephHomeEnvGuard;
    use aleph_cdp::testkit::FakeCdpServer;

    /// Final review M10: a principal's copy must not reach the operator's
    /// browser store through `extra_args` — both spellings of both flags,
    /// behind either switch prefix (`--`, and the single `-` Chromium also
    /// accepts — re-review N7), are dropped (a separate value with them),
    /// everything else is kept, and the configured profile itself is
    /// untouched.
    #[test]
    fn a_principal_copy_drops_the_operators_store_flags() {
        let mut config = BrowserSystemConfig::default();
        let operator_args: Vec<String> = [
            "--user-data-dir=/operator/chrome",
            "--profile-directory=Default",
            "--user-data-dir",
            "/operator/other",
            "--profile-directory",
            "Profile 1",
            "-user-data-dir=/x",
            "-profile-directory",
            "Profile 2",
            "--lang=en",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        config.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                engine: Some(Engine::Chromium),
                extra_args: operator_args.clone(),
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        let key = manager
            .principal_profile("default", Some("u-alice"))
            .expect("a member may use a managed profile");
        let copy = manager.get_config(&key).expect("the copy is an entry");
        assert_eq!(copy.extra_args, vec!["--lang=en".to_string()]);
        assert_eq!(
            manager.get_config("default").unwrap().extra_args,
            operator_args
        );
    }

    /// A manager whose `default` profile is `driver = cdp, engine = chromium`
    /// and whose registry's only launcher is a fake.
    fn manager_with_fake_engine(
        server: &FakeCdpServer,
        sidecar_dir: &std::path::Path,
    ) -> (ProfileManager, Arc<FakeEngineProcess>) {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                engine: Some(Engine::Chromium),
                ..Default::default()
            },
        );
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            server,
            sidecar_dir,
        ));
        let mut processes: HashMap<Engine, Arc<dyn EngineProcess>> = HashMap::new();
        processes.insert(Engine::Chromium, proc.clone());
        let registry = Arc::new(EngineRegistry::new(
            processes,
            Duration::from_millis(500),
            Duration::from_secs(2),
        ));
        (ProfileManager::with_engine_registry(config, registry), proc)
    }

    /// The sync half: everything a launch needs, derived from the profile and
    /// the LIVE policy, without touching the network or the disk.
    ///
    /// The private-network bit is read from the running guard, not the boot
    /// snapshot: `apply_policy` swaps it at runtime and the argv is written
    /// once, so the stale copy would hand obscura a permission the running
    /// policy has already withdrawn.
    #[test]
    fn launch_request_for_derives_the_request_from_the_profile_and_the_live_policy() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "work".into(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                engine: Some(Engine::Chromium),
                headless: Some(false),
                proxy: Some("socks5://127.0.0.1:1080".into()),
                extra_args: vec!["--disable-gpu".into()],
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);

        let (engine, req) = manager
            .launch_request_for("work")
            .expect("a configured profile has a launch");
        assert_eq!(engine, Engine::Chromium);
        assert_eq!(req.profile, "work");
        assert_eq!(req.session_key, "work");
        assert!(!req.headless, "the profile's explicit headless=false");
        assert_eq!(req.proxy.as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(req.extra_args, vec!["--disable-gpu".to_string()]);
        assert!(
            req.data_dir.ends_with("work"),
            "the data dir is keyed by the sanitized profile name: {}",
            req.data_dir.display()
        );
        assert!(
            req.data_dir.starts_with(home.path()),
            "the data dir must sit under THIS test's ALEPH_HOME, not the \
             developer's: {}",
            req.data_dir.display()
        );
        assert!(
            !req.allow_private_network,
            "the default policy blocks private ranges, so the engine must not \
             be handed permission for them"
        );

        // Hot-apply an open policy: the NEXT request must carry the new answer.
        manager.apply_policy(SsrfConfig {
            block_private: false,
            ..SsrfConfig::default()
        });
        let (_, req) = manager.launch_request_for("work").expect("still resolves");
        assert!(
            req.allow_private_network,
            "the request must read the LIVE guard, not the boot snapshot"
        );

        assert!(matches!(
            manager.launch_request_for("no-such-profile"),
            Err(BrowserError::ProfileNotFound(_))
        ));
    }

    /// `engine_handle` is a delegate: what it returns is what the registry
    /// stored, under the profile's own key.
    #[tokio::test]
    async fn engine_handle_delegates_to_the_registry() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let server = FakeCdpServer::start(engine_peer).await;
        let (manager, proc) = manager_with_fake_engine(&server, home.path());

        let handle = manager
            .engine_handle("default", EngineLaunch::Allow)
            .await
            .expect("launches through the registry");
        let from_registry = manager
            .engines()
            .get("default")
            .await
            .expect("the registry holds it");
        assert!(Arc::ptr_eq(&handle, &from_registry));
        assert_eq!(proc.launches().len(), 1);

        // The explicit-engine face resolves to the same handle rather than a
        // second browser.
        let same = manager
            .engine_handle_for("default", Engine::Chromium, EngineLaunch::Allow)
            .await
            .expect("same engine, same handle");
        assert!(Arc::ptr_eq(&handle, &same));
        assert_eq!(proc.launches().len(), 1);

        // The Cdp arm of `session_active` asks the registry, not a constant:
        // a live engine reads as a live session, and an unknown profile does
        // not.
        assert!(
            manager.session_active("default"),
            "a launched CDP engine must read as an active session"
        );
        drop(handle);
        drop(from_registry);
        drop(same);

        manager
            .shutdown_browsers(engine::ENGINE_SHUTDOWN_BUDGET)
            .await;
        assert!(
            !manager.session_active("default"),
            "a stopped engine must stop reading as an active session"
        );
        server.shutdown().await;
    }

    /// `BrowserSystemConfig` derives no `Default` by hand for nothing: a `u64`
    /// timeout field defaults to **0** unless `Default` is hand-written, and a
    /// zero command timeout makes every CDP call fail instantly, everywhere,
    /// silently — every test that builds a manager from `::default()` would be
    /// exercising that. Pin the product default at the face the engine path
    /// actually reads.
    #[test]
    fn cdp_command_timeout_is_the_product_default_and_below_obscuras_guillotine() {
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        let t = manager.cdp_command_timeout();
        assert_eq!(t, Duration::from_secs(30), "spec §6.3 default");
        assert!(
            t < Duration::from_secs(super::super::profile::CDP_COMMAND_TIMEOUT_CEILING_SECS),
            "must stay under obscura's own guillotine, or the page dies before \
             our wait does"
        );
    }

    /// `shutdown_browsers` must reclaim the CDP engines too, by effect.
    #[tokio::test]
    async fn shutdown_browsers_stops_engines_and_removes_their_sidecars() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let server = FakeCdpServer::start(engine_peer).await;
        let (manager, proc) = manager_with_fake_engine(&server, home.path());

        let handle = manager
            .engine_handle("default", EngineLaunch::Allow)
            .await
            .expect("launch");
        let sidecar = handle.launched.sidecar_path.clone();
        let pid = handle.launched.pid;
        assert!(sidecar.exists(), "precondition: the launch wrote a record");
        drop(handle);

        assert_eq!(
            manager
                .shutdown_browsers(engine::ENGINE_SHUTDOWN_BUDGET)
                .await,
            1,
            "one engine was stopped and the count must say so"
        );
        assert_eq!(proc.kills(), vec![pid], "the engine's pid was never killed");
        assert!(!sidecar.exists(), "the sidecar outlived the engine");
        assert_eq!(
            manager
                .shutdown_browsers(engine::ENGINE_SHUTDOWN_BUDGET)
                .await,
            0,
            "a second stop must find nothing, not re-report the first"
        );
        server.shutdown().await;
    }

    /// The single-derivation pin for the shutdown story: both daemon exit
    /// paths already reach `shutdown_browsers_global` (pinned in
    /// `start/helpers.rs`), so covering the engines is a property of THIS
    /// function, not of the two call sites. A source pin because the
    /// playwright half needs a real Chromium to observe and the two halves
    /// must be asserted together.
    #[test]
    fn shutdown_browsers_reaches_both_browser_families() {
        let src = include_str!("manager.rs").replace('\r', "");
        // `code_text` on top of the `#[cfg(test)]` bound, matching the two
        // sibling pins (`engine_handle_is_built_in_exactly_one_production_place`
        // and `both_daemon_exit_paths_reap_background_jobs_and_browsers`, whose
        // own comment documents this hazard). Without it a COMMENT spelling the
        // call satisfies the assertion, so deleting the statement and leaving a
        // `// engines.shutdown_all()` behind would stay green — a guard a
        // comment can satisfy is not a guard (判据 §2).
        // Two separate measurements, because they answer two questions and
        // one of them stopped being able to fail. The floor below is about the
        // `#[cfg(test)]` BOUND, so it has to be taken before `code_text`:
        // `code_text` strips comments and therefore shrinks the text on its
        // own, so `code_text(&prefix).len() < src.len()` holds even when the
        // prefix matched nothing at all — measured by substituting
        // `code_text(&src)`, which is exactly what that failure produces, and
        // the assertion stayed green. A predicate that can no longer detect the
        // condition its own message names is 判据 §2's second face, and M5's
        // fix is what created it here. `registry.rs`'s sibling keeps its own
        // independent floor for the same reason.
        let prefix = crate::utils::source_scan::production_prefix(&src);
        assert!(
            prefix.len() < src.len(),
            "the #[cfg(test)] bound matched nothing — this test would then be \
             reading its own source"
        );
        let production = crate::utils::source_scan::code_text(&prefix);
        assert!(
            production.contains("shutdown_all_chromium("),
            "shutdown_browsers must still stop the playwright-owned browsers"
        );
        assert!(
            production.contains("engines.shutdown_all("),
            "shutdown_browsers must stop the CDP engines too — otherwise every \
             restart leaves one running and the count it returns is a half-truth"
        );
    }

    #[test]
    fn test_manager_registers_profiles_from_config() {
        let mut config = BrowserSystemConfig::default();
        config
            .profiles
            .insert("default".into(), ProfileConfig::default());
        config.profiles.insert(
            "work".into(),
            ProfileConfig {
                headless: Some(true),
                ..Default::default()
            },
        );

        let manager = ProfileManager::new(config);
        let profiles = manager.list_profiles();
        // 2 explicit + auto-injected "user" = 3
        assert_eq!(profiles.len(), 3);
        assert!(profiles.iter().any(|p| p.0 == "default"));
        assert!(profiles.iter().any(|p| p.0 == "work"));
        assert!(profiles.iter().any(|p| p.0 == "user"));
    }

    #[test]
    fn test_manager_default_profile_if_none_configured() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let profiles = manager.list_profiles();
        // "default" + auto-injected "user" = 2
        assert_eq!(profiles.len(), 2);
        assert!(profiles.iter().any(|p| p.0 == "default"));
        assert!(profiles.iter().any(|p| p.0 == "user"));
    }

    #[test]
    fn test_get_profile_state_removed_in_favor_of_session_active() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        // No browser has been used → both profiles report inactive.
        assert!(!manager.session_active("default"));
        assert!(!manager.session_active("user"));
        assert!(!manager.session_active("nonexistent"));

        // Not an approximation any more: Aleph owns the browser process, so
        // `session_active` asks it. A tracked tab says a tab was USED, which is
        // a different fact and no longer stands in for a live browser.
        manager.touch_tab("default", "1");
        assert!(
            !manager.session_active("default"),
            "a tracked tab must not imply a browser that was never launched"
        );
    }

    #[test]
    fn test_is_idle_timeout_filter() {
        // Age is expressed by advancing `now`, never by subtracting from
        // `Instant::now()` — see `is_idle`'s note on the clock origin.
        let touched = std::time::Instant::now();
        assert!(!is_idle(touched, touched, 1800));
        assert!(is_idle(
            touched,
            touched + std::time::Duration::from_secs(1801),
            1800
        ));
        // Boundary: elapsed must strictly exceed the timeout.
        assert!(!is_idle(
            touched,
            touched + std::time::Duration::from_secs(1800),
            1800
        ));
        assert!(!is_idle(touched, touched, 0));
    }

    #[test]
    fn test_auto_injects_user_profile() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let profiles = manager.list_profiles();
        assert!(profiles.iter().any(|p| p.0 == "default"));
        assert!(profiles.iter().any(|p| p.0 == "user"));
    }

    #[test]
    fn test_user_profile_is_existing_session() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let user_config = manager.get_config("user").unwrap();
        assert_eq!(user_config.driver, BrowserDriver::ExistingSession);
        assert_eq!(user_config.browser, BrowserType::Chrome);
    }

    #[test]
    fn test_explicit_user_profile_not_overridden() {
        // An explicitly-configured "user" profile must survive the
        // auto-injection pass verbatim. This used `color` as its distinguishing
        // marker until that field was cut in 3757bb4f8; `idle_timeout_secs`
        // carries the same proof (default is 1800, so 999 can only come from
        // the explicit config).
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "user".into(),
            ProfileConfig {
                browser: BrowserType::Chrome,
                driver: BrowserDriver::ExistingSession,
                idle_timeout_secs: 999,
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        let user_config = manager.get_config("user").unwrap();
        assert_eq!(user_config.idle_timeout_secs, 999);
        assert_eq!(user_config.driver, BrowserDriver::ExistingSession);
    }

    /// **The flip, from both injection sites.**
    ///
    /// `ProfileManager::new` creates the `default` profile in TWO places — the
    /// empty-config branch and the "the operator named other profiles but not
    /// this one" branch — and for most of this file's life one of them named
    /// `Managed` explicitly while the other took the type's default. Changing
    /// only one leaves the other on the old driver, and the difference is
    /// invisible until somebody runs with an empty config (判据 §6: count the
    /// writers, and the count is always short by one).
    ///
    /// Driving both branches in one test rather than trusting that they agree
    /// is the whole point; a single-branch assertion would stay green with the
    /// other site reverted.
    #[test]
    fn the_default_profile_is_obscura_over_cdp_from_both_injection_sites() {
        let default_engine = BrowserSystemConfig::default().default_engine;

        // Empty config → the first branch.
        let empty = ProfileManager::new(BrowserSystemConfig::default());
        let d = empty.get_config("default").expect("default profile");
        assert_eq!(d.driver, BrowserDriver::Cdp, "empty-config branch");
        assert_eq!(d.resolved_engine(default_engine), Engine::Obscura);

        // A config naming SOME OTHER profile → the second branch.
        let mut cfg = BrowserSystemConfig::default();
        cfg.profiles
            .insert("other".into(), ProfileConfig::default());
        let seeded = ProfileManager::new(cfg);
        let d = seeded.get_config("default").expect("default profile");
        assert_eq!(d.driver, BrowserDriver::Cdp, "auto-inject branch");
        assert_eq!(d.resolved_engine(default_engine), Engine::Obscura);

        // The `user` profile is untouched: it attaches to the operator's own
        // Chrome and has nothing to do with the engine choice. Without this the
        // test above is satisfied by "every profile is Cdp now".
        let u = seeded.get_config("user").expect("user profile");
        assert_eq!(u.driver, BrowserDriver::ExistingSession);
        assert_eq!(u.resolved_engine(default_engine), Engine::Chromium);
    }

    /// An operator who wrote `driver` down keeps it, through the same
    /// constructor that injects the new default for everyone else.
    #[test]
    fn an_explicitly_configured_default_profile_is_not_flipped() {
        let mut cfg = BrowserSystemConfig::default();
        cfg.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Managed,
                ..ProfileConfig::default()
            },
        );
        let manager = ProfileManager::new(cfg);
        let d = manager.get_config("default").expect("default profile");
        assert_eq!(
            d.driver,
            BrowserDriver::Managed,
            "the flip must change what happens when nothing was said, never \
             what an operator wrote down"
        );
        assert_eq!(
            d.resolved_engine(Engine::Obscura),
            Engine::Chromium,
            "a legacy driver pins the engine; the global default must not drag \
             it onto obscura"
        );
    }

    #[test]
    fn test_get_driver() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        assert_eq!(manager.get_driver("default"), Some(BrowserDriver::Cdp));
        assert_eq!(
            manager.get_driver("user"),
            Some(BrowserDriver::ExistingSession)
        );
        assert_eq!(manager.get_driver("nonexistent"), None);
    }

    #[test]
    fn test_get_backend_routes_managed_to_playwright_cli() {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Managed,
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        let backend = manager.get_backend("default");
        assert!(backend.is_ok());
    }

    /// The third driver has to route somewhere, and to the RIGHT somewhere.
    /// Before this task it hit an arm that refused by name, which reads to an
    /// operator exactly like a config typo.
    ///
    /// The concrete type is the claim. `Arc<dyn BrowserBackend>` is satisfied by
    /// any of the four, and the arm this replaces produced "some answer" too —
    /// so an assertion on `is_ok()` alone would have been green against a
    /// `Cdp` profile silently driven by the managed backend, which is a
    /// DIFFERENT engine reporting success (判据 §11).
    #[test]
    fn test_get_backend_routes_cdp_profile_to_cdp_backend() {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "cdp".into(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                engine: Some(Engine::Chromium),
                // Explicit, so `launch_request_for` never has to derive one
                // from `ALEPH_HOME` — a routing test must not depend on, or
                // touch, the developer's own browser storage.
                user_data_dir: Some("/nonexistent/aleph-routing-test".into()),
                ..Default::default()
            },
        );
        // The Managed control, named explicitly. `default` used to serve as
        // one and cannot any more.
        config.profiles.insert(
            "managed".into(),
            ProfileConfig {
                driver: BrowserDriver::Managed,
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);

        let backend = manager
            .get_backend("cdp")
            .expect("a driver=cdp profile must route to a backend");
        assert!(
            backend
                .as_ref()
                .as_any()
                .downcast_ref::<crate::browser::cdp_backend::CdpBackend>()
                .is_some(),
            "driver=cdp must produce a CdpBackend, not whichever backend the \
             fallback arm names"
        );

        // And the other two arms must not have moved onto it. Asserted in both
        // directions because "everything is a CdpBackend now" would satisfy the
        // claim above on its own.
        // POSITIVE downcasts, not `is_none::<CdpBackend>()`. "Not a
        // CdpBackend" is true of every other backend there is, including a
        // fourth one nobody meant to route to — so the assertion would have
        // been weaker than the sentence beside it claimed, which is the same
        // gap `is_ok()` left in the first arm.
        // The Managed control is an EXPLICITLY managed profile, not `default`.
        // It used to be `default`, which stopped being a control the moment the
        // dual-engine flip made the default driver `Cdp` — a control that
        // silently becomes a copy of the thing it is controlling for
        // (判据 §3: the guard's green only covers the shape it enumerates).
        let managed = manager.get_backend("managed").expect("managed routes");
        assert!(
            managed
                .as_ref()
                .as_any()
                .downcast_ref::<PlaywrightCliBackend>()
                .is_some(),
            "the Managed arm must still produce a PlaywrightCliBackend"
        );
        let existing = manager.get_backend("user").expect("user routes");
        assert!(
            existing
                .as_ref()
                .as_any()
                .downcast_ref::<ChromeMcpBackend>()
                .is_some(),
            "the ExistingSession arm must still produce a ChromeMcpBackend"
        );
    }

    /// A `Cdp` profile must resolve a backend WITHOUT launching anything.
    ///
    /// `get_backend` is what the idle reaper calls, so a construction that
    /// resolved the engine eagerly would make an observer create the browser it
    /// is measuring. The claim is asserted by EFFECT on the registry: after
    /// routing, it is still holding no engine for this profile.
    #[test]
    fn routing_a_cdp_profile_does_not_launch_its_engine() {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "cdp".into(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                engine: Some(Engine::Chromium),
                user_data_dir: Some("/nonexistent/aleph-routing-test".into()),
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        assert!(
            !manager.engines().is_live("cdp"),
            "precondition: nothing is running before the route"
        );
        let _backend = manager.get_backend("cdp").expect("routes");
        assert!(
            !manager.engines().is_live("cdp"),
            "resolving a backend must not have started a browser"
        );
    }

    #[test]
    fn test_get_backend_routes_user_to_chrome_mcp() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let backend = manager.get_backend("user");
        assert!(backend.is_ok());
    }

    /// Tracked for the drivers whose browser Aleph launched, never for the
    /// user's own. Renamed from `..._managed_only` when the `Cdp` arm joined:
    /// a test name is a claim, and that one had become false while staying
    /// green (判据 §1).
    #[test]
    fn test_touch_tab_tracks_the_drivers_aleph_launched() {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "cdp".into(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);

        // "default" is Managed -> tracked.
        manager.touch_tab("default", "1");
        assert!(manager.has_tracked_tabs("default"));

        // A Cdp profile is one Aleph launched too -> tracked.
        manager.touch_tab("cdp", "T1");
        assert!(
            manager.has_tracked_tabs("cdp"),
            "a cdp profile's tabs are Aleph's own and must be tracked"
        );

        // "user" is ExistingSession (user's real Chrome) -> never tracked.
        manager.touch_tab("user", "1");
        assert!(!manager.has_tracked_tabs("user"));
    }

    #[tokio::test]
    async fn test_reap_idle_tabs_no_browser_is_noop() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        // Track a tab but no browser is running → list_tabs fails, profile is
        // cleared, nothing is closed.
        manager.touch_tab("default", "1");
        assert_eq!(manager.reap_idle_tabs().await, 0);
        assert!(!manager.has_tracked_tabs("default"));
    }

    /// **The tab sweeper reaches every driver `touch_tab` writes for.**
    ///
    /// `touch_tab` records `Managed | Cdp`; `reap_idle_tabs` selected `Managed`
    /// alone until the default flip. That mismatch is a writer with no reader
    /// (判据 §7), and it was invisible while `Cdp` was opt-in — the day the
    /// default profile became a Cdp profile it turned into "the default
    /// profile's `max_tabs_per_profile` and `tab_idle_timeout_secs` do
    /// nothing".
    ///
    /// Both sets are derived from `BrowserDriver::ALL` rather than listed, so a
    /// fourth driver has to be given an answer here instead of quietly
    /// inheriting one (判据 §5).
    #[tokio::test]
    async fn the_tab_sweeper_selects_exactly_the_drivers_touch_tab_records() {
        for driver in BrowserDriver::ALL {
            let mut config = BrowserSystemConfig::default();
            config.profiles.insert(
                "p".into(),
                ProfileConfig {
                    driver,
                    ..ProfileConfig::default()
                },
            );
            let manager = ProfileManager::new(config);

            manager.touch_tab("p", "T1");
            let recorded = manager.has_tracked_tabs("p");
            assert_eq!(
                recorded,
                driver != BrowserDriver::ExistingSession,
                "{driver:?}: touch_tab's own rule is 'the browsers Aleph owns'"
            );
            if !recorded {
                continue;
            }

            // No browser is running, so `list_tabs` fails and the sweeper's job
            // is to stop re-probing this profile. That the entry is GONE is the
            // effect proving the profile was selected at all — a sweeper that
            // skipped it would leave the entry in place and still return 0, so
            // the return value alone cannot tell the two apart (判据 §4).
            assert_eq!(manager.reap_idle_tabs().await, 0);
            assert!(
                !manager.has_tracked_tabs("p"),
                "{driver:?}: the sweeper never looked at this profile, so its \
                 tab entries are kept forever and its tab settings are dead"
            );
        }
    }

    #[tokio::test]
    async fn test_reap_idle_fresh_manager_is_noop() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        // No live sessions exist → nothing to tear down.
        assert_eq!(manager.reap_idle().await, 0);
    }

    /// The sweep's `list_tabs` is the OLD drivers' identity discovery point:
    /// a `Managed` profile's backend holds no registry handle, so the sweep
    /// loop is the sole writer of what those tabs were last seen at. This is
    /// the test that reddens when the loop is dropped (判据 §6: count the
    /// writers — for the old drivers there is exactly one, here).
    ///
    /// The CLI is a script that answers `tab-list` with a real
    /// playwright-format listing and exits 0 for everything else; no browser
    /// exists, which is fine — the fake's exit-0 is what `LaunchPolicy::Refuse`
    /// needs, and the listing is the subject, not the browser.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_sweep_records_a_managed_profiles_tab_identities() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let tmp = tempfile::tempdir().expect("tempdir");
        let cli = tmp.path().join("fake-playwright-cli");
        std::fs::write(
            &cli,
            "#!/bin/sh\n\
             case \" $* \" in\n\
             *\" tab-list \"*) printf '%s\\n' '### Result' '- 0: (current) [Example Domain](https://example.com/)' '- 1: [Other](https://other.example/)' ;;\n\
             esac\n\
             exit 0\n",
        )
        .expect("write the fake playwright-cli");
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod the fake playwright-cli");

        let manager = ProfileManager::new(managed_config(Some(&cli)));
        // A tracked tab makes the profile a sweep candidate; both listed tabs
        // are fresh and under the cap, so nothing is closed.
        manager.touch_tab("default", "0");
        assert_eq!(manager.reap_idle_tabs().await, 0, "no victims");

        let registry = manager.tab_registry();
        assert_eq!(
            registry.last_url("default", "0").as_deref(),
            Some("https://example.com/"),
            "the listing's URL observation reached the registry"
        );
        // The old-driver shape: no targetId was ever learnable, so the
        // registry holds the URL and must NOT be able to call the tab gone
        // (判据 §8 — 'I cannot tell' is not a verdict).
        let identity = registry
            .resolve_identity("default", "1", &[])
            .expect("a recorded identity without a targetId resolves, never TabGone");
        assert_eq!(identity.target_id, None);
        assert_eq!(identity.last_url.as_deref(), Some("https://other.example/"));
    }

    #[tokio::test]
    async fn policy_update_applies_without_a_restart() {
        // Boot with SSRF on: loopback is refused.
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        assert!(manager
            .check_url("http://127.0.0.1:9000/admin")
            .await
            .is_err());

        // The operator turns private-network blocking off (what `browser.update`
        // writes to disk). Before this hot-apply existed the manager kept its
        // boot-time guard and the RPC reported success over a no-op.
        manager.apply_policy(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });
        assert!(
            manager
                .check_url("http://127.0.0.1:9000/admin")
                .await
                .is_ok(),
            "the running manager must serve the new policy"
        );
    }

    #[tokio::test]
    async fn live_apply_reaches_a_published_manager_and_downgrades_otherwise() {
        let open = SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        };
        // No handle at all → the caller must be told the change did NOT land
        // (honest downgrade, mirroring config::live_apply).
        assert!(!apply_policy_to(None, open.clone()));

        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let handle = Arc::downgrade(&manager);
        assert!(apply_policy_to(Some(&handle), open.clone()));
        assert!(manager.check_url("http://127.0.0.1/x").await.is_ok());

        // A handle to a manager that has since been dropped is not a live
        // target either — it must downgrade, not resurrect anything.
        drop(manager);
        assert!(!apply_policy_to(Some(&handle), open));
    }

    /// The boot hook must actually call the orphan sweep.
    ///
    /// A SOURCE pin, and deliberately so: `sweep_orphaned_engines`'s
    /// production half is `cfg(not(test))` — it reads the real `$ALEPH_HOME`
    /// and kills pids, so no unit test may run it — which leaves the wire
    /// itself unobservable at runtime. Same shape and the same reason as
    /// `both_daemon_exit_paths_reap_background_jobs_and_browsers`. Deleting the
    /// call then fails a test by name, instead of silently letting every
    /// crashed daemon's Chromium survive forever.
    #[test]
    fn the_boot_hook_still_calls_the_orphan_sweep() {
        let src = include_str!("manager.rs").replace('\r', "");
        // The non-vacuity floor on the PREFIX, the containment checks on
        // `code_text` of it — the same two measurements, in the same order, as
        // `shutdown_browsers_reaches_both_browser_families`, which this test's
        // doc already names as its template. It was still using the bare
        // prefix, so a comment spelling either identifier below satisfied it.
        let prefix = crate::utils::source_scan::production_prefix(&src);
        assert!(
            prefix.len() < src.len(),
            "the #[cfg(test)] bound matched nothing — this test would then be \
             reading its own source"
        );
        let production = crate::utils::source_scan::code_text(&prefix);
        assert!(
            production.contains("Self::sweep_orphaned_engines()"),
            "spawn_idle_reaper must still call the boot sweep"
        );
        assert!(
            production.contains("engine::process::reap_orphans_now"),
            "the sweep must reach engine::process::reap_orphans_now — it is the \
             only thing that ever finds a browser a crashed daemon left running"
        );
    }

    /// A `playwright-cli` stand-in that exits 0 for every verb and, when it is
    /// asked to `close`, records whether `pid` was still alive at that moment.
    ///
    /// It is written *after* the stand-in browser is spawned, which is the only
    /// reason it can name that pid — and naming it is the point: the marker is
    /// how a unit test observes the ORDER of the reaper's two halves without
    /// being able to step inside `reap_idle`.
    #[cfg(unix)]
    fn fake_cli_recording_close(
        dir: &std::path::Path,
        pid: u32,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let marker = dir.join("close-saw-a-live-browser");
        let cli = dir.join("fake-playwright-cli");
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\n\
                 case \" $* \" in\n\
                 *\" close \"*) kill -0 {pid} 2>/dev/null && : > {marker:?} ;;\n\
                 esac\n\
                 exit 0\n",
                marker = marker.to_string_lossy(),
            ),
        )
        .expect("write the fake playwright-cli");
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod the fake playwright-cli");
        (cli, marker)
    }

    /// Move a profile's idle clock back so the reaper's timeout filter admits
    /// it, without sleeping through a real timeout.
    ///
    /// `is_idle` reads the monotonic clock and `tokio::time::pause` does not
    /// move that, so a test either waits or backdates. `checked_sub` because
    /// the clock's origin can be more recent than the offset (see `is_idle`);
    /// the caller asserts the profile really did become a candidate, so an
    /// underflow fails loudly instead of turning the test green for the wrong
    /// reason.
    fn backdate(manager: &ProfileManager, profile: &str, ago: Duration) {
        let mut profiles = manager.profiles.write().unwrap_or_else(|e| e.into_inner());
        let entry = profiles.get_mut(profile).expect("profile exists");
        if let Some(t) = std::time::Instant::now().checked_sub(ago) {
            entry.last_activity = t;
        }
    }

    /// A `BrowserSystemConfig` with one `Managed` profile that is idle the
    /// instant it stops being touched.
    fn managed_config(cli: Option<&std::path::Path>) -> BrowserSystemConfig {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Managed,
                idle_timeout_secs: 0,
                ..Default::default()
            },
        );
        config.playwright_cli.binary_path = cli.map(|p| p.to_string_lossy().into_owned());
        config
    }

    /// The view accessor spec §3.2 asks for, and the one property that makes it
    /// honest: an `ExistingSession` profile has no Aleph-owned browser, so it
    /// must answer `None` rather than somebody else's endpoint. The live view is
    /// Managed-only on purpose — a user's own Chrome is already visible to them.
    ///
    /// The `get_driver` assertion is not decoration. Without it the
    /// `ExistingSession` arm is an empty guard: point it at the driver too and
    /// the test stays green, because no browser exists either way. Pinning
    /// *which* driver the profile has is what makes the arm falsifiable
    /// (判据 §3 — a guard that has never been falsified is not a guard).
    #[tokio::test]
    async fn live_endpoint_is_none_without_a_browser_and_never_answers_for_existing_session() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        // Both auto-injected profiles exist (`ProfileManager::new`): `default`
        // is Cdp since the dual-engine flip, `user` is ExistingSession.
        assert_eq!(manager.get_driver("default"), Some(BrowserDriver::Cdp));
        assert_eq!(
            manager.get_driver("user"),
            Some(BrowserDriver::ExistingSession),
            "precondition: `user` is the ExistingSession arm this asserts on"
        );
        assert!(manager.live_endpoint("default").is_none());
        assert!(manager.live_endpoint("user").is_none());
        assert!(manager.live_endpoint("no-such-profile").is_none());
    }

    /// The falsifying half of the test above.
    ///
    /// Asserting `None` for `user` on a manager that never launched anything
    /// is an EMPTY guard: point the `ExistingSession` arm at the driver and it
    /// stays green, because the driver has nothing to answer with either
    /// (判据 §3 — a guard that cannot go red is not a guard). So give the
    /// driver something to answer with, under that very profile's key, and the
    /// arm has to be the thing that refuses.
    ///
    /// Not a contrived state: the driver's map is keyed by profile name, so a
    /// profile that is `Managed` today and `ExistingSession` in tomorrow's
    /// config is exactly this shape — and the live view showing Aleph's own
    /// stale browser as "the user's Chrome" is the failure it would produce.
    #[cfg(unix)]
    #[tokio::test]
    async fn live_endpoint_refuses_an_existing_session_profile_that_has_a_child_in_the_map() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        assert_eq!(
            manager.get_driver("user"),
            Some(BrowserDriver::ExistingSession),
            "precondition: `user` is the arm under test"
        );

        let child = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .expect("spawn the stand-in browser");
        manager.insert_test_child("user", child);

        assert!(
            manager.live_endpoint("user").is_none(),
            "the live view is Managed-only; an ExistingSession profile must not \
             be handed an endpoint even when the driver has one under its key"
        );
        assert_eq!(
            manager
                .shutdown_browsers(engine::ENGINE_SHUTDOWN_BUDGET)
                .await,
            1,
            "precondition: the driver really was holding a child to answer with"
        );
    }

    /// `session_active` used to answer from the tab registry, which its own doc
    /// called an approximation. Now that Aleph owns the process there is an
    /// exact answer, and the approximation must be GONE rather than kept beside
    /// it — two answers to "does this profile have a browser" is how they drift.
    /// Concretely: tracking a tab must no longer make a browserless profile
    /// report itself active.
    #[tokio::test]
    async fn a_tracked_tab_no_longer_fakes_a_live_managed_session() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        manager.touch_tab("default", "tab-1");
        assert!(
            manager.has_tracked_tabs("default"),
            "precondition: the registry did record the tab"
        );
        assert!(
            !manager.session_active("default"),
            "no chromium was ever launched, so the profile is not active"
        );
    }

    /// The reaper's Managed arm has two halves now, and the second one is the
    /// point: under `attach`, `playwright-cli close` only DISCONNECTS (measured
    /// — nine Chrome processes before and after). A reaper that stopped at
    /// `close` would report a reaped profile and leave the browser running
    /// forever. With no browser to begin with, the sweep must be a no-op and
    /// must not invent one.
    #[tokio::test]
    async fn the_reaper_does_not_launch_a_browser_in_order_to_reap_one() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(managed_config(None));
        // Past its timeout, so the sweep's answer is about the browser rather
        // than about the clock.
        backdate(&manager, "default", Duration::from_secs(2));
        assert_eq!(manager.reap_idle().await, 0);
        assert!(manager.live_endpoint("default").is_none());
    }

    /// spec §3.6 「退出时杀」. `std::process::Child` does NOT kill on drop, and
    /// under `attach --cdp` the CLI was never the browser's parent — so without
    /// an explicit stop every restart leaves a browser behind until the next
    /// boot sweep finds it.
    ///
    /// The fake browser is a real `sleep` subprocess, because the thing being
    /// tested is that a live pid stops being live. A mock child would assert
    /// that a method was called (判据 §4: assert the effect arrived, not that
    /// the call happened).
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_browsers_kills_what_it_launched_and_says_how_many() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());

        // Nothing launched → nothing to stop, and it must not pretend otherwise.
        assert_eq!(
            manager
                .shutdown_browsers(engine::ENGINE_SHUTDOWN_BUDGET)
                .await,
            0
        );

        // A stand-in browser: long-lived, harmless, and observable by pid.
        let child = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = child.id();
        manager.insert_test_child("default", child);
        assert!(
            crate::utils::process_alive::is_process_alive(pid as i32),
            "precondition: the stand-in is running"
        );

        assert_eq!(
            manager
                .shutdown_browsers(engine::ENGINE_SHUTDOWN_BUDGET)
                .await,
            1
        );
        assert!(
            !crate::utils::process_alive::is_process_alive(pid as i32),
            "the stand-in browser is still running after shutdown_browsers"
        );
        // Idempotent: a second stop finds nothing and says so.
        assert_eq!(
            manager
                .shutdown_browsers(engine::ENGINE_SHUTDOWN_BUDGET)
                .await,
            0
        );
    }

    /// The central behavioural claim of the launch-chain flip, asserted in the
    /// only direction a unit test can speak to it: **Aleph's own reclamation
    /// does not come from `close`.**
    ///
    /// The CLI here is a script that succeeds at everything, so the reaper's
    /// first half runs to completion — and it records whether the browser was
    /// still alive when it ran. Both facts together are the order: `close`
    /// happened over a live browser (so the reaper did not kill first, which
    /// would drop the CLI session's own teardown), and the browser is gone
    /// afterwards (so something other than `close` reclaimed it).
    ///
    /// That the *external* `playwright-cli close` leaves the browser running
    /// was measured in the spike (nine Chrome processes before and after) and
    /// is not a property a unit test can hold; what it can hold is that Aleph
    /// no longer depends on it doing anything.
    #[cfg(unix)]
    #[tokio::test]
    async fn reap_idle_closes_the_cli_session_then_kills_the_browser_close_left_running() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let tmp = tempfile::tempdir().expect("tempdir");

        let child = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = child.id();
        let (cli, close_saw_live_browser) = fake_cli_recording_close(tmp.path(), pid);

        let manager = ProfileManager::new(managed_config(Some(&cli)));
        manager.insert_test_child("default", child);
        backdate(&manager, "default", Duration::from_secs(2));
        assert_eq!(
            manager.idle_managed_profiles(),
            vec!["default".to_string()],
            "precondition: an idle profile WITH a browser is a reap candidate"
        );

        assert_eq!(
            manager.reap_idle().await,
            1,
            "the sweep reclaimed a browser and must say so"
        );
        assert!(
            close_saw_live_browser.exists(),
            "the cli `close` either did not run or ran after the kill"
        );
        assert!(
            !crate::utils::process_alive::is_process_alive(pid as i32),
            "`close` disconnects; only the kill reclaims — the browser is still running"
        );
        assert!(
            manager.live_endpoint("default").is_none(),
            "the reaped profile must not still advertise an endpoint"
        );
    }

    /// A Managed browser that exited on its own is still the reaper's to clean
    /// up — the flip to `chromium_alive` must not turn "died" into "nothing to
    /// do here".
    ///
    /// Three different things outlive that browser and only this sweep clears
    /// them while the daemon runs: the child record in the driver's map, the
    /// sidecar file that names its pid on disk, and the profile's tab entries.
    /// `chromium_died` exists as a concept distinct from "no browser" (Task 5)
    /// for OTHER call sites (`run`'s pre-verb check reads it alone) — but the
    /// candidate filter here admits both `chromium_alive` and `chromium_died`
    /// profiles for a simpler reason than recognising two facts: `chromium_died`
    /// is `key_present && !alive`, so the disjunction is exactly "Aleph has a
    /// record for this profile at all", which is what "something to reclaim"
    /// actually requires.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_browser_that_died_on_its_own_is_still_reclaimed() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(managed_config(None));

        // A browser that is already gone: spawned, then reaped, so `try_wait`
        // has a definite answer rather than a racy one.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn the stand-in browser");
        child
            .wait()
            .expect("the stand-in browser exits immediately");
        manager.insert_test_child("default", child);
        manager.touch_tab("default", "tab-1");
        backdate(&manager, "default", Duration::from_secs(2));

        assert!(
            !manager.session_active("default"),
            "precondition: the browser really is gone"
        );
        assert!(
            manager.has_tracked_tabs("default"),
            "precondition: the registry has something to clear"
        );

        assert_eq!(
            manager.reap_idle().await,
            1,
            "a dead browser's record, sidecar and tabs are still state to reclaim"
        );
        assert!(manager.live_endpoint("default").is_none());
        assert!(
            !manager.has_tracked_tabs("default"),
            "the tab entries survived the sweep that was supposed to clear them"
        );
    }

    /// A failed `close` is not a reason to keep a browser.
    ///
    /// The CLI is unavailable here (the sealed test twin refuses to install
    /// one), so the first half of the reaper's Managed arm cannot even run. The
    /// browser is still Aleph's child, and the CLI's opinion of its own session
    /// says nothing about it — treating that `Err` as a reason to skip the kill
    /// would make a broken CLI, the state most likely to have leaked a browser,
    /// the one state where Aleph refuses to reclaim it (判据 §8).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_close_that_could_not_run_does_not_spare_the_browser() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let manager = ProfileManager::new(managed_config(None));
        let child = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = child.id();
        manager.insert_test_child("default", child);
        backdate(&manager, "default", Duration::from_secs(2));

        assert_eq!(manager.reap_idle().await, 1);
        assert!(
            !crate::utils::process_alive::is_process_alive(pid as i32),
            "a browser survived a sweep because the cli could not be run"
        );
    }

    #[test]
    fn test_get_backend_nonexistent_profile() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let backend = manager.get_backend("nonexistent");
        assert!(matches!(backend, Err(BrowserError::ProfileNotFound(_))));
    }
    /// The one ordering claim that matters: a failed migration must never cost
    /// the user the browser they already had.
    #[tokio::test]
    async fn a_failed_import_leaves_the_source_engine_running_and_selected() {
        use crate::browser::engine::Engine;
        use crate::browser::engine::EngineLaunch;
        use crate::browser::testkit::switch_fixture;

        // `switch_engine` and `engine_handle_for` both reach
        // `launch_request_for_engine` -> `browser_state_dir` -> the process's
        // real `$ALEPH_HOME`. Nothing is written there (the fake launcher
        // ignores `data_dir`), but a test that READS the developer's home is
        // one whose result depends on it, so it holds the guard like every
        // other resolver-reaching test under `src/browser/`.
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let fixture = switch_fixture(
            Engine::Obscura,
            Engine::Chromium,
            /* target_rejects_cookies */ true,
        )
        .await;

        let err = fixture
            .manager
            .switch_engine("default", Engine::Chromium, true)
            .await
            .expect_err("a target that refuses the cookies must fail the switch");
        // The VARIANT and the method, not a substring of the rendered text.
        // This assertion was `err.to_string().contains("chromium")` and it
        // passed for the wrong reason: the launch was failing at the readiness
        // gate (the fake answered `"ok"` to the gate's `evaluate`), and the
        // `LaunchFailed` text happens to contain the word "chromium" because
        // it now names `browser_open{engine:"chromium"}` as the recovery. A
        // reading taken BESIDE the path is not a reading ABOUT it.
        assert!(
            matches!(&err, BrowserError::Cdp { engine, method, .. }
                     if *engine == Engine::Chromium && method == "Network.setCookies"),
            "the switch must fail on the TARGET's cookie write, got {err:?}"
        );
        assert!(
            err.to_string().contains("chromium"),
            "and the rendered text must name the engine that failed, got: {err}"
        );
        assert!(
            !fixture.source_killed(),
            "the source engine must still be alive after a failed import"
        );
        // And the profile still routes to the engine it had. `Refuse` proves
        // the handle was found rather than launched.
        let handle = fixture
            .manager
            .engine_handle_for("default", Engine::Obscura, EngineLaunch::Refuse)
            .await
            .expect("the source handle is still the profile's handle");
        assert_eq!(handle.engine, Engine::Obscura);
        // The override was not recorded either: a half-applied switch would
        // leave every later verb resolving an engine the registry does not
        // hold.
        assert_eq!(
            fixture.manager.adopted_engine("default"),
            None,
            "a failed switch must not adopt the engine it could not reach"
        );
        // And the operator's own config is untouched in either direction — it
        // is the authority `user_data_dir`'s owner is read from, so an override
        // must never write it (判据 §1).
        assert_eq!(
            fixture.manager.get_config("default").and_then(|c| c.engine),
            None,
            "an override must not write the configured engine"
        );
    }

    #[tokio::test]
    async fn switching_to_the_engine_already_running_is_refused_by_name() {
        use crate::browser::engine::Engine;
        use crate::browser::testkit::switch_fixture;

        let fixture = switch_fixture(Engine::Obscura, Engine::Chromium, false).await;
        let err = fixture
            .manager
            .switch_engine("default", Engine::Obscura, true)
            .await
            .expect_err("a no-op switch must be refused, not reported as success");
        assert!(
            matches!(err, BrowserError::AlreadyOnEngine { .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn a_successful_switch_kills_the_source_and_replaces_the_handle() {
        use crate::browser::engine::Engine;
        use crate::browser::engine::EngineLaunch;
        use crate::browser::testkit::switch_fixture;

        // `switch_engine` and `engine_handle_for` both reach
        // `launch_request_for_engine` -> `browser_state_dir` -> the process's
        // real `$ALEPH_HOME`. Nothing is written there (the fake launcher
        // ignores `data_dir`), but a test that READS the developer's home is
        // one whose result depends on it, so it holds the guard like every
        // other resolver-reaching test under `src/browser/`.
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let fixture = switch_fixture(Engine::Obscura, Engine::Chromium, false).await;

        let report = fixture
            .manager
            .switch_engine("default", Engine::Chromium, true)
            .await
            .expect("switch");
        assert_eq!(report.from, Engine::Obscura);
        assert_eq!(report.to, Engine::Chromium);
        assert_eq!(report.cookies_moved, 1);
        assert_eq!(report.tabs_reopened, 1);

        // Those two counts are the code's own bookkeeping. The claim they stand
        // for — "the cookie left the source and arrived at the target" — is only
        // settled by the bytes, so it is read off both fakes (判据 §4: assert
        // the effect arrived, not that the call was made).
        let read_from_source = fixture
            .source
            .received()
            .into_iter()
            .any(|m| m.get("method").and_then(|x| x.as_str()) == Some("Network.getAllCookies"));
        assert!(
            read_from_source,
            "the source's cookie jar was never read: {:?}",
            fixture.source.received()
        );
        let landed = fixture
            .target
            .received()
            .into_iter()
            .find(|m| m.get("method").and_then(|x| x.as_str()) == Some("Network.setCookies"))
            .expect("the target must be sent the cookies");
        let cookies = landed["params"]["cookies"]
            .as_array()
            .expect("cookies array")
            .clone();
        assert_eq!(cookies.len(), 1, "{cookies:?}");
        assert_eq!(cookies[0]["name"], "sid", "{cookies:?}");
        assert!(
            fixture.source_killed(),
            "the source process must be killed once the target holds the state"
        );
        // The page tree is read AFTER the point of no return, and this fake
        // cannot answer a `Page.getLayoutMetrics` (measured: it fails with
        // `reply has no cssVisualViewport`). The contract is that such a
        // failure is a WARNING on a completed switch, never an error — telling
        // the model the escape hatch failed while it stands on the far side of
        // it would send it back to `switch_engine`, which now answers
        // `AlreadyOnEngine`. The success side of this read is covered on real
        // binaries by `qa/browser_dual/run.sh switch`.
        assert!(
            report.snapshot.is_none(),
            "this fake cannot serve a snapshot; if it now can, this assertion \
             is measuring something else"
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("could not be read")),
            "a read that failed after the point of no return must be stated, \
             not swallowed: {:?}",
            report.warnings
        );
        let handle = fixture
            .manager
            .engine_handle_for("default", Engine::Chromium, EngineLaunch::Refuse)
            .await
            .expect("the target is now the profile's handle");
        assert_eq!(handle.engine, Engine::Chromium);
        // The profile's LIVE engine moved with it. Without this every verb
        // after the switch resolves obscura, meets a chromium handle in the
        // registry and answers `EngineMismatch` — a switch that reports success
        // over a browser nothing can address (判据 §11).
        assert_eq!(
            fixture.manager.adopted_engine("default"),
            Some(Engine::Chromium)
        );
        assert_eq!(
            fixture
                .manager
                .launch_request_for("default")
                .expect("request")
                .0,
            Engine::Chromium,
            "and it is the engine `get_backend` will build a backend for"
        );
        // The operator's config is NOT what moved. `request_from` asks it which
        // engine `user_data_dir` belongs to, so an override that wrote it would
        // hand the operator's obscura directory to chromium on the next
        // relaunch (判据 §1 — one fact, one author).
        assert_eq!(
            fixture.manager.get_config("default").and_then(|c| c.engine),
            None,
            "the runtime override must not rewrite the configured engine"
        );
    }

    /// A driver with no engine must not reach the engine registry at all.
    ///
    /// The regression this pins: `prepare_engine` gated the driver only when an
    /// engine was explicitly REQUESTED, and `resolved_engine` answers
    /// `Chromium` for `Managed`/`ExistingSession` — so a plain `browser_open`
    /// on the auto-injected `user` profile launched a CDP Chromium that nothing
    /// then used. On a default install it did worse: `ChromiumLauncher::launch`
    /// wanted `managed_cli_path()` before it resolved a binary, so the call
    /// failed outright and blamed playwright-cli, on a host whose own
    /// `existing_session_driver_ready()` calls that driver ready. **Past tense
    /// on purpose**: W5 moved that lookup into
    /// `chromium_resolve::resolve_binary`'s third route. The regression this
    /// test pins is unaffected — it is about the driver gate, not the launcher
    /// — but a present-tense clause here would be a second author for a
    /// mechanism that now has one.
    ///
    /// **Asserted at this level rather than through `BrowserOpenTool`, and the
    /// reason is not convenience.** The tool-level version would continue into
    /// `make_backend` → `ChromeMcpBackend::open_tab` →
    /// `ChromeMcpDriver::ensure_session` → `create_session`, whose failure arm
    /// **launches Chrome**. A unit test that spawns the developer's browser is
    /// not a test. What the tool adds over this is one `match` arm, and
    /// `browser_open`'s own `engine: None` field is asserted by
    /// `an_engine_override_against_a_live_other_engine_names_switch_engine`.
    ///
    /// `launches()` is the effect, not the call (判据 §4): it is the list the
    /// fake launcher appends to inside `launch`, so a green here means no
    /// process was ever asked for.
    #[tokio::test]
    async fn a_driver_with_no_engine_never_reaches_the_engine_registry() {
        use crate::browser::engine::Engine;
        use crate::browser::testkit::switch_fixture;

        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let fixture = switch_fixture(Engine::Obscura, Engine::Chromium, false).await;

        // `user` is auto-injected by `ProfileManager::new` on every install,
        // with `driver: ExistingSession`. Read it rather than asserting a
        // literal, so a change to the auto-injection reddens here.
        assert_eq!(
            fixture.manager.get_driver("user"),
            Some(BrowserDriver::ExistingSession),
            "this test's whole subject is the auto-injected profile"
        );
        // The TARGET launcher is the one a stray launch would grow: the bug
        // sent `resolved_engine`'s `Chromium` to the registry, and the source
        // here is obscura. Counting the source's launches would be a reading
        // beside the path.
        let launches_before = fixture.target_proc.launches().len();

        let prepared = fixture
            .manager
            .prepare_engine("user", None)
            .await
            .expect("a driver with no engine is not an error");
        assert_eq!(
            prepared, None,
            "there is no engine on this path, and naming one would put a label \
             on a browser that does not exist"
        );
        assert_eq!(
            fixture.target_proc.launches().len(),
            launches_before,
            "nothing may be launched for a driver that has no engine"
        );

        // And asking for one explicitly is still refused BY NAME, before any
        // launch — the half of the old gate that was correct.
        let err = fixture
            .manager
            .prepare_engine("user", Some(Engine::Chromium))
            .await
            .expect_err("an engine on a driver that has none is a model mistake");
        let text = err.to_string();
        assert!(text.contains("existing_session"), "got: {text}");
        assert!(text.contains("browser_profile"), "got: {text}");
        assert_eq!(
            fixture.target_proc.launches().len(),
            launches_before,
            "the refusal must come before the launch, not after it"
        );
    }

    /// The engine that survives the switch keeps a record of its own.
    ///
    /// `sidecar_path` was keyed on the session key alone, which was true while
    /// a profile could only have one engine. This task made two of them live at
    /// once, so the target's record overwrote the source's on launch and the
    /// source's `stop_launched` then deleted that same file — leaving the
    /// chromium that holds every migrated cookie invisible to
    /// `reap_orphans` for the rest of the process's life.
    ///
    /// The fixture shares ONE sidecar directory, because production has one
    /// registry: with a directory per launcher the collision cannot happen here
    /// however coarse the key is (判据 §10).
    #[tokio::test]
    async fn a_switch_leaves_the_surviving_engine_a_record_of_its_own() {
        use crate::browser::engine::process::sidecar_file_name;
        use crate::browser::engine::Engine;
        use crate::browser::testkit::switch_fixture;

        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let fixture = switch_fixture(Engine::Obscura, Engine::Chromium, false).await;

        let source_record = fixture
            .sidecar_dir
            .join(sidecar_file_name(Engine::Obscura, "default"));
        let target_record = fixture
            .sidecar_dir
            .join(sidecar_file_name(Engine::Chromium, "default"));
        assert!(
            source_record.exists(),
            "the source's own launch wrote a record: {source_record:?}"
        );

        fixture
            .manager
            .switch_engine("default", Engine::Chromium, true)
            .await
            .expect("switch");

        assert!(
            target_record.exists(),
            "the engine that SURVIVED the switch has no orphan-reap record; a \
             crash from here on strands it forever ({target_record:?} in {:?})",
            std::fs::read_dir(&fixture.sidecar_dir)
                .map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
                .unwrap_or_default()
        );
        assert!(
            !source_record.exists(),
            "the source died, so its record must be gone: {source_record:?}"
        );
        assert_ne!(
            source_record, target_record,
            "two engines of one profile must not share one record"
        );
    }

    /// `user_data_dir` belongs to the engine the OPERATOR configured, and a
    /// runtime override must not move that answer.
    ///
    /// The defect this pins: `request_from` decides applicability with
    /// `engine == configured`, and `configured` was
    /// `cfg.resolved_engine(default)` — the very value an adopt used to write.
    /// One switch and the fact had no independent author left, so a relaunch
    /// pointed Chromium at obscura's store, a switch back abandoned the
    /// operator's directory, and the warning named the wrong engine (判据 §1).
    ///
    /// No registry and no switch here on purpose: the whole defect is in the
    /// derivation, so the test drives the derivation.
    #[test]
    fn an_adopted_engine_does_not_inherit_the_operators_user_data_dir() {
        use crate::browser::engine::Engine;

        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let operator_dir = home.path().join("operator-chose-this");

        let mut config = BrowserSystemConfig {
            default_engine: Engine::Obscura,
            ..Default::default()
        };
        config.profiles.insert(
            "p".into(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                engine: Some(Engine::Obscura),
                user_data_dir: Some(operator_dir.to_string_lossy().into_owned()),
                ..ProfileConfig::default()
            },
        );
        let manager = ProfileManager::new(config);

        let obscura_dir = |m: &ProfileManager| {
            m.launch_request_for_engine("p", Engine::Obscura)
                .expect("request")
                .data_dir
        };
        let chromium_dir = |m: &ProfileManager| {
            m.launch_request_for_engine("p", Engine::Chromium)
                .expect("request")
                .data_dir
        };

        // Before any override: the configured engine gets the operator's
        // directory, the other engine gets its own managed one.
        assert_eq!(obscura_dir(&manager), operator_dir);
        assert_ne!(chromium_dir(&manager), operator_dir);

        // Now the profile is running on chromium via a runtime override.
        manager.set_adopted_engine("p", Some(Engine::Chromium));

        // The LIVE engine moved — that is what `get_backend` resolves …
        assert_eq!(
            manager.launch_request_for("p").expect("request").0,
            Engine::Chromium
        );
        // … and the operator's directory did NOT move with it.
        assert_ne!(
            chromium_dir(&manager),
            operator_dir,
            "the adopted engine was handed the directory the operator wrote for \
             the other one"
        );
        assert_eq!(
            obscura_dir(&manager),
            operator_dir,
            "switching back must return to the operator's own store"
        );
        assert_eq!(
            manager.get_config("p").and_then(|c| c.engine),
            Some(Engine::Obscura),
            "the config is the authority for `user_data_dir`'s owner and an \
             override must not write it"
        );
    }

    /// A source that refuses to die is not a clean switch.
    ///
    /// `EngineHandle::shutdown` answers `true` only when the process actually
    /// died. The return value used to be dropped, so "I could not kill it" was
    /// reported as "it is gone" (判据 §8) — and the state that hides is the
    /// expensive one: both browsers running, the old one still holding this
    /// profile's original cookies, and a report saying the switch succeeded.
    #[tokio::test]
    async fn a_source_that_refuses_to_die_is_reported_not_swallowed() {
        use crate::browser::engine::Engine;
        use crate::browser::testkit::{switch_fixture_with_source_kill, ScriptedKill};

        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let fixture = switch_fixture_with_source_kill(
            Engine::Obscura,
            Engine::Chromium,
            false,
            ScriptedKill::Survived,
        )
        .await;

        let report = fixture
            .manager
            .switch_engine("default", Engine::Chromium, true)
            .await
            .expect("the switch itself still completed");

        assert!(
            fixture.source_killed(),
            "the fixture must have been asked to kill it, or this test is \
             about nothing"
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("did not exit") && w.contains("obscura")),
            "a source that survived must be stated, and named: {:?}",
            report.warnings
        );
    }

    /// r11 N5: one configured profile, a store per principal — on BOTH
    /// families of browser this manager owns: the playwright-managed Chromium
    /// (`chromium-udd/<key>`) and a CDP engine (`<engine>/<key>`). The owner
    /// and an actor-less caller keep the `default` directory every install
    /// already has.
    #[test]
    fn each_principal_gets_its_own_browser_store_on_both_engines() {
        use crate::gateway::security::store::OWNER_USER_ID;
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Managed,
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        let udd = |principal: Option<&str>| {
            let key = manager
                .principal_profile("default", principal)
                .expect("resolves");
            let cfg = manager
                .get_config(&key)
                .expect("the key is a profile entry");
            crate::browser::playwright_cli::chromium_user_data_dir(
                &SessionLaunch::from_profile(&cfg, true),
                &key,
            )
            .expect("home resolves")
        };
        let owner = udd(Some(OWNER_USER_ID));
        assert!(
            owner.ends_with("chromium-udd/default"),
            "{}",
            owner.display()
        );
        assert_eq!(
            udd(None),
            owner,
            "an actor-less caller drives the owner's browser"
        );
        let alice = udd(Some("u-alice"));
        let bob = udd(Some("u-bob"));
        assert!(
            alice.ends_with("chromium-udd/default__u-alice"),
            "{}",
            alice.display()
        );
        assert!(
            bob.ends_with("chromium-udd/default__u-bob"),
            "{}",
            bob.display()
        );
        assert_ne!(alice, bob);

        for engine in [Engine::Chromium, Engine::Obscura] {
            let mut config = BrowserSystemConfig::default();
            config.profiles.insert(
                "default".into(),
                ProfileConfig {
                    driver: BrowserDriver::Cdp,
                    engine: Some(engine),
                    ..Default::default()
                },
            );
            let manager = ProfileManager::new(config);
            let data_dir = |principal: Option<&str>| {
                let key = manager.principal_profile("default", principal).unwrap();
                let (_, req) = manager.launch_request_for(&key).expect("launchable");
                assert_eq!(
                    req.session_key, key,
                    "registry key and data dir share one string"
                );
                req.data_dir
            };
            let sub = engine.data_subdir();
            assert!(data_dir(Some(OWNER_USER_ID)).ends_with(format!("{sub}/default")));
            assert!(data_dir(Some("u-alice")).ends_with(format!("{sub}/default__u-alice")));
            assert!(data_dir(Some("u-bob")).ends_with(format!("{sub}/default__u-bob")));
        }
    }

    /// A configured `user_data_dir` is the OPERATOR's browser store, logins
    /// included. It stays the owner's; a second principal gets a managed one.
    #[test]
    fn a_configured_user_data_dir_is_never_handed_to_another_principal() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Managed,
                user_data_dir: Some("/operator/chrome".into()),
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        let owner = manager.principal_profile("default", None).unwrap();
        assert_eq!(
            manager.get_config(&owner).unwrap().user_data_dir.as_deref(),
            Some("/operator/chrome")
        );
        let alice = manager
            .principal_profile("default", Some("u-alice"))
            .unwrap();
        let cfg = manager.get_config(&alice).unwrap();
        assert_eq!(cfg.user_data_dir, None);
        let dir = crate::browser::playwright_cli::chromium_user_data_dir(
            &SessionLaunch::from_profile(&cfg, true),
            &alice,
        )
        .unwrap();
        assert!(
            dir.ends_with("chromium-udd/default__u-alice"),
            "{}",
            dir.display()
        );
    }

    /// A composed name is the OUTPUT of `principal_profile`, never an input:
    /// refused as "not found", in the same words as a profile that does not
    /// exist, whoever asks — including the principal it names. The configured
    /// `work__u-bob` is the case only the `is_composed_id` gate catches (the
    /// `materialized_for` filter covers the materialized ones).
    #[test]
    fn a_composed_profile_name_is_refused_as_not_found() {
        let mut config = BrowserSystemConfig::default();
        config
            .profiles
            .insert("work__u-bob".into(), ProfileConfig::default());
        let manager = ProfileManager::new(config);
        manager
            .principal_profile("default", Some("u-alice"))
            .unwrap();
        for (input, who) in [
            ("default__u-alice", Some("u-bob")),
            ("default__u-alice", Some("u-alice")),
            ("default__u-alice", None),
            ("default__p-room", Some("u-bob")),
            ("work__u-bob", Some("u-carol")),
            ("work__u-bob", None),
        ] {
            let err = manager
                .principal_profile(input, who)
                .expect_err("a composed name must be refused");
            assert_eq!(
                err.to_string(),
                BrowserError::ProfileNotFound(input.into()).to_string(),
                "{input} as {who:?}"
            );
        }
    }

    /// `ExistingSession` attaches to the machine owner's own Chrome; there is
    /// no per-principal copy of it to hand out (assumption A-1).
    #[test]
    fn only_the_owner_may_drive_the_existing_session_profile() {
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        assert_eq!(manager.principal_profile("user", None).unwrap(), "user");
        assert!(matches!(
            manager.principal_profile("user", Some("u-alice")),
            Err(BrowserError::ActionFailed(_))
        ));
        assert!(
            manager.get_config("user__u-alice").is_none(),
            "a refusal must not leave a materialized entry behind"
        );
    }

    /// The listing a principal sees: configured names only, its own liveness,
    /// nobody else's materialized copy, no composed key.
    #[test]
    fn a_principal_lists_configured_names_only() {
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        manager.principal_profile("default", Some("u-bob")).unwrap();
        let alice: Vec<String> = manager
            .list_profiles_for(Some("u-alice"))
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(alice.contains(&"default".to_string()), "{alice:?}");
        assert!(alice.iter().all(|n| !n.contains("__")), "{alice:?}");
        assert!(!alice.contains(&"user".to_string()), "{alice:?}");
        let mut owner: Vec<String> = manager
            .list_profiles_for(None)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        owner.sort();
        assert_eq!(owner, vec!["default".to_string(), "user".to_string()]);
    }
}
