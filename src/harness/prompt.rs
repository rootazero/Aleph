//! Prompt Assembly Seam — Stage 3 of the 12-module harness roadmap.
//!
//! `PromptBuilder` is the single seam through which `AgentHarness` produces
//! the per-turn `Vec<UnifiedMessage>` handed to the provider. Default
//! behavior matches the legacy private `build_prompt` byte-for-byte;
//! downstream stages (#11 Subagent, #10 Verification) inject custom
//! builders that compose memory hints, chain context, or judge prompts
//! without patching `agent.rs`.

use async_trait::async_trait;

use crate::providers::message::UnifiedMessage;
use crate::session::events::SessionEventRecord;

/// Input to `PromptBuilder::assemble`. Carries the slice of session events
/// and the tail boundary computed by `tail_start_index`. Future stages may
/// extend this struct with memory hints, skill suggestions, or chain
/// context — additions must be additive (existing builders keep working).
#[derive(Debug)]
pub struct TurnContext<'a> {
    pub events: &'a [SessionEventRecord],
    pub tail_start: usize,
}

impl<'a> TurnContext<'a> {
    pub fn new(events: &'a [SessionEventRecord], tail_start: usize) -> Self {
        Self { events, tail_start }
    }
}

/// Pluggable per-turn message assembler. Implementations must be
/// `Send + Sync` so `Arc<dyn PromptBuilder>` lives in `HarnessDeps`.
#[async_trait]
pub trait PromptBuilder: Send + Sync {
    /// Produce the `Vec<UnifiedMessage>` for the next provider call.
    /// Errors propagate as `HarnessError::Session` (or future variants).
    async fn assemble(
        &self,
        ctx: &TurnContext<'_>,
    ) -> Result<Vec<UnifiedMessage>, crate::harness::trait_def::HarnessError>;
}

/// Default builder — byte-equivalent to the pre-Stage-3 private
/// `build_prompt` function (former `agent.rs:846`).
#[derive(Debug, Default, Clone)]
pub struct DefaultPromptBuilder;

#[async_trait]
impl PromptBuilder for DefaultPromptBuilder {
    async fn assemble(
        &self,
        ctx: &TurnContext<'_>,
    ) -> Result<Vec<UnifiedMessage>, crate::harness::trait_def::HarnessError> {
        use crate::providers::message::{ContentBlock, UnifiedMessage};
        use crate::session::events::SessionEvent;

        let events = ctx.events;
        let tail_start = ctx.tail_start;
        let mut messages = Vec::new();

        // Reconstruct the preceding assistant turn (if any) so the model sees
        // its own tool_use request in context.
        if tail_start > 0 {
            if let SessionEvent::AssistantMessage { content, .. } = &events[tail_start - 1].event {
                let mut blocks: Vec<ContentBlock> = Vec::new();
                if !content.text.is_empty() {
                    blocks.push(ContentBlock::Text {
                        text: content.text.clone(),
                        cache_control: None,
                    });
                }
                for raw in &content.blocks {
                    if let Some(tc) = parse_tool_use_block(raw) {
                        blocks.push(tc);
                    }
                }
                if !blocks.is_empty() {
                    messages.push(UnifiedMessage::Assistant { content: blocks });
                }
            }
        }

        // Walk the tail and emit UserMessage / ToolResult entries.
        for (offset, record) in events[tail_start..].iter().enumerate() {
            match &record.event {
                SessionEvent::UserMessage { content, .. } => {
                    messages.push(UnifiedMessage::user(&content.text));
                }
                SessionEvent::ToolResult { call_id, output, .. } => {
                    let tool_result_idx = tail_start + offset;
                    let tool_name =
                        resolve_tool_name(events, tool_result_idx, call_id).unwrap_or("unknown");
                    messages.push(UnifiedMessage::tool_result_json(
                        call_id.clone(),
                        tool_name.to_string(),
                        output.value.clone(),
                        false,
                    ));
                }
                SessionEvent::ToolError { call_id, error, .. } => {
                    let tool_result_idx = tail_start + offset;
                    let tool_name =
                        resolve_tool_name(events, tool_result_idx, call_id).unwrap_or("unknown");
                    messages.push(UnifiedMessage::ToolResult {
                        tool_call_id: call_id.clone(),
                        tool_name: tool_name.to_string(),
                        content: vec![ContentBlock::Text {
                            text: error.clone(),
                            cache_control: None,
                        }],
                        is_error: true,
                    });
                }
                _ => {}
            }
        }

        Ok(messages)
    }
}

/// Parse a previously persisted `tool_use` JSON block back into a
/// `ContentBlock::ToolCall`. Returns `None` for blocks that don't match
/// the shape written by `tool_use_blocks`.
///
/// `pub(crate)` so the round-trip test in `harness::tests::act` can exercise
/// the writer/reader pair without re-exporting through `harness::agent`.
pub(crate) fn parse_tool_use_block(block: &serde_json::Value) -> Option<crate::providers::message::ContentBlock> {
    use crate::providers::message::ContentBlock;
    let obj = block.as_object()?;
    if obj.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
        return None;
    }
    let id = obj.get("id").and_then(serde_json::Value::as_str)?.to_string();
    let name = obj.get("name").and_then(serde_json::Value::as_str)?.to_string();
    let arguments = obj.get("input").cloned().unwrap_or(serde_json::Value::Null);
    Some(ContentBlock::ToolCall { id, name, arguments })
}

/// Find the `ToolCallRequested.name` whose `call_id` matches, searching
/// strictly BEFORE `before_idx` (i.e. within `events[..before_idx]`).
fn resolve_tool_name<'a>(
    events: &'a [crate::session::events::SessionEventRecord],
    before_idx: usize,
    call_id: &str,
) -> Option<&'a str> {
    use crate::session::events::SessionEvent;
    let upper = before_idx.min(events.len());
    events[..upper].iter().rev().find_map(|r| match &r.event {
        SessionEvent::ToolCallRequested { call_id: id, name, .. } if id == call_id => {
            Some(name.as_str())
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_builder_compiles_and_runs() {
        let events: Vec<SessionEventRecord> = Vec::new();
        let ctx = TurnContext::new(&events, 0);
        let builder = DefaultPromptBuilder;
        let out = builder.assemble(&ctx).await.expect("assemble ok");
        assert!(out.is_empty(), "empty events → empty output");
    }
}
