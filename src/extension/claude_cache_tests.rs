//! `PluginOrigin::ClaudeCache` end to end: Claude Code's
//! `~/.claude/plugins/installed_plugins.json` → discovery → admit → rows.
//!
//! Every test points the manager's Claude root at a tempdir
//! (`DiscoveryConfig::claude_home_override`); none of them reads the real
//! `~/.claude`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::discovery::DiscoveryConfig;
use crate::extension::hooks::ShellHookConsent;
use crate::extension::plugin_state::PluginsConfig;
use crate::extension::visibility::ScopeKey;
use crate::extension::{ExtensionConfig, ExtensionManager, PluginInfo, PluginOrigin};

async fn isolated_manager_with_claude_home(
    dir: &Path,
    claude_home: &Path,
) -> (ExtensionManager, PathBuf) {
    let cfg_path = dir.join("plugins.toml");
    let manager = ExtensionManager::new(ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: dir.to_path_buf(),
            scan_claude_dirs: true,
            scan_project_dirs: false,
            max_upward_depth: 0,
            claude_home_override: Some(claude_home.to_path_buf()),
        },
        plugins_config_path: Some(cfg_path.clone()),
        extra_plugin_parents: vec![],
    })
    .await
    .unwrap();
    (manager, cfg_path)
}

/// `(name, marketplace, version)` rows of `installed_plugins.json`, every one
/// a `user`-scope install.
fn write_index(claude_home: &Path, installs: &[(&str, &str, &str)]) {
    let plugins = installs
        .iter()
        .map(|(name, market, version)| {
            format!(
                r#""{name}@{market}":[{{"scope":"user","installPath":"/elsewhere","version":"{version}","installedAt":"2026-01-01T00:00:00Z","lastUpdated":"2026-01-01T00:00:00Z"}}]"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let dir = claude_home.join("plugins");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("installed_plugins.json"),
        format!(r#"{{"version":2,"plugins":{{{plugins}}}}}"#),
    )
    .unwrap();
}

/// A Claude Code plugin tree at its cache dir, with one command and one
/// command hook. Returns the cache dir.
fn write_cc_plugin(claude_home: &Path, market: &str, name: &str, version: &str) -> PathBuf {
    let root = claude_home
        // Component-wise joins: callers compare the returned path against
        // manager-reported, separator-normalized paths.
        .join("plugins")
        .join("cache")
        .join(market)
        .join(name)
        .join(version);
    std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(root.join("commands")).unwrap();
    std::fs::create_dir_all(root.join("hooks")).unwrap();
    std::fs::write(
        root.join(".claude-plugin/plugin.json"),
        format!(r#"{{"name":"{name}","version":"{version}"}}"#),
    )
    .unwrap();
    std::fs::write(
        root.join("commands/hello.md"),
        "---\ndescription: hi\n---\nSay hi to $ARGUMENTS.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("hooks/hooks.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"echo qa"}]}]}}"#,
    )
    .unwrap();
    root
}

/// Every path under `dir` with its mtime and length.
fn snapshot(dir: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    fn walk(d: &Path, out: &mut Vec<(PathBuf, SystemTime, u64)>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            out.push((p.clone(), m.modified().unwrap(), m.len()));
            if m.is_dir() {
                walk(&p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

fn row<'a>(info: &'a [PluginInfo], name: &str) -> Option<&'a PluginInfo> {
    info.iter().find(|p| p.name == name)
}

#[tokio::test]
async fn a_claude_cache_plugin_loads_disabled_and_never_writes_under_claude_home() {
    let claude_home = tempfile::tempdir().unwrap();
    write_index(claude_home.path(), &[("qa-cc", "qa-market", "1.0.0")]);
    let root = write_cc_plugin(claude_home.path(), "qa-market", "qa-cc", "1.0.0");
    // A stray legacy marker: the migration must NOT remove it here.
    std::fs::write(root.join(".disabled"), "").unwrap();
    let before = snapshot(claude_home.path());

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let scratch = tempfile::tempdir().unwrap();
    let (manager, cfg_path) =
        isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;
    manager.load_all().await.unwrap();
    let info = manager.get_plugin_info().await;
    let cc = row(&info, "qa-cc").expect("discovered");
    assert_eq!(cc.origin, "claude_cache");
    assert_eq!(
        cc.status, "disabled",
        "a CC-installed plugin is off until Aleph is told otherwise"
    );
    assert!(!cc.enabled);
    assert_eq!(
        std::path::Path::new(&cc.path)
            .canonicalize()
            .unwrap_or_default(),
        root.canonicalize().unwrap_or_default(),
        "the registry should record the path it walked, not a normalized copy"
    );
    // Nobody disabled it: the row must not say an operator did (判据 §17).
    let detail = cc.error.as_deref().unwrap_or_default();
    assert!(
        detail.contains("installed by Claude Code") && detail.contains("not enabled in Aleph"),
        "{detail:?}"
    );
    assert!(!detail.contains("by the operator"), "{detail:?}");
    assert!(
        !PluginsConfig::load(&cfg_path).entries.contains_key("qa-cc"),
        "loading records no preference: the foreign marker is not migrated into a disable"
    );

    assert!(manager.set_plugin_enabled("qa-cc", true).await);
    let info = manager.get_plugin_info().await;
    assert!(row(&info, "qa-cc").unwrap().enabled);
    // The one bit Aleph owns lives in Aleph's own document, and survives a
    // fresh load.
    assert!(PluginsConfig::load(&cfg_path).is_enabled_for("qa-cc", PluginOrigin::ClaudeCache));
    manager.load_all().await.unwrap();
    let info = manager.get_plugin_info().await;
    assert_eq!(row(&info, "qa-cc").unwrap().status, "loaded");

    assert_eq!(
        snapshot(claude_home.path()),
        before,
        "nothing under ~/.claude may change"
    );
    assert!(
        root.join(".disabled").exists(),
        "the legacy-marker migration must not touch a foreign tree"
    );
}

/// Controller ruling 2 — one id, two roots. The higher-priority root wins the
/// id (`~/.aleph` 10 > ClaudeCache 5; `discover_and_mount` walks highest
/// first); the loser is not a row. `plugins.toml` is keyed by id, so the
/// preference belongs to whichever copy currently holds the id: an explicit
/// `false` follows the id to the Claude Code copy when the Aleph copy goes
/// away (fail closed), and so does an explicit `true` (the documented
/// residual: the opt-in is to an id, as it is across a version update).
#[tokio::test]
async fn one_id_in_aleph_home_and_the_claude_cache_is_one_row_the_aleph_copy() {
    let aleph_home = tempfile::tempdir().unwrap();
    // Component-wise joins: this path is compared against the manager's
    // separator-normalized `PluginInfo.path` below, and `join("a/b")` keeps
    // the forward slash verbatim on Windows.
    let own = aleph_home.path().join("plugins").join("dup");
    std::fs::create_dir_all(own.join(".claude-plugin")).unwrap();
    std::fs::write(own.join(".claude-plugin/plugin.json"), r#"{"name":"dup"}"#).unwrap();
    let _home = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(aleph_home.path());

    let claude_home = tempfile::tempdir().unwrap();
    write_index(claude_home.path(), &[("dup", "m", "1.0.0")]);
    let cached = write_cc_plugin(claude_home.path(), "m", "dup", "1.0.0");

    let scratch = tempfile::tempdir().unwrap();
    let (manager, _cfg) =
        isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;
    let summary = manager.load_all().await.unwrap();
    assert_eq!(summary.shadowed, 1, "the Claude Code copy lost the id");
    let info = manager.get_plugin_info().await;
    let rows: Vec<&PluginInfo> = info.iter().filter(|p| p.name == "dup").collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].origin, "global");
    assert_eq!(rows[0].path, own.display().to_string());
    assert_eq!(rows[0].status, "loaded");

    // Disable the id, then lose the Aleph copy outside `uninstall` (which
    // would have forgotten the preference): the Claude Code copy now holds
    // the id, and the explicit `false` holds for it too.
    assert!(manager.set_plugin_enabled("dup", false).await);
    std::fs::remove_dir_all(&own).unwrap();
    manager.load_all().await.unwrap();
    let info = manager.get_plugin_info().await;
    let dup = row(&info, "dup").unwrap();
    assert_eq!(
        (dup.origin.as_str(), dup.path.clone(), dup.status.as_str()),
        ("claude_cache", cached.display().to_string(), "disabled")
    );

    // The residual, pinned so a change to it is a decision: an explicit
    // `true` is an opt-in to the id.
    assert!(manager.set_plugin_enabled("dup", true).await);
    let info = manager.get_plugin_info().await;
    assert_eq!(row(&info, "dup").unwrap().status, "loaded");
}

/// Controller ruling 3 — a Claude Code plugin's hooks are rooted at its
/// VERSIONED cache dir, so consent (bound to the root) does not carry an
/// approval to the bytes of an update: Claude Code writes the new version to
/// a new dir, the hook's root moves, and the old approval stops answering.
/// Enabling approves nothing.
#[tokio::test]
async fn a_claude_code_update_moves_the_hook_root_and_the_approval_stays_behind() {
    let claude_home = tempfile::tempdir().unwrap();
    write_index(claude_home.path(), &[("hooky", "m", "1.0.0")]);
    let v1 = write_cc_plugin(claude_home.path(), "m", "hooky", "1.0.0");

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let scratch = tempfile::tempdir().unwrap();
    let (manager, _cfg) =
        isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;
    manager.load_all().await.unwrap();
    assert!(manager.set_plugin_enabled("hooky", true).await);

    let hook_roots = |registry: &crate::extension::PluginRegistry| -> Vec<Option<PathBuf>> {
        registry
            .list_hooks()
            .into_iter()
            .filter(|h| h.plugin_id == "hooky")
            .map(|h| h.plugin_root.clone())
            .collect()
    };
    let roots = hook_roots(&*manager.get_plugin_registry().await);
    assert_eq!(
        roots,
        vec![Some(v1.clone())],
        "rooted at the versioned cache dir"
    );
    // Enabling approves nothing: the hook is there, gated, and pending. An
    // empty filter, or a snapshot with no consent gate (`consent: None`),
    // would satisfy "nothing approved" without testing it.
    let inventory = manager.hook_executor_snapshot().await.inventory();
    let hooky: Vec<Option<&str>> = inventory
        .iter()
        .filter(|h| h.source == "hooky")
        .map(|h| h.consent.as_deref())
        .collect();
    assert!(
        !hooky.is_empty(),
        "the plugin's hook must be live: {inventory:?}"
    );
    assert!(
        hooky.iter().all(|c| *c == Some("pending")),
        "enabling a plugin approves none of its hooks: {hooky:?}"
    );

    // The operator reviews and approves the hook as it runs from v1.
    let state = tempfile::tempdir().unwrap();
    let consent = ShellHookConsent::with_path(state.path().join("allowlist.json"));
    consent.record_pending("hooky", &ScopeKey::Global, "echo qa", "PreToolUse", &v1);
    let entry = consent.entries().remove(0);
    consent
        .approve(&entry.fingerprint, entry.plugin_root.as_deref())
        .unwrap();
    assert!(consent.is_approved("hooky", &ScopeKey::Global, &v1, "echo qa"));

    // Claude Code updates the plugin: a new version dir, the index moved.
    let v2 = write_cc_plugin(claude_home.path(), "m", "hooky", "1.1.0");
    write_index(claude_home.path(), &[("hooky", "m", "1.1.0")]);
    manager.reload().await.unwrap();
    let roots = hook_roots(&*manager.get_plugin_registry().await);
    assert_eq!(
        roots,
        vec![Some(v2.clone())],
        "the root moved with the update"
    );
    assert!(
        !consent.is_approved("hooky", &ScopeKey::Global, &v2, "echo qa"),
        "an approval of v1 must not run v2"
    );
}

/// Controller ruling 5 — an unreadable index is "unknown", not "none". The
/// rows it named are not listed for that load (the registry is rebuilt from
/// disk), but nothing is forgotten: the preference survives, and the plugin
/// comes back enabled the moment the index reads again, with no new verb.
#[tokio::test]
async fn an_unreadable_index_forgets_nothing() {
    let claude_home = tempfile::tempdir().unwrap();
    write_index(claude_home.path(), &[("keep", "m", "1.0.0")]);
    write_cc_plugin(claude_home.path(), "m", "keep", "1.0.0");

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let scratch = tempfile::tempdir().unwrap();
    let (manager, cfg_path) =
        isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;
    manager.load_all().await.unwrap();
    assert!(manager.set_plugin_enabled("keep", true).await);

    let index = claude_home.path().join("plugins/installed_plugins.json");
    std::fs::write(&index, "{ not json").unwrap();
    manager.load_all().await.unwrap();
    assert!(row(&manager.get_plugin_info().await, "keep").is_none());
    assert!(
        PluginsConfig::load(&cfg_path).is_enabled_for("keep", PluginOrigin::ClaudeCache),
        "an unknown index must not cost the operator their opt-in"
    );

    write_index(claude_home.path(), &[("keep", "m", "1.0.0")]);
    manager.load_all().await.unwrap();
    assert_eq!(
        row(&manager.get_plugin_info().await, "keep")
            .unwrap()
            .status,
        "loaded"
    );
}

/// A Claude Code dir's leaf is its VERSION, shared by many plugins. The
/// parse-error row for such a dir is named after the plugin (the version
/// dir's parent), so two broken plugins at `1.0.0` are two rows — not one
/// row called `1.0.0` and a second that vanished.
#[tokio::test]
async fn two_broken_claude_code_plugins_at_one_version_are_two_error_rows() {
    let claude_home = tempfile::tempdir().unwrap();
    write_index(
        claude_home.path(),
        &[("broken-a", "m", "1.0.0"), ("broken-b", "m", "1.0.0")],
    );
    for name in ["broken-a", "broken-b"] {
        let root = write_cc_plugin(claude_home.path(), "m", name, "1.0.0");
        std::fs::write(root.join(".claude-plugin/plugin.json"), "{ not json").unwrap();
    }

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let scratch = tempfile::tempdir().unwrap();
    let (manager, _cfg) =
        isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;
    manager.load_all().await.unwrap();
    let info = manager.get_plugin_info().await;
    let mut errors: Vec<(&str, &str)> = info
        .iter()
        .filter(|p| p.status == "error")
        .map(|p| (p.name.as_str(), p.origin.as_str()))
        .collect();
    errors.sort_unstable();
    assert_eq!(
        errors,
        vec![("broken-a", "claude_cache"), ("broken-b", "claude_cache")]
    );
}

/// Fix round 1, F1 — the human faces resolve the id in the REGISTRY, not in
/// `default_plugins_dir()`: `plugins.enable` / `plugins.disable` (the RPC the
/// CLI's `aleph plugin enable|disable` and the Panel toggle both call) turn a
/// Claude Code install on and off. An id nothing discovered is still refused.
#[tokio::test]
async fn the_rpc_face_enables_and_disables_a_claude_cache_row() {
    use crate::gateway::handlers::plugins::set_enabled_via_registry;

    let claude_home = tempfile::tempdir().unwrap();
    write_index(claude_home.path(), &[("rpc-cc", "m", "1.0.0")]);
    write_cc_plugin(claude_home.path(), "m", "rpc-cc", "1.0.0");
    let before = snapshot(claude_home.path());

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let scratch = tempfile::tempdir().unwrap();
    let (manager, cfg_path) =
        isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;

    let on = set_enabled_via_registry(&manager, None, "rpc-cc", true).await;
    assert!(on.is_success(), "{on:?}");
    let info = manager.get_plugin_info().await;
    assert_eq!(row(&info, "rpc-cc").unwrap().status, "loaded");

    let off = set_enabled_via_registry(&manager, None, "rpc-cc", false).await;
    assert!(off.is_success(), "{off:?}");
    let info = manager.get_plugin_info().await;
    assert_eq!(row(&info, "rpc-cc").unwrap().status, "disabled");
    assert!(!PluginsConfig::load(&cfg_path).is_enabled_for("rpc-cc", PluginOrigin::ClaudeCache));

    let unknown = set_enabled_via_registry(&manager, None, "nothing-found", true).await;
    assert!(unknown.is_error(), "{unknown:?}");
    assert!(!PluginsConfig::load(&cfg_path)
        .entries
        .contains_key("nothing-found"));
    assert_eq!(
        snapshot(claude_home.path()),
        before,
        "nothing under ~/.claude may change"
    );
}

/// Fix round 1, F1 — the offline `aleph-server plugin enable|disable` asks
/// the same walk `load_all` makes, minus every side effect: a Claude Code id
/// is found, nothing is mounted, and nothing under the Claude home changes.
#[tokio::test]
async fn read_only_discovery_names_a_claude_cache_id_without_loading_it() {
    let claude_home = tempfile::tempdir().unwrap();
    write_index(claude_home.path(), &[("cli-cc", "m", "1.0.0")]);
    write_cc_plugin(claude_home.path(), "m", "cli-cc", "1.0.0");
    let before = snapshot(claude_home.path());

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let scratch = tempfile::tempdir().unwrap();
    let (manager, _cfg) =
        isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;
    let ids = manager.discovered_plugin_ids().unwrap();
    assert!(
        ids.contains(&("cli-cc".to_string(), PluginOrigin::ClaudeCache)),
        "{ids:?}"
    );
    assert!(
        manager.get_plugin_info().await.is_empty(),
        "discovery alone registers nothing"
    );
    assert_eq!(snapshot(claude_home.path()), before);
}

/// Fix round 1, D-A (provisional ruling) — the MODEL face may turn a Claude
/// Code install off, never on: enabling runs code another tool installed, so
/// `plugin_manage` answers with the human command and writes nothing. An id
/// that nothing discovered is refused too, or the model could pre-record an
/// opt-in for a Claude Code plugin that is not listed yet.
#[tokio::test]
async fn the_model_may_disable_but_not_enable_a_claude_cache_row() {
    use crate::builtin_tools::plugin_manage::{PluginAction, PluginManageArgs, PluginManageTool};
    use crate::gateway::handlers::plugins::set_enabled_via_registry;

    let claude_home = tempfile::tempdir().unwrap();
    write_index(claude_home.path(), &[("model-cc", "m", "1.0.0")]);
    write_cc_plugin(claude_home.path(), "m", "model-cc", "1.0.0");

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let scratch = tempfile::tempdir().unwrap();
    let (manager, cfg_path) =
        isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;
    manager.load_all().await.unwrap();
    let args = |action, name: &str| PluginManageArgs {
        action,
        name: Some(name.to_string()),
        source: None,
        query: None,
        config: None,
        enforce: None,
    };

    let refused = PluginManageTool::call_on(&manager, args(PluginAction::Enable, "model-cc"))
        .await
        .expect_err("the model must not enable a Claude Code install")
        .to_string();
    assert!(
        refused.contains("aleph plugin enable model-cc"),
        "{refused}"
    );
    // A deliberate refusal wears the policy label, not a config/DB one — and,
    // wrapped as the tool adapter wraps it for the model, reads as a policy
    // refusal with no route to another tool.
    assert!(refused.starts_with("Permission denied: "), "{refused}");
    assert!(!refused.contains("Configuration/Database"), "{refused}");
    let as_model_reads_it = crate::tools::service::ToolError::Execution {
        name: "plugin_manage".to_string(),
        cause: refused.clone(),
    };
    assert_eq!(
        as_model_reads_it.kind(),
        crate::tools::error_kind::ToolErrorKind::Permission
    );
    assert_eq!(
        crate::tools::fallback_registry::render_persistence_hint(
            &as_model_reads_it,
            "plugin_manage"
        ),
        ""
    );
    assert!(!PluginsConfig::load(&cfg_path)
        .entries
        .contains_key("model-cc"));
    let info = manager.get_plugin_info().await;
    assert_eq!(row(&info, "model-cc").unwrap().status, "disabled");

    // F3 on the mount face: a reload of the default-off row says why without
    // blaming an operator.
    let reload = PluginManageTool::call_on(&manager, args(PluginAction::Reload, "model-cc"))
        .await
        .expect_err("a plugin that is not enabled does not mount")
        .to_string();
    assert!(reload.contains("not enabled in Aleph"), "{reload}");
    assert!(!reload.contains("by the operator"), "{reload}");

    let pre = PluginManageTool::call_on(&manager, args(PluginAction::Enable, "not-listed-yet"))
        .await
        .expect_err("an id nothing discovered cannot be pre-enabled");
    assert!(pre.to_string().contains("not-listed-yet"), "{pre}");
    assert!(!PluginsConfig::load(&cfg_path)
        .entries
        .contains_key("not-listed-yet"));

    // The operator turns it on; the model may turn it off.
    assert!(set_enabled_via_registry(&manager, None, "model-cc", true)
        .await
        .is_success());
    PluginManageTool::call_on(&manager, args(PluginAction::Disable, "model-cc"))
        .await
        .expect("disabling is the fail-safe direction");
    let info = manager.get_plugin_info().await;
    assert_eq!(row(&info, "model-cc").unwrap().status, "disabled");
    assert!(!PluginsConfig::load(&cfg_path).is_enabled_for("model-cc", PluginOrigin::ClaudeCache));
}

/// Fix round 1, F2 (ruling 5 on the usage face) — while Claude Code's index
/// cannot be read, its plugins are UNKNOWN: the usage inventory says the
/// plugin kind is unavailable, no `plugin:<id>` row is declared an orphan,
/// and `forget_orphans` deletes nothing. The signal is the one read discovery
/// made, carried with that load's rows.
#[tokio::test]
async fn an_unreadable_index_keeps_the_usage_history() {
    use crate::tools::usage::report::{add_extension_inventory, forget_orphans, ExtensionKind};
    use crate::tools::usage::store::ToolUsageStore;
    use crate::tools::usage::{build_report, UsageInventory};

    let claude_home = tempfile::tempdir().unwrap();
    write_index(claude_home.path(), &[("used", "m", "1.0.0")]);
    write_cc_plugin(claude_home.path(), "m", "used", "1.0.0");

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let scratch = tempfile::tempdir().unwrap();
    let store = ToolUsageStore::at(scratch.path().join("tool_usage.json"));
    let (manager, _cfg) =
        isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;
    manager.load_all().await.unwrap();
    assert!(manager.set_plugin_enabled("used", true).await);
    store.record_call("plugin:used", "some_tool", true);

    std::fs::write(
        claude_home.path().join("plugins/installed_plugins.json"),
        "{ not json",
    )
    .unwrap();
    manager.load_all().await.unwrap();

    let mut inventory = UsageInventory::default();
    add_extension_inventory(&manager, &mut inventory).await;
    assert_eq!(inventory.unavailable, vec![ExtensionKind::Plugin]);
    let report = build_report(&inventory, &store.snapshot(), chrono::Utc::now());
    assert!(report.orphans.is_empty(), "{:?}", report.orphans);
    assert_eq!(forget_orphans(&report, &store), 0);
    assert!(store.snapshot().contains_key("plugin:used"));

    // Readable again: the plugin claims its own row, and the kind is whole.
    write_index(claude_home.path(), &[("used", "m", "1.0.0")]);
    manager.load_all().await.unwrap();
    let mut inventory = UsageInventory::default();
    add_extension_inventory(&manager, &mut inventory).await;
    assert!(
        inventory.unavailable.is_empty(),
        "{:?}",
        inventory.unavailable
    );
    let report = build_report(&inventory, &store.snapshot(), chrono::Utc::now());
    assert!(report.orphans.is_empty(), "{:?}", report.orphans);
    assert!(report.entries.iter().any(|e| e.id == "used"));
}
