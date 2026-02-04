# Agent-Action 交互系统设计

> 日期: 2026-01-31
> 状态: 已批准

## 概述

将 suggestion 和 question 模块深度集成到 Aleph 的 agent_loop 架构中，采用 **结构化优先，非结构化作为 fallback** 的策略。

## 设计原则

1. **结构化优先** — 主流 Provider (Claude/GPT/Gemini) 使用原生 tool_use，100% 解析成功率
2. **Fallback 兼容** — 本地模型不支持 tool_use 时，通过 XML 标签解析
3. **类型安全** — 使用 Rust 强类型系统，编译期检查
4. **向后兼容** — 分阶段迁移，提供默认实现

## 架构

```
┌─────────────────────────────────────────────────────────┐
│                    Provider Layer                        │
│  (Claude tool_use / OpenAI function_calling / Gemini)   │
└─────────────────────────┬───────────────────────────────┘
                          │ 结构化 JSON
                          ▼
┌─────────────────────────────────────────────────────────┐
│              Decision Types (增强版)                     │
│  Decision::AskUser { question, kind: QuestionKind }     │
└─────────────────────────┬───────────────────────────────┘
                          │
          ┌───────────────┴───────────────┐
          ▼                               ▼
┌─────────────────────┐         ┌─────────────────────┐
│   主流 Provider     │         │   本地/兼容模型      │
│   (直接结构化)       │         │   (XML fallback)    │
└─────────────────────┘         └─────────────────────┘
```

---

## 核心类型定义

### QuestionKind

```rust
// core/src/agent_loop/question.rs

use serde::{Deserialize, Serialize};

/// 问题类型，决定 UI 渲染方式和验证规则
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuestionKind {
    /// 是/否确认 — 最简单的二元选择
    Confirmation {
        /// 默认选中值（用户直接按 Enter 时采用）
        #[serde(default)]
        default: bool,
        /// 自定义标签，如 ("Approve", "Reject") 替代 ("Yes", "No")
        #[serde(default)]
        labels: Option<(String, String)>,
    },

    /// 单选 — 从多个选项中选一个
    SingleChoice {
        choices: Vec<ChoiceOption>,
        /// 默认选中的索引
        #[serde(default)]
        default_index: Option<usize>,
    },

    /// 多选 — 可选多个
    MultiChoice {
        choices: Vec<ChoiceOption>,
        /// 最少选几个（0 = 可不选）
        #[serde(default)]
        min_selections: usize,
        /// 最多选几个（None = 不限）
        #[serde(default)]
        max_selections: Option<usize>,
    },

    /// 自由文本输入
    TextInput {
        #[serde(default)]
        placeholder: Option<String>,
        /// 多行输入（用于代码、长文本）
        #[serde(default)]
        multiline: bool,
        /// 输入验证（可选）
        #[serde(default)]
        validation: Option<TextValidation>,
    },
}

/// 选项，支持 label + 可选描述
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChoiceOption {
    pub label: String,
    /// 选项的详细描述（UI 可作为 tooltip 或副标题）
    #[serde(default)]
    pub description: Option<String>,
}

/// 文本验证规则
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TextValidation {
    /// 正则匹配
    Regex { pattern: String, message: String },
    /// 长度限制
    Length { min: Option<usize>, max: Option<usize> },
    /// 非空
    Required,
}
```

### Decision (更新版)

```rust
// core/src/agent_loop/decision.rs

/// LLM 做出的决策
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Decision {
    /// 调用工具
    UseTool {
        tool_name: String,
        arguments: Value,
    },

    /// 向用户提问（统一入口）
    AskUser {
        /// 问题文本
        question: String,
        /// 问题类型（决定 UI 渲染）
        kind: QuestionKind,
        /// 可选：问题 ID（用于追踪多轮对话中的同一问题）
        #[serde(default)]
        question_id: Option<String>,
    },

    /// 任务完成
    Complete { summary: String },

    /// 任务失败
    Fail { reason: String },
}
```

**关键改动：**

| 原设计 | 新设计 | 理由 |
|-------|-------|------|
| `AskUser { options: Option<Vec<String>> }` | `AskUser { kind: QuestionKind }` | 类型更丰富 |
| `AskUserMultigroup { groups }` | 移除，用 `MultiChoice` 替代 | 简化 API |

### Action 与 ActionResult

```rust
// core/src/agent_loop/decision.rs

/// 待执行的动作（从 Decision 转换而来）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// 工具调用
    ToolCall {
        tool_name: String,
        arguments: Value,
    },

    /// 用户交互
    UserInteraction {
        question: String,
        kind: QuestionKind,
        question_id: Option<String>,
    },

    /// 完成
    Completion { summary: String },

    /// 失败
    Failure { reason: String },
}

/// 动作执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionResult {
    /// 工具成功
    ToolSuccess {
        output: Value,
        duration_ms: u64,
    },

    /// 工具失败
    ToolError {
        error: String,
        retryable: bool,
    },

    /// 用户响应（统一类型）
    UserResponse {
        response: UserAnswer,
    },

    /// 完成确认
    Completed,

    /// 失败确认
    Failed,
}
```

### UserAnswer

```rust
// core/src/agent_loop/answer.rs

/// 用户回答（结构化）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserAnswer {
    /// 确认结果
    Confirmation { confirmed: bool },
    /// 单选结果
    SingleChoice { selected_index: usize, selected_label: String },
    /// 多选结果
    MultiChoice { selected_indices: Vec<usize>, selected_labels: Vec<String> },
    /// 文本输入
    TextInput { text: String },
    /// 用户取消（适用于所有类型）
    Cancelled,
}

impl UserAnswer {
    /// 转换为 LLM 可理解的文本反馈
    pub fn to_llm_feedback(&self) -> String {
        match self {
            Self::Confirmation { confirmed } => {
                if *confirmed { "User confirmed: Yes" } else { "User confirmed: No" }.into()
            }
            Self::SingleChoice { selected_label, .. } => {
                format!("User selected: {}", selected_label)
            }
            Self::MultiChoice { selected_labels, .. } => {
                format!("User selected: {}", selected_labels.join(", "))
            }
            Self::TextInput { text } => {
                format!("User input: {}", text)
            }
            Self::Cancelled => "User cancelled the operation".into(),
        }
    }
}
```

---

## LoopCallback 更新

```rust
// core/src/agent_loop/callback.rs

#[async_trait]
pub trait LoopCallback: Send + Sync {
    // ===== 生命周期回调（保持不变）=====
    async fn on_loop_start(&self, state: &LoopState);
    async fn on_step_start(&self, step: usize);
    async fn on_thinking_start(&self, step: usize);
    async fn on_thinking_done(&self, thinking: &Thinking);
    async fn on_action_start(&self, action: &Action);
    async fn on_action_done(&self, action: &Action, result: &ActionResult);

    // ===== 用户交互（统一入口）=====

    /// 处理所有类型的用户问题
    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer;

    // ===== 审批相关（保持不变）=====

    /// 工具执行确认（危险操作）
    async fn on_confirmation_required(&self, tool_name: &str, arguments: &Value) -> bool;

    // ===== 错误处理（保持不变）=====
    async fn on_guard_triggered(&self, violation: &GuardViolation);
    async fn on_doom_loop_detected(&self, tool_name: &str, arguments: &Value, count: usize) -> bool;
    async fn on_retry_scheduled(&self, attempt: u32, max: u32, delay_ms: u64, error: &str);
    async fn on_retries_exhausted(&self, attempts: u32, error: &str);

    // ===== 终止回调（保持不变）=====
    async fn on_complete(&self, summary: &str);
    async fn on_failed(&self, reason: &str);
}
```

**关键改动：**

| 原方法 | 新方法 | 变化 |
|-------|-------|------|
| `on_user_input_required(question, options)` | `on_user_question(question, kind)` | 统一入口 |
| `on_user_multigroup_required(question, groups)` | 移除 | 合并到上面 |
| 返回 `String` | 返回 `UserAnswer` | 结构化响应 |

---

## Fallback 解析层

```rust
// core/src/providers/fallback_parser.rs

use crate::agent_loop::decision::{Decision, QuestionKind, ChoiceOption};
use regex::Regex;
use lazy_static::lazy_static;

lazy_static! {
    static ref TOOL_RE: Regex = Regex::new(
        r"<tool\s+name=[\"']([^\"']+)[\"']>\s*([\s\S]*?)\s*</tool>"
    ).unwrap();

    static ref ASK_RE: Regex = Regex::new(
        r"<ask\s+type=[\"']([^\"']+)[\"']>\s*([\s\S]*?)\s*</ask>"
    ).unwrap();

    static ref COMPLETE_RE: Regex = Regex::new(
        r"<complete>\s*([\s\S]*?)\s*</complete>"
    ).unwrap();
}

/// 从非结构化文本解析 Decision
/// 仅在 provider 不支持 tool_use 时调用
pub fn parse_text_to_decision(text: &str) -> Result<Decision, ParseError> {
    // 优先级：tool > ask > complete > fail

    if let Some(caps) = TOOL_RE.captures(text) {
        let tool_name = caps[1].to_string();
        let args_str = &caps[2];
        let arguments = serde_json::from_str(args_str)?;
        return Ok(Decision::UseTool { tool_name, arguments });
    }

    if let Some(caps) = ASK_RE.captures(text) {
        let kind_str = &caps[1];
        let content = &caps[2];
        return parse_ask_block(kind_str, content);
    }

    if let Some(caps) = COMPLETE_RE.captures(text) {
        return Ok(Decision::Complete { summary: caps[1].to_string() });
    }

    // 无法解析时，视为纯文本回复（Complete）
    Ok(Decision::Complete { summary: text.to_string() })
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Invalid JSON in tool arguments: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("Unknown question type: {0}")]
    UnknownQuestionType(String),
}
```

**触发条件（在 Provider 层）：**

```rust
// core/src/providers/mod.rs

impl Provider {
    pub async fn get_decision(&self, prompt: &Prompt) -> Result<Decision> {
        if self.supports_tool_use() {
            // 直接结构化输出
            self.call_with_tools(prompt).await
        } else {
            // Fallback: 文本 + 解析
            let text = self.call_text(prompt.with_xml_instructions()).await?;
            fallback_parser::parse_text_to_decision(&text)
        }
    }
}
```

---

## agent_loop 主循环整合

```rust
// core/src/agent_loop/agent_loop.rs (关键改动部分)

impl<T, E, C> AgentLoop<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    async fn execute_step(&self, state: &mut LoopState) -> Result<StepOutcome> {
        // ... thinking 阶段保持不变 ...

        let decision = self.thinker.think(&state, &tools).await?;

        match decision {
            Decision::UseTool { tool_name, arguments } => {
                // 现有逻辑保持不变
                self.handle_tool_call(state, &tool_name, &arguments).await
            }

            Decision::AskUser { question, kind, question_id } => {
                // 新的统一处理
                self.callback.on_action_start(&Action::UserInteraction {
                    question: question.clone(),
                    kind: kind.clone(),
                    question_id: question_id.clone(),
                }).await;

                let answer = self.callback.on_user_question(&question, &kind).await;

                let result = ActionResult::UserResponse { response: answer.clone() };

                self.callback.on_action_done(
                    &Action::UserInteraction { question, kind, question_id },
                    &result,
                ).await;

                // 将结构化回答转为 LLM 可理解的格式
                let feedback = answer.to_llm_feedback();
                state.add_user_response(feedback);

                Ok(StepOutcome::Continue)
            }

            Decision::Complete { summary } => {
                self.callback.on_complete(&summary).await;
                Ok(StepOutcome::Done(LoopResult::Completed(summary)))
            }

            Decision::Fail { reason } => {
                self.callback.on_failed(&reason).await;
                Ok(StepOutcome::Done(LoopResult::Failed(reason)))
            }
        }
    }
}
```

---

## CLI 层实现

```rust
// core/src/agent_loop/callback_cli.rs

use inquire::{Confirm, Select, MultiSelect, Text};
use crate::agent_loop::decision::{QuestionKind, UserAnswer, ChoiceOption};

/// CLI 环境下的 LoopCallback 实现
pub struct CliLoopCallback {
    pub auto_approve_confirmations: bool,
}

impl CliLoopCallback {
    /// 处理用户问题（核心方法）
    pub fn prompt_user(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        match kind {
            QuestionKind::Confirmation { default, labels } => {
                let (yes, no) = labels.clone().unwrap_or_else(||
                    ("Yes".into(), "No".into())
                );

                match Confirm::new(question)
                    .with_default(*default)
                    .with_help_message(&format!("{} / {}", yes, no))
                    .prompt()
                {
                    Ok(confirmed) => UserAnswer::Confirmation { confirmed },
                    Err(_) => UserAnswer::Cancelled,
                }
            }

            QuestionKind::SingleChoice { choices, default_index } => {
                let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();

                let mut select = Select::new(question, labels);
                if let Some(idx) = default_index {
                    select = select.with_starting_cursor(*idx);
                }

                match select.prompt() {
                    Ok(selected) => {
                        let idx = choices.iter().position(|c| c.label == selected).unwrap();
                        UserAnswer::SingleChoice {
                            selected_index: idx,
                            selected_label: selected.to_string(),
                        }
                    }
                    Err(_) => UserAnswer::Cancelled,
                }
            }

            QuestionKind::MultiChoice { choices, min_selections, .. } => {
                let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();

                match MultiSelect::new(question, labels).prompt() {
                    Ok(selected) => {
                        if selected.len() < *min_selections {
                            return UserAnswer::Cancelled;
                        }
                        let indices: Vec<usize> = selected.iter()
                            .filter_map(|s| choices.iter().position(|c| &c.label == *s))
                            .collect();
                        UserAnswer::MultiChoice {
                            selected_indices: indices,
                            selected_labels: selected.into_iter().map(String::from).collect(),
                        }
                    }
                    Err(_) => UserAnswer::Cancelled,
                }
            }

            QuestionKind::TextInput { placeholder, .. } => {
                let mut prompt = Text::new(question);
                if let Some(p) = placeholder {
                    prompt = prompt.with_placeholder(p);
                }

                match prompt.prompt() {
                    Ok(text) => UserAnswer::TextInput { text },
                    Err(_) => UserAnswer::Cancelled,
                }
            }
        }
    }
}
```

---

## 文件清单

### 新增文件

| 文件路径 | 职责 |
|---------|------|
| `core/src/agent_loop/question.rs` | `QuestionKind`, `ChoiceOption`, `TextValidation` 类型 |
| `core/src/agent_loop/answer.rs` | `UserAnswer` 类型及 `to_llm_feedback()` |
| `core/src/agent_loop/callback_cli.rs` | CLI 环境的 `LoopCallback` 实现 |
| `core/src/providers/fallback_parser.rs` | 非结构化文本 → Decision 解析器 |

### 修改文件

| 文件路径 | 改动内容 |
|---------|---------|
| `core/src/agent_loop/decision.rs` | 更新 `Decision`, `Action`, `ActionResult` |
| `core/src/agent_loop/callback.rs` | 更新 `LoopCallback` trait，移除旧方法 |
| `core/src/agent_loop/agent_loop.rs` | 整合新的 `AskUser` 处理逻辑 |
| `core/src/agent_loop/mod.rs` | 导出新模块 |
| `core/Cargo.toml` | 添加 `inquire = "0.7"` 依赖 |

---

## 迁移计划

### Phase 1 - 添加新类型（非破坏性）

- 新增 `question.rs`, `answer.rs`
- 在 `decision.rs` 中添加新变体，保留旧变体

### Phase 2 - 更新 Callback

- 添加 `on_user_question()` 方法
- 提供默认实现委托到旧方法

### Phase 3 - 迁移主循环

- 更新 `agent_loop.rs` 使用新类型
- 添加 `callback_cli.rs`

### Phase 4 - 清理

- 移除 `AskUserMultigroup` 变体
- 移除旧的 callback 方法

---

## 与现有架构的关系

本设计增强了 Aleph 现有的 agent_loop 架构，主要改动集中在：

1. **Decision 类型扩展** — 更丰富的用户交互类型
2. **UserAnswer 结构化** — 替代简单的 String 返回
3. **Fallback 层** — 兼容不支持 tool_use 的 Provider

保持不变的部分：

- Guard 系统（stuck loop, doom loop 检测）
- Permission Mode（Normal/AutoAccept/PlanMode）
- Compaction 和 Overflow 检测
- EventBus 集成
