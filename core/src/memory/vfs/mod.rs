//! Virtual Filesystem (VFS) layer for hierarchical memory organization
//!
//! Provides the aleph:// URI scheme for organizing facts into
//! a navigable directory structure.

pub mod hash;
pub mod l1_generator;
pub mod migration;

pub use hash::compute_directory_hash;
pub use l1_generator::L1Generator;
pub use migration::migrate_existing_facts_to_paths;

use crate::memory::namespace::NamespaceScope;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::FactSource;

/// Load top-level L1 Overviews for Agent bootstrapping.
/// Returns Markdown text suitable for injection into system prompt.
pub async fn bootstrap_agent_context(database: &MemoryBackend) -> String {
    let top_level_paths = [
        "aleph://user/",
        "aleph://knowledge/",
        "aleph://agent/",
    ];

    let mut sections = Vec::new();

    for path in &top_level_paths {
        // Old: database.get_l1_overview(path) → New: database.get_by_path(path, &NamespaceScope::Owner)
        if let Ok(Some(l1)) = database.get_by_path(path, &NamespaceScope::Owner).await {
            if l1.fact_source == FactSource::Summary {
                sections.push(format!("### {}\n{}", path, l1.content));
            }
        }
    }

    if sections.is_empty() {
        return String::new();
    }

    format!("## Memory Overview\n\n{}", sections.join("\n\n"))
}

#[cfg(test)]
mod integration_tests;
