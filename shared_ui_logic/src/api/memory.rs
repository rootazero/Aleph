//! Memory API

use crate::connection::AlephConnector;
use crate::protocol::{RpcClient, RpcError};
use serde::{Deserialize, Serialize};

/// Memory statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    /// Total number of facts
    pub count: u64,
    /// Total size in bytes
    pub size_bytes: u64,
}

/// Memory search result item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchItem {
    /// Fact ID
    pub id: String,
    /// Fact content
    pub content: String,
    /// Similarity score (0.0 - 1.0)
    pub similarity: f64,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Memory API client
///
/// Provides high-level methods for interacting with the Aleph memory system.
///
/// ## Example
///
/// ```rust,ignore
/// use aleph_ui_logic::api::MemoryApi;
/// use aleph_ui_logic::connection::create_connector;
///
/// let connector = create_connector();
/// let memory = MemoryApi::new(connector);
///
/// // Get memory statistics
/// let stats = memory.stats().await?;
/// println!("Total facts: {}", stats.count);
///
/// // Search memory
/// let results = memory.search("rust programming", Some(10)).await?;
/// for item in results {
///     println!("{}: {}", item.id, item.content);
/// }
/// ```
pub struct MemoryApi<C: AlephConnector> {
    client: RpcClient<C>,
}

impl<C: AlephConnector> MemoryApi<C> {
    /// Create a new Memory API client
    pub fn new(connector: C) -> Self {
        Self {
            client: RpcClient::new(connector),
        }
    }

    /// Get memory statistics
    ///
    /// # Returns
    ///
    /// [`MemoryStats`] containing count and size information
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the request fails
    pub async fn stats(&self) -> Result<MemoryStats, RpcError> {
        self.client.call("memory.stats", ()).await
    }

    /// Search memory for similar content
    ///
    /// # Arguments
    ///
    /// - `query`: The search query
    /// - `limit`: Optional maximum number of results (default: 10)
    ///
    /// # Returns
    ///
    /// A vector of [`MemorySearchItem`] sorted by similarity
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the request fails
    pub async fn search(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> Result<Vec<MemorySearchItem>, RpcError> {
        #[derive(Serialize)]
        struct SearchParams<'a> {
            query: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            limit: Option<u32>,
        }

        #[derive(Deserialize)]
        struct SearchResult {
            results: Vec<MemorySearchItem>,
        }

        let result: SearchResult = self
            .client
            .call("memory.search", SearchParams { query, limit })
            .await?;

        Ok(result.results)
    }

    /// Delete a memory fact by ID
    ///
    /// # Arguments
    ///
    /// - `id`: The fact ID to delete
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the request fails
    pub async fn delete(&self, id: &str) -> Result<(), RpcError> {
        #[derive(Serialize)]
        struct DeleteParams<'a> {
            id: &'a str,
        }

        self.client
            .call::<_, ()>("memory.delete", DeleteParams { id })
            .await
    }

    /// Clear all memory facts
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the request fails
    pub async fn clear(&self) -> Result<(), RpcError> {
        self.client.call::<_, ()>("memory.clear", ()).await
    }

    /// Clear only facts (keep other memory data)
    ///
    /// # Returns
    ///
    /// The number of facts deleted
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the request fails
    pub async fn clear_facts(&self) -> Result<u64, RpcError> {
        #[derive(Deserialize)]
        struct ClearResult {
            deleted: u64,
        }

        let result: ClearResult = self.client.call("memory.clearFacts", ()).await?;
        Ok(result.deleted)
    }

    /// Compress memory (trigger compression)
    ///
    /// # Returns
    ///
    /// `true` if compression was successful
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the request fails
    pub async fn compress(&self) -> Result<bool, RpcError> {
        #[derive(Deserialize)]
        struct CompressResult {
            ok: bool,
        }

        let result: CompressResult = self.client.call("memory.compress", ()).await?;
        Ok(result.ok)
    }

    /// List apps that have memory data
    ///
    /// # Returns
    ///
    /// A vector of app names
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the request fails
    pub async fn app_list(&self) -> Result<Vec<String>, RpcError> {
        #[derive(Deserialize)]
        struct AppListResult {
            apps: Vec<String>,
        }

        let result: AppListResult = self.client.call("memory.appList", ()).await?;
        Ok(result.apps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_stats_serialization() {
        let stats = MemoryStats {
            count: 100,
            size_bytes: 1024,
        };

        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: MemoryStats = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.count, 100);
        assert_eq!(deserialized.size_bytes, 1024);
    }

    #[test]
    fn test_memory_search_item_serialization() {
        let item = MemorySearchItem {
            id: "fact-123".to_string(),
            content: "Test content".to_string(),
            similarity: 0.95,
            metadata: Some(serde_json::json!({"key": "value"})),
        };

        let json = serde_json::to_string(&item).unwrap();
        let deserialized: MemorySearchItem = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "fact-123");
        assert_eq!(deserialized.similarity, 0.95);
    }
}
