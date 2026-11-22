# NSH 可定制 Panel 增量重构设计 / Modular Panel migration design

- 日期 / Date: 2026-11-22
- 状态 / Status: **Builder-reviewed and revised — awaiting user approval of this document; no implementation authorized yet**.
- 工作分支 / Branch: `nsh-panel-refactor`; baseline `22e7c7b22656d9231583f9d5a629a35f3d6583a4`.
- 范围 / Scope: NSH Panel 架构、能力连线、模块定制、迁移验证与文档同步；不是重新设计 Rust Core。

## 1. 目的与既有决策 / Goal and settled decisions

将 Panel 从「宿主打开现有网页」迁移为 Rust Core 的可靠、可扩展入口：共享 TSX 页面在 Desktop / Web 两端运行，Desktop 使用 NSH Native Components 做增强，Web 有明确降级实现。默认发行版开箱可用，用户可按契约配置、添加、禁用和替换模块，而不是得到一个功能写死的 TSX 应用。

**English:** Build a customizable, shared Desktop/Web Panel, not another fixed desktop wrapper. Preserve one Core authority and reuse existing capability contracts.

依据以下既有设计，不另起一套相互竞争的架构：

1. [NSH ADR-001–009](../../research/NSH_ADR-001~009.md)：001–005 Accepted；006–009 Proposed，不因本设计自动升级为 Accepted。
2. [Protocol v1](../../research/NSH_PROTOCOL_V1.md) 与 [Accepted Web amendment](../../research/NSH_ADR-001_Amendment_Web_Target.md)。协议中的 RPC 名称示例映射现有业务方法，不机械新建同义端点。
3. [模块化方案](../../research/aleph_modularization_plan.md)：内核不换，叶子能力可换；默认内置与扩展通过相同契约装配；失败不能伪装成功。
4. [Capability 总纲](../../research/Everything-externally-composable-is-a-Capability.md)：只注册一次，多出口投影；可见性、授权与生命周期分开；收敛现有权威，而不是再加注册表。
5. 用户已选择采用新 NSH ADR 方向，允许修订 R2 的 Leptos/WASM 实现限定。独立可实施性审阅后，用户选择同源短期 Cookie、首条纵切只替换呈现、生成 UI 可独立延期；本文件仍需单独批准后才能实施。

## 2. 已核实的现状 / Verified baseline

以下是源码观察，不是完成声明：

| 范围 | 当前事实与可复用资产 | 迁移含义 |
|---|---|---|
| NSH 入口 | `desktop/nsh-shell/src/main.rs` 使用 `web_view`; `shell_host.rs` 生成现有 Panel URL | 目前不是 Mixed Runtime；保留服务身份、目标切换、凭据和平台适配，不复制其业务层 |
| 页面 | `interfaces/webchat` 使用 Leptos 0.8；`src/context.rs` 通过 WasmConnector 连接 WS | NativeIpc 凭据通路不等于 Core IPC；不得把网页套壳称为目标架构 |
| 模块壳 | `interfaces/webchat/src/panel/{mod,registry,projection,discovery,source,shell}.rs` | 已有模块契约和读侧注册机制，优先抽取可移植语义；Leptos 渲染代码需要适配，不是假装直接复用 |
| 模块 SDK / loader | `interfaces/webchat/src/panel/sdk.rs`, `loader.rs`, `loader/browser.rs` | 已有窄 ABI、订阅释放、异步导入提交前校验；迁移这些语义，而非新增无约束 bridge |
| 模块生产源 | `src/extension/panel.rs`, `src/extension/panel_publication/`, `src/gateway/handlers/panel_capabilities.rs` | 活跃 PluginRecord、信任策略和发布流程派生 descriptor；Panel 是投影，不接管插件事实源 |
| 跨端契约 | `shared/protocol/src/panel.rs`, `system_hello.rs`, `capability.rs`, `jsonrpc.rs` | 复用现有 crate 和 wire 类型；新字段/错误码统一定义，TS 类型由契约产生并验证 |
| 客户端 | `shared/client/src/connection.rs`, `shared/ui_logic/src/connection/` | 扩展 transport seam；Client 不依赖 Core；避免 NSH 再造完整客户端 |
| 会话与事件 | `src/session/store.rs::SessionEventStore`, 现有 Gateway `chat.*`、`sessions.*`、`events.subscribe` | 复用业务入口与持久事件，不创建新 EventLog / task authority |
| 本机平台 | 现有 updater、凭据、autostart、服务准入、target transaction | 保持 minisign / feed / app identity 等已批准语义，不趁 UI 重构换信任体系 |

`panel-modular` 当前是可选 feature，不在 `interfaces/webchat/Cargo.toml` 的 default 中。已有实现存在 ≠ 默认用户已使用，也不等于 NSH 新运行时已接线。

## 3. 架构与代价 / Architecture and trade-offs

```text
                       Rust Core / canonical owners
      sessions / authorization / extension records / capability ownership
                                  │
                 existing RPC handlers + capability projections
                                  │
           protocol + authenticated transport + client connection state
                  ┌───────────────┴─────────────────┐
          Desktop local IPC                   Web WebSocket
           UDS / Named Pipe                  authenticated WSS
                  │                                 │
            NSH host adapter                  browser adapter
                  └───────────────┬─────────────────┘
                         shared TSX Panel
               module shell / narrow SDK / local layout state
                  │                                 │
       NSH Native Components                  Web fallbacks
```

- UI 代码只有一份业务呈现源；不同端仅保留 transport、存储、窗口/平台能力与 Native Component 适配。
- Core 持有业务事实、权限与效果准入。Panel 可持有可丢弃投影、草稿、布局、选择状态，不持有第二套 Session / Tool / Plugin 权威。
- NSH JS VM 仅存在于客户端宿主。模块化方案禁止向 Aleph **Core** 嵌入 JS 扩展运行时的约束保持；不将 UI VM 变成工具/插件的 Core 执行后门。
- 代价：迁移期保留两种渲染器，需要兼容测试、布局数据迁移和逐页退出条件；Native/Web 的 IME、可访问性、虚拟列表等必须分别验证。
- 不采用「永远保留 Leptos，新增 TSX 复制版」：会制造双份 UI 业务源。不采用一次性删除旧客户端：现有跨平台与双轨验收尚未完成。

## 4. 可定制契约 / Customization contract

### 4.1 定制分层 / Layers

| 层 | 用户可做什么 | 不可跨越的边界 |
|---|---|---|
| 本地表现 | 排序、显示/隐藏、选择模块，保存可支持的布局/主题偏好，恢复默认 | 隐藏不是停用后端；布局不能授予权限或启动隐藏 provider |
| 模块贡献 | 安装受支持的贡献、按 qualified identity 加载，与内置模块使用同一 SDK | 插件清单不是授权；不把同源 JS 模块当沙箱 |
| 能力替换 | 经契约/版本/授权检查替换可替换叶子；显示实际来源、冲突、降级原因 | 不替换内核权限/审批/密钥，不覆盖保留命名空间，不静默降权后声称成功 |
| 发行版定制 | 默认内置集合可裁剪；Desktop 和 Web 对不支持的能力明确降级 | 不要求用户改 Rust Core 或重编译才能调整日常模块 |

布局最小集为模块显示、顺序、激活项与可用区域尺寸；任意嵌套 docking 并非第一切片前置条件。参考项目若提供更完整布局，只吸收有真实消费者的机制，不直接引入其整个状态框架。

### 4.2 一次注册，多端投影 / One registration, multiple projections

- 现有 `PluginRecord` / extension manager 仍是贡献来源；`panel_ui_capability_snapshot_for` 按 authenticated caller / project 可见性输出快照。
- 现有 `PanelTabDescriptor` / `PanelUiCapabilityDeclaration` 的 identity、UI API 版本、integrity、status、source revision 继续作为贡献边界。新增跨端 renderer 信息应扩展同一 wire 契约，不能在 TSX 再声明一份权威插件 schema。
- TSX 的 module catalog / tab registry 只是快照的读侧索引和挂载状态。不得有独立的 install、grant 或 owner authority。
- 动态可调用、可订阅、可替换或可管理的面必须经 Capability 契约表达。复用当前 kind / relation / owner / lease 模型；不为 UI 生造一个未经审批的新 kind，也不把 Tool-only facade 描述成所有 kind 已挂载。
- owner / generation / scope / integrity 变化时，先撤销旧 SDK 权限与监听，阻止旧异步结果提交，再释放视图；已被 Core 接纳的业务任务不因关页被取消。

### 4.3 SDK 与权限 / SDK and permissions

迁移现有 `PanelSdk` 概念：`invoke`、`subscribe`、连接状态、导航、主题、locale、dispose。模块只拿到自身 owner/scope 绑定的 SDK，不拿全局 DashboardState、原始 socket、凭据、任意 OS API。

- SDK 调用由 Core 按 caller、scope、method policy、lease 重新准入；前端 allowlist 不是安全边界。
- 默认内置模块也走同一能力入口；不能为内置页面开旁门，导致社区替代品永远缺功能。
- 订阅有界且可释放；断线/target switch/project switch/owner disposal 后丢弃旧代回调；需要快照恢复时明确标记 stale / resync。
- 模块禁用后没有属于该模块的活跃订阅、timer、View 或可再次调用的旧 SDK；只读 catalog 不得触发后端激活。
- 替换和失败语义沿用模块化方案：同层单基数冲突 fail-closed；安装/解析失败与调用失败分开；用户 `fallback=none` 不偷偷回到 builtin；实际 `served_by` 与失败原因可见。该原则不是自动实现整个 SlotResolver 的授权。
- **首条纵切替换呈现，不替换后端。** 内置与替代呈现各有 qualified ID。用户偏好将页面位置绑定到当前快照内已可见、可加载且满足该呈现契约的贡献；这一绑定归 Panel 布局偏好，不是 Core 槽位绑定、不授予任何权限。实际 `chat.*` 仍由同一 Core 后端服务；测试同时断言渲染者变了、业务效果未改变。
- 保留 builtin namespace，不允许社区 descriptor 冒充内置 ID。后端能力替换单独由现有 Core owner / Capability 槽位及授权控制；以实际提供者变化、旧租约失效和可见回退验证，不把 UI 替换当作后端验收，也不把全量 SlotResolver 拉入首条纵切。
- 第一阶段用户扩展复用现有安装记录与受信任来源的发布流程；当前 community fixture 不能直接变成默认生态。新的远程分发/签名协议仍受 ADR009 待定边界约束，不因支持本地模块加载而获得发布授权。

### 4.4 布局持久化 / Layout persistence

布局只保存稳定模块 ID 与纯数据，不保存实现函数、lease、token 或模块代码。通过 host storage adapter 使用带版本且按端点/用户隔离的键；Desktop/Web 共用数据模型，允许不同的本地物理存储。

缺失/撤销模块显示可理解的 unavailable 状态或可恢复布局占位，不恢复执行旧代码；重装同 ID 仍重新验证 version/integrity/owner。损坏/旧版数据只重置表现层，不删除会话或插件。跨设备同步不在第一切片范围；如以后支持，必须走 Core 现有配置 authority，而非新增同步服务器。

## 5. Transport 与真实能力协商 / Transport and honest negotiation

1. 在 `shared/protocol` 定义/复用有界 length-prefix framing、hello negotiation 与错误集合；上限从实际 transport 配置派生，而非 handler 单独写常量。
2. Desktop 新 IPC endpoint：macOS/Linux private-dir UDS；Windows Named Pipe + DACL/SID 检查。OS 身份绑定与现有授权合并，先认证/hello 后业务；不把 loopback 当身份验证。
3. 复用 Gateway dispatch、method visibility、permission、project context 和 idempotency gates；IPC 不能绕开 WS 已有准入。endpoint lifecycle 归 Server，宿主不成为第二个 Gateway。
4. 客户端 transport 分离 framing/I/O 与请求关联、超时、连接 epoch、订阅；同一协议实现不需要 Client 依赖 Core。Desktop Core 流量不偷偷回退 WS；远程目标明确使用经授权的网络 transport。
5. Web v1 使用独立 `/v1/ws` + `aleph.protocol.v1` subprotocol + length-prefixed JSON；旧 `/ws` text JSON / `connect` 行为不变。按 endpoint 和握手显式区分 wire 模式，不能猜帧。旧路径的 Cookie bootstrap 已移除，不能把它当作新入口复用（`src/gateway/server/mod.rs:248`、`src/gateway/server/connection/mod.rs:451`）。
6. **浏览器身份入口：显式同源 HTTPS 凭据交换。** 新 v1 HTTP POST 在 Core 复用现有配对票据兑换／设备凭据验证（`src/gateway/handlers/connect.rs::resolve_connect_auth` 的校验语义，而非旧 `connect` 握手或无凭据 loopback 分支）。验证成功后签发短期、Secure、HttpOnly、SameSite=Strict、host-only Cookie，绑定原用户、设备与该 Gateway；不把它作为新的授权事实源。拒绝跨 Origin、缺省凭据、已过期、已撤销及主体不匹配；限制速率，不记录凭据，不把 token 放进 URL。不支持隐式 LAN 信任登录，也不允许浏览器脚本读回 Cookie。
7. `/v1/ws` 在 Upgrade 前校验同源 Origin、Cookie 有效期及仍有效的 Core 设备身份，且把该身份交给连接上下文。首个应用消息必须是 `system.hello`，只协商协议、不携带登录凭据、不重新选择身份；连接到期/撤销后不得继续准入，续连必须重新校验。确切 TTL、会话票据的复用实现与路由命名由计划确定，不放宽这里的身份绑定和失败语义。
8. 当前 `system.hello` 仅按 `Feature::is_known()` 过滤并回显 minor；应改为实际支持集合与版本交集。现有 `events.subscribe` 的 topics 通知不等于 Protocol v1 durable replay，不得宣告已支持。
9. 第一切片只协商经测试的特性。snapshot/replay/binary 若未达到 v1 语义则不宣告；早期页面用快照重取与明确 stale 状态，不伪造 cursor 或 exactly-once。完整迁移必须经过 §7 的恢复闸门。
10. 断线不自动重放写请求。业务 retry 仅复用现有 durable idempotency 入口，明确 uncertain outcome；观察回放不是效果重做。

**English:** A same-origin HTTPS exchange validates existing pairing credentials and issues a short-lived, HttpOnly session for the new Web endpoint. Authentication precedes Upgrade; `system.hello` is the first application frame. The legacy `/ws` handshake remains unchanged. IPC shares existing authorization and dispatch, and each transport advertises only capabilities it implements.

## 6. UI 与运行时边界 / UI and runtime boundaries

- 第一条端到端路径：默认模块壳 → session list → chat history → `chat.send` 流式结果 → `chat.abort`。映射现有 API，不添加另一套 `agent.run` / task store。
- 同一路径必须同时演示：默认模块、用户排序/隐藏/恢复、同一业务的替代呈现（§4.3）、禁用后的实际清理、Desktop/Web 相同 UI 源。
- Mixed Runtime 先做真实 Native Component + Web fallback 的最小组件验证，再逐页迁移。NSH 参考仓版本号与 crates.io 同为 0.6.0 不足以证明 API 相同，以锁定发布源码和实际构建为准。
- 现有同源可信模块的 DOM mount ABI 不能直接搬到 NSH。抽出 runtime-neutral identity/lifecycle/SDK，保留 browser mount adapter，并增加 NSH View adapter；格式不兼容时明确标记，不执行猜测性转换。
- Generated UI 必须是独立 owner-bound View，限 heap/stack/time/inflight/resources，停止标记可生效；无任意 host import、文件/网络、组件注册或权限管理。Desktop/Web 分别做隔离探针，只有通过的一端才可启用。未通过的一端明确 unavailable，可独立延期、不阻塞基础 Panel 与已审核模块迁移、不算 ADR-005 已交付，也不据此删除对应现有路径；不用主 UI VM 充当沙箱。
- 第三方 Core 插件仍使用已有 WASM/Process 路线；不加载 Rust dylib。ADR006–009 的 runtime 选型、通用 durable recovery、签名分发不在本设计中暗中定案。
- 已有凭据拒绝路径可能存在 native stored token 未清除问题；这是静态候选，先以真实实现测试确认。修复必须绑定来源、目标、attempt 与 lease，不能删除手工新 token 或另一代凭据。

## 7. 迁移切片与退出条件 / Migration slices and exit criteria

这不是只交付聊天 demo 后结束整个请求。实施计划应为所有现有入口列迁移台账，记录 production render、依赖能力、旧代码所有者、测试与退场条件。

| 切片 | 交付 | 退出条件 |
|---|---|---|
| S0 契约与文档 | 修订 R2；收敛 hello/framing；固定复用/迁移台账与工具链 | 契约测试证明不虚报特性；架构文档不把目标写成现状 |
| S1 transport | 共享 Client seam + local IPC + `/v1/ws` 浏览器认证 | Unix 实测 + Windows 独立平台实测；真实浏览器同源交换、hello、拒绝测试；旧 WS 回归 |
| S2 customizable spine | shared TSX shell、SDK adapter、稳定 ID 布局、模块挂载/撤销 | Desktop/Web 同源；配置重启生效；真实资源归零；旧句柄失效 |
| S3 业务纵切 | 会话/聊天/取消/重连 + 真实 Native Component/Web fallback | 实际 Core 效果到达；替代呈现不改变后端；IME/长列表/streaming 通过；先允许重取快照 |
| S4 恢复闸门 | 初次一致性快照 + 已授权 scope 的持久观察事件补读与 live 接续 | Desktop 重启、Web 断线重连在同一协议语义下还原会话/任务投影；缺历史返回 `resync_required`；不重做业务效果 |
| S5 剩余模块 | 按迁移台账迁移既有页面/贡献，连接可复用能力；清理已无消费者的原型 | 每个页面保留功能映射与默认挂载证明，模块禁用/替换验证通过；生成 UI 按 §6 独立登记状态 |
| S6 收口 | docs/reference、FEATURE_LOCATOR、测试证据、本地提交 | S4 必须通过才算完整迁移；旧 UI 源只在对应替代品验收后删除；保留仍需的客户端回退 |

**恢复范围 / Recovery boundary.** 以真实会话／任务视图为消费者，复用 `SessionEventStore::load_events_range`、现有 actor / 业务投影和授权规则；没有一致切点就明确是缺口，不把 actor broadcast 或 `events.subscribe` 当持久源。快照和 watermark 必须同一逻辑切点；回放按获授权的 `(scope, sequence)` 顺序，游标按已应用的连续前缀推进，接 live 不留竞态窗口；历史不可用或无法证明连续性时要求重取快照。验证在快照/订阅交界写入、断线与服务重启、可见性变化与历史耗尽，且不重复执行写请求。这是 Accepted ADR-002 / Web amendment 的观察恢复，不预选 Proposed ADR-007 的通用事件架构或承诺任意 Capability 的 durable replay。

任何阶段都不以 `panel-modular` feature 存在、prototype 方法存在或编译通过替代 production mount 证明。不得提前删除 Tauri：原三平台替代与真实共同 14 天双轨门槛仍适用。

## 8. 验证矩阵 / Acceptance and evidence

- **契约 / Contract:** framing partial/coalesced/oversize/invalid；hello major/minor/unsupported feature；Rust↔TS shape；错误码无冲突；不改现有 `IDEMPOTENCY_KEY_REQUIRED=-32030` 的含义。
- **权限 / Authorization:** wrong OS user / pipe ACL；真实浏览器凭据交换、同源升级、缺失/过期/撤销 Cookie 与跨 Origin 拒绝；hello 前业务拒绝；module scope 越界；project/target 切换旧回调拒绝；grant/hash/owner 变更失效；Core 与前端双向检验。
- **定制 / Customization:** 默认集合可用；排序/隐藏/恢复/重启持久化；添加/替代呈现/禁用；同层冲突；integrity 变化；缺失/损坏配置；local layout 不启动或替换后端。
- **生命周期 / Lifecycle:** dispose 后订阅/timer/View/handler 归零；慢异步 import 不复活；旧 SDK 调用拒绝；业务任务关页继续；重复 dispose / reconnect 不累积资源。
- **运行时 / Runtime:** Mixed Rust Native Component 与 Web fallback 实际渲染；IME 中文/日文组合输入、复制/撤销、焦点与键盘；窄宽屏、长会话、streaming；generated View 仅在该端预算/停止隔离证明通过后启用。
- **恢复 / Recovery:** Desktop 重启、Web 断线、并发写入、重复交付、不可见事件与历史耗尽；按 §7 验证 snapshot + replay 连续性及 `resync_required`，不重放业务效果。
- **连线 / End-to-end:** 独立 QA Core，真实 WS/IPC → handler → canonical session/extension source → projection → renderer；每份展示数据指出实际渲染代码，不制造成功 no-op。
- **回归 / Regression:** 修改前先跑受影响 baseline；每次 cargo 前运行 `python qa/browser_live/check_memory.py`；Panel 改动遵守现有 lib 测试和 production `just wasm` 要求；跨平台运行分开记录，不能以 macOS 代替 Windows/Linux。
- **证据 / Evidence:** commit/source hash、版本、端点、配置、owner/generation、实际观察与限制分别记录。构建/源码/真实平台测试不能互相冒充。

新增测试前先读取项目适用测试指南/skill。保持隔离命名空间、临时端口与服务；不触碰安装应用、生产凭据/登录项、正式服务或发布源。

## 9. 文档、删除与范围之外 / Documentation, deletion and exclusions

批准后同步修改根 `AGENTS.md` 与 `CLAUDE.md` 的 R2，保持一致：**复杂业务 UI 使用唯一共享呈现源；宿主/Bridge 仅做平台与协议适配；Core 保持唯一业务事实与授权权威。NSH 目标为共享 TSX + Native Components/Web fallback。** 同步 `docs/reference/REDLINES.md`、`DESKTOP_SHELL.md`、`FEATURE_LOCATOR.md` 与模块化方案中指向旧 Leptos 实现的说明，保留历史证据的时间边界。

清理必须有 replacement mapping 与无消费者证明：不因文件名含 spike/legacy 就删除；已接线模块与新旧过渡适配不是重复权威。废弃兼容代码按各切片完成情况删除，不永久保留双份业务 UI。

不做 / Not included without separate approval: main 原地修改；push/tag/release；生产 key/feed/credential migration；toolkit fork/静默升级；直接默认切换；Tauri 提前删除；万能 scheduler / external-effect exactly-once；另建业务数据库或插件权威；为定制性引入新 Core JS runtime；未批准的 ADR006–009 决策。

## 10. 参考项目核验 / Reference-source comparison

- NSH `5e3b0f78a`：`crates/nsh-cli/src/templates/mixed/lib.rs:1-23` 的 native module 注册，`crates/nsh-engine/src/script.rs:129-179` 的 stop flag，`crates/nsh-engine/src/host_channel.rs:374-407` 的 handler removal，提供运行时与释放参考，不是 Aleph 权限系统。
- noop `4e30a4d`：`app/nsh.json:18-20` 与 `app/host.js:108-125` 的 browser host，`app/App.tsx:337-361,458-466` 的 socket generation fencing 与 timer/socket disposal；其直接 filesystem 权限不能带进 generated UI。
### t3code — 布局而非插件内核 / Layout, not a plugin kernel

核验 revision `b42770897952`。`apps/web/src/rightPanelStore.ts:20-183` 使用闭集 panel kinds 与 per-thread surfaces；`:592-774` 迁移/清理历史状态；`:1381-1409` 的 `persist` / `partialize` 只持久化经过筛选的布局数据；`apps/web/src/components/chat/PanelLayoutControls.tsx:25-173` 是实际 toggle / maximize 入口。

借鉴 typed surface identity、打开/关闭/移动/激活操作、版本化迁移和「只保存用户意图」；不复制其闭集业务面板分派为 Aleph 扩展协议，也不为此引入 Zustand 或第二份会话 store。它不能证明动态第三方贡献、安全卸载或 Capability lease 已实现。

### deepseek-harness — 贡献与释放 / Contributions and disposal

核验 revision `fcc0ad462172`。`packages/extensions/ui-cordis/src/client/slots.ts:18-44` 定义 keyed、session-scoped 的 `tool.view.cordis` slot 及 owner props；`:47-88` 将 inventory / approval / run / stop / remove 作为 host seam。`packages/extensions/ui-cordis/src/client/dynamic-port.ts:8-29` 是窄 RPC port；`packages/extensions/cordis-host-runner/src/registry.ts:16-65` 的 activation 拥有 handlers / handlerDisposers / fiber，package version 不可变；`:152-238` 管理 run 与 pending approval identity。

借鉴贡献即受 owner 管理的 effect、注册返回 disposer、版本与 activation 分离、显式审批和失败可见；不复制其 process-local plugin registry / business state，不以运行 JS 的机制代替 Aleph Core 的授权。该实现并非通用 docking 或跨设备布局同步方案。

### Aleph reuse boundary / 当前状态限制

`interfaces/webchat/src/panel/source.rs:7-95` 区分内置 terminal source 与 `panel-tab-spike` community fixture；只有在相应模块壳 feature 启用后才能讨论其挂载，不能称为整个产品默认已交付社区插件。`loader.rs:35-169` 的 MountedTab/SDK/cleanup 与 generation 适合作迁移基础，但 UI cleanup 不等于 Core capability revocation。

三个参考机制分别承担 layout、contribution lifecycle、native runtime 的职责；由 Aleph 现有 descriptor / ownership / permission 契约组合，绝不把三个参考仓的 registry 层叠进 Core。

当前检出引用仓仅用于研究；项目文档不依赖这些仓的机器绝对路径。以上为 repo-relative source paths 与 revision；版本兼容性仍以锁定制品和实际构建验证。
