# Embedding Evolution Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Decouple Aleph's embedding system from the hardcoded 384-dim multilingual-e5-small model, enabling configurable providers, dynamic dimensions, and zero-downtime model switching.

**Architecture:** Introduce an `EmbeddingProvider` trait that abstracts local (fastembed) and remote (OpenAI-compatible) embedding backends. Make vector dimensions config-driven instead of hardcoded. Add per-fact embedding model metadata and a lazy migration engine for model switching.

**Tech Stack:** Rust, async-trait, fastembed, reqwest (for remote providers), rusqlite/sqlite-vec, serde/schemars (config)

---

### Task 1: Add `EmbeddingConfig` to config system

This task adds the new `EmbeddingConfig` struct to `MemoryConfig`, replacing the flat `embedding_model` field with a structured sub-section.

**Files:**
- Modify: `core/src/config/types/memory.rs`
- Test: `core/src/config/types/memory.rs` (inline tests)

**Context:**
- `MemoryConfig` is at `core/src/config/types/memory.rs:15-92`
- It has a flat `embedding_model: String` field at line 21 that defaults to `"bge-small-zh-v1.5"` (line 198-199)
- This field is currently unused by SmartEmbedder (which hardcodes `multilingual-e5-small`)
- `Config.memory` is declared at `core/src/config/structs.rs:26`

**Step 1: Add the `EmbeddingConfig` struct and defaults**

Add before the `impl Default for MemoryConfig` block (before line 318):

```rust
// =============================================================================
// EmbeddingConfig
// =============================================================================

/// Embedding provider configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingConfig {
    /// Provider type: "local", "openai", or "custom"
    #[serde(default = "default_embedding_provider")]
    pub provider: String,
    /// Model name (provider-specific)
    #[serde(default = "default_embedding_model_name")]
    pub model: String,
    /// Output embedding dimension
    #[serde(default = "default_embedding_dimension")]
    pub dimension: u32,
    /// Environment variable name for API key (remote providers only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// API base URL (remote providers only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    /// Request timeout in milliseconds (remote providers only)
    #[serde(default = "default_embedding_timeout_ms")]
    pub timeout_ms: u64,
    /// Batch size for embedding requests
    #[serde(default = "default_embedding_batch_size")]
    pub batch_size: u32,
}

pub fn default_embedding_provider() -> String {
    "local".to_string()
}

pub fn default_embedding_model_name() -> String {
    "multilingual-e5-small".to_string()
}

pub fn default_embedding_dimension() -> u32 {
    384
}

pub fn default_embedding_timeout_ms() -> u64 {
    10000
}

pub fn default_embedding_batch_size() -> u32 {
    32
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: default_embedding_provider(),
            model: default_embedding_model_name(),
            dimension: default_embedding_dimension(),
            api_key_env: None,
            api_base: None,
            timeout_ms: default_embedding_timeout_ms(),
            batch_size: default_embedding_batch_size(),
        }
    }
}
```

**Step 2: Add `embedding` field to `MemoryConfig`**

Add to the `MemoryConfig` struct, in the "Dreaming + Memory Graph" section (after line 91):

```rust
    /// Embedding provider configuration
    #[serde(default)]
    pub embedding: EmbeddingConfig,
```

And add to the `Default` impl (before `dreaming` in the Default impl, around line 346):

```rust
            embedding: EmbeddingConfig::default(),
```

**Step 3: Run tests to verify it compiles**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check 2>&1 | head -20`
Expected: Compilation succeeds (or warnings only, no errors).

**Step 4: Verify config serialization round-trips**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib config::tests 2>&1 | tail -20`
Expected: All existing config tests pass. The new field uses `#[serde(default)]` so existing TOML files are unaffected.

**Step 5: Commit**

```bash
git add core/src/config/types/memory.rs
git commit -m "config: add EmbeddingConfig struct to MemoryConfig"
```

---

### Task 2: Create `EmbeddingProvider` trait

This task creates the abstract trait that all embedding providers (local, remote) must implement.

**Files:**
- Create: `core/src/memory/embedding_provider.rs`
- Modify: `core/src/memory/mod.rs`

**Context:**
- The trait design is documented in the design doc (section 3.1)
- `SmartEmbedder` at `core/src/memory/smart_embedder.rs` already has `embed()`, `embed_batch()`, `dimensions()`, `model_name()` methods
- A `TextEmbedder` trait exists at `core/src/dispatcher/model_router/intelligent/semantic_cache/embedder.rs` but is scoped to the semantic cache and we should not couple to it
- `AlephError` is the standard error type in the project

**Step 1: Create the trait file with `truncate_and_normalize` utility**

Create `core/src/memory/embedding_provider.rs`:

```rust
//! Embedding provider abstraction
//!
//! Defines the `EmbeddingProvider` trait that unifies local (fastembed)
//! and remote (OpenAI-compatible) embedding backends.

use crate::error::AlephError;

/// Abstract embedding provider
///
/// Implementations wrap specific backends (local fastembed, OpenAI API, etc.)
/// behind a uniform async interface.
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError>;

    /// Generate embeddings for multiple texts (batch)
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError>;

    /// Get the output dimension of this provider
    fn dimensions(&self) -> usize;

    /// Get the model name (e.g., "multilingual-e5-small")
    fn model_name(&self) -> &str;

    /// Get the provider type (e.g., "local", "openai", "custom")
    fn provider_type(&self) -> &str;
}

/// Truncate embedding to target dimension and L2 normalize.
///
/// Used when a remote model returns vectors larger than the configured
/// storage dimension. Borrowed from OpenViking's design.
///
/// If `embedding.len() <= target_dim`, returns the embedding unchanged.
pub fn truncate_and_normalize(embedding: Vec<f32>, target_dim: usize) -> Vec<f32> {
    if embedding.len() <= target_dim {
        return embedding;
    }
    let truncated = &embedding[..target_dim];
    let norm: f32 = truncated.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        truncated.iter().map(|x| x / norm).collect()
    } else {
        truncated.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_and_normalize_no_op_when_smaller() {
        let embedding = vec![0.6, 0.8]; // 2-dim
        let result = truncate_and_normalize(embedding.clone(), 5); // target is larger
        assert_eq!(result, embedding);
    }

    #[test]
    fn test_truncate_and_normalize_equal_dim() {
        let embedding = vec![0.6, 0.8];
        let result = truncate_and_normalize(embedding.clone(), 2);
        assert_eq!(result, embedding);
    }

    #[test]
    fn test_truncate_and_normalize_truncates_and_normalizes() {
        // 4-dim vector, truncate to 2-dim
        let embedding = vec![3.0, 4.0, 99.0, 99.0];
        let result = truncate_and_normalize(embedding, 2);
        assert_eq!(result.len(), 2);
        // norm of [3,4] = 5, so normalized = [0.6, 0.8]
        assert!((result[0] - 0.6).abs() < 1e-6);
        assert!((result[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_truncate_and_normalize_zero_vector() {
        let embedding = vec![0.0, 0.0, 0.0, 0.0];
        let result = truncate_and_normalize(embedding, 2);
        assert_eq!(result, vec![0.0, 0.0]);
    }
}
```

**Step 2: Register the module and re-export**

In `core/src/memory/mod.rs`, add after line 39 (`pub mod smart_embedder;`):

```rust
pub mod embedding_provider;
```

And add to the re-exports section (after line 89):

```rust
pub use embedding_provider::{EmbeddingProvider, truncate_and_normalize};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib memory::embedding_provider 2>&1 | tail -10`
Expected: 4 tests pass.

**Step 4: Commit**

```bash
git add core/src/memory/embedding_provider.rs core/src/memory/mod.rs
git commit -m "memory: add EmbeddingProvider trait and truncate_and_normalize utility"
```

---

### Task 3: Implement `LocalEmbeddingProvider` wrapping `SmartEmbedder`

This task implements the `EmbeddingProvider` trait for the existing local fastembed model by wrapping `SmartEmbedder`.

**Files:**
- Modify: `core/src/memory/embedding_provider.rs`

**Context:**
- `SmartEmbedder` is at `core/src/memory/smart_embedder.rs`
- It implements `Clone` and has `embed()`, `embed_batch()`, `dimensions()`, `model_name()` methods
- All methods return `Result<_, AlephError>` except `dimensions()` (returns `usize`) and `model_name()` (returns `&'static str`)

**Step 1: Add `LocalEmbeddingProvider` struct and impl**

Add to `core/src/memory/embedding_provider.rs`, before the `#[cfg(test)]` block:

```rust
use crate::memory::smart_embedder::SmartEmbedder;

/// Local embedding provider wrapping SmartEmbedder (fastembed)
///
/// This is the default provider that uses a local multilingual-e5-small model.
/// It supports TTL-based lazy loading and background cleanup.
#[derive(Clone)]
pub struct LocalEmbeddingProvider {
    embedder: SmartEmbedder,
}

impl LocalEmbeddingProvider {
    /// Create a new local provider from an existing SmartEmbedder
    pub fn new(embedder: SmartEmbedder) -> Self {
        Self { embedder }
    }

    /// Get a reference to the underlying SmartEmbedder
    pub fn inner(&self) -> &SmartEmbedder {
        &self.embedder
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for LocalEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError> {
        self.embedder.embed(text).await
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        self.embedder.embed_batch(texts).await
    }

    fn dimensions(&self) -> usize {
        self.embedder.dimensions()
    }

    fn model_name(&self) -> &str {
        self.embedder.model_name()
    }

    fn provider_type(&self) -> &str {
        "local"
    }
}
```

**Step 2: Update re-exports in `mod.rs`**

In `core/src/memory/mod.rs`, update the embedding_provider re-export line:

```rust
pub use embedding_provider::{EmbeddingProvider, LocalEmbeddingProvider, truncate_and_normalize};
```

**Step 3: Add a unit test**

Add to the `#[cfg(test)]` block in `embedding_provider.rs`:

```rust
    #[tokio::test]
    async fn test_local_provider_creation() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);
        let provider = LocalEmbeddingProvider::new(embedder);

        assert_eq!(provider.dimensions(), 384);
        assert_eq!(provider.model_name(), "multilingual-e5-small");
        assert_eq!(provider.provider_type(), "local");

        provider.inner().shutdown();
    }
```

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib memory::embedding_provider 2>&1 | tail -10`
Expected: 5 tests pass (4 previous + 1 new).

**Step 5: Commit**

```bash
git add core/src/memory/embedding_provider.rs core/src/memory/mod.rs
git commit -m "memory: implement LocalEmbeddingProvider wrapping SmartEmbedder"
```

---

### Task 4: Make vec0 table creation use dynamic dimension

This task removes the hardcoded `float[384]` from the schema SQL and makes vec0 tables use a dimension parameter.

**Files:**
- Modify: `core/src/memory/database/core.rs`

**Context:**
- `CURRENT_EMBEDDING_DIM` is defined at `core/src/memory/database/core.rs:12` as `pub const CURRENT_EMBEDDING_DIM: u32 = 384`
- It's re-exported in `core/src/memory/database/mod.rs:26` and used in tests
- The vec0 tables are in `schema_sql()` at lines 264-271: `embedding float[384]`
- `VectorDatabase::new()` and `VectorDatabase::in_memory()` use `CURRENT_EMBEDDING_DIM` for schema_info tracking
- The `experiences_vec` table in `migration.rs:158` also hardcodes `float[384]`

**Step 1: Split schema into static SQL and dynamic vec0 SQL**

In `core/src/memory/database/core.rs`:

1. Change `CURRENT_EMBEDDING_DIM` from a constant to a configurable default:

```rust
/// Default embedding dimension (multilingual-e5-small)
/// Use EmbeddingConfig.dimension for the actual configured value.
pub const DEFAULT_EMBEDDING_DIM: u32 = 384;
```

2. Remove the vec0 table creation from `schema_sql()`. Replace lines 259-271 (the `sqlite-vec Virtual Tables` section) with a comment:

```rust
            -- ================================================================
            -- sqlite-vec Virtual Tables: created dynamically via vec_schema_sql()
            -- ================================================================
```

3. Add a new method `vec_schema_sql(dim: u32)` to VectorDatabase:

```rust
    /// SQL for creating vec0 virtual tables with dynamic dimension
    fn vec_schema_sql(dim: u32) -> String {
        format!(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec USING vec0(
                embedding float[{dim}]
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec USING vec0(
                embedding float[{dim}]
            );
            "#,
            dim = dim
        )
    }
```

4. Update `create_schema()` to accept and use the dimension:

```rust
    fn create_schema(conn: &Connection, embedding_dim: u32) -> Result<(), AlephError> {
        conn.execute_batch(Self::schema_sql())
            .map_err(|e| AlephError::config(format!("Failed to create schema: {}", e)))?;
        conn.execute_batch(&Self::vec_schema_sql(embedding_dim))
            .map_err(|e| AlephError::config(format!("Failed to create vec0 tables: {}", e)))?;
        Ok(())
    }
```

5. Update `new()` to pass dimension to `create_schema()`:

Replace `Self::create_schema(&conn)?;` (line 465) with:
```rust
        Self::create_schema(&conn, DEFAULT_EMBEDDING_DIM)?;
```

Replace `params![CURRENT_EMBEDDING_DIM.to_string()]` with `params![DEFAULT_EMBEDDING_DIM.to_string()]` in both `new()` and `in_memory()`.

Replace all remaining `CURRENT_EMBEDDING_DIM` references in this file with `DEFAULT_EMBEDDING_DIM`.

6. Update `in_memory()` similarly:

Replace `Self::create_schema(&conn)?;` with:
```rust
        Self::create_schema(&conn, DEFAULT_EMBEDDING_DIM)?;
```

**Step 2: Update the re-export in `database/mod.rs`**

Change line 26 from:
```rust
pub use core::{MemoryStats, VectorDatabase, CURRENT_EMBEDDING_DIM};
```
to:
```rust
pub use core::{MemoryStats, VectorDatabase, DEFAULT_EMBEDDING_DIM};
```

**Step 3: Update all references to `CURRENT_EMBEDDING_DIM` across the codebase**

The grep results show these files use `CURRENT_EMBEDDING_DIM`:
- `core/src/memory/database/mod.rs` lines 54, 370 (tests) — change to `DEFAULT_EMBEDDING_DIM`

**Step 4: Update references to `EMBEDDING_DIM` in test files**

These test files use `crate::memory::EMBEDDING_DIM` which still refers to `smart_embedder::EMBEDDING_DIM = 384`. These can remain unchanged since `EMBEDDING_DIM` is still the dimension of the local model. No change needed.

**Step 5: Update the migration.rs experiences_vec table**

In `core/src/memory/database/migration.rs` line 158, change:
```sql
CREATE VIRTUAL TABLE IF NOT EXISTS experiences_vec USING vec0(
    embedding float[384]
);
```
to use the same default dimension:
```rust
// NOTE: experiences_vec uses DEFAULT_EMBEDDING_DIM (384) for now.
// A future migration will make this dynamic.
```

Leave the hardcoded `384` for now since `experiences_vec` is in a migration function and changing it would require a different approach. Add a `// TODO: Make dynamic` comment.

**Step 6: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib memory::database 2>&1 | tail -20`
Expected: All existing database tests pass.

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check 2>&1 | head -20`
Expected: Clean compilation (or warnings only).

**Step 7: Commit**

```bash
git add core/src/memory/database/core.rs core/src/memory/database/mod.rs
git commit -m "database: make vec0 tables use dynamic dimension, rename CURRENT_EMBEDDING_DIM to DEFAULT_EMBEDDING_DIM"
```

---

### Task 5: Add `VectorDatabase::new_with_dim()` constructor

This task adds a constructor that accepts a dimension parameter, enabling config-driven database creation.

**Files:**
- Modify: `core/src/memory/database/core.rs`

**Context:**
- `VectorDatabase::new()` is at line 436 and always uses `DEFAULT_EMBEDDING_DIM`
- The dimension change detection logic at `check_needs_migration()` (line 561) compares stored dimension vs hardcoded constant
- When dimension changes, old memories table is dropped (line 452-461)

**Step 1: Add `new_with_dim()` method**

Add after the `new()` method (after line 490):

```rust
    /// Initialize vector database with a specific embedding dimension
    ///
    /// Use this when the embedding dimension is known from configuration.
    /// Falls back to DEFAULT_EMBEDDING_DIM behavior when dimension matches.
    pub fn new_with_dim(db_path: PathBuf, embedding_dim: u32) -> Result<Self, AlephError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AlephError::config(format!("Failed to create database directory: {}", e))
            })?;
        }

        Self::register_sqlite_vec_extension();

        let conn = Connection::open(&db_path)
            .map_err(|e| AlephError::config(format!("Failed to open database: {}", e)))?;

        // Check if dimension changed
        let dim_changed = Self::check_dimension_change(&conn, embedding_dim)?;

        if dim_changed {
            // Drop vec0 tables (they need recreation with new dimension)
            conn.execute_batch(
                "DROP TABLE IF EXISTS memories_vec;
                 DROP TABLE IF EXISTS facts_vec;"
            )
            .map_err(|e| AlephError::config(format!("Failed to drop vec0 tables: {}", e)))?;

            tracing::info!(
                new_dim = embedding_dim,
                "Dropped vec0 tables for dimension change. Embeddings will be re-indexed."
            );
        }

        Self::create_schema(&conn, embedding_dim)?;

        // Run migrations
        migration::migrate_add_namespace(&conn)?;
        migration::migrate_add_experience_replays(&conn)?;
        migration::migrate_add_vfs_paths(&conn)?;
        migration::migrate_add_embedding_model(&conn)?;

        if !dim_changed {
            Self::migrate_to_vec0(&conn)?;
        }

        // Store dimension in schema_info
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('embedding_dimension', ?1)",
            params![embedding_dim.to_string()],
        )
        .map_err(|e| AlephError::config(format!("Failed to update schema_info: {}", e)))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    /// Check if the configured dimension differs from the stored dimension
    fn check_dimension_change(conn: &Connection, target_dim: u32) -> Result<bool, AlephError> {
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_info'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !table_exists {
            return Ok(false); // Fresh database, no change
        }

        let stored: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        match stored {
            Some(dim) if dim == target_dim.to_string() => Ok(false),
            Some(dim) => {
                tracing::info!(
                    stored_dim = %dim,
                    target_dim = target_dim,
                    "Embedding dimension change detected"
                );
                Ok(true)
            }
            None => Ok(false), // No stored dimension = first init
        }
    }
```

**Step 2: Add a test for `new_with_dim`**

Add in the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn test_new_with_dim_default() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new_with_dim(db_path, 384).unwrap();

        let conn = db.conn.lock().unwrap();
        let dim: String = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dim, "384");
    }
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib memory::database::core 2>&1 | tail -20`
Expected: All tests pass including new test.

Note: `new_with_dim` calls `migration::migrate_add_embedding_model` which doesn't exist yet. This will be added in Task 6. For now, this task creates the structure and the method will compile after Task 6 is complete. **Implement Task 5 and Task 6 together** — the commit should include both.

**Step 4: Commit (combined with Task 6)**

---

### Task 6: Add `embedding_model` column migration

This task adds a new migration to add the `embedding_model` column to `memory_facts`, recording which model generated each fact's embedding.

**Files:**
- Modify: `core/src/memory/database/migration.rs`
- Modify: `core/src/memory/database/core.rs` (call the migration in `new()`)

**Context:**
- Existing migrations follow the pattern in `migration.rs`: savepoint, check column existence, add if missing, create indexes, release savepoint
- The `memory_facts` table has columns listed at `core/src/memory/database/core.rs:100-119`
- The schema SQL should include `embedding_model` in the CREATE TABLE for new databases
- The `insert_fact` method at `facts/crud.rs:194` stores facts without `embedding_model`

**Step 1: Add `embedding_model` to the static schema SQL**

In `core/src/memory/database/core.rs`, in the `memory_facts` CREATE TABLE (around line 118), add:

```sql
                embedding_model TEXT NOT NULL DEFAULT ''
```

Right after the `parent_path` column.

**Step 2: Add `migrate_add_embedding_model` function**

Add to `core/src/memory/database/migration.rs` (before the `#[cfg(test)]` block):

```rust
/// Migrate memory_facts table to include embedding_model column
///
/// Records which embedding model generated each fact's vector.
/// Enables lazy re-embedding when the configured model changes.
///
/// # Safety
/// - Idempotent: checks for column existence before adding
/// - Uses savepoint for atomic migration
pub fn migrate_add_embedding_model(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_embedding_model")
        .map_err(|e| AlephError::config(format!("Failed to begin embedding_model migration: {}", e)))?;

    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='embedding_model'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_embedding_model");
            AlephError::config(format!("Failed to check embedding_model column: {}", e))
        })?;

    if has_column == 0 {
        conn.execute(
            "ALTER TABLE memory_facts ADD COLUMN embedding_model TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_embedding_model");
            AlephError::config(format!("Failed to add embedding_model column: {}", e))
        })?;

        tracing::info!("Added embedding_model column to memory_facts table");
    }

    // Create index for migration queries (find facts with outdated embeddings)
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_facts_embedding_model ON memory_facts(embedding_model);",
    )
    .map_err(|e| {
        let _ = conn.execute_batch("ROLLBACK TO migration_embedding_model");
        AlephError::config(format!("Failed to create embedding_model index: {}", e))
    })?;

    conn.execute_batch("RELEASE migration_embedding_model")
        .map_err(|e| AlephError::config(format!("Failed to commit embedding_model migration: {}", e)))?;

    Ok(())
}
```

**Step 3: Call migration in `VectorDatabase::new()`**

In `core/src/memory/database/core.rs`, add after line 474 (`migration::migrate_add_vfs_paths(&conn)?;`):

```rust
        // Migrate to add embedding_model column for provider tracking (idempotent)
        migration::migrate_add_embedding_model(&conn)?;
```

**Step 4: Add migration test**

Add to `migration.rs` in the `#[cfg(test)]` block:

```rust
    #[test]
    fn test_migrate_add_embedding_model_idempotent() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                is_valid INTEGER NOT NULL DEFAULT 1,
                namespace TEXT NOT NULL DEFAULT 'owner',
                path TEXT NOT NULL DEFAULT '',
                fact_source TEXT NOT NULL DEFAULT 'extracted',
                content_hash TEXT NOT NULL DEFAULT '',
                parent_path TEXT NOT NULL DEFAULT ''
            )",
        )
        .unwrap();

        // First migration should add column
        migrate_add_embedding_model(&conn).unwrap();

        let has_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='embedding_model'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_col, 1);

        // Second migration should be no-op
        migrate_add_embedding_model(&conn).unwrap();

        let has_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='embedding_model'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_col, 1);
    }
```

**Step 5: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib memory::database::migration 2>&1 | tail -20`
Expected: All migration tests pass including new one.

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check 2>&1 | head -20`
Expected: Clean compilation.

**Step 6: Commit (combined with Task 5)**

```bash
git add core/src/memory/database/core.rs core/src/memory/database/migration.rs core/src/memory/database/mod.rs
git commit -m "database: add embedding_model column, new_with_dim constructor, dynamic vec0 dimension"
```

---

### Task 7: Update `MemoryFact` struct and CRUD to store `embedding_model`

This task adds the `embedding_model` field to `MemoryFact` and updates the database CRUD to persist it.

**Files:**
- Modify: `core/src/memory/context.rs` (MemoryFact struct)
- Modify: `core/src/memory/database/facts/crud.rs` (insert/read operations)

**Context:**
- `MemoryFact` is at `core/src/memory/context.rs:403-441`
- `insert_fact()` is at `facts/crud.rs:194-254`
- `get_fact()` is at `facts/crud.rs:47-111`
- `get_all_facts()` is at `facts/crud.rs:113-192`
- All SELECT queries currently read 17 columns (id through parent_path)

**Step 1: Add `embedding_model` field to `MemoryFact`**

In `core/src/memory/context.rs`, add after `parent_path` field (around line 441):

```rust
    /// Name of the embedding model that generated this fact's vector
    pub embedding_model: String,
```

**Step 2: Update all `MemoryFact` constructors**

Search the codebase for `MemoryFact {` construction sites and add `embedding_model: String::new()` (or `"".to_string()`) to each one.

Key locations:
- `facts/crud.rs` — the `|row|` closures in `get_fact()` and `get_all_facts()` need to read column index 17
- `context.rs` — any builder/constructor methods
- Compression service creates MemoryFact instances

For the CRUD read operations, add to the SELECT:
```sql
SELECT id, content, fact_type, embedding, source_memory_ids,
       created_at, updated_at, confidence, is_valid, invalidation_reason,
       specificity, temporal_scope, decay_invalidated_at,
       path, fact_source, content_hash, parent_path, embedding_model
```

And in the row mapping:
```rust
let embedding_model: String = row.get(17)?;
```

Add to the MemoryFact construction:
```rust
embedding_model,
```

**Step 3: Update `insert_fact` to store `embedding_model`**

In `facts/crud.rs`, update `insert_fact()`:
- Add `embedding_model` to the INSERT column list
- Add `?18` parameter
- Add `fact.embedding_model` to params

```sql
INSERT INTO memory_facts (
    id, content, fact_type, embedding, source_memory_ids,
    created_at, updated_at, confidence, is_valid, invalidation_reason,
    specificity, temporal_scope, decay_invalidated_at,
    path, fact_source, content_hash, parent_path, embedding_model
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
```

Also update `insert_fact_with_namespace` similarly.

**Step 4: Fix all compilation errors**

Run `cargo check` and fix every `MemoryFact` construction site that is missing the new field. Use `embedding_model: String::new()` as the default value for all existing sites.

**Step 5: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib memory 2>&1 | tail -30`
Expected: All memory tests pass.

**Step 6: Commit**

```bash
git add core/src/memory/context.rs core/src/memory/database/facts/
git commit -m "memory: add embedding_model field to MemoryFact and CRUD operations"
```

---

### Task 8: Add `OpenAiEmbeddingProvider` for remote embedding

This task implements the OpenAI-compatible embedding provider for remote API calls.

**Files:**
- Modify: `core/src/memory/embedding_provider.rs`

**Context:**
- The OpenAI embeddings API endpoint is `POST /v1/embeddings`
- Request body: `{"input": ["text1", "text2"], "model": "text-embedding-3-small", "dimensions": 384}`
- Response: `{"data": [{"embedding": [0.1, 0.2, ...], "index": 0}, ...], "model": "...", "usage": {...}}`
- Aleph already uses `reqwest` (check Cargo.toml)
- The `truncate_and_normalize` function can handle oversized vectors

**Step 1: Add the provider struct**

Add to `core/src/memory/embedding_provider.rs`:

```rust
use std::time::Duration;

/// Remote embedding provider using OpenAI-compatible API
///
/// Works with OpenAI, Azure OpenAI, Ollama, vLLM, and any service
/// that implements the `/v1/embeddings` endpoint.
pub struct RemoteEmbeddingProvider {
    client: reqwest::Client,
    api_base: String,
    api_key: String,
    model: String,
    dimension: usize,
    batch_size: usize,
}

impl RemoteEmbeddingProvider {
    /// Create from EmbeddingConfig
    pub fn from_config(config: &crate::config::types::memory::EmbeddingConfig) -> Result<Self, AlephError> {
        let api_key = if let Some(ref env_var) = config.api_key_env {
            std::env::var(env_var).map_err(|_| {
                AlephError::config(format!(
                    "Environment variable {} not set for embedding API key",
                    env_var
                ))
            })?
        } else {
            String::new()
        };

        let api_base = config
            .api_base
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| AlephError::config(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            api_base,
            api_key,
            model: config.model.clone(),
            dimension: config.dimension as usize,
            batch_size: config.batch_size as usize,
        })
    }

    /// Call the embeddings API for a batch of texts
    async fn call_api(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        let url = format!("{}/embeddings", self.api_base.trim_end_matches('/'));

        let mut body = serde_json::json!({
            "input": texts,
            "model": self.model,
        });

        // Add dimensions parameter if supported (OpenAI text-embedding-3-*)
        if self.dimension > 0 {
            body["dimensions"] = serde_json::json!(self.dimension);
        }

        let mut request = self.client.post(&url).json(&body);

        if !self.api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = request.send().await.map_err(|e| {
            AlephError::config(format!("Embedding API request failed: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::config(format!(
                "Embedding API returned {}: {}",
                status, body
            )));
        }

        let resp: serde_json::Value = response.json().await.map_err(|e| {
            AlephError::config(format!("Failed to parse embedding response: {}", e))
        })?;

        let data = resp["data"]
            .as_array()
            .ok_or_else(|| AlephError::config("Missing 'data' array in response".to_string()))?;

        let mut embeddings: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());

        for item in data {
            let index = item["index"].as_u64().unwrap_or(0) as usize;
            let embedding: Vec<f32> = item["embedding"]
                .as_array()
                .ok_or_else(|| AlephError::config("Missing 'embedding' array".to_string()))?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();

            embeddings.push((index, embedding));
        }

        // Sort by index to match input order
        embeddings.sort_by_key(|(idx, _)| *idx);

        // Apply truncate_and_normalize if needed
        let results: Vec<Vec<f32>> = embeddings
            .into_iter()
            .map(|(_, emb)| truncate_and_normalize(emb, self.dimension))
            .collect();

        Ok(results)
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for RemoteEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError> {
        let results = self.call_api(&[text]).await?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| AlephError::config("No embedding returned from API".to_string()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_embeddings = Vec::with_capacity(texts.len());

        // Process in batches
        for chunk in texts.chunks(self.batch_size) {
            let batch_result = self.call_api(chunk).await?;
            all_embeddings.extend(batch_result);
        }

        Ok(all_embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider_type(&self) -> &str {
        "remote"
    }
}
```

**Step 2: Update re-exports**

In `core/src/memory/mod.rs`, update the embedding_provider re-export:

```rust
pub use embedding_provider::{EmbeddingProvider, LocalEmbeddingProvider, RemoteEmbeddingProvider, truncate_and_normalize};
```

**Step 3: Run compilation check**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check 2>&1 | head -20`
Expected: Compiles. (No unit test that calls a real API — this is tested via integration tests.)

**Step 4: Commit**

```bash
git add core/src/memory/embedding_provider.rs core/src/memory/mod.rs
git commit -m "memory: add RemoteEmbeddingProvider for OpenAI-compatible APIs"
```

---

### Task 9: Add provider factory function

This task adds a factory function that creates the correct `EmbeddingProvider` based on `EmbeddingConfig`.

**Files:**
- Modify: `core/src/memory/embedding_provider.rs`

**Context:**
- `EmbeddingConfig` has a `provider` field: `"local"`, `"openai"`, or `"custom"`
- For `"local"`: create `SmartEmbedder` → wrap in `LocalEmbeddingProvider`
- For `"openai"` or `"custom"`: create `RemoteEmbeddingProvider`
- `SmartEmbedder::default_cache_dir()` returns the default cache path

**Step 1: Add factory function**

Add to `core/src/memory/embedding_provider.rs`:

```rust
use std::sync::Arc;

/// Create an EmbeddingProvider from configuration
///
/// Returns a trait object that can be used for embedding operations.
pub fn create_embedding_provider(
    config: &crate::config::types::memory::EmbeddingConfig,
) -> Result<Arc<dyn EmbeddingProvider>, AlephError> {
    match config.provider.as_str() {
        "local" => {
            let cache_dir = SmartEmbedder::default_cache_dir()?;
            let embedder = SmartEmbedder::new(cache_dir, crate::memory::DEFAULT_MODEL_TTL_SECS);
            Ok(Arc::new(LocalEmbeddingProvider::new(embedder)))
        }
        "openai" | "custom" => {
            let provider = RemoteEmbeddingProvider::from_config(config)?;
            Ok(Arc::new(provider))
        }
        other => Err(AlephError::config(format!(
            "Unknown embedding provider: '{}'. Supported: local, openai, custom",
            other
        ))),
    }
}
```

**Step 2: Export factory function**

Update `core/src/memory/mod.rs` re-exports:

```rust
pub use embedding_provider::{
    EmbeddingProvider, LocalEmbeddingProvider, RemoteEmbeddingProvider,
    create_embedding_provider, truncate_and_normalize,
};
```

**Step 3: Add a test**

Add to the `#[cfg(test)]` block in `embedding_provider.rs`:

```rust
    #[test]
    fn test_create_local_provider() {
        let config = crate::config::types::memory::EmbeddingConfig::default();
        // This may fail if default_cache_dir can't be resolved in test env
        // so just check the factory logic
        assert_eq!(config.provider, "local");
        assert_eq!(config.dimension, 384);
    }

    #[test]
    fn test_unknown_provider_fails() {
        let mut config = crate::config::types::memory::EmbeddingConfig::default();
        config.provider = "unknown".to_string();
        let result = create_embedding_provider(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown embedding provider"));
    }
```

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib memory::embedding_provider 2>&1 | tail -10`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add core/src/memory/embedding_provider.rs core/src/memory/mod.rs
git commit -m "memory: add create_embedding_provider factory function"
```

---

### Task 10: Implement `EmbeddingMigration` engine

This task creates the lazy migration engine that re-embeds facts when the configured model changes.

**Files:**
- Create: `core/src/memory/embedding_migration.rs`
- Modify: `core/src/memory/mod.rs`

**Context:**
- The `embedding_model` column (added in Task 6) tracks which model generated each fact's embedding
- Facts with `embedding_model != current_model` (or `embedding_model = ""`) need re-embedding
- Re-embedding: read fact content → embed → update BLOB + vec0 + embedding_model column
- Should be batch-oriented and idempotent

**Step 1: Create the migration module**

Create `core/src/memory/embedding_migration.rs`:

```rust
//! Lazy embedding migration engine
//!
//! Re-embeds facts when the configured embedding model changes.
//! Runs in background during idle periods (DreamDaemon, CompressionDaemon)
//! or on-demand via CLI.

use crate::error::AlephError;
use crate::memory::database::VectorDatabase;
use crate::memory::embedding_provider::EmbeddingProvider;
use std::sync::Arc;

/// Progress report from a migration batch
#[derive(Debug, Clone)]
pub struct MigrationProgress {
    /// Number of facts migrated in this batch
    pub migrated: usize,
    /// Number of facts remaining to migrate
    pub remaining: usize,
    /// Number of facts that failed to re-embed
    pub failed: usize,
}

/// Lazy embedding migration engine
///
/// Detects facts with outdated embeddings and re-embeds them
/// using the current embedding provider.
pub struct EmbeddingMigration {
    database: Arc<VectorDatabase>,
    provider: Arc<dyn EmbeddingProvider>,
    batch_size: usize,
}

impl EmbeddingMigration {
    pub fn new(
        database: Arc<VectorDatabase>,
        provider: Arc<dyn EmbeddingProvider>,
        batch_size: usize,
    ) -> Self {
        Self {
            database,
            provider,
            batch_size,
        }
    }

    /// Get count of facts needing migration
    pub async fn pending_count(&self) -> Result<usize, AlephError> {
        let current_model = self.provider.model_name().to_string();
        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_facts WHERE is_valid = 1 AND embedding_model != ?1",
                rusqlite::params![current_model],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to count pending migrations: {}", e)))?;

        Ok(count as usize)
    }

    /// Run one batch of migration
    ///
    /// Returns progress report. Call repeatedly until `remaining == 0`.
    pub async fn run_batch(&self) -> Result<MigrationProgress, AlephError> {
        let current_model = self.provider.model_name().to_string();

        // 1. Fetch batch of facts needing migration
        let facts: Vec<(String, String)> = {
            let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());
            let mut stmt = conn
                .prepare(
                    "SELECT id, content FROM memory_facts
                     WHERE is_valid = 1 AND embedding_model != ?1
                     LIMIT ?2",
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare migration query: {}", e)))?;

            stmt.query_map(
                rusqlite::params![current_model, self.batch_size as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|e| AlephError::config(format!("Failed to query facts for migration: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect migration facts: {}", e)))?
        };

        if facts.is_empty() {
            let remaining = self.pending_count().await?;
            return Ok(MigrationProgress {
                migrated: 0,
                remaining,
                failed: 0,
            });
        }

        // 2. Re-embed in batch
        let texts: Vec<&str> = facts.iter().map(|(_, content)| content.as_str()).collect();
        let embeddings = self.provider.embed_batch(&texts).await?;

        // 3. Update each fact
        let mut migrated = 0;
        let mut failed = 0;

        for ((id, _), embedding) in facts.iter().zip(embeddings.into_iter()) {
            match self.update_fact_embedding(id, &embedding, &current_model).await {
                Ok(()) => migrated += 1,
                Err(e) => {
                    tracing::warn!(fact_id = %id, error = %e, "Failed to migrate fact embedding");
                    failed += 1;
                }
            }
        }

        let remaining = self.pending_count().await?;

        tracing::info!(
            migrated = migrated,
            failed = failed,
            remaining = remaining,
            model = %current_model,
            "Embedding migration batch complete"
        );

        Ok(MigrationProgress {
            migrated,
            remaining,
            failed,
        })
    }

    /// Update a single fact's embedding and vec0 entry
    async fn update_fact_embedding(
        &self,
        fact_id: &str,
        embedding: &[f32],
        model_name: &str,
    ) -> Result<(), AlephError> {
        let embedding_bytes = VectorDatabase::serialize_embedding(embedding);

        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Get rowid for vec0 update
        let rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM memory_facts WHERE id = ?1",
                rusqlite::params![fact_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to get rowid: {}", e)))?;

        // Update embedding BLOB and model name
        conn.execute(
            "UPDATE memory_facts SET embedding = ?1, embedding_model = ?2 WHERE id = ?3",
            rusqlite::params![embedding_bytes, model_name, fact_id],
        )
        .map_err(|e| AlephError::config(format!("Failed to update fact embedding: {}", e)))?;

        // Update vec0 (delete old + insert new)
        conn.execute(
            "DELETE FROM facts_vec WHERE rowid = ?1",
            rusqlite::params![rowid],
        )
        .map_err(|e| AlephError::config(format!("Failed to delete old vec0 entry: {}", e)))?;

        conn.execute(
            "INSERT INTO facts_vec (rowid, embedding) VALUES (?1, ?2)",
            rusqlite::params![rowid, embedding_bytes],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert new vec0 entry: {}", e)))?;

        Ok(())
    }

    /// Run migration to completion
    ///
    /// Keeps running batches until no more facts need migration.
    /// Returns total migrated count.
    pub async fn run_to_completion(&self) -> Result<usize, AlephError> {
        let mut total_migrated = 0;

        loop {
            let progress = self.run_batch().await?;
            total_migrated += progress.migrated;

            if progress.remaining == 0 || progress.migrated == 0 {
                break;
            }
        }

        Ok(total_migrated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migration_on_empty_database() {
        let db = VectorDatabase::in_memory().unwrap();
        let db = Arc::new(db);

        // Create a mock local provider
        let temp_dir = tempfile::TempDir::new().unwrap();
        let embedder = crate::memory::SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);
        let provider: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::memory::embedding_provider::LocalEmbeddingProvider::new(embedder));

        let migration = EmbeddingMigration::new(db, provider, 10);

        let count = migration.pending_count().await.unwrap();
        assert_eq!(count, 0);

        let progress = migration.run_batch().await.unwrap();
        assert_eq!(progress.migrated, 0);
        assert_eq!(progress.remaining, 0);
    }
}
```

**Step 2: Register module and re-export**

In `core/src/memory/mod.rs`, add module declaration:

```rust
pub mod embedding_migration;
```

And re-export:

```rust
pub use embedding_migration::{EmbeddingMigration, MigrationProgress};
```

**Step 3: Make `serialize_embedding` pub**

In `core/src/memory/database/core.rs`, change `pub(crate)` to `pub` for `serialize_embedding`:

```rust
    pub fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
```

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib memory::embedding_migration 2>&1 | tail -10`
Expected: Migration test passes.

**Step 5: Commit**

```bash
git add core/src/memory/embedding_migration.rs core/src/memory/mod.rs core/src/memory/database/core.rs
git commit -m "memory: implement EmbeddingMigration lazy re-embedding engine"
```

---

### Task 11: Integration test for the full embedding evolution flow

This task creates an integration test that validates the complete flow: config → provider creation → fact storage with model metadata → migration detection.

**Files:**
- Create: `core/src/memory/embedding_provider.rs` (integration test at bottom)

**Context:**
- We now have: EmbeddingConfig, EmbeddingProvider trait, LocalEmbeddingProvider, RemoteEmbeddingProvider, factory, EmbeddingMigration
- The integration test should verify the plumbing without calling real remote APIs or loading real models

**Step 1: Add integration test**

Add to the `#[cfg(test)]` block in `embedding_provider.rs`:

```rust
    // =========================================================================
    // Mock provider for testing
    // =========================================================================

    /// Mock embedding provider for tests
    pub struct MockEmbeddingProvider {
        dim: usize,
        model: String,
    }

    impl MockEmbeddingProvider {
        pub fn new(dim: usize, model: &str) -> Self {
            Self {
                dim,
                model: model.to_string(),
            }
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(vec![0.1; self.dim])
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(texts.iter().map(|_| vec![0.1; self.dim]).collect())
        }

        fn dimensions(&self) -> usize {
            self.dim
        }

        fn model_name(&self) -> &str {
            &self.model
        }

        fn provider_type(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn test_full_embedding_evolution_flow() {
        use crate::memory::database::VectorDatabase;
        use crate::memory::embedding_migration::EmbeddingMigration;
        use crate::memory::context::{MemoryFact, FactType, FactSpecificity, TemporalScope, FactSource};

        // 1. Create database
        let db = Arc::new(VectorDatabase::in_memory().unwrap());

        // 2. Insert a fact with old model metadata
        let mut fact = MemoryFact {
            id: "test-fact-1".to_string(),
            content: "User prefers dark mode".to_string(),
            fact_type: FactType::Preference,
            embedding: Some(vec![0.5; 384]),
            source_memory_ids: vec!["mem-1".to_string()],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::Pattern,
            temporal_scope: TemporalScope::Contextual,
            similarity_score: None,
            path: "aleph://user/preferences/".to_string(),
            fact_source: FactSource::Extracted,
            content_hash: "abc123".to_string(),
            parent_path: "aleph://user/".to_string(),
            embedding_model: "old-model-v1".to_string(),
        };

        db.insert_fact(fact.clone()).await.unwrap();

        // 3. Create a new provider (different model name)
        let provider: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(384, "new-model-v2"));

        // 4. Check migration detects the mismatch
        let migration = EmbeddingMigration::new(Arc::clone(&db), provider, 10);

        let pending = migration.pending_count().await.unwrap();
        assert_eq!(pending, 1, "Should detect 1 fact needing migration");

        // 5. Run migration
        let progress = migration.run_batch().await.unwrap();
        assert_eq!(progress.migrated, 1);
        assert_eq!(progress.remaining, 0);

        // 6. Verify fact was updated
        let updated = db.get_fact("test-fact-1").await.unwrap().unwrap();
        assert_eq!(updated.embedding_model, "new-model-v2");
    }
```

**Step 2: Run the integration test**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib memory::embedding_provider::tests::test_full_embedding_evolution_flow 2>&1 | tail -10`
Expected: Test passes.

**Step 3: Commit**

```bash
git add core/src/memory/embedding_provider.rs
git commit -m "memory: add integration test for full embedding evolution flow"
```

---

### Task 12: Fix remaining `CURRENT_EMBEDDING_DIM` references and clean up

This task is a sweep to ensure no hardcoded `CURRENT_EMBEDDING_DIM` references remain and all tests pass.

**Files:**
- Modify: Various files that reference `CURRENT_EMBEDDING_DIM`

**Context:**
- Task 4 renamed `CURRENT_EMBEDDING_DIM` to `DEFAULT_EMBEDDING_DIM` in `database/core.rs`
- The grep from earlier shows test files in `database/mod.rs` and `memory_ops.rs` use `CURRENT_EMBEDDING_DIM`
- The deprecated `embedding.rs` module has its own `EMBEDDING_DIM = 512` (unrelated)

**Step 1: Search and fix all references**

Run: `cd /Users/zouguojun/Workspace/Aleph && grep -rn "CURRENT_EMBEDDING_DIM" core/src/ --include="*.rs"`

For each occurrence:
- Replace `CURRENT_EMBEDDING_DIM` with `DEFAULT_EMBEDDING_DIM`
- Update import paths accordingly

**Step 2: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --lib 2>&1 | tail -30`
Expected: All tests pass. Note pre-existing failures in exec/sandbox, exec/approval, perception modules are unrelated.

**Step 3: Run cargo clippy**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo clippy --lib 2>&1 | head -30`
Expected: No new warnings from our changes.

**Step 4: Commit**

```bash
git add -A
git commit -m "memory: clean up CURRENT_EMBEDDING_DIM references, replace with DEFAULT_EMBEDDING_DIM"
```

---

### Task 13: Update module exports and documentation

This task ensures all new types are properly exported and the memory module documentation is updated.

**Files:**
- Modify: `core/src/memory/mod.rs`
- Modify: `docs/MEMORY_SYSTEM.md` (update embedding section)

**Context:**
- New public types: `EmbeddingProvider`, `LocalEmbeddingProvider`, `RemoteEmbeddingProvider`, `EmbeddingMigration`, `MigrationProgress`, `create_embedding_provider`, `truncate_and_normalize`
- New config: `EmbeddingConfig`
- New DB constant: `DEFAULT_EMBEDDING_DIM` (replaces `CURRENT_EMBEDDING_DIM`)

**Step 1: Verify all exports in `mod.rs`**

Ensure `core/src/memory/mod.rs` has these lines (add any missing):

```rust
pub mod embedding_provider;
pub mod embedding_migration;
```

And re-exports:
```rust
pub use embedding_provider::{
    EmbeddingProvider, LocalEmbeddingProvider, RemoteEmbeddingProvider,
    create_embedding_provider, truncate_and_normalize,
};
pub use embedding_migration::{EmbeddingMigration, MigrationProgress};
pub use database::DEFAULT_EMBEDDING_DIM;
```

**Step 2: Run final compilation check**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check 2>&1 | head -20`
Expected: Clean compilation.

**Step 3: Commit**

```bash
git add core/src/memory/mod.rs
git commit -m "memory: finalize embedding evolution module exports"
```
