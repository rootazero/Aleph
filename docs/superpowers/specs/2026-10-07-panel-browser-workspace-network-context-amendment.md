# Panel 浏览器工作区：网络授权绑定 BrowserContext 修订稿
# Panel Browser Workspace: Network Grant Bound to Task BrowserContext (Amendment)

**状态 / Status: Approved（用户书面批准，2026-10-07）。** 本稿只修订书面规格，不批准产品实现，不批准任何新 probe，不改变原 Task 0 Gate；下一安全 probe 须另行批准。
修订对象 / Amends: `docs/superpowers/specs/2026-10-07-panel-browser-workspace-design.md`（原文不改；批准后由主控回写）。

## 1. 已证实事实与边界 / Proven facts and limits
- P0（`.analysis/context-proxy-p0/evidence.md`）：Chrome 154.0.8037.98 / win32，一进程一条根 CDP；两个 context 以 `proxyServer`/`proxyBypassList=<-loopback>`/`disposeOnDetach` 创建成功，`getTargets` 的 `browserContextId` 匹配；proxyX X=1,Y=0，proxyY X=0,Y=1；fixture X=1,Y=1，各 context 代理不串；4.1s，剩余 `p0ctx-` Chrome=0。
- 仅为**能力**证据：未测 WS/HMR/QUIC/DNS rebind/popup/revoke；不构成任何安全 Gate。原 Task 0 Gate 仍 **Blocked**（Fetch 无法在连接前拒绝 WS）。
- Cookie / SameSite / host-only 只绑定 origin，不绑定 CDP target，**不得**作为 context 归属或授权证明（`security-local-redesign.md` §2）。
- 未证实：QUIC/HTTP3、WebRTC、DNS 预取是否绕过 context 代理；flag 被 Chrome 接受 ≠ 完整隔离。

## 2. 概念修订 / Concept change（用户已接受）
两个**不同**授权面，不得混成 ViewerGrant / Two distinct authority surfaces, never merged into ViewerGrant:

| | 网络授权 NetworkContextGrant | Viewer / 输入 ViewerBinding |
|---|---|---|
| 绑定 | principal + chat/session + run + **backend-minted context identity + epoch** | r4 原样：actual BrowserTarget / resource / epoch |
| 控制 | policy ∩ request；精确 origin+port；expiry / revoke | r4 控制租约、take/cede/input 不变 |
| 签发 | 仅 issuer 可信 managed-launch registry | 现有 IssueViewerGrant 流程 |

- 客户端**不得提交 contextId 自证归属**；contextId 由后端在 `Target.createBrowserContext` 成功后铸造并登记。
- 同一 context（同一任务）内的 popup / worker / OOPIF / SW **继承**该精确网络授权（scope 改变，用户已接受）；跨 context / 跨任务**无权**。**不再要求** popup 逐 target 网络拒绝。
- UI target ≠ network scope：Viewer/输入仍只对 actual target 生效，网络范围对 context 内所有 target 一致，二者不得互相推导。
- 不得把 network grant 偷偷加入 artifact 绑定、`ResourceBinding`、帧头或通用 bus 消息。
- 适用范围：仅 **managed Chromium 开发预览**。新 context 不继承 default profile 登录态/cookie/storage；不附着既有 Chrome；不静默切换引擎；Obscura / unsupported 引擎显式拒绝。
- 保持：单 profile 单根 CDP，单 recording / ACK / Fetch owner；context 化**不得**新增第二条连接。

## 3. 对原 spec 的精确影响 / Exact impact on original spec（行号为原文）
| 原节 | 影响 |
|---|---|
| §2.2 L53 "不…承诺完整 SSRF 隔离" | 保留；补充：context 代理不等于完整 SSRF 隔离，除非后续 Gate 按 engine+OS+cases 通过 |
| §3 表 L65 "网络安全" | "Fetch 逐请求拒绝点"改为"context 级出口代理为候选拒绝点（待证）；Fetch 仅辅助" |
| §4.1 L87 `PreviewNetworkGrant` | 改名/收窄为 `NetworkContextGrant`：scope 如 §2；其 scheme/host/port/origin/协议范围保留 |
| §4.4 L145 ViewerGrant | **不变**；追加一句：ViewerGrant 不携带、不兑换网络授权 |
| §6.1 L171 "按 task 绑定…" | "task"→"task 的 BrowserContext"；加入 context 创建/绑定/撤销生命周期（§5） |
| §6.2 L173–182 Spike 项 1–4 | 项 2 的 target 级判据改为 context 级；项 3 增加"mock 与 context 代理共享 owner"；新增 §6 的待批准 probe；项 4 引擎差异：Obscura 直接拒绝 |
| §6.3 L186 | 追加未证实清单（§4 末项）；不预先承诺平台数 |
| §10 安全验收 L243–249 | 判据限定 engine+OS+cases；未实测平台默认**不启用**开发预览网络授权 |
| §11 L260–268 | 在项 2 后加：本修订书面批准、probe 另行单独批准 |

## 4. 不受影响 / 将变动 / Unaffected vs changed
**不受影响（r4 wire/契约字段）**：`ConversationBinding`、`BrowserTargetBinding{profile,browser_session_id,target_id}`、`ViewerBinding`、`ArtifactBinding`、`ResourceBinding`、`ViewerPermission`、`IssueViewerGrantRequest/Response`、`ViewerGrantInfo`、`AuthHello`、`ViewerWsFrameHeader`、`BrowserLiveState/Closed`。不加 `context_id` 字段；context 仅在服务端由 target→context 登记表解析。

**将变动的内部接口（均待批准后实现，本稿不改）**：
- `crates/aleph-cdp/src/methods/target.rs:22-29` `create_target` 需可选 `browserContextId`；`:12-20` `TargetInfo` 需解码 `browserContextId`；新增 create/dispose context 方法。
- `src/browser/cdp_backend/migration.rs`（重建 target 时带 context）；`src/browser/cdp_backend/routes.rs`（Fetch owner 对 context 维度的 mock/recording 共享）。
- `src/browser/engine/chromium.rs:100-102`（仅 managed 启动记录进 registry）；`src/browser/network_policy.rs`（新增 context/origin/port 维度，不动全局 `block_private`）。
- 新增服务端 managed-launch registry 与 context 出口代理组件（使用既有 HTTP/tokio 设施；若需新库，**另行批准**，R3）。
- 现有 tab 不能跨 context 迁移，只能新建并重放导航。
- 出口代理：**不得依赖 target 猜测或仅日志拒绝**；host/port canonicalization；TLS CONNECT 只见 host:port，须显式端口。私网例外只覆盖 trusted launcher 确认的 dev-server endpoint，并在签发时固定地址与端口；每次上游连接只能使用该固定 endpoint，不得重新解析后扩大到其他私网地址。其他授权公网 origin 的 DNS 结果若转为未授权私网地址，必须拒绝。QUIC/WebRTC/DNS 绕过**尚未证实**，不得宣称已封住。

## 5. Context 生命周期 / Lifecycle
1. **创建前全拒**：无 context 或未绑定受信 issuer 时，该 profile 的开发预览网络请求一律拒绝（fail-closed；拒绝答案不得被当作值消费为许可）。
2. **创建**：managed launch registry 记录的 profile，经单根 CDP 创建 context（`proxyServer`、`proxyBypassList=<-loopback>`、`disposeOnDetach`），后端铸造 context identity + epoch。
3. **绑定受信 issuer**：registry 把 context 绑定 principal + chat/session + run；未经 registry 的 context 无授权资格。
4. **授权**：`policy ∩ request`，exact origin+port，带 expiry；颁发后才允许代理上游 connect。
5. **撤销/销毁**：revoke、expiry、run end、context dispose、epoch 递增、连接/issuer 死亡时，代理须**主动关闭该 context 已打开的转发连接（含 WS/HMR）**，而非仅拒绝新请求；旧 epoch 授权不可复活。网络切断不自动关闭页面：实际 dispose 必须另行满足 §5A 的控制或既有系统生命周期规则，不能因 revoke 绕过 human lease。

## 5A. 跨 target 副作用边界 / Cross-target side-effect boundary（不改 r4 控制契约）
- **授权扩大 ≠ 控制扩大**：context 内网络授权共享，不赋予 Agent 对 context 内任何 target 的控制、输入或页面变更许可；resource/target UI lease、take/cede/input 仍严格绑定 r4 的 actual target/resource/epoch，不因同 context 而合并或继承。
- **分别建模**：(a) **安全 revoke 的网络切断**（关闭代理转发连接、拒绝新连接）是安全动作，立即执行，不受 human lease 阻塞，也不是 Agent page mutation；(b) **Agent page mutation**（导航、输入、关闭 target 等）沿用 r4 序列化器与 lease 检查，不因 (a) 放宽。
- **多 target 动作**：会影响多个 target 的 Agent 动作（context dispose、engine 切换、批量关闭/重建）属 Agent page mutation，必须**复用并复检每个受影响 target** 的接管约束（r4 "actual CDP call 前重新校验"），任一 target 受 human lease 约束即整体拒绝，不得绕过 human lease、不得部分执行后静默继续。安全 revoke 触发的 context 销毁若需关闭页面，仅走 (a) 的网络切断；页面关闭仍须经上述复检或由人类/系统生命周期（run end 等）按 r4 既有规则处理。
- **不新增自动恢复策略**：拒绝/切断后由 Agent 或用户显式重试，harness 不替模型挑恢复策略（R10）。
- **不声称 cookie/storage 隔离消除同 context 页面间副作用**：同 context 的页面、popup、worker 共享 cookie/storage/缓存，可互相影响；context 隔离只约束跨 context。

## 6. 待独立批准的下一安全 probe 判据 / Next probe criteria（本次不运行，本稿不构成批准）
范围：`.analysis/` throwaway，自有 loopback 接收端 + 隔离 DNS 模拟，不触真实 LAN/metadata，不改产品。
- **Allowed**：真实 HTTP/HTTPS + WS/WSS/HMR 到授权 origin+port，接收端 `>0`。
- **Denied** 接收端 accept 全部 `=0`：错 origin、错 port、错 protocol、resolve-to-private（经 DNS 模拟，指未授权/未固定的私网地址，不包含已明确授权的 owned dev endpoint）、无 grant、cross-context。
- **继承**：同 context 的 popup/worker/OOPIF/SW 继承授权（`>0`）；cross-context 不得继承（`=0`）。
- **Revoke**：撤销后既有 WS ≤1s 关闭，新 accept `=0`。
- **Owner 不破坏**：mock route add/remove 与 recording/lifecycle 共享同一 Fetch owner，单根 CDP 不变，无第二连接。
- **旁路**：QUIC/WebRTC/DNS 预取等旁路 protocol 若无法封住 ⇒ `Gate=Blocked(reason=<exact>)`，不降级为部分完成。
- Gate 结论必须按 **engine + OS + cases** 限定；本修订**不承诺**三平台实测，未实测平台默认不启用开发预览网络授权。

## 7. 自审 / Self-review
- 无 TODO；无"popup 逐 target 拒绝"遗留要求（已由同 context 继承取代，§2）。
- host-only cookie ≠ context 归属：context 归属仅来自后端铸造的 identity + registry，cookie 只作被隔离的副作用（不同 context 存储独立）。
- UI target ≠ network scope：§2 对照表与 §4"不加 `context_id` 字段"一致。
- 原 Task 0 Gate 仍 Blocked；P0 仅为能力证据。
- §5A：网络授权扩大未被写成控制许可扩大；revoke 切断与 Agent mutation 分开建模，r4 lease 契约未改。

## 8. 明确未做 / Not done
未批准产品实现、未批准或运行新 probe、未改 main / 原 spec / plan / ledger / 冻结 r4 / scratch、未提交（文档提交由主控安排）、未安装依赖、未启动浏览器、未验证 Windows 以外平台及 QUIC/WebRTC/DNS 旁路。
