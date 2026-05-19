---
title: Subagent Uplift Roadmap (Master Spec)
status: draft
date: 2026-05-08
authors: ["claude-opus-4-7"]
scope: roadmap-only — 不写代码、不写 plan、不修改运行时
supersedes: none
follows: 2026-05-08-phase6-config-wiring-design.md
---

✅ P1 Shipped: <local-only, no PR url> on 2026-05-08
✅ P2 Shipped: 37c5bb759 on 2026-05-09 (Stage E: cb5317474 + 99613bcb1 + 344a9623f · Stage F: 3a9b7abd5 · Stage G: d0223dd4c + 37c5bb759 polish)
✅ P3 Stage H Shipped: cfb2b358722089768d1c5f358b3525f9f4f94d62 on 2026-05-09
✅ P3 Stage I Shipped: 864f0e53a40d7fa4eaac883ed3665197aef8382a on 2026-05-09
✅ Stage J-pre Shipped: c56c5d014 on 2026-05-09 — cache observability pipeline; reassess Stage J fork branch on 2026-05-23 (≥2 weeks of trace data)
⚠️ Stage C (LaneScheduler) reverted 2026-05-19 (commits ae4f05532 + e0e29d886) — orphaned, never wired into the orchestrator/gateway. Replaced by a `tokio::Semaphore` on `SubagentTool`/`SpawnerBase` in 2026-05-19-subagent-hardening.
✅ Production wiring of Stages A/F/H/I closed 2026-05-19 (subagent-hardening branch) — see docs/superpowers/specs/2026-05-19-subagent-hardening-design.md. The run_loop.rs → SubagentTool → AgentRuntime hop, left at `None` defaults, is now wired: token accounting, parent-cancel propagation, trace_sink inheritance (→ background progress), AgentDef-driven worktree isolation. Two documented follow-ups remain: the per-agent MCP `plugin_registry` and the four resilience values need an ExtensionManager/Orchestrator storage change to reach the construction site.

# Aleph Subagent Uplift Roadmap — Master Spec

> **目标**：Phase-6（commit `4aa1c0f6d`，2026-05-08）把主 runner 的 5 个 HarnessDeps
> 字段（`guardrails / fallback_llm / stall_config / consecutive_failure_cap /
> turn_timeout`）从 `None` 接通后，`src/agents/subagent_spawner.rs:200-225` 仍有
> **10 个 HarnessDeps 字段是 `None`**；其中 5 个（`fallback_llm` /
> `stall_config` / `consecutive_failure_cap` / `turn_timeout` / `trace_sink`）的
> 主 runner builder 已在 Phase-6 + P0 rescue 中存在，可直接复用 —— 本路线图 Stage A
> 直击这 5 个字段。剩余 5 个 None 字段主 runner 自身亦未完整接通，列入 § 3 Anchor A3
> 等待 P4 后续路线图。其余 7 个子项沿"P1 还债 → P2 补齐 → P3 超越"三阶段，对照
> `claude-code` 已沉淀的子 agent 工程模式，系统性补齐 Aleph subagent 路径与主 runner /
> claude-code 间的 gap，并在 R10 薄 Harness + R7 LLM 主权 + Rust 类型安全的前提下
> 逼近并超越 CC 的 robustness。
>
> **非目标**：本文件本身**不**包含 P1/P2/P3 的完整设计或实施 plan。每个 P 阶段
> 被认领时单独走 brainstorm → design → plan → 实施流程，与本路线图独立提交。

## 0. 红线与边界

### 0.1 这份 spec 是什么

- 一份 **路线图索引**（roadmap index）
- 涵盖 10 个 subagent 子项目，分 3 个 phase（P1 还债 4 项 / P2 补齐 3 项 / P3 超越 3 项）
- 每个子项详细到 problem / solution sketch / allowed seams / old code to retire / acceptance / future-proof note
- 输出物：本单文件 `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`
- 用途：
  1. 下次会话开 P1 spec 时直接读它即可知道该 brainstorm 哪 4 个子项
  2. 任何后续改动 subagent 路径的人有明文红线（哪些模块"勿动"、哪些子项互为前置）
  3. P1/P2/P3 各自 design 完成后追加 `✅ Shipped: <hash> on <date>` 一行

### 0.2 这份 spec 不是什么

- ❌ 不是 P1/P2/P3 的完整 design.md（每个 phase 被认领时再单独 brainstorm + design + plan）
- ❌ 不会预先冻结接口、struct 字段、文件改动清单（属于 phase design 的工作）
- ❌ 不会写代码、不会写 plan、不会做 verifier
- ❌ 不替代 Phase-6 / Stage 4 / Spec C 等已 ship 的 spec/plan/changelog
- ❌ 不重新设计远程 a2a 协议（本路线图全部是 in-process subagent；远程 agent-to-agent 走 `src/a2a/`，是另一条独立线，见 § 3 Anchor A2）

### 0.3 修订规则（详见 § 4）

- 一次 commit 定稿
- 每个 phase ship 后追加一行 `✅ Shipped: <hash> on <date>`（轻量修订）
- 每个子项 ship 时在其 stage 条目下追加 `✅ Shipped: <hash>`（更轻量）
- 依赖被实证证伪 / 新发现 P0 缺陷插队 / scope 边界证伪 → 走正式修订（重新 brainstorm + commit）
- claude-code 上游若出现重大 subagent 改动 → 不自动追入；评估后走 P4 新 phase 流程

### 0.4 全局约束

| 约束 | 数值 / 规则 |
|------|------------|
| 单个 phase 的最终 PR ≤ | 1500 行（含测试，三个子项打包） |
| 单个子项的最终 commit ≤ | 600 行（含测试） |
| 跳过依赖直接做下游禁止 | 例：P2.E 在 P1.A 未 ship 前不能开始；P3.J 在 P2.E 未 ship 前不能开始 |
| 触及 anchor 模块（§ 3）需 | 在 phase design 明文说明影响 + verifier 证明零 regression |
| `src/agents/` 总行数基线 | 实施时 lock 当前 baseline；每 phase 增量 ≤ +600 行 |
| `src/agents/subagent_spawner.rs` 单文件预算 | ≤ 600 行（现 ~290 行）；超出则拆子模块 |
| 新增文件优先放进 | `src/agents/` / `src/scheduler/` / `src/sandbox/` / `src/extension/`，避免污染 `src/harness/`（R10） |
| 输入兼容硬约束 | `subagent` tool input schema 仅可加 optional 字段；现有字段不删 / 不改语义 / 不改默认值 |
| AgentDef 兼容硬约束 | `types.rs::AgentDef` 现有字段不删 / 不改语义；新字段必须 `#[serde(default)]` 或 `Option<T>` |
| `builtin_agents()` 注册项 | 不删 / 不改 id；可新增 |
| R3 / R7 / R10 | 零违反；任一 phase design 必须显式声明对三红线的影响（即使是 "无影响"） |

## 1. 路线图依赖图

### 1.1 拓扑

```text
P1 (还债 — 4 子项内部独立，建议同 PR 打包)
┌────────────────────────────────────────────────┐
│ A. HarnessDeps 同步至主 runner 水平              │
│ B. 显式递归 guard                               │
│ C. LaneScheduler 接入 spawner                    │
│ D. 父→子 cancellation 传播测试 + 修补            │
└────────────────────────────────────────────────┘
         │ (P1 完整 ship 后启动 P2 brainstorm)
         ▼
P2 (补齐 — 3 子项有跨 phase 依赖)
┌────────────────────────────────────────────────┐
│ E. 文件系统 agent 定义加载    ─── 依赖 B         │
│ F. Subagent 流式进度事件      ─── 依赖 A         │
│ G. 每 agent 工具集语义命名    ─── 依赖 B         │
└────────────────────────────────────────────────┘
         │ (P2 完整 ship 后启动 P3 brainstorm)
         ▼
P3 (超越 — 3 子项跨 phase 依赖密集，建议各自独立 PR)
┌────────────────────────────────────────────────┐
│ H. Worktree isolation         ─── 依赖 A, D      │
│ I. 每 agent MCP 范围          ─── 依赖 B, E      │
│ J. Fork-subagent prompt cache ─── 依赖 A, E      │
└────────────────────────────────────────────────┘
```

### 1.2 顺序总表

| Stage | 子项 | Phase | Risk | Depends on | 量级估算（含测试） |
|-------|------|-------|------|------------|-------------------|
| A | HarnessDeps 同步至主 runner 水平（5 字段：fallback_llm / stall_config / consecutive_failure_cap / turn_timeout / trace_sink） | P1 | low | — | ~200 行 |
| B | 显式递归 guard（`AllowlistToolService` 显式 block "subagent" + `AgentDef::SubAgent` mode 默认拒绝） | P1 | low | — | ~80 行 |
| C | LaneScheduler 接入 spawner（spawn 路径走 lane budget 检查 + 优先级队列） | P1 | medium | — | ~250 行 |
| D | 父→子 cancellation 传播测试 + 修补（CancellationToken chain 端到端验证） | P1 | low | — | ~150 行 |
| **小计 P1** | | | | | **~680 行** |
| E | 文件系统 agent 定义加载（markdown frontmatter，复用 `extension/skill_ops` 加载基础设施） | P2 | medium | B | ~400 行 |
| F | Subagent 流式进度事件（trace_sink emit → 父级 LoopCallback / BackgroundAgentTracker 中间状态） | P2 | medium | A | ~250 行 |
| G | 每 agent 工具集语义命名（命名 `ASYNC_AGENT_ALLOWED` / `READ_ONLY` / `INVESTIGATION` 等 tool set） | P2 | low | B | ~150 行 |
| **小计 P2** | | | | | **~800 行** |
| H | Worktree isolation（基于 `src/sandbox/` + git worktree 创建/清理） | P3 | high | A, D | ~500 行 |
| I | 每 agent MCP 范围（agent definition 内联或引用 MCP server，加载到 child harness） | P3 | medium | B, E | ~400 行 |
| J | Fork-subagent prompt cache 复用（byte-identical prefix → Anthropic prompt cache 命中） | P3 | high | A, E | ~600 行 |
| **小计 P3** | | | | | **~1500 行** |
| **总计** | | | | | **~2980 行** |

### 1.3 依赖理由摘要

- **P1 内部 4 项**无相互依赖，建议同 PR 打包，但顺序建议 A→B→C→D：
  - A 是基础（直接复用 Phase-6 builder 范式），先做让 B/C/D 拥有完整 deps 装配上下文
  - B 改 allowlist，是 P2.E / P2.G / P3.I 的前置
  - C 是 scheduler 接入，独立线
  - D 是端到端验证 + 修补，做最后能复用 A/B/C 的工作做集成测试

- **P2 在 P1 完整 ship 后启动**：
  - E 需要 B —— 文件系统加载的 agent 可能定义错误的 allowlist，递归 guard 必须就绪
  - F 需要 A —— trace_sink wiring 在 A 范围内；F 仅消费已就绪的 trace_sink
  - G 需要 B —— allowlist 命名集合是 B 的延伸，改同一份代码

- **P3 在 P2 完整 ship 后启动**：
  - H 需要 A（trace_sink 可观测 worktree 生命周期）+ D（cancellation 传播，worktree 清理 hook 依赖）
  - I 需要 E（agent definition 写 MCP 引用）+ B（每 agent MCP 必须有 allowlist 隔离防递归）
  - J 需要 A（prompt builder seam 必须稳定）+ E（agent 定义可声明 `inherit_parent_prompt: true` 标记）

- **跨 phase 直接做下游禁止**：例如 P3.J 在 P2.E 未 ship 前不能开始（fork-subagent 依赖文件系统 agent 定义里的 fork 标记）

- **P3 三子项各自独立 PR**：H/I/J 跨 phase 依赖密集 + 风险等级高，不打包同 PR；P3 phase 整体仍按 0.4 PR 上限估算 1500 行，但拆为三次 review 降低单次风险。

### 1.4 跨 stage 收纳的 fix（不单独成 stage）

| Fix | 来源 | 收纳 stage |
|-----|------|-----------|
| `MULTI_AGENT_SYSTEM.md` 文档与代码一致性修订（"subagent excluded" 条目落实 / "lane scheduler" 实际接入点） | 探索发现 | 各 phase ship 时同步更新对应章节 |
| `subagent_tool.rs:6` 注释与代码不一致（"excludes the subagent tool"） | 探索发现 | P1.B 内部修补（同时改注释 + 加代码） |
| `lane_scheduler.rs:113` TODO "apply per-run priority boosts" | 探索发现 | P1.C 内部决定是否做（如不做则删 TODO） |
| Background agent tracker 的 status 字段语义对齐（pending/running/completed → 加 streaming） | 探索发现 | P2.F 内部 |
| `subagent_spawner.rs` 单文件 290 → 600 上限突破检查 | 0.4 全局约束 | 任一 phase 触发即拆子模块（建议抽 `harness_deps_builder.rs` / `lane_integration.rs`） |

## 2. Gap Stages（详细条目）

每个 stage 条目按统一模板：Status / Depends on / Risk / Phase / Problem /
Solution sketch / Allowed seams / Old code to retire / Acceptance / Future-proof note。

---

### Stage A — HarnessDeps 同步至主 runner 水平

**Status**: ✅ Shipped: 70c3f1480 on 2026-05-08
**Depends on**: 无
**Risk class**: low
**Phase**: P1（还债）

**Problem (现状缺陷)**

- `src/agents/subagent_spawner.rs:200-225` 共 **10 个 `None` 占位符**；本 stage 范围 **5 个核心字段**：`fallback_llm` / `stall_config` / `consecutive_failure_cap` / `turn_timeout` / `trace_sink`（剩余 5 个 None — `verifier_chain` / `context_budget` / `context_compactor` / `skill_prefetcher` / `power` — 见 § 3 Anchor A3）。
- 这 5 个字段的主 runner builder 已就绪：`fallback_llm` / `stall_config` / `consecutive_failure_cap` / `turn_timeout` 在 Phase-6 (`commit 4aa1c0f6d`) 接通（`build_fallback_llm` + `build_stability_triple` in `orchestrator_init.rs`）；`trace_sink` 在 P0 rescue 时为主 runner 接通。
- 注：Phase-6 的第 5 个字段 `guardrails` 在 subagent_spawner.rs:218 已通过父级 inherit 路径就位（不是 None），不属于本 stage 范围。
- 子 agent 路径事实上是"半成品 main runner"：在 `consecutive_failure_cap` / `turn_timeout` 等关键稳定性维度上完全裸奔。

**Solution sketch**

- 把 Phase-6 的 2 个 builder（`build_fallback_llm` 装配 1 字段 + `build_stability_triple` 装配 3 字段：stall_config / consecutive_failure_cap / turn_timeout）从 `orchestrator_init.rs` 抽到共享位置（候选：`src/agents/deps_builder.rs` 新文件，或 `src/orchestrator/deps_builder.rs`）。
- `subagent_spawner.rs` 调用共享 builder；可选支持子 agent 自有覆盖（如更紧的 `turn_timeout`）。
- `trace_sink` 直接 `clone` 父级 `HarnessDeps.trace_sink`（最简）。
- 保留 `Option<T>` 语义（`None` 仍表示 opt-in 未配置，零破坏性）。

**Allowed seams**

- 共享 builder 模块（≤ 1 个新文件，pub fn 暴露已有 builder 函数）
- `subagent_spawner` 调用共享 builder（仅修改 6 行 `None` → `build_*(...)`）
- 必须 ≥1 真实消费者：subagent_spawner.rs:200-225（替换 5 个 None）+ orchestrator_init.rs（保留调用，不重写）

**Old code to retire**

- `src/agents/subagent_spawner.rs:200-225` 的 5 处 `None` 字面量（fallback_llm / stall_config / consecutive_failure_cap / turn_timeout / trace_sink）
- 若抽取共享 builder：`orchestrator_init.rs` 内联实现替换为共享 builder 调用（不删，仅 forward）

**Acceptance criteria**

- 功能：`subagent_spawner.rs` 装配的 5 个字段与主 runner 同等成熟度（同样从 `aleph.toml` 配置驱动 / 同样的 self-reference + unknown-name guards）。
- 不破坏：Phase-6 13 builder unit + 4 init_audit + 现有 Stage 4 subagent 测试 = 全绿；R10 `src/harness/agent.rs` ≤ 1500 行不变。
- 测试：≥3 unit 验证共享 builder 在 subagent 上下文行为；≥1 integration 验证子 agent 实际继承 guardrails / fallback / stability triple。
- 性能：subagent spawn 延迟 ≤ 1.05× baseline（baseline 在 phase design 时通过 hyperfine lock）。

**Future-proof note**

builder 是 boot-time 装配 + opt-in `Option<T>` 模式，不依赖具体模型语义。R10 Future-Proof Test 通过：换更强的模型，子 agent 行为自然提升。

---

### Stage B — 显式递归 guard

**Status**: ✅ Shipped: 61ce09a96 on 2026-05-08
**Depends on**: 无
**Risk class**: low
**Phase**: P1（还债）

**Problem (现状缺陷)**

- `src/agents/subagent_tool.rs:6` 注释声称 "excludes the `subagent` tool to prevent infinite recursion"，**但代码无对应实现**。
- `AllowlistToolService::execute()` (allowlist_tool_service.rs:16-58) 仅基于 `AgentDef::is_tool_allowed`，未显式 deny "subagent" tool 名。
- `registry.rs:78-164` 的 `builtin_agents()` 主 agent allowlist `["*"]`，未显式排除 "subagent"；SubAgent mode 也未默认 deny。
- ChainContext depth guard (`subagent_spawner.rs:114-117`) 是第二道防线，但若 depth 上限被绕开 / 未配置，仍可无限递归。
- `MULTI_AGENT_SYSTEM.md` 文档与代码不一致。

**Solution sketch**

- `AllowlistToolService::execute()` 增加 mode-aware hardcoded deny：当 `AgentDef.mode == SubAgent` 且 tool name == "subagent"，直接 PermissionDenied（无需读取 allowlist）。
- Primary mode 不变，仍可调 subagent。
- 同步修订 `subagent_tool.rs:6` 注释 + `MULTI_AGENT_SYSTEM.md` 描述，使三者一致。
- ChainContext depth guard 保留作为第二道防线（不删）。

**Allowed seams**

- `AllowlistToolService` 内部 mode-aware deny check（≤ 10 行新增）
- 不引入新 trait / struct / 文件

**Old code to retire**

- `subagent_tool.rs:6` 误导性注释（重写为代码事实）
- `MULTI_AGENT_SYSTEM.md` 中关于递归保护的过期描述

**Acceptance criteria**

- 功能：SubAgent mode agent 调 "subagent" tool → 立即 PermissionDenied，不进入 allowlist 检查。
- 不破坏：Primary mode agent 仍可调 subagent；ChainContext depth guard 行为不变；现有 allowlist_tool_service 测试全绿。
- 测试：≥2 unit (Primary 通过 / SubAgent 拒绝) + ≥1 integration（端到端：父 spawn 子，子尝试 spawn 子子 → 拒绝，父收到 ToolError）。

**Future-proof note**

递归保护是工程纪律护栏，不参与推理。R10 Future-Proof Test 通过：模型升级不影响。

---

### Stage C — LaneScheduler 接入 spawner

**Status**: ✅ Shipped: 5f9f155f1 on 2026-05-08
**Depends on**: 无
**Risk class**: medium（并发治理 wiring 影响所有 spawn 路径）
**Phase**: P1（还债）

**Problem (现状缺陷)**

- `src/scheduler/lane_scheduler.rs` 全功能实现（lines 1-595），但**完全孤儿**：subagent_spawner / subagent_tool 路径 zero references。
- 文档 `MULTI_AGENT_SYSTEM.md` 声称 "Main > Nested > Subagent > Cron" 优先级，代码未生效。
- 当前 subagent spawn 无全局并发 cap，理论上可同时 spawn 几十个 child harness 拖死主 loop。
- `lane_scheduler.rs:113` TODO "apply per-run priority boosts" 未实施。

**Solution sketch**

- `subagent_spawner::spawn()` 入口通过 `LaneScheduler::reserve(LaneId::Subagent)` 占用 lane budget；spawn 出口（成功 / 失败 / timeout / cancel 任一路径）释放 budget。
- LaneId::Subagent 优先级低于 Main（防 subagent 风暴饿死 main loop）。
- Lane budget 通过 `aleph.toml` 新 section `[scheduler.subagent_lane]` 可配（optional，默认值即向后兼容）。
- `lane_scheduler.rs:113` TODO 决策：本 stage 内不实现 priority boost，删 TODO 注释（YAGNI）；如未来需要再单独 phase。

**Allowed seams**

- `LaneScheduler` 不变（已实现）
- `subagent_spawner` 入 / 出口加 reserve / release（≤ 30 行）
- `aleph.toml` 新 section（兼容）；从 config struct 装配（与 Phase-6 同 builder 模式）
- 必须 ≥1 真实消费者：subagent_spawner.rs spawn 路径

**Old code to retire**

- `lane_scheduler.rs:113` TODO 注释
- 探索发现的任何 ad-hoc 并发限制代码（如有）

**Acceptance criteria**

- 功能：subagent_spawner 路径走 LaneScheduler；超 budget → 队列等待；优先级生效。
- 不破坏：单 subagent spawn latency ≤ 1.10× baseline；现有 lane_scheduler 测试 + 现有 subagent 测试全绿。
- 测试：≥2 unit (lane reserve / release on subagent path) + ≥1 stress test（≥10 concurrent spawn 触发 lane queueing） + ≥1 integration（Main vs Subagent 优先级行为）。
- 性能：lane reserve / release 开销 ≤ 100µs per spawn。

**Future-proof note**

调度器是 boot-time 资源治理，不参与认知。R10 Future-Proof Test 通过：模型变化不影响。

---

### Stage D — 父→子 cancellation 传播测试 + 修补

**Status**: ✅ Shipped: 7c062b548 on 2026-05-08
**Depends on**: 无（独立验证 + 可能修补）
**Risk class**: low（验证为主，修补量预算 ≤80 行）
**Phase**: P1（还债）

**Problem (现状缺陷)**

- `subagent_spawner.rs:98` SpawnRequest.cancel: CancellationToken 已穿线，但 **zero 测试**验证父级 abort → 子级实际停止。
- 仅有外层 wall-clock timeout（subagent_spawner.rs:245）；父级主动 cancel 是否传播未知。
- claude-code 用 `createChildAbortController` 做父子 token 显式链接（forkedAgent.ts:354）；Aleph 实现需审计是否等价（被动 check vs 主动派生）。
- Background subagent 的 cancellation 路径（`BackgroundAgentTracker` → CancellationToken）也未测；潜在泄露 tokio task / lane budget。

**Solution sketch**

- 写 ≥3 集成测试覆盖：
  1. 父级 cancel → 同步 subagent 在下一轮迭代前停止
  2. 父级 cancel → background subagent 通过 BackgroundAgentTracker 收到 cancel 并停止
  3. 父级 timeout → 子级也被取消（外层 timeout 路径）
- 加 ≥1 leak detection 测试：spawn → cancel → 验证无 leaked tokio task / 未释放 lane budget（与 Stage C 配合）/ 未关闭 trace stream。
- 若发现父子 token 链未真正主动链接（仅被动检查）：改为主动 child token 派生（语义对齐 claude-code）。
- 修补量预算 ≤ 80 行；超出则拆为 follow-up phase。

**Allowed seams**

- 不增加新 trait
- 可能新增内部 helper（如 `spawn_child_token` ≤ 20 行）
- 必须 ≥1 真实消费者：subagent_spawner spawn 路径

**Old code to retire**

- 任何被新测试证伪的 stale 错误处理路径
- 探索发现的 dead code（如 spawn 出口未触达分支）

**Acceptance criteria**

- 功能：3 个 cancellation 场景测试全绿；leak detection 通过。
- 不破坏：现有 background tracker / subagent_tool 集成测试全绿；现有 subagent_spawner 路径行为不变（仅加测试 + 小修补）。
- 测试：≥3 integration（如上）+ ≥1 leak detection。

**Future-proof note**

cancellation 是基础设施可靠性，不依赖模型推理。R10 Future-Proof Test 通过。

---

### Stage E — 文件系统 agent 定义加载

**Status**: ✅ Shipped: cb5317474 on 2026-05-09 (+ 99613bcb1 R10-restore revert · 344a9623f doc fix) · plan: docs/superpowers/plans/2026-05-09-subagent-uplift-p2-plan.md
**Depends on**: B（递归 guard 必须就绪 — 外部加载的 agent 可能 misconfig allowlist）
**Risk class**: medium（新加载路径 + frontmatter 解析）
**Phase**: P2（补齐）

**Problem (现状缺陷)**

- `src/agents/registry.rs:78-164` 的 `builtin_agents()` **硬编码**所有 agent 定义；用户 / 项目无法添加自定义子 agent。
- claude-code 通过 markdown frontmatter 加载（`loadAgentsDir.ts:28-99`），用 Zod schema 验证，支持用户级 + 项目级 + 插件级三层来源 + 优先级覆盖。
- Aleph 已有 `src/extension/skill_ops.rs` 模块加载 markdown skill（同样的 frontmatter 解析模式），**基础设施齐备 —— 缺加载 agent 这一类的 wire**。

**Solution sketch**

- 新增 `src/agents/loader.rs`（≤ 200 行）：parse markdown frontmatter（YAML header → AgentDef，body → prompt_section）。
- 复用 `extension/skill_ops` 的 frontmatter 解析 + 文件扫描逻辑（DRY）；不引入新解析器。
- 加载路径（按优先级 high → low）：
  1. `<project>/.aleph/agents/*.md` — 项目级
  2. `~/.aleph/data/agents/*.md` — 用户级
  3. `builtin_agents()` — 内置（兜底）
- 同名 id 覆盖：高优先级胜出；agent metadata 标注 `source: project / user / builtin`。
- 启动时一次加载，注册到 `AgentRegistry`。
- frontmatter schema 与 `AgentDef` (types.rs:42-67) 一一对应：`id` / `description` / `when_to_use` / `mode` / `model_hint` / `allowed_tools` / `denied_tools` / `max_iterations` / `token_budget` / `context_mode`。

**Allowed seams**

- 新文件 `src/agents/loader.rs`
- `AgentRegistry` 增加 `register_from_markdown(path)` / `load_from_dir(dir)` 方法（pub crate 可见）
- `AgentDef` 增加 `source: AgentSource { Builtin, User, Project }` 字段（optional，`#[serde(default)]`）
- 必须 ≥1 真实消费者：`bin/aleph-server/commands/start/builder/` 启动路径里调用 loader

**Old code to retire**

- 无（`builtin_agents()` 保留兜底）
- 探索后：可能砍掉 builtin 中可被外部 markdown 替代的样板代码（仅当用户已有等价 markdown 时）

**Acceptance criteria**

- 功能：用户 / 项目目录下 `.md` 文件加载到 registry，可被 SubagentTool spawn；优先级覆盖按 project > user > builtin 生效。
- 不破坏：builtin agents 仍可用；现有 registry / subagent_tool 测试全绿；启动时间 ≤ 1.05× baseline（10 个外部 agent 文件场景）。
- 测试：≥3 unit（frontmatter 解析 / 优先级覆盖 / malformed 文件错误处理） + ≥1 integration（写临时 markdown → 启动 → spawn → 验证执行）。
- 性能：启动加载 10 个 agent 文件 ≤ 50ms。

**Future-proof note**

markdown frontmatter 是稳定数据格式，不依赖模型语义。R10 Future-Proof Test 通过。

---

### Stage F — Subagent 流式进度事件

**Status**: ✅ Shipped: 3a9b7abd5 on 2026-05-09 · plan: docs/superpowers/plans/2026-05-09-subagent-uplift-p2-plan.md
**Depends on**: A（trace_sink wiring 必须就绪 — F 仅消费）
**Risk class**: medium（事件流量观测 + 父子转发链）
**Phase**: P2（补齐）

**Problem (现状缺陷)**

- claude-code 通过 `updateProgressFromMessage` + `emitTaskProgress`（`LocalAgentTask.tsx:68-96`）把 subagent 中间消息推到父亲；fork transcript 持久化为 markdown（`runAgent.ts:744-751`）。
- Aleph `BackgroundAgentTracker` (`subagent_tool.rs:705-708`) **仅在最终 status 更新**；中间状态不可见。
- 用户无法观察 background subagent 当前在做什么（哪一步 / 哪些工具 / 卡在哪）；调试与运维无抓手。

**Solution sketch**

- `LoopTrace` enum (Stage A 接通的 trace_sink 消费的 trace 类型) 增加 `SubagentProgress { step: usize, tool_name: Option<String>, summary: Option<String> }` variant。
- `subagent_spawner` 配置一个 **forwarding trace_sink** wrapper：把子级 trace events 透传到父级 LoopCallback；同时本地 cache 最新 N 条到 `BackgroundAgentTracker.progress`。
- `BackgroundAgentTracker` 增加 `progress: VecDeque<SubagentProgress>` 字段（cap 50；FIFO 淘汰）。
- `subagent_tool.rs::check_status` action 返回最新 N 条 progress（默认 10）。
- 不持久化 transcript 文件（claude-code 那做法是 UI 残留产物；Aleph 是 server 模型，trace_sink 已经能持久化）。

**Allowed seams**

- `LoopTrace` 新 variant（向后兼容，已有消费者用 `_` 通配）
- `BackgroundAgentTracker.progress` 字段
- subagent_spawner 的 forwarding trace_sink wrapper（≤ 50 行）
- 必须 ≥1 真实消费者：subagent_tool.rs check_status 路径

**Old code to retire**

- 探索发现的 stale "TODO: streaming" 注释（如有）

**Acceptance criteria**

- 功能：parent 通过 check_status 看到 subagent 中间步骤（tool_name + summary）；progress 队列自动淘汰超 50 条。
- 不破坏：现有 trace_sink 消费者全绿；现有 BackgroundAgentTracker 测试全绿；同步 subagent 行为不变（progress 仅在 background 模式落盘）。
- 测试：≥2 unit (forwarding wrapper / queue 淘汰) + ≥1 integration（spawn background → multiple tool calls → check_status 验证 progress 时序可见）。
- 性能：每 trace event 转发 ≤ 50µs；progress 写入 ≤ 10µs。

**Future-proof note**

progress 事件是基础设施可观测性，不参与认知。R10 Future-Proof Test 通过。

---

### Stage G — 每 agent 工具集语义命名

**Status**: ✅ Shipped: d0223dd4c on 2026-05-09 (+ 37c5bb759 review polish) · plan: docs/superpowers/plans/2026-05-09-subagent-uplift-p2-plan.md
**Depends on**: B（allowlist 改造同一份代码）
**Risk class**: low（配置组织 refactor + 兼容字段）
**Phase**: P2（补齐）

**Problem (现状缺陷)**

- 当前 `AgentDef.allowed_tools` / `denied_tools` (types.rs:42-67) 是平铺 string list；当 Aleph builtin agents 数量增加，allowlist 配置散落、易错、难维护。
- claude-code 用命名常量集合（`constants/tools.ts`：`ALL_AGENT_DISALLOWED_TOOLS` / `ASYNC_AGENT_ALLOWED_TOOLS` / `IN_PROCESS_TEAMMATE_ALLOWED_TOOLS` / `CUSTOM_AGENT_DISALLOWED_TOOLS`）+ 每 agent 引用，可读性 + 安全默认显著提升。
- 安全敏感：async / background subagent 应默认更严（不允许 Bash / Edit / Write），Aleph 当前未做语义区分；Stage B 加了递归 guard，但其他敏感工具仍裸奔。

**Solution sketch**

- 新增 `src/agents/tool_sets.rs`（≤ 100 行）定义命名常量集合：
  - `READ_ONLY_TOOLS` — Grep / Glob / Read / NotebookRead
  - `INVESTIGATION_TOOLS` = READ_ONLY ∪ {WebSearch, WebFetch, Task}
  - `ASYNC_SAFE_TOOLS` — 更严，禁止任何会改世界的工具
  - `ALL_AGENT_DENIED_TOOLS` — 全 agent 默认禁（Bash / Edit / Write 等敏感工具）
- `AgentDef` 增加 `allowed_tool_sets: Vec<String>` 可选字段（`#[serde(default)]`，向后兼容）。
- 解析顺序：`allowed_tool_sets` ∪ `allowed_tools` − `denied_tools` − `ALL_AGENT_DENIED_TOOLS`（除非 explicit `allow_override`）。
- `builtin_agents()` 中可读性高的 agents 用命名集合替代平铺 list（增量替换，不一次全改）。

**Allowed seams**

- 新文件 `src/agents/tool_sets.rs`
- `AgentDef` 新增 optional 字段（与 0.4 兼容硬约束一致）
- 解析逻辑放在 `AgentDef::is_tool_allowed`（types.rs:142-154）扩展，不动 `AllowlistToolService`
- 必须 ≥1 真实消费者：≥1 个 builtin agent 用命名集合声明

**Old code to retire**

- 探索后：`builtin_agents()` 中重复出现的 allowlist 字面量（如多个 agents 都列 ["Read", "Grep", "Glob"]）替换为 `READ_ONLY_TOOLS` 引用

**Acceptance criteria**

- 功能：AgentDef 可用命名集合声明 allowlist；解析为最终 allowed / denied 集合；deny 始终优先。
- 不破坏：现有 `AgentDef.allowed_tools` 平铺仍生效；builtin agents 行为字符完全相同（即使内部用命名集合实现）。
- 测试：≥3 unit（命名集合解析 / 平铺与命名混用 / deny 优先）+ ≥1 integration（替换 1 个 builtin agent 为命名集合声明，跑现有 subagent 测试套件全绿验证行为等价）。

**Future-proof note**

tool sets 是配置组织模式，不参与认知；新模型 / 新工具加入时，集合定义集中维护，不需要改 agents。R10 Future-Proof Test 通过。

---

### Stage H — Worktree isolation

**Status**: ✅ Shipped: cfb2b358722089768d1c5f358b3525f9f4f94d62 on 2026-05-09 · plan: docs/superpowers/plans/2026-05-09-subagent-uplift-p3-stage-h-plan.md
**Depends on**: A（trace_sink 可观测 worktree 生命周期）, D（cancellation 传播 — 清理 hook 依赖）
**Risk class**: high（文件系统副作用 + 清理可靠性 + 并发隔离）
**Phase**: P3（超越）

**Problem (现状缺陷)**

- claude-code 通过 `isolation: 'worktree'` 为 subagent 创建临时 git worktree（`createAgentWorktree`）；子 agent 在隔离工作树上工作，避免污染父级 cwd。
- Aleph 无 worktree 隔离；所有 subagent **共享 parent cwd**；并发 subagent 写同一目录时可能互相覆盖 / 触发 file lock 冲突。
- 长程实验性 subagent（refactor 试验、自动化代码生成）无安全边界，失败时清理回滚困难。
- Aleph 已有 `src/sandbox/` 模块作基础设施候选（具体能力在 phase design 时审计）。

**Solution sketch**

- 复用 `src/sandbox/` 的隔离原语（如已有 git wrapper，扩展 worktree create / remove；如未有，最小新增）。
- `SubagentSpawnRequest` 增加 `isolation: Option<IsolationMode>` 字段（`None` 默认 / `Worktree`）。
- `Worktree` 模式下 spawn 流程：
  1. 启动时从 parent cwd 创建临时 worktree（target: `$TMPDIR/aleph-subagent-<id>/`）
  2. child harness 的 cwd / cargo target dir 指向 worktree
  3. spawn 结束（success / fail / timeout / cancel 任一路径）必须清理
- 清理可靠性：用 RAII Drop guard（`WorktreeHandle`），即使 panic 也清理。
- trace_sink emit `WorktreeCreated { path }` / `WorktreeCleanedUp { path, leaked: bool }` 事件（依赖 A）。
- 失败模式：worktree 创建失败 → `IsolationFailed` 错误回流；不 fall back 到共享 cwd（fail loudly，隔离声明违反就该失败）。

**Allowed seams**

- `SubagentSpawnRequest` 新 optional 字段
- `src/sandbox/` 扩展（worktree create / remove pair）
- `WorktreeHandle` RAII guard（≤ 50 行）
- 新文件 `src/agents/worktree.rs` 仅当 sandbox 不适合容纳
- 必须 ≥1 真实消费者：subagent_spawner Worktree 分支

**Old code to retire**

- 探索发现的 ad-hoc cwd manipulation 代码（如有）
- 任何"父子共享 cwd 假设"的代码注释（更新为现状）

**Acceptance criteria**

- 功能：`isolation: Worktree` 时子 agent 在独立 worktree 工作；spawn 结束清理；并发 subagent 互不干扰。
- 不破坏：默认 `None` 模式行为完全不变；现有 subagent 测试全绿。
- 测试：≥3 integration（worktree 创建 / 正常 cleanup / cancel 时 cleanup） + ≥1 leak detection（spawn × 10 with random cancel → 验证 `$TMPDIR` 无 leftover worktree）。
- 性能：worktree 创建 ≤ 200ms（git worktree add 实际开销）；清理 ≤ 100ms。

**Future-proof note**

worktree 是 git 原语，不依赖模型推理。R10 Future-Proof Test 通过：模型升级不影响。

---

### Stage I — 每 agent MCP 范围

**Status**: ✅ Shipped: 864f0e53a40d7fa4eaac883ed3665197aef8382a on 2026-05-09 · plan: docs/superpowers/plans/2026-05-09-subagent-uplift-p3-stage-i-plan.md
**Depends on**: B（递归 guard / allowlist 必须就绪）, E（agent 定义可声明 mcp_servers）
**Risk class**: medium（MCP 生命周期 + 子 harness 装配）
**Phase**: P3（超越）

**Problem (现状缺陷)**

- claude-code 允许 agent definition 声明 `mcpServers`（inline + referenced，`runAgent.ts:104-227`）；子 agent 可独享 / 引用特定 MCP server。
- Aleph 当前所有 MCP server 是 **全局加载**（aleph-server 启动时一次性装配）；subagent 无法独享子集 / 加载父级未启用的 MCP。
- 实际场景：一个 "git-research" subagent 想用 GitHub MCP，其他 agent 不需要 → 现状下要么全局开 GitHub MCP（噪音），要么手动重启服务。
- 安全维度：每 agent MCP 范围必须配合 allowlist（B），否则同样会绕过递归 / 工具黑名单。

**Solution sketch**

- `AgentDef` 增加 `mcp_servers: Vec<McpServerSpec>` 可选字段：
  - `McpServerSpec { name: String, config: Inline(McpInline) | Reference(String) }`
  - `Inline`: 完整 MCP server 配置内联
  - `Reference`: 引用全局已注册的 server name（仅扩展可见性，不重启）
- subagent_spawner 在 spawn 时：
  1. 解析 `agent_def.mcp_servers`
  2. 为子 harness 创建一个 `McpRegistry view`（父级 globals ∪ Reference 引用 ∪ Inline 临时启动）
  3. 临时 MCP servers 在 child harness 生命周期内启停
  4. spawn 结束（任意路径）关闭临时 MCP servers（与 Stage H 同 RAII pattern）
- 文件系统 agent（Stage E）frontmatter 即可声明 mcp_servers，无需 hardcode。

**Allowed seams**

- `AgentDef` 新字段
- `McpRegistry view` 抽象（如已存在则复用；否则 ≤ 100 行新代码）
- 临时 MCP server 启停的 RAII guard
- 必须 ≥1 真实消费者：subagent_spawner MCP 装配路径

**Old code to retire**

- 无（增量）
- 探索后：MCP registry 全局 singleton 假设的代码注释更新（仍是 singleton，但 view 模式叠加）

**Acceptance criteria**

- 功能：agent definition 声明 mcp_servers → subagent 加载并使用其工具；spawn 结束 cleanup；非 fork 路径下父级 MCP 范围不变。
- 不破坏：默认无 `mcp_servers` 行为完全不变；全局 MCP server 注册流程不变；现有 MCP 测试全绿。
- 测试：≥2 unit（Inline / Reference 解析）+ ≥1 integration（subagent 调用 agent-scoped MCP tool 成功 + 父级未加载该 MCP；验证父子隔离）+ ≥1 leak detection（spawn × 5 → 验证临时 MCP 进程全部回收）。
- 性能：Reference 模式 ≤ 10ms；Inline 模式 ≤ 500ms（受具体 MCP 启动开销支配）。

**Future-proof note**

MCP 协议是稳定接口，不依赖模型语义。R10 Future-Proof Test 通过。

---

### Stage J — Fork-subagent prompt cache 复用

**Status**: 📋 Planned · plan: TBD
· J-pre (cache observability) shipped 2026-05-09; fork-branch decision deferred to 2026-05-23 review
**Depends on**: A（trace_sink 验证 cache 命中），E（agent 定义可声明 inherit_parent_prompt）
**Risk class**: high（prompt 字节稳定性 + cache 命中验证 + LLM 协议依赖）
**Phase**: P3（超越）

**Problem (现状缺陷)**

- claude-code 引入 fork-subagent（`forkSubagent.ts:60-71`）：子 agent 继承父级 prompt prefix **完全字节对齐**（包括 placeholder tool results），利用 Anthropic prompt cache 命中。
- 子 agent 第一轮调用的 prompt 输入 token 几乎零成本（仅 incremental 部分计费），cache hit ratio 显著影响长程 ralph / multi-subagent 模式总成本。
- Aleph 子 agent 当前总是通过 PromptBuilder 装配新 system_prompt → cache miss → 长程任务 token 成本累积明显（Aleph 本身就是常驻 server，运行时间长 → 影响放大）。
- 缺 fork 模式还会鼓励错误模式：开发者把"子任务"塞进父级 context 来省 token（破坏 R10 Think→Act 单一性）。

**Solution sketch**

- `AgentDef` 增加 `inherit_parent_prompt: bool` 可选字段（默认 `false`，明确 opt-in）。
- 当 `true`，subagent_spawner 走 **fork branch**（绕开 PromptBuilder）：
  1. 从父级 `HarnessDeps` clone 完整 system_prompt（byte-identical，不重新装配）
  2. 在 user message 末尾追加子任务指令
  3. 父级对话历史最后 N 条作为 **placeholder tool results** 重放（claude-code 用 "[fork message]" 占位字符串），保证 prompt prefix 字节稳定
- 关键技术：placeholder tool result 的内容必须 deterministic（同一父级 + 同一子任务 → 同一字节序列）。
- 通过 trace_sink (Stage A) 持续观测命中率：`usage.cache_creation_input_tokens` / `usage.cache_read_input_tokens` 字段；写一个 fork-cache hit ratio 监控测试断言（≥ 0.8 硬阈值）。
- 文件系统 agent（Stage E）frontmatter 中的 `inherit_parent_prompt` 标记；不接受 inline 调用方运行时 override（避免误用导致 cache miss）。

**Allowed seams**

- `AgentDef` 新 optional 字段
- subagent_spawner 内部 fork branch（≤ 150 行；与 PromptBuilder 路径互斥）
- placeholder tool result 生成器（确定性，≤ 50 行）
- 不修改 `PromptBuilder`（fork 路径完全绕开）
- 必须 ≥1 真实消费者：≥1 个 fork agent definition

**Old code to retire**

- 无（增量）

**Acceptance criteria**

- 功能：fork agent 第一轮调用 prompt prefix 与父级字节相等；trace 显示 `cache_read_input_tokens > 0`。
- 不破坏：非 fork agent 行为完全不变；PromptBuilder 路径不变；现有 subagent 测试全绿。
- 测试：≥2 unit（fork prompt prefix byte-equal 断言 / placeholder 确定性）+ ≥1 integration（启用 fork agent → 真实 LLM 调用 → 验证 `usage.cache_read_input_tokens > 0`）+ ≥1 cost 回归测试（fork 模式 N 个并发子 agent 总成本 < N × non_fork_cost × 0.5）。
- 性能：fork prompt 装配 ≤ 5ms（clone + append 主导）。

**Future-proof note**

依赖 Anthropic prompt cache 协议；如未来 cache 行为改变（key 算法 / TTL）需调整。但 Aleph 通过 trace_sink 持续观测命中率，劣化即时可见。R10 Future-Proof Test **软通过**（基础设施依赖外部 cache，不依赖模型推理）。

风险声明：本 stage 是路线图唯一软通过项；P3 实施前重新评估。如 claude-code 改变 fork 实现，本 stage 重新设计。

## 3. Anchor Entries（出范围模块）

以下模块虽与 subagent 路径相关或邻接，但**显式声明不在本路线图范围**。每个 P 阶段
design 不得动这些 anchor；任何"顺手改一下"的尝试需先开新 brainstorm。

| Anchor | 模块 | 当前状态 | 不动理由 |
|--------|------|---------|---------|
| A1 | `src/harness/` Harness Loop | 健康（R10 神圣 + Phase-6 已 ship） | 本路线图不修改 Think→Act 循环本体；任何 stage 触及 harness/ 必须在 phase design 明文论证不可避免 + verifier 证零 regression |
| A2 | `src/a2a/` Remote Agent Protocol | 健康（远程 peer-to-peer） | 本路线图全部是 **in-process subagent**；远程 agent-to-agent 是另一条独立线，不混合。a2a 既有的 RawMemory(Delegation) emit 模式继续保留（subagent_spawner.rs:273-288 已对齐） |
| A3 | 剩余 5 个 HarnessDeps None 字段（`verifier_chain` / `context_budget` / `context_compactor` / `skill_prefetcher` / `power`） | 主 runner 自身亦未完整接通 | 等主 runner 在这些字段上 ship 后再启动 **P4 后续路线图** 同步子 agent。本路线图 Stage A 仅同步 Phase-6 + P0 rescue 已为主 runner 接通的 5 字段（fallback_llm / stall_config / consecutive_failure_cap / turn_timeout / trace_sink） |
| A4 | `src/agents/runtime.rs` AgentRuntime 抽象 | 健康，已是干净 LoopTool 适配层 | 本路线图不重写 / 不引入新 runtime 抽象层；所有 stage 在现有 runtime 之上扩展 |
| A5 | `src/init_unified/` First-Install + `src/extension/skill_ops` Skill Loader | 健康，是已验证基础设施 | 本路线图 Stage E **复用** skill_ops frontmatter 解析 + 文件扫描，但不修改 skill_ops 本身；init_unified 的 first-install 路径完全不动 |

**Anchor 红线**：以上任一 anchor 在 phase design 中被触及 → 该 phase brainstorm 必须开
独立子段论证：(a) 为何无法绕开 anchor，(b) 触及范围最小化方案，(c) verifier 证零
regression 的具体测试 plan。

## 4. 修订规则

### 4.1 轻量修订（无需重新 brainstorm）

允许直接在本文件追加 / 修改的场景：

- 某个 stage 完成 ship → 在该 stage 条目 `Status` 行追加 `✅ Shipped: <hash> on <date> · plan: <link>`
- 某个 phase（P1/P2/P3）完成 ship → 在文件顶部 `status` frontmatter 追加 phase 完成标记
- 1.4 节 "跨 stage 收纳的 fix" 表内行的 stage 归属调整（如某 fix 实际由另一 stage 顺手解决）
- typo / 链接修正
- Anchor 表新增条目（仅当发现新邻接模块需要明文外置时）

### 4.2 正式修订（需重新 brainstorm + commit）

以下情形必须走正式 brainstorm 流程：

- **依赖关系被实证证伪**：某个 stage 实际 design 时发现依赖关系与 1.3 节描述不一致 →
  需修订 1.1/1.2/1.3 + 触及 stage 的依赖项
- **新发现 P0 缺陷插队**：subagent 路径出现 P0 级 bug（数据丢失 / 安全洞 / 严重崩溃） →
  P0 fix 单独走 rescue 流程，路线图修订加入新 stage 或调整顺序
- **Scope 边界证伪**：某 stage 实际工作量远超估算（如 Stage H 实测 worktree 兼容性
  > 800 行），需拆 phase 或降级
- **Anchor 红线被迫触及**：某 stage 必须修改 anchor 模块 → 修订 § 3 + 该 stage 设
  独立 risk-mitigation 段
- **路线图全局结构调整**：从 P1/P2/P3 三 phase 改为其他切片方式 → 全文修订

### 4.3 上游重大改动响应

- claude-code 上游若出现重大 subagent 改动（fork / coordinator / remote 模式范式变化 /
  新工具类别 / 新 isolation 维度）→ **不自动追入** 当前路线图
- 评估后选择三种处理之一：
  1. 加入当前路线图作为新 stage（仅当与现有 phase 兼容 + 风险可控）
  2. 推迟到 P4 后续路线图（默认选项）
  3. 显式声明放弃（在 § 3 Anchor 表新增条目）

### 4.4 Phase 启动入口

每个 P 阶段被认领时：

1. 阅读本路线图 § 0（红线）+ § 1（依赖）+ 该 phase 对应 Stages（详细条目）
2. 启动新会话，使用 `superpowers:brainstorming` 流程
3. brainstorm 输入：本路线图链接 + 该 phase Stages 编号
4. brainstorm 产物：`docs/superpowers/specs/<date>-subagent-uplift-<phase>-design.md`
5. design 完成 → `superpowers:writing-plans` 产 plan
6. 实施完成 → 在本路线图对应 stage 条目追加 `✅ Shipped` 行（轻量修订）

### 4.5 关闭条件

整份路线图"关闭"的条件：

- ✅ 全部 10 个 stage 状态 = `✅ Shipped`
- ✅ § 3 Anchor 红线 0 违反
- ✅ § 0.2 列出的非目标边界未被违反（无意外引入 a2a 重写 / 完整 design.md / 代码 / plan / verifier 等）
- ✅ § 0.4 全局约束零违反（PR / commit / 单文件预算 / R3 / R7 / R10 等）
- ✅ 文档与代码事实一致（`MULTI_AGENT_SYSTEM.md` 不再撒谎）
- 关闭后本文件追加最终 commit hash + 日期，移交给后续 P4 路线图（如存在）作为 follows
