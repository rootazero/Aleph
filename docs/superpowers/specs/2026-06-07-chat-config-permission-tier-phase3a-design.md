# Chat/Config 权限分层 — Phase 3a 设计（device 永久提升/降级 RPC）

> 续 Phase 1 (B1) / Phase 2 / Phase 2b。本 spec 只覆盖 **Phase 3a**：一个 operator-only 的网关 RPC，永久改变已配对设备的档位（chat ↔ config），并立即生效于该设备的活连接。Phase 3b（Panel/Leptos UI）单独成 spec。

## 目标

Operator 通过单个 RPC 把一台**已配对**设备的权限档位在 chat 与 config 之间永久切换（持久化到 `device_store`），且对该设备的**活 websocket 连接立即生效**——无需等待重连。这是 Phase 2b 显式推迟的「永久设备提升」。

## 背景与现状（探查结论）

- **持久化原语已存在**：`DeviceStore::update_permissions(device_id, &[String])`（`src/gateway/device_store.rs:225`）。设备记录 `ApprovedDevice.permissions: Vec<String>`，`["*"]` = operator/config 档，否则 chat 档。
- **档位 SSOT**：`src/gateway/handlers/auth/tier.rs` —
  - `Tier::{Chat, Config}`，`Tier::from_level(Option<&str>)`（`"config"` ⇒ Config，否则 Chat），`Tier::permissions()`（Config ⇒ `["*"]`，Chat ⇒ `["chat","read"]`）
  - `role_for_permissions(&[String]) -> &'static str`（含 `"*"` ⇒ `"operator"`，否则 `"guest"`）
- **现有同址 handler**：`handlers/auth/devices.rs` 内 `handle_devices_list`、`handle_devices_revoke`。`pairing.approve`（`handlers/auth/pairing.rs`）在配对时用 `level` 参（默认 chat）设初始档位，并同时写 `device_store.approve_device()` 与 `security_store.upsert_device()`。
- **门控**：`src/gateway/method_authz.rs` 的 `OPERATOR_METHODS` 已收录 `devices.revoke`、`pairing.approve` 等；`required_privilege(method)` 返回 `MethodPrivilege::Operator`。
- **活连接 role 现状**：连接注册表 `connections: Arc<RwLock<HashMap<String /*conn_id*/, ConnectionState>>>`（`src/gateway/server/mod.rs:132`）。`ConnectionState.role` / `.permissions` **仅在 connect 时**由 `authenticate()` 写入，之后从不刷新。
- **无断连原语**：`ConnectionState` 不存任何 close handle，写半边 socket 是 `handle_connection` 任务私有，外部无法主动关闭指定连接。`devices.revoke` 只删库 + 撤 token，不动活 socket（活 socket 仅在「下一请求」命中 token 撤销重检时才关，且只对 device-token 连接生效）。
- **每请求读活 role**：Phase 2 的 `caller_role` 由 handler 每请求 `conns.get(conn_id).role.clone()` 计算；operator 门控 `is_operator()`（`handler.rs:846`）也每请求读活 `ConnectionState.role`。

> **设计推论**：因为门控每请求读活 `ConnectionState`，「原地 mutate 活连接的 role/permissions」即可让档位变更在该 socket 的**下一个请求立即咬合**，且**无需任何新断连原语**。这比「断连重连」（需新建 per-connection 取消原语）改动小，也消除了「仅下次重连生效」的降级安全窗口。故采纳原地刷新。

## 方案

### RPC 方法

**`devices.set_level`**（单一对称方法，优于 `elevate`/`downgrade` 双方法：复用 tier SSOT、RPC 面最小）

- **params**：`{ "device_id": String, "level": "chat" | "config" }`
- **权限**：operator-only（经 `method_authz`）
- **返回**：`{ "device_id": String, "level": String, "permissions": [String] }`

### Handler：`handle_devices_set_level`

加在 `src/gateway/handlers/auth/devices.rs`（与 `handle_devices_list` / `handle_devices_revoke` 同址同模式），签名沿用 `(request: JsonRpcRequest, ctx: Arc<AuthContext>) -> JsonRpcResponse`。

执行顺序：

1. **解析 params**：缺 `device_id` 或 `level` → JsonRpcError（invalid params）。
2. **校验 level 合法**：`level` 不在 `{"chat", "config"}`（大小写不敏感）→ JsonRpcError。**显式拒非法值**，不依赖 `Tier::from_level` 的静默 default-to-Chat（防拼错把 operator 误降级，P7 防御性设计 / fail-loud）。
3. **校验设备存在**：`ctx.device_store.get_device(device_id)` 为 `None` → JsonRpcError（unknown device）。
4. **算 permissions**：`tier = Tier::from_level(Some(level))`；`permissions = tier.permissions()`。
5. **持久化**：`ctx.device_store.update_permissions(device_id, &permissions)`。失败 → JsonRpcError。
6. **镜像 security_store**：若 `pairing.approve` 路径写了 `security_store`（含 role），此处同样更新以保持两库一致、避免漂移。具体调用在 plan 阶段对齐 `pairing.approve` 的写法（同源同字段）。
7. **原地刷新活连接**：取 `connections` 写锁，遍历所有 `ConnectionState`，对 `device_id` 匹配者：`state.role = role_for_permissions(&permissions).to_string()`，`state.permissions = permissions.clone()`。非匹配连接不动。
8. **返回** `{device_id, level, permissions}`。

### 门控与注册

- `method_authz.rs`：把 `"devices.set_level"` 加入 `OPERATOR_METHODS`（与 `devices.revoke` 并列）。
- dispatch/registration：在 auth handler 暴露/路由处（`handlers/auth/mod.rs` 导出 + 现有 auth RPC 分发点）接入 `devices.set_level → handle_devices_set_level`，与 `devices.revoke` 同样接法。

## 生效语义

- **降级（config → chat）**：持久化 + 原地把活连接 role 改为 `guest`、permissions 改为 `["chat","read"]`。该设备**下一个请求**起，Phase 2 caller_role = `guest`、operator 门控拒绝其 config 类 RPC、Phase 2b 工具门控对其 config 工具求 operator 审批；event_scope 也按新 permissions 过滤（停发 operator-only 事件如 `approval.requested`）。重连后从 `device_store` 重新派生，结果一致。
- **提升（chat → config）**：对称——活连接 role 改为 `operator`、permissions 改为 `["*"]`，下一请求起获得 config 权限。
- **幂等**：对已是目标档位的设备重复调用，结果不变（permissions 写同值、role 算同值），返回成功。

## 边界与非目标

- **不做「最后一个 operator」锁死保护**：本机 shared-token 连接恒为 operator（连接鉴权 Case 0），始终存在一个本地 operator，不可能把自己锁出 → 无需守卫（YAGNI）。
- **不新建断连原语**：明确采用原地刷新而非强制断连（理由见上）。强制断连/取消原语若将来其它特性需要，另行设计。
- **不改 connect 路径的 role 派生**：connect 仍按 `device_store` permissions 派生，与本 RPC 写入的值天然一致。
- **不在本 RPC 处理 UI**：所有 Panel 交互（Devices「授权配置/降级」钮）属 Phase 3b。

## 错误处理

所有失败路径返回 JsonRpcError，不静默：缺参 / 非法 level / 未知 device_id / 持久化失败。security_store 镜像失败按 `pairing.approve` 现有处理风格（warn 不致命）对齐，避免与既有路径不一致。

## 测试

单元测试（`cargo test`，与现有 devices/method_authz 测试同址）：

1. **提升**：chat 档设备 → `set_level config` → `device_store` permissions == `["*"]`；预置一个 device_id 匹配的活 `ConnectionState`（role=guest）→ 调用后其 role == `"operator"`、permissions == `["*"]`。
2. **降级**：config 档设备 → `set_level chat` → permissions == `["chat","read"]`；活连接 role == `"guest"`。
3. **未知设备** → JsonRpcError，库不变。
4. **非法 level**（如 `"admin"`）→ JsonRpcError，库不变。
5. **非匹配连接不受影响**：另一 device_id 的活连接 role/permissions 在调用后不变。
6. **method_authz**：`required_privilege("devices.set_level") == MethodPrivilege::Operator`。

## 验证

- `cargo check --all-targets` 绿。
- 新增单测 + `devices` / `method_authz` 现有测试全绿。
- clippy 改动文件零警告。

## 后续（非本期）

- Phase 3b（Panel/Leptos）：Devices「授权配置/降级」钮消费 `devices.set_level`；配对卡「批准为 chat/config」双钮（靠 B1 `pairing.approve` level）；sudo「等待 server 授权…」态消费 Phase 2b 的 `approval.requested`。
- Spec A：桌面 shell 远程连接本体。
- follow-ups：CLI/wizard 配对仍硬写 `["*"]`；真 WS-level e2e。
