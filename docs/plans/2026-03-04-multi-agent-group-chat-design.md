# Multi-Agent Group Chat Design

> 多 Agent 群聊角色扮演 — 底层能力 + 通道适配架构

**Date**: 2026-03-04
**Status**: Approved

---

## 概述

通过 Telegram/Discord/CLI 等任意通道，用户发起"群聊模式"，多个 AI 角色（Persona）以不同身份、不同模型轮流发言，形成真实的多视角讨论。

**核心原则**：通道无关的底层编排 + 通道特定的渲染适配，符合 R7「一核多端」。

## 架构分层

```
┌─────────────────────────────────────────────────┐
│  Channel Layer (纯 I/O 适配)                      │
│  Telegram / Discord / CLI / Web                  │
│  职责: 解析指令 → GroupChatRequest                  │
│        GroupChatMessage → 渠道格式化发送             │
└──────────────────────┬──────────────────────────┘
                       │ GroupChatRequest / GroupChatMessage
┌──────────────────────▼──────────────────────────┐
│  Core: GroupChat Orchestrator (核心编排层)          │
│  - Coordinator Agent (分析 → 选角色 → 定顺序)       │
│  - PersonaRegistry (预设 + 临时角色库)              │
│  - 累积上下文管理                                   │
│  - 多轮会话状态                                     │
│  - spawn 子 Agent + 收集结果                        │
└──────────────────────┬──────────────────────────┘
                       │ spawn / result
┌──────────────────────▼──────────────────────────┐
│  已有基础设施                                      │
│  SubAgentHandler / SessionsSpawnTool / Thinker   │
│  Provider 路由 (Claude/GPT/DeepSeek)              │
└─────────────────────────────────────────────────┘
```

---

## 1. Core 层数据模型

### 1.1 Persona（角色定义）

```rust
/// 一个可参与群聊的角色身份
pub struct Persona {
    pub id: String,              // "architect"
    pub name: String,            // "架构师"
    pub system_prompt: String,   // 角色人设
    pub provider: Option<String>,// 可选覆盖，缺省继承默认
    pub model: Option<String>,   // 可选覆盖
    pub thinking_level: Option<ThinkingLevel>,
}

/// 角色来源
pub enum PersonaSource {
    Preset(String),              // 从 aleph.toml 预设库中按 id 引用
    Inline(Persona),             // 用户临时定义
}
```

### 1.2 GroupChatRequest（通道无关的输入协议）

```rust
/// Channel 层解析指令后统一提交给 Core 的请求
pub enum GroupChatRequest {
    /// 开启新群聊
    Start {
        personas: Vec<PersonaSource>,
        topic: Option<String>,
        initial_message: String,
    },
    /// 在已有群聊中追问
    Continue {
        session_id: String,
        message: String,
    },
    /// 在群聊中 @指定角色
    Mention {
        session_id: String,
        message: String,
        targets: Vec<String>,          // 被 @ 的角色 id
    },
    /// 结束群聊
    End {
        session_id: String,
    },
}
```

### 1.3 GroupChatMessage（通道无关的输出协议）

```rust
/// Core 编排后输出给 Channel 渲染的消息
pub struct GroupChatMessage {
    pub session_id: String,
    pub speaker: Speaker,
    pub content: String,
    pub round: u32,                    // 第几轮讨论
    pub sequence: u32,                 // 本轮第几个发言
    pub is_final: bool,                // 是否本轮最后一条
}

pub enum Speaker {
    Coordinator,
    Persona { id: String, name: String },
    System,
}
```

### 1.4 配置（aleph.toml）

```toml
# 预设角色库
[[personas]]
id = "architect"
name = "架构师"
system_prompt = "你是一位资深软件架构师，擅长系统设计..."
provider = "claude"
model = "claude-sonnet-4-20250514"
thinking_level = "deep"

[[personas]]
id = "pm"
name = "产品经理"
system_prompt = "你是一位产品经理，关注用户价值和商业可行性..."

[[personas]]
id = "security"
name = "安全专家"
system_prompt = "你是一位安全审计专家..."
provider = "deepseek"
model = "deepseek-chat"

# 群聊全局设置
[group_chat]
max_personas_per_session = 6
max_rounds = 10
coordinator_visible = false
default_coordinator_model = "claude-sonnet-4-20250514"
```

---

## 2. Core 编排层 — GroupChat Orchestrator

### 2.1 Orchestrator

```rust
pub struct GroupChatOrchestrator {
    persona_registry: PersonaRegistry,
    sessions: HashMap<String, GroupChatSession>,
    coordinator_config: CoordinatorConfig,
}
```

### 2.2 GroupChatSession

```rust
pub struct GroupChatSession {
    pub id: String,
    pub topic: Option<String>,
    pub participants: Vec<Persona>,
    pub history: Vec<GroupChatTurn>,
    pub current_round: u32,
    pub status: GroupChatStatus,
    pub created_at: i64,
    pub source_channel: String,
    pub source_session_key: String,
}

pub struct GroupChatTurn {
    pub round: u32,
    pub speaker: Speaker,
    pub content: String,
    pub timestamp: i64,
}

pub enum GroupChatStatus {
    Active,
    Paused,
    Ended,
}
```

### 2.3 编排流程

```
用户消息到达
    │
    ▼
┌─────────────────────────────────┐
│ Step 1: Coordinator 分析         │
│                                  │
│ 输入: 用户消息 + 讨论历史 + 角色列表 │
│ 输出: CoordinatorPlan            │
│   - respondents: [角色id, 顺序]   │
│   - guidance: 每个角色的关注点提示   │
│   - need_summary: bool           │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│ Step 2: 串行 spawn 各角色        │
│                                  │
│ for persona in plan.respondents: │
│   context = 用户消息              │
│           + 讨论历史              │
│           + 前面角色本轮发言       │  ← 累积上下文
│           + coordinator guidance │
│                                  │
│   result = spawn_agent(...)      │
│   emit GroupChatMessage          │  ← 实时流式推送
│   append to history              │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│ Step 3: 可选总结                 │
│                                  │
│ if plan.need_summary:            │
│   Coordinator 生成本轮总结        │
│   emit GroupChatMessage(final)   │
└─────────────────────────────────┘
```

### 2.4 Coordinator Plan

```rust
pub struct CoordinatorPlan {
    pub respondents: Vec<RespondentPlan>,
    pub need_summary: bool,
}

pub struct RespondentPlan {
    pub persona_id: String,
    pub order: u32,
    pub guidance: String,
}
```

### 2.5 PersonaRegistry

```rust
pub struct PersonaRegistry {
    presets: HashMap<String, Persona>,
}

impl PersonaRegistry {
    pub fn load_from_config(config: &AlephConfig) -> Self;
    pub fn resolve(&self, sources: &[PersonaSource]) -> Result<Vec<Persona>>;
    pub fn reload(&mut self, config: &AlephConfig);
}
```

### 2.6 流式输出

每个角色发言完成后立即推送，不等所有角色说完：

```rust
impl GroupChatOrchestrator {
    pub async fn handle_request(
        &mut self,
        request: GroupChatRequest,
        tx: mpsc::Sender<GroupChatMessage>,
    ) -> Result<()>;
}
```

---

## 3. Channel 适配层

### 3.1 GroupChatRenderer Trait

```rust
pub trait GroupChatRenderer {
    fn render_message(&self, msg: &GroupChatMessage) -> RenderedContent;
    fn render_session_start(&self, participants: &[Persona], topic: Option<&str>) -> RenderedContent;
    fn render_session_end(&self, session: &GroupChatSession) -> RenderedContent;
    fn render_typing(&self, persona: &Persona) -> Option<RenderedContent>;
}

pub struct RenderedContent {
    pub text: String,
    pub format: ContentFormat,       // Markdown / HTML / Plain
    pub metadata: Option<serde_json::Value>,
}
```

### 3.2 各通道渲染

| 通道 | 消息格式 | Typing |
|------|----------|--------|
| **Telegram** | `**[架构师]**: 内容...` (Markdown) | 原生 sendChatAction + 文字提示 |
| **Discord** | Embed (author=角色名, color=角色色) | 原生 typing indicator |
| **CLI** | ANSI 颜色前缀 `\x1b[36m[架构师]\x1b[0m` | 无 |

### 3.3 GroupChatCommandParser Trait

```rust
pub trait GroupChatCommandParser {
    fn parse_group_chat_command(&self, raw_message: &str) -> Option<GroupChatRequest>;
}
```

各通道指令语法：

- **Telegram**: `/groupchat start --preset architect,pm --role "安全专家: ..." --topic "..."`
- **Discord**: `/groupchat start presets:architect,pm topic:...`
- **CLI**: `:groupchat start architect pm --topic "..."`

都解析为同一个 `GroupChatRequest::Start { ... }`。

---

## 4. 系统集成

### 4.1 Gateway RPC

```
"group_chat.start"    → GroupChatStartHandler
"group_chat.continue" → GroupChatContinueHandler
"group_chat.mention"  → GroupChatMentionHandler
"group_chat.end"      → GroupChatEndHandler
"group_chat.list"     → GroupChatListHandler
"group_chat.history"  → GroupChatHistoryHandler
```

### 4.2 SubAgent 复用

角色调用复用已有的 `SubAgentHandler` 底层逻辑，构造 `RunContext` 覆盖 provider/model/system_prompt。角色默认不带工具（纯对话），Persona 配置可扩展 `tools` 字段。

### 4.3 Session 存储（SQLite）

```sql
CREATE TABLE group_chat_sessions (
    id TEXT PRIMARY KEY,
    topic TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    source_channel TEXT NOT NULL,
    source_session_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE group_chat_turns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES group_chat_sessions(id),
    round INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    speaker_type TEXT NOT NULL,
    speaker_id TEXT,
    speaker_name TEXT NOT NULL,
    content TEXT NOT NULL,
    timestamp INTEGER NOT NULL
);
```

### 4.4 Config 集成

```rust
pub struct GroupChatConfig {
    pub max_personas_per_session: usize,
    pub max_rounds: usize,
    pub coordinator_visible: bool,
    pub default_coordinator_model: Option<String>,
}
```

挂载到 `AlephConfig`，通过已有 config watcher 热重载。

### 4.5 模块放置

```
core/src/
├── group_chat/
│   ├── mod.rs
│   ├── orchestrator.rs
│   ├── session.rs
│   ├── coordinator.rs
│   ├── persona.rs
│   └── protocol.rs
├── gateway/handlers/
│   └── group_chat.rs
├── config/types/
│   └── group_chat.rs
```

---

## 5. 边界情况与约束

### 5.1 错误类型

```rust
pub enum GroupChatError {
    PersonaNotFound(String),
    TooManyPersonas { max: usize, requested: usize },
    MaxRoundsReached { session_id: String, max: usize },
    SessionNotFound(String),
    CoordinatorPlanParseError(String),
    PersonaInvocationFailed { persona_id: String, source: anyhow::Error },
    ProviderUnavailable { provider: String, source: anyhow::Error },
}
```

### 5.2 降级策略

| 场景 | 处理 |
|------|------|
| Coordinator plan 解析失败 | fallback：全员发言，按配置顺序 |
| 某角色 LLM 调用失败 | 跳过该角色，标注 `⚠️ 暂时无法回应`，其余继续 |
| 某角色 provider 不可用 | 尝试 fallback model，否则跳过 |
| Coordinator 自身调用失败 | fallback：全员发言，按配置顺序 |
| 用户连续追问过快 | 队列化，当前轮完成后再处理 |

### 5.3 安全约束

- 角色数上限：`max_personas_per_session`（默认 6）
- 轮数上限：`max_rounds`（默认 10）
- 上下文截断：超过 token 阈值时 Coordinator 压缩早期轮次
- system_prompt 长度限制：不超过 2000 字符
- 临时角色 system_prompt 基础过滤

### 5.4 性能

```
单轮群聊延迟（3 角色）:
  Coordinator 分析:  ~2s
  角色 1 发言:       ~3s  (完成后立即推送)
  角色 2 发言:       ~3s
  角色 3 发言:       ~3s
  总计:             ~11s  (用户从第 2s 开始看到第一条回复)
```

### 5.5 v1 范围

**做：**
- Core 编排层完整实现
- 协议层完整定义
- aleph.toml 配置支持
- Telegram 渲染适配（第一个 Channel）
- SQLite 会话持久化
- 多轮讨论
- 降级处理

**不做（后续版本）：**
- Discord / CLI 渲染适配（接口已预留）
- 角色带工具能力
- 并行角色调用
- 角色间自发讨论
- 群聊会话导出 / 分享
