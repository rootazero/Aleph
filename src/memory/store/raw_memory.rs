use crate::error::AlephError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Source of raw memory data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawMemorySource {
    // Legacy — keep for backward compatibility.
    SessionCompressed,
    Transcript,
    ToolOutput,
    Attachment,

    // Spec 1 — Memory Capture Hooks.
    PreCompress,
    Delegation {
        child_agent_id: String,
    },
    SessionEnd {
        reason: SessionEndReason,
    },

    // Phase 3 self-evolution — user-correction signal written by the
    // `flag_user_correction` tool. `severity` is a String round-tripped
    // through `crate::memory::notes::Severity` at the boundary so this
    // module stays free of upward dependencies.
    Correction {
        severity: String,
        suggested_rule: Option<String>,
    },

    // Spec 3 (Dream signals) — one row per tool invocation. The `content`
    // field carries a short human-readable summary; structured stats live
    // in `source_detail` so SQL aggregations can extract them cheaply.
    // Fields are flat strings/primitives for on-disk format stability —
    // higher layers reconstruct any richer structure.
    ToolInvocation {
        tool_name: String,
        success: bool,
        duration_ms: u64,
    },

    // Batch 2 (session-end reflection) — a first-person "lessons learned"
    // distillation produced once a substantive session ends. Unlike the raw
    // `SessionEnd` transcript tail, the content is already condensed into
    // reusable lessons; it is ingestable (the compound ingestor turns it into
    // feedback/lessons notes) and never telemetry.
    Reflection,
}

/// Sub-reason for `RawMemorySource::SessionEnd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    /// Gateway close or idle timeout.
    Disconnect,
    /// LLM called the `session_complete` tool.
    TaskDone,
}

impl RawMemorySource {
    /// Split enum into `(token, optional_detail_json)` for `SQLite` storage.
    /// Legacy variants return `(token, None)` so existing rows stay unchanged.
    #[must_use]
    pub fn to_persisted(&self) -> (&'static str, Option<String>) {
        match self {
            Self::SessionCompressed => ("session_compressed", None),
            Self::Transcript => ("transcript", None),
            Self::ToolOutput => ("tool_output", None),
            Self::Attachment => ("attachment", None),
            Self::PreCompress => ("pre_compress", None),
            Self::Delegation { child_agent_id } => (
                "delegation",
                Some(serde_json::json!({ "child_agent_id": child_agent_id }).to_string()),
            ),
            Self::SessionEnd { reason } => (
                "session_end",
                Some(serde_json::json!({ "reason": reason }).to_string()),
            ),
            Self::Correction {
                severity,
                suggested_rule,
            } => (
                "correction",
                Some(
                    serde_json::json!({
                        "severity": severity,
                        "suggested_rule": suggested_rule,
                    })
                    .to_string(),
                ),
            ),
            Self::ToolInvocation {
                tool_name,
                success,
                duration_ms,
            } => (
                "tool_invocation",
                Some(
                    serde_json::json!({
                        "tool_name": tool_name,
                        "success": success,
                        "duration_ms": duration_ms,
                    })
                    .to_string(),
                ),
            ),
            Self::Reflection => ("reflection", None),
        }
    }

    /// Parse `(token, optional_detail_json)` back into the enum.
    /// Unknown tokens fall through to `ToolOutput` (matches old behaviour).
    pub fn from_persisted(token: &str, detail: Option<&str>) -> Self {
        match token {
            "session_compressed" => Self::SessionCompressed,
            "transcript" => Self::Transcript,
            "tool_output" => Self::ToolOutput,
            "attachment" => Self::Attachment,
            "pre_compress" => Self::PreCompress,
            "delegation" => {
                let child_agent_id = detail
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| {
                        v.get("child_agent_id")
                            .and_then(|x| x.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                Self::Delegation { child_agent_id }
            }
            "session_end" => {
                let reason = detail
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| v.get("reason").and_then(|x| x.as_str().map(str::to_string)))
                    .unwrap_or_else(|| "disconnect".into());
                let reason = match reason.as_str() {
                    "task_done" => SessionEndReason::TaskDone,
                    _ => SessionEndReason::Disconnect,
                };
                Self::SessionEnd { reason }
            }
            "correction" => {
                let parsed = detail
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                let severity = parsed
                    .get("severity")
                    .and_then(|x| x.as_str())
                    .map_or_else(|| "low".to_string(), str::to_string);
                let suggested_rule = parsed
                    .get("suggested_rule")
                    .and_then(|x| x.as_str())
                    .map(str::to_string);
                Self::Correction {
                    severity,
                    suggested_rule,
                }
            }
            "tool_invocation" => {
                let parsed = detail
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                let tool_name = parsed
                    .get("tool_name")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .unwrap_or_default();
                let success = parsed
                    .get("success")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                let duration_ms = parsed
                    .get("duration_ms")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                Self::ToolInvocation {
                    tool_name,
                    success,
                    duration_ms,
                }
            }
            "reflection" => Self::Reflection,
            _ => Self::ToolOutput,
        }
    }

    /// Backwards-compat shim — existing callers that only had a token.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        self.to_persisted().0
    }
}

/// A raw memory record — ephemeral data consumed by `CompressionService`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMemory {
    pub id: String,
    pub content: String,
    pub source: RawMemorySource,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub path: Option<String>,
    pub attachment_text: Option<String>,
    pub is_processed: bool,
    pub created_at: i64,
}

impl RawMemory {
    #[must_use]
    pub fn new(content: String, source: RawMemorySource) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content,
            source,
            agent_id: "default".to_string(),
            session_id: None,
            path: None,
            attachment_text: None,
            is_processed: false,
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = agent_id.into();
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

/// Storage trait for raw memory records.
#[async_trait]
pub trait RawMemoryStore: Send + Sync {
    /// Insert a raw memory record.
    async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError>;

    /// Like `insert_raw_memory`, but silently discards unique-constraint
    /// violations (first writer wins). Callers must re-read after the call to
    /// get the winning row when a race is possible.
    ///
    /// Default impl calls `insert_raw_memory` and swallows errors whose
    /// message contains "UNIQUE constraint failed". `SQLite` backends override
    /// this with `INSERT OR IGNORE` SQL for proper atomicity.
    async fn insert_raw_memory_or_ignore(&self, raw: &RawMemory) -> Result<(), AlephError> {
        match self.insert_raw_memory(raw).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("UNIQUE constraint failed") || msg.contains("unique constraint") {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Get unprocessed raw memories for an agent, ordered by `created_at` ASC.
    async fn get_unprocessed_raw_memories(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError>;

    /// Mark raw memories as processed after `CompressionService` consumes them.
    async fn mark_raw_as_processed(&self, ids: &[String]) -> Result<usize, AlephError>;

    /// Count unprocessed raw memories for an agent.
    async fn count_unprocessed(&self, agent_id: &str) -> Result<usize, AlephError>;

    /// List the distinct `agent_id` values that have at least one unprocessed
    /// raw memory. Used by `CompressionService::compress` to fan out across
    /// every agent rather than only the historical "main" default.
    /// Default impl returns just `["main"]` for backwards compatibility with
    /// in-memory test stores; the `SQLite` backend overrides with a real query.
    async fn unprocessed_agent_ids(&self) -> Result<Vec<String>, AlephError> {
        Ok(vec!["main".to_string()])
    }

    /// Get raw memories whose `path` starts with the given prefix, scoped to an agent.
    /// Used for session data retrieval (e.g. "<aleph://session/{id>}/").
    async fn get_raw_by_path_prefix(
        &self,
        path_prefix: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError>;

    /// Same as `get_raw_by_path_prefix`, but only returns rows with
    /// `created_at > since_created_at`. Used by stages that maintain a
    /// per-consumer watermark (e.g. `FeedbackDistill`) to avoid re-processing
    /// rows already distilled in a previous cycle.
    ///
    /// Default impl falls back to fetching the full prefix window and
    /// filtering client-side, so in-memory test stores keep working without
    /// a SQL change.
    async fn get_raw_by_path_prefix_since(
        &self,
        path_prefix: &str,
        agent_id: &str,
        since_created_at: i64,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        let all = self
            .get_raw_by_path_prefix(path_prefix, agent_id, limit)
            .await?;
        Ok(all
            .into_iter()
            .filter(|r| r.created_at > since_created_at)
            .collect())
    }

    /// Returns the single `RawMemory` row whose `(agent_id, path)` matches
    /// exactly, or `Ok(None)` if no such row exists. Default impl reuses
    /// `get_raw_by_path_prefix` and filters in-Rust; the `SQLite` backend
    /// overrides with an indexed exact-match SELECT for efficiency.
    async fn find_by_path(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Option<RawMemory>, AlephError> {
        let candidates = self.get_raw_by_path_prefix(path, agent_id, 4).await?;
        Ok(candidates
            .into_iter()
            .find(|r| r.path.as_deref() == Some(path)))
    }

    /// Fetch raw memory rows by their explicit ids, scoped to an agent.
    /// Returns an empty vec when `ids` is empty. Default: empty.
    async fn get_raws_by_ids(
        &self,
        _agent_id: &str,
        _ids: &[String],
    ) -> Result<Vec<RawMemory>, AlephError> {
        Ok(Vec::new())
    }

    /// Fetch all raw memory rows for a session, ordered by `created_at` ASC.
    /// Default: empty.
    async fn get_raws_by_session(
        &self,
        _agent_id: &str,
        _session_id: &str,
    ) -> Result<Vec<RawMemory>, AlephError> {
        Ok(Vec::new())
    }

    /// Get raw memories by storage source type, scoped to an agent.
    /// Used for cross-session retrieval (e.g. all `SessionCompressed` summaries).
    ///
    /// Default impl fetches all unprocessed memories and filters client-side;
    /// `SQLite` backend overrides with an efficient indexed query.
    async fn get_raw_by_source(
        &self,
        source: RawMemorySource,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        let token = source.to_persisted().0;
        let all = self
            .get_unprocessed_raw_memories(agent_id, limit * 10)
            .await?;
        Ok(all
            .into_iter()
            .filter(|r| r.source.to_persisted().0 == token)
            .take(limit)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_pre_compress() {
        let src = RawMemorySource::PreCompress;
        let (token, detail) = src.to_persisted();
        assert_eq!(token, "pre_compress");
        assert!(detail.is_none());
        let back = RawMemorySource::from_persisted(token, detail.as_deref());
        assert_eq!(back, src);
    }

    #[test]
    fn round_trip_reflection() {
        let src = RawMemorySource::Reflection;
        let (token, detail) = src.to_persisted();
        assert_eq!(token, "reflection");
        assert!(detail.is_none());
        let back = RawMemorySource::from_persisted(token, detail.as_deref());
        assert_eq!(back, src);
    }

    #[test]
    fn round_trip_delegation_with_detail() {
        let src = RawMemorySource::Delegation {
            child_agent_id: "child-42".into(),
        };
        let (token, detail) = src.to_persisted();
        assert_eq!(token, "delegation");
        let detail = detail.expect("delegation carries detail JSON");
        let back = RawMemorySource::from_persisted(token, Some(&detail));
        assert_eq!(back, src);
    }

    #[test]
    fn round_trip_session_end_task_done() {
        let src = RawMemorySource::SessionEnd {
            reason: SessionEndReason::TaskDone,
        };
        let (token, detail) = src.to_persisted();
        assert_eq!(token, "session_end");
        let detail = detail.expect("session_end carries detail JSON");
        let back = RawMemorySource::from_persisted(token, Some(&detail));
        assert_eq!(back, src);
    }

    #[test]
    fn legacy_variants_still_parse() {
        let back = RawMemorySource::from_persisted("session_compressed", None);
        assert_eq!(back, RawMemorySource::SessionCompressed);
    }

    #[test]
    fn round_trip_correction_with_rule() {
        let src = RawMemorySource::Correction {
            severity: "med".into(),
            suggested_rule: Some("never write JSDoc".into()),
        };
        let (token, detail) = src.to_persisted();
        assert_eq!(token, "correction");
        let detail = detail.expect("correction carries detail JSON");
        let back = RawMemorySource::from_persisted(token, Some(&detail));
        assert_eq!(back, src);
    }

    #[test]
    fn round_trip_correction_without_rule() {
        let src = RawMemorySource::Correction {
            severity: "high".into(),
            suggested_rule: None,
        };
        let (token, detail) = src.to_persisted();
        let back = RawMemorySource::from_persisted(token, detail.as_deref());
        assert_eq!(back, src);
    }

    #[test]
    fn round_trip_tool_invocation_success() {
        let src = RawMemorySource::ToolInvocation {
            tool_name: "read_file".into(),
            success: true,
            duration_ms: 42,
        };
        let (token, detail) = src.to_persisted();
        assert_eq!(token, "tool_invocation");
        let detail = detail.expect("tool_invocation carries detail JSON");
        let back = RawMemorySource::from_persisted(token, Some(&detail));
        assert_eq!(back, src);
    }

    #[test]
    fn round_trip_tool_invocation_failure() {
        let src = RawMemorySource::ToolInvocation {
            tool_name: "shell".into(),
            success: false,
            duration_ms: 0,
        };
        let (token, detail) = src.to_persisted();
        let back = RawMemorySource::from_persisted(token, detail.as_deref());
        assert_eq!(back, src);
    }

    #[test]
    fn tool_invocation_missing_detail_falls_back_to_zero() {
        // Defensive: if a buggy writer ever emitted "tool_invocation" with no
        // detail, parse to a zero-stat default rather than panic.
        let back = RawMemorySource::from_persisted("tool_invocation", None);
        assert_eq!(
            back,
            RawMemorySource::ToolInvocation {
                tool_name: String::new(),
                success: false,
                duration_ms: 0,
            }
        );
    }

    #[test]
    fn correction_missing_detail_falls_back_to_low() {
        // Defensive: if a buggy writer ever emitted "correction" with no detail,
        // we should still parse rather than panic — severity defaults to "low".
        let back = RawMemorySource::from_persisted("correction", None);
        assert_eq!(
            back,
            RawMemorySource::Correction {
                severity: "low".into(),
                suggested_rule: None,
            }
        );
    }
}
