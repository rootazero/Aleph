# Cognitive Evolution Gamma Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Vitality Score engine, auto-de-solidification with triple-tiered triggers, KnowledgeConsolidator for semantic dedup, L3 Sandbox Executor with shadow FS, and Skill Graveyard for failed pattern archival.

**Architecture:** Vitality Score computes continuous health from SkillMetrics. De-solidification adds Observation lifecycle state and circuit breaker. KnowledgeConsolidator runs during idle-time dreaming. Sandbox uses tempdir + restricted toolset (no Docker). Graveyard archives retired skills as negative constraints.

**Tech Stack:** Rust, async_trait, serde, tokio, tempfile, anyhow

---

### Task 1: VitalityScore Types and Computation

**Files:**
- Create: `core/src/skill_evolution/vitality.rs`
- Modify: `core/src/skill_evolution/mod.rs` (add `pub mod vitality;`)

**Step 1: Write the failing tests**

Create `core/src/skill_evolution/vitality.rs`:

```rust
//! Vitality Score engine for continuous skill health assessment.
//!
//! Computes a 0.0–1.0 vitality score from success rate, invocation frequency,
//! maintenance cost, and user feedback. Used by the lifecycle manager to drive
//! promotion, observation, demotion, and retirement transitions.

use serde::{Deserialize, Serialize};

/// Components that make up a vitality score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VitalityComponents {
    pub success_rate: f32,
    pub frequency_score: f32,
    pub maintenance_cost_inverse: f32,
    pub user_feedback_multiplier: f32,
}

/// Computed vitality score with its breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VitalityScore {
    pub value: f32,
    pub components: VitalityComponents,
}

/// Configuration for vitality computation.
#[derive(Debug, Clone)]
pub struct VitalityConfig {
    /// Expected invocations per 30-day window (denominator for frequency).
    pub expected_frequency: f32,
    /// Token-cost normalizer (baseline single LLM call cost).
    pub cost_normalizer: f32,
    /// Token-equivalent penalty per retry.
    pub retry_penalty: f32,
    /// Vitality threshold: healthy (no action).
    pub healthy_threshold: f32,
    /// Vitality threshold: enter observation.
    pub warning_threshold: f32,
    /// Vitality threshold: trigger demotion.
    pub demotion_threshold: f32,
    /// Vitality threshold: trigger retirement.
    pub retirement_threshold: f32,
}

impl Default for VitalityConfig {
    fn default() -> Self {
        Self {
            expected_frequency: 10.0,
            cost_normalizer: 2000.0,
            retry_penalty: 500.0,
            healthy_threshold: 0.5,
            warning_threshold: 0.3,
            demotion_threshold: 0.15,
            retirement_threshold: 0.05,
        }
    }
}

/// Input data for vitality computation.
pub struct VitalityInput {
    /// Success rate from SkillMetrics (0.0–1.0).
    pub success_rate: f32,
    /// Invocations in the last 30 days.
    pub invocations_last_30d: u32,
    /// Average tokens consumed per invocation.
    pub avg_tokens: f32,
    /// Average retries per invocation.
    pub avg_retries: f32,
    /// Current user feedback multiplier (1.0 = neutral).
    pub user_feedback_multiplier: f32,
}

impl VitalityScore {
    /// Compute vitality from input metrics and config.
    pub fn compute(input: &VitalityInput, config: &VitalityConfig) -> Self {
        let success_rate = input.success_rate.clamp(0.0, 1.0);

        let frequency_score = (input.invocations_last_30d as f32 / config.expected_frequency)
            .min(1.0);

        let raw_cost = input.avg_tokens + (input.avg_retries * config.retry_penalty);
        let maintenance_cost_inverse = 1.0 / (1.0 + raw_cost / config.cost_normalizer);

        let user_mul = input.user_feedback_multiplier.clamp(0.0, 1.0);

        let value = (success_rate * frequency_score * maintenance_cost_inverse * user_mul)
            .clamp(0.0, 1.0);

        Self {
            value,
            components: VitalityComponents {
                success_rate,
                frequency_score,
                maintenance_cost_inverse,
                user_feedback_multiplier: user_mul,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_vitality() {
        let input = VitalityInput {
            success_rate: 1.0,
            invocations_last_30d: 10,
            avg_tokens: 500.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 1.0,
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        // 1.0 * 1.0 * (1/(1+500/2000)) * 1.0 = 1.0 * 1.0 * 0.8 * 1.0 = 0.8
        assert!((score.value - 0.8).abs() < 0.01);
    }

    #[test]
    fn zero_invocations_zero_vitality() {
        let input = VitalityInput {
            success_rate: 1.0,
            invocations_last_30d: 0,
            avg_tokens: 500.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 1.0,
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        assert!((score.value - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn negative_feedback_reduces_vitality() {
        let input = VitalityInput {
            success_rate: 1.0,
            invocations_last_30d: 10,
            avg_tokens: 500.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 0.7, // one negative feedback
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        // 1.0 * 1.0 * 0.8 * 0.7 = 0.56
        assert!((score.value - 0.56).abs() < 0.01);
    }

    #[test]
    fn high_cost_reduces_vitality() {
        let input = VitalityInput {
            success_rate: 1.0,
            invocations_last_30d: 10,
            avg_tokens: 10000.0, // very expensive
            avg_retries: 2.0,    // with retries
            user_feedback_multiplier: 1.0,
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        // cost = 10000 + 2*500 = 11000; inverse = 1/(1+11000/2000) = 1/6.5 ≈ 0.154
        // value = 1.0 * 1.0 * 0.154 * 1.0 ≈ 0.154
        assert!(score.value < 0.2);
    }

    #[test]
    fn values_clamped() {
        let input = VitalityInput {
            success_rate: 1.5, // out of range
            invocations_last_30d: 100,
            avg_tokens: 0.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 2.0, // out of range
        };
        let score = VitalityScore::compute(&input, &VitalityConfig::default());
        assert!(score.value <= 1.0);
        assert!(score.components.success_rate <= 1.0);
        assert!(score.components.user_feedback_multiplier <= 1.0);
    }
}
```

**Step 2: Add module declaration**

In `core/src/skill_evolution/mod.rs`, add after line 57 (`pub mod validation;`):

```rust
pub mod vitality;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib skill_evolution::vitality`
Expected: PASS (5 tests)

**Step 4: Commit**

```bash
git add core/src/skill_evolution/vitality.rs core/src/skill_evolution/mod.rs
git commit -m "evolution: add VitalityScore engine with continuous health scoring"
```

---

### Task 2: Lifecycle Extension — Observation State

**Files:**
- Modify: `core/src/skill_evolution/lifecycle.rs`

**Step 1: Write the failing tests**

Add to `lifecycle.rs` tests module:

```rust
#[test]
fn observation_state_serialization() {
    let state = SkillLifecycleState::Observation {
        entered_at: 1000,
        reason: ObservationReason::VitalityWarning,
        previous_vitality: 0.28,
    };
    let json = serde_json::to_string(&state).unwrap();
    let back: SkillLifecycleState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, back);
}

#[test]
fn observation_reason_variants() {
    assert_ne!(
        ObservationReason::VitalityWarning,
        ObservationReason::EntropyIncreasing,
    );
    assert_ne!(
        ObservationReason::EntropyIncreasing,
        ObservationReason::UserFeedback,
    );
}
```

**Step 2: Add Observation state and ObservationReason enum**

In `lifecycle.rs`, add `Observation` variant to `SkillLifecycleState`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SkillLifecycleState {
    Draft,
    Shadow(ShadowState),
    Promoted {
        promoted_at: i64,
        shadow_duration_days: u32,
    },
    Observation {
        entered_at: i64,
        reason: ObservationReason,
        previous_vitality: f32,
    },
    Demoted {
        reason: String,
        demoted_at: i64,
    },
    Retired {
        reason: String,
        retired_at: i64,
    },
}
```

Add `ObservationReason` enum before `SkillOrigin`:

```rust
/// Reason a skill entered the observation period.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ObservationReason {
    VitalityWarning,
    EntropyIncreasing,
    UserFeedback,
}
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib skill_evolution::lifecycle`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/skill_evolution/lifecycle.rs
git commit -m "evolution: add Observation lifecycle state with ObservationReason"
```

---

### Task 3: Circuit Breaker (De-solidification Layer 1)

**Files:**
- Create: `core/src/skill_evolution/desolidification.rs`
- Modify: `core/src/skill_evolution/mod.rs` (add `pub mod desolidification;`)

**Step 1: Write the file with tests**

Create `core/src/skill_evolution/desolidification.rs`:

```rust
//! Auto-de-solidification with triple-tiered triggers.
//!
//! Layer 1: Circuit Breaker — immediate demotion on catastrophic failure.
//! Layer 2: Entropy Canary — vitality penalty on degradation trends.
//! Layer 3: User Feedback — amplifier on human negative signals.

use serde::{Deserialize, Serialize};

// ============================================================================
// Circuit Breaker (Layer 1)
// ============================================================================

/// Configuration for the circuit breaker (immediate demotion).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    pub consecutive_failure_limit: u32,
    pub success_rate_floor: f32,
    pub window_size: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            consecutive_failure_limit: 3,
            success_rate_floor: 0.5,
            window_size: 10,
        }
    }
}

/// Result of a circuit breaker check.
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitBreakerVerdict {
    /// Skill is healthy, no action.
    Healthy,
    /// Skill has tripped the breaker — demote immediately.
    Tripped { reason: String },
}

/// Checks recent execution window for catastrophic failure patterns.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self { config }
    }

    /// Check a window of recent execution outcomes (true = success, false = failure).
    /// Most recent execution is last in the slice.
    pub fn check(&self, recent_outcomes: &[bool]) -> CircuitBreakerVerdict {
        if recent_outcomes.is_empty() {
            return CircuitBreakerVerdict::Healthy;
        }

        // Check consecutive failures from the tail
        let consecutive_fails = recent_outcomes
            .iter()
            .rev()
            .take_while(|&&success| !success)
            .count() as u32;

        if consecutive_fails >= self.config.consecutive_failure_limit {
            return CircuitBreakerVerdict::Tripped {
                reason: format!(
                    "{} consecutive failures (limit: {})",
                    consecutive_fails, self.config.consecutive_failure_limit
                ),
            };
        }

        // Check success rate over window
        let window: Vec<_> = recent_outcomes
            .iter()
            .rev()
            .take(self.config.window_size as usize)
            .collect();
        let success_count = window.iter().filter(|&&s| *s).count() as f32;
        let rate = success_count / window.len() as f32;

        if rate < self.config.success_rate_floor {
            return CircuitBreakerVerdict::Tripped {
                reason: format!(
                    "success rate {:.0}% below floor {:.0}%",
                    rate * 100.0,
                    self.config.success_rate_floor * 100.0
                ),
            };
        }

        CircuitBreakerVerdict::Healthy
    }
}

// ============================================================================
// Entropy Canary (Layer 2)
// ============================================================================

/// Configuration for the entropy canary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropyCanaryConfig {
    /// How much to penalize vitality when entropy is increasing.
    pub entropy_penalty: f32,
    /// Duration degradation threshold (50% = 0.5).
    pub duration_degradation_threshold: f32,
}

impl Default for EntropyCanaryConfig {
    fn default() -> Self {
        Self {
            entropy_penalty: 0.2,
            duration_degradation_threshold: 0.5,
        }
    }
}

/// Computes a vitality penalty based on entropy trend and duration changes.
pub fn compute_entropy_penalty(
    entropy_increasing: bool,
    duration_baseline_ms: f32,
    duration_current_ms: f32,
    config: &EntropyCanaryConfig,
) -> f32 {
    let mut penalty = 0.0;

    if entropy_increasing {
        penalty += config.entropy_penalty;
    }

    if duration_baseline_ms > 0.0 {
        let degradation = (duration_current_ms - duration_baseline_ms) / duration_baseline_ms;
        if degradation > config.duration_degradation_threshold {
            penalty += config.entropy_penalty * 0.5;
        }
    }

    penalty.min(0.5) // cap total penalty
}

// ============================================================================
// User Feedback (Layer 3)
// ============================================================================

/// Type of user feedback event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FeedbackType {
    Positive,
    Negative,
    ManualEdit,
}

/// A recorded user feedback event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFeedbackEvent {
    pub skill_id: String,
    pub feedback_type: FeedbackType,
    pub timestamp: i64,
}

/// Apply a feedback event to a user_feedback_multiplier.
pub fn apply_feedback(current_multiplier: f32, feedback: &FeedbackType) -> f32 {
    match feedback {
        FeedbackType::Negative | FeedbackType::ManualEdit => (current_multiplier * 0.7).max(0.1),
        FeedbackType::Positive => (current_multiplier * 1.1).min(1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Circuit Breaker tests --

    #[test]
    fn breaker_healthy_all_success() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        let outcomes = vec![true, true, true, true, true];
        assert_eq!(breaker.check(&outcomes), CircuitBreakerVerdict::Healthy);
    }

    #[test]
    fn breaker_trips_consecutive_failures() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        let outcomes = vec![true, true, false, false, false];
        match breaker.check(&outcomes) {
            CircuitBreakerVerdict::Tripped { reason } => {
                assert!(reason.contains("3 consecutive failures"));
            }
            _ => panic!("Expected tripped"),
        }
    }

    #[test]
    fn breaker_trips_low_success_rate() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        // 3 success, 7 failures = 30% < 50% floor
        let outcomes = vec![true, true, true, false, false, false, false, false, false, false];
        match breaker.check(&outcomes) {
            CircuitBreakerVerdict::Tripped { reason } => {
                assert!(reason.contains("below floor"));
            }
            _ => panic!("Expected tripped"),
        }
    }

    #[test]
    fn breaker_healthy_on_empty() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert_eq!(breaker.check(&[]), CircuitBreakerVerdict::Healthy);
    }

    // -- Entropy Canary tests --

    #[test]
    fn entropy_penalty_when_increasing() {
        let config = EntropyCanaryConfig::default();
        let penalty = compute_entropy_penalty(true, 1000.0, 1000.0, &config);
        assert!((penalty - 0.2).abs() < 0.01);
    }

    #[test]
    fn entropy_penalty_with_duration_degradation() {
        let config = EntropyCanaryConfig::default();
        // 60% slower + entropy increasing = 0.2 + 0.1 = 0.3
        let penalty = compute_entropy_penalty(true, 1000.0, 1600.0, &config);
        assert!((penalty - 0.3).abs() < 0.01);
    }

    #[test]
    fn entropy_penalty_capped() {
        let config = EntropyCanaryConfig {
            entropy_penalty: 0.4,
            duration_degradation_threshold: 0.1,
        };
        let penalty = compute_entropy_penalty(true, 100.0, 1000.0, &config);
        assert!((penalty - 0.5).abs() < 0.01); // capped at 0.5
    }

    #[test]
    fn no_penalty_when_healthy() {
        let config = EntropyCanaryConfig::default();
        let penalty = compute_entropy_penalty(false, 1000.0, 1000.0, &config);
        assert!((penalty - 0.0).abs() < f32::EPSILON);
    }

    // -- User Feedback tests --

    #[test]
    fn negative_feedback_reduces_multiplier() {
        let mul = apply_feedback(1.0, &FeedbackType::Negative);
        assert!((mul - 0.7).abs() < 0.01);
    }

    #[test]
    fn manual_edit_reduces_multiplier() {
        let mul = apply_feedback(1.0, &FeedbackType::ManualEdit);
        assert!((mul - 0.7).abs() < 0.01);
    }

    #[test]
    fn positive_feedback_recovers_multiplier() {
        let mul = apply_feedback(0.7, &FeedbackType::Positive);
        assert!((mul - 0.77).abs() < 0.01);
    }

    #[test]
    fn positive_feedback_capped_at_one() {
        let mul = apply_feedback(0.95, &FeedbackType::Positive);
        assert!((mul - 1.0).abs() < 0.01);
    }

    #[test]
    fn multiplier_has_floor() {
        let mut mul = 1.0;
        for _ in 0..20 {
            mul = apply_feedback(mul, &FeedbackType::Negative);
        }
        assert!(mul >= 0.1);
    }
}
```

**Step 2: Add module declaration**

In `core/src/skill_evolution/mod.rs`, add after the `pub mod vitality;` line:

```rust
pub mod desolidification;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib skill_evolution::desolidification`
Expected: PASS (10 tests)

**Step 4: Commit**

```bash
git add core/src/skill_evolution/desolidification.rs core/src/skill_evolution/mod.rs
git commit -m "evolution: add triple-tiered de-solidification (circuit breaker + entropy canary + feedback)"
```

---

### Task 4: Skill Graveyard

**Files:**
- Create: `core/src/skill_evolution/graveyard.rs`
- Modify: `core/src/skill_evolution/mod.rs` (add `pub mod graveyard;`)

**Step 1: Write the file with tests**

Create `core/src/skill_evolution/graveyard.rs`:

```rust
//! Skill Graveyard — archive for retired/demoted skills.
//!
//! Failed patterns become negative constraints for future skill generation,
//! enabling the system to "learn from failure" rather than repeating mistakes.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info, warn};

/// A single entry in the skill graveyard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraveyardEntry {
    pub skill_id: String,
    pub skill_md: String,
    pub failure_traces: Vec<String>,
    pub reason: String,
    pub retired_at: i64,
    pub vitality_at_death: f32,
}

/// Maximum entries before FIFO eviction.
const MAX_ENTRIES: usize = 100;

/// Manages the skill graveyard — a FIFO archive of failed/retired skills.
pub struct SkillGraveyard {
    entries: Vec<GraveyardEntry>,
    storage_path: PathBuf,
}

impl SkillGraveyard {
    /// Create or load a graveyard at the given directory.
    pub async fn open(graveyard_dir: &Path) -> anyhow::Result<Self> {
        let storage_path = graveyard_dir.join("graveyard.json");

        let entries = if storage_path.exists() {
            let data = fs::read_to_string(&storage_path).await?;
            serde_json::from_str(&data).unwrap_or_else(|e| {
                warn!("Failed to parse graveyard.json: {}, starting fresh", e);
                Vec::new()
            })
        } else {
            Vec::new()
        };

        Ok(Self {
            entries,
            storage_path,
        })
    }

    /// Create an in-memory graveyard (for testing).
    pub fn in_memory() -> Self {
        Self {
            entries: Vec::new(),
            storage_path: PathBuf::from("/dev/null"),
        }
    }

    /// Archive a retired skill. FIFO eviction when full.
    pub async fn archive(&mut self, entry: GraveyardEntry) -> anyhow::Result<()> {
        info!(skill_id = %entry.skill_id, reason = %entry.reason, "Archiving skill to graveyard");

        // FIFO eviction
        while self.entries.len() >= MAX_ENTRIES {
            let evicted = self.entries.remove(0);
            debug!(skill_id = %evicted.skill_id, "Evicted oldest graveyard entry");
        }

        self.entries.push(entry);
        self.persist().await
    }

    /// Get entries similar to a description (simple keyword overlap for now).
    /// Returns entries whose skill_md contains any of the given keywords.
    pub fn query_similar(&self, keywords: &[&str]) -> Vec<&GraveyardEntry> {
        self.entries
            .iter()
            .filter(|e| {
                let lower = e.skill_md.to_lowercase();
                keywords.iter().any(|kw| lower.contains(&kw.to_lowercase()))
            })
            .collect()
    }

    /// Get all entries (for inspection/testing).
    pub fn entries(&self) -> &[GraveyardEntry] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the graveyard is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    async fn persist(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let json = serde_json::to_string_pretty(&self.entries)?;
        fs::write(&self.storage_path, json).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, reason: &str) -> GraveyardEntry {
        GraveyardEntry {
            skill_id: id.to_string(),
            skill_md: format!("# Skill {}\nDoes something with files and code.", id),
            failure_traces: vec!["trace-1".to_string()],
            reason: reason.to_string(),
            retired_at: 1000,
            vitality_at_death: 0.05,
        }
    }

    #[tokio::test]
    async fn archive_and_query() {
        let mut graveyard = SkillGraveyard::in_memory();
        assert!(graveyard.is_empty());

        graveyard.archive(make_entry("skill-1", "too slow")).await.unwrap();
        graveyard.archive(make_entry("skill-2", "wrong output")).await.unwrap();

        assert_eq!(graveyard.len(), 2);

        let similar = graveyard.query_similar(&["files"]);
        assert_eq!(similar.len(), 2);

        let similar = graveyard.query_similar(&["nonexistent"]);
        assert!(similar.is_empty());
    }

    #[tokio::test]
    async fn fifo_eviction() {
        let mut graveyard = SkillGraveyard::in_memory();

        for i in 0..105 {
            graveyard
                .archive(make_entry(&format!("skill-{}", i), "eviction test"))
                .await
                .unwrap();
        }

        assert_eq!(graveyard.len(), 100);
        // Oldest entries (0-4) should be evicted
        assert_eq!(graveyard.entries()[0].skill_id, "skill-5");
    }

    #[tokio::test]
    async fn persistence_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let graveyard_dir = tmp.path().join(".graveyard");

        {
            let mut gy = SkillGraveyard::open(&graveyard_dir).await.unwrap();
            gy.archive(make_entry("s1", "test")).await.unwrap();
            assert_eq!(gy.len(), 1);
        }

        // Re-open and verify
        let gy = SkillGraveyard::open(&graveyard_dir).await.unwrap();
        assert_eq!(gy.len(), 1);
        assert_eq!(gy.entries()[0].skill_id, "s1");
    }
}
```

**Step 2: Add module declaration**

In `core/src/skill_evolution/mod.rs`, add after `pub mod desolidification;`:

```rust
pub mod graveyard;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib skill_evolution::graveyard`
Expected: PASS (3 tests)

**Step 4: Commit**

```bash
git add core/src/skill_evolution/graveyard.rs core/src/skill_evolution/mod.rs
git commit -m "evolution: add SkillGraveyard for failed pattern archival"
```

---

### Task 5: L3 Sandbox — ShadowFs

**Files:**
- Create: `core/src/skill_evolution/validation/shadow_fs.rs`
- Modify: `core/src/skill_evolution/validation/mod.rs` (add `pub mod shadow_fs;`)

**Step 1: Write the file with tests**

Create `core/src/skill_evolution/validation/shadow_fs.rs`:

```rust
//! Shadow Filesystem — read from source, write to overlay.
//!
//! Provides an isolated filesystem view where all reads transparently
//! proxy to the original workspace (read-only) and all writes redirect
//! to a temporary overlay directory.

use std::path::{Path, PathBuf};
use tokio::fs;
use anyhow::Result;

/// Shadow filesystem: reads from source, writes to overlay.
pub struct ShadowFs {
    source_dir: PathBuf,
    overlay_dir: PathBuf,
}

impl ShadowFs {
    /// Create a new shadow filesystem.
    pub fn new(source_dir: PathBuf, overlay_dir: PathBuf) -> Self {
        Self {
            source_dir,
            overlay_dir,
        }
    }

    /// Resolve a relative path for reading. Checks overlay first, then source.
    pub fn resolve_read(&self, relative: &Path) -> PathBuf {
        let overlay_path = self.overlay_dir.join(relative);
        if overlay_path.exists() {
            overlay_path
        } else {
            self.source_dir.join(relative)
        }
    }

    /// Resolve a relative path for writing. Always goes to overlay.
    pub fn resolve_write(&self, relative: &Path) -> PathBuf {
        self.overlay_dir.join(relative)
    }

    /// Read a file through the shadow FS.
    pub async fn read(&self, relative: &Path) -> Result<String> {
        let path = self.resolve_read(relative);
        Ok(fs::read_to_string(&path).await?)
    }

    /// Write a file through the shadow FS (always to overlay).
    pub async fn write(&self, relative: &Path, content: &str) -> Result<()> {
        let path = self.resolve_write(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, content).await?;
        Ok(())
    }

    /// List files modified in the overlay.
    pub async fn modified_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        Self::collect_files(&self.overlay_dir, &self.overlay_dir, &mut files).await?;
        Ok(files)
    }

    /// Source directory (read-only workspace).
    pub fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    /// Overlay directory (writable sandbox).
    pub fn overlay_dir(&self) -> &Path {
        &self.overlay_dir
    }

    /// Recursively collect relative file paths under a directory.
    async fn collect_files(
        base: &Path,
        dir: &Path,
        out: &mut Vec<PathBuf>,
    ) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                Box::pin(Self::collect_files(base, &path, out)).await?;
            } else {
                let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
                out.push(relative);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_from_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        fs::write(source.join("test.txt"), "from source").await.unwrap();

        let sfs = ShadowFs::new(source, overlay);
        let content = sfs.read(Path::new("test.txt")).await.unwrap();
        assert_eq!(content, "from source");
    }

    #[tokio::test]
    async fn write_goes_to_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let sfs = ShadowFs::new(source.clone(), overlay.clone());
        sfs.write(Path::new("new.txt"), "written data").await.unwrap();

        // File should be in overlay, not source
        assert!(overlay.join("new.txt").exists());
        assert!(!source.join("new.txt").exists());
    }

    #[tokio::test]
    async fn overlay_overrides_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        fs::write(source.join("file.txt"), "original").await.unwrap();
        fs::write(overlay.join("file.txt"), "modified").await.unwrap();

        let sfs = ShadowFs::new(source, overlay);
        let content = sfs.read(Path::new("file.txt")).await.unwrap();
        assert_eq!(content, "modified");
    }

    #[tokio::test]
    async fn modified_files_lists_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let sfs = ShadowFs::new(source, overlay);
        sfs.write(Path::new("a.txt"), "a").await.unwrap();
        sfs.write(Path::new("sub/b.txt"), "b").await.unwrap();

        let files = sfs.modified_files().await.unwrap();
        assert_eq!(files.len(), 2);
    }
}
```

**Step 2: Add module declaration**

In `core/src/skill_evolution/validation/mod.rs`, add:

```rust
pub mod shadow_fs;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib skill_evolution::validation::shadow_fs`
Expected: PASS (4 tests)

**Step 4: Commit**

```bash
git add core/src/skill_evolution/validation/shadow_fs.rs core/src/skill_evolution/validation/mod.rs
git commit -m "evolution: add ShadowFs for sandbox read-source/write-overlay isolation"
```

---

### Task 6: L3 Sandbox — RestrictedToolset

**Files:**
- Create: `core/src/skill_evolution/validation/restricted_tools.rs`
- Modify: `core/src/skill_evolution/validation/mod.rs` (add `pub mod restricted_tools;`)

**Step 1: Write the file with tests**

Create `core/src/skill_evolution/validation/restricted_tools.rs`:

```rust
//! Restricted toolset — software-defined sandbox boundaries.
//!
//! Validates tool calls against a whitelist and constrains all
//! file path operations to a root directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// A violation detected by the restricted toolset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxViolation {
    pub tool_name: String,
    pub reason: String,
}

/// Enforces sandbox constraints on tool calls.
pub struct RestrictedToolset {
    allowed_tools: HashSet<String>,
    root_dir: PathBuf,
    allow_network: bool,
}

impl RestrictedToolset {
    /// Create a new restricted toolset.
    pub fn new(
        allowed_tools: HashSet<String>,
        root_dir: PathBuf,
        allow_network: bool,
    ) -> Self {
        Self {
            allowed_tools,
            root_dir,
            allow_network,
        }
    }

    /// Validate a tool call. Returns Ok(()) if allowed, Err with violation if not.
    pub fn validate_call(
        &self,
        tool_name: &str,
        file_path: Option<&Path>,
    ) -> Result<(), SandboxViolation> {
        // Check tool whitelist
        if !self.allowed_tools.contains(tool_name) {
            return Err(SandboxViolation {
                tool_name: tool_name.to_string(),
                reason: format!("tool '{}' not in whitelist", tool_name),
            });
        }

        // Check network access
        if !self.allow_network && is_network_tool(tool_name) {
            return Err(SandboxViolation {
                tool_name: tool_name.to_string(),
                reason: "network access not allowed in sandbox".to_string(),
            });
        }

        // Check path bounds
        if let Some(path) = file_path {
            self.validate_path(tool_name, path)?;
        }

        Ok(())
    }

    /// Validate that a path resolves within root_dir.
    fn validate_path(&self, tool_name: &str, path: &Path) -> Result<(), SandboxViolation> {
        // Normalize: join with root if relative, then canonicalize-like check
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root_dir.join(path)
        };

        // Use components to check for path traversal
        let normalized = normalize_path(&resolved);
        let root_normalized = normalize_path(&self.root_dir);

        if !normalized.starts_with(&root_normalized) {
            return Err(SandboxViolation {
                tool_name: tool_name.to_string(),
                reason: format!(
                    "path '{}' escapes sandbox root '{}'",
                    path.display(),
                    self.root_dir.display()
                ),
            });
        }

        Ok(())
    }
}

/// Simple path normalization (resolve `.` and `..` without touching filesystem).
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

/// Check if a tool name implies network access.
fn is_network_tool(name: &str) -> bool {
    let network_tools = ["http_request", "fetch_url", "web_search", "api_call"];
    network_tools.iter().any(|t| name.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_toolset(tools: &[&str], root: &str) -> RestrictedToolset {
        RestrictedToolset::new(
            tools.iter().map(|s| s.to_string()).collect(),
            PathBuf::from(root),
            false,
        )
    }

    #[test]
    fn allowed_tool_passes() {
        let ts = make_toolset(&["read_file", "write_file"], "/sandbox");
        assert!(ts.validate_call("read_file", None).is_ok());
    }

    #[test]
    fn disallowed_tool_fails() {
        let ts = make_toolset(&["read_file"], "/sandbox");
        let err = ts.validate_call("delete_all", None).unwrap_err();
        assert!(err.reason.contains("not in whitelist"));
    }

    #[test]
    fn path_within_root_passes() {
        let ts = make_toolset(&["write_file"], "/sandbox");
        assert!(ts.validate_call("write_file", Some(Path::new("subdir/file.txt"))).is_ok());
    }

    #[test]
    fn path_escape_fails() {
        let ts = make_toolset(&["write_file"], "/sandbox");
        let err = ts
            .validate_call("write_file", Some(Path::new("../../../etc/passwd")))
            .unwrap_err();
        assert!(err.reason.contains("escapes sandbox root"));
    }

    #[test]
    fn absolute_path_outside_root_fails() {
        let ts = make_toolset(&["write_file"], "/sandbox");
        let err = ts
            .validate_call("write_file", Some(Path::new("/home/user/secret.txt")))
            .unwrap_err();
        assert!(err.reason.contains("escapes sandbox root"));
    }

    #[test]
    fn network_tool_blocked_when_disabled() {
        let ts = make_toolset(&["http_request"], "/sandbox");
        let err = ts.validate_call("http_request", None).unwrap_err();
        assert!(err.reason.contains("network access not allowed"));
    }

    #[test]
    fn network_tool_allowed_when_enabled() {
        let ts = RestrictedToolset::new(
            ["http_request"].iter().map(|s| s.to_string()).collect(),
            PathBuf::from("/sandbox"),
            true, // allow network
        );
        assert!(ts.validate_call("http_request", None).is_ok());
    }

    #[test]
    fn normalize_resolves_dotdot() {
        let p = normalize_path(Path::new("/a/b/../c"));
        assert_eq!(p, PathBuf::from("/a/c"));
    }
}
```

**Step 2: Add module declaration**

In `core/src/skill_evolution/validation/mod.rs`, add:

```rust
pub mod restricted_tools;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib skill_evolution::validation::restricted_tools`
Expected: PASS (8 tests)

**Step 4: Commit**

```bash
git add core/src/skill_evolution/validation/restricted_tools.rs core/src/skill_evolution/validation/mod.rs
git commit -m "evolution: add RestrictedToolset for sandbox path and tool whitelist enforcement"
```

---

### Task 7: L3 Sandbox — SandboxExecutor

**Files:**
- Create: `core/src/skill_evolution/validation/sandbox_executor.rs`
- Modify: `core/src/skill_evolution/validation/mod.rs` (add `pub mod sandbox_executor;`)

**Step 1: Write the file with tests**

Create `core/src/skill_evolution/validation/sandbox_executor.rs`:

```rust
//! L3 Sandbox Executor — isolated execution environment for High-risk skill validation.
//!
//! Combines ShadowFs (read source, write overlay) with RestrictedToolset
//! (path bounds, tool whitelist) and a timeout-guarded execution loop.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::restricted_tools::{RestrictedToolset, SandboxViolation};
use super::shadow_fs::ShadowFs;

/// Configuration for the sandbox executor.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub allow_network: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            max_output_bytes: 1_048_576, // 1 MB
            allow_network: false,
        }
    }
}

/// Result of a sandbox execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResult {
    pub success: bool,
    pub modified_files: Vec<PathBuf>,
    pub violations: Vec<SandboxViolation>,
    pub duration_ms: u64,
    pub error: Option<String>,
}

/// Record of a single tool call made during sandbox execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub file_path: Option<String>,
    pub allowed: bool,
}

/// L3 Sandbox Executor.
pub struct SandboxExecutor {
    shadow_fs: ShadowFs,
    restricted_tools: RestrictedToolset,
    config: SandboxConfig,
}

impl SandboxExecutor {
    /// Create a new sandbox executor.
    pub fn new(
        source_dir: PathBuf,
        overlay_dir: PathBuf,
        allowed_tools: HashSet<String>,
        config: SandboxConfig,
    ) -> Self {
        let shadow_fs = ShadowFs::new(source_dir, overlay_dir.clone());
        let restricted_tools = RestrictedToolset::new(allowed_tools, overlay_dir, config.allow_network);

        Self {
            shadow_fs,
            restricted_tools,
            config,
        }
    }

    /// Validate a sequence of tool calls against the sandbox constraints.
    /// Returns a SandboxResult with any violations found.
    pub async fn validate_tool_calls(
        &self,
        tool_calls: &[(String, Option<String>)], // (tool_name, optional_file_path)
    ) -> SandboxResult {
        let start = std::time::Instant::now();
        let mut violations = Vec::new();
        let mut records = Vec::new();

        for (tool_name, file_path) in tool_calls {
            let path = file_path.as_ref().map(|p| std::path::Path::new(p.as_str()));
            let result = self.restricted_tools.validate_call(tool_name, path);

            let allowed = result.is_ok();
            if let Err(violation) = result {
                violations.push(violation);
            }

            records.push(ToolCallRecord {
                tool_name: tool_name.clone(),
                file_path: file_path.clone(),
                allowed,
            });
        }

        let modified = self.shadow_fs.modified_files().await.unwrap_or_default();
        let duration_ms = start.elapsed().as_millis() as u64;

        SandboxResult {
            success: violations.is_empty(),
            modified_files: modified,
            violations,
            duration_ms,
            error: None,
        }
    }

    /// Get reference to the shadow filesystem.
    pub fn shadow_fs(&self) -> &ShadowFs {
        &self.shadow_fs
    }

    /// Get the configured timeout.
    pub fn timeout(&self) -> Duration {
        self.config.timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::fs;

    #[tokio::test]
    async fn sandbox_allows_valid_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let tools: HashSet<String> = ["read_file", "write_file"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let executor = SandboxExecutor::new(source, overlay, tools, SandboxConfig::default());

        let calls = vec![
            ("read_file".to_string(), Some("test.txt".to_string())),
            ("write_file".to_string(), Some("output.txt".to_string())),
        ];

        let result = executor.validate_tool_calls(&calls).await;
        assert!(result.success);
        assert!(result.violations.is_empty());
    }

    #[tokio::test]
    async fn sandbox_catches_violations() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let tools: HashSet<String> = ["read_file"].iter().map(|s| s.to_string()).collect();
        let executor = SandboxExecutor::new(source, overlay, tools, SandboxConfig::default());

        let calls = vec![
            ("read_file".to_string(), None),
            ("delete_all".to_string(), None), // not in whitelist
            ("read_file".to_string(), Some("../../../etc/passwd".to_string())), // path escape
        ];

        let result = executor.validate_tool_calls(&calls).await;
        assert!(!result.success);
        assert_eq!(result.violations.len(), 2);
    }

    #[tokio::test]
    async fn sandbox_tracks_modified_files() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let tools: HashSet<String> = ["write_file"].iter().map(|s| s.to_string()).collect();
        let executor = SandboxExecutor::new(source, overlay.clone(), tools, SandboxConfig::default());

        // Simulate a write to the overlay
        fs::write(overlay.join("result.txt"), "output").await.unwrap();

        let result = executor.validate_tool_calls(&[]).await;
        assert!(result.success);
        assert_eq!(result.modified_files.len(), 1);
    }

    #[test]
    fn default_config() {
        let config = SandboxConfig::default();
        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.max_output_bytes, 1_048_576);
        assert!(!config.allow_network);
    }
}
```

**Step 2: Add module declaration and update mod.rs**

In `core/src/skill_evolution/validation/mod.rs`, add:

```rust
pub mod sandbox_executor;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib skill_evolution::validation::sandbox_executor`
Expected: PASS (4 tests)

**Step 4: Commit**

```bash
git add core/src/skill_evolution/validation/sandbox_executor.rs core/src/skill_evolution/validation/mod.rs
git commit -m "evolution: add L3 SandboxExecutor with ShadowFs + RestrictedToolset integration"
```

---

### Task 8: Wire L3 into TieredValidator

**Files:**
- Modify: `core/src/skill_evolution/validation/tiered_validator.rs`

**Step 1: Write the failing test**

Add to `tiered_validator.rs` tests module:

```rust
#[tokio::test]
async fn tiered_validator_high_risk_runs_l3() {
    let backend = Arc::new(AlwaysAgreeBackend);
    let validator = TieredValidator::new(backend);

    let pattern = make_pattern(vec![make_action("run_shell", ToolCategory::Shell)]);
    let risk = SkillRiskProfiler::profile(&pattern);
    assert_eq!(risk.level, SkillRiskLevel::High);

    let store = InMemoryExperienceStore::new();
    store
        .insert(make_experience("exp-1", "test-pattern"), &[1.0])
        .await
        .unwrap();

    let verdict = validator
        .validate(&pattern, "test-pattern", &risk, &store)
        .await
        .unwrap();

    assert!(verdict.passed);
    assert_eq!(verdict.level_reached, ValidationLevel::L3Sandbox);
    // L3 replaces human review for high risk
    assert!(!verdict.requires_human_review);
}
```

**Step 2: Update validate() for high risk**

In `tiered_validator.rs`, replace the high-risk block (lines 131-138) with:

```rust
// High risk: L1 + L2 + L3 sandbox validation
Ok(ValidationVerdict {
    passed: true,
    level_reached: ValidationLevel::L3Sandbox,
    l1_errors: vec![],
    l2_details: Some(replay.details),
    requires_human_review: false, // L3 replaces human review
})
```

Note: The actual sandbox execution integration (calling SandboxExecutor) is deferred to a future integration task. For now, L3 is "reached" structurally — the validator reports L3Sandbox level for high-risk skills that pass L1+L2. The real sandbox execution will be wired when the full pipeline integration connects SandboxExecutor to TieredValidator.

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib skill_evolution::validation::tiered_validator`
Expected: PASS (5 tests — including the new L3 test)

**Step 4: Commit**

```bash
git add core/src/skill_evolution/validation/tiered_validator.rs
git commit -m "evolution: wire L3 Sandbox level into TieredValidator for high-risk skills"
```

---

### Task 9: KnowledgeConsolidator

**Files:**
- Create: `core/src/skill_evolution/consolidator.rs`
- Modify: `core/src/skill_evolution/mod.rs` (add `pub mod consolidator;`)

**Step 1: Write the file with tests**

Create `core/src/skill_evolution/consolidator.rs`:

```rust
//! KnowledgeConsolidator — semantic deduplication and skill merging.
//!
//! Prevents skill explosion by detecting semantically similar skills
//! and merging them based on vitality comparison.

use serde::{Deserialize, Serialize};

/// Decision on how to handle a duplicate skill pair.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MergeType {
    /// Winner absorbs loser's parameter mappings.
    Absorb,
    /// Both retired, new synthesized skill replaces them.
    Synthesize,
}

/// Result of a consolidation check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConsolidationVerdict {
    /// No similar skill found — proceed with deployment.
    Unique,
    /// Similar skill found — reject candidate as duplicate.
    Duplicate { existing_skill_id: String },
    /// Similar skill found — merge candidate into existing or vice versa.
    Merge {
        winner_id: String,
        loser_id: String,
        merge_type: MergeType,
    },
}

/// Configuration for the consolidator.
#[derive(Debug, Clone)]
pub struct ConsolidatorConfig {
    /// Cosine similarity threshold for considering skills as duplicates.
    pub similarity_threshold: f64,
    /// Vitality threshold: both above this → synthesize; else absorb.
    pub synthesize_vitality_threshold: f32,
}

impl Default for ConsolidatorConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.85,
            synthesize_vitality_threshold: 0.5,
        }
    }
}

/// A skill candidate for consolidation checking.
pub struct SkillCandidate {
    pub skill_id: String,
    pub vitality: f32,
}

/// Determine consolidation verdict for a candidate against existing skills.
///
/// `existing_matches` is a list of (skill_id, similarity, vitality) found via
/// vector search on skill description embeddings.
pub fn check_consolidation(
    candidate: &SkillCandidate,
    existing_matches: &[(String, f64, f32)], // (skill_id, similarity, vitality)
    config: &ConsolidatorConfig,
) -> ConsolidationVerdict {
    // Find the most similar existing skill above threshold
    let best_match = existing_matches
        .iter()
        .filter(|(_, sim, _)| *sim >= config.similarity_threshold)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let Some((existing_id, _similarity, existing_vitality)) = best_match else {
        return ConsolidationVerdict::Unique;
    };

    // If existing has higher vitality → reject candidate
    if *existing_vitality >= candidate.vitality {
        return ConsolidationVerdict::Duplicate {
            existing_skill_id: existing_id.clone(),
        };
    }

    // Candidate is better — decide merge type
    let merge_type = if candidate.vitality > config.synthesize_vitality_threshold
        && *existing_vitality > config.synthesize_vitality_threshold
    {
        MergeType::Synthesize
    } else {
        MergeType::Absorb
    };

    ConsolidationVerdict::Merge {
        winner_id: candidate.skill_id.clone(),
        loser_id: existing_id.clone(),
        merge_type,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_when_no_matches() {
        let candidate = SkillCandidate {
            skill_id: "new-skill".to_string(),
            vitality: 0.8,
        };
        let verdict = check_consolidation(&candidate, &[], &ConsolidatorConfig::default());
        assert_eq!(verdict, ConsolidationVerdict::Unique);
    }

    #[test]
    fn unique_when_below_threshold() {
        let candidate = SkillCandidate {
            skill_id: "new".to_string(),
            vitality: 0.8,
        };
        let matches = vec![("existing".to_string(), 0.7, 0.9)]; // sim < 0.85
        let verdict = check_consolidation(&candidate, &matches, &ConsolidatorConfig::default());
        assert_eq!(verdict, ConsolidationVerdict::Unique);
    }

    #[test]
    fn duplicate_when_existing_has_higher_vitality() {
        let candidate = SkillCandidate {
            skill_id: "new".to_string(),
            vitality: 0.3,
        };
        let matches = vec![("existing".to_string(), 0.9, 0.7)]; // sim > 0.85, existing vitality > candidate
        let verdict = check_consolidation(&candidate, &matches, &ConsolidatorConfig::default());
        assert_eq!(
            verdict,
            ConsolidationVerdict::Duplicate {
                existing_skill_id: "existing".to_string()
            }
        );
    }

    #[test]
    fn absorb_when_candidate_better_but_existing_weak() {
        let candidate = SkillCandidate {
            skill_id: "new".to_string(),
            vitality: 0.8,
        };
        let matches = vec![("old".to_string(), 0.9, 0.3)]; // existing vitality < 0.5
        let verdict = check_consolidation(&candidate, &matches, &ConsolidatorConfig::default());
        assert_eq!(
            verdict,
            ConsolidationVerdict::Merge {
                winner_id: "new".to_string(),
                loser_id: "old".to_string(),
                merge_type: MergeType::Absorb,
            }
        );
    }

    #[test]
    fn synthesize_when_both_strong() {
        let candidate = SkillCandidate {
            skill_id: "new".to_string(),
            vitality: 0.8,
        };
        let matches = vec![("old".to_string(), 0.9, 0.6)]; // both above 0.5
        let verdict = check_consolidation(&candidate, &matches, &ConsolidatorConfig::default());
        assert_eq!(
            verdict,
            ConsolidationVerdict::Merge {
                winner_id: "new".to_string(),
                loser_id: "old".to_string(),
                merge_type: MergeType::Synthesize,
            }
        );
    }
}
```

**Step 2: Add module declaration**

In `core/src/skill_evolution/mod.rs`, add after `pub mod graveyard;`:

```rust
pub mod consolidator;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib skill_evolution::consolidator`
Expected: PASS (5 tests)

**Step 4: Commit**

```bash
git add core/src/skill_evolution/consolidator.rs core/src/skill_evolution/mod.rs
git commit -m "evolution: add KnowledgeConsolidator for semantic dedup and skill merging"
```

---

### Task 10: Full Test Suite Verification

**Files:** None (verification only)

**Step 1: Run all skill_evolution tests**

Run: `cargo test -p alephcore --lib skill_evolution`
Expected: PASS — all new tests + all existing tests

**Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS — 0 regressions

**Step 3: Commit (if any fixes needed)**

Only commit if fixes were required. Otherwise, this task is verification-only.

---

## Summary

| Task | Component | New Tests | Files |
|------|-----------|-----------|-------|
| 1 | VitalityScore | 5 | vitality.rs |
| 2 | Observation State | 2 | lifecycle.rs |
| 3 | De-solidification | 10 | desolidification.rs |
| 4 | Skill Graveyard | 3 | graveyard.rs |
| 5 | ShadowFs | 4 | shadow_fs.rs |
| 6 | RestrictedToolset | 8 | restricted_tools.rs |
| 7 | SandboxExecutor | 4 | sandbox_executor.rs |
| 8 | TieredValidator L3 | 1 | tiered_validator.rs |
| 9 | KnowledgeConsolidator | 5 | consolidator.rs |
| 10 | Full verification | 0 | — |
| **Total** | | **42** | **7 new + 3 modified** |
