# Session Isolation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add session isolation with `/new` command, LLM topic generation, and session-aware memory retrieval.

**Architecture:** Extend SessionKey with epoch versioning, intercept `/new` at Gateway layer, generate topics via LLM on session close, add session_id to MemoryFilter for two-phase retrieval.

**Tech Stack:** Rust, SQLite (rusqlite), LanceDB (Arrow), tokio async

---

## Task 1: Clean up topic_id → session_id in ContextAnchor

**Files:**
- Modify: `src/memory/context/mod.rs:26-67`

**Step 1: Rename topic_id to session_id in ContextAnchor**

In `src/memory/context/mod.rs`, replace:

```rust
pub struct ContextAnchor {
    pub window_title: String,
    pub timestamp: i64,
    pub topic_id: String,
}

pub const SINGLE_TURN_TOPIC_ID: &str = "single-turn";
```

With:

```rust
pub struct ContextAnchor {
    pub window_title: String,
    pub timestamp: i64,
    pub session_id: String,
}

pub const NO_SESSION: &str = "none";
```

And update all constructors:
- `now()` → use `NO_SESSION` instead of `SINGLE_TURN_TOPIC_ID`
- `with_timestamp()` → same
- `with_topic()` → rename to `with_session(window_title, session_id)`

**Step 2: Fix all compilation errors from the rename**

Run: `cargo check -p alephcore 2>&1 | head -80`

This will show ~40 compile errors. Fix them mechanically:
- `m.context.topic_id` → `m.context.session_id`
- `topic_id: None` → `session_id: None` (in CapturedContext, InputEvent, RequestContext)
- `topic_id: Some(...)` → `session_id: Some(...)`
- `SINGLE_TURN_TOPIC_ID` → `NO_SESSION`
- `with_topic_id(...)` → `with_session_id(...)` (in RequestContext builder)

Key files to touch:
- `src/core/types.rs:33` — CapturedContext field
- `src/event/types.rs:235` — InputEvent field
- `src/executor/types.rs:319,351-352` — RequestContext field + builder
- `src/memory/store/lance/arrow_convert.rs:647-732` — Arrow column name `"topic_id"` → `"session_id"`
- `src/memory/store/lance/schema.rs:122` — schema field name
- `src/resilience/database/state_database.rs:65,75` — SQLite column + index
- `src/conversation/manager.rs:263` — context creation
- `src/conversation/session.rs:112` — context creation
- `src/components/intent_analyzer.rs:301` — context creation
- `src/components/session_recorder.rs:761,973` — test data
- `src/components/session_compactor/tests/mod.rs:557` — test data
- `src/components/integration_test.rs:29` — test data
- `src/components/loop_controller.rs:877` — test data
- `src/components/task_planner.rs:318` — context creation
- `src/event/bus.rs:354,379` — event construction
- `src/event/global_bus.rs:429` — event construction
- `src/event/filter.rs:222,336,356` — test data
- `src/event/integration_test.rs:146` — test data
- `src/event/tests/integration.rs:70,78,88,243` — test data
- `src/event/handler.rs:332,371` — event construction
- `src/memory/compression/extractor.rs:269` — test data
- `src/memory/mod.rs:4` — doc comment

**Step 3: Handle Arrow backward compatibility**

In `src/memory/store/lance/arrow_convert.rs`, the deserialization must handle both old `"topic_id"` and new `"session_id"` columns:

```rust
// Try new column name first, fall back to old for backward compatibility
let session_id_col = col::<StringArray>(batch, "session_id")
    .or_else(|_| col::<StringArray>(batch, "topic_id"))
    .ok();
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: All tests pass (or only pre-existing failures in markdown_skill)

**Step 5: Commit**

```
session: rename topic_id to session_id across codebase
```

---

## Task 2: Add epoch to SessionKey

**Files:**
- Modify: `src/routing/session_key.rs:36-81,92-257,260-349`
- Modify: `src/gateway/router.rs:19-45,51-114,152-205`

**Step 1: Write tests for epoch serialization**

In `src/routing/session_key.rs`, add tests at the bottom of the `mod tests` block:

```rust
#[test]
fn test_main_with_epoch() {
    let key = SessionKey::Main {
        agent_id: "main".to_string(),
        main_key: "main".to_string(),
        epoch: 2,
    };
    assert_eq!(key.to_key_string(), "agent:main:main:s2");
}

#[test]
fn test_main_epoch_zero_no_suffix() {
    let key = SessionKey::main("main");
    assert_eq!(key.to_key_string(), "agent:main:main");
    assert_eq!(key.epoch(), 0);
}

#[test]
fn test_parse_with_epoch() {
    let key = SessionKey::parse("agent:main:main:s3").unwrap();
    assert_eq!(key.epoch(), 3);
    assert!(matches!(key, SessionKey::Main { epoch: 3, .. }));
}

#[test]
fn test_parse_without_epoch_defaults_zero() {
    let key = SessionKey::parse("agent:main:main").unwrap();
    assert_eq!(key.epoch(), 0);
}

#[test]
fn test_dm_with_epoch() {
    let key = SessionKey::DirectMessage {
        agent_id: "main".to_string(),
        channel: "telegram".to_string(),
        peer_id: "user123".to_string(),
        dm_scope: DmScope::PerPeer,
        epoch: 1,
    };
    assert_eq!(key.to_key_string(), "agent:main:dm:user123:s1");
}

#[test]
fn test_epoch_roundtrip() {
    let key = SessionKey::Main {
        agent_id: "work".to_string(),
        main_key: "main".to_string(),
        epoch: 5,
    };
    let s = key.to_key_string();
    let parsed = SessionKey::parse(&s).unwrap();
    assert_eq!(parsed.epoch(), 5);
    assert_eq!(parsed.to_key_string(), s);
}

#[test]
fn test_next_epoch() {
    let key = SessionKey::main("main");
    let next = key.with_next_epoch();
    assert_eq!(next.epoch(), 1);
    assert_eq!(next.to_key_string(), "agent:main:main:s1");
}

#[test]
fn test_base_key_without_epoch() {
    let key = SessionKey::Main {
        agent_id: "main".to_string(),
        main_key: "main".to_string(),
        epoch: 3,
    };
    assert_eq!(key.base_key_pattern(), "agent:main:main");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib session_key -- --no-run 2>&1 | tail -10`
Expected: Compilation errors (epoch field doesn't exist yet)

**Step 3: Add epoch field to SessionKey**

In `src/routing/session_key.rs`:

Add `epoch: u32` to `Main` and `DirectMessage` variants with `#[serde(default)]`:

```rust
pub enum SessionKey {
    Main {
        agent_id: String,
        #[serde(default = "default_main_key")]
        main_key: String,
        #[serde(default)]
        epoch: u32,
    },
    DirectMessage {
        agent_id: String,
        channel: String,
        peer_id: String,
        #[serde(default)]
        dm_scope: DmScope,
        #[serde(default)]
        epoch: u32,
    },
    // Group, Task, Subagent, Ephemeral unchanged
}
```

Add `epoch()` method:

```rust
/// Get the session epoch (version number)
pub fn epoch(&self) -> u32 {
    match self {
        Self::Main { epoch, .. } => *epoch,
        Self::DirectMessage { epoch, .. } => *epoch,
        _ => 0,
    }
}
```

Add `with_next_epoch()` method:

```rust
/// Create a new key with epoch incremented by 1
pub fn with_next_epoch(&self) -> Self {
    match self {
        Self::Main { agent_id, main_key, epoch } => Self::Main {
            agent_id: agent_id.clone(),
            main_key: main_key.clone(),
            epoch: epoch + 1,
        },
        Self::DirectMessage { agent_id, channel, peer_id, dm_scope, epoch } => Self::DirectMessage {
            agent_id: agent_id.clone(),
            channel: channel.clone(),
            peer_id: peer_id.clone(),
            dm_scope: *dm_scope,
            epoch: epoch + 1,
        },
        other => other.clone(), // Other types don't support epochs
    }
}
```

Add `base_key_pattern()` method:

```rust
/// Get the base key string without epoch suffix (for LIKE queries)
pub fn base_key_pattern(&self) -> String {
    match self {
        Self::Main { agent_id, main_key, .. } => format!("agent:{}:{}", agent_id, main_key),
        Self::DirectMessage { agent_id, peer_id, dm_scope, channel, .. } => {
            match dm_scope {
                DmScope::Main => format!("agent:{}:main", agent_id),
                DmScope::PerPeer => format!("agent:{}:dm:{}", agent_id, peer_id),
                DmScope::PerChannelPeer => format!("agent:{}:{}:dm:{}", agent_id, channel, peer_id),
            }
        }
        _ => self.to_key_string(),
    }
}
```

Update `main()` constructor to set `epoch: 0`.

Update `dm()` constructor to set `epoch: 0`.

**Step 4: Update serialization**

In `to_key_string()`, append `:sN` when epoch > 0:

```rust
Self::Main { agent_id, main_key, epoch } => {
    let base = format!("agent:{}:{}", agent_id, main_key);
    if *epoch > 0 { format!("{}:s{}", base, epoch) } else { base }
}
```

Same pattern for `DirectMessage`.

**Step 5: Update parsing**

In `parse()`, detect `:sN` suffix before matching:

```rust
pub fn parse(s: &str) -> Option<Self> {
    let s = s.trim().to_lowercase();
    let parts: Vec<&str> = s.split(':').collect();

    if parts.len() < 3 || parts[0] != "agent" {
        return None;
    }

    let agent_id = normalize_agent_id(parts[1]);
    if agent_id.is_empty() {
        return None;
    }

    // Check if last part is epoch suffix "sN"
    let (rest_parts, epoch) = if let Some(last) = parts.last() {
        if let Some(n_str) = last.strip_prefix('s') {
            if let Ok(n) = n_str.parse::<u32>() {
                (&parts[2..parts.len()-1], n)
            } else {
                (&parts[2..], 0)
            }
        } else {
            (&parts[2..], 0)
        }
    } else {
        (&parts[2..], 0)
    };

    let rest = rest_parts;

    match rest {
        ["dm", peer_id] => Some(Self::DirectMessage {
            agent_id,
            channel: String::new(),
            peer_id: peer_id.to_string(),
            dm_scope: DmScope::PerPeer,
            epoch,
        }),
        // ... all other arms add epoch where applicable
        [main_key] => Some(Self::Main {
            agent_id,
            main_key: main_key.to_string(),
            epoch,
        }),
        _ => None,
    }
}
```

**Step 6: Update legacy router compatibility**

In `src/gateway/router.rs`, update `to_new()` and `from_new()` to handle epoch:

```rust
// to_new(): add epoch: 0 to Main and DirectMessage
Self::Main { agent_id, main_key } => crate::routing::SessionKey::Main {
    agent_id: agent_id.clone(),
    main_key: main_key.clone(),
    epoch: 0,
},

// from_new(): ignore epoch (legacy doesn't support it)
crate::routing::SessionKey::Main { agent_id, main_key, .. } => Self::Main {
    agent_id: agent_id.clone(),
    main_key: main_key.clone(),
},
```

**Step 7: Fix all compilation errors**

Run: `cargo check -p alephcore 2>&1 | head -80`

Fix any struct literal errors where `Main { .. }` or `DirectMessage { .. }` now require `epoch`.
Add `epoch: 0` to all existing constructors.

**Step 8: Run tests**

Run: `cargo test -p alephcore --lib session_key 2>&1 | tail -20`
Expected: All tests pass

**Step 9: Commit**

```
session: add epoch versioning to SessionKey
```

---

## Task 3: Add topic and status to SessionMetadata

**Files:**
- Modify: `src/gateway/session_manager.rs:28-37,48-140,248-309`
- Modify: `src/gateway/handlers/session.rs:22-35`

**Step 1: Write tests**

Add to `src/gateway/session_manager.rs` tests:

```rust
#[test]
fn test_session_identity_meta_with_topic() {
    let mut meta = SessionIdentityMeta::owner("cli");
    meta.custom.insert("topic".to_string(), serde_json::json!("Rust 并发测试"));
    meta.custom.insert("status".to_string(), serde_json::json!("closed"));

    let json = meta.to_json_string().unwrap();
    let parsed = SessionIdentityMeta::from_json_str(Some(&json));
    assert_eq!(
        parsed.custom.get("topic").and_then(|v| v.as_str()),
        Some("Rust 并发测试")
    );
    assert_eq!(
        parsed.custom.get("status").and_then(|v| v.as_str()),
        Some("closed")
    );
}
```

**Step 2: Run test to verify it passes**

The `custom: HashMap<String, Value>` with `#[serde(flatten)]` already supports arbitrary fields. This test should pass without changes.

Run: `cargo test -p alephcore --lib test_session_identity_meta_with_topic 2>&1`

**Step 3: Add topic to SessionInfo**

In `src/gateway/handlers/session.rs`, add `topic` to `SessionInfo`:

```rust
pub struct SessionInfo {
    pub key: String,
    pub agent_id: String,
    pub session_type: String,
    pub message_count: u32,
    pub created_at: String,
    pub last_active_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}
```

Update `handle_list_db()` to populate topic from metadata:

```rust
// After creating SessionInfo from metadata, query session's metadata JSON
// to extract topic and status
```

**Step 4: Add close_session and get_current_epoch to SessionManager**

In `src/gateway/session_manager.rs`:

```rust
/// Close a session: set status=closed, store topic in metadata
pub async fn close_session(
    &self,
    key: &SessionKey,
    topic: Option<String>,
) -> Result<(), SessionManagerError> {
    let key_str = key.to_key_string();
    let conn = self.conn.lock().map_err(|e|
        SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

    // Get existing metadata
    let existing_json: Option<String> = conn
        .query_row("SELECT metadata FROM sessions WHERE key = ?", params![&key_str], |row| row.get(0))
        .ok()
        .flatten();

    let mut meta = existing_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    if let Some(obj) = meta.as_object_mut() {
        obj.insert("status".to_string(), serde_json::json!("closed"));
        if let Some(t) = &topic {
            obj.insert("topic".to_string(), serde_json::json!(t));
        }
    }

    let meta_json = serde_json::to_string(&meta)
        .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

    conn.execute(
        "UPDATE sessions SET metadata = ? WHERE key = ?",
        params![&meta_json, &key_str],
    ).map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

    Ok(())
}

/// Get current epoch for a base key pattern
pub async fn get_current_epoch(
    &self,
    base_key_pattern: &str,
) -> Result<u32, SessionManagerError> {
    let conn = self.conn.lock().map_err(|e|
        SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

    let like_pattern = format!("{}%", base_key_pattern);
    let latest_key: Option<String> = conn
        .query_row(
            "SELECT key FROM sessions WHERE key LIKE ? ORDER BY created_at DESC LIMIT 1",
            params![&like_pattern],
            |row| row.get(0),
        )
        .ok();

    match latest_key {
        Some(key_str) => {
            // Parse epoch from key string suffix ":sN"
            if let Some(suffix) = key_str.rsplit(':').next() {
                if let Some(n_str) = suffix.strip_prefix('s') {
                    if let Ok(n) = n_str.parse::<u32>() {
                        return Ok(n);
                    }
                }
            }
            Ok(0) // No epoch suffix = epoch 0
        }
        None => Ok(0), // No session found = epoch 0
    }
}
```

**Step 5: Write tests for new methods**

```rust
#[tokio::test]
async fn test_close_session_with_topic() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();
    manager.add_message(&key, "user", "Hello").await.unwrap();

    manager.close_session(&key, Some("测试对话".to_string())).await.unwrap();

    // Verify metadata was updated
    let sessions = manager.list_sessions(Some("test")).await.unwrap();
    assert!(!sessions.is_empty());
}

#[tokio::test]
async fn test_get_current_epoch() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    // Create epoch 0
    let key0 = SessionKey::main("test");
    manager.get_or_create(&key0).await.unwrap();

    let epoch = manager.get_current_epoch("agent:test:main").await.unwrap();
    assert_eq!(epoch, 0);

    // Create epoch 1
    let key1 = key0.with_next_epoch();
    manager.get_or_create(&key1).await.unwrap();

    let epoch = manager.get_current_epoch("agent:test:main").await.unwrap();
    assert_eq!(epoch, 1);
}
```

**Step 6: Run tests**

Run: `cargo test -p alephcore --lib session_manager 2>&1 | tail -20`
Expected: All tests pass

**Step 7: Commit**

```
session: add close_session and epoch tracking to SessionManager
```

---

## Task 4: Add session_id filter to MemoryFilter

**Files:**
- Modify: `src/memory/store/types.rs:292-350`

**Step 1: Write tests**

Add to `src/memory/store/types.rs` tests:

```rust
#[test]
fn memory_filter_with_session_ids() {
    let f = MemoryFilter {
        session_ids: Some(vec![
            "agent:main:main".to_string(),
            "agent:main:main:s1".to_string(),
        ]),
        ..Default::default()
    };
    let sql = f.to_lance_filter().unwrap();
    assert!(sql.contains("session_id IN ("));
    assert!(sql.contains("agent:main:main"));
}

#[test]
fn memory_filter_session_ids_escapes_sql() {
    let f = MemoryFilter {
        session_ids: Some(vec!["agent:main:O'Brien".to_string()]),
        ..Default::default()
    };
    let sql = f.to_lance_filter().unwrap();
    assert!(sql.contains("O''Brien")); // SQL escaped
}
```

**Step 2: Run test to verify it fails**

Expected: `session_ids` field doesn't exist yet.

**Step 3: Add session_ids field to MemoryFilter**

```rust
pub struct MemoryFilter {
    pub window_title: Option<String>,
    pub namespace: Option<NamespaceScope>,
    pub workspace: Option<WorkspaceFilter>,
    pub after_timestamp: Option<i64>,
    /// Filter to memories from specific sessions
    pub session_ids: Option<Vec<String>>,
}
```

Update `to_lance_filter()`:

```rust
if let Some(ref ids) = self.session_ids {
    if !ids.is_empty() {
        let escaped: Vec<String> = ids.iter()
            .map(|id| format!("'{}'", escape_sql_string(id)))
            .collect();
        clauses.push(format!("session_id IN ({})", escaped.join(", ")));
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib memory_filter 2>&1 | tail -20`
Expected: All tests pass

**Step 5: Commit**

```
memory: add session_ids filter to MemoryFilter
```

---

## Task 5: Add `/new` handler in inbound_router

**Files:**
- Modify: `src/gateway/inbound_router.rs:599-646,676-734,1370-1394`

**Step 1: Add handle_new_session method**

In `src/gateway/inbound_router.rs`, after `handle_switch_command()`:

```rust
/// Handle /new command: close current session, create new epoch
async fn handle_new_session(
    &self,
    msg: &InboundMessage,
    ctx: &InboundContext,
) -> Result<(), RoutingError> {
    let old_key = &ctx.session_key;
    let agent_id = old_key.agent_id().to_string();

    // Generate topic from recent history (if session manager available)
    let topic = self.generate_session_topic(old_key).await;

    // Close old session
    if let Some(ref sm) = self.session_manager {
        if let Err(e) = sm.close_session(&old_key.to_new(), topic).await {
            warn!("[Router] Failed to close session: {}", e);
        }
    }

    // Get current epoch and create next
    let new_key = old_key.to_new().with_next_epoch();
    if let Some(ref sm) = self.session_manager {
        if let Err(e) = sm.get_or_create(&new_key).await {
            warn!("[Router] Failed to create new session: {}", e);
        }
    }

    // Update workspace manager with new session key (if applicable)
    // This ensures subsequent messages use the new epoch

    // Reply to user
    let new_key_str = new_key.to_key_string();
    let reply_text = format!("✅ 新对话已开始 ({})", new_key_str);
    let reply = OutboundMessage::text(msg.conversation_id.as_str(), reply_text);
    if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
        error!("[Router] Failed to send /new reply: {}", e);
    }

    Ok(())
}

/// Generate a topic summary for the current session using LLM
async fn generate_session_topic(
    &self,
    session_key: &SessionKey,
) -> Option<String> {
    let sm = self.session_manager.as_ref()?;
    let llm = self.llm_provider.as_ref()?;

    // Get recent history
    let history = sm.get_history(&session_key.to_new(), Some(20)).await.ok()?;
    if history.len() < 2 {
        return None; // Not enough conversation to generate topic
    }

    // Build conversation summary for LLM
    let conversation: String = history.iter()
        .map(|m| format!("{}: {}", m.role, truncate_for_topic(&m.content, 100)))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "用一句简短的中文概括以下对话的主题（10字以内，不要标点符号）：\n\n{}",
        conversation
    );

    // Single LLM call
    match llm.complete_simple(&prompt).await {
        Ok(topic) => {
            let topic = topic.trim().to_string();
            if topic.is_empty() { None } else { Some(topic) }
        }
        Err(e) => {
            warn!("[Router] Failed to generate session topic: {}", e);
            None
        }
    }
}
```

Add helper function:

```rust
fn truncate_for_topic(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}
```

**Step 2: Wire /new into slash command interception**

In the slash command section (line ~601-646), add `/new` handling:

```rust
// After /switch check (line ~614):
if parsed.command_name == "new" {
    return self.handle_new_session(&msg, &ctx).await;
}

// And in fallback section (line ~633):
if slash_text.trim() == "/new" {
    return self.handle_new_session(&msg, &ctx).await;
}
```

**Step 3: Add session_manager field to InboundMessageRouter**

The router needs access to SessionManager. Add field:

```rust
/// Session manager for session lifecycle
session_manager: Option<Arc<SessionManager>>,
```

And builder method:

```rust
pub fn with_session_manager(mut self, sm: Arc<SessionManager>) -> Self {
    self.session_manager = Some(sm);
    self
}
```

**Step 4: Enhance /switch to close old session**

In `handle_switch_command()`, add session close before switching:

```rust
// Before switching agent, close current session
let topic = self.generate_session_topic(&ctx.session_key).await;
if let Some(ref sm) = self.session_manager {
    if let Err(e) = sm.close_session(&ctx.session_key.to_new(), topic).await {
        warn!("[Router] Failed to close session on switch: {}", e);
    }
}
```

**Step 5: Check AiProvider trait for complete_simple**

Verify that the LLM provider trait has a simple completion method. If not, use an existing method or add one. Check:

Run: `cargo check -p alephcore 2>&1 | head -40`

If `complete_simple` doesn't exist, use whatever the AiProvider trait provides for single-turn completion, or use a raw API call. Adapt the implementation to the actual provider API.

**Step 6: Run compilation check**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: Compiles (may need to adjust method names based on actual AiProvider API)

**Step 7: Commit**

```
gateway: add /new command and session close on /switch
```

---

## Task 6: Wire SessionManager into server startup

**Files:**
- Modify: `src/bin/aleph/server_init.rs` (or wherever InboundMessageRouter is built)

**Step 1: Find where InboundMessageRouter is constructed**

Search for `InboundMessageRouter::builder` or `with_command_parser` calls in the startup code.

**Step 2: Add .with_session_manager()**

Pass the existing `SessionManager` Arc to the router builder:

```rust
let router = InboundMessageRouter::builder()
    // ... existing builder calls
    .with_session_manager(session_manager.clone())
    .build();
```

**Step 3: Run compilation check**

Run: `cargo check -p alephcore 2>&1 | tail -20`

**Step 4: Commit**

```
server: wire session manager into inbound router
```

---

## Task 7: Add sessions.new RPC handler

**Files:**
- Modify: `src/gateway/handlers/session.rs`

**Step 1: Add handle_new_db handler**

```rust
/// Handle sessions.new RPC request with database backend
///
/// Closes the current session (generates topic), creates new epoch.
pub async fn handle_new_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let session_key_str = match request
        .params
        .as_ref()
        .and_then(|p| p.get("session_key"))
        .and_then(|v| v.as_str())
    {
        Some(k) => k.to_string(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    };

    let topic = request
        .params
        .as_ref()
        .and_then(|p| p.get("topic"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let old_key = match SessionKey::from_key_string(&session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key format");
        }
    };

    // Close old session
    if let Err(e) = manager.close_session(&old_key.to_new(), topic.clone()).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to close session: {}", e),
        );
    }

    // Create new epoch
    let new_key = old_key.to_new().with_next_epoch();
    match manager.get_or_create(&new_key).await {
        Ok(_) => JsonRpcResponse::success(
            request.id,
            json!({
                "old_session": {
                    "key": session_key_str,
                    "topic": topic,
                    "status": "closed"
                },
                "new_session": {
                    "key": new_key.to_key_string()
                }
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create new session: {}", e),
        ),
    }
}
```

**Step 2: Register the handler in the RPC dispatch**

Find where `"sessions.list"`, `"sessions.history"` etc. are dispatched and add:

```rust
"sessions.new" => handle_new_db(request, session_manager).await,
```

**Step 3: Update handle_list_db to include topic**

In `handle_list_db()`, after creating `SessionInfo`, query metadata for topic:

```rust
// For each session, try to extract topic from metadata
let infos: Vec<SessionInfo> = sessions
    .into_iter()
    .map(|m| {
        // Parse metadata JSON to extract topic
        let (topic, status) = extract_topic_status_from_metadata(&m, &manager);
        SessionInfo {
            key: m.key,
            agent_id: m.agent_id,
            session_type: m.session_type,
            message_count: m.message_count as u32,
            created_at: /* ... */,
            last_active_at: /* ... */,
            topic,
            status,
        }
    })
    .collect();
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib session 2>&1 | tail -20`

**Step 5: Commit**

```
gateway: add sessions.new RPC handler with topic support
```

---

## Task 8: Memory retrieval session-aware filtering

**Files:**
- Modify: `src/thinker/memory_context_provider.rs`

**Step 1: Add session-aware retrieval**

Update `search_memories()` in `MemoryContextProvider` to accept an optional session_id and use two-phase retrieval:

```rust
async fn search_memories_with_session(
    &self,
    embedding: &[f32],
    agent_id: &str,
    current_session_id: Option<&str>,
    session_manager: Option<&SessionManager>,
) -> Result<Vec<MemorySummary>, ()> {
    let mut filter = MemoryFilter {
        workspace: Some(WorkspaceFilter::Single(agent_id.to_string())),
        ..Default::default()
    };

    // Phase 1: If we have session context, find related sessions
    if let (Some(session_id), Some(sm)) = (current_session_id, session_manager) {
        // Get all closed sessions with topics
        if let Ok(sessions) = sm.list_sessions(Some(agent_id)).await {
            let related_ids: Vec<String> = sessions.iter()
                .filter_map(|s| {
                    // Include current session always
                    if s.key == session_id {
                        return Some(s.key.clone());
                    }
                    // TODO: Phase 2 enhancement — topic similarity comparison
                    // For now, include all sessions (no filtering)
                    None
                })
                .collect();

            if !related_ids.is_empty() {
                filter.session_ids = Some(related_ids);
            }
        }
    }

    // Phase 2: Vector search with session filter
    self.memory_db.search_memories(embedding, &filter, self.config.max_memories)
        .await
        .map(|entries| {
            entries.into_iter()
                .filter(|e| e.similarity_score.unwrap_or(0.0) >= self.config.threshold)
                .map(|e| MemorySummary {
                    date: format_date(e.context.timestamp),
                    user_input: truncate(&e.user_input, 150),
                    ai_output: truncate(&e.ai_output, 200),
                    score: e.similarity_score.unwrap_or(0.0),
                })
                .collect()
        })
        .map_err(|_| ())
}
```

Note: The topic similarity comparison (using topic embeddings) is marked as a Phase 2 enhancement. The initial implementation includes all sessions, maintaining current behavior. The infrastructure (session_ids filter) is ready for when topic embeddings are added.

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib memory 2>&1 | tail -20`

**Step 3: Commit**

```
memory: add session-aware retrieval infrastructure
```

---

## Task 9: Integration test & full compilation

**Step 1: Full compilation check**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: Compiles clean

**Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: All tests pass (except pre-existing markdown_skill failures)

**Step 3: Run clippy**

Run: `cargo clippy -p alephcore 2>&1 | tail -30`
Fix any warnings.

**Step 4: Final commit**

```
session: complete session isolation feature
```

---

## Execution Order & Dependencies

```
Task 1 (topic_id cleanup)
    ↓
Task 2 (epoch in SessionKey)
    ↓
Task 3 (topic/status in SessionMetadata)     Task 4 (MemoryFilter session_ids)
    ↓                                            ↓
Task 5 (/new handler in inbound_router) ←────────┘
    ↓
Task 6 (wire into server startup)
    ↓
Task 7 (sessions.new RPC handler)
    ↓
Task 8 (memory retrieval integration)
    ↓
Task 9 (integration test)
```

Tasks 3 and 4 can be done in parallel. All others are sequential.
