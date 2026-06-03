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

## 5. Phase 2（待实施）：pin/prefer 运行时 teeth + A 层

Phase 1 让 unset/prefer 的**能力地板**生效。pin 的"provider 精确尊重"与 prefer/pin 的运行时区别
需要一个 provider-precise channel（[2026-06-01 spec §8](./2026-06-01-agent-default-model-design.md)
deferred 项），是独立一刀，规格如下：

1. **directive channel**：`ResolvedAgent` 增 `route_directive: RouteDirective`
   （`Pin{provider, model}` | `Prefer{model}` | `Auto`），由 `agent_resolver` 从 `AgentModelRef`
   （+ 新 `prefer` 标志，untagged serde 向后兼容）映射。
2. **payload 传递**：`RequestPayload` 增 `route: RouteDirective`（default `Auto` = byte-identical；
   约 12 处非测试构造点需补字段或 `..Default::default()`）。
3. **failover 消费**：`candidates()` 据 directive 调整 —— `Pin` 把指定 provider 强制置首并不向其它
   provider 漫游；`Prefer` 置首但保留 fallback；`Auto` 纯 C 地板。
4. **A 层**：`select_model` tool（R8，让主循环 LLM 显式改写后续轮次的 model）或 system prompt 注入
   gated catalog —— **不引入独立 router 模型**（R7/R9）。已有 `providers.catalog` RPC 已把
   capability/cost 暴露给 LLM，A 层大部分数据基建已就位。

**为何不在 Phase 1 一并做**：在 channel 落地前单加 `prefer` 标志或 payload 字段是 R10 禁止的
"零消费者抽象"。Phase 1 的 C 地板有真实消费者（failover 候选过滤），是非投机的地基。

## 6. 红线对齐

- **R7**：C 地板只读结构性事实（图/工具/token），非语义分类；A 层由 LLM 自主选，无确定性替代推理。
- **R9**：零额外 LLM 调用。
- **R10**：全部落在 `src/providers/`，`src/harness/` 零改动。
- **P7 防御性**：fail-open，静态能力表不完整或过期都不会 hard-fail 请求。
