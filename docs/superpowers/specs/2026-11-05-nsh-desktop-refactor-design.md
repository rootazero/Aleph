# NSH Desktop 重构设计文档

- **状态**：草稿（待 builder-check 审阅）
- **日期**：2026-11-05
- **项目**：Aleph — Desktop 层 NSH 迁移
- **作者**：Kiro (Claude Opus 5.5) + 用户

---

## 一、意图与背景

### 1.1 我们要做什么

用 **NSH v0.4.0**（Longbridge，GPUI-based native shell 框架）替换 Aleph 当前的 **Tauri v2 + Leptos/WASM** 桌面栈。

目标架构：

```
┌─────────────────────────────────────────┐
│  desktop/nsh-shell  (NSH native host)   │
│  ├── run_app(AppSource::Rust(Root))     │
│  ├── tray / hotkey / deeplink / update  │
│  └── daemon supervisor                 │
├─────────────────────────────────────────┤
│  interfaces/nsh-desktop  (NSH UI crate) │
│  ├── session list / chat / composer     │
│  ├── settings / memory / canvas / ...   │
│  └── register_component() → Capability │
├─────────────────────────────────────────┤
│  shared/client + shared/protocol        │
│  (现有 Gateway 客户端 + DTO，保持不变)   │
└─────────────────────────────────────────┘
         ↕ JSON-RPC over local IPC
┌─────────────────────────────────────────┐
│        aleph-server (不变)              │
└─────────────────────────────────────────┘
```

### 1.2 为什么

| 现状痛点 | NSH 解法 |
|---------|---------|
| Tauri = WebView 壳，UI 在 WASM sandbox 里，与 Capability 系统隔一层 | `AppSource::Rust(Root)` — UI 是 Rust 组件树，直接对接 Capability |
| 构建链双轨（cargo + wasm-pack/trunk），调试路径长 | 纯 `cargo build`，单一工具链 |
| Plugin UI lifecycle 不可见，无法接入 Native Component Registry | `AppView::register_component()` — 正式的原生扩展点 |
| ADR-002 要求 Server = standalone；Tauri Command 是另一条 IPC 路径 | NSH app 直接复用 `shared/client` 的 WebSocket/IPC 连接 |

### 1.3 Aleph 设计原则对应

- **R1（大脑与四肢绝对分离）**：NSH app 不调 Server 内部 API，只走 `shared/client`
- **R2（UI 逻辑唯一源）**：`interfaces/nsh-desktop` 是唯一 UI crate，NSH shell 零业务逻辑
- **R6（一核多端）**：NSH 只是新的"端"，aleph-server 不变
- **Capability**：`register_component()` 是插件 UI 能力的正式注册点

---

## 二、目标、非目标、反目标

### 目标

1. 用 NSH `AppSource::Rust(Root)` 驱动 Aleph 桌面 UI
2. 保留现有的全部外部行为：tray、hotkey、deeplink、auto-update、daemon lifecycle、通知
3. `shared/client` 和 `shared/protocol` 不动，新旧客户端共用同一条 Gateway 连接路径
4. 插件 UI 的扩展点（`register_component()`）在第一阶段架构中预留，第二阶段实现
5. 迁移期间旧 Tauri 客户端继续可用，两套并行运行，直到 NSH 端验证完毕

### 非目标

- 不修改 `aleph-server` 任何业务逻辑
- 不在本次重构中引入新的持久化机制
- 不同时重写 Server 和 Desktop（ADR 明确禁止）
- JS 插件 UI（ADR-005 Restricted TSX + nsh-engine QuickJS）——第二阶段
- Windows 平台适配——与 macOS/Linux 并行，不阻塞主线

### 反目标（做了就算失败）

- 把 `aleph-server` 变成依赖 NSH 的代码（违 R1/R6）
- 把 NSH 特定类型泄露到 `shared/protocol` 或 `shared/client`（违跨进程契约原则）
- 把所有 Leptos 组件一次性全量替换（风险不可控）

---

## 三、约束

| 来源 | 约束 |
|------|------|
| ADR-001 | NSH v0.4.0 是目标版本；使用前必须对照源码核实 API |
| ADR-002 | Desktop 通过本地 IPC 连接 Server；Server 独立运行 |
| ADR-003 | Plugin Core 在 Server 侧；Plugin UI 在 Desktop NSH Runtime |
| ADR-004 | 第三方插件 WASM/进程隔离（本期只预留接口） |
| ADR-005/006 | JS 插件 UI Restricted TSX — 本期不实现，预留扩展点 |
| ADR-007 | 事件序号、Snapshot、Replay — 复用 `shared/protocol` 现有类型 |
| R1/R2/R6/R10 | NSH shell 零业务逻辑；UI crate 不调 Server 内部；保持 harness 轻 |
| 编码规则 | 每个阶段保持可编译、可测试；不为目录结构做无收益搬迁 |

---

## 四、现状盘点

### 4.1 将被替换的组件

| crate | 行数 | 技术 | 替换方式 |
|-------|------|------|---------|
| `desktop/shell` | ~2,000 | Tauri v2 | 新建 `desktop/nsh-shell` 并行，最后删除 |
| `interfaces/webchat` | ~157,000 (429 文件) | Leptos/WASM | 新建 `interfaces/nsh-desktop`，分阶段移植，最后删除 |

### 4.2 保持不变的组件

| crate | 说明 |
|-------|------|
| `shared/protocol` | 所有 DTO、事件类型，两套 Desktop 共用 |
| `shared/client` | Gateway 连接、credential store，两套 Desktop 共用 |
| `desktop/macos` | macOS 原生桥（Swift bridge），NSH 可通过 NativeComponent 集成 |
| `desktop/shared` | 截图/感知等原生能力，迁移后继续复用 |
| `aleph-server` 及所有 `src/` | 完全不动 |

### 4.3 NSH v0.4.0 关键 API（已核实）

核实来源：本地 clone `/Volumes/TBU4/Github/nsh`，workspace version `0.4.0`。

| API | 源码位置 | 状态 |
|-----|---------|------|
| `pub enum AppSource { Rust(Root), Bundle(Bundle), DevServer(String) }` | `crates/nsh-platform-macos/src/app.rs:105` | ✅ 存在 |
| `pub fn run_app(source: AppSource, options: AppOptions)` | `crates/nsh-platform-macos/src/app.rs:668` | ✅ 存在 |
| `AppView::register_component(name, factory)` | `crates/nsh-platform-macos/src/app.rs:379` | ✅ 存在 |
| `pub struct AppOptions { components: Components, .. }` | `crates/nsh-platform-macos/src/app.rs:118` | ✅ 存在 |
| `global::<T>()` | `examples/gallery-rs/native/src/shell.rs` 使用 | ✅ 存在 |
| `#[component]` 宏 | `nsh-macros` crate，gallery-rs 示例使用 | ✅ 存在 |

Phase 0 时以 path 依赖锁定该本地 clone 的固定 commit，记入 CLAUDE.md。

---

## 五、目标架构

### 5.1 Crate 边界

```
Aleph Workspace
├── desktop/
│   ├── nsh-shell/          ← 新建（替换 desktop/shell）
│   │   纯 OS 集成：窗口、tray、hotkey、deeplink、
│   │   auto-update、daemon supervisor。
│   │   零业务逻辑（R2/R10）。
│   │   依赖：nsh-platform-{macos,linux,windows}
│   │         shared/client（daemon probe + 连接目标）
│   │         shared/protocol（握手类型）
│   │
│   ├── shell/              ← 暂时保留（并行运行期）
│   ├── macos/              ← 保留（Swift bridge）
│   ├── linux/              ← 保留
│   ├── windows/            ← 保留
│   └── shared/             ← 保留
│
├── interfaces/
│   ├── nsh-desktop/        ← 新建（替换 interfaces/webchat）
│   │   NSH Rust 组件 UI：
│   │   ├── app.rs          根组件，路由
│   │   ├── platform/
│   │   │   ├── wide/       宽屏布局（对应现有 wide/）
│   │   │   └── phone/      手机布局（对应现有 phone/）
│   │   ├── views/          各页面（chat, settings, memory, ...）
│   │   ├── components/     共用组件
│   │   ├── state/          全局状态（对应现有 context.rs）
│   │   ├── api/            Gateway 事件订阅和 RPC 封装
│   │   └── plugin/         Native Component 注册（预留）
│   │
│   └── webchat/            ← 暂时保留（并行运行期）
│
└── shared/
    ├── protocol/           ← 不变
    └── client/             ← 不变
```

### 5.2 进程模型（不变）

```
┌──────────────────┐      local IPC      ┌─────────────────┐
│  aleph-desktop   │ ←─── WebSocket ───→ │  aleph-server   │
│  (NSH app)       │   (shared/client)   │  (不变)         │
└──────────────────┘                     └─────────────────┘
        ↑
   OS 原生层
  tray/hotkey/notify
```

### 5.3 Plugin UI 扩展点（本期预留，第二阶段实现）

```rust
// Phase 1 预留接口（desktop/nsh-shell/src/components.rs）
pub fn register_plugin_components(view: &AppView, plugins: &[LoadedPlugin]) {
    for plugin in plugins {
        if let Some(component) = plugin.native_component() {
            view.register_component(&component.name, component.factory);
        }
    }
}
```

### 5.4 Swift bridge 与原生能力（独立任务，不在 Phase 2 范围）

`desktop/macos` Swift bridge 承载的截图、语音、摄像头等原生能力，与 NSH NativeComponent 的集成路径**不在本期 Phase 2 范围内**，标为已知限制，后续独立立项。

Phase 2 验收只覆盖：窗口、tray、hotkey、deeplink、auto-update、daemon supervisor、OS 通知。

---

## 六、迁移阶段计划

### Phase 0 — NSH Compatibility Spike（1 周）

**目标**：验证 NSH v0.4.0 能在 Aleph 工具链下构建，`AppSource::Rust(Root)` 跑通，`shared/client` 能在 NSH 的 tokio runtime 中连接 Gateway。

**交付物**：
- `desktop/nsh-shell/` crate 骨架（不加入 workspace.members，spike 用独立验证）
- 一个最小 `Hello World` NSH 窗口，显示从 Gateway 取到的 server version
- 确认 NSH 的 tokio 版本与 Aleph workspace 兼容
- 记录所有 API 与文档不符之处

**成功标准**：`cargo run -p aleph-nsh-spike` 在 macOS 上打开窗口，显示 `aleph-server v{version}`。

**注意**：NSH 是独立 workspace（`crates/gpui` 在 exclude 中），集成时以 path 依赖引用本地 clone，不发布到 crates.io。

---

### Phase 1 — Protocol 传输层（2 周）

**目标**：建立 `desktop/nsh-shell` → `shared/client` → `aleph-server` 的完整连接链路，包括 `system.hello` 握手、事件订阅、Session/Agent RPC。

**交付物**：
- `desktop/nsh-shell` 加入 workspace，依赖 `shared/client` + `shared/protocol`
- Supervisor：发现并启动本地 `aleph-server`，健康检查，指数退避重连
- `system.hello` 握手（`client_name: "aleph-nsh-desktop"`）
- 事件流订阅（`shared/protocol::events`）
- Session 列表 RPC

**这让以下事情变难**：NSH 的事件循环是主线程 AppKit，tokio runtime 需要 `tokio::runtime::Builder::new_multi_thread().build()` 在后台线程运行，两者通过 channel 通信。这是固定成本，必须在 Phase 0 Spike 中验证。

---

### Phase 2 — NSH Shell 原生功能（2–3 周）

**目标**：实现现有 `desktop/shell` 的全部 OS 集成功能，用 NSH + 平台 API 替代 Tauri 插件。

| 功能 | Tauri 现有实现 | NSH 替代方案 |
|------|-------------|-------------|
| 系统托盘 | `tauri-plugin-tray` | macOS: `objc2-app-kit::NSStatusItem` |
| 全局热键 | `tauri-plugin-global-shortcut` | macOS: `CGEventTap` / `AddEventMonitor` |
| Deeplink `aleph://` | `tauri-plugin-deep-link` | macOS: `NSAppleEventManager` |
| 自动更新 | `tauri-plugin-updater` | `desktop/nsh-shell/src/update.rs`（复用现有逻辑） |
| OS 通知 | `tauri-plugin-notification` | `nsh_platform_macos::notifications` |
| 窗口状态持久化 | `tauri-plugin-window-state` | `AppOptions.size` + 手动持久化 |
| Daemon supervisor | `desktop/shell/src/daemon.rs` | 直接 port，无 Tauri 依赖 |
| 单实例 | `tauri-plugin-single-instance` | macOS: `NSRunningApplication` |

**注意**：tray、hotkey、deeplink 在 NSH 中没有内置实现，需要直接调用 `objc2`。NSH 已经在 `nsh-platform-macos` 中依赖 `objc2`，可以复用其 `objc2-app-kit` 版本（需锁定相同版本避免冲突）。

**决策（owner 确认）**：完全替换路径，Tauri 在 Phase 6 彻底删除。Phase 2 必须自建上述全部 OS 集成，无 Tauri 兜底。

**范围排除（owner 确认）**：截图/语音/摄像头等 `desktop/macos` Swift bridge 承载的原生能力不在 Phase 2 范围，独立立项。Phase 2 验收只覆盖窗口/tray/hotkey/deeplink/update/daemon/notify。

**交付物**：
- `desktop/nsh-shell/src/` 实现上表所有功能
- QA checklist：对照 `desktop/shell` 的现有行为逐项验证（仅限上表范围）

---

### Phase 3 — 核心 Chat UI（3–4 周）

**目标**：在 `interfaces/nsh-desktop` 中用 NSH Rust 组件实现核心聊天界面，达到可日常使用的程度。旧 `interfaces/webchat` 保持不动。

**优先迁移的视图**（按重要性排序）：

1. **Session 列表侧边栏** (`chat_sidebar.rs` → `views/session_list.rs`)
2. **Chat 消息流** (`views/chat/messages.rs` → `views/chat/messages.rs`)
3. **输入 Composer** (`views/chat/composer/` → `views/chat/composer/`)
4. **连接状态 / 握手错误页** (`components/connection_status.rs`)
5. **Settings 基础页** (providers, appearance)

**状态模型**：

```rust
// interfaces/nsh-desktop/src/state/mod.rs
// 对应现有 interfaces/webchat/src/context.rs（3413 行）
// NSH 用 global::<T>() 替代 Leptos provide_context

global::<SessionState>()     // 会话列表 + 当前会话
global::<ConnectionState>()  // Gateway 连接状态
global::<AppearanceState>()  // 主题 / 布局
```

**这让以下事情变难**：现有 `context.rs` 是 3413 行的 Leptos reactive 上下文。NSH 的信号模型与 Leptos 相似但不同，需要逐函数理解语义后重写，不能机械翻译。建议先写 state 层测试（不依赖渲染）再写组件。

---

### Phase 4 — 完整 UI 迁移（6–8 周）

**目标**：移植全部视图，使 `interfaces/nsh-desktop` 达到与 `interfaces/webchat` 的功能对等。

**视图清单**（按模块）：

| 模块 | 现有文件估算行数 | 优先级 |
|------|----------------|--------|
| Chat（messages/events/state） | ~8,000 | P0（Phase 3 已开始） |
| Canvas（画布/画廊） | ~6,000 | P1 |
| Settings（channels/providers/memory） | ~5,000 | P1 |
| Browser（内置浏览器） | ~2,700 | P2 |
| Panel（插件面板系统） | ~3,000 | P2（与 Phase 5 联动） |
| Phone/Tablet 布局 | ~2,000 | P2 |
| 其余组件 | ~20,000+ | P3 |

**验收标准**：两套 Desktop 并行运行，功能对等，通过现有 QA checklist。i18n 不作为 Phase 4 验收条件（独立任务）。

---

### Phase 5 — Plugin Runtime（独立时间线）

**目标**：实现 ADR-003/005 的插件 UI 体系。

**交付物**：
- `nsh-engine` QuickJS 集成（Restricted TSX sandbox）
- `register_component()` 对接 Capability 系统（`src/capability/`）
- Plugin UI lifecycle：owner/scope/资源配额/到期清理
- `aleph-ui-bridge`：Plugin UI → Server RPC 的受控通道

**前提**：ADR-006（WASM Runtime 选型）和 ADR-008（插件信任/签名）需要先得到 Accepted 状态。

---

### Phase 6 — 移除 Tauri（Phase 4 完成后）

1. 双端并行运行 2 周，QA 确认无回归
2. 从 `Cargo.toml` workspace.members 移除 `desktop/shell` 和 `interfaces/webchat`
3. 删除两个 crate
4. 更新 FEATURE_LOCATOR.md、CLAUDE.md、所有相关 reference 文档

---

## 七、关键风险与缓解

| 风险 | 可能性 | 缓解措施 |
|------|--------|---------|
| NSH objc2 版本与 `desktop/macos` Swift bridge 的 objc2 冲突 | 高 | Phase 0 Spike 第一天验证；锁定版本 |
| NSH tokio runtime 与 Aleph workspace 版本不兼容 | 中 | Phase 0 Spike 验证；NSH 用 path 依赖 |
| `interfaces/webchat/src/context.rs`（3413 行）迁移工作量超估 | 高 | Phase 3 先做 state 层 + 测试，不绑定渲染；Phase 4 分批 |
| NSH tray/hotkey/deeplink 无内置支持，objc2 直接调用维护成本高 | 中 | 完全替换路径已定（owner 确认），无 Tauri 兜底；Phase 2 直接用 `objc2-app-kit` 实现 |
| NSH API 变动（v0.4.0 仍在活跃开发） | 中 | 锁定 path 依赖到本地 clone 的固定 commit；不跟随 head |

---

## 八、构建者须知

### 8.1 你需要决定的事（构建者可自行裁定）

- NSH 路径依赖的 commit hash（锁定后记入 CLAUDE.md）
- `interfaces/nsh-desktop` 内部目录结构的具体命名
- `state/mod.rs` 中全局状态的切分粒度（一个大 struct vs 多个小 global）
- tray/hotkey 的 objc2 调用是放在 `desktop/nsh-shell` 还是 `desktop/macos`

### 8.2 已决策事项（owner 确认）

| 问题 | 决策 | 影响 |
|------|------|------|
| Phase 0 Spike 失败的 Plan B | **(d) 失败时开新会话讨论**，不预设 fallback | Phase 0 是 go/no-go 关口，失败即暂停整个迁移 |
| Tauri 混合路径 vs 完全替换 | **完全替换**：Phase 2 用 `objc2-app-kit` 直接实现 tray/hotkey/deeplink，Phase 6 彻底删除 Tauri | Phase 2 工作量较大；Phase 6 可实现零 Tauri 依赖 |
| i18n 迁移策略 | **独立任务**，不作为 Phase 4 验收条件，不阻塞 Phase 6 | NSH CoreText 渲染与 Leptos DOM i18n 机制根本不同，单独立项边界更清晰（构建者裁定） |
| Swift bridge ↔ NSH NativeComponent 集成时机 | **不在 Phase 2 范围**，标为已知限制，后续独立立项 | Phase 2 验收只覆盖窗口/tray/hotkey/deeplink/daemon；截图/语音/摄像头等原生能力另立 Phase |

**仍需 owner 决策的事项**：ADR-006/008 的 Accepted 决策（影响 Phase 5 能否启动）。

### 8.3 构建者须知——未覆盖事项

- NSH tray/hotkey/deeplink 的 `objc2-app-kit` 具体调用实现（Phase 2 时设计）
- `interfaces/nsh-desktop/src/api/` 内部抽象粒度（Phase 3 时决定）
- `state/mod.rs` 中全局状态的切分粒度（Phase 3 实测后调整）
- tray/hotkey objc2 调用归属：`desktop/nsh-shell` vs `desktop/macos`（Phase 2 前根据 Swift bridge 复用需求决定）
- **i18n 迁移**（独立任务，不在本期范围）：`i18n_census.rs`（1680 行），NSH CoreText 路径的中文字面量机制需单独设计
- **desktop/macos Swift bridge ↔ NSH NativeComponent 集成**（独立任务）：截图、语音、摄像头等原生能力的绑定路径
- ADR-006/008 的 Accepted 决策（影响 Phase 5 能否启动，owner 决定）
- Windows 平台适配（与 macOS/Linux 并行，不阻塞主线）

---

## 九、FEATURE_LOCATOR 更新指针

完成各阶段后需同步更新以下条目：

- `§7.5 Tauri 桌面壳` → 替换为 `§7.5 NSH 桌面壳 (desktop/nsh-shell)`
- 新增 `§7.6 NSH Desktop UI (interfaces/nsh-desktop)`
- `§7.x Plugin Native Component Registry` (Phase 5 后新增)
- ADR 状态：ADR-001/002 已是 Accepted，无需变更。ADR-003/004 在 Phase 2 前需确认 Accepted；ADR-005/006/007/008/009 在 Phase 5 前需确认 Accepted
