# Harness 八场景端到端验证与连线修复 实施计划 / Harness E2E Verification & Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用真实服务 + 真实客户端证明 F1–F8 八个场景从用户动作连到可见结果；只修真机测出的断点，每个修复先有失败测试。

**Architecture:** 复用 `qa/` 现有骨架（`qa/lib/scratch_home.sh`、`qa/lib/build.sh`、`qa/busy_input/{patch_config.py,lib.py,mock_anthropic.py}`）。不新增 mock：`mock_anthropic.py` 已支持按轮次的工具调用清单（第 4 参数）与请求录制（第 7 参数 `request_log`）。新装置两个：`qa/thread_continuity`（F1/F2/F7/F8）、`qa/human_gate`（F4）；复活 `qa/plan_handoff`（F5/F6）；F3 用现有 `qa/busy_input steer`。

**Tech Stack:** Rust（tokio）、Python 3 驱动脚本（`websockets`，与 `qa/busy_input` 相同）、bash 编排、SQLite 会话库。

**Spec:** `docs/superpowers/specs/2026-11-26-harness-e2e-wiring-design.md`（已批准，`d83ac5e6b`）

## Global Constraints

- 回复与文档：中文，文档中英双语；代码注释英文。
- 不新增第二份状态：不加 `todo` / `exit_plan_mode` / 双队列工具（规格 §1）。
- 不引入新依赖（R3）；不新建 mock 文件，扩展 `qa/busy_input/mock_anthropic.py`。
- R10：`src/harness/` 不越过 `src/harness/tests/budget.rs` 的 `const CEILING: usize = 5535;`，也不加文件。若修复必须在 `src/harness/` 增长，停下问用户。
- 每次真机结论都来自**本次构建**的二进制：禁止在 `SKIP_BUILD=1` 下宣布红/绿（`qa/README.md` 第 205 行起的教训）。
- 先 `qa_build` 再 `qa_redirect_home`；所有状态落在 scratch `HOME/ALEPH_HOME`，不碰 `~/.aleph`。
- 配置必须带 chat provider（指向 mock），否则服务进入模拟模式，测的是启动模式而不是产品。
- F8 语义：多端都可写，gateway 串行化（规格 §8.1）。F7 只验证 fork 兄弟间与同角色跨轮次前缀（§8.2）。F6 不改行为（§8.3）。
- worktree `.worktrees/harness-e2e-wiring` 与主检出共享 `target/`；不要在主检出并行构建。
- DeepSeek 只用 Flash 系列。

## Review Focus

1. **未设置的旋钮在重启后仍是"跟随全局"**：`None` 不能被物化成具体值 → Task 4 断言 `memory_mode` 未设置时重启后仍为 `null`。
2. **两个终端发送相同文本**：不能被去重合并成一条 → Task 6 两端发送同一文本 `"same text"`，断言两条用户消息。
3. **过期的作答**：对已结束的提问再作答，必须得到 `stale: true`，不能误答到新的提问上 → Task 7 `stale` 断言。
4. **清单被清空后重开**：`chat.history` 必须返回空清单或 `null`，不能返回旧清单 → Task 5 `clear` 断言。
5. **计划被拒后模型继续尝试改动**：仍被 `plan_mode` 拒绝，层级停在 `plan` → Task 2 `deny` 阶段。

---

### Task 1: 基线——在本分支（= main `961f38dc1`）上重跑现有装置

**Files:**
- Create: `docs/superpowers/runs/2026-11-26-harness-e2e-wiring-receipt.md`
- Modify（仅当断言缺失时）: `qa/busy_input/drive_channel_busy.py`

**Interfaces:**
- Produces: 回执中的 `## 基线 / Baseline` 表（装置、阶段、结果、证据行）与 `## 缺陷台账 / Defect ledger` 空表（列：编号 D1…、场景、症状、根因、失败测试、修复提交）。后续任务往台账追加。

- [ ] **Step 1: 构建一次**

Run: `cd .worktrees/harness-e2e-wiring && cargo build --bin aleph-server --bin aleph`
Expected: 成功；记录 `git rev-parse --short HEAD` 到回执。

- [ ] **Step 2: 跑基线阶段（每条单独执行，`KEEP=1` 保留现场）**

```bash
for s in burst-drain interrupt queue steer; do KEEP=1 ./qa/busy_input/run.sh $s; echo "busy_input $s rc=$?"; done
for s in knobs holes unanswered; do KEEP=1 ./qa/resume_boundary/run.sh $s; echo "resume_boundary $s rc=$?"; done
KEEP=1 ./qa/session_order/run.sh; echo "session_order rc=$?"
for s in crash cap; do KEEP=1 ./qa/run_halt/run.sh $s; echo "run_halt $s rc=$?"; done
```
Expected: 每条打印 `rc=0`；非 0 记入基线表。

- [ ] **Step 3: 确认 F3 的 `steer` 阶段断言了"本步插入"**

读 `qa/busy_input/drive_channel_busy.py` 的 `steer` 分支（第 171 行附近）。必须同时断言：(a) 插话后只有**一个** run id；(b) 插话文本出现在同一 run 的下一次 provider 请求 `body.messages` 中（读 `request_log`）。缺哪条补哪条，补后重跑 `./qa/busy_input/run.sh steer`。

- [ ] **Step 4: 分诊每个红点**

装置过期或环境问题 → 修装置，在基线表注明"装置修复"；产品缺陷 → 台账新增 `Dn`，留给 Task 9。

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/runs/2026-11-26-harness-e2e-wiring-receipt.md qa/busy_input
git commit -m "qa: baseline the existing harness fixtures for the E2E wiring round"
```

---

### Task 2: 复活 `qa/plan_handoff`（F5 + F6 真机部分）

**Files:**
- Modify: `qa/plan_handoff/run.sh`（删除头部 `⚠️ STALE` 段，改写为当前设计说明）
- Modify: `qa/plan_handoff/drive_plan_handoff.py`
- Modify: `qa/plan_handoff/mock_plan.py`（只改工具调用内容）
- Keep: `qa/plan_handoff/add_overrides.py`（与设计无关，原样保留）

**Interfaces:**
- Consumes: RPC `chat.send{session_key, message, exec_tier}`、`clarification.pending{session_key}`、`clarification.resolve{session_key, reply, expected_request_id}`、`chat.history{session_key}` → `result.session.exec_tier`。
- Produces: `./qa/plan_handoff/run.sh {handoff|deny|floor}`，退出码 = 失败断言数。

按 `qa/plan_handoff/run.sh` 头注释的迁移清单改写：

- [ ] **Step 1: 改写驱动与 mock**
  - `chat.send{exec_tier:"plan"}` 取代 `plan_phase:"planning"`；对照组保留 `{exec_tier:"full"}`。
  - mock 依次调用：`file_write`（期望被拒）→ `scratchpad{action:"set_plan", …}` → `scratchpad{action:"request_approval"}` → `file_write`（期望成功）。
  - 人工闸门改为轮询 `clarification.pending`，删除 `exec.approval` 卡片相关断言（`once_only` / 无常驻授权）。
  - `assert_hidden()` 反转为 `assert_listed_but_refused()`：工具仍在模型的 `tools[]` 中（读 `request_log`），调用结果带 gate id `plan_mode`。

- [ ] **Step 2: 三个阶段的断言**
  - `handoff`：首个 `file_write` 结果含 `plan_mode`；`clarification.resolve{reply:"approved"}` 后**同一 run** 的第二个 `file_write` 成功且文件落盘；`chat.history.session.exec_tier != "plan"`；模型收到的批准文本以 `Plan approved` 开头。
  - `deny`：`reply:"rejected"` 后第二个 `file_write` 仍含 `plan_mode`，`chat.history.session.exec_tier == "plan"`，文件不存在。
  - `floor`：`add_overrides.py … bash=allow file_write=allow` 加 `exec_tier:"plan"`，`file_write` 仍含 `plan_mode`。
  - 新增：`chat.send` 携带已删除的 `plan_phase` 字段必须被拒（返回错误，不是静默忽略）。

- [ ] **Step 3: 运行**

Run: `for s in handoff deny floor; do KEEP=1 ./qa/plan_handoff/run.sh $s; echo "$s rc=$?"; done`
Expected: 三条 `rc=0`。红点按 Task 1 Step 4 分诊。

- [ ] **Step 4: Commit**

```bash
git add qa/plan_handoff
git commit -m "qa(plan_handoff): revive against ExecTier::Plan and request_approval"
```

---

### Task 3: F6 审批顺序回归单测

**Files:**
- Test: `src/tools/scoped/tests.rs`（与 `session_grant_skips_reprompt_within_session`、`a_session_grant_does_not_survive_into_an_unattended_run` 同处，复用 `confirm_registry()`、`turn_ctx()`、`FakeRequester`）

**Interfaces:**
- Consumes: `ScopedToolService::new(..).with_turn_context(..).with_confirmation(..).with_unattended(bool)`、`GateRule::id()`。

- [ ] **Step 1: 写测试 `approval_order_matches_the_documented_five_stages`**

一张表，每行是用户五段中的一段，断言 Aleph 的实际结果（规格 §4）：

| 行 | 配置 | 期望 |
|---|---|---|
| 逐工具拒绝 | 该工具 `deny`，`exec_tier = "full"` | 拒绝，id `policy_deny`，requester 调用 0 次 |
| YOLO（无越底线） | `exec_tier = "full"`，工具声明 `requires_confirmation` | 仍询问，id `tool_declared`，requester 调用 1 次 |
| 会话授权 | 先 `ApprovedForSession`，同参数再调 | 第二次不询问（调用计数不变） |
| 无人值守先于授权 | 同上授权后 `.with_unattended(true)` | 拒绝，requester 调用计数不变 |
| 询问 | 无授权、有人值守、`requires_confirmation` | 询问 1 次 |

`tool_declared` 的 id 字符串以 `GateRule::ToolDeclared.id()` 为准（`src/tools/scoped/gate_chain.rs:183-198`），测试里用常量而不是字面量。

- [ ] **Step 2: 运行**

Run: `cargo test --lib tools::scoped::tests::approval_order_matches_the_documented_five_stages`
Expected: PASS（这是锁定现状的回归测试；若 FAIL，说明现状与规格 §4 不符 → 台账记 `Dn`，不改断言迁就代码）。

- [ ] **Step 3: Commit**

```bash
git add src/tools/scoped/tests.rs
git commit -m "test(scoped): pin the approval chain order against the five-stage reading"
```

---

### Task 4: 新装置 `qa/thread_continuity` + `reopen` 阶段（F1）

**Files:**
- Create: `qa/thread_continuity/run.sh`（结构照抄 `qa/busy_input/run.sh`：source 两个 lib、`patch_config.py`、起 `mock_anthropic.py` 带 `request_log`、起服务、调驱动、清理；增加 `restart_server` 函数：`kill -TERM` 等进程退出 → 同 `ALEPH_HOME` 重启 → 等就绪）
- Create: `qa/thread_continuity/drive.py`（`python3 drive.py <stage> <ws-url> <request_log> <phase>`，复用 `qa/busy_input/lib.py` 的 `SessionLog` / `log`）

**Interfaces:**
- Produces: `run.sh <stage>`，`stage ∈ {reopen, checklist, two_clients, fork}`；需要重启的阶段分 `before` / `after` 两个 phase 调用 `drive.py`，中间由 `run.sh` 重启服务；phase 间状态写 `$QA_ROOT/state.json`。
- Produces: `drive.py` 中 `def check(ok: bool, claim: str, evidence: str = "") -> None`（照 `qa/plan_handoff/drive_plan_handoff.py:54`），退出码 = 失败数。

- [ ] **Step 1: `reopen before`**
  `sessions.patch` 设 `mode="code"`、`exec_tier="auto"`、`think_level="high"`、选一个模型 pin（`sessions.patch` 的 `model` 字段，见 `src/gateway/handlers/session/db_handlers/modify.rs:552`）；**不设** `memory_mode`。发一次 `chat.send`（mock 计划 `single-shot`），等 run 结束。保存 `chat.history.result.session` 与 `session.usage` 到 `state.json`。

- [ ] **Step 2: `reopen after`（重启后）**
  断言 `chat.history.result.session` 的 `mode / exec_tier / think_level / model_pin / model / input_tokens / output_tokens` 与 before 逐字段相等；`memory_mode` 仍为 `null`（Review Focus 1）；`session.usage` 相等。再发一次 `chat.send`，断言新 run 的 `RunStarted` 快照带 `exec_tier="auto"`、`think_level="high"`（读法参照 `qa/resume_boundary` 的 `knobs` 阶段在 `drive_r2.mjs` 中的实现），且该请求体反映 think 档位（与 before 那次请求体中 thinking 字段相同）。

- [ ] **Step 3: 运行**

Run: `KEEP=1 ./qa/thread_continuity/run.sh reopen`
Expected: `rc=0`；红点分诊。

- [ ] **Step 4: Commit**

```bash
git add qa/thread_continuity
git commit -m "qa(thread_continuity): reopen keeps knobs and usage across a gateway restart"
```

---

### Task 5: `checklist` 阶段（F2）

**Files:**
- Modify: `qa/thread_continuity/drive.py`、`qa/thread_continuity/run.sh`（新增工具清单 `checklist.tools.json`）
- Create: `qa/thread_continuity/checklist.tools.json`

**Interfaces:**
- Consumes: `scratchpad` 动作 `set_plan` / `start_item` / `complete_item` / `clear`（`src/builtin_tools/scratchpad.rs:44-56`，参数名以该文件的 schema 为准）；`chat.history.result.plan`（`null` = 无清单，对象 = `PlanSnapshot`，见 `interfaces/tui/src/tui/commands.rs:1470-1485`）。

- [ ] **Step 1: 工具清单**（mock 按轮次调用）
  `set_plan` 两项 → `start_item` 第 1 项 → `complete_item` 第 1 项 → `end`（模型试图收尾，第 2 项未勾）→ `complete_item` 第 2 项 → `end`。

- [ ] **Step 2: `checklist before` 断言**
  - 每个变更类 `scratchpad` 调用对应的 `tool_call_completed` 帧带清单快照。
  - 第一次 `end` 后 run **没有**结束：`request_log` 里紧随其后还有一次请求，且其 `messages` 含 `ScratchpadGoalVerifier` 的否决文本（取该 verifier 的实际文案，`src/verification/scratchpad_goal_verifier.rs`）。
  - run 结束时 `chat.history.result.plan.items` 两项均为 `completed`。

- [ ] **Step 3: `checklist after`（重启后）断言**
  `chat.history.result.plan` 与 before 相同。然后发一轮让 mock 调 `scratchpad{action:"clear"}`，再读 `chat.history.result.plan`：为 `null` 或 `items == []`，不是旧清单（Review Focus 4）。

- [ ] **Step 4: 运行并提交**

Run: `KEEP=1 ./qa/thread_continuity/run.sh checklist` → `rc=0`
```bash
git add qa/thread_continuity
git commit -m "qa(thread_continuity): checklist gates the finish and survives a restart"
```

---

### Task 6: `two_clients` 阶段（F8）

**Files:**
- Modify: `qa/thread_continuity/drive.py`

- [ ] **Step 1: 断言**
  开两个独立 WS 连接 A、B，同一 `session_key`。用 `asyncio.gather` 同时 `chat.send`，两端文本都是 `"same text"`（Review Focus 2）；mock 计划 `quick`。等会话空闲（无活动 run 且 busy queue 为空）后读 `chat.history`：
  - 用户消息 `"same text"` 恰好 2 条；
  - 每条用户消息之后、下一条用户消息之前（或转录结尾前）至少有一条助手回复；
  - 两端都没收到错误帧；两端各自的 `chat.send` 都返回成功；
  - 会话事件序号连续无空洞（读法参照 `qa/session_order/drive_session_order.py`）。

- [ ] **Step 2: 运行并提交**

Run: `KEEP=1 ./qa/thread_continuity/run.sh two_clients` → `rc=0`
```bash
git add qa/thread_continuity
git commit -m "qa(thread_continuity): two clients on one thread lose and merge nothing"
```

---

### Task 7: 新装置 `qa/human_gate`（F4）

**Files:**
- Create: `qa/human_gate/run.sh`（同 Task 4 骨架；`patch_config.py --channel-busy-mode queue` 以获得 webhook 通道）
- Create: `qa/human_gate/drive.py`、`qa/human_gate/ask.tools.json`

**Interfaces:**
- Consumes: `ask_user` 输入 schema（`src/builtin_tools/ask_user.rs`）；`clarification.pending` / `clarification.resolve`（`shared/protocol/src/clarification.rs:52-70`）；`qa/busy_input/lib.py` 的 `webhook_post` / `channel_message`；`cron.create` / `cron.run`。
- Produces: `./qa/human_gate/run.sh {ask|text|unattended|stale}`。

- [ ] **Step 1: 四个阶段**
  - `ask`：mock 调 `ask_user`（两个问题）。驱动在 `clarification.pending` 看到请求后用 `clarification.resolve{answers:["A1","A2"]}` 作答；断言工具结果按序含 `A1`、`A2`，run 正常结束。
  - `text`：同一问题经 webhook 通道发起；驱动用两条 `channel_message` 逐条作答（游标路径）；断言工具结果同样按序含两条答案。
  - `unattended`：`cron.create` 一个任务让 mock 调 `ask_user`，`cron.run` 立即触发；断言工具结果含 `no human is reachable on this run`（`src/clarification/ask.rs:133`），`clarification.pending` 始终为空，run 在 60 秒内结束。
  - `stale`（Review Focus 3）：`ask` 阶段结束后，以旧的 `expected_request_id` 再调 `clarification.resolve`，断言返回 `stale: true`、`resolved: false`。

- [ ] **Step 2: 运行并提交**

Run: `for s in ask text unattended stale; do KEEP=1 ./qa/human_gate/run.sh $s; echo "$s rc=$?"; done` → 全 `rc=0`
```bash
git add qa/human_gate
git commit -m "qa(human_gate): ask_user answers, plain-text cursor, unattended refusal, stale reply"
```

---

### Task 8: `fork` 阶段（F7）

**Files:**
- Modify: `qa/busy_input/mock_anthropic.py`（工具清单的一项可以是**数组**，表示同一轮返回多个 `tool_use` 块；单对象行为不变）
- Modify: `qa/thread_continuity/drive.py`、`qa/thread_continuity/run.sh`
- Create: `qa/thread_continuity/fork.tools.json`

**Interfaces:**
- Consumes: `subagent{task, context: "isolated"|"summary"|"fork"}`（`src/agents/subagent_tool/loop_tool.rs:124-126`）；请求归属按**内容**而不是位置，做法照 `qa/plugins/drive_subagent.py` 头注释。

- [ ] **Step 1: mock 扩展先过回归**
  改完后重跑 `./qa/busy_input/run.sh burst-drain`，仍 `rc=0`（单对象路径字节不变）。

- [ ] **Step 2: 断言**
  父会话先完成两轮带标记文本 `QA_PARENT_MARKER` 的对话，然后：
  - 第 3 轮同一轮返回两个 `subagent{context:"fork", task:"QA_FORK_TASK_{1,2}"}`：两个子请求的 `system` 逐字节相同，且去掉最后一条任务消息后的 `messages` 前缀逐字节相同；前缀中含 `QA_PARENT_MARKER`；切点落在完整轮次上（前缀最后一条是助手消息，不是悬空的 `tool_use`）。
  - 第 5 轮再 fork 同一角色：第 3 轮子请求的 `messages` 前缀（去掉任务消息）是第 5 轮子请求 `messages` 的前缀。
  - 第 6 轮 `subagent{context:"isolated", task:"QA_ISO_TASK"}`：子请求任何消息都不含 `QA_PARENT_MARKER`。

- [ ] **Step 3: 运行并提交**

Run: `KEEP=1 ./qa/thread_continuity/run.sh fork` → `rc=0`
```bash
git add qa/busy_input/mock_anthropic.py qa/thread_continuity
git commit -m "qa(thread_continuity): fork siblings share a byte-identical prefix; isolated sees no parent"
```

---

### Task 9: 修复台账中的缺陷（每个 `Dn` 一轮）

本任务的条目数由 Task 1–8 决定，计划无法预先列出。每个 `Dn` 严格按以下步骤：

- [ ] **Step 1:** 在能复现的**最低层**写失败测试（单测优先，其次 `tests/` 集成测试），测试名描述用户可见的行为。
- [ ] **Step 2:** `cargo test <测试路径>`，确认按预期原因 FAIL。
- [ ] **Step 3:** 最小修复。发现同一事实的第二份实现时，删除多余的一份，并在提交说明中列出删除内容。
- [ ] **Step 4:** 单测 PASS；`cargo test --lib harness::tests::budget` PASS（R10）；重跑暴露它的真机阶段转绿。
- [ ] **Step 5:** 台账补全根因、测试名、提交号；提交 `fix(<area>): <用户可见行为>`。

如果修复要越过 R10 棘轮、改变 F8 多写者语义、或涉及子代理 prompt 组装（规格 §5 另开一轮）→ 停下问用户。

---

### Task 10: 文档与终验

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§3 Harness：F1–F8 各一行"真机证明"指向装置阶段与回执；校对本轮触及的锚点行号）
- Modify: `qa/README.md`（登记 `thread_continuity`、`human_gate` 的运行行与"每个阶段证明什么"；`plan_handoff` 去掉过期说明）
- Modify: `docs/superpowers/runs/2026-11-26-harness-e2e-wiring-receipt.md`（最终结果表 + 台账 + 没做的事）
- Modify（视修复而定）: 修复所涉子系统在 `docs/reference/*` 的条目

- [ ] **Step 1:** 改文档。只写代码里能指到的事实；证据层与代码冲突时以代码为准。
- [ ] **Step 2: 终验**

```bash
cargo clippy --all-targets -- -D warnings
cargo test --lib tools::scoped
cargo test --lib harness::tests::budget
for s in handoff deny floor; do ./qa/plan_handoff/run.sh $s; done
for s in reopen checklist two_clients fork; do ./qa/thread_continuity/run.sh $s; done
for s in ask text unattended stale; do ./qa/human_gate/run.sh $s; done
./qa/busy_input/run.sh steer
```
Expected: clippy 无警告；测试全过；每个阶段 `rc=0`。不能转绿的项写进回执的"未完成"，不隐藏。

- [ ] **Step 3: Commit**

```bash
git add docs qa/README.md
git commit -m "docs: record the harness E2E proofs in FEATURE_LOCATOR, qa/README and the receipt"
```

---

## 与规格的两处调整 / Deviations from the spec

1. **F3 不新建 `steer` 阶段**：`qa/busy_input/run.sh steer` 已存在（`run.sh:69`）。Task 1 Step 3 只检查并补齐其断言。
2. **F7 落在 `qa/thread_continuity fork`**：它与 F1/F2/F8 共用同一套启动和请求录制，不再单开装置。
