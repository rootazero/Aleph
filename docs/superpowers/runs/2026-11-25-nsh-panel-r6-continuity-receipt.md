# 2026-11-25 — NSH Panel R6 continuity / 会话连续性验证

## Scope / 范围

The [R6 design](../specs/2026-11-25-nsh-panel-r6-session-continuity-design.md) and [execution plan](../plans/2026-11-25-nsh-panel-r6-session-continuity.md) cover the opt-in NSH TSX Panel, not a default-host cutover. [The QA harness](../../../qa/nsh_s3_chat/README.md) uses an isolated Gateway, temporary homes and workspace, loopback TLS, real Desktop IPC, and the same TSX app on `/v1/panel/` in Chrome.

第六轮先连通 Shell 的会话选择与 Core 归属检查，再由 SQLite / 文件后端按各自既有投影顺序向前翻页，最后由共享 Rust reducer 合并旧页和实时转录。不新增会话目录、转录缓存或宿主业务逻辑。

## Mounted checks / 实际挂载

macOS 27.2.1, Chrome 158.0.8127.37; source and harness at `f9de16c7d`. Artifact SHA-256: Gateway `af9fbf93bcffd268f27c320fb2bb421e9786d40e8f1f9d5d0130521215902979`, NSH shell `d85b91dec638a1a31dac1c71516bf4881b6ef1b6f748534c482c5be3085f9e67`, Web entry `909794b2ec965a6972d22a50f8b1e0c7c3153f79b0736bada9abdda332997ca0` (index, not a whole-bundle digest).

| Check / 检查 | Evidence / 证据 |
| --- | --- |
| macOS IPC · restore | Opened Settings and saw `Appearance`; returned to Chat in the same process and saw the sent marker again. / 同进程从设置返回，恢复已选会话。 |
| macOS IPC · prepend | Seeded 205 rows only in the stopped fixture's SQLite projection. At the head of the initial 200-row window, Vision OCR located `Fixture earlier answer 007` at 172.31 pt before and after prepend. `Scrollback fixture start 000` then reached the window. / 使用窗口内 OCR 坐标量测，不把截图本身当断言。 |
| Chrome · prepend | Observed the older-page request and selected session key in actual Web v1 frames. The same row measured 129.75 CSS px before and after prepend. The oldest row then reached the clipped viewport, not just the overscan region. |
| Chrome · reconnect | Restarted Gateway and paired again. The first restored history request carried the original session key, and the send count did not increase. / 复配后恢复原会话，未重发。 |
| Rich transcript / 富转录 | A separate loopback fixture exercised real desktop Write/Edit, saved diffs, web focus return and hover, Markdown/Mermaid, Context, Jump to bottom, IME, Stop, and web text → tool → text before the provider released the terminal response. |

Both `--continuity` and `--rich` passed on those artifact hashes; the final Gateway exited with status 0 in each run. Each output directory contains its own `report.json` and screenshots.

## Automated checks / 自动验证

- `just panel-tsx-test`: 67 NSH tests, TSX typecheck, and 14 Node web-host tests passed. Native bundle, Web WASM package, and `aleph-nsh-shell --features tsx-panel` built successfully.
- `just test-shared`: 2,174 passes across both shared reducer feature shapes and the protocol, TUI, CLI, and slot API targets; six project-declared ignored cases remained ignored.
- Core filters `scrollback`, `history_page`, `history_reports_the_whole_transcript_alongside_the_window_it_serves`, and `v1_transcript_details_discloses_only_owned_safe_tools` passed. These are overlapping filters, not a full Core suite. They cover both history backends, equal timestamps, healed ordering, appends between pages, deleted and ambiguous boundaries, legacy totals, and the admission-before-cursor contract.

## Reviews / 独立审阅

- NSH's top offset needs the start anchor hook; that hook never requests a page. Only the pager button calls `chat.history`. The same review led to denying an initial empty state on failed reads and distinguishing verified empty tool groups from omitted or limited replay.
- The Core review checked identical store ordering, preserved legacy pagination, and spotted that serde rejected a non-string cursor before admission. The handler now decodes the scrollback shape after admission; the regression probes malformed string and numeric cursors against owned, foreign, and missing sessions.

## Limits / 未验证或未改变

- Session selection is in memory, not persisted across process restarts. Older page reads do not change the active run; there is no background paging, stream replay, or automatic resend. The existing 500-row replay ownership horizon remains; the older prose itself has no dependency on tool replay.
- These runs cover one macOS desktop and one same-origin Chromium configuration, not native Windows/Linux, a Leptos end-to-end run, or Phase 11 cutover evidence. No terminal, Capability refresh, default host, Tauri release, or production configuration changed. No page-latency or memory claim is made.
- The continuity fixture's providerless execution is for transport, selection, and paging evidence. It does not establish durable run correlation on the existing simulated engine. The rich check uses the configured loopback fixture instead.
