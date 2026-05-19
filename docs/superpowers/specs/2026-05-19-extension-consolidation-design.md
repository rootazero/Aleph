# 设计 Spec:插件系统整固 —— 修复 McpServer 接线 + 撤回死能力

- **日期**:2026-05-19
- **分支**:`extension-consolidation`(worktree:`.worktrees/extension-consolidation`)
- **范围**:`src/extension/` 子系统
- **类型**:bug 修复 + 死代码撤回(无破坏性重构)

---

## 1. 背景与动机

参照 hermes-agent(Python 进程内插件系统)对比 Aleph 的 `src/extension/`(60+ 文件,~21,990 行)。
结论:**Aleph 在热重载、显式 `runtime` 声明、WASM/Extism 沙箱、分级权限模型上已领先 hermes;
唯一实质差距是若干"基础设施建好却从未接通"的能力。**

`src/extension/` 声明了 13 个能力类型(`CapabilityDeclaration` 枚举)。其中:

- **6 个完整接线、正常工作**:Tool / Hook / Skill / Command / Agent / Service。
- **1 个有真 bug**:McpServer —— 插件声明的 MCP server 永远到不了 `McpManager`。
- **5 个死能力区**:Channel / Provider / GatewayMethod / Cli / HttpRoute+HttpHandler ——
  注册进 `PluginRegistry` 后零消费者。

按 CLAUDE.md **R10**(「任何"零现有消费者"的抽象立即删除/撤回,绝不"为未来留口"」)
→ 撤回 5 个死能力区;按用户目标「重点修复错误」→ 修复 McpServer。

---

## 2. 已核实的问题证据

### 2.1 McpServer:插件 MCP server 到不了 McpManager(真 bug)

死代码链(逐项核实):

1. `src/extension/mod.rs:281-298` —— `load_all()` 把 `CapabilityDeclaration::McpServer`
   收集进**函数局部变量** `mcp_configs: Vec<(String, McpServerConfig)>`,函数返回时即丢弃。
   源码注释直言 `// Collect MCP configs (no-op in CapabilityApi dispatch)`。
2. `src/extension/registrar/api.rs:110-112` —— `CapabilityApi::dispatch()` 对 `McpServer`
   是空操作:`// No-op: MCP servers are handled by the loader, not the registry`。
3. `src/extension/mcp_config.rs` —— `read_mcp_json()` 把插件目录下 `.mcp.json` 解析为
   `HashMap<String, McpManagerConfig>`。它在 `src/extension/loader.rs:158`
   `load_mcp_plugin()` 中被调用,结果存入 `PluginLoader.mcp_configs`。
4. **但 `PluginLoader.mcp_configs` 从不被取出** —— `get_mcp_configs()` /
   `all_mcp_configs_map()` 全仓零调用。
5. `McpManager` 仅从磁盘 `~/.aleph/data/mcp_config.json` 初始化;
   `McpManagerHandle::add_server()`(`src/mcp/manager/handle.rs:68`)支持运行时动态添加,
   但从未为插件调用过。

净结果:无论插件经 `.mcp.json` 还是 manifest 声明 MCP server,都无法在运行时生效。

### 2.2 Channel:按错误的进程内模型设计(死代码)

- `src/extension/channel_manager.rs`(701 行)`ChannelManager` 实现完整、带测试,
  但仅在 `src/extension/mod.rs:56` 被 `pub use`,**零真实消费者**;
  `ExtensionManager` 甚至不持有 `ChannelManager` 字段。
- 设计缺陷:`ChannelManager` 用进程内 `tokio::mpsc::Sender/Receiver`,
  约定「插件经 `take_incoming_sender()` 写入、经 `take_outgoing_receiver()` 读取」。
  但 Aleph 插件是 **WASM(Extism)/ MCP(进程外)**,**无法跨 IPC 边界持有 Rust mpsc 句柄**。
- 佐证:`loader.rs:275` —— `call_tool()` 对 MCP 插件直接返回 `Err`;
  WASM 宿主函数仅 4 个(`host_log` / `host_now_millis` / `host_workspace_read` /
  `host_secret_exists`),**不存在任何插件→宿主推送/事件机制**。
- 因此 `ChannelManager` 不是「缺连线」,而是按 Aleph 根本不存在的插件模型建造。
  正确的插件通道需先设计 IPC 推送机制 —— 属新功能,不在本周期。按 R10 撤回。

### 2.3 Provider / GatewayMethod / Cli / HttpRoute+HttpHandler(死代码)

- `src/extension/provider_adapter.rs`(294 行)`PluginProviderAdapter` —— 仅 `pub use`,零消费者。
- `src/extension/http_handler.rs` `PluginHttpHandler` —— 仅 `pub use`,registry 存
  `http_handlers` 却无 dispatch、无 HTTP server 接入。
- `GatewayMethod` / `Cli` —— `PluginRegistry` 有 `register_*`/`get_*`/`list_*`,从不被读取。
- 全部 bundled 插件(`plugins/*/`)与示例(`examples/plugins/`)**均未声明**这些能力
  (仅声明 Tool / Hook / Service)。零插件 + 零消费者 → R10 撤回。

---

## 3. Part A —— 修复 McpServer 接线

### 3.1 目标

插件声明的 MCP server 在 `load_all()` 后真正注册进运行中的 `McpManager`,可被 agent 调用;
`reload()` / 卸载时同步移除。

### 3.2 设计

1. **依赖注入**:`ExtensionManager` 新增可选字段 `mcp_handle: RwLock<Option<McpManagerHandle>>`,
   经新增 setter `set_mcp_handle()` 注入 —— 完全照搬已有 `set_tool_registry()`
   模式(`src/extension/mod.rs:261`),零新范式。
2. **注册时机**:`load_all()` 解析完插件后,从已工作的 `.mcp.json → McpManagerConfig`
   路径收集每个插件的 MCP server,对每个调用 `McpManagerHandle::add_server()`。
   `unregister_plugin()` / `reload_plugin()` 对应调用 `remove_server()`。
3. **路径收敛**:统一到 `.mcp.json → McpManagerConfig`(`read_mcp_json` 已就绪、
   transport-aware)。删除 `load_all()` 中那段死的 `mcp_configs` 局部变量收集逻辑。
   > 实现规划阶段需精读 `loader.rs:157-172` 与 manifest 的 MCP 段,确定
   > `CapabilityDeclaration::McpServer(McpServerConfig)` 这一极简变体是否仍由任何
   > manifest 段产生。若无产生者,则该变体随收敛一并移除(属"修复"的一部分,
   > 非新增"删第 6 个能力");若有,则路由到同一注册入口。
4. **启动接线**:`src/bin/aleph-server/commands/start/builder/` 中,
   `McpManager` 与 `ExtensionManager` 构造完成后,把 `McpManagerHandle`
   经 `set_mcp_handle()` 注入,且在 `load_all()` 调用之前完成注入。

### 3.3 错误隔离

单个插件 MCP server 注册失败(进程拉起失败等)不得中断 `load_all()`;
失败记入 `LoadSummary.errors` 并 `tracing::warn!`,与现有插件解析失败处理一致
(`mod.rs:323-329`)。

### 3.4 测试(TDD,先行)

- 单测:构造携带 `.mcp.json` 的临时插件目录,注入 mock/真实 `McpManagerHandle`,
  `load_all()` 后断言该 server 出现在 manager 中。
- 单测:`unregister_plugin()` 后断言 server 被 `remove_server()`。
- 单测:某插件 MCP 配置非法时 `load_all()` 不 panic、其余插件仍加载、错误进 `LoadSummary`。

---

## 4. Part B —— 撤回 5 个死能力区

外科式删除。**原则**:每个被删项必须 grep 追溯到 `src/extension/` 内外零残留引用。

### 4.1 逐能力删除清单

**Channel**

- `capability.rs`:`CapabilityDeclaration::Channel` 变体及其 `tier()`/`kind_name()`/
  `required_permission()` 分支。
- `registrar/api.rs`:`dispatch()` 的 `Channel` 分支 + 相关测试。
- `registry/types.rs`:`ChannelRegistration` 结构体 + 测试。
- `registry/plugin_registry/mod.rs`:`channels` 字段、`register_channel`/`get_channel`/
  `list_channels`/`list_channels_by_order`、`clear()` 行、`unregister_plugin()` retain、
  `RegistryStats.channels`。
- 整文件删除:`channel_manager.rs`(701 行)。
- `mod.rs`:`mod channel_manager` + `pub use channel_manager::{...}`。
- `types/runtime.rs`:`ChannelInfo` / `ChannelMessage` / `ChannelSendRequest` / `ChannelState`
  (确认无 `src/extension/` 外引用)。
- `error.rs`:`ExtensionError::ChannelNotFound` 变体。
- manifest:`ChannelSection`(`toml_types.rs` / `types.rs` / `manifest/mod.rs`)。

**Provider**

- `capability.rs`:`Provider` 变体及分支;`provider_adapter.rs::ProviderDeclaration`/
  `ProviderRegistration` 引用。
- `registrar/api.rs`:`Provider` dispatch 分支 + 测试。
- `registry/types.rs`:`ProviderRegistration` + 测试。
- `registry/plugin_registry/mod.rs`:`providers` 字段及 `register_*`/`get_*`/`list_*`、
  `clear()`、`unregister_plugin()` retain、`RegistryStats.providers`。
- 整文件删除:`provider_adapter.rs`(294 行)。
- `mod.rs`:`pub use provider_adapter::PluginProviderAdapter`。
- `types/runtime.rs`:`ProviderChatRequest` / `ProviderChatResponse` / `ProviderMessage`。

**GatewayMethod**

- `capability.rs`:`GatewayMethod` 变体、`GatewayMethodDeclaration` 类型别名、各分支。
- `registrar/api.rs`:dispatch 分支 + 测试。
- `registry/types.rs`:`GatewayMethodRegistration` + 测试。
- `registry/plugin_registry/mod.rs`:`gateway_methods` 字段及方法、`RegistryStats.gateway_methods`。
- `types/plugins.rs`:`PluginRecord.gateway_methods` 字段及 `from_adapter_output` 相关行。

**Cli**

- `capability.rs`:`Cli` 变体、`CliDeclaration` 别名、各分支。
- `registrar/api.rs`:dispatch 分支 + 测试。
- `registry/types.rs`:`CliRegistration` + 测试。
- `registry/plugin_registry/mod.rs`:`cli_commands` 字段及方法、`RegistryStats.cli_commands`。

**HttpRoute + HttpHandler**

- `capability.rs`:`HttpRoute` 变体、`HttpRouteDeclaration` 别名、各分支。
- `registrar/api.rs`:dispatch 分支 + `make_http_route` 等测试。
- `registry/types.rs`:`HttpRouteRegistration` / `HttpHandlerRegistration` + 测试。
- `registry/plugin_registry/mod.rs`:`http_routes` / `http_handlers` 字段及方法、
  `RegistryStats` 对应字段。
- 整文件删除:`http_handler.rs`(`match_path` / `PluginHttpHandler`)。
- `mod.rs`:`mod http_handler` + `pub use http_handler::{...}`。
- manifest:`HttpRouteSection`、`http_routes`/`http_routes_v2` 字段
  (`toml_types.rs` / `types.rs` / `manifest/mod.rs`)。

### 4.2 跨切面收尾

- `capability.rs`:`Tier::GatewayExtension` 在 GatewayMethod/HttpRoute/Cli 全删后变空 →
  移除该 tier 变体及 `register_capability()` 中对应 match 臂;更新 tier 文档注释。
- `manifest/types.rs`:`PluginPermission::HttpRoutes` 与 `::GatewayRpc` 删除后无引用 →
  移除变体及其 `Display`、字符串解析臂(`"http-routes"` / `"gateway-rpc"`);
  确认权限解析对未知字符串仍优雅处理(不 panic)。
- `types/plugins.rs:316-324`:`from_adapter_output` 中针对被删能力的 `CapabilityDeclaration`
  match 臂相应清理。
- `registry/mod.rs` / `registry/types.rs` 顶部文档注释中列举被删能力的行。
- 删除所有针对被删能力的单测(`api.rs`、`registry/types.rs`、`capability.rs` 内)。

### 4.3 外部消费者复核(规划阶段必做)

grep 已初步确认 Channel/Provider 等类型集中在 `src/extension/` 内。规划阶段仍须复核:

- `packages/plugin-sdk/`(是否引用被删 manifest 段/类型)。
- `examples/plugins/`(`media-video/aleph.plugin.toml` 仅 `[[tools]]`/`[[hooks]]`,初判安全)。
- `src/gateway/handlers/plugins/`(插件 RPC handler 是否触及被删 registry 方法)。
- `bindings/`、`apps/`、`tests/`。

若发现 `src/extension/` 外的消费者,**暂停并向用户报告**,不擅自扩大改动面。

---

## 5. 非目标(YAGNI)

- 不新建插件→宿主推送机制、不实现插件通道功能。
  (正确的、基于 MCP 长驻服务的插件通道设计留作独立周期,由 Part A 解锁。)
- 不改动 6 个正常能力(Tool/Hook/Skill/Command/Agent/Service)。
- 不跑全项目 `cargo fmt`(main 非 rustfmt-clean,约 130 文件漂移 —— 见项目记忆)。
  仅对本周期改动的文件做局部 `cargo fmt`。
- 不顺手重构相邻无关代码。

---

## 6. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 删除波及 `src/extension/` 外 | 规划阶段全仓 grep 复核;发现外部消费者即暂停报告 |
| `McpManager` 启动顺序依赖 | `set_mcp_handle()` 在 `load_all()` 前注入;handle 为 `Option`,缺失时降级跳过并 warn |
| `PluginPermission` 变体删除破坏 manifest 解析 | 保留未知权限字符串的优雅处理;为权限解析补回归测试 |
| 基线已有失败测试 | 项目记忆载明 main 有 8 lib + 4 集成测试预存失败;只断言"无**新增**失败" |

---

## 7. 验证标准

- `cargo check -p alephcore` 通过,无新增 warning。
- `cargo test -p alephcore --lib`:extension 相关测试全绿;对比基线无新增失败。
- Part A:新增 MCP 接线测试通过(见 3.4)。
- Part B:`rg` 确认被删标识符零残留;`cargo build` 链接通过。
- 改动文件局部 `cargo fmt` + `cargo clippy`(仅改动文件)无新增 lint。

---

## 8. 工作流

1. 已建立 worktree `.worktrees/extension-consolidation`(分支 `extension-consolidation`)。
2. 本 spec 提交至该分支。
3. 进入 `writing-plans` 生成分阶段实现计划:
   - Phase 1:Part A(McpServer 修复,TDD 先行)。
   - Phase 2:Part B(5 能力撤回,逐能力删除 + 编译验证)。
   - Phase 3:跨切面收尾 + 全量验证。
4. 实现期每阶段 `cargo check` + 测试;完成后代码审查。
5. 仅合并不在 worktree 会话内删除 worktree(见 CLAUDE.md worktree 注意事项)。

---

## 9. 预期净效果

- 修复 1 个真 bug:插件 MCP server 真正可用。
- 移除约 1,500–2,000 行死/误设计代码(`channel_manager.rs` 701 + `provider_adapter.rs` 294
  + `http_handler.rs` + registry/manifest/types 各处)。
- 插件系统从「声明 13 能力、实则 6 个能用」收敛为「6 个能力诚实可用 + MCP 插件真正工作」。
- 完全遵守 R10(薄 Harness / YAGNI 撤回)、R3(核心轻量化);无破坏性重构。
