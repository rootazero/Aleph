# AI 动态路由：A+C 分层 + pin/prefer/unset — 设计与实施

- **日期**: 2026-06-03
- **状态**: 方案已确认；Phase 1（C 地板）已实施并通过测试，Phase 2 已规格化待实施
- **关联**: 接续 [2026-06-01-agent-default-model-design.md](./2026-06-01-agent-default-model-design.md) §8 deferred 的"provider 路由精确尊重"
- **参考**: `/Volumes/TBU4/Github` 下 RouteLLM / Semantic-Router / LiteLLM / Bifrost

## 1. 背景：从"全局默认 + 继承"到"AI 动态路由"

旧模型：全局默认 provider/model，agent 默认继承、可自定义。新方向：让模型按任务在已配置的
provider/model 间动态选择。问题是这与架构红线冲突：

- **R7（LLM 主权）** 禁止"用确定性代码或独立 LLM 调用做意图分类 → 选模型"的 dispatcher。
- **R9（智慧在 Prompt 中）** 要求零额外 LLM 调用、零中间件税。
- **R10（薄 Harness）** 禁止在 `src/harness/` 堆认知。

## 2. 三种路由形态对比与裁决

| 形态 | R7 | R9 | R10 | 裁决 |
|---|---|---|---|---|
| **A. 主循环 LLM 内联选**（catalog 进 prompt / `select_model` tool） | ✅ | ✅ | ✅ | **采纳** |
| **B. 独立常驻 router 模型**（请求前先调用一次分类→选模型） | ⚠️ 即被解散的 Dispatcher | ❌ 每请求 +1 LLM 调用 | ❌ 新认知层 | **拒绝** |
| **C. 轻量确定性能力地板**（按结构性事实过滤候选，不看 prompt 语义） | ✅ 基础设施可行性 | ✅ 零 LLM | ✅ 留在 providers 层 | **采纳** |

**结论：A + C 分层**。C 做"硬约束地板"（剔除结构上不可能服务请求的模型），A 做"智能软选择"
（LLM 看 catalog 自己定）。**拒绝 B** —— 它过不了 R10 的"面向未来测试"，模型越强它越是拖累。

参考项目印证此取舍：**Bifrost 故意不在选择层做能力过滤**，把 capability/cost 元数据当成"展示给
调用方(LLM)的 catalog 数据"，选择权交上游 —— 这正是 R7/R8 的正解。**LiteLLM 的
`_pre_call_checks`** 是事实上的 C 地板：按 context window / 支持的参数等**确定性元数据**过滤候选，
唯一看请求的地方是 token 计数（确定性度量，非语义分类）。RouteLLM / Semantic-Router 的训练分类器 /
embedding 意图路由是 R7 禁止的"意图分类路由器"，仅作反面教材。

## 3. agent provider/model 的语义：pin / prefer / unset

动态路由下，agent 的 model 字段从"硬绑定执行目标"退化为"路由的约束输入"：

- **pin**（`AgentModelRef::Qualified{provider, model}`）：强制使用，**不参与动态路由**。这是
  [2026-06-01 spec](./2026-06-01-agent-default-model-design.md) 已落地的语义。
- **prefer**：声明倾向，路由可在成本/能力/故障时覆盖。
- **unset**（`None`）：全权交给路由（C 地板筛可行候选 → A 由 LLM 选 / 落系统默认）。

全局默认 provider/model 退化为：路由无明确偏好或动态选择失败时的兜底 + failover 链末端。

## 4. Phase 1（已实施）：C 能力地板

**新增** `src/providers/capability_gate.rs` —— 纯函数模块，移植 LiteLLM `_pre_call_checks`，
消费已有 `model_catalog::capabilities`，是 `route_policy` 的兄弟（同样 prompt-blind、shape 候选集、
harness 无感知）。

- `RequestRequirements{needs_vision, needs_tools, input_tokens}`：从请求的**结构性事实**派生
  —— `content_blocks()` 扫 `ContentBlock::Image` → vision；`tools` 非空 → tools；
  `text_content().chars()/4` → token 估算。**绝不读 prompt 语义**（R7 安全线）。
- `capability_gate(req, caps) -> Keep | Drop(reason)`：context window / vision / tools 三维。
- `retain_capable_models(models, req)`：过滤候选模型列表，**fail-open**：未知能力的模型保留；
  若全被剔除则恢复原列表（不完整的静态表绝不能 hard-fail 请求）；unconstrained 时原样返回。

**接线**：`FailoverProvider::process` 在候选循环前从 payload 派生一次 `RequestRequirements`，
对每个候选的 `models` 列表过滤。**零 RequestPayload 字段改动**（需求全在现有 payload 字段里）；
unconstrained / 单 model 路径 byte-identical。

效果：带图请求自动跳过无 vision 的模型、超 context window 的模型被剔除 —— 让 unset/prefer 的
路由真正变"能力感知智能"，而非盲目按链顺序撞。

测试：12 个纯函数行为测试 + 1 个 failover 集成测试（带图请求只 dial gpt-4o，跳过 o1-mini），
25 failover 测试全绿，`cargo check`/`clippy -D warnings` 干净。

## 5. Phase 2（已实施）：pin/prefer 运行时 teeth + select_model tool

实施时的关键发现：模型绑定**不**走 run_loop 的 `resolved`（那只喂 Panel 显示 + health），而是
`harness_bridge::pick_llm` 通过 **provider wrapper（`ModelOverrideProvider`）stamp `payload.model`**
的机制（subagent_spawner 已用此法）。`BrainRef` 早有 `Default`(unset)/`Preferred`(prefer)/
`Strict{provider,model}`(pin) 三档骨架，但 `pick_llm` 只选 provider、**不 stamp model**（注释明写
"deferred to Phase 6"）。这才是 teeth 的咬合点。

实施分三块（均已提交、测试、`check`/`clippy -D warnings` 干净）：

1. **Keystone — pick_llm stamp model**（commit `20ede7aac`）：`BrainRef::Strict{provider, model:
   Some(m)}` 用共享的 `ModelOverrideProvider` 包住所选 provider，stamp `m`。把 subagent_spawner 的
   私有 wrapper 提升为 `src/providers/model_override_provider.rs`（熵减，一份两用）。这是 pin 的
   provider+model teeth 在绑定层真正生效。
2. **select_model tool（A 层，R8）**（commit `4662976bf`）：`select_model{model, provider?}` 让主循环
   LLM 一次推理里换模型，写进程级 `session_model_handle`（仿 `route_handle`，进程内、按会话、poison-safe）；
   `harness_bridge.run` 在 `pick_llm` 前读它 → 包 `ModelOverrideProvider`（复用 keystone）。绑定是 per-run，
   故下一轮生效（tool 回复里说明）。注册进 builtin catalog + tool group。**不引入独立 router 模型**（R7/R9）。
3. **agent pin teeth**（commit `7b81e57c4`）：`harness_bridge.run` 把 agent 的 `provider_hint`/`model_hint`
   折进绑定优先级：`select_model 会话 pick > agent pin > flow BrainRef preset`。让 markdown agent 声明的
   model 在主运行也生效（此前只影响 subagent spawn）。无 model_hint 的 agent 落 `pick_llm` —— byte-identical。

**优先级链**（`harness_bridge.run`）：session-model（select_model）→ agent provider_hint/model_hint →
flow BrainRef preset。前两者经 `ModelOverrideProvider` stamp；最后一档不变。

测试：keystone 6 + session_model_handle 2 + select_model tool 2 + 33 harness_bridge 全绿；
顺手把预存未分组的 `desktop_som` 补进 tool group（修复 `test_all_builtin_tools_have_a_group`，
已验证它在 main 上即未分组）。

### 剩余（可选后续）

- config.toml `AgentDefinition.model`（`AgentModelRef::Qualified` + 新 `prefer` 标志）→ AgentDef
  `provider_hint`/`model_hint` 的映射：目前 markdown agent 的 hint 已有 teeth；config-toml 定义的
  agent 走 `AgentInstanceConfig` 路径，其 `Qualified` 自动 pin 需补一条 resolver→registry 映射。
- 同名 model 跨 provider 的精确区分（[2026-06-01 §8]）在 pin 路径已天然解决（provider 显式），
  prefer/unset 仍按 C 地板 + failover。

## 6. 红线对齐

- **R7**：C 地板只读结构性事实（图/工具/token），非语义分类；A 层由 LLM 自主选，无确定性替代推理。
- **R9**：零额外 LLM 调用。
- **R10**：全部落在 `src/providers/`，`src/harness/` 零改动。
- **P7 防御性**：fail-open，静态能力表不完整或过期都不会 hard-fail 请求。
