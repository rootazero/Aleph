# Spec · Aleph 集群 Phase 0b — NodeRegistry + role:node enroll + environments.list

> 2026-06-08 · 状态：设计已批准，待 writing-plans
> 母 spec：[2026-06-08-aleph-cluster-design.md](./2026-06-08-aleph-cluster-design.md)（集群总架构，定不变的红线与拓扑）
> 前置：Phase 0a 反向 RPC 地基已完成（分支 `feat/cluster-phase0a-reverse-rpc`，未合并）。0b 在**同一 worktree/分支**上往下长。

## 1. 范围与边界

0b 是集群的**中心侧节点登记层**。中心能：

1. 铸 node token（`cluster.enroll`）
2. 接受 `role:node` 连接并登记进 `NodeRegistry`
3. 把在线节点作为「环境」只读暴露给中心 LLM（`environments.list`）

**明确挪走（不在 0b）**：

- ❌ NodeClient 拨出端 → **0c**
- ❌ `node_invoke` 写路径 + 节点上真正执行命令 → **0c**
- ❌ 交互式 pairing enroll（节点发起的 6 位码流程）→ **0c**（有真 NodeClient 驱动时才测得实）
- ❌ allowlist **强制**（默认 deny 门控）→ **0c**（强制点在 invoke；0b 只**存+显示**节点自声明的 command 目录）
- ❌ `local` 环境、Panel 渲染 RPC、`cluster.list`（enrolled-but-offline 全量视图）→ 后续相位

**为什么 enroll 只做 operator 铸券、不做交互式 pairing**：0b 没有 NodeClient，交互式 pairing（节点拨入→中心弹审批→节点收 token 落盘）的发起侧与落盘侧都在 0c。现在写交互式 pairing 的中心半边，等于造一条本相位**诚实测不了**的半截路径，且与 0c 的发起侧分两阶段拼接、接缝易漏。故 0b 采用 operator 主动铸券（pre-provision）这一条可独立闭环、可单测的纵切。交互式 pairing enroll 不是砍掉，是挪到 0c 与 NodeClient 同期落地。

**怎么测（修正——不做重型全栈 WS e2e）**：`GatewayServer::with_config` 的 `token_manager` 默认 `None`（完整 token 鉴权栈只在 binary 的 subsystems builder 里组装），故全栈「裸 WS + node token」e2e 需要拉起整个鉴权子系统，代价过高。改为把 connect→register 接缝抽成**纯可测 helper** `maybe_register_node`，每条链路用单测覆盖（复用现有 `create_test_context()` AuthContext 测试夹具——它带真 `token_manager`）：①connect.rs 真 token 校验后发 `role:"node"`；②`maybe_register_node` 见 node 角色即登记；③NodeRegistry register/deregister/重连安全；④enroll/list handler。全栈 WS 冒烟留到 0c（有真 NodeClient 驱动时做）。

## 2. 组件与物理落点（R10 不污染 harness）

| 组件 | 位置 | 状态 | 职责 |
|---|---|---|---|
| `NodeRegistry` / `NodeSession` / `Environment` / `CommandDescriptor` | **`src/cluster/registry.rs`**（新，紧挨 0a 的 `reverse_rpc.rs`） | 净新增 | 双 Map（`nodes_by_id` / `nodes_by_conn`）、register/deregister、`list_environments()`、`get(node_id)` |
| 共享态挂载 | `src/gateway/server/mod.rs`（+ `probe.rs` 测试构造点） | 复用 0a 套路 | `GatewaySharedState`+`GatewayServer` 加 `node_registry: Arc<NodeRegistry>`，`build_router` 共享同一 Arc |
| **connect role 发射** | `src/gateway/handlers/auth/connect.rs`（Case 1 token 路径，line ~382） | 微调 | **关键接缝**：connect 响应的 `role` 现从 scopes 经 `role_for_permissions` 推（只认 operator/guest）。加一句 `validation.role == DeviceRole::Node → role="node"`，否则 node-role token 永远拿不到 `role:"node"` |
| `role:node` 登记分支 | `src/gateway/server/handler.rs`（ConnectionContext + connect-success 分支 + cleanup） | 复用 0a 接缝 | ConnectionContext 加 `node_registry`（镜像 `reverse_rpc`）；connect 成功且 `role=="node"`：拿 `ReverseRpcChannel` clone + 连接帧 commands → `register()`；断线清理块 `deregister()`（与 0a reverse_rpc 注销并排） |
| AuthContext 挂 registry | `src/gateway/handlers/auth/mod.rs`（`AuthContext` 加字段）+ ~11 个构造点 | 复用 Phase 3a 套路 | 加 `node_registry: Arc<NodeRegistry>`（与 Phase 3a 加 `connections` 同法）。编译器强制更新所有字面构造点（生产 1 处 `subsystems.rs` + ~10 测试点） |
| `cluster.enroll` RPC | **`src/gateway/handlers/cluster.rs`**（新）+ builder 注册 + 加进 `OPERATOR_METHODS` | 净新增 | operator-gated RPC `handle_cluster_enroll(req, ctx: Arc<AuthContext>)`：`Params{node_name}` → `security_store.upsert_device(role=Node)`（**token 表对设备有 FK，必须先建设备行**，见 connect.rs:214）→ `token_manager.issue_token(device_id, DeviceRole::Node, vec!["node"])` → 返回 `{node_id, token, signature}` |
| `environments.list` RPC | **同 `src/gateway/handlers/cluster.rs`** + builder 注册 | 净新增 | read RPC `handle_environments_list(req, ctx: Arc<AuthContext>)`：`node_registry.list_environments()`。chat/guest 可读 |
| 连接帧扩展 | connect params 加可选 `commands: [{name, schema}]` | 微调 | 节点自声明 command 目录，0b 只存只显 |

**纪律**：`src/harness/` 一行不改；gateway 改动面 = server/mod.rs 挂 Arc + handler.rs connect 分支 + connect.rs role 发射 + AuthContext 字段 + 新 cluster.rs handler（与 devices.*/pairing.* 同模式）。

**形态修正（planning 实测推翻 spec 初稿）**：enroll/list 初稿定为 builtin rig 工具（R8），但实测**无任何 builtin 工具持有 `TokenManager`**，而所有凭证/设备操作（devices.*/pairing.*）在现有代码里全是 operator-gated gateway RPC（handler 经 `AuthContext` 直接拿 `token_manager`）。故 0b 把 enroll/list 落为 **gateway RPC**（顺现有模式、布线最小、可立即单测）；**R8 的「LLM 可调用」面（environments 作工具/注入 prompt + node_invoke）挪到 0c 与 node_invoke 一起成体系地做**——那时 LLM 面整体一次成型，更诚实。

## 3. 数据模型

```rust
// src/cluster/registry.rs

/// 一个已连入的节点会话（中心侧视图）。
pub struct NodeSession {
    pub node_id: String,            // = device_id，直接当环境 id
    pub conn_id: String,            // 对应 0a reverse_rpc 表的键，断线清理对账
    pub device_name: String,        // 人类可读名（connect 帧）
    pub channel: ReverseRpcChannel, // 0a 通道 clone —— 0c 的 node_invoke 经它下发
    pub declared_commands: Vec<CommandDescriptor>, // 节点自声明目录，0b 只存只显
    pub connected_at: i64,          // Utc::now().timestamp()
}

/// 节点声明的一个 command（名字 + 自描述 schema）。0b 不解析 schema，原样透传。
#[derive(Clone, Serialize, Deserialize)]
pub struct CommandDescriptor {
    pub name: String,               // e.g. "bash"
    pub schema: serde_json::Value,  // JSON Schema，节点提供，中心原样收下
}

/// environments.list 的对外序列化视图（薄渲染契约，R4）。
#[derive(Serialize)]
pub struct Environment {
    pub id: String,                 // = node_id
    pub name: String,               // = device_name
    pub status: &'static str,       // 0b 恒 "online"（在线才在表里）
    pub commands: Vec<CommandDescriptor>,
    pub connected_at: i64,
}

pub struct NodeRegistry {
    inner: RwLock<RegistryInner>,   // P7：lock 中毒 into_inner
}
struct RegistryInner {
    nodes_by_id: HashMap<String, NodeSession>,   // node_id → session（权威）
    nodes_by_conn: HashMap<String, String>,      // conn_id → node_id（断线反查）
}
```

**方法面**：

- `register(session)` → 双 Map 写入（同 node_id 重连=覆盖旧会话）
- `deregister(conn_id)` → 经 `nodes_by_conn` 反查 node_id，两 Map 同删
- `list_environments() -> Vec<Environment>` → 快照投影
- `get(node_id) -> Option<...>` → 给 0c node_invoke 取 channel（0b 建好接口，自己不调）

**两个细节决策**：

1. **重连即覆盖**：同 node_id 再连入直接覆盖旧 session（旧 channel 句柄自然失效）。不做"拒绝重复连接"。
2. **status 0b 恒 online**：离线节点不在表里（在线才登记）。enrolled-but-offline 全量视图（`cluster.list`）留后续。

## 4. 生命周期（三步）

```
① 铸券 (enroll)              ② 连入登记 (connect)               ③ 列举 (list)
operator 对中心说              模拟节点开 WS，connect 帧带:         中心 LLM/operator 调
"加个节点叫 worker-1"          { role:"node", token:<上步铸>,      environments.list
  → cluster.enroll{node_name}   device_name:"worker-1",            → list_environments()
  → 复用 device store             commands:[{name,schema},…] }      → [{id,name,status:"online",
    + issue_token(Node)         → handler.rs connect 分支              commands,connected_at}]
  → 返回 token 串                  验 token → role==Node →           ← 节点带目录出现
   (operator 拷到节点机)            register(NodeSession{channel.clone(),
                                      commands,…})
                                 断线 → deregister(conn_id) → 列表消失
```

**复用点**：①enroll 全走现有 device store + `issue_token`（和 pairing 审批铸设备同一套，role=Node、非交互）。②复用 0a 已建好的 per-connection `ReverseRpcChannel`，登记分支顺手 clone 一份进 NodeRegistry。

## 5. 安全姿态（显式声明）

- **`cluster.enroll` = operator-only**：铸 node token 是签发凭证的敏感操作，进 `method_authz::OPERATOR_METHODS`（与 `devices.set_level`/`pairing.approve` 同表门控）。chat/guest 调不动。
- **`environments.list` = read（不进 OPERATOR_METHODS）**：chat/guest 可读环境视图，但响应只含 `id/name/status/commands/connected_at`，**绝不含 token 或任何凭证**。
- **⚠️ allowlist 强制本期不做（显式 deferral，非安全缺口）**：0b 只存+显示节点自声明 command 目录，不做"默认 deny + 只能调批准命令"的强制。强制点在 invoke，而 invoke 在 0c。**0b 落地后中心仍无法在节点上跑任何命令**（无 node_invoke），这条 RCE-by-design 通道在 0b 阶段**尚未打开**；allowlist 强制与 node_invoke 在 0c 同期落地、同期测。
- **node token 隔离**：经 `issue_token` 独立签发，绝不复用本机/operator token。

## 6. 测试策略（纯单元优先）

| 层 | 测试 |
|---|---|
| `NodeRegistry` 单元 | register/deregister 双 Map 一致性；同 node_id 重连覆盖；**重连后旧连接 cleanup 不误删新会话**；deregister 经 conn_id 反查正确删；`list_environments` 快照投影；lock 中毒 into_inner 不 panic |
| `CommandDescriptor`/`Environment` serde | 往返；schema `Value` 原样透传不丢字段 |
| `maybe_register_node` glue | role=="node" → 登记带 channel+commands 的 session；role 非 node → 不登记；返回是否登记 |
| `connect.rs` role 发射 | 经 `create_test_context()` 铸 Node token → `handle_connect` → 断言响应 `role=="node"`（真 token 校验，无 WS） |
| `handle_cluster_enroll` | upsert_device(Node)+issue_token 铸 token；返回的 token 经 `validate_token` 验出 `DeviceRole::Node`；method_authz 认 `cluster.enroll` 为 operator-only |
| `handle_environments_list` | 空注册表返空；node_registry 登记后返带目录节点；响应不含凭证字段 |

## 7. 红线对账

| 红线 | 0b 落地 |
|---|---|
| R1 大脑/四肢分离 | 全 Rust，无平台 API |
| R3 核心轻量化 | 零新重依赖，复用 WS/JSON-RPC/device store/issue_token |
| R4 Interface 纯 I/O | `environments.list` 薄渲染契约（投影快照），NodeRegistry 不聚合不路由不持久化业务 |
| R7 LLM 主权 | enroll/list 是工具，LLM 对话驱动；无意图分类/路由引擎 |
| R8 一切皆工具 | 0b：enroll/list 为 operator/read gateway RPC（凭证操作的既有模式）。LLM-callable 工具面（environments 作工具/注入 prompt + node_invoke）随 0c 一起成体系落地 |
| R10 薄 harness | `src/harness/` 一行不改；NodeRegistry 在 cluster/，RPC handler 在 gateway/handlers/cluster.rs，gateway 只动 connect/handler 接缝 |

## 8. 范围外（YAGNI）

- node_invoke / 节点上执行 / allowlist 强制（→ 0c）
- 交互式 pairing enroll（→ 0c）
- `local` 环境、`cluster.list` 全量视图、Panel 渲染 RPC（→ 后续）
- 节点心跳/健康监控、离线状态机（0b：在线才在表里）
