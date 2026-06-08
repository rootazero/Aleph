# Panel「Network」配置页设计 — 壳核分离连接切换 (A) + Aleph 集群 (B)

- **日期**: 2026-06-08
- **状态**: Design (approved, pre-plan)
- **范围**: 信息架构(IA) + Feature A 完整页面 + Feature B 骨架
- **改动面**: **纯 panel(Leptos WASM)新增代码** — 零 core 改动、零 shell 改动

---

## 1. 背景与目标

两个分布式相关功能需要在 panel 出配置入口:

- **Feature A — 壳核分离 / 连接切换**:在 panel(运行于桌面 Tauri shell)里切换连接「本地 core」还是「远程 core」。**已完成**(后端 + shell 命令齐备)。
- **Feature B — Aleph 集群**:center core 经反向 RPC 登记并管理 node core(执行臂)。**大部分完成**(`environments.list` / `cluster.enroll` 已在 main;`node_invoke`/bash 在 `feat/cluster-phase0c-core` worktree 收尾,未合 main)。

目标:在 panel 给这两个功能一个**合并的单页**配置入口,信息架构清晰、按环境/角色优雅降级,且不引入 core/shell 改动。

### 决策记录(brainstorm 结论)

1. **合并为一页**(而非两个独立页 / 顶栏切换器)。直接回答「能否合并」= 能,用「上游连接 + 下游集群 = 这台 panel 看到的整张拓扑」框定。
2. **新建顶级分组「Network」**,置于 `Advanced` 之后(最 infra/operator、低频)。
3. **B 骨架**:接线 main 已有的 `environments.list` + `cluster.enroll`,其余(invoke/bash/deregister/详情抽屉)留禁用占位,标注「0c 收尾后启用」。

---

## 2. 架构事实(亲读确认)

### Feature A 后端(已完成,不改)

| 项 | 位置 |
|---|---|
| `ConnectionTarget::Local \| Remote(Url)` | `desktop/shell/src/connection.rs:22` |
| `parse()`(`host` / `host:port` / `http(s)://…`;默认 http、默认端口 18790) | `connection.rs:38` |
| 持久化 `~/.aleph/.desktop-shell-target` | `connection.rs:129/141` |
| Tauri 命令 `get/set/clear_connection_target` | `connection.rs:159/167/180` |
| 命令已注册进 invoke_handler | `desktop/shell/src/main.rs:115` |
| `set_connection_target` → `reroute_for_target`(导航 webview;远程先做可达性探测再载入远端 origin) | `connection.rs:174` + `main.rs:261` |
| `withGlobalTauri: true`(WASM 可经 `window.__TAURI__.core.invoke` 调命令) | `desktop/shell/tauri.conf.json:10` |

关键属性:A 是**客户端连接目标**、需在**连接之前**可用、**仅桌面 shell 内有效**(纯浏览器 panel 无法 reroute 自己的宿主)。Apply 会**整页重载** panel。

### Feature B 后端(main 已有部分)

| RPC | 鉴权 | 入参 | 返回 | 位置 |
|---|---|---|---|---|
| `cluster.enroll` | **operator-only** | `{node_name}` | `{node_id, token, signature}` | `src/gateway/handlers/cluster.rs:22` |
| `environments.list` | **已认证可读** | `null` | `{environments: [...]}` | `src/gateway/handlers/cluster.rs:77` |
| `node_invoke` / bash 节点命令 | — | — | — | **未合 main**(`feat/cluster-phase0c-core` worktree) |

`Environment` 字段(`src/cluster/registry.rs:41`):`id` / `name` / `status`(="online")/ `commands: Vec<CommandDescriptor>` / `connected_at: i64`。`CommandDescriptor`:`name` + `schema`(JSON Schema)。

关键属性:B 是 **center 视角**、需**连接之后**、需 **operator 角色**;浏览器/桌面 panel 均可用。

### Panel 接线约定

- 现有 settings API client 模式(如 `ProvidersApi` / `MemoryApi`):`api/{feature}.rs`,静态 impl,`state.rpc_call(method, params) -> Result<Value, String>`,serde 解析。
- 设置路由在**顶层 router** `interfaces/webchat/src/app.rs`(如 `app.rs:389` `/settings/providers`)。`SettingsLayout` 已废弃为空壳;侧栏由统一 sidebar 渲染 `SETTINGS_GROUPS`。
- **现状:panel WASM 没有任何 Tauri-invoke 桥**,只用了 `data-tauri-drag-region` CSS 属性。→ A 需要**新建** WASM→Tauri invoke shim。

---

## 3. 信息架构

`interfaces/webchat/src/components/settings_sidebar.rs`:

1. `SettingsTab` 新增 `Network` 变体。
2. `path()`:`Self::Network => "/settings/network"`。
3. `i18n_label()`:`settings.tabs.network`(中文「网络」)。
4. `icon_svg()`:网络/拓扑图标(节点连线)。
5. `SETTINGS_GROUPS` 末尾新增分组(Advanced 之后):
   ```rust
   SettingsGroup { label: "Network", tabs: &[SettingsTab::Network] }
   ```
6. `SettingsGroup::i18n_label()` 加 `"Network" => settings.groups.network`。

`app.rs` 顶层路由表新增:`"/settings/network" => view! { <NetworkView /> }.into_any()`。

---

## 4. 页面:`NetworkView`(`/settings/network`)

单页,两个分区,纵向堆叠。H1「网络与集群」。

### Section 1 — 上游连接 (Feature A, 完整)

**挂载时探测** `is_shell()`(= `window.__TAURI__` 是否存在):

- **桌面 shell 内 → 可交互切换器**
  - Radio:`○ 本地 Local` / `● 远程 Remote`。
  - 选「远程」时:endpoint 文本输入框。占位 `https://core.example:18790`。客户端只做**预览归一**(补 http scheme、补端口 18790)用于展示,**服务端 `parse` 为权威**。
  - 当前目标行 + 连接状态点(复用 `DashboardState.is_connected` 的活 socket 状态)。
  - **Apply** 按钮 → 确认弹窗:「将切换到 `<target>` 并**重新加载 Panel**」→ 调 `invoke("set_connection_target", {raw})` → webview 自行 reroute(panel 重载,本视图随之销毁)。
  - **重置为本地** 按钮 → `invoke("clear_connection_target")`。
- **纯浏览器内 → 只读**
  - 显示当前 origin + 提示:「连接切换仅在桌面 App 内可用」。不渲染切换控件。

**非目标**:不收集远程 auth token。远程鉴权由**远端 core 自己的 bootstrap/pair UX** 在 webview 载入远端后处理(与现实现一致,token 仅 env `ALEPH_GATEWAY_TOKEN`)。

### Section 2 — 下游集群 (Feature B, 骨架)

**角色门**:operator 才有意义。非 operator → 灰示占位「集群管理需要 operator 权限」。
(实现:优先读现有 session/auth 角色信号;若无则乐观发 `environments.list`,遇 auth 错误降级为占位。)

- **现在接线(main 已有)**
  - **节点列表**(读):`environments.list` → 表格列:`name` / 短 `id` / 状态点(online)/ `connected_at`(相对时间)/ 命令数。空态:「暂无已登记节点」。
  - **Enroll**:弹窗输入 `node_name` → `cluster.enroll` → 返回 `{node_id, token, signature}` → copy-box 展示 token + 说明「在目标机器上用此 token 加入」。
- **骨架占位(0c 合 main 后填充)**
  - 节点详情抽屉:展示 `commands[]` 能力列表。
  - **Invoke / bash** 动作:**禁用按钮** +「0c 收尾后启用」。
  - **Deregister**:占位(后端暂未暴露)。

---

## 5. 新增 / 修改文件(panel-only)

**新增**:
```
interfaces/webchat/src/api/
  tauri_bridge.rs        # wasm-bindgen extern → window.__TAURI__.core.invoke
                         # pub fn is_shell() -> bool
                         # async get/set/clear_connection_target 包装(JsFuture)
  cluster.rs             # ClusterApi{ list_environments(), enroll_node(name) }
                         # 类型 Environment / EnrollResult
                         # invoke()/deregister() stub,注释指向 phase 0c
interfaces/webchat/src/views/settings/network/
  mod.rs                 # NetworkView 外壳 + 两 section 组合
  connection.rs          # Section 1 (A)
  cluster.rs             # Section 2 (B)
```

**修改**:
```
interfaces/webchat/src/views/settings/mod.rs       # pub mod network; pub use network::NetworkView;
interfaces/webchat/src/components/settings_sidebar.rs  # SettingsTab::Network + path + label + icon + 新分组
interfaces/webchat/src/app.rs                      # /settings/network 路由
interfaces/webchat/src/api.rs (或 api/mod.rs)       # pub mod tauri_bridge; pub mod cluster;
interfaces/webchat/src/i18n/*                       # settings.tabs.network / settings.groups.network (+ section 文案)
```

**不改**:`desktop/shell/*`(命令已注册)、`src/*` core(RPC 已存在)。

---

## 6. 关键实现注记

- **Tauri v2 invoke shim**:`withGlobalTauri: true` → 绑 `window.__TAURI__.core.invoke(cmd, args) -> Promise`。wasm-bindgen `#[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"])] fn invoke(...)`,配 `wasm-bindgen-futures::JsFuture` 取结果。确认 panel 依赖含 `wasm-bindgen` / `js-sys` / `wasm-bindgen-futures`(Leptos CSR 通常已有)。
- **`is_shell()`**:检测 `window.__TAURI__` 存在(`js_sys::Reflect::get`),决定 Section 1 渲染交互态还是只读态。
- **endpoint 归一**:客户端 helper 镜像 `ConnectionTarget::parse` 规则**仅用于预览/即时校验**;真正解析由 shell 命令完成,UI 信任其 `Result` 的 `Err(String)` 文案。
- **文件大小**:`network/` 从一开始就拆 `mod/connection/cluster` 三文件,符合 P2 高内聚与大文件拆分。

---

## 7. 测试与验证

- **wasm 单测**:
  - endpoint 归一/校验纯函数。
  - `Environment` / `EnrollResult` 的 JSON 解析(样例 JSON round-trip)。
- **手动 e2e**(WASM UI 大部分只能手动验证,遵循 CLAUDE.md 刷新链):
  - `just wasm` → `cargo build --release -p alephcore --bin aleph-server` → 替换运行中 binary → relaunch。
  - A:shell 内切换 local↔remote、Apply 重载、浏览器内只读降级。
  - B:列表读取(连/不连 node)、Enroll 出 token、非 operator 占位。
- **既有覆盖**:`connection.rs` 的 `parse` 已有 Rust 单测,不重复。

---

## 8. 非目标(YAGNI)

- 单一连接目标,不做 saved/recent history(`connection.rs` 本就只持久化一个)。
- 无远程 auth token UI(交由远端 pair/bootstrap)。
- 不接线 `node_invoke`/bash(0c 未合 main),仅占位。
- 无 deregister(后端未暴露)。
- 浏览器内不支持连接切换(架构性约束,非缺陷)。

---

## 9. 红线核对

- **R2 UI 唯一源**:复杂业务 UI 在 panel,shell 仅暴露 I/O 命令 — 守住。
- **R4 Interface 纯 I/O**:panel 只发 RPC / 调 Tauri 命令,不持久化、不规划 — 守住。
- **R6 一核多端**:本页正是「核与核」拓扑的可视化入口 — 强化。
- **P2 高内聚 / P6 简洁**:三文件拆分、合并页两分区、骨架 stub 不提前抽象 — 守住。
