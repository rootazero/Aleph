//! Mock data generators
//!
//! Provides realistic mock data for UI development and testing.

use crate::models::*;

/// Generate mock trace nodes for Agent thinking process
pub fn generate_mock_trace_nodes() -> Vec<TraceNode> {
    vec![
        TraceNode {
            id: "1".to_string(),
            node_type: TraceNodeType::Thinking,
            timestamp: js_sys::Date::now(),
            duration_ms: Some(150),
            content: "User asked about implementing a new feature...".to_string(),
            status: TraceStatus::Success,
            children: vec![],
        },
        TraceNode {
            id: "2".to_string(),
            node_type: TraceNodeType::Decision,
            timestamp: js_sys::Date::now() + 200.0,
            duration_ms: Some(50),
            content: "Need to search codebase for similar implementations".to_string(),
            status: TraceStatus::Success,
            children: vec![],
        },
        TraceNode {
            id: "3".to_string(),
            node_type: TraceNodeType::ToolCall,
            timestamp: js_sys::Date::now() + 300.0,
            duration_ms: None,
            content: "grep_search(pattern: \"feature\", path: \"src/\")".to_string(),
            status: TraceStatus::InProgress,
            children: vec![],
        },
    ]
}

/// Generate a single mock trace node (for streaming simulation)
pub fn generate_next_trace_node(index: usize) -> TraceNode {
    let templates = vec![
        (TraceNodeType::Thinking, "Analyzing the request..."),
        (TraceNodeType::Decision, "Decided to use the file_read tool"),
        (TraceNodeType::ToolCall, "file_read(path: \"src/main.rs\")"),
        (TraceNodeType::ToolResult, "Successfully read 245 lines"),
        (TraceNodeType::Observation, "Found the target function at line 123"),
        (TraceNodeType::Thinking, "Now I need to understand the context..."),
        (TraceNodeType::ToolCall, "grep_search(pattern: \"impl\", path: \"src/\")"),
        (TraceNodeType::ToolResult, "Found 15 matches"),
        (TraceNodeType::Decision, "Will modify the existing implementation"),
        (TraceNodeType::Thinking, "Preparing the code changes..."),
    ];

    let (node_type, content) = templates[index % templates.len()];
    let status = if index % 3 == 2 {
        TraceStatus::InProgress
    } else {
        TraceStatus::Success
    };

    TraceNode {
        id: format!("node-{}", index),
        node_type,
        timestamp: js_sys::Date::now(),
        duration_ms: if status == TraceStatus::Success {
            Some(50 + (index as u64 * 10) % 200)
        } else {
            None
        },
        content: content.to_string(),
        status,
        children: vec![],
    }
}

/// Generate mock memory statistics
pub fn generate_mock_memory_stats() -> MemoryStats {
    MemoryStats {
        count: 1247,
        size_bytes: 5_242_880, // 5 MB
        apps_count: 12,
    }
}

/// Generate mock memory search results
pub fn generate_mock_memory_search() -> Vec<MemorySearchItem> {
    vec![
        MemorySearchItem {
            id: "fact-1".to_string(),
            content: "User prefers using Rust for system programming".to_string(),
            score: 0.95,
            timestamp: js_sys::Date::now() - 86400000.0, // 1 day ago
        },
        MemorySearchItem {
            id: "fact-2".to_string(),
            content: "Project uses Leptos framework for UI development".to_string(),
            score: 0.89,
            timestamp: js_sys::Date::now() - 172800000.0, // 2 days ago
        },
        MemorySearchItem {
            id: "fact-3".to_string(),
            content: "Dashboard should follow Aleph design tokens".to_string(),
            score: 0.82,
            timestamp: js_sys::Date::now() - 259200000.0, // 3 days ago
        },
    ]
}

/// Generate mock tool metrics
pub fn generate_mock_tool_metrics() -> Vec<ToolMetrics> {
    vec![
        ToolMetrics {
            name: "file_read".to_string(),
            total_calls: 145,
            success_count: 142,
            failed_count: 3,
            avg_duration_ms: 23.5,
        },
        ToolMetrics {
            name: "grep_search".to_string(),
            total_calls: 89,
            success_count: 87,
            failed_count: 2,
            avg_duration_ms: 156.3,
        },
        ToolMetrics {
            name: "file_write".to_string(),
            total_calls: 67,
            success_count: 65,
            failed_count: 2,
            avg_duration_ms: 45.2,
        },
        ToolMetrics {
            name: "bash_exec".to_string(),
            total_calls: 34,
            success_count: 32,
            failed_count: 2,
            avg_duration_ms: 892.7,
        },
    ]
}
