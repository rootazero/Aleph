//! Tool use summary — async LLM-generated one-line summaries of tool calls.
//!
//! Fires after tool execution and resolves during the next Think step's
//! streaming. Non-blocking, silent on failure.

use crate::providers::adapter::RequestPayload;
use crate::providers::AiProvider;

const SUMMARY_SYSTEM_PROMPT: &str = "\
You are a tool-use summarizer. Write a single short summary (~30 characters) \
of what the tools accomplished. Use past tense verb, be specific, no period.\n\
\n\
Examples:\n\
- Searched auth/ for login bugs\n\
- Read 3 config files\n\
- Created signup API endpoint\n\
- Fetched weather data for Tokyo";

const INPUT_TRUNCATE_CHARS: usize = 300;
const ASSISTANT_TEXT_TRUNCATE_CHARS: usize = 200;

/// Input for a single tool call to be summarized.
pub struct ToolSummaryInput {
    pub tool_name: String,
    pub tool_input: String,
    pub tool_output: String,
}

/// Truncate a string at the nearest char boundary at or before `max` bytes.
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let end = s
        .char_indices()
        .take_while(|(i, _)| *i <= max)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    &s[..end]
}

/// Build the user prompt from tool inputs and optional assistant context.
fn build_prompt(tools: &[ToolSummaryInput], last_assistant_text: Option<&str>) -> String {
    let mut prompt = String::new();

    if let Some(text) = last_assistant_text {
        let truncated = truncate_str(text, ASSISTANT_TEXT_TRUNCATE_CHARS);
        prompt.push_str(&format!("User intent: {truncated}\n\n"));
    }

    for (i, tool) in tools.iter().enumerate() {
        let input = truncate_str(&tool.tool_input, INPUT_TRUNCATE_CHARS);
        let output = truncate_str(&tool.tool_output, INPUT_TRUNCATE_CHARS);
        prompt.push_str(&format!(
            "Tool {}: {}\nInput: {}\nOutput: {}\n\n",
            i + 1,
            tool.tool_name,
            input,
            output,
        ));
    }

    prompt.push_str("Summary:");
    prompt
}

/// Generate a tool use summary using a lightweight LLM provider.
///
/// Returns `None` on any error (logged via tracing::warn). Never panics.
pub async fn generate_tool_summary(
    provider: &dyn AiProvider,
    tools: &[ToolSummaryInput],
    last_assistant_text: Option<&str>,
) -> Option<String> {
    if tools.is_empty() {
        return None;
    }

    let user_prompt = build_prompt(tools, last_assistant_text);
    let messages = vec![crate::providers::message::UnifiedMessage::user(
        &user_prompt,
    )];

    let payload = RequestPayload {
        messages: &messages,
        system_prompt: Some(SUMMARY_SYSTEM_PROMPT),
        tools: None,
        model: None,
        ..Default::default()
    };

    match provider.process(payload).await {
        Ok(response) => {
            let text = response.text?.trim().to_string();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "Tool summary generation failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str_ascii() {
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_str_no_truncation_needed() {
        assert_eq!(truncate_str("short", 100), "short");
    }

    #[test]
    fn test_truncate_str_utf8_boundary() {
        let s = "你好世界";
        let result = truncate_str(s, 7);
        assert_eq!(result, "你好");
    }

    #[test]
    fn test_truncate_str_empty() {
        assert_eq!(truncate_str("", 10), "");
    }

    #[test]
    fn test_build_prompt_single_tool() {
        let tools = vec![ToolSummaryInput {
            tool_name: "search".into(),
            tool_input: r#"{"query": "rust async"}"#.into(),
            tool_output: "Found 3 results".into(),
        }];
        let prompt = build_prompt(&tools, None);
        assert!(prompt.contains("Tool 1: search"));
        assert!(prompt.contains("rust async"));
        assert!(prompt.contains("Summary:"));
    }

    #[test]
    fn test_build_prompt_with_assistant_context() {
        let tools = vec![ToolSummaryInput {
            tool_name: "read".into(),
            tool_input: "config.toml".into(),
            tool_output: "[file content]".into(),
        }];
        let prompt = build_prompt(&tools, Some("I'll check the config file"));
        assert!(prompt.contains("User intent: I'll check the config file"));
    }

    #[test]
    fn test_build_prompt_truncates_long_input() {
        let long_input = "x".repeat(1000);
        let tools = vec![ToolSummaryInput {
            tool_name: "fetch".into(),
            tool_input: long_input.clone(),
            tool_output: "ok".into(),
        }];
        let prompt = build_prompt(&tools, None);
        assert!(!prompt.contains(&long_input));
        assert!(prompt.len() < 1000);
    }
}
