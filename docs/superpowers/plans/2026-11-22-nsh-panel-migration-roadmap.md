# NSH 可定制 Panel 迁移路线图 / Migration Roadmap

**Approved design:** [NSH Panel 增量重构设计](../specs/2026-11-22-nsh-panel-modular-refactor-design.md) (`4f951d4d1`)。本路线图记录阶段依赖与退场门槛。已批准的工作包在实施计划落盘并自审后直接执行；未批准范围和发布操作不因此获批。

**Goal:** 在 Rust Core 仍为唯一事实和授权权威的前提下，逐步交付共享 TSX、Desktop IPC、正式 Web 客户端与可按契约定制的 Panel。

## 顺序与分包 / Order and packages

先执行可独立落地、仍兼容现有产品的 [S0 契约基础计划](2026-11-22-nsh-panel-contract-foundation.md)。S1–S6 按下列独立工作包制定后续实施计划。每包以所依赖阶段的真实接口和验证回执定稿，不把尚未选型的运行时细节、未经核查的页面迁移或待定 ADR 写成固定接口。S1 的 Desktop / Web 实现可以独立验证，但只能在共同 Gateway 认证分发接缝确定后并行；S2 运行时可移植性探针不依赖真实登录，产品接线依赖 S1。

| 阶段 | 可独立交付的软件 | 进入下一阶段的证据 |
|---|---|---|
| S0 契约基础 | 修正现有 hello；共享帧边界；R2 目标与当前挂载分开记录 | 现有 WS 无换线；1.0 协商与帧边界定向测试通过；明确新 endpoint 尚未上线 |
| S1-A 共同接缝 | 共享 Client 请求状态与 Gateway 已绑定身份的分发路径 | 同一业务 handler 经新旧路径到达；方法可见性、速率、请求状态、幂等和清理不分叉 |
| [S1-B Desktop](2026-11-23-nsh-panel-s1b-desktop-ipc.md) | 本地 UDS/Named Pipe endpoint，宿主通过 Client 使用它 | macOS/Linux UDS、Windows Named Pipe 真机验证；用户身份、帧边界、并发/关闭；不回退 TCP/WS。当前进度见 [S1-B 回执](../runs/2026-11-23-nsh-panel-s1b-desktop-ipc-verification.md)，平台门槛未关闭 |
| [S1-C Web](2026-11-23-nsh-panel-s1c-web-v1.md) | 原生 TLS 上的同源显式 HTTPS 凭据交换与只读 `/v1/ws` | [S1-C 回执](../runs/2026-11-23-nsh-panel-s1c-web-v1-verification.md)：Chrome 以原成员主体完成 hello/只读 RPC；缺凭据、跨端口、撤销和旧 `/ws` 在真实 TLS 挂载上验证，过期经绑定测试验证。未切换 Panel |
| S2 可定制壳 | 共用 TSX 壳、稳定 ID 布局、已有贡献投影及挂载清理 | 两端真实渲染同一源码；显示/隐藏/排序/重启恢复；取消异步加载后资源归零，旧 SDK 拒绝 |
| S3 会话纵切 | 会话/聊天/取消，真实 Native Component 与 Web fallback，替换同一能力的呈现 | 真实 Core 结果与后端身份；中文/日文 IME、流式、长列表；早期重取快照可用但不算 replay |
| S4 观察恢复 | 已授权会话/任务的一致快照、持久补读与 live 接续 | Desktop 重启 / Web 重连、并发写入、去重、有界队列和历史耗尽，证明不重做业务效果 |
| S5 迁移剩余页面 | 按台账逐页移植和退场；Generated UI 单独探针 | 每个页面真实挂载与效果到达；待移植模块及 Generated UI 未通过端保持可用旧路径或明确 unavailable |
| S6 收口 | 更新参考、测试回执、本地提交与已证明无消费者的清理 | S4 通过、页面退场逐项批准；默认切换和移除 Tauri 不由本计划自动触发 |

## 后续工作包的接线与判定 / Wiring and gate ownership

### S1：传输、认证和运行时所有权 / Transport and admission

- **先复用分发。** 从 `src/gateway/server/connection/mod.rs::handle_connection` 枚举首帧、限流、连接内身份/项目、鉴权、离线处理、通知和 cleanup 调用点。以 `connection/dispatch.rs::{dispatch_with_caller_context,revalidate_for_dispatch,dispatch_for_connection}` 为复用边界，先用新旧入口测试断言同一真实 handler 效果和未经授权的请求不可达。不为 IPC 另装一份 HandlerRegistry、会话上下文或幂等状态。`shared/client/src/connection.rs::AlephClient` 的 request ID、超时、关闭与事件代际只有一个所有者；`Tokio/Tungstenite` 细节留在 WS adapter；浏览器侧复用 wire 形状和现有 `shared/ui_logic/src/connection` 的可移植语义，不强塞本机 socket 依赖。
- **Desktop 先对齐宿主边界。** `desktop/nsh-shell/src/{supervisor,shell_host,platform,runtime,main}.rs` 保持服务与目标事务；IPC acceptor 归 Gateway `src/gateway/server/mod.rs` 的已有启动/关闭所有者。Unix private directory + 对端用户校验，Windows Named Pipe DACL + SID 绑定，映射 Core 主体后才 hello。计划需分别写出验证普通进程没有权限凭本机地址冒充其他主体、服务更替不会误删仍活跃 endpoint、断连释放连接而不停止会话。远程目标不冒充 local IPC，原来校验的目标及失败回滚原样保留。
- **Web 使用独立 endpoint。** `src/gateway/handlers/connect.rs::resolve_connect_auth` 是凭据校验现有语义，复用 `DeviceTokenManager` 和已绑定设备主体，不复用无凭据 loopback 分支。独立同源 HTTPS POST，短期 Secure/HttpOnly/SameSite=Strict/host-only Cookie；`src/gateway/server/connection/upgrade.rs` 的传输与 Origin 边界不是身份本身。新 `/v1/ws` upgrade 前验证活跃凭据，首个业务前消息 `system.hello`；授权、撤销和到期循 Core，不改旧 `/ws` 的 text/`connect`。测试浏览器网络请求与实际 RPC；成功用例必须证明仍绑定原普通成员，而非默默提升为 owner。
- **必须在 S1 关闭的边界。** length-prefix 的 partial/coalesced/partial write、首帧中止、实际 payload/inflight/depth 上限、关闭清理、旧客户端对接、多 RPC ID 乱序完成与同 ID 重用规则。逐端记录实测，不借 macOS 代签其他平台。

### S2–S3：共享界面和连线 / Customization and vertical slice

- **先过锁定运行时探针。** `desktop/nsh-shell/Cargo.toml` 保持 NSH `=0.6.0`；借鉴设计 §10 的 published NSH/noop 分层，不用参考仓 HEAD 替代锁定版本。先在隔离 QA 目标验证实际 Mixed 构建、Native Component 提供真实 props/events、Web fallback、dispose；探针不通过则记缺口，不偷偷 fork、升级或保留两套业务呈现。
- **用现有贡献管线。** `src/extension/{panel.rs,panel_publication/}` → `src/gateway/handlers/panel_capabilities.rs::handle_with_context` → `shared/protocol/src/panel.rs` → `interfaces/webchat/src/panel/{projection,registry,sdk,loader,shell}.rs`。抽取读侧索引、opaque SDK、代际与释放语义，分别适配 DOM 和 NSH View；不把原同源 JS 当隔离程序。读取目录时不启动 provider。测试替换和撤销交界，证明慢 import 不复活、已经释放的句柄不会跨 target/project 接收事件。
- **定制是可验证的产品路径。** 只持久化稳定 ID/布局（顺序、显示、激活、最小区域尺寸），绑定 endpoint 与用户；损坏偏好重置表现、保留业务。受信任插件 own qualified identity 可替换显示，不覆盖 builtin namespace 或改变后端。真实后端替换只对已拥有 Core 槽位和授权的叶子办理，另外验证新提供者效果、旧 lease 拒绝及故障降级的 `served_by`；本地布局不做任何授权。
- **第一条业务纵切。** 从 `interfaces/webchat/src/api/{chat,sessions}.rs` 追现有 `sessions.list`、`chat.history`、`chat.send`、`chat.abort` 的生产消费者；同源 UI 在两端验证真实落库／输出和取消结果。刷新时若只有快照重取，只标记 stale 和重取，不宣告 durable replay。native stored credential 候选单列回归：先证明拒绝的确是来自旧 stored lease；只有确认的认证拒绝可删除同目标同代凭据，不删除手输新值或把网络错误视为失效。

### S4：恢复契约 / Observation recovery

- `src/session/store.rs::SessionEventStore::{load_events_range,append_batch}` 与 `src/session/actor.rs` 是现有数据路径。截取可见会话/任务快照与同切点 watermark，复用真实 durable 事实源；`src/gateway/handlers/events.rs` 的 topic 通知只当 live 信号。若现有任务源无法给一致切点，计划需先证明缺口再提出最小接缝；不创建通用 EventLog 或通过推断写入监控事件。
- 测试快照与 live 接续交界上的写入、同一 scope 连续前缀、重复事件、权限变化/过滤和历史耗尽；不可证明连续性返回 `resync_required`。两端的 renderer 都显示正确恢复状态，且服务侧业务写入次数不变。ADR-007 通用方案不因此被接受。

### S5–S6：逐页迁移与退场 / Per-surface migration and retirement

下列覆盖 `interfaces/webchat/src/app.rs::LegacyMainContent` 的十个模式及已知外围入口；还不是子路由或资产的全量清单。各阶段以生产分派、feature flags 及项目路由索引逐条补齐：当前 renderer、进入方式、协议/能力事实源、NSH/Web 目标、双端影响测试、保留消费者、可删除条件。每新增或迁移一项必须写出实际渲染行；未盘点的子路由或资产不算完成。

| 当前入口 / owner | 替代验证 | 删除或保留判据 |
|---|---|---|
| `interfaces/webchat/src/app.rs::LegacyMainContent` Chat（`ChatView` / `PhoneChat`） | S3 先验证会话/历史/发送/取消；再盘点 workspace pane、消息附件等子入口与宽窄屏行为 | 仍由 Tauri、旧 Web、移动端或外部路由消费就保留；有真正替代和无消费者证明才删 |
| `interfaces/webchat/src/app.rs` Dashboard、Memory、Agents、Teams | S5 按 `DashboardRouter` 和各模式实际分派细化，验证 Core 投影、深链及手机已支持功能 | 同上；对只读页证明确实渲染正确业务数据 |
| `interfaces/webchat/src/app.rs` Projects、Terminal | S5 宽屏逐项迁移，测试项目切换的旧回调和终端生命周期；当前两者无 Phone renderer | 不把撤下 renderer 当停止服务，不扩大到未有的移动端功能 |
| `interfaces/webchat/src/app.rs` Extensions、Settings、More | S5 验证扩展管理与已有授权；枚举 `desktop_settings_body`、`SettingsRouter` 手机分派及 `PhoneMore` | 不误删仅手机可达入口；不能以默认壳渲染代替子路由验证 |
| `interfaces/webchat/src/app.rs::MainContent` 的 feature-gated `PanelShell` 与 Browser viewer 外围入口 | 核对真实挂载、开关与旧句柄；终端、浏览器遵循各自 QA 契约 | 禁用只释放本模块；资产端点仍有消费者不删 |
| `desktop/nsh-shell/src/{connection,session}.rs` 等候选原型 | 查 crate module 图、features、入口、引用与实际替代路径；第一步是不把未挂载原型当生产资产 | 确认无消费者再删，随对应替代切片单独提交；不按文件名猜 |
| `desktop/nsh-shell/src/{main,shell_host}.rs` 旧 WebView 路径；`desktop/shell` Tauri | 平台联机、窗口、服务身份、目标切换、独立进程工作继续；保留 Phase11 回执 | 不走捷径：三平台替代和真实共同 14 天双轨闸门保持；默认切换及回退移除另行决定 |
| ADR-005 Generated UI | 每端独立 owner-bound View 隔离/预算/停止验证；不与主 UI VM 混跑 | 未通过端 unavailable 并单列，不阻止基础软件继续迁移、不算 ADR-005 完成、不自动移除其旧路径 |

每个阶段使用独立 QA 根和入口，先读 `docs/reference/{SUBSYSTEM_ROUTING,DEVELOPMENT,FEATURE_LOCATOR}.md` 对应范围及 `qa/README.md`；优先复用已有 QA 装置与证据格式。收口同步 `AGENTS.md`、`CLAUDE.md`、`docs/reference/{REDLINES,DESKTOP_SHELL,FEATURE_LOCATOR}.md`，保留历史回执只证明当时/当平台的事实。不要改 main、安装/接管正式服务、触碰正式凭据，或执行 push/tag/release。
