# Discord Channel Redesign - Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 Discord channel 多账号支持和功能增强（per-guild/channel 配置、交互组件、streaming、thread binding、安全审计）

**Architecture:** 采用配置继承链 + AccountPool 模式，参考 openclaw 的嵌套配置但用 Rust type-safe 方式实现

**Tech Stack:** Rust (serenity, tokio, serde, thiserror), Aleph gateway interfaces

---

## File Structure

```
src/gateway/interfaces/discord/
├── mod.rs              # [修改] DiscordChannel 主入口
├── config.rs           # [重构] 配置类型定义 (新增嵌套结构)
├── api.rs              # [保留] REST API 封装
├── account_pool.rs     # [新增] 多账号管理 (BotPool pattern)
├── resolver/
│   ├── mod.rs          # [修改] Resolver trait
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

## Phase 1: Config & Account Pool

### Task 1: 重构 config.rs - 添加嵌套配置类型

**Files:**
- Modify: `src/gateway/interfaces/discord/config.rs`

- [ ] **Step 1: 读取现有 config.rs**

```rust
// 当前结构 (252行) 需要扩展
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordConfig {
    pub token: String,
    pub application_id: u64,
    pub allowlist: Vec<u64>,  // 当前是扁平结构
}
```

- [ ] **Step 2: 添加 DiscordChannelSettings**

在 config.rs 末尾添加：

```rust
/// Discord channel settings with per-guild/per-channel override support
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordChannelSettings {
    /// Command prefix (e.g., "/")
    #[serde(default)]
    pub prefix: Option<String>,

    /// Allowed channel IDs (empty = allow all)
    #[serde(default)]
    pub allowlist: Vec<u64>,

    /// Blocked channel IDs
    #[serde(default)]
    pub blocklist: Vec<u64>,

    /// Feature toggles
    #[serde(default)]
    pub features: DiscordFeatures,

    /// Thread binding configuration
    #[serde(default)]
    pub thread_binding: ThreadBindingConfig,

    /// Reply configuration
    #[serde(default)]
    pub reply: ReplyConfig,

    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub typing_indicator: bool,
}

fn default_true() -> bool { true }
```

- [ ] **Step 3: 添加 DiscordFeatures**

```rust
/// Feature toggles for Discord channel
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordFeatures {
    #[serde(default = "default_true")]
    pub reactions: bool,

    #[serde(default = "default_true")]
    pub slash_commands: bool,

    #[serde(default)]
    pub interactions: bool,

    #[serde(default)]
    pub streaming_preview: bool,

    #[serde(default)]
    pub plural_kit: bool,

    #[serde(default)]
    pub exec_approval: bool,
}

impl Default for DiscordFeatures {
    fn default() -> Self {
        Self {
            reactions: true,
            slash_commands: true,
            interactions: false,
            streaming_preview: false,
            plural_kit: false,
            exec_approval: false,
        }
    }
}
```

- [ ] **Step 4: 添加 ThreadBindingConfig**

```rust
/// Thread binding configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ThreadBindingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Allow sub-agents to participate in thread
    #[serde(default)]
    pub allow_sub_agents: bool,

    /// Command prefix for /focus command
    #[serde(default = "default_focus_prefix")]
    pub command_prefix: String,
}

fn default_focus_prefix() -> String {
    "/focus".to_string()
}

impl Default for ThreadBindingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_sub_agents: false,
            command_prefix: "/focus".to_string(),
        }
    }
}
```

- [ ] **Step 5: 添加 ReplyConfig**

```rust
/// Reply configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ReplyConfig {
    /// Auto-reply to threads
    #[serde(default = "default_true")]
    pub auto_reply: bool,

    /// Include quoted original message
    #[serde(default)]
    pub include_quotes: bool,
}

impl Default for ReplyConfig {
    fn default() -> Self {
        Self {
            auto_reply: true,
            include_quotes: false,
        }
    }
}
```

- [ ] **Step 6: 重构 DiscordConfig 为嵌套结构**

```rust
/// Discord channel configuration with multi-account support
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordChannelConfig {
    /// Default settings applied to all accounts
    #[serde(default)]
    pub default: DiscordChannelSettings,

    /// Account configurations (account_id -> AccountConfig)
    /// Multiple bot instances supported
    #[serde(default)]
    pub accounts: HashMap<String, AccountConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountConfig {
    /// Bot token
    pub token: String,

    /// Application ID (from Discord developer portal)
    pub application_id: u64,

    /// Default settings for this account
    #[serde(default)]
    pub default_settings: DiscordChannelSettings,

    /// Per-guild settings (guild_id -> DiscordGuildSettings)
    #[serde(default)]
    pub guilds: HashMap<u64, DiscordGuildSettings>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscordGuildSettings {
    pub name: String,

    /// Guild-level override settings
    #[serde(default)]
    pub settings: DiscordChannelSettings,

    /// Per-channel settings (channel_id -> DiscordChannelSettings)
    #[serde(default)]
    pub channels: HashMap<u64, DiscordChannelSettings>,
}
```

- [ ] **Step 7: 添加 ContentRetention enum**

```rust
/// Content retention policy for audit logs
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContentRetention {
    Full,           // 保留全文
    Anonymized,     // 移除 user_id, channel_id
    MetadataOnly,   // 仅保留元数据
}

impl Default for ContentRetention {
    fn default() -> Self {
        ContentRetention::Anonymized
    }
}
```

- [ ] **Step 8: 添加 DiscordSecurityConfig**

```rust
/// Security configuration for Discord channel
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordSecurityConfig {
    /// Enable security audit logging
    #[serde(default = "default_true")]
    pub audit_enabled: bool,

    /// Channel IDs to send audit logs to
    #[serde(default)]
    pub audit_channels: Vec<u64>,

    /// Which events to audit
    #[serde(default)]
    pub audit_events: AuditEvents,

    /// Content retention policy
    #[serde(default)]
    pub content_retention: ContentRetention,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuditEvents {
    #[serde(default = "default_true")]
    pub commands: bool,

    #[serde(default)]
    pub exec_approvals: bool,

    #[serde(default)]
    pub message_content: bool,
}

impl Default for AuditEvents {
    fn default() -> Self {
        Self {
            commands: true,
            exec_approvals: true,
            message_content: false,
        }
    }
}

impl Default for DiscordSecurityConfig {
    fn default() -> Self {
        Self {
            audit_enabled: false,
            audit_channels: Vec::new(),
            audit_events: AuditEvents::default(),
            content_retention: ContentRetention::default(),
        }
    }
}
```

- [ ] **Step 9: 添加 DiscordConfig (backward compatibility)**

```rust
/// Legacy flat config for backward compatibility
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordConfig {
    pub token: String,
    pub application_id: u64,
    #[serde(default)]
    pub allowlist: Vec<u64>,
}

impl From<DiscordConfig> for DiscordChannelConfig {
    fn from(legacy: DiscordConfig) -> Self {
        let default_settings = DiscordChannelSettings {
            allowlist: legacy.allowlist,
            ..Default::default()
        };

        let account_config = AccountConfig {
            token: legacy.token,
            application_id: legacy.application_id,
            default_settings,
            guilds: HashMap::new(),
        };

        let mut accounts = HashMap::new();
        accounts.insert("default".to_string(), account_config);

        Self {
            default: DiscordChannelSettings::default(),
            accounts,
        }
    }
}
```

- [ ] **Step 10: 添加 derive 宏依赖**

确保文件顶部有：

```rust
use serde::Deserialize;
use std::collections::HashMap;
```

- [ ] **Step 11: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: 无编译错误

- [ ] **Step 12: 提交**

```bash
git add src/gateway/interfaces/discord/config.rs
git commit -m "feat(discord): add nested config hierarchy for multi-account support"
```

---

### Task 2: 创建 account_pool.rs - 多账号管理

**Files:**
- Create: `src/gateway/interfaces/discord/account_pool.rs`

- [ ] **Step 1: 创建文件骨架**

```rust
//! Discord Account Pool
//!
//! Manages multiple Discord bot instances with pooled creation and reuse.

use crate::gateway::interfaces::discord::config::{AccountConfig, DiscordChannelConfig};
use crate::gateway::interfaces::discord::api::DiscordClient;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Discord bot instance wrapper
#[derive(Clone)]
pub struct DiscordBot {
    /// Unique account identifier
    pub account_id: String,
    /// Discord client
    pub client: Arc<DiscordClient>,
    /// Account-specific configuration
    pub config: AccountConfig,
}

/// Pool for managing multiple Discord bot instances
pub struct DiscordAccountPool {
    config: DiscordChannelConfig,
    bots: Arc<RwLock<HashMap<String, Arc<DiscordBot>>>>,
    http_client: Arc<reqwest::Client>,
}

impl DiscordAccountPool {
    /// Create a new account pool from configuration
    pub fn new(config: DiscordChannelConfig) -> Self {
        Self {
            config,
            bots: Arc::new(RwLock::new(HashMap::new())),
            http_client: Arc::new(reqwest::Client::new()),
        }
    }

    /// Get or create a bot instance for the given account_id
    pub async fn get_or_create(&self, account_id: &str) -> Result<Arc<DiscordBot>, AccountPoolError> {
        // 先尝试从缓存获取
        {
            let bots = self.bots.read().await;
            if let Some(bot) = bots.get(account_id) {
                return Ok(bot.clone());
            }
        }

        // 获取账号配置
        let account_config = self.config.accounts
            .get(account_id)
            .ok_or_else(|| AccountPoolError::AccountNotFound(account_id.to_string()))?;

        // 创建新的 bot 实例
        let bot = self.create_bot(account_id, account_config.clone()).await?;

        // 缓存
        {
            let mut bots = self.bots.write().await;
            bots.insert(account_id.to_string(), bot.clone());
        }

        Ok(bot)
    }

    /// Create a new bot instance
    async fn create_bot(&self, account_id: &str, config: AccountConfig) -> Result<Arc<DiscordBot>, AccountPoolError> {
        let client = DiscordClient::new(&config.token)
            .map_err(AccountPoolError::ClientCreationFailed)?;

        Ok(Arc::new(DiscordBot {
            account_id: account_id.to_string(),
            client: Arc::new(client),
            config,
        }))
    }

    /// List all account IDs in the pool
    pub async fn list_accounts(&self) -> Vec<String> {
        let bots = self.bots.read().await;
        bots.keys().cloned().collect()
    }

    /// Remove a bot instance from the pool
    pub async fn remove_bot(&self, account_id: &str) -> Result<(), AccountPoolError> {
        let mut bots = self.bots.write().await;
        bots.remove(account_id)
            .ok_or_else(|| AccountPoolError::AccountNotFound(account_id.to_string()))?;
        Ok(())
    }
}

/// Account pool errors
#[derive(Debug, thiserror::Error)]
pub enum AccountPoolError {
    #[error("account not found: {0}")]
    AccountNotFound(String),

    #[error("failed to create client: {0}")]
    ClientCreationFailed(String),
}
```

- [ ] **Step 2: 创建 DiscordClient wrapper**

在 `api.rs` 中添加：

```rust
/// Discord HTTP client wrapper
pub struct DiscordClient {
    pub http: Arc<serenity::http::Http>,
}

impl DiscordClient {
    /// Create a new Discord client from token
    pub fn new(token: &str) -> Result<Self, serenity::Error> {
        serenity::http::Http::new(token).map(|http| Self {
            http: Arc::new(http),
        })
    }
}
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: 无编译错误

- [ ] **Step 4: 提交**

```bash
git add src/gateway/interfaces/discord/account_pool.rs src/gateway/interfaces/discord/api.rs
git commit -m "feat(discord): add DiscordAccountPool for multi-bot-instance support"
```

---

### Task 3: 创建 resolver/account.rs - 账号解析

**Files:**
- Create: `src/gateway/interfaces/discord/resolver/account.rs`

- [ ] **Step 1: 创建 AccountResolver**

```rust
//! Account Resolver
//!
//! Resolves channel_id -> account_id mapping for multi-account setup.

use crate::gateway::interfaces::discord::config::DiscordChannelConfig;
use std::collections::HashMap;

/// Resolves channel to account mapping
#[derive(Clone)]
pub struct AccountResolver {
    /// Maps channel_id -> account_id
    channel_to_account: HashMap<u64, String>,
    /// Maps guild_id -> account_id (fallback)
    guild_to_account: HashMap<u64, String>,
}

impl AccountResolver {
    /// Create a new AccountResolver from config
    pub fn new(config: &DiscordChannelConfig) -> Self {
        let mut channel_to_account = HashMap::new();
        let mut guild_to_account = HashMap::new();

        for (account_id, account) in &config.accounts {
            for (guild_id, guild) in &account.guilds {
                // 每个 channel 映射到其所属 guild 的 account
                for channel_id in guild.channels.keys() {
                    channel_to_account.insert(*channel_id, account_id.clone());
                }
                // guild 本身也映射
                guild_to_account.insert(*guild_id, account_id.clone());
            }
        }

        Self {
            channel_to_account,
            guild_to_account,
        }
    }

    /// Resolve account_id from channel_id
    pub fn resolve_account(&self, channel_id: u64) -> Option<String> {
        self.channel_to_account.get(&channel_id).cloned()
    }

    /// Resolve account_id from guild_id
    pub fn resolve_account_by_guild(&self, guild_id: u64) -> Option<String> {
        self.guild_to_account.get(&guild_id).cloned()
    }
}
```

- [ ] **Step 2: 更新 resolver/mod.rs 导出**

```rust
pub mod account;
pub use account::AccountResolver;
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: 无编译错误

- [ ] **Step 4: 提交**

```bash
git add src/gateway/interfaces/discord/resolver/account.rs src/gateway/interfaces/discord/resolver/mod.rs
git commit -m "feat(discord): add AccountResolver for channel-to-account mapping"
```

---

### Task 4: 创建 resolver/channel.rs - Channel 配置解析

**Files:**
- Create: `src/gateway/interfaces/discord/resolver/channel.rs`

- [ ] **Step 1: 创建 ChannelResolver**

```rust
//! Channel Settings Resolver
//!
//! Resolves effective channel settings with per-guild/per-channel override.

use crate::gateway::interfaces::discord::config::{
    AccountConfig, DiscordChannelConfig, DiscordChannelSettings, DiscordGuildSettings,
};

/// Resolved channel settings (with override chain applied)
#[derive(Debug, Clone)]
pub struct ResolvedChannelSettings {
    /// The resolved settings
    pub settings: DiscordChannelSettings,
    /// Which account these settings came from
    pub account_id: String,
    /// Which guild these settings came from (if any)
    pub guild_id: Option<u64>,
    /// Which channel these settings came from (if any)
    pub channel_id: Option<u64>,
}

/// Resolves effective channel settings with override chain
pub struct ChannelSettingsResolver {
    config: DiscordChannelConfig,
}

impl ChannelSettingsResolver {
    pub fn new(config: DiscordChannelConfig) -> Self {
        Self { config }
    }

    /// Resolve effective settings for a channel
    ///
    /// Override chain: default -> account -> guild -> channel
    pub fn resolve(&self, account_id: &str, guild_id: Option<u64>, channel_id: Option<u64>) -> ResolvedChannelSettings {
        // Start with global defaults
        let mut settings = self.config.default.clone();

        // Apply account defaults
        if let Some(account) = self.config.accounts.get(account_id) {
            settings = Self::merge_settings(settings, &account.default_settings);
        }

        // Apply guild settings
        if let Some(gid) = guild_id {
            if let Some(account) = self.config.accounts.get(account_id) {
                if let Some(guild) = account.guilds.get(&gid) {
                    settings = Self::merge_settings(settings, &guild.settings);

                    // Apply channel settings
                    if let Some(cid) = channel_id {
                        if let Some(channel) = guild.channels.get(&cid) {
                            settings = Self::merge_settings(settings, channel);
                        }
                    }
                }
            }
        }

        ResolvedChannelSettings {
            settings,
            account_id: account_id.to_string(),
            guild_id,
            channel_id,
        }
    }

    /// Merge two settings, with non-default values in `override` taking precedence
    fn merge_settings(base: DiscordChannelSettings, override_: &DiscordChannelSettings) -> DiscordChannelSettings {
        DiscordChannelSettings {
            prefix: override_.prefix.clone().or(base.prefix),
            allowlist: if override_.allowlist.is_empty() { base.allowlist } else { override_.allowlist.clone() },
            blocklist: if override_.blocklist.is_empty() { base.blocklist } else { override_.blocklist.clone() },
            features: DiscordChannelSettings::merge_features(&base.features, &override_.features),
            thread_binding: DiscordChannelSettings::merge_thread_binding(&base.thread_binding, &override_.thread_binding),
            reply: DiscordChannelSettings::merge_reply(&base.reply, &override_.reply),
            typing_indicator: base.typing_indicator,
        }
    }
}

impl DiscordChannelSettings {
    fn merge_features(base: &crate::gateway::interfaces::discord::config::DiscordFeatures, override_: &crate::gateway::interfaces::discord::config::DiscordFeatures) -> crate::gateway::interfaces::discord::config::DiscordFeatures {
        crate::gateway::interfaces::discord::config::DiscordFeatures {
            reactions: override_.reactions,
            slash_commands: override_.slash_commands,
            interactions: override_.interactions,
            streaming_preview: override_.streaming_preview,
            plural_kit: override_.plural_kit,
            exec_approval: override_.exec_approval,
        }
    }

    fn merge_thread_binding(base: &crate::gateway::interfaces::discord::config::ThreadBindingConfig, override_: &crate::gateway::interfaces::discord::config::ThreadBindingConfig) -> crate::gateway::interfaces::discord::config::ThreadBindingConfig {
        crate::gateway::interfaces::discord::config::ThreadBindingConfig {
            enabled: override_.enabled,
            allow_sub_agents: override_.allow_sub_agents,
            command_prefix: override_.command_prefix.clone().unwrap_or(base.command_prefix.clone()),
        }
    }

    fn merge_reply(base: &crate::gateway::interfaces::discord::config::ReplyConfig, override_: &crate::gateway::interfaces::discord::config::ReplyConfig) -> crate::gateway::interfaces::discord::config::ReplyConfig {
        crate::gateway::interfaces::discord::config::ReplyConfig {
            auto_reply: override_.auto_reply,
            include_quotes: override_.include_quotes,
        }
    }
}
```

- [ ] **Step 2: 更新 resolver/mod.rs 导出**

```rust
pub mod channel;
pub use channel::{ChannelSettingsResolver, ResolvedChannelSettings};
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: 无编译错误

- [ ] **Step 4: 提交**

```bash
git add src/gateway/interfaces/discord/resolver/channel.rs src/gateway/interfaces/discord/resolver/mod.rs
git commit -m "feat(discord): add ChannelSettingsResolver for per-channel config override"
```

---

### Task 5: 更新 discord/mod.rs - 适配新配置结构

**Files:**
- Modify: `src/gateway/interfaces/discord/mod.rs` (前 100 行和后 100 行)

- [ ] **Step 1: 读取 mod.rs 了解现有结构**

查看 `struct DiscordChannel` 的定义和 `new` 方法

- [ ] **Step 2: 添加 AccountPool 字段**

在 `DiscordChannel` struct 中添加：

```rust
pub struct DiscordChannel {
    // ... existing fields ...
    /// Account pool for multi-bot support
    account_pool: Option<DiscordAccountPool>,
    /// Settings resolver
    settings_resolver: Option<ChannelSettingsResolver>,
}
```

- [ ] **Step 3: 更新 new 方法支持新配置**

```rust
impl DiscordChannel {
    pub fn new(id: String, config: DiscordChannelConfig) -> Self {
        let account_pool = DiscordAccountPool::new(config.clone());
        let settings_resolver = ChannelSettingsResolver::new(config);

        Self {
            id,
            // ... 保留现有字段 ...
            account_pool: Some(account_pool),
            settings_resolver: Some(settings_resolver),
        }
    }

    /// Create from legacy flat config (backward compatibility)
    pub fn from_legacy(id: String, config: DiscordConfig) -> Self {
        let channel_config: DiscordChannelConfig = config.into();
        Self::new(id, channel_config)
    }
}
```

- [ ] **Step 4: 添加 settings_resolver 方法**

```rust
impl DiscordChannel {
    /// Resolve settings for a channel
    pub fn resolve_settings(&self, channel_id: u64, guild_id: Option<u64>) -> Result<DiscordChannelSettings, DiscordChannelError> {
        let resolver = self.settings_resolver
            .as_ref()
            .ok_or_else(|| DiscordChannelError::NotConfigured)?;

        let account_id = resolver.resolve_account(channel_id)
            .or_else(|| guild_id.and_then(|g| resolver.resolve_account_by_guild(g)))
            .ok_or_else(|| DiscordChannelError::ChannelNotAllowed(channel_id))?;

        let resolved = resolver.resolve(&account_id, guild_id, Some(channel_id));
        Ok(resolved.settings)
    }
}
```

- [ ] **Step 5: 添加 DiscordChannelError**

```rust
#[derive(Debug, thiserror::Error)]
pub enum DiscordChannelError {
    #[error("channel not configured")]
    NotConfigured,

    #[error("channel {0} not in any account")]
    ChannelNotAllowed(u64),

    #[error("account not found: {0}")]
    AccountNotFound(String),
}
```

- [ ] **Step 6: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -100`
Expected: 无编译错误

- [ ] **Step 7: 提交**

```bash
git add src/gateway/interfaces/discord/mod.rs
git commit -m "feat(discord): adapt DiscordChannel to new nested config structure"
```

---

## Phase 2: Feature Enhancement

### Task 6: 创建 handlers/interaction.rs - 交互组件

**Files:**
- Create: `src/gateway/interfaces/discord/handlers/interaction.rs`

- [ ] **Step 1: 创建 InteractionHandler**

```rust
//! Interaction Handler
//!
//! Handles button clicks and select menu interactions.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Message component interaction
#[derive(Debug, Clone, Deserialize)]
pub struct MessageComponent {
    pub custom_id: String,
    pub component_type: u8,
}

/// Interaction handler result
pub type InteractionResult = Result<(), InteractionError>;

/// Interaction errors
#[derive(Debug, thiserror::Error)]
pub enum InteractionError {
    #[error("invalid interaction: {0}")]
    InvalidInteraction(String),

    #[error("handler error: {0}")]
    HandlerError(String),
}

/// Interaction handler for buttons and select menus
#[derive(Clone)]
pub struct InteractionHandler {
    /// Approval queue for exec commands
    approval_queue: Option<Arc<ApprovalQueue>>,
}

impl InteractionHandler {
    /// Create a new InteractionHandler
    pub fn new() -> Self {
        Self {
            approval_queue: None,
        }
    }

    /// Set approval queue for exec commands
    pub fn with_approval_queue(mut self, queue: Arc<ApprovalQueue>) -> Self {
        self.approval_queue = Some(queue);
        self
    }

    /// Handle an interaction
    pub async fn handle(&self, interaction: Interaction) -> InteractionResult {
        match interaction {
            Interaction::MessageComponent(component) => {
                self.handle_component(component).await
            }
            Interaction::ModalSubmit(_) => {
                // TODO: Implement modal handling
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn handle_component(&self, component: MessageComponent) -> InteractionResult {
        match component.component_type {
            2 => self.handle_button(&component.custom_id).await?,
            3 | 4 | 5 => self.handle_select_menu(&component.custom_id).await?,
            _ => {}
        }
        Ok(())
    }

    async fn handle_button(&self, custom_id: &str) -> InteractionResult {
        // Handle approval buttons: "exec_approve:{id}" or "exec_deny:{id}"
        if let Some(approval_id) = custom_id.strip_prefix("exec_approve:") {
            if let Some(queue) = &self.approval_queue {
                queue.approve(approval_id).await
                    .map_err(|e| InteractionError::HandlerError(e.to_string()))?;
            }
        } else if let Some(approval_id) = custom_id.strip_prefix("exec_deny:") {
            if let Some(queue) = &self.approval_queue {
                queue.deny(approval_id).await
                    .map_err(|e| InteractionError::HandlerError(e.to_string()))?;
            }
        }
        Ok(())
    }

    async fn handle_select_menu(&self, custom_id: &str) -> InteractionResult {
        // TODO: Implement select menu handling
        Ok(())
    }
}

impl Default for InteractionHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Approval queue placeholder (实现见 handlers/approval.rs)
#[derive(Clone)]
pub struct ApprovalQueue;

impl ApprovalQueue {
    pub async fn approve(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    pub async fn deny(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
}
```

- [ ] **Step 2: 创建 handlers/mod.rs**

```rust
//! Discord Message Handlers
//!
//! Modular handlers for different message types.

pub mod interaction;
pub mod message;
pub mod streaming;
pub mod thread;

pub use interaction::InteractionHandler;
pub use message::MessageHandler;
pub use streaming::StreamingHandler;
pub use thread::ThreadBindingHandler;
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: 无编译错误

- [ ] **Step 4: 提交**

```bash
git add src/gateway/interfaces/discord/handlers/
git commit -m "feat(discord): add InteractionHandler for buttons and select menus"
```

---

### Task 7: 创建 handlers/streaming.rs - Streaming Preview

**Files:**
- Create: `src/gateway/interfaces/discord/handlers/streaming.rs`

- [ ] **Step 1: 创建 StreamingHandler**

```rust
//! Streaming Handler
//!
//! Handles Discord presence updates for streaming status preview.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Streaming preview data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingPreview {
    /// User ID who is streaming
    pub user_id: u64,
    /// Username
    pub username: String,
    /// Stream URL (Twitch/YouTube)
    pub stream_url: String,
    /// Stream title
    pub title: String,
    /// Viewer count (if available)
    pub viewer_count: Option<i64>,
}

/// Presence update event
#[derive(Debug, Clone)]
pub struct PresenceUpdate {
    pub user_id: u64,
    pub username: String,
    pub activities: Vec<Activity>,
}

/// Activity (e.g., streaming, playing a game)
#[derive(Debug, Clone)]
pub struct Activity {
    pub kind: ActivityType,
    pub name: String,
    pub url: Option<String>,
    pub details: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone)]
pub enum ActivityType {
    Playing,
    Streaming,
    Listening,
    Watching,
    Custom,
    Competing,
}

/// In-memory cache for streaming previews
#[derive(Default)]
pub struct StreamingCache {
    entries: HashMap<u64, StreamingPreview>,
}

impl StreamingCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, user_id: u64, preview: StreamingPreview) {
        self.entries.insert(user_id, preview);
    }

    pub fn get(&self, user_id: u64) -> Option<&StreamingPreview> {
        self.entries.get(&user_id)
    }

    pub fn remove(&mut self, user_id: u64) {
        self.entries.remove(&user_id);
    }
}

/// Handler for streaming presence updates
#[derive(Clone)]
pub struct StreamingHandler {
    cache: Arc<RwLock<StreamingCache>>,
}

impl StreamingHandler {
    /// Create a new StreamingHandler
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(StreamingCache::new())),
        }
    }

    /// Handle a presence update
    pub async fn handle_presence_update(&self, update: PresenceUpdate) -> Result<(), StreamingError> {
        // Find streaming activity
        for activity in &update.activities {
            if matches!(activity.kind, ActivityType::Streaming) {
                let preview = StreamingPreview {
                    user_id: update.user_id,
                    username: update.username,
                    stream_url: activity.url.clone().unwrap_or_default(),
                    title: activity.name.clone(),
                    viewer_count: activity.details
                        .as_ref()
                        .and_then(|d| d.get("viewer_count"))
                        .and_then(|v| v.as_i64()),
                };

                let mut cache = self.cache.write().await;
                cache.set(update.user_id, preview);
            }
        }

        Ok(())
    }

    /// Get streaming preview for a user
    pub async fn get_preview(&self, user_id: u64) -> Option<StreamingPreview> {
        let cache = self.cache.read().await;
        cache.get(user_id).cloned()
    }

    /// Remove streaming preview when user goes offline
    pub async fn remove_preview(&self, user_id: u64) {
        let mut cache = self.cache.write().await;
        cache.remove(user_id);
    }
}

impl Default for StreamingHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StreamingError {
    #[error("streaming error: {0}")]
    Error(String),
}
```

- [ ] **Step 2: 更新 handlers/mod.rs**

```rust
pub mod streaming;
pub use streaming::StreamingHandler;
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: 无编译错误

- [ ] **Step 4: 提交**

```bash
git add src/gateway/interfaces/discord/handlers/streaming.rs src/gateway/interfaces/discord/handlers/mod.rs
git commit -m "feat(discord): add StreamingHandler for presence update tracking"
```

---

### Task 8: 创建 handlers/thread.rs - Thread Binding (增强版)

**Files:**
- Create: `src/gateway/interfaces/discord/handlers/thread.rs`

- [ ] **Step 1: 创建 ThreadBindingHandler**

```rust
//! Thread Binding Handler
//!
//! Manages thread bindings with sub-agent support.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Thread binding entry
#[derive(Debug, Clone)]
pub struct ThreadBinding {
    /// Original message ID that started the thread
    pub parent_message_id: u64,
    /// Discord thread ID
    pub thread_id: u64,
    /// Guild ID
    pub guild_id: u64,
    /// Channel ID
    pub channel_id: u64,
    /// Agent IDs participating in this thread
    pub participants: Vec<AgentId>,
    /// When the binding was created
    pub created_at: DateTime<Utc>,
}

/// Agent identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Thread binding errors
#[derive(Debug, thiserror::Error)]
pub enum ThreadBindingError {
    #[error("thread binding not found: {0}")]
    NotFound(u64),

    #[error("already bound: {0}")]
    AlreadyBound(u64),

    #[error("sub-agent not allowed in thread: {0}")]
    SubAgentNotAllowed(u64),
}

/// Handler for thread bindings
#[derive(Clone)]
pub struct ThreadBindingHandler {
    /// Thread ID -> ThreadBinding
    bindings: Arc<RwLock<HashMap<u64, ThreadBinding>>>,
    /// Parent message ID -> Thread ID
    message_to_thread: Arc<RwLock<HashMap<u64, u64>>>,
    /// Allow sub-agents by default
    allow_sub_agents: bool,
}

impl ThreadBindingHandler {
    /// Create a new ThreadBindingHandler
    pub fn new() -> Self {
        Self {
            bindings: Arc::new(RwLock::new(HashMap::new())),
            message_to_thread: Arc::new(RwLock::new(HashMap::new())),
            allow_sub_agents: false,
        }
    }

    /// Enable or disable sub-agent participation
    pub fn with_sub_agents(mut self, allow: bool) -> Self {
        self.allow_sub_agents = allow;
        self
    }

    /// Create a new thread binding
    pub async fn create_binding(
        &self,
        parent_message_id: u64,
        thread_id: u64,
        guild_id: u64,
        channel_id: u64,
        agent_id: AgentId,
    ) -> Result<ThreadBinding, ThreadBindingError> {
        // Check if already bound
        {
            let bindings = self.bindings.read().await;
            if bindings.contains_key(&thread_id) {
                return Err(ThreadBindingError::AlreadyBound(thread_id));
            }
        }

        let binding = ThreadBinding {
            parent_message_id,
            thread_id,
            guild_id,
            channel_id,
            participants: vec![agent_id],
            created_at: Utc::now(),
        };

        // Store both mappings
        {
            let mut bindings = self.bindings.write().await;
            bindings.insert(thread_id, binding.clone());
        }
        {
            let mut message_to_thread = self.message_to_thread.write().await;
            message_to_thread.insert(parent_message_id, thread_id);
        }

        Ok(binding)
    }

    /// Add a sub-agent participant to a thread
    pub async fn add_participant(
        &self,
        thread_id: u64,
        agent_id: AgentId,
    ) -> Result<(), ThreadBindingError> {
        // Check sub-agent permission
        if !self.allow_sub_agents {
            return Err(ThreadBindingError::SubAgentNotAllowed(thread_id));
        }

        let mut bindings = self.bindings.write().await;
        let binding = bindings.get_mut(&thread_id)
            .ok_or_else(|| ThreadBindingError::NotFound(thread_id))?;

        if !binding.participants.contains(&agent_id) {
            binding.participants.push(agent_id);
        }

        Ok(())
    }

    /// Remove a participant from a thread
    pub async fn remove_participant(
        &self,
        thread_id: u64,
        agent_id: &AgentId,
    ) -> Result<(), ThreadBindingError> {
        let mut bindings = self.bindings.write().await;
        let binding = bindings.get_mut(&thread_id)
            .ok_or_else(|| ThreadBindingError::NotFound(thread_id))?;

        binding.participants.retain(|a| a != agent_id);
        Ok(())
    }

    /// Get a thread binding by thread ID
    pub async fn get_binding(&self, thread_id: u64) -> Option<ThreadBinding> {
        let bindings = self.bindings.read().await;
        bindings.get(&thread_id).cloned()
    }

    /// Get thread ID for a parent message
    pub async fn get_thread_for_message(&self, message_id: u64) -> Option<u64> {
        let message_to_thread = self.message_to_thread.read().await;
        message_to_thread.get(&message_id).copied()
    }

    /// Delete a thread binding
    pub async fn delete_binding(&self, thread_id: u64) -> Result<(), ThreadBindingError> {
        let binding = {
            let mut bindings = self.bindings.write().await;
            bindings.remove(&thread_id)
                .ok_or_else(|| ThreadBindingError::NotFound(thread_id))?
        };

        // Also remove the message mapping
        {
            let mut message_to_thread = self.message_to_thread.write().await;
            message_to_thread.remove(&binding.parent_message_id);
        }

        Ok(())
    }
}

impl Default for ThreadBindingHandler {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 2: 更新 handlers/mod.rs**

```rust
pub mod thread;
pub use thread::{ThreadBindingHandler, ThreadBinding, AgentId};
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: 无编译错误

- [ ] **Step 4: 提交**

```bash
git add src/gateway/interfaces/discord/handlers/thread.rs src/gateway/interfaces/discord/handlers/mod.rs
git commit -m "feat(discord): enhance ThreadBindingHandler with sub-agent support"
```

---

### Task 9: 创建 handlers/approval.rs - Exec Approval

**Files:**
- Create: `src/gateway/interfaces/discord/handlers/approval.rs`

- [ ] **Step 1: 创建 ApprovalQueue**

```rust
//! Exec Approval Handler
//!
//! Manages exec command approval workflow with Discord interactions.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Pending exec request
#[derive(Debug, Clone)]
pub struct PendingExec {
    /// Unique approval ID
    pub id: String,
    /// User who requested the exec
    pub user_id: u64,
    /// Command to execute
    pub command: String,
    /// When the request was made
    pub created_at: DateTime<Utc>,
    /// Approval status
    pub status: ApprovalStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

/// Approval queue errors
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("approval request not found: {0}")]
    NotFound(String),

    #[error("already {0}: {1}")]
    AlreadyResolved(String, String),

    #[error("expired: {0}")]
    Expired(String),
}

/// Queue for managing exec command approvals
#[derive(Clone)]
pub struct ApprovalQueue {
    /// Pending approvals: approval_id -> PendingExec
    pending: Arc<RwLock<HashMap<String, PendingExec>>>,
    /// User's pending approvals: user_id -> Vec<approval_id>
    user_pending: Arc<RwLock<HashMap<u64, Vec<String>>>>,
    /// TTL in seconds
    ttl_secs: u64,
}

impl ApprovalQueue {
    /// Create a new ApprovalQueue
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            user_pending: Arc::new(RwLock::new(HashMap::new())),
            ttl_secs: 3600, // 1 hour default
        }
    }

    /// Create a new pending exec request
    pub async fn create(&self, user_id: u64, command: String) -> String {
        let id = format!("exec_{}", uuid::Uuid::new_v4());

        let pending = PendingExec {
            id: id.clone(),
            user_id,
            command,
            created_at: Utc::now(),
            status: ApprovalStatus::Pending,
        };

        // Store in both maps
        {
            let mut pending_guard = self.pending.write().await;
            pending_guard.insert(id.clone(), pending);
        }
        {
            let mut user_pending = self.user_pending.write().await;
            user_pending.entry(user_id).or_default().push(id.clone());
        }

        id
    }

    /// Approve an exec request
    pub async fn approve(&self, approval_id: &str) -> Result<PendingExec, ApprovalError> {
        self.resolve(approval_id, ApprovalStatus::Approved).await
    }

    /// Deny an exec request
    pub async fn deny(&self, approval_id: &str) -> Result<PendingExec, ApprovalError> {
        self.resolve(approval_id, ApprovalStatus::Denied).await
    }

    async fn resolve(&self, approval_id: &str, status: ApprovalStatus) -> Result<PendingExec, ApprovalError> {
        let mut pending_guard = self.pending.write().await;

        let pending = pending_guard.get_mut(approval_id)
            .ok_or_else(|| ApprovalError::NotFound(approval_id.to_string()))?;

        // Check if already resolved
        if pending.status != ApprovalStatus::Pending {
            return Err(ApprovalError::AlreadyResolved(
                format!("{:?}", pending.status),
                approval_id.to_string(),
            ));
        }

        // Check if expired
        let age = Utc::now().signed_duration_since(pending.created_at).num_seconds() as u64;
        if age > self.ttl_secs {
            pending.status = ApprovalStatus::Expired;
            return Err(ApprovalError::Expired(approval_id.to_string()));
        }

        pending.status = status.clone();

        // Remove from user's pending list
        {
            let mut user_pending = self.user_pending.write().await;
            if let Some(ids) = user_pending.get_mut(&pending.user_id) {
                ids.retain(|id| id != approval_id);
            }
        }

        Ok(pending.clone())
    }

    /// Get a pending exec request
    pub async fn get(&self, approval_id: &str) -> Option<PendingExec> {
        let pending = self.pending.read().await;
        pending.get(approval_id).cloned()
    }

    /// List pending requests for a user
    pub async fn list_user_pending(&self, user_id: u64) -> Vec<PendingExec> {
        let pending = self.pending.read().await;
        pending.values()
            .filter(|p| p.user_id == user_id && p.status == ApprovalStatus::Pending)
            .cloned()
            .collect()
    }

    /// Clean up expired requests
    pub async fn cleanup_expired(&self) {
        let now = Utc::now();
        let mut pending_guard = self.pending.write().await;

        for pending in pending_guard.values_mut() {
            if pending.status == ApprovalStatus::Pending {
                let age = now.signed_duration_since(pending.created_at).num_seconds() as u64;
                if age > self.ttl_secs {
                    pending.status = ApprovalStatus::Expired;
                }
            }
        }
    }
}

impl Default for ApprovalQueue {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 2: 添加 uuid 依赖到 Cargo.toml**

如果还没有，添加 `uuid = "1.0"` 到 `alephcore` 的 dependencies

- [ ] **Step 3: 更新 handlers/mod.rs**

```rust
pub mod approval;
pub use approval::{ApprovalQueue, PendingExec, ApprovalStatus, ApprovalError};
```

- [ ] **Step 4: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: 无编译错误

- [ ] **Step 5: 提交**

```bash
git add src/gateway/interfaces/discord/handlers/approval.rs src/gateway/interfaces/discord/handlers/mod.rs
git commit -m "feat(discord): add ApprovalQueue for exec command workflow"
```

---

## Phase 3: Security & Cleanup

### Task 10: 创建 security/audit.rs - 安全审计

**Files:**
- Create: `src/gateway/interfaces/discord/security/audit.rs`

- [ ] **Step 1: 创建审计类型**

```rust
//! Discord Security Audit
//!
//! Audit logging for Discord channel events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Audit event types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    CommandExecuted,
    ExecApprovalRequested,
    ExecApproved,
    ExecDenied,
    MessageReceived,
    InteractionReceived,
}

/// Audit event metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditMetadata {
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub content_preview: Option<String>,
    pub success: Option<bool>,
}

/// A complete audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordAuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub account_id: String,
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub user_id: u64,
    pub metadata: AuditMetadata,
}

impl DiscordAuditEvent {
    /// Create a new audit event
    pub fn new(
        event_type: AuditEventType,
        account_id: String,
        guild_id: Option<u64>,
        channel_id: u64,
        user_id: u64,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            event_type,
            account_id,
            guild_id,
            channel_id,
            user_id,
            metadata: AuditMetadata {
                command: None,
                args: None,
                content_preview: None,
                success: None,
            },
        }
    }

    /// Set command info
    pub fn with_command(mut self, command: String, args: Vec<String>) -> Self {
        self.metadata.command = Some(command);
        self.metadata.args = Some(args);
        self
    }

    /// Set content preview
    pub fn with_content_preview(mut self, preview: String) -> Self {
        self.metadata.content_preview = Some(preview);
        self
    }

    /// Set success status
    pub fn with_success(mut self, success: bool) -> Self {
        self.metadata.success = Some(success);
        self
    }
}

/// Content retention policy
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ContentRetention {
    Full,
    Anonymized,
    #[default]
    MetadataOnly,
}
```

- [ ] **Step 2: 创建 DiscordAuditLogger**

```rust
use crate::gateway::interfaces::discord::api::DiscordClient;
use crate::gateway::interfaces::discord::config::{AuditEvents, ContentRetention, DiscordSecurityConfig};

/// Audit logger for Discord events
#[derive(Clone)]
pub struct DiscordAuditLogger {
    config: DiscordSecurityConfig,
    http_client: Arc<DiscordClient>,
}

impl DiscordAuditLogger {
    /// Create a new audit logger
    pub fn new(config: DiscordSecurityConfig, http_client: Arc<DiscordClient>) -> Self {
        Self { config, http_client }
    }

    /// Log an audit event
    pub async fn log(&self, event: DiscordAuditEvent) -> Result<(), AuditError> {
        if !self.config.audit_enabled {
            return Ok(());
        }

        // Check if this event type should be logged
        if !self.should_log(&event.event_type) {
            return Ok(());
        }

        // Apply content retention policy
        let sanitized = self.sanitize(event);

        // Format as Discord embed
        let payload = self.format_payload(sanitized);

        // Send to all configured audit channels
        for channel_id in &self.config.audit_channels {
            self.send_to_channel(*channel_id, &payload).await?;
        }

        Ok(())
    }

    fn should_log(&self, event_type: &AuditEventType) -> bool {
        match event_type {
            AuditEventType::CommandExecuted => self.config.audit_events.commands,
            AuditEventType::ExecApprovalRequested |
            AuditEventType::ExecApproved |
            AuditEventType::ExecDenied => self.config.audit_events.exec_approvals,
            AuditEventType::MessageReceived |
            AuditEventType::InteractionReceived => self.config.audit_events.message_content,
        }
    }

    fn sanitize(&self, mut event: DiscordAuditEvent) -> DiscordAuditEvent {
        match self.config.content_retention {
            ContentRetention::Full => {}
            ContentRetention::Anonymized => {
                event.user_id = 0;
                event.channel_id = 0;
                event.guild_id = None;
                event.metadata.content_preview = event.metadata.content_preview
                    .map(|_| "[CONTENT REDACTED]".to_string());
            }
            ContentRetention::MetadataOnly => {
                event.metadata.content_preview = None;
                event.metadata.command = None;
                event.metadata.args = None;
            }
        }
        event
    }

    fn format_payload(&self, event: DiscordAuditEvent) -> serde_json::Value {
        use serde_json::json;

        let color = match event.event_type {
            AuditEventType::CommandExecuted => 0x3498db,
            AuditEventType::ExecApprovalRequested => 0xf39c12,
            AuditEventType::ExecApproved => 0x27ae60,
            AuditEventType::ExecDenied => 0xe74c3c,
            AuditEventType::MessageReceived => 0x9b59b6,
            AuditEventType::InteractionReceived => 0x1abc9c,
        };

        json!({
            "embeds": [{
                "title": format!("{:?}", event.event_type),
                "color": color,
                "timestamp": event.timestamp.to_rfc3339(),
                "fields": [
                    {"name": "Account", "value": &event.account_id, "inline": true},
                    {"name": "User", "value": event.user_id.to_string(), "inline": true},
                ],
                "footer": {
                    "text": "Aleph Discord Audit"
                }
            }]
        })
    }

    async fn send_to_channel(&self, channel_id: u64, payload: &serde_json::Value) -> Result<(), AuditError> {
        // TODO: Implement actual Discord API call
        tracing::debug!(channel_id = channel_id, "audit log sent");
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("audit error: {0}")]
    Error(String),

    #[error("Discord API error: {0}")]
    ApiError(String),
}
```

- [ ] **Step 2: 创建 security/mod.rs**

```rust
//! Discord Security Module
//!
//! Security auditing and policy enforcement.

pub mod audit;

pub use audit::{DiscordAuditLogger, DiscordAuditEvent, AuditEventType, AuditMetadata};
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: 无编译错误

- [ ] **Step 4: 提交**

```bash
git add src/gateway/interfaces/discord/security/
git commit -m "feat(discord): add security audit infrastructure"
```

---

### Task 11: 代码清理 - 移除冗余代码

**Files:**
- Modify: `src/gateway/interfaces/discord/mod.rs`

- [ ] **Step 1: 清理过时的注释和结构**

识别并移除设计文档中标记为 deprecated 的代码

- [ ] **Step 2: 验证所有模块正确导出**

确保 `mod.rs` 正确导出所有 public types

- [ ] **Step 3: 运行 clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Expected: 无警告

- [ ] **Step 4: 提交**

```bash
git add src/gateway/interfaces/discord/
git commit -m "chore(discord): clean up legacy code and align exports"
```

---

## Verification

### 编译验证

```bash
cargo check -p alephcore 2>&1 | grep -E "(error|warning:)" | head -20
```

### 测试验证

```bash
cargo test -p alephcore --lib -- discord 2>&1 | tail -20
```

---

## Self-Review Checklist

- [ ] Spec coverage: 每个 Phase 1-3 的功能都有对应 task
- [ ] Placeholder scan: 无 "TODO", "TBD", "fill in later"
- [ ] Type consistency: 所有类型、trait、方法签名在 tasks 间一致
- [ ] 测试覆盖: 每个新模块有基本单元测试
- [ ] 提交规范: 每 task 一提交，遵循 `<scope>: <description>` 格式

---

## Plan Complete

**Saved to**: `docs/superpowers/plans/2026-04-15-discord-channel-redesign-plan.md`

---

## Execution Options

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
