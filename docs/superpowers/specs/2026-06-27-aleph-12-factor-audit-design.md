# Aleph × 12-Factor Agents · 宪法对照审查与采纳 — Design Spec

> **Date**: 2026-06-27
> **Status**: Design approved (brainstorming) → 待 writing-plans
> **Type**: 文档 / 宪法（CLAUDE.md）+ 参考文档（Tier 2）。**本 spec 不改任何 Rust 代码**——代码改造拆为后续独立 plan（见 §6 backlog）。
> **参考项目**: `/Volumes/TBU4/Github/12-factor-agents`（HumanLayer，CC BY-SA 4.0）

---

## 0. 锁定的决策（brainstorming 产出）

| 决策 | 选定 |
|------|------|
| 宪法改法 | **叠加映射层，R1–R10 / P1–P8 一字不动**。缺口的硬约束以"采纳条款"形式存在，**非新红线** |
| 模块边界 | **文档到位，代码拆后续**。本 spec 产出 = CLAUDE.md 新节 + AUDIT 文档（含模块改造建议清单）；代码留 backlog |
| 落地结构 | **两层**：宪法只放精简映射 + 采纳条款；详细审计 + backlog 单开 Tier-2 文档（护 R3 核心轻量化） |
| 采纳条款数量 | **4 条**（F3 / F9 / F5+F12 / F6）。其中 F3、F9 为高价值零风险文档级澄清；F5+F12、F6 为方向性原则 + backlog |

---

## 1. 背景与目标

**背景**：Aleph 宪法（CLAUDE.md 的 R1–R10 红线 + P1–P8 设计原则）已天然内化 12-Factor Agents 的多数原则，部分（F8 自有控制流 / F11 触发无处不在）甚至**超越**参考实现。但：

1. 个别招牌能力（F3 context engineering）实现极强，**宪法层却无任何命名条目**；
2. 个别原则与现有红线存在**易被误读的张力**（F9 错误压缩 vs R10「不做错误恢复策略选择」）；
3. 个别架构性原则（F5/F12 状态统一与 reducer 纯度、F6 launch/pause/resume）**机件齐全但分散、未被作为一组契约命名**。

**目标**：对 Aleph 做一次系统的 12-Factor 合规审查，把"必要原则"以**叠加、可审计、不动红线**的方式落入宪法，并为相关模块产出**分优先级、带锚点**的改造建议清单——全部到文档为止，代码实现拆后续。

**非目标**（YAGNI / P6）：
- 不重写或收紧 R1–R10 / P1–P8 任何一条；
- 不在本轮改任何 Rust 代码、不跑 `cargo`（除非后续独立 plan 显式需要）；
- 不为 F5/F12 这类架构缺口在本轮做任何实现——仅列为待评估 backlog。

---

## 2. 参考与方法

- **原则来源**：12-Factor Agents 全 12 条 + 附录 F13（pre-fetch context）。
- **现状证据**：`docs/reference/FEATURE_LOCATOR.md`（Prompt→Context→Harness→Loop 四层 + 横切，带文件锚点）+ CLAUDE.md R/P。
- **裁定方法**：逐 factor 对照"已对应 R/P + 代码锚点"，给 ✅（已内化/超越）/ ⚠️（实现存在但宪法缺口或有张力）。AUDIT 文档中每条裁定**须引代码锚点**，并在撰写时按需读源码复核（不需 `cargo`）。

---

## 3. 12-Factor → Aleph 对照（复核裁定）

> 此表是 CLAUDE.md 新节映射表的**权威母本**（宪法里放精简版，AUDIT 文档放带证据版）。

| # | Factor | 已对应 R/P | 主锚点 | 裁定 |
|---|--------|-----------|--------|------|
| F1 | Natural Language → Tool Calls | R7 / R8 / P8 | `harness/agent/act.rs` · `tools/scoped/` | ✅ 已深度内化 |
| F2 | Own your prompts | R9 | `thinker/prompt_pipeline.rs` + `layers/`(40+) | ✅ 超额覆盖 |
| F3 | **Own your context window** | （**宪法无条目**）↔ R9 间接 | `context/`（compact/budget/cheap_passes/structured）§2.1–2.8 | ⚠️ **实现极强，宪法零命名** → **采纳 A1** |
| F4 | Tools are structured outputs | R8 | `providers/adapter.rs::NativeToolCall` · `tools/runtime.rs::LoopTool` | ✅ 输出/执行已解耦，撤出缺口名单 |
| F5 | Unify execution & business state | R10 | `harness/trait_def.rs::TurnState` · `goal/` · `looping/`(内存态) · `agents/swarm/tasks/store/` | ⚠️ 无单一 event-log 状态源 → **采纳 A3** |
| F6 | Launch / Pause / Resume | R5 / R6 | `gateway/cancellation.rs` · `resume_coordinator.rs` · `workflow/` · `execution_engine/steering.rs` | ⚠️ 机件齐全，缺统一契约命名 → **采纳 A4** |
| F7 | Contact humans with tool calls | R5 | `clarification/` · `builtin_tools/ask_user.rs` · `approval/` | ✅ |
| F8 | Own your control flow | **R10（哲学高度自觉）** | `harness/agent/{think,act}.rs` | ✅ 典范，已超越 hermes/openclaw/pi |
| F9 | Compact errors into context | R10（第 5 不） | §2.7 `cheap_passes/structured/log.rs` · ToolError 事件 · `think.rs` 有界恢复 | ⚠️ **compact≠recovery 边界无处写清** → **采纳 A2** |
| F10 | Small, focused agents | R3 | `teams/` · `agents/`(subagent_spawner) | ✅ |
| F11 | Trigger from anywhere | R5 / R6 | `gateway/` · `tasks/cron/` · channels | ✅ 核心招牌 |
| F12 | Stateless reducer | R10 | `harness/agent/prompt.rs`（逐轮重建裸消息）· `context/compact`（增量摘要继承） | ⚠️ reducer-like 但非纯 → **与 F5 合并 采纳 A3** |
| F13 | (附录) Pre-fetch context | — | `context/assembler/` · memory 召回 | ✅ 已具备（assembler 主动召回）；仅 AUDIT 备注，不立条款 |

**净缺口 = 4**：F3（A1）、F9（A2）、F5+F12（A3）、F6（A4）。

---

## 4. 设计：两层落地

### 4.1 第一层 — CLAUDE.md 新增一节（精简、宪法级）

- **位置**：置于 `## 🧬 设计原则 (Design Principles)` 之后、`## 🛠 技术栈与禁用清单` 之前。
- **标题**：`## 🧭 12-Factor 对照与采纳 (12-Factor Conformance & Adoption)`
- **内容**（紧凑，控制篇幅护 R3）：
  1. 一句话引子（这一节是叠加映射层，**不改 R/P，不设新红线**）；
  2. **精简映射表**（12 行：Factor → 一句话 → 已对应 R/P → ✅/⚠️），由 §3 母表蒸馏；
  3. **4 条采纳条款 A1–A4**（见 §5，每条 ≤ 6 行，交叉引用 R/P，**显式标注「采纳·非红线」**）；
  4. 指针：`> 逐 factor 证据、锚点与模块改造建议清单见 [TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md)`。
- **同时**：在 CLAUDE.md `## 📚 文档索引` 的表格里登记新文档（Tier 2）。

### 4.2 第二层 — 新建 `docs/reference/TWELVE_FACTOR_AUDIT.md`（Tier 2，按需加载）

结构：
1. **§A 逐 factor 审计**：F1–F13 每条 = 原则一句话 + Aleph 现状 + 代码锚点 + 裁定 + （⚠️ 条目）缺口说明。**每条须引锚点**，撰写时按需读源码复核 F5/F12/F6 的架构性论断。
2. **§B 模块改造建议清单**：见 §6（P0/P1/P2 分级，带文件锚点，只描述不改码）。
3. **§C 与红线的关系**：明示采纳条款如何叠加在 R/P 之上、为何不是新红线、F9 与 R10 的读法说明。

---

## 5. 四条采纳条款（措辞骨架 — 即将写入 CLAUDE.md）

> 统一前缀 **A**（Adoption 采纳）以区别于 R（红线）/ P（原则）。每条结尾固定一行 `↳ 采纳·非红线`。

**A1 · 自有 Context Window（Own Your Context Window，F3）**
> Aleph 的 agent 按 **Prompt → Context → Harness → Loop** 四层构建；其中 **Context 层**（历史压缩 / 按模型窗口的压缩时机 / 内容类型路由缩减 / FTS5 检索回注 / 记忆三支柱）是一等工程关切，**context 的取舍由我们显式拥有**，不外包给框架默认消息格式。
> 关联：R9（智慧在 prompt）、P6（简洁）。锚点：`src/context/`、`src/thinker/`。
> ↳ 采纳·非红线

**A2 · 错误压缩 ≠ 错误恢复（Compact Errors, Not Recover Strategies，F9）**
> **采纳**：把工具/Provider 错误**压缩并呈递给模型**，让模型在下一轮自愈（§2.7 内容路由缩减、ToolError 事件、`think.rs` 有界 provider-failure drain）。
> **仍禁**（= R10 第 5 不）：在 harness 里做**确定性的"错误恢复策略选择 / 多级重试矩阵"**（hermes 式 fat-harness 重试有意不移植）。
> 边界一句话：**让模型看见并自愈错误 = 要；让 harness 替模型挑恢复策略 = 不要**。
> 关联：R7 / R10。锚点：`context/budget/cheap_passes/structured/`、`harness/agent/think.rs`。
> ↳ 采纳·非红线（本条是 R10 的读法说明，不改 R10 一字）

**A3 · 状态可重建，趋向纯 Reducer（Reconstructible State, Toward a Pure Reducer，F5+F12）**
> **方向性承诺**：执行状态应尽量可从**单一持久源**重建；每轮 Think 趋向"对持久 context 的纯 reduce"（`prompt.rs` 已逐轮重建裸消息）。新增有状态机件时，优先让其状态可观测、可重建。
> **硬约束**：**不得**为此让 `src/harness/` 越过 R10 的 12 文件 / ~4900 行预算，或把业务状态搬进笨循环。具体统一方案先走"加代码前必答 3 问"，列为 backlog 评估（§6 P2）。
> 关联：R10、P4（依赖倒置）。锚点：`harness/trait_def.rs::TurnState`、`looping/`（内存态）、`goal/`、`agents/swarm/tasks/store/`。
> ↳ 采纳·非红线（方向性原则，落地需独立评估）

**A4 · 统一 Launch / Pause / Resume 契约（Unified Lifecycle Contract，F6）**
> 把已存在的取消（`cancellation.rs`）、续跑（`resume_coordinator.rs`）、改需求打断/注入（`steering.rs` 三态 Steer/Interrupt/Queue）、workflow resume 命名为**一组生命周期契约**：任何长跑单元（goal / loop / workflow / team task）都应可被一致地启动、暂停、恢复、取消。
> 关联：R5（AI 主动到达）、R6（一核多端）。统一 API 面（薄 facade，**不进 harness**）列为 backlog（§6 P1）。
> ↳ 采纳·非红线

---

## 6. 模块改造建议清单（写入 AUDIT §B）

> 分级：**P0** = 文档级零代码风险；**P1** = 轻代码、独立、不碰 harness；**P2** = 架构性、需评估。每项注明锚点、性质、为何不在本轮做。

### P0（文档级，可随本轮文档一并落地）
- **P0-1（A1/F3）**：在 AUDIT 写清 Context 层四层叙事与各子系统锚点（直接引 FEATURE_LOCATOR §2.x）。性质：纯描述。
- **P0-2（A2/F9）**：在 AUDIT §C 写"错误压缩 vs 恢复"读法说明，消除 R10 误读。性质：纯描述。（**可选**：在 FEATURE_LOCATOR §3.1 既有 R10 注解旁加一行交叉链接到 A2——属本轮文档范围外的顺带优化，不强制。）

### P1（轻代码，独立，后续各自 plan）
- **P1-1（A4/F6）**：盘点 `cancellation` / `resume_coordinator` / `steering` / `workflow` resume 的现有入口，评估是否值得一层**薄 facade / trait** 统一命名"生命周期契约"。约束：facade 落在 gateway/loop 层，**绝不进 `src/harness/`**（R10）。先产出"现状映射 + 是否需要 facade"的判断，再决定是否实现。
- **P1-2（A1/F3，可选）**：评估是否把 F13 pre-fetch 的 assembler 主动召回在 AUDIT 标注为"已满足"即可，无需代码。

### P2（架构性，仅列为待评估 backlog，本轮不动）
- **P2-1（A3/F5+F12）**：评估"执行状态单一可重建源"。当前 loop 状态纯内存（重启即丢，设计意图）、goal/task 各自持久、harness TurnState 逐轮重建。问题：是否值得引入统一的可重建状态视图？**必须先过"加代码前必答 3 问"**（脚手架 vs 认知 / 模型升级后是否还需要 / 是否有真实消费者）。**预判**：很可能 YAGNI——现状的"逐轮重建裸消息 + 持久 session + 增量摘要继承"已覆盖多数 reducer 收益；除非出现真实的跨重启执行恢复需求，否则不实现。AUDIT 须诚实记录此预判。

---

## 7. 范围边界 / 非目标（再申明）

- ✅ 改 `CLAUDE.md`（新增一节 + 文档索引登记）；新建 `docs/reference/TWELVE_FACTOR_AUDIT.md`。
- ❌ 不改 R1–R10 / P1–P8 任何一条文字。
- ❌ 不改任何 `src/**` Rust 代码；不跑 `cargo`。
- ❌ 不实现 A3（F5/F12）/ A4（F6）的代码部分——仅 backlog。

---

## 8. 验收标准

1. CLAUDE.md 新节存在、位置正确（设计原则后 / 技术栈前），含：精简 12 行映射表 + A1–A4 四条（每条带 `↳ 采纳·非红线`）+ AUDIT 指针。
2. R1–R10 / P1–P8 原文 `git diff` 为**零改动**（仅新增节 + 索引表一行）。
3. `docs/reference/TWELVE_FACTOR_AUDIT.md` 存在，含 §A 逐 factor（F1–F13，每条引锚点）+ §B P0/P1/P2 backlog + §C 红线关系。
4. AUDIT 中每条 ⚠️ 裁定可由所引锚点佐证（F5/F12/F6 的架构论断在撰写时读源码复核过）。
5. CLAUDE.md 文档索引表登记了新文档。
6. 全程零 Rust 代码改动、零 `cargo`。
7. 文档遵循仓库既有风格（中文 + English 规范名，对齐 FEATURE_LOCATOR）。

---

## 9. 风险与约束

- **R3 核心轻量化**：宪法是 Tier-1 每次加载，故新节必须精简；详细内容下沉 Tier-2 AUDIT。这正是两层设计的理由。
- **R10**：A3/A4 的任何后续实现都**不得**让 `src/harness/` 膨胀；facade 落 gateway/loop 层。
- **误读风险**：A2 必须把"压缩并呈递"与"恢复策略选择"切干净，否则读者可能以为 Aleph 要引入 hermes 式重试——AUDIT §C 专门澄清。
- **裁定漂移**：FEATURE_LOCATOR 锚点会随重构漂移；AUDIT 撰写时以当时代码为准复核 F5/F12/F6。

---

## 10. 后续（writing-plans 将规划什么）

本 spec 通过后，writing-plans 规划的"实现"= **纯文档工作**，有界：
1. 写 `CLAUDE.md` 新节《12-Factor 对照与采纳》（映射表 + A1–A4 + 索引登记）；
2. 写 `docs/reference/TWELVE_FACTOR_AUDIT.md`（§A 逐 factor + §B backlog + §C 红线关系，撰写时按需读源码复核）；
3. 验收对照 §8；按仓库约定 `git add -f`（docs/* 默认 gitignore）+ 提交。

代码 backlog（P1-1 facade 评估、P2-1 状态源评估）各自成独立后续 spec/plan，**不在本轮**。
