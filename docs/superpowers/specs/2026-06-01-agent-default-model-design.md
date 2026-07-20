# Agent 默认 Model 机制重构 — 设计文档

- **日期**: 2026-06-01
- **状态**: 已与用户确认设计,待写实现计划
- **方案**: A(复用 `ModelOverride` 语义,agent 存结构化 model 引用)

## 1. 背景与问题

当前 agent 的 model 配置存在以下问题:

- `AgentDefinition.model: Option<String>` 是**自由字符串**,用户需手填 model 名,无校验。
- panel 侧 `agents.update` 的 `AgentPatch` **根本没有 model 字段** —— UI 改不了 agent 的 model(现存缺口)。
- 没有「选中的 model 被删除后自动回退」的语义。

## 2. 目标行为(用户确认)

1. **agent 默认继承系统级默认 model**。
2. **可自定义**:自定义时不是手填字符串,而是**从「已配置的 model」里选**(panel 手动选,数据源 = `providers.catalog` view=configured)。
3. **删除兜底**:被选中的已配置 model 若 provider 被删 / 被禁用 / model 不在其 `models` 列表 → agent 自动回退系统默认 model。

### 关键约定

- **不引入新的「系统级默认 model」概念**。现有解析链
  `agent.model > defaults.model > profile.model > DEFAULT_MODEL`
  的尾端(`defaults.model > profile.model > DEFAULT_MODEL`)**就是**「系统默认 model」=「当前默认 model」。代码注释中点明即可。
- 回退 = 「当 agent 选中的 model 不可用时,行为等同于 `agent.model = None`」,自动落回上述尾端链。

## 3. 数据结构(§1)

`src/config/types/agents_def.rs`:`AgentDefinition.model` 升级:

```rust
/// agent 选中的 model。
/// `None` = 继承系统默认 model
/// (= defaults.model > profile.model > DEFAULT_MODEL,即“当前默认 model”)。
#[serde(untagged)]
pub enum AgentModelRef {
    /// 旧 config 的裸字符串 "claude-x" → 向后兼容,不做删除检测
    Legacy(String),
    /// panel 从 catalog 选的已配置 model,带 provider id 以便检测删除
    Qualified { provider: String, model: String },
}

// 字段:pub model: Option<AgentModelRef>   // None = 继承系统默认
```

设计要点:

- **untagged serde**:旧 `model = "claude-x"`(裸 TOML 字符串)自动落 `Legacy`;新结构
  `{ provider = "...", model = "..." }` 落 `Qualified`。**零破坏迁移,无需迁移脚本**。
- **分层(P4)**:`AgentModelRef` 定义在 `config/types`(底层)。gateway 的 `ModelOverride`
  改为复用/转换此类型,避免 config 反向依赖 gateway。

## 4. 解析 + 回退逻辑(§2)

`src/config/agent_resolver.rs:275-281` 改为:

```rust
let model = agent.model.as_ref()
    .and_then(|m| resolve_model_ref(m, &providers))  // 不可用 → None → 触发回退
    .or_else(|| defaults.model.clone())
    .or_else(|| profile.model.clone())
    .unwrap_or_else(|| DEFAULT_MODEL.to_string());
```

新纯函数(便于单测):

```rust
fn resolve_model_ref(m: &AgentModelRef, providers: &ProvidersConfig) -> Option<String> {
    match m {
        AgentModelRef::Legacy(s) => Some(s.clone()),     // 不校验
        AgentModelRef::Qualified { provider, model } => {
            // provider 存在 && enabled && model ∈ provider.models
            let p = providers.get(provider)?;
            if p.enabled && p.models.iter().any(|x| x == model) {
                Some(model.clone())
            } else {
                tracing::warn!(
                    agent_model = %model, provider = %provider,
                    "selected model unavailable (provider removed/disabled or model dropped), \
                     falling back to system default"
                );
                None  // 触发回退
            }
        }
    }
}
```

## 5. RPC 接线(§3)

补现存缺口:

- `AgentPatch` 加 `model` 字段,语义需区分「设为某 model」与「清除→继承默认」
  (用 `Option<Option<AgentModelRef>>` 或显式 clear 标志,实现期定)。
- `AgentSummary.model` / `agents.get` 返回类型同步为 `Option<AgentModelRef>`。
- gateway 在 RPC 边界做 `AgentModelRef ↔ ModelOverride` 转换(panel 用 tagged `kind`)。

涉及文件:`src/gateway/handlers/agents.rs`、`src/config/agent_manager/`。

## 6. Panel UI(§4)

agent 设置页加一个 model 下拉,**复用现有 `ModelPicker` + `providers.catalog(view=configured)`**:

- 顶部固定项「继承系统默认」→ 写 `None`。
- 其余项 = 已配置 model 列表 → 写 `Qualified { provider, model }`。
- 若 agent 当前存的 `Qualified` 已不在 catalog(被删/禁用)→ UI 显示「⚠ 已失效(回退默认)」灰条,
  选中态自动归到「继承默认」。

涉及文件:`interfaces/webchat/src/components/model_picker.rs`、agent 设置组件、
`interfaces/webchat/src/api/providers.rs`。

## 7. 兼容 / 迁移(§5)

- **无需迁移脚本**:untagged serde 让旧 `model="x"` 原地兼容为 `Legacy`。
- 旧 `Legacy` 值永不被删除回退逻辑触碰(只有 `Qualified` 才校验)。
- `AgentDefaults.model` / `profile.model` 保持 `Option<String>` 不动 —— 它们是可信回退源,
  不参与删除检测。最小改动。

## 8. 已裁决的边界(用户接受)

当前解析链最终只产出**一个 model 字符串**,运行时再由别处把字符串映射到 provider。
若两个 provider 配了同名 model,`Qualified` 里选的 provider 在路由阶段可能不被尊重。

**裁决**:本设计**保持产出 model 字符串**(行为不变、改动最小),provider 仅用于
「校验是否被删」。**provider 路由的精确尊重**作为可选后续,不在本次范围。

## 9. 测试计划

- **单元(纯函数)**:`resolve_model_ref` —— Legacy 直通、Qualified 有效命中、provider 不存在/
  禁用/model 不在列表 三种回退、与 untagged serde 往返(裸字符串 ↔ Legacy,结构 ↔ Qualified)。
- **解析链**:`agent_resolver` —— Qualified 失效时落到 `defaults.model` / `DEFAULT_MODEL`。
- **RPC**:`agents.update` 带 model patch 往返;清除→继承语义。
- **不破坏**:既有 `agent_resolver` / `agents_integration` 测试(`claude-opus-4-6` 等)仍通过。

## 10. R10 / 红线对齐

- 不在 harness 加逻辑;改动落在 config 解析层 + gateway I/O + panel I/O。
- `resolve_model_ref` 是确定性校验(provider 是否存在),非 LLM 越俎代庖,符合 R7。
- panel 仅 I/O 渲染(R2/R4),业务解析在 core。
