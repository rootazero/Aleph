# Multi-user Round 11 — 权限要在触发时刻成立：九个无主执行器、一个解析器

- **日期**：2026-09-23
- **分支**：`worktree-multiuser-r11`（worktree `/home/zou/data/workspace/Aleph-wt-multiuser-r11`，基于 main `b5cf9aed3`）
- **Status**：设计已定（用户在线逐段批准 §3–§6；全部裁定见 §2）
- **承接**：FEATURE_LOCATOR §5.22 多用户线第十一轮。前十轮裁定不重做（r10 spec §1 的「已裁定区域」表整体沿用）。
- **参考项目**：`/home/zou/mnt/macmini/TBU4/Github/qm`（qm 第五次走查；上次走查 = r10，2026-09-03，基线 qm `d15295ea` 之前，本轮审 346 个非 merge 提交）。
- **本轮重心（用户裁定）**：**还债 + 修缺陷**；qm 增量只取能直接连到现有模块的部分，新能力一律记入 §8 推迟。

---

## 1. 背景：三份输入

本轮不是新子系统，输入是三份只读侦察的合并：

1. **qm 增量**（2026-09-02 之后）。
2. **Aleph 已登记欠账**，逐条回 HEAD 验证是否仍成立。
3. **新一轮断线扫描**，重点是 r10 之后新增或被拆分的代码面。

### 1.1 qm 增量对照

| qm 机制 | qm 提交 / 文件 | Aleph 对应 | 结论 |
|---|---|---|---|
| 定时 run 的 authority 由单函数构造（scheduler / loop stage / 审批回放共用） | `8ac53f7a` `cron/authority.ts` | `build_cron_metadata`（`tasks/cron/executor.rs:496`）、`stamp_current_scope` | 对齐 |
| 特权作业要求 owner 在**触发时**仍是 admin（撤 admin 即失效） | `8ac53f7a` `unattendedGrantRefusal` | 后台 run 不带 `caller_role` ⇒ 按 operator；降级只 restamp 在线连接 | **GAP → §3（N4）** |
| 定时房间回合带名册 | `9add29b1` | `thinker/layers/room_roster.rs` 由 scope 派生 | 对齐 |
| 一条路由漏过名单式身份闸 | `546c216b` | `method_census.rs` 逐方法钉住 | 对齐（结构免疫） |
| 按名字加项目成员 | `fe5f7b4a` | `projects/authz.rs::principal_id_for_name` | 对齐（r10 ㉗） |
| 可撤销登录会话 / 滚动期限 | `d2def2f4` `4610c141` | 设备吊销 + `tokens.expires_at` | 对齐 |
| 续跑沿用**最初请求人**的 authority | `1bd53763` | T07（r10 推迟） | **本轮裁定 → §2 R-b** |
| 外部成员 + 过期时间 | `0e1aeaa0` | `UserStatus{Active,Deactivated}` 无过期 | 推迟（§8） |
| 无人值守 run 异步向 owner 请求审批 | `570db440` | 无人值守立即 fail-closed | 推迟（§8） |
| Isolated/Open 共享档位 | `1366726e` | 房间只读自己的分区 ＝ qm Isolated | 推迟（§8） |
| 个人模型 / MCP 凭据 | `ba6707eb` `31f44cde` | 装机级 | 推迟（§8） |
| fast-mode 按优先级价格记账 | `40949fc4` | `providers/metering.rs` 无 service tier | 推迟，待确认（§8） |
| background-ownership 租约 / fence compensation | `73f73c54` `b388454a` 等 | 单进程 + flock | 不适用（是部署实例租约，不是人的所有权） |
| OIDC / 目录同步 / LLM screener | 若干 | — | 不适用（外部 IdP / 违 R7） |

### 1.2 本轮缺陷与欠账清单

| # | 问题 | 级别 | 来源 |
|---|---|---|---|
| N1 | member 经 `teams.create_task`（`Class::Open`，自动打 `managed_by: dispatcher`）建的任务被派发器以 **operator** 身份执行：裸 `tokio::spawn`（`teams/dispatcher/schedule/mod.rs:195`）→ `task_run_metadata`（`runner.rs:199`）只读已失效的 task-local ⇒ 无 scope / 角色 / 作者；无档位上限、`OPERATOR_TOOLS` 全开、`allowed_users` 围栏（`ambient_actor()=None ⇒ true`）失效、花费不归人、会话行 NULL/NULL 被 operator 收养 | **HIGH** | 新发现 |
| N2 | 停用只冻结四条腿（goal/loop/cron/heartbeat），无 teams/workflow 第五腿 | MED | 新发现 |
| N3 | heartbeat 无触发时 owner 存活检查（cron 有） | MED | r10 遗留 ⑤ |
| N4 | admin 被降级后，其后台工作仍按 operator 执行 | MED | qm GAP-1 |
| N5 | 托管浏览器 profile 全员共用（`chromium-udd/<profile>` 无 principal），member 可读 / 操作 operator 已登录的会话 | MED | 新发现 |
| N6 | `session.artifact` 裸 `TopicEvent` 落 `_ => Global`，向所有连接广播他人会话键 | LOW | 新发现 |
| N7 | `event_scope.rs:537` 的 `node.*` 守卫 `include_str!("server/handler.rs")`，拆分（`4d1061370`）后该文件生产段只剩 re-export ⇒ 守卫已断 | LOW | 新发现 |
| N8 | 启动恢复（`resume_coordinator.rs:848`）角色恒 None ⇒ member 会话恢复后按 operator | MED | 设计侦察 |
| N9 | announce 投递（`announce_delivery.rs:170`）只带 `metadata_key`：无 scope / 角色 / unattended | MED | 设计侦察 |
| N10 | 持久繁忙队列重注入（`busy_queue/durable.rs:410`）重放入队时冻结的角色，含已停用者 | MED | 设计侦察 |
| D1 | `session_manager/ops/query.rs` `filter_map(\|r\| r.ok())` 静默丢坏行 ⇒ 绑定回执误报 `NothingToMove` | 欠账 | SECURITY gap 6 |
| D2 | 重新绑定不带 `--label` 清空已存 label（`projects/store.rs:866`） | 欠账 | FL r9 |
| D3 | 单项目响应无类型化信封 + 服务端未派生 `manageable` | 欠账 | r10 A13 / OI-49 |
| D4 | `AuthorityChange` doc 列 8 个动词，生产者实有 23 个前缀 | 欠账 | FL r9 ⑤ |
| D5 | `visibility_guards` 共享硬编码会话键导致不稳定 | 欠账 | FL r9 ⓪ |
| D6 | FL / r10 spec 过期表述（「分支未合并」「backfill 仍 open」） | 欠账 | 核对发现 |
| D7 | `memory_events` 无分区列 ⇒ 知道事实 id 即可跨 principal 读其历史 | 欠账 | r10 ⑤ |
| D8 | slash 快速路径手搓第二份 `ToolFacts`（`slash_command.rs:568` vs `tools/scoped/builder.rs:369`） | 欠账 | SECURITY P1 gap 4 |

**N1–N4、N8–N10 是同一个形状**：后台工作的 authority 在创建时（或从未）确定，而不是在触发时重新推导；九个执行器各自处理且不一致。判据 §13「界限要在执行时刻成立」、§16「孪生子系统」、§6「数错的方向永远是少一个」（最初数出 5 个，实为 9 个）。

---

## 2. 裁定（用户逐条批准）

| # | 问题 | 裁定 |
|---|---|---|
| R-a | 触发时读 users store **失败**（DB 错误，非查无此人） | **跳过本次、不禁用**：记录「authority unknown」，任务保持启用，下次触发重问。只有**确定**的 Deactivated / Gone 才暂停 / 拒绝。判据 §8（`Err` 只配说「我不知道」）、§15（「未知」不写成「失败」） |
| R-b | T07：房间里 goal/loop 续跑归属谁 | **最初发起人**：`carry_policy_metadata` 携带第 5 键 `AUTHOR_USER_KEY`；花费 / 审计链 / 权限随之。角色仍按此人**触发时**的当前角色推导 |
| R-c | N5 浏览器 profile | **按 principal 分 profile**（`default__u-alice`），组合方式与记忆分区同源；既有 `default` 归 owner（`u-owner`），不迁移数据 |
| R-d | 房间后台任务：发起人在任但房间创建者已停用 | **只看发起人**。权限跟行使它的人走；房间不属于创建者一人（与「名册即授权」一致） |
| R-e | D2 如何清空 label | **传空字符串清空**；不传保留原值；不新增参数 |
| R-f | 范围 | 包 1–4 + N5 全部纳入；§8 所列一律推迟 |

---

## 3. 包 1：触发时 authority 单一源（核心）

### 3.1 解析器 `src/scope/authority.rs`（本轮唯一无法复用的新模块）

```rust
pub enum FireAuthority {
    /// owner 为 None（单用户 / legacy 行）：与今天逐字节一致
    Legacy,
    Granted { scope: Option<ScopeAttribution>, author: Option<String>, role_ceiling: Option<&'static str> },
    /// 确定性结论 ⇒ 暂停 / 拒绝
    Refused(RefusalReason /* Deactivated | Gone */),
    /// store 读 Err ⇒ 跳过本次，任务保持启用（R-a）
    Unknown(String),
}
pub fn resolve_with(users: Option<&SecurityStore>, owner: Option<&str>, scope: Option<&str>, author: Option<&str>) -> FireAuthority;
pub fn resolve(owner: Option<&str>, scope: Option<&str>, author: Option<&str>) -> FireAuthority; // 读下方 slot
impl Granted { fn stamp(&self, m: &mut HashMap<String, String>); fn carried(&self) -> CarriedAttribution; }
```

- **检查对象**：`author.unwrap_or(owner)`（R-b / R-d）。
- **角色只降不升**：若携带角色 `role_is_operator(carried)`（`tools/turn_context.rs:95`）且此人当前是 Member ⇒ 盖 `"member"`；否则保持携带值。频道续跑携带的 `"guest"`（`channel_policy.rs:192`）不会被抬高；在任 admin 什么都不盖 ⇒ admin 的 cron/heartbeat 行为不变（`turn_permissions.rs:165-190`）。**N4 因此无需改降级管线**，`restamp_live_connections` 保持原样。
- **角色映射单一源**：`UserRole::wire_role()`（`security/store/users.rs`），替换 `handlers/connect.rs:208`、`handlers/users.rs:667` 两份手写映射，解析器为第三个消费者。
- **store 句柄**：新增 `CapabilitySlot<Arc<SecurityStore>>`（`users_store_slot()`），沿 `spend::install_ledger` 先例（FL §5.25）：登记进 `capability/mod.rs ALL_SLOTS`（受 `every_declared_slot_is_in_the_roster` 约束）、在 `initialize_vault` 之后安装、加入 `start/mod.rs` 的 boot install census；`MissingSemantics::FailsOpen` 如实声明——未安装 ⇒ `Legacy`（测试与最小服务器今天的行为）。

### 3.2 九个执行器 → 七个接入点

| # | 执行器 | 接入点 | Refused | Unknown |
|---|---|---|---|---|
| 1 | cron | `tasks/cron/executor.rs:149 execute_cron_job` | 既有 `disable_walled_owner_job` + Permanent 错误 | 瞬时 RetryHint，fire log 写「authority unknown」，任务保持启用 |
| 2 | heartbeat | `tasks/heartbeat/service/timer.rs:295`，**L1 探测之前** | 新增单任务 `ops::disable_walled_owner_task`（`ops.rs` 批量版的单任务变体） | tick `l1_status:"Skipped"` + 原因 |
| 3–5 | goal 运行后续跑 / goal 唤醒 / loop tick | 三条汇入 `execution_engine/execute.rs:1392 spawn_continuation_run`，在 `confirm_fire` 之后 | goal `block_if_active` + 注记；loop `transition(Paused, reason)`（复用 agent-miss 臂） | 复用 `rearm_goal_after_busy` / `rearm_loop_after_busy` |
| 6 | 团队派发器 | `teams/dispatcher/schedule/mod.rs`，`acquire_lock` 之前 | 任务置 `Paused` + 原因 | 不认领，留在 Pending |
| 7 | 启动恢复 | `gateway/resume_coordinator.rs:848 resume_metadata` | 按会话行解析 | 同左 |
| 8 | announce 投递 | `gateway/announce_delivery.rs:170-202`（另补 scope 与 `UNATTENDED_KEY`，同 cron） | 同上 | 同上 |
| 9 | 持久繁忙队列重注入 | `gateway/busy_queue/durable.rs:410 reinject_survivors` | 同上 | 同上 |

配套改动：

- **R-b 续跑作者**：`carry_policy_metadata`（`execute.rs:~1340`）加第 5 键；`goal/types.rs` 加 `author_user_id: Option<String>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`，JSON 载荷无需迁移），`with_owner_scope` 设置；`goal_wait.rs::rehydrate_owner_scope` 同时携带作者（否则唤醒路径退回房间创建者）。
- **派发器 owner / 作者**：owner 取 team 行 `owner_user_id/scope_id`（`teams/store.rs`）；legacy / NULL 时回退 origin session 行（`goal_budget::origin_session_from_metadata`）。作者在唯一入口 `CoordTaskStore::create_task` 盖 `metadata[AUTHOR_USER_KEY]`（缺省时读 `visibility::ambient_actor()`，同 `teams/store.rs` 盖 owner 的先例），覆盖约 15 个 `NewCoordTask{` 生产者。Granted ⇒ spawn 的 future 包在 `granted.carried().reestablish(...)` 里（`CarriedAttribution` 需一个 `pub(crate) fn from_parts`）。`runner.rs:199 task_run_metadata` 代码不变（它读的 task-local 现在有值了），`allowed_users` 围栏随之生效。
- **N2 第五条腿**：`handlers/users.rs freeze_owned_background_work_with` 加 `team_tasks` 腿——暂停 owner 所拥有的团队下 Pending 的 dispatcher 托管任务；`aleph_protocol::users::FrozenBackgroundWork` 加字段（`Option<usize>`，保持「没测量」与「零」可区分），`users.get` 预览与 CLI 渲染同步。

### 3.3 防回归

- **`RunRequest` 构造 census**：生产代码中每处 `RunRequest {` 构造（约 17 个文件），要么在「有活调用者」白名单中（每条写理由 + 自检仍匹配），要么调用 `scope::authority::resolve`。第十个执行器出现即红——这是本可以抓住 N8–N10 的那条 pin。
- **单测**：`resolve_with` 每臂一条（Legacy / 在任 admin 不盖戳 / 在任 member 携带 operator ⇒ member / member 携带 guest ⇒ 仍 guest / Deactivated / Gone / store Err ⇒ Unknown）。
- **行为测试**：被降级 owner 的 cron 以 `caller_role=member` 运行；store Err 时任务仍 `enabled` 且 fire log 写原因；已停用 owner 的 heartbeat 在 L1 之前被禁用；member 建的派发任务以 member 运行且被 `allowed_users` 拒绝（**变异**：删掉 `reestablish` 必须变红）；`carry_policy_metadata` 携带作者。

### 3.4 删除（熵减）

- `walled_owner_reason`（`cron/executor.rs:109`）及其测试；cron executor 的 `users_store` 参数链（含 boot 处实参）。
- 已不成立的注释：`runner.rs:193-198`（"spawns bare … genuinely has no live caller"）、`handlers/users.rs:979-983`（「刻意推迟的 heartbeat 兜底」）、`resume_coordinator.rs:876-878`、`scope/mod.rs:176-179`（"None = cron … unrestricted"）。

### 3.5 代价（这个设计让什么变难）

- 新增后台执行器**必须**调用 `resolve`——有意为之，由 census 强制。
- R-a 翻转了 cron 今天「读失败照跑」的行为：数据库抖动时所有带 owner 的作业都停。fire log / tick 结果必须写明原因，否则就是 fail-dead（判据 §14）。
- goal/loop 的 Unknown 重排要确认不烧迭代、不触发 stale grace（`PENDING_TICK_STALE_GRACE_MS` 60s vs 繁忙重试 30s）。
- `src/harness/` 不改（R10）。

---

## 4. 包 2：事件面与守卫

### 4.1 N6 + 通用事件分类 census

1. `event_visibility.rs` 的 `session_identity_of` 拆出 `classify(topic, data) -> Option<SessionIdentity>`：`None` ＝「从没听说过」，保留 `session_identity_of = classify(..).unwrap_or(Global)`。**「显式 Global」与「掉进默认臂」从此可区分。**
2. 新增 `source_census::all_topic_producers()`：遍历 `src/`（沿 `capability/census.rs` 的目录遍历先例 + `utils::source_scan` 的测试路径剔除），收集 `TopicEvent::new` 的字面量首参；常量首参解析 `pub const X: &str = "…"`（`src/` 与 `shared/protocol/src`）；`format!` 拼出的 topic（`team.<id>.*`、`CoordTaskStore::emit_task_topic`）进豁免表，按文件键、每条写理由并自检仍匹配。
3. 一条测试 `every_raw_topic_producer_has_an_explicit_arm`：对每个 topic 断言 `classify(t, None).is_some()`。
4. 补 `aleph_protocol::artifact::TOPIC => BySessionKey(session_key)`（缺 key ⇒ `OperatorOnly`）。
5. **删除** voice（`the_voice_delta_topic_is_classified_at_its_producer`）与 pty（`every_pty_topic_the_center_publishes_is_owner_scoped`）两份手写生产者扫描的「扫描」半边，保留各自的语义断言。

### 4.2 N7 + 拆分后源码守卫普查

- `every_node_topic_the_center_publishes_is_refused_to_a_member` 改为在 `all_topic_producers()` 上按 `node.` 过滤，**不**重新指向两个具体文件（生产者现在 `server/connection/mod.rs:~979`、`connection/cleanup.rs:~79`）。
- `session/steer_signal.rs:533` 只读 `gateway/session_projector.rs`，`3a9c882ae` 拆出的 `projector_sub/*` 对它不可见，且它自己手搓 cfg-test 切分 ⇒ 改为目录遍历 + `source_census::production_prefix`。
- 修正仍指向 `server::handler` 的过期注释约 12 处（`handlers/users.rs:1276`、`cluster/reverse_rpc.rs:58`、`security/shared_token.rs:24`、`caller_identity.rs`、`isolation_acceptance.rs`、`tools_invoke.rs`、`memory_scope.rs`、`connect.rs`、`method_admin.rs`、`rate_limit.rs`）。

**代价**：目录遍历 census 比单文件慢；豁免表是一份新清单（有自检，仍需维护）。

---

## 5. N5：托管浏览器按 principal 分 profile

- **单一组合点**：`browser::profile::principal_profile_key(name, actor) -> String` ＝ `memory::project_scope::scoped_agent_id(name, actor)`（`NS_SEP = "__"`）⇒ `default__u-alice`；actor 为 None 或 owner ⇒ 原名（既有 `default` 目录即 owner 的，零数据迁移）。
- **接入在 `BrowserManager` 边界**（其所有接收 profile 的方法 + `browser_tools/mod.rs` 的 `make_backend*`），不在 25 个工具文件里；`profile_tool` / `cookies` / `session` / `tabs` 自动覆盖。`crates/aleph-cdp` 不动（纯传输层）。
- **拒绝已组合的输入**（`project_scope::is_composed_id`），与 `read_partitions` 同规则——否则 Bob 手写 `default__u-alice`。
- **列表面**（`browser_profile list` 及任何 profile 列举）只列调用者自己的后缀，并去后缀显示。
- **actor 源**：`visibility::ambient_actor()`——依赖包 1 让它在后台 run 中为真 ⇒ N5 排在包 1 之后。
- **守卫**：Alice / Bob 得到不同 `user_data_dir`；owner 得到 `chromium-udd/default`；组合输入被拒；census：除 `principal_profile_key` 外无生产代码把裸 profile 串传给 `chromium_user_data_dir` / `prepare_engine`（同一提交内更新 `browser/mod.rs` 既有的 `chromium_user_data_dir(` 调用者 census）。
- **代价**：每人一份 Chromium 进程与磁盘目录。

---

## 6. 包 3 / 包 4：欠账

| 项 | 做法 | 守卫 |
|---|---|---|
| D1 | `collect_rows(rows, ctx) -> Result<Vec<T>>`：任一行映射失败即逐行 warn 并返回 `Err`（`list_sessions` / `list_by_state` 的契约是「完整列表或 Err」，于是 `classify_rescope` 如实给 `Unknown`）；只用于展示的搜索 / 预览（`query.rs:111,186`）用记录并计数的有损变体；删除 `projects_channel.rs:116-123` 的免责声明 | 坏行 ⇒ 绑定回执 `Unknown`；变异回 `filter_map` 必红 |
| D2 | `label = CASE WHEN excluded.label IS NULL THEN project_channel_bindings.label WHEN excluded.label = '' THEN NULL ELSE excluded.label END` + `RETURNING label`，返回的 `ChannelBinding.label` 报存储值（R-e） | 不带 label 保留；带 label 覆盖；`""` 清空 |
| D3 | `shared/protocol/src/projects.rs` 定义 `ProjectResult { project: ProjectRow }` 与 `ProjectRow.manageable: bool`；服务端 6 处（`handlers/projects.rs`）+ `project_manage.rs` **用该类型构造**响应（判据 §10）；新增 `projects::authz::manageable(project, actor, users)` 吸收 `handlers/projects.rs:220` 与 `project_manage.rs:238` 两份「在任 admin」查找；Panel `api/projects.rs` 5 处反序列化 `ProjectResult`，`project_page/settings.rs` 改用 `manageable` | 协议往返测试（由构造器产出）；`the_roster_is_the_named_exception` 翻转为断言零例外 |
| D4 | `security/audit.rs` doc 不再手抄动词清单，改指向 census：抓取生产 `authority_change(` 调用点、要求 detail 是 `"<verb>: "` 前缀的字面量，与测试内快照比对 | 新生产者 ⇒ 红 |
| D5 | `handlers/chat.rs:~1471` 每测试唯一会话键 | 循环 50 次 |
| D6 | FL:3491 与 r10 spec 第 5 行「分支未合并」→「已合并 `d481911d1`」；FL 中 round-8「backfill 仍 open」逐条回代码确认后标注由 `00b8f82f8` 关闭 | — |
| D7 | 幂等迁移 `migrate_add_memory_events_partition`（`ALTER TABLE memory_events ADD COLUMN partition TEXT` + `(partition, fact_id)` 索引）；写入方盖**事实自身的分区**（不是环境 `session_write_id`——dream / 清扫写的事件也得对上）；读取 `get_memory_events_for_fact(fact_id, partitions)`，RPC 面走 `memory_scope::read_partitions`、工具面走 `caller_memory_partition`（判据 §9 同一推导）；旧行按 `fact_id` 关联事实表回填，回填不上的对 scoped 调用者 fail-closed（`owner_or_legacy` 惯例）；删除 `memory_timeline.rs` / `memory_events.rs` 中「部分修复」的说明段 | 双面测试：把 A 的事实 id 交给 B ⇒ B 得空 |
| D8 | `ToolFacts::for_tool(...)` 单一构造函数（落在 `config/types/policies`），`tools/scoped/builder.rs` 与 `slash_command.rs` 都调它，删手写字面量 | census：生产代码无构造器之外的 `ToolFacts {` 字面量 |

**代价**：D1 让 `list_sessions` 更容易整体失败（一行坏数据 ⇒ Err 而非少一行）——有意，展示路径保持有损但可见；D7 留下回填不上的尾巴（fail-closed）。D3 触及 Panel ⇒ 需 `cargo test -p aleph-panel --lib`、`just wasm`（dist 单独 `panel: rebuild dist` 提交）、并跑 `canvas_wire` 集成测试。

---

## 7. 顺序与任务拆分

**依赖**：包 1 先行（N5、D7 读 `ambient_actor()`，只有包 1 之后它在后台 run 中为真）；包 2 独立，但 N7 可能已红，先修；D3 触及 webchat；D7 在包 1 之后。

| # | 任务 | 规模 |
|---|---|---|
| T01 | N7：`node.*` 守卫改建在目录遍历上；`steer_signal` 拆分盲区；`server::handler` 过期注释普查 | S |
| T02 | 包 2：`classify()` 拆分、`all_topic_producers()` census、N6 分类臂；删 voice/pty 扫描半边 | M |
| T03 | `UserRole::wire_role()` + users-store `CapabilitySlot` + boot 安装 + census 条目 | S |
| T04 | `scope/authority.rs` 解析器 + 单测 | M |
| T05 | cron 接入解析器；删 `walled_owner_reason` 与参数链 | S |
| T06 | heartbeat 接入（L1 前闸 + 单任务禁用；关 N3） | M |
| T07 | 续跑：第 5 携带键、`Goal.author_user_id`、`spawn_continuation_run` 触发时解析、唤醒路径 | M |
| T08 | 派发器：`create_task` 盖作者、认领前解析、`reestablish`；runner doc 修正（关 N1） | M |
| T09 | 启动恢复 / announce / 繁忙队列重注入接入；`RunRequest` 构造 census（关 N8–N10） | M |
| T10 | N2：freeze 与预览的 team-task 腿、`FrozenBackgroundWork` 字段（协议 + CLI 渲染） | S |
| T11 | D1、D2、D4 census、D5、D6 | S×5 |
| T12 | D3：`ProjectResult` + `manageable`（服务端 / 工具 / Panel）+ dist 重建 | M |
| T13 | D7：迁移、写入盖戳、双面读取、回填 | L |
| T14 | D8：`ToolFacts` 单一构造器 + census | S |
| T15 | N5：principal profile key 接在 manager 边界 | M |
| T16 | 文档：FEATURE_LOCATOR §5.22 round-11 条目、附录 D/E 新实例、SECURITY.md 相关小节（浏览器 profile、后台 authority）、本 spec 状态更新 | M |

每个任务独立提交（`<scope>: <description>`，英文），独立验证。

### 7.1 验证策略

- 本机内存受限：每次只跑一个 cargo，`-j1`，`CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0`，测试 `--test-threads=2`；worktree 传 `CARGO_TARGET_DIR=/home/zou/data/workspace/Aleph/target`。
- 每个修复类任务做**变异验证**：把修复变异回去，确认红的是预期那几条（判据 §18）。
- 收尾跑 CLAUDE.md「最小可信验证集」六条命令（clippy 按内存规则拆分）。
- 真机：`qa/teamchat_rooms/run.sh` 与 `qa/channels/run.sh` 仅作回归；**N1 的真机失效面需要一个 member 身份通过 `teams.create_task` 驱动派发器**——现有装置不走这条路（判据附录 D.0.139：装置能走到缺陷路径是待证命题），本轮**不新建真机装置**：N1–N10 以行为测试 + 变异为证据，真机失效面在 FEATURE_LOCATOR 条目中明确记为「未跑」，并写进 `qa/teamchat_rooms/run.sh` 的 claim 注释（附录 D.0.139 ③：结论写进装置本身）。

---

## 8. 刻意不做 / 推迟

| 项 | 理由 |
|---|---|
| principal 过期（qm external members） | 新能力；本轮重心是还债 |
| 无人值守 run 异步向 owner 请求审批 | 触及 2026-08-07 / 08-11「无人值守 = fail-closed」裁定，需单独设计 |
| Isolated/Open 共享档位 | 披露最终取决于模型复述，qm 自己承认；产品决定 |
| 个人模型 / MCP 凭据 | 新子系统（R3 运行时例外下可做，但不便宜） |
| fast-mode 优先级计价 | 先确认 Aleph 是否可能被按优先级计费 |
| T22 频道档位 × 角色合成 | 产品裁定未做 |
| 未配对发言者写入 bare `main` 分区 | 需先裁定陌生人笔记归属 |
| `flush_partitions` 压缩全部 principal 分区 | 产品裁定未做 |
| `bind_channel` / 配对 approve 的工具面 | 「需要时再做」的既有裁定 |
| member 可达的审批 / 中断面、`allowed_roots` 作用于绑定写者、房间 curated memory、goal/loop 写面（OI-44） | 均为既有产品裁定未做项 |
| r10 已裁定的全部「不移植」项 | 不重开 |
