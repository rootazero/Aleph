//! Test set generator -- builds validation test sets from experience data.
//!
//! Selects representative and boundary-case experiences from the store
//! to form a validation test set for pattern evaluation.

use serde::{Deserialize, Serialize};

use crate::poe::crystallization::experience_store::{ExperienceStore, PoeExperience};

// ============================================================================
// Types
// ============================================================================

/// Source of a test sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SampleSource {
    /// Representative of a cluster of similar experiences.
    ClusterRepresentative,
    /// Boundary case along some dimension.
    BoundaryCase { dimension: String },
}

/// A single test sample with its source metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSample {
    pub experience: PoeExperience,
    pub source: SampleSource,
}

/// A validation test set composed of selected samples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationTestSet {
    pub samples: Vec<TestSample>,
}

// ============================================================================
// TestSetGenerator
// ============================================================================

/// Generates validation test sets from experience store data.
pub struct TestSetGenerator {
    pub max_samples: usize,
    pub min_satisfaction: f32,
}

impl TestSetGenerator {
    /// Create a new generator with given max sample count.
    pub fn new(max_samples: usize) -> Self {
        Self {
            max_samples,
            min_satisfaction: 0.8,
        }
    }

    /// Generate a validation test set for the given pattern.
    pub async fn generate(
        &self,
        pattern_id: &str,
        store: &dyn ExperienceStore,
    ) -> anyhow::Result<ValidationTestSet> {
        // 1. Get all experiences for this pattern
        let experiences = store.get_by_pattern_id(pattern_id).await?;

        // 2. Filter by satisfaction threshold
        let good: Vec<PoeExperience> = experiences
            .into_iter()
            .filter(|e| e.satisfaction >= self.min_satisfaction)
            .collect();

        let mut samples: Vec<TestSample> = Vec::new();

        // 3. Group by objective, take best per group (cluster representatives)
        let mut groups: std::collections::HashMap<String, Vec<PoeExperience>> =
            std::collections::HashMap::new();
        for exp in &good {
            groups
                .entry(exp.objective.clone())
                .or_default()
                .push(exp.clone());
        }

        for (_objective, mut group) in groups {
            // Take the one with highest satisfaction
            group.sort_by(|a, b| {
                b.satisfaction
                    .partial_cmp(&a.satisfaction)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some(best) = group.into_iter().next() {
                samples.push(TestSample {
                    experience: best,
                    source: SampleSource::ClusterRepresentative,
                });
            }
        }

        // 4. Add boundary cases
        if !good.is_empty() {
            let avg_duration: f64 =
                good.iter().map(|e| e.duration_ms as f64).sum::<f64>() / good.len() as f64;

            // Max duration boundary (if > 2x average)
            if let Some(max_dur) = good.iter().max_by_key(|e| e.duration_ms) {
                if max_dur.duration_ms as f64 > avg_duration * 2.0 {
                    // Only add if not already present
                    let already = samples.iter().any(|s| s.experience.id == max_dur.id);
                    if !already {
                        samples.push(TestSample {
                            experience: max_dur.clone(),
                            source: SampleSource::BoundaryCase {
                                dimension: "max_duration".to_string(),
                            },
                        });
                    }
                }
            }

            // Max attempts boundary (if any have > 1 attempt)
            if let Some(max_att) = good.iter().filter(|e| e.attempts > 1).max_by_key(|e| e.attempts)
            {
                let already = samples.iter().any(|s| s.experience.id == max_att.id);
                if !already {
                    samples.push(TestSample {
                        experience: max_att.clone(),
                        source: SampleSource::BoundaryCase {
                            dimension: "max_attempts".to_string(),
                        },
                    });
                }
            }
        }

        // 5. Cap at max_samples
        samples.truncate(self.max_samples);

        Ok(ValidationTestSet { samples })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::experience_store::InMemoryExperienceStore;

    fn make_exp(id: &str, objective: &str, satisfaction: f32, duration_ms: u64, attempts: u8) -> PoeExperience {
        PoeExperience {
            id: id.to_string(),
            task_id: format!("task-{}", id),
            objective: objective.to_string(),
            pattern_id: "test-pattern".to_string(),
            tool_sequence_json: "[]".to_string(),
            parameter_mapping: None,
            satisfaction,
            distance_score: 1.0 - satisfaction,
            attempts,
            duration_ms,
            created_at: 0,
        }
    }

    #[tokio::test]
    async fn test_generate_from_successful_experiences() {
        let store = InMemoryExperienceStore::new();
        store.insert(make_exp("1", "obj-a", 0.9, 1000, 1), &[1.0]).await.unwrap();
        store.insert(make_exp("2", "obj-b", 0.85, 1000, 1), &[1.0]).await.unwrap();

        let gen = TestSetGenerator::new(10);
        let test_set = gen.generate("test-pattern", &store).await.unwrap();

        assert_eq!(test_set.samples.len(), 2);
        assert!(test_set.samples.iter().all(|s| matches!(s.source, SampleSource::ClusterRepresentative)));
    }

    #[tokio::test]
    async fn test_boundary_cases_included() {
        let store = InMemoryExperienceStore::new();
        // Best experience for obj-a (will be cluster rep)
        store.insert(make_exp("1", "obj-a", 0.95, 1000, 1), &[1.0]).await.unwrap();
        // Slow experience, same objective but lower satisfaction (not cluster rep)
        store.insert(make_exp("2", "obj-a", 0.82, 10000, 1), &[1.0]).await.unwrap();
        // Multi-attempt experience, same objective but lower satisfaction (not cluster rep)
        store.insert(make_exp("3", "obj-a", 0.81, 1000, 3), &[1.0]).await.unwrap();

        let gen = TestSetGenerator::new(10);
        let test_set = gen.generate("test-pattern", &store).await.unwrap();

        // Cluster rep is "1" (highest satisfaction for obj-a)
        // Boundary: "2" has max_duration=10000, avg=4000, 10000 > 2*4000 => included
        // Boundary: "3" has attempts=3 > 1 => included
        let has_duration_boundary = test_set.samples.iter().any(|s| {
            matches!(&s.source, SampleSource::BoundaryCase { dimension } if dimension == "max_duration")
        });
        let has_attempts_boundary = test_set.samples.iter().any(|s| {
            matches!(&s.source, SampleSource::BoundaryCase { dimension } if dimension == "max_attempts")
        });

        assert!(has_duration_boundary, "should include max_duration boundary");
        assert!(has_attempts_boundary, "should include max_attempts boundary");
        assert_eq!(test_set.samples.len(), 3); // 1 cluster rep + 2 boundary
    }

    #[tokio::test]
    async fn test_filters_low_satisfaction() {
        let store = InMemoryExperienceStore::new();
        store.insert(make_exp("1", "obj-a", 0.9, 1000, 1), &[1.0]).await.unwrap();
        store.insert(make_exp("2", "obj-b", 0.3, 1000, 1), &[1.0]).await.unwrap();
        store.insert(make_exp("3", "obj-c", 0.5, 1000, 1), &[1.0]).await.unwrap();

        let gen = TestSetGenerator::new(10);
        let test_set = gen.generate("test-pattern", &store).await.unwrap();

        // Only exp "1" has satisfaction >= 0.8
        assert_eq!(test_set.samples.len(), 1);
        assert_eq!(test_set.samples[0].experience.id, "1");
    }
}
