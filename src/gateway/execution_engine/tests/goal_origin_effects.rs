//! Effect-level fences for the autonomous-telecom goal origin.
//!
//! `goal_origin_race.rs` proves the awaited-token-read race for `post_run` and
//! the wake sweep. This file covers the OTHER seams where a goal's creator
//! origin must still equal the origin a continuation carries, and asserts the
//! EFFECT (a run reached the adapter or it did not; the in-flight marker was
//! consumed or it was not) instead of a store verdict or a source grep:
//!
//! 1. fire time  - `spawn_continuation_run` -> `confirm_fire_for_origin`: a
//!    claimed step whose live origin changed before the fire must not spawn a
//!    run and must not consume the marker.
//! 2. busy re-arm - `AgentBusy` -> `rearm_goal_after_busy`: a goal replaced
//!    under another origin during the busy window must not be re-armed.
//! 3. gate veto - `post_run` -> `arbitrate_gate` -> `claim_after_gate_veto`:
//!    the objective gate runs with the run slot free; a goal whose origin was
//!    replaced while it ran must not be reopened and re-run under the stale
//!    origin.
//! 4. boot re-arm - `GoalWakeService::rearm_parked_goals`: a parked wake is
//!    re-spawned under the stored creator origin (and only then), and a goal
//!    replaced between that re-arm and the fire is dropped by the fire guard.
//!
//! Every mismatch case has a same-origin control that proves the harness
//! reaches the effect, so a vacuous "nothing happened" cannot pass.

use super::*;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::telecom::run_origin::sample_public_context;
use crate::gateway::telecom::TelecomPublicContext;
use crate::goal::{BoundClaim, ContinuationDecision, Goal, GoalStatus, PursuitMode};
use crate::verification::stop_hooks::{StopHookContext, StopHookHandler, StopHookVerdict};

type Hook = Box<dyn FnOnce() + Send>;

const OBJECTIVE: &str = "keep going";
/// A dispatch that must happen arrives well within this.
const FIRES: std::time::Duration = std::time::Duration::from_secs(3);
/// A dispatch that must NOT happen is given this long to (wrongly) show up.
const QUIET: std::time::Duration = std::time::Duration::from_millis(700);

fn active_goal(session: &str) -> Goal {
    Goal::new(session, OBJECTIVE, 0, 0).with_pursuit(PursuitMode::Active { max_iterations: 5 })
}

/// What the user does to the goal's creator origin while a claimed step is in
/// flight. The goal itself (id, status, marker, counters) is left untouched so
/// ONLY the origin check can reject the step.
#[derive(Clone)]
enum Swap {
    /// Re-record the same goal under this origin (`None` = ordinary).
    To(Option<TelecomPublicContext>),
    /// The origin record becomes unreadable.
    Corrupt,
}

impl Swap {
    fn apply(&self, store: &crate::goal::GoalStore, session: &str) {
        match self {
            Swap::To(origin) => {
                let live = store.get(session).unwrap().expect("the goal is live");
                store.put_with_origin(&live, origin.as_ref()).unwrap();
            }
            Swap::Corrupt => store.corrupt_origin_marker_for_test(session),
        }
    }

    fn hook(self, store: Arc<crate::goal::GoalStore>, session: String) -> Hook {
        Box::new(move || self.apply(&store, &session))
    }
}

/// The mismatching swaps, as (label, captured origin, swap).
fn mismatches(tag: &str) -> Vec<(&'static str, Option<TelecomPublicContext>, Swap)> {
    let a = sample_public_context(&format!("{tag}-a"));
    let b = sample_public_context(&format!("{tag}-b"));
    assert_ne!(a, b, "the two contexts must be distinguishable");
    vec![
        ("public -> ordinary", Some(a.clone()), Swap::To(None)),
        ("ordinary -> public", None, Swap::To(Some(b.clone()))),
        ("public A -> public B", Some(a.clone()), Swap::To(Some(b))),
        ("public -> unreadable", Some(a), Swap::Corrupt),
    ]
}

struct Rig {
    _temp: tempfile::TempDir,
    registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    sessions: Arc<dyn crate::gateway::session_store::SessionStore>,
    adapter: Arc<RecordingAdapter>,
    store: Arc<crate::goal::GoalStore>,
    session: SessionKey,
    session_str: String,
}

async fn rig(tag: &str) -> Rig {
    let temp = tempfile::tempdir().unwrap();
    let store = goal_store_global();
    let session = SessionKey::main(tag);
    let session_str = session.to_key_string();
    let sessions: Arc<dyn crate::gateway::session_store::SessionStore> =
        test_session_manager(&temp);
    let agent = AgentInstance::new(
        AgentInstanceConfig {
            agent_id: session.agent_id().to_string(),
            workspace: temp.path().join("agent-workspace"),
            agent_dir: temp.path().join("agents"),
            ..Default::default()
        },
        Arc::clone(&sessions),
    )
    .unwrap();
    let registry = Arc::new(crate::gateway::agent_instance::AgentRegistry::new());
    registry.register(agent).await;
    Rig {
        _temp: temp,
        registry,
        sessions,
        adapter: Arc::new(RecordingAdapter::new()),
        store,
        session,
        session_str,
    }
}

impl Rig {
    fn deps(&self, gate: Option<Arc<dyn StopHookHandler>>) -> ContinuationDeps {
        ContinuationDeps {
            registry: Arc::clone(&self.registry),
            adapter: Arc::clone(&self.adapter) as Arc<dyn ExecutionAdapter>,
            gate: gate.map(|g| Arc::new(vec![g])),
            event_bus: None,
        }
    }

    fn goal(&self) -> Option<Goal> {
        self.store.get(&self.session_str).unwrap()
    }

    /// Record `goal` under `origin`, then claim its next continuation exactly
    /// as the engine does (bound claim). Returns what the engine would spawn.
    fn claim(&self, goal: &Goal, origin: Option<&TelecomPublicContext>) -> (u64, String) {
        self.store.put_with_origin(goal, origin).unwrap();
        let peek = self.goal().expect("seeded");
        let claimed = self
            .store
            .try_claim_continuation_bound(
                &peek,
                None,
                super::super::goal_continuation::now_ms(),
                false,
                None,
            )
            .unwrap();
        let BoundClaim::Claimed {
            decision: ContinuationDecision::Fire {
                wake_ms, prompt, ..
            },
            origin: resolved,
        } = claimed
        else {
            panic!("an Active goal with runway must claim a continuation");
        };
        assert_eq!(
            resolved.as_ref(),
            origin,
            "the claim must resolve the creator origin the goal was recorded under"
        );
        assert_eq!(self.goal().unwrap().pending_continuation_ms, Some(wake_ms));
        (wake_ms, prompt)
    }

    fn spawn(
        &self,
        adapter: Arc<dyn ExecutionAdapter>,
        prompt: String,
        delay_ms: u64,
        wake_ms: u64,
        origin: Option<TelecomPublicContext>,
    ) {
        super::super::execute::spawn_continuation_run(
            Arc::clone(&self.registry),
            adapter,
            self.session.clone(),
            self.session_str.clone(),
            prompt,
            HashMap::new(),
            None,
            None,
            Some(Arc::clone(&self.sessions)),
            Some(delay_ms),
            super::super::execute::ContinuationKind::Goal { wake_ms },
            origin,
        );
    }

    /// The request this rig's session was dispatched with, within `wait`.
    /// Matched by session: the goal store is process-global and a boot sweep
    /// visits every parked goal, so a blind pop could take someone else's.
    async fn request_within(&self, wait: std::time::Duration) -> Option<RunRequest> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            {
                let mut requests = self.adapter.requests.write().await;
                if let Some(at) = requests
                    .iter()
                    .position(|r| r.session_key.to_key_string() == self.session_str)
                {
                    return Some(requests.remove(at));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

async fn wait_until(within: std::time::Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if cond() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

// ---- 1. fire time: confirm_fire_for_origin ------------------------------

/// Claim under `captured`, apply `swap` (if any) before the fire, fire.
async fn fire_case(
    tag: &str,
    captured: Option<TelecomPublicContext>,
    swap: Option<&Swap>,
) -> (Rig, u64, Option<RunRequest>) {
    let rig = rig(tag).await;
    let (wake_ms, prompt) = rig.claim(&active_goal(&rig.session_str), captured.as_ref());
    if let Some(swap) = swap {
        swap.apply(&rig.store, &rig.session_str);
    }
    rig.spawn(
        Arc::clone(&rig.adapter) as Arc<dyn ExecutionAdapter>,
        prompt,
        0,
        wake_ms,
        captured,
    );
    let wait = if swap.is_some() { QUIET } else { FIRES };
    let request = rig.request_within(wait).await;
    (rig, wake_ms, request)
}

/// Controls: with the origin unchanged the claimed step fires under its own
/// origin and the fire consumes the marker.
#[tokio::test]
async fn fire_with_an_unchanged_origin_runs_and_consumes_the_marker() {
    for (label, captured) in [
        ("public", Some(sample_public_context("fire-control"))),
        ("ordinary", None),
    ] {
        let (rig, _wake, request) = fire_case(
            &format!("effects-fire-control-{label}"),
            captured.clone(),
            None,
        )
        .await;
        let request = request.unwrap_or_else(|| panic!("{label}: the claimed step must run"));
        assert_eq!(request.telecom_public, captured, "{label}");
        assert_eq!(
            rig.goal().unwrap().pending_continuation_ms,
            None,
            "{label}: the fire must consume the in-flight marker"
        );
    }
}

/// The fence: a claimed step whose live creator origin no longer equals the
/// origin it carries must NOT spawn a run, and must leave the marker alone.
#[tokio::test]
async fn fire_after_the_origin_changed_spawns_nothing_and_keeps_the_marker() {
    for (label, captured, swap) in mismatches("fire") {
        let (rig, wake_ms, request) = fire_case(
            &format!("effects-fire-{}", label.replace([' ', '>'], "")),
            captured,
            Some(&swap),
        )
        .await;
        assert!(
            request.is_none(),
            "{label}: a stale-origin continuation reached the adapter: {request:?}"
        );
        let live = rig.goal().expect("the goal is still there");
        assert_eq!(
            live.pending_continuation_ms,
            Some(wake_ms),
            "{label}: a refused fire must not consume the marker"
        );
        assert_eq!(live.continuations_used, 1, "{label}: no extra iteration");
    }
}

// ---- 2. busy re-arm: rearm_goal_after_busy ------------------------------

/// An adapter that loses the run slot: runs `hook` (the user replacing the
/// goal during the busy window), then answers `AgentBusy`.
struct BusyOnce {
    hook: std::sync::Mutex<Option<Hook>>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ExecutionAdapter for BusyOnce {
    async fn execute(
        &self,
        _request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        let hook = self.hook.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(hook) = hook {
            hook();
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ExecutionError::AgentBusy("effects-busy".to_string()))
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        Err(ExecutionError::RunNotFound(run_id.to_string()))
    }

    async fn get_status(&self, _run_id: &str) -> Option<RunStatus> {
        None
    }

    async fn active_run_count(&self) -> usize {
        0
    }
}

/// Claim, fire into a busy adapter (optionally swapping the origin inside the
/// busy window) and return once the busy answer was produced.
async fn busy_case(
    tag: &str,
    captured: Option<TelecomPublicContext>,
    swap: Option<Swap>,
) -> (Rig, u64, Arc<AtomicUsize>) {
    let rig = rig(tag).await;
    let (wake_ms, prompt) = rig.claim(&active_goal(&rig.session_str), captured.as_ref());
    let calls = Arc::new(AtomicUsize::new(0));
    let hook = swap.map(|s| s.hook(Arc::clone(&rig.store), rig.session_str.clone()));
    let adapter = Arc::new(BusyOnce {
        hook: std::sync::Mutex::new(hook),
        calls: Arc::clone(&calls),
    });
    rig.spawn(adapter, prompt, 0, wake_ms, captured);
    let reached = wait_until(FIRES, || calls.load(Ordering::SeqCst) == 1).await;
    assert!(reached, "the claimed step never reached the adapter");
    (rig, wake_ms, calls)
}

/// Control: the same origin across the busy window re-arms the claimed step
/// (a fresh in-flight marker is stamped for the retry).
#[tokio::test]
async fn busy_with_an_unchanged_origin_rearms_the_step() {
    for (label, captured) in [
        ("public", Some(sample_public_context("busy-control"))),
        ("ordinary", None),
    ] {
        let (rig, _wake, calls) =
            busy_case(&format!("effects-busy-control-{label}"), captured, None).await;
        let rearmed = wait_until(FIRES, || {
            rig.goal()
                .is_some_and(|g| g.pending_continuation_ms.is_some())
        })
        .await;
        assert!(rearmed, "{label}: a busy collision must re-arm the step");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{label}");
    }
}

/// The fence: a goal replaced under another origin while the slot was busy is
/// dropped - the engine must not stamp a retry marker for it (the retry would
/// re-run it under the stale captured origin).
#[tokio::test]
async fn busy_after_the_origin_changed_drops_instead_of_rearming() {
    for (label, captured, swap) in mismatches("busy") {
        let (rig, _wake, calls) = busy_case(
            &format!("effects-busy-{}", label.replace([' ', '>'], "")),
            captured,
            Some(swap),
        )
        .await;
        // The re-arm decision is made synchronously right after the busy
        // answer; give it (and a wrongly stamped retry) time to land.
        tokio::time::sleep(QUIET).await;
        let live = rig.goal().expect("the goal is still there");
        assert_eq!(
            live.pending_continuation_ms, None,
            "{label}: a goal whose origin changed must not be re-armed"
        );
        assert_eq!(live.continuations_used, 1, "{label}: no extra iteration");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{label}: no second try");
    }
}

// ---- 3. gate veto: claim_after_gate_veto --------------------------------

/// A real `StopHookHandler` standing in for the objective gate. It runs `hook`
/// (the user replacing the goal while the gate runs) and then VETOES.
struct VetoGate {
    hook: std::sync::Mutex<Option<Hook>>,
    evaluated: Arc<AtomicUsize>,
}

#[async_trait]
impl StopHookHandler for VetoGate {
    fn name(&self) -> &str {
        "effects-veto-gate"
    }

    async fn evaluate(
        &self,
        _ctx: &StopHookContext,
        _cancel: &tokio_util::sync::CancellationToken,
    ) -> StopHookVerdict {
        let hook = self.hook.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(hook) = hook {
            hook();
        }
        self.evaluated.fetch_add(1, Ordering::SeqCst);
        StopHookVerdict::Block {
            reason: "the objective is not met".to_string(),
        }
    }
}

/// `post_run` over a goal the model self-reported `complete`, with a vetoing
/// gate that optionally swaps the origin while it runs.
async fn gate_case(
    tag: &str,
    seeded: Option<TelecomPublicContext>,
    swap: Option<Swap>,
) -> (Rig, Option<RunRequest>) {
    let rig = rig(tag).await;
    rig.store
        .put_with_origin(
            &active_goal(&rig.session_str).with_status(GoalStatus::Complete, 1),
            seeded.as_ref(),
        )
        .unwrap();
    let evaluated = Arc::new(AtomicUsize::new(0));
    let hook = swap
        .clone()
        .map(|s| s.hook(Arc::clone(&rig.store), rig.session_str.clone()));
    let gate = Arc::new(VetoGate {
        hook: std::sync::Mutex::new(hook),
        evaluated: Arc::clone(&evaluated),
    });
    let agent = rig
        .registry
        .get(rig.session.agent_id())
        .await
        .expect("agent");
    super::super::goal_continuation::post_run(
        &rig.deps(Some(gate)),
        &Some(Arc::clone(&rig.sessions)),
        &rig.session,
        &agent,
        &HashMap::new(),
        None,
    )
    .await;
    assert_eq!(
        evaluated.load(Ordering::SeqCst),
        1,
        "the completion claim must reach the real gate exactly once - otherwise \
         this case never exercised the veto path"
    );
    let request = rig
        .request_within(if swap.is_some() { QUIET } else { FIRES })
        .await;
    (rig, request)
}

/// Control: a veto with the origin unchanged reopens the goal and re-runs it
/// under its own creator origin.
#[tokio::test]
async fn gate_veto_with_an_unchanged_origin_reruns_under_that_origin() {
    for (label, seeded) in [
        ("public", Some(sample_public_context("gate-control"))),
        ("ordinary", None),
    ] {
        let (rig, request) = gate_case(
            &format!("effects-gate-control-{label}"),
            seeded.clone(),
            None,
        )
        .await;
        let request = request.unwrap_or_else(|| panic!("{label}: the veto must re-run the goal"));
        assert_eq!(request.telecom_public, seeded, "{label}");
        assert_eq!(
            rig.goal().unwrap().status,
            GoalStatus::Active,
            "{label}: the vetoed goal reopens"
        );
    }
}

/// The fence: the gate ran with the run slot free and the goal's origin was
/// replaced meanwhile. The veto must be discarded - no run under the stale
/// origin, and the goal is NOT reopened for it.
#[tokio::test]
async fn gate_veto_after_the_origin_changed_runs_nothing_and_reopens_nothing() {
    for (label, seeded, swap) in mismatches("gate") {
        let (rig, request) = gate_case(
            &format!("effects-gate-{}", label.replace([' ', '>'], "")),
            seeded,
            Some(swap),
        )
        .await;
        assert!(
            request.is_none(),
            "{label}: a stale-origin gate-veto rerun reached the adapter: {request:?}"
        );
        let live = rig.goal().expect("the goal is still there");
        assert_eq!(
            live.status,
            GoalStatus::Complete,
            "{label}: the discarded veto must not reopen the goal"
        );
        assert_eq!(live.continuations_used, 0, "{label}: no iteration spent");
    }
}

// ---- 4. boot re-arm: rearm_parked_goals ---------------------------------

/// A goal parked on an elapsed timer barrier whose claimed wake died with the
/// previous process: Active, pursuit Active, barrier + stamped marker.
fn parked_goal(session: &str, wake_ms: u64) -> Goal {
    active_goal(session)
        .with_wait_until(1, None, 0)
        .with_pending_continuation(Some(wake_ms))
}

fn boot_service(rig: &Rig) -> super::super::goal_wait::GoalWakeService {
    super::super::goal_wait::GoalWakeService::new(
        rig.deps(None),
        None,
        Some(Arc::clone(&rig.sessions)),
    )
}

/// Controls: the boot re-arm replays the parked wake under the goal's stored
/// creator origin - public stays public, ordinary and legacy stay ordinary -
/// and the fire consumes the marker.
#[tokio::test]
async fn boot_rearm_respawns_the_parked_wake_under_the_stored_origin() {
    let _guard = GOAL_SWEEP_LOCK.lock().await;
    let public = sample_public_context("boot-control");
    for (label, origin, legacy) in [
        ("public", Some(public), false),
        ("ordinary", None, false),
        ("legacy", None, true),
    ] {
        let rig = rig(&format!("effects-boot-control-{label}")).await;
        let wake_ms = super::super::goal_continuation::now_ms().saturating_sub(1);
        let goal = parked_goal(&rig.session_str, wake_ms);
        if legacy {
            rig.store.put(&goal).unwrap();
        } else {
            rig.store.put_with_origin(&goal, origin.as_ref()).unwrap();
        }
        boot_service(&rig).rearm_parked_goals().await;
        let request = rig
            .request_within(FIRES)
            .await
            .unwrap_or_else(|| panic!("{label}: the parked wake must be re-spawned"));
        assert_eq!(request.telecom_public, origin, "{label}");
        assert_eq!(
            rig.goal().unwrap().pending_continuation_ms,
            None,
            "{label}: the replayed fire consumes the marker"
        );
        rig.store.delete(&rig.session_str).unwrap();
    }
}

/// An unreadable origin record at boot: the wake is dropped without a run and
/// the stored marker is left for the user to resolve.
#[tokio::test]
async fn boot_rearm_refuses_an_unreadable_origin_and_keeps_the_marker() {
    let _guard = GOAL_SWEEP_LOCK.lock().await;
    let rig = rig("effects-boot-corrupt").await;
    let wake_ms = super::super::goal_continuation::now_ms().saturating_sub(1);
    rig.store
        .put_with_origin(
            &parked_goal(&rig.session_str, wake_ms),
            Some(&sample_public_context("boot-corrupt")),
        )
        .unwrap();
    rig.store.corrupt_origin_marker_for_test(&rig.session_str);
    boot_service(&rig).rearm_parked_goals().await;
    assert!(
        rig.request_within(QUIET).await.is_none(),
        "a goal with an untrustworthy origin must not be re-armed"
    );
    assert_eq!(
        rig.goal().unwrap().pending_continuation_ms,
        Some(wake_ms),
        "a refused re-arm must not consume the marker"
    );
    rig.store.delete(&rig.session_str).unwrap();
}

/// Boot -> fire race: the re-arm resolved the origin and spawned the wake, then
/// the goal's origin changed before the (delayed) fire. The fire guard must
/// drop it: no run, marker untouched.
#[tokio::test]
async fn boot_rearmed_wake_is_dropped_when_the_origin_changes_before_the_fire() {
    let _guard = GOAL_SWEEP_LOCK.lock().await;
    for (label, captured, swap) in mismatches("boot-race") {
        let rig = rig(&format!(
            "effects-boot-race-{}",
            label.replace([' ', '>'], "")
        ))
        .await;
        // Far enough out that the swap lands well before the fire.
        let wake_ms = super::super::goal_continuation::now_ms() + 1_000;
        rig.store
            .put_with_origin(&parked_goal(&rig.session_str, wake_ms), captured.as_ref())
            .unwrap();
        boot_service(&rig).rearm_parked_goals().await;
        swap.apply(&rig.store, &rig.session_str);
        assert!(
            rig.request_within(std::time::Duration::from_millis(2_000))
                .await
                .is_none(),
            "{label}: a boot-rearmed wake ran under a stale origin"
        );
        assert_eq!(
            rig.goal().unwrap().pending_continuation_ms,
            Some(wake_ms),
            "{label}: a refused fire must not consume the marker"
        );
        rig.store.delete(&rig.session_str).unwrap();
    }
}
