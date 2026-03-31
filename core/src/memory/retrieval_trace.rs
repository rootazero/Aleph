//! Retrieval trace for debugging the scoring pipeline.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetrievalTrace {
    pub query: String,
    pub timestamp: i64,
    pub stages: Vec<TraceStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStage {
    pub name: String,
    pub duration_ms: u64,
    pub input_count: usize,
    pub output_count: usize,
    pub scores: Vec<ScoreSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreSnapshot {
    pub fact_id: String,
    pub score: f32,
    pub rank: usize,
}

impl RetrievalTrace {
    /// Create a new trace for the given query.
    pub fn new(query: impl Into<String>, timestamp: i64) -> Self {
        Self {
            query: query.into(),
            timestamp,
            stages: Vec::new(),
        }
    }

    /// Record a pipeline stage with its scored facts.
    ///
    /// `scored_facts` contains `(fact_id, score)` pairs; rank is assigned as index+1.
    pub fn record_stage(
        &mut self,
        name: impl Into<String>,
        duration_ms: u64,
        input_count: usize,
        scored_facts: &[(String, f32)],
    ) {
        let scores: Vec<ScoreSnapshot> = scored_facts
            .iter()
            .enumerate()
            .map(|(i, (id, score))| ScoreSnapshot {
                fact_id: id.clone(),
                score: *score,
                rank: i + 1,
            })
            .collect();

        self.stages.push(TraceStage {
            name: name.into(),
            duration_ms,
            input_count,
            output_count: scores.len(),
            scores,
        });
    }

    /// Total duration across all recorded stages.
    pub fn total_duration_ms(&self) -> u64 {
        self.stages.iter().map(|s| s.duration_ms).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_records_stages() {
        let mut trace = RetrievalTrace::new("test query", 1000);
        let facts = vec![("fact-1".to_string(), 0.95), ("fact-2".to_string(), 0.80)];

        trace.record_stage("vector_search", 42, 100, &facts);

        assert_eq!(trace.stages.len(), 1);
        let stage = &trace.stages[0];
        assert_eq!(stage.name, "vector_search");
        assert_eq!(stage.duration_ms, 42);
        assert_eq!(stage.input_count, 100);
        assert_eq!(stage.output_count, 2);
        assert_eq!(stage.scores[0].fact_id, "fact-1");
        assert!((stage.scores[0].score - 0.95).abs() < f32::EPSILON);
        assert_eq!(stage.scores[0].rank, 1);
        assert_eq!(stage.scores[1].rank, 2);
    }

    #[test]
    fn total_duration_sums_stages() {
        let mut trace = RetrievalTrace::new("query", 2000);
        trace.record_stage("stage_a", 30, 50, &[("f1".to_string(), 0.9)]);
        trace.record_stage("stage_b", 70, 10, &[]);

        assert_eq!(trace.total_duration_ms(), 100);
    }
}
