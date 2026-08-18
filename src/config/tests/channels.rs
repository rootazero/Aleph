//! Tests for multi-bot channel configuration parsing

use crate::Config;
use serde_json::json;

#[test]
fn test_resolved_channels_with_explicit_type() {
    let mut config = Config::default();
    config.channels.insert(
        "telegram-main".to_string(),
        json!({ "type": "telegram", "bot_token": "123:ABC" }),
    );
    config.channels.insert(
        "telegram-work".to_string(),
        json!({ "type": "telegram", "bot_token": "456:DEF" }),
    );

    let instances = config.resolved_channels();
    assert_eq!(instances.len(), 2);

    let main = instances.iter().find(|i| i.id == "telegram-main").unwrap();
    assert_eq!(main.channel_type, "telegram");
    assert!(main.config.get("type").is_none());
    assert_eq!(main.config["bot_token"], "123:ABC");

    let work = instances.iter().find(|i| i.id == "telegram-work").unwrap();
    assert_eq!(work.channel_type, "telegram");
    assert_eq!(work.config["bot_token"], "456:DEF");
}

#[test]
fn test_resolved_channels_infers_type_from_key() {
    let mut config = Config::default();
    config
        .channels
        .insert("telegram".to_string(), json!({ "bot_token": "123:ABC" }));

    let instances = config.resolved_channels();
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].id, "telegram");
    assert_eq!(instances[0].channel_type, "telegram");
    assert_eq!(instances[0].config["bot_token"], "123:ABC");
}

#[test]
fn test_resolved_channels_unknown_key_no_type_skipped() {
    let mut config = Config::default();
    config.channels.insert(
        "my-custom-bot".to_string(),
        json!({ "bot_token": "123:ABC" }),
    );

    let instances = config.resolved_channels();
    assert_eq!(instances.len(), 0);
}

#[test]
fn test_resolved_channels_mixed_old_and_new_format() {
    let mut config = Config::default();
    config
        .channels
        .insert("telegram".to_string(), json!({ "bot_token": "old-token" }));
    config.channels.insert(
        "telegram-work".to_string(),
        json!({ "type": "telegram", "bot_token": "new-token" }),
    );

    let instances = config.resolved_channels();
    assert_eq!(instances.len(), 2);

    let old = instances.iter().find(|i| i.id == "telegram").unwrap();
    assert_eq!(old.channel_type, "telegram");

    let new = instances.iter().find(|i| i.id == "telegram-work").unwrap();
    assert_eq!(new.channel_type, "telegram");
}

#[test]
fn test_resolved_channels_all_known_platforms() {
    // Derived, not hand-copied. This test used to spell out fourteen names of
    // its own next to a private `KNOWN_CHANNEL_TYPES` that spelled out fifteen
    // — a hand-copied list asserting against a hand-copied list, which is only
    // ever a statement about serde. It passed for months while `line` and
    // `wechat` (both fully registered, both configurable) were missing from
    // the production list, so `[channels.line]` was silently skipped unless the
    // user also wrote `type = "line"`; and its own copy had already lost
    // `feishu`, so it could not have noticed either way.
    use aleph_protocol::channels::CONFIGURABLE_CHANNEL_TYPES;

    let mut config = Config::default();
    for name in CONFIGURABLE_CHANNEL_TYPES {
        config.channels.insert((*name).to_string(), json!({}));
    }

    let instances = config.resolved_channels();
    let resolved: Vec<&str> = instances.iter().map(|i| i.channel_type.as_str()).collect();
    let missing: Vec<&&str> = CONFIGURABLE_CHANNEL_TYPES
        .iter()
        .filter(|t| !resolved.contains(*t))
        .collect();
    assert!(
        missing.is_empty(),
        "`[channels.{missing:?}]` does not infer its type from the section key, so the block is \
         skipped with a warn that only reaches the file log — the user sees a channel that \
         silently does not exist",
    );
    assert_eq!(instances.len(), CONFIGURABLE_CHANNEL_TYPES.len());
}

#[test]
fn test_resolved_channels_from_toml_string() {
    let toml_str = r#"
[channels.telegram]
bot_token = "old-single-bot"
allowed_users = [111]

[channels."telegram-work"]
type = "telegram"
bot_token = "new-work-bot"
allowed_users = [222]

[channels."discord-gaming"]
type = "discord"
bot_token = "discord-token"
"#;

    let config: Config = toml::from_str(toml_str).expect("should parse");
    let instances = config.resolved_channels();

    assert_eq!(instances.len(), 3);

    // Sorted by id
    assert_eq!(instances[0].id, "discord-gaming");
    assert_eq!(instances[0].channel_type, "discord");

    assert_eq!(instances[1].id, "telegram");
    assert_eq!(instances[1].channel_type, "telegram");
    assert_eq!(instances[1].config["bot_token"], "old-single-bot");

    assert_eq!(instances[2].id, "telegram-work");
    assert_eq!(instances[2].channel_type, "telegram");
    assert_eq!(instances[2].config["bot_token"], "new-work-bot");
}
