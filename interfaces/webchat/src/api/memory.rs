use crate::canvas_engine::adapter::SearchResultDto;
use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

/// Raw memory entry (Layer 1 — one conversation record).
///
/// `user_input` / `ai_output` stay separate: the card renders the two halves
/// with different weights, which a pre-joined `content` string made impossible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawMemory {
    pub id: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub user_input: String,
    #[serde(default)]
    pub ai_output: String,
    /// Session the row was recorded in, when known.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl RawMemory {
    /// Both halves as one string, for clipboard export and single-line previews.
    #[must_use]
    pub fn display_text(&self) -> String {
        match (self.user_input.is_empty(), self.ai_output.is_empty()) {
            (false, false) => format!("Q: {}\nA: {}", self.user_input, self.ai_output),
            (false, true) => self.user_input.clone(),
            _ => self.ai_output.clone(),
        }
    }
}

/// Compiled knowledge note (Layer 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompressedFact {
    pub id: String,
    #[serde(default)]
    pub agent_id: String,
    /// Display title (the note filename).
    pub content: String,
    pub fact_type: String,
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    pub category: String,
    pub path: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub link_count: usize,
}

impl CompressedFact {
    /// Minimal fact for drill-into-note navigation: only `path`/`category`/
    /// `content`(title) are load-bearing for the detail views' fetch flow.
    #[must_use]
    pub fn stub_from_path(path: &str) -> Self {
        let (category, filename) = path.split_once('/').unwrap_or(("other", path));
        Self {
            id: path.to_string(),
            agent_id: String::new(),
            content: filename.to_string(),
            fact_type: String::new(),
            created_at: 0,
            updated_at: 0,
            category: category.to_string(),
            path: path.to_string(),
            tags: Vec::new(),
            link_count: 0,
        }
    }

    /// Convert a `graph.search` hit into the same card model the note layers
    /// use. The hit carries the whole index row, so this needs no round trip.
    #[must_use]
    pub fn from_search_hit(hit: &SearchResultDto) -> Self {
        Self {
            id: hit.id.clone(),
            agent_id: hit.agent_id.clone(),
            content: hit.name.clone(),
            fact_type: hit.category.clone(),
            created_at: hit.created_at,
            updated_at: hit.updated_at,
            category: hit.category.clone(),
            path: hit.id.clone(),
            tags: hit.tags.clone(),
            link_count: hit.link_count,
        }
    }
}

/// Backend `list_facts` response wrapper.
#[derive(Debug, Clone, Deserialize)]
struct BackendListFactsResponse {
    #[serde(default)]
    facts: Vec<CompressedFact>,
    /// Total notes for the agent, independent of `limit`/`offset`.
    #[serde(default)]
    total: u64,
}

/// Backend `memory.search` response wrapper.
#[derive(Debug, Clone, Deserialize)]
struct BackendSearchResponse {
    #[serde(default)]
    memories: Vec<BackendMemoryEntry>,
    /// Rows matching the same filter, independent of `limit`/`offset`. Defaults
    /// to 0 against an un-upgraded core, which the pager reads as "unknown".
    #[serde(default)]
    total: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct BackendMemoryEntry {
    id: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    user_input: String,
    #[serde(default)]
    ai_output: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    timestamp: i64,
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
    /// `None` when the server answered store-wide: the note graph is per-agent,
    /// so there is no honest single number.
    #[serde(default)]
    pub total_graph_nodes: Option<u64>,
    #[serde(default)]
    pub total_graph_edges: Option<u64>,
    /// `"agent"` or `"global"` — which population the counts describe.
    #[serde(default)]
    pub scope: String,
}

/// Which kind of target `memory.trace` walks. Mirrors the server's `TraceKind`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    /// A note path: walk DOWN to the raw rows it was distilled from.
    Note,
    /// A raw memory id: walk UP to the notes citing it.
    Raw,
}

/// One piece of ground-truth evidence.
#[derive(Debug, Clone, Deserialize)]
pub struct EvidenceItem {
    pub raw_id: String,
    #[serde(default)]
    pub via_note: Option<String>,
    #[serde(default)]
    pub via_session: Option<String>,
    /// First 800 chars of raw content; `None` when `pruned`.
    #[serde(default)]
    pub content: Option<String>,
    /// The raw id was cited but its row is gone from the store.
    #[serde(default)]
    pub pruned: bool,
}

/// Result of walking the evidence chain.
#[derive(Debug, Clone, Deserialize)]
pub struct TraceResult {
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceItem>,
}

pub struct MemoryApi;

impl MemoryApi {
    /// Browse / filter raw memories (Layer 1).
    ///
    /// `query` is a substring filter over raw content. This never returns
    /// notes — note full-text search is `GraphApi::search`.
    /// Returns the page plus the **filtered** row count, so a pager over a
    /// query result sizes itself to the matches rather than to the whole store.
    pub async fn browse_raw(
        state: &DashboardState,
        agent_id: &str,
        query: String,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<RawMemory>, u64), String> {
        let params = serde_json::json!({
            "agent_id": agent_id,
            "query": query,
            "limit": limit,
            "offset": offset,
        });

        let result = state.rpc_call("memory.search", params).await?;
        let response: BackendSearchResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse memory.search: {e}"))?;

        let total = response.total;
        let rows = response
            .memories
            .into_iter()
            .map(|entry| RawMemory {
                id: entry.id,
                agent_id: entry.agent_id,
                user_input: entry.user_input,
                ai_output: entry.ai_output,
                session_id: entry.session_id,
                created_at: (entry.timestamp > 0).then(|| format_timestamp_secs(entry.timestamp)),
            })
            .collect();
        Ok((rows, total))
    }

    /// Delete one raw memory. Note deletion is `GraphApi::delete_note` —
    /// passing a note path here fails server-side by design.
    pub async fn delete(state: &DashboardState, memory_id: String) -> Result<(), String> {
        state
            .rpc_call("memory.delete", serde_json::json!({ "id": memory_id }))
            .await?;
        Ok(())
    }

    /// List knowledge notes (Layer 2). Returns the page plus the agent's total.
    pub async fn list_facts(
        state: &DashboardState,
        agent_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<CompressedFact>, u64), String> {
        let params = serde_json::json!({
            "agent_id": agent_id,
            "limit": limit,
            "offset": offset,
        });

        let result = state.rpc_call("memory.listFacts", params).await?;
        let response: BackendListFactsResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse memory.listFacts: {e}"))?;

        Ok((response.facts, response.total))
    }

    /// Memory statistics scoped to one agent, so the numbers describe the same
    /// population as the rows shown beneath them.
    pub async fn stats(state: &DashboardState, agent_id: &str) -> Result<MemoryStats, String> {
        let result = state
            .rpc_call("memory.stats", serde_json::json!({ "agent_id": agent_id }))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse memory.stats: {e}"))
    }

    /// Walk a memory claim down (or up) to ground-truth evidence.
    pub async fn trace(
        state: &DashboardState,
        agent_id: &str,
        target: &str,
        kind: TraceKind,
        max_results: usize,
    ) -> Result<TraceResult, String> {
        let params = serde_json::json!({
            "agent_id": agent_id,
            "target": target,
            "kind": kind,
            "max_results": max_results,
        });
        let result = state.rpc_call("memory.trace", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse memory.trace: {e}"))
    }
}

/// Format unix timestamp (seconds) to human-readable date string
fn format_timestamp_secs(ts: i64) -> String {
    // Simple date formatting for WASM (no chrono needed for basic display)
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    let year = date.get_full_year();
    let month = date.get_month() + 1; // 0-indexed
    let day = date.get_date();
    let hour = date.get_hours();
    let min = date.get_minutes();
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}")
}

#[cfg(test)]
mod tests {
    use super::{CompressedFact, MemoryStats, RawMemory};
    use crate::canvas_engine::adapter::SearchResultDto;

    /// The server sends `null` graph counts when they cannot be computed
    /// (store-wide scope). That must survive as `None`, not become `0` — a
    /// renderer that unwraps to `0` would draw an honest "unknown" as a false
    /// "empty graph".
    #[test]
    fn null_graph_counts_deserialize_to_none_not_zero() {
        let json = r#"{
            "totalFacts": 12,
            "totalMemories": 34,
            "validFacts": 10,
            "totalGraphNodes": null,
            "totalGraphEdges": null,
            "scope": "global"
        }"#;
        let stats: MemoryStats = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(stats.total_graph_nodes, None);
        assert_eq!(stats.total_graph_edges, None);
        assert_eq!(stats.scope, "global");
    }

    #[test]
    fn present_graph_counts_deserialize_to_some() {
        let json = r#"{
            "totalFacts": 12,
            "totalMemories": 34,
            "validFacts": 10,
            "totalGraphNodes": 7,
            "totalGraphEdges": 9,
            "scope": "agent"
        }"#;
        let stats: MemoryStats = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(stats.total_graph_nodes, Some(7));
        assert_eq!(stats.total_graph_edges, Some(9));
        assert_eq!(stats.scope, "agent");
    }

    #[test]
    fn stub_from_path_splits_category_and_filename() {
        let fact = CompressedFact::stub_from_path("facts/rust-notes.md");
        assert_eq!(fact.id, "facts/rust-notes.md");
        assert_eq!(fact.path, "facts/rust-notes.md");
        assert_eq!(fact.category, "facts");
        assert_eq!(fact.content, "rust-notes.md");
        assert_eq!(fact.agent_id, "");
        assert_eq!(fact.created_at, 0);
        assert!(fact.tags.is_empty());
        assert_eq!(fact.link_count, 0);
    }

    #[test]
    fn stub_from_path_falls_back_to_other_for_bare_filename() {
        let fact = CompressedFact::stub_from_path("rust-notes.md");
        assert_eq!(fact.category, "other");
        assert_eq!(fact.content, "rust-notes.md");
    }

    /// A search hit is a full note row, so it converts into the same card model
    /// the note layers use — no second round trip per row.
    #[test]
    fn from_search_hit_carries_the_whole_row() {
        let hit = SearchResultDto {
            id: "facts/deploy-notes".into(),
            name: "deploy-notes".into(),
            category: "facts".into(),
            match_field: "content".into(),
            agent_id: "main".into(),
            created_at: 1_700_000_000,
            updated_at: 1_700_009_999,
            tags: vec!["rust".into(), "ci".into()],
            link_count: 3,
        };
        let fact = CompressedFact::from_search_hit(&hit);
        assert_eq!(fact.path, "facts/deploy-notes");
        assert_eq!(fact.content, "deploy-notes");
        assert_eq!(fact.category, "facts");
        assert_eq!(fact.agent_id, "main");
        assert_eq!(fact.created_at, 1_700_000_000);
        assert_eq!(fact.updated_at, 1_700_009_999);
        assert_eq!(fact.tags, vec!["rust".to_string(), "ci".to_string()]);
        assert_eq!(fact.link_count, 3);
    }

    #[test]
    fn raw_display_text_joins_both_halves_only_when_present() {
        let both = RawMemory {
            id: "r1".into(),
            agent_id: "main".into(),
            user_input: "q".into(),
            ai_output: "a".into(),
            session_id: None,
            created_at: None,
        };
        assert_eq!(both.display_text(), "Q: q\nA: a");

        let q_only = RawMemory {
            id: "r2".into(),
            agent_id: "main".into(),
            user_input: "q".into(),
            ai_output: String::new(),
            session_id: None,
            created_at: None,
        };
        assert_eq!(q_only.display_text(), "q");

        let a_only = RawMemory {
            id: "r3".into(),
            agent_id: "main".into(),
            user_input: String::new(),
            ai_output: "a".into(),
            session_id: None,
            created_at: None,
        };
        assert_eq!(a_only.display_text(), "a");
    }
}
