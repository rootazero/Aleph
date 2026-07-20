# Discord Channel Redesign - Design Document

**Status**: Draft
**Created**: 2026-04-15
**Author**: Sisyphus (Aleph Team)
**Review**: Pending user approval

---

## 1. Overview

### 1.1 Problem Statement

Aleph's Discord integration is functionally incomplete compared to openclaw:
- Missing multi-account (multi-bot-instance) support
- Missing per-guild/channel configuration overrides
- Missing interaction components (buttons, select menus)
- Missing streaming message preview
- Missing security audit infrastructure

### 1.2 Design Philosophy

- **融合优于照搬**: Learn from openclaw's plugin architecture, but adapt to Rust/Aleph idioms
- **增量重构**: No big-bang rewrites; each phase is independently verifiable
- **配置即代码**: Leverage Rust's type-safe config with serde deserialization
- **接口纯净**: R4 compliance - interface layers are pure I/O

### 1.3 Reference Implementations

| Project | Key Insight |
|---------|-------------|
| OpenClaw | 366-line nested config with per-guild/channel override, 20+ adapter traits |
| Aleph Telegram | Multi-bot-instance support via `BotPool` pattern |
| Aleph现有Discord | 824行简化实现，基础消息处理 + slash commands |

---

## 2. Target Architecture

### 2.1 Config Hierarchy

```
DiscordChannelConfig
├── default: DiscordChannelSettings          # 全局默认设置
├── accounts: HashMap<String, AccountConfig> # bot token -> account settings
│   └── [account_id]: AccountConfig
│       ├── token: String
│       ├── default_settings: DiscordChannelSettings
│       └── guilds: HashMap<u64, DiscordGuildSettings>  # per-guild override
│           └── [guild_id]: DiscordGuildSettings
│               └── channels: HashMap<u64, DiscordChannelSettings>  # per-channel override
│
└── security: DiscordSecurityConfig           # 安全审计配置
```

### 2.2 Account-Pool Pattern (参考 Telegram)

```rust
// 核心抽象：AccountPool
struct DiscordAccountPool {
    accounts: HashMap<String, Arc<DiscordBot>>,
    config: DiscordChannelConfig,
}

struct DiscordBot {
    client: Arc<DiscordClient>,
    config: AccountConfig,
    event_handlers: Vec<Arc<dyn EventHandler>>,
}

impl DiscordAccountPool {
    fn create_bot(config: AccountConfig) -> Result<Arc<DiscordBot>> { ... }
    fn get_or_create(account_id: &str) -> Result<Arc<DiscordBot>> { ... }
}
```

### 2.3 Resolver Chain

```
Message -> DiscordResolver
         ├── AccountResolver (根据 channel_id 找到 account_id)
         ├── GuildResolver (获取 guild settings)
         ├── ChannelResolver (获取 channel settings)
         └── SecurityResolver (安全检查)
```

### 2.4 Feature Modules

```
discord/
├── mod.rs              # DiscordChannel 主入口
├── config.rs           # 配置类型定义
├── account_pool.rs     # 多账号管理 (BotPool pattern)
├── resolver/
│   ├── mod.rs
│   ├── account.rs      # 账号解析
│   ├── guild.rs        # guild 解析
│   └── channel.rs      # channel 解析
├── handlers/
│   ├── mod.rs
│   ├── message.rs      # 消息处理
│   ├── interaction.rs  # 按钮/选择菜单
│   ├── streaming.rs    # streaming 状态
│   └── thread.rs       # thread binding
├── security/
│   ├── mod.rs
│   ├── audit.rs        # 审计日志
│   └── policy.rs       # 安全策略
└── api.rs              # REST API 封装 (现有)
```

---

## 3. Config Types

### 3.1 Core Settings

```rust
#[derive(Clone, Deserialize)]
pub struct DiscordChannelSettings {
    // 基础设置
    pub prefix: Option<String>,
    pub allowlist: Vec<u64>,           // allowed channel ids
    pub blocklist: Vec<u64>,           // blocked channel ids

    // 功能开关
    pub features: DiscordFeatures,

    // 线程绑定
    #[serde(default)]
    pub thread_binding: ThreadBindingConfig,

    // 回复设置
    #[serde(default)]
    pub reply: ReplyConfig,

    // Typing indicator
    #[serde(default = "default_true")]
    pub typing_indicator: bool,
}

#[derive(Clone, Deserialize)]
pub struct DiscordFeatures {
    #[serde(default)]
    pub reactions: bool,              // 启用 reactions
    #[serde(default)]
    pub slash_commands: bool,          // 启用 slash commands
    #[serde(default)]
    pub interactions: bool,            // 按钮/选择菜单
    #[serde(default)]
    pub streaming_preview: bool,       // streaming 状态预览
    #[serde(default)]
    pub plural_kit: bool,              // PluralKit 支持
    #[serde(default)]
    pub exec_approval: bool,           // exec 审批流程
}
```

### 3.2 Per-Account Config

```rust
#[derive(Clone, Deserialize)]
pub struct AccountConfig {
    pub token: String,
    pub application_id: u64,
    pub default_settings: DiscordChannelSettings,
    pub guilds: HashMap<u64, DiscordGuildSettings>,
}

#[derive(Clone, Deserialize)]
pub struct DiscordGuildSettings {
    pub name: String,
    pub settings: DiscordChannelSettings,  // guild-level override
    pub channels: HashMap<u64, DiscordChannelSettings>,  // channel-level override
}
```

### 3.3 Security Config

```rust
#[derive(Clone, Deserialize)]
pub struct DiscordSecurityConfig {
    #[serde(default = "default_true")]
    pub audit_enabled: bool,

    #[serde(default)]
    pub audit_channels: Vec<u64>,      // 审计日志发往的 channel

    #[serde(default)]
    pub audit_events: AuditEvents,

    #[serde(default)]
    pub content_retention: ContentRetention,
}

#[derive(Clone, Deserialize)]
pub struct AuditEvents {
    #[serde(default = "default_true")]
    pub commands: bool,

    #[serde(default)]
    pub exec_approvals: bool,

    #[serde(default)]
    pub message_content: bool,          // 需要 content_retention
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContentRetention {
    Full,           // 保留全文
    Anonymized,     // 移除 user_id, channel_id
    MetadataOnly,   // 仅保留元数据
}
```

### 3.4 Thread Binding Config

```rust
#[derive(Clone, Deserialize)]
pub struct ThreadBindingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_false")]
    pub allow_sub_agents: bool,        // 允许子 agent 参与

    #[serde(default = "default_prefix")]
    pub command_prefix: String,        // e.g., "/focus"
}
```

---

## 4. Account Pool Implementation

### 4.1 BotPool Trait

```rust
/// 多账号池化接口 (参考 Telegram 的 BotPool)
trait BotPool<C: ChannelConfig> {
    type Bot: ChannelBot;

    fn create_bot(&self, config: C) -> Result<Self::Bot>;
    fn get_bot(&self, account_id: &str) -> Option<Self::Bot>;
    fn remove_bot(&self, account_id: &str) -> Result<()>;
    fn list_bots(&self) -> Vec<String>;
}
```

### 4.2 DiscordAccountPool

```rust
pub struct DiscordAccountPool {
    config: DiscordChannelConfig,
    bots: RwLock<HashMap<String, Arc<DiscordBot>>>,
    http_client: Arc<HttpClient>,
}

impl DiscordAccountPool {
    pub fn new(config: DiscordChannelConfig) -> Self { ... }

    pub fn get_or_create(&self, account_id: &str) -> Result<Arc<DiscordBot>> {
        let bots = self.bots.read().unwrap();
        if let Some(bot) = bots.get(account_id) {
            return Ok(bot.clone());
        }
        drop(bots);

        let account_config = self.config.accounts.get(account_id)
            .ok_or_else(|| Error::AccountNotFound(account_id.to_string()))?;

        let bot = self.create_bot(account_config.clone())?;
        let mut bots = self.bots.write().unwrap();
        bots.insert(account_id.to_string(), bot.clone());
        Ok(bot)
    }
}
```

---

## 5. Resolver Chain

### 5.1 Trait Definition

```rust
/// 配置解析器 trait
trait ConfigResolver {
    type Config;
    fn resolve(&self, ctx: &ResolverContext) -> Result<Self::Config>;
}

/// 上下文信息
pub struct ResolverContext {
    pub channel_id: u64,
    pub guild_id: Option<u64>,
    pub account_id: String,
    pub message: Option<&Message>,
}
```

### 5.2 AccountResolver

```rust
pub struct AccountResolver {
    channel_to_account: HashMap<u64, String>,
}

impl AccountResolver {
    pub fn new(config: &DiscordChannelConfig) -> Self {
        let mut map = HashMap::new();
        // 构建 channel_id -> account_id 的反向索引
        for (account_id, account) in &config.accounts {
            for (guild_id, guild) in &account.guilds {
                for (channel_id, _) in &guild.channels {
                    map.insert(*channel_id, account_id.clone());
                }
            }
        }
        Self { channel_to_account: map }
    }
}

impl ConfigResolver for AccountResolver {
    type Config = AccountConfig;

    fn resolve(&self, ctx: &ResolverContext) -> Result<Self::Config> {
        let account_id = self.channel_to_account.get(&ctx.channel_id)
            .ok_or_else(|| Error::ChannelNotInAnyAccount(ctx.channel_id))?;
        // 从全局配置中获取（缓存由 AccountPool 处理）
        // ...
    }
}
```

---

## 6. Feature Implementation

### 6.1 Interaction Components (Buttons/Select Menus)

```rust
// handlers/interaction.rs

#[derive(Clone)]
pub struct InteractionHandler {
    approval_queue: Arc<ApprovalQueue>,
}

impl InteractionHandler {
    pub async fn handle(&self, i: Interaction) -> Result<()> {
        match i {
            Interaction::MessageComponent(component) => {
                match component.data.component_type {
                    2 => self.handle_button(component).await?,  // Button
                    3..=5 => self.handle_select_menu(component).await?,  // SelectMenu
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_button(&self, btn: MessageComponent) -> Result<()> {
        let custom_id = btn.data.custom_id.as_str();
        if let Some(approval_id) = custom_id.strip_prefix("exec_approve:") {
            self.approval_queue.approve(approval_id).await?;
        } else if let Some(approval_id) = custom_id.strip_prefix("exec_deny:") {
            self.approval_queue.deny(approval_id).await?;
        }
        Ok(())
    }
}
```

### 6.2 Streaming Preview

```rust
// handlers/streaming.rs

#[derive(Clone)]
pub struct StreamingHandler {
    cache: Arc<StreamingCache>,
}

impl StreamingHandler {
    pub async fn handle_presence_update(&self, p: PresenceUpdate) -> Result<()> {
        if let Some(activity) = &p.activities.first() {
            if let ActivityType::Streaming = activity.kind {
                let preview = StreamingPreview {
                    user_id: p.user.id,
                    username: p.user.username.clone(),
                    stream_url: activity.url.clone(),
                    title: activity.name.clone(),
                    viewer_count: activity.details.as_ref()
                        .and_then(|d| d.get("viewer_count"))
                        .and_then(|v| v.as_i64()),
                };
                self.cache.set(p.user.id, preview).await;
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct StreamingPreview {
    pub user_id: u64,
    pub username: String,
    pub stream_url: String,
    pub title: String,
    pub viewer_count: Option<i64>,
}
```

### 6.3 Thread Binding (with Sub-agents)

```rust
// handlers/thread.rs

#[derive(Clone)]
pub struct ThreadBindingHandler {
    bindings: Arc<RwLock<HashMap<u64, ThreadBinding>>>,
}

#[derive(Clone)]
pub struct ThreadBinding {
    pub parent_message_id: u64,
    pub thread_id: u64,
    pub guild_id: u64,
    pub channel_id: u64,
    pub participants: Vec<AgentId>,      // 参与的 agent
    pub created_at: DateTime<Utc>,
}

impl ThreadBindingHandler {
    pub async fn create_binding(&self, msg: &Message, agent_id: AgentId) -> Result<u64> {
        let thread = msg.channel_id.create_thread(CreateThread {
            name: format!("讨论-{}", msg.id),
            auto_archive_duration: Some(1440),
            ..Default::default()
        }).await?;

        let binding = ThreadBinding {
            parent_message_id: msg.id,
            thread_id: thread.id,
            guild_id: msg.guild_id.unwrap(),
            channel_id: msg.channel_id,
            participants: vec![agent_id],
            created_at: Utc::now(),
        };

        self.bindings.write().unwrap()
            .insert(thread.id, binding);

        Ok(thread.id)
    }

    pub async fn add_participant(&self, thread_id: u64, agent_id: AgentId) -> Result<()> {
        let mut bindings = self.bindings.write().unwrap();
        if let Some(binding) = bindings.get_mut(&thread_id) {
            if !binding.participants.contains(&agent_id) {
                binding.participants.push(agent_id);
            }
        }
        Ok(())
    }
}
```

---

## 7. Security Audit

### 7.1 Audit Event

```rust
#[derive(Serialize)]
pub struct DiscordAuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub account_id: String,
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub user_id: u64,
    pub metadata: AuditMetadata,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    CommandExecuted,
    ExecApprovalRequested,
    ExecApproved,
    ExecDenied,
    MessageReceived,
    InteractionReceived,
}

#[derive(Serialize)]
pub struct AuditMetadata {
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub content_preview: Option<String>,
    pub success: Option<bool>,
}
```

### 7.2 Audit Logger

```rust
pub struct DiscordAuditLogger {
    config: DiscordSecurityConfig,
    http_client: Arc<HttpClient>,
}

impl DiscordAuditLogger {
    pub async fn log(&self, event: DiscordAuditEvent) -> Result<()> {
        if !self.should_log(&event.event_type) {
            return Ok(());
        }

        let sanitized = self.sanitize(event);
        let payload = self.format_payload(sanitized);

        for channel_id in &self.config.audit_channels {
            self.http_client.send_message(*channel_id, &payload).await?;
        }

        Ok(())
    }

    fn sanitize(&self, mut event: DiscordAuditEvent) -> DiscordAuditEvent {
        match self.config.content_retention {
            ContentRetention::Full => {}
            ContentRetention::Anonymized => {
                event.user_id = 0;
                event.channel_id = 0;
            }
            ContentRetention::MetadataOnly => {
                event.metadata.content_preview = None;
            }
        }
        event
    }
}
```

---

## 8. Implementation Phases

### Phase 1: Config & Account Pool (Week 1)

| Task | Files | Description |
|------|-------|-------------|
| 重构配置类型 | `config.rs` | 添加嵌套配置结构 |
| 实现 AccountPool | `account_pool.rs` | 多 bot 实例管理 |
| 实现 Resolver Chain | `resolver/*.rs` | 配置解析 |
| 迁移现有代码 | `mod.rs` | 适配新配置 |

### Phase 2: Feature Enhancement (Week 2)

| Task | Files | Description |
|------|-------|-------------|
| Interaction Handler | `handlers/interaction.rs` | 按钮/选择菜单 |
| Streaming Handler | `handlers/streaming.rs` | streaming 状态 |
| Thread Binding | `handlers/thread.rs` | 增强版，支持子 agent |
| Approval Queue | `handlers/approval.rs` | exec 审批 |

### Phase 3: Security & Cleanup (Week 3)

| Task | Files | Description |
|------|-------|-------------|
| Audit Logger | `security/audit.rs` | 安全审计 |
| Security Policy | `security/policy.rs` | 策略引擎 |
| 代码清理 | `mod.rs`, `api.rs` | 移除冗余代码 |
| 测试完善 | `tests/` | 单元测试 + 集成测试 |

---

## 9. API Changes

### 9.1 New Public API

```rust
// In discord/mod.rs

impl DiscordChannel {
    // 多账号管理
    pub fn account_pool(&self) -> &DiscordAccountPool;

    // 配置查询
    pub fn resolve_settings(&self, channel_id: u64) -> Result<DiscordChannelSettings>;

    // Thread binding (增强)
    pub fn create_thread_binding(&self, msg: &Message, agent_id: AgentId) -> Result<u64>;
    pub fn add_thread_participant(&self, thread_id: u64, agent_id: AgentId) -> Result<()>;
}
```

### 9.2 Breaking Changes

None. 所有改动都是向后兼容的增量式重构。

---

## 10. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| 配置迁移破坏现有 bot | Medium | High | Phase 1 完成后测试现有账号 |
| Resolver 性能瓶颈 | Low | Medium | 缓存 account_id -> settings 映射 |
| 审计日志 channel 满 | Low | Low | 添加 channel 容量检查 + 告警 |
| 子 agent thread 竞争 | Medium | Medium | 使用 RwLock + 乐观并发控制 |

---

## 11. Open Questions

1. [x] 多账号模式：多 bot instance（与 Telegram 一致）
2. [x] Per-channel 配置覆盖：命令白名单、prefix、权限
3. [x] Thread Binding：支持子 agent
4. [x] 安全审计：命令执行、exec 审批、消息内容（可选脱敏）

---

## 12. Appendix: File List

```
src/gateway/interfaces/discord/
├── mod.rs              # [修改] DiscordChannel 主入口
├── config.rs           # [重构] 配置类型定义
├── api.rs              # [保留] REST API 封装
├── account_pool.rs     # [新增] 多账号管理
├── resolver/
│   ├── mod.rs          # [新增] Resolver trait
│   ├── account.rs      # [新增] 账号解析
│   ├── guild.rs        # [新增] guild 解析
│   └── channel.rs      # [新增] channel 解析
├── handlers/
│   ├── mod.rs          # [新增] Handler trait
│   ├── message.rs      # [修改] 消息处理
│   ├── interaction.rs  # [新增] 交互组件
│   ├── streaming.rs    # [新增] streaming
│   ├── thread.rs       # [修改] 线程绑定
│   └── approval.rs     # [新增] exec 审批
└── security/
    ├── mod.rs          # [新增] Security 模块
    ├── audit.rs        # [新增] 审计日志
    └── policy.rs       # [新增] 安全策略
```

---

**Next Step**: 用户批准后，调用 `writing-plans` skill 生成实施计划。
