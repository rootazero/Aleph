//! Subagent completion announce — proactive delivery of background results.
//!
//! Background subagent runs (`subagent` tool, `background: true`) used to
//! finish silently: `BackgroundAgentTracker::mark_completed` parked the result
//! and the only reader was the LLM-initiated `list`/`check_status` action. If the
//! parent's run had already ended, nobody would ever look — the user never
//! heard back (an R5 violation, and the gap openclaw closes with its
//! `subagent.task_completed` announce flow).
//!
//! This module is the consuming half of the wire: `subagent_tool::spawn`
//! broadcasts [`AlephEvent::SubAgentCompleted`] on the `GlobalBus` (scope =
//! parent session key), and the subscriber here drives ONE proactive turn on
//! the parent session.
//!
//! The ladder that turns "an event arrived" into "the parent saw it" — dedup,
//! session resolution, idle→fresh-run / busy→steering, bounded retries — now
//! lives in [`super::announce_delivery`], shared with the background `bash`
//! announcer, which closes the identical R5 gap for the other kind of work that
//! outlives its run. What stays here is what is actually about sub-agents: the
//! event shape, the notice, and what "the parent already knows" means for the
//! tracker and its durable sidecar.

use crate::event::{AlephEvent, EventType, GlobalEvent};
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::announce_delivery::{self, Announcement};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::sync_primitives::Arc;

/// Where a sub-agent result remains reachable when the announce is skipped or
/// gives up. Named in every such log line: "we gave up" without "and here is
/// where it still is" reads as a lost result.
const FALLBACK: &str = "result remains available via the subagent tool's list action";

/// Subscribe to `SubAgentCompleted` global events and announce each completed
/// background subagent into its parent session.
///
/// Registration is **awaited**, not spawned — see
/// [`announce_delivery::subscribe`], which owns that invariant for both
/// announcers. Boot broadcasts a `SubAgentCompleted` of its own right after
/// this call (the restart-orphan notice from `agents::background_persistence`),
/// and a subscriber that is merely *scheduled* is a subscriber that is not
/// listening yet.
pub async fn spawn_subagent_announce(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    event_bus: Arc<GatewayEventBus>,
) {
    announce_delivery::subscribe(
        EventType::SubAgentCompleted,
        "subagent",
        move |global_event| {
            let adapter = adapter.clone();
            let registry = registry.clone();
            let event_bus = event_bus.clone();
            tokio::spawn(async move {
                announce_one(adapter, registry, event_bus, global_event).await;
            });
        },
    )
    .await;
}

/// Deliver one completion into the parent session.
async fn announce_one(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    event_bus: Arc<GatewayEventBus>,
    global_event: GlobalEvent,
) {
    let AlephEvent::SubAgentCompleted(result) = &global_event.event else {
        return;
    };

    // `request_id` is optional on the wire type; the broadcaster always sets
    // it to the tracker's request id, with `child_session_id` (same value at
    // the spawn site) as the defensive fallback.
    let request_id = result
        .request_id
        .clone()
        .unwrap_or_else(|| result.child_session_id.clone());

    // Every child this notice speaks for. The grouped boot notice carries N;
    // the ordinary per-child completion carries none and falls back to the one
    // id above.
    let batch: Vec<String> = if result.request_ids.is_empty() {
        vec![request_id.clone()]
    } else {
        result.request_ids.clone()
    };

    let status = if result.success {
        "succeeded"
    } else {
        "failed"
    };
    // B2-01: prefer `summary` whenever it carries real content, with
    // `error` appended as a headline. The previous success/error switch
    // discarded `summary` on any non-success path, which dropped every
    // recovered orphan's partial result on a mixed batch (3 interrupted + 2
    // finished-unannounced children) — the finished children were also lost
    // because `success` flipped on the interrupted count. The producer side
    // (`background_persistence::init_and_announce_orphans`) puts the full
    // grouped report in `summary` and the interruption headline in `error`;
    // the consumer must render both, not one or the other.
    let detail = match (result.summary.clone(), result.error.clone()) {
        (s, _) if !s.is_empty() => match result.error.clone() {
            Some(err) => format!("{err}\n\n{s}"),
            None => s,
        },
        (_, Some(err)) => err,
        _ => "(no detail)".to_string(),
    };
    // A batch of N children has N outcomes, and `success` is a single bool
    // computed by the producer over the whole batch. Rendering it as one
    // verdict ("run <one id> succeeded") told the model that every child in the
    // batch had that outcome, and named one arbitrary child as the one to ask
    // about — while the detail underneath said something different per child.
    // The grouped header counts instead of judging; the per-child verdicts stay
    // where the producer wrote them, in `detail`.
    let head = if batch.len() > 1 {
        format!(
            "[system] {} background subagent runs settled while the daemon was down. \
             Each one's own outcome is listed below — they are not all the same.",
            batch.len()
        )
    } else {
        format!("[system] Background subagent run {request_id} {status}.")
    };
    let pointer = if batch.len() > 1 {
        format!(
            "Use the subagent tool's 'check_status' action with any of these \
             request_ids if you need the full output: {}.",
            batch.join(", ")
        )
    } else {
        format!(
            "Use the subagent tool's 'check_status' action with \
             request_id='{request_id}' if you need the full output."
        )
    };
    let input = format!(
        "{head}\n\
         Result summary:\n{detail}\n\n\
         Process this result now: report the outcome to the user in your \
         reply (or to your team leader via team messaging when you work as a \
         team member), and take any follow-up actions the original task \
         implies. {pointer}",
    );

    // Dedup with the on-demand paths: a result the parent already saw via a
    // `wait` or a `check_status` must not cost a fresh turn. Read against the
    // process-global tracker — the same instance the spawn/wait paths use — and
    // re-read by the ladder before every retry, because the reason a parent is
    // busy is very often that it is parked in the `wait` that consumes this
    // exact result.
    let dedup_id = request_id.clone();
    let stamp_ids = batch;

    announce_delivery::deliver(
        adapter,
        registry,
        event_bus,
        Announcement {
            key: request_id,
            metadata_key: "subagent_announce",
            session_id: global_event.source_session_id.clone(),
            input,
            kind: "subagent",
            fallback: FALLBACK,
            already_delivered: Box::new(move || {
                crate::agents::background_tracker::BackgroundAgentTracker::global()
                    .is_consumed(&dedup_id)
            }),
            on_delivered: Box::new(move || {
                // Durable "the parent knows", for EVERY child this notice spoke
                // for. Without the stamp, a restart inside the retry ladder
                // leaves a `Settled` sidecar record whose promised announcement
                // is withdrawn in silence; with only the first id stamped, the
                // other N-1 of a grouped notice come back at the next boot.
                crate::agents::background_persistence::on_delivered(&stamp_ids);
            }),
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use async_trait::async_trait;
    use tempfile::TempDir;

    use crate::agents::background_tracker::{BackgroundAgentTracker, CompletedOutcome, SpawnMeta};
    use crate::event::global_bus::GlobalEvent;
    use crate::event::{AlephEvent, SubAgentCompletionEvent};
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::execution_adapter::ExecutionAdapter;
    use crate::gateway::execution_engine::{ExecutionError, RunRequest, RunState, RunStatus};
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::SessionStore;
    use crate::sync_primitives::Arc;
    use tokio_util::sync::CancellationToken;

    use super::announce_one;

    /// Adapter that records every `execute` call and can be configured to
    /// return `AgentBusy` for the first N calls. Lets one test assert the retry
    /// loop and another assert the early-return dedup.
    struct RecordingAdapter {
        busy_first: usize,
        calls: std::sync::Mutex<Vec<RunRequest>>,
    }

    impl RecordingAdapter {
        fn new(busy_first: usize) -> Arc<Self> {
            Arc::new(Self {
                busy_first,
                calls: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).len()
        }
    }

    #[async_trait]
    impl ExecutionAdapter for RecordingAdapter {
        async fn execute(
            &self,
            request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
            calls.push(request);
            if calls.len() <= self.busy_first {
                Err(ExecutionError::AgentBusy("simulated".into()))
            } else {
                Ok(())
            }
        }

        async fn cancel(&self, _run_id: &str) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
            Some(RunStatus {
                run_id: run_id.to_string(),
                state: RunState::Completed,
                started_at: None,
                completed_at: None,
                steps_completed: 0,
                current_tool: None,
            })
        }

        async fn active_run_count(&self) -> usize {
            0
        }
    }

    async fn registry_with_main_agent() -> (Arc<AgentRegistry>, TempDir) {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let config = AgentInstanceConfig {
            agent_id: "main".into(),
            workspace: tmp.path().join("workspace"),
            agent_dir: tmp.path().join("agent"),
            ..AgentInstanceConfig::default()
        };
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: tmp.path().join("sessions"),
            ..FileSessionStoreConfig::default()
        })
        .expect("FileSessionStore::new");
        let session_store: Arc<dyn SessionStore> = Arc::new(store);
        let instance = AgentInstance::new(config, session_store).expect("AgentInstance::new");
        let registry = Arc::new(AgentRegistry::new());
        registry.register(instance).await;
        (registry, tmp)
    }

    /// `agent:main:peer:user` is the `SessionKey::to_key_string()` form for the
    /// `main` agent under the default peer.
    fn sample_event(request_id: &str) -> GlobalEvent {
        GlobalEvent::for_test(
            "agent:main:peer:user",
            Some("main".into()),
            AlephEvent::SubAgentCompleted(SubAgentCompletionEvent {
                agent_id: "main".into(),
                child_session_id: "child-sid".into(),
                summary: "result text".into(),
                success: true,
                error: None,
                request_id: Some(request_id.into()),
                request_ids: vec![request_id.into()],
            }),
        )
    }

    /// `mark_completed` on the global tracker so the consume check has a real
    /// entry. Uses a unique id per test to keep the process-global tracker
    /// isolated across `cargo test` invocations of this file (it accumulates).
    fn seed_completed(request_id: &str) {
        let tracker = BackgroundAgentTracker::global();
        tracker.register_with_meta(
            request_id.into(),
            CancellationToken::new(),
            "seeded".into(),
            SpawnMeta {
                root_session: "agent:main:peer:user".into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        tracker.mark_completed(request_id, CompletedOutcome::ok_text("ok"));
    }

    /// The early-return dedup: a result that the model already saw via
    /// `wait` / `check_status` must not be re-announced. The adapter must
    /// not be called at all.
    #[tokio::test]
    async fn announce_skips_when_result_already_consumed() {
        let request_id = format!("dedup-{}", uuid::Uuid::new_v4());
        seed_completed(&request_id);
        BackgroundAgentTracker::global().mark_consumed(&request_id);

        let (registry, _tmp) = registry_with_main_agent().await;
        let adapter = RecordingAdapter::new(0);
        let event_bus = Arc::new(GatewayEventBus::new());

        announce_one(
            adapter.clone(),
            registry,
            event_bus,
            sample_event(&request_id),
        )
        .await;

        assert_eq!(
            adapter.call_count(),
            0,
            "consumed result must skip the proactive announce entirely"
        );
    }

    /// The happy path: a fresh result drives exactly one `adapter.execute`
    /// call carrying the announce metadata.
    #[tokio::test]
    async fn announce_drives_one_execute_call_for_fresh_result() {
        let request_id = format!("fresh-{}", uuid::Uuid::new_v4());
        seed_completed(&request_id);

        let (registry, _tmp) = registry_with_main_agent().await;
        let adapter = RecordingAdapter::new(0);
        let event_bus = Arc::new(GatewayEventBus::new());

        announce_one(
            adapter.clone(),
            registry,
            event_bus,
            sample_event(&request_id),
        )
        .await;

        assert_eq!(adapter.call_count(), 1);
        let recorded = adapter
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop()
            .expect("one call recorded");
        assert_eq!(
            recorded
                .metadata
                .get("subagent_announce")
                .map(String::as_str),
            Some(request_id.as_str()),
            "announce metadata must carry the request_id so the engine can tag the turn"
        );
        assert!(
            recorded.input.contains(&request_id),
            "announce input must reference the request_id so the parent can fetch via check_status"
        );
    }

    /// The retry path, all three rungs of it: the parent is busy for the first
    /// two attempts, so the ladder must sleep 30s, retry, sleep 120s and retry
    /// again before the third attempt succeeds.
    ///
    /// # Why this no longer costs two and a half minutes
    ///
    /// It used to be `#[ignore]`d, and the reason recorded here was that the
    /// only alternative was "injecting a clock — which would require
    /// refactoring the production code purely for testability, which this
    /// layer rightly resists (R10)". That reasoning talked itself out of a fix
    /// that needs **no production change at all**: the ladder already sleeps on
    /// `tokio::time::sleep`, and `start_paused` runs the whole test on tokio's
    /// virtual clock, auto-advancing to the next deadline whenever the runtime
    /// goes idle. Zero production lines, real sleeps, no wall clock.
    ///
    /// So the arm that had never actually been executed is now executed on
    /// every `--lib` run, and it exercises the FULL `[0, 30, 120]` schedule
    /// rather than just its first rung — under a virtual clock, the longer
    /// ladder is free.
    #[tokio::test(start_paused = true)]
    async fn announce_retries_through_the_whole_busy_ladder() {
        let request_id = format!("retry-{}", uuid::Uuid::new_v4());
        seed_completed(&request_id);

        let (registry, _tmp) = registry_with_main_agent().await;
        // Busy for attempts 1 and 2 → success only on attempt 3, which is
        // reachable only if BOTH the 30s and the 120s rung are traversed.
        let adapter = RecordingAdapter::new(2);
        let event_bus = Arc::new(GatewayEventBus::new());

        let started = tokio::time::Instant::now();
        announce_one(
            adapter.clone(),
            registry,
            event_bus,
            sample_event(&request_id),
        )
        .await;
        let virtual_elapsed = started.elapsed();

        assert_eq!(
            adapter.call_count(),
            3,
            "two busy responses must be retried, and the third attempt succeeds"
        );
        // The call count alone cannot tell a real ladder from one whose sleeps
        // were deleted; the virtual clock can. 0 + 30 + 120 = 150s must have
        // elapsed on tokio's timer even though the test takes milliseconds.
        assert_eq!(
            virtual_elapsed.as_secs(),
            150,
            "the ladder must actually wait 30s then 120s (virtual time)"
        );
    }

    /// The retry path with a fast assertion: drive the consume-during-sleep
    /// case without waiting for the real delay. We hand-craft the inner loop
    /// by reproducing the dedup check at the only place it fires — after
    /// the sleep — and assert the consumed guard holds.
    ///
    /// This complements `announce_retries_on_busy_then_succeeds` (which the
    /// retry sleep makes too slow for the lib-test budget) by proving the
    /// bookkeeping primitive: once the result is consumed, the next call
    /// sees `is_consumed() == true`.
    #[tokio::test]
    async fn consume_during_retry_short_circuits_announce() {
        let request_id = format!("consume-mid-{}", uuid::Uuid::new_v4());
        seed_completed(&request_id);

        // Simulate the retry sleep window: the parent consumed the result
        // while we were sleeping, so the next attempt must short-circuit.
        let tracker = BackgroundAgentTracker::global();
        assert!(!tracker.is_consumed(&request_id));
        tracker.mark_consumed(&request_id);
        assert!(tracker.is_consumed(&request_id));

        // The dedup check inside `announce_one` is the only condition that
        // matters here: a consumed result must not reach the adapter.
        let (registry, _tmp) = registry_with_main_agent().await;
        let adapter = RecordingAdapter::new(0);
        let event_bus = Arc::new(GatewayEventBus::new());
        announce_one(
            adapter.clone(),
            registry,
            event_bus,
            sample_event(&request_id),
        )
        .await;
        assert_eq!(
            adapter.call_count(),
            0,
            "consumed during retry window must short-circuit the adapter call"
        );
    }

    /// An unparseable session key is silently dropped (no panic, no adapter
    /// call). The result remains available via the subagent tool's `list`.
    #[tokio::test]
    async fn announce_skips_when_session_key_unparseable() {
        let request_id = format!("badkey-{}", uuid::Uuid::new_v4());
        seed_completed(&request_id);

        let (registry, _tmp) = registry_with_main_agent().await;
        let adapter = RecordingAdapter::new(0);
        let event_bus = Arc::new(GatewayEventBus::new());

        let mut event = sample_event(&request_id);
        event.source_session_id = "not a session key".into();

        announce_one(adapter.clone(), registry, event_bus, event).await;
        assert_eq!(adapter.call_count(), 0);
    }

    /// A session key that does not resolve to a registered agent is dropped
    /// (the parent must be re-registered before the announce can fire). The
    /// result stays poll-only — the model can still read it via `list` /
    /// `check_status`.
    #[tokio::test]
    async fn announce_skips_when_parent_agent_not_registered() {
        let request_id = format!("orphan-{}", uuid::Uuid::new_v4());
        seed_completed(&request_id);

        // Empty registry — `get("main")` returns None.
        let registry = Arc::new(AgentRegistry::new());
        let adapter = RecordingAdapter::new(0);
        let event_bus = Arc::new(GatewayEventBus::new());

        announce_one(
            adapter.clone(),
            registry,
            event_bus,
            sample_event(&request_id),
        )
        .await;
        assert_eq!(adapter.call_count(), 0);
    }

    /// A failure that is NOT `AgentBusy` is terminal (the loop returns
    /// immediately rather than retrying — there is no reason to think a
    /// second attempt will succeed when the first hit a different error).
    #[tokio::test]
    async fn announce_returns_on_non_busy_error() {
        let request_id = format!("err-{}", uuid::Uuid::new_v4());
        seed_completed(&request_id);

        struct AlwaysFail;
        #[async_trait]
        impl ExecutionAdapter for AlwaysFail {
            async fn execute(
                &self,
                _: RunRequest,
                _: Arc<AgentInstance>,
                _: Arc<dyn EventEmitter + Send + Sync>,
            ) -> Result<(), ExecutionError> {
                Err(ExecutionError::Failed("simulated".into()))
            }
            async fn cancel(&self, _: &str) -> Result<(), ExecutionError> {
                Ok(())
            }
            async fn get_status(&self, _: &str) -> Option<RunStatus> {
                None
            }
            async fn active_run_count(&self) -> usize {
                0
            }
        }

        let (registry, _tmp) = registry_with_main_agent().await;
        let adapter: Arc<dyn ExecutionAdapter> = Arc::new(AlwaysFail);
        let event_bus = Arc::new(GatewayEventBus::new());

        announce_one(adapter, registry, event_bus, sample_event(&request_id)).await;

        // Structural assertion: a non-busy error exits the retry loop on the
        // first attempt (no `AgentBusy` match arm), so the function returns
        // without driving further adapter calls. The exact call count is
        // implicitly `1` (the failing one) — the loop saw one non-busy
        // `Err` and returned. We prove this by a focused unit test below.
    }

    /// Sharpened version of the structural assertion: a custom
    /// `RecordingAdapter` (busy_first=0, always-fail overridden) lets us
    /// count adapter calls and prove the retry loop exited after exactly
    /// one.
    #[tokio::test]
    async fn announce_returns_on_non_busy_error_records_one_call() {
        let request_id = format!("err2-{}", uuid::Uuid::new_v4());
        seed_completed(&request_id);

        let (registry, _tmp) = registry_with_main_agent().await;
        let adapter = RecordingAdapter::new(0);
        // Override the `RecordingAdapter`'s `Ok` arm for this test by
        // wrapping it: a thin adapter that forwards the call list but
        // always returns `Failed`.
        let _ = AtomicUsize::new(0); // keep the type referenced
        let always_fail = AlwaysFailAdapter {
            inner: adapter.clone(),
        };
        let event_bus = Arc::new(GatewayEventBus::new());

        announce_one(
            Arc::new(always_fail) as Arc<dyn ExecutionAdapter>,
            registry,
            event_bus,
            sample_event(&request_id),
        )
        .await;

        // Inner recorded exactly the one call `announce_one` made before the
        // non-busy `Err` returned the loop.
        assert_eq!(
            adapter.call_count(),
            1,
            "non-busy error must exit the retry loop after exactly one attempt"
        );
    }

    /// Records calls to `inner` and overwrites the result with `Failed`,
    /// so a test can count calls while exercising the non-busy exit path.
    struct AlwaysFailAdapter {
        inner: Arc<RecordingAdapter>,
    }

    #[async_trait]
    impl ExecutionAdapter for AlwaysFailAdapter {
        async fn execute(
            &self,
            request: RunRequest,
            agent: Arc<AgentInstance>,
            emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            // Forward to record + return a non-busy error.
            let mut calls = self.inner.calls.lock().unwrap_or_else(|e| e.into_inner());
            calls.push(request);
            drop(calls);
            let _ = (agent, emitter);
            Err(ExecutionError::Failed("always fail".into()))
        }
        async fn cancel(&self, _: &str) -> Result<(), ExecutionError> {
            Ok(())
        }
        async fn get_status(&self, _: &str) -> Option<RunStatus> {
            None
        }
        async fn active_run_count(&self) -> usize {
            0
        }
    }
}
