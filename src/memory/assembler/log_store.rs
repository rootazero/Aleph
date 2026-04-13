//! Optional persistence writer for assembly decisions. Spec 2 consumes rows
//! from this table to correlate with citation / re-retrieval signals.

use super::envelope::MemoryEnvelope;
use crate::config::types::memory::AssemblyLogConfig;
use crate::memory::store::sqlite::assembly_logs::AssemblyLogRow;
use crate::memory::store::MemoryBackend;
use sha2::{Digest, Sha256};
use tracing::warn;
use uuid::Uuid;

pub struct AssemblyLogWriter {
    backend: MemoryBackend,
    config: AssemblyLogConfig,
}

impl AssemblyLogWriter {
    pub fn new(backend: MemoryBackend, config: AssemblyLogConfig) -> Self {
        Self { backend, config }
    }

    /// Persist a single assembly decision if logging is enabled.
    pub fn write(&self, env: &MemoryEnvelope) {
        if !self.config.enabled {
            return;
        }
        let query_hash = format!("{:x}", Sha256::digest(env.query.as_bytes()));
        let selected_ids: Vec<&str> = env
            .slots
            .iter()
            .flat_map(|s| s.items.iter().map(|i| i.id.as_str()))
            .collect();
        let selected_json = serde_json::to_string(&selected_ids).unwrap_or_else(|_| "[]".into());
        let total_tokens: u32 = env.slots.iter().map(|s| s.tokens_used).sum();

        let row = AssemblyLogRow {
            id: Uuid::new_v4().to_string(),
            agent_id: env.agent_id.clone(),
            session_id: env.session_id.clone(),
            query_hash,
            strategy: env.meta.strategy.clone(),
            used_fallback: env.meta.used_fallback,
            fallback_reason: env.meta.fallback_reason.clone(),
            candidates_count: env.meta.candidates_considered as i64,
            selected_item_ids: selected_json,
            total_tokens: total_tokens as i64,
            rerank_latency_ms: env.meta.llm_rerank_latency_ms.map(|v| v as i64),
            total_latency_ms: env.meta.total_latency_ms as i64,
            created_at: env.generated_at,
        };

        if let Err(e) = self.backend.insert_assembly_log(&row) {
            warn!(error = %e, "assembly_log insert failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::{EnvelopeMeta, SCHEMA_VERSION};
    use crate::memory::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use std::path::PathBuf;

    fn tmp_backend() -> MemoryBackend {
        let tmp = tempfile::tempdir().unwrap();
        let path: PathBuf = tmp.path().join("mem.db");
        Box::leak(Box::new(tmp));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    fn env() -> MemoryEnvelope {
        MemoryEnvelope {
            schema_version: SCHEMA_VERSION.to_string(),
            generated_at: 1_700_000_000,
            query: "q".into(),
            agent_id: "default".into(),
            session_id: None,
            slots: vec![],
            meta: EnvelopeMeta {
                strategy: "hybrid_v1".into(),
                candidates_considered: 0,
                used_fallback: false,
                fallback_reason: None,
                llm_rerank_latency_ms: None,
                total_latency_ms: 0,
            },
        }
    }

    #[test]
    fn writer_noops_when_disabled() {
        let backend = tmp_backend();
        let writer = AssemblyLogWriter::new(
            backend.clone(),
            AssemblyLogConfig {
                enabled: false,
                retention_days: 14,
            },
        );
        writer.write(&env());
        assert_eq!(backend.assembly_log_count().unwrap(), 0);
    }

    #[test]
    fn writer_persists_when_enabled() {
        let backend = tmp_backend();
        let writer = AssemblyLogWriter::new(
            backend.clone(),
            AssemblyLogConfig {
                enabled: true,
                retention_days: 14,
            },
        );
        writer.write(&env());
        assert_eq!(backend.assembly_log_count().unwrap(), 1);
    }
}
