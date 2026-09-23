//! A Project-scoped plugin is invisible to a session bound to no project and
//! visible to a session bound to that project — end to end, through the real
//! `ProjectStore` and the real discovery walk (`ProjectStore::shared().add`,
//! exactly the producer `collect_plugin_dirs` reads).
//!
//! Faces observed here:
//!   - the registry row's key (`ExtensionManager::plugin_scope_key`) and the
//!     `plugin_visible` predicate every capability face calls, for two
//!     plugins: one whose directory name equals its registry id, and one
//!     whose directory name does not (D-4: `Vis_Other` → `vis-other`, via
//!     `sanitize_plugin_id`, auto-discovered with no manifest file);
//!   - face ③ end to end: sub-agent resolution (`AgentRegistry::resolve`)
//!     for the first plugin's agent, from inside the project and from
//!     another one;
//!   - face ②'s `skill_read` search set end to end
//!     (`utils::paths::get_all_skills_dirs`), for the second (dir-name≠id)
//!     plugin's skill, from inside the project and from another one.
//!
//! NOT observed here: the prompt-index sides of faces ②/③ — the
//! `<available_skills>` and `<available_agents>` snapshots baked into the
//! system prompt (`prompt_build.rs`'s `retain_visible_plugin_skills` call and
//! its `<available_agents>` counterpart) — and face ①/④/⑤'s own request-time
//! wiring (tool index, slash list, MCP join). Those have no fire-site test in
//! this file; face ①/②a/③a/④/⑤ are unit-tested at their own chokepoints
//! (`src/extension/mod.rs`, `src/agents/registry.rs`, `src/mcp/*`), and the
//! prompt-index sides of ②/③ are intended to be observed on a real daemon by
//! the planned `qa/plugins/run.sh visibility` stage (not written as of this
//! task).
//!
//! ONE test in this file on purpose: it pins `ALEPH_HOME` (and `HOME`) for
//! the whole process — `ProjectStore::shared()` and the plugin discovery
//! scanner both resolve those env vars once, at first use, via process-wide
//! statics (`OnceLock` / cached `DirectoryScanner`) — so a second test in the
//! same binary would race on them.

use alephcore::agents::AgentRegistry;
use alephcore::discovery::DiscoveryConfig;
use alephcore::extension::visibility::{canonical_root, ScopeKey, VisibilityCtx};
use alephcore::extension::{ExtensionConfig, ExtensionManager};

/// Plant a Project-scoped plugin whose directory name equals its registry
/// id: `.claude-plugin/plugin.toml` (the preferred, highest-priority
/// manifest format — confirmed against `src/extension/manifest/mod.rs`'s
/// adapter order) declaring `name = "<id>"`, plus one sub-agent under
/// `agents/` (auto-discovered by the loader; a plugin need not declare its
/// component list for the well-known `agents/` directory to be scanned).
fn plant_named_plugin(project_root: &std::path::Path, id: &str) {
    let plugin = project_root.join(".aleph/plugins").join(id);
    std::fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    std::fs::write(
        plugin.join(".claude-plugin/plugin.toml"),
        format!("name = \"{id}\"\nversion = \"1.0.0\"\n"),
    )
    .unwrap();
    std::fs::create_dir_all(plugin.join("agents")).unwrap();
    std::fs::write(
        plugin.join("agents").join(format!("{id}-agent.md")),
        format!("---\nname: {id}-agent\ndescription: scoped helper\n---\nYou help.\n"),
    )
    .unwrap();
}

/// Plant a Project-scoped plugin with NO manifest file at all (auto-discover,
/// `src/extension/manifest/mod.rs::auto_discover_manifest`), whose directory
/// name is deliberately not a valid registry id: `dir_name` sanitizes to the
/// id the caller expects (D-4 — the directory name is not the id). Gives it
/// one skill under `skills/<skill_name>/SKILL.md`, the well-known layout
/// `has_any_component` recognises one level deep.
fn plant_auto_discovered_plugin(project_root: &std::path::Path, dir_name: &str, skill_name: &str) {
    let plugin = project_root.join(".aleph/plugins").join(dir_name);
    let skill_dir = plugin.join("skills").join(skill_name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {skill_name}\ndescription: a planted skill\n---\nbody\n"),
    )
    .unwrap();
}

#[tokio::test]
async fn a_project_plugin_is_invisible_without_the_project_and_visible_inside_it() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    // Point every HOME-derived path (plugins root, data dir → projects.db) at
    // the scratch home. Process-local; this is the only test in this binary.
    std::env::set_var("ALEPH_HOME", home.path());
    std::env::set_var("HOME", home.path());

    // Plugin 1: directory name == registry id.
    plant_named_plugin(project.path(), "vis-proj");
    // Plugin 2 (D-4): directory name != registry id. `Vis_Other` sanitizes to
    // `vis-other` (lowercase; `_` is not `[a-z0-9-]` so it becomes `-`).
    plant_auto_discovered_plugin(project.path(), "Vis_Other", "greeter");

    // `ProjectStore::shared()` opens `<ALEPH_HOME>/data/projects.db` but does
    // NOT create its schema by itself outside `cfg(test)` (that branch is an
    // in-memory db with `create_schema()` called inline) — in a real process
    // this runs once at boot. `migrate()` is the boot-time entry point
    // (schema + optional `projects.json` adoption + roster republish) and is
    // idempotent, so it is safe to call here before the first write.
    let store = alephcore::projects::ProjectStore::shared();
    store.migrate().expect("boot-time schema migration");
    store
        .add(project.path(), Some("vis".into()))
        .expect("register the project the way the Panel picker does");

    let cfg = ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: home.path().to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
        },
        plugins_config_path: Some(home.path().join("plugins.toml")),
    };
    let manager = ExtensionManager::new(cfg).await.expect("manager");
    manager.load_all().await.expect("load_all");

    let inside = VisibilityCtx {
        project_root: Some(canonical_root(project.path())),
    };
    let nowhere = VisibilityCtx { project_root: None };

    // ---- Plugin 1 (dir name == id): row key + predicate --------------------
    let key = manager
        .plugin_scope_key("vis-proj")
        .expect("the project plugin was discovered through the registered project");
    assert_eq!(key, ScopeKey::project(project.path()));
    assert!(manager.plugin_visible("vis-proj", &inside));
    assert!(!manager.plugin_visible("vis-proj", &nowhere));

    // ---- Plugin 1, face ③ end to end: sub-agent resolution -----------------
    let registry = AgentRegistry::with_builtins();
    let elsewhere = tempfile::tempdir().unwrap();
    assert!(
        registry
            .resolve("vis-proj-agent", Some(project.path()))
            .is_some(),
        "inside the project the plugin agent is delegatable"
    );
    assert!(
        registry
            .resolve("vis-proj-agent", Some(elsewhere.path()))
            .is_none(),
        "from another project it does not exist"
    );

    // ---- Plugin 2 (D-4, dir name != id): row key + predicate on the
    //      REGISTRY id, not the directory name -----------------------------
    let key2 = manager
        .plugin_scope_key("vis-other")
        .expect("Vis_Other auto-discovers under its sanitised registry id vis-other");
    assert_eq!(key2, ScopeKey::project(project.path()));
    assert!(manager.plugin_visible("vis-other", &inside));
    assert!(!manager.plugin_visible("vis-other", &nowhere));

    // ---- Plugin 2, face ② end to end: the `skill_read` search set ---------
    let expected_skills_dir =
        canonical_root(&project.path().join(".aleph/plugins/Vis_Other/skills"));

    let dirs_inside = alephcore::utils::paths::get_all_skills_dirs(Some(project.path()))
        .expect("skills dirs inside the project");
    assert!(
        dirs_inside
            .iter()
            .any(|d| canonical_root(d) == expected_skills_dir),
        "vis-other's skills dir must be in the search set from inside its project: {dirs_inside:?}"
    );

    let dirs_elsewhere = alephcore::utils::paths::get_all_skills_dirs(Some(elsewhere.path()))
        .expect("skills dirs from another project");
    assert!(
        !dirs_elsewhere
            .iter()
            .any(|d| canonical_root(d) == expected_skills_dir),
        "vis-other's skills dir must not leak to another project: {dirs_elsewhere:?}"
    );
}
