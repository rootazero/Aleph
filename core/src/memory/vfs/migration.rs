//! Migration logic for assigning paths to existing facts
//!
//! This was originally used to backfill aleph:// paths for facts stored
//! in SQLite before the VFS system was introduced. Since the memory system
//! has been migrated to LanceDB where all facts are created with paths,
//! this function is now a no-op.

use crate::error::AlephError;
use crate::memory::store::MemoryBackend;
use tracing::info;

/// Migrate existing facts to have aleph:// paths based on their FactType.
///
/// With the LanceDB backend, all facts are created with paths at insertion time,
/// so this function is a no-op. It is retained for backward compatibility.
pub async fn migrate_existing_facts_to_paths(_database: &MemoryBackend) -> Result<usize, AlephError> {
    info!("No facts need path migration (LanceDB backend assigns paths at insertion)");
    Ok(0)
}
