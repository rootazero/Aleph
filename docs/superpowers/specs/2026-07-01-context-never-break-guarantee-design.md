# 上下文永不中断保证 + 模型上下文数据库 + 占用仪表对齐

- **Date**: 2026-07-01
- **Scope**: Core `src/context/budget/` + `src/context/compact/` + `src/session/`（就地压缩持久化）+ `src/providers/model_catalog/`（模型数据库）+ panel 仪表口径。**`src/harness/` 12 文件/4900 行不动**。
- **Status**: Design approved — pending implementation plan。
- **触发**: 用户切到一个 near-full 历史会话、只想"打开之前做的 HTML"，第一条消息即报「抱歉，上下文预算已用尽（0 次迭代后），无法继续」；同时仪表显示 65% ——"65% 却说满了"的矛盾。
- **用户目标（原话）**: "任何时候，任何状态下，对话都不能因为上下文填满导致中断。上下文预算和控制和压缩，必须是 harness 极强的能力，根据不同模型的上下文长度，在最合适的时机实施压缩，确保用户无论是连续对话，还是切换到某个历史对话都能顺利进行。"
- **红线对照**: 压缩/分裂是机制（`src/context/compact/`），决策是确定性预算门（`src/context/budget/`），二者皆在 harness 之外 → R10（薄 harness，12 文件不动）；不引入 LLM 做意图判断 → R7；模型数据库是数据非路由、内置 JSON + 用户覆盖、零新依赖 → R3/P8；优雅降级（病态截断地板）→ P7。
- **承接 / 关联**: [[project-context-gauge-history-replay]]（档②历史回放，已在工作树未部署）、[[project-context-gauge-config-override-and-tabswap]]、兄弟 spec `2026-07-01-context-gauge-empty-session-estimate-design.md`（空会话占用预估，与本 spec 共用 `ContextPressure::compute` 传感器，互补不重叠）。

---

## 1. 背景与根因（已查证）

### 1.1 复现与证据

- 运行中 server = `/Applications/Aleph.app/Contents/MacOS/aleph-server`（内嵌、旧构建，不含任何近期未部署修复）。
- 用户 `~/.aleph/config.toml`：`[context_budget] enabled=true, warning_threshold=0.65, critical_threshold=0.85`（无显式 `token_budget` 覆盖）。
- 预算日志（`~/.aleph/logs/`）：`context budget derived ... model=Kimi-K2.7, window=262144, reserve=32768, usable=229376, chain_len=2`。MiniMax-M2.x 族窗口 = 204800。

### 1.2 两条"因满而停"的硬停路径

错误文案来自 `src/gateway/i18n.rs:163`（`TerminateReason::ContextBudgetExhausted` 的 zh 渲染）。"0 次迭代后"= 第一次 Think、首个 LLM 调用前，预算门 `before_turn` 就返回了终止指令。全栈共有**两条**因上下文满而终止 run 的路径，二者都必须被本 spec 消除：

| # | 路径 | 触发 | 终止理由 | 现状缺陷 |
|---|------|------|----------|----------|
| P1 主动 | `ContextBudget::before_turn`（`src/context/budget/mod.rs:452`） | 估算 `pressure.ratio >= critical_threshold` | `ContextBudgetExhausted`（think.rs:658） | **critical 路径直奔 `FinalReply`，根本不压缩**；warning 带 `[0.65,0.85)` 才压缩。重载一个估算 ≥critical 的会话 → 第 0 轮即硬停，且每次发消息都重新撞 critical → **永久砖死**。 |
| P2 反应式 | `try_reactive_compact_and_retry`（`src/harness/agent/think.rs:1431`） | provider 真返回 `CompactAndRetry`（context overflow，带 `token_gap`） | `ReactiveCompactExhausted`（think.rs:1462/1479/1509） | 压缩器未挂 / 重试 cap（`MAX_REACTIVE_COMPACT_ATTEMPTS`）耗尽 / 压缩失败 → 直接把 provider 错误抛给用户，无终极兜底。 |

### 1.3 仪表与预算口径分歧（"65% 却说满了"）

| | 分子 | 分母 |
|---|------|------|
| 仪表（65%） | provider 真实 `prompt_tokens` | **完整窗口**（`resolve_context_window_with_override`，如 204800/262144） |
| 预算门（critical） | 内容感知**字符估算**（CJK=1.5、code 更密 字符/token），重载时 `calibration=None`→×1.0 裸估算、系统性高估 | **`usable = 窗口 − 32768 输出预留`**（实测 229376） |

两偏差同向叠加（分母更小 + 分子高估）→ 仪表 65% 的会话在预算眼里可冲过 85%。再叠加仪表本身在切历史会话后是**陈旧/缺失值**（档②回放修复未部署），用户看到的 65% 甚至不是当前会话真实占用。

---

## 2. 目标与核心不变量

### 2.1 核心不变量（要立成 harness 级能力红线）

> **上下文压力永远不能终止一次 run。** 它只能触发"压缩 → 就地重写同一会话 → （终极）截断"。run 的终止只允许来自：迭代上限（`max_iterations`）、模型显式 stop、用户取消、或与"满"无关的 diminishing-returns 防空转。

落地后果：`TerminateReason::ContextBudgetExhausted` 与 `ReactiveCompactExhausted` **从"因满终止"语义中退役**——P1/P2 两条路径都不再以它们收场，改为始终压缩并继续。两个 enum 变体可保留（避免大改），但压力路径不再产出它们。

### 2.2 非目标（YAGNI / 范围外）

- 不动 diminishing-returns 停车（`StopDiminishing`）——那是"模型空转无产出"，非"上下文满"，是另一种正当停止。
- 不引入远端模型 catalog 同步 / SQLite 模型库（用户已选"内置 JSON + 用户覆盖文件"，远端同步列为未来二期）。
- 不引入 LLM 做压缩时机判断（压缩**时机**由确定性窗口感知阈值决定；压缩**内容摘要**复用现有 `summary_utils` 的既有 LLM 摘要，非新增推理层）。

---

## 3. 已查证的技术地基（复用而非新造）

| 砖块 | 位置 | 作用 |
|------|------|------|
| `ContextBudget::before_turn` / `peek_pressure` / `note_compaction_effect` | `src/context/budget/mod.rs` | 主动预算门 + 压力快照 + 压缩效果回标。`LoopDirective::{Continue,CompactAndContinue,SplitSession,FinalReply,StopDiminishing}`。 |
| `ContextPressure::compute(msgs, sys, tool_tokens, budget, ratio)` | `src/context/budget/pressure.rs` | 内容感知 token 估算传感器（CJK/code/prose 比率）。`calibration: Option<f64>` EWMA 自标定。 |
| `Compactor::compact(&mut messages, fresh_tail, session)` | `src/context/compact/compactor.rs:190` | 压缩原语，**当前只改 in-flight 消息 vec（短暂、不落盘）**。 |
| `perform_session_split(...)` + `summarize_pretail` | `src/context/compact/session_split.rs:46` | 摘要 pre-tail + 新鲜尾 → **子会话**（epoch+1）。本 spec 复用其摘要原语、改落本会话。 |
| `SessionEvent::CompactionPerformed { from_seq, to_seq, summary_ref, at }` | `src/session/events.rs:260` | **就地压缩标记已存在**。语义（`src/session/store.rs` 注释）：压缩"从工作上下文窗口逐出旧轮次，但事件在 store 中存活"（保 FTS/BM25 可检索）= append-only 安全、raw 不删。 |
| `try_reactive_compact_and_retry` | `src/harness/agent/think.rs:1431` | 反应式压缩+重试（P2 路径）。 |
| `build_context_budget_config` / `derive_token_budget` / `derive_chain_min_budget` | `src/orchestrator/deps_builder.rs:715/593` | 从模型窗口派生预算：`provider.context_window` ▸ catalog ▸ 保守兜底；`usable = window − reserve`；`window_aware_warning_default` / `window_aware_fresh_tail`。 |
| `CAPABILITY_TABLE` / `capabilities_for` / `resolve_context_window_with_override` / `CONSERVATIVE_CONTEXT_WINDOW`(128K) | `src/providers/model_catalog/capabilities.rs` | 模型窗口唯一源（仪表 + 预算共用）。现覆盖 Claude/GPT/o/Gemini/DeepSeek/Grok/Mistral/MiniMax/Kimi-Moonshot/GLM/Qwen/Llama/Command/Sonar。**硬编码 Rust，改窗口需重编。** |
| `RunContextOccupancy` 持久化 + `occupancy_from_history` 回投 | 工作树未部署（[[project-context-gauge-history-replay]]） | 切历史会话回填仪表。 |

**关键洞察**：就地压缩所需的事件模型（`CompactionPerformed`）、压缩原语（`compact`）、摘要原语（`summarize_pretail`）、窗口源（catalog）**都已存在**；本 spec 主要是 (a) 改预算决策不再硬停、(b) 把短暂压缩升级为就地落盘、(c) 把硬编码 catalog 抽成可热更数据库、(d) 对齐仪表口径。

---

## 4. 已敲定的设计决策

| # | 决策 | 取值 |
|---|------|------|
| D1 | 满溢终极逃逸 | **原会话内就地重写**（`session_key` 不变；旧消息回放时被一段摘要替换；raw 事件保留供 FTS）。 |
| D2 | 模型覆盖范围 | **多个主流模型皆保证**（非仅 MiniMax）。 |
| D3 | 模型上下文数据库形态 | **内置 `models.json`（`include_str!` 编译期嵌入）+ `~/.aleph/models.toml` 用户覆盖**，启动合并、不重编可改。远端同步=未来二期。 |
| D4 | 仪表语义 | **纯占用指示器** = provider 真实 prompt tokens ÷ 完整窗口。系统永不拒绝 → 无"说满了"矛盾；就地重写后历史变小、仪表自然回落。 |
| D5 | 推进方式 | 先出本 spec → writing-plans。 |

---

## 5. 工作流 1 —— 永不中断（消除 P1/P2 两条硬停）

### 5.1 主动路径 P1：`before_turn` 决策改写

`src/context/budget/mod.rs::before_turn`：

1. **critical（`ratio >= critical_threshold`）不再返回 `FinalReply`**，改返回新指令 `CompactToFit`（语义："压到能放下为止"）。
2. warning 带 `[warning, critical)` 维持 `CompactAndContinue`（含 circuit breaker → `SplitSession`/聚合压缩，见 5.3）。
3. `FinalReply` 在压力路径**不可达**。

`src/harness/agent/think.rs` 对 `CompactToFit` 的处理（落在已有 `CompactAndContinue` 分支旁、机械分派，不新增 harness 文件）：

- 调 `compactor.compact(&mut messages, fresh_tail, session)`。
- 压缩后用 `peek_pressure` 复测；若仍 ≥critical → **升级 5.3 聚合压缩**（摘要到 fresh tail）。
- 若聚合后仍 ≥critical（fresh tail + system + tools 本身超窗）→ **5.4 截断地板**。
- 三级保证："总能压到 < critical 再发请求"，因此 LLM 调用前永远不会因满而停。

### 5.2 run 启动体检（iteration 0）

第 0 轮、首个 LLM 调用前，对重建出的 prompt 跑一次 `before_turn`；若 ≥warning 即先压缩（≥critical 即走 5.1 三级）。**这一条直接解砖**"切到 near-full 历史会话 → 第一条消息即 `ContextBudgetExhausted`"。当前 think.rs 已在第 0 轮调 `before_turn`（line 524）；改动在于其返回 critical 时走压缩而非硬停——无需新缝。

### 5.3 聚合压缩（circuit breaker 升级，不再以 FinalReply 收场）

现状：warning 带连续 3 次压缩无效 → breaker trip → `SplitSession`（splits 用尽再 `FinalReply`）。改为：

- breaker trip → 优先**就地聚合压缩**（`summarize_pretail` 落本会话，见 §6），把 pre-tail 全量摘成一段。
- 仅当用户/配置显式选择"满溢分裂到子会话"时才走 `SplitSession`（D1 已定就地重写为默认；split 保留为可选策略，不在本期默认路径）。
- `FinalReply` 从这里彻底移除。

### 5.4 反应式路径 P2：`try_reactive_compact_and_retry` 兜底

provider 真返回 overflow 时：

- cap（`MAX_REACTIVE_COMPACT_ATTEMPTS`）耗尽 / 普通压缩仍 overflow → **不再 `ReactiveCompactExhausted` 抛错**，改走 5.3 聚合压缩 + 5.4 截断地板后再重试一次。
- 截断地板保证压缩后的 prompt 必定 < 窗口 → 重试必不再 overflow。
- 仅当**截断后仍被 provider 拒**（理论不可达：单条 system/tool schema 已超窗的病态配置）才如实抛错——这属于"配置错误"而非"对话满"，文案区分。

### 5.5 截断地板（终极保证，P7 优雅降级）

当摘要到 fresh tail 后估算仍 ≥ 窗口（极端：单条巨型 tool 结果 / fresh tail 自身超窗）：

- 对超窗部分按 UTF-8 安全边界（`char_indices`，遵 P7）**硬截断并插入 `[...truncated N tokens...]` 标记**。
- 优先截断最旧、最大的非 fresh-tail 内容；fresh tail 内的巨型单条消息按"保头尾、中段截断"处理。
- 这是"无论如何都能塞进窗口"的数学保证——永不中断的最后一道地板。

---

## 6. 工作流 2a —— 就地会话重写（落盘持久化）

把 §5 的短暂压缩升级为**对同一会话事件日志的 append-only 就地压缩**，复用既有 `SessionEvent::CompactionPerformed`：

1. 摘要 `from_seq..to_seq` 的 pre-tail 对话（复用 `summarize_pretail`）→ 得 `summary_ref`（摘要文本或其存储引用）。
2. 向**本会话**追加 `CompactionPerformed { from_seq, to_seq, summary_ref, at }`（不删 raw 事件；raw 仍 FTS 可检索）。
3. `compaction_count`（`sessions.n_count` 列，已存在）++。
4. **历史回放 / `build_prompt` 必须遵守压缩标记**：回放时跳过 `from_seq..to_seq` 的对话事件、改注入 `summary_ref`，再接 fresh tail。
   - ⚠️ **计划阶段必验**：现状回放（`src/session/store.rs` / `src/orchestrator/harness_bridge/prompt_build.rs`）是否已 honor `CompactionPerformed`。`store.rs` 注释表明"压缩从工作上下文逐出旧轮次"——意图在，但需确认 build_prompt 实际跳过逻辑是否完整、或仅子会话 split 路径用过。这是本 spec **最高实现风险点**，plan 第一步即验证。

**效果**：一旦压过，会话持久变小 → 下次重载 prompt 直接小、provider prompt_tokens 降、仪表自然回落 → "切历史对话顺利进行"（用户目标）。`session_key` 不变（D1）。

---

## 7. 工作流 2b —— 模型上下文数据库（内置 JSON + 用户覆盖）

### 7.1 抽出为数据文件

- 把 `CAPABILITY_TABLE` 的内容外化为 **`src/providers/model_catalog/models.json`**（结构同 `ModelCapabilities`：`prefix, context_window, max_output_tokens, supports_vision/tools/reasoning`），经 `include_str!` 编译期嵌入 → 单机零配置、离线可用（与 pricing 表同 stance）。
- `capabilities_for` 改为解析这份内置 JSON（启动一次、缓存为静态表），**查找语义不变**（前缀匹配、specific 先于 broad）。

### 7.2 用户覆盖

- 启动时若存在 `~/.aleph/models.toml`，解析并**合并覆盖**内置默认（同 prefix → 用户值胜；新 prefix → 追加）。
- 用途：为 kimi-for-coding / t8star / 302ai 等**代理端点**或内置库未收录的模型，不重编即增改窗口/输出上限。
- 优先级（窗口解析）：`[providers.*] context_window`（单 provider 级）▸ `models.toml`（模型级覆盖）▸ 内置 JSON ▸ `CONSERVATIVE_CONTEXT_WINDOW`(128K)。
- `models.toml` schema 与合并语义在 plan 细化；解析失败 = 整体回退内置默认 + 日志告警（P7，不崩）。

### 7.3 单一来源

仪表（`resolve_context_window_with_override`）与预算（`derive_token_budget`）**继续共用**这份数据库（现已如此）→ 窗口口径天然一致，跨主流模型（D2）"按各自窗口在最合适时机压缩"（用户目标）由 `window_aware_warning_default` / `window_aware_fresh_tail` 提供。

---

## 8. 工作流 2c —— 仪表对齐 + 标定持久化

1. **仪表 = 纯占用**（D4）：provider 真实 prompt tokens ÷ 完整窗口。系统永不拒绝 → "65% 却说满了"矛盾消失。就地重写后历史变小 → 下一轮 provider 数自然降 → 仪表回落。
2. **部署档②历史回放修复**（[[project-context-gauge-history-replay]]，已在工作树）：切历史会话从持久占用 metadata 回填仪表，不再陈旧/消失。
3. **标定因子持久化**：`ContextBudget::calibration: Option<f64>` 当前重载归零 → 裸高估。按"模型 → EWMA 标定因子"存盘（落 `~/.aleph` 或既有 store），重载回填 → 预算内部估算贴近 provider 实际、减少误判提前压缩。*（这是修预算估算准确度，不改 D4 仪表口径。）*
4. 与兄弟 spec（空会话占用预估 `≈`）协同：本 spec 让"有真实记录/压过的会话"占用正确回落；兄弟 spec 让"从未跑过的空会话"显示 `≈` 预估。互补。

---

## 9. 边界与降级

| 场景 | 行为 |
|------|------|
| 重载 near-full 历史会话发首条消息 | run 启动体检（§5.2）压缩 → 顺利继续（核心修复） |
| 连续对话逼近窗口 | warning 带就地压缩；critical 不再硬停，聚合压缩兜底 |
| provider 真 overflow | 反应式压缩 + 聚合 + 截断地板后重试，不再抛 `ReactiveCompactExhausted` |
| fresh tail / 单条消息本身超窗 | UTF-8 安全截断地板，永远塞得进 |
| 代理端点模型库未收录 | `models.toml` 覆盖；未覆盖则 128K 保守兜底 + 告警 |
| `models.toml` 解析失败 | 回退内置 JSON + 告警，不崩 |
| 压缩器/registrar 未挂（降级配置） | 截断地板仍保证不中断；摘要质量下降但 run 不停 |

---

## 10. 测试

- **预算决策（`src/context/budget`）**：critical 永不返回 `FinalReply`（返回 `CompactToFit`）；warning 仍 `CompactAndContinue`；breaker trip 走聚合压缩不走 FinalReply。
- **就地压缩持久化（`src/session` / `src/context/compact`）**：`CompactionPerformed` 落盘后回放跳过 `from_seq..to_seq` 用 summary；raw 事件仍 FTS 可查；`compaction_count`++；`session_key` 不变；持久 prompt 体积下降。
- **截断地板**：单条超窗消息 UTF-8 安全截断、加标记、估算 < 窗口。
- **反应式路径**：overflow → 聚合+截断后重试成功，不出 `ReactiveCompactExhausted`。
- **模型数据库**：内置 JSON 解析 = 旧 `CAPABILITY_TABLE`（行为保持，逐族断言窗口）；`models.toml` 覆盖生效；解析失败回退；优先级链（provider ▸ toml ▸ json ▸ 128K）。
- **仪表**：占用值随就地重写下降；标定因子重载回填（非归零）。
- **回归（端到端）**：精确复现"切到 near-full 历史会话 → 发消息 → 顺利继续（不再 `ContextBudgetExhausted（0 次迭代后）`）"。

---

## 11. R10 合规 + 加代码前必答 3 问

**文件落点**：`src/context/budget/`（决策）、`src/context/compact/`（就地重写机制）、`src/session/`（事件回放遵守压缩标记）、`src/providers/model_catalog/`（数据库）、panel（仪表口径）。**`src/harness/` 的 12 文件 / 4900 行不动**——think.rs 对新指令 `CompactToFit` 的处理落在已有 `CompactAndContinue` 分支旁的机械分派，不新增文件、不引入推理。

1. **脚手架还是认知？** 压缩时机=确定性窗口感知阈值（脚手架）；摘要内容复用既有 LLM 摘要原语（非新推理层）。✓
2. **模型升级一档还需要它吗？** 需要——上下文窗口是物理约束，与模型能力正交；模型越强、窗口管理越要稳。✓
3. **现在有几个真实消费者？** 1 个明确（用户撞到砖死 + 立目标）；覆盖所有走 `before_turn`/反应式压缩的 agent run。✓

---

## 12. 范围与分期（writing-plans 输入）

建议 plan 按风险排序、可独立验证：

1. **P0 解砖（工作流 1 主动路径）**：`before_turn` critical → `CompactToFit`；run 启动体检；think.rs 机械分派。最小改动让 near-full 会话能继续。
2. **P0 就地压缩落盘验证（工作流 2a）**：先验回放是否 honor `CompactionPerformed`（最高风险），再补落盘 + 回放跳过。
3. **截断地板 + 反应式兜底（工作流 1 余下）**：终极保证。
4. **模型数据库（工作流 2b）**：抽 JSON + `models.toml` 覆盖。
5. **仪表对齐 + 标定持久化（工作流 2c）**：部署档②回放 + 占用口径 + 标定存盘。

每步独立编译/单测通过；全部完成后重编 `/Applications/Aleph.app` 内嵌 server 做运行时 QA。
