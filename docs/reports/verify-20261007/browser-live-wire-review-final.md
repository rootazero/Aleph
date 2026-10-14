# Browser-live wire / viewer final read-only review (#23)

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256` (dirty/untracked working tree, not just HEAD diff)
Method: static read of full current source; no edits, no Cargo/tests, no processes, no network, no keyring/HKCU/native home.
方法：仅静态通读当前工作区完整源码；未编辑、未运行 Cargo/测试/进程/网络/keyring/HKCU。

---

## 1. Verdict / 结论

**EN.** No new Critical and no new Important finding in the files below. Six Low and three Info items are listed (none blocks the Frames-only production path). All previously-unread ranges named in the task are now read in full. I do **not** issue a whole-branch approval: this review covers the listed files only (see §6 limits).

**中文。** 下列文件中未发现新的 Critical / Important 问题；列出 6 条 Low 与 3 条 Info（均不阻塞 Frames-only 生产路径）。任务所列此前未读范围现已全部读完。**不对整个分支给出批准**：本次仅覆盖下列文件（见 §6 覆盖限制）。

## 2. Coverage / 覆盖范围

| File | Read |
|---|---|
| `shared/protocol/src/browser_live.rs` (1839 lines) | fully, incl. 212-590, 630-756, 868-1020 and tests |
| `interfaces/webchat/src/platform/wide/views/browser/transport.rs` (726) | fully |
| `interfaces/webchat/src/platform/wide/views/browser/session.rs` (1332) | production 1-720 fully; tests 721-1332 skimmed by test names/helpers only (names listed below), bodies not line-audited |
| `src/browser/live/control.rs` | `validate_input` (265-300), `queue_input`/`dispatch` (448-560), `change` (350-410), `valid_event` (713-775) |
| `src/browser/live/mod.rs` | `dispatch_routed_human_input` / `dispatch_human_input_gated` (pointer, key, text, navigate branches, ~821-1040), `bind_control` (~451-510), `declare_boundary` (1404-1438) |
| `src/gateway/server/browser_viewer_route.rs` | production 60-785 |
| `src/browser/live/viewer_backend.rs` | `authoritative_resource`, `bind_attached`, `prepare`, `dispatch_input` (150-354) |
| `model.rs` | `from_snapshot` / `from_resource` / `vet_resource` |

Note: the pointer-event branch lives in `src/browser/live/mod.rs` (`dispatch_human_input_gated`), not `control.rs`; `control.rs` holds the admission validator `valid_event`.
注：指针事件分支实际在 `src/browser/live/mod.rs`；`control.rs` 持有入口校验 `valid_event`。

## 3. Confirmed findings / 确认的发现

No Critical/Important. All below are Low/Info, fail-closed or resource-hygiene only.

### Low

**L1 – Size caps enforced after full buffering** — `transport.rs` `io::post_json` (`response.text()`, `RESPONSE_MAX_BYTES`=1 MiB) and `handle_message` (`TEXT_MESSAGE_MAX_BYTES`=256 KiB, `BINARY_MESSAGE_MAX_BYTES`).
Mechanism: browser has already allocated the whole body/message before the cap check. Effect: memory use bounded only by the browser/gateway, not the 'cap'. Peer is the same-origin trusted gateway, so low. Test: wasm/integration test with a >cap reply asserting failure without retaining content (cannot be done natively).
EN/中文：上限在浏览器已完整缓冲后才检查；对端为同源可信网关，故 Low。

**L2 – No connect-phase timeout** — `transport.rs` wasm `handle_open` arms the 2 s `AUTH_TIMEOUT` only on `open`.
Mechanism: a socket stuck in CONNECTING leaves the session in `Exchanged` holding the one-shot secret; UI stays pending. Effect: indefinite spinner; the server parked viewer expires after `PARKED_TTL`=120 s (`browser_viewer_route.rs:157-172`), so no server leak. Suggested test: wasm test with a never-opening mock socket expecting a failure state after a bounded time.
中文：仅 open 后才启动认证计时器；CONNECTING 卡住时界面无限等待，服务端 120s 后回收。

**L3 – Fetch timeout does not abort the request** — `transport.rs` `post_json` has a 10 s timeout race but no `AbortController`.
Effect: UI reports Network failure while the request may still consume the nonce/ticket server-side; user must reissue (fail closed, no secret exposure).
中文：超时不取消 fetch，可能仍消耗一次性 nonce/ticket；失败闭合，需重新签发。

**L4 – Wheel deltas unbounded in magnitude** — `control.rs:753` `valid_event` Wheel: `delta_x.is_finite() && delta_y.is_finite()` only; `mod.rs` passes them straight to `dispatch_mouse_event`.
Effect: a controlling HumanInput viewer can send ±1e308 deltas to CDP. Not reachable in production today (no HumanInput issuance). Test: `valid_event` with `delta_x=1e300` expecting a documented clamp/reject.
中文：滚轮增量仅校验有限，不限幅度；当前生产不可达（HumanInput 未签发）。

**L5 – Pointer coordinates validated against bind-time viewport** — `control.rs:716-725` uses `st.live.viewport_css_px`, set once in `bind_control` (`mod.rs` ~495-502) from `LiveViewerBackend.viewport`, never updated from the real page viewport.
Effect: coordinates in the frozen range may fall outside the real viewport if the page differs; frame header uses the same snapshot viewport (`browser_viewer_route.rs:~636`), so viewer and validator are mutually consistent. Mapping fidelity concern only. Dormant (HumanInput). Test: input on a target whose actual viewport ≠ configured.
中文：坐标按绑定时的固定 viewport 校验；与帧头一致，仅为映射保真问题，且当前休眠。

**L6 – Client accepts `Redacted`, gateway only issues `Safe`** — `session.rs` `validate_exchange` allows `Safe|Redacted`; `viewer_backend.rs` `authoritative_resource` requires exactly `Safe`.
Effect: client is looser than server; server is authoritative so no exploit. Suggest aligning client to `Safe` (or documenting) to avoid a future mismatch.
中文：客户端放宽到 Safe|Redacted，服务端只放行 Safe；服务端为权威，无利用路径，建议对齐。

### Info

- **I1** `browser_live.rs`: `validate_frame_header` checks only `content_type`; `header.version` is validated in `session.rs` `accept_binary_frame` (`VIEWER_PROTOCOL_VERSION`) — compensated, but any other consumer of `ViewerWsFrame::decode*` must repeat it.
- **I2** `browser_live.rs`: no wire-level validation for `HumanInputRequest`/`ControlRequest` (`seq`, `expires_at_ms`, f64 coords); serde_json already rejects NaN/Inf; semantic validation is in `control.rs` `validate_input`/`valid_event` (finite, in-viewport, modifiers ≤15, click_count 1..=2, key/code ≤128, text ≤4096, URL ≤8192) — checked, adequate.
- **I3** `transport.rs`: server `Closed` is not matched against session binding; any `Closed` ends the session (fail-closed direction, harmless). Trailing-dot FQDN origins are refused by `valid_dns_host` (fail closed).

## 4. Refuted false alarms / 已排除的误报

1. **Real frames rejected as StaleSeq/ScopeMismatch because snapshot has `last_frame_seq: 0` / `stream_epoch` from control?** Refuted. `viewer_backend.rs:231-233` uses `live.stream_epoch`, which `mod.rs bind_control` sets to `stream.epoch`; the hub publishes with the same `stream.epoch` (`stream_hub.rs` `epoch`, per-epoch `seq` starts at 1). Gateway header uses `frame.epoch`/`frame.seq` (`browser_viewer_route.rs:629-631`). First frame seq 1 > 0. A later epoch bump ends the viewer (fail closed, intended).
   误报：真实帧被 StaleSeq/ScopeMismatch 拒绝——已排除（epoch 同源，seq 从 1 起）。
2. **Cleanup/`run()` race leaves a live socket** — `Closed(Local)` makes `accept_*` return `WrongPhase`; `run` re-checks `Exchanged` before `open_socket`; orphan socket disposed.
3. **Double `on_error`/`on_close`** — `close()` is idempotent, first reason wins.
4. **Fragment/origin parser laxness** — strict: grant_id ≤64, nonce ≤128, `[A-Za-z0-9_-]`, duplicate/unknown refused, raw hash ≤512; userinfo/path/query/port 0 refused; `http` only for loopback.
5. **Client AuthHello unchecked** — `decode_viewer_auth_client_message` does not call `validate_shape`, but gateway `read_auth_hello` (`browser_viewer_route.rs:487-515`) does; client side re-validates in `validate_exchange`.
6. **Viewer sends input** — the standalone transport sends only `AuthHello`; never Attach/Take/Input.
7. **Missing HumanInput gate on input/take** — both `validate_input` (control.rs:~290) and `change` (control.rs:378) require `ViewerPermission::HumanInput`; Navigate additionally requires `Navigate`. A Frames-only viewer's Input/Take yields `PermissionDenied`.
8. **Navigate bypasses SSRF** — `viewer_backend.rs dispatch_input` runs `navigation_guard.check_navigation` before CDP and re-checks the owner lease around it.
9. **Post-document-boundary input still dispatched** — `declare_boundary` notifies sinks (grant revoke → lease close) before advancing the generation; Frames viewers additionally get a fence-driven `Closed` in `run_live_loop`. Residual dormant window is §5 D2.

## 5. Risk classification / 风险分类

**Frames-only (production issuer today)** — Active surface: consume → exchange → `AuthHello` → binary JPEG frames. Hard constraints verified: sole Gateway CDP (viewer never touches the connection), exact resource/binding + connection/stream epoch + monotonic seq on every frame, dedicated `AuthHello` first-frame (≤256 B, 2 s, text only), body cap 4 KiB, WS inbound 8 KiB, client binary rejected, parked viewer 120 s TTL, one-shot secret/nonce/ticket, no URL/current-tab inference. Active findings: L1, L2, L3, L6, I1, I3 (all Low/Info).

**Dormant HumanInput/Navigate (no production issuance)** — Gated by permission checks in `validate_input`/`change`, lease, epochs, replay-seq, controller identity, `valid_event`. Findings: L4, L5, and D2 below. Must be re-reviewed before any issuer starts granting `HumanInput`.

- **D2 (dormant, Low–Medium if enabled)**: a `HumanInput`-only viewer (no `Frames`) has `can_frames=false`, so no frame-fence close ever fires for it in `run_live_loop`; its only fence is the boundary sink's grant revoke. Pointer/keyboard input validity between a page-driven navigation and sink revoke is not checked against content generation in `validate_input`. Suggested test: HumanInput-only lease, trigger `Page.frameNavigated` mid-input, assert the next `Input` is `Revoked`.
  中文：仅 HumanInput 的 viewer 无帧栅栏，依赖边界 sink 撤销；启用前须补测。

## 6. Explicit limits / 明确限制

- `session.rs` tests 721-1332 and `browser_viewer_route.rs` tests (785+) were not line-audited; only names/helpers (e.g. `mismatched_frames_are_rejected_without_mutation`, `ws_full_session_frames_input_and_revoke`) were seen. Their assertions were not independently verified, and nothing was executed.
- Not reviewed: `viewer.rs` beyond viewport/display usage, `standalone.rs`, `bridge.rs`, `launch.rs`, `grant.rs` (begin_auth/commit_auth/exchange internals), `issuer.rs`, `stream_hub.rs` beyond epoch/seq, the CDP method layer, sandbox, Managed Windows plan, future plans.
- wasm-only code paths (`transport.rs` io module) were read, not executed; browser behavior (L1-L3) is inferred from source.
- Whole-branch approval is **not** granted. Scope of this report: wire crate + standalone Panel viewer session/transport + exact gateway route/dispatch path. Verdict applies to those files at the current dirty working tree only.
- 不授予整分支批准；结论仅适用于上述文件在当前脏工作区的状态。未执行任何代码，wasm 行为为源码推断。

## 7. Not done / 未做

Did not run tests or Cargo; did not audit sandbox/other branches; did not verify test assertions beyond names; did not modify any code.
未运行测试/Cargo；未审沙箱与其他分支；未核对测试断言细节；未修改任何代码。
