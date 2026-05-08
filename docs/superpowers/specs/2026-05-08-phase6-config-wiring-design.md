# Phase-6 配置驱动装配 — Stage 7 wiring 路径的真实启用

**日期**：2026-05-08
**状态**：🟡 Design Approved（待 writing-plans 产出实施 plan）
**关联**：
- Master Spec `2026-05-05-harness-12-module-roadmap-design.md` § Stage 7
- Stage 7 plan `2026-05-08-harness-stage7-init-audit-plan.md`
- Stage 7 audit report `2026-05-08-harness-stage7-audit-report.md`

---

## 1. Context

Stage 7（commits `f13f355c6` → `c2cd8d293`）打通了 aleph-server 启动路径上 5 个 production wiring gap：`AgentHarnessRunner` 现在持有 5 个 `pub` 字段（`guardrails` / `fallback_llm` / `stall_config` / `consecutive_failure_cap` / `turn_timeout`），`harness_bridge.rs::run()` 已经在每次 session 把它们 clone 到 `HarnessDeps`。

但 `src/bin/aleph-server/commands/start/orchestrator_init.rs:130-134` 给这 5 个字段全部传 `None`（PHASE-6 占位）。结果：Stage 5a 守卫、Stage 5b fallback、P0 rescue 三大模块在 production 真实路径下**全部静默不生效**。

## 2. Goal

在 `orchestrator_init.rs` 里把这 5 个字段从 `aleph.toml` 配置加载真实值，让上述三大模块首次在 production 真实生效。

**强约束**：
- 缺任何 section → 字段保持 `None` → 行为等同 Stage 7 ship 后 main HEAD（opt-in 行为变化）
- 不新增 trait / struct / 抽象，只在 boot 路径上装配现有类型
- R10 不变：`src/harness/agent.rs` 行数不增，`src/harness/` 文件数 ≤ 9

## 3. Out-of-Scope

| 范围 | 原因 |
|------|------|
| `src/harness/agent.rs` 修改 | R10 cap 已用满（1520 行） |
| `src/orchestrator/harness_bridge.rs` 修改 | Stage 7 已 clone 5 字段，无 gap |
| `src/agents/subagent_spawner.rs` 修改 | 故意 4 字段 None（subagent 短任务不需 stall/timeout/cap/verifier_chain，guardrails 已继承） |
| FailoverProvider wrap default_provider | 不同 use case；fallback_llm 是 single-step seam，FailoverProvider 是 N-tier 负载/熔断 |
| 新 GuardrailImpl（如 ContentSafety） | 当前真实 impl 仅 `PiiSecretsGuardrail`，YAGNI |
| Stage 6b（JudgeVerifier / ComputationalVerifier） | 永久 defer（`src/verification/mod.rs` preamble 禁令） |
| `[providers]` 配置改动 | 复用现有 schema，by-name 引用 |

## 4. Architecture

Phase-6 在 boot 路径上**只增 4 块代码**：

```
src/config/structs.rs                                         +3 字段（Config 顶层）
src/config/types/phase6_wiring.rs                             新文件（3 个 schema struct）
src/config/types/mod.rs                                       +1 行 re-export
src/bin/aleph-server/commands/start/orchestrator_init.rs      +3 build_xxx 函数 + 改 line 130-134
```

5 个字段的装配数据流：

```
aleph.toml
   ├── [guardrails]          → Config::guardrails        → build_guardrail_registry(&Config)  → Option<Arc<GuardrailRegistry>>
   ├── [stability]           → Config::stability         → build_stability_triple(&Config)    → (Option<StallConfig>, Option<usize>, Option<Duration>)
   └── [fallback_provider]   → Config::fallback_provider → build_fallback_llm(&Config, primary_provider_key) → Option<Arc<dyn AiProvider>>
                                                            ↓
                                                AgentHarnessRunner { guardrails, fallback_llm, stall_config, consecutive_failure_cap, turn_timeout, .. }
                                                            ↓
                                                harness_bridge::run() clone 进 HarnessDeps（已 ship）
```

## 5. Schema 设计

### 5.1 toml（user-facing）

```toml
# 三个 section 全顶层（与 [stop_hooks] 风格对称）

[guardrails]
enabled = true                       # 单开关，true → 装 PiiSecretsGuardrail::from_globals() 三 trait

[stability]
stall_timeout_secs = 300             # Some → 构造 StallConfig；缺则 stall_config = None
stall_check_interval_secs = 30       # 缺时复用 StallConfig::default().check_interval (30s)
consecutive_failure_cap = 8          # 缺则 None
turn_timeout_secs = 300              # 缺则 None

[fallback_provider]
provider = "openai-mini"             # by-name 引用 [providers.openai-mini]；不内嵌 ProviderConfig
```

### 5.2 Rust（`src/config/types/phase6_wiring.rs`，新文件）

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[guardrails]` — Phase-6 single switch wiring `PiiSecretsGuardrail::from_globals()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GuardrailsToml {
    #[serde(default)]
    pub enabled: bool,
}

/// `[stability]` — P0 rescue knobs (stall watchdog + failure cap + per-turn timeout).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct StabilityToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_check_interval_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consecutive_failure_cap: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_timeout_secs: Option<u64>,
}

/// `[fallback_provider]` — single-step Stage 5b fallback. References an existing
/// `[providers.<name>]` entry by toml key; nothing inlined here.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FallbackProviderToml {
    pub provider: String,
}
```

### 5.3 `Config` 顶层新增

```rust
// src/config/structs.rs (within `pub struct Config`)
#[serde(default, skip_serializing_if = "Option::is_none")]
pub guardrails: Option<GuardrailsToml>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub stability: Option<StabilityToml>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub fallback_provider: Option<FallbackProviderToml>,
```

`Default for Config` 三字段全 `None`。复用 `[behavior]` (`Option<BehaviorConfig>`) 已有的 `#[serde(default, skip_serializing_if)]` idiom，toml roundtrip 无新风险。

## 6. Builder API

全部为 `orchestrator_init.rs` 内部 `fn`（非 `pub`），与 Stage 6a 的 `verification::stop_hooks::build_from_config` idiom 对称。

### 6.1 `build_guardrail_registry`

```rust
fn build_guardrail_registry(cfg: &Config) -> Option<Arc<GuardrailRegistry>> {
    let g = cfg.guardrails.as_ref()?;
    if !g.enabled { return None; }
    let pii = Arc::new(PiiSecretsGuardrail::from_globals());
    Some(Arc::new(
        GuardrailRegistry::builder()
            .with_input(pii.clone())
            .with_output(pii.clone())
            .with_tool_call(pii)
            .build()
    ))
}
```

`PiiSecretsGuardrail` 同 struct 实现 Input + Output + ToolCall 三 trait，`from_globals()` 内部处理 `PiiEngine::global()` 返回 None 的情况（仅装 SecretLeakDetector，仍工作）。

### 6.2 `build_fallback_llm`

```rust
fn build_fallback_llm(cfg: &Config, primary_provider_key: &str) -> Option<Arc<dyn AiProvider>> {
    let fb = cfg.fallback_provider.as_ref()?;
    if fb.provider == primary_provider_key {
        tracing::warn!(provider = %fb.provider, "fallback_provider self-reference; disabling");
        return None;
    }
    let pc = match cfg.providers.get(&fb.provider) {
        Some(c) => c.clone(),
        None => {
            tracing::warn!(provider = %fb.provider, "fallback_provider not in [providers]; disabling");
            return None;
        }
    };
    match create_provider(&fb.provider, pc) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(provider = %fb.provider, error = %e, "create_provider failed; disabling");
            None
        }
    }
}
```

**`primary_provider_key` 来源**（plan 阶段决议）：boot 路径在选择 `default_provider` 时已经在 toml 里读了一个 key（如 `general.default_provider = "anthropic"`）。该 key 透传到 `initialize_orchestrator()` 即可。如果实现成本高（caller 链长），降级为 `default_provider.name()` 比较 type；接受"用户填同 type 的另一个 key 不会被自指挡住"的弱语义。

### 6.3 `build_stability_triple`

```rust
fn build_stability_triple(
    cfg: &Config,
) -> (Option<StallConfig>, Option<usize>, Option<Duration>) {
    let Some(s) = cfg.stability.as_ref() else { return (None, None, None); };
    let stall_config = s.stall_timeout_secs.map(|secs| {
        let mut sc = StallConfig::default();
        sc.timeout = Duration::from_secs(secs);
        if let Some(ci) = s.stall_check_interval_secs {
            sc.check_interval = Duration::from_secs(ci);
        }
        sc
    });
    (
        stall_config,
        s.consecutive_failure_cap,
        s.turn_timeout_secs.map(Duration::from_secs),
    )
}
```

`stall_check_interval_secs` 缺省 → 复用 `StallConfig::default().check_interval` (30s)。`stall_timeout_secs` 是构造 `StallConfig` 的必要条件 — 缺则整个 `stall_config = None`。

## 7. Wiring 改造（`initialize_orchestrator`）

`orchestrator_init.rs` line 117-138 改造前后：

```rust
// 新签名加 &Config
pub(in crate::commands::start) async fn initialize_orchestrator(
    config: &Config,                                                        // ← NEW
    agent_registry: Arc<...>,
    session_service: Arc<...>,
    tool_service: Arc<...>,
    default_provider: Arc<dyn AiProvider>,
    primary_provider_key: &str,                                              // ← NEW (plan 阶段决议)
    sandbox: Arc<dyn Sandbox>,
    stop_hook_configs: &[StopHookConfig],
    memory_context_provider: Option<Arc<...>>,
) -> anyhow::Result<Arc<Orchestrator>> {
    // ... existing flow (presets / routing / sandbox_factory / verifier_chain / power) ...

    let guardrails = build_guardrail_registry(config);
    let fallback_llm = build_fallback_llm(config, primary_provider_key);
    let (stall_config, consecutive_failure_cap, turn_timeout) =
        build_stability_triple(config);

    let harness = Arc::new(AgentHarnessRunner {
        agent_registry: agent_registry.clone(),
        session_service: session_service.clone(),
        tool_service,
        default_provider,
        named_providers: HashMap::new(),
        verifier_chain,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        guardrails,                  // ← was None
        fallback_llm,                // ← was None
        stall_config,                // ← was None
        consecutive_failure_cap,     // ← was None
        turn_timeout,                // ← was None
        power,
        memory_context_provider,
    });
    // ... existing Orchestrator::new + Arc::new ...
}
```

caller（`start.rs` / `bootstrap.rs`）已经持有 `Arc<Config>` —— plan 阶段确认调用点签名后透传 `&config` 进来。

## 8. 测试矩阵

全部为 `cargo test -p alephcore --lib` 内部测试，紧贴 builder 同 mod（`#[cfg(test)] mod tests`），避免暴露 builder 为 `pub`。

| # | Builder | 测试名 | 输入 | 期望 |
|---|---------|--------|------|------|
| 1 | guardrails | `missing_section_returns_none` | `Config::default()` | `None` |
| 2 | guardrails | `disabled_returns_none` | `enabled = false` | `None` |
| 3 | guardrails | `enabled_wires_pii_secrets` | `enabled = true` | `Some(reg)`，`input_count == 1 && output_count == 1 && tool_call_count == 1` |
| 4 | stability | `missing_section_all_none` | `Config::default()` | `(None, None, None)` |
| 5 | stability | `partial_only_turn_timeout` | `[stability] turn_timeout_secs = 60` | `(None, None, Some(60s))` |
| 6 | stability | `stall_uses_default_check_interval` | `stall_timeout_secs = 120` 单字段 | `(Some(StallConfig { timeout: 120s, check_interval: 30s }), None, None)` |
| 7 | stability | `full_section_all_some` | 4 字段全填 | 三 Option 全 Some + 字段精确匹配 |
| 8 | fallback_llm | `missing_section_returns_none` | `Config::default()` | `None` |
| 9 | fallback_llm | `self_reference_returns_none` | `provider = "anthropic"` 且 primary_provider_key = `"anthropic"` | `None` |
| 10 | fallback_llm | `unknown_name_returns_none` | `provider = "ghost"` 不在 providers map | `None` |
| 11 | fallback_llm | `valid_name_returns_some` | `[providers.mock]` + `provider = "mock"` + primary_provider_key = `"anthropic"` | `Some(_)`（用 `MockProvider`） |
| 12 | fallback_llm | `create_provider_failure_returns_none` | invalid `ProviderConfig` | `None` |

补充 schema roundtrip 测试 2 个（`src/config/types/phase6_wiring.rs::tests`）：
- `empty_toml_yields_none_for_three_sections`
- `full_toml_yields_three_some`

## 9. Commit 拆分（建议 6 commits，合并原 P6-6）

| # | Scope | 文件 | 测试守门 | 行为变化 |
|---|-------|------|---------|---------|
| **P6-1** | docs: writing-plans 产出 plan 文档 | `2026-05-08-phase6-config-wiring-plan.md` | — | 无 |
| **P6-2** | config: schema | `phase6_wiring.rs` (新) + `types/mod.rs` re-export + `structs.rs` 3 字段 | schema roundtrip ×2 | 字段不被读，无运行时变化 |
| **P6-3** | guardrails: build + wire | `orchestrator_init.rs` (+`build_guardrail_registry` + `&Config` 参 + line 130 改) | 测试 #1-#3 | `[guardrails] enabled=true` 首次激活 PII guardrail |
| **P6-4** | provider: build + wire | 同文件（+`build_fallback_llm` + line 131 改） | 测试 #8-#12 | `[fallback_provider]` 首次激活 single-step fallback |
| **P6-5** | stability: build + wire 3 fields | 同文件（+`build_stability_triple` + line 132-134 改） | 测试 #4-#7 | `[stability]` 首次激活 P0 rescue |
| **P6-6** | docs: ship | `CHANGELOG.md [Unreleased]` + master spec Phase-6 翻 🟢 + Stage 7 plan 加 "Phase-6 closed" 收尾 | — | 无代码 |

每个 commit 独立可 revert；行为变化全 opt-in。

## 10. 验收映射

| 验收项 | 守门测试 / 检查 |
|--------|----------------|
| ① 5 section 全填 → 5 字段全 Some | 测试 #3 + #7 + #11 |
| ② 缺 section → 5 字段全 None | 测试 #1 + #4 + #8 |
| ③ `cargo test -p alephcore --lib` 全绿 | CI |
| ④ `init_audit.rs` 三个 Stage 7 测试不退化 | 不改 `emit_init_seams` 签名，自动满足 |
| ⑤ 启动时间 < 1.05× baseline | 三 builder 全是 `Option` 取值；`PiiSecretsGuardrail::from_globals()` 已在 `http_provider.rs` / `runtime_guard.rs` 调用过，无新引导开销 |
| ⑥ R10：`agent.rs` 行数不变 + `harness/` 文件数 ≤ 9 | 全部代码在 `src/config/` + `src/bin/aleph-server/commands/start/`，每 commit 后 `wc -l src/harness/agent.rs && ls src/harness/` 自查 |

## 11. 红线 self-check

| 红线 | 规避策略 |
|------|---------|
| **R1** Brain-Limb 分离 | 无原生 API 调用 |
| **R7** LLM 主权 | 纯配置加载，无推理替代 |
| **R8** 工具即一切 | 不增工具，不动 |
| **R10** 薄 Harness | `src/harness/` 0 行变化；`agent.rs` 0 行变化 |
| **subagent path** | `subagent_spawner.rs` 不动，4 字段保持 None |
| **Stage 6b defer** | 不新增 JudgeVerifier / ComputationalVerifier |

## 12. 开放问题（plan 阶段决议）

| # | 问题 | 备选 |
|---|------|------|
| O1 | `primary_provider_key` 怎么传？ | (a) 接受 type-name 弱语义比较；(b) caller 链透传 toml key — **推荐 (b)** |
| O2 | `config: &Config` vs `Arc<Config>` 在 caller 链 | grep `initialize_orchestrator(` 调用点确认 |
| O3 | `MockProvider` 在测试 #11 是否需要 ProviderConfig fixture | grep `MockProvider::new` 现有用法 |

## 13. 风险与缓解

| 风险 | 触发条件 | 缓解 |
|------|---------|------|
| `orchestrator_init.rs` 行数膨胀 | +3 builder ≈ +60 行 → ~210 行 | 不触红线（R10 只盯 `src/harness/agent.rs`）；如未来再涨可拆 `wiring.rs` 子模块 |
| toml roundtrip serde 行为 | `Option<XxxToml>` 缺 key | 复用 `Config::behavior: Option<BehaviorConfig>` 同 idiom，已 production-validated |
| `PiiEngine::global()` 返 None 在测试环境 | 测试 #3 | `PiiSecretsGuardrail` 内部已处理（仅装 SecretLeakDetector） |
| `create_provider` 在测试中需要真 ProviderConfig | 测试 #11 / #12 | 用 `MockProvider`（`src/providers/mock.rs:76` 已存在） |

---

**完成态**：所有 6 commits 推到 main 后，aleph.toml 写入完整三 section 的用户启动 aleph-server，`AgentHarnessRunner` 5 字段全部 Some；缺 section 的用户行为等同 Stage 7 ship 后 main HEAD。Phase-6 闭环。
