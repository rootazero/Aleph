# NSH Desktop Server RPC Connection Plan
# NSH Desktop 服务端 RPC 接入计划

**Decision / 决策:** Proceed with real Gateway RPC using Aleph’s existing canonical methods; keep a mock implementation injectable for tests. Do not add compatibility aliases for the stale `message.*` / `session.start` names.

使用 Aleph Gateway 已有的规范 RPC 接入真实服务端；保留可注入的 mock 供测试使用。不要为过时的 `message.*` / `session.start` 名称额外增加兼容别名。

## Status

**Functional capability: ready. Exact assumed method set: not ready / does not exist.** Session creation, chat submission, history retrieval, and streaming are all implemented in production, but the Phase 3 document uses names and response shapes that do not match the server contract. Therefore choose **Option A with a contract correction**: integrate real RPC through `AlephClient`, rather than treating RPC as unavailable or implementing a parallel API.

**功能已具备；假定的 RPC 名称并不准确。** 生产 Gateway 已实现创建会话、发送对话、读取历史和流式输出，但 Phase 3 文档里的方法名与响应结构并非服务端契约。因此选择**方案 A（按实际契约接入）**：通过 `AlephClient` 连接真实 RPC，而不是因名称不匹配就认定服务端不可用，也不新造一套并行 API。

| Phase 3 assumed method / Phase 3 假定方法 | Production contract / 生产契约 | Status / 状态 |
|---|---|---|
| `session.create` → `{ session_id }` | `session.create` → `{ session_key, name }` | Implemented and registered; response field differs. / 已实现并注册，但响应字段不同。 |
| `message.send` or `session.start` | `chat.send` starts a run; accepts `input` (also `message` alias), optional `session_key`, and `stream`; returns `{ run_id, session_key, streaming }`. | Exact assumed names absent; equivalent operation is implemented and registered. / 假定名称不存在；等价操作已实现并注册。 |
| `message.list` | `chat.history` (and separately `sessions.history`) | Exact name absent; chat history is implemented and registered. / 假定名称不存在；对话历史已实现并注册。 |
| `message.receive` / `events.subscribe` | No `message.receive` RPC. Subscribe with `events.subscribe`; server streams typed `stream.*` notifications, decoded by `AlephClient` as `StreamEvent`. | `events.subscribe` is implemented in the Gateway connection dispatch; `message.receive` is absent. / Gateway 连接分派实现了 `events.subscribe`；不存在 `message.receive`。 |

The actual expected list in the Phase 3 plan is `session.create`, `message.send`, `message.list`, `events.subscribe` (`docs/superpowers/plans/2026-11-05-nsh-desktop-phase3-ui-core.md:389-399`). The stale technical-debt note says those APIs are not implemented (`docs/superpowers/plans/2026-11-05-nsh-desktop-phase3-technical-debt.md:74-101`); this investigation found the functional equivalents and contract mismatch instead. That note should be corrected when its owner updates the Phase 4 backlog; it is not changed by this task.

Phase 3 计划实际列出 `session.create`、`message.send`、`message.list`、`events.subscribe`（`docs/superpowers/plans/2026-11-05-nsh-desktop-phase3-ui-core.md:389-399`）。Phase 3 技术债文档称这些 API 尚未实现（`docs/superpowers/plans/2026-11-05-nsh-desktop-phase3-technical-debt.md:74-101`）；本次调查发现的实际情况是对应功能已存在，但契约名称与字段不同。该旧文档应由 Phase 4 backlog 的维护者后续更正，本任务不改它。

## Evidence from the current server

- `session.create` is registered against `handle_create_db` in production (`src/bin/aleph-server/commands/start/builder/handlers/session.rs:57-61`). The handler accepts an optional `name` and returns `session_key` and `name`, not `session_id` (`src/gateway/handlers/session/db_handlers/create.rs:11-47`).
- `chat.send` is registered in the production agent initialization path (`src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1126-1154`). It invokes the run manager and returns `run_id`, resolved `session_key`, and `streaming` (`src/gateway/handlers/chat.rs:30-43,185-225`). Its input is `AgentRunParams.input` (with `message` as a serde alias), with optional `session_key`; streaming defaults to true (`src/gateway/handlers/agent.rs:35-57`).
- `chat.history` is registered unconditionally in production (`src/bin/aleph-server/commands/start/builder/agent_init/common_handlers.rs:193-203`). It requires `session_key`, accepts `limit` and `before`, and returns `messages` with additional snapshot fields such as `count`, `total`, `active_run`, and `pending` (`src/gateway/handlers/chat.rs:62-76,595-616`).
- `events.subscribe` is dispatched by the live Gateway connection handler (`src/gateway/server/connection/mod.rs:569-570`). `AlephClient::connect` already returns a typed `mpsc::Receiver<StreamEvent>` alongside the connected client (`shared/client/src/connection.rs:330-336`); the Phase 2 NSH connection already calls `events.subscribe` and `sessions.list` (`desktop/nsh-shell/src/connection.rs:139-151`), but currently discards received events (`desktop/nsh-shell/src/connection.rs:160-168`).
- The reusable RPC transport already exists: `AlephClient::call` / `call_with_timeout` serialize typed JSON-RPC requests and deserialize typed results (`shared/client/src/connection.rs:810-835`). Do not implement another WebSocket or JSON-RPC stack.
- Phase 3’s `SessionManager` is explicitly a local mock: it manufactures a UUID session ID, returns an empty history, and synthesizes assistant output (`desktop/nsh-shell/src/session.rs:67-89,98-125`).

- `session.create` 在生产环境注册为 `handle_create_db`（`src/bin/aleph-server/commands/start/builder/handlers/session.rs:57-61`）。处理器接收可选 `name`，返回 `session_key` 和 `name`，而不是 `session_id`（`src/gateway/handlers/session/db_handlers/create.rs:11-47`）。
- `chat.send` 在生产 agent 初始化路径注册（`src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1126-1154`）。它调用 run manager，并返回 `run_id`、解析后的 `session_key` 和 `streaming`（`src/gateway/handlers/chat.rs:30-43,185-225`）。输入字段为 `AgentRunParams.input`（serde 别名兼容 `message`），可携带 `session_key`；流式输出默认开启（`src/gateway/handlers/agent.rs:35-57`）。
- `chat.history` 在生产环境无条件注册（`src/bin/aleph-server/commands/start/builder/agent_init/common_handlers.rs:193-203`）。它要求 `session_key`，支持 `limit` 和 `before`，返回 `messages` 及 `count`、`total`、`active_run`、`pending` 等快照字段（`src/gateway/handlers/chat.rs:62-76,595-616`）。
- `events.subscribe` 由运行中的 Gateway 连接处理器分派（`src/gateway/server/connection/mod.rs:569-570`）。`AlephClient::connect` 已同时返回已连接客户端和类型化的 `mpsc::Receiver<StreamEvent>`（`shared/client/src/connection.rs:330-336`）；NSH Phase 2 已调用 `events.subscribe` 与 `sessions.list`（`desktop/nsh-shell/src/connection.rs:139-151`），但目前丢弃收到的事件（`desktop/nsh-shell/src/connection.rs:160-168`）。
- 可复用的 RPC 传输已存在：`AlephClient::call` / `call_with_timeout` 负责 JSON-RPC 请求序列化和类型化响应反序列化（`shared/client/src/connection.rs:810-835`）。不要再实现一套 WebSocket 或 JSON-RPC 栈。
- Phase 3 的 `SessionManager` 明确是本地 mock：生成 UUID 会话 ID、返回空历史、模拟 assistant 回复（`desktop/nsh-shell/src/session.rs:67-89,98-125`）。

## Proposed client architecture

Add a small `AlephRpcClient` adapter in NSH Desktop over `Arc<AlephClient>`; use the already-running connection supervisor as the authenticated connection owner. Keep session/view state in `SessionManager`, and inject a narrow local session API/port implemented by both `AlephRpcClient` and the existing mock. This is an app-level seam, not a new general RPC framework or a presumed NSH framework `SessionService` trait.

在 NSH Desktop 中新增一个轻量 `AlephRpcClient`，包装 `Arc<AlephClient>`；认证连接仍由现有 connection supervisor 所有。会话/视图状态仍由 `SessionManager` 管理，并注入一个窄的本地 session API/port，由 `AlephRpcClient` 与现有 mock 分别实现。这是应用层接缝，不是新建通用 RPC 框架，也不假定 NSH framework 自带 `SessionService` trait。

Suggested operations and mapping:

建议的操作及映射：

1. `create_session(name?)` → `client.call("session.create", { name })`; store returned `session_key` as the authoritative conversation identifier.
2. `send_message(session_key, content)` → `client.call("chat.send", { session_key, input: content, stream: true })`; use returned `run_id` to track the run. The server response is run acceptance, not a generated assistant message.
3. `load_history(session_key, limit?, before?)` → `client.call("chat.history", ...)`; map the server’s `messages` into NSH display models. Treat the extra active/pending/session snapshot fields as data, not as a second source of transcript truth.
4. Stream events through the typed receiver already returned by `AlephClient::connect`. Project `RunAccepted`, `ResponseChunk`, `RunComplete`, and `RunError` into UI state, keyed by `run_id` / `session_key`; do not add `message.receive`.
5. Reuse the current connection’s `events.subscribe` call. The connection owner must expose the client handle and forward the typed event receiver to the session/UI state owner. On reconnect, re-subscribe and fetch `chat.history` before treating subsequent stream deltas as a complete view.

1. `create_session(name?)` → `client.call("session.create", { name })`；将返回的 `session_key` 作为权威会话标识。
2. `send_message(session_key, content)` → `client.call("chat.send", { session_key, input: content, stream: true })`；用返回的 `run_id` 跟踪执行。服务端响应表示 run 已受理，不是 assistant 消息正文。
3. `load_history(session_key, limit?, before?)` → `client.call("chat.history", ...)`；将服务端 `messages` 映射到 NSH 展示模型。额外的 active/pending/session 快照只作为状态数据，不另建一套 transcript 权威。
4. 使用 `AlephClient::connect` 已返回的类型化 receiver 接收流事件。将 `RunAccepted`、`ResponseChunk`、`RunComplete`、`RunError` 投影到 UI 状态，并按 `run_id` / `session_key` 关联；不要增加 `message.receive`。
5. 复用现有连接中的 `events.subscribe` 调用。连接 owner 需要将 client handle 与类型化事件 receiver 转交给 session/UI 状态 owner。重连后重新订阅并读取 `chat.history`，之后才将新的 stream delta 视为完整视图。

### Integration sequence

### 接入步骤

1. Introduce the narrow mockable session port and retain current mock behavior behind it; add contract tests before switching the production path.
2. Add `AlephRpcClient` using existing `AlephClient::call`; use explicit request/response structs for `session.create`, `chat.send`, and `chat.history`. Avoid passing untyped `Value` through UI-facing state.
3. Adjust connection/runtime ownership to make the authenticated client available to the session port and to route `StreamEvent`s to the session projection. Keep reconnect/backoff and auth policy in the current supervisor.
4. Replace mock-only operations in `SessionManager`; remove generated fake assistant responses from the real path. On accepted send, track the run and reconcile canonical history at completion or after any event gap.
5. Add a local Gateway integration test, then run real-server QA using the existing approved connection/auth setup.

1. 引入窄且可 mock 的 session port，将当前 mock 行为保留在该 port 后；切换生产路径前先添加契约测试。
2. 基于已有 `AlephClient::call` 实现 `AlephRpcClient`；为 `session.create`、`chat.send`、`chat.history` 使用显式请求/响应结构，避免 UI 状态层传递无类型的 `Value`。
3. 调整 connection/runtime 所有权，使认证后的 client 可供 session port 使用，并将 `StreamEvent` 路由给 session projection。重连、退避与认证策略继续归现有 supervisor 管理。
4. 替换 `SessionManager` 中仅 mock 的操作；真实路径移除伪造 assistant 回复。发送受理后跟踪 run，并在完成或事件有缺口时从服务端历史重新对账。
5. 先增加本地 Gateway 集成测试，再使用现有获准的连接/认证方式执行真实服务端 QA。

## Trade-offs and risks

**What this makes easier:** one production protocol source, reuse of the existing authenticated transport, and deterministic UI tests with a fake port.

**What this makes harder:** the UI adapter must follow canonical Gateway schemas and separate request acceptance from asynchronous streamed output. This deliberately avoids freezing an obsolete `message.*` API into Aleph or NSH.

**Concrete risks:**

- **Schema mismatch:** `session_key` is not the Phase 3 `session_id`; `chat.send` returns a run ID, not `message_id`. Convert at one boundary and retain the server key unchanged.
- **Event gaps / reconnect:** stream notifications are live deltas, not a durable transcript. Reload `chat.history` after reconnect or detected continuity loss; do not infer successful completion from a closed channel.
- **Duplicate or phantom messages:** the mock currently inserts a local user message and generates a fake reply. Do not keep that behavior in the RPC path; the server history is authoritative.
- **Connection ownership:** `run_authenticated_cycle` currently owns a local client and closes it at cycle end. Exposing a shared handle/event receiver to the session layer needs a deliberate lifecycle handoff; do not create a second client solely for chat calls.
- **Authorization:** preserve the existing handshake and per-method Gateway authorization. Do not add a direct server/store access path from NSH.

**Trade-off / 取舍：**统一使用生产协议、复用现有认证传输，并通过 fake port 保持 UI 测试确定性。

**增加的难点：**UI adapter 必须跟随 Gateway 规范 schema，并区分 RPC 受理与随后到达的流式响应。这样做是为了避免把过时的 `message.*` API 固化进 Aleph 或 NSH。

**具体风险：**

- **Schema 不匹配：**`session_key` 不等同于 Phase 3 的 `session_id`；`chat.send` 返回 run ID，而不是 `message_id`。应在单一边界转换，并原样保留服务端 key。
- **事件缺口/重连：**流通知是实时 delta，不是持久 transcript。重连或检测到连续性丢失后重新读取 `chat.history`；不能把通道关闭推断为 run 成功完成。
- **重复/幻影消息：**mock 当前会本地插入 user message 并生成假回复。RPC 路径不能保留此行为；服务端历史是权威数据。
- **连接所有权：**`run_authenticated_cycle` 当前持有局部 client，并在 cycle 结束时关闭。向 session 层交接共享 handle/event receiver 时必须明确生命周期；不要仅为了 chat 调用创建第二个 client。
- **授权：**保留现有握手与 Gateway 每方法授权；不要让 NSH 绕过 RPC 直接访问服务端/store。

## Testing and acceptance

- **Adapter unit tests:** request serialization and response decoding for session creation, send acceptance, history pagination, and error responses; assert `session_key` / `run_id` are not mistaken for message IDs.
- **Projection tests:** stream chunk accumulation, terminal completion/error, unrelated-session/run filtering, duplicate event handling as dictated by existing sequence fields, and history reconciliation after a dropped/disconnected stream.
- **Gateway integration test:** connect through a local test Gateway, create a session, send a message, receive stream frames, then verify the result with `chat.history`. Include rejected/unauthorized and server-error cases.
- **Real-server QA:** verify authenticated create → send → response chunks → completion → history after reconnect. Use an isolated test session; do not mutate production sessions.
- **Keep mock tests:** retain fast UI/session tests against the mock; they must not be treated as evidence of server interoperability.

- **Adapter 单元测试：**验证创建会话、发送受理、历史分页及错误响应的请求序列化/响应解码；明确断言 `session_key` / `run_id` 不会被误当成 message ID。
- **Projection 测试：**测试流分块累积、终态完成/错误、无关 session/run 过滤、按现有序号语义处理重复事件，以及流丢失/断线后的历史对账。
- **Gateway 集成测试：**连接本地测试 Gateway，创建会话、发送消息、接收流帧，再通过 `chat.history` 核对结果；覆盖拒绝/未授权及服务端错误。
- **真实服务端 QA：**验证认证后的 create → send → response chunks → completion → 重连后 history。使用隔离测试会话，不修改生产会话。
- **保留 mock 测试：**保留快速 UI/session mock 测试；这些测试不能作为服务端互操作已验证的证据。

**Acceptance gate / 验收门:** no UI/session call site relies on the assumed `message.*` or `session.start` names; real client contract tests and Gateway integration pass; reconnect recovery is verified against history; the existing mock path remains test-only and available.

**验收门：**UI/session 调用点不再依赖假定的 `message.*` 或 `session.start` 名称；真实 client 契约测试与 Gateway 集成测试通过；用历史接口验证重连恢复；现有 mock 路径仅用于测试且仍可用。

## Estimate

**Estimated total: 3–5 hours of implementation and verification**, assuming the local Gateway test setup and authenticated test credentials are available. Approximate split: adapter/model seam 1–1.5 h; runtime/client/event handoff 1–1.5 h; tests and real-server QA 1–2 h. Credential/bootstrap setup or a lifecycle redesign outside NSH is excluded and may extend the schedule.

**估算总计：实现与验证 3–5 小时**，前提是本地 Gateway 测试环境与认证测试凭据可用。粗略拆分：adapter/model 接缝 1–1.5 小时；runtime/client/event 交接 1–1.5 小时；测试和真实服务端 QA 1–2 小时。若需配置凭据/bootstrap，或 NSH 之外的生命周期设计调整，则不在此估算内。

## Not done / 未执行

This is an investigation and implementation plan only. No code, RPC method, mock behavior, prior plan, or live server state was changed. No Rust tests or end-to-end connection were run; availability of authenticated test credentials and the real server remains to be verified during implementation.

本文件仅记录调查结果与实现计划；未修改代码、RPC 方法、mock 行为、既有计划或在线服务端状态。未运行 Rust 测试或端到端连接；真实服务端与认证测试凭据的可用性需在实施阶段验证。
