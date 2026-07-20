# 设计 Spec:插件系统整固 —— 撤回死能力区

- **日期**:2026-05-19
- **分支**:`extension-consolidation`(worktree:`.worktrees/extension-consolidation`)
- **范围**:`src/extension/` 子系统
- **类型**:死代码撤回(纯 R10 清理,无破坏性重构,无新功能)

---

## 1. 背景与动机

参照 hermes-agent(Python 进程内插件系统)对比 Aleph 的 `src/extension/`(60+ 文件,~21,990 行)。
结论:**Aleph 在热重载、显式 `runtime` 声明、WASM/Extism 沙箱、分级权限模型上已领先 hermes;
唯一差距是若干"基础设施建好却从未接通"的能力。**

`src/extension/` 的 `CapabilityDeclaration` 枚举声明 12 个能力类型。其中:

- **6 个完整接线、正常工作**:Tool / Hook / Skill / Command / Agent / Service。
- **5 个死能力区**:Channel / Provider / GatewayMethod / Cli / HttpRoute+HttpHandler ——
  注册进 `PluginRegistry` 后零消费者,且无任何 bundled / 示例插件声明。
- **1 个待专项处理**:McpServer —— 见 §5「延后工作」。

按 CLAUDE.md **R10**(「任何"零现有消费者"的抽象立即删除/撤回,绝不"为未来留口"」)
→ 本周期撤回 5 个死能力区。

## 2. 范围演进记录

调研中两次推翻"缺连线"的初始前提,据此两度收敛范围:

1. **Channel**:`ChannelManager`(701 行)用进程内 `tokio::mpsc` 句柄,要求插件持有
   Rust `Sender`/`Receiver`;但 Aleph 插件是 WASM(Extism)/ MCP(进程外),
   `loader.call_tool()` 对 MCP 插件直接返回 `Err`,WASM 仅 4 个宿主函数、无插件→宿主
   推送机制。`ChannelManager` 是按 Aleph 不存在的"进程内插件模型"建造 —— 非"缺连线",
   按 R10 撤回。
2. **McpServer**:本拟修复其接线 bug,但核实发现 `McpManagerActor` 是完全死代码
   (`McpManagerActor::new()` 仅测试调用,启动时从不 spawn),`McpClient::start_external_server()`
   启动时也从不被调用 —— Aleph 的 MCP server 运行时**整体未接线**(配置端、插件端都不工作)。
   "修 McpServer"实为一项需独立设计的特性,不属本周期。详见 §5。

最终范围:**仅撤回 5 个确凿死能力区**。零新功能、零 gateway 改动、零运行时风险。

## 3. 已核实的问题证据

5 个死能力区的共同特征:`CapabilityApi::dispatch()`(`src/extension/registrar/api.rs`)
把它们写入 `PluginRegistry` 的对应集合,但**没有任何代码读取这些集合**。

- `src/extension/channel_manager.rs`(701 行)`ChannelManager` —— 仅 `src/extension/mod.rs:56`
  `pub use`,`ExtensionManager` 不持有该字段,零真实消费者。
- `src/extension/provider_adapter.rs`(294 行)`PluginProviderAdapter` —— 仅 `mod.rs:62`
  `pub use`,零消费者。
- `src/extension/http_handler.rs` `PluginHttpHandler` / `match_path` —— 仅 `mod.rs:59`
  `pub use`;`PluginRegistry` 存 `http_handlers`/`http_routes` 却无 dispatch、无 HTTP server 接入。
- `GatewayMethod` / `Cli` —— `PluginRegistry` 有 `register_*`/`get_*`/`list_*`,从不被读取。
- 全部 bundled 插件(`plugins/*/`)与示例(`examples/plugins/`)**均只声明** Tool / Hook /
  Service,**无一**声明这 5 类能力。

零插件 + 零消费者 → R10 撤回。

## 4. 实施 —— 撤回 5 个死能力区

外科式删除。**原则**:每个被删项必须 `rg` 追溯到 `src/extension/` 内外零残留引用;
每步删除后 `cargo check -p alephcore` 通过。

### 4.1 逐能力删除清单

**Channel**

- `capability.rs`:`CapabilityDeclaration::Channel` 变体、`ChannelDeclaration` 类型别名、
  `tier()`/`kind_name()`/`required_permission()` 中的 `Channel` 分支、测试
  `test_all_12_variants_have_kind_names` 中的 Channel 条目(并把断言数 12→对应下调)。
- `registrar/api.rs`:`dispatch()` 的 `Channel` 分支 + `make_channel` 等相关测试。
- `registry/types.rs`:`ChannelRegistration` 结构体 + 测试。
- `registry/plugin_registry/mod.rs`:`channels` 字段、`register_channel`/`get_channel`/
  `list_channels`(及任何 `list_channels_by_order`)、`clear()` 行、`unregister_plugin()` retain、
  `RegistryStats.channels`。
- 整文件删除:`channel_manager.rs`(701 行)。
- `mod.rs`:`mod channel_manager;` + `pub use channel_manager::{ChannelHandle, ChannelManager};`。
- `types/runtime.rs`:`ChannelInfo` / `ChannelMessage` / `ChannelSendRequest` / `ChannelState`
  及 `types/mod.rs` 对应 `pub use`(规划阶段确认 `src/extension/` 外无引用)。
- `error.rs`:`ExtensionError::ChannelNotFound` 变体。
- manifest:`ChannelSection`(`toml_types.rs` / `types.rs` / `manifest/mod.rs` 的相关字段与导出)。

**Provider**

- `capability.rs`:`Provider` 变体、`ProviderDeclaration` 别名、各分支、测试条目。
- `registrar/api.rs`:`Provider` dispatch 分支 + `make_provider` 等测试。
- `registry/types.rs`:`ProviderRegistration` + 测试。
- `registry/plugin_registry/mod.rs`:`providers` 字段及 `register_provider`/`get_provider`/
  `list_providers`、`clear()`、`unregister_plugin()` retain、`RegistryStats.providers`。
- 整文件删除:`provider_adapter.rs`(294 行)。
- `mod.rs`:`mod provider_adapter;` + `pub use provider_adapter::PluginProviderAdapter;`。
- `types/runtime.rs`:`ProviderChatRequest` / `ProviderChatResponse` / `ProviderMessage`
  及 `types/mod.rs` 对应 `pub use`。

**GatewayMethod**

- `capability.rs`:`GatewayMethod` 变体、`GatewayMethodDeclaration` 别名、各分支、测试条目。
- `registrar/api.rs`:dispatch 分支 + 测试。
- `registry/types.rs`:`GatewayMethodRegistration` + 测试。
- `registry/plugin_registry/mod.rs`:`gateway_methods` 字段及方法、`RegistryStats.gateway_methods`。
- `types/plugins.rs`:`PluginRecord.gateway_methods` 字段、构造默认值、`from_adapter_output`
  中针对 `GatewayMethod` 的 `CapabilityDeclaration` match 臂。

**Cli**

- `capability.rs`:`Cli` 变体、`CliDeclaration` 别名、各分支、测试条目。
- `registrar/api.rs`:dispatch 分支 + 测试。
- `registry/types.rs`:`CliRegistration` + 测试。
- `registry/plugin_registry/mod.rs`:`cli_commands` 字段及方法、`RegistryStats.cli_commands`。
- `types/plugins.rs`:`from_adapter_output` 中 `Cli` 的 match 臂。

**HttpRoute + HttpHandler**

- `capability.rs`:`HttpRoute` 变体、`HttpRouteDeclaration` 别名、各分支、测试条目。
- `registrar/api.rs`:dispatch 分支 + `make_http_route` 及 `test_p2_http_route_*` 等测试。
- `registry/types.rs`:`HttpRouteRegistration` / `HttpHandlerRegistration` + 测试。
- `registry/plugin_registry/mod.rs`:`http_routes` / `http_handlers` 字段及
  `register_*`/`get_*`/`list_*`、`clear()`、`unregister_plugin()` retain、`RegistryStats` 对应字段。
- 整文件删除:`http_handler.rs`(`match_path` / `PluginHttpHandler`)。
- `mod.rs`:`mod http_handler;` + `pub use http_handler::{match_path, PluginHttpHandler};`。
- manifest:`HttpRouteSection` 及 `http_routes` / `http_routes_v2` 字段
  (`toml_types.rs` / `types.rs` / `manifest/mod.rs`)。
- `types/plugins.rs`:`from_adapter_output` 中 `HttpRoute` 的 match 臂。

### 4.2 跨切面收尾

- **`capability.rs`**:5 个 P2/P3 能力删除后,`Tier::GatewayExtension` 变空 →
  移除该 `Tier` 变体及 `register_capability()` 中对应 match 臂;`Tier` 文档注释更新。
  `tier()` 函数的 P2/P3 分支相应收缩。
- **`manifest/types.rs`**:`PluginPermission::HttpRoutes` 与 `::GatewayRpc` 删除后无引用 →
  移除变体及其 `Display` impl 臂、字符串解析臂(`"http-routes"` / `"gateway-rpc"`);
  确认权限解析对未知字符串仍优雅降级(不 panic);为此补一条回归测试。
- **`registry/mod.rs` / `registry/types.rs`** 顶部文档注释中列举被删能力的行 → 更新。
- 删除所有针对被删能力的单测(`api.rs`、`registry/types.rs`、`capability.rs`、
  `channel_manager.rs`/`provider_adapter.rs` 随文件删除)。
- `capability.rs` 测试 `test_all_12_variants_have_kind_names` → 改名/改断言为剩余 7 个变体。

### 4.3 外部消费者复核(实现首步必做)

grep 已初步确认 Channel/Provider 等类型集中在 `src/extension/` 内。实现第一步须 `rg` 全仓复核:

- `packages/plugin-sdk/`、`bindings/`、`apps/`
- `examples/plugins/`(`media-video/aleph.plugin.toml` 仅 `[[tools]]`/`[[hooks]]`,初判安全)
- `src/gateway/handlers/plugins/`(插件 RPC handler 是否触及被删 registry 方法)
- `tests/`(集成测试)

若发现 `src/extension/` 之外的消费者,**暂停并向用户报告**,不擅自扩大改动面。

## 5. 延后工作(归档,不在本周期)

调研中确认的两块"看似缺连线、实为缺特性"的工作,留作独立周期,各需独立 brainstorm:

### 5.1 MCP server 运行时接线(专项)

- `src/mcp/manager/`(`McpManagerActor` + `McpManagerHandle`)是完全死代码 ——
  `McpManagerActor::new()` 仅测试调用,启动时从不 spawn。
- 活 API 是 `McpClient::start_external_server()`(`src/mcp/client.rs:211`),
  但启动时也从不被调用;配置文件声明的 `Config::mcp.external_servers` 不会在 boot 时拉起。
- `mcp.*` gateway handler(`src/gateway/handlers/mcp.rs`)是返回 "MCP Manager not initialized"
  的桩。
- 插件侧:`src/extension/loader.rs::load_mcp_plugin` 调 `read_mcp_json` → `loader.mcp_configs`
  (`all_mcp_configs_map()` 已就绪却无消费者);`load_all()`(`mod.rs:281-298`)把
  `CapabilityDeclaration::McpServer` 收进局部变量后丢弃。
- 专项须决策:复活 `McpManagerActor` vs 直接接线 `McpClient` 路径;`McpManagerConfig` →
  `ExternalServerConfig` 转换;工具注册经 `register_mcp_tools()`
  (`src/tools/handlers/registration.rs`);启动接线点约在
  `src/bin/aleph-server/commands/start/mod.rs` 的 `initialize_extension_manager` 之后。
- **本周期对 McpServer 不做任何改动**:`CapabilityDeclaration::McpServer` 变体、
  `load_all()` 的死 `mcp_configs` 收集、`mcp_config.rs`、`loader.rs` 的 MCP 路径全部保持原样,
  交由 MCP 专项一并处理。

### 5.2 插件消息通道(专项)

- 正确的插件通道需先有插件→宿主事件推送机制(新 WASM 宿主函数,或基于 MCP 长驻服务
  的通知路由),依赖 §5.1 先落地。本周期删除 `ChannelManager` 后,该特性从零正确设计。

## 6. 非目标(YAGNI)

- 不改动 6 个正常能力(Tool/Hook/Skill/Command/Agent/Service)与 McpServer。
- 不接线 MCP 运行时、不做插件通道(见 §5)。
- 不跑全项目 `cargo fmt`(main 非 rustfmt-clean,约 130 文件漂移 —— 见项目记忆)。
  仅对本周期改动的文件做局部 `cargo fmt`。
- 不顺手重构相邻无关代码、不动相邻注释格式。

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 删除波及 `src/extension/` 外 | 实现首步全仓 `rg` 复核(§4.3);发现外部消费者即暂停报告 |
| `PluginPermission` 变体删除破坏 manifest 解析 | 保留未知权限字符串的优雅降级;补回归测试 |
| `types/runtime.rs` 类型被 `src/extension/` 外引用 | 删除前对每个类型单独 `rg` 确认 |
| 基线已有失败测试 | 项目记忆载明 main 有 8 lib + 4 集成测试预存失败;只断言"无**新增**失败" |

## 8. 验证标准

- 每删一个能力区后 `cargo check -p alephcore` 通过。
- 全部删除后:`rg` 确认 5 类被删标识符在 `src/` 零残留;`cargo build` 链接通过。
- `cargo test -p alephcore --lib`:extension 相关测试全绿;对比基线无新增失败。
- 改动文件局部 `cargo fmt` + `cargo clippy`(仅改动文件)无新增 lint。

## 9. 工作流

1. 已建立 worktree `.worktrees/extension-consolidation`(分支 `extension-consolidation`)。
2. 本 spec 提交至该分支。
3. 进入 `writing-plans` 生成分阶段实现计划(逐能力删除 + 每步编译验证 + 跨切面收尾 + 全量验证)。
4. 实现期每能力一次提交;完成后代码审查。
5. 仅合并不在 worktree 会话内删除 worktree(见 CLAUDE.md worktree 注意事项)。

## 10. 预期净效果

- 移除约 1,500–2,000 行死/误设计代码(`channel_manager.rs` 701 + `provider_adapter.rs` 294
  + `http_handler.rs` + registry/manifest/types/capability 各处)。
- 插件能力枚举从 12 收敛为 7(Tool/Hook/Skill/Command/Agent/Service/McpServer),
  且 7 个全部"诚实"—— 要么完整接线,要么(McpServer)有明确归属的专项。
- 完全遵守 R10(薄 Harness / YAGNI 撤回)、R3(核心轻量化);零破坏性重构、零运行时风险。
- MCP 与插件通道的调研结论归档(§5),为后续专项节省重复探索。
