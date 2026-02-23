//! SkillRegistry — in-memory registry with priority-based deduplication.

use std::collections::HashMap;

use crate::domain::skill::{SkillId, SkillManifest};
use crate::domain::Entity;

/// In-memory skill registry that stores `SkillManifest` values keyed by `SkillId`.
///
/// When two manifests share the same id, the one with the higher
/// [`SkillSource::priority`] wins (workspace > plugin > global > bundled).
#[derive(Debug, Default)]
pub struct SkillRegistry {
    manifests: HashMap<SkillId, SkillManifest>,
}

impl SkillRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a single manifest.
    ///
    /// If a manifest with the same id already exists, the one with the
    /// higher source priority is kept. Returns `true` if the manifest was
    /// actually inserted (or replaced the existing one).
    pub fn register(&mut self, manifest: SkillManifest) -> bool {
        let id = manifest.id().clone();
        match self.manifests.get(&id) {
            Some(existing) if existing.priority() >= manifest.priority() => {
                // Existing has equal or higher priority — reject the newcomer.
                false
            }
            _ => {
                self.manifests.insert(id, manifest);
                true
            }
        }
    }

    /// Register multiple manifests at once.
    pub fn register_all(&mut self, manifests: impl IntoIterator<Item = SkillManifest>) {
        for m in manifests {
            self.register(m);
        }
    }

    /// Look up a manifest by id.
    pub fn get(&self, id: &SkillId) -> Option<&SkillManifest> {
        self.manifests.get(id)
    }

    /// Return all registered manifests.
    pub fn list_all(&self) -> Vec<&SkillManifest> {
        self.manifests.values().collect()
    }

    /// Number of registered skills.
    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }

    /// Remove all registered skills.
    pub fn clear(&mut self) {
        self.manifests.clear();
    }

    /// Iterate over all (id, manifest) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&SkillId, &SkillManifest)> {
        self.manifests.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{SkillContent, SkillSource};

    /// Helper: create a SkillManifest with the given name and source.
    fn make_manifest(name: &str, source: SkillSource) -> SkillManifest {
        SkillManifest::new(
            SkillId::new(name),
            name,
            &format!("{} description", name),
            SkillContent::new("content"),
            source,
        )
    }

    #[test]
    fn register_and_get() {
        let mut reg = SkillRegistry::new();
        assert!(reg.is_empty());

        let m = make_manifest("git:commit", SkillSource::Bundled);
        assert!(reg.register(m));
        assert_eq!(reg.len(), 1);

        let id = SkillId::new("git:commit");
        let got = reg.get(&id).expect("should be present");
        assert_eq!(got.name(), "git:commit");
        assert_eq!(got.description(), "git:commit description");
    }

    #[test]
    fn dedup_higher_priority_wins() {
        let mut reg = SkillRegistry::new();

        // Register bundled first (priority 1)
        let bundled = make_manifest("git:commit", SkillSource::Bundled);
        assert!(reg.register(bundled));

        // Register workspace version (priority 4) — should replace
        let workspace = make_manifest("git:commit", SkillSource::Workspace);
        assert!(reg.register(workspace));

        assert_eq!(reg.len(), 1);
        let id = SkillId::new("git:commit");
        let got = reg.get(&id).unwrap();
        assert_eq!(*got.source(), SkillSource::Workspace);
    }

    #[test]
    fn dedup_lower_priority_rejected() {
        let mut reg = SkillRegistry::new();

        // Register workspace first (priority 4)
        let workspace = make_manifest("git:commit", SkillSource::Workspace);
        assert!(reg.register(workspace));

        // Try to register bundled (priority 1) — should be rejected
        let bundled = make_manifest("git:commit", SkillSource::Bundled);
        assert!(!reg.register(bundled));

        assert_eq!(reg.len(), 1);
        let id = SkillId::new("git:commit");
        let got = reg.get(&id).unwrap();
        assert_eq!(*got.source(), SkillSource::Workspace);
    }

    #[test]
    fn list_all() {
        let mut reg = SkillRegistry::new();
        reg.register(make_manifest("a:one", SkillSource::Bundled));
        reg.register(make_manifest("b:two", SkillSource::Global));
        reg.register(make_manifest("c:three", SkillSource::Workspace));

        let all = reg.list_all();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn clear() {
        let mut reg = SkillRegistry::new();
        reg.register(make_manifest("a:one", SkillSource::Bundled));
        reg.register(make_manifest("b:two", SkillSource::Global));
        assert_eq!(reg.len(), 2);

        reg.clear();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }
}
