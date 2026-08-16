//! Trace event presentation / formatting for all frontends.
//!
//! Provides a single source of truth for rendering `AgentTraceEvent` into
//! human-readable text. CLI, TUI, and web panel all consume these functions
//! instead of duplicating formatting logic.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::{
    AgentTraceEvent, AgentTraceSessionOutcome, AgentTraceState, AgentTraceTextKind,
    AgentTraceToolResult, AgentTraceTurnOutcome,
};

// ---------------------------------------------------------------------------
// Preset
// ---------------------------------------------------------------------------

/// Predefined presentation configurations for each frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTracePresentationPreset {
    /// Compact single-line output for CLI.
    CliCompact,
    /// Verbose debug output for TUI panels.
    TuiDebug,
    /// Full-width output for the web panel.
    PanelTrace,
}

impl AgentTracePresentationPreset {
    /// Return the presentation options associated with this preset.
    #[must_use]
    pub const fn options(&self) -> AgentTracePresentationOptions {
        match self {
            Self::CliCompact => AgentTracePresentationOptions {
                content_limit: 120,
                tool_input_limit: 80,
                tool_output_limit: 200,
            },
            Self::TuiDebug => AgentTracePresentationOptions {
                content_limit: 200,
                tool_input_limit: 120,
                tool_output_limit: 300,
            },
            Self::PanelTrace => AgentTracePresentationOptions {
                content_limit: 500,
                tool_input_limit: 300,
                tool_output_limit: 500,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Truncation limits that control how much text is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentTracePresentationOptions {
    /// Maximum characters for general content fields.
    pub content_limit: usize,
    /// Maximum characters for serialized tool input.
    pub tool_input_limit: usize,
    /// Maximum characters for tool output / result text.
    pub tool_output_limit: usize,
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

/// Human-readable labels used when formatting trace events.
#[derive(Debug, Clone)]
pub struct AgentTracePresentationLabels {
    pub turn_started: String,
    pub state_prepare: String,
    pub state_think: String,
    pub state_resolve: String,
    pub state_act: String,
    pub state_finalize: String,
    pub text_intermediate: String,
    pub text_final: String,
    pub tool_call_started: String,
    pub tool_call_completed: String,
    pub tool_summary: String,
    pub turn_completed: String,
    pub session_completed: String,
    pub outcome_continue: String,
    pub outcome_stop: String,
    pub outcome_hit_limit: String,
    pub outcome_cancelled: String,
    pub session_completed_ok: String,
    pub session_hit_limit: String,
    pub session_cancelled: String,
}

impl AgentTracePresentationLabels {
    /// Default English labels.
    #[must_use]
    pub fn english() -> Self {
        Self {
            turn_started: "Turn started".into(),
            state_prepare: "Preparing".into(),
            state_think: "Thinking".into(),
            state_resolve: "Resolving".into(),
            state_act: "Acting".into(),
            state_finalize: "Finalizing".into(),
            text_intermediate: "Intermediate text".into(),
            text_final: "Final text".into(),
            tool_call_started: "Tool call".into(),
            tool_call_completed: "Tool done".into(),
            tool_summary: "Tool summary".into(),
            turn_completed: "Turn completed".into(),
            session_completed: "Session completed".into(),
            outcome_continue: "continue".into(),
            outcome_stop: "stop".into(),
            outcome_hit_limit: "hit limit".into(),
            outcome_cancelled: "cancelled".into(),
            session_completed_ok: "completed".into(),
            session_hit_limit: "hit limit".into(),
            session_cancelled: "cancelled".into(),
        }
    }

    fn state_label(&self, state: &AgentTraceState) -> &str {
        match state {
            AgentTraceState::Prepare => &self.state_prepare,
            AgentTraceState::Think => &self.state_think,
            AgentTraceState::Resolve => &self.state_resolve,
            AgentTraceState::Act => &self.state_act,
            AgentTraceState::Finalize => &self.state_finalize,
        }
    }

    fn turn_outcome_label(&self, outcome: &AgentTraceTurnOutcome) -> &str {
        match outcome {
            AgentTraceTurnOutcome::Continue => &self.outcome_continue,
            AgentTraceTurnOutcome::Stop => &self.outcome_stop,
            AgentTraceTurnOutcome::HitLimit => &self.outcome_hit_limit,
            AgentTraceTurnOutcome::Cancelled => &self.outcome_cancelled,
        }
    }

    fn session_outcome_label(&self, outcome: &AgentTraceSessionOutcome) -> &str {
        match outcome {
            AgentTraceSessionOutcome::Completed => &self.session_completed_ok,
            AgentTraceSessionOutcome::HitLimit => &self.session_hit_limit,
            AgentTraceSessionOutcome::Cancelled => &self.session_cancelled,
        }
    }
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// Status indicator for a presentation item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTracePresentationStatus {
    InProgress,
    Success,
    Failed,
    Info,
    /// Recoverable-but-noteworthy condition (e.g. cache health degraded).
    Warning,
}

/// Formatted representation of a single trace event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTracePresentation {
    /// Event kind identifier (mirrors `AgentTraceEvent::kind()`).
    pub kind: String,
    /// Status category.
    pub status: AgentTracePresentationStatus,
    /// Human-readable content string.
    pub content: String,
    /// Optional duration in milliseconds (for tool calls).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// Public API – convenience wrappers
// ---------------------------------------------------------------------------

/// Format a trace event using a preset with default English labels.
#[must_use]
pub fn present_agent_trace_event_with_preset(
    event: &AgentTraceEvent,
    preset: AgentTracePresentationPreset,
) -> Option<AgentTracePresentation> {
    let labels = AgentTracePresentationLabels::english();
    let options = preset.options();
    present_agent_trace_event(event, &options, &labels)
}

/// Format a trace event using custom labels and a preset.
#[must_use]
pub fn present_agent_trace_event_with_labels_and_preset(
    event: &AgentTraceEvent,
    labels: &AgentTracePresentationLabels,
    preset: AgentTracePresentationPreset,
) -> Option<AgentTracePresentation> {
    let options = preset.options();
    present_agent_trace_event(event, &options, labels)
}

// ---------------------------------------------------------------------------
// Core presentation logic
// ---------------------------------------------------------------------------

/// Format a trace event into a [`AgentTracePresentation`].
///
/// Returns `None` for events that have no meaningful visual representation
/// (currently all variants produce output, but callers should handle `None`).
#[must_use]
pub fn present_agent_trace_event(
    event: &AgentTraceEvent,
    options: &AgentTracePresentationOptions,
    labels: &AgentTracePresentationLabels,
) -> Option<AgentTracePresentation> {
    match event {
        AgentTraceEvent::TurnStarted { iteration } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::Info,
            content: format!("{} (iteration {})", labels.turn_started, iteration),
            duration_ms: None,
        }),

        AgentTraceEvent::TurnStateEntered { state, iteration } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::InProgress,
            content: format!("{} (iteration {})", labels.state_label(state), iteration),
            duration_ms: None,
        }),

        AgentTraceEvent::TextEmitted {
            text,
            stream,
            iteration,
        } => {
            let label = match stream {
                AgentTraceTextKind::Intermediate => &labels.text_intermediate,
                AgentTraceTextKind::Final => &labels.text_final,
            };
            let status = match stream {
                AgentTraceTextKind::Intermediate => AgentTracePresentationStatus::InProgress,
                AgentTraceTextKind::Final => AgentTracePresentationStatus::Success,
            };
            Some(AgentTracePresentation {
                kind: event.kind().into(),
                status,
                content: format!(
                    "[{}] iter {}: {}",
                    label,
                    iteration,
                    truncate(text, options.content_limit)
                ),
                duration_ms: None,
            })
        }

        AgentTraceEvent::ToolCallStarted { call, .. } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::InProgress,
            content: format!(
                "{}: {} ({})",
                labels.tool_call_started,
                call.tool_name,
                summarize_tool_input(&call.input, *options)
            ),
            duration_ms: None,
        }),

        AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
            let status = if result.is_success() {
                AgentTracePresentationStatus::Success
            } else {
                AgentTracePresentationStatus::Failed
            };
            Some(AgentTracePresentation {
                kind: event.kind().into(),
                status,
                content: format!(
                    "{}: {} — {}",
                    labels.tool_call_completed,
                    call.tool_name,
                    summarize_tool_result(result, options.tool_output_limit)
                ),
                duration_ms: Some(call.duration_ms),
            })
        }

        AgentTraceEvent::ToolSummary { summary, .. } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::Info,
            content: format!(
                "{}: {}",
                labels.tool_summary,
                truncate(summary, options.content_limit)
            ),
            duration_ms: None,
        }),

        AgentTraceEvent::TurnCompleted {
            outcome, metrics, ..
        } => {
            let status = match outcome {
                AgentTraceTurnOutcome::Continue => AgentTracePresentationStatus::Success,
                AgentTraceTurnOutcome::Stop => AgentTracePresentationStatus::Success,
                AgentTraceTurnOutcome::HitLimit => AgentTracePresentationStatus::Failed,
                AgentTraceTurnOutcome::Cancelled => AgentTracePresentationStatus::Info,
            };
            Some(AgentTracePresentation {
                kind: event.kind().into(),
                status,
                content: format!(
                    "{} ({}) — tools: {}/{}, tokens: {}",
                    labels.turn_completed,
                    labels.turn_outcome_label(outcome),
                    metrics.executed_tool_calls,
                    metrics.requested_tool_calls,
                    metrics.total_tokens,
                ),
                duration_ms: None,
            })
        }

        AgentTraceEvent::SessionCompleted {
            outcome,
            iterations,
            tool_calls_made,
            total_tokens,
            final_text,
            ..
        } => {
            let status = match outcome {
                AgentTraceSessionOutcome::Completed => AgentTracePresentationStatus::Success,
                AgentTraceSessionOutcome::HitLimit => AgentTracePresentationStatus::Failed,
                AgentTraceSessionOutcome::Cancelled => AgentTracePresentationStatus::Info,
            };
            let tail = final_text
                .as_deref()
                .map(|t| format!(" — {}", truncate(t, options.content_limit)))
                .unwrap_or_default();
            Some(AgentTracePresentation {
                kind: event.kind().into(),
                status,
                content: format!(
                    "{} ({}) — iterations: {}, tools: {}, tokens: {}{}",
                    labels.session_completed,
                    labels.session_outcome_label(outcome),
                    iterations,
                    tool_calls_made,
                    total_tokens,
                    tail,
                ),
                duration_ms: None,
            })
        }

        AgentTraceEvent::WorktreeCreated { path } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::Info,
            content: format!("worktree created: {}", path.display()),
            duration_ms: None,
        }),

        AgentTraceEvent::WorktreeCleanedUp { path, leaked } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: if *leaked {
                AgentTracePresentationStatus::Failed
            } else {
                AgentTracePresentationStatus::Info
            },
            content: format!(
                "worktree cleaned up{}: {}",
                if *leaked { " (leaked)" } else { "" },
                path.display()
            ),
            duration_ms: None,
        }),

        AgentTraceEvent::McpScopeAttached {
            agent_id,
            references,
            inline_count,
        } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::Info,
            content: format!(
                "mcp scope attached: agent={agent_id} references={} inline={inline_count}",
                references.len()
            ),
            duration_ms: None,
        }),

        AgentTraceEvent::McpScopeCleaned { agent_id, leaked } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: if *leaked {
                AgentTracePresentationStatus::Failed
            } else {
                AgentTracePresentationStatus::Info
            },
            content: format!(
                "mcp scope cleaned{}: agent={agent_id}",
                if *leaked { " (leaked)" } else { "" }
            ),
            duration_ms: None,
        }),

        AgentTraceEvent::ProviderUsage { .. } => {
            /* observability passthrough — no presentation */
            None
        }

        AgentTraceEvent::ReactiveCompactionAttempted {
            token_gap,
            succeeded,
        } => {
            let status = if *succeeded {
                AgentTracePresentationStatus::Success
            } else {
                AgentTracePresentationStatus::Failed
            };
            let gap = token_gap
                .map(|g| format!(" (gap={g}t)"))
                .unwrap_or_default();
            Some(AgentTracePresentation {
                kind: event.kind().into(),
                status,
                content: format!(
                    "reactive compaction {}{}",
                    if *succeeded { "rescued" } else { "exhausted" },
                    gap,
                ),
                duration_ms: None,
            })
        }

        AgentTraceEvent::VerifierVeto { iteration, reason } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::Info,
            content: format!(
                "goal-loop veto (iteration {iteration}): checklist incomplete — {}",
                truncate(reason, options.content_limit)
            ),
            duration_ms: None,
        }),

        AgentTraceEvent::MoaAdvisor {
            index,
            count,
            label,
            text,
            error,
        } => Some(if *count == 0 {
            // Activation-failure form (runner Step 3-MoA build failure).
            AgentTracePresentation {
                kind: event.kind().into(),
                status: AgentTracePresentationStatus::Failed,
                content: format!(
                    "MoA not activated — {}",
                    truncate(error.as_deref().unwrap_or("unknown"), options.content_limit)
                ),
                duration_ms: None,
            }
        } else if let Some(err) = error {
            AgentTracePresentation {
                kind: event.kind().into(),
                status: AgentTracePresentationStatus::Failed,
                content: format!(
                    "Advisor {index}/{count} — {label} [failed: {}]",
                    truncate(err, options.content_limit)
                ),
                duration_ms: None,
            }
        } else {
            // Audit TO1: render the advisor's advice (not just the label) so
            // native TUI / CLI / Panel trace-tree — all of which consume this
            // shared formatter — reach parity with the webchat inline renderer,
            // which reads the raw `text` field directly. Previously the `..`
            // dropped `text`, leaving the advisor's whole point invisible on
            // those surfaces. Mirrors the adjacent MoaAdvisorSpend cost_usd fix.
            let advice = text.trim();
            AgentTracePresentation {
                kind: event.kind().into(),
                status: AgentTracePresentationStatus::Info,
                content: if advice.is_empty() {
                    format!("Advisor {index}/{count} — {label}")
                } else {
                    format!(
                        "Advisor {index}/{count} — {label}\n{}",
                        truncate(advice, options.content_limit)
                    )
                },
                duration_ms: None,
            }
        }),

        AgentTraceEvent::MoaAggregating {
            aggregator, cached, ..
        } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::InProgress,
            content: if *cached {
                format!("MoA aggregating ({aggregator}, cached advice)")
            } else {
                format!("MoA aggregating ({aggregator})")
            },
            duration_ms: None,
        }),

        AgentTraceEvent::MoaAdvisorSpend {
            input_tokens,
            output_tokens,
            billed_count,
            advisor_count,
            cost_usd,
        } => {
            // Append the priced cost when known so this shared formatter (native
            // TUI + trace REPLAY) reaches parity with the webchat renderer, which
            // already shows it. The `..` here previously dropped `cost_usd`,
            // leaving the already-computed advisor spend dark on those surfaces.
            let cost = cost_usd.map_or(String::new(), |c| format!(" (~${c:.4})"));
            Some(AgentTracePresentation {
                kind: event.kind().into(),
                status: AgentTracePresentationStatus::Info,
                content: format!(
                    "MoA advisors spent {input_tokens}+{output_tokens} tok ({billed_count}/{advisor_count} billed){cost}"
                ),
                duration_ms: None,
            })
        }

        AgentTraceEvent::MoaTurnTrace { preset, payload } => {
            // Persisted-only on the wire; in REPLAY the one-line summary makes
            // the audit record discoverable (round-2 W3). Full advisor I/O
            // renders panel-side (events.rs "moa_turn_trace" arm).
            let n = payload
                .get("advisors")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len);
            Some(AgentTracePresentation {
                kind: event.kind().into(),
                status: AgentTracePresentationStatus::Info,
                content: format!(
                    "MoA turn trace — preset {preset} ({n} advisor{})",
                    if n == 1 { "" } else { "s" }
                ),
                duration_ms: None,
            })
        }

        AgentTraceEvent::CacheHealthDegraded {
            scope,
            streak,
            reads,
            writes,
            prefix_changed,
        } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::Warning,
            content: format!(
                "prompt cache re-creating rather than read — {streak} consecutive calls \
                 (last: {reads} read / {writes} created) · {scope}{}",
                match prefix_changed {
                    Some(true) => " · stable prefix CHANGED between calls",
                    Some(false) => " · stable prefix unchanged (provider-side eviction?)",
                    None => "",
                }
            ),
            duration_ms: None,
        }),
    }
}

// ---------------------------------------------------------------------------
// Tool summarization helpers
// ---------------------------------------------------------------------------

/// Produce a compact text summary of tool input parameters.
///
/// The second argument is `AgentTracePresentationOptions`; the
/// `tool_input_limit` field controls the maximum output length.
#[must_use]
pub fn summarize_tool_input(params: &Value, options: AgentTracePresentationOptions) -> String {
    summarize_tool_input_raw(params, options.tool_input_limit)
}

/// Truncate raw tool output to `limit` characters.
#[must_use]
pub fn summarize_tool_output(output: &str, limit: usize) -> String {
    truncate(output, limit)
}

/// Summarize a `AgentTraceToolResult` into a short string.
#[must_use]
pub fn summarize_tool_result(result: &AgentTraceToolResult, limit: usize) -> String {
    match result {
        AgentTraceToolResult::Success { output } => {
            let text = compact_json(output);
            truncate(&text, limit)
        }
        AgentTraceToolResult::Error { error, .. } => {
            format!("ERROR: {}", truncate(error, limit.saturating_sub(7)))
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Summarize tool input from a JSON value into a compact string.
fn summarize_tool_input_raw(params: &Value, limit: usize) -> String {
    match params {
        Value::Object(map) if map.is_empty() => "{}".into(),
        Value::Object(map) => {
            // Show key=value pairs, compact
            let pairs: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}={}", k, compact_value(v)))
                .collect();
            let joined = pairs.join(", ");
            truncate(&joined, limit)
        }
        Value::Null => "null".into(),
        other => truncate(&compact_json(other), limit),
    }
}

/// Compact a JSON value to a short inline representation.
fn compact_value(v: &Value) -> String {
    match v {
        Value::String(s) => {
            if s.chars().count() > 60 {
                let boundary = s.char_indices().nth(57).map_or(s.len(), |(i, _)| i);
                format!("\"{}...\"", &s[..boundary])
            } else {
                format!("\"{s}\"")
            }
        }
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        Value::Array(arr) => format!("[..{}]", arr.len()),
        Value::Object(map) => format!("{{..{}}}", map.len()),
    }
}

/// Serialize a value to compact JSON (single line).
fn compact_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"))
}

/// Truncate a string to at most `limit` characters, appending "..." if cut.
fn truncate(s: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    let char_count = s.chars().count();
    if char_count <= limit {
        return s.to_string();
    }
    // For limits smaller than the 3-char ellipsis, appending "..." would itself
    // exceed `limit`; return a plain character prefix so the contract holds.
    if limit <= 3 {
        return s.chars().take(limit).collect();
    }
    let boundary = s.char_indices().nth(limit - 3).map_or(s.len(), |(i, _)| i);
    format!("{}...", &s[..boundary])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        AgentTraceEvent, AgentTraceSessionOutcome, AgentTraceState, AgentTraceToolCallEnd,
        AgentTraceToolCallStart, AgentTraceToolResult, AgentTraceTurnMetrics,
        AgentTraceTurnOutcome,
    };
    use serde_json::json;

    // -- Preset options ------------------------------------------------------

    #[test]
    fn cli_compact_preset_values() {
        let opts = AgentTracePresentationPreset::CliCompact.options();
        assert_eq!(opts.content_limit, 120);
        assert_eq!(opts.tool_input_limit, 80);
        assert_eq!(opts.tool_output_limit, 200);
    }

    #[test]
    fn tui_debug_preset_values() {
        let opts = AgentTracePresentationPreset::TuiDebug.options();
        assert_eq!(opts.content_limit, 200);
        assert_eq!(opts.tool_input_limit, 120);
        assert_eq!(opts.tool_output_limit, 300);
    }

    #[test]
    fn panel_trace_preset_values() {
        let opts = AgentTracePresentationPreset::PanelTrace.options();
        assert_eq!(opts.content_limit, 500);
        assert_eq!(opts.tool_input_limit, 300);
        assert_eq!(opts.tool_output_limit, 500);
    }

    // -- summarize_tool_input ------------------------------------------------

    #[test]
    fn summarize_empty_object() {
        let opts = AgentTracePresentationPreset::TuiDebug.options();
        assert_eq!(summarize_tool_input(&json!({}), opts), "{}");
    }

    #[test]
    fn summarize_flat_object() {
        let opts = AgentTracePresentationPreset::TuiDebug.options();
        let params = json!({"path": "/tmp/foo", "recursive": true});
        let result = summarize_tool_input(&params, opts);
        assert!(result.contains("path="));
        assert!(result.contains("recursive="));
    }

    #[test]
    fn summarize_null_input() {
        let opts = AgentTracePresentationPreset::CliCompact.options();
        assert_eq!(summarize_tool_input(&Value::Null, opts), "null");
    }

    #[test]
    fn summarize_truncates_long_input() {
        let opts = AgentTracePresentationOptions {
            content_limit: 50,
            tool_input_limit: 20,
            tool_output_limit: 50,
        };
        let params = json!({"very_long_key": "a]very long value that should be truncated"});
        let result = summarize_tool_input(&params, opts);
        assert!(result.len() <= 20);
    }

    // -- summarize_tool_result -----------------------------------------------

    #[test]
    fn summarize_success_result() {
        let result = AgentTraceToolResult::Success {
            output: json!("ok"),
        };
        let s = summarize_tool_result(&result, 100);
        assert!(s.contains("ok"));
    }

    #[test]
    fn summarize_error_result() {
        let result = AgentTraceToolResult::Error {
            error: "timeout".into(),
            retryable: true,
        };
        let s = summarize_tool_result(&result, 100);
        assert!(s.starts_with("ERROR:"));
        assert!(s.contains("timeout"));
    }

    // -- present_agent_trace_event -------------------------------------------

    #[test]
    fn present_turn_started() {
        let event = AgentTraceEvent::TurnStarted { iteration: 1 };
        let p =
            present_agent_trace_event_with_preset(&event, AgentTracePresentationPreset::TuiDebug)
                .unwrap();
        assert_eq!(p.kind, "turn_started");
        assert_eq!(p.status, AgentTracePresentationStatus::Info);
        assert!(p.content.contains("iteration 1"));
    }

    #[test]
    fn present_state_entered() {
        let event = AgentTraceEvent::TurnStateEntered {
            iteration: 2,
            state: AgentTraceState::Think,
        };
        let p =
            present_agent_trace_event_with_preset(&event, AgentTracePresentationPreset::CliCompact)
                .unwrap();
        assert_eq!(p.status, AgentTracePresentationStatus::InProgress);
        assert!(p.content.contains("Thinking"));
    }

    #[test]
    fn present_tool_call_started() {
        let event = AgentTraceEvent::ToolCallStarted {
            iteration: 1,
            call: AgentTraceToolCallStart {
                tool_id: "t1".into(),
                tool_name: "read_file".into(),
                input: json!({"path": "/tmp/x"}),
            },
        };
        let p =
            present_agent_trace_event_with_preset(&event, AgentTracePresentationPreset::TuiDebug)
                .unwrap();
        assert!(p.content.contains("read_file"));
        assert_eq!(p.status, AgentTracePresentationStatus::InProgress);
    }

    #[test]
    fn present_tool_call_completed_success() {
        let event = AgentTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: AgentTraceToolCallEnd {
                tool_id: "t1".into(),
                tool_name: "read_file".into(),
                input: json!({}),
                duration_ms: 42,
            },
            result: AgentTraceToolResult::Success {
                output: json!("file contents"),
            },
        };
        let p =
            present_agent_trace_event_with_preset(&event, AgentTracePresentationPreset::TuiDebug)
                .unwrap();
        assert_eq!(p.status, AgentTracePresentationStatus::Success);
        assert_eq!(p.duration_ms, Some(42));
    }

    #[test]
    fn present_tool_call_completed_error() {
        let event = AgentTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: AgentTraceToolCallEnd {
                tool_id: "t1".into(),
                tool_name: "run".into(),
                input: json!({}),
                duration_ms: 5,
            },
            result: AgentTraceToolResult::Error {
                error: "not found".into(),
                retryable: false,
            },
        };
        let p =
            present_agent_trace_event_with_preset(&event, AgentTracePresentationPreset::CliCompact)
                .unwrap();
        assert_eq!(p.status, AgentTracePresentationStatus::Failed);
    }

    #[test]
    fn present_session_completed() {
        let event = AgentTraceEvent::SessionCompleted {
            outcome: AgentTraceSessionOutcome::Completed,
            iterations: 3,
            tool_calls_made: 10,
            total_tokens: 5000,
            hit_limit: false,
            final_text: Some("Done.".into()),
            terminate_reason: None,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        };
        let p =
            present_agent_trace_event_with_preset(&event, AgentTracePresentationPreset::PanelTrace)
                .unwrap();
        assert_eq!(p.status, AgentTracePresentationStatus::Success);
        assert!(p.content.contains("iterations: 3"));
        assert!(p.content.contains("Done."));
    }

    #[test]
    fn present_turn_completed() {
        let event = AgentTraceEvent::TurnCompleted {
            iteration: 2,
            outcome: AgentTraceTurnOutcome::Continue,
            metrics: AgentTraceTurnMetrics {
                requested_tool_calls: 3,
                executed_tool_calls: 2,
                productive: true,
                total_tokens: 1200,
            },
        };
        let p =
            present_agent_trace_event_with_preset(&event, AgentTracePresentationPreset::TuiDebug)
                .unwrap();
        assert_eq!(p.status, AgentTracePresentationStatus::Success);
        assert!(p.content.contains("tools: 2/3"));
    }

    #[test]
    fn with_labels_and_preset_uses_custom_labels() {
        let mut labels = AgentTracePresentationLabels::english();
        labels.turn_started = "Runde gestartet".into();
        let event = AgentTraceEvent::TurnStarted { iteration: 1 };
        let p = present_agent_trace_event_with_labels_and_preset(
            &event,
            &labels,
            AgentTracePresentationPreset::CliCompact,
        )
        .unwrap();
        assert!(p.content.contains("Runde gestartet"));
    }

    // -- MoA presentation (round-2 W3a) --------------------------------------

    #[test]
    fn moa_presentation_reflects_error_cached_and_turn_trace() {
        let opts = AgentTracePresentationPreset::PanelTrace.options();
        let labels = AgentTracePresentationLabels::english();

        let failed = AgentTraceEvent::MoaAdvisor {
            index: 2,
            count: 3,
            label: "p:m".into(),
            text: String::new(),
            error: Some("timeout after 120s".into()),
        };
        let p = present_agent_trace_event(&failed, &opts, &labels).unwrap();
        assert!(matches!(p.status, AgentTracePresentationStatus::Failed));
        assert!(p.content.contains("failed"));

        let not_activated = AgentTraceEvent::MoaAdvisor {
            index: 0,
            count: 0,
            label: "moa:deep".into(),
            text: String::new(),
            error: Some("preset not found".into()),
        };
        let p = present_agent_trace_event(&not_activated, &opts, &labels).unwrap();
        assert!(p.content.contains("not activated"));

        // Audit TO1: a SUCCESSFUL advisor renders its advice text on the shared
        // formatter (TUI / CLI / Panel-tree parity), not just the bare label.
        let ok_advisor = AgentTraceEvent::MoaAdvisor {
            index: 1,
            count: 3,
            label: "openai:gpt-5".into(),
            text: "use a binary search here".into(),
            error: None,
        };
        let p = present_agent_trace_event(&ok_advisor, &opts, &labels).unwrap();
        assert!(matches!(p.status, AgentTracePresentationStatus::Info));
        assert!(p.content.contains("openai:gpt-5"), "{}", p.content);
        assert!(
            p.content.contains("use a binary search here"),
            "{}",
            p.content
        );

        let cached = AgentTraceEvent::MoaAggregating {
            aggregator: "p:m".into(),
            advisor_count: 2,
            cached: true,
        };
        let p = present_agent_trace_event(&cached, &opts, &labels).unwrap();
        assert!(p.content.contains("cached"));

        let spend = AgentTraceEvent::MoaAdvisorSpend {
            advisor_count: 3,
            billed_count: 2,
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: Some(0.1234),
        };
        let p = present_agent_trace_event(&spend, &opts, &labels).unwrap();
        assert!(p.content.contains("2/3 billed"));
        // Priced cost reaches the shared formatter (TUI/REPLAY parity, not just webchat).
        assert!(p.content.contains("$0.1234"));

        let trace = AgentTraceEvent::MoaTurnTrace {
            preset: "deep".into(),
            payload: json!({ "advisors": [{"label": "a", "output": "x"}] }),
        };
        let p = present_agent_trace_event(&trace, &opts, &labels).unwrap();
        assert!(p.content.contains("deep"));
        assert!(p.content.contains("1 advisor"));
    }
}
