# Memory System Optimization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance Aleph's memory system by integrating key features from memory-lancedb-pro: RRF fusion, cross-encoder rerank, query expansion, retrieval trace, tiered decay, access reinforcement, tier promotion, session-end reflection, and Panel UI integration.

**Architecture:** Four-phase modular enhancement. Each phase builds on stable existing infrastructure (LanceDB backend, scoring pipeline, DreamDaemon). New modules are added alongside existing ones; refactored modules preserve backward compatibility via `#[serde(default)]`.

**Tech Stack:** Rust (core), Leptos/WASM (Panel UI), LanceDB (storage), JSON-RPC over WebSocket (API)

---

## Phase 1: Retrieval Enhancement

### Task 1: Retrieval Trace Infrastructure

**Files:**
- Create: `src/memory/retrieval_trace.rs`
- Modify: `src/memory/mod.rs` (add `pub mod retrieval_trace;`)
- Test: inline `#[cfg(test)]` module

**Why first:** Every subsequent task instruments trace points. Build the data structure first.

**Step 1: Write the failing test**

In `src/memory/retrieval_trace.rs`:

```rust
//! Retrieval trace for debugging the scoring pipeline.
//!
//! Records per-stage score evolution so the debug panel can visualize
//! how each candidate's score changes through the pipeline.

use serde::{Deserialize, Serialize};

/// Full trace of a single retrieval request.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetrievalTrace {
    pub query: String,
    pub timestamp: i64,
    pub stages: Vec<TraceStage>,
}

/// One stage's snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStage {
    pub name: String,
    pub duration_ms: u64,
    pub input_count: usize,
    pub output_count: usize,
    pub scores: Vec<ScoreSnapshot>,
}

/// A single candidate's score at a given stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreSnapshot {
    pub fact_id: String,
    pub score: f32,
    pub rank: usize,
}

impl RetrievalTrace {
    pub fn new(query: &str, timestamp: i64) -> Self {
        Self {
            query: query.to_string(),
            timestamp,
            stages: Vec::new(),
        }
    }

    /// Record a stage result. Call after each pipeline stage.
    pub fn record_stage(
        &mut self,
        name: &str,
        duration_ms: u64,
        input_count: usize,
        scored_facts: &[(String, f32)],
    ) {
        let scores: Vec<ScoreSnapshot> = scored_facts
            .iter()
            .enumerate()
            .map(|(rank, (id, score))| ScoreSnapshot {
                fact_id: id.clone(),
                score: *score,
                rank: rank + 1,
            })
            .collect();

        self.stages.push(TraceStage {
            name: name.to_string(),
            duration_ms,
            input_count,
            output_count: scores.len(),
            scores,
        });
    }

    /// Total pipeline duration in ms.
    pub fn total_duration_ms(&self) -> u64 {
        self.stages.iter().map(|s| s.duration_ms).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_records_stages() {
        let mut trace = RetrievalTrace::new("test query", 1700000000);
        assert!(trace.stages.is_empty());

        trace.record_stage(
            "vector_search",
            23,
            0,
            &[
                ("fact-1".into(), 0.89),
                ("fact-2".into(), 0.76),
            ],
        );

        assert_eq!(trace.stages.len(), 1);
        assert_eq!(trace.stages[0].name, "vector_search");
        assert_eq!(trace.stages[0].output_count, 2);
        assert_eq!(trace.stages[0].scores[0].rank, 1);
        assert_eq!(trace.stages[0].scores[1].fact_id, "fact-2");
    }

    #[test]
    fn total_duration_sums_stages() {
        let mut trace = RetrievalTrace::new("q", 0);
        trace.record_stage("a", 10, 0, &[]);
        trace.record_stage("b", 20, 0, &[]);
        assert_eq!(trace.total_duration_ms(), 30);
    }
}
```

**Step 2: Register module and run test**

Add `pub mod retrieval_trace;` to `src/memory/mod.rs`.

Run: `cargo test -p alephcore --lib retrieval_trace -- -v`
Expected: PASS (2 tests)

**Step 3: Commit**

```bash
git add src/memory/retrieval_trace.rs src/memory/mod.rs
git commit -m "memory: add RetrievalTrace infrastructure for pipeline debugging"
```

---

### Task 2: RRF Fusion Strategy

**Files:**
- Create: `src/memory/hybrid_retrieval/fusion.rs`
- Modify: `src/memory/hybrid_retrieval/mod.rs` (add `pub mod fusion;`)
- Modify: `src/memory/hybrid_retrieval/hybrid.rs` (use new fusion in search)
- Test: inline `#[cfg(test)]` module in `fusion.rs`

**Step 1: Write fusion module with tests**

Create `src/memory/hybrid_retrieval/fusion.rs`:

```rust
//! Score fusion strategies for hybrid retrieval.
//!
//! Supports two fusion modes:
//! - **Weighted**: `combined = vector_weight * vec_score + text_weight * text_score`
//! - **RRF**: Reciprocal Rank Fusion — `score(d) = Σ 1/(k + rank_i(d))`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fusion strategy selector.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FusionStrategy {
    /// Reciprocal Rank Fusion (recommended).
    #[default]
    Rrf,
    /// Legacy weighted sum.
    Weighted,
}

/// A candidate with its source scores.
#[derive(Debug, Clone)]
pub struct FusionCandidate {
    pub id: String,
    pub vector_score: Option<f32>,
    pub text_score: Option<f32>,
}

/// Fused result.
#[derive(Debug, Clone)]
pub struct FusedScore {
    pub id: String,
    pub score: f32,
}

/// RRF constant (standard value from literature).
const DEFAULT_RRF_K: u32 = 60;

/// Extra BM25 bonus (aligned with memory-lancedb-pro).
const DEFAULT_BM25_BONUS: f32 = 0.15;

/// Fuse vector and BM25 results using RRF.
///
/// 1. Rank each result list by its native score (descending).
/// 2. For each document: `rrf_score = Σ 1/(k + rank)`.
/// 3. Apply BM25 bonus for docs that appear in text results.
/// 4. Normalize to [0, 1].
pub fn rrf_fuse(
    vector_results: &[(String, f32)],
    text_results: &[(String, f32)],
    k: u32,
    bm25_bonus: f32,
) -> Vec<FusedScore> {
    let k = k as f32;
    let mut scores: HashMap<String, f32> = HashMap::new();
    let mut in_text: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Vector ranks (already sorted descending by score)
    for (rank, (id, _score)) in vector_results.iter().enumerate() {
        *scores.entry(id.clone()).or_default() += 1.0 / (k + rank as f32 + 1.0);
    }

    // Text/BM25 ranks
    for (rank, (id, _score)) in text_results.iter().enumerate() {
        *scores.entry(id.clone()).or_default() += 1.0 / (k + rank as f32 + 1.0);
        in_text.insert(id.clone());
    }

    // BM25 bonus
    if bm25_bonus > 0.0 {
        for (id, score) in scores.iter_mut() {
            if in_text.contains(id) {
                *score *= 1.0 + bm25_bonus;
            }
        }
    }

    // Normalize to [0, 1]
    let max_score = scores.values().cloned().fold(0.0_f32, f32::max);
    let mut fused: Vec<FusedScore> = scores
        .into_iter()
        .map(|(id, s)| FusedScore {
            id,
            score: if max_score > 0.0 { s / max_score } else { 0.0 },
        })
        .collect();

    fused.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    fused
}

/// Fuse using weighted sum (legacy).
pub fn weighted_fuse(
    vector_results: &[(String, f32)],
    text_results: &[(String, f32)],
    vector_weight: f32,
    text_weight: f32,
) -> Vec<FusedScore> {
    let mut scores: HashMap<String, (f32, f32)> = HashMap::new();

    for (id, score) in vector_results {
        scores.entry(id.clone()).or_default().0 = *score;
    }
    for (id, score) in text_results {
        scores.entry(id.clone()).or_default().1 = *score;
    }

    let mut fused: Vec<FusedScore> = scores
        .into_iter()
        .map(|(id, (vs, ts))| FusedScore {
            id,
            score: vector_weight * vs + text_weight * ts,
        })
        .collect();

    fused.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    fused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_fuse_combines_both_sources() {
        let vec_results = vec![
            ("a".into(), 0.9_f32),
            ("b".into(), 0.7),
            ("c".into(), 0.5),
        ];
        let text_results = vec![
            ("b".into(), 0.8_f32),
            ("d".into(), 0.6),
            ("a".into(), 0.4),
        ];

        let fused = rrf_fuse(&vec_results, &text_results, 60, 0.15);

        // "a" and "b" appear in both → should rank highest
        assert!(fused.len() >= 2);
        let top_ids: Vec<&str> = fused.iter().take(2).map(|f| f.id.as_str()).collect();
        assert!(top_ids.contains(&"a") || top_ids.contains(&"b"));

        // All scores normalized to [0, 1]
        assert!(fused.iter().all(|f| f.score >= 0.0 && f.score <= 1.0));
        // First score should be 1.0 (max after normalization)
        assert!((fused[0].score - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn rrf_fuse_empty_text_returns_vector_only() {
        let vec_results = vec![("a".into(), 0.9_f32), ("b".into(), 0.7)];
        let fused = rrf_fuse(&vec_results, &[], 60, 0.15);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].id, "a");
    }

    #[test]
    fn weighted_fuse_applies_weights() {
        let vec_results = vec![("a".into(), 1.0_f32)];
        let text_results = vec![("a".into(), 0.5_f32)];
        let fused = weighted_fuse(&vec_results, &text_results, 0.7, 0.3);
        // 0.7 * 1.0 + 0.3 * 0.5 = 0.85
        assert!((fused[0].score - 0.85).abs() < 0.01);
    }

    #[test]
    fn bm25_bonus_boosts_text_hits() {
        let vec_results = vec![("a".into(), 0.9_f32), ("b".into(), 0.8)];
        let text_results = vec![("b".into(), 0.9_f32)];

        let fused = rrf_fuse(&vec_results, &text_results, 60, 0.15);

        // "b" has both vector + text + bm25 bonus → should outrank "a" (vector only)
        assert_eq!(fused[0].id, "b");
    }
}
```

**Step 2: Register module**

Add `pub mod fusion;` to `src/memory/hybrid_retrieval/mod.rs`.

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib fusion -- -v`
Expected: PASS (4 tests)

**Step 4: Commit**

```bash
git add src/memory/hybrid_retrieval/fusion.rs src/memory/hybrid_retrieval/mod.rs
git commit -m "memory: add RRF and weighted fusion strategies for hybrid retrieval"
```

---

### Task 3: Cross-Encoder Rerank Module

**Files:**
- Create: `src/memory/rerank/mod.rs`
- Create: `src/memory/rerank/provider.rs`
- Create: `src/memory/rerank/jina.rs`
- Create: `src/memory/rerank/siliconflow.rs`
- Create: `src/memory/rerank/voyage.rs`
- Create: `src/memory/rerank/pinecone.rs`
- Create: `src/memory/rerank/vllm.rs`
- Modify: `src/memory/mod.rs` (add `pub mod rerank;`)
- Test: inline tests in `mod.rs`

**Step 1: Write the RerankProvider trait and config types**

Create `src/memory/rerank/provider.rs`:

```rust
//! Cross-encoder rerank provider trait.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AlephError;

/// Result of reranking a single document.
#[derive(Debug, Clone)]
pub struct RerankResult {
    /// Index into the original documents slice.
    pub index: usize,
    /// Relevance score from the cross-encoder (0.0 to 1.0).
    pub relevance_score: f32,
}

/// Cross-encoder rerank provider.
#[async_trait]
pub trait RerankProvider: Send + Sync {
    /// Rerank `documents` against `query`, returning the top-n results
    /// sorted by relevance (descending).
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError>;

    /// Provider identifier for logging / tracing.
    fn provider_id(&self) -> &str;
}

/// Which rerank provider to use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RerankProviderType {
    #[default]
    Jina,
    SiliconFlow,
    Voyage,
    Pinecone,
    Vllm,
}

/// Rerank configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankConfig {
    /// Whether cross-encoder reranking is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Which provider to use.
    #[serde(default)]
    pub provider: RerankProviderType,
    /// API base URL.
    #[serde(default)]
    pub api_base: String,
    /// API key (empty for vLLM local).
    #[serde(default)]
    pub api_key: String,
    /// Model name.
    #[serde(default = "default_rerank_model")]
    pub model: String,
    /// Timeout in milliseconds.
    #[serde(default = "default_rerank_timeout")]
    pub timeout_ms: u64,
    /// Weight for rerank score in final blend (vs original score).
    #[serde(default = "default_rerank_weight")]
    pub rerank_weight: f32,
}

fn default_rerank_model() -> String { "BAAI/bge-reranker-v2-m3".to_string() }
fn default_rerank_timeout() -> u64 { 5000 }
fn default_rerank_weight() -> f32 { 0.6 }

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: RerankProviderType::default(),
            api_base: String::new(),
            api_key: String::new(),
            model: default_rerank_model(),
            timeout_ms: default_rerank_timeout(),
            rerank_weight: default_rerank_weight(),
        }
    }
}
```

**Step 2: Write provider implementations**

Each provider follows the same HTTP pattern. Create `src/memory/rerank/jina.rs` as the reference:

```rust
//! Jina rerank provider.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

use crate::error::AlephError;
use super::provider::{RerankProvider, RerankResult, RerankConfig};

pub struct JinaRerankProvider {
    client: Client,
    config: RerankConfig,
}

impl JinaRerankProvider {
    pub fn new(config: RerankConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .unwrap_or_default();
        Self { client, config }
    }
}

#[async_trait]
impl RerankProvider for JinaRerankProvider {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError> {
        let body = json!({
            "model": self.config.model,
            "query": query,
            "documents": documents,
            "top_n": top_n,
        });

        let resp = self.client
            .post(&self.config.api_base)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AlephError::Internal(format!("Jina rerank request failed: {e}")))?;

        let data: Value = resp
            .json()
            .await
            .map_err(|e| AlephError::Internal(format!("Jina rerank parse failed: {e}")))?;

        parse_jina_response(&data)
    }

    fn provider_id(&self) -> &str { "jina" }
}

fn parse_jina_response(data: &Value) -> Result<Vec<RerankResult>, AlephError> {
    let results = data["results"]
        .as_array()
        .ok_or_else(|| AlephError::Internal("Missing results array".into()))?;

    let mut out: Vec<RerankResult> = results
        .iter()
        .filter_map(|r| {
            Some(RerankResult {
                index: r["index"].as_u64()? as usize,
                relevance_score: r["relevance_score"].as_f64()? as f32,
            })
        })
        .collect();

    out.sort_by(|a, b| b.relevance_score.partial_cmp(&a.relevance_score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}
```

Create similar files for the other 4 providers. Key differences per provider:

- **`siliconflow.rs`**: Same as Jina (identical API format). `provider_id() = "siliconflow"`.
- **`voyage.rs`**: Uses `top_k` not `top_n` in request body. Response: `data[].relevance_score`. `provider_id() = "voyage"`.
- **`pinecone.rs`**: Header `Api-Key` (not Authorization Bearer). Documents as `[{text: "..."}]`. Response: `data[].score`. `provider_id() = "pinecone"`.
- **`vllm.rs`**: No auth header. Local Docker endpoint. Response: `results[].relevance_score`. `provider_id() = "vllm"`.

**Step 3: Write mod.rs with factory and pipeline integration**

Create `src/memory/rerank/mod.rs`:

```rust
//! Cross-encoder reranking module.
//!
//! Supports 5 providers: Jina, SiliconFlow, Voyage, Pinecone, vLLM.
//! Falls back to cosine similarity rerank on API failure.

pub mod provider;
pub mod jina;
pub mod siliconflow;
pub mod voyage;
pub mod pinecone;
pub mod vllm;

use provider::{RerankConfig, RerankProvider, RerankProviderType, RerankResult};
use crate::error::AlephError;

/// Build the configured rerank provider.
pub fn build_provider(config: &RerankConfig) -> Box<dyn RerankProvider> {
    match config.provider {
        RerankProviderType::Jina => Box::new(jina::JinaRerankProvider::new(config.clone())),
        RerankProviderType::SiliconFlow => Box::new(siliconflow::SiliconFlowRerankProvider::new(config.clone())),
        RerankProviderType::Voyage => Box::new(voyage::VoyageRerankProvider::new(config.clone())),
        RerankProviderType::Pinecone => Box::new(pinecone::PineconeRerankProvider::new(config.clone())),
        RerankProviderType::Vllm => Box::new(vllm::VllmRerankProvider::new(config.clone())),
    }
}

/// Blend rerank scores with original scores.
///
/// `final = rerank_weight * rerank_score + (1 - rerank_weight) * original_score`
pub fn blend_scores(
    originals: &[(String, f32)],
    reranked: &[RerankResult],
    rerank_weight: f32,
) -> Vec<(String, f32)> {
    let original_weight = 1.0 - rerank_weight;

    let mut blended: Vec<(String, f32)> = originals
        .iter()
        .map(|(id, orig_score)| {
            let rerank_score = reranked
                .iter()
                .find(|r| r.index < originals.len() && originals[r.index].0 == *id)
                .map(|r| r.relevance_score)
                .unwrap_or(0.0);

            let final_score = rerank_weight * rerank_score + original_weight * orig_score;
            (id.clone(), final_score)
        })
        .collect();

    blended.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    blended
}
```

**Step 4: Register and test**

Add `pub mod rerank;` to `src/memory/mod.rs`.

Run: `cargo check -p alephcore`
Expected: PASS (compile check)

**Step 5: Commit**

```bash
git add src/memory/rerank/
git commit -m "memory: add cross-encoder rerank module with 5 providers"
```

---

### Task 4: Query Expander

**Files:**
- Create: `src/memory/query_expander.rs`
- Modify: `src/memory/mod.rs` (add `pub mod query_expander;`)
- Test: inline `#[cfg(test)]`

**Step 1: Write module with tests**

Create `src/memory/query_expander.rs`:

```rust
//! Query expansion for improving BM25 recall.
//!
//! Detects Chinese queries and injects common synonyms to broaden
//! full-text search coverage. Vector search uses the original query.

use std::collections::HashMap;
use once_cell::sync::Lazy;

/// Expanded query for hybrid retrieval.
#[derive(Debug, Clone)]
pub struct ExpandedQuery {
    /// Original user query (used for vector search).
    pub original: String,
    /// Expanded query with synonyms (used for BM25/FTS).
    pub bm25_query: String,
}

/// Built-in Chinese synonym pairs.
static SYNONYMS: Lazy<HashMap<&str, &[&str]>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("喜欢", &["偏好", "倾向", "爱好"][..]);
    m.insert("偏好", &["喜欢", "倾向"][..]);
    m.insert("设置", &["配置", "调整", "修改"][..]);
    m.insert("配置", &["设置", "调整"][..]);
    m.insert("问题", &["bug", "错误", "故障", "缺陷"][..]);
    m.insert("错误", &["问题", "bug", "故障"][..]);
    m.insert("使用", &["用", "利用", "采用"][..]);
    m.insert("代码", &["程序", "源码"][..]);
    m.insert("记忆", &["记得", "记住", "回忆"][..]);
    m.insert("之前", &["以前", "上次", "过去"][..]);
    m.insert("工作", &["项目", "任务"][..]);
    m.insert("学习", &["了解", "掌握"][..]);
    m
});

/// Check if a string contains CJK characters.
fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
}

/// Expand a query for BM25 search.
///
/// If the query contains Chinese, inject synonyms.
/// Returns the original query unchanged for vector search.
pub fn expand(query: &str) -> ExpandedQuery {
    if !contains_cjk(query) {
        return ExpandedQuery {
            original: query.to_string(),
            bm25_query: query.to_string(),
        };
    }

    let mut expansions: Vec<&str> = Vec::new();

    for (keyword, synonyms) in SYNONYMS.iter() {
        if query.contains(keyword) {
            expansions.extend_from_slice(synonyms);
        }
    }

    let bm25_query = if expansions.is_empty() {
        query.to_string()
    } else {
        format!("{} {}", query, expansions.join(" "))
    };

    ExpandedQuery {
        original: query.to_string(),
        bm25_query,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_query_unchanged() {
        let result = expand("What does the user prefer?");
        assert_eq!(result.original, result.bm25_query);
    }

    #[test]
    fn chinese_query_expands_synonyms() {
        let result = expand("用户喜欢什么编程语言");
        assert_ne!(result.original, result.bm25_query);
        assert!(result.bm25_query.contains("偏好"));
    }

    #[test]
    fn chinese_without_known_keywords_unchanged() {
        let result = expand("天气很好");
        assert_eq!(result.original, result.bm25_query);
    }

    #[test]
    fn original_always_preserved() {
        let result = expand("我喜欢使用Rust");
        assert_eq!(result.original, "我喜欢使用Rust");
        assert!(result.bm25_query.starts_with("我喜欢使用Rust"));
    }
}
```

**Step 2: Register module**

Add `pub mod query_expander;` to `src/memory/mod.rs`.

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib query_expander -- -v`
Expected: PASS (4 tests)

**Step 4: Commit**

```bash
git add src/memory/query_expander.rs src/memory/mod.rs
git commit -m "memory: add Chinese-aware query expander for BM25 recall improvement"
```

---

### Task 5: Integrate RRF + Rerank + Trace into Hybrid Retrieval

**Files:**
- Modify: `src/memory/hybrid_retrieval/hybrid.rs` (use RRF fusion, plug in rerank, thread trace)
- Modify: `src/memory/scoring_pipeline/mod.rs` (accept `Option<&mut RetrievalTrace>` in `run()`)
- Modify: `src/memory/scoring_pipeline/stages/mod.rs` (extend ScoringStage trait)
- Modify: `src/config/types/memory.rs` (add new config fields)
- Test: update existing tests

**Step 1: Add config fields to MemoryConfig**

In `src/config/types/memory.rs`, add to `MemoryConfig` struct:

```rust
// --- Phase 1: Retrieval Enhancement ---

/// Fusion strategy for hybrid retrieval.
#[serde(default)]
pub fusion_strategy: crate::memory::hybrid_retrieval::fusion::FusionStrategy,

/// RRF constant k (only used when fusion_strategy = Rrf).
#[serde(default = "default_rrf_k")]
pub rrf_k: u32,

/// Extra BM25 bonus weight for RRF fusion.
#[serde(default = "default_bm25_bonus")]
pub bm25_bonus_weight: f32,

/// Whether query expansion is enabled.
#[serde(default)]
pub query_expansion_enabled: bool,

/// Cross-encoder reranking configuration.
#[serde(default)]
pub rerank: crate::memory::rerank::provider::RerankConfig,
```

Add default functions:
```rust
fn default_rrf_k() -> u32 { 60 }
fn default_bm25_bonus() -> f32 { 0.15 }
```

**Step 2: Extend ScoringStage trait for trace support**

Modify `src/memory/scoring_pipeline/stages/mod.rs` — add an optional trace-aware method with a default impl so existing stages don't break:

```rust
pub trait ScoringStage: Send + Sync {
    fn name(&self) -> &str;
    fn apply(&self, candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact>;
}
```

**Do NOT change the trait signature.** Instead, instrument trace recording in `ScoringPipeline::run()`.

**Step 3: Add trace support to ScoringPipeline::run()**

In `src/memory/scoring_pipeline/mod.rs`, add a new method:

```rust
/// Run all stages with optional trace recording.
pub fn run_traced(
    &self,
    candidates: Vec<ScoredFact>,
    ctx: &ScoringContext,
    trace: Option<&mut crate::memory::retrieval_trace::RetrievalTrace>,
) -> Vec<ScoredFact> {
    let mut current = candidates;

    for stage in &self.stages {
        let before = current.len();
        let start = std::time::Instant::now();
        current = stage.apply(current, ctx);
        let elapsed = start.elapsed().as_millis() as u64;

        debug!(
            stage = stage.name(),
            before = before,
            after = current.len(),
            "scoring stage applied"
        );

        if let Some(ref mut t) = trace {
            let snapshots: Vec<(String, f32)> = current
                .iter()
                .map(|sf| (sf.fact.id.to_string(), sf.score))
                .collect();
            t.record_stage(stage.name(), elapsed, before, &snapshots);
        }
    }

    current
}
```

Keep the existing `run()` method delegating to `run_traced(candidates, ctx, None)`.

**Step 4: Wire fusion + query expansion into HybridRetrieval**

In `src/memory/hybrid_retrieval/hybrid.rs`, modify the `search()` method to:
1. Use `query_expander::expand()` when enabled
2. Call `rrf_fuse()` or `weighted_fuse()` based on config
3. Thread trace through pipeline

This is the integration step — exact code depends on the current `search()` implementation shape. Read `hybrid.rs` fully before modifying.

**Step 5: Run all existing tests + new tests**

Run: `cargo test -p alephcore --lib -- -v`
Expected: All existing tests PASS, plus new fusion/trace tests.

**Step 6: Commit**

```bash
git add src/memory/hybrid_retrieval/ src/memory/scoring_pipeline/ src/config/types/memory.rs
git commit -m "memory: integrate RRF fusion, query expansion, and trace into hybrid retrieval pipeline"
```

---

### Task 6: Gateway RPC for Debug Trace

**Files:**
- Modify: `src/gateway/handlers/memory_config.rs` (or create new handler file)
- Test: manual via Panel debug panel (Task in Phase 4)

**Step 1: Add `memory.retrieve_with_trace` handler**

```rust
pub async fn handle_retrieve_with_trace(
    params: Value,
    ctx: &AppContext,
) -> Result<Value, AlephError> {
    let query = params["query"].as_str()
        .ok_or_else(|| AlephError::InvalidInput("missing query".into()))?;

    // Run hybrid retrieval with trace enabled
    let mut trace = RetrievalTrace::new(query, now_unix());
    let results = ctx.memory_service()
        .search_with_trace(query, &mut trace)
        .await?;

    Ok(json!({
        "results": results,
        "trace": trace,
    }))
}
```

**Step 2: Add `memory.test_rerank_connection` handler**

```rust
pub async fn handle_test_rerank_connection(
    params: Value,
    ctx: &AppContext,
) -> Result<Value, AlephError> {
    let config: RerankConfig = serde_json::from_value(params)?;
    let provider = rerank::build_provider(&config);

    let test_docs = vec![
        "test document one".to_string(),
        "test document two".to_string(),
        "test document three".to_string(),
    ];

    let results = provider.rerank("test query", &test_docs, 3).await?;

    Ok(json!({
        "success": true,
        "results_count": results.len(),
    }))
}
```

**Step 3: Register handlers in gateway router**

**Step 4: Commit**

```bash
git add src/gateway/
git commit -m "gateway: add memory.retrieve_with_trace and memory.test_rerank_connection RPCs"
```

---

## Phase 2: Lifecycle Improvements

### Task 7: Tiered Decay Config Types

**Files:**
- Modify: `src/memory/decay.rs` (add `TieredDecayConfig`, `TierDecayParams`, `AccessReinforcementConfig`)
- Modify: `src/config/types/memory.rs` (add `tiered_decay`, `promotion` fields)
- Test: inline `#[cfg(test)]`

**Step 1: Write new types with tests in `decay.rs`**

Add to `src/memory/decay.rs`:

```rust
use crate::memory::context::MemoryTier;

/// Per-tier decay parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierDecayParams {
    #[serde(default = "default_half_life")]
    pub half_life_days: f32,
    #[serde(default = "default_min_strength")]
    pub min_strength: f32,
    #[serde(default)]
    pub reinforcement: AccessReinforcementConfig,
}

/// Access-based half-life reinforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessReinforcementConfig {
    /// Reinforcement factor (default: 0.5).
    #[serde(default = "default_reinforcement_factor")]
    pub factor: f32,
    /// Max multiplier for half-life extension (default: 3.0).
    #[serde(default = "default_max_multiplier")]
    pub max_multiplier: f32,
    /// Days after which access count starts to "expire" (default: 30.0).
    #[serde(default = "default_access_decay_days")]
    pub access_decay_days: f32,
}

/// Tiered decay configuration — replaces flat DecayConfig.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredDecayConfig {
    #[serde(default = "default_core_params")]
    pub core: TierDecayParams,
    #[serde(default = "default_long_term_params")]
    pub long_term: TierDecayParams,
    #[serde(default = "default_short_term_params")]
    pub short_term: TierDecayParams,
    #[serde(default = "default_protected_types")]
    pub protected_types: Vec<FactType>,
}

impl TieredDecayConfig {
    /// Get params for a given tier.
    pub fn params_for_tier(&self, tier: &MemoryTier) -> &TierDecayParams {
        match tier {
            MemoryTier::Core => &self.core,
            MemoryTier::LongTerm => &self.long_term,
            MemoryTier::ShortTerm => &self.short_term,
        }
    }
}

/// Compute effective half-life with access reinforcement.
///
/// Frequent recent access extends the half-life; stale access counts fade.
pub fn effective_half_life(
    base: f32,
    access_count: u32,
    days_since_last_access: f32,
    config: &AccessReinforcementConfig,
) -> f32 {
    let freshness = (-days_since_last_access * std::f32::consts::LN_2 / config.access_decay_days).exp();
    let effective_count = access_count as f32 * freshness;
    let extension = base * config.factor * (1.0 + effective_count).ln();
    (base + extension).min(base * config.max_multiplier)
}

// Default functions
fn default_reinforcement_factor() -> f32 { 0.5 }
fn default_max_multiplier() -> f32 { 3.0 }
fn default_access_decay_days() -> f32 { 30.0 }
fn default_half_life() -> f32 { 30.0 }
fn default_min_strength() -> f32 { 0.1 }

fn default_core_params() -> TierDecayParams {
    TierDecayParams { half_life_days: 90.0, min_strength: 0.05, reinforcement: AccessReinforcementConfig::default() }
}
fn default_long_term_params() -> TierDecayParams {
    TierDecayParams { half_life_days: 45.0, min_strength: 0.1, reinforcement: AccessReinforcementConfig::default() }
}
fn default_short_term_params() -> TierDecayParams {
    TierDecayParams { half_life_days: 7.0, min_strength: 0.15, reinforcement: AccessReinforcementConfig::default() }
}
fn default_protected_types() -> Vec<FactType> {
    vec![FactType::Personal]
}

impl Default for AccessReinforcementConfig {
    fn default() -> Self {
        Self { factor: 0.5, max_multiplier: 3.0, access_decay_days: 30.0 }
    }
}

impl Default for TieredDecayConfig {
    fn default() -> Self {
        Self {
            core: default_core_params(),
            long_term: default_long_term_params(),
            short_term: default_short_term_params(),
            protected_types: default_protected_types(),
        }
    }
}
```

Add tests:

```rust
#[cfg(test)]
mod tiered_tests {
    use super::*;

    #[test]
    fn effective_half_life_no_access() {
        let config = AccessReinforcementConfig::default();
        let result = effective_half_life(7.0, 0, 0.0, &config);
        assert!((result - 7.0).abs() < 0.01, "zero access → base half-life");
    }

    #[test]
    fn effective_half_life_recent_access() {
        let config = AccessReinforcementConfig::default();
        let result = effective_half_life(7.0, 3, 1.0, &config);
        assert!(result > 7.0, "recent access should extend half-life");
        assert!(result < 21.0, "should not exceed max (7 * 3.0)");
    }

    #[test]
    fn effective_half_life_stale_access() {
        let config = AccessReinforcementConfig::default();
        let result = effective_half_life(7.0, 50, 90.0, &config);
        // 90 days since last access → freshness is very low
        assert!(result < 10.0, "stale access should barely extend: got {result}");
    }

    #[test]
    fn effective_half_life_capped() {
        let config = AccessReinforcementConfig::default();
        let result = effective_half_life(7.0, 1000, 0.0, &config);
        assert!((result - 21.0).abs() < 0.01, "should cap at 7 * 3.0 = 21.0, got {result}");
    }

    #[test]
    fn tiered_config_returns_correct_params() {
        let config = TieredDecayConfig::default();
        assert!((config.params_for_tier(&MemoryTier::Core).half_life_days - 90.0).abs() < 0.01);
        assert!((config.params_for_tier(&MemoryTier::ShortTerm).half_life_days - 7.0).abs() < 0.01);
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib tiered_tests -- -v`
Expected: PASS (5 tests)

**Step 3: Commit**

```bash
git add src/memory/decay.rs
git commit -m "memory: add TieredDecayConfig with per-tier params and access reinforcement"
```

---

### Task 8: Wire Tiered Decay into LazyDecayEngine

**Files:**
- Modify: `src/memory/lazy_decay.rs` (use `TieredDecayConfig` + `effective_half_life()`)
- Modify: `src/memory/decay.rs` (update `MemoryStrength::calculate_strength` to accept tier)
- Test: update existing tests

**Step 1: Add tier-aware strength calculation**

Add to `MemoryStrength` in `decay.rs`:

```rust
/// Calculate strength using tiered config.
pub fn calculate_strength_tiered(
    &self,
    tiered_config: &TieredDecayConfig,
    tier: &MemoryTier,
    fact_type: &FactType,
    now: i64,
) -> f32 {
    // Protected types never decay
    if tiered_config.protected_types.contains(fact_type) {
        return 1.0;
    }

    let params = tiered_config.params_for_tier(tier);
    let days_since_access = (now - self.last_accessed) as f32 / 86400.0;

    // Compute effective half-life with access reinforcement
    let eff_hl = effective_half_life(
        params.half_life_days,
        self.access_count,
        days_since_access,
        &params.reinforcement,
    );

    // Exponential decay
    let strength = 0.5_f32.powf(days_since_access / eff_hl);
    strength.min(1.0)
}
```

**Step 2: Update LazyDecayEngine to use tiered config**

In `lazy_decay.rs`, modify `evaluate()` to look up `fact.tier` and call `calculate_strength_tiered()` instead of `calculate_strength_for_type()`.

**Step 3: Run all decay tests**

Run: `cargo test -p alephcore --lib decay -- -v`
Expected: All PASS

**Step 4: Commit**

```bash
git add src/memory/decay.rs src/memory/lazy_decay.rs
git commit -m "memory: wire tiered decay and access reinforcement into LazyDecayEngine"
```

---

### Task 9: Tier Promotion in DreamDaemon

**Files:**
- Create: `src/memory/promotion.rs`
- Modify: `src/memory/mod.rs` (add `pub mod promotion;`)
- Modify: DreamDaemon consolidation logic (integrate promotion check)
- Test: inline `#[cfg(test)]`

**Step 1: Write promotion module**

Create `src/memory/promotion.rs`:

```rust
//! Tier promotion logic for memory facts.
//!
//! Facts that are frequently accessed and survive decay are promoted
//! to higher tiers with longer half-lives.

use serde::{Deserialize, Serialize};
use crate::memory::context::{MemoryFact, MemoryTier};

/// Promotion criteria per transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionRule {
    pub min_access_count: u32,
    pub min_age_days: f32,
    pub min_strength: f32,
}

/// Full promotion configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionCriteria {
    #[serde(default = "default_short_to_long")]
    pub short_to_long: PromotionRule,
    #[serde(default = "default_long_to_core")]
    pub long_to_core: PromotionRule,
}

fn default_short_to_long() -> PromotionRule {
    PromotionRule { min_access_count: 3, min_age_days: 3.0, min_strength: 0.5 }
}
fn default_long_to_core() -> PromotionRule {
    PromotionRule { min_access_count: 10, min_age_days: 30.0, min_strength: 0.7 }
}

impl Default for PromotionCriteria {
    fn default() -> Self {
        Self {
            short_to_long: default_short_to_long(),
            long_to_core: default_long_to_core(),
        }
    }
}

/// Check if a fact is eligible for promotion.
///
/// Returns `Some(new_tier)` if promotion criteria are met.
pub fn check_promotion(
    fact: &MemoryFact,
    strength: f32,
    now: i64,
    criteria: &PromotionCriteria,
) -> Option<MemoryTier> {
    let age_days = (now - fact.created_at) as f32 / 86400.0;

    match fact.tier {
        MemoryTier::ShortTerm => {
            let rule = &criteria.short_to_long;
            if fact.access_count >= rule.min_access_count
                && age_days >= rule.min_age_days
                && strength >= rule.min_strength
            {
                Some(MemoryTier::LongTerm)
            } else {
                None
            }
        }
        MemoryTier::LongTerm => {
            let rule = &criteria.long_to_core;
            if fact.access_count >= rule.min_access_count
                && age_days >= rule.min_age_days
                && strength >= rule.min_strength
            {
                Some(MemoryTier::Core)
            } else {
                None
            }
        }
        MemoryTier::Core => None, // Already at highest tier
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;

    fn make_fact(tier: MemoryTier, access_count: u32, age_days: f32) -> MemoryFact {
        let now = 1700000000_i64;
        let mut fact = MemoryFact::new("test".into(), FactType::Other, vec![]);
        fact.tier = tier;
        fact.access_count = access_count;
        fact.created_at = now - (age_days * 86400.0) as i64;
        fact
    }

    #[test]
    fn short_term_promotes_when_criteria_met() {
        let fact = make_fact(MemoryTier::ShortTerm, 5, 10.0);
        let result = check_promotion(&fact, 0.6, 1700000000, &PromotionCriteria::default());
        assert_eq!(result, Some(MemoryTier::LongTerm));
    }

    #[test]
    fn short_term_stays_when_too_few_accesses() {
        let fact = make_fact(MemoryTier::ShortTerm, 1, 10.0);
        let result = check_promotion(&fact, 0.6, 1700000000, &PromotionCriteria::default());
        assert_eq!(result, None);
    }

    #[test]
    fn short_term_stays_when_too_young() {
        let fact = make_fact(MemoryTier::ShortTerm, 5, 1.0);
        let result = check_promotion(&fact, 0.6, 1700000000, &PromotionCriteria::default());
        assert_eq!(result, None);
    }

    #[test]
    fn core_never_promotes() {
        let fact = make_fact(MemoryTier::Core, 100, 365.0);
        let result = check_promotion(&fact, 1.0, 1700000000, &PromotionCriteria::default());
        assert_eq!(result, None);
    }

    #[test]
    fn long_term_promotes_to_core() {
        let fact = make_fact(MemoryTier::LongTerm, 15, 60.0);
        let result = check_promotion(&fact, 0.8, 1700000000, &PromotionCriteria::default());
        assert_eq!(result, Some(MemoryTier::Core));
    }
}
```

**Step 2: Register module and test**

Add `pub mod promotion;` to `src/memory/mod.rs`.

Run: `cargo test -p alephcore --lib promotion -- -v`
Expected: PASS (5 tests)

**Step 3: Integrate into DreamDaemon**

In the DreamDaemon consolidation cycle, after decay evaluation, iterate valid facts and call `check_promotion()`. For promoted facts, emit `TierTransitioned` event and update fact tier via `MemoryStore::update_fact()`.

**Step 4: Commit**

```bash
git add src/memory/promotion.rs src/memory/mod.rs
git commit -m "memory: add tier promotion logic with DreamDaemon integration"
```

---

## Phase 3: Reflection System

### Task 10: Add Lesson FactType

**Files:**
- Modify: `src/memory/context/enums.rs` (add `Lesson` variant to `FactType`)
- Test: verify existing tests still pass

**Step 1: Add variant**

In `src/memory/context/enums.rs`, add `Lesson` to `FactType` enum:

```rust
pub enum FactType {
    Preference,
    Plan,
    Learning,
    Project,
    Personal,
    Tool,
    /// Lesson learned from experience (symptom → cause → fix).
    Lesson,
    #[default]
    Other,
    SubagentRun,
    SubagentSession,
    SubagentCheckpoint,
    SubagentTranscript,
}
```

Add `"lesson"` to `as_str()` and `from_str_or_other()` match arms.

**Step 2: Run all tests**

Run: `cargo test -p alephcore --lib -- -v`
Expected: All PASS

**Step 3: Commit**

```bash
git add src/memory/context/enums.rs
git commit -m "memory: add Lesson fact type for reflection-extracted lessons"
```

---

### Task 11: Reflection Parser

**Files:**
- Create: `src/memory/reflection/mod.rs`
- Create: `src/memory/reflection/parser.rs`
- Modify: `src/memory/mod.rs` (add `pub mod reflection;`)
- Test: inline `#[cfg(test)]`

**Step 1: Write parser with tests**

Create `src/memory/reflection/parser.rs`:

```rust
//! Parse structured reflection markdown from LLM output.
//!
//! Expected format:
//! ```markdown
//! ## Invariants
//! - item 1
//! - item 2
//!
//! ## Derived
//! - item 1
//!
//! ## Lessons
//! - symptom: cause → fix
//!
//! ## Open Loops
//! - action item
//! ```

/// Parsed reflection output.
#[derive(Debug, Clone, Default)]
pub struct ReflectionOutput {
    pub invariants: Vec<String>,
    pub derived: Vec<String>,
    pub lessons: Vec<LessonItem>,
    pub open_loops: Vec<String>,
}

/// Structured lesson.
#[derive(Debug, Clone)]
pub struct LessonItem {
    pub symptom: String,
    pub cause: String,
    pub resolution: String,
}

/// Parse reflection markdown into structured output.
pub fn parse_reflection(text: &str) -> ReflectionOutput {
    let mut output = ReflectionOutput::default();
    let mut current_section: Option<&str> = None;

    for line in text.lines() {
        let trimmed = line.trim();

        // Detect section headers
        if trimmed.starts_with("## ") {
            let header = trimmed.trim_start_matches("## ").trim().to_lowercase();
            current_section = match header.as_str() {
                "invariants" => Some("invariants"),
                "derived" => Some("derived"),
                "lessons" | "lessons & pitfalls" => Some("lessons"),
                "open loops" | "open loops / next actions" => Some("open_loops"),
                _ => None,
            };
            continue;
        }

        // Extract bullet items
        if let Some(section) = current_section {
            if let Some(item) = trimmed.strip_prefix("- ") {
                let item = item.trim();
                if is_placeholder(item) {
                    continue;
                }

                match section {
                    "invariants" => output.invariants.push(item.to_string()),
                    "derived" => output.derived.push(item.to_string()),
                    "lessons" => output.lessons.push(parse_lesson(item)),
                    "open_loops" => output.open_loops.push(item.to_string()),
                    _ => {}
                }
            }
        }
    }

    output
}

/// Parse a lesson line: "symptom: cause → fix"
fn parse_lesson(line: &str) -> LessonItem {
    // Try "symptom: cause → fix" format
    if let Some((symptom, rest)) = line.split_once(": ") {
        if let Some((cause, fix)) = rest.split_once(" → ").or_else(|| rest.split_once(" -> ")) {
            return LessonItem {
                symptom: symptom.trim().to_string(),
                cause: cause.trim().to_string(),
                resolution: fix.trim().to_string(),
            };
        }
    }

    // Fallback: whole line as symptom
    LessonItem {
        symptom: line.to_string(),
        cause: String::new(),
        resolution: String::new(),
    }
}

/// Check if a line is a placeholder.
fn is_placeholder(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower == "(none)" || lower == "(none captured)" || lower == "none" || s.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_reflection() {
        let text = r#"
## Invariants
- User prefers Rust over Python
- User works on Aleph project

## Derived
- Currently investigating memory optimization
- Referencing memory-lancedb-pro

## Lessons
- UTF-8 slicing panic: used &s[..n] on multi-byte chars → use s.get(..n) instead
- Lock poisoning: unwrap() cascades panics → use unwrap_or_else(|e| e.into_inner())

## Open Loops
- Investigate cross-encoder performance on Chinese queries
- Verify RRF vs weighted fusion A/B test results
"#;

        let output = parse_reflection(text);

        assert_eq!(output.invariants.len(), 2);
        assert_eq!(output.derived.len(), 2);
        assert_eq!(output.lessons.len(), 2);
        assert_eq!(output.open_loops.len(), 2);

        assert_eq!(output.lessons[0].symptom, "UTF-8 slicing panic");
        assert!(output.lessons[0].resolution.contains("s.get(..n)"));
    }

    #[test]
    fn parse_skips_placeholders() {
        let text = "## Invariants\n- (none)\n- Real item\n- (none captured)\n";
        let output = parse_reflection(text);
        assert_eq!(output.invariants.len(), 1);
        assert_eq!(output.invariants[0], "Real item");
    }

    #[test]
    fn parse_lesson_fallback() {
        let lesson = parse_lesson("some unstructured lesson text");
        assert_eq!(lesson.symptom, "some unstructured lesson text");
        assert!(lesson.cause.is_empty());
    }

    #[test]
    fn parse_empty_returns_default() {
        let output = parse_reflection("");
        assert!(output.invariants.is_empty());
        assert!(output.lessons.is_empty());
    }
}
```

**Step 2: Create mod.rs**

Create `src/memory/reflection/mod.rs`:

```rust
//! Session-end reflection system.
//!
//! Extracts structured insights from completed conversations:
//! - Invariants → Core tier (long-term patterns)
//! - Derived → ShortTerm tier (session-specific learnings)
//! - Lessons → LongTerm tier (experience-based fixes)
//! - Open Loops → DreamDaemon follow-up actions

pub mod parser;
```

**Step 3: Register and test**

Add `pub mod reflection;` to `src/memory/mod.rs`.

Run: `cargo test -p alephcore --lib reflection -- -v`
Expected: PASS (4 tests)

**Step 4: Commit**

```bash
git add src/memory/reflection/
git commit -m "memory: add reflection parser for 4-category structured extraction"
```

---

### Task 12: Reflection Prompt Template

**Files:**
- Create: `src/memory/reflection/prompt.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write prompt builder**

Create `src/memory/reflection/prompt.rs`:

```rust
//! Reflection prompt template for session-end LLM call.

/// Build the reflection system prompt.
pub fn reflection_system_prompt() -> &'static str {
    r#"You are a reflection engine. Analyze the conversation and extract structured insights.

Output EXACTLY this markdown format:

## Invariants
- {Durable user preferences, work patterns, identity traits that will hold across sessions}

## Derived
- {New information learned THIS session — temporary context, current task details}

## Lessons
- {symptom}: {root cause} → {fix or prevention strategy}

## Lessons & pitfalls
Use the format above. If none, write: - (none)

## Open Loops
- {Follow-up actions with action verbs: investigate, verify, update, test, check}

Rules:
1. Write in third person ("The user prefers..." not "You prefer...")
2. Be specific and concrete — avoid vague statements
3. Invariants must be TRUE ACROSS SESSIONS, not session-specific
4. Lessons MUST have the symptom: cause → fix format
5. Open Loops MUST start with an action verb
6. If a section has no items, write: - (none)
7. Do NOT repeat facts that are in the ALREADY EXTRACTED list below"#
}

/// Build the reflection user prompt with conversation context.
pub fn reflection_user_prompt(
    conversation_summary: &str,
    already_extracted_facts: &[String],
) -> String {
    let facts_section = if already_extracted_facts.is_empty() {
        "No facts extracted yet.".to_string()
    } else {
        already_extracted_facts
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{}. {}", i + 1, f))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "## ALREADY EXTRACTED (do not repeat)\n{}\n\n## CONVERSATION TO REFLECT ON\n{}",
        facts_section, conversation_summary
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_contains_all_sections() {
        let prompt = reflection_system_prompt();
        assert!(prompt.contains("## Invariants"));
        assert!(prompt.contains("## Derived"));
        assert!(prompt.contains("## Lessons"));
        assert!(prompt.contains("## Open Loops"));
    }

    #[test]
    fn user_prompt_includes_facts() {
        let prompt = reflection_user_prompt(
            "User asked about memory optimization",
            &["User prefers Rust".to_string()],
        );
        assert!(prompt.contains("User prefers Rust"));
        assert!(prompt.contains("ALREADY EXTRACTED"));
    }

    #[test]
    fn user_prompt_handles_empty_facts() {
        let prompt = reflection_user_prompt("conversation", &[]);
        assert!(prompt.contains("No facts extracted yet"));
    }
}
```

**Step 2: Register in mod.rs**

Add `pub mod prompt;` to `src/memory/reflection/mod.rs`.

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib reflection -- -v`
Expected: PASS (7 tests total: 4 parser + 3 prompt)

**Step 4: Commit**

```bash
git add src/memory/reflection/prompt.rs src/memory/reflection/mod.rs
git commit -m "memory: add reflection prompt templates for session-end LLM call"
```

---

### Task 13: Reflection Mapper (Parse → MemoryFact)

**Files:**
- Create: `src/memory/reflection/mapper.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write mapper**

Create `src/memory/reflection/mapper.rs`:

```rust
//! Map parsed reflection output to MemoryFact entries.

use crate::memory::context::{
    FactType, MemoryCategory, MemoryFact, MemoryLayer, MemoryScope, MemoryTier,
};
use crate::memory::context::enums::FactSource;
use super::parser::{ReflectionOutput, LessonItem};

/// Map reflection output to facts ready for storage.
pub fn map_to_facts(output: &ReflectionOutput) -> Vec<MemoryFact> {
    let mut facts = Vec::new();

    // Invariants → Core tier
    for item in &output.invariants {
        let mut fact = MemoryFact::new(
            item.clone(),
            classify_invariant(item),
            vec![],
        );
        fact.tier = MemoryTier::Core;
        fact.confidence = 0.85;
        fact.layer = MemoryLayer::L1Overview;
        fact.fact_source = FactSource::Reflection;
        facts.push(fact);
    }

    // Derived → ShortTerm tier
    for item in &output.derived {
        let mut fact = MemoryFact::new(
            item.clone(),
            FactType::Other,
            vec![],
        );
        fact.tier = MemoryTier::ShortTerm;
        fact.confidence = 0.70;
        fact.layer = MemoryLayer::L2Detail;
        fact.fact_source = FactSource::Reflection;
        facts.push(fact);
    }

    // Lessons → LongTerm tier
    for lesson in &output.lessons {
        let content = format_lesson(lesson);
        let mut fact = MemoryFact::new(
            content,
            FactType::Lesson,
            vec![],
        );
        fact.tier = MemoryTier::LongTerm;
        fact.confidence = 0.80;
        fact.layer = MemoryLayer::L1Overview;
        fact.category = MemoryCategory::Cases;
        fact.fact_source = FactSource::Reflection;
        facts.push(fact);
    }

    facts
}

/// Classify invariant into Preference or Personal based on content.
fn classify_invariant(text: &str) -> FactType {
    let lower = text.to_lowercase();
    if lower.contains("prefer") || lower.contains("like") || lower.contains("偏好") || lower.contains("喜欢") {
        FactType::Preference
    } else {
        FactType::Personal
    }
}

/// Format a lesson as a single-line fact.
fn format_lesson(lesson: &LessonItem) -> String {
    if lesson.cause.is_empty() {
        lesson.symptom.clone()
    } else {
        format!("{}: {} → {}", lesson.symptom, lesson.cause, lesson.resolution)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::reflection::parser::parse_reflection;

    #[test]
    fn maps_all_categories() {
        let text = r#"
## Invariants
- User prefers dark mode

## Derived
- Working on memory optimization

## Lessons
- Panic on slice: byte indexing → use char_indices

## Open Loops
- Check performance
"#;
        let parsed = parse_reflection(text);
        let facts = map_to_facts(&parsed);

        assert_eq!(facts.len(), 3); // Open Loops not mapped

        // Invariant → Core
        assert_eq!(facts[0].tier, MemoryTier::Core);
        assert_eq!(facts[0].fact_type, FactType::Preference);
        assert!((facts[0].confidence - 0.85).abs() < 0.01);

        // Derived → ShortTerm
        assert_eq!(facts[1].tier, MemoryTier::ShortTerm);

        // Lesson → LongTerm
        assert_eq!(facts[2].tier, MemoryTier::LongTerm);
        assert_eq!(facts[2].fact_type, FactType::Lesson);
    }

    #[test]
    fn empty_reflection_produces_no_facts() {
        let parsed = parse_reflection("");
        let facts = map_to_facts(&parsed);
        assert!(facts.is_empty());
    }
}
```

**Note:** This references `FactSource::Reflection` — check if `FactSource` enum exists in `enums.rs`. If not, add a `Reflection` variant. Similarly ensure `MemoryFact` has a `fact_source` field. Adapt field names to match actual codebase.

**Step 2: Register and test**

Add `pub mod mapper;` to `src/memory/reflection/mod.rs`.

Run: `cargo test -p alephcore --lib reflection -- -v`
Expected: PASS (9 tests total)

**Step 3: Commit**

```bash
git add src/memory/reflection/mapper.rs src/memory/reflection/mod.rs
git commit -m "memory: add reflection mapper — parsed output to MemoryFact with tier/type mapping"
```

---

### Task 14: ReflectionService + Gate + DreamDaemon Integration

**Files:**
- Create: `src/memory/reflection/service.rs`
- Modify: `src/memory/reflection/mod.rs` (add `pub mod service;`)
- Modify: `src/config/types/memory.rs` (add `ReflectionConfig`)
- Modify: DreamDaemon / agent loop end hook (trigger reflection)
- Test: inline `#[cfg(test)]`

**Step 1: Write ReflectionConfig**

Add to `src/config/types/memory.rs`:

```rust
/// Session-end reflection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_reflection_min_turns")]
    pub min_turns: u32,
    #[serde(default = "default_reflection_min_chars")]
    pub min_user_chars: u32,
    #[serde(default = "default_reflection_cooldown")]
    pub cooldown_minutes: u32,
    #[serde(default)]
    pub open_loop_tracking: bool,
    #[serde(default)]
    pub open_loop_inject_prompt: bool,
}

fn default_reflection_min_turns() -> u32 { 5 }
fn default_reflection_min_chars() -> u32 { 200 }
fn default_reflection_cooldown() -> u32 { 30 }

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_turns: 5,
            min_user_chars: 200,
            cooldown_minutes: 30,
            open_loop_tracking: false,
            open_loop_inject_prompt: false,
        }
    }
}
```

Add `pub reflection: ReflectionConfig` to `MemoryConfig` with `#[serde(default)]`.

**Step 2: Write ReflectionService**

Create `src/memory/reflection/service.rs`:

```rust
//! ReflectionService orchestrates session-end reflection.

use crate::config::types::memory::ReflectionConfig;
use crate::error::AlephError;
use super::parser::parse_reflection;
use super::mapper::map_to_facts;
use super::prompt::{reflection_system_prompt, reflection_user_prompt};
use crate::memory::context::MemoryFact;

/// Gate check: should we run reflection for this session?
pub fn should_reflect(
    turn_count: u32,
    total_user_chars: u32,
    last_reflection_minutes_ago: Option<u32>,
    config: &ReflectionConfig,
) -> bool {
    if !config.enabled {
        return false;
    }
    if turn_count < config.min_turns {
        return false;
    }
    if total_user_chars < config.min_user_chars {
        return false;
    }
    if let Some(mins) = last_reflection_minutes_ago {
        if mins < config.cooldown_minutes {
            return false;
        }
    }
    true
}

/// Process LLM reflection output into facts.
///
/// The caller is responsible for:
/// 1. Calling the LLM with `reflection_system_prompt()` + `reflection_user_prompt()`
/// 2. Passing the LLM text output to this function
/// 3. Storing the returned facts via MemoryStore
/// 4. Handling open loops (emit events for DreamDaemon)
pub fn process_reflection(
    llm_output: &str,
    already_extracted: &[String],
) -> (Vec<MemoryFact>, Vec<String>) {
    let parsed = parse_reflection(llm_output);
    let facts = map_to_facts(&parsed);
    let open_loops = parsed.open_loops;
    (facts, open_loops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_rejects_when_disabled() {
        let config = ReflectionConfig { enabled: false, ..Default::default() };
        assert!(!should_reflect(10, 500, None, &config));
    }

    #[test]
    fn gate_rejects_too_few_turns() {
        let config = ReflectionConfig { enabled: true, ..Default::default() };
        assert!(!should_reflect(2, 500, None, &config));
    }

    #[test]
    fn gate_rejects_too_few_chars() {
        let config = ReflectionConfig { enabled: true, ..Default::default() };
        assert!(!should_reflect(10, 50, None, &config));
    }

    #[test]
    fn gate_rejects_cooldown() {
        let config = ReflectionConfig { enabled: true, ..Default::default() };
        assert!(!should_reflect(10, 500, Some(5), &config));
    }

    #[test]
    fn gate_passes_when_all_criteria_met() {
        let config = ReflectionConfig { enabled: true, ..Default::default() };
        assert!(should_reflect(10, 500, Some(60), &config));
    }

    #[test]
    fn gate_passes_no_prior_reflection() {
        let config = ReflectionConfig { enabled: true, ..Default::default() };
        assert!(should_reflect(10, 500, None, &config));
    }

    #[test]
    fn process_returns_facts_and_open_loops() {
        let llm_output = "## Invariants\n- User likes Rust\n\n## Open Loops\n- Check perf\n";
        let (facts, loops) = process_reflection(llm_output, &[]);
        assert_eq!(facts.len(), 1);
        assert_eq!(loops.len(), 1);
    }
}
```

**Step 3: Register and test**

Add `pub mod service;` to `src/memory/reflection/mod.rs`.

Run: `cargo test -p alephcore --lib reflection -- -v`
Expected: PASS (16 tests total across all reflection sub-modules)

**Step 4: Commit**

```bash
git add src/memory/reflection/service.rs src/config/types/memory.rs
git commit -m "memory: add ReflectionService with gate logic and session-end orchestration"
```

---

### Task 15: Open Loop Event + DreamDaemon Listener

**Files:**
- Modify: `src/memory/events/mod.rs` (add `OpenLoopDetected` event variant if event system supports it)
- Modify: DreamDaemon to check and resolve open loops during consolidation
- This is an integration task — exact changes depend on event bus implementation

**Step 1: Define OpenLoopAction struct**

Add to `src/memory/reflection/mod.rs`:

```rust
/// An action item extracted from session reflection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenLoopAction {
    pub description: String,
    pub source_session_id: String,
    pub created_at: i64,
    pub resolved: bool,
}
```

**Step 2: Integrate into DreamDaemon consolidation**

During DreamDaemon's consolidation cycle:
1. Load unresolved open loops from store
2. For each: check if context suggests it's resolved (LLM judgment or fact match)
3. Resolved → extract as Lesson fact, mark resolved
4. Unresolved → keep for system prompt injection

**Step 3: Commit**

```bash
git commit -m "memory: add Open Loop tracking with DreamDaemon resolution cycle"
```

---

## Phase 4: UI Integration

### Task 16: Extend MemoryConfig API Types

**Files:**
- Modify: `apps/panel/src/api.rs` (add new config fields to match backend)
- Test: `cargo check` the panel crate

**Step 1: Add new fields to Panel's MemoryConfig mirror type**

Ensure the Panel `api.rs` MemoryConfig struct has all new fields matching the backend, with `#[serde(default)]` on each.

**Step 2: Commit**

```bash
git commit -m "panel: extend MemoryConfig API types for Phase 1-3 config fields"
```

---

### Task 17: Retrieval Pipeline Settings UI

**Files:**
- Modify: `apps/panel/src/views/settings/memory.rs` (add new section component)

**Step 1: Add RetrievalPipelineSettings component**

Add after CompressionSettings section:

```rust
// Retrieval Pipeline Settings section
// - Fusion Strategy dropdown (Rrf / Weighted)
// - RRF Constant k (number input, shown when Rrf selected)
// - BM25 Bonus Weight (number input)
// - Enable Query Expansion (checkbox)
// - Expansion Mode dropdown (BuiltIn / LlmPowered)
```

Follow the existing pattern in memory.rs: receive `config: RwSignal<Option<MemoryConfig>>`, use `on:input` handlers to update signal fields.

**Step 2: Commit**

```bash
git commit -m "panel: add Retrieval Pipeline settings section to Memory page"
```

---

### Task 18: Rerank Provider Settings UI

**Files:**
- Modify: `apps/panel/src/views/settings/memory.rs`

**Step 1: Add RerankProviderSettings component**

Add after RetrievalPipelineSettings:

```rust
// Rerank Provider Settings section
// - Enable Cross-Encoder Rerank (checkbox)
// - Provider dropdown (Jina / SiliconFlow / Voyage / Pinecone / Vllm)
// - API Base URL (text input)
// - API Key (password input)
// - Model (text input)
// - Timeout ms (number input)
// - Rerank Weight (number/range input, 0.0-1.0)
// - [Test Connection] button → calls memory.test_rerank_connection RPC
```

**Test Connection button**: On click, serialize current rerank config to JSON and call `state.rpc_call("memory.test_rerank_connection", params)`. Show success/error message.

**Step 2: Commit**

```bash
git commit -m "panel: add Rerank Provider settings with connection test"
```

---

### Task 19: Tiered Decay Settings UI

**Files:**
- Modify: `apps/panel/src/views/settings/memory.rs` (replace flat decay section with tabbed view)

**Step 1: Replace FactDecaySettings with TieredDecaySettings**

Convert the existing single-parameter-group section into a 3-tab component:

```rust
// Tiered Decay Settings
// Tab bar: [Core] [LongTerm] [ShortTerm]
// Per tab:
//   - Half-Life (days)
//   - Min Strength
//   - Reinforcement Factor
//   - Max Half-Life Multiplier
//   - Access Decay (days)
// Core tab also shows:
//   - Protected Types (comma-separated text)
// Below tabs:
//   - Tier Promotion section
//   - Short→Long: min_access_count, min_age_days, min_strength
//   - Long→Core: min_access_count, min_age_days, min_strength
```

Use a `RwSignal<String>` for `active_tab` state (`"core" | "long_term" | "short_term"`). Conditionally render the parameter group for the active tab.

**Step 2: Commit**

```bash
git commit -m "panel: replace flat decay settings with tiered decay tabs + promotion config"
```

---

### Task 20: Reflection Settings UI

**Files:**
- Modify: `apps/panel/src/views/settings/memory.rs` (extend DreamDaemon section)

**Step 1: Add reflection sub-section**

Below existing DreamDaemon fields:

```rust
// Session Reflection sub-section
// - Enable Session-End Reflection (checkbox)
// - Min Turns to Trigger (number)
// - Min User Chars (number)
// - Cooldown minutes (number)
// - Enable Open Loop Tracking (checkbox)
// - Inject to System Prompt (checkbox)
```

**Step 2: Commit**

```bash
git commit -m "panel: add Session Reflection settings to DreamDaemon section"
```

---

### Task 21: Retrieval Debug Panel

**Files:**
- Modify: `apps/panel/src/views/settings/memory.rs` (add collapsible debug panel at bottom)
- Modify: `apps/panel/src/api.rs` (add retrieve_with_trace API call)

**Step 1: Add API method**

In `apps/panel/src/api.rs`:

```rust
pub struct RetrievalTraceApi;

impl RetrievalTraceApi {
    pub async fn retrieve_with_trace(
        state: &DashboardState,
        query: &str,
    ) -> Result<Value, String> {
        let params = json!({"query": query});
        state.rpc_call("memory.retrieve_with_trace", params).await
    }
}
```

**Step 2: Add debug panel component**

At the bottom of MemoryView, add a collapsible section:

```rust
// Retrieval Debug Panel (collapsible, default collapsed)
// - Text input: query
// - Search button → calls RetrievalTraceApi::retrieve_with_trace
// - Results area:
//   - Pipeline Trace table: stage | items | time | top score
//   - Result Details list: rank, score, content preview, tier, type
```

Use `RwSignal<bool>` for expanded/collapsed state. Use `RwSignal<Option<Value>>` for trace results.

**Step 3: Commit**

```bash
git commit -m "panel: add Retrieval Debug Panel with pipeline trace visualization"
```

---

### Task 22: Final Integration Test

**Step 1: Run full build**

```bash
cargo check -p alephcore
```

**Step 2: Run all tests**

```bash
cargo test -p alephcore --lib
```

**Step 3: Build Panel WASM**

```bash
just build
```

**Step 4: Manual verification**

1. Start dev server: `just dev`
2. Open Panel → Settings → Memory
3. Verify all new sections render correctly
4. Test rerank connection button
5. Test retrieval debug panel with a sample query

**Step 5: Final commit**

```bash
git commit -m "memory: complete Phase 1-4 memory optimization integration"
```

---

## Summary

| Task | Phase | Description | Test Count |
|------|-------|-------------|-----------|
| 1 | P1 | RetrievalTrace infrastructure | 2 |
| 2 | P1 | RRF fusion strategy | 4 |
| 3 | P1 | Cross-encoder rerank (5 providers) | compile |
| 4 | P1 | Query expander (Chinese synonyms) | 4 |
| 5 | P1 | Integrate RRF+rerank+trace into pipeline | existing |
| 6 | P1 | Gateway RPCs for debug + test connection | manual |
| 7 | P2 | TieredDecayConfig + access reinforcement | 5 |
| 8 | P2 | Wire tiered decay into LazyDecayEngine | existing |
| 9 | P2 | Tier promotion in DreamDaemon | 5 |
| 10 | P3 | Add Lesson FactType | existing |
| 11 | P3 | Reflection parser (4-category) | 4 |
| 12 | P3 | Reflection prompt templates | 3 |
| 13 | P3 | Reflection mapper → MemoryFact | 2 |
| 14 | P3 | ReflectionService + gate + config | 7 |
| 15 | P3 | Open Loop → DreamDaemon integration | integration |
| 16 | P4 | Extend Panel API types | compile |
| 17 | P4 | Retrieval Pipeline settings UI | manual |
| 18 | P4 | Rerank Provider settings UI | manual |
| 19 | P4 | Tiered Decay tabbed UI | manual |
| 20 | P4 | Reflection settings UI | manual |
| 21 | P4 | Retrieval Debug Panel | manual |
| 22 | P4 | Final integration test | full suite |
