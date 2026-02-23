//! Slash command resolution — maps user-typed command names to skill manifests.
//!
//! Resolution strategy:
//! 1. Try exact match on full `SkillId` (e.g. "git:commit")
//! 2. Try match on skill name part only (e.g. "commit" matches "git:commit")
//! 3. Only user-invocable skills are considered

use crate::domain::skill::{SkillId, SkillManifest};
use crate::domain::Entity;
use crate::skill::registry::SkillRegistry;

/// A resolved slash command backed by a skill.
#[derive(Debug, Clone)]
pub struct SkillCommandSpec {
    /// The skill ID this command resolves to.
    pub skill_id: SkillId,
    /// Human-readable command name.
    pub name: String,
    /// Short description for help text.
    pub description: String,
}

/// Resolve a user-typed command name to a skill command spec.
///
/// Resolution order:
/// 1. Exact match on `SkillId` (e.g. "git:commit")
/// 2. Match on `skill_name()` part (e.g. "commit" matches "git:commit")
///
/// Only user-invocable skills are considered.
pub fn resolve_command(name: &str, registry: &SkillRegistry) -> Option<SkillCommandSpec> {
    // 1. Try exact SkillId match
    let exact_id = SkillId::new(name);
    if let Some(manifest) = registry.get(&exact_id) {
        if manifest.is_user_invocable() {
            return Some(spec_from_manifest(manifest));
        }
    }

    // 2. Try match on skill_name() part
    for (_id, manifest) in registry.iter() {
        if manifest.id().skill_name() == name && manifest.is_user_invocable() {
            return Some(spec_from_manifest(manifest));
        }
    }

    None
}

/// List all available slash commands from the registry.
///
/// Only includes user-invocable skills.
pub fn list_available_commands(registry: &SkillRegistry) -> Vec<SkillCommandSpec> {
    let mut commands: Vec<SkillCommandSpec> = registry
        .iter()
        .filter(|(_, manifest)| manifest.is_user_invocable())
        .map(|(_, manifest)| spec_from_manifest(manifest))
        .collect();

    // Sort by skill ID for deterministic output
    commands.sort_by(|a, b| a.skill_id.as_str().cmp(b.skill_id.as_str()));
    commands
}

/// Build a `SkillCommandSpec` from a manifest.
fn spec_from_manifest(manifest: &SkillManifest) -> SkillCommandSpec {
    SkillCommandSpec {
        skill_id: manifest.id().clone(),
        name: manifest.name().to_string(),
        description: manifest.description().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{
        InvocationPolicy, SkillContent, SkillManifest, SkillSource,
    };

    fn make_manifest(id: &str, name: &str, invocable: bool) -> SkillManifest {
        let mut m = SkillManifest::new(
            id,
            name,
            &format!("{} description", name),
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        if !invocable {
            m.set_invocation(InvocationPolicy {
                user_invocable: false,
                ..Default::default()
            });
        }
        m
    }

    #[test]
    fn resolve_by_exact_id() {
        let mut registry = SkillRegistry::new();
        registry.register(make_manifest("git:commit", "Git Commit", true));

        let result = resolve_command("git:commit", &registry);
        assert!(result.is_some());
        let spec = result.unwrap();
        assert_eq!(spec.skill_id.as_str(), "git:commit");
        assert_eq!(spec.name, "Git Commit");
    }

    #[test]
    fn resolve_by_skill_name() {
        let mut registry = SkillRegistry::new();
        registry.register(make_manifest("git:commit", "Git Commit", true));

        let result = resolve_command("commit", &registry);
        assert!(result.is_some());
        let spec = result.unwrap();
        assert_eq!(spec.skill_id.as_str(), "git:commit");
    }

    #[test]
    fn resolve_not_found() {
        let mut registry = SkillRegistry::new();
        registry.register(make_manifest("git:commit", "Git Commit", true));

        let result = resolve_command("nonexistent", &registry);
        assert!(result.is_none());
    }

    #[test]
    fn non_invocable_excluded() {
        let mut registry = SkillRegistry::new();
        registry.register(make_manifest("internal:hidden", "Hidden", false));

        // Exact match should fail because not invocable
        let result = resolve_command("internal:hidden", &registry);
        assert!(result.is_none());

        // Name match should also fail
        let result = resolve_command("hidden", &registry);
        assert!(result.is_none());
    }

    #[test]
    fn list_commands() {
        let mut registry = SkillRegistry::new();
        registry.register(make_manifest("git:commit", "Git Commit", true));
        registry.register(make_manifest("docker:build", "Docker Build", true));
        registry.register(make_manifest("internal:hidden", "Hidden", false));

        let commands = list_available_commands(&registry);

        // Only invocable skills should be listed
        assert_eq!(commands.len(), 2);

        // Should be sorted by skill_id
        assert_eq!(commands[0].skill_id.as_str(), "docker:build");
        assert_eq!(commands[1].skill_id.as_str(), "git:commit");
    }
}
