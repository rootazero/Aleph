# Community Panel production publication validation / 社区 Panel production publication 验证

Status: production-router and real Gateway/Chrome acceptance passed. The generated host uses `panel-modular` without spike/legacy features. This report separates the production ingress from the retained deterministic fixture. / 状态：production router 与真实 Gateway/Chrome 验收通过；新 host 使用非 spike/legacy 的 `panel-modular`，本报告将其与 deterministic fixture 分开记录。

## Ownership and request proof / 归属与请求证明

Core remains the only community declaration, admission, trust, bytes, scope and revision authority. The HTTP module has no community publication map, filesystem producer, manifest parser or revision counter. Its narrow state contains live credential validation, transport/origin policy, a private rate limiter and a current Core accessor. Existing public built-in build artifacts remain under `ControlPlaneAssets`: an uncredentialed request for an exact existing safe public asset delegates to that original owner. It cannot read plugin/Core bytes; authenticated requests always use Core, and unknown community addresses never become SPA HTML.

Core 仍唯一拥有 community declaration、admission、trust、bytes、scope、revision；HTTP 不增加 community map、文件生产者、manifest parser 或 revision。窄 state 只持有 credential、transport/origin、私有限流与 Core accessor。现有公共 built-in 构建产物仍由 `ControlPlaneAssets` 服务：无 credential 且精确命中安全公共资源时委托原 owner，不读取 plugin/Core bytes；带 Authorization 始终走 Core，未知 community 地址不回落 SPA。

| Boundary / 边界 | Production derivation / production 推导 | Validation / 验证 |
| --- | --- | --- |
| Transport | `resolve_client` honors forwarding only through configured proxies; `refuse_insecure_remote` repeats the existing TLS rule | Remote plaintext refusal, TLS acceptance, spoofed forwarding refusal, configured proxy HTTPS |
| Origin | Reject duplicate/malformed Origin/Host, then call the existing `OriginPolicy` | Allowed origin acceptance and invalid-origin generic refusal |
| Credential | Exactly one nonempty bearer; `DeviceTokenManager::validate_device_token`, then the existing live shared-token validator; no bootstrap exchange or credentialless loopback exception | Missing/invalid/duplicate/empty credentials, live device revocation and shared-token rotation |
| Caller | Device ID comes from the validated row; `resolve_connection_identity` and `wall_admits` resolve the current user/role | Bound member identity and deactivated-user refusal |
| Internal proof | Private `PanelHttpAuthContext` fields; only the validator constructs it; the byte-read method consumes it | HTTP requests supply headers, never a boolean or identity assertion |
| Admission | Match one current declaration, derive its complete identity/revision/entry, then call `ExtensionManager::panel_ui_asset` | Exact admitted bytes, stale/replaced/retracted/missing entries, failed SRI re-admission |
| Scope | `VisibilityCtx::default()` on discovery and delivery; Global-only, no CWD or client project inference | Project-scoped Core record cannot be read; project headers/queries refuse |
| Address | Raw `/tabs/...` must equal the admitted entry address; queries and percent-encoded alternatives refuse | No client revision, generic entry, URL, parent path or raw root authority |
| Response | JavaScript MIME, `nosniff`, `private, no-store`; generic 404 without path/existence/credential detail | Reserved `/tabs` routes precede SPA fallback even when dependencies are unavailable |

The private rate limiter allows 120 requests per client IP per 60 seconds, including loopback, independently of RPC budgets. Every HTTP request revalidates the credential. A request already linearized before token revocation or publication withdrawal can finish; there is no claim of retroactive cancellation.

私有 rate limiter 为每个 client IP 每 60 秒 120 次，含 loopback，与 RPC budget 独立；每次 HTTP 都重新校验 credential。已先于撤销边界完成授权/读取的请求可能结束，不宣称可回溯取消。

## Browser delivery and lifecycle / 浏览器 delivery 与生命周期

Explicit `/panel/<plugin_id>/<tab_id>` selection is still required. Snapshot arrival only updates discovery. The existing host bridge supplies its credential to the existing loader after selection; it does not extend the module or `BrowserPanelSdk` ABI with credentials. The URL must share the browser and current Gateway origin. Fetch refuses redirects, disables caching, carries bearer authorization and verifies the exact SHA-256 bytes before Blob import.

仍须显式 `/panel/<plugin_id>/<tab_id>`；snapshot arrival 只更新 discovery。host 在选择后把 credential 传给现有 loader，不扩展 module 或 `BrowserPanelSdk` 的 credential ABI。URL 必须同时与浏览器、当前 Gateway 同源；fetch 拒绝 redirect、禁缓存、携带 bearer，校验 exact SHA-256 bytes 后才 Blob import。

Web credentials come from the existing device/shared credential owner after connect; bootstrap tickets are not asset credentials. Native credentials use the fixed Bridge lease, endpoint checks and protected identity, with no browser-storage fallback. Connection epoch, exact descriptor and SDK disposal are checked before/after the credential handoff and again before import; existing loader generations and one-shot cleanup fence late mounts and unmount failures.

Web 使用既有 connect 后 device/shared credential owner，bootstrap ticket 不是 asset credential。native 使用固定 Bridge lease、endpoint 检查与 protected identity，不回退浏览器存储。credential handoff 前后及 import 前核对 connection epoch、exact descriptor、SDK disposal；既有 loader generation 与一次性 cleanup 继续阻断 late mount/unmount failure。

## Evidence levels / 证据层级

1. Core admission: [Core acceptance ledger](../superpowers/runs/2026-10-31-panel-core-byte-admission.md), including scoped real-file lifecycle proof and the Windows symlink-privilege limitation. / Core admission 证据沿用既有台账，含真实文件/scope/lifecycle 验证与 Windows symlink 权限限制。
2. HTTP integration: real `SecurityStore`, token manager, Core manager and Axum handlers. In-process HTTP tests do not alone prove browser credential propagation. / HTTP 使用真实 store/token/Core/Axum；进程内 HTTP 测试单独不能证明 browser credential propagation。
3. Generated browser host: `build-panel-live.mjs` compiles fresh `panel-modular` WASM without `panel-tab-spike` or `legacy-terminal`, runs wasm-bindgen and records source/WASM hashes in ignored `build.json`. / 新 shell 为 production feature，记录源码与 WASM hash。
4. Gateway/browser: the opt-in `panel_asset_live_gateway_fixture` in `tests/panel_http/live.rs` runs the real `GatewayServer`, actual `/ws` connect/snapshot handlers, current device authentication and the same Core producer. The independent `panel_http_publication` integration target links the regular production library to avoid the monolithic lib-test harness; it adds no public production shim. Test-only additions are generated static shell hosting and local file controls. No fake JSON-RPC endpoint or alternate asset registry is used. / 独立 integration target 链接正常 production library，避开单体 lib-test 编译，不增加公开测试 shim；真实 Gateway fixture 只额外提供新 shell 静态托管与本地文件控制，不伪造 RPC 或 asset registry。
5. Deterministic fixture: the existing `panel-tab-spike` fixed-map fixture remains separate; its disconnected exception is compile-time limited to its exact fixture identity. It is never evidence of production auth. / 原 deterministic fixture 保持独立；断开连接例外仅在编译期 fixture identity 下启用，不作为 production auth 证据。

Actual commands, RED/GREEN results, review disposition and environment limits belong in the [HTTP/browser ledger](../superpowers/runs/2026-10-31-panel-http-publication.md). / 实际命令、RED/GREEN、review 与环境限制见台账。

## Executed acceptance / 已执行验收

| Check / 检查 | Actual result / 实际结果 |
| --- | --- |
| Production-router integration | 2 passed, 0 failed; the opt-in live fixture is ignored in this normal invocation. Real Core/device/shared-token managers and the production router are used. / 2 passed、0 failed，普通调用忽略 opt-in fixture，使用真实 Core/token manager/production router。 |
| Native Panel/Terminal regression | 1603 passed, 0 failed with `--no-default-features --features panel-modular`. / 原生 Panel/Terminal 回归 1603 passed、0 failed。 |
| Fresh WASM/glue loader | 14 passed, 0 failed, 0 skipped; generated glue and source/WASM hashes checked. / 新 WASM/glue 的 loader 14 passed、无失败或跳过，hash 一致。 |
| Real Gateway + Chrome | Driver exit 0; opt-in Gateway fixture 1 passed, 0 failed, exit 0. Actual authenticated `/ws` and `/tabs` traffic reached the Core admission state. / driver 与真实 Gateway fixture 均 exit 0。 |

The browser observed snapshots `(revision, declarations, version)` of `(1,1,1.0.0)`, `(2,0,none)`, `(3,1,2.0.0)`, `(4,1,3.0.0)`, `(5,1,2.0.0)`. Discovery and passive `/panel/` caused no asset fetch. Explicit selection produced six credential-bearing asset requests across the scenarios. Two initial mounts each cleaned up once, including a throwing unmount; duplicate cleanup count stayed zero. A real request held until after disconnect completed without importing or mounting. Retraction refused the prior entry; replacement stayed passive until selection; an admitted module that threw during import and a revoked device credential both failed closed.

浏览器记录上述五代真实 snapshot；discovery 与 `/panel/` 不 fetch。各场景中只有显式选择触发六次带 credential 的 asset request；最初两次 mount 各清理一次，抛错的 unmount 也未重复清理。断连后才完成的真实请求没有 import/mount；retraction 拒绝旧 entry，replacement 在选择前仍 passive，import 抛错及 device 撤销均拒绝。

The browser driver navigates explicit routes through browser History/PopState and observes the existing shell. Its first two runs exposed an anchor-transition wait problem: the shell mounted while the address bar remained pending. The final driver asserts the actual history path rather than unrelated dashboard transition completion. It does not inject a projection, fake JSON-RPC response or asset bytes. Local redacted evidence is retained in `interfaces/webchat/.panel-live/control/browser-report.json`; the temporary credential file is separate and ignored.

driver 通过标准 browser History/PopState 显式导航，观察真实 shell。前两次 anchor transition 的等待在 shell 已 mount 后仍超时；最终使用实际 history path，不等待无关 dashboard transition，也不注入 projection、假 RPC 或 asset bytes。脱敏报告本地保留，credential 文件独立 ignored。

Monolithic Core lib-test code generation hit LLVM OOM in three configurations, so the newly added private unit modules were not runtime-verified. The public-API integration target linked the normal production library and passed; this is the executed HTTP proof. Full workspace tests, native WebView QA and the old deterministic spike browser suite were not rerun in this continuation. Earlier Core-stage baseline failures and the Windows symlink limitation remain recorded in the Core ledger.

三种单体 Core lib-test 编译均 OOM，新增私有 unit modules 没有运行验证；实际 HTTP 证据来自已通过的公开 API integration target。未重跑全 workspace、native WebView 真机或旧 deterministic spike browser suite；Core 阶段既有失败及 Windows symlink 限制仍见旧台账。

## Reproduce / 复现

From `interfaces/webchat`, install the pinned UI tooling with `bun install --frozen-lockfile`, then run `node scripts/build-panel-live.mjs`. `CARGO_TARGET_DIR` may point to a local cache; the script runs the required memory guard from the worktree root before Cargo and records fresh output hashes. Keep Cargo invocations serial.

在 `interfaces/webchat` 安装锁定工具并构建；可通过 `CARGO_TARGET_DIR` 指定本地 cache。脚本在 worktree root 先执行 memory guard，再串行 Cargo，记录新产物 hash。

From the worktree root, run `python qa/browser_live/check_memory.py`, then start the opt-in fixture with `PANEL_LIVE_SHELL=interfaces/webchat/.panel-live/shell PANEL_LIVE_CONTROL=interfaces/webchat/.panel-live/control cargo test -p alephcore --test panel_http_publication panel_asset_live_gateway_fixture -- --ignored --nocapture`. On memory-constrained hosts use `CARGO_PROFILE_TEST_DEBUG=0` and `CARGO_BUILD_JOBS=1` consistently. Wait for the readiness line. Run `PANEL_LIVE_CONTROL=.panel-live/control node scripts/test-panel-live.mjs` from `interfaces/webchat` with Playwright available; `PANEL_PLAYWRIGHT_MODULE` may name an isolated local Playwright module and `PANEL_BROWSER_EXECUTABLE` an installed Chromium/Edge executable. The driver writes a redacted report/screenshot and stops the fixture. `connection.json` contains a temporary test credential and must remain ignored and local.

从 worktree root 先运行 memory guard，再启动上述 opt-in fixture；内存紧张时统一 debug=0/jobs=1。看到 ready 后，在 webchat 运行 browser driver；Playwright 可通过独立本地 module 指定，浏览器可指定本机 Chromium/Edge。driver 写脱敏报告/截图并停止 fixture；含临时测试 credential 的 `connection.json` 必须保持本地 ignored。

## Deferred / 未实施

Trusted project selection for both discovery/delivery, native WebView runtime QA, install/update/uninstall, marketplace/signatures, default-release switching, notifications/background refresh, external WASM, multi-file dependency graphs and Zahir runtime mount remain deferred. `/terminal` and built-in release policy are not changed. No merge or push is part of this delivery.

可信 project selector（须同时覆盖 discovery/delivery）、native WebView 真机 QA、安装/更新/卸载、marketplace/signature、默认 release 切换、通知/后台刷新、external WASM、多文件依赖图、Zahir runtime mount 均 deferred；不改变 `/terminal` 与 built-in release，不合并、不推送。
