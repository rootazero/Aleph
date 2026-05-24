//! Integration tests for the Watcher Wiring Cycle.
//!
//! Verify that `ExtensionManager::start_watcher_with_dirs` routes
//! `ExtensionChangeType` events to the cheapest-correct reload path:
//!
//! - `HooksConfig` → `sync_user_hooks` only (no full reload).
//! - `Skill`       → no-op (delegated to the wired SkillWatcher).
//! - `Plugin` / `Command` / `Agent` / `Unknown` → full `reload()`.
//!
//! Also verify `InternalWriteTracker` suppresses watcher feedback when
//! Aleph itself marks a path as self-written.
//!
//! These tests target only `reload_count` (cfg(test) field on
//! `ExtensionManager`) so they don't require host `~/.aleph/` state.

use std::sync::Arc;
use std::time::Duration;

use alephcore::extension::{ExtensionConfig, ExtensionManager};
use tempfile::TempDir;

/// Long enough to outlast the watcher's 500ms debounce + a generous fs
/// settle margin on macOS FSEvents (~500-800ms in CI).
const POLL_BUDGET: Duration = Duration::from_millis(2500);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

async fn manager_with_temp_watcher() -> (Arc<ExtensionManager>, TempDir) {
    let mut cfg = ExtensionConfig::default();
    // Keep discovery from leaking host dotfiles into the test.
    cfg.discovery.scan_claude_dirs = false;
    cfg.discovery.scan_project_dirs = false;
    let manager = Arc::new(
        ExtensionManager::new(cfg)
            .await
            .expect("manager construction"),
    );
    let dir = TempDir::new().expect("tempdir");
    manager
        .start_watcher_with_dirs(Some(vec![dir.path().to_path_buf()]), None)
        .await
        .expect("start_watcher");
    // Watcher needs a tick to actually arm its FSEvents/inotify handle
    // before the first write — without this we lose the create event.
    tokio::time::sleep(Duration::from_millis(250)).await;
    (manager, dir)
}

/// Poll until `predicate` returns true or POLL_BUDGET elapses.
async fn wait_until<F: Fn() -> bool>(label: &str, predicate: F) -> bool {
    let deadline = std::time::Instant::now() + POLL_BUDGET;
    loop {
        if predicate() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("wait_until({label}) timed out after {POLL_BUDGET:?}");
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn plugin_change_triggers_full_reload() {
    let (manager, dir) = manager_with_temp_watcher().await;
    let before = manager.reload_count();
    let plugin_path = dir.path().join("plugin.json");
    std::fs::write(&plugin_path, b"{}").expect("write plugin.json");

    let mgr = Arc::clone(&manager);
    let fired = wait_until("reload after plugin.json", || {
        mgr.reload_count() > before
    })
    .await;
    assert!(fired, "Plugin manifest change must trigger ExtensionManager::reload");

    let _ = manager.stop_watcher();
}

#[tokio::test]
async fn skill_change_is_deferred_to_skill_watcher() {
    let (manager, dir) = manager_with_temp_watcher().await;
    let before = manager.reload_count();
    let skills = dir.path().join("skills").join("demo");
    std::fs::create_dir_all(&skills).expect("mkdir skills/demo");
    std::fs::write(skills.join("SKILL.md"), b"# demo").expect("write SKILL.md");

    // Wait the full poll budget — we are asserting the *absence* of a
    // reload, so we must give the system ample time to fire it if it would.
    tokio::time::sleep(POLL_BUDGET).await;
    assert_eq!(
        manager.reload_count(),
        before,
        "SkillWatcher owns SKILL.md reloads; ExtensionWatcher must not double-fire"
    );

    let _ = manager.stop_watcher();
}

#[tokio::test]
async fn hooks_config_routes_to_user_hooks_only() {
    let (manager, dir) = manager_with_temp_watcher().await;
    let before = manager.reload_count();
    let aleph_dir = dir.path().join(".aleph");
    std::fs::create_dir_all(&aleph_dir).expect("mkdir .aleph");
    std::fs::write(
        aleph_dir.join("hooks.json"),
        br#"{"hooks":{"MessageReceived":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
    )
    .expect("write hooks.json");

    tokio::time::sleep(POLL_BUDGET).await;
    assert_eq!(
        manager.reload_count(),
        before,
        "HooksConfig must route through sync_user_hooks, NOT full reload"
    );

    let _ = manager.stop_watcher();
}

#[tokio::test]
async fn internal_write_suppresses_watcher_reload() {
    let (manager, dir) = manager_with_temp_watcher().await;
    let before = manager.reload_count();
    let plugin_path = dir.path().join("plugin.json");

    // Pre-mark the path so the eventual fs write is filtered.
    // Mark BEFORE creating, so canonicalize-fallback uses the raw path —
    // the watcher event will carry the same raw path post-creation.
    manager.mark_self_write(&plugin_path);
    std::fs::write(&plugin_path, b"{}").expect("write plugin.json");

    tokio::time::sleep(POLL_BUDGET).await;
    assert_eq!(
        manager.reload_count(),
        before,
        "InternalWriteTracker must suppress watcher event for self-written paths"
    );

    let _ = manager.stop_watcher();
}
