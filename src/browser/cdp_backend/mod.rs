//! `CdpBackend` — ONE `BrowserBackend` for both engines, spoken over Aleph's
//! own CDP client (`crates/aleph-cdp`).
//!
//! The engine is branched on in exactly three places — the geometry fetch
//! (`snapshot`), JS dialogs (`dialog`, Task 13) and the capability refusals
//! derived from `engine::capability` — and nowhere else. Every verb is the same
//! sequence of CDP commands whichever engine is on the far end; a fourth branch
//! is the signal that a second backend is growing inside this one.
//!
//! # The `dead_code` permit below has a lifetime, and a test that ends it
//!
//! **Nothing in production constructs a `CdpBackend` yet.** `manager::get_backend`
//! has a `BrowserDriver::Cdp` arm, but that arm refuses by name; Task 14 replaces
//! it with the real construction. Until then every item this module defines is
//! reachable only from `#[cfg(test)]`, which the `lib` target does not compile —
//! so the repo's own gate, `cargo clippy --workspace --all-targets`, reports a
//! `dead_code` warning for each of them, about code that is tested and correct.
//! (**Measured**, not assumed: with the permit removed, that command names this
//! module in a warning block for every item, all of them emitted by the `(lib)`
//! target rather than `(lib test)`. `cargo clippy -p alephcore --lib` was NOT a
//! sufficient measurement — it does not build the test target at all, so it
//! could not tell "the gate sees these" from "only a `check`-shaped build does".)
//!
//! They are silenced in ONE place because a page of known warnings is how the
//! next real one becomes invisible (判据 §3). The cost is real and is the reason
//! this needs an expiry rather than a promise: a genuinely dead item added
//! INSIDE this module is not reported either, for as long as the permit stands.
//!
//! **The permit's stated reason expires the moment anything constructs a
//! `CdpBackend`, and `the_dead_code_permit_is_gone_once_production_constructs_this_backend`
//! goes red at exactly that moment.** A debt recorded only in a report is a debt
//! nobody collects: Task 13's `pump_started` obligation is self-executing (forget
//! it and the build fails), and this one is the opposite — forget to delete this
//! attribute and nothing happens, forever. So the guard is the thing that
//! remembers, not the reader.
//!
//! Visibility was NOT the fix. Making this module `pub` would also silence them
//! — `pub` items in a library are never dead-code-warned — but that widens the
//! crate's public API to work around a lint, and every other backend here is
//! `pub(crate)`.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use aleph_cdp::CdpError;

use super::backend::BrowserBackend;
use super::engine::process::LaunchRequest;
use super::engine::registry::EngineRegistry;
use super::engine::{
    capabilities, supported_by, Cap, Engine, EngineCapabilities, EngineHandle, EngineLaunch,
};
use super::error::BrowserError;
use super::network_policy::BrowserSsrfGuard;
use super::tab_registry::TabLine;
use super::types::{
    ActionTarget, CookieOp, EmulateOptions, HistoryNav, ScreenshotOpts, ScreenshotOutput,
    ScrollDirection, SnapshotOutput, TabId,
};

mod actions;
mod evaluate;
mod events;
mod navigate;
mod screenshot;
mod snapshot;
mod tabs;

pub struct CdpBackend {
    registry: Arc<EngineRegistry>,
    engine: Engine,
    /// How to launch this profile's engine, carried because the registry is
    /// shared across profiles and cannot know one profile's launch spec — the
    /// same reason `PlaywrightCliBackend` carries a `SessionLaunch`.
    ///
    /// Its `profile` is also **the** profile name this backend drives: the
    /// registry keys on it, so there is no second field holding the same string
    /// (see [`Self::new`]).
    req: LaunchRequest,
    ssrf_guard: Arc<BrowserSsrfGuard>,
    command_timeout: Duration,
}

impl CdpBackend {
    pub fn new(
        registry: Arc<EngineRegistry>,
        engine: Engine,
        req: LaunchRequest,
        profile: impl Into<String>,
        ssrf_guard: Arc<BrowserSsrfGuard>,
        command_timeout: Duration,
    ) -> Self {
        // ONE spelling of "which profile this backend drives". The registry
        // keys on `req.profile`, so a second `profile` field free to disagree
        // with it would make every verb resolve a profile the backend does not
        // claim to be on (判据 §1). The argument is the authority and the
        // request is corrected to match, rather than the two being carried side
        // by side for a later reader to pick between.
        //
        // ⚠️ `req.session_key` is NOT normalised, and that is a decision for
        // whoever writes the first production constructor rather than one to
        // make blind here. It is a second profile-shaped string — it names the
        // engine's on-disk sidecar (`engine::chromium`) — so a caller passing a
        // `profile` argument that disagrees with `req.session_key` gets a
        // backend whose registry key and whose sidecar name are different
        // strings, silently. Overwriting it here would be worse: the session
        // key is deliberately separable from the profile (that is what lets one
        // profile hold more than one launched session), so this constructor
        // does not get to decide they are the same thing. **Task 14 owns
        // this**: build the `LaunchRequest` and the profile name from one
        // source, or say in its own words why they differ.
        let mut req = req;
        req.profile = profile.into();
        Self {
            registry,
            engine,
            req,
            ssrf_guard,
            command_timeout,
        }
    }

    /// The profile's engine, which must ALREADY exist.
    ///
    /// Deliberately non-launching: 27 of the 28 verbs act on a page that must
    /// already be there, and letting any of them launch would make an observer
    /// create the browser it was checking on. Only [`Self::handle_launching`]
    /// may open one — the same split, for the same reason, as
    /// `PlaywrightCliBackend::run` / `run_launching`.
    pub(crate) async fn handle(&self) -> Result<Arc<EngineHandle>, BrowserError> {
        let handle = self
            .registry
            .handle(self.engine, &self.req, EngineLaunch::Refuse)
            .await?;
        events::ensure_pump(&handle);
        Ok(handle)
    }

    /// The profile's engine, launching one if there is none — i.e. the caller is
    /// saying "give me a browser".
    pub(crate) async fn handle_launching(&self) -> Result<Arc<EngineHandle>, BrowserError> {
        let handle = self
            .registry
            .handle(self.engine, &self.req, EngineLaunch::Allow)
            .await?;
        events::ensure_pump(&handle);
        Ok(handle)
    }

    pub(crate) const fn engine(&self) -> Engine {
        self.engine
    }

    pub(crate) fn guard(&self) -> &BrowserSsrfGuard {
        &self.ssrf_guard
    }

    pub(crate) const fn command_timeout(&self) -> Duration {
        self.command_timeout
    }
}

/// Three CDP failures, three different operator problems.
///
/// They are kept apart because the caller's next move differs: a timeout means
/// the engine is alive and busy (retry, or switch engines); a protocol error is
/// the engine's verdict about the page (fix the argument); a disconnect means
/// there is no engine any more (relaunch). Folding them into one arm is the
/// `match` that fans N classes into one value — the shape whose tell is
/// `#[allow(clippy::match_same_arms)]` (判据 §2).
pub(crate) fn map_cdp_err(engine: Engine, method: &str, err: CdpError) -> BrowserError {
    match err {
        CdpError::Timeout { waited, .. } => BrowserError::EngineBusy {
            engine,
            method: method.to_string(),
            waited_secs: waited.as_secs(),
        },
        CdpError::Protocol { code, message, .. } => BrowserError::Cdp {
            engine,
            method: method.to_string(),
            code,
            message,
        },
        CdpError::Disconnected(reason) => BrowserError::EngineFailure {
            engine,
            reason: format!(
                "the {} process stopped answering ({reason:?}); reopen it with \
                 `browser_open`, or move to the other engine with \
                 `browser_session{{action:\"switch_engine\"}}`",
                engine.as_str()
            ),
        },
        CdpError::Transport(detail) => BrowserError::EngineFailure {
            engine,
            reason: format!(
                "CDP transport failure talking to {}: {detail}; reopen it with \
                 `browser_open`",
                engine.as_str()
            ),
        },
        CdpError::Decode(detail) => BrowserError::Cdp {
            engine,
            method: method.to_string(),
            code: 0,
            message: format!("could not decode the engine's answer: {detail}"),
        },
    }
}

/// Turn a capability row into a refusal, or `Ok(())`.
///
/// **`caps` is an argument, never read off the static table in here** (ruling
/// R42). The table stays the authority — production hands this
/// `capabilities(engine)` and nothing else — but a branch whose input is a
/// parameter can be driven to both outcomes by a test, and a branch that reads
/// a `static` directly cannot. That matters right now rather than in theory:
/// with `file_upload` and `insert_text` `Supported` on both engines, `upload`'s
/// refusal and `type_text`'s per-character fallback are branches **nothing can
/// reach** through the public verb, and a guard that cannot go red is not a
/// guard (判据 §2 恒绿, §3).
///
/// `supported_by` deliberately consults the REAL table rather than `caps`: the
/// hint has to name an engine that genuinely supports the verb, so that a test
/// injecting a hypothetical row still gets a truthful "use this engine
/// instead" rather than a hint invented to match the fixture (判据 §14).
pub(crate) fn require(
    caps: &EngineCapabilities,
    engine: Engine,
    pick: fn(&EngineCapabilities) -> Cap,
    verb: &'static str,
) -> Result<(), BrowserError> {
    if pick(caps) == Cap::Supported {
        return Ok(());
    }
    Err(BrowserError::UnsupportedByEngine {
        engine,
        verb,
        supported_by: supported_by(pick),
    })
}

/// Placeholder for the verbs Task 13 fills in.
///
/// Unreachable in production: nothing constructs a `CdpBackend` until Task 14
/// adds the `get_backend` Cdp arm. Task 13 deletes this function and pins its
/// absence with a census test, so it cannot become a permanent "reports success
/// while doing nothing" (判据 §11) by outliving the task that owed the code.
fn not_yet_wired(verb: &'static str) -> BrowserError {
    BrowserError::ActionFailed(format!(
        "CDP_VERB_NOT_WIRED: {verb} lands in the next task; this backend is not \
         reachable from any profile yet"
    ))
}

#[async_trait]
impl BrowserBackend for CdpBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        tabs::open_tab(self, url).await
    }
    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        tabs::close_tab(self, tab_id).await
    }
    async fn list_tabs(&self) -> Result<Vec<TabLine>, BrowserError> {
        tabs::list_tabs(self).await
    }
    async fn switch_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        tabs::switch_tab(self, tab_id).await
    }
    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        navigate::navigate(self, tab_id, url).await
    }
    async fn history(&self, tab_id: &str, nav: HistoryNav) -> Result<(), BrowserError> {
        navigate::history(self, tab_id, nav).await
    }
    async fn snapshot(&self, tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        snapshot::snapshot(self, tab_id).await
    }
    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<String, BrowserError> {
        evaluate::evaluate(self, tab_id, js).await
    }
    async fn screenshot(
        &self,
        tab_id: &str,
        opts: ScreenshotOpts,
    ) -> Result<ScreenshotOutput, BrowserError> {
        screenshot::screenshot(self, tab_id, opts).await
    }
    async fn pdf(&self, tab_id: &str, output_path: &std::path::Path) -> Result<(), BrowserError> {
        screenshot::pdf(self, capabilities(self.engine), tab_id, output_path).await
    }
    async fn resize(&self, tab_id: &str, width: u32, height: u32) -> Result<(), BrowserError> {
        screenshot::resize(self, tab_id, width, height).await
    }
    async fn emulate(&self, tab_id: &str, opts: &EmulateOptions) -> Result<(), BrowserError> {
        screenshot::emulate(self, tab_id, opts).await
    }

    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        actions::click(self, tab_id, target).await
    }
    async fn dblclick(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        actions::dblclick(self, tab_id, target).await
    }
    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        actions::hover(self, tab_id, target).await
    }
    async fn type_text(
        &self,
        tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        actions::type_text(self, capabilities(self.engine), tab_id, target, text).await
    }
    async fn fill(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        actions::fill(self, tab_id, target, value).await
    }
    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        actions::select(self, tab_id, target, value).await
    }
    async fn press_key(&self, tab_id: &str, key: &str) -> Result<(), BrowserError> {
        actions::press_key(self, tab_id, key).await
    }
    async fn scroll(
        &self,
        tab_id: &str,
        target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        actions::scroll(self, tab_id, target, direction).await
    }
    async fn drag(
        &self,
        tab_id: &str,
        from: ActionTarget,
        to: ActionTarget,
    ) -> Result<(), BrowserError> {
        actions::drag(self, capabilities(self.engine), tab_id, from, to).await
    }
    async fn upload(
        &self,
        tab_id: &str,
        target: Option<ActionTarget>,
        paths: &[String],
    ) -> Result<(), BrowserError> {
        actions::upload(self, capabilities(self.engine), tab_id, target, paths).await
    }
    async fn handle_dialog(
        &self,
        _t: &str,
        _action: &str,
        _prompt: Option<&str>,
    ) -> Result<(), BrowserError> {
        Err(not_yet_wired("handle_dialog"))
    }
    async fn console_messages(&self, tab_id: &str) -> Result<String, BrowserError> {
        events::console_messages(self, tab_id).await
    }
    async fn network_log(&self, tab_id: &str) -> Result<String, BrowserError> {
        events::network_log(self, tab_id).await
    }
    async fn cookies(&self, _op: &CookieOp) -> Result<String, BrowserError> {
        Err(not_yet_wired("cookies"))
    }
    async fn save_state(&self, _path: &std::path::Path) -> Result<(), BrowserError> {
        Err(not_yet_wired("save_state"))
    }
    async fn load_state(&self, _path: &std::path::Path) -> Result<(), BrowserError> {
        Err(not_yet_wired("load_state"))
    }
    // `wait_for` and `fill_form` keep the trait's shared defaults
    // (`backend.rs`) — real shared implementations, not capability stubs, and
    // this backend's `evaluate` runs the probe.
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;
    use std::time::Duration;

    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use aleph_cdp::{CdpConnection, ConnectOptions};
    use serde_json::json;

    use crate::browser::engine::process::LaunchRequest;
    use crate::browser::engine::registry::EngineRegistry;
    use crate::browser::engine::{Engine, EngineHandle};
    use crate::browser::network_policy::{BrowserSsrfGuard, SsrfConfig};

    use super::CdpBackend;

    /// The command budget every test runs under. Short on purpose: the timeout
    /// arm is a test subject, not an accident, and the 30 s production default
    /// would make that test take 30 s.
    pub(crate) const TEST_TIMEOUT: Duration = Duration::from_millis(400);

    /// A `LaunchRequest` that names nothing real. Every test resolves through
    /// `insert_for_test`, so this never launches anything; it exists because
    /// `CdpBackend` carries one.
    pub(crate) fn dummy_launch(profile: &str) -> LaunchRequest {
        LaunchRequest {
            profile: profile.to_string(),
            session_key: profile.to_string(),
            data_dir: std::path::PathBuf::from("/nonexistent/aleph-qa"),
            headless: true,
            proxy: None,
            browser: crate::browser::profile::BrowserType::default(),
            allow_private_network: false,
            stealth: false,
            extra_args: Vec::new(),
        }
    }

    /// An SSRF guard with the product default policy (blocks loopback/private).
    pub(crate) fn default_guard() -> Arc<BrowserSsrfGuard> {
        Arc::new(BrowserSsrfGuard::new(SsrfConfig::default()))
    }

    /// An SSRF guard that blocks nothing — for the tests whose subject is not
    /// the policy.
    pub(crate) fn open_guard() -> Arc<BrowserSsrfGuard> {
        Arc::new(BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        }))
    }

    /// Register the responders every session needs: attach, the three `enable`s
    /// `attach_tab` issues, and the discovery subscription the event pump opens.
    ///
    /// `Target.setDiscoverTargets` arrives HERE, with its sender. Task 12 left
    /// it out on the rule that a responder for a method nothing sends can never
    /// fire, which is the same shape as a guard that can never go red
    /// (判据 §2); `events::ensure_pump` sends it as of Task 13, so it now has
    /// one. Registered explicitly rather than left to `scripted(vec![])`'s
    /// catch-all `Reply({})`, so this list stays a readable inventory of what a
    /// session actually puts on the wire.
    pub(crate) fn wire_session(server: &FakeCdpServer, session_id: &str) {
        let sid = session_id.to_string();
        server.on(
            "Target.attachToTarget",
            Responder::Reply(json!({ "sessionId": sid })),
        );
        for m in [
            "Page.enable",
            "Runtime.enable",
            "Network.enable",
            "Target.setDiscoverTargets",
        ] {
            server.on(m, Responder::Reply(json!({})));
        }
    }

    /// A backend whose engine handle is pre-seeded (no launch path is taken)
    /// and whose connection points at `server`.
    pub(crate) async fn backend_with(
        server: &FakeCdpServer,
        engine: Engine,
        guard: Arc<BrowserSsrfGuard>,
    ) -> (Arc<EngineRegistry>, CdpBackend) {
        let conn = CdpConnection::connect(
            &server.ws_url(),
            ConnectOptions {
                command_timeout: TEST_TIMEOUT,
            },
        )
        .await
        .expect("the fake server accepts a websocket");
        let handle = Arc::new(EngineHandle::for_test(engine, "default", conn));
        // No launcher is registered, so a test that accidentally took the
        // launching path would get a refusal rather than a browser — which is
        // the fail-closed direction for a unit test.
        let registry = Arc::new(EngineRegistry::new(
            std::collections::HashMap::new(),
            TEST_TIMEOUT,
            crate::browser::engine::readiness::READY_GATE_BUDGET,
        ));
        registry.insert_for_test("default", handle).await;
        let backend = CdpBackend::new(
            registry.clone(),
            engine,
            dummy_launch("default"),
            "default",
            guard,
            TEST_TIMEOUT,
        );
        (registry, backend)
    }

    /// A capability row with everything `Supported`, for a test to knock one
    /// field out of.
    ///
    /// Built here rather than cloned from `capabilities(engine)` on purpose: a
    /// fixture derived from the production table would change meaning the day
    /// that table does, and these tests are about the BRANCH, not about what
    /// the engines happen to support this month (R42).
    pub(crate) fn all_supported() -> crate::browser::engine::EngineCapabilities {
        use crate::browser::engine::{Cap, EngineCapabilities};
        EngineCapabilities {
            js_dialogs: Cap::Supported,
            drag: Cap::Supported,
            file_upload: Cap::Supported,
            pdf: Cap::Supported,
            insert_text: Cap::Supported,
            measured_on: "test fixture",
        }
    }

    /// Methods the fake was asked for, in order — the shape every wire
    /// assertion is written against.
    pub(crate) fn methods(server: &FakeCdpServer) -> Vec<String> {
        server
            .received()
            .iter()
            .filter_map(|m| m.get("method")?.as_str().map(str::to_string))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use aleph_cdp::{CdpError, CloseReason};
    use serde_json::json;

    use super::test_support::*;
    use crate::browser::backend::BrowserBackend;
    use crate::browser::engine::Engine;
    use crate::browser::error::BrowserError;
    use crate::browser::wait_probe::WAIT_PROBE_FOUND;

    /// Whether `src` carries a MODULE-level permit that includes `dead_code`.
    ///
    /// Deliberately broader than the one spelling in use: a permit rewritten as
    /// `#![allow(dead_code, unused)]` suppresses exactly as much, and a
    /// recogniser that only knew the current spelling would report the same
    /// green for "the permit is gone" and "the permit was reworded" (判据 §3).
    fn module_dead_code_permit(src: &str) -> bool {
        src.lines()
            .map(str::trim)
            .any(|l| l.starts_with("#![allow(") && l.contains("dead_code"))
    }

    /// Whether `code` brings a `CdpBackend` into existence.
    ///
    /// Three spellings, because one was not enough and the miss was in the
    /// expensive direction: a recogniser that fails to see a construction
    /// leaves the permit standing forever, silently. A factory
    /// `fn build(..) -> CdpBackend` called as `cdp_backend::build(..)` says
    /// `CdpBackend::new(` at neither end — the call site does not name the type
    /// at all — so the shape has to be recognised where the backend is produced.
    ///
    /// Like every name list this covers the day it was written (判据 §5), and
    /// its failure direction is the bad one, which is why the controls below
    /// carry a case per spelling rather than one case for today's code.
    fn constructs_the_backend(code: &str) -> bool {
        code.contains("CdpBackend::new(")
            || code.contains("-> CdpBackend")
            || code.contains("<CdpBackend")
    }

    /// The recogniser controls, and they are not optional here.
    ///
    /// `src/` contains no production construction today, so the guard below is
    /// asserting a NEGATIVE over a corpus with no positive case in it. Without
    /// these, a recogniser that silently stopped matching would look exactly
    /// like "Task 14 has not landed yet", and the permit would ship forever
    /// with the guard green — which is the same trap
    /// `the_roster_recogniser_knows_a_roster_from_an_accessor` exists for in
    /// `capability::census`.
    #[test]
    fn the_permit_and_construction_recognisers_match_what_they_claim_to() {
        for permit in [
            "#![allow(dead_code)]",
            "#![allow(dead_code, unused)]",
            "  #![allow(unused, dead_code)]  ",
        ] {
            assert!(
                module_dead_code_permit(permit),
                "a module-level permit this shape is not recognised, so the \
                 expiry guard would never fire while it stood: {permit:?}"
            );
        }
        for not_a_permit in [
            "#[allow(dead_code)]",
            "#![allow(unused_imports)]",
            "let s = \"#![allow(dead_code)]\";",
        ] {
            assert!(
                !module_dead_code_permit(&crate::utils::source_scan::code_text(not_a_permit)),
                "recognised as a module permit and it is not one: {not_a_permit:?}"
            );
        }

        for construction in [
            "Ok(Arc::new(CdpBackend::new(reg, engine)))",
            // The F-2 scenario, named: a factory INSIDE this module, called
            // from elsewhere by a name that never mentions the type. Neither
            // end says `CdpBackend::new(`, and the guard used to skip this
            // whole directory, so nothing saw it at either end.
            "pub(crate) fn build(m: &Manager) -> CdpBackend { todo!() }",
            "fn build(m: &Manager) -> Result<CdpBackend, BrowserError> { todo!() }",
            "fn parked() -> Option<Arc<CdpBackend>> { None }",
        ] {
            assert!(
                constructs_the_backend(construction),
                "a construction this shape is not recognised, so the permit \
                 would never expire for it: {construction:?}"
            );
        }
        assert!(
            !constructs_the_backend("pub struct CdpBackend {"),
            "the DECLARATION is not a construction, or the permit is red on \
             its own first run"
        );
        assert!(
            !constructs_the_backend(&crate::utils::source_scan::code_text(
                "// Task 14 replaces this arm with CdpBackend::new(...)"
            )),
            "a mention in a COMMENT must not count as a construction, or the \
             permit expires against prose"
        );
    }

    /// **The permit at the top of this file expires the moment anything in
    /// production constructs a `CdpBackend`.**
    ///
    /// The permit's stated reason is "nothing constructs one yet". That reason
    /// is checkable, so it is checked here rather than promised in a report:
    /// the day `manager::get_backend`'s `Cdp` arm builds a real backend, every
    /// warning the permit hides disappears on its own — and any that does not
    /// is a genuinely dead item the permit would go on hiding.
    ///
    /// ⚠️ Not written as "does the `Cdp` arm exist": it already does, refusing
    /// by name, so that trigger would be red on its first run. The trigger is
    /// the CONSTRUCTION, which is the thing the reason actually names.
    ///
    /// ⚠️ Not `#[expect(dead_code)]` either. The test build reaches every one
    /// of these items, so `dead_code` never fires there and the expectation
    /// would be unfulfilled today — the same reason `capability::census`
    /// records for not using it.
    ///
    /// `production_text`, not `production_prefix`: a whole-file test module
    /// carries no `#[cfg(test)]` of its own, and the per-file cut would hand
    /// this walk 100% of such a file as production.
    #[test]
    fn the_dead_code_permit_is_gone_once_production_constructs_this_backend() {
        use crate::utils::source_scan::{code_text, production_text, rust_sources_under};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = rust_sources_under(&root);
        assert!(
            sources.len() > 100,
            "the source walk found only {} files under src/ — the census \
             scanned nothing, which is not the same as finding nothing wrong",
            sources.len()
        );

        let me = std::fs::read_to_string(root.join("browser/cdp_backend/mod.rs"))
            .expect("this module's own source is readable");
        assert!(
            me.contains("pub struct CdpBackend"),
            "the guard is reading the wrong file — it found no CdpBackend \
             declaration where this module lives"
        );
        let permit = module_dead_code_permit(&me);

        // **No directory is skipped**, and the earlier version's skip of
        // `src/browser/cdp_backend/` is why this comment exists. It was
        // justified as "this module's own tests construct one on purpose" — but
        // `production_text` already removes every `#[cfg(test)]` item wherever
        // it sits in a file, which is what handles `mod test_support`. So the
        // skip bought nothing and cost the one scenario that most needs
        // catching: a factory living in this very directory, whose call site
        // never names the type (判据 §3 — the guard's green covered only the
        // shapes it recognised, and that was not one of them).
        let mut sites: Vec<String> = Vec::new();
        for (rel, text) in sources {
            if constructs_the_backend(&code_text(&production_text(
                std::path::Path::new(&rel),
                &text,
            ))) {
                sites.push(rel);
            }
        }

        assert!(
            sites.is_empty() || !permit,
            "production now constructs a CdpBackend:\n  {}\n\nbut \
             src/browser/cdp_backend/mod.rs still carries a module-level \
             `dead_code` permit. Its stated reason — \"nothing constructs one \
             yet\" — has expired. Delete the attribute: every warning it was \
             hiding disappears with the construction, and any that does not is \
             a genuinely dead item the permit would go on hiding.",
            sites.join("\n  ")
        );
    }

    /// The SSRF guard must be consulted BEFORE the wire, not after. "The
    /// navigation was refused" and "the navigation was refused after the
    /// browser already went there" are different facts, and only the byte count
    /// on the wire separates them.
    #[tokio::test]
    async fn a_blocked_navigation_produces_no_cdp_message_at_all() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Chromium, default_guard()).await;

        let err = backend
            .navigate("T1", "http://127.0.0.1:8080/secret")
            .await
            .expect_err("loopback is blocked by the default policy");
        assert!(
            matches!(err, BrowserError::NavigationFailed(_)),
            "got {err:?}"
        );
        assert!(
            methods(&server).is_empty(),
            "the guard must run before any CDP call; the fake saw {:?}",
            methods(&server)
        );
    }

    /// `backend.rs`: evaluate returns the VALUE. The probe's own source
    /// carries the sentinel, so a backend that echoed the script back would
    /// make every `wait_for` report "found" on its first poll.
    #[tokio::test]
    async fn evaluate_returns_the_value_and_never_the_script() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Runtime.evaluate",
            Responder::Reply(json!({ "result": { "type": "string", "value": "absent" } })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("pre-seeded handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach ok");

        let probe = crate::browser::wait_probe::wait_probe_func(
            &crate::browser::types::WaitCondition::Text("never on this page".into()),
        );
        assert!(
            probe.contains(WAIT_PROBE_FOUND),
            "precondition: the sentinel is a literal inside the probe"
        );

        let out = backend.evaluate("T1", &probe).await.expect("evaluate ok");
        assert_eq!(out, "\"absent\"", "the JSON of the value, nothing else");
        assert!(
            !out.contains(WAIT_PROBE_FOUND),
            "the returned text must not carry the echoed script: {out:?}"
        );
    }

    /// The whole tool layer speaks arrow functions (`wait_probe_func` builds
    /// `() => …`; `qa/browser_managed/drive_tools.py`'s `Page.js` sends
    /// `() => (expr)`). `Runtime.evaluate` on that source yields a FUNCTION,
    /// not the value — so the wrapper that calls it is load-bearing.
    #[tokio::test]
    async fn an_arrow_function_script_is_called_not_merely_evaluated() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Runtime.evaluate",
            Responder::Reply(json!({ "result": { "type": "number", "value": 7 } })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("pre-seeded handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach ok");

        let out = backend.evaluate("T1", "() => 3 + 4").await.expect("ok");
        assert_eq!(out, "7");

        let sent = server.received();
        let call = sent
            .iter()
            .find(|m| m.get("method").and_then(|v| v.as_str()) == Some("Runtime.evaluate"))
            .expect("the fake saw a Runtime.evaluate");
        let expr = call["params"]["expression"]
            .as_str()
            .expect("it carries an expression");
        assert!(expr.contains("() => 3 + 4"), "script must survive: {expr}");
        assert!(
            expr.contains("typeof") && expr.ends_with(")()"),
            "the script must be wrapped in a call, not evaluated bare: {expr}"
        );
        assert_eq!(
            call["params"]["returnByValue"].as_bool(),
            Some(true),
            "returnByValue is what makes the answer a value"
        );
        assert_eq!(
            call["params"]["awaitPromise"].as_bool(),
            Some(true),
            "an async script must answer with its value, not with a promise"
        );
    }

    /// Three CDP failures, three different operator problems. Folding them into
    /// one arm is the `match` that fans N classes into one value (判据 §2).
    #[test]
    fn cdp_errors_map_to_three_distinct_browser_errors() {
        use super::map_cdp_err;
        use std::time::Duration;

        let busy = map_cdp_err(
            Engine::Chromium,
            "Page.navigate",
            CdpError::Timeout {
                method: "Page.navigate".into(),
                waited: Duration::from_secs(30),
            },
        );
        assert!(
            matches!(
                busy,
                BrowserError::EngineBusy {
                    waited_secs: 30,
                    ..
                }
            ),
            "got {busy:?}"
        );

        let proto = map_cdp_err(
            Engine::Chromium,
            "DOM.resolveNode",
            CdpError::Protocol {
                method: "DOM.resolveNode".into(),
                code: -32000,
                message: "No node with given id found".into(),
                data: None,
            },
        );
        assert!(
            matches!(proto, BrowserError::Cdp { code: -32000, .. }),
            "got {proto:?}"
        );

        let gone = map_cdp_err(
            Engine::Obscura,
            "Page.enable",
            CdpError::Disconnected(CloseReason::PeerClosed),
        );
        match gone {
            BrowserError::EngineFailure { engine, ref reason } => {
                assert_eq!(engine, Engine::Obscura);
                assert!(
                    reason.contains("browser_open"),
                    "a fail-closed answer must name the door that reopens it: {reason}"
                );
            }
            other => panic!("expected EngineFailure, got {other:?}"),
        }
    }

    /// The one verb that may open a browser is `open_tab`; every other verb
    /// must refuse rather than launch one to answer a question about it.
    ///
    /// Written on the registry rather than on a launcher count, because the
    /// observable difference is which gate the backend passes: with no
    /// launcher registered, `EngineLaunch::Allow` is the only path that can
    /// reach a launch at all, and the refusal text differs between the two.
    #[tokio::test]
    async fn a_read_verb_never_brings_an_engine_into_existence() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (registry, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        // A profile with no handle at all — the case where "resolve the
        // engine" and "start one" part company.
        let _parked = registry.remove("default").await;

        let err = backend
            .snapshot("T1")
            .await
            .expect_err("a read verb has no engine to read");
        assert!(
            matches!(err, BrowserError::NoSession(_)),
            "an observer must answer 'there is none', not create one: {err:?}"
        );
        assert!(
            methods(&server).is_empty(),
            "nothing may reach the wire: {:?}",
            methods(&server)
        );
    }
}
