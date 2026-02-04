# Phase 6B: iMessage Channel Implementation

**Date**: 2026-01-28
**Status**: In Progress
**Duration**: 2-3 weeks

---

## Moltbot Reference Analysis

Moltbot 使用 **外部 CLI (`imsg`)** + **JSON-RPC over stdio** 模式：
- 不直接操作 Messages.app
- 通过 `imsg rpc` 子进程通信
- 支持远程 SSH 部署

### Aleph 实现策略

我们采用 **Rust 原生实现**：

| 功能 | 实现方式 | 原因 |
|------|----------|------|
| **发送消息** | AppleScript | 简单可靠，无需额外依赖 |
| **接收消息** | SQLite 轮询 (chat.db) | 实时性好，Rust 原生支持 |
| **附件处理** | 文件系统 + MIME 检测 | 直接访问 ~/Library/Messages/Attachments |

**需要的权限**:
- Full Disk Access (读取 chat.db)
- Automation (发送 AppleScript)

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    iMessage Channel                          │
│                                                              │
│  ┌──────────────────┐     ┌──────────────────────────────┐ │
│  │  MessageSender    │     │     MessageReceiver          │ │
│  │  (AppleScript)    │     │  (SQLite Polling)            │ │
│  │                   │     │                              │ │
│  │  send_text()      │     │  poll_new_messages()         │ │
│  │  send_file()      │     │  parse_attachments()         │ │
│  └──────────────────┘     └──────────────────────────────┘ │
│           │                           │                      │
│           └───────────┬───────────────┘                      │
│                       │                                      │
│              ┌────────▼────────┐                            │
│              │  IMessageChannel │                            │
│              │  impl Channel    │                            │
│              └─────────────────┘                            │
│                       │                                      │
└───────────────────────┼──────────────────────────────────────┘
                        │
              ┌─────────▼─────────┐
              │  ChannelRegistry   │
              └───────────────────┘
```

---

## Implementation Tasks

### Task 1: Messages Database Reader

读取 `~/Library/Messages/chat.db` (SQLite)

**Key Tables**:
- `message` - 消息内容、时间戳、发送者
- `chat` - 会话信息
- `handle` - 联系人 ID (电话号码/email)
- `message_attachment_join` - 消息-附件关联
- `attachment` - 附件元数据

**Schema (简化)**:
```sql
-- message table
CREATE TABLE message (
    ROWID INTEGER PRIMARY KEY,
    guid TEXT UNIQUE,
    text TEXT,
    handle_id INTEGER,
    date INTEGER,           -- Apple timestamp (nanoseconds since 2001-01-01)
    is_from_me INTEGER,
    is_read INTEGER,
    cache_has_attachments INTEGER
);

-- chat table
CREATE TABLE chat (
    ROWID INTEGER PRIMARY KEY,
    guid TEXT UNIQUE,
    chat_identifier TEXT,   -- phone number or group ID
    display_name TEXT,
    group_id TEXT
);

-- handle table
CREATE TABLE handle (
    ROWID INTEGER PRIMARY KEY,
    id TEXT,                -- phone number or email
    service TEXT            -- "iMessage" or "SMS"
);
```

**Implementation**:
```rust
// core/src/gateway/channels/imessage/db.rs
pub struct MessagesDb {
    conn: rusqlite::Connection,
    last_message_id: i64,
}

impl MessagesDb {
    pub fn open() -> Result<Self>;
    pub fn poll_new_messages(&mut self) -> Result<Vec<RawMessage>>;
    pub fn get_chat_info(&self, chat_id: i64) -> Result<ChatInfo>;
    pub fn get_handle(&self, handle_id: i64) -> Result<Handle>;
}
```

### Task 2: AppleScript Message Sender

使用 `osascript` 发送消息。

**AppleScript Template**:
```applescript
tell application "Messages"
    set targetService to 1st account whose service type = iMessage
    set targetBuddy to participant "{phone_number}" of targetService
    send "{message}" to targetBuddy
end tell
```

**Implementation**:
```rust
// core/src/gateway/channels/imessage/sender.rs
pub struct MessageSender;

impl MessageSender {
    pub async fn send_text(to: &str, text: &str) -> Result<()>;
    pub async fn send_file(to: &str, file_path: &Path) -> Result<()>;
}
```

### Task 3: iMessage Channel Implementation

实现 `Channel` trait。

```rust
// core/src/gateway/channels/imessage/mod.rs
pub struct IMessageChannel {
    info: ChannelInfo,
    db: Arc<Mutex<MessagesDb>>,
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    poll_interval: Duration,
    running: Arc<AtomicBool>,
}

#[async_trait]
impl Channel for IMessageChannel {
    async fn start(&mut self) -> ChannelResult<()>;
    async fn stop(&mut self) -> ChannelResult<()>;
    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult>;
    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>>;
}
```

### Task 4: Target Parsing

支持多种地址格式（参考 Moltbot）。

```rust
// core/src/gateway/channels/imessage/target.rs

/// iMessage target formats:
/// - "+15551234567" (phone number)
/// - "user@example.com" (email)
/// - "chat_id:123" (group chat by ID)
/// - "imessage:+15551234567" (force iMessage)
/// - "sms:+15551234567" (force SMS)
pub enum IMessageTarget {
    Phone { number: String, service: Service },
    Email { email: String },
    ChatId { id: i64 },
}

pub enum Service {
    Auto,
    IMessage,
    Sms,
}

pub fn parse_target(target: &str) -> Result<IMessageTarget>;
pub fn normalize_phone(phone: &str) -> String;
```

### Task 5: Channel Factory & Registration

注册到 ChannelRegistry。

```rust
// core/src/gateway/channels/imessage/factory.rs
pub struct IMessageChannelFactory;

#[async_trait]
impl ChannelFactory for IMessageChannelFactory {
    fn channel_type(&self) -> &str { "imessage" }
    async fn create(&self, config: Value) -> ChannelResult<Box<dyn Channel>>;
}
```

---

## File Structure

```
core/src/gateway/channels/
├── mod.rs                    # Channel exports
├── cli.rs                    # CLI channel (existing)
└── imessage/
    ├── mod.rs                # IMessageChannel implementation
    ├── db.rs                 # SQLite database reader
    ├── sender.rs             # AppleScript message sender
    ├── target.rs             # Target parsing & normalization
    ├── factory.rs            # Channel factory
    └── config.rs             # Configuration types
```

---

## Configuration

```toml
# ~/.aleph/gateway.toml
[channels.imessage]
enabled = true
db_path = "~/Library/Messages/chat.db"
poll_interval_ms = 1000
dm_policy = "pairing"           # pairing | allowlist | open
allow_from = ["+15551234567"]   # allowlist entries
```

---

## Testing Plan

### Unit Tests
1. Target parsing (phone numbers, emails, chat IDs)
2. Apple timestamp conversion
3. Message text sanitization

### Integration Tests
1. Database connection and query
2. AppleScript execution (requires Messages.app)
3. Full send/receive cycle

### Manual Testing
```bash
# Start gateway with iMessage channel
cargo run --features gateway --bin aleph-gateway

# Send test message via Gateway
echo '{"jsonrpc":"2.0","method":"channel.send","params":{"channel":"imessage","to":"+15551234567","text":"Hello from Aleph!"},"id":1}' | websocat ws://127.0.0.1:18789
```

---

## Security Considerations

1. **Full Disk Access** - Required for reading chat.db
2. **Automation Permission** - Required for AppleScript
3. **DM Pairing** - Unknown senders get pairing code
4. **Allowlist** - Optional whitelist for authorized contacts

---

## Implementation Order

1. **Day 1-2**: Task 1 (Database Reader)
   - Open and query chat.db
   - Parse messages, handles, chats
   - Apple timestamp conversion

2. **Day 3-4**: Task 2 (AppleScript Sender)
   - Send text messages
   - Send file attachments
   - Error handling

3. **Day 5-6**: Task 3 (Channel Implementation)
   - Implement Channel trait
   - Message polling loop
   - Inbound message conversion

4. **Day 7-8**: Task 4 & 5 (Target Parsing & Factory)
   - Parse target formats
   - Register with ChannelRegistry
   - Configuration loading

5. **Day 9-10**: Testing & Polish
   - Integration tests
   - Documentation
   - Error handling improvements

---

## Success Criteria

- [x] Can read new messages from chat.db
- [x] Can send messages via AppleScript
- [x] Polling loop detects new messages in <2s
- [x] Supports phone numbers and email addresses
- [x] Supports group chats (by chat_id)
- [x] Attachments are included with messages
- [x] Configuration via gateway.toml
- [x] Registered in ChannelRegistry via IMessageChannelFactory

---

## Implementation Progress (2026-01-28)

### Completed

**Task 1: Messages Database Reader** - DONE
- `db.rs`: MessagesDb struct with poll_new_messages()
- Apple timestamp conversion
- Handle/Chat/Attachment info queries

**Task 2: AppleScript Message Sender** - DONE
- `sender.rs`: send_text(), send_file(), send_to_chat()
- String escaping for AppleScript
- Messages.app availability check

**Task 3: Channel Implementation** - DONE
- `mod.rs`: IMessageChannel implementing Channel trait
- start/stop lifecycle
- Message polling loop
- Send via AppleScript

**Task 4: Target Parsing** - DONE
- `target.rs`: parse_target(), normalize_phone()
- Supports: phone, email, chat_id, chat_guid
- Service prefixes: imessage:, sms:

**Task 5: Configuration** - DONE
- `config.rs`: IMessageConfig struct
- DM policy: pairing, allowlist, open
- Group policy: open, allowlist, disabled

### Files Created

```
core/src/gateway/channels/imessage/
├── mod.rs      (165 lines) - Channel implementation
├── db.rs       (220 lines) - SQLite database reader
├── sender.rs   (175 lines) - AppleScript sender
├── target.rs   (240 lines) - Target parsing
└── config.rs   (155 lines) - Configuration
```

Total: ~955 lines of Rust code

---

## References

- Moltbot iMessage: `/Users/zouguojun/Workspace/moltbot/src/imessage/`
- Apple Messages DB schema: Various reverse-engineering docs
- AppleScript Messages dictionary: `Script Editor > File > Open Dictionary > Messages`
