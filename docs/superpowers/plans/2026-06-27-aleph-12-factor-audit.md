# Aleph × 12-Factor 宪法对照审查与采纳 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 12-Factor Agents 对 Aleph 的合规审查落成两份文档——新建 Tier-2 审计文档 `docs/reference/TWELVE_FACTOR_AUDIT.md`，并在宪法 `CLAUDE.md` 叠加一节《12-Factor 对照与采纳》（精简映射表 + 4 条采纳条款），R1–R10 / P1–P8 一字不动。

**Architecture:** 两层落地（护 R3 核心轻量化）。详细逐 factor 证据、锚点与模块改造 backlog 沉到 Tier-2 审计文档；宪法只放蒸馏后的精简映射表 + 采纳条款 A1–A4（工程承诺级，**非红线**）。先写审计文档（母本/证据），再把精简版蒸馏进宪法。

**Tech Stack:** 纯 Markdown 文档。无代码、无构建、无 `cargo`。验证 = `git diff` / `grep` / `ls`。

## Global Constraints

> 每个任务的要求都隐含包含本节。数值/字符串从 spec 逐字复制。

- **R1–R10 / P1–P8 原文零改动**——CLAUDE.md 的唯一改动是「新增一节 + 文档索引表加一行」，红线/原则正文 `git diff` 必须为纯新增、零删除/零改写。
- **不改任何 `src/**` Rust 代码；不跑 `cargo`。**
- **执行在 `main` 分支直接进行**（项目 单分支开发模式）；不开 worktree。
- **文档风格**：中文叙述 + English 规范名，对齐 `docs/reference/FEATURE_LOCATOR.md`。
- **docs/* 默认 gitignore** → 提交用 `git add -f`。
- **采纳条款前缀 `A`**（区别于 R 红线 / P 原则）；每条结尾固定一行 `↳ 采纳·非红线`。
- **新节标题**（逐字）：`## 🧭 12-Factor 对照与采纳 (12-Factor Conformance & Adoption)`
- **新节位置**：`## 🧬 设计原则 (Design Principles)` 之后、`## 🛠 技术栈与禁用清单` 之前。
- **净缺口 = 4**：F3→A1、F9→A2、F5+F12→A3、F6→A4。F4/F8/F13 等其余 factor **不立条款**。
- **来源 spec**：`docs/superpowers/specs/2026-06-27-aleph-12-factor-audit-design.md`（§3 裁定表、§5 条款骨架、§6 backlog、§8 验收为权威母本）。

---

### Task 1: 新建审计文档 `docs/reference/TWELVE_FACTOR_AUDIT.md`

**Files:**
- Create: `docs/reference/TWELVE_FACTOR_AUDIT.md`
- Read-only 复核（不改）: `src/harness/agent/prompt.rs`, `src/harness/trait_def.rs`, `src/looping/`, `src/goal/store.rs`, `src/agents/swarm/tasks/store/`, `src/gateway/cancellation.rs`, `src/gateway/resume_coordinator.rs`, `src/gateway/execution_engine/steering.rs`, `src/workflow/`

**Interfaces:**
- Consumes: spec §3（裁定表）、§5（A1–A4 骨架）、§6（backlog）。
- Produces: 文件 `docs/reference/TWELVE_FACTOR_AUDIT.md`，含锚点 `## A. 逐 factor 审计` / `## B. 模块改造建议清单` / `## C. 与红线的关系`——Task 2 的宪法新节指针指向它。

- [ ] **Step 1: 复核 ⚠️ factor 的架构性论断（读源码，不改）**

只为坐实 §A 里 F5/F12/F6 三条 ⚠️ 裁定的锚点真实性。运行：

```bash
cd /Volumes/TBU4/Workspace/Aleph
ls src/harness/agent/prompt.rs src/harness/trait_def.rs src/gateway/cancellation.rs src/gateway/resume_coordinator.rs src/gateway/execution_engine/steering.rs
ls -d src/looping src/goal src/agents/swarm/tasks/store src/workflow
grep -n "TurnState" src/harness/trait_def.rs | head
grep -rn "struct LoopState" src/looping/
```

Expected: 上述文件/目录均存在；`TurnState` 在 `trait_def.rs` 有定义；`LoopState` 在 `src/looping/` 有定义。
确认三件事，写进 §A 对应条目的"证据"：① harness 每轮在 `prompt.rs` 重建裸消息（reducer-like）；② `LoopState` 为进程内内存态、daemon 重启即丢；③ goal/task 各自持久（`goal/store.rs`、`tasks/store/`）但**无统一 event-log 状态源**；④ F6 的取消/续跑/steering/workflow-resume 四处机件**存在但分散、未作为一组契约命名**。
**若任一论断与代码不符**：以代码为准，在该条目据实修正裁定（诚实优先），并在本任务的 commit message 注明偏差。

- [ ] **Step 2: 写文档头 + §A 逐 factor 审计**

把以下内容写入文件开头。§A 每条格式固定：`原则一句话 / Aleph 现状 / 锚点 / 裁定`。✅ 条目锚点直接取自 spec §3 表；⚠️ 条目用 Step 1 复核过的锚点。

````markdown
# TWELVE_FACTOR_AUDIT.md — 12-Factor Agents 合规审计

> **用途**：Aleph 对 [12-Factor Agents](https://github.com/humanlayer/12-factor-agents) 的逐 factor 合规审计 + 模块改造建议清单。是宪法 `CLAUDE.md` §《🧭 12-Factor 对照与采纳》的**带证据母本**（宪法放蒸馏版，本文放全量）。
> **方法**：逐 factor 对照"已对应 R/P + 代码锚点"，给 ✅（已内化/超越）/ ⚠️（实现存在但宪法缺口或有张力）。锚点随重构漂移，发现不符就地更新。
> **生成**：2026-06-27，依据 `docs/reference/FEATURE_LOCATOR.md` + 源码复核。

## A. 逐 factor 审计

### F1 · Natural Language → Tool Calls — ✅
- 原则：自然语言转结构化工具调用。
- 现状：LLM 发原生 tool_call → harness 分批并/序执行 → 回 ToolResult/ToolError。
- 锚点：`src/harness/agent/act.rs`、`src/tools/scoped/`。已对应 R7/R8/P8。

### F2 · Own Your Prompts — ✅
- 原则：自有提示词，不外包给框架。
- 现状：40+ layer 优先级流水线，分 Basic/Soul/Context/Cached 路径组装。
- 锚点：`src/thinker/prompt_pipeline.rs` + `layers/`。已对应 R9（智慧在 prompt）。

### F3 · Own Your Context Window — ⚠️（→ 采纳 A1）
- 原则：显式拥有 context window 的取舍，不被默认消息格式绑架。
- 现状：Context 层极强——三策略历史压缩、按模型窗口的压缩时机、内容类型路由缩减、FTS5 检索回注、记忆三支柱。**但宪法层无任何命名条目**（R9 仅间接相关）。
- 锚点：`src/context/`（compact/budget/cheap_passes/structured）、`src/thinker/`。FEATURE_LOCATOR §2.1–2.8。
- 缺口：实现 ✅ / 宪法 ✗ → A1 给它正式命名。

### F4 · Tools Are Structured Outputs — ✅
- 原则：工具只是结构化输出，输出与执行解耦。
- 现状：`NativeToolCall` 结构化输出 ↔ `LoopTool` 执行解耦，三层管道（act 并行 / scoped 拦截 / result store 溢出）。
- 锚点：`src/providers/adapter.rs::NativeToolCall`、`src/tools/runtime.rs::LoopTool`、`src/harness/agent/act.rs`。已对应 R8。

### F5 · Unify Execution & Business State — ⚠️（→ 采纳 A3，与 F12 合并）
- 原则：执行状态与业务状态统一、可从单一源（理想为 context window）推断。
- 现状：harness `TurnState` 为逐轮执行态；业务态分散——`looping/` 纯内存（重启即丢）、`goal/` 持久、`tasks/store/` 持久。**无单一 event-log 状态源**。
- 锚点：`src/harness/trait_def.rs::TurnState`、`src/looping/`、`src/goal/store.rs`、`src/agents/swarm/tasks/store/`。
- 缺口：架构性，最深。落地需评估（B §P2-1），预判很可能 YAGNI。

### F6 · Launch / Pause / Resume — ⚠️（→ 采纳 A4）
- 原则：用简单 API 启动/暂停/恢复 agent。
- 现状：机件齐全但分散——取消、续跑、改需求三态打断/注入、workflow resume 各自独立，**未作为一组契约命名**。
- 锚点：`src/gateway/cancellation.rs`、`src/gateway/resume_coordinator.rs`、`src/gateway/execution_engine/steering.rs`、`src/workflow/`。已对应 R5/R6。
- 缺口：缺统一契约命名 + API 面（B §P1-1）。

### F7 · Contact Humans With Tool Calls — ✅
- 原则：人在环 = 结构化工具调用，而非特例分支。
- 现状：`ask_user` 工具 + clarification 闸 + 三级 approval。
- 锚点：`src/clarification/`、`src/builtin_tools/ask_user.rs`、`src/approval/`。已对应 R5。

### F8 · Own Your Control Flow — ✅ 典范
- 原则：自有控制流，自定义循环/中断/续跑。
- 现状：薄 harness 笨循环（Think→Act），R10 哲学高度自觉；多维度已**超越** hermes/openclaw/pi（资源域分群并行、保序、失败去重、有界恢复）。
- 锚点：`src/harness/agent/{think,act}.rs`。已对应 R10。

### F9 · Compact Errors Into Context — ⚠️（→ 采纳 A2）
- 原则：把错误压缩进 context，让模型自愈。
- 现状：**采纳侧**——错误经 §2.7 内容路由缩减、ToolError 事件、`think.rs` 有界 provider-failure drain 进 context。**禁止侧**——R10 第 5 不（不做确定性错误恢复策略选择 / hermes 式重试矩阵，有意不移植）。**这两件事的边界没有任何地方写清**，R10 易被误读成"别把错误给模型看"。
- 锚点：`src/context/budget/cheap_passes/structured/log.rs`、`src/harness/agent/think.rs`。
- 缺口：纯澄清，零代码（C §读法说明）。

### F10 · Small, Focused Agents — ✅
- 原则：小而专的 agent，而非巨型全能体。
- 现状：teams 多代理 + subagent spawner，3 道防风暴闸。
- 锚点：`src/teams/`、`src/agents/`。已对应 R3。

### F11 · Trigger From Anywhere — ✅ 核心招牌
- 原则：从任何渠道触发，在用户所在处相遇。
- 现状：多端通道（Telegram/Slack/Email/桌面/Panel）+ cron + daemon 事件。
- 锚点：`src/gateway/`、`src/tasks/cron/`。已对应 R5/R6。

### F12 · Stateless Reducer — ⚠️（→ 采纳 A3，与 F5 合并）
- 原则：agent = 对(状态, 事件)的纯 reduce。
- 现状：`prompt.rs` 逐轮重建裸消息 + 增量摘要继承 = reducer-like，但轮内 `TurnState`/有界恢复带状态、loop 纯内存，**非纯 reducer**。
- 锚点：`src/harness/agent/prompt.rs`、`src/context/compact`。
- 缺口：与 F5 同源，合并入 A3。

### F13 · (附录) Pre-fetch Context — ✅（仅备注，不立条款）
- 原则：预取可能需要的 context。
- 现状：assembler 主动召回记忆/上下文，已具备。
- 锚点：`src/memory/assembler/`、`src/context/retrieval/`、memory 召回链。

## B. 模块改造建议清单

> 分级：**P0** 文档级零代码风险；**P1** 轻代码独立不碰 harness；**P2** 架构性需评估。每项注明锚点、性质、为何不在本轮做。代码项各自成后续独立 spec/plan，**不在本轮**。

### P0（文档级）
- **P0-1（A1/F3）**：本文 §A-F3 已写清 Context 层叙事与锚点（引 FEATURE_LOCATOR §2.x）。性质：纯描述，本轮完成。
- **P0-2（A2/F9）**：本文 §C 写"错误压缩 vs 恢复"读法说明。性质：纯描述，本轮完成。（可选：FEATURE_LOCATOR §3.1 R10 注解旁加一行链到 A2——范围外顺带优化，不强制。）

### P1（轻代码，后续 plan）
- **P1-1（A4/F6）**：盘点 `cancellation`/`resume_coordinator`/`steering`/`workflow` resume 现有入口，评估是否值一层**薄 facade/trait** 统一命名"生命周期契约"。约束：facade 落 gateway/loop 层，**绝不进 `src/harness/`**（R10）。先产出"现状映射 + 是否需 facade"判断再决定实现。
- **P1-2（F13 pre-fetch，可选）**：F13 pre-fetch 在 §A 标"已满足"即可，无需代码。

### P2（架构性，仅 backlog，本轮不动）
- **P2-1（A3/F5+F12）**：评估"执行状态单一可重建源"。现状 loop 纯内存（重启即丢，设计意图）、goal/task 各自持久、harness 逐轮重建。**必须先过"加代码前必答 3 问"**（脚手架 vs 认知 / 模型升级后是否还需 / 真实消费者几个）。**预判**：很可能 YAGNI——现状"逐轮重建裸消息 + 持久 session + 增量摘要继承"已覆盖多数 reducer 收益；除非出现真实跨重启执行恢复需求，否则不实现。

## C. 与红线的关系

- 采纳条款 A1–A4 是**叠加在 R/P 之上的工程承诺**，**不是新红线**——它们交叉引用既有 R/P，不新增"违反不得合入"效力。
- **F9 / R10 读法说明（关键，消除误读）**：R10 第 5 不「不做错误恢复策略选择」约束的是**确定性 harness 替模型挑恢复策略 / 多级重试矩阵**；它**不**禁止"把错误压缩并呈递给模型让模型自己决定下一步"。换言之——**让模型看见并自愈错误 = 要（A2 采纳）；让 harness 替模型挑恢复策略 = 不要（R10 仍禁）**。`think.rs` 的 empty/ctx-overflow/max_tokens 有界 drain 是 provider-failure 的幂等恢复，不是策略选择，故合规。
- A3 的硬约束：任何后续实现**不得**让 `src/harness/` 越过 R10 的 12 文件 / ~4900 行预算。
````

- [ ] **Step 3: 验证文件结构与锚点**

Run:

```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -nE '^## (A|B|C)\.' docs/reference/TWELVE_FACTOR_AUDIT.md
grep -c '### F' docs/reference/TWELVE_FACTOR_AUDIT.md
```

Expected: §A/§B/§C 三个二级标题都在；`### F` 计数 = 13（F1–F13）。

- [ ] **Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add -f docs/reference/TWELVE_FACTOR_AUDIT.md
git commit -m "docs: add 12-factor conformance audit reference (F1-F13 + module backlog)"
```

---

### Task 2: 宪法 `CLAUDE.md` 叠加《12-Factor 对照与采纳》节 + 文档索引登记

**Files:**
- Modify: `CLAUDE.md`（新增一节 + 文档索引表加一行；R/P 正文零改）

**Interfaces:**
- Consumes: Task 1 产出的 `docs/reference/TWELVE_FACTOR_AUDIT.md`（新节指针指向它）。
- Produces: 宪法新节 `## 🧭 12-Factor 对照与采纳 (12-Factor Conformance & Adoption)`。

- [ ] **Step 1: 定位插入点（设计原则节末、技术栈节前）**

Run:

```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n '^## 🛠 技术栈与禁用清单\|^## 🧬 设计原则\|^## 📚 文档索引' CLAUDE.md
```

Expected: 拿到三个节的行号。新节插在「🧬 设计原则」节内容**结束后**、「🛠 技术栈与禁用清单」标题**之前**（即技术栈标题行的正上方，前后留一空行与既有 `---` 分隔符风格一致）。

- [ ] **Step 2: 插入新节（逐字写入以下内容）**

在 Step 1 定位的「🛠 技术栈」标题正上方插入：

````markdown
## 🧭 12-Factor 对照与采纳 (12-Factor Conformance & Adoption)

> 本节是叠加在 R1–R10 / P1–P8 之上的**映射层**——**不改任何红线/原则，不设新红线**。它把 [12-Factor Agents](https://github.com/humanlayer/12-factor-agents) 与 Aleph 现状对照，并对真实缺口立"采纳条款"（前缀 A，工程承诺级，**非红线**）。逐 factor 证据、锚点与模块改造建议清单见 [TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md)。

### 对照速览

| # | Factor | 一句话 | 已对应 | 状态 |
|---|--------|--------|--------|------|
| F1 | Natural Language → Tool Calls | 自然语言转结构化工具调用 | R7/R8/P8 | ✅ |
| F2 | Own your prompts | 自有提示词流水线 | R9 | ✅ |
| F3 | Own your context window | 显式拥有 context 取舍 | A1 ↔ R9 | ⚠️→A1 |
| F4 | Tools are structured outputs | 工具=结构化输出，执行解耦 | R8 | ✅ |
| F5 | Unify execution & business state | 执行/业务状态统一可重建 | A3 ↔ R10 | ⚠️→A3 |
| F6 | Launch / Pause / Resume | 统一启动/暂停/恢复契约 | A4 ↔ R5/R6 | ⚠️→A4 |
| F7 | Contact humans with tools | 人在环=工具调用 | R5 | ✅ |
| F8 | Own your control flow | 自有控制流（笨循环） | R10 | ✅ 典范 |
| F9 | Compact errors into context | 压缩错误进 context 自愈 | A2 ↔ R10 | ⚠️→A2 |
| F10 | Small, focused agents | 小而专的 agent | R3 | ✅ |
| F11 | Trigger from anywhere | 触发无处不在 | R5/R6 | ✅ |
| F12 | Stateless reducer | 趋向纯 reducer | A3 ↔ R10 | ⚠️→A3 |

> F13（pre-fetch context）已由 assembler 主动召回满足，仅在审计文档备注，不立条款。

### 采纳条款 (Adoption Clauses)

**A1 · 自有 Context Window (Own Your Context Window, F3)**
> Aleph 的 agent 按 **Prompt → Context → Harness → Loop** 四层构建；其中 **Context 层**（历史压缩 / 按模型窗口的压缩时机 / 内容类型路由缩减 / FTS5 检索回注 / 记忆三支柱）是一等工程关切，**context 的取舍由我们显式拥有**，不外包给框架默认消息格式。
> 关联：R9（智慧在 prompt）、P6（简洁）。锚点：`src/context/`、`src/thinker/`。
> ↳ 采纳·非红线

**A2 · 错误压缩 ≠ 错误恢复 (Compact Errors, Not Recover Strategies, F9)**
> **采纳**：把工具/Provider 错误**压缩并呈递给模型**，让模型在下一轮自愈（内容路由缩减、ToolError 事件、`think.rs` 有界 provider-failure drain）。
> **仍禁**（= R10 第 5 不）：在 harness 里做**确定性的"错误恢复策略选择 / 多级重试矩阵"**（hermes 式 fat-harness 重试有意不移植）。
> 边界一句话：**让模型看见并自愈错误 = 要；让 harness 替模型挑恢复策略 = 不要**。
> 关联：R7 / R10。锚点：`src/context/budget/cheap_passes/structured/`、`src/harness/agent/think.rs`。
> ↳ 采纳·非红线（本条是 R10 的读法说明，不改 R10 一字）

**A3 · 状态可重建，趋向纯 Reducer (Reconstructible State, Toward a Pure Reducer, F5+F12)**
> **方向性承诺**：执行状态应尽量可从**单一持久源**重建；每轮 Think 趋向"对持久 context 的纯 reduce"（`prompt.rs` 已逐轮重建裸消息）。新增有状态机件时，优先让其状态可观测、可重建。
> **硬约束**：**不得**为此让 `src/harness/` 越过 R10 的 12 文件 / ~4900 行预算，或把业务状态搬进笨循环。具体统一方案先走"加代码前必答 3 问"，列为 backlog 评估（见审计文档 B §P2-1）。
> 关联：R10、P4（依赖倒置）。锚点：`src/harness/trait_def.rs::TurnState`、`src/looping/`、`src/goal/`、`src/agents/swarm/tasks/store/`。
> ↳ 采纳·非红线（方向性原则，落地需独立评估）

**A4 · 统一 Launch / Pause / Resume 契约 (Unified Lifecycle Contract, F6)**
> 把已存在的取消（`cancellation.rs`）、续跑（`resume_coordinator.rs`）、改需求打断/注入（`steering.rs` 三态 Steer/Interrupt/Queue）、workflow resume 命名为**一组生命周期契约**：任何长跑单元（goal / loop / workflow / team task）都应可被一致地启动、暂停、恢复、取消。
> 关联：R5（AI 主动到达）、R6（一核多端）。统一 API 面（薄 facade，**不进 harness**）列为 backlog（见审计文档 B §P1-1）。
> ↳ 采纳·非红线
````

- [ ] **Step 3: 文档索引表登记新文档**

在 `## 📚 文档索引` 的表格里、紧随 `HARNESS_PHILOSOPHY.md` 行之后，新增一行（与既有行格式一致）：

```markdown
| **TWELVE_FACTOR_AUDIT.md** | [docs/reference/TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md) — 12-Factor Agents 逐 factor 合规审计 + 采纳条款 A1–A4 母本 + 模块改造 backlog（对照宪法《🧭 12-Factor 对照与采纳》节） |
```

- [ ] **Step 4: 验证红线零改动（spec §8 #2，关键验收）**

Run:

```bash
cd /Volumes/TBU4/Workspace/Aleph
git diff CLAUDE.md | grep '^-' | grep -v '^---'
```

Expected: **空输出**（无任何删除行）——证明 R1–R10 / P1–P8 原文一字未动，改动纯为新增。
若有删除行：说明误改了既有内容，回退并只做纯新增。

- [ ] **Step 5: 验证新节位置、条款、索引（spec §8 #1/#5）**

Run:

```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n '🧭 12-Factor 对照与采纳\|🛠 技术栈与禁用清单\|🧬 设计原则' CLAUDE.md
grep -c '↳ 采纳·非红线' CLAUDE.md
grep -c 'TWELVE_FACTOR_AUDIT.md' CLAUDE.md
```

Expected: 行号顺序为「🧬 设计原则 < 🧭 12-Factor < 🛠 技术栈」；`↳ 采纳·非红线` 计数 = 4（A1–A4）；`TWELVE_FACTOR_AUDIT.md` 出现 ≥ 2 次（新节指针 + 索引行）。

- [ ] **Step 6: 全量验收对照 spec §8**

人工核对 spec §8 七条全绿：① 新节位置/映射表/4 条款/指针 ✅；② R 零 diff（Step 4 已验）；③ AUDIT 含 §A/§B/§C（Task 1 已验）；④ ⚠️ 裁定锚点可佐证（Task 1 Step 1 已复核）；⑤ 索引登记（Step 5 已验）；⑥ 零 Rust 改动、零 cargo（`git status` 确认只动两份 md）；⑦ 风格对齐 FEATURE_LOCATOR。

```bash
cd /Volumes/TBU4/Workspace/Aleph
git status --short
```

Expected: 仅 `CLAUDE.md`（已跟踪，M）+（Task 1 已提交，故此处不应再出现 AUDIT）。确认无任何 `src/` 改动。

- [ ] **Step 7: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add CLAUDE.md
git commit -m "docs: add 12-Factor Conformance & Adoption section to constitution (A1-A4, redlines untouched)"
```

---

## 完成定义

- `docs/reference/TWELVE_FACTOR_AUDIT.md` 存在，§A（F1–F13 带锚点）+ §B（P0/P1/P2 backlog）+ §C（红线关系含 F9 读法说明）齐全。
- `CLAUDE.md` 新增《🧭 12-Factor 对照与采纳》节（精简映射表 + A1–A4 + AUDIT 指针）+ 文档索引一行；R1–R10 / P1–P8 原文零改动。
- 两次提交；零 `src/` 改动；零 `cargo`。
- 代码 backlog（P1-1 facade 评估、P2-1 状态源评估）留各自后续独立 spec/plan，不在本轮。
