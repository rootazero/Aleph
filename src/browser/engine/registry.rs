//! The one place an engine process is launched, cached and stopped.
//!
//! Held as an `Arc` by `ProfileManager` and by every CDP backend, because
//! `ProfileManager::get_backend` is SYNCHRONOUS (and the idle reaper calls it)
//! and so cannot resolve a handle at construction. A backend takes the
//! registry and resolves on its own first async call.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::process::{EngineProcess, LaunchRequest, Launched};
use super::readiness::ready_gate;
use super::{stop_launched, Engine, EngineHandle, EngineLaunch, ENGINE_SHUTDOWN_BUDGET};
use crate::browser::error::BrowserError;

pub struct EngineRegistry {
    /// One live engine per profile. A `tokio::sync::Mutex` held across the
    /// whole get-or-launch, which is what makes "the second caller gets the
    /// same handle" true rather than merely likely. It serialises the FIRST
    /// launch of two profiles against each other; if that ever costs anything,
    /// copy the per-key lock map at `src/runtimes/ensure.rs` — do not invent a
    /// third shape.
    handles: tokio::sync::Mutex<HashMap<String, Arc<EngineHandle>>>,
    /// The launcher for each engine. A missing entry is an honest "this build
    /// cannot start that engine", not a panic and not a silent fallback.
    processes: HashMap<Engine, Arc<dyn EngineProcess>>,
    command_timeout: Duration,
    ready_budget: Duration,
}

impl EngineRegistry {
    #[must_use]
    pub fn new(
        processes: HashMap<Engine, Arc<dyn EngineProcess>>,
        command_timeout: Duration,
        ready_budget: Duration,
    ) -> Self {
        Self {
            handles: tokio::sync::Mutex::new(HashMap::new()),
            processes,
            command_timeout,
            ready_budget,
        }
    }

    /// The live engine for `req.profile`, launching one if `gate` allows.
    ///
    /// Get-or-launch under one lock: a second caller either finds the handle or
    /// waits for the first launch, never starts a second browser.
    ///
    /// A handle whose connection has closed is **stopped and then** replaced,
    /// and only by an `Allow` caller: spec §5.2 says the next call restarts the
    /// same engine rather than switching, and a closed socket says Aleph lost
    /// its grip, not that the browser exited. Under [`EngineLaunch::Refuse`]
    /// that next call is not this one, so it answers `NoSession` and leaves the
    /// dead handle exactly where it found it — an observer must not be the
    /// thing that reclaims a browser.
    pub async fn handle(
        &self,
        engine: Engine,
        req: &LaunchRequest,
        gate: EngineLaunch,
    ) -> Result<Arc<EngineHandle>, BrowserError> {
        let mut map = self.handles.lock().await;
        let existing = map.get(&req.profile).cloned();

        // A LIVE engine is a determinate answer for both gates, and reaching it
        // changes nothing.
        if let Some(handle) = &existing {
            if handle.alive() {
                if handle.engine != engine {
                    // Answered under `Refuse` too, on purpose. It is a fact
                    // about the world — this profile is running that engine —
                    // and `NoSession` would be a lie an observer then spends as
                    // a licence to launch a second browser (判据 §8). The
                    // recovery verb the message names is addressed to whoever
                    // eventually acts; naming it does not oblige an observer to.
                    return Err(BrowserError::EngineMismatch {
                        profile: req.profile.clone(),
                        running: handle.engine,
                        requested: engine,
                    });
                }
                return Ok(handle.clone());
            }
        }

        // Everything past here either MUTATES the map or starts a browser, so
        // the gate decides before any of it. To an observer a closed socket and
        // an empty slot are the same answer — "no live engine for this profile"
        // — and it must leave both exactly as it found them. `EngineLaunch::Refuse`
        // says so in its own doc ("Observing. A missing engine is an answer, not
        // something to fix"), and this used to evict the dead handle before ever
        // reading the gate: a sensor that changed what it measured (判据 §4).
        if gate == EngineLaunch::Refuse {
            return Err(BrowserError::NoSession(req.profile.clone()));
        }

        // An `Allow` caller is the only one that may replace a dead handle, so
        // it owns reclaiming what it displaces.
        //
        // **A closed socket is not a dead process.** It means Aleph lost its
        // grip: the browser may well still be running, and `EngineHandle` has
        // no `Drop`, so simply forgetting the handle orphans it — and the
        // relaunch below writes a sidecar under the same session key, which
        // `write_sidecar_record` documents as an *overwrite*, destroying the
        // orphan's only record. Kill first, and only then let the launch have
        // the slot.
        if let Some(dead) = existing {
            map.remove(&req.profile);
            tracing::warn!(
                profile = %req.profile,
                engine = dead.engine.as_str(),
                pid = dead.launched.pid,
                "the engine's CDP connection is closed; stopping the process before \
                 relaunching, because a closed socket says we lost our grip, not that \
                 the browser exited"
            );
            dead.shutdown().await;
        }

        // The launch itself is `launch_engine`, which takes no `&self` and so
        // CANNOT reach `handles` — the map guard above is still held here, and
        // a launch path that locked it again would deadlock rather than fail.
        // Structural, not guarded: the signature is the proof.
        let handle = launch_engine(
            &self.processes,
            engine,
            req,
            self.command_timeout,
            self.ready_budget,
        )
        .await?;
        map.insert(req.profile.clone(), handle.clone());
        Ok(handle)
    }

    /// Launch, connect and pass the readiness gate — and park NOTHING.
    ///
    /// The primitive `switch_engine` (Task 19) needs: bring the target engine
    /// up while the source is still parked and still serving, migrate across
    /// it, and only then retire the source. [`Self::handle`] cannot do that —
    /// it would answer `EngineMismatch` for exactly the profile being switched.
    ///
    /// Ignores any existing entry for `req.profile` on purpose; it neither
    /// reads nor writes the map.
    ///
    /// ⚠️ **The caller owns this handle.** Nothing else will ever stop it:
    /// [`Self::shutdown_all`] drains the map, and this is not in the map. Park
    /// it with [`Self::replace`] or stop it with [`EngineHandle::shutdown`], on
    /// every path including the error ones — a detached handle that is dropped
    /// is a browser process nobody can find again until the next boot sweep.
    pub async fn launch_detached(
        &self,
        engine: Engine,
        req: &LaunchRequest,
    ) -> Result<Arc<EngineHandle>, BrowserError> {
        launch_engine(
            &self.processes,
            engine,
            req,
            self.command_timeout,
            self.ready_budget,
        )
        .await
    }

    /// Park `new` under `profile` and hand back whatever it displaced.
    ///
    /// Atomic in the sense that matters: one lock acquisition, so no caller can
    /// observe the profile with no engine, and none can be handed the old
    /// engine after the new one is in place. `launch_detached` + `replace` is
    /// `switch_engine`'s whole shape.
    ///
    /// ⚠️ The returned handle is **still running**. Whoever calls this owns
    /// stopping it; dropping the `Option` leaks a browser (判据 §15 — the
    /// hand-off across an irreversible boundary is the caller's to complete).
    pub async fn replace(
        &self,
        profile: &str,
        new: Arc<EngineHandle>,
    ) -> Option<Arc<EngineHandle>> {
        self.handles.lock().await.insert(profile.to_string(), new)
    }

    /// The handle for `profile`, without launching one. A sensor must not
    /// create what it measures.
    pub async fn get(&self, profile: &str) -> Option<Arc<EngineHandle>> {
        self.handles.lock().await.get(profile).cloned()
    }

    /// Whether `profile` has a live engine — **synchronously**.
    ///
    /// `ProfileManager::session_active` is sync and its `Managed` arm asks
    /// `PlaywrightCliDriver::chromium_alive`; the `Cdp` arm asks this. A sync
    /// reader of a `tokio::sync::Mutex` can only `try_lock`, so both failure
    /// modes answer `false`, and both are the fail-closed direction here: a
    /// caller told "not live" opens a browser, which is merely wasteful, while
    /// a spurious "live" strands the profile with a session nothing can drive.
    ///
    /// `.is_some_and(|h| h.alive())`, not `.is_some()`: a map entry whose
    /// connection has closed is a dead engine, and answering "live" for it is
    /// the same lie `session_active` used to tell when it read the tab registry
    /// instead of the process. Parity with the `Managed` arm is the point —
    /// both ask the thing, not the bookkeeping.
    ///
    /// A contended lock means some other call is mid-launch or mid-shutdown for
    /// SOME profile; `false` is then "I could not tell", spent in the direction
    /// that cannot strand anything (判据 §8).
    ///
    /// ⚠️ **That reasoning is about the LAUNCH consumer, and there is a second
    /// one.** `session_active` also feeds `browser_profile`'s listing
    /// (`builtin_tools/browser_tools/profile_tool.rs`), where the cost of a
    /// spurious `false` is not a wasted launch but a **wrong label on a live
    /// session** — and a wrong label costs more than a missing one (判据 §17).
    /// The behaviour is deliberately unchanged, because the same `false` is
    /// still the right answer for the launch consumer and a display cannot be
    /// allowed to dictate a safety default.
    ///
    /// **How long that wrong label can last**, derived rather than quoted, so
    /// that when one of these moves this paragraph is wrong in a way a reader
    /// can see instead of a way only a stopwatch can find. The lock is held by
    /// [`Self::handle`] across, in order:
    /// a dead handle's `shutdown()` (≤ [`super::ENGINE_KILL_GRACE`]),
    /// `process.launch()` (≤ [`super::chromium::DEVTOOLS_PORT_DEADLINE`] for
    /// the spawn, **plus an unbounded binary-resolve step that no constant
    /// covers**), and `bring_up` (≤ [`super::readiness::READY_GATE_BUDGET`]).
    /// Sum the three constants for the floor; the resolve makes it a floor
    /// rather than a ceiling.
    ///
    /// An earlier version of this paragraph said "at most the bring-up budget,
    /// once", which named only the last of the three and understated the window
    /// by roughly an order of magnitude — while being the paragraph that exists
    /// specifically to bound it.
    #[must_use]
    pub fn is_live(&self, profile: &str) -> bool {
        self.handles
            .try_lock()
            .is_ok_and(|m| m.get(profile).is_some_and(|h| h.alive()))
    }

    /// Take the handle out of the map. The caller owns stopping it — used by
    /// `switch_engine` (Task 19), which must not have the old engine restarted
    /// under it while the new one comes up.
    pub async fn remove(&self, profile: &str) -> Option<Arc<EngineHandle>> {
        self.handles.lock().await.remove(profile)
    }

    /// Every live handle, for callers that sweep rather than address one.
    pub async fn all(&self) -> Vec<Arc<EngineHandle>> {
        self.handles.lock().await.values().cloned().collect()
    }

    /// Park a pre-built handle under `profile`, so a test can exercise a
    /// backend without launching a browser.
    ///
    /// A door around the launch chain — it skips the process launch, the CDP
    /// connect and `ready_gate` — so a test using it is asserting something
    /// about a driver, never about readiness. Same discipline as
    /// `ProfileManager::insert_test_child`: gated, and there is exactly one.
    ///
    /// Delegates to [`Self::replace`] rather than writing the map itself: two
    /// functions writing one map under two spellings of the same key is how a
    /// seam and production drift apart (判据 §1), and
    /// `an_inserted_handle_is_the_one_handle_hands_back` pins that they agree.
    /// The displaced handle is dropped here because a test's fake process has
    /// nothing to leak; production callers use `replace` and must not.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn insert_for_test(&self, profile: &str, handle: Arc<EngineHandle>) {
        let _displaced = self.replace(profile, handle).await;
    }

    /// Drain the map and stop everything, returning how many actually died.
    ///
    /// The count is `died`, not `attempted`: reporting a stopped browser over
    /// one that is still running is the "success reported for a no-op" shape
    /// (判据 §11), and this number is what the daemon logs on its way out.
    ///
    /// Drained BEFORE stopping, so a caller that times this out cannot leave
    /// half-killed handles in the map for the next call to hand out.
    pub async fn shutdown_all(&self) -> usize {
        // The deadline starts HERE, before the lock, not after it.
        //
        // [`Self::handle`] holds this same lock across `process.launch()` —
        // `chromium::DEVTOOLS_PORT_DEADLINE` (30 s) plus an unbounded binary
        // resolve — plus `bring_up` (`READY_GATE_BUDGET`) plus a dead handle's
        // `shutdown()`. Starting the clock after the acquisition made the
        // declared budget describe only the kills: measured at 1.856 s and
        // 2.856 s in two runs against a stated 1 s. A limit's POSITION decides
        // what it limits (判据 §13), and the consumer here is the wedged-exit
        // failsafe, where `SHUTDOWN_FAILSAFE` is already spent, `exit(0)`
        // follows and nothing is behind it — so an overrun is not slow, it is
        // the browsers never being stopped at all.
        let deadline = tokio::time::Instant::now() + ENGINE_SHUTDOWN_BUDGET;

        // Declared before either timed section, so both of them can report what
        // actually died rather than losing it with the dropped future.
        let died = Arc::new(AtomicUsize::new(0));

        let handles: Vec<Arc<EngineHandle>> =
            match tokio::time::timeout_at(deadline, self.handles.lock()).await {
                Ok(mut map) => map.drain().map(|(_, handle)| handle).collect(),
                Err(_) => {
                    // Its OWN error, deliberately distinct from the one below:
                    // "I could not take the lock" is not "nothing died"
                    // (判据 §8). Nothing was drained, so every engine is still
                    // in the map and still running — which is a different
                    // sentence from "the stops ran and none of them worked",
                    // and the operator's next move differs between them.
                    tracing::error!(
                        budget_ms = ENGINE_SHUTDOWN_BUDGET.as_millis(),
                        "could not take the engine registry lock within the shutdown \
                         budget — a launch is in flight and holds it. NO engine was \
                         stopped; every one of them is left for the next boot sweep"
                    );
                    return 0;
                }
            };
        if handles.is_empty() {
            return 0;
        }
        // Named before they are moved into the futures below, so a budget
        // expiry can say WHICH processes it walked away from instead of
        // leaving a silent leak (判据 §17).
        let pids: Vec<u32> = handles.iter().map(|h| h.launched.pid).collect();

        // The count has to survive the budget. `tokio::time::timeout` DROPS the
        // future it was given, so a counter living inside that future is lost
        // exactly when it has the most to say: the run that stopped two
        // browsers and then hit the wall reported 0, and the caller believed
        // the number rather than the world (判据 §11).
        let stops = handles.into_iter().map(|handle| {
            let died = Arc::clone(&died);
            async move {
                if handle.shutdown().await {
                    died.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        // CONCURRENT, not sequential. Each engine's kill waits up to
        // `ENGINE_KILL_GRACE` on its own child and those windows overlap, so
        // the wall cost is one grace window whatever N is — which is the only
        // reason `ENGINE_SHUTDOWN_BUDGET` can be derived from a single grace.
        // Run in sequence the cost was N x grace, and three stuck engines could
        // not fit in any fixed budget. The stops touch nothing in common: one
        // process and one sidecar path each.
        if tokio::time::timeout_at(deadline, futures::future::join_all(stops))
            .await
            .is_err()
        {
            tracing::error!(
                ?pids,
                budget_ms = ENGINE_SHUTDOWN_BUDGET.as_millis(),
                stopped = died.load(Ordering::Relaxed),
                "the engine shutdown did not finish inside its budget; the engines \
                 among these pids that had not been stopped yet are left for the next \
                 boot sweep, and their sidecars with them"
            );
        }
        died.load(Ordering::Relaxed)
    }
}

/// Launch an engine, connect to it, open its first page and prove it is ready.
///
/// **A free function taking no `&self`, deliberately.** Both
/// [`EngineRegistry::handle`] and [`EngineRegistry::launch_detached`] call it,
/// and `handle` calls it while still holding the `handles` guard — so a launch
/// path that could reach `handles` would DEADLOCK rather than fail, which is
/// the worst failure mode available (a hang tells the reader nothing). Not
/// being handed `&self` makes that unreachable by construction rather than by
/// a comment nobody rereads.
///
/// One launch derivation, so `handle` and `launch_detached` cannot come to
/// disagree about what "ready" means.
async fn launch_engine(
    processes: &HashMap<Engine, Arc<dyn EngineProcess>>,
    engine: Engine,
    req: &LaunchRequest,
    command_timeout: Duration,
    ready_budget: Duration,
) -> Result<Arc<EngineHandle>, BrowserError> {
    let process = processes
        .get(&engine)
        .cloned()
        .ok_or_else(|| BrowserError::LaunchFailed {
            stage: "engine-process",
            detail: format!(
                "no launcher is registered for engine '{engine}'. Install it with \
                 runtime_manage{{action:\"install\", capability:\"{engine}\"}}, or set \
                 [general.browser] default_engine to an engine this build can start."
            ),
        })?;

    let launched = process.launch(req.clone()).await?;

    // ⚠️ A browser is running from here on, and only this function knows its
    // pid. Every failure below therefore goes through the reclaim arm rather
    // than through `?`: each of these errors tells the caller to RETRY, and a
    // retry that leaves the previous process behind turns one refusal into N
    // orphans — a message that is false about the world it describes
    // (判据 §15: the hand-off across an irreversible boundary is this
    // function's to complete, because after it returns nobody can).
    match bring_up(&launched, engine, command_timeout, ready_budget).await {
        Ok((conn, target, session)) => {
            let handle = Arc::new(EngineHandle::new(
                engine,
                req.profile.clone(),
                launched,
                conn,
                process,
                (target.0.clone(), session),
            ));
            tracing::info!(
                profile = %req.profile,
                engine = engine.as_str(),
                pid = handle.launched.pid,
                "engine launched and ready"
            );
            Ok(handle)
        }
        Err(e) => {
            let pid = launched.pid;
            let died = stop_launched(&*process, &launched, engine).await;
            tracing::warn!(
                profile = %req.profile,
                engine = engine.as_str(),
                pid,
                died,
                error = %e,
                "the engine came up but could not be driven; stopped it so the retry \
                 this error asks for does not add a second browser"
            );
            Err(e)
        }
    }
}

/// Connect to a launched engine, open its first page and prove it is ready.
///
/// **One deadline for the whole bring-up.** `tokio_tungstenite::connect_async`
/// has no timeout of its own, and this runs while [`EngineRegistry::handle`]
/// holds the `handles` guard — so an endpoint that accepts the TCP connection
/// and never completes the websocket handshake used to wedge EVERY profile for
/// the life of the process, with `is_live` answering `false` for all of them
/// throughout. Being stuck must not be indistinguishable from being absent, and
/// there has to be a way out (判据 §14 — that was fail-dead, not fail-closed).
///
/// Every step is bounded by what is LEFT of `budget` rather than by a budget of
/// its own, so there is exactly one number for "how long may bringing an engine
/// up take" and no two of them can add up to a total nobody chose. That is also
/// why `ready_gate` is handed the remainder instead of `budget`: given the whole
/// thing again, its own timeout could only fire after this one already had, and
/// an inner limit that can never be reached is not a limit (判据 §2).
///
/// These outer `timeout_at`s DROP the CDP futures they wrap, so
/// `call_with_timeout`'s own `Err(_elapsed)` arm — which is what removes the
/// `pending` entry — never runs. `EngineHandle::attach_tab` had to stop doing
/// that; here it is bounded rather than benign-by-assumption: every path out of
/// this function on an error drops `conn`, and the connection's `Drop` tears
/// the whole shared state down with it, so a leaked entry cannot outlive the
/// call. The connect itself has no inner timeout to defer to at all
/// (`connect_async` takes none), which is why the wrapper exists.
async fn bring_up(
    launched: &Launched,
    engine: Engine,
    command_timeout: Duration,
    budget: Duration,
) -> Result<
    (
        aleph_cdp::CdpConnection,
        aleph_cdp::TargetId,
        aleph_cdp::SessionId,
    ),
    BrowserError,
> {
    let deadline = tokio::time::Instant::now() + budget;
    let stalled = |step: &str| BrowserError::LaunchFailed {
        stage: "cdp-endpoint",
        detail: format!(
            "the {engine} process is running (pid {}) but {step} against its CDP \
             endpoint {} did not finish within {}s. The process has been stopped; \
             retry, or switch engines with browser_session{{action:\"switch_engine\"}}.",
            launched.pid,
            launched.endpoint.ws_url,
            budget.as_secs_f64()
        ),
    };

    let conn = tokio::time::timeout_at(
        deadline,
        aleph_cdp::CdpConnection::connect(
            &launched.endpoint.ws_url,
            aleph_cdp::ConnectOptions { command_timeout },
        ),
    )
    .await
    .map_err(|_| stalled("opening the websocket"))?
    .map_err(|e| BrowserError::LaunchFailed {
        stage: "cdp-endpoint",
        detail: format!(
            "the {engine} process is running (pid {}) but its CDP endpoint {} did \
             not accept a connection: {e}",
            launched.pid, launched.endpoint.ws_url
        ),
    })?;

    // One tab, created and attached before the gate: the gate navigates, and a
    // navigation needs a page.
    let target = tokio::time::timeout_at(
        deadline,
        aleph_cdp::methods::target::create_target(&conn, "about:blank"),
    )
    .await
    .map_err(|_| stalled("opening the first page"))?
    .map_err(|e| BrowserError::LaunchFailed {
        stage: "cdp-ready",
        detail: format!("the engine would not open its first page: {e}"),
    })?;
    let session = tokio::time::timeout_at(deadline, conn.attach(&target))
        .await
        .map_err(|_| stalled("attaching to the first page"))?
        .map_err(|e| BrowserError::LaunchFailed {
            stage: "cdp-ready",
            detail: format!("the engine would not attach to its first page: {e}"),
        })?;

    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    ready_gate(&conn, &session, remaining).await?;
    Ok((conn, target, session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::engine::ENGINE_KILL_GRACE;
    use crate::browser::profile::BrowserType;
    use crate::browser::testkit::{engine_peer, FakeEngineProcess, ScriptedKill};
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    fn request(profile: &str, data_dir: &std::path::Path) -> LaunchRequest {
        LaunchRequest {
            profile: profile.to_string(),
            session_key: profile.to_string(),
            data_dir: data_dir.to_path_buf(),
            headless: true,
            proxy: None,
            browser: BrowserType::default(),
            allow_private_network: false,
            stealth: false,
            extra_args: vec![],
        }
    }

    /// The refusal a call produced, or a panic naming what it should have
    /// refused.
    ///
    /// Not `Result::expect_err`: that needs the OK type to be `Debug`, and
    /// [`EngineHandle`] deliberately is not — it owns a live socket, a process
    /// handle and a session table, and a derived `Debug` on it would put all
    /// three into whatever log formatted it. The refusal is the only half these
    /// tests read, so only that half is required to be printable.
    fn refusal<T>(result: Result<T, BrowserError>, expected: &str) -> BrowserError {
        match result {
            Ok(_) => panic!("{expected}"),
            Err(e) => e,
        }
    }

    /// A registry whose only launcher is a fake Chromium.
    fn registry_with(engine: Engine, proc: &Arc<FakeEngineProcess>) -> EngineRegistry {
        let mut processes: HashMap<Engine, Arc<dyn EngineProcess>> = HashMap::new();
        processes.insert(engine, proc.clone());
        EngineRegistry::new(
            processes,
            Duration::from_millis(500),
            Duration::from_secs(2),
        )
    }

    /// Get-or-launch: the second call returns the SAME handle, and the
    /// launcher was asked exactly once.
    ///
    /// The count is the substance. Asserting only `Arc::ptr_eq` would stay
    /// green over an implementation that launches a second browser and throws
    /// it away — a leaked process, reported as a cache hit.
    #[tokio::test]
    async fn handle_is_get_or_launch_not_launch_every_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let req = request("default", dir.path());

        let first = registry
            .handle(Engine::Chromium, &req, EngineLaunch::Allow)
            .await
            .expect("the fake engine launches");
        let second = registry
            .handle(Engine::Chromium, &req, EngineLaunch::Allow)
            .await
            .expect("the second call is served from the map");

        assert!(Arc::ptr_eq(&first, &second), "two handles for one profile");
        assert_eq!(proc.launches().len(), 1, "the engine was launched twice");
        assert_eq!(first.engine, Engine::Chromium);
        assert!(first.alive(), "a live connection must read as alive");

        // `get` sees what `handle` stored, and under the same key.
        let got = registry
            .get("default")
            .await
            .expect("stored under the profile");
        assert!(Arc::ptr_eq(&first, &got));
        assert_eq!(registry.all().await.len(), 1);

        // The first target really was created and attached — `ensure_tab`
        // answers for it, and for nothing else.
        let tabs = first.tabs.lock().await;
        let tab_id = tabs
            .entries
            .keys()
            .next()
            .expect("one tab was created")
            .clone();
        drop(tabs);
        assert_eq!(first.ensure_tab(&tab_id).await.expect("known tab").0, "S1");
        assert!(first.ensure_tab("no-such-tab").await.is_err());

        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// [`EngineLaunch::Refuse`] with no handle is an ANSWER, not a launch.
    #[tokio::test]
    async fn handle_refuse_without_a_handle_is_no_session_and_launches_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let req = request("default", dir.path());

        let err = refusal(
            registry
                .handle(Engine::Chromium, &req, EngineLaunch::Refuse)
                .await,
            "Refuse must not open a browser",
        );
        assert!(
            matches!(err, BrowserError::NoSession(ref p) if p == "default"),
            "expected NoSession(default), got {err:?}"
        );
        assert!(proc.launches().is_empty(), "Refuse launched an engine");
        assert!(registry.get("default").await.is_none());
        server.shutdown().await;
    }

    /// A profile already running one engine must not be silently handed the
    /// other. Swapping engines under an open profile discards cookies, tabs
    /// and every live ref without anyone asking — that is `switch_engine`'s
    /// job (spec §5.3), which migrates state and returns a fresh page.
    #[tokio::test]
    async fn handle_refuses_when_running_engine_differs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let chromium = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let obscura = Arc::new(FakeEngineProcess::new(Engine::Obscura, &server, dir.path()));
        let mut processes: HashMap<Engine, Arc<dyn EngineProcess>> = HashMap::new();
        processes.insert(Engine::Chromium, chromium.clone());
        processes.insert(Engine::Obscura, obscura.clone());
        let registry = EngineRegistry::new(
            processes,
            Duration::from_millis(500),
            Duration::from_secs(2),
        );
        let req = request("default", dir.path());

        registry
            .handle(Engine::Chromium, &req, EngineLaunch::Allow)
            .await
            .expect("chromium launches");

        let err = refusal(
            registry
                .handle(Engine::Obscura, &req, EngineLaunch::Allow)
                .await,
            "a running chromium must not be swapped for an obscura",
        );
        let text = err.to_string();
        assert!(
            matches!(
                err,
                BrowserError::EngineMismatch { ref profile, running, requested }
                    if profile == "default"
                        && running == Engine::Chromium
                        && requested == Engine::Obscura
            ),
            "expected EngineMismatch, got {err:?}"
        );
        assert!(
            text.contains("chromium") && text.contains("obscura"),
            "{text}"
        );
        assert!(
            text.contains("switch_engine"),
            "a refusal must name the verb that reconciles it: {text}"
        );
        assert!(
            obscura.launches().is_empty(),
            "the refused engine must not have been started anyway"
        );

        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// A profile whose engine has no registered launcher must say which engine
    /// and which door opens it — never a bare "failed".
    ///
    /// ⚠️ Task 16 registers `ObscuraLauncher`; this test then goes red and must
    /// be flipped to assert a handle. That is the point: the gap is visible.
    #[tokio::test]
    async fn an_engine_without_a_launcher_names_itself_and_the_fix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let req = request("obs", dir.path());

        let err = refusal(
            registry
                .handle(Engine::Obscura, &req, EngineLaunch::Allow)
                .await,
            "no obscura launcher is registered yet",
        );
        let text = err.to_string();
        assert!(text.contains("obscura"), "must name the engine: {text}");
        // The WHOLE call, not just the tool name: `RuntimeManageArgs` is
        // `{ action, capability }` today, and a rename would otherwise leave
        // this green while the model is handed a call the schema rejects
        // (判据 §17, one layer weaker than a verb that never existed).
        assert!(
            text.contains(r#"runtime_manage{action:"install", capability:"obscura"}"#),
            "a fail-closed answer must name a door that can actually be opened: {text}"
        );
        server.shutdown().await;
    }

    /// `shutdown_all` must reclaim by EFFECT: the launcher was asked to kill
    /// that exact pid, the sidecar file is gone, and the map is empty after.
    #[tokio::test]
    async fn shutdown_all_stops_every_handle_and_removes_their_sidecars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);

        let handle = registry
            .handle(
                Engine::Chromium,
                &request("default", dir.path()),
                EngineLaunch::Allow,
            )
            .await
            .expect("launch");
        let sidecar = handle.launched.sidecar_path.clone();
        let pid = handle.launched.pid;
        assert!(sidecar.exists(), "precondition: the launch wrote a record");
        drop(handle);

        assert_eq!(registry.shutdown_all().await, 1);
        assert_eq!(proc.kills(), vec![pid], "the engine's pid was never killed");
        assert!(!sidecar.exists(), "the sidecar outlived the engine");
        assert!(
            registry.get("default").await.is_none(),
            "the map was not drained"
        );
        assert_eq!(
            registry.shutdown_all().await,
            0,
            "a second stop must find nothing, not re-report the first"
        );
        server.shutdown().await;
    }

    /// `is_live` is the synchronous liveness read `ProfileManager::session_active`
    /// needs, and every one of its four answers is asserted — including the two
    /// that are "I could not tell", spent as `false`.
    ///
    /// The dropped-socket case is the one that matters: a map entry whose
    /// connection has closed is a DEAD engine, and `.is_some()` would call it
    /// live. `FakeCdpServer::drop_socket` is observable rather than instant, so
    /// this waits on the connection's own `closed` watch instead of sleeping.
    #[tokio::test]
    async fn is_live_answers_synchronously_and_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);

        // 1. a live handle
        let handle = registry
            .handle(
                Engine::Chromium,
                &request("default", dir.path()),
                EngineLaunch::Allow,
            )
            .await
            .expect("launch");
        assert!(registry.is_live("default"), "a live engine reads as live");

        // 3. an unknown profile
        assert!(!registry.is_live("no-such-profile"));

        // 2. a parked handle whose peer went away. Parked under a SECOND name,
        // so the assertion cannot pass because the map is empty.
        registry.insert_for_test("dead", handle.clone()).await;
        assert!(registry.is_live("dead"), "precondition: it starts live");
        server.drop_socket();
        let mut closed = handle.conn.closed();
        tokio::time::timeout(Duration::from_secs(5), closed.wait_for(Option::is_some))
            .await
            .expect("the client notices the peer went away within 5s")
            .expect("the watch sender outlives the connection");
        assert!(
            !registry.is_live("dead"),
            "a map entry whose connection has closed is a DEAD engine, not a \
             live one — this is what `.is_some()` would get wrong"
        );
        assert!(
            !registry.is_live("default"),
            "the same connection backs both entries, so both are dead"
        );

        // 4. contention: a sync reader that cannot take the lock says "not
        // live" rather than blocking a runtime thread or guessing.
        let guard = registry.handles.lock().await;
        assert!(
            !registry.is_live("default"),
            "a contended try_lock must answer false, not block"
        );
        drop(guard);

        server.shutdown().await;
    }

    /// `attach_tab` creates the tab entry; `ensure_tab` only finds one. The
    /// split is the whole point — one name cannot be both fail directions —
    /// so both halves are asserted against the same handle.
    ///
    /// Idempotence is asserted by EFFECT, not by "it did not error": a second
    /// attach must not add a second entry and must not send a second
    /// `Target.attachToTarget`, because two sessions on one page means the
    /// second one's events arrive where nothing reads them.
    #[tokio::test]
    async fn attach_tab_creates_the_entry_that_ensure_tab_only_finds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let handle = registry
            .handle(
                Engine::Chromium,
                &request("default", dir.path()),
                EngineLaunch::Allow,
            )
            .await
            .expect("launch");

        let target = aleph_cdp::TargetId("T2".to_string());
        assert!(
            handle.ensure_tab("T2").await.is_err(),
            "precondition: a tab nobody attached is not in the table"
        );

        let session = handle.attach_tab(&target).await.expect("attach");
        assert_eq!(
            handle.ensure_tab("T2").await.expect("now it is known"),
            session,
            "attach_tab must store the session ensure_tab hands out"
        );

        // The three domains really were enabled — a tab whose domains were
        // never enabled looks exactly like a quiet one.
        //
        // ⚠️ This block is also the guard for a DUPLICATION (R89): `attach_tab`
        // spells these three method names itself, because
        // `aleph_cdp::methods::{page,runtime,network}::enable` hardcode the
        // connection's full `command_timeout` and cannot be handed a
        // remainder. Two copies of one fact are survivable exactly when a
        // guard fails on drift and the next reader can find it — this is that
        // guard, it asserts all THREE names (a guard covering two of three
        // reports green on the drift it misses), and it must not be deleted as
        // redundant with the loop it is checking.
        let asked: Vec<String> = server
            .received()
            .iter()
            .filter_map(|m| {
                m.get("method")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        for domain in ["Page.enable", "Runtime.enable", "Network.enable"] {
            assert!(
                asked.iter().any(|m| m == domain),
                "{domain} missing: {asked:?}"
            );
        }
        let attaches = asked
            .iter()
            .filter(|m| *m == "Target.attachToTarget")
            .count();

        // Idempotent: same session, no second entry, no second attach.
        assert_eq!(handle.attach_tab(&target).await.expect("again"), session);
        assert_eq!(
            server
                .received()
                .iter()
                .filter(|m| m.get("method").and_then(serde_json::Value::as_str)
                    == Some("Target.attachToTarget"))
                .count(),
            attaches,
            "a second attach_tab opened a second session on one page"
        );
        {
            let tabs = handle.tabs.lock().await;
            assert_eq!(tabs.entries.len(), 2, "the launch tab plus T2, and no more");
            assert_eq!(
                tabs.entries.get("T2").map(|e| e.url.as_str()),
                Some("about:blank"),
                "a freshly attached target starts where the browser put it, not \
                 at an empty string standing in for 'unknown'"
            );
        }

        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// M1's positive direction: an attach whose four round trips are all SLOW
    /// but all answer still succeeds, and leaves nothing pending behind it.
    ///
    /// The negative half — a stalled attach is refused, bounded — is what
    /// produced D2, so this is the other half of the same gate (判据 §14: ask
    /// both directions). Without it, "bounded" and "refuses anything that is
    /// not instant" are indistinguishable, and the second one would break every
    /// real attach against a loaded browser.
    ///
    /// Each call is delayed by ~0.2 x `command_timeout`, so four of them
    /// together sit at ~0.8 x — comfortably inside the single deadline and
    /// comfortably outside "fast". `pending_len()` afterwards is the D2
    /// assertion: the per-call budgets must have been spent by calls that
    /// answered, not by futures dropped from under the crate's cleanup arm.
    #[tokio::test]
    async fn a_slow_but_answering_attach_succeeds_and_leaves_nothing_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        // A generous command timeout so the delays below are a fraction of it
        // rather than a race against it.
        let mut processes: HashMap<Engine, Arc<dyn EngineProcess>> = HashMap::new();
        processes.insert(Engine::Chromium, proc.clone());
        let command_timeout = Duration::from_secs(2);
        let registry = EngineRegistry::new(processes, command_timeout, Duration::from_secs(5));
        let handle = registry
            .handle(
                Engine::Chromium,
                &request("default", dir.path()),
                EngineLaunch::Allow,
            )
            .await
            .expect("launch");

        // Only now make the peer slow, so the launch itself is unaffected and
        // this measures the attach alone.
        let slow = command_timeout / 5;
        for method in [
            "Target.attachToTarget",
            "Page.enable",
            "Runtime.enable",
            "Network.enable",
        ] {
            let answer = engine_peer(&serde_json::json!({ "method": method }));
            server.on(method, Responder::Delay(slow, Box::new(answer)));
        }

        let target = aleph_cdp::TargetId("T9".to_string());
        let started = std::time::Instant::now();
        let session = handle
            .attach_tab(&target)
            .await
            .expect("four slow-but-answering calls are inside one command_timeout");
        let elapsed = started.elapsed();

        assert!(
            elapsed >= slow * 4,
            "the peer was not actually slow, so this proves nothing about the \
             budget: {elapsed:?}"
        );
        assert!(
            elapsed < command_timeout,
            "four calls at a fifth of the budget each must fit inside it: {elapsed:?}"
        );
        assert_eq!(
            handle
                .ensure_tab("T9")
                .await
                .expect("the tab is in the table"),
            session
        );
        assert_eq!(
            handle.conn.pending_len(),
            0,
            "a call that ANSWERED left a pending entry behind — the per-call \
             budgets are being pre-empted by an outer timeout that drops the \
             future before the crate's own cleanup arm can run"
        );

        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// `launch_detached` brings an engine all the way up and parks NOTHING —
    /// which is what lets `switch_engine` have the target engine ready while
    /// the source is still serving the profile.
    ///
    /// "Ready" is asserted by effect, not by the absence of an error: the fake
    /// peer must have been asked to navigate, because a `launch_detached` that
    /// skipped the gate would return `Ok` just as happily.
    #[tokio::test]
    async fn launch_detached_does_not_park() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let req = request("default", dir.path());

        let detached = registry
            .launch_detached(Engine::Chromium, &req)
            .await
            .expect("it launches");
        assert!(detached.alive());
        assert_eq!(proc.launches().len(), 1);
        assert!(
            server
                .received()
                .iter()
                .any(|m| m.get("method").and_then(serde_json::Value::as_str)
                    == Some("Page.navigate")),
            "the readiness gate did not run"
        );

        assert!(
            registry.get("default").await.is_none(),
            "launch_detached parked the handle — switch_engine would then be \
             refused by its own target engine"
        );
        assert!(registry.all().await.is_empty());
        assert_eq!(
            registry.shutdown_all().await,
            0,
            "a detached handle is not the registry's to stop, and the count \
             must not claim it was"
        );

        // The caller owns it, so the caller stops it (this test is the caller).
        assert!(detached.shutdown().await);
        assert_eq!(proc.kills(), vec![detached.launched.pid]);
        server.shutdown().await;
    }

    /// `replace` is the other half of the switch: park the new engine and hand
    /// the old one back, in one lock acquisition, so nothing can observe the
    /// profile with no engine.
    ///
    /// The displaced handle comes back **still running** — that is the
    /// contract, and this test asserts it rather than assuming it, because a
    /// `replace` that stopped it would make `switch_engine`'s migration read
    /// from a dead browser.
    #[tokio::test]
    async fn replace_returns_the_previous_handle_and_parks_the_new_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let req = request("default", dir.path());

        let old = registry
            .handle(Engine::Chromium, &req, EngineLaunch::Allow)
            .await
            .expect("the source engine");
        let new = registry
            .launch_detached(Engine::Chromium, &req)
            .await
            .expect("the target engine");
        assert!(!Arc::ptr_eq(&old, &new), "two distinct engines");

        let displaced = registry
            .replace("default", new.clone())
            .await
            .expect("something was parked there");
        assert!(Arc::ptr_eq(&old, &displaced));
        assert!(
            displaced.alive(),
            "the displaced engine must still be running — switch_engine reads \
             cookies and tabs off it AFTER the swap"
        );
        assert!(Arc::ptr_eq(
            &new,
            &registry
                .get("default")
                .await
                .expect("the new one is parked")
        ));
        assert!(proc.kills().is_empty(), "replace must not stop anything");

        // An empty slot displaces nothing, and says so.
        assert!(registry.replace("fresh", new.clone()).await.is_none());

        assert!(
            displaced.shutdown().await,
            "the caller retires the old engine"
        );
        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// The parked handle must be the one `handle` hands back — same key, same
    /// map. A seam that stored under a different key would make every Part-4
    /// backend test green while production never found the handle, and nothing
    /// would connect the two ends (判据 §7).
    ///
    /// `EngineLaunch::Refuse` is the load-bearing half: it proves the handle
    /// was FOUND rather than launched, so the assertion cannot pass by
    /// accidentally starting a second engine.
    #[tokio::test]
    async fn an_inserted_handle_is_the_one_handle_hands_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let req = request("parked", dir.path());

        // Build one the ordinary way under a DIFFERENT profile, then park it
        // under "parked" — so the test cannot pass because both names happen
        // to be the same string.
        let built = registry
            .handle(
                Engine::Chromium,
                &request("origin", dir.path()),
                EngineLaunch::Allow,
            )
            .await
            .expect("launch");
        registry.insert_for_test("parked", built.clone()).await;

        let found = registry
            .handle(Engine::Chromium, &req, EngineLaunch::Refuse)
            .await
            .expect("a parked handle must be found, not refused");
        assert!(Arc::ptr_eq(&built, &found));
        assert!(Arc::ptr_eq(
            &built,
            &registry.get("parked").await.expect("get sees it too")
        ));
        assert_eq!(
            proc.launches().len(),
            1,
            "the parked handle was found, not re-launched"
        );
        assert_eq!(registry.all().await.len(), 2);

        registry.shutdown_all().await;
        server.shutdown().await;
    }

    // ---- fix round 1: the leak set ------------------------------------------

    /// F2. An engine that refused to die keeps its sidecar.
    ///
    /// The record is the only thing the boot sweep reads, so deleting it for a
    /// live process does not tidy anything — it makes that process unreapable
    /// for good. `shutdown`'s own `Ok(false)` message says "leaving it for the
    /// next boot sweep"; before this the very next line deleted what the sweep
    /// would have looked for.
    #[tokio::test]
    async fn a_refused_kill_keeps_the_sidecar_the_boot_sweep_needs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(
            FakeEngineProcess::new(Engine::Chromium, &server, dir.path())
                .with_kill_outcomes([ScriptedKill::Survived]),
        );
        let registry = registry_with(Engine::Chromium, &proc);
        let handle = registry
            .handle(
                Engine::Chromium,
                &request("default", dir.path()),
                EngineLaunch::Allow,
            )
            .await
            .expect("launch");
        let sidecar = handle.launched.sidecar_path.clone();
        let pid = handle.launched.pid;
        assert!(sidecar.exists(), "precondition: the launch wrote a record");
        drop(handle);

        assert_eq!(
            registry.shutdown_all().await,
            0,
            "a process that is still running must not be counted as stopped"
        );
        assert_eq!(proc.kills(), vec![pid], "it must still have been asked");
        assert!(
            sidecar.exists(),
            "the sidecar of a process that refused to die was deleted — the boot \
             sweep reads that record and nothing else, so the process is now \
             unreapable and the next launch of this profile will overwrite its slot"
        );
        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// F2, the other arm: a kill that could not even be attempted is also not
    /// a death, and must not cost the record either.
    ///
    /// Separate from the `Survived` case because they are different facts and
    /// `stop_launched` reaches the `remove_file` through two different branches
    /// — one test could only ever falsify one of them.
    #[tokio::test]
    async fn a_kill_that_failed_outright_keeps_the_sidecar_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(
            FakeEngineProcess::new(Engine::Chromium, &server, dir.path())
                .with_kill_outcomes([ScriptedKill::Failed("no such process".into())]),
        );
        let registry = registry_with(Engine::Chromium, &proc);
        let handle = registry
            .handle(
                Engine::Chromium,
                &request("default", dir.path()),
                EngineLaunch::Allow,
            )
            .await
            .expect("launch");
        let sidecar = handle.launched.sidecar_path.clone();
        drop(handle);

        assert_eq!(registry.shutdown_all().await, 0, "an Err is not a death");
        assert!(
            sidecar.exists(),
            "a kill that errored deleted the record anyway — 'I could not kill it' \
             is not 'it is gone'"
        );
        server.shutdown().await;
    }

    /// F3. A launch that comes up but cannot be driven must not leave the
    /// browser running.
    ///
    /// The error it returns tells the caller to retry, so the retry is what
    /// this asserts: two refusals must leave two dead browsers, not two live
    /// ones nobody holds a handle to.
    #[tokio::test]
    async fn a_bring_up_failure_stops_the_browser_it_started() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Answers the version probe and nothing else, so `ready_gate` refuses
        // after `launch` has already succeeded.
        let server = FakeCdpServer::start(|msg: &serde_json::Value| {
            match msg.get("method").and_then(serde_json::Value::as_str) {
                Some("Target.createTarget") => {
                    Responder::Reply(serde_json::json!({"targetId": "T1"}))
                }
                Some("Target.attachToTarget") => {
                    Responder::Reply(serde_json::json!({"sessionId": "S1"}))
                }
                Some("Browser.getVersion") => Responder::Reply(serde_json::json!({
                    "protocolVersion": "1.3", "product": "Fake/1.0",
                    "revision": "@fake", "userAgent": "fake", "jsVersion": "13"
                })),
                _ => Responder::Drop,
            }
        })
        .await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let req = request("default", dir.path());

        let err = refusal(
            registry
                .handle(Engine::Chromium, &req, EngineLaunch::Allow)
                .await,
            "the readiness gate must refuse this peer",
        );
        assert!(err.to_string().contains("cdp-ready"), "{err}");
        assert_eq!(proc.launches().len(), 1);
        assert_eq!(
            proc.kills(),
            vec![proc.pid()],
            "the refusal abandoned a running browser: its error tells the caller to \
             retry, and nothing else knows this pid"
        );
        assert!(
            registry.all().await.is_empty(),
            "a failed launch must not be parked"
        );

        // The retry the error asks for. It must cost one more browser started
        // and one more stopped — never a second orphan.
        let _ = registry
            .handle(Engine::Chromium, &req, EngineLaunch::Allow)
            .await;
        assert_eq!(proc.launches().len(), 2);
        assert_eq!(
            proc.kills(),
            vec![proc.pid(), proc.pid()],
            "the retry left the second browser running too"
        );
        server.shutdown().await;
    }

    /// F4. A closed socket means Aleph lost its grip, not that the browser
    /// died — so the relaunch must stop the old process before taking its slot.
    ///
    /// `write_sidecar_record` is keyed on the session key and documents itself
    /// as an overwrite, so a relaunch that skipped the kill would destroy the
    /// orphan's only record on its way past.
    #[tokio::test]
    async fn a_closed_socket_is_killed_before_the_relaunch_takes_its_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let req = request("default", dir.path());

        let first = registry
            .handle(Engine::Chromium, &req, EngineLaunch::Allow)
            .await
            .expect("launch");
        let first_sidecar = first.launched.sidecar_path.clone();
        let pid = first.launched.pid;

        server.drop_socket();
        let mut closed = first.conn.closed();
        tokio::time::timeout(Duration::from_secs(5), closed.wait_for(Option::is_some))
            .await
            .expect("the client notices the peer went away within 5s")
            .expect("the watch sender outlives the connection");
        drop(first);

        let second = registry
            .handle(Engine::Chromium, &req, EngineLaunch::Allow)
            .await
            .expect("the relaunch");
        assert_eq!(proc.launches().len(), 2, "precondition: it relaunched");
        assert_eq!(
            proc.kills(),
            vec![pid],
            "the orphan was discarded without a kill — its socket closed, which says \
             we lost our grip, not that the process exited, and EngineHandle has no Drop"
        );
        assert_eq!(
            second.launched.sidecar_path, first_sidecar,
            "precondition: the record is keyed on the session key, so the relaunch \
             writes over exactly the slot the orphan would have been reaped from"
        );

        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// F5. An observing call leaves a dead handle exactly where it found it.
    ///
    /// `EngineLaunch::Refuse` says so in its own doc — "Observing. A missing
    /// engine is an answer, not something to fix" — and the eviction used to
    /// run before the gate was ever read. A sensor must not change what it
    /// measures (判据 §4).
    #[tokio::test]
    async fn an_observing_call_does_not_evict_or_kill_a_dead_handle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let registry = registry_with(Engine::Chromium, &proc);
        let req = request("default", dir.path());

        let handle = registry
            .handle(Engine::Chromium, &req, EngineLaunch::Allow)
            .await
            .expect("launch");
        server.drop_socket();
        let mut closed = handle.conn.closed();
        tokio::time::timeout(Duration::from_secs(5), closed.wait_for(Option::is_some))
            .await
            .expect("the client notices the peer went away within 5s")
            .expect("the watch sender outlives the connection");
        drop(handle);

        let err = refusal(
            registry
                .handle(Engine::Chromium, &req, EngineLaunch::Refuse)
                .await,
            "a dead engine is not a live one, so Refuse must answer NoSession",
        );
        assert!(matches!(err, BrowserError::NoSession(ref p) if p == "default"));
        assert!(
            registry.get("default").await.is_some(),
            "an observing call evicted the handle it was only asked about"
        );
        assert!(
            proc.kills().is_empty(),
            "an observing call killed a browser"
        );
        assert_eq!(
            proc.launches().len(),
            1,
            "and it must not have launched one"
        );

        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// F5, second half: a live engine of the WRONG kind is a fact, and both
    /// gates get the same answer.
    ///
    /// Its own test because the mismatch arm sits before the gate on purpose —
    /// answering `NoSession` to an observer would be a lie it would then spend
    /// as a licence to launch a second browser (判据 §8).
    #[tokio::test]
    async fn an_observer_is_told_the_truth_about_a_profile_running_another_engine() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let chromium = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let obscura = Arc::new(FakeEngineProcess::new(Engine::Obscura, &server, dir.path()));
        let mut processes: HashMap<Engine, Arc<dyn EngineProcess>> = HashMap::new();
        processes.insert(Engine::Chromium, chromium.clone());
        processes.insert(Engine::Obscura, obscura.clone());
        let registry = EngineRegistry::new(
            processes,
            Duration::from_millis(500),
            Duration::from_secs(2),
        );
        let req = request("default", dir.path());

        registry
            .handle(Engine::Chromium, &req, EngineLaunch::Allow)
            .await
            .expect("chromium launches");

        let err = refusal(
            registry
                .handle(Engine::Obscura, &req, EngineLaunch::Refuse)
                .await,
            "an observer must be told which engine is running",
        );
        assert!(
            matches!(err, BrowserError::EngineMismatch { running, .. } if running == Engine::Chromium),
            "an observer was told NoSession about a profile with a live engine, which \
             it would spend as a licence to start a second one: {err:?}"
        );
        assert!(obscura.launches().is_empty());
        assert!(chromium.kills().is_empty(), "observing killed something");

        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// F6. An endpoint that accepts the connection and never finishes the
    /// handshake is bounded, not fatal.
    ///
    /// `tokio_tungstenite::connect_async` has no timeout, and this runs while
    /// `handle` holds the global map guard — so an unbounded wait here wedged
    /// EVERY profile for the life of the process, with `is_live` answering
    /// `false` for all of them throughout. Being stuck must be distinguishable
    /// from being absent and there must be a way out (判据 §14).
    ///
    /// Asserts the elapsed time, not just the error: a refusal that arrived
    /// after ten minutes would satisfy every other assertion here.
    #[tokio::test]
    async fn a_half_open_endpoint_is_bounded_and_the_browser_is_reclaimed() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A listener that accepts TCP and never speaks HTTP. `FakeCdpServer`
        // cannot model this: it always completes the upgrade.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral loopback port");
        let port = listener.local_addr().expect("local addr").port();
        let accepted = tokio::spawn(async move {
            // Hold every accepted socket open, answering nothing.
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let proc = Arc::new(FakeEngineProcess::pointing_at(
            Engine::Chromium,
            &format!("ws://127.0.0.1:{port}/devtools/browser/wedged"),
            dir.path(),
        ));
        let mut processes: HashMap<Engine, Arc<dyn EngineProcess>> = HashMap::new();
        processes.insert(Engine::Chromium, proc.clone());
        // The bring-up budget is the one number this is measured against.
        let budget = Duration::from_millis(300);
        let registry = EngineRegistry::new(processes, Duration::from_secs(30), budget);

        // The outer `timeout` is the assertion, not a convenience: without the
        // bound under test this call never returns, and a hanging test is
        // indistinguishable from a slow suite — the guard has to be able to go
        // RED, not to stop (判据 §2).
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            registry.handle(
                Engine::Chromium,
                &request("default", dir.path()),
                EngineLaunch::Allow,
            ),
        )
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the connect was never bounded: 5s against a {budget:?} bring-up \
                 budget, and it holds the registry's global map lock the whole time, \
                 so every other profile is wedged with it"
            )
        });
        let err = refusal(
            outcome,
            "a peer that never completes the handshake must not be waited on forever",
        );
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "the refusal arrived, but far outside the {budget:?} bring-up budget it \
             was supposed to be bounded by: {elapsed:?}"
        );
        assert!(
            err.to_string().contains("cdp-endpoint"),
            "the stage must name the step that stalled: {err}"
        );
        assert_eq!(
            proc.kills(),
            vec![proc.pid()],
            "the wedged engine was left running"
        );
        assert!(registry.get("default").await.is_none());

        accepted.abort();
    }

    /// F7. `shutdown_all` reports what actually died, even when the budget
    /// expires part-way.
    ///
    /// Three engines: two die at once, one overruns its own grace window by
    /// more than the whole budget. The count must be 2. Under the old shape —
    /// a sequential loop with the timeout wrapped around it in
    /// `shutdown_browsers` — the timeout dropped the future and took the count
    /// with it, so a run that really did stop two browsers reported 0
    /// (判据 §11: the number and the world disagree, and the number is what the
    /// caller believes).
    ///
    /// Which handle draws which outcome is not deterministic, because the stops
    /// run concurrently and share one scripted queue. The multiset is: two
    /// `Died`, one `Stalls`. The COUNT is therefore 2 whichever way they land.
    #[tokio::test]
    async fn shutdown_all_counts_what_died_even_when_the_budget_expires() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let proc = Arc::new(
            FakeEngineProcess::new(Engine::Chromium, &server, dir.path()).with_kill_outcomes([
                ScriptedKill::Died,
                ScriptedKill::Died,
                ScriptedKill::Stalls(ENGINE_SHUTDOWN_BUDGET * 4),
            ]),
        );
        let registry = registry_with(Engine::Chromium, &proc);
        for profile in ["a", "b", "c"] {
            registry
                .handle(
                    Engine::Chromium,
                    &request(profile, dir.path()),
                    EngineLaunch::Allow,
                )
                .await
                .unwrap_or_else(|e| panic!("launch {profile}: {e}"));
        }
        assert_eq!(registry.all().await.len(), 3);

        let started = std::time::Instant::now();
        let stopped = registry.shutdown_all().await;
        let elapsed = started.elapsed();

        assert_eq!(
            stopped, 2,
            "the budget expired and took the count with it: two browsers really were \
             stopped and the caller was told zero"
        );
        assert_eq!(proc.kills().len(), 3, "every engine must have been asked");
        assert!(
            elapsed < ENGINE_SHUTDOWN_BUDGET * 3,
            "the budget did not bound the run: {elapsed:?}"
        );
        server.shutdown().await;
    }

    /// F7. The stops run concurrently, so N engines cost ONE grace window, not
    /// N of them.
    ///
    /// This is the assumption `ENGINE_SHUTDOWN_BUDGET`'s derivation rests on,
    /// and it is the only thing that separates a concurrent `shutdown_all` from
    /// a sequential one: three engines that each take 400 ms to go down fit in
    /// the 1 s budget together and cannot fit end to end (3 x 400 ms = 1.2 s).
    /// Run in sequence the third one is still being killed when the budget
    /// expires, and the count comes back 2.
    #[tokio::test]
    async fn shutdown_all_stops_every_engine_inside_one_grace_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = FakeCdpServer::start(engine_peer).await;
        let slow = Duration::from_millis(400);
        assert!(
            slow < ENGINE_KILL_GRACE && slow * 3 > ENGINE_SHUTDOWN_BUDGET,
            "the fixture must be inside one grace window and outside three of them, \
             or this test is not about concurrency at all"
        );
        let proc = Arc::new(
            FakeEngineProcess::new(Engine::Chromium, &server, dir.path())
                .with_kill_outcomes([ScriptedKill::DiesAfter(slow)]),
        );
        let registry = registry_with(Engine::Chromium, &proc);
        for profile in ["a", "b", "c"] {
            registry
                .handle(
                    Engine::Chromium,
                    &request(profile, dir.path()),
                    EngineLaunch::Allow,
                )
                .await
                .unwrap_or_else(|e| panic!("launch {profile}: {e}"));
        }

        let started = std::time::Instant::now();
        let stopped = registry.shutdown_all().await;
        let elapsed = started.elapsed();

        assert_eq!(
            stopped, 3,
            "three engines that each die well inside the grace window did not all \
             get stopped — run end to end they cost 3 x {slow:?}, which no fixed \
             budget derived from ONE grace window can cover"
        );
        assert!(
            elapsed < ENGINE_SHUTDOWN_BUDGET,
            "the stops did not overlap: {elapsed:?} for three {slow:?} kills"
        );
        server.shutdown().await;
    }

    /// E1. `ENGINE_SHUTDOWN_BUDGET` bounds **taking the lock**, not just the
    /// kills.
    ///
    /// This is the guard D1 did not have. That defect — the deadline computed
    /// after `self.handles.lock().await` instead of before it — was HIGH, was
    /// introduced by a fix, and was found only because a reviewer built a
    /// throwaway probe: measured 1.856 s and 2.856 s against a declared 1 s,
    /// where the pre-range shape returned 1.0016 s. Nothing in the suite would
    /// have gone red if it moved back, and the next person to simplify
    /// `shutdown_all` would have had nothing telling them not to (判据 §2).
    ///
    /// The property is the POSITION of the deadline, not "shutdown is fast".
    /// So: a launch is parked in flight holding the lock for several times the
    /// budget, and `shutdown_all` must still come back inside it. The bound is
    /// written as a multiple of `ENGINE_SHUTDOWN_BUDGET` rather than a literal,
    /// so the test and the constant it guards derive from one place (判据 §12).
    ///
    /// Headroom, both directions: the fixed shape returns at ~1 x the budget
    /// and the threshold is 2 x, while the broken shape waits out the whole
    /// lock hold at 3 x. A loaded machine has to be off by 2 x before this
    /// misreports in either direction.
    #[tokio::test]
    async fn the_shutdown_budget_bounds_taking_the_lock_not_only_the_kills() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A listener that accepts TCP and never speaks HTTP, so the launch
        // parks inside `bring_up` — holding the `handles` guard the whole time.
        // The same shape `a_half_open_endpoint_is_bounded_and_the_browser_is_reclaimed`
        // uses; here the stall is the fixture rather than the subject.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral loopback port");
        let port = listener.local_addr().expect("local addr").port();
        let accepting = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let proc = Arc::new(FakeEngineProcess::pointing_at(
            Engine::Chromium,
            &format!("ws://127.0.0.1:{port}/devtools/browser/wedged"),
            dir.path(),
        ));
        let mut processes: HashMap<Engine, Arc<dyn EngineProcess>> = HashMap::new();
        processes.insert(Engine::Chromium, proc.clone());
        // How long the launch holds the lock. Three budgets: long enough that
        // an unbounded acquisition is unmistakable, short enough to keep the
        // test a few seconds.
        let lock_hold = ENGINE_SHUTDOWN_BUDGET * 3;
        let registry = Arc::new(EngineRegistry::new(
            processes,
            Duration::from_secs(30),
            lock_hold,
        ));

        let req = request("wedged", dir.path());
        let launcher = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move {
                let _ = registry
                    .handle(Engine::Chromium, &req, EngineLaunch::Allow)
                    .await;
            })
        };

        // Wait until the launch is genuinely in flight. `process.launch()` is
        // called from inside the guard, so a recorded launch means the lock is
        // held and will stay held until the bring-up deadline expires.
        let armed = tokio::time::timeout(Duration::from_secs(5), async {
            while proc.launches().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            armed.is_ok(),
            "the fixture never got a launch in flight, so this test would be \
             measuring an uncontended lock and could not fail"
        );

        let started = std::time::Instant::now();
        let stopped = registry.shutdown_all().await;
        let elapsed = started.elapsed();

        // Non-vacuity first: an UNCONTENDED lock would satisfy the upper bound
        // below and the count too, so without this the test could pass by the
        // fixture having quietly finished rather than by the deadline being in
        // the right place (判据 §2).
        assert!(
            elapsed >= ENGINE_SHUTDOWN_BUDGET,
            "the shutdown did not wait out its budget on the lock, which means \
             the lock was NOT held and this test measured nothing: {elapsed:?}"
        );
        assert!(
            elapsed < ENGINE_SHUTDOWN_BUDGET * 2,
            "the shutdown budget did not bound the LOCK acquisition: {elapsed:?} \
             against a {ENGINE_SHUTDOWN_BUDGET:?} budget, with a launch holding \
             the guard. The caller is the wedged-exit failsafe, where \
             SHUTDOWN_FAILSAFE is already spent and exit(0) follows — an overrun \
             there is the external SIGKILL landing first and the browsers never \
             being stopped at all"
        );
        assert_eq!(
            stopped, 0,
            "nothing was drained, so nothing can have died — a lock we could not \
             take must not be reported as engines that did not exist"
        );

        launcher.abort();
        let _ = launcher.await;
        accepting.abort();
    }

    /// E2. `bring_up`'s surviving connection carries nothing pending.
    ///
    /// `bring_up` keeps outer `timeout_at` wrappers, which DROP the CDP future
    /// and so skip `call_with_timeout`'s own `Err(_elapsed)` cleanup arm — the
    /// mechanism behind D2. The reason that is tolerable there rather than in
    /// `attach_tab` is a claim about `Drop`: every error path out of `bring_up`
    /// drops `conn`, so a leaked entry cannot outlive the call, and the only
    /// connection that SURVIVES is the one from a successful bring-up.
    ///
    /// That claim was argued and its sibling in `attach_tab` was measured
    /// (`a_slow_but_answering_attach_succeeds_and_leaves_nothing_pending`), and
    /// one of two sibling claims measured is what makes the argued one look
    /// checked (判据 §16). So this measures the half that can actually be
    /// observed: the connection that lives.
    ///
    /// Both halves, because the first is the premise the second rests on —
    /// an outer wrapper really does leak, which is exactly why the surviving
    /// connection having zero is worth asserting rather than assumed.
    #[tokio::test]
    async fn an_outer_timeout_leaks_a_pending_entry_and_bring_ups_connection_has_none() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Half 1 — the premise. Wrapping a CDP call in an OUTER timeout leaves
        // the entry behind, because the crate's cleanup lives in the arm the
        // drop skips. This is a property of the CALLER's wrapping, not of
        // `aleph-cdp` (whose own suite covers the inner arm), so it belongs
        // here.
        let slow = FakeCdpServer::start(|_: &serde_json::Value| {
            Responder::Delay(
                Duration::from_secs(2),
                Box::new(Responder::Reply(serde_json::json!({}))),
            )
        })
        .await;
        let conn = aleph_cdp::CdpConnection::connect(
            &slow.ws_url(),
            aleph_cdp::ConnectOptions {
                command_timeout: Duration::from_secs(30),
            },
        )
        .await
        .expect("connect");
        assert_eq!(conn.pending_len(), 0, "precondition");
        let outer = tokio::time::timeout(
            Duration::from_millis(100),
            conn.call(None, "Browser.getVersion", serde_json::json!({})),
        )
        .await;
        assert!(outer.is_err(), "the peer must not have answered that fast");
        assert_eq!(
            conn.pending_len(),
            1,
            "an outer timeout was expected to strand the entry — if it no longer \
             does, the reason attach_tab stopped using one has gone away and the \
             comment there is now wrong"
        );
        drop(conn);
        slow.shutdown().await;

        // Half 2 — the connection that survives a bring-up has nothing pending,
        // even when every step of that bring-up was slow enough to matter.
        let server = FakeCdpServer::start(engine_peer).await;
        let ready_budget = Duration::from_secs(5);
        let step = ready_budget / 10;
        for method in [
            "Browser.getVersion",
            "Target.createTarget",
            "Target.attachToTarget",
            "Page.navigate",
            "Runtime.evaluate",
        ] {
            let answer = engine_peer(&serde_json::json!({ "method": method }));
            server.on(method, Responder::Delay(step, Box::new(answer)));
        }
        let proc = Arc::new(FakeEngineProcess::new(
            Engine::Chromium,
            &server,
            dir.path(),
        ));
        let mut processes: HashMap<Engine, Arc<dyn EngineProcess>> = HashMap::new();
        processes.insert(Engine::Chromium, proc.clone());
        let registry = EngineRegistry::new(processes, Duration::from_secs(30), ready_budget);

        let started = std::time::Instant::now();
        let handle = registry
            .handle(
                Engine::Chromium,
                &request("default", dir.path()),
                EngineLaunch::Allow,
            )
            .await
            .expect("a slow but answering engine still comes up");
        let elapsed = started.elapsed();
        // The subject assertion FIRST, then the non-vacuity check. The other
        // order puts "was the peer actually slow" in front of the thing this
        // test exists to measure, and any mutation that shortens the bring-up
        // trips it before `pending_len` is ever read — the ordering corollary,
        // applied before it costs something rather than after.
        assert_eq!(
            handle.conn.pending_len(),
            0,
            "the connection a successful bring-up handed on is carrying stranded \
             entries — that is the one connection whose leak would outlive the \
             call, and it is what the Drop argument claims cannot happen"
        );
        assert!(
            elapsed >= step * 3,
            "the peer was not actually slow, so this proves nothing: {elapsed:?}"
        );

        registry.shutdown_all().await;
        server.shutdown().await;
    }

    /// F7's other half: the budget is a function of the per-item cost, not a
    /// number someone picked.
    ///
    /// A flat 1 s over an N x 500 ms **sequential** loop could not be met for
    /// N >= 2 in the worst case (2 x 500 ms = the whole budget, before the
    /// socket closes and the unlinks) and was arithmetically impossible for
    /// N >= 3. The loop is concurrent now, so the wall cost is one grace
    /// window; this pins that the budget still covers exactly that, with slack,
    /// and that neither number can be moved without the other.
    #[test]
    fn the_shutdown_budget_is_derived_from_one_kill_grace() {
        assert_eq!(
            ENGINE_SHUTDOWN_BUDGET,
            ENGINE_KILL_GRACE * 2,
            "the budget stopped being a function of the cost it has to cover"
        );
        assert!(
            ENGINE_SHUTDOWN_BUDGET > ENGINE_KILL_GRACE,
            "a budget that does not cover one grace window cannot stop one engine"
        );
    }

    /// Only the registry assembles a handle.
    ///
    /// `EngineHandle::new` is `pub` so `insert_for_test`'s callers can build
    /// what they park, which means visibility no longer holds the "one place
    /// launches" rule — this does. A SOURCE pin, because the thing being
    /// forbidden is a call site, and a second one would work perfectly at
    /// runtime while quietly skipping the ready gate.
    #[test]
    fn engine_handle_is_built_in_exactly_one_production_place() {
        use crate::utils::source_scan::{code_text, production_text, rust_sources_under};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = rust_sources_under(&root);
        assert!(sources.len() > 100, "the source walk scanned nothing");

        let mut sites: Vec<String> = Vec::new();
        for (rel, text) in sources {
            let code = code_text(&production_text(std::path::Path::new(&rel), &text));
            for _ in 0..code.matches("EngineHandle::new(").count() {
                sites.push(rel.clone());
            }
        }
        assert_eq!(
            sites,
            vec!["src/browser/engine/registry.rs".to_string()],
            "an engine handle is assembled somewhere other than \
             EngineRegistry::handle — that path skips the launch, the CDP \
             connect and ready_gate, and nothing at runtime would say so"
        );
    }
}
