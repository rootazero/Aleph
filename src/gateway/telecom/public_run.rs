//! Public telecom SMS run driver (public session isolation contract §4–§6).
//!
//! Admission → (isolated history load) → isolated runner → acknowledged
//! buffered reply → (history append). Owns no business logic beyond that sequencing:
//! the reply text comes from the runner, refusals are fixed notices. The
//! optional [`PublicHistoryStore`] is the only cross-turn state; without one
//! every run is a single, history-free turn.

use std::time::Duration;

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::public_admission::{PublicAdmission, PublicRunRegistry, PUBLIC_BUSY_NOTICE};
use super::public_history::{PublicHistorySession, PublicHistoryStore, PublicHistoryTurn};
use super::TelecomPublicContext;
use crate::orchestrator::dispatch::{
    flow_event_channel, HarnessRunner, PublicRunRequest, PUBLIC_PROMPT_VERSION,
};

/// Delivery seam of the single public reply (the channel reply path in
/// production, a spy in tests). `Ok(())` acknowledges transport acceptance,
/// not receipt by the caller's handset. An error or timeout may leave the
/// external outcome unknown; neither is evidence of a completed public turn.
#[async_trait]
pub trait PublicReplySink: Send + Sync {
    async fn send_reply(&self, text: String) -> Result<(), crate::gateway::channel::ChannelError>;

    /// Fence cancellation before submission, then await the transport ACK even
    /// if cancellation fires. Legacy sinks commit when `send_reply` is first
    /// polled; sinks with internal pre-submit waits must override this method
    /// and fence those waits too. Never race cancellation against a committed send.
    async fn send_reply_if_not_cancelled(
        &self,
        text: String,
        cancel: &CancellationToken,
    ) -> Result<PublicReplyOutcome, crate::gateway::channel::ChannelError> {
        if cancel.is_cancelled() {
            return Ok(PublicReplyOutcome::Cancelled);
        }
        self.send_reply(text).await?;
        Ok(PublicReplyOutcome::Acknowledged)
    }
}

/// Cancellation is affirmative evidence of no submission, not a transport error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicReplyOutcome {
    Acknowledged,
    Cancelled,
}

/// What happened to one public inbound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicRunDisposition {
    /// The sink acknowledged transport acceptance of the runner's reply.
    Replied,
    /// Refused at admission; the sink acknowledged the fixed notice.
    Refused(&'static str),
    /// Cancelled (`/stop`) before reply submission; no reply submitted.
    Cancelled,
    /// No acknowledged reply: runner failure, empty reply, sink error or timeout.
    /// An in-flight send may already have been accepted externally.
    Failed,
}

/// Wall-clock bound of one public runner invocation. A hung provider is
/// cancelled and its admission released after this long; nothing is sent.
/// Mirrors `DEFAULT_RUN_TIMEOUT_SECS` of subagent runs.
pub const PUBLIC_RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Wall-clock bound of delivering the single reply while the binding is held.
/// Holding the permit through delivery must not let a hung sink pin admission.
pub const PUBLIC_REPLY_DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Drive one public SMS run. `harness == None` (no orchestrator wired) is
/// refused with the busy notice rather than falling back to the owner path.
///
/// `history`, when present, supplies the binding's completed public turns
/// before the run and records this one only after the sink acknowledges it. A
/// history that cannot be read (wrong key, corruption) fails the run closed:
/// the runner is not invoked and nothing is sent. A failed *append* (e.g. the
/// store is at capacity) only means the turn is not remembered.
///
/// The binding permit is held through reply delivery, and `/stop` is
/// fenced by the sink through its pre-submit waits, so a cancellation
/// observed before the commit boundary suppresses the reply and history append.
/// Once submitted, `/stop` cannot retract transport acceptance: an eventual
/// `Ok(())` is still recorded, while an error or timeout is never recorded.
pub async fn run_public_sms(
    registry: &Arc<PublicRunRegistry>,
    harness: Option<Arc<dyn HarnessRunner>>,
    history: Option<&PublicHistoryStore>,
    ctx: TelecomPublicContext,
    text: String,
    sink: &dyn PublicReplySink,
) -> PublicRunDisposition {
    run_public_sms_bounded(
        registry,
        harness,
        history,
        ctx,
        text,
        sink,
        PUBLIC_RUN_TIMEOUT,
        PUBLIC_REPLY_DELIVERY_TIMEOUT,
    )
    .await
}

/// Share the acknowledgment boundary across replies and refusal notices.
/// Do not log channel errors: transport diagnostics can contain bodies or phones.
async fn send_reply_bounded(
    sink: &dyn PublicReplySink,
    text: String,
    delivery_timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<PublicReplyOutcome, ()> {
    let delivery = async {
        match cancel {
            Some(cancel) => sink.send_reply_if_not_cancelled(text, cancel).await,
            None => sink
                .send_reply(text)
                .await
                .map(|()| PublicReplyOutcome::Acknowledged),
        }
    };
    match tokio::time::timeout(delivery_timeout, delivery).await {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(_)) => {
            tracing::warn!("[public] reply transport did not acknowledge delivery");
            Err(())
        }
        Err(_) => {
            tracing::warn!(
                "[public] reply delivery exceeded {delivery_timeout:?}; outcome unknown"
            );
            Err(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_public_sms_bounded(
    registry: &Arc<PublicRunRegistry>,
    harness: Option<Arc<dyn HarnessRunner>>,
    history: Option<&PublicHistoryStore>,
    ctx: TelecomPublicContext,
    text: String,
    sink: &dyn PublicReplySink,
    run_timeout: Duration,
    delivery_timeout: Duration,
) -> PublicRunDisposition {
    let admission = registry.try_admit(&ctx);
    let permit = match admission {
        PublicAdmission::Admitted(p) => p,
        refused => {
            let notice = refused.notice().unwrap_or(PUBLIC_BUSY_NOTICE);
            return if send_reply_bounded(sink, notice.to_string(), delivery_timeout, None)
                .await
                .is_ok()
            {
                PublicRunDisposition::Refused(notice)
            } else {
                PublicRunDisposition::Failed
            };
        }
    };
    let Some(harness) = harness else {
        let disposition =
            if send_reply_bounded(sink, PUBLIC_BUSY_NOTICE.to_string(), delivery_timeout, None)
                .await
                .is_ok()
            {
                PublicRunDisposition::Refused(PUBLIC_BUSY_NOTICE)
            } else {
                PublicRunDisposition::Failed
            };
        drop(permit);
        return disposition;
    };
    let cancel = permit.cancel_token();
    let history_key = history.map(|_| PublicHistorySession::from_provenance(ctx.provenance()));
    let prior = match (history, &history_key) {
        (Some(store), Some(key)) => match store.load(key, chrono::Utc::now()) {
            Ok(turns) => turns,
            Err(e) => {
                tracing::warn!("[public] history unavailable, run refused closed: {e}");
                drop(permit);
                return PublicRunDisposition::Failed;
            }
        },
        _ => Vec::new(),
    };
    let (events, _rx) = flow_event_channel();
    let user_text = history.map(|_| text.clone());
    let request = PublicRunRequest {
        ctx,
        text,
        history: prior,
        run_id: uuid::Uuid::new_v4().to_string(),
        prompt_version: PUBLIC_PROMPT_VERSION,
    };
    let result = match tokio::time::timeout(
        run_timeout,
        harness.run_public_text(request, cancel.clone(), events),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            // Hung provider: stop the run, release admission, say nothing.
            tracing::warn!("[public] run exceeded {run_timeout:?}; cancelled, no reply");
            cancel.cancel();
            drop(permit);
            return PublicRunDisposition::Failed;
        }
    };
    if let Ok(outcome) = &result {
        registry.record_spend(u64::from(outcome.total_tokens));
    }
    // The permit stays held from here through delivery and the history
    // append: `/stop` keeps finding this run until the sink acknowledges, and
    // a follow-up cannot load a stale history snapshot.
    let disposition = match result {
        // Early exit only; the sink owns the final pre-submit cancellation fence.
        _ if cancel.is_cancelled() => PublicRunDisposition::Cancelled,
        Ok(outcome) if !outcome.final_text.trim().is_empty() => {
            let reply = outcome.final_text;
            match send_reply_bounded(sink, reply.clone(), delivery_timeout, Some(&cancel)).await {
                Ok(PublicReplyOutcome::Acknowledged) => {}
                Ok(PublicReplyOutcome::Cancelled) => {
                    drop(permit);
                    return PublicRunDisposition::Cancelled;
                }
                Err(()) => {
                    drop(permit);
                    return PublicRunDisposition::Failed;
                }
            }
            // Only an explicit sink acknowledgment completes a turn. A `/stop`
            // after submission cannot undo an externally accepted reply.
            if let (Some(store), Some(key), Some(user)) = (history, &history_key, user_text) {
                let turn = PublicHistoryTurn {
                    user,
                    assistant: reply.trim().to_string(),
                };
                if let Err(e) = store.append(key, &turn, chrono::Utc::now()) {
                    tracing::warn!("[public] history append skipped: {e}");
                }
            }
            PublicRunDisposition::Replied
        }
        Err(crate::orchestrator::errors::FlowError::Cancelled) => PublicRunDisposition::Cancelled,
        _ => PublicRunDisposition::Failed,
    };
    drop(permit);
    disposition
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::telecom::public_admission::PUBLIC_OVER_SPEND_NOTICE;
    use crate::gateway::telecom::public_context::test_support::sample_public_context;
    use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent};
    use crate::orchestrator::errors::FlowError;
    use crate::orchestrator::flow_spec::{FlowInput, FlowSpec};
    use crate::sync_primitives::Mutex;
    use tokio::sync::{broadcast, Notify};
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct SpySink {
        sent: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PublicReplySink for SpySink {
        async fn send_reply(
            &self,
            text: String,
        ) -> Result<(), crate::gateway::channel::ChannelError> {
            self.sent.lock().unwrap().push(text);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingSink {
        attempts: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PublicReplySink for FailingSink {
        async fn send_reply(
            &self,
            text: String,
        ) -> Result<(), crate::gateway::channel::ChannelError> {
            self.attempts.lock().unwrap().push(text);
            Err(crate::gateway::channel::ChannelError::SendFailed(
                "untrusted diagnostic with body/phone must not be logged".into(),
            ))
        }
    }

    /// Runner spy: counts owner `run` calls (must stay 0) and serves the
    /// public path with a fixed reply, optionally blocking until released.
    struct SpyRunner {
        owner_runs: Mutex<u32>,
        public_runs: Mutex<Vec<String>>,
        seen_history: Mutex<Vec<Vec<PublicHistoryTurn>>>,
        gate: Option<Arc<Notify>>,
        entered: Arc<Notify>,
        reply: Result<&'static str, ()>,
    }

    impl SpyRunner {
        fn new(reply: Result<&'static str, ()>, gate: Option<Arc<Notify>>) -> Arc<Self> {
            Arc::new(Self {
                owner_runs: Mutex::new(0),
                public_runs: Mutex::new(Vec::new()),
                seen_history: Mutex::new(Vec::new()),
                gate,
                entered: Arc::new(Notify::new()),
                reply,
            })
        }
    }

    #[async_trait]
    impl HarnessRunner for SpyRunner {
        async fn run(
            &self,
            _session_key: String,
            _spec: Arc<FlowSpec>,
            _input: FlowInput,
            _sandbox: Arc<dyn crate::sandbox::Sandbox>,
            _events: broadcast::Sender<FlowStreamEvent>,
            _cancel: CancellationToken,
            _tool_service_override: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
            _trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
            _interaction_manifest: Option<crate::thinker::InteractionManifest>,
            _workspace_override: Option<std::path::PathBuf>,
            _max_iterations_override: Option<u32>,
            _transient_context: Option<String>,
            _think_level: Option<crate::agents::thinking::ThinkLevel>,
            _envelope: crate::thinker::TurnEnvelope,
            _turn_model: Option<crate::providers::session_model_handle::SessionModelPref>,
            _run_id: String,
            _telecom_public: Option<TelecomPublicContext>,
        ) -> Result<FlowOutcome, FlowError> {
            *self.owner_runs.lock().unwrap() += 1;
            Ok(FlowOutcome::default())
        }

        async fn run_public_text(
            &self,
            request: PublicRunRequest,
            cancel: CancellationToken,
            _events: broadcast::Sender<FlowStreamEvent>,
        ) -> Result<FlowOutcome, FlowError> {
            self.public_runs.lock().unwrap().push(request.text.clone());
            self.seen_history
                .lock()
                .unwrap()
                .push(request.history.clone());
            self.entered.notify_one();
            if let Some(gate) = &self.gate {
                tokio::select! {
                    _ = gate.notified() => {}
                    _ = cancel.cancelled() => return Err(FlowError::Cancelled),
                }
            }
            match self.reply {
                Ok(t) => Ok(FlowOutcome {
                    final_text: t.to_string(),
                    total_tokens: 10,
                    ..FlowOutcome::default()
                }),
                Err(()) => Err(FlowError::Internal("boom".into())),
            }
        }
    }

    #[tokio::test]
    async fn exactly_one_reply_and_no_owner_run() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let runner = SpyRunner::new(Ok("answer"), None);
        let sink = SpySink::default();
        let d = run_public_sms(
            &reg,
            Some(runner.clone() as Arc<dyn HarnessRunner>),
            None,
            sample_public_context("bind-a"),
            "/btw hello".into(),
            &sink,
        )
        .await;
        assert_eq!(d, PublicRunDisposition::Replied);
        assert_eq!(*sink.sent.lock().unwrap(), vec!["answer".to_string()]);
        assert_eq!(*runner.owner_runs.lock().unwrap(), 0);
        assert_eq!(
            *runner.public_runs.lock().unwrap(),
            vec!["/btw hello".to_string()],
            "slash text reaches the public runner verbatim as plain text"
        );
        assert_eq!(reg.active_count(), 0);
        assert_eq!(reg.tokens_spent(), 10);
    }

    #[tokio::test]
    async fn failure_sends_nothing() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let runner = SpyRunner::new(Err(()), None);
        let sink = SpySink::default();
        let d = run_public_sms(
            &reg,
            Some(runner as Arc<dyn HarnessRunner>),
            None,
            sample_public_context("bind-a"),
            "hi".into(),
            &sink,
        )
        .await;
        assert_eq!(d, PublicRunDisposition::Failed);
        assert!(sink.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_same_binding_gets_busy_notice_and_cancel_sends_nothing() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let gate = Arc::new(Notify::new());
        let runner = SpyRunner::new(Ok("late answer"), Some(gate));
        let sink = Arc::new(SpySink::default());

        let first = {
            let reg = reg.clone();
            let runner = runner.clone();
            let sink = sink.clone();
            tokio::spawn(async move {
                run_public_sms(
                    &reg,
                    Some(runner as Arc<dyn HarnessRunner>),
                    None,
                    sample_public_context("bind-a"),
                    "first".into(),
                    sink.as_ref(),
                )
                .await
            })
        };
        runner.entered.notified().await;

        let second = run_public_sms(
            &reg,
            Some(runner.clone() as Arc<dyn HarnessRunner>),
            None,
            sample_public_context("bind-a"),
            "second".into(),
            sink.as_ref(),
        )
        .await;
        assert_eq!(second, PublicRunDisposition::Refused(PUBLIC_BUSY_NOTICE));

        assert!(reg.cancel_binding("bind-a"));
        assert_eq!(first.await.unwrap(), PublicRunDisposition::Cancelled);
        assert_eq!(
            *sink.sent.lock().unwrap(),
            vec![PUBLIC_BUSY_NOTICE.to_string()],
            "only the busy notice; the cancelled run delivers nothing"
        );
        assert_eq!(reg.active_count(), 0);
    }

    #[tokio::test]
    async fn over_spend_gets_fixed_notice_without_running() {
        let reg = PublicRunRegistry::new(4, 5);
        reg.record_spend(5);
        let runner = SpyRunner::new(Ok("x"), None);
        let sink = SpySink::default();
        let d = run_public_sms(
            &reg,
            Some(runner.clone() as Arc<dyn HarnessRunner>),
            None,
            sample_public_context("bind-a"),
            "hi".into(),
            &sink,
        )
        .await;
        assert_eq!(d, PublicRunDisposition::Refused(PUBLIC_OVER_SPEND_NOTICE));
        assert!(runner.public_runs.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_harness_refuses_instead_of_owner_fallback() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let sink = SpySink::default();
        let d = run_public_sms(
            &reg,
            None,
            None,
            sample_public_context("bind-a"),
            "hi".into(),
            &sink,
        )
        .await;
        assert_eq!(d, PublicRunDisposition::Refused(PUBLIC_BUSY_NOTICE));
        assert_eq!(reg.active_count(), 0);
    }

    use crate::gateway::telecom::public_history::{PublicHistoryConfig, PublicHistoryStore};
    use crate::secrets::crypto::SecretsCrypto;

    fn history_store(key: &str) -> PublicHistoryStore {
        PublicHistoryStore::open_in_memory(SecretsCrypto::new(key), PublicHistoryConfig::default())
            .unwrap()
    }

    async fn run_with(
        reg: &Arc<PublicRunRegistry>,
        runner: &Arc<SpyRunner>,
        store: Option<&PublicHistoryStore>,
        binding: &str,
        text: &str,
    ) -> (PublicRunDisposition, Vec<String>) {
        let sink = SpySink::default();
        let d = run_public_sms(
            reg,
            Some(runner.clone() as Arc<dyn HarnessRunner>),
            store,
            sample_public_context(binding),
            text.into(),
            &sink,
        )
        .await;
        let sent = sink.sent.lock().unwrap().clone();
        (d, sent)
    }

    #[tokio::test]
    async fn second_turn_sees_first_and_other_binding_does_not() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let store = history_store("k");
        let r1 = SpyRunner::new(Ok("reply-one-amber"), None);
        run_with(&reg, &r1, Some(&store), "bind-a", "q-one-lilac").await;
        assert!(r1.seen_history.lock().unwrap()[0].is_empty());

        let r2 = SpyRunner::new(Ok("reply-two"), None);
        run_with(&reg, &r2, Some(&store), "bind-a", "q-two").await;
        assert_eq!(
            r2.seen_history.lock().unwrap()[0],
            vec![PublicHistoryTurn {
                user: "q-one-lilac".into(),
                assistant: "reply-one-amber".into()
            }]
        );
        assert_eq!(*r2.owner_runs.lock().unwrap(), 0);

        let r3 = SpyRunner::new(Ok("reply-three"), None);
        run_with(&reg, &r3, Some(&store), "bind-b", "q-three").await;
        assert!(
            r3.seen_history.lock().unwrap()[0].is_empty(),
            "binding isolation"
        );
    }

    #[tokio::test]
    async fn failed_delivery_does_not_complete_history_and_releases_admission() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let store = history_store("k");
        let runner = SpyRunner::new(Ok("unacknowledged answer"), None);
        let sink = FailingSink::default();
        let d = run_public_sms(
            &reg,
            Some(runner.clone() as Arc<dyn HarnessRunner>),
            Some(&store),
            sample_public_context("bind-a"),
            "failed question".into(),
            &sink,
        )
        .await;
        assert_eq!(d, PublicRunDisposition::Failed);
        assert_eq!(
            *sink.attempts.lock().unwrap(),
            vec!["unacknowledged answer".to_string()],
            "one attempt, no implicit retry"
        );
        assert_eq!(*runner.owner_runs.lock().unwrap(), 0);
        assert_eq!(reg.active_count(), 0, "failed delivery releases admission");
        assert_eq!(reg.tokens_spent(), 10, "provider spend still counts");
        let key =
            PublicHistorySession::from_provenance(sample_public_context("bind-a").provenance());
        assert!(store.load(&key, chrono::Utc::now()).unwrap().is_empty());

        let next = SpyRunner::new(Ok("acknowledged answer"), None);
        let (d, _) = run_with(&reg, &next, Some(&store), "bind-a", "next").await;
        assert_eq!(d, PublicRunDisposition::Replied, "binding is free again");
        assert!(next.seen_history.lock().unwrap()[0].is_empty());
        assert_eq!(
            store.load(&key, chrono::Utc::now()).unwrap(),
            vec![PublicHistoryTurn {
                user: "next".into(),
                assistant: "acknowledged answer".into(),
            }],
            "only the acknowledged next turn is remembered"
        );
    }

    #[tokio::test]
    async fn refusal_send_errors_are_failed_without_history_or_owner_fallback() {
        // Both the admission refusal and admitted-but-unwired notice use the
        // same acknowledgment gate; neither can become completed history.
        for admission_refusal in [true, false] {
            let reg = PublicRunRegistry::new(4, 1_000);
            let store = history_store("k");
            let runner = SpyRunner::new(Ok("must-not-run"), None);
            let harness = if admission_refusal {
                reg.record_spend(1_000);
                Some(runner.clone() as Arc<dyn HarnessRunner>)
            } else {
                None
            };
            let sink = FailingSink::default();
            let d = run_public_sms(
                &reg,
                harness,
                Some(&store),
                sample_public_context("bind-a"),
                "refused question".into(),
                &sink,
            )
            .await;
            assert_eq!(d, PublicRunDisposition::Failed);
            let notice = if admission_refusal {
                PUBLIC_OVER_SPEND_NOTICE
            } else {
                PUBLIC_BUSY_NOTICE
            };
            assert_eq!(*sink.attempts.lock().unwrap(), vec![notice.to_string()]);
            assert_eq!(reg.active_count(), 0);
            assert!(runner.public_runs.lock().unwrap().is_empty());
            assert_eq!(*runner.owner_runs.lock().unwrap(), 0);
            let key =
                PublicHistorySession::from_provenance(sample_public_context("bind-a").provenance());
            assert!(store.load(&key, chrono::Utc::now()).unwrap().is_empty());

            let next = SpyRunner::new(Ok("fine"), None);
            let fresh_reg = PublicRunRegistry::new(4, 1_000);
            run_with(&fresh_reg, &next, Some(&store), "bind-a", "next").await;
            assert!(next.seen_history.lock().unwrap()[0].is_empty());
        }
    }

    #[tokio::test]
    async fn hung_refusal_notices_are_bounded_and_failed_without_history() {
        for admission_refusal in [true, false] {
            let reg = PublicRunRegistry::new(4, 1_000);
            let store = history_store("k");
            let runner = SpyRunner::new(Ok("must-not-run"), None);
            let harness = if admission_refusal {
                reg.record_spend(1_000);
                Some(runner.clone() as Arc<dyn HarnessRunner>)
            } else {
                None
            };
            let sink = BlockingSink {
                entered: Notify::new(),
                release: Notify::new(),
                sent: Mutex::new(Vec::new()),
            };
            let d = tokio::time::timeout(
                Duration::from_secs(5),
                run_public_sms_bounded(
                    &reg,
                    harness,
                    Some(&store),
                    sample_public_context("bind-a"),
                    "refused question".into(),
                    &sink,
                    Duration::from_secs(5),
                    Duration::from_millis(50),
                ),
            )
            .await
            .expect("refusal notice must honor the delivery timeout");
            assert_eq!(d, PublicRunDisposition::Failed);
            assert_eq!(reg.active_count(), 0, "no leaked admission");
            let notice = if admission_refusal {
                PUBLIC_OVER_SPEND_NOTICE
            } else {
                PUBLIC_BUSY_NOTICE
            };
            assert_eq!(*sink.sent.lock().unwrap(), vec![notice.to_string()]);
            assert!(runner.public_runs.lock().unwrap().is_empty());
            assert_eq!(*runner.owner_runs.lock().unwrap(), 0);
            let key =
                PublicHistorySession::from_provenance(sample_public_context("bind-a").provenance());
            assert!(store.load(&key, chrono::Utc::now()).unwrap().is_empty());
            let next = SpyRunner::new(Ok("fine"), None);
            let fresh_reg = PublicRunRegistry::new(4, 1_000);
            run_with(&fresh_reg, &next, Some(&store), "bind-a", "next").await;
            assert!(next.seen_history.lock().unwrap()[0].is_empty());
        }
    }

    #[tokio::test]
    async fn no_store_means_history_free_single_turn() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let r = SpyRunner::new(Ok("a"), None);
        run_with(&reg, &r, None, "bind-a", "one").await;
        run_with(&reg, &r, None, "bind-a", "two").await;
        assert!(r.seen_history.lock().unwrap().iter().all(Vec::is_empty));
    }

    #[tokio::test]
    async fn failed_and_cancelled_turns_are_not_remembered() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let store = Arc::new(history_store("k"));
        let bad = SpyRunner::new(Err(()), None);
        let (d, sent) = run_with(&reg, &bad, Some(&store), "bind-a", "fails").await;
        assert_eq!(d, PublicRunDisposition::Failed);
        assert!(sent.is_empty());

        let slow = SpyRunner::new(Ok("late"), Some(Arc::new(Notify::new())));
        let handle = {
            let (reg, slow, store) = (reg.clone(), slow.clone(), store.clone());
            tokio::spawn(async move {
                let sink = SpySink::default();
                run_public_sms(
                    &reg,
                    Some(slow as Arc<dyn HarnessRunner>),
                    Some(store.as_ref()),
                    sample_public_context("bind-a"),
                    "cancelled".into(),
                    &sink,
                )
                .await
            })
        };
        slow.entered.notified().await;
        assert!(reg.cancel_binding("bind-a"));
        assert_eq!(handle.await.unwrap(), PublicRunDisposition::Cancelled);

        let ok = SpyRunner::new(Ok("fine"), None);
        run_with(&reg, &ok, Some(&store), "bind-a", "next").await;
        assert!(ok.seen_history.lock().unwrap()[0].is_empty());
    }

    #[tokio::test]
    async fn unreadable_history_fails_closed_without_running_or_sending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        let open = |k: &str| {
            PublicHistoryStore::open(&path, SecretsCrypto::new(k), PublicHistoryConfig::default())
                .unwrap()
        };
        let reg = PublicRunRegistry::new(4, 1_000);
        let first = SpyRunner::new(Ok("reply-one"), None);
        run_with(&reg, &first, Some(&open("k1")), "bind-a", "q-one").await;

        let wrong = open("k2");
        let r = SpyRunner::new(Ok("must-not-run"), None);
        let (d, sent) = run_with(&reg, &r, Some(&wrong), "bind-a", "q-two").await;
        assert_eq!(d, PublicRunDisposition::Failed);
        assert!(sent.is_empty());
        assert!(
            r.public_runs.lock().unwrap().is_empty(),
            "runner not invoked"
        );
        assert_eq!(*r.owner_runs.lock().unwrap(), 0);
        assert_eq!(reg.active_count(), 0, "permit released");
    }

    /// Runner that ignores cancellation: fires the token itself (a `/stop`
    /// landing just as the run completes) and still returns a reply, or
    /// never returns at all (hung provider).
    struct RaceRunner {
        cancel_then_reply: bool,
        saw_cancel: Mutex<Option<CancellationToken>>,
    }

    #[async_trait]
    impl HarnessRunner for RaceRunner {
        async fn run(
            &self,
            _session_key: String,
            _spec: Arc<FlowSpec>,
            _input: FlowInput,
            _sandbox: Arc<dyn crate::sandbox::Sandbox>,
            _events: broadcast::Sender<FlowStreamEvent>,
            _cancel: CancellationToken,
            _tool_service_override: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
            _trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
            _interaction_manifest: Option<crate::thinker::InteractionManifest>,
            _workspace_override: Option<std::path::PathBuf>,
            _max_iterations_override: Option<u32>,
            _transient_context: Option<String>,
            _think_level: Option<crate::agents::thinking::ThinkLevel>,
            _envelope: crate::thinker::TurnEnvelope,
            _turn_model: Option<crate::providers::session_model_handle::SessionModelPref>,
            _run_id: String,
            _telecom_public: Option<TelecomPublicContext>,
        ) -> Result<FlowOutcome, FlowError> {
            Ok(FlowOutcome::default())
        }

        async fn run_public_text(
            &self,
            _request: PublicRunRequest,
            cancel: CancellationToken,
            _events: broadcast::Sender<FlowStreamEvent>,
        ) -> Result<FlowOutcome, FlowError> {
            *self.saw_cancel.lock().unwrap() = Some(cancel.clone());
            if self.cancel_then_reply {
                cancel.cancel();
                return Ok(FlowOutcome {
                    final_text: "stale reply".into(),
                    total_tokens: 3,
                    ..FlowOutcome::default()
                });
            }
            std::future::pending().await
        }
    }

    fn race_runner(cancel_then_reply: bool) -> Arc<RaceRunner> {
        Arc::new(RaceRunner {
            cancel_then_reply,
            saw_cancel: Mutex::new(None),
        })
    }

    #[tokio::test]
    async fn cancel_landing_before_send_suppresses_reply_and_history() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let store = history_store("k");
        let runner = race_runner(true);
        let sink = SpySink::default();
        let d = run_public_sms(
            &reg,
            Some(runner as Arc<dyn HarnessRunner>),
            Some(&store),
            sample_public_context("bind-a"),
            "q".into(),
            &sink,
        )
        .await;
        assert_eq!(d, PublicRunDisposition::Cancelled);
        assert!(sink.sent.lock().unwrap().is_empty());
        assert_eq!(reg.active_count(), 0);
        let next = SpyRunner::new(Ok("fine"), None);
        run_with(&reg, &next, Some(&store), "bind-a", "next").await;
        assert!(next.seen_history.lock().unwrap()[0].is_empty());
    }

    /// Sink that submits first, then parks until transport acknowledgment.
    /// Cancellation after `entered` cannot undo submission, but only release
    /// produces the explicit acknowledgment needed for success/history.
    struct BlockingSink {
        entered: Notify,
        release: Notify,
        sent: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PublicReplySink for BlockingSink {
        async fn send_reply(
            &self,
            text: String,
        ) -> Result<(), crate::gateway::channel::ChannelError> {
            self.sent.lock().unwrap().push(text);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn binding_and_history_wait_for_ack_even_after_stop_after_submission() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let store = Arc::new(history_store("k"));
        let runner = SpyRunner::new(Ok("answer"), None);
        let sink = Arc::new(BlockingSink {
            entered: Notify::new(),
            release: Notify::new(),
            sent: Mutex::new(Vec::new()),
        });
        let first = {
            let (reg, runner, sink, store) =
                (reg.clone(), runner.clone(), sink.clone(), store.clone());
            tokio::spawn(async move {
                run_public_sms(
                    &reg,
                    Some(runner as Arc<dyn HarnessRunner>),
                    Some(store.as_ref()),
                    sample_public_context("bind-a"),
                    "q".into(),
                    sink.as_ref(),
                )
                .await
            })
        };
        sink.entered.notified().await;
        assert_eq!(reg.active_count(), 1, "permit held while reply in flight");
        assert!(!first.is_finished(), "submission is not acknowledgment");
        let key =
            PublicHistorySession::from_provenance(sample_public_context("bind-a").provenance());
        assert!(store.load(&key, chrono::Utc::now()).unwrap().is_empty());
        assert!(reg.cancel_binding("bind-a"), "/stop still finds the run");
        assert!(store.load(&key, chrono::Utc::now()).unwrap().is_empty());
        sink.release.notify_one();
        assert_eq!(first.await.unwrap(), PublicRunDisposition::Replied);
        assert_eq!(reg.active_count(), 0);
        let expected = vec![PublicHistoryTurn {
            user: "q".into(),
            assistant: "answer".into(),
        }];
        assert_eq!(store.load(&key, chrono::Utc::now()).unwrap(), expected);
        assert_eq!(*sink.sent.lock().unwrap(), vec!["answer".to_string()]);
        let next = SpyRunner::new(Ok("fine"), None);
        run_with(&reg, &next, Some(&store), "bind-a", "next").await;
        assert_eq!(next.seen_history.lock().unwrap()[0], expected);
    }

    #[tokio::test]
    async fn hung_delivery_is_failed_without_history_and_releases_admission() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let store = history_store("k");
        let runner = SpyRunner::new(Ok("answer"), None);
        let sink = BlockingSink {
            entered: Notify::new(),
            release: Notify::new(),
            sent: Mutex::new(Vec::new()),
        };
        let d = tokio::time::timeout(
            Duration::from_secs(5),
            run_public_sms_bounded(
                &reg,
                Some(runner as Arc<dyn HarnessRunner>),
                Some(&store),
                sample_public_context("bind-a"),
                "q".into(),
                &sink,
                Duration::from_secs(5),
                Duration::from_millis(50),
            ),
        )
        .await
        .expect("normal reply must honor the delivery timeout");
        assert_eq!(d, PublicRunDisposition::Failed);
        assert_eq!(reg.active_count(), 0);
        assert_eq!(*sink.sent.lock().unwrap(), vec!["answer".to_string()]);
        let key =
            PublicHistorySession::from_provenance(sample_public_context("bind-a").provenance());
        assert!(store.load(&key, chrono::Utc::now()).unwrap().is_empty());
        let next = SpyRunner::new(Ok("fine"), None);
        let (d, _) = run_with(&reg, &next, Some(&store), "bind-a", "next").await;
        assert_eq!(d, PublicRunDisposition::Replied, "binding is free again");
        assert!(next.seen_history.lock().unwrap()[0].is_empty());
    }

    #[tokio::test]
    async fn hung_provider_times_out_without_reply_history_or_leaked_admission() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let store = history_store("k");
        let runner = race_runner(false);
        let sink = SpySink::default();
        let d = run_public_sms_bounded(
            &reg,
            Some(runner.clone() as Arc<dyn HarnessRunner>),
            Some(&store),
            sample_public_context("bind-a"),
            "q".into(),
            &sink,
            Duration::from_millis(50),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(d, PublicRunDisposition::Failed);
        assert!(sink.sent.lock().unwrap().is_empty());
        assert_eq!(reg.active_count(), 0, "admission released");
        assert_eq!(reg.tokens_spent(), 0);
        assert!(
            runner
                .saw_cancel
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .is_cancelled(),
            "timed-out run is cancelled"
        );
        let next = SpyRunner::new(Ok("fine"), None);
        let (d2, _) = run_with(&reg, &next, Some(&store), "bind-a", "next").await;
        assert_eq!(d2, PublicRunDisposition::Replied, "binding is free again");
        assert!(next.seen_history.lock().unwrap()[0].is_empty());
    }
}
