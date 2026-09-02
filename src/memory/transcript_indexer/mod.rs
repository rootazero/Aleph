//! Transcript Indexer Module
//!
//! Stores transcript chunks as `raw_memories` rows (source
//! `Transcript`, path `aleph://transcript/...`) for later compression and
//! substring recall via the `memory_search` transcripts leg. No embeddings
//! are produced here.

pub mod config;
pub mod indexer;

pub use config::TranscriptIndexerConfig;
pub use indexer::{TranscriptIndexer, TRANSCRIPT_PATH_PREFIX};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::{MemoryBackend, SqliteMemoryBackend};
    use crate::sync_primitives::Arc;
    use tempfile::tempdir;

    fn create_test_db(temp_dir: &std::path::Path) -> MemoryBackend {
        let db_path = temp_dir.join("sqlite_db");
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    fn indexer_with_config(config: TranscriptIndexerConfig) -> (tempfile::TempDir, TranscriptIndexer) {
        let temp_dir = tempdir().unwrap();
        let db = create_test_db(temp_dir.path());
        (temp_dir, TranscriptIndexer::with_config(db, config))
    }

    #[test]
    fn chunk_text_splits_long_text_and_keeps_short_text_whole() {
        let (_dir, indexer) = indexer_with_config(TranscriptIndexerConfig {
            max_tokens_per_chunk: 50,
            overlap_tokens: 10,
            enable_chunking: true,
        });

        let short_text = "This is short.";
        let chunks = indexer.chunk_text(short_text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], short_text);

        let long_text = "This is a sentence. ".repeat(40);
        let chunks = indexer.chunk_text(&long_text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            let tokens = indexer.estimate_tokens(chunk);
            assert!(
                tokens <= 50 + 20,
                "Chunk too large: {tokens} tokens (limit 50 + overlap margin)"
            );
        }
    }

    #[test]
    fn chunk_text_consecutive_chunks_share_overlap() {
        let (_dir, indexer) = indexer_with_config(TranscriptIndexerConfig {
            max_tokens_per_chunk: 50,
            overlap_tokens: 10,
            enable_chunking: true,
        });

        let text = "First sentence here padding words. Second sentence here padding words. \
                    Third sentence here padding words. Fourth sentence here padding words. \
                    Fifth sentence here padding words."
            .to_string();
        let chunks = indexer.chunk_text(&text);

        if chunks.len() > 1 {
            for i in 0..chunks.len() - 1 {
                let current_end: String = {
                    let chars: Vec<char> = chunks[i].chars().collect();
                    chars[chars.len().saturating_sub(40)..].iter().collect()
                };
                let next_start: String = chunks[i + 1].chars().take(40).collect();
                let has_overlap = current_end.chars().any(|c| next_start.contains(c));
                assert!(has_overlap, "No overlap between chunks {} and {}", i, i + 1);
            }
        }
    }

    #[test]
    fn chunk_text_disabled_returns_single_chunk() {
        let (_dir, indexer) = indexer_with_config(TranscriptIndexerConfig {
            max_tokens_per_chunk: 50,
            overlap_tokens: 10,
            enable_chunking: false,
        });

        let long_text = "word ".repeat(200);
        let chunks = indexer.chunk_text(&long_text);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn estimate_tokens_counts_ascii_chars() {
        let (_dir, indexer) = indexer_with_config(TranscriptIndexerConfig::default());

        assert_eq!(indexer.estimate_tokens("1234"), 1);
        assert_eq!(indexer.estimate_tokens("12345678"), 2);
        assert_eq!(indexer.estimate_tokens("123456789"), 3); // rounded up
    }

    #[test]
    fn estimate_tokens_counts_chars_not_bytes_for_cjk() {
        let (_dir, indexer) = indexer_with_config(TranscriptIndexerConfig::default());

        // 4 CJK chars = 12 bytes; a byte-based estimate would report 3.
        assert_eq!(indexer.estimate_tokens("你好世界"), 1);
        // 8 CJK chars = 2 tokens, matching the ASCII rate.
        assert_eq!(indexer.estimate_tokens("你好世界你好世界"), 2);
    }

    /// Text between `max_tokens_per_chunk` and the old hardcoded 800-token
    /// pre-gate used to skip chunking entirely and land as one oversized row.
    /// Assert on the effect: multiple rows reach the store.
    #[tokio::test]
    async fn index_turn_text_chunks_text_between_config_limit_and_old_800_gate() {
        use crate::memory::store::raw_memory::RawMemoryStore;

        let temp_dir = tempdir().unwrap();
        let db = create_test_db(temp_dir.path());
        // Default config: max_tokens_per_chunk = 400.
        let indexer = TranscriptIndexer::new(db.clone());

        // ~500 estimated tokens (2000 chars) — above the 400-token config
        // limit but below the removed 800-token gate.
        let text = "This is a filler sentence for the chunk gate test. ".repeat(40);
        assert!(indexer.estimate_tokens(&text) > 400);
        assert!(indexer.estimate_tokens(&text) < 800);

        let ids = indexer
            .index_turn_text("sess-gate", 0, &text, "", "owner", "main")
            .await;
        assert!(
            ids.len() > 1,
            "expected multiple chunks for {} estimated tokens, got {} row(s)",
            indexer.estimate_tokens(&text),
            ids.len()
        );

        let rows = db
            .get_raw_by_path_prefix("aleph://transcript/sess-gate/", "main", 50)
            .await
            .unwrap();
        assert_eq!(rows.len(), ids.len(), "every returned id must be a stored row");
    }
}
