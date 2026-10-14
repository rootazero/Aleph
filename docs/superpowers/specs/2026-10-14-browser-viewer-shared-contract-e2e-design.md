# 共享只读浏览器契约与真机收口设计
# Shared Read-only Browser Contract and Real-machine Acceptance Design

- Date / 日期：2026-10-14
- Status / 状态：DRAFT — builder check completed; material planning questions resolved below; awaiting user approval. No implementation authorized by this document.
- Worktree：`D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`
- Branch：`feature/panel-browser-workspace-20261007-202532-13256`
- Predecessor：`docs/superpowers/specs/2026-10-07-panel-browser-workspace-design.md`
- This design narrows the immediate delivery to Frames-only and adds recovery and real-machine gates; other predecessor constraints remain effective. 本文把近期交付限定为只读，增加恢复契约与真机门禁；前规格其余约束继续有效。
- No commit/merge/reset/cleanup is authorized. Local `.analysis/` evidence remains preserved, not force-tracked. 不提交、不合并、不 reset、不清理缓存或用户数据。

## 1. 用户目标与已确认决定 / User intent and confirmed decisions

用户要看到的实际结果：LLM 在 Panel 对话中给出本地／公网页面的浏览器资源连接，用户点击后直接展开右侧栏看到页面；TUI 用户触发连接后，由系统默认浏览器显示同一 Gateway target。

The desired user-visible result is a resource link in the Panel conversation opening the browser right rail; the same resource activated in TUI opens the OS default browser, rendering the same Gateway-owned target.

用户已确认 / Confirmed by the user:
- 优先 managed-companion / Native / 全链路真机证据，而非先完成全部 off-loop 优化。
- 先冻结 Panel/TUI 共享契约，再实施两端。
- 当前只读 `Frames`；显式接管、输入、导航权限仅列入路线图。
- 普通断线／grant 到期允许一次自动恢复；恢复时保留最后一帧并明确标识非实时。
- 撤销、导航、target 关闭、身份变化不得静默自动恢复；旧画面清空。
- 允许只读核对现有实现后写完整设计；尚未批准本设计或新的高副作用验证。

我的设计选择 / Designer calls, not independently specified by the user:
- 只有结构化浏览器资源链接进入 viewer；普通 Markdown 超链接保留原行为。LLM 用 Gateway 工具准备网页，而不是客户端从裸 URL 推断目标。
- Sidebar 使用现有 Leptos/WASM viewer transport，直接渲染 JPEG，不引入远端网页 iframe、第二个 CDP 客户端或第二个业务 UI。
- Pending 资源卡明确写“确认只读展示并查看”。一次点击展开右栏并对卡片显示的精确 revision 发起现有 Safe CAS；已 Safe 的卡片写“查看页面”。不让 LLM 自动判 Safe。
- 外部 viewer 的自动恢复采用新的、服务端限定的单次短期恢复能力，不把 device token 或 bootstrap ticket 放进浏览器。
- 首个闭合目标是 Windows + Chromium 的真实本地链路；公网 HTTPS 链路与其他平台另列独立必验门，不用 Windows 结果替代。

## 2. 范围、非目标与反目标 / Scope, non-goals and anti-goals

### In scope / 本次设计范围
1. 资源发布到对话的结构化连接、Panel sidebar Frames、TUI 默认浏览器交接。
2. Native protected pairing → keyring save ACK → reload same-device → Safe decision → grant → real frame。
3. 单次自动恢复、stale-frame UI、安全关闭、issuer device/user 失效传递。
4. Windows managed launch 的现有 AppContainer/WFP 生产路径验证与最小接线；真实 companion handshake、资源发布、同一 viewer 生命周期。
5. 当前源码／构建／进程／served WASM 一致的 E2E 证据，不拼接历史 PASS 冒充一个版本通过。
6. 明确分层的跨平台、shutdown、Obscura/subframe 验收矩阵及阻塞条件。

### Non-goals / 非目标
- 本轮不实现 HumanInput、Navigate、接管、交回；没有隐藏控制入口。
- 不捕获任意外部脚本的 `open`，不自动把普通超链接升级成浏览器任务。
- 不开通用户机器到公网 Gateway 的通用隧道；Gateway 的 localhost 不是用户机器的 localhost。
- 不引入 SCM 常驻管理员服务、不自动 UAC 提权、不默认给 AppContainer 安装系统级 loopback exemption。
- 不解决全部 D1 剩余同步步骤，也不宣称 Tokio 的 abort 能中止已启动的阻塞函数。
- 不执行 commit、merge、reset、renormalize、删除 target/.analysis/nul/用户数据。这些不是功能验收前置条件。

### Anti-goals / 不能以这些方式“完成”
Synthetic grants, fake keyring evidence presented as protected persistence, a green test discovery list presented as E2E, static capability flips, plaintext auth fallback, arbitrary URL-to-target inference, and unrelated-home test side effects are disallowed.
不得用合成 grant、内存后端、静态 flags 或测试发现列表冒充真实闭合；不得用全局放开私网替代 sandbox／network grant。

## 3. 当前实现事实与差距 / Current facts and gaps

Read-only reconnaissance; these are source/report facts, not fresh runtime verification. 源码／报告事实，不是本轮新运行结果。

| Area | Existing implementation | Required delta |
|---|---|---|
| Shared wire | `shared/protocol/src/browser_live.rs`: PreviewResource、ViewerBinding、Frames grant、consume/exchange/AuthHello、closed reasons | Structured conversation reference, public non-secret fence identity, bounded recovery protocol |
| Producer | `src/builtin_tools/browser_tools/operation_publish.rs` auto-publishes Pending resource after real CDP operation; `present.rs` prevents LLM-asserted Safe | Return the exact publication reference through tool output and transcript projection, without logging grant secrets |
| Panel chat | `interfaces/webchat/src/components/markdown.rs` renders sanitized ordinary links; chat messages lack browser-resource parts | Structured resource card bound to session/run; click routes to sidebar rather than window.open |
| Sidebar | `components/workspace_panel.rs` already has Browser body; browser viewer launch UI opens a new tab | Reuse `session.rs`/`transport.rs` in sidebar; cleanup, recovery, stale overlay |
| Standalone | consume → exchange → rotating AuthHello secret → JPEG has Windows historical evidence | Same bounded recovery UI/protocol and revocation behavior; no long-lived issuer credentials |
| TUI | `/browser <exact resource_id>` and shell-free `viewer_launcher.rs`; launcher default disabled | Selectable transcript resource action, explicit Safe decision, safe default-OS-launch policy and real OS evidence |
| Protected auth | Native Bridge/keyring implementation, Windows keyring probe, QA IPC memory-backend PASS | Full production-backend Native run, same-device reload, resource recovery after reload and sidebar frames |
| Managed | `launcher.rs` implements ManagedLaunchApi; driver spawn uses it; WFP/AppContainer code exists | Capabilities/HostFacts remain false; actual admitted launch/network/readback/cleanup and handshake fixture missing |
| E2E | Historical selectors plus newer isolated runs | Version-aligned run manifests and missing in-Panel/TUI/native/managed integration evidence |

Correction to stale summaries: `docs/reports/verify-20261007/task56-native-real-window-6.md` and `task56-native-real-window-7.md` exist. Window 6 failed at `server_same_device`; window 7 failed at `reveal_browser`, with MAX_WINDOWS=7 stated in that report. No further window is authorized by those historical reports or this design. 本次不运行第八窗口；新预算必须重新取得明确授权。

Managed source `src/sandbox/platforms/windows/driver.rs` calls launcher after preflight; it is not accurate to say no production ManagedLaunchApi code exists. The correct distinction is implemented-but-disabled and unproven-on-admin-host. 历史文档要按这个区别更新，不把“代码存在”改写成“安全能力通过”。

## 4. 唯一所有权与结构化连接 / Ownership and resource references

### 4.1 ResourceReference — proposed shared wire extension

在 `shared/protocol` 定义一份有版本的 browser resource reference，复用现有资源／任务类型，不在 Panel/TUI 各自复制：
- `resource_id`, `expected_revision`, exact `ConversationBinding` (session_key/run_id).
- A non-secret, opaque Gateway-minted `view_fence` representing the resource revision, target binding and internal target/navigation epoch. It is not authorization and cannot be used to pick a target.
- Safe display label and kind; all labels remain untrusted presentation data.
- No destination URL required to launch, no grant_id/nonce/ticket/device_token embedded in the message.

这是提议新增契约，不是现有 wire 字段。Exact Rust names and backward-compatible version negotiation are left to the implementation planner; the semantics here are fixed. 旧消息没有此字段时继续作为普通文本，不扫描文本识别资源。

Initial issuance and recovery must be server-fenced against the submitted exact reference: resource revision, task/user visibility and view_fence must still match when authorizing and committing a grant. A client-only catalog check followed by the existing resource_id-only issue request is insufficient. Whether this is a versioned method or a negotiated request extension is left to the builder; if the peer lacks the fence/recovery capability, the new flow reports unsupported and does not silently downgrade to unfenced issuance. The legacy viewer path remains compatibility-only and cannot count as this contract's acceptance.

Gateway 在浏览器工具完成真实操作并成功发布后，把 reference 作为结构化结果交回工具输出／转录记录；同一结果也供 LLM 表述。客户端只渲染 Gateway 已验证的结构化引用。LLM 文字里的伪造 resource_id 不获得点击能力。

Publication failure must not be reported as successful viewer preparation: page opening may succeed while the tool explicitly reports that no viewable reference was published. 消费者可恢复加载精确 catalog 资源；不能从 URL 找“类似”目标。

### 4.2 User activation / 用户动作

Panel 点击卡片立即展开右栏，不打开新标签。
- Pending：卡片明确告知点击即“确认当前版本只读展示”；Gateway 根据已认证 owner User、expected_revision 执行现有 `resource.decide(safe)` CAS。
- CAS 成功后的 revision 由 Gateway 返回，用此新 revision 对应的 fence 获取 grant；不把旧 Pending fence 当新 Safe fence。
- Already Safe：检查当前资源完全匹配再 issue Frames grant。
- Refused、closed、expired、stale reference 或异会话：泛化拒绝，显示“资源已失效／需要重新发布”，不偷偷确认新版本。
- 资源与 user/task/connection/Gateway 切换都清理旧 session；同一任务新资源也必须有新的明确点击。

TUI 用同一语义：选择资源卡的“确认并查看／查看页面”动作；`/browser <id>` 保持兼容并从精确 catalog 构造同一引用。Pending 时必须有显式确认，命令不能暗中把 Pending 变 Safe。TUI 不要求终端鼠标支持；键盘选择提供等价激活。

TUI 默认启动策略（designer call）：明确选择资源卡或执行 `/browser` 是本机默认浏览器启动的用户同意，不要求额外设置 opt-in；只为此显式动作路径启用 launcher，不允许被动事件自动打开。已有用户显式禁用设置继续优先，必须提示禁用而非绕过。Default browser means the machine running TUI. SSH/headless/noninteractive sessions lack the local-desktop handoff contract: refuse OS launch with a clear unsupported-local-browser message before minting a grant; do not pretend a remote server browser is the user's local browser, and do not log a token-bearing fallback URL. Remote-client handoff is outside this delivery.

Ordinary links stay ordinary links. No arbitrary href handler invokes CDP or grants. 本地／公网目标的选择属于 LLM 工具及 Gateway 网络策略，不属于 interface 路由推断。

### 4.3 Shared display state / 共享展示状态

`Idle → Opening → Live → Recovering → Live` or `Closed/Error`.
- Opening：sidebar 展开、显示加载，consume/exchange 完成后拿 frame；失败不给成功提示。
- Live：真实 frame；Frames-only 无输入事件转发，无导航动作。
- Recovering：保留本 view instance 的最后一帧并覆盖“重新连接中，内容可能已过期”。不存 localStorage、磁盘、转录记录或新用户 view。
- 第二次恢复失败：清除 frame，显示简要原因和用户手动重新打开入口；诊断中不记录 secrets。
- 安全 terminal：立即清除 frame/credential/recovery capability，不能因 queued reconnect response 恢复画面。
- 用户关闭 sidebar 或外部 viewer：终止 transport；Panel/TUI owner 清理未消费 grant；关闭不影响 Gateway target 本身。

Gateway 状态和原因是唯一事实源；UI 仅做 rendering 和生命周期 I/O。No heuristic retry strategy in harness.

## 5. 单次恢复、安全失效与凭据 / One recovery, safety closure and credentials

### 5.1 Why a new narrow mechanism / 为什么不能直接调用 issue

Standalone viewer 没有 verified-device RPC 身份，不能调用 `browser.viewer.grant.issue`；把 device token 注入页面或让原 nonce 重放都违反既有边界。Panel 虽持有认证连接，也不能自行重试到新 target。

Proposed solution: a separate one-use resume capability issued by Gateway, bound to the same viewing lineage. This is new security surface requiring design approval and adversarial tests, not an existing feature. 所有 viewer 共用同一恢复判据与额度；发行一次新 grant 的决定在 Gateway。

### 5.2 Server-bound lineage / 服务端绑定

每次用户激活建立 viewing lineage，服务端保存：
- Exact resource_id + Safe revision + binding + internal epoch / view_fence.
- Original verified issuer user AND device identity/auth epoch (server-derived, never trusted from request fields).
- Frames-only permissions, origin, absolute resource expiry, active generation, recovery_used=false.
- Revocation/tombstone state and one active viewer session per lineage.

Recovery requires resource still active+Safe, exact fence unchanged, issuer User active and device still valid, role/scope still admit the viewer action, same origin and permission set. 旧 grant 被明确撤销、target 导航/关闭、engine/service/run 结束、resource refused/expired、身份/设备失效均禁止恢复。

Existing policy does not cascade device revoke to all Frames grants. This design deliberately changes that: originating-device revoke, principal invalidation and relevant privilege demotion terminate that lineage. Token rotation that invalidates the originating token/device auth epoch is also terminal, even if the same user subsequently authenticates again; the old lineage does not recover across rotation. A new explicit user activation under the new verified epoch creates a new lineage. This is the designer's security choice, implemented using canonical token/device invalidation hooks, never a parallel UI policy. Revalidation also occurs before delivering new frames and before recovery, so absence of a UI event cannot authorize a new stream.

### 5.3 Capability and exchange / 恢复能力

- Gateway supplies a high-entropy opaque recovery handle over the authenticated viewer channel; memory only. No persistent cookie, localStorage, URL, argv or transcript entry for it.
- Separate store/namespace from nonce/ticket/rotating secret. The existing consumed-nonce and secret-replay protections are not relaxed.
- The handle only permits one authenticated-origin recovery of this lineage; cannot list resources, execute tools, issue arbitrary grants, extend resource TTL or add permissions.
- Reconnect request carries the handle and a request attempt id. Server atomically consumes the one retry budget, invalidates the previous viewer generation, and returns a replacement handoff via the response body (not a new URL in history).
- Same request retry may get an idempotent cached response only while its replacement ticket remains unconsumed; after consumption, replay yields denial and never revokes an unrelated newer session. Concurrent attempts have at most one winner.
- The replacement has recovery_used=true and receives no further renewal capability. A later user click is a new explicit viewing lineage, not a reset caused by networking.
- Do not shorten the existing 600s grant cap just to force a renewal demo. Designer default: original grant expiry is `min(resource_expires_at, issued_at + existing_600s_cap)`; replacement follows the same cap and the SAME original resource deadline. There is no new artificial four-minute viewing cap and no indefinite renewal. A separately configured shorter grant lifetime can create a grant-only expiry; dedicated timing tests may use shorter real deadlines without claiming a 600s production wait was run.
- At the grant deadline, Gateway stops frame delivery from the old session. A grant-only expiry is recoverable only if the resource is still valid; default grants may expire simultaneously with the 600s resource, in which case no expiry recovery occurs. Resource expiry itself is terminal and NEVER renewed automatically.
- Recovery handle validity ends at `min(resource_expires_at, old_grant_deadline + 30s)`; network-loss recovery may begin immediately before the grant deadline. The post-expiry grace is exactly `[old_grant_deadline, old_grant_deadline + 30s]`, truncated by resource expiry, not proactive renewal. The handle confers no right to keep delivering old frames during grace.
- Add explicit viewer transport expiry semantics distinguishing `GrantExpired` (conditionally recoverable) from `ResourceExpired` (terminal), separate from the resource lifecycle tombstone. Exact wire representation/negotiation is a builder choice; existing ambiguous `ClosedReason::Expired` from older peers must be treated as terminal, never guessed recoverable. Unknown close reasons fail closed.
- Normal WS interruption may use a fresh rotating-secret reconnect inside this same single retry budget; it must not additionally mint a grant or attempt a second recovery. Either successful reconnect or grant replacement spends the one automatic attempt, so Panel and external viewer cannot double retry.
- Recovery attempt wall-clock timeout 15s (designer call). During that interval last frame stays stale; any safety event or denial clears it immediately. No proof that a stale screenshot vanished from a user's previously captured screen is claimed.

### 5.4 Protected auth and URL handoff / 受保护认证与交接

- Only device_id/device_token are persisted in OS keyring through Native Bridge/TUI store; bootstrap ticket never persisted.
- `protected_user` requires Gateway identity echo and keyring SAVE ACK; no UI-only success and no localStorage fallback.
- Native reload must load the same device without re-pair/re-save and Gateway must prove a newer authenticated connection for that device, plus successful owner-only viewer action.
- Viewer launch URL has only short-lived one-shot grant handoff in fragment; scrub before consume; no token-bearing query/referrer/logs. Default OS launch necessarily hands a short viewer URL to the opener process; never a bootstrap or device token. 原约束“token 不进 argv”指长期身份凭据，短期 viewer fragment 仍需泄漏面审计。
- Panel sidebar uses in-memory consume input, not href/window.open and not browser navigation. CSP/origin validation remain Gateway-owned.

## 6. Native 与 Windows managed 路径 / Native and Windows managed path

### 6.1 Native root-cause gate

新窗口前先用非 Native 浏览器诊断与隔离 fakes 建立证据：resource publish → per-user delivery/filter → decode → model acceptance → workspace reveal。Capture non-secret outcome labels and connection/resource revisions; not only a subscribe call. 不以降低 DOM 判据让测试通过。

Window 7 报告已指出订阅先于发布，但 delivery/model 未被观测；未配对初始路径与 paired reload 路径分开测试。新契约允许首次配对发生在用户点击资源之前；不能要求 unpaired connection 拥有 viewer authority。配对完成后从 Gateway authorized resource catalog seed，并再按绑定与当前任务校验。

Then, only after an explicit new Native budget and owned-root/keyring cleanup approval, run real keyring-backed Tauri Panel and prove end-to-end success. 不在本设计中自动提高 MAX_WINDOWS。

### 6.2 Managed proof gate

Reuse the existing parent-side suspended AppContainer launcher, owned Job process identity, WFP filters and readback; do not introduce a wrapper PID or SCM service. WindowsSandboxDriver stays fail-closed until host and tested production capability facts meet preflight.

The first proof environment is a disposable Windows VM with explicit operator elevation. Installing or changing policy on the everyday host is not a fallback. Elevated Gateway use is limited to the isolated QA environment; this design does not decide a production privilege-distribution/installer model.

Required fixtures (to be built after plan approval):
1. Owned network fixture with allowed/forbidden loopback TCP ports, external reachability baseline, IPv4/IPv6 and a receipt log to prove arrivals/denials; no real private network or metadata target.
2. Handshake-speaking companion service with exact bind/port protocol, safe fixture page, status/health, graceful stop and controlled failure modes.
3. Probe using the SAME production launcher/WFP path and exact returned service PID+creation time/Job identity; no proof-only capability bypass reported as product PASS.

Sequence must prove: process suspended → AppContainer identity/ACL → Job admission+membership → exact WFP rules/readback → resume → handshake/endpoint binding → resource Pending → explicit user Safe → Frame → stop → Job empty → filters/profile/ACL removed. Unknown teardown retains confinement and reports failure rather than announcing clean.

Test forbidden egress reaches no receiving fixture. Include denied ports, address-family variants, child process restrictions, service exit, cancellation, forced termination and residual-filter checks. Network outage without filters cannot count as WFP denial.

AppContainer loopback arbitration is a research gate: first measure whether current WFP exact permit can reach the approved port. If it cannot, STOP and return the minimal per-SID exemption alternatives for user review. No automatic global exemption, no silent SCM delegation. Any future exception would require exact owned SID scope, rollback/crash reconciliation and renewed design approval.

CURRENT_CAPABILITIES and HostFacts are not flipped merely because code compiles. After real evidence, capability representation must describe a supported implementation plus actual host readiness—not grant approval via a client assertion. The planner must derive both preflight and launch from the same capability source and include negative host tests. This document does not authorize a flag flip.

## 7. 本地与公网拓扑 / Local and public topology

- Local destination means a service on the Gateway host. Remote TUI/Panel see Frames; they do not need direct access to the destination URL.
- A local Gateway may use explicitly configured loopback HTTP origin. Public Gateway must use an explicit reachable HTTPS viewer origin and WSS, with exact Origin/CSP validation and existing protected-device auth; no `allow_any_origin` fallback.
- 本轮不新开公网端口、不配置用户 DNS/cert、不做隧道。Public proof needs an operator-provided isolated HTTPS endpoint plus credentials/cert provisioning, never secrets in the design.
- Managed loopback permit is exact port on the Gateway host; does not replace browser per-task network grant. Local dev preview must pass BOTH companion sandbox policy and browser engine/network-context policy.
- Unknown engine/network capability refuses that preview; regular publicly permitted pages do not need a managed producer. This distinction avoids using managed absence to block all viewer work or using standalone success to approve managed.

## 8. 验收分层与证据 / Acceptance layers and evidence

Every result records source revision + dirty-file hashes, executable hash, served JS/WASM hashes, config with secrets redacted, process identity, commands/exit codes, fixture receipt evidence and cleanup. A PASS on a previous executable is historical, not a test of newly edited source. 使用真实 Gateway grants、真实 CDP、真实 JPEG/WS/DOM；LLM scripted tool-choice fixture可以验证接线，但至少一个可用真实 provider 的 smoke 验证自然对话输出连接。Provider 不可用时此格 BLOCKED，不能说模型流程已通过。

| Gate | Required evidence | What it does NOT prove |
|---|---|---|
| A: shared contract | Wire round-trip, stale revision/fence, forged link, wrong user/session, Safe CAS, one recovery, replay/race, device revoke, role demotion tests | Native or managed OS behavior |
| B: Panel click | Real message resource card click opens sidebar; frame arrives; no new tab/target; ordinary link unchanged; reload seed and task switch fences | Native keyring persistence |
| C: TUI launch | Real paired TUI activation starts system default browser; same issued resource/target; one OS launch; actual failure surfaced; fragment hygiene | Merely mocked CommandRunner is insufficient |
| D: recovery/closure | Disconnect/grant-only expiry retains stale frame then one recovery; second loss stops; revoke/navigation/device invalidation clears and cannot recover; resource expiry is terminal | Polling old frames is not a live proof |
| E: Native | Real OS keyring Bridge load/save ACK/reload same-device plus Panel resource click/frame and cleanup of exact test namespace | QA memory backend IPC does not count |
| F: managed | Elevated disposable-VM production path, network receipt negatives, handshake binding, Panel/TUI frames, stop/reap/confinement cleanup | Contract/fake ManagedLaunchApi tests do not count |
| G: Gateway shutdown | Owned isolated process stop, viewer terminal state, managed process/confinement cleanup; no residual serving frames | Socket disconnect alone is not whole resource cleanup |
| H: public origin | Reachable isolated HTTPS Gateway, remote client Frames, no target access required, strict origin rejection and protected auth | Loopback HTTP does not prove public deployment |
| I: engineering | Audited targeted tests, WASM/server build, clippy, fmt actual exit; then audited full workspace run where feasible | Compile/unit scope does not substitute for B–H |
| J: other platforms/engines | macOS Keychain/seatbelt build+real test, Linux Secret Service/runtime, Obscura eventless/subframe network boundaries on respective available runners | Windows/Chromium cannot prove these |

Read-only user intent does not justify dropping subframe egress tests: sandbox/network authorization must still constrain requests even with no human input. 新安全支持被声明前仍需相应 engine 的证据；缺平台时 unsupported/BLOCKED 作为明确状态，不篡改为 PASS。

Global completion requires applicable gates actually pass or the user explicitly changes the scope in a later approval; Windows milestone can complete independently, but not mean all platforms are ready. Overall remains PARTIAL until then.

## 9. 顺序、依赖与后续路线图 / Ordering and roadmap

This is a design, not the implementation task list. 本文批准后用 writing-plans 生成具体计划。

Plan deliverable is the entire A–J contract and evidence program, including Native/managed fixture and wiring implementation NOW, not only A–D with permanent placeholder gates. Execution is phased: safe code/fixture preparation and isolation tests proceed after approval; E/F/G/H/J real runs wait at explicit environment/authorization gates. If a gate remains unavailable, keep it BLOCKED and overall PARTIAL, deliver completed code/evidence, and request the exact missing input. Do not redefine this user's managed/Native objective into standalone-only completion. Windows A–G+I forms the first milestone; H and J remain distinct required follow-up gates unless the user explicitly narrows scope.

1. Freeze shared reference/fence/recovery/issuer-invalidations and wire negotiation.
2. Resolve observed Native reveal/reload gaps with instrumented non-Native evidence; simultaneously prepare the isolated managed fixtures, without running admin/native gates.
3. Wire resource cards, sidebar session, TUI activation and one recovery against the common contract.
4. Build version-aligned bundles and complete gates A–D locally.
5. Obtain narrow new authorization/environment for E/F/G, then run production-keyring and elevated managed proof; do not repeat failed windows without new evidence.
6. Public-origin and platform gates follow on provisioned runners; publish a truthful matrix and update stale historical docs.

Lower-priority follow-up / 后续事项：
- D1 residual sync work: evaluate WASM/vault/projection/hook/legacy steps individually, with ownership/cancellation tests and real cold timing. No unbounded mutex/runtime wrapping.
- D2: bounded teardown remains the goal; where feasible pass cooperative cancellation into scans between filesystem operations. Cannot promise immediate preemption of Tokio blocking tasks; document OS calls that cannot be interrupted.
- Ordinary queued admin RPC arrival snapshots: separate audit using canonical authorization after lane permit; do not attach it silently to a viewer-only fix.
- User-visible control roadmap: explicit consent and separate permissions, HumanInput/Navigate grants, capture and navigation fencing. No production buttons until that design is approved.
- Full workspace/cross-platform QA: audit home/keyring/SQLite/delete/env paths before run; never mutate HOME/USERPROFILE to pretend Windows isolation.
- Commit/merge/disk cleanup: separate user-authorized operational stage AFTER preserving/validating all changes. Not a task automatically triggered by PASS.

## 10. 安全执行与阻塞条件 / Execution safety and stop conditions

No new high-side-effect authorization was given during this brainstorming. The plan must ask separately, at the execution gate, for:
- New Native count/duration, exact root and random credential namespace, exact cleanup, and any rebuild/restart scope.
- Disposable VM availability, manual admin session, ignored AppContainer/WFP tests and expected owned HKCU/Packages/ACL/filter changes.
- Gateway stop/restart budget; exact existing PID/image/config must be proven owned before stopping.
- Public HTTPS endpoint/provider/platform runner availability.

Never access/delete/repair `C:/Users/zou/.aleph`; preserve dirty/untracked `.analysis/`, `nul`, `main`. Cargo only after path audit and memory >=4294967296, single runner, exported jobs=1/debug=0/offline; debug/codegen reduction as verified in prior reports. No cleanup to hide failed runs.

Stop when a required environment/permission is missing, a safety invariant fails or policy arbitration cannot support exact confinement. Preserve evidence and name the next input needed. Do not loosen acceptance or auto-renew Safe decisions to make a demo pass.

## 11. 留给实施者的选择 / Builder discretion

The builder may choose exact Rust type/function names, UI styles, bounded backoff <=15s, transcript part versus typed metadata within existing transcript architecture, and test fixture organization. They may not choose different target inference, wider permissions, automatic Safe, plaintext auth, multiple retries, an exemption/SCM architecture, or treat historical PASS as current evidence.

Builder check completed on 2026-10-14. Four planning questions were resolved as designer calls: (1) retain the existing 600s cap, distinguish grant/resource expiry and define the 30s post-deadline grace; (2) treat originating auth-epoch invalidation on token rotation as terminal; (3) explicit local TUI activation is sufficient launch consent, while SSH/headless launch is unsupported; (4) include fixture/wiring work and all A–J gates in the plan with gated execution. No new user testing permission is inferred.

Open approval point: the new bounded resume capability and origin-device revocation cascade are designer proposals, not current behavior. They are covered by approval of the full design only. 本文尚未得到批准；本轮未改产品代码、未安装依赖、未执行 Cargo/真机测试、未新增 Native 窗口或管理员 probe。
