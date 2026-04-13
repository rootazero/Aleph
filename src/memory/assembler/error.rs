//! Internal assembler error. Never crosses the module boundary — the public
//! API returns `Result<MemoryEnvelope, AlephError>` and maps all variants to
//! graceful fallback or degraded output.

use crate::error::AlephError;
use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)] // variants consumed by downstream tasks
pub(crate) enum AssemblerError {
    #[error("retrieval failed: {0}")]
    Retrieval(#[source] AlephError),

    #[error("llm rerank timeout after {0}ms")]
    RerankTimeout(u64),

    #[error("llm rerank returned invalid json: {0}")]
    RerankParse(String),

    #[error("llm rerank produced no valid slots")]
    RerankEmpty,

    #[error("content load failed for {id}: {source}")]
    Hydration {
        id: String,
        #[source]
        source: AlephError,
    },
}
