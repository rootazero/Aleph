//! Companion lifecycle: start (launch -> endpoint proof -> real tab -> publish),
//! status/stop with caller ownership, and ONE close path shared by explicit
//! stop, natural exit, lifetime expiry, cancelled/failed start and daemon
//! shutdown.
//!
//! Terminal ordering, always: close the published resource (revokes viewer
//! grants, emits the close event, retires the owner route) -> close the tab ->
//! terminate the child and wait for its real exit. The stop is only reported
//! successful when every step that exists was confirmed:
//!
//! * a resource close that fails stops the sequence BEFORE the tab and the
//!   child are touched (killing the child would free its port while viewer
//!   grants may still name it) and leaves the state `StopFailed`; a later
//!   `stop`/watcher retry/`shutdown_all` re-runs the remaining steps;
//! * a failed or timed-out reap is an error and the state is `StopFailed`,
//!   never `Stopped`.
//!
//! Cancellation: the companion is registered (phase `Starting`) in the same
//! synchronous block that receives the launched child, and a manager-owned
//! watcher task is spawned there. `start` holds a [`StartGuard`]; dropping the
//! `start` future (a sync `Drop`, not an async one) only signals that watcher,
//! which runs the same `finish` path. So no await point of `start` can leave a
//! supervised child without an owner. The one thing this cannot recover is a
//! tab whose `open_tab` call was cancelled before it returned its id.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::Duration;

use aleph_protocol::browser_live::{DisplayPolicy, PreviewKind, RedactionStatus};
use async_trait::async_trait;
use tokio::sync::Notify;

use super::protocol::{CanonicalEndpoint, EndpointProver, LaunchCredentials, ProtocolError};
use super::record::ManagedLaunchRecord;
use crate::browser::cdp_backend::BrowserOperationRecord;
use crate::builtin_tools::bash_exec::session_label;
use crate::builtin_tools::browser_tools::present::{
    BrowserLiveProducer, BrowserPresentOperation, BrowserPresentation, PresentError,
    PublishedPreview,
};
use crate::gateway::visibility::{run_principal_sans_caller, RunPrincipal};
use crate::tools::turn_context::current_turn_context;

/// Live companions allowed at once.
pub(crate) const MAX_LIVE_COMPANIONS: usize = 4;
/// Stopped companions kept so `status` can still answer.
const MAX_TERMINAL_COMPANIONS: usize = 16;
/// Budget for the child to report its real exit after a kill request.
pub(crate) const DEFAULT_REAP_BUDGET: Duration = Duration::from_secs(10);
/// How many times the watcher retries a failed close before leaving the
/// companion `StopFailed` for `stop`/`shutdown_all` to retry.
const WATCHER_CLOSE_ATTEMPTS: usize = 3;
#[cfg(not(test))]
const WATCHER_RETRY_DELAY: Duration = Duration::from_secs(2);
#[cfg(test)]
const WATCHER_RETRY_DELAY: Duration = Duration::from_millis(5);

// ------------------------------------------------------------------ seams

/// A structured launch request: program + argv, never a shell line.
#[derive(Debug, Clone)]
pub(crate) struct LaunchRequest {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) endpoint: CanonicalEndpoint,
    pub(crate) ready_timeout: Duration,
    pub(crate) lifetime: Duration,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchError {
    #[error("managed launch is unsupported here: {0}")]
    Unsupported(String),
    #[error("launch refused: {0}")]
    Refused(String),
    #[error("no session in scope")]
    NoSession,
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("ready handshake failed: {0}")]
    Ready(ProtocolError),
    #[error("the companion did not report ready in time")]
    ReadyTimeout,
    #[error("launch failed and the child exit was NOT confirmed: {0}")]
    ReapFailed(String),
}

/// A launched child as the manager sees it.
#[async_trait]
pub(crate) trait CompanionProcess: Send + Sync {
    /// The PID the sandbox driver vouched for (never a wrapper PID).
    fn service_pid(&self) -> u32;
    fn is_alive(&self) -> bool;
    /// Resolves once the OS process has really exited.
    async fn exited(&self);
    /// Terminate and wait. `Ok` only after the real exit was observed.
    async fn stop_and_reap(&self, budget: Duration) -> Result<(), String>;
}

#[async_trait]
pub(crate) trait CompanionLauncher: Send + Sync {
    /// Side-effect free: can this host ever launch a managed child?
    fn preflight(&self) -> Result<(), LaunchError>;
    /// Spawn, deliver the launch secret, and verify the ready report. On any
    /// failure the child has already been killed and reaped (or the error says
    /// the reap was not confirmed). If the returned future is dropped before it
    /// completes, the implementation must still kill and reap the child.
    async fn launch(
        &self,
        req: &LaunchRequest,
        creds: &LaunchCredentials,
    ) -> Result<Arc<dyn CompanionProcess>, LaunchError>;
}

/// The browser tab a companion page is shown in.
#[async_trait]
pub(crate) trait CompanionTabs: Send + Sync {
    /// Real `open_tab_with_record`: the receipt names the created target.
    async fn open_tab(&self, url: &str) -> Result<(String, BrowserOperationRecord), String>;
    async fn close_tab(&self, tab_id: &str) -> Result<(), String>;
}

// --------------------------------------------------------------- identity

/// Who started a companion. All three must match for status/stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallerIdentity {
    session: String,
    run_id: String,
    principal: RunPrincipal,
}

impl CallerIdentity {
    pub(crate) fn capture() -> Option<Self> {
        let session = session_label()?;
        let turn = current_turn_context()?;
        if turn.run_id.trim().is_empty() {
            return None;
        }
        Some(Self {
            session,
            run_id: turn.run_id,
            principal: run_principal_sans_caller(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(session: &str, run_id: &str) -> Self {
        Self {
            session: session.into(),
            run_id: run_id.into(),
            principal: RunPrincipal::Legacy,
        }
    }
}

// ------------------------------------------------------------------ steps

/// Ordered record of terminal steps, so ordering is observable in tests.
#[derive(Default)]
pub(crate) struct StepLog(StdMutex<Vec<&'static str>>);

impl StepLog {
    pub(crate) fn push(&self, step: &'static str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(step);
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Vec<&'static str> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

// ----------------------------------------------------------------- errors

#[derive(Debug, thiserror::Error)]
pub(crate) enum StartError {
    #[error("browser live publication is not configured; nothing was launched")]
    NotConfigured,
    #[error("no managed launcher is available; nothing was launched")]
    NoLauncher,
    #[error("{0}")]
    Unsupported(String),
    #[error("at most {MAX_LIVE_COMPANIONS} companions may be live at once")]
    Capacity,
    #[error("the companion manager is shutting down; nothing was launched")]
    ShuttingDown,
    #[error("the companion was closed while it was starting")]
    ClosedDuringStart,
    #[error("launch failed: {0}")]
    Launch(LaunchError),
    #[error("endpoint ownership proof failed: {0}")]
    Proof(ProtocolError),
    #[error("could not open the page: {0}")]
    OpenTab(String),
    #[error("could not publish the page: {0}")]
    Publish(String),
    #[error("{cause}; rollback incomplete, closure NOT confirmed: {reap}")]
    RollbackIncomplete { cause: String, reap: String },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum LookupError {
    #[error("unknown companion")]
    NotFound,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum StopError {
    #[error("unknown companion")]
    NotFound,
    #[error("stop incomplete, closure was NOT confirmed (a later stop retries): {0}")]
    Failed(String),
}

// ------------------------------------------------------------------ state

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// Launched and registered; the page is not yet published.
    Starting,
    Running,
    Stopping,
    Stopped(&'static str),
    StopFailed(String),
}

/// What `finish` must release. Filled in as `start` creates each thing, so a
/// close at ANY point of the start closes exactly what exists by then.
#[derive(Default)]
struct Resources {
    preview: Option<PublishedPreview>,
    tab_id: Option<String>,
    resource_closed: bool,
    tab_closed: bool,
}

struct Companion {
    owner: CallerIdentity,
    endpoint: String,
    process: Arc<dyn CompanionProcess>,
    tabs: Arc<dyn CompanionTabs>,
    resources: StdMutex<Resources>,
    phase: StdMutex<Phase>,
    gate: tokio::sync::Mutex<()>,
    steps: Arc<StepLog>,
    reap_budget: Duration,
    /// Test fault injection: this many resource closes fail before the real one.
    #[cfg(test)]
    close_faults: Arc<std::sync::atomic::AtomicUsize>,
}

/// Why a start-time attachment was refused.
enum AttachError {
    /// `finish` already began: nothing new may be attached.
    Closed,
    Present(PresentError),
}

impl Companion {
    fn phase(&self) -> Phase {
        self.phase.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_phase(&self, phase: Phase) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = phase;
    }

    fn resources(&self) -> std::sync::MutexGuard<'_, Resources> {
        self.resources.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record the opened tab, unless the companion was closed meanwhile (the
    /// caller then closes that tab itself). Lock order: resources -> phase.
    fn attach_tab(&self, tab_id: String) -> Result<(), AttachError> {
        let mut res = self.resources();
        if self.phase() != Phase::Starting {
            return Err(AttachError::Closed);
        }
        res.tab_id = Some(tab_id);
        Ok(())
    }

    /// Publish and record the preview in one synchronous step under the
    /// resources lock, so `finish` either sees it or it is never created.
    fn attach_preview(
        &self,
        publish: impl FnOnce() -> Result<PublishedPreview, PresentError>,
    ) -> Result<(), AttachError> {
        let mut res = self.resources();
        if self.phase() != Phase::Starting {
            return Err(AttachError::Closed);
        }
        res.preview = Some(publish().map_err(AttachError::Present)?);
        Ok(())
    }

    /// `Starting -> Running`, only if nothing closed the companion meanwhile.
    fn promote(&self) -> bool {
        let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
        if *phase == Phase::Starting {
            *phase = Phase::Running;
            true
        } else {
            false
        }
    }

    /// Step 1. `Ok(true)` when a resource was closed now, `Ok(false)` when
    /// there is nothing (left) to close.
    fn close_resource(&self) -> Result<bool, String> {
        let mut res = self.resources();
        if res.resource_closed {
            return Ok(false);
        }
        let Some(preview) = res.preview.as_ref() else {
            return Ok(false);
        };
        #[cfg(test)]
        {
            use std::sync::atomic::Ordering;
            if self
                .close_faults
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err("injected resource close failure".to_owned());
            }
        }
        match preview.service_exited() {
            // `AlreadyClosed`: the registry already closed it (idempotent).
            Ok(_) | Err(PresentError::AlreadyClosed) => {
                res.resource_closed = true;
                Ok(true)
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

/// The single terminal path. Idempotent once `Stopped`; re-runnable after a
/// failure (each step records that it completed, so a retry resumes).
async fn finish(c: &Companion, reason: &'static str) -> Result<(), String> {
    let _gate = c.gate.lock().await;
    if matches!(c.phase(), Phase::Stopped(_)) {
        return Ok(());
    }
    // Before touching `resources`: attachments check for `Starting` under the
    // resources lock, so nothing can be added after this point.
    c.set_phase(Phase::Stopping);
    // 1. Close the resource: grants revoked, close event emitted (viewer
    //    sockets fenced), owner route retired. Done BEFORE the child dies, and
    //    a failure stops here: the tab and child are left alone so a retry
    //    still has everything it needs and the port is not freed under a
    //    possibly still-valid grant.
    match c.close_resource() {
        Ok(true) => c.steps.push("resource_closed"),
        Ok(false) => {}
        Err(error) => {
            c.steps.push("resource_close_failed");
            tracing::warn!(%error, "browser_companion resource close was not confirmed");
            let error = format!("resource close not confirmed: {error}");
            c.set_phase(Phase::StopFailed(error.clone()));
            return Err(error);
        }
    }
    // 2. Close the tab. A failure is retained, not swallowed: the tab is only
    //    marked closed on success, the child is still reaped below (so it is
    //    never leaked), and the terminal state is then `StopFailed` so a retry
    //    re-attempts the tab close.
    let tab = {
        let res = c.resources();
        if res.tab_closed {
            None
        } else {
            res.tab_id.clone()
        }
    };
    let mut tab_error: Option<String> = None;
    if let Some(tab_id) = tab {
        match c.tabs.close_tab(&tab_id).await {
            Ok(()) => {
                c.steps.push("tab_closed");
                c.resources().tab_closed = true;
            }
            Err(error) => {
                c.steps.push("tab_close_failed");
                tracing::warn!(%error, "browser_companion tab close was not confirmed");
                tab_error = Some(error);
            }
        }
    }
    // 3. Terminate the child and require its real exit (also after a failed
    //    tab close, so the child is not leaked).
    match c.process.stop_and_reap(c.reap_budget).await {
        Ok(()) => {
            c.steps.push("process_reaped");
            if let Some(error) = tab_error {
                let error = format!("tab close not confirmed: {error}");
                c.set_phase(Phase::StopFailed(error.clone()));
                return Err(error);
            }
            c.set_phase(Phase::Stopped(reason));
            Ok(())
        }
        Err(error) => {
            c.steps.push("reap_failed");
            let error = format!("child exit not confirmed: {error}");
            c.set_phase(Phase::StopFailed(error.clone()));
            Err(error)
        }
    }
}

/// The manager-owned supervisor of one companion: waits for the first of
/// natural exit / lifetime / start cancellation, then runs `finish`, retrying a
/// failed close a bounded number of times.
fn spawn_watcher(c: Arc<Companion>, lifetime: Duration, cancel: Arc<Notify>) {
    tokio::spawn(async move {
        let reason = tokio::select! {
            () = c.process.exited() => "exited",
            () = tokio::time::sleep(lifetime) => "lifetime",
            () = cancel.notified() => "start_cancelled",
        };
        for attempt in 0..WATCHER_CLOSE_ATTEMPTS {
            match finish(&c, reason).await {
                Ok(()) => return,
                Err(error) => {
                    tracing::warn!(%error, attempt, reason, "browser_companion close failed");
                    if attempt + 1 < WATCHER_CLOSE_ATTEMPTS {
                        tokio::time::sleep(WATCHER_RETRY_DELAY).await;
                    }
                }
            }
        }
        tracing::error!(
            reason,
            "browser_companion left StopFailed; stop/shutdown retries"
        );
    });
}

/// Held by `start` from the moment the child is registered. Dropping it
/// before `disarm` (error return OR the `start` future being dropped at an
/// await point) wakes the watcher, which closes everything. Sync `Drop`: it
/// only signals, the watcher task does the awaiting.
struct StartGuard {
    cancel: Arc<Notify>,
    armed: bool,
}

impl StartGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.notify_one();
        }
    }
}

// ---------------------------------------------------------------- manager

#[derive(Default)]
struct State {
    companions: HashMap<String, Arc<Companion>>,
    /// Slots taken by `start`s that have not yet registered their child.
    reserved: usize,
    /// Set by `shutdown_all`: no new start is admitted.
    closed: bool,
}

impl State {
    fn live(&self) -> usize {
        self.companions
            .values()
            .filter(|c| !matches!(c.phase(), Phase::Stopped(_)))
            .count()
            + self.reserved
    }
}

/// A capacity slot, taken atomically with the cap check and released when the
/// child is registered (`install`) or the start fails/cancels (`Drop`).
struct Reservation {
    manager: Arc<CompanionManager>,
    held: bool,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.held {
            let mut state = self.manager.state();
            state.reserved = state.reserved.saturating_sub(1);
        }
    }
}

pub(crate) struct CompanionManager {
    producer: Option<BrowserLiveProducer>,
    launcher: Option<Arc<dyn CompanionLauncher>>,
    prover: Arc<dyn EndpointProver>,
    steps: Arc<StepLog>,
    reap_budget: Duration,
    inner: StdMutex<State>,
    #[cfg(test)]
    close_faults: Arc<std::sync::atomic::AtomicUsize>,
}

/// Managers that must be drained on daemon shutdown. Weak: a dropped registry
/// never keeps a manager alive.
static MANAGERS: OnceLock<StdMutex<Vec<Weak<CompanionManager>>>> = OnceLock::new();

impl CompanionManager {
    pub(crate) fn new(
        producer: Option<BrowserLiveProducer>,
        launcher: Option<Arc<dyn CompanionLauncher>>,
        prover: Arc<dyn EndpointProver>,
    ) -> Arc<Self> {
        Self::with_steps(
            producer,
            launcher,
            prover,
            Arc::new(StepLog::default()),
            DEFAULT_REAP_BUDGET,
        )
    }

    /// As [`Self::new`] with an injected step log and reap budget (tests share
    /// the log with their fake process so cross-component order is visible).
    pub(crate) fn with_steps(
        producer: Option<BrowserLiveProducer>,
        launcher: Option<Arc<dyn CompanionLauncher>>,
        prover: Arc<dyn EndpointProver>,
        steps: Arc<StepLog>,
        reap_budget: Duration,
    ) -> Arc<Self> {
        let manager = Arc::new(Self {
            producer,
            launcher,
            prover,
            steps,
            reap_budget,
            inner: StdMutex::new(State::default()),
            #[cfg(test)]
            close_faults: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        MANAGERS
            .get_or_init(|| StdMutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::downgrade(&manager));
        manager
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Cheap checks that must pass before any approval card is raised.
    pub(crate) fn preflight(&self) -> Result<(), StartError> {
        if self.producer.is_none() {
            return Err(StartError::NotConfigured);
        }
        let launcher = self.launcher.as_ref().ok_or(StartError::NoLauncher)?;
        launcher.preflight().map_err(|e| match e {
            LaunchError::Unsupported(reason) => StartError::Unsupported(reason),
            other => StartError::Launch(other),
        })
    }

    #[cfg(test)]
    fn live_count(&self) -> usize {
        self.state().live()
    }

    /// Cap check and slot claim in ONE critical section, so concurrent starts
    /// cannot overshoot `MAX_LIVE_COMPANIONS`.
    fn reserve(self: &Arc<Self>) -> Result<Reservation, StartError> {
        let mut state = self.state();
        if state.closed {
            return Err(StartError::ShuttingDown);
        }
        if state.live() >= MAX_LIVE_COMPANIONS {
            return Err(StartError::Capacity);
        }
        state.reserved += 1;
        Ok(Reservation {
            manager: Arc::clone(self),
            held: true,
        })
    }

    /// Register the launched child (consuming the slot) in the same critical
    /// section. Returns `true` if the manager began shutting down meanwhile.
    fn install(&self, mut reservation: Reservation, id: &str, companion: &Arc<Companion>) -> bool {
        let mut state = self.state();
        state.reserved = state.reserved.saturating_sub(1);
        reservation.held = false;
        prune_terminal(&mut state.companions);
        state
            .companions
            .insert(id.to_owned(), Arc::clone(companion));
        state.closed
    }

    /// Undo a partial start through the one close path, then report the cause.
    /// On a confirmed close the entry is dropped; otherwise it stays
    /// `StopFailed` (and the armed guard makes the watcher retry).
    async fn abort_start(&self, id: &str, c: &Arc<Companion>, cause: StartError) -> StartError {
        match finish(c, "start_failed").await {
            Ok(()) => {
                self.state().companions.remove(id);
                cause
            }
            Err(reap) => StartError::RollbackIncomplete {
                cause: cause.to_string(),
                reap,
            },
        }
    }

    pub(crate) async fn start(
        self: &Arc<Self>,
        owner: CallerIdentity,
        req: LaunchRequest,
        tabs: Arc<dyn CompanionTabs>,
    ) -> Result<(String, String), StartError> {
        self.preflight()?;
        let producer = self.producer.as_ref().ok_or(StartError::NotConfigured)?;
        let launcher = self.launcher.as_ref().ok_or(StartError::NoLauncher)?;
        let reservation = self.reserve()?;

        let creds = LaunchCredentials::generate();
        let process = launcher
            .launch(&req, &creds)
            .await
            .map_err(StartError::Launch)?;

        // From here to the first await below there is NO await: the launched
        // child is registered, owned by a watcher and covered by the guard
        // atomically with respect to cancellation.
        let id = uuid::Uuid::new_v4().simple().to_string();
        let cancel = Arc::new(Notify::new());
        let companion = Arc::new(Companion {
            owner,
            endpoint: req.endpoint.as_string(),
            process: Arc::clone(&process),
            tabs: Arc::clone(&tabs),
            resources: StdMutex::new(Resources::default()),
            phase: StdMutex::new(Phase::Starting),
            gate: tokio::sync::Mutex::new(()),
            steps: Arc::clone(&self.steps),
            reap_budget: self.reap_budget,
            #[cfg(test)]
            close_faults: Arc::clone(&self.close_faults),
        });
        let shutting_down = self.install(reservation, &id, &companion);
        let mut guard = StartGuard {
            cancel: Arc::clone(&cancel),
            armed: true,
        };
        spawn_watcher(Arc::clone(&companion), req.lifetime, cancel);
        if shutting_down {
            return Err(self
                .abort_start(&id, &companion, StartError::ShuttingDown)
                .await);
        }

        // Endpoint ownership: a fresh challenge only the launch-secret holder
        // can answer, with the child required alive on both sides.
        let alive = {
            let process = Arc::clone(&process);
            move || process.is_alive()
        };
        let proof = match self.prover.prove(&creds, &req.endpoint, &alive).await {
            Ok(proof) => proof,
            Err(error) => {
                drop(creds);
                return Err(self
                    .abort_start(&id, &companion, StartError::Proof(error))
                    .await);
            }
        };
        drop(creds);

        // The page URL is the PROVEN endpoint, not whatever the caller typed.
        let endpoint = proof.endpoint().as_string();
        let (tab_id, record) = match tabs.open_tab(&endpoint).await {
            Ok(opened) => opened,
            Err(error) => {
                return Err(self
                    .abort_start(&id, &companion, StartError::OpenTab(error))
                    .await);
            }
        };
        if companion.attach_tab(tab_id.clone()).is_err() {
            // Closed while the tab was opening: `finish` never saw this tab.
            match tabs.close_tab(&tab_id).await {
                Ok(()) => self.steps.push("tab_closed"),
                Err(_) => self.steps.push("tab_close_failed"),
            }
            return Err(self
                .abort_start(&id, &companion, StartError::ClosedDuringStart)
                .await);
        }

        let launched_at_ms = chrono::Utc::now().timestamp_millis();
        let Some(launch) = ManagedLaunchRecord::new(process.service_pid(), launched_at_ms, proof)
        else {
            return Err(self
                .abort_start(
                    &id,
                    &companion,
                    StartError::Publish("launch record rejected".into()),
                )
                .await);
        };
        tracing::debug!(
            launch_id = launch.proof().launch_id(),
            endpoint = %launch.proof().endpoint().as_string(),
            service_pid = launch.service_pid(),
            "browser_companion endpoint proven; publishing"
        );
        let operation = match BrowserPresentOperation::from_managed_operation(&record, &launch) {
            Ok(operation) => operation,
            Err(error) => {
                return Err(self
                    .abort_start(&id, &companion, StartError::Publish(error.to_string()))
                    .await);
            }
        };
        let presentation = BrowserPresentation {
            kind: PreviewKind::DesignCompanion,
            display: DisplayPolicy::ExplicitOnly,
            // The producer clamps anything but Pending/Refused; never Safe.
            redaction: RedactionStatus::Pending,
            ttl: req.lifetime,
            companion_candidates: Vec::new(),
        };
        match companion.attach_preview(|| producer.browser_present(Some(operation), presentation)) {
            Ok(()) => {}
            Err(AttachError::Closed) => {
                return Err(self
                    .abort_start(&id, &companion, StartError::ClosedDuringStart)
                    .await);
            }
            Err(AttachError::Present(error)) => {
                return Err(self
                    .abort_start(&id, &companion, StartError::Publish(error.to_string()))
                    .await);
            }
        }
        if !companion.promote() {
            return Err(self
                .abort_start(&id, &companion, StartError::ClosedDuringStart)
                .await);
        }
        guard.disarm();
        Ok((id, endpoint))
    }

    fn lookup(&self, owner: &CallerIdentity, id: &str) -> Result<Arc<Companion>, LookupError> {
        let state = self.state();
        match state.companions.get(id) {
            // A foreign caller gets the same answer as an unknown id.
            Some(c) if &c.owner == owner => Ok(Arc::clone(c)),
            _ => Err(LookupError::NotFound),
        }
    }

    /// `(state, endpoint, process_alive)`.
    pub(crate) fn status(
        &self,
        owner: &CallerIdentity,
        id: &str,
    ) -> Result<(String, String, bool), LookupError> {
        let c = self.lookup(owner, id)?;
        let state = match c.phase() {
            Phase::Starting => "starting".to_owned(),
            Phase::Running if !c.process.is_alive() => "exited_closing".to_owned(),
            Phase::Running => "running".to_owned(),
            Phase::Stopping => "stopping".to_owned(),
            Phase::Stopped(reason) => format!("stopped:{reason}"),
            Phase::StopFailed(_) => "stop_failed".to_owned(),
        };
        Ok((state, c.endpoint.clone(), c.process.is_alive()))
    }

    pub(crate) async fn stop(&self, owner: &CallerIdentity, id: &str) -> Result<(), StopError> {
        let c = self.lookup(owner, id).map_err(|_| StopError::NotFound)?;
        finish(&c, "stopped").await.map_err(StopError::Failed)
    }

    /// Close every companion of every owner (daemon shutdown), including ones
    /// still `Starting`, and refuse new starts. Returns the number whose
    /// closure (resource and child exit) could NOT be confirmed.
    pub(crate) async fn shutdown_all(&self) -> usize {
        let all: Vec<Arc<Companion>> = {
            let mut state = self.state();
            state.closed = true;
            state.companions.values().cloned().collect()
        };
        let mut failed = 0;
        for c in all {
            if finish(&c, "shutdown").await.is_err() {
                failed += 1;
            }
        }
        failed
    }
}

fn prune_terminal(inner: &mut HashMap<String, Arc<Companion>>) {
    let stopped: Vec<String> = inner
        .iter()
        .filter(|(_, c)| matches!(c.phase(), Phase::Stopped(_)))
        .map(|(id, _)| id.clone())
        .collect();
    if stopped.len() > MAX_TERMINAL_COMPANIONS {
        for id in stopped.into_iter().take(inner.len()) {
            inner.remove(&id);
            if inner
                .values()
                .filter(|c| matches!(c.phase(), Phase::Stopped(_)))
                .count()
                <= MAX_TERMINAL_COMPANIONS
            {
                break;
            }
        }
    }
}

/// Close every companion of every live manager, before the generic background
/// reaper runs. Returns the number whose exit could not be confirmed.
pub async fn shutdown_all_companions() -> usize {
    let managers: Vec<Arc<CompanionManager>> = MANAGERS
        .get()
        .map(|m| {
            m.lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter_map(Weak::upgrade)
                .collect()
        })
        .unwrap_or_default();
    let mut failed = 0;
    for manager in managers {
        failed += manager.shutdown_all().await;
    }
    failed
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use aleph_protocol::browser_live::PreviewProvenance;
    use serde_json::json;
    use tokio::sync::{broadcast::error::TryRecvError, watch};

    use super::super::protocol::{proof_for_test, EndpointProof};
    use super::*;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::cdp_backend::CdpBackend;
    use crate::browser::engine::Engine;
    use crate::browser::live::grant::ViewerGrantStore;
    use crate::browser::live::resource::ResourceRegistry;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::events::GatewayEventFrame;
    use crate::routing::session_key::SessionKey;
    use crate::scope::ScopeAttribution;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    fn turn() -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("companion-test"),
            run_id: "companion-run".into(),
            channel_id: "".into(),
            conversation_id: "".into(),
            caller_role: None,
            telecom_authority: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    async fn scoped<F: std::future::Future>(future: F) -> F::Output {
        Box::pin(crate::scope::with_scope(
            Some(ScopeAttribution::personal("user-42")),
            Box::pin(TURN_CONTEXT.scope(turn(), Box::pin(future))),
        ))
        .await
    }

    struct FakeProcess {
        pid: u32,
        reap_ok: AtomicBool,
        exit: watch::Sender<bool>,
    }

    impl FakeProcess {
        fn new(pid: u32) -> Arc<Self> {
            Arc::new(Self {
                pid,
                reap_ok: AtomicBool::new(true),
                exit: watch::channel(false).0,
            })
        }
    }

    #[async_trait]
    impl CompanionProcess for FakeProcess {
        fn service_pid(&self) -> u32 {
            self.pid
        }
        fn is_alive(&self) -> bool {
            !*self.exit.borrow()
        }
        async fn exited(&self) {
            let mut rx = self.exit.subscribe();
            let _ = rx.wait_for(|e| *e).await;
        }
        async fn stop_and_reap(&self, _budget: Duration) -> Result<(), String> {
            if self.reap_ok.load(Ordering::SeqCst) {
                let _ = self.exit.send_replace(true);
                Ok(())
            } else {
                Err("did not exit".into())
            }
        }
    }

    struct FakeLauncher {
        steps: Arc<StepLog>,
        process: Arc<FakeProcess>,
        unsupported: bool,
        launched: AtomicBool,
    }

    #[async_trait]
    impl CompanionLauncher for FakeLauncher {
        fn preflight(&self) -> Result<(), LaunchError> {
            if self.unsupported {
                Err(LaunchError::Unsupported(
                    "wrapper pid is not the service pid".into(),
                ))
            } else {
                Ok(())
            }
        }
        async fn launch(
            &self,
            _req: &LaunchRequest,
            _creds: &LaunchCredentials,
        ) -> Result<Arc<dyn CompanionProcess>, LaunchError> {
            self.preflight()?;
            self.launched.store(true, Ordering::SeqCst);
            self.steps.push("launched");
            Ok(self.process.clone())
        }
    }

    struct FakeProver {
        fail: Option<ProtocolError>,
    }

    #[async_trait]
    impl EndpointProver for FakeProver {
        async fn prove(
            &self,
            creds: &LaunchCredentials,
            endpoint: &CanonicalEndpoint,
            process_alive: &(dyn Fn() -> bool + Send + Sync),
        ) -> Result<EndpointProof, ProtocolError> {
            assert!(process_alive(), "the child must be alive when proven");
            match &self.fail {
                Some(error) => Err(match error {
                    ProtocolError::ChallengeMismatch => ProtocolError::ChallengeMismatch,
                    _ => ProtocolError::ChildNotAlive,
                }),
                None => Ok(proof_for_test(creds, endpoint)),
            }
        }
    }

    /// Real receipt from a fake CDP server; `fail_open` refuses instead.
    struct FakeTabs {
        steps: Arc<StepLog>,
        backend: CdpBackend,
        server: Arc<FakeCdpServer>,
        _registry: Arc<crate::browser::engine::registry::EngineRegistry>,
        fail_open: bool,
        close_faults: AtomicUsize,
    }

    async fn tabs(steps: &Arc<StepLog>, fail_open: bool) -> Arc<FakeTabs> {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Target.createTarget",
            Responder::Reply(json!({ "targetId": "T-NEW" })),
        );
        server.on(
            "Page.navigate",
            Responder::Reply(json!({ "frameId": "F1", "loaderId": "L1" })),
        );
        let (registry, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        Arc::new(FakeTabs {
            steps: Arc::clone(steps),
            backend,
            server: Arc::new(server),
            _registry: registry,
            fail_open,
            close_faults: AtomicUsize::new(0),
        })
    }

    #[async_trait]
    impl CompanionTabs for FakeTabs {
        async fn open_tab(&self, url: &str) -> Result<(String, BrowserOperationRecord), String> {
            if self.fail_open {
                return Err("open refused".into());
            }
            let server = Arc::clone(&self.server);
            let emitter = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                server.push_event(json!({
                    "sessionId": "S1",
                    "method": "Page.frameNavigated",
                    "params": { "frame": { "id": "F1", "loaderId": "L1", "url": "https://ok.example/" }}
                }));
                server.push_event(json!({
                    "sessionId": "S1", "method": "Page.loadEventFired", "params": {}
                }));
            });
            let opened = self
                .backend
                .open_tab_with_record(url)
                .await
                .map_err(|e| e.to_string());
            let _ = emitter.await;
            opened
        }
        async fn close_tab(&self, _tab_id: &str) -> Result<(), String> {
            self.steps.push("tab_close_called");
            let injected = self
                .close_faults
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok();
            if injected {
                return Err("tab gone".into());
            }
            Ok(())
        }
    }

    struct Fixture {
        manager: Arc<CompanionManager>,
        process: Arc<FakeProcess>,
        launcher: Arc<FakeLauncher>,
        bus: Arc<GatewayEventBus>,
        steps: Arc<StepLog>,
    }

    fn fixture(prover_fail: Option<ProtocolError>, unsupported: bool, pid: u32) -> Fixture {
        fixture_with(Arc::new(FakeProver { fail: prover_fail }), unsupported, pid)
    }

    fn fixture_with(prover: Arc<dyn EndpointProver>, unsupported: bool, pid: u32) -> Fixture {
        let bus = Arc::new(GatewayEventBus::default());
        let producer =
            BrowserLiveProducer::new(ResourceRegistry::default(), ViewerGrantStore::default())
                .with_event_bus(bus.clone());
        let steps = Arc::new(StepLog::default());
        let process = FakeProcess::new(pid);
        let launcher = Arc::new(FakeLauncher {
            steps: Arc::clone(&steps),
            process: process.clone(),
            unsupported,
            launched: AtomicBool::new(false),
        });
        let manager = CompanionManager::with_steps(
            Some(producer),
            Some(launcher.clone()),
            prover,
            Arc::clone(&steps),
            Duration::from_millis(200),
        );
        Fixture {
            manager,
            process,
            launcher,
            bus,
            steps,
        }
    }

    fn request() -> LaunchRequest {
        LaunchRequest {
            program: "dev-server".into(),
            args: vec!["--port".into(), "5173".into()],
            cwd: None,
            endpoint: CanonicalEndpoint::parse("http://127.0.0.1:5173/app").expect("canonical"),
            ready_timeout: Duration::from_secs(5),
            lifetime: Duration::from_secs(60),
        }
    }

    fn owner() -> CallerIdentity {
        CallerIdentity::for_test("session-a", "run-a")
    }

    /// Success path: record-derived managed resource, driver-reported PID.
    #[tokio::test]
    async fn start_publishes_a_managed_design_companion_with_the_driver_pid() {
        let f = fixture(None, false, 4242);
        let mut rx = f.bus.subscribe_typed();
        let tabs = tabs(&f.steps, false).await;
        let (id, endpoint) = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect("start");
        assert_eq!(endpoint, "http://127.0.0.1:5173/app");
        let resource = match rx.try_recv().expect("one resource event") {
            GatewayEventFrame::BrowserLiveResource { resource } => resource,
            other => panic!("unexpected frame: {other:?}"),
        };
        assert_eq!(resource.kind, PreviewKind::DesignCompanion);
        assert_eq!(resource.display, DisplayPolicy::ExplicitOnly);
        assert_eq!(resource.redaction, RedactionStatus::Pending, "never Safe");
        assert!(resource.companion_candidates.is_empty());
        assert_eq!(
            resource.provenance,
            PreviewProvenance::Managed {
                launcher: "aleph.browser_companion".into(),
                process_id: 4242,
                launched_at_ms: match &resource.provenance {
                    PreviewProvenance::Managed { launched_at_ms, .. } => *launched_at_ms,
                    other => panic!("not managed: {other:?}"),
                },
            },
            "process_id is exactly what the launcher reported (not a wrapper pid)"
        );
        let target = &resource.viewer_binding().expect("binding").target;
        assert_eq!(target.target_id, "T-NEW", "target comes from the receipt");
        let (state, _, alive) = f.manager.status(&owner(), &id).expect("status");
        assert_eq!((state.as_str(), alive), ("running", true));
        f.manager.stop(&owner(), &id).await.expect("stop");
    }

    /// Stop order is observable: resource -> tab -> process reaped.
    #[tokio::test]
    async fn stop_closes_the_resource_then_the_tab_then_reaps_the_process() {
        let f = fixture(None, false, 7);
        let mut rx = f.bus.subscribe_typed();
        let tabs = tabs(&f.steps, false).await;
        let (id, _) = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect("start");
        let _ = rx.try_recv();
        f.manager.stop(&owner(), &id).await.expect("stop");
        assert!(matches!(
            rx.try_recv(),
            Ok(GatewayEventFrame::BrowserLiveClosed { .. })
        ));
        let steps = f.steps.snapshot();
        let pos = |s| steps.iter().position(|x| *x == s).unwrap_or(usize::MAX);
        assert!(
            pos("resource_closed") < pos("tab_close_called"),
            "{steps:?}"
        );
        assert!(pos("tab_close_called") < pos("tab_closed"), "{steps:?}");
        assert!(pos("tab_closed") < pos("process_reaped"), "{steps:?}");
        assert!(!f.process.is_alive());
        let (state, _, _) = f.manager.status(&owner(), &id).expect("status");
        assert_eq!(state, "stopped:stopped");
        // Idempotent: a second stop changes nothing.
        f.manager.stop(&owner(), &id).await.expect("second stop");
        assert_eq!(
            f.steps
                .snapshot()
                .iter()
                .filter(|s| **s == "process_reaped")
                .count(),
            1
        );
    }

    /// A reap that cannot be confirmed is an error and `stop_failed`.
    #[tokio::test]
    async fn stop_with_unconfirmed_exit_fails_closed_and_is_retryable() {
        let f = fixture(None, false, 8);
        let tabs = tabs(&f.steps, false).await;
        let (id, _) = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect("start");
        f.process.reap_ok.store(false, Ordering::SeqCst);
        assert!(matches!(
            f.manager.stop(&owner(), &id).await,
            Err(StopError::Failed(_))
        ));
        let (state, _, _) = f.manager.status(&owner(), &id).expect("status");
        assert_eq!(state, "stop_failed", "never reported as stopped");
        f.process.reap_ok.store(true, Ordering::SeqCst);
        f.manager.stop(&owner(), &id).await.expect("retry succeeds");
    }

    /// A tab close that errors is not success: the child is still reaped (no
    /// leak) but the stop fails closed, and a retry re-attempts the tab close.
    #[tokio::test]
    async fn tab_close_failure_fails_closed_reaps_the_child_and_the_retry_closes_the_tab() {
        let f = fixture(None, false, 31);
        let tabs = tabs(&f.steps, false).await;
        tabs.close_faults.store(1, Ordering::SeqCst);
        let (id, _) = scoped(f.manager.start(owner(), request(), tabs.clone()))
            .await
            .expect("start");

        let err = f.manager.stop(&owner(), &id).await.expect_err("fails");
        assert!(
            matches!(&err, StopError::Failed(m) if m.contains("tab close")),
            "{err}"
        );
        assert!(!f.process.is_alive(), "the child is still reaped");
        let steps = f.steps.snapshot();
        let pos = |s| steps.iter().position(|x| *x == s).unwrap_or(usize::MAX);
        assert!(pos("tab_close_failed") < pos("process_reaped"), "{steps:?}");
        assert!(!steps.contains(&"tab_closed"), "{steps:?}");

        f.manager.stop(&owner(), &id).await.expect("retry");
        let steps = f.steps.snapshot();
        assert_eq!(
            steps.iter().filter(|s| **s == "tab_close_called").count(),
            2,
            "{steps:?}"
        );
        assert_eq!(
            steps.iter().filter(|s| **s == "tab_closed").count(),
            1,
            "{steps:?}"
        );
        assert_eq!(
            f.manager.status(&owner(), &id).expect("status").0,
            "stopped:stopped"
        );
        // Idempotent afterwards.
        f.manager.stop(&owner(), &id).await.expect("third stop");
        assert_eq!(
            f.steps
                .snapshot()
                .iter()
                .filter(|s| **s == "tab_closed")
                .count(),
            1
        );
    }

    /// The natural exit of the child closes the resource and finishes cleanly.
    #[tokio::test]
    async fn natural_exit_closes_the_resource_and_stops() {
        let f = fixture(None, false, 9);
        let mut rx = f.bus.subscribe_typed();
        let tabs = tabs(&f.steps, false).await;
        let (id, _) = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect("start");
        let _ = rx.try_recv();
        let _ = f.process.exit.send_replace(true);
        for _ in 0..100 {
            if f.manager.status(&owner(), &id).expect("status").0 == "stopped:exited" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            f.manager.status(&owner(), &id).expect("status").0,
            "stopped:exited"
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(GatewayEventFrame::BrowserLiveClosed { .. })
        ));
    }

    /// status/stop need the same session, run and principal; a foreign caller
    /// cannot tell the id exists, and cannot stop it.
    #[tokio::test]
    async fn status_and_stop_require_the_starting_caller() {
        let f = fixture(None, false, 10);
        let tabs = tabs(&f.steps, false).await;
        let (id, _) = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect("start");
        for stranger in [
            CallerIdentity::for_test("session-b", "run-a"),
            CallerIdentity::for_test("session-a", "run-b"),
        ] {
            assert_eq!(
                f.manager.status(&stranger, &id).unwrap_err(),
                LookupError::NotFound
            );
            assert!(matches!(
                f.manager.stop(&stranger, &id).await,
                Err(StopError::NotFound)
            ));
        }
        assert!(f.process.is_alive(), "a stranger stop must not kill it");
        assert_eq!(
            f.manager.status(&owner(), "no-such-id").unwrap_err(),
            LookupError::NotFound
        );
        f.manager.stop(&owner(), &id).await.expect("owner stop");
    }

    /// Endpoint proof failure: nothing published, no tab, child reaped.
    #[tokio::test]
    async fn proof_failure_rolls_back_without_publishing_or_opening_a_tab() {
        let f = fixture(Some(ProtocolError::ChallengeMismatch), false, 11);
        let mut rx = f.bus.subscribe_typed();
        let tabs = tabs(&f.steps, false).await;
        let err = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect_err("proof fails");
        assert!(matches!(err, StartError::Proof(_)), "{err}");
        assert!(!f.process.is_alive(), "child killed and reaped");
        assert_eq!(f.steps.snapshot(), vec!["launched", "process_reaped"]);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        assert_eq!(f.manager.live_count(), 0);
    }

    /// open_tab failure: no resource, child reaped.
    #[tokio::test]
    async fn open_tab_failure_rolls_back_the_child() {
        let f = fixture(None, false, 12);
        let mut rx = f.bus.subscribe_typed();
        let tabs = tabs(&f.steps, true).await;
        let err = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect_err("open fails");
        assert!(matches!(err, StartError::OpenTab(_)), "{err}");
        assert_eq!(f.steps.snapshot(), vec!["launched", "process_reaped"]);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// Publish failure (no turn in scope): tab closed BEFORE the child is
    /// killed, nothing published.
    #[tokio::test]
    async fn publish_failure_closes_the_tab_then_reaps_the_child() {
        let f = fixture(None, false, 13);
        let mut rx = f.bus.subscribe_typed();
        let tabs = tabs(&f.steps, false).await;
        // Deliberately NOT scoped: current_binding() => MissingTurn.
        let err = f
            .manager
            .start(owner(), request(), tabs)
            .await
            .expect_err("publish fails");
        assert!(matches!(err, StartError::Publish(_)), "{err}");
        assert_eq!(
            f.steps.snapshot(),
            vec![
                "launched",
                "tab_close_called",
                "tab_closed",
                "process_reaped"
            ]
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// A rollback whose reap cannot be confirmed says so; it is not a clean
    /// failure.
    #[tokio::test]
    async fn rollback_reports_an_unconfirmed_reap() {
        let f = fixture(Some(ProtocolError::ChallengeMismatch), false, 14);
        f.process.reap_ok.store(false, Ordering::SeqCst);
        let tabs = tabs(&f.steps, false).await;
        let err = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect_err("fails");
        assert!(
            matches!(err, StartError::RollbackIncomplete { .. }),
            "{err}"
        );
    }

    /// An unsupported driver (Windows wrapper PID) fails BEFORE any launch.
    #[tokio::test]
    async fn unsupported_launcher_fails_before_any_launch() {
        let f = fixture(None, true, 15);
        let tabs = tabs(&f.steps, false).await;
        let err = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect_err("unsupported");
        assert!(matches!(err, StartError::Unsupported(_)), "{err}");
        assert!(!f.launcher.launched.load(Ordering::SeqCst));
        assert!(f.steps.snapshot().is_empty());
    }

    /// Without a producer nothing is launched at all.
    #[tokio::test]
    async fn missing_producer_launches_nothing() {
        let steps = Arc::new(StepLog::default());
        let process = FakeProcess::new(16);
        let launcher = Arc::new(FakeLauncher {
            steps: Arc::clone(&steps),
            process,
            unsupported: false,
            launched: AtomicBool::new(false),
        });
        let manager = CompanionManager::new(
            None,
            Some(launcher.clone()),
            Arc::new(FakeProver { fail: None }),
        );
        let tabs = tabs(&steps, false).await;
        let err = manager
            .start(owner(), request(), tabs)
            .await
            .expect_err("not configured");
        assert!(matches!(err, StartError::NotConfigured), "{err}");
        assert!(!launcher.launched.load(Ordering::SeqCst));
    }

    /// Daemon shutdown closes the resource before killing the child.
    #[tokio::test]
    async fn shutdown_all_uses_the_same_close_then_kill_order() {
        let f = fixture(None, false, 17);
        let tabs = tabs(&f.steps, false).await;
        let (id, _) = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect("start");
        assert_eq!(f.manager.shutdown_all().await, 0);
        let steps = f.steps.snapshot();
        let pos = |s| steps.iter().position(|x| *x == s).unwrap_or(usize::MAX);
        assert!(pos("resource_closed") < pos("process_reaped"), "{steps:?}");
        assert_eq!(
            f.manager.status(&owner(), &id).expect("status").0,
            "stopped:shutdown"
        );
    }

    // ------------------------------------------------ hardening (task48)

    /// Opens through the real fake tab only after `gate` is released, and
    /// signals `entered` first, so a test can cancel or shut down mid-start.
    struct GatedTabs {
        inner: Arc<FakeTabs>,
        gate: Arc<tokio::sync::Notify>,
        entered: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl CompanionTabs for GatedTabs {
        async fn open_tab(&self, url: &str) -> Result<(String, BrowserOperationRecord), String> {
            self.entered.notify_one();
            self.gate.notified().await;
            self.inner.open_tab(url).await
        }
        async fn close_tab(&self, tab_id: &str) -> Result<(), String> {
            self.inner.close_tab(tab_id).await
        }
    }

    async fn gated(steps: &Arc<StepLog>) -> Arc<GatedTabs> {
        Arc::new(GatedTabs {
            inner: tabs(steps, false).await,
            gate: Arc::new(tokio::sync::Notify::new()),
            entered: Arc::new(tokio::sync::Notify::new()),
        })
    }

    /// Wait (bounded) until the fake child is dead.
    async fn wait_dead(process: &FakeProcess) {
        for _ in 0..400 {
            if !process.is_alive() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the child was never reaped");
    }

    /// The `start` future is dropped while the tab is opening, i.e. AFTER the
    /// launcher returned and BEFORE the old code registered anything. The
    /// manager-owned watcher must still close and reap the child, and nothing
    /// may have been published.
    #[tokio::test]
    async fn cancelled_start_after_launch_reaps_the_child_and_publishes_nothing() {
        let f = fixture(None, false, 21);
        let mut rx = f.bus.subscribe_typed();
        let tabs = gated(&f.steps).await;
        let entered = Arc::clone(&tabs.entered);
        let manager = Arc::clone(&f.manager);
        let handle = tokio::spawn(scoped(async move {
            manager.start(owner(), request(), tabs).await.map(|_| ())
        }));
        entered.notified().await;
        assert!(f.process.is_alive(), "mid-start the child is supervised");
        handle.abort();
        assert!(handle.await.unwrap_err().is_cancelled());

        wait_dead(&f.process).await;
        assert_eq!(f.steps.snapshot(), vec!["launched", "process_reaped"]);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        assert_eq!(f.manager.live_count(), 0, "the slot was released");
    }

    /// Same, cancelled while the endpoint proof is still pending.
    #[tokio::test]
    async fn cancelled_start_during_proof_reaps_the_child() {
        struct HangingProver(Arc<tokio::sync::Notify>);
        #[async_trait]
        impl EndpointProver for HangingProver {
            async fn prove(
                &self,
                _creds: &LaunchCredentials,
                _endpoint: &CanonicalEndpoint,
                _alive: &(dyn Fn() -> bool + Send + Sync),
            ) -> Result<EndpointProof, ProtocolError> {
                self.0.notify_one();
                std::future::pending().await
            }
        }
        let entered = Arc::new(tokio::sync::Notify::new());
        let f = fixture_with(Arc::new(HangingProver(Arc::clone(&entered))), false, 22);
        let manager = Arc::clone(&f.manager);
        let tabs = tabs(&f.steps, false).await;
        let m = Arc::clone(&manager);
        let handle = tokio::spawn(scoped(async move {
            m.start(owner(), request(), tabs).await.map(|_| ())
        }));
        entered.notified().await;
        handle.abort();
        let _ = handle.await;
        wait_dead(&f.process).await;
        assert_eq!(f.steps.snapshot(), vec!["launched", "process_reaped"]);
        assert_eq!(manager.live_count(), 0);
    }

    /// `shutdown_all` sees a companion that is still `Starting` (through the
    /// same registry), closes it, and the late `start` then fails closed and
    /// closes the tab it opened meanwhile instead of publishing.
    #[tokio::test]
    async fn shutdown_all_closes_an_in_flight_start_and_it_does_not_publish() {
        let f = fixture(None, false, 23);
        let mut rx = f.bus.subscribe_typed();
        let tabs = gated(&f.steps).await;
        let entered = Arc::clone(&tabs.entered);
        let gate = Arc::clone(&tabs.gate);
        let manager = Arc::clone(&f.manager);
        let handle = tokio::spawn(scoped(async move {
            manager.start(owner(), request(), tabs).await.map(|_| ())
        }));
        entered.notified().await;
        assert_eq!(f.manager.shutdown_all().await, 0);
        assert!(!f.process.is_alive());
        gate.notify_one();
        let err = handle.await.expect("joined").expect_err("closed");
        assert!(matches!(err, StartError::ClosedDuringStart), "{err}");
        assert_eq!(
            f.steps.snapshot(),
            vec![
                "launched",
                "process_reaped",
                "tab_close_called",
                "tab_closed"
            ]
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        // A manager that shut down admits no new start.
        let again = f.manager.start(owner(), request(), tabs_ok(&f).await).await;
        assert!(matches!(again, Err(StartError::ShuttingDown)));
    }

    async fn tabs_ok(f: &Fixture) -> Arc<FakeTabs> {
        tabs(&f.steps, false).await
    }

    /// A failed resource close is surfaced, leaves the tab and the child
    /// untouched (the port stays owned), reports `stop_failed`, and a retry
    /// completes in the documented order.
    #[tokio::test]
    async fn resource_close_failure_fails_closed_and_the_retry_finishes_in_order() {
        use std::sync::atomic::Ordering as O;
        let f = fixture(None, false, 24);
        let mut rx = f.bus.subscribe_typed();
        let tabs = tabs(&f.steps, false).await;
        let (id, _) = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect("start");
        let _ = rx.try_recv();
        f.manager.close_faults.store(1, O::SeqCst);

        let err = f.manager.stop(&owner(), &id).await.expect_err("fails");
        assert!(
            matches!(&err, StopError::Failed(m) if m.contains("resource close")),
            "{err}"
        );
        assert_eq!(
            f.manager.status(&owner(), &id).expect("status").0,
            "stop_failed"
        );
        assert!(
            f.process.is_alive(),
            "the child is not killed under a live grant"
        );
        assert_eq!(
            f.steps.snapshot(),
            vec!["launched", "resource_close_failed"]
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        // shutdown reports it as unconfirmed too, still without killing.
        f.manager.close_faults.store(1, O::SeqCst);
        assert_eq!(f.manager.shutdown_all().await, 1);
        assert!(f.process.is_alive());

        f.manager.stop(&owner(), &id).await.expect("retry");
        assert_eq!(
            f.steps.snapshot(),
            vec![
                "launched",
                "resource_close_failed",
                "resource_close_failed",
                "resource_closed",
                "tab_close_called",
                "tab_closed",
                "process_reaped"
            ]
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(GatewayEventFrame::BrowserLiveClosed { .. })
        ));
        assert_eq!(
            f.manager.status(&owner(), &id).expect("status").0,
            "stopped:stopped"
        );
    }

    /// The watcher does not give up on a failed close: it retries, and only a
    /// confirmed close lets the child be reaped.
    #[tokio::test]
    async fn watcher_retries_a_failed_close_before_reaping() {
        use std::sync::atomic::Ordering as O;
        let f = fixture(None, false, 25);
        let tabs = tabs(&f.steps, false).await;
        let mut req = request();
        req.lifetime = Duration::from_millis(300);
        let (id, _) = scoped(f.manager.start(owner(), req, tabs))
            .await
            .expect("start");
        f.manager.close_faults.store(2, O::SeqCst);
        wait_dead(&f.process).await;
        let steps = f.steps.snapshot();
        assert_eq!(
            &steps[steps.len() - 4..],
            [
                "resource_closed",
                "tab_close_called",
                "tab_closed",
                "process_reaped"
            ],
            "{steps:?}"
        );
        assert_eq!(
            steps
                .iter()
                .filter(|s| **s == "resource_close_failed")
                .count(),
            2
        );
        assert_eq!(
            f.manager.status(&owner(), &id).expect("status").0,
            "stopped:lifetime"
        );
    }

    /// The cap is claimed atomically with the check: slots held by starts that
    /// have not registered a child yet count, and release on drop.
    #[tokio::test]
    async fn capacity_is_reserved_atomically_before_launch() {
        let f = fixture(None, false, 26);
        let mut held = Vec::new();
        for _ in 0..MAX_LIVE_COMPANIONS {
            held.push(f.manager.reserve().expect("slot"));
        }
        assert!(matches!(f.manager.reserve(), Err(StartError::Capacity)));
        assert_eq!(f.manager.live_count(), MAX_LIVE_COMPANIONS);
        held.pop();
        assert!(f.manager.reserve().is_ok(), "a dropped slot is reusable");
        drop(held);
        assert_eq!(f.manager.live_count(), 0, "every slot was released");
        // A start with no free slot launches nothing.
        let full: Vec<_> = (0..MAX_LIVE_COMPANIONS)
            .map(|_| f.manager.reserve().expect("slot"))
            .collect();
        assert_eq!(full.len(), MAX_LIVE_COMPANIONS);
        let tabs = tabs(&f.steps, false).await;
        let err = scoped(f.manager.start(owner(), request(), tabs))
            .await
            .expect_err("full");
        assert!(matches!(err, StartError::Capacity), "{err}");
        assert!(!f.launcher.launched.load(Ordering::SeqCst));
    }
}
