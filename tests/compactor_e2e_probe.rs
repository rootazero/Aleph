//! End-to-end probe tests for the session compactor.
//!
//! Spawns a real `aleph-server` server process, sends multi-turn conversations via
//! WebSocket JSON-RPC with a real LLM, and verifies that session compression
//! facts are created and retrievable through the memory API.
//!
//! All tests are `#[ignore]` — they require:
//! - A built `aleph-server` binary (`cargo build -p alephcore --bin aleph-server`)
//! - A configured LLM provider (API key in vault or environment)
//!
//! Run manually: `cargo test -p alephcore --test compactor_e2e_probe -- --ignored`

mod compactor_e2e_probe {
    pub mod compression_depth;
    pub mod harness;
    pub mod multi_turn;
    pub mod session_recall;
}
