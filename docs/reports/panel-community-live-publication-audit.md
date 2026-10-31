# Panel production publication audit / Panel 生产发布审计

Date / 日期: 2026-10-31

Status / 状态: **Core producer scoped lifecycle verification passed; production HTTP authentication and browser delivery remain incomplete. / Core producer 已接入并通过聚焦 lifecycle 验收；production HTTP 认证与 browser delivery 仍未完成。**

## Ownership and current boundary / 所有权与当前边界

The initial audit at `fae49f50684dfb95bcf9c0bfdb0a199395926511` found only an unmounted Gateway byte store and stopped under the plan's missing-producer rule. The user subsequently approved [the Core byte-admission design](../superpowers/specs/2026-10-31-panel-core-byte-admission-design.md) for dependency #7. The actual store has moved into Core; the former Gateway module and caller `authorized: bool` request are removed. Removing that boolean is a data-ownership migration, **not** a route-owned authentication proof.

起初基线审计只找到未挂载的 Gateway bytes store，按缺失 producer 条款停止；用户随后批准 #7 的 Core bytes 准入设计。实际 store 已迁入 Core，原 Gateway 模块及 caller `authorized: bool` request 已移除。移除 boolean 是数据所有权迁移，**不是** route-owned 认证证明。

| Boundary / 边界 | Current owner / 当前 owner | Meaning / 含义 |
| --- | --- | --- |
| Declaration and trust admission / 声明与信任准入 | `src/extension/panel.rs`, existing `OwnerTrustPolicy` | Existing explicit manifest derivation; generic plugin entry cannot publish a Panel. Existing load/admission-gate trust semantics remain. / 复用显式 manifest 派生与既有 trust gate；generic entry 无权发布 Panel。 |
| Exact bytes and scope / exact bytes 与 scope | `src/extension/panel_publication/{mod,admission}.rs` | One state retains declaration, immutable bytes, internal `ScopeKey`, and committed revision; no parallel snapshot authority. / 同一 state 保留 declaration、immutable bytes、内部 scope 和 committed revision，无第二 snapshot authority。 |
| Lifecycle producer / 生命周期 producer | `src/extension/lifecycle.rs::after_transition` → `Views::republish_plugin_projections` | Sole existing trigger; copy registry facts, release its guard before file I/O, prepare privately, then commit under policy-read → publication-write locks. / 复用唯一 trigger，复制事实后释放 registry guard，锁外准备，再按固定锁序 commit。 |
| Scoped reads / scoped 读取 | `ExtensionManager::{panel_ui_capability_snapshot_for,panel_ui_asset}` | Both reuse `visible_to`. No-context compatibility discovery uses `VisibilityCtx::default()` and exposes Global only; no cwd fallback. / 两者共用 visible_to，无 context discovery 只见 Global，不借 daemon cwd。 |
| Public wire / 公开 wire | `shared/protocol/src/panel.rs` | Existing declaration/snapshot shapes unchanged; scope and filesystem roots stay internal. WASM depends on Protocol, not Core. / 不改 wire shape，scope/root 保持内部，WASM 不依赖 Core。 |
| HTTP address and authentication / HTTP 地址与认证 | Not implemented / 未实现 | No Panel route merge, HTTP auth proof, or browser credential injection. Canonical `./tabs/` metadata does not prove an endpoint exists. / 无 route merge、HTTP proof 或凭据注入，canonical metadata 不证明 endpoint 存在。 |
| Browser lifecycle / 浏览器生命周期 | Existing `PanelProjection`, `PanelLoader`, `PanelShell` | Explicit `/panel/<plugin_id>/<tab_id>` only; snapshot arrival stays passive. Deterministic fixture uses a separate fixed map. / 仅显式 selection；snapshot passive，fixture 仍是独立 fixed map。 |

The Core producer reads only lifecycle-owned `PluginRecord.panel.entry`, under canonical root containment, from a regular file. It checks a **4 MiB** entry limit, a bounded read, exact SHA-256 SRI, and a deterministic **64 MiB** generation budget counted per record. Entry failures and duplicate keys omit that Panel while other plugin capabilities continue. Immutable bytes survive later file mutation until a new publication; failed replacements never fall back to prior bytes. Whole-generation replacement withdraws old versions/scopes/disabled or omitted rows. Revision exhaustion clears records and refuses byte reads instead of wrapping.

Core producer 只读取 lifecycle-owned explicit Panel entry，要求 canonical root containment、regular file、4 MiB entry 上限、有界读取、exact SHA-256 SRI，以及按 canonical key 顺序、每 record 计费的 64 MiB generation budget。entry 失败或 duplicate key 仅遗漏该 Panel，其余 plugin capability 继续。已准入 bytes 在文件变动后保持 immutable，直到下一次 publication；失败替换不回退旧 bytes。whole-generation replacement 撤回旧版本、旧 scope、disabled/omitted rows；revision 耗尽时清空并拒绝 bytes，不 wrap。

The registry is ID-keyed: one plugin ID does not represent simultaneous installations in multiple projects. Root/scope changes replace that ID's record. Internal `VisibilityCtx` reads are not evidence that an HTTP caller was authenticated or authorized for a project. A future route must derive fresh trusted caller/project context; a client path or bare project ID is insufficient. Explicitly invalid claimed project context must be refused, rather than silently becoming Global access.

registry 以 ID 为 key，同一 ID 不是跨项目并存安装；root/scope 改变是 replacement。内部 VisibilityCtx 读取不证明 HTTP caller 已认证或获准访问 project；后续 route 必须派生可信 caller/project context，不能接受 client path 或裸 project ID。显式但非法的 project claim 必须拒绝，不能静默降级 Global。

## HTTP and browser authentication still required / 仍需实现的 HTTP 与 browser 认证

Existing artifact/canvas/browser-viewer routes show the transport pattern: `resolve_client`, `refuse_insecure_remote`, `OriginPolicy`, and private rate limits. Their surface-specific grants do not authenticate Panel GETs. MCP `authorize` intentionally admits credentialless loopback and is unsuitable for direct reuse here. A future Panel boundary must explicitly require and validate a live shared/device credential plus current user/device status and role, then bind that result to a narrow route state. Broad `GatewaySharedState` must not become a Panel authority.

既有 HTTP route 提供 client/TLS/origin/private rate-limit 模式，但各自 grant 不能认证 Panel GET。MCP authorize 允许无凭据 loopback，不能直接复用；Panel boundary 必须强制 live shared/device credential，校验当前 user/device status 与 role，并绑定窄 route state，不能把 broad GatewaySharedState 变成 Panel authority。

`panel_loader.js` still fetches with `credentials: 'same-origin'` and `redirect: 'error'` without bearer injection. That does not inherit `/ws` authentication or convert localStorage/native-keyring state into HTTP credentials. Host-owned authenticated fetch must reach the existing exact-byte loader without exposing credentials in the module SDK, URL, descriptor or logs. No real credential values were read. Route refusal/origin/TLS/auth tests and real Gateway → browser → explicit `PanelShell` proof remain pending.

loader 的 same-origin fetch 仍没有 bearer 注入，不继承 WS 认证，也不会自动使用 localStorage/native keyring credential。后续 host-owned authenticated fetch 必须接到既有 exact-byte loader，且不向 module SDK、URL、descriptor 或日志暴露 credential。本轮未读取真实 credential；HTTP refusal/origin/TLS/auth 与真实 Gateway/browser/shell 链路仍待验证。

Public route lookup must derive the current declaration/revision/exact entry from Core admission. Client-selected revision, URL, path, manifest fields, generic entry or filesystem roots must not change publication. No endpoint was added to bypass this boundary.

公开 route 查询必须从 Core admission 派生 current declaration/revision/exact entry，client-selected revision、URL、path、manifest fields、generic entry 或 root 均不能改变 publication；没有为绕过边界新增 endpoint。

## Validation and limitations / 验证与限制

The [Core execution ledger](../superpowers/runs/2026-10-31-panel-core-byte-admission.md) records each RED/GREEN command, commit, runtime result and skip. The combined publication acceptance reported 16 passing tests, including six real-manager lifecycle cases; RPC shape tests passed 2/2, Core `cargo check` succeeded, and native modular Panel tests passed 1,602/1,602. The broader extension run reported 883 passing and five failing tests; the ledger records exact names, baseline-source comparisons and the uninitialized plugin-submodule boundary. It is not an all-green extension suite. The Windows escaping-symlink test explicitly returned early because symlink privilege was unavailable; it is not a proved case on this machine.

[Core 台账](../superpowers/runs/2026-10-31-panel-core-byte-admission.md) 保留各项 RED/GREEN、commit、runtime result 与 skip。publication 聚焦验收报告 16 项通过（含六项真实 manager lifecycle）；RPC 2/2、Core check、native modular Panel 1,602/1,602 通过。全 extension 回归是 883 通过、5 失败，具体名称、源码基线对比及未初始化 submodule 边界见台账，不宣称整套全绿。Windows symlink 测试因权限明确提前返回，本机尚未证明该用例。

Historical audit evidence, including the shared hook's baseline formatter failure and non-clean-exit written reviews, remains in the [live-publication ledger](../superpowers/runs/2026-10-31-panel-community-live-publication.md). Historical fixture results remain in [the mount report](panel-community-assets-mount-validation.md); its fixed Node/Bun map is not a Rust lifecycle producer or production Gateway/auth proof. Generated browser artifact tests previously stopped at missing `PANEL_OUTPUT_ROOT`; no other checkout's artifact was borrowed.

历史 audit 的 baseline hook failure、非正常退出书面 review 留在原台账；mount report 的 fixed-map fixture 不能充作 Rust lifecycle producer 或 Gateway/auth proof。browser artifact test 此前因缺少新构建产物的 PANEL_OUTPUT_ROOT 停止，未借用其他 checkout 产物。

Install/update/uninstall, marketplace/signatures, background refresh, live notification, default release, multi-file graphs, external WASM, remote fetch and Zahir runtime mount remain deferred. No merge or push.

install/update/uninstall、marketplace/signatures、background refresh、live notification、default release、多文件 graph、external WASM、remote fetch 与 Zahir runtime mount 仍 deferred；不 merge 或 push。
