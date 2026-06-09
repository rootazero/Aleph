# CLUSTER.md — Aleph 集群（单中心非对称节点联邦）

> Aleph 把**执行能力**扩展到多台机器：一个 **center**（大脑）驱动若干 **node**
> （执行臂）。Center 跑 DB / LLM / agent loop;node 只在本机 sandbox 跑
> bash 与文件命令,无 DB、无 LLM、无 harness。
>
> 与「一核多端」(R6) 的区别:**多端是 I/O 通道**(Telegram/Panel/CLI 把输入
> 转发给 core);**集群节点是 core 能远程驱动的「手」**。详见
> [self-management 指南 `cluster`](../guides/cluster.md)(LLM 视角) 与本文(工程视角)。

## 红线归属

- **R1**:node 是执行臂,允许直接碰 host-fs / 跑系统命令(这是它的本职)。
- **R4**:`Environment` 是对外薄渲染契约,绝不含凭证。
- **R7**:集群层全是确定性查表/路由,**无 LLM 推理**;命令/方向/路径的选择由
  中心侧 LLM 工具的调用者(模型)决定。
- **R10**:集群代码不进入 `src/harness/`。

## 模块版图

| 文件 | 角色 | 关键类型 |
|------|------|----------|
| `src/cluster/mod.rs` | 模块根 + 再导出 | — |
| `src/cluster/reverse_rpc.rs` | 反向 RPC 传输原语(中心→已连客户端的带 id 请求/响应) | `PendingInvokes` / `ReverseRpcChannel` / `ReverseRpcError` |
| `src/cluster/registry.rs` | 中心侧节点登记表 + 只读环境投影 | `NodeRegistry` / `NodeSession` / `Environment` / `CommandDescriptor` / `maybe_register_node` |
| `src/cluster/node_runtime.rs` | 节点侧命令分发(执行臂) | `NodeCommand` / `CommandTable` / `BashNodeCommand` |
| `src/cluster/node_file_cmd.rs` | 节点侧文件命令 | `FileReadCommand` / `FileWriteCommand` / `MAX_FILE_BYTES` / `sha256_hex` |
| `src/cluster/node_approval.rs` | 节点侧审批请求器(反向上送中心) | `CenterApprovalRequester` / `ApprovalSlot` / `NODE_APPROVAL_TIMEOUT_MS` |
| `src/builtin_tools/node_invoke.rs` | 中心侧 **LLM 工具**:在节点上跑命令 | `NodeInvokeTool` |
| `src/builtin_tools/node_file.rs` | 中心侧 **LLM 工具**:node↔center 文件传输 | `NodeFileTool` |
| `src/gateway/handlers/cluster.rs` | 中心侧 RPC:`cluster.enroll` / `environments.list` | `handle_cluster_enroll` / `handle_environments_list` |
| `src/bin/aleph-server/commands/node.rs` | `aleph-server node` 节点拨出运行时 | `handle_node` / `run_session` / `run_pairing` |

## 架构

```
          ┌──────────────────────────── CENTER (大脑) ─────────────────────────────┐
          │  agent loop / LLM                                                       │
          │     │ 选择工具                                                          │
          │     ▼                                                                   │
          │  node_invoke / node_file  ──resolve(name|id)──►  NodeRegistry           │
          │     │                                              │ ReverseRpcChannel  │
          │     ▼                                              ▼                     │
          │  channel.call("tool.call", {tool,args})  ──► 出站 mpsc ──► WS ──┐        │
          │  PendingInvokes ◄── resolve(id) ◄── 入站循环 ◄── WS ◄───────────┼──┐     │
          └─────────────────────────────────────────────────────────────────┼──┼────┘
                                                                             │  │ reverse-RPC
          ┌──────────────────────────── NODE (执行臂) ────────────────────── ▼  │ ───┐
          │  read loop:                                                          │   │
          │   • center→node REQUEST {method:"tool.call"} ─► tokio::spawn         │   │
          │        └─► CommandTable.dispatch ─► bash / file.read / file.write    │   │
          │             (本机 sandbox, allowlist = 表 keys)                       │   │
          │   • center→node RESPONSE ─► node PendingInvokes.resolve              │   │
          │  node→center REQUEST {method:"node.approval.request"} ───────────────┘   │
          │        (sandbox 能力升级 → 反向上送中心审批)                              │
          └─────────────────────────────────────────────────────────────────────────┘
```

要点:
- **请求/响应靠结构区分,不靠 id**(有 `method`=请求;有 `result`/`error`=响应)。
  因此中心与节点各自的反向 RPC id 空间可重叠而不互扰(`reverse_rpc.rs` 顶注)。
- **通道是双向的**:中心向节点发 `tool.call`,节点也能向中心发
  `node.approval.request`。节点 `run_session` 把 WS `split` 成读/写两半,出站
  走 mpsc + writer task,`tool.call` 在 `tokio::spawn` 里分发——**长命令(等审批)
  不会阻塞读循环**(否则审批响应到不了 → 死锁)。

## 节点接入(三步)

### 1. 登记(铸 token) — `cluster.enroll`(operator)

中心侧 RPC(凭证操作模式,同 `devices.*` / `pairing.*`,非 LLM 工具)。
铸一个 `DeviceRole::Node` 设备 + token,返还操作员转交节点机:

```jsonc
// → cluster.enroll  { "node_name": "worker-1" }
// ← { "node_id": "<uuid>", "token": "<...>", "signature": "<...>" }
```

Panel 入口:设置 → **服务与集群 → Aleph 集群 → + Enroll**。

### 2. 拨出 — `aleph-server node`

```bash
aleph-server node \
  --center ws://<center-host>:18790 \
  --token  <token-from-enroll> \
  --name   <node-name>
```

凭证解析优先级(`handle_node`):**持久化凭证 > `--token` > 交互配对**。
凭证持久化在 `~/.aleph/node/<name>.json`(unix `0o600`),含 `{node_id, bearer,
center}`;`bearer` = `"{token}:{signature}"`,作为 `connect` 帧的 `token` 原样发送。

重连:断线后指数退避 `2s → 60s`(`BACKOFF_INITIAL_MS` / `BACKOFF_MAX_MS`)。
`connect` 收到 `AUTH_FAILED (-32001)` → 清凭证 + 自动重新配对。

### 3. 交互配对(省略 `--token`)

无 token 时 `run_pairing`:匿名 WS → `pairing.start_node {node_name, commands}`
→ 打印 6 位配对码 → 每 2s 轮询 `pairing.poll {code}` → operator 在中心 **Panel
通知卡**批准后,`pairing.poll` 返回 `{status:"approved", token, device_id}` →
落盘凭证 → 进入 `run_session`。`status` 取值:`approved|rejected|expired|pending`。

## 中心侧 LLM 工具

模型经这两个工具驱动节点(纯 I/O 翻译,R4;选择由模型做,R7):

### `node_invoke` — 在节点跑命令

```jsonc
{ "node": "worker-1",          // name 或 id(见 environments.list)
  "command": "bash",            // 必须是节点声明的命令
  "args": { "cmd": "uname -a" },// 原样透传给该命令
  "timeout_ms": 120000 }        // 默认 120s,需大于命令本身耗时
```

寻址 `NodeRegistry::resolve`(先 id 精确,后 name);**中心侧 fail-fast**:仅当
节点声明了非空命令目录且其中不含该命令时才拒绝(空目录→交节点权威)。下发即
`channel.call("tool.call", {tool, args})`。

### `node_file` — node↔center 文件传输

```jsonc
{ "node": "worker-1",
  "direction": "push",          // push=center→node;pull=node→center
  "local_path": "/abs/center/path",
  "remote_path": "rel/to/node/workspace",
  "overwrite": false,
  "timeout_ms": 120000 }
```

**字节在中心进程↔节点进程间流动,永不进入 LLM 上下文**——模型只传路径,工具
负责读写中心盘 + base64 + 驱动反向 RPC,返回 `{direction, bytes, sha256,
local_path, remote_path}` 摘要。两端硬 **8MB**(`MAX_FILE_BYTES`)+ **sha256
完整性校验**(pull 时不匹配不落盘)。push 需节点声明 `file.write`,pull 需
`file.read`。

## 节点侧执行(`CommandTable`)

- **allowlist 即命令表的 keys,节点侧权威**:中心发什么都得在表里,否则
  `"command '<x>' not permitted on this node"`。`dispatch` 只认 `method ==
  "tool.call"`,`params = {tool, args}`。
- 默认能力:`bash`(`BashNodeCommand`,在固定 `SessionKey` 下经 `SESSION_ID.scope`
  委托 `BashExecTool`,跑在本机 sandbox);可选 `file.read` / `file.write`。
- **文件 jail**:`file.*` 锁在节点 session workspace 目录内
  (`session_workspace_dir`,与该节点 bash 同目录,故 push 的脚本能被 bash 跑、
  bash 产物能被 pull)。`resolve_in_jail` 复用 file_ops 的 canonicalize + deny-list,
  **再补一道 containment 闸**(`resolved.starts_with(root)`,绝对路径越界即拒)。

## 节点侧审批回中心

节点 headless,其 `ApprovalGate` 默认会拒一切能力升级(`requester=None`)。
集群把它换成 `CenterApprovalRequester`:命中升级时反向发 `node.approval.request
{tool, reason}` 上中心 → 路由进**现有** `ExecApprovalManager` + 复用 Panel 的
`ApprovalRequested` 审批卡(节点上下文编码进 `command` 字段)→ operator
approve/deny → 决策作 JSON-RPC 响应下行。

- `ApprovalSlot = Arc<RwLock<Option<ReverseRpcChannel>>>`:`run_session` 连上写
  `Some`,断开写 `None`。**fail-closed**:slot 为 `None`(断线)、传输错误、超时,
  一律映射 `Denied`,绝不静默放行。
- 下行 `outcome` → `outcome_from_str`:`approved` / `approved_session` /
  `timeout` → 对应 `ApprovalOutcome`;其余(含 `denied` 与任何未知值)→ `Denied`。
- `NODE_APPROVAL_TIMEOUT_MS = 130_000`,**故意大于**中心默认审批超时(120s),
  让中心先裁决、返回显式 `"timeout"`;节点侧超时只是传输死亡兜底。

## 存活性 / 断线 fail-fast

- 每条连接持一份 `ReverseRpcChannel` clone;节点掉线时连接 cleanup 调
  `PendingInvokes::cancel_all()` 排空全部等待者 → 每个仍在 `call()` 里 await 的
  在途 `node_invoke` / `node_file` / 审批调用即时收到 `RecvError` → 映射
  `ReverseRpcError::Cancelled` **立即返回**,而非空等满 `timeout_ms`。
- 重连安全:`NodeRegistry::register` 同 `node_id` 重连覆盖旧会话并清旧 `conn`
  映射;`deregister` 仅当当前会话确属该 `conn_id` 时才移除(旧连接 cleanup 不误删
  新会话)。
- 防伪:`node_identity_by_conn` 从**已认证连接**盖章节点身份,而非信任请求
  params——节点无法冒充别的节点(审批路由用此)。

## 线协议速查

| 方向 | 帧 |
|------|----|
| node → center | `connect { token, device_name, commands }` |
| center → node | 请求 `{ id, method:"tool.call", params:{ tool, args } }` |
| node → center | 响应 `{ id, result }` / `{ id, error }` |
| node → center | 请求 `{ id, method:"node.approval.request", params:{ tool, reason } }` |
| center → node | 响应 `{ id, result:{ outcome:"approved"|"approved_session"|"timeout"|"denied" } }` |

## RPC 权限(`src/gateway/method_authz.rs`)

| 方法 | 权限 |
|------|------|
| `cluster.enroll` | operator |
| `environments.list` | read(只读,不含凭证) |
| `pairing.start_node` / `pairing.poll` | 匿名(节点配对入口) |
| `pairing.approve` / `pairing.reject` | operator |
| `exec.approval.resolve` | operator(同样裁决节点审批) |

## 相关代码与测试

- 反向 RPC 不变量:`reverse_rpc.rs` 的 `cancel_all_drops_every_waiter_*` /
  `inflight_call_returns_cancelled_after_cancel_all`(断线 fail-fast)。
- 登记/重连:`registry.rs` 的 `reconnect_same_node_overwrites_and_old_cleanup_does_not_evict_new`。
- allowlist 权威:`node_runtime.rs` 的 `dispatch_rejects_unlisted_command`。
- jail containment:`node_file_cmd.rs` 的 `file_write_rejects_traversal`。
- 审批 fail-closed:`node_approval.rs` 的 `outcome_mapping_is_fail_closed` /
  `none_channel_denies` / `transport_closed_denies`。

## 与「一核多端」的边界

| | 一核多端 (R6) | Aleph 集群 |
|--|--------------|-----------|
| 扩展的是 | **I/O 触达**(多渠道/多客户端) | **执行**(多机器) |
| 远端角色 | 渠道/Panel = 纯 I/O 表面 | node = 远程执行臂 |
| 远端是否推理 | 否(推理在 core) | 否(推理在 center) |
| 配置指南 | [`multi_channel`](../guides/multi_channel.md) | [`cluster`](../guides/cluster.md) |
