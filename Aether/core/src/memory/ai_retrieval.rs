//! AI-powered memory retrieval for selecting relevant past conversations.
//!
//! This module replaces embedding-based vector similarity search with AI-based
//! relevance evaluation. The AI analyzes candidate memories and selects those
//! most relevant to the current user query.
//!
//! # Architecture
//!
//! ```text
//! User Query + Recent Memories
//!     ↓
//! [1] Fetch recent N memories from database
//! [2] Filter out current session content (deduplication)
//! [3] Send candidates to AI for relevance evaluation
//! [4] Parse AI response → selected memory IDs
//! [5] Return selected memories for prompt augmentation
//! ```

use crate::error::{AetherError, Result};
use crate::memory::context::MemoryEntry;
use crate::providers::AiProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Simplified memory candidate for AI evaluation.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryCandidate {
    /// Memory ID (for reference)
    pub id: String,
    /// User input from this memory
    pub user_input: String,
    /// AI response from this memory (truncated for prompt size)
    pub ai_output: String,
    /// Unix timestamp
    pub timestamp: i64,
    /// App bundle ID (for context)
    pub app_bundle_id: String,
}

impl From<&MemoryEntry> for MemoryCandidate {
    fn from(entry: &MemoryEntry) -> Self {
        Self {
            id: entry.id.clone(),
            user_input: entry.user_input.clone(),
            // Truncate AI output to save tokens
            ai_output: entry.ai_output.chars().take(300).collect(),
            timestamp: entry.context.timestamp,
            app_bundle_id: entry.context.app_bundle_id.clone(),
        }
    }
}

/// Request for AI memory selection.
#[derive(Debug, Clone)]
pub struct AiMemoryRequest {
    /// Current user query
    pub query: String,
    /// Candidate memories to evaluate
    pub candidates: Vec<MemoryCandidate>,
}

/// Result of AI memory selection.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AiMemoryResult {
    /// IDs of memories selected as relevant
    pub selected_memory_ids: Vec<String>,
    /// Brief reasoning (optional, for debugging)
    #[serde(default)]
    pub reasoning: Option<String>,
}


/// AI-powered memory retriever.
pub struct AiMemoryRetriever {
    /// AI provider for memory evaluation
    provider: Arc<dyn AiProvider>,
    /// Timeout for AI memory selection
    timeout: Duration,
    /// Maximum candidates to send to AI
    max_candidates: u32,
    /// Fallback count if AI selection fails
    fallback_count: u32,
}

impl AiMemoryRetriever {
    /// Create a new AI memory retriever.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider,
            timeout: Duration::from_millis(3000),
            max_candidates: 20,
            fallback_count: 3,
        }
    }

    /// Set the timeout duration.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set maximum candidates to send to AI.
    pub fn with_max_candidates(mut self, max_candidates: u32) -> Self {
        self.max_candidates = max_candidates;
        self
    }

    /// Set fallback count for when AI fails.
    pub fn with_fallback_count(mut self, fallback_count: u32) -> Self {
        self.fallback_count = fallback_count;
        self
    }

    /// Select relevant memories using AI.
    ///
    /// Returns memories that the AI determined are relevant to the query.
    /// Falls back to most recent memories on error or timeout.
    pub async fn retrieve(
        &self,
        query: &str,
        candidates: Vec<MemoryEntry>,
        exclude_inputs: &[String],
    ) -> Result<Vec<MemoryEntry>> {
        // Filter out current session content
        let filtered_candidates: Vec<MemoryEntry> = candidates
            .into_iter()
            .filter(|m| !exclude_inputs.iter().any(|ex| m.user_input.contains(ex)))
            .collect();

        if filtered_candidates.is_empty() {
            debug!("No candidates after filtering, returning empty");
            return Ok(Vec::new());
        }

        // Limit candidates
        let limited_candidates: Vec<MemoryEntry> = filtered_candidates
            .into_iter()
            .take(self.max_candidates as usize)
            .collect();

        // Convert to candidate format for AI
        let ai_candidates: Vec<MemoryCandidate> =
            limited_candidates.iter().map(MemoryCandidate::from).collect();

        let request = AiMemoryRequest {
            query: query.to_string(),
            candidates: ai_candidates,
        };

        // Try AI selection with timeout
        let result = tokio::time::timeout(self.timeout, self.retrieve_internal(&request)).await;

        match result {
            Ok(Ok(selected_ids)) => {
                // Filter candidates by selected IDs
                let selected_set: HashSet<_> = selected_ids.into_iter().collect();
                let selected_memories: Vec<MemoryEntry> = limited_candidates
                    .into_iter()
                    .filter(|m| selected_set.contains(&m.id))
                    .collect();

                info!(
                    selected_count = selected_memories.len(),
                    "AI memory selection completed"
                );
                Ok(selected_memories)
            }
            Ok(Err(e)) => {
                warn!(error = %e, "AI memory selection failed, using fallback");
                Ok(self.fallback_selection(limited_candidates))
            }
            Err(_) => {
                warn!(
                    timeout_ms = self.timeout.as_millis(),
                    "AI memory selection timed out, using fallback"
                );
                Ok(self.fallback_selection(limited_candidates))
            }
        }
    }

    /// Internal retrieval logic.
    async fn retrieve_internal(&self, request: &AiMemoryRequest) -> Result<Vec<String>> {
        if request.candidates.is_empty() {
            return Ok(Vec::new());
        }

        let prompt = self.build_selection_prompt(&request.query, &request.candidates);
        let system_prompt = self.get_system_prompt();

        debug!(
            query_length = request.query.len(),
            candidate_count = request.candidates.len(),
            "Starting AI memory selection"
        );

        // Call AI provider
        let response = self
            .provider
            .process(&prompt, Some(&system_prompt))
            .await
            .map_err(|e| {
                AetherError::config(format!(
                    "AI memory selection failed: {}. Falling back to recent memories.",
                    e
                ))
            })?;

        // Parse response
        let result = self.parse_response(&response)?;

        debug!(
            selected_ids = ?result.selected_memory_ids,
            reasoning = ?result.reasoning,
            "AI memory selection parsed"
        );

        Ok(result.selected_memory_ids)
    }

    /// Fallback selection: return most recent N memories.
    fn fallback_selection(&self, candidates: Vec<MemoryEntry>) -> Vec<MemoryEntry> {
        candidates
            .into_iter()
            .take(self.fallback_count as usize)
            .collect()
    }

    /// Get the system prompt for memory selection.
    fn get_system_prompt(&self) -> String {
        r#"You are a memory relevance evaluator. Your task is to select which past conversations are relevant to the current user query.

RULES:
1. Select memories that provide useful context for answering the current query
2. Prioritize memories with similar topics, entities, or concepts
3. Recent memories may be more relevant than older ones
4. Select 0-5 memories maximum (empty selection is valid if nothing is relevant)
5. Do NOT select memories that are unrelated to the current query

OUTPUT FORMAT:
Respond ONLY with valid JSON, no markdown:
{"selected_memory_ids":["id1","id2"],"reasoning":"Brief explanation"}

If no memories are relevant:
{"selected_memory_ids":[],"reasoning":"No relevant memories found"}

EXAMPLE:
Current query: "What was the Python version we discussed?"
Memories:
- [1] ID: abc123, User: "How to install Python 3.11?", Assistant: "Use pyenv or official installer..."
- [2] ID: def456, User: "What's the weather in Tokyo?", Assistant: "Tokyo weather is..."
- [3] ID: ghi789, User: "Best Python IDE?", Assistant: "VS Code with Python extension..."

Output: {"selected_memory_ids":["abc123","ghi789"],"reasoning":"Both memories relate to Python discussion"}"#.to_string()
    }

    /// Build the selection prompt with candidates.
    fn build_selection_prompt(&self, query: &str, candidates: &[MemoryCandidate]) -> String {
        let mut prompt = format!(
            r#"Select relevant memories for this query.

Current user query: "{}"

Past conversations:
"#,
            query.replace('"', "\\\"")
        );

        for (idx, candidate) in candidates.iter().enumerate() {
            let truncated_input: String = candidate.user_input.chars().take(200).collect();
            let truncated_output: String = candidate.ai_output.chars().take(200).collect();

            prompt.push_str(&format!(
                "\n[{}] ID: {}\nUser: {}\nAssistant: {}...\n---",
                idx + 1,
                candidate.id,
                truncated_input,
                truncated_output
            ));
        }

        prompt.push_str("\n\nSelect relevant memories (JSON only):");
        prompt
    }

    /// Parse the AI response into AiMemoryResult.
    fn parse_response(&self, response: &str) -> Result<AiMemoryResult> {
        let json_str = self.extract_json(response);

        match serde_json::from_str::<AiMemoryResult>(&json_str) {
            Ok(result) => Ok(result),
            Err(e) => {
                warn!(
                    response = %response,
                    error = %e,
                    "Failed to parse AI memory response, returning empty selection"
                );
                Ok(AiMemoryResult::default())
            }
        }
    }

    /// Extract JSON from response, handling markdown code blocks.
    fn extract_json(&self, response: &str) -> String {
        let response = response.trim();

        // Check for markdown code block
        if response.starts_with("```") {
            let lines: Vec<&str> = response.lines().collect();
            let mut json_lines = Vec::new();
            let mut in_block = false;

            for line in lines {
                if line.starts_with("```") {
                    if in_block {
                        break;
                    }
                    in_block = true;
                    continue;
                }
                if in_block {
                    json_lines.push(line);
                }
            }

            json_lines.join("\n")
        } else if response.starts_with('{') {
            response.to_string()
        } else {
            // Try to find JSON object in response
            if let Some(start) = response.find('{') {
                if let Some(end) = response.rfind('}') {
                    return response[start..=end].to_string();
                }
            }
            response.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::ContextAnchor;
    use std::future::Future;
    use std::pin::Pin;

    struct MockProvider {
        response: String,
    }

    impl AiProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }

        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<String>> + Send + '_>> {
            let response = self.response.clone();
            Box::pin(async move { Ok(response) })
        }
    }

    fn create_test_memory(id: &str, user_input: &str, ai_output: &str) -> MemoryEntry {
        MemoryEntry::new(
            id.to_string(),
            ContextAnchor::now("com.test.app".to_string(), "Test".to_string()),
            user_input.to_string(),
            ai_output.to_string(),
        )
    }

    #[test]
    fn test_extract_json_plain() {
        let provider = Arc::new(MockProvider {
            response: String::new(),
        });
        let retriever = AiMemoryRetriever::new(provider);
        let json = r#"{"selected_memory_ids":["id1"],"reasoning":"test"}"#;
        assert_eq!(retriever.extract_json(json), json);
    }

    #[test]
    fn test_extract_json_markdown() {
        let provider = Arc::new(MockProvider {
            response: String::new(),
        });
        let retriever = AiMemoryRetriever::new(provider);
        let input = "```json\n{\"selected_memory_ids\":[\"id1\"]}\n```";
        assert_eq!(
            retriever.extract_json(input),
            r#"{"selected_memory_ids":["id1"]}"#
        );
    }

    #[test]
    fn test_parse_response_valid() {
        let provider = Arc::new(MockProvider {
            response: String::new(),
        });
        let retriever = AiMemoryRetriever::new(provider);
        let json = r#"{"selected_memory_ids":["id1","id2"],"reasoning":"Both relevant"}"#;
        let result = retriever.parse_response(json).unwrap();
        assert_eq!(result.selected_memory_ids, vec!["id1", "id2"]);
        assert_eq!(result.reasoning, Some("Both relevant".to_string()));
    }

    #[test]
    fn test_parse_response_empty() {
        let provider = Arc::new(MockProvider {
            response: String::new(),
        });
        let retriever = AiMemoryRetriever::new(provider);
        let json = r#"{"selected_memory_ids":[],"reasoning":"No relevant memories"}"#;
        let result = retriever.parse_response(json).unwrap();
        assert!(result.selected_memory_ids.is_empty());
    }

    #[test]
    fn test_parse_response_invalid() {
        let provider = Arc::new(MockProvider {
            response: String::new(),
        });
        let retriever = AiMemoryRetriever::new(provider);
        let result = retriever.parse_response("not json").unwrap();
        assert!(result.selected_memory_ids.is_empty());
    }

    #[test]
    fn test_fallback_selection() {
        let provider = Arc::new(MockProvider {
            response: String::new(),
        });
        let retriever = AiMemoryRetriever::new(provider).with_fallback_count(2);

        let candidates = vec![
            create_test_memory("id1", "input1", "output1"),
            create_test_memory("id2", "input2", "output2"),
            create_test_memory("id3", "input3", "output3"),
        ];

        let result = retriever.fallback_selection(candidates);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, "id1");
        assert_eq!(result[1].id, "id2");
    }

    #[tokio::test]
    async fn test_retrieve_with_ai() {
        let provider = Arc::new(MockProvider {
            response: r#"{"selected_memory_ids":["id1"],"reasoning":"Relevant to query"}"#
                .to_string(),
        });
        let retriever = AiMemoryRetriever::new(provider);

        let candidates = vec![
            create_test_memory("id1", "Python version?", "Python 3.11"),
            create_test_memory("id2", "Weather in Tokyo?", "Sunny"),
        ];

        let result = retriever
            .retrieve("What Python version?", candidates, &[])
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "id1");
    }

    #[tokio::test]
    async fn test_retrieve_with_exclusion() {
        let provider = Arc::new(MockProvider {
            response: r#"{"selected_memory_ids":["id2"],"reasoning":"Relevant"}"#.to_string(),
        });
        let retriever = AiMemoryRetriever::new(provider);

        let candidates = vec![
            create_test_memory("id1", "Current question", "Response"),
            create_test_memory("id2", "Past question", "Past response"),
        ];

        // Exclude "Current question" (simulating current session content)
        let result = retriever
            .retrieve("Follow up", candidates, &["Current question".to_string()])
            .await
            .unwrap();

        // id1 should be filtered out, only id2 considered
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "id2");
    }

    #[test]
    fn test_memory_candidate_from_entry() {
        let entry = create_test_memory("test-id", "test input", "test output that is long");
        let candidate = MemoryCandidate::from(&entry);

        assert_eq!(candidate.id, "test-id");
        assert_eq!(candidate.user_input, "test input");
        assert_eq!(candidate.ai_output, "test output that is long");
    }
}
