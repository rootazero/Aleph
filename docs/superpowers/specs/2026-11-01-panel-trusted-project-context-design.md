# Panel trusted project context design / Panel 可信 Project Context 设计

Date / 日期: 2026-11-01

Status / 状态: **Approved — implementation plan in progress. / 已批准——正在编写 implementation plan。**

Task / 任务: Panel community UI trusted project context, following the production publication phase and the plan in `docs/superpowers/plans/2026-11-01-panel-trusted-project-context-plan.md`.

## Intent and success / 意图与验收

The production Panel publication path currently has one deliberate limitation: both `panel.capabilities.snapshot` and `/tabs/...` use `VisibilityCtx::default()`, so they expose Global records only. This design adds one authenticated, server-owned project selection shared by discovery and asset delivery. The selection chooses a project identifier; it never becomes a second project authority, publication registry, byte store, trust model or revision counter.

当前 production Panel publication 有一个明确限制：`panel.capabilities.snapshot` 与 `/tabs/...` 都使用 `VisibilityCtx::default()`，因此只能暴露 Global records。本设计增加一个经过认证、由服务端持有、供 discovery 与 asset delivery 共用的 project selection。selection 只选择 project identifier，不成为第二份 project authority、publication registry、bytes map、trust model 或 revision counter。

Success means:

- An authenticated user with a stable device identity can explicitly select one project through a typed Panel request.
- The server validates the claim through the existing `ProjectStore` and project authorization path using the verified actor; `authz::project_for(..., actor=None)` is never used for a claimed project.
- The RPC snapshot and HTTP asset route resolve the same server-owned selection and then construct the same Core `VisibilityCtx` from the authoritative project record.
- No selection preserves the existing Global-only behavior.
- An invalid, inaccessible, inactive, cross-user or revoked explicit selection refuses generically and never falls back to Global.
- A valid project selection exposes the Core-visible Global records plus the selected project records; it never exposes another project's records.
- Project switches, retractions, replacements and source revisions cannot publish stale discovery or loader results.
- Existing bearer, identity, origin, TLS, proxy, wall, rate-limit, Terminal and passive-discovery behavior remains intact.

成功标准包括：

- 具有稳定 device identity 的 authenticated user 可以通过类型化 Panel request 显式选择一个 project。
- 服务端通过现有 `ProjectStore` 与 project authorization，使用 verified actor 校验 claim；claimed project 绝不使用 `authz::project_for(..., actor=None)`。
- RPC snapshot 与 HTTP asset route 解析同一个 server-owned selection，再从权威 project record 构造同一个 Core `VisibilityCtx`。
- 没有 selection 时保持现有 Global-only 行为。
- 无效、不可访问、停用、跨用户或已撤销的显式 selection 统一拒绝，绝不回退到 Global。
- project switch、retraction、replacement 与 source revision 不会让旧 discovery 或 loader result 重新生效。
- 既有 bearer、identity、origin、TLS、proxy、wall、rate-limit、Terminal 与 passive discovery 行为保持不变。

## Goals, non-goals and anti-goals / 目标、非目标与反目标

### Goals / 目标

1. Define one typed internal `PanelProjectContext` with an explicit Global state and an explicit project state.
2. Add one server-owned selection store keyed by authenticated device identity, shared by the live RPC registry and production HTTP route.
3. Revalidate the selected project against the current verified actor and `ProjectStore` on every snapshot and asset read.
4. Construct Core visibility only from the authorized internal `Project.workspace_path`; never accept a client root or use daemon CWD as a claimed project.
5. Add RED→GREEN evidence for no selection, valid selection, invalid selection, cross-user isolation, revocation, switching, revision replacement and browser late-result fencing.

1. 定义一个带明确 Global/project 状态的 typed internal `PanelProjectContext`。
2. 增加一个以 authenticated device identity 为 key、由 live RPC registry 与 production HTTP route 共用的 server-owned selection store。
3. 每次 snapshot 与 asset read 都用当前 verified actor 和 `ProjectStore` 重新校验 selected project。
4. 只使用已授权内部 `Project.workspace_path` 构造 Core visibility；不接受 client root，也不把 daemon CWD 当作 claimed project。
5. 为 no selection、valid selection、invalid selection、cross-user isolation、revocation、switching、revision replacement 与 browser late-result fencing 增加 RED→GREEN 证据。

### Non-goals / 非目标

- No install/update/uninstall, marketplace, signatures, default-release switching, live notifications, background refresh, multi-file assets, external WASM, native WebView QA or Zahir runtime mount.
- No persistent project-selection database, project registry, byte map, trust cache or revision authority in Gateway.
- No raw filesystem roots in protocol, browser state, selection state or error responses.
- No project inference from daemon CWD, filesystem root, URL path, query string, arbitrary header, client boolean or chat/session binding.
- No new project-specific credential or lease system.
- No automatic import, mount or fetch when a snapshot arrives.

- 不包含 install/update/uninstall、marketplace、signatures、default-release switching、live notifications、background refresh、多文件 assets、external WASM、native WebView QA 或 Zahir runtime mount。
- Gateway 不新增持久化 project-selection database、project registry、bytes map、trust cache 或 revision authority。
- protocol、browser state、selection state 与 error response 不包含 raw filesystem roots。
- 不从 daemon CWD、filesystem root、URL path、query string、任意 header、client boolean 或 chat/session binding 推断 project。
- 不新增 project-specific credential 或 lease system。
- snapshot 到达时不自动 import、mount 或 fetch。

### Anti-goals / 反目标

A project claim must never become a permissive hint. If the server has an explicit selection but cannot prove that the current authenticated actor can still access it, the request fails. Clearing a selection is an explicit state transition; an authorization failure must not silently clear it and thereby expose Global records on the next request.

Project claim 绝不能成为宽松 hint。如果服务端已有显式 selection，却无法证明当前 authenticated actor 仍可访问它，请求必须失败。清除 selection 是显式状态转换；授权失败不能悄悄清除 selection，否则下一次请求可能暴露 Global records。

## Approaches / 方案比较

### 1. Device-bound server-owned selection — selected / 设备绑定的 server-owned selection——已选择

The browser sends a typed selection request containing `project_id: Option<String>`. The authenticated server resolves `current_caller_verified_user()` at the live dispatch boundary, requires a stable device identity, and checks the project through `ProjectStore` with that actor. The server stores only the selected project ID and a monotonic selection generation in an in-memory selector keyed by the authenticated device identity. The same selector is injected into the RPC handler registry and the HTTP route state.

浏览器发送包含 `project_id: Option<String>` 的类型化 selection request。authenticated server 在 live dispatch boundary 解析 `current_caller_verified_user()`，要求稳定的 device identity，并使用该 actor 通过 `ProjectStore` 检查 project。服务端只在以内存 selector 中保存 selected project ID 与单调递增的 selection generation，key 为 authenticated device identity。同一个 selector 注入 RPC handler registry 与 HTTP route state。

This preserves the existing HTTP bearer model and makes the two ingress paths converge before Core. It costs one shared per-device selection: tabs using the same device see the same selection. That trade-off is deliberate and matches the current webchat global project selection. Browser-local context epochs and loader disposal fence stale work after a switch.

该方案保留现有 HTTP bearer 模型，让两条 ingress path 在进入 Core 前汇合。代价是同一 device 的 tabs 共享 selection；这是有意选择，也符合当前 webchat 全局 project selection。browser-local context epoch 与 loader disposal 负责在 switch 后隔离旧工作。

### 2. Project-bound opaque lease — project-bound opaque lease

The server could issue a short-lived project-specific lease and require both RPC and HTTP to present it. This would allow independent tabs to use different projects, but would add another credential lifecycle, revocation path and bearer interpretation layer. It would make the boundary harder to audit and is excluded from this phase.

服务端可以签发短期 project-specific lease，让 RPC 与 HTTP 都提交该 lease。它可以让不同 tabs 同时使用不同 projects，但会增加另一套 credential lifecycle、revocation path 与 bearer interpretation layer，使边界更难审计，因此本阶段排除。

### 3. Existing session/channel binding — existing session/channel binding

The existing session and room bindings are authoritative for their own session semantics, but Panel discovery and `/tabs/...` requests have no shared session key. Introducing a dedicated Panel session would recreate the selection store with more coupling and would not improve the authorization proof. It is therefore not used.

现有 session 与 room binding 对其自身 session 语义是权威的，但 Panel discovery 与 `/tabs/...` request 没有共同 session key。增加专用 Panel session 会在更强耦合下重新发明 selection store，也不会改善授权证明，因此不采用。

## Ownership and dependency direction / 所有权与依赖方向

Core remains the only authority for Panel declaration admission, immutable bytes, visibility scope and source revision. The new Gateway selector is only a server-side pointer to a project ID plus generation. It cannot create, modify or enumerate Panel records, and it never stores a filesystem root.

Core 继续作为 Panel declaration admission、immutable bytes、visibility scope 与 source revision 的唯一 authority。新的 Gateway selector 只是指向 project ID 与 generation 的服务端指针，不能创建、修改或枚举 Panel records，也不保存 filesystem root。

The selector is created once during server construction and passed by `Arc` into the live handler registry and `PanelAssetRouteState`. It must not be hidden in a separate route-local map or duplicated between RPC and HTTP. The route state remains narrow: existing transport/auth dependencies plus the shared selector and the existing `ExtensionManager` accessor.

selector 在 server construction 时只创建一次，通过 `Arc` 注入 live handler registry 与 `PanelAssetRouteState`。不能在 route-local map 中隐藏它，也不能让 RPC 与 HTTP 各自维护一份。route state 保持窄：既有 transport/auth dependencies，加上 shared selector 与现有 `ExtensionManager` accessor。

`ProjectStore` remains the project authority. The selector store is process-local and non-persistent; restart starts with no selection, which means Global-only. Selection state is not a substitute for roster membership, active status, user identity or project lifecycle state.

`ProjectStore` 继续作为 project authority。selector store 是 process-local、non-persistent；重启后没有 selection，即 Global-only。selection state 不能替代 roster membership、active status、user identity 或 project lifecycle state。

## Typed context and admission / Typed context 与准入

The internal context has two states:

内部 context 有两个状态：

```text
PanelProjectContext::Global
PanelProjectContext::Project { project_id: ProjectId, generation: u64 }
```

`ProjectId` is an opaque logical identifier. `generation` is selector metadata used for browser fencing and diagnostics; it is not the Core publication revision and cannot select an older publication.

`ProjectId` 是 opaque logical identifier。`generation` 是用于 browser fencing 与 diagnostics 的 selector metadata，不是 Core publication revision，也不能选择旧 publication。

The selector key is a stable authenticated device identity. A project selection request is accepted only when all of these are true:

selector key 是稳定的 authenticated device identity。project selection request 只有同时满足以下条件才接受：

1. The request is dispatched under a current verified user whose role is permitted by the existing project authorization rules.
2. A stable device identity is present and belongs to that authenticated identity.
3. `ProjectStore` resolves the requested project with `project_for(store, project_id, Some(verified_user))` or an equivalent shared authorization helper that preserves the same fail-closed semantics.
4. The project is active, accessible under the current roster/owner/admin rules, and has an internal `workspace_path` from which Core project visibility can be constructed without a CWD fallback.

1. request 在当前 verified user 下派发，且该 role 满足既有 project authorization rules。
2. 存在稳定 device identity，且它属于该 authenticated identity。
3. `ProjectStore` 使用 `project_for(store, project_id, Some(verified_user))` 或保持相同 fail-closed 语义的 shared authorization helper 解析 requested project。
4. project 在当前 roster/owner/admin rules 下 active、可访问，并且有 internal `workspace_path`，可以在不使用 CWD fallback 的情况下构造 Core project visibility。

If any check fails, selection is rejected with the existing generic refusal shape. The previous valid selection is not replaced by the failed request. A clear request (`project_id: null`) is allowed only for the authenticated device owner and creates `Global` with a new generation. Requests from implicit owner compatibility identity, credential-less loopback, unbound legacy credentials, missing device identity or stale task-local identity cannot create a project selection.

任一检查失败时，selection 使用既有 generic refusal 形态拒绝。失败 request 不替换之前的有效 selection。只有 authenticated device owner 可以发起 clear request（`project_id: null`），并以新的 generation 创建 `Global`。来自 implicit owner compatibility identity、credential-less loopback、unbound legacy credentials、缺少 device identity 或 stale task-local identity 的 request 不能创建 project selection。

The resolver distinguishes three cases:

resolver 区分三种情况：

- No selection exists: return `VisibilityCtx::default()` and Global behavior.
- A selection exists and current authorization succeeds: construct `VisibilityCtx` from the selected internal `Project.workspace_path`, without serializing the path and without using the CWD fallback. Core's existing visibility contract means Global records remain visible alongside the selected project's records; records from every other project remain hidden.
- A selection exists but the project is missing, inactive, inaccessible, has no usable `workspace_path`, or otherwise fails resolution: return refusal. Do not clear the selection and do not return Global.

- 没有 selection：返回 `VisibilityCtx::default()`，保持 Global 行为。
- selection 存在且当前授权成功：使用 selected internal `Project.workspace_path` 构造 `VisibilityCtx`，不序列化 path，也不使用 CWD fallback。Core 既有 visibility contract 规定 Global records 仍与 selected project records 一起可见；其他 project 的 records 全部隐藏。
- selection 存在但 project 缺失、inactive、不可访问、没有可用 `workspace_path` 或解析失败：返回 refusal。不清除 selection，也不返回 Global。

The same resolver is called separately for snapshot and asset requests. It rechecks the current `ProjectStore` record each time, so revocation, deactivation, roster removal, deletion or user changes take effect without trusting stale selector state.

snapshot 与 asset request 分别调用同一个 resolver。每次都重新检查当前 `ProjectStore` record，因此 revocation、deactivation、roster removal、deletion 或 user change 会生效，不会信任过期 selector state。

## RPC discovery / RPC discovery

Add a typed server-owned selection operation to the authenticated handler path. It is the only operation that changes the selection. The browser project UI invokes it when the global project selection changes and invokes it with `null` when the user explicitly returns to Global.

在 authenticated handler path 增加一个类型化的 server-owned selection operation；只有该 operation 可以改变 selection。browser project UI 在全局 project selection 变化时调用它，用户显式回到 Global 时传 `null`。

Change `panel.capabilities.snapshot` from a contextless Global handler to a context-aware handler invocation. The handler receives a typed request context created by the live dispatch boundary; it does not read caller task-locals after a spawn and does not accept a raw project ID from the snapshot request. It resolves the shared selection, then calls the existing Core `panel_ui_capability_snapshot_for(&VisibilityCtx)` accessor.

将 `panel.capabilities.snapshot` 从无 context 的 Global handler 改为 context-aware handler invocation。handler 接收由 live dispatch boundary 创建的 typed request context；不会在 spawn 后读取 caller task-local，也不接受 snapshot request 中的 raw project ID。它解析 shared selection，再调用现有 Core `panel_ui_capability_snapshot_for(&VisibilityCtx)` accessor。

A snapshot response may carry the selected project ID and selector generation as non-authorizing metadata only if the existing protocol shape needs browser fencing. It must never carry `workspace_path`, and the metadata must not be accepted back as authorization input. If the existing response can fence using the browser-local selection epoch and request generation, keep the wire shape unchanged.

如果 browser fencing 需要，snapshot response 可以携带 selected project ID 与 selector generation 作为不具授权作用的 metadata；不得携带 `workspace_path`，也不得把 metadata 回传后当作授权输入。如果现有 response 通过 browser-local selection epoch 与 request generation 已足够 fencing，则保持 wire shape 不变。

Discovery remains passive. Decoding a snapshot only stores validated declarations. It never fetches, imports, mounts or selects an asset.

discovery 继续保持 passive。解码 snapshot 只保存经过验证的 declarations，不 fetch、import、mount 或选择 asset。

## HTTP asset delivery / HTTP asset delivery

`PanelAssetRouteState` receives the same selector store. After the existing transport, origin, proxy/TLS, wall, rate-limit and explicit bearer checks, `authenticate` produces the private HTTP caller context. The route resolves the selector using that authenticated user/device, then passes the resulting `VisibilityCtx` to both `panel_ui_capability_snapshot_for` and `panel_ui_asset`.

`PanelAssetRouteState` 接收同一个 selector store。现有 transport、origin、proxy/TLS、wall、rate-limit 与 explicit bearer checks 通过后，`authenticate` 生成 private HTTP caller context。route 使用该 authenticated user/device 解析 selector，再把得到的 `VisibilityCtx` 同时传给 `panel_ui_capability_snapshot_for` 与 `panel_ui_asset`。

The canonical URI match, declaration identity derivation, exact current entry lookup and immutable bytes read remain Core-owned. The route does not inspect project roots, build a second map or use a project query/header/path. A selected project that cannot be revalidated returns the existing generic private refusal. Unauthenticated exact built-in assets continue to delegate to `ControlPlaneAssets`; community responses retain `private, no-store`, `nosniff` and `Vary: Authorization, Origin`.

canonical URI matching、declaration identity derivation、exact current entry lookup 与 immutable bytes read 继续由 Core 负责。route 不检查 project roots，不构造第二份 map，也不使用 project query/header/path。selected project 无法重新校验时返回既有 generic private refusal。未认证的 exact built-in assets 继续委托 `ControlPlaneAssets`；community response 保持 `private, no-store`、`nosniff` 与 `Vary: Authorization, Origin`。

An HTTP request that arrives after a project switch observes the current selector. The browser's old request generation and descriptor/disposal fences discard a response belonging to an older local context. The server must not make a failed old selection fall through to Global merely because the route is still reachable.

project switch 后到达的 HTTP request 读取当前 selector。浏览器旧 request generation 与 descriptor/disposal fences 会丢弃属于旧 local context 的 response。服务端不能因为 route 仍可访问，就让失败的旧 selection 回退到 Global。

## Browser selection and lifecycle / Browser selection 与生命周期

The existing `ProjectsTabState::selected_project_id` becomes the source of the explicit Panel selection action. It remains separate from `ChatState::room_project_id`; chat room binding is not used as Panel authorization.

现有 `ProjectsTabState::selected_project_id` 成为显式 Panel selection action 的来源。它继续与 `ChatState::room_project_id` 分离；chat room binding 不用于 Panel authorization。

On selection change, the host:

selection change 时，host：

1. increments a local Panel context epoch;
2. deactivates/disposes the current PanelShell loader and clears active project-scoped projection state;
3. sends the typed server selection request;
4. refreshes discovery only after successful admission;
5. rejects the selection visibly and keeps project-scoped Panel state unavailable if admission fails.

1. 增加 local Panel context epoch；
2. deactivates/disposes 当前 PanelShell loader，并清理 active project-scoped projection state；
3. 发送 typed server selection request；
4. 只有 admission 成功后才 refresh discovery；
5. admission 失败时明确拒绝，并保持 project-scoped Panel state 不可用。

The existing connection epoch, request generation, revision monotonicity, descriptor equality and loader disposal fences remain in force. Add the smallest project-context generation check needed to prevent a response from an earlier selection from changing current projections or activating a descriptor. No project ID or root is added to module/SDK credential arguments.

现有 connection epoch、request generation、revision monotonicity、descriptor equality 与 loader disposal fences 继续生效。只增加防止旧 selection response 改写当前 projection 或激活 descriptor 所需的最小 project-context generation check。不把 project ID 或 root 加入 module/SDK credential arguments。

The server selection is authoritative for the device. Because this phase has no live notification channel, another tab's selection change is observed by this tab on its next discovery or asset request, not through an immediate push. A tab may retain already-rendered state until that next boundary; the existing request, descriptor, revision and disposal fences prevent newly arriving stale work from becoming current. This is an explicit limitation of the shared-device selection trade-off, not an authorization relaxation.

server selection 是该 device 的权威。由于本阶段不增加 live notification channel，另一个 tab 的 selection change 会在本 tab 下一次 discovery 或 asset request 时生效，而不是通过即时 push 同步。tab 可能在下一个边界前保留已渲染 state；现有 request、descriptor、revision 与 disposal fences 会阻止新到达的 stale work 成为 current。这是 shared-device selection trade-off 的明确限制，不是放宽授权。

## Replacement, revocation and revision fencing / 替换、撤销与 revision fencing

Core publication revision remains the only asset source revision. Selector generation is independent metadata. A project switch never selects an older Core revision. A source replacement or retraction produces the existing Core revision behavior; a stale browser snapshot or loader result is rejected by local context, descriptor and revision fences.

Core publication revision 仍是唯一 asset source revision。selector generation 是独立 metadata。project switch 不选择旧 Core revision。source replacement 或 retraction 沿用既有 Core revision behavior；stale browser snapshot 或 loader result 由 local context、descriptor 与 revision fences 拒绝。

When authorization is revoked, the server resolver refuses both snapshot and bytes. The browser drops the active project Panel state when the next discovery or asset boundary observes that refusal and requires an explicit user action to select another project or clear to Global. With no live notification channel, the design does not claim immediate destruction of a Panel that has already loaded and is idle. The resolver must not turn revocation into an implicit Global selection.

授权被撤销时，server resolver 同时拒绝 snapshot 与 bytes。browser 在下一次 discovery 或 asset boundary 观察到 refusal 时丢弃 active project Panel state，要求用户显式选择其他 project 或 clear 到 Global。由于没有 live notification channel，本设计不宣称立即销毁已加载且处于 idle 的 Panel。resolver 不得把 revocation 变成 implicit Global selection。

## Tests and verification / 测试与验证

Each implementation task starts with a meaningful failing test, followed by the smallest production change and a passing verification. Before every Cargo command, run `python qa/browser_live/check_memory.py` from the worktree root and use `CARGO_BUILD_JOBS=1`; run Cargo from the owning package directory. Prefer the independent production-library integration target if the monolithic Core harness reaches the known LLVM OOM.

每个 implementation task 都从有意义的 failing test 开始，再做最小 production change 并验证通过。每次 Cargo command 前从 worktree root 运行 `python qa/browser_live/check_memory.py`，并使用 `CARGO_BUILD_JOBS=1`；Cargo 从 owning package directory 执行。若 monolithic Core harness 遇到已知 LLVM OOM，优先使用 independent production-library integration target。

Required cases / 必需用例：

- No selection returns the existing Global snapshot and Global asset behavior.
- A valid Project A selection returns Global records plus Project A declarations and bytes; Project B is isolated.
- Cross-user, inactive, deleted, revoked and malformed project selections fail generically and never return Global records.
- A loopback compatibility identity, unbound legacy credential, shared token without stable device identity and stale task-local cannot create a project selection.
- The same authenticated device selection is observed by the RPC snapshot and HTTP asset route.
- Project selection failure does not overwrite a previous valid selection; explicit clear is the only Global transition.
- Switching A→B fences snapshot, projection, asset request and late loader results.
- Project record replacement/retraction and Core source revision changes reject stale descriptors and bytes.
- Discovery remains passive.
- Existing authenticated Global route, built-in Terminal delegation, exact `private, no-store` response, SRI bytes, origin/TLS/proxy/wall/rate-limit checks and native Panel/Terminal regressions remain green.
- The real production-library Gateway/browser fixture records fresh evidence for Project A/B isolation and switching. A monolithic test OOM, skipped native WebView or unavailable fixture must be recorded exactly rather than treated as a pass.

- 无 selection 返回现有 Global snapshot 与 Global asset behavior。
- 有效 Project A selection 返回 Global records 与 Project A declarations/bytes；Project B 保持隔离。
- cross-user、inactive、deleted、revoked 与 malformed project selection 都 generic fail，绝不返回 Global records。
- loopback compatibility identity、unbound legacy credential、没有稳定 device identity 的 shared token 与 stale task-local 都不能创建 project selection。
- 同一 authenticated device 的 selection 同时被 RPC snapshot 与 HTTP asset route 观察到。
- selection failure 不覆盖之前的 valid selection；只有显式 clear 才能切换到 Global。
- A→B switch 会 fence snapshot、projection、asset request 与 late loader result。
- project replacement/retraction 与 Core source revision 变化会拒绝 stale descriptor 与 bytes。
- discovery 保持 passive。
- 现有 authenticated Global route、built-in Terminal delegation、exact `private, no-store` response、SRI bytes、origin/TLS/proxy/wall/rate-limit checks 与 native Panel/Terminal regressions 保持 green。
- real production-library Gateway/browser fixture 记录 Project A/B isolation 与 switching 的新证据；monolithic test OOM、native WebView skip 或 fixture 不可用必须如实记录，不能冒称通过。

## Builder latitude / Builder 可决定内容

The implementer may choose exact Rust type names, module placement, lock implementation, protocol field representation and test fixture structure. Those choices must preserve the following decisions: one selector store, one resolver, actor-bound revalidation on every use, no project claim without stable authenticated device identity, no implicit Global fallback after an explicit claim fails, no raw roots, and no second Core authority.

实现者可以决定 Rust type name、module placement、lock implementation、protocol field representation 与 test fixture structure。但必须保留以下决定：一个 selector store、一个 resolver、每次使用都 actor-bound revalidation、没有稳定 authenticated device identity 就不能 claim project、显式 claim 失败后不能 implicit Global fallback、不暴露 raw roots、不增加第二份 Core authority。

## Approval gate / 批准闸门

This document was approved after the builder check and resolved questions. Implementation must follow the separate writing-plans and executing-plans workflows, one RED→GREEN task at a time.

本文档已在 builder check 并处理未决问题后获得批准。实现必须遵循独立的 writing-plans 与 executing-plans workflow，每次只推进一个 RED→GREEN task。
