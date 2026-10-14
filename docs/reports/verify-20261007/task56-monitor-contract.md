# Task 56 — QA monitor contract reconciliation / QA 监控契约对账

Scope / 范围: `qa/native_ipc/monitor_contract.mjs` (+ its self-tests in `qa/native_ipc/real_qa_prep.mjs`) vs. `QA_MONITOR_JS` in `desktop/shell/src/main.rs`.
Not touched / 未改动: `desktop/shell/**` (Rust, owned by another agent), production behavior, other docs.

## 1. Result / 结论

EN: The harness-side contract now matches what the injected monitor actually emits. Real-run readiness stays **gated (fail closed)** because the monitor channel is origin-checked only, not nonce-authenticated.

ZH: 契约与注入的监控脚本实际输出已对齐。真机运行仍被**拒绝（fail closed）**：该通道只做了 Origin 校验，尚无 nonce 认证。

## 2. Mismatches fixed / 已修复的不一致

| # | Old contract / 旧契约 | main.rs actual / 实际 | Now / 现状 |
|---|---|---|---|
| M6 | same-origin `/__qa_monitor/<run>` | fixed loopback `ws://127.0.0.1:<port>/qa` | `validateMonitorChannel` |
| a | no `install_failed` | sole first event `{trusted:false, reason}` | recognized, untrusted terminal (seq 0 only, never complete) |
| b | no `"unexpected"` | `resultShape`/`errorCode` may be `"unexpected"` | accepted, counted as anomaly (never `clean`) |
| c | `argKeys` exact | present allow-listed keys (ordered subset) | ordered subset; strict subset = anomaly |
| d | `monitor_end` required, reason `window_elapsed` | only on pagehide / overflow: `page_unload`\|`monitor_error`; socket input ignored | harness closing the socket ends the window (`finish({closedByHarness:true})`) |
| e | `connect_evidence`, `dom_command` | never emitted (monitor cannot see the Gateway WS) | removed; rejected as `unknown_type` |

## 3. Design chosen / 选定设计

EN
- Fixed loopback port (`--qa-monitor-port`, 1024..65535). `validateMonitorChannel({port, wsUrl, markerOrigin, requestOrigin})` requires: exact URL `ws://127.0.0.1:<port>/qa`; `markerOrigin` canonical http(s) origin on exact loopback (`127.0.0.1`/`localhost`/`[::1]`), not the monitor port; the upgrade `Origin` header byte-equal to `markerOrigin`.
- Returns `nonceAuthenticated:false` always. `PREREQUISITES.monitorNonceAuth = false` (new) and `monitorInjectionSeam = false` keep `unmetPrerequisites()` non-empty, so the runner still refuses (self-tested).
- No arbitrary values: unchanged field allow-lists, unknown key/type rejected, 512-byte event cap.
- `install_failed` after `hello` → `install_failed_after_start`; `trusted:true`, free-text reason, extra keys rejected.

ZH
- 固定回环端口；`validateMonitorChannel` 要求精确 URL、marker origin 为精确回环且不等于监控端口、升级请求的 `Origin` 与 marker origin 逐字节一致。
- 始终返回 `nonceAuthenticated:false`；新增前置条件 `monitorNonceAuth=false`，运行器仍拒绝执行。
- 仍无任意值/凭据/URL/文本字段；`install_failed` 为不可信终止事件，永不视为 complete。

## 4. Blocker / follow-up / 阻塞与后续

EN: A nonce-authenticated first message needs a shell flag (e.g. `--qa-monitor-nonce`) in `desktop/shell/src/qa_root.rs` + `main.rs` (monitor `hello` carrying a secret, harness verifying it). `qa_root.rs` has only `--qa-root/--qa-nonce/--qa-credential-backend/--qa-monitor-port` today. Out of scope here (Rust owned elsewhere). Until then a local process on loopback could race for the port and spoof only the Origin header (non-browser clients can set any Origin); do not flip `monitorNonceAuth`.
ZH: 需在 Rust 侧新增监控 nonce 参数并让 `hello` 携带，harness 校验；此处不在范围内。在此之前，本机其他进程可抢占端口并伪造 Origin，故不得翻转 `monitorNonceAuth`。

## 5. Tests actually run / 实际运行的测试

- `node --check qa/native_ipc/monitor_contract.mjs`, `real_qa_prep.mjs`, `run.mjs` → ok.
- `node qa/native_ipc/real_qa_prep.mjs --self-test` → `total 32, passed 32, failed []`. New/changed cases: valid stream (monitor_end and harness close), install_failed untrusted, unexpected/subset anomalies, values-cannot-be-carried (incl. rejected `dom_command`/`connect_evidence`/`window_elapsed`), stream order, `validateMonitorChannel` matrix + `monitorNonceAuth` gate, source audit.
- `run.mjs` has no `--self-test` flag (it refuses unknown args); not applicable.

## 6. Not done / 未做

- No Rust edit, no cargo build/test; main.rs read only — behavior claims come from reading `QA_MONITOR_JS`, which has never been compiled or executed.
- No listener implemented; no real WebSocket, browser Origin/mixed-content (https page → `ws://127.0.0.1`) behavior verified.
- No Gateway/native/keyring/browser run; no connect evidence is provided (monitor cannot observe the Gateway WS).
- `task56-monitor-seam.md` / `task56-real-qa-plan.md` still mention the old `/__qa_monitor` design and were not edited (no broad docs).
- Nonce authentication not implemented (section 4).
- 未验证：脚本从未执行、Origin/混合内容行为未实测、监听端未实现、无 Gateway 连接证据、旧文档未更新。
