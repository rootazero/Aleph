# S3 chat vertical slice — real-machine QA / S3 会话纵切真机 QA

Drives the TSX Chat module (`interfaces/panel/modules/Chat.tsx`) against one isolated Gateway on macOS:
the NSH desktop shell over Desktop IPC, and Chrome over Web v1 (`/v1/ws`, `/v1/panel/`). The Gateway has
no provider, so Core runs the simulated engine and streams real `stream.*` frames. Receipt:
[`docs/superpowers/runs/2026-11-24-nsh-panel-s3-chat-receipt.md`](../../docs/superpowers/runs/2026-11-24-nsh-panel-s3-chat-receipt.md).

在 macOS 上对一个隔离 Gateway 驱动 TSX Chat 模块：桌面壳走 Desktop IPC，Chrome 走 Web v1。Gateway 无
provider，Core 使用模拟引擎推送真实 `stream.*` 帧。

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
  --out <new evidence dir>
```

- Reuses the S2 fixture (`qa/nsh_s2_shell/run.py`): temporary `HOME`/`ALEPH_HOME`, random port (never 18790),
  self-signed TLS. It never touches an installed service.
- The desktop phase sends keystrokes with `osascript`, so the terminal needs Accessibility permission. Under a
  Chinese input method the first Return commits the composition and must not send; the second sends. The web
  phase asserts that the message reached Core exactly once.
- `web-chat.cjs` talks to `run.py` over stdout/stdin: it prints `RESTART_GATEWAY`, and `run.py` restarts the
  Gateway and writes back one fresh pairing ticket. After a restart the browser must pair again (cookie bindings
  live in Gateway memory).

## Evidence / 证据

`evidence/` holds the run cited by the receipt: `report.json`, `web-report.json` and the screenshots.
