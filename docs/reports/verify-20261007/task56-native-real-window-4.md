# Task #56 — fourth real Native window (index 4): FAIL `dom_not_as_expected` (server-side same-device proven; UI evidence missing) / 第四次真实 Native 窗口：失败（服务端同设备已证明；UI 证据缺失）

**Later update / 后续更新：** the user approved one final fifth window (`MAX_WINDOWS` 4→5); it FAILED at `reveal_browser` (`dom_not_as_expected`), cleanup clean; see `task56-native-real-window-5.md`. Statements below are the window-4 historical state. / 用户批准最终第五窗口，已失败于 `reveal_browser`，见 `task56-native-real-window-5.md`。

**Verdict / 结论：overall FAIL (exit 1), cleanup CLEAN. Not a Native PASS. Overall remains PARTIAL, not production ready. / 整体 FAIL（退出码 1），清理干净；**不是** Native PASS；整体仍为 PARTIAL，not production ready。**

**Authorization / 授权：** `MAX_WINDOWS` was changed from 3 to 4 (`qa/native_ipc/real_qa_prep.mjs:37`, "3 original + exactly one user-approved fourth window") by explicit user approval, for exactly this one window. **No fifth window.** No admin / WFP / managed companion was used or is claimed. / `MAX_WINDOWS` 经用户明确批准由 3 改为 4，仅此一个窗口；**不再有第五个窗口**；未使用、未声称 admin / WFP / managed companion。

This report was written from the evidence reported by the controller. The documentation pass itself ran no Cargo, process, Native, keyring, Gateway or source edit; raw run JSON/log paths are not recorded in this repo tree and were not re-read here. / 本报告依据主验提供的证据撰写；文档更新本身未运行 Cargo / 进程 / Native / keyring / Gateway，也未改源码；原始运行 JSON / 日志路径未收录于仓库，此处未重读。

## 1. Pre-run evidence / 运行前证据

| Item | Result |
|------|--------|
| Fresh preflight, window index 4, prior windows `180000,180000,180000` ms | all 7 gates true |
| QA shell sha256 | `8d434299e4729327bfa06996568c70cf3ba7b60651de976a8fe90d0b8b5449e0` (same as window 3) |
| Reaper sha256 | `fcadd138fd427bd1f8eaee661064d3c4a16c9b73b5788ee53c23c66534def4ec` (same as window 3) |
| Gateway | PID 12704, untouched |

## 2. The real run / 真实运行

Real window index 4, max 180000 ms. / 第 4 个真实窗口，上限 180000 ms。

| Phase / evidence | Result |
|------------------|--------|
| Monitor | authenticated, 2 docs |
| Native trace | 4 records, complete |
| Empty load | PASS |
| Ticket | PASS |
| Save ACK | PASS |
| New devices | exactly one new device |
| Stored credential load after reload | PASS |
| Diagnostic sampling | `sampleCount=5`, `errorCount=0`, `timeoutCount=0`, `anyDisconnected=true`, `anyAdvance=true` |
| Strict server-side edge (`evaluateDeviceEdge`) | `sameDevice=true`, `sawDisconnect=true`, `reconnectedNewer=true` — **proven** (contrast window 3: `device_same_unproven`) |
| DOM after reload (`server_same_device` phase) | `domResource=false`, `domPaired=false`, `domTokenWall=false` — **FAILED** |
| Overall | **FAIL**, only failure code `dom_not_as_expected`, exit 1 |

## 3. What is proven vs. what failed / 已证明 vs 失败

**Proven in this single window (Windows single machine, QA-owned build, Gateway PID 12704):**
- the monitor channel authenticates; the native trace is complete;
- credential round-trip: empty load → ticket → save ACK → exactly one new device → stored credential still loads after reload;
- **server-side same-device across reload:** the same device id was observed disconnecting and then reconnecting with a strictly newer `last_seen_at`, without a re-save. This is the edge window 3 could not prove.

已证明（单窗口）：监控通道认证、native trace 完整、credential 往返（空加载 → ticket → 保存 ACK → 恰好新增一个设备 → reload 后仍可加载存储 credential）；**reload 后服务端同设备**：同一 device id 先断开、再以严格更新的 `last_seen_at` 重连且未重存。

**Failed (UI evidence):** after reload the runner's DOM probe saw none of the Panel resource view (`domResource=false`), the paired state (`domPaired=false`), nor the token wall (`domTokenWall=false`). The runner requires UI resource + paired evidence in addition to the server edge, so the phase fails with `dom_not_as_expected` (see `real_run_core.mjs`, `server_same_device` phase). This is **not** a Native PASS and must not be quoted as one, even though the server-side edge itself is true.

失败（UI 证据）：reload 后 DOM 探测既没有看到 Panel 资源视图，也没有已配对状态，也没有 token wall；runner 在服务端边沿之外还要求 UI resource + paired 证据，故以 `dom_not_as_expected` 失败。尽管服务端边沿为真，这**不是** Native PASS，不得引用为通过。

**Cause of the DOM failure is not established.** Possibilities not distinguished here: the Panel did not render within the probe window after reload, the probe target/selector did not match the post-reload page, the page was in an intermediate state, or a real Panel defect. No root-cause claim is made. The all-false triple (not "paired but no wall") also does not by itself say which. / DOM 失败**原因未确定**：可能是 reload 后探测窗口内未渲染、探测目标/选择器与 reload 后页面不匹配、处于中间态，或确为 Panel 缺陷；不作根因声称。

## 4. Cleanup / 清理

Cleanup CLEAN (controller-reported): exact credential absent, owned root removed, Gateway fixture cleanup clean. Gateway PID 12704 was not touched by this documentation pass. / 清理干净（主验报告）：精确 credential 已不存在、owned root 已删除、Gateway fixture 清理干净；本文档更新未触碰 Gateway PID 12704。

## 5. What this does not show / 未证明

- A Native PASS: the overall run failed.
- UI resource view, paired state, or token wall after reload; a protected Panel action; JPEG frame delivery (`PASS_WITHOUT_JPEG_FRAME` **not** reached).
- Production OS-keyring protected pairing end to end (QA build compiles the OS keyring out); the full Panel; macOS/Linux; managed companion (`managed-companion` remains BLOCKED).
- Repeatability: one window only; the window-3 → window-4 difference in the server edge is not explained (no root cause for either result).

## 6. Not done / 未做

- No fifth window, no retry, no code change to chase the DOM failure in this pass.
- No admin / WFP / managed companion.
- No root-cause analysis of `domResource/domPaired/domTokenWall=false`.
- No whole-workspace test, strict clippy, or macOS/Linux check.
- Nothing committed; all work dirty.

## 7. Next input needed / 后续需用户决策

Whether to authorize any further real window (budget is now exhausted at `MAX_WINDOWS=4`), accept the DOM-evidence gap as a documented limitation, or investigate the post-reload DOM probe offline (selectors/timing vs. an actual Panel defect) first. / 是否批准再开真机窗口（`MAX_WINDOWS=4` 已用尽）、是否将 DOM 证据缺口记为已知限制，或先离线排查 reload 后的 DOM 探测（选择器/时序 vs 真实 Panel 缺陷）。
