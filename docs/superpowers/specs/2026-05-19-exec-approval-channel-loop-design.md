# exec-approval 通道审批闭环 — 设计文档

- **日期**: 2026-05-19
- **分支**: `feat/exec-approval-channel-wiring`（worktree `/Volumes/TBU4/Workspace/Aleph-exec-approval`）
- **状态**: 设计待用户确认 → 之后 writing-plans → 实现
- **前置**: commit `189b7a459`（`ApprovalGate::set_requester` 后置注入原语，已在 main）

---

## 1. 背景与问题

`ApprovalGate` 负责把两类需要人工确认的工具调用送达用户：

- **sandbox elevated-capability 升级**（`workspace.rs:195` → `request_approval_for_tool`）
- **PermissionLayer Ask-tier 工具调用**（`permission/mod.rs:62` → `request_approval_for_tool`）

现状：`ApprovalGate` 在 boot（`start/mod.rs:555`）以 `None` requester 构造，`request_approval_for_tool` 无 requester 时**一律返回 `Denied`**。库里 `ChannelApprovalBridgeAdapter` / `ChannelApprovalBridge` / `ExecApprovalManager` 都存在，但 boot 处完全没有接线，且接线点本身藏有库级缺陷。

目标：让审批请求真正送达用户通道（本期 Telegram），用户点 inline keyboard 按钮 approve/deny，决策回灌唤醒阻塞的工具调用 —— 取代当前的 auto-Denied。

---

## 2. 调研结论

链路 `ApprovalGate → ChannelApprovalBridgeAdapter → ChannelApprovalBridge → ExecApprovalManager` 看似齐全，实有 **3 处库级缺陷** + boot 完全未接线。

### 2.1 缺陷① — 三个 id 互不相连

| id | 出处 | 用途 |
|----|------|------|
| `record.id`（UUID） | `ExecApprovalManager::create` | `wait_for_decision` / `resolve` 的键 |
| `tg-{uuid}` ②号 | `telegram/approval.rs:70` `deliver_approval` | 仅返回在 `PendingApproval.approval_id` |
| `tg-{uuid}` ③号 | `telegram/approval.rs:115` `render_approval` | **真正写进按钮 callback data 的 id** |

`deliver_approval` 转手调用 `render_approval`，后者**第三次独立生成 UUID**。用户点的按钮带 id③，而 `wait_for_decision` 阻塞在 id①，三者全不相等 —— resolve 永远落空。

### 2.2 缺陷② — 回调数据格式不一致

- Telegram `approval_callback_data`（approval.rs:55）发 **2 段**：`approve:{id}` / `deny:{id}`
- `ApprovalBridge::parse_callback`（RPC `exec.callback.handle` 用）要 **3 段**：`approve:{id}:{once|always|deny}`

即使 id 对齐，格式也对不上，parse 直接失败。

### 2.3 缺陷③ — `parse_session_key` 对默认 DM scope 静默失败（最危险）

`ChannelApprovalBridge::parse_session_key`（channel_bridge.rs:368）靠在 session_key **字符串**里扫 `"telegram"` 字样并取固定偏移 `parts[i+2]`。但 `SessionKey::to_key_string()` 对常见场景根本不含通道名：

| SessionKey 形态 | `to_key_string()` 输出 | parse 结果 |
|----------------|----------------------|-----------|
| `DirectMessage` + `DmScope::PerPeer`（**默认值**） | `agent:{agent}:dm:{peer}` | ❌ `None` → Denied |
| Main-scope DM（坍缩为 `SessionKey::Main`） | `agent:{agent}:main` | ❌ `None` → Denied |
| `DirectMessage` + `DmScope::PerChannelPeer` | `agent:{agent}:telegram:dm:{peer}` | ✅ |
| `Group` | `agent:{agent}:telegram:group:{peer}` | ✅ |

**结论**：字符串 session_key 是有损表示，无法可靠还原 `(channel, conversation)`。默认 DM 场景即便修好缺陷①②、接好线，仍会静默拒绝。必须改用结构化 `SessionKey`。

### 2.4 boot 完全未接线

- `ExecApprovalManager` —— 全代码库无 boot 级实例。
- `ApprovalGate` —— `start/mod.rs:555` 带 `None` requester 构造。
- `exec_approvals::create_handlers` —— 零调用者；返回 `Fn(&str)->Option<RpcHandler>`，与 `HandlerRegistry::register(method, handler)` 形状不兼容。
- Telegram 回调 —— 轮询（`telegram/mod.rs:515`）+ webhook（`telegram/webhook.rs:252`）都把按钮点击包成普通 `InboundMessage`（`text` = callback data）塞进 `InboundMessageRouter`，无任何 `approve:` 分发逻辑，会被当聊天消息发给 LLM。

---

## 3. 范围决策（已与用户确认锁定）

| 议题 | 决策 |
|------|------|
| 审批路径 | **仅 ApprovalGate 路径**。`ExecSecurityGate`（bash/code_exec Danger 层）本身 boot 未接 channel bridge，是独立的更大工作，不在本期。 |
| 通道范围 | **仅 Telegram**。唯一已有 `ChannelApprovalCapability` 实现。 |
| RPC 接口 | **本期注册** `exec.approval.*` JSON-RPC 处理器。 |
| 超时行为 | 保持默认 **2 分钟**；超时向通道发友好提示，点过期按钮回友好提示。 |

---

## 4. 目标架构 — 端到端链路

```
工具调用（Ask-tier / sandbox 升级）
  → ApprovalGate.request_approval_for_tool(tool, reason)
  → ChannelApprovalBridgeAdapter.request_approval
        从 task-local 结构化 SessionKey 解出 (ChannelId, ConversationId)
        无通道（Main/Task/Ephemeral）→ 明确 Denied + 告警
  → ChannelApprovalBridge.request_for_tool(manager, tool, reason, channel_id, conversation_id, timeout)
        let record = manager.create(request)           // record.id = 唯一 id
        deliver: channel.approval_capability()
                   .deliver_approval(conversation_id, request, &record.id)   ← id 直达按钮
        manager.wait_for_decision(record)               // 阻塞在 oneshot(record.id)

  ┄┄ 用户在 Telegram 点按钮 ┄┄

  → 回调 → InboundMessage { id: "cb_…", text: "approve:{record.id}:once" }
  → InboundMessageRouter 入口：cb_ 前缀 → ApprovalCallbackSink.handle_callback
  → ManagerCallbackSink: ApprovalBridge::parse_callback → manager.resolve(record.id, decision, resolved_by)
  → oneshot 唤醒 → wait_for_decision 返回 Some(decision)
  → ApprovalOutcome::Approved/Denied → 工具放行/拒绝
  → router 把 sink 返回的 response_text 渲染回通道（"✅ 已批准" / "❌ 已拒绝" / "该审批已过期"）
```

唯一 id 全程为 `ExecApprovalManager` 的 `record.id`，缺陷①②③在链路上一并消除。

---

## 5. 详细设计

### 5.1 缺陷① 修复 — id 透传（goal 子工作 #4）

`ChannelApprovalCapability::deliver_approval` 增加 `approval_id: &str` 参数，由调用方传入 `record.id`。capability 用该 id 自行拼按钮 callback data，**不再调用会自生成 id 的 `render_approval`**。

```rust
// gateway/channel_approval.rs — trait 签名变更
async fn deliver_approval(
    &self,
    conversation_id: &ConversationId,
    request: &ApprovalRequest,
    approval_id: &str,            // 新增：调用方（manager record.id）
) -> ChannelResult<PendingApproval>;
```

- 唯一 impl：`TelegramChannelApprovalCapability`（telegram/approval.rs）。
- 唯一 caller：`ChannelApprovalBridge`（channel_bridge.rs:108）。
- trait 文档示例（channel_approval.rs:106-118）同步更新。
- `render_approval`（自生成 id 那个）保留为 trait 必需方法，Telegram 不再用它 —— 属预存轻微冗余，**不在本期清理**（见 §9）。

### 5.2 缺陷② 修复 — 回调格式统一

统一采用 `ApprovalBridge` 的 3 段格式 `approve:{id}:{once|deny}`。Telegram MVP 用 2 个按钮：

- `✅ Approve` → `approve:{record.id}:once`
- `❌ Deny` → `approve:{record.id}:deny`

（格式保留 `always` 段位以便后续扩展 Allow-Always，本期 UI 不出该按钮。）`ApprovalBridge::parse_callback` 已能解析该格式，无需改动。

### 5.3 缺陷③ 修复 — 结构化 SessionKey 路由

新增一个从结构化 `SessionKey` 解出通道路由的纯函数（置于 `src/approval/` 模块内，保持与 adapter 同域内聚）：

```rust
/// 从结构化 SessionKey 解出 (channel, conversation)。
/// 无通道来源的会话（Main/Task/Ephemeral）返回 None。
fn channel_route(key: &SessionKey) -> Option<(ChannelId, ConversationId)> {
    match key {
        SessionKey::DirectMessage { channel, peer_id, .. }
        | SessionKey::Group { channel, peer_id, .. }
            => Some((ChannelId::new(channel), ConversationId::new(peer_id))),
        SessionKey::Subagent { parent_key, .. } => channel_route(parent_key),  // 递归
        SessionKey::Main { .. } | SessionKey::Task { .. } | SessionKey::Ephemeral { .. }
            => None,
    }
}
```

- `ChannelApprovalBridgeAdapter` 改为读 task-local `SESSION_ID`（本就是结构化 `SessionKey`）并调 `channel_route`，**不再 `sid.to_string()`**。
- `ChannelApprovalBridge::request_for_tool` 签名把 `session_key: &str` 换成结构化 `channel_id: &ChannelId, conversation_id: &ConversationId`；内部直接按 `channel_id` 查 registry → `approval_capability()` → `deliver_approval(...)`，**不再调 `parse_session_key`**。
- 唯一 caller 是 adapter（adapters.rs:80），签名变更影响面闭合。
- 旧 `request_approval(&request)` + `parse_session_key` + `authorize_and_deliver` + `resolve_approval` 由 out-of-scope 的 `ExecSecurityGate` 间接依赖，**保留不动并标注为已知缺陷**（见 §9）。
- 假设：单 Telegram 通道实例时 `SessionKey.channel` 字符串 == 注册的 `ChannelId`。多实例命名差异属边界情况，本期不处理但在实现中加断言式日志。

### 5.4 回调分发链路（goal 子工作 #3）

`InboundMessageRouter::handle_message` 入口拦截 `cb_` 前缀的回调消息。router **不直接持有 `ExecApprovalManager`**，而是依赖一个注入的窄接口 `ApprovalCallbackSink`（trait，定义于新模块 `src/gateway/inbound_router/approval_callback.rs`）：

```rust
pub(crate) struct ApprovalCallbackResult { pub resolved: bool, pub response_text: String }

#[async_trait]
pub(crate) trait ApprovalCallbackSink: Send + Sync {
    /// 返回 Some 当且仅当 callback_data 是审批按钮回调。
    async fn handle_callback(&self, callback_data: &str, user_id: &str)
        -> Option<ApprovalCallbackResult>;
}
```

具体实现 `ManagerCallbackSink`（新文件 `src/approval/callback_sink.rs`）包 `Arc<ExecApprovalManager>`：
`ApprovalBridge::parse_callback` 解析 callback_data —— `None` 即非审批回调；`Some((id, decision))`
→ `manager.resolve(id, decision, Some(user_id))` → 据 resolved 结果产出确认 / 过期文案。

router 在 `handle_message` 顶部：`msg.id` 以 `cb_` 开头且 sink 存在 → `sink.handle_callback` →
`Some` 即拦截，`response_text` 经 `channel_registry.send` 渲染回通道并 `return Ok(())`；
`None` → 放行进正常消息流（其它 inline keyboard 不受影响）。

**为何不走 JSON-RPC 自分发**：让 router 持 `Arc<HandlerRegistry>` 会使注册表引用计数 >1，
之后任何 `GatewayServer::handlers_mut()`（`Arc::get_mut`，如 config watcher 注册）会 panic。
注入窄 trait 既避开此隐患，又让 router 依赖抽象而非 core 具体类型（P4 依赖倒置）。

> **R4/R7/P8 说明**：router 仍是纯 I/O —— 收回调、交注入的 sink、渲染返回文案，不解析不持久化不推理。
> `manager.resolve` 仅把人类已做决策投递进 oneshot，是机制非业务逻辑。解析 `approve:{id}:{decision}`
> 针对**固定机器格式**（P8「正则只适用于格式固定的机器生成文本」），非自然语言匹配，不违反 R7/R10。

### 5.5 RPC handler 注册（goal 子工作 #2）

`exec_approvals.rs` 现有 `create_handlers` 返回的 `Fn(&str)->Option<RpcHandler>` 与 `HandlerRegistry::register` 不兼容。改造：

- 新增 `pub fn register_handlers(registry: &mut HandlerRegistry, manager: Arc<ExecApprovalManager>)`，逐方法 `registry.register("exec.approval.resolve", …)` 等。
- 删除死的 `create_handlers` 与 `RpcHandler` 类型别名（属本次重构直接产生的清理，符合 goal「重构后清理旧码」）。
- `handle_*` 异步处理函数全部保留。
- 注册的方法：`exec.approval.request` / `exec.approval.resolve` / `exec.approvals.get` / `exec.approvals.set` / `exec.approvals.pending` / `exec.callback.handle`。

### 5.6 boot 接线（goal 子工作 #1、#5）

`src/bin/aleph-server/commands/start/mod.rs` + `builder/subsystems.rs`：

1. **`start/mod.rs` 第 555 行前**：构造共享 `let exec_approval_manager = Arc::new(ExecApprovalManager::new());`。
2. `ApprovalGate` 仍在 555 以 `None` requester 构造（不变）。
3. **RPC 注册**：`initialize_channels` 返回后、`initialize_inbound_router` 之前调 `exec_approvals::register_handlers(server.handlers_mut(), exec_approval_manager.clone())`（此时 `server` 仍 `&mut`、handlers 引用计数为 1）。无需改 `initialize_channels` 签名。
4. **`initialize_channels` 返回 `channel_registry` 后**（`start/mod.rs:1705` 之后）：
   ```rust
   let bridge = Arc::new(ChannelApprovalBridge::new(channel_registry.clone()));
   let adapter = Arc::new(ChannelApprovalBridgeAdapter::new(bridge, exec_approval_manager.clone()));
   approval_gate.set_requester(adapter);
   ```
5. **router 注入**：`initialize_inbound_router` 增参 `Option<Arc<dyn ApprovalCallbackSink>>`（boot 构造 `ManagerCallbackSink`），router 经 `with_approval_callback_sink` 持有。
6. **`handlers_mut` 安全**：router 不再 clone `Arc<HandlerRegistry>`，注册表引用计数维持为 1，后续 `setup_config_watcher` 等的 `handlers_mut()` 不受影响。

### 5.7 错误与超时处理

| 情形 | 行为 |
|------|------|
| task-local 无 `SESSION_ID` / 会话无通道（Main/Task/Ephemeral） | `Denied` + `tracing::warn`（非静默 auto-approve） |
| 通道未注册 / 无 `approval_capability` / 投递失败 | `Denied` + `warn` |
| `wait_for_decision` 超时（默认 2 分钟） | 返回 `Timeout`；`request_for_tool` 在返回前向 `conversation_id` 发「⏱ 审批请求已超时，操作被拒绝。」 |
| 用户点已过期/已 resolve 的按钮 | `exec.callback.handle` → `handled:false, approval_id:Some` → router 渲染「该审批已过期或已处理」 |

`ApprovalOutcome::Timeout` 与 `Denied` 对调用方等价（`is_approved()` 仅 `Approved` 为真），故超时即安全拒绝。

> **竞态说明**：`request_for_tool` 中 `deliver_approval` 返回与 `wait_for_decision` 的 `pending` insert 之间无 `.await` 点（同步 Rust，无让出），用户的按钮点击作为独立入站消息只可能在其后到达 —— 不存在「resolve 早于 register」竞态，`ExecApprovalManager` **零改动**。

### 5.8 授权

回调 `InboundMessage` 能进入 router，说明已通过通道级 `access.check_message`（webhook.rs:273 / mod.rs:546，仅 allowed/paired 用户的回调才被包成入站消息）。MVP 以此为授权底线；`exec.callback.handle` 把 `user_id`（= `msg.sender_id`）记入 `resolved_by` 供审计。capability 内更细的 `authorize_actor` 配对校验属增强项，本期不强制接入。

---

## 6. 改动清单（按文件）

| 文件 | 改动 |
|------|------|
| `src/gateway/channel_approval.rs` | `ChannelApprovalCapability::deliver_approval` 增 `approval_id: &str`；更新 trait 文档示例 |
| `src/gateway/interfaces/telegram/approval.rs` | `deliver_approval` 用传入 id 自拼 3 段格式键盘（2 按钮），不再调 `render_approval` |
| `src/exec/approval/channel_bridge.rs` | `request_for_tool` 签名改结构化 `(ChannelId, ConversationId)`；内部直接按 channel 查 registry，不调 `parse_session_key`；超时向通道发提示 |
| `src/approval/adapters.rs` | 新增 `channel_route(&SessionKey)`；`request_approval` 改读结构化 SessionKey；无通道 → Denied + warn |
| `src/gateway/handlers/exec_approvals.rs` | 新增 `register_handlers(&mut HandlerRegistry, Arc<ExecApprovalManager>)`；删除死的 `create_handlers` + `RpcHandler` 别名 |
| `src/gateway/inbound_router/approval_callback.rs` **〔新〕** | 定义 `ApprovalCallbackSink` trait + `ApprovalCallbackResult` |
| `src/approval/callback_sink.rs` **〔新〕** | `ManagerCallbackSink`：实现 `ApprovalCallbackSink`，包 `Arc<ExecApprovalManager>`，`parse_callback` + `resolve` |
| `src/gateway/inbound_router/mod.rs` | router 增 `Option<Arc<dyn ApprovalCallbackSink>>` 字段 + `with_approval_callback_sink`；`handle_message` 顶部拦截 `cb_` 回调 |
| `src/bin/aleph-server/commands/start/mod.rs` | 555 前构造共享 `Arc<ExecApprovalManager>`；1697 后构造 bridge+adapter+`set_requester` |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs` | `initialize_channels` 增 manager 参 + 调 `register_handlers`；`initialize_inbound_router` 增 `HandlerRegistry` 参 |

**不需改动**：`src/exec/manager.rs`（`ExecApprovalManager` 零改动）、`src/sandbox/exec_approval/gate.rs`（`set_requester` 已就位）。

---

## 7. 测试策略（TDD：先红后绿）

**单元测试**

- `channel_route`：`DirectMessage`(PerPeer/Main/PerChannelPeer 各形态) → `Some`；`Group` → `Some`；`Main`/`Task`/`Ephemeral` → `None`；`Subagent` → 递归父键。
- Telegram `deliver_approval`：透传 `approval_id`，按钮 callback data == `approve:{id}:once` / `approve:{id}:deny`。
- `ApprovalBridge::parse_callback` 对 Telegram 实发格式往返成立。
- `ChannelApprovalBridgeAdapter`：结构化 SessionKey + 已注册假通道 → 走通投递；task-local 缺失 / 无通道 → `Denied`。
- `exec_approvals::register_handlers`：注册后 `registry.has_method("exec.approval.resolve")` 等为真。
- `approval_callback`：审批回调 → 拦截 + resolve；非审批回调（parse 失败）→ 放行；过期回调 → 拦截 + 过期文案。

**集成测试**（`tests/`）

- 全闭环：注册带 `ChannelApprovalCapability` 的假通道 → `ApprovalGate.set_requester(adapter)` → `request_approval_for_tool` 在一任务中阻塞 → 另一任务模拟 `cb_` 回调入站消息经 `exec.callback.handle` resolve → 断言阻塞侧得 `Approved`；对称验 `Denied`、`Timeout`（短超时）。

**E2E**（`/e2e-verify`，真 Telegram 手动）

- 真实 Telegram bot：触发一次 Ask-tier 工具 → 收到带按钮消息 → 点 Approve → 工具放行 → 通道收到确认；点 Deny 对称；放任 2 分钟 → 收到超时提示。

覆盖率目标 80%+（CLAUDE.md / rules）。

---

## 8. 不在本期范围

- **`ExecSecurityGate` 路径**（bash/code_exec Danger 层）：`with_channel_registry` 零调用者，自身 boot 未接 channel bridge，属独立工作。
- **Discord / Slack / WebChat 按钮审批**：无 `ChannelApprovalCapability` 实现，需各自新写。
- **Allow-Always 按钮**：格式预留 `always` 段位，本期 UI 不出该按钮。
- **审批消息按钮失效编辑**：resolve 后编辑原 Telegram 消息移除按钮（需 message_id 跟踪 + edit API）。本期靠「过期文案」兜底。
- **`authorize_actor` 配对级授权接入**：本期以通道级 access 为授权底线。

---

## 9. 预存代码处置（不破坏性重构）

以下为 out-of-scope 的预存代码，**保留不动**，仅在代码注释/本文档标注：

- `ChannelApprovalBridge::parse_session_key`、`request_approval(&request)`、`authorize_and_deliver`、`resolve_approval`、`pending_approvals` —— 由 `ExecSecurityGate` 间接依赖；`parse_session_key` 即缺陷③，标注为「已知缺陷，仅 ExecSecurityGate 旧路径使用」。
- `TelegramChannelApprovalCapability::render_approval`（自生成 id）—— trait 必需方法，Telegram MVP 路径不再使用，标注轻微冗余。

唯一主动删除：死的 `exec_approvals::create_handlers` + `RpcHandler` 别名（被 `register_handlers` 取代，属本次重构直接产物）。

---

## 10. 架构红线合规

| 红线 | 合规说明 |
|------|---------|
| **R4**（Interface 纯 I/O） | router 不触 core，仅把回调交注入的 `ApprovalCallbackSink`、响应渲染回通道 |
| **R7 / R10**（LLM 主权 / 笨循环） | 不新增 LLM 调用、不碰 `src/harness/`；`manager.resolve` 仅投递人类决策，非推理 |
| **P8**（LLM-First） | 解析 callback data 是固定机器格式解析，非自然语言模式匹配 |
| **P1 / P2**（低耦合高内聚） | 回调逻辑收敛于 `approval_callback.rs`；router 依赖 `HandlerRegistry` 而非具体 manager |
| **P6**（KISS/YAGNI） | `ExecApprovalManager` 零改动；不引入未使用抽象 |

---

## 11. 验收标准

1. `cargo build -p alephcore` 与 `aleph-server` 通过。
2. 新增单测 + 集成测试全绿；改动模块覆盖率 ≥ 80%。
3. boot 日志不再出现「ApprovalGate has no ApprovalRequester wired」告警。
4. 集成测试证明：Ask-tier 工具调用经假通道投递 → 模拟按钮回调 → 工具得 `Approved`/`Denied`/`Timeout`。
5. `cargo clippy` 不引入新告警（基线既有告警除外）。
6. main 不受影响（全部工作在 `feat/exec-approval-channel-wiring`）。

---

## 12. 实施阶段划分（writing-plans 细化）

- **P1 库缺陷修复**：缺陷①②③ 三处库改 + `channel_route` + 全部单测。不接 boot，可独立编译测试。
- **P2 RPC 注册**：`exec_approvals::register_handlers` + 删死代码 + 单测。
- **P3 回调分发**：`approval_callback.rs` + router 接线 + 单测。
- **P4 boot 接线**：`ExecApprovalManager` 构造、adapter、`set_requester`、`HandlerRegistry` 注入 router。
- **P5 集成与验证**：集成测试 + `/e2e-verify`。

每阶段独立可测；P1–P3 不依赖 boot，P4 收口，P5 验收。
