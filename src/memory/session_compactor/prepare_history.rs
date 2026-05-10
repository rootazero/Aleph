use super::{SessionCompactor, session_message_to_unified, extract_depth};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::router::SessionKey;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::providers::message::UnifiedMessage;
use crate::sync_primitives::Ordering;
use tracing::{debug, warn};

impl SessionCompactor {
    /// Assemble compressed history for a new agent loop turn.
    pub async fn prepare_history(
        &self,
        agent: &AgentInstance,
        session_key: &SessionKey,
        _current_input: &str,
        token_budget: u64,
    ) -> Vec<UnifiedMessage> {
        self.metrics
            .prepare_history_calls
            .fetch_add(1, Ordering::Relaxed);
        tracing::info!(target: "session_compactor", "prepare");

        if !self.config.enabled {
            let raw = agent.get_history(session_key, None).await;
            return raw
                .into_iter()
                .map(|m| session_message_to_unified(&m))
                .collect();
        }

        let raw_messages = agent.get_history(session_key, None).await;
        if raw_messages.len() <= self.config.fresh_tail_count {
            return raw_messages
                .iter()
                .map(session_message_to_unified)
                .collect();
        }

        let session_id = session_key.to_key_string();
        let agent_id = agent.id().to_string();
        let path_prefix = format!("aleph://session/{}/", session_id);

        let mut summaries = match self
            .database
            .get_raw_by_path_prefix(&path_prefix, &agent_id, 200)
            .await
        {
            Ok(raws) => raws,
            Err(e) => {
                warn!(
                    error = %e,
                    session = %session_id,
                    "Failed to fetch session summaries, falling back to raw history"
                );
                let raw = agent.get_history(session_key, None).await;
                return raw
                    .into_iter()
                    .map(|m| session_message_to_unified(&m))
                    .collect();
            }
        };

        summaries.sort_by(|a, b| {
            let pa = a.path.as_deref().unwrap_or("");
            let pb = b.path.as_deref().unwrap_or("");
            let da = extract_depth(pa);
            let db = extract_depth(pb);
            db.cmp(&da).then_with(|| pa.cmp(pb))
        });

        let tail_start = if raw_messages.len() > self.config.fresh_tail_count {
            raw_messages.len() - self.config.fresh_tail_count
        } else {
            0
        };
        let fresh_tail: Vec<UnifiedMessage> = raw_messages[tail_start..]
            .iter()
            .map(session_message_to_unified)
            .collect();

        let mut result: Vec<UnifiedMessage> = Vec::new();
        let ratio = self.config.token_estimate_ratio;
        let budget = token_budget as usize;
        let mut used_tokens: usize = 0;

        let tail_tokens: usize = fresh_tail
            .iter()
            .map(|m| super::context_window::estimate_tokens(&m.text_content(), ratio))
            .sum();

        let summary_budget = budget.saturating_sub(tail_tokens);

        for fact in &summaries {
            let path = fact.path.as_deref().unwrap_or("");
            let depth = extract_depth(path);
            let summary_tokens = super::context_window::estimate_tokens(&fact.content, ratio);

            if used_tokens + summary_tokens > summary_budget {
                debug!(
                    session = %session_id,
                    used_tokens,
                    summary_budget,
                    "Summary token budget exhausted, evicting remaining summaries"
                );
                break;
            }

            let xml_content = format!(
                "<session_context depth=\"d{depth}\">\n{}\n</session_context>",
                fact.content
            );
            result.push(UnifiedMessage::user(xml_content));
            used_tokens += summary_tokens;
        }

        result.extend(fresh_tail);

        debug!(
            session = %session_id,
            summary_count = summaries.len(),
            injected = result.len().saturating_sub(raw_messages[tail_start..].len()),
            tail_count = raw_messages[tail_start..].len(),
            total_tokens_est = used_tokens + tail_tokens,
            "Prepared session history"
        );

        result
    }
}
