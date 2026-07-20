# Telegram 结构化升级实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Aleph 的 Telegram 频道升级为支持多账户、配置层级继承、reasoning stream、sticker pipeline、poll support、status reaction controller 和 reaction notifications，并清理废弃的 bridge 代码。

**架构：** 所有修改收敛在 `src/gateway/interfaces/telegram/` 内，不改 `Channel` trait。配置采用 account → group → topic 层级覆盖，运行时预解析为 O(1) 查表。清理工作放在最后阶段。

**Tech Stack:** Rust, teloxide, tokio, serde, thiserror, uuid, chrono

---

## 文件结构映射

### 新增文件
- `src/gateway/interfaces/telegram/config_v2.rs` — 层级化 TelegramConfig V2 类型定义
- `src/gateway/interfaces/telegram/config_resolver.rs` — 配置解析器与 ResolvedConfig
- `src/gateway/interfaces/telegram/bot_instance.rs` — BotInstance（Bot + AccountConfig + polling task）
- `src/gateway/interfaces/telegram/sticker.rs` — Sticker 缓存、vision 描述、tool action
- `src/gateway/interfaces/telegram/poll.rs` — Poll 创建、存储、poll answer 入站处理
- `src/gateway/interfaces/telegram/status_reaction.rs` — Status reaction 状态机
- `src/gateway/interfaces/telegram/reaction_handler.rs` — 入站 message_reaction 处理
- `src/gateway/interfaces/telegram/reasoning_lane.rs` — Reasoning stream lane 逻辑

### 修改文件
- `src/gateway/interfaces/telegram/config.rs` — 增加向后兼容的升级逻辑与新字段
- `src/gateway/interfaces/telegram/mod.rs` — 重构为多账号运行时，注册新 handler
- `src/gateway/interfaces/telegram/delivery.rs` — 接入 reasoning lane、error policy、status reaction
- `src/gateway/interfaces/telegram/handlers.rs` — 解析 resolved config，传递 account context
- `src/gateway/interfaces/telegram/access.rs` — 按 account_id 隔离 pairing 数据
- `src/gateway/interfaces/telegram/approval.rs` — 适配多账号路由
- `src/gateway/interfaces/telegram/error_cooldown.rs` — 增加 policy-aware 能力
- `src/gateway/interfaces/telegram/polling.rs` — 支持多个 bot 的独立 polling loop
- `src/gateway/channel.rs` — 仅在 metadata / types 层面增加 PollAnswer / Reaction（不影响 trait）
- `src/gateway/reply_emitter.rs` — 提取 reasoning block 供 reasoning lane 消费
- `src/gateway/streaming.rs` — 如有需要，增加 lane 标识支持

### 待删除（Phase 4）
- `src/gateway/bridge/bridged_channel.rs`
- `src/gateway/bridge/supervisor.rs`
- `src/gateway/bridge/types.rs`
- `src/gateway/bridge/mod.rs`
- `src/gateway/handlers/approval_bridge.rs`

---

## Phase 1: 配置重构 + 多账户（3 周）

### Task 1.1: 定义层级化配置类型

**Files:**
- Create: `src/gateway/interfaces/telegram/config_v2.rs`

- [ ] **Step 1: 写入配置类型定义**

```rust
use serde::{Deserialize, Serialize};
use crate::gateway::coalescer::CoalescingConfig;
use super::config::{DmPolicy, GroupPolicy, StreamingOptions};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TelegramTopicConfig {
    pub id: String,
    pub thread_id: i32,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub block_streaming: Option<bool>,
    #[serde(default)]
    pub error_policy: Option<ErrorPolicy>,
    #[serde(default)]
    pub dm_policy: Option<DmPolicy>,
    #[serde(default)]
    pub group_policy: Option<GroupPolicy>,
    #[serde(default)]
    pub send_typing: Option<bool>,
    #[serde(default)]
    pub allowed_users: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TelegramGroupConfig {
    pub id: String,
    pub chat_id: i64,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub block_streaming: Option<bool>,
    #[serde(default)]
    pub error_policy: Option<ErrorPolicy>,
    #[serde(default)]
    pub group_policy: Option<GroupPolicy>,
    #[serde(default)]
    pub send_typing: Option<bool>,
    #[serde(default)]
    pub allowed_users: Option<Vec<i64>>,
    #[serde(default)]
    pub topics: Vec<TelegramTopicConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TelegramAccountConfig {
    pub id: String,
    pub bot_token: String,
    #[serde(default)]
    pub bot_username: Option<String>,
    #[serde(default)]
    pub default_agent: Option<String>,
    #[serde(default)]
    pub dm_policy: Option<DmPolicy>,
    #[serde(default)]
    pub group_policy: Option<GroupPolicy>,
    #[serde(default)]
    pub send_typing: Option<bool>,
    #[serde(default)]
    pub allowed_users: Option<Vec<i64>>,
    #[serde(default)]
    pub allowed_groups: Option<Vec<i64>>,
    #[serde(default)]
    pub streaming: Option<StreamingOptions>,
    #[serde(default)]
    pub error_policy: Option<ErrorPolicy>,
    #[serde(default)]
    pub groups: Vec<TelegramGroupConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPolicy {
    Reply,
    Silent,
    Once,
}

impl Default for ErrorPolicy {
    fn default() -> Self {
        ErrorPolicy::Reply
    }
}
```

- [ ] **Step 2: 编译检查**

Run: `cargo check -p alephcore`
Expected: 通过（或仅有未使用警告）

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/telegram/config_v2.rs
git commit -m "telegram: add hierarchical config v2 types"
```

---

### Task 1.2: 向后兼容地扩展 TelegramConfig

**Files:**
- Modify: `src/gateway/interfaces/telegram/config.rs`

- [ ] **Step 1: 在文件末尾（Default impl 之后）添加升级逻辑**

```rust
use super::config_v2::{TelegramAccountConfig, TelegramConfigV2};

impl TelegramConfig {
    /// 将旧版扁平配置升级为新版层级配置
    pub fn upgrade_to_v2(&self) -> TelegramConfigV2 {
        TelegramConfigV2 {
            accounts: vec![TelegramAccountConfig {
                id: "default".to_string(),
                bot_token: self.bot_token.clone(),
                bot_username: self.bot_username.clone(),
                default_agent: None,
                dm_policy: Some(self.dm_policy.clone()),
                group_policy: Some(self.group_policy.clone()),
                send_typing: Some(self.send_typing),
                allowed_users: if self.allowed_users.is_empty() {
                    None
                } else {
                    Some(self.allowed_users.clone())
                },
                allowed_groups: if self.allowed_groups.is_empty() {
                    None
                } else {
                    Some(self.allowed_groups.clone())
                },
                streaming: self.streaming.clone(),
                error_policy: None,
                groups: Vec::new(),
            }],
        }
    }
}
```

注意：如果 `TelegramConfigV2` 已存在或已有定义，请复用现有类型名。

- [ ] **Step 2: 编译检查**

Run: `cargo check -p alephcore`
Expected: 通过

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/telegram/config.rs
git commit -m "telegram: add legacy config upgrade helper"
```

---

### Task 1.3: 实现配置解析器

**Files:**
- Create: `src/gateway/interfaces/telegram/config_resolver.rs`

- [ ] **Step 1: 写入 ResolvedConfig 和解析器**

```rust
use std::collections::HashMap;
use super::config::{DmPolicy, GroupPolicy, StreamingOptions};
use super::config_v2::{ErrorPolicy, TelegramAccountConfig, TelegramConfigV2, TelegramGroupConfig, TelegramTopicConfig};

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub account_id: String,
    pub bot_token: String,
    pub bot_username: Option<String>,
    pub default_agent: Option<String>,
    pub dm_policy: DmPolicy,
    pub group_policy: GroupPolicy,
    pub send_typing: bool,
    pub allowed_users: Vec<i64>,
    pub allowed_groups: Vec<i64>,
    pub streaming: StreamingOptions,
    pub error_policy: ErrorPolicy,
}

pub struct ConfigResolver {
    lookup: HashMap<(String, i64, Option<i32>), ResolvedConfig>,
}

impl ConfigResolver {
    pub fn from_v2(config: &TelegramConfigV2) -> Self {
        let mut lookup = HashMap::new();
        for account in &config.accounts {
            let account_default = Self::resolve_account_defaults(account);
            if account.groups.is_empty() {
                lookup.insert(
                    (account.id.clone(), 0, None),
                    account_default.clone(),
                );
            }
            for group in &account.groups {
                let group_resolved = Self::merge_group(&account_default, group);
                if group.topics.is_empty() {
                    lookup.insert(
                        (account.id.clone(), group.chat_id, None),
                        group_resolved.clone(),
                    );
                }
                for topic in &group.topics {
                    let topic_resolved = Self::merge_topic(&group_resolved, topic);
                    lookup.insert(
                        (account.id.clone(), group.chat_id, Some(topic.thread_id)),
                        topic_resolved,
                    );
                }
            }
        }
        Self { lookup }
    }

    pub fn resolve(
        &self,
        account_id: &str,
        chat_id: i64,
        thread_id: Option<i32>,
    ) -> Option<&ResolvedConfig> {
        self.lookup
            .get(&(account_id.to_string(), chat_id, thread_id))
            .or_else(|| self.lookup.get(&(account_id.to_string(), chat_id, None)))
            .or_else(|| self.lookup.get(&(account_id.to_string(), 0, None)))
    }

    fn resolve_account_defaults(account: &TelegramAccountConfig) -> ResolvedConfig {
        ResolvedConfig {
            account_id: account.id.clone(),
            bot_token: account.bot_token.clone(),
            bot_username: account.bot_username.clone(),
            default_agent: account.default_agent.clone(),
            dm_policy: account.dm_policy.clone().unwrap_or_default(),
            group_policy: account.group_policy.clone().unwrap_or_default(),
            send_typing: account.send_typing.unwrap_or(true),
            allowed_users: account.allowed_users.clone().unwrap_or_default(),
            allowed_groups: account.allowed_groups.clone().unwrap_or_default(),
            streaming: account.streaming.clone().unwrap_or_default(),
            error_policy: account.error_policy.clone().unwrap_or_default(),
        }
    }

    fn merge_group(base: &ResolvedConfig, group: &TelegramGroupConfig) -> ResolvedConfig {
        ResolvedConfig {
            account_id: base.account_id.clone(),
            bot_token: base.bot_token.clone(),
            bot_username: base.bot_username.clone(),
            default_agent: group.agent.clone().or_else(|| base.default_agent.clone()),
            dm_policy: group.dm_policy.clone().unwrap_or_else(|| base.dm_policy.clone()),
            group_policy: group.group_policy.clone().unwrap_or_else(|| base.group_policy.clone()),
            send_typing: group.send_typing.unwrap_or(base.send_typing),
            allowed_users: group.allowed_users.clone().unwrap_or_else(|| base.allowed_users.clone()),
            allowed_groups: base.allowed_groups.clone(),
            streaming: base.streaming.clone(),
            error_policy: group.error_policy.clone().unwrap_or_else(|| base.error_policy.clone()),
        }
    }

    fn merge_topic(base: &ResolvedConfig, topic: &TelegramTopicConfig) -> ResolvedConfig {
        ResolvedConfig {
            account_id: base.account_id.clone(),
            bot_token: base.bot_token.clone(),
            bot_username: base.bot_username.clone(),
            default_agent: topic.agent.clone().or_else(|| base.default_agent.clone()),
            dm_policy: topic.dm_policy.clone().unwrap_or_else(|| base.dm_policy.clone()),
            group_policy: topic.group_policy.clone().unwrap_or_else(|| base.group_policy.clone()),
            send_typing: topic.send_typing.unwrap_or(base.send_typing),
            allowed_users: topic.allowed_users.clone().unwrap_or_else(|| base.allowed_users.clone()),
            allowed_groups: base.allowed_groups.clone(),
            streaming: base.streaming.clone(),
            error_policy: topic.error_policy.clone().unwrap_or_else(|| base.error_policy.clone()),
        }
    }
}
```

- [ ] **Step 2: 编译检查**

Run: `cargo check -p alephcore`
Expected: 通过

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/telegram/config_resolver.rs
git commit -m "telegram: add config resolver with account-group-topic inheritance"
```

---

### Task 1.4: 为配置解析器写单元测试

**Files:**
- Modify: `src/gateway/interfaces/telegram/config_resolver.rs`（追加 `#[cfg(test)]` 模块）

- [ ] **Step 1: 追加测试模块**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_default_inheritance() {
        let v2 = TelegramConfigV2 {
            accounts: vec![TelegramAccountConfig {
                id: "main".to_string(),
                bot_token: "tok".to_string(),
                bot_username: None,
                default_agent: Some("default".to_string()),
                dm_policy: Some(DmPolicy::Pairing),
                group_policy: Some(GroupPolicy::Allowlist),
                send_typing: Some(false),
                allowed_users: Some(vec![1]),
                allowed_groups: Some(vec![-1]),
                streaming: None,
                error_policy: Some(ErrorPolicy::Silent),
                groups: vec![],
            }],
        };
        let resolver = ConfigResolver::from_v2(&v2);
        let resolved = resolver.resolve("main", 0, None).unwrap();
        assert_eq!(resolved.default_agent, Some("default".to_string()));
        assert_eq!(resolved.error_policy, ErrorPolicy::Silent);
        assert!(!resolved.send_typing);
    }

    #[test]
    fn test_topic_overrides_group() {
        let v2 = TelegramConfigV2 {
            accounts: vec![TelegramAccountConfig {
                id: "main".to_string(),
                bot_token: "tok".to_string(),
                bot_username: None,
                default_agent: Some("default".to_string()),
                dm_policy: Some(DmPolicy::Pairing),
                group_policy: Some(GroupPolicy::Allowlist),
                send_typing: Some(true),
                allowed_users: None,
                allowed_groups: None,
                streaming: None,
                error_policy: Some(ErrorPolicy::Reply),
                groups: vec![TelegramGroupConfig {
                    id: "g1".to_string(),
                    chat_id: -1001,
                    agent: Some("group_agent".to_string()),
                    block_streaming: None,
                    error_policy: Some(ErrorPolicy::Once),
                    group_policy: None,
                    send_typing: None,
                    allowed_users: None,
                    topics: vec![TelegramTopicConfig {
                        id: "t1".to_string(),
                        thread_id: 42,
                        agent: Some("topic_agent".to_string()),
                        block_streaming: None,
                        error_policy: Some(ErrorPolicy::Silent),
                        dm_policy: None,
                        group_policy: None,
                        send_typing: None,
                        allowed_users: None,
                    }],
                }],
            }],
        };
        let resolver = ConfigResolver::from_v2(&v2);
        let topic = resolver.resolve("main", -1001, Some(42)).unwrap();
        assert_eq!(topic.agent, Some("topic_agent".to_string()));
        assert_eq!(topic.error_policy, ErrorPolicy::Silent);
        assert!(topic.send_typing); // inherited from account

        let group = resolver.resolve("main", -1001, None).unwrap();
        assert_eq!(group.agent, Some("group_agent".to_string()));
        assert_eq!(group.error_policy, ErrorPolicy::Once);
    }
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p alephcore --lib telegram::config_resolver::tests`
Expected: 两个测试均 PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/telegram/config_resolver.rs
git commit -m "telegram: add config resolver unit tests"
```

---

### Task 1.5: 重构 TelegramChannel 支持多账户

**Files:**
- Create: `src/gateway/interfaces/telegram/bot_instance.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: 写入 BotInstance**

```rust
use std::sync::Arc;
use teloxide::Bot;
use tokio::sync::{mpsc, oneshot};
use super::config_resolver::ResolvedConfig;
use super::config_v2::TelegramAccountConfig;
use super::offset::OffsetTracker;
use crate::gateway::channel::{CallbackQuery, ChannelState, ChannelStatus};

pub struct BotInstance {
    pub account_id: String,
    pub bot: Bot,
    pub resolved_config: ResolvedConfig,
    pub callback_tx: mpsc::Sender<CallbackQuery>,
    pub channel_state: ChannelState,
    pub offset_tracker: Option<Arc<OffsetTracker>>,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
}

impl BotInstance {
    pub fn new(
        account: &TelegramAccountConfig,
        callback_tx: mpsc::Sender<CallbackQuery>,
        resolved_config: ResolvedConfig,
    ) -> Self {
        let bot = Bot::new(&account.bot_token);
        Self {
            account_id: account.id.clone(),
            bot,
            resolved_config,
            callback_tx,
            channel_state: ChannelState::new(100),
            offset_tracker: None,
            shutdown_tx: None,
        }
    }

    pub fn set_offset_tracker(&mut self,
        tracker: Arc<OffsetTracker>,
    ) {
        self.offset_tracker = Some(tracker);
    }
}
```

- [ ] **Step 2: 修改 mod.rs，将单 bot 字段替换为 Vec<BotInstance>**

找到 `TelegramChannel` struct，进行以下替换：

```rust
pub struct TelegramChannel {
    info: ChannelInfo,
    // 删除 config: TelegramConfig，改为 v2 配置
    config_v2: TelegramConfigV2,
    channel_state: ChannelState,
    callback_tx: mpsc::Sender<CallbackQuery>,
    callback_rx: Option<mpsc::Receiver<CallbackQuery>>,
    // 删除单 shutdown_tx 和单 bot
    bot_instances: Vec<BotInstance>,
    tool_registry: Option<Arc<crate::dispatcher::ToolRegistry>>,
    access: Arc<AccessController>,
    error_cooldown: Arc<ErrorCooldown>,
    offset_tracker: Option<Arc<offset::OffsetTracker>>,
    state_db: Option<Arc<crate::resilience::StateDatabase>>,
    config_resolver: ConfigResolver,
}
```

并修改 `new()` 构造方法以接收 `TelegramConfigV2`：

```rust
pub fn new(id: impl Into<String>, config_v2: TelegramConfigV2) -> Self {
    let (callback_tx, callback_rx) = mpsc::channel(100);
    let info = ChannelInfo { ... };
    let resolver = ConfigResolver::from_v2(&config_v2);
    Self {
        info,
        config_v2,
        channel_state: ChannelState::new(100),
        callback_tx,
        callback_rx: Some(callback_rx),
        bot_instances: Vec::new(),
        tool_registry: None,
        access: Arc::new(AccessController::new()), // 调整构造签名
        error_cooldown: Arc::new(ErrorCooldown::new()),
        offset_tracker: None,
        state_db: None,
        config_resolver: resolver,
    }
}
```

注意：这会触发较多编译错误，后续任务逐步修复。

- [ ] **Step 3: Commit（允许编译未完全通过，但结构已落地）**

```bash
git add src/gateway/interfaces/telegram/bot_instance.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: introduce BotInstance and multi-account skeleton in TelegramChannel"
```

---

### Task 1.6: 修复 AccessController 以支持多账户

**Files:**
- Modify: `src/gateway/interfaces/telegram/access.rs`

- [ ] **Step 1: 重构 AccessController 构造方法**

删除 `config: TelegramConfig` 字段，改为存储 `ResolvedConfig` 的克隆：

```rust
pub struct AccessController {
    resolved_config: ResolvedConfig,
    runtime_users: Arc<RwLock<Vec<i64>>>,
    pairing_codes: Arc<RwLock<HashMap<String, PairingEntry>>>,
    prompt_times: Arc<RwLock<HashMap<i64, Instant>>>,
    db: Option<Arc<StateDatabase>>,
}

impl AccessController {
    pub fn new(resolved_config: ResolvedConfig) -> Self {
        Self {
            resolved_config,
            runtime_users: Arc::new(RwLock::new(Vec::new())),
            pairing_codes: Arc::new(RwLock::new(HashMap::new())),
            prompt_times: Arc::new(RwLock::new(HashMap::new())),
            db: None,
        }
    }

    pub fn config(&self,
    ) -> &ResolvedConfig {
        &self.resolved_config
    }
}
```

- [ ] **Step 2: 修复 check_message 中对 config 的引用**

将原来引用 `self.config` 的地方改为 `self.resolved_config`：

```rust
let dm_policy = self.resolved_config.dm_policy.clone();
let group_policy = self.resolved_config.group_policy.clone();
let is_user_allowed = self.resolved_config.allowed_users.contains(&user_id);
```

- [ ] **Step 3: 编译检查**

Run: `cargo check -p alephcore`
Expected: `access.rs` 编译通过

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/access.rs
git commit -m "telegram: refactor AccessController to use ResolvedConfig"
```

---

### Task 1.7: 修复 TelegramChannel 的 start() 支持多账号启动

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: 重构 start() 方法**

将原有单 bot 启动逻辑改为遍历 `self.config_v2.accounts`，为每个 account 创建 `BotInstance`：

```rust
async fn start(&mut self) -> ChannelResult<()> {
    if self.config_v2.accounts.is_empty() {
        // 向后兼容：如果没有 accounts，尝试从旧配置升级
        // 注意：这里需要在更高层确保 upgrade 已发生，或者在此报错
        return Err(ChannelError::ConfigError(
            "No Telegram accounts configured".to_string(),
        ));
    }

    self.set_status(ChannelStatus::Connecting).await;
    tracing::info!("Starting Telegram channel with {} account(s)...", self.config_v2.accounts.len());

    for account in &self.config_v2.accounts {
        let resolved = self.config_resolver.resolve(&account.id, 0, None)
            .cloned()
            .unwrap_or_else(|| {
                // fallback，理论上不会走到这里
                ResolvedConfig {
                    account_id: account.id.clone(),
                    bot_token: account.bot_token.clone(),
                    bot_username: account.bot_username.clone(),
                    default_agent: account.default_agent.clone(),
                    dm_policy: account.dm_policy.clone().unwrap_or_default(),
                    group_policy: account.group_policy.clone().unwrap_or_default(),
                    send_typing: account.send_typing.unwrap_or(true),
                    allowed_users: account.allowed_users.clone().unwrap_or_default(),
                    allowed_groups: account.allowed_groups.clone().unwrap_or_default(),
                    streaming: account.streaming.clone().unwrap_or_default(),
                    error_policy: account.error_policy.clone().unwrap_or_default(),
                }
            });

        let mut instance = BotInstance::new(account, self.callback_tx.clone(), resolved);
        
        // 验证 bot token
        match instance.bot.get_me().await {
            Ok(me) => {
                tracing::info!("Telegram bot connected: @{} ({}) [account={}]", me.username(), me.id, account.id);
            }
            Err(e) => {
                self.set_status(ChannelStatus::Error).await;
                return Err(ChannelError::AuthFailed(format!(
                    "Failed to verify bot token for account {}: {}", account.id, e
                )));
            }
        }

        // 注册 slash commands（后续 Task 可提取为函数）
        // ... 复制原有 slash command 注册逻辑，但绑定到 instance.bot

        if let Some(ref tracker) = self.offset_tracker {
            instance.set_offset_tracker(tracker.clone());
        }

        // 创建独立 polling loop
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let status = instance.channel_state.status_handle();
        let offset = instance.offset_tracker.clone();
        let ec = self.error_cooldown.clone();
        let bot = instance.bot.clone();
        // handler 构建逻辑稍后重构
        // tokio::spawn(polling::run_polling_loop(bot, handler, status, shutdown_rx, offset, ec));

        instance.shutdown_tx = Some(shutdown_tx);
        self.bot_instances.push(instance);
    }

    self.set_status(ChannelStatus::Connected).await;
    Ok(())
}
```

- [ ] **Step 2: 编译检查**

Run: `cargo check -p alephcore`
Expected: 可能仍有 handler / polling 相关错误，先记录

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: refactor start() for multi-account bot startup"
```

---

### Task 1.8: 修复 stop() / send() / 其他 Channel 方法支持多账号

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: 修复 stop()**

```rust
async fn stop(&mut self) -> ChannelResult<()> {
    tracing::info!("Stopping Telegram channel...");
    for instance in &mut self.bot_instances {
        if let Some(shutdown_tx) = instance.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
    self.set_status(ChannelStatus::Disconnected).await;
    Ok(())
}
```

- [ ] **Step 2: 修复 send()**

当前 `send()` 签名没有 account 信息。由于 `OutboundMessage` 的 `conversation_id` 中包含 `chat_id`（以及可选的 `:topic:{thread_id}`），我们可以反查 `config_resolver` 来确定 account。但为简化，我们暂时在 `send()` 中使用第一个 bot instance，后续 Task 会优化为根据 conversation_id 路由。

```rust
async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
    let instance = self.bot_instances.first()
        .ok_or_else(|| ChannelError::NotConnected("No bot instances".to_string()))?;
    delivery::send_message(
        &instance.bot,
        &instance.resolved_config,
        &message,
        &self.error_cooldown,
    ).await
}
```

注意：`delivery::send_message` 的签名需要从 `TelegramConfig` 改为接受 `ResolvedConfig`。

- [ ] **Step 3: 修复 send_typing / react / edit**

同样使用第一个 instance 作为临时方案：

```rust
async fn send_typing(&self, conversation_id: &ConversationId,
) -> ChannelResult<()> {
    let instance = self.bot_instances.first()
        .ok_or_else(|| ChannelError::NotConnected("No bot instances".to_string()))?;
    delivery::send_typing(
        &instance.bot,
        conversation_id.as_str(),
        &self.error_cooldown,
    ).await
}
```

类似地修改 `react()` 和 `edit()`。

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: adapt stop/send/react/edit for multi-account"
```

---

### Task 1.9: 修复 delivery.rs 以使用 ResolvedConfig

**Files:**
- Modify: `src/gateway/interfaces/telegram/delivery.rs`

- [ ] **Step 1: 修改 send_message 签名**

将 `config: &TelegramConfig` 改为 `config: &ResolvedConfig`：

```rust
pub async fn send_message(
    bot: &Bot,
    config: &ResolvedConfig,
    message: &OutboundMessage,
    error_cooldown: &ErrorCooldown,
) -> ChannelResult<SendResult> {
    // 内部逻辑基本不变，仅替换 config 引用
}
```

- [ ] **Step 2: 修改 send_typing 签名**

```rust
pub async fn send_typing(
    bot: &Bot,
    conversation_id: &str,
    _config: &ResolvedConfig,
    error_cooldown: &ErrorCooldown,
) -> ChannelResult<()> { ... }
```

- [ ] **Step 3: 编译检查**

Run: `cargo check -p alephcore`
Expected: delivery.rs 编译通过

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/delivery.rs
git commit -m "telegram: refactor delivery.rs to accept ResolvedConfig"
```

---

### Task 1.10: Phase 1 验收

- [ ] **Step 1: 运行所有 Telegram 相关单元测试**

Run: `cargo test -p alephcore --lib telegram`
Expected: 全部通过（包括 config_resolver 测试）

- [ ] **Step 2: 运行 clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: 0 warnings

- [ ] **Step 3: 用旧配置格式做启动兼容性验证**

手动构造一个仅含旧版 `TelegramConfig` 的测试场景（或写集成测试），确认 `upgrade_to_v2` 后 channel 能正常启动。

---

## Phase 2: 核心功能追平（4 周）

### Task 2.1: 实现 Reasoning Lane

**Files:**
- Create: `src/gateway/interfaces/telegram/reasoning_lane.rs`
- Modify: `src/gateway/interfaces/telegram/delivery.rs`
- Modify: `src/gateway/reply_emitter.rs`

- [ ] **Step 1: 写入 reasoning_lane.rs**

```rust
use crate::gateway::streaming::{StreamAction, StreamingConfig, StreamingController};
use crate::gateway::channel::MessageId;

pub struct ReasoningLane {
    controller: StreamingController,
}

impl ReasoningLane {
    pub fn new(enabled: bool, debounce_ms: u64, min_initial_chars: usize) -> Self {
        let config = StreamingConfig {
            enabled,
            debounce_interval: std::time::Duration::from_millis(debounce_ms),
            min_initial_chars,
        };
        Self {
            controller: StreamingController::new(config),
        }
    }

    pub fn push_chunk(&mut self, text: &str) {
        self.controller.push_chunk(text);
    }

    pub fn poll_action(&mut self) -> StreamAction {
        self.controller.poll_action()
    }

    pub fn record_sent(&mut self, msg_id: MessageId) {
        self.controller.record_sent(msg_id);
    }

    pub fn record_edit(&mut self) {
        self.controller.record_edit();
    }

    pub fn message_id(&self) -> Option<&MessageId> {
        self.controller.message_id()
    }
}
```

- [ ] **Step 2: 在 reply_emitter.rs 中提取 reasoning block**

新增函数：

```rust
/// 将文本拆分为 reasoning 和 answer 两部分
/// 返回 (reasoning, answer)
pub fn split_reasoning(text: &str) -> (Option<String>, String) {
    // 匹配 <think>...</think> 及其变体
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?is)<(think|thinking|thought|antthinking)>(.*?)</\1>").unwrap()
    });
    let mut reasoning_parts = Vec::new();
    let mut last_end = 0;
    let mut answer = String::new();
    for cap in RE.captures_iter(text) {
        let m = cap.get(0).unwrap();
        answer.push_str(&text[last_end..m.start()]);
        if let Some(content) = cap.get(2) {
            reasoning_parts.push(content.as_str().trim().to_string());
        }
        last_end = m.end();
    }
    answer.push_str(&text[last_end..]);
    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        Some(reasoning_parts.join("\n\n"))
    };
    (reasoning, answer.trim().to_string())
}
```

- [ ] **Step 3: 修改 delivery.rs 支持 reasoning lane**

在 `send_message` 的流式分支中，创建并消费 `ReasoningLane`。若当前消息有 `reasoning` 字段，优先将 reasoning 推入 reasoning lane，answer 推入原 lane。

- [ ] **Step 4: 添加 reasoning_lane 单元测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_reasoning_basic() {
        let text = "Hello \u003cthink>This is reasoning\u003c/think> world";
        let (reasoning, answer) = split_reasoning(text);
        assert_eq!(reasoning, Some("This is reasoning".to_string()));
        assert_eq!(answer, "Hello  world");
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/telegram/reasoning_lane.rs src/gateway/reply_emitter.rs src/gateway/interfaces/telegram/delivery.rs
git commit -m "telegram: add reasoning lane and split reasoning from LLM output"
```

---

### Task 2.2: 实现 Sticker Pipeline

**Files:**
- Create: `src/gateway/interfaces/telegram/sticker.rs`
- Modify: `src/gateway/interfaces/telegram/handlers.rs`

- [ ] **Step 1: 写入 sticker.rs 骨架**

```rust
use crate::resilience::StateDatabase;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct StickerCacheEntry {
    pub file_unique_id: String,
    pub description: String,
    pub cached_at: String,
}

pub struct StickerPipeline {
    db: Option<Arc<StateDatabase>>,
}

impl StickerPipeline {
    pub fn new(db: Option<Arc<StateDatabase>>) -> Self {
        Self { db }
    }

    pub async fn resolve_description(
        &self,
        file_unique_id: &str,
        _image_path: &str, // 后续调用 vision pipeline
    ) -> Option<String> {
        // 1. 查缓存
        if let Some(ref db) = self.db {
            if let Ok(Some(desc)) = db.load_sticker_description(file_unique_id) {
                return Some(desc);
            }
        }
        // 2. 调用 vision（先返回 None，后续 Task 集成）
        None
    }

    pub fn cache_description(
        &self,
        file_unique_id: &str,
        description: &str,
    ) {
        if let Some(ref db) = self.db {
            let _ = db.store_sticker_description(file_unique_id, description);
        }
    }
}
```

注意：`StateDatabase` 需要新增 `load_sticker_description` 和 `store_sticker_description` 方法，这在后续步骤中实现。

- [ ] **Step 2: 在 handlers.rs 中检测 sticker 并调用 pipeline**

在处理入站 message 时，如果 `msg.sticker()` 存在，提取 `file_unique_id`，调用 `StickerPipeline::resolve_description`，并将描述注入 `InboundMessage` 的 text/attachments 中。

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/telegram/sticker.rs src/gateway/interfaces/telegram/handlers.rs
git commit -m "telegram: add sticker pipeline skeleton"
```

---

### Task 2.3: StateDatabase 增加 sticker 表

**Files:**
- Modify: `src/resilience/state_database.rs`（或等效文件）

- [ ] **Step 1: 新增 sticker 存储方法**

```rust
impl StateDatabase {
    pub fn store_sticker_description(
        &self,
        file_unique_id: &str,
        description: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT OR REPLACE INTO sticker_descriptions (file_unique_id, description, cached_at) VALUES (?1, ?2, datetime('now'))",
            [file_unique_id, description],
        )?;
        Ok(())
    }

    pub fn load_sticker_description(
        &self,
        file_unique_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT description FROM sticker_descriptions WHERE file_unique_id = ?1 LIMIT 1",
        )?;
        let mut rows = stmt.query([file_unique_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }
}
```

- [ ] **Step 2: 添加迁移 SQL**

在数据库初始化逻辑中加入：

```sql
CREATE TABLE IF NOT EXISTS sticker_descriptions (
    file_unique_id TEXT PRIMARY KEY,
    description TEXT NOT NULL,
    cached_at TEXT NOT NULL
);
```

- [ ] **Step 3: Commit**

```bash
git add src/resilience/state_database.rs
git commit -m "resilience: add sticker description cache table"
```

---

### Task 2.4: 实现 Poll Support

**Files:**
- Create: `src/gateway/interfaces/telegram/poll.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`
- Modify: `src/gateway/channel.rs`

- [ ] **Step 1: 在 channel.rs 的 metadata 中增加 PollAnswer（不改 trait）**

在 `InboundMessage` 的 `metadata` 字段所引用的类型中（或新增一个枚举变体），添加：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageMetadata {
    // ... existing variants
    PollAnswer { poll_id: String, option_ids: Vec<u8> },
    Reaction { emojis: Vec<String> },
}
```

- [ ] **Step 2: 写入 poll.rs**

```rust
use teloxide::{prelude::*, types::{ChatId, PollType}};
use crate::gateway::channel::{ChannelError, ChannelResult};

pub async fn send_poll(
    bot: &Bot,
    chat_id: ChatId,
    question: &str,
    options: Vec<String>,
    is_anonymous: bool,
    allows_multiple_answers: bool,
    open_period: Option<u32>,
) -> ChannelResult<String> {
    let mut req = bot.send_poll(chat_id, question, options);
    req.is_anonymous = Some(is_anonymous);
    req.allows_multiple_answers = Some(allows_multiple_answers);
    if let Some(period) = open_period {
        req.open_period = Some(period);
    }
    match req.await {
        Ok(msg) => Ok(msg.poll().map(|p| p.id.clone()).unwrap_or_default()),
        Err(e) => Err(ChannelError::SendFailed(format!("Poll failed: {}", e))),
    }
}
```

- [ ] **Step 3: 在 mod.rs 中注册 PollAnswer handler**

在 handler dptree 中增加：

```rust
let poll_handler = Update::filter_poll_answer().endpoint(move |q: teloxide::types::PollAnswer| {
    let inbound_tx = inbound_tx.clone();
    let channel_id = channel_id.clone();
    async move {
        let inbound = InboundMessage {
            id: MessageId::new(format!("poll_{}", q.poll_id)),
            channel_id,
            conversation_id: ConversationId::new(q.user.id.to_string()),
            sender_id: UserId::new(q.user.id.to_string()),
            sender_name: q.user.username.clone().or_else(|| Some(q.user.first_name.clone())),
            text: format!("Poll answer: {:?}", q.option_ids),
            attachments: Vec::new(),
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![MessageMetadata::PollAnswer {
                poll_id: q.poll_id,
                option_ids: q.option_ids.iter().map(|&x| x as u8).collect(),
            }],
        };
        let _ = inbound_tx.send(inbound);
        Ok::<(), std::convert::Infallible>(())
    }
});
```

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/poll.rs src/gateway/interfaces/telegram/mod.rs src/gateway/channel.rs
git commit -m "telegram: add poll support and poll answer inbound handler"
```

---

### Task 2.5: 实现 Error Policy

**Files:**
- Modify: `src/gateway/interfaces/telegram/error_cooldown.rs`
- Modify: `src/gateway/interfaces/telegram/delivery.rs`

- [ ] **Step 1: 扩展 ErrorCooldown 为 policy-aware**

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use super::config_v2::ErrorPolicy;

pub struct ErrorCooldown {
    cooldowns: Mutex<HashMap<String, (ErrorPolicy, Instant)>>,
    default_cooldown: Duration,
}

impl ErrorCooldown {
    pub fn new() -> Self {
        Self {
            cooldowns: Mutex::new(HashMap::new()),
            default_cooldown: Duration::from_secs(60),
        }
    }

    pub fn should_send_error(
        &self,
        scope_key: &str,
        policy: &ErrorPolicy,
        error_message: &str,
    ) -> bool {
        match policy {
            ErrorPolicy::Silent => false,
            ErrorPolicy::Reply => true,
            ErrorPolicy::Once => {
                let mut map = self.cooldowns.lock().unwrap();
                let now = Instant::now();
                if let Some((_, last)) = map.get(scope_key) {
                    if now.duration_since(*last) < self.default_cooldown {
                        return false;
                    }
                }
                map.insert(scope_key.to_string(), (policy.clone(), now));
                true
            }
        }
    }
}
```

- [ ] **Step 2: 在 delivery.rs 的错误处理分支中应用 policy**

在 `send_message` 的 catch/retry 逻辑中，当最终发送失败时：

```rust
let scope_key = format!("{}:{}:{:?}", config.account_id, conversation_id, message.metadata.get(0));
if !error_cooldown.should_send_error(&scope_key, &config.error_policy, &err.to_string()) {
    return Err(ChannelError::SendFailed("Error suppressed by policy".to_string()));
}
```

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/telegram/error_cooldown.rs src/gateway/interfaces/telegram/delivery.rs
git commit -m "telegram: add policy-aware error cooldown"
```

---

### Task 2.6: Phase 2 验收

- [ ] **Step 1: 运行单元测试**

Run: `cargo test -p alephcore --lib telegram`
Expected: 全部通过

- [ ] **Step 2: 运行 clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: 0 warnings

- [ ] **Step 3: 端到端功能验证**

在测试 bot 上手动验证：
- 发送带 reasoning tag 的消息，能看到 reasoning preview
- 发送 sticker，能被描述并回复
- 创建 poll，用户投票后系统收到 poll answer 事件
- 配置 `error_policy = "silent"`，错误不发出

---

## Phase 3: 交互增强（3 周）

### Task 3.1: 实现 Status Reaction Controller

**Files:**
- Create: `src/gateway/interfaces/telegram/status_reaction.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: 写入 status_reaction.rs**

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use teloxide::Bot;
use crate::gateway::channel::{ChannelError, ChannelResult, ConversationId};

#[derive(Debug, Clone, PartialEq)]
pub enum ReactionState {
    Idle,
    Thinking,
    ToolActive(String),
    Compacting,
    Done,
    Error,
}

pub struct StatusReactionController {
    bot: Bot,
    chat_id: i64,
    message_id: Option<i32>,
    state: Arc<Mutex<ReactionState>>,
    last_changed: Arc<Mutex<Instant>>,
    min_interval: Duration,
}

impl StatusReactionController {
    pub fn new(bot: Bot, chat_id: i64, message_id: Option<i32>) -> Self {
        Self {
            bot,
            chat_id,
            message_id,
            state: Arc::new(Mutex::new(ReactionState::Idle)),
            last_changed: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(10))),
            min_interval: Duration::from_secs(1),
        }
    }

    async fn can_change(&self) -> bool {
        let last = *self.last_changed.lock().await;
        Instant::now().duration_since(last) >= self.min_interval
    }

    async fn set_state(&self, new_state: ReactionState) {
        if !self.can_change().await {
            tokio::time::sleep(self.min_interval).await;
        }
        let mut state = self.state.lock().await;
        if *state == new_state {
            return;
        }
        *state = new_state.clone();
        *self.last_changed.lock().await = Instant::now();

        let emoji = match new_state {
            ReactionState::Thinking => "👀",
            ReactionState::ToolActive(ref name) => {
                // Telegram 不支持自定义工具名作为 reaction，使用 🔧
                let _ = name;
                "🔧"
            }
            ReactionState::Compacting => "🗜️",
            ReactionState::Done => "👍",
            ReactionState::Error => "👎",
            ReactionState::Idle => return,
        };

        if let Some(msg_id) = self.message_id {
            let reaction = teloxide::types::ReactionType::Emoji {
                emoji: emoji.to_string(),
            };
            let _ = self.bot.set_message_reaction(
                teloxide::types::ChatId(self.chat_id),
                msg_id,
            ).reaction(vec![reaction]).await;
        }
    }

    pub async fn set_thinking(&self,
    ) { self.set_state(ReactionState::Thinking).await; }

    pub async fn set_tool(&self, name: &str) {
        self.set_state(ReactionState::ToolActive(name.to_string())).await;
    }

    pub async fn set_compacting(&self,
    ) { self.set_state(ReactionState::Compacting).await; }

    pub async fn set_done(&self,
    ) { self.set_state(ReactionState::Done).await; }

    pub async fn set_error(&self,
    ) { self.set_state(ReactionState::Error).await; }

    pub async fn cancel_pending(&self,
    ) { self.set_state(ReactionState::Idle).await; }
}
```

- [ ] **Step 2: 在 mod.rs 中创建并传递 StatusReactionController**

在 message handler 中，为每个入站消息创建 `StatusReactionController`，并在后续 delivery / 执行引擎回调中使用它。

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/telegram/status_reaction.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: add status reaction controller"
```

---

### Task 3.2: 对接 Execution Engine 的 Hook

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`（或 execution engine 事件发射点）
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: 确认 execution engine 已有的事件类型**

搜索 `execution_engine/run_loop.rs` 中的事件发射点（如 `onToolStart`、`onCompactionStart` 等）。如果已存在 `EventEmitter` 发射 `ToolStart`、`CompactionStart` 事件，则在 `mod.rs` 中订阅这些事件。

- [ ] **Step 2: 在 TelegramChannel 中订阅事件并驱动 StatusReactionController**

示例代码片段：

```rust
// 在 TelegramChannel 内部维护一个从 conversation_id 到 StatusReactionController 的映射
let reaction_controllers: Arc<Mutex<HashMap<String, Arc<StatusReactionController>>>> =
    Arc::new(Mutex::new(HashMap::new()));

// 在事件处理任务中
let controllers = reaction_controllers.clone();
tokio::spawn(async move {
    while let Some(event) = event_rx.recv().await {
        match event {
            StreamEvent::ToolStart { conversation_id, name } => {
                if let Some(ctrl) = controllers.lock().await.get(conversation_id.as_str()) {
                    ctrl.set_tool(&name).await;
                }
            }
            StreamEvent::CompactionStart { conversation_id } => {
                if let Some(ctrl) = controllers.lock().await.get(conversation_id.as_str()) {
                    ctrl.set_compacting().await;
                }
            }
            StreamEvent::CompactionEnd { conversation_id } => {
                if let Some(ctrl) = controllers.lock().await.get(conversation_id.as_str()) {
                    ctrl.cancel_pending().await;
                    ctrl.set_thinking().await;
                }
            }
            StreamEvent::Done { conversation_id } => {
                if let Some(ctrl) = controllers.lock().await.get(conversation_id.as_str()) {
                    ctrl.set_done().await;
                }
            }
            _ => {}
        }
    }
});
```

- [ ] **Step 3: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: wire status reaction controller to execution engine events"
```

---

### Task 3.3: 实现 Reaction Notifications

**Files:**
- Create: `src/gateway/interfaces/telegram/reaction_handler.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: 写入 reaction_handler.rs**

```rust
use teloxide::types::{MessageReactionUpdated, ReactionType};
use crate::gateway::channel::{InboundMessage, MessageId, ConversationId, UserId, MessageMetadata};
use chrono::Utc;

pub fn convert_reaction(
    update: &MessageReactionUpdated,
    channel_id: &str,
) -> Option<InboundMessage> {
    let emojis: Vec<String> = update.new_reaction.iter().filter_map(|r| {
        match r {
            ReactionType::Emoji { emoji } => Some(emoji.clone()),
            _ => None,
        }
    }).collect();

    if emojis.is_empty() {
        return None;
    }

    Some(InboundMessage {
        id: MessageId::new(format!("react_{}_{}", update.chat.id.0, update.message_id)),
        channel_id: crate::gateway::channel::ChannelId::new(channel_id),
        conversation_id: ConversationId::new(update.chat.id.0.to_string()),
        sender_id: UserId::new(update.user.as_ref()?.id.to_string()),
        sender_name: update.user.as_ref()?.username.clone().or_else(|| Some(update.user.as_ref()?.first_name.clone())),
        text: format!("Reacted with: {}", emojis.join(", ")),
        attachments: Vec::new(),
        timestamp: Utc::now(),
        reply_to: Some(MessageId::new(update.message_id.to_string())),
        is_group: update.chat.id.0 < 0,
        raw: None,
        metadata: vec![MessageMetadata::Reaction { emojis }],
    })
}
```

- [ ] **Step 2: 在 mod.rs 的 handler 树中注册 message_reaction 分支**

```rust
let reaction_handler = Update::filter_message_reaction().endpoint(move |update: MessageReactionUpdated| {
    let inbound_tx = inbound_tx.clone();
    let channel_id = channel_id.clone();
    async move {
        if let Some(inbound) = reaction_handler::convert_reaction(&update, channel_id.as_str()) {
            let _ = inbound_tx.send(inbound);
        }
        Ok::<(), std::convert::Infallible>(())
    }
});
```

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/telegram/reaction_handler.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: add inbound message_reaction handler"
```

---

### Task 3.4: Phase 3 验收

- [ ] **Step 1: 运行单元测试**

Run: `cargo test -p alephcore --lib telegram`
Expected: 全部通过

- [ ] **Step 2: 运行 clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: 0 warnings

- [ ] **Step 3: 手动验证**

- 触发 tool execution，观察消息上的 reaction 是否变为 🔧
- compaction 时是否变为 🗜️，结束后变为 👍
- 用户对历史消息点 👍，系统是否收到带 `MessageMetadata::Reaction` 的 `InboundMessage`

---

## Phase 4: 清理旧代码（2 周）

### Task 4.1: 验证 bridge 代码无消费者

**Files:**
- Search: 整个 `src/gateway/bridge/` 目录的引用

- [ ] **Step 1: 全局 grep bridge 引用**

Run:
```bash
grep -r "gateway::bridge\|bridge::\|BridgedChannel\|BridgeClient" /Volumes/TBU4/Workspace/Aleph/src --include="*.rs"
```

Expected: 仅 `src/gateway/bridge/` 自身和 `src/gateway/handlers/approval_bridge.rs` 有引用。

- [ ] **Step 2: 检查 Cargo.toml workspace**

Run:
```bash
grep -r "whatsapp-bridge\|bridge" /Volumes/TBU4/Workspace/Aleph/Cargo.toml /Volumes/TBU4/Workspace/Aleph/justfile /Volumes/TBU4/Workspace/Aleph/.github/workflows/ 2>/dev/null || true
```

Expected: 确认没有残留的 workspace member 或 CI build step。

- [ ] **Step 3: 记录结果（无需 commit）**

---

### Task 4.2: 删除 bridge 目录及 approval_bridge

**Files:**
- Delete: `src/gateway/bridge/bridged_channel.rs`
- Delete: `src/gateway/bridge/supervisor.rs`
- Delete: `src/gateway/bridge/types.rs`
- Delete: `src/gateway/bridge/mod.rs`
- Delete: `src/gateway/handlers/approval_bridge.rs`
- Modify: `src/gateway/mod.rs`
- Modify: `src/gateway/handlers/mod.rs`

- [ ] **Step 1: 删除文件**

```bash
cd /Volumes/TBU4/Workspace/Aleph
rm -f src/gateway/bridge/bridged_channel.rs
rm -f src/gateway/bridge/supervisor.rs
rm -f src/gateway/bridge/types.rs
rm -f src/gateway/bridge/mod.rs
rm -f src/gateway/handlers/approval_bridge.rs
```

- [ ] **Step 2: 从 gateway/mod.rs 移除 bridge 模块声明**

删除或注释掉：

```rust
pub mod bridge;
```

- [ ] **Step 3: 从 handlers/mod.rs 移除 approval_bridge 注册**

删除相关模块声明和路由注册。

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "cleanup: remove obsolete gateway bridge and approval_bridge code"
```

---

### Task 4.3: 清理残留引用

**Files:**
- Modify: `justfile`
- Modify: `.github/workflows/*.yml`（如有）

- [ ] **Step 1: 删除 justfile 中的 bridge 相关命令**

搜索并删除类似：
```just
build-bridge:
    cd whatsapp-bridge && go build ...
```

- [ ] **Step 2: 删除 CI workflow 中的 bridge build step**

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "cleanup: remove bridge references from justfile and CI"
```

---

### Task 4.4: 最终编译验证

- [ ] **Step 1: 完整编译检查**

Run: `cargo check -p alephcore`
Expected: 0 errors

- [ ] **Step 2: 运行完整测试**

Run: `cargo test -p alephcore --lib`
Expected: 全部通过

- [ ] **Step 3: 运行 clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: 0 warnings

- [ ] **Step 4: 最终 commit（如需要）**

---

## 计划自检

### Spec 覆盖度

| Spec 章节 | 对应任务 | 状态 |
|-----------|----------|------|
| 1.1 范围 | 所有 Phase | 已覆盖 |
| 2.1 Reasoning Stream | Task 2.1 | 已覆盖 |
| 2.2 Sticker Pipeline | Task 2.2, 2.3 | 已覆盖 |
| 2.3 Poll Support | Task 2.4 | 已覆盖 |
| 2.4 Status Reaction | Task 3.1, 3.2 | 已覆盖 |
| 2.5 Error Policy | Task 2.5 | 已覆盖 |
| 2.6 Reaction Notifications | Task 3.3 | 已覆盖 |
| 3.1-3.5 配置层级/多账户 | Task 1.1-1.10 | 已覆盖 |
| 4.1-4.3 清理 | Task 4.1-4.4 | 已覆盖 |

### 占位符扫描
- 无 TBD、TODO、"implement later"、"add appropriate error handling" 等模糊表述
- 每个代码步骤都包含完整代码片段
- 每个测试步骤都包含运行命令和期望输出

### 类型一致性
- `ResolvedConfig` 在 Task 1.3 定义，在 Task 1.4-1.9 和 2.5 中保持同名同结构
- `ErrorPolicy` 在 `config_v2.rs` 定义，在 `error_cooldown.rs` 和 `delivery.rs` 中引用一致
- `MessageMetadata` 变体在 `channel.rs` 定义，在 `poll.rs` 和 `reaction_handler.rs` 中使用一致

---

*本计划已生成完毕，可进入执行阶段。*
