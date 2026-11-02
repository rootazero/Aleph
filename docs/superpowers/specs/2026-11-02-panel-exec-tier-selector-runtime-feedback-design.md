# Panel ExecTierPicker runtime feedback and selection reliability design

## Intent / 目标

The composer control shown in the user screenshot is the execution-tier selector (`ExecTierPicker`): the lock icon, “跟随全局” label, and dangerous-command floor warning identify the per-session tool-permission tier. The current source already wires the value through `chat.session_exec_tier`, `sessions.patch`, `session_dials_for_send`, `ChatApi::send`, and `SessionSnapshot`. The remaining product problem is that a runtime failure can make the control appear inert or misleading: the tier catalogue may be loading, empty, refused, or unavailable, and an existing-session persistence failure can leave the local pill showing a choice that the server did not accept.

截图中的控件是执行档位 selector（`ExecTierPicker`）：锁图标、“跟随全局”以及危险命令硬底线说明共同指向 per-session tool-permission tier。当前源码已经把值接到 `chat.session_exec_tier`、`sessions.patch`、`session_dials_for_send`、`ChatApi::send` 和 `SessionSnapshot`。需要修复的是运行时失败时的表现：tier catalogue 可能仍在加载、为空、被拒绝或不可用；已有 session 的持久化失败也可能让本地 pill 显示一个服务端并未接受的选择。

## Goals / 目标

- Make the selector visibly distinguish loading, ready, empty, refused, failed, saving, and saved states.
- Keep “Follow global” as a real selection that clears the session override.
- For an existing session, commit the displayed selection only after `sessions.patch` succeeds; preserve the previous displayed value on failure.
- For a new conversation with no session key, retain the selected value locally so it rides the first `chat.send` through `dials.exec_tier`.
- Provide a retry action when `config.get_tool_permissions` fails or returns no selectable session tiers.
- Surface member/admin refusal through the existing `components::admin_refusal` wording path and use a generic localized error for other failures; never expose raw RPC or backend text in the selector.
- Add deterministic tests for state transitions and the send/persistence contract.

- 让 selector 明确区分 loading、ready、empty、refused、failed、saving 和 saved 状态。
- 保持“跟随全局”是真实选择，并能清除 session override。
- 已有 session 只有在 `sessions.patch` 成功后才提交显示值；失败时保留之前显示的值。
- 没有 session key 的新对话继续在本地保存选择，并通过 `dials.exec_tier` 搭载到第一条 `chat.send`。
- `config.get_tool_permissions` 失败或返回空的 session tiers 时提供 retry。
- member/admin refusal 继续使用 `components::admin_refusal` 的既有文案路径；其他错误使用本地化通用错误，不把 raw RPC/backend text 暴露给用户。
- 为状态迁移以及发送/持久化契约增加确定性测试。

## Non-goals / 非目标

- No new RPC method, wire field, permission tier, authentication behavior, or server-side authorization rule.
- No replacement of `sessions.patch`, `chat.send`, session snapshots, or the shared `session_dials_for_send` rule.
- No new project selector, agent selector, pairing UI, hidden selector, direct Tauri invoke, browser credential storage, fake actor, or test endpoint.
- No claim that this product fix resolves the separate native WebView2 `monitor_auth_failed` boundary.
- No automatic retry loop and no silent fallback from a failed explicit selection to another tier.

- 不新增 RPC method、wire field、权限档位、认证行为或服务端授权规则。
- 不替换 `sessions.patch`、`chat.send`、session snapshot 或共享的 `session_dials_for_send` 规则。
- 不新增 project selector、agent selector、pairing UI、hidden selector、direct Tauri invoke、browser credential storage、fake actor 或 test endpoint。
- 不把这个产品修复宣称为 native WebView2 `monitor_auth_failed` 的解决方案。
- 不做自动重试循环，也不在显式选择失败时静默切换到其他档位。

## Anti-goals / 反目标

The selector must not look selected when a live-session persistence request has failed. It must not disappear merely because a permissions read was refused, and it must not interpret an empty response as “there are no permissions” without showing the reason and a retry path. It must not change the meaning of the dangerous-command floor or allow any tier to disable that floor.

当 live session 的持久化请求失败时，selector 不应继续伪装成已选中。权限读取被拒绝时不能直接消失；返回空结果时不能把它静默解释为“没有权限”，而应显示原因和 retry。不能改变危险命令硬底线的含义，也不能让任何档位关闭该底线。

## Current architecture / 当前架构

`ExecTierPicker` owns the composer pill and reads `ToolPermissionsApi::get_global`. It derives the effective tier from the global response plus `ChatState::session_exec_tier`.

- Existing session: `set_exec_tier` sends `sessions.patch` with `metadata.exec_tier`; the UI currently updates its signal before the asynchronous persistence result is known.
- New conversation: `session_dials_for_send(false, &chat.session_knobs())` carries `exec_tier` on the first `chat.send`.
- Session restore: `SessionSnapshot` and `apply_session_knobs` restore `session_exec_tier`.
- Permission response: `ToolPermissionsResponse::session_tier_presets()` falls back from `session_tiers` to `tiers` for older cores.
- Refusal classification: `components::admin_refusal::is_admin_refusal` and the existing localized read-error wording are already used by neighboring settings surfaces.

`ExecTierPicker` 负责 composer pill，并读取 `ToolPermissionsApi::get_global`。它根据 global response 与 `ChatState::session_exec_tier` 推导 effective tier。

- 已有 session：`set_exec_tier` 用 `metadata.exec_tier` 发送 `sessions.patch`；当前 UI 在异步持久化结果返回前就更新 signal。
- 新对话：`session_dials_for_send(false, &chat.session_knobs())` 把 `exec_tier` 带入第一条 `chat.send`。
- session restore：`SessionSnapshot` 与 `apply_session_knobs` 恢复 `session_exec_tier`。
- permission response：`ToolPermissionsResponse::session_tier_presets()` 对旧 core 从 `session_tiers` 回退到 `tiers`。
- refusal 分类：`components::admin_refusal::is_admin_refusal` 和现有 localized read-error 文案已被相邻 settings surface 使用。

## Approaches / 方案

### A. Runtime state and feedback around the existing picker — selected

Keep the existing transport and state model. Add a small picker-local state model that records catalogue load status and selection persistence status. Make the existing-session selection transactional: persist first, then update the visible session override on success; retain the old value and show a retryable error on failure. Keep new-session selection local because the first message is its persistence boundary. Render a retryable empty/error body instead of hiding the popover.

保留现有 transport 和状态模型。在 picker 内增加小型状态模型，记录 catalogue load 状态和 selection persistence 状态。已有 session 的选择改为事务式：先持久化，成功后再更新显示的 session override；失败时保留旧值并显示可 retry 的错误。新 session 仍先保存在本地，因为第一条消息是它的持久化边界。popover 不再因空/失败而静默隐藏，而是显示可 retry 的内容。

Trade-off: the component gains a few explicit states and localized copy, but the state contract remains local and the server semantics stay unchanged. This is the smallest change that can explain and correct an apparently unwired control.

取舍：组件会增加少量明确状态和本地化文案，但状态契约仍是局部的，服务端语义不变。这是能够解释并修复“看起来没有连线”的最小改动。

### B. Add a shared generic picker state framework

Extract loading/refusal/empty/retry and persistence handling for `ExecTierPicker`, `ModePicker`, and dial pickers into a shared component or state machine.

Trade-off: it reduces duplicated UI later, but broadens the change into several controls before the reported selector’s failure is proven. It also risks erasing the meaningful differences between global permission refusal and per-session persistence.

### C. Replace the selector with a new composer RPC path

Introduce a new selector-specific state or RPC route and bypass the current session metadata/send-dials design.

Trade-off: this would make the implementation visibly different, but it duplicates existing authority and creates a second path for session tier state. It violates the repository’s single-source/session-contract principles and is rejected.

## Selected design / 选定设计

Proceed with Approach A.

The implementation will keep these boundaries:

1. `ToolPermissionsApi::get_global` remains the only catalogue read.
2. `ToolPermissionsResponse::session_tier_presets()` remains the compatibility fallback.
3. Existing-session persistence continues through `set_exec_tier` and `sessions.patch`.
4. New-session selection continues through `ChatState::session_exec_tier` and `session_dials_for_send`.
5. `ChatApi::send` and the server wire shape remain unchanged.
6. The dangerous-command floor remains an informational policy constraint and is never selectable or disableable.

实现采用方案 A，并保持以下边界：

1. `ToolPermissionsApi::get_global` 仍是唯一 catalogue read。
2. `ToolPermissionsResponse::session_tier_presets()` 仍保留兼容回退。
3. 已有 session 仍通过 `set_exec_tier` 和 `sessions.patch` 持久化。
4. 新 session 仍通过 `ChatState::session_exec_tier` 和 `session_dials_for_send` 传递。
5. `ChatApi::send` 和服务端 wire shape 不变。
6. 危险命令硬底线仍是不可关闭的 policy constraint，不成为可选项。

### Proposed state behavior / 状态行为

- `Loading`: show the existing pill but disable tier rows until the catalogue arrives.
- `Ready`: show Follow global plus the server-provided session tier presets.
- `Empty`: keep the pill available and show a localized “no selectable tiers” explanation plus Retry.
- `Refused`: show the existing admin-refusal wording plus Retry where appropriate.
- `Failed`: show a generic localized load failure plus Retry; raw error remains diagnostic-only.
- `Saving`: disable duplicate selection while `sessions.patch` is pending and show a non-ambiguous pending indicator.
- `Saved`: close the popover and update the displayed override only after persistence succeeds.
- `SaveFailed`: keep the previous displayed override, reopen or retain the popover, and show Retry for the same explicit choice.
- New-session selection: update the local override immediately and close the popover; the first-send path remains the authoritative handoff.

- `Loading`：显示现有 pill，但 catalogue 返回前禁用 tier rows。
- `Ready`：显示 Follow global 和服务端提供的 session tier presets。
- `Empty`：保留 pill，显示“没有可选档位”的本地化解释和 Retry。
- `Refused`：显示既有 admin-refusal 文案，并在适用时提供 Retry。
- `Failed`：显示本地化通用 load failure 和 Retry；raw error 只保留在诊断路径。
- `Saving`：`sessions.patch` pending 时禁止重复选择，并显示明确 pending 状态。
- `Saved`：只有持久化成功后才关闭 popover 并更新 displayed override。
- `SaveFailed`：保留之前 displayed override，保留或重新打开 popover，并为同一显式选择提供 Retry。
- 新 session 选择：立即更新 local override 并关闭 popover；第一条消息路径仍是权威交接点。

## Constraints / 约束

- Follow repository dependency direction and existing Leptos/WASM UI ownership.
- Use semantic theme tokens and existing admin-refusal conventions.
- Do not add a dependency.
- Before changing tests, read the applicable `.opencode/skills` test skill and follow its package-specific command requirements.
- Cargo commands require the repository-root memory gate and `CARGO_BUILD_JOBS=1`.
- Native WebView2 validation remains a separate gate; this design does not authorize or require another native launch.

- 遵守仓库依赖方向和现有 Leptos/WASM UI ownership。
- 使用 semantic theme token 和现有 admin-refusal 约定。
- 不新增依赖。
- 修改测试前先读取适用的 `.opencode/skills` test skill，并遵守对应 package 的命令要求。
- Cargo 命令必须先从仓库根运行 memory gate，并使用 `CARGO_BUILD_JOBS=1`。
- native WebView2 验收仍是独立 gate；本设计不授权也不要求新的 native launch。

## Verification / 验证

The implementation plan should add deterministic coverage for:

- loading → ready with session tier fallback;
- empty catalogue remains visible and retryable;
- refusal and transport failure render distinct localized states;
- existing-session selection does not change the displayed override when `sessions.patch` fails;
- existing-session selection commits after `sessions.patch` succeeds;
- new-session selection remains in `session_dials_for_send(false, ...)` and reaches `ChatApi::send`’s `exec_tier` field;
- Follow global clears the override;
- duplicate clicks are ignored while saving;
- Full-tier confirmation behavior and the dangerous-command floor remain unchanged.

实现计划应增加以下确定性覆盖：

- loading → ready 以及 session tier fallback；
- 空 catalogue 保持可见且可 retry；
- refusal 与 transport failure 显示不同的 localized state；
- `sessions.patch` 失败时已有 session 的 displayed override 不改变；
- `sessions.patch` 成功后才提交已有 session 的选择；
- 新 session 的选择仍进入 `session_dials_for_send(false, ...)` 并到达 `ChatApi::send` 的 `exec_tier` 字段；
- Follow global 清除 override；
- saving 时重复点击被忽略；
- Full-tier 二次确认和危险命令硬底线保持不变。

## Builder check decisions / Builder check 决策

The builder review found two behavior decisions and no scope-changing omissions. The decisions are:

1. **Session switch while async work is pending:** discard late results from the old session. Capture the session key and a picker request generation when starting a load or save. Apply the result only if the component is still live and the captured session identity/generation is current. Reset `Saving`, `SaveFailed`, Full confirmation, and selection-pending state when the active session changes. This prevents session A's delayed response from changing session B's visible state.
2. **Retry after a failed Full-tier save:** require the Full confirmation again. A retry is a new explicit attempt and must preserve the existing two-click safety boundary.

The builder also called three implementation-level details that do not change scope: keep Follow global usable in an Empty catalogue; use single-flight or latest-request-wins catalogue refreshes; and guard post-`await` signal writes against component disposal.

builder review 发现两个行为决策，没有发现需要扩大范围的问题：

1. **异步操作 pending 时切换 session：** 丢弃旧 session 的迟到结果。启动 load/save 时捕获 session key 和 picker request generation；只有组件仍存活且 session identity/generation 仍当前时才应用结果。active session 改变时重置 `Saving`、`SaveFailed`、Full confirmation 和 selection-pending 状态，避免 session A 的迟到响应改变 session B 的显示状态。
2. **Full 档位 save 失败后的 Retry：** 必须重新完成 Full confirmation。Retry 是新的显式尝试，应继续保留已有的两次点击安全边界。

builder 还指出三个不改变范围的实现细节：Empty catalogue 中 Follow global 仍可用；catalogue refresh 使用 single-flight 或 latest-request-wins；所有 `await` 后的 signal 写入都要检查组件是否仍存活。

## Builder check status / Builder check 状态

The builder-level review is complete. The user answered both behavior questions above. The design is ready for user approval before implementation.

builder-level review 已完成。用户已回答上述两个行为问题。本设计等待用户批准后才能进入 implementation plan 和代码修改。
