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

// =============================================================================
// Every section a caller names must be writable
// =============================================================================

/// Walk `src/` and hand back each `.rs` file's contents with `//` comment
/// lines dropped and CRLF normalised.
///
/// Both chores are load-bearing for a source-level scan in this repo: a
/// commented-out call site would otherwise be counted as a real one, and this
/// tree is checked out with CRLF on Windows, where a separator anchored to
/// `\n` matches nothing and the scan silently covers nothing.
fn production_sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let stripped: String = text
                        .replace('\r', "")
                        .lines()
                        .filter(|l| !l.trim_start().starts_with("//"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    out.push((path.display().to_string(), stripped));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    out
}

/// Every dot-path any `save_incremental` / `save_incremental_to_file` call in
/// `src/` names.
fn harvested_section_names() -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    for (_, text) in production_sources() {
        for (idx, _) in text.match_indices("save_incremental") {
            let Some(open) = text[idx..].find("(&[") else {
                continue;
            };
            let start = idx + open + 3;
            let Some(close) = text[start..].find(']') else {
                continue;
            };
            let args = &text[start..start + close];
            let mut rest = args;
            while let Some(q) = rest.find('"') {
                let after = &rest[q + 1..];
                let Some(end) = after.find('"') else { break };
                names.insert(after[..end].to_string());
                rest = &after[end + 1..];
            }
        }
    }
    names
}

/// Sections `save.rs` guards explicitly against the empty case.
///
/// `providers` carries `skip_serializing_if = "HashMap::is_empty"` like the
/// three collection sections fixed on 2026-08-20, and unlike them it is
/// deliberately *not* clearable: `guard_incremental_providers` refuses a save
/// that would drop on-disk providers and says so. That is the loud refusal
/// this whole class is about, arrived at from the other direction, so a
/// guarded section is not an unwritable one.
///
/// Derived from the guard function names rather than listed, so a section that
/// gains a guard inherits the exception and one that loses it inherits the
/// assertion.
fn explicitly_guarded_sections() -> std::collections::BTreeSet<String> {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config/save.rs"),
    )
    .expect("save.rs must be readable")
    .replace('\r', "");

    let mut out = std::collections::BTreeSet::new();
    for (idx, _) in text.match_indices("fn guard_incremental_") {
        let rest = &text[idx + "fn guard_incremental_".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
    }
    out
}

/// Top-level `Config` fields declared `Option<…>` and skipped when `None`.
///
/// Derived from `structs.rs` rather than listed here: a list would be right on
/// the day it was written and blind to the next optional section, which is the
/// exact failure mode the assertion below exists to catch.
fn optional_top_level_sections() -> std::collections::BTreeSet<String> {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config/structs.rs"),
    )
    .expect("structs.rs must be readable")
    .replace('\r', "");
    let body = text
        .split_once("pub struct Config {")
        .expect("Config struct must exist")
        .1;
    let body = body.split_once("\n}").expect("Config struct must close").0;

    let mut out = std::collections::BTreeSet::new();
    let mut skipped = false;
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("#[serde(") {
            skipped = t.contains("Option::is_none");
            continue;
        }
        if let Some(decl) = t.strip_prefix("pub ") {
            if skipped {
                if let Some((name, ty)) = decl.split_once(':') {
                    if ty.trim_start().starts_with("Option<") {
                        out.insert(name.trim().to_string());
                    }
                }
            }
            skipped = false;
        }
    }
    out
}

/// A section a handler persists must be one the merge can actually write.
///
/// `merge_sections` now refuses a named section that is absent from the
/// serialised config instead of warning and reporting success, so a name that
/// cannot resolve is a user-visible error rather than a silent no-op. That
/// makes "does every existing call site still resolve?" a question with
/// consequences, and this answers it from the source rather than from a list
/// somebody has to remember to extend.
///
/// Three ways to pass, and only three:
///
/// * the dot-path resolves against a serialised `Config::default()` — true for
///   every plain section and, deliberately, for `plugin_marketplaces`,
///   `channels` and `rules`, whose `skip_serializing_if` was removed on
///   2026-08-20 precisely so their delete handlers could clear them;
/// * or its root is an `Option` section, whose handlers set `Some(default)`
///   before saving (audited: `behavior`, `search`, `fetch`, `unified_tools` —
///   each call site is inside an `is_none() → Some` init or an `if let Some`);
/// * or `save.rs` guards it explicitly, which is the same loud refusal reached
///   from the other side (`providers`).
///
/// A new collection section that skips itself when empty satisfies none of the
/// three and fails here by name — which is the bug this whole class began with.
#[test]
fn every_section_a_handler_persists_is_one_the_merge_can_write() {
    let names = harvested_section_names();
    assert!(
        names.len() >= 15,
        "the scan found only {} section names; it is not reading the call sites it thinks it is",
        names.len()
    );

    let optional = optional_top_level_sections();
    assert!(
        !optional.is_empty(),
        "no optional sections parsed out of structs.rs — the parser has gone blind"
    );

    let guarded = explicitly_guarded_sections();
    assert!(
        guarded.contains("providers"),
        "no guarded sections parsed out of save.rs — the parser has gone blind"
    );

    let serialised = toml::Value::try_from(Config::default()).expect("default config serialises");

    let mut unwritable = Vec::new();
    for name in &names {
        let parts: Vec<&str> = name.split('.').collect();
        let resolves = parts.iter().try_fold(&serialised, |n, p| n.get(p)).is_some();
        if !resolves && !optional.contains(parts[0]) && !guarded.contains(parts[0]) {
            unwritable.push(name.clone());
        }
    }

    assert!(
        unwritable.is_empty(),
        "these sections are named by a `save_incremental` call but the merge cannot write them, \
         so those calls change nothing: {unwritable:?}. Either the name is stale, or the field \
         carries `skip_serializing_if` and must emit when empty (see `Config::plugin_marketplaces`)."
    );
}

/// The refusal itself: an absent section is an error, and the file is left
/// exactly as it was.
///
/// Proven to fail by name: restoring `merge_sections`' `warn!`-and-skip makes
/// this report `Ok`, which is precisely the lie the change removes.
#[test]
fn naming_a_section_the_merge_cannot_write_is_an_error_not_a_silent_success() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("config.toml");
    let before = "[general]\ndefault_provider = \"p\"\n";
    fs::write(&path, before).unwrap();

    // `behavior` is `Option` and `None` by default, so it is absent from the
    // serialised config — the same shape as a collection section that skipped
    // itself once its last element was removed.
    let cfg = Config {
        behavior: None,
        ..Config::default()
    };

    let err = cfg
        .save_incremental_to_file(&path, &["behavior"])
        .expect_err("an unwritable section must not report success");
    let msg = err.to_string();
    assert!(
        msg.contains("behavior") && msg.contains("changed nothing"),
        "the refusal must name the section and say nothing happened, got: {msg}"
    );

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        before,
        "a refused incremental save must not rewrite the file"
    );
}

/// The refusal is all-or-nothing: naming one writable and one unwritable
/// section does not half-apply. A partial write reported as a whole one is the
/// same lie in a smaller box.
#[test]
fn a_refused_section_does_not_half_apply_its_siblings() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("config.toml");
    fs::write(&path, "[general]\ndefault_provider = \"before\"\n").unwrap();

    let mut cfg = Config {
        behavior: None,
        ..Config::default()
    };
    cfg.general.default_provider = Some("after".to_string());

    assert!(cfg
        .save_incremental_to_file(&path, &["general", "behavior"])
        .is_err());
    let on_disk = fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.contains("before") && !on_disk.contains("after"),
        "the writable sibling must not have been written: {on_disk}"
    );
}
