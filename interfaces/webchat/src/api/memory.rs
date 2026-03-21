use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

/// Raw memory entry (Layer 1 — user_input + ai_output conversation records)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMemory {
    pub id: String,
    /// Agent that produced this memory
    #[serde(default)]
    pub agent_id: String,
    /// Combined display content (mapped from user_input + ai_output)
    pub content: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// Compressed fact entry (Layer 2 — extracted from raw memories by compression)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedFact {
    pub id: String,
    #[serde(default)]
    pub agent_id: String,
    pub content: String,
    pub fact_type: String,
    pub confidence: f32,
    pub is_valid: bool,
    pub created_at: i64,
    pub category: String,
    pub path: String,
}

/// Backend list_facts response wrapper
#[derive(Debug, Clone, Deserialize)]
struct BackendListFactsResponse {
    #[serde(default)]
    facts: Vec<CompressedFact>,
}

/// Backend memory search result entry (matches handler MemoryEntry)
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct BackendMemoryEntry {
    id: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    window_title: String,
    #[serde(default)]
    user_input: String,
    #[serde(default)]
    ai_output: String,
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    similarity_score: Option<f32>,
}

/// Backend search response wrapper
#[derive(Debug, Clone, Deserialize)]
struct BackendSearchResponse {
    #[serde(default)]
    memories: Vec<BackendMemoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    #[serde(default)]
    pub total_facts: u64,
    #[serde(default)]
    pub total_memories: u64,
    #[serde(default)]
    pub valid_facts: u64,
    #[serde(default)]
    pub total_graph_nodes: u64,
    #[serde(default)]
    pub total_graph_edges: u64,
}

pub struct MemoryApi;

impl MemoryApi {
    /// Search raw memories (Layer 1)
    pub async fn search(
        state: &DashboardState,
        query: String,
        limit: Option<u32>,
    ) -> Result<Vec<RawMemory>, String> {
        let params = serde_json::json!({
            "query": query,
            "limit": limit,
        });

        let result = state.rpc_call("memory.search", params).await?;

        // Backend returns {"memories": [MemoryEntry...]}
        let response: BackendSearchResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse search results: {}", e))?;

        // Map backend entries to RawMemory
        let facts = response.memories.into_iter().map(|entry| {
            // Combine user_input and ai_output for display
            let content = if !entry.user_input.is_empty() && !entry.ai_output.is_empty() {
                format!("Q: {}\nA: {}", entry.user_input, entry.ai_output)
            } else if !entry.user_input.is_empty() {
                entry.user_input
            } else {
                entry.ai_output
            };

            // Format timestamp
            let created_at = if entry.timestamp > 0 {
                Some(format_timestamp_secs(entry.timestamp))
            } else {
                None
            };

            RawMemory {
                id: entry.id,
                agent_id: entry.agent_id,
                content,
                created_at,
            }
        }).collect();

        Ok(facts)
    }

    /// Delete a memory
    pub async fn delete(
        state: &DashboardState,
        memory_id: String,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "id": memory_id,
        });

        state.rpc_call("memory.delete", params).await?;
        Ok(())
    }

    /// List compressed facts (Layer 2)
    pub async fn list_facts(
        state: &DashboardState,
        limit: Option<usize>,
    ) -> Result<Vec<CompressedFact>, String> {
        let params = serde_json::json!({
            "limit": limit.unwrap_or(50),
        });

        let result = state.rpc_call("memory.listFacts", params).await?;

        let response: BackendListFactsResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse facts: {}", e))?;

        Ok(response.facts)
    }

    /// Get memory statistics
    pub async fn stats(state: &DashboardState) -> Result<MemoryStats, String> {
        let result = state.rpc_call("memory.stats", Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse stats: {}", e))
    }
}

/// Format unix timestamp (seconds) to human-readable date string
fn format_timestamp_secs(ts: i64) -> String {
    // Simple date formatting for WASM (no chrono needed for basic display)
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64((ts * 1000) as f64));
    let year = date.get_full_year();
    let month = date.get_month() + 1; // 0-indexed
    let day = date.get_date();
    let hour = date.get_hours();
    let min = date.get_minutes();
    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, month, day, hour, min)
}
