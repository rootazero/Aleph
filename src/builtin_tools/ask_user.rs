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
//! 3. Deliver the rendered question to the channel.
//! 4. Block on the oneshot until the inbound router routes the user's reply
//!    back via `ClarificationManager::resolve`, or the timeout fires.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::clarification::{
    ClarificationManager, ClarificationOption, ClarificationRequest, ClarificationResult,
    ClarificationResultType, DEFAULT_CLARIFY_TIMEOUT,
};
use crate::error::{AlephError, Result};
use crate::gateway::channel::{ChannelId, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::sync_primitives::Arc;
use crate::tools::turn_context::current_turn_context;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for the `ask_user` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AskUserArgs {
    /// The question to ask the user. Make it specific and self-contained —
    /// the user sees only this text, not the surrounding task context.
    pub question: String,

    /// Optional list of choices. When non-empty the user is asked to pick one;
    /// their reply is matched by number, by label, or taken as free text.
    #[serde(default)]
    pub choices: Vec<String>,
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

    /// Build the clarification request and the channel-rendered prompt.
    fn build_request(
        request_id: &str,
        question: &str,
        choices: &[String],
    ) -> (ClarificationRequest, String) {
        if choices.is_empty() {
            (
                ClarificationRequest::text(request_id, question, None),
                format!("❓ {question}\n\nReply with your answer."),
            )
        } else {
            let options: Vec<ClarificationOption> = choices
                .iter()
                .map(|c| ClarificationOption::new(c, c))
                .collect();
            let mut menu = String::new();
            for (i, choice) in choices.iter().enumerate() {
                menu.push_str(&format!("{}. {}\n", i + 1, choice));
            }
            (
                ClarificationRequest::select(request_id, question, options),
                format!("❓ {question}\n\n{menu}\nReply with the number or your answer."),
            )
        }
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

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"ask_user(question="Which file should I edit?")"#.to_string(),
            r#"ask_user(question="Deploy to which environment?", choices=["staging", "production"])"#
                .to_string(),
        ])
    }

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

        let request_id = uuid::Uuid::new_v4().to_string();
        let (request, rendered) = Self::build_request(&request_id, question, &args.choices);

        // Register BEFORE delivery so a reply arriving immediately is not lost.
        let rx = self
            .clarification
            .register(session_key.clone(), request, DEFAULT_CLARIFY_TIMEOUT)
            .await;

        // Deliver the question to the originating channel.
        let message = OutboundMessage::text(turn.conversation_id.clone(), rendered);
        if let Err(e) = self
            .channels
            .send(&ChannelId::new(&turn.channel_id), message)
            .await
        {
            // Delivery failed — nobody can ever answer. Drop the registration.
            self.clarification.cancel(&session_key).await;
            warn!(error = %e, "ask_user: failed to deliver question to channel");
            return Err(AlephError::tool(format!(
                "ask_user: failed to deliver the question to the user's channel: {e}"
            )));
        }
        info!(session = %session_key, "ask_user: question delivered — awaiting reply");

        // Block until the user replies, the request is superseded, or the
        // timeout fires. The explicit timeout guarantees the tool never hangs
        // even if no cleanup pass reaps the registry entry.
        let result = match tokio::time::timeout(DEFAULT_CLARIFY_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            // Sender dropped without sending — treat as cancelled.
            Ok(Err(_)) => ClarificationResult::cancelled(),
            // Timed out — reap the stale registry entry.
            Err(_) => {
                self.clarification.cancel(&session_key).await;
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
    async fn errors_when_channel_delivery_fails() {
        // Routable turn, but the registry has no such channel — delivery fails
        // and the pending clarification is rolled back inside `call`.
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

    #[test]
    fn build_request_text_vs_select() {
        let (text_req, text_prompt) = AskUserTool::build_request("id-1", "Pick?", &[]);
        assert!(text_req.options.is_none());
        assert!(text_prompt.contains("Reply with your answer"));

        let (select_req, select_prompt) =
            AskUserTool::build_request("id-2", "Pick?", &["alpha".to_string(), "beta".to_string()]);
        assert_eq!(select_req.options.as_ref().map(|o| o.len()), Some(2));
        assert!(select_prompt.contains("1. alpha"));
        assert!(select_prompt.contains("2. beta"));
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
