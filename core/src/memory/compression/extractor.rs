//! Fact Extractor
//!
//! Extracts structured facts from conversation memories using LLM.
//! Facts are third-person statements about the user.

use crate::error::AlephError;

/// Safely truncate a string at character boundaries (UTF-8 safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}
use crate::memory::context::{FactType, MemoryEntry, MemoryFact};
use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::Arc;

/// A fact extracted by the LLM before embedding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFact {
    /// Fact content (third-person statement)
    pub content: String,
    /// Type classification
    pub fact_type: String,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
    /// Source memory IDs
    pub source_ids: Vec<String>,
}

/// Response format from LLM extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExtractionResponse {
    facts: Vec<ExtractedFact>,
}

/// Extracts facts from conversations using LLM
pub struct FactExtractor {
    provider: Arc<dyn AiProvider>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl FactExtractor {
    /// Create a new fact extractor
    pub fn new(provider: Arc<dyn AiProvider>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self { provider, embedder }
    }

    /// Extract facts from a batch of memories
    pub async fn extract_facts(
        &self,
        memories: &[MemoryEntry],
    ) -> Result<Vec<MemoryFact>, AlephError> {
        if memories.is_empty() {
            return Ok(Vec::new());
        }

        // Build extraction prompt
        let prompt = self.build_extraction_prompt(memories);
        let system_prompt = self.get_system_prompt();

        // Call LLM
        let response = self
            .provider
            .process(&prompt, Some(&system_prompt))
            .await
            .map_err(|e| AlephError::other(format!("LLM extraction failed: {}", e)))?;

        // Parse response
        let extracted = self.parse_extraction_response(&response, memories)?;

        // Generate embeddings for each fact
        let mut facts_with_embeddings = Vec::new();
        for extracted_fact in extracted {
            let embedding = self
                .embedder
                .embed(&extracted_fact.content)
                .await
                .map_err(|e| AlephError::other(format!("Embedding generation failed: {}", e)))?;

            let fact = MemoryFact::new(
                extracted_fact.content,
                FactType::from_str_or_other(&extracted_fact.fact_type),
                extracted_fact.source_ids,
            )
            .with_embedding(embedding)
            .with_confidence(extracted_fact.confidence);

            facts_with_embeddings.push(fact);
        }

        Ok(facts_with_embeddings)
    }

    /// Get the system prompt for fact extraction
    fn get_system_prompt(&self) -> String {
        r#"You are a memory compression assistant. Extract key facts, preferences, and plans from conversations.

RULES:
1. Write facts in THIRD PERSON (e.g., "The user is learning Rust", NOT "I am learning Rust")
2. Each fact should be a single, atomic statement
3. Classify each fact into one of: preference, plan, learning, project, personal, other
4. Assign confidence (0.0-1.0) based on how certain the information is
5. Include the source memory IDs for traceability
6. Extract 0-10 facts maximum per batch
7. Focus on ACTIONABLE or MEMORABLE information
8. Ignore greetings, small talk, and transient information

OUTPUT FORMAT (JSON only, no markdown code blocks):
{
  "facts": [
    {
      "content": "The user prefers using Vim for coding",
      "fact_type": "preference",
      "confidence": 0.9,
      "source_ids": ["mem-123"]
    }
  ]
}

FACT TYPES:
- preference: User likes/dislikes, habits, style choices
- plan: Goals, intentions, scheduled activities
- learning: Skills being learned, knowledge areas
- project: Work projects, side projects, tasks
- personal: Personal info (non-sensitive), relationships
- other: Anything else notable

EXAMPLE INPUT:
User: "I've been learning Rust for 2 months now, really enjoying it"
Assistant: "That's great! Rust has a steep learning curve..."

EXAMPLE OUTPUT:
{
  "facts": [
    {
      "content": "The user has been learning Rust programming for approximately 2 months",
      "fact_type": "learning",
      "confidence": 0.95,
      "source_ids": ["mem-xxx"]
    },
    {
      "content": "The user enjoys learning Rust",
      "fact_type": "preference",
      "confidence": 0.85,
      "source_ids": ["mem-xxx"]
    }
  ]
}"#
            .to_string()
    }

    /// Build the extraction prompt from memories
    fn build_extraction_prompt(&self, memories: &[MemoryEntry]) -> String {
        let mut prompt = String::from("Extract key facts from these conversations:\n\n");

        for (idx, memory) in memories.iter().enumerate() {
            prompt.push_str(&format!(
                "--- Conversation {} (ID: {}) ---\n",
                idx + 1,
                memory.id
            ));
            prompt.push_str(&format!("User: {}\n", memory.user_input));

            // Truncate long AI responses
            let ai_output: String = memory.ai_output.chars().take(500).collect();
            let truncated = if memory.ai_output.len() > 500 {
                format!("{}...", ai_output)
            } else {
                ai_output
            };
            prompt.push_str(&format!("Assistant: {}\n\n", truncated));
        }

        prompt.push_str("Extract facts (JSON only, no markdown):");
        prompt
    }

    /// Parse the LLM response into extracted facts
    fn parse_extraction_response(
        &self,
        response: &str,
        memories: &[MemoryEntry],
    ) -> Result<Vec<ExtractedFact>, AlephError> {
        // Try to find JSON in the response
        let json_str = self.extract_json_from_response(response)?;

        // Parse JSON
        let parsed: ExtractionResponse = serde_json::from_str(&json_str).map_err(|e| {
            AlephError::other(format!(
                "Failed to parse extraction response: {}. Response: {}",
                e, json_str
            ))
        })?;

        // Validate and fix source_ids
        let memory_ids: Vec<String> = memories.iter().map(|m| m.id.clone()).collect();

        let validated_facts: Vec<ExtractedFact> = parsed
            .facts
            .into_iter()
            .map(|mut fact| {
                // If source_ids are invalid or empty, use all memory IDs
                if fact.source_ids.is_empty()
                    || fact.source_ids.iter().any(|id| !memory_ids.contains(id))
                {
                    fact.source_ids = memory_ids.clone();
                }

                // Clamp confidence
                fact.confidence = fact.confidence.clamp(0.0, 1.0);

                fact
            })
            .collect();

        Ok(validated_facts)
    }

    /// Extract JSON from potentially wrapped response
    fn extract_json_from_response(&self, response: &str) -> Result<String, AlephError> {
        let trimmed = response.trim();

        // Try to find JSON object directly
        if trimmed.starts_with('{') {
            // Find matching closing brace
            let mut depth = 0;
            let mut end_idx = 0;

            for (idx, ch) in trimmed.chars().enumerate() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end_idx = idx + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }

            if end_idx > 0 {
                return Ok(trimmed[..end_idx].to_string());
            }
        }

        // Try to extract from markdown code block
        if let Some(start) = trimmed.find("```json") {
            if let Some(end) = trimmed[start + 7..].find("```") {
                return Ok(trimmed[start + 7..start + 7 + end].trim().to_string());
            }
        }

        if let Some(start) = trimmed.find("```") {
            if let Some(end) = trimmed[start + 3..].find("```") {
                let content = trimmed[start + 3..start + 3 + end].trim();
                if content.starts_with('{') {
                    return Ok(content.to_string());
                }
            }
        }

        // Try to find JSON anywhere in the response
        if let Some(start) = trimmed.find('{') {
            if let Some(end) = trimmed.rfind('}') {
                if end > start {
                    return Ok(trimmed[start..=end].to_string());
                }
            }
        }

        Err(AlephError::other(format!(
            "Could not find valid JSON in response: {}",
            truncate_str(trimmed, 200)
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_extract_json_simple() {
        let extractor = create_test_extractor();

        let response = r#"{"facts": []}"#;
        let result = extractor.extract_json_from_response(response);
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_extract_json_with_markdown() {
        let extractor = create_test_extractor();

        let response = r#"Here are the facts:
```json
{"facts": [{"content": "test", "fact_type": "other", "confidence": 0.9, "source_ids": []}]}
```"#;

        let result = extractor.extract_json_from_response(response);
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_extract_json_with_text_before() {
        let extractor = create_test_extractor();

        let response = r#"Based on the conversations, I extracted:
{"facts": [{"content": "User likes Rust", "fact_type": "preference", "confidence": 0.8, "source_ids": []}]}"#;

        let result = extractor.extract_json_from_response(response);
        assert!(result.is_ok());
    }

    fn create_test_extractor() -> FactExtractor {
        use crate::providers::create_mock_provider;
        use crate::memory::embedding_provider::tests::MockEmbeddingProvider;

        let provider = create_mock_provider();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));

        FactExtractor::new(provider, embedder)
    }
}
