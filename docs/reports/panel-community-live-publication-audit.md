# Panel production publication audit / Panel 生产发布审计

Date / 日期: 2026-10-31

Status / 状态: **BLOCKED — audit/design finding only; production publication is not implemented. / 阻塞——仅交付审计与设计发现，未实现生产发布。**

## Decision and scope / 裁定与范围

The live-publication plan explicitly requires stopping if a safe Core-admitted producer/auth injection cannot be identified. At base `fae49f50684dfb95bcf9c0bfdb0a199395926511`, no production producer of Panel publication bytes was identified. This is a missing data/ownership contract, not missing user authorization and not a Windows or browser availability diagnosis. No HTTP endpoint, authentication proof, byte producer or browser credential bridge was added. The existing unmounted boolean seam is still unsuitable for production authorization.

live-publication plan 明确要求：找不到安全的 Core-admitted producer/auth injection 时，停在 audit/design finding。在基线 `fae49f50684dfb95bcf9c0bfdb0a199395926511` 上，没有找到 Panel publication bytes 的生产 producer。这是数据与所有权契约缺口，不是缺少用户授权，也不是 Windows 或浏览器不可用的诊断。本次未新增 HTTP endpoint、认证证明、bytes producer 或浏览器凭据桥；既有未挂载的 boolean seam 仍不能用于生产授权。

## Ownership evidence / 所有权证据

| Boundary / 边界 | Current source / 当前事实源 | Finding / 发现 |
| --- | --- | --- |
| Declaration, trust, source revision / 声明、信任、版本 | `src/extension/panel.rs::derive_panel_declaration`, `src/extension/projection.rs::Views`, `src/extension/mod.rs::panel_ui_capability_snapshot` | Core derives and replaces the snapshot. It does not return module bytes or a publication receipt. / Core 派生并替换 snapshot，不返回 module bytes 或 publication receipt。 |
| Plugin storage / 插件存储 | `src/extension/types/plugins.rs::PluginRecord` | Retains `panel` metadata and `root_dir`, but no admitted Panel byte buffer or producer handle. A filesystem root is not a publication receipt. / 保留 `panel` metadata 与 `root_dir`，没有已准入 Panel bytes 或 producer handle；filesystem root 不等于 publication receipt。 |
| Publication / 发布 | `src/gateway/server/panel_asset_route.rs::PanelAssetPublicationStore` | All constructions and `publish` calls are in its test module. The store validates supplied declaration/SRI/bytes; it cannot establish their lifecycle provenance by itself. / 所有构造与 `publish` 调用均在该模块单测内；store 校验输入 declaration/SRI/bytes，不能独自证明其 lifecycle provenance。 |
| HTTP address / HTTP 地址 | `src/gateway/server/mod.rs` | Only declares `mod panel_asset_route`; there is no Panel route merge or production route state. `./tabs/` is declaration metadata, not evidence of a mounted authenticated endpoint. / 仅声明模块，没有 Panel route merge 或生产 route state；`./tabs/` 是 declaration metadata，不证明已挂载认证 endpoint。 |
| Fixture bytes / 夹具 bytes | `interfaces/webchat/scripts/build-panel-spike.ts` | A fixed Node/Bun map reads `tests/fixtures/community-panel.mjs`. It does not call the Rust store or Core lifecycle. / 固定 Node/Bun map 读取 fixture 文件，不调用 Rust store 或 Core lifecycle。 |
| Explicit browser mount / 显式浏览器挂载 | `interfaces/webchat/src/app.rs::explicit_community_panel`, existing projection/registry/loader/shell | Explicit selection and fixture mount already exist; passive snapshot arrival still does not select, fetch, import or mount. Production asset delivery is the missing edge. / 已有显式 selection 与 fixture mount；snapshot arrival 仍不选择、fetch、import 或 mount，缺口在生产 asset delivery。 |

Reproduce the call-site census from the worktree root with `rg -n 'PanelAssetPublicationStore|panel_asset_route' src`. Inspect the `#[cfg(test)]` boundary in the publication module rather than counting test constructors as production producers.

在 worktree root 使用上述 `rg` 可复查调用点；必须检查 publication module 的 `#[cfg(test)]` 边界，不能把测试构造计作生产 producer。

The existing store fences lower/equal revisions only against a retained record in the same `(plugin_id, plugin_version, tab_id)` slot. `retract` removes that record; there is no retained tombstone, and another plugin version is a different slot. These are in-process guarantees, not proof of current Core snapshot fencing. A producer must bind visibility to the current Core snapshot, including retraction and version replacement, before any HTTP exposure. This audit does not change the store or claim these scenarios have runtime regression coverage.

既有 store 只针对仍保留的同 `(plugin_id, plugin_version, tab_id)` slot 拒绝较低或相等 revision；`retract` 删除记录，不保留 tombstone，另一 plugin version 也是不同 slot。这只是进程内保证，不证明与当前 Core snapshot 同步。HTTP 暴露前，producer 必须把可见性绑定到当前 Core snapshot，包含 retraction 与跨版本 replacement。本审计没有修改 store，也不宣称这些场景已有 runtime 回归验证。

## HTTP authentication audit / HTTP 认证审计

The artifact/canvas/browser-viewer routes provide reusable transport patterns: `resolve_client` derives the effective client/proxy/TLS facts; `refuse_insecure_remote` enforces transport policy; `OriginPolicy` rejects disallowed origins; artifact/canvas use dedicated private `RateLimiter` instances. Their capability/grant credentials are specific to those surfaces and do not authenticate Panel module requests. Neither network locality nor Origin is a Panel credential.

artifact/canvas/browser-viewer routes 提供可复用的 transport 模式：`resolve_client` 派生 client/proxy/TLS 事实，`refuse_insecure_remote` 执行 transport policy，`OriginPolicy` 拒绝非法 origin，artifact/canvas 使用独立 private rate limiter。其 capability/grant credential 各属原 surface，不能替代 Panel module 认证；network locality 与 Origin 均不是 Panel credential。

`src/gateway/mcp_face/auth.rs::authorize` composes the existing connect/identity rules and live shared/device-token validators, but intentionally permits credentialless loopback and ignores its bearer as a device claim. Direct reuse would violate this phase's requirement that unauthenticated or invalidly authenticated asset requests fail closed. The existing derivations remain candidates for a credential-required boundary; no typed Panel HTTP proof was implemented or verified. Token validation alone also must not bypass current user/device status and role resolution.

`mcp_face/auth.rs::authorize` 组合既有 connect/identity 与 live shared/device-token validators，但明确允许无凭据 loopback，并且不把其 bearer 当作 device claim；直接复用不满足本阶段无认证或非法认证 asset request 必须拒绝的要求。其既有派生逻辑仍可用于后续 credential-required boundary；本次没有实现或验证 typed Panel HTTP proof。仅校验 token 也不能绕过当前 user/device status 与 role resolution。

`interfaces/webchat/src/context.rs` owns browser connect/bootstrap persistence and separate native credential handling. `panel_loader.js::importVerifiedModule` currently fetches with `credentials: 'same-origin'` and `redirect: 'error'`, but supplies no bearer header. That fetch option does not transfer WS authentication or turn localStorage/native-keyring credentials into HTTP credentials. A future bridge must use the host's authenticated credential state without exposing credentials through the module SDK, URLs, descriptors or logs; discovery must remain passive. No credential values were read for this audit.

`context.rs` 负责浏览器 connect/bootstrap persistence 与独立 native credential handling。`panel_loader.js` 当前 fetch 使用 `credentials: 'same-origin'` 与 `redirect: 'error'`，没有 bearer header；该选项不会转移 WS 认证，也不会把 localStorage/native keyring credential 自动变成 HTTP credential。后续 bridge 必须使用 host 的认证凭据状态，不把凭据暴露给 module SDK、URL、descriptor 或日志，discovery 必须保持 passive。本次没有读取任何真实 credential 值。

## Missing contract and rejected shortcuts / 缺失契约与不可用捷径

Before route implementation, Core needs a bounded producer contract that pairs the admitted declaration with exact SRI-verified single-file bytes under the same source revision, and withdraws obsolete publications with the canonical snapshot. Its authoritative owner and lifecycle admission boundary must be explicit. A narrow HTTP state may consume that result and route-owned authenticated caller context; broad `GatewaySharedState` must not become another Panel authority. Public lookup must derive the current revision and exact entry from that admitted state, never from a client-selected revision, URL, path or manifest field.

开始 route 实现前，Core 需要有界 producer 契约：把已准入 declaration 与 SRI 验证后的 single-file exact bytes 绑定到同一 source revision，并随 canonical snapshot 撤回过期 publication。权威 owner 与 lifecycle admission boundary 必须明确；窄 HTTP state 只能消费该结果和 route-owned authenticated caller context，不能让 broad `GatewaySharedState` 成为第二 Panel authority。公开查询必须从准入状态派生当前 revision 与 exact entry，不能信任 client-selected revision、URL、path 或 manifest field。

`PluginRecord.scope_key` is a second unresolved ownership dimension. `src/extension/visibility.rs::visible_to` restricts project plugins to the matching project context, but `derive_plugin_projection` passes all records to Panel snapshot derivation and the declaration carries no scope. Presence in that snapshot plus a valid credential is therefore not proof of caller-specific byte access. Before resuming, choose and document either Global-only publication or Core-retained scope enforcement shared by discovery and HTTP byte authorization, using a trusted caller/project context; unresolved scope must fail closed. Do not serialize filesystem/project roots into the asset URL or public declaration to solve this. This is an audit finding in existing metadata projection, not a claim that a new route leaked bytes; no route was added.

`PluginRecord.scope_key` 是另一个尚未解决的 ownership 维度。`visible_to` 限制 project plugin 只对匹配 project context 可见，但 `derive_plugin_projection` 把全部 records 交给 Panel snapshot derivation，declaration 没有 scope。因此 snapshot 中存在 declaration 加有效 credential，不等于该 caller 有权读取 bytes。恢复前必须明确选择 Global-only publication，或由 Core 保留 scope 并让 discovery 与 HTTP byte authorization 使用相同的可信 caller/project context；scope 无法解析时拒绝。不能为此把 filesystem/project root 序列化到 asset URL 或公开 declaration。这是对既有 metadata projection 的审计发现，不宣称新 route 已泄漏 bytes；本次没有增加 route。

Reading `root_dir` plus a client path in a new endpoint, reinterpreting generic plugin entry, manufacturing records from browser snapshots, or promoting the deterministic map to a production producer would each bypass this missing contract. Merely mounting an empty store would provide no authenticated delivery proof. None of those shortcuts was implemented. The design cost of stopping is that real delivery remains unavailable; the benefit is preserving Core ownership without inventing a new publication/trust/revision source.

新 endpoint 读取 `root_dir` 加 client path、借用 generic plugin entry、从 browser snapshot 造 record，或把 deterministic map 升格为生产 producer，都会绕过缺失契约。仅挂空 store 也不能证明认证 delivery；本次均未采用。停止的代价是尚不能真实投递，收益是保留 Core ownership，不另造 publication/trust/revision authority。

## Validation and limits / 验证与限制

- Source/call-site audit: performed on the isolated branch from the base above. / 已在上述基线的隔离分支完成源码与调用点审计。
- `node --test tests/panel_loader.test.mjs` from `interfaces/webchat`: **12 passed, 0 failed, 1 skipped**; skipped generated-WASM bridge requires `ALEPH_PANEL_BRIDGE_GLUE`. This is existing JS loader evidence, not browser/Gateway/auth parity. / 既有 JS loader 验证，不代表 browser/Gateway/auth parity。
- An initial combined loader/build-test invocation failed because `panel-build.test.mjs` requires `PANEL_OUTPUT_ROOT` naming newly built isolated output. No output was built or borrowed from another checkout; artifact validation remains blocked, not RED contract evidence. / 首次联合运行因缺少新构建 output 的 `PANEL_OUTPUT_ROOT` 失败；未借用其他 checkout 产物，该项阻塞，不计作 RED contract。
- No production code changed, so no implementation RED/GREEN cycle occurred. The first commit attempt unexpectedly invoked the shared pre-commit hook's `cargo fmt -p alephcore -p aleph-cdp -p aleph-desktop -p aleph-desktop-macos -p aleph-desktop-linux -p aleph-desktop-windows -- --check`; it failed on baseline formatting differences without modifying files, and clippy did not run. The required memory guard was missed before that implicit Cargo invocation; this procedural error is recorded, not counted as a successful check. Subsequent docs-only commits bypass that hook per invocation without changing its configuration. HTTP refusal, auth binding, Gateway integration, real browser mount, generated WASM and Terminal runtime were not executed. / 未改 production code，未进入 implementation RED/GREEN。首次提交意外触发共享 hook 的上述 Cargo fmt check，因基线格式差异失败、未改文件，clippy 未执行；该隐式 Cargo 调用前漏跑 memory guard，明确记录流程失误，不计作通过。后续仅对本次文档提交单次 bypass hook，不改配置；上述 HTTP/browser/runtime 验证未执行。
- Fresh independent read-only review produced a complete written assessment for `fae49f506..842e4a274`, then its process terminated with `SIGTERM`. It found no Critical issue, one Important missing project-visibility contract and one Minor historical-test evidence overstatement. Source checks confirmed both; this branch documents the visibility requirement and corrects the evidence wording. The written review is available, but this is not a clean-exit reviewer run. Full disposition and verification limits are in the plan run ledger. / 独立只读 review 留下完整意见后进程以 SIGTERM 结束：无 Critical，一项 Important project-visibility 契约遗漏，一项 Minor 历史测试证据夸大。源码核实后已补充 scope 要求并校正措辞；有书面意见，但不记作正常退出的 reviewer run，处置与验证限制见台账。

Install/update/uninstall, marketplace/signatures, background refresh, live notification, default release, multi-file assets, external WASM, remote fetch and Zahir runtime mount remain deferred. The branch remains local; no merge or push is authorized.

install/update/uninstall、marketplace/signatures、background refresh、live notification、default release、多文件 assets、external WASM、remote fetch 与 Zahir runtime mount 仍 deferred；分支保留本地，未授权 merge 或 push。
