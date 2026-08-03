//! Host side of MCP sampling: what actually answers `sampling/createMessage`.
//!
//! The plumbing between an MCP server's sampling request and `SamplingHandler`
//! was complete except for this — nobody ever registered a callback. Because
//! every connection was constructed with a `SamplingHandler` regardless,
//! `can_sample()` was structurally true and Aleph declared the `sampling`
//! capability to every server it spoke to, then answered the resulting requests
//! with "No sampling callback registered". A declared capability is a promise;
//! this module is the other half of it.
//!
//! Shape follows the existing session-end hooks (`memory_context_provider`):
//! a process-wide `OnceCell` registered at startup, read lazily at call time.
//! Lazy is required, not incidental — the MCP manager spawns (and servers
//! handshake) before the agent's provider exists, so a callback that captured a
//! provider eagerly could only ever be installed too late to be declared.

use crate::sync_primitives::Arc;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::mcp::{
    PromptRole, SamplingContent, SamplingRequest, SamplingResponse, StopReason,
};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;

/// Ceiling on a single sampling completion.
///
/// A sampling request is an *MCP server* spending the user's tokens, so the
/// server's own `max_tokens` is treated as a request, not an authority.
const MAX_SAMPLING_TOKENS: u32 = 4096;

static SAMPLING_LLM: tokio::sync::OnceCell<Arc<dyn AiProvider>> =
    tokio::sync::OnceCell::const_new();

/// Register the provider that answers MCP sampling requests.
/// Idempotent; the first call wins.
pub fn register_sampling_llm(provider: Arc<dyn AiProvider>) {
    let _ = SAMPLING_LLM.set(provider);
}

/// Whether a sampling provider has been registered.
#[must_use]
pub fn sampling_llm_registered() -> bool {
    SAMPLING_LLM.get().is_some()
}

/// Answer one `sampling/createMessage`.
///
/// Deliberately a single-shot completion with no tools: the requesting server
/// asked for a model's words, not for an agent that can act on its behalf.
pub async fn serve_sampling(request: SamplingRequest) -> Result<SamplingResponse> {
    let provider = SAMPLING_LLM.get().ok_or_else(|| {
        AlephError::IoError(
            "MCP sampling requested but no LLM provider is registered on this host".to_string(),
        )
    })?;

    let messages: Vec<UnifiedMessage> = request.messages.iter().map(to_unified).collect();
    if messages.is_empty() {
        return Err(AlephError::IoError(
            "MCP sampling request carried no messages".to_string(),
        ));
    }

    let max_tokens = request
        .max_tokens
        .map_or(MAX_SAMPLING_TOKENS, |n| n.min(MAX_SAMPLING_TOKENS));

    let payload = RequestPayload::new(&messages)
        .with_system(request.system_prompt.as_deref())
        .with_max_tokens(Some(max_tokens));

    let response = provider.process(payload).await?;

    Ok(SamplingResponse {
        role: PromptRole::Assistant,
        content: SamplingContent::Text {
            text: response.text_content(),
        },
        model: None,
        stop_reason: Some(StopReason::EndTurn),
    })
}

/// Flatten one sampling message onto a provider message.
///
/// `System` collapses to a user turn: the wire type allows the role per-message
/// while `RequestPayload` carries one system prompt for the whole request, and
/// dropping the text outright would silently lose the server's instruction.
fn to_unified(msg: &crate::mcp::jsonrpc::mcp::SamplingMessage) -> UnifiedMessage {
    let text = match &msg.content {
        SamplingContent::Text { text } => text.clone(),
        // Images are declined rather than mangled: `image_capability` is not
        // part of what this host advertises.
        SamplingContent::Image { mime_type, .. } => {
            format!("[unsupported {mime_type} image omitted]")
        }
    };
    match msg.role {
        PromptRole::Assistant => UnifiedMessage::assistant(text),
        PromptRole::User | PromptRole::System => UnifiedMessage::user(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::jsonrpc::mcp::SamplingMessage;

    fn text_msg(role: PromptRole, text: &str) -> SamplingMessage {
        SamplingMessage {
            role,
            content: SamplingContent::Text { text: text.into() },
        }
    }

    #[test]
    fn roles_map_onto_provider_messages() {
        assert!(matches!(
            to_unified(&text_msg(PromptRole::Assistant, "hi")),
            UnifiedMessage::Assistant { .. }
        ));
        assert!(matches!(
            to_unified(&text_msg(PromptRole::User, "hi")),
            UnifiedMessage::User { .. }
        ));
        // A system-role message must still reach the model somehow.
        assert!(matches!(
            to_unified(&text_msg(PromptRole::System, "be terse")),
            UnifiedMessage::User { .. }
        ));
    }

    #[test]
    fn image_content_degrades_to_a_visible_placeholder() {
        let m = to_unified(&SamplingMessage {
            role: PromptRole::User,
            content: SamplingContent::Image {
                data: "AAAA".into(),
                mime_type: "image/png".into(),
            },
        });
        assert!(m.text_content().contains("image/png"));
    }

    /// The server's `max_tokens` is a request, not an authority — it is another
    /// process asking to spend this user's tokens.
    #[test]
    fn server_max_tokens_is_capped() {
        let clamp = |asked: Option<u32>| {
            asked.map_or(MAX_SAMPLING_TOKENS, |n| n.min(MAX_SAMPLING_TOKENS))
        };
        assert_eq!(clamp(None), MAX_SAMPLING_TOKENS);
        assert_eq!(clamp(Some(100)), 100);
        assert_eq!(clamp(Some(1_000_000)), MAX_SAMPLING_TOKENS);
    }

    #[tokio::test]
    async fn errors_clearly_when_no_provider_is_registered() {
        // The global may have been set by another test in this binary; only the
        // unregistered case has a determinate message to assert.
        if sampling_llm_registered() {
            return;
        }
        let err = serve_sampling(SamplingRequest {
            messages: vec![text_msg(PromptRole::User, "hi")],
            model_preferences: None,
            system_prompt: None,
            include_context: None,
            max_tokens: None,
        })
        .await
        .expect_err("no provider registered");
        assert!(err.to_string().contains("no LLM provider is registered"));
    }
}
