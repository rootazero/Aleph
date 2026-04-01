//! ContextDiagnostics — token breakdown and observability.

use super::pipeline::PipelineResult;
use super::pressure::estimate_tokens_smart;
use crate::providers::message::UnifiedMessage;
use std::collections::HashMap;

// =============================================================================
// DiagnosticsSnapshot
// =============================================================================

/// Token breakdown for a single analysis pass over a message list.
#[derive(Debug, Clone)]
pub struct DiagnosticsSnapshot {
    pub user_tokens: usize,
    pub assistant_tokens: usize,
    pub system_tokens: usize,
    pub tool_result_tokens: HashMap<String, usize>,
    pub tool_result_counts: HashMap<String, usize>,
    pub total_tokens: usize,
}

impl DiagnosticsSnapshot {
    /// Produce a compact human-readable summary line.
    ///
    /// Example:
    /// `context: 72000/100000 (72%) | user=5000 asst=30000 | top: Bash=18000, Read=15000 | dupes: Read×3`
    pub fn format_summary(&self, budget: u64) -> String {
        let budget = budget as usize;
        let pct = if budget == 0 {
            0
        } else {
            (self.total_tokens * 100) / budget
        };

        // Sort tools by token count descending for "top:" section
        let mut tools: Vec<(&str, usize)> = self
            .tool_result_tokens
            .iter()
            .map(|(k, &v)| (k.as_str(), v))
            .collect();
        tools.sort_by(|a, b| b.1.cmp(&a.1));

        let top_str = if tools.is_empty() {
            String::new()
        } else {
            let entries: Vec<String> = tools
                .iter()
                .take(5)
                .map(|(name, tokens)| format!("{name}={tokens}"))
                .collect();
            format!(" | top: {}", entries.join(", "))
        };

        // Dupes: tools called more than once
        let mut dupes: Vec<(&str, usize)> = self
            .tool_result_counts
            .iter()
            .filter(|(_, &count)| count > 1)
            .map(|(k, &v)| (k.as_str(), v))
            .collect();
        dupes.sort_by_key(|(name, _)| *name);

        let dupes_str = if dupes.is_empty() {
            String::new()
        } else {
            let entries: Vec<String> = dupes
                .iter()
                .map(|(name, count)| format!("{name}\u{d7}{count}"))
                .collect();
            format!(" | dupes: {}", entries.join(", "))
        };

        format!(
            "context: {}/{} ({}%) | user={} asst={}{}{}",
            self.total_tokens,
            budget,
            pct,
            self.user_tokens,
            self.assistant_tokens,
            top_str,
            dupes_str,
        )
    }
}

// =============================================================================
// ContextDiagnostics
// =============================================================================

/// Accumulates diagnostics across multiple pipeline runs and circuit breaker events.
#[derive(Debug)]
pub struct ContextDiagnostics {
    pipeline_runs: Vec<PipelineResult>,
    circuit_breaker_trips: usize,
}

impl ContextDiagnostics {
    /// Create a fresh diagnostics accumulator.
    pub fn new() -> Self {
        Self {
            pipeline_runs: Vec::new(),
            circuit_breaker_trips: 0,
        }
    }

    /// Analyze a message list and produce a token breakdown snapshot.
    ///
    /// - User messages starting with "[SYSTEM]" → `system_tokens`
    /// - Other user messages → `user_tokens`
    /// - Assistant messages → `assistant_tokens`
    /// - Tool results → `tool_result_tokens[tool_name]` and `tool_result_counts[tool_name]`
    pub fn analyze(messages: &[UnifiedMessage]) -> DiagnosticsSnapshot {
        let mut user_tokens: usize = 0;
        let mut assistant_tokens: usize = 0;
        let mut system_tokens: usize = 0;
        let mut tool_result_tokens: HashMap<String, usize> = HashMap::new();
        let mut tool_result_counts: HashMap<String, usize> = HashMap::new();

        for msg in messages {
            if msg.is_user() {
                let text = msg.text_content();
                let tokens = estimate_tokens_smart(&text);
                if text.starts_with("[SYSTEM]") {
                    system_tokens += tokens;
                } else {
                    user_tokens += tokens;
                }
            } else if msg.is_assistant() {
                let text = msg.text_content();
                assistant_tokens += estimate_tokens_smart(&text);
            } else if msg.is_tool_result() {
                if let Some((tool_name, content)) = msg.tool_result_info() {
                    let tokens = estimate_tokens_smart(&content);
                    *tool_result_tokens.entry(tool_name.to_string()).or_insert(0) += tokens;
                    *tool_result_counts.entry(tool_name.to_string()).or_insert(0) += 1;
                }
            }
        }

        let total_tokens = user_tokens
            + assistant_tokens
            + system_tokens
            + tool_result_tokens.values().sum::<usize>();

        DiagnosticsSnapshot {
            user_tokens,
            assistant_tokens,
            system_tokens,
            tool_result_tokens,
            tool_result_counts,
            total_tokens,
        }
    }

    /// Record the result of a compaction pipeline run.
    pub fn record_pipeline(&mut self, result: PipelineResult) {
        self.pipeline_runs.push(result);
    }

    /// Record a circuit breaker trip event.
    pub fn record_circuit_breaker_trip(&mut self) {
        self.circuit_breaker_trips += 1;
    }

    /// Return the recorded pipeline run results.
    pub fn pipeline_runs(&self) -> &[PipelineResult] {
        &self.pipeline_runs
    }
}

impl Default for ContextDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    #[test]
    fn analyze_classifies_by_role() {
        let msgs = vec![
            UnifiedMessage::user("hello world"),
            UnifiedMessage::assistant("goodbye world"),
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        assert!(snapshot.user_tokens > 0);
        assert!(snapshot.assistant_tokens > 0);
        assert_eq!(snapshot.tool_result_tokens.len(), 0);
    }

    #[test]
    fn analyze_tracks_tool_results_by_name() {
        let msgs = vec![
            UnifiedMessage::user("search"),
            UnifiedMessage::tool_result("c1", "Bash", "output here", false),
            UnifiedMessage::tool_result("c2", "Read", "file content", false),
            UnifiedMessage::tool_result("c3", "Bash", "more output", false),
            UnifiedMessage::assistant("done"),
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        assert_eq!(snapshot.tool_result_tokens.len(), 2); // Bash and Read
        assert!(snapshot.tool_result_tokens.contains_key("Bash"));
        assert!(snapshot.tool_result_tokens.contains_key("Read"));
    }

    #[test]
    fn analyze_detects_duplicate_tool_calls() {
        let msgs = vec![
            UnifiedMessage::user("read files"),
            UnifiedMessage::tool_result("c1", "Read", "content of src/main.rs", false),
            UnifiedMessage::assistant("ok"),
            UnifiedMessage::user("read again"),
            UnifiedMessage::tool_result("c2", "Read", "content of src/main.rs again", false),
            UnifiedMessage::assistant("ok again"),
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        let read_count = snapshot
            .tool_result_counts
            .get("Read")
            .copied()
            .unwrap_or(0);
        assert_eq!(read_count, 2);
    }

    #[test]
    fn format_summary_produces_readable_output() {
        let msgs = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::tool_result("c1", "Bash", &"x".repeat(500), false),
            UnifiedMessage::assistant("done"),
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        let summary = snapshot.format_summary(10_000);
        assert!(
            summary.contains("Bash"),
            "summary should mention tool names"
        );
    }

    #[test]
    fn analyze_classifies_system_user_messages() {
        let msgs = vec![
            UnifiedMessage::user("[SYSTEM] you are a helpful assistant"),
            UnifiedMessage::user("regular user message"),
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        assert!(
            snapshot.system_tokens > 0,
            "system tokens should be counted"
        );
        assert!(snapshot.user_tokens > 0, "user tokens should be counted");
    }

    #[test]
    fn analyze_total_tokens_is_sum_of_all() {
        let msgs = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::assistant("world"),
            UnifiedMessage::tool_result("c1", "Bash", "output", false),
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        let expected = snapshot.user_tokens
            + snapshot.assistant_tokens
            + snapshot.system_tokens
            + snapshot.tool_result_tokens.values().sum::<usize>();
        assert_eq!(snapshot.total_tokens, expected);
    }

    #[test]
    fn diagnostics_records_pipeline_runs() {
        use super::super::{pipeline::PipelineResult, ContextPressure};
        let mut diag = ContextDiagnostics::new();
        let pressure = ContextPressure {
            used_tokens: 100,
            budget_tokens: 1000,
            ratio: 0.1,
        };
        let result = PipelineResult {
            pressure_before: pressure.clone(),
            pressure_after: pressure,
            tokens_freed: 50,
            stages_run: vec![("stage_a", 50)],
        };
        diag.record_pipeline(result);
        assert_eq!(diag.pipeline_runs().len(), 1);
    }

    #[test]
    fn diagnostics_records_circuit_breaker_trips() {
        let mut diag = ContextDiagnostics::new();
        diag.record_circuit_breaker_trip();
        diag.record_circuit_breaker_trip();
        assert_eq!(diag.circuit_breaker_trips, 2);
    }
}
