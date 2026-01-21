//! Compression strategies for context management
//!
//! This module defines different strategies for compressing
//! conversation history to manage token usage.

use crate::agent_loop::LoopStep;

/// Key information extracted from steps for preservation
#[derive(Debug, Clone, Default)]
pub struct KeyInfo {
    /// Files that were created or modified
    pub file_changes: Vec<String>,
    /// Important tool outputs (search results, errors, etc.)
    pub important_outputs: Vec<String>,
    /// User decisions made during execution
    pub user_decisions: Vec<String>,
    /// Current state description
    pub current_state: String,
}

/// Strategy for extracting key information from steps
pub struct KeyInfoExtractor;

impl KeyInfoExtractor {
    /// Extract key information from a list of steps
    pub fn extract(steps: &[LoopStep]) -> KeyInfo {
        let mut info = KeyInfo::default();

        for step in steps {
            // Extract file changes from tool calls
            if step.action.action_type().contains("file") {
                let args = step.action.args_summary();
                if args.contains("path") || args.contains("file") {
                    info.file_changes.push(format!(
                        "Step {}: {} - {}",
                        step.step_id,
                        step.action.action_type(),
                        truncate(&args, 100)
                    ));
                }
            }

            // Extract important outputs (errors, search results)
            let result_summary = step.result.summary();
            if result_summary.contains("Error")
                || result_summary.contains("error")
                || result_summary.contains("found")
                || result_summary.contains("result")
            {
                info.important_outputs.push(format!(
                    "Step {}: {}",
                    step.step_id,
                    truncate(&result_summary, 200)
                ));
            }

            // Extract user decisions
            if step.action.action_type() == "ask_user" {
                if let crate::agent_loop::ActionResult::UserResponse { response } = &step.result {
                    info.user_decisions.push(format!(
                        "Step {}: User chose: {}",
                        step.step_id,
                        truncate(response, 100)
                    ));
                }
            }
        }

        // Build current state summary
        if let Some(last_step) = steps.last() {
            info.current_state = format!(
                "After {} steps. Last action: {} -> {}",
                steps.len(),
                last_step.action.action_type(),
                if last_step.result.is_success() {
                    "success"
                } else {
                    "failed"
                }
            );
        }

        info
    }

    /// Format key info for inclusion in summary
    pub fn format(info: &KeyInfo) -> String {
        let mut parts = Vec::new();

        if !info.file_changes.is_empty() {
            parts.push(format!(
                "File changes:\n{}",
                info.file_changes
                    .iter()
                    .map(|s| format!("  - {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        if !info.important_outputs.is_empty() {
            parts.push(format!(
                "Key outputs:\n{}",
                info.important_outputs
                    .iter()
                    .map(|s| format!("  - {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        if !info.user_decisions.is_empty() {
            parts.push(format!(
                "User decisions:\n{}",
                info.user_decisions
                    .iter()
                    .map(|s| format!("  - {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        if !info.current_state.is_empty() {
            parts.push(format!("Current state: {}", info.current_state));
        }

        parts.join("\n\n")
    }
}

/// Rule-based compression strategy (no LLM needed)
pub struct RuleBasedStrategy;

impl RuleBasedStrategy {
    /// Compress steps using rule-based extraction
    pub fn compress(steps: &[LoopStep], current_summary: &str) -> String {
        let key_info = KeyInfoExtractor::extract(steps);
        let key_info_text = KeyInfoExtractor::format(&key_info);

        let mut summary_parts = Vec::new();

        // Include previous summary if exists
        if !current_summary.is_empty() {
            summary_parts.push(format!("Previous context:\n{}", current_summary));
        }

        // Add step summaries
        let step_summaries: Vec<String> = steps
            .iter()
            .map(|step| {
                format!(
                    "Step {}: {} → {}",
                    step.step_id,
                    step.action.action_type(),
                    if step.result.is_success() { "✓" } else { "✗" }
                )
            })
            .collect();

        summary_parts.push(format!("Steps executed:\n{}", step_summaries.join("\n")));

        // Add key information
        if !key_info_text.is_empty() {
            summary_parts.push(key_info_text);
        }

        summary_parts.join("\n\n---\n\n")
    }
}

/// Prompt template for LLM-based compression
pub struct CompressionPrompt;

impl CompressionPrompt {
    /// Build compression prompt
    pub fn build(current_summary: &str, steps: &[LoopStep], target_tokens: usize) -> String {
        let key_info = KeyInfoExtractor::extract(steps);
        let key_info_text = KeyInfoExtractor::format(&key_info);

        let steps_text: String = steps
            .iter()
            .map(|step| {
                format!(
                    "Step {}:\n  Reasoning: {}\n  Action: {}\n  Result: {}\n",
                    step.step_id,
                    step.thinking
                        .reasoning
                        .as_deref()
                        .unwrap_or("No reasoning"),
                    step.action.action_type(),
                    truncate(&step.result.summary(), 200)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"Summarize the following task execution history concisely.

## Current Summary (if any)
{current_summary}

## New Steps to Compress
{steps_text}

## Key Information to Preserve
{key_info_text}

## Instructions
1. Preserve KEY information:
   - Important tool outputs (file paths, search results, errors)
   - User decisions and clarifications
   - State changes (files created, data fetched)

2. Remove redundant details:
   - Verbose tool output formatting
   - Repeated similar operations
   - Intermediate reasoning that led nowhere

3. Format as bullet points, max {target_tokens} tokens

## Output
Provide a concise summary that allows continuing the task:"#,
            current_summary = if current_summary.is_empty() {
                "(none)"
            } else {
                current_summary
            },
            steps_text = steps_text,
            key_info_text = if key_info_text.is_empty() {
                "(none)"
            } else {
                &key_info_text
            },
            target_tokens = target_tokens
        )
    }
}

/// Truncate string to max length
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::{Action, ActionResult, Decision, Thinking};
    use serde_json::json;

    fn create_test_step(id: usize, action_type: &str, success: bool) -> LoopStep {
        // Add path argument for file-related actions
        let arguments = if action_type.contains("file") {
            json!({"path": format!("/test/file_{}.txt", id)})
        } else {
            json!({})
        };

        LoopStep {
            step_id: id,
            observation_summary: String::new(),
            thinking: Thinking {
                reasoning: Some(format!("Reasoning for step {}", id)),
                decision: Decision::Complete {
                    summary: "done".to_string(),
                },
            },
            action: Action::ToolCall {
                tool_name: action_type.to_string(),
                arguments,
            },
            result: if success {
                ActionResult::ToolSuccess {
                    output: json!({"result": "ok"}),
                    duration_ms: 100,
                }
            } else {
                ActionResult::ToolError {
                    error: "Failed".to_string(),
                    retryable: false,
                }
            },
            tokens_used: 100,
            duration_ms: 100,
        }
    }

    #[test]
    fn test_key_info_extraction() {
        let steps = vec![
            create_test_step(0, "read_file", true),
            create_test_step(1, "web_search", true),
            create_test_step(2, "write_file", false),
        ];

        let info = KeyInfoExtractor::extract(&steps);

        // Should extract file-related actions
        assert!(!info.file_changes.is_empty());
        // Should have current state
        assert!(!info.current_state.is_empty());
    }

    #[test]
    fn test_rule_based_compression() {
        let steps = vec![
            create_test_step(0, "search", true),
            create_test_step(1, "read_file", true),
            create_test_step(2, "summarize", true),
        ];

        let summary = RuleBasedStrategy::compress(&steps, "");

        assert!(summary.contains("Step 0"));
        assert!(summary.contains("Step 1"));
        assert!(summary.contains("Step 2"));
    }

    #[test]
    fn test_compression_prompt_building() {
        let steps = vec![create_test_step(0, "search", true)];

        let prompt = CompressionPrompt::build("Previous context", &steps, 500);

        assert!(prompt.contains("Previous context"));
        assert!(prompt.contains("Step 0"));
        assert!(prompt.contains("500 tokens"));
    }
}
