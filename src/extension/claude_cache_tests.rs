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
        .join("plugins/cache")
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
    assert_eq!(cc.path, root.display().to_string());
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
    let own = aleph_home.path().join("plugins/dup");
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
    let inventory = manager.hook_executor_snapshot().await.inventory();
    assert!(
        inventory
            .iter()
            .filter(|h| h.source == "hooky")
            .all(|h| h.consent.as_deref() != Some("approved")),
        "enabling a plugin approves none of its hooks: {inventory:?}"
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
