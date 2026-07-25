//! `memory_timeline` tool — view the complete lifecycle of a memory fact.
//!
//! Wraps [`MemoryTimeTraveler::explain_fact`] to provide a human-readable
//! timeline of creation, modification, decay, and invalidation events.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::error::ToolError;
use crate::error::Result;
use crate::memory::events::traveler::MemoryTimeTraveler;
use crate::memory::explain::FactExplanation;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// ── Args / Output ───────────────────────────────────────────────────────────

/// Arguments for the `memory_timeline` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryTimelineArgs {
    /// The fact ID to inspect
    pub fact_id: String,
}

/// Output from the `memory_timeline` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryTimelineOutput {
    /// The full lifecycle explanation of the fact
    pub explanation: FactExplanation,
}

// ── Tool struct ─────────────────────────────────────────────────────────────

/// View the complete lifecycle of a memory fact
pub struct MemoryTimelineTool {
    traveler: Arc<MemoryTimeTraveler>,
}

impl MemoryTimelineTool {
    #[must_use]
    pub const fn new(traveler: Arc<MemoryTimeTraveler>) -> Self {
        Self { traveler }
    }

    /// Internal implementation
    async fn call_impl(
        &self,
        args: MemoryTimelineArgs,
    ) -> std::result::Result<MemoryTimelineOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        let args_summary = format!("fact timeline: {}", &args.fact_id);
        notify_tool_start(Self::NAME, &args_summary);

        let explanation = self
            .traveler
            .explain_fact(&args.fact_id)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to explain fact: {e}")))?;

        notify_tool_result(
            Self::NAME,
            &format!("fact_id={}, valid={}", args.fact_id, explanation.is_valid),
            true,
        );

        Ok(MemoryTimelineOutput { explanation })
    }
}

impl Clone for MemoryTimelineTool {
    fn clone(&self) -> Self {
        Self {
            traveler: self.traveler.clone(),
        }
    }
}

// ── AlephTool impl ──────────────────────────────────────────────────────────

#[async_trait]
impl AlephTool for MemoryTimelineTool {
    const NAME: &'static str = "memory_timeline";
    const DESCRIPTION: &'static str =
        "View the complete lifecycle of a memory fact — creation, modification, \
         decay, invalidation timeline. Use when you need to understand why a \
         fact changed or was invalidated.";

    type Args = MemoryTimelineArgs;
    type Output = MemoryTimelineOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec!["memory_timeline(fact_id='abc-123-def')".to_string()])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_deserialization() {
        let json = r#"{"fact_id": "abc-123"}"#;
        let args: MemoryTimelineArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.fact_id, "abc-123");
    }
}
