//! History building and memory persistence helpers.

use crate::gateway::agent_instance::{AgentInstance, MessageRole};
use crate::gateway::router::SessionKey;

/// Build loop history from the agent's session, excluding the current user input.
pub(super) async fn build_loop_history(
    agent: &AgentInstance,
    session_key: &SessionKey,
    current_input: &str,
) -> Vec<crate::providers::message::UnifiedMessage> {
    use crate::providers::message::UnifiedMessage;

    let session_history = agent.get_history(session_key, Some(50)).await;
    let mut msgs: Vec<UnifiedMessage> = Vec::new();

    // Skip the last message if it's the current user input we just stored
    let history_slice = if session_history
        .last()
        .is_some_and(|m| m.role == MessageRole::User && m.content == current_input)
    {
        // safe: last() returned Some, so len() >= 1
        &session_history[..session_history.len().saturating_sub(1)]
    } else {
        &session_history
    };

    for msg in history_slice {
        match msg.role {
            MessageRole::User => msgs.push(UnifiedMessage::user(msg.content.clone())),
            MessageRole::Assistant => msgs.push(UnifiedMessage::assistant(msg.content.clone())),
            _ => {}
        }
    }
    msgs
}
