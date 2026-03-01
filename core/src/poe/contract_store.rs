//! Pending Contract Store
//!
//! In-memory storage for contracts awaiting user signature.
//!
//! ## Design Decisions
//!
//! - **In-memory storage**: Simple and fast, no persistence needed
//! - **No timeout**: Contracts persist until explicitly signed or rejected
//! - **Thread-safe**: Uses `RwLock` for concurrent access

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::poe::contract::PendingContract;

// ============================================================================
// PendingContractStore
// ============================================================================

/// In-memory store for pending contracts.
///
/// Contracts are stored until:
/// - Signed via `take()` (removes and returns the contract)
/// - Rejected via `remove()` (removes without returning)
/// - Cleared via `clear()` (removes all)
///
/// # Example
///
/// ```rust,ignore
/// let store = PendingContractStore::new();
///
/// // Insert a new contract
/// store.insert(contract).await;
///
/// // Sign: take the contract (removes it from store)
/// if let Some(contract) = store.take("contract-123").await {
///     // Execute the contract
/// }
///
/// // Or reject: just remove
/// store.remove("contract-456").await;
/// ```
#[derive(Debug)]
pub struct PendingContractStore {
    contracts: Arc<RwLock<HashMap<String, PendingContract>>>,
}

impl PendingContractStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            contracts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Insert a new pending contract.
    ///
    /// If a contract with the same ID already exists, it will be replaced.
    pub async fn insert(&self, contract: PendingContract) {
        let mut store = self.contracts.write().await;
        store.insert(contract.contract_id.clone(), contract);
    }

    /// Take a contract by ID (removes and returns it).
    ///
    /// This is an atomic operation used during signing.
    /// Returns `None` if the contract doesn't exist.
    pub async fn take(&self, contract_id: &str) -> Option<PendingContract> {
        let mut store = self.contracts.write().await;
        store.remove(contract_id)
    }

    /// Get a contract by ID without removing it.
    ///
    /// Useful for preview or validation before signing.
    pub async fn get(&self, contract_id: &str) -> Option<PendingContract> {
        let store = self.contracts.read().await;
        store.get(contract_id).cloned()
    }

    /// Check if a contract exists.
    pub async fn contains(&self, contract_id: &str) -> bool {
        let store = self.contracts.read().await;
        store.contains_key(contract_id)
    }

    /// Remove a contract by ID.
    ///
    /// Returns `true` if the contract was removed, `false` if it didn't exist.
    pub async fn remove(&self, contract_id: &str) -> bool {
        let mut store = self.contracts.write().await;
        store.remove(contract_id).is_some()
    }

    /// List all pending contracts.
    ///
    /// Returns cloned contracts to avoid holding the lock.
    pub async fn list(&self) -> Vec<PendingContract> {
        let store = self.contracts.read().await;
        store.values().cloned().collect()
    }

    /// Get the number of pending contracts.
    pub async fn len(&self) -> usize {
        let store = self.contracts.read().await;
        store.len()
    }

    /// Check if the store is empty.
    pub async fn is_empty(&self) -> bool {
        let store = self.contracts.read().await;
        store.is_empty()
    }

    /// Clear all pending contracts.
    ///
    /// Returns the number of contracts that were removed.
    pub async fn clear(&self) -> usize {
        let mut store = self.contracts.write().await;
        let count = store.len();
        store.clear();
        count
    }
}

impl Default for PendingContractStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for PendingContractStore {
    fn clone(&self) -> Self {
        Self {
            contracts: self.contracts.clone(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::SuccessManifest;

    fn create_test_contract(id: &str) -> PendingContract {
        let manifest = SuccessManifest::new(format!("task-{}", id), "Test objective");
        PendingContract::new(id, "Test instruction", manifest)
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let store = PendingContractStore::new();
        let contract = create_test_contract("123");

        store.insert(contract).await;

        let retrieved = store.get("123").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().contract_id, "123");
    }

    #[tokio::test]
    async fn test_take_removes_contract() {
        let store = PendingContractStore::new();
        let contract = create_test_contract("123");

        store.insert(contract).await;

        // First take should succeed
        let taken = store.take("123").await;
        assert!(taken.is_some());

        // Second take should fail (already removed)
        let taken_again = store.take("123").await;
        assert!(taken_again.is_none());
    }

    #[tokio::test]
    async fn test_remove() {
        let store = PendingContractStore::new();
        let contract = create_test_contract("123");

        store.insert(contract).await;

        assert!(store.remove("123").await);
        assert!(!store.remove("123").await); // Already removed
        assert!(!store.contains("123").await);
    }

    #[tokio::test]
    async fn test_list() {
        let store = PendingContractStore::new();

        store.insert(create_test_contract("1")).await;
        store.insert(create_test_contract("2")).await;
        store.insert(create_test_contract("3")).await;

        let list = store.list().await;
        assert_eq!(list.len(), 3);
    }

    #[tokio::test]
    async fn test_len_and_is_empty() {
        let store = PendingContractStore::new();

        assert!(store.is_empty().await);
        assert_eq!(store.len().await, 0);

        store.insert(create_test_contract("1")).await;

        assert!(!store.is_empty().await);
        assert_eq!(store.len().await, 1);
    }

    #[tokio::test]
    async fn test_clear() {
        let store = PendingContractStore::new();

        store.insert(create_test_contract("1")).await;
        store.insert(create_test_contract("2")).await;

        let cleared = store.clear().await;
        assert_eq!(cleared, 2);
        assert!(store.is_empty().await);
    }

    #[tokio::test]
    async fn test_contains() {
        let store = PendingContractStore::new();

        assert!(!store.contains("123").await);

        store.insert(create_test_contract("123")).await;

        assert!(store.contains("123").await);
    }

    #[tokio::test]
    async fn test_replace_existing() {
        let store = PendingContractStore::new();

        let contract1 = create_test_contract("123");
        store.insert(contract1).await;

        // Insert again with same ID (should replace)
        let manifest = SuccessManifest::new("task-replaced", "Replaced objective");
        let contract2 = PendingContract::new("123", "Replaced instruction", manifest);
        store.insert(contract2).await;

        let retrieved = store.get("123").await.unwrap();
        assert_eq!(retrieved.instruction, "Replaced instruction");
        assert_eq!(store.len().await, 1);
    }
}
