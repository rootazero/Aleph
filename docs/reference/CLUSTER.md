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
| `src/cluster/enrollment.rs` | 中心侧节点**生命周期**(准入/预登记/注销)的单一真源 | `admit_node` / `NodeAdmission` / `enroll_node_device` / `deregister_node` |
| `src/cluster/reverse_rpc.rs` | 反向 RPC 传输原语(中心→已连客户端的带 id 请求/响应) | `PendingInvokes` / `ReverseRpcChannel` / `ReverseRpcError` |
| `src/cluster/registry.rs` | 中心侧节点登记表 + 只读环境投影 | `NodeRegistry` / `NodeSession` / `Environment` / `CommandDescriptor` / `maybe_register_node` |
| `src/cluster/node_runtime.rs` | 节点侧命令分发(执行臂) | `NodeCommand` / `CommandTable` / `BashNodeCommand` |
| `src/cluster/node_file_cmd.rs` | 节点侧文件命令 | `FileReadCommand` / `FileWriteCommand` / `MAX_FILE_BYTES` / `sha256_hex` |
| `src/cluster/node_approval.rs` | 节点侧审批请求器(反向上送中心) | `CenterApprovalRequester` / `ApprovalSlot` / `NODE_APPROVAL_TIMEOUT_MS` |
| `src/builtin_tools/node_list.rs` | 中心侧 **LLM 工具**:列出在线节点(invoke 工具的 discover 半边) | `NodeListTool` |
| `src/builtin_tools/node_invoke.rs` | 中心侧 **LLM 工具**:在单个节点上跑命令 | `NodeInvokeTool` |
| `src/builtin_tools/node_invoke_many.rs` | 中心侧 **LLM 工具**:按标签把命令并发扇出到一组节点 | `NodeInvokeManyTool` / `invoke_one` |
| `src/builtin_tools/node_file.rs` | 中心侧 **LLM 工具**:node↔center 文件传输 | `NodeFileTool` |
| `src/builtin_tools/node_manage.rs` | 中心侧 **LLM 工具**:改变舰队成员(登记/注销) | `NodeManageTool` |
| `src/gateway/handlers/cluster.rs` | 中心侧 RPC:`cluster.enroll` / `cluster.deregister` / `environments.list` | `handle_cluster_enroll` / `handle_cluster_deregister` / `handle_environments_list` |
| `src/bin/aleph-server/commands/node.rs` | `aleph-server node` 节点拨出运行时 | `handle_node` / `run_session` / `parse_connect_verdict` / `init_node_tracing` |

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

## 节点接入 — LAN-trust,登记发生在 `connect` 里

> **信任模型 = 网络边界**:节点**不持 token**。信任边界即网络边界(中心默认只绑
> `127.0.0.1`,`[gateway] host = "0.0.0.0"` 才放开 LAN)。连接身份由 `connect` 帧的
> **参数形状**(`commands` + `tags`,无其他客户端会发)声明。无 token、无配对码、
> 无 `AUTH_FAILED`。

### 为什么登记不能是一个独立 RPC

中心对每条连接强制两道闸,顺序是**先首帧规则、后登录墙**:

1. **首帧必须是 `connect`**(`gateway/server/handler.rs`)——否则回 `AUTH_REQUIRED`
   **并关闭 socket**。
2. **登录墙**:远程未授权连接只能发 `connect`,别的方法一律拒。

`connect` 是唯一同时越过这两道闸的帧。所以:

- 节点若另开一条 socket、首帧直接发 `cluster.enroll`,**必被第 1 道闸拒掉**
  (这正是历史 bug:新节点从来没登记成功过,而单测直接调 handler 函数、绕过了
  这条规则,所以一直是绿的);
- 即使补发 `connect` 再发 `cluster.enroll`,**远程**节点仍会被第 2 道闸挡住
  ——它没有 token,拿不到 operator。

因此登记只能长在 `connect` 上。节点全生命周期只用三种帧:`connect`(登记 + 注册)、
`tool.call` 的**响应**、`node.approval.request` 的**请求**——后两者都在登录墙之前
被拦截处理,故远程节点无需任何凭据即可稳态工作。

> **「首帧规则」是协议级不变量,不是集群的私事。** 任何直连 WS 的客户端都必须先握手
> `connect` 再发方法。它曾同时放倒两处:① 本节的节点冷启动登记;② `aleph-server
> gateway call`(`shared/client/src/gateway_client.rs::call_raw` 把方法当首帧发 ⇒
> **全方法失效**,恒报 `First request must be 'connect'`)。两者已一并修复。
> **写新的 WS 客户端时**:握手 params 只带 `device_name`/`channel_kind`,**绝不要带
> `commands`/`tags`**——那是中心识别「集群节点」的形状,会把你的客户端注册成执行臂。

### 节点自助登记(`cluster::admit_node`)

```bash
aleph-server node \
  --center ws://<center-host>:18790 \
  --name   <node-name> \
  --tag    gpu --tag region=us      # 可重复;经 connect 帧上报,供 node_invoke_many 选择
```

首启时节点不带 `device_id` 拨入;中心在 connect 回包里交回身份:

```jsonc
// node → center
{ "method": "connect", "params": {
    "device_name": "worker-1", "commands": [...], "tags": ["gpu"] } }   // 无 device_id

// center → node
{ "result": { ..., "node": { "node_id": "<uuid>", "status": "registered", "persist": true } } }
```

节点把 `node_id` 落盘到 `~/.aleph/node/<name>.json`(unix `0o600`,仅含
`{node_id, center}`),之后每次 connect 都带上它。`persist:false` = 中心只是确认了
你已有的 id,不必重写文件。旧版含 `bearer` 字段的凭证文件可无损升级(serde 丢弃死
字段、保留 `node_id`)。标签由 CLI 每次启动提供,**不**持久化。

> **落盘时机是关键**:`run_session` 在**收到 connect 裁决的当场**回调 `on_identity`
> 写盘,**早于读循环**——不能等 `run_session` 返回后再写。session 与节点同寿,等它返回
> 意味着「节点被 kill 就永远不落盘」,下次启动重新登记 ⇒ 恰好铸出本流程要消灭的那种
> 重复设备行。(此坑由真机 e2e 抓出,单测看不见。)

> **节点进程自带日志**:`handle_node` 入口调 `init_node_tracing()`(stderr + `RUST_LOG`,
> 默认 info)。tracing subscriber 只在 `start` 命令里装,而 `node` 子命令根本走不到那儿
> ——不自己装就**全程静默**:登记成功/被拒/重连一行日志都没有,连下文「被注销 → 打印
> 可操作提示后退出」这条路径都不可能被看见。

`admit_node` 的四条判定(`src/cluster/enrollment.rs`):

| 情形 | 判定 |
|------|------|
| 带 id,记录**已吊销** | `Deregistered` → 拒绝(注销粘住,见下) |
| 带 id,记录活跃 | 原样复用(稳定身份) |
| 带 id,**查无此记录**(中心库重置/换中心) | 采纳该 id 并补写记录 |
| 无 id(首启) | 先按**归一化名**复用 operator 预登记的行,否则铸新的;`persist:true` |

"按名复用"消灭了幽灵行:Panel 预登记的 "GPU Box" 与节点 `--name gpu-box` 拨入会
归并到**同一条**设备记录,而不是各铸一个 UUID 留下一条永不上线的离线条目
(该幽灵还会让按名的离线注销因歧义而失败)。

### `cluster.enroll` — operator 预占位(可选)

中心侧 RPC。节点**并不需要**先走它。它的作用是让 operator 先把名字占下——节点还没
拨入前就以 `status:"offline"` 出现在舰队视图里,且节点随后同名拨入时被上表最后一条
规则归并到这一行。与 connect 自助登记共用同一设备记录真源(`enroll_node_device`)。

```jsonc
// → cluster.enroll  { "node_name": "worker-1" }
// ← { "node_id": "<uuid>", "reused": false }   // 不铸 token
```

> **登记是幂等的**(2026-07-25 修)。同名再登记返回**同一个** `node_id`(`reused:true`),
> 不再铸第二行。此前 `mint_node_device` 无条件铸新 UUID,于是 Panel「+ Enroll」多点一次
> 就多一条同名 `role=node` 行——而这种重复是**自我封死**的:第二行让按名归并变歧义,节点
> 首启于是既不认领任何一行、又去铸**第三行**;同时 `cluster.deregister` 的离线回退要求
> 名字唯一命中,于是这几行**一条都删不掉**,operator 只能去改数据库。既有舰队若已被污染,
> 登记会**报错并要求先按 id 删重复**,而不是再加一行。

Panel 入口:设置 → **服务与集群 → Aleph 集群 → + Enroll**。Panel 拿到 `node_id` 后
展示的是**要在目标机器上跑的那条命令**,不是 token。

### 注销 — `cluster.deregister`,且**注销是粘的**

`{ "node": "<name|id>" }`。两步下线:

1. `NodeRegistry::forget` 即时驱逐在线会话(立刻从 `environments.list` 消失,且不再
   被 `node_invoke`/`node_file` 寻址);
2. `revoke_device` 置 `revoked_at`。

**关键**:第 ② 步不再只是记账。节点下一次 `connect` 会经 `admit_node` 撞上这条
`revoked_at`,被判 `Deregistered` 而拒绝登记,回包 `{"node":{"status":"deregistered"}}`;
节点据此**打印可操作日志并退出**,不再无谓重连。此前 connect 从不查设备表,被注销的
节点在下一轮退避重连里就自己复活了——注销形同虚设。

返回 `{ node_id, evicted, device_removed }`。寻址先走在线 `NodeRegistry` 多级匹配,
不在线则回退 `security_store` 已登记节点(精确 id / **唯一归一化 name**),故
`environments.list` 里可见的离线节点同样可注销(此时 `evicted:false`)。

**第 ① 步现在会当场掐断那条 socket**(2026-07-25):`NodeRegistry::forget` 除了驱逐会话,
还会触发该连接的 close 信号(`ReverseRpcChannel::close_connection`,与慢消费者踢除共用
同一根线),读循环退出 → 跑现有全套 cleanup → 关 socket。两张表在**同一个写锁**下清空,
close 信号在锁释放之后才发——不是为了躲死锁(`close_connection` 是不拿锁、不阻塞的
`Notify::notify_one`),而是让驱逐本身原子、临界区里不带唤醒副作用。此前只驱逐不关连接,那条连接要
熬到下一次 ping / ≤90s 入站 idle-watchdog 才断——而这段窗口里被注销的节点**仍在跑**中心
先前下发的命令,且它的 `node.approval.request` 通道**仍然活着**,刚把它踢掉的 operator
还会收到它弹的审批卡。

**这三处 RPC/工具共用同一真源** `cluster::deregister_node`(`enrollment.rs`):
`cluster.deregister` RPC、`node_manage` 工具、以及它们背后的两段式拆除,都是同一段代码
——handler 只做 I/O 翻译(R4)。

### 谁能改舰队成员

| 入口 | 面向 | 说明 |
|------|------|------|
| Panel 设置 → Aleph 集群 | operator 点按 | `cluster.enroll` / `cluster.deregister` RPC |
| `node_manage` 工具 | **模型/对话** | R8:「把 worker-3 踢出集群」应当能对话完成 |

两者调的是同一对函数(`enroll_node_device` / `deregister_node`),不存在「Panel 删得掉、
对话删不掉」这类语义分叉。

### 重连

断线后指数退避 `2s → 60s`,带 **±25% jitter**(中心重启时 N 个节点不会齐步撞门)。
退避**只在真正活过一段时间(≥30s)的会话之后才重置**——否则一个"接受连接后立刻关闭"
的中心(origin 不对、正在关停)会让节点以 2s 死循环热转。

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
`results` **按 `(node, node_id)` 排序**——`JoinSet` 是按完成先后 yield 的,不排序的话
同一个扇出每次跑都会给模型一个顺序不同的数组(快的节点排前面),不可复现。
同理,节点上报的命令目录(`CommandTable::descriptors()`)也按名排序,不让 `HashMap`
的随机迭代序漏进 `environments.list` 与 `node_list`。

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

### `node_manage` — 改变舰队成员(R8 的写半边)

```jsonc
{ "action": "enroll",     "node": "worker-3" }   // 占名,返回 node_id(幂等)
{ "action": "deregister", "node": "worker-3" }   // 踢出,粘性,离线节点同样可删
```

`node_list` 只读舰队,这个工具**写**舰队。此前登记/注销只有 Panel RPC,于是「把 worker-3
踢出集群」是唯一无法对话完成的集群操作(违 R8)。工具与 Panel 走**同一对函数**
(`enroll_node_device` / `deregister_node`),不新增任何语义。

`enroll` 不会远程安装或启动任何东西——它只是占名并回一个 `node_id`,机器要等有人在那台
机器上跑 `aleph-server node --center … --name worker-3` 才真正加入。收在 `OPERATOR_TOOLS`
里:改变「中心拥有哪些机器」比在已有机器上执行更强。

## 版本握手(仅观测,不拒连)

节点在 `connect` 帧带 `version: env!("ALEPH_VERSION")`,中心存进 `NodeSession`,经
`node_list` / `environments.list` 的 `version` 字段透出(Panel 舰队行也渲染),并在版本与
中心不一致时 `warn!` 一行。

**刻意不拒连**。openclaw 的 `server.node-version-mismatch` 守卫治的是它**同机 bundled 的
local node**——版本天然应当一致。Aleph 的节点是 LAN 里独立升级的机器,滚动升级是常态,
按版本拒连等于**每次升级中心就打掉全部未升级节点**。我们要的只是:当有人报告「那台机器
表现和别的不一样」时,有一个可对照的事实。离线节点**永远** `version:null`——设备表没有
版本列,记住的版本是对一台看不见的机器的过期断言。

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
- **`timeout_ms` 是整次调用的预算**,覆盖「把帧压进出站队列」+「等响应」两段。出站是
  有界 mpsc(容量 64);对端 TCP 停止收字节时(慢消费者 / 半开连接)writer 会卡住、
  队列灌满,此时 `outbound.send().await` 会**无限期挂起**——旧实现在入队这一段没有
  超时,等于把 `timeout_ms` 契约悄悄作废,调用方永久挂死。现在入队也在预算内,满队列
  到点回 `ReverseRpcError::OutboundWedged`(与「等响应超时」= `Timeout` 类型级区分:
  帧根本没上线 vs 节点慢)。
- **卡死连接自动摘除**(慢消费者踢除,映射 openclaw `rejectSlowNodeSocket`):中心侧每
  条连接的 `ReverseRpcChannel` 经 `with_close` 绑一个 close 信号;`call()` 命中入队卡死
  时除回 `OutboundWedged` 外还 `notify` 它,令该连接读循环退出、跑**现有全套** cleanup
  (`cancel_all` + `deregister` + `node.disconnected` 事件 + `touch_device` + 关 socket),
  节点随后退避重连自愈。**为何非踢不可**:入站 idle-watchdog 只看 `last_activity_at`
  (peer 自动 Pong 刷新),对「center→node 写死、node→center 读活」的**半开写卡死**
  永不触发——不主动踢,这条僵尸连接会**无限**占着 registry 一个在线位,`node_list` 报它
  online、发给它的调用全超时。**不误杀**:只在**入队**卡死(容量 64 满满整段预算=强背压
  信号)触发,绝不碰响应超时(长命令落 `Timeout`,安全);且踢除=关连接→节点秒级重连,
  即便偶发误判也自愈,严格优于留着僵尸。节点侧拨出通道用 `new`(无 close 信号),重连由其
  `run_session` 自管。
- 重连安全:`NodeRegistry::register` 同 `node_id` 重连覆盖旧会话、清旧 `conn` 映射,
  **并关掉被顶下去的那条连接**(B1-01);同一 `conn_id` 改用另一个 `node_id` 重新
  announce 则只删掉被孤立会话在两张表里的条目、**不关任何东西**(B1-03)——
  `nodes_by_conn[conn_id] == prev_node_id` 蕴含 `prev.conn_id == conn_id`,那条
  channel 就是**本连接自己的**(中心每条连接只绑一个 `with_close` channel,存进
  registry 的都是它的 clone,共享同一个 `Arc<Notify>`),关掉它等于把正要登记的这条
  会话拆掉。两条臂的这种不对称是刻意的,由 `registry.rs` 的一对对照测试钉住(见
  下方测试清单)。`deregister` 仅当当前会话确属该 `conn_id` 时才移除(旧连接 cleanup
  不误删新会话)。
- **`register` 对两张表的读-改-写是原子的**(2026-09-05 起):整段 read-modify-write
  在**同一个写锁**下完成,要关的 channel 收进 `to_close`、锁释放之后才 fire——
  `forget` 早就是这个形状。此前它在函数中途 drop 写锁去调 `close_connection()` 再重新
  获取,写着的理由是「被通知的连接任务会经 `deregister` 重入 registry」,而这条理由是
  **伪的**:`close_connection` 全部内容就是 `Notify::notify_one`,不拿锁、不阻塞、不碰
  registry 状态,重入发生在稍后的另一个任务上。那个窗口的实际代价见下方记账条目。
- 防伪:`node_identity_by_conn` 从**已认证连接**盖章节点身份,而非信任请求
  params——节点无法冒充别的节点(审批路由用此)。**这条依赖上面那条原子性**:它要求
  `nodes_by_conn` 与 `nodes_by_id` 互为一致(`conn → node_id → session` 且
  `session.conn_id == conn`),而中途放锁的旧实现会**瞬时打破**这个双表不变量。在
  2026-09-05 修掉之前,这句话描述的是**期望**,不是既成事实。

## 线协议速查

| 方向 | 帧 |
|------|----|
| node → center | `connect { device_id?, device_name, commands, tags, version }`(LAN-trust,无 token;首启省略 `device_id`;`version` 仅观测) |
| center → node | connect 回包 `{ result:{ …, node:{ node_id, status:"registered"|"deregistered", persist } } }` |
| center → node | 请求 `{ id, method:"tool.call", params:{ tool, args } }` |
| node → center | 响应 `{ id, result }` / `{ id, error }` |
| node → center | 请求 `{ id, method:"node.approval.request", params:{ tool, reason } }` |
| center → node | 响应 `{ id, result:{ outcome:"approved"|"approved_session"|"timeout"|"denied" } }` |

## RPC 权限与工具权限

### RPC 方法

LAN-trust 下**授权是二元的**:一条连接要么通过 `connect`(loopback 免 token;远程凭
device token / bootstrap ticket / 共享 Gateway token)成为 **operator**,要么被登录墙
挡在外面、只能发 `connect`。**没有** per-method 权限表——`src/gateway/method_authz.rs`
只有 `tool_requires_operator` 一个**工具名**分级器,与 RPC 方法无关。(本文旧版曾列出
一张"`cluster.enroll` = operator / `environments.list` = read"的方法权限表,那张表在
代码里从来不存在。)

因此:`cluster.enroll` / `cluster.deregister` / `environments.list` 三个方法,凡是过了
登录墙的连接都能调。**这不是漏洞而是 LAN-trust 的定义**——同一条连接本来就能凭
`connect` 的参数形状直接把自己注册成节点。信任边界是网络边界(bind / origin),不是
方法表。

| 方法 | 可达性 |
|------|--------|
| `cluster.enroll` | 任何已授权连接(= operator) |
| `cluster.deregister` | 任何已授权连接 |
| `environments.list` | 任何已授权连接(只读投影,绝不含凭证) |
| `exec.approval.resolve` | 任何已授权连接(同样裁决节点审批) |

`environments.list` 是**合并视图**:NodeRegistry 在线会话 + security_store 已登记
(role=node、未吊销)但离线的设备(`status:"offline"`,附 `last_seen_at` Unix 秒,
`null` = 登记后从未连入)。last_seen 在节点 connect/disconnect 两接缝经
`SecurityStore::touch_device`(`gateway/security/store/devices.rs`)盖章——**不是**
`SharedTokenManager`(那是凭据 vault,与设备表无关;旧写法 `TokenManager::touch_device`
指向一个不存在的符号)。

### LLM 工具(`OPERATOR_TOOLS`)

这一层治理的是 **channel**(Telegram / Slack…):`inbound_router` 按
`ChannelPermissionLevel`(默认 Chat ⇒ `guest`)盖 `caller_role`,`ScopedToolService`
据此拒绝 chat-tier 跑 operator 工具。Panel 授权后恒 operator,此闸对它自然全过。

| 工具 | 权限 | 理由 |
|------|------|------|
| `node_list` | **chat-tier 开放** | 只读发现:能列出节点,不能驱动它们 |
| `node_invoke` | **operator** | 远程执行 |
| `node_invoke_many` | **operator** | 一次调用打到中心拥有的每一台机器 |
| `node_file` | **operator** | 跨机器搬字节 |
| `node_manage` | **operator** | 改变舰队成员本身 |

本地 `bash` 是刻意对 chat-tier 开放的,但集群写工具的**爆炸半径**完全不是一个量级
——所以三个写工具收进了 `OPERATOR_TOOLS`,只读的 `node_list` 留在外面。

> LAN-trust 迁移后**已无** `pairing.*` 节点配对方法——节点不再凭 token/配对码接入。

## 相关代码与测试

- **冷启动登记 / 注销粘性(真实 socket)**:`tests/cluster_node_enrollment.rs` —— 首启铸 id、
  重连复用无幽灵行、按名归并 operator 预登记行、注销后重连被拒。**这层覆盖此前是缺的,
  正是"新节点永远登记不上"能长期存活的原因**:单测直接调 handler 函数,绕过了
  "首帧必须是 connect" 这条规则。
- 反向 RPC 不变量:`reverse_rpc.rs` 的 `cancel_all_drops_every_waiter_*` /
  `inflight_call_returns_cancelled_after_cancel_all`(断线 fail-fast)/
  `call_times_out_instead_of_hanging_on_a_wedged_outbound_queue`(满队列不挂死)。
- 登记/重连:`registry.rs` 的 `reconnect_same_node_overwrites_and_old_cleanup_does_not_evict_new`。
- **`register` 原子性**:`registry.rs::concurrent_register_same_node_leaves_no_orphan_conn_mapping`
  ——多线程同 `node_id` 并发登记,断言经**公开**的 `node_identity_by_conn` 只有一条连接解析得出
  身份(守的是"效果到达了",不是"调用发生了")。
- **两条驱逐臂的不对称**:`registry.rs::{reconnect_from_a_new_conn_closes_the_displaced_connection,
  reannounce_under_new_node_id_does_not_close_this_connection}` ——一对**对照**测试:B1-01 断言被顶
  下去的连接的 close 信号**确实**发了、新连接的**没发**;B1-03 让两条会话共享同一个 `Arc<Notify>`
  (镜像 handler 每连接一个 `rpc_close` 的形状)并断言**没发**。同一个 `close_fired` 探针在 B1-01
  测试内部同时给出 true 与 false,即"这个探针能区分"本身也被证明了。
- allowlist 权威:`node_runtime.rs` 的 `dispatch_rejects_unlisted_command`。
- jail containment:`node_file_cmd.rs` 的 `file_write_rejects_traversal`。
- 审批 fail-closed:`node_approval.rs` 的 `outcome_mapping_is_fail_closed` /
  `none_channel_denies` / `transport_closed_denies`。
- **登记幂等 / 注销真源**:`enrollment.rs` 的 `enroll_is_idempotent_across_name_spellings` /
  `enroll_refuses_to_deepen_a_pre_existing_duplicate` / `deregister_is_sticky_and_reaches_offline_nodes`,
  RPC 层 `handlers/cluster.rs::enroll_twice_returns_the_same_node_id`,工具层
  `node_manage.rs::enroll_is_idempotent_by_name`。
- **注销掐断连接(真实 socket)**:`tests/cluster_node_enrollment.rs::deregister_tears_down_the_nodes_live_socket`
  ——**单测钉不住这条线**,测试通道用 `ReverseRpcChannel::new`(无 close 信号)。同一测试顺带钉死
  版本握手落到 `Environment.version`。
- **UTF-8 安全**:`registry.rs::{truncate_on_char_boundary_never_splits_a_scalar,
  ambiguity_labels_survive_non_ascii_node_ids}` / `enrollment.rs::admit_node_survives_a_non_ascii_presented_id`
  (fixture 刻意让第 16 字节落在标量内部)。
- **并发 scope 归一化**:`registry_adapter.rs::node_invoke_claim_folds_spelling_variants_onto_one_scope`。

## 与 openclaw 的对照映射 (Gap Analysis)

> 参考实现:`openclaw` 的 **`src/node-host/`(边缘) ↔ `src/gateway/node-*`(中心)**。
> **注意别走错门**:`openclaw/src/fleet/` 不是分布式节点子系统,那是多租户容器编排
> (Docker/Podman 里跑整个 openclaw 实例),与本文无关。
>
> 本节记录「Aleph 的哪些设计是从 openclaw 映射来的、哪些是有意分道、哪些还欠着」,
> 以便下次改集群时不必重新做一遍对比。锚点均已核到行。

| 维度 | openclaw | Aleph | 判定 |
|------|----------|-------|------|
| 拓扑 | 星型,node 拨出中心,单 WS | 同 | **对齐** |
| **身份在握手回包里交回** | HelloOk 携 `auth.deviceToken`,客户端就地落盘(`packages/gateway-client/src/client.ts:866-876`) | `connect` 回包携 `result.node.node_id`,节点落盘(`cluster::admit_node`) | **映射**——Aleph「登记折叠进 connect」正是此模式;旧实现另开 socket 发 `cluster.enroll` 才是偏离 |
| 未批准/已撤销 → 能力清零 | 配对未批准 ⇒ `effectiveCaps: []`(`src/gateway/node-connect-reconcile.ts:167`) | 设备 `revoked_at` ⇒ `NodeAdmission::Deregistered`,拒绝登记 | **映射**(注销粘性) |
| 反向 RPC 形状 | center 发 **event** `node.invoke.request`,node 回 **req** `node.invoke.result`;关联需 (id, nodeId, connId) 三元组 | 对称 req/res,靠**结构**区分(有 `method`=请求);pending 表天然 per-connection ⇒ connId 隐含 | **分道且更简**(少一层关联键) |
| 断线 fail-fast | `unregister` reject 全部 pending | `PendingInvokes::cancel_all()` | **对齐** |
| 舰队排序 | connected-first → displayName → nodeId(`src/gateway/node-catalog.ts:305-311`) | `handle_environments_list` 同序(online → name → id) | **对齐** |
| 按名寻址归一化 | `node-match.ts::normalizeNodeKey` 已演进为 **Unicode**(NFC + `[^\p{L}\p{M}\p{N}]+ → -`,保留 CJK/带重音字母) | `cluster::normalize_node_key` 单一真源,在线/离线共用,**Unicode 感知**(`char::is_alphanumeric` + `to_lowercase`,保留 CJK/带重音字母;组合记号 `\p{M}` 有意折为分隔符,零依赖) | **对齐**(2026-07-20 收口 ASCII-only 漂移,见下「已闭合」) |
| 重连退避 | 1s → ×2 → 30s,**无 jitter**(`client.ts:1506`;全仓 grep `jitter` 零命中) | 2s → ×2 → 60s,**±25% jitter** + 仅在活过 30s 的会话后重置 | 🟢 **超越**(N 节点不齐步撞门;不对"接受即关闭"的中心热转) |
| 扇出 | **没有**——`invoke()` 只收单个 `nodeId`(`src/gateway/node-registry.ts:606`) | `node_invoke_many` 按 tag AND 并发扇出 + 部分失败容忍 + 结果确定序 | 🟢 **超越**(openclaw 无对应物) |
| 并发调度 | 无(每 invoke 各自 Promise,无资源模型) | `ExclusiveScope::Nodes`——不同机器的 `node_invoke` 并行,同机器串行 | 🟢 **超越**(利用已有 `ConcurrencyClaim` 基建,Rust 侧独有) |
| 出站背压 | `bufferedAmount > MAX_BUFFERED_BYTES` ⇒ **直接关 socket**(`node-registry.ts:894` `rejectSlowNodeSocket`) | 有界 mpsc(容量 64)+ **入队纳入 `timeout_ms` 预算**;入队卡死回 `OutboundWedged` **并触发关连接**(`with_close` 信号→读循环退出→全套 cleanup),节点退避重连 | **对齐**(2026-07-17 闭合;见下「存活性」节) |
| 超时语义 | 解析为 `{ok:false, error:{code:"TIMEOUT"}}`(值) | `Timeout`(等响应超时)与 `OutboundWedged`(入队卡死)**两个类型化 Err**——「节点慢」与「socket 死」不再混为一谈 | 🟢 **超越**(Rust 类型安全 + 比 openclaw 单一 TIMEOUT 更细粒度,调用方/模型可分辨) |
| 取消帧 | **无**(靠中心 deadline + 节点自杀子进程 + 断线) | 无(节点侧 bash 自带 60s 超时兜底) | **对齐** |
| **节点版本握手** | `connect.client.version` 进 NodeSession(`node-registry.ts:202`);其**同机 bundled local node** 版本不符**拒连**(`server.node-version-mismatch.test.ts`) | 节点 connect 帧带 `version`,进 `NodeSession`/`Environment`,`node_list` + Panel 舰队行透出,不一致 `warn!` 一行 | **分道**——采纳观测面,**刻意不拒连**(Aleph 无 local-node 概念,LAN 里滚动升级是常态,按版本拒连＝每次升中心就打掉全部未升节点) |
| **中心侧命令策略** | `node-command-policy.ts` 423 行:平台默认表 + `allowCommands`/`denyCommands` + dangerous 清单(camera/screen/sms/contacts/calendar…) | 无中心侧策略;节点侧 `CommandTable` allowlist 权威 | **有意不移植**——那 423 行治的是**移动端能力面**,Aleph 节点只有 `bash`+`file.*`(R3 纯执行臂);爆炸半径闸已在 `OPERATOR_TOOLS`,再加一层配置是零消费者的旋钮 |
| 节点侧 invoke 超时 | `node-host/with-timeout.ts`(AbortSignal 包裹) | `bash` 自带 60s 默认 / ~170s 前台钳位;`file.*` 有界(8MB) | **对齐**(同一效果,不引入第二套超时机件) |
| **舰队成员写面** | CLI/gateway 方法(`nodes` 命令族),**非 agent 工具** | `node_manage` 工具(enroll/deregister)与 Panel RPC 共用同一对函数 | 🟢 **超越**(R8:对话即管理面板;openclaw 的模型改不了舰队成员) |

### 有意不移植(YAGNI / 红线)

| openclaw 能力 | 为何不移植 |
|---------------|-----------|
| 设备密钥对 + 签名挑战 + 三态配对审批 + SSH 反向验证 | Aleph 信任模型 = **LAN-trust**(网络边界即信任边界),刻意不建认证层 |
| 节点托管 MCP server / skills(`node-host/mcp.ts`, `skills.ts`) | **R3 核心轻量化**:节点是纯执行臂(bash + file),不做能力宿主 |
| 离线工作队列 + APNs 唤醒(`node-pending-work.ts`) | Aleph 节点是常驻 headless 机器,不是会睡眠的手机 |
| `idempotencyKey` | openclaw 自己的 node-host **也完全忽略它**(只用于 pending 去重),移植即死代码 |
| 三段式 exec 审批(prepare → approve → run + 计划绑定) | Aleph 复用**已有** `ExecApprovalManager` + Panel 审批卡,`node.approval.request` 一跳搞定(更薄,R10) |
| 中心侧 `allowCommands`/`denyCommands` 命令策略(`node-command-policy.ts`) | 治的是移动端能力面(camera/sms/contacts…);Aleph 节点只有 `bash`+`file.*`,节点侧 allowlist 已权威、`OPERATOR_TOOLS` 已管爆炸半径 —— 再加一层是零消费者的旋钮(R3/YAGNI) |
| 版本不符**拒连** | 只对 openclaw 同机 bundled local node 成立;Aleph 节点是 LAN 里独立升级的机器,拒连＝每次升中心打掉全部未升节点。**采纳观测面,不采纳拒绝**(见上「版本握手」节) |

### 已闭合(记账)

- **`register` 原子化 / 被证伪的死锁理由(2026-09-05)**:`register` 在两条驱逐臂里都会中途
  drop 写锁去 fire `close_connection()` 再重新获取,理由写作「被通知的连接任务会经 `deregister`
  重入 registry」。**这条理由不成立**:`close_connection` 全部内容就是 `Notify::notify_one`,
  不拿锁、不阻塞、不碰 registry 状态,重入发生在稍后的另一个任务上——那段说明描述的是这段代码
  **没有**的危险,而它正是那个窗口存在的原因。**窗口的实际代价**:B1-01 臂当时只对
  `nodes_by_id` 做 `get`,陈旧会话跨越整个间隙都可见;两条连接并发登记同一 `node_id`(每条 WS
  连接是独立 task,一次网络抖动就让节点在旧 socket 还活着时重拨)会**读到同一个** previous
  session 并各自插入自己的 `conn_id`——输的那条既没被关也没被覆盖,却仍能经
  `node_identity_by_conn`(**每一帧入站节点帧的防伪盖章**)以赢家的身份解析出来。同一个间隙还
  让并发的 `forget` 打出 "evicted"、把会话交还给 operator,然后被 re-insert **静默撤销**。现整段
  read-modify-write 收进**一个**写锁,close 收进 `to_close`、锁释放后才 fire(`forget` 的既有
  形状),B1-01 改用 `remove` 而非 `get`,临界区里不再有一瞬可见的陈旧会话。**B1-03 从此不关任何
  东西**:`nodes_by_conn[conn_id] == prev_node_id` 蕴含 `prev.conn_id == conn_id`,那条 channel
  就是本连接的——旧代码是先叫正在登记的这条连接自我拆除、然后照样把它登记上去,日志却声称新会话
  活着。回归测试 `concurrent_register_same_node_leaves_no_orphan_conn_mapping` 在无修复时**第 1
  轮**(共 500 轮)即红——窗口是宽的,不是罕见的。**同轮顺带修掉的腐烂引用**:`reverse_rpc.rs`
  的「Two producers fire this」(实际三个;改写为**不带任何数目**的表述,因为注释里的数目是一张
  会腐烂的名单)、`register` doc 里引用的过期调用方**行号**(改为指符号)、`forget` doc 里描述的
  它早已不返回的 `bool`、以及 `handler.rs` 里 `node.disconnected` 的 "KNOWN GAP (2026-08-29)"
  段(其修复其实已随 `180f9a0b1` 落地)。锚点 `cluster/registry.rs::{register, forget}`。

- **`cluster.enroll` 幂等化(2026-07-25,自身 bug,非 openclaw delta)**:`handle_cluster_enroll`
  直接调 `mint_node_device`,**无条件铸新 UUID**——而模块 doc 一直声称「与 connect 自助登记
  共用同一真源,故同名 enroll 不会铸重复行」。**声称的不变量在代码里根本不存在**:Panel
  「+ Enroll」双击 / RPC 重试 / 模型问两次,每次都多一条同名 `role=node` 行。伤害是**自我封死**
  的三连:① 第二行让 `admit_node` 的 `reuse_by_name` 变歧义 ⇒ 节点首启既不认领也不复用,去铸
  **第三行**;② `cluster.deregister` 的离线回退要求名字**唯一**命中(`[d] => Some, _ => None`)
  ⇒ 这几行**一条都删不掉**;③ 于是舰队视图里永久挂着 operator 自己造的幽灵行,只能改数据库。
  现新增 `enroll_node_device`(`enrollment.rs`)作为**唯一**预登记入口:`match_by_name` 三态
  (`Unique`/`None`/`Ambiguous`)——唯一命中即复用(`reused:true`)、无命中才铸、**已有重复则报错
  要求先按 id 清理**,绝不再加一行。`mint_node_device` 同时降为**私有**,堵死绕过路径(熵减)。
  锚点 `cluster/enrollment.rs::{enroll_node_device, match_by_name}`。

- **注销当场掐断 socket(2026-07-25,`with_close` 只连了一半)**:慢消费者踢除那轮建了
  `ReverseRpcChannel::with_close` + handler 的 `rpc_close` select arm,但**只有 `call()` 的入队
  卡死会触发它**。`cluster.deregister` 明明要求节点立刻离场,却只驱逐 registry、不碰连接——那条
  socket 要熬到下一次 ping / ≤90s 入站 idle-watchdog。这段窗口里被注销的节点**仍在执行**中心
  先前下发的命令,且 `node.approval.request` 通道**仍然活着**:刚把它踢掉的 operator 还会收到
  它弹的审批卡。现 `NodeRegistry::forget` 直接 `channel.close_connection()`(当时新增的一个
  producer;**producer 清单不在这里维护**——真源是 `reverse_rpc.rs::close_connection` 的 doc
  与 `grep close_connection()`,注释里写死一个数目就是一张会腐烂的名单,那条注释本身后来
  正是这么错的),复用**同一根线**跑现有全套 cleanup。单测看不见这条线(测试通道用 `new`,无 close
  信号),故补真实 socket 集成测试 `deregister_tears_down_the_nodes_live_socket`。

- **`ExclusiveScope::Nodes` 的 key 归一化(2026-07-25,又一次自引 SSOT 漂移)**:
  `registry_adapter.rs` 用**调用方原样 selector** 做并发 scope key,于是同一批次里
  `{"node":"worker-1"}` 与 `{"node":"Worker 1"}` 被判**互不冲突**、并发跑向同一台机器的
  **同一个 bash session workspace**——正是该 scope 引入时要防的那个竞态。现折叠进
  `normalize_node_key`(第 4 个消费者)。**残余(已接受)**:按 name 与按 id 寻址同一节点仍看似
  disjoint——claim 只由 `input` 计算,手上没有 registry 可解析身份;收窄拼写变体是免费的,在这里
  解析身份不是。

- **UTF-8 切片 panic ×2(2026-07-25,P7)**:`enrollment.rs` 的 fingerprint(`&id[..16]`)与
  `registry.rs::candidate_labels` 的短 id(`&node_id[..8]`)都在**字节**切片 node_id。而 node_id
  **不保证是中心铸的 ASCII UUID**——`admit_node` 的「未知 id」分支会采纳对端 connect 帧里带的
  任意 `device_id`。字节 16/8 落在多字节标量内部即 **panic 掉整条连接任务**。现统一走
  `truncate_on_char_boundary`。

- **慢消费者踢除(2026-07-17 闭合,比原设想更彻底)**:openclaw 在出站缓冲超限时
  **主动关掉**坏连接(`rejectSlowNodeSocket`)把坏节点摘掉。此前 Aleph 只做到「调用方
  不再被拖死」(入队有预算),但**卡死的节点 socket 仍占着 registry 一个在线位**——原本
  以为 90s idle-watchdog 会收走,但复核发现 watchdog **只看入站** `last_activity_at`,
  对「center→node 写死、node→center 读活」的**半开写卡死永不触发**,僵尸会**无限**占位。
  原设想的补法(入队超时路径 `NodeRegistry::forget`)只去路由、留着僵尸 socket,且怕
  「一次超时误杀」。**实际采用更彻底也更省的路径**:给中心侧每连接的 `ReverseRpcChannel`
  绑一个 close 信号(`with_close`),`call()` 命中入队卡死时 `notify` 它 → 连接读循环退出
  → 跑**现有全套** cleanup(`cancel_all`+`deregister`+`node.disconnected`+关 socket),
  节点退避重连自愈。**误杀顾虑已解**:只在**入队**卡死(容量 64 满满整段预算 = 强背压)触发、
  绝不碰响应超时(长命令安全),且关连接→节点秒级重连,偶发误判也自愈。**该轮**四个 LLM 工具与
  `NodeRegistry` **零改动**(纯传输层收口)——这是当时那笔 diff 的记账,不是现状:
  一周后的注销掐断、以及 2026-09-05 的原子化,都改到了 `NodeRegistry`。锚点 `cluster/reverse_rpc.rs::with_close` +
  `gateway/server/handler.rs` 的 `rpc_close` select arm。

- **按名寻址 Unicode 化(2026-07-20 收口 ASCII-only 漂移)**:`normalize_node_key` 曾用
  `is_ascii_alphanumeric` + `to_ascii_lowercase`,把所有非 ASCII 字母当分隔符——中文/日文节点名
  (如 "工作站")整段折成**空键** ⇒ ① 按名寻址被空键守卫跳过,`node_invoke`/`node_file`/
  `cluster.deregister` 都够不着;② 更糟:`admit_node` 首启的 `reuse_by_name` 拿到空键返 `None` ⇒
  **每次重连重铸一个新 node_id**,离线幽灵行无限堆积。而其 doc 注释自称是 openclaw
  `node-match.ts::normalizeNodeKey` 的 SSOT 映射,但 openclaw 早已把该函数演进为 Unicode 感知
  (NFC + `[^\p{L}\p{M}\p{N}]+ → -`,其 tests 钉死 `工作站 01→工作站-01`)——Aleph 从**自己引用的
  SSOT 漂移**了(与「慢消费者」不同:那是尚未闭合的已知项,这是静默回归)。现改用 Unicode 感知的
  `char::is_alphanumeric` + `char::to_lowercase`(**零新依赖**):CJK / 带重音的拉丁字母被保留、
  仍可按名寻址(`工作站`→`工作站`、`GPU 工作站`→`gpu-工作站`、precomposed `Café`→`café`)。
  **有意偏差(R3 核心轻量化,不为单一 helper 引 `unicode-normalization`)**:组合记号(`\p{M}`,如
  天城文元音符号 / 分解式重音)当分隔符、不做 NFC——只影响键**外观**不影响可寻址性(归一化对
  query 与库存名**对称**施加,两侧折叠一致即匹配)。三个消费者(`registry.rs::match_id` /
  `enrollment.rs::reuse_by_name` / `handlers/cluster.rs::resolve_enrolled_node`)共用这一 SSOT,
  改一处即全线生效、零消费者改动。回归测试 `registry.rs::{normalize_node_key_is_unicode_aware,
  resolve_cjk_name_is_addressable}` + `enrollment.rs::first_boot_adopts_pre_enrolled_cjk_name_without_churn`
  钉死 CJK 可寻址 + 无重铸 churn。锚点 `cluster/registry.rs::normalize_node_key`。

## 与「一核多端」的边界

| | 一核多端 (R6) | Aleph 集群 |
|--|--------------|-----------|
| 扩展的是 | **I/O 触达**(多渠道/多客户端) | **执行**(多机器) |
| 远端角色 | 渠道/Panel = 纯 I/O 表面 | node = 远程执行臂 |
| 远端是否推理 | 否(推理在 core) | 否(推理在 center) |
| 配置指南 | [`multi_channel`](../guides/multi_channel.md) | [`cluster`](../guides/cluster.md) |
