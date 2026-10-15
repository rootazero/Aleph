# Task #56 — third real Native window (index 3): FAIL `device_same_unproven` / 第三次真实 Native 窗口：失败

**Verdict / 结论：FAIL (exit 1), cleanup CLEAN. Not a protected PASS. Overall remains PARTIAL, not production ready. / 最终状态 FAIL（退出码 1），清理干净；**不是** protected PASS；整体仍为 PARTIAL，not production ready。**

**Later update / 后续更新：** the user subsequently approved exactly one fourth window (`MAX_WINDOWS` 3→4); see `task56-native-real-window-4.md` (server-side same-device proven, but overall FAIL `dom_not_as_expected`). The statements below are the historical window-3 state. / 用户随后批准了恰好一个第四窗口（`MAX_WINDOWS` 3→4），见 `task56-native-real-window-4.md`（服务端同设备已证明，但整体 FAIL `dom_not_as_expected`）；以下为第 3 窗口当时的历史状态。

**(Historical) No fourth window. / 当时无第四个窗口：** `MAX_WINDOWS=3` is exhausted (windows index 1, 2, 3 used). There is no admin / WFP / managed companion path in this work, so none of those was used or is claimed. / `MAX_WINDOWS=3` 已用尽；本工作不含 admin / WFP / managed companion，均未使用、未声称。

This report was written from the evidence reported by the controller; the documentation pass itself ran no Cargo, Native, keyring, Gateway or source edit. Raw run JSON/log paths for this window are not recorded in this repo tree and were not re-read here. / 本报告依据主验提供的证据撰写；文档更新本身未运行 Cargo / Native / keyring / Gateway，也未改源码；本窗口的原始运行 JSON / 日志路径未收录在仓库内，此处未重读。

## 1. Pre-run build and test evidence / 运行前构建与测试证据

| Item | Result |
|------|--------|
| Rust QA trace: `cargo test -p aleph-desktop-shell --no-default-features --features qa-owned-root --bin aleph-desktop-shell qa_trace:: -- --test-threads=1` | 3/3 passed (compiled and tested) |
| QA-feature clippy `-D warnings` | 0 warnings |
| Production no-default-features clippy `-D warnings` | 0 warnings |
| JS self-tests | Panel 37/37; `real_qa_prep` 34/34; `real_run_io` fixes 32/32; `run-real` 15/15 + 32/32; all `node --check` pass |
| Fresh QA shell build sha256 | `8d434299e4729327bfa06996568c70cf3ba7b60651de976a8fe90d0b8b5449e0` |
| Reaper sha256 | `fcadd138fd427bd1f8eaee661064d3c4a16c9b73b5788ee53c23c66534def4ec` |
| Gateway | PID 12704, untouched, exact worktree + config |
| Dry preflight (index 3) | all 7 gates true |

Scope of these counts: QA-feature `desktop/shell` and the QA JS harness only; not a whole-workspace claim. / 这些计数仅限 QA feature 的 `desktop/shell` 与 QA JS harness，不代表整 workspace。

## 2. The real run / 真实运行

Third real Native window, index 3. Prior windows' timeouts: 180000 ms, 180000 ms; this window's max: 180000 ms. / 第三个真实 Native 窗口（index 3）；前两窗口超时 180000、180000 ms；本窗口上限 180000 ms。

| Phase | Result |
|-------|--------|
| Monitor | authenticated, 2 docs |
| Native trace | 4 records, complete |
| Empty load / save / reload stored credential | all PASS |
| Device paired | PASS |
| `server_same_device` | **FAILED** `device_same_unproven` |
| Final status | **FAIL**, exit 1 |

Why the edge failed (`evaluateDeviceEdge`, `qa/native_ipc/real_run_core.mjs`): `baselineNonzero: true`, `sawDisconnect: false`, `reconnectedNewer: false`. The strict edge (same device id observed `connected=false`, then `connected=true` with `last_seen_at` strictly newer, no re-save) was never observed, so the result is `unproven` and by design never PASS (see `task56-real-harness.md`, fix B2). / 严格边沿（同一 device id 先 `connected=false`、再 `connected=true` 且 `last_seen_at` 严格变大、无重存）未被观察到，故为 `unproven`，按设计绝不判 PASS。

Cause is **not established**: it could be that the Gateway never saw the old socket close within the sampler window, that the reload reused or kept the connection, or a sampler / timing issue. No root-cause claim is made. / 原因**未确定**（可能是采样窗口内未见旧 socket 关闭、reload 复用了连接，或采样 / 时序问题）；不作根因声称。

## 3. Cleanup / 清理

Cleanup CLEAN: the exact credential is absent, the owned root was removed, and the Gateway fixture cleanup was clean. Gateway PID 12704 was not touched by this report. / 清理干净：精确 credential 已不存在、owned root 已删除、Gateway fixture 清理干净。

## 4. What this does and does not show / 证明与未证明

Shown (this single window, Windows single machine, QA-owned build): the monitor channel authenticates, the native trace is complete, a credential round-trips (empty load → save → reload) and a device pairs. / 已显示：监控通道认证、native trace 完整、credential 往返、设备配对。

**Not shown / 未证明：** the server-side same-device proof across reload (`server_same_device`); a protected Panel action; JPEG frame delivery; production OS-keyring protected pairing end to end; the full Panel on macOS/Linux; managed companion. The in-window evidence is therefore not a protected PASS and must not be quoted as one. A `PASS_WITHOUT_JPEG_FRAME` verdict was **not** reached. / 未证明：reload 后服务端 same-device；protected Panel action；JPEG 帧；生产 OS keyring 端到端 protected pairing；macOS/Linux；managed companion。**未**达到 `PASS_WITHOUT_JPEG_FRAME`。

## 5. Not done / 未做

- No fourth window (budget exhausted); no retry; no code change to chase the edge.
- No admin / WFP / managed companion; `managed-companion` remains BLOCKED.
- No root cause analysis of the missing disconnect edge.
- No whole-workspace test, strict clippy, or macOS/Linux check.
- Nothing committed; all work dirty. Unrelated `.analysis/` left in place.

## 6. Next input needed / 后续需用户决策

Whether to authorize additional real-run budget beyond `MAX_WINDOWS=3`, or to accept the unproven edge as a documented limitation, or to investigate the Gateway `last_seen_at` / disconnect timing offline first. / 是否批准超出 `MAX_WINDOWS=3` 的额外真机窗口、是否将该边沿记为已知限制，或先离线排查 Gateway `last_seen_at` / 断连时序。
