# Agent 默认 Model 机制重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 agent 的 model 从「手填自由字符串」改为「选已配置 model 或继承系统默认」,被选中的 model 若 provider 被删/禁用/model 不在列表则自动回退系统默认。

**Architecture:** `AgentDefinition.model` 升级为结构化 `Option<AgentModelRef>`(`None`=继承系统默认,`Qualified{provider,model}`=选中的已配置 model,`Legacy(String)`=旧自由字符串兼容)。解析在 `agent_resolver` 末尾对 `Qualified` 做存在性校验,失效则当作 `None` 落到既有 `defaults.model > profile.model > DEFAULT_MODEL` 链。Panel Overview tab 的自由文本框换成 catalog 下拉。

**Tech Stack:** Rust(serde untagged enum、toml_edit、schemars)、Leptos WASM(webchat panel)。

**Spec:** `docs/superpowers/specs/2026-06-01-agent-default-model-design.md`

**关键约定**:`DEFAULT_MODEL = ""`(空串信号 provider registry 用自身默认)。「系统默认 model」= 解析链尾端 `defaults.model > profile.model > DEFAULT_MODEL`,**不引入新概念**。

**全局构建/测试命令**(每个任务复用):
- 核心快速编译:`cargo check -p alephcore`
- 核心单测:`cargo test -p alephcore --lib <filter>`
- Panel 编译:`cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`

---

## Task 1: 定义 `AgentModelRef` 类型(独立可编译 + serde 往返)

**Files:**
- Modify: `src/config/types/agents_def.rs`(在 `AgentParams` 之后、`AgentDefinition` 之前新增类型,约第 151 行处)

- [ ] **Step 1: 写失败测试**

在 `src/config/types/agents_def.rs` 文件末尾新增 `#[cfg(test)]` 模块(若文件已有 tests 模块则并入):

```rust
#[cfg(test)]
mod model_ref_tests {
    use super::AgentModelRef;

    #[test]
    fn legacy_bare_string_roundtrips() {
        // 旧 config 的裸字符串 → Legacy
        let m: AgentModelRef = serde_json::from_value(serde_json::json!("claude-opus-4")).unwrap();
        assert_eq!(m, AgentModelRef::Legacy("claude-opus-4".to_string()));
        assert_eq!(m.model_str(), "claude-opus-4");
    }

    #[test]
    fn qualified_table_parses() {
        let m: AgentModelRef = serde_json::from_value(
            serde_json::json!({ "provider": "anthropic", "model": "claude-sonnet-4" }),
        )
        .unwrap();
        assert_eq!(
            m,
            AgentModelRef::Qualified {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
            }
        );
        assert_eq!(m.model_str(), "claude-sonnet-4");
    }

    #[test]
    fn legacy_serializes_back_to_bare_string() {
        let m = AgentModelRef::Legacy("gpt-5".to_string());
        assert_eq!(serde_json::to_value(&m).unwrap(), serde_json::json!("gpt-5"));
    }

    #[test]
    fn qualified_serializes_to_table() {
        let m = AgentModelRef::Qualified {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({ "provider": "openai", "model": "gpt-5" })
        );
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore --lib model_ref_tests`
Expected: FAIL — `cannot find type AgentModelRef`。

- [ ] **Step 3: 实现 `AgentModelRef`**

在 `src/config/types/agents_def.rs` 的 `AgentParams` struct 之后插入:

```rust
// =============================================================================
// AgentModelRef
// =============================================================================

/// agent 选中的 model 引用。
///
/// 作为 `AgentDefinition.model` 的值;字段为 `None` 时表示**继承系统默认 model**
/// (= `defaults.model > profile.model > DEFAULT_MODEL`,即“当前默认 model”)。
///
/// 用 `#[serde(untagged)]` 兼容两种 TOML 写法:
/// - 裸字符串 `model = "claude-x"` → [`AgentModelRef::Legacy`](旧格式,不做删除检测)
/// - 内联表 `model = { provider = "anthropic", model = "claude-x" }` → [`AgentModelRef::Qualified`]
///   (Panel 从已配置 model 里选出,带 provider 以便检测“被删/禁用/移除”而自动回退)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AgentModelRef {
    /// Panel 选中的已配置 model,pin 住 provider + model。
    Qualified { provider: String, model: String },
    /// 旧 config 的自由字符串。永不参与删除回退校验。
    Legacy(String),
}

impl AgentModelRef {
    /// 返回 model id(两种变体都带)。仅用于显示与“不校验”路径;
    /// 删除回退的真正解析在 `agent_resolver::resolve_model_ref`。
    pub fn model_str(&self) -> &str {
        match self {
            Self::Qualified { model, .. } => model,
            Self::Legacy(s) => s,
        }
    }
}
```

确认文件顶部已 `use` 了 `serde::{Deserialize, Serialize}` 与 `schemars::JsonSchema`(本文件其他类型已在用,无需新增 import)。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore --lib model_ref_tests`
Expected: PASS(4 个测试)。

- [ ] **Step 5: 提交**

```bash
git add src/config/types/agents_def.rs
git commit -m "config: add AgentModelRef type for structured agent model selection"
```

---

## Task 2: 切换 `AgentDefinition.model` 为 `Option<AgentModelRef>`(修复所有编译点,行为不变)

> 本任务只把字段类型换掉并保留**现有行为**(Legacy/Qualified 都直接取 model 字符串,暂不加回退校验——回退在 Task 3 加)。Rust 要求整 crate 编译,故所有读取点同一提交内修复。

**Files:**
- Modify: `src/config/types/agents_def.rs:200-202`(字段类型)
- Modify: `src/config/agent_manager/toml_ops.rs:135-137`(写 TOML)
- Modify: `src/config/agent_resolver.rs:275-281`(解析链 model 步骤)
- Modify: `src/gateway/handlers/agents.rs:49`(`AgentSummary::from` 显示)
- Modify(测试字面量): `src/config/agent_resolver.rs`(约 711-720 构造 `AgentDefinition` 的测试)、`src/config/tests/agents_integration.rs`

- [ ] **Step 1: 改字段类型**

`src/config/types/agents_def.rs` 第 200-202 行:

```rust
    /// AI model override for this agent.
    /// `None` = 继承系统默认 model(= defaults.model > profile.model > DEFAULT_MODEL)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<AgentModelRef>,
```

- [ ] **Step 2: 跑编译,收集所有报错点**

Run: `cargo check -p alephcore 2>&1 | rg "expected|mismatched|method .* not found|\.rs:" | head -40`
Expected: 多处 `expected AgentModelRef, found String` / `.as_str()` 等错误,集中在下列文件。逐一修复(Step 3-6)。

- [ ] **Step 3: 修 `toml_ops.rs` 写 TOML**

`src/config/agent_manager/toml_ops.rs` 第 135-137 行,替换为:

```rust
        if let Some(ref model) = def.model {
            agent["model"] = model_ref_to_item(model);
        }
```

并在同文件末尾(`impl` 块外)新增共享 helper:

```rust
/// 把 `AgentModelRef` 写成 toml_edit Item:
/// Legacy → 裸字符串;Qualified → 内联表 `{ provider, model }`。
pub(super) fn model_ref_to_item(
    m: &crate::config::types::agents_def::AgentModelRef,
) -> toml_edit::Item {
    use crate::config::types::agents_def::AgentModelRef;
    match m {
        AgentModelRef::Legacy(s) => toml_edit::value(s.as_str()),
        AgentModelRef::Qualified { provider, model } => {
            let mut t = toml_edit::InlineTable::new();
            t.insert("provider", provider.as_str().into());
            t.insert("model", model.as_str().into());
            toml_edit::value(t)
        }
    }
}
```

- [ ] **Step 4: 修 `agent_resolver.rs` 解析链(暂不校验,行为不变)**

`src/config/agent_resolver.rs` 第 275-281 行,替换为:

```rust
        // 4. Resolve model: agent.model > defaults.model > profile.model > DEFAULT_MODEL
        // 注:Task 3 会把 `m.model_str()` 替换为带删除校验的 resolve_model_ref。
        let model = agent
            .model
            .as_ref()
            .map(|m| m.model_str().to_string())
            .or_else(|| defaults.model.clone())
            .or_else(|| profile.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
```

- [ ] **Step 5: 修 gateway `AgentSummary::from`(显示用字符串)**

`src/gateway/handlers/agents.rs` 第 49 行 `model: def.model.clone(),` 替换为:

```rust
            model: def.model.as_ref().map(|m| match m {
                crate::config::types::agents_def::AgentModelRef::Legacy(s) => s.clone(),
                crate::config::types::agents_def::AgentModelRef::Qualified { provider, model } => {
                    format!("{provider}/{model}")
                }
            }),
```

(`AgentSummary.model` 保持 `Option<String>`,仅作列表显示。)

- [ ] **Step 6: 修测试字面量**

把所有构造 `AgentDefinition { model: Some("...".to_string()), .. }` 的测试改为 `AgentModelRef::Legacy`。已知位置:

`src/config/agent_resolver.rs` 测试中(搜索 `model: Some(`)——例如把:

```rust
            model: Some("claude-opus-4".to_string()),
```

改成:

```rust
            model: Some(crate::config::types::agents_def::AgentModelRef::Legacy(
                "claude-opus-4".to_string(),
            )),
```

`src/config/tests/agents_integration.rs` 同理(断言 `resolved[0].model == "claude-opus-4-6"` 这类**断言解析后的 String**——`ResolvedAgent.model` 仍是 `String`,断言不变;只改构造 `AgentDefinition` 的 model 字面量)。

用以下命令定位全部需改点:

Run: `rg -n "model: Some\(\"" src --type rust`
逐一替换为 `Legacy(...)`。

- [ ] **Step 7: 编译 + 跑相关测试确认通过**

Run: `cargo check -p alephcore`
Expected: 0 error。

Run: `cargo test -p alephcore --lib agent_resolver`
Run: `cargo test -p alephcore --test '*' agents_integration 2>/dev/null || cargo test -p alephcore agents_integration`
Expected: PASS(行为未变,Legacy 直通)。

- [ ] **Step 8: 提交**

```bash
git add src/config/types/agents_def.rs src/config/agent_manager/toml_ops.rs \
        src/config/agent_resolver.rs src/gateway/handlers/agents.rs \
        src/config/tests/agents_integration.rs
git commit -m "config: migrate AgentDefinition.model to structured AgentModelRef (behavior-preserving)"
```

---

## Task 3: Qualified 删除校验 + 回退(threading providers 进解析)

**Files:**
- Modify: `src/config/agent_resolver.rs`(新增 `resolve_model_ref` + `resolve_all`/`resolve_one` 加 `providers` 参数 + 解析链)
- Modify: 所有 `resolve_all` 调用点(2 生产 + 1 集成测试 + 5 单测)

- [ ] **Step 1: 写失败测试**

在 `src/config/agent_resolver.rs` 的 `#[cfg(test)]` 模块内新增:

```rust
    #[test]
    fn resolve_model_ref_legacy_passes_through() {
        use crate::config::types::agents_def::AgentModelRef;
        let providers = std::collections::HashMap::new();
        let m = AgentModelRef::Legacy("anything".to_string());
        assert_eq!(super::resolve_model_ref(&m, &providers), Some("anything".to_string()));
    }

    #[test]
    fn resolve_model_ref_qualified_hits_when_valid() {
        use crate::config::types::agents_def::AgentModelRef;
        use crate::config::types::provider::ProviderConfig;
        let mut providers = std::collections::HashMap::new();
        providers.insert("anthropic".to_string(), ProviderConfig {
            enabled: true,
            models: vec!["claude-sonnet-4".to_string()],
            ..test_provider()
        });
        let m = AgentModelRef::Qualified {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
        };
        assert_eq!(super::resolve_model_ref(&m, &providers), Some("claude-sonnet-4".to_string()));
    }

    #[test]
    fn resolve_model_ref_falls_back_when_provider_missing_disabled_or_model_dropped() {
        use crate::config::types::agents_def::AgentModelRef;
        use crate::config::types::provider::ProviderConfig;
        let mut providers = std::collections::HashMap::new();
        providers.insert("anthropic".to_string(), ProviderConfig {
            enabled: false, // 禁用
            models: vec!["claude-sonnet-4".to_string()],
            ..test_provider()
        });
        providers.insert("openai".to_string(), ProviderConfig {
            enabled: true,
            models: vec!["gpt-5".to_string()], // 不含 gpt-4
            ..test_provider()
        });
        let disabled = AgentModelRef::Qualified { provider: "anthropic".into(), model: "claude-sonnet-4".into() };
        let missing_provider = AgentModelRef::Qualified { provider: "ghost".into(), model: "x".into() };
        let model_dropped = AgentModelRef::Qualified { provider: "openai".into(), model: "gpt-4".into() };
        assert_eq!(super::resolve_model_ref(&disabled, &providers), None);
        assert_eq!(super::resolve_model_ref(&missing_provider, &providers), None);
        assert_eq!(super::resolve_model_ref(&model_dropped, &providers), None);
    }
```

并在测试模块内新增构造 `ProviderConfig` 的 helper(字段照搬 `src/config/types/provider.rs` 默认):

```rust
    #[cfg(test)]
    fn test_provider() -> crate::config::types::provider::ProviderConfig {
        crate::config::types::provider::ProviderConfig {
            protocol: None,
            api_key: None,
            models: vec![],
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 60,
            stream_idle_timeout_secs: None,
            cache_retention: None,
            enabled: true,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
        }
    }
```

> 注:`test_provider()` 必须覆盖 `ProviderConfig` 的**全部字段**。实现前先 `rg -n "pub " src/config/types/provider.rs | rg -A30 "struct ProviderConfig"` 核对字段集,缺字段会编译失败——补齐即可。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p alephcore --lib resolve_model_ref`
Expected: FAIL — `cannot find function resolve_model_ref`。

- [ ] **Step 3: 实现 `resolve_model_ref`**

在 `src/config/agent_resolver.rs` 顶部 `use` 区补:

```rust
use std::collections::HashMap;
use crate::config::types::agents_def::AgentModelRef;
use crate::config::types::provider::ProviderConfig;
```

(`HashMap` 可能已 import——若重复,删掉重复行。)

在文件中(`impl` 块外,模块级)新增纯函数:

```rust
/// 把 agent 选中的 [`AgentModelRef`] 解析为最终 model 字符串;
/// 不可用时返回 `None`,由调用方回退到系统默认。
///
/// - `Legacy(s)` → 始终 `Some(s)`(旧自由字符串,不校验)。
/// - `Qualified{provider, model}` → 仅当 **provider 存在 && enabled && model ∈ provider.models**
///   时 `Some(model)`;否则 `None` 并 warn(provider 被删/禁用/model 被移除)。
pub(crate) fn resolve_model_ref(
    m: &AgentModelRef,
    providers: &HashMap<String, ProviderConfig>,
) -> Option<String> {
    match m {
        AgentModelRef::Legacy(s) => Some(s.clone()),
        AgentModelRef::Qualified { provider, model } => {
            let available = providers
                .get(provider)
                .is_some_and(|p| p.enabled && p.models.iter().any(|x| x == model));
            if available {
                Some(model.clone())
            } else {
                tracing::warn!(
                    provider = %provider,
                    model = %model,
                    "selected agent model unavailable (provider removed/disabled or model dropped), \
                     falling back to system default"
                );
                None
            }
        }
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore --lib resolve_model_ref`
Expected: PASS(3 个测试)。

- [ ] **Step 5: 把 `providers` threaded 进 `resolve_all` → `resolve_one` 并接到解析链**

`resolve_all` 签名(`src/config/agent_resolver.rs:114`)改为:

```rust
    pub fn resolve_all(
        &mut self,
        config: &AgentsConfig,
        profiles: &HashMap<String, ProfileConfig>,
        providers: &HashMap<String, ProviderConfig>,
    ) -> Vec<ResolvedAgent> {
```

内部 `.map(|agent_def| self.resolve_one(agent_def, &effective.defaults, profiles))`(约第 150 行)改为:

```rust
            .map(|agent_def| self.resolve_one(agent_def, &effective.defaults, profiles, providers))
```

`resolve_one` 签名(约第 199 行)加参数:

```rust
    fn resolve_one(
        &mut self,
        agent: &AgentDefinition,
        defaults: &AgentDefaults,
        profiles: &HashMap<String, ProfileConfig>,
        providers: &HashMap<String, ProviderConfig>,
    ) -> ResolvedAgent {
```

把 Task 2 Step 4 的 model 步骤(`.map(|m| m.model_str().to_string())`)替换为校验版:

```rust
        // 4. Resolve model: 选中的 Qualified 失效 → 当作 None 落到系统默认链。
        let model = agent
            .model
            .as_ref()
            .and_then(|m| resolve_model_ref(m, providers))
            .or_else(|| defaults.model.clone())
            .or_else(|| profile.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
```

- [ ] **Step 6: 修所有 `resolve_all` 调用点**

Run: `rg -n "resolve_all\(" src --type rust`

生产调用(传真实 providers):
- `src/bin/aleph-server/commands/start/mod.rs:568` →
  `agent_resolver.resolve_all(&loaded_app_config.agents, &loaded_app_config.profiles, &loaded_app_config.providers);`
- `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1174` →
  `resolver.resolve_all(&app_config.agents, &app_config.profiles, &app_config.providers)`

测试调用(传空 map,这些用例用 Legacy 字符串,不受校验影响):
- `src/config/agent_resolver.rs` 内 716/753/815/863/905 五处 → 末尾加 `, &std::collections::HashMap::new()`
- `src/config/tests/agents_integration.rs:98` → 加 `, &config.providers`(该集成测试持有完整 `config`)

> 若某测试构造的是 `Qualified` 且期望命中,记得给那条测试的 providers map 填入对应 provider;现有测试都是 Legacy,空 map 即可。

- [ ] **Step 7: 编译 + 全量解析测试**

Run: `cargo check -p alephcore`
Expected: 0 error。

Run: `cargo test -p alephcore --lib agent_resolver`
Run: `cargo test -p alephcore agents_integration`
Expected: PASS。

- [ ] **Step 8: 提交**

```bash
git add src/config/agent_resolver.rs src/bin/aleph-server/commands/start/mod.rs \
        src/bin/aleph-server/commands/start/builder/agent_init/mod.rs \
        src/config/tests/agents_integration.rs
git commit -m "config: validate Qualified agent model against live providers, fall back to system default when unavailable"
```

---

## Task 4: `AgentPatch.model` + crud 写入/清除

> Panel 保存时:选了具体 model → patch `{"model": {"provider":..,"model":..}}`;选「继承默认」→ patch `{"model": null}`(清除);未触碰 → 不带 model 键。用 double-option 区分这三态。

**Files:**
- Modify: `src/config/agent_manager/mod.rs:46-55`(`AgentPatch` 加 model + double-option helper)
- Modify: `src/config/agent_manager/crud.rs:273-349`(update 应用 model patch)
- Modify: `src/config/agent_manager/tests.rs`(新增测试)

- [ ] **Step 1: 写失败测试**

在 `src/config/agent_manager/tests.rs` 新增(若无该文件,在 `mod.rs` 的 `#[cfg(test)] mod tests` 内):

```rust
    #[test]
    fn update_sets_qualified_model() {
        use crate::config::types::agents_def::AgentModelRef;
        let (mgr, _tmp) = test_manager_with_agent("main");
        let patch = AgentPatch {
            model: Some(Some(AgentModelRef::Qualified {
                provider: "anthropic".into(),
                model: "claude-sonnet-4".into(),
            })),
            ..Default::default()
        };
        mgr.update("main", patch).unwrap();
        let def = mgr.get("main").unwrap();
        assert_eq!(
            def.model,
            Some(AgentModelRef::Qualified {
                provider: "anthropic".into(),
                model: "claude-sonnet-4".into()
            })
        );
    }

    #[test]
    fn update_clears_model_to_inherit() {
        use crate::config::types::agents_def::AgentModelRef;
        let (mgr, _tmp) = test_manager_with_agent("main");
        // 先设一个 model
        mgr.update("main", AgentPatch {
            model: Some(Some(AgentModelRef::Legacy("gpt-5".into()))),
            ..Default::default()
        }).unwrap();
        assert!(mgr.get("main").unwrap().model.is_some());
        // 清除 → 继承
        mgr.update("main", AgentPatch {
            model: Some(None),
            ..Default::default()
        }).unwrap();
        assert!(mgr.get("main").unwrap().model.is_none());
    }

    #[test]
    fn update_absent_model_leaves_it_untouched() {
        use crate::config::types::agents_def::AgentModelRef;
        let (mgr, _tmp) = test_manager_with_agent("main");
        mgr.update("main", AgentPatch {
            model: Some(Some(AgentModelRef::Legacy("gpt-5".into()))),
            ..Default::default()
        }).unwrap();
        // patch 不带 model(None outer)→ 不动
        mgr.update("main", AgentPatch {
            name: Some("Renamed".into()),
            ..Default::default()
        }).unwrap();
        assert_eq!(mgr.get("main").unwrap().model, Some(AgentModelRef::Legacy("gpt-5".into())));
    }
```

> `test_manager_with_agent` 若不存在,复用 `tests.rs` 中既有的 `AgentManager` 构造 helper(查 `rg -n "fn .*manager|AgentManager::new" src/config/agent_manager/tests.rs`),仿照其建一个写入单个 agent 的 fixture。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p alephcore --lib agent_manager::tests::update_sets_qualified_model`
Expected: FAIL — `AgentPatch` 无 `model` 字段。

- [ ] **Step 3: 给 `AgentPatch` 加 model 字段(double-option)**

`src/config/agent_manager/mod.rs`:在 `use` 区补 `AgentModelRef`:

```rust
use crate::config::types::agents_def::{AgentIdentity, AgentModelRef, AgentParams, SubagentPolicy};
```

`AgentPatch`(第 46-55 行)加字段:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentPatch {
    pub name: Option<String>,
    pub identity: Option<AgentIdentity>,
    pub params: Option<AgentParams>,
    pub skills: Option<Vec<String>>,
    pub skills_blacklist: Option<Vec<String>>,
    pub subagents: Option<SubagentPolicy>,
    pub allowed_links: Option<Vec<String>>,
    /// Model 更新三态:
    /// - 缺省(`None`)= 不动
    /// - `Some(None)` = 清除 → 继承系统默认
    /// - `Some(Some(ref))` = 设为该 model
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub model: Option<Option<AgentModelRef>>,
}

/// serde helper:把缺省键与显式 `null` 区分为 `None` vs `Some(None)`。
fn deserialize_double_option<'de, D>(
    deserializer: D,
) -> Result<Option<Option<AgentModelRef>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // 键存在时本函数才被调用(配合 `#[serde(default)]`):
    // 值为 null → Some(None);值为对象/字符串 → Some(Some(..))。
    Ok(Some(Option::<AgentModelRef>::deserialize(deserializer)?))
}
```

确认 `mod.rs` 顶部已 `use serde::{Deserialize, Serialize};`(现有),并补 `use serde::Deserialize as _;` 不需要——`Option::deserialize` 通过 trait 已可用,但需 `use serde::Deserialize;`(已存在)。

- [ ] **Step 4: crud `update` 应用 model patch**

`src/config/agent_manager/crud.rs`:在 `update` 的 patch 应用段(`allowed_links` 处理之后、`self.save_document(&doc)?` 之前,约第 349 行)新增:

```rust
        // Model: 三态 — Some(Some)=set, Some(None)=clear, None=untouched
        match &patch.model {
            Some(Some(model_ref)) => {
                agent_table["model"] = super::toml_ops::model_ref_to_item(model_ref);
            }
            Some(None) => {
                agent_table.remove("model");
            }
            None => {}
        }
```

> `model_ref_to_item` 是 Task 2 Step 3 在 `toml_ops.rs` 新增的 `pub(super)` helper。确认 `crud.rs` 能以 `super::toml_ops::model_ref_to_item` 访问;若 `toml_ops` 非 `pub(super) mod`,在 `mod.rs` 把 `mod toml_ops;` 保持模块可见即可(同 crate 同父模块默认可见)。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p alephcore --lib agent_manager`
Expected: PASS(含 3 新测试 + 既有)。

- [ ] **Step 6: 提交**

```bash
git add src/config/agent_manager/mod.rs src/config/agent_manager/crud.rs \
        src/config/agent_manager/tests.rs
git commit -m "agent_manager: support model field in AgentPatch (set/clear/untouched tri-state)"
```

---

## Task 5: Panel Overview tab — catalog 下拉替换自由文本框

> 把 `overview.rs` 的「Primary model」自由文本 input 换成 `<select>`:顶部「继承系统默认」+ 已配置 model 列表(来自 `providers.catalog(Configured)`)。保存写 `patch["model"]`(选中→`{provider,model}`,继承→`null`)。删除既有 `model_config`/`fallbacks` 写法(后端从不消费,死字段)。

**Files:**
- Modify: `interfaces/webchat/src/views/agents/overview.rs`

- [ ] **Step 1: 加载 catalog + 改信号模型**

`overview.rs` 顶部 import 补:

```rust
use crate::api::providers::{CatalogEntry, CatalogView, ProvidersApi};
```

把第 20-21 行的:

```rust
    let primary_model = RwSignal::new(String::new());
    let fallbacks = RwSignal::new(String::new());
```

替换为(`selected_model` 存 `provider::model` 编码串,空串=继承;`catalog` 存可选项):

```rust
    // "" = 继承系统默认;否则 "provider\u{1f}model" 编码(catalog 选中项)
    let selected_model = RwSignal::new(String::new());
    let catalog: RwSignal<Vec<CatalogEntry>> = RwSignal::new(Vec::new());
```

在 `Effect::new` 加载 agent detail 的 `spawn_local` **之前**,新增一次 catalog 拉取:

```rust
    {
        let dash = state;
        spawn_local(async move {
            if let Ok(items) = ProvidersApi::catalog(&dash, CatalogView::Configured).await {
                catalog.set(items);
            }
        });
    }
```

- [ ] **Step 2: 加载时解析已存 model 为下拉选中值**

替换第 71-87 行(`model_config`/`model` 解析块)为:

```rust
                // 读取已存 model:Qualified 对象 → "provider\u{1f}model";Legacy 字符串 → 尝试匹配,匹配不到回退继承
                if let Some(mv) = def.get("model") {
                    if let Some(obj) = mv.as_object() {
                        let p = obj.get("provider").and_then(|v| v.as_str()).unwrap_or("");
                        let m = obj.get("model").and_then(|v| v.as_str()).unwrap_or("");
                        if !p.is_empty() && !m.is_empty() {
                            selected_model.set(format!("{p}\u{1f}{m}"));
                        }
                    }
                    // Legacy 裸字符串:无 provider 上下文,留空=继承(用户可重选);不阻塞。
                }
```

> 编码用 `\u{1f}`(unit separator)分隔 provider/model,避免与 model 名里的 `/` 冲突。

- [ ] **Step 3: 保存时构造 model patch(三态)**

替换 `handle_save` 中第 115-137 行(`fb_list` + `model_config` 段)为:

```rust
        let sel = selected_model.get();
        let model_patch = if sel.is_empty() {
            serde_json::Value::Null // 继承系统默认
        } else if let Some((p, m)) = sel.split_once('\u{1f}') {
            json!({ "provider": p, "model": m })
        } else {
            serde_json::Value::Null
        };
```

并把 `patch` 构造(第 122-137 行附近)改为始终带 `model` 键:

```rust
        let mut patch = json!({
            "name": name.get(),
            "identity": {
                "emoji": emoji.get(),
                "description": description.get(),
                "theme": theme.get(),
            },
            "model": model_patch,
        });
```

(删除原 `let pm = primary_model.get(); if !pm.is_empty() { patch["model_config"] = ... }` 整段。)

- [ ] **Step 4: 把 Model Configuration 区块换成下拉**

替换第 223-249 行(`// Model Configuration` 整个 div)为:

```rust
            // Model Configuration
            <div class="bg-surface-raised border border-border rounded-xl p-6">
                <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, agents.overview.model_config)}</h2>
                <div class="space-y-2">
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.primary_model)}</label>
                    <select
                        prop:value=move || selected_model.get()
                        on:change=move |ev| selected_model.set(event_target_value(&ev))
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono text-sm"
                    >
                        <option value="">"继承系统默认 (inherit system default)"</option>
                        {move || {
                            catalog.get().into_iter().flat_map(|entry: CatalogEntry| {
                                let provider_id = entry.id.clone();
                                let models = if entry.models.is_empty() {
                                    vec![entry.default_model.clone()]
                                } else {
                                    entry.models.clone()
                                };
                                let dn = entry.display_name.clone();
                                models.into_iter().map(move |m| {
                                    let val = format!("{}\u{1f}{}", provider_id, m);
                                    let label = format!("{} / {}", dn, m);
                                    view! { <option value=val>{label}</option> }
                                }).collect::<Vec<_>>()
                            }).collect::<Vec<_>>()
                        }}
                    </select>
                    // 已存 Qualified 不在 catalog(被删/禁用)时提示
                    {move || {
                        let sel = selected_model.get();
                        let in_catalog = sel.is_empty() || catalog.get().iter().any(|e| {
                            let models = if e.models.is_empty() { vec![e.default_model.clone()] } else { e.models.clone() };
                            models.iter().any(|m| format!("{}\u{1f}{}", e.id, m) == sel)
                        });
                        (!in_catalog).then(|| view! {
                            <p class="mt-1 text-xs text-warning">
                                "\u{26a0} 当前选中的 model 已失效(provider 被删/禁用),保存后将回退系统默认"
                            </p>
                        })
                    }}
                </div>
            </div>
```

> 若 `text-warning` 在 Tailwind token 中不存在,用 `text-danger/80`(behavior.rs 已用该类)。

- [ ] **Step 5: 删除残留引用 + 编译**

确认 `fallbacks` / `primary_model` 信号已无引用(Step 1 已删定义)。若 `t!(i18n, agents.overview.fallback_*)` 等 i18n key 仅此处用过,留着不影响(未引用即可)。

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: 0 error。若报 `CatalogEntry` 字段名不符,以 `interfaces/webchat/src/api/providers.rs` 实际定义为准(`id`/`display_name`/`default_model`/`models`/`color` 见 model_picker.rs 用法)。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/views/agents/overview.rs
git commit -m "panel: replace agent model free-text with configured-model dropdown + inherit option"
```

---

## Task 6: 端到端验证(daemon 重建 + 手测)

> Panel 资源在 `aleph-server` 编译时静态嵌入(rust_embed),改 panel 后必须重建 binary 才生效(见 CLAUDE.md)。

- [ ] **Step 1: 重建 WASM + binary**

Run:
```bash
just wasm
cargo build --release -p alephcore --bin aleph-server
```
Expected: 均成功。

- [ ] **Step 2: 替换运行中的 daemon(dev 路径)**

Run:
```bash
./target/release/aleph-server stop || true
cargo run --release -p alephcore --bin aleph-server -- start
```
(若用 .app daemon,按 CLAUDE.md 的 mv+cp+kill 流程。)

- [ ] **Step 3: 手测三条路径**

1. Panel → Agents → 某 agent → Overview → Model 下拉:选一个已配置 model → 保存 → 重新进入页面,下拉应保持该选中项;`~/.aleph/config.toml` 对应 `[[agents.list]]` 出现 `model = { provider = "...", model = "..." }`。
2. 把该 provider 在 Providers 页禁用(或从 config 删除)→ 重启 daemon → 该 agent 运行时落回系统默认 model(日志出现 `selected agent model unavailable ... falling back`)。
3. 下拉选「继承系统默认」→ 保存 → config.toml 中该 agent 的 `model` 键被移除。

- [ ] **Step 4: 全量核心测试 + lint**

Run:
```bash
cargo test -p alephcore --lib
cargo clippy -p alephcore --bin aleph-server 2>&1 | rg "^warning|^error" | head
```
Expected: 测试全过;clippy 对改动文件 0 warning。

- [ ] **Step 5: 提交(若有验证期微调)**

```bash
git add -A && git commit -m "agent-model: end-to-end verification fixups" || echo "nothing to commit"
```

---

## Self-Review 记录

- **Spec §3 数据结构** → Task 1(类型)+ Task 2(切换字段)。✅
- **Spec §4 解析+回退** → Task 3(`resolve_model_ref` + threading + 回退)。✅
- **Spec §5 RPC 接线**(AgentPatch.model 缺口)→ Task 4。✅
- **Spec §6 Panel UI** → Task 5。✅
- **Spec §7 兼容**(untagged Legacy)→ Task 1 测试覆盖裸字符串;Task 2 保留 Legacy 直通。✅
- **Spec §8 边界**(产出 model 字符串,provider 仅校验)→ Task 3 `resolve_model_ref` 返回 `Option<String>`,不改运行时路由。✅
- **类型一致性**:`model_ref_to_item`(Task 2 定义,Task 4 复用);`resolve_model_ref`(Task 3);`AgentPatch.model: Option<Option<AgentModelRef>>`(Task 4)三态与 Task 5 panel 的 null/对象 patch 对齐。✅
- **AgentDefaults.model / profile.model** 保持 `Option<String>` 不动(可信回退源)。✅
