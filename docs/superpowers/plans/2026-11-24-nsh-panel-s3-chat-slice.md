# NSH Panel S3 实施计划 / Chat Vertical Slice Implementation Plan

> **Goal**: 同一份 TSX 源码在 NSH 桌面宿主（Desktop IPC）与浏览器（Web `/v1/ws`）完成"选会话 → 读历史 → 发送 → 流式回复 → 取消"，结果来自真实 Core，权限与旧 `/ws` 一致。
> One TSX source completes a real chat path on both hosts with legacy-equivalent authorization.
>
> **Spec**: [2026-11-24-nsh-panel-s3-chat-slice-design.md](../specs/2026-11-24-nsh-panel-s3-chat-slice-design.md)（Approved，含 §7a B1–B6）。
> **Worktree**: `.worktrees/nsh-panel-s3`，branch `nsh-panel-s3-chat`，基线 `b2c00e624`。main 不动。
>
> **Architecture**: 旧 `/ws` 内联事件准入抽为 `admit_event`（一份判定）；v1 两端在 `system.hello` 协商 `live_events` 后用同一判定推送，再以 `V1_EVENT_TOPICS` 精确收窄；`chat.*` 写方法加入 `V1_METHODS` 并恢复幂等缓存；客户端（`shared/client`、`panel_host`、`host.js`）把事件转为宿主事件 `aleph.event {topic,data}`；TSX SDK 增 `on()`，新 `Chat` 模块。
>
> **Tech stack**: Rust（tokio、axum、serde）、NSH TSX、Node test runner。
>
> **Global constraints**: R1/R2/R6（Core 唯一授权权威）；不为 v1 另写过滤或归属判定；未协商时 v1 不推送任何 id-less 帧；不宣告 replay；代码注释英文，回复中文，文档中英。
>
> **Review focus**: `admit_event` 迁移是否逐字等价；v1 推送是否只在协商后发生；`V1_EVENT_TOPICS` 只收窄；`chat.send` 幂等缓存在 v1 生效；撤销/到期在请求与事件两个方向都关闭连接。

## 偏差说明 / Deviation from spec

- §5.1 要求"迁移前先给内联逻辑写特征测试"。内联逻辑只能经完整 `/ws` 握手 + 鉴权才可达，逐条写端到端测试成本过高。改为：**逐字迁移**（diff 审阅只允许 `continue`→`return Skip`、`write.send`→`return Send/Close` 的机械替换），迁移后对 `admit_event` 直接写特征测试，并保留/新增一个旧 `/ws` 端到端用例证明旧循环仍走该函数。护栏强度：逐字 diff + 现有 gateway 测试全绿 + 新特征测试。

## Tasks

### T1 B1：模拟执行路径补会话归属检查
- Files: `src/gateway/handlers/agent.rs`（`AgentRunManager`）、`src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1648-1657`、`src/gateway/method_visibility.rs:156-164`。
- Interface: `AgentRunManager::with_session_store(self, Arc<dyn SessionStore>) -> Self`；`start_run` 在调用方提供 `session_key` 时先 `visibility::existing_session_is_visible`，不可见 → `Err(SESSION_NOT_VISIBLE)`；`handle_run`/`handle_send` 把该错误映射为 `visibility::not_found_response`。
- Steps: 失败测试（他人会话被拒、自己的会话与新会话放行）→ 实现 → 删除 carve-out 文字 → `cargo test` gateway::handlers::agent/chat → commit。

### T2 抽取 `admit_event`
- Files: 新 `src/gateway/server/connection/event_admission.rs`；`mod.rs` 事件分支。
- Interface: `pub(super) enum EventVerdict { Skip, Send(String), Close(&'static str) }`；`pub(super) async fn admit_event(ctx:&ConnectionContext, conn_id:&str, session_device_id:Option<&str>, event_json:String) -> EventVerdict`（`client_is_local` 取自 `ctx`）。
- Steps: 逐字迁移 → 旧循环只做 verdict 分派 → 特征测试（pty 帧、他人 `stream.*`、自己的 `stream.*`、guest 登录墙、无连接行、audience 不匹配、token-rotated 远程关闭/本地吞掉、device-revoked 自己/他人、不可解析帧默认转发）→ 旧 `/ws` 端到端一条事件到达 → commit。

### T3 协商、方法与主题、幂等
- Files: `shared/protocol/src/system_hello.rs`、`src/gateway/handlers/system_hello.rs`、`connection/mod.rs`、`local_ipc.rs`、`web_v1.rs`、`rpc.rs`。
- Interface: `Feature::LiveEvents`（`"live_events"`）；`negotiate(.., implemented:&[Feature])`；`V1_METHODS += chat.history/send/abort`；`V1_EVENT_TOPICS`；`fn v1_event_admitted(wire:&str)->bool`（按已生成的线上帧取 topic）；v1 去掉 `without_cache()`。
- Steps: 测试（协商、方法放行、主题集合）→ 实现 → commit。

### T4 Desktop IPC 与 Web v1 推送
- Files: `local_ipc.rs`（`serve_requests` 事件分支）、`web_v1.rs`（`socket.split()` + `select!`）、两个 tests.rs。
- Behavior: 仅协商后订阅 `event_bus`；`admit_event` → `V1_EVENT_TOPICS` → 端点帧上限（超限 → `overflow_warning_frame(1,total)` 保留连接）；`Lagged` → warning 后关闭；`Close` verdict → 结束连接。
- Steps: 测试（未协商无推送；协商后自己的 `stream.response_chunk` 到达、他人的不到达、不在白名单的主题不到达、超限变 warning）→ 实现 → commit。

### T5 客户端与桌面宿主
- Files: `shared/client/src/connection.rs`、`desktop/nsh-shell/src/panel_host.rs`。
- Behavior: `open_local` 接受 `features ⊆ requested`；local 读循环在协商 `live_events` 后把无 id notification 交给 `handle_message` 分流；`panel_host` 请求 `live_events`，保留 `_events` 与 topic 接收器，按 `link.attempt` 绑定转发任务，发 `aleph.event {topic,data}`；`stream_continuity().dropped_events` 增长 → 合成 `connection.stale`。
- Steps: client 单测（协商/未协商）→ panel_host 单测（事件序列化形状）→ `cargo test -p aleph-nsh-shell --features tsx-panel` → commit。

### T6 Web 宿主、SDK、Chat 模块
- Files: `interfaces/panel/web/host.js`(+test)、`shared/protocol/tests/fixtures/v1_frames.json`（hello 请求 `live_events`，重算 wire_hex）、`interfaces/panel/shell/{sdk.ts,sdk.test.ts,Shell.tsx}`、`interfaces/panel/modules/{Chat.tsx,Sessions.tsx}`、`interfaces/panel/src/lib.rs`（`SHELL_MODULES` 加 `aleph:chat`）、`App.test.tsx`。
- Behavior: host.js 对无 id 帧发 `aleph.event`；SDK `on(prefix, listener)` 共用 release/代际门控；Chat：history → send（UUID 幂等键）→ 按 `run_id` 追加 `delta||content` → 完成/失败重取 history；Stop → `chat.abort`；stale/warning/代际变化 → 标记并重取；IME 组合中不提交；应答丢失 → "结果未知"。
- Steps: 测试 → 实现 → `just panel-tsx-test`、`node --test interfaces/panel/web/host.test.mjs` → commit。

### T7 真机验收与回执
- Files: `qa/nsh_s2_shell/`（扩展或新 `qa/nsh_s3_chat/`），`docs/superpowers/runs/2026-11-24-nsh-panel-s3-chat-receipt.{md,json}`。
- macOS 隔离 Gateway（无模型配置 → 模拟执行，或已配置模型）：两端发送、流式、Stop、IME、Gateway 重启 stale；逐项写已验证/未验证。

### T8 文档
- `docs/reference/{DESKTOP_SHELL,FEATURE_LOCATOR,SUBSYSTEM_ROUTING}.md`、`docs/research/NSH_PROTOCOL_V1.md` 中 v1 只读表述、路线图 S3 状态；随后通读 FEATURE_LOCATOR 与参考文档做校对（m00001 遗留目标）。

## Self-review

- 覆盖 spec §5.1–§5.5、§7、§7a 全部条目；B2（重新配对）不需代码，B4 为已知限制写入回执。
- 不新增 registry/全局；`V1_EVENT_TOPICS` 与 `V1_METHODS` 同文件同形态。
- 风险：Web v1 循环重写会改变请求时序——保持 `max_inflight_requests: 1`，现有 `web_v1/tests.rs` 全绿为门槛。
