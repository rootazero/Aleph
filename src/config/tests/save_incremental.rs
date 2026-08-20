//! Incremental save tests (fix config loss during migration)

use super::super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_save_incremental_preserves_other_sections() {
    // Create temp directory for test
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Create initial config with custom provider
    let initial_toml = r##"
[general]
default_provider = "my_provider"

[providers.my_provider]
api_key = "sk-secret-key-123"
model = "gpt-4o"
base_url = "https://my-api.example.com/v1"
color = "#ff0000"
timeout_seconds = 60
enabled = true

[[rules]]
regex = "^/custom"
provider = "my_provider"
system_prompt = "Custom assistant"

[memory]
enabled = true
similarity_threshold = 0.5
"##;

    fs::write(&config_path, initial_toml).expect("Should write initial config");

    // Load config and test save_incremental
    let config = Config {
        behavior: Some(BehaviorConfig::default()),
        ..Config::default()
    };

    // Read existing content
    let existing_content = fs::read_to_string(&config_path).expect("Should read");
    let mut existing: toml::Value = toml::from_str(&existing_content).expect("Should parse");
    let current: toml::Value = toml::Value::try_from(&config).expect("Should serialize");

    // Only update behavior section
    if let (toml::Value::Table(ref mut existing_table), toml::Value::Table(ref current_table)) =
        (&mut existing, &current)
    {
        if let Some(behavior) = current_table.get("behavior") {
            existing_table.insert("behavior".to_string(), behavior.clone());
        }
    }

    // Write back
    let new_content = toml::to_string_pretty(&existing).expect("Should serialize");
    fs::write(&config_path, &new_content).expect("Should write");

    // Verify: Load the config and check that original sections are preserved
    let final_content = fs::read_to_string(&config_path).expect("Should read final");

    // Verify original provider is preserved
    assert!(
        final_content.contains("my_provider"),
        "Provider name should be preserved"
    );
    assert!(
        final_content.contains("sk-secret-key-123"),
        "API key should be preserved"
    );
    assert!(
        final_content.contains("my-api.example.com"),
        "Base URL should be preserved"
    );

    // Verify original rule is preserved
    assert!(
        final_content.contains("/custom"),
        "Custom rule should be preserved"
    );

    // Verify behavior section was added
    assert!(
        final_content.contains("[behavior]"),
        "Behavior section should be added"
    );

    // Verify memory config is preserved
    assert!(
        final_content.contains("similarity_threshold = 0.5"),
        "Memory config should be preserved"
    );
}


/// Removing the *last* entry of a collection must reach the file.
///
/// `plugin_marketplaces` is `skip_serializing_if = "HashMap::is_empty"`, so an
/// emptied map does not appear in the serialised current config at all — and
/// `merge_sections` treats a section it cannot find as "the caller did not
/// mean this one", warns, and skips. That fail-soft is deliberate (the guards
/// above exist because a partially-populated `Config` once erased on-disk
/// providers), which is why the fix belongs on the field rather than in the
/// merge: the section must still be *emitted* when empty, so "empty" is a
/// value the merge can write instead of an absence it has to interpret.
///
/// Without it, `plugin.marketplace.remove` answers `{"ok": true}`, logs a
/// `warn!` nobody reads, and leaves the registration exactly where it was —
/// on every surface, since the RPC handler and the `aleph-server plugin
/// marketplace remove` subcommand both persist this way.
#[test]
fn removing_the_last_plugin_marketplace_reaches_the_file() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    let mut config = Config::default();
    config.plugin_marketplaces.insert(
        "third-party".to_string(),
        PluginMarketplaceEntry {
            source: "owner/repo".to_string(),
            source_type: "github".to_string(),
        },
    );
    config
        .save_to_file(&config_path)
        .expect("initial save should work");
    let on_disk = fs::read_to_string(&config_path).expect("read back");
    assert!(
        on_disk.contains("third-party"),
        "the fixture must actually register one, or the removal below proves          nothing: {on_disk}"
    );

    // What `handle_marketplace_remove` does: load, drop the key, persist the
    // one section.
    config.plugin_marketplaces.remove("third-party");
    config
        .save_incremental_to_file(&config_path, &["plugin_marketplaces"])
        .expect("incremental save should work");

    let after = fs::read_to_string(&config_path).expect("read back");
    assert!(
        !after.contains("third-party"),
        "the removal was reported as successful but never reached disk, so the \
         next `Config::load()` brings the marketplace back: {after}"
    );

    let reloaded: Config = toml::from_str(&after).expect("still valid TOML");
    assert!(
        reloaded.plugin_marketplaces.is_empty(),
        "reloaded config still holds {:?}",
        reloaded.plugin_marketplaces.keys().collect::<Vec<_>>()
    );
}


/// The same defect on the channels section, which has a shipped delete button.
///
/// `channel.rs`'s delete handler drops the key and persists `["channels"]`.
/// With the section skipping itself when empty, removing the *last* channel
/// answered success, changed nothing on disk, and the channel came back on the
/// next load — the operator's only reading of which is "the delete button is
/// broken", after they have already stopped looking.
#[test]
fn removing_the_last_channel_reaches_the_file() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    let mut config = Config::default();
    config.channels.insert(
        "telegram".to_string(),
        serde_json::json!({ "enabled": true, "bot_token": "x" }),
    );
    config.save_to_file(&config_path).expect("initial save");
    assert!(
        fs::read_to_string(&config_path).unwrap().contains("telegram"),
        "the fixture must register one, or the removal proves nothing"
    );

    config.channels.remove("telegram");
    config
        .save_incremental_to_file(&config_path, &["channels"])
        .expect("incremental save");

    let after = fs::read_to_string(&config_path).expect("read back");
    assert!(
        !after.contains("telegram"),
        "the delete was reported as successful but never reached disk: {after}"
    );
}

/// And on the routing rules, whose delete handler persists `["rules"]` the
/// same way. `Vec::is_empty` rather than `HashMap::is_empty`, same outcome:
/// deleting the last rule was a silent no-op.
#[test]
fn removing_the_last_routing_rule_reaches_the_file() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    let mut config = Config {
        rules: vec![serde_json::from_value(serde_json::json!({
            "regex": "^/custom",
            "provider": "my_provider",
        }))
        .expect("a minimal routing rule")],
        ..Config::default()
    };
    config.save_to_file(&config_path).expect("initial save");
    assert!(
        fs::read_to_string(&config_path).unwrap().contains("^/custom"),
        "the fixture must register one, or the removal proves nothing"
    );

    config.rules.clear();
    config
        .save_incremental_to_file(&config_path, &["rules"])
        .expect("incremental save");

    let after = fs::read_to_string(&config_path).expect("read back");
    assert!(
        !after.contains("^/custom"),
        "the delete was reported as successful but never reached disk: {after}"
    );
}
