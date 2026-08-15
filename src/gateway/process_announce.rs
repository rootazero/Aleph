//! Background `bash` completion announce — proactive delivery of finished jobs.
//!
//! A backgrounded command used to finish in silence. `bash`'s spawn receipt says
//! "Started background process N. Poll with …", and polling was the *only* way
//! the result was ever seen: `ProcessRegistry::complete` wrote the entry, woke
//! whoever happened to be parked in `wait`, and stopped there. So a 30-minute
//! `cargo build` kicked off near the end of a run finished into an empty room —
//! the model's "I'll report back when the build is done" was silently broken,
//! and the user heard nothing.
//!
//! That is the same R5 violation [`super::subagent_announce`] closed for
//! background sub-agents, in its own words: *"If the parent's run had already
//! ended, nobody would ever look."* Both kinds of work outlive the run that
//! started them; only one of them had a way home. This module is the other one.
//!
//! The wire: `builtin_tools::bash_exec`'s detached task broadcasts
//! [`AlephEvent::ProcessCompleted`] scoped to the owning session (and
//! `process_journal::init_and_announce` re-broadcasts a completion whose notice
//! died with the previous daemon), and the subscriber here hands it to the
//! shared ladder in [`super::announce_delivery`] — idle parent → a fresh run,
//! mid-run parent → steering absorbed at the next turn boundary, busy parent →
//! bounded retries and then quiet.
//!
//! ## What deliberately does NOT announce
//!
//! * **Killed jobs.** `kill` is the owner's own synchronous action and its
//!   result is already in that tool call's return; the registry's `Killed`
//!   verdict never produces this event. Same stance the sub-agent side takes
//!   for a cancelled child — you asked for it to stop, so its outcome is not
//!   news.
//! * **Shutdown-reaped jobs.** The daemon is going away; there is no turn to
//!   drive and nobody to drive it for.
//! * **Jobs the model already collected.** A terminal `poll` / `wait` stamps
//!   `ProcessRegistry::is_reported`, which is this announcer's dedup predicate.
//!   Spending a whole parent turn to re-state a result the model has already
//!   folded into its context is the cost this check exists to avoid.
//! * **Unowned jobs.** No session (CLI / library callers) means nobody to
//!   announce to; the producer never broadcasts.
//!
//! No reasoning happens here (R10): the harness delivers, the parent agent's own
//! turn decides what the result means.

use crate::builtin_tools::process_journal;
use crate::builtin_tools::process_registry::process_registry;
use crate::event::{AlephEvent, EventType, GlobalEvent};
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::announce_delivery::{self, Announcement};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::sync_primitives::Arc;

/// Where the output remains reachable when the announce is skipped or gives up.
const FALLBACK: &str = "output remains available via bash's poll action";

/// Subscribe to `ProcessCompleted` global events and announce each finished
/// background job into the session that started it.
///
/// Registration is **awaited**, not spawned — see
/// [`announce_delivery::subscribe`]. `process_journal::init_and_announce` runs
/// its boot handback right after this, and a subscriber that is merely
/// *scheduled* is a subscriber that is not listening yet (§9).
pub async fn spawn_process_announce(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    event_bus: Arc<GatewayEventBus>,
) {
    announce_delivery::subscribe(
        EventType::ProcessCompleted,
        "background bash",
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

/// Deliver one finished job into its session.
async fn announce_one(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    event_bus: Arc<GatewayEventBus>,
    global_event: GlobalEvent,
) {
    let AlephEvent::ProcessCompleted(done) = &global_event.event else {
        return;
    };
    let id = done.process_id;

    announce_delivery::deliver(
        adapter,
        registry,
        event_bus,
        Announcement {
            key: id.to_string(),
            metadata_key: "process_announce",
            session_id: global_event.source_session_id.clone(),
            input: notice(done),
            kind: "background bash",
            fallback: FALLBACK,
            // A job the model already collected must not cost a turn. Read
            // against the process-global registry — the same table `poll` and
            // `wait` stamp — and re-read by the ladder before every retry: a
            // parent that is busy is quite often busy inside the very `wait`
            // that collects this job.
            already_delivered: Box::new(move || process_registry().is_reported(id)),
            // Durable "the session knows", so a restart does not re-deliver a
            // notice that already landed.
            on_delivered: Box::new(move || process_journal::record_announced(id)),
        },
    )
    .await;
}

/// The `[system]` notice the parent's turn reads.
///
/// Mirrors the sub-agent notice's shape and its restraint: state what happened,
/// say where the rest is, and hand the turn back. It carries a **tail**, so it
/// says so — a model that thinks it is holding the whole log will report a
/// build as clean because the last line it can see is not an error.
fn notice(done: &crate::event::ProcessCompletionEvent) -> String {
    let id = done.process_id;
    let verdict = if done.success {
        "succeeded".to_string()
    } else {
        format!("failed with exit code {}", done.exit_code)
    };
    let output = if done.output_tail.is_empty() {
        "(the command produced no output)".to_string()
    } else if done.output_truncated {
        format!(
            "Output (END of the output — earlier lines were cut; poll for the full text):\n{}",
            done.output_tail
        )
    } else {
        format!("Output:\n{}", done.output_tail)
    };
    format!(
        "[system] Background process {id} finished: it {verdict}.\n\
         Command: {command}\n{output}\n\n\
         Process this result now: report the outcome to the user in your reply \
         (or to your team leader via team messaging when you work as a team \
         member), and take any follow-up actions the original task implies. Use \
         {{\"process_action\":\"poll\",\"process_id\":{id}}} if you need the full output.",
        command = done.command,
    )
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use tempfile::TempDir;

    use crate::builtin_tools::code_exec::CodeExecOutput;
    use crate::builtin_tools::process_registry::{process_registry, PollOutcome};
    use crate::event::global_bus::GlobalEvent;
    use crate::event::{AlephEvent, ProcessCompletionEvent};
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::execution_adapter::ExecutionAdapter;
    use crate::gateway::execution_engine::{ExecutionError, RunRequest, RunState, RunStatus};
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::SessionStore;
    use crate::sync_primitives::Arc;

    use super::{announce_one, notice};

    /// Records every `execute` call so a test can assert whether a parent turn
    /// was spent at all.
    struct RecordingAdapter {
        calls: std::sync::Mutex<Vec<RunRequest>>,
    }

    impl RecordingAdapter {
        fn new() -> Arc<Self> {
            Arc::new(Self {
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
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request);
            Ok(())
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
    fn sample_event(id: u64) -> GlobalEvent {
        GlobalEvent::for_test(
            "agent:main:peer:user",
            Some("main".into()),
            AlephEvent::ProcessCompleted(ProcessCompletionEvent {
                process_id: id,
                command: "cargo build --release".into(),
                exit_code: 0,
                success: true,
                output_tail: "[stdout]\n    Finished release\n".into(),
                output_truncated: false,
            }),
        )
    }

    /// The happy path: a finished job the model never collected drives exactly
    /// one turn, carrying the id it must poll with.
    #[tokio::test]
    async fn a_finished_job_drives_one_parent_turn() {
        let (registry, _tmp) = registry_with_main_agent().await;
        let adapter = RecordingAdapter::new();
        let event_bus = Arc::new(GatewayEventBus::new());

        // An id the global registry has never issued: nothing to dedup against.
        let id = u64::MAX - 7;
        announce_one(adapter.clone(), registry, event_bus, sample_event(id)).await;

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
                .get("process_announce")
                .map(String::as_str),
            Some(id.to_string().as_str()),
            "the announce metadata must carry the process id so the engine can tag the turn"
        );
        assert!(
            recorded.input.contains(&id.to_string()),
            "the notice must name the id the model polls with"
        );
    }

    /// The dedup: a job whose result the model already pulled through `poll`
    /// must not cost a fresh turn. Uses the process-global registry because
    /// that is exactly the table the production predicate reads — a test
    /// against a private instance would prove nothing about the wiring.
    #[tokio::test]
    async fn a_job_the_model_already_polled_is_not_announced() {
        let reg = process_registry();
        let owner = format!("announce-dedup-{}", uuid::Uuid::new_v4());
        let jh = tokio::spawn(async move {});
        let abort = jh.abort_handle();
        let _ = jh.await;
        let id = match reg.register_running("echo hi", Some(owner.clone()), abort) {
            crate::builtin_tools::process_registry::RegisterOutcome::Registered(id) => id,
            crate::builtin_tools::process_registry::RegisterOutcome::TooManyRunning { limit } => {
                panic!("unexpected per-session cap hit (limit {limit})")
            }
        };
        reg.complete(id, done_output());
        // The model collects it itself — the stamp lives on that read.
        assert!(matches!(reg.poll(id, Some(&owner)), PollOutcome::Done(_)));
        assert!(reg.is_reported(id));

        let (registry, _tmp) = registry_with_main_agent().await;
        let adapter = RecordingAdapter::new();
        let event_bus = Arc::new(GatewayEventBus::new());
        announce_one(adapter.clone(), registry, event_bus, sample_event(id)).await;

        assert_eq!(
            adapter.call_count(),
            0,
            "a result the model already collected must not spend a parent turn"
        );
    }

    /// An unparseable session key is dropped rather than broadcast to whoever
    /// happens to be listening. The job stays poll-able.
    #[tokio::test]
    async fn an_unaddressable_session_is_skipped() {
        let (registry, _tmp) = registry_with_main_agent().await;
        let adapter = RecordingAdapter::new();
        let event_bus = Arc::new(GatewayEventBus::new());

        let mut event = sample_event(u64::MAX - 8);
        event.source_session_id = "not a session key".into();

        announce_one(adapter.clone(), registry, event_bus, event).await;
        assert_eq!(adapter.call_count(), 0);
    }

    /// A truncated tail must say it is a tail. Silence here is how a model
    /// reports a build as clean because the last line it can see is not an
    /// error.
    #[test]
    fn a_truncated_tail_is_labelled_as_the_end_of_the_output() {
        let mut done = ProcessCompletionEvent {
            process_id: 4,
            command: "make".into(),
            exit_code: 1,
            success: false,
            output_tail: "…error: 1 test failed\n".into(),
            output_truncated: true,
        };
        let text = notice(&done);
        assert!(text.contains("earlier lines were cut"), "got: {text}");
        assert!(text.contains("exit code 1"), "got: {text}");

        done.output_truncated = false;
        let whole = notice(&done);
        assert!(!whole.contains("earlier lines were cut"));
    }

    /// A silent job says so, rather than presenting an empty string as output.
    #[test]
    fn a_job_that_printed_nothing_says_so() {
        let done = ProcessCompletionEvent {
            process_id: 5,
            command: "true".into(),
            exit_code: 0,
            success: true,
            output_tail: String::new(),
            output_truncated: false,
        };
        assert!(notice(&done).contains("no output"));
    }

    fn done_output() -> CodeExecOutput {
        CodeExecOutput {
            success: true,
            exit_code: 0,
            stdout: "hi\n".into(),
            stderr: String::new(),
            duration_ms: 1,
            language: "shell".into(),
            truncated: None,
            stdout_truncated_bytes: 0,
            stderr_truncated_bytes: 0,
            advisory: None,
        }
    }
}
