//! Integration tests for memory namespace isolation
//!
//! Verifies that Owner and Guest namespaces are properly isolated at database layer.
//!
//! NOTE: These tests are currently ignored because namespace isolation needs to be
//! re-implemented in the LanceDB backend. The old SQLite-based VectorDatabase had
//! built-in namespace filtering; the new LanceMemoryBackend stores namespace as a
//! field but the trait-level search filters need namespace support to be wired up.

use alephcore::memory::context::{FactType, MemoryFact};
use alephcore::memory::NamespaceScope;
use alephcore::memory::store::lance::LanceMemoryBackend;
use alephcore::memory::store::MemoryStore;
use std::sync::Arc;
use tempfile::TempDir;

/// Helper: Create test database with sample facts
async fn create_test_db_with_facts() -> (Arc<LanceMemoryBackend>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db = Arc::new(
        LanceMemoryBackend::open_or_create(temp_dir.path())
            .await
            .unwrap(),
    );

    // Insert owner facts
    let owner_fact = MemoryFact::new(
        "Owner secret data".to_string(),
        FactType::Other,
        vec![],
    );
    db.insert_fact(&owner_fact).await.unwrap();

    // Insert guest facts
    let guest_fact = MemoryFact::new(
        "Guest alice data".to_string(),
        FactType::Other,
        vec![],
    );
    db.insert_fact(&guest_fact).await.unwrap();

    (db, temp_dir)
}

#[tokio::test]
#[ignore = "TODO: Namespace isolation not yet implemented in LanceDB backend"]
async fn test_guest_cannot_read_owner_facts() {
    let (_db, _temp) = create_test_db_with_facts().await;
    // TODO: Implement namespace-scoped search in LanceDB backend
}

#[tokio::test]
#[ignore = "TODO: Namespace isolation not yet implemented in LanceDB backend"]
async fn test_owner_can_read_all_namespaces() {
    let (_db, _temp) = create_test_db_with_facts().await;
    // TODO: Implement namespace-scoped search in LanceDB backend
}

#[tokio::test]
#[ignore = "TODO: Namespace isolation not yet implemented in LanceDB backend"]
async fn test_guests_cannot_see_each_other() {
    let (_db, _temp) = create_test_db_with_facts().await;
    // TODO: Implement namespace-scoped search in LanceDB backend
}
