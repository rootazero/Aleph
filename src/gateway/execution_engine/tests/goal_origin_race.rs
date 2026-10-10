//! Goal-origin binding under a REAL awaited token gate.
//!
//! `post_run` and the wake service both read a goal, then `await` the live
//! token total (`live_tokens` -> `tree_tokens` -> `SessionStore::get_total_tokens`)
//! before they claim a continuation. The user can replace the goal inside that
//! await. These tests put the replacement exactly there: [`ReplaceOnTokenRead`]
//! decorates the session store the engine reads tokens from and runs a hook on
//! the first `get_total_tokens`, i.e. between the engine's goal snapshot and
//! its claim. Every other store method delegates unchanged.
//!
//! A goal only reaches that await when it carries a token budget, so every
//! goal here is budgeted. Each case asserts the hook actually ran, so a future
//! change that stops reading tokens cannot turn these into vacuous passes.

use super::*;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types as store_types;
use crate::gateway::session_store::types::SessionFilter;
use crate::gateway::session_store::SessionStore;
use crate::gateway::telecom::run_origin::sample_public_context;
use crate::gateway::telecom::TelecomPublicContext;
use crate::goal::{Goal, PursuitMode};

type Hook = Box<dyn FnOnce() + Send>;

/// A session store whose first `get_total_tokens` runs `hook` and then answers
/// `Some(0)` (a budgeted goal, nowhere near over budget).
struct ReplaceOnTokenRead {
    inner: Arc<dyn SessionStore>,
    hook: std::sync::Mutex<Option<Hook>>,
    reads: Arc<AtomicUsize>,
}

impl ReplaceOnTokenRead {
    fn new(inner: Arc<dyn SessionStore>, hook: Hook) -> (Arc<Self>, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                inner,
                hook: std::sync::Mutex::new(Some(hook)),
                reads: Arc::clone(&reads),
            }),
            reads,
        )
    }
}

#[async_trait]
impl SessionStore for ReplaceOnTokenRead {
    async fn get_total_tokens(&self, _key: &SessionKey) -> Result<Option<u64>, SessionStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let hook = self.hook.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(Some(0))
    }

    fn transcript_file(&self, key: &SessionKey) -> Option<std::path::PathBuf> {
        self.inner.transcript_file(key)
    }
    async fn get_or_create(
        &self,
        key: &SessionKey,
    ) -> Result<store_types::SessionMetadata, SessionStoreError> {
        self.inner.get_or_create(key).await
    }
    async fn get_metadata(
        &self,
        key: &SessionKey,
    ) -> Result<Option<store_types::SessionMetadata>, SessionStoreError> {
        self.inner.get_metadata(key).await
    }
    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<store_types::SessionMetadata>, SessionStoreError> {
        self.inner.list_sessions(filter).await
    }
    async fn delete_session(
        &self,
        key: &SessionKey,
    ) -> Result<store_types::DeleteResult, SessionStoreError> {
        self.inner.delete_session(key).await
    }
    async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionStoreError> {
        self.inner.reset_session(key).await
    }
    async fn append_message(
        &self,
        key: &SessionKey,
        msg: store_types::MessageRecord,
    ) -> Result<(), SessionStoreError> {
        self.inner.append_message(key, msg).await
    }
    async fn get_history(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
    ) -> Result<Vec<store_types::MessageRecord>, SessionStoreError> {
        self.inner.get_history(key, limit).await
    }
    async fn search_messages(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<store_types::SearchHit>, SessionStoreError> {
        self.inner.search_messages(query, max_results).await
    }
    async fn list_checkpoints(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<store_types::CheckpointSummary>, SessionStoreError> {
        self.inner.list_checkpoints(key).await
    }
    async fn branch_from_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
        new_key: &SessionKey,
    ) -> Result<store_types::SessionMetadata, SessionStoreError> {
        self.inner
            .branch_from_checkpoint(key, checkpoint_id, new_key)
            .await
    }
    async fn restore_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
    ) -> Result<store_types::SessionMetadata, SessionStoreError> {
        self.inner.restore_checkpoint(key, checkpoint_id).await
    }
    async fn close_session(
        &self,
        key: &SessionKey,
        topic: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.inner.close_session(key, topic).await
    }
    async fn set_topic(&self, key: &SessionKey, topic: &str) -> Result<(), SessionStoreError> {
        self.inner.set_topic(key, topic).await
    }
    async fn set_state(
        &self,
        key: &SessionKey,
        state: crate::gateway::session_manager::SessionState,
    ) -> Result<(), SessionStoreError> {
        self.inner.set_state(key, state).await
    }
    async fn get_state(
        &self,
        key: &SessionKey,
    ) -> Result<crate::gateway::session_manager::SessionState, SessionStoreError> {
        self.inner.get_state(key).await
    }
    async fn get_identity_context(
        &self,
        session_key: &str,
        source_channel: &str,
    ) -> Result<aleph_protocol::IdentityContext, SessionStoreError> {
        self.inner
            .get_identity_context(session_key, source_channel)
            .await
    }
    async fn get_current_epoch(&self, base_key_pattern: &str) -> Result<u32, SessionStoreError> {
        self.inner.get_current_epoch(base_key_pattern).await
    }
    async fn get_session_topic(
        &self,
        key: &SessionKey,
    ) -> Result<Option<String>, SessionStoreError> {
        self.inner.get_session_topic(key).await
    }
    async fn cleanup_expired(&self) -> Result<usize, SessionStoreError> {
        self.inner.cleanup_expired().await
    }
    async fn patch_session(
        &self,
        key: &SessionKey,
        patch: &store_types::SessionPatch,
    ) -> Result<bool, SessionStoreError> {
        self.inner.patch_session(key, patch).await
    }
    async fn get_session_preview(
        &self,
        key: &SessionKey,
        message_limit: usize,
    ) -> Result<store_types::SessionPreview, SessionStoreError> {
        self.inner.get_session_preview(key, message_limit).await
    }
    async fn count_by_state(
        &self,
        state: crate::gateway::session_manager::SessionState,
    ) -> Result<usize, SessionStoreError> {
        self.inner.count_by_state(state).await
    }
    async fn list_by_state(
        &self,
        state: crate::gateway::session_manager::SessionState,
    ) -> Result<Vec<store_types::SessionMetadata>, SessionStoreError> {
        self.inner.list_by_state(state).await
    }
    async fn set_error(
        &self,
        key: &SessionKey,
        error_msg: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.inner.set_error(key, error_msg).await
    }
    async fn stop(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.inner.stop(key).await
    }
    async fn set_idle(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.inner.set_idle(key).await
    }
    async fn stamp_and_bill_in_range(
        &self,
        key: &SessionKey,
        after_seq: u64,
        before_seq: u64,
        metadata: &serde_json::Value,
        bill: Option<&crate::gateway::session_store::RunBill>,
    ) -> Result<crate::gateway::session_store::StampOutcome, SessionStoreError> {
        self.inner
            .stamp_and_bill_in_range(key, after_seq, before_seq, metadata, bill)
            .await
    }
}

const OBJECTIVE: &str = "keep going";

fn budgeted(session: &str, objective: &str) -> Goal {
    Goal::new(session, objective, 0, 0)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_budget(Some(1_000_000))
}

/// What the goal looks like at the moment the engine snapshots it.
enum Seed {
    /// Created by a public run.
    Public(TelecomPublicContext),
    /// Created by an ordinary run (an `Ordinary` marker row).
    Ordinary,
    /// Written before origins existed (no marker row at all).
    Legacy,
}

/// What the user does to the goal during the awaited token read.
enum Replace {
    /// Same objective (so the SAME goal id), re-created by a run with this
    /// origin (`None` = an ordinary run).
    SameId(Option<TelecomPublicContext>),
    /// A different objective: a different goal id.
    OtherObjective(Option<TelecomPublicContext>),
    /// The goal is deleted outright.
    Cleared,
    /// Same goal id, but the origin record becomes unreadable.
    CorruptOrigin(TelecomPublicContext),
    /// Nothing: the control that proves the hook plumbing reaches the claim.
    Nothing,
}

fn seed(store: &crate::goal::GoalStore, goal: &Goal, seed: &Seed) {
    match seed {
        Seed::Public(ctx) => store.put_with_origin(goal, Some(ctx)).unwrap(),
        Seed::Ordinary => store.put_with_origin(goal, None).unwrap(),
        Seed::Legacy => store.put(goal).unwrap(),
    }
}

fn replace_hook(store: Arc<crate::goal::GoalStore>, session: String, replace: Replace) -> Hook {
    Box::new(move || match replace {
        Replace::SameId(origin) => store
            .put_with_origin(&budgeted(&session, OBJECTIVE), origin.as_ref())
            .unwrap(),
        Replace::OtherObjective(origin) => store
            .put_with_origin(
                &budgeted(&session, "a different objective"),
                origin.as_ref(),
            )
            .unwrap(),
        Replace::Cleared => {
            store.delete(&session).unwrap();
        }
        Replace::CorruptOrigin(ctx) => {
            // Re-create (public) then make the record unreadable.
            store
                .put_with_origin(&budgeted(&session, OBJECTIVE), Some(&ctx))
                .unwrap();
            store.corrupt_origin_marker_for_test(&session);
        }
        Replace::Nothing => {}
    })
}

struct Fixture {
    _temp: tempfile::TempDir,
    agent: Arc<AgentInstance>,
    registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    adapter: Arc<RecordingAdapter>,
    gated_sessions: Arc<ReplaceOnTokenRead>,
    reads: Arc<AtomicUsize>,
    store: Arc<crate::goal::GoalStore>,
    session: SessionKey,
    session_str: String,
}

async fn fixture(tag: &str, seed_as: &Seed, replace: Replace) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let store = goal_store_global();
    let session = SessionKey::main(tag);
    let session_str = session.to_key_string();
    seed(&store, &budgeted(&session_str, OBJECTIVE), seed_as);

    let real: Arc<dyn SessionStore> = test_session_manager(&temp);
    let agent = AgentInstance::new(
        AgentInstanceConfig {
            agent_id: session.agent_id().to_string(),
            workspace: temp.path().join("agent-workspace"),
            agent_dir: temp.path().join("agents"),
            ..Default::default()
        },
        Arc::clone(&real),
    )
    .unwrap();
    let registry = Arc::new(crate::gateway::agent_instance::AgentRegistry::new());
    registry.register(agent).await;
    let agent = registry.get(session.agent_id()).await.expect("agent");
    let (gated_sessions, reads) = ReplaceOnTokenRead::new(
        real,
        replace_hook(Arc::clone(&store), session_str.clone(), replace),
    );
    Fixture {
        _temp: temp,
        agent,
        registry,
        adapter: Arc::new(RecordingAdapter::new()),
        gated_sessions,
        reads,
        store,
        session,
        session_str,
    }
}

impl Fixture {
    fn deps(&self) -> ContinuationDeps {
        ContinuationDeps {
            registry: Arc::clone(&self.registry),
            adapter: Arc::clone(&self.adapter) as Arc<dyn ExecutionAdapter>,
            gate: None,
            event_bus: None,
        }
    }

    fn sessions(&self) -> Arc<dyn SessionStore> {
        Arc::clone(&self.gated_sessions) as Arc<dyn SessionStore>
    }

    /// The request this fixture's session was dispatched with, within `wait`.
    /// Other tests' goals share the global store and a wake sweep visits all of
    /// them, so requests are matched by session instead of taken blindly.
    async fn request_for_session(&self, wait: std::time::Duration) -> Option<RunRequest> {
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

    async fn run_post_run(&self) {
        super::super::goal_continuation::post_run(
            &self.deps(),
            &Some(self.sessions()),
            &self.session,
            &self.agent,
            &HashMap::new(),
            None,
        )
        .await;
    }
}

const FIRES: std::time::Duration = std::time::Duration::from_secs(3);
const QUIET: std::time::Duration = std::time::Duration::from_millis(600);

/// Run `post_run` with the goal replaced inside the awaited token read.
async fn post_run_case(
    tag: &str,
    seed_as: Seed,
    replace: Replace,
) -> (Fixture, Option<RunRequest>) {
    let expect_fire = !matches!(
        replace,
        Replace::OtherObjective(_) | Replace::Cleared | Replace::CorruptOrigin(_)
    );
    let fx = fixture(tag, &seed_as, replace).await;
    fx.run_post_run().await;
    assert_eq!(
        fx.reads.load(Ordering::SeqCst),
        1,
        "the budgeted goal must reach the awaited token read exactly once — otherwise \
         this case never exercised the snapshot-then-claim gap"
    );
    let request = fx
        .request_for_session(if expect_fire { FIRES } else { QUIET })
        .await;
    (fx, request)
}

fn assert_nothing_claimed(fx: &Fixture) {
    if let Some(live) = fx.store.get(&fx.session_str).unwrap() {
        assert_eq!(
            live.continuations_used, 0,
            "a dropped claim must not spend an iteration on the replacement goal"
        );
        assert!(
            live.pending_continuation_ms.is_none(),
            "a dropped claim must not leave an in-flight marker on the replacement goal"
        );
    }
}

// ---- post_run ----------------------------------------------------------

/// The finding itself: a PUBLIC goal replaced by an ordinary one during the
/// awaited gate must continue ORDINARY (the live goal's creator origin), not
/// under the stale public context the snapshot resolved.
#[tokio::test]
async fn post_run_replaced_public_by_ordinary_continues_ordinary() {
    let stale = sample_public_context("race-pub-ord");
    let (fx, req) = post_run_case(
        "race-post-run-pub-to-ord",
        Seed::Public(stale),
        Replace::SameId(None),
    )
    .await;
    let req = req.expect("the replacement goal has runway and must continue");
    assert_eq!(req.telecom_public, None, "stale public context leaked");
    assert_eq!(
        fx.store
            .get(&fx.session_str)
            .unwrap()
            .unwrap()
            .continuations_used,
        1
    );
}

/// The reverse: an ordinary goal replaced by a public one during the gate must
/// NOT run unrestricted under the snapshot's `None`.
#[tokio::test]
async fn post_run_replaced_ordinary_by_public_continues_public() {
    let live = sample_public_context("race-ord-pub");
    let (_fx, req) = post_run_case(
        "race-post-run-ord-to-pub",
        Seed::Ordinary,
        Replace::SameId(Some(live.clone())),
    )
    .await;
    let req = req.expect("the replacement goal has runway and must continue");
    assert_eq!(req.telecom_public, Some(live));
}

/// A legacy (no marker) goal replaced by a public one: same fail-closed answer.
#[tokio::test]
async fn post_run_replaced_legacy_by_public_continues_public() {
    let live = sample_public_context("race-legacy-pub");
    let (_fx, req) = post_run_case(
        "race-post-run-legacy-to-pub",
        Seed::Legacy,
        Replace::SameId(Some(live.clone())),
    )
    .await;
    assert_eq!(req.expect("must continue").telecom_public, Some(live));
}

/// Public A replaced by public B: B's context, never A's.
#[tokio::test]
async fn post_run_replaced_public_by_another_public_uses_the_live_context() {
    let stale = sample_public_context("race-a");
    let live = sample_public_context("race-b");
    assert_ne!(stale, live, "the two contexts must be distinguishable");
    let (_fx, req) = post_run_case(
        "race-post-run-pub-to-pub",
        Seed::Public(stale),
        Replace::SameId(Some(live.clone())),
    )
    .await;
    assert_eq!(req.expect("must continue").telecom_public, Some(live));
}

/// A different goal (different id) replaced the one the engine snapshotted: the
/// claim is dropped — no run, no iteration spent on the new goal, in both
/// directions.
#[tokio::test]
async fn post_run_drops_the_claim_when_a_different_goal_replaced_it() {
    let (fx, req) = post_run_case(
        "race-post-run-other-ord",
        Seed::Public(sample_public_context("race-other-a")),
        Replace::OtherObjective(None),
    )
    .await;
    assert!(req.is_none(), "must not run the replacement goal: {req:?}");
    assert_nothing_claimed(&fx);

    let (fx, req) = post_run_case(
        "race-post-run-other-pub",
        Seed::Ordinary,
        Replace::OtherObjective(Some(sample_public_context("race-other-b"))),
    )
    .await;
    assert!(req.is_none(), "must not run the replacement goal: {req:?}");
    assert_nothing_claimed(&fx);
}

/// The goal was cleared during the gate: nothing to continue.
#[tokio::test]
async fn post_run_drops_the_claim_when_the_goal_was_cleared() {
    let (fx, req) = post_run_case(
        "race-post-run-cleared",
        Seed::Public(sample_public_context("race-cleared")),
        Replace::Cleared,
    )
    .await;
    assert!(req.is_none(), "{req:?}");
    assert!(fx.store.get(&fx.session_str).unwrap().is_none());
}

/// An origin record that became unreadable during the gate is refused
/// (fail-closed), not read as ordinary.
#[tokio::test]
async fn post_run_refuses_an_origin_that_became_untrustworthy_during_the_gate() {
    let (fx, req) = post_run_case(
        "race-post-run-corrupt",
        Seed::Ordinary,
        Replace::CorruptOrigin(sample_public_context("race-corrupt")),
    )
    .await;
    assert!(req.is_none(), "must fail closed: {req:?}");
    assert_nothing_claimed(&fx);
}

/// Controls — the gate path with NO replacement behaves as before.
#[tokio::test]
async fn post_run_through_the_token_gate_keeps_each_origin_kind_unchanged() {
    let ctx = sample_public_context("race-control");
    let (_fx, req) = post_run_case(
        "race-control-public",
        Seed::Public(ctx.clone()),
        Replace::Nothing,
    )
    .await;
    assert_eq!(req.expect("fires").telecom_public, Some(ctx));

    let (_fx, req) = post_run_case("race-control-ordinary", Seed::Ordinary, Replace::Nothing).await;
    assert_eq!(req.expect("fires").telecom_public, None);

    let (_fx, req) = post_run_case("race-control-legacy", Seed::Legacy, Replace::Nothing).await;
    assert_eq!(
        req.expect("a legacy goal keeps running as before")
            .telecom_public,
        None
    );
}

// ---- wake --------------------------------------------------------------

/// The wake sweep over a goal whose timer barrier has elapsed. Same hook, same
/// gap: `claim_and_spawn` awaits `live_tokens` between the sweep's snapshot and
/// the claim.
async fn wake_case(tag: &str, seed_as: Seed, replace: Replace) -> (Fixture, Option<RunRequest>) {
    let expect_fire = !matches!(
        replace,
        Replace::OtherObjective(_) | Replace::Cleared | Replace::CorruptOrigin(_)
    );
    let fx = fixture(tag, &seed_as, replace).await;
    // Park the (already seeded) goal on an elapsed timer barrier.
    let parked = budgeted(&fx.session_str, OBJECTIVE).with_wait_until(1, None, 0);
    seed(&fx.store, &parked, &seed_as);
    let wake = super::super::goal_wait::GoalWakeService::new(fx.deps(), None, Some(fx.sessions()));
    wake.sweep_once().await;
    assert_eq!(
        fx.reads.load(Ordering::SeqCst),
        1,
        "the wake claim must reach the awaited token read exactly once"
    );
    let request = fx
        .request_for_session(if expect_fire { FIRES } else { QUIET })
        .await;
    (fx, request)
}

#[tokio::test]
async fn wake_replaced_public_by_ordinary_continues_ordinary() {
    let _guard = GOAL_SWEEP_LOCK.lock().await;
    let (_fx, req) = wake_case(
        "race-wake-pub-to-ord",
        Seed::Public(sample_public_context("race-wake-a")),
        Replace::SameId(None),
    )
    .await;
    assert_eq!(req.expect("the wake must fire").telecom_public, None);
}

#[tokio::test]
async fn wake_replaced_ordinary_by_public_continues_public() {
    let _guard = GOAL_SWEEP_LOCK.lock().await;
    let live = sample_public_context("race-wake-b");
    let (_fx, req) = wake_case(
        "race-wake-ord-to-pub",
        Seed::Ordinary,
        Replace::SameId(Some(live.clone())),
    )
    .await;
    assert_eq!(req.expect("the wake must fire").telecom_public, Some(live));
}

#[tokio::test]
async fn wake_drops_the_claim_when_a_different_goal_replaced_it() {
    let _guard = GOAL_SWEEP_LOCK.lock().await;
    let (fx, req) = wake_case(
        "race-wake-other",
        Seed::Public(sample_public_context("race-wake-c")),
        Replace::OtherObjective(None),
    )
    .await;
    assert!(req.is_none(), "{req:?}");
    assert_nothing_claimed(&fx);
}

#[tokio::test]
async fn wake_refuses_an_origin_that_became_untrustworthy_during_the_gate() {
    let _guard = GOAL_SWEEP_LOCK.lock().await;
    let (fx, req) = wake_case(
        "race-wake-corrupt",
        Seed::Ordinary,
        Replace::CorruptOrigin(sample_public_context("race-wake-d")),
    )
    .await;
    assert!(req.is_none(), "{req:?}");
    assert_nothing_claimed(&fx);
}

/// Controls for the wake path with no replacement: public stays public,
/// ordinary and legacy stay ordinary.
#[tokio::test]
async fn wake_through_the_token_gate_keeps_each_origin_kind_unchanged() {
    let _guard = GOAL_SWEEP_LOCK.lock().await;
    let ctx = sample_public_context("race-wake-control");
    let (_fx, req) = wake_case(
        "race-wake-control-public",
        Seed::Public(ctx.clone()),
        Replace::Nothing,
    )
    .await;
    assert_eq!(req.expect("fires").telecom_public, Some(ctx));
    let (_fx, req) = wake_case(
        "race-wake-control-ordinary",
        Seed::Ordinary,
        Replace::Nothing,
    )
    .await;
    assert_eq!(req.expect("fires").telecom_public, None);
    let (_fx, req) = wake_case("race-wake-control-legacy", Seed::Legacy, Replace::Nothing).await;
    assert_eq!(req.expect("fires").telecom_public, None);
}

// ---- census ------------------------------------------------------------

/// The unbound `GoalStore::try_claim_continuation` (session-only) has no
/// production caller outside `goal/store.rs`: an engine that wants a claim must
/// use `try_claim_continuation_bound`, which compares the snapshot's goal
/// identity and returns the live origin under the same lock. A new caller of
/// the unbound form re-opens the snapshot-then-claim gap, so it reds here.
#[test]
fn no_production_code_outside_the_store_calls_the_unbound_claim() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let readers = crate::utils::source_scan::files_whose_production_code_reads(
        &root,
        "try_claim_continuation",
    );
    let outside: Vec<&String> = readers
        .iter()
        .filter(|f| !f.ends_with("goal/store.rs"))
        .collect();
    assert!(
        outside.is_empty(),
        "unbound try_claim_continuation called outside goal/store.rs: {outside:?}"
    );
}

/// `spawn_continuation_run`'s goal confirm must carry the run's origin: the
/// plain `confirm_fire` would let a run proceed whose creator origin changed
/// between the claim and the fire.
#[test]
fn the_goal_confirm_in_spawn_continuation_run_is_origin_bound() {
    use crate::utils::source_scan::{production_prefix, strip_comment_lines};
    let src = include_str!("../execute.rs").replace('\r', "");
    let code = strip_comment_lines(&production_prefix(&src));
    assert!(
        code.contains(".confirm_fire_for_origin("),
        "the goal arm must confirm through confirm_fire_for_origin"
    );
    assert!(
        !code.contains("store.confirm_fire("),
        "a plain goal-store confirm_fire in execute.rs ignores the run's origin"
    );
}
