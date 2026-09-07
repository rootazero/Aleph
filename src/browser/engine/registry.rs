//! The one place an engine process is launched, cached and stopped.
//!
//! Held as an `Arc` by `ProfileManager` and by every CDP backend, because
//! `ProfileManager::get_backend` is SYNCHRONOUS (and the idle reaper calls it)
//! and so cannot resolve a handle at construction. A backend takes the
//! registry and resolves on its own first async call.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::process::{EngineProcess, LaunchRequest};
use super::readiness::ready_gate;
use super::{Engine, EngineHandle, EngineLaunch};
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
    /// A handle whose connection has closed is DROPPED here rather than
    /// returned: the engine died, and spec §5.2 says the next call restarts the
    /// same engine rather than switching. Under [`EngineLaunch::Refuse`] that
    /// next call is not this one, so it answers `NoSession` — the observer's
    /// answer, not a launch.
    pub async fn handle(
        &self,
        engine: Engine,
        req: &LaunchRequest,
        gate: EngineLaunch,
    ) -> Result<Arc<EngineHandle>, BrowserError> {
        let mut map = self.handles.lock().await;
        if let Some(existing) = map.get(&req.profile) {
            if !existing.alive() {
                tracing::warn!(
                    profile = %req.profile,
                    engine = existing.engine.as_str(),
                    "the engine's CDP connection is closed; discarding the handle"
                );
                map.remove(&req.profile);
            } else if existing.engine != engine {
                return Err(BrowserError::EngineMismatch {
                    profile: req.profile.clone(),
                    running: existing.engine,
                    requested: engine,
                });
            } else {
                return Ok(existing.clone());
            }
        }

        if gate == EngineLaunch::Refuse {
            return Err(BrowserError::NoSession(req.profile.clone()));
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
        let handles: Vec<Arc<EngineHandle>> = {
            let mut map = self.handles.lock().await;
            map.drain().map(|(_, handle)| handle).collect()
        };
        let mut stopped = 0;
        for handle in handles {
            if handle.shutdown().await {
                stopped += 1;
            }
        }
        stopped
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

    let conn = aleph_cdp::CdpConnection::connect(
        &launched.endpoint.ws_url,
        aleph_cdp::ConnectOptions { command_timeout },
    )
    .await
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
    let target = aleph_cdp::methods::target::create_target(&conn, "about:blank")
        .await
        .map_err(|e| BrowserError::LaunchFailed {
            stage: "cdp-ready",
            detail: format!("the engine would not open its first page: {e}"),
        })?;
    let session = conn
        .attach(&target)
        .await
        .map_err(|e| BrowserError::LaunchFailed {
            stage: "cdp-ready",
            detail: format!("the engine would not attach to its first page: {e}"),
        })?;
    ready_gate(&conn, &session, ready_budget).await?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserType;
    use crate::browser::testkit::FakeEngineProcess;
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    /// The scripted peer a launched engine talks to: version, one target, one
    /// session, a navigable page.
    fn engine_peer(msg: &serde_json::Value) -> Responder {
        match msg.get("method").and_then(serde_json::Value::as_str) {
            Some("Browser.getVersion") => Responder::Reply(serde_json::json!({
                "protocolVersion": "1.3", "product": "Fake/1.0",
                "revision": "@fake", "userAgent": "fake", "jsVersion": "13"
            })),
            Some("Target.createTarget") => Responder::Reply(serde_json::json!({"targetId": "T1"})),
            Some("Target.attachToTarget") => {
                Responder::Reply(serde_json::json!({"sessionId": "S1"}))
            }
            Some("Page.navigate") => {
                Responder::Reply(serde_json::json!({"frameId": "F1", "loaderId": "L1"}))
            }
            Some("Runtime.evaluate") => Responder::Reply(serde_json::json!({
                "result": {"type": "number", "value": 1}
            })),
            // `attach_tab` enables these three and refuses the tab if any of
            // them fails, so the peer has to answer them.
            Some("Page.enable" | "Runtime.enable" | "Network.enable") => {
                Responder::Reply(serde_json::json!({}))
            }
            Some(other) => Responder::Error {
                code: -32601,
                message: format!("fake peer does not implement {other}"),
            },
            None => Responder::Error {
                code: -32600,
                message: "not a request".into(),
            },
        }
    }

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
