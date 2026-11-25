# 2026-11-25 — NSH Chat transcript / 富转录验证收据

## Scope / 范围

Opt-in macOS NSH shell via Desktop IPC and the same TSX source under `/v1/panel/` in Chrome. One isolated Core, temporary homes and workspace, and a loopback Anthropic fixture. The fixture controls reply timing; the real Core executes the file tools. See the [QA command](../../../qa/nsh_s3_chat/README.md) and [current contract](../../reference/TRANSCRIPT_RENDERING.md#41-nsh-chat--nsh-会话).

本次只验证 opt-in NSH 和同源 Chrome。脚本模型控制回复时机，文件工具由真实 Core 执行；不访问真实工作区或线上模型。

## Evidence / 证据

- **macOS NSH / 原生：** The held-response screenshot shows prose, settled Write, more prose, settled Edit, then the final prose while Stop is still visible. Verified the edited file's exact contents in the temporary workspace. / 截图发生在终态之前；真实文件内容符合预期。
- **Chrome / 同源 Web：** Reopened the Desktop-created session and found one committed copy of the sent prompt. Replay carried two tool start/finish pairs and two saved diffs. Checked both changes, hover highlighting, reader Escape with focus return, Jump to bottom, callout, bare URL, Mermaid, and measured Context. For a separate real Write, asserted the rendered vertical order and two tool frames while the fixture withheld the terminal frame. Also checked IME, Stop, and history after Gateway restart and re-pairing. / 区分实时与历史回放，并验证重连、输入法和停止。
- **Authorization / 授权：** Core and transport fixtures cover bound owner versus foreign/missing reads, exhausted wire/page limits, expired output, expired credentials and absent feature grants. These are separate from the rich UI run; the UI fixture does not query another user's data.
- **Render cost / 增量开销：** One short chunk causes one row upsert and a 274-byte serialized Rust→TSX patch with either 10 or 500 completed history rows. This counts patch bytes, not network traffic or frame time. Replay selects at most 12 runs and 80 deltas, with a 384 KiB reply envelope; output is requested on expansion/page action under a 24 KiB wire ceiling and a 256 KiB client preview. / 这是局部补丁测量，不是 FPS 或网路带宽结论。

Selected artifacts / 留存证据: [sanitized check results](../../../qa/nsh_s3_chat/evidence-rich/checks.json), [native live](../../../qa/nsh_s3_chat/evidence-rich/desktop-live-tools.png), [Web live](../../../qa/nsh_s3_chat/evidence-rich/web-live-tool-order.png), [Web diff reader](../../../qa/nsh_s3_chat/evidence-rich/web-rich-diff.png), [Web Context](../../../qa/nsh_s3_chat/evidence-rich/web-rich-context.png). Test inputs and paths are from the isolated fixture only.

Build identities (SHA-256) / 构建身份:

| Artifact | SHA-256 |
| --- | --- |
| `aleph-server` | `7e483a4fc50973b02dea3bb3e54cdfda32f29238f4d4cfa6020d1538414e7331` |
| `aleph-nsh-shell` | `7c40c30a6ba2a5231bc9f3318881c6f62ea6504bdfd8768e2b330d38e5fbd8fa` |
| Web `index.html` (entry point only) | `0c7e20fb05176c947c47f34327c0d006f96501d89b1528ca192b750dfba9096d` |

Final validation / 最终验证: 285 shared-ui Rust, 38 NSH headless and 12 Web host tests passed. Also passed: 69 client library tests, 2 focused Core detail tests, targeted IPC grant test, shared-ui strict Clippy, NSH typecheck, native bundle, matched Web packaging, server and NSH shell builds, and `git diff --check`. The headless suite verifies anchored reading during an update and unsupported Mermaid fallback. Repo-wide `cargo fmt --all -- --check` finds pre-existing formatting differences outside the changed Rust files; the touched Rust files are formatted separately.

Independent review / 独立复审: Whole-branch review identified lifecycle, feature-grant, and replay regressions. Subsequent focused reviews checked the corrections. The last targeted review found no blocker in the run-scoped history reread: only a terminal frame accepted by the Rust fold can request it, and only while that exact run remains unconfirmed. The review did not execute the host QA. / 整分支复审发现的问题分别修复并复测，最后一次只读复审确认补读绑定到终态轮次；真机结果来自独立执行的 QA。

## Limits / 未验证或未改变

- Native screenshots demonstrate streaming and rendering, not native keyboard focus return or pointer hover. Browser and headless evidence are reported independently; this is not Windows/Linux evidence or the release cutover gate.
- Older Core histories can contain both intermediate prose and an aggregated final assistant message. Panel preserves authoritative assistant rows instead of guessing by content; only raw tool-role rows are hidden. No exact historical interleaving is reconstructed. Tool cards remain individual, not grouped.
- No Tauri, Leptos, default mount, release workflow, TUI interaction, tool execution, or model-context change. No external provider for this QA, no live customer credentials, and no published artifact.

原生截图不证明键盘焦点归还或悬停；不代表 Windows/Linux 或发布门禁。旧历史正文保留 Core 原样，不按内容猜测去重；不改默认挂载与发布路径。
