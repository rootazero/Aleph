# S3 chat vertical slice — real-machine QA / S3 会话纵切真机 QA

Drives the TSX Chat module (`interfaces/panel/modules/Chat.tsx`) against one isolated Gateway on macOS:
the NSH desktop shell over Desktop IPC, and Chrome over Web v1 (`/v1/ws`, `/v1/panel/`). The Gateway has
no provider, so Core runs the simulated engine and streams real `stream.*` frames. Receipt:
[`docs/superpowers/runs/2026-11-24-nsh-panel-s3-chat-receipt.md`](../../docs/superpowers/runs/2026-11-24-nsh-panel-s3-chat-receipt.md).

在 macOS 上对一个隔离 Gateway 驱动 TSX Chat 模块：桌面壳走 Desktop IPC，Chrome 走 Web v1。Gateway 无
provider，Core 使用模拟引擎推送真实 `stream.*` 帧。使用 `--rich` 时改用临时回环脚本模型；写入和编辑仍由真实 Core 工具执行。

## Run / 运行

```sh
just panel-tsx-bundle panel-tsx-web
cargo build --locked --bin aleph-server
cargo build --locked -p aleph-nsh-shell --features tsx-panel
swiftc -O qa/nsh_s2_shell/winlist.swift -o <scratch>/winlist
python3 qa/nsh_s3_chat/run.py \
  --server target/debug/aleph-server \
  --shell target/debug/aleph-nsh-shell \
  --web "interfaces/panel/out-web/Aleph Panel" \
  --winlist <scratch>/winlist \
  --node-modules <repo>/node_modules \
  --out <new evidence dir> [--rich]
```

- Reuses the S2 fixture (`qa/nsh_s2_shell/run.py`): temporary `HOME`/`ALEPH_HOME`, random port (never 18790),
  self-signed TLS. It never touches an installed service.
- The desktop phase sends keystrokes with `osascript`, so the terminal needs Accessibility permission. Under a
  Chinese input method the first Return may commit the composition; the second sends. An IME may also transform
  the fixture's prefix. Chrome finds the stable ASCII suffix and verifies the exact committed row appears once.
- `web-chat.cjs` talks to `run.py` over stdout/stdin: it prints `RESTART_GATEWAY`, and `run.py` restarts the
  Gateway and writes back one fresh pairing ticket. After a restart the browser must pair again (cookie bindings
  live in Gateway memory).

Use `--rich` to execute a real Desktop IPC Write/Edit and reopen its durable cards in Chrome. The fixture holds the final provider response for a native screenshot of the live run. Chrome checks recorded changes, tool hover, fullscreen/Escape and keyboard focus return, Markdown, Context, and Jump to bottom. It also sends a separate Write and verifies text → tool → text while the provider response is still open. Set `ALEPH_BROWSER_LIVE_CHROME_PATH` to a local Chrome binary if Playwright's Chromium is not installed.

`--rich` 不联网调用模型，不读写真实工作区。Desktop IPC 驱动真实写入/编辑，在终态前截图。Chrome 重开历史验证变更、悬停、全屏退出与焦点归还、Markdown、Context 和跳到末尾，并单独发起一次真实写入，在完成前断言正文 → 工具 → 正文顺序。可用环境变量指定本机 Chrome。

## Evidence / 证据

The selected output directory holds `report.json`, `web-report.json`, fixture-provider decisions and screenshots. See the [rich transcript receipt](../../docs/superpowers/runs/2026-11-25-nsh-chat-transcript-receipt.md) for the evidence boundaries. / 输出目录保留报告和截图；验证范围见富转录收据。
