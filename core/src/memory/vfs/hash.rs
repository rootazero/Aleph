//! Content hashing for L1 staleness detection

use sha2::{Sha256, Digest};
use crate::memory::MemoryFact;

/// Compute a stable content hash for a set of facts under a path.
/// Used to detect when L1 Overviews need regeneration.
///
/// The hash is deterministic regardless of input order (sorts by id internally).
/// Returns a 16-character hex string.
pub fn compute_directory_hash(facts: &[MemoryFact]) -> String {
    let mut hasher = Sha256::new();
    let mut sorted: Vec<&MemoryFact> = facts.iter().collect();
    sorted.sort_by_key(|f| &f.id);
    for fact in sorted {
        hasher.update(fact.id.as_bytes());
        hasher.update(fact.updated_at.to_be_bytes());
    }
    hex::encode(hasher.finalize())[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;

    #[test]
    fn test_compute_directory_hash_deterministic() {
        let fact1 = MemoryFact::with_id("aaa".into(), "Fact A".into(), FactType::Preference);
        let fact2 = MemoryFact::with_id("bbb".into(), "Fact B".into(), FactType::Learning);

        let hash1 = compute_directory_hash(&[fact1.clone(), fact2.clone()]);
        let hash2 = compute_directory_hash(&[fact2.clone(), fact1.clone()]);

        // Order shouldn't matter (sorted by id internally)
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 16);
    }

    #[test]
    fn test_compute_directory_hash_changes_on_update() {
        let fact1 = MemoryFact::with_id("aaa".into(), "Fact A".into(), FactType::Preference);
        let mut fact1_updated = fact1.clone();
        fact1_updated.updated_at += 1;

        let hash_before = compute_directory_hash(&[fact1]);
        let hash_after = compute_directory_hash(&[fact1_updated]);

        assert_ne!(hash_before, hash_after);
    }

    #[test]
    fn test_compute_directory_hash_empty() {
        let hash = compute_directory_hash(&[]);
        assert_eq!(hash.len(), 16);
    }
}
