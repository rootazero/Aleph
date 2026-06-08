# 记忆扩展生命周期连线（X1）

> Spec date: 2026-06-08
> Scope: wire the MemoryExtension lifecycle end-to-end — bind the MCP caller (Task 11),
> fire `on_delegation` + `on_pre_compress`, and honestly handle the unfireable `on_session_switch`.
> Branch: `fix/memory-extension-lifecycle-x1` (worktree, off `main`)

## 1. 背景

Gap C 审计把 X1 标为「可插拔记忆扩展消费侧在生产中不工作」：

- `MemoryExtensionRegistry` 的三个生命周期 dispatch 点
  (`on_session_switch` / `on_pre_compress` / `on_delegation`) **已完整实现且有测试**，
  但生产 **0 caller**（`registry.rs:169/188/212` 留有 wire-point TODO）。
- 第三方 MCP `[memory]` 插件经 `register_memory_extension_if_declared`
  (`extension/loader.rs:401`) 注册为 `McpMemoryExtension`，但其 caller 是
  `UnboundMcpCaller`——**每次调用都返回 "Task 11 will wire the real McpManager" 错误**。
  整个第三方 MCP 记忆扩展特性已发布但**静默失效**。

X1 = 让 MemoryExtension 生命周期真正端到端可用。`on_retrieve` / `on_capture` 两个 hook
已连线（`memory_context_provider` / `insert_helper`）；本 spec 补齐其余。

## 2. 设计原则

- **连线优先**：所有 hook 的 dispatch 层、超时/fail-safe 策略、MCP adapter 均已存在且测试。
  本期只接生产 caller + 绑定真实 McpManager，不重写 hook 逻辑。
- **非破坏性**：`UnboundMcpCaller` 保留为绑定前/热加载竞态的优雅降级默认；`caller` 字段改
  `ArcSwap` 后未绑定行为逐字节不变。新增 trait 参数用 `Option` 保持向后兼容。
- **R10 诚实**：只连有真实生产者的 hook。`on_session_switch` 的合约（mint 新 session_id +
  rotation reason）在 Aleph 侧**无匹配的生产事件**（会话只新建 / 原地压缩 / 删除，从不轮转
  id），故本期**不接生产者**——见 §6。

## 3. 范围（3 个组件）

### C1 — MCP caller 绑定（Task 11 核心）`[HIGH]`

**现状**：`McpMemoryExtension.caller: Arc<dyn McpCaller>` 是不可变字段，注册时填
`UnboundMcpCaller`。`ExtensionManager` 已同时持有 `mcp_handle:
RwLock<Option<McpManagerHandle>>`（`set_mcp_handle`）和 `memory_registry:
RwLock<Option<Arc<MemoryExtensionRegistry>>>`（`set_memory_registry`）——绑定所需的两端齐备。

**改法（ArcSwap rebind + 类型边表，已确认决策）**：

1. **`mcp_adapter.rs`**：`McpMemoryExtension.caller` 由 `Arc<dyn McpCaller>` 改为
   `arc_swap::ArcSwap<dyn McpCaller>`（`arc-swap = "1"` 已是依赖）。dispatch 读经 `.load()`。
   新增 `pub fn rebind(&self, caller: Arc<dyn McpCaller>)` = 一次 `store`。
   `McpMemoryExtension::new` 用 `ArcSwap::new(caller)` 初始化。

2. **`mcp_adapter.rs`**：新增真实 caller
   ```
   pub struct ManagerBackedMcpCaller { handle: McpManagerHandle, server_id: String }
   #[async_trait] impl McpCaller for ManagerBackedMcpCaller {
       async fn call(&self, method, args) -> Result<Value> {
           let client = self.handle.get_client(&self.server_id).await?
               .ok_or_else(|| AlephError::other(format!(
                   "memory MCP server '{}' not running", self.server_id)))?;
           let res = client.call_tool(method, args).await?;   // McpToolResult
           Ok(serde_json::to_value(res)?)                      // 映射回 Value
       }
   }
   ```
   server 未运行（`get_client` → `None`）时返回 Err——dispatch 层 per-hook 超时+warn 已优雅降级。

3. **`registry.rs`**：新增类型边表
   `mcp_bindings: RwLock<Vec<Arc<McpMemoryExtension>>>` + `pub fn register_mcp(&self, ext:
   Arc<McpMemoryExtension>)`（把**同一个** `Arc` upcast 进主 `extensions` 列表，同时存进边表保留具
   体类型）+ `pub fn replace_caller(&self, plugin_name: &str, caller: Arc<dyn McpCaller>) -> bool`
   （边表按 `name()` 命中后 `rebind`，返回是否命中）。主列表与边表指向同一对象，故 rebind 对
   dispatch 立即可见。

4. **`extension/loader.rs`**：`register_memory_extension_if_declared` 改用 `registry.register_mcp`
   注册（而非 `register`），使 MCP 扩展进边表。

5. **`extension/mod.rs`**：新增 `pub async fn bind_memory_callers(&self)`——读 `mcp_handle` +
   `memory_registry`（都 Some 才进行），对每个已注册 MCP 记忆扩展解析 server_id，
   构造 `ManagerBackedMcpCaller` 调 `registry.replace_caller`。

6. **启动接线** (`bin/aleph-server/.../agent_init` 或 boot 序列)：在
   `set_mcp_handle` + `set_memory_registry` + 插件加载之后调一次 `bind_memory_callers()`。
   热加载（`load_plugin_with_memory` 跑时若 `mcp_handle` 已 Some）立即绑定该插件。

**server_id 解析假设**（`[memory]` section 不声明 server id）：记忆插件的 hooks 由**恰好一个**
MCP server 承载，解析为该插件唯一注册的 server（`plugin:{plugin_id}/*`，经
`PluginLoader::get_mcp_configs(plugin_id)`）；若 >1 server，取第一个并 `warn!`。

**熵减**：`UnboundMcpCaller` **保留**——绑定前 / 热加载竞态的正确降级默认，其 doc 注释已描述本
`replace_caller` 流程。

### C2 — `on_delegation` 连线 `[MEDIUM]`

**现状**：`subagent_tool` 已持有 `capture_registry: Option<Arc<MemoryExtensionRegistry>>`
(`agents/subagent_tool/mod.rs:74`)。子代理完成路径有 `result_summary` + parent/child session id。

**改法**：子代理 run 返回、`result_summary` 构造完成后，构造 `DelegationCtx{agent_id,
namespace, parent_session_id, child_session_id, task, result_summary}` 调
`registry.dispatch_on_delegation(&ctx).await`。fire-and-forget 语义（dispatch 内部已 per-hook
超时+warn，父代理已拿到结果，不阻塞）。`capture_registry` 为 None 时跳过。

### C3 — `on_pre_compress` 连线 `[MEDIUM]`

**现状**：`compress_to_notes` (`compression/service.rs:219`) 是真实 L1 压缩管线（每次压缩必经）。
extract/ingest 经 `CompoundIngestor::ingest_batch(agent_id, raws)`——签名**不接** extra context。

**改法**：

1. `CompressionService` 加可选依赖 `extension_registry: Option<Arc<MemoryExtensionRegistry>>`
   + builder `with_extension_registry(self, reg) -> Self`（镜像现有 `with_command_handler` 等
   可选依赖模式）。
2. `CompoundIngestor::ingest_batch` 加参数 `extra_context: Option<&str>`（内部 trait，非公开插件
   API）。真实实现 `DefaultCompoundIngestor` (`notes/ingest/ingestor.rs:296`) 把非空 extra_context
   前置进 ingest LLM prompt；3 个 test mock (`service.rs:682/755/791`) + ingestor 内 mock 仅补签名。
3. `compress_to_notes` 内、`ingest_batch` 调用**之前**：若有 registry，构造
   `PreCompressCtx{agent_id, namespace, session_id, messages_count, oldest_at, newest_at}` 调
   `dispatch_on_pre_compress(&ctx).await`，把返回文本（空串=无贡献）作为 `extra_context` 传入
   `ingest_batch`。

### on_session_switch — 保留合约，不接生产者（决策见 §6）

不连任何生产者；hook / `SessionSwitchCtx` / `SessionSwitchReason` / `dispatch_on_session_switch`
/ MCP adapter 实现 / 测试**全部保留**，作为第三方 MCP 插件可实现的 API hook（随 C1 绑定后可被第三方
消费）。`registry.rs` 的 wire-point TODO 注释更新为：「Aleph 侧暂无 session-id 轮转事件，故不接生产
者；待轮转模型出现再连」。

## 4. 数据流

```
第三方 MCP [memory] 插件加载
  └─ register_memory_extension_if_declared → registry.register_mcp(McpMemoryExtension{UnboundMcpCaller})
boot: set_mcp_handle + set_memory_registry + 插件加载完成
  └─ ExtensionManager::bind_memory_callers()
        └─ 每个 MCP 记忆扩展: 解析 server_id → replace_caller(name, ManagerBackedMcpCaller)
              └─ McpMemoryExtension.caller.store(real)   ← dispatch 立即可见

运行期 hook 触发:
  on_retrieve   (已连) memory_context_provider
  on_capture    (已连) insert_helper
  on_pre_compress (C3) compress_to_notes → dispatch → extra_context → ingest_batch prompt
  on_delegation   (C2) subagent_tool 子代理完成 → dispatch
  on_session_switch    保留 API，Aleph 侧不 fire
```

## 5. 测试策略

- **C1**：单测——`McpMemoryExtension` rebind 后 dispatch 见到新 caller；`ManagerBackedMcpCaller`
  经 canned `get_client` 路径映射 method→tool（可用现有 `CannedCaller` 模式 + 一个 stub handle）；
  `registry.register_mcp` + `replace_caller` 命中/未命中返回值；server 未运行→Err 被 dispatch warn-skip。
- **C2**：单测——子代理完成后 `RecordDelegationExt` 收到正确 task/result_summary（复用 registry.rs
  既有测试 stub）。
- **C3**：单测——有 registry 时 `compress_to_notes` 把 `PreCompressContribExt` 的贡献作为
  extra_context 传入 ingest（mock ingestor 断言收到非空 extra_context）；无 registry / 空贡献时
  extra_context = None / 不改 prompt。
- 删除项无（C1-C3 纯新增 + 1 个内部 trait 参数）；trait 参数变更靠编译器强制所有 impl 更新。

> **项目协议约束**：按用户「资源并发治理」强制要求，**完成任务后不进行 cargo check / 测试校验，
> 直接提交**。本 spec 列出测试期望作为正确性参照；caller-verification grep 守卫替代编译器
> （同 Gap C）。trait 签名变更的全 impl 覆盖用 grep 守卫核验（`fn ingest_batch` 全命中单参→双参）。

## 6. 明确不做（附理由）

| 项 | 理由 |
|---|---|
| **`on_session_switch` 接生产者** | Aleph 侧无 session-id 轮转事件：`handle_compact_db` 原地压缩（不 mint 新 id）、`handle_reset_db` 原地重置、`handle_delete_db` 走独立 SessionEnd 路径。合约的 `new_session_id`+rotation reason 无匹配生产事件，强连=制造假消费者（违 R10）。保留 hook 作为第三方 API 表面。 |
| **删除 `on_session_switch` hook 表面** | 用户决策保留：它是第三方 MCP 插件可实现的 published hook，缺的是 Aleph 侧生产者而非消费者。 |
| **CLI /resume,/branch,/reset,/new 各 handler 连 session_switch** | 同上——这些是原地操作，且 session RPC handler 未线 registry，超范围。 |
| **Gap A 实体图谱 / Gap B capture 冲突决策** | 各自独立 spec（Gap C 已立项）。 |

## 7. 安全重构守则

- **分支隔离**：实现全程 worktree 分支 `fix/memory-extension-lifecycle-x1`，不直接触碰 main（本 spec
  doc 除外，append-only 提交到 main）。
- **非破坏性**：`ArcSwap` 改造 + `UnboundMcpCaller` 保留 + `extra_context: Option` 保证向后兼容；
  空 registry / 未绑定 / 空贡献逐字节回归。
- **熵减**：本期纯连线，无死代码可删（C1-C3 均补真实生产 caller）；不为 `on_session_switch` 留假连线。

## 8. 涉及文件清单

| 文件 | 改动 |
|---|---|
| `src/memory/extensions/mcp_adapter.rs` | C1: `caller` 改 `ArcSwap` + `rebind` + `ManagerBackedMcpCaller` |
| `src/memory/extensions/registry.rs` | C1: `mcp_bindings` 边表 + `register_mcp` + `replace_caller`；更新 session_switch TODO 注释 |
| `src/extension/loader.rs` | C1: `register_memory_extension_if_declared` 改用 `register_mcp` |
| `src/extension/mod.rs` | C1: `bind_memory_callers()` |
| `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`（或 boot 序列） | C1: 调 `bind_memory_callers()` |
| `src/agents/subagent_tool/mod.rs`（及其完成路径子模块） | C2: 子代理完成后 `dispatch_on_delegation` |
| `src/memory/compression/service.rs` | C3: `with_extension_registry` + `compress_to_notes` 内 dispatch + extra_context 传递；3 mock 补签名 |
| `src/memory/notes/ingest/ingestor.rs` | C3: `ingest_batch` 加 `extra_context: Option<&str>` + prompt 前置 |
