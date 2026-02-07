//! Contradiction detection using LLM and similarity search

use std::sync::Arc;

use crate::memory::{MemoryFact, VectorDatabase};
use crate::providers::AiProvider;
use crate::Result;

/// Detects contradictions between facts
pub struct ContradictionDetector {
    database: Arc<VectorDatabase>,
    provider: Option<Arc<dyn AiProvider>>,
    similarity_threshold: f32,
}

/// Configuration for contradiction detection
#[derive(Debug, Clone)]
pub struct ContradictionConfig {
    /// Similarity threshold for finding candidate facts (default: 0.7)
    pub similarity_threshold: f32,

    /// Maximum number of similar facts to check (default: 10)
    pub max_candidates: u32,

    /// Whether to use LLM for detection (default: true if provider available)
    pub use_llm: bool,
}

impl Default for ContradictionConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.7,
            max_candidates: 10,
            use_llm: true,
        }
    }
}

impl ContradictionDetector {
    /// Create a new contradiction detector
    pub fn new(
        database: Arc<VectorDatabase>,
        provider: Option<Arc<dyn AiProvider>>,
    ) -> Self {
        Self {
            database,
            provider,
            similarity_threshold: 0.7,
        }
    }

    /// Create with custom similarity threshold
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.similarity_threshold = threshold;
        self
    }

    /// Detect facts that contradict the given fact
    ///
    /// Returns a list of facts that contradict the new fact, along with
    /// the reason for the contradiction.
    pub async fn detect(&self, new_fact: &MemoryFact) -> Result<Vec<(MemoryFact, String)>> {
        // Find similar facts as candidates
        let candidates = self.find_candidates(new_fact).await?;

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Use LLM to detect contradictions if available
        if let Some(provider) = &self.provider {
            self.detect_with_llm(new_fact, &candidates, provider)
                .await
        } else {
            // Fallback to keyword-based detection
            Ok(self.detect_with_keywords(new_fact, &candidates))
        }
    }

    /// Find candidate facts that might contradict the new fact
    async fn find_candidates(&self, new_fact: &MemoryFact) -> Result<Vec<MemoryFact>> {
        let Some(embedding) = &new_fact.embedding else {
            return Ok(Vec::new());
        };

        // Search for similar facts
        let similar = self
            .database
            .search_facts(embedding, crate::memory::NamespaceScope::Owner, 10, false)
            .await?;

        // Filter by similarity threshold and exclude the fact itself
        Ok(similar
            .into_iter()
            .filter(|f| {
                f.id != new_fact.id
                    && f.similarity_score
                        .map(|s| s >= self.similarity_threshold)
                        .unwrap_or(false)
            })
            .collect())
    }

    /// Detect contradictions using LLM
    async fn detect_with_llm(
        &self,
        new_fact: &MemoryFact,
        candidates: &[MemoryFact],
        provider: &Arc<dyn AiProvider>,
    ) -> Result<Vec<(MemoryFact, String)>> {
        let mut contradictions = Vec::new();

        // Build prompt
        let candidates_text = candidates
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{}. {}", i + 1, f.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "New fact: {}\n\n\
             Existing facts:\n{}\n\n\
             Does the new fact contradict any existing facts? \
             Respond with a JSON array of objects with 'index' (1-based) and 'reason' fields. \
             If no contradictions, respond with an empty array [].",
            new_fact.content, candidates_text
        );

        let system_prompt = "You are a fact contradiction detector. \
                            Identify logical contradictions between facts. \
                            Two facts contradict if they cannot both be true at the same time.";

        // Call LLM
        let response = provider.process(&prompt, Some(system_prompt)).await?;

        // Parse response (simple JSON parsing)
        if let Ok(parsed) = self.parse_llm_response(&response, candidates) {
            contradictions = parsed;
        }

        Ok(contradictions)
    }

    /// Parse LLM response to extract contradictions
    fn parse_llm_response(
        &self,
        response: &str,
        candidates: &[MemoryFact],
    ) -> Result<Vec<(MemoryFact, String)>> {
        // Try to parse as JSON
        let response = response.trim();

        // Handle empty array
        if response == "[]" {
            return Ok(Vec::new());
        }

        // Simple parsing: look for patterns like {"index": 1, "reason": "..."}
        let mut contradictions = Vec::new();

        // Extract index and reason pairs
        for line in response.lines() {
            if let Some(index) = self.extract_index(line) {
                if index > 0 && index <= candidates.len() {
                    let reason = self.extract_reason(line).unwrap_or_else(|| {
                        "Contradiction detected by LLM".to_string()
                    });
                    contradictions.push((candidates[index - 1].clone(), reason));
                }
            }
        }

        Ok(contradictions)
    }

    /// Extract index from JSON-like string
    fn extract_index(&self, text: &str) -> Option<usize> {
        // Look for "index": N or "index":N
        if let Some(start) = text.find("\"index\"") {
            let after = &text[start + 7..];
            if let Some(colon) = after.find(':') {
                let after_colon = &after[colon + 1..];
                // Extract digits
                let digits: String = after_colon
                    .chars()
                    .skip_while(|c| !c.is_ascii_digit())
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                return digits.parse().ok();
            }
        }
        None
    }

    /// Extract reason from JSON-like string
    fn extract_reason(&self, text: &str) -> Option<String> {
        // Look for "reason": "..."
        if let Some(start) = text.find("\"reason\"") {
            let after = &text[start + 8..];
            if let Some(colon) = after.find(':') {
                let after_colon = &after[colon + 1..].trim();
                // Extract quoted string
                if let Some(quote_start) = after_colon.find('"') {
                    let after_quote = &after_colon[quote_start + 1..];
                    if let Some(quote_end) = after_quote.find('"') {
                        return Some(after_quote[..quote_end].to_string());
                    }
                }
            }
        }
        None
    }

    /// Detect contradictions using simple keyword matching (fallback)
    fn detect_with_keywords(
        &self,
        new_fact: &MemoryFact,
        candidates: &[MemoryFact],
    ) -> Vec<(MemoryFact, String)> {
        let mut contradictions = Vec::new();

        // Simple heuristic: look for negation patterns
        let new_lower = new_fact.content.to_lowercase();
        let has_negation = new_lower.contains("not ")
            || new_lower.contains("never ")
            || new_lower.contains("doesn't ")
            || new_lower.contains("don't ");

        for candidate in candidates {
            let candidate_lower = candidate.content.to_lowercase();
            let candidate_has_negation = candidate_lower.contains("not ")
                || candidate_lower.contains("never ")
                || candidate_lower.contains("doesn't ")
                || candidate_lower.contains("don't ");

            // If one has negation and the other doesn't, might be a contradiction
            if has_negation != candidate_has_negation {
                // Check if they share significant words
                let new_words: Vec<_> = new_lower
                    .split_whitespace()
                    .filter(|w| w.len() > 3)
                    .collect();
                let candidate_words: Vec<_> = candidate_lower
                    .split_whitespace()
                    .filter(|w| w.len() > 3)
                    .collect();

                let common_words = new_words
                    .iter()
                    .filter(|w| candidate_words.contains(w))
                    .count();

                if common_words >= 2 {
                    contradictions.push((
                        candidate.clone(),
                        "Potential negation-based contradiction".to_string(),
                    ));
                }
            }
        }

        contradictions
    }
}
