# Native AX references for computer use / 桌面原生 AX 引用实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal / 目标** — Connect AX JSON observation, native reference actions, and independent UI readback on macOS, Windows, and Linux. 三平台接通 AX JSON 观察、原生引用动作和独立回读。

**Architecture / 架构** — Core owns visible tokens, generation and authorization. Platform drivers own bounded native handles and enforce liveness, scope, and affordances at dispatch. The Agent interprets observations and chooses actions. Core 持有可见引用、代次和授权，driver 持有有界的原生句柄并执行派发校验，Agent 决定后续操作。

**Tech stack / 技术栈** — Rust 1.99, Tokio, serde, existing Swift RPC, Windows UI Automation and Linux AT-SPI.

**Spec / 设计** — [Desktop computer use through accessibility references](../specs/2026-11-28-computer-use-ax-references-design.md)

## Global constraints / 全局约束

- Work only on `refactor/computer-use`. Preserve existing Capability names, Core authorization, raw and unattended `Ask` behavior, explicit pointer gestures and role/title locators. Use existing dependencies. 仅在隔离分支工作，复用能力、授权、显式指针和定位器及已有依赖。
- The `token` field remains the public reference and `element` consumes it. A reference action addresses exactly one retained element. Geometric matching, screenshot acquisition, and pointer delivery have no role in native reference dispatch. 原生动作按已保留对象精确派发。
- Preserve the current traversal limits, up to 32 session/PID lanes and 500 returned rows per snapshot. Each platform retains up to 32 snapshots and 500 elements per snapshot with a 90-second monotonic lifetime. Purge at the next use and release on an expiry wakeup or worker shutdown. 核对原有预算，平台租约使用同样的数量界限，90 秒到期。
- PID scope works across the three drivers. Require exact public HWND mapping on Windows. A backend without a provable public mapping returns `window_scope_unsupported` for explicit `window_id`. 跨平台支持 PID 观察，Windows 明确验证 HWND，其他窗口 ID 仅在可证时接受。
- Keep existing application permissions, cancellation, Escape, and the earlier of the invocation deadline and the 60-second approval window. Native ref mutations under `Ask` use the existing Allow Once authority and never gain a standing grant. 保留现有权限与截止时间，单次审批仅用于原调用。

## Review focus / 审阅重点

| Input and expected result / 输入与预期 | Test / 测试 |
| --- | --- |
| Secure node has actionable provider metadata. Its value and native ID never appear in public JSON or approval output. 安全值和原生 ID 不进入回包或审批卡。 | Task 2 `native_snapshot_redacts_value_and_handle` and Task 6 `ax_approval_card_redacts_payload` |
| An older or restarted helper receives a ref action. It cannot execute it as a locator request. 旧 helper 或重启拒绝旧引用。 | Task 3 `ax_ref_bridge_generation_isolation` |
| A returned token is used after another snapshot of the same PID. The stale action performs zero writes. 原范围换代后旧引用零派发。 | Task 2 `native_ref_rejects_previous_generation` |
| Outer timeout occurs while the platform action is still running. The next snapshot cannot claim a current observation before the dispatch settles. 晚完成的动作结束前，同目标快照返回待定状态。 | Task 2 `pending_ax_action_orders_readback` and Task 3 `ax_ref_timeout_reaps_before_readback` |
| Two controls have identical labels, or a control is recycled after approval. Only the original live unchanged object can be acted on. 同名或复用控件不可串目标。 | Tasks 3–6 retain-and-revalidate tests |

## Task 1. Shared typed reference protocol / 共享类型契约

**Files / 文件** — `shared/protocol/src/desktop_bridge/methods/ax.rs`, the method table in `shared/protocol/src/desktop_bridge/methods/mod.rs`, `desktop/shared/src/traits/ax.rs`, `desktop/shared/src/error.rs`.

**Produces / 接口** — `AxSnapshotRequest { pid: Option<i32>, window_id: Option<u64>, max_depth: u32, max_nodes: u32, max_elements: usize }`. `AxReferenceSnapshot { app_pid: i32, window_id: Option<u64>, tree_available: bool, elements: Vec<AxSnapshotRow>, focused: Option<AxElement>, tree_truncated: bool, truncated: bool, nodes_walked: u32, total_interactable: usize }`. Each `AxSnapshotRow { element: AxElement, reference: Option<NativeAxReference> }` is a flat AX element with no children. `NativeAxReference { lease: String, index: u32 }` is an opaque platform-owned address. `AxReferenceAction { reference: NativeAxReference, mutation: AxMutation }` with `AxMutation::SetValue { value: String }` and `AxMutation::Perform { action: String }`. `AccessibilityCapability::snapshot_references(&self, request: AxSnapshotRequest) -> Result<AxReferenceSnapshot>` and `AccessibilityCapability::act_on_reference(&self, action: AxReferenceAction) -> Result<AxActionResult>` are async. Establish typed AX ref refusal reasons for unsupported scope, stale reference, disabled or unsupported mutation, and pending and unknown action outcomes. Carry these as typed platform errors through existing `DesktopError` and the existing macOS JSON-RPC error envelope.

- [ ] Add `ax_reference_wire_roundtrip` asserting large `window_id` serializes losslessly, both mutation variants round-trip, and a typed reference request is never routed to either locator mutation. 测试大窗口 ID 和两类动作。
- [ ] Run `cargo test -p aleph-protocol ax_reference_wire_roundtrip` and confirm the new contract is absent before implementation. Run the same command after implementation and require a pass.
- [ ] Add methods `ax.snapshot_references` and `ax.act_on_reference` with the existing deadline policy. Leave `ax.query_tree`, `ax.set_value`, and `ax.perform_action` wire shapes intact. Default trait implementations return explicit unsupported errors. Add `cargo test -p aleph-desktop --lib` and commit the protocol and shared contract.

## Task 2. Core vertical slice and ownership / Core 连线与所有权

**Files / 文件** — `src/builtin_tools/desktop/{ax,element_ref,interactable,mod,native,types,recovery,tests}.rs`. Keep registry construction at its existing entry in `src/executor/builtin_registry/builder/constructor/mod.rs` and lookup at `src/executor/builtin_registry/registry/tool_registry_impl.rs`.

**Consumes / 输入** — Task 1 `snapshot_references`, `act_on_reference`, and typed refusals.

**Produces / 接口** — `DesktopAxSnapshotArgs::window_id: Option<u64>` and `element_ref::ElementRecord` containing `pid: i32`, `window_id: Option<u64>`, safe role/title metadata, and `reference: NativeAxReference`. Preserve `issue(session: &str, pid: i32, elements: Vec<ElementRecord>) -> u32`, `resolve(session: &str, token: &str) -> Result<ElementRecord, TokenError>`, and the existing token syntax. `native::prepare_reference(session: &str, args: &DesktopArgs) -> Result<PreparedAxReference, DesktopOutput>` validates `element`, explicit selectors, and action/payload shape. `PreparedAxReference` carries the original token and the resolved record and request. Expose stable machine-readable error `data.code` for canonical refusal paths.

- [ ] Add a mock `AccessibilityCapability` behind the real tool calls. In `native_snapshot_redacts_value_and_handle`, assert a no-bounds actionable row has `token` and lacks `center`, value of secure rows, and the opaque handle. In `native_ref_rejects_previous_generation`, assert same-PID generation replacement and cross-session refusal leave the fake write count at zero. In `pending_ax_action_orders_readback`, use a controllable fake delayed action, abort the caller, observe `action_pending` on the target, and assert the next successful snapshot sees the completed effect. Run `cargo test -p alephcore --lib builtin_tools::desktop` before wiring and establish failing assertions.
- [ ] Resolve unscoped foreground PID once using the production platform. On successful native snapshot, reuse the current compact row rendering and safe value filter, omit coordinates when absent, and issue a generation for the published rows. Preserve `count`, `truncated`, `tree_truncated`, `nodes_walked`, `total_interactable`, `app_pid`, and `focused` response semantics as well as the empty-tree response. A completed empty snapshot for a resolved PID replaces that lane's previous generation with an empty one. Scope `focused` to the captured root. Keep `collect_interactable` for geometric workflows. Remove AX snapshot-only coordinate records and `query_by_role` retargeting for AX tokens. Keep explicit locator setters and coordinates. Pointer-only actions with a token return a native action hint; explicit conflicting selectors fail.
- [ ] Serialize publication and action admission in the existing session/PID ref lifetime. Track at most one unresolved native mutation per PID across sessions, with a worker that owns the pending state after caller cancellation. Check generation immediately before synchronous admission into that worker; use an admission lease that is released on completion, not on caller drop. A snapshot checks pending state before and after collection and publishes only if no intervening mutation began. Return `action_pending` for an unresolved overlapping scope without advancing the generation. No lock crosses the approval wait. Unknown-outcome receipts carry the observed PID and no automatic retry.
- [ ] Re-run `cargo test -p alephcore --lib builtin_tools::desktop`, confirm production dispatch performs no screenshot or synthetic input in the mock, and commit the Core slice.

## Task 3. macOS Bridge / macOS 原生连线

**Files / 文件** — `desktop/macos/src/ax.rs`, `desktop/macos/bridge/Sources/AlephBridge/RPC/{AxHandlers,AxSession}.swift`, `desktop/shared/src/bridge/client.rs` if needed, `desktop/macos/bridge/Tests/AlephBridgeTests`, `desktop/macos/tests/computer_use_e2e.rs`.

- [ ] Add `ax_ref_bridge_generation_isolation` using a helper that lacks the new methods and a restarted helper, and `ax_ref_timeout_reaps_before_readback` using a delayed reply. Assert no legacy mutation is sent and no successful same-target snapshot precedes definitive completion or prior helper exit. In the owned GUI fixture, use two same-label buttons and retain the second. After reorder, assert only the chosen button's counter changes. After removing it, assert zero additional invocations. Verify secure redaction and Unicode write readback using existing fixture controls.
- [ ] `AxQuerier` retains `AXUIElement`s as it walks bounded AX children and flattens returned rows. On `ax.act_on_reference`, validate lease, PID, public window identity when available, semantic evidence, and live action or value availability before using the existing native handler body on that object. Reuse `AXUIElement` liveness and role/title reads without a locator search. Return an explicit unsupported result for `window_id` when public identity is unavailable.
- [ ] Route reference actions through the existing bound helper request lifetime in `SwiftBridge::call_bound`. An accepted, well-formed same-request reply settles dispatch. Scope reference leases to the emitting helper generation and reject replay into a replacement helper. For a timeout with an unproven action, let the bound transport retire and reap the exact generation before releasing the pending target. This recovery can interrupt sibling calls on that helper. Its normal success path retains all valid handles. The receiver survives outer call cancellation and never resubmits against a new helper. 复用 helper 回收机制，异常回收可能中断同一 helper 的其他请求。
- [ ] Run `swift test --package-path desktop/macos/bridge`, `cargo test -p aleph-desktop --lib bridge`, and `cargo test -p aleph-desktop-macos --test computer_use_e2e tier_a -- --test-threads=1` on macOS with fixture/TCC prerequisites. Verify actual GUI state and commit.

## Task 4. Windows UI Automation / Windows UIA 连线

**Files / 文件** — `desktop/windows/src/ax.rs`, `desktop/windows/tests/uia_live.rs`. Keep MTA worker and platform handles inside the Windows driver.

- [ ] Add an owned two-window fixture case `uia_ref_selects_second_window_and_rejects_closed_window`. Assert `window_id` selects the exact HWND/PID, mismatch performs no calls, same-title controls in the other window stay unchanged, and closure/recreation invalidates the cached element. Add a worker unit check for expiry on the owning thread.
- [ ] Replace per-call UIA client construction in `run_blocking` with one serialized, long-lived MTA worker. Use bounded commands and return only Rust value results. Keep `IUIAutomation` and live elements on the worker thread through drop. Reuse query/action logic in `imp`; extract the existing mutation bodies to operate on an already selected element. Use `ElementFromHandle`, current process and native window membership for exact roots. Return the selected HWND for the existing PID-only single root. Validate the same UIA RuntimeId or current provider identity when exposed, owner, role/name and supported/enabled patterns before mutation. Hung platform work stays pending until the worker settles.
- [ ] Run `cargo check -p aleph-desktop-windows --all-targets --target x86_64-pc-windows-msvc`, then `cargo test -p aleph-desktop-windows --test uia_live -- --ignored --test-threads=1` on an interactive Windows fixture desktop. Record target build and live effect independently. Commit Windows integration.

## Task 5. Linux AT-SPI / Linux AT-SPI 连线

**Files / 文件** — `desktop/linux/src/ax/{mod,walk,bus}.rs`, `desktop/linux/tests/atspi_live.rs`.

- [ ] Add `atspi_ref_rejects_provider_restart_and_recycled_path` to the live fixture. Assert a fresh PID snapshot captures a writable control with no usable bounds, `set_value` round-trips Unicode, duplicate labels target the retained control, and provider loss and changed semantic identity perform zero writes. Exercise `window_scope_unsupported` for identifiers without a reliable mapping.
- [ ] Capture retained references from the current `Walk`, associating their live proxies with the unique bus owner, object path, process and owning window evidence. Resolve actions on the retained provider object and reuse the native AT-SPI patterns and readback. Bind leases to the connection instance; invalidate when the connection or provider is replaced. Validate current affordances and identity before dispatch. Accept explicit window IDs only when the native backend supplies a verifiable mapping to that same AX root.
- [ ] Run `cargo check -p aleph-desktop-linux --all-targets --target x86_64-unknown-linux-gnu`, then `cargo test -p aleph-desktop-linux --test atspi_live -- --ignored --test-threads=1` with an owned fixture and live AT-SPI bus. Verify actual state, record platform evidence, and commit Linux integration.

## Task 6. Allow Once and registry integration / 单次审批与生产验收

**Files / 文件** — `src/builtin_tools/desktop/{mod,native,operator_approval,tests}.rs`, `src/executor/builtin_registry/{definitions,groups}.rs` as needed for schema descriptions, and `docs/reference/{FEATURE_LOCATOR,DESKTOP_BRIDGE,LINUX_DESKTOP,WINDOWS_RUNTIME}.md` for changed behavior.

- [ ] Add `ax_native_ref_allow_once_rechecks_generation` and `ax_approval_card_redacts_payload`. Assert approved actions write once, policy denial, deadline, changed generation, expired handle, lock contention, Escape or raw/unattended `Ask` write zero times. Assert the approval card carries verified action/target and no value or opaque handle. Assert initial snapshot and post-effect observation both route through the registered Capability paths.
- [ ] Extend `PreparedDesktopAction` capture, card rendering, and `ApprovedDesktopAction` recheck to carry a native AX variant. Keep macOS targeted-input capture as-is. Reuse `DesktopApprovalInvocation::validate_before_dispatch` and the existing approval manager, lock, cancellation, audit and deadline. Bind the frozen request, PID, session and token to the invocation without keeping a desktop lock across the wait. Re-resolve that exact token after approval and consume the same prepared native request using Task 2 admission. Platform validation is the final no-effect guard.
- [ ] Update capability descriptions to show snapshot → `ax_action` with `element` and `ax_action_name`, or `set_value` with `element` and `text` → same PID/window readback. Document supported scope, outcome statuses, approval contract, retained token lifecycle, and bridge compatibility. Update the bilingual §7 locator and desktop reference passages. 清理旧坐标转换说明，记录平台实测覆盖。
- [ ] Run `cargo test -p aleph-protocol --lib`, `cargo test -p aleph-desktop --lib`, `cargo test -p alephcore --lib builtin_tools::desktop`, `cargo check -p alephcore`, and `git diff --check`. Review all patch files for obsolete snapshot-token locator code, duplicated state, and effect claims. Capture platform live or cross-check results, obtain independent integration review and commit the final route and reference updates.

Tasks 3–5 can run concurrently in separate writer worktrees after the shared contracts are pinned. The main agent owns Core wiring, integration, and accepted diffs. Use a fresh independent reviewer for cross-platform contract and approval invariants, then integrate each platform against the same committed shared contract. 第 3–5 项在共用契约固定后可隔离并行，Core 及最终验收由主 Agent 负责。
