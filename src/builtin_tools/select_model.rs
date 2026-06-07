//! SelectModelTool — LLM-facing model picker (R8 "everything is a tool").
//!
//! The "A layer" of AI dynamic routing: lets the main-loop LLM choose the model
//! for the rest of the conversation in one inference, rather than a separate
//! routing model deciding for it (which would violate R7/R9). The pick is
//! recorded in [`session_model_handle`](crate::providers::session_model_handle)
//! and applied at the next turn's run construction (`harness_bridge`), where it
//! wraps the chosen provider in a `ModelOverrideProvider` to stamp the model.
//!
//! Model binding is per-run, so a pick takes effect from the *next* turn — the
//! tool says so in its response.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::providers::session_model_handle;
use crate::tools::turn_context::current_turn_context;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SelectModelArgs {
    /// Model id to use for the rest of this conversation, e.g.
    /// `"claude-opus-4"`, `"gpt-5"`, `"deepseek-chat"`.
    #[schemars(description = "Model id to switch to for the rest of this conversation.")]
    pub model: String,
    /// Optional provider id to pin (e.g. `"openai"`, `"anthropic"`). Omit to
    /// let the system route by model name / fall back to the default provider.
    #[serde(default)]
    #[schemars(description = "Optional provider id to pin; omit to auto-route by model name.")]
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SelectModelOutput {
    /// True when the preference was recorded.
    pub ok: bool,
    /// The model now selected.
    pub model: String,
    /// The provider pinned, if any.
    pub provider: Option<String>,
    /// Human-readable confirmation.
    pub message: String,
}

#[derive(Clone, Default)]
pub struct SelectModelTool;

#[async_trait]
impl AlephTool for SelectModelTool {
    const NAME: &'static str = "select_model";
    const DESCRIPTION: &'static str = "Switch the LLM model for the rest of this conversation. \
        Use when a task needs a different model than the current one — e.g. a larger context \
        window for a big document, a vision-capable model for images, a reasoning model for hard \
        problems, or a cheaper model for simple chat. Pass `model` (required) and optionally \
        `provider` to pin a specific provider. The change applies from the next turn (the current \
        turn finishes on the current model).";

    type Args = SelectModelArgs;
    type Output = SelectModelOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.model);

        // The session to scope the preference to comes from the task-local
        // turn context set by the tool dispatcher. Outside a turn (should not
        // happen for a real tool call) there is nothing to scope to.
        let Some(ctx) = current_turn_context() else {
            let message =
                "No active session: select_model must run inside a conversation turn.".to_string();
            notify_tool_result(Self::NAME, &message, false);
            return Ok(SelectModelOutput {
                ok: false,
                model: args.model,
                provider: args.provider,
                message,
            });
        };

        let key = ctx.session_key.to_key_string();
        session_model_handle::set_session_model(&key, args.provider.clone(), args.model.clone());

        let message = match &args.provider {
            Some(p) => format!(
                "Model switched to '{}' (provider '{}'); takes effect from the next turn.",
                args.model, p
            ),
            None => format!(
                "Model switched to '{}'; takes effect from the next turn.",
                args.model
            ),
        };
        notify_tool_result(Self::NAME, &message, true);
        Ok(SelectModelOutput {
            ok: true,
            model: args.model,
            provider: args.provider,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
    use crate::tools::AlephTool;

    #[tokio::test]
    async fn writes_session_model_under_turn_context() {
        let sk = SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "select-model-test".to_string(),
        };
        let key = sk.to_key_string();
        session_model_handle::clear_session_model(&key);

        let ctx = TurnContext {
            session_key: sk.clone(),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
        };
        let out = TURN_CONTEXT
            .scope(ctx, async {
                SelectModelTool
                    .call(SelectModelArgs {
                        model: "gpt-5".to_string(),
                        provider: Some("openai".to_string()),
                    })
                    .await
            })
            .await
            .unwrap();

        assert!(out.ok);
        let pref = session_model_handle::get_session_model(&key).unwrap();
        assert_eq!(pref.model, "gpt-5");
        assert_eq!(pref.provider.as_deref(), Some("openai"));
        session_model_handle::clear_session_model(&key);
    }

    #[tokio::test]
    async fn no_turn_context_is_graceful() {
        // Outside a turn scope there is no session to bind to — degrade, not panic.
        let out = SelectModelTool
            .call(SelectModelArgs {
                model: "claude-opus-4".to_string(),
                provider: None,
            })
            .await
            .unwrap();
        assert!(!out.ok);
    }
}
