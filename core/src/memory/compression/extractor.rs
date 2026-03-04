//! Fact Extractor
//!
//! Extracts structured facts from conversation memories using LLM.
//! Facts are third-person statements about the user.

use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryEntry, MemoryFact};
use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use crate::utils::json_extract::extract_json_robust;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::Arc;
use tracing::warn;

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
        let json_value = match extract_json_robust(response) {
            Some(v) => v,
            None => {
                warn!("No JSON found in extraction response, returning empty facts");
                return Ok(vec![]);
            }
        };

        // Parse JSON
        let parsed: ExtractionResponse = match serde_json::from_value(json_value) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse extraction JSON as ExtractionResponse: {}, returning empty facts", e);
                return Ok(vec![]);
            }
        };

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

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_simple() {
        let response = r#"{"facts": []}"#;
        let result = extract_json_robust(response);
        assert!(result.is_some());
    }

    #[test]
    fn test_extract_json_with_markdown() {
        let response = r#"Here are the facts:
```json
{"facts": [{"content": "test", "fact_type": "other", "confidence": 0.9, "source_ids": []}]}
```"#;

        let result = extract_json_robust(response);
        assert!(result.is_some());
    }

    #[test]
    fn test_extract_json_with_text_before() {
        let response = r#"Based on the conversations, I extracted:
{"facts": [{"content": "User likes Rust", "fact_type": "preference", "confidence": 0.8, "source_ids": []}]}"#;

        let result = extract_json_robust(response);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_extraction_plain_text_fallback() {
        use crate::memory::context::ContextAnchor;

        let extractor = create_test_extractor();
        let memories = vec![MemoryEntry {
            id: "mem-1".to_string(),
            user_input: "Hello".to_string(),
            ai_output: "Hi there".to_string(),
            context: ContextAnchor {
                app_bundle_id: String::new(),
                window_title: String::new(),
                timestamp: 0,
                topic_id: "single-turn".to_string(),
            },
            embedding: None,
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            similarity_score: None,
        }];

        // Plain text should return empty vec, not error
        let result = extractor.parse_extraction_response("这是纯文本回复", &memories);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    fn create_test_extractor() -> FactExtractor {
        use crate::providers::create_mock_provider;
        use crate::memory::embedding_provider::tests::MockEmbeddingProvider;

        let provider = create_mock_provider();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbeddingProvider::new(1024, "mock-model"));

        FactExtractor::new(provider, embedder)
    }
}
