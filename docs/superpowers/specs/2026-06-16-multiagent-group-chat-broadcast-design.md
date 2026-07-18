# Multiagent 群聊广播范式设计（telegram 式平等群聊）

- **日期**: 2026-06-16
- **状态**: 设计已批准（brainstorming）→ 待生成实现计划
- **范围**: 重写 `teams.chat.send` 的编排范式，从 hub-and-spoke 改为平等广播
- **关联红线**: R6（一核多端）、R7（LLM 主权）、R9（智慧在 prompt）、R10（薄 Harness）

---

## 1. 背景与问题

### 1.1 现状：subagent 式 hub-and-spoke

当前 `teams.chat.send`（`src/gateway/handlers/teams.rs` 的 `handle_chat_send`，约 line 2852）是 **leader 单点编排**：

- 只 resolve + spawn **leader 一个** agent（`leader_prompt::build` → 单次 `execution_adapter.execute`）。
- 成员靠 leader 的 `team_delegate` / `task_create` 间接、后台触发，产出**回流给 leader 汇总**。
- 成员**从不直接在群里发言** → 用户只看到 leader（main）说话。

### 1.2 两个独立问题

**A) 真 bug（已修，保留）**：`handle_chat_send` 给 leader 的 prompt 只传 `team.name`、不传 `team_id`，而 `task_create` / `team_delegate` / `team_status` 都把 `team_id` 作为必填首参。leader 只能拿团队名字当 id → 日志铁证 `team_status → Team 'main的群聊' not found` → 无法派活 → 成员零响应 → leader 空转 generic `subagent`（8 次，每次 10–22s）→ 被 verifier halt → 首句延迟 4–5 分钟。
- **修复**：`src/teams/leader_prompt.rs::build` 增加 `team_id` 参数，prompt 显式声明"调用任何团队工具时 team_id 必须填 `<id>`"；`handle_chat_send` 传入 `&params.team_id`。已加回归测试 `build_surfaces_team_id_so_leader_can_address_its_team`。

**B) 范式 gap（本设计的主题）**：即便修好 A，设计本身仍是 hub-and-spoke，用户仍只与 leader 对话。用户要的是 **telegram 式 multiagent 群聊**——用户 + leader + 成员**全员在一个群多方互通**，员工↔员工、员工↔用户直连；**leader 不是代码强制管控，而是 LLM 的身份认定**。

### 1.3 multiagent vs subagent（用户洞察）

- **subagent**（当前实现）：主 agent 单线管控子 agent，启动/通信/销毁都是主↔子单线；leader 是代码强制的唯一发言人。
- **multiagent**（目标）：用户 + leader + 员工全员在一个群里多方完全互通；leader 有"领导/汇总/交付"职责但**像现实里的领导**——其作用来自 LLM 身份认定，不是代码强制管控。

---

## 2. 目标

把 `teams.chat.send` 从 hub-and-spoke 改为**平等广播群聊**：

- 代码层**所有 agent 完全平等**，共享一份 transcript（唯一事实源，持久化）。
- 用户 / agent 消息经 @mention 解析 → 被点名 agent **各自独立 run**。
- 没 @ 任何人时 → **leader 兜底**。
- agent 回复 append 进共享 transcript + 广播到 `team.<id>.*`（attributed bubbles）。
- agent 回复里可 @ 别人接话，**归一到同一 fan-out 入口**（用户触发与 agent 接话同路径）。
- 防风暴：@mention 门控 + `chain_depth` 深度闸 + 单轮宽度闸。
- leader 仅靠 prompt 身份；现有编排工具（team_delegate / task_create / 工作区 tab）保留。

---

## 3. 设计决策（brainstorming 结论）

| # | 决策 | 选择 | 理由 |
|---|---|---|---|
| Q1 | 谁回复 | **@提及驱动 + leader 兜底**（被@才回；@all/@everyone 全员；没@→leader） | 成本可控、降噪；leader 兜底契合"像现实领导" |
| Q2 | 互动深度 | **允许接话 + 保守上限兜底**（chain_depth + 宽度闸） | 满足"完全互通"又防 token 爆炸/死循环 |
| Q3 | 群记忆 | **落库 + 共享 transcript 注入 + token 预算截断**（复用 messages store） | openteams 黄金模式 + 复用现有设施，修掉"team 态 ephemeral"局限 |

---

## 4. 参考项目结论（4 个并行研究的综合）

- **openteams（Rust，黄金参考）**：共享 transcript 是唯一事实源 + @mention 触发 + `chain_depth`（消息 `meta` 计数器，默认 8）防风暴 + agent 用 `Send` 协议互相 @ 接话、**归一到同一 fan-out 入口** + 不做 per-agent 角色重写（所有 agent 看同一份带 `[发言人]` 前缀的逐字 transcript）。三件套与 Aleph 现有设施几乎一一对应，移植成本最低。
- **openclaw**：agent "by design 互不可见"——正是要避免的反面；但"广播时捕获一次 transcript 快照统一喂所有 agent，保证视角一致"可借。
- **clawteam**：leader 中心 hub-and-spoke——要抛弃；其 `RuntimeRouter` 聚合状态机 Aleph `aggregator.rs` 已对标。
- **hermes**：mention 门控 + `[sender name]` 前缀 + 共享 session——轻量可借；但**完全无轮次/宽度上限**是隐患。
- **共识**：平等互通必须补"单轮宽度上限"（防一条 `@all` 在大群一次炸开 N 个并发 run）——openteams 缺，Aleph 不能省。

---

## 5. 架构

### 5.1 复用（不重写）

| 现有设施 | 用途 |
|---|---|
| `src/teams/messages/store.rs` | 群 transcript 落 SQLite（Q3 持久化）|
| `src/teams/messages/mentions.rs` | @mention 解析（`@all`/`@everyone`/`MENTION_ALL` 哨兵/防 email/代码块转义）|
| `src/teams/messages/types.rs` | `TeamMessage` 信封（sender/recipient 寻址）|
| `src/gateway/event_emitter/team_fanout.rs` | `TeamFanoutEmitter`：agent run 事件 → `team.<id>.*` |
| Panel attributed bubbles（commit a18443c60）| 按 agent 归属渲染群发言 |
| `src/builtin_tools/team/*`（delegate/task_create/status…）| leader 编排工具，保留可用 |

### 5.2 新增（放 `src/teams/` 下，**不进 `src/harness/`**——是脚手架不参与推理，守 R10）

**广播编排器**（新模块，如 `src/teams/broadcast/mod.rs` 或 `src/teams/chat_broadcast.rs`）：

- `fan_out(team, trigger_message, chain_depth)`：
  1. 解析 trigger_message 的 @mention → 确定目标 agent 集合（被@的；`@all`→全员；空→leader 兜底）。
  2. **宽度门控**：目标数 > M 时取前 M，丢弃的留痕（log + 可选系统提示）。
  3. 为每个目标 agent spawn 一个独立 run（注入群上下文 prompt + `chain_depth` 进 `RunRequest.metadata`），emitter = `TeamFanoutEmitter(team_id, author=该 agent)`。
- **chain_depth 守卫**：`fan_out` 入口 `if chain_depth >= MAX return`（不再触发任何 agent）。
- **回流**：agent run 完成 → 存回复进 messages store（sender=agent, chain_depth+1）→ 已由 emitter 广播 → 解析回复里的 @mention → 若 `chain_depth+1 < MAX` 则递归 `fan_out`（**归一入口**）。
- **护栏**：忽略 agent 对**自己**的 @；忽略 agent 对保留 handle `user` 的 @（防自环，openteams 的 `RESERVED_USER_HANDLE`）。

**群成员 prompt 构造器**（改造 `src/teams/leader_prompt.rs` → 扩展为 `member_prompt`，或新增 `src/teams/member_prompt.rs`）：

- 输入：`team_id`、当前 `agent_id` / `role`、名册（其他成员）、共享 transcript（`[发言人]` 前缀，token 预算截断）、触发原因（谁 @ 了你 / 新消息）、`is_leader`。
- 输出：注入身份 + 名册 + transcript + **接话协议**（"你可以 @ 名册里的成员接话；要不要回、回什么由你 LLM 判断；调团队工具时 team_id 填 `<id>`"）。
- `is_leader` 时**追加**身份段（见 §8）。

### 5.3 改造 `handle_chat_send`

从"build leader prompt + execute 单 run"改为：

1. 存 user 消息进 messages store（sender=user, chain_depth=0）。
2. 调广播编排器 `fan_out(team, user_message, chain_depth=0)`。
3. 立即返回 `run_id`（run 异步进行，事件经 fanout 广播到 Panel）。

---

## 6. 数据流（一条消息的生命周期）

```
用户: "@alice @bob 讨论下方案"
  → handle_chat_send
  → 存 user 消息进 messages store (chain_depth=0)
  → fan_out:
       解析 @mention = [alice, bob]   (没@人→leader 兜底; @all→全员)
       宽度门控 (2 ≤ M=5 ✓)
       为 alice、bob 各 spawn 独立 run:
         输入 = [身份] + [名册] + [共享transcript "[发言人]"前缀] + "用户@了你"
         metadata.chain_depth = 1
         emitter = TeamFanoutEmitter(team_id, author=该agent)
  → alice run 完成 → "我觉得用X,@bob 你看?"
       存 alice 消息 (chain_depth=1) → 广播 team.<id>.* → Panel alice 气泡
       解析回复@ = [bob], chain_depth(1) < MAX(6) ✓
       fan_out bob (注入含 alice 刚说的更新版 transcript, chain_depth=2)
  → bob → "X有坑,@alice 试试Y?" (chain_depth=2) → fan_out alice (chain_depth=3)
  → … 直到 chain_depth=6 或没人再@ → 自然停
```

**关键**：用户触发与 agent 接话走**完全同一个 `fan_out` 入口**（openteams 归一设计），区别只在 `sender` 与 `chain_depth`。

---

## 7. 防风暴三道闸

全是**确定性脚手架，不参与推理**（守 R10）。

| 闸 | 机制 | 默认值 |
|---|---|---|
| 谁回（门控）| `mentions.rs` 解析：被@的才唤醒；`@all`/`@everyone`→全员；空→leader 兜底 | — |
| 接话深度 | 每条消息 `meta.chain_depth`；`fan_out` 前 `if depth >= MAX return`；接话写 `depth+1` | MAX = **6**（可配）|
| 单轮宽度 | 一条消息最多同时唤醒 M 个 agent（`@all` 在大群也不炸）| M = **5**（可配）|

**额外护栏**（来自 openteams，防自环）：agent 不能 @ 自己；agent 不能 @ 回 `user`（保留 handle）。

**到顶行为**：停止级联 + 在群里 append 一条**系统提示**（"讨论已达深度上限，等你接话"），而非静默消失。

---

## 8. leader 身份融合（回应"leader 是 LLM 身份非代码强制"）

**代码层面 leader 与成员零差别**——一样被 fan_out、一样注入群上下文、一样能 @ 别人、一样能调团队工具。leader 的"领导力"**只来自 prompt 注入的身份段**：

- 成员 prompt：`你是群里的 {agent_id}（{role}）。{名册}。{共享transcript}。…`
- leader **追加**：`你还是这个群的 leader——除了平等参与讨论，当任务需要严肃编排时，你可以用 task_create / team_delegate 派活给成员、汇总产出给用户。但这是你的判断，不是义务。`

效果：

- 没 @ 时 leader 兜底接话（"群里没指定谁，领导先接"）。
- leader 用 **LLM 自己判断**该闲聊回一句、还是该拆活编排（R7 / R9）。
- 现有 `team_delegate` / `task_create` / 工作区交付物 tab **全部保留**——谁（不限 leader）想严肃干活就调；§1.2 修的 **team_id bug 在这里继续生效**（agent 调团队工具需要 team_id）。

---

## 9. MVP 边界（YAGNI）

**第一版做**：
- @mention fan-out + leader 兜底。
- 共享 transcript 落库（messages store）+ 注入。
- agent 互相 @ 接话（chain_depth + 宽度闸）。
- attributed bubbles 显示每个 agent 发言（已有）。
- token 预算"从最新往回填"朴素截断。

**第一版不做（各留后续一轮）**：
- 后台 LLM 群历史摘要（先朴素截断）。
- 群结论喂进长期 memory（Q3 选项 C）。
- aggregator 聚合 progress ping 的"人话渲染"（clawteam 增强）。
- "全员答完"完成信号 + 群态进 SessionSnapshot。

---

## 10. 测试策略

纯函数 / host 单测（不依赖真 LLM）：

- `mentions` 门控：被@/`@all`/空→leader 的目标解析。
- `chain_depth` 守卫：到 MAX 即停，不再 fan_out。
- 单轮宽度上限：目标 > M 时只取 M、留痕。
- 回流：agent 回复含 @ → 再解析并触发；含自@/＠user → 被忽略。
- transcript token 截断：超预算从最新往回填。
- prompt 构造器：is_leader 注入身份段；team_id 出现在 prompt（沿用已加的回归测试）。

---

## 11. 涉及文件清单

**改造**：
- `src/gateway/handlers/teams.rs` — `handle_chat_send` 改为存消息 + 调 fan_out。
- `src/teams/leader_prompt.rs` — 扩展为群成员 prompt 构造器（含 transcript 注入 + 身份）。

**新增**：
- `src/teams/broadcast/`（或 `src/teams/chat_broadcast.rs`）— 广播编排器（fan_out + chain_depth 守卫 + 回流 + 护栏）。
- 可选 `src/teams/member_prompt.rs`（若不就地扩展 leader_prompt）。

**复用（只读 / 接线）**：
- `src/teams/messages/{store,mentions,types,router}.rs`
- `src/gateway/event_emitter/team_fanout.rs`
- `src/builtin_tools/team/*`

---

## 12. 实现时需核实（writing-plans 阶段细化）

1. `TeamMessage`（`messages/types.rs`）是否有可存 `chain_depth` 的字段（`meta`/`payload`）；没有则加一个。
2. 群消息存储后触发 fan_out 的机制：直接函数调用 vs 经 event_bus（注意 MEMORY 记录 "GatewayContext 无 event_bus 需 boot 注入"）。MVP 倾向**直接调用**最简。
3. agent run 完成事件如何捕获以回流：`execution_adapter.execute` 的返回 / emitter 回调 / run-complete 事件订阅。
4. `RunRequest.metadata`（已是 `HashMap`）携带 `chain_depth` 与 `author`。
5. leader 兜底：`team.leader_id` 作为空@时的默认目标。
6. 并发上限：多 agent 同时 spawn run 是否受 `execution_adapter` 既有并发限制约束；宽度闸 M 与之协调。
7. 共享 transcript 注入格式：`[user:name]` / `[agent:id]` / `[system]` 前缀（openteams 风格），token 用既有计数器估算。

---

## 13. 风险与未决

- **fan-out 宽度 vs token**：M=5 默认，可配；`@all` 大群需观察实际成本。
- **chain_depth=6 是否够日常讨论**：可配，上线后据实调。
- **agent 严格输出 @ 协议**依赖 prompt 约束（R9）；模型跑偏不出 @ 则链自然断（可接受，非故障）。
- **并发 run 资源**：多 agent 同时 spawn 的进程/内存/provider 限流，需与既有 run 调度协调。

---

## 14. 已修 bug 记录

§1.2 A 的 team_id 缺失 bug 已在本会话修复（`leader_prompt.rs` + `handle_chat_send` 两处 + 回归测试），与本设计正交，**保留**——广播范式里 agent 调团队工具同样需要 team_id。
