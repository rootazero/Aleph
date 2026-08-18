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

        // BT-D-R4-05 (partial fix): validate the fact_id format up-front.
        // The full fix — per-agent authorization at the traveler — is
        // tracked separately because the underlying MemoryTimeTraveler
        // API does not yet take an agent id (see sev-wire-2026-08-19-r4
        // builtin_tools-d/REPORT.md). Without per-agent gating, any
        // caller that learns or guesses another corpus's fact id can
        // read its lifecycle and current content; this validation
        // bounds the input surface but does not close that gap.
        let fact_id = args.fact_id.trim();
        if fact_id.is_empty() {
            return Err(ToolError::InvalidArgs(
                "memory_timeline requires a non-empty fact_id".to_string(),
            ));
        }
        if fact_id.len() > 256 {
            return Err(ToolError::InvalidArgs(format!(
                "fact_id is {} bytes; max 256",
                fact_id.len()
            )));
        }
        if fact_id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '/' || c == '\\' || c == '`' || c == '$')
        {
            return Err(ToolError::InvalidArgs(
                "fact_id contains an invalid character (whitespace, control, /, \\, `, or $)"
                    .to_string(),
            ));
        }

        let args_summary = format!("fact timeline: {}", &fact_id);
        notify_tool_start(Self::NAME, &args_summary);

        let explanation = self
            .traveler
            .explain_fact(fact_id)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to explain fact: {e}")))?;

        notify_tool_result(
            Self::NAME,
            &format!("fact_id={}, valid={}", fact_id, explanation.is_valid),
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
