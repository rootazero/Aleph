//! Signal Collector — aggregates learning signals from four data sources.
//!
//! Produces a `SignalSnapshot` at the start of each Dream cycle, feeding
//! the Strategy Selector with normalized scores.

use serde::{Deserialize, Serialize};

/// Signal type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    Quality,
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
    pub never_recalled_count: u32,
    pub skill_notes_total: u32,
    pub skill_notes_recalled: u32,
    pub correction_count: u32,
    pub session_count: u32,
}

impl SignalSnapshot {
    /// Build a snapshot from raw metrics, normalizing each to [0.0, 1.0].
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

        // Recall signals
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

        // Skill usage signals
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

        // Quality signals
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

    /// Get score by signal name, defaulting to 0.0 if not found.
    pub fn score(&self, name: &str) -> f64 {
        self.signals.iter().find(|s| s.name == name).map(|s| s.score).unwrap_or(0.0)
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
