# Spec · Aleph 集群（单中心非对称节点联邦）

> 2026-06-08 · 状态：设计已批准，待 writing-plans
> 轨道归属：**轨道 2（Aleph 集群）**。与 **轨道 1（壳核分离 / Aleph 专属 Channel，Spec A `2026-06-07-desktop-remote-gateway-design.md`）正交，独立 spec、独立 worktree、不混同。**

## 0. 两条正交轨道（先厘清，避免混淆）

「一核多端，壳核分离；网关互通，权限控制」展开为两条**正交**的轴：

- **轴 1 · 壳核分离（肢体↔大脑的附着）** = Spec A 已落地。壳是纯 I/O 肢体，附着到**一个**核（本机或远程，**二选一互斥**）。本 spec **不改动它**——把壳打磨成 Aleph 自己的专用 Channel（不再寄生 Telegram），壳一次只连一个核。
- **轴 2 · 集群联邦（大脑↔节点的编排）** = 本 spec。一个**中心核**指挥多个**节点**协同工作。

**两轴的唯一交汇点**：集群建好后，"在异地操作集群"= 用**轨道 1** 把专属 Channel 远程连到**中心**。开发期不耦合；运行期 Panel 仅**渲染**中心暴露的环境视图（薄渲染契约，R4 安全）。

> ⚠️ 明确否决的设计：**Model A（Panel 同时连本机+多远端并聚合）已放弃**。它让肢体握多个大脑，违反 R4/R6。"多机统一视图"完全由本 spec 的集群通过"连中心这一条连接 + environments"提供——壳永远只握一个大脑，"一核"在每一层都成立。

## 1. 背景与动机

R6「一核多端」今天只到"多端"（多个 I/O 通道连一个核）。本 spec 把它推进到 **"一核多端 + 一核多体"**：让一个 Aleph 核（中心）编排多台机器（节点）协同工作，同时严守"集群只有一个大脑"。

参考 openclaw 的真实集群模型（**单 Gateway 控制平面 + 多 Node 拨入**，`NodeRegistry` + `environments` 抽象 + `node.invoke` 反向 RPC + events 回推；openclaw 文档明确"does NOT aggregate state across multiple Gateways"），结合 Aleph 红线裁剪而成。

## 2. 锁定决策（brainstorming 已确认）

| 维度 | 决策 |
|---|---|
| 集群拓扑 | **单中心非对称**：1 中心（唯一大脑，持 NodeRegistry + 编排）+ N 节点（纯执行臂，拨入中心）。**否决**对称网格/多主/分布式共识。 |
| 集群入口 | **唯一前门 = 中心**。连节点 ≠ 用集群（节点看不到也不编排集群）。异地操作集群 = 轨道 1 远程连中心。 |
| 身份模型 | **身份 = (你连的核) × (该核给你设备的档位)**。复用现有 pairing + operator/guest + chat/config。无新身份体系。 |
| 能力载荷 | 四类能力坍缩为**同一套双向传输 + 一个 command 目录**：①工具执行 ②能力访问 ③子代理委派 ④事件上报。 |
| LLM 集成 | **通用元工具**：中心 LLM 只看 `environments.list`（自描述，含 command schema）+ `node_invoke(node_id, command, params)`。工具数恒定，不随节点数膨胀。 |
| 记忆 | **中心记忆 = 集群记忆；无分布式共享记忆**。节点子代理用节点本地记忆，结果回流中心。 |
| 节点拨向 | **节点拨出到中心**（NAT 友好），非中心连节点。 |
| 双重身份 | **MVP 禁止**：一台机器是 单机 / 中心 / 节点 三选一。 |
| 集群管理 | **全是工具**（R8）：`cluster.enroll` / `cluster.approve` / `cluster.expose` / `cluster.list`，对话即管理。 |

## 3. 核心模型

```
        [人/专属 Channel]                    ← 轨道 1：连唯一前门(中心)，本机/远程互斥不变
              │ JSON-RPC over WS
              ▼
   ╔══════════════════════════╗
   ║   中心 Aleph Core (大脑)    ║
   ║  Think→Act 循环 (不变)      ║   LLM 看到 2 个集群工具:
   ║   tool: node_invoke        ║     environments.list (读)
   ║   ctx:  environments       ║     node_invoke(node,cmd,params) (写)
   ║  ┌────────────────────┐    ║
   ║  │ src/cluster/        │    ║   NodeRegistry: nodes_by_id / nodes_by_conn
   ║  │  NodeRegistry       │    ║   pending_invokes: 反向 RPC id 关联
   ║  │  环境聚合 + 路由      │    ║
   ║  └─────────┬──────────┘    ║
   ╚════════════│═════════════╝
       中心→节点 │ node.invoke      节点→中心 ▲ events
       (反向RPC) │ (走节点拨入的常开WS) (回推)  │
          ┌──────┴───────┬───────────────┐
          ▼              ▼               ▼
     ┌─────────┐   ┌─────────┐    ┌─────────┐
     │ 节点 B   │   │ 节点 C   │    │ local   │
     │ NodeClient│   │NodeClient│    │(本机也是 │
     │ 拨出→中心 │   │ 拨出→中心 │    │ 一个环境)│
     │ 声明command│  │声明command│   └─────────┘
     │ 执行=本地  │  │执行=本地  │
     │ tool/agent│   │tool/agent│
     └─────────┘   └─────────┘
   节点 = 纯执行臂：收 node.invoke → 调本地工具/agent → 回结果+事件
```

**一句话**：中心 LLM 把"在哪台机器上做事"当成 `node_invoke` 的一个参数；节点把自己**已有的本地工具/agent 能力**接到一条拨出的 WS 上听候差遣。harness 一行不改，集群只是"多了一个工具 + 一个上下文段"。

## 4. 组件与物理落点（P2 高内聚 / R10 不污染 harness）

| 组件 | 位置 | 状态 | 职责 |
|---|---|---|---|
| `NodeRegistry` / `NodeSession` | **新 `src/cluster/`** | 净新增 | 节点注册表（`nodes_by_id` / `nodes_by_conn` 双 Map）、环境聚合、`node.invoke` 反向 RPC + pending 关联、超时清理 |
| `NodeClient`（拨出端） | **`src/cluster/node_client.rs`** | 净新增 | 节点角色：拨出到中心、用 node token 鉴权、声明 command 目录、收 invoke→派发给**本地**工具/agent、回推结果与事件、断线重连 |
| `role:node` 连接分支 | `src/gateway/handlers/auth/connect.rs`（role 赋值点 170/222/383/448）+ `src/gateway/security/device.rs:62`（`DeviceRole::Node` **已存在**） | 复用 | connect 握手识别 node 角色，登记进 NodeRegistry。token 走 `issue_token(device_id, DeviceRole::Node, scopes)` 现成路径 |
| 反向 RPC 编排 + pending 表 | `src/gateway/server/` + `src/gateway/protocol.rs:226-293`（`ToolCallParams`/`ToolCallResult` **类型已存在但未接线**） | 净新增（~100-200 LOC） | 服务端→客户端带 id 的请求/响应关联：`pending: DashMap<(conn_id, req_id), oneshot::Sender>` + 下发 + 路由回 + 超时 |
| `environments.list` / `cluster.*` RPC | `src/gateway/handlers/` | 净新增 | 环境枚举；集群管理（enroll/approve/expose/list）——R8 全是工具 |
| `node_invoke` 元工具 | **`src/builtin_tools/node_invoke.rs`** + 注册于 `src/builtin_tools/mod.rs` | 净新增（<50 LOC + 路由） | 普通 `AlephTool`：`Args{node_id, command, params}`，schema 由 schemars 自动生成；`call()` = 经 NodeRegistry 路由到目标节点 |
| environments 上下文注入 | `src/harness/agent/prompt.rs`（已存在文件，只加**数据段**） | 复用 | 把当前在线环境+能力目录作为**数据**喂给 prompt（R9） |
| 跨机子代理（Phase 2） | 复用 `src/agents/subagent_spawner/`（`SpawnerBase`/`SpawnRequest`/`spawn()`）+ `src/agents/forwarding_trace_sink.rs` | 复用 | 节点侧本地 `spawn()` 跑子代理，进度经节点→中心事件通道流式回推 |
| 集群配置 | 复用 self-config | 复用 | 我是不是中心 / 我 enroll 进了谁 / 我暴露哪些 command |

**R10 红线守住**：`src/harness/` 不增不改逻辑——`node_invoke` 是 tool（builtin_tools），NodeRegistry/反向 RPC 是子系统（cluster + gateway）。harness 仍只做 Think→Act 调度，连"这工具是远程的"都不知道。`prompt.rs` 仅注入数据，不加判断逻辑。

## 5. 节点生命周期（四步，全复用现有 pairing/token/tool 设施）

1. **入伙（enroll）**：节点 B 的 operator 用自然语言"把这台机器加入 `<中心URL>` 集群"→ `cluster.enroll` 工具 → B 的 `NodeClient` 拨出到中心、走**现有 pairing 流程**（`PairingRequest::Device` 路径，新增 `role:node` 语义）→ 中心 operator 批准 → 中心 `issue_token(device_id, DeviceRole::Node, scopes)` 发 node token，B 持久化（与 Spec A `~/.aleph/` 同目录风格）。
2. **声明（declare）**：B 连上后上报**它愿意暴露的 command 目录**（名字 + JSON Schema），来源是 B 本地的工具/skill/能力。**默认 deny，显式 allowlist**（仿 openclaw declaredCommands → approved commands）。
3. **调用（invoke）**：中心 LLM `node_invoke("node:B","bash",{...})` → `node_invoke` 工具 → NodeRegistry 查 B 的 `NodeSession` → 经反向 RPC（`tool.call` 帧 + pending 关联）下发到 B 的常开 WS → B 的 `NodeClient` 派给**本地**工具执行 → 结果带 id 回中心 → pending 表唤醒 → 回到 LLM。长任务流式经事件回推。
4. **感知（events）**：B 的 daemon 事件 / 子代理进度，经同一条 WS 往中心回推，中心按 topic 路由给订阅者（含 Panel 渲染、中心 LLM 主动反应）。

## 6. 安全模型（最高优先级）

`node.invoke` 本质是**按设计的远程代码执行通道**——中心能在节点上跑命令。边界必须收死：

- **节点侧 allowlist 是唯一安全边界**：B 只暴露显式声明的 command，中心**只能**调被批准的。默认 deny。即使中心被攻陷，也只能做 B 允许的事。
- **双向信任**：B 拨入中心 + 持中心签发的 node token = B 信任该中心；中心持 B 的 allowlist = 中心只能在边界内驱动 B。
- **人对中心的档位门控**：触发 `node_invoke` 的人受中心 tiering 约束（operator 可编排集群；chat/guest 默认**只读** `environments.list`，不能 invoke）。
- **敏感 node 操作走审批**：复用 **Phase 2b 的 operator 审批 infra**——某些 node command 标 config 档，调用时挂起等中心 operator 批准。
- **凭证隔离**：node token 绝不复用本机 token；与轨道 1 的"本机 token 绝不外泄给远程"同纪律。
- **传输**：与轨道 1 同样的显式权衡（LAN/Tailscale 明文 http 可接受），spec 记录之。

## 7. LLM 集成（R7 主权 / R9 智慧在 prompt / R10 薄 harness）

中心 LLM 永远只看到 **2 个集群工具**（不随节点数膨胀）：

- `environments.list` → 自描述：每个在线节点的 `id` / `status` / `command 目录`（含 JSON Schema）/ 能力标签。本机本身也是一个环境（`id: "local"`）。
- `node_invoke(node_id, command, params)` → 通用执行入口。

LLM 读 environments 自己拼 invoke——意图理解、选哪台机器、拼参数，全是 prompt 里的推理（R7/R9）。

四类能力的统一表达：
- **能力①工具执行** = `node_invoke("node:B","bash",{...})`
- **能力②能力访问** = `node_invoke("node:B","desktop.screenshot",{...})`
- **能力③子代理委派** = `node_invoke("node:B","agent.run",{task})`，B 本地 `spawn()` 跑 Think→Act，流式回推
- **能力④事件上报** = 节点→中心事件通道（反向），中心订阅 + LLM 主动反应

## 8. 传输 / 反向 RPC（净新增的核心技术点）

现状（验证）：WS = JSON-RPC 2.0，client→server 请求/响应 + server→client 单向通知（EventBus）。**没有** server→client 带 id 的请求/响应关联，**没有** pending 表。`ToolCallParams`/`ToolCallResult`/`ToolCallContext`（`protocol.rs:226-293`）已定义为脚手架但未接线。

需新增：
1. **pending 关联表**：`Arc<DashMap<(conn_id, req_id), oneshot::Sender<JsonRpcResponse>>>`。
2. **服务端下发**：向指定 node 连接发 `{jsonrpc, method:"tool.call"/"node.invoke", params, id}`。
3. **客户端路由回**：node 的 `NodeClient` 收到带 id 的请求 → 执行 → 回 `{jsonrpc, result, id}`；服务端按 id 唤醒 pending。
4. **超时/取消**：未应答请求的 timeout 清理（复用 `ToolCallContext.timeout_ms`）。

## 9. 红线对账

| 红线 | 落地 |
|---|---|
| R1 — 大脑/四肢分离 | 集群是核-核/核-节点联邦，全 Rust。节点的平台能力（如 desktop）仍走其本地 `DesktopCapability` trait + bridge，src 不碰平台 API |
| R3 — 核心轻量化 | 零新重依赖，复用现有 WS/JSON-RPC/DashMap 栈 |
| R4 — Interface 纯 I/O | Panel 只**渲染** environments（薄契约），不聚合、不路由、不持久化 |
| R6 — 一核多端 | 单中心 = 唯一大脑；本 spec 是其"一核多体"延伸 |
| R7 — LLM 主权 | 选哪台机器/拼参数/失败重试，全交 LLM 推理；无确定性意图分类/路由引擎 |
| R8 — 一切皆工具 | 集群管理（enroll/approve/expose/list）+ 节点能力全是工具 |
| R9 — 智慧在 prompt | environments 作数据注入 prompt，LLM 一次推理覆盖判断 |
| R10 — 薄 harness | `src/harness/` 不增逻辑；NodeRegistry/反向 RPC/node_invoke 各归 cluster/gateway/builtin_tools |

## 10. 风险与权衡

1. **`DeviceRole::Node` 语义对齐**：现有变体注释为 "Limited execution (iOS/Android nodes)"（移动端）。本 spec 的"节点"是执行臂机器，需在 plan 阶段确认语义统一/或细分，不要冲突。
2. **与多 Agent 系统的接缝**：跨机子代理（Phase 2）必须**复用** `subagent_spawner`，不另起一套。plan 阶段核对 `SpawnerBase`/`ForwardingTraceSink` 的跨机适配点（网络传输、序列化、远程超时、网络 vs 本地错误面）。
3. **反向 RPC 是净新增**：协议层需扩 server→client 关联调用，~100-200 LOC async 协调，是本 spec 最大技术风险点。
4. **单中心是单点**：节点多了中心成瓶颈。个人助手场景（几台机器）OK；spec 明确**非高可用集群**。
5. **安全面扩大**：新增了一条 RCE-by-design 通道，节点 allowlist 默认 deny 是不可妥协的前提。

## 11. 分阶段（每阶段独立 plan，同一份 spec 定架构）

- **Phase 0 · 集群地基（MVP）**：`role:node` 握手 + 节点 enroll/pairing + `NodeRegistry` + 双向传输（反向 RPC + events 回推）+ `environments.list` + `node_invoke` 元工具 + 节点暴露 `bash`/`code_exec`（能力①）+ 节点侧 allowlist。**验收：中心 LLM 在节点上执行命令并拿回结果。**
- **Phase 1 · 能力扩展**：节点暴露更多 command（desktop / files / mcp = 能力②）。
- **Phase 2 · 跨机子代理**：`agent.run` command + 流式回推（能力③，复用 `subagent_spawner`）。
- **Phase 3 · 跨机感知**：节点 daemon 事件桥接到中心 + 中心主动反应（能力④，R5 跨机版）。

## 12. 测试策略（延续现有风格，纯单元优先）

- `NodeRegistry`：注册/注销/双 Map 一致性/超时清理。
- 反向 RPC：id 关联往返、超时、节点离线时的错误面。
- `node_invoke` 工具：schema 生成、路由到正确 session、节点离线返回错误（LLM 可读）。
- 安全：未在 allowlist 的 command 被拒；chat/guest 档位不能 invoke 只能读 environments；node token 不复用本机 token（connect 帧断言）。
- enroll/pairing：node 角色配对往返、token 签发（`DeviceRole::Node`）。
- `environments.list`：在线/离线节点枚举、能力目录序列化。

## 13. 范围外（YAGNI）

- 对称网格 / 多主 / 分布式共识。
- 跨节点分布式共享记忆。
- 高可用 / 中心故障转移 / 多中心。
- 中心+节点双重身份（一台机器同时是中心又是别人的节点）。
- Model A（Panel 同时连多核聚合）——已明确否决。
- 轨道 1（壳核分离）的任何改动——保持互斥不变。
