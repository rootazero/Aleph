//! # 可观测性层（Observability Layer）
//!
//! Agent 行为追踪和指标收集（基础版本）。
//!
//! ## 核心组件
//!
//! - [`TraceNode`]: 追踪节点
//! - [`ToolMetrics`]: 工具调用指标
//!
//! ## 注意
//!
//! 完整的可观测性功能（包括 Leptos Signals 集成）需要启用 `observability` feature。
//! 当前版本提供基础的数据结构和类型定义。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Trace node type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TraceNodeType {
    /// Agent run session
    AgentRun,
    /// Observation phase
    Observation,
    /// Thinking phase
    Thinking,
    /// Tool call
    ToolCall,
    /// Memory retrieval
    MemoryRetrieval,
    /// User interaction
    UserInteraction,
}

/// Agent behavior trace node
///
/// Represents a single node in the Agent's execution trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceNode {
    /// Node ID
    pub id: String,
    /// Timestamp (milliseconds since epoch)
    pub timestamp: u64,
    /// Node type
    pub node_type: TraceNodeType,
    /// Duration in milliseconds (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Parent node ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Child node IDs
    #[serde(default)]
    pub children: Vec<String>,
    /// Additional metadata
    pub metadata: serde_json::Value,
}

/// Tool call metrics
///
/// Statistics about tool usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMetrics {
    /// Tool name
    pub tool_name: String,
    /// Total number of calls
    pub total_calls: u64,
    /// Number of successful calls
    pub success_count: u64,
    /// Number of failed calls
    pub failure_count: u64,
    /// Average duration in milliseconds
    pub avg_duration_ms: f64,
    /// Last called timestamp
    pub last_called: u64,
}

impl ToolMetrics {
    /// Create new tool metrics
    pub fn new(tool_name: String) -> Self {
        Self {
            tool_name,
            total_calls: 0,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0.0,
            last_called: 0,
        }
    }

    /// Record a tool call
    pub fn record_call(&mut self, duration_ms: u64, success: bool, timestamp: u64) {
        self.total_calls += 1;
        if success {
            self.success_count += 1;
        } else {
            self.failure_count += 1;
        }

        // Update average duration
        self.avg_duration_ms = (self.avg_duration_ms * (self.total_calls - 1) as f64
            + duration_ms as f64)
            / self.total_calls as f64;

        self.last_called = timestamp;
    }

    /// Get success rate (0.0 - 1.0)
    pub fn success_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.success_count as f64 / self.total_calls as f64
        }
    }

    /// Get failure rate (0.0 - 1.0)
    pub fn failure_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.failure_count as f64 / self.total_calls as f64
        }
    }
}

/// Simple metrics collector (non-reactive version)
///
/// For reactive version with Leptos Signals, enable the `observability` feature.
#[derive(Debug, Clone)]
pub struct MetricsCollector {
    tool_metrics: HashMap<String, ToolMetrics>,
    total_runs: u64,
    total_tokens: u64,
}

impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            tool_metrics: HashMap::new(),
            total_runs: 0,
            total_tokens: 0,
        }
    }

    /// Record a tool call
    pub fn record_tool_call(&mut self, tool_name: &str, duration_ms: u64, success: bool) {
        let timestamp = current_timestamp();
        let metrics = self
            .tool_metrics
            .entry(tool_name.to_string())
            .or_insert_with(|| ToolMetrics::new(tool_name.to_string()));

        metrics.record_call(duration_ms, success, timestamp);
    }

    /// Increment run count
    pub fn increment_runs(&mut self) {
        self.total_runs += 1;
    }

    /// Add tokens
    pub fn add_tokens(&mut self, tokens: u64) {
        self.total_tokens += tokens;
    }

    /// Get tool metrics
    pub fn tool_metrics(&self, tool_name: &str) -> Option<&ToolMetrics> {
        self.tool_metrics.get(tool_name)
    }

    /// Get all tool metrics
    pub fn all_tool_metrics(&self) -> &HashMap<String, ToolMetrics> {
        &self.tool_metrics
    }

    /// Get total runs
    pub fn total_runs(&self) -> u64 {
        self.total_runs
    }

    /// Get total tokens
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens
    }

    /// Get top N most used tools
    pub fn top_tools(&self, n: usize) -> Vec<ToolMetrics> {
        let mut tools: Vec<_> = self.tool_metrics.values().cloned().collect();
        tools.sort_by(|a, b| b.total_calls.cmp(&a.total_calls));
        tools.truncate(n);
        tools
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Get current timestamp (milliseconds since epoch)
fn current_timestamp() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metrics_record_call() {
        let mut metrics = ToolMetrics::new("test_tool".to_string());

        metrics.record_call(100, true, 1000);
        assert_eq!(metrics.total_calls, 1);
        assert_eq!(metrics.success_count, 1);
        assert_eq!(metrics.avg_duration_ms, 100.0);

        metrics.record_call(200, false, 2000);
        assert_eq!(metrics.total_calls, 2);
        assert_eq!(metrics.failure_count, 1);
        assert_eq!(metrics.avg_duration_ms, 150.0);
    }

    #[test]
    fn test_tool_metrics_rates() {
        let mut metrics = ToolMetrics::new("test_tool".to_string());

        metrics.record_call(100, true, 1000);
        metrics.record_call(100, true, 1000);
        metrics.record_call(100, false, 1000);

        assert_eq!(metrics.success_rate(), 2.0 / 3.0);
        assert_eq!(metrics.failure_rate(), 1.0 / 3.0);
    }

    #[test]
    fn test_metrics_collector() {
        let mut collector = MetricsCollector::new();

        collector.record_tool_call("tool1", 100, true);
        collector.record_tool_call("tool1", 200, true);
        collector.record_tool_call("tool2", 150, false);

        assert_eq!(collector.tool_metrics("tool1").unwrap().total_calls, 2);
        assert_eq!(collector.tool_metrics("tool2").unwrap().total_calls, 1);

        collector.increment_runs();
        collector.add_tokens(1000);

        assert_eq!(collector.total_runs(), 1);
        assert_eq!(collector.total_tokens(), 1000);
    }

    #[test]
    fn test_top_tools() {
        let mut collector = MetricsCollector::new();

        collector.record_tool_call("tool1", 100, true);
        collector.record_tool_call("tool1", 100, true);
        collector.record_tool_call("tool1", 100, true);
        collector.record_tool_call("tool2", 100, true);
        collector.record_tool_call("tool2", 100, true);
        collector.record_tool_call("tool3", 100, true);

        let top = collector.top_tools(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].tool_name, "tool1");
        assert_eq!(top[1].tool_name, "tool2");
    }

    #[test]
    fn test_trace_node_serialization() {
        let node = TraceNode {
            id: "node-1".to_string(),
            timestamp: 1234567890,
            node_type: TraceNodeType::ToolCall,
            duration_ms: Some(100),
            parent_id: Some("parent-1".to_string()),
            children: vec!["child-1".to_string()],
            metadata: serde_json::json!({"tool": "test"}),
        };

        let json = serde_json::to_string(&node).unwrap();
        let deserialized: TraceNode = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "node-1");
        assert_eq!(deserialized.node_type, TraceNodeType::ToolCall);
    }
}
