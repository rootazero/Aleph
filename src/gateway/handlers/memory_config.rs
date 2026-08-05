//! Memory Configuration Handlers
//!
//! RPC handlers for managing memory/RAG configuration:
//! - `memory_config.get`: Get current memory configuration
//! - `memory_config.update`: Update memory configuration
//! - `memory.retrieve_with_trace`: Retrieve memories with per-stage scoring trace
//!
//! All modifications are persisted to config file and broadcast as events.
//!
//! Note: Rerank configuration has its own dedicated handlers in `rerank_config`.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::config::Config;
use crate::error::AlephError;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::indexer::NoteIndexer;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::routing::DEFAULT_AGENT_ID;

/// Handle `memory_config.get` request
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;

    let mut memory_config =
        serde_json::to_value(&cfg.memory).unwrap_or_else(|_| serde_json::json!({}));

    // Bridge the compression scheduling policy into the memory payload. These
    // knobs physically live in `policies.memory.compression` (not in
    // `MemoryConfig`), but the panel surfaces them on the Memory & Knowledge
    // page, so we project them under a `compression` key the panel reads/writes.
    if let serde_json::Value::Object(ref mut map) = memory_config {
        map.insert(
            "compression".to_string(),
            project_compression(&cfg.policies.memory.compression),
        );
    }

    JsonRpcResponse::success(request.id, memory_config)
}

/// Handle `memory_config.update` request
///
/// Uses JSON merge to update only the fields provided by the caller,
/// preserving any fields not present in the incoming payload (e.g.
/// embedding, rerank, assembler).
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params as raw JSON value
    let mut incoming = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    // Pull the bridged compression policy out before merging the remainder into
    // `MemoryConfig` — it targets `policies.memory.compression`, not the memory
    // section. Stripping keeps the memory merge clean.
    let compression_update = incoming
        .as_object_mut()
        .and_then(|m| m.remove("compression"));

    // Merge: read existing config as JSON, overlay incoming fields, deserialize back
    {
        let mut cfg = config.write().await;

        // Serialize existing memory config to JSON
        let mut base = match serde_json::to_value(&cfg.memory) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize existing config: {e}"),
                )
            }
        };

        // Merge incoming fields on top of existing (only overwrites keys present in incoming)
        json_merge(&mut base, &incoming);

        // Deserialize merged JSON back to MemoryConfig
        let merged: crate::config::types::memory::MemoryConfig = match serde_json::from_value(base)
        {
            Ok(c) => c,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid memory config after merge: {e}"),
                )
            }
        };

        cfg.memory = merged;

        // Apply the bridged compression policy (partial-update tolerant) and
        // mark its section for persistence alongside memory.
        let mut sections: Vec<&str> = vec!["memory"];
        if let Some(comp) = compression_update {
            apply_compression_update(&mut cfg.policies.memory.compression, &comp);
            sections.push("policies.memory.compression");
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&sections) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("memory".to_string()),
        value: serde_json::json!({ "action": "updated" }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

// ============================================================================
// Retrieve with Trace
// ============================================================================

/// Max chars of note content returned per traced result (debug panel only).
const TRACE_CONTENT_MAX: usize = 280;

#[derive(Debug, Default, Deserialize)]
struct RetrieveTraceParams {
    query: Option<String>,
    agent_id: Option<String>,
    limit: Option<usize>,
}

/// UTF-8-safe truncation to `max` chars (no panic on multi-byte boundaries).
/// No marker: this is trace payload, not display text.
fn truncate_chars(s: &str, max: usize) -> String {
    crate::utils::text_format::truncate_chars(s, max).to_string()
}

/// Trim a raw query param; `None` when absent or blank.
fn normalized_query(raw: Option<&str>) -> Option<String> {
    let q = raw.map_or("", str::trim);
    if q.is_empty() {
        None
    } else {
        Some(q.to_string())
    }
}

/// Stand-in embedder used when no real embedding provider is configured.
/// Its `embed` always errors, which makes `NoteFactRetrieval::retrieve_traced`
/// fall back to FTS-only search (recorded as the `fts_search` stage).
struct UnavailableEmbedder;

#[async_trait]
impl EmbeddingProvider for UnavailableEmbedder {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Err(AlephError::config("embedding provider not configured"))
    }
    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Err(AlephError::config("embedding provider not configured"))
    }
    fn dimensions(&self) -> usize {
        0
    }
    fn model_name(&self) -> &str {
        "unavailable"
    }
    fn provider_id(&self) -> &str {
        "unavailable"
    }
}

/// Handle `memory.retrieve_with_trace` request
///
/// Real retrieval trace: runs the scoring pipeline and returns per-stage
/// telemetry + scored results for the Settings ▸ Memory debug panel.
///
/// P1 partition isolation (spec §11-1c): takes a caller-supplied `agent_id`
/// exactly like `memory.search`, and returns note CONTENT — a superset of
/// what `memory.search` discloses, since the per-result `content` is the note
/// body (truncated to `TRACE_CONTENT_MAX`, not withheld). An invisible
/// partition therefore gets the same "ran, found nothing" shape an unused
/// partition produces: empty stages, empty results, no oracle. Checked
/// BEFORE the retrieval pipeline is built, so a denied caller never touches
/// the note store.
pub async fn handle_retrieve_with_trace(
    request: JsonRpcRequest,
    db: MemoryBackend,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    app_config: Arc<tokio::sync::RwLock<crate::Config>>,
) -> JsonRpcResponse {
    let params: RetrieveTraceParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let query = match normalized_query(params.query.as_deref()) {
        Some(q) => q,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'query' parameter")
        }
    };
    let agent_id = params
        .agent_id
        .unwrap_or_else(|| DEFAULT_AGENT_ID.to_string());

    // P1 partition isolation — see this fn's doc. Same empty shape a
    // partition with no matching notes produces; the default (no suffix)
    // always passes, so the common path is unaffected.
    if !crate::gateway::visibility::partition_visible(&agent_id) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        return JsonRpcResponse::success(
            request.id,
            json!({
                "query": query,
                "trace": { "query": query, "timestamp": now_ms, "stages": [] },
                "results": [],
            }),
        );
    }

    let limit = params.limit.unwrap_or(10);

    // Snapshot the three scoring configs, then drop the lock before retrieval.
    let (rerank_cfg, scoring_cfg, expansion_cfg) = {
        let cfg = app_config.read().await;
        (
            cfg.memory.rerank.clone(),
            cfg.memory.retrieval_scoring.clone(),
            cfg.memory.expansion.clone(),
        )
    };

    let memory_dir = match crate::utils::paths::get_note_memory_dir() {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("note memory dir unavailable: {e}"),
            );
        }
    };
    let indexer = Arc::new(NoteIndexer::new(memory_dir, Arc::clone(&db)));
    let embedder: Arc<dyn EmbeddingProvider> =
        embedder.unwrap_or_else(|| Arc::new(UnavailableEmbedder));

    let retrieval = NoteFactRetrieval::new(indexer, embedder)
        .with_rerank_config(&rerank_cfg)
        .with_scoring_config(&scoring_cfg)
        .with_expansion_config(&expansion_cfg);

    let (results, stages) = match retrieval.retrieve_traced(&query, &agent_id, limit).await {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("retrieval failed: {e}"),
            );
        }
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let stages_json: Vec<_> = stages
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "duration_ms": s.duration_ms,
                "input_count": s.input_count,
                "output_count": s.output_count,
            })
        })
        .collect();

    let results_json: Vec<_> = results
        .iter()
        .map(|sf| {
            json!({
                "id": sf.fact.id,
                "content": truncate_chars(&sf.fact.content, TRACE_CONTENT_MAX),
                "score": sf.score,
            })
        })
        .collect();

    JsonRpcResponse::success(
        request.id,
        json!({
            "query": query,
            "trace": {
                "query": query,
                "timestamp": now_ms,
                "stages": stages_json,
            },
            "results": results_json,
        }),
    )
}

// ============================================================================
// Helpers
// ============================================================================

/// Recursively merge `overlay` into `base`.
/// For objects, overlay keys overwrite base keys; for all other types the
/// overlay value replaces the base value entirely.
fn json_merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    use serde_json::Value;
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let entry = base_map.entry(key.clone()).or_insert(Value::Null);
                json_merge(entry, overlay_val);
            }
        }
        (base, overlay) => {
            *base = overlay.clone();
        }
    }
}

/// Project the compression scheduling policy into the JSON shape the panel
/// reads under the `compression` key (see [`handle_get`]).
fn project_compression(c: &crate::config::CompressionPolicy) -> serde_json::Value {
    json!({
        "turn_threshold": c.turn_threshold,
        "background_interval_seconds": c.background_interval_seconds,
    })
}

/// Apply a (possibly partial) `compression` payload from the panel back onto
/// the compression policy. Missing or malformed fields are left untouched so
/// the update is tolerant of partial payloads.
fn apply_compression_update(
    policy: &mut crate::config::CompressionPolicy,
    comp: &serde_json::Value,
) {
    if let Some(v) = comp.get("turn_threshold").and_then(|x| x.as_u64()) {
        policy.turn_threshold = v as u32;
    }
    if let Some(v) = comp
        .get("background_interval_seconds")
        .and_then(|x| x.as_u64())
    {
        policy.background_interval_seconds = v as u32;
    }
}

#[cfg(test)]
mod retrieve_trace_tests {
    use super::{normalized_query, truncate_chars};

    #[test]
    fn truncate_chars_is_utf8_safe_and_bounds_length() {
        // Multi-byte chars must not panic and must cut on a char boundary.
        let s = "中文字符测试内容"; // 8 chars, 3 bytes each
        let out = truncate_chars(s, 4);
        assert_eq!(out, "中文字符");
        // Shorter-than-limit returns the whole string.
        assert_eq!(truncate_chars("abc", 10), "abc");
        // Exact length returns whole string.
        assert_eq!(truncate_chars("abcd", 4), "abcd");
    }

    #[test]
    fn normalized_query_rejects_blank() {
        assert_eq!(normalized_query(None), None);
        assert_eq!(normalized_query(Some("   ")), None);
        assert_eq!(normalized_query(Some(" hi ")), Some("hi".to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CompressionPolicy;

    /// Final-review I6: `memory.retrieve_with_trace` returns note CONTENT — a
    /// superset of what the now-guarded `memory.search` discloses — off the
    /// same caller-supplied `agent_id`. A foreign partition must read as a
    /// real-but-empty one, and the retrieval pipeline must not run for it.
    #[tokio::test]
    async fn retrieve_with_trace_hides_a_foreign_partition() {
        use crate::gateway::caller_identity::CALLER_USER;
        use crate::memory::store::sqlite::SqliteMemoryBackend;

        let db: MemoryBackend =
            Arc::new(SqliteMemoryBackend::in_memory().expect("in-memory backend"));
        let cfg = Arc::new(tokio::sync::RwLock::new(crate::Config::default()));
        let req = JsonRpcRequest::with_id(
            "memory.retrieve_with_trace",
            Some(json!({ "agent_id": "main__u-alice", "query": "address" })),
            json!(1),
        );

        let denied = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_retrieve_with_trace(req, db, None, cfg),
            )
            .await;

        let v = denied.result.expect("success, never an error");
        assert!(v["results"].as_array().expect("results").is_empty());
        assert!(v["trace"]["stages"].as_array().expect("stages").is_empty());
        // The query is echoed, exactly as on the allowed path — the denial
        // must not be identifiable by a missing field either.
        assert_eq!(v["query"], "address");
    }

    #[test]
    fn project_compression_emits_panel_shape() {
        let policy = CompressionPolicy {
            turn_threshold: 7,
            background_interval_seconds: 999,
        };
        let v = project_compression(&policy);
        assert_eq!(v["turn_threshold"], 7);
        assert_eq!(v["background_interval_seconds"], 999);
    }

    #[test]
    fn apply_compression_update_routes_all_fields() {
        let mut policy = CompressionPolicy::default();
        let comp = json!({
            "turn_threshold": 9,
            "background_interval_seconds": 4242,
        });
        apply_compression_update(&mut policy, &comp);
        assert_eq!(policy.turn_threshold, 9);
        assert_eq!(policy.background_interval_seconds, 4242);
    }

    #[test]
    fn apply_compression_update_is_partial_tolerant() {
        let mut policy = CompressionPolicy {
            turn_threshold: 20,
            background_interval_seconds: 30,
        };
        // Only one field present; the others must be preserved.
        apply_compression_update(&mut policy, &json!({ "turn_threshold": 99 }));
        assert_eq!(policy.turn_threshold, 99);
        assert_eq!(policy.background_interval_seconds, 30);

        // Malformed (non-numeric) value is ignored, not panicked on.
        apply_compression_update(&mut policy, &json!({ "turn_threshold": "oops" }));
        assert_eq!(policy.turn_threshold, 99);
    }
}
