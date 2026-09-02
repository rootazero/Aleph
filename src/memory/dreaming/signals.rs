//! Signal Collector — aggregates learning signals from four data sources.
//!
//! Produces a `SignalSnapshot` at the start of each Dream cycle, feeding
//! the Strategy Selector with normalized scores.

use serde::{Deserialize, Serialize};

/// Signal type classification.
///
/// A `Quality` variant used to sit here, carrying a single `correction_rate`
/// signal derived from `RawMetrics::correction_count / session_count`. Both
/// counters had exactly one producer — `..Default::default()` — so the rate was
/// structurally 0.0 on every cycle, and no consumer ever read the signal by name
/// (`StrategySelector` and the evolution gate both address signals through
/// `SignalSnapshot::score(name)` and neither key existed). Filling the counters
/// with real numbers would have changed no output byte, which is the definition
/// of a dead island: deleted rather than reconnected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    Recall,
    Health,
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
#[derive(Debug, Clone, Default)]
pub struct RawMetrics {
    pub duplication_rate: f64,
    pub contradiction_rate: f64,
    pub staleness_rate: f64,
    pub notes_added_24h: u32,
    pub total_notes: u32,
    pub note_hit_rate: f64,
    pub skill_notes_total: u32,
    pub skill_notes_recalled: u32,
    /// Mature skill-note cohort (created more than `MATURE_SKILL_DAYS` ago) —
    /// the population MutationGate's wasted-distillation detector judges. Notes
    /// too new to have had a recall opportunity are excluded so a fresh cycle's
    /// produce cannot make the detector misfire on cold start.
    pub mature_skill_total: u32,
    /// How many of `mature_skill_total` have at least one recall hit — the
    /// numerator of the wasted-distillation ratio.
    pub mature_skill_recalled: u32,
}

impl SignalSnapshot {
    /// Build a snapshot from raw metrics, normalizing each to [0.0, 1.0].
    #[must_use]
    pub fn from_metrics(m: &RawMetrics) -> Self {
        let now = chrono::Utc::now().timestamp();
        let mut signals = Vec::new();

        // Health signals
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
            (f64::from(m.notes_added_24h) / f64::from(m.total_notes)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        signals.push(DreamSignal {
            signal_type: SignalType::Health,
            name: "note_growth_rate".into(),
            score: growth_rate,
            source: "note_indexer".into(),
        });

        // Recall signals
        // A `never_recalled_ratio` signal used to be pushed alongside this
        // one. No consumer ever addressed it by name (`memory_health_score`
        // and `StrategySelector::select` both read through
        // `SignalSnapshot::score`, and neither key existed), so it met the
        // same dead-island bar as the `Quality` variant above and was cut.
        signals.push(DreamSignal {
            signal_type: SignalType::Recall,
            name: "note_hit_rate".into(),
            score: m.note_hit_rate.clamp(0.0, 1.0),
            source: "recall_signals".into(),
        });

        // Skill usage signals
        let skill_recall_rate = if m.skill_notes_total > 0 {
            (f64::from(m.skill_notes_recalled) / f64::from(m.skill_notes_total)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        signals.push(DreamSignal {
            signal_type: SignalType::SkillUsage,
            name: "skill_recall_rate".into(),
            score: skill_recall_rate,
            source: "recall_signals".into(),
        });

        Self {
            signals,
            collected_at: now,
        }
    }

    /// Get score by signal name, defaulting to 0.0 if not found.
    #[must_use]
    pub fn score(&self, name: &str) -> f64 {
        self.signals
            .iter()
            .find(|s| s.name == name)
            .map_or(0.0, |s| s.score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_snapshot_from_empty_metrics() {
        let metrics = RawMetrics::default();
        let snapshot = SignalSnapshot::from_metrics(&metrics);
        assert!(!snapshot.signals.is_empty());
        assert!(snapshot
            .signals
            .iter()
            .all(|s| s.score >= 0.0 && s.score <= 1.0));
    }

    #[test]
    fn high_contradiction_rate_produces_high_health_signal() {
        let metrics = RawMetrics {
            contradiction_rate: 0.8,
            ..Default::default()
        };
        let snapshot = SignalSnapshot::from_metrics(&metrics);
        let sig = snapshot
            .signals
            .iter()
            .find(|s| s.name == "high_contradiction_rate");
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
        let sig = snapshot
            .signals
            .iter()
            .find(|s| s.name == "note_growth_rate")
            .unwrap();
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
        let sig = snapshot
            .signals
            .iter()
            .find(|s| s.name == "skill_recall_rate")
            .unwrap();
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
