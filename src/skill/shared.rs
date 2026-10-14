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

static INIT_CELL: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Populate the shared system with the default skill directories, once per
/// process.
///
/// The latch lives here, beside the singleton it initializes, so every consumer
/// (gateway RPC handlers, the Hub's installed-state reconciliation, tools)
/// shares one first-init rather than each holding its own cell — the
/// divergence this module exists to prevent, one level up.
///
/// **Merge semantics, not replace.** The base dir set is only ever widened
/// (`ensure_dir_registered`, and `SkillSystem::init` merges too): a second
/// initializer running after `ExtensionManager` already registered
/// `discover_skill_dirs()` can add dirs, never remove them, so no
/// project-local `.aleph/skills` is silently dropped from the registry,
/// snapshot, and injected `<available_skills>` prompt index. Plugin skill dirs
/// are not in this set at all — every rescan reads them from the list
/// `extension::projection` publishes (`SkillSystem::scan_roots`). The
/// `INIT_CELL` latch makes the rescan here run once; subsequent calls
/// short-circuit.
///
/// The synchronous steps of THIS cold path run on the blocking pool via
/// `super::run_blocking_fs`, so the first `skills.status` /
/// `extensions.installed` / `extensions.catalog` does not pin a runtime worker
/// on them: first construction of the singleton (`SkillSystem::new` reads
/// `skills.toml`), `default_skill_dirs`' `exists()` probes, the root-set
/// canonicalisation dedup in `scan_roots`, the recursive scan in
/// `rescan_dirs`, and config load / usage sidecars in the snapshot rebuild.
/// This does NOT cover extension loading (`ExtensionManager::ensure_loaded`),
/// which is a separate, still-pending boundary, nor the public synchronous
/// [`shared_skill_system`] accessor, which constructs inline when it is the
/// first caller.
///
/// **Cancellation.** If either blocking hop (singleton/dirs, or the recursive
/// scan inside `rescan_dirs`) is cancelled (runtime shutdown) the latch is
/// NOT set: the error is logged and this call returns without
/// having initialised anything, so a later call retries the whole first-init
/// instead of the process keeping a silently half-initialised system.
/// A panic inside the blocking work is re-raised on the caller
/// (`run_blocking_fs`).
pub async fn ensure_shared_skill_system_initialized() {
    let outcome: Result<&(), ()> = INIT_CELL
        .get_or_try_init(|| async {
            // Obtain the singleton INSIDE the blocking closure: its first
            // construction (`SkillSystem::new` -> `SkillsConfig::load`) is
            // synchronous file I/O. `SkillSystem` is `Clone` over an `Arc`,
            // so the owned clone shares the registry with every other holder.
            let (system, dirs) =
                super::run_blocking_fs("ensure_shared_skill_system_initialized", || {
                    let system = shared_skill_system().clone();
                    (system, super::default_skill_dirs())
                })
                .await
                .ok_or(())?;
            for dir in dirs {
                system.ensure_dir_registered(dir).await;
            }
            // A cancelled rescan (runtime shutdown) must not latch the cell:
            // the registry was left untouched, so report it as a failed init.
            if !system.rescan_dirs().await {
                return Err(());
            }
            Ok(())
        })
        .await;
    if outcome.is_err() {
        tracing::warn!(
            "shared skill system first-init cancelled; not latched, next call will retry"
        );
    }
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
