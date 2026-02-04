# Phase 6: Gateway 完善与首个渠道实现

**日期:** 2026-01-28
**状态:** 规划中
**前置完成:** Phase 1-5 (Gateway 基础设施)

---

## 目标概述

完成 Gateway 核心功能，实现首个消息渠道 (iMessage)，为多渠道架构奠定基础。

**核心原则:**
- 参考 Moltbot 架构，保持 API 兼容性
- Rust 重写，追求高性能和内存安全
- 渐进式实现，每个阶段可独立验证

---

## Priority 0: 真实 AgentLoop 连接

### 目标
将 Gateway 的 `agent.run` 从模拟 echo 升级为真实 AgentLoop 执行。

### 实现步骤

#### 0.1 完善 ExecutionEngine

**文件:** `core/src/gateway/execution_engine.rs`

当前 `run_agent_loop()` 是占位实现。需要:

```rust
async fn run_agent_loop<E: EventEmitter>(...) -> Result<String, ExecutionError> {
    // 1. 获取 Provider (从 ProviderRegistry)
    let provider = self.provider_registry.get_provider(&agent.config.model)?;

    // 2. 创建 Thinker
    let thinker = Arc::new(Thinker::new(provider, ThinkerConfig::default()));

    // 3. 创建 Executor
    let executor = Arc::new(SingleStepExecutor::new(self.tool_registry.clone()));

    // 4. 创建 EventEmittingCallback
    let callback = EventEmittingCallback::new(emitter, &run_id);

    // 5. 运行 AgentLoop
    let agent_loop = AgentLoop::new(thinker, executor, compressor, config);
    let result = agent_loop.run(input, context, tools, &callback, abort_rx, history).await;

    // 6. 转换结果
    match result {
        LoopResult::Completed { summary, .. } => Ok(summary),
        LoopResult::Failed { reason, .. } => Err(ExecutionError::Failed(reason)),
        _ => Err(ExecutionError::Cancelled),
    }
}
```

#### 0.2 配置 Provider Registry

**新文件:** `core/src/gateway/provider_config.rs`

```rust
/// Gateway 专用的 Provider 配置
pub struct GatewayProviderConfig {
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub default_model: String,
}

impl GatewayProviderConfig {
    pub fn from_env() -> Self {
        Self {
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            default_model: std::env::var("AETHER_MODEL")
                .unwrap_or_else(|_| "anthropic/claude-sonnet-4".to_string()),
        }
    }
}
```

#### 0.3 更新 aleph-gateway 二进制

**文件:** `core/src/bin/aleph_gateway.rs`

```rust
// 初始化 Provider Registry
let provider_config = GatewayProviderConfig::from_env();
let provider_registry = create_provider_registry(&provider_config)?;

// 创建带真实 Provider 的 ExecutionEngine
let execution_engine = ExecutionEngine::new(provider_registry, tool_registry);

// 注册 agent.run handler
server.handlers_mut().register("agent.run", move |req| {
    let engine = execution_engine.clone();
    async move { handle_run_with_engine(req, engine).await }
});
```

### 验证

```bash
# 设置 API Key
export ANTHROPIC_API_KEY=sk-ant-...

# 启动 Gateway
cargo run --features gateway --bin aleph-gateway

# 测试真实 Agent 执行
echo '{"jsonrpc":"2.0","method":"agent.run","params":{"input":"What is 2+2?"},"id":"1"}' | websocat -t ws://127.0.0.1:18789
```

---

## Priority 1: Session 持久化

### 目标
实现 JSONL 格式的 Session 存储，支持会话历史、压缩、恢复。

### 设计 (参考 Moltbot `src/gateway/session-utils.ts`)

#### 1.1 Session 存储结构

**目录:** `~/.aleph/sessions/`

```
~/.aleph/sessions/
├── main.jsonl           # 主会话
├── telegram_123.jsonl   # Telegram DM
├── discord_456.jsonl    # Discord channel
└── imessage_789.jsonl   # iMessage
```

**JSONL 格式:**
```jsonl
{"type":"session","version":"1","id":"main","created_at":"2026-01-28T...","agent_id":"default"}
{"role":"user","content":"Hello","timestamp":"2026-01-28T...","channel":"gui","peer_id":"local"}
{"role":"assistant","content":"Hi!","timestamp":"2026-01-28T...","tool_use":[],"thinking":"..."}
{"role":"user","content":"What's the weather?","timestamp":"2026-01-28T..."}
{"role":"assistant","content":"Let me check...","tool_use":[{"name":"web_search","params":{}}]}
```

#### 1.2 Session Manager 实现

**新文件:** `core/src/gateway/session_storage.rs`

```rust
use std::path::PathBuf;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct SessionStorage {
    base_dir: PathBuf,
}

impl SessionStorage {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// 加载会话消息
    pub async fn load_messages(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<Message>> {
        let path = self.session_path(session_id);
        let file = File::open(&path).await?;
        let reader = BufReader::new(file);

        let mut messages = Vec::new();
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            if let Ok(entry) = serde_json::from_str::<SessionEntry>(&line) {
                if let SessionEntry::Message(msg) = entry {
                    messages.push(msg);
                }
            }
        }

        // 应用 limit
        if let Some(n) = limit {
            messages = messages.into_iter().rev().take(n).rev().collect();
        }

        Ok(messages)
    }

    /// 追加消息
    pub async fn append_message(&self, session_id: &str, message: &Message) -> Result<()> {
        let path = self.session_path(session_id);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        let entry = SessionEntry::Message(message.clone());
        let json = serde_json::to_string(&entry)?;
        file.write_all(format!("{}\n", json).as_bytes()).await?;

        Ok(())
    }

    /// 压缩会话 (保留最近 N 条，其余生成摘要)
    pub async fn compact(&self, session_id: &str, keep_recent: usize) -> Result<()> {
        // 1. 加载所有消息
        // 2. 分离: 旧消息 | 保留消息
        // 3. 对旧消息生成摘要
        // 4. 重写文件: header + summary + recent
        todo!()
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.base_dir.join(format!("{}.jsonl", session_id))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum SessionEntry {
    #[serde(rename = "session")]
    Header(SessionHeader),
    #[serde(rename = "message")]
    Message(Message),
    #[serde(rename = "summary")]
    Summary(SessionSummary),
}
```

#### 1.3 集成到 Agent Handler

**修改:** `core/src/gateway/handlers/agent.rs`

```rust
// 在 start_run 中:
// 1. 从 storage 加载历史
let history = storage.load_messages(&session_key, Some(50)).await?;

// 2. 执行 agent
let result = engine.run(input, history, callback).await?;

// 3. 保存用户消息和助手回复
storage.append_message(&session_key, &user_message).await?;
storage.append_message(&session_key, &assistant_message).await?;
```

### 验证

```bash
# 启动 Gateway
cargo run --features gateway --bin aleph-gateway

# 发送消息
echo '{"jsonrpc":"2.0","method":"agent.run","params":{"input":"Remember my name is Alice"},"id":"1"}' | websocat -t ws://127.0.0.1:18789

# 检查 JSONL 文件
cat ~/.aleph/sessions/main.jsonl

# 再次发送，验证历史
echo '{"jsonrpc":"2.0","method":"agent.run","params":{"input":"What is my name?"},"id":"2"}' | websocat -t ws://127.0.0.1:18789
```

---

## Priority 2: Channel 抽象层

### 目标
实现可扩展的 Channel 插件系统，为多渠道接入奠定基础。

### 设计 (参考 Moltbot `src/channels/plugins/`)

#### 2.1 Channel Trait 定义

**新文件:** `core/src/channels/mod.rs`

```rust
use async_trait::async_trait;

/// 渠道能力描述
#[derive(Debug, Clone)]
pub struct ChannelCapabilities {
    pub chat_types: Vec<ChatType>,  // direct, channel, thread
    pub media: bool,
    pub reactions: bool,
    pub threads: bool,
    pub polls: bool,
}

#[derive(Debug, Clone)]
pub enum ChatType {
    Direct,
    Channel,
    Thread,
}

/// 入站消息
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub channel_id: String,
    pub account_id: String,
    pub peer_id: String,
    pub content: String,
    pub media: Option<Vec<MediaAttachment>>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub metadata: serde_json::Value,
}

/// 出站消息
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub content: String,
    pub media: Option<Vec<MediaAttachment>>,
    pub reply_to: Option<String>,
}

/// Channel 插件接口
#[async_trait]
pub trait ChannelPlugin: Send + Sync {
    /// 渠道 ID (如 "telegram", "discord", "imessage")
    fn id(&self) -> &str;

    /// 渠道能力
    fn capabilities(&self) -> ChannelCapabilities;

    /// 初始化渠道
    async fn initialize(&mut self, config: &ChannelConfig) -> Result<(), ChannelError>;

    /// 启动消息监听
    async fn start(&mut self, sender: mpsc::Sender<InboundMessage>) -> Result<(), ChannelError>;

    /// 停止渠道
    async fn stop(&mut self) -> Result<(), ChannelError>;

    /// 发送消息
    async fn send(&self, peer_id: &str, message: OutboundMessage) -> Result<(), ChannelError>;

    /// 检查配置是否有效
    fn is_configured(&self, config: &ChannelConfig) -> bool;
}
```

#### 2.2 Channel Registry

**新文件:** `core/src/channels/registry.rs`

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ChannelRegistry {
    channels: RwLock<HashMap<String, Arc<dyn ChannelPlugin>>>,
    message_tx: mpsc::Sender<InboundMessage>,
}

impl ChannelRegistry {
    pub fn new(message_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            message_tx,
        }
    }

    /// 注册渠道插件
    pub async fn register(&self, plugin: Arc<dyn ChannelPlugin>) {
        let id = plugin.id().to_string();
        self.channels.write().await.insert(id, plugin);
    }

    /// 启动所有已配置的渠道
    pub async fn start_all(&self, config: &GatewayConfig) -> Result<(), ChannelError> {
        let channels = self.channels.read().await;
        for (id, plugin) in channels.iter() {
            if let Some(channel_config) = config.channels.get(id) {
                if plugin.is_configured(channel_config) {
                    plugin.clone().start(self.message_tx.clone()).await?;
                }
            }
        }
        Ok(())
    }

    /// 发送消息到指定渠道
    pub async fn send(&self, channel_id: &str, peer_id: &str, message: OutboundMessage) -> Result<(), ChannelError> {
        let channels = self.channels.read().await;
        if let Some(plugin) = channels.get(channel_id) {
            plugin.send(peer_id, message).await
        } else {
            Err(ChannelError::NotFound(channel_id.to_string()))
        }
    }
}
```

#### 2.3 Gateway 集成

**修改:** `core/src/gateway/server.rs`

```rust
impl GatewayServer {
    pub fn with_channels(mut self, registry: Arc<ChannelRegistry>) -> Self {
        self.channel_registry = Some(registry);

        // 启动消息处理循环
        let event_bus = self.event_bus.clone();
        let registry_clone = registry.clone();
        tokio::spawn(async move {
            let mut rx = registry_clone.subscribe();
            while let Some(msg) = rx.recv().await {
                // 将入站消息转换为 agent.run 请求
                let session_key = format!("{}:{}:{}", msg.channel_id, msg.account_id, msg.peer_id);
                // ... 路由到 agent
            }
        });

        self
    }
}
```

---

## Priority 3: iMessage 渠道实现

### 目标
实现首个消息渠道 - macOS 原生 iMessage 接入。

### 设计

#### 3.1 iMessage 插件

**新文件:** `core/src/channels/imessage.rs`

```rust
use crate::channels::{ChannelPlugin, ChannelCapabilities, InboundMessage, OutboundMessage};

pub struct IMessageChannel {
    db_path: PathBuf,
    last_rowid: i64,
    poll_interval: Duration,
}

impl IMessageChannel {
    pub fn new() -> Self {
        Self {
            db_path: dirs::home_dir()
                .unwrap()
                .join("Library/Messages/chat.db"),
            last_rowid: 0,
            poll_interval: Duration::from_secs(2),
        }
    }

    /// 轮询 chat.db 获取新消息
    async fn poll_messages(&mut self) -> Result<Vec<InboundMessage>, ChannelError> {
        let conn = rusqlite::Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;

        let mut stmt = conn.prepare(r#"
            SELECT
                m.ROWID,
                m.text,
                m.date,
                m.is_from_me,
                h.id as handle_id
            FROM message m
            JOIN handle h ON m.handle_id = h.ROWID
            WHERE m.ROWID > ?
            ORDER BY m.ROWID ASC
        "#)?;

        let messages: Vec<InboundMessage> = stmt
            .query_map([self.last_rowid], |row| {
                let rowid: i64 = row.get(0)?;
                let text: String = row.get(1)?;
                let date: i64 = row.get(2)?;
                let is_from_me: bool = row.get(3)?;
                let handle_id: String = row.get(4)?;

                self.last_rowid = rowid;

                if is_from_me {
                    return Ok(None); // 跳过自己发的消息
                }

                Ok(Some(InboundMessage {
                    channel_id: "imessage".to_string(),
                    account_id: "default".to_string(),
                    peer_id: handle_id,
                    content: text,
                    media: None,
                    timestamp: parse_imessage_date(date),
                    metadata: serde_json::json!({}),
                }))
            })?
            .filter_map(|r| r.ok().flatten())
            .collect();

        Ok(messages)
    }
}

#[async_trait]
impl ChannelPlugin for IMessageChannel {
    fn id(&self) -> &str {
        "imessage"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            chat_types: vec![ChatType::Direct],
            media: true,
            reactions: true,
            threads: false,
            polls: false,
        }
    }

    async fn start(&mut self, sender: mpsc::Sender<InboundMessage>) -> Result<(), ChannelError> {
        let poll_interval = self.poll_interval;

        loop {
            let messages = self.poll_messages().await?;
            for msg in messages {
                sender.send(msg).await?;
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    async fn send(&self, peer_id: &str, message: OutboundMessage) -> Result<(), ChannelError> {
        // 使用 AppleScript 或 osascript 发送 iMessage
        let script = format!(
            r#"tell application "Messages"
                set targetService to 1st account whose service type = iMessage
                set targetBuddy to participant "{}" of targetService
                send "{}" to targetBuddy
            end tell"#,
            peer_id,
            message.content.replace("\"", "\\\"")
        );

        tokio::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .await?;

        Ok(())
    }

    fn is_configured(&self, _config: &ChannelConfig) -> bool {
        // macOS 上始终可用 (需要 Full Disk Access 权限)
        self.db_path.exists()
    }
}
```

#### 3.2 权限说明

iMessage 需要 **Full Disk Access** 权限才能读取 `~/Library/Messages/chat.db`:

1. System Preferences → Security & Privacy → Privacy
2. Full Disk Access → Add Terminal / Aleph.app
3. 或在 entitlements 中添加:
   ```xml
   <key>com.apple.security.files.user-selected.read-write</key>
   <true/>
   ```

### 验证

```bash
# 启动 Gateway (带 iMessage 渠道)
cargo run --features gateway,imessage --bin aleph-gateway

# 用 iPhone 或另一台 Mac 发送 iMessage 到本机
# 观察 Gateway 日志，应该能看到:
# [INFO] iMessage: New message from +1234567890

# 检查 agent 是否响应
cat ~/.aleph/sessions/imessage_default_+1234567890.jsonl
```

---

## Priority 4: Config 热重载

### 目标
实现配置文件热重载，支持运行时更新配置。

### 设计

#### 4.1 Config Schema (参考 Moltbot)

**新文件:** `core/src/gateway/config_schema.rs`

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GatewayConfig {
    #[serde(default)]
    pub gateway: GatewaySettings,

    #[serde(default)]
    pub agent: AgentSettings,

    #[serde(default)]
    pub channels: HashMap<String, ChannelConfig>,

    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,

    #[serde(default)]
    pub cron: Vec<CronJob>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GatewaySettings {
    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_bind")]
    pub bind: String,

    #[serde(default)]
    pub auth: AuthSettings,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AgentSettings {
    #[serde(default = "default_model")]
    pub model: String,

    #[serde(default = "default_thinking")]
    pub thinking: ThinkingLevel,

    #[serde(default)]
    pub max_loops: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}
```

#### 4.2 Hot Reload Watcher

**新文件:** `core/src/gateway/config_watcher.rs`

```rust
use notify::{Watcher, RecursiveMode, watcher};
use std::sync::mpsc::channel;
use std::time::Duration;

pub struct ConfigWatcher {
    config_path: PathBuf,
    on_reload: Box<dyn Fn(GatewayConfig) + Send + Sync>,
}

impl ConfigWatcher {
    pub fn start(config_path: PathBuf, on_reload: impl Fn(GatewayConfig) + Send + Sync + 'static) {
        let (tx, rx) = channel();

        let mut watcher = watcher(tx, Duration::from_secs(2)).unwrap();
        watcher.watch(&config_path, RecursiveMode::NonRecursive).unwrap();

        tokio::spawn(async move {
            loop {
                match rx.recv() {
                    Ok(DebouncedEvent::Write(_)) | Ok(DebouncedEvent::Create(_)) => {
                        match load_config(&config_path) {
                            Ok(config) => {
                                tracing::info!("Config reloaded");
                                on_reload(config);
                            }
                            Err(e) => {
                                tracing::error!("Config reload failed: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Watch error: {:?}", e);
                        break;
                    }
                    _ => {}
                }
            }
        });
    }
}
```

---

## 实施时间线

| 阶段 | 任务 | 预估 |
|------|------|------|
| **P0** | 真实 AgentLoop 连接 | 1-2 天 |
| **P1** | Session 持久化 (JSONL) | 1 天 |
| **P2** | Channel 抽象层 | 1 天 |
| **P3** | iMessage 渠道实现 | 1-2 天 |
| **P4** | Config 热重载 | 0.5 天 |

**总计:** 4-6 天

---

## 验收标准

### P0 验收
- [ ] `agent.run` 调用真实 Claude API
- [ ] 流式返回 reasoning + tool_use + response
- [ ] API Key 从环境变量或配置文件读取

### P1 验收
- [ ] 消息持久化到 `~/.aleph/sessions/*.jsonl`
- [ ] 支持 session 历史加载
- [ ] Agent 能记住之前的对话

### P2 验收
- [ ] Channel trait 定义完成
- [ ] ChannelRegistry 可注册/启动/停止渠道

### P3 验收
- [ ] iMessage 入站消息能触发 agent
- [ ] Agent 响应能通过 iMessage 发回
- [ ] 权限说明文档完成

### P4 验收
- [ ] 修改 `~/.aleph/config.json5` 后自动生效
- [ ] 日志显示 "Config reloaded"

---

## 参考资源

- Moltbot Gateway: `/Users/zouguojun/Workspace/moltbot/src/gateway/`
- Moltbot Channels: `/Users/zouguojun/Workspace/moltbot/src/channels/`
- Moltbot Config: `/Users/zouguojun/Workspace/moltbot/src/config/`
- Aleph CLAUDE.md: `/Users/zouguojun/Workspace/Aleph/CLAUDE.md`
