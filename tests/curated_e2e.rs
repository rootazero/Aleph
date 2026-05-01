//! End-to-end: fresh agent, remember(add), verify frozen prompt + post-compression refresh.
//!
//! Mirrors Spec A acceptance criteria 1, 2, 3.

use alephcore::memory::curated::snapshot::render_agent_block;
use alephcore::memory::curated::{CuratedConfig, CuratedMemoryStore};
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn fresh_agent_remember_then_compression_refresh() {
    let d = tempdir().unwrap();
    let mem_path = d.path().join("MEMORY.md");
    let cfg = CuratedConfig::default();

    let store = Arc::new(
        CuratedMemoryStore::load(mem_path.clone(), cfg.memory_char_limit, "agent-fresh")
            .await
            .unwrap(),
    );

    let outcome = store.add("User prefers concise replies").await.unwrap();
    assert_eq!(outcome.entries.len(), 1);
    assert!(outcome.usage_pct > 0);

    let envelope = render_agent_block(
        &store.current_entries(),
        cfg.memory_char_limit,
        cfg.legacy_warn_threshold,
    );
    assert!(envelope.contains("User prefers concise replies"));
    assert!(
        envelope.contains(&format!("/{} chars", cfg.memory_char_limit)),
        "envelope was: {envelope}"
    );

    store.add("Linux Mint host with podman").await.unwrap();
    assert!(
        !envelope.contains("Linux Mint"),
        "frozen snapshot must not reflect post-capture writes"
    );

    let envelope2 = render_agent_block(
        &store.current_entries(),
        cfg.memory_char_limit,
        cfg.legacy_warn_threshold,
    );
    assert!(envelope2.contains("Linux Mint"));
    assert!(envelope2.contains("User prefers concise replies"));
}
