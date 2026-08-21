use crate::context::DashboardState;
use crate::memory_graph::adapter::SearchResultDto;
use serde::{Deserialize, Serialize};

/// Raw memory entry (Layer 1 — one conversation record).
///
/// **One body, not two halves.** This used to carry `user_input` / `ai_output`
/// so the card could style a question and an answer differently — but
/// `raw_memories` has a single `content` column, and the handler filled
/// `ai_output` with `String::new()` on every row ever sent. The Q/A card, the
/// `Q:`/`A:` export prefixes and the two-weight styling were rendering a
/// distinction the store cannot make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawMemory {
    pub id: String,
    #[serde(default)]
    pub agent_id: String,
    /// The recorded turn text.
    #[serde(default)]
    pub content: String,
    /// Session the row was recorded in, when known.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl RawMemory {
    /// The row body, for clipboard export and single-line previews.
    #[must_use]
    pub fn display_text(&self) -> String {
        self.content.clone()
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
    /// Which column the full-text index matched — `"title"` or `"content"`.
    /// Only set for rows built from a `graph.search` hit; `None` for a plain
    /// listing, where nothing was matched against.
    ///
    /// The server has always sent this and the DTO has always parsed it; no
    /// renderer ever read it, so a title hit and a body hit looked identical
    /// in the results list.
    #[serde(default)]
    pub match_field: Option<String>,
}

impl CompressedFact {
    /// Minimal fact for drill-into-note navigation: only `path`/`category`/
    /// `content`(title) are load-bearing for the detail views' fetch flow.
    #[must_use]
    pub fn stub_from_path(partition: &str, path: &str) -> Self {
        let (category, filename) = path.split_once('/').unwrap_or(("other", path));
        Self {
            id: path.to_string(),
            // A stub is an ADDRESS, and a note's address is (partition, path) —
            // a path alone does not say which store to look in now that one
            // list can span the union `memory_scope::read_partitions` resolves.
            // This used to be `String::new()`, which every caller then had to
            // paper over by re-reading the agent picker, i.e. by guessing.
            agent_id: partition.to_string(),
            content: filename.to_string(),
            fact_type: String::new(),
            created_at: 0,
            updated_at: 0,
            category: category.to_string(),
            path: path.to_string(),
            tags: Vec::new(),
            link_count: 0,
            // A navigation stub was not matched against anything.
            match_field: None,
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
            match_field: (!hit.match_field.is_empty()).then(|| hit.match_field.clone()),
        }
    }
}

/// Backend `list_facts` response wrapper.
#[derive(Debug, Clone, Deserialize)]
struct BackendListFactsResponse {
    #[serde(default)]
    facts: Vec<CompressedFact>,
    /// Total notes for the agent, independent of `limit`/`offset`. `None`
    /// when an un-upgraded core doesn't send the field at all — NOT the same
    /// as `0`, which would read as "the store is empty" and silently hide
    /// the 1000-note truncation notice.
    #[serde(default)]
    total: Option<u64>,
}

/// Backend `memory.search` response wrapper.
#[derive(Debug, Clone, Deserialize)]
struct BackendSearchResponse {
    #[serde(default)]
    memories: Vec<BackendMemoryEntry>,
    /// Rows matching the same filter, independent of `limit`/`offset`. `None`
    /// when an un-upgraded core doesn't send the field at all, which the
    /// pager reads as genuinely unknown. Defaulting this to `0` instead would
    /// read as "there are no more rows" and make the raw pager's prev/next
    /// controls vanish entirely against a skewed core.
    #[serde(default)]
    total: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct BackendMemoryEntry {
    id: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    content: String,
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
    /// A curated hot-tier fact: one row per write ATTEMPT, refusals included.
    /// The answer to "why was this never remembered" — the server has
    /// serialised these all along, and no client could ask for them.
    WriteDecision,
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
    /// Curated write attempts matching the target, newest first. Only
    /// populated for [`TraceKind::WriteDecision`] (the server omits the field
    /// entirely when empty, hence `default`).
    #[serde(default)]
    pub write_decisions: Vec<WriteDecisionRow>,
}

/// One curated-memory write attempt, as recorded in `memory_write_decisions`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WriteDecisionRow {
    /// `add` / `replace` / `remove` / `batch`, or `flag_correction`.
    #[serde(default)]
    pub action: String,
    /// Why it landed or did not — a server-side enum, never free text.
    #[serde(default)]
    pub reason: String,
    /// Bounded excerpt of what was attempted.
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub created_at: i64,
}

/// One curated hot-memory entry (`MEMORY.md`, the block injected into every
/// system prompt).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CuratedEntry {
    pub text: String,
    /// Chars this entry costs against the budget — counted server-side in
    /// **chars, not bytes**, the same way the store bills it. Deriving it
    /// here would be a second accounting that disagrees on CJK.
    #[serde(default)]
    pub chars: usize,
}

/// The whole curated tier plus its budget, as one snapshot.
///
/// Mutations return this too, so the list the Panel renders after an edit is
/// the server's own post-write state rather than a locally patched guess.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CuratedSnapshot {
    #[serde(default)]
    pub entries: Vec<CuratedEntry>,
    #[serde(default)]
    pub usage_chars: usize,
    #[serde(default)]
    pub usage_pct: u8,
    #[serde(default)]
    pub limit: usize,
    /// The file is still in pre-curation markdown form; `remember(add)` is
    /// blocked until it is split into entries.
    #[serde(default)]
    pub legacy: bool,
    /// Server's one-line outcome for a mutation; absent on reads.
    #[serde(default)]
    pub message: Option<String>,
}

pub struct MemoryApi;

impl MemoryApi {
    /// Browse / filter raw memories (Layer 1).
    ///
    /// `query` is a substring filter over raw content. This never returns
    /// notes — note full-text search is `GraphApi::search`.
    /// Returns the page plus the **filtered** row count, so a pager over a
    /// query result sizes itself to the matches rather than to the whole
    /// store. `None` when an un-upgraded core didn't report a total at all —
    /// the pager falls back to its own "this page came back full" heuristic.
    pub async fn browse_raw(
        state: &DashboardState,
        agent_id: &str,
        query: String,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<RawMemory>, Option<u64>), String> {
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
                content: entry.content,
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

    /// List knowledge notes (Layer 2). Returns the page plus the agent's
    /// total; `None` when an un-upgraded core didn't send one.
    pub async fn list_facts(
        state: &DashboardState,
        agent_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<CompressedFact>, Option<u64>), String> {
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

    /// Read the curated hot tier (`MEMORY.md`) plus its budget usage.
    ///
    /// `agent_id` is the **base** agent id, exactly as it appears in the
    /// agent picker. The server composes the caller's own scope; sending a
    /// composed id is refused there, so never compose one here.
    pub async fn curated_list(
        state: &DashboardState,
        agent_id: &str,
    ) -> Result<CuratedSnapshot, String> {
        let result = state
            .rpc_call(
                "memory.curated.list",
                serde_json::json!({ "agent_id": agent_id }),
            )
            .await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse memory.curated.list: {e}"))
    }

    /// Rewrite the single entry matching `old_text`; returns the new snapshot.
    pub async fn curated_replace(
        state: &DashboardState,
        agent_id: &str,
        old_text: &str,
        content: &str,
    ) -> Result<CuratedSnapshot, String> {
        let result = state
            .rpc_call(
                "memory.curated.replace",
                serde_json::json!({
                    "agent_id": agent_id,
                    "old_text": old_text,
                    "content": content,
                }),
            )
            .await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse memory.curated.replace: {e}"))
    }

    /// Drop the single entry matching `old_text`; returns the new snapshot.
    pub async fn curated_remove(
        state: &DashboardState,
        agent_id: &str,
        old_text: &str,
    ) -> Result<CuratedSnapshot, String> {
        let result = state
            .rpc_call(
                "memory.curated.remove",
                serde_json::json!({ "agent_id": agent_id, "old_text": old_text }),
            )
            .await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse memory.curated.remove: {e}"))
    }

    /// List user corrections (`flag_user_correction` rows) and whether the
    /// dream daemon has distilled each one yet.
    pub async fn list_corrections(
        state: &DashboardState,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<CorrectionRow>, String> {
        let result = state
            .rpc_call(
                "memory.list_corrections",
                serde_json::json!({ "agent_id": agent_id, "limit": limit }),
            )
            .await?;
        let parsed: CorrectionsResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse memory.list_corrections: {e}"))?;
        Ok(parsed.corrections)
    }
}

/// One user correction awaiting (or past) distillation into the feedback tier.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CorrectionRow {
    pub id: String,
    #[serde(default)]
    pub content: String,
    /// `low` / `medium` / `high`, as the tool recorded it.
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub suggested_rule: Option<String>,
    /// `"pending"` or `"distilled"` — derived server-side from the
    /// FeedbackDistill watermark, not from a row flag.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Deserialize)]
struct CorrectionsResponse {
    #[serde(default)]
    corrections: Vec<CorrectionRow>,
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
    use super::{
        BackendListFactsResponse, BackendSearchResponse, CompressedFact, CuratedSnapshot,
        MemoryStats, RawMemory, TraceResult,
    };
    use crate::memory_graph::adapter::SearchResultDto;

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

    // ── Version skew: `total` absent, not zero ──────────────────────────────
    //
    // Mirrors `memory_graph::adapter::search_result_dto_deserializes_without_
    // the_new_fields`: a narrow response from an un-upgraded core (Panel
    // connected to an older gateway over LAN) must still parse, and the
    // missing `total` must come back `None` — deserialization already
    // succeeded before this fix (that's exactly why a `u64` default of `0`
    // hid as "the store is empty" instead of "unknown").

    #[test]
    fn list_facts_response_total_is_none_when_the_core_never_sent_it() {
        let json = r#"{"facts": []}"#;
        let resp: BackendListFactsResponse =
            serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(resp.total, None);
    }

    #[test]
    fn search_response_total_is_none_when_the_core_never_sent_it() {
        let json = r#"{"memories": []}"#;
        let resp: BackendSearchResponse =
            serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(resp.total, None);
    }

    #[test]
    fn stub_from_path_splits_category_and_filename() {
        let fact = CompressedFact::stub_from_path("main", "facts/rust-notes.md");
        assert_eq!(fact.id, "facts/rust-notes.md");
        assert_eq!(fact.path, "facts/rust-notes.md");
        assert_eq!(fact.category, "facts");
        assert_eq!(fact.content, "rust-notes.md");
        assert_eq!(
            fact.agent_id, "main",
            "a stub is an address, so it carries the partition it was built for \
             rather than an empty string every caller then has to guess around"
        );
        assert_eq!(fact.created_at, 0);
        assert!(fact.tags.is_empty());
        assert_eq!(fact.link_count, 0);
    }

    #[test]
    fn stub_from_path_falls_back_to_other_for_bare_filename() {
        let fact = CompressedFact::stub_from_path("main", "rust-notes.md");
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

    /// The row body arrives verbatim: no `Q:` / `A:` prefixes are synthesised
    /// around it. Those existed because the DTO carried two halves, and the
    /// server filled the second with `""` on every row it ever sent.
    #[test]
    fn raw_display_text_is_the_body_verbatim() {
        let row = RawMemory {
            id: "r1".into(),
            agent_id: "main".into(),
            content: "user asked about phantom pages".into(),
            session_id: None,
            created_at: None,
        };
        assert_eq!(row.display_text(), "user asked about phantom pages");
    }

    /// A curated snapshot must survive a mutation response, whose only extra
    /// field is the outcome `message`; a read has no `message` at all and
    /// must still parse.
    #[test]
    fn curated_snapshot_parses_with_and_without_the_outcome_message() {
        let read: CuratedSnapshot = serde_json::from_str(
            r#"{"entries":[{"text":"likes tea","chars":9}],"usage_chars":12,
                "usage_pct":6,"limit":200,"legacy":false}"#,
        )
        .expect("read shape");
        assert_eq!(read.entries.len(), 1);
        assert_eq!(read.entries[0].chars, 9);
        assert_eq!(read.message, None);

        let written: CuratedSnapshot = serde_json::from_str(
            r#"{"entries":[],"usage_chars":0,"usage_pct":0,"limit":200,
                "legacy":false,"message":"Entry removed."}"#,
        )
        .expect("mutation shape");
        assert_eq!(written.message.as_deref(), Some("Entry removed."));
    }

    /// `write_decisions` is omitted entirely for the evidence-chain kinds
    /// (the server skips it when empty), so the field must default rather
    /// than make those responses unparseable.
    #[test]
    fn trace_result_parses_with_and_without_write_decisions() {
        let evidence_kind: TraceResult =
            serde_json::from_str(r#"{"target":"habits/x","notes":["habits/x"],"evidence":[]}"#)
                .expect("evidence shape");
        assert!(evidence_kind.write_decisions.is_empty());

        let decisions: TraceResult = serde_json::from_str(
            r#"{"target":"tea","notes":[],"evidence":[],
                "write_decisions":[{"action":"add","reason":"over_budget",
                "subject":"likes tea","created_at":17}]}"#,
        )
        .expect("write-decision shape");
        assert_eq!(decisions.write_decisions.len(), 1);
        assert_eq!(decisions.write_decisions[0].reason, "over_budget");
    }
}
