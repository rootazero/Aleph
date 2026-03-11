//! Decay-filtered experience store wrapper.
//!
//! Wraps an `ExperienceStore` with decay-weighted filtering,
//! ensuring stale or harmful experiences are excluded from search results.

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use async_trait::async_trait;

use crate::error::AlephError;
use crate::poe::crystallization::experience_store::{ExperienceStore, PoeExperience};
use crate::poe::memory_decay::decay::{DecayCalculator, DecayConfig};
use crate::poe::memory_decay::reuse_tracker::InMemoryReuseTracker;

/// Wraps an ExperienceStore with decay-weighted filtering.
///
/// When searching, applies decay weights to results and filters out
/// experiences that have decayed below the archive threshold.
pub struct DecayFilteredStore<S: ExperienceStore> {
    inner: S,
    decay_config: DecayConfig,
    reuse_tracker: Arc<RwLock<InMemoryReuseTracker>>,
}

impl<S: ExperienceStore> DecayFilteredStore<S> {
    pub fn new(
        inner: S,
        decay_config: DecayConfig,
        reuse_tracker: Arc<RwLock<InMemoryReuseTracker>>,
    ) -> Self {
        Self {
            inner,
            decay_config,
            reuse_tracker,
        }
    }

    /// Search with decay-weighted results, filtering archived experiences.
    ///
    /// Returns (experience, similarity, effective_weight) tuples.
    /// Results with effective_weight below archive_threshold are excluded.
    /// Results are sorted by similarity * effective_weight (combined score).
    pub async fn weighted_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        min_similarity: f64,
    ) -> Result<Vec<(PoeExperience, f64, f32)>, AlephError> {
        // Fetch extra results to account for filtering
        let raw_results = self
            .inner
            .vector_search(query_embedding, limit * 2, min_similarity)
            .await?;

        let tracker = self.reuse_tracker.read().await;
        let now_ms = chrono::Utc::now().timestamp_millis();

        tracing::info!(
            subsystem = "poe",
            probe = "phase2",
            feature = "memory_decay",
            raw_count = raw_results.len(),
            limit = limit,
            "🧠 DECAY search: fetched {} raw candidates for limit={}",
            raw_results.len(), limit,
        );

        let mut weighted: Vec<(PoeExperience, f64, f32)> = Vec::new();
        let mut archived_count = 0_usize;

        for (experience, similarity) in raw_results {
            // Performance factor from reuse history
            let recent = tracker.get_recent(
                &experience.id,
                self.decay_config.performance_window as usize,
            );
            let total_count = recent.len() as u32;
            let success_count = recent.iter().filter(|r| r.led_to_success).count() as u32;
            let performance = DecayCalculator::performance_factor(
                success_count,
                total_count,
                self.decay_config.min_reuses_for_decay,
            );

            // Drift factor: 1.0 (no file change data available)
            let drift = 1.0_f32;

            // Time factor from experience age
            let age_ms = (now_ms - experience.created_at).max(0) as f64;
            let age_days = age_ms / (1000.0 * 60.0 * 60.0 * 24.0);
            let time = DecayCalculator::time_factor(age_days, self.decay_config.time_half_life_days);

            let effective_weight = DecayCalculator::effective_weight(performance, drift, time);

            // Filter archived experiences
            if DecayCalculator::should_archive(effective_weight, &self.decay_config) {
                tracing::info!(
                    subsystem = "poe",
                    probe = "phase2",
                    feature = "memory_decay",
                    experience_id = %experience.id,
                    effective_weight = effective_weight,
                    performance = performance,
                    time_factor = time,
                    age_days = age_days,
                    "🧠 DECAY archived experience '{}' (weight={:.4} perf={:.2} time={:.4} age={:.0}d)",
                    experience.id, effective_weight, performance, time, age_days,
                );
                archived_count += 1;
                continue;
            }

            tracing::debug!(
                subsystem = "poe",
                probe = "phase2",
                feature = "memory_decay",
                experience_id = %experience.id,
                effective_weight = effective_weight,
                similarity = similarity,
                "🧠 DECAY kept experience '{}' (weight={:.4} sim={:.4})",
                experience.id, effective_weight, similarity,
            );

            weighted.push((experience, similarity, effective_weight));
        }

        // Sort by combined score (similarity * effective_weight) descending
        weighted.sort_by(|a, b| {
            let score_a = a.1 * a.2 as f64;
            let score_b = b.1 * b.2 as f64;
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        weighted.truncate(limit);

        tracing::info!(
            subsystem = "poe",
            probe = "phase2",
            feature = "memory_decay",
            final_count = weighted.len(),
            archived_count = archived_count,
            "🧠 DECAY search complete: {} results returned, {} archived/filtered",
            weighted.len(), archived_count,
        );

        Ok(weighted)
    }
}

// ============================================================================
// ExperienceStore trait implementation — transparent decay-filtered wrapper
// ============================================================================

#[async_trait]
impl<S: ExperienceStore> ExperienceStore for DecayFilteredStore<S> {
    async fn insert(
        &self,
        experience: PoeExperience,
        embedding: &[f32],
    ) -> Result<(), AlephError> {
        self.inner.insert(experience, embedding).await
    }

    async fn vector_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        min_similarity: f64,
    ) -> Result<Vec<(PoeExperience, f64)>, AlephError> {
        // Delegate to weighted_search and drop the weight column
        let weighted = self.weighted_search(query_embedding, limit, min_similarity).await?;
        Ok(weighted.into_iter().map(|(exp, sim, _weight)| (exp, sim)).collect())
    }

    async fn get_by_pattern_id(
        &self,
        pattern_id: &str,
    ) -> Result<Vec<PoeExperience>, AlephError> {
        self.inner.get_by_pattern_id(pattern_id).await
    }

    async fn count(&self) -> Result<usize, AlephError> {
        self.inner.count().await
    }

    async fn delete(&self, experience_id: &str) -> Result<bool, AlephError> {
        self.inner.delete(experience_id).await
    }

    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<PoeExperience>, AlephError> {
        self.inner.get_by_ids(ids).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::experience_store::InMemoryExperienceStore;
    use crate::poe::memory_decay::reuse_tracker::ReuseRecord;

    fn make_experience(id: &str, created_at: i64) -> PoeExperience {
        PoeExperience {
            id: id.into(),
            task_id: format!("task-{}", id),
            objective: format!("Objective for {}", id),
            pattern_id: "test-pattern".into(),
            tool_sequence_json: "[]".into(),
            parameter_mapping: None,
            satisfaction: 0.9,
            distance_score: 0.1,
            attempts: 1,
            duration_ms: 1000,
            created_at,
        }
    }

    fn make_reuse(exp_id: &str, success: bool, timestamp: i64) -> ReuseRecord {
        ReuseRecord {
            experience_id: exp_id.into(),
            reused_at: timestamp,
            led_to_success: success,
            task_id: format!("reuse-task-{}", timestamp),
        }
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    #[tokio::test]
    async fn test_low_weight_experiences_filtered() {
        let store = InMemoryExperienceStore::new();
        let now = now_ms();

        // Recent experience but with many failures
        let exp = make_experience("bad-exp", now - 1000);
        store.insert(exp, &[1.0, 0.0, 0.0]).await.unwrap();

        let mut tracker = InMemoryReuseTracker::new();
        // 5 recent failures => performance_factor = 0/5 = 0.0
        // effective_weight = 0.0 * 1.0 * ~1.0 = 0.0 < 0.1 => archived
        for i in 0..5 {
            tracker.record_reuse(make_reuse("bad-exp", false, now / 1000 + i));
        }

        let filtered_store = DecayFilteredStore::new(
            store,
            DecayConfig::default(),
            Arc::new(RwLock::new(tracker)),
        );

        let results = filtered_store
            .weighted_search(&[1.0, 0.0, 0.0], 10, 0.0)
            .await
            .unwrap();

        // Should be filtered out due to low effective weight
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_high_weight_experiences_preserved() {
        let store = InMemoryExperienceStore::new();
        let now = now_ms();

        // Fresh experience
        let exp = make_experience("good-exp", now - 1000);
        store.insert(exp, &[1.0, 0.0, 0.0]).await.unwrap();

        let mut tracker = InMemoryReuseTracker::new();
        // 5 successes => performance_factor = 1.0
        for i in 0..5 {
            tracker.record_reuse(make_reuse("good-exp", true, now / 1000 + i));
        }

        let filtered_store = DecayFilteredStore::new(
            store,
            DecayConfig::default(),
            Arc::new(RwLock::new(tracker)),
        );

        let results = filtered_store
            .weighted_search(&[1.0, 0.0, 0.0], 10, 0.0)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.id, "good-exp");
        // Weight should be high (close to 1.0)
        assert!(results[0].2 > 0.9);
    }

    #[tokio::test]
    async fn test_weight_affects_ranking() {
        let store = InMemoryExperienceStore::new();
        let now = now_ms();

        // Two experiences with identical embeddings (same similarity)
        let exp_high = make_experience("high-weight", now - 1000);
        let exp_low = make_experience("low-weight", now - 1000);
        store.insert(exp_high, &[1.0, 0.0]).await.unwrap();
        store.insert(exp_low, &[1.0, 0.0]).await.unwrap();

        let mut tracker = InMemoryReuseTracker::new();
        // high-weight: all successes => performance = 1.0
        for i in 0..5 {
            tracker.record_reuse(make_reuse("high-weight", true, now / 1000 + i));
        }
        // low-weight: 1 success, 4 failures => performance = 0.2
        // effective_weight = 0.2 * 1.0 * ~1.0 = ~0.2 (above 0.1 threshold)
        tracker.record_reuse(make_reuse("low-weight", true, now / 1000));
        for i in 1..5 {
            tracker.record_reuse(make_reuse("low-weight", false, now / 1000 + i));
        }

        let filtered_store = DecayFilteredStore::new(
            store,
            DecayConfig::default(),
            Arc::new(RwLock::new(tracker)),
        );

        let results = filtered_store
            .weighted_search(&[1.0, 0.0], 10, 0.0)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        // Higher weight should rank first
        assert_eq!(results[0].0.id, "high-weight");
        assert_eq!(results[1].0.id, "low-weight");
        assert!(results[0].2 > results[1].2);
    }

    #[tokio::test]
    async fn test_archived_excluded() {
        let store = InMemoryExperienceStore::new();
        let now = now_ms();

        // One good experience and one that should be archived
        let good = make_experience("keeper", now - 1000);
        let bad = make_experience("archive-me", now - 1000);
        store.insert(good, &[1.0, 0.0]).await.unwrap();
        store.insert(bad, &[1.0, 0.0]).await.unwrap();

        let mut tracker = InMemoryReuseTracker::new();
        // keeper: no reuse history => performance = 1.0 (benefit of the doubt)
        // archive-me: all failures => performance = 0.0
        // effective_weight = 0.0 => should_archive = true
        for i in 0..5 {
            tracker.record_reuse(make_reuse("archive-me", false, now / 1000 + i));
        }

        let filtered_store = DecayFilteredStore::new(
            store,
            DecayConfig::default(),
            Arc::new(RwLock::new(tracker)),
        );

        let results = filtered_store
            .weighted_search(&[1.0, 0.0], 10, 0.0)
            .await
            .unwrap();

        // Only the keeper should remain
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.id, "keeper");
        // Archived experience should not appear
        assert!(results.iter().all(|(exp, _, _)| exp.id != "archive-me"));
    }
}
