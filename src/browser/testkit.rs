//! Test-only fake [`BrowserBackend`] — records every call, can be told to
//! fail at a chosen ordinal, and can be handed the page text it should answer
//! with, so tool-level sequencing code (`browser_exec`), the post-navigation
//! audit and the wait probe can all be tested without a live browser.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::types::{
    ActionTarget, CookieOp, EmulateOptions, HistoryNav, ScreenshotOpts, ScreenshotOutput,
    ScrollDirection, SnapshotOutput, TabId, WaitCondition,
};

/// Compact target rendering for the recorded call log (matches the formats
/// the batch tool's tests assert on, e.g. `click:Ref{ref_id:"e5"}`).
fn fmt_target(target: &ActionTarget) -> String {
    match target {
        ActionTarget::Ref { ref_id } => format!("Ref{{ref_id:\"{ref_id}\"}}"),
        ActionTarget::Coordinates { x, y } => format!("Coords{{x:{x},y:{y}}}"),
    }
}

/// Default `list_tabs` answer — a single clean public tab.
const DEFAULT_TABS_TEXT: &str = "1: https://example.com";

/// Default text of the error the `fail_at` call returns.
const DEFAULT_FAILURE_MESSAGE: &str = "boom";

/// A [`BrowserBackend`] that never touches a browser: each method appends a
/// `verb:detail` entry to [`Self::calls`] and returns a trivial `Ok`.
///
/// `fail_at` (1-based ordinal over recorded calls) makes that one call return
/// `Err(BrowserError::ActionFailed(…))` instead — the `browser_exec` abort test
/// drives its failure path through this. The text defaults to
/// [`DEFAULT_FAILURE_MESSAGE`]; [`Self::with_failure_message`] replaces it, so a
/// test can hand the tool layer an error shaped like the real thing — raw
/// playwright-cli stderr, credential-bearing and unbounded — rather than a tidy
/// token that every egress transform is a no-op on.
///
/// The `with_*` builders override what the fake *answers*; every one of them
/// defaults to the value the fake returned before it existed, so a test that
/// does not call them sees the historical behaviour unchanged.
///
/// `evaluate` returns the [`super::wait_probe::WAIT_PROBE_FOUND`] sentinel by
/// default so any code path that polls a wait condition through `evaluate`
/// resolves on the first probe. `wait_for` itself is overridden to record
/// `wait:…` and resolve immediately: the fake must not really sleep out a
/// [`WaitCondition::Time`] delay or tests pay wall-clock time for nothing.
/// (Tests that exercise the polling loop itself call
/// `wait_probe::poll_wait_for` directly and steer it with
/// [`Self::with_evaluate_responses`].)
pub(crate) struct FakeBackend {
    calls: Mutex<Vec<String>>,
    fail_at: Option<usize>,
    failure_message: String,
    /// What `list_tabs` answers, already parsed. Held as rows rather than
    /// text so the fake returns exactly what the trait promises; the builder
    /// still takes listing TEXT, because a test that is about a driver's
    /// rendering should be able to write that rendering.
    tabs: Vec<super::tab_registry::TabLine>,
    snapshot_text: String,
    screenshot_png: Vec<u8>,
    console_text: String,
    network_text: String,
    /// Queued `evaluate` answers. The last entry sticks once the queue is down
    /// to one, so a polling loop can be given a steady "absent" without
    /// guessing how many probes it will run.
    evaluate_responses: Mutex<VecDeque<String>>,
}

impl FakeBackend {
    pub(crate) fn new(fail_at: Option<usize>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_at,
            failure_message: DEFAULT_FAILURE_MESSAGE.to_string(),
            tabs: super::tab_registry::parse_tab_lines(DEFAULT_TABS_TEXT),
            snapshot_text: String::new(),
            screenshot_png: Vec::new(),
            console_text: String::new(),
            network_text: String::new(),
            evaluate_responses: Mutex::new(VecDeque::new()),
        }
    }

    /// What the `fail_at` call's [`BrowserError::ActionFailed`] carries
    /// (default: [`DEFAULT_FAILURE_MESSAGE`]).
    pub(crate) fn with_failure_message(mut self, message: impl Into<String>) -> Self {
        self.failure_message = message.into();
        self
    }

    /// What `list_tabs` answers, given as one driver's listing text
    /// (default: [`DEFAULT_TABS_TEXT`]). Parsed through the same function the
    /// real text backends use — a second parser here would put a different
    /// reading of a listing underneath every tool-layer test.
    pub(crate) fn with_tabs_text(mut self, text: impl Into<String>) -> Self {
        self.tabs = super::tab_registry::parse_tab_lines(&text.into());
        self
    }

    /// What `snapshot` puts in `snapshot_text` (default: empty).
    pub(crate) fn with_snapshot_text(mut self, text: impl Into<String>) -> Self {
        self.snapshot_text = text.into();
        self
    }

    /// What `screenshot` answers with (default: zero bytes).
    ///
    /// The bytes need not be a real PNG: `bound_screenshot_png` returns its
    /// input unchanged when decoding fails, so a caller can hand over a payload
    /// of a chosen *size* to drive the parts of the pipeline that care about
    /// size — the inline-image hoist's `> 256` base64 floor above all, which a
    /// zero-byte default silently sits below.
    pub(crate) fn with_screenshot_png(mut self, png: impl Into<Vec<u8>>) -> Self {
        self.screenshot_png = png.into();
        self
    }

    /// What `console_messages` answers with (default: empty).
    pub(crate) fn with_console_text(mut self, text: impl Into<String>) -> Self {
        self.console_text = text.into();
        self
    }

    /// What `network_log` answers with (default: empty).
    pub(crate) fn with_network_text(mut self, text: impl Into<String>) -> Self {
        self.network_text = text.into();
        self
    }

    /// Queue the `evaluate` answers, in order. Once one entry remains it is
    /// repeated for every further call; an empty queue falls back to the
    /// wait-probe "found" sentinel.
    pub(crate) fn with_evaluate_responses<I, S>(self, responses: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        {
            let mut q = self
                .evaluate_responses
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            q.clear();
            q.extend(responses.into_iter().map(Into::into));
        }
        self
    }

    /// Recorded calls, in order.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Append `entry` to the call log; the `fail_at`-th recorded call fails.
    fn record(&self, entry: String) -> Result<(), BrowserError> {
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        calls.push(entry);
        if self.fail_at == Some(calls.len()) {
            return Err(BrowserError::ActionFailed(self.failure_message.clone()));
        }
        Ok(())
    }

    /// Next queued `evaluate` answer (see [`Self::with_evaluate_responses`]).
    fn next_evaluate_response(&self) -> String {
        let mut q = self
            .evaluate_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match q.len() {
            0 => super::wait_probe::WAIT_PROBE_FOUND.to_string(),
            1 => q[0].clone(),
            _ => q.pop_front().unwrap_or_default(),
        }
    }
}

#[async_trait]
impl BrowserBackend for FakeBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        self.record(format!("open_tab:{url}"))?;
        Ok("1".into())
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        self.record(format!("close_tab:{tab_id}"))
    }

    async fn list_tabs(&self) -> Result<Vec<super::tab_registry::TabLine>, BrowserError> {
        self.record("list_tabs".into())?;
        Ok(self.tabs.clone())
    }

    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.record(format!("navigate:{tab_id}:{url}"))
    }

    async fn click(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.record(format!("click:{}", fmt_target(&target)))
    }

    async fn type_text(
        &self,
        _tab_id: &str,
        _target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        self.record(format!("type_text:{text}"))
    }

    async fn fill(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        self.record(format!("fill:{}:{value}", fmt_target(&target)))
    }

    async fn hover(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.record(format!("hover:{}", fmt_target(&target)))
    }

    async fn scroll(
        &self,
        _tab_id: &str,
        _target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        self.record(format!("scroll:{direction:?}"))
    }

    async fn screenshot(
        &self,
        _tab_id: &str,
        _opts: ScreenshotOpts,
    ) -> Result<ScreenshotOutput, BrowserError> {
        self.record("screenshot".into())?;
        Ok(ScreenshotOutput {
            png_bytes: self.screenshot_png.clone(),
        })
    }

    async fn snapshot(&self, _tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        self.record("snapshot".into())?;
        Ok(SnapshotOutput {
            ref_count: self
                .snapshot_text
                .matches(crate::browser::types::REF_TOKEN)
                .count(),
            snapshot_text: self.snapshot_text.clone(),
            page_url: Some("https://example.com".into()),
            page_title: Some("Example".into()),
            state_json: None,
        })
    }

    async fn evaluate(&self, _tab_id: &str, js: &str) -> Result<String, BrowserError> {
        self.record(format!("evaluate:{js}"))?;
        Ok(self.next_evaluate_response())
    }

    async fn select(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        self.record(format!("select:{}:{value}", fmt_target(&target)))
    }

    async fn history(&self, tab_id: &str, nav: HistoryNav) -> Result<(), BrowserError> {
        self.record(format!("history:{tab_id}:{nav:?}"))
    }

    async fn dblclick(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.record(format!("dblclick:{}", fmt_target(&target)))
    }

    async fn press_key(&self, _tab_id: &str, key: &str) -> Result<(), BrowserError> {
        self.record(format!("press_key:{key}"))
    }

    /// Records the condition and resolves immediately — the fake never polls
    /// and never sleeps out a `Time` delay (see the type-level doc).
    async fn wait_for(
        &self,
        _tab_id: &str,
        condition: &WaitCondition,
        _timeout_ms: u64,
    ) -> Result<bool, BrowserError> {
        self.record(format!("wait:{condition:?}"))?;
        Ok(true)
    }

    async fn console_messages(&self, _tab_id: &str) -> Result<String, BrowserError> {
        self.record("console_messages".into())?;
        Ok(self.console_text.clone())
    }

    async fn network_log(&self, _tab_id: &str) -> Result<String, BrowserError> {
        self.record("network_log".into())?;
        Ok(self.network_text.clone())
    }

    async fn pdf(&self, _tab_id: &str, output_path: &Path) -> Result<(), BrowserError> {
        self.record(format!("pdf:{}", output_path.display()))
    }

    async fn switch_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        self.record(format!("switch_tab:{tab_id}"))
    }

    async fn handle_dialog(
        &self,
        _tab_id: &str,
        action: &str,
        prompt_text: Option<&str>,
    ) -> Result<(), BrowserError> {
        self.record(format!(
            "handle_dialog:{action}:{}",
            prompt_text.unwrap_or("")
        ))
    }

    async fn drag(
        &self,
        _tab_id: &str,
        from: ActionTarget,
        to: ActionTarget,
    ) -> Result<(), BrowserError> {
        self.record(format!("drag:{}:{}", fmt_target(&from), fmt_target(&to)))
    }

    async fn upload(
        &self,
        _tab_id: &str,
        target: Option<ActionTarget>,
        paths: &[String],
    ) -> Result<(), BrowserError> {
        let target = target.as_ref().map_or_else(String::new, fmt_target);
        self.record(format!("upload:{target}:{}", paths.join(",")))
    }

    async fn resize(&self, _tab_id: &str, width: u32, height: u32) -> Result<(), BrowserError> {
        self.record(format!("resize:{width}x{height}"))
    }

    async fn emulate(&self, _tab_id: &str, opts: &EmulateOptions) -> Result<(), BrowserError> {
        self.record(format!("emulate:{opts:?}"))
    }

    async fn save_state(&self, path: &Path) -> Result<(), BrowserError> {
        self.record(format!("save_state:{}", path.display()))
    }

    async fn load_state(&self, path: &Path) -> Result<(), BrowserError> {
        self.record(format!("load_state:{}", path.display()))
    }

    async fn cookies(&self, op: &CookieOp) -> Result<String, BrowserError> {
        self.record(format!("cookies:{op:?}"))?;
        Ok(String::new())
    }

    /// Recorded as one call (like the MCP backend's native `fill_form`) rather
    /// than delegating to the trait's per-field loop, so a test can tell the
    /// batch verb apart from N individual fills.
    async fn fill_form(
        &self,
        _tab_id: &str,
        fields: &[(ActionTarget, String)],
    ) -> Result<usize, BrowserError> {
        self.record(format!("fill_form:{}", fields.len()))?;
        Ok(fields.len())
    }
}

/// A test-only [`EngineProcess`](super::engine::process::EngineProcess) that
/// "launches" nothing and points every
/// [`Launched`](super::engine::process::Launched) at an in-process
/// [`aleph_cdp::testkit::FakeCdpServer`].
///
/// It writes a real sidecar file, because the record on disk is half of what
/// `EngineHandle::shutdown` is asserted to undo — a fake that skipped it would
/// let the deletion rot unobserved (判据 §4: assert the effect arrived).
///
/// `pub(crate)`, like [`FakeBackend`] beside it. It was briefly `pub` for "an
/// integration test under `--features test-helpers` may need to build one" —
/// a consumer that is structurally impossible, because this module is
/// `pub(crate)` and `alephcore::browser::testkit` therefore does not resolve
/// from another crate at all. Measured: 35 hits across 4 files, all under
/// `src/`, none under the 174 files in `tests/`. Widening a visibility for a
/// consumer that cannot exist is the abstraction this repo cuts (R84, P5, R10).
pub(crate) struct FakeEngineProcess {
    engine: super::engine::Engine,
    ws_url: String,
    http_url: String,
    sidecar_dir: std::path::PathBuf,
    pid: u32,
    launches: Mutex<Vec<String>>,
    kills: Mutex<Vec<u32>>,
    /// Scripted answers for [`Self::kill`], in order; the LAST entry sticks
    /// once the queue is down to one (the shape [`FakeBackend`] uses for
    /// `evaluate`), so a test can say "this engine never dies" without
    /// counting calls. Empty means [`ScriptedKill::Died`].
    ///
    /// Without this the fake answered `Ok(true)` unconditionally, which made
    /// `EngineHandle::shutdown`'s `Ok(false)`/`Err` arms and
    /// `EngineRegistry::shutdown_all`'s "the count is *died*, not *attempted*"
    /// contract **unfalsifiable**: no test that could exist would reach them
    /// (判据 §2 — an instrument that can only report success is not an
    /// instrument). Three real leaks sat green underneath it.
    kill_outcomes: Mutex<VecDeque<ScriptedKill>>,
}

/// What a scripted [`FakeEngineProcess::kill`] answers.
///
/// Named `ScriptedKill` rather than `KillOutcome`, which is what it was called
/// when it landed: `crate::builtin_tools::process_registry::KillOutcome`
/// already exists and is a PRODUCTION type about killing bash jobs. Two
/// unrelated things answering to one name makes a grep for it stop identifying
/// anything (判据 §6 — the census you run to answer "how many of these are
/// there" quietly counts both).
///
/// Its own enum rather than a queue of `Result<bool, BrowserError>` because
/// `BrowserError` is not `Clone` and the last entry has to stick, and because
/// "still running" and "could not even signal it" are different facts the
/// caller is supposed to treat differently.
#[derive(Clone, Debug)]
pub(crate) enum ScriptedKill {
    /// Signalled **and reaped** — `terminate`'s `Ok(true)`.
    Died,
    /// Still there when the grace window closed — `terminate`'s `Ok(false)`.
    /// Answers immediately: a launcher that reports this has already spent its
    /// own grace, and making every test pay 500 ms to observe that would buy
    /// nothing.
    Survived,
    /// Reaped, but only after `Duration` — a browser that takes a while to go
    /// down, within its grace window. The only way to tell a concurrent
    /// `shutdown_all` from a sequential one: N of these fit in one grace window
    /// together and do not fit end to end.
    DiesAfter(std::time::Duration),
    /// Still there, and the launcher **overran its own grace window** by
    /// `Duration` before saying so. The only way to reach
    /// `shutdown_all`'s budget, which is derived on the assumption that a
    /// launcher honours `grace`.
    Stalls(std::time::Duration),
    /// The kill could not be attempted at all — `EngineProcess::kill`'s `Err`.
    Failed(String),
}

/// A pid no test kills for real. Recorded and asserted on; never signalled.
const FAKE_ENGINE_PID: u32 = 424_242;

/// The scripted CDP peer a launched engine talks to: version, one target, one
/// session, a navigable page, and the three domains `attach_tab` enables.
///
/// **One copy, here, because both test modules that need it can reach this
/// one.** It lived twice — `engine::registry`'s tests and `manager`'s tests
/// each had their own — and two fakes that must agree about what a healthy
/// engine answers are two things free to disagree: the day one gains a method
/// the other does not, the two modules are quietly testing different browsers
/// (判据 §1). Measured before collapsing them: the two copies were
/// behaviourally identical, differing only by a comment, so the duplication had
/// not cost anything yet.
///
/// `readiness`'s own `healthy` is deliberately NOT folded in — it answers the
/// three methods the gate asks and nothing else, and its `version_only` sibling
/// is a mutilation of exactly that smaller thing. See its doc.
pub(crate) fn engine_peer(msg: &serde_json::Value) -> aleph_cdp::testkit::Responder {
    use aleph_cdp::testkit::Responder;
    match msg.get("method").and_then(serde_json::Value::as_str) {
        Some("Browser.getVersion") => Responder::Reply(serde_json::json!({
            "protocolVersion": "1.3", "product": "Fake/1.0",
            "revision": "@fake", "userAgent": "fake", "jsVersion": "13"
        })),
        Some("Target.createTarget") => Responder::Reply(serde_json::json!({"targetId": "T1"})),
        Some("Target.attachToTarget") => Responder::Reply(serde_json::json!({"sessionId": "S1"})),
        Some("Page.navigate") => {
            Responder::Reply(serde_json::json!({"frameId": "F1", "loaderId": "L1"}))
        }
        Some("Runtime.evaluate") => Responder::Reply(serde_json::json!({
            "result": {"type": "number", "value": 1}
        })),
        // `attach_tab` enables these three and refuses the tab if any of them
        // fails, so the peer has to answer them.
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

impl FakeEngineProcess {
    pub(crate) fn new(
        engine: super::engine::Engine,
        server: &aleph_cdp::testkit::FakeCdpServer,
        sidecar_dir: &Path,
    ) -> Self {
        Self {
            engine,
            ws_url: server.ws_url(),
            http_url: server.http_url(),
            sidecar_dir: sidecar_dir.to_path_buf(),
            pid: FAKE_ENGINE_PID,
            launches: Mutex::new(Vec::new()),
            kills: Mutex::new(Vec::new()),
            kill_outcomes: Mutex::new(VecDeque::new()),
        }
    }

    /// A fake pointing at a raw websocket URL rather than at a
    /// [`aleph_cdp::testkit::FakeCdpServer`].
    ///
    /// For the one thing a working fake peer cannot model: an endpoint that
    /// accepts the TCP connection and never completes the websocket
    /// handshake. `FakeCdpServer` always completes it, so without this the
    /// bring-up's connect bound has no way to be shown red.
    pub(crate) fn pointing_at(
        engine: super::engine::Engine,
        ws_url: &str,
        sidecar_dir: &Path,
    ) -> Self {
        Self {
            engine,
            ws_url: ws_url.to_string(),
            http_url: String::new(),
            sidecar_dir: sidecar_dir.to_path_buf(),
            pid: FAKE_ENGINE_PID,
            launches: Mutex::new(Vec::new()),
            kills: Mutex::new(Vec::new()),
            kill_outcomes: Mutex::new(VecDeque::new()),
        }
    }

    /// Script what [`Self::kill`] answers, in order; the last entry sticks.
    ///
    /// Takes `self` by value so it is applied before the fake goes behind an
    /// `Arc` — nothing can rewrite a running engine's death mid-shutdown.
    #[must_use]
    pub(crate) fn with_kill_outcomes(
        self,
        outcomes: impl IntoIterator<Item = ScriptedKill>,
    ) -> Self {
        {
            let mut q = self.kill_outcomes.lock().unwrap_or_else(|e| e.into_inner());
            q.clear();
            q.extend(outcomes);
        }
        self
    }

    /// The next scripted outcome, leaving the last entry in place.
    fn next_kill_outcome(&self) -> ScriptedKill {
        let mut q = self.kill_outcomes.lock().unwrap_or_else(|e| e.into_inner());
        match q.len() {
            0 => ScriptedKill::Died,
            1 => q[0].clone(),
            _ => q.pop_front().unwrap_or(ScriptedKill::Died),
        }
    }

    /// A fake with no server and no sidecar directory, for a handle that has
    /// no process at all — a test that only needs an
    /// `Arc<dyn EngineProcess>` to fill a field, never to launch anything.
    ///
    /// [`Self::launch`] REFUSES on one of these rather than handing back an
    /// endpoint that goes nowhere. A fake that answered `Ok` with an empty
    /// `ws_url` would fail later, at `CdpConnection::connect`, with a message
    /// about a URL nobody wrote.
    pub(crate) fn detached(engine: super::engine::Engine) -> Self {
        Self {
            engine,
            ws_url: String::new(),
            http_url: String::new(),
            sidecar_dir: std::path::PathBuf::new(),
            pid: FAKE_ENGINE_PID,
            launches: Mutex::new(Vec::new()),
            kills: Mutex::new(Vec::new()),
            kill_outcomes: Mutex::new(VecDeque::new()),
        }
    }

    /// One `"<profile>/<session_key> headless=<bool>"` line per launch.
    pub(crate) fn launches(&self) -> Vec<String> {
        self.launches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The pids `kill` was called with, in order.
    pub(crate) fn kills(&self) -> Vec<u32> {
        self.kills.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) const fn pid(&self) -> u32 {
        self.pid
    }
}

#[async_trait]
impl super::engine::process::EngineProcess for FakeEngineProcess {
    fn engine(&self) -> super::engine::Engine {
        self.engine
    }

    async fn launch(
        &self,
        req: super::engine::process::LaunchRequest,
    ) -> Result<super::engine::process::Launched, BrowserError> {
        if self.ws_url.is_empty() {
            return Err(BrowserError::LaunchFailed {
                stage: "engine-process",
                detail: "FakeEngineProcess::detached has no endpoint to launch \
                         against; use FakeEngineProcess::new with a FakeCdpServer"
                    .to_string(),
            });
        }
        self.launches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!(
                "{}/{} headless={}",
                req.profile, req.session_key, req.headless
            ));
        let sidecar_path = self.sidecar_dir.join(format!("{}.json", req.session_key));
        tokio::fs::create_dir_all(&self.sidecar_dir).await?;
        tokio::fs::write(&sidecar_path, b"{\"fake\":true}").await?;
        Ok(super::engine::process::Launched {
            pid: self.pid,
            endpoint: super::engine::process::CdpEndpoint {
                http_url: self.http_url.clone(),
                ws_url: self.ws_url.clone(),
                pid: self.pid,
            },
            sidecar_path,
            // No OS process behind this fake, so there is nothing to reap.
            // `None` from the start is the honest shape: `kill` below reports
            // what it *recorded*, not what it found in here.
            child: std::sync::Arc::new(crate::sync_primitives::Mutex::new(None)),
        })
    }

    /// Records the pid it was asked to stop and answers the next scripted
    /// [`ScriptedKill`] (default [`ScriptedKill::Died`]).
    ///
    /// Takes the whole `Launched` because the real launchers need the `Child`
    /// inside it to `wait()` after signalling; this fake has no OS process, so
    /// it reads only the pid — and reads it from the ARGUMENT rather than from
    /// `self.pid`, so a test that hands over the wrong `Launched` fails
    /// instead of quietly passing.
    ///
    /// The pid is recorded **before** the outcome is consulted: "we asked this
    /// pid to die" and "it died" are different facts, and a `kills()` that only
    /// listed the successful ones could not tell a refusal from a kill that
    /// never happened (判据 §4).
    async fn kill(
        &self,
        launched: &super::engine::process::Launched,
        _grace: std::time::Duration,
    ) -> Result<bool, BrowserError> {
        self.kills
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(launched.pid);
        match self.next_kill_outcome() {
            ScriptedKill::Died => Ok(true),
            ScriptedKill::Survived => Ok(false),
            ScriptedKill::DiesAfter(d) => {
                tokio::time::sleep(d).await;
                Ok(true)
            }
            ScriptedKill::Stalls(d) => {
                tokio::time::sleep(d).await;
                Ok(false)
            }
            ScriptedKill::Failed(reason) => Err(BrowserError::ActionFailed(reason)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `BrowserBackend` method must be answered by the fake.
    ///
    /// A method left on a trait default is a hole in the test double that the
    /// compiler cannot see: the tool layer calls it, gets the default's error
    /// (or the default's *implementation*), and the test observes something the
    /// real backends never do. Source-level because the check is "is it
    /// declared here", which no runtime reflection can answer in Rust.
    ///
    /// CRLF-safe: `\r` is stripped before any splitting and the split token is
    /// not anchored to a line boundary — on a Windows checkout an anchored
    /// `"\n…"` token matches nothing and the guard silently scans its own test
    /// module instead of the production code (CLAUDE.md §10).
    #[test]
    fn fake_backend_implements_every_backend_method() {
        let trait_src = include_str!("backend.rs").replace('\r', "");
        let fake_src = include_str!("testkit.rs").replace('\r', "");
        // Only the production half of testkit.rs counts — a name mentioned in
        // this very test must not satisfy the census.
        let fake_prod = crate::utils::source_scan::production_prefix(&fake_src);

        let methods: Vec<String> = trait_src
            .match_indices("async fn ")
            .filter_map(|(idx, _)| {
                let rest = &trait_src[idx + "async fn ".len()..];
                let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_')?;
                Some(rest[..end].to_string())
            })
            .collect();
        assert!(
            methods.len() > 20,
            "the trait scan found only {} methods — the extractor, not the trait, is broken",
            methods.len()
        );

        let missing: Vec<&String> = methods
            .iter()
            .filter(|m| !fake_prod.contains(&format!("async fn {m}(")))
            .collect();
        assert!(
            missing.is_empty(),
            "FakeBackend leaves these BrowserBackend methods on the trait default: {missing:?}"
        );
    }

    #[tokio::test]
    async fn builders_default_to_the_historical_answers() {
        let fake = FakeBackend::new(None);
        assert_eq!(
            fake.list_tabs().await.unwrap(),
            super::super::tab_registry::parse_tab_lines(DEFAULT_TABS_TEXT)
        );
        assert_eq!(fake.snapshot("1").await.unwrap().snapshot_text, "");
        assert_eq!(
            fake.evaluate("1", "() => 1").await.unwrap(),
            super::super::wait_probe::WAIT_PROBE_FOUND
        );
        // The reads that gained a builder later: their defaults are the empty
        // answers every pre-existing test was written against.
        assert!(fake
            .screenshot("1", ScreenshotOpts::default())
            .await
            .unwrap()
            .png_bytes
            .is_empty());
        assert_eq!(fake.console_messages("1").await.unwrap(), "");
        assert_eq!(fake.network_log("1").await.unwrap(), "");
        // The failure text is a builder too, so its default is pinned here with
        // the rest: the existing abort tests match on `boom`.
        let failing = FakeBackend::new(Some(1));
        let err = failing.click(
            "1",
            ActionTarget::Ref {
                ref_id: "e1".into(),
            },
        );
        assert!(err.await.unwrap_err().to_string().contains("boom"));
    }

    /// The fake answers ROWS, and `with_tabs_text` still takes the listing
    /// text every existing test hands it — the text is parsed once, at the
    /// seam, through the same function the real text backends use. A fake
    /// with its own parser would be a second answer to "what does this
    /// listing mean" (判据 §1) sitting under every tool-layer test.
    #[tokio::test]
    async fn the_fake_answers_rows_parsed_from_the_listing_it_was_given() {
        let fake =
            FakeBackend::new(None).with_tabs_text("1: https://a.com\n2: https://b.com [selected]");
        let rows = fake.list_tabs().await.expect("list_tabs");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].id, "2");
        assert!(rows[1].selected);
        assert_eq!(
            super::super::tab_registry::active_tab_id(&rows).as_deref(),
            Some("2")
        );
        assert_eq!(fake.calls(), vec!["list_tabs"]);
    }

    #[tokio::test]
    async fn the_failure_message_is_replaceable() {
        let fake = FakeBackend::new(Some(1)).with_failure_message("playwright: 401 for token X");
        let err = fake
            .click(
                "1",
                ActionTarget::Ref {
                    ref_id: "e1".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("401 for token X"));
    }

    #[tokio::test]
    async fn evaluate_queue_replays_then_sticks_on_its_last_entry() {
        let fake = FakeBackend::new(None).with_evaluate_responses(["first", "absent"]);
        assert_eq!(fake.evaluate("1", "p").await.unwrap(), "first");
        assert_eq!(fake.evaluate("1", "p").await.unwrap(), "absent");
        assert_eq!(fake.evaluate("1", "p").await.unwrap(), "absent");
    }
    /// The fake engine must hand back an endpoint a real `CdpConnection` can
    /// connect to, and must leave a sidecar file on disk — both are what
    /// `EngineHandle::shutdown` is later asserted to undo.
    #[tokio::test]
    async fn fake_engine_process_launches_against_the_fake_peer_and_writes_a_sidecar() {
        use crate::browser::engine::process::{EngineProcess, LaunchRequest};
        use crate::browser::engine::Engine;
        use crate::browser::profile::BrowserType;
        use aleph_cdp::testkit::{FakeCdpServer, Responder};

        let dir = tempfile::tempdir().expect("tempdir");
        let server =
            FakeCdpServer::start(|_: &serde_json::Value| Responder::Reply(serde_json::json!({})))
                .await;
        let proc = FakeEngineProcess::new(Engine::Chromium, &server, dir.path());
        assert_eq!(proc.engine(), Engine::Chromium);

        let launched = proc
            .launch(LaunchRequest {
                profile: "default".into(),
                session_key: "default".into(),
                data_dir: dir.path().join("udd"),
                headless: true,
                proxy: None,
                browser: BrowserType::default(),
                allow_private_network: false,
                stealth: false,
                extra_args: vec![],
            })
            .await
            .expect("the fake launch cannot fail");

        assert_eq!(launched.endpoint.ws_url, server.ws_url());
        assert_eq!(launched.pid, proc.pid());
        assert!(
            launched.sidecar_path.exists(),
            "a launch must leave a reapable record behind"
        );
        assert_eq!(
            proc.launches(),
            vec!["default/default headless=true".to_string()]
        );

        assert!(proc
            .kill(&launched, std::time::Duration::from_millis(1))
            .await
            .unwrap());
        assert_eq!(proc.kills(), vec![launched.pid]);
        server.shutdown().await;
    }

    /// `detached` has no peer to point at, so it REFUSES rather than handing
    /// back an endpoint that goes nowhere — the failure would otherwise land
    /// at `CdpConnection::connect`, naming a URL nobody wrote.
    #[tokio::test]
    async fn a_detached_fake_engine_refuses_to_launch() {
        use crate::browser::engine::process::{EngineProcess, LaunchRequest};
        use crate::browser::engine::Engine;
        use crate::browser::profile::BrowserType;

        let proc = FakeEngineProcess::detached(Engine::Obscura);
        assert_eq!(proc.engine(), Engine::Obscura);
        // Not `expect_err`: that needs `Launched` to be `Debug`, and it
        // deliberately is not — it owns the process handle, and a derived
        // `Debug` would print it into whatever formatted the panic.
        let Err(err) = proc
            .launch(LaunchRequest {
                profile: "p".into(),
                session_key: "p".into(),
                data_dir: std::path::PathBuf::from("/nonexistent"),
                headless: true,
                proxy: None,
                browser: BrowserType::default(),
                allow_private_network: false,
                stealth: false,
                extra_args: vec![],
            })
            .await
        else {
            panic!("a detached fake has nothing to launch against");
        };
        assert!(
            matches!(err, BrowserError::LaunchFailed { stage, .. } if stage == "engine-process"),
            "got {err:?}"
        );
        assert!(proc.launches().is_empty(), "a refusal must not be recorded");
    }
}
