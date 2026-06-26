# 设计:Unreal Engine MCP 预设 + `post_install` 预设能力

- **日期**: 2026-06-27
- **分支**: `worktree-feat-unreal-engine-mcp-preset`
- **参考**: `hermes-agent/optional-mcps/unreal-engine/manifest.yaml`
- **状态**: 待复核

## 1. 背景与前提澄清

任务初始表述为「对 Aleph 的**虚幻引擎 MCP 协议**进行深度架构重构、错误修复、功能连线」。扫描后确认这是一个**前提错误**:

1. **Aleph 不存在任何 UE MCP**。全仓 `grep -i unreal` → 0 命中。没有可重构 / 修 bug / 连线的模块。
2. **参考侧 hermes 也不是协议实现**,只是一条 21 行的声明式目录条目 `manifest.yaml`(`name / description / transport(http 127.0.0.1:8000/mcp) / auth(none) / post_install`)。hermes 零行 UE 专属代码 —— 它纯靠**通用 MCP client** 连接 Epic 官方「Unreal MCP」插件,该插件把 MCP server 内嵌进正在运行的 Unreal Editor 进程,经 localhost HTTP 暴露。
3. **Aleph 的通用 MCP 栈本就强于 hermes**(`src/mcp/` 全栈:client / manager / http+sse+stdio transport / tool_bridge / auth / preflight)。

因此真实任务 = **把 hermes 的 UE「能力」映射到 Aleph 的预设目录**,而非重构。经逐项验证,二者唯一的能力差距是预设缺 `post_install` 字段。

### 1.1 精确 Gap Analysis

| 维度 | hermes(参考) | Aleph(现状) | 判定 |
|---|---|---|---|
| UE 集成形态 | 1 个 `manifest.yaml`,零代码 | 不存在 | 净新增一条目录数据 |
| 连接机制 | 通用 MCP client → 编辑器内置 http server | `src/mcp/` 全栈,能力 ≥ hermes | 直接复用,无差距 |
| 预设字段 | 含 `post_install` 安装后指引 | `McpPreset` 缺 `post_install` | **唯一真实缺口** |
| 安装期工具裁剪 | `hermes mcp configure` 探测+勾选 | `McpManagerConfig.tool_filter`(allow/deny,deny 优先,startup 应用)已存在 | 已连线,勿造第二套 |
| 可达性建模 | auth:none(localhost) | `Reachability` 不投影到 Hub、全仓无 `match` | 复用 `cn-native`,勿加变体 |

## 2. 目标与非目标

### 目标
- G1. 新增 `unreal-engine` 预设条目,用户可在 Aleph Hub 一键启用,连到编辑器内置 server。
- G2. 给预设模型补 `post_install` 能力(惠及**所有**预设),并在披露预览 + 安装成功两处向用户露出。

### 非目标(含理由,防熵增)
- N1. **不做安装期工具探测/勾选 UI** —— `tool_filter` 已覆盖,工具连接后由 `tool_bridge` 动态发现;再加一层违 R7「多层 Tool Filter」且增熵。用户如需裁剪 UE 工具,走既有 per-server `tool_filter`。
- N2. **不加 `Reachability::Local` 变体** —— `reachability` 不被 `map_entry` 投影到 Hub `ExtensionEntry`、全仓无 `match` 消费;新增即死变体(违 YAGNI)。
- N3. **不做原生 UE bridge,也不让 Aleph 自起 UE server** —— 编辑器已内嵌 server(违 R1/R3)。

## 3. 设计

### 3.1 改动 A —— 预设模型补 `post_install`(通用能力)

**数据模型**(`src/mcp/presets/mod.rs`):`McpPreset` 新增
```rust
/// 安装后展示给用户的中文设置指引(本机/外部依赖类预设用)。None = 无额外步骤。
#[serde(default, skip_serializing_if = "Option::is_none")]
pub post_install: Option<String>,
```
- `#[serde(default)]` 保证旧 `catalog.json` 条目(无该字段)继续反序列化。

**Hub 投影**(`src/hub/types.rs` + `src/hub/official_mcp.rs`):
- `ExtensionEntry` 新增同名 `pub post_install: Option<String>`(同 serde 属性,保持 wire 向后兼容)。
- `map_entry` 透传 `post_install: p.post_install.clone()`。

**Gateway 露出**(`src/gateway/handlers/extensions/install.rs`):
- `handle_install` 成功响应 JSON 增 `"post_install": entry.post_install`。
- `handle_disclosure` 预览响应增 `"post_install": entry.post_install`(方案 B —— 让用户**点安装前**就看到需先在编辑器起 server)。

**Panel 渲染**(Leptos/WASM,R2:UI 唯一源):
- 在扩展披露/安装成功视图,若 `post_install` 非空,渲染为一条信息提示(notice)。这是该字段的**真实消费者**,非投机抽象。
- 具体组件由 writing-plans 阶段定位。

### 3.2 改动 B —— `unreal-engine` catalog 条目

`src/mcp/presets/catalog.json` 追加:
```json
{
  "id": "unreal-engine",
  "name": "虚幻引擎 (Unreal Engine)",
  "category": "developer",
  "description": "驱动正在运行的 Unreal Editor 5.8+:生成 Actor、配置光照、材质实例、运行自动化测试等。",
  "vendor": "Epic Games",
  "official": true,
  "reachability": "cn-native",
  "transports": [
    { "kind": "http", "url": "http://127.0.0.1:8000/mcp" }
  ],
  "required_env": [],
  "tags": ["game-engine", "unreal", "developer", "local"],
  "post_install": "<见 3.3>"
}
```

**`reachability` 取 `cn-native` 的理由**:localhost server 永远可达(不受 GFW 影响),`cn-native` 语义=「完全可达」为真;且该字段不投影到 Hub、不面向用户,取值实际不可见,选最不误导的真值即可。

### 3.3 `post_install` 文案(中文,面向用户)

> 连接的是 Epic 官方「Unreal MCP」插件,它运行在 Unreal Editor 进程内。⚠️ **安装前请先在编辑器里把 server 跑起来**(见 §4.1:安装会立即握手,server 未启动则安装报「连接失败」):
> 1. 用 Unreal Editor 5.8+ 打开你的项目。
> 2. Edit → Plugins 搜索「Unreal MCP」,启用并重启编辑器(依赖的 Toolset Registry 会自动启用)。
> 3. Edit → Editor Preferences → General → Model Context Protocol,打开「Auto Start Server」(或在编辑器控制台运行 `ModelContextProtocol.StartServer`),默认监听 `http://127.0.0.1:8000/mcp`。
> 4. 确认编辑器内 server 已在监听后,再回到 Aleph 点安装/启用 —— 这样才能探测到工具。
>
> 注意:Epic 标记此功能为**实验性**;工具调用在引擎 game thread **串行执行**,避免并发下发。若你改过端口/路径,请相应修改该 server 的 URL。

## 4. 数据流(安装路径,验证无误)

```
catalog.json (unreal-engine)
  → presets::catalog()                       [反序列化,含 post_install]
  → official_mcp::map_entry                   [keyless http → is_projectable=true]
      → InstallSpec::McpRemote{StreamableHttp}, requires_config=false, post_install 透传
  → hub::primer 写入 aleph-hub slot
  → extensions.disclosure                     [响应带 post_install 预览]  ← 方案 B
  → extensions.install → run_install
      → mcp.add_server(McpManagerConfig::http(...).with_auto_start(true))
      → InstallOutcome::Mcp{id}
  → verify_install                            [start_server + list_servers → tool_count]
  → 响应 { ok, outcome, verify, post_install, ... }
  → Panel 渲染 post_install notice
```

关键不变量:`http://127.0.0.1:8000/mcp` 无 `<ENV_KEY>` 占位 → `is_projectable` 通过 → 投影为 `McpRemote`;`required_env` 空 → `requires_config=false` → 无密钥收集步骤,一键安装。

### 4.1 已验证的实现事实:安装是「急切握手」(决定指引顺序)

读源码确认(非假设):
- `run_install`(`hub/install.rs:183`)→ `mcp.add_server(cfg)`,`map_err…?` 直接上抛。
- `add_server`(`actor.rs:464`)先 upsert+存盘配置(L470-476),再 `if auto_start { start_server_internal(&config).await?; }`(L479-480)—— 远程预设经 `mcp_config_from_spec` 恒 `auto_start=true`。
- `start_server_internal`(`actor.rs:692-716`)对 http 走 `client.start_remote_server(...).await?`。
- `start_remote_server`(`client.rs:473`)先 `preflight_remote_url(...).await?`(L496,HTTP 探测)再急切 `connect` 握手。

**结论**:编辑器内 server 未启动时,preflight/握手因连接被拒而失败 → 层层 `?` 上抛 → **`extensions.install` 返回错误**(配置已落盘,但 Panel 见到的是安装失败)。

**因此**:用户必须**先在编辑器启动 server,再安装**。这正是选**方案 B**(披露预览即露出 `post_install`)的承重理由 —— 让用户在点安装**之前**读到该前置条件。**刻意不改** `add_server` 的急切语义(改成容忍离线会波及所有 server,违 R3/P6 最小集);仅靠文案排序解决。该「急切连接失败即报错」对所有远程 MCP 一致,非 UE 特例。

## 5. 测试

- `presets::tests`:`catalog.json` 解析含 `unreal-engine`;其 transport 为 http、url=`http://127.0.0.1:8000/mcp`、`required_env` 空、`post_install` 非空。
- `official_mcp::tests`:`unreal-engine` 投影为 `aleph-hub:unreal-engine`、`InstallSpec::McpRemote`(`StreamableHttp`)、`requires_config=false`、`post_install` 透传非空。
- `install.rs::tests`:`handle_disclosure` / `handle_install` 响应含 `post_install`(可用纯函数级断言或现有 handler 测试风格)。
- 旧 `catalog.json` 条目(无 `post_install`)反序列化为 `None`(向后兼容回归)。

## 6. 熵减 / 清理

本次为**纯增量**:`McpPreset` / `ExtensionEntry` 确实缺该字段,新增不替换任何旧逻辑;改动 A 让 `post_install` 落地即有消费者(UE 条目 + Panel),不留悬空抽象。无死代码产生,无旧代码需删除。

## 7. 红线核对

- R1:不直接调平台 API,纯走 MCP IPC ✅
- R2:业务 UI(post_install 渲染)在 Panel,不在原生 bridge ✅
- R3/P6:最小集,拒绝 N1/N2/N3 投机扩展 ✅
- R7:不造第二套 tool filter ✅
- R8:UE 作为可对话安装的 Tool/扩展条目 ✅
