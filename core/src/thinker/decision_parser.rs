//! Decision parser for Agent Loop
//!
//! This module parses LLM responses into structured decisions.

use serde_json::Value;

use crate::agent_loop::{Decision, LlmResponse, Thinking, ThinkingParser};
use crate::error::{AlephError, Result};

/// Parser for LLM decision responses
pub struct DecisionParser {
    /// Whether to be strict about JSON format
    strict_mode: bool,
}

impl Default for DecisionParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DecisionParser {
    /// Create a new parser
    pub fn new() -> Self {
        Self { strict_mode: false }
    }

    /// Create a strict parser
    pub fn strict() -> Self {
        Self { strict_mode: true }
    }

    /// Parse LLM response into Thinking struct
    pub fn parse(&self, response: &str) -> Result<Thinking> {
        // Check for protocol tokens before JSON parsing
        if let Some(token) = super::protocol_tokens::ProtocolToken::parse(response) {
            let decision = match token {
                super::protocol_tokens::ProtocolToken::HeartbeatOk => Decision::HeartbeatOk,
                super::protocol_tokens::ProtocolToken::SilentComplete => Decision::Silent,
                super::protocol_tokens::ProtocolToken::NoReply => Decision::Silent,
                super::protocol_tokens::ProtocolToken::NeedsAttention(msg) => {
                    Decision::Complete { summary: format!("⚠️ Needs Attention: {}", msg) }
                }
            };
            return Ok(Thinking {
                reasoning: Some("Protocol token response".to_string()),
                decision,
                structured: None,
                tokens_used: None,
            });
        }

        // Try to extract JSON from response
        let json_str = match self.extract_json(response) {
            Ok(json) => json,
            Err(_) => {
                // Log a preview of the response for debugging
                let preview: String = response.chars().take(200).collect();
                let preview_suffix = if response.len() > 200 { "..." } else { "" };
                tracing::warn!(
                    response_len = response.len(),
                    preview = %format!("{}{}", preview, preview_suffix),
                    "LLM did not return JSON action format"
                );
                return Err(AlephError::Other {
                    message: "LLM did not return valid JSON action format".to_string(),
                    suggestion: Some(format!(
                        "The LLM returned plain text instead of JSON action. Response preview: '{}{}'",
                        preview, preview_suffix
                    )),
                });
            }
        };

        // Parse JSON
        let llm_response: LlmResponse = serde_json::from_str(&json_str).map_err(|e| {
            // Log the extracted JSON for debugging
            let json_preview: String = json_str.chars().take(300).collect();
            tracing::warn!(
                error = %e,
                json_preview = %json_preview,
                "Failed to parse extracted JSON as LlmResponse"
            );
            AlephError::Other {
                message: format!("Failed to parse LLM response JSON: {}", e),
                suggestion: Some(format!(
                    "Extracted JSON is malformed. Expected format: {{\"reasoning\": \"...\", \"action\": {{\"type\": \"tool|complete|fail|ask_user\", ...}}}}. Got: {}",
                    json_preview
                )),
            }
        })?;

        // Convert to Thinking
        let decision: Decision = llm_response.action.into();

        // Parse structured thinking if reasoning is present
        let structured = llm_response.reasoning.as_ref().map(|r| ThinkingParser::parse(r));

        Ok(Thinking {
            reasoning: llm_response.reasoning,
            decision,
            structured,
            tokens_used: None,
        })
    }

    /// Try to parse with fallback for malformed responses
    pub fn parse_with_fallback(&self, response: &str) -> Result<Thinking> {
        // First try normal parsing
        if let Ok(thinking) = self.parse(response) {
            return Ok(thinking);
        }

        // Try alternative JSON field names (e.g., "thought" instead of "reasoning")
        if let Some(thinking) = self.try_parse_alternative_format(response) {
            return Ok(thinking);
        }

        // Try to recover from truncated JSON (LLM hit max_tokens mid-response)
        if let Some(thinking) = self.try_recover_truncated_json(response) {
            return Ok(thinking);
        }

        // Try to extract tool call from response
        if let Some(thinking) = self.try_extract_tool_call(response) {
            return Ok(thinking);
        }

        // Try to detect completion intent
        if let Some(thinking) = self.try_detect_completion(response) {
            return Ok(thinking);
        }

        // If all else fails, treat as a failure
        if self.strict_mode {
            Err(AlephError::Other {
                message: "Could not parse LLM response into valid decision".to_string(),
                suggestion: Some("Check the LLM response format".to_string()),
            })
        } else {
            let reasoning = Some(response.to_string());
            let structured = reasoning.as_ref().map(|r| ThinkingParser::parse(r));
            Ok(Thinking {
                reasoning,
                decision: Decision::Fail {
                    reason: "Could not parse response into valid action".to_string(),
                },
                structured,
                tokens_used: None,
            })
        }
    }

    /// Try to parse with alternative field names that LLMs might use
    fn try_parse_alternative_format(&self, response: &str) -> Option<Thinking> {
        let json_str = self.extract_json(response).ok()?;
        let value: serde_json::Value = serde_json::from_str(&json_str).ok()?;
        let obj = value.as_object()?;

        // Extract reasoning (try multiple field names)
        let reasoning = obj
            .get("reasoning")
            .or_else(|| obj.get("thought"))
            .or_else(|| obj.get("thinking"))
            .or_else(|| obj.get("rationale"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Extract action object
        let action_obj = obj.get("action").and_then(|v| v.as_object())?;

        // Extract action type
        let action_type = action_obj
            .get("type")
            .or_else(|| action_obj.get("action_type"))
            .and_then(|v| v.as_str())?
            .to_lowercase();

        // Parse based on action type
        let decision = match action_type.as_str() {
            "tool" | "use_tool" | "tool_call" => {
                let tool_name = action_obj
                    .get("tool_name")
                    .or_else(|| action_obj.get("name"))
                    .or_else(|| action_obj.get("tool"))
                    .and_then(|v| v.as_str())?
                    .to_string();

                let arguments = action_obj
                    .get("arguments")
                    .or_else(|| action_obj.get("args"))
                    .or_else(|| action_obj.get("params"))
                    .or_else(|| action_obj.get("parameters"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                Decision::UseTool {
                    tool_name,
                    arguments,
                }
            }
            "ask_user" | "ask" | "question" | "clarify" => {
                let question = action_obj
                    .get("question")
                    .or_else(|| action_obj.get("message"))
                    .or_else(|| action_obj.get("text"))
                    .and_then(|v| v.as_str())?
                    .to_string();

                let options = action_obj
                    .get("options")
                    .or_else(|| action_obj.get("choices"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    });

                Decision::AskUser { question, options }
            }
            "complete" | "done" | "finish" | "success" => {
                let summary = action_obj
                    .get("summary")
                    .or_else(|| action_obj.get("result"))
                    .or_else(|| action_obj.get("message"))
                    .or_else(|| action_obj.get("output"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Task completed")
                    .to_string();

                Decision::Complete { summary }
            }
            "fail" | "error" | "failure" | "abort" => {
                let reason = action_obj
                    .get("reason")
                    .or_else(|| action_obj.get("error"))
                    .or_else(|| action_obj.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();

                Decision::Fail { reason }
            }
            _ => return None,
        };

        let structured = reasoning.as_ref().map(|r| ThinkingParser::parse(r));
        Some(Thinking { reasoning, decision, structured, tokens_used: None })
    }

    /// Extract JSON from response (handles markdown code blocks)
    fn extract_json(&self, response: &str) -> Result<String> {
        let trimmed = response.trim();

        // Try to find JSON in code block
        if let Some(json) = self.extract_from_code_block(trimmed) {
            return Ok(json);
        }

        // Try to find action JSON specifically (for long responses with embedded data)
        if let Some(json) = self.extract_action_json(trimmed) {
            return Ok(json);
        }

        // Try to find raw JSON object from the end (LLM might output action at the end)
        if let Some(json) = self.extract_raw_json_from_end(trimmed) {
            return Ok(json);
        }

        // Try to find raw JSON object from the start
        if let Some(json) = self.extract_raw_json(trimmed) {
            return Ok(json);
        }

        // If response starts with { but we couldn't extract valid JSON, still return it
        // and let the JSON parser provide a more specific error
        if trimmed.starts_with('{') {
            return Ok(trimmed.to_string());
        }

        // No JSON found at all - return error
        Err(AlephError::Other {
            message: "No JSON object found in response".to_string(),
            suggestion: Some("LLM response does not contain a JSON action object".to_string()),
        })
    }

    /// Extract JSON from markdown code block
    ///
    /// Only extracts if the JSON looks like an action JSON (contains "action" and "type")
    fn extract_from_code_block(&self, response: &str) -> Option<String> {
        // Collect all potential code blocks
        let mut candidates = Vec::new();

        // Try ```json ... ``` format
        let mut search_start = 0;
        while let Some(start) = response[search_start..].find("```json") {
            let abs_start = search_start + start;
            let content_start = abs_start + 7;
            if let Some(end) = response[content_start..].find("```") {
                let content = response[content_start..content_start + end].trim().to_string();
                candidates.push(content);
                search_start = content_start + end + 3;
            } else {
                break;
            }
        }

        // Try ``` ... ``` format (without json marker) - only if no json blocks found
        if candidates.is_empty() {
            search_start = 0;
            while let Some(start) = response[search_start..].find("```") {
                let abs_start = search_start + start;
                let content_start = abs_start + 3;
                // Skip language identifier if present
                let content_start = response[content_start..]
                    .find('\n')
                    .map(|i| content_start + i + 1)
                    .unwrap_or(content_start);

                if let Some(end) = response[content_start..].find("```") {
                    let content = response[content_start..content_start + end].trim();
                    // Verify it looks like JSON
                    if content.starts_with('{') {
                        candidates.push(content.to_string());
                    }
                    search_start = content_start + end + 3;
                } else {
                    break;
                }
            }
        }

        // Return first candidate that looks like an action JSON
        for candidate in &candidates {
            if candidate.contains("\"action\"") && candidate.contains("\"type\"") {
                return Some(candidate.clone());
            }
        }

        // If no action JSON found in code blocks, return None
        // and let other extraction methods try to find the action JSON
        None
    }

    /// Extract raw JSON object from response
    fn extract_raw_json(&self, response: &str) -> Option<String> {
        // Find first { and matching }
        let start = response.find('{')?;
        let mut depth = 0;
        let mut end = start;

        for (i, c) in response[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }

        if depth == 0 && end > start {
            Some(response[start..end].to_string())
        } else {
            None
        }
    }

    /// Extract action JSON specifically from response
    ///
    /// Looks for JSON objects that contain "action" field with a "type" field,
    /// which is the expected format for LLM decisions.
    /// This handles cases where LLM outputs content before/after the action JSON.
    fn extract_action_json(&self, response: &str) -> Option<String> {
        // Look for patterns that indicate action JSON
        // Pattern 1: {"reasoning":... (with or without leading whitespace)
        // Pattern 2: {"action":... (some LLMs skip reasoning)

        let action_patterns = [
            r#"{"reasoning""#,
            r#"{ "reasoning""#,
            r#"{"action""#,
            r#"{ "action""#,
        ];

        for pattern in action_patterns {
            if let Some(pos) = response.find(pattern) {
                // Extract JSON starting from this position
                if let Some(json) = self.extract_json_at(response, pos) {
                    // Validate it looks like an action JSON
                    if json.contains("\"action\"") && json.contains("\"type\"") {
                        return Some(json);
                    }
                }
            }
        }

        None
    }

    /// Extract raw JSON from the end of response (for long responses where action is at the end)
    fn extract_raw_json_from_end(&self, response: &str) -> Option<String> {
        // Find last } and matching {
        let end = response.rfind('}')?;

        let response_bytes = response.as_bytes();
        let mut depth = 0;
        let mut start = end;

        // Walk backwards to find matching {
        for i in (0..=end).rev() {
            match response_bytes[i] as char {
                '}' => depth += 1,
                '{' => {
                    depth -= 1;
                    if depth == 0 {
                        start = i;
                        break;
                    }
                }
                _ => {}
            }
        }

        if depth == 0 && start < end {
            let json = &response[start..=end];
            // Validate it looks like an action JSON (has "action" and "type")
            if json.contains("\"action\"") && json.contains("\"type\"") {
                return Some(json.to_string());
            }
        }

        None
    }

    /// Extract JSON starting at a specific position
    fn extract_json_at(&self, response: &str, start: usize) -> Option<String> {
        let mut depth = 0;
        let mut end = start;

        for (i, c) in response[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }

        if depth == 0 && end > start {
            Some(response[start..end].to_string())
        } else {
            None
        }
    }

    /// Try to recover from truncated JSON (LLM hit max_tokens mid-response).
    ///
    /// When the LLM generates a very long reasoning field, the JSON may be truncated
    /// before the action field is reached. Instead of failing, we extract the partial
    /// reasoning and return a Complete decision with a summary derived from the context.
    fn try_recover_truncated_json(&self, response: &str) -> Option<Thinking> {
        let trimmed = response.trim();

        // Must look like truncated JSON: starts with code-block or `{`, has "reasoning" but no matching `}`
        let json_content = if let Some(start) = trimmed.find("```json") {
            let content_start = start + 7;
            // No closing ``` — truncated code block
            if trimmed[content_start..].contains("```") {
                return None; // Code block is properly closed, not truncated
            }
            trimmed[content_start..].trim()
        } else if trimmed.starts_with('{') {
            trimmed
        } else {
            return None;
        };

        // Check it has "reasoning" but the JSON is incomplete (unmatched braces)
        if !json_content.contains("\"reasoning\"") {
            return None;
        }
        let open_braces = json_content.chars().filter(|&c| c == '{').count();
        let close_braces = json_content.chars().filter(|&c| c == '}').count();
        if close_braces >= open_braces {
            return None; // Braces are balanced, not truncated
        }

        tracing::warn!(
            response_len = response.len(),
            open_braces,
            close_braces,
            "Detected truncated JSON response (likely max_tokens exceeded), recovering"
        );

        // Extract partial reasoning text
        let reasoning_text = if let Some(start) = json_content.find("\"reasoning\"") {
            let after_key = &json_content[start + 11..]; // skip `"reasoning"`
            // Find the value start (skip `: "` or `:"`)
            if let Some(quote_pos) = after_key.find('"') {
                let value_start = quote_pos + 1;
                let value_content = &after_key[value_start..];
                // Take content up to the end (since JSON is truncated, there's no closing quote)
                // Try to find a clean end point
                let clean_text = value_content
                    .replace("\\n", "\n")
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\");
                Some(clean_text)
            } else {
                None
            }
        } else {
            None
        };

        // Build a Complete decision from the partial reasoning
        let summary = if let Some(ref text) = reasoning_text {
            // Use the reasoning as the response since the LLM was clearly
            // trying to generate a final answer (it had all the data)
            let truncated: String = text.chars().take(4000).collect();
            truncated
        } else {
            "Response was truncated. Please try again with a simpler request.".to_string()
        };

        let reasoning = reasoning_text.or_else(|| Some(response.to_string()));
        let structured = reasoning.as_ref().map(|r| ThinkingParser::parse(r));

        Some(Thinking {
            reasoning,
            decision: Decision::Complete { summary },
            structured,
            tokens_used: None,
        })
    }

    /// Try to extract a tool call from non-JSON response
    fn try_extract_tool_call(&self, response: &str) -> Option<Thinking> {
        // Look for patterns like "I'll use the search tool" or "Let me search for"
        let response_lower = response.to_lowercase();

        // Common tool invocation patterns (more specific to avoid false positives)
        // These patterns indicate intent to use a tool, not past actions
        let tool_patterns = [
            ("use search", "web_search"),
            ("search for", "web_search"),
            ("let me search", "web_search"),
            ("i'll search", "web_search"),
            ("read file", "read_file"),
            ("read the file", "read_file"),
            ("write file", "write_file"),
            ("write to file", "write_file"),
            ("execute code", "execute_code"),
            ("run the code", "execute_code"),
            ("run command", "run_command"),
            ("execute command", "run_command"),
        ];

        for (pattern, tool_name) in tool_patterns {
            if response_lower.contains(pattern) {
                let reasoning = Some(response.to_string());
                let structured = reasoning.as_ref().map(|r| ThinkingParser::parse(r));
                return Some(Thinking {
                    reasoning,
                    decision: Decision::UseTool {
                        tool_name: tool_name.to_string(),
                        arguments: Value::Object(serde_json::Map::new()),
                    },
                    structured,
                    tokens_used: None,
                });
            }
        }

        None
    }

    /// Try to detect completion intent from response
    fn try_detect_completion(&self, response: &str) -> Option<Thinking> {
        let response_lower = response.to_lowercase();

        // Completion indicators
        let completion_patterns = [
            "task complete",
            "task is complete",
            "i have completed",
            "successfully completed",
            "finished",
            "done",
            "here is the result",
            "here are the results",
        ];

        for pattern in completion_patterns {
            if response_lower.contains(pattern) {
                let reasoning = Some(response.to_string());
                let structured = reasoning.as_ref().map(|r| ThinkingParser::parse(r));
                return Some(Thinking {
                    reasoning,
                    decision: Decision::Complete {
                        summary: response.to_string(),
                    },
                    structured,
                    tokens_used: None,
                });
            }
        }

        None
    }

    /// Validate a decision
    pub fn validate(&self, decision: &Decision) -> Result<()> {
        match decision {
            Decision::UseTool {
                tool_name,
                arguments,
            } => {
                if tool_name.is_empty() {
                    return Err(AlephError::Other {
                        message: "Tool name cannot be empty".to_string(),
                        suggestion: Some("Provide a valid tool name".to_string()),
                    });
                }
                if !arguments.is_object() {
                    return Err(AlephError::Other {
                        message: "Tool arguments must be an object".to_string(),
                        suggestion: Some("Provide arguments as a JSON object".to_string()),
                    });
                }
                Ok(())
            }
            Decision::AskUser { question, .. } => {
                if question.is_empty() {
                    return Err(AlephError::Other {
                        message: "Question cannot be empty".to_string(),
                        suggestion: Some("Provide a question for the user".to_string()),
                    });
                }
                Ok(())
            }
            Decision::AskUserMultigroup { question, groups } => {
                if question.is_empty() {
                    return Err(AlephError::Other {
                        message: "Question cannot be empty".to_string(),
                        suggestion: Some("Provide a question for the user".to_string()),
                    });
                }
                if groups.is_empty() {
                    return Err(AlephError::Other {
                        message: "Groups cannot be empty for multi-group question".to_string(),
                        suggestion: Some("Provide at least one question group".to_string()),
                    });
                }
                Ok(())
            }
            Decision::AskUserRich { question, .. } => {
                if question.is_empty() {
                    return Err(AlephError::Other {
                        message: "Question cannot be empty".to_string(),
                        suggestion: Some("Provide a question for the user".to_string()),
                    });
                }
                Ok(())
            }
            Decision::Complete { summary } => {
                if summary.is_empty() {
                    return Err(AlephError::Other {
                        message: "Summary cannot be empty".to_string(),
                        suggestion: Some("Provide a summary of what was accomplished".to_string()),
                    });
                }
                Ok(())
            }
            Decision::Fail { reason } => {
                if reason.is_empty() {
                    return Err(AlephError::Other {
                        message: "Failure reason cannot be empty".to_string(),
                        suggestion: Some("Provide a reason for the failure".to_string()),
                    });
                }
                Ok(())
            }
            Decision::Silent | Decision::HeartbeatOk => {
                // These are always valid - no parameters to check
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_response() {
        let parser = DecisionParser::new();

        let response = r#"{
            "reasoning": "I need to search for information",
            "action": {
                "type": "tool",
                "tool_name": "web_search",
                "arguments": {"query": "rust tutorials"}
            }
        }"#;

        let thinking = parser.parse(response).unwrap();
        assert!(thinking.reasoning.is_some());
        assert!(matches!(thinking.decision, Decision::UseTool { .. }));
    }

    #[test]
    fn test_parse_code_block_response() {
        let parser = DecisionParser::new();

        let response = r#"Here's my decision:

```json
{
    "reasoning": "The task is complete",
    "action": {
        "type": "complete",
        "summary": "Task completed successfully"
    }
}
```

That should do it!"#;

        let thinking = parser.parse(response).unwrap();
        assert!(matches!(thinking.decision, Decision::Complete { .. }));
    }

    #[test]
    fn test_parse_ask_user_response() {
        let parser = DecisionParser::new();

        let response = r#"{
            "reasoning": "I need clarification",
            "action": {
                "type": "ask_user",
                "question": "Which option do you prefer?",
                "options": ["Option A", "Option B"]
            }
        }"#;

        let thinking = parser.parse(response).unwrap();
        if let Decision::AskUser { question, options } = thinking.decision {
            assert_eq!(question, "Which option do you prefer?");
            assert!(options.is_some());
        } else {
            panic!("Expected AskUser decision");
        }
    }

    #[test]
    fn test_parse_fail_response() {
        let parser = DecisionParser::new();

        let response = r#"{
            "reasoning": "Cannot proceed",
            "action": {
                "type": "fail",
                "reason": "Required file not found"
            }
        }"#;

        let thinking = parser.parse(response).unwrap();
        assert!(matches!(thinking.decision, Decision::Fail { .. }));
    }

    #[test]
    fn test_extract_raw_json() {
        let parser = DecisionParser::new();

        let response = r#"Let me think about this...

{"reasoning": "test", "action": {"type": "complete", "summary": "done"}}

Hope that helps!"#;

        let thinking = parser.parse(response).unwrap();
        assert!(matches!(thinking.decision, Decision::Complete { .. }));
    }

    #[test]
    fn test_validation() {
        let parser = DecisionParser::new();

        // Valid decision
        let valid = Decision::UseTool {
            tool_name: "search".to_string(),
            arguments: serde_json::json!({"query": "test"}),
        };
        assert!(parser.validate(&valid).is_ok());

        // Invalid: empty tool name
        let invalid = Decision::UseTool {
            tool_name: "".to_string(),
            arguments: serde_json::json!({}),
        };
        assert!(parser.validate(&invalid).is_err());

        // Invalid: empty question
        let invalid = Decision::AskUser {
            question: "".to_string(),
            options: None,
        };
        assert!(parser.validate(&invalid).is_err());
    }

    #[test]
    fn test_fallback_parsing() {
        let parser = DecisionParser::new();

        // Response that looks like completion
        let response = "Task complete! I have finished searching for the information.";
        let thinking = parser.parse_with_fallback(response).unwrap();
        assert!(matches!(thinking.decision, Decision::Complete { .. }));
    }

    #[test]
    fn test_alternative_format_parsing() {
        let parser = DecisionParser::new();

        // Test with alternative field names: "thought" instead of "reasoning"
        // and "name" instead of "tool_name"
        let response = r#"{
            "thought": "I should write the file now",
            "action": {
                "type": "tool",
                "name": "file_ops",
                "args": {"operation": "write", "path": "/tmp/test.txt", "content": "hello"}
            }
        }"#;

        let thinking = parser.parse_with_fallback(response).unwrap();
        assert!(thinking.reasoning.as_deref() == Some("I should write the file now"));
        if let Decision::UseTool { tool_name, arguments } = thinking.decision {
            assert_eq!(tool_name, "file_ops");
            assert_eq!(arguments["operation"], "write");
        } else {
            panic!("Expected UseTool decision");
        }
    }

    #[test]
    fn test_alternative_complete_format() {
        let parser = DecisionParser::new();

        // Test with "done" instead of "complete" and "result" instead of "summary"
        let response = r#"{
            "thinking": "Task is done",
            "action": {
                "type": "done",
                "result": "Successfully wrote all files"
            }
        }"#;

        let thinking = parser.parse_with_fallback(response).unwrap();
        if let Decision::Complete { summary } = thinking.decision {
            assert_eq!(summary, "Successfully wrote all files");
        } else {
            panic!("Expected Complete decision");
        }
    }

    #[test]
    fn test_extract_action_json_with_leading_content() {
        let parser = DecisionParser::new();

        // Simulate LLM outputting large content before the action JSON
        let response = r#"Here is the processed content:

## Chapter 1: Introduction
This is a long document with lots of content...
{"data": [1, 2, 3], "nested": {"key": "value"}}

## Chapter 2: More content
Even more text here with JSON-like content...

Now here is my action:
{"reasoning": "I have processed all chapters", "action": {"type": "complete", "summary": "Processed 2 chapters successfully"}}
"#;

        let thinking = parser.parse(response).unwrap();
        assert!(matches!(thinking.decision, Decision::Complete { .. }));
        if let Decision::Complete { summary } = thinking.decision {
            assert!(summary.contains("Processed 2 chapters"));
        }
    }

    #[test]
    fn test_extract_action_json_from_end() {
        let parser = DecisionParser::new();

        // Action JSON at the very end with lots of content before
        let response = r#"# Knowledge Graph Analysis

## Entities Found:
- Person: John (id: 1)
- Organization: Acme Corp (id: 2)

## Relationships:
{"source": 1, "target": 2, "type": "works_at"}

## Summary
Analysis complete.

{"reasoning": "Generated knowledge graph with 2 entities and 1 relationship", "action": {"type": "complete", "summary": "Knowledge graph generated successfully"}}"#;

        let thinking = parser.parse(response).unwrap();
        assert!(matches!(thinking.decision, Decision::Complete { .. }));
    }

    #[test]
    fn test_extract_action_json_with_data_json() {
        let parser = DecisionParser::new();

        // Response with data JSON followed by action JSON
        let response = r#"Here are the triples I extracted:

```json
{
  "triples": [
    {"subject": "Claude", "predicate": "is", "object": "AI"},
    {"subject": "Anthropic", "predicate": "created", "object": "Claude"}
  ]
}
```

Now I need to write these to a file:

{"reasoning": "I will write the triples to a file", "action": {"type": "tool", "tool_name": "file_ops", "arguments": {"operation": "write", "path": "triples.json"}}}"#;

        let thinking = parser.parse(response).unwrap();
        assert!(matches!(thinking.decision, Decision::UseTool { .. }));
        if let Decision::UseTool { tool_name, .. } = thinking.decision {
            assert_eq!(tool_name, "file_ops");
        }
    }

    #[test]
    fn test_very_long_response_with_action() {
        let parser = DecisionParser::new();

        // Simulate a very long response (like 122KB) with action at the end
        let mut long_content = String::new();
        for i in 0..1000 {
            long_content.push_str(&format!(
                "Line {}: This is some content that the LLM generated. {{\"data\": {}}}\n",
                i, i
            ));
        }
        long_content.push_str(
            r#"{"reasoning": "Processed all content", "action": {"type": "complete", "summary": "Done processing 1000 lines"}}"#,
        );

        let thinking = parser.parse(&long_content).unwrap();
        assert!(matches!(thinking.decision, Decision::Complete { .. }));
    }

    #[test]
    fn test_parse_produces_structured_thinking() {
        let parser = DecisionParser::new();

        let response = r#"{
            "reasoning": "Looking at the request, I see the user wants to search. I'll use the web_search tool. Therefore, I will execute the search.",
            "action": {
                "type": "tool",
                "tool_name": "web_search",
                "arguments": {"query": "rust tutorials"}
            }
        }"#;

        let thinking = parser.parse(response).unwrap();

        // Verify structured thinking was populated
        assert!(thinking.structured.is_some());

        let structured = thinking.structured.as_ref().unwrap();
        assert!(!structured.reasoning.is_empty());

        // Should have detected some steps
        assert!(structured.has_steps());
    }

    #[test]
    fn test_parse_with_fallback_produces_structured() {
        let parser = DecisionParser::new();

        // Response that triggers completion detection
        let response = "Task complete! I have finished searching for the information.";
        let thinking = parser.parse_with_fallback(response).unwrap();

        // Verify structured thinking was populated even for fallback path
        assert!(thinking.structured.is_some());
    }

    #[test]
    fn test_parse_no_reasoning_no_structured() {
        let parser = DecisionParser::new();

        // Response with null reasoning
        let response = r#"{
            "reasoning": null,
            "action": {
                "type": "complete",
                "summary": "Done"
            }
        }"#;

        let thinking = parser.parse(response).unwrap();

        // No reasoning means no structured thinking
        assert!(thinking.reasoning.is_none());
        assert!(thinking.structured.is_none());
    }

    #[test]
    fn test_parse_protocol_token_heartbeat() {
        let parser = DecisionParser::new();
        let result = parser.parse("ALEPH_HEARTBEAT_OK");
        assert!(result.is_ok());
        let thinking = result.unwrap();
        assert!(matches!(thinking.decision, Decision::HeartbeatOk));
    }

    #[test]
    fn test_parse_protocol_token_silent_complete() {
        let parser = DecisionParser::new();
        let result = parser.parse("ALEPH_SILENT_COMPLETE");
        assert!(result.is_ok());
        let thinking = result.unwrap();
        assert!(matches!(thinking.decision, Decision::Silent));
    }

    #[test]
    fn test_parse_protocol_token_no_reply() {
        let parser = DecisionParser::new();
        let result = parser.parse("ALEPH_NO_REPLY");
        assert!(result.is_ok());
        let thinking = result.unwrap();
        assert!(matches!(thinking.decision, Decision::Silent));
    }

    #[test]
    fn test_parse_protocol_token_needs_attention() {
        let parser = DecisionParser::new();
        let result = parser.parse("ALEPH_NEEDS_ATTENTION: Database disk usage at 95%");
        assert!(result.is_ok());
        let thinking = result.unwrap();
        if let Decision::Complete { summary } = thinking.decision {
            assert!(summary.contains("Needs Attention"));
            assert!(summary.contains("Database disk usage at 95%"));
        } else {
            panic!("Expected Complete decision for NeedsAttention token");
        }
    }

    #[test]
    fn test_parse_protocol_token_with_fallback() {
        let parser = DecisionParser::new();
        // parse_with_fallback should also handle tokens (via parse())
        let result = parser.parse_with_fallback("  ALEPH_HEARTBEAT_OK  ");
        assert!(result.is_ok());
        let thinking = result.unwrap();
        assert!(matches!(thinking.decision, Decision::HeartbeatOk));
    }

    #[test]
    fn test_recover_truncated_json_in_code_block() {
        let parser = DecisionParser::new();

        // Simulate truncated JSON in code block (LLM hit max_tokens)
        let response = "```json\n{\n  \"reasoning\": \"用户想要比特币分析报告。根据搜索结果，当前价格约67000美元，最近一个月涨幅明显。关键数据：上证指数突破4000点，成交额3万亿\",\n  \"action\": {\n    \"type\": \"complete\",\n    \"summary\": \"## 比特币分析报告";

        let thinking = parser.parse_with_fallback(response).unwrap();
        assert!(
            matches!(thinking.decision, Decision::Complete { .. }),
            "Truncated JSON should recover as Complete, got {:?}",
            thinking.decision
        );
        if let Decision::Complete { summary } = &thinking.decision {
            assert!(summary.contains("比特币"), "Summary should contain extracted reasoning");
        }
    }

    #[test]
    fn test_recover_truncated_json_raw() {
        let parser = DecisionParser::new();

        // Truncated raw JSON (reasoning field cut off, no action field)
        let response = "{\n  \"reasoning\": \"I collected data from 3 searches. The market shows strong momentum with volume exceeding 3 trillion. Key support at 4000, resistance at 4150. MACD shows bullish crossov";

        let thinking = parser.parse_with_fallback(response).unwrap();
        assert!(
            matches!(thinking.decision, Decision::Complete { .. }),
            "Truncated raw JSON should recover as Complete, got {:?}",
            thinking.decision
        );
    }

    #[test]
    fn test_no_recovery_for_complete_json() {
        let parser = DecisionParser::new();

        // Complete (non-truncated) JSON should NOT trigger recovery
        let response = r#"{"reasoning": "test", "action": {"type": "complete", "summary": "done"}}"#;
        let thinking = parser.parse(response).unwrap();
        assert!(matches!(thinking.decision, Decision::Complete { .. }));
        if let Decision::Complete { summary } = &thinking.decision {
            assert_eq!(summary, "done");
        }
    }
}
