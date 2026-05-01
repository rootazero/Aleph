//! Legacy free-format MEMORY.md → over-budget warning + add-blocked + replace-allowed.
//!
//! Spec A acceptance criterion 5.

use alephcore::memory::curated::snapshot::render_agent_block;
use alephcore::memory::curated::{CuratedConfig, CuratedMemoryStore};
use tempfile::tempdir;

#[tokio::test]
async fn legacy_over_budget_blocks_add_allows_replace() {
    let d = tempdir().unwrap();
    let mem_path = d.path().join("MEMORY.md");

    let big = format!("# Long-Term Memory\n\n{}\n", "lesson, ".repeat(300));
    std::fs::write(&mem_path, big).unwrap();
    let cfg = CuratedConfig::default();

    let store = CuratedMemoryStore::load(mem_path.clone(), cfg.memory_char_limit, "agent-legacy")
        .await
        .unwrap();

    assert!(store.is_legacy());

    let env = render_agent_block(
        &store.current_entries(),
        cfg.memory_char_limit,
        cfg.legacy_warn_threshold,
    );
    assert!(env.contains("OVER BUDGET"), "envelope was: {env}");

    let err = store.add("new fact").await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("legacy"),
        "expected legacy block reason, got: {msg}"
    );

    store
        .replace("Long-Term Memory", "User uses Linux Mint")
        .await
        .unwrap();
    assert!(!store.is_legacy());
    let entries = store.current_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0], "User uses Linux Mint");
}
