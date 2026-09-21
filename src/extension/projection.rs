//! The single place where plugin state becomes a **process-global projection**
//! — the *view* half of the plugin lifecycle.
//!
//! # Effects and views (spec 2026-09-20 §3.1, ruling U8 "C")
//!
//! A plugin's footprint on the running process splits by one question: **does
//! it have an inverse?**
//!
//! * **Effects** have one, and live in the plugin's `EffectScope`
//!   (`effects/scope.rs`): the registry row, the WASM module, transient MCP
//!   servers, background services, the memory extension, slash entries. Each
//!   registration returns a `Disposer`; unmount (`lifecycle.rs`) disposes
//!   the scope, which runs the list in reverse (`effects/scope.rs`). Guard:
//!   `effects::census` (G1) and `lifecycle::tests::g2_*` (G2).
//! * **Views** have none but can be recomputed from the registry, and that is
//!   what this module does:
//!   - `utils::paths::PLUGIN_SKILL_DIRS` — read by `get_all_skills_dirs`, i.e.
//!     the search set of the `skill_read` / `skill_list` tools.
//!   - `agents::PLUGIN_SUBAGENTS` — read by `AgentRegistry::resolve`
//!     (delegation) and the harness prompt builder (`<available_agents>`).
//!   - `SkillSystem` — the scan that feeds the model's `<available_skills>`
//!     index.
//!   - `ExtensionManager::active_plugin_tools` — the tool-name index.
//!     (The hook executor is the fifth view; `after_transition` rebuilds it
//!     with `sync_hooks_from_registry` — registry-defined hooks — then
//!     `sync_user_hooks` — the user's own layer on top — triggered from the
//!     same place.)
//!
//! **One function derives the whole view set from the registry, and exactly
//! one trigger calls it: `lifecycle.rs::after_transition`, once at the end of
//! every public lifecycle primitive** (`mount` / `unmount` / `reload_plugin` /
//! `reload` / `load_all`). Guard:
//! `tests::publishing_plugin_projections_has_exactly_one_author` (G3) pins
//! both the publish calls to this file and the trigger to `lifecycle.rs`.
//!
//! # What was narrowed, and why
//!
//! Three rounds (2026-08-15 dsh, 08-16 plugin-system, 08-19 compat) ruled
//! "对照 Cordis 但架构不移植", and this comment used to say the derivation was
//! the cheaper equivalent of a fiber's `dispose()` because "every path that
//! can change plugin activation calls it". That was a list (判据 §5): the
//! derivation covered three surfaces, and four effects outside it leaked
//! past a disable — memory extensions (no unregister), slash entries
//! (boot-only), MCP servers on re-enable (only `reload()` re-added them), and
//! the narrow `reload_plugin` twin. The 2026-09-20 round kept the rulings'
//! substance (no DI container, no Proxy context, no cascade restart, no HMR)
//! and absorbed the one thing Cordis actually enforces: **a registration
//! returns its disposer and the plugin handle owns it**
//! (scan-dsh-cordis.md Top-8 #1). `src/harness/` is untouched.
//!
//! # The bug the single derivation fixed (still true)
//!
//! Before this module there were two authors of the derivation — `load_all`
//! and `set_plugin_enabled` — and **they disagreed about the predicate**:
//!
//! | | skill dirs | sub-agents |
//! |---|---|---|
//! | `load_all` | `list_plugins()` — every status | `list_agents()` — unfiltered |
//! | `set_plugin_enabled` | `list_active_plugins()` | filtered by `status.is_active()` |
//!
//! So a boot (or any `reload()`, which the file watcher triggers) published the
//! skills and sub-agents of plugins that were **disabled, shadowed, or failed to
//! load** while a runtime toggle used the correct predicate. The predicate is
//! stated once, in [`ExtensionManager::derive_plugin_projection`].

use std::path::PathBuf;

use super::ExtensionManager;

/// Everything a plugin set projects onto process-global state.
///
/// Derived from the registry in one pass so the *same* activation predicate
/// decides every surface; publishing is a separate step so the derivation can
/// be unit-tested without touching global state.
#[derive(Debug, Default, Clone)]
pub(crate) struct PluginProjection {
    /// `<root_dir>/skills` of every **active** plugin, de-duplicated and in
    /// registry order. Feeds both the `SkillSystem` scan and
    /// `publish_plugin_skill_dirs`.
    pub(crate) plugin_skill_dirs: Vec<PathBuf>,
    /// Sub-agents contributed by **active** plugins.
    pub(crate) subagents: Vec<crate::agents::AgentDef>,
}

impl ExtensionManager {
    /// Derive the projection from the registry under one read lock.
    ///
    /// `base_skill_dirs` is the non-plugin part of the skill search set; it is
    /// passed in so a plugin dir that discovery already found is not added
    /// twice (the `SkillSystem` would scan it once anyway, but a duplicate in
    /// `PLUGIN_SKILL_DIRS` would make `skill_read` report the same skill from
    /// two roots).
    ///
    /// **The activation predicate lives here and nowhere else.** `is_active()`
    /// is true only for [`PluginStatus::Loaded`](crate::extension::PluginStatus),
    /// so `Disabled` / `Overridden` / `Error` plugins contribute nothing — which
    /// is the whole point: an inactive plugin must be invisible to the model,
    /// not merely absent from the management list.
    async fn derive_plugin_projection(&self, base_skill_dirs: &[PathBuf]) -> PluginProjection {
        let registry = self.plugin_registry.read().await;

        let mut plugin_skill_dirs: Vec<PathBuf> = Vec::new();
        for record in registry.list_active_plugins() {
            let dir = record.root_dir.join("skills");
            if dir.is_dir() && !base_skill_dirs.contains(&dir) && !plugin_skill_dirs.contains(&dir)
            {
                plugin_skill_dirs.push(dir);
            }
        }

        // An agent row survives only while its owning plugin is active. The
        // registry keys agents by plugin id, so this is the same predicate as
        // above applied one level down — not a second rule.
        let subagents = registry
            .list_agents()
            .into_iter()
            .filter(|agent| {
                registry
                    .get_plugin(&agent.plugin_id)
                    .is_some_and(|p| p.status.is_active())
            })
            .filter_map(super::plugin_agent_to_def)
            .collect();

        PluginProjection {
            plugin_skill_dirs,
            subagents,
        }
    }

    /// Re-derive **all** plugin-owned process-global projections and install
    /// them, replacing whatever was published before.
    ///
    /// Publish is replace-semantics (not append), so this doubles as the
    /// retraction path: a plugin that stopped being active simply is not in the
    /// new vector. Called from exactly one place,
    /// `lifecycle.rs::after_transition` (G3 pins it); do not add a second
    /// caller — route the new path through a lifecycle primitive instead.
    ///
    /// Returns the projection that was installed, for logging and tests.
    pub(crate) async fn republish_plugin_projections(&self) -> PluginProjection {
        let mut skill_dirs: Vec<PathBuf> = self
            .discovery
            .discover_skill_dirs()
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.path)
            .collect();

        let projection = self.derive_plugin_projection(&skill_dirs).await;

        skill_dirs.extend(projection.plugin_skill_dirs.iter().cloned());

        // Publish the plugin roots on their own too: `SkillSystem` only feeds
        // the prompt index, while `get_all_skills_dirs` (the `skill_read` /
        // `skill_list` search set) reads this global. Splitting them is what
        // lets a plugin root outside the well-known locations
        // (e.g. `plugins/cache/<market>/<id>/skills`) still be readable.
        crate::utils::paths::publish_plugin_skill_dirs(projection.plugin_skill_dirs.clone());
        crate::agents::publish_plugin_subagents(projection.subagents.clone());

        self.skill_system.init(skill_dirs).await;
        self.refresh_active_plugin_tools().await;

        projection
    }
}

#[cfg(test)]
mod tests {
    //! The census below is the guard that keeps this module a chokepoint.

    /// Source-level census: five needles are pinned each to exactly one
    /// owner file — `publish_plugin_skill_dirs(` and
    /// `publish_plugin_subagents(` may appear only in `projection.rs` (the
    /// two process-global publish calls); `republish_plugin_projections(`,
    /// `sync_hooks_from_registry(`, and `after_transition(` may appear only
    /// in `lifecycle.rs` (after P1.9 the view recomputation has exactly one
    /// trigger, `after_transition`, and every public primitive ends with it
    /// — spec §3.2).
    ///
    /// This has to be a source scan, not a runtime assertion: at runtime a
    /// second author looks exactly like the first one running twice. The
    /// failure mode it prevents is the one that shipped — `load_all` and
    /// `set_plugin_enabled` each deriving the set with a different predicate.
    ///
    /// Comment lines, `#[cfg(test)]` items, and every string/char literal's
    /// payload are stripped before matching (via
    /// `utils::source_scan::production_code_text`, shared with the G1
    /// census in `effects/census.rs`), because a doc comment or a test
    /// fixture that *names* a needle is documentation, not a call. Without
    /// that, this very `G3_PINNED` array — whose tuples are string literals
    /// containing the needle text itself — would self-match its own
    /// `lifecycle.rs`-owned entries as offenders: it lives inside this
    /// `mod tests`, which is itself `#[cfg(test)]`-attributed, so
    /// `production_code_text`'s item-blanking step alone (before it ever
    /// reaches literal payloads) already removes the whole block.
    ///
    /// (needle, the only file allowed to contain a non-definition line with
    /// it). A module-level const on purpose: P6 appends
    /// `("notify_tools_list_changed(", "lifecycle.rs")` when
    /// `after_transition` gains the MCP-face broadcast — one line here, no
    /// test-body edit.
    pub(super) const G3_PINNED: &[(&str, &str)] = &[
        ("publish_plugin_skill_dirs(", "projection.rs"),
        ("publish_plugin_subagents(", "projection.rs"),
        // The view recomputation has one trigger: lifecycle.rs::after_transition.
        ("republish_plugin_projections(", "lifecycle.rs"),
        ("sync_hooks_from_registry(", "lifecycle.rs"),
        ("after_transition(", "lifecycle.rs"),
    ];

    #[test]
    fn publishing_plugin_projections_has_exactly_one_author() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/extension");
        let mut offenders: Vec<String> = Vec::new();
        let mut checked_files = 0usize;
        let mut found: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();

        for (rel, src) in crate::utils::source_scan::rust_sources_under(&root) {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&rel);
            let text = crate::utils::source_scan::production_code_text(&path, &src);
            checked_files += 1;
            let file_name = rel.rsplit('/').next().unwrap_or(&rel);
            for (lineno, line) in text.lines().enumerate() {
                let code = line.trim_start();
                for &(needle, owner) in G3_PINNED {
                    if !code.contains(needle) {
                        continue;
                    }
                    // A definition is not a call.
                    if code.contains(&format!("fn {}", needle.trim_end_matches('('))) {
                        continue;
                    }
                    if file_name == owner {
                        *found.entry(needle).or_default() += 1;
                    } else {
                        offenders.push(format!(
                            "{rel}:{} — `{}` (only {owner} may call this)",
                            lineno + 1,
                            code.trim()
                        ));
                    }
                }
            }
        }

        assert!(
            checked_files > 10,
            "census scanned only {checked_files} files — it is not looking where it thinks it is"
        );
        // Self-check: every pinned call must exist in its owner, or the
        // census is blind to a rename.
        for &(needle, owner) in G3_PINNED {
            assert!(
                found.get(needle).copied().unwrap_or(0) >= 1,
                "expected `{needle}` inside {owner}, found none — did it move? the census is now blind"
            );
        }
        assert!(
            offenders.is_empty(),
            "plugin projections are published only from projection.rs and re-derived only \
             from lifecycle.rs::after_transition (spec §3.2: one trigger after every \
             transition). Second author(s) found:\n  {}",
            offenders.join("\n  ")
        );
    }
}
