# PLAN_HANDOFF.md — 只读规划相位与「计划 → 构建」交接 (Read-only Planning & the Plan→Build Handoff)

> 2026-08-12 落地（v1）。参考对照：codex `ModeKind::Plan` + TUI `plan_implementation.rs`、
> Claude Code plan permission mode + `ExitPlanMode`、hermes-agent `/plan` skill。

## 1. 是什么 (What)

一个**会话相位**：`PlanPhase { Building, Planning }`（默认 `Building`）。
`Planning` 期间**任何会改变东西的工具都被拒绝**——模型只能调研、用 `scratchpad`
写计划、用 `ask_user` 提问；唯一的出口是把计划交给人批准。**批准即放行，且在
同一个 run 内立即生效**：模型下一次工具调用起就可以按会话原有的 `ExecTier` 干活。

| | 规划相位 `planning` | 建造相位 `building`（默认） |
|---|---|---|
| 可调用 | 声明为 idempotent 的一切（`file_read` / `ctx_search` / `search` / `web_fetch` / `memory_*` / `session_*` 读 …）+ `scratchpad` + `ask_user` + `flag_user_correction` + `file_ops` 的只读动作（`list`/`search`/`stats`/`tree`/`find_duplicates`） | 全部（按 tier / `tool_permissions` / sandbox 裁决） |
| 被**隐藏**（不在工具表里） | 每一个「没有任何一种参数形态可放行」的工具：`bash` / `code_exec` / `file_write` / `file_edit` / `apply_patch` / `subagent` / `session_send` / `remember` / 全部未知工具与未声明 `readOnlyHint` 的 MCP 工具 | 无 |
| 被**拒绝但仍可见** | 参数相关的两个：`file_ops` 的破坏性动作、`scratchpad` 的 `request_build`（那是闸不是拒绝） | 无 |
| 出口 | `scratchpad { action: "request_build" }` → 审批卡 → 批准 | — |
| prompt 行 | `Plan phase: planning (read-only) …`（`OperatingEnvelopeLayer` @1758，Dynamic） | **零字节** |

## 2. 三个架构判断（都不显然，且都被试算过）

### 2.1 相位**不是**第四个 `SessionMode`

`session_mode.rs` 的模块头与 [MODE_SYSTEM.md](MODE_SYSTEM.md) §1 都逐字承诺
「模式不授予、不拒绝任何权限」，而 `SessionMode::prompt_line()` **每一轮都把这句话
发给模型**。加一个会拒绝的模式 = 让那句话在被读到的当场变假（判据 §0：一句关于
「什么被闸住」的话有三份拷贝，最贵的那份是发给模型的）。

### 2.2 相位**不是**第四个 `ExecTier`

两个理由，都是结构性的：

1. **tier 不是地板。** `effective_permission` 把 explicit `[policies.tool_permissions]`
   条目排在 tier **之前**——这对 Ask/Auto/Full 完全正确（operator 点名了某个工具就是
   他决定了），对这里恰好完全错误：一条 `"bash" = "allow"` 就能掏空一个「你做什么
   都改不了东西」的承诺。
2. **tier 值需要一个「恢复值」。** `resolve_exec_tier` 每轮都夹紧（非 operator 天花板、
   channel clamp）。若 Plan 是一个 tier 值，离开它就意味着**恢复到之前那个 tier**，
   即持久化一份在更早时刻、更早夹紧条件下拍下的权限值——一个陈旧且形如提权的状态。
   相位正交之后**没有任何东西需要恢复**：闩一松，本轮**早已解析并夹紧过**的 tier
   自然接管。规划期是 `Ask` 的人，建造期还是 `Ask`。

### 2.3 用 `enum` 不用 `bool`

`bool` 今天更便宜，明天想要第三种姿态（只读+可写记忆 / 已批准但计划冻结）就是错的形状。
给 enum 加变体是每个 `match` 的编译错误；给 bool 加语义是一次静默的含义变更。

## 3. 代码锚点 (Anchors)

**单一源**：`src/config/types/policies/plan_phase.rs`
— `PlanPhase` + `PLAN_PHASE_SESSION_KEY`(`"plan_phase"`) + `HANDOFF_ACTION`(`"request_build"`)
+ `from_id/id` + `admits()` / `hides()` / `prompt_line()` / `refusal()` + 三张表
（`PLANNING_TOOLS` / `READ_ONLY_FILE_OPS` / `name_verdict`）。

> **`admits` 与 `hides` 是同一张表的两个投影**（都走 `name_verdict`）。工具表构建时
> 没有参数、dispatch 时有——两处必须不能互相矛盾，`hiding_never_contradicts_admission`
> 钉住这一点。`READ_ONLY_FILE_OPS` 与 `exec_tier::DESTRUCTIVE_FILE_OPS` 对同一个参数
> 回答相反的问题，`read_and_destructive_file_ops_are_disjoint` 钉住两者不相交。

**强制点（唯一）**：`crate::config::types::policies::effective_permission`
— `phase` 是**必填参数**，不是带良性默认的 `Option`。两个调用点是
`ScopedToolService::permission_for`（循环的强制咽喉）和
`ExecutionEngine::slash_gate_reason`（网关 slash 快路径）；让编译器逼两者作答，
强于任何一句「记得也改快路径」的注释。相位检查排在 explicit 条目**之前**（§2.2）。

**参数级第二问**：`ScopedToolService::execute_inner` 里的 `plan_refusal(name, input)`，
跑两次——一次在 hook 之前判 `input`，一次在 `effective_input != input` 时判改写后的字节
（一个 `BeforeToolCall` 的 `update_input` 能把 `file_ops{list}` 改成 `delete`）。

**交接闸**：`gate_chain.rs::GateRule::PlanHandoff`，排在链条**最前**——它是唯一一条
没有任何配置参与的规则，也是唯一一条「批准之后还会做别的事」的规则。卡的决策集由
`exec::allowed_decisions::for_confirm_gate` 收到 `once_only()`：**对所有人，包括
operator**。「本会话都批准我的计划」和「永远批准我的计划」都是在同意一份还没写出来的
计划，而那正是这张卡存在的唯一目的。

**活闩**：`src/tools/scoped/plan_gate.rs::PlanGate`
— `AtomicBool`，**单调**（只解不上）。`release()` **先写持久记录再动闩**：写失败
就保持闩住并如实告诉模型（判据：先记录意图、再做不可逆动作）。释放会 bump
`ScopedToolService::cache_generation`，于是下一轮 `metadata_schema()` 重建，被隐藏的
工具重新出现——与 health 探针、deferred 晋升共用同一套失效机制。

**持久记录**：`turn_plan_phase.rs::PlanPhaseWriter`（`PlanPhaseSink` 的网关实现），
与读取器 `session_plan_phase()` 同文件同一个 `write_plan_phase` 函数——写它的和读它的
必须是同一个函数。

**每轮解析**：`gateway/execution_engine/turn_plan_phase.rs::resolve_turn_plan_phase`
（第四孪生）。precedence 只有两级 **requested > stored**，**没有全局档**：
`[policies]` 是装机级声明，而「这台装机永远在规划」不是任何人说得通的话，且会把每一次
cron tick、每一次 heartbeat 推进一个出口需要人的相位。无 channel clamp、无非 operator
天花板——这条轴没有「更松」的方向可去。

**进入面**：`chat.send{plan_phase}` / `agent.run{plan_phase}`（未知 id fail-loud）
· `sessions.patch{metadata.plan_phase}`（配对校验，`null` 合法＝离开规划）
· Panel `PlanPhasePill`。

**读回面**：`SessionInfo.plan_phase`（`sessions.list` 投影）→ Panel 侧栏 Effect →
`ChatState.session_plan_phase`。**这是必须的**：批准是**服务端**在没有客户端请求的
情况下写下的，客户端唯一学到它的途径就是读回。

**载运纪律**：`shared_ui_logic::state::session_dials_for_send`（三个 dial 的单一源）
— 相位**只在首条消息**载运。会话存在后 store 权威：一个把缓存的 `planning` 每条消息
重发一遍的客户端，会撤销它刚刚亲眼看着用户给出的批准，并在工作做了一半时把只读地板
重新合上。

**prompt**：`TurnEnvelope.plan_phase`（读的是**本轮的 gate 实时值**，不是 run 开始时
解析出的那个变量——两者差一个事件，即本 run 内已经发生的一次批准）→ `ResolvedContext`
→ `OperatingEnvelopeLayer` @1758（Dynamic）。**排在 `Approval mode:` 之前**：项目符号
列表里，顺序是唯一能表达「哪条规则赢」的手段，而「auto — 常规调用不打断」被读在
「什么都改不了」之前是主动误导。

## 4. 刻意不做 (Deliberately NOT)

1. **`bash` 的只读沙箱化。** codex 的 plan 档配 read-only sandbox；Aleph v1 直接拒。
   缝确实在（`SandboxCommand.capabilities` 是 per-call，`PolicyTier::ReadOnly` 已存在），
   但那会是**第二个执行点**，违反「执行档位唯一强制点是 `src/tools/scoped/`」，且
   依赖各平台 driver 真的强制 `writable_roots`——需要独立一轮审计。研究靠
   `file_read` / `file_ops(search|list|stats)` / `ctx_search` / `search` / `web_fetch`。
2. **模型自己进入规划相位。** 三个参考项目**没有一个**让模型进 plan mode（Claude Code
   是 Shift+Tab，codex 是 TUI picker）。零现实消费者不预建（R10 YAGNI）；而且保持
   `PlanGate` **单调**（只解不上）是一条比便利性更值钱的不变量。
3. **规划期允许 `subagent`。** 一个能派出会写东西的孩子的只读相位不是只读的。
   v1 靠「`subagent` 非 idempotent ⇒ 默认被拒」关掉它，零额外代码。
   **已验证的那一段**：`parent_view_for_children` 与主工具服务共用同一个
   `Arc<PlanGate>`（那是唯一正确的实参，不是为未来留的口），而
   `subagent_spawner` 只在它外面套**只会收窄**的两层（`McpScopedToolService` /
   `AllowlistToolService`）——所以**前台子代理**的相位传播是结构性成立的。
   **未验证的那两段**：detached 后台子代理（活过 run，要确认它握的确实是同一个
   服务而不是重建的）与 `task_manage` 团队派发（成员 run 是**新会话**，
   `resolve_turn_plan_phase` 读不到父会话的相位）。把 `subagent` 放进许可集之前
   先把这两条走一遍——第二条看起来就是个真缺口。
4. **全局 `[policies] plan_phase` 默认。** 见 §3 每轮解析。
5. **不重排 `ExecTier`**，不加第四档，不碰 `permissiveness()` / `most_restrictive`。
6. **不给 `PlanPhase` 加 channel clamp / 非 operator 天花板**——进入只会收窄。

## 5. 已知边界 (Known Limits)

- **unattended run 无法自我交接。** 无审批通道 ⇒ `check_confirmation_gate` fail-closed。
  一个处在规划相位的会话收到 cron / goal / loop 续跑时会保持只读并如实拒绝。这是
  正确方向（没人在读那份计划），但一个长期挂在规划相位的会话会让它的 loop 空转——
  出口是人回来批准，或用 pill / `sessions.patch` 清掉相位。
- **相位的读取是 fail-**open**。** `session_plan_phase()` 对无法解析的存量值回落
  `Building`。这与本仓大多数闸的方向相反，是有意的：相位不是安全边界，而是一个
  **本来就是要被离开的**工作位置；一个因为元数据里一个错字而永久只读、出口又在一张
  模型再也够不到的卡后面的会话，比一个恢复干活的会话更糟。
- **Panel 手机端没有这个 pill**（wide 端有）。`session_dials_for_send` 已经三个 dial
  全接，所以手机端补 UI 时不需要碰载运逻辑。

## 5.5 真机验证 (Real-machine QA · 2026-08-12)

`qa/plan_handoff/run.sh {handoff|deny|floor}` — 隔离 HOME、mock provider、真
`aleph-server`、真 JSON-RPC。三场景 **28 条断言全 PASS**。

每个场景都先跑一轮**普通 `building` 会话**当控制组。这不是礼节：没有它，
「规划期看不到 `bash`」与「这个 session mode 本来就没有 `bash`」在夹具内部
**逐字节相同**，整个场景会一边全绿一边什么都没测。实测两个数字是 **179 vs 51**。

| 场景 | 证据（不是结论） |
|---|---|
| `handoff` | 控制组 179 个工具（含 `file_write`/`bash`），规划期 51 个（都不含）。卡的 `allowed_decisions` 实为 `["allow-once","deny"]`——连 operator 连接也拿不到常驻档。`allow-once` 之后**同一个 run 的下一轮**工具表回到 179，`file_write` 返回 `Wrote 17 bytes to …`，`sessions.list` 读回 `plan_phase: "building"`。 |
| `deny` | 同一张卡答 `deny`。工具结果是 `The user did not approve running 'scratchpad' (Denied)`，下一轮仍是 51 个工具，其后的 `file_write` 仍被地板拒，会话仍是 `planning`。**被拒的计划不是被抬起的闩。** |
| `floor` | 配置写着 `[policies.tool_permissions.overrides] bash = "allow"`、`file_write = "allow"`，会话跑在 `exec_tier: "full"`——两个能赢过 tier 的东西，一个都没能把工具放回那 51 个里。同一轮里 `file_ops list` 放行、`file_ops delete` 拒绝：参数感知那一半，在真机上，在同一个工具上。 |

### 它抓到的缺陷：一个 `Deny`，两个作者

`handoff` 第一次跑挂在一条**关于文案**的断言上。一个到达了 dispatch 的
`file_write` 收到的是：

> `file_write` is denied by `default` in the merged tool permission policy
> (`[policies.tool_permissions]`, global → agent → channel, most restrictive wins)。

效果是对的，解释的**每一个字**是编的：没有任何策略条目做过这个决定（地板做的），
它点名的旋钮改了也不会改变结果，而唯一真能改变结果的动作（`request_build`）一个字
没提——同一相位的 prompt 还刚刚告诉过模型「不要绕开拒绝」。

成因：`effective_permission` 现在有两个作者能产出同一个 `Deny`，而下游 `deny_rule`
只有一个故事可讲（判据 §0「一个布尔够挡住调用，不够解释调用」）。

修法在 `gate_chain.rs`：新增 `GateRule::PlanFloor`（**只在 deny 链上**，不是卡），
`deny_rule` 先问一个反事实——**把地板拿掉，策略自己还拒不拒**
（`permission_ignoring_plan_floor`）。拒 ⇒ 策略是诚实答案（批准也没用）；不拒 ⇒
地板是作者。**胜出顺序与解释顺序回答的是两个问题，允许不同**：地板在 chokepoint
上赢在最前，在解释链上排在策略之后，因为每条理由都必须对「改什么能改变结果」说真话。
配套 `GateRule::deny_followup()`：「去改 `[policies.tool_permissions]`」这句尾巴对
策略拒绝是对的，对地板是有害的。守卫
`gate_chain::tests::a_floor_deny_names_the_floor_and_not_the_policy`（变异证过 RED）。

> 为什么只有真机抓得到：那道闸上原本有一句我自己写的注释，说这类调用「never reach
> here at all — 它们不在工具表里」。**不在工具表里不等于够不到**——名字可以从对话
> 上文记得、可以猜、可以被一个 `BeforeToolCall` 改写进来。一个真模型在第一分钟就
> 证明了这句注释是错的。

## 6. 排查话术 (Troubleshooting)

- 「模型说工具被拒绝了」→ 先看会话相位。规划期的拒绝**是功能在工作**；拒绝文案里
  逐字带着出口（`request_build`）。
- 「批准了但工具还是没出现」→ 工具表在**下一轮**重建（真机实测：同一个 run 的
  下一轮就回来了，见 §5.5）。若跨轮仍不出现，查 `PlanGate::release` 是不是返回了
  `Err`（持久化失败时闩**不动**，模型会收到一句点名 storage 问题的话）。
- 「拒绝文案说的是 `[policies.tool_permissions]`，可我没写过那条」→ 回归。地板拒
  绝必须报 `plan_floor` 而不是 `policy_deny`，见 §5.5；跑
  `gate_chain::tests::a_floor_deny_names_the_floor_and_not_the_policy`。
- 「刷新页面后 pill 又亮了 / 又灭了」→ pill 是**镜像**不是偏好。查
  `SessionInfo.plan_phase` 投影与侧栏那条 Effect；客户端不许记住自己发过什么。
- 「`/bash` 在规划期还能跑」→ 不应发生。slash 快路径同源
  （`slash_gate_reason` 里的 `plan_phase.admits(...)`）；若复发，查是不是新增了
  一条绕开 `effective_permission` 的派发路径。
- 「团队 run 进了规划相位」→ 不应发生：团队 run 的 metadata 不带 `plan_phase`，
  而成员 run 的会话是新建的（stored 为空）⇒ `Building`。
