//! Inline scoring-pipeline telemetry for `NoteFactRetrieval::retrieve_traced`.
//! Observational only: the hot `retrieve()` path uses `TraceSink::Off`, whose
//! `record` is a no-op and allocates nothing.

/// One scoring-pipeline stage's measured telemetry.
#[derive(Debug, Clone, PartialEq)]
pub struct StageTrace {
    /// Stage name, e.g. "hybrid_search", "rerank", "truncate".
    pub name: String,
    /// Wall-clock time spent in the stage.
    pub duration_ms: u64,
    /// Working-set size entering the stage.
    pub input_count: usize,
    /// Working-set size leaving the stage.
    pub output_count: usize,
}

/// Collects per-stage telemetry only in traced mode. `Off` is the hot path:
/// `record` is a no-op and no `Vec` is allocated.
pub enum TraceSink {
    Off,
    On(Vec<StageTrace>),
}

impl TraceSink {
    /// Record one stage. No-op when `Off`.
    pub fn record(&mut self, name: &str, duration_ms: u64, input_count: usize, output_count: usize) {
        if let Self::On(stages) = self {
            stages.push(StageTrace {
                name: name.to_string(),
                duration_ms,
                input_count,
                output_count,
            });
        }
    }

    /// Consume the sink, returning collected stages (empty when `Off`).
    pub fn into_stages(self) -> Vec<StageTrace> {
        match self {
            Self::On(stages) => stages,
            Self::Off => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_sink_off_records_nothing() {
        let mut s = TraceSink::Off;
        s.record("x", 1, 2, 3);
        assert!(s.into_stages().is_empty());
    }

    #[test]
    fn trace_sink_on_collects_in_order() {
        let mut s = TraceSink::On(Vec::new());
        s.record("a", 1, 0, 5);
        s.record("b", 2, 5, 5);
        let stages = s.into_stages();
        assert_eq!(stages.len(), 2);
        assert_eq!(stages[0].name, "a");
        assert_eq!(stages[0].output_count, 5);
        assert_eq!(stages[1].input_count, 5);
    }
}
