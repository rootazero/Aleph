//! Session compactor probe integration tests.
//!
//! Tests the SessionCompactor's compression pipeline: tool compaction,
//! summary generation, depth upgrades, history assembly, session search,
//! fallback chains, and prompt layer integration.

mod session_compactor_probe {
    pub mod mock_llm;
    pub mod harness;
    pub mod tool_compaction;
    pub mod summary_gen;
    pub mod depth_upgrade;
    pub mod history_assembly;
    pub mod session_search;
    pub mod fallback_chain;
    pub mod prompt_layer;
}
