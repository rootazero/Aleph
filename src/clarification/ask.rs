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
//! registered transport the tool refuses outright with an actionable error
//! rather than routing the credential through it. That is a property of the
//! shape, not of a caller remembering to check.

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

/// Register `request` for the current turn, deliver it, and park until the
/// human has answered every question (or the wait times out).
///
/// # Errors
///
/// Returns a tool error when no human can be reached — no turn context, an
/// unattended run, no transport that can carry the question, or a secret
/// question on a channel turn. Every one of them is immediate: a tool that
/// cannot be answered must fail fast, never occupy the 600-second wait.
pub async fn ask(
    deps: &ClarificationDeps,
    request: ClarificationRequest,
) -> Result<ClarificationResult> {
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

    let wants_secret = request.questions.iter().any(|q| q.secret);
    let channel = ChannelId::new(&turn.channel_id);
    // Resolved BEFORE registering so a refusal never leaves a pending entry
    // behind. `None` is the Panel's `gui:chat` pseudo-channel (and any turn
    // whose channel is stopped): no third-party transport exists, so a secret
    // has somewhere safe to go.
    let channel_is_third_party = deps.channels.get(&channel).await.is_some();
    if wants_secret && channel_is_third_party {
        return Err(AlephError::tool(format!(
            "cannot ask for a secret over `{}`: the reply would be a permanent message in that \
             service's history. Ask the user to set it through the Panel, or via configuration, \
             and continue without it.",
            turn.channel_id
        )));
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
    // which closes the receiver and lets `is_live` retire the entry.
    match tokio::time::timeout(DEFAULT_CLARIFY_TIMEOUT, rx).await {
        Ok(Ok(result)) => Ok(result),
        // Sender dropped without sending — treat as cancelled.
        Ok(Err(_)) => Ok(ClarificationResult::cancelled()),
        // Timed out — reap the stale registry entry. `cleanup_expired` rather
        // than `cancel` so the terminal frame clients receive says `expired`,
        // matching the status returned here; the entry is past its deadline by
        // construction (same duration, registered first).
        Err(_) => {
            deps.clarification.cleanup_expired().await;
            Ok(ClarificationResult::timeout())
        }
    }
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
        }
    }

    async fn ask_in(
        turn: TurnContext,
        request: ClarificationRequest,
    ) -> Result<ClarificationResult> {
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

    /// A secret must never be routed to a third-party channel. The registry in
    /// this fixture holds no channel, so `gui:chat`-shaped turns fall through
    /// to the bus and only the delivery error remains — which is the point:
    /// the refusal below is about the transport, not about the question.
    #[tokio::test]
    async fn a_secret_question_is_refused_when_the_turn_has_a_real_channel() {
        let registry = Arc::new(ChannelRegistry::new());
        registry
            .register(Box::new(RegisteredChannel::new("telegram")))
            .await;
        let d = ClarificationDeps::new(Arc::new(ClarificationManager::new()), registry);
        let request = ClarificationRequest::new(vec![ClarificationQuestion::text(
            "token",
            "Paste the API token",
        )
        .with_secret(true)])
        .expect("one question");

        let err = TURN_CONTEXT
            .scope(routable_turn(), async move { ask(&d, request).await })
            .await
            .expect_err("a secret must not be routed through a third-party channel");
        let msg = err.to_string();
        assert!(msg.contains("secret"), "{msg}");
        assert!(msg.contains("permanent message"), "{msg}");
        // No pending entry may be left behind by a refusal.
        assert!(msg.contains("continue without it"), "{msg}");
    }
}
