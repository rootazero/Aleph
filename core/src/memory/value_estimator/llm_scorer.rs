//! LLM-based importance scoring with performance monitoring

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::memory::MemoryEntry;
use crate::providers::AiProvider;
use crate::Result;

/// Performance metrics for LLM scoring
#[derive(Debug, Clone, Default)]
pub struct ScoringMetrics {
    /// Total number of LLM calls
    pub total_calls: u64,

    /// Number of cache hits
    pub cache_hits: u64,

    /// Total latency in milliseconds
    pub total_latency_ms: u64,

    /// Number of timeouts
    pub timeouts: u64,

    /// Number of errors
    pub errors: u64,
}

impl ScoringMetrics {
    /// Get average latency in milliseconds
    pub fn avg_latency_ms(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.total_latency_ms as f64 / self.total_calls as f64
        }
    }

    /// Get cache hit rate
    pub fn cache_hit_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.cache_hits as f64 / self.total_calls as f64
        }
    }

    /// Estimate cost based on token usage (rough estimate)
    /// Assumes ~100 tokens per call at $0.01 per 1K tokens
    pub fn estimated_cost_usd(&self) -> f64 {
        let tokens_per_call = 100.0;
        let cost_per_1k_tokens = 0.01;
        let total_tokens = self.total_calls as f64 * tokens_per_call;
        (total_tokens / 1000.0) * cost_per_1k_tokens
    }
}

/// Cache entry for LLM scoring
#[derive(Debug, Clone)]
struct CacheEntry {
    score: f32,
    timestamp: Instant,
}

/// LLM-based scorer for importance estimation
pub struct LlmScorer {
    provider: Arc<dyn AiProvider>,
    config: LlmScorerConfig,
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    metrics: Arc<RwLock<ScoringMetrics>>,
}

/// Configuration for LLM scorer
#[derive(Debug, Clone)]
pub struct LlmScorerConfig {
    /// Model to use for scoring (optional, uses provider default if None)
    pub model: Option<String>,

    /// Temperature for LLM (default: 0.0 for deterministic scoring)
    pub temperature: f32,

    /// Whether to use caching for repeated queries (default: true)
    pub use_cache: bool,

    /// Cache TTL in seconds (default: 3600 = 1 hour)
    pub cache_ttl_secs: u64,

    /// Timeout for LLM calls in milliseconds (default: 5000 = 5 seconds)
    pub timeout_ms: u64,
}

impl Default for LlmScorerConfig {
    fn default() -> Self {
        Self {
            model: None,
            temperature: 0.0,
            use_cache: true,
            cache_ttl_secs: 3600,
            timeout_ms: 5000,
        }
    }
}

impl LlmScorer {
    /// Create a new LLM scorer
    pub fn new(provider: Arc<dyn AiProvider>, config: LlmScorerConfig) -> Self {
        Self {
            provider,
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RwLock::new(ScoringMetrics::default())),
        }
    }

    /// Get current performance metrics
    pub async fn metrics(&self) -> ScoringMetrics {
        self.metrics.read().await.clone()
    }

    /// Clear the cache
    pub async fn clear_cache(&self) {
        self.cache.write().await.clear();
        info!("LLM scorer cache cleared");
    }

    /// Score the importance of a memory entry using LLM
    ///
    /// Returns a score between 0.0 and 1.0 indicating the importance
    /// of the conversation for long-term memory.
    pub async fn score(&self, entry: &MemoryEntry) -> Result<f32> {
        let start = Instant::now();

        // Generate cache key
        let cache_key = self.generate_cache_key(entry);

        // Check cache if enabled
        if self.config.use_cache {
            if let Some(cached_score) = self.check_cache(&cache_key).await {
                let mut metrics = self.metrics.write().await;
                metrics.total_calls += 1;
                metrics.cache_hits += 1;
                metrics.total_latency_ms += start.elapsed().as_millis() as u64;

                debug!(
                    "LLM scorer cache hit for key: {} (score: {})",
                    &cache_key[..8],
                    cached_score
                );
                return Ok(cached_score);
            }
        }

        // Call LLM with timeout
        let score = match self.score_with_timeout(entry).await {
            Ok(s) => {
                // Update metrics - success
                let mut metrics = self.metrics.write().await;
                metrics.total_calls += 1;
                metrics.total_latency_ms += start.elapsed().as_millis() as u64;

                info!(
                    "LLM scored memory entry: {} (latency: {}ms)",
                    s,
                    start.elapsed().as_millis()
                );

                // Cache the result
                if self.config.use_cache {
                    self.cache_result(&cache_key, s).await;
                }

                s
            }
            Err(e) => {
                // Update metrics - error
                let mut metrics = self.metrics.write().await;
                metrics.total_calls += 1;
                metrics.errors += 1;
                metrics.total_latency_ms += start.elapsed().as_millis() as u64;

                warn!("LLM scoring failed: {}", e);
                return Err(e);
            }
        };

        Ok(score)
    }

    /// Score with timeout
    async fn score_with_timeout(&self, entry: &MemoryEntry) -> Result<f32> {
        let timeout_duration = Duration::from_millis(self.config.timeout_ms);

        match tokio::time::timeout(timeout_duration, self.score_internal(entry)).await {
            Ok(result) => result,
            Err(_) => {
                // Timeout occurred
                let mut metrics = self.metrics.write().await;
                metrics.timeouts += 1;

                warn!(
                    "LLM scoring timeout after {}ms",
                    self.config.timeout_ms
                );

                Err(crate::error::AlephError::ConfigError {
                    message: format!("LLM scoring timeout after {}ms", self.config.timeout_ms),
                    suggestion: Some("Increase timeout_ms or check LLM provider".to_string()),
                })
            }
        }
    }

    /// Internal scoring logic
    async fn score_internal(&self, entry: &MemoryEntry) -> Result<f32> {
        let prompt = self.build_scoring_prompt(entry);
        let system_prompt = self.build_system_prompt();

        // Call LLM
        let response = self.provider.process(&prompt, Some(&system_prompt)).await?;

        // Parse response
        let score = self.parse_score(&response)?;

        Ok(score)
    }

    /// Generate cache key from memory entry
    fn generate_cache_key(&self, entry: &MemoryEntry) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        entry.user_input.hash(&mut hasher);
        entry.ai_output.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// Check cache for existing score
    async fn check_cache(&self, key: &str) -> Option<f32> {
        let cache = self.cache.read().await;
        if let Some(entry) = cache.get(key) {
            // Check if entry is still valid
            let age = entry.timestamp.elapsed();
            if age.as_secs() < self.config.cache_ttl_secs {
                return Some(entry.score);
            }
        }
        None
    }

    /// Cache a scoring result
    async fn cache_result(&self, key: &str, score: f32) {
        let mut cache = self.cache.write().await;
        cache.insert(
            key.to_string(),
            CacheEntry {
                score,
                timestamp: Instant::now(),
            },
        );

        debug!("Cached LLM score for key: {}", &key[..8]);
    }

    /// Build the scoring prompt
    fn build_scoring_prompt(&self, entry: &MemoryEntry) -> String {
        format!(
            "Rate the importance of this conversation on a scale of 0.0 to 1.0:\n\n\
             User: {}\n\
             Assistant: {}\n\n\
             Consider:\n\
             - Personal information (high importance)\n\
             - Preferences and decisions (high importance)\n\
             - Factual knowledge (medium importance)\n\
             - Greetings and small talk (low importance)\n\
             - Questions without answers (low importance)\n\n\
             Respond with ONLY a number between 0.0 and 1.0, nothing else.",
            entry.user_input, entry.ai_output
        )
    }

    /// Build the system prompt
    fn build_system_prompt(&self) -> String {
        "You are an importance scorer for conversation memory. \
         Your task is to rate how important a conversation is for long-term memory. \
         Consider the informational value, personal relevance, and decision-making content. \
         Respond with only a decimal number between 0.0 and 1.0."
            .to_string()
    }

    /// Parse the LLM response to extract the score
    fn parse_score(&self, response: &str) -> Result<f32> {
        let trimmed = response.trim();

        // Try to extract a number from the response
        let score_str = trimmed
            .split_whitespace()
            .find(|s| s.parse::<f32>().is_ok())
            .unwrap_or(trimmed);

        let score: f32 = score_str
            .parse()
            .map_err(|_| crate::error::AlephError::ConfigError {
                message: format!("Failed to parse LLM score: {}", response),
                suggestion: Some("LLM should return a number between 0.0 and 1.0".to_string()),
            })?;

        // Clamp to valid range
        Ok(score.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_score_valid() {
        let config = LlmScorerConfig::default();
        let provider = Arc::new(MockProvider);
        let scorer = LlmScorer::new(provider, config);

        assert_eq!(scorer.parse_score("0.5").unwrap(), 0.5);
        assert_eq!(scorer.parse_score("0.95").unwrap(), 0.95);
        assert_eq!(scorer.parse_score("0.0").unwrap(), 0.0);
        assert_eq!(scorer.parse_score("1.0").unwrap(), 1.0);
    }

    #[test]
    fn test_parse_score_with_text() {
        let config = LlmScorerConfig::default();
        let provider = Arc::new(MockProvider);
        let scorer = LlmScorer::new(provider, config);

        // Should extract number from text
        assert_eq!(scorer.parse_score("The score is 0.75").unwrap(), 0.75);
        assert_eq!(scorer.parse_score("0.8 is the importance").unwrap(), 0.8);
    }

    #[test]
    fn test_parse_score_clamping() {
        let config = LlmScorerConfig::default();
        let provider = Arc::new(MockProvider);
        let scorer = LlmScorer::new(provider, config);

        // Should clamp to valid range
        assert_eq!(scorer.parse_score("1.5").unwrap(), 1.0);
        assert_eq!(scorer.parse_score("-0.5").unwrap(), 0.0);
    }

    #[test]
    fn test_parse_score_invalid() {
        let config = LlmScorerConfig::default();
        let provider = Arc::new(MockProvider);
        let scorer = LlmScorer::new(provider, config);

        // Should fail on invalid input
        assert!(scorer.parse_score("not a number").is_err());
        assert!(scorer.parse_score("").is_err());
    }

    // Mock provider for testing
    struct MockProvider;

    impl AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String>> + Send + '_>,
        > {
            Box::pin(async { Ok("0.5".to_string()) })
        }

        fn process_with_image(
            &self,
            _input: &str,
            _image: Option<&crate::clipboard::ImageData>,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String>> + Send + '_>,
        > {
            Box::pin(async { Ok("0.5".to_string()) })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }
}
