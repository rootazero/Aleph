use crate::memory::context::{MemoryEntry, MemoryFact};
use crate::memory::fact_retrieval::RetrievalResult;
use super::config::ComptrollerConfig;
use super::types::{ArbitratedContext, RetentionMode, TokenBudget};

pub struct ContextComptroller {
    config: ComptrollerConfig,
}

impl ContextComptroller {
    pub fn new(config: ComptrollerConfig) -> Self {
        Self { config }
    }

    /// Arbitrate retrieval results to eliminate redundancy
    pub fn arbitrate(
        &self,
        results: RetrievalResult,
        _budget: TokenBudget,
    ) -> ArbitratedContext {
        let mut tokens_saved = 0;

        // Detect redundancy between facts and transcripts
        let redundant_pairs = self.detect_redundancy(&results.facts, &results.raw_memories);

        let mut kept_facts = Vec::new();
        let mut kept_transcripts = Vec::new();

        // Apply retention strategy
        match self.config.retention_mode {
            RetentionMode::PreferTranscript => {
                // Keep transcripts, remove redundant facts
                for fact in results.facts {
                    if !redundant_pairs.iter().any(|(f_id, _)| f_id == &fact.id) {
                        kept_facts.push(fact);
                    } else {
                        tokens_saved += self.estimate_tokens(&fact.content);
                    }
                }
                kept_transcripts = results.raw_memories;
            }
            RetentionMode::PreferFact => {
                // Keep facts, remove redundant transcripts
                kept_facts = results.facts;
                for transcript in results.raw_memories {
                    if !redundant_pairs.iter().any(|(_, t_id)| t_id == &transcript.id) {
                        kept_transcripts.push(transcript);
                    } else {
                        let text = format!("{} {}", transcript.user_input, transcript.ai_output);
                        tokens_saved += self.estimate_tokens(&text);
                    }
                }
            }
            RetentionMode::Hybrid => {
                // TODO: Implement hybrid strategy
                kept_facts = results.facts;
                kept_transcripts = results.raw_memories;
            }
        }

        ArbitratedContext {
            facts: kept_facts,
            raw_memories: kept_transcripts,
            tokens_saved,
        }
    }

    /// Detect redundant fact-transcript pairs
    fn detect_redundancy(
        &self,
        facts: &[MemoryFact],
        transcripts: &[MemoryEntry],
    ) -> Vec<(String, String)> {
        let mut pairs = Vec::new();

        for fact in facts {
            for transcript in transcripts {
                // Check if fact was derived from this transcript
                if fact.source_memory_ids.contains(&transcript.id) {
                    pairs.push((fact.id.clone(), transcript.id.clone()));
                    continue;
                }

                // Check embedding similarity if both have embeddings
                if let (Some(fact_emb), Some(trans_emb)) = (&fact.embedding, &transcript.embedding) {
                    let similarity = self.cosine_similarity(fact_emb, trans_emb);
                    if similarity >= self.config.similarity_threshold {
                        pairs.push((fact.id.clone(), transcript.id.clone()));
                    }
                }
            }
        }

        pairs
    }

    /// Calculate cosine similarity between two embeddings
    fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }

        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot_product / (norm_a * norm_b)
    }

    /// Estimate tokens (4 chars per token)
    fn estimate_tokens(&self, text: &str) -> usize {
        (text.len() / 4).max(1)
    }
}
