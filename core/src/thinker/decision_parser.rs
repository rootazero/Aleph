//! Decision parser for Agent Loop
//!
//! This module parses LLM responses into structured decisions.

use serde_json::Value;

use crate::agent_loop::{Decision, LlmResponse, Thinking};
use crate::error::{AetherError, Result};

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
        // Try to extract JSON from response
        let json_str = self.extract_json(response)?;

        // Parse JSON
        let llm_response: LlmResponse = serde_json::from_str(&json_str).map_err(|e| {
            AetherError::Other {
            message: format!("Failed to parse LLM response: {}", e),
            suggestion: Some("Ensure the LLM response is valid JSON".to_string()),
        }
        })?;

        // Convert to Thinking
        let decision: Decision = llm_response.action.into();

        Ok(Thinking {
            reasoning: llm_response.reasoning,
            decision,
        })
    }

    /// Try to parse with fallback for malformed responses
    pub fn parse_with_fallback(&self, response: &str) -> Result<Thinking> {
        // First try normal parsing
        if let Ok(thinking) = self.parse(response) {
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
            Err(AetherError::Other {
                message: "Could not parse LLM response into valid decision".to_string(),
                suggestion: Some("Check the LLM response format".to_string()),
            })
        } else {
            Ok(Thinking {
                reasoning: Some(response.to_string()),
                decision: Decision::Fail {
                    reason: "Could not parse response into valid action".to_string(),
                },
            })
        }
    }

    /// Extract JSON from response (handles markdown code blocks)
    fn extract_json(&self, response: &str) -> Result<String> {
        let trimmed = response.trim();

        // Try to find JSON in code block
        if let Some(json) = self.extract_from_code_block(trimmed) {
            return Ok(json);
        }

        // Try to find raw JSON object
        if let Some(json) = self.extract_raw_json(trimmed) {
            return Ok(json);
        }

        // Return trimmed response as-is and let JSON parser handle errors
        Ok(trimmed.to_string())
    }

    /// Extract JSON from markdown code block
    fn extract_from_code_block(&self, response: &str) -> Option<String> {
        // Try ```json ... ``` format
        if let Some(start) = response.find("```json") {
            let content_start = start + 7;
            if let Some(end) = response[content_start..].find("```") {
                return Some(response[content_start..content_start + end].trim().to_string());
            }
        }

        // Try ``` ... ``` format (without json marker)
        if let Some(start) = response.find("```") {
            let content_start = start + 3;
            // Skip language identifier if present
            let content_start = response[content_start..]
                .find('\n')
                .map(|i| content_start + i + 1)
                .unwrap_or(content_start);

            if let Some(end) = response[content_start..].find("```") {
                let content = response[content_start..content_start + end].trim();
                // Verify it looks like JSON
                if content.starts_with('{') {
                    return Some(content.to_string());
                }
            }
        }

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
                return Some(Thinking {
                    reasoning: Some(response.to_string()),
                    decision: Decision::UseTool {
                        tool_name: tool_name.to_string(),
                        arguments: Value::Object(serde_json::Map::new()),
                    },
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
                return Some(Thinking {
                    reasoning: Some(response.to_string()),
                    decision: Decision::Complete {
                        summary: response.to_string(),
                    },
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
                    return Err(AetherError::Other {
                        message: "Tool name cannot be empty".to_string(),
                        suggestion: Some("Provide a valid tool name".to_string()),
                    });
                }
                if !arguments.is_object() {
                    return Err(AetherError::Other {
                        message: "Tool arguments must be an object".to_string(),
                        suggestion: Some("Provide arguments as a JSON object".to_string()),
                    });
                }
                Ok(())
            }
            Decision::AskUser { question, .. } => {
                if question.is_empty() {
                    return Err(AetherError::Other {
                        message: "Question cannot be empty".to_string(),
                        suggestion: Some("Provide a question for the user".to_string()),
                    });
                }
                Ok(())
            }
            Decision::Complete { summary } => {
                if summary.is_empty() {
                    return Err(AetherError::Other {
                        message: "Summary cannot be empty".to_string(),
                        suggestion: Some("Provide a summary of what was accomplished".to_string()),
                    });
                }
                Ok(())
            }
            Decision::Fail { reason } => {
                if reason.is_empty() {
                    return Err(AetherError::Other {
                        message: "Failure reason cannot be empty".to_string(),
                        suggestion: Some("Provide a reason for the failure".to_string()),
                    });
                }
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
}
