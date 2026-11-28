# Agent-to-Agent (A2A) / 代理互操作

Aleph Gateway 可接收其他 Agent 的文本任务，也可以通过 `a2a_delegate` 把文本任务发给已注册的远程 Agent。A2A 在 `[a2a] enabled = true` 时启用。入站任务由 Core 执行，出站选择从 `CardRegistry` 读取。

Aleph Gateway accepts text tasks from A2A clients and delegates text tasks to registered peers through `a2a_delegate`. Enable it with `[a2a] enabled = true`. Core runs inbound work, and `CardRegistry` supplies outbound peer selection.

## 配置与发现 / Configuration and discovery

```toml
[a2a]
enabled = true
health_check_interval_secs = 0

[a2a.server]
advertised_base_url = "https://aleph.example.net/gateway"

[a2a.server.security]
local_bypass = false
tokens = ["replace-with-a-per-peer-secret"]

[[a2a.agents]]
name = "Research peer"
url = "https://peer.example.net"
```

`advertised_base_url` 填 Gateway 的公开根地址，可带反代路径前缀。省略时由 Gateway 监听地址生成。`health_check_interval_secs = 0` 关闭周期性出站探测。`a2a_agents` 可列出、添加或移除远程 Agent，`a2a_delegate` 接受远程 Agent 的 ID 或名称。注册与健康探测的实现见 [`card_registry.rs`](../../src/a2a/service/card_registry.rs) 和 [`card_refresh.rs`](../../src/a2a/service/card_refresh.rs)。

`advertised_base_url` is the public Gateway base URL and may include a reverse-proxy path prefix. The Gateway listener supplies the default. `health_check_interval_secs = 0` disables periodic outbound checks. Use `a2a_agents` to list, add, or remove agents and `a2a_delegate` to select a peer by ID or name. See [`card_registry.rs`](../../src/a2a/service/card_registry.rs) and [`card_refresh.rs`](../../src/a2a/service/card_refresh.rs).

| 接口 / Interface | 路径 / Path | 行为 / Behavior |
|---|---|---|
| 标准卡片 / Standard card | `GET /.well-known/agent-card.json` | 同一份卡片提供 0.3 发现字段与 1.0 `supportedInterfaces`。 The card exposes 0.3 discovery fields and 1.0 `supportedInterfaces`. |
| 标准 JSON-RPC 与 SSE / Standard JSON-RPC and SSE | `POST /a2a/rpc` | 缺省为 0.3，`A2A-Version: 1.0` 选择 1.0。 Defaults to 0.3 and selects 1.0 with `A2A-Version: 1.0`. |
| Aleph 兼容卡片 / Aleph compatibility card | `GET /a2a/legacy/.well-known/agent-card.json` | 提供旧版 Aleph 卡片。 Serves the old Aleph card shape. |
| Aleph 兼容执行 / Aleph compatibility execution | `POST /a2a`, `POST /a2a/stream` | 保留旧版同步与 SSE 路由，同样可通过 `/a2a/legacy` 前缀访问。 Preserves the original unary and SSE routes, also reachable under `/a2a/legacy`. |

需要旧版 Aleph 卡片的客户端可把远端配置根地址设置为 `https://aleph.example.net/gateway/a2a/legacy`。标准请求和响应在 [`wire.rs`](../../src/a2a/adapter/wire.rs) 转换，路由挂载见 [`routes.rs`](../../src/a2a/adapter/server/routes.rs)。

Clients that require the old Aleph card can use a base URL such as `https://aleph.example.net/gateway/a2a/legacy`. See [`wire.rs`](../../src/a2a/adapter/wire.rs) for standard payload conversion and [`routes.rs`](../../src/a2a/adapter/server/routes.rs) for mounting.

## 协议范围 / Protocol scope

| 功能 / Operation | 0.3 | 1.0 | Aleph compatibility |
|---|---|---|---|
| 发送 / Send | `message/send` | `SendMessage` | `message/send` |
| 流式发送 / Streaming send | `message/stream` | `SendStreamingMessage` | `message/stream` |
| 查询 / Get | `tasks/get` | `GetTask` | `tasks/get` |
| 取消 / Cancel | `tasks/cancel` | `CancelTask` | `tasks/cancel` |
| 重订阅 / Subscribe | `tasks/resubscribe` | `SubscribeToTask` | `tasks/resubscribe` |
| 列表 / List | — | `ListTasks` | `tasks/list` |
| 自定义 webhook / Legacy webhook | — | — | `tasks/pushNotificationConfig/*` |

标准 `message/send` 的 `blocking=false` 和 `SendMessage` 的 `returnImmediately=true` 返回当前任务，调用者可随后查询。标准流先发送任务快照，再发送实时更新。1.0 对终态任务的订阅请求返回 `UnsupportedOperation`，可用 `GetTask` 读取快照。兼容重订阅立即返回保留的终态并结束。运行期间观察缓冲区溢出时，客户端可重新查询任务。标准范围为 `text/plain`、JSON-RPC 和 SSE。文件或结构化输入、标准 push、REST 和 gRPC 不在此绑定的支持范围内。

Standard `message/send` with `blocking=false` and `SendMessage` with `returnImmediately=true` return the current task for subsequent queries. Standard streams send the snapshot before live updates. A 1.0 subscription to a terminal task returns `UnsupportedOperation`, and `GetTask` retrieves the final snapshot. Compatibility subscriptions receive the retained terminal status and close. Clients can query the task after losing its live stream to lag. This binding supports `text/plain`, JSON-RPC, and SSE. File or structured inputs, standard push, REST, and gRPC are outside the binding's supported surface.

## 任务权限与生命周期 / Task authority and lifecycle

- 同一个已验证 token 在 Bearer 和 `X-API-Key` 上使用同一任务范围。不同 token 的获取、继续、取消、列表、订阅和 webhook 配置相互隔离。无凭据的直接本机请求只在显式设置 `local_bypass = true` 时共享一个本机范围。携带转发标头的请求不使用本机旁路。
- A verified token identifies the same task scope through Bearer and `X-API-Key`. Each credential scopes get, continue, cancel, list, subscribe, and webhook configuration. Direct credential-free loopback requests share their own scope when `local_bypass = true`. Forwarding headers prevent loopback bypass.
- 标准任务 ID 由服务端生成。兼容任务允许调用者选择公开 ID，按凭据解析并绑定到服务端任务实例。快照、订阅和回调在同一任务实例下协调。快照保存在有限容量的内存存储中，终态可能被回收，进程重启会清空存储。
- Standard tasks use server-assigned IDs. Compatibility IDs are scoped aliases for server-assigned task instances. The task instance binds snapshots, subscriptions, and callbacks. Snapshots live in bounded memory, where terminal tasks may be evicted. Restarting the process clears the store.
- 同一标准上下文一次只准入一个 Core 运行。运行期间收到第二条发送请求会返回 `context_busy`。取消请求向 Core 记录停止意图，确认结算后才返回 `Canceled`。五秒内未结算时返回 `cancellation_pending`，之后可查询任务结果。入站任务按无人值守策略执行。
- A standard context admits one Core run at a time. A second send returns `context_busy` during execution. Active cancellation records stop intent in Core and reports `Canceled` once settlement is confirmed. A five-second wait can return `cancellation_pending`, after which the task remains queryable. Inbound work uses the unattended tool policy.
- 模型调用 `ask_user` 时，Core 停止本次运行，任务返回 `InputRequired` 并携带澄清问题。原凭据可以向同一任务发送后续消息以启动新 Core run，也可以取消暂停中的任务。澄清消息使用文本，工具会拒绝索取凭据的 `secret` 问题。
- A model call to `ask_user` stops the Core run and returns `InputRequired` with the clarification question. The original credential can use the same task ID to submit a follow-up and start a new Core run, or cancel the paused task. Clarification travels as text and the tool rejects questions marked `secret`.

服务端任务存储与执行接线见 [`task_store.rs`](../../src/a2a/adapter/server/task_store.rs)、[`bridge.rs`](../../src/a2a/adapter/server/bridge.rs) 和 [`standard.rs`](../../src/a2a/adapter/server/standard.rs)。

See [`task_store.rs`](../../src/a2a/adapter/server/task_store.rs), [`bridge.rs`](../../src/a2a/adapter/server/bridge.rs), and [`standard.rs`](../../src/a2a/adapter/server/standard.rs) for task and Core execution wiring.

## 出站委派 / Outbound delegation

`[[a2a.agents]]` 的 `url` 接受对端部署根地址或 Agent Card 地址。客户端从卡片选取支持的 JSON-RPC 接口，优先选择 1.0，并在发送前决定用 SSE 还是普通响应。默认只接受发现地址同源的执行端点。确需使用另一源时，operator 可在该条目设置精确的 `rpc_url`。发现和发送都使用 Gateway 配置的出站地址策略。

The `url` in `[[a2a.agents]]` accepts the peer's deployment root or Agent Card URL. The client chooses a supported JSON-RPC interface from the card, prefers 1.0, and selects SSE or unary before sending. The default requires the execution endpoint to share the discovery origin. An operator can set an exact `rpc_url` on the entry to authorize another origin. Discovery and dispatch both apply the Gateway's outbound address policy.

`a2a_delegate` 通过 Zahir 调用 `a2a/<peer-id>` Agent Capability。卡片与端点由注册表持有。健康变化更新描述，影响调用的变更替换注册代次。撤回、替换或关停先于最终准入时，旧注册不能发送请求。每次委派最多发送一次请求。标准对端可在 SSE 开始前通过 JSON-RPC 返回拒绝，工具返回对应错误和 `delivery: response_received`。发送后结果仍不明确时，工具返回 `delivery: unknown`。

`a2a_delegate` calls the `a2a/<peer-id>` Agent Capability through Zahir. The registry holds the card and endpoint. Health changes update the description, while changes affecting dispatch advance the registration generation. Withdrawal, replacement, or shutdown before final admission prevents dispatch through the old registration. A delegation sends at most one request. A standard peer can reject a request through JSON-RPC before opening SSE. The tool returns the matching error with `delivery: response_received`. When the result remains uncertain after dispatch, the tool returns `delivery: unknown`.

代码与验证入口 / Code and verification entry points [client](../../src/a2a/adapter/client/http_client.rs) · [registry](../../src/a2a/service/card_registry.rs) · [Capability](../../src/a2a/capability.rs) · [SDK QA](../../qa/a2a/README.md) · [验收记录 / Verification record](../superpowers/runs/2026-11-28-a2a-architecture-verification.md)
