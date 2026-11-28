# Desktop computer use through accessibility references / 通过辅助功能引用操作桌面

## Intent / 目标

An Agent observes a desktop application as AX JSON, selects an actionable `ref`, calls an advertised native action or sets a value, then reads the same UI scope to decide what to do next. The normal path works with no screenshot, pixel comparison, or element geometry. The Agent owns the observe–act–verify loop.

Agent 读取桌面应用的 AX JSON，从中选择可操作的 `ref`，执行控件公开的原生动作或设置值，再读取同一界面范围并决定下一步。主路径在没有截图、像素比较或控件矩形时也可工作。观察、行动和验证的循环由 Agent 驱动。

Keep the existing Desktop Capability catalog, Core authorization and cancellation, `DesktopPlatform::ax()`, and each platform driver. Screenshot, SOM, and explicit pointer and keyboard inputs remain available through their existing Capability routes for workflows that request them. This change concerns the accessibility path from snapshot to observed effect. Batch policy, OS-wide input coordination, a new action engine, and general-purpose backend rewrites are outside its scope.

复用现有 Desktop Capability、Core 授权与取消机制、`DesktopPlatform::ax()` 及各平台 driver。需要截图、SOM 或显式指针和键盘输入的流程仍走对应的 Capability。重构覆盖从辅助功能快照到效果观察的主路径，批处理策略、系统全局输入协调和通用动作引擎另行处理。

## Evidence / 对照依据

| Project / 项目 | Evidence and application / 证据与取舍 |
| --- | --- |
| agent-desktop | `README.md` and `crates/core/src/refs/{ref_entry_identity,resolve_saved_ref}.rs` show agent-owned iteration and rejection of stale or ambiguous refs. Adopt that user-facing loop and failure contract. / 采用 Agent 驱动的循环与明确的引用失效语义。 |
| cua | `libs/cua-driver/docs/macos-background-input-v1-plan.md` and `libs/cua-driver/rust/crates/platform-macos/src/ax/{exact_target,snapshot}.rs` carry exact window identity across action and observation and retain native AX elements. / 借鉴窗口身份贯穿观察和动作，以及保留原生 AX 元素的思路。 |
| open-codex-computer-use | `docs/linux-action-read-back.md` distinguishes action dispatch from UI outcome and describes an effect-asserting GTK fixture. `packages/OpenComputerUseKit/Sources/OpenComputerUseKit/{TargetedQuery,ElementIdentity}.swift` show bounded handle retention and live identity checks. / 借鉴独立回读、有界保留和实时身份校验。 |
| orca | `native/computer-use-macos/Sources/OrcaComputerUseMacOSCore/AccessibilityWindowIdentity.swift` gets window identity from native AX objects. / 窗口身份由平台端提供。 |
| lcu | `lcu/session.mjs` attaches to an account-owned desktop session. / 以当前桌面会话作为原生对象的生命周期边界。 |
| Aleph | `src/builtin_tools/desktop/{ax,interactable,element_ref,native}.rs` already connect observation and actions to Core. AX snapshots currently require bounds, and a ref becomes a soft role/title/center locator for native actions. / 保留已有入口，补齐无几何观察与精确的原生动作连线。 |

## Approaches / 方案

1. **Retained native elements, selected by the user.** The platform driver retains each published object or provider reference for a bounded lease. It checks live identity and scope on the same object before dispatch. This handles providers that expose actions without a stable search identifier. It adds platform-specific ownership rules and lifetime management.
2. **Re-resolve by exact evidence.** Store a fingerprint and allow dispatch only when a stable identity matches uniquely in a live walk. This reduces retained state but omits providers with insufficient identity data and repeats an AX traversal per action.

1. **用户选定保留原生元素。** Driver 对已发布的原生对象或 provider 代理做有界保留；执行前对同一对象检查存活状态、身份和窗口范围。可覆盖无稳定搜索 ID 的控件，代价是平台侧需要处理对象归属与寿命。
2. **按身份重新查找。** 保存指纹，仅当实时遍历中有唯一且可证明身份的目标时执行。Driver 状态更少，但能力覆盖更窄，每次动作都要额外遍历。

## Snapshot, scope, and actions / 快照、范围与动作

- Extend `desktop_ax_snapshot` with optional `window_id` using the unsigned 64-bit ID from Aleph's window list. Accept it only where a platform can establish an exact AX root and validate an accompanying PID. For the first release, require a working HWND mapping on Windows. macOS and Linux must support PID-scoped snapshots and native ref actions. An explicit window ID on those platforms requires a proven public provider mapping, otherwise return `window_scope_unsupported`. Use public system APIs and provider identity for this path. With neither selector, resolve the foreground app once at snapshot start. Windows keeps its current single UIA root for a PID and returns the chosen HWND so the Agent can pin it on readback. Other platforms return the observed PID, and the confirmed window ID when applicable.
- Reuse the current traversal budgets, `max_elements` cap, truncation indicators, compact rows, and secure-value filtering. The existing JSON `token` field carries the native `ref`. A row with a native action or writable value can be actionable without bounds. Emit `center` only when available. Include enabled state and advertised affordances. Publish a native `ref` only if the retained object is safely addressable and at least one enabled action or supported value mutation is available under the platform's existing checks. Observation-only rows can still carry useful roles and labels, subject to the same projection budget.
- Use the existing `desktop` action values `ax_action` with `element` and an advertised `ax_action_name`, or `set_value` with `element` and the requested value in `text`. macOS and Windows can advertise `AXPress`; Linux can advertise the native AT-SPI `click` action. Preserve value setter semantics and platform readback behavior. A press or invocation receipt records dispatch. The Agent observes again to verify application behavior. Existing coordinate and SOM inputs serve explicit pointer gestures. An AX snapshot ref used with a pointer-only verb returns a structured error with the supported action guidance. Existing role/title actions without a ref and existing visual ref behavior retain their current contracts.

- `desktop_ax_snapshot` 增加可选 `window_id`，沿用窗口列表的 64 位无符号 ID。仅当平台能确定精确 AX 根且给定 PID 与实际所属进程一致时接受该参数。首轮要求 Windows 支持 HWND 精确映射，macOS 和 Linux 支持按 PID 观察和执行原生引用动作。后两者只有具备可证的公开 provider 映射才接受显式窗口 ID，否则返回 `window_scope_unsupported`。此路径采用公开系统 API 和 provider 身份。两个选择器均省略则在快照开始时解析一次前台应用。Windows 保留已有的单 UIA 根行为，并返回所选 HWND 供 Agent 固定窗口回读。其他平台返回实际 PID，适用时同时返回可证的窗口 ID。
- 复用现有遍历预算、`max_elements` 上限、截断标志、紧凑 JSON 和安全字段过滤。原生 `ref` 沿用现有 JSON `token` 字段。控件无需矩形即可公开原生动作或写值，仅在存在可用几何信息时输出 `center`。快照公开启用状态和动作。可安全保留且拥有可执行动作或支持写值的控件获得原生 `ref`。只读行可在同一输出预算内保留有意义的角色与标签。
- 通过现有 `desktop` 的 `ax_action` 与 `set_value` 消费引用。`set_value` 继续从 `text` 读取要写入的值。`ax_action_name` 使用平台快照公开的动作名，例如 macOS 和 Windows 的 `AXPress` 或 Linux 的 AT-SPI `click`。保留现有写值语义和平台回读。按钮调用回执描述派发，Agent 随后独立观察界面效果。指针手势继续使用显式坐标或 SOM，向仅支持指针的动词传入原生 `ref` 时给出结构化错误和动作提示。显式 role/title 和视觉引用维持原有契约。

```text
desktop_ax_snapshot { pid: 42, window_id: 8 }
  → { app_pid: 42, window_id: 8, elements: [{ token: "…", role: "AXButton", actions: ["AXPress"] }] }
desktop { action: "ax_action", element: "…", ax_action_name: "AXPress" }
  → dispatch receipt
desktop_ax_snapshot { pid: 42, window_id: 8 }
  → updated AX state
```

## Ownership and failure boundaries / 所有权与错误边界

- Core's reference store still owns model-facing tokens, session binding, and generation. A successful application snapshot supersedes refs for its session and PID, including refs from earlier scoped windows of that PID. Preserve the existing session/PID lane behavior so changing apps does not by itself invalidate another app's refs. Failed snapshots do not publish a partial generation. Serialize publication and mutation per session/PID lane so an action admitted after publication cannot execute with a prior generation. Calls to independent lanes can continue through the platform's existing execution context.
- Only a typed snapshot reference request can create a native lease. A typed reference action must address that lease explicitly. Use additive bridge methods or an equally explicit wire discrimination that an older helper rejects. Existing locator-only wire requests keep their current behavior. Bind handles to the platform instance and issuing snapshot. Stale, evicted, expired, wrong-session, wrong-window, unsupported, or changed targets fail before mutation. A Bridge restart invalidates its leases. Report recoverable pre-dispatch errors with guidance to take a fresh snapshot.
- The default `Ask` decision for AX writes is part of the release path. Extend the existing Core Allow Once flow for canonical, attended `ax_action` and `set_value` calls with a native ref on all three platforms, subject to existing eligibility checks. Before waiting, bind the frozen verb, payload, session, reference generation and provider lease to the original invocation. The operator card uses the verified app, scope and action and redacts sensitive values. Release the desktop lock during the wait. On approval, acquire the existing lock, validate the policy and invocation lease, cancellation, Escape, and current ref generation, then let the platform revalidate the native object immediately before dispatch. Denial, expiry, replacement during approval or a busy lock prevents dispatch. Preserve the current cutoff and audit behavior. Raw and unattended calls under `Ask` continue to return approval required.
- A timed-out or cancelled action that has been handed off can still affect the app. Keep completion attached to the underlying platform request even when the originating tool call returns. While the action may still dispatch or is still executing, reject reference mutations and report `action_pending` for an AX snapshot of the affected target, using the process as the conservative scope when window ownership cannot be established. Once the worker confirms completion or that it cannot dispatch later, resume observation and mark the previous outcome unknown when there is no receipt. The Agent re-observes before deciding whether to issue another action. Completion is a platform execution boundary. App updates may still settle asynchronously, so a fresh snapshot or existing state predicate verifies the intended effect.
- Reuse the Bridge AX session on macOS and retain AT-SPI proxies on their owning connection on Linux. On Windows, existing `run_blocking` calls create a UIA client per call and therefore cannot own retained references. Move their serialized work to one long-lived MTA worker that owns the UIA client and elements, reusing the current query and action implementations. Validate PID, owning window when present, provider liveness, enabled state, and requested affordance on the retained object before acting. Use provider identity when exposed and capture available role and semantic label evidence to reject recycled controls. Mutable values and geometry are not identity keys. A missing or changed precondition produces a no-effect response. Providers can recycle an object with indistinguishable identity data. As with other native APIs the lease establishes object identity and rejects detectable reuse; it cannot prove an app's invisible data model identity.
- Bound the platform store by retained elements and snapshot count, with expiry even without explicit release from Core. Existing Core limits are 32 session/PID lanes and 500 published rows per AX snapshot; the platform lifetime should be short and testable, with exact sizing and expiry chosen during planning. Stale/evicted leases fail closed. Cleanup must release objects on their owning execution context. Secure contents and native handles must stay out of the model-facing response and user-visible diagnostics. Keep the existing desktop permission, Escape and approval checks at their current boundaries.

- Core 引用存储继续负责模型可见 token、会话和代次。成功快照会替换同一会话、同一 PID 的旧代次，包括该 PID 其他窗口的快照。切换到不同 PID 不会自动使旧应用引用失效。失败快照不发布半截代次。同一会话与 PID 的发布和动作要串行化，避免旧代次在新代次发布后通过；独立范围仍使用各平台既有执行上下文。
- 由显式类型化请求创建、使用原生句柄租约。共享协议采用旧 helper 会拒绝的显式方法或分支，老式定位器请求仍维持既有行为。句柄绑定平台实例及签发快照。会话、窗口、平台实例、代次或租约状态不符时在调用系统动作前失败。Bridge 重启使旧租约失效。派发前的可恢复错误提示重新读取快照。
- 默认 `Ask` 涵盖在首轮主路径中。在现有资格检查下，三平台的规范、有人值守调用可为带原生引用的 `ax_action` 或 `set_value` 使用 Core 的 Allow Once。等待前将冻结的动作、参数、会话、引用代次与 provider 租约绑定原调用。审批卡显示已验证的应用、范围与动作，对敏感值脱敏。等待期间不持桌面锁。批准后获取已有锁，复核策略与调用租约、取消状态、Escape、引用代次，再由平台在派发前复核原生元素。拒绝、过期、等待时引用换代或锁被占用均阻止派发。沿用现有截止时间与审计。原始调用和无人值守入口在 `Ask` 下继续返回需要审批。
- 已交给平台的动作即使超时或取消，也可能继续产生影响。原工具调用返回后，继续跟踪底层请求的结束。动作仍可派发或尚在执行时，同一目标的原生引用动作被拒绝，AX 快照返回 `action_pending`。无法确认窗口归属时，以进程为范围。worker 确认动作已结束或今后无法派发后恢复观察，缺少回执时标记前次结果未知。由 Agent 重新观察后决定是否再操作。应用仍可能异步更新，实际效果以新快照或现有状态谓词验证。
- macOS 复用 Bridge AX 会话，Linux 保留 AT-SPI 连接上的对象代理。Windows 的 `run_blocking` 每次调用创建 UIA 客户端，将其串行工作交给一个长寿命 MTA worker 以持有客户端和元素，查询和动作逻辑继续复用。执行前在同一对象上校验 PID、窗口归属、provider 存活、启用状态和动作支持。使用平台公开的身份信息及角色、语义标签辅助识别复用控件。几何和可变值不参与身份比较。原生对象可证明自身身份，应用若用不可区分的身份数据复用对象，Bridge 无法窥见应用内部数据身份。
- 平台有界保留对象，支持从未发起释放请求时的自然过期。按现有 Core 32 个会话与 PID 范围、快照最多 500 行的约束规划平台容量和时间窗口。缓存淘汰和超期引用返回失效，句柄释放留在原所属执行上下文。模型回包与用户可见诊断不公开原生句柄或安全字段内容。保留当前桌面权限、Escape 和审批边界。

## Connection-first rollout and evidence / 连线顺序与验收

1. Trace the production Capability routes and pin a mock-backed vertical slice through the real `DesktopAxSnapshot` and `DesktopTool` entry points. Reuse their budget handling, typed target validation, authorization, errors, and recovery hints. Define the narrow reference protocol shared by the DBus/UIA/Swift implementations and reject unsupported bridge versions explicitly.
2. Connect the new reference target to the existing native setters and action implementations on macOS, Windows, and Linux. Keep the verified native handler code shared between locator and reference paths. Introduce bounded platform retention at the existing observations and release references when their owning lifetime ends. Avoid repeated tree walks on the semantic action path. Keep geometry-based resolution confined to visual workflows where it is part of their contract.
3. Add exact window-scoped observation, complete capability descriptions and errors, and remove AX snapshot-only conversions that inject coordinates or guess a target. The platform-neutral contract and native application or window resolution should each have a single owner. Refresh `FEATURE_LOCATOR.md` §7 and `DESKTOP_BRIDGE.md`; align Linux and Windows reference passages where behavior changes.
4. Exercise the production entry points with elements that have no bounds, duplicate labels, reordering, disappearance, wrong session/PID/window, stale generations, disabled affordances, expiry and eviction, secure fields, concurrent re-snapshot and actions, and an older or restarted Bridge. Exercise Allow Once, denial and generation change during approval, plus a timed-out action whose late completion precedes the next successful same-target observation. Assert dispatch counts and actual effect separately. Reuse `desktop/macos/tests/computer_use_e2e.rs`, `desktop/windows/tests/uia_live.rs` and `desktop/linux/tests/atspi_live.rs`. Record available live fixture evidence for each platform and any prerequisites.

1. 先在真实 `DesktopAxSnapshot` 与 `DesktopTool` 入口用 mock driver 确认端到端接线。复用预算、授权、目标校验和错误恢复提示，补齐 DBus、UIA 与 Swift 共用的引用协议。
2. 将原生引用接到三平台已有的写值和动作实现，共享既有调用与回读逻辑。在现有观察过程中保留原生对象，所有者生命周期结束时释放。语义动作避免额外遍历，依赖几何的定位仅在视觉工作流使用。
3. 增加精确窗口观察，更新 Capability 说明和错误提示，清理 AX 快照引用注入坐标和宽松重定位的旧分支。校对 `FEATURE_LOCATOR.md` §7、`DESKTOP_BRIDGE.md` 和受影响的平台参考文档。
4. 用相同入口验证无矩形控件、同名控件重排、目标消失、跨会话或窗口引用、过期与淘汰、安全字段、禁用控件、并发刷新和旧 Bridge。验证单次批准、拒绝、等待审批时引用换代，以及动作超时后晚于工具调用结束、早于同目标快照恢复。分别断言派发次数和界面实际效果，复用各平台原生夹具并记录验证覆盖。
