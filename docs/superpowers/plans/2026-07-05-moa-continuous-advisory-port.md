# MoA 连续咨询移植 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 hermes-agent 的 MoA（Mixture of Agents）连续咨询机制移植为 Aleph 的虚拟 Provider 门面：每个 Think 迭代前 N 个 advisor 模型并行咨询当前对话状态，建议作为私密指导注入聚合器（=行动模型）prompt 末尾，agent loop 完全无感知。

**Architecture:** `MoaProvider` 实现 `AiProvider`，插入 `runner_impl.rs` Step 3 brain-pick 缝（`ModelOverrideProvider` 同款），零改 `src/harness/`（仅 `trace.rs` 加枚举变体）。advisor 经 `named_providers` 链解析继承熔断/降级。会话状态走 `session_model_handle` 同款进程级 map，one-shot 用 consume-and-clear 原子语义。

**Tech Stack:** Rust (tokio + futures::join_all + serde/schemars)，Leptos/WASM panel（仅 2 个事件渲染分支）。零新依赖。

**Spec:** [docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md](../specs/2026-07-05-moa-continuous-advisory-port-design.md)；四份深读报告（全部 file:line 锚点）在 [specs/assets/2026-07-05-moa/](../specs/assets/2026-07-05-moa/)。

## Global Constraints

- **R10**：`src/harness/` 只许 `trace.rs` 增加枚举变体（~25 行），不加文件不动其他 11 个文件。
- **零新依赖**：`futures = "0.3"` 与 tokio `time` feature 已是直接依赖（Cargo.toml:171 / :134）。签名哈希用 `std::collections::hash_map::DefaultHasher`，不引入 sha2。
- **cargo 节制**（用户全局约束）：每个任务只跑**过滤后的**测试 `cargo test -p alephcore --lib <filter>`；不跑全量套件；`cargo check -p alephcore --lib` 每任务至多一次。Panel 任务用一次 `just wasm` 验证。
- **提交格式**：英文 `<scope>: <description>`，无 attribution footer（全局 settings 已禁用）。
- **命名**：hermes "reference models" → Aleph **advisors**；聚合器 = acting model。用户可见文案：panel 中文、prompt 英文、代码注释英文。
- **不硬编码默认 preset 模型**；无可用 preset 时 fail-soft 回退普通 provider 链 + warn。
- **advisor 用量绝不混入返回的 `ProviderResponse.usage`**（gauge 诚实性）；每 advisor 独立 `MeteringProvider` + 汇总 `MoaAdvisorSpend` 事件。
- **⚠️ Spec 决定 #6 修订**（已验证，见 Task 10）：gateway 每回合 `model_override` 在 harness 路径上只进 `ModelResolved` 事件与健康上报（`run_loop/inner.rs:480-523,1019,1045,1095` 是 `resolved` 的全部消费者；`FlowRequest` 无模型字段），**从不影响 runner Step 3**。故优先级实现为 **MoA > select_model pick > agent pin > brain**；model_override 交互留待其自身管道补通时挂钩。
- `UnifiedMessage` / `ContentBlock` 是 `#[non_exhaustive]`——所有 match 必须带通配 arm。
- 锁风格：`std` 锁用 `.unwrap_or_else(|e| e.into_inner())`（项目 P7 惯例）。

---

### Task 1: `[moa]` 配置节（类型 + 校验 + 挂载）

**Files:**
- Create: `src/config/types/moa.rs`
- Modify: `src/config/types/mod.rs`（`pub mod moa;` + `pub use moa::*;`）
- Modify: `src/config/structs.rs`（字段 + Default）
- Test: `src/config/types/moa.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Consumes: 无（纯类型）
- Produces: `MoaToml { default_preset: Option<String>, save_traces: bool, presets: HashMap<String, MoaPreset> }`、`MoaPreset { enabled, advisors: Vec<MoaSlot>, aggregator: MoaSlot, fanout: MoaFanout, advisor_timeout_secs: u64, advisor_max_tokens: Option<u32>, advisor_temperature: Option<f32>, aggregator_temperature: Option<f32> }`、`MoaSlot { provider: String, model: String }`、`MoaFanout::PerIteration | UserTurn`、`MoaToml::resolve_preset(name: Option<&str>) -> Option<(String, &MoaPreset)>`、`MoaToml::validation_errors() -> Vec<String>`。后续任务经 `crate::config::MoaToml` 等路径使用（types/mod.rs glob re-export）。

- [ ] **Step 1: 写失败测试**

创建 `src/config/types/moa.rs`，先只写测试骨架 + 空实现让编译失败地暴露缺失（Rust 下 TDD 以"测试引用尚不存在的项"体现——直接整文件一次写完类型与测试，跑测试验证）：

- [ ] **Step 2: 写完整实现 + 测试**

```rust
//! MoA (Mixture of Agents) configuration — the `[moa]` config.toml section.
//!
//! Ported from hermes-agent's moa_config.py, adapted to typed Rust config:
//! validation happens at load/patch time instead of runtime string coercion.
//! Spec: docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One advisor/aggregator slot: a (provider, model) pair. `provider` must
/// name a `[providers.<key>]` entry; `"moa"` is rejected (recursion guard,
/// layer 1 of 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MoaSlot {
    pub provider: String,
    pub model: String,
}

/// Advisor fan-out cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MoaFanout {
    /// Advisors re-run whenever the advisory view changes (every tool
    /// iteration). hermes default — maximally informed.
    #[default]
    PerIteration,
    /// Advisors run once per user turn (= once per run); later iterations
    /// reuse that advice. The original MoA shape.
    UserTurn,
}

/// One named MoA preset.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MoaPreset {
    /// `false` = skip advisors entirely; the aggregator acts alone.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Advisor slots, fanned out in parallel on each consultation.
    #[serde(default)]
    pub advisors: Vec<MoaSlot>,
    /// The acting model: gets the full payload + advisor guidance.
    pub aggregator: MoaSlot,
    #[serde(default)]
    pub fanout: MoaFanout,
    /// Per-advisor wall-clock budget in seconds. A timed-out advisor
    /// degrades to a labelled note (hermes has no timeout at all).
    #[serde(default = "default_advisor_timeout_secs")]
    pub advisor_timeout_secs: u64,
    /// Caps ONLY advisor output (the dominant latency lever); the acting
    /// aggregator is never capped here. `None` = provider default.
    #[serde(default)]
    pub advisor_max_tokens: Option<u32>,
    /// `None` = omit the parameter so the provider default applies.
    #[serde(default)]
    pub advisor_temperature: Option<f32>,
    #[serde(default)]
    pub aggregator_temperature: Option<f32>,
}

/// The `[moa]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct MoaToml {
    /// Preset used by `/moa` one-shot and a bare `moa on`.
    #[serde(default)]
    pub default_preset: Option<String>,
    /// Gate for the heavy `MoaTurnTrace` (full advisor I/O) trace events.
    #[serde(default)]
    pub save_traces: bool,
    #[serde(default)]
    pub presets: HashMap<String, MoaPreset>,
}

const fn default_true() -> bool {
    true
}

const fn default_advisor_timeout_secs() -> u64 {
    120
}

impl MoaToml {
    /// Resolve a preset: explicit name > `default_preset` > the sole preset
    /// when exactly one exists. Returns the resolved key alongside the preset.
    #[must_use]
    pub fn resolve_preset(&self, name: Option<&str>) -> Option<(String, &MoaPreset)> {
        let key = name
            .map(str::to_string)
            .or_else(|| self.default_preset.clone())
            .or_else(|| {
                (self.presets.len() == 1).then(|| self.presets.keys().next().cloned().unwrap())
            })?;
        self.presets.get(&key).map(|p| (key, p))
    }

    /// Validation errors; empty when valid. Layer-1 recursion guard.
    #[must_use]
    pub fn validation_errors(&self) -> Vec<String> {
        let mut errs = Vec::new();
        for (name, preset) in &self.presets {
            if name.trim().is_empty() {
                errs.push("[moa] preset name must not be empty".to_string());
            }
            let mut slots: Vec<&MoaSlot> = preset.advisors.iter().collect();
            slots.push(&preset.aggregator);
            for slot in slots {
                if slot.provider.trim().is_empty() || slot.model.trim().is_empty() {
                    errs.push(format!(
                        "[moa.presets.{name}] slot provider/model must be non-empty"
                    ));
                }
                if slot.provider.trim().eq_ignore_ascii_case("moa") {
                    errs.push(format!(
                        "[moa.presets.{name}] slots cannot reference provider 'moa' \
                         (recursive MoA is forbidden)"
                    ));
                }
            }
            if preset.enabled && preset.advisors.is_empty() {
                errs.push(format!(
                    "[moa.presets.{name}] an enabled preset needs at least one advisor"
                ));
            }
        }
        if let Some(d) = &self.default_preset {
            if !self.presets.contains_key(d) {
                errs.push(format!("[moa] default_preset '{d}' does not exist"));
            }
        }
        errs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset_toml() -> &'static str {
        r#"
default_preset = "default"

[presets.default]
advisors = [
  { provider = "openai", model = "gpt-5.5" },
  { provider = "deepseek", model = "deepseek-v4" },
]
aggregator = { provider = "anthropic", model = "claude-opus-4-8" }
"#
    }

    #[test]
    fn defaults_from_empty_toml() {
        let parsed: MoaToml = toml::from_str("").unwrap();
        assert!(parsed.presets.is_empty());
        assert!(!parsed.save_traces);
        assert_eq!(parsed.default_preset, None);
    }

    #[test]
    fn preset_missing_fields_get_defaults() {
        let parsed: MoaToml = toml::from_str(preset_toml()).unwrap();
        let p = &parsed.presets["default"];
        assert!(p.enabled);
        assert_eq!(p.fanout, MoaFanout::PerIteration);
        assert_eq!(p.advisor_timeout_secs, 120);
        assert_eq!(p.advisor_max_tokens, None);
        assert_eq!(p.advisor_temperature, None);
        assert!(parsed.validation_errors().is_empty());
    }

    #[test]
    fn fanout_parses_snake_case() {
        let parsed: MoaToml = toml::from_str(
            r#"
[presets.p]
fanout = "user_turn"
advisors = [{ provider = "a", model = "m" }]
aggregator = { provider = "b", model = "n" }
"#,
        )
        .unwrap();
        assert_eq!(parsed.presets["p"].fanout, MoaFanout::UserTurn);
    }

    #[test]
    fn recursive_moa_slot_rejected_case_insensitive() {
        for prov in ["moa", "MoA", "MOA"] {
            let cfg = MoaToml {
                presets: HashMap::from([(
                    "p".to_string(),
                    MoaPreset {
                        enabled: true,
                        advisors: vec![MoaSlot {
                            provider: prov.to_string(),
                            model: "m".to_string(),
                        }],
                        aggregator: MoaSlot {
                            provider: "anthropic".to_string(),
                            model: "n".to_string(),
                        },
                        fanout: MoaFanout::default(),
                        advisor_timeout_secs: 120,
                        advisor_max_tokens: None,
                        advisor_temperature: None,
                        aggregator_temperature: None,
                    },
                )]),
                ..MoaToml::default()
            };
            assert!(
                cfg.validation_errors().iter().any(|e| e.contains("recursive")),
                "provider {prov} must be rejected"
            );
        }
    }

    #[test]
    fn enabled_preset_without_advisors_invalid() {
        let parsed: MoaToml = toml::from_str(
            r#"
[presets.p]
aggregator = { provider = "b", model = "n" }
"#,
        )
        .unwrap();
        assert!(parsed
            .validation_errors()
            .iter()
            .any(|e| e.contains("at least one advisor")));
    }

    #[test]
    fn unknown_default_preset_invalid() {
        let parsed: MoaToml = toml::from_str("default_preset = \"ghost\"").unwrap();
        assert!(parsed
            .validation_errors()
            .iter()
            .any(|e| e.contains("does not exist")));
    }

    #[test]
    fn resolve_preset_precedence() {
        let parsed: MoaToml = toml::from_str(preset_toml()).unwrap();
        // explicit name
        assert_eq!(parsed.resolve_preset(Some("default")).unwrap().0, "default");
        // unknown explicit name -> None
        assert!(parsed.resolve_preset(Some("ghost")).is_none());
        // default_preset fallback
        assert_eq!(parsed.resolve_preset(None).unwrap().0, "default");
        // sole-preset fallback when default_preset unset
        let mut solo = parsed.clone();
        solo.default_preset = None;
        assert_eq!(solo.resolve_preset(None).unwrap().0, "default");
    }
}
```

- [ ] **Step 3: 挂载到 Config**

`src/config/types/mod.rs`：在 `pub mod memory;` 附近按字母序加 `pub mod moa;`，在 `pub use memory::*;` 附近加 `pub use moa::*;`。

`src/config/structs.rs`：在 `pub strategy: Option<...>` 字段（`:214` 附近）后加：

```rust
    /// MoA (Mixture of Agents) continuous-advisory presets (`[moa]`). When
    /// present and a session activates MoA, run construction wraps the brain
    /// in a `MoaProvider` facade (advisors consult in parallel; the preset's
    /// aggregator acts). Absent ⇒ feature dormant, zero cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moa: Option<crate::config::types::moa::MoaToml>,
```

`Default` impl（`:417-484`）加 `moa: None,`（放 `strategy: None,` 后）。

- [ ] **Step 4: 跑测试**

Run: `cargo test -p alephcore --lib config::types::moa`
Expected: 7 个测试 PASS（编译含 structs.rs 挂载）。

- [ ] **Step 5: Commit**

```bash
git add src/config/types/moa.rs src/config/types/mod.rs src/config/structs.rs
git commit -m "config: add [moa] section types with presets, fanout cadence and recursion-guard validation"
```

---

### Task 2: 会话 MoA 状态 handle（sticky + one-shot consume-and-clear）

**Files:**
- Create: `src/providers/session_moa_handle.rs`
- Modify: `src/providers/mod.rs`（加 `pub mod session_moa_handle;`——放在 `pub mod session_model_handle;` 旁；先 `grep -n "session_model_handle" src/providers/mod.rs` 找锚点）
- Test: 文件内 `#[cfg(test)]`

**Interfaces:**
- Consumes: 无
- Produces: `SessionMoaPref { preset: Option<String>, one_shot: bool }`、`set_session_moa(session_key: &str, preset: Option<String>, one_shot: bool)`、`get_session_moa(session_key: &str) -> Option<SessionMoaPref>`（非消费，status 用）、`take_for_run(session_key: &str) -> Option<SessionMoaPref>`（**one_shot 在读取的同一写锁内原子移除**；sticky 保留）、`clear_session_moa(session_key: &str)`。路径 `crate::providers::session_moa_handle::*`。

- [ ] **Step 1: 写实现 + 测试**（镜像 `src/providers/session_model_handle.rs`，完整文件）：

```rust
//! Process-global per-session MoA activation state.
//!
//! Mirrors [`session_model_handle`](super::session_model_handle): a
//! process-global lock-guarded map keyed by the canonical `SessionKey`
//! string; written by the `moa` tool / the `/moa` one-shot intercept, read
//! (and for one-shots, consumed) at run construction in `harness_bridge`.
//! In-memory by design — soft UX state that resets on restart.
//!
//! One-shot restore is a single mechanism: [`take_for_run`] removes a
//! `one_shot` pref atomically under the write lock, so success, error and
//! cancel paths all leave no state behind (hermes needed three divergent
//! restore implementations; this is surpass item ④ in the spec).

use crate::sync_primitives::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

/// A session's MoA activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMoaPref {
    /// Preset name; `None` = the config `default_preset`.
    pub preset: Option<String>,
    /// `true` = applies to exactly one run, consumed by [`take_for_run`].
    pub one_shot: bool,
}

static SESSION_MOA: OnceLock<RwLock<HashMap<String, SessionMoaPref>>> = OnceLock::new();

fn map() -> &'static RwLock<HashMap<String, SessionMoaPref>> {
    SESSION_MOA.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Record (or overwrite) the session's MoA activation.
pub fn set_session_moa(session_key: &str, preset: Option<String>, one_shot: bool) {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_key.to_string(), SessionMoaPref { preset, one_shot });
}

/// Non-consuming read (for `moa status`).
#[must_use]
pub fn get_session_moa(session_key: &str) -> Option<SessionMoaPref> {
    map()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_key)
        .cloned()
}

/// Read for run construction. A `one_shot` pref is REMOVED in the same
/// write-lock section it is read in — the single restore point.
#[must_use]
pub fn take_for_run(session_key: &str) -> Option<SessionMoaPref> {
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    let pref = guard.get(session_key).cloned()?;
    if pref.one_shot {
        guard.remove(session_key);
    }
    Some(pref)
}

/// Drop the session's activation (`moa off`).
pub fn clear_session_moa(session_key: &str) {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_survives_take_for_run() {
        let key = "test:moa:sticky";
        set_session_moa(key, Some("deep".to_string()), false);
        let taken = take_for_run(key).unwrap();
        assert_eq!(taken.preset.as_deref(), Some("deep"));
        assert!(!taken.one_shot);
        // Sticky: still present for the next run.
        assert!(take_for_run(key).is_some());
        clear_session_moa(key);
        assert!(take_for_run(key).is_none());
    }

    #[test]
    fn one_shot_consumed_atomically() {
        let key = "test:moa:oneshot";
        set_session_moa(key, None, true);
        let taken = take_for_run(key).unwrap();
        assert!(taken.one_shot);
        // Consumed: a second run sees nothing — no restore step can leak.
        assert!(take_for_run(key).is_none());
        assert!(get_session_moa(key).is_none());
    }

    #[test]
    fn status_read_does_not_consume() {
        let key = "test:moa:status";
        set_session_moa(key, None, true);
        assert!(get_session_moa(key).is_some());
        assert!(get_session_moa(key).is_some());
        clear_session_moa(key);
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p alephcore --lib session_moa_handle`
Expected: 3 PASS。

- [ ] **Step 3: Commit**

```bash
git add src/providers/session_moa_handle.rs src/providers/mod.rs
git commit -m "providers: session MoA handle with atomic one-shot consume-and-clear"
```

---

### Task 3: 顾问视图变换（纯函数）

**Files:**
- Create: `src/providers/moa/mod.rs`（本任务先只含 `pub mod advisory_view;`，后续任务扩充）
- Create: `src/providers/moa/advisory_view.rs`
- Modify: `src/providers/mod.rs`（加 `pub mod moa;`）
- Test: `advisory_view.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Consumes: `crate::providers::message::{UnifiedMessage, ContentBlock}`（注意两者 `#[non_exhaustive]`；`UnifiedMessage::user(text)` / `::assistant(text)` 构造器可用）
- Produces: `pub(crate) fn build_advisory_view(messages: &[UnifiedMessage]) -> Vec<UnifiedMessage>`、`pub(crate) fn view_signature(view: &[UnifiedMessage]) -> u64`、`pub(crate) fn truncate_tool_result(text: &str, budget: usize) -> String`、`pub(crate) const TOOL_RESULT_BUDGET: usize = 4000`、`pub(crate) const ADVISORY_INSTRUCTION: &str`

**扁平化规则**（忠实移植 hermes `_reference_messages`，spec §4.3.1）：产出只含 User/Assistant 纯文本消息，零 ToolResult 变体、零 ToolCall 块；User → Text 块拼接为一条 user；Assistant → Text 块 + 每个 ToolCall 渲染为 `[called tool: name(args_json)]` 行（Thinking/Json/Image 跳过），全空则丢弃；ToolResult → 文本（Text 拼接 + Json 用 `value.to_string()`）经 head+tail 截断后成 `[tool result: ...]` 块（`is_error` 时 `[tool result (error): ...]`），追加进前一条 assistant 文本，无前驱 assistant 则独立成 assistant 消息；末尾是 assistant → 追加 `ADVISORY_INSTRUCTION` user 轮；全空退化 → 最后一条 user 原文（无则空 Vec）。截断必须 UTF-8 安全（用 `char_indices` 找边界，禁止 `&s[..n]`）。

- [ ] **Step 1: 写实现 + 测试**

`advisory_view.rs`：

```rust
//! Advisory-view transform: flatten the acting agent's conversation into
//! plain user/assistant text turns for advisor (reference) models.
//!
//! Faithful port of hermes moa_loop.py `_reference_messages`: advisors see
//! what the agent DID (tool calls) and what came back (truncated tool
//! results) as text — zero tool-role messages, zero tool_calls arrays — so
//! strict providers never 400, and the view always ends on a user turn
//! (Anthropic no-trailing-assistant-prefill rule) without deleting context.

use std::hash::{Hash, Hasher};

use crate::providers::message::{ContentBlock, UnifiedMessage};

/// Per-tool-result character budget for the advisory copy. The acting
/// aggregator always gets the untrimmed transcript; this only shapes the
/// disposable advisory view.
pub(crate) const TOOL_RESULT_BUDGET: usize = 4000;

/// Synthetic trailing user turn when the view would end on an assistant turn.
pub(crate) const ADVISORY_INSTRUCTION: &str =
    "[The conversation above is the current state of the task. Give your \
     most intelligent judgement: what is going on, what should happen next, \
     what risks or mistakes you see, and how the acting agent should \
     proceed.]";

/// Head+tail preview with a `[... N chars omitted ...]` marker. UTF-8 safe.
pub(crate) fn truncate_tool_result(text: &str, budget: usize) -> String {
    let total = text.chars().count();
    if total <= budget {
        return text.to_string();
    }
    let half = budget / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = {
        let skip = total - half;
        text.chars().skip(skip).collect()
    };
    let omitted = total - 2 * half;
    format!("{head}\n[... {omitted} chars omitted ...]\n{tail}")
}

fn text_of(blocks: &[ContentBlock]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => {
                if !text.is_empty() {
                    parts.push(text.clone());
                }
            }
            ContentBlock::Json { value } => parts.push(value.to_string()),
            // Thinking is the acting model's private reasoning; ToolCall is
            // rendered separately; images carry no advisory text.
            _ => {}
        }
    }
    parts.join("\n")
}

fn render_tool_calls(blocks: &[ContentBlock]) -> Vec<String> {
    let mut lines = Vec::new();
    for block in blocks {
        if let ContentBlock::ToolCall { name, arguments, .. } = block {
            let args = if arguments.is_null() {
                String::new()
            } else {
                arguments.to_string()
            };
            if args.is_empty() {
                lines.push(format!("[called tool: {name}]"));
            } else {
                lines.push(format!("[called tool: {name}({args})]"));
            }
        }
    }
    lines
}

fn append_to_last_assistant(rendered: &mut Vec<UnifiedMessage>, block: String) {
    if let Some(UnifiedMessage::Assistant { content }) = rendered.last_mut() {
        if let Some(ContentBlock::Text { text, .. }) = content.last_mut() {
            text.push('\n');
            text.push_str(&block);
            return;
        }
    }
    rendered.push(UnifiedMessage::assistant(block));
}

/// Build the flattened advisory view. See module docs for the rules.
pub(crate) fn build_advisory_view(messages: &[UnifiedMessage]) -> Vec<UnifiedMessage> {
    let mut rendered: Vec<UnifiedMessage> = Vec::new();
    let mut last_user_text: Option<String> = None;

    for msg in messages {
        match msg {
            UnifiedMessage::User { content } => {
                let text = text_of(content);
                if !text.trim().is_empty() {
                    last_user_text = Some(text.clone());
                }
                rendered.push(UnifiedMessage::user(text));
            }
            UnifiedMessage::Assistant { content } => {
                let mut parts: Vec<String> = Vec::new();
                let text = text_of(content);
                if !text.trim().is_empty() {
                    parts.push(text.trim().to_string());
                }
                parts.extend(render_tool_calls(content));
                if !parts.is_empty() {
                    rendered.push(UnifiedMessage::assistant(parts.join("\n")));
                }
            }
            UnifiedMessage::ToolResult {
                content, is_error, ..
            } => {
                let result_text = truncate_tool_result(&text_of(content), TOOL_RESULT_BUDGET);
                let tag = if *is_error {
                    "tool result (error)"
                } else {
                    "tool result"
                };
                append_to_last_assistant(&mut rendered, format!("[{tag}: {result_text}]"));
            }
            // #[non_exhaustive]: future variants carry no advisory meaning
            // until explicitly handled.
            _ => {}
        }
    }

    match rendered.last() {
        Some(UnifiedMessage::Assistant { .. }) => {
            rendered.push(UnifiedMessage::user(ADVISORY_INSTRUCTION));
        }
        Some(_) => {}
        None => {
            if let Some(text) = last_user_text {
                rendered.push(UnifiedMessage::user(text));
            }
        }
    }
    rendered
}

/// Stable signature of the advisory view — the fan-out cache key. Uses the
/// std hasher (cache dedup only, not security).
pub(crate) fn view_signature(view: &[UnifiedMessage]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for msg in view {
        let (role, content) = match msg {
            UnifiedMessage::User { content } => ("user", content),
            UnifiedMessage::Assistant { content } => ("assistant", content),
            UnifiedMessage::ToolResult { content, .. } => ("tool", content),
            _ => continue,
        };
        role.hash(&mut hasher);
        text_of(content).hash(&mut hasher);
    }
    hasher.finish()
}
```

测试（同文件 `#[cfg(test)] mod tests`，用真实 `UnifiedMessage` 构造器）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant_with_tool_call() -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text { text: "Let me check.".to_string(), cache_control: None },
                ContentBlock::ToolCall {
                    id: "c1".to_string(),
                    name: "bash".to_string(),
                    arguments: json!({"cmd": "ls"}),
                    thought_signature: None,
                },
            ],
        }
    }

    fn view_texts(view: &[UnifiedMessage]) -> Vec<(&'static str, String)> {
        view.iter()
            .map(|m| match m {
                UnifiedMessage::User { content } => ("user", super::text_of(content)),
                UnifiedMessage::Assistant { content } => ("assistant", super::text_of(content)),
                _ => panic!("advisory view must contain only user/assistant"),
            })
            .collect()
    }

    #[test]
    fn tool_calls_rendered_as_text_and_results_folded() {
        let msgs = vec![
            UnifiedMessage::user("fix the bug"),
            assistant_with_tool_call(),
            UnifiedMessage::tool_result("c1", "bash", "file1\nfile2", false),
        ];
        let view = build_advisory_view(&msgs);
        let texts = view_texts(&view);
        // user, assistant(text+call+result), synthetic trailing user
        assert_eq!(texts.len(), 3);
        assert_eq!(texts[0], ("user", "fix the bug".to_string()));
        assert!(texts[1].1.contains("[called tool: bash("));
        assert!(texts[1].1.contains("[tool result: file1"));
        assert_eq!(texts[2].1, ADVISORY_INSTRUCTION);
    }

    #[test]
    fn error_results_labelled() {
        let msgs = vec![
            UnifiedMessage::user("go"),
            assistant_with_tool_call(),
            UnifiedMessage::tool_result("c1", "bash", "boom", true),
        ];
        let view = build_advisory_view(&msgs);
        assert!(view_texts(&view)[1].1.contains("[tool result (error): boom]"));
    }

    #[test]
    fn fresh_user_turn_kept_as_terminal() {
        let view = build_advisory_view(&[UnifiedMessage::user("hello")]);
        let texts = view_texts(&view);
        assert_eq!(texts, vec![("user", "hello".to_string())]);
    }

    #[test]
    fn orphan_tool_result_becomes_assistant_line() {
        let msgs = vec![UnifiedMessage::tool_result("c9", "bash", "out", false)];
        let view = build_advisory_view(&msgs);
        let texts = view_texts(&view);
        assert_eq!(texts[0].0, "assistant");
        assert!(texts[0].1.starts_with("[tool result: out]"));
        // ends on the synthetic user turn
        assert_eq!(texts.last().unwrap().1, ADVISORY_INSTRUCTION);
    }

    #[test]
    fn truncation_is_head_tail_and_utf8_safe() {
        let long = "汉".repeat(5000);
        let out = truncate_tool_result(&long, 4000);
        assert!(out.contains("chars omitted"));
        assert!(out.chars().count() < 4100);
        // must not panic on multi-byte boundaries (would have above)
    }

    #[test]
    fn short_results_untouched() {
        assert_eq!(truncate_tool_result("ok", 4000), "ok");
    }

    #[test]
    fn signature_changes_with_new_tool_result_and_is_stable() {
        let base = vec![UnifiedMessage::user("go"), assistant_with_tool_call()];
        let v1 = build_advisory_view(&base);
        let mut grown = base.clone();
        grown.push(UnifiedMessage::tool_result("c1", "bash", "out", false));
        let v2 = build_advisory_view(&grown);
        assert_ne!(view_signature(&v1), view_signature(&v2));
        assert_eq!(view_signature(&v1), view_signature(&build_advisory_view(&base)));
    }

    #[test]
    fn empty_assistant_dropped() {
        let msgs = vec![
            UnifiedMessage::user("go"),
            UnifiedMessage::Assistant { content: vec![] },
        ];
        let view = build_advisory_view(&msgs);
        assert_eq!(view_texts(&view).len(), 1);
    }
}
```

`src/providers/moa/mod.rs`（本任务版本）：

```rust
//! MoA (Mixture of Agents) virtual-provider facade.
//!
//! Ported from hermes-agent's MoAClient: the agent loop is unaware of MoA;
//! advisors consult on a flattened view of the live conversation, and the
//! preset's aggregator is the acting model.
//! Spec: docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md

pub(crate) mod advisory_view;
```

注：`text_of` 供测试用需 `pub(crate)` 或测试放同文件（上面测试用 `super::text_of`，同文件即可，保持私有）。

- [ ] **Step 2: 跑测试**

Run: `cargo test -p alephcore --lib moa::advisory_view`
Expected: 8 PASS。

- [ ] **Step 3: Commit**

```bash
git add src/providers/moa/ src/providers/mod.rs
git commit -m "providers: MoA advisory-view transform (flatten tool calls/results, end-on-user, utf8-safe truncation)"
```

---

### Task 4: Prompt 模板与指导注入

**Files:**
- Create: `src/providers/moa/prompts.rs`
- Modify: `src/providers/moa/mod.rs`（加 `pub(crate) mod prompts;`）
- Test: `prompts.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Consumes: `UnifiedMessage`/`ContentBlock`；Task 6 的 `AdvisorOutcome { label: String, text: String }`（本任务先在 prompts.rs 定义，Task 6 复用：`pub(crate) struct AdvisorOutcome`）
- Produces: `pub(crate) const ADVISOR_SYSTEM_PROMPT: &str`、`pub(crate) fn build_guidance(preset: &str, aggregator_label: &str, outcomes: &[AdvisorOutcome]) -> String`、`pub(crate) fn attach_guidance(messages: &mut Vec<UnifiedMessage>, guidance: &str)`

- [ ] **Step 1: 写实现 + 测试**

```rust
//! MoA prompt templates + guidance attachment.

use crate::providers::message::{ContentBlock, UnifiedMessage};

/// One advisor's consultation outcome (text or a labelled failure note).
#[derive(Clone, Debug)]
pub(crate) struct AdvisorOutcome {
    pub label: String,
    pub text: String,
}

/// System prompt for every advisor call. Ported from hermes
/// `_REFERENCE_SYSTEM_PROMPT`: without this framing a bare trimmed
/// conversation makes the advisor believe it is the acting agent — it then
/// refuses ("I can't access files") or hallucinates tool calls.
pub(crate) const ADVISOR_SYSTEM_PROMPT: &str =
    "You are an advisor in a Mixture of Agents (MoA) process. You are NOT \
     the acting agent and you do NOT execute anything: you cannot call \
     tools, run commands, browse, or access files, repositories, or URLs, \
     and you should not try to or apologize for being unable to. A separate \
     aggregator model holds those capabilities and will take the actual \
     actions.\n\n\
     The conversation below is the current state of a task handled by that \
     acting agent. Your job is to give your most intelligent analysis of \
     that state: understand the goal, reason about the problem, and advise \
     on what to do next. Surface the best approach, concrete next steps and \
     tool-use strategy, likely pitfalls and risks, and anything the acting \
     agent may have missed or gotten wrong. Assume any referenced files, \
     URLs, or systems exist and reason about them from the context given \
     rather than asking for access.\n\n\
     Respond with your advice directly — no preamble, no disclaimers about \
     tools or access. Your response is private guidance handed to the \
     aggregator, not an answer shown to the user.";

/// Build the guidance block injected at the END of the aggregator's prompt.
pub(crate) fn build_guidance(
    preset: &str,
    aggregator_label: &str,
    outcomes: &[AdvisorOutcome],
) -> String {
    let joined = outcomes
        .iter()
        .enumerate()
        .map(|(idx, o)| format!("Advisor {} — {}:\n{}", idx + 1, o.label, o.text))
        .collect::<Vec<_>>()
        .join("\n\n");
    let labels = outcomes
        .iter()
        .map(|o| o.label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[Mixture of Agents advisory context]\n\
         Preset: {preset}\n\
         Aggregator/acting model: {aggregator_label}\n\
         Advisors: {labels}\n\n\
         Use the advisor responses below as private context. You are the \
         aggregator and acting model: answer the user directly or call tools \
         as needed.\n\n\
         {joined}"
    )
}

/// Attach the guidance at the very END of the message list, so the
/// `[system][task][tool-history]` prefix stays byte-stable and KV-cache
/// reusable (hermes lesson: merging into an earlier user turn re-prefills
/// the whole conversation on every tool iteration). Merge into a trailing
/// user turn when present; otherwise append a new user turn.
pub(crate) fn attach_guidance(messages: &mut Vec<UnifiedMessage>, guidance: &str) {
    if let Some(UnifiedMessage::User { content }) = messages.last_mut() {
        if let Some(ContentBlock::Text { text, .. }) = content.last_mut() {
            text.push_str("\n\n");
            text.push_str(guidance);
            return;
        }
        content.push(ContentBlock::Text {
            text: guidance.to_string(),
            cache_control: None,
        });
        return;
    }
    messages.push(UnifiedMessage::user(guidance));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcomes() -> Vec<AdvisorOutcome> {
        vec![
            AdvisorOutcome { label: "openai:gpt-5.5".into(), text: "advice A".into() },
            AdvisorOutcome { label: "deepseek:v4".into(), text: "[failed: timeout]".into() },
        ]
    }

    #[test]
    fn guidance_lists_all_advisors_in_order() {
        let g = build_guidance("default", "anthropic:opus", &outcomes());
        let a = g.find("Advisor 1 — openai:gpt-5.5").unwrap();
        let b = g.find("Advisor 2 — deepseek:v4").unwrap();
        assert!(a < b);
        assert!(g.contains("advice A"));
        assert!(g.contains("[failed: timeout]"));
        assert!(g.contains("Preset: default"));
    }

    #[test]
    fn attach_merges_into_trailing_user_turn() {
        let mut msgs = vec![UnifiedMessage::user("original prompt")];
        attach_guidance(&mut msgs, "GUIDE");
        assert_eq!(msgs.len(), 1);
        let UnifiedMessage::User { content } = &msgs[0] else { panic!() };
        let ContentBlock::Text { text, .. } = &content[0] else { panic!() };
        assert!(text.starts_with("original prompt"));
        assert!(text.ends_with("GUIDE"));
    }

    #[test]
    fn attach_appends_after_trailing_assistant() {
        let mut msgs = vec![
            UnifiedMessage::user("q"),
            UnifiedMessage::assistant("a"),
        ];
        attach_guidance(&mut msgs, "GUIDE");
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs.last(), Some(UnifiedMessage::User { .. })));
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p alephcore --lib moa::prompts`
Expected: 3 PASS。

- [ ] **Step 3: Commit**

```bash
git add src/providers/moa/
git commit -m "providers: MoA advisor system prompt and cache-stable guidance attachment"
```

---

### Task 5: Trace 事件变体（LoopTraceEvent + 协议镜像 + 白名单）

**Files:**
- Modify: `src/harness/trace.rs`（`LoopTraceEvent` 加 4 变体 + `From` 转换 4 个 arm；R10 内唯一 harness 触点）
- Modify: `shared/protocol/src/events.rs`（`AgentTraceEvent` 加 4 变体 + `kind()` 4 个 arm + presentation fn）
- Modify: `src/gateway/execution_engine/agent_trace_emit_sink.rs`（`is_step_event` 白名单加 3 个轻量变体）
- Test: `trace.rs` 现有测试模块（若有）+ 编译验证

**Interfaces:**
- Consumes: 现有 `LoopTraceEvent`（`#[serde(tag = "type", rename_all = "snake_case")]`、`#[non_exhaustive]`）、`aleph_protocol::AgentTraceEvent`（`#[serde(tag = "kind", ...)]`）
- Produces: `LoopTraceEvent::{MoaAdvisor, MoaAggregating, MoaAdvisorSpend, MoaTurnTrace}` 及协议镜像；wire kind 字符串 `"moa_advisor"` / `"moa_aggregating"` / `"moa_advisor_spend"`（Task 9 panel 消费）；`MoaTurnTrace` **不进白名单**（只落库）。

- [ ] **Step 1: LoopTraceEvent 加变体**（`src/harness/trace.rs`，`VerifierVeto` 变体之后，enum 关闭花括号之前）：

```rust
    /// MoA advisor consultation result — one per advisor per fan-out
    /// (cache-MISS iterations only). Emitted by the `MoaProvider` facade
    /// through the run's TraceSink (MeteringProvider pattern — zero harness
    /// logic; this enum is the carrier, not the brain).
    MoaAdvisor {
        index: usize,
        count: usize,
        /// `provider:model` of the advisor slot.
        label: String,
        text: String,
    },
    /// MoA fan-out complete; the aggregator (acting model) is being called.
    MoaAggregating {
        aggregator: String,
        advisor_count: usize,
    },
    /// Summed advisor spend for one fan-out. Priced per-advisor at each
    /// advisor's OWN model rate; kept out of `ProviderResponse.usage` so the
    /// context gauge stays honest (spec §8).
    MoaAdvisorSpend {
        advisor_count: usize,
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: Option<f64>,
    },
    /// Full advisor I/O snapshot for one fan-out. Heavy; emitted only when
    /// `[moa] save_traces = true`; persisted-only (never whitelisted onto
    /// the wire). Opaque JSON keeps the payload shape free to evolve.
    MoaTurnTrace { preset: String, payload: Value },
```

- [ ] **Step 2: 协议镜像**（`shared/protocol/src/events.rs` `AgentTraceEvent`，`VerifierVeto` 后加同形 4 变体——`MoaTurnTrace` 的 `payload: Value`；`kind()` 加 4 个 arm：`"moa_advisor"` / `"moa_aggregating"` / `"moa_advisor_spend"` / `"moa_turn_trace"`）。再处理 presentation：`grep -n "present_agent_trace_event" shared/protocol/src/*.rs` 找到 `present_agent_trace_event_with_labels_and_preset` 的 match，为 4 个新变体加 arm（标签形如 `format!("Advisor {index}/{count} — {label}")`、`format!("MoA aggregating ({aggregator})")`、`format!("MoA advisors spent {input_tokens}+{output_tokens} tok")`、`"MoA turn trace"`；照相邻 arm 的返回类型构造）。

- [ ] **Step 3: From 转换**（`src/harness/trace.rs` `impl From<LoopTraceEvent> for aleph_protocol::AgentTraceEvent` 的 match，`VerifierVeto` arm 后）：

```rust
            LoopTraceEvent::MoaAdvisor { index, count, label, text } => {
                Self::MoaAdvisor { index, count, label, text }
            }
            LoopTraceEvent::MoaAggregating { aggregator, advisor_count } => {
                Self::MoaAggregating { aggregator, advisor_count }
            }
            LoopTraceEvent::MoaAdvisorSpend { advisor_count, input_tokens, output_tokens, cost_usd } => {
                Self::MoaAdvisorSpend { advisor_count, input_tokens, output_tokens, cost_usd }
            }
            LoopTraceEvent::MoaTurnTrace { preset, payload } => {
                Self::MoaTurnTrace { preset, payload }
            }
```

- [ ] **Step 4: 白名单**（`agent_trace_emit_sink.rs` `is_step_event` 的 `matches!` 里加，**不含 MoaTurnTrace**）：

```rust
            | LoopTraceEvent::MoaAdvisor { .. }
            | LoopTraceEvent::MoaAggregating { .. }
            | LoopTraceEvent::MoaAdvisorSpend { .. }
```

- [ ] **Step 5: 编译验证**

Run: `cargo check -p alephcore --lib`
Expected: 编译通过（protocol crate 作为依赖一并检查）。若 protocol 是独立 workspace member 且 check 未覆盖，跑 `cargo check -p aleph_protocol`（先 `grep '^name' shared/protocol/Cargo.toml` 确认包名）。

- [ ] **Step 6: Commit**

```bash
git add src/harness/trace.rs shared/protocol/src/events.rs src/gateway/execution_engine/agent_trace_emit_sink.rs
git commit -m "trace: MoA advisor/aggregating/spend/turn-trace event variants with wire whitelist"
```

---

### Task 6: `MoaProvider` 门面（fan-out + 缓存 + 事件 + 身份委托）

**Files:**
- Create: `src/providers/moa/provider.rs`
- Create: `src/providers/moa/config_handle.rs`
- Modify: `src/providers/moa/mod.rs`（模块声明 + `resolve` 助手 + re-export）
- Test: `provider.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 1 `MoaToml/MoaPreset/MoaFanout`、Task 2 `SessionMoaPref`、Task 3 `build_advisory_view/view_signature`、Task 4 `ADVISOR_SYSTEM_PROMPT/build_guidance/attach_guidance/AdvisorOutcome`、Task 5 trace 变体；`ModelOverrideProvider`、`MeteringProvider`、`TraceSink`、`pricing::estimate`、`TokenBreakdown`（`crate::orchestrator::dispatch::TokenBreakdown`）
- Produces:
  - `pub struct MoaProvider`（`impl AiProvider`）
  - `pub fn try_build_for_run(pref: &SessionMoaPref, moa_cfg: Option<&MoaToml>, named: &HashMap<String, Arc<dyn AiProvider>>, sink: Option<Arc<dyn TraceSink>>) -> Result<MoaProvider, String>`（Task 7 runner 调用）
  - `pub fn store_moa_config(cfg: Option<MoaToml>)` / `pub fn get_moa_config() -> Option<MoaToml>`（config_handle；Task 7 boot 写入、Task 8 工具热更新）
  - `pub(crate) fn parse_one_shot_command(input: &str) -> Option<&str>`（Task 8 拦截用：`/moa <prompt>` → `Some(prompt)`；裸 `/moa` 或非 moa 输入 → `None`）

- [ ] **Step 1: config_handle.rs**（镜像 route_handle 的全局热载模式）：

```rust
//! Process-global live handle for the `[moa]` config section.
//!
//! Mirrors `route_handle`: written at boot from the loaded Config, re-stored
//! by the `moa` tool after a successful preset patch (hot reload), read at
//! run construction. Avoids threading a Config handle through
//! `AgentHarnessRunner` (which holds only boot-time snapshots by design).

use crate::config::types::moa::MoaToml;
use crate::sync_primitives::RwLock;
use std::sync::OnceLock;

static MOA_CONFIG: OnceLock<RwLock<Option<MoaToml>>> = OnceLock::new();

fn cell() -> &'static RwLock<Option<MoaToml>> {
    MOA_CONFIG.get_or_init(|| RwLock::new(None))
}

/// Publish the current `[moa]` section (boot + after config patches).
pub fn store_moa_config(cfg: Option<MoaToml>) {
    *cell().write().unwrap_or_else(|e| e.into_inner()) = cfg;
}

/// Snapshot of the current `[moa]` section.
#[must_use]
pub fn get_moa_config() -> Option<MoaToml> {
    cell().read().unwrap_or_else(|e| e.into_inner()).clone()
}
```

- [ ] **Step 2: provider.rs — 结构体与解析**

```rust
//! `MoaProvider` — the AiProvider facade that runs the MoA turn shape:
//! flatten conversation → parallel advisor fan-out (per-advisor timeout,
//! fail-soft) → inject guidance at prompt tail → call the aggregator, which
//! IS the acting model. The harness sees one provider (R10).

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::json;

use crate::config::types::moa::{MoaFanout, MoaPreset, MoaToml};
use crate::error::Result;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::providers::adapter::{ProviderResponse, RequestPayload, TokenUsage};
use crate::providers::message::UnifiedMessage;
use crate::providers::session_moa_handle::SessionMoaPref;
use crate::providers::{AiProvider, MeteringProvider, ModelOverrideProvider};
use crate::sync_primitives::{Arc, Mutex};

use super::advisory_view::{build_advisory_view, view_signature};
use super::prompts::{attach_guidance, build_guidance, AdvisorOutcome, ADVISOR_SYSTEM_PROMPT};

/// One resolved advisor: label + provider chain + identity for pricing.
pub(crate) struct AdvisorSlot {
    label: String,
    provider_key: String,
    model: String,
    chain: Arc<dyn AiProvider>,
}

struct AdvisorCache {
    signature: u64,
    outcomes: Vec<AdvisorOutcome>,
}

pub struct MoaProvider {
    display_name: String,
    preset_name: String,
    advisors: Vec<AdvisorSlot>,
    aggregator: Arc<dyn AiProvider>,
    aggregator_label: String,
    fanout: MoaFanout,
    advisor_timeout: Duration,
    advisor_max_tokens: Option<u32>,
    advisor_temperature: Option<f32>,
    aggregator_temperature: Option<f32>,
    save_traces: bool,
    sink: Option<Arc<dyn TraceSink>>,
    cache: Mutex<Option<AdvisorCache>>,
}
```

解析助手（`mod.rs` 或 provider.rs 内，`pub fn try_build_for_run`）：

```rust
/// Build a run-scoped `MoaProvider` from the session pref + live config.
/// Errors are human-readable reasons — the runner logs and falls back to
/// the normal provider chain (fail-soft; the conversation never breaks).
pub fn try_build_for_run(
    pref: &SessionMoaPref,
    moa_cfg: Option<&MoaToml>,
    named: &HashMap<String, Arc<dyn AiProvider>>,
    sink: Option<Arc<dyn TraceSink>>,
) -> std::result::Result<MoaProvider, String> {
    let cfg = moa_cfg.ok_or("no [moa] section configured")?;
    let (preset_name, preset) = cfg
        .resolve_preset(pref.preset.as_deref())
        .ok_or_else(|| {
            format!(
                "MoA preset '{}' not found (configure [moa.presets.*] or ask me to set one up)",
                pref.preset.as_deref().unwrap_or("<default>")
            )
        })?;
    let errs = cfg.validation_errors();
    if !errs.is_empty() {
        return Err(format!("[moa] config invalid: {}", errs.join("; ")));
    }

    let resolve_slot = |slot: &crate::config::types::moa::MoaSlot| {
        // Runtime recursion guard (layer 3) — config validation already
        // rejects this, but presets can arrive through raw TOML edits.
        if slot.provider.trim().eq_ignore_ascii_case("moa") {
            return Err(format!("slot {}:{} is recursive", slot.provider, slot.model));
        }
        let base = named.get(&slot.provider).cloned().ok_or_else(|| {
            format!("provider '{}' is not configured/keyed", slot.provider)
        })?;
        Ok(Arc::new(ModelOverrideProvider::new(base, slot.model.clone()))
            as Arc<dyn AiProvider>)
    };

    let mut advisors = Vec::new();
    if preset.enabled {
        for (idx, slot) in preset.advisors.iter().enumerate() {
            let chain = resolve_slot(slot)?;
            let label = format!("{}:{}", slot.provider, slot.model);
            // Per-advisor metering: usage lands as ProviderUsage events
            // labelled "moa:<i>:<provider>:<model>", priced per advisor.
            let metered = Arc::new(MeteringProvider::new(
                chain,
                sink.clone(),
                format!("moa:{idx}:{label}"),
            )) as Arc<dyn AiProvider>;
            advisors.push(AdvisorSlot {
                label,
                provider_key: slot.provider.clone(),
                model: slot.model.clone(),
                chain: metered,
            });
        }
    }
    let aggregator = resolve_slot(&preset.aggregator)?;
    let aggregator_label = format!(
        "{}:{}",
        preset.aggregator.provider, preset.aggregator.model
    );

    Ok(MoaProvider {
        display_name: format!("moa:{preset_name}"),
        preset_name,
        advisors,
        aggregator,
        aggregator_label,
        fanout: preset.fanout,
        advisor_timeout: Duration::from_secs(preset.advisor_timeout_secs.max(1)),
        advisor_max_tokens: preset.advisor_max_tokens,
        advisor_temperature: preset.advisor_temperature,
        aggregator_temperature: preset.aggregator_temperature,
        save_traces: cfg.save_traces,
        sink,
        cache: Mutex::new(None),
    })
}
```

- [ ] **Step 3: `impl AiProvider for MoaProvider`**（身份委托聚合器 + `process` 主流程）：

```rust
impl MoaProvider {
    fn emit(&self, event: LoopTraceEvent) {
        if let Some(sink) = &self.sink {
            sink.on_trace(&event);
        }
    }

    /// Sum advisor usages + per-advisor own-rate pricing for the spend event.
    fn spend_event(&self, usages: &[(usize, TokenUsage)]) -> LoopTraceEvent {
        let mut input = 0u32;
        let mut output = 0u32;
        let mut cost: Option<f64> = None;
        for (idx, usage) in usages {
            input = input.saturating_add(usage.input_tokens);
            output = output.saturating_add(usage.output_tokens);
            let slot = &self.advisors[*idx];
            let breakdown = crate::orchestrator::dispatch::TokenBreakdown {
                input: u64::from(usage.input_tokens),
                output: u64::from(usage.output_tokens),
                cache_read: u64::from(usage.cache_read_tokens.unwrap_or(0)),
                cache_creation: u64::from(usage.cache_creation_tokens.unwrap_or(0)),
                reasoning: u64::from(usage.thinking_tokens.unwrap_or(0)),
            };
            let est = crate::pricing::estimate(&slot.provider_key, &slot.model, &breakdown);
            // CostEstimate field access: verify against src/pricing.rs
            // (grep "pub struct CostEstimate"); expected `usd: Option<f64>`
            // or `usd: f64` — fold into `cost` accordingly.
            if let Some(usd) = est.usd {
                cost = Some(cost.unwrap_or(0.0) + usd);
            }
        }
        LoopTraceEvent::MoaAdvisorSpend {
            advisor_count: usages.len(),
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
        }
    }
}

impl AiProvider for MoaProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // Own the borrowed payload fields (FailoverProvider pattern) so the
        // async block can rebuild sub-request payloads freely.
        let messages: Vec<UnifiedMessage> = payload.messages.to_vec();
        let system_prompt = payload.system_prompt.map(str::to_string);
        let system_blocks = payload.system_blocks.map(<[_]>::to_vec);
        let tools = payload.tools.map(<[_]>::to_vec);
        let think_level = payload.think_level;
        let caller_temperature = payload.temperature;
        let max_tokens = payload.max_tokens;
        let tool_choice = payload.tool_choice.clone();
        let metadata = payload.metadata.clone();

        Box::pin(async move {
            // 1. Advisory view + signature.
            let view = build_advisory_view(&messages);
            let sig = view_signature(&view);

            // 2. Cache decision (per_iteration: same-signature repeat calls
            //    — harness internal retries — are HITs; user_turn: any
            //    existing cache is a HIT for the rest of this run).
            let cached: Option<Vec<AdvisorOutcome>> = {
                let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                guard.as_ref().and_then(|c| match self.fanout {
                    MoaFanout::UserTurn => Some(c.outcomes.clone()),
                    MoaFanout::PerIteration => {
                        (c.signature == sig).then(|| c.outcomes.clone())
                    }
                })
            };

            let outcomes: Vec<AdvisorOutcome> = if let Some(hit) = cached {
                hit
            } else if self.advisors.is_empty() {
                Vec::new()
            } else {
                // 3. Parallel fan-out, per-advisor timeout, fail-soft.
                let timeout = self.advisor_timeout;
                let futures = self.advisors.iter().map(|slot| {
                    let view = &view;
                    async move {
                        let advisor_payload = RequestPayload::new(view)
                            .with_system(Some(ADVISOR_SYSTEM_PROMPT))
                            .with_temperature(self.advisor_temperature)
                            .with_max_tokens(self.advisor_max_tokens);
                        match tokio::time::timeout(timeout, slot.chain.process(advisor_payload))
                            .await
                        {
                            Ok(Ok(resp)) => {
                                let text = resp
                                    .text
                                    .clone()
                                    .filter(|t| !t.trim().is_empty())
                                    .unwrap_or_else(|| "(empty response)".to_string());
                                (text, resp.usage, None::<String>)
                            }
                            Ok(Err(e)) => (format!("[failed: {e}]"), None, Some(e.to_string())),
                            Err(_) => (
                                format!("[timeout after {}s]", timeout.as_secs()),
                                None,
                                Some("timeout".to_string()),
                            ),
                        }
                    }
                });
                let results = futures::future::join_all(futures).await;

                let mut outcomes = Vec::with_capacity(results.len());
                let mut usages: Vec<(usize, TokenUsage)> = Vec::new();
                for (idx, (text, usage, _err)) in results.into_iter().enumerate() {
                    if let Some(u) = usage {
                        usages.push((idx, u));
                    }
                    outcomes.push(AdvisorOutcome {
                        label: self.advisors[idx].label.clone(),
                        text,
                    });
                }

                // 4. Display + accounting + heavy trace events (MISS only).
                let count = outcomes.len();
                for (idx, o) in outcomes.iter().enumerate() {
                    self.emit(LoopTraceEvent::MoaAdvisor {
                        index: idx + 1,
                        count,
                        label: o.label.clone(),
                        text: o.text.clone(),
                    });
                }
                self.emit(LoopTraceEvent::MoaAggregating {
                    aggregator: self.aggregator_label.clone(),
                    advisor_count: count,
                });
                if !usages.is_empty() {
                    let spend = self.spend_event(&usages);
                    self.emit(spend);
                }
                if self.save_traces {
                    self.emit(LoopTraceEvent::MoaTurnTrace {
                        preset: self.preset_name.clone(),
                        payload: json!({
                            "aggregator": self.aggregator_label,
                            "view_signature": sig,
                            "advisors": outcomes
                                .iter()
                                .map(|o| json!({ "label": o.label, "output": o.text }))
                                .collect::<Vec<_>>(),
                        }),
                    });
                }

                *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(AdvisorCache {
                    signature: sig,
                    outcomes: outcomes.clone(),
                });
                outcomes
            };

            // 5. Guidance injection at the prompt tail (cache-stable prefix).
            let mut agg_messages = messages;
            if !outcomes.is_empty() {
                let guidance =
                    build_guidance(&self.preset_name, &self.aggregator_label, &outcomes);
                attach_guidance(&mut agg_messages, &guidance);
            }

            // 6. Aggregator = acting model: full payload passthrough. Its
            //    ProviderResponse (tool_calls/thinking/usage) returns as-is —
            //    advisor usage is deliberately NOT merged in (gauge honesty).
            let agg_payload = RequestPayload::new(&agg_messages)
                .with_system(system_prompt.as_deref())
                .with_system_blocks(system_blocks.as_deref())
                .with_tools(tools.as_deref())
                .with_think_level(think_level)
                .with_temperature(self.aggregator_temperature.or(caller_temperature))
                .with_max_tokens(max_tokens)
                .with_tool_choice(tool_choice)
                .with_metadata(metadata);
            self.aggregator.process(agg_payload).await
        })
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn color(&self) -> &str {
        "#8b5cf6"
    }

    // Identity surfaces all delegate to the aggregator — it IS the acting
    // model: prompt behavior family, tool extraction, gauge window, pricing.
    fn supports_native_tools(&self) -> bool {
        self.aggregator.supports_native_tools()
    }

    fn protocol(&self) -> Cow<'_, str> {
        self.aggregator.protocol()
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.aggregator.model_behavior_override()
    }

    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        self.aggregator.behavior_hint()
    }

    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        self.aggregator.serving_model_hint()
    }

    // as_http_provider stays the default `None` — forwarding the aggregator's
    // HttpProvider would let think.rs stream AROUND the facade and advisors
    // would never run. The production failover path is `None` today anyway.
}
```

`mod.rs` 最终形态：

```rust
pub(crate) mod advisory_view;
pub mod config_handle;
pub(crate) mod prompts;
pub mod provider;

pub use config_handle::{get_moa_config, store_moa_config};
pub use provider::{try_build_for_run, MoaProvider};

/// Parse a `/moa <prompt>` one-shot command. The argument is ALWAYS a
/// prompt, never a preset name (hermes-pinned semantics). Bare `/moa`
/// returns `None` (falls through to the LLM → `moa` tool).
#[must_use]
pub fn parse_one_shot_command(input: &str) -> Option<&str> {
    let rest = input.trim().strip_prefix("/moa")?;
    let rest = rest.strip_prefix(char::is_whitespace)?.trim();
    (!rest.is_empty()).then_some(rest)
}
```

- [ ] **Step 4: 测试**（provider.rs `#[cfg(test)]`；需要本地计数/慢速/工具调用 mock——`MockProvider` 只有固定单响应，故写局部 stub）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::moa::MoaSlot;
    use crate::providers::mock::MockProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counting stub: fixed text, optional delay, call counter.
    struct CountingProvider {
        text: String,
        delay: Option<Duration>,
        calls: Arc<AtomicUsize>,
    }
    impl AiProvider for CountingProvider {
        fn process<'a>(
            &'a self,
            _p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let text = self.text.clone();
            let delay = self.delay;
            Box::pin(async move {
                if let Some(d) = delay {
                    tokio::time::sleep(d).await;
                }
                Ok(ProviderResponse::text_only(text))
            })
        }
        fn name(&self) -> &str { "counting" }
        fn color(&self) -> &str { "#000" }
    }

    fn make_provider(
        advisors: Vec<(Arc<dyn AiProvider>, &str)>,
        aggregator: Arc<dyn AiProvider>,
        fanout: MoaFanout,
        timeout_secs: u64,
    ) -> MoaProvider {
        MoaProvider {
            display_name: "moa:test".into(),
            preset_name: "test".into(),
            advisors: advisors
                .into_iter()
                .enumerate()
                .map(|(i, (chain, label))| AdvisorSlot {
                    label: label.to_string(),
                    provider_key: "mock".into(),
                    model: format!("m{i}"),
                    chain,
                })
                .collect(),
            aggregator,
            aggregator_label: "mock:agg".into(),
            fanout,
            advisor_timeout: Duration::from_secs(timeout_secs),
            advisor_max_tokens: None,
            advisor_temperature: None,
            aggregator_temperature: None,
            save_traces: false,
            sink: None,
            cache: Mutex::new(None),
        }
    }

    fn user_msgs(text: &str) -> Vec<UnifiedMessage> {
        vec![UnifiedMessage::user(text)]
    }

    #[tokio::test]
    async fn advisors_run_in_parallel_and_aggregator_answers() {
        let start = std::time::Instant::now();
        let calls = Arc::new(AtomicUsize::new(0));
        let slow = |t: &str| -> Arc<dyn AiProvider> {
            Arc::new(CountingProvider {
                text: t.into(),
                delay: Some(Duration::from_millis(150)),
                calls: calls.clone(),
            })
        };
        let p = make_provider(
            vec![(slow("advice-1"), "a:1"), (slow("advice-2"), "a:2")],
            Arc::new(MockProvider::new("final answer")),
            MoaFanout::PerIteration,
            30,
        );
        let msgs = user_msgs("go");
        let resp = p.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "final answer");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Parallel: two 150ms advisors must not take 300ms serially.
        assert!(start.elapsed() < Duration::from_millis(280));
    }

    #[tokio::test]
    async fn advisor_failure_and_timeout_degrade_to_notes() {
        use crate::providers::mock::MockError;
        let failing: Arc<dyn AiProvider> =
            Arc::new(MockProvider::new("x").with_error(MockError::Network("down".into())));
        let sleepy: Arc<dyn AiProvider> = Arc::new(
            MockProvider::new("late").with_delay(Duration::from_secs(5)),
        );
        // Aggregator records what it saw via the guidance in its messages —
        // use a capturing stub.
        struct Capture(Arc<Mutex<String>>);
        impl AiProvider for Capture {
            fn process<'a>(
                &'a self,
                p: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                let joined = p
                    .messages
                    .iter()
                    .flat_map(UnifiedMessage::content_blocks)
                    .filter_map(|b| match b {
                        crate::providers::message::ContentBlock::Text { text, .. } => {
                            Some(text.clone())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                *self.0.lock().unwrap_or_else(|e| e.into_inner()) = joined;
                Box::pin(async { Ok(ProviderResponse::text_only("ok".into())) })
            }
            fn name(&self) -> &str { "capture" }
            fn color(&self) -> &str { "#000" }
        }
        let seen = Arc::new(Mutex::new(String::new()));
        let p = make_provider(
            vec![(failing, "f:1"), (sleepy, "s:2")],
            Arc::new(Capture(seen.clone())),
            MoaFanout::PerIteration,
            1, // 1s timeout < 5s delay
        );
        let msgs = user_msgs("go");
        let resp = p.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "ok");
        let guidance = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(guidance.contains("[failed:"));
        assert!(guidance.contains("[timeout after 1s]"));
        // Order stable: advisor 1 note appears before advisor 2 note.
        assert!(guidance.find("f:1").unwrap() < guidance.find("s:2").unwrap());
    }

    #[tokio::test]
    async fn per_iteration_cache_dedupes_identical_state() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counting: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            text: "advice".into(),
            delay: None,
            calls: calls.clone(),
        });
        let p = make_provider(
            vec![(counting, "c:1")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::PerIteration,
            30,
        );
        let msgs = user_msgs("same state");
        p.process(RequestPayload::new(&msgs)).await.unwrap();
        p.process(RequestPayload::new(&msgs)).await.unwrap(); // identical → HIT
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Changed state → MISS.
        let msgs2 = vec![
            UnifiedMessage::user("same state"),
            UnifiedMessage::assistant("did something"),
        ];
        p.process(RequestPayload::new(&msgs2)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn user_turn_cache_survives_state_growth() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counting: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            text: "advice".into(),
            delay: None,
            calls: calls.clone(),
        });
        let p = make_provider(
            vec![(counting, "c:1")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::UserTurn,
            30,
        );
        let msgs = user_msgs("go");
        p.process(RequestPayload::new(&msgs)).await.unwrap();
        let grown = vec![UnifiedMessage::user("go"), UnifiedMessage::assistant("step")];
        p.process(RequestPayload::new(&grown)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1); // run-scoped: once only
    }

    #[tokio::test]
    async fn no_advisors_means_bare_aggregator() {
        let p = make_provider(
            vec![],
            Arc::new(MockProvider::new("solo")),
            MoaFanout::PerIteration,
            30,
        );
        let msgs = user_msgs("go");
        let resp = p.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "solo");
    }

    #[test]
    fn identity_delegates_to_aggregator_and_no_http_downcast() {
        let p = make_provider(
            vec![],
            Arc::new(ModelOverrideProvider::new(
                Arc::new(MockProvider::new("x")),
                "agg-model",
            )),
            MoaFanout::PerIteration,
            30,
        );
        assert_eq!(p.serving_model_hint().unwrap(), "agg-model");
        assert_eq!(p.name(), "moa:test");
        assert!(p.as_http_provider().is_none());
    }

    #[test]
    fn parse_one_shot_command_semantics() {
        use super::super::parse_one_shot_command;
        assert_eq!(parse_one_shot_command("/moa write a poem"), Some("write a poem"));
        // Arg equal to a preset name is STILL a prompt (hermes-pinned).
        assert_eq!(parse_one_shot_command("/moa default"), Some("default"));
        assert_eq!(parse_one_shot_command("/moa"), None);
        assert_eq!(parse_one_shot_command("/moa   "), None);
        assert_eq!(parse_one_shot_command("hello"), None);
        assert_eq!(parse_one_shot_command("/moab x"), None);
    }

    #[test]
    fn try_build_for_run_errors() {
        use crate::providers::session_moa_handle::SessionMoaPref;
        let named: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
        let pref = SessionMoaPref { preset: None, one_shot: false };
        // No config at all.
        assert!(try_build_for_run(&pref, None, &named, None).is_err());
        // Preset references an unconfigured provider.
        let cfg: MoaToml = toml::from_str(
            r#"
[presets.p]
advisors = [{ provider = "ghost", model = "m" }]
aggregator = { provider = "ghost", model = "n" }
"#,
        )
        .unwrap();
        let err = try_build_for_run(
            &SessionMoaPref { preset: Some("p".into()), one_shot: false },
            Some(&cfg),
            &named,
            None,
        )
        .unwrap_err();
        assert!(err.contains("ghost"));
    }
}
```

注：`MoaSlot` 未用则删 import；`AdvisorSlot` 字段测试直接构造需要 `pub(crate)` 或测试在同文件（同文件即可）。`spend_event` 里 `CostEstimate` 字段先 `grep -n "pub struct CostEstimate" src/pricing.rs` 核对再落笔。

- [ ] **Step 5: 跑测试**

Run: `cargo test -p alephcore --lib moa::provider`
Expected: 8 PASS（并行性断言若在慢 CI 上抖动，放宽到 <280ms 已留余量）。

- [ ] **Step 6: Commit**

```bash
git add src/providers/moa/
git commit -m "providers: MoaProvider facade — parallel advisor fan-out with timeout, signature cache, aggregator passthrough"
```

---

### Task 7: Runner 接线（Step 3 激活 + gauge 修正 + boot 发布配置）

**Files:**
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs`（Step 3，~`:239-256` 与 gauge_model ~`:323`）
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs`（boot 发布 `[moa]` 到全局 handle）

**Interfaces:**
- Consumes: Task 2 `take_for_run`、Task 6 `try_build_for_run/get_moa_config/store_moa_config`
- Produces: 运行时激活链——`session_moa_handle` 有值的 run 其 `deps.llm = Metering("root", MoaProvider)`；优先级 **MoA > select_model pick > agent pin > brain**（见 Global Constraints 的 spec #6 修订）。

- [ ] **Step 1: Step 3 插入**（`runner_impl.rs`，锚点：`let llm = match model_directive {` 块结束后、`// Stage J-pre: wrap the root provider with MeteringProvider` 注释之前）：

```rust
        // Step 3-MoA: a session MoA activation supersedes the directive/brain
        // pick — the MoaProvider facade fans advisors out and lets the
        // preset's aggregator act. `take_for_run` consumes a one-shot pref
        // atomically (the single restore point: success, error and cancel
        // paths all leave no state). Fail-soft: an unusable preset logs and
        // falls back to the normal chain — the conversation never breaks.
        // Spec: docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md
        let mut moa_active = false;
        let llm: Arc<dyn crate::providers::AiProvider> =
            match crate::providers::session_moa_handle::take_for_run(&session_pref_key) {
                Some(pref) => {
                    let moa_cfg = crate::providers::moa::get_moa_config();
                    match crate::providers::moa::try_build_for_run(
                        &pref,
                        moa_cfg.as_ref(),
                        &self.named_providers,
                        trace_sink.clone(),
                    ) {
                        Ok(moa) => {
                            moa_active = true;
                            Arc::new(moa)
                        }
                        Err(reason) => {
                            tracing::warn!(
                                reason = %reason,
                                "MoA activation unusable; run proceeds on the normal provider chain"
                            );
                            llm
                        }
                    }
                }
                None => llm,
            };
```

- [ ] **Step 2: gauge 修正**（同文件 `let gauge_model: String = if routing_model_id == "(dynamic)" {` 处）：当 MoA 激活时 select_model/agent-pin 折出的 `routing_model_id` 不再是实际执行模型，改为：

```rust
        let gauge_model: String = if moa_active || routing_model_id == "(dynamic)" {
            llm.serving_model_hint()
                .map_or_else(|| provider_name.clone(), std::borrow::Cow::into_owned)
        } else {
            routing_model_id.clone()
        };
```

（`serving_model_hint` 经 Metering→MoaProvider→聚合器的 ModelOverride 链返回聚合器模型——gauge 分母与 run 定价自动正确。）

- [ ] **Step 3: boot 发布**（`orchestrator_init.rs`，`let harness = Arc::new(AgentHarnessRunner {` 之前）：

```rust
    // MoA: publish the [moa] section to the process-global handle so run
    // construction (runner_impl Step 3-MoA) and the `moa` tool read live
    // presets. The tool re-stores after successful config patches (hot
    // reload, mirroring route_handle).
    alephcore::providers::moa::store_moa_config(config.moa.clone());
    if let Some(moa) = &config.moa {
        for err in moa.validation_errors() {
            tracing::warn!(error = %err, "[moa] config validation");
        }
    }
```

- [ ] **Step 4: 编译验证**

Run: `cargo check -p alephcore --lib`（runner 在 lib 内；orchestrator_init 在 bin——若 bin 未覆盖，本任务允许一次 `cargo check --bin aleph-server`）
Expected: 通过。运行时正确性由 Task 6 单测 + 终验兜底（runner_impl 无独立单测先例，遵循 surgical changes）。

- [ ] **Step 5: Commit**

```bash
git add src/orchestrator/harness_bridge/runner_impl.rs src/bin/aleph-server/commands/start/orchestrator_init.rs
git commit -m "orchestrator: activate MoaProvider at run construction with atomic one-shot consume and gauge fix"
```

---

### Task 8: `moa` 工具 + `/moa` 命令面 + one-shot 拦截

**Files:**
- Create: `src/builtin_tools/moa_manage.rs`
- Modify: `src/builtin_tools/mod.rs`（`pub mod moa_manage;` + `pub use moa_manage::{MoaManageArgs, MoaManageOutput, MoaManageTool};`）
- Modify: `src/executor/builtin_registry/definitions.rs`（静态目录条目 + `create_tool_boxed` → `None` 注释 arm）
- Modify: builder（`grep -rn "SelfConfigTool::new" src/executor/ src/bin/` 找到构造点，旁边同款构造 `MoaManageTool` 并注入 config/patcher；`grep -rn "\"self_config\"" src/executor/builtin_registry/` 找 metadata 注册/执行分发点镜像）
- Modify: `src/gateway/execution_engine/slash_command.rs`（fast-path 排除）
- Modify: one-shot 拦截点（`grep -rn "try_resolve_slash_command(" src/gateway/execution_engine/` 找调用处；`grep -rn "execute_slash_command_fast_path" src/gateway/execution_engine/` 找 direct_tool 分发处）
- Test: `moa_manage.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 1/2/6 全部 pub API、`AlephTool` trait、`ConfigPatcher`（`PatchRequest { path, patch, health_check, dry_run }` → `apply()`）、`current_turn_context()`、`notify_tool_start/notify_tool_result`
- Produces: `moa` 工具（action 枚举 `#[serde(tag = "action", rename_all = "snake_case")]`：`On { preset: Option<String> }` / `Off` / `Once { preset: Option<String> }` / `Status` / `List` / `SetPreset { name, advisors: Vec<MoaSlot>, aggregator: MoaSlot, fanout: Option<MoaFanout>, advisor_timeout_secs: Option<u64>, advisor_max_tokens: Option<u32>, advisor_temperature: Option<f32>, aggregator_temperature: Option<f32>, set_default: Option<bool> }` / `DeletePreset { name: String }`）；输出 `MoaManageOutput { success: bool, message: String, data: Option<serde_json::Value> }`。

- [ ] **Step 1: 工具实现**。结构照 `SelfConfigTool`（`config: Option<Arc<RwLock<Config>>>` + `config_patcher: Option<Arc<ConfigPatcher>>`，builder 式 `with_config`/`with_patcher`）。行为要点：
  - `On/Once/Off/Status`：经 `current_turn_context()` 取 session key（无 turn context → `success:false` 优雅降级，照 `select_model`）；`On`/`Once` 先用 `get_moa_config()` 验证 preset 可解析（`resolve_preset`），不可解析时 `success:false` 且 message 给指引（“no [moa] presets configured — use action='set_preset' to create one; use list_models to discover available models”）；可解析则 `set_session_moa(key, preset, one_shot)`，message 说明生效时机（`On`: "MoA '<name>' active for this session from the NEXT turn"；`Once`: "…for the next turn only"）。
  - `Status`：`get_session_moa` + 生效 preset 的 advisors/aggregator/fanout 概览。
  - `List`：`get_moa_config()` 全 preset 概览，default 标 `*`。
  - `SetPreset`：构造 `MoaPreset`，先跑 `MoaToml::validation_errors()` 级校验（拒 recursive slot——防护第 2 层）；patch `PatchRequest { path: "moa", patch: json!({"presets": {name: preset_json}, ...maybe default_preset}), health_check: false, dry_run: false }` → `patcher.apply()`；成功后热更新：`store_moa_config(self.config.as_ref()?.read().await.moa.clone())`（镜像 self_config 的 route hot-apply，self_config.rs:403-419）。
  - `DeletePreset`：守护——拒删最后一个 preset；被删者是 `default_preset` 则顺延到任一剩余者（patch 一并更新）；session 状态指向被删者的（无法逐 session 遍历——进程 map 无枚举需求，跳过，运行时 fail-soft 已兜底）。删除经 patch `{"presets": {name: null}}`；**先** `grep -n "fn deep_merge" src/config/patcher.rs` 读 null 语义：若 deep-merge 不支持 null 删键，则在 patcher 的 deep_merge 中加"显式 null = 删除键"分支（小改 + 在 patcher 现有测试模块补一个 null-delete 测试）；成功后同样热更新 handle。
  - 每个 action 开头 `notify_tool_start`、结尾 `notify_tool_result`（照 self_config 的 match 模式）。
  - `DESCRIPTION`（英文，给 LLM）: "Manage Mixture-of-Agents (MoA) advisory mode for this session. MoA consults several advisor models in parallel on the live conversation before each step and hands their private guidance to the acting aggregator model. action='on' activates a preset for this session (sticky), 'once' for the next turn only, 'off' deactivates, 'status'/'list' inspect, 'set_preset'/'delete_preset' manage presets conversationally. MoA multiplies per-turn cost by the advisor count — activate only when the user asks for it."
- [ ] **Step 2: 工具测试**（同文件）：`On` 无 preset 配置 → `success:false` 带指引；`On` 有可解析 preset（用 `store_moa_config` 塞唯一命名 preset，测试后清理）→ session handle 写入 sticky；`Once` → `one_shot:true`；`Off` → 清除；无 turn context → 优雅失败；`SetPreset` 拒 recursive slot（无 patcher 也能测校验分支）。session key 经 `TURN_CONTEXT.scope(ctx, ...)` 注入（照 `select_model.rs` 测试原样）。
- [ ] **Step 3: 注册**。(a) `definitions.rs` `BUILTIN_TOOL_DEFINITIONS` 加：

```rust
    BuiltinToolDefinition {
        name: "moa",
        description: "Mixture-of-Agents advisory mode: parallel advisor models consult on the live conversation and feed private guidance to the acting aggregator; manage per-session activation and presets",
        requires_config: true, // needs injected config + patcher handles
    },
```

(b) `create_tool_boxed` 加 arm（照 session/remember 注释模式）：

```rust
        // moa requires the shared Config handle + ConfigPatcher, injected at
        // boot — constructed in the builder, same pattern as self_config.
        "moa" => None,
```

(c) builder：在 `SelfConfigTool::new` 构造点旁构造 `MoaManageTool::new().with_config(...).with_patcher(...)` 并按同路径注册实例 + metadata（若 self_config 走 `reg(...)`/registry 字段模式则镜像之——以 grep 结果为准）。
- [ ] **Step 4: fast-path 排除 + one-shot 拦截**。(a) `slash_command.rs` 把 `if cmd_name == "loop" || cmd_name == "goal"` 改为 `if cmd_name == "loop" || cmd_name == "goal" || cmd_name == "moa"`（注释补一句：moa 的 one-shot 在更早的拦截点处理，裸 /moa 由 LLM 映射到工具）。(b) Panel/CLI 拦截：在 `try_resolve_slash_command(` 调用点之前（request/session_key 在作用域内）插入：

```rust
        // `/moa <prompt>` one-shot (hermes semantics): arm MoA for THIS run
        // and let the prompt run as a normal turn. The argument is ALWAYS a
        // prompt, never a preset name. Bare `/moa` falls through to the LLM.
        if let Some(prompt) = crate::providers::moa::parse_one_shot_command(&request.input) {
            crate::providers::session_moa_handle::set_session_moa(
                &request.session_key.to_key_string(),
                None,
                true,
            );
            request.input = prompt.to_string();
        }
```

（`request` 不可变则先取 `let input_override = ...` 再在构造 run 输入处替换——以现场代码形状为准，语义不变：**状态写入必须发生在 run 构造之前**。）(c) 渠道路径：`execute_slash_command_fast_path` 的 direct_tool 分发处加早退分支——`tool_id == "moa"` 且 args 非空 → 写 one_shot + `return Err(ExecutionError::Fallthrough)`；args 为空 → 直接 `Fallthrough`（LLM 处理）。
- [ ] **Step 5: 跑测试**

Run: `cargo test -p alephcore --lib moa_manage`
Expected: 6 PASS。再 `cargo check -p alephcore --lib` 一次覆盖注册/拦截接线。

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/moa_manage.rs src/builtin_tools/mod.rs src/executor/builtin_registry/ src/gateway/execution_engine/slash_command.rs src/gateway/execution_engine/ src/config/patcher.rs
git commit -m "tools: moa manage tool (on/off/once/status/presets) with /moa one-shot intercept and fast-path exclusion"
```

---

### Task 9: Panel 顾问块渲染

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs`（`apply_trace_event` 加 3 个 arm）
- Modify: `interfaces/webchat/src/platform/wide/views/agent_trace_model.rs`（若 Task 5 的协议 presentation fn 已覆盖则此处无需改动——先确认编译）

**Interfaces:**
- Consumes: Task 5 的 wire kind `"moa_advisor"` / `"moa_aggregating"` / `"moa_advisor_spend"`（serde tag 字段 `type`（LoopTraceEvent 形）或 `kind`（协议形），`apply_trace_event` 两者都接受）
- Produces: 顾问建议以 reasoning 风格块出现在聊天流（live + `trace.by_runs` 回放两路自动生效）

- [ ] **Step 1: 加渲染 arm**（`events.rs` `apply_trace_event` 的 match，`"verifier_veto"` arm 之后、`_ => {}` 之前；复用 `append_reasoning` sink——与 `tool_summary`/`verifier_veto` 同款）：

```rust
        "moa_advisor" => {
            let index = trace_event.get("index").and_then(serde_json::Value::as_u64).unwrap_or(0);
            let count = trace_event.get("count").and_then(serde_json::Value::as_u64).unwrap_or(0);
            let label = trace_event.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let text = trace_event.get("text").and_then(|v| v.as_str()).unwrap_or("");
            append_reasoning(chat, &format!("◇ 顾问 {index}/{count} — {label}\n{text}"));
            workspace.note_activity();
        }
        "moa_aggregating" => {
            let aggregator = trace_event.get("aggregator").and_then(|v| v.as_str()).unwrap_or("");
            let n = trace_event.get("advisor_count").and_then(serde_json::Value::as_u64).unwrap_or(0);
            append_reasoning(chat, &format!("◆ MoA 聚合中（{aggregator}，{n} 位顾问）"));
        }
        "moa_advisor_spend" => {
            let input = trace_event.get("input_tokens").and_then(serde_json::Value::as_u64).unwrap_or(0);
            let output = trace_event.get("output_tokens").and_then(serde_json::Value::as_u64).unwrap_or(0);
            let cost = trace_event.get("cost_usd").and_then(serde_json::Value::as_f64);
            let cost_str = cost.map_or(String::new(), |c| format!("，约 ${c:.4}"));
            append_reasoning(chat, &format!("▫ 顾问开销：{input}+{output} tokens{cost_str}"));
        }
```

- [ ] **Step 2: 构建验证**

Run: `just wasm`
Expected: WASM 构建通过。若 `agent_trace_model.rs` 因协议 presentation fn 的新变体缺 arm 而编译失败，按 Task 5 Step 2 的标签风格补齐。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/
git commit -m "panel: render MoA advisor/aggregating/spend trace events as reasoning blocks"
```

> ⚠️ 运行时看效果需重编 server 二进制（Panel 经 rust_embed 编译期嵌入——见 CLAUDE.md「Panel ↔ Daemon 资源嵌入链」）；本计划不含部署，QA 时执行。

---

### Task 10: 文档 + spec 修订 + 终验

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`（新增 MoA 小节）
- Modify: `docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md`（决定 #6 修订注记）

**Interfaces:** 无代码。

- [ ] **Step 1: MULTI_AGENT_SYSTEM.md 加一节**（放在四模式（Spawn/Delegate/Team/A2A）之后，中文为主、术语英文，~30 行）：标题「MoA（Mixture of Agents）」，两小节：① **连续咨询（本次移植）**——`MoaProvider` 门面、`moa` 工具/`/moa` one-shot、`[moa]` presets、fanout 节奏、advisor 独立计量、trace 事件（源自 hermes-agent MoAClient，spec 链接）；② **一次性任务 fan-out（既有，此前未文档化）**——`subagent` 工具 `proposer_models`+`synthesize`+`aggregator_model`（引 `src/agents/subagent_tool/`，Wang et al. 2406.04692），说明两者互补边界（任务 fan-out fresh context vs 对话状态连续咨询）。
- [ ] **Step 2: spec 修订**：在 spec §2 决定 6 末尾追加一句：「**实施期修订（2026-07-05 验证）**：gateway 每回合 `model_override` 在 harness 路径上只进 `ModelResolved` 事件与健康上报，从不到达 runner Step 3（`FlowRequest` 无模型字段）——该覆盖今天对线上模型本就无效。故优先级实现为 MoA > select_model > agent pin > brain；model_override 交互留待其管道补通时挂钩。」
- [ ] **Step 3: 终验**（一次性）：

```bash
cargo test -p alephcore --lib moa 2>&1 | tail -5
cargo check -p alephcore --lib
```

Expected: 全部 moa 相关测试 PASS（Task 1-8 累计 ~30 个），check 干净。如任一失败，修复后重跑（只跑失败的过滤集）。

- [ ] **Step 4: Commit**

```bash
git add docs/reference/MULTI_AGENT_SYSTEM.md
git add -f docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md
git commit -m "docs: MoA continuous advisory + existing one-shot subagent MoA documentation; spec decision 6 amendment"
```

---

## Self-Review 记录（计划作者已跑）

1. **Spec 覆盖**：§4 架构/组件（Task 1-7）✓ §5 配置（Task 1）✓ §6 用户面含 one-shot 语义与优先级（Task 2/7/8）✓ §7 错误处理（Task 6 fail-soft 测试 + Task 7 回退）✓ §8 可观测/核算（Task 5/6/9）✓ §9 测试策略逐条对应 ✓ §10 超越清单：①join_all ②timeout ③named_providers 链 ④consume-and-clear ⑤typed config ⑥usage 不合并 ⑦仅 trace.rs 触点 ✓ §11 范围外未混入 ✓。spec 决定 #6 的偏差已作为显式修订（Global Constraints + Task 10）。
2. **占位符**：三处「以现场代码形状为准」的接线点（Task 8 Step 3c/4b/4c、Task 6 CostEstimate 字段）均给出精确 grep 锚点 + 完整目标代码 + 不变语义约束，非 TBD。
3. **类型一致性**：`SessionMoaPref{preset,one_shot}`、`take_for_run`、`try_build_for_run(pref, Option<&MoaToml>, named, sink)`、`AdvisorOutcome{label,text}`、wire kinds `moa_advisor/moa_aggregating/moa_advisor_spend/moa_turn_trace` 跨任务口径一致 ✓。

