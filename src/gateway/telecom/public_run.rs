//! Public telecom SMS run driver (public session isolation contract §4–§6).
//!
//! Admission → (isolated history load) → isolated runner → acknowledged
//! buffered reply → (history append). Owns no business logic beyond that sequencing:
//! the reply text comes from the runner, refusals are fixed notices. The
//! optional [`PublicHistoryStore`] is the only cross-turn state; without one
//! every run is a single, history-free turn.

use std::time::Duration;

use crate::sync_primitives::Arc;

use crate::gateway::interfaces::sms::PrivateSmsLease;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Turn-local cancellation. Caller stop propagates only DOWN; the bridge never
/// receives a mutable lease root, and is aborted when its owner future drops.
pub(super) struct LeaseTurnCancellation {
    pub token: CancellationToken,
    bridge: tokio::task::JoinHandle<()>,
}

impl LeaseTurnCancellation {
    pub fn new(stop: &CancellationToken, lease: &PrivateSmsLease) -> Self {
        let token = stop.child_token();
        if !lease.is_active() {
            token.cancel();
        }
        let cancelled = token.clone();
        let lease = lease.clone();
        let bridge = tokio::spawn(async move {
            lease.revoked().await;
            cancelled.cancel();
        });
        Self { token, bridge }
    }
}

impl Drop for LeaseTurnCancellation {
    fn drop(&mut self) {
        self.bridge.abort();
    }
}

use super::operator_control::{OperatorControl, ScopeGuard};
use super::private_turn::{
    run_private_turn, PrivateTurnInput, PrivateTurnOutcome, PrivateTurnWiring,
    PRIVATE_TURN_ASSISTANT_MARKER, PRIVATE_TURN_USER_MARKER,
};
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

    /// No transport witness means no private lookup, even with enabled wiring.
    async fn private_sms_lease(&self, _ctx: &TelecomPublicContext) -> Option<PrivateSmsLease> {
        None
    }

    /// Private-only path. The implementation MUST invoke `validate` after its
    /// final async lock wait and before the first transport send poll. A public
    /// send cannot serve as a fallback here.
    async fn send_approved_reply(
        &self,
        _text: String,
        _lease: &PrivateSmsLease,
        _cancel: &CancellationToken,
        _validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
    ) -> Result<ApprovedReplyOutcome, crate::gateway::channel::ChannelError> {
        Ok(ApprovedReplyOutcome::Invalid)
    }

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

/// A private reply is invalidated before submit, cancelled before submit, or
/// acknowledged by the transport. Errors and timeouts remain unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovedReplyOutcome {
    Acknowledged,
    Cancelled,
    Invalid,
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
    /// The model requested a private lookup and the private turn ended as
    /// reported (see [`super::private_turn`]).
    Private(PrivateTurnOutcome),
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
    operator_control: Option<&Arc<OperatorControl>>,
) -> PublicRunDisposition {
    run_public_sms_with_private_read(
        registry,
        harness,
        history,
        ctx,
        text,
        sink,
        operator_control,
        None,
    )
    .await
}

/// [`run_public_sms`] with the optional private turn. `private` is wired
/// only when `[telecom_private_read]` is enabled; it takes effect only
/// together with `operator_control` (approvals need the trusted plane).
/// Then, and only then, the public run offers `request_private_lookup`, and
/// a recorded request diverts the turn into the A/B private flow instead of
/// sending the model's public reply.
#[allow(clippy::too_many_arguments)]
pub async fn run_public_sms_with_private_read(
    registry: &Arc<PublicRunRegistry>,
    harness: Option<Arc<dyn HarnessRunner>>,
    history: Option<&PublicHistoryStore>,
    ctx: TelecomPublicContext,
    text: String,
    sink: &dyn PublicReplySink,
    operator_control: Option<&Arc<OperatorControl>>,
    private: Option<&PrivateTurnWiring>,
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
        operator_control,
        private,
    )
    .await
}

/// Share the acknowledgment boundary across replies and refusal notices.
/// Do not log channel errors: transport diagnostics can contain bodies or phones.
pub(super) async fn send_reply_bounded(
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
    operator_control: Option<&Arc<OperatorControl>>,
    private: Option<&PrivateTurnWiring>,
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
    // Trusted A/B scope lifecycle: only an ADMITTED run with a harness opens
    // a PublicApprovalScope under this binding. Registering a binding cancels
    // any outstanding approvals of the prior scope, so a message refused
    // above (busy, over-spend, global cap, no harness) must never reach this
    // point: it would wipe the live turn's pending requests. The guard is
    // dropped on every exit path of this function, i.e. together with the
    // permit, after delivery and the history append.
    //
    // The random core/agent/turn ids below exist only to give this
    // turn a unique identity, so an operator-ledger consume is bound to
    // *this* turn and cannot be replayed against a later one. They are not
    // correlated with any owner core/agent/turn: the private turn consumes
    // the scope only through this guard.
    let scope_guard = operator_control.map(|c| {
        let scope = super::PublicApprovalScope::new(
            &ctx,
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            ctx.provenance().connection_generation(),
        );
        ScopeGuard::register(c.clone(), scope)
    });
    let caller_cancel = permit.cancel_token();
    // Admission precedes capability acquisition. Keep this SAME witness across
    // public lookup selection and the entire A/B flow; never reacquire a new gen.
    let lease = if private.is_some() && scope_guard.is_some() {
        tokio::select! {
            biased;
            _ = caller_cancel.cancelled() => return PublicRunDisposition::Cancelled,
            witness = sink.private_sms_lease(&ctx) => witness.filter(|lease| lease.matches_context(&ctx)),
        }
    } else {
        None
    };
    let combined = lease
        .as_ref()
        .map(|lease| LeaseTurnCancellation::new(&caller_cancel, lease));
    let cancel = combined
        .as_ref()
        .map(|combined| combined.token.clone())
        .unwrap_or(caller_cancel);
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
    // The private turn needs both the trusted approval plane (a live scope
    // guard) and an enabled read driver; otherwise the model is not even
    // offered the lookup tool.
    let private = private
        .zip(scope_guard.as_ref())
        .zip(lease.as_ref())
        .filter(|((_, guard), lease)| {
            guard.control().is_current(guard.scope()) && lease.matches_context(&ctx)
        });
    let private_inputs = private.map(|_| (ctx.clone(), text.clone(), prior.clone()));
    let request = PublicRunRequest {
        ctx,
        text,
        history: prior,
        run_id: uuid::Uuid::new_v4().to_string(),
        prompt_version: PUBLIC_PROMPT_VERSION,
        allow_private_lookup: private.is_some(),
    };
    let result = match tokio::time::timeout(run_timeout, async {
        if let Some(lease) = lease.as_ref() {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(crate::orchestrator::errors::FlowError::Cancelled),
                _ = lease.revoked() => Err(crate::orchestrator::errors::FlowError::Cancelled),
                result = harness.run_public_text(request, cancel.clone(), events) => result,
            }
        } else {
            harness
                .run_public_text(request, cancel.clone(), events)
                .await
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            // Hung provider: stop the run, release admission, say nothing.
            tracing::warn!("[public] run exceeded {run_timeout:?}; cancelled, no reply");
            registry.exhaust_unaccounted_spend();
            cancel.cancel();
            drop(permit);
            return PublicRunDisposition::Failed;
        }
    };
    match &result {
        Ok(outcome) if outcome.total_tokens == 0 && outcome.private_lookup.is_some() => {
            // A recorded lookup can survive a later harness failure, with no
            // trustworthy usage total. Never price that provider run at zero.
            registry.exhaust_unaccounted_spend();
        }
        Ok(outcome) => registry.record_spend(u64::from(outcome.total_tokens)),
        Err(crate::orchestrator::errors::FlowError::Cancelled) if cancel.is_cancelled() => {
            // Cancellation might have interrupted a paid provider call.
            registry.exhaust_unaccounted_spend();
        }
        Err(_) => registry.exhaust_unaccounted_spend(),
    }
    // The permit stays held from here through delivery and the history
    // append: `/stop` keeps finding this run until the sink acknowledges, and
    // a follow-up cannot load a stale history snapshot.
    let disposition = match result {
        // Early exit only; the sink owns the final pre-submit cancellation fence.
        _ if cancel.is_cancelled() || lease.as_ref().is_some_and(|lease| !lease.is_active()) => {
            PublicRunDisposition::Cancelled
        }
        // The model asked for a private lookup: its public reply is
        // discarded and the turn continues as a private turn.
        Ok(outcome) if outcome.private_lookup.is_some() => {
            let (Some(((wiring, guard), lease)), Some((ctx, text, prior)), Some(query)) =
                (private, private_inputs, outcome.private_lookup)
            else {
                // Unreachable by construction (the tool is offered only with
                // the wiring); fail closed without sending anything.
                drop(permit);
                return PublicRunDisposition::Failed;
            };
            let report = run_private_turn(PrivateTurnInput {
                wiring,
                guard,
                registry,
                harness: &harness,
                sink,
                cancel: &cancel,
                lease,
                ctx,
                text,
                history: prior,
                query,
                run_timeout,
                delivery_timeout,
            })
            .await;
            // Fixed marker only: neither the question, the query, the lookup
            // result nor the draft ever reaches public history.
            if report.caller_saw_turn
                && (lease.is_active() || report.outcome == PrivateTurnOutcome::Disclosed)
            {
                if let (Some(store), Some(key)) = (history, &history_key) {
                    let turn = PublicHistoryTurn {
                        user: PRIVATE_TURN_USER_MARKER.to_string(),
                        assistant: PRIVATE_TURN_ASSISTANT_MARKER.to_string(),
                    };
                    if let Err(e) = store.append(key, &turn, chrono::Utc::now(), false) {
                        tracing::warn!("[public] history append skipped: {e}");
                    }
                }
            }
            PublicRunDisposition::Private(report.outcome)
        }
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
            //
            // The `private_result` argument is the wall behind the public
            // history contract: the call site at this depth only ever passes
            // `false`: public history only ever stores the original caller
            // text and the public reply, so a private body cannot leak
            // through the normal-run append path.
            if let (Some(store), Some(key), Some(user)) = (history, &history_key, user_text) {
                let turn = PublicHistoryTurn {
                    user,
                    assistant: reply.trim().to_string(),
                };
                if let Err(e) = store.append(key, &turn, chrono::Utc::now(), false) {
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

        async fn send_approved_reply(
            &self,
            text: String,
            _lease: &PrivateSmsLease,
            cancel: &CancellationToken,
            validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
        ) -> Result<ApprovedReplyOutcome, crate::gateway::channel::ChannelError> {
            if cancel.is_cancelled() {
                return Ok(ApprovedReplyOutcome::Cancelled);
            }
            if !validate(&text) {
                return Ok(ApprovedReplyOutcome::Invalid);
            }
            self.send_reply(text).await?;
            Ok(ApprovedReplyOutcome::Acknowledged)
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

        async fn send_approved_reply(
            &self,
            text: String,
            _lease: &PrivateSmsLease,
            cancel: &CancellationToken,
            validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
        ) -> Result<ApprovedReplyOutcome, crate::gateway::channel::ChannelError> {
            if cancel.is_cancelled() {
                return Ok(ApprovedReplyOutcome::Cancelled);
            }
            if !validate(&text) {
                return Ok(ApprovedReplyOutcome::Invalid);
            }
            self.send_reply(text).await?;
            Ok(ApprovedReplyOutcome::Acknowledged)
        }
    }

    /// Runner spy: counts owner `run` calls (must stay 0) and serves the
    /// public path with a fixed reply, optionally blocking until released.
    struct SpyRunner {
        owner_runs: Mutex<u32>,
        public_runs: Mutex<Vec<String>>,
        seen_history: Mutex<Vec<Vec<PublicHistoryTurn>>>,
        lookup_allowed: Mutex<Vec<bool>>,
        lookup: Option<&'static str>,
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
                lookup_allowed: Mutex::new(Vec::new()),
                lookup: None,
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
            self.lookup_allowed
                .lock()
                .unwrap()
                .push(request.allow_private_lookup);
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
                    private_lookup: self.lookup.map(str::to_owned),
                    ..FlowOutcome::default()
                }),
                Err(()) => Err(FlowError::Internal("boom".into())),
            }
        }
    }

    #[tokio::test]
    async fn enabled_wiring_without_live_witness_keeps_public_chat_and_denies_lookup_a_read() {
        use crate::gateway::telecom::private_read::{
            PrivateReadDriver, PrivateReadError, PrivateReadResult,
        };
        use crate::gateway::telecom::public_approval::{PublicApprovalLedger, PublicApprovalScope};
        struct NoRead;
        #[async_trait]
        impl PrivateReadDriver for NoRead {
            async fn read(
                &self,
                _: &PublicApprovalScope,
                _: &str,
                _: &CancellationToken,
            ) -> Result<PrivateReadResult, PrivateReadError> {
                panic!("configuration/fake sink must not authorize an owner read")
            }
        }
        let reg = PublicRunRegistry::new(4, 1000);
        let ledger = Arc::new(PublicApprovalLedger::new());
        let control = Arc::new(OperatorControl::new(ledger.clone()));
        let runner = SpyRunner::new(Ok("ordinary answer"), None);
        let gate = Arc::new(Notify::new());
        // The fake sink exposes no authority, even though it can ordinary-send.
        let sink = SpySink::default();
        let private = PrivateTurnWiring::new(Arc::new(NoRead));
        let runner = Arc::try_unwrap(runner).ok().unwrap();
        let runner = Arc::new(SpyRunner {
            gate: Some(gate.clone()),
            ..runner
        });
        let (d, ()) = tokio::join!(
            run_public_sms_with_private_read(
                &reg,
                Some(runner.clone() as Arc<dyn HarnessRunner>),
                None,
                sample_public_context("bind-a"),
                "hello".into(),
                &sink,
                Some(&control),
                Some(&private)
            ),
            async {
                runner.entered.notified().await;
                assert_eq!(*runner.lookup_allowed.lock().unwrap(), vec![false]);
                assert!(
                    ledger.operator_views(chrono::Utc::now()).is_empty(),
                    "no A approval exists"
                );
                for scope in control.list_pending_for_test() {
                    assert_eq!(scope.binding_ref(), "bind-a");
                }
                gate.notify_one();
            }
        );
        assert_eq!(d, PublicRunDisposition::Replied);
        assert_eq!(*sink.sent.lock().unwrap(), vec!["ordinary answer"]);
        assert_eq!(*runner.owner_runs.lock().unwrap(), 0);
        assert_eq!(reg.active_count(), 0);
        assert!(control.list_pending_for_test().is_empty());
        let (_transport, lease) =
            crate::gateway::interfaces::sms::transport::test_private_lease("bind-a").await;
        assert!(sink
            .private_sms_lease(&sample_public_context("bind-a"))
            .await
            .is_none());
        // An ordinary-only sink must not synthesize approved-send fallback.
        struct OrdinaryOnly;
        #[async_trait]
        impl PublicReplySink for OrdinaryOnly {
            async fn send_reply(
                &self,
                _: String,
            ) -> Result<(), crate::gateway::channel::ChannelError> {
                panic!("private fallback must never call ordinary send")
            }
        }
        assert_eq!(
            OrdinaryOnly
                .send_approved_reply("private".into(), &lease, &CancellationToken::new(), &|_| {
                    true
                })
                .await
                .unwrap(),
            ApprovedReplyOutcome::Invalid
        );
    }

    #[tokio::test]
    async fn admitted_live_witness_enables_lookup_but_revocation_discards_runner_or_a_and_history()
    {
        use crate::gateway::channel::{Channel, CoreSmsChannelExt, OutboundMessage};
        use crate::gateway::interfaces::sms::{
            test_private_channel, transport::test_private_context, SmsChannel,
        };
        use crate::gateway::telecom::private_read::{
            PrivateReadDriver, PrivateReadError, PrivateReadResult,
        };
        use crate::gateway::telecom::public_approval::{PublicApprovalLedger, PublicApprovalScope};
        struct NoRead;
        #[async_trait]
        impl PrivateReadDriver for NoRead {
            async fn read(
                &self,
                _: &PublicApprovalScope,
                _: &str,
                _: &CancellationToken,
            ) -> Result<PrivateReadResult, PrivateReadError> {
                panic!("revoked A must not read memory")
            }
        }
        struct LiveSink(SmsChannel);
        #[async_trait]
        impl PublicReplySink for LiveSink {
            async fn send_reply(
                &self,
                _: String,
            ) -> Result<(), crate::gateway::channel::ChannelError> {
                panic!("private run must not ordinary-send")
            }
            async fn private_sms_lease(
                &self,
                ctx: &TelecomPublicContext,
            ) -> Option<PrivateSmsLease> {
                (&self.0 as &dyn Channel)
                    .private_sms_lease(ctx, "+15550000001")
                    .await
            }
            async fn send_approved_reply(
                &self,
                text: String,
                lease: &PrivateSmsLease,
                cancel: &CancellationToken,
                validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
            ) -> Result<ApprovedReplyOutcome, crate::gateway::channel::ChannelError> {
                (&self.0 as &dyn Channel)
                    .send_private_sms(
                        OutboundMessage::text("+15550000001", text),
                        lease,
                        cancel,
                        validate,
                    )
                    .await
            }
        }
        for stage in ["runner", "a"] {
            let reg = PublicRunRegistry::new(1, 1000);
            let store = history_store("key");
            let ctx = test_private_context("binding");
            let ledger = Arc::new(PublicApprovalLedger::new());
            let control = Arc::new(OperatorControl::new(ledger.clone()));
            let (channel, transport, _lease, peer) = test_private_channel("binding").await;
            let sink = LiveSink(channel);
            let private = PrivateTurnWiring::new(Arc::new(NoRead));
            let runner = Arc::try_unwrap(SpyRunner::new(Ok("discarded model output"), None))
                .ok()
                .unwrap();
            let runner = Arc::new(SpyRunner {
                lookup: Some("owner query"),
                gate: (stage == "runner").then(|| Arc::new(Notify::new())),
                ..runner
            });
            let (d, ()) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(
                    run_public_sms_with_private_read(
                        &reg,
                        Some(runner.clone() as Arc<dyn HarnessRunner>),
                        Some(&store),
                        ctx.clone(),
                        "private caller question".into(),
                        &sink,
                        Some(&control),
                        Some(&private)
                    ),
                    async {
                        runner.entered.notified().await;
                        assert_eq!(*runner.lookup_allowed.lock().unwrap(), vec![true]);
                        if stage == "a" {
                            loop {
                                if !ledger.operator_views(chrono::Utc::now()).is_empty() {
                                    break;
                                }
                                tokio::task::yield_now().await;
                            }
                            assert_eq!(
                                peer.submissions().len(),
                                1,
                                "checking notice already committed"
                            );
                        }
                        transport.connect().await.unwrap();
                    }
                )
            })
            .await
            .expect("revocation must terminate without releasing the provider");
            assert_eq!(
                d,
                if stage == "a" {
                    PublicRunDisposition::Private(PrivateTurnOutcome::Cancelled)
                } else {
                    PublicRunDisposition::Cancelled
                }
            );
            assert!(ledger.operator_views(chrono::Utc::now()).is_empty());
            assert!(control.list_pending_for_test().is_empty());
            assert_eq!(reg.active_count(), 0);
            assert_eq!(*runner.owner_runs.lock().unwrap(), 0);
            assert_eq!(peer.submissions().len(), usize::from(stage == "a"));
            assert!(peer.generations().iter().all(|generation| *generation == 1));
            let key = PublicHistorySession::from_provenance(ctx.provenance());
            assert!(
                store.load(&key, chrono::Utc::now()).unwrap().is_empty(),
                "no stale notice/history question or marker on revoked run"
            );
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
            None,
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
            None,
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
                    None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
                None,
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
                    None,
                    None,
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
        assert_eq!(
            reg.tokens_spent(),
            1_000,
            "unknown failed cost exhausts fallback"
        );

        // New process-budget seam: the failed call exhausted the old fallback.
        let reg = PublicRunRegistry::new(4, 1_000);
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
                    None,
                )
                .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), slow.entered.notified())
            .await
            .expect("admitted runner must enter");
        assert!(reg.cancel_binding("bind-a"));
        assert_eq!(handle.await.unwrap(), PublicRunDisposition::Cancelled);

        let reg = PublicRunRegistry::new(4, 1_000);
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
            None,
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

        async fn send_approved_reply(
            &self,
            text: String,
            _lease: &PrivateSmsLease,
            cancel: &CancellationToken,
            validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
        ) -> Result<ApprovedReplyOutcome, crate::gateway::channel::ChannelError> {
            if cancel.is_cancelled() {
                return Ok(ApprovedReplyOutcome::Cancelled);
            }
            if !validate(&text) {
                return Ok(ApprovedReplyOutcome::Invalid);
            }
            self.send_reply(text).await?;
            Ok(ApprovedReplyOutcome::Acknowledged)
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
                    None,
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
                None,
                None,
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
            None,
            None,
        )
        .await;
        assert_eq!(d, PublicRunDisposition::Failed);
        assert!(sink.sent.lock().unwrap().is_empty());
        assert_eq!(reg.active_count(), 0, "admission released");
        assert_eq!(
            reg.tokens_spent(),
            1_000,
            "timeout has unknown provider cost"
        );
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
        assert_eq!(d2, PublicRunDisposition::Refused(PUBLIC_OVER_SPEND_NOTICE));
        assert!(next.public_runs.lock().unwrap().is_empty());
        // Unknown cost refuses reuse of the old budget, not the history itself.
        let fresh = PublicRunRegistry::new(4, 1_000);
        let (d3, _) = run_with(&fresh, &next, Some(&store), "bind-a", "next").await;
        assert_eq!(d3, PublicRunDisposition::Replied);
        assert!(next.seen_history.lock().unwrap()[0].is_empty());
    }

    /// A busy refusal on the same binding must not touch the operator ledger:
    /// the live turn's scope and its pending private-read request survive, and
    /// only the admitted turn's own exit retires them.
    #[tokio::test]
    async fn busy_refusal_keeps_live_turn_scope_and_pending_request() {
        use crate::gateway::telecom::public_approval::PublicApprovalLedger;

        let reg = PublicRunRegistry::new(4, 1_000);
        let ledger = Arc::new(PublicApprovalLedger::new());
        let control = Arc::new(OperatorControl::new(ledger.clone()));
        let gate = Arc::new(Notify::new());
        let runner = SpyRunner::new(Ok("first answer"), Some(gate.clone()));
        let sink = Arc::new(SpySink::default());

        let first = {
            let reg = reg.clone();
            let runner = runner.clone();
            let sink = sink.clone();
            let control = control.clone();
            tokio::spawn(async move {
                run_public_sms(
                    &reg,
                    Some(runner as Arc<dyn HarnessRunner>),
                    None,
                    sample_public_context("bind-a"),
                    "first".into(),
                    sink.as_ref(),
                    Some(&control),
                )
                .await
            })
        };
        runner.entered.notified().await;

        let live: Vec<_> = control
            .list_pending_for_test()
            .into_iter()
            .filter(|s| s.binding_ref() == "bind-a")
            .collect();
        assert_eq!(live.len(), 1, "admitted turn registers exactly one scope");
        let live = live.into_iter().next().unwrap();
        let now = chrono::Utc::now();
        let id = ledger
            .request_private_read(live.clone(), "owner files", now)
            .expect("pending private-read request");

        let second = run_public_sms(
            &reg,
            Some(runner.clone() as Arc<dyn HarnessRunner>),
            None,
            sample_public_context("bind-a"),
            "second".into(),
            sink.as_ref(),
            Some(&control),
        )
        .await;
        assert_eq!(second, PublicRunDisposition::Refused(PUBLIC_BUSY_NOTICE));
        assert!(
            ledger.operator_view(&id, chrono::Utc::now()).is_some(),
            "a busy refusal must not cancel the live turn's pending request"
        );
        assert!(
            control.list_pending_for_test().iter().any(|s| s == &live),
            "a busy refusal must not replace the live turn's scope"
        );

        gate.notify_one();
        assert_eq!(first.await.unwrap(), PublicRunDisposition::Replied);
        assert!(
            control
                .list_pending_for_test()
                .iter()
                .all(|s| s.binding_ref() != "bind-a"),
            "the finished turn retires its scope"
        );
        assert!(
            ledger.operator_view(&id, chrono::Utc::now()).is_none(),
            "retiring the scope cancels its pending request"
        );
        assert_eq!(reg.active_count(), 0);
    }
}
