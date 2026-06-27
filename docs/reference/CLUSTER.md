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
| `src/builtin_tools/node_list.rs` | 中心侧 **LLM 工具**:列出在线节点(invoke 工具的 discover 半边) | `NodeListTool` |
| `src/builtin_tools/node_invoke.rs` | 中心侧 **LLM 工具**:在单个节点上跑命令 | `NodeInvokeTool` |
| `src/builtin_tools/node_invoke_many.rs` | 中心侧 **LLM 工具**:按标签把命令并发扇出到一组节点 | `NodeInvokeManyTool` / `invoke_one` |
| `src/builtin_tools/node_file.rs` | 中心侧 **LLM 工具**:node↔center 文件传输 | `NodeFileTool` |
| `src/gateway/handlers/cluster.rs` | 中心侧 RPC:`cluster.enroll` / `cluster.deregister` / `environments.list` | `handle_cluster_enroll` / `handle_cluster_deregister` / `handle_environments_list` |
| `src/bin/aleph-server/commands/node.rs` | `aleph-server node` 节点拨出运行时 | `handle_node` / `run_session` / `enroll_node` |

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

## 节点接入(两步) — LAN-trust

> **信任模型 = 网络边界**:节点**不持 token**。信任边界即网络边界(中心默认只绑
> `127.0.0.1`,`[gateway] host = "0.0.0.0"` 才放开 LAN)。enroll 只在中心留一条设备
> 记录并铸一个稳定 `node_id`(UUID);连接身份由 `connect` 帧的**参数形状**
> (`commands` + `tags`,无其他客户端会发)声明,中心据此把节点稳定键入同一 UUID
> (修正 `touch_device` / `environments.list` / `deregister` 的记账漂移)。无 token、
> 无配对码、无 `AUTH_FAILED`。收紧节点认证属 T6 计划。

### 1. 登记 — `cluster.enroll`(operator)

中心侧 RPC(operator 门槛,同 `devices.*`,非 LLM 工具)。写一条 `role=node` 设备
记录,返还操作员转交节点机的 `node_id`(**不铸 token**):

```jsonc
// → cluster.enroll  { "node_name": "worker-1" }
// ← { "node_id": "<uuid>" }
```

Panel 入口:设置 → **服务与集群 → Aleph 集群 → + Enroll**。

> **注销** — `cluster.deregister`(operator):`{ "node": "<name|id>" }`。两步下线:
> ① `NodeRegistry::forget` 即时驱逐在线会话(立刻从 `environments.list` 消失且不再
> 被 `node_invoke`/`node_file` 寻址);② `revoke_device`(软删,置 `revoked_at`)抹除
> 设备记录(enroll 的对称撤除,`list_devices` 仅返 `revoked_at IS NULL` 故离线视图也
> 随即消失)。返回 `{ node_id, evicted, device_removed }`。不强制 close 节点当前
> socket——它在下一次 ping/idle-watchdog 到期时由传输层断开;LAN-trust 下没有 token
> 可撤,阻止重连属网络边界(bind/origin)职责。寻址先走在线 `NodeRegistry` 多级匹配,
> 不在线则回退 `security_store` 已登记节点(精确 id / 唯一精确 name),故
> `environments.list` 里可见的离线节点同样可注销(此时 `evicted:false`)。
> Panel 入口:每行节点的 **「注销」** 按钮。

### 2. 拨出 — `aleph-server node`

```bash
aleph-server node \
  --center ws://<center-host>:18790 \
  --name   <node-name> \
  --tag    gpu --tag region=us      # 可重复;经 connect 帧上报,供 node_invoke_many 选择
```

身份解析(`handle_node`):**持久化身份 > 现场 enroll**。首启无持久化身份时
`enroll_node` 拨一条临时 WS 调 `cluster.enroll` 拿回 `node_id` 并落盘;之后每次启动
直接复用。身份持久化在 `~/.aleph/node/<name>.json`(unix `0o600`),仅含
`{node_id, center}`(无 bearer/token)。旧版含 `bearer` 字段的凭证文件可无损升级
——serde 丢弃死字段、保留 `node_id`,升级后的节点不会重新 enroll。

标签由 CLI 每次启动提供(经 `connect` 帧上报,出现在 `environments.list`),**不**
持久化进身份文件。

重连:断线后指数退避 `2s → 60s`(`BACKOFF_INITIAL_MS` / `BACKOFF_MAX_MS`)。
`connect` 回复在 LAN-trust 下恒成功(无 auth 可校验),drain 后即进 `run_session`。

## 中心侧 LLM 工具

模型经这些工具发现并驱动节点(纯 I/O 翻译,R4;选择由模型做,R7)。四个工具
全部列于 `BUILTIN_TOOL_DEFINITIONS` + `cluster` 工具组(模型可见性与 Panel
工具配置页都从这两处投影——只注册运行时 schema/分发是不够的):

### `node_list` — 发现在线节点(discover 半边)

```jsonc
{ "tags": ["gpu"] }   // 可选 AND 过滤;省略 = 全部在线节点
```

返回 `{ count, nodes:[{id, name, status, commands, tags, connected_at}] }`。
传与 `node_invoke_many` 相同的 `tags` 可**预览**扇出将命中的节点集合。
旧版工具描述让模型"see `environments.list`"——那是 Panel RPC,模型不可达;
现在 discover→invoke 闭环全在工具面。

### `node_invoke` — 在节点跑命令

```jsonc
{ "node": "worker-1",          // name 或 id(见 environments.list)
  "command": "bash",            // 必须是节点声明的命令
  "args": { "cmd": "uname -a" },// 原样透传给该命令
  "timeout_ms": 120000 }        // 默认 120s,需大于命令本身耗时
```

寻址 `NodeRegistry::resolve` 走**多级匹配**(映射 openclaw `node-match.ts`,但用
类型安全的 `ResolveError` 枚举表达):① 精确 node_id(原样,UUID) → ② 归一化
device_name 等值 → ③ 模糊(id 前缀 ≥4 字符 *或* 归一化 name 子串)。**名字匹配
经 `normalize_node_key` 大小写 + 标点/空格不敏感**(映射 openclaw
`normalizeNodeKey`:`[^a-z0-9]+ → -`),故 "GPU Box" / "gpu_box" / "gpu-box"
折叠为同一键、可互相寻址;两个归一化后撞键的名字(如 "Worker 1" 与 "worker-1")
按 ② 报歧义而非静默挑一个。任一级多命中即返回
`ResolveError::Ambiguous(候选标签)`,绝不静默挑第一个;调用方把它翻成给模型的
精确提示(`node 'x' ambiguous — matches: worker-1 (id…), worker-2 (id…)`)。
registry 只存在线会话,故无需"prefer-connected" tie-break。同一 `normalize_node_key`
被 `cluster.deregister` 的离线回退寻址(`handlers/cluster.rs::resolve_enrolled_node`)
复用,消除"在线名大小写不敏感、离线敏感"的旧漂移。**中心侧 fail-fast**:
仅当节点声明了非空命令目录且其中不含该命令时才拒绝(空目录→交节点权威)。
下发即 `channel.call("tool.call", {tool, args})`。

### `node_invoke_many` — 按标签并发扇出

```jsonc
{ "tags": ["gpu"],            // AND 匹配:节点须含全部 tag;[] = 所有在线节点
  "command": "bash",          // 每个命中节点都要声明该命令(否则该节点单独报错)
  "args": { "cmd": "nvidia-smi -L" },
  "timeout_ms": 120000 }      // 每节点独立超时
```

经 `NodeRegistry::resolve_all_by_tags`(AND 语义)取命中集合,用 `tokio::task::JoinSet`
**并发**下发 `tool.call`——墙钟 = 最慢单节点。**容忍部分失败**:逐节点 fail-fast
(节点声明非空命令目录却不含该命令 → 该节点错,其余照跑),返回聚合
`{ invoked, succeeded, failed, results:[{node,node_id,ok,(result|error)}] }`。
**零命中报错**并附"available tags: …"提示(镜像 `resolve` 的 fail-fast 风格)。
标签纯用于选择,不构成授权层(R7);命令执行权威仍是节点侧 `CommandTable` allowlist。

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
| node → center | `connect { device_id, device_name, commands, tags }`(LAN-trust,无 token) |
| center → node | 请求 `{ id, method:"tool.call", params:{ tool, args } }` |
| node → center | 响应 `{ id, result }` / `{ id, error }` |
| node → center | 请求 `{ id, method:"node.approval.request", params:{ tool, reason } }` |
| center → node | 响应 `{ id, result:{ outcome:"approved"|"approved_session"|"timeout"|"denied" } }` |

## RPC 权限(`src/gateway/method_authz.rs`)

| 方法 | 权限 |
|------|------|
| `cluster.enroll` | operator |
| `cluster.deregister` | operator |
| `environments.list` | read(只读,不含凭证) |

`environments.list` 是**合并视图**:NodeRegistry 在线会话 + security_store 已登记
(role=node、未吊销)但离线的设备(`status:"offline"`,附 `last_seen_at` Unix 秒,
`null`=登记后从未连入)。last_seen 在节点 connect/disconnect 两接缝经
`TokenManager::touch_device` 盖章。`cluster.deregister` 寻址同样支持离线节点
(在线 registry 多级匹配 → 回退 store 精确 id/唯一精确 name;离线注销时
`evicted:false`,仅软删设备记录)。

| 方法 | 权限 |
|------|------|
| `exec.approval.resolve` | operator(同样裁决节点审批) |

> LAN-trust 迁移后**已无** `pairing.*` 节点配对方法——节点不再凭 token/配对码接入,
> 连接身份由 `connect` 帧参数形状声明(见上「节点接入」)。

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
