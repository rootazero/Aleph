# Agent Definition + Workspace 实体化 + Channel Binding 设计

> **日期**: 2026-03-04
> **状态**: Draft
> **范围**: 核心三件套 — Agent Definition, Workspace 实体化, Channel Binding
> **方法**: 演进融合 — 保留 Aleph 现有架构，学习 OpenClaw 的配置简洁性

---

## 1. 背景与动机

### 1.1 问题陈述

对比 OpenClaw 的 agent/workspace/multi-agent 设计，Aleph 存在三个核心差距：

| 差距 | OpenClaw | Aleph 现状 |
|------|----------|-----------|
| **Agent 定义** | TOML 配置条目即可创建 agent | `AgentInstanceConfig` 只能通过代码创建 |
| **Workspace** | 物理目录 + markdown 文件 = agent 的家 | 抽象概念（Profile + CacheState），用户不可见 |
| **Channel 路由** | 声明式 Binding（channel → agent） | 无明确 channel→agent 映射机制 |

### 1.2 设计目标

1. **配置驱动**：在 `aleph.toml` 中声明 agent，零代码创建新 agent
2. **Workspace 可见化**：每个 agent 拥有物理目录，markdown 文件定义人格、记忆、技能
3. **声明式路由**：通过 Binding 配置将 channel/peer 路由到特定 agent
4. **向后兼容**：无 `[agents]` 配置时行为与当前完全一致

### 1.3 设计原则

- **演进融合**：保留 Profile、SoulManifest、WorkspaceManager 等现有概念，增加 workspace 文件作为加载源
- **不照搬 OpenClaw**：Aleph 的 Rust trait 系统、TaskGraph DAG、Swarm 智能等优势保持不变
- **向后兼容**：现有用户不受影响，新功能通过新配置段 opt-in

---

## 2. 设计方案：配置聚合层

### 2.1 核心思路

在 `aleph.toml` 中新增 `[agents]` 和 `[[bindings]]` 段，由 `AgentDefinitionResolver` 将配置条目 + workspace 目录文件 + 现有 Profile/SoulManifest 统一解析为 `AgentInstance`。

---

## 3. Agent Definition System

### 3.1 配置结构

```toml
# aleph.toml

[agents]
# 全局默认值，所有 agent 继承未显式设置的字段
[agents.defaults]
model = "claude-opus-4-6"
workspace_root = "~/.aleph/workspaces"    # 非默认 agent 的 workspace 自动布局目录
skills = ["*"]                             # 默认允许所有 skills
dm_scope = "per_peer"                      # 默认 DM 隔离策略

# Agent 列表
[[agents.list]]
id = "main"
default = true
name = "Aleph"
workspace = "~/.aleph/workspace"           # 默认 agent 显式指定路径
profile = "general"                        # 绑定 [profiles.general]

[[agents.list]]
id = "coding"
name = "代码专家"
# workspace 省略 → 自动推导为 {workspace_root}/coding/
profile = "coding"                         # 绑定 [profiles.coding]
model = "claude-opus-4-6"                 # Override defaults.model
skills = ["git_*", "fs_*", "terminal", "code_review"]
subagents = { allow = [] }                 # 不允许生成子 agent

[[agents.list]]
id = "research"
name = "调研助手"
profile = "general"
model = "gemini-2.5-pro"
skills = ["search", "note_taking", "summarize"]
subagents = { allow = [] }

[[agents.list]]
id = "coordinator"
name = "协调者"
profile = "general"
model = "claude-opus-4-6"
skills = ["*"]
subagents = { allow = ["coding", "research"] }  # 可以委派给 coding 和 research
```

### 3.2 Rust 数据结构

```rust
// core/src/config/types/agents.rs (新文件)

use std::path::PathBuf;
use serde::Deserialize;

/// Agent 系统顶层配置
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentsConfig {
    #[serde(default)]
    pub defaults: AgentDefaults,
    #[serde(default)]
    pub list: Vec<AgentDefinition>,
}

/// 全局默认值，所有 agent 继承未显式设置的字段
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentDefaults {
    pub model: Option<String>,
    pub workspace_root: Option<PathBuf>,
    pub skills: Option<Vec<String>>,
    pub dm_scope: Option<DmScope>,
    pub bootstrap_max_chars: Option<usize>,       // 单个 workspace 文件最大字符数 (默认 20000)
    pub bootstrap_total_max_chars: Option<usize>,  // 所有 workspace 文件合计最大 (默认 150000)
}

/// 单个 Agent 的定义
#[derive(Debug, Clone, Deserialize)]
pub struct AgentDefinition {
    /// 唯一标识符 (URL-safe slug)
    pub id: String,
    /// 是否为默认 agent (收到无法路由的消息时使用)
    #[serde(default)]
    pub default: bool,
    /// 显示名称
    pub name: Option<String>,
    /// 显式 workspace 目录路径 (省略则自动推导)
    pub workspace: Option<PathBuf>,
    /// 绑定的 Profile 名 (对应 [profiles.xxx])
    pub profile: Option<String>,
    /// Override 默认 model
    pub model: Option<String>,
    /// Skill 白名单 (支持 glob: "git_*", "*" = all)
    pub skills: Option<Vec<String>>,
    /// 子 agent 授权策略
    pub subagents: Option<SubagentPolicy>,
}

/// 子 agent 授权策略
#[derive(Debug, Clone, Deserialize)]
pub struct SubagentPolicy {
    /// 允许委派到的 agent ID 列表 (["*"] = 任意)
    pub allow: Vec<String>,
}
```

### 3.3 AgentDefinitionResolver

```rust
// core/src/config/agent_resolver.rs (新文件)

/// 将 AgentsConfig + Profiles + Workspace 文件统一解析为 ResolvedAgent
pub struct AgentDefinitionResolver {
    workspace_loader: WorkspaceFileLoader,
}

/// 解析后的 Agent 完整定义
pub struct ResolvedAgent {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub workspace_path: PathBuf,
    pub profile: ProfileConfig,
    pub soul: Option<SoulManifest>,
    pub agents_md: Option<String>,
    pub memory_md: Option<String>,
    pub model: String,
    pub skills: Vec<String>,
    pub subagent_policy: SubagentPolicy,
}

impl AgentDefinitionResolver {
    /// 解析所有 agent 定义
    pub fn resolve_all(
        &mut self,
        agents_config: &AgentsConfig,
        profiles: &HashMap<String, ProfileConfig>,
    ) -> Vec<ResolvedAgent> {
        // 1. 如果 agents_config.list 为空，创建默认 main agent
        // 2. 对每个 AgentDefinition：
        //    a. 合并 defaults
        //    b. 解析 workspace 路径
        //    c. 加载 ProfileConfig (从 profile 字段查 profiles)
        //    d. 从 workspace 加载 SOUL.md → SoulManifest
        //    e. 从 workspace 加载 AGENTS.md, MEMORY.md
        //    f. 产出 ResolvedAgent
    }

    /// 解析 workspace 路径
    fn resolve_workspace_path(
        &self,
        agent: &AgentDefinition,
        defaults: &AgentDefaults,
    ) -> PathBuf {
        if let Some(ref ws) = agent.workspace {
            resolve_user_path(ws)
        } else {
            let root = defaults.workspace_root
                .as_ref()
                .map(|p| resolve_user_path(p))
                .unwrap_or_else(default_workspace_root);
            root.join(&agent.id)
        }
    }
}
```

### 3.4 向后兼容策略

无 `[agents]` 段时的行为：

```rust
impl AgentsConfig {
    /// 无配置时生成默认 agent
    pub fn ensure_default(&mut self) {
        if self.list.is_empty() {
            self.list.push(AgentDefinition {
                id: "main".into(),
                default: true,
                name: Some("Aleph".into()),
                workspace: None,  // 使用默认路径
                profile: None,
                model: None,
                skills: Some(vec!["*".into()]),
                subagents: None,
            });
        }
    }
}
```

---

## 4. Workspace 实体化

### 4.1 目录结构

每个 Agent 拥有一个物理 workspace 目录：

```
~/.aleph/workspace-coding/           # agent "coding" 的家
├── SOUL.md                          # 人格定义 → SoulManifest
├── AGENTS.md                        # Agent 行为指令 → system prompt 附加
├── TOOLS.md                         # 工具说明 (信息性，不控制权限)
├── IDENTITY.md                      # 名称、头像、自我介绍
├── MEMORY.md                        # 长期记忆摘要 (人类可编辑)
├── memory/                          # 每日记忆日志 (浅层记忆)
│   ├── 2026-03-03.md
│   └── 2026-03-04.md
└── skills/                          # Workspace 级自定义 Skills
    └── my_custom_skill.md
```

### 4.2 文件约定

| 文件 | 映射到 Aleph 概念 | 加载时机 | 必需 |
|------|-------------------|----------|------|
| `SOUL.md` | `SoulManifest` | 启动 + mtime 热重载 | 否 |
| `AGENTS.md` | 追加到 `system_prompt` | 每次会话开始 | 否 |
| `TOOLS.md` | 纯信息文件 (不影响权限) | 每次会话开始 | 否 |
| `IDENTITY.md` | Agent 显示名/头像 | 启动 | 否 |
| `MEMORY.md` | 注入上下文的长期摘要 | 每次会话开始 | 否 |
| `memory/*.md` | 每日浅层记忆日志 | 按需检索 | 否 |

所有文件都是可选的。缺失时使用 Profile 或全局默认值。

### 4.3 SoulManifest 解析优先级

扩展现有 `IdentityResolver` 的解析链：

```
Session 级 override
    ↓ (无则 fallback)
Workspace SOUL.md
    ↓ (无则 fallback)
Profile system_prompt
    ↓ (无则 fallback)
全局默认 SoulManifest
```

`SOUL.md` 格式兼容现有 `SoulManifest` 的 markdown 解析：

```markdown
---
relationship: mentor
voice:
  tone: professional
  verbosity: concise
expertise:
  - rust
  - systems-programming
---

## Identity
I am a Rust systems programming expert...

## Directives
- Always suggest idiomatic Rust patterns
- Prefer zero-cost abstractions

## Anti-Patterns
- Never suggest unsafe without justification
```

### 4.4 AGENTS.md 注入

`AGENTS.md` 的内容作为 system prompt 的附加部分注入：

```
final_system_prompt = [
    profile.system_prompt,          // Profile 定义的基础提示
    "\n\n",
    workspace.agents_md,            // AGENTS.md 内容
    "\n\n",
    workspace.memory_md,            // MEMORY.md 内容 (截断到 max_chars)
].join("")
```

### 4.5 每日记忆日志 (双轨制)

**浅层记忆 (新增)**：
- 路径: `{workspace}/memory/YYYY-MM-DD.md`
- 格式: 纯 markdown，人类可读可编辑
- 写入时机: 每次会话结束时，agent 自动追加当日摘要
- 读取时机: 会话开始时，加载最近 N 天的日志作为上下文
- 可通过 git 备份

**深层记忆 (现有不变)**：
- 存储: LanceDB 向量数据库
- 检索: 混合检索 (ANN + FTS)
- 隔离: `WorkspaceFilter` 按 agent 的 workspace_id 过滤
- 功能: 语义关联、知识图谱、自动压缩

**双轨关系**：
```
浅层记忆 (markdown) ─── 人类可读、可编辑、可 git 备份
         │
         ├─ 定期同步 (可选) ──→ 深层记忆 (LanceDB)
         │                       语义检索、知识图谱
         │
         └─ 独立价值 ─────── 即使无 LanceDB 也能工作
```

### 4.6 WorkspaceFileLoader

```rust
// core/src/gateway/workspace_loader.rs (新文件)

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::collections::HashMap;

/// mtime 缓存的 workspace 文件加载器
pub struct WorkspaceFileLoader {
    cache: HashMap<PathBuf, CachedFile>,
}

struct CachedFile {
    content: String,
    mtime: SystemTime,
}

pub struct DailyMemory {
    pub date: String,       // "2026-03-04"
    pub content: String,
}

impl WorkspaceFileLoader {
    pub fn new() -> Self;

    /// 加载 workspace 下的指定文件，mtime 缓存避免重复读取
    pub fn load(&mut self, workspace: &Path, filename: &str) -> Option<String>;

    /// 加载 SOUL.md 并解析为 SoulManifest
    pub fn load_soul(&mut self, workspace: &Path) -> Option<SoulManifest>;

    /// 加载 AGENTS.md 内容
    pub fn load_agents_md(&mut self, workspace: &Path) -> Option<String>;

    /// 加载 MEMORY.md 内容 (截断到 max_chars)
    pub fn load_memory_md(&mut self, workspace: &Path, max_chars: usize) -> Option<String>;

    /// 加载最近 N 天的 memory 日志
    pub fn load_recent_memory(&mut self, workspace: &Path, days: u32) -> Vec<DailyMemory>;

    /// 写入每日记忆日志
    pub fn append_daily_memory(
        &self,
        workspace: &Path,
        date: &str,
        content: &str,
    ) -> Result<(), std::io::Error>;
}
```

### 4.7 Workspace 初始化

新 agent 首次启动时，如果 workspace 目录不存在：

1. 创建目录结构
2. 生成默认 `SOUL.md`（基于 Profile 的 system_prompt 或全局默认）
3. 生成默认 `AGENTS.md`（基本指令模板）
4. 创建 `memory/` 子目录
5. 不生成 `MEMORY.md`（空白起步）

```rust
/// 初始化 workspace 目录
pub fn initialize_workspace(path: &Path, agent: &ResolvedAgent) -> Result<()> {
    fs::create_dir_all(path.join("memory"))?;

    if !path.join("SOUL.md").exists() {
        // 从 agent 的 SoulManifest 或 Profile 生成默认 SOUL.md
        let default_soul = generate_default_soul(agent);
        fs::write(path.join("SOUL.md"), default_soul)?;
    }

    if !path.join("AGENTS.md").exists() {
        let default_agents = format!(
            "# {} Operating Instructions\n\nCustomize this file to guide agent behavior.\n",
            agent.name
        );
        fs::write(path.join("AGENTS.md"), default_agents)?;
    }

    Ok(())
}
```

---

## 5. Channel Binding 路由

### 5.1 配置结构

```toml
# aleph.toml

[[bindings]]
agent_id = "coding"
comment = "公司 Slack 全部路由到代码专家"
[bindings.match]
channel = "slack"
team_id = "T12345"

[[bindings]]
agent_id = "research"
comment = "Telegram 调研群"
[bindings.match]
channel = "telegram"
peer = { kind = "group", id = "-100123456" }

[[bindings]]
agent_id = "main"
comment = "Fallback: 其他所有消息走默认 agent"
[bindings.match]
channel = "*"
```

### 5.2 Rust 数据结构

```rust
// core/src/routing/binding.rs (新文件)

use serde::Deserialize;

/// Channel → Agent 绑定规则
#[derive(Debug, Clone, Deserialize)]
pub struct AgentBinding {
    pub agent_id: String,
    pub comment: Option<String>,
    #[serde(rename = "match")]
    pub match_rule: BindingMatchRule,
}

/// 绑定匹配条件
#[derive(Debug, Clone, Deserialize)]
pub struct BindingMatchRule {
    /// 渠道名 ("telegram", "discord", "slack", "*" = 通配符)
    pub channel: Option<String>,
    /// 账号 ID (多账号场景)
    pub account_id: Option<String>,
    /// 精确 peer 匹配
    pub peer: Option<PeerMatch>,
    /// Discord guild ID
    pub guild_id: Option<String>,
    /// Slack team ID
    pub team_id: Option<String>,
}

/// Peer 匹配条件
#[derive(Debug, Clone, Deserialize)]
pub struct PeerMatch {
    pub kind: PeerKind,      // "user", "group", "channel"
    pub id: String,
}
```

### 5.3 匹配优先级

按特异性排序 (CSS specificity 模型)，从高到低：

| 优先级 | 匹配条件 | 示例 |
|--------|----------|------|
| **P1** | peer (精确用户/群组) | `peer = { kind = "group", id = "-100123" }` |
| **P2** | guild_id / team_id | `team_id = "T12345"` |
| **P3** | account_id | `account_id = "bot-123"` |
| **P4** | channel (渠道级别) | `channel = "telegram"` |
| **P5** | 通配符 | `channel = "*"` |

同一优先级内，配置顺序靠前的优先 (first-match-wins)。

### 5.4 BindingRouter

```rust
// core/src/routing/binding_router.rs (新文件)

/// 解析消息应该路由到哪个 agent
pub struct BindingRouter {
    /// 按特异性排序的绑定规则
    bindings: Vec<(Specificity, AgentBinding)>,
    /// 默认 agent ID (当无匹配时)
    default_agent_id: String,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
enum Specificity {
    Peer = 5,         // 最高优先级
    GuildOrTeam = 4,
    AccountId = 3,
    Channel = 2,
    Wildcard = 1,     // 最低优先级
}

/// 路由上下文 (从 channel handler 传入)
pub struct RoutingContext {
    pub channel: String,
    pub account_id: Option<String>,
    pub peer_kind: Option<PeerKind>,
    pub peer_id: Option<String>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
}

impl BindingRouter {
    /// 从配置构建，自动按特异性排序
    pub fn new(bindings: Vec<AgentBinding>, default_agent_id: String) -> Self;

    /// 解析消息应该路由到哪个 agent
    /// 返回匹配的 agent_id，无匹配返回 default_agent_id
    pub fn resolve(&self, ctx: &RoutingContext) -> &str;
}
```

### 5.5 与现有系统的集成

**路由发生在 SessionKey 构建之前**：

```
消息到达 Channel Handler
    ↓
BindingRouter::resolve(channel, account_id, peer)
    ↓ 返回 agent_id
SessionKey::new(agent_id, channel, peer_id, ...)
    ↓
AgentRegistry::get(agent_id) → AgentInstance
    ↓
AgentInstance.run(session_key, message)
```

现有 `SessionKey` 已有 `agent_id` 字段，集成自然。

### 5.6 与现有 `[[routing]]` 的关系

```
[[bindings]]  — Agent 级路由 (channel → agent)       ← 新增
[[routing]]   — 消息级路由 (regex → provider/model)   ← 保留
```

两者在不同层级工作：
1. `[[bindings]]` 先决定 "哪个 agent 处理这条消息"
2. `[[routing]]` 再在 agent 内部决定 "用哪个 provider/model 处理特定命令"

### 5.7 向后兼容

- 无 `[[bindings]]` 段时：所有消息路由到 default agent (`id = "main"`)
- 现有 channel handler：仅需在消息入口增加 `BindingRouter::resolve()` 调用

---

## 6. 整体架构图

```
┌─────────────────────────────────────────────────────────────┐
│                       aleph.toml                            │
├──────────────┬──────────────┬───────────────┬───────────────┤
│ [agents]     │ [profiles]   │ [[bindings]]  │ [channels]    │
│ defaults     │ coding       │ slack→coding  │ telegram      │
│ list:        │ general      │ tg→main       │ discord       │
│  - main      │ research     │ *→main        │ slack         │
│  - coding    │              │               │               │
│  - research  │              │               │               │
└──────┬───────┴──────┬───────┴───────┬───────┴───────────────┘
       │              │               │
       ▼              │               ▼
 AgentDefinition      │         BindingRouter
 Resolver             │          .resolve()
       │              │               │
       ▼              ▼               │
 ┌─────────────────────────┐         │
 │    AgentInstance         │◄────────┘
 │  ┌───────────────────┐  │
 │  │ ProfileConfig     │  │  ← 从 [profiles.*]
 │  │ SoulManifest      │  │  ← 从 workspace/SOUL.md
 │  │ WorkspaceLoader   │  │  ← 读 workspace 目录文件
 │  │ SkillFilter       │  │  ← 从 agents.list[].skills
 │  │ SubagentPolicy    │  │  ← 从 agents.list[].subagents
 │  └───────────────────┘  │
 │                         │
 │  workspace/             │
 │  ├── SOUL.md            │
 │  ├── AGENTS.md          │
 │  ├── MEMORY.md          │
 │  ├── memory/            │
 │  │   └── 2026-03-04.md  │
 │  └── skills/            │
 └─────────────────────────┘
```

---

## 7. 数据流

### 7.1 启动流程

```
1. 加载 aleph.toml
2. 解析 [agents] → AgentsConfig
3. 解析 [profiles] → HashMap<String, ProfileConfig>
4. 解析 [[bindings]] → Vec<AgentBinding>
5. AgentDefinitionResolver.resolve_all()
   ├── 对每个 AgentDefinition:
   │   ├── 合并 defaults
   │   ├── 解析 workspace 路径
   │   ├── 初始化 workspace 目录 (如不存在)
   │   ├── 加载 ProfileConfig
   │   ├── 加载 workspace/SOUL.md → SoulManifest
   │   ├── 加载 workspace/AGENTS.md
   │   └── 加载 workspace/MEMORY.md
   └── 产出 Vec<ResolvedAgent>
6. 注册到 AgentRegistry
7. 构建 BindingRouter
8. 启动 Channel Handlers (注入 BindingRouter)
```

### 7.2 消息处理流程

```
1. 消息到达 Channel Handler
2. 构建 RoutingContext { channel, peer_id, guild_id, ... }
3. BindingRouter.resolve(ctx) → agent_id
4. SessionKey::new(agent_id, channel, peer_id, ...)
5. AgentRegistry.get(agent_id) → AgentInstance
6. AgentInstance 加载 workspace 文件 (mtime 缓存)
7. 构建 system_prompt = profile + AGENTS.md + MEMORY.md
8. 运行 Agent Loop (OTAF)
9. 会话结束时追加 memory/YYYY-MM-DD.md
```

---

## 8. 新增文件清单

| 文件路径 | 职责 |
|----------|------|
| `core/src/config/types/agents.rs` | AgentsConfig, AgentDefinition, SubagentPolicy 数据结构 |
| `core/src/config/agent_resolver.rs` | AgentDefinitionResolver — 统一解析 |
| `core/src/gateway/workspace_loader.rs` | WorkspaceFileLoader — mtime 缓存文件加载 |
| `core/src/routing/binding.rs` | AgentBinding, BindingMatchRule 数据结构 |
| `core/src/routing/binding_router.rs` | BindingRouter — 声明式路由解析 |

### 修改文件清单

| 文件路径 | 修改内容 |
|----------|----------|
| `core/src/config/structs.rs` | 新增 `agents: AgentsConfig` 和 `bindings: Vec<AgentBinding>` 字段 |
| `core/src/config/load.rs` | 加载 `[agents]` 和 `[[bindings]]` 段 |
| `core/src/gateway/agent_instance.rs` | 从 `ResolvedAgent` 构建 `AgentInstance` |
| `core/src/thinker/soul.rs` | IdentityResolver 增加 workspace SOUL.md 优先级 |
| `core/src/gateway/interfaces/*.rs` | Channel handlers 注入 BindingRouter 调用 |

---

## 9. 后续扩展 (本次不实现)

以下功能在核心三件套稳定后可作为后续迭代：

| 功能 | 描述 |
|------|------|
| **SubAgent 授权执行** | `subagents.allow` 策略在 SubAgentRegistry 中强制执行 |
| **Agent-to-Agent 通信** | 直接消息传递 (需 opt-in) |
| **每日记忆自动同步** | 浅层 markdown → 深层 LanceDB 的定期同步 |
| **Workspace 热重载** | 文件变更时实时更新 agent 配置 |
| **Agent 模板** | 预定义 agent 原型 (Researcher, Coder, Coordinator) |
| **Per-Agent Sandbox** | 每个 agent 独立的沙箱策略 |

---

## 10. OpenClaw 对比总结

| 维度 | OpenClaw 做法 | Aleph 融合方案 | 差异与优势 |
|------|--------------|---------------|-----------|
| Agent 定义 | TOML 条目 | TOML 条目 + Profile 绑定 | Aleph 支持模板复用 (Profile) |
| Workspace | 目录 + markdown | 目录 + markdown | 基本一致 |
| Channel 路由 | Binding 系统 | Binding 系统 (扩展 Specificity) | 基本一致 |
| 记忆 | 每日 markdown | 双轨制 (markdown + LanceDB) | Aleph 更强 (语义检索) |
| 编排 | 简单 spawn | TaskGraph DAG + Swarm | Aleph 远超 |
| 弹性 | 无 | ShadowReplay + ResourceGovernor | Aleph 独有 |
| 模型选择 | 静态 per-agent | ModelRouter (语义缓存/A/B测试) | Aleph 远超 |
