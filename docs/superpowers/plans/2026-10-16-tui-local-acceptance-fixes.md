# Local terminal acceptance fixes / 本地终端验收修复

Date: 2026-10-16. Branch: `feat/tui-streaming-refactor`.

## Changes / 修复

- Remove the Chat rectangle and title. Header, transcript, scrolling and mouse regions use the full chat viewport; no residual one-cell border offset. Keep the narrow one-line latest-user chip and its measured jump target.
- Draw the composer between two thin horizontal rules, without corners or side borders. Empty input occupies three rows; multiline input remains capped.
- The failed run's final `RunError` now takes its sequence from the existing event emitter. The independent active-run bookkeeping counter could place it before an already emitted `RunComplete`, causing clients to discard the error as old.
- A TUI error receipt for a known, settled run stays visible without re-adopting that run and scheduling history replacement which would erase the receipt.
- Security-store v14 migration probes each `user_id` column before ALTER. A partially completed old upgrade must reopen the persisted key store, preserving its vault master key, rather than falling back to an unrelated in-memory key store. Existing encrypted provider credentials are retained.

中文：无回复问题包含凭据加载失败与错误回执被吞掉两层。修复保留现有保险库和主密钥，不要求先更换 MiniMax key；运行前已备份加密保险库和一致性 SQLite 快照。

## Verification / 验证

- `cargo test -p alephcore --lib v14_retry_preserves_master_token_when_identity_columns_already_exist -- --test-threads=1`: 1 passed. Fixture is memory SQLite with a synthetic master key; no real credentials used.
- `cargo test -p aleph-tui --lib -- --test-threads=1`: 546 passed, 1 existing live-Gateway test ignored. Tests cover borderless viewport coordinates, composer rules, sticky jump, and visible post-completion failures.
- Each Cargo command remains serialized behind the available-physical-memory guard, with one build job and dev debug information disabled.
- Runtime build, redeployment and a real model-reply probe are tracked separately below; compile success alone does not establish provider connectivity.

## Runtime acceptance / 运行验收

- Deployed code commit `b5fce8ca3` to `C:/Users/zou/AppData/Local/Aleph/tui-refactor`; both executable SHA256 hashes match the completed dev build. `/health` and `/ready` returned HTTP 200 with ready=true.
- Read-only comparison with the pre-recovery backup confirmed the persisted master key is unchanged; SQLite `user_version` advanced from 13 to 19.
- A real `agent.run` request returned `OK` via `stream.response_chunk`, followed by successful completion: 8 stream frames, 0 error frames. No replacement MiniMax key was required.

中文：已部署修复版并核对二进制哈希；健康与就绪检查通过。原主密钥保留，安全数据库从版本 13 升至 19；真实模型请求返回 `OK`，8 个流事件、0 个错误事件。无需更换 MiniMax key。

Manual appearance, mouse and opacity inspection remain the user's terminal acceptance. The independent read-only review timed out without findings; a direct review of the committed acceptance diff was completed. No full-workspace test rerun or merge to main was performed.

中文：外观、鼠标和透明度仍需用户在真实终端中验收。独立只读审查超时，未产生结论；已直接复查验收修复的提交差异。本轮未重跑全工作区测试，未合入 main。

## Follow-up: chat subscriptions / 追加：聊天订阅修复

The earlier direct WebSocket probe explicitly subscribed to `stream.*` and therefore established provider recovery only. The TUI instead first subscribed to runtime-agent and browser topics; that first explicit subscription replaced the Gateway's receive-all default with a filter that excluded every chat stream event. A sent message could finish at the provider without any response reaching the TUI.

中文：此前探针自行订阅了 `stream.*`，只能证明凭据与模型服务恢复。TUI 首次仅订阅侧栏与浏览器主题，Gateway 随即启用过滤，聊天事件全部被挡住，所以消息发送成功仍不显示回复。

- All TUI subscription paths (startup, reconnect, background refresh) now use the same complete topic set, including `stream.*`.
- Opt-in live regression `subscribed_reply_reaches_screen_after_startup_and_reconnect` failed before the change: zero frames in 20 seconds. After the change, both initial connection and reconnect produced assistant text and visible rendered replies (8 and 9 events respectively).
- TUI library suite: 546 passed, 2 ignored, including the new explicitly opt-in live test. Scoped all-target Clippy with `-D warnings` and the executable build passed. Cargo remained serialized through the memory guard.
- Native Windows console acceptance drove the rebuilt executable with real keyboard events and read its active console buffer. Two consecutive prompts both produced their distinct reply markers on screen; neither marker appeared verbatim in its prompt. The owned acceptance process was stopped after verification.

中文：启动、重连、后台刷新均保留聊天订阅。新增真实连接回归测试先复现零事件，修复后两次均在助手正文和渲染画面找到回复；常规测试 546 项通过，严格 Clippy 与构建通过。另用 Windows 控制台键盘事件驱动实际可执行文件，连续两条消息的回复均出现在屏幕，验收进程已结束。

## Follow-up: terminal ownership and scrolling / 追加：终端日志与滚动

The standalone TUI previously installed the shared file-and-console logger. Because its stdout is a terminal, asynchronous connection diagnostics wrote into the alternate screen while ratatui was drawing. The resulting terminal scrolling could place reply text below the composer. The TUI now uses the rotating, scrubbed file-only logger; initialization failure does not install a terminal writer. Other components retain their existing logging behavior.

中文：独立 TUI 原先启用了文件及控制台双路日志，连接诊断会直接写入全屏界面，引发错位。现在保持文件轮转、脱敏与级别过滤，全屏期间不再向终端写诊断日志。

The shared client also excludes the two exact Panel invalidation notifications (`stream.session_updated`, `stream.running_set_changed`) from transcript-loss detection, matching the Gateway frame census. Unknown stream methods and malformed supported frames still report continuity faults. PageUp/PageDown scroll the transcript while the composer has focus and preserve the unfinished draft; wheel events keep their existing transcript route.

中文：客户端按 Gateway 的既有豁免列表识别两种 Panel 通知，避免误报聊天帧丢失；真正未知或损坏的流事件仍会报告。输入框聚焦时可用 PageUp/PageDown 翻阅转录，草稿保持不变，滚轮继续走聊天滚动逻辑。

- TUI library: 547 passed, 2 opt-in live tests ignored; binary argument tests: 8 passed. Shared client: 48 passed, 1 ignored.
- Logging: 22 passed with two unrelated existing PII assertions excluded. The full logging run still fails `test_generic_secret_redacts_json_shaped_value` and `test_scrub_generic_password_assignment`; their implementation and tests are unchanged by this fix.
- Scoped all-target Clippy for TUI, client and logging with `-D warnings`, targeted rustfmt, and the TUI executable build passed. Cargo commands remained serialized through the memory guard.

中文：TUI 常规测试与参数测试、共享客户端测试、严格 Clippy、格式与构建通过。日志模块全量测试仍有两项既存 PII 断言失败；限定验证中其余 22 项通过，本次未改动这些 PII 实现及测试。

Native executable acceptance drove the rebuilt TUI with real Windows console keyboard and mouse-wheel events against the existing local Gateway. The assistant marker was above the composer separator and caret; no connection diagnostics appeared on screen. A fenced 65-line reply allowed wheel-up to hide its final row and wheel-down to restore the bottom. All four behavior assertions passed, and the owned acceptance process tree was stopped. Hidden legacy-console Unicode glyph appearance remains distinct from Windows Terminal visual acceptance.

中文：原生控制台验收驱动真实可执行文件与本地 Gateway，确认回复位于输入分隔线及光标上方、诊断日志不写入画面、滚轮可上翻并回到底部，四项行为断言全部通过。测试进程树已结束。隐藏旧式控制台的 Unicode 字形效果不作为 Windows Terminal 外观验收结论。
