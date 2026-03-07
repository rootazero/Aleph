# Multi-Bot Channel Support Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow each social platform to configure multiple independent bot instances while keeping backward compatibility with existing single-bot configs.

**Architecture:** Minimal-change approach — add `ChannelInstanceConfig` struct and `resolved_channels()` method to config layer, rewrite `initialize_channels` to iterate dynamically, refactor `create_channel_from_config` to accept separate `id` and `channel_type` args, add `channel.create`/`channel.delete` RPCs.

**Tech Stack:** Rust, serde_json, tokio, TOML config

---

### Task 1: Add `ChannelInstanceConfig` and `resolved_channels()` method

**Files:**
- Modify: `core/src/config/structs.rs:120-124`
- Create: `core/src/config/tests/channels.rs`
- Modify: `core/src/config/tests/mod.rs:9-17`

**Step 1: Write the failing test**

Create `core/src/config/tests/channels.rs`:

```rust
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
    // type field should be stripped from config
    assert!(main.config.get("type").is_none());
    assert_eq!(main.config["bot_token"], "123:ABC");

    let work = instances.iter().find(|i| i.id == "telegram-work").unwrap();
    assert_eq!(work.channel_type, "telegram");
    assert_eq!(work.config["bot_token"], "456:DEF");
}

#[test]
fn test_resolved_channels_infers_type_from_key() {
    let mut config = Config::default();
    config.channels.insert(
        "telegram".to_string(),
        json!({ "bot_token": "123:ABC" }),
    );

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
    // Old format: key = platform name, no type
    config.channels.insert(
        "telegram".to_string(),
        json!({ "bot_token": "old-token" }),
    );
    // New format: custom id + explicit type
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
    let mut config = Config::default();
    let platforms = [
        "telegram", "discord", "whatsapp", "slack", "imessage",
        "email", "matrix", "signal", "mattermost", "irc",
        "webhook", "xmpp", "nostr",
    ];
    for name in &platforms {
        config.channels.insert(name.to_string(), json!({}));
    }

    let instances = config.resolved_channels();
    assert_eq!(instances.len(), platforms.len());
}
```

**Step 2: Register the test module**

Add `mod channels;` to `core/src/config/tests/mod.rs`.

**Step 3: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::tests::channels -- --nocapture`
Expected: FAIL — `resolved_channels` method and `ChannelInstanceConfig` don't exist yet.

**Step 4: Implement `ChannelInstanceConfig` and `resolved_channels()`**

In `core/src/config/structs.rs`, add after line 146 (after the `Config` struct closing brace):

```rust
/// Known platform names for type auto-inference from channel config keys.
const KNOWN_CHANNEL_TYPES: &[&str] = &[
    "telegram", "discord", "whatsapp", "slack", "imessage",
    "email", "matrix", "signal", "mattermost", "irc",
    "webhook", "xmpp", "nostr",
];

/// A resolved channel instance from the channels config HashMap.
///
/// Handles both new format (explicit `type` field) and old format
/// (key is a known platform name, type auto-inferred).
#[derive(Debug, Clone)]
pub struct ChannelInstanceConfig {
    /// Instance identifier (the HashMap key)
    pub id: String,
    /// Channel platform type (e.g. "telegram", "discord")
    pub channel_type: String,
    /// Remaining config with `type` field stripped
    pub config: serde_json::Value,
}

impl Config {
    /// Parse the `channels` HashMap into resolved channel instances.
    ///
    /// Type resolution rules:
    /// 1. If value has a `type` string field -> use it as channel_type
    /// 2. If no `type` field and key is a known platform name -> infer type = key
    /// 3. Otherwise -> warn and skip
    pub fn resolved_channels(&self) -> Vec<ChannelInstanceConfig> {
        let mut instances = Vec::new();
        for (key, value) in &self.channels {
            let channel_type = if let Some(t) = value.get("type").and_then(|v| v.as_str()) {
                t.to_string()
            } else if KNOWN_CHANNEL_TYPES.contains(&key.as_str()) {
                key.clone()
            } else {
                tracing::warn!(
                    "Channel '{}' has no 'type' field and is not a known platform name, skipping",
                    key
                );
                continue;
            };

            // Strip the `type` field from the config value
            let config = if let serde_json::Value::Object(mut map) = value.clone() {
                map.remove("type");
                serde_json::Value::Object(map)
            } else {
                value.clone()
            };

            instances.push(ChannelInstanceConfig {
                id: key.clone(),
                channel_type,
                config,
            });
        }
        // Sort by id for deterministic ordering
        instances.sort_by(|a, b| a.id.cmp(&b.id));
        instances
    }
}
```

Add `use tracing;` at the top of `structs.rs` if not already present.

**Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib config::tests::channels -- --nocapture`
Expected: All 5 tests PASS.

**Step 6: Commit**

```bash
git add core/src/config/structs.rs core/src/config/tests/channels.rs core/src/config/tests/mod.rs
git commit -m "config: add ChannelInstanceConfig and resolved_channels() for multi-bot support"
```

---

### Task 2: Refactor `create_channel_from_config` to accept separate `id` and `type`

**Files:**
- Modify: `core/src/gateway/handlers/channel.rs:214-260`

**Step 1: Refactor the function signature and body**

Change `create_channel_from_config` at line 215 from:

```rust
fn create_channel_from_config(channel_type: &str, config: Value) -> Option<Box<dyn crate::gateway::channel::Channel>> {
```

to:

```rust
/// Create a channel instance from config JSON.
///
/// # Arguments
/// * `id` - Unique instance identifier (e.g. "telegram-main")
/// * `channel_type` - Platform type (e.g. "telegram")
/// * `config` - Platform-specific configuration JSON
pub fn create_channel_from_config(id: &str, channel_type: &str, config: serde_json::Value) -> Option<Box<dyn crate::gateway::channel::Channel>> {
```

Then update every match arm to use `id` instead of the hardcoded platform name. For example:

```rust
    match channel_type {
        "telegram" => serde_json::from_value::<TelegramConfig>(config).ok()
            .map(|cfg| {
                let slash_cmds = get_telegram_slash_commands();
                Box::new(TelegramChannel::new(id, cfg).with_slash_commands(slash_cmds))
                    as Box<dyn crate::gateway::channel::Channel>
            }),
        "discord" => serde_json::from_value::<DiscordConfig>(config).ok()
            .map(|cfg| Box::new(DiscordChannel::new(id, cfg)) as _),
        "whatsapp" => serde_json::from_value::<WhatsAppConfig>(config).ok()
            .map(|cfg| Box::new(WhatsAppChannel::new(id, cfg)) as _),
        "slack" => serde_json::from_value::<SlackConfig>(config).ok()
            .map(|cfg| Box::new(SlackChannel::new(id, cfg)) as _),
        "email" => serde_json::from_value::<EmailConfig>(config).ok()
            .map(|cfg| Box::new(EmailChannel::new(id, cfg)) as _),
        "matrix" => serde_json::from_value::<MatrixConfig>(config).ok()
            .map(|cfg| Box::new(MatrixChannel::new(id, cfg)) as _),
        "signal" => serde_json::from_value::<SignalConfig>(config).ok()
            .map(|cfg| Box::new(SignalChannel::new(id, cfg)) as _),
        "mattermost" => serde_json::from_value::<MattermostConfig>(config).ok()
            .map(|cfg| Box::new(MattermostChannel::new(id, cfg)) as _),
        "irc" => serde_json::from_value::<IrcConfig>(config).ok()
            .map(|cfg| Box::new(IrcChannel::new(id, cfg)) as _),
        "webhook" => serde_json::from_value::<WebhookConfig>(config).ok()
            .map(|cfg| Box::new(WebhookChannel::new(id, cfg)) as _),
        "xmpp" => serde_json::from_value::<XmppConfig>(config).ok()
            .map(|cfg| Box::new(XmppChannel::new(id, cfg)) as _),
        "nostr" => serde_json::from_value::<NostrConfig>(config).ok()
            .map(|cfg| Box::new(NostrChannel::new(id, cfg)) as _),
        _ => None,
    }
```

**Step 2: Update the caller in `handle_start`**

At line 190, change:

```rust
if let Some(new_channel) = create_channel_from_config(channel_id.as_str(), channel_config.clone()) {
```

to use `resolved_channels()` for type resolution:

```rust
// Resolve channel type from config (supports both old and new format)
let channel_type = channel_config
    .get("type")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string())
    .unwrap_or_else(|| channel_id.as_str().to_string());

// Strip `type` from config before passing to constructor
let mut clean_config = channel_config.clone();
if let serde_json::Value::Object(ref mut map) = clean_config {
    map.remove("type");
}

if let Some(new_channel) = create_channel_from_config(channel_id.as_str(), &channel_type, clean_config) {
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: No errors.

**Step 4: Run existing tests**

Run: `cargo test -p alephcore --lib gateway::handlers::channel -- --nocapture`
Expected: Existing `test_status_to_string` passes.

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/channel.rs
git commit -m "gateway: refactor create_channel_from_config to accept separate id and type"
```

---

### Task 3: Rewrite `initialize_channels` for dynamic multi-instance creation

**Files:**
- Modify: `core/src/bin/aleph/commands/start/mod.rs:1049-1176`

**Step 1: Rewrite `initialize_channels`**

Replace the entire function body (lines 1057-1176) with the dynamic approach. The key logic:

```rust
async fn initialize_channels(
    server: &mut GatewayServer,
    gateway_config: &FullGatewayConfig,
    app_config: &alephcore::Config,
    app_config_arc: &Arc<tokio::sync::RwLock<alephcore::Config>>,
    dispatch_registry: Option<&alephcore::dispatcher::ToolRegistry>,
    daemon: bool,
) -> Arc<ChannelRegistry> {
    use alephcore::config::structs::ChannelInstanceConfig;
    use alephcore::gateway::handlers::channel::{
        create_channel_from_config, set_telegram_slash_commands,
    };

    let channel_registry = Arc::new(ChannelRegistry::new());

    // 1. Resolve all channel instances from config
    let mut instances = app_config.resolved_channels();

    // 2. Fallback: if no telegram instance from config, try aleph.toml / env
    let has_telegram = instances.iter().any(|i| i.channel_type == "telegram");
    if !has_telegram {
        let fallback_config = if let Some(ref gw_tg) = gateway_config.channels.telegram {
            let mut cfg = serde_json::Map::new();
            cfg.insert("bot_token".to_string(), serde_json::Value::String(gw_tg.token.clone()));
            Some(serde_json::Value::Object(cfg))
        } else {
            // Try env var fallback
            TelegramConfig::from_env().ok().map(|cfg| {
                serde_json::to_value(&cfg).unwrap_or_default()
            })
        };
        if let Some(config) = fallback_config {
            instances.push(ChannelInstanceConfig {
                id: "telegram".to_string(),
                channel_type: "telegram".to_string(),
                config,
            });
        }
    }

    // 3. Fallback: if no discord instance, create default
    let has_discord = instances.iter().any(|i| i.channel_type == "discord");
    if !has_discord {
        instances.push(ChannelInstanceConfig {
            id: "discord".to_string(),
            channel_type: "discord".to_string(),
            config: serde_json::Value::Object(serde_json::Map::new()),
        });
    }

    // 4. Fallback: if no whatsapp instance, create default
    let has_whatsapp = instances.iter().any(|i| i.channel_type == "whatsapp");
    if !has_whatsapp {
        instances.push(ChannelInstanceConfig {
            id: "whatsapp".to_string(),
            channel_type: "whatsapp".to_string(),
            config: serde_json::Value::Object(serde_json::Map::new()),
        });
    }

    // 5. macOS: Fallback iMessage if not configured
    #[cfg(target_os = "macos")]
    {
        let has_imessage = instances.iter().any(|i| i.channel_type == "imessage");
        if !has_imessage {
            instances.push(ChannelInstanceConfig {
                id: "imessage".to_string(),
                channel_type: "imessage".to_string(),
                config: serde_json::Value::Object(serde_json::Map::new()),
            });
        }
    }

    // 6. Build slash commands once for all Telegram instances
    let slash_commands = if instances.iter().any(|i| i.channel_type == "telegram") {
        let cmds = if let Some(reg) = dispatch_registry {
            use alephcore::dispatcher::ChannelType;
            let tools = reg.list_for_channel(ChannelType::Telegram).await;
            tools.iter()
                .map(|t| (t.name.clone(), t.description.clone()))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if !daemon {
            println!("  Telegram slash commands to register: {}", cmds.len());
            for (name, desc) in &cmds {
                let truncated: String = desc.chars().take(60).collect();
                println!("    /{} — {}", name, truncated);
            }
        }
        set_telegram_slash_commands(cmds.clone());
        cmds
    } else {
        Vec::new()
    };

    // 7. Create and register all channel instances
    for inst in &instances {
        // iMessage requires special handling on macOS
        #[cfg(target_os = "macos")]
        if inst.channel_type == "imessage" {
            let imessage_config = serde_json::from_value::<IMessageConfig>(inst.config.clone())
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to parse imessage config for '{}': {}, using default", inst.id, e);
                    IMessageConfig::default()
                });
            let imessage_channel = IMessageChannel::new(imessage_config);
            let channel_id = channel_registry.register(Box::new(imessage_channel)).await;
            if !daemon {
                println!("Registered channel: {} (iMessage)", channel_id);
            }
            continue;
        }

        if let Some(channel) = create_channel_from_config(&inst.id, &inst.channel_type, inst.config.clone()) {
            let channel_id = channel_registry.register(channel).await;
            if !daemon {
                println!("Registered channel: {} ({})", channel_id, inst.channel_type);
            }
        } else {
            tracing::warn!("Failed to create channel '{}' (type: {})", inst.id, inst.channel_type);
        }
    }

    register_channel_handlers(server, &channel_registry, app_config_arc);

    // Auto-start all registered channels
    let start_results = channel_registry.start_all().await;
    for (ch_id, result) in &start_results {
        match result {
            Ok(()) => println!("  ✓ Channel {} started", ch_id),
            Err(e) => eprintln!("  ✗ Channel {} failed: {}", ch_id, e),
        }
    }
    if !daemon {
        let ok_count = start_results.iter().filter(|(_, r)| r.is_ok()).count();
        println!("Auto-started {}/{} channels", ok_count, start_results.len());
    }

    if !daemon {
        println!("Channel methods:");
        println!("  - channels.list   : List all channels");
        println!("  - channels.status : Get channel status");
        println!("  - channel.start   : Start a channel");
        println!("  - channel.stop    : Stop a channel");
        println!("  - channel.send    : Send message via channel");
        println!("  - channel.create  : Create a new channel instance");
        println!("  - channel.delete  : Delete a channel instance");
        println!();
    }

    // Start external bridge plugins via LinkManager
    {
        use alephcore::gateway::link::LinkManager;
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph");
        let link_manager = LinkManager::new(base_dir);
        if let Err(e) = link_manager.start().await {
            tracing::warn!("LinkManager startup encountered errors: {}", e);
        }
        if !daemon {
            println!("LinkManager started (external bridge plugins)");
            println!();
        }
    }

    channel_registry
}
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: No errors. Note: `ChannelInstanceConfig` must be made public from `alephcore` (it's in `config::structs` which is already public).

**Step 3: Manual smoke test**

Run: `cargo run --bin aleph` (with existing `config.toml` that has `[channels.telegram]`)
Expected: Server starts, Telegram channel registers as before with id "telegram".

**Step 4: Commit**

```bash
git add core/src/bin/aleph/commands/start/mod.rs
git commit -m "gateway: rewrite initialize_channels for dynamic multi-instance creation"
```

---

### Task 4: Add `channel.create` and `channel.delete` RPC handlers

**Files:**
- Modify: `core/src/gateway/handlers/channel.rs` (add two new handler functions)
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs:131-150` (register new handlers)

**Step 1: Add `handle_create` handler**

In `core/src/gateway/handlers/channel.rs`, add before the `#[cfg(test)]` block:

```rust
/// Handle channel.create RPC request
///
/// Creates a new channel instance, saves to config, registers, and auto-starts.
pub async fn handle_create(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
    app_config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let id = match params.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'id' field");
        }
    };

    let channel_type = match params.get("type").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'type' field");
        }
    };

    let config = params
        .get("config")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    debug!("Handling channel.create: id={}, type={}", id, channel_type);

    // Check if channel already exists
    let channel_id = ChannelId::new(&id);
    if registry.get(&channel_id).await.is_some() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel '{}' already exists", id),
        );
    }

    // Create channel instance
    let channel = match create_channel_from_config(&id, &channel_type, config.clone()) {
        Some(ch) => ch,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Failed to create channel: unsupported type '{}' or invalid config", channel_type),
            );
        }
    };

    // Register the channel
    registry.register(channel).await;

    // Save to app config
    {
        let mut app_cfg = app_config.write().await;
        let mut config_to_save = if let Value::Object(map) = config {
            map
        } else {
            serde_json::Map::new()
        };
        config_to_save.insert("type".to_string(), Value::String(channel_type.clone()));
        app_cfg.channels.insert(id.clone(), Value::Object(config_to_save));
    }

    // Auto-start the channel
    let start_result = registry.start_channel(&channel_id).await;

    match start_result {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "id": id,
                "type": channel_type,
                "status": "started",
            }),
        ),
        Err(e) => JsonRpcResponse::success(
            request.id,
            json!({
                "id": id,
                "type": channel_type,
                "status": "created_but_start_failed",
                "error": e.to_string(),
            }),
        ),
    }
}

/// Handle channel.delete RPC request
///
/// Stops a channel, removes from registry, and removes from config.
pub async fn handle_delete(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
    app_config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let channel_id = match &request.params {
        Some(Value::Object(map)) => map.get("id").and_then(|v| v.as_str()),
        _ => None,
    };

    let id = match channel_id {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'id' field");
        }
    };

    let channel_id = ChannelId::new(&id);

    debug!("Handling channel.delete: id={}", id);

    // Check if channel exists
    if registry.get(&channel_id).await.is_none() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel '{}' not found", id),
        );
    }

    // Stop the channel first
    let _ = registry.stop_channel(&channel_id).await;

    // Remove from registry
    registry.unregister(&channel_id).await;

    // Remove from app config
    {
        let mut app_cfg = app_config.write().await;
        app_cfg.channels.remove(&id);
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "id": id,
            "status": "deleted",
        }),
    )
}
```

**Step 2: Register the new handlers**

In `core/src/bin/aleph/commands/start/builder/handlers.rs`, add after line 141 (after `channel.send`):

```rust
    register_handler!(server, "channel.create", channel_handlers::handle_create, channel_registry, app_config);
    register_handler!(server, "channel.delete", channel_handlers::handle_delete, channel_registry, app_config);
```

**Step 3: Verify compilation**

Run: `cargo check --bin aleph`
Expected: No errors.

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/channel.rs core/src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "gateway: add channel.create and channel.delete RPC handlers"
```

---

### Task 5: Update `handle_start` to resolve type for multi-instance channels

**Files:**
- Modify: `core/src/gateway/handlers/channel.rs:186-195`

This was partially covered in Task 2 Step 2, but let's be explicit. The `handle_start` handler must resolve the channel type correctly for both old and new format configs.

**Step 1: Verify the updated `handle_start` from Task 2 works correctly**

The changes from Task 2 Step 2 already handle this. Run a manual test:

Run: `cargo run --bin aleph`
Then via WebSocket or CLI, send: `{"jsonrpc":"2.0","id":1,"method":"channel.start","params":{"channel_id":"telegram"}}`
Expected: Channel restarts successfully.

**Step 2: Commit** (if any additional changes needed)

```bash
git add core/src/gateway/handlers/channel.rs
git commit -m "gateway: ensure handle_start resolves channel type for multi-instance"
```

---

### Task 6: Export `ChannelInstanceConfig` from alephcore public API

**Files:**
- Modify: `core/src/lib.rs` (or wherever `config::structs` is re-exported)

**Step 1: Verify `ChannelInstanceConfig` is accessible from the binary crate**

Check that `core/src/config/structs.rs` is publicly accessible. The struct needs to be usable from `core/src/bin/aleph/commands/start/mod.rs` as `alephcore::config::structs::ChannelInstanceConfig`.

Run: `cargo check --bin aleph`

If `ChannelInstanceConfig` is not accessible, add a re-export in the config module's public API. Check `core/src/config/mod.rs` for existing re-exports and add:

```rust
pub use structs::ChannelInstanceConfig;
```

Also ensure `create_channel_from_config` in `channel.rs` is `pub` (changed in Task 2).

**Step 2: Commit if changes needed**

```bash
git add core/src/config/mod.rs core/src/lib.rs
git commit -m "config: export ChannelInstanceConfig from public API"
```

---

### Task 7: End-to-end integration test

**Files:**
- Create: `core/src/config/tests/channels.rs` (add integration-style test, append to existing file)

**Step 1: Add integration test for full config round-trip**

Append to `core/src/config/tests/channels.rs`:

```rust
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
```

**Step 2: Run all channel config tests**

Run: `cargo test -p alephcore --lib config::tests::channels -- --nocapture`
Expected: All 6 tests PASS.

**Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: No regressions (pre-existing `markdown_skill` failures are known and unrelated).

**Step 4: Commit**

```bash
git add core/src/config/tests/channels.rs
git commit -m "test: add integration test for multi-bot channel config parsing"
```

---

### Task 8: Final verification and cleanup

**Step 1: Full compilation check**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: Clean.

**Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: No new failures.

**Step 3: Manual smoke test with existing config**

Run: `cargo run --bin aleph`
Expected: Existing single-bot Telegram channel starts normally with id "telegram". All other channels register as before.

**Step 4: Verify backward compatibility**

Confirm that a `config.toml` with old format `[channels.telegram]` (no `type` field) still works identically to before.

---

## Summary of Tasks

| Task | Description | Files | Estimated Steps |
|------|-------------|-------|-----------------|
| 1 | `ChannelInstanceConfig` + `resolved_channels()` + tests | structs.rs, tests/ | 6 |
| 2 | Refactor `create_channel_from_config` signature | channel.rs | 5 |
| 3 | Rewrite `initialize_channels` | start/mod.rs | 4 |
| 4 | Add `channel.create`/`channel.delete` RPCs | channel.rs, handlers.rs | 4 |
| 5 | Verify `handle_start` type resolution | channel.rs | 2 |
| 6 | Export public API | config/mod.rs | 2 |
| 7 | Integration test | tests/channels.rs | 4 |
| 8 | Final verification | — | 4 |

Note: Panel UI changes (overview badges, instance list page) are deferred to a separate plan as they involve Leptos/WASM code in `apps/panel/` and are independent of the core logic.
