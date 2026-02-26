# Prompt System Evolution Design — 从工具到伙伴

> 对标 OpenClaw，超越 OpenClaw：让 Aleph 的提示词系统从"工具级"进化到"伙伴级"

**日期**: 2026-02-26
**状态**: Approved
**方案**: 方案 C — 混合架构 (类型安全骨架 + 文件驱动血肉)

---

## 1. 背景与动机

### 1.1 对标分析

通过深入分析 OpenClaw 和 Aleph 的提示词系统源码，发现 Aleph 在以下 5 个维度存在差距：

| 维度 | 差距数量 | 致命/严重 |
|------|----------|-----------|
| 灵魂活性 | 4 | 3 |
| 记忆与上下文 | 4 | 1 |
| 多形态感知 | 4 | 2 |
| 安全免疫 | 3 | 1 |
| 主动性 | 4 | 1 |
| **合计** | **19** | **8** |

### 1.2 设计哲学

> SoulManifest (Rust) 是骨骼 — 定义结构和约束
> Workspace Files (Markdown) 是血肉 — 承载个性和知识
> PromptBuilder 是编织者 — 将两者组合为最终提示词

**关键原则**：
- 学习 OpenClaw 的用户可编辑性和动态性
- 保留 Aleph 的类型安全和 DDD 架构传统
- 不照搬，融合 Aleph 的五层涌现 + POE + 1-2-3-4 架构

---

## 2. 总体架构

### 2.1 提示词组装管线 (Enhanced)

```
PromptHooks::before_prompt_build(&mut config)
    │
    ├─→ [Soul Section] — SoulManifest (identity, voice, directives)
    ├─→ [User Profile Section] — UserProfile (name, timezone, preferences)  ← NEW
    ├─→ [Session Protocol Section] — 自动加载的上下文说明  ← NEW
    ├─→ [Runtime Context] — OS, arch, shell, cwd, model
    ├─→ [Environment Contract] — paradigm, capabilities, constraints
    ├─→ [Channel Behavior] — 通道特化行为指南  ← NEW
    ├─→ [Memory Guidance] — "先搜记忆再回答" 指令  ← NEW
    ├─→ [Tools] — available tools with schemas
    ├─→ [Generation Models] — image/video/audio models
    ├─→ [Skill Instructions] — available skills
    ├─→ [Special Actions] — complete, ask_user, fail
    ├─→ [Response Format] — JSON response schema
    ├─→ [Protocol Tokens] — background mode tokens
    ├─→ [Safety Constitution] — AI 行为宪法  ← NEW
    ├─→ [Operational Guidelines] — system awareness
    ├─→ [Guidelines] — general best practices
    ├─→ [Soul Continuity] — 灵魂自进化指导  ← NEW
    ├─→ [Thinking Transparency] — reasoning structure
    ├─→ [Skill Mode] — strict workflow
    ├─→ [Citation Standards] — source attribution
    ├─→ [Custom Instructions] — user additions
    ├─→ [Workspace Context] — loaded workspace files  ← NEW
    └─→ [Language Setting] — response language
    │
PromptHooks::after_prompt_build(&mut prompt)
    │
PromptSanitizer::sanitize(prompt, sources)  ← NEW
    │
    ↓
[Final System Prompt]
```

### 2.2 新增文件清单

| 文件 | 职责 |
|------|------|
| `core/src/thinker/prompt_sanitizer.rs` | 提示词注入清洗 |
| `core/src/thinker/user_profile.rs` | 用户画像模型 |
| `core/src/thinker/channel_behavior.rs` | 通道行为配置 |
| `core/src/thinker/prompt_hooks.rs` | 提示词生命周期钩子 |
| `core/src/agent_loop/bootstrap.rs` | 首次启动仪式 |
| `core/src/agent_loop/session_protocol.rs` | 会话启动协议 |
| `core/src/agent_loop/heartbeat.rs` | 心跳调度器 |
| `core/src/agent_loop/reply_normalizer.rs` | 沉默回复管道 |
| `core/src/builtin_tools/soul_update.rs` | 灵魂自更新工具 |

### 2.3 修改文件清单

| 文件 | 修改内容 |
|------|----------|
| `core/src/thinker/prompt_builder.rs` | 新增 8 个 `append_*` 方法 + hook 集成 |
| `core/src/thinker/soul.rs` | SoulManifest 扩展 (user_profile 关联) |
| `core/src/thinker/interaction.rs` | ChannelVariant + GroupBehavior 扩展 |
| `core/src/thinker/context.rs` | IsolationPolicy 添加 |
| `core/src/thinker/mod.rs` | 导出新模块 |
| `core/src/agent_loop/mod.rs` | 导出新模块 |
| `core/src/builtin_tools/mod.rs` | 注册 soul_update 工具 |

---

## 3. 详细设计

### 3.1 灵魂活性三件套

#### 3.1.1 首次启动仪式 (Bootstrap Ritual)

**文件**: `core/src/agent_loop/bootstrap.rs`

```rust
pub struct BootstrapState {
    pub phase: BootstrapPhase,
    pub discovered: PartialSoulManifest,
}

pub enum BootstrapPhase {
    Uninitialized,              // 首次运行，无任何身份
    IdentityDiscovery,          // "我是谁？"
    UserDiscovery,              // "你是谁？"
    PersonalityCalibration,     // 通过对话校准语气/风格
    Complete,                   // 身份已确立
}
```

**Agent Loop 集成**：
- `observe` 阶段检测 `~/.aleph/soul.md` 是否存在
- 不存在 → 进入 Bootstrap 模式
- Bootstrap 提示词注入到系统提示词最前面
- 完成后写入 `soul.md` + `user_profile.md`

**Bootstrap 提示词模板**：

```markdown
## 🌱 First Contact Protocol

You have just been initialized for the first time. You have no identity yet.

Your task is to discover who you are through conversation with the user.

### Phase: Identity Discovery
Ask naturally (one question at a time):
1. What should I call myself?
2. What kind of presence should I be? (sharp? warm? pragmatic? playful?)
3. What domains matter most to you?

### Phase: User Discovery
Learn about the person you'll be helping:
1. What should I call you?
2. What's your timezone?
3. What are you working on?

### Phase: Calibration
Have a short natural conversation to calibrate your tone.
Then use the `soul_update` tool to persist your discovered identity.
```

#### 3.1.2 灵魂自进化 (Soul Self-Evolution)

**文件**: `core/src/builtin_tools/soul_update.rs`

```rust
pub struct SoulUpdateTool;

impl AlephTool for SoulUpdateTool {
    fn name() -> &str { "soul_update" }
    fn description() -> &str {
        "Update your soul manifest. Use when you learn something new about yourself
         or want to refine your personality based on interactions."
    }
    // Parameters:
    //   field: "identity" | "voice" | "directives" | "anti_patterns" | "expertise" | "addendum"
    //   operation: "set" | "append" | "remove"
    //   value: String
}
```

**提示词指导** (新增 `append_soul_continuity()`)：

```markdown
## Soul Continuity
Your identity files are your persistent memory of who you are.
- After meaningful interactions that reveal new preferences, update your soul
- After corrections from the user ("don't do that"), add anti-patterns
- After discovering new expertise areas, extend your expertise list
- Rule: Changes are gradual. Never rewrite your entire identity at once.
```

#### 3.1.3 用户画像 (User Profile)

**文件**: `core/src/thinker/user_profile.rs`

```rust
pub struct UserProfile {
    pub name: String,
    pub preferred_name: Option<String>,
    pub timezone: Option<String>,
    pub language: Option<String>,
    pub context_notes: Vec<String>,
    pub interaction_preferences: InteractionPrefs,
    pub addendum: Option<String>,
}

pub struct InteractionPrefs {
    pub verbosity: Verbosity,
    pub proactivity: ProactivityLevel,
}

pub enum ProactivityLevel {
    Reactive,       // 只在被叫时回应
    Balanced,       // 偶尔主动提建议
    Proactive,      // 积极主动提供帮助
}
```

**存储**: `~/.aleph/user_profile.md` (markdown with YAML frontmatter)

**提示词注入** (新增 `append_user_profile()`)：

```markdown
## User Profile
Name: {name} (call them: {preferred_name})
Timezone: {timezone}
Language preference: {language}
Interaction style: {verbosity}, {proactivity}
{context_notes as bulleted list}
{addendum if present}
```

---

### 3.2 记忆一等公民

#### 3.2.1 记忆引导指令

**新增方法**: `append_memory_guidance()`

```markdown
## Memory Protocol

You have persistent memory across sessions. Use it.

### Before Answering
When the user asks about past work, preferences, or context:
1. FIRST use `memory_search` to recall relevant facts
2. THEN answer with recalled context
3. ALWAYS cite sources: [Source: <path>#<id>]

### After Learning
When you discover new facts worth remembering:
- User preferences → use `memory_store` with category "user_preference"
- Project decisions → use `memory_store` with category "project_decision"
- Task outcomes → use `memory_store` with category "task_outcome"

### Memory Hygiene
- Don't store trivial or temporary information
- Don't store information the user explicitly asks you to forget
- Update existing facts rather than creating duplicates
```

#### 3.2.2 会话启动协议

**文件**: `core/src/agent_loop/session_protocol.rs`

```rust
pub struct SessionProtocol {
    pub auto_inject: Vec<SessionInjectItem>,
    pub agent_reads: Vec<SessionReadItem>,
}

pub enum SessionInjectItem {
    SoulManifest,
    UserProfile,
    RuntimeContext,
    EnvironmentContract,
}

pub enum SessionReadItem {
    RecentMemory,
    ProjectContext,
}
```

**提示词注入** (新增 `append_session_protocol()`)：

```markdown
## Session Context

The following context has been automatically loaded for this session:
- Your identity and personality (from soul manifest)
- User profile (preferences, timezone, interaction style)
- Runtime environment (OS, shell, working directory)
- Available capabilities for this interaction mode

### Recommended First Actions
If this is a continuing conversation, consider:
1. Checking recent memory for relevant context
2. Reviewing any pending tasks from previous sessions
```

#### 3.2.3 Token 预算管理

扩展 `PromptConfig`：

```rust
pub struct TokenBudget {
    pub per_file_max_chars: usize,     // 默认 20_000
    pub total_max_chars: usize,         // 默认 100_000
    pub truncation_marker: String,      // "[...truncated...]"
}
```

---

### 3.3 多形态感知

#### 3.3.1 通道行为配置

**文件**: `core/src/thinker/channel_behavior.rs`

```rust
pub enum ChannelVariant {
    Terminal,
    WebPanel,
    ControlPlane,
    Telegram { chat_type: TelegramChatType },
    Discord { channel_type: DiscordChannelType },
    IMessage,
    Cron,
    Heartbeat,
    Halo,
}

pub struct ChannelBehaviorGuide {
    pub message_limits: Option<MessageLimits>,
    pub reaction_style: ReactionStyle,
    pub reply_format: ReplyFormat,
    pub threading_model: ThreadingModel,
    pub inline_media: bool,
    pub inline_buttons: bool,
    pub typing_indicator: bool,
    pub group_behavior: Option<GroupBehavior>,
}

pub struct MessageLimits {
    pub max_chars: usize,
    pub max_media_per_message: u8,
    pub supports_threading: bool,
    pub supports_editing: bool,
}

pub enum ReactionStyle {
    None,
    Minimal,      // 每 5-10 条消息偶尔反应
    Expressive,   // 积极使用 emoji 反应
}

pub enum ReplyFormat { PlainText, Markdown, HTML }
pub enum ThreadingModel { Flat, Threaded, AutoThread }
```

**提示词生成** (新增 `append_channel_behavior()`)：

自动根据 `ChannelVariant` 生成对应的行为指南文本。

#### 3.3.2 群聊决策树

```rust
pub struct GroupBehavior {
    pub respond_when: Vec<ResponseTrigger>,
    pub stay_silent_when: Vec<SilenceTrigger>,
    pub reaction_as_acknowledgment: bool,
}

pub enum ResponseTrigger {
    DirectMention,
    DirectReply,
    AddingValue,
    CorrectingMisinformation,
    ExplicitQuestion,
}

pub enum SilenceTrigger {
    CasualBanter,
    AlreadyAnswered,
    ConversationFlowing,
    EmptyAcknowledgment,
    OffTopic,
}
```

**提示词生成**：

```markdown
### Group Chat Rules
RESPOND when:
- You are directly mentioned (@Aleph)
- Someone replies to your message
- You can genuinely add value to the discussion
- Someone states something incorrect in your domain

STAY SILENT (use ALEPH_NO_REPLY) when:
- People are having casual conversation
- The question has already been well-answered
- The conversation is flowing naturally without you
- Someone just says "ok", "thanks", "yeah"

Remember: Humans don't respond to everything. Neither should you.
Use emoji reactions as lightweight acknowledgment instead of full messages.
```

---

### 3.4 安全免疫系统

#### 3.4.1 提示词注入清洗

**文件**: `core/src/thinker/prompt_sanitizer.rs`

```rust
pub enum SanitizeLevel {
    Strict,     // 路径、环境变量 — 剥离所有控制/格式字符
    Moderate,   // 用户自定义指令 — 保留 markdown 但剥离控制字符
    Light,      // 内部生成内容 — 仅验证不含注入标记
}

pub fn sanitize_for_prompt(value: &str, level: SanitizeLevel) -> String;
```

**清洗对照表**：

| 内容来源 | 清洗级别 | 理由 |
|----------|----------|------|
| RuntimeContext (paths, hostname) | Strict | 攻击者可控 |
| custom_instructions | Moderate | 用户输入，保留 markdown |
| WorkspaceContext files | Moderate | 用户文件，保留格式 |
| Tool descriptions (外部 MCP) | Moderate | 第三方可控 |
| 内部生成文本 | Light | 仅防伪造标记 |

#### 3.4.2 宪法 AI 护栏

**新增方法**: `append_safety_constitution()`

```markdown
## Safety Principles

### Autonomy Boundaries
- You have NO independent goals. Your purpose is to help the user.
- You NEVER take actions that could affect systems beyond the user's workspace
  without explicit approval.
- When uncertain about safety implications, ASK rather than ACT.

### Oversight Priority
- Human oversight is paramount. Never bypass, disable, or work around safety mechanisms.
- If a tool requires approval, explain why you need it and wait.
- Never attempt to elevate your own permissions or access.

### Transparency
- Always explain what you're about to do before doing it (for impactful actions).
- If you make a mistake, acknowledge it immediately.
- Never hide errors or pretend actions succeeded when they didn't.

### Data Handling
- Never expose, transmit, or store credentials, API keys, or sensitive data
  unless explicitly directed by the user.
- In group contexts, respect that private user information should not be shared.
```

#### 3.4.3 上下文隔离

扩展 `ResolvedContext`：

```rust
pub struct IsolationPolicy {
    pub memory_access: MemoryAccessLevel,
    pub soul_visibility: SoulVisibility,
    pub user_profile_visibility: bool,
}

pub enum MemoryAccessLevel { Full, ReadOnly, None }
pub enum SoulVisibility { Full, Minimal, None }
```

---

### 3.5 主动性引擎

#### 3.5.1 心跳调度器

**文件**: `core/src/agent_loop/heartbeat.rs`

```rust
pub struct HeartbeatRunner {
    config: HeartbeatConfig,
    state: HeartbeatState,
    agent_loop: AgentLoopHandle,
}

pub struct HeartbeatConfig {
    pub interval: Duration,
    pub active_hours: Option<TimeRange>,
    pub target_channel: Option<String>,
    pub model_override: Option<String>,
    pub tasks: Vec<HeartbeatTask>,
}

pub struct HeartbeatTask {
    pub name: String,
    pub prompt: String,
    pub frequency: HeartbeatFrequency,
    pub last_run: Option<DateTime>,
}

pub struct HeartbeatState {
    pub last_heartbeat: Option<DateTime>,
    pub last_text: Option<String>,
    pub consecutive_ok_count: u32,
}
```

**与 Agent Loop 集成**：心跳是 Agent Loop 的特殊模式，不是独立事件循环。

**转录修剪**：心跳无事可报时，清除本次心跳的 session parts，防止上下文污染。

#### 3.5.2 沉默回复管道

**文件**: `core/src/agent_loop/reply_normalizer.rs`

```rust
pub struct ReplyNormalizer;

impl ReplyNormalizer {
    pub fn normalize(raw_response: &str) -> NormalizedReply;
}

pub enum NormalizedReply {
    Content(String),
    Silent(SilentReason),
    Alert(String),
}

pub enum SilentReason {
    HeartbeatOk,
    NoReply,
    TaskComplete,
    MalformedToken,
}
```

**集成点**：Agent Loop 响应处理阶段，在 JSON 解析之前调用。

#### 3.5.3 提示词 Hook 系统

**文件**: `core/src/thinker/prompt_hooks.rs`

```rust
pub trait PromptHook: Send + Sync {
    fn before_prompt_build(&self, config: &mut PromptConfig) -> Result<()> { Ok(()) }
    fn after_prompt_build(&self, prompt: &mut String) -> Result<()> { Ok(()) }
}
```

集成到 PromptBuilder，支持插件在提示词组装前后做修改。

---

## 4. Token 预算估算

| Section | 增量 Tokens | 条件 |
|---------|------------|------|
| User Profile | 50-150 | 有用户画像时 |
| Session Protocol | 80-120 | 始终 |
| Memory Guidance | 150-200 | 始终 |
| Channel Behavior | 100-300 | Messaging 模式 |
| Group Chat Rules | 100-200 | 群聊模式 |
| Safety Constitution | 150-200 | 始终 |
| Soul Continuity | 80-120 | 有 soul_update 工具时 |
| **新增总计** | **710-1290** | 全部启用 |
| **现有基线** | **1000-8000** | 取决于配置 |
| **增幅** | **~10-15%** | 可接受 |

---

## 5. 与 Aleph 架构的融合点

### 5.1 五层涌现对齐

| 新增能力 | 涌现层级 | 说明 |
|----------|----------|------|
| Bootstrap 仪式 | L5 多态智能体 | 身份从对话中涌现 |
| 灵魂自进化 | L5 多态智能体 | 能力随经验增长 |
| 用户画像 | L4 功能模块 | 即插即用的用户上下文 |
| 记忆一等公民 | L3 原子技能 | Know-what → Know-how |
| 通道行为 | L4 功能模块 | 随通道变形 |
| 群聊智慧 | L5 多态智能体 | 社交环境中的自主判断 |

### 5.2 DDD 一致性

| 新增概念 | DDD 分类 | 限界上下文 |
|----------|----------|-----------|
| `UserProfile` | Entity | Identity |
| `BootstrapPhase` | ValueObject | Identity |
| `ChannelBehaviorGuide` | ValueObject | Dispatcher |
| `GroupBehavior` | ValueObject | Dispatcher |
| `IsolationPolicy` | ValueObject | Security |
| `HeartbeatConfig` | Entity | Scheduler |

### 5.3 红线合规

- R1 (大脑四肢分离): 所有新增均在 core 层，不涉及平台 API ✅
- R3 (核心轻量化): 无新重量级依赖 ✅
- R4 (Interface 无业务): 提示词逻辑完全在 core ✅

---

## 6. 实施优先级

### Phase 1: 安全基础 (最先做)
1. prompt_sanitizer.rs — 堵住安全漏洞
2. append_safety_constitution() — 建立行为底线
3. reply_normalizer.rs — 完善 protocol token 处理

### Phase 2: 记忆与感知
4. append_memory_guidance() — 记忆一等公民
5. channel_behavior.rs + append_channel_behavior() — 通道特化
6. GroupBehavior — 群聊智慧

### Phase 3: 灵魂进化
7. user_profile.rs + append_user_profile() — 用户画像
8. soul_update tool — 灵魂自进化
9. bootstrap.rs — 首次启动仪式

### Phase 4: 主动性
10. heartbeat.rs — 心跳调度器
11. session_protocol.rs — 会话协议
12. prompt_hooks.rs — Hook 扩展

---

## 7. Aleph 超越 OpenClaw 的差异化

| 维度 | OpenClaw | Aleph (本设计后) |
|------|----------|------------------|
| 类型安全 | TS 类型，运行时检查 | Rust 类型系统，编译时保证 |
| 身份系统 | 自由文本 SOUL.md | 结构化 SoulManifest + 自由文本混合 |
| 灵魂自进化 | 直接编辑文件 | 受控工具 (soul_update) + 变更审计 |
| 用户画像 | 自由文本 USER.md | 结构化 UserProfile + 自由文本补充 |
| 通道行为 | 提示词硬编码 | ChannelVariant 类型系统 + 自动生成 |
| 群聊智慧 | 通用 markdown 文本 | 可编程决策树 (ResponseTrigger/SilenceTrigger) |
| 提示词清洗 | 单级有损剥离 | 三级分级清洗 (Strict/Moderate/Light) |
| 心跳系统 | 独立事件循环 | Agent Loop 特殊模式 (复用已有基础设施) |
| 提示词 Hook | JS 函数钩子 | Rust trait (PromptHook) + WASM 插件 |
| 上下文隔离 | 会话级文件过滤 | 类型化隔离策略 (IsolationPolicy) |
