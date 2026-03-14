//! Session isolation probe integration tests.
//!
//! Tests the session lifecycle, epoch mechanics, topic generation,
//! RPC handlers, memory filtering, and multi-agent scenarios.

mod session_probe {
    pub mod mock_llm;
    pub mod harness;
    pub mod lifecycle;
    pub mod epoch_mechanics;
    pub mod topic_generation;
    pub mod rpc_handler;
    pub mod memory_filter;
    pub mod multi_agent;
    pub mod edge_cases;
}
