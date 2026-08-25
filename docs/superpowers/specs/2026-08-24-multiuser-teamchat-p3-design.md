# 多用户 × 项目管理 × 团队群聊真人化 — 设计规格
# Multi-User × Project Management (P3) × Human-in-Team-Chat — Design Spec

- **日期**: 2026-08-24
- **分支**: `worktree-multiuser-teamchat-p3`（严禁触碰 main）
- **参考项目**: `T:\Github\qm`（对照结论见附录 A）
- **上游 spec**: `2026-08-04-multi-user-org-project-design.md`（P0/P1/P2 已执行；本 spec 承接其 P3 期并新增「团队群聊真人化」）
- **记录文档**: 完成后回填 FEATURE_LOCATOR §5.22 / §4.5、SECURITY.md、`src/gateway/CLAUDE.md`
- **状态**: ✅ **已实施（2026-08-25，13 任务全部完成，worktree `multiuser-teamchat-p3`）**。逐项落点与刻意未做清单见 FEATURE_LOCATOR §5.22 第八轮。与本 spec 的两处偏离，均已在实施中记录理由：① `project_manage` **不含** `bind_workspace`（用户裁定 2026-08-25；`caller_may_choose_directory()` 对无连接角色 fail-OPEN，工具面够得到那条臂）；② 房间名册的单名上限用 `speaker_label` 的 40 字符而非 spec 写的 64（两个上限＝同一条规则的两种拼法）。

---

## 1. 背景与动机 (Background)

P0（身份）/P1（隔离）/P2（项目房间）+ round 2/3 之后，多用户机件的谓词层已经完整：users 表、CALLER_USER 管道、visibility 咽喉、项目房间名册、房间共享会话与记忆分区。但两块产品面仍是半成品：

1. **Panel 项目管理**：项目页五个 tab 里 Kanban / Workspace / Memory 三个是 `PlaceholderTab`（`interfaces/webchat/src/components/project_page.rs:197`），且没有 `projects.*` 推送 topic（侧栏靠手动 refresh，`sidebar/projects.rs:33-35` 有 pin 注释）。原 spec 的 P3 期（"项目页五个 tab 全活"）从未执行。
2. **团队群聊是纯 agent 面**：`teams.chat.send` 虽已 member 可达（`Class::Open` + `KeyChecked`），但人类消息匿名落库——`team_messages` 表**没有 author 列**，`from_agent = "user"` 是唯一的人类记号（`src/teams/messages/store.rs:170-200`）。第二个真人的发言在 transcript、history 回放、prompt 注入里与第一个人不可区分；每条人类消息无条件激活团队 fan-out；审批卡假设"看 Panel 的就是 operator"。

本轮把多用户能力应用到这两个面：**真实用户（operator + members）可以和 agent 团队在同一线程里协作**，且**项目页成为多人协作的完整管理面**。

## 2. 已锁定的四个决策 (Settled Decisions)

以下由用户在 2026-08-24 brainstorming 中裁定，不再重开：

| # | 决策 | 裁定 |
|---|------|------|
| D1 | 真人群聊的架构位置 | **团队线程自身多人化 + 项目页集成**。`team_messages` 加 author 列，团队群聊保留独立面；项目页把 room-scoped 团队的群聊与任务嵌进 P3 tab。**不**合并 `team_messages` 与 `session_events` 两套存储 |
| D2 | agent 激活语义 | **提及闸 + 旁听模式**。多真人线程中无 @ 的消息只落库不激活（旁听）；`@agent` 激活该 agent；`@all`（既有语法）激活 leader 分发。单真人线程逐字节保持现状。纯确定性规则，零额外 LLM 调用（LLM inbound screener 既往已裁定违 R7/R10，不重提） |
| D3 | 审批路由 | **路由到发言人**。触发 run 的真人收到并裁决常规执行审批卡（其 exec-tier 上限照常钳住）；`OPERATOR_TOOLS` 升级闸的卡复用 `ExecApprovalRecord::operator_only`，仍只落 operator |
| D4 | 本轮范围 | 群聊真人化 + **P3 五 tab 全量**（含 goals/loops 盖 project scope + `projects.changed` 推送 topic）+ **R8 `project_manage` 工具** + **房间名册 prompt 层**。**排除** P4 渠道进房间 |

## 3. 现状基线 (Current-State Anchors)

实施前的关键锚点（2026-08-24 实测，探索报告全文见会话记录）：

- **projects**: `src/projects/store.rs`（projects + project_members 两表）、`src/gateway/handlers/projects.rs`（14 个 RPC，三道闸 `gate_project`/`require_owner`/`require_directory_choice` + `require_known_user`）、`src/projects/roster.rs`（进程内投影，写锁内 `republish_roster_locked` 发布）。
- **teams broadcast**: `src/gateway/handlers/teams/canvas.rs`（`teams.chat.send`：盖 transcript → 铸 run_id → `register_fanout` → spawn `dispatch_user`）、`src/teams/broadcast/mod.rs`（风暴闸 + `run_member` + `member_run_metadata`）、`src/teams/messages/store.rs`（`team_messages` 无 author 列）、事件单一源 `src/gateway/event_emitter/team_fanout.rs::publish_team_event`。
- **可见性**: `src/gateway/visibility.rs`（`ambient_actor` ≠ `ambient_owner`；`owner_and_scope_visible_to` → project scope 走 `roster::is_member`）、`event_visibility.rs`（`team.<id>.*` 按团队 owner/scope 门控）、`method_visibility.rs` + `method_census.rs`（projects.* 全 `Class::Open`）。
- **审批**: 团队成员 run 刻意不标 unattended（`mod.rs:~110-140`，假设 operator 在看），审批走 OperatorApprovalRequester；member 已有 `exec.approvals.pending`/`exec.approval.resolve` carve-out；`ExecApprovalRecord::operator_only` 机制在位（§4.12 round-12 ①）。
- **Panel**: `project_page.rs`（五 tab 骨架，三个占位）、`team_events.rs`（团队事件订阅唯一解析点）、`api/team_chat.rs`（send/cancel/history/thread）。
- **已知活缺口**（本轮 E 节修）: 裸 `chat.send` 打到新认领房间键会把会话永久盖成 personal（FEATURE_LOCATOR §5.22 round-2 ②，仍开）。

## 4. A — 团队群聊真人化 (Humans in Team Chat)

### A1. 存储署名 (Author Attribution)

- `team_messages` 加列 `author_user_id TEXT`（nullable）。SQLite `ALTER TABLE ... ADD COLUMN`，幂等守卫照 `agent_envs` 的 ALTER 模式。
- **写者盘点**（§0「收敛写者时要数一遍写者」，一次数完）：① `teams.chat.send`（人类，盖 `Some(user_id)`）；② `run_member` agent 回复（`None`）；③ `post_system`（`None`）。`send_message_with_ttl` 及其下层签名携带 `author_user_id: Option<String>`，只有 ① 传 `Some`。
- 发言人取值：`visibility::ambient_actor()`（**不是** `ambient_owner`——§5.22 round 3 ① 判据：`ambient_owner` 答"这一行属于谁"，署名要答"谁在问"）。
- `from_agent = "user"` 保持不变（既有渲染器/回放键在它上面，向后兼容零破坏）。
- Display name 解析：读时按 `author_user_id` 批量查 users 表（`users.list` 同源），**不落库**（改名后历史消息跟随新名字；display_name 是投影不是副本）。

### A2. 激活语义 (Activation Semantics)

**谓词**（纯函数，单测矩阵覆盖）：

```
multi_human(team_id, speaker) :=
    |distinct{ author_user_id ≠ NULL in team_messages(team_id) } ∪ {speaker}| > 1
```

- 自包含：只数线程内真实出现过的人类作者（agent/system 行 `author_user_id IS NULL` 不计入），不依赖名册同步状态。
- 新增 store 查询 `distinct_human_authors(team_id) -> Vec<String>`。

**分派规则**（在 `teams.chat.send` 落 transcript 之后、spawn 之前裁决）：

| 场景 | 无 @ | `@agent` | `@all`（既有语法） |
|---|---|---|---|
| 单真人（现状） | 激活 leader 分发（**逐字节不变**） | 激活该 agent | 激活 leader 分发 |
| 多真人 | **旁听**：落库 + 发 `team.<id>.message` 事件，不铸 run，不激活 | 激活该 agent（点名直达，不经 leader） | 激活 leader 分发 |

- @ 词法以 broadcast 既有实现为准（`@agent-id` 与 `@all`）；plan 阶段若发现别名（如 `@team`）按既有词法处理，**不新造语法**。

- @ 解析复用 broadcast 既有的 `resolve_targets` @ 语法（agent 链式接话同一套词法），提取为共享纯函数供 `chat.send` 前置调用——**不**另写一份提及正则（单一源）。
- 旁听消息天然进下次激活的 transcript（`format_transcript` 每次从 store 重建），对齐 qm 的 overheard 语义。
- **响应形状变化**：`teams.chat.send` 响应从 `{run_id}` 变为 `{run_id: Option<String>, observed: bool}`。旁听时 `run_id: null, observed: true`。Panel/TUI 对 `run_id: null` 渲染"已送达（旁听）"态而非错误（客户端解码字段带 `#[serde(default)]`，老 server 兼容）。
- 并发激活：多个真人各自 @ 不同 agent 产生并发 run——现状已允许（fanout 按 run 注册），风暴闸按 dispatch 树各自封顶，不新增闸。

### A3. 审批路由 (Approval Routing)

- `dispatch_user` 链路携带发言人：`GroupChatBroadcaster::dispatch_user` 新增 author 参数，下传至 `run_member` 的 `member_run_metadata`（复用 `AUTHOR_USER_KEY` 常量 + `scope::CarriedAttribution` 既有载体；census `scope_stamping_producers_are_all_accounted_for` 需为新 producer 记账）。
- **常规执行审批卡**：登记时按发言人归属（安全位随记录走——§5.6 判据；具体载体在 plan 阶段定，约束：发言人经 `exec.approvals.pending` 能列出、能 resolve；operator 按角色恒可见可裁决全部卡——机主监督权）。
- **`OPERATOR_TOOLS` 升级闸**：照旧走 `ExecApprovalRecord::operator_only`，member 结构上不可见（回归测试点名：member 触发的 run 命中升级闸时卡不出现在其 pending 列表）。
- **发言人离线**：超时语义照旧（不回落到 operator 自动批——回落等于把 operator 变成 member 动作的默认批准人，方向反了；operator 主动去批是监督，自动落到他头上是转嫁）。
- **档位钳制**：团队成员 run 携带发言人的 `caller_role`（`CarriedAttribution` 已含），member 触发 → member 的 exec-tier 上限与角色天花板照常生效。⚠️ 此链路 **fail-closed**：author/role 缺失时按 member 处理，**不得**依赖 `role_is_operator(None) == true` 的既有 fail-open 缺省（§5.22 round 3 ③④）。
- unattended 判定：发言人在线（有活连接）→ attended 语义照旧；此判定与既有 `platform="webchat"` 假设的差异只在"谁在看"，不在"有没有人看"。

### A4. Panel 渲染 (Panel Rendering)

- `team.<id>.message` 事件 payload 增加 `author_user_id` + `author_display_name`（可选字段）。
- **发布补线**：`teams.chat.send` 落库后必须发 `.message` 事件（现状：自己的消息靠 composer 乐观渲染，其他真人的消息无实时通路）。Panel `team_events.rs` 按 message id 去重（乐观渲染的自己那条与事件回声不重复入列；判据用消息 id，不用"是不是我发的"——§6.1 竞态教训）。
- `teams.chat.history` / `.thread` 行增加同样两个可选字段；Panel DTO 侧 `#[serde(default)]`（两 crate wire 契约判据：新增字段必须对老 server 可缺省，Panel 解码对账测试同步扩展）。
- 气泡：`author_user_id` 存在 → 显示 display_name（人类气泡）；否则维持 agent/system 现状。

### A5. Prompt 注入 (Prompt Injection)

- `format_transcript`（`src/teams/broadcast/`）：人类消息渲染为 `[alice]: content`（display_name 解析一次、缓存于本次 format；解析失败回退 user_id；全部过 `xml_util::escape_xml`）。
- `member_prompt.rs` 名册块：既有"群成员名册"追加真人参与者小节——来源 = 线程 distinct 人类作者 ∪（团队 room-scoped 时）房间名册；真人标 `(human)`（qm `isBot→agent` 的反向）。名字数量与长度 capped（防 prompt 膨胀）。
- prompt 内既有"不要 @ user"约束改写为"不要 @ 任何真人参与者"（工具/prompt 两份表述同批改——§0 判据）。

## 5. B — P3 五 tab + 推送 (Project Page Full Form)

### B1. Kanban tab

- **数据**：`scope_id == "project:p-<id>"` 的团队（`ScopedTeamStore` 既有查询）→ 其 `coord_tasks`（既有 board RPC 族，§6.6 团队看板组件复用，过滤到房间团队）。**不给 `coord_tasks` 加 project 列**——经 teams.scope_id 一跳即达，加列是第二个真源。
- **goals/loops 进项目**：创建路径盖 ambient scope——`Goal.scope_id` / `LoopSpec.scope_id` 字段已存在，写路径改为从 `ScopeAttribution` task-local 取（房间 run 内创建 → `project:p-<id>`；非房间 → 现状 personal 不变）。读侧：goals/loops 列表 RPC 增加 scope 过滤（可见性走既有 `project_visible_to`）。
- **群聊入口**：看板内每个团队卡带"打开群聊"入口，走既有 `on_open_group` 流（A 的成果在此消费）。
- 进展实时：复用既有 `team.<id>.task.*` 事件（已按团队 scope → roster 门控）。

### B2. Workspace tab

- **新 RPC**：`projects.workspace.list`（目录列表）+ `projects.workspace.read`（有界文本预览）。闸（单一入口，写在 handler 咽喉）：
  1. `gate_project`（roster 成员可读）；
  2. 路径 canonicalize 后必须在 `workspace_path` canonical 根之下（比较两边同函数归一化——§0 `\\?\` 判据；显示用 `display_string`）;
  3. 尊重 `deny_read_globs` 地板（复用 `deny_globs::glob_to_anchored_regex` 展开，**不**近似第二份——§3.15⑤）；
  4. 预览有界（字节上限 + 二进制探测拒绝）。
- 传感器纪律：只读 RPC 不建目录（§8「传感器不许创造它测量的东西」）；`workspace_path` 未绑定 → 明确的"未绑定"响应而非空列表。
- 车道：新 RPC 进 `gateway/lane.rs::override_for` 只读名单（§6.8 判据：后缀启发式不认 `projects.workspace.list`）。
- Panel：Workspace tab = `DirectoryBrowser` 只读变体（浏览 + 预览；**不做写**——成员写文件走 agent run 工具面，受审批/tier 闸）。

### B3. Memory tab

- 项目分区 `{agent}__p-<id>` 的 curated MEMORY.md + notes 列表。**纯连线**：RPC 走既有 `memory.*` 族（`gateway::handlers::memory_scope::read_partitions` 咽喉 + `partition_visible_to` 的 p-* → roster 臂都已在）；Panel 复用 §6.7 记忆浏览组件，锁定单分区。
- 若既有 memory RPC 不接受显式分区参数的面有缺口，按 round 3 的单一源模式补（经 `read_partitions`，不绕）。

### B4. `projects.changed` 推送 topic

- 新 `GatewayEventFrame` 变体 `ProjectsChanged { project_id, verb }`（verb ∈ create/rename/archive/bind_workspace/member_add/member_remove/remove）。
- 生产者：七个 mutation handler 在 store 提交后发（roster 投影仍在写锁内发布不变；事件是刷新提示不是权威——§4.5 既有分工）。
- `event_visibility`：新变体按 project roster 分类（resolve project → `roster::is_member`；`every_frame_variant_is_classified` pin 强制归类；帧自带 project_id——§4.8「帧自报归属」判据）。`member_remove` 的帧对**被移除者**也投递一次（他需要知道自己被移出以刷新列表；帧不含其他 payload，无泄漏）。
- 消费者：`sidebar/projects.rs` + `project_page`（名册/设置）订阅 → 自动刷新；删掉"无推送 topic"的 pin 注释与手动 refresh 兜底说明（同一事实两份表述同批改）。

## 6. C — `project_manage` 工具 (R8 Tool Surface)

- **动作**：`list / get / create / rename / archive / member_add / member_remove / member_list / bind_workspace`。
- **谓词共用推导**（"一个动词 N 张脸共用判据也共用推导"）：把 `gate_project` / `require_owner` / `require_known_user` 的判定核心从 `handlers/projects.rs` 下沉到 `src/projects/authz.rs`（显式 actor 版），RPC handler 与工具都调它——handler 变薄，工具不抄第二份。actor：工具面用 `visibility::ambient_actor()`（task-local `CALLER_USER` 在 spawn run 里死——§5.22 既有教训）。
- **目录选择权**：`bind_workspace` 与带 path 的 `create` 是 `workspace_path` 的第 4/5 个写入者，必须过 `caller_may_choose_directory` 的**工具面孪生**（读 ambient role，fail-closed：role 缺失即拒）。
- **注册五处一次做全**（`plugin_manage` 教训 + §5.21 hub 工具清单）：目录条目（description 指常量）→ `create_tool_boxed` → `core_tools::reg`/registry → **dispatch match arm**（`dispatchable.rs` 守卫点名）→ `groups.rs` 分类。会话模式分区：work + code（chat 不呈现）。
- 描述字节：`catalog_description_bytes_ratchet` 抬升，按 R10 三问作答写进账本。
- 进程内经 `ProjectStore` 句柄（boot 注入），天然满足 roster "写必须经 IPC/进程内单一源"契约。

## 7. D — 房间名册 prompt 层 (Room Roster Prompt Layer)

- **名册块**：新层 `src/thinker/layers/room_roster.rs`，仅当会话 scope 为 `project:*` 时渲染 `<room_context>`（成员 display_name 列表 + owner 标记，数量/长度 capped）。`stability() = Dynamic`（名册可中途变）；`priority()` 按阅读顺序独立作答（§1「stability 说字节变不变，priority 说阅读顺序」）。自带字节界测试（"表上每个 Dynamic 层都欠一个自己的界"）。
- **发言人**：房间会话的历史用户消息在 build_prompt 投影为 `[alice]: text` 前缀（`session_events` payload 的 `author_user_id` 已在——P2 产物；**投影不改存储**）。渲染点收敛在 prompt 构建的单一历史渲染函数。
- 名字过 `escape_xml`；防伪造：投影前缀只由服务端从 payload 派生，模型/用户正文中的 `[xxx]:` 无权威性（行式伪造判据 §2.3/§4.12——名册块不用行式权威格式）。
- 落地后跑 `aleph-server prompt-size` + `stable_prefix_ignores_per_run_facts` 守卫确认无 per-run 字节进稳定前缀。

## 8. E — 修复与连线 (Fixes & Wiring, 同批熵减)

1. **裸 `chat.send` 打 personal 戳**（§5.22 round-2 ② 仍开）：`ensure_session_under_request_scope` 对命中某 project `current_session_key` 的键强制 project scope——网关拥有的映射走网关通道，不信 request metadata（§0 首条判据）。新增 `ProjectStore::project_of_session_key(key)` 查询。
2. **删 `PlaceholderTab` + `project_room.coming_soon` i18n 条目**（被 B 替代；熵减原则——死代码同批清）。
3. **Census 全数过闸**：新帧分类 pin、`method_visibility` 登记（workspace 两 RPC = `KeyChecked`）、`method_census` 归类（`Class::Open`）、lane override、dispatchable 守卫、描述棘轮、Panel 解码对账、`scope_stamping_producers` census。
4. **文档回填**：FEATURE_LOCATOR §5.22（新 round 条目）/§4.5（真人化）、SECURITY.md（审批路由变化 + workspace 读面威胁面）、`src/gateway/CLAUDE.md`（若新增地雷形状）、本 spec 状态行。

## 9. 边界语义 (Edge Semantics — 定死)

1. **旁听 ≠ 丢弃**：旁听消息必须落库 + 实时推送到所有可见连接；只是不铸 run。
2. **谓词只数人**：`author_user_id IS NULL`（agent/system）不计入 multi_human；单真人线程行为逐字节等于现状（回归测试以字节断言）。
3. **模式单调（只进不退）**：multi_human 逐消息从线程历史现算，而 distinct 作者集只增不减——第二个人第一条消息起线程**永久**进入多真人模式，此后所有人（含 operator）都受提及闸。不提供"退回单人模式"的开关（那是第二个真源）。
4. **审批不回落**：发言人离线 → 卡照常超时，**不**自动转 operator；operator 凭角色恒可见可代裁（主动监督 ≠ 默认转嫁）。
5. **升级闸不降格**：member 触发的 run 命中 `OPERATOR_TOOLS` → 卡 operator_only，member pending 列表不出现（回归点名）。
6. **Workspace 只读**：list + 有界预览；不提供写、不提供下载归档；`deny_read_globs` 命中 → 该行不出现在列表（与读拒绝同形，无 oracle）。
7. **`run_id: null` 是成功**：旁听响应对客户端是正常终态，不重试、不报错。

## 10. 测试策略 (Testing)

- **单测**：激活谓词矩阵（单/多真人 × 无@/@agent/@all × agent 行不计入）；署名写者（三写者只有一个盖戳）；审批路由（发言人可列可裁 / operator 恒可裁 / operator_only 不降格 / role 缺失 fail-closed）；workspace 闸（路径逃逸、软链、deny_globs、未绑定）；goals/loops scope 盖戳（房间内/外）；`projects.changed` 分类与投递（成员/非成员/被移除者）。
- **守卫**：帧分类 pin、census 全套（见 E3）、单真人字节不变回归、Panel 解码对账。
- **真机 QA**：复用 `panel-realmachine-qa-harness`（隔离 `ALEPH_HOME` + `pair --user` 第二身份 + Playwright + mock provider）。端到端脚本：两真人一 agent 团队线程——旁听不激活、@ 激活、双端署名实时可见、member 批自己的卡、operator_only 卡 member 不可见、项目页五 tab 各一条效果断言、`projects.changed` 实时刷新。
- **验证集**：CLAUDE.md §10 五条命令 + `cargo test -p aleph-panel --lib`（非 check）+ `just wasm`（⚠️ worktree 无 `node_modules`：wasm 重编需 `npm ci`，round-2 曾因此三处 Panel 修复未上线——计划阶段显式排一步）。

## 11. 刻意不做 (Non-Goals — 勿重提)

- **P4 渠道进房间**（IM 群绑定项目）：原 spec backlog，本轮用户明确排除。
- **LLM shouldRespond judge / ambient judge**（qm Stage B/C）：既往裁定违 R7/R10（不移植 inbound screener），提及闸是确定性替代。
- **Panel 用户管理 UI**：`users.*` 保持 CLI-only（既往裁定：admin-gated + loopback 免新授权概念）。
- **合并 `team_messages` 与 `session_events`**：D1 已裁定不收敛存储。
- **qm 参与者窗口**（join-from-now / leave-freezes-history）：房间维持"加入即见全史"（组织内同事语义）；记 backlog 不实施。
- **`coord_tasks` 加 project 列**：经 teams.scope_id 一跳即达，加列是第二真源。
- **逐资源 ACL grant / posture enum**：round 2 既有裁定。
- **display_name 落库到消息行**：投影不是副本。

## 12. 风险与诚实声明 (Risks)

1. **激活语义是行为变化**：多真人线程里 operator 也要 @ 才能激活——这是 D2 裁定的直接后果，写进用户文档与 Panel 旁听态提示；单真人不变保住主线体验。
2. **审批载体待 plan 定形**：A3 的"按发言人归属登记"具体走 session_key 还是记录级 author 字段，plan 阶段读 `ExecApprovalRecord` 现状后定；约束（可列/可裁/operator 恒裁/升级闸不降格）在本 spec 定死。
3. **workspace 读面是新威胁面**：成员可枚举绑定目录文件树。缓解：roster 闸 + canonical 包含 + deny_globs 地板 + 有界预览 + SECURITY.md 记录。剩余风险：owner 绑定过宽目录（如整个 HOME）即向全 roster 披露——文档警示，不做额外闸（owner 选择目录本就是 `caller_may_choose_directory` 特权行为）。
4. **prompt 字节增长**：名册块 + `[name]:` 前缀增加房间会话 token；名册 capped、层有界、prompt-size 实测后记账。
5. **`team_messages` 迁移**：ALTER ADD COLUMN 幂等，老库无损；老 server + 新 Panel / 新 server + 老 Panel 靠可选字段双向兼容。

## 13. 附录 A：qm 对照结论 (Divergence from qm)

| qm 模式 | 采纳？ | 落点/理由 |
|---|---|---|
| 内联署名 `alice: ...` + People-here 名册块 | ✅ | A5 / D（transcript 投影 + 名册层） |
| overheard（旁听落库不激活） | ✅ | A2 旁听模式 |
| 提及闸（Stage A 确定性过滤） | ✅ | A2（thread-stake 简化为 distinct-authors 谓词） |
| ambient judge / shouldRespond LLM（Stage B/C） | ❌ | 违 R7/R10 既往裁定 |
| 审批卡发给具体的人（ask-agent 同意流） | ✅（形变） | A3 发言人路由；ask-agent 指令语法不移植 |
| 参与者窗口（valid_from_seq/valid_to_seq） | ❌（backlog） | 组织内同事语义，加入即见全史 |
| Project = group scope 别名 | 已等价 | Aleph `ScopeId::Project` 即同构 |
| 名册版本乐观并发（scopeVersion 环绕整轮） | ❌（记录） | Aleph 房间名册变更不撕裂在跑 run（roster 投影写锁内发布已保序）；整轮版本闸收益低于复杂度，记 backlog |
| routeWake 转向策略 | 已领先 | busy_queue 三态 + steer_signal |
| 无用户表 / 内存名册 / 4 层分散授权 | ❌ | qm 自身反模式，Aleph 已有更强形态 |
