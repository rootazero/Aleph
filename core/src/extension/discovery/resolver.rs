//! Plugin conflict resolution based on origin priority

use std::collections::HashMap;
use tracing::info;

use super::scanner::PluginCandidate;

/// Resolve plugin conflicts based on origin priority
///
/// When multiple plugins have the same ID, the one with highest
/// priority origin wins. Others are marked for override tracking.
pub fn resolve_conflicts(candidates: Vec<PluginCandidate>) -> ResolvedPlugins {
    let mut by_id: HashMap<String, Vec<PluginCandidate>> = HashMap::new();

    // Group by ID
    for candidate in candidates {
        by_id.entry(candidate.id.clone()).or_default().push(candidate);
    }

    let mut active = Vec::new();
    let mut overridden = Vec::new();

    for (id, mut group) in by_id {
        if group.len() == 1 {
            active.push(group.pop().unwrap());
        } else {
            // Sort by priority (highest first)
            group.sort_by(|a, b| b.origin.priority().cmp(&a.origin.priority()));

            let winner = group.remove(0);
            info!(
                "Plugin '{}' from {:?} overrides {} other(s)",
                id,
                winner.origin,
                group.len()
            );

            active.push(winner);
            overridden.extend(group);
        }
    }

    ResolvedPlugins { active, overridden }
}

/// Result of conflict resolution
#[derive(Debug)]
pub struct ResolvedPlugins {
    /// Plugins that should be loaded
    pub active: Vec<PluginCandidate>,
    /// Plugins that were overridden by higher priority
    pub overridden: Vec<PluginCandidate>,
}

impl ResolvedPlugins {
    /// Check if there are any active plugins
    pub fn has_plugins(&self) -> bool {
        !self.active.is_empty()
    }

    /// Get total number of discovered plugins (active + overridden)
    pub fn total_count(&self) -> usize {
        self.active.len() + self.overridden.len()
    }

    /// Get number of conflicts resolved
    pub fn conflict_count(&self) -> usize {
        self.overridden.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::manifest::PluginManifest;
    use crate::extension::types::{PluginKind, PluginOrigin};
    use std::path::PathBuf;

    fn make_candidate(id: &str, origin: PluginOrigin) -> PluginCandidate {
        PluginCandidate {
            id: id.to_string(),
            source: PathBuf::new(),
            root_dir: PathBuf::new(),
            origin,
            kind: PluginKind::Static,
            manifest: PluginManifest::new(
                id.to_string(),
                id.to_string(),
                PluginKind::Static,
                PathBuf::new(),
            ),
        }
    }

    #[test]
    fn test_resolve_no_conflicts() {
        let candidates = vec![
            make_candidate("a", PluginOrigin::Global),
            make_candidate("b", PluginOrigin::Workspace),
        ];

        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active.len(), 2);
        assert_eq!(resolved.overridden.len(), 0);
        assert!(resolved.has_plugins());
        assert_eq!(resolved.total_count(), 2);
        assert_eq!(resolved.conflict_count(), 0);
    }

    #[test]
    fn test_resolve_with_conflict() {
        let candidates = vec![
            make_candidate("same", PluginOrigin::Bundled),
            make_candidate("same", PluginOrigin::Workspace),
            make_candidate("same", PluginOrigin::Global),
        ];

        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active.len(), 1);
        assert_eq!(resolved.overridden.len(), 2);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Workspace);
        assert_eq!(resolved.conflict_count(), 2);
    }

    #[test]
    fn test_resolve_config_highest_priority() {
        let candidates = vec![
            make_candidate("plugin", PluginOrigin::Config),
            make_candidate("plugin", PluginOrigin::Workspace),
        ];

        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Config);
    }

    #[test]
    fn test_resolve_priority_order() {
        // Test full priority chain: Config > Workspace > Global > Bundled
        let candidates = vec![
            make_candidate("p1", PluginOrigin::Bundled),
            make_candidate("p1", PluginOrigin::Global),
        ];
        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Global);

        let candidates = vec![
            make_candidate("p2", PluginOrigin::Global),
            make_candidate("p2", PluginOrigin::Workspace),
        ];
        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Workspace);

        let candidates = vec![
            make_candidate("p3", PluginOrigin::Workspace),
            make_candidate("p3", PluginOrigin::Config),
        ];
        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Config);
    }

    #[test]
    fn test_resolve_empty() {
        let candidates: Vec<PluginCandidate> = vec![];
        let resolved = resolve_conflicts(candidates);
        assert!(resolved.active.is_empty());
        assert!(resolved.overridden.is_empty());
        assert!(!resolved.has_plugins());
    }

    #[test]
    fn test_resolve_mixed_plugins() {
        let candidates = vec![
            make_candidate("unique-a", PluginOrigin::Global),
            make_candidate("shared", PluginOrigin::Bundled),
            make_candidate("shared", PluginOrigin::Config),
            make_candidate("unique-b", PluginOrigin::Workspace),
        ];

        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active.len(), 3); // unique-a, shared (config), unique-b
        assert_eq!(resolved.overridden.len(), 1); // shared (bundled)

        // Check that the shared plugin winner is from Config
        let shared = resolved.active.iter().find(|c| c.id == "shared").unwrap();
        assert_eq!(shared.origin, PluginOrigin::Config);
    }
}
