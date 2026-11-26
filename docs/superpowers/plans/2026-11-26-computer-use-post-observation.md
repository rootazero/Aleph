# Computer Use Scoped Post-Observation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把动作后观测绑定到动作的实际投递对象；证明不了目标时，明确返回不可用。 / Bind post-observation to the dispatched action's subject; report unavailable when it cannot be proven.

**Architecture:** Rust Core keeps approval and dispatch and passes a typed subject to its existing observer. The macOS limb adds an owner-checked RPC that validates and captures the same window object. The standalone screenshot path and existing image pipeline remain the shared implementation for capture processing. / Core 保留审批与分发，观测消费类型化目标；macOS Bridge 验证并捕获同一个窗口对象；独立截图和后置截图复用现有图像处理。

**Tech Stack:** Rust, Tokio, Serde, Swift, ScreenCaptureKit, existing macOS Bridge stdio RPC.

**Spec:** [2026-11-26-computer-use-post-observation-design.md](../specs/2026-11-26-computer-use-post-observation-design.md) (`86cc8873b`).

## Global Constraints / 全局约束

- Only work in `refactor/computer-use`; no changes to `main`, no new dependencies, registry or runtime. / 仅在隔离分支工作，不引入依赖、新注册表或运行时。
- Preserve approval, lock, Escape, leaf eligibility, batch inheritance/stop, and public tool arguments. `verify_state` and action effects do not change. / 不改审批、锁、Escape、观测资格、批次语义、公开参数、验证及效果分级。
- Preserve `300 ms` settle, `200`-character focus-value limit with secure-value redaction, and `1568` post-image max width. Window capture excludes the cursor. / 保留等待、脱敏、字符与图像尺寸上限，窗口捕获不含指针。
- No broadening of a scoped read; no retried mutation; no window guess based on z-order, active focus or app name. / 定向读取不扩大范围、不重放动作、不猜窗口。
- No Zahir projection, interactive approval, or new Windows/Linux backend. No live desktop actions or OS permissions changes without authorization. / 不接 Zahir 或交互审批，不新增其他平台的所有者截图，不擅自操控真机或改权限。

## Review Focus / 审阅重点

1. **Explicit window conflicts with the actual PID or disappears** → the bridge never calls the capture function with that candidate (Tasks 1–2). / 窗口归属冲突或已关闭，不能捕获。
2. **Legacy helper rejects the new method** → exactly one dedicated RPC, no retry through `screen.capture` (Task 1). / 旧 helper 不触发兼容回退。
3. **AX backend supplies someone else's focused element** → no element payload reaches the model (Task 2). / 后端返回其他进程焦点时不暴露内容。
4. **A process/window selector is present on global delivery** → no global fallback, even when the mutation itself succeeded (Task 2). / 输入实际上全局投递，也不冒充目标回执。
5. **Batch succeeds on one item then fails** → no observation on the failed item, no later action, and preserved `partial` grade (Task 2). / 批次失败项无观测，后续不执行，部分效果等级不变。

---

### Task 1: Add and prove owned-window capture / 所有者约束的窗口捕获

**Files:** Modify `shared/protocol/src/desktop_bridge/methods/screen.rs` (wire schema), `desktop/shared/src/traits/screen.rs` (limb contract), `desktop/macos/src/screen.rs` (RPC client), `desktop/macos/bridge/Sources/AlephBridge/RPC/ScreenCaptureHandlers.swift` (selection and capture). Test in the protocol module and `desktop/macos/bridge/Tests/AlephBridgeTests/OwnedWindowCaptureTests.swift`; put the Rust fake Bridge RPC test in the macOS screen module or its existing integration fixture.

**Produces:** `METHOD_CAPTURE_OWNED_WINDOW = "screen.capture_owned_window"`; `OwnedWindowCaptureParams { window_id: u64, expected_pid: i32 }` with existing wire `CaptureResult`. `ScreenCapability::screenshot_owned_window(&self, window_id: u64, expected_pid: i32) -> Result<WindowShot>` returns unsupported by default without invoking ordinary capture. The wire accepts only a nonzero `CGWindowID` and positive `Int32` PID.

- [ ] **Regression assertions:** protocol round-trip equals `{ "window_id": 900, "expected_pid": 4242 }`, and a missing field is not accepted. Bridge test supplies absent, fractional, negative, zero and out-of-range operands and asserts invalid params before capture access. Synthetic window selection tests assert only `(900, Some(4242))` can lead to capture with requested `(900, 4242)`; absent window, no owner, or another owner do not invoke capture. macOS fake RPC returns method-not-found and observes exactly `['screen.capture_owned_window']` in the request log. Never query a real desktop for these tests.
- [ ] **Red:** `python3 qa/browser_live/check_memory.py && cargo test --offline -p aleph-protocol --lib owned_window`; `swift test --package-path desktop/macos/bridge --filter OwnedWindowCaptureTests`. Confirm assertions fail against the missing contract, not a fixture/environment failure. Stage test code with its implementation rather than leaving a red commit.
- [ ] **Implement contract:** make the default trait method consistently unsupported; encode the dedicated RPC and share the `CaptureResult -> WindowShot` conversion with `screenshot_window`. Do not retry via `METHOD_CAPTURE`. In Swift, register the new method beside the legacy handler, use strict decoding and no cursor, and check `owningApplication?.processID` on the exact object used to construct `SCContentFilter(desktopIndependentWindow:)`. Reuse existing capture functions and preserve legacy validation and output. Never reuse a general decoder that turns an invalid window selector into display capture.
- [ ] **Green:** rerun the protocol and Swift filters; `python3 qa/browser_live/check_memory.py && cargo test --offline -p aleph-desktop-macos --lib owned_window`. Expect matching test counts to be nonzero and all to pass; inspect actual logs. Keep project prerequisites distinct from a failing assertion.
- [ ] **Commit:** `feat(desktop): add owner-checked window capture` scoped to these files.

### Task 2: Connect execution evidence to observation / 连线实际投递与后置观测

**Files:** Modify `src/builtin_tools/desktop/native.rs` (resolved dispatch subject, shared screenshot processing), `src/builtin_tools/desktop/observe.rs` (typed observation and focus), `src/builtin_tools/desktop/mod.rs` (remove old observer, tool description), `src/builtin_tools/desktop/types.rs` (argument docs) and `src/builtin_tools/desktop/tests.rs` (fakes and integration). Reuse `src/builtin_tools/desktop/window_lookup.rs`, `interactable.rs`, and `src/tools/result_processing.rs`; do not fork those implementations.

**Consumes:** `ScreenCapability::screenshot_owned_window` from Task 1, `QueryFocusedParams { pid }`, existing `DesktopOutput`, `effect_for`, and `hoist_inline_images(&mut Value) -> Vec<ToolImage>`.

**Produces:** internal `observe::PostTarget::{Global, Process { pid: i32, window_id: Option<u32> }, Unavailable}`; `observe::gather_post_state(platform: &Arc<dyn DesktopPlatform>, target: PostTarget) -> Value`; `DesktopTool::capture_screenshot(&self, screen: &dyn ScreenCapability, args: &DesktopArgs, expected_owner_pid: Option<i32>) -> Result<DesktopOutput>` selected by the actual target. An owner PID with no valid window is rejected without any capture. Keep `DesktopTool::call_via_platform`'s existing external call signature and do not expose the internal subject in serialized action arguments.

- [ ] **Regression assertions:** in `post_observation` tests, record `QueryFocusedParams::pid` and the number/type of capture calls. After targeted input to PID `4242` with window `900`, assert `post_state.scope == "process"`, `target_pid == 4242`, no `frontmost_app`, and exactly one owned capture `(900, 4242)`, with neither legacy window nor global capture. A nested batch image must yield one image from `hoist_inline_images` per successful image-bearing entry. Assert `effect` unchanged and the image retains window coordinates.
- [ ] **Failure and compatibility assertions:** foreign AX PID gives `focus_status == "unavailable"` and zero exposed foreign fields; `Ok(None)` gives `"not_found"`; secure value remains withheld. PID-only/token input gives no capture and `post_screenshot_unavailable == "window_unconfirmed"`; failing/blank owned window gives `"capture_failed"` and zero wider calls. Scoped global delivery or an unattributable action gives `scope == "unavailable"`, `"scope_unavailable"` for requested pixels, and no reads. Unscoped global observes foreground/screen as before. Explicit opt-out, a refused action and a failed batch item call no observer. For inherited batch scope, verify each leaf's target independently; for partial batch, verify observed first item, failed second, no third, `effect == "partial"` and unchanged `delivered_count`. Exercise explicit PID versus `matched.pid` mismatch for AX and missing pre-dispatch owner for window mutation.
- [ ] **Red:** `python3 qa/browser_live/check_memory.py && cargo test --offline -p alephcore --lib post_observation`; confirm at least one assertion fails on the real old dispatch-to-observer path (wrong focus/capture or missing scoped result), not just on a new helper not yet defined.
- [ ] **Connect native dispatch:** after element resolution and actual rail choice, use that rail's PID for targeted input and only the same action's explicit candidate window. Preserve global behavior for unscoped input and `clipboard_write`. Named process/window with global delivery is unavailable. Resolve `move_window` / `resize_window` owner before mutation only when observing; failure remains additive. For AX use effective locator PID and `AxActionResult.matched` before it is serialized; conflict or unknown is unavailable. Lifecycle without effective PID is unavailable. Keep `call_via_platform` as the one caller connecting execution context to observation; move only the native dispatch boundary needed to borrow the subject through actual handlers. No new approval/classification map in `mod.rs`.
- [ ] **Connect state and pixels:** wait once before real observation; query process focus only with `Some(pid)` and verify element PID in Core, else report status without foreign values. Reuse one screenshot-processing path for direct, global-post and owned-post capture: existing blank-frame guard, size/budget/format work and coordinate metadata; owned branch is single-path and cannot fall through. Attach `post_state` and, on request, real `post_screenshot` or separate stable `post_screenshot_unavailable`; capture failures cannot change `success`, `message`, or `effect`. Delete the old synthetic global call and unconditional global focus path; update built-in text and argument docs.
- [ ] **Green:** rerun the `post_observation` filter; `python3 qa/browser_live/check_memory.py && cargo test --offline -p alephcore --lib builtin_tools::desktop` for existing rail, element, batch and action-effect regressions. Review the build logs and require nonzero tests in both filters.
- [ ] **Commit:** `fix(desktop): observe the dispatched action target` with the Core files only.

### Task 3: Align capability docs and verify the integrated branch / 校对文档与集成验证

**Files:** Modify `src/builtin_tools/desktop/perm.rs` (description only), `docs/reference/FEATURE_LOCATOR.md` (§7.x and §E.6 where affected), `docs/reference/DESKTOP_BRIDGE.md` (capture RPC and platform differences). Add a brief run record under `docs/superpowers/runs/` if integration needs to record independent review or skipped platform gates.

- [ ] **Copy check:** permission tool description says it reports OS capability state and platform settings guidance, with TCC only as the macOS case. Permission behavior is unchanged; `Unknown` remains possible. Reference docs name the scoped contract and fallback behavior, distinguish the built-in Tool path from unmounted Zahir projection, and remove unconditionally-global observation claims. / 修正文案，不改权限逻辑，不把投影写成已挂载。
- [ ] **Integrated gates:** `git diff --check 2ffce02fd` (includes modified tracked reference files; check any new run record when staging it); `python3 qa/browser_live/check_memory.py && cargo fmt --all --check`; `python3 qa/browser_live/check_memory.py && cargo check --offline -p alephcore -p aleph-desktop-macos`; rerun the focused protocol/Core/macOS and Swift suites if integration changed them. Use a shared target directory only outside concurrent Rust writers; preserve bounded logs and actual exit codes. On a known pre-existing validation failure, compare against `2ffce02fd` before claiming a regression. Never lower the gate to get green.
- [ ] **Platform proof:** verify macOS fixture prerequisites without altering permissions or interacting with real apps; if authorized and available, use `desktop/macos/tests/computer_use_e2e.rs` for background focus and pointer invariance, plus an owned-window capture check against the fixture. Report unavailable platform/host checks and the exact failure. Do not call simulated evidence real desktop QA. / 明示实际验证和未验证的平台差异。
- [ ] **Independent review:** assign a fresh reviewer to `git diff 2ffce02fd` including pending reference changes and the five Review Focus cases. The parent integrates feedback, reruns affected gates, and verifies one owner-checked call path with no wider fallback. Use permitted independent subagents per task/model rules; if only one task has a writer, keep overlapping work read-only or use a separate worktree. / 主代理负责验收与修复，不并发写同一工作树。
- [ ] **Commit and hand back:** `docs(desktop): document scoped observation boundaries` (permission description + docs/run record). Verify `git status --short --branch`, report commit IDs, check results, gaps, and the unchanged `main`; do not push or merge.
