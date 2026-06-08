# Aleph 集群 Phase 0c-pairing：交互式配对 enroll 设计

**日期**: 2026-06-08
**状态**: 设计已批准，待写实施计划
**前置**: Phase 0a（反向 RPC）+ 0b（NodeRegistry + `cluster.enroll` + `environments.list`）+ 0c-core（NodeClient 拨出 + `node_invoke` + node-side bash + allowlist），均已合入 main。

---

## 1. 目标与范围

让集群**节点无需手动拷贝预铸 token** 即可加入中心：节点首次启动时无凭证，自动拨向中心发起**交互式配对**，中心产生 6 位配对码，operator 在 Panel 通知卡或 CLI 点 Approve 后铸 **node-role** token，节点领取并**本地持久化**，随后落回 0c-core 的 `run_session` 重连循环正常服务。

**定位**：0c-pairing 是对 0c-core `cluster.enroll`（operator 预铸 token + 手动拷贝）的**补充而非替代**。`cluster.enroll` 适合脚本化/自动化供给；0c-pairing 适合交互式零拷贝引导。两条路径共存。

**核心复用**：几乎完全复用成熟的 `browser` 配对流程（匿名发起 → 6 位码 → `PairingRequested` 事件 → operator `pairing.approve` → 匿名 `pairing.poll` single-use 领凭证）。唯一实质差异：铸 node-role token 而非 chat-tier device token。

### 范围内

- 中心侧：`PairingRequest::Node` 变体、`request_node_pairing`、`handle_pairing_start_node`、approve handler 的 Node 臂（铸 node-token）、unauth allowlist 扩展。
- 节点侧：`--token` 变可选、三态启动（持久化凭证 > `--token` > 配对）、`run_pairing` 配对流程、`~/.aleph/node/<name>.json` 持久化读写。
- **凭证失效自动回落配对**：节点 `connect` 收到 `AUTH_FAILED` 时清除本地凭证并回落 `run_pairing`（闭环运维：operator 吊销节点后节点自动重新配对，而非陷入无效重连循环）。

### 范围外（follow-up）

- 节点配对专属 Panel 卡片样式：复用通用 `PairingRequested` 卡，不做专属。
- QR / 带外验证：节点 headless，stdout 打印码足够。
- 多 operator 审批路由：沿用现有 operator-gated `pairing.approve`。
- 配对码节点侧重试（poll rejected/expired 后自动重发 start_node）：本期 rejected/expired 即退出，需人工重启。

---

## 2. 组件清单（文件级）

### 中心侧（`alephcore`）

**`src/gateway/security/pairing.rs`**
- 新 `PairingRequest::Node { code, node_name, expires_at, .. }` 变体（镜像 `Browser`）。
- `From<PairingRequestRow>` 映射 `pairing_type == "node"` → `PairingRequest::Node`（现有 `from` 的 match 加一臂）。
- `code()` / `expires_at()` 的 match 各加 `Node` 臂。
- `request_node_pairing(&self, node_name: &str) -> Result<(String, i64), PairingError>`：镜像 `create_browser_pairing`，`pairing_type: "node"`，复用 `max_pending` 上限与 `generate_unique_browser_code`（码生成与类型无关）。
- `poll_node_pairing(&self, code: &str) -> PollState`：镜像 `poll_browser_pairing`，唯一差异是 Pending 判定改为 `row.pairing_type == "node"`。side-table（`approved_browser_sessions` / `rejected_browser_codes`）类型无关，直接复用其 `record_browser_credential` / `mark_browser_rejected` / `gc_browser_state`。

  > **决策**：复用既有 side-table 而非新建并行表。side-table 仅以 code 为键存 `(token, device_id)`，与 pairing 类型无关；只有 DB-row 的 Pending 检查需类型区分。`poll_node_pairing` 是薄包装，避免改 `poll_browser_pairing` 签名（OCP，不碰 browser 调用点）。

**`src/gateway/handlers/auth/pairing.rs`**
- `handle_pairing_start_node(request, ctx) -> JsonRpcResponse`：匿名 RPC，镜像 `handle_pairing_start_browser`。读 `{node_name}` → `request_node_pairing` → emit `GatewayEventFrame::PairingRequested { device_name: node_name }` → 回 `{code, expires_at, expires_in_secs}`。
- approve handler 的 `match &pairing_request` 加 `PairingRequest::Node { node_name, .. }` 臂：
  - `upsert_device(DeviceUpsertData { role: DeviceRole::Node.as_str(), scopes: &["node"], public_key: &[0u8;32], .. })`（镜像 `cluster.enroll`：先 upsert 设备满足 token 表 FK）。
  - `issue_token(&device_id, DeviceRole::Node, vec!["node"])`。
  - `record_browser_credential(code, &format!("{token}:{signature}"), &device_id)`（合并 `token:signature` 串，与 browser 一致）。

  > **凭证格式决策**：`connect` 期望 `token` 参数是合并的 `"{token}:{signature}"` 串（`connect.rs:378` `split_once(':')`）。browser 分支正是把合并串交给 poll，节点领到后**原样**作为 bearer 发 connect，无需拆分。沿用此格式，poll 应答 schema 与 connect 输入完全一致，也与 0c-core `--token`（本就该是合并串）语义对齐。

**`src/gateway/server/handler.rs`**
- `allow_unauth_browser_pairing` 扩展：`matches!(method, "pairing.start_browser" | "pairing.start_node" | "pairing.poll")`。`pairing.poll` 已在内，被节点配对复用。

  > 节点配对天然远程（集群节点在异机），与 browser 配对同属「远程可达匿名 RPC」类别，不受 loopback 限制——与现有 `allow_unauth_browser_pairing` 语义一致。

### 节点侧（`aleph-server` bin）

**`src/bin/aleph-server/cli.rs`**
- `Node.token`: `String` → `Option<String>`（去掉必填，保留 `--token` 与 `ALEPH_NODE_TOKEN` env）。

**`src/bin/aleph-server/commands/node.rs`**
- `NodeCredential { node_id, bearer, center }` 结构 + serde，其中 `bearer` 是合并的 `"{token}:{signature}"` 串（poll 应答的 `token` 字段，直接作为 connect 的 `token` 参数）；`credential_path(name) -> PathBuf`（`dirs::home_dir().join(".aleph").join("node").join(format!("{name}.json"))`）；`load_credential` / `store_credential`（父目录 `create_dir_all`）。
- `handle_node(center, token: Option<String>, name)` 三态，统一解析出一个 `bearer: String`：
  1. `load_credential(&name)` 有 → 用 `cred.bearer`。
  2. 否则 `token` 有 → 用 `--token`（0c-core 预铸路径；`--token` 值本就是合并 bearer 串）。
  3. 否则 → `run_pairing(&center, &name, &declared).await?` 拿 `NodeCredential` → `store_credential` → 用 `cred.bearer`。
- `run_pairing(center, name, declared) -> Result<NodeCredential>`：匿名 `connect_async` → 发 `pairing.start_node {node_name, commands}` → 读 `{code}` → stdout 打印「配对码 {code}，请在中心 Panel/CLI 批准」→ 每 2s 发 `pairing.poll {code}` → match status：`approved` 取 `token` 串作 `bearer` → 返回 `NodeCredential`；`rejected`/`expired` → `Err`；`pending` → 续 poll。
- **失效回落**：`run_session` 返回类型从 `Result<()>` 改为 `Result<SessionOutcome>`，其中 `SessionOutcome::AuthFailed`（解析 connect 回复，`error.code == AUTH_FAILED == -32001`）/ `SessionOutcome::Ended`。`handle_node` 主循环收到 `AuthFailed` → 删持久化凭证 → 回落 `run_pairing` 重新引导；其余错误 → 现有退避重连。

  > `run_session` 当前 `node.rs:82` 丢弃 connect 回复（`let _connect_resp`）。改为解析回复区分鉴权失败与瞬时错误。

---

## 3. 数据流

### 首次引导

```
节点启动 (load_credential 空 / 无 --token)
 → 匿名 WS connect_async 到 {center}/ws
 → 发 pairing.start_node {node_name, commands}
 → 中心: request_node_pairing → insert pairing_type="node" 行
        + emit PairingRequested {device_name: node_name}
        → 回 {code, expires_in_secs}
 → 节点 stdout: "配对码 123456 — 请在中心 Panel 通知卡或 `aleph pairing approve 123456` 批准"
 → 节点循环每 2s 发 pairing.poll {code}
 ┌─ operator 点 Approve (Panel 卡 / CLI)
 │   → pairing.approve {code}
 │   → approve handler Node 臂:
 │       upsert_device(Node) + issue_token(Node,["node"])
 │       + record_browser_credential(code, "token:sig", node_id)
 └─ 节点 poll 收到 {status:"approved", token:"token:sig", device_id}
 → NodeCredential{node_id, bearer:"token:sig", center}（token 串原样作 bearer，不拆分）
 → store_credential → ~/.aleph/node/<name>.json
 → 关闭匿名 WS
 → 落回 run_session 重连循环 (bearer 作 connect 的 token 参数 → NodeRegistry 注册 → 服务 tool.call)
```

### 后续重启

```
节点启动 → load_credential 命中 → 直接 run_session（无 operator 介入）
```

### 凭证失效回落

```
run_session connect → 中心回 AUTH_FAILED (operator 已吊销 node device)
 → SessionOutcome::AuthFailed
 → handle_node 删 ~/.aleph/node/<name>.json
 → run_pairing 重新引导（回到「首次引导」）
```

---

## 4. 错误处理

- poll **rejected / expired** → `run_pairing` 返 `Err`，节点打印明确错误并退出（非无限重试，避免反复骚扰已拒绝节点的 operator；需人工重启重配）。
- 配对码 TTL 到期未批准 → poll 收 `expired` → 同上退出。
- 匿名 WS 在配对中断开 → `run_pairing` 返 `Err` → `handle_node` 按现有退避重连语义重试 `handle_node` 入口（注意：退避重连发生在 `run_pairing` 之外，配对自身不内置退避，保持配对流程线性可读）。
- 持久化写失败（磁盘/权限）→ 打印 warn 但继续用内存中的 token 服务（不因持久化失败阻断引导；下次重启会重新配对）。
- 复用 `PollState` 四态（Pending/Approved/Rejected/Expired），不新增状态机。

---

## 5. 测试

**`src/gateway/security/pairing.rs` 单测**
- `request_node_pairing` 产唯一码 + 写 `pairing_type="node"` 行。
- `poll_node_pairing` 三态：Pending（行在）→ Approved（record 后 drain single-use，二次 poll Expired）→ Rejected。
- `From<row>` 把 `"node"` 行映射为 `PairingRequest::Node`。

**`src/gateway/handlers/auth/pairing.rs` 单测**
- `handle_pairing_start_node` 产码 + emit `PairingRequested`。
- approve Node 臂铸 token 且 `validate_token` 结果 `role == DeviceRole::Node`、`scopes` 含 `"node"`。
- start_node → approve → poll 端到端拿到 `approved` + token 串，二次 poll `expired`（single-use）。

**`src/gateway/server/handler.rs` 单测**
- `allow_unauth_browser_pairing("pairing.start_node") == true`；`approve`/`list` 仍 false。

**`src/bin/aleph-server/commands/node.rs` 单测**
- `NodeCredential` serde round-trip。
- `credential_path` 拼接正确。
- `store_credential` → `load_credential` round-trip（用 tempdir 覆盖 home，或注入 path）。

**集成测试 `tests/cluster_node_pairing.rs`（NEW）**
- 起真 `GatewayServer`（AuthMode 需校验 token → 用要求 token 的模式）→ 节点 `run_pairing`（程序化驱动：start_node 拿码 → 调 `pairing.approve` handler 批准 → poll 拿 token）→ 断言节点拿到的 token `validate` 为 Node role → 用该 token `connect` 成功并注册进 `NodeRegistry`（`environments.list` 能看到）。
- 复用 0c-core 集成测试的 inline `CannedSandbox`（lib `MockSandbox` 对集成测试不可见）。

---

## 6. 红线对账

- **R1 / R6**（脑-肢分离 / 一核多端）：节点仍是纯执行臂，配对只是引导，无 DB / harness / LLM。
- **R4**（Interface 纯 I/O）：`handle_pairing_start_node` 是纯 I/O RPC，无业务逻辑。
- **R7**（LLM 主权）：配对是纯确定性状态机，无任何推理判断。
- **R10**（薄 harness）：`src/harness/` 零改动。
- **P7**（锁安全）：复用 pairing_manager 既有 side-table 锁处理，不引入新锁。

---

## 7. YAGNI / 设计取舍

- **复用 browser side-table 而非并行 node 表**：side-table 类型无关，只有 DB-row Pending 检查需类型区分 → `poll_node_pairing` 薄包装，不改 browser 签名。
- **token:signature 拼接格式沿用 browser**：poll 应答 schema 与 browser 完全一致，节点侧拆分。
- **配对不内置退避**：配对流程线性可读，退避只在 `handle_node` 外层重连循环。
- **`--token` 路径保留**：0c-core 预铸供给不删除，与配对共存。
- **失效回落纳入本期**：闭环运维（吊销→自动重配），避免节点陷入无效重连。
