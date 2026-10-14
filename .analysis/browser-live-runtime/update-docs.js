const fs = require('fs');
const root = require('path').resolve(__dirname, '../..');
const p = (x) => require('path').join(root, x);
function edit(file, fn) {
  const before = fs.readFileSync(p(file), 'utf8');
  const crlf = before.includes('\r\n');
  const after = fn(before.replace(/\r\n/g, '\n'));
  if (after === before.replace(/\r\n/g, '\n')) throw new Error('no change: ' + file);
  fs.writeFileSync(p(file), crlf ? after.replace(/\n/g, '\r\n') : after);
  console.log('updated', file);
}
const EV = 'docs/reports/verify-20261007/browser-live-e2e.md';

edit('.superpowers/sdd/2026-10-07-panel-browser-workspace/task-23-acceptance-report.md', (s) => {
  const lines = s.split('\n');
  const i13 = lines.findIndex((l) => l.startsWith('| 13 | Real browser / WS E2E'));
  const i78 = lines.findIndex((l) => l.startsWith('| WebSocket lifecycle E2E'));
  if (i13 < 0 || i78 < 0) throw new Error('rows not found');
  lines[i13] = '| 13 | Real browser / WS E2E | **PARTIAL: 6/7 real selectors exit 0; managed-companion BLOCKED (no production producer)** | 2026-10-11 real run, reproduced on a rebuilt server: worktree aleph-server + real Chrome/CDP + Gateway WS + WASM standalone viewer DOM + Playwright (system Chrome). Paired device bound to u-owner, real agent turn `browser_open`, Safe decide CAS, fresh Frames-only grant per case. stream / control-all-entries / standalone-same-target / grant-boundary / binding-switch / reap exit 0; managed-companion exit 1 fail-closed (`from_launcher_record` / `DesignCompanion` have test-only callers). Only the LLM provider was mocked. Not covered: mid-stream revoke/TTL, live navigation fence, Panel in-app launch click, TUI launcher, Native IPC. | `' + EV + '` |';
  lines[i78] = '| WebSocket lifecycle E2E (consume -> exchange -> ws -> AuthOk -> frames -> close) | 6/7 selectors exit 0 (real Gateway/Chrome/WS/DOM); managed-companion BLOCKED | **REAL RUN 2026-10-11** |';
  const iTui = lines.findIndex((l) => l.startsWith('> **Current TUI status**'));
  lines.splice(iTui + 1, 0, '', '> **Real E2E 2026-10-11 / 真实 E2E**: 6/7 browser-live selectors exit 0 against real Gateway-issued URLs, real Chrome/CDP, WS and WASM DOM (`' + EV + '`). managed-companion is BLOCKED: no production managed/design-companion producer exists. The run also found and fixed a stale embedded dist, Windows `run.sh` filters, and Panel Open/Dismiss buttons that had lost their `on:click` handlers (Panel lib 1519/1519, `just wasm` zero warnings after the fix). Task23 remains PARTIAL / not production ready.');
  return lines.join('\n');
});

edit('docs/reference/BROWSER_LIVE_VIEWER.md', (s) => {
  const old = '尚无本机运行结果；没有命令退出 0 前不得读作通过。';
  if (!s.includes(old)) throw new Error('anchor missing');
  return s.replace(old, '2026-10-11 真实运行（Windows；真实 Gateway / Chrome / CDP / WS / WASM DOM，仅 LLM provider 为 mock）：`stream`、`control-all-entries`、`standalone-same-target`、`grant-boundary`、`binding-switch`、`reap` 退出 0；`managed-companion` 因不存在生产 managed / design-companion producer 而 BLOCKED（runner fail closed）。Real run: 6/7 selectors exit 0, managed-companion BLOCKED. 运行前须 `just wasm` 并重建 `aleph-server`（dist 为 rust-embed 编译期嵌入）；可用 `ALEPH_BROWSER_LIVE_CHROME_PATH` 指定客户端 Chrome。证据 → `' + EV + '`。');
});
