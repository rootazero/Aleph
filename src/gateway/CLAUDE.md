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

> 详见 [SECURITY.md 多用户数据隔离层（P1）](../../docs/reference/SECURITY.md#multi-user-isolation-p1)。以下逐条是本目录内、新代码最容易踩空的连线（字母是标签不是顺序，**别在这里维护一个条数**——上一版写"三条"而实际列了四条）。

- **地雷 A（新 RPC 返回 scoped 数据必须注册 + 调谓词）**：任何新增的、响应内容依赖"谁在问"的 RPC 都要做两件事——① 在 `method_visibility.rs` 里登记它的执行形态（`KeyChecked` / `PartitionChecked` / `ListFiltered`），该文件自带的 pin 测试会在登记缺失或被删时报错；② 在 handler 内部实际调用 `visibility::session_visible` / `visibility::partition_visible` / `visibility::not_found_response` 之一——**手写一行 `meta.owner_user_id == caller` 的内联比较，就是这套模块存在的目的所要防的那种旁路**（判定逻辑分叉出第二个真源，且大概率漏掉 fail-closed / no-oracle 的细节）。`sessions.list`-形态的端点额外要把 `visibility::visible_owner_filter()` 塞进 `SessionFilter::owner_visible_to`，不能只在拿到结果后过滤——那样只是隐藏了列表，没有真正约束查询。
- **地雷 J（一条连接有两个方向，闸只挂在其中一个上）**：登录墙 `wall_admits` 直到 2026-08-08 只在**请求臂**被求值。事件转发臂的判决是 `scope_allowed && audience_allows && should_receive && event_admits` —— 四项，没有一项是身份验证，而且对一条**从未授权过**的连接四项全真：`ConnectionState` 在 socket 被 accept 的那一刻就写进 `ctx.connections`（`permissions: []` / `caller_user: None` / `caller_role: "guest"`）；`can_receive` 放行任何没有规则点名的 topic；`should_receive` 在连接**根本没注册 filter** 时恒真（`None => true`）；`event_admits` 在 `Global` 上**先于**读 `caller_user` 就短路。`pty.output`（子进程写给 operator 终端的每个字节的 base64）正是这样一个 topic ⇒ **一条什么都没发过的局域网 WebSocket 收得到 operator 的 shell**。现 `wall_admits` 是第 0 项、读同一个 `caller_role` 字段，所以 `restamp_live_connections` 一次关掉两个平面。**判据：授权谓词属于连接携带数据的每一个方向，不只是调用者提问的那个方向。** 新增任何"把帧发给客户端"的路径时先问：这条路上有身份项吗？
- **地雷 K（`EventScopeGuard` 按 topic 前缀键控，所以一个前缀下装着两类帧时它只能一次答完两者）**：`approval.` 家族同时装着**工具门审批**（指名被阻塞的会话）与**集群节点审批**（`node_requester` 发的 `session_key: String::new()`）。整族判 admin 的后果不是"约束了 member"，而是**把他推过了门**：他自己的 run 卡在一道他既看不见也答不了的确认上，死于 120s 超时，而记录在案的解法是 `exec_tier:"full"` —— 最不安全的档位成了唯一能用的档位。现该前缀在这张表里**没有规则**（`surface.approval` 仍有），判据下沉到 `event_visibility::session_identity_of`：有会话键 ⇒ `BySessionKey`，空 ⇒ `OperatorOnly`。**别把 `approval.` 的前缀规则加回来**——那会在舰队那半毫无察觉的情况下重新关上 member 那半。同族提问：**新加一道闸时问"被闸住的人接下来会干什么"，不只问"这道闸拦住了什么"。**
- **地雷 B（新 `GatewayEventFrame` 变体必须分类）**：任何新增的 `GatewayEventFrame` 变体都要在 `event_visibility::session_identity_of` 里显式分类为 `BySessionKey` / `ByRunId` / `Global` 之一——`every_frame_variant_is_classified` 那条 pin 测试对真实枚举做穷尽匹配、没有通配臂，编不过就是提醒你去做这个判断。**默认答案不是"先编译过再说"**：一个本该 session-scoped 的新变体如果被判成 `Global`，就是一次跨用户数据泄漏，而不是"以后再补"的功能缺口——`session_identity_of` 自己的模块文档把这句话写在最前面。
  ⚠️ **`SessionIdentity` 现在有六个变体，新增三个都有各自的用途，别混**：`ByUserId`（帧属于**一个人**而不是一场会话——语音转写不是对话）、`OperatorOnly`（舰队级、没有 owner 可比；**用于"没有归属可解"，不是"解起来麻烦"**）、`Unattributed`（**该有戳而没有** ⇒ 对 scoped caller 拒、对无 scope 进程放行）。`Unattributed` **刻意不并进 `Global`**：`Global` 的意思是"所有人都可以有"，把一个没盖戳的帧折进去，正是"缺失的生产者变成一次广播"的机制本身。
  ⚠️ **但这条 pin 只看得见"有变体"的帧**：`publish_team_event`（`team.<id>.{message,activity,system,fanout}`）、`CoordTaskStore::emit_task_topic`（`team.<id>.task.<verb>`）与 `voice::streaming::relay` 的 `voice.transcribe.delta` 发的是**裸 `{topic,data}` 字符串**，没有任何 `GatewayEventFrame` 变体，所以那条穷尽匹配对它们**结构性失明**——`team.*` 整个事件平面就是这么在 `Global` 上待了一整程（每个连着的用户都收到别人团队的聊天**正文**）。新增任何裸字符串 topic 生产者时要做两件事：① 在 `session_identity_of` 里分类它；② 给它一条**源码级** pin（模板见 `event_visibility.rs::no_published_team_topic_suffix_classifies_as_global`，读生产者源码里的 suffix 字面量）。且分类判据要**结构化**（"`team.` 前缀 + 非空 id + 任意后缀"），不要写后缀白名单——白名单只覆盖立法当天的世界，下一个新后缀会重新掉回广播路径。topic 语法的单一源是 `aleph_protocol::team_topic`（Panel 的渲染路由同源）。
- **地雷 D（两个 surface 共用的数据，谓词要放在两者都必经的那条路上）**：`teams.*` 的所有权强制点是 **`TeamStore` 装饰器**（`src/teams/scoped.rs::ScopedTeamStore`），不是逐 handler 的检查——团队既能从 gateway 的 37 个 RPC 到达，也能从模型 run 中的 ~30 个 `team_*` 工具到达，把谓词写在 handler 里等于**只强制了 Panel 那一半，聊天那一半全开**（而且它不报错，测试全绿）。**装饰器只在唯一构造点包一次**（`builder::agent_init::coord_stores`），任何地方把裸 `SqliteTeamStore` 发布出去就是旁路。⚠️ 配套两条：① **resolver 是 `scope::ambient_owner()` 不是 `visible_owner_filter()`**——后者只读 `CALLER_USER`，而那个 task-local 在 spawn 出的 run 里恒 `None`，用它建的团队谓词对每一次工具调用都 fail-open；② **gateway 侧仍要显式调 `handlers::teams::visibility::{gate_team, gate_task}`**，不是重复判定，而是装饰器结构上做不到的两件事——把拒绝塑成与「真的不存在」逐字节相同的 `not_found` 响应，以及够到那 ~20 个**按 `task_id` 寻址**、要去 `coord_tasks`（另一个数据库）里解析的方法。按 `team_id` 枚举 handler 会把它们整类漏掉，这正是 P1 终审抓到 `agent.run` 的同一个形状。③ **工具侧有对位的六个**（`team_task_control` / `workflow_step_review` / `task_comment` / `task_exit_journal` / `task_submit` / `team_workflow_canvas`）——它们只经 `CoordTaskStore` 寻址，装饰器看不见，各自调 `teams::task_team_reachable`。**新增任何按 id 够到 coord task 的工具都欠这一句**；`team_workflow_canvas` 只读也照闸，因为它的 `export` 枚举整个团队的任务，正是给另外五个发 id 的那张脸。④ **第三种寻址形状：fan-out 树的 `run_id`，两个 store 都解不出**（`teams.chat.cancel`）。它一度靠一句论证活着——"run id 是不可猜的 capability，调用者只可能从自己的 `teams.chat.send` 响应、或一条他本来就有权收到的 `team.<id>.fanout` 事件里拿到"——而那个"本来就有权"是**另一个子系统的不变量**，且当时是假的（`team.*` 整个平面在广播）。现 `register_fanout`（树 run id 的**唯一**铸造点）记 `run_id → team_id` 进有界索引（`teams::broadcast::team_of_fanout_run`），handler 解析后走同一个 `gate_team`，拒绝形状与未知 id 逐字节相同。**判据**：写下"这个 id 拿不到所以不用闸"之前，先去**那个发 id 的面**上验证它——一道闸如果实际上是另一个子系统的不变量，它不是闸，而且它会在那个子系统被改动时无声失效。
- **地雷 H（分类器要的解析句柄，必须挂在不比"帧的生产条件"更窄的条件上）**：把一类帧从 `Global` 收成"要解析才知道给谁"，就等于给投递面新加了一个**运行时依赖**——而那个依赖的安装点通常在另一个文件、跟另一个子系统的初始化捆在一起。判据一句话：**grep 出这类帧的生产者是在什么条件下被注册的，再 grep 出解析句柄是在什么条件下被注入的，两个条件必须一样宽**；窄了就是 fail-closed 方向上的**全量功能静默熄灭**（拒绝路径通常连一行日志都没有，因为拒绝是常态）。教科书实例（**已修**）：`server.set_team_store` 一度挂在 `team_store && coord_task_store` 上，而 `teams.chat.send` 的注册（`publish_team_event` 的产地）只要 `team_store`；两个 store 开的是两个数据库（`teams.db` / `coord.db`），`coord.db` 打不开是一条**明确支持的** warn-and-continue 降级启动。于是那条降级路径下，团队群聊照常发帧、每一条连接（含 operator）全部拒收，没有任何一处报错。现已把 `set_team_store` 挪出双 store 的 tuple、**单挂 `team_store`**（`register_teams_handlers` 留在原处，它确实两个 store 都要），并由源码级 pin `event_visibility.rs::the_team_resolver_gate_is_no_narrower_than_its_frame_producers_gate` 钉住：它抓出两处调用点各自最近的那道 `if let` 门，断言**解析侧命名的 `*_store` 集合是生产侧的子集**，任何重新收窄都是红。**同族提问**：新加的解析依赖有没有可能在合法降级启动下缺席？缺席时被解析的那类东西，谁还在继续生产？
- **地雷 I（一帧若既不能整份放行也不能整份拒绝，它是要被"投影"的——而投影欠两条不变量 + 一个孪生）**：`stream.running_set_changed` 的载荷是一个跨全体用户的数组，pass/fail 对它就是错的问题；答案是保持 `Global`、由 `EventVisibilityIndex::project_for` 逐连接**收窄载荷**（`Option<Value>`，一条 arm 一个消费者，**刻意不建 trait / registry / Delivery 枚举**）。写第二条投影 arm 前先过三关：① **数组空了也必须发**——客户端按 `seq` 单调丢弃旧帧，压掉一帧就把陈旧状态焊死一整条连接（红点亮着不灭）；② **解不出归属的元素丢弃，不放行**——"我算不出这是谁的"绝不能读作"是大家的"（注意这条与 `existing_session_is_visible` 方向**相反**：那条答"我能不能寻址这个键"，还没创建的会话是新对话的正常第一句、必须放行；这条答"能不能告诉我这个键存在"，算不出来就不能说）；③ **同一集合的每一个生产者都要同批同规则**——这里的第二个生产者是 `gateway.metrics.run_concurrency`（红点的冷启动种子），只修事件面等于没修。⚠️ **投影插在 `params` 上这件事依赖被投影帧仍然有 `stream_method()`**：一旦那个帧退化成裸 `{topic,data}`，投影会落进一个**新的** `params` 键、原始未收窄数组从 `params.data` 原样上线，而所有测试保持绿——改动这个帧的 stream 属性时，欠一条源码级 pin，不是一句注释。⚠️ **投影和"帧什么时候被发出"是耦合的**：`try_claim` 在会话行写盘之前就广播，于是新会话的第一回合解不出归属、被丢弃，而那一帧**同时把 seq 花掉了**——单个在飞 run 的下一帧是 run 结束时的 release（同样不含该键），红点整个第一回合不亮。修法是**给那句"下一帧会自愈"补一个生产者**，不是放宽丢弃规则：`execute.rs` 在 `ensure_session` 之后调 `SessionRunRegistry::republish_running_set()`，以**更高的 seq** 重发同一集合（客户端丢 `seq <=` 旧值的帧，沿用旧 seq 等于没修）。**加投影时要问"这个 id 在这一刻已经能被解析了吗"**，而不是只问"解析规则对不对"。
- **地雷 L（谓词的 actor 取法有两种，工具面只有一种能用）**：`visibility::partition_visible` / `session_visible` / `project_visible` 经 `visible_owner_filter()` 读 `CALLER_USER` —— 而那个 task-local 在 spawn 出的 run 里是**死的**，且**每一次工具调用都在里面**。于是一个照着文档去接现成谓词的工具作者会拿到**静默恒真**：不报错、不告警、单测全绿，只在多用户真机上是个洞。故每个谓词都有显式 actor 孪生（`partition_visible_to` / `project_visible_to` / `session_visible_to`），**工具一律传 `scope::ambient_owner()`**（它先读 gateway 身份、再退回 run 播种的 `ScopeAttribution`）。2026-08-08 这一条同时解释了五个缺陷：`session_list` 的 `SessionFilter` 用 `..Default::default()` 留下 `owner_visible_to: None`（= 全体 owner，而 RPC 孪生一直设着它）、`session_send` 按模型给的 key **派 run**、`memory_search{cross_workspace}` 扇出磁盘上每一个分区、以及两个 coord task 工具。**判据：新工具碰到 per-user/per-room 数据时，先问"这个谓词的 actor 是从哪来的"，`CALLER_USER` 这个答案在工具面永远是错的。**
- **地雷 M（fail-closed 的答案被当成值消费，会反转成许可）**：`ScopedTeamStore` 对不属于调用者的团队返回 `Ok(None)` —— 这是**拒绝**。`task_review` 把它折进 `leader = get_team(...).ok().flatten().map(|t| t.leader_id)`，再交给 `is_authorized(caller, leader) = leader.is_none_or(...)` ⇒ **"没有 leader，所以谁都能审"**，于是任何调用者都能把别人的 coord task 翻成 Completed、解锁它的依赖、往里写 feedback。**闸要跑在折叠之前，不能跑在里面**：`Ok(None)` 一旦和"这个任务本来就没有团队"合流，两者就再也分不开了。同族检查：任何 `.ok().flatten()` / `unwrap_or_default()` 落在一个**装饰器**的返回值上，都要问一句"这个默认值和那个拒绝长得一样吗"。
- **地雷 C（新 `tokio::spawn` 的 run 工作必须重新播种 scope）**：`tokio::task_local!`（`scope::current_scope()`，以及 P0 的 `CALLER_USER`/`CALLER_ROLE`）**不会**跨越 `tokio::spawn` 边界——子任务里读到的永远是 `None`，不是父任务当时的值。任何新的、在 spawn 出的任务里会碰业务数据（记忆检索、会话写入、后台 goal/loop）的调用点，都必须在 `tokio::spawn(...)` **之前** `let captured = crate::scope::current_scope();`，再在 spawned 的 future 内部用 `crate::scope::with_scope(captured, ...)` 包一层——反面教材是 `src/agents/subagent_tool/spawn.rs::spawn_background`（这个函数本身已经修好：`captured_scope`/`captured_root`/`captured_agent` 在 spawn 前捕获、`with_scope`/`with_project_root`/`with_agent_id` 在 spawn 内重新建立），修复前它让后台 subagent 的记忆检索静默退回到无 owner 的 base 命名空间。新的后台产地（另一个 subagent 变体、新的后台 tool、新的 daemon 触发路径）复制这个形状，不要假设"内层调用会自己继承"。
  ⚠️ **地雷 C2（会话行的 owner/scope 戳，曾对每一条运行路径都为空——2026-08-08 真机 QA）**：`SessionMetadata::stamp_attribution` 在 `get_or_create` 的 CREATE 分支读 `scope::current_scope()`，而**六个产地（Panel `handlers::agent`、channel inbound router、cron、heartbeat、teams dispatcher、`sessions_send`、A2A）无一例外把请求交给一个 spawn 出的任务**。归属被 `build_run_request` 正确算出并 `stamp_metadata` 进了 `request.metadata`，但 task-local 不跨 spawn，于是 `execute()` 里的 `agent.ensure_session()` 建行时 scope 恒 `None`——**每一个会话行的 `owner_user_id`/`scope_id` 都落成 NULL**，被"缺省即 owner"收养成 owner 的。症状全在 member 那一侧且看起来像"功能没做"：`sessions.list` 空、`sessions.set_topic` 自己的会话报 not found、`chat.context_estimate` 自己的会话返回 null，而 transcript 被记在 operator 名下。修法是 `run_loop::ensure_session_under_request_scope`（**读 metadata 不是捕获 task-local**——`current_scope()` 在 gateway 派发循环里同样是 `None`，归属只存在于 metadata 里；一个 helper 两个引擎共用，别在两处各抄三行）。
  ⚠️ **两条推论，比这个 bug 本身更值钱**：① **下游的谓词会跟着一起哑掉。**`event_visibility` 的房间投递读的正是这两列，所以房间直播面对每个成员静默失效——把 `scope_id` 手工置回 NULL 复现，member 收到 **0** 帧而 operator 收到 9 帧。**修一处会让它下游从没跑过的路径第一次真正跑起来**，那不是回归。② **fixture 会制造生产从未产生过的状态。**那些谓词的单测全绿，因为测试自己调 `stamp_owner(...)` 把行填好了——**测试里手工构造的前置状态，必须回头问一句"生产里谁写它"**，否则守卫守的是产地不是连线。

## P2 项目房间地雷

> 详见 [SECURITY.md 项目房间层（P2）](../../docs/reference/SECURITY.md#project-rooms-p2)。以下逐条是本目录内、新代码最容易踩空的连线（字母是标签不是顺序，**别在这里维护一个条数**——P1 那节已经因为数错一次了）。

- **地雷 E（房间的可见性判据是名册，不是 owner 列）**：`projects` 行的 `owner_user_id` 记的是**创建者**，只用于 owner-only 动词（rename / archive / roster / bind_workspace）；「谁能看」一律问名册（`projects::roster::is_member`，经 `visibility::project_visible` / `session_visible_to` / `partition_visible` 到达）。拿 owner 列答 can-see 会把每个成员都判成外人（或反过来）。拒绝形状有分界：**看不见 → `gate_project` 给逐字节相同的 `not_found`**（存在性即泄漏）；**看得见但角色不够 → `require_owner` 给诚实的 `PERMISSION_DENIED`**（他已知道房间存在，forbidden 不泄漏且可行动）。新的 `projects.*` 动词先问自己落在哪一侧。
- **地雷 F（`workspace_path` 的写入者与读取者共用一道闸）**：绑定目录是房间会话的默认 cwd，所以**每一个能选目录**的写 `workspace_path` 动词（`projects.add` / `create_blank` / `bind_workspace`，以及未来任何新写入者）都必须过 `caller_identity::caller_may_choose_directory()`——与 `agent.run` 的显式 `project_root` 同一个谓词。漏一个写入者就是「两步都合法、合起来等价」的绕闸路径（先注册目录、再进房间聊天）。解绑是降权，刻意豁免。成员**使用**绑定不需要闸——目录是 owner 经闸选的，成员只是继承。⚠️ **写入者一共四个，不是三个**：第四个是 `execution_engine/run_loop/inner.rs` 的目录簿自动登记（`ProjectStore::add_for`），它**不过闸**且这是不变量而非疏漏——**它从不引入目录，只登记本 run 已经在其中执行的那个 cwd**，换不到任何新可达目录。于是授权问题上移到**设置 `workspace_override` 的那个生产者**（过闸的 `project_root` 参数 / 闸后写下的房间绑定 / channel 配置的 `default_workspace` / resume 或继承而来的工作区——**别把这串读成穷举**）。规矩因此有两半：**新增会「选目录」的 `workspace_path` 写入者要过 `require_directory_choice`；新增 `workspace_override` 的来源要在它自己的选目录处过闸**——这一行会忠实登记它产出的任何东西。census 全文与豁免推导在 `handlers/projects.rs` 模块 doc，第四个写入者处有回指注释。
- **地雷 G（会话入房后 scope 是 tier-1 事实，别读 `params.project_id`）**：`resolve_attribution` 的第一优先级是会话**已存储**的 scope——Panel 只在开房那一回合发 `project_id`，之后每回合都不带。任何按房间分派的新逻辑（默认 cwd、记忆路由、名册谓词）都必须读**已解析的** `ScopeAttribution`，读请求参数会让第一条消息生效、之后每一条静默退回默认。同族：`run_loop/inner.rs` 的目录簿写入对房间回合要走 scope 短路（按 `project_id` `touch`），不能走 owner-keyed 路径查找——否则每个成员每回合都会查空并给共享文件夹注册一行重复的个人项目。
- **地雷 H（`ambient_owner()` 在房间里是**房主**不是说话人——这句话此前在仓里被写反了三处）**：`scope::ambient_owner()` = `CALLER_USER`（跨 spawn 即死）→ 回退到**run 的 scope owner**，而房间会话行的 `owner_user_id` 记的是**创建者**，对每个成员**都是同一个人**（这正是他们共享一个记忆分区的机制）。所以任何"这个人能看什么"的工具面谓词都必须用 `visibility::ambient_actor()`（= `scope::ambient_room_author()` 优先、否则 `ambient_owner()`；非房间恒等于旧行为，因为 `room_author` 在非 `Project` scope 上返回 `None`）。**四个谓词各自独立地踩了同一个坑**（`memory_search` 的分区闸、`session_list` 的 `owner_visible_to`、`session_send` 的寻址闸、`ScopedTeamStore` 的 `team_visible`），全部是因为作者 grep 先例时读到了写反的那句话。判据一句话：**问"这一行属于谁"用 `ambient_owner`，问"谁在问"用 `ambient_actor`**。

## 红线

- 改认证 / 授权 / Origin 逻辑**必须同步更新测试**，不得只改实现。
- 不在 Gateway/Interface 层处理业务逻辑（R4：纯 I/O）。
