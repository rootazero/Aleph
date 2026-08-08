//! `ask_user` — mid-task clarification tool.
//!
//! Lets the agent pause, ask the user a question through the originating
//! channel, and resume once the user replies. The agent loop blocks inside
//! this tool's `call` until the reply arrives, the question is superseded, or
//! the clarification times out.
//!
//! Flow:
//! 1. Read the turn's `TURN_CONTEXT` for the originating channel + session.
//! 2. Register a [`ClarificationRequest`] with the [`ClarificationManager`]
//!    keyed by the session — *before* delivery, so a fast reply is never lost.
//! 3. Deliver the rendered question — to the originating channel when one is
//!    registered, otherwise onto the gateway event bus as a `stream.ask_user`
//!    frame (see [`publish_to_event_bus`]).
//! 4. Block on the oneshot until the reply arrives, or the timeout fires.
//!
//! Both delivery paths converge on `ClarificationManager::resolve`, keyed by
//! the same session: a channel reply (typed text or a `clarify:<idx>` inline
//! button) is routed there by `inbound_router::try_intercept_hitl`; the Panel's
//! answer to the `stream.ask_user` card is routed there by the
//! `clarification.resolve` RPC (`gateway::handlers::clarification`).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::clarification::{
    ClarificationManager, ClarificationOption, ClarificationRequest, ClarificationResult,
    ClarificationResultType, DEFAULT_CLARIFY_TIMEOUT,
};
use crate::error::{AlephError, Result};
use crate::gateway::channel::{ChannelId, InlineButton, InlineKeyboard, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::events::GatewayEventFrame;
use crate::sync_primitives::Arc;
use crate::tools::turn_context::{current_turn_context, TurnContext};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// A single choice offered to the user.
///
/// Accepts either a bare string (`"staging"`) or an object with an
/// explanatory description (`{"label": "staging", "description": "shared QA
/// environment"}`). The bare-string form keeps backward compatibility with
/// the simple `choices: ["a", "b"]` shape.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum AskUserChoice {
    /// A simple choice label (also used as the returned value).
    Simple(String),
    /// A labeled choice with a short description shown beside it.
    Detailed {
        /// The choice label (also used as the returned value).
        label: String,
        /// A short description helping the user choose.
        description: String,
    },
}

impl AskUserChoice {
    /// The label/value the user picks and that is returned as the answer.
    fn label(&self) -> &str {
        match self {
            Self::Simple(label) | Self::Detailed { label, .. } => label,
        }
    }

    /// The optional description shown beside the label.
    fn description(&self) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::Detailed { description, .. } => Some(description),
        }
    }
}

/// Render a compact button label `"<n>. <label>"`, truncated so a long choice
/// doesn't bloat the keyboard — the full text is always listed in the message
/// body, so the button only needs to be tappable, not complete.
fn button_label(index: usize, label: &str) -> String {
    /// Max button label length (chars) before truncation.
    const MAX_LABEL_CHARS: usize = 32;

    let text = format!("{index}. {label}");
    if text.chars().count() > MAX_LABEL_CHARS {
        let truncated: String = text.chars().take(MAX_LABEL_CHARS - 1).collect();
        format!("{truncated}…")
    } else {
        text
    }
}

/// Arguments for the `ask_user` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AskUserArgs {
    /// The question to ask the user. Make it specific and self-contained —
    /// the user sees only this text, not the surrounding task context.
    pub question: String,

    /// Optional list of choices. When non-empty the user is asked to pick one;
    /// their reply is matched by number, by label, or taken as free text. Each
    /// choice may be a plain string or an object with a `label` and a short
    /// `description` to help the user decide.
    #[serde(default)]
    pub choices: Vec<AskUserChoice>,
}

/// Output of the `ask_user` tool.
#[derive(Debug, Clone, Serialize)]
pub struct AskUserOutput {
    /// `"answered"`, `"timeout"`, or `"cancelled"`.
    pub status: String,

    /// The user's answer — selected option value or free text. Absent on
    /// timeout or cancellation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,

    /// 0-based index of the chosen option, when a choice list was offered and
    /// the reply matched one of the choices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_index: Option<u32>,
}

/// Publish the question on the gateway event bus as a `stream.ask_user` frame.
///
/// This is the producer for the `AskUser` consumer chain — the Panel renders
/// the question inline against `run_id` and answers it with
/// `clarification.resolve` on the frame's `session_key`, and the R5 surface
/// router turns it into an "Aleph has a question" notification. Used when the
/// turn's channel cannot take the question: the Panel talks to core over the
/// `gui:chat` pseudo-channel, which is never registered in the
/// `ChannelRegistry`.
///
/// Returns `false` when the question cannot be published — no bus wired (CLI
/// subcommands, unit tests) or no gateway run to correlate against — so the
/// caller can roll the pending clarification back instead of blocking on a
/// reply that can never arrive.
fn publish_to_event_bus(turn: &TurnContext, question: &str, choices: &[AskUserChoice]) -> bool {
    if turn.run_id.is_empty() {
        return false;
    }
    let Some(bus) = crate::gateway::event_emitter::gateway_event_bus() else {
        return false;
    };
    if let Err(e) = bus.publish_frame(&build_ask_user_frame(turn, question, choices)) {
        warn!(error = %e, "ask_user: failed to publish the question on the event bus");
        return false;
    }
    true
}

/// Build the `AskUser` frame published by [`publish_to_event_bus`].
fn build_ask_user_frame(
    turn: &TurnContext,
    question: &str,
    choices: &[AskUserChoice],
) -> GatewayEventFrame {
    GatewayEventFrame::AskUser {
        run_id: turn.run_id.clone(),
        // The run's emitter owns the stream sequence counter and an
        // out-of-band producer has no handle on it. Same convention as the
        // operator approval requester's out-of-band frames.
        seq: 0,
        // The key `register` used above — the Panel posts its answer back
        // against exactly this, so the two can never drift.
        session_key: turn.session_key.to_string(),
        question: question.to_string(),
        options: choices.iter().map(|c| c.label().to_string()).collect(),
    }
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that asks the user a clarifying question and waits for the reply.
#[derive(Clone)]
pub struct AskUserTool {
    clarification: Arc<ClarificationManager>,
    channels: Arc<ChannelRegistry>,
}

impl AskUserTool {
    pub const fn new(
        clarification: Arc<ClarificationManager>,
        channels: Arc<ChannelRegistry>,
    ) -> Self {
        Self {
            clarification,
            channels,
        }
    }

    /// Build the clarification request, the channel-rendered prompt, and an
    /// optional inline keyboard mirroring the choices.
    ///
    /// The keyboard is the clarification twin of the approval `approve:`
    /// keyboard ([`crate::exec::ApprovalBridge`]): each choice becomes a button
    /// whose `callback_data` is `clarify:<1-based index>`, re-injected through
    /// the normal pipeline and resolved by the inbound router's HITL
    /// interception. The numbered text menu is always rendered too, so channels
    /// without inline-keyboard support degrade gracefully to a typed reply.
    fn build_request(
        question: &str,
        choices: &[AskUserChoice],
    ) -> (ClarificationRequest, String, Option<InlineKeyboard>) {
        if choices.is_empty() {
            return (
                ClarificationRequest::text(question),
                format!("❓ {question}\n\nReply with your answer."),
                None,
            );
        }
        let options: Vec<ClarificationOption> = choices
            .iter()
            .map(|c| {
                let opt = ClarificationOption::new(c.label(), c.label());
                match c.description() {
                    Some(desc) => opt.with_description(desc),
                    None => opt,
                }
            })
            .collect();
        let mut menu = String::new();
        for (i, choice) in choices.iter().enumerate() {
            match choice
                .description()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                Some(desc) => {
                    menu.push_str(&format!("{}. {} — {desc}\n", i + 1, choice.label()));
                }
                None => menu.push_str(&format!("{}. {}\n", i + 1, choice.label())),
            }
        }
        (
            ClarificationRequest::select(question, options),
            format!("❓ {question}\n\n{menu}\nReply with the number or your answer."),
            Self::build_choice_keyboard(choices),
        )
    }

    /// Build an inline keyboard for `choices`, two buttons per row.
    ///
    /// Returns `None` when there are too many choices to render compactly — the
    /// numbered text body always lists every choice, so a long list simply
    /// falls back to typed selection and the keyboard payload stays well under
    /// the channel's per-message limits.
    fn build_choice_keyboard(choices: &[AskUserChoice]) -> Option<InlineKeyboard> {
        /// Max choices rendered as buttons; beyond this the menu is text-only.
        const MAX_CHOICE_BUTTONS: usize = 12;

        if choices.is_empty() || choices.len() > MAX_CHOICE_BUTTONS {
            return None;
        }
        let buttons: Vec<InlineButton> = choices
            .iter()
            .enumerate()
            .map(|(i, c)| InlineButton {
                text: button_label(i + 1, c.label()),
                // 1-based index; the router strips `clarify:` and resolves the
                // pending clarification with the bare number (see
                // `try_intercept_hitl`).
                callback_data: format!("clarify:{}", i + 1),
            })
            .collect();
        let mut keyboard = InlineKeyboard::new();
        for chunk in buttons.chunks(2) {
            keyboard.rows.push(chunk.to_vec());
        }
        Some(keyboard)
    }

    /// Map a resolved [`ClarificationResult`] onto the tool output.
    fn result_to_output(result: ClarificationResult) -> AskUserOutput {
        match result.result_type {
            ClarificationResultType::Selected | ClarificationResultType::TextInput => {
                AskUserOutput {
                    status: "answered".to_string(),
                    answer: result.value,
                    selected_index: result.selected_index,
                }
            }
            ClarificationResultType::Timeout => AskUserOutput {
                status: "timeout".to_string(),
                answer: None,
                selected_index: None,
            },
            ClarificationResultType::Cancelled => AskUserOutput {
                status: "cancelled".to_string(),
                answer: None,
                selected_index: None,
            },
        }
    }
}

#[async_trait]
impl AlephTool for AskUserTool {
    const NAME: &'static str = "ask_user";
    const DESCRIPTION: &'static str =
        "Ask the user a clarifying question and wait for their reply before continuing. \
         Use this when the task is ambiguous, a required detail is missing, or you need \
         the user to choose between options — instead of guessing. Optionally pass a list \
         of `choices` to offer a menu. The agent pauses until the user answers; the reply \
         (or a timeout) is returned so you can resume.";

    type Args = AskUserArgs;
    type Output = AskUserOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let question = args.question.trim();
        if question.is_empty() {
            return Err(AlephError::tool("ask_user: `question` must not be empty"));
        }

        // Resolve the originating channel for this turn.
        let Some(turn) = current_turn_context() else {
            return Err(AlephError::tool(
                "ask_user is only available inside an interactive channel turn",
            ));
        };
        if !turn.is_channel_routable() {
            return Err(AlephError::tool(
                "ask_user: this turn has no interactive channel to reach the user",
            ));
        }
        let session_key = turn.session_key.to_string();

        let (request, rendered, keyboard) = Self::build_request(question, &args.choices);

        // Register BEFORE delivery so a reply arriving immediately is not lost.
        let rx = self
            .clarification
            .register(session_key.clone(), request, DEFAULT_CLARIFY_TIMEOUT)
            .await;

        // Deliver the question to the originating channel. Channels that render
        // inline keyboards (e.g. Telegram) show tappable choice buttons; the
        // rest fall back to the numbered text menu.
        let mut message = OutboundMessage::text(turn.conversation_id.clone(), rendered);
        message.inline_keyboard = keyboard;
        if let Err(e) = self
            .channels
            .send(&ChannelId::new(&turn.channel_id), message)
            .await
        {
            // The Panel's `gui:chat` is a pseudo-channel that is never
            // registered in the `ChannelRegistry`, so the channel transport
            // alone denies every Panel question. Fall back to the gateway
            // event bus — mirrors the approval path's channel → operator
            // fallback (`exec::approval::FallbackApprovalRequester`).
            if !publish_to_event_bus(&turn, question, &args.choices) {
                // Neither transport can reach the user — nobody can ever
                // answer. Drop the registration.
                self.clarification.cancel(&session_key).await;
                warn!(error = %e, "ask_user: failed to deliver question to channel");
                return Err(AlephError::tool(format!(
                    "ask_user: failed to deliver the question to the user's channel: {e}"
                )));
            }
        }
        info!(session = %session_key, "ask_user: question delivered — awaiting reply");

        // Block until the user replies, the request is superseded, or the
        // timeout fires. The explicit timeout guarantees the tool never hangs
        // even if no cleanup pass reaps the registry entry.
        let result = match tokio::time::timeout(DEFAULT_CLARIFY_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            // Sender dropped without sending — treat as cancelled.
            Ok(Err(_)) => ClarificationResult::cancelled(),
            // Timed out — reap the stale registry entry. `cleanup_expired`
            // rather than `cancel` so the terminal frame clients receive says
            // `expired`, matching the status returned here; the entry is past
            // its deadline by construction (same duration, registered first).
            Err(_) => {
                self.clarification.cleanup_expired().await;
                ClarificationResult::timeout()
            }
        };

        Ok(Self::result_to_output(result))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    fn tool() -> AskUserTool {
        AskUserTool::new(
            Arc::new(ClarificationManager::new()),
            Arc::new(ChannelRegistry::new()),
        )
    }

    fn routable_turn() -> TurnContext {
        TurnContext {
            session_key: crate::routing::session_key::SessionKey::ephemeral("ask-user-test"),
            run_id: String::new(),
            channel_id: "telegram".to_string(),
            conversation_id: "user-1".to_string(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
        }
    }

    #[tokio::test]
    async fn errors_when_question_empty() {
        let err = tool()
            .call(AskUserArgs {
                question: "   ".to_string(),
                choices: vec![],
            })
            .await
            .expect_err("empty question must be rejected");
        assert!(err.to_string().contains("must not be empty"));
    }

    #[tokio::test]
    async fn errors_without_turn_context() {
        // No TURN_CONTEXT scope — the tool cannot reach any channel.
        let err = tool()
            .call(AskUserArgs {
                question: "Which one?".to_string(),
                choices: vec![],
            })
            .await
            .expect_err("missing turn context must be rejected");
        assert!(err.to_string().contains("interactive channel turn"));
    }

    #[tokio::test]
    async fn errors_on_non_routable_turn() {
        let non_channel_turn = TurnContext {
            session_key: crate::routing::session_key::SessionKey::task("main", "cron", "daily"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
        };
        let err = TURN_CONTEXT
            .scope(non_channel_turn, async {
                tool()
                    .call(AskUserArgs {
                        question: "Which one?".to_string(),
                        choices: vec![],
                    })
                    .await
            })
            .await
            .expect_err("non-routable turn must be rejected");
        assert!(err.to_string().contains("no interactive channel"));
    }

    #[tokio::test]
    async fn errors_when_no_transport_can_reach_the_user() {
        // Routable turn, but the registry has no such channel AND the turn has
        // no gateway run (`run_id` empty) to publish an `AskUser` frame
        // against — neither transport can reach the user, so the pending
        // clarification is rolled back inside `call` instead of blocking on a
        // reply that can never arrive.
        let err = TURN_CONTEXT
            .scope(routable_turn(), async {
                tool()
                    .call(AskUserArgs {
                        question: "Which one?".to_string(),
                        choices: vec![],
                    })
                    .await
            })
            .await
            .expect_err("delivery failure must surface as an error");
        assert!(err.to_string().contains("failed to deliver"));
    }

    /// The Panel's `gui:chat` is a pseudo-channel that is never registered, so
    /// the channel transport can never carry a Panel question. The event-bus
    /// frame is the producer the `AskUser` consumer chain was missing: it
    /// carries the question + the choice labels against the run id the Panel
    /// renders into, plus the session key the Panel posts its answer back on.
    #[test]
    fn ask_user_frame_carries_run_id_session_key_question_and_choice_labels() {
        let turn = TurnContext {
            run_id: "run-panel-1".to_string(),
            channel_id: "gui:chat".to_string(),
            ..routable_turn()
        };
        let expected_session = turn.session_key.to_string();
        let frame = build_ask_user_frame(
            &turn,
            "Deploy where?",
            &[
                AskUserChoice::Simple("staging".to_string()),
                AskUserChoice::Detailed {
                    label: "production".to_string(),
                    description: "live traffic".to_string(),
                },
            ],
        );
        match frame {
            GatewayEventFrame::AskUser {
                run_id,
                session_key,
                question,
                options,
                ..
            } => {
                assert_eq!(run_id, "run-panel-1");
                // The key `register` used — `clarification.resolve` will not
                // find the pending entry under any other string.
                assert_eq!(session_key, expected_session);
                assert_eq!(question, "Deploy where?");
                // Labels only — the same values the clarification resolves on.
                assert_eq!(options, vec!["staging", "production"]);
            }
            other => panic!("expected an AskUser frame, got {other:?}"),
        }
    }

    #[test]
    fn build_request_text_vs_select() {
        let (text_req, text_prompt, text_kb) = AskUserTool::build_request("Pick?", &[]);
        assert!(text_req.options.is_none());
        assert!(text_prompt.contains("Reply with your answer"));
        // No choices → open-ended → no keyboard (mirrors hermes' open-ended path).
        assert!(text_kb.is_none());

        let (select_req, select_prompt, select_kb) = AskUserTool::build_request(
            "Pick?",
            &[
                AskUserChoice::Simple("alpha".to_string()),
                AskUserChoice::Simple("beta".to_string()),
            ],
        );
        assert_eq!(select_req.options.as_ref().map(|o| o.len()), Some(2));
        assert!(select_prompt.contains("1. alpha"));
        assert!(select_prompt.contains("2. beta"));
        // Two choices → an inline keyboard with one `clarify:<idx>` button each.
        let kb = select_kb.expect("choices must produce a keyboard");
        let datas: Vec<&str> = kb
            .rows
            .iter()
            .flatten()
            .map(|b| b.callback_data.as_str())
            .collect();
        assert_eq!(datas, vec!["clarify:1", "clarify:2"]);
    }

    #[test]
    fn build_request_detailed_choice_renders_and_wires_description() {
        let (req, prompt, _kb) = AskUserTool::build_request(
            "Strategy?",
            &[
                AskUserChoice::Detailed {
                    label: "in-place".to_string(),
                    description: "brief downtime".to_string(),
                },
                AskUserChoice::Simple("blue-green".to_string()),
            ],
        );
        // Description is wired onto the ClarificationOption, not just rendered.
        let options = req.options.expect("select request must carry options");
        assert_eq!(options[0].value, "in-place");
        assert_eq!(options[0].description.as_deref(), Some("brief downtime"));
        assert!(options[1].description.is_none());
        // Rendered menu surfaces the description with an em dash separator.
        assert!(prompt.contains("1. in-place — brief downtime"));
        assert!(prompt.contains("2. blue-green\n"));
    }

    #[test]
    fn keyboard_caps_long_choice_lists_to_text_only() {
        // Beyond MAX_CHOICE_BUTTONS the keyboard is suppressed (text menu still
        // lists every choice), keeping the callback payload bounded.
        let many: Vec<AskUserChoice> = (0..20)
            .map(|i| AskUserChoice::Simple(format!("opt{i}")))
            .collect();
        let kb = AskUserTool::build_choice_keyboard(&many);
        assert!(
            kb.is_none(),
            "oversized choice lists must not render buttons"
        );

        // At the cap boundary the keyboard is still rendered.
        let twelve: Vec<AskUserChoice> = (0..12)
            .map(|i| AskUserChoice::Simple(format!("opt{i}")))
            .collect();
        let kb = AskUserTool::build_choice_keyboard(&twelve).expect("12 choices render");
        assert_eq!(kb.rows.iter().flatten().count(), 12);
    }

    #[test]
    fn button_label_truncates_long_choice() {
        let short = button_label(1, "staging");
        assert_eq!(short, "1. staging");
        let long = button_label(2, &"x".repeat(80));
        assert!(long.chars().count() <= 32, "label too long: {long}");
        assert!(long.ends_with('…'));
    }

    #[test]
    fn ask_user_choice_deserializes_string_and_object_forms() {
        // Backward-compatible bare-string form.
        let simple: AskUserChoice = serde_json::from_str(r#""staging""#).unwrap();
        assert_eq!(simple.label(), "staging");
        assert!(simple.description().is_none());
        // Richer object form.
        let detailed: AskUserChoice =
            serde_json::from_str(r#"{"label":"prod","description":"live traffic"}"#).unwrap();
        assert_eq!(detailed.label(), "prod");
        assert_eq!(detailed.description(), Some("live traffic"));
    }

    #[test]
    fn result_to_output_maps_each_status() {
        let answered =
            AskUserTool::result_to_output(ClarificationResult::text_input("hi".to_string()));
        assert_eq!(answered.status, "answered");
        assert_eq!(answered.answer.as_deref(), Some("hi"));

        let selected =
            AskUserTool::result_to_output(ClarificationResult::selected(1, "beta".to_string()));
        assert_eq!(selected.status, "answered");
        assert_eq!(selected.selected_index, Some(1));

        let timed_out = AskUserTool::result_to_output(ClarificationResult::timeout());
        assert_eq!(timed_out.status, "timeout");

        let cancelled = AskUserTool::result_to_output(ClarificationResult::cancelled());
        assert_eq!(cancelled.status, "cancelled");
    }
}
