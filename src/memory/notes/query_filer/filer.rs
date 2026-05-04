//! QueryFiler trait + DefaultQueryFiler implementation.
//!
//! Two-tier gating pipeline: cheap heuristic gate → LLM novelty gate → write note.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::config::types::memory::QueryFilerConfig;
use crate::error::AlephError;
use crate::memory::notes::indexer::NoteIndexer;
use crate::memory::notes::note::{sanitize_title, KnowledgeNote};
use crate::memory::notes::orientation::NoteOrientation;
use crate::memory::notes::query_filer::prompts::PROMPT_QUERY_FILE_CHECK;
use crate::memory::notes::query_filer::types::{CheapGateReason, FileOutcome, QueryFiledRow};
use crate::memory::reflector::types::Synthesis;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::utils::json_extract::extract_json_robust;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Decides whether a memory_reflect synthesis is worth archiving as a query note.
#[async_trait]
pub trait QueryFiler: Send + Sync {
    async fn maybe_file(
        &self,
        agent_id: &str,
        query: &str,
        synthesis: &Synthesis,
        session_id: Option<&str>,
    ) -> Result<FileOutcome, AlephError>;
}

// ---------------------------------------------------------------------------
// LLM gate response shape
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LlmGateResponse {
    file: bool,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    proposed_title: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    links: Vec<String>,
}

// ---------------------------------------------------------------------------
// DefaultQueryFiler
// ---------------------------------------------------------------------------

/// Production query filer backed by SQLite + an LLM provider.
pub struct DefaultQueryFiler {
    pub store: Arc<SqliteMemoryBackend>,
    pub indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
    pub provider: Arc<dyn AiProvider>,
    pub orientation: Option<Arc<dyn NoteOrientation>>,
    pub memory_dir: PathBuf,
    pub config: QueryFilerConfig,
}

/// Compute SHA-256 hex digest of a normalized query string.
fn query_hash(query: &str) -> String {
    let normalized = query.trim().to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hex::encode(hasher.finalize())
}

#[async_trait]
impl QueryFiler for DefaultQueryFiler {
    async fn maybe_file(
        &self,
        agent_id: &str,
        query: &str,
        synthesis: &Synthesis,
        session_id: Option<&str>,
    ) -> Result<FileOutcome, AlephError> {
        // ── Cheap gate ──────────────────────────────────────────────────
        if synthesis.sources.len() < self.config.min_sources {
            return Ok(FileOutcome::SkippedCheapGate {
                reason: CheapGateReason::TooFewSources {
                    count: synthesis.sources.len(),
                },
            });
        }

        let answer_chars = synthesis.text.chars().count();
        if answer_chars < self.config.min_answer_chars {
            return Ok(FileOutcome::SkippedCheapGate {
                reason: CheapGateReason::AnswerTooShort {
                    chars: answer_chars,
                },
            });
        }

        let hash = query_hash(query);

        // Dedup check via query_filed table
        if let Some(note_path) = self.store.query_filed_lookup(agent_id, &hash)? {
            return Ok(FileOutcome::AlreadyFiled { note_path });
        }

        // ── LLM gate ───────────────────────────────────────────────────
        if self.config.llm_gate_enabled {
            let sources_list: String = synthesis
                .sources
                .iter()
                .map(|s| format!("- {} (relevance: {:.2})", s.path, s.relevance))
                .collect::<Vec<_>>()
                .join("\n");

            let user_prompt = format!(
                "Query: {query}\n\nAnswer:\n{}\n\nSources:\n{sources_list}",
                synthesis.text
            );
            let msgs = [UnifiedMessage::user(&user_prompt)];
            let resp = self
                .provider
                .process(RequestPayload::new(&msgs).with_system(Some(PROMPT_QUERY_FILE_CHECK)))
                .await
                .map_err(|e| AlephError::other(format!("query filer LLM gate: {e}")))?;

            let text = resp.text_content();
            let json = extract_json_robust(&text).ok_or_else(|| {
                AlephError::other("query filer: no JSON in LLM gate response".to_string())
            })?;
            let gate: LlmGateResponse = serde_json::from_value(json).map_err(|e| {
                AlephError::other(format!("query filer: parse LLM gate response: {e}"))
            })?;

            if !gate.file {
                return Ok(FileOutcome::SkippedLlmGate {
                    reason: gate.reason,
                });
            }

            // ── Write note ──────────────────────────────────────────────
            return self
                .write_and_record(agent_id, query, synthesis, session_id, &hash, &gate)
                .await;
        }

        // LLM gate disabled — file with defaults
        let default_gate = LlmGateResponse {
            file: true,
            reason: "llm_gate_disabled".into(),
            proposed_title: String::new(),
            tags: vec![],
            links: vec![],
        };
        self.write_and_record(agent_id, query, synthesis, session_id, &hash, &default_gate)
            .await
    }
}

impl DefaultQueryFiler {
    /// Write the note to disk, insert dedup row, and optionally log to orientation.
    async fn write_and_record(
        &self,
        agent_id: &str,
        query: &str,
        synthesis: &Synthesis,
        session_id: Option<&str>,
        hash: &str,
        gate: &LlmGateResponse,
    ) -> Result<FileOutcome, AlephError> {
        let now = chrono::Utc::now().timestamp();

        // Determine filename
        let raw_title = if gate.proposed_title.is_empty() {
            hash[..12].to_string()
        } else {
            gate.proposed_title.clone()
        };
        let filename = sanitize_title(&raw_title)?;

        // Build source links from synthesis
        let mut all_links: Vec<String> = synthesis.sources.iter().map(|s| s.path.clone()).collect();
        for link in &gate.links {
            if !all_links.contains(link) {
                all_links.push(link.clone());
            }
        }

        // Truncate query for the summary fact
        let query_summary: String = query.chars().take(120).collect();

        let note = KnowledgeNote {
            title: filename.clone(),
            category: "query".to_string(),
            tags: gate.tags.clone(),
            facts: vec![
                format!("## Question\n{query_summary}"),
                format!("## Answer\n{}", synthesis.text),
            ],
            links: all_links,
            created_at: now,
            updated_at: now,
            content_hash: String::new(),
            confidence: 1.0,
            severity: crate::memory::notes::Severity::Low,
            source_notes: Vec::new(),
            ..Default::default()
        };

        self.indexer.write_note(agent_id, "query", &note).await?;

        let note_path = format!("query/{filename}");

        // Record dedup row
        let row = QueryFiledRow {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            query_hash: hash.to_string(),
            note_path: note_path.clone(),
            session_id: session_id.map(|s| s.to_string()),
            filed_at: now,
        };
        self.store.query_filed_insert(&row)?;

        // Log to orientation if available
        if let Some(orient) = &self.orientation {
            use crate::memory::notes::orientation::types::{LogAction, LogEntry};
            let _ = orient
                .record_query(
                    agent_id,
                    LogEntry {
                        timestamp_utc: now,
                        action: LogAction::Query,
                        summary: format!("filed query/{filename}"),
                        detail_lines: vec![],
                    },
                )
                .await;
        }

        Ok(FileOutcome::Filed {
            note_path,
            created_at: now,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::reflector::types::NoteRef;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::providers::recording_mock::RecordingMockProvider;
    use tempfile::TempDir;

    fn make_synthesis(text: &str, source_count: usize) -> Synthesis {
        let sources = (0..source_count)
            .map(|i| NoteRef {
                path: format!("learning/source-{i}"),
                title: format!("Source {i}"),
                relevance: 0.9 - (i as f32 * 0.1),
            })
            .collect();
        Synthesis {
            text: text.to_string(),
            sources,
        }
    }

    fn setup() -> (
        TempDir,
        Arc<SqliteMemoryBackend>,
        Arc<NoteIndexer<SqliteMemoryBackend>>,
    ) {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(SqliteMemoryBackend::new(&dir.path().join("test.db")).unwrap());
        let indexer = Arc::new(NoteIndexer::new(dir.path().join("memory"), db.clone()));
        (dir, db, indexer)
    }

    #[tokio::test]
    async fn cheap_gate_rejects_few_sources() {
        let (_dir, db, indexer) = setup();
        let provider = Arc::new(RecordingMockProvider::new("unused".into()));
        let filer = DefaultQueryFiler {
            store: db,
            indexer,
            provider,
            orientation: None,
            memory_dir: _dir.path().join("memory"),
            config: QueryFilerConfig::default(),
        };

        // Only 1 source, default min_sources = 3
        let synthesis = make_synthesis(&"a]".repeat(200), 1);
        let result = filer
            .maybe_file("agent1", "test query", &synthesis, None)
            .await
            .unwrap();

        match result {
            FileOutcome::SkippedCheapGate {
                reason: CheapGateReason::TooFewSources { count },
            } => assert_eq!(count, 1),
            other => panic!("expected SkippedCheapGate(TooFewSources), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cheap_gate_rejects_short_answer() {
        let (_dir, db, indexer) = setup();
        let provider = Arc::new(RecordingMockProvider::new("unused".into()));
        let filer = DefaultQueryFiler {
            store: db,
            indexer,
            provider,
            orientation: None,
            memory_dir: _dir.path().join("memory"),
            config: QueryFilerConfig::default(),
        };

        // 3 sources but answer is too short (< 200 chars default)
        let synthesis = make_synthesis("short", 3);
        let result = filer
            .maybe_file("agent1", "test query", &synthesis, None)
            .await
            .unwrap();

        match result {
            FileOutcome::SkippedCheapGate {
                reason: CheapGateReason::AnswerTooShort { chars },
            } => assert_eq!(chars, 5),
            other => panic!("expected SkippedCheapGate(AnswerTooShort), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn llm_gate_rejects_when_not_novel() {
        let (_dir, db, indexer) = setup();
        let llm_response = r#"{"file": false, "reason": "merely restates existing notes"}"#;
        let provider = Arc::new(RecordingMockProvider::new(llm_response.into()));
        let filer = DefaultQueryFiler {
            store: db,
            indexer,
            provider,
            orientation: None,
            memory_dir: _dir.path().join("memory"),
            config: QueryFilerConfig::default(),
        };

        let synthesis = make_synthesis(&"long enough answer ".repeat(20), 3);
        let result = filer
            .maybe_file("agent1", "test query", &synthesis, None)
            .await
            .unwrap();

        match result {
            FileOutcome::SkippedLlmGate { reason } => {
                assert!(reason.contains("restates"), "reason: {reason}");
            }
            other => panic!("expected SkippedLlmGate, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn files_when_both_gates_pass() {
        let (_dir, db, indexer) = setup();
        let llm_response = r#"{
            "file": true,
            "reason": "novel synthesis",
            "proposed_title": "test-query-result",
            "tags": ["rust", "async"],
            "links": ["reference/tokio"]
        }"#;
        let provider = Arc::new(RecordingMockProvider::new(llm_response.into()));
        let filer = DefaultQueryFiler {
            store: db.clone(),
            indexer,
            provider,
            orientation: None,
            memory_dir: _dir.path().join("memory"),
            config: QueryFilerConfig::default(),
        };

        let synthesis = make_synthesis(&"detailed answer about async rust ".repeat(15), 4);
        let result = filer
            .maybe_file("agent1", "how does tokio work?", &synthesis, Some("sess-1"))
            .await
            .unwrap();

        match &result {
            FileOutcome::Filed {
                note_path,
                created_at,
            } => {
                assert_eq!(note_path, "query/test-query-result");
                assert!(*created_at > 0);

                // Verify file exists on disk
                let file_path = _dir.path().join("memory/agent1/query/test-query-result.md");
                assert!(file_path.exists(), "note file should exist on disk");

                // Verify dedup row was inserted
                let hash = super::query_hash("how does tokio work?");
                let lookup = db.query_filed_lookup("agent1", &hash).unwrap();
                assert_eq!(lookup, Some("query/test-query-result".to_string()));
            }
            other => panic!("expected Filed, got {other:?}"),
        }

        // Filing the same query again should return AlreadyFiled
        let llm_response2 = r#"{"file": true, "reason": "novel", "proposed_title": "x"}"#;
        let provider2 = Arc::new(RecordingMockProvider::new(llm_response2.into()));
        let filer2 = DefaultQueryFiler {
            store: db,
            indexer: filer.indexer.clone(),
            provider: provider2,
            orientation: None,
            memory_dir: _dir.path().join("memory"),
            config: QueryFilerConfig::default(),
        };
        let result2 = filer2
            .maybe_file("agent1", "how does tokio work?", &synthesis, None)
            .await
            .unwrap();
        match result2 {
            FileOutcome::AlreadyFiled { note_path } => {
                assert_eq!(note_path, "query/test-query-result");
            }
            other => panic!("expected AlreadyFiled, got {other:?}"),
        }
    }
}
