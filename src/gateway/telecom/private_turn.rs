//! Private turn of a public SMS run: the A/B dual-approval path that lets a
//! public caller receive an answer drawn from the owner's memory.
//!
//! Entered only when the isolated public run returned a model-chosen lookup
//! request (`request_private_lookup`) **and** the owner enabled
//! `[telecom_private_read]` (a driver is wired and the trusted operator
//! control plane is installed). The sequence, all under the binding's
//! admission permit and [`ScopeGuard`]:
//!
//! 1. Approval A: request a read of the one opened source (owner memory,
//!    read-only search) for the exact query; tell the caller once, with a
//!    fixed notice, that nothing private has been sent.
//! 2. Wait for the operator (poll the ledger, `/stop` aborts).
//! 3. Consume A, then read through the [`PrivateReadDriver`].
//! 4. Draft the reply in an isolated, tool-free run.
//! 5. Approval B: the operator reviews the exact bounded draft.
//! 6. Re-check the scope, consume B and deliver the exact draft.
//!
//! Denial, expiry or failure sends one fixed notice that names no source or
//! stage; `/stop` sends nothing. The query, the lookup result and the draft
//! are never logged and never reach the public history: the caller of this
//! module records only fixed marker turns.

use std::time::Duration;

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use super::operator_control::ScopeGuard;
use super::private_read::{PrivateReadDriver, PrivateReadError, PrivateReadResult};
use super::public_admission::PublicRunRegistry;
use super::public_approval::{
    deliver_private_disclosure, ApprovalSource, PrivateDisclosureSink, PublicApprovalError,
    PublicApprovalStage, PublicApprovalStatus, MAX_APPROVAL_BODY_BYTES,
};
use super::public_history::PublicHistoryTurn;
use super::public_run::{ApprovedReplyOutcome, LeaseTurnCancellation, PublicReplySink};
use super::TelecomPublicContext;
use crate::gateway::interfaces::sms::PrivateSmsLease;
use crate::orchestrator::dispatch::{
    flow_event_channel, HarnessRunner, PrivateDraftRequest, PUBLIC_PROMPT_VERSION,
};

/// Sent once after approval A was requested.
pub const PRIVATE_CHECKING_NOTICE: &str = "需要在可信控制面确认，当前尚未发送私人信息。/ \
Checking with the owner; no private information has been sent.";

/// Sent on denial, expiry or any failure. Names no source and no stage.
pub const PRIVATE_INCOMPLETE_NOTICE: &str =
    "本次请求未完成，可继续普通聊天。/ This request could not be completed; you can continue chatting.";

/// Fixed public-history marker of a private turn (user side). The original
/// question is deliberately not kept.
pub const PRIVATE_TURN_USER_MARKER: &str = "[private turn omitted]";

/// Fixed public-history marker of a private turn (assistant side).
pub const PRIVATE_TURN_ASSISTANT_MARKER: &str = "[omitted]";

/// The only private source opened to public turns.
pub const OWNER_MEMORY_SOURCE_LINE: &str = "source: owner_memory (read-only search)";

/// Maximum caller text allowed into approval A; longer text is refused,
/// never silently truncated into a different approval.
pub const MAX_APPROVAL_CALLER_TEXT_BYTES: usize = 1024;

/// Production poll interval of the ledger while waiting for the operator.
pub const PRIVATE_APPROVAL_POLL: Duration = Duration::from_millis(250);

/// Clock seam: production uses [`Utc::now`]; tests move time to expire
/// approvals without waiting for the SMS TTL.
pub type ApprovalClock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Wiring of the private turn: present only when `[telecom_private_read]`
/// is enabled and a driver could be built.
#[derive(Clone)]
pub struct PrivateTurnWiring {
    driver: Arc<dyn PrivateReadDriver>,
    clock: ApprovalClock,
    poll: Duration,
}

impl PrivateTurnWiring {
    pub fn new(driver: Arc<dyn PrivateReadDriver>) -> Self {
        Self {
            driver,
            clock: Arc::new(Utc::now),
            poll: PRIVATE_APPROVAL_POLL,
        }
    }

    /// Test seam: inject the ledger clock and the poll interval.
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: ApprovalClock, poll: Duration) -> Self {
        self.clock = clock;
        self.poll = poll;
        self
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }
}

impl std::fmt::Debug for PrivateTurnWiring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateTurnWiring")
            .field("poll", &self.poll)
            .finish_non_exhaustive()
    }
}

/// End state of one private turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateTurnOutcome {
    /// Both approvals granted and the transport acknowledged the exact draft.
    Disclosed,
    /// Denied, expired or failed; the fixed incomplete notice was attempted.
    Incomplete,
    /// `/stop`: nothing further was sent.
    Cancelled,
    /// The transport did not acknowledge a send; the outcome is unknown and
    /// nothing more is attempted.
    DeliveryFailed,
}

/// Result reported back to the public run driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivateTurnReport {
    pub outcome: PrivateTurnOutcome,
    /// The caller acknowledged-received at least the checking notice, so the
    /// turn exists from the caller's point of view and the fixed marker may
    /// be recorded.
    pub caller_saw_turn: bool,
}

/// Inputs of one private turn. Borrowed from the public run driver, which
/// keeps holding the admission permit and the scope guard throughout.
pub(crate) struct PrivateTurnInput<'a> {
    pub wiring: &'a PrivateTurnWiring,
    pub guard: &'a ScopeGuard,
    pub registry: &'a Arc<PublicRunRegistry>,
    pub harness: &'a Arc<dyn HarnessRunner>,
    pub sink: &'a dyn PublicReplySink,
    pub cancel: &'a CancellationToken,
    pub lease: &'a PrivateSmsLease,
    pub ctx: TelecomPublicContext,
    pub text: String,
    pub history: Vec<PublicHistoryTurn>,
    pub query: String,
    pub run_timeout: Duration,
    pub delivery_timeout: Duration,
}

/// Deterministic, human-readable body of approval A: the exact source,
/// query and complete caller text. Debug quoting escapes newlines/control
/// characters so caller-controlled content cannot spoof another field.
pub fn private_read_approval_body(query: &str, caller_text: &str) -> String {
    format!("{OWNER_MEMORY_SOURCE_LINE}\nquery: {query:?}\ncaller message: {caller_text:?}")
}

fn bounded_private_read_approval_body(query: &str, caller_text: &str) -> Option<String> {
    (caller_text.len() <= MAX_APPROVAL_CALLER_TEXT_BYTES)
        .then(|| private_read_approval_body(query, caller_text))
}

/// Re-check the consumed A and live scope at a read/draft checkpoint; cap
/// provider time to the original A deadline rather than starting a fresh TTL.
fn read_remaining(input: &PrivateTurnInput<'_>, id: &str, body: &str) -> Option<Duration> {
    let control = input.guard.control();
    let scope = input.guard.scope();
    if input.cancel.is_cancelled()
        || !input.lease.matches_context(&input.ctx)
        || !control.is_current(scope)
    {
        return None;
    }
    let now = input.wiring.now();
    let deadline = control
        .ledger()
        .validate_consumed_read(id, scope, body, now)
        .ok()?;
    let remaining = deadline.signed_duration_since(now).to_std().ok()?;
    (!remaining.is_zero()).then_some(remaining.min(input.run_timeout))
}

enum Decision {
    Approved,
    Closed,
    Cancelled,
}

async fn await_decision(input: &PrivateTurnInput<'_>, id: &str) -> Decision {
    let ledger = input.guard.control().ledger();
    loop {
        if input.cancel.is_cancelled()
            || !input.lease.is_active()
            || !input.guard.control().is_current(input.guard.scope())
        {
            return Decision::Cancelled;
        }
        match ledger.status(id, input.wiring.now()) {
            PublicApprovalStatus::Approved => return Decision::Approved,
            PublicApprovalStatus::Closed => return Decision::Closed,
            PublicApprovalStatus::Pending => {}
        }
        tokio::select! {
            () = input.cancel.cancelled() => return Decision::Cancelled,
            () = input.lease.revoked() => return Decision::Cancelled,
            () = tokio::time::sleep(input.wiring.poll) => {}
        }
    }
}

/// Disclosure sink of approval B: the public reply path with its
/// acknowledgment and cancellation fence.
struct ReplyDisclosureSink<'a> {
    sink: &'a dyn PublicReplySink,
    cancel: &'a CancellationToken,
    lease: &'a PrivateSmsLease,
    delivery_timeout: Duration,
    guard: &'a ScopeGuard,
    wiring: &'a PrivateTurnWiring,
    read_id: &'a str,
    disclosure_id: &'a str,
    draft: &'a str,
    attempted: std::sync::atomic::AtomicBool,
    cancelled: std::sync::atomic::AtomicBool,
    invalid: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl PrivateDisclosureSink for ReplyDisclosureSink<'_> {
    async fn deliver(&self, body: &str) -> Result<(), PublicApprovalError> {
        use std::sync::atomic::Ordering;
        self.attempted.store(true, Ordering::SeqCst);
        let validate = |candidate: &str| {
            let control = self.guard.control();
            let scope = self.guard.scope();
            candidate == self.draft
                && self.lease.is_active()
                && control.is_current(scope)
                && control
                    .ledger()
                    .validate_consumed_disclosure(
                        self.disclosure_id,
                        self.read_id,
                        scope,
                        candidate,
                        self.wiring.now(),
                    )
                    .is_ok()
        };
        match tokio::time::timeout(
            self.delivery_timeout,
            self.sink
                .send_approved_reply(body.to_string(), self.lease, self.cancel, &validate),
        )
        .await
        {
            Ok(Ok(ApprovedReplyOutcome::Acknowledged)) => Ok(()),
            Ok(Ok(ApprovedReplyOutcome::Cancelled)) => {
                self.cancelled.store(true, Ordering::SeqCst);
                Err(PublicApprovalError::DeliveryFailed)
            }
            Ok(Ok(ApprovedReplyOutcome::Invalid)) => {
                self.invalid.store(true, Ordering::SeqCst);
                Err(PublicApprovalError::DeliveryFailed)
            }
            Ok(Err(_)) | Err(_) => {
                tracing::warn!(
                    "[public] private reply transport did not acknowledge; outcome unknown"
                );
                Err(PublicApprovalError::DeliveryFailed)
            }
        }
    }
}

/// Run one private turn. Every exit retires this binding's approvals.
pub(crate) async fn run_private_turn(input: PrivateTurnInput<'_>) -> PrivateTurnReport {
    let combined = LeaseTurnCancellation::new(input.cancel, input.lease);
    let input = PrivateTurnInput {
        cancel: &combined.token,
        ..input
    };
    let mut caller_saw_turn = false;
    let outcome = drive(&input, &mut caller_saw_turn).await;
    // No request of this turn may outlive it; a later turn of the binding
    // cannot exist yet because the permit is still held.
    input
        .guard
        .control()
        .ledger()
        .cancel_binding(input.guard.scope().binding_ref());
    let outcome = match outcome {
        Step::Done(o) => o,
        Step::Incomplete
            if !input.guard.control().is_current(input.guard.scope())
                || input.cancel.is_cancelled()
                || !input.lease.is_active() =>
        {
            PrivateTurnOutcome::Cancelled
        }
        Step::Incomplete => match send_notice_scoped(&input, PRIVATE_INCOMPLETE_NOTICE).await {
            Ok(ApprovedReplyOutcome::Acknowledged) => PrivateTurnOutcome::Incomplete,
            Ok(ApprovedReplyOutcome::Cancelled | ApprovedReplyOutcome::Invalid) => {
                PrivateTurnOutcome::Cancelled
            }
            Err(()) => PrivateTurnOutcome::DeliveryFailed,
        },
    };
    PrivateTurnReport {
        outcome,
        caller_saw_turn,
    }
}

enum Step {
    Done(PrivateTurnOutcome),
    /// Send the fixed incomplete notice.
    Incomplete,
}

/// Fixed notices belong to this scope too: recheck after the sink's last
/// asynchronous lock wait so revocation cannot submit even a stale notice.
async fn send_notice_scoped(
    input: &PrivateTurnInput<'_>,
    text: &'static str,
) -> Result<ApprovedReplyOutcome, ()> {
    let validate = |candidate: &str| {
        candidate == text
            && input.lease.matches_context(&input.ctx)
            && !input.cancel.is_cancelled()
            && input.guard.control().is_current(input.guard.scope())
    };
    match tokio::time::timeout(
        input.delivery_timeout,
        input
            .sink
            .send_approved_reply(text.to_string(), input.lease, input.cancel, &validate),
    )
    .await
    {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(_)) | Err(_) => {
            tracing::warn!("[public] private notice transport did not acknowledge");
            Err(())
        }
    }
}

async fn drive(input: &PrivateTurnInput<'_>, caller_saw_turn: &mut bool) -> Step {
    let control = input.guard.control();
    let ledger = control.ledger();
    let scope = input.guard.scope();
    let source = ApprovalSource::operator_control_plane();
    if input.cancel.is_cancelled()
        || !input.lease.matches_context(&input.ctx)
        || !control.is_current(scope)
    {
        return Step::Done(PrivateTurnOutcome::Cancelled);
    }

    // -- Approval A ------------------------------------------------------
    let Some(read_body) = bounded_private_read_approval_body(&input.query, &input.text) else {
        tracing::warn!("[public] private turn: caller text exceeds approval bound");
        return Step::Incomplete;
    };
    let read_id = match control.request_private_read_for(scope, &read_body, input.wiring.now()) {
        Ok(id) => id,
        Err(_) => {
            tracing::warn!("[public] private turn: read approval could not be opened");
            return Step::Incomplete;
        }
    };
    match send_notice_scoped(input, PRIVATE_CHECKING_NOTICE).await {
        Ok(ApprovedReplyOutcome::Acknowledged) => *caller_saw_turn = true,
        Ok(ApprovedReplyOutcome::Cancelled | ApprovedReplyOutcome::Invalid) => {
            return Step::Done(PrivateTurnOutcome::Cancelled);
        }
        Err(()) => return Step::Done(PrivateTurnOutcome::DeliveryFailed),
    }
    match await_decision(input, &read_id).await {
        Decision::Approved => {}
        Decision::Closed => return Step::Incomplete,
        Decision::Cancelled => return Step::Done(PrivateTurnOutcome::Cancelled),
    }
    if ledger
        .consume(
            &read_id,
            PublicApprovalStage::PrivateRead,
            source,
            scope,
            &read_body,
            input.wiring.now(),
        )
        .is_err()
    {
        tracing::warn!("[public] private turn: read approval no longer valid");
        return Step::Incomplete;
    }

    // -- Read (only after A was consumed) ---------------------------------
    let Some(read_timeout) = read_remaining(input, &read_id, &read_body) else {
        return Step::Incomplete;
    };
    let read = tokio::select! {
        biased;
        () = input.cancel.cancelled() => return Step::Done(PrivateTurnOutcome::Cancelled),
        () = input.lease.revoked() => return Step::Done(PrivateTurnOutcome::Cancelled),
        result = tokio::time::timeout(
            read_timeout,
            input.wiring.driver.read(scope, &input.query, input.cancel),
        ) => result,
    };
    let private_result = match read {
        _ if input.cancel.is_cancelled() || !input.lease.is_active() => {
            return Step::Done(PrivateTurnOutcome::Cancelled)
        }
        Ok(Ok(PrivateReadResult::Transient(body))) => body,
        Ok(Err(PrivateReadError::Cancelled)) => return Step::Done(PrivateTurnOutcome::Cancelled),
        Ok(Ok(PrivateReadResult::Unavailable)) | Ok(Err(_)) | Err(_) => {
            tracing::warn!("[public] private turn: read produced no usable result");
            return Step::Incomplete;
        }
    };

    // -- Draft (isolated, tool-free) ---------------------------------------
    let Some(draft_timeout) = read_remaining(input, &read_id, &read_body) else {
        return Step::Incomplete;
    };
    let (events, _rx) = flow_event_channel();
    let request = PrivateDraftRequest {
        ctx: input.ctx.clone(),
        text: input.text.clone(),
        history: input.history.clone(),
        run_id: uuid::Uuid::new_v4().to_string(),
        prompt_version: PUBLIC_PROMPT_VERSION,
        query: input.query.clone(),
        private_result,
    };
    let drafted = tokio::select! {
        biased;
        () = input.cancel.cancelled() => {
            input.registry.exhaust_unaccounted_spend();
            return Step::Done(PrivateTurnOutcome::Cancelled);
        }
        () = input.lease.revoked() => {
            input.registry.exhaust_unaccounted_spend();
            return Step::Done(PrivateTurnOutcome::Cancelled);
        }
        result = tokio::time::timeout(
            draft_timeout,
            input.harness.run_private_draft(request, input.cancel.clone(), events),
        ) => result,
    };
    match &drafted {
        Ok(Ok(outcome)) => input.registry.record_spend(u64::from(outcome.total_tokens)),
        _ => input.registry.exhaust_unaccounted_spend(),
    }
    let draft = match drafted {
        _ if input.cancel.is_cancelled() || !input.lease.is_active() => {
            return Step::Done(PrivateTurnOutcome::Cancelled)
        }
        Ok(Ok(outcome)) => outcome.final_text.trim().to_string(),
        Ok(Err(_)) | Err(_) => {
            tracing::warn!("[public] private turn: draft failed");
            return Step::Incomplete;
        }
    };
    if draft.is_empty() || draft.len() > MAX_APPROVAL_BODY_BYTES {
        tracing::warn!("[public] private turn: draft empty or over the approval bound");
        return Step::Incomplete;
    }

    // -- Approval B --------------------------------------------------------
    if read_remaining(input, &read_id, &read_body).is_none() {
        return Step::Incomplete;
    }
    let disclosure_id =
        match ledger.request_disclosure(&read_id, scope.clone(), &draft, input.wiring.now()) {
            Ok(id) => id,
            Err(_) => {
                tracing::warn!("[public] private turn: disclosure approval could not be opened");
                return Step::Incomplete;
            }
        };
    match await_decision(input, &disclosure_id).await {
        Decision::Approved => {}
        Decision::Closed => return Step::Incomplete,
        Decision::Cancelled => return Step::Done(PrivateTurnOutcome::Cancelled),
    }

    // -- Re-check, consume B, deliver the exact draft -----------------------
    if input.cancel.is_cancelled() || !input.lease.is_active() {
        return Step::Done(PrivateTurnOutcome::Cancelled);
    }
    if !control.is_current(scope) {
        tracing::warn!("[public] private turn: scope no longer current before delivery");
        return Step::Incomplete;
    }
    let disclosure_sink = ReplyDisclosureSink {
        sink: input.sink,
        cancel: input.cancel,
        lease: input.lease,
        delivery_timeout: input.delivery_timeout,
        guard: input.guard,
        wiring: input.wiring,
        read_id: &read_id,
        disclosure_id: &disclosure_id,
        draft: &draft,
        attempted: Default::default(),
        cancelled: Default::default(),
        invalid: Default::default(),
    };
    let delivered = deliver_private_disclosure(
        ledger,
        &disclosure_id,
        source,
        scope,
        &draft,
        input.wiring.now(),
        &disclosure_sink,
    )
    .await;
    use std::sync::atomic::Ordering;
    match delivered {
        Ok(_) => Step::Done(PrivateTurnOutcome::Disclosed),
        Err(_) if disclosure_sink.cancelled.load(Ordering::SeqCst) => {
            Step::Done(PrivateTurnOutcome::Cancelled)
        }
        Err(_) if disclosure_sink.invalid.load(Ordering::SeqCst) => Step::Incomplete,
        Err(_) if disclosure_sink.attempted.load(Ordering::SeqCst) => {
            Step::Done(PrivateTurnOutcome::DeliveryFailed)
        }
        Err(_) => {
            tracing::warn!("[public] private turn: disclosure approval no longer valid");
            Step::Incomplete
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{Channel, ChannelError, CoreSmsChannelExt, OutboundMessage};
    use crate::gateway::interfaces::sms::transport::test_private_context;
    use crate::gateway::interfaces::sms::{
        test_private_channel, InMemorySmsPeer, SmsChannel, SmsRelayTransport,
    };
    use crate::gateway::telecom::operator_control::OperatorControl;
    use crate::gateway::telecom::public_admission::{PublicAdmission, PublicRunPermit};
    use crate::gateway::telecom::public_approval::{
        PublicApprovalLedger, PublicApprovalScope, PublicApprovalView,
    };
    use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent};
    use crate::orchestrator::errors::FlowError;
    use crate::orchestrator::flow_spec::{FlowInput, FlowSpec};
    use crate::sync_primitives::Mutex;
    use tokio::sync::{broadcast, Notify};

    const TEST_BOUND: Duration = Duration::from_secs(3);
    const QUERY: &str = "owner memory: approved meeting note";
    const CALLER_TEXT: &str = "What did I agree to? This text is not the lookup query.";
    const PRIVATE_RESULT: &str = "PRIVATE-READ-CANARY-7731";
    const DRAFT: &str = "Approved private answer: PRIVATE-DRAFT-CANARY-7731";

    async fn bounded<F: std::future::Future>(future: F) -> F::Output {
        tokio::time::timeout(TEST_BOUND, future)
            .await
            .expect("private-turn test exceeded its bounded wait")
    }

    struct ReadSpy {
        seen: Mutex<Vec<(PublicApprovalScope, String)>>,
        entered: Notify,
        release: Option<Notify>,
    }

    #[async_trait]
    impl PrivateReadDriver for ReadSpy {
        async fn read(
            &self,
            scope: &PublicApprovalScope,
            query: &str,
            cancel: &CancellationToken,
        ) -> Result<PrivateReadResult, PrivateReadError> {
            self.seen
                .lock()
                .unwrap()
                .push((scope.clone(), query.into()));
            self.entered.notify_one();
            if let Some(release) = &self.release {
                tokio::select! {
                    () = release.notified() => {}
                    () = cancel.cancelled() => return Err(PrivateReadError::Cancelled),
                }
            }
            Ok(PrivateReadResult::Transient(Arc::from(PRIVATE_RESULT)))
        }
    }

    struct DraftSpy {
        seen: Mutex<Vec<PrivateDraftRequest>>,
        entered: Notify,
        release: Option<Notify>,
    }

    #[async_trait]
    impl HarnessRunner for DraftSpy {
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
            panic!("private turn must never enter the owner harness")
        }

        async fn run_private_draft(
            &self,
            request: PrivateDraftRequest,
            cancel: CancellationToken,
            _events: broadcast::Sender<FlowStreamEvent>,
        ) -> Result<FlowOutcome, FlowError> {
            self.seen.lock().unwrap().push(request);
            self.entered.notify_one();
            if let Some(release) = &self.release {
                tokio::select! {
                    () = release.notified() => {}
                    () = cancel.cancelled() => return Err(FlowError::Cancelled),
                }
            }
            Ok(FlowOutcome {
                final_text: format!("  {DRAFT}\n"),
                total_tokens: 7,
                ..FlowOutcome::default()
            })
        }
    }

    #[derive(Default)]
    struct ReplySpy {
        // Attempts cross the transport boundary; acknowledged is a separate fact.
        attempts: Mutex<Vec<String>>,
        channel: Option<SmsChannel>,
        acknowledged: Mutex<Vec<String>>,
        fail_on: Option<&'static str>,
        wait_before: Option<&'static str>,
        entered: Notify,
        release: Notify,
        wait_for_draft_ack: bool,
        submitted: Notify,
        ack_release: Notify,
    }

    #[async_trait]
    impl PublicReplySink for ReplySpy {
        async fn send_reply(&self, _text: String) -> Result<(), ChannelError> {
            panic!("private sends must not fall back to send_reply")
        }

        async fn send_approved_reply(
            &self,
            text: String,
            lease: &PrivateSmsLease,
            cancel: &CancellationToken,
            validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
        ) -> Result<ApprovedReplyOutcome, ChannelError> {
            if self.wait_before == Some(text.as_str()) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            // Validate after the final asynchronous pre-submit wait, not before it.
            if cancel.is_cancelled() {
                return Ok(ApprovedReplyOutcome::Cancelled);
            }
            if !lease.is_active() || !validate(&text) {
                return Ok(ApprovedReplyOutcome::Invalid);
            }
            self.attempts.lock().unwrap().push(text.clone());
            if self.fail_on == Some(text.as_str()) {
                return Err(ChannelError::SendFailed("unknown transport outcome".into()));
            }
            if let Some(channel) = &self.channel {
                let result = (channel as &dyn Channel)
                    .send_private_sms(
                        OutboundMessage::text("+15550000001", &text),
                        lease,
                        cancel,
                        validate,
                    )
                    .await?;
                if result != ApprovedReplyOutcome::Acknowledged {
                    return Ok(result);
                }
            }
            if self.wait_for_draft_ack && text == DRAFT {
                self.submitted.notify_one();
                self.ack_release.notified().await;
            }
            self.acknowledged.lock().unwrap().push(text);
            Ok(ApprovedReplyOutcome::Acknowledged)
        }
    }

    struct Fixture {
        ctx: TelecomPublicContext,
        control: Arc<OperatorControl>,
        guard: ScopeGuard,
        registry: Arc<PublicRunRegistry>,
        _permit: PublicRunPermit,
        cancel: CancellationToken,
        now: Arc<Mutex<DateTime<Utc>>>,
        wiring: PrivateTurnWiring,
        reader: Arc<ReadSpy>,
        drafter: Arc<DraftSpy>,
        sink: ReplySpy,
        transport: Arc<SmsRelayTransport>,
        lease: PrivateSmsLease,
        peer: Arc<InMemorySmsPeer>,
        text: String,
        query: String,
    }

    impl Fixture {
        async fn new(block_read: bool, block_draft: bool, mut sink: ReplySpy) -> Self {
            let ctx = test_private_context("private-turn-bind");
            let (channel, transport, lease, peer) = test_private_channel("private-turn-bind").await;
            sink.channel = Some(channel);
            let control = Arc::new(OperatorControl::new(Arc::new(PublicApprovalLedger::new())));
            let scope = PublicApprovalScope::new(&ctx, "core", "agent", "turn", 1);
            let guard = ScopeGuard::register(control.clone(), scope);
            let registry = PublicRunRegistry::new(1, 100);
            let PublicAdmission::Admitted(permit) = registry.try_admit(&ctx) else {
                panic!("fixture must be admitted")
            };
            let cancel = permit.cancel_token();
            let now = Arc::new(Mutex::new(
                DateTime::parse_from_rfc3339("2026-10-09T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ));
            let reader = Arc::new(ReadSpy {
                seen: Mutex::new(Vec::new()),
                entered: Notify::new(),
                release: block_read.then(Notify::new),
            });
            let drafter = Arc::new(DraftSpy {
                seen: Mutex::new(Vec::new()),
                entered: Notify::new(),
                release: block_draft.then(Notify::new),
            });
            let clock_now = now.clone();
            let clock: ApprovalClock = Arc::new(move || *clock_now.lock().unwrap());
            let wiring =
                PrivateTurnWiring::new(reader.clone()).with_clock(clock, Duration::from_millis(1));
            Self {
                ctx,
                control,
                guard,
                registry,
                _permit: permit,
                cancel,
                now,
                wiring,
                reader,
                drafter,
                sink,
                transport,
                lease,
                peer,
                text: CALLER_TEXT.into(),
                query: QUERY.into(),
            }
        }

        fn now(&self) -> DateTime<Utc> {
            *self.now.lock().unwrap()
        }

        async fn run(&self) -> PrivateTurnReport {
            let harness: Arc<dyn HarnessRunner> = self.drafter.clone();
            run_private_turn(PrivateTurnInput {
                wiring: &self.wiring,
                guard: &self.guard,
                registry: &self.registry,
                harness: &harness,
                sink: &self.sink,
                cancel: &self.cancel,
                lease: &self.lease,
                ctx: self.ctx.clone(),
                text: self.text.clone(),
                history: Vec::new(),
                query: self.query.clone(),
                run_timeout: Duration::from_secs(5),
                delivery_timeout: Duration::from_secs(5),
            })
            .await
        }

        async fn pending(&self, stage: PublicApprovalStage) -> PublicApprovalView {
            bounded(async {
                loop {
                    if let Some(view) = self
                        .control
                        .ledger()
                        .operator_views(self.now())
                        .into_iter()
                        .find(|view| view.stage == stage)
                    {
                        return view;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
        }

        fn decide(&self, view: &PublicApprovalView, approve: bool) {
            self.control
                .ledger()
                .decide_exact(
                    &view.id,
                    view.stage,
                    &view.body,
                    ApprovalSource::operator_control_plane(),
                    approve,
                    self.now(),
                )
                .unwrap();
        }

        fn assert_no_read_or_draft(&self) {
            assert!(self.reader.seen.lock().unwrap().is_empty());
            assert!(self.drafter.seen.lock().unwrap().is_empty());
        }

        fn assert_sends(&self, expected: &[&str]) {
            let expected: Vec<String> = expected.iter().map(|s| (*s).into()).collect();
            assert_eq!(*self.sink.attempts.lock().unwrap(), expected);
            assert_eq!(*self.sink.acknowledged.lock().unwrap(), expected);
            let submitted: Vec<_> = self
                .peer
                .submissions()
                .into_iter()
                .map(|op| op.command.body)
                .collect();
            assert_eq!(
                submitted, expected,
                "assert actual peer effects, not sink calls"
            );
        }

        fn assert_retired(&self, ids: &[&str]) {
            assert!(self.control.ledger().operator_views(self.now()).is_empty());
            for id in ids {
                assert_eq!(
                    self.control.ledger().status(id, self.now()),
                    PublicApprovalStatus::Closed
                );
            }
        }
    }

    #[tokio::test]
    async fn a_then_b_discloses_only_the_exact_draft_after_transport_ack() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let f = Fixture::new(
            false,
            false,
            ReplySpy {
                wait_for_draft_ack: true,
                ..ReplySpy::default()
            },
        )
        .await;
        let completed = AtomicBool::new(false);
        let (report, (a, b)) = bounded(async {
            tokio::join!(
                async {
                    let report = f.run().await;
                    completed.store(true, Ordering::SeqCst);
                    report
                },
                async {
                    let a = f.pending(PublicApprovalStage::PrivateRead).await;
                    assert_eq!(a.body, private_read_approval_body(QUERY, CALLER_TEXT));
                    f.assert_no_read_or_draft();
                    f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
                    f.decide(&a, true);
                    let b = f.pending(PublicApprovalStage::Disclosure).await;
                    assert_eq!(b.body, DRAFT, "B reviews the complete trimmed draft");
                    assert_eq!(
                        *f.reader.seen.lock().unwrap(),
                        vec![(f.guard.scope().clone(), QUERY.into())],
                        "read exactly the approved query once, never raw caller text"
                    );
                    {
                        let drafts = f.drafter.seen.lock().unwrap();
                        assert_eq!(drafts.len(), 1);
                        let request = &drafts[0];
                        assert_eq!(request.query, QUERY);
                        assert_eq!(request.text, CALLER_TEXT);
                        assert_eq!(&*request.private_result, PRIVATE_RESULT);
                        assert_eq!(request.ctx.provenance(), f.ctx.provenance());
                        assert!(request.history.is_empty());
                        assert_eq!(request.prompt_version, PUBLIC_PROMPT_VERSION);
                        assert!(!request.run_id.is_empty());
                    }
                    assert_eq!(f.registry.tokens_spent(), 7);
                    f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
                    f.decide(&b, true);
                    bounded(f.sink.submitted.notified()).await;
                    assert!(!completed.load(Ordering::SeqCst));
                    assert_eq!(
                        *f.sink.attempts.lock().unwrap(),
                        vec![PRIVATE_CHECKING_NOTICE.to_string(), DRAFT.to_string()]
                    );
                    assert_eq!(
                        *f.sink.acknowledged.lock().unwrap(),
                        vec![PRIVATE_CHECKING_NOTICE.to_string()],
                        "submission alone is not disclosure success"
                    );
                    f.sink.ack_release.notify_one();
                    (a.id, b.id)
                }
            )
        })
        .await;
        assert_eq!(
            report,
            PrivateTurnReport {
                outcome: PrivateTurnOutcome::Disclosed,
                caller_saw_turn: true,
            }
        );
        f.assert_sends(&[PRIVATE_CHECKING_NOTICE, DRAFT]);
        f.assert_retired(&[&a, &b]);
    }

    #[tokio::test]
    async fn a_denial_sends_only_fixed_notices_without_read_or_draft() {
        let f = Fixture::new(false, false, ReplySpy::default()).await;
        let (report, a) = bounded(async {
            tokio::join!(f.run(), async {
                let a = f.pending(PublicApprovalStage::PrivateRead).await;
                f.assert_no_read_or_draft();
                f.decide(&a, false);
                a.id
            })
        })
        .await;
        assert_eq!(report.outcome, PrivateTurnOutcome::Incomplete);
        assert!(report.caller_saw_turn);
        f.assert_no_read_or_draft();
        f.assert_sends(&[PRIVATE_CHECKING_NOTICE, PRIVATE_INCOMPLETE_NOTICE]);
        f.assert_retired(&[&a]);
    }

    #[tokio::test]
    async fn b_denial_never_sends_the_read_result_or_private_draft() {
        let f = Fixture::new(false, false, ReplySpy::default()).await;
        let (report, (a, b)) = bounded(async {
            tokio::join!(f.run(), async {
                let a = f.pending(PublicApprovalStage::PrivateRead).await;
                f.decide(&a, true);
                let b = f.pending(PublicApprovalStage::Disclosure).await;
                assert_eq!(b.body, DRAFT);
                f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
                f.decide(&b, false);
                (a.id, b.id)
            })
        })
        .await;
        assert_eq!(report.outcome, PrivateTurnOutcome::Incomplete);
        assert!(report.caller_saw_turn);
        assert_eq!(f.reader.seen.lock().unwrap().len(), 1);
        assert_eq!(f.drafter.seen.lock().unwrap().len(), 1);
        f.assert_sends(&[PRIVATE_CHECKING_NOTICE, PRIVATE_INCOMPLETE_NOTICE]);
        f.assert_retired(&[&a, &b]);
    }

    #[tokio::test]
    async fn scope_retirement_while_waiting_a_cancels_without_another_notice() {
        let f = Fixture::new(false, false, ReplySpy::default()).await;
        let (report, a) = bounded(async {
            tokio::join!(f.run(), async {
                let a = f.pending(PublicApprovalStage::PrivateRead).await;
                f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
                f.control.retire_scope(f.guard.scope().binding_ref());
                a.id
            })
        })
        .await;
        assert_eq!(report.outcome, PrivateTurnOutcome::Cancelled);
        assert!(report.caller_saw_turn);
        assert!(
            !f.cancel.is_cancelled(),
            "scope invalidation is independent of /stop"
        );
        f.assert_no_read_or_draft();
        f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
        f.assert_retired(&[&a]);
    }

    #[tokio::test]
    async fn caller_stop_while_waiting_a_or_b_suppresses_all_further_sends() {
        for stop_at in [
            PublicApprovalStage::PrivateRead,
            PublicApprovalStage::Disclosure,
        ] {
            let f = Fixture::new(false, false, ReplySpy::default()).await;
            let (report, ids) = bounded(async {
                tokio::join!(f.run(), async {
                    let a = f.pending(PublicApprovalStage::PrivateRead).await;
                    let mut ids = vec![a.id.clone()];
                    if stop_at == PublicApprovalStage::Disclosure {
                        f.decide(&a, true);
                        let b = f.pending(PublicApprovalStage::Disclosure).await;
                        ids.push(b.id);
                    } else {
                        f.assert_no_read_or_draft();
                    }
                    f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
                    assert!(
                        f.registry.cancel_caller(&f.ctx),
                        "use the caller /stop seam"
                    );
                    assert!(f.cancel.is_cancelled());
                    ids
                })
            })
            .await;
            assert_eq!(report.outcome, PrivateTurnOutcome::Cancelled, "{stop_at:?}");
            assert!(report.caller_saw_turn);
            f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
            if stop_at == PublicApprovalStage::PrivateRead {
                f.assert_no_read_or_draft();
            }
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            f.assert_retired(&ids);
        }
    }

    #[tokio::test]
    async fn a_expiring_during_read_or_draft_prevents_b_and_private_send() {
        for during_read in [true, false] {
            let f = Fixture::new(during_read, !during_read, ReplySpy::default()).await;
            let (report, a) = bounded(async {
                tokio::join!(f.run(), async {
                    let a = f.pending(PublicApprovalStage::PrivateRead).await;
                    f.decide(&a, true);
                    if during_read {
                        bounded(f.reader.entered.notified()).await;
                        assert!(f.drafter.seen.lock().unwrap().is_empty());
                    } else {
                        bounded(f.drafter.entered.notified()).await;
                    }
                    // Expiry at the deadline itself is closed, not one tick later.
                    *f.now.lock().unwrap() = a.expires_at;
                    if during_read {
                        f.reader.release.as_ref().unwrap().notify_one();
                    } else {
                        f.drafter.release.as_ref().unwrap().notify_one();
                    }
                    a.id
                })
            })
            .await;
            assert_eq!(report.outcome, PrivateTurnOutcome::Incomplete);
            assert!(report.caller_saw_turn);
            assert_eq!(f.reader.seen.lock().unwrap().len(), 1);
            assert_eq!(
                f.drafter.seen.lock().unwrap().len(),
                usize::from(!during_read)
            );
            f.assert_sends(&[PRIVATE_CHECKING_NOTICE, PRIVATE_INCOMPLETE_NOTICE]);
            f.assert_retired(&[&a]);
        }
    }

    #[tokio::test]
    async fn transport_failure_at_checking_or_disclosure_is_never_retried() {
        for fail_on in [PRIVATE_CHECKING_NOTICE, DRAFT] {
            let f = Fixture::new(
                false,
                false,
                ReplySpy {
                    fail_on: Some(fail_on),
                    ..ReplySpy::default()
                },
            )
            .await;
            let (report, ()) = bounded(async {
                tokio::join!(f.run(), async {
                    if fail_on == DRAFT {
                        let a = f.pending(PublicApprovalStage::PrivateRead).await;
                        f.decide(&a, true);
                        let b = f.pending(PublicApprovalStage::Disclosure).await;
                        f.decide(&b, true);
                    }
                })
            })
            .await;
            assert_eq!(report.outcome, PrivateTurnOutcome::DeliveryFailed);
            assert_eq!(report.caller_saw_turn, fail_on == DRAFT);
            let expected = if fail_on == DRAFT {
                vec![PRIVATE_CHECKING_NOTICE.to_string(), DRAFT.to_string()]
            } else {
                f.assert_no_read_or_draft();
                vec![PRIVATE_CHECKING_NOTICE.to_string()]
            };
            assert_eq!(*f.sink.attempts.lock().unwrap(), expected);
            assert_eq!(
                *f.sink.acknowledged.lock().unwrap(),
                if fail_on == DRAFT {
                    vec![PRIVATE_CHECKING_NOTICE.to_string()]
                } else {
                    Vec::<String>::new()
                }
            );
            f.assert_retired(&[]);
        }
    }

    #[tokio::test]
    async fn checking_notice_revalidates_retirement_or_stop_after_sink_wait() {
        for retire in [true, false] {
            let f = Fixture::new(
                false,
                false,
                ReplySpy {
                    wait_before: Some(PRIVATE_CHECKING_NOTICE),
                    ..ReplySpy::default()
                },
            )
            .await;
            let (report, ()) = bounded(async {
                tokio::join!(f.run(), async {
                    bounded(f.sink.entered.notified()).await;
                    f.pending(PublicApprovalStage::PrivateRead).await;
                    if retire {
                        f.control.retire_scope(f.guard.scope().binding_ref());
                    } else {
                        assert!(f.registry.cancel_caller(&f.ctx));
                    }
                    f.sink.release.notify_one();
                })
            })
            .await;
            assert_eq!(report.outcome, PrivateTurnOutcome::Cancelled);
            assert!(!report.caller_saw_turn);
            f.assert_no_read_or_draft();
            f.assert_sends(&[]);
            f.assert_retired(&[]);
        }
    }

    #[tokio::test]
    async fn disclosure_revalidates_scope_stop_and_a_expiry_after_sink_wait() {
        for invalidate in ["retire", "stop", "expire"] {
            let f = Fixture::new(
                false,
                false,
                ReplySpy {
                    wait_before: Some(DRAFT),
                    ..ReplySpy::default()
                },
            )
            .await;
            let (report, (a, b)) = bounded(async {
                tokio::join!(f.run(), async {
                    let a = f.pending(PublicApprovalStage::PrivateRead).await;
                    f.decide(&a, true);
                    let b = f.pending(PublicApprovalStage::Disclosure).await;
                    f.decide(&b, true);
                    bounded(f.sink.entered.notified()).await;
                    f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
                    match invalidate {
                        "retire" => f.control.retire_scope(f.guard.scope().binding_ref()),
                        "stop" => assert!(f.registry.cancel_caller(&f.ctx)),
                        "expire" => *f.now.lock().unwrap() = a.expires_at,
                        _ => unreachable!(),
                    }
                    f.sink.release.notify_one();
                    (a.id, b.id)
                })
            })
            .await;
            assert_eq!(
                report.outcome,
                if invalidate == "expire" {
                    PrivateTurnOutcome::Incomplete
                } else {
                    PrivateTurnOutcome::Cancelled
                }
            );
            assert!(report.caller_saw_turn);
            if invalidate == "expire" {
                f.assert_sends(&[PRIVATE_CHECKING_NOTICE, PRIVATE_INCOMPLETE_NOTICE]);
            } else {
                f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
            }
            f.assert_retired(&[&a, &b]);
        }
    }

    #[tokio::test]
    async fn bounded_caller_text_and_query_are_reviewed_and_read_without_truncation() {
        let mut f = Fixture::new(false, false, ReplySpy::default()).await;
        f.text = "é".repeat(MAX_APPROVAL_CALLER_TEXT_BYTES / 2);
        f.query = "q".repeat(512);
        let (report, ()) = bounded(async {
            tokio::join!(f.run(), async {
                let a = f.pending(PublicApprovalStage::PrivateRead).await;
                assert_eq!(a.body, private_read_approval_body(&f.query, &f.text));
                f.assert_no_read_or_draft();
                f.decide(&a, true);
                let b = f.pending(PublicApprovalStage::Disclosure).await;
                assert_eq!(f.reader.seen.lock().unwrap()[0].1, f.query);
                assert_eq!(f.drafter.seen.lock().unwrap()[0].text, f.text);
                f.decide(&b, true);
            })
        })
        .await;
        assert_eq!(report.outcome, PrivateTurnOutcome::Disclosed);
        f.assert_sends(&[PRIVATE_CHECKING_NOTICE, DRAFT]);
        f.assert_retired(&[]);
    }

    #[tokio::test]
    async fn oversized_caller_text_fails_before_opening_a_or_reading() {
        let mut f = Fixture::new(false, false, ReplySpy::default()).await;
        f.text = "x".repeat(MAX_APPROVAL_CALLER_TEXT_BYTES + 1);
        let report = bounded(f.run()).await;
        assert_eq!(report.outcome, PrivateTurnOutcome::Incomplete);
        assert!(!report.caller_saw_turn);
        f.assert_no_read_or_draft();
        f.assert_sends(&[PRIVATE_INCOMPLETE_NOTICE]);
        f.assert_retired(&[]);
    }

    #[tokio::test]
    async fn real_epoch_revocation_at_a_b_read_or_draft_discards_late_results_and_notices() {
        for reason in ["connect", "disconnect", "heartbeat"] {
            for stage in ["a", "b", "read", "draft"] {
                let f = Fixture::new(stage == "read", stage == "draft", ReplySpy::default()).await;
                let (report, ()) = bounded(async {
                    tokio::join!(f.run(), async {
                        let a = f.pending(PublicApprovalStage::PrivateRead).await;
                        if stage != "a" {
                            f.decide(&a, true);
                            match stage {
                                "b" => {
                                    f.pending(PublicApprovalStage::Disclosure).await;
                                }
                                "read" => {
                                    f.reader.entered.notified().await;
                                }
                                "draft" => {
                                    f.drafter.entered.notified().await;
                                }
                                _ => unreachable!(),
                            }
                        }
                        match reason {
                            "connect" => {
                                f.transport.connect().await.unwrap();
                            }
                            "disconnect" => f.transport.disconnect().await,
                            "heartbeat" => {
                                f.peer.fail_heartbeat();
                                assert!(f.transport.heartbeat(1).await.is_err());
                            }
                            _ => unreachable!(),
                        }
                        assert!(!f.lease.is_active());
                        // Uncooperative late releases must not create B/a notice.
                        if let Some(release) = &f.reader.release {
                            release.notify_one();
                        }
                        if let Some(release) = &f.drafter.release {
                            release.notify_one();
                        }
                    })
                })
                .await;
                assert_eq!(
                    report.outcome,
                    PrivateTurnOutcome::Cancelled,
                    "{reason}/{stage}"
                );
                assert!(
                    !f.cancel.is_cancelled(),
                    "revocation never cancels caller stop root"
                );
                f.assert_sends(&[PRIVATE_CHECKING_NOTICE]);
                f.assert_retired(&[]);
                assert_eq!(
                    f.reader.seen.lock().unwrap().len(),
                    usize::from(stage != "a")
                );
                assert_eq!(
                    f.drafter.seen.lock().unwrap().len(),
                    usize::from(stage == "b" || stage == "draft")
                );
            }
        }
    }

    #[tokio::test]
    async fn caller_stop_during_committed_b_wait_preserves_ack_and_does_not_revoke_lease() {
        let f = Fixture::new(
            false,
            false,
            ReplySpy {
                wait_for_draft_ack: true,
                ..ReplySpy::default()
            },
        )
        .await;
        let (report, ()) = bounded(async {
            tokio::join!(f.run(), async {
                let a = f.pending(PublicApprovalStage::PrivateRead).await;
                f.decide(&a, true);
                let b = f.pending(PublicApprovalStage::Disclosure).await;
                f.decide(&b, true);
                f.sink.submitted.notified().await;
                assert_eq!(f.peer.submissions().len(), 2, "already written before stop");
                assert!(f.registry.cancel_caller(&f.ctx));
                assert!(f.lease.is_active(), "stop must never cancel the lease root");
                assert_eq!(f.registry.active_count(), 1, "permit held through ACK");
                f.sink.ack_release.notify_one();
            })
        })
        .await;
        assert_eq!(report.outcome, PrivateTurnOutcome::Disclosed);
        f.assert_sends(&[PRIVATE_CHECKING_NOTICE, DRAFT]);
        assert_eq!(f.peer.generations(), vec![1, 1]);
        f.assert_retired(&[]);
    }

    #[test]
    fn approval_a_shows_exact_escaped_fields_and_rejects_oversize_caller_text() {
        let query = "notes\ncaller message: forged";
        let text = "hello\nsource: forged\rquery: forged";
        let body = bounded_private_read_approval_body(query, text).unwrap();
        assert_eq!(
            body,
            format!("{OWNER_MEMORY_SOURCE_LINE}\nquery: {query:?}\ncaller message: {text:?}")
        );
        assert_eq!(body.matches("\nquery:").count(), 1);
        assert_eq!(body.matches("\ncaller message:").count(), 1);
        let max = "é".repeat(MAX_APPROVAL_CALLER_TEXT_BYTES / 2);
        assert!(bounded_private_read_approval_body("q", &max).is_some());
        assert!(bounded_private_read_approval_body("q", &(max + "a")).is_none());
    }
}
