# iMessage 无缝接入：BlueBubbles 传输 + 本地降级 — 设计文档

- **日期**: 2026-06-27
- **分支**: `worktree-imessage-bluebubbles`
- **参考项目**: hermes-agent (`gateway/platforms/bluebubbles.py`)
- **状态**: 设计已批准，待落实施计划

---

## 1. 背景与目标

Aleph 已有一套 iMessage 实现（`src/gateway/interfaces/imessage/`）：轮询 `~/Library/Messages/chat.db`（SQLite 只读）收消息 + `osascript`（AppleScript）发消息。该路径功能有天花板（无 tapback / 打字指示 / 已读回执 / 回复线程），与运行 Aleph 的 Mac 强耦合（必须同机），且 **macOS-only，在 Windows 上无法开发或验证**。

参考项目 hermes-agent 走的是 **BlueBubbles** 架构：一个跑在 Mac 上的开源 iMessage 服务器，暴露 REST API + webhook。这套方案功能完整、与 Aleph 主机解耦（可跨局域网）、纯 HTTP（任意 OS 可开发可测）。

**目标**：把 iMessage 升级为一等公民 channel——

1. **架构增强**：新增 BlueBubbles 传输（REST + webhook 实时入站），作为功能完整的主路径。
2. **保留降级**：现有 chat.db + AppleScript 路径保留为"同机零依赖"降级模式。
3. **错误修复 + 连线**：修群聊发送 bug，连线被绕过的 `parse_target`/`send_to_chat`，消灭 R8 能力撒谎。
4. **熵减**：删除真正无法工作的死代码（`send_tapback` local 实现）。
5. **超越参考**：用 Rust + 现有 `OffsetTracker` 基建实现 hermes 没有的**宕机补偿对账**。

**非目标**：见 §12。

---

## 2. 现状与差距 (Gap Analysis)

| 维度 | Aleph 现状 (chat.db + AppleScript) | hermes-agent (BlueBubbles) | 差距 / 机会 |
|------|-----------------------------------|----------------------------|-------------|
| 收消息 | 轮询 chat.db（默认 1s） | 实时 webhook 推送 | 延迟；随 macOS schema 漂移而脆弱 |
| 发消息 | `osascript` AppleScript | REST `/api/v1/message/text` + multipart | AppleScript **群聊发送实际失败**（按 participant 发，群需 chat-guid） |
| Tapback | ❌ `send_tapback()` 永远 Err，但 `capabilities.reactions=true` | ✅ code 2000-2005/3000-3005 | **R8 诚实工具违规** |
| 打字/已读/回复线程 | ❌ 全无 | ✅ private-api 门控 | 体验差距 |
| 部署耦合 | 必须与 Aleph **同机** | server_url 可指向局域网另一台 Mac | 契合 PRODUCT_TOPOLOGY「家庭服务器+瘦客户端」 |
| OS 约束 | macOS-only，**Windows 不可测** | 纯 HTTP，**任意 OS 可 mock 测试** | 当前开发机是 Windows |
| 架构红线 | 平台特定 I/O 居 `src`（R1 软张力） | 零平台耦合（更契合 R1/R6） | BlueBubbles 反而更"大脑-四肢分离" |

**已验证非死代码（不在清理范围）**：`normalize_phone` 被 `inbound_router/agent_resolver.rs:158` 与 `inbound_router/permission.rs:134` 消费。

---

## 3. 架构决策（已批准）

### D1. 一个 channel，两个 transport，工厂分发

`channel_type` 恒为 `"imessage"`，由 config 的 `transport` 字段在构造时分发：

```
channel_type = "imessage"
        ├── transport="local"       → IMessageChannel    (现有，保留)
        └── transport="bluebubbles" → BlueBubblesChannel  (新增)
```

- **不引入第三个 trait**——复用现有 `Channel` trait（`src/gateway/channel.rs`）作为唯一接缝（R3/P5）。两个并存的具体 `Channel` 实现就是接缝本身；这是有两个真实实现支撑的接缝，不是投机抽象。
- **分发点**：`src/bin/aleph-server/commands/start/builder/subsystems.rs`（现 line 274 特判 `imessage` 直构 `IMessageChannel::new`）改为读 `config.transport` 分支。
- **默认值**：`transport` 缺省 = `"local"`（向后兼容现有部署）；若 TOML 存在 `[channels.imessage.bluebubbles]` 块，则默认提升为 `"bluebubbles"`。
- **channel_id 恒为 `"imessage"`**：`inbound_router` 权限层、`normalize_phone` 配对、`OffsetTracker`（已在 subsystems.rs:282 为 imessage 接好）全部零改动复用。

### D2. BlueBubbles 模块放 `imessage/bluebubbles/`

归在 iMessage 之下（"一个 iMessage 两 transport"心智模型），churn 最小，不割裂。

### D3. local target 模型：连线（而非删除）

`parse_target`/`IMessageTarget`/`send_to_chat` 当前被 send 路径绕过——它们不是死代码，而是**修群聊发送 bug 的缺失连线**。处置见 §8。

---

## 4. 模块设计（镜像 Feishu 双入站范本）

Feishu (`src/gateway/interfaces/feishu/`) 已是成熟的"双入站模式"channel：`feishu_runtime/`（websocket 长连）+ `feishu_inbound/webhook_server.rs`（axum webhook），由 `connection_mode` 切换，自带 `MessageDedup`。BlueBubbles 几乎 1:1 镜像它。

```
src/gateway/interfaces/imessage/
  mod.rs              — IMessageChannel (local，保留) + 工厂按 transport 分发
  config.rs           — IMessageConfig + transport 字段 + 内嵌 BlueBubblesConfig
  db.rs               — local chat.db 读 (保留)
  sender.rs           — local AppleScript 发 (§8 连线修复 + 删 send_tapback)
  target.rs           — parse_target/IMessageTarget (§8 连线进 local send)
  bluebubbles/                      ← 新增
    mod.rs            — BlueBubblesChannel (Channel impl: start/stop/send + capabilities)
    config.rs         — BlueBubblesConfig (schemars-friendly，未来 R8 工具零成本接入)
    api.rs            — REST 客户端 (reqwest，零新依赖)
    inbound/
      mod.rs
      webhook_server.rs — axum POST 处理器（镜像 feishu webhook_server.rs）
      mapper.rs         — BB JSON → InboundMessage
      dedup.rs          — 按 message GUID 去重（webhook∩补偿轮询，必须）
      poll.rs           — 补偿轮询 message/query?after=<cursor> + OffsetTracker
    outbound/
      mod.rs
      text.rs / attachment.rs / reaction.rs / typing.rs / read.rs
```

### `api.rs` REST 端点（全部经 reqwest，password 走 query 参数，照搬 hermes 鉴权）

| 方法 | 端点 | 用途 |
|------|------|------|
| GET | `/api/v1/ping` | 连通性探测 |
| GET | `/api/v1/server/info` | 探测 `private_api` / `helper_connected`（富能力门控）|
| POST | `/api/v1/message/text` | 发文本（+`method=private-api`+`selectedMessageGuid` 回复线程）|
| POST | `/api/v1/message/attachment` | multipart 发附件 |
| POST | `/api/v1/chat/query` | 解析 chatGuid（参与者/identifier 匹配，LRU 缓存，cap 500）|
| POST | `/api/v1/chat/new` | 新建会话（首条消息）|
| POST/DELETE | `/api/v1/chat/{guid}/typing` | 打字指示开/关 |
| POST | `/api/v1/chat/{guid}/read` | 已读回执 |
| POST/GET/DELETE | `/api/v1/webhook` | webhook 注册/列出/注销（幂等，崩溃复用）|
| POST | `/api/v1/message/query` | **补偿轮询**：`after` 时间游标拉漏消息 |
| GET | `/api/v1/attachment/{guid}/download` | 下载入站附件落本地缓存 |

---

## 5. 入站设计：Webhook 实时 + 轮询补偿（超越点）

### 5.1 Webhook 实时（镜像 feishu）

- `BlueBubblesChannel::start` 内 `tokio::spawn(run_webhook_server(state))`（同构 feishu mod.rs:184），axum `TcpListener` 绑 `webhook_host:webhook_port` + `webhook_path`。
- start 时向 BB `/api/v1/webhook` 注册回调 URL（含 password query）；**幂等**——先列出已存在注册，命中则复用（崩溃后不重复，照搬 hermes `_register_webhook`）。
- `stop` 时注销。
- **鉴权**：webhook POST 的 `?password=` 必须等于配置 password，否则 401（照搬 feishu password 校验范式）。

### 5.2 补偿轮询（hermes 没有，这是"超越"）

- hermes webhook-only 在 Aleph 宕机期间会丢消息。Aleph 增补：start 时 + 周期性 `message/query?after=<cursor>` 对账，捞回漏消息。
- **游标复用现成 `OffsetTracker`**（`src/gateway/interfaces/telegram/offset.rs`，已在 subsystems.rs:282 为 imessage 接好）。BlueBubbles 游标是消息时间戳（`dateCreated`）而非 ROWID——`OffsetTracker` 存 i64 单调值，时间戳天然适配。

### 5.3 去重（混合方案正确性关键）

- 同一条消息会**同时**经 webhook 与补偿轮询到达 → 必须按 message GUID 去重。
- `dedup.rs` 镜像 feishu `MessageDedup`（`feishu_inbound/dedup.rs`）：有界 LRU/集合记最近已见 GUID，重复则丢弃。
- 入站统一漏斗：`webhook handler` 与 `poll loop` 都先过 `dedup.is_duplicate(guid)` 再 `sender.send(inbound)`。

### 5.4 mapper（BB JSON → InboundMessage）

- 跳过 `isFromMe`；跳过 tapback（`associatedMessageType ∈ 2000-2005/3000-3005`）。
- 附件：`attachment/{guid}/download` 落本地缓存，填 `InboundMessage.attachments`（复用现有 `Attachment` 结构）。
- 群聊：`isGroup` 或 chatGuid 含 `;+;`；`require_mention` 时按 mention 模式过滤 + 清洗前缀（照搬 hermes，但 mention 用配置而非硬编码 wake word）。
- 输出 `InboundMessage`（`channel_id="imessage"`），与 local 路径同构，下游零差异。

---

## 6. 出站与诚实能力位（修 R8 撒谎）

### 6.1 出站

| 操作 | 实现 |
|------|------|
| 文本 | `message/text`；段落按双换行切多气泡（照搬 hermes，去掉 `(1/3)` 分页后缀）|
| 附件 | `message/attachment` multipart（image/voice/video/document）|
| Tapback | `reaction.rs`，private-api 门控 |
| 打字指示/已读 | `typing.rs`/`read.rs`，`private_api`+`helper_connected` 门控 |
| 回复线程 | `message/text` 带 `selectedMessageGuid`（private-api）|

### 6.2 诚实能力位

`capabilities()` 改为**按 transport 各报各的真值**：

| 能力 | local | bluebubbles |
|------|-------|-------------|
| attachments / images / audio / video | ✅ | ✅ |
| reactions | **false**（改正撒谎）| true |
| replies | false | true |
| typing_indicator | false | true |
| read_receipts | false | true |

- 富能力静态广播 transport 上限，**运行时按探测到的 `private_api`/`helper_connected` call-time 优雅降级**（照搬 hermes：探测不到就静默 no-op，不报错）。

---

## 7. 配置 Schema

```toml
[channels.imessage]
enabled = true
transport = "bluebubbles"          # "local"(默认) | "bluebubbles"；存在下方块时默认 bluebubbles
# ---- local 路径既有字段（保留，向后兼容）----
db_path = "~/Library/Messages/chat.db"
poll_interval_ms = 1000
dm_policy = "pairing"
# ...（allow_from / group_policy / require_mention 等全部保留）

[channels.imessage.bluebubbles]
server_url = "http://192.168.1.50:1234"
password = "{{secret:bluebubbles_password}}"   # 复用现有 src/secrets/ 保险库管道
webhook_host = "0.0.0.0"
webhook_port = 8645
webhook_path = "/bluebubbles-webhook"
poll_interval_secs = 30            # 补偿轮询间隔（实时由 webhook 负责）
send_read_receipts = true
require_mention = false
mention_patterns = []              # 群聊唤醒词（空=不限）
```

- `BlueBubblesConfig` 加 `#[derive(JsonSchema)]`（schemars）——未来 R8 channel 配置工具零成本接入（P3 schema 驱动）。
- password 走现有 `{{secret:}}` 保险库管道（`src/secrets/`），不明文落配置。

---

## 8. 熵减 / 错误修复 / 连线

逐项已验证（遵循"删前再验"纪律，呼应记忆 `aleph-audit-verify-findings`）：

| 符号 | 真相 | 处置 |
|------|------|------|
| `send_to_chat`(sender.rs) | **非死代码**——修群聊发送 bug 的缺失连线 | **连线**：local `send()` 对群聊（is_group / `chat_id:` 前缀）改走 `send_to_chat`（现用 `participant "x"` 发群**必然失败**）|
| `parse_target`/`IMessageTarget`(target.rs) | send 路径绕过，直接用原始 conversation_id | **连线**：作 local send 路由分发器（phone/email→`send_text`，chat→`send_to_chat`），实现上面的修复 |
| `send_tapback`(sender.rs) | AppleScript 真做不到（永远 Err）| **删除** + local `capabilities.reactions=false` |
| `Service::Sms` | AppleScript 硬编码 iMessage，SMS 强制无效 | **保留** enum（`parse_target` 用它路由），但 local send 不实现 SMS 强制（AppleScript 限制），doc 注明；不删避免 churn |
| `normalize_phone` | 被 agent_resolver/permission 消费 | **保留** |

**净效果**：删除 1 处真死代码（`send_tapback`），连线 2 处修真 bug（群聊发送），消灭 1 处 R8 撒谎（reactions 能力位），新增功能完整的 BlueBubbles transport。

---

## 9. Onboarding（无缝接入）

- **TOML**：§7 的配置块即主配置面。
- **Wizard**：扩展现有 `src/wizard/flows/onboarding.rs:136` 的 imessage 步骤——加 transport 选择（local / bluebubbles），选 bluebubbles 时收集 server_url/password/webhook 字段；订正 "macOS only" hint（BlueBubbles 远程模式下 Aleph 主机不必是 Mac）。
- **R8 缺口说明**（精炼范围）：通用「自然语言 channel 配置工具」当前**不存在**（`channels.list` 仅只读），是跨所有 channel 的独立特性，**另立 spec**，不捆入本次（避免 R3/P6 范围蔓延）。本次把 `BlueBubblesConfig` 写成 schemars-friendly，为那个未来工具铺路。

---

## 10. 测试策略（终于能在 Windows 验证 iMessage）

BlueBubbles = 纯 HTTP → **Windows 上可全测**：

- **单元**：`mapper`（BB JSON → InboundMessage，录制 fixture）、`config` 解析、GUID 缓存 LRU、`dedup`、补偿游标推进、能力位真值。
- **集成**：起 axum mock BB server——
  - `api.rs`：发文本/附件、GUID 解析、webhook 注册幂等。
  - `webhook_server`：用 feishu `test_challenge_response` 的 `tower::ServiceExt::oneshot` 范式喂 POST，断言产出正确 `InboundMessage` + 鉴权 401 路径。
  - webhook∩poll 去重：同 GUID 两路径只投递一次。
- **保留**：现有 `tests/imessage_{contract,fixture,protocol}_test.rs`（local 路径，macOS 门控）不动。
- 覆盖率目标：新增 BlueBubbles 代码 ≥ 80%（呼应 testing 规则，且这是 local 路径在 Windows 上永远做不到的）。

---

## 11. 成功标准（可验证）

1. `cargo check -p alephcore` 通过（Windows）。
2. `transport="bluebubbles"` 时：mock BB server 下，webhook POST → 产出 InboundMessage；`send()` → 命中 mock `message/text`。
3. `transport="local"`（默认）时：现有行为不变，现有 imessage 测试全绿；群聊 send 现在走 `send_to_chat`（新增回归测试覆盖）。
4. `capabilities()` 按 transport 返回真值；local `reactions=false`。
5. webhook∩补偿轮询去重：同一 GUID 不重复投递（测试断言）。
6. 无新依赖进 Cargo.toml（reqwest/axum 复用）。
7. `send_tapback`(local) 已删，无残留引用；`parse_target`/`send_to_chat` 有真实消费者（local send）。
8. 新增 BlueBubbles 代码测试覆盖 ≥ 80%。

---

## 12. 范围外 (Out of Scope)

- 通用「R8 自然语言 channel 配置工具」（跨所有 channel，另立 spec）。
- socket.io 入站（已决策用 webhook+poll，不引入 rust-socketio）。
- BlueBubbles 私有 API 的高级特性（编辑/撤回消息、群管理）——首版不做。
- local 路径的进一步增强（其功能天花板已知，BlueBubbles 是增强答案）。

---

## 13. 风险

| 风险 | 缓解 |
|------|------|
| BlueBubbles webhook 载荷跨版本漂移（v1.9+ chatGuid 嵌套变化）| mapper 多候选字段回退（照搬 hermes `_value`/嵌套 `chats[0].guid` 回退）|
| webhook 端口被占 / 防火墙 | bind 失败优雅降级到纯补偿轮询（不 panic，记录 error，照搬 feishu bind 失败处理）|
| 用户未开 BlueBubbles private-api | 富能力 call-time 探测降级，基础收发不受影响 |
| 时间戳游标精度导致补偿轮询边界重复 | dedup 兜底；游标取 `>=` 配合 GUID 去重 |
| 与现有 OffsetTracker（ROWID 语义）混用 | BlueBubbles 用独立 tracker key（如 `imessage_bb`），不与 local ROWID 串味 |

---

## 14. 实施顺序提示（交 writing-plans 细化）

先连线、后扩展：

1. **骨架 + 分发**：config `transport` 字段 + subsystems.rs 分支 + 空 `BlueBubblesChannel`（Channel impl 桩）。
2. **熵减/bugfix**（独立可先做）：连线 `parse_target`/`send_to_chat` 修群聊；删 `send_tapback`；改 local 能力位真值 + 回归测试。
3. **BlueBubbles 出站**：api.rs + outbound/{text,attachment} + capabilities。
4. **BlueBubbles 入站**：webhook_server + mapper + dedup + poll（补偿）。
5. **富能力**：reaction/typing/read + private-api 门控。
6. **Onboarding**：wizard 步骤扩展 + schemars。
7. **测试**：mock BB server 集成 + 各单元，达 80%。
