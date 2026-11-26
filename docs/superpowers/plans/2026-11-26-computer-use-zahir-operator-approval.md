# Computer Use Zahir Projection and Operator Approval Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the existing Zahir desktop discovery-to-execution path, and let an operator approve one admitted, verifiably targeted macOS action. / 验证现有 Zahir 投影，并将操作员批准绑定到同一次、目标可验证的 macOS 动作。

**Architecture:** Keep the canonical registry/router, scoped admission, desktop policy, platform boundary and existing operator requester. Carry the admitted handler and lease, absolute deadline, and cancellation into its owned invocation. Only the desktop `Ask` arm prepares, pauses and then revalidates the original action. / 不另建注册表、执行入口或审批系统，只补齐受控调用已有证据和桌面动作闸门之间的连线。

**Tech Stack:** Rust, Tokio, `ToolHandlerRegistry`, `ProjectionHost` / `ZahirFacade`, `ExecApprovalManager`, macOS AppKit limb.

**Spec:** [Computer Use Zahir projection and operator approval design](../specs/2026-11-26-computer-use-zahir-operator-approval-design.md) (`3cd0aa1a9`).

## Global Constraints / 全局约束

- Work in `refactor/computer-use`. No merge, push, new dependencies, public RPC, grant store, approval UI, registry, or harness policy. No live desktop input or OS permissions changes without separate authorization. / 仅在隔离分支，复用现有接口，不擅自操控真机或改权限。
- `Allow` / `Deny`, tool visibility and admission, and current raw MCP/ACP behavior do not change. `Ask` without the canonical, live turn context still refuses. / 不改变现有许可、拒绝与原始路由。
- Eligible only for standalone `click`, `double_click`, `type_text`, `key_combo` on the **actual targeted macOS rail**, with verified process instance and any claimed or coordinate-bearing window. No global input, batch/script leaves, key/button hold or release, AX writes, clipboard, app lifecycle, or other platform enablement. / 只支持已核实目标的独立定向动作；不扩张其他动作。
- Once only, at most 60 seconds and never beyond the authoritative absolute call deadline or any already-known earlier deadline; drop/cancel cleans the pending card. No desktop lock during the human wait. No retry or new grant; an already handed-off operation still settles. / 单次、限时、可取消；等待不占桌面锁、不重放。
- Run `python3 qa/browser_live/check_memory.py` before each Cargo command; serialize Cargo writers using the same target directory. / 每次 Cargo 前通过内存门禁，共用 target 时串行。

## Review Focus / 审阅重点

1. Same name and schema, different handler: reject the original held call; never dispatch the replacement (Task 3).
2. Approved before deadline, then waits on the desktop lock until cancellation or expiry: no native effect after lock release (Task 4).
3. Same PID, new process instance, or same window ID with changed owner or frame: reject after approval, still no native effect (Tasks 2, 4).
4. Named `type_text` on the global rail or eligible-looking batch/script child: no card and no input (Task 4).
5. Two pending requests with sensitive text: one answer only wakes its own call; approved payload is unchanged, but sensitive text stays out of operator records and requester events (Task 4).

### Task 1: Prove the canonical desktop mount / 验证唯一生产投影

**Files:** `src/tools/scoped/production_policy_tests.rs` (fixture); read `src/bin/aleph-server/commands/start/mod.rs`, `src/tools/handlers/builtin.rs`, and `src/capability/projection_host.rs` for composition. Modify startup/registration only if the fixture demonstrates a gap.

**Consumes:** `ProjectionHost::mount`, `wait_until_ready`, `facade`, `register_builtin_routers`, `ToolRegistrationScope::dispose`, and `ScopedToolService`. Build `BuiltinToolRegistry::with_config(...)` under a test-owned temporary configuration, then replace its crate-visible `desktop_tool` with a fake before sharing the registry; exercise `ToolRegistry::execute_tool` on the real registry, not a mock router that simply matches `desktop`.

- [ ] **Write `late_desktop_registration_reaches_scoped_handler_once`:** mount the production-shaped registry before registering built-in routers. Assert no desktop descriptor in the first snapshot, exactly one kind-`Tool` desktop descriptor after registration, and publication reaches the existing subscription within a Tokio timeout. Assert `describe`/`resolve` do not increment the fake platform count; after admission through the production request builder, one safe desktop read increments it to `1`. A denied scope increments it by `0`. Dispose registration, prove new discovery drops the descriptor and the original lease is stale even before a lookup could find a replacement; no second read. Await host shutdown. Use the same real registration helper and Zahir facade as production, not the global slot in parallel tests.
- [ ] **Verify the fixture:** `cargo test --offline -p alephcore --lib late_desktop_registration_reaches_scoped_handler_once`. If already green, commit the proof with no product wiring changes. If red, isolate the missing production edge, repair that edge, then rerun. / 已有接线能通过就不另注册一遍。
- [ ] **Commit:** `test(desktop): prove canonical Zahir discovery and dispatch`.

### Task 2: Supply native process-instance evidence / 提供进程实例证据

**Files:** `desktop/shared/src/system_types.rs`, `desktop/shared/src/traits/system.rs`, `desktop/macos/src/system/{mod,workspace}.rs`; unit tests in the corresponding modules. Reuse the existing `objc2`/AppKit bindings and blocking wrapper, without a new native library or Core OS API.

**Produces:** `DesktopProcessIdentity { pid: i32, bundle_id: String, launch_stamp: u64 }`, `Debug + Clone + PartialEq + Eq`; `SystemCapability::desktop_process_identity(&self, pid: i32) -> Result<Option<DesktopProcessIdentity>>`. The stamp is opaque and comparable, not calendar time to the caller.

- [ ] **Write `desktop_process_identity_requires_live_native_evidence`:** pure tests pin positive PID, same object supplies nonempty bundle ID and finite launch timestamp, terminated or missing metadata → `None`; distinct start stamps for the same PID make the values unequal. The default trait implementation returns `Ok(None)` without looking up apps.
- [ ] **Implement identity lookup:** macOS resolves `NSRunningApplication` by PID and reads PID, bundle ID, termination state and `launchDate` from that object, using the launch date's stable `f64::to_bits()` encoding; reject invalid/absent data or PID mismatch. Query again under the desktop lock, not by name or cached `AppInfo`. The contract reports evidence at read time only, not a cross-syscall guarantee. / 上层不以同名或同 PID 代替进程实例。
- [ ] **Verify:** `cargo test --offline -p aleph-desktop-macos --lib desktop_process_identity` and `cargo check --offline -p alephcore -p aleph-desktop-macos`. Inspect nonzero cases and zero failures. No real desktop query in the tests.
- [ ] **Commit:** `feat(desktop): expose verified process-instance evidence`.

### Task 3: Carry the admitted call and lease / 传递同一次调用及租约

**Files:** `src/tools/scoped/dispatch.rs`, `src/tools/scoped/mod.rs`, `src/tools/registry.rs`, `src/gateway/execution_engine/tool_service_builder.rs`, `src/builtin_tools/desktop/operator_approval.rs` (new private module; mount it from `src/builtin_tools/desktop/mod.rs`). Existing boot installer at `src/bin/aleph-server/commands/start/mod.rs` supplies the concrete operator requester.

**Produces:** an internal `DesktopApprovalInvocation` context with a `tokio::time::Instant` deadline, caller-interest `CancellationToken`, the existing concrete `Arc<OperatorApprovalRequester>`, and the captured registry/handler/backend lease recheck. Its `validate_before_dispatch(&self) -> Result<(), ToolError>` synchronously checks the final deadline, cancellation and original leases; any asynchronous policy/readback belongs before this final check. Its constructor and task-local scope are crate-internal, never exposed as a declarative capability or set from JSON.

- [ ] **Write `desktop_approval_invocation_is_bound_to_capture`:** exercise the actual owned handler task, then remove or replace the exact registry entry with equal schema while it is parked. Pre-dispatch validation must fail even on a same-name replacement; an unrelated registration must not invalidate an unchanged capture. Assert a raw router call and the noncanonical fallback receive no interactive context. A captured call with no live turn/requester also gets none.
- [ ] **Connect context at `route_and_execute` → `invoke_admitted_target`:** pass the same absolute tool deadline already computed by `ScopedToolService`, not a new relative timer. Preserve any earlier existing enclosing deadline/cancel signal. Scope only the admitted canonical `desktop` handler future inside the existing owned task; carry the admitted turn identity and effective tier for this invocation (Tokio task-locals do not automatically cross `spawn`). Do not grant an absent tier or leak context to legacy/sibling paths. Use the existing child drop guard; do not abort an already handed-off native action. Require `ReplayPolicy::Unsafe` to stay on the no-retry route. Reuse the installed concrete config-tier requester through the same slot and inject it into the scoped builder; preserve existing generic `ApprovalRequester` consumers. Do not add a second process-wide requester slot.
- [ ] **Bind final validation to data, not names:** check the original `BackendLease` with `ZahirFacade::validate_lease`, and compare the original canonical entry's handler with the current publication using `Arc::ptr_eq` alongside provider/descriptor checks. Preserve the existing captured handler dispatch, never recapture and invoke a new one after approval. Keep the check private to the desktop approval path; no general authorization refactor. / 保留原 handler，不按名字重新选择。
- [ ] **Verify:** `cargo test --offline -p alephcore --lib desktop_approval_invocation_is_bound_to_capture`; rerun the Task 1 fixture and `cargo check --offline -p alephcore --bin aleph-server`. This task by itself must not change action behavior or start emitting cards.
- [ ] **Commit:** `refactor(desktop): carry the admitted invocation through settlement`.

### Task 4: Resume the approved desktop leaf exactly once / 审批并恢复原叶动作

**Files:** `src/builtin_tools/desktop/{mod,native,operator_approval,tests}.rs`; add cross-layer tests in `src/tools/scoped/production_policy_tests.rs` and reuse operator auth cases in `src/gateway/handlers/exec_approvals.rs` and `src/approval/operator_requester.rs`.

**Consumes:** Task 2 native identity, Task 3 admitted invocation and `OperatorApprovalRequester::request_operator_once(&ApprovalAction, &CancellationToken, tokio::time::Instant) -> ApprovalResponse`.

**Produces:** private `PreparedDesktopAction` (resolved native `DesktopArgs`, targeted `Rail`, `DesktopProcessIdentity`, and optional bound window ID/owner/frame); `DesktopTool::prepare_operator_action(&self, platform: &Arc<dyn DesktopPlatform>, args: &DesktopArgs) -> Result<PreparedDesktopAction, DesktopOutput>`. Use a shared native prepare/dispatch seam rather than calling the normal name resolver again after approval.

- [ ] **Write `desktop_operator_approval` end-to-end cases with a fake platform and actual approval manager:** verify no input before verdict, non-operator denial and `ApproveOnce` exactly one input with the original payload. Make a second request while the first is pending; only the answered call runs. Deny, no route, cancel/drop, timeout, Escape, changed effective policy or leases, same-PID replacement, window owner/frame drift and missing evidence cause zero input. Hold the session lock through the deadline after verdict; freeing it cannot trigger input. While waiting for the operator (before verdict), prove the approval waiter does not own a session lock. Test targeted eligibility, untargeted input and named selectors that actually take the global rail, synthetic platform without an identity, raw entry, batch/script exclusions, and unchanged `Allow`/`Deny`/read path. Put a sentinel secret in the input and assert it is absent from the card, approval record and non-operator frames. Prove success preserves effect and post-observation. / 用实际平台调用计数断言效果，而不只看发卡次数。
- [ ] **Red:** `cargo test --offline -p alephcore --lib desktop_operator_approval`; the eligible-flow fixture gets the old structured `approval_required` and no pending card. Ineligible and `Deny` cases already refuse.
- [ ] **Prepare and request in `Ask` before lock:** preserve the existing hard guards and current approval request semantics. Freeze the effective args, actual targeted rail, process evidence, and any explicit/window-relative owner and frame used to map coordinates. No guessing a window; if the evidence needed by the action is absent, preserve structured refusal. If a locator or window geometry supplies coordinates, revalidate the same proof on resume; never renormalize the action against another window. Do not re-run generic confirmation or reuse its verdict as leaf consent. Keep the existing generic gate in place and refuse interactive approval for a batch/script child. Generate an `ApprovalAction::bare` and set its capped summary from the fixed action name and verified platform identity/window ID only; no args or caller strings. The existing requester owns notification, 60s cap, once-only enforcement and pending-record cleanup.
- [ ] **Resume with the original action:** approved verdict only. Acquire existing session lock; after waiting, check Escape, current hard guards and policy, original rail/identity/window/frame, then `validate_before_dispatch` immediately before handing the prepared payload to the limb. Changed/widened resolution never replaces the prepared intent. Audit the operator decision separately from platform delivery through existing records; no sensitive summary or generic denial prompt added to those records. Keep the existing output/effect and scoped observation path, not a retry, new lease, or broader capture. / 同一次调用、同一个 payload；不允许批准漂移到另一目标。
- [ ] **Green:** rerun `desktop_operator_approval`, `operator_once`, `builtin_tools::desktop`, and `late_desktop_registration_reaches_scoped_handler_once` with `cargo test --offline -p alephcore --lib <filter>`. Require nonzero focused tests and zero failures. Run existing exec approval RPC tests covering trusted operator versus chat and non-once decisions. Use fake desktop only.
- [ ] **Commit:** `feat(desktop): connect operator-once approval to targeted actions`.

### Task 5: Validate the branch and align references / 集成验收与文档校对

- [ ] **Update references:** `docs/reference/FEATURE_LOCATOR.md` (§7 desktop wiring and relevant approval sections), `docs/reference/DESKTOP_BRIDGE.md`, and the approval reference linked from `docs/reference/SECURITY.md` if affected. Correct the prior claim that desktop is not mounted; describe discovery, actual admission and platform reach separately. Document operator-only/once-only, exclusions, absolute deadline and no retried mutation. Preserve the historical first-slice record as a dated artifact; add a supersession pointer if necessary. / 校对当前事实，不重写旧试验记录。
- [ ] **Run integrated gates:** before each Cargo validation run `python3 qa/browser_live/check_memory.py`; run `cargo check --offline -p alephcore --bin aleph-server` and `cargo check --offline -p aleph-desktop-macos`, the focused tests above, `cargo fmt --all --check`, and `git diff --check 6a351411f`. Compare any unrelated formatting failure against the base; do not edit unrelated files or treat a skipped gate as pass. No parallel Cargo writers sharing a target directory.
- [ ] **Independently review the integrated change:** cover the five Review Focus cases, production context injection, absolute deadline, raw fallback, operator Panel versus requesting chat session trust boundary, no retry or public scope widening. Parent verifies evidence, fixes confirmed issues and reruns affected gates. Record results and unrun native/Windows/Linux validations in `docs/superpowers/runs/2026-11-26-computer-use-zahir-operator-approval.md`.
- [ ] **Commit:** `docs(desktop): document Zahir discovery and operator approval`. Confirm clean status in the isolated worktree and unchanged `main`; no push or merge.
