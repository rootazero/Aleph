# src/gateway/ — 安全边界护栏

> 本目录是 Aleph 的网络信任边界。改动认证 / 授权 / Origin 逻辑高风险，
> 编辑前必读。完整模型见 [SECURITY.md#auth-ux](../../docs/reference/SECURITY.md#auth-ux)。

## 信任模型 = 网络边界 + Gateway token

- **网络边界**：默认只绑 `127.0.0.1`；`[gateway] host = "0.0.0.0"` 显式开放整个局域网。
- **本机 (loopback)**：免 token 自动 operator（零配置，勿回归）。
- **远程 (LAN)**：纯 WS 直连（**非 channel 通道**），行为等同浏览器打开 core IP。授权凭据
  按优先级（`connect::resolve_connect_auth` 4 级）：① `device_token`（`aleph-dt-*` 长效绑设备）
  ② `bootstrap_ticket`（`aleph-bt-*` 5min 一次性配对票，扫 `?bt=` QR，connect 时换取 device token）
  ③ legacy 共享 **Gateway token**（`aleph-<uuid>`，`SharedTokenManager`）。校验通过 = operator，
  权限与本地**完全一致**（单层）；未通过 = 登录墙（WS 派发仅放行 `connect`）。**长效凭据不进
  URL/QR**——QR 只编码一次性配对票，修复 `?token=` 泄露向量。**授权之后连接携带 (user, role)**——
  `connect::resolve_connection_identity` 把已授权连接进一步解析成 `(Option<user_id>, role)`
  （`"operator"` / `"member"` / `"guest"`），member 由 `method_admin.rs` 闸在 `process_request`
  单点强制（P0 身份基础，`src/gateway/caller_identity.rs` 模块 doc 有完整链路）。
  ⚠️ **两道闸别混**：登录墙 `handler.rs::wall_admits` 是**访客墙**，`operator` 与 `member`
  **都放行全部方法**；admin/member 的分野是**更深处**那道 `method_admin.rs` 闸。把 member
  写进登录墙的拒绝分支，等于真 member 每一帧都吃 `AUTH_REQUIRED` 并被 flood-guard 当滥用者
  踢掉，而所有把 task-local scope 在墙**下面**的测试都会保持全绿。另：`connect` 回包里的
  `role`/`authorized`/`needs_token` 必须回**解析后**的角色（`connect_verdict`），不是凭据级
  verdict——否则 member 拿到 operator UI，被停用用户的有效设备被告知 `authorized:true` 却
  在墙上寸步难行。
- **撤销**：① `gateway.token.rotate` = 核弹级（重生共享 token **并** `revoke_all_panel_devices`，
  cluster 节点不受影响）+ **强踢全部远程 socket**（`start/mod.rs` 发 `TokenRotated` 事件 →
  `handler.rs` 的 `is_token_rotated_frame` 关闭远程 session）。② `gateway.devices.revoke
  {device_id}` = 单设备，**同样立即生效**，且是 `users.update` 停用用户时吊销其全部设备**共用
  的同一条**管线（P0 身份基础 Task 5，2026-08-04）——顺序、单一源、capability 注入的位置见下方
  地雷 2。清单 `gateway.devices.list`（仅 `device_type='panel'`，带 `connected` 实时位）。
  ⚠️ **地雷 1（wire form）**：`is_token_rotated_frame` / `device_revoked_id` 读的是 `publish_frame`
  的 **wire `topic`**（非流事件包成 `{topic,data}`），**不是顶层 `type`**——读错字段谓词恒 false，
  `rotate` 变哑弹（曾静默失效，2026-07-17 修）；改它测试必须喂 `publish_frame` 真实输出。
  ⚠️ **地雷 2（顺序 + 单一源）**：**先降权、后关 socket**，且这是唯一一条管线——单一源
  `gateway_devices::revoke_device_and_kick`：store revoke（`device_token_mgr.
  revoke_panel_device`）**只有** `revoked == true`（未知 id / cluster 节点 / 已撤销都是
  no-op）**才**继续，先 `invalidate_device_sessions`（把该设备的活连接同步降回 guest）**再**
  发 `DeviceRevoked` 关它的 socket——两步严格顺序写死在这一个函数体内（`.await` 完了才
  `publish_frame`），不是"handler 算完、接线处再补一刀"。只关 socket 的话，那条 socket 上
  已经排队的帧仍会以 operator 身份被服务完——`tokio::select!` 两条臂是伪随机调度，不存在
  "事件一定先到"。（反过来不必担心自撤销收不到回包：响应由**派发它的那条 read 臂**同步写出，
  事件臂要下一轮 select 才被 poll。）连接表 (`connections`) 与事件总线 (`event_bus`) 两个
  capability 由 `start/mod.rs` 接线处构造，经 `DevicesHandlerContext` 注入给
  `handle_devices_revoke`；`users.update` 的停用路径对 `list_device_ids_for_user` 返回的每台
  设备调用**同一个** `revoke_device_and_kick`（`UserDeactivationKick` 在接线处注入**同一份**
  `connections`/`event_bus`），不另写第二份。⚠️ 这不是对 R1/R4 的开口：函数本身仍不直接碰
  平台 API，只是把已经被注入的 capability 串成一次调用——旧版"handler 保持纯 I/O，会话副作用
  归接线处"描述的是一个从未真正存在过的切分（`start/mod.rs` 的闭包此前一直在做业务判断，只是
  物理位置在 handler 之外），Task 5 的抽取纠正的是文档而非架构；这里唯一要守的红线是两个
  消费者共享同一个函数，不是"handler 该不该碰 capability"。
  ⚠️ **地雷 2b（关 socket 只做了一半）**：踢人的 close reason 必须同时登记进客户端的
  `shared/ui_logic/src/connection/failure.rs::AUTH_KICK_REASONS`。漏登记不会报错——Panel
  把这次踢当成普通掉线，先花一个 backoff 延迟拿**已经死掉的凭据**再连一次，然后才由握手
  兜到登录墙；更糟的是那条短路径同时是登录墙判断"凭据被拒 vs 从未有过凭据"的地方，漏了
  就会用**首次登录**文案迎接一个刚被踢下线的人，并且把死凭据继续留在 localStorage 里。
  `device_revoked` 就是这么漏的（2026-07-27 真机 QA 抓到，同轮修）。
  ⚠️ **地雷 3（命名空间）**：`devices` 是 panel 与 cluster 节点**共用的一张表**，且两边的
  `device_id` 都是**对端自报**的；`upsert_device` 的 ON CONFLICT **有意不改写 `device_type`**
  （否则配对能把节点行改姓），而 `list_panel_devices` 恰恰按 `device_type='panel'` 过滤。
  所以任何「按 id 认领一行」的路径都必须先问这行属不属于另一半命名空间——两个守卫是对称的：
  `exchange_bootstrap_ticket` 拒非 panel 行（`DeviceIdConflict`，**在消费票之前**，撞 id 不该
  烧掉 operator 的一次性票），`cluster::admit_node` 拒 panel 行（`IdentityConflict`）。判据只有
  `PANEL_DEVICE_TYPE` 一个字面量；注意判据是「**是不是 panel**」而非「是不是 node」——
  `admit_node` 回填出的行 `device_type` 是 **NULL**。少了这道闸，一张合法配对票就能换到一枚
  **roster 列不出、`revoke_all_panel_devices` 吊不掉、连 rotate 核弹也炸不到**的 operator token。
  ⚠️ **地雷 4（重配对）**：`store::upsert_device` 的 ON CONFLICT 必须清 `revoked_at = NULL`，
  否则已 revoke 的 `device_id` 扫码重配会复活成**不可列/不可撤销、扛过轮换**的 operator token
  （device 行藏在 `list_devices` 的 `WHERE revoked_at IS NULL` 之外，而新 token 行 revoke 戳为
  NULL 照常校验通过）。设备令牌/配对票逻辑在 `security/device_token_manager.rs`。
- **配对地址由服务端解析**：`gateway.ticket.create` 回传 `urls[]`，源头是
  `tls::discover_interface_ips`（**自签名 SAN 与配对 URL 共用同一份"本机可达地址"**，不得各说
  各话）。客户端**不要**用 `window.location` 拼——在本机桌面 App 里拼出来的必然是
  `http://127.0.0.1:<port>/?bt=…`，手机扫了打不开。loopback-bound 的 core 宁可一个地址都不
  给；Panel 侧唯一的回退是自己的**非 loopback** origin（专治反代部署）。无头机走
  `aleph-server pair`。

## 两道护栏

- **登录墙**（`server::handler` + `handlers::connect::resolve_connect_auth` 4 级，
  `connect_authorized` 为无 device-mgr 时的 legacy 回退）：远程未授权连接只能发 `connect`；
  授权（loopback 或有效凭据）= operator 全权，与本地一致。**审计**：远程失败连接记
  `AuditEventType::AuthFailure`，flood-guard 关连接记 `RateLimited`，入
  `SecurityAuditLog`（专用 drain，`start/mod.rs`，与 guardrail 解耦）；loopback 永不审计
  （`connect::should_audit_connect_failure` 守）。⚠️ **地雷（新 dispatch 路径必须过
  process_request）**：admin 闸（`method_admin.rs`）和 `CALLER_ROLE`/`CALLER_USER`/
  `CALLER_IS_LOOPBACK` 的 task-local scope 都住在 `process_request` **周围**（两个 WS 派发
  站点 `do_lane_dispatch` 与幂等 `Proceed` 臂，各自把三个 task-local 包在
  `process_request(...)` 调用外层）——任何绕开 `process_request` 直接派发 RPC 的新路径
  （新 WS 帧类型、新内部快路径、新后台产地）都拿不到已解析的 `(user, role)`，也扫不到
  `method_requires_admin` 闸：对 admin 方法家族它是一条无身份旁路，对 member 专属逻辑
  它读到的 `current_caller_role()`/`current_caller_user()` 恒为 `None`（task-local 未
  scope）。新增派发路径必须复用 `process_request`，不得重新实现一遍它的解析+派发。
- **channel 工具闸**（`method_authz.rs::tool_requires_operator` + `tools/scoped/dispatch.rs`）：
  **仅治理 channel**（Telegram / Slack…）——`inbound_router` 按 `ChannelPermissionLevel`
  （默认 Chat ⇒ `guest`）盖 `caller_role`，禁 chat-tier channel 跑自配置类工具。Panel
  授权后恒 operator，此闸对 Panel 自然全过。
- **channel access / pairing 单一真源**：per-channel 的 `dm_policy` / `group_policy` /
  allowlist / pairing **由 `inbound_router::check_permission` + `pairing_store` 权威裁决**
  （非 channel 接口自持）。channel 配置经 `From<&*Config> for ChannelConfig` 桥接进 router
  并在 `start/builder/subsystems.rs` 注册（iMessage、Telegram 均已接）。⚠️ **地雷**：新增/改
  channel 若不桥接，router 退回 `ChannelConfig::default()`（DM `Pairing` / group `Open`）
  **静默忽略 operator 的策略配置**。Telegram 的接口侧 `access.rs` 只是配置化预过滤，
  `NeedsPairing` 转发 router（不自持 pairing 码）。
- **WS Origin 校验**（`origin_policy.rs`）：挡公网恶意网页跨源驱动 agent。域名部署须把
  origin 加进 `[gateway] allowed_origins`。

## P1 数据隔离地雷

> 详见 [SECURITY.md 多用户数据隔离层（P1）](../../docs/reference/SECURITY.md#multi-user-isolation-p1)。以下三条是本目录内、新代码最容易踩空的三处连线。

- **地雷 A（新 RPC 返回 scoped 数据必须注册 + 调谓词）**：任何新增的、响应内容依赖"谁在问"的 RPC 都要做两件事——① 在 `method_visibility.rs` 里登记它的执行形态（`KeyChecked` / `PartitionChecked` / `ListFiltered`），该文件自带的 pin 测试会在登记缺失或被删时报错；② 在 handler 内部实际调用 `visibility::session_visible` / `visibility::partition_visible` / `visibility::not_found_response` 之一——**手写一行 `meta.owner_user_id == caller` 的内联比较，就是这套模块存在的目的所要防的那种旁路**（判定逻辑分叉出第二个真源，且大概率漏掉 fail-closed / no-oracle 的细节）。`sessions.list`-形态的端点额外要把 `visibility::visible_owner_filter()` 塞进 `SessionFilter::owner_visible_to`，不能只在拿到结果后过滤——那样只是隐藏了列表，没有真正约束查询。
- **地雷 B（新 `GatewayEventFrame` 变体必须分类）**：任何新增的 `GatewayEventFrame` 变体都要在 `event_visibility::session_identity_of` 里显式分类为 `BySessionKey` / `ByRunId` / `Global` 之一——`every_frame_variant_is_classified` 那条 pin 测试对真实枚举做穷尽匹配、没有通配臂，编不过就是提醒你去做这个判断。**默认答案不是"先编译过再说"**：一个本该 session-scoped 的新变体如果被判成 `Global`，就是一次跨用户数据泄漏，而不是"以后再补"的功能缺口——`session_identity_of` 自己的模块文档把这句话写在最前面。
- **地雷 D（两个 surface 共用的数据，谓词要放在两者都必经的那条路上）**：`teams.*` 的所有权强制点是 **`TeamStore` 装饰器**（`src/teams/scoped.rs::ScopedTeamStore`），不是逐 handler 的检查——团队既能从 gateway 的 37 个 RPC 到达，也能从模型 run 中的 ~30 个 `team_*` 工具到达，把谓词写在 handler 里等于**只强制了 Panel 那一半，聊天那一半全开**（而且它不报错，测试全绿）。**装饰器只在唯一构造点包一次**（`builder::agent_init::coord_stores`），任何地方把裸 `SqliteTeamStore` 发布出去就是旁路。⚠️ 配套两条：① **resolver 是 `scope::ambient_owner()` 不是 `visible_owner_filter()`**——后者只读 `CALLER_USER`，而那个 task-local 在 spawn 出的 run 里恒 `None`，用它建的团队谓词对每一次工具调用都 fail-open；② **gateway 侧仍要显式调 `handlers::teams::visibility::{gate_team, gate_task}`**，不是重复判定，而是装饰器结构上做不到的两件事——把拒绝塑成与「真的不存在」逐字节相同的 `not_found` 响应，以及够到那 ~20 个**按 `task_id` 寻址**、要去 `coord_tasks`（另一个数据库）里解析的方法。按 `team_id` 枚举 handler 会把它们整类漏掉，这正是 P1 终审抓到 `agent.run` 的同一个形状。③ **工具侧有对位的六个**（`team_task_control` / `workflow_step_review` / `task_comment` / `task_exit_journal` / `task_submit` / `team_workflow_canvas`）——它们只经 `CoordTaskStore` 寻址，装饰器看不见，各自调 `teams::task_team_reachable`。**新增任何按 id 够到 coord task 的工具都欠这一句**；`team_workflow_canvas` 只读也照闸，因为它的 `export` 枚举整个团队的任务，正是给另外五个发 id 的那张脸。
- **地雷 C（新 `tokio::spawn` 的 run 工作必须重新播种 scope）**：`tokio::task_local!`（`scope::current_scope()`，以及 P0 的 `CALLER_USER`/`CALLER_ROLE`）**不会**跨越 `tokio::spawn` 边界——子任务里读到的永远是 `None`，不是父任务当时的值。任何新的、在 spawn 出的任务里会碰业务数据（记忆检索、会话写入、后台 goal/loop）的调用点，都必须在 `tokio::spawn(...)` **之前** `let captured = crate::scope::current_scope();`，再在 spawned 的 future 内部用 `crate::scope::with_scope(captured, ...)` 包一层——反面教材是 `src/agents/subagent_tool/spawn.rs::spawn_background`（这个函数本身已经修好：`captured_scope`/`captured_root`/`captured_agent` 在 spawn 前捕获、`with_scope`/`with_project_root`/`with_agent_id` 在 spawn 内重新建立），修复前它让后台 subagent 的记忆检索静默退回到无 owner 的 base 命名空间。新的后台产地（另一个 subagent 变体、新的后台 tool、新的 daemon 触发路径）复制这个形状，不要假设"内层调用会自己继承"。

## 红线

- 改认证 / 授权 / Origin 逻辑**必须同步更新测试**，不得只改实现。
- 不在 Gateway/Interface 层处理业务逻辑（R4：纯 I/O）。
