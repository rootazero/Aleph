# Phase 8: Aether vs Moltbot Gap Analysis & Roadmap

**Date**: 2026-01-28
**Status**: Planning

---

## Executive Summary

经过对 Moltbot 源码的深入分析，本文档总结了 Aether 与 Moltbot 的功能差距，并规划 Phase 8 的实施路线。

---

## Current State Comparison

### Aether 已实现 ✅

| 组件 | 状态 | 对应 Moltbot |
|------|------|-------------|
| **Gateway WebSocket** | ✅ 完整 | `src/gateway/server.ts` |
| **JSON-RPC Protocol** | ✅ 完整 | `src/gateway/protocol/` |
| **Event Bus (PubSub)** | ✅ 完整 | `src/gateway/server-chat.ts` |
| **Session Manager** | ✅ 完整 (SQLite) | `src/agents/session-*` |
| **Session Compaction** | ✅ 完整 | `src/agents/compaction.ts` |
| **Agent Loop (LLM)** | ✅ 完整 | `src/agents/pi-embedded-runner/` |
| **Tool Streaming** | ✅ 完整 | `src/agents/pi-embedded-subscribe.ts` |
| **Device Pairing** | ✅ 完整 | `src/gateway/server-methods/devices.ts` |
| **Hot Config Reload** | ✅ 完整 | `src/config/config-reload.ts` |
| **WebChat UI** | ✅ 完整 | `ui/` |
| **macOS App** | ✅ 完整 | `platforms/macos/` |
| **Channel Abstraction** | ✅ 完整 | `src/channels/` |
| **CLI Channel** | ✅ 完整 | (测试用) |
| **iMessage Channel** | ✅ 完整 | `src/imessage/` |

### Aether 缺失功能 ❌

| 功能 | 优先级 | Moltbot 参考 | 复杂度 |
|------|--------|-------------|--------|
| **Telegram Channel** | 🔴 High | `src/telegram/` (81 dirs) | Medium |
| **Discord Channel** | 🔴 High | `src/discord/` (42 dirs) | Medium |
| **Slack Channel** | 🟡 Medium | `src/slack/` (36 dirs) | Medium |
| **Browser Control** | 🔴 High | `src/browser/` (70 dirs) | High |
| **Cron Jobs** | 🔴 High | `src/cron/` (23 dirs) | Medium |
| **Model Failover** | 🔴 High | `src/agents/model-fallback.ts` | Medium |
| **Canvas/A2UI** | 🟡 Medium | `src/canvas-host/` | High |
| **Voice Wake** | 🟡 Medium | (platforms) | Medium |
| **Talk Mode** | 🟡 Medium | (platforms) | Medium |
| **WhatsApp** | 🟢 Low | `src/web/` (Baileys) | High |
| **Signal** | 🟢 Low | `src/signal/` | High |
| **LINE** | 🟢 Low | `src/line/` | Medium |
| **iOS Node** | 🟢 Low | `platforms/ios/` | High |
| **Android Node** | 🟢 Low | `platforms/android/` | High |
| **Vector Memory** | 🟡 Medium | `src/memory/` (73KB manager) | High |
| **Skills Platform** | 🟡 Medium | `src/agents/skills/` | Medium |
| **Exec Approval Gate** | 🟡 Medium | `src/gateway/server-methods/exec-approval.ts` | Low |
| **Sub-agent System** | 🟡 Medium | `src/agents/subagent-*` | Medium |

---

## Phase 8 Recommended Focus

### Option A: "Channel Expansion" (推荐)

扩展消息渠道覆盖，增加用户触达范围。

```
Week 1-2: Telegram Channel
Week 3-4: Discord Channel
Week 5-6: Slack Channel (optional)
```

### Option B: "Tools & Automation"

增强工具能力和自动化。

```
Week 1-2: Browser Control (CDP)
Week 3-4: Cron Jobs
Week 5-6: Model Failover
```

### Option C: "Hybrid Approach" (平衡方案)

```
Week 1-2: Telegram Channel + Model Failover
Week 3-4: Browser Control (Basic)
Week 5-6: Cron Jobs
```

---

## Detailed Implementation Plans

### 8.1 Telegram Channel

**Moltbot Reference**: `/src/telegram/` (81 dirs, grammY framework)

**Rust Implementation**:

```rust
// core/src/gateway/channels/telegram/mod.rs

use teloxide::prelude::*;

pub struct TelegramChannel {
    bot: Bot,
    config: TelegramConfig,
    message_tx: mpsc::Sender<InboundMessage>,
}

pub struct TelegramConfig {
    pub bot_token: String,
    pub allowed_users: Vec<i64>,      // User IDs
    pub allowed_groups: Vec<i64>,     // Group/Chat IDs
    pub webhook_url: Option<String>,  // Webhook mode
    pub polling: bool,                // Long-polling mode
}

impl Channel for TelegramChannel {
    async fn start(&mut self) -> Result<(), ChannelError>;
    async fn stop(&mut self) -> Result<(), ChannelError>;
    async fn send(&self, msg: OutboundMessage) -> Result<(), ChannelError>;
    fn status(&self) -> ChannelStatus;
}
```

**Features**:
- Bot token configuration
- Long-polling or Webhook mode
- User/group allowlist
- Inline keyboards
- File/media handling
- Reply threading

**Crates**:
```toml
teloxide = { version = "0.13", features = ["macros", "auto-send"] }
```

**Effort**: ~800 LoC, 1-2 weeks

---

### 8.2 Discord Channel

**Moltbot Reference**: `/src/discord/` (42 dirs, discord.js)

**Rust Implementation**:

```rust
// core/src/gateway/channels/discord/mod.rs

use serenity::prelude::*;

pub struct DiscordChannel {
    client: Client,
    config: DiscordConfig,
    cache: Arc<Cache>,
}

pub struct DiscordConfig {
    pub bot_token: String,
    pub application_id: u64,
    pub allowed_guilds: Vec<u64>,
    pub allowed_channels: Vec<u64>,
    pub dm_allowed: bool,
}

impl Channel for DiscordChannel {
    // ... Channel trait implementation
}

// Event handler
struct Handler {
    message_tx: mpsc::Sender<InboundMessage>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        // Convert Discord message to InboundMessage
        // Send to channel message_tx
    }
}
```

**Features**:
- Bot token + OAuth2
- Guild/DM message handling
- Slash commands registration
- Embeds and reactions
- Thread support
- Voice channel (future)

**Crates**:
```toml
serenity = { version = "0.12", features = ["client", "gateway", "cache"] }
```

**Effort**: ~1000 LoC, 1-2 weeks

---

### 8.3 Browser Control (CDP)

**Moltbot Reference**: `/src/browser/` (70 dirs)

**Rust Implementation**:

```rust
// core/src/browser/mod.rs

use chromiumoxide::{Browser, BrowserConfig, Page};

pub struct BrowserManager {
    browser: Option<Browser>,
    pages: HashMap<String, Page>,
    config: BrowserConfig,
}

impl BrowserManager {
    pub async fn launch(&mut self) -> Result<()>;
    pub async fn new_page(&mut self, url: &str) -> Result<String>; // Returns page_id
    pub async fn navigate(&self, page_id: &str, url: &str) -> Result<()>;
    pub async fn click(&self, page_id: &str, selector: &str) -> Result<()>;
    pub async fn type_text(&self, page_id: &str, selector: &str, text: &str) -> Result<()>;
    pub async fn screenshot(&self, page_id: &str) -> Result<Vec<u8>>;
    pub async fn get_content(&self, page_id: &str) -> Result<String>;
    pub async fn close_page(&mut self, page_id: &str) -> Result<()>;
    pub async fn shutdown(&mut self) -> Result<()>;
}

// Agent tool wrapper
pub struct BrowserTool {
    manager: Arc<RwLock<BrowserManager>>,
}

impl Tool for BrowserTool {
    // Implements rig::tool::Tool trait
}
```

**Features**:
- Chrome/Chromium auto-discovery
- Page lifecycle management
- Navigation, clicks, typing
- Screenshots and PDF generation
- Cookie/storage management
- JavaScript execution

**Crates**:
```toml
chromiumoxide = { version = "0.7", features = ["tokio-runtime"] }
```

**Effort**: ~1500 LoC, 2-3 weeks

---

### 8.4 Cron Jobs

**Moltbot Reference**: `/src/cron/` (23 dirs)

**Rust Implementation**:

```rust
// core/src/cron/mod.rs

use cron::Schedule;
use tokio_cron_scheduler::{Job, JobScheduler};

pub struct CronService {
    scheduler: JobScheduler,
    jobs: HashMap<String, CronJob>,
    db: Arc<Connection>,
}

pub struct CronJob {
    pub id: String,
    pub name: String,
    pub schedule: String,           // Cron expression
    pub agent_id: String,
    pub prompt: String,             // Message to send to agent
    pub enabled: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
}

impl CronService {
    pub async fn start(&mut self) -> Result<()>;
    pub async fn stop(&mut self) -> Result<()>;
    pub async fn add_job(&mut self, job: CronJob) -> Result<String>;
    pub async fn remove_job(&mut self, job_id: &str) -> Result<()>;
    pub async fn list_jobs(&self) -> Vec<CronJob>;
    pub async fn enable_job(&mut self, job_id: &str) -> Result<()>;
    pub async fn disable_job(&mut self, job_id: &str) -> Result<()>;
}

// RPC Handlers
// cron.list, cron.create, cron.delete, cron.enable, cron.disable

// Agent Tool
pub struct CronTool {
    service: Arc<RwLock<CronService>>,
}
```

**Features**:
- Cron expression parsing
- Job persistence (SQLite)
- Agent invocation on schedule
- Run history logging
- Enable/disable jobs
- CLI: `aether cron list/create/delete`

**Crates**:
```toml
cron = "0.13"
tokio-cron-scheduler = "0.13"
```

**Effort**: ~600 LoC, 1 week

---

### 8.5 Model Failover

**Moltbot Reference**: `/src/agents/model-fallback.ts`

**Rust Implementation**:

```rust
// core/src/providers/failover.rs

pub struct FailoverConfig {
    pub models: Vec<ModelConfig>,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
    pub health_check_interval_secs: u64,
}

pub struct ModelConfig {
    pub provider: String,        // anthropic, openai, gemini
    pub model: String,
    pub api_key_env: String,
    pub priority: u32,           // Lower = higher priority
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

pub struct FailoverProvider {
    configs: Vec<ModelConfig>,
    health_status: Arc<RwLock<HashMap<String, bool>>>,
}

impl FailoverProvider {
    pub async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        for config in self.configs.iter().filter(|c| self.is_healthy(c)) {
            match self.try_provider(config, &request).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    tracing::warn!("Provider {} failed: {}", config.provider, e);
                    self.mark_unhealthy(config);
                    continue;
                }
            }
        }
        Err(anyhow!("All providers failed"))
    }

    async fn health_check_loop(&self) {
        // Periodically check provider health
    }
}
```

**Features**:
- Priority-based model selection
- Automatic failover on error
- Health check monitoring
- Per-model configuration
- Retry with exponential backoff

**Effort**: ~400 LoC, 1 week

---

## Implementation Order (Recommended)

| Phase | Week | Tasks | Deliverables |
|-------|------|-------|--------------|
| **8.1** | 1-2 | Telegram Channel | Bot integration, message handling |
| **8.2** | 3-4 | Model Failover | Multi-provider resilience |
| **8.3** | 5-6 | Discord Channel | Bot integration, slash commands |
| **8.4** | 7-8 | Cron Jobs | Scheduled automation |
| **8.5** | 9-10 | Browser Control | CDP integration, basic tools |

---

## Success Criteria

### Phase 8.1: Telegram Channel
- [ ] Bot responds to messages
- [ ] User allowlist works
- [ ] File/image upload works
- [ ] Inline keyboard works
- [ ] Long-polling stable

### Phase 8.2: Model Failover
- [ ] Automatic failover on API error
- [ ] Health monitoring active
- [ ] Config supports multiple providers
- [ ] Graceful degradation

### Phase 8.3: Discord Channel
- [ ] Bot responds in guilds
- [ ] DM support works
- [ ] Slash commands registered
- [ ] Embeds render correctly

### Phase 8.4: Cron Jobs
- [ ] Jobs persist across restart
- [ ] Cron expressions parse correctly
- [ ] Agent invoked on schedule
- [ ] CLI commands work

### Phase 8.5: Browser Control
- [ ] Chrome launches successfully
- [ ] Navigation works
- [ ] Click/type works
- [ ] Screenshots work

---

## Technical Dependencies

### New Crates

```toml
[dependencies]
# Telegram
teloxide = { version = "0.13", features = ["macros"], optional = true }

# Discord
serenity = { version = "0.12", features = ["client", "gateway", "cache"], optional = true }

# Browser
chromiumoxide = { version = "0.7", features = ["tokio-runtime"], optional = true }

# Cron
cron = { version = "0.13", optional = true }
tokio-cron-scheduler = { version = "0.13", optional = true }

[features]
telegram = ["teloxide"]
discord = ["serenity"]
browser = ["chromiumoxide"]
cron = ["cron", "tokio-cron-scheduler"]
all-channels = ["telegram", "discord"]
```

---

## Moltbot Code Reference

| Feature | Moltbot Location | Key Files |
|---------|-----------------|-----------|
| Telegram | `src/telegram/` | `channel.ts`, `inbound.ts`, `outbound.ts` |
| Discord | `src/discord/` | `channel.ts`, `client.ts`, `commands.ts` |
| Browser | `src/browser/` | `chrome.ts`, `cdp.ts`, `pw-session.ts` |
| Cron | `src/cron/` | `service.ts`, `schedule.ts`, `run-log.ts` |
| Failover | `src/agents/` | `model-fallback.ts`, `model-selection.ts` |

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Telegram API changes | Low | Medium | Pin teloxide version |
| Discord rate limits | Medium | Low | Implement rate limiting |
| Chrome version compat | Medium | Medium | Auto-detect Chrome path |
| Cron time drift | Low | Low | Use system time sync |
| Provider API outages | Medium | High | Failover + health checks |

---

## Decision Required

**推荐 Option C: Hybrid Approach**

理由：
1. **Telegram** 用户覆盖广，实现相对简单
2. **Model Failover** 提升系统可靠性，是生产必需
3. **Discord** 开发者社区活跃
4. **Cron Jobs** 实现自动化能力
5. **Browser Control** 增强工具能力

请确认：
- [ ] 接受推荐方案 (Hybrid)
- [ ] 选择 Option A (纯渠道扩展)
- [ ] 选择 Option B (工具优先)
- [ ] 自定义优先级

---

## Appendix: Moltbot Architecture Highlights

### Gateway Protocol (参考)

```typescript
// Moltbot: src/gateway/protocol/index.ts
interface GatewayMessage {
  id: string;
  type: 'request' | 'response' | 'event' | 'stream';
  method?: string;
  params?: any;
  result?: any;
  error?: GatewayError;
}
```

### Channel Abstraction (参考)

```typescript
// Moltbot: src/channels/registry.ts
interface ChannelPlugin {
  id: string;
  start(): Promise<void>;
  stop(): Promise<void>;
  send(message: OutboundMessage): Promise<void>;
  onMessage(handler: (msg: InboundMessage) => void): void;
}
```

### Agent Tools (参考)

```typescript
// Moltbot: src/agents/tools/browser-tool.ts
const browserTool = {
  name: 'browser',
  description: 'Control a web browser',
  parameters: {
    action: { type: 'string', enum: ['navigate', 'click', 'type', 'screenshot'] },
    url: { type: 'string' },
    selector: { type: 'string' },
    text: { type: 'string' },
  },
  execute: async (params) => { /* ... */ }
};
```
