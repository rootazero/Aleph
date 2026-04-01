//! Performance monitoring for memory system LLM calls

use std::fmt;

use serde::{Deserialize, Serialize};

/// Aggregated performance metrics for memory system
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryPerformanceReport {
    /// LLM scorer metrics
    pub llm_scorer: ComponentMetrics,

    /// Contradiction detector metrics
    pub contradiction_detector: ComponentMetrics,

    /// Total metrics across all components
    pub total: ComponentMetrics,
}

/// Performance metrics for a single component
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComponentMetrics {
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

impl ComponentMetrics {
    /// Get average latency in milliseconds
    pub fn avg_latency_ms(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.total_latency_ms as f64 / self.total_calls as f64
        }
    }

    /// Get cache hit rate (0.0-1.0)
    pub fn cache_hit_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.cache_hits as f64 / self.total_calls as f64
        }
    }

    /// Get error rate (0.0-1.0)
    pub fn error_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.errors as f64 / self.total_calls as f64
        }
    }

    /// Get timeout rate (0.0-1.0)
    pub fn timeout_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.timeouts as f64 / self.total_calls as f64
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

    /// Add metrics from another component
    pub fn add(&mut self, other: &ComponentMetrics) {
        self.total_calls += other.total_calls;
        self.cache_hits += other.cache_hits;
        self.total_latency_ms += other.total_latency_ms;
        self.timeouts += other.timeouts;
        self.errors += other.errors;
    }
}

impl MemoryPerformanceReport {
    /// Create a new empty report
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate total metrics
    pub fn calculate_total(&mut self) {
        self.total = ComponentMetrics::default();
        self.total.add(&self.llm_scorer);
        self.total.add(&self.contradiction_detector);
    }

    /// Format as human-readable text
    pub fn format_text(&self) -> String {
        format!(
            "Memory System Performance Report\n\
             ================================\n\n\
             LLM Scorer:\n\
             - Total calls: {}\n\
             - Cache hits: {} ({:.1}%)\n\
             - Avg latency: {:.0}ms\n\
             - Timeouts: {} ({:.1}%)\n\
             - Errors: {} ({:.1}%)\n\
             - Est. cost: ${:.4}\n\n\
             Contradiction Detector:\n\
             - Total calls: {}\n\
             - Cache hits: {} ({:.1}%)\n\
             - Avg latency: {:.0}ms\n\
             - Timeouts: {} ({:.1}%)\n\
             - Errors: {} ({:.1}%)\n\
             - Est. cost: ${:.4}\n\n\
             Total:\n\
             - Total calls: {}\n\
             - Cache hits: {} ({:.1}%)\n\
             - Avg latency: {:.0}ms\n\
             - Timeouts: {} ({:.1}%)\n\
             - Errors: {} ({:.1}%)\n\
             - Est. cost: ${:.4}",
            self.llm_scorer.total_calls,
            self.llm_scorer.cache_hits,
            self.llm_scorer.cache_hit_rate() * 100.0,
            self.llm_scorer.avg_latency_ms(),
            self.llm_scorer.timeouts,
            self.llm_scorer.timeout_rate() * 100.0,
            self.llm_scorer.errors,
            self.llm_scorer.error_rate() * 100.0,
            self.llm_scorer.estimated_cost_usd(),
            self.contradiction_detector.total_calls,
            self.contradiction_detector.cache_hits,
            self.contradiction_detector.cache_hit_rate() * 100.0,
            self.contradiction_detector.avg_latency_ms(),
            self.contradiction_detector.timeouts,
            self.contradiction_detector.timeout_rate() * 100.0,
            self.contradiction_detector.errors,
            self.contradiction_detector.error_rate() * 100.0,
            self.contradiction_detector.estimated_cost_usd(),
            self.total.total_calls,
            self.total.cache_hits,
            self.total.cache_hit_rate() * 100.0,
            self.total.avg_latency_ms(),
            self.total.timeouts,
            self.total.timeout_rate() * 100.0,
            self.total.errors,
            self.total.error_rate() * 100.0,
            self.total.estimated_cost_usd(),
        )
    }

    /// Format as JSON
    pub fn format_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

impl fmt::Display for MemoryPerformanceReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_metrics() {
        let mut metrics = ComponentMetrics {
            total_calls: 100,
            cache_hits: 30,
            total_latency_ms: 5000,
            timeouts: 2,
            errors: 1,
        };

        assert_eq!(metrics.avg_latency_ms(), 50.0);
        assert_eq!(metrics.cache_hit_rate(), 0.3);
        assert_eq!(metrics.error_rate(), 0.01);
        assert_eq!(metrics.timeout_rate(), 0.02);
        assert!(metrics.estimated_cost_usd() > 0.0);

        let other = ComponentMetrics {
            total_calls: 50,
            cache_hits: 20,
            total_latency_ms: 2500,
            timeouts: 1,
            errors: 0,
        };

        metrics.add(&other);
        assert_eq!(metrics.total_calls, 150);
        assert_eq!(metrics.cache_hits, 50);
        assert_eq!(metrics.total_latency_ms, 7500);
    }

    #[test]
    fn test_performance_report() {
        let mut report = MemoryPerformanceReport::new();
        report.llm_scorer = ComponentMetrics {
            total_calls: 100,
            cache_hits: 30,
            total_latency_ms: 5000,
            timeouts: 2,
            errors: 1,
        };
        report.contradiction_detector = ComponentMetrics {
            total_calls: 50,
            cache_hits: 20,
            total_latency_ms: 2500,
            timeouts: 1,
            errors: 0,
        };

        report.calculate_total();
        assert_eq!(report.total.total_calls, 150);
        assert_eq!(report.total.cache_hits, 50);

        let text = report.format_text();
        assert!(text.contains("Memory System Performance Report"));
        assert!(text.contains("Total calls: 100"));

        let json = report.format_json().unwrap();
        assert!(json.contains("llm_scorer"));
        assert!(json.contains("contradiction_detector"));
    }
}
