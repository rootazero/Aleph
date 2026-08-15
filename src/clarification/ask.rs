//! Park the current agent turn on a human.
//!
//! One function, [`ask`], shared by every tool that stops and waits: the
//! `ask_user` clarification tool and the `scratchpad` plan-approval gate. It
//! owns the four things all of them get wrong independently otherwise —
//! **who may be asked**, **how the question reaches them**, **what happens when
//! nothing can**, and **how long to wait**.
//!
//! ## Who may be asked
//!
//! Two conditions, deliberately both asked:
//!
//! * `is_channel_routable()` — there is a channel address to deliver to; and
//! * `!unattended` — a human is expected to be on the other end of it.
//!
//! Today those two agree across all six producers of an unattended run
//! (goal/loop continuations, heartbeat, A2A, cron and boot-resume without an
//! origin route). Asking only the first would make this tool's headless
//! behaviour a *consequence* of an invariant maintained six places away, and a
//! predicate that is true for reasons outside the file that reads it is one
//! refactor from being quietly false. The sibling `session_send` already
//! branches on `unattended` for exactly this reason. Costs nothing today;
//! costs a 600-second stall the day the invariant slips.
//!
//! ## How it reaches them
//!
//! Two transports, tried in order, both always producing plain text:
//!
//! 1. the originating **channel** (Telegram & co.) — a numbered text menu,
//!    plus an inline keyboard where the channel renders one; then
//! 2. the gateway **event bus** as a `stream.ask_user` frame — the Panel and
//!    TUI card. The Panel talks to core over the `gui:chat` pseudo-channel,
//!    which is never registered in the `ChannelRegistry`, so for Panel turns
//!    the channel transport always declines and this is the real path.
//!
//! Mirrors the approval path's channel → operator fallback
//! (`exec::approval::FallbackApprovalRequester`).
//!
//! ## Secrets never take transport 1
//!
//! A `secret` question asks for a credential. A channel reply is a durable
//! message in a third party's datastore — Telegram keeps it forever, and no
//! amount of masking on our side reaches it. So a secret question is delivered
//! **only** over the event bus, and when the turn's channel is a real
//! registered transport the credential is never handed to it. That is a
//! property of the shape, not of a caller remembering to check.
//!
//! The rule is per **question**, not per call. It began as a per-call refusal,
//! which meant one secret question among four threw away the three that had
//! nothing wrong with them: the human never saw them, the model got an error
//! instead of three answers, and the round trip bought nothing. A call is now
//! **partitioned** — the answerable questions go to the channel in order, the
//! secret ones are withheld and named in [`AskOutcome::withheld_secret`] so the
//! caller can tell the model exactly what it did not get and why. A call whose
//! questions are *all* secret still fails: there is nothing left to ask, and a
//! parked tool with no question is a 600-second stall.

use tokio::sync::oneshot;
use tracing::{info, warn};

use super::render::RenderedQuestion;
use super::session::ask_user_frame;
use super::{
    ClarificationManager, ClarificationRequest, ClarificationResult, DEFAULT_CLARIFY_TIMEOUT,
};
use crate::error::{AlephError, Result};
use crate::gateway::channel::{ChannelId, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::sync_primitives::Arc;
use crate::tools::turn_context::{current_turn_context, TurnContext};

/// The two handles a parking tool needs. Constructed once per tool and cloned
/// into [`ask`].
#[derive(Clone)]
pub struct ClarificationDeps {
    clarification: Arc<ClarificationManager>,
    channels: Arc<ChannelRegistry>,
}

impl ClarificationDeps {
    #[must_use]
    pub const fn new(
        clarification: Arc<ClarificationManager>,
        channels: Arc<ChannelRegistry>,
    ) -> Self {
        Self {
            clarification,
            channels,
        }
    }
}

/// Publish the question on the gateway event bus as a `stream.ask_user` frame.
///
/// Returns `false` when the question cannot be published — no bus wired (CLI
/// subcommands, unit tests) or no gateway run to correlate against — so the
/// caller can roll the pending clarification back instead of blocking on a
/// reply that can never arrive.
fn publish_to_event_bus(turn: &TurnContext, request: &ClarificationRequest) -> bool {
    if turn.run_id.is_empty() {
        return false;
    }
    let Some(bus) = crate::gateway::event_emitter::gateway_event_bus() else {
        return false;
    };
    let frame = ask_user_frame(
        &turn.run_id,
        &turn.session_key.to_string(),
        request,
        // Freshly registered: nothing answered yet.
        0,
    );
    if let Err(e) = bus.publish_frame(&frame) {
        warn!(error = %e, "ask: failed to publish the question on the event bus");
        return false;
    }
    true
}

/// Deny reason when the turn cannot reach a human, phrased for the model.
///
/// The model is the audience: this comes back as a tool error and its next
/// move should be to proceed on a stated assumption, not to retry. (A2 —
/// compress the error into context and let the model self-heal; the harness
/// never picks the recovery strategy.)
const HEADLESS_DENIAL: &str = "no human is reachable on this run (it is unattended — a scheduled \
     job, a goal/loop continuation, or a machine-to-machine delegation). Nobody can answer, so \
     do not retry: decide with the information you have and state the assumption you made, or \
     stop and report what you needed.";

/// What [`ask`] came back with.
///
/// Two facts, because a call can partly succeed: the answers the human gave,
/// and the questions they were never shown. A bare [`ClarificationResult`]
/// could only express the first, so the second used to be an error that
/// discarded the first along with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskOutcome {
    /// One answer per question that was actually asked, in order.
    pub result: ClarificationResult,
    /// Ids of the questions withheld because they ask for a credential and the
    /// turn's only transport is a third-party channel. Empty on every other
    /// path — the caller's branch on it is the branch on "did the human see
    /// everything I asked".
    pub withheld_secret: Vec<String>,
}

/// Retires the registry entry when a parked [`ask`] is **dropped** instead of
/// returning.
///
/// A cancelled run — and a run that hits the engine's deadline — is torn down
/// by dropping this whole future at the park below (the run-level
/// `tokio::select!` in `execution_engine::execute`), so not one of `ask`'s
/// return paths runs. Nothing else announces the end of the question at that
/// moment: the entry lives on as a zombie and every already-open client keeps
/// rendering the card, and its Enter hijack, until something unrelated happens
/// to sweep it. The terminal frame's producer belongs next to the fact it
/// reports, which is here.
struct RetireOnAbandon {
    manager: Arc<ClarificationManager>,
    session_key: String,
    /// Owned so [`Drop`] closes it **before** asking the manager to retire the
    /// entry: `cancel_abandoned` decides by reading this channel's closed flag,
    /// so retirement must not race the drop of what it reads.
    rx: Option<oneshot::Receiver<ClarificationResult>>,
    armed: bool,
}

impl RetireOnAbandon {
    /// Armed from the start: the entry exists from the moment this is built,
    /// and delivery is itself a slow await (a channel send crosses a network),
    /// so the window it guards opens immediately.
    fn new(
        manager: Arc<ClarificationManager>,
        session_key: String,
        rx: oneshot::Receiver<ClarificationResult>,
    ) -> Self {
        Self {
            manager,
            session_key,
            rx: Some(rx),
            armed: true,
        }
    }

    /// The receiver to park on — borrowed, never moved out, so [`Drop`] still
    /// owns it (see the field docs).
    fn receiver(&mut self) -> &mut oneshot::Receiver<ClarificationResult> {
        self.rx.as_mut().expect("invariant: taken only by Drop")
    }

    /// Every path that returns has already retired the entry — answered or
    /// superseded through the manager, timed out through `cleanup_expired`,
    /// undeliverable through `cancel`.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RetireOnAbandon {
    fn drop(&mut self) {
        drop(self.rx.take());
        if !self.armed {
            return;
        }
        // `Drop` cannot await. Off-loading to the runtime is the same
        // best-effort posture as publishing with no bus wired: outside a
        // runtime there is no client holding a card either.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let manager = Arc::clone(&self.manager);
        let session_key = std::mem::take(&mut self.session_key);
        handle.spawn(async move {
            manager.cancel_abandoned(&session_key).await;
        });
    }
}

/// Why a secret question cannot go to `channel`, phrased for the model.
fn secret_refusal(channel_id: &str) -> String {
    format!(
        "a reply on `{channel_id}` is a permanent message in that service's history, so a \
         credential must not be asked for there. Have the user set it through the Panel, or via \
         configuration, and continue without it."
    )
}

/// Register `request` for the current turn, deliver it, and park until the
/// human has answered every question (or the wait times out).
///
/// Secret questions are dropped from `request` when the turn's transport is a
/// registered third-party channel; the ids come back in
/// [`AskOutcome::withheld_secret`] and the rest of the call proceeds normally.
///
/// # Errors
///
/// Returns a tool error when no human can be reached — no turn context, an
/// unattended run, no transport that can carry the question, or *every*
/// question being a secret on a channel turn. Every one of them is immediate: a
/// tool that cannot be answered must fail fast, never occupy the 600-second
/// wait.
pub async fn ask(
    deps: &ClarificationDeps,
    mut request: ClarificationRequest,
) -> Result<AskOutcome> {
    let Some(turn) = current_turn_context() else {
        return Err(AlephError::tool(
            "this tool is only available inside an interactive channel turn",
        ));
    };
    if turn.unattended || !turn.is_channel_routable() {
        return Err(AlephError::tool(format!(
            "cannot ask the user: {HEADLESS_DENIAL}"
        )));
    }
    let session_key = turn.session_key.to_string();

    let channel = ChannelId::new(&turn.channel_id);
    // Resolved BEFORE registering so a refusal never leaves a pending entry
    // behind. `None` is the Panel's `gui:chat` pseudo-channel (and any turn
    // whose channel is stopped): no third-party transport exists, so a secret
    // has somewhere safe to go.
    let channel_is_third_party = deps.channels.get(&channel).await.is_some();
    let mut withheld_secret: Vec<String> = Vec::new();
    if channel_is_third_party && request.questions.iter().any(|q| q.secret) {
        // `take` rather than `iter().cloned()`: every question is moved into
        // exactly one of the two halves, and the only two ways out of this
        // block both replace `request` (or return), so the emptied original is
        // never observable.
        let (askable, secret): (Vec<_>, Vec<_>) = std::mem::take(&mut request.questions)
            .into_iter()
            .partition(|q| !q.secret);
        withheld_secret = secret.into_iter().map(|q| q.id).collect();
        // Nothing left to ask: parking on an empty request is a stall, so this
        // is the one case that still fails the whole call.
        let Ok(narrowed) = ClarificationRequest::new(askable) else {
            return Err(AlephError::tool(format!(
                "nothing in this call can be asked here: {}",
                secret_refusal(&turn.channel_id)
            )));
        };
        info!(
            channel = %turn.channel_id,
            withheld = withheld_secret.len(),
            "ask: withholding secret questions from a third-party channel"
        );
        request = narrowed;
    }

    // Register BEFORE delivery so a reply arriving immediately is not lost.
    let rx = deps
        .clarification
        .register(
            session_key.clone(),
            request.clone(),
            DEFAULT_CLARIFY_TIMEOUT,
            turn.run_id.clone(),
        )
        .await;
    // The receiver moves into the guard: a cancelled run drops this whole
    // future at the park below, and the guard is the only thing that runs then.
    let mut parked = RetireOnAbandon::new(Arc::clone(&deps.clarification), session_key.clone(), rx);

    // Recomputed AFTER the partition, so it can only be true on a turn with no
    // third-party transport (the Panel's `gui:chat`, or a stopped channel).
    // The bus is then not a fallback but the only route the credential may
    // take, and saying so here keeps that explicit rather than leaving it to a
    // channel send that would have failed anyway.
    let wants_secret = request.questions.iter().any(|q| q.secret);
    let delivered = if wants_secret {
        // Bus only — see the module doc.
        publish_to_event_bus(&turn, &request)
    } else {
        let RenderedQuestion { text, keyboard } =
            super::render::render(request.first(), 0, request.len());
        let mut message = OutboundMessage::text(turn.conversation_id.clone(), text);
        message.inline_keyboard = keyboard;
        match deps.channels.send(&channel, message).await {
            Ok(_) => true,
            Err(e) => {
                let published = publish_to_event_bus(&turn, &request);
                if !published {
                    warn!(error = %e, "ask: failed to deliver question to channel");
                }
                published
            }
        }
    };

    if !delivered {
        // Neither transport can reach the user — nobody can ever answer. Drop
        // the registration rather than park on a reply that cannot arrive.
        deps.clarification.cancel(&session_key).await;
        parked.disarm();
        return Err(AlephError::tool(
            "failed to deliver the question to the user — no channel accepted it and no Panel \
             session is attached. Continue without the answer and say what you assumed.",
        ));
    }

    info!(
        session = %session_key,
        questions = request.len(),
        "ask: question delivered — awaiting reply"
    );

    // Block until the user replies, the request is superseded, or the timeout
    // fires. The explicit timeout guarantees the tool never hangs even if no
    // cleanup pass reaps the registry entry. Cancellation of the run drops this
    // whole future (run-level `tokio::select!` in `execution_engine::execute`),
    // and `parked`'s guard is then the one that retires the entry and tells
    // every open client the question is over.
    let result = match tokio::time::timeout(DEFAULT_CLARIFY_TIMEOUT, parked.receiver()).await {
        Ok(Ok(result)) => result,
        // Sender dropped without sending — treat as cancelled.
        Ok(Err(_)) => ClarificationResult::cancelled(),
        // Timed out — reap the stale registry entry. `cleanup_expired` rather
        // than `cancel` so the terminal frame clients receive says `expired`,
        // matching the status returned here.
        //
        // SIDE EFFECT: this is a registry-wide sweep, not a per-session reap,
        // so other sessions' expired entries are reaped along with this one.
        // Each reap publishes its own `ClarificationEnded` frame, so the
        // affected clients see the right outcome for *their* request, not
        // this `ask`'s timeout. The current entry is past its deadline by
        // construction (same duration, registered first); any other reaped
        // entries are unrelated stale runs that would have reaped on the
        // next `register` anyway.
        Err(_) => {
            deps.clarification.cleanup_expired().await;
            ClarificationResult::timeout()
        }
    };
    parked.disarm();

    Ok(AskOutcome {
        result,
        withheld_secret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clarification::{ClarificationQuestion, ClarificationRequest};
    use crate::gateway::channel::{
        Channel, ChannelCapabilities, ChannelInfo, ChannelResult, ChannelState, ChannelStatus,
        MessageId, SendResult,
    };
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::TURN_CONTEXT;

    /// A registered transport, so `deps.channels.get(..)` answers `Some` — the
    /// only thing the secret refusal reads.
    struct RegisteredChannel {
        info: ChannelInfo,
        state: ChannelState,
    }

    impl RegisteredChannel {
        fn new(id: &str) -> Self {
            Self {
                info: ChannelInfo {
                    id: ChannelId::new(id),
                    name: id.to_string(),
                    channel_type: "test".to_string(),
                    status: ChannelStatus::Connected,
                    capabilities: ChannelCapabilities::default(),
                },
                state: ChannelState::new(8),
            }
        }
    }

    #[async_trait::async_trait]
    impl Channel for RegisteredChannel {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }
        fn state(&self) -> &ChannelState {
            &self.state
        }
        async fn start(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
            Ok(SendResult {
                message_id: MessageId::new("ok"),
                timestamp: chrono::Utc::now(),
            })
        }
    }

    fn deps() -> ClarificationDeps {
        ClarificationDeps::new(
            Arc::new(ClarificationManager::new()),
            Arc::new(ChannelRegistry::new()),
        )
    }

    fn routable_turn() -> TurnContext {
        TurnContext {
            session_key: SessionKey::ephemeral("ask-test"),
            run_id: String::new(),
            channel_id: "telegram".to_string(),
            conversation_id: "user-1".to_string(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        }
    }

    async fn ask_in(turn: TurnContext, request: ClarificationRequest) -> Result<AskOutcome> {
        let d = deps();
        TURN_CONTEXT
            .scope(turn, async move { ask(&d, request).await })
            .await
    }

    #[tokio::test]
    async fn errors_without_turn_context() {
        let err = ask(&deps(), ClarificationRequest::text("Which one?"))
            .await
            .expect_err("missing turn context must be rejected");
        assert!(err.to_string().contains("interactive channel turn"));
    }

    #[tokio::test]
    async fn errors_on_non_routable_turn() {
        let turn = TurnContext {
            session_key: SessionKey::task("main", "cron", "daily"),
            channel_id: String::new(),
            conversation_id: String::new(),
            ..routable_turn()
        };
        let err = ask_in(turn, ClarificationRequest::text("Which one?"))
            .await
            .expect_err("non-routable turn must be rejected");
        assert!(err.to_string().contains("unattended"), "{err}");
    }

    /// The second half of the headless guard, and the one that does not follow
    /// from the first: a run CAN carry a full channel route and still be
    /// unattended. Today no producer emits that combination — this test is what
    /// keeps the refusal from silently depending on that staying true.
    #[tokio::test]
    async fn errors_on_a_routable_but_unattended_turn() {
        let turn = TurnContext {
            unattended: true,
            plan_gate: None,
            ..routable_turn()
        };
        let err = ask_in(turn, ClarificationRequest::text("Which one?"))
            .await
            .expect_err("an unattended run must never park on a human");
        assert!(err.to_string().contains("unattended"), "{err}");
        // Actionable, not a retry instruction — the model's next move is to
        // proceed on a stated assumption.
        assert!(err.to_string().contains("do not retry"), "{err}");
    }

    #[tokio::test]
    async fn errors_when_no_transport_can_reach_the_user() {
        // Routable turn, but the registry has no such channel AND the turn has
        // no gateway run (`run_id` empty) to publish an `AskUser` frame
        // against — neither transport can reach the user, so the pending
        // clarification is rolled back instead of blocking on a reply that can
        // never arrive.
        let err = ask_in(routable_turn(), ClarificationRequest::text("Which one?"))
            .await
            .expect_err("delivery failure must surface as an error");
        assert!(err.to_string().contains("failed to deliver"), "{err}");
    }

    /// A registry holding one live `telegram` transport, so
    /// `deps.channels.get(..)` answers `Some` — the only thing the secret
    /// partition reads.
    async fn deps_with_registered_channel() -> ClarificationDeps {
        let registry = Arc::new(ChannelRegistry::new());
        registry
            .register(Box::new(RegisteredChannel::new("telegram")))
            .await;
        ClarificationDeps::new(Arc::new(ClarificationManager::new()), registry)
    }

    fn secret_question(id: &str) -> ClarificationQuestion {
        ClarificationQuestion::text(id, "Paste the API token").with_secret(true)
    }

    /// A call with nothing BUT secrets still fails outright: withholding every
    /// question would park the tool on a request no reply can complete.
    #[tokio::test]
    async fn an_all_secret_call_is_refused_when_the_turn_has_a_real_channel() {
        let d = deps_with_registered_channel().await;
        let request =
            ClarificationRequest::new(vec![secret_question("token")]).expect("one question");

        let err = TURN_CONTEXT
            .scope(routable_turn(), async move { ask(&d, request).await })
            .await
            .expect_err("a secret must not be routed through a third-party channel");
        let msg = err.to_string();
        assert!(msg.contains("permanent message"), "{msg}");
        assert!(msg.contains("continue without it"), "{msg}");
    }

    /// The partition. One secret question among several used to throw away the
    /// answerable ones with it — the human never saw three perfectly ordinary
    /// questions because a fourth asked for a token.
    ///
    /// The delivery error is what this fixture can observe (no bus is wired in
    /// a unit test, and the fake channel's `send` is the only transport), and
    /// it is enough for the claim being made: the call got PAST the secret gate
    /// on a turn whose channel is a registered third-party transport, which the
    /// all-secret test above proves it does not do when there is nothing else
    /// to ask. What reaches the human is covered end-to-end in the real-machine
    /// walk.
    #[tokio::test]
    async fn a_secret_among_others_withholds_only_itself() {
        let registry = Arc::new(ChannelRegistry::new());
        registry
            .register(Box::new(RegisteredChannel::new("telegram")))
            .await;
        let manager = Arc::new(ClarificationManager::new());
        let d = ClarificationDeps::new(Arc::clone(&manager), registry);
        let request = ClarificationRequest::new(vec![
            ClarificationQuestion::text("env", "Which environment?"),
            secret_question("token"),
            ClarificationQuestion::text("ticket", "Ticket id?"),
        ])
        .expect("three questions");

        // The fake channel accepts the send, so the call parks. Drive it from a
        // second task: answer the two askable questions and confirm the secret
        // one is neither asked nor waited for.
        //
        // ONE turn, and the key read off it — `SessionKey::ephemeral` mints a
        // fresh UUID per call, so building the fixture twice would leave this
        // test resolving a session the tool never registered.
        let turn = routable_turn();
        let key = turn.session_key.to_string();
        let asker = tokio::spawn(TURN_CONTEXT.scope(turn, async move { ask(&d, request).await }));

        // Wait for registration, then walk the questions one reply at a time —
        // exactly what the inbound router does for a channel turn.
        let registered = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !manager.has_pending(&key).await {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(registered.is_ok(), "the call must park on a live question");

        let pending = manager.list_pending().await;
        let entry = pending.first().expect("one pending clarification");
        let asked: Vec<&str> = entry.questions.iter().map(|q| q.id.as_str()).collect();
        assert_eq!(
            asked,
            vec!["env", "ticket"],
            "the secret question must not be registered for a channel turn"
        );
        assert!(
            entry.questions.iter().all(|q| !q.secret),
            "nothing marked secret may reach a third-party transport"
        );

        assert!(manager.resolve(&key, "staging").await.consumed());
        assert!(manager.resolve(&key, "ABC-1").await.consumed());

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), asker)
            .await
            .expect("the parked call must resolve")
            .expect("task must not panic")
            .expect("a partly-askable call must succeed");
        assert_eq!(outcome.withheld_secret, vec!["token".to_string()]);
        let answers: Vec<&str> = outcome
            .result
            .answers
            .iter()
            .map(|a| a.question_id.as_str())
            .collect();
        assert_eq!(answers, vec!["env", "ticket"]);
    }

    /// A cancelled run is torn down by dropping the parked call, so none of
    /// `ask`'s return paths runs and the guard is the only thing left that can
    /// retire the registry entry. Without it the entry survives as a zombie:
    /// every already-open client keeps rendering a question nobody can answer,
    /// and `has_pending` cannot tell the difference — hence `is_registered`.
    #[tokio::test]
    async fn dropping_a_parked_ask_retires_its_entry() {
        let d = deps_with_registered_channel().await;
        let manager = Arc::clone(&d.clarification);

        // ONE turn, and the key read off it — `SessionKey::ephemeral` mints a
        // fresh UUID per call.
        let turn = routable_turn();
        let key = turn.session_key.to_string();
        let request = ClarificationRequest::text("Which environment?");
        let asker = tokio::spawn(TURN_CONTEXT.scope(turn, async move { ask(&d, request).await }));

        let parked = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !manager.has_pending(&key).await {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(parked.is_ok(), "the call must park on a live question");

        // Exactly what the run-level `select!` does on cancel or deadline: drop
        // the future. Awaiting the aborted handle returns only once it is gone,
        // so the guard has run by the time the poll below starts.
        asker.abort();
        let _ = asker.await;

        let retired = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while manager.is_registered(&key).await {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            retired.is_ok(),
            "the abandoned entry must be retired, not left for the 600 s deadline"
        );
    }

    /// The Panel's `gui:chat` is never in the registry, so nothing is withheld
    /// there — that is the whole reason a secret has somewhere to go at all.
    #[tokio::test]
    async fn a_secret_is_not_withheld_when_there_is_no_third_party_transport() {
        let d = deps();
        let request =
            ClarificationRequest::new(vec![secret_question("token")]).expect("one question");
        let err = TURN_CONTEXT
            .scope(routable_turn(), async move { ask(&d, request).await })
            .await
            .expect_err("no bus is wired in a unit test, so delivery still fails");
        // The refusal is about delivery, NOT about the question being secret.
        let msg = err.to_string();
        assert!(msg.contains("failed to deliver"), "{msg}");
        assert!(!msg.contains("permanent message"), "{msg}");
    }
}
