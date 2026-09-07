# Review: shared

## Summary
- **crate 数**: 4
- **文件总数**: 102 `.rs` (protocol:62, ui_logic:28, client:7, logging:5)
- **估计 P0 / P1 / P2 数量**: 0 / 2 / 3
- **整体评估**: **有风险** — 协议契约总体严谨(每个 wire 类型都有详尽文档+前/后向兼容测试),但缺少关键的 `#[non_exhaustive]` 防御,且 `ProviderInfo.api_key` 字段是凭证泄漏的隐患。

---

## 协议兼容性评估

### 序列化策略
- **框架**: 全部 `serde` + `serde_json`,无 protobuf / FlatBuffers / 自研二进制。
- **JSON-RPC 2.0**: `shared/protocol/src/jsonrpc.rs` 定义了 `JsonRpcRequest/Response/Error`,但 **id 类型是 `serde_json::Value`**(允许字符串/数字/null),这是一个开放契约。
- **版本号字段**: **无**。整个 crate 没有 wire-level 的 protocol version 字段或 envelope 包装器。演化完全依赖字段级 `#[serde(default, skip_serializing_if = "Option::is_none")]`。
- **字段演进策略**: **additive + opt-in** — 新字段全部 `#[serde(default, skip_serializing_if)]`;旧字段重命名通过文档约束("the wire spelling is the contract")。

### 关键正向做法
- **wire 字段名锁定测试** 大量存在:`commands.rs:236-256` 验证 `name`/`hint` 而不是 `key`/`description`;`session_thread.rs:746-825` 测试 `SessionSnapshot.last_run` 缺席语义;`events.rs:1018-1053` 测试 `RunRetrying` / `ModelResolved` / `ContextGauge` 与 gateway twin 的 wire 对齐。
- **`deny_unknown_fields` 选择性使用**:`WorkspaceListParams`, `AuditQueryParams` 等"收窄语义"的请求使用 `deny_unknown_fields`,防止拼错键悄悄扩大查询。
- **`skip_serializing_if` + `#[serde(default)]`** 配套使用,缺席语义明确(缺席 != null,参见 `session_thread.rs` 的完整论述)。
- **wire 常量统一**:`AGENTS.md` 和 `jsonrpc.rs` 头注释明确说明 `ADMIN_REQUIRED_MESSAGE`、`TOPIC_EVENT_METHOD`、`RATE_LIMITED` 等为什么必须只活在一个地方。
- **`SessionSnapshot` 三值化(unset/clear/value) 通过 `Option<T>` + `skip_serializing_if`**,文档明确"unset 不等于 off"。

### 向后兼容性风险
- **高风险点**: `shared/protocol/src/events.rs:13` 的 `StreamEvent` 和 `events.rs:458` 的 `AgentTraceEvent` — 这是 **wire 上最高频** 的两个枚举(每条 stream 帧都携带),任何新增 variant 都会让旧客户端反序列化失败。
- **中高风险点**: `auth.rs:9` 的 `Role` 枚举 — `Role::Owner`/`Guest`/`Anonymous`,新增 role(如 `Auditor`)将立刻破坏所有 `aleph-cli` 和 TUI 用户的 `Role` 反序列化路径。
- **风险点**: `receipt.rs:18` 的 `ReceiptCode` — 显式注释"existing spellings may never change",但 **没有 `#[non_exhaustive]`** 阻止开发者无意中破坏这条不变式。
- **影响范围**: `shared/client` 的所有 `Rpc` 反序列化路径;`shared/ui_logic` 的 wasm 客户端反序列化;`interfaces/tui`, `interfaces/cli`, `interfaces/webchat` 三处 indirect consumers。

---

## Findings

### [P1] `shared/protocol/src/providers/wire.rs:198` — `ProviderInfo.api_key` 字段暴露在响应中
**问题**:
```rust
#[serde(default)]
pub has_api_key: bool,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub api_key: Option<String>,
```
`ProviderInfo` 是 `providers.list` / `providers.get` 的响应 DTO。`api_key: Option<String>` 与 `has_api_key: bool` 同时存在 —— `skip_serializing_if` 仅在构造时设 `None` 才生效,但这是 **约定而非强制**。任何从数据库读取 provider 行时把 `api_key` 一起拷贝到 `ProviderInfo` 的代码路径,都会把完整 API key 推到所有调用 `providers.list` 的客户端。这是经典 "if we ever feel like sending it" 凭证泄漏 footgun。

**对比**: 同文件 `GenerationProviderConfigJson:195` 显式文档 "Client → server only. The server resolves keys from the vault and never sends one back",这是请求侧 DTO。但响应侧 `ProviderInfo` 没有这种约束,只有文档/代码评审守护。

**建议修复**:
1. **首选**: 从 `ProviderInfo` 中 **删除** `api_key` 字段(只保留 `has_api_key: bool`,需要 key 详情时用单独的 `provider.secret` RPC);
2. **或**: 在 `api_key` 字段上加 `#[serde(skip_serializing)]` 常驻反序列化,强制编译期/序列化期拒绝泄露;
3. **或**: 拆分为 `ProviderInfoPublic`(响应)与 `ProviderInfoFull`(服务端内部),后者 never serializes。

---

### [P1] `shared/protocol/src/events.rs:13,458` 等 58 个公开枚举 — 全部缺少 `#[non_exhaustive]`
**问题**:
整个 `shared/protocol/` 有 **58 个 `pub enum`**(grep 结果),其中 **0 个**使用 `#[non_exhaustive]`。最关键的几个:
- `events.rs:13 StreamEvent` (18 个 variants,wire 上每帧都携带)
- `events.rs:458 AgentTraceEvent` (20+ variants)
- `auth.rs:9 Role` (授权核心)
- `file_change.rs:20 FileChangeKind`, `:28 LineTag`, `:53 Unavailable`, `:134 Presentation`
- `receipt.rs:18 ReceiptCode` (显式约束"never rename")
- `commands.rs:119 CommandMatch<'a>` (虽然 lifetime-bound,仍是公开枚举)
- `thinking.rs:13 ReasoningStepType`, `:64 ConfidenceLevel`
- `events.rs:301/327/337/344/362/402` — `UncertaintyAction`, `AgentTraceState`, `AgentTraceTextKind`, `AgentTraceTurnOutcome`, `AgentTraceSessionOutcome`, `AgentTraceToolResult`
- `ui_logic/src/safety/prompt_injection.rs` 的 `PromptInjectionVerdict`
- 等等

`events.rs:374-378` 的注释承认问题:
```rust
/// the enum is a plain externally-tagged serde enum, not
/// `#[non_exhaustive]`), which is in-policy here because the three shipped
/// products are built and released from one `VERSION` tag
```
但这是 **in-policy today**,不是 **enforced ever**。任何后续开发者在 `StreamEvent` 加 variant 都不会触发编译期警告,而是会让所有运行旧 binary 的用户反序列化失败。

**为什么这是 P1 而不是 P2**:
- 这是 **wire contract**,失败是 silent parse error(`Unrecognized` 路径在 `classify_frame` 里有 `loud=true` 但也只是 `warn!`,不阻断);
- 修复成本极低(每个 enum 加一行 attribute),收益是结构性的;
- 整个 `shared/protocol/` 注释多次提到"renaming/renumbering 是 the renumber ratchet",但缺少 crate-level 的强制机制。

**建议修复**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]   // <-- 加这一行
pub enum StreamEvent { ... }
```
所有 58 个公开枚举都需要这个 attribute。`#[non_exhaustive]` 让任何新增 variant 在所有 match 表达式上变成编译错误 —— 这是 Rust 生态的标准 wire-stability 机制。

注意 `#[non_exhaustive]` 会破坏 exhaustive `match`,但这正是它的目的 —— 它要求任何 match 都明确处理 future variants。

---

### [P2] `shared/protocol/src/desktop_bridge/envelope.rs:14` — JSON-RPC id 类型与主 `jsonrpc.rs` 不一致
**问题**:
主协议 `shared/protocol/src/jsonrpc.rs` 的 `JsonRpcRequest.id: Option<Value>` (接受 string/number/null),而 `desktop_bridge/envelope.rs:14` 的 `Request.id: u64` (硬编码整数)。两套 envelope 共存,但 `Request::id` 是 **required**,而 `JsonRpcRequest::id` 是 **optional** —— desktop_bridge 通道无法承载 JSON-RPC notification (id=null)。

更糟的是 `desktop_bridge/envelope.rs` 是 `desktop_bridge/bin/export_desktop_bridge_schema.rs` 的 schema 源,如果某个桌面集成使用 TypeScript schema 生成客户端,它会假设 id 一定是 u64,与主 gateway 通信时会失败。

**建议修复**: 让 desktop_bridge envelope 与主协议共用 `JsonRpcRequest/Response/Error`,或在两套之间做显式转换。不要让两份 ID 类型共存。

---

### [P2] `shared/client/src/connection.rs:587` — 调试日志打印完整 StreamEvent 可能跨过 PII 边界
**问题**:
```rust
Inbound::Event(event) => {
    debug!("Parsed event: {:?}", event);   // <-- 整个 event 用 Debug 打印
    let _ = event_tx.send(*event).await;
}
```
`StreamEvent` 包含 `params: Value` 的 `tool_input` / `tool_output` / `content` 字段,这些是 **用户输入或模型输出**,可能含 PII(凭证、个人信息、API key)。`shared/logging` 的 `PiiScrubbingFormat` 会调用 `scrub_pii(&visitor.message)`,`{:?}` 格式化后的字符串会进 visitor 然后被 scrub,理论上 OK。

但 **有一个微妙的边界**:`StringVisitor::record_debug` 对非 `message` 字段使用 `field={:?}` 格式。`event` 字段名是 `event`,值是 `Debug`-格式化的 StreamEvent。整行最终是 `event=<debug-string>`,然后被 `scrub_pii` 处理 —— 这条路径是干净的。

**真正的问题在别处**: `shared/client/src/connection.rs:571` `debug!("Received raw message: {}", &text[..preview_end])` —— 这是 **未 scrubbed 的 raw JSON 文本**,只在 `debug` 级别才打印,所以不影响默认 INFO 级别。但 PII regex 是基于格式(Authorization: Basic xxxx, password=yyy),而 **JSON 内容里的 PII 不会被捕获**(`{"password": "xxx"}` 不会被 generic_secret 捕获,因为它的 key 后面是 `"` 不是 `=`)。这是一个已知的 PII-scrubber 设计权衡。

**建议修复**:
1. 把 debug 日志改成 `info!` 级别以下或受 `tracing::level_enabled!` 守护;
2. 或在 `PiiScrubbingFormat` 的 `record_str`/`record_debug` 中对 JSON 字段值递归调用 `scrub_pii`(成本: 高);
3. **或** 在 `RawMessagePreview` 字段中先尝试 `serde_json::from_str` 再 scrub —— 但这增加延迟。

**置信度**: 65% 这是一个 finding。Logging 是 opt-in 的 debug level,但代码路径正确,只是边缘情况。

---

### [P2] `shared/client/src/connection.rs:530` — 重连死锁风险
**问题**:
```rust
pub async fn reconnect(&self, config: &CliConfig) -> CliResult<()> {
    let _guard = self.reconnect_lock.lock().await;  // 持锁 30 秒 (handshake timeout)
    ...
    self.handshake(config).await?;  // 内部会发起 RPC
}
```
`reconnect_lock` 跨越整个 handshake(可达 30 秒)。如果 `call_with_timeout` 同时被持有调用方调用,它会立即返回 `Disconnected`(因为 `connected == false`),**不**会阻塞在 `reconnect_lock`。所以理论上不阻塞 RPC。

但 **有一个隐患**:`take_topic_events` 持 `topic_events: Arc<Mutex<Option<...>>>`(`std::sync::Mutex`),如果 `read_loop` 同时调用 `topic_tx.try_send` 和 `take_topic_events` 同时持锁—— 这是 std mutex,非 async,不会死锁。

**实际风险低**,但注释明确说 "Serialises the entire reconnect critical section" —— 这是设计选择,不是 bug。

**置信度**: 50%,不够 80% 阈值,**不报**为 P1。标记为 P2 以供 awareness。

---

## 违反红线的情况

未发现直接违反 `R1-R10` 的情况。具体到每条:
- **R1 (Core 不调用平台 API)**: `shared/client` 完全没有 platform-specific 代码(只用 tokio);`shared/ui_logic` 的 `wasm.rs` feature-gated 在 `wasm` feature 下,符合 "core never calls platform APIs" 的精神。
- **R4 (Interface layers 纯 I/O)**: `shared/client::connection.rs` 和 `shared/client::gateway_client.rs` 都是 I/O,业务逻辑在 `alephcore`。
- **R7 (One core, many shells)**: `shared/protocol` 正是为这个目标而存在;`shared/client` 是 the shell 共享的客户端;`shared/ui_logic` 是 TUI 和 webchat 共享的逻辑层。
- **R10 (智能在 prompt, 无中间件)**: 没有发现过度抽象层。`shared/ui_logic` 的 `AlephConnector` trait 只有一个 WASM 实现和零个 native 实现 —— 这是一个被设计但未完全使用的抽象(R10 YAGNI 信号),但不阻塞。

---

## 建议但不阻塞 (P3)

1. **`shared/protocol/src/commands.rs:119 CommandMatch<'a>`** —— 这是 `<'a>` 借用的 enum,客户端代码构造需要生命周期标注。考虑是否值得放在 crate root re-export,或者是否能让它变成 owned struct。

2. **`shared/protocol/src/canvas.rs` 大量 enum** (`TitleRejection`, `GeoForm`, `SizeKind`, `AiFrameStatus`, `Shape`, `CanvasOp`) 同样缺 `#[non_exhaustive]` —— 与 P1 同一问题的另一面,全部 enum 都需要这个 attribute。

3. **`shared/client/src/connection.rs` 有 11 个参数的方法** `read_loop` —— `#[allow(clippy::too_many_arguments)]` 已使用,文档解释了原因。可以接受,但每次重构前需重新评估。

4. **`shared/ui_logic/src/transcript/diff_view.rs:94,100` 和 `md_enhance.rs:94`** —— `s[i..].chars().next().unwrap()` 是 safe 的(循环 `while i < s.len()` 守卫),但看起来很 fragile。建议改用 `s[i..].chars().next().expect("i < s.len()")`。

5. **`shared/logging/src/pii_filter.rs:30` —— `PiiScrubbingLayer` deprecated 但保留** —— 这是 backward-compat API 留作 deprecated 警告。清晰且有警告;**不**是 finding。

6. **`shared/client/src/gateway_client.rs:126` — handshake timeout 5 秒** —— 比 `AlephClient::reconnect` 的 10 秒短。如果冷启动较慢(磁盘加密容器),可能不够。但 5 秒是合理的起点。

7. **`shared/client/src/connection.rs:530` `reconnect_lock` 持锁长达 30 秒** —— 不阻塞但可能影响 responsiveness。如果 `connected == false`,`call_with_timeout` 已经快速失败,所以一般不会感知。但运维监控角度值得知道。

8. **`shared/protocol/src/team_topic.rs:50 TeamTopicKind` 缺 `#[non_exhaustive]`** —— 与 P1 同根问题,统一修复。

---

## 核心结论

`shared/` 模块整体 **工程质量高**:
- 详尽的 doc comment,每个 wire 类型都有 `Why this lives here` 的 R7-style 论据;
- 大量 wire-compat 测试(events.rs:1018-1053 的 `run_retrying_deserializes_from_gateway_frame_shape` 等是 gold-standard);
- 错误处理健全(`Classify` 函数用 `Unrecognized` 作为 safety hatch,从不假设 client 知道所有 status word);
- 凭证处理有专门模块(`shared/logging/src/pii.rs`)。

但有两个 **结构性弱点**:
1. **缺 `#[non_exhaustive]` 的协议枚举**(P1)—— 58 个 enum,零防御,新增 variant 是 silent footgun;
2. **`ProviderInfo.api_key` 字段暴露在响应 DTO 中**(P1)—— 凭证泄漏 footgun,需要结构性修复(删除/拆分/强制 skip_serializing)。

修复这两个 P1 后,该模块是 production-ready。