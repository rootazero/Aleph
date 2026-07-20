---
title: Harness 12-Module Roadmap (Master Spec)
status: draft
date: 2026-05-05
authors: ["claude-opus-4-7"]
scope: roadmap-only — 不写代码、不写 plan、不修改运行时
supersedes: none
follows: 2026-05-04-harness-stability-rescue-design.md
---

# Aleph Harness 12-Module Roadmap — Master Spec

> **目标**：在 P0 stability rescue（commit `bf0de41cc`）之后，沿"12 模块对照"路径
> 系统性补齐 Aleph Harness 与 `claude-code` 实现间的 gap，逐步逼近并超越 CC 的能力面。
>
> **非目标**：本文件本身**不**包含 12 个 stage 的完整设计或实施 plan。每个 gap stage
> 被认领时单独走 brainstorm → design → plan → 实施流程，与本路线图独立提交。

## 0. 红线与边界

### 0.1 这份 spec 是什么

- 一份 **路线图索引**（roadmap index）
- 涵盖 12 个 Harness 模块：**7 个 gap stage**（详细到 problem / sketch / acceptance）
  + **5 个 anchor entry**（一段说明 + 红线声明）
- 输出物：本单文件 `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`
- 用途：
  1. 下次会话开 stage 1 时直接读它即可知道该做什么
  2. 任何后续改动 harness 的人有明文红线（哪些模块"勿动"、哪些 stage 互为前置）

### 0.2 这份 spec 不是什么

- ❌ 不是 12 个 stage 的完整 design.md（每个 stage 被认领时再单独 brainstorm）
- ❌ 不会预先冻结接口、struct 字段、文件改动清单（属于子 stage design 的工作）
- ❌ 不会写代码、不会写 plan、不会做 verifier
- ❌ 不替代 P0 rescue 已经留下的 spec/plan/changelog

### 0.3 修订规则（详见第 5 节）

- 一次 commit 定稿
- 每个 stage ship 后追加一行 `✅ Shipped: <hash> on <date>`（轻量修订）
- 依赖被实证证伪 / 新发现 P0 缺陷插队 / scope 边界证伪 → 走正式修订（重新 brainstorm + commit）

### 0.4 全局约束

- 单个 stage 的最终 PR ≤ 600 行（含测试）。超出则 stage 自身要拆。
- 任何 stage 触及 anchor 模块（#1 / #3 / #4 / #6 / #7）需在 stage design 明文说明影响并通过 verifier 证明无 regression。
- 跳过依赖直接做下游 stage 禁止（如 Stage 4 在 Stage 3 未 ship 前不能开始）。
- R10 薄 Harness 边界基线现状：R10 nominal 写"`src/harness/` ≤ 9 文件 / ~1500 行"；P0 rescue 后实际为 10 文件 / 2344 行（agent.rs 1340 行 + 9 个支持文件 ~1004 行），R10 的字面数字已是历史指标。本路线图沿袭 R10 精神（薄 Harness）但显式重设运行基线：
  - **harness/ 总行数基线**：2344 行（post `bf0de41cc`）；本路线图希望长期收敛回 R10 nominal，但短期允许合理波动
  - **harness/ 单 stage 增量预算**：≤ +400 行（含测试）。超出则 stage 必须拆
  - **agent.rs 子约束**：≤ 1500 行（R10 nominal 1500 在循环核心文件上的合理映射；现 1340 行剩余预算 160 行）
  - **harness/ 文件数预算**：10 → 12 上限（仅当 stage design 论证新文件不可避免时）。优先考虑把新代码放进 src/guardrails/ / src/verification/ / src/tools/ 等 harness 外部模块

---

## 1. 路线图依赖图

### 1.1 拓扑

```
Stage 1 (ErrorClass) ──┐
                       │
Stage 2 (Tools)      ──┼──> Stage 5 (Guardrails) ──┐
                       │                            │
                       │                            ├──> Stage 6 (Verification) ──> Stage 7 (Init Audit)
                       │                            │
Stage 3 (Prompt) ──────┴─> Stage 4 (Subagent) ─────┘
```

### 1.2 顺序总表

| Stage | 模块 | Risk | Depends on | 量级估算 |
|-------|------|------|------------|----------|
| 1 | #8 Error Handling Classification | low | — | ~150 行 |
| 2 | #2 Tools Surface Unification | medium | — | ~300 行 |
| 3 | #5 Prompt Assembly Seam | medium | — | ~250 行 |
| 4 | #11 Subagent ChainContext Wiring | low | Stage 3 | ~150 行 |
| 5 | #9 Guardrails Pipeline | high | Stage 1, 2 | ~500 行 |
| 6 | #10 Verification & Feedback Loop | high | Stage 1, 3, 4, 5 | ~500 行 |
| 7 | #12 Initialization Audit | medium | Stage 1-6 | ~200 行 |
| **小计 (gap)** | | | | **~2050 行** |

| Anchor | 模块 | 状态 |
|--------|------|------|
| A1 | #1 Orchestration Loop | 健康（P0 rescue 后） |
| A2 | #3 Memory | 健康（Spec A/B/C 已 ship） |
| A3 | #4 Context Management | 健康（已接入） |
| A4 | #6 Tool Calling / Structured Output | 健康（原生 tool_use） |
| A5 | #7 State & Checkpointing | 健康（事件溯源 + sqlite） |

### 1.3 依赖理由摘要

- **Stage 1-3 无相互依赖**，但建议按 1→2→3 顺序做：Stage 1 提供"错误词汇" / Stage 2 是 Guardrails 前置 / Stage 3 是 Subagent 前置。
- **Stage 4 在 Stage 3 之后**，因为 ChainContext 接入路径会触发 prompt 改造（subagent 的 system prompt 需要 PromptBuilder 装配）；先 #5 再 #11 避免回头改 #5。
- **Stage 5 在 Stage 1 + 2 之后**，因为 ToolCallGuardrail 需要稳定的 ToolService surface（Stage 2），且 GuardrailDecision 用 ErrorClass 表达（Stage 1）。
- **Stage 6 是收口节点**，依赖最多：JudgeAgent 是 subagent (Stage 4)、用 PromptBuilder (Stage 3)、用 OutputGuardrail (Stage 5)、用 ErrorClass 反馈 (Stage 1)。
- **Stage 7 放最后**：装配审计需要前 6 个 stage 引入的所有零件就位才能端到端审。

### 1.4 跨 stage 收纳的 P1/P2 fix（不单独成 stage）

| Fix | 原 priority | 收纳 stage |
|-----|-------------|-----------|
| Stop hook 仅在模型停手触发（tool_use 死循环不覆盖） | P1 | Stage 6（行为扩展） |
| `run_turn` O(n) 事件扫描 | P2 | Stage 4 或 Stage 7 内部 sub-task（哪个 stage 真正动到事件读取就顺手处理） |

---

## 2. Gap Stages（详细条目）

每个 stage 条目按统一模板。

---

### Stage 1 — Error Handling Classification (#8)

**Status**: ✅ Shipped d5fc9d159 on 2026-05-05 · plan: docs/superpowers/specs/2026-05-05-harness-stage1-error-class-plan.md
**Depends on**: 无
**Risk class**: low

**Problem (现状缺陷)**
- `HarnessError`（src/harness/trait_def.rs）和 `AlephError` 缺少错误分类（瞬时 / 可恢复 / 可修复 / 意外）。
- Guardrails 决策、Verification 反馈通道、上层重试策略都没有共享词汇，目前只能通过 `match` 各 variant 推断。
- P0 rescue 引入的 `consecutive_failure_cap` 计数逻辑（agent.rs run loop）按 variant 类型粗略统计，应升级为按 class 决策。

**Solution sketch**
- 引入 `ErrorClass { Transient, Recoverable, Fixable, Unexpected }` 枚举。
- `HarnessError` 各 variant 标注分类（`fn class(&self) -> ErrorClass` 方法或 `const` 表）。
- `AlephError` 在 boundary 处映射到 `ErrorClass`。
- `consecutive_failure_cap` 改为按 class 决策（如 Transient 不计入 cap，Fixable 计入但允许更多次）。

**Allowed seams**
- `enum ErrorClass`
- `impl HarnessError { pub fn class(&self) -> ErrorClass }`
- 必须 ≥1 真实消费者：consecutive cap 决策、未来 Stage 5 GuardrailDecision、Stage 6 verification trace 字段。

**Old code to retire**
- agent.rs 中针对 `HarnessError::Tool` / `HarnessError::Stalled` 等 variant 的 ad-hoc 分支判断，替换为按 class 派发。
- consecutive cap 中 hardcoded variant 比对逻辑。

**Acceptance criteria**
- 功能：`HarnessError` 全部 variants 有 class 分类；`AlephError` 在 boundary 处可映射。
- 不破坏：P0 rescue 行为（act 错误回流、consecutive cap 触发条件、watchdog timeout）保持等价。
- 测试：≥3 个 unit 验证 class 映射；≥1 个集成验证 cap 仍按预期触发。
- 性能：分类开销 ≤ 1 个 enum match（编译期常量化优先）。

**Future-proof note**
- 模型升级后错误模式可能改变，但分类词汇稳定（Transient 永远是网络/超时类，Fixable 永远是模型可读懂的工具失败）。是基础设施而非认知层 ── 通过 R10 Future-Proof Test。

---

### Stage 2 — Tools Surface Unification (#2)

**Status**: ✅ Shipped eee6fd70a on 2026-05-05 · plan: docs/superpowers/specs/2026-05-05-harness-stage2-tools-surface-plan.md
**Depends on**: 无
**Risk class**: medium

**Problem (现状缺陷)**
- agent.rs:158-170 每轮重新拉取并转换工具列表为 dispatcher 格式，浪费 + 耦合。
- src/tools/ + src/builtin_tools/ + src/dispatcher/ 三处分裂的 schema 表示形式互相转换。
- ToolService 没有暴露稳定的 dispatcher-ready 接口。

**Solution sketch**
- ToolService 在 builder/init 阶段一次性产出 `Arc<[DispatcherTool]>` 并 cache。
- AgentHarnessRunner 持有 `Arc<[DispatcherTool]>` 引用。
- agent.rs 每轮直接 `Arc::clone`（O(1) 引用计数操作）而非格式转换。
- 评估三处 schema 表示能否统一为同一 `DispatcherTool` 类型；如不能，至少让转换函数收敛到一处。

**Allowed seams**
- `ToolService::dispatcher_schema() -> Arc<[DispatcherTool]>`
- 可能新增内部类型 `DispatcherTool` 作为统一表示
- 必须 ≥1 真实消费者：AgentHarnessRunner / agent.rs run_turn

**Old code to retire**
- agent.rs:158-170 的 `to_dispatcher_format` 调用。
- src/dispatcher/mod.rs 中重复的 schema 转换函数（如确认无外部消费者）。
- src/tools/ 与 src/builtin_tools/ 之间冗余的 schema 适配代码。

**Acceptance criteria**
- 功能：每轮工具列表获取 `O(1)` clone 而非 `O(n)` 转换。
- 性能：per-turn schema 转换次数从 N 次（每轮）降到 0 次；perf assertion 在测试中固化。
- 不破坏：所有现有 tool 调用语义不变；ToolService 公开 API 不破坏外部消费者（agents/, dispatcher/, gateway/ 等）。
- 测试：≥2 个集成（工具调用端到端）+ ≥1 个 perf assertion + ≥1 个 property test 验证不同 tool 集合下序列化稳定。

**Future-proof note**
- 模型支持的 tool schema 格式（Anthropic / OpenAI / Gemini）变化时，cache 层是稳定 seam，不需要每次都改 agent.rs。属于 R10 通过项。

---

### Stage 3 — Prompt Assembly Seam (#5)

**Status**: ✅ Shipped 2026-05-05 · last functional commit `3ed7390cf` · plan: `docs/superpowers/specs/2026-05-05-harness-stage3-prompt-builder-plan.md`
**Depends on**: 无
**Risk class**: medium

**Problem (现状缺陷)**
- agent.rs:602 `build_prompt` 是私有函数，硬编码组装顺序（system / memory / tools / history / hint）。
- 新增 memory 注入升级、skill 提示、chain context、persona 等内容都要改主路径，违背 OCP（P3 可扩展性）。
- 无 PromptBuilder seam，下游 Stage 4 / 6 想注入内容只能 patch agent.rs。

**Solution sketch**
- 引入 `PromptBuilder` trait：`fn assemble(&self, ctx: &TurnContext) -> Result<Vec<MessagePart>>`。
- 实现 `DefaultPromptBuilder`：保持当前 `build_prompt` 的字节级行为。
- AgentHarnessRunner 通过 `dyn PromptBuilder` 调用；构造时支持 `.with_prompt_builder()` 注入自定义实现。
- `TurnContext` 五段 input struct（system / memory / tools / history / hint）作为传参容器。

**Allowed seams**
- `trait PromptBuilder`
- `struct DefaultPromptBuilder`
- `struct TurnContext`
- 必须 ≥1 真实消费者：agent.rs 主循环

**Old code to retire**
- agent.rs:602 `build_prompt` 函数（迁入 `DefaultPromptBuilder`）。
- 任何在 agent.rs 内部直接拼装 prompt 字符串的代码。

**Acceptance criteria**
- 功能：AgentHarnessRunner 暴露 `.with_prompt_builder(...)` 构造点；`DefaultPromptBuilder` 行为与原 `build_prompt` 字节级一致。
- 不破坏：现有 system prompt 内容、Memory 注入路径、Tools 列表注入完全不变。
- 测试：≥2 个 prompt golden test（DefaultPromptBuilder 输出 == 旧 build_prompt 输出）+ ≥1 个 property test 验证 TurnContext 任意排列下的稳定性。
- 性能：trait dispatch 开销 ≤ 1 个 vtable 跳转（无额外 alloc）。

**Future-proof note**
- 模型升级后 prompt 工程会变（更短 / 更长 / 不同 format / 多模态），seam 稳定不需要改 harness 主循环。R10 通过。

---

### Stage 4 — Subagent ChainContext Wiring (#11)

**Status**: ✅ Shipped 2026-05-05 · plan: docs/superpowers/specs/2026-05-05-harness-stage4-subagent-chain-plan.md
**Depends on**: Stage 3
**Risk class**: low

**Problem (现状缺陷)**
- src/harness/chain_context.rs (156 行) 已实现但 agent.rs 不持有也不传递。
- src/agents/subagent_spawner.rs 自行构造独立 chain context，subagent 谱系断链。
- Stage 6 JudgeAgent 需要在 chain 中可见，否则无法被 trace 追溯。

**Solution sketch**
- AgentHarnessRunner 持有 `ChainContext`（构造时注入或 default 初始化）。
- `spawn_subagent` 时通过 trait_def 传递 parent chain，subagent 在内部链入。
- subagent_spawner.rs 移除独立构造路径，统一从 AgentHarness 取 chain。

**Allowed seams**
- `AgentHarness::chain_context() -> &ChainContext` accessor
- trait_def 加 chain context propagation 方法
- 必须 ≥1 真实消费者：subagent_spawner / future JudgeAgent

**Old code to retire**
- subagent_spawner.rs 中独立构造 chain context 的代码段。
- 任何以"我没有上层 chain"为前提的 fallback 路径（如有，需保留 root agent 无 chain 的合理行为）。

**Acceptance criteria**
- 功能：subagent 调用链中每个 agent 能追溯到根（chain depth 可查询）。
- 不破坏：单 agent 调用（无 subagent 场景）行为不变；root agent 不强制要求 chain。
- 测试：≥1 个集成验证 3 层 subagent 谱系完整 + ≥1 个验证根 agent 无显式 chain 时行为合理 + ≥1 个 loom（跨线程 spawn 安全）。
- 顺手 sub-task：如果实施时触及 `run_turn` 事件读取路径，处理 P2 #11 O(n) 事件扫描。

**Future-proof note**
- 模型支持更长 subagent 链（10+ 层）时，链路追溯仍是 `O(1)` 上行查询，不会随深度退化。R10 通过。

---

### Stage 5 — Guardrails Pipeline (#9)

**Status**: ✅ Shipped on 2026-05-06 · 5a plan: docs/superpowers/specs/2026-05-05-harness-stage5a-guardrails-pipeline-plan.md · 5b plan: docs/superpowers/specs/2026-05-06-harness-stage5b-guardrails-toolcall-fallback-plan.md · all three callsites (Input + Output + ToolCall) live + on_model_fallback wired via `HarnessDeps.fallback_llm`
**Depends on**: Stage 1 (ErrorClass), Stage 2 (Tools surface)
**Risk class**: high

**Problem (现状缺陷)**
- src/verification/ 仅有 `stop_hooks`（Done 前），无输入 / 输出 / 工具调用三方位护栏。
- src/security/pii / src/security/secrets 模块存在（具体路径在 stage design 时确认）但未接入 harness 入口。
- callback.rs:67 `on_model_fallback` callback 接口已留，agent.rs 从不触发，是死接口。
- 长时运行 + R5 主动到达场景下，模型会把残留敏感数据回流到 prompt。

**Solution sketch**
- 引入 `Guardrail` trait 三方位：
  - `InputGuardrail`：turn 入口（用户消息进入前）
  - `OutputGuardrail`：turn 出口（模型输出递交给 channel 前）
  - `ToolCallGuardrail`：tool dispatch 前后
- 每方位至少一个 `PiiSecretsGuardrail` 实作，复用现有 pii/secrets 模块。
- `GuardrailDecision { Allow, Sanitize(Replacement), Block(ErrorClass), Warn }`，决策按 Stage 1 ErrorClass 分类。
- `GuardrailRegistry` 注册各方位 guardrail；harness 在三处 callsite 询问 registry。
- `on_model_fallback` callsite 接通到 ProviderRegistry 的备用模型列表（当 primary provider 持续 Transient 失败时触发）。

**Allowed seams**
- `trait InputGuardrail` / `trait OutputGuardrail` / `trait ToolCallGuardrail`
- `struct GuardrailRegistry`
- `enum GuardrailDecision`
- 必须 ≥1 真实消费者：harness 三处 callsite + PiiSecretsGuardrail impl

**Old code to retire**
- callback.rs:67 死接口（变为活路径）。
- 任何在 agent.rs 中临时硬编码的 PII 过滤逻辑（如有）。
- pii/secrets 模块中可能存在的"自启动监听"路径（如有），统一归并为 Guardrail impl。

**Acceptance criteria**
- 功能：
  - 输入注入的敏感数据被 InputGuardrail 在 turn 入口拦截并 Sanitize 或 Block
  - 模型输出敏感数据被 OutputGuardrail 拦截
  - 工具调用敏感参数被 ToolCallGuardrail 拦截
  - on_model_fallback 在 provider 持续 Transient 失败时被调用，切到备用 provider
- 不破坏：无敏感数据的常规对话不受影响（noop 路径零开销，benchmark 可验）。
- 测试：≥3 个集成（每方位至少 1 个）+ ≥1 个 fallback 触发测试 + ≥1 个 loom（registry 并发安全）+ ≥1 个 noop 性能 benchmark。
- Rollback note（high risk 必填）：每个 guardrail 注册支持运行时关闭（feature flag 或 GuardrailRegistry::disable_all()），便于 ship 后单独 revert 而不动 P0 rescue。

**Future-proof note**
- Guardrail trait 是稳定接口；具体 impl（PII / secrets / safety classifier）是可替换实现，模型变强后接更复杂 classifier 不需要改 trait。R10 通过。

**Sub-stage 拆分预案**
- 实施时若实测 ≥600 行，按以下边界拆：
  - 5a：InputGuardrail + OutputGuardrail + GuardrailRegistry + PiiSecrets impl
  - 5b：ToolCallGuardrail + on_model_fallback callsite 接通

---

### Stage 6 — Verification & Feedback Loop (#10)

**Status**: 🟢 6a Shipped on 2026-05-06 · 6b **Permanently Deferred (R7+R8+R10 incompatible)** on 2026-05-08 · 6a plan: docs/superpowers/specs/2026-05-06-harness-stage6a-turn-verifier-plan.md · 6a ships TurnVerifier trait + StopHookVerifier (1:1 migration) + ToolLoopVerifier (default threshold 5) at a single Think→Act callsite; closes the § 1.4 P1 fix. 6b (JudgeVerifier + ComputationalVerifier) was reviewed against R7 (LLM Sovereignty) / R8 (Everything-is-a-Tool) / R10 笨循环 5 个不 #3 (no Rust completion judgment) + #4 (no Rust content review) and rejected — cognitive judgment lives in the prompt (`VERDICT: PASS|FAIL|PARTIAL` at `src/thinker/layers/agent_role.rs`), not in `src/verification/`. Re-opening 6b requires rewriting the redline in `src/verification/mod.rs` first.
**Depends on**: Stage 1, Stage 3, Stage 4, Stage 5
**Risk class**: high

**Problem (现状缺陷)**
- src/verification/stop_hooks.rs 仅在模型停手时触发（agent.rs:212-247）。模型死循环 tool_use 时 stop hook 永不上场，只能靠 max_iterations 或 P0 rescue 的 consecutive cap 兜底。
- 无 judge agent / 计算式验证回路。
- `MAX_STOP_HOOK_VETOS=10` 上限只在 stop hook 触发后才生效。

**Solution sketch**
- 三件套：
  1. **TurnVerifier trait**：在每轮 think→act 之间生效（不再只是 Done 前）。`StopHookVerifier` 迁现有 stop hook 行为。覆盖 tool_use 死循环检测（无 thinking 文本、纯重复 tool call N 轮）。
  2. **JudgeVerifier**：用 Stage 4 的 subagent 容器 + Stage 3 的 PromptBuilder，可选地在长会话尾段（>K 轮）评估输出。
  3. **ComputationalVerifier**：基于 Stage 1 ErrorClass + Stage 5 OutputGuardrail 的 trace，自动检测"模型说做了 X 但工具调用 trace 中没 X"。
- agent.rs 主循环在 think→act 间调用 `verifier_chain.verify(turn_state)`，决策为 Continue / Veto / Cancel。

**Allowed seams**
- `trait TurnVerifier`
- impls: `StopHookVerifier`（迁）/ `JudgeVerifier` / `ComputationalVerifier`
- `struct VerifierChain`（按顺序依次询问，short-circuit on Veto）
- 必须 ≥1 真实消费者：agent.rs 主循环

**Old code to retire**
- src/verification/stop_hooks.rs 的"只在 Done 前"路径合并入 `TurnVerifier::verify` seam。
- agent.rs 中针对 stop hook 的特殊路径（替换为 `VerifierChain` 调用）。
- P0 rescue 中 consecutive_failure_cap 与 stop hook 的双重判定收敛为 verifier chain 的统一决策。

**Acceptance criteria**
- 功能：
  - tool_use 死循环（无 thinking 文本、纯重复 tool call）被 TurnVerifier 在 N 轮内识别并触发 fallback
  - JudgeAgent 在长会话（>K 轮，K 在 design 时定）尾段被调用一次
  - ComputationalVerifier 能识别 trace 中的 say-do mismatch
- 不破坏：现有 stop hook veto 行为（≤10 vetos）通过 StopHookVerifier 完全保留。
- 测试：≥2 个集成（tool_use 死循环检测 / JudgeAgent 触发）+ ≥1 个 ComputationalVerifier 单元 + ≥1 个 loom（verifier chain 并发安全）。
- Rollback note（high risk 必填）：VerifierChain 支持空 chain 退化（行为等价 P0 rescue baseline），便于 single commit revert。

**Future-proof note**
- 验证策略集合可扩展（trait + `Vec<Box<dyn TurnVerifier>>`），新模型对应新策略时不需要改 agent.rs 主循环。R10 通过。

**Sub-stage 实测结果**
- 6a：TurnVerifier trait + StopHookVerifier 迁移 + tool_use 死循环检测 — ✅ Shipped 2026-05-06
- 6b：JudgeVerifier + ComputationalVerifier — ❌ Permanently deferred 2026-05-08（R7+R8+R10 不可兼容；详见 Status 字段及 `src/verification/mod.rs` 红线注释）

---

### Stage 7 — Initialization Audit (#12)

**Status**: 🟢 Shipped on 2026-05-08 · plan: docs/superpowers/specs/2026-05-08-harness-stage7-init-audit-plan.md · audit: docs/superpowers/specs/2026-05-08-harness-stage7-audit-report.md · 6 commits T1-T6 (`f13f355c6` plan + 6b permanent defer → `fae84fe9c` audit report → `83b26848c` 5 wiring gaps patched → `319bc4572` `TraceSink::on_init_seam` trait method + 9 emit calls → `ca6bc5f9b` 3 integration tests in `src/orchestrator/tests/init_audit.rs` → docs ship). Closes the 12-module roadmap. Production behavior unchanged at Stage 7 ship (5 new `AgentHarnessRunner` fields default `None`). **Phase-6 closed 2026-05-08** — three top-level toml sections ([guardrails] / [stability] / [fallback_provider]) drive the five fields via three boot-time builders in `orchestrator_init.rs`; missing section preserves Stage 7 behavior. Plan: `docs/superpowers/specs/2026-05-08-phase6-config-wiring-plan.md`.
**Depends on**: Stage 1-6 全部
**Risk class**: medium

**Problem (现状缺陷)**
- src/init_unified/ 等初始化路径在前 6 个 stage 引入 trait/struct 后是否被正确装配未审。
- 冷启动 trace 完整性、SubagentRunner / Guardrail / TurnVerifier / PromptBuilder 的 wiring 是否端到端可达，需要一次扫描确认。

**Solution sketch**
- 一次"装配审计 spec"：
  1. 读所有 init 路径（init_unified / AgentHarnessRunner builder / Gateway 启动序列）。
  2. 确认 Stage 1-6 引入的每个 seam 都有真实消费者（无悬挂 trait）。
  3. 冷启动 trace 含 Stage 1-6 各组件 init 事件（通过 TraceSink 端到端验证）。
  4. 如发现遗漏则补连线（不引入新抽象）。
- 顺手 sub-task：处理 P2 #11 `run_turn` O(n) 事件扫描（如前 6 stage 未触及）。

**Allowed seams**
- 无（纯 wiring + trace 字段补充）

**Old code to retire**
- 前 6 stage 之后变成"死接口"的 trait（如某 seam 没有任何 impl 注册），按 R10 删除。
- init 路径中遗留的占位 / fallback 路径（如启动时的"如果 X 不存在则 noop"被前面 stage 全部消除）。

**Acceptance criteria**
- 功能：冷启动 trace 含全 12 模块组件 init 事件。
- 不破坏：启动时间 < 1.05 × baseline（baseline = bf0de41cc commit 测得）。
- 测试：≥1 个端到端启动 trace assertion + ≥1 个 init 时序完整性测试。

**Future-proof note**
- 装配是 wiring 不是认知，模型升级与本 stage 无关。R10 通过。

---

## 3. Anchor Stages（已健康模块，路线图不修改）

每个 anchor 共享同一 redline：**任何 stage 触及该模块需在 stage design 明文说明影响并通过 verifier 证明无 regression**。

---

### Stage A1 — Orchestration Loop (#1) · ANCHOR

**Current state**: src/harness/agent.rs (1340 行) Think→Act 笨循环。P0 rescue (`bf0de41cc`) 后 watchdog / per-turn timeout / TraceSink fire points / act 错误回流 / consecutive failure cap 均已就位。harness 模块共 2344 行 / 10 文件。

**Spec history**: `docs/superpowers/specs/2026-05-04-harness-stability-rescue-design.md`（已 ship）

**Redline**: 任何 stage 触及 agent.rs 主循环（`run` / `run_turn_internal` / `act`）需在 design 明文说明，并通过 verifier 证明 P0 rescue 行为不退化。运行基线见 Section 0.4：agent.rs ≤ 1500 行（R10 在循环核心文件上的合理映射），harness/ 总行数 ≤ 2344 + 各 stage 累计 ≤ +400 行单 stage 增量。

**Re-open trigger**:
- agent.rs 超过 1500 行
- harness/ 总行数超过 3000 行（约为当前基线 + 7 stage 的最坏增量）
- think→act 简单循环不再适配新模型（如双向 streaming / 真实并行 thinking）
- `run_turn` O(n) 事件扫描实测成为瓶颈（>5ms p50）

---

### Stage A2 — Memory (#3) · ANCHOR

**Current state**: src/memory/ 三级记忆（Hot / Warm / Cold） + 事件溯源。Spec A (Curated Hot Memory) / Spec B (Session Search Summarization) / Spec C (Cross-Process Safety) 全部已 ship。

**Spec history**: 见 `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md` 中的 project_spec_a/b/c 条目。

**Redline**: 任何 stage 触及 memory 接口需保持 Spec A/B/C 接口不变。如需扩展（dream daemon 等），单开 spec。

**Re-open trigger**:
- dream daemon 需求落地
- memory 检索 P95 退化（具体阈值在 dream daemon spec 时定）
- 新模型支持 1M+ context 改变 hot/warm 比例

---

### Stage A3 — Context Management (#4) · ANCHOR

**Current state**: src/context/{budget, compact} 已接入 agent.rs:125-145。session_info / environment / memory_context 三个辅助模块就位。

**Spec history**: 散落于各历史 harness spec（无单独 context spec）。

**Redline**: budget / compact 的 trim 策略不在本路线图修改。如需新策略，在涉及它的 stage design 中明文说明。

**Re-open trigger**:
- 模型 context window 扩大到 1M+ 改变 budget 模型
- compact 触发频次实测过高（>1 次 / 100 turns 视为过高）

---

### Stage A4 — Tool Calling / Structured Output (#6) · ANCHOR

**Current state**: providers/adapter::NativeToolCall（原生 tool_use schema）。各 provider（Anthropic / OpenAI / Gemini）通过 adapter 转换为统一表示。

**Spec history**: 散落于各 provider migration spec。

**Redline**: NativeToolCall 接口不变。Stage 2 Tools surface 改 schema 但不改 NativeToolCall 表示。

**Re-open trigger**:
- 新增 provider 不支持 native tool_use，需要 fallback 到 structured JSON parsing
- 多模态 tool_use（图像 / 音频）支持需求落地

---

### Stage A5 — State & Checkpointing (#7) · ANCHOR

**Current state**: src/session/{events, store} 事件溯源 + sqlite 持久化。session_id / turn 事件 schema 稳定。

**Spec history**: 散落于各 session-related spec。

**Redline**: 事件 schema 不在本路线图修改。如需新增事件类型，作为 stage 内部 sub-task 追加（不改既有事件、不破坏回放兼容）。

**Re-open trigger**:
- 需要跨节点 checkpoint
- 事件回放速度退化（>10ms / 1000 events 视为退化）
- sqlite 替换为其他后端

---

## 4. 全局验收策略

每个 gap stage 的 verifier 必须满足以下五项强制条款 + 风险分级附加项。

### 4.1 强制条款（所有 7 个 gap stage 共有）

1. **Future-Proof Test (R10 必答)**：design 必须回答"模型升级一档后这个改动还需要吗？为什么需要 / 不需要？"。答案不能是"以防万一"。
2. **Old code retired**：在该 stage commit 中必须有 `git show` 可见的删除。不允许"先加新代码，旧代码下个 stage 删"的延迟清理。commit message 列出删除的文件:行号。
3. **No-regression**：
   - P0 rescue 行为不退化（act 错误回流 / per-turn timeout / TraceSink fire points / consecutive cap）
   - Anchor 模块（#1 / #3 / #4 / #6 / #7）API 不破坏
   - 现有 41 个 harness 测试全绿（baseline = bf0de41cc）
4. **Seam justified**：每个新引入的 trait / struct 在 stage commit 中必须有 ≥1 个真实 impl + ≥1 个真实 caller。零消费者的 seam 在 stage 完成时按 R10 删除（不留口）。
5. **Acceptance criteria 可机械验证**：design 写出的 acceptance 必须是测试 / cargo metric / trace 字段可验证的。禁止主观条目（如"代码更清晰"）。

### 4.2 风险分级附加项

| Risk class | 附加要求 |
|-----------|---------|
| low | 上述五项即可 |
| medium | + ≥1 个 property test（quickcheck / proptest）覆盖核心不变量 |
| high | + ≥1 个 loom 并发测试 + 一份 rollback note（说明如何 single commit revert 而不破坏 P0 rescue） |

按此分级，路线图中 high risk 的 stage 是 Stage 5 Guardrails 与 Stage 6 Verification。

### 4.3 Stage 7 Init Audit 附加：冷启动 trace 验证

- 启动后通过 TraceSink 抓到的 init events 必须涵盖 Stage 1-6 的全部新组件（按 stage design 中列出的 seam 名称匹配）。
- 启动时间 < 1.05 × baseline（baseline = bf0de41cc commit 测得）。

### 4.4 baseline 锁定

- harness 测试基线：bf0de41cc commit 处 41 个 harness 测试全绿。
- harness 模块体量基线：2344 行 / 10 文件（已偏离 R10 nominal 9 文件 / 1500 行；本路线图视为新运行基线，详见 Section 0.4）。
- agent.rs 体量基线：1340 行（路线图设上限 1500 行，对应 R10 nominal 1500 在循环核心文件上的映射，剩余预算 160 行）。
- 启动时间基线：bf0de41cc 处冷启动 timing（具体数值在 Stage 1 实施时通过 benchmark 锁定）。

---

## 5. 修订与生命周期

### 5.1 生成时

一次 `git commit` 定稿（commit message 锁定 12 stage 的初始顺序与依赖）。

建议 commit message:
```
docs(harness): add 12-module roadmap master spec

Indexes 7 gap stages + 5 anchor entries with dependency chain.
Locks scope discipline (surgical + necessary seam) and verifier
gates (future-proof / old code retired / no-regression / seam
justified / mechanically verifiable acceptance).

Refs: Agent Harness 12-module gap analysis (2026-05-04 ~ 05-05).
```

### 5.2 追加修改（轻量，无需重新 brainstorm）

允许直接 commit：
- 某 stage ship 后：在该条目末尾追加 `✅ Shipped: <hash> on <date> · design: <link>`
- 某 stage 中途发现的小幅 sub-task 增删（不破坏 acceptance）
- typo / 格式修正

### 5.3 正式修订（需要 brainstorm + commit）

以下情况需要重新 brainstorm 后再修订 master spec：
- 依赖被实证证伪（如 Stage 5 不真的依赖 Stage 2）
- 新发现的 P0 缺陷要插队（≤current Stage 1 的优先级）
- 某 stage 的 surgical+seam scope 边界被实证证伪（实施时发现需要拆 / 合并）
- Anchor 模块的 re-open trigger 被触发

### 5.4 禁止操作

- ❌ stage 已开始实施后修改其 acceptance criteria（避免移动验收门槛）
- ❌ 跳过依赖直接做下游 stage（如 Stage 4 在 Stage 3 未 ship 前开始）
- ❌ 将 anchor 模块降级为 gap stage 而不走正式 brainstorm
- ❌ 在本 master spec 内部维护 changelog 段（修订历史交给 git log）

### 5.5 文件落点

- 主文件：`docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`（本文件）
- 每个 gap stage 被认领时新增：`docs/superpowers/specs/<YYYY-MM-DD>-harness-stage<N>-<topic>-design.md`
- 实施 plan：同目录 `<...>-plan.md`
- 修订历史：通过 `git log` 追踪

---

## 6. 路线图量级与节奏

### 6.1 总量级估算

| 项 | 量级 |
|----|------|
| 7 gap stages 代码增量（含测试） | ~2050 行 |
| 7 gap stages 删除旧代码（每个 stage 必含） | 每 stage design 时确定（无下界，但禁止零删除） |
| anchor stages 文档维护 | 0 行（仅本 master spec 内） |
| **本 master spec 自身** | ~700 行 markdown |

### 6.2 节奏（参考，不强制）

- 单 stage 时间预算：1~3 个工作会话（含 brainstorm + design + plan + 实施 + verifier）
- 跨会话恢复：新会话开始时读本 master spec 即可定位下一个未 ship stage
- 串行 vs 并行：依赖链允许 Stage 1 / 2 / 3 并行（无相互依赖），但建议串行以保持每次会话 scope 收敛

### 6.3 跨会话恢复机制

新会话开始时按以下步骤定位：
1. `git log --oneline | grep harness` 找最近 harness commit
2. `grep -n "✅ Shipped" docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` 找已完成 stage
3. 第一个未 ship 且依赖已满足的 stage 即为下一个目标
4. 如该 stage 已有对应 design.md（同目录），直接进 plan / 实施；否则启动 brainstorm

---

## 7. 已知风险与缓解

| 风险 | 触发条件 | 缓解 |
|------|---------|------|
| Stage 5/6 single PR 超 600 行 | 实测 line count | 预案 sub-stage 拆分（5a/5b 与 6a/6b 已在条目中预留） |
| 前置 stage 实施后 anchor 模块意外 regression | verifier 漏测 | 每 stage commit 后回跑全 harness 测试套（41 测试基线）+ Stage 7 Init Audit 兜底 |
| 模型在路线图实施期间升级一档（如 Anthropic 出 Opus 5）| 时间因素 | R10 Future-Proof Test 预设答案：每个 stage 设计时已论证模型无关性，不需要重做 |
| Stage 6 JudgeAgent 引入额外 LLM 调用成本 | budget 关切 | JudgeAgent 触发条件（>K 轮）在 design 时按成本预算调整 K |
| harness/ 模块行数突破本路线图运行基线（>3000 行总体或单 stage > +400 行） | 实测 line count | 每 stage 必须报 harness/ 行数与 agent.rs 行数；超出 budget 立即拆分或撤回 |

---

## 8. 与 P0 stability rescue 的关系

- **承接**：本路线图是 P0 rescue 之后的下一阶段路线，不替代 P0 rescue 设计。
- **基线**：P0 rescue commit `bf0de41cc` 是所有 stage 的 no-regression baseline。
- **复用**：P0 rescue 引入的 `TraceSink` fire points、per-turn timeout、consecutive failure cap 是 Stage 1 (ErrorClass) 和 Stage 6 (Verification) 的现成基础。
- **冲突解决**：若实施 stage 时发现 P0 rescue 留下的某段代码可以更优雅地融入新 seam，按 stage 的 "Old code to retire" 流程处理（不破坏 P0 rescue 的对外行为）。

---

**End of Master Spec.**
