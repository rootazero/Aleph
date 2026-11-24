# NSH S2 customizable shell QA / S2 可定制外壳真机 QA

Reproduction for the S2 shell receipt
([2026-11-23-nsh-panel-s2-shell-receipt.md](../../docs/superpowers/runs/2026-11-23-nsh-panel-s2-shell-receipt.md)).
One isolated Gateway (temporary `HOME`/`ALEPH_HOME`, random port, never 18790, TLS self-signed,
`panel_web_dir` set) serves both renderers of the same TSX source in `interfaces/panel`:

- the `tsx-panel` NSH shell over Desktop IPC (macOS), and
- the Web package at `/v1/panel/` in Chrome.

S2 外壳回执的复跑装置：一个隔离 Gateway 同时服务桌面嵌入面（Desktop IPC）与 Web 包（`/v1/panel/`），两者渲染同一份 TSX 源码。不触碰已安装服务、默认端口或用户主目录。

- `run.py`: starts the Gateway, runs the shell phases (`unavailable` without an IPC target, `connected`, `sessions`, `restored` after a layout edit plus a Gateway restart on a fresh socket path), mints a pairing ticket and runs the Chrome driver. Writes `report.json`, logs and screenshots into `--out`.
- `web-panel.cjs`: Playwright driver: CSP and `nosniff` on `/v1/panel/`, pairing form, real `health` / `sessions.list` / capability data, layout edit through the UI, reload restore, cookie loss back to pairing.
- `winlist.swift`: JSON lines for a process's windows, so `screencapture -l` can capture the shell window.
- `evidence/`: the run the receipt cites.

## Replay / 复跑 (macOS arm64)

```bash
just panel-tsx-bundle panel-tsx-web
cargo build --locked --bin aleph-server
cargo build --locked -p aleph-nsh-shell --features tsx-panel
cp target/debug/aleph-nsh-shell /tmp/aleph-nsh-shell-tsx   # keep it from later default builds
swiftc -O qa/nsh_s2_shell/winlist.swift -o /tmp/winlist
ALEPH_BROWSER_LIVE_CHROME_PATH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
python3 qa/nsh_s2_shell/run.py --server target/debug/aleph-server --shell /tmp/aleph-nsh-shell-tsx \
  --web "interfaces/panel/out-web/Aleph Panel" --winlist /tmp/winlist \
  --node-modules "$(git rev-parse --show-toplevel)/node_modules" --out /tmp/s2-shell-run
```

The shell windows open and are brought to the front for a few seconds each (Stage Manager
would otherwise capture a thumbnail). `osascript` needs Accessibility permission for the
terminal, and `screencapture` needs Screen Recording permission.

桌面阶段会依次打开并前置 Aleph 窗口数秒；终端需要“辅助功能”与“屏幕录制”权限。

## Memory Nebula fixtures / 记忆星云夹具

Append `--nebula normal` to the replay above. Use fresh output directories with `--nebula dense` and `--nebula off` to check dense links and a mounted-but-unavailable runtime. `--web-only` omits the `--shell` and `--winlist` requirements and runs real Gateway + Chrome on hosts without the macOS embedding. Use `ALEPH_BROWSER_LIVE_CHROME_PATH` to name the installed Chrome and pass the package directory containing `index.html` to `--web`.

Normal and dense fixtures put same-path notes in separate partitions. The driver checks the 100-note window, samples Chrome animation-frame intervals over 12 zoom-in/out cycles, clicks an actual painted node, searches and opens the exact personal note, verifies foreign notes are absent, and records framed graph/detail byte counts. The oversized detail must return one bounded error without a body or retry. Animation-frame intervals include idle/vsync time and are not GPU timings. No user notes, existing pairing credentials or running Gateway are used.

普通与密集用例使用隔离的同路径笔记夹具；停用用例验证入口仍存在、读取明确失败。`--web-only` 是浏览器证据，不代替 macOS Desktop IPC 证据。绘图预算、拾取与相似边的快速回归可用 Node.js 22.18+ 运行：

```bash
node --test interfaces/panel/modules/nebulaViewport.test.mjs
```
