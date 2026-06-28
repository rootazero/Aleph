# 单聊 Todo 面板 · 多任务覆盖 + 沉入对话流

> Date: 2026-06-28 · Status: Design approved, ready for plan
> Scope: `interfaces/webchat` only (panel-side, **zero core change**)

## 1. 背景与问题

单聊 Chat 已有一个钉在输入框上方的 sticky `TodoPanel`(`platform/wide/views/chat/todo_panel.rs`),由 LLM 经 `scratchpad` 工具(`set_plan` / `start_item` / `complete_item` / `clear`)驱动,经 `events.rs` 投影成 `ChatState.plan`。

现状两个缺口:

1. **完成后永久挂着**:`plan.rs` 的 `scratchpad_plan_update` 只有 `action=="clear"` 才返回 `Hide`。任务全部打勾后若模型不调 `clear`,完成的计划就一直钉在输入框上方。
2. **同一 session 第二次任务分解时,旧计划无声消失**:模型再次 `set_plan` → 面板直接覆盖,上一份(通常已完成)那份既不留痕、也无法回看。

## 2. 参考实现结论(codex / kimi-cli)

- **codex `update_plan`**(`codex-rs/protocol/src/plan_tool.rs`):模型每次传**整份** `plan: Vec<PlanItemArg>`,全量替换,单一活动计划,无累加;完成的计划作为 history cell 落进**对话流**。
- **kimi `SetTodoList`**(`src/kimi_cli/tools/todo/__init__.py`):模型每次传**整份** `todos`(或不传=只读),全量替换,存 `session.state.todos`,单一列表/会话;完成的作为 `TodoDisplayBlock` 渲染进**消息流**。

**洞察**:两者都用「单一、模型全量重写、归属模型」的列表,根本没有"覆盖 vs 保留"的纠结——因为它们的 todo **渲染进消息流**,完成的随滚动自然成为历史。Aleph 把它做成了**固定浮窗**,所以"旧的怎么办"才成了问题:它被钉住、不在 scroll 里。

→ 本设计采纳同一范式:**固定槽只持单一活动计划(覆盖);完成/被替换的计划沉入对话流(历史)。**

## 3. 已决策项(与用户逐项确认)

| 决策 | 选择 |
|------|------|
| 同 session 第二次分解时旧计划的处理 | **覆盖 + 旧的沉入对话流**(对齐 codex/kimi,固定区不膨胀,历史可回看) |
| 完成的计划何时离开固定槽 | **完成后保留「✓ 已完成」细条,下一动作(模型 clear / 新 set_plan / 用户发下一条消息)才沉入** |
| 沉入对话流的形态 | **紧凑胶囊・点击展开**(与 codex/kimi history cell 同密度) |
| 是否持久化(replay 后仍在) | **持久化:从 scratchpad 事件流确定性重建(live 与 replay 共用同一投影)** |
| 被覆盖的**未完成**计划 | **也留痕**,沉成灰色「◗ 未完成 done/total」胶囊(回看"做到一半被切换"的轨迹) |

## 4. 行为规范(状态机)

固定面板永远只持有**单一活动计划**(`ChatState.plan`)。退场统一为一个动作 `archive_active_plan()`:把当前 `plan` 压成一条 `plan_archive` 胶囊消息推入对话流尾部,再把 `plan` 置 `None`。

三条下沉触发:

| 触发 | 归档门控条件 | 后续动作 |
|------|-------------|----------|
| **新 `set_plan`(覆盖)** | 旧计划 `has_activity()`(≥1 项 done/in-progress,或已完成) | 归档旧的 → 显示新计划 |
| **`clear`(模型显式收尾)** | 旧计划 `has_activity()` | 归档旧的 → 面板隐藏 |
| **下一轮开始 `start_assistant_message()`** | 旧计划 **`complete == true`** | 归档已完成细条 → 面板隐藏 |

**去噪 / 防误沉规则(关键)**:

- **微调静默替换**:一份刚 `set_plan`、尚无任何进展(0 done、0 in-progress、未完成)的计划被立刻覆盖 → 视为计划微调,`has_activity()==false` → **静默替换、不留胶囊**,避免对话流刷屏。
- **下一轮只沉已完成**:下一轮触发**只对 `complete==true`** 生效,不对进行中的计划。一份还没做完的计划,用户只是补充一句话(未触发新 `set_plan`),它**继续留在固定槽**——因为模型可能要接着做,系统不替模型判断"任务是否该结束"(R7)。
- **完成态细条**:全部打勾后,面板收成「✓ 已完成 N/N」细条**仍留原位**(让用户看到完成),直到上述任一触发才沉。

### 4.1 典型时序(逐项验证)

- **完成 → 新任务**:task1 全勾 → 细条留位;用户发"再做 task2" → 新 run `start_assistant_message`(plan 已完成 → 归档 task1 胶囊 + plan 置 None);模型 `set_plan(task2)`(plan 现为 None → 不归档,直接显示)。✅ 单次归档。
- **完成 → 用户闲聊不开新任务**:task1 全勾 → 细条留位;用户发普通消息 → `start_assistant_message`(归档 task1 胶囊 + 隐藏);模型仅作答不 `set_plan`。✅ 细条不再永久挂着。
- **进行中被覆盖(中途切换)**:task1 做到 2/5(有进展,未完成);用户"改做 X" → 模型 `set_plan(X)` → 该分支见 plan `has_activity()==true` → 归档"◗ 未完成 2/5"胶囊 → 显示 X。✅
- **进行中、未开新任务**:task1 做到 2/5;用户补充一句信息,模型继续 task1。`start_assistant_message` 见 plan 未 `complete` → 不归档;无新 `set_plan` → 不覆盖。✅ 固定槽继续显示 task1。
- **同一 run 内完成并立刻开新计划**:模型 `complete_item` 收尾 task1(complete=true)→ 紧接 `set_plan(task2)`(无 run 边界)→ `set_plan` 分支见 plan `has_activity()` → 归档 task1 → 显示 task2。✅(这正是 `set_plan` 分支也必须带归档的原因。)

> 无双重归档:每次 `archive_active_plan()` 都把 `plan` 置 None,三个调用点彼此互斥地命中"非空 plan"。

## 5. 为什么 live 与 reload 自动一致(持久化机制)

`archive_active_plan()` 只挂在**两类被 live 与 replay 共用**的调用点:

- `ChatState::start_assistant_message()`:live 的 `run_accepted` 分支与 replay 的 `replay_run()` **都调用它** → 顶部加"若 plan 已完成则先归档"对两条路径同时生效。
- `events.rs::apply_trace_event()` 中 scratchpad 的 `set_plan` / `clear` 分支:live WS 流与 `trace.by_runs` replay **都走这个函数**。

→ 重开会话 / 切回 tab(replay)时,把同一串持久化的 scratchpad 事件按序重放,确定性重建出同样的胶囊。**无需新协议、无需 core 改动、无需 SessionSnapshot 新字段**(胶囊本就是 `messages` 里的普通消息,切 tab 随 `messages` 一并保留)。

胶囊在对话流里的**落点语义**:插入在"完成任务的最后一条消息之后、下一轮之前"的会话边界。live 与 replay 走同一调用点 → 落点一致。(具体插入索引为实现细节,留待 plan 阶段确定,两条路径必须产出相同序列。)

## 6. 改动清单(全部 panel 侧,`interfaces/webchat/src/platform/wide/views/chat/`)

1. **`plan.rs`**
   - 给 `PlanView` / `PlanItemView` / `PlanItemStatusView` 增加 `Serialize, Deserialize` derive(胶囊要随 `ChatMessage` 进 `SessionSnapshot`)。
   - 新增 `PlanView::has_activity(&self) -> bool`:`done_count() > 0 || items.iter().any(|i| i.status == InProgress) || complete`。
2. **`state.rs`**
   - `ChatMessage` 增加 `#[serde(default, skip_serializing_if = "Option::is_none")] pub plan_archive: Option<PlanView>`(旧快照兼容:无字段 → `None`)。
   - 所有 `ChatMessage { .. }` 字面量补 `plan_archive: None`(`push_user_message` / `start_assistant_message` / `begin_step` 内联构造处)。
   - 新增 `ChatState::archive_active_plan(&self, gate: ArchiveGate)`:按门控读取 `plan`,满足则推 `plan_archive` 胶囊消息(id 经 `next_msg_id` 取唯一,如 `plan-archive-{seq}`)+ `plan` 置 `None`;否则按调用点决定静默替换/隐藏。
   - `start_assistant_message()` 顶部:`if plan.is_some_and(|p| p.complete) { archive_active_plan(Completed) }`。
3. **`events.rs`**
   - `apply_trace_event` 的 scratchpad 投影:在 `apply_plan_update` 之前,`set_plan` 与 `clear` 两种 action 先调 `archive_active_plan(Activity)`(覆盖/收尾)。注意只在 action 确为 `set_plan`/`clear` 时触发——`start_item`/`complete_item` 是对**同一计划**的就地更新,不得归档。
4. **`todo_panel.rs`**
   - 完成态(`complete==true`)收成「✓ 已完成 N/N」细条 header(现已接近,微调 header 文案与折叠)。
5. **新增胶囊渲染**(消息列表组件,`MessageList`/`MessageBubble` 分支或新 `plan_archive_cell.rs`)
   - `msg.plan_archive` 为 `Some` → 渲染紧凑胶囊:完成 = 「✓ 任务完成 · N/N 「objective」▾」(success 色);未完成 = 「◗ 未完成 · done/total 「objective」▾」(muted 色)。点击展开整份 ✓/未完成清单。
   - 复用 `todo_panel.rs` 既有 OKLCH 设计 token / 清单行样式,避免新 CSS 体系。

> `SessionSnapshot` **无新字段**;固定槽 `plan` 维持现状(ephemeral,`restore_from` 重置为 None),不在本特性范围。

## 7. 测试(纯投影,沿用现有 `Owner` 模式 + `projection_tests` 风格)

- **覆盖-有进展**:set_plan(A) → start_item → set_plan(B) ⇒ 出现 A 的胶囊(◗ 未完成 0/1 起算需有 in-progress),plan=B。
- **覆盖-微调静默**:set_plan(A, 0 进展) → 立刻 set_plan(B) ⇒ **无**胶囊,plan=B。
- **clear-有进展**:set_plan(A) → complete_item ×N(complete) → clear ⇒ 出现 A 的「✓ 任务完成」胶囊,plan=None。
- **完成→细条→下一轮沉**:set_plan(A) → 全 complete(plan 仍在、complete=true,无胶囊)→ `start_assistant_message` ⇒ 出现胶囊 + plan=None。
- **进行中不被过早沉**:set_plan(A) → start_item(进行中)→ `start_assistant_message`(未 complete)⇒ **无**胶囊,plan 仍为 A。
- **replay 确定性**:对同一 scratchpad 事件序列分别走 live 投影与 `replay_run` ⇒ 产出相同的 `plan_archive` 胶囊集合与顺序。
- **serde 兼容**:无 `plan_archive` 字段的旧 `ChatMessage` JSON 反序列化为 `None`;带胶囊的 `ChatMessage` round-trip 保真。

## 8. 红线对照

- **R4(Interface 纯 I/O)**:面板仅渲染模型经 scratchpad 产出的快照信号;归档是对已持久化事件的确定性投影,非业务逻辑。✅
- **R7 / R10(LLM 主权 / 笨循环)**:不新增 harness 逻辑、不做"任务是否完成/是否该结束"的确定性判断——`complete`(全勾)与 `set_plan`/`clear` 都是模型显式信号;"下一轮只沉已完成、进行中不沉"正是把判断留给模型。✅
- **核心轻量化 / 零 core 改动**:全部落在 `interfaces/webchat`,无新协议、无新依赖。✅

## 9. 显式非目标(YAGNI)

- 不为固定槽 `plan` 加 `SessionSnapshot` 持久化(活动计划切 tab 仍 ephemeral,维持现状)。
- 不做多份计划在固定槽内堆叠(已否决:挤压输入框,违背单一活动列表范式)。
- 不在 core 侧累加/存档历史计划(scratchpad 文件仍单份、`set_plan` 仍全量替换)。
- 不引入手机端(`platform/phone`)的对应改动(本特性限单聊 wide 视图)。
