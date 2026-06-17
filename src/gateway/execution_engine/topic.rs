//! Shared conversation-topic generation.
//!
//! Single source of truth for "turn the first user message into a short
//! title". Used by both single chat (`execute.rs` auto-topic) and team
//! group chat (`handlers/teams.rs` first-message auto-name) so the prompt
//! and fallback never drift.

use crate::sync_primitives::Arc;

/// Fallback when the LLM returns nothing usable: truncate the message to 20
/// chars (matching single chat's historical behavior). Pure + host-testable.
fn fallback_topic(message: &str) -> String {
    let msg = message.trim();
    let truncated: String = msg.chars().take(20).collect();
    if msg.chars().count() > 20 {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Generate a concise topic title from the first user message, via the given
/// provider. Falls back to a truncated message when the LLM errors or returns
/// empty. Never fails — always returns a non-panicking String.
pub async fn generate_conversation_topic(
    provider: &Arc<dyn crate::providers::AiProvider>,
    message: &str,
) -> String {
    use crate::providers::adapter::RequestPayload;
    use crate::providers::message::UnifiedMessage;

    let prompt = format!(
        "Generate a concise topic title (5-10 characters, same language as the message) \
         for a conversation that starts with: {message}"
    );
    let messages = vec![UnifiedMessage::user(&prompt)];
    let payload = RequestPayload {
        messages: &messages,
        system_prompt: Some("You are a title generator. Output ONLY the title, nothing else."),
        system_blocks: None,
        tools: None,
        think_level: None,
        temperature: Some(0.3),
        max_tokens: None,
        tool_choice: None,
        model: None,
        metadata: None,
    };

    match provider.process(payload).await {
        Ok(resp) => {
            let text = resp.text_content().trim().to_string();
            if text.is_empty() {
                fallback_topic(message)
            } else {
                text
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "topic generation: LLM call failed, using fallback");
            fallback_topic(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fallback_topic;

    #[test]
    fn fallback_truncates_long_messages_with_ellipsis() {
        let long = "a".repeat(30);
        let out = fallback_topic(&long);
        assert_eq!(out.chars().count(), 21); // 20 chars + '…'
        assert!(out.ends_with('…'));
    }

    #[test]
    fn fallback_keeps_short_messages_verbatim() {
        assert_eq!(fallback_topic("  hi there  "), "hi there");
    }
}
