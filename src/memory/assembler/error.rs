//! Internal assembler error. Never crosses the module boundary — the public
//! API returns `Result<MemoryEnvelope, AlephError>` and maps all variants to
//! graceful fallback or degraded output.

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum AssemblerError {
    #[error("llm rerank returned invalid json: {0}")]
    RerankParse(String),

    #[error("llm rerank produced no valid slots")]
    RerankEmpty,
}
