use crate::memory::extensions::types::CaptureCtx;
use crate::memory::extensions::{insert_with_capture_filter, MemoryExtensionRegistry};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::MemoryBackend;
use crate::sync_primitives::Arc;

use super::config::TranscriptIndexerConfig;

/// Path prefix of every row this indexer writes. Readers (the
/// `memory_search` transcripts leg) must query by this same constant.
pub const TRANSCRIPT_PATH_PREFIX: &str = "aleph://transcript/";

/// Stores per-turn transcript chunks as plain `raw_memories` rows — no
/// vectors are produced here; recall over these rows is substring-based
/// (`memory_search` transcripts leg) and later compression consumes them.
pub struct TranscriptIndexer {
    database: MemoryBackend,
    config: TranscriptIndexerConfig,
    /// Optional capture-filter registry (Spec 4 Task 6).
    /// When set, transcript raw-memory rows go through `insert_with_capture_filter`.
    /// Task 11 wires the real registry at startup; `None` falls back to direct insert.
    capture_registry: Option<Arc<MemoryExtensionRegistry>>,
}

impl TranscriptIndexer {
    /// Create new indexer with default config
    pub fn new(database: MemoryBackend) -> Self {
        Self {
            database,
            config: TranscriptIndexerConfig::default(),
            capture_registry: None,
        }
    }

    /// Create with custom config
    pub fn with_config(database: MemoryBackend, config: TranscriptIndexerConfig) -> Self {
        Self {
            database,
            config,
            capture_registry: None,
        }
    }

    /// Attach a capture-filter registry (Spec 4 Task 6).
    ///
    /// When set, transcript raw-memory rows go through `insert_with_capture_filter`
    /// so extensions can mutate or block them. Task 11 wires the real registry
    /// at startup; `None` preserves current behaviour.
    pub fn with_capture_registry(mut self, registry: Arc<MemoryExtensionRegistry>) -> Self {
        self.capture_registry = Some(registry);
        self
    }

    /// Index a single conversation turn's text into raw memory rows
    ///
    /// Combines user input and AI output, chunks if necessary, and stores
    /// each chunk as a `RawMemorySource::Transcript` row under
    /// `aleph://transcript/{session}/{seq}`. Returns the IDs of all
    /// successfully created rows. Never fails — partial failures are
    /// logged and skipped.
    pub async fn index_turn_text(
        &self,
        session_key: &str,
        seq: u32,
        user_input: &str,
        ai_output: &str,
        _namespace: &str,
        agent: &str,
    ) -> Vec<String> {
        // Build combined text
        let combined = if ai_output.trim().is_empty() {
            user_input.to_string()
        } else {
            format!("[user]: {user_input}\n\n[assistant]: {ai_output}")
        };

        if combined.trim().is_empty() {
            return Vec::new();
        }

        // `chunk_text` already short-circuits below `config.max_tokens_per_chunk`
        // (and when chunking is disabled) — no second threshold here, or the
        // two numbers drift and the gap between them bypasses chunking.
        let chunks = self.chunk_text(&combined);

        let multi_chunk = chunks.len() > 1;
        let mut created_ids = Vec::with_capacity(chunks.len());

        for (i, chunk) in chunks.iter().enumerate() {
            // Build path
            let path = if multi_chunk {
                format!("{TRANSCRIPT_PATH_PREFIX}{session_key}/{seq}_chunk{i}")
            } else {
                format!("{TRANSCRIPT_PATH_PREFIX}{session_key}/{seq}")
            };

            // Insert to raw_memories
            {
                use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
                // rust-doctor-disable-next-line excessive-clone
                let raw = RawMemory::new(chunk.clone(), RawMemorySource::Transcript)
                    .with_agent(agent)
                    .with_session(session_key)
                    // rust-doctor-disable-next-line excessive-clone
                    .with_path(path.clone());
                // rust-doctor-disable-next-line excessive-clone
                let raw_id = raw.id.clone();
                let insert_result = if let Some(ref registry) = self.capture_registry {
                    // rust-doctor-disable-next-line excessive-clone
                    let store: Arc<dyn RawMemoryStore> = self.database.clone();
                    let ctx = CaptureCtx {
                        agent_id: agent.to_string(),
                        namespace: NamespaceScope::Owner,
                        session_id: Some(session_key.to_string()),
                        source_hint: "transcript".into(),
                    };
                    insert_with_capture_filter(&store, registry, &ctx, raw)
                        .await
                        .map(|_| ())
                } else {
                    self.database.insert_raw_memory(&raw).await
                };
                if let Err(e) = insert_result {
                    tracing::warn!(
                        session_key,
                        seq,
                        chunk_idx = i,
                        error = %e,
                        "transcript indexer: insert_raw_memory failed, skipping chunk"
                    );
                    continue;
                }
                created_ids.push(raw_id);
            }
        }

        created_ids
    }

    /// Chunk text into overlapping segments
    #[must_use]
    pub fn chunk_text(&self, text: &str) -> Vec<String> {
        if !self.config.enable_chunking {
            return vec![text.to_string()];
        }

        let tokens = self.estimate_tokens(text);
        if tokens <= self.config.max_tokens_per_chunk {
            return vec![text.to_string()];
        }

        // Split by sentences
        let sentences: Vec<&str> = text.split('.').filter(|s| !s.trim().is_empty()).collect();
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        let mut current_tokens = 0;

        for sentence in sentences {
            let sentence_tokens = self.estimate_tokens(sentence);

            if current_tokens + sentence_tokens > self.config.max_tokens_per_chunk
                && !current_chunk.is_empty()
            {
                // rust-doctor-disable-next-line excessive-clone
                chunks.push(current_chunk.clone());

                // Add overlap from previous chunk
                let overlap_text = self.get_overlap_text(&current_chunk);
                current_chunk = overlap_text;
                current_tokens = self.estimate_tokens(&current_chunk);
            }

            if !current_chunk.is_empty() && !current_chunk.ends_with(' ') {
                current_chunk.push(' ');
            }
            current_chunk.push_str(sentence.trim());
            current_chunk.push('.');
            current_tokens += sentence_tokens;
        }

        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        if chunks.is_empty() {
            chunks.push(text.to_string());
        }

        chunks
    }

    /// Estimate token count for text.
    ///
    /// Counts chars, not bytes — the same unit as
    /// `assembler::hydration::estimate_tokens`. A byte count triples the
    /// estimate for CJK text and over-chunks it.
    #[must_use]
    pub fn estimate_tokens(&self, text: &str) -> usize {
        text.chars().count().div_ceil(4) // ~4 chars per token, round up
    }

    /// Get overlap text from end of chunk (UTF-8 safe)
    fn get_overlap_text(&self, text: &str) -> String {
        let overlap_chars = self.config.overlap_tokens * 4;
        let char_count = text.chars().count();
        if char_count <= overlap_chars {
            return text.to_string();
        }
        let skip = char_count - overlap_chars;
        text.char_indices().nth(skip).map_or_else(
            || text.to_string(),
            |(byte_pos, _)| text[byte_pos..].to_string(),
        )
    }
}
