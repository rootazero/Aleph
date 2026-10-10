//! Loop-origin binding under REAL awaited boundaries.
//!
//! The loop sibling of `goal_origin_race.rs` / `goal_origin_effects.rs`. A loop
//! tick is claimed in `loop_post_run` after an `await` (the live token total)
//! and then lives through two more boundaries before it executes: the fire-time
//! delay (`confirm_fire_for`) and an `AgentBusy` / transient re-arm
//! (`rearm_loop_after_busy`). The loop CREATOR's telecom origin must be the one
//! the tick runs under at every one of them, so each boundary is exercised with
//! the loop replaced INSIDE it:
//!
//! 1. post-run token gate - [`ReplaceOnTokenRead`] runs the replacement inside
//!    the first `get_total_tokens`, i.e. between the engine's snapshot and its
//!    claim; the REAL adapter then sees (or does not see) the tick.
//! 2. fire time - `spawn_continuation_run`: a tick claimed on one incarnation
//!    must not fire a replacement that merely carries the same wake marker.
//! 3. busy re-arm - an adapter that answers `AgentBusy` after replacing the
//!    loop; the replacement must not receive a retry marker.
//!
//! Every mismatch has a control that proves the harness reaches the effect, so
//! a vacuous "nothing happened" cannot pass. No case waits out a real retry
//! delay: the cadence is 0 and the busy retry is judged by the registry row.

use super::goal_origin_race::{Hook, ReplaceOnTokenRead};
use super::*;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::session_store::SessionStore;
use crate::gateway::telecom::run_origin::sample_public_context;
use crate::gateway::telecom::TelecomPublicContext;
use crate::looping::types::{Cadence, LoopState, LoopStatus};
use crate::looping::LoopRegistry;

const FIRES: std::time::Duration = std::time::Duration::from_secs(3);
const QUIET: std::time::Duration = std::time::Duration::from_millis(600);

fn loops() -> Arc<LoopRegistry> {
    crate::looping::global().unwrap_or_else(|| {
        crate::looping::init_global(Arc::new(LoopRegistry::default()));
        crate::looping::global().expect("loop registry installed")
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A budgeted loop (so the post-run hook reaches the awaited token read) with
/// a 0 ms cadence (so the tick fires immediately; nothing waits).
fn loop_for(session: &str, origin: Option<&TelecomPublicContext>) -> LoopState {
    LoopState::new(
        session,
        "watch CI",
        Cadence::Fixed { interval_ms: 0 },
        now_ms(),
    )
    .with_token_budget(Some(1_000_000))
    .with_telecom_origin(origin.cloned())
}

/// What the user does to the loop inside the awaited boundary.
#[derive(Clone)]
enum Replace {
    /// `stop` + `start`: a brand-new loop incarnation under this origin
    /// (`None` = an ordinary run started it).
    Restart(Option<TelecomPublicContext>),
    /// `stop`.
    Stop,
    /// `pause`.
    Pause,
    /// `pause` then `resume`: the SAME loop incarnation (same creator origin).
    PauseResume,
    /// `loop(action='update')`: the SAME incarnation, a field changed.
    FieldUpdate,
    /// Nothing: the control that proves the hook plumbing reaches the claim.
    Nothing,
}

impl Replace {
    fn apply(&self, session: &str) {
        let reg = loops();
        match self {
            Replace::Restart(origin) => reg.put(loop_for(session, origin.as_ref())),
            Replace::Stop => {
                let _ = reg.transition(session, LoopStatus::Stopped, Some("stopped".into()));
            }
            Replace::Pause => {
                let _ = reg.transition(session, LoopStatus::Paused, None);
            }
            Replace::PauseResume => {
                let _ = reg.transition(session, LoopStatus::Paused, None);
                let _ = reg.transition(session, LoopStatus::Active, None);
            }
            Replace::FieldUpdate => {
                let live = reg.get(session).expect("loop is live");
                assert!(reg.commit_field_update(live.with_prompt("a new prompt"), false));
            }
            Replace::Nothing => {}
        }
    }

    fn hook(self, session: String) -> Hook {
        Box::new(move || self.apply(&session))
    }
}

/// Origins that differ from each other, as (label, loop creator, replacement).
fn swaps(tag: &str) -> Vec<(&'static str, Option<TelecomPublicContext>, Replace)> {
    let a = sample_public_context(&format!("{tag}-a"));
    let b = sample_public_context(&format!("{tag}-b"));
    assert_ne!(a, b, "the two contexts must be distinguishable");
    vec![
        (
            "public -> ordinary",
            Some(a.clone()),
            Replace::Restart(None),
        ),
        (
            "ordinary -> public",
            None,
            Replace::Restart(Some(b.clone())),
        ),
        ("public A -> public B", Some(a), Replace::Restart(Some(b))),
    ]
}

struct Rig {
    _temp: tempfile::TempDir,
    agent: Arc<AgentInstance>,
    registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    sessions: Arc<dyn SessionStore>,
    session: SessionKey,
    session_str: String,
}

async fn rig(tag: &str) -> Rig {
    let temp = tempfile::tempdir().unwrap();
    let session = SessionKey::main(tag);
    let session_str = session.to_key_string();
    let sessions: Arc<dyn SessionStore> = test_session_manager(&temp);
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
    let agent = registry.get(session.agent_id()).await.expect("agent");
    Rig {
        _temp: temp,
        agent,
        registry,
        sessions,
        session,
        session_str,
    }
}

impl Rig {
    fn deps(&self, adapter: Arc<dyn ExecutionAdapter>) -> ContinuationDeps {
        ContinuationDeps {
            registry: Arc::clone(&self.registry),
            adapter,
            gate: None,
            event_bus: None,
        }
    }

    fn live(&self) -> Option<LoopState> {
        loops().get(&self.session_str)
    }

    fn spawn(
        &self,
        adapter: Arc<dyn ExecutionAdapter>,
        prompt: String,
        wake_ms: u64,
        instance: u64,
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
            Some(0),
            super::super::execute::ContinuationKind::Loop { wake_ms, instance },
            origin,
        );
    }

    /// Claim the loop's next tick the way the engine does (bound claim).
    fn claim(&self) -> (u64, u64, String, Option<TelecomPublicContext>) {
        let snapshot = self.live().expect("seeded");
        let crate::looping::BoundTick::Claimed {
            decision:
                crate::looping::TickDecision::Fire {
                    wake_ms, prompt, ..
                },
            origin,
            instance,
        } = loops().try_claim_tick_bound(&snapshot, None, now_ms())
        else {
            panic!("an Active loop with runway must claim a tick");
        };
        (wake_ms, instance, prompt, origin)
    }
}

async fn request_within(
    adapter: &RecordingAdapter,
    session_str: &str,
    wait: std::time::Duration,
) -> Option<RunRequest> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        {
            let mut requests = adapter.requests.write().await;
            if let Some(at) = requests
                .iter()
                .position(|r| r.session_key.to_key_string() == session_str)
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

// ---- 1. post-run token gate ---------------------------------------------

/// Seed a loop under `creator`, run the real post-run hook with `replace`
/// applied inside the awaited token read, and return what the REAL adapter saw.
async fn post_run_case(
    tag: &str,
    creator: Option<TelecomPublicContext>,
    replace: Replace,
    expect_run: bool,
) -> (Rig, Option<RunRequest>) {
    let rig = rig(tag).await;
    loops().put(loop_for(&rig.session_str, creator.as_ref()));
    let (gated, reads) = ReplaceOnTokenRead::new(
        Arc::clone(&rig.sessions),
        replace.hook(rig.session_str.clone()),
    );
    let adapter = Arc::new(RecordingAdapter::new());
    super::super::execute::loop_post_run(
        &rig.deps(Arc::clone(&adapter) as Arc<dyn ExecutionAdapter>),
        &Some(gated as Arc<dyn SessionStore>),
        &rig.session,
        &rig.agent,
        &HashMap::new(),
        None,
    )
    .await;
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "the budgeted loop must reach the awaited token read exactly once - otherwise \
         this case never exercised the snapshot-then-claim gap"
    );
    let request = request_within(
        &adapter,
        &rig.session_str,
        if expect_run { FIRES } else { QUIET },
    )
    .await;
    (rig, request)
}

fn assert_untouched(rig: &Rig, label: &str, status: LoopStatus) {
    let live = rig.live().expect("the loop is registered");
    assert_eq!(live.status, status, "{label}");
    assert_eq!(
        live.iterations_used, 0,
        "{label}: a dropped claim must not spend an iteration on the live loop"
    );
    assert_eq!(
        live.pending_tick_wake_ms, None,
        "{label}: a dropped claim must not leave an in-flight marker on the live loop"
    );
}

/// The finding itself, in all three directions: a loop restarted (possibly
/// under another origin) during the awaited gate is a DIFFERENT incarnation than
/// the one the hook snapshotted. The tick must not be claimed on the
/// replacement, so nothing runs - in particular nothing runs under the stale
/// snapshot's origin - and the replacement is left untouched for its own turn.
#[tokio::test]
async fn post_run_restarted_loop_is_dropped_not_run_under_the_stale_origin() {
    for (label, creator, replace) in swaps("loop-race-post") {
        let live_origin = match &replace {
            Replace::Restart(o) => o.clone(),
            _ => unreachable!(),
        };
        let tag = format!("loop-race-post-{}", label.replace([' ', '>'], ""));
        let (rig, req) = post_run_case(&tag, creator, replace, false).await;
        assert!(
            req.is_none(),
            "{label}: must not run the replacement: {req:?}"
        );
        assert_untouched(&rig, label, LoopStatus::Active);
        assert_eq!(
            rig.live().unwrap().telecom_origin,
            live_origin,
            "{label}: the replacement keeps its own creator origin"
        );
    }
}

/// A loop that was stopped inside the gate and then RESTARTED is dropped too
/// (the restart is a new incarnation), and the next post-run for the new loop
/// claims it normally - under ITS creator origin.
#[tokio::test]
async fn post_run_after_a_dropped_claim_the_replacement_runs_under_its_own_origin() {
    let live = sample_public_context("loop-race-next-live");
    let (rig, req) = post_run_case(
        "loop-race-next",
        Some(sample_public_context("loop-race-next-stale")),
        Replace::Restart(Some(live.clone())),
        false,
    )
    .await;
    assert!(req.is_none());
    let (wake_ms, _instance, _prompt, origin) = rig.claim();
    assert_eq!(
        origin,
        Some(live),
        "the claim reports the LIVE creator origin"
    );
    assert_eq!(rig.live().unwrap().pending_tick_wake_ms, Some(wake_ms));
}

/// Controls: no replacement, and same-incarnation edits (field update,
/// pause+resume), keep running under the creator origin the loop was started
/// with, public and ordinary.
#[tokio::test]
async fn post_run_without_a_replacement_keeps_each_origin_kind() {
    let ctx = sample_public_context("loop-race-control");
    for (label, replace) in [
        ("nothing", Replace::Nothing),
        ("field update", Replace::FieldUpdate),
        ("pause+resume", Replace::PauseResume),
    ] {
        for (kind, creator) in [("public", Some(ctx.clone())), ("ordinary", None)] {
            let tag = format!("loop-race-ctl-{}-{kind}", label.replace([' ', '+'], ""));
            let (rig, req) = post_run_case(&tag, creator.clone(), replace.clone(), true).await;
            let req = req.unwrap_or_else(|| panic!("{label}/{kind}: control must run"));
            assert_eq!(req.telecom_public, creator, "{label}/{kind}");
            assert_eq!(rig.live().unwrap().iterations_used, 1, "{label}/{kind}");
        }
    }
}

// ---- 2. fire time: confirm_fire_for -------------------------------------

/// Claim on the creator's incarnation, replace the loop with one that carries
/// the SAME pending wake marker (the only thing the unbound confirm compared),
/// fire.
async fn fire_case(
    tag: &str,
    creator: Option<TelecomPublicContext>,
    replacement: Option<Option<TelecomPublicContext>>,
) -> (Rig, Option<RunRequest>) {
    let rig = rig(tag).await;
    loops().put(loop_for(&rig.session_str, creator.as_ref()));
    let (wake_ms, instance, prompt, origin) = rig.claim();
    assert_eq!(origin, creator, "the claim returns the live creator origin");
    if let Some(live_origin) = &replacement {
        loops()
            .put(loop_for(&rig.session_str, live_origin.as_ref()).with_pending_tick(Some(wake_ms)));
    }
    let adapter = Arc::new(RecordingAdapter::new());
    rig.spawn(
        Arc::clone(&adapter) as Arc<dyn ExecutionAdapter>,
        prompt,
        wake_ms,
        instance,
        origin,
    );
    let wait = if replacement.is_some() { QUIET } else { FIRES };
    let req = request_within(&adapter, &rig.session_str, wait).await;
    (rig, req)
}

/// Control: the tick fires under its own origin and the fire consumes the marker.
#[tokio::test]
async fn fire_on_the_claimed_incarnation_runs_and_consumes_the_marker() {
    for (kind, creator) in [
        ("public", Some(sample_public_context("loop-fire-ctl"))),
        ("ordinary", None),
    ] {
        let (rig, req) = fire_case(&format!("loop-fire-ctl-{kind}"), creator.clone(), None).await;
        let req = req.unwrap_or_else(|| panic!("{kind}: control must run"));
        assert_eq!(req.telecom_public, creator, "{kind}");
        assert_eq!(rig.live().unwrap().pending_tick_wake_ms, None, "{kind}");
    }
}

/// The fence: a loop replaced under another origin during the delay - even one
/// whose wake marker equals the claimed tick's - does not fire, and its marker
/// is left alone.
#[tokio::test]
async fn fire_after_a_restart_does_not_run_the_replacement() {
    for (label, creator, replace) in swaps("loop-fire") {
        let Replace::Restart(live) = replace else {
            unreachable!()
        };
        let tag = format!("loop-fire-{}", label.replace([' ', '>'], ""));
        let (rig, req) = fire_case(&tag, creator, Some(live)).await;
        assert!(req.is_none(), "{label}: replacement must not run: {req:?}");
        let live = rig.live().unwrap();
        assert!(
            live.pending_tick_wake_ms.is_some(),
            "{label}: a refused fire must not consume the replacement's marker"
        );
        assert_eq!(live.iterations_used, 0, "{label}");
    }
}

// ---- 3. busy re-arm ------------------------------------------------------

/// An adapter that loses the run slot: runs `hook` (the user replacing the loop
/// during the busy window) then answers `AgentBusy`.
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
        Err(ExecutionError::AgentBusy("loop-busy".to_string()))
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

async fn busy_case(
    tag: &str,
    creator: Option<TelecomPublicContext>,
    replace: Replace,
) -> (Rig, Arc<AtomicUsize>) {
    let rig = rig(tag).await;
    loops().put(loop_for(&rig.session_str, creator.as_ref()));
    let (wake_ms, instance, prompt, origin) = rig.claim();
    let calls = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(BusyOnce {
        hook: std::sync::Mutex::new(Some(replace.hook(rig.session_str.clone()))),
        calls: Arc::clone(&calls),
    });
    rig.spawn(adapter, prompt, wake_ms, instance, origin);
    assert!(
        wait_until(FIRES, || calls.load(Ordering::SeqCst) == 1).await,
        "the claimed tick never reached the adapter"
    );
    (rig, calls)
}

/// Control: the same loop across the busy window re-arms the claimed tick (a
/// fresh in-flight marker is stamped for the retry, no extra iteration).
#[tokio::test]
async fn busy_on_the_same_incarnation_rearms_the_tick() {
    for (kind, creator) in [
        ("public", Some(sample_public_context("loop-busy-ctl"))),
        ("ordinary", None),
    ] {
        let (rig, calls) =
            busy_case(&format!("loop-busy-ctl-{kind}"), creator, Replace::Nothing).await;
        assert!(
            wait_until(FIRES, || rig
                .live()
                .is_some_and(|l| l.pending_tick_wake_ms.is_some()))
            .await,
            "{kind}: a busy collision must re-arm the tick"
        );
        assert_eq!(rig.live().unwrap().iterations_used, 1, "{kind}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{kind}");
    }
}

/// The fence: a loop restarted under another origin (or stopped) while the slot
/// was busy is NOT re-armed - the retry would run the replacement under the
/// stale captured origin.
#[tokio::test]
async fn busy_after_a_restart_drops_instead_of_rearming() {
    let mut cases: Vec<(String, Option<TelecomPublicContext>, Replace)> = swaps("loop-busy")
        .into_iter()
        .map(|(l, c, r)| (l.to_string(), c, r))
        .collect();
    cases.push((
        "stop".into(),
        Some(sample_public_context("loop-busy-stop")),
        Replace::Stop,
    ));
    for (label, creator, replace) in cases {
        let tag = format!("loop-busy-{}", label.replace([' ', '>'], ""));
        let stopped = matches!(replace, Replace::Stop);
        let (rig, calls) = busy_case(&tag, creator, replace).await;
        // The re-arm decision is synchronous right after the busy answer; give
        // it (and a wrongly stamped retry) time to land.
        tokio::time::sleep(QUIET).await;
        let live = rig.live().expect("the loop is still registered");
        assert_eq!(
            live.pending_tick_wake_ms, None,
            "{label}: the replacement must not receive a retry marker"
        );
        // `stop` is not a replacement: the original tick keeps the iteration it
        // was claimed with. Every restart leaves the replacement at 0.
        assert_eq!(
            live.iterations_used,
            u32::from(stopped),
            "{label}: no iteration on the replacement"
        );
        assert_eq!(
            live.status,
            if stopped {
                LoopStatus::Stopped
            } else {
                LoopStatus::Active
            },
            "{label}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{label}: no second try");
    }
}

// ---- 4. pure seams: re-arm helpers --------------------------------------

/// The shared helper behind the busy AND authority-unknown re-arm, driven on a
/// loop whose pending marker was already cleared by the fire (so a replacement
/// is indistinguishable from "ours" except by incarnation).
#[tokio::test]
async fn rearm_loop_after_busy_is_bound_to_the_claimed_incarnation() {
    let reg = loops();
    for (label, creator, replace) in swaps("loop-rearm") {
        let tag = format!("loop-rearm-{}", label.replace([' ', '>'], ""));
        let session = SessionKey::main(&tag).to_key_string();
        reg.put(
            loop_for(&session, creator.as_ref())
                .spent_iteration()
                .with_pending_tick(None),
        );
        let claimed = reg.get(&session).unwrap().instance;
        replace.apply(&session);
        let out =
            super::super::execute::rearm_loop_after_busy(&session, claimed, None, "agent busy")
                .await;
        assert!(out.is_none(), "{label}: must drop, got {out:?}");
        let live = reg.get(&session).unwrap();
        assert_eq!(live.pending_tick_wake_ms, None, "{label}");
        assert_eq!(live.iterations_used, 0, "{label}");
    }
    // Control.
    let session = SessionKey::main("loop-rearm-control").to_key_string();
    reg.put(
        loop_for(&session, None)
            .spent_iteration()
            .with_pending_tick(None),
    );
    let claimed = reg.get(&session).unwrap().instance;
    let out =
        super::super::execute::rearm_loop_after_busy(&session, claimed, None, "agent busy").await;
    assert!(out.is_some(), "control must re-arm");
    assert_eq!(reg.get(&session).unwrap().iterations_used, 1);
}

// ---- census --------------------------------------------------------------

/// `execute.rs` production code reaches the registry only through the
/// identity-bound forms. A new caller of the unbound claim / confirm / re-arm
/// re-opens the snapshot-then-claim gap and reds here.
#[test]
fn execute_rs_uses_only_the_identity_bound_loop_registry_calls() {
    use crate::utils::source_scan::{production_prefix, strip_comment_lines};
    let src = include_str!("../execute.rs").replace('\r', "");
    let code = strip_comment_lines(&production_prefix(&src));
    for unbound in [
        ".try_claim_tick(",
        "reg.confirm_fire(",
        ".rearm_after_busy(",
        ".rearm_after_interruption(",
    ] {
        assert!(
            !code.contains(unbound),
            "execute.rs production code calls the unbound `{unbound}`"
        );
    }
    for bound in [
        ".try_claim_tick_bound(",
        ".confirm_fire_for(",
        ".rearm_after_busy_for(",
        ".rearm_after_interruption_for(",
    ] {
        assert!(code.contains(bound), "execute.rs must call `{bound}`");
    }
}
