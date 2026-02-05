# Channel Capability Awareness Architecture

> **Status**: Design Complete
> **Date**: 2026-02-05
> **Author**: Claude + Human Architect
> **Related**: `core/src/thinker/prompt_builder.rs`, OpenClaw system-prompt.ts

## 1. Overview

### 1.1 Problem Statement

Aleph 当前的 `PromptBuilder` 在技术实现上（Rust、Prompt Cache）领先于 OpenClaw，但在"环境感知"上处于开环状态：

- AI 不知道当前运行在 CLI、Web 还是 Telegram
- 可能返回不支持的 UI 操作（如在 CLI 中使用 `ask_user_multigroup`）
- 缺乏对安全边界的透明告知，导致 AI 尝试调用被禁用的工具

### 1.2 Design Goal

实现"环境契约驱动"的自适应系统，让 AI：

1. **感知**当前交互环境的能力边界
2. **理解**安全策略对工具的限制
3. **自适应**输出格式和交互方式

### 1.3 Design Philosophy

**超越 OpenClaw 的关键点**：

| OpenClaw | Aleph (本设计) |
|----------|----------------|
| `buildMessagingSection` 中硬编码条件判断 | 声明式 `InteractionManifest` |
| 安全逻辑与交互逻辑耦合 | 正交组合：交互 × 安全 |
| `NO_REPLY` / `HEARTBEAT_OK` 字符串 token | JSON 协议原生 `silent` / `heartbeat_ok` |
| 被动适配（代码写死支持什么） | 主动适配（AI 根据清单自我调节） |

---

## 2. Core Data Structures

### 2.1 InteractionManifest（交互清单）

描述"技术上能做什么"——由 Channel 声明。

```rust
/// 交互范式 - 定义基础行为模式
pub enum InteractionParadigm {
    CLI,           // 纯文本终端
    WebRich,       // 支持富交互的 Web 界面
    Messaging,     // 即时通讯渠道 (Telegram, Discord...)
    Background,    // 后台任务/定时任务
    Embedded,      // 嵌入式/受限环境
}

/// 原子交互能力
pub enum Capability {
    RichText,           // Markdown/HTML 渲染
    InlineButtons,      // 内联按钮/快速回复
    MultiGroupUI,       // ask_user_multigroup 支持
    Streaming,          // 流式输出
    ImageInline,        // 内联图片显示
    MermaidCharts,      // Mermaid 图表渲染
    CodeHighlight,      // 代码语法高亮
    FileUpload,         // 用户可上传文件
    Canvas,             // 画布/可视化组件
    SilentReply,        // 支持静默回复（后台任务场景）
}

/// 物理约束
pub struct InteractionConstraints {
    pub max_output_chars: Option<usize>,  // 输出长度限制
    pub supports_streaming: bool,
    pub prefer_compact: bool,              // 偏好紧凑输出
}

/// 交互清单 - 三层结构
pub struct InteractionManifest {
    pub paradigm: InteractionParadigm,
    pub capabilities: HashSet<Capability>,
    pub constraints: InteractionConstraints,
}
```

**设计要点**：

- `Paradigm` 提供语义化的"世界观"，让 AI 快速理解所处环境
- `Capability` 是细粒度开关，支持精确控制
- `Constraints` 处理物理限制，避免 AI 产出超长内容被截断

### 2.2 SecurityContext（安全上下文）

描述"策略上允许什么"——由用户/管理员策略决定。

```rust
/// 沙箱级别
pub enum SandboxLevel {
    None,           // 无限制（仅限本地可信用户）
    Standard,       // 标准沙箱：限制文件系统、网络
    Strict,         // 严格沙箱：仅允许只读操作
    Untrusted,      // 不可信代码：完全隔离执行
}

/// 工具权限状态
pub enum ToolPermission {
    Allowed,
    Denied { reason: String },
    RequiresApproval { prompt: String },  // 需要用户确认
}

/// 安全上下文
pub struct SecurityContext {
    pub sandbox_level: SandboxLevel,
    pub allowed_tools: Option<HashSet<String>>,   // 白名单模式
    pub denied_tools: HashSet<String>,            // 黑名单
    pub filesystem_scope: Option<PathBuf>,        // 文件系统边界
    pub network_allowed: bool,
    pub elevated_exec: ElevatedPolicy,
}

pub enum ElevatedPolicy {
    Off,                    // 禁止提权
    Ask,                    // 每次询问用户
    AllowList(Vec<String>), // 特定命令自动批准
    Full,                   // 完全信任（危险）
}

impl SecurityContext {
    pub fn check_tool(&self, tool_name: &str) -> ToolPermission {
        // 黑名单优先
        if self.denied_tools.contains(tool_name) {
            return ToolPermission::Denied {
                reason: "blocked by security policy".into()
            };
        }
        // 白名单检查（如果启用）
        if let Some(ref allowed) = self.allowed_tools {
            if !allowed.contains(tool_name) {
                return ToolPermission::Denied {
                    reason: "not in allowed tools list".into()
                };
            }
        }
        // 特殊处理：exec 工具的提权策略
        if tool_name == "exec" || tool_name == "bash" {
            return self.check_exec_permission();
        }
        ToolPermission::Allowed
    }
}
```

**设计要点**：

- `SandboxLevel` 提供粗粒度的安全预设
- 白名单/黑名单双模式，适应不同场景
- `RequiresApproval` 支持交互式审批流程（对接现有 Exec 审批系统）

---

## 3. Context Aggregation

### 3.1 ContextAggregator（上下文聚合器）

在 PromptBuilder 前进行"对账"，计算最终可用的工具集。

```rust
/// 解析后的上下文 - 传递给 PromptBuilder
pub struct ResolvedContext {
    /// 最终可用的工具列表
    pub available_tools: Vec<ToolInfo>,
    /// 被禁用的工具及原因（用于提示词透明化）
    pub disabled_tools: Vec<DisabledTool>,
    /// 环境契约（用于生成 System Prompt）
    pub environment_contract: EnvironmentContract,
}

pub struct DisabledTool {
    pub name: String,
    pub reason: DisableReason,
}

pub enum DisableReason {
    UnsupportedByChannel,   // 交互层不支持（静默过滤）
    BlockedByPolicy,        // 安全策略禁止（需告知 AI）
    RequiresApproval,       // 需要审批（告知 AI 可请求）
}

/// 环境契约 - AI 的"自我认知"
pub struct EnvironmentContract {
    pub paradigm: InteractionParadigm,
    pub active_capabilities: Vec<Capability>,
    pub constraints: InteractionConstraints,
    pub security_notes: Vec<String>,
}

/// 上下文聚合器
pub struct ContextAggregator;

impl ContextAggregator {
    pub fn resolve(
        interaction: &InteractionManifest,
        security: &SecurityContext,
        all_tools: &[ToolInfo],
    ) -> ResolvedContext {
        let mut available = Vec::new();
        let mut disabled = Vec::new();

        for tool in all_tools {
            // Step 1: 交互层过滤（静默）
            if !interaction.supports_tool(&tool.name) {
                disabled.push(DisabledTool {
                    name: tool.name.clone(),
                    reason: DisableReason::UnsupportedByChannel,
                });
                continue;
            }

            // Step 2: 安全层检查（透明）
            match security.check_tool(&tool.name) {
                ToolPermission::Allowed => available.push(tool.clone()),
                ToolPermission::Denied { reason } => {
                    disabled.push(DisabledTool {
                        name: tool.name.clone(),
                        reason: DisableReason::BlockedByPolicy,
                    });
                }
                ToolPermission::RequiresApproval { .. } => {
                    available.push(tool.clone()); // 工具可用，但执行时需审批
                    disabled.push(DisabledTool {
                        name: tool.name.clone(),
                        reason: DisableReason::RequiresApproval,
                    });
                }
            }
        }

        ResolvedContext {
            available_tools: available,
            disabled_tools: disabled,
            environment_contract: Self::build_contract(interaction, security),
        }
    }
}
```

**两阶段过滤策略**：

| 阶段 | 来源 | 行为 | 原因 |
|------|------|------|------|
| 交互层过滤 | `InteractionManifest` | 静默过滤 | 技术上不支持，无需告知 |
| 安全层过滤 | `SecurityContext` | 透明记录 | 策略禁止，需告知避免困惑 |

---

## 4. PromptBuilder Integration

### 4.1 New Entry Point

```rust
impl PromptBuilder {
    /// 新的入口方法 - 接收 ResolvedContext
    pub fn build_system_prompt_v2(&self, ctx: &ResolvedContext) -> String {
        let mut prompt = String::new();

        // 1. 角色定义（保持现有）
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // 2. 【新增】环境契约章节
        self.append_environment_contract(&mut prompt, &ctx.environment_contract);

        // 3. 工具列表（基于过滤后的 available_tools）
        self.append_tools(&mut prompt, &ctx.available_tools);

        // 4. 【新增】安全约束章节
        self.append_security_constraints(&mut prompt, ctx);

        // 5. 【新增】静默行为章节（如果支持）
        self.append_silent_behavior(&mut prompt, &ctx.environment_contract);

        // 6. 其余章节（响应格式、指南等）
        self.append_response_format(&mut prompt);
        self.append_guidelines(&mut prompt);

        prompt
    }
}
```

### 4.2 Environment Contract Section

```rust
fn append_environment_contract(
    &self,
    prompt: &mut String,
    contract: &EnvironmentContract
) {
    prompt.push_str("## Environment Contract\n\n");

    // 范式声明
    let paradigm_desc = match contract.paradigm {
        InteractionParadigm::CLI =>
            "CLI (text-only terminal)",
        InteractionParadigm::WebRich =>
            "Web Rich Interface (supports interactive UI)",
        InteractionParadigm::Messaging =>
            "Messaging Channel (chat-optimized)",
        InteractionParadigm::Background =>
            "Background Task (no direct user interaction)",
        InteractionParadigm::Embedded =>
            "Embedded/Constrained Environment",
    };
    prompt.push_str(&format!("**Paradigm**: {}\n\n", paradigm_desc));

    // 活跃能力
    if !contract.active_capabilities.is_empty() {
        prompt.push_str("**Active Capabilities**:\n");
        for cap in &contract.active_capabilities {
            let (name, hint) = capability_hint(cap);
            prompt.push_str(&format!("- `{}`: {}\n", name, hint));
        }
        prompt.push('\n');
    }

    // 约束
    prompt.push_str("**Constraints**:\n");
    if let Some(max) = contract.constraints.max_output_chars {
        prompt.push_str(&format!("- Max output: {} chars\n", max));
    }
    if contract.constraints.prefer_compact {
        prompt.push_str("- Prefer concise responses\n");
    }
    if !contract.constraints.supports_streaming {
        prompt.push_str("- No streaming (batch response only)\n");
    }
    prompt.push('\n');
}
```

### 4.3 Security Constraints Section

```rust
fn append_security_constraints(
    &self,
    prompt: &mut String,
    ctx: &ResolvedContext,
) {
    prompt.push_str("## Security & Constraints\n\n");

    // 安全备注
    for note in &ctx.environment_contract.security_notes {
        prompt.push_str(&format!("- {}\n", note));
    }

    // 被策略禁用的工具（透明告知）
    let policy_blocked: Vec<_> = ctx.disabled_tools.iter()
        .filter(|t| matches!(t.reason, DisableReason::BlockedByPolicy))
        .collect();

    if !policy_blocked.is_empty() {
        prompt.push_str("\n**Disabled by Policy**:\n");
        for tool in policy_blocked {
            prompt.push_str(&format!(
                "- `{}` — unavailable in current security context\n",
                tool.name
            ));
        }
    }

    // 需要审批的工具
    let requires_approval: Vec<_> = ctx.disabled_tools.iter()
        .filter(|t| matches!(t.reason, DisableReason::RequiresApproval))
        .collect();

    if !requires_approval.is_empty() {
        prompt.push_str("\n**Requires User Approval**:\n");
        for tool in requires_approval {
            prompt.push_str(&format!(
                "- `{}` — available, but each invocation requires user confirmation\n",
                tool.name
            ));
        }
    }

    prompt.push('\n');
}
```

### 4.4 Silent Behavior Section

```rust
fn append_silent_behavior(&self, prompt: &mut String, contract: &EnvironmentContract) {
    if !contract.active_capabilities.contains(&Capability::SilentReply) {
        return;
    }

    prompt.push_str("## Silent Behavior\n\n");
    prompt.push_str("In background or monitoring contexts, you may have nothing to report.\n\n");
    prompt.push_str("**When to use silent response**:\n");
    prompt.push_str("- Heartbeat poll with no pending tasks → `{\"action\": {\"type\": \"heartbeat_ok\"}}`\n");
    prompt.push_str("- Monitoring check with no anomalies → `{\"action\": {\"type\": \"silent\"}}`\n");
    prompt.push_str("- Already delivered via `message` tool → `{\"action\": {\"type\": \"silent\"}}`\n\n");
    prompt.push_str("**Never** output filler like \"Task complete, standing by\" — use silent instead.\n\n");
}
```

---

## 5. System Integration

### 5.1 ChannelProvider Trait

```rust
/// Channel 注册时提供其交互清单
pub trait ChannelProvider {
    /// 返回该通道的默认交互清单
    fn interaction_manifest(&self) -> InteractionManifest;

    /// 可选：运行时动态调整（如检测到 iTerm2 图片支持）
    fn detect_capabilities(&self) -> Option<HashSet<Capability>> {
        None
    }
}
```

### 5.2 Channel Implementations

```rust
// Telegram Channel
impl ChannelProvider for TelegramChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest {
            paradigm: InteractionParadigm::Messaging,
            capabilities: hashset! {
                Capability::RichText,
                Capability::InlineButtons,
                Capability::ImageInline,
            },
            constraints: InteractionConstraints {
                max_output_chars: Some(4096),
                supports_streaming: false,
                prefer_compact: true,
            },
        }
    }
}

// CLI Channel
impl ChannelProvider for CliChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest {
            paradigm: InteractionParadigm::CLI,
            capabilities: hashset! {
                Capability::RichText,
                Capability::CodeHighlight,
                Capability::Streaming,
            },
            constraints: InteractionConstraints {
                max_output_chars: None,
                supports_streaming: true,
                prefer_compact: false,
            },
        }
    }
}

// Background Task Channel
impl ChannelProvider for BackgroundTaskChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest {
            paradigm: InteractionParadigm::Background,
            capabilities: hashset! {
                Capability::SilentReply,
            },
            constraints: InteractionConstraints {
                max_output_chars: Some(500),
                supports_streaming: false,
                prefer_compact: true,
            },
        }
    }
}
```

### 5.3 Agent Loop Integration

```rust
impl AgentLoop {
    pub async fn run(&mut self) -> Result<()> {
        // 1. 从 Channel 获取交互清单
        let interaction = self.channel.interaction_manifest();

        // 2. 从 Session/Config 获取安全上下文
        let security = self.session.security_context();

        // 3. 聚合上下文
        let resolved = ContextAggregator::resolve(
            &interaction,
            &security,
            &self.all_tools,
        );

        // 4. 构建提示词
        let system_prompt = self.prompt_builder.build_system_prompt_v2(&resolved);

        // 5. 进入循环...
    }
}
```

---

## 6. Generated Prompt Examples

### 6.1 Web Rich Interface

```
## Environment Contract

**Paradigm**: Web Rich Interface (supports interactive UI)

**Active Capabilities**:
- `multi_group_ui`: Use ask_user_multigroup for structured input
- `mermaid`: Render diagrams with ```mermaid blocks
- `streaming`: Your reasoning is visible in real-time

**Constraints**:
- Prefer concise responses

## Security & Constraints

- Running in Standard Sandbox Mode
- Filesystem access limited to: /workspace

**Requires User Approval**:
- `exec` — available, but each invocation requires user confirmation
```

### 6.2 Strict Sandbox CLI

```
## Environment Contract

**Paradigm**: CLI (text-only terminal)

**Active Capabilities**:
- `rich_text`: Markdown rendering supported
- `code_highlight`: Syntax highlighting available

**Constraints**:
- No streaming (batch response only)

## Security & Constraints

- Running in Strict Sandbox Mode
- Filesystem access limited to: /workspace/sandbox
- Network access: blocked

**Disabled by Policy**:
- `web_search` — unavailable in current security context
- `web_fetch` — unavailable in current security context

**Requires User Approval**:
- `exec` — available, but each invocation requires user confirmation
```

### 6.3 Background Task

```
## Environment Contract

**Paradigm**: Background Task (no direct user interaction)

**Active Capabilities**:
- `silent_reply`: Use silent/heartbeat_ok when nothing to report

**Constraints**:
- Max output: 500 chars
- Prefer concise responses

## Silent Behavior

In background or monitoring contexts, you may have nothing to report.

**When to use silent response**:
- Heartbeat poll with no pending tasks → `{"action": {"type": "heartbeat_ok"}}`
- Monitoring check with no anomalies → `{"action": {"type": "silent"}}`

**Never** output filler like "Task complete, standing by" — use silent instead.
```

---

## 7. Implementation Roadmap

### Phase 1: Core Types
- [ ] Define `InteractionParadigm`, `Capability`, `InteractionConstraints`
- [ ] Define `InteractionManifest` struct
- [ ] Define `SecurityContext` and `ToolPermission`

### Phase 2: Aggregation Layer
- [ ] Implement `ContextAggregator::resolve()`
- [ ] Implement `DisableReason` classification
- [ ] Implement `EnvironmentContract` builder

### Phase 3: PromptBuilder Integration
- [ ] Add `build_system_prompt_v2()` method
- [ ] Implement `append_environment_contract()`
- [ ] Implement `append_security_constraints()`
- [ ] Implement `append_silent_behavior()`

### Phase 4: Channel Integration
- [ ] Define `ChannelProvider` trait
- [ ] Implement for existing channels (CLI, Telegram, Discord, Web)
- [ ] Add `detect_capabilities()` for runtime detection

### Phase 5: Agent Loop Integration
- [ ] Modify `AgentLoop::run()` to use new context flow
- [ ] Add `silent` and `heartbeat_ok` action types
- [ ] Update action parsing logic

---

## 8. References

- OpenClaw `system-prompt.ts`: Conditional section builders, `PromptMode`
- OpenClaw `tokens.ts`: `SILENT_REPLY_TOKEN`, `HEARTBEAT_TOKEN`
- Aleph `prompt_builder.rs`: Current implementation, cache support
- Aleph `AGENT_DESIGN_PHILOSOPHY.md`: POE architecture principles
