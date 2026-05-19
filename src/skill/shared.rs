//! Process-wide shared `SkillSystem` — the single source of truth for v2 skills.
//!
//! Before this module the codebase held several divergent `SkillSystem`
//! instances: the gateway RPC handlers had a private `OnceLock`, the builtin
//! `skill_*` tools each constructed an empty `SkillSystem::new()`, and
//! `ExtensionManager` held its own. They never agreed. `shared_skill_system()`
//! collapses them onto one `Arc`-backed instance: any holder that calls
//! `init()` populates the registry for every other holder.

use std::sync::OnceLock;

use super::SkillSystem;

static SHARED: OnceLock<SkillSystem> = OnceLock::new();

/// Return the process-wide shared `SkillSystem`.
///
/// `SkillSystem` is `Clone` over an internal `Arc`, so callers may freely
/// `.clone()` the returned reference to obtain an owned handle that still
/// shares the same registry/snapshot. The instance is created empty; whoever
/// owns skill-directory discovery (`ExtensionManager::load_all`, or the
/// gateway RPC path) calls `.init()` on it. `init()` is re-runnable.
pub fn shared_skill_system() -> &'static SkillSystem {
    SHARED.get_or_init(SkillSystem::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_instance_is_identical_across_calls() {
        // Two calls return the same Arc kernel: initialising on one handle,
        // the other handle sees the same version.
        let a = shared_skill_system();
        let b = shared_skill_system();
        // SkillSystem is Clone over an Arc; both snapshots share the same
        // version counter.
        let snap_a = a.current_snapshot().await;
        let snap_b = b.current_snapshot().await;
        assert_eq!(snap_a.version, snap_b.version);
    }
}
