> Historical independent report / 历史独立审查报告. Reviewer: `e0a5bfe5-3948-45b1-a6b2-212adb184fee`. Only repository-absolute path prefixes are normalized for portability; verdict and findings are unchanged. Raw report SHA-256: `2f0037bcbe600541fbfccb567d5e7a7faf10fa24dfaf2fba5d397b23eef90f23`. The original bytes remain in the task evidence workspace. / 仅移除本机仓库路径前缀，结论未改。

## Review

- **Correct：A/B 基础证据**

  - A 的候选完整测试回执仍为 `22,402 passed, 0 failed, 20 ignored`，对应 [20261116T091528.077216Z.stdout.log:22453](docs/superpowers/runs/phase9-test-output/20261116T091528.077216Z.stdout.log:22453)。该回执证明的是候选 Core 库测试闭合，不扩展为 C/D 运行时通过。
  - B 使用 Cargo resolver 生成的候选锁，候选条目数为 1,572，SHA-256 为 `12300967333b45f8a0fa1f401cf13a559b5b8a881ee95550b1bbf1a8d42e21f5`；冻结锁和 retained 快照仍被保留。Cargo.lock 中七个 NSH Git 依赖继续固定到 `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`。
  - `qa/cargo_lock_delta.py` 按 `(name, version, source)` 区分包，并保留完整 added/removed/changed 集合，未发现把多版本包错误折叠成单一名称的缺陷。

- **Correct：raw-event provenance 修复**

  - 当前 [qa/nsh_panel_launcher.py:157-165](qa/nsh_panel_launcher.py:157) 会从已哈希校验的 stdout 中重新解析 `ALEPH_PANEL_QA ` marker，按 nonce 和 `qa_run_id` 过滤，并将原始 marker 序列与去除 `at_ms` 后的 `document["events"]` 做精确比较。
  - 这修复了此前仅以 stdout/stderr 文件哈希存在为依据、却没有证明事件数组来自该子进程的问题。
  - 根据最新回报，新增真实 child 回归测试共 14 个通过；现有 [qa/test_nsh_panel_launcher.py:92-108](qa/test_nsh_panel_launcher.py:92) 也覆盖了保存事件被篡改时 `verified_capture` 必须失败的行为。该问题不再作为当前 finding。

- **Correct：Panel reconnect 防护与 R2 边界**

  - `interfaces/webchat` 的历史、artifact 和 reattach 异步结果均增加了 connection epoch/session key 检查；artifact 在连接 epoch 变化时重新请求，Sidebar remount 保留已有 agent selection。对应 Panel library 回执为 `1,560 passed, 0 failed, 0 ignored`。
  - NSH 仍只托管现有 Panel；当前源码没有重新引入 native Chat、native artifact projection 或第二套业务 Gateway transport，符合 R2/R4。
  - [desktop/shell/build.rs:31-35](desktop/shell/build.rs:31) 只在 `debug_assertions` 与 `qa-owned-root` 同时成立时注册 `phase9_observation`。
  - [desktop/shell/src/phase9_qa.rs:35-52](desktop/shell/src/phase9_qa.rs:35) 将 QA capability 限定为当前 Remote canonical origin 和 `main` window；[desktop/shell/src/phase9_qa.rs:64-84](desktop/shell/src/phase9_qa.rs:64) 继续执行 window label、origin、nonce、run ID 和 16 KiB 大小检查。该增量权限修复尚不能替代新的 host/window 回执，但从源码看没有发现放宽到任意 origin 的问题。

- **Finding: P1 — `old_epoch` challenge 没有经过实际 Panel 接收/拒绝路径**

  - **位置：** [qa/nsh_panel_driver.js:43-53](qa/nsh_panel_driver.js:43)，以及 [shared/ui_logic/src/connection/wasm.rs:190-203](shared/ui_logic/src/connection/wasm.rs:190)。
  - **触发条件：** reconnect challenge 在 `priorSocket` 上执行 `socket.dispatchEvent(new MessageEvent("message", ...))`。`priorSocket` 是旧连接；在实际 reconnect 前，`WasmConnector::disconnect()` 已经在 [shared/ui_logic/src/connection/wasm.rs:195-203](shared/ui_logic/src/connection/wasm.rs:195) 清除 `onmessage` handler 并关闭旧 socket。
  - **影响：** 该 challenge 证明的是“旧 socket 曾经有 receiver，随后 receiver 被清除，并且向没有 receiver 的 EventTarget 派发消息后 DOM 没有变化”。它没有证明生产 reducer 收到了旧 epoch 帧并依据 epoch/session 规则拒绝该帧。`challenge()` 随后只比较两次 DOM projection，因此仍会把 detached transport 的 no-op 记录为 `rejected`。
  - **规范矛盾：** [qa/phase9_runtime.md:28](qa/phase9_runtime.md:28) 要求通过 actual WebSocket event handlers 验证 old-epoch、foreign-session、malformed-topic 和 duplicate，并明确规定 observer 自身过滤不能满足 gate；当前 `old_epoch` 恰好绕过了生产 handler。
  - **最小修正：** 在仍由生产 Panel handler 接收的连接路径上注入带旧 epoch 的实际消息，或为 transport/reducer 提供仅测试使用的可观测注入 seam，使消息经过真实 `WasmConnector` 接收闭包和 Panel event dispatch 后再检查 projection。必须保留“生产 handler 实际收到该帧”的证据；仅增加 `observedReceivers`、检查 `onmessage === null` 或继续向已关闭 socket 派发事件都不能关闭此 finding。

- **Open gate：C 尚未完成，属于验收缺口而非另一项源码 bug**

  - 最新 runtime9 仍没有证明同一 session 的 reconnect history hydration；父会话提供的最新状态仍显示 C OPEN。
  - 先前真实窗口只达到 accepted → streamed → complete → artifact-refreshed，以及第二次 authenticated connection；仍缺少完整 fresh snapshot、同 session history、真实 adverse-frame rejection 和 owned close。
  - Tauri 的此前 title channel 受 1,000 字节截断问题已由 debug-only canonical-origin/root-nonce/run IPC 替代，但当前增量权限修复仍在串行 host/window 验证中，不能把源码存在视为运行时 PASS。
  - 因此，Panel library、WASM pipeline、host unit/API tests 的通过不能关闭 C。

- **Open gate：D 尚未完成，属于资源归属和实测证据缺口**

  - [qa/nsh_panel_launcher.py:247-248](qa/nsh_panel_launcher.py:247) 明确将当前资源范围记录为 `direct_child_only`，并将 `resource_scope.complete` 固定为 `False`，原因是 WebKit XPC 进程无法由 POSIX parentage 归属。
  - [qa/nsh_panel_launcher.py:259-261](qa/nsh_panel_launcher.py:259) 会阻止不完整资源范围进入性能 metrics export，这个 fail-closed 行为是正确的。
  - 但 D 要求 Tauri-first、NSH-second、10 warmups + 30 measured、100 ms sampling、p95/median 阈值和 owned-process/resource completeness 的真实证据。当前没有满足这些条件的最终实测回执，不能导出 parity acceptance，也不能宣称性能通过。

- **Correct：证据与结论边界**

  - `runtime-valid.json` 仍被标记为 protocol fixture；fixture boundary tests 不会把它升级为 live Gateway proof。
  - `verified_capture` 的修复已经避免了原先的 raw-event 伪造路径，但它仍不能解决 WebKit XPC 资源归属或 runtime9 缺少同 session history 的问题。
  - `docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md` 当前状态保持为 `IN PROGRESS — C/D/E OPEN`，与实际回执和源码审查结果一致。

## Merge verdict

**BLOCK / NOT COMPLETE。**

当前唯一确认的代码级问题是 `old_epoch` challenge 没有通过真实 Panel 接收和 reducer 拒绝路径，属于 P1，必须修正或以明确的真实 handler 注入证据替代。raw-event provenance 绑定缺口已在当前源码中修复，不应重复报告。

即使修复该 P1，Phase9 仍不能关闭：runtime9 尚未证明同 session history/reconnect/close 的完整 C 闭环，D 仍缺少 WebKit XPC owned-resource completeness 和 10 warmups + 30 measured 的有效实测证据。当前不应宣称整体 PASS、parity PASS 或 release performance PASS。

本次审查未编辑文件，未运行 Cargo、构建、测试或真实窗口；未读取受保护的 `.claude/settings.json`、root `config.lock` 或 root `config.toml`；未验证尚未落盘的 runtime9/Gateway 进行中回执；也未把 C/D 的开放验收门槛伪装成已确认的额外生产源码 bug。