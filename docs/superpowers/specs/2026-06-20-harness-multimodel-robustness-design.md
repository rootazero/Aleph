# Harness 多模型鲁棒性设计 (Harness Multi-Model Robustness)

- **Date**: 2026-06-20
- **Status**: Approved design — ready for writing-plans
- **Scope**: B-standard（见下）
- **Approach**: Approach 1 — 原地扩展现有判决机制（非重写）
- **Author**: brainstorming session (Claude + user)

---

## 1. 背景与动机 (Motivation)

### 1.1 触发问题

一个每日新闻摘要 cron（抓 5 个版块页 + 钻进 ≤2 文章页 = "Design 2"）在手动触发时**两次都被 harness 的防循环闸 halt**，产出空（`notdelivered`）+ 二次 provider 400。根因不是单个 cron，而是 harness 对"不听话/弱模型"的管控方式：

- 默认 failover 模型（kimi 链）一口气并行甩出 8 个 `web_fetch`、无任何叙述文字、甚至重复抓同一篇。
- 当前 `ToolLoopVerifier` 的 **Tier-2**（同 tool name 占满整个 8-窗口 + 无叙述）**直接 Halt**，杀死整个 run。
- prompt 层的"≤7""先写说明"等节奏指令，弱模型根本不遵守 → 无法靠 prompt 稳住。

### 1.2 这是 harness 结构问题，不只是 cron

用户判断正确：这是**整个 harness 的多模型管控能力**问题。参考 openclaw / hermes-agent / pi 三个 agent 框架，它们处理"不听话 LLM"的共识做法是：

1. **Steer，不要 halt** — 检测到循环时把一条固定的纠正反馈**作为 tool-result 回喂**，让模型自纠，而非杀 run。
2. **按 (tool+args+outcome) 哈希 + 分级 + 进度感知**检测，而非"连续 N 次同名静默"。
3. **按模型的行为档案**（config/data，非分支代码）：弱模型收紧阈值 + 多 steer，强模型保持薄。

### 1.3 关键发现：Aleph 已有大半骨架（reframe）

实读 Aleph harness seam 后发现这是**扩展+修复**而非重写：

- **`Veto` 判决已经 = steer**：`think.rs` 的 Veto 分支已注入反馈消息 + 强制再来一轮 `Continue`。Tier-1 已是"先 steer"。
- **grace/收尾轮已存在**：`Halt` 时已 `fire_boundary_grace_turn(GraceReason::ToolLoopHalt)`，nudge "别调工具，用现有信息出最终交付"。即用户选的"收尾交付 partial"恢复契约**已布线**。
- **按模型 behavior 系统已存在**：`src/providers/model_behaviors/*.md`，每 run 按 protocol/model 解析（日志 `behavior_name=anthropic`）。

### 1.4 三个真实缺口（本设计要补的）

1. **Tier-2 跳过 steer 直接 halt** — Tier-1 有 veto→halt 升级；Tier-2（并行静默批 = 新闻案例）直接 Halt，无 steer 机会。
2. **并行批次下收尾轮坏掉** — Halt 后 grace turn 撞 **provider 400（orphaned tool_call_ids）** → 产出空。"交付 partial"契约今天对最常见触发场景静默失效。
3. **检测钝且对模型无感** — 纯 name/args 结构计数（无 outcome/进度感知 → 误杀合法 varying-args 探索），阈值是全局常量（强模型与 kimi 同窗口）。

---

## 2. 范围 (Scope)

### 2.1 In scope (B-standard)

- 检测信号从"计数"升级到"novelty 感知"（统一 Tier-1/Tier-2 到单一 novelty 轴）。
- Tier-2 走 veto→steer→halt 升级阶梯（与 Tier-1 一致）。
- 修复 grace-turn orphaned-tool-call 400，使 partial 真能交付。
- 按模型鲁棒性档案（per-model `ModelRobustnessProfile`），经 `model_behaviors` frontmatter 配置，每 run 注入。

### 2.2 Out of scope (YAGNI)

- ping-pong / unknown-tool 专用检测器（Approach 2 / openclaw 全套）——无证据表明 Aleph 实际出现这些循环。
- 空响应多级回退（fallback narration / thinking-prefill / provider rotation 的完整套件，hermes "Max" 范围）。
- 工具参数 JSON 修复（hermes 弱本地模型场景）。
- idle-timeout 断路器（openclaw "Max" 范围）。
- 把新闻 cron 切回 Design 2 ——**独立决定**，不在本 spec。本工作让 Design 2 变可行，但不在此启用。

### 2.3 互补的独立 spec

- **Sub-project A**：`web_fetch` 链接/时间戳提取丢失（Readability/selector 抹掉 `<a href>`/`<time>`）。tool 层修复，单独 spec/plan。本 spec 不含。

---

## 3. 架构 (Architecture)

四个改动面，R10 红线目录 `src/harness/` 仅极小判决处理改动：

| 面 | 位置（约） | 性质 |
|---|---|---|
| 检测信号 | `src/verification/tool_loop_verifier.rs`、`src/verification/turn_verifier.rs` | 纯结构计数升级（novelty） |
| 判决处理 | `src/harness/agent/think.rs` | 极小改动（Tier-2 走 Veto + 复用 veto 计数升级） |
| 收尾修复 | `src/harness/agent/*`（grace-turn payload 构建路径） | bug 修复（orphaned tool_call 400） |
| 按模型档案 | `src/providers/model_behaviors/`、`orchestrator_init.rs`、`TurnVerifyContext` | config/data + 每 run 注入 |

> 注：以下 file:line 为设计时的近似锚点，实现时以实际符号/函数名为准（行号会漂移）。

**核心理念**：把"钝 halt"换成"分级 steer→收尾"。harness 仍**零推理**——检测器是纯结构计数，profile 是 data，模型负责恢复决策。检测器留在 `src/verification/`，不进 R10 核心。

---

## 4. 组件设计 (Components)

### 4.1 检测信号：novelty 感知

**现状**：`ToolCallSummary { name, args_hash }`。Tier-1 = "同 name+同 args 连续 ≥ repeat_threshold"；Tier-2 = "同 name 占满 `TOOL_HISTORY_WINDOW`(8) + 无叙述" → 直接 Halt。

**问题**：Tier-2 把 5 个不同 URL 的 `web_fetch`（5 个不同 args、5 个不同结果）误判为循环。

**改**：
- 给 `ToolCallSummary` 增 `outcome_hash: Option<u64>`（已执行的历史调用带结果哈希；当前待执行批次为 `None`）。
- 在窗口上定义 **novelty = distinct(args_hash, outcome_hash) 对数 / 窗口长度**，区分三态：
  - **高 distinctness**（多为新 args，无 revisit）= fan-out / 探索 → **Continue**。新闻案例 5 distinct / 5 = 1.0，**永不触发**。
  - **零变化**（同 name 同 args 连续重复）= Tier-1 死循环（= 现有逻辑）。
  - **低 distinctness**（同 name，args 在小集合内 revisit/cycling，如 template/layouts/themes 三文件转圈）= Tier-2 thrash。
- 阈值（`novelty_min`、窗口）来自 profile（§4.3）。纯结构计数，无模型推理 → R10-safe。

> **为何丢弃 `outcome_hash`（规划期决定）**：tool_history 在 `think.rs:1053` **执行前**填充（emit 即 push，Act 之前），push 时无 outcome；要带 outcome 需 Act 后回填整个 ring buffer，为边缘场景（polling 类"同 args 但结果在变"）增加跨 think/act 可变状态。**args-distinctness 单独已分离 fan-out / thrash / identical 三态**（验收的真实 bug 即靠它解决），故按 YAGNI / R10 删除 outcome 维度，`ToolCallSummary` **不改结构**。

**判定函数**（伪，纯 `name`+`args_hash`）：
```
distinct = count_distinct{(name, args_hash) in window}
run      = trailing identical (name,args_hash) run     // 现有 trailing_repeat_run
same_run = trailing same-name run                       // 现有 trailing_same_name_run
if run >= repeat_threshold                       -> Tier-1（identical 重复）
elif same_run >= window
     and distinct/len(window) < novelty_min
     and (not silence_required or no_text)        -> Tier-2（低 distinctness thrash）
else                                              -> Continue（含高 distinctness fan-out）
```

### 4.2 升级阶梯：Tier-2 也先 steer

**现状**：Tier-1 有 veto→halt；Tier-2 **直接 halt**（这是误杀 fan-out 的钝边）。

**改（复用现有 veto-cap→grace 机器，规划时简化）**：让 **Tier-2 发 `Veto` 而非 `Halt`**。Aleph 已有完整的"steer→收尾"链，不必新建升级计数：
- `Veto` 路径已是 steer（`think.rs` 约 :1087-1189 的 Veto 分支注入 `[verifier veto]` 反馈 + Continue），且**每次 veto 都 `close_unexecuted_tool_uses`**（给每个未执行 tool_use 补合成 `ToolError`，保 tool_use↔result 配对）。
- harness 已有 **veto-cap→grace**：`agent.rs:600` `verifier_veto_count >= MAX_VERIFIER_VETOS` → `fire_boundary_grace_turn(GraceReason::VerifierVeto)` → 收尾交付 partial。且 `verifier_veto_count` 在**非-veto 轮归零**（`agent.rs` 约 :625），已是 episode 语义——无需新计数。
- **唯一改动**：把 `MAX_VERIFIER_VETOS`（现 `const =10`）换成**每 run 来自 profile 的 `steer_max`**（弱模型可调小=更早收尾，强模型调大）。
- `Halt` 仍保留给 **Tier-1 identical**（同 name 同 args 连续 ≥ `halt_threshold`，真·死循环，直接收尾合理）。故 §4.4 的 grace-400 修复仍要做（防御 Tier-1 直 Halt 的并行批次），但 fan-out 主场景已被 Veto 路径绕开。

### 4.3 按模型鲁棒性档案 (ModelRobustnessProfile)

**新类型**：
```rust
pub struct ModelRobustnessProfile {
    pub repeat_threshold: usize,   // Tier-1 veto 阈值（identical run）
    pub halt_threshold: usize,     // 硬 halt 上界（窗口内）
    pub steer_max: usize,          // halt 前允许的 steer 次数
    pub novelty_min: f32,          // 低于此判 Tier-2 thrash
    pub silence_required: bool,    // Tier-2 是否要求无叙述才触发
}
```
- **默认 profile**：等价于今天的保守行为（repeat=5, halt=8, silence_required=true），向后兼容。
- **来源（规划期简化）**：**Rust 内置表** `ModelRobustnessProfile::for_behavior(name)`，按 behavior 名给默认（anthropic → 宽松；ollama/弱 → 收紧 + 多 steer；未知 → 保守默认）。**不**用 prose `.md` frontmatter——那些 `.md` 是注入 prompt 的正文，frontmatter 会泄进系统 prompt。用户级 config 覆盖（`~/.aleph/...`）按 YAGNI 暂不做，未来需要再加。
- **关键架构点**：`ToolLoopVerifier` 在 `orchestrator_init` 启动时构造一次，而 behavior 是**每 run 按模型解析**。因此 profile **走 `TurnVerifyContext`（每 run 注入）**，verifier 保持 `(history, profile)` 的纯函数，**不重建 verifier**。
  - harness 已每 run 解析 model behavior，可在建 `TurnVerifyContext` 时一并放入 resolved profile。
  - `ToolLoopVerifier::verify()` 从 `ctx.robustness_profile` 读阈值，而非 self 字段。

### 4.4 收尾修复：grace-turn orphaned-tool-call 400

**现状**：`Halt` 已 `fire_boundary_grace_turn(GraceReason::ToolLoopHalt)`（nudge：别调工具、用现有信息出最终交付）。但并行批次 halt 时，grace payload 含未配对的 tool_use → Anthropic **400 orphaned tool_call_ids** → 交付空。这是新闻 cron "notdelivered" 的真因。

**改（含一次精确 root-cause）**：
- Halt 时确保 **halting 轮里每个 emit 的 tool_use 都补一个合成 tool_result**（call_id 精确匹配，内容如 `"skipped: loop terminated"`），**再**构建 grace payload → 消息序列合法。
- 实现时先精确定位现有"close unexecuted tool_use blocks"为何对并行批次不完整（部分关闭 / payload builder 漏带合成结果），systematic-debugging 复现一次。
- grace turn 自身 provider 失败时 → **回退到最后一段 assistant 文本**，永不静默空交付。

---

## 5. 数据流 (Data Flow)

```
模型轮 emit tool calls (+可选文本)
  → harness 建 TurnVerifyContext {
        recent_tool_calls (含 outcome_hash),
        final_text, stop_reason,
        robustness_profile (来自本 run 的 model behavior 解析)
    }
  → ToolLoopVerifier.verify() 算 novelty
  → VerifierVerdict:
        Continue (高 novelty / 进度) — 正常继续
        Veto     (停滞，steer 未用尽) — think.rs 注入 steer 反馈 + Continue
        Halt     (停滞，steer 用尽)   — think.rs:
                                          1) 给每个未配对 tool_use 补合成 tool_result
                                          2) fire grace 收尾轮（投递 partial）
                                          3) 结束 run
```

---

## 6. 错误处理 (Error Handling)

- grace payload **必须无 orphaned id**（§4.4 修复点，带回归测试）。
- grace turn provider 失败 → 回退最后 assistant 文本（永不静默空交付）。
- profile 解析失败 → 默认 profile（不破坏 run）。
- 阈值钳位：沿用现有 `with_threshold`/`with_halt_threshold` 的 clamp 不变量（`repeat ∈ [2, WINDOW]`，`halt ∈ [repeat, WINDOW]`，两 tier 不反转）。

---

## 7. 测试 (Testing)

- **单元（检测）**：identical run（Tier-1 触发）/ cycling-thrash 小集合（Tier-2 触发）/ high-novelty fan-out（**新闻案例：5 distinct → 不触发**）/ 有叙述 vs 无叙述。
- **单元（profile）**：frontmatter 解析 + 默认回退 + 钳位不变量。
- **单元（升级）**：veto 计数到 `steer_max` 再 halt；未达则持续 veto。
- **回归（关键）**：并行静默批 → halt → grace payload 每个 tool_use 都有配对 tool_result（**无 400**）。
- **集成**：模拟弱模型 fan-out 后 stall → 被 steer →（仍 stall）收尾交付 **partial（非空）**。
- 覆盖率沿用项目 80% 目标；harness 改动以 `--lib` 单测守护（遵循 cargo 节制约束，默认不跑全量）。

---

## 8. R10 合规 & 设计纪律

- **检测器纯结构**（novelty 计数）、**profile 是 data**（frontmatter）、**think.rs 仅改已有判决**（Veto/Halt 已存在）→ harness 仍零推理。
- R10 五"不"逐条：不判断意图分类 / 不做工具过滤 / 不做完成度判断 / 不做内容审查 / 不做错误恢复策略选择——本设计把"harness 单方杀 run"（更接近违反第 5 条）换成"机械 steer + 模型自选恢复"，**更** R10-aligned。
- 检测器与 profile 都在 `src/verification/` + `src/providers/`，不堆进 `src/harness/` 12 文件红线。
- **加代码前必答 3 问**：①脚手架非认知 ✓ ②模型升级后仍需要？——是（弱模型护栏 + 强模型薄，profile 驱动）③真实消费者？——是（verifier chain 已是消费者，本工作增强它）。

---

## 9. 实施顺序建议 (for writing-plans)

1. `ModelRobustnessProfile` 类型 + 默认值 + frontmatter 解析（含单测）。
2. `TurnVerifyContext` 注入 `robustness_profile` + harness 每 run 解析连线。
3. `ToolLoopVerifier` distinctness 判定 + Tier-2 改发 `Veto`（读 ctx.profile，纯 `name`+`args_hash`，不改 `ToolCallSummary`）。
4. `MAX_VERIFIER_VETOS` → profile.`steer_max`（`agent.rs`，每 run）。
5. grace-turn orphaned-tool-call 400 root-cause + 修复 + 回归测试（防御 Tier-1 直 Halt 的并行批次）。
6. 集成测试（弱模型 fan-out→steer→partial 交付）。

---

## 10. 验收标准 (Success Criteria)

- 新闻 5-distinct 并行 `web_fetch` fan-out **不再触发 halt**（high novelty）。
- 真实 thrash（小集合 cycling 静默）先被 **steer**，被忽略 `steer_max` 次后才 halt。
- halt 后 grace 收尾**成功交付 partial**（无 400，非空）。
- 弱模型与强模型走**不同阈值**（profile 驱动），强模型行为不变（默认 profile 字节兼容）。
- `src/harness/` 不新增认知层；检测器/profile 在 verification/providers。
