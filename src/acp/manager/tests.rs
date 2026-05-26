use super::persistence::acp_sessions_path_for_test;
use super::session_key::normalize_path;
use super::*;
use crate::acp::adapter::AdapterMode;
use crate::config::types::acp::AcpAdapterEntry;
use std::collections::HashMap;

#[tokio::test]
async fn test_manager_registers_harnesses() {
    let manager = AcpAdapterManager::new();
    let ids = manager.harness_ids().await;
    assert!(ids.contains(&"claude-code".to_string()));
    assert!(ids.contains(&"codex".to_string()));
    assert!(ids.contains(&"gemini".to_string()));
}

#[tokio::test]
async fn test_manager_has_harness() {
    let manager = AcpAdapterManager::new();
    assert!(manager.has_harness("claude-code").await);
    assert!(!manager.has_harness("unknown").await);
}

#[tokio::test]
async fn test_manager_disable_harness() {
    let mut entries: HashMap<String, AcpAdapterEntry> =
        AcpAdapterEntry::all_presets().into_iter().collect();
    entries.get_mut("codex").unwrap().enabled = false;
    let manager = AcpAdapterManager::from_entries(entries);
    assert!(!manager.has_harness("codex").await);
    assert!(manager.has_harness("claude-code").await);
    assert!(manager.has_harness("gemini").await);
}

#[tokio::test]
async fn test_manager_display_name() {
    let manager = AcpAdapterManager::new();
    assert_eq!(
        manager.display_name("claude-code").await,
        Some("Claude Code".to_string())
    );
    assert_eq!(
        manager.display_name("codex").await,
        Some("Codex".to_string())
    );
    assert_eq!(
        manager.display_name("gemini").await,
        Some("Gemini".to_string())
    );
    assert_eq!(manager.display_name("unknown").await, None);
}

#[tokio::test]
async fn test_manager_harness_modes() {
    let manager = AcpAdapterManager::new();
    assert_eq!(
        manager.harness_mode("gemini").await,
        Some(AdapterMode::NativeAcp)
    );
    assert_eq!(
        manager.harness_mode("claude-code").await,
        Some(AdapterMode::Oneshot)
    );
    assert_eq!(
        manager.harness_mode("codex").await,
        Some(AdapterMode::Oneshot)
    );
}

#[tokio::test]
async fn test_manager_executable_override() {
    let mut entries: HashMap<String, AcpAdapterEntry> =
        AcpAdapterEntry::all_presets().into_iter().collect();
    entries.get_mut("claude-code").unwrap().executable = Some("/custom/claude".to_string());
    let manager = AcpAdapterManager::from_entries(entries);
    assert!(manager.has_harness("claude-code").await);
    let adapters = manager.adapters.read().await;
    let harness = adapters.get("claude-code").unwrap();
    let cfg = harness.build_config(None);
    assert_eq!(cfg.executable, "/custom/claude");
}

#[tokio::test]
async fn test_from_entries_with_custom_harness() {
    let mut entries = HashMap::new();
    entries.insert(
        "my-tool".to_string(),
        AcpAdapterEntry {
            display_name: "My Tool".to_string(),
            executable: Some("my-tool-bin".to_string()),
            enabled: true,
            ..Default::default()
        },
    );

    let manager = AcpAdapterManager::from_entries(entries);
    assert!(manager.has_harness("my-tool").await);
    assert_eq!(
        manager.display_name("my-tool").await,
        Some("My Tool".to_string())
    );
}

#[tokio::test]
async fn test_from_entries_skips_disabled() {
    let mut entries = HashMap::new();
    entries.insert(
        "disabled-tool".to_string(),
        AcpAdapterEntry {
            display_name: "Disabled".to_string(),
            enabled: false,
            ..Default::default()
        },
    );

    let manager = AcpAdapterManager::from_entries(entries);
    assert!(!manager.has_harness("disabled-tool").await);
}

#[tokio::test]
async fn test_register_harness() {
    let manager = AcpAdapterManager::from_entries(HashMap::new());
    let entry = AcpAdapterEntry {
        display_name: "New Tool".to_string(),
        executable: Some("new-tool".to_string()),
        enabled: true,
        ..Default::default()
    };

    manager
        .register_harness("new-tool".to_string(), entry)
        .await
        .unwrap();
    assert!(manager.has_harness("new-tool").await);
    assert_eq!(
        manager.display_name("new-tool").await,
        Some("New Tool".to_string())
    );
}

#[tokio::test]
async fn test_register_duplicate_fails() {
    let manager = AcpAdapterManager::new();
    let entry = AcpAdapterEntry {
        display_name: "Dup".to_string(),
        enabled: true,
        preset: Some("claude-code".to_string()),
        ..Default::default()
    };

    let result = manager
        .register_harness("claude-code".to_string(), entry)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_unregister_custom_harness() {
    let manager = AcpAdapterManager::from_entries(HashMap::new());
    let entry = AcpAdapterEntry {
        display_name: "Temp".to_string(),
        executable: Some("temp".to_string()),
        enabled: true,
        ..Default::default()
    };

    manager
        .register_harness("temp".to_string(), entry)
        .await
        .unwrap();
    assert!(manager.has_harness("temp").await);

    manager.unregister_harness("temp").await.unwrap();
    assert!(!manager.has_harness("temp").await);
}

#[tokio::test]
async fn test_unregister_preset_fails() {
    let manager = AcpAdapterManager::new();
    let result = manager.unregister_harness("claude-code").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_update_harness() {
    let manager = AcpAdapterManager::new();
    let updated = AcpAdapterEntry {
        display_name: "Claude Code Updated".to_string(),
        executable: Some("/new/path/claude".to_string()),
        enabled: true,
        preset: Some("claude-code".to_string()),
        ..Default::default()
    };

    manager
        .update_harness("claude-code", updated)
        .await
        .unwrap();

    let config = manager.get_config("claude-code").await.unwrap();
    assert_eq!(config.executable, Some("/new/path/claude".to_string()));
}

#[tokio::test]
async fn test_update_harness_disable() {
    let manager = AcpAdapterManager::new();
    assert!(manager.has_harness("codex").await);

    let disabled = AcpAdapterEntry {
        display_name: "Codex".to_string(),
        enabled: false,
        preset: Some("codex".to_string()),
        ..Default::default()
    };

    manager.update_harness("codex", disabled).await.unwrap();
    assert!(!manager.has_harness("codex").await);
}

#[tokio::test]
async fn test_list_configs() {
    let manager = AcpAdapterManager::new();
    let configs = manager.list_configs().await;
    let preset_ids = crate::config::types::acp::AcpAdapterEntry::preset_ids();
    assert_eq!(configs.len(), preset_ids.len());
    // Should be sorted
    for i in 1..configs.len() {
        assert!(
            configs[i - 1].0 <= configs[i].0,
            "configs should be sorted by id"
        );
    }
    for id in preset_ids {
        assert!(
            configs.iter().any(|(k, _)| k == id),
            "missing preset config: {}",
            id
        );
    }
}

#[tokio::test]
async fn test_get_config() {
    let manager = AcpAdapterManager::new();
    let config = manager.get_config("claude-code").await;
    assert!(config.is_some());
    assert_eq!(config.unwrap().display_name, "Claude Code");

    let config = manager.get_config("nonexistent").await;
    assert!(config.is_none());
}

#[test]
fn test_acp_sessions_path() {
    let path = acp_sessions_path_for_test();
    assert!(path.to_string_lossy().contains("acp_sessions.json"));
}

#[test]
fn test_session_key_canonicalization() {
    let k1 = SessionKey::new("claude-code", "/tmp");
    let k2 = SessionKey::new("claude-code", "/tmp/");
    assert_eq!(k1, k2);
}

#[test]
fn test_session_key_different_cwd() {
    let k1 = SessionKey::new("claude-code", "/tmp");
    let k2 = SessionKey::new("claude-code", "/var");
    assert_ne!(k1, k2);
}

#[test]
fn test_session_key_different_harness() {
    let k1 = SessionKey::new("claude-code", "/tmp");
    let k2 = SessionKey::new("codex", "/tmp");
    assert_ne!(k1, k2);
}

#[test]
fn test_normalize_path_removes_dot() {
    let path = std::path::Path::new("/tmp/./foo");
    let normalized = normalize_path(path);
    assert_eq!(normalized, std::path::PathBuf::from("/tmp/foo"));
}

#[test]
fn test_normalize_path_resolves_dotdot() {
    let path = std::path::Path::new("/tmp/foo/../bar");
    let normalized = normalize_path(path);
    assert_eq!(normalized, std::path::PathBuf::from("/tmp/bar"));
}

#[test]
fn test_normalize_path_preserves_relative() {
    let path = std::path::Path::new("foo/../bar");
    let normalized = normalize_path(path);
    assert_eq!(normalized, std::path::PathBuf::from("bar"));
}

#[test]
fn test_normalize_path_empty_becomes_dot() {
    let path = std::path::Path::new(".");
    let normalized = normalize_path(path);
    assert_eq!(normalized, std::path::PathBuf::from("."));
}

// ── Phase 1.5: named sessions ────────────────────────────────────────

#[test]
fn test_session_key_default_name_matches_legacy() {
    let legacy = SessionKey::new("claude-code", "/tmp");
    let named = SessionKey::with_name("claude-code", "/tmp", None);
    assert_eq!(legacy, named, "None name must equal legacy new()");
    assert_eq!(legacy.name(), "");
}

#[test]
fn test_session_key_distinguishes_names_same_cwd() {
    let backend = SessionKey::with_name("claude-code", "/tmp", Some("backend"));
    let frontend = SessionKey::with_name("claude-code", "/tmp", Some("frontend"));
    let default = SessionKey::with_name("claude-code", "/tmp", None);
    assert_ne!(backend, frontend, "different names → different keys");
    assert_ne!(backend, default, "named != unnamed");
    assert_eq!(backend.name(), "backend");
    assert_eq!(frontend.name(), "frontend");
}

#[test]
fn test_session_key_accessors() {
    let key = SessionKey::with_name("claude-code", "/tmp", Some("alpha"));
    assert_eq!(key.harness_id(), "claude-code");
    assert_eq!(key.name(), "alpha");
    assert!(key.cwd().to_string_lossy().contains("tmp"));
}

// ── Phase 2: SessionSnapshot for panel ───────────────────────────────

#[tokio::test]
async fn test_list_sessions_empty_pool() {
    let manager = AcpAdapterManager::new();
    let snaps = manager.list_sessions().await;
    assert!(snaps.is_empty(), "no sessions yet → empty snapshot list");
}

// ── Phase 3 follow-ups: persistence + gateway broadcast ──────────────

/// `PersistedAcpSession` deserializes legacy snapshots that lack the
/// `session_name` field (treats them as the default unnamed session).
#[test]
fn test_persisted_session_legacy_compat() {
    let legacy = r#"{
        "harness_id": "claude-code",
        "acp_session_id": "abc123",
        "cwd": "/tmp/repo",
        "created_at": "2026-05-24T00:00:00Z",
        "last_used_at": "2026-05-24T00:00:00Z"
    }"#;
    let parsed: crate::acp::session::PersistedAcpSession =
        serde_json::from_str(legacy).expect("legacy snapshot must parse");
    assert_eq!(parsed.session_name, None);
}

/// `set_gateway_change_hook` fires every time `emit_persistence_event`
/// fans out — confirms the broadcast wiring used by `acp.sessions.changed`.
#[tokio::test]
async fn test_gateway_change_hook_fires_on_emit() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let manager = AcpAdapterManager::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();
    manager
        .set_gateway_change_hook(Arc::new(move || {
            c2.fetch_add(1, Ordering::SeqCst);
        }))
        .await;

    // Two direct emits — fan-out should fire the hook each time even
    // without a persistence_hook installed.
    manager
        .emit_persistence_event(crate::acp::AcpSessionEvent::Created {
            harness_id: "claude-code".to_string(),
            acp_session_id: "s1".to_string(),
            cwd: "/tmp/a".to_string(),
            session_name: None,
        })
        .await;
    manager
        .emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
            harness_id: "claude-code".to_string(),
            cwd: "/tmp/a".to_string(),
            session_name: Some("backend".to_string()),
        })
        .await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}
