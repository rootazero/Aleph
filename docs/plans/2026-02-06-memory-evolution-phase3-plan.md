# Memory System Evolution - Phase 3 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement advanced memory features including RippleTask, Fact Evolution Chain, ConsolidationTask, semantic chunking, and LLM-based importance scoring.

**Architecture:** Build on Phase 1 & 2 foundations to add cognitive depth, knowledge evolution, and user profiling capabilities.

**Tech Stack:** Rust (tokio async), sqlite-vec, fastembed, LLM providers (Claude/GPT-4), graph algorithms

---

## Overview

Phase 3 implements advanced cognitive features:

1. **RippleTask** - Local exploration and knowledge expansion
2. **Fact Evolution Chain** - Contradiction resolution and fact evolution
3. **ConsolidationTask** - User profile distillation
4. **Semantic Chunking** - Advanced chunking based on semantic boundaries
5. **LLM-based Scoring** - More accurate importance estimation using LLMs

**Success Criteria:**
- Contradictory facts are automatically reconciled
- High-frequency facts are distilled into user profiles
- Semantic chunking improves retrieval precision
- LLM scoring outperforms keyword-based scoring
- All tests pass, documentation complete

---

## Task 1: Implement RippleTask for Local Exploration

**Goal:** Enable local exploration around retrieved facts to expand knowledge context.

**Concept:**
When a fact is retrieved, RippleTask explores related facts within N hops in the knowledge graph, enriching the context with connected information.

**Files:**
- Create: `core/src/memory/ripple/mod.rs`
- Create: `core/src/memory/ripple/task.rs`
- Create: `core/src/memory/ripple/config.rs`
- Modify: `core/src/memory/mod.rs`

### Implementation Steps

#### Step 1: Define RippleTask structure

```rust
pub struct RippleTask {
    graph_store: Arc<GraphStore>,
    database: Arc<VectorDatabase>,
    config: RippleConfig,
}

pub struct RippleConfig {
    pub max_hops: usize,           // Default: 2
    pub max_facts_per_hop: usize,  // Default: 5
    pub similarity_threshold: f32,  // Default: 0.7
}

pub struct RippleResult {
    pub seed_facts: Vec<MemoryFact>,
    pub expanded_facts: Vec<MemoryFact>,
    pub total_hops: usize,
}
```

#### Step 2: Implement exploration algorithm

```rust
impl RippleTask {
    pub async fn explore(&self, seed_facts: Vec<MemoryFact>) -> Result<RippleResult> {
        let mut visited = HashSet::new();
        let mut expanded = Vec::new();

        for hop in 0..self.config.max_hops {
            let current_facts = if hop == 0 {
                seed_facts.clone()
            } else {
                expanded.clone()
            };

            for fact in current_facts {
                if visited.contains(&fact.id) {
                    continue;
                }
                visited.insert(fact.id.clone());

                // Find related facts via graph edges
                let related = self.graph_store
                    .get_related_facts(&fact.id, self.config.max_facts_per_hop)
                    .await?;

                // Filter by similarity
                for related_fact in related {
                    if self.is_similar(&fact, &related_fact) {
                        expanded.push(related_fact);
                    }
                }
            }
        }

        Ok(RippleResult {
            seed_facts,
            expanded_facts: expanded,
            total_hops: self.config.max_hops,
        })
    }
}
```

#### Step 3: Write tests

```rust
#[tokio::test]
async fn test_ripple_single_hop() {
    // Create graph with connected facts
    // Run ripple with max_hops=1
    // Verify only 1-hop neighbors are returned
}

#[tokio::test]
async fn test_ripple_multi_hop() {
    // Create graph with 3-level hierarchy
    // Run ripple with max_hops=2
    // Verify 2-hop expansion works correctly
}
```

---

## Task 2: Implement Fact Evolution Chain

**Goal:** Detect and resolve contradictory facts, maintaining evolution history.

**Concept:**
When new facts contradict existing facts, create an evolution chain that records the progression of knowledge, allowing the system to understand how beliefs changed over time.

**Files:**
- Create: `core/src/memory/evolution/mod.rs`
- Create: `core/src/memory/evolution/detector.rs`
- Create: `core/src/memory/evolution/resolver.rs`
- Create: `core/src/memory/evolution/chain.rs`

### Implementation Steps

#### Step 1: Define evolution structures

```rust
pub struct FactEvolution {
    pub chain_id: String,
    pub facts: Vec<EvolutionNode>,
    pub created_at: i64,
}

pub struct EvolutionNode {
    pub fact_id: String,
    pub fact: MemoryFact,
    pub superseded_by: Option<String>,
    pub reason: String,
    pub timestamp: i64,
}

pub struct ContradictionDetector {
    database: Arc<VectorDatabase>,
    provider: Arc<dyn AiProvider>,
}
```

#### Step 2: Implement contradiction detection

```rust
impl ContradictionDetector {
    pub async fn detect(&self, new_fact: &MemoryFact) -> Result<Vec<MemoryFact>> {
        // Find similar facts
        let similar = self.database
            .search_facts(&new_fact.embedding.unwrap(), 10, false)
            .await?;

        // Use LLM to detect contradictions
        let prompt = format!(
            "Does this new fact contradict any existing facts?\n\
             New: {}\n\
             Existing: {:?}",
            new_fact.content,
            similar.iter().map(|f| &f.content).collect::<Vec<_>>()
        );

        let response = self.provider.complete(&prompt).await?;

        // Parse LLM response to identify contradictions
        // Return list of contradictory facts
    }
}
```

#### Step 3: Implement evolution chain

```rust
impl EvolutionChain {
    pub async fn create_evolution(
        &self,
        old_fact: MemoryFact,
        new_fact: MemoryFact,
        reason: String,
    ) -> Result<FactEvolution> {
        let chain_id = uuid::Uuid::new_v4().to_string();

        let old_node = EvolutionNode {
            fact_id: old_fact.id.clone(),
            fact: old_fact,
            superseded_by: Some(new_fact.id.clone()),
            reason: reason.clone(),
            timestamp: now_timestamp(),
        };

        let new_node = EvolutionNode {
            fact_id: new_fact.id.clone(),
            fact: new_fact,
            superseded_by: None,
            reason,
            timestamp: now_timestamp(),
        };

        Ok(FactEvolution {
            chain_id,
            facts: vec![old_node, new_node],
            created_at: now_timestamp(),
        })
    }
}
```

---

## Task 3: Implement ConsolidationTask

**Goal:** Distill high-frequency facts into user profiles.

**Concept:**
Analyze frequently accessed facts to extract stable user preferences, habits, and characteristics, creating a consolidated user profile.

**Files:**
- Create: `core/src/memory/consolidation/mod.rs`
- Create: `core/src/memory/consolidation/analyzer.rs`
- Create: `core/src/memory/consolidation/profile.rs`

### Implementation Steps

#### Step 1: Define profile structures

```rust
pub struct UserProfile {
    pub profile_id: String,
    pub categories: HashMap<String, ProfileCategory>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct ProfileCategory {
    pub name: String,
    pub facts: Vec<ConsolidatedFact>,
    pub confidence: f32,
}

pub struct ConsolidatedFact {
    pub content: String,
    pub source_fact_ids: Vec<String>,
    pub access_count: u32,
    pub last_accessed: i64,
}
```

#### Step 2: Implement frequency analysis

```rust
impl ConsolidationAnalyzer {
    pub async fn analyze_access_patterns(&self) -> Result<Vec<FrequentFact>> {
        // Query database for fact access logs
        let access_logs = self.database.get_fact_access_logs(30).await?;

        // Count access frequency
        let mut frequency_map = HashMap::new();
        for log in access_logs {
            *frequency_map.entry(log.fact_id).or_insert(0) += 1;
        }

        // Filter high-frequency facts (top 10%)
        let threshold = self.calculate_threshold(&frequency_map);
        let frequent: Vec<_> = frequency_map
            .into_iter()
            .filter(|(_, count)| *count >= threshold)
            .collect();

        Ok(frequent)
    }
}
```

#### Step 3: Implement profile generation

```rust
impl ProfileGenerator {
    pub async fn generate_profile(&self, frequent_facts: Vec<MemoryFact>) -> Result<UserProfile> {
        // Group facts by category using LLM
        let categories = self.categorize_facts(&frequent_facts).await?;

        // For each category, consolidate similar facts
        let mut profile_categories = HashMap::new();
        for (category_name, facts) in categories {
            let consolidated = self.consolidate_category(facts).await?;
            profile_categories.insert(category_name, consolidated);
        }

        Ok(UserProfile {
            profile_id: uuid::Uuid::new_v4().to_string(),
            categories: profile_categories,
            created_at: now_timestamp(),
            updated_at: now_timestamp(),
        })
    }
}
```

---

## Task 4: Implement Semantic Chunking

**Goal:** Improve chunking by using semantic boundaries instead of sentence boundaries.

**Concept:**
Use embeddings to detect semantic shifts in text, creating chunks that preserve semantic coherence.

**Files:**
- Modify: `core/src/memory/transcript_indexer/indexer.rs`
- Create: `core/src/memory/transcript_indexer/semantic_chunker.rs`

### Implementation Steps

#### Step 1: Implement semantic similarity detection

```rust
pub struct SemanticChunker {
    embedder: Arc<SmartEmbedder>,
    config: SemanticChunkConfig,
}

pub struct SemanticChunkConfig {
    pub similarity_threshold: f32,  // Default: 0.85
    pub min_chunk_size: usize,      // Default: 50 tokens
    pub max_chunk_size: usize,      // Default: 400 tokens
}

impl SemanticChunker {
    pub async fn chunk(&self, text: &str) -> Result<Vec<String>> {
        // Split into sentences
        let sentences = self.split_sentences(text);

        // Generate embeddings for each sentence
        let embeddings = self.embed_sentences(&sentences).await?;

        // Detect semantic boundaries
        let boundaries = self.detect_boundaries(&embeddings);

        // Create chunks based on boundaries
        let chunks = self.create_chunks(&sentences, &boundaries);

        Ok(chunks)
    }

    fn detect_boundaries(&self, embeddings: &[Vec<f32>]) -> Vec<usize> {
        let mut boundaries = vec![0];

        for i in 1..embeddings.len() {
            let similarity = cosine_similarity(&embeddings[i-1], &embeddings[i]);

            if similarity < self.config.similarity_threshold {
                boundaries.push(i);
            }
        }

        boundaries.push(embeddings.len());
        boundaries
    }
}
```

---

## Task 5: Implement LLM-based Importance Scoring

**Goal:** Use LLM to provide more accurate importance scores than keyword-based detection.

**Concept:**
Send memory content to LLM with a scoring prompt, getting nuanced importance assessment.

**Files:**
- Create: `core/src/memory/value_estimator/llm_scorer.rs`
- Modify: `core/src/memory/value_estimator/estimator.rs`

### Implementation Steps

#### Step 1: Define LLM scorer

```rust
pub struct LlmScorer {
    provider: Arc<dyn AiProvider>,
    config: LlmScorerConfig,
}

pub struct LlmScorerConfig {
    pub model: String,
    pub temperature: f32,
    pub use_cache: bool,
}

impl LlmScorer {
    pub async fn score(&self, entry: &MemoryEntry) -> Result<f32> {
        let prompt = format!(
            "Rate the importance of this conversation on a scale of 0.0 to 1.0:\n\
             User: {}\n\
             Assistant: {}\n\n\
             Consider:\n\
             - Personal information (high importance)\n\
             - Preferences and decisions (high importance)\n\
             - Factual knowledge (medium importance)\n\
             - Greetings and small talk (low importance)\n\n\
             Respond with only a number between 0.0 and 1.0.",
            entry.user_input,
            entry.ai_output
        );

        let response = self.provider.complete(&prompt).await?;
        let score: f32 = response.trim().parse()?;

        Ok(score.clamp(0.0, 1.0))
    }
}
```

#### Step 2: Integrate with ValueEstimator

```rust
impl ValueEstimator {
    pub async fn estimate_with_llm(&self, entry: &MemoryEntry) -> Result<f32> {
        // Get keyword-based score
        let keyword_score = self.estimate(entry).await?;

        // Get LLM score if available
        if let Some(llm_scorer) = &self.llm_scorer {
            let llm_score = llm_scorer.score(entry).await?;

            // Weighted average (70% LLM, 30% keyword)
            Ok(llm_score * 0.7 + keyword_score * 0.3)
        } else {
            Ok(keyword_score)
        }
    }
}
```

---

## Completion Checklist

- [ ] RippleTask implemented and tested
- [ ] Fact Evolution Chain implemented and tested
- [ ] ConsolidationTask implemented and tested
- [ ] Semantic chunking implemented and tested
- [ ] LLM-based scoring implemented and tested
- [ ] Integration tests pass
- [ ] Documentation updated
- [ ] All commits follow conventional format

---

## Notes

- Phase 3 features are complex and require careful integration
- LLM-based features will increase API costs
- Consider caching LLM responses for repeated queries
- Evolution chains should be queryable for debugging
- User profiles should be exportable for transparency
