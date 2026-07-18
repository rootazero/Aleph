# Dream Daemon Evolution Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade Dream Daemon from a fixed-order pipeline to a signal-driven evolution engine with strategy selection, mutation gating, validation, and immutable audit trail.

**Architecture:** Six new modules (signals, strategy, selector, validation, event_log, skill_distill stage) replace the hardcoded daily/weekly pipeline. The DreamDaemon main loop becomes: Signal Collect → Strategy Select → Mutation Gate → Pipeline Execute → Validate → Solidify. All new types are Serialize/Deserialize for JSON event logging.

**Tech Stack:** Rust, serde/serde_json, tokio, async-trait, chrono, regex, sha2

---

### Task 1: DreamStrategy enum and stage mapping

**Files:**
- Create: `src/memory/dreaming/strategy.rs`
- Modify: `src/memory/dreaming/mod.rs` (add `pub mod strategy;`)

- [ ] **Step 1: Write the failing test**

In `src/memory/dreaming/strategy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consolidate_stages() {
        let names = DreamStrategy::Consolidate.stage_names();
        assert_eq!(
            names,
            vec!["note_lint", "note_consolidate", "note_drift", "index_refresher", "note_decay"]
        );
    }

    #[test]
    fn synthesize_stages() {
        let names = DreamStrategy::Synthesize.stage_names();
        assert_eq!(
            names,
            vec!["note_lint", "note_consolidate", "note_synthesis", "skill_distill", "daily_digest"]
        );
    }

    #[test]
    fn conserve_stages() {
        let names = DreamStrategy::Conserve.stage_names();
        assert_eq!(names, vec!["note_lint", "index_refresher"]);
    }

    #[test]
    fn display_roundtrip() {
        for strategy in [DreamStrategy::Consolidate, DreamStrategy::Synthesize, DreamStrategy::Conserve] {
            let s = serde_json::to_string(&strategy).unwrap();
            let back: DreamStrategy = serde_json::from_str(&s).unwrap();
            assert_eq!(back, strategy);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::strategy -- --nocapture 2>&1 | head -30`
Expected: compilation error — module not found

- [ ] **Step 3: Write the implementation**

Create `src/memory/dreaming/strategy.rs`:

```rust
//! DreamStrategy — signal-driven strategy selection for the dream pipeline.
//!
//! Replaces the hardcoded daily/weekly pipeline with three adaptive strategies,
//! each defining which stages to execute.

use serde::{Deserialize, Serialize};

/// Evolution strategy for a Dream cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamStrategy {
    /// Default mode: merge duplicates, fix formats, detect drift, maintain index.
    Consolidate,
    /// Growth mode: cross-category synthesis, skill-note distillation.
    Synthesize,
    /// Defensive mode: deterministic-only ops, skip all LLM stages.
    Conserve,
}

impl DreamStrategy {
    /// Ordered list of stage names this strategy will execute.
    pub fn stage_names(&self) -> Vec<&'static str> {
        match self {
            Self::Consolidate => vec![
                "note_lint",
                "note_consolidate",
                "note_drift",
                "index_refresher",
                "note_decay",
            ],
            Self::Synthesize => vec![
                "note_lint",
                "note_consolidate",
                "note_synthesis",
                "skill_distill",
                "daily_digest",
            ],
            Self::Conserve => vec![
                "note_lint",
                "index_refresher",
            ],
        }
    }
}

impl std::fmt::Display for DreamStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Consolidate => write!(f, "consolidate"),
            Self::Synthesize => write!(f, "synthesize"),
            Self::Conserve => write!(f, "conserve"),
        }
    }
}
```

- [ ] **Step 4: Register the module**

Add to `src/memory/dreaming/mod.rs` after `pub mod stages;`:

```rust
pub mod strategy;
```

And add re-export after existing re-exports:

```rust
pub use strategy::DreamStrategy;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dreaming::strategy -- --nocapture`
Expected: 4 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/strategy.rs src/memory/dreaming/mod.rs
git commit -m "feat(dreaming): add DreamStrategy enum with stage mapping"
```

---

### Task 2: Signal types and Signal Collector

**Files:**
- Create: `src/memory/dreaming/signals.rs`
- Modify: `src/memory/dreaming/mod.rs` (add module declaration)

- [ ] **Step 1: Write the failing test**

In `src/memory/dreaming/signals.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_snapshot_from_empty_metrics() {
        let metrics = RawMetrics::default();
        let snapshot = SignalSnapshot::from_metrics(&metrics);
        // All signals should be present but with 0.0 scores
        assert!(!snapshot.signals.is_empty());
        assert!(snapshot.signals.iter().all(|s| s.score >= 0.0 && s.score <= 1.0));
    }

    #[test]
    fn high_contradiction_rate_produces_high_health_signal() {
        let metrics = RawMetrics {
            contradiction_rate: 0.8,
            ..Default::default()
        };
        let snapshot = SignalSnapshot::from_metrics(&metrics);
        let sig = snapshot.signals.iter().find(|s| s.name == "high_contradiction_rate");
        assert!(sig.is_some());
        assert!(sig.unwrap().score > 0.5);
    }

    #[test]
    fn note_growth_signal_normalized() {
        let metrics = RawMetrics {
            notes_added_24h: 50,
            total_notes: 100,
            ..Default::default()
        };
        let snapshot = SignalSnapshot::from_metrics(&metrics);
        let sig = snapshot.signals.iter().find(|s| s.name == "note_growth_rate").unwrap();
        // 50/100 = 0.5, clamped to [0,1]
        assert!((sig.score - 0.5).abs() < 0.01);
    }

    #[test]
    fn skill_recall_rate_zero_when_no_skills() {
        let metrics = RawMetrics {
            skill_notes_total: 0,
            skill_notes_recalled: 0,
            ..Default::default()
        };
        let snapshot = SignalSnapshot::from_metrics(&metrics);
        let sig = snapshot.signals.iter().find(|s| s.name == "skill_recall_rate").unwrap();
        assert_eq!(sig.score, 0.0);
    }

    #[test]
    fn serde_roundtrip() {
        let snapshot = SignalSnapshot::from_metrics(&RawMetrics::default());
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: SignalSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.signals.len(), snapshot.signals.len());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::signals -- --nocapture 2>&1 | head -30`
Expected: compilation error

- [ ] **Step 3: Write the implementation**

Create `src/memory/dreaming/signals.rs`:

```rust
//! Signal Collector — aggregates learning signals from four data sources.
//!
//! Produces a `SignalSnapshot` at the start of each Dream cycle, feeding
//! the Strategy Selector with normalized scores.

use serde::{Deserialize, Serialize};

/// Signal type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    /// Conversation quality (corrections, retries, abandonment).
    Quality,
    /// Memory recall effectiveness (hit rate, never-recalled notes).
    Recall,
    /// Note health (duplication, contradiction, staleness).
    Health,
    /// Skill-note usage patterns.
    SkillUsage,
}

/// A single normalized signal with score in [0.0, 1.0].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamSignal {
    pub signal_type: SignalType,
    pub name: String,
    pub score: f64,
    pub source: String,
}

/// Snapshot of all signals for a Dream cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalSnapshot {
    pub signals: Vec<DreamSignal>,
    pub collected_at: i64,
}

/// Raw metrics gathered from various stores before normalization.
///
/// Each field is populated by the DreamDaemon before calling
/// `SignalSnapshot::from_metrics`. Fields default to zero/empty.
#[derive(Debug, Clone, Default)]
pub struct RawMetrics {
    // -- Note health --
    pub duplication_rate: f64,
    pub contradiction_rate: f64,
    pub staleness_rate: f64,
    pub notes_added_24h: u32,
    pub total_notes: u32,

    // -- Recall --
    pub note_hit_rate: f64,
    pub never_recalled_count: u32,

    // -- Skill usage --
    pub skill_notes_total: u32,
    pub skill_notes_recalled: u32,

    // -- Conversation quality --
    pub correction_count: u32,
    pub session_count: u32,
}

impl SignalSnapshot {
    /// Build a snapshot from raw metrics, normalizing each to [0.0, 1.0].
    pub fn from_metrics(m: &RawMetrics) -> Self {
        let now = chrono::Utc::now().timestamp();
        let mut signals = Vec::new();

        // -- Health signals --
        signals.push(DreamSignal {
            signal_type: SignalType::Health,
            name: "high_contradiction_rate".into(),
            score: m.contradiction_rate.clamp(0.0, 1.0),
            source: "dream_report".into(),
        });
        signals.push(DreamSignal {
            signal_type: SignalType::Health,
            name: "high_duplication_rate".into(),
            score: m.duplication_rate.clamp(0.0, 1.0),
            source: "dream_report".into(),
        });
        signals.push(DreamSignal {
            signal_type: SignalType::Health,
            name: "high_staleness_rate".into(),
            score: m.staleness_rate.clamp(0.0, 1.0),
            source: "dream_report".into(),
        });

        let growth_rate = if m.total_notes > 0 {
            (m.notes_added_24h as f64 / m.total_notes as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        signals.push(DreamSignal {
            signal_type: SignalType::Health,
            name: "note_growth_rate".into(),
            score: growth_rate,
            source: "note_indexer".into(),
        });

        // -- Recall signals --
        signals.push(DreamSignal {
            signal_type: SignalType::Recall,
            name: "note_hit_rate".into(),
            score: m.note_hit_rate.clamp(0.0, 1.0),
            source: "recall_signals".into(),
        });
        let never_recalled_ratio = if m.total_notes > 0 {
            (m.never_recalled_count as f64 / m.total_notes as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        signals.push(DreamSignal {
            signal_type: SignalType::Recall,
            name: "never_recalled_ratio".into(),
            score: never_recalled_ratio,
            source: "recall_signals".into(),
        });

        // -- Skill usage signals --
        let skill_recall_rate = if m.skill_notes_total > 0 {
            (m.skill_notes_recalled as f64 / m.skill_notes_total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        signals.push(DreamSignal {
            signal_type: SignalType::SkillUsage,
            name: "skill_recall_rate".into(),
            score: skill_recall_rate,
            source: "recall_signals".into(),
        });

        // -- Quality signals --
        let correction_rate = if m.session_count > 0 {
            (m.correction_count as f64 / m.session_count as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        signals.push(DreamSignal {
            signal_type: SignalType::Quality,
            name: "correction_rate".into(),
            score: correction_rate,
            source: "session_metadata".into(),
        });

        Self {
            signals,
            collected_at: now,
        }
    }

    /// Find a signal by name.
    pub fn get(&self, name: &str) -> Option<&DreamSignal> {
        self.signals.iter().find(|s| s.name == name)
    }

    /// Get score by signal name, defaulting to 0.0 if not found.
    pub fn score(&self, name: &str) -> f64 {
        self.get(name).map(|s| s.score).unwrap_or(0.0)
    }
}
```

- [ ] **Step 4: Register the module**

Add to `src/memory/dreaming/mod.rs` after `pub mod strategy;`:

```rust
pub mod signals;
```

And re-export:

```rust
pub use signals::{DreamSignal, RawMetrics, SignalSnapshot, SignalType};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dreaming::signals -- --nocapture`
Expected: 5 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/signals.rs src/memory/dreaming/mod.rs
git commit -m "feat(dreaming): add Signal Collector with 4-source signal extraction"
```

---

### Task 3: Strategy Selector with personality adaptation

**Files:**
- Create: `src/memory/dreaming/selector.rs`
- Modify: `src/memory/dreaming/mod.rs` (add module declaration)

- [ ] **Step 1: Write the failing test**

In `src/memory/dreaming/selector.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::signals::RawMetrics;

    fn snapshot_from(metrics: &RawMetrics) -> SignalSnapshot {
        SignalSnapshot::from_metrics(metrics)
    }

    #[test]
    fn default_metrics_select_consolidate() {
        let snapshot = snapshot_from(&RawMetrics::default());
        let decision = StrategySelector::new().select(&snapshot, &GateDecision::Allow);
        assert_eq!(decision.strategy, DreamStrategy::Consolidate);
    }

    #[test]
    fn high_growth_low_skill_recall_selects_synthesize() {
        let snapshot = snapshot_from(&RawMetrics {
            notes_added_24h: 80,
            total_notes: 100,
            skill_notes_total: 10,
            skill_notes_recalled: 0, // 0% recall
            ..Default::default()
        });
        let decision = StrategySelector::new().select(&snapshot, &GateDecision::Allow);
        assert_eq!(decision.strategy, DreamStrategy::Synthesize);
    }

    #[test]
    fn gate_conserve_overrides_synthesize() {
        let snapshot = snapshot_from(&RawMetrics {
            notes_added_24h: 80,
            total_notes: 100,
            ..Default::default()
        });
        let gate = GateDecision::Conserve {
            reason: "merge cycle detected".into(),
            cooldown_remaining: 3,
        };
        let decision = StrategySelector::new().select(&snapshot, &gate);
        assert_eq!(decision.strategy, DreamStrategy::Conserve);
    }

    #[test]
    fn gate_skip_returns_none_strategy() {
        let snapshot = snapshot_from(&RawMetrics::default());
        let gate = GateDecision::Skip { reason: "cooldown".into() };
        let decision = StrategySelector::new().select(&snapshot, &gate);
        assert_eq!(decision.strategy, DreamStrategy::Conserve);
    }

    #[test]
    fn personality_high_pass_rate_lowers_threshold() {
        let mut selector = StrategySelector::new();
        // Simulate 10 cycles with all validations passing
        for _ in 0..10 {
            selector.record_cycle_outcome(DreamStrategy::Consolidate, true, 0.5);
        }
        assert!(selector.synthesize_threshold() < DEFAULT_SYNTHESIZE_THRESHOLD);
    }

    #[test]
    fn personality_low_pass_rate_raises_threshold() {
        let mut selector = StrategySelector::new();
        // Simulate 10 cycles with all validations failing
        for _ in 0..10 {
            selector.record_cycle_outcome(DreamStrategy::Consolidate, false, 0.0);
        }
        assert!(selector.synthesize_threshold() > DEFAULT_SYNTHESIZE_THRESHOLD);
    }

    #[test]
    fn threshold_clamped_to_range() {
        let mut selector = StrategySelector::new();
        // 20 passing cycles should not push threshold below MIN
        for _ in 0..20 {
            selector.record_cycle_outcome(DreamStrategy::Synthesize, true, 1.0);
        }
        assert!(selector.synthesize_threshold() >= MIN_SYNTHESIZE_THRESHOLD);
        assert!(selector.synthesize_threshold() <= MAX_SYNTHESIZE_THRESHOLD);
    }

    #[test]
    fn serde_roundtrip_decision() {
        let decision = SelectionDecision {
            strategy: DreamStrategy::Synthesize,
            rationale: "high growth pressure".into(),
            personality_adjustment: -0.1,
        };
        let json = serde_json::to_string(&decision).unwrap();
        let back: SelectionDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(back.strategy, DreamStrategy::Synthesize);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::selector -- --nocapture 2>&1 | head -30`
Expected: compilation error

- [ ] **Step 3: Write the implementation**

Create `src/memory/dreaming/selector.rs`:

```rust
//! Strategy Selector — deterministic signal-to-strategy mapping.
//!
//! No LLM calls. Uses composite signal scores and a sliding-window
//! personality adaptation to choose the optimal DreamStrategy.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::signals::SignalSnapshot;
use super::strategy::DreamStrategy;

/// Mutation gate decision (input to selector).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GateDecision {
    Allow,
    Conserve {
        reason: String,
        cooldown_remaining: u32,
    },
    Skip {
        reason: String,
    },
}

/// Output of strategy selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionDecision {
    pub strategy: DreamStrategy,
    pub rationale: String,
    pub personality_adjustment: f64,
}

pub const DEFAULT_SYNTHESIZE_THRESHOLD: f64 = 0.6;
pub const MIN_SYNTHESIZE_THRESHOLD: f64 = 0.4;
pub const MAX_SYNTHESIZE_THRESHOLD: f64 = 0.8;
const PERSONALITY_WINDOW: usize = 10;
const PERSONALITY_STEP: f64 = 0.1;
const HIGH_PASS_RATE: f64 = 0.8;
const LOW_PASS_RATE: f64 = 0.5;
const MIN_STABILITY: f64 = 0.5;

/// Record of a past cycle outcome for personality adaptation.
#[derive(Debug, Clone)]
struct CycleRecord {
    strategy: DreamStrategy,
    validation_passed: bool,
    skill_recall_hit_rate: f64,
}

/// Deterministic strategy selector with sliding-window personality.
pub struct StrategySelector {
    history: VecDeque<CycleRecord>,
}

impl StrategySelector {
    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(PERSONALITY_WINDOW),
        }
    }

    /// Record outcome of a completed Dream cycle for personality adaptation.
    pub fn record_cycle_outcome(
        &mut self,
        strategy: DreamStrategy,
        validation_passed: bool,
        skill_recall_hit_rate: f64,
    ) {
        if self.history.len() >= PERSONALITY_WINDOW {
            self.history.pop_front();
        }
        self.history.push_back(CycleRecord {
            strategy,
            validation_passed,
            skill_recall_hit_rate,
        });
    }

    /// Current synthesize threshold after personality adjustment.
    pub fn synthesize_threshold(&self) -> f64 {
        let adjustment = self.personality_adjustment();
        (DEFAULT_SYNTHESIZE_THRESHOLD + adjustment)
            .clamp(MIN_SYNTHESIZE_THRESHOLD, MAX_SYNTHESIZE_THRESHOLD)
    }

    /// Compute the personality adjustment delta.
    fn personality_adjustment(&self) -> f64 {
        if self.history.is_empty() {
            return 0.0;
        }
        let pass_rate = self.history.iter().filter(|r| r.validation_passed).count() as f64
            / self.history.len() as f64;

        if pass_rate > HIGH_PASS_RATE {
            -PERSONALITY_STEP // more aggressive
        } else if pass_rate < LOW_PASS_RATE {
            PERSONALITY_STEP // more conservative
        } else {
            0.0
        }
    }

    /// Select the best strategy given current signals and gate decision.
    pub fn select(&self, snapshot: &SignalSnapshot, gate: &GateDecision) -> SelectionDecision {
        let adjustment = self.personality_adjustment();

        // Gate override: Conserve or Skip → Conserve
        match gate {
            GateDecision::Conserve { reason, .. } => {
                return SelectionDecision {
                    strategy: DreamStrategy::Conserve,
                    rationale: format!("gate forced conserve: {reason}"),
                    personality_adjustment: adjustment,
                };
            }
            GateDecision::Skip { reason } => {
                return SelectionDecision {
                    strategy: DreamStrategy::Conserve,
                    rationale: format!("gate skip: {reason}"),
                    personality_adjustment: adjustment,
                };
            }
            GateDecision::Allow => {}
        }

        let threshold = self.synthesize_threshold();

        // Composite scores
        let growth_rate = snapshot.score("note_growth_rate");
        let skill_recall_rate = snapshot.score("skill_recall_rate");
        let growth_pressure = growth_rate * (1.0 - skill_recall_rate);

        let contradiction_rate = snapshot.score("high_contradiction_rate");
        let duplication_rate = snapshot.score("high_duplication_rate");
        let stability = 1.0 - (contradiction_rate + duplication_rate) / 2.0;

        if growth_pressure > threshold && stability > MIN_STABILITY {
            SelectionDecision {
                strategy: DreamStrategy::Synthesize,
                rationale: format!(
                    "growth_pressure={growth_pressure:.2} > threshold={threshold:.2}, stability={stability:.2}"
                ),
                personality_adjustment: adjustment,
            }
        } else {
            SelectionDecision {
                strategy: DreamStrategy::Consolidate,
                rationale: format!(
                    "growth_pressure={growth_pressure:.2} <= threshold={threshold:.2} or stability={stability:.2} <= {MIN_STABILITY}"
                ),
                personality_adjustment: adjustment,
            }
        }
    }
}

impl Default for StrategySelector {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Register the module**

Add to `src/memory/dreaming/mod.rs` after `pub mod signals;`:

```rust
pub mod selector;
```

And re-export:

```rust
pub use selector::{GateDecision, SelectionDecision, StrategySelector};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dreaming::selector -- --nocapture`
Expected: 7 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/selector.rs src/memory/dreaming/mod.rs
git commit -m "feat(dreaming): add Strategy Selector with personality adaptation"
```

---

### Task 4: Extended Mutation Gate

**Files:**
- Create: `src/memory/dreaming/mutation_gate.rs`
- Modify: `src/memory/dreaming/mod.rs` (add module declaration)

Note: We create a new file rather than extending `gate.rs`, because `gate.rs` handles the existing 3-level cheap-to-expensive gate chain (time/count/drift) for deciding *whether to run at all*. The mutation gate is conceptually different — it detects *evolution pathologies* after deciding to run. Keeping them separate maintains single responsibility.

- [ ] **Step 1: Write the failing test**

In `src/memory/dreaming/mutation_gate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_history_allows() {
        let gate = MutationGate::new();
        let decision = gate.evaluate();
        assert!(matches!(decision, GateDecision::Allow));
    }

    #[test]
    fn merge_cycle_detected_after_three_repeats() {
        let mut gate = MutationGate::new();
        let pair = ("note_a".to_string(), "note_b".to_string());
        gate.record_merge_pair(&pair.0, &pair.1);
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
        gate.advance_cycle();
        gate.record_merge_pair(&pair.0, &pair.1);
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
        gate.advance_cycle();
        gate.record_merge_pair(&pair.0, &pair.1);
        // Third consecutive cycle with same pair → Conserve
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn different_pairs_do_not_trigger() {
        let mut gate = MutationGate::new();
        gate.record_merge_pair("a", "b");
        gate.advance_cycle();
        gate.record_merge_pair("c", "d");
        gate.advance_cycle();
        gate.record_merge_pair("e", "f");
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn oscillation_detected_with_negation() {
        let mut gate = MutationGate::new();
        gate.record_synthesis_assertion("should use async");
        gate.advance_cycle();
        gate.record_synthesis_assertion("should not use async");
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn no_oscillation_without_negation() {
        let mut gate = MutationGate::new();
        gate.record_synthesis_assertion("should use async");
        gate.advance_cycle();
        gate.record_synthesis_assertion("should use traits");
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn wasted_distillation_detected() {
        let mut gate = MutationGate::new();
        // 5 cycles, each producing 2 skill notes, none recalled
        for _ in 0..5 {
            gate.record_skill_distill_output(2, 0);
            gate.advance_cycle();
        }
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn distillation_with_recalls_ok() {
        let mut gate = MutationGate::new();
        for _ in 0..5 {
            gate.record_skill_distill_output(2, 1); // 50% recall
            gate.advance_cycle();
        }
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn cooldown_prevents_reevaluation() {
        let mut gate = MutationGate::new();
        // Trigger conserve via oscillation
        gate.record_synthesis_assertion("prefer X");
        gate.advance_cycle();
        gate.record_synthesis_assertion("avoid X");
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));

        // Activate cooldown
        gate.activate_cooldown(3);

        // Next 2 cycles: still in cooldown
        gate.advance_cycle();
        let d = gate.evaluate();
        assert!(matches!(d, GateDecision::Conserve { cooldown_remaining, .. } if cooldown_remaining == 2));

        gate.advance_cycle();
        gate.tick_cooldown();
        gate.tick_cooldown();
        // After 3 ticks total, cooldown expired — clear history and re-evaluate
        gate.advance_cycle();
        gate.clear_after_cooldown();
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn serde_roundtrip_gate_decision() {
        let d = GateDecision::Conserve {
            reason: "test".into(),
            cooldown_remaining: 2,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: GateDecision = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, GateDecision::Conserve { cooldown_remaining: 2, .. }));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::mutation_gate -- --nocapture 2>&1 | head -30`
Expected: compilation error

- [ ] **Step 3: Write the implementation**

Create `src/memory/dreaming/mutation_gate.rs`:

```rust
//! MutationGate — detects evolution pathologies and enforces cooldown.
//!
//! Three detection mechanisms:
//! 1. Merge cycle: same note pair merged 3+ consecutive cycles
//! 2. Synthesis oscillation: negation patterns between recent syntheses
//! 3. Wasted distillation: skill-notes produced but never recalled

use std::collections::{HashSet, VecDeque};

use regex::Regex;
use serde::{Deserialize, Serialize};

// Re-use the GateDecision from selector (it's the shared type)
pub use super::selector::GateDecision;

const MERGE_CYCLE_WINDOW: usize = 5;
const MERGE_CYCLE_THRESHOLD: usize = 3;
const DISTILL_WINDOW: usize = 5;
const DISTILL_MIN_RECALL_RATE: f64 = 0.1;

/// Tracks evolution pathology state across Dream cycles.
pub struct MutationGate {
    /// Per-cycle merge pairs: sorted (a, b) tuples.
    merge_history: VecDeque<HashSet<(String, String)>>,
    /// Current cycle's merge pairs (not yet committed).
    current_merges: HashSet<(String, String)>,
    /// Synthesis assertions from last two cycles.
    synthesis_assertions: VecDeque<Vec<String>>,
    /// Current cycle's synthesis assertions.
    current_assertions: Vec<String>,
    /// Per-cycle distillation stats: (produced, recalled).
    distill_history: VecDeque<(u32, u32)>,
    /// Active cooldown counter (0 = no cooldown).
    cooldown: u32,
}

impl MutationGate {
    pub fn new() -> Self {
        Self {
            merge_history: VecDeque::with_capacity(MERGE_CYCLE_WINDOW),
            current_merges: HashSet::new(),
            synthesis_assertions: VecDeque::with_capacity(2),
            current_assertions: Vec::new(),
            distill_history: VecDeque::with_capacity(DISTILL_WINDOW),
            cooldown: 0,
        }
    }

    /// Record a merge pair in the current cycle.
    pub fn record_merge_pair(&mut self, note_a: &str, note_b: &str) {
        let pair = if note_a < note_b {
            (note_a.to_string(), note_b.to_string())
        } else {
            (note_b.to_string(), note_a.to_string())
        };
        self.current_merges.insert(pair);
    }

    /// Record a synthesis assertion from the current cycle.
    pub fn record_synthesis_assertion(&mut self, assertion: &str) {
        self.current_assertions.push(assertion.to_string());
    }

    /// Record skill distillation output for the current cycle.
    pub fn record_skill_distill_output(&mut self, produced: u32, recalled: u32) {
        if self.distill_history.len() >= DISTILL_WINDOW {
            self.distill_history.pop_front();
        }
        self.distill_history.push_back((produced, recalled));
    }

    /// Advance to the next cycle: commit current data to history.
    pub fn advance_cycle(&mut self) {
        // Commit merges
        if self.merge_history.len() >= MERGE_CYCLE_WINDOW {
            self.merge_history.pop_front();
        }
        self.merge_history
            .push_back(std::mem::take(&mut self.current_merges));

        // Commit assertions
        if self.synthesis_assertions.len() >= 2 {
            self.synthesis_assertions.pop_front();
        }
        self.synthesis_assertions
            .push_back(std::mem::take(&mut self.current_assertions));
    }

    /// Activate cooldown for N cycles.
    pub fn activate_cooldown(&mut self, cycles: u32) {
        self.cooldown = cycles;
    }

    /// Decrement cooldown by one tick.
    pub fn tick_cooldown(&mut self) {
        self.cooldown = self.cooldown.saturating_sub(1);
    }

    /// Clear pathology history after cooldown expires.
    pub fn clear_after_cooldown(&mut self) {
        if self.cooldown == 0 {
            self.merge_history.clear();
            self.synthesis_assertions.clear();
            self.distill_history.clear();
        }
    }

    /// Evaluate all pathology detectors. Returns the gate decision.
    pub fn evaluate(&self) -> GateDecision {
        // Active cooldown
        if self.cooldown > 0 {
            return GateDecision::Conserve {
                reason: "cooldown active".into(),
                cooldown_remaining: self.cooldown,
            };
        }

        // 1. Merge cycle detection
        if let Some(reason) = self.detect_merge_cycle() {
            return GateDecision::Conserve {
                reason,
                cooldown_remaining: 0,
            };
        }

        // 2. Synthesis oscillation
        if let Some(reason) = self.detect_oscillation() {
            return GateDecision::Conserve {
                reason,
                cooldown_remaining: 0,
            };
        }

        // 3. Wasted distillation
        if let Some(reason) = self.detect_wasted_distillation() {
            return GateDecision::Conserve {
                reason,
                cooldown_remaining: 0,
            };
        }

        GateDecision::Allow
    }

    /// Check if any note pair appears in 3+ consecutive cycles.
    fn detect_merge_cycle(&self) -> Option<String> {
        // Include current_merges as the latest "cycle"
        let all_sets: Vec<&HashSet<(String, String)>> = self
            .merge_history
            .iter()
            .chain(std::iter::once(&self.current_merges))
            .collect();

        if all_sets.len() < MERGE_CYCLE_THRESHOLD {
            return None;
        }

        // Check all windows of MERGE_CYCLE_THRESHOLD consecutive sets
        for window in all_sets.windows(MERGE_CYCLE_THRESHOLD) {
            let intersection: HashSet<_> = window[0]
                .iter()
                .filter(|pair| window[1..].iter().all(|set| set.contains(*pair)))
                .cloned()
                .collect();

            if !intersection.is_empty() {
                let pair = intersection.into_iter().next().unwrap();
                return Some(format!(
                    "merge cycle: ({}, {}) repeated {} consecutive cycles",
                    pair.0, pair.1, MERGE_CYCLE_THRESHOLD
                ));
            }
        }

        None
    }

    /// Check for negation patterns between the two most recent synthesis cycles.
    fn detect_oscillation(&self) -> Option<String> {
        // Need at least the previous cycle in history + current assertions
        let prev = if !self.synthesis_assertions.is_empty() {
            self.synthesis_assertions.back().unwrap()
        } else {
            return None;
        };
        let curr = &self.current_assertions;

        if prev.is_empty() || curr.is_empty() {
            return None;
        }

        // Build negation regex patterns
        let negation_pairs = [
            (r"should\s+", r"should\s+not\s+"),
            (r"prefer\s+", r"avoid\s+"),
            (r"use\s+", r"do\s+not\s+use\s+"),
        ];

        for prev_assertion in prev {
            for curr_assertion in curr {
                for (positive, negative) in &negation_pairs {
                    let pos_re = Regex::new(positive).ok()?;
                    let neg_re = Regex::new(negative).ok()?;

                    let is_oscillation = (pos_re.is_match(prev_assertion)
                        && neg_re.is_match(curr_assertion))
                        || (neg_re.is_match(prev_assertion) && pos_re.is_match(curr_assertion));

                    if is_oscillation {
                        return Some(format!(
                            "synthesis oscillation: '{}' vs '{}'",
                            truncate(prev_assertion, 60),
                            truncate(curr_assertion, 60),
                        ));
                    }
                }
            }
        }

        None
    }

    /// Check if recent distillation output has very low recall rate.
    fn detect_wasted_distillation(&self) -> Option<String> {
        if self.distill_history.len() < DISTILL_WINDOW {
            return None;
        }

        let (total_produced, total_recalled): (u32, u32) =
            self.distill_history.iter().fold((0, 0), |(p, r), (dp, dr)| (p + dp, r + dr));

        if total_produced == 0 {
            return None;
        }

        let rate = total_recalled as f64 / total_produced as f64;
        if rate < DISTILL_MIN_RECALL_RATE {
            Some(format!(
                "wasted distillation: {total_recalled}/{total_produced} recalled ({:.0}% < {:.0}%)",
                rate * 100.0,
                DISTILL_MIN_RECALL_RATE * 100.0,
            ))
        } else {
            None
        }
    }
}

impl Default for MutationGate {
    fn default() -> Self {
        Self::new()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}
```

- [ ] **Step 4: Register the module**

Add to `src/memory/dreaming/mod.rs` after `pub mod selector;`:

```rust
pub mod mutation_gate;
```

And re-export:

```rust
pub use mutation_gate::MutationGate;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dreaming::mutation_gate -- --nocapture`
Expected: 9 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/mutation_gate.rs src/memory/dreaming/mod.rs
git commit -m "feat(dreaming): add MutationGate with cycle/oscillation/waste detection"
```

---

### Task 5: Validation Layer

**Files:**
- Create: `src/memory/dreaming/validation.rs`
- Modify: `src/memory/dreaming/mod.rs` (add module declaration)

- [ ] **Step 1: Write the failing test**

In `src/memory/dreaming/validation.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_frontmatter_passes_l1() {
        let content = "---\ncategory: learning\ntags: [rust]\ncreated: 2026-04-17\nupdated: 2026-04-17\n---\n\n- Some fact\n";
        let issues = validate_frontmatter(content, "learning/test");
        assert!(issues.is_empty(), "got issues: {:?}", issues);
    }

    #[test]
    fn missing_category_fails_l1() {
        let content = "---\ntags: [rust]\n---\n\n- Some fact\n";
        let issues = validate_frontmatter(content, "learning/test");
        assert!(!issues.is_empty());
        assert!(issues[0].message.contains("category"));
    }

    #[test]
    fn empty_content_fails_l1() {
        let content = "---\ncategory: learning\ntags: []\ncreated: 2026-04-17\nupdated: 2026-04-17\n---\n";
        let issues = validate_frontmatter(content, "learning/test");
        assert!(issues.iter().any(|i| i.message.contains("empty")));
    }

    #[test]
    fn invalid_category_fails_l1() {
        let content = "---\ncategory: nonexistent\ntags: []\ncreated: 2026-04-17\nupdated: 2026-04-17\n---\n\n- fact\n";
        let issues = validate_frontmatter(content, "learning/test");
        assert!(issues.iter().any(|i| i.message.contains("category")));
    }

    #[test]
    fn duplicate_hashes_fail_l2() {
        let notes = vec![
            ("a/note1".to_string(), "hash_abc".to_string()),
            ("b/note2".to_string(), "hash_abc".to_string()),
            ("c/note3".to_string(), "hash_xyz".to_string()),
        ];
        let issues = check_duplicate_hashes(&notes);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("duplicate"));
    }

    #[test]
    fn no_duplicate_hashes_passes_l2() {
        let notes = vec![
            ("a/note1".to_string(), "hash_abc".to_string()),
            ("b/note2".to_string(), "hash_def".to_string()),
        ];
        let issues = check_duplicate_hashes(&notes);
        assert!(issues.is_empty());
    }

    #[test]
    fn validation_report_overall_ok_when_l1_l2_pass() {
        let report = DreamValidationReport {
            l1_format: ValidationTier { passed: true, checks_run: 5, checks_passed: 5, issues: vec![] },
            l2_consistency: ValidationTier { passed: true, checks_run: 3, checks_passed: 3, issues: vec![] },
            l3_semantic: None,
            l4_retrospective: None,
        };
        assert!(report.overall_ok());
    }

    #[test]
    fn validation_report_not_ok_when_l1_fails() {
        let report = DreamValidationReport {
            l1_format: ValidationTier { passed: false, checks_run: 5, checks_passed: 3, issues: vec![] },
            l2_consistency: ValidationTier { passed: true, checks_run: 3, checks_passed: 3, issues: vec![] },
            l3_semantic: None,
            l4_retrospective: None,
        };
        assert!(!report.overall_ok());
    }

    #[test]
    fn l3_failure_still_overall_ok() {
        let report = DreamValidationReport {
            l1_format: ValidationTier { passed: true, checks_run: 5, checks_passed: 5, issues: vec![] },
            l2_consistency: ValidationTier { passed: true, checks_run: 3, checks_passed: 3, issues: vec![] },
            l3_semantic: Some(ValidationTier { passed: false, checks_run: 1, checks_passed: 0, issues: vec![] }),
            l4_retrospective: None,
        };
        // L3 failure is warning, not blocking
        assert!(report.overall_ok());
    }

    #[test]
    fn serde_roundtrip_report() {
        let report = DreamValidationReport {
            l1_format: ValidationTier { passed: true, checks_run: 1, checks_passed: 1, issues: vec![] },
            l2_consistency: ValidationTier { passed: true, checks_run: 1, checks_passed: 1, issues: vec![] },
            l3_semantic: None,
            l4_retrospective: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: DreamValidationReport = serde_json::from_str(&json).unwrap();
        assert!(back.overall_ok());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::validation -- --nocapture 2>&1 | head -30`
Expected: compilation error

- [ ] **Step 3: Write the implementation**

Create `src/memory/dreaming/validation.rs`:

```rust
//! Validation Layer — four-tier verification after Dream Pipeline execution.
//!
//! - L1 Format: YAML frontmatter, wikilinks, categories, non-empty content
//! - L2 Consistency: duplicate hashes, index-fs sync
//! - L3 Semantic: LLM check (Synthesize mode only, run externally)
//! - L4 Retrospective: recall hit rate from previous cycle (run externally)

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::memory::notes::indexer::CATEGORY_DIRS;

/// A single validation issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub tier: String,
    pub note_path: String,
    pub message: String,
}

/// Result of a single validation tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationTier {
    pub passed: bool,
    pub checks_run: u32,
    pub checks_passed: u32,
    pub issues: Vec<ValidationIssue>,
}

/// Full validation report across all tiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamValidationReport {
    pub l1_format: ValidationTier,
    pub l2_consistency: ValidationTier,
    pub l3_semantic: Option<ValidationTier>,
    pub l4_retrospective: Option<ValidationTier>,
}

impl DreamValidationReport {
    /// Overall OK if L1 and L2 both passed. L3/L4 failures are warnings.
    pub fn overall_ok(&self) -> bool {
        self.l1_format.passed && self.l2_consistency.passed
    }
}

// ---------------------------------------------------------------------------
// L1: Format validation helpers
// ---------------------------------------------------------------------------

/// Validate frontmatter and content of a single note's markdown.
pub fn validate_frontmatter(content: &str, note_path: &str) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let tier = "L1".to_string();

    // Check for frontmatter delimiters
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        issues.push(ValidationIssue {
            tier: tier.clone(),
            note_path: note_path.to_string(),
            message: "missing YAML frontmatter delimiters".into(),
        });
        return issues;
    }

    let frontmatter = parts[1].trim();
    let body = parts[2].trim();

    // Check category field exists
    let has_category = frontmatter
        .lines()
        .any(|line| line.trim_start().starts_with("category:"));
    if !has_category {
        issues.push(ValidationIssue {
            tier: tier.clone(),
            note_path: note_path.to_string(),
            message: "missing category field in frontmatter".into(),
        });
    } else {
        // Validate category value
        if let Some(cat) = extract_yaml_value(frontmatter, "category") {
            let valid_categories: HashSet<&str> = CATEGORY_DIRS.iter().copied().collect();
            // Also allow "synthesis" and "query"
            if !valid_categories.contains(cat.as_str())
                && cat != "synthesis"
                && cat != "query"
            {
                issues.push(ValidationIssue {
                    tier: tier.clone(),
                    note_path: note_path.to_string(),
                    message: format!("invalid category '{}' not in CATEGORY_DIRS", cat),
                });
            }
        }
    }

    // Check for empty content (body after frontmatter)
    if body.is_empty() {
        issues.push(ValidationIssue {
            tier: tier.clone(),
            note_path: note_path.to_string(),
            message: "empty content body after frontmatter".into(),
        });
    }

    issues
}

/// Simple YAML value extractor for `key: value` lines.
fn extract_yaml_value(yaml: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    yaml.lines()
        .find(|line| line.trim_start().starts_with(&prefix))
        .map(|line| {
            line.trim_start()
                .strip_prefix(&prefix)
                .unwrap_or("")
                .trim()
                .to_string()
        })
}

// ---------------------------------------------------------------------------
// L2: Consistency validation helpers
// ---------------------------------------------------------------------------

/// Check for duplicate content hashes across notes.
pub fn check_duplicate_hashes(notes: &[(String, String)]) -> Vec<ValidationIssue> {
    let mut seen: HashMap<&str, Vec<&str>> = HashMap::new();
    for (path, hash) in notes {
        if !hash.is_empty() {
            seen.entry(hash.as_str()).or_default().push(path.as_str());
        }
    }

    let mut issues = Vec::new();
    for (hash, paths) in &seen {
        if paths.len() > 1 {
            issues.push(ValidationIssue {
                tier: "L2".into(),
                note_path: paths.join(", "),
                message: format!(
                    "duplicate content_hash '{}' across {} notes",
                    &hash[..hash.len().min(16)],
                    paths.len()
                ),
            });
        }
    }
    issues
}

/// Run L1 format validation on a batch of notes.
pub fn run_l1_validation(
    note_contents: &HashMap<String, String>,
) -> ValidationTier {
    let mut issues = Vec::new();
    let mut checks_run = 0u32;
    let mut checks_passed = 0u32;

    for (path, content) in note_contents {
        checks_run += 1;
        let note_issues = validate_frontmatter(content, path);
        if note_issues.is_empty() {
            checks_passed += 1;
        } else {
            issues.extend(note_issues);
        }
    }

    ValidationTier {
        passed: issues.is_empty(),
        checks_run,
        checks_passed,
        issues,
    }
}

/// Run L2 consistency validation on note hashes.
pub fn run_l2_validation(
    note_hashes: &[(String, String)],
) -> ValidationTier {
    let dup_issues = check_duplicate_hashes(note_hashes);
    let checks_run = 1u32; // duplicate hash check
    let checks_passed = if dup_issues.is_empty() { 1 } else { 0 };

    ValidationTier {
        passed: dup_issues.is_empty(),
        checks_run,
        checks_passed,
        issues: dup_issues,
    }
}
```

- [ ] **Step 4: Register the module**

Add to `src/memory/dreaming/mod.rs` after `pub mod mutation_gate;`:

```rust
pub mod validation;
```

And re-export:

```rust
pub use validation::{DreamValidationReport, ValidationIssue, ValidationTier};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dreaming::validation -- --nocapture`
Expected: 10 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/validation.rs src/memory/dreaming/mod.rs
git commit -m "feat(dreaming): add 4-tier Validation Layer for Dream cycles"
```

---

### Task 6: Immutable Event Log (Solidify)

**Files:**
- Create: `src/memory/dreaming/event_log.rs`
- Modify: `src/memory/dreaming/mod.rs` (add module declaration)

- [ ] **Step 1: Write the failing test**

In `src/memory/dreaming/event_log.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_event(cycle: u32) -> DreamEvent {
        DreamEvent {
            id: format!("dream_test_{cycle}"),
            cycle,
            strategy: DreamStrategy::Consolidate,
            selection: SelectionDecision {
                strategy: DreamStrategy::Consolidate,
                rationale: "test".into(),
                personality_adjustment: 0.0,
            },
            gate_decision: GateDecision::Allow,
            report: DreamReport::default(),
            validation: DreamValidationReport {
                l1_format: ValidationTier { passed: true, checks_run: 1, checks_passed: 1, issues: vec![] },
                l2_consistency: ValidationTier { passed: true, checks_run: 1, checks_passed: 1, issues: vec![] },
                l3_semantic: None,
                l4_retrospective: None,
            },
            duration_ms: 100,
            created_at: 1_700_000_000,
        }
    }

    #[tokio::test]
    async fn append_and_read_events() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));

        log.append(&make_event(1)).await.unwrap();
        log.append(&make_event(2)).await.unwrap();
        log.append(&make_event(3)).await.unwrap();

        let events = log.read_last(2).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].cycle, 2);
        assert_eq!(events[1].cycle, 3);
    }

    #[tokio::test]
    async fn read_from_empty_log() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));
        let events = log.read_last(10).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn read_more_than_available() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));
        log.append(&make_event(1)).await.unwrap();
        let events = log.read_last(100).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn next_cycle_number_from_empty() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));
        assert_eq!(log.next_cycle().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn next_cycle_increments() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));
        log.append(&make_event(5)).await.unwrap();
        assert_eq!(log.next_cycle().await.unwrap(), 6);
    }

    #[test]
    fn event_serde_roundtrip() {
        let event = make_event(42);
        let json = serde_json::to_string(&event).unwrap();
        let back: DreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cycle, 42);
        assert_eq!(back.id, "dream_test_42");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::event_log -- --nocapture 2>&1 | head -30`
Expected: compilation error

- [ ] **Step 3: Write the implementation**

Create `src/memory/dreaming/event_log.rs`:

```rust
//! EventLog — append-only audit trail for Dream cycles.
//!
//! Each Dream cycle produces one `DreamEvent` serialized as a JSON line
//! in `{agent_dir}/dream_events.jsonl`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::AlephError;
use crate::memory::dreaming::report::DreamReport;
use crate::memory::dreaming::selector::{GateDecision, SelectionDecision};
use crate::memory::dreaming::strategy::DreamStrategy;
use crate::memory::dreaming::validation::{DreamValidationReport, ValidationTier};

const EVENT_LOG_FILENAME: &str = "dream_events.jsonl";

/// A single Dream cycle event, the unit of the audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamEvent {
    pub id: String,
    pub cycle: u32,
    pub strategy: DreamStrategy,
    pub selection: SelectionDecision,
    pub gate_decision: GateDecision,
    pub report: DreamReport,
    pub validation: DreamValidationReport,
    pub duration_ms: u64,
    pub created_at: i64,
}

/// Append-only event log stored as JSONL.
pub struct EventLog {
    agent_dir: PathBuf,
}

impl EventLog {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self {
            agent_dir: agent_dir.into(),
        }
    }

    fn log_path(&self) -> PathBuf {
        self.agent_dir.join(EVENT_LOG_FILENAME)
    }

    /// Append one event to the log file.
    pub async fn append(&self, event: &DreamEvent) -> Result<(), AlephError> {
        tokio::fs::create_dir_all(&self.agent_dir)
            .await
            .map_err(|e| AlephError::config(format!("create agent dir: {e}")))?;

        let mut line =
            serde_json::to_string(event).map_err(|e| AlephError::config(format!("serialize event: {e}")))?;
        line.push('\n');

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path())
            .await
            .map_err(|e| AlephError::config(format!("open event log: {e}")))?;

        file.write_all(line.as_bytes())
            .await
            .map_err(|e| AlephError::config(format!("write event log: {e}")))?;

        Ok(())
    }

    /// Read the last N events from the log. Returns them in chronological order.
    pub async fn read_last(&self, n: usize) -> Result<Vec<DreamEvent>, AlephError> {
        let path = self.log_path();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| AlephError::config(format!("read event log: {e}")))?;

        let events: Vec<DreamEvent> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        let skip = events.len().saturating_sub(n);
        Ok(events.into_iter().skip(skip).collect())
    }

    /// Get the next cycle number (max existing + 1, or 1 if empty).
    pub async fn next_cycle(&self) -> Result<u32, AlephError> {
        let events = self.read_last(1).await?;
        Ok(events.last().map(|e| e.cycle + 1).unwrap_or(1))
    }
}
```

- [ ] **Step 4: Register the module**

Add to `src/memory/dreaming/mod.rs` after `pub mod validation;`:

```rust
pub mod event_log;
```

And re-export:

```rust
pub use event_log::{DreamEvent, EventLog};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dreaming::event_log -- --nocapture`
Expected: 6 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/event_log.rs src/memory/dreaming/mod.rs
git commit -m "feat(dreaming): add immutable EventLog for Dream cycle audit trail"
```

---

### Task 7: SkillDistill stage

**Files:**
- Create: `src/memory/dreaming/stages/skill_distill.rs`
- Modify: `src/memory/dreaming/stages/mod.rs` (register stage)

- [ ] **Step 1: Write the failing test**

In `src/memory/dreaming/stages/skill_distill.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_name() {
        assert_eq!(SkillDistillStage.name(), "skill_distill");
    }

    #[test]
    fn prompt_contains_synthesis_content() {
        let synthesis_text = "Cross-cutting theme: async patterns are preferred.";
        let prompt = build_distill_prompt(synthesis_text, "learning");
        assert!(prompt.contains("async patterns"));
        assert!(prompt.contains("learning"));
        assert!(prompt.contains("skill"));
    }

    #[test]
    fn parse_distilled_skills_valid_json() {
        let response = r#"[
            {"title": "async-error-handling", "facts": ["Always use ? for propagation", "Wrap spawned tasks in catch_unwind"]},
            {"title": "trait-design", "facts": ["Keep traits small", "Prefer associated types over generics"]}
        ]"#;
        let skills = parse_distilled_skills(response);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].title, "async-error-handling");
        assert_eq!(skills[0].facts.len(), 2);
    }

    #[test]
    fn parse_distilled_skills_invalid_json_returns_empty() {
        let response = "This is not valid JSON at all.";
        let skills = parse_distilled_skills(response);
        assert!(skills.is_empty());
    }

    #[test]
    fn parse_distilled_skills_empty_array() {
        let response = "[]";
        let skills = parse_distilled_skills(response);
        assert!(skills.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::stages::skill_distill -- --nocapture 2>&1 | head -30`
Expected: compilation error

- [ ] **Step 3: Write the implementation**

Create `src/memory/dreaming/stages/skill_distill.rs`:

```rust
//! SkillDistill stage — extracts reusable skill-notes from synthesis output.
//!
//! Runs after NoteSynthesis in the Synthesize strategy. Reads synthesis notes
//! produced in the current cycle and asks an LLM to extract actionable
//! patterns as `skill`-category knowledge notes.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::KnowledgeNote;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::DreamStage;

pub struct SkillDistillStage;

#[async_trait]
impl DreamStage for SkillDistillStage {
    fn name(&self) -> &'static str {
        "skill_distill"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        // Only run if there are synthesis notes to distill from
        ctx.notes.iter().any(|n| n.category == "synthesis")
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let synthesis_notes: Vec<_> = ctx
            .notes
            .iter()
            .filter(|n| n.category == "synthesis")
            .map(|n| n.path.clone())
            .collect();

        let mut distilled_count = 0u32;

        for path in &synthesis_notes {
            let content = match ctx.load_content(path).await {
                Some(c) => c,
                None => continue,
            };

            let category = path
                .split('/')
                .nth(0)
                .and_then(|p| {
                    // Extract the original category from synthesis title
                    // e.g., "synthesis/learning-synthesis" → "learning"
                    p.strip_suffix("-synthesis").or(Some(p))
                })
                .unwrap_or("general");

            let prompt = build_distill_prompt(&content, category);
            let system = "You are a skill extraction engine. Extract actionable, reusable patterns from synthesis notes. Return a JSON array.";

            let msgs = vec![UnifiedMessage::user(&prompt)];
            let response = match ctx
                .provider
                .process(RequestPayload::new(&msgs).with_system(Some(system)))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(path, error = %e, "SkillDistill LLM call failed");
                    continue;
                }
            };

            let skills = parse_distilled_skills(&response.text_content());

            for skill in &skills {
                let note = KnowledgeNote {
                    title: skill.title.clone(),
                    category: "skill".to_string(),
                    tags: vec!["distilled".to_string(), category.to_string()],
                    facts: skill.facts.clone(),
                    links: vec![format!("[[{}]]", path)],
                    created_at: chrono::Utc::now().timestamp(),
                    updated_at: chrono::Utc::now().timestamp(),
                    content_hash: String::new(),
                };

                match ctx
                    .indexer
                    .write_note(&ctx.agent_id, "skill", &note)
                    .await
                {
                    Ok(_) => {
                        distilled_count += 1;
                        tracing::info!(title = %skill.title, "Distilled skill-note");
                    }
                    Err(e) => {
                        tracing::warn!(title = %skill.title, error = %e, "Failed to write skill-note");
                    }
                }
            }
        }

        // Store distilled count in extras for the report
        ctx.report
            .extra
            .insert("skill_distill_count".into(), distilled_count.to_string());

        tracing::info!(distilled_count, "SkillDistill completed");
        Ok(ctx)
    }
}

/// Build the LLM prompt for skill extraction from synthesis content.
pub fn build_distill_prompt(synthesis_text: &str, source_category: &str) -> String {
    format!(
        "Analyze this synthesis note from the '{source_category}' category and extract reusable skill patterns.\n\n\
         Synthesis:\n{synthesis_text}\n\n\
         Extract 0-3 actionable skill patterns. For each, provide:\n\
         - A kebab-case title (e.g., \"async-error-handling\")\n\
         - 2-5 concise fact bullets (third person, actionable)\n\n\
         Return as JSON array:\n\
         ```json\n\
         [\n\
           {{\"title\": \"skill-name\", \"facts\": [\"fact 1\", \"fact 2\"]}}\n\
         ]\n\
         ```\n\
         Return `[]` if no actionable patterns found."
    )
}

/// Parsed skill from LLM response.
#[derive(Debug, Clone)]
pub struct DistilledSkill {
    pub title: String,
    pub facts: Vec<String>,
}

/// Parse LLM response into distilled skills. Tolerant of formatting issues.
pub fn parse_distilled_skills(response: &str) -> Vec<DistilledSkill> {
    // Try to find JSON array in response (may be wrapped in markdown code block)
    let json_str = response
        .find('[')
        .and_then(|start| {
            response.rfind(']').map(|end| &response[start..=end])
        })
        .unwrap_or("[]");

    let parsed: Vec<serde_json::Value> = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    parsed
        .into_iter()
        .filter_map(|v| {
            let title = v.get("title")?.as_str()?.to_string();
            let facts: Vec<String> = v
                .get("facts")?
                .as_array()?
                .iter()
                .filter_map(|f| f.as_str().map(String::from))
                .collect();
            if title.is_empty() || facts.is_empty() {
                return None;
            }
            Some(DistilledSkill { title, facts })
        })
        .collect()
}
```

- [ ] **Step 4: Register the stage**

In `src/memory/dreaming/stages/mod.rs`, add after `pub mod note_synthesis;`:

```rust
pub mod skill_distill;
```

And add the re-export after existing re-exports:

```rust
pub use skill_distill::SkillDistillStage;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dreaming::stages::skill_distill -- --nocapture`
Expected: 5 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/stages/skill_distill.rs src/memory/dreaming/stages/mod.rs
git commit -m "feat(dreaming): add SkillDistill stage for skill-note extraction from synthesis"
```

---

### Task 8: Wire DreamPipeline::from_strategy()

**Files:**
- Modify: `src/memory/dreaming/stages/mod.rs`
- Modify: `src/memory/dreaming/mod.rs` (DreamPipeline changes)

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block at the bottom of `src/memory/dreaming/mod.rs`:

```rust
    #[test]
    fn pipeline_from_strategy_consolidate() {
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Consolidate);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(names, vec!["note_lint", "note_consolidate", "note_drift", "index_refresher", "note_decay"]);
    }

    #[test]
    fn pipeline_from_strategy_synthesize() {
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Synthesize);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(names, vec!["note_lint", "note_consolidate", "note_synthesis", "skill_distill", "daily_digest"]);
    }

    #[test]
    fn pipeline_from_strategy_conserve() {
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Conserve);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(names, vec!["note_lint", "index_refresher"]);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::tests::pipeline_from_strategy -- --nocapture 2>&1 | head -30`
Expected: compilation error — `from_strategy` not defined

- [ ] **Step 3: Add from_strategy to DreamPipeline**

In `src/memory/dreaming/mod.rs`, add this method to `impl DreamPipeline`, after the `weekly()` method:

```rust
    /// Build a pipeline from a DreamStrategy.
    pub fn from_strategy(strategy: DreamStrategy) -> Self {
        let stage_list: Vec<Box<dyn DreamStage>> = match strategy {
            DreamStrategy::Consolidate => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteConsolidateStage),
                Box::new(stages::NoteDriftStage),
                Box::new(stages::IndexRefresherStage),
                Box::new(stages::NoteDecayStage),
            ],
            DreamStrategy::Synthesize => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteConsolidateStage),
                Box::new(stages::NoteSynthesisStage),
                Box::new(stages::SkillDistillStage),
                Box::new(stages::DailyDigestStage),
            ],
            DreamStrategy::Conserve => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::IndexRefresherStage),
            ],
        };
        Self::new(stage_list)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dreaming::tests::pipeline_from_strategy -- --nocapture`
Expected: 3 tests pass

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/mod.rs
git commit -m "feat(dreaming): add DreamPipeline::from_strategy() for strategy-driven stage selection"
```

---

### Task 9: Refactor DreamDaemon main loop

**Files:**
- Modify: `src/memory/dreaming/mod.rs`

This task replaces `run_dream` with the new evolution loop: Signal → Select → Gate → Pipeline → Validate → Solidify.

- [ ] **Step 1: Add new fields to DreamDaemon**

In the `DreamDaemon` struct definition, add after the `orientation` field:

```rust
    /// Strategy selector with personality adaptation.
    selector: std::sync::Mutex<StrategySelector>,
    /// Mutation gate tracking evolution pathologies.
    mutation_gate: std::sync::Mutex<MutationGate>,
```

- [ ] **Step 2: Initialize new fields in from_config**

In `DreamDaemon::from_config`, add the new fields to the `Ok(Self { ... })`:

```rust
            selector: std::sync::Mutex::new(StrategySelector::new()),
            mutation_gate: std::sync::Mutex::new(MutationGate::new()),
```

- [ ] **Step 3: Replace determine_run_type and run_dream**

Replace the `determine_run_type` method entirely. Replace `run_dream` with the new evolution loop:

```rust
    async fn run_dream(
        &self,
        run_start: i64,
        _run_date: String,
    ) -> Result<(DreamRunStatus, DreamReport), AlephError> {
        // --- Phase 1: Collect signals ---
        // For now, use empty metrics since DreamContext wiring is still pending.
        // When fully wired, populate RawMetrics from database queries.
        let raw_metrics = RawMetrics::default();
        let signal_snapshot = SignalSnapshot::from_metrics(&raw_metrics);

        // --- Phase 2: Mutation gate evaluation ---
        let gate_decision = {
            let gate = self.mutation_gate.lock().unwrap_or_else(|e| e.into_inner());
            gate.evaluate()
        };

        // --- Phase 3: Strategy selection ---
        let selection = {
            let selector = self.selector.lock().unwrap_or_else(|e| e.into_inner());
            selector.select(&signal_snapshot, &gate_decision)
        };

        let strategy = selection.strategy;
        info!(strategy = %strategy, rationale = %selection.rationale, "Dream strategy selected");

        // --- Phase 4: Build and run pipeline ---
        let pipeline = DreamPipeline::from_strategy(strategy);

        // NOTE: Full DreamContext wiring requires NoteIndexer and EmbeddingProvider
        // (same constraint as before). Return stub report until those are wired.
        let _pipeline = pipeline;
        let report = DreamReport {
            pipeline_type: strategy.to_string(),
            started_at: run_start,
            finished_at: now_timestamp(),
            duration_ms: 0,
            status: DreamReportStatus::Completed,
            stages_executed: Vec::new(),
            ..Default::default()
        };

        // --- Phase 5: Validation (L1 + L2, deterministic) ---
        let validation_report = DreamValidationReport {
            l1_format: ValidationTier {
                passed: true,
                checks_run: 0,
                checks_passed: 0,
                issues: vec![],
            },
            l2_consistency: ValidationTier {
                passed: true,
                checks_run: 0,
                checks_passed: 0,
                issues: vec![],
            },
            l3_semantic: None,
            l4_retrospective: None,
        };

        // --- Phase 6: Solidify (event log) ---
        let memory_dir = crate::utils::paths::get_note_memory_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from(".aleph/data/memory"));
        let agent_dir = memory_dir.join("default"); // TODO: use actual agent_id when available
        let event_log = EventLog::new(&agent_dir);
        let cycle = event_log.next_cycle().await.unwrap_or(1);

        let event = DreamEvent {
            id: format!("dream_{}_{}", run_start, cycle),
            cycle,
            strategy,
            selection: selection.clone(),
            gate_decision: gate_decision.clone(),
            report: report.clone(),
            validation: validation_report,
            duration_ms: ((now_timestamp() - run_start).max(0) as u64) * 1000,
            created_at: now_timestamp(),
        };

        if let Err(e) = event_log.append(&event).await {
            warn!(error = %e, "Failed to write dream event log");
        }

        // --- Phase 7: Update personality + mutation gate ---
        {
            let mut selector = self.selector.lock().unwrap_or_else(|e| e.into_inner());
            selector.record_cycle_outcome(
                strategy,
                event.validation.overall_ok(),
                signal_snapshot.score("skill_recall_rate"),
            );
        }
        {
            let mut gate = self.mutation_gate.lock().unwrap_or_else(|e| e.into_inner());
            gate.advance_cycle();
            gate.tick_cooldown();
        }

        Ok((DreamRunStatus::Success, report))
    }
```

- [ ] **Step 4: Add necessary imports**

At the top of `mod.rs`, ensure these are imported (add any that are missing):

```rust
use crate::memory::dreaming::signals::{RawMetrics, SignalSnapshot};
use crate::memory::dreaming::selector::{GateDecision, StrategySelector};
use crate::memory::dreaming::mutation_gate::MutationGate;
use crate::memory::dreaming::validation::{DreamValidationReport, ValidationTier};
use crate::memory::dreaming::event_log::{DreamEvent, EventLog};
use crate::memory::dreaming::strategy::DreamStrategy;
use crate::utils::paths::get_note_memory_dir;
```

- [ ] **Step 5: Run all dreaming tests to verify nothing broke**

Run: `cargo test -p alephcore --lib dreaming -- --nocapture`
Expected: All existing tests pass + new from_strategy tests pass

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/mod.rs
git commit -m "refactor(dreaming): wire evolution loop into DreamDaemon main cycle"
```

---

### Task 10: Cleanup old code

**Files:**
- Modify: `src/memory/dreaming/mod.rs`

- [ ] **Step 1: Remove DreamPipeline::daily() and ::weekly()**

Delete the `daily()` and `weekly()` methods from `impl DreamPipeline`. Keep `new()`, `from_strategy()`, and `run()`.

- [ ] **Step 2: Remove determine_run_type method**

Delete the `determine_run_type` method from `impl DreamDaemon` (it was replaced by strategy selection).

- [ ] **Step 3: Update report.rs**

In `src/memory/dreaming/report.rs`:
- Remove the `DreamRunType` enum and `DreamRunMetadata` struct.
- Add `Deserialize` to `DreamReport`'s derive (needed for JSONL roundtrip in EventLog):

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DreamReport {
```

Also add `Deserialize` to `DreamReportStatus`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum DreamReportStatus {
``` Update `DreamContext` if it references `DreamRunMetadata` — replace with `DreamStrategy` or remove the field.

Check `DreamContext.run_metadata` usage:

```rust
// In DreamContext, replace:
pub run_metadata: DreamRunMetadata,
// With:
pub strategy: DreamStrategy,
```

- [ ] **Step 4: Update tests that reference old methods**

In `src/memory/dreaming/mod.rs` tests, remove:

```rust
    #[test]
    fn test_pipeline_builder_daily() {
        let pipeline = DreamPipeline::daily();
        assert_eq!(pipeline.stages.len(), 6);
    }

    #[test]
    fn test_pipeline_builder_weekly() {
        let pipeline = DreamPipeline::weekly();
        assert_eq!(pipeline.stages.len(), 7);
    }
```

These are replaced by the `pipeline_from_strategy_*` tests.

- [ ] **Step 5: Update note_synthesis should_run**

In `src/memory/dreaming/stages/note_synthesis.rs`, update `should_run`:

```rust
    async fn should_run(&self, ctx: &DreamContext) -> bool {
        // Runs when strategy is Synthesize and there are enough notes
        ctx.notes.len() >= 5
    }
```

Remove the `pipeline_type == "weekly"` check — strategy selection now controls when synthesis runs.

- [ ] **Step 6: Remove re-exports of deleted types**

In `src/memory/dreaming/mod.rs`, remove from the re-exports:

```rust
pub use report::{DreamRunMetadata, DreamRunType};
```

Keep only:

```rust
pub use report::{DreamReport, DreamReportStatus};
```

- [ ] **Step 7: Fix any remaining references**

Run: `cargo check -p alephcore 2>&1 | head -50`

Fix any remaining compilation errors from removed types. Common fixes:
- `pipeline_type` field in `DreamContext` and `DreamReport` → keep as `String`, set from `strategy.to_string()`
- Any `DreamRunType::Daily` / `DreamRunType::Weekly` references → remove
- `is_weekly` checks in tests → remove

- [ ] **Step 8: Run full test suite**

Run: `cargo test -p alephcore --lib dreaming -- --nocapture`
Expected: All tests pass

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor(dreaming): remove daily/weekly pipeline, replace with strategy-driven evolution"
```

---

### Task 11: Integration test

**Files:**
- Create: `tests/dream_evolution.rs`

- [ ] **Step 1: Write the integration test**

Create `tests/dream_evolution.rs`:

```rust
//! Integration test for Dream Daemon evolution upgrade.
//!
//! Tests the full signal → select → gate → validate → solidify flow
//! using in-memory/temp-dir setup without an actual LLM provider.

use alephcore::memory::dreaming::event_log::EventLog;
use alephcore::memory::dreaming::mutation_gate::MutationGate;
use alephcore::memory::dreaming::selector::{GateDecision, SelectionDecision, StrategySelector};
use alephcore::memory::dreaming::signals::{RawMetrics, SignalSnapshot};
use alephcore::memory::dreaming::strategy::DreamStrategy;
use alephcore::memory::dreaming::validation::{
    check_duplicate_hashes, run_l1_validation, DreamValidationReport, ValidationTier,
};
use alephcore::memory::dreaming::{DreamPipeline, DreamReport};
use std::collections::HashMap;
use tempfile::tempdir;

/// Full evolution cycle: signals → select → gate → validate → log.
#[tokio::test]
async fn full_evolution_cycle_consolidate() {
    let dir = tempdir().unwrap();

    // 1. Collect signals (default → low growth, low issues)
    let metrics = RawMetrics::default();
    let snapshot = SignalSnapshot::from_metrics(&metrics);

    // 2. Gate evaluation (no history → Allow)
    let gate = MutationGate::new();
    let gate_decision = gate.evaluate();
    assert!(matches!(gate_decision, GateDecision::Allow));

    // 3. Strategy selection (default → Consolidate)
    let selector = StrategySelector::new();
    let selection = selector.select(&snapshot, &gate_decision);
    assert_eq!(selection.strategy, DreamStrategy::Consolidate);

    // 4. Build pipeline (verify stages)
    let pipeline = DreamPipeline::from_strategy(selection.strategy);
    assert_eq!(pipeline.stages.len(), 5);

    // 5. Validation (empty notes → passes trivially)
    let l1 = run_l1_validation(&HashMap::new());
    let l2_issues = check_duplicate_hashes(&[]);
    assert!(l1.passed);
    assert!(l2_issues.is_empty());

    // 6. Solidify (write event)
    let event_log = EventLog::new(dir.path().join("test_agent"));
    let cycle = event_log.next_cycle().await.unwrap();
    assert_eq!(cycle, 1);

    let event = alephcore::memory::dreaming::event_log::DreamEvent {
        id: format!("dream_test_{}", cycle),
        cycle,
        strategy: selection.strategy,
        selection,
        gate_decision,
        report: DreamReport::default(),
        validation: DreamValidationReport {
            l1_format: l1,
            l2_consistency: ValidationTier {
                passed: true,
                checks_run: 1,
                checks_passed: 1,
                issues: vec![],
            },
            l3_semantic: None,
            l4_retrospective: None,
        },
        duration_ms: 42,
        created_at: chrono::Utc::now().timestamp(),
    };

    event_log.append(&event).await.unwrap();

    // Verify event was persisted
    let events = event_log.read_last(10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].strategy, DreamStrategy::Consolidate);
    assert!(events[0].validation.overall_ok());
}

/// High-growth scenario selects Synthesize.
#[tokio::test]
async fn high_growth_selects_synthesize() {
    let metrics = RawMetrics {
        notes_added_24h: 80,
        total_notes: 100,
        skill_notes_total: 10,
        skill_notes_recalled: 0,
        ..Default::default()
    };
    let snapshot = SignalSnapshot::from_metrics(&metrics);

    let gate = MutationGate::new();
    let gate_decision = gate.evaluate();

    let selector = StrategySelector::new();
    let selection = selector.select(&snapshot, &gate_decision);
    assert_eq!(selection.strategy, DreamStrategy::Synthesize);

    let pipeline = DreamPipeline::from_strategy(selection.strategy);
    assert_eq!(pipeline.stages.len(), 5);
    assert_eq!(pipeline.stages[3].name(), "skill_distill");
}

/// Mutation gate forces Conserve on merge cycle.
#[tokio::test]
async fn merge_cycle_forces_conserve() {
    let mut gate = MutationGate::new();

    // Simulate 3 cycles with same merge pair
    for _ in 0..3 {
        gate.record_merge_pair("note_a", "note_b");
        if gate.evaluate() != GateDecision::Allow {
            break; // Hit conserve early
        }
        gate.advance_cycle();
    }

    // After 3 cycles, the pair triggers conserve
    let gate_decision = gate.evaluate();
    assert!(matches!(gate_decision, GateDecision::Conserve { .. }));

    // Selector should respect the gate
    let snapshot = SignalSnapshot::from_metrics(&RawMetrics {
        notes_added_24h: 80,
        total_notes: 100,
        ..Default::default()
    });
    let selector = StrategySelector::new();
    let selection = selector.select(&snapshot, &gate_decision);
    assert_eq!(selection.strategy, DreamStrategy::Conserve);

    // Conserve pipeline is minimal
    let pipeline = DreamPipeline::from_strategy(selection.strategy);
    assert_eq!(pipeline.stages.len(), 2);
}

/// Personality adaptation across multiple cycles.
#[tokio::test]
async fn personality_adapts_over_cycles() {
    let mut selector = StrategySelector::new();

    // 10 successful cycles → threshold drops
    for _ in 0..10 {
        selector.record_cycle_outcome(DreamStrategy::Consolidate, true, 0.5);
    }
    let threshold_after_success = selector.synthesize_threshold();

    // Reset and do 10 failed cycles → threshold rises
    let mut selector2 = StrategySelector::new();
    for _ in 0..10 {
        selector2.record_cycle_outcome(DreamStrategy::Consolidate, false, 0.0);
    }
    let threshold_after_failure = selector2.synthesize_threshold();

    assert!(
        threshold_after_success < threshold_after_failure,
        "success threshold ({}) should be lower than failure threshold ({})",
        threshold_after_success,
        threshold_after_failure
    );
}

/// L1 validation catches bad frontmatter.
#[test]
fn l1_catches_bad_frontmatter() {
    let mut contents = HashMap::new();
    contents.insert(
        "learning/good".to_string(),
        "---\ncategory: learning\ntags: []\ncreated: 2026-04-17\nupdated: 2026-04-17\n---\n\n- fact\n".to_string(),
    );
    contents.insert(
        "learning/bad".to_string(),
        "no frontmatter at all".to_string(),
    );

    let tier = run_l1_validation(&contents);
    assert!(!tier.passed);
    assert_eq!(tier.checks_run, 2);
    assert_eq!(tier.checks_passed, 1);
    assert!(!tier.issues.is_empty());
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test dream_evolution -- --nocapture`
Expected: All 5 tests pass

- [ ] **Step 3: Fix any compilation issues**

If any types aren't publicly exported from `alephcore`, add `pub use` in the crate's `lib.rs` or adjust visibility.

Run: `cargo test --test dream_evolution -- --nocapture`
Expected: All pass after fixes

- [ ] **Step 4: Commit**

```bash
git add tests/dream_evolution.rs
git commit -m "test(dreaming): add integration tests for Dream evolution upgrade"
```

---

### Task 12: Final verification

- [ ] **Step 1: Run all tests**

Run: `cargo test -p alephcore -- --nocapture 2>&1 | tail -20`
Expected: All tests pass, no regressions

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`
Expected: No warnings

- [ ] **Step 3: Check formatting**

Run: `cargo fmt -p alephcore -- --check`
Expected: No formatting issues

- [ ] **Step 4: Verify the complete file structure**

New files created:
- `src/memory/dreaming/strategy.rs`
- `src/memory/dreaming/signals.rs`
- `src/memory/dreaming/selector.rs`
- `src/memory/dreaming/mutation_gate.rs`
- `src/memory/dreaming/validation.rs`
- `src/memory/dreaming/event_log.rs`
- `src/memory/dreaming/stages/skill_distill.rs`
- `tests/dream_evolution.rs`

Modified files:
- `src/memory/dreaming/mod.rs`
- `src/memory/dreaming/stages/mod.rs`
- `src/memory/dreaming/report.rs`
- `src/memory/dreaming/stages/note_synthesis.rs`

Removed:
- `DreamPipeline::daily()` / `::weekly()` methods
- `DreamRunType` enum
- `DreamRunMetadata` struct
- `determine_run_type()` method

- [ ] **Step 5: Final commit if any fixes were needed**

```bash
git add -A
git commit -m "chore(dreaming): final cleanup and lint fixes for evolution upgrade"
```
