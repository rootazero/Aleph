# Phase 0c-core: Node Runtime + node_invoke + Allowlist — Design

> Aleph 单中心非对称集群的**执行竖切**：一个节点进程拨出到中心、用 node-token
> 认证、服务中心发来的 `tool.call`、在自己机器的 sandbox 里跑 bash、回传结果。
> 消费 Phase 0a（反向 RPC 传输）+ Phase 0b（NodeRegistry + node-token enroll）。

- **Date**: 2026-06-08
- **Phase**: 0c-core（执行竖切；交互式配对 enroll 拆到独立 spec `0c-pairing`）
- **Branch**: 新 worktree + 新分支（如 `feat/cluster-phase0c-core`），从
  **0a+0b 的 tip `71566d0b6`** 切出（0c 硬依赖 0a/0b 的类型，必须以其为 base，
  不能基于尚无 0a/0b 的 main）。0a+0b 由另一 session 以 `--no-ff` 合并进 main +
  清理旧 worktree（`Aleph-wt-cluster-phase0a`）；因 `--no-ff` 保留原始 SHA 为
  祖先，本 0c 分支等 0a+0b 进 main 后再合 main 仍以 `71566d0b6` 为共同祖先 →
  干净合并，两 session 互不踩。
- **Depends on**: 0a `ReverseRpcChannel`/`PendingInvokes`；0b `NodeRegistry`
  （`get()` 接缝）、node-token enroll（operator-mint）、`environments.list`、
  `CommandDescriptor`、`NodeSession.declared_commands`。

---

## §1 Scope

**做什么（一条端到端可测试链路）：**

中心 LLM 调 `node_invoke(node, command, args)` → 中心查 `NodeRegistry` 取节点通道
→ 中心侧 fail-fast 校 `declared_commands` → 反向 RPC `tool.call` 发给节点 →
节点 dispatcher 查命令表（allowlist 权威）→ 命中则在节点本机 sandbox 跑 bash →
结果沿反向 RPC 回传 → `node_invoke` 把结果交还 LLM。

**节点形态：** `aleph-server` 的 `node` 子命令，纯执行臂——无 DB、无 harness、
无 LLM、无持久化。拨出 + 认证 + 服务 `tool.call` + 本机 sandbox 跑 bash + 断线
重连。复用同一二进制（R6）。

**本 spec 明确不做（推迟）：**
- 交互式配对 enroll（节点无 token 拨入 → 6 位码 → operator 批准）→ 独立 spec
  `0c-pairing`。0c-core 用 0b 的 operator-mint token 即可跑通。
- 多命令（python 等）：dispatcher 通用查表，但 0c 只注册 `bash`。加第二命令是
  插表，不在本 spec。
- 节点侧文件传输、流式 tail、PTY/交互 stdin。
- 中心向多节点广播 / fan-out。
- 节点侧自建 sandbox 的高级配置（用 server 默认 sandbox 构造路径）。

---

## §2 Component Map

### 节点侧（greenfield）

| 文件 | 职责 | 状态 |
|------|------|------|
| `src/bin/aleph-server/cli.rs` | 加 `Node { action: NodeAction }` + `NodeAction::Start { center, token, name? }` | 改 |
| `src/bin/aleph-server/commands/node.rs` | 节点主循环：拨出 → connect 握手（带 token + 声明命令）→ 入站循环 → dispatch `tool.call` → 写回响应 → 断线指数退避重连 | NEW |
| `src/bin/aleph-server/commands/mod.rs` | `pub mod node;` + dispatch 到 `node` arm | 改 |
| `src/bin/aleph-server/main.rs` | `async_main` 加 `Command::Node` match arm（async，在 tokio runtime 内） | 改 |
| `src/cluster/node_runtime.rs` | 节点侧 dispatcher：`CommandTable`（allowlist=keys）+ `dispatch(method, params) -> JsonRpcResponse`；bash 唯一注册项委托 `BashExecTool::call_json` | NEW |

### 中心侧

| 文件 | 职责 | 状态 |
|------|------|------|
| `src/builtin_tools/node_invoke.rs` | `NodeInvokeTool { node_registry }`，args `{node, command, args, timeout_ms?}`，name/id 寻址，fail-fast 校 `declared_commands`，经 `channel.call` 反向 RPC | NEW |
| `src/executor/builtin_registry/registry.rs` | 注册 `node_invoke_tool`（注入 `node_registry` clone） | 改 |
| `src/gateway/handlers/auth/mod.rs` | `ConnectParams` 加可选 `commands: Option<Vec<CommandDescriptor>>` | 改 |
| `src/cluster/mod.rs` | `mod node_runtime;` + 必要 `pub use` | 改 |

### 复用现成（不改）

- `src/cluster/reverse_rpc.rs` — `ReverseRpcChannel::call(method, params, timeout_ms)`（0a，生产就绪）。
- `src/cluster/registry.rs` — `NodeRegistry::get()`（0b 预留接缝，本 spec 首个消费者）；`maybe_register_node` 已读 `params["commands"]`，只差 `ConnectParams` 的结构化字段。
- `src/builtin_tools/bash_exec.rs` — `BashExecTool::call_json(Value) -> Result<Value>`（standalone，无需 harness）。
- `src/sandbox/` — server boot 的 sandbox 构造路径，节点复用。

---

## §3 Data Model & Wire Protocol

### 反向 RPC 线协议（沿用 0a）

中心 → 节点请求帧：
```json
{ "jsonrpc": "2.0", "id": "rpc-N", "method": "tool.call",
  "params": { "tool": "<command>", "args": { /* 命令参数 */ } } }
```
节点 → 中心响应帧：`JsonRpcResponse`（`result` 成功 / `error` 失败）。结构区分
请求 vs 响应（有 `method`=请求；有 `result`/`error`=响应），id 关联（0a 既有）。

### `node_invoke` 工具参数

```rust
struct NodeInvokeArgs {
    node: String,                    // 节点 name 或 id（先试 name 再试 id）
    command: String,                 // 节点声明的命令名（0c 仅 "bash"）
    args: serde_json::Value,         // 该命令的 JSON 参数，原样透传给节点
    timeout_ms: Option<u64>,         // 反向 RPC 通道超时，默认 120_000
}
```
`command` → 线协议 `params.tool`；`args` → 线协议 `params.args`。
返回：节点执行结果（`bash` 即 `CodeExecOutput` 的 JSON）。

### connect 握手扩展

`ConnectParams` 加 `commands: Option<Vec<CommandDescriptor>>`。节点 connect 时声明
自己的命令表 keys（0c 即 `[{name:"bash", schema:…}]`）。`maybe_register_node`
已从 `params["commands"]` 反序列化 `Vec<CommandDescriptor>` 写进 `NodeSession`，
新结构化字段只是给它一个类型化来源。

### 节点身份

节点只传 `--token`；服务端 token 校验产出设备身份（0b enroll 铸的 node_id +
name）。节点不必单独传 device_id——node-role 连接从已校验 token 取身份，非
params。`--name` 可选覆盖（默认主机名），仅作连接显示用。

---

## §4 Lifecycle

### 节点启动 → 服务 → 断线重连

```
aleph-server node start --center wss://host/ws --token <node-token> [--name worker-1]
  1. 构造节点 sandbox（复用 server boot 路径）+ CommandTable（注册 bash）。
  2. 重连循环（指数退避，初始 ~2s，上限 ~60s）:
     a. connect_async(center) → 失败则退避重试。
     b. 发 connect 握手帧: {method:"connect", params:{token, device_name, commands}}。
     c. 读 connect 响应（认证失败 → 记日志，退避重连）。
     d. 认证成功后进入入站循环:
        - 读帧 → serde 解析。
        - 是请求（有 method）且 method=="tool.call":
            dispatch(params) → 写回 JsonRpcResponse（id 回填）。
        - 其它帧忽略。
     e. 连接断开（read 返 None/Err）→ 跳回 (a) 退避重连。
```

### 中心 node_invoke 一次调用

```
node_invoke(node, command, args, timeout_ms?)
  1. NodeRegistry.get(node)  —— 先按 name 命中在线节点，再按 id；都无 → 错误
     "node '<node>' not online".
  2. fail-fast: command ∈ session.declared_commands？否 → 错误
     "command '<command>' not declared by node '<node>'"（不拨出）。
  3. channel.call("tool.call", {tool: command, args}, timeout_ms.unwrap_or(120_000)).
  4. 反向 RPC 结果:
     - Ok(JsonRpcResponse 成功) → 返回其 result 给 LLM。
     - Ok(错误响应) → 把节点错误透传给 LLM。
     - Err(Timeout/TransportClosed) → 返回清晰错误（通道超时/连接已断）。
```

### 节点侧 dispatch

```
dispatch(params: {tool, args}) -> JsonRpcResponse
  1. tool = params["tool"]；allowlist 权威: tool ∈ CommandTable.keys()？
     否 → JsonRpcResponse::error("command '<tool>' not permitted on this node").
  2. 命中 → CommandTable[tool].call_json(params["args"]).await。
  3. Ok(v) → success(v)；Err(e) → error(e)（节点本机执行错误，含 sandbox 拒绝）。
```

---

## §5 Security

- **节点凭证独立**：node-token 是 0b 铸的 `DeviceRole::Node` 凭证，绝不复用
  operator/local token。连接经 token 校验产出 `role="node"`（0b connect.rs）。
- **allowlist 节点侧权威**：节点 dispatcher 是唯一权威闸门——收到 `tool.call`
  先校 `tool ∈ CommandTable.keys()`，不在则拒，**无论中心发什么**。节点对自己跑
  什么有主权（R1 四肢自治）。
- **中心侧 fail-fast 是 UX 防御**：`node_invoke` 校 `declared_commands` 只为早报错
  + 不让 LLM 反复试节点跑不了的命令；不替代节点侧权威。防御纵深。
- **sandbox 约束节点 bash**：节点 bash 经 `BashExecTool` 在节点本机 sandbox 内执行，
  受节点自己的 sandbox 策略约束（命令策略、路径 denylist 等沿用 server 默认）。
- **token 经 --token/env 传入**：不持久化到节点磁盘（节点无 DB）。`ALEPH_NODE_TOKEN`
  环境变量为备选传入路径，避免进程列表泄漏。
- **无凭证回流**：`environments.list`（0b）与 `node_invoke` 结果都不含 token。

---

## §6 Testing Strategy

与 0a/0b 一致：偏可测胶水函数 + in-memory 通道，避免重 WS e2e 的脆弱。

**节点侧 dispatcher（`node_runtime.rs` 单测）：**
- 命中 bash → 经 `MockSandbox` 跑通，返回 success（复用 `sandbox::test_util::MockSandbox`）。
- 未命中（allowlist）→ 返回 error，不执行。
- bash 执行错误 → 透传为 error 响应。

**中心 `node_invoke` 工具单测（in-memory `ReverseRpcChannel`）：**
- name 寻址命中 / id 寻址命中 / 都不命中 → "not online" 错误。
- fail-fast：未声明命令 → 不拨出，返回 "not declared" 错误。
- 经 `ReverseRpcChannel::new(out_tx)` + 后台 task 扮节点 dispatcher → 端到端
  resolve（复用 0a `reverse_rpc.rs` 测试范式）。
- 通道超时 → Timeout 错误透传。

**一条集成测试（`tests/cluster_node_runtime.rs`，复用 `cluster_reverse_rpc.rs` 骨架）：**
- 真 `connect_async` 节点逻辑拨入中心（`AuthMode::None` 隔离传输）→ 中心从
  `reverse_rpc` 取通道 `call("tool.call", {tool:"bash", args:{cmd:"echo hi"}})`
  → 节点 dispatcher 经（真或 mock）sandbox 跑 → 拿回结果。

**最终验证：** `cargo test -p alephcore --lib cluster::`、`node_invoke`、
`cargo build -p alephcore --bin aleph-server`、`cargo clippy -p alephcore --lib`。

---

## §7 Redline Reconciliation

- **R1（大脑四肢分离）**：节点是纯执行臂——无业务 UI、无推理、无记忆检索、
  无任务规划。只收 `tool.call` 跑 sandbox。✓
- **R4（Interface 纯 I/O）**：`node_invoke` 工具 + 节点 dispatcher 都是 I/O 翻译
  （JSON-RPC ↔ 工具调用），无业务逻辑。✓
- **R6（一核多端）**：节点复用 `aleph-server` 子命令，中心是唯一大脑；节点不参与
  业务推理。✓
- **R7（LLM 主权）**：dispatcher 与 allowlist 是确定性查表/集合判定，非推理；命令
  选择由中心 LLM 经 `node_invoke` 做。无确定性意图引擎。✓
- **R8（工具即一切）**：`node_invoke` 是 LLM 可达工具，自然语言驱动远程执行。✓
- **R10（薄 harness）**：零改动 `src/harness/`。全部落在 `src/cluster` +
  `src/builtin_tools` + `src/bin`。✓
- **P4（依赖倒置）**：节点 `CommandTable` 持 `Arc<dyn NodeCommand>`，bash 是一个
  实现；中心 `NodeInvokeTool` 依赖 `NodeRegistry`（0b 既有抽象）。✓

---

## §8 YAGNI / 撤回模式

- dispatcher 通用查表但 0c **只注册 bash**——不为假想的 python/file-transfer 预建
  trait 层级以外的东西。`Arc<dyn NodeCommand>` 是最小可扩展接口（加命令=插表）。
- 不建节点侧配置文件 / 节点 DB（token 经 flag/env，身份经 token 校验）。
- 不建中心→多节点广播、负载均衡、节点健康探活（`environments.list` 的在线状态
  即足够；探活留到有真实消费者）。
- 重连退避是节点常驻所必需（非投机）；上限 60s 防忙等。
