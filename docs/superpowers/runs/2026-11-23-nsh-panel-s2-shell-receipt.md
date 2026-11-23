# S2 customizable shell receipt / S2 可定制外壳真机回执

## Scope / 范围

S2 exit criteria from the [roadmap](../plans/2026-11-22-nsh-panel-migration-roadmap.md) and the
[S2 design](../specs/2026-11-23-nsh-panel-s2-customizable-shell-design.md): Desktop and Web render the
same TSX source; the layout is restored after a restart; resources are released after dispose; old
handles are invalid. This receipt covers [plan](../plans/2026-11-23-nsh-panel-s2-customizable-shell.md)
Task 8 on macOS arm64 only.

本回执验证 S2 退出条件在 macOS arm64 上成立：桌面与 Web 渲染同一份 TSX 源码；重启后布局恢复；
dispose 后资源释放；旧句柄失效。Linux / Windows 不在本回执内。

## Build under test / 被测构建

- Branch `nsh-panel-s2-spine`, tree at `80435029f` (Task 7) plus the QA driver committed with this receipt. `main` untouched at `7f263df93`.
- `interfaces/panel` git tree `6dc33e1cd0dd2bcefa23b97a1d3c09d71c6faafe`, unchanged since `e9162f26e`.
- `just panel-tsx-bundle`: `bundle.js` sha256 `63a66cbc848ddd48e94dbaa1744a854710d8caea3ad43a27c53fadd9c3525b7c` (17.9 KB), `capabilities.json` sha256 `facfd1b500b68e2863a8f35104abafcebf5e32dbca44b36a161dd42a04c4c3f2`.
- `just panel-tsx-web`: `Aleph Panel/index.html` sha256 `e08a6f87714bb1933928c6fc10d1c232f792507316c224265e9844c526605e37`; NSH reports 14.8 MB (25 MB on disk with the `.br`/`.gz` siblings).
- Debug binaries: `aleph-server` sha256 `7efa4a27…66dbfd`, `aleph-nsh-shell --features tsx-panel` sha256 `ea7600bb…c4a9f8` (full hashes in the [JSON](2026-11-23-nsh-panel-s2-shell-receipt.json)). Debug shell size: default 69,694,704 bytes, `tsx-panel` 135,509,440 bytes (+65.8 MB, mostly the QuickJS runtime and debug info). Release size not measured.
- macOS 27.2.1 arm64, Rust/Cargo 1.99.1, Node 26.13.0, Chrome 158.0.8127.37, `nsh 0.6.0 (5541637)`.

## Real-machine run / 真机运行

Driver [`qa/nsh_s2_shell/run.py`](../../../qa/nsh_s2_shell/README.md): one isolated Gateway (temporary
`HOME`/`ALEPH_HOME`, random port, TLS self-signed, `panel_web_dir` set, `--desktop-ipc <root>/gw-N.sock`).
No installed service, default port 18790 or user home was touched. Evidence: [`report.json`](../../../qa/nsh_s2_shell/evidence/report.json), [`web-report.json`](../../../qa/nsh_s2_shell/evidence/web-report.json).

### Desktop (embedded TSX surface over Desktop IPC) / 桌面

The shell ran with `ALEPH_SHELL_PANEL=tsx`, its window brought to the front and captured by window id.

| phase | setup | result | threads |
|---|---|---|---|
| `unavailable` | no `ALEPH_SHELL_PANEL_IPC` | "Aleph is not reachable from here", no pairing form ([png](../../../qa/nsh_s2_shell/evidence/desktop-unavailable.png)) | 18 |
| `connected` | IPC `gw-1.sock` | Overview "Gateway: healthy", endpoint `desktop-ipc`; default layout written under `aleph.panel.layout.v1:desktop-ipc:<subject>` ([png](../../../qa/nsh_s2_shell/evidence/desktop-connected.png)) | 12 |
| `sessions` | stored `active` = Sessions | real `sessions.list`: "No sessions yet"; stored layout not rewritten ([png](../../../qa/nsh_s2_shell/evidence/desktop-sessions.png)) | 10 |
| `restored` | stored order Capabilities, Overview, Sessions; Sessions hidden; Gateway restarted on `gw-2.sock` | tab strip "Capabilities, Overview, Customize", Capabilities active listing three enabled tabs; same layout key and stored value after the run ([png](../../../qa/nsh_s2_shell/evidence/desktop-restored.png)) | 11 |

- The layout key survives a Gateway restart on a new socket path because the Desktop endpoint name is the stable `desktop-ipc`, and the `subject` is the Gateway's HMAC of the principal, not the path.
- Each run ended with `SIGTERM` to the shell (exit −15); the Gateway exited 0 both times.
- The thread column is one sample per launch, a few seconds after the window came up; it is not a teardown measurement.

### Web (same TSX source at `/v1/panel/`) / Web

Chrome via [`web-panel.cjs`](../../../qa/nsh_s2_shell/web-panel.cjs), pinning the Gateway's SPKI:

- `GET /v1/panel/` → 200 with `Content-Security-Policy: script-src 'self' 'wasm-unsafe-eval'; img-src 'self' blob: data:; connect-src 'self'` and `X-Content-Type-Options: nosniff`; the shell shows the pairing form ([png](../../../qa/nsh_s2_shell/evidence/web-pairing.png)).
- A ticket from `aleph-server pair --user u-owner` pasted into the form → cookie `__Host-aleph-v1` (HttpOnly, Secure, SameSite=Strict) → "Gateway: healthy" ([png](../../../qa/nsh_s2_shell/evidence/web-overview.png)), Sessions "No sessions yet", Capabilities listing three enabled tabs.
- Customize → "Move Capabilities up" twice → "Hide Sessions" → Done: localStorage `nsh:com.aleph.panel:aleph.panel.layout.v1:https://127.0.0.1:<port>:<subject>` holds the edited layout; the Sessions tab is gone.
- Reload → the cookie still authenticates, the stored value is identical, and the tab order starts with Capabilities ([png](../../../qa/nsh_s2_shell/evidence/web-restored.png)).
- Clear cookies and reload (standing in for cookie expiry, TTL 300 s) → pairing form, no blank shell ([png](../../../qa/nsh_s2_shell/evidence/web-cookie-cleared.png)).
- The Web subject equals the Desktop subject for the same principal (`subject_matches_desktop: true`), while the endpoint part keeps the two layouts separate.
- Console: two expected `/v1/ws` 401 failures (before pairing and after the cookie was cleared), one 404 for a resource the console did not name (likely the browser's root `/favicon.ico`, which `/v1/panel/` does not serve), and `Unrecognized feature: 'web-share'` warnings from Chrome.

## Covered by automated tests (re-run for this receipt) / 自动化覆盖

- `just panel-tsx-test`: `nsh test interfaces/panel` 15/15 — 5 Rust (`aleph-panel-ui`), 6 App (`layout round-trips through storage`, `a stored tab the gateway no longer offers renders unavailable`, `a DOM-ABI community tab shows unavailable in this renderer`, `a hidden module releases its subscriptions`, `a refused session shows the pairing form and pairs`, overview health), 4 SDK (`invoke after dispose rejects`, `a late result after dispose is dropped`, `a result from an older generation is dropped`, `dispose releases every subscription`); `nsh check` clean; `node --test interfaces/panel/web/host.test.mjs` 9/9 (golden frames, out-of-order correlation, oversize frame, hello first, missing WebSocket).
- `cargo test -p aleph-nsh-shell --features tsx-panel`: 24 + 8 + 2 pass, including `second_surface_is_refused`, `rpc_handler_maps_client_error_to_error_reply`, `remote_target_reports_unavailable`, `rpc_without_a_connection_is_host_unavailable`.
- Earlier tasks, unchanged since: Gateway `/v1/panel/` routes (absent when unconfigured, CSP, traversal), `V1_METHODS` on IPC and Web v1, `HelloResult.subject`, `PanelLayout` restore/repair (13), v1 golden frames (2).

## Not verified / 未验证

- **Teardown on the production surface.** Hidden-module release and old-SDK rejection are proven in the headless runner; JS timers, held Rust objects, the `AppView` drop and thread return to baseline across repeated mounts are proven only by the [runtime spike](2026-11-23-nsh-panel-s2-runtime-spike.md) (Q4). The production shell has no toggle that unmounts and remounts its surface, so no three-cycle measurement was taken on it.
- **Desktop layout editing through clicks.** The desktop run edited storage while the shell was down; the edit UI was driven in Chrome and in `nsh test` on the same TSX source.
- **Real cookie expiry.** Simulated by clearing cookies, not by waiting out the 300 s TTL.
- Linux and Windows (the surface is macOS-only in S2 and `tsx-panel` refuses to compile elsewhere); release binary size; bytecode bundles; signed packages; the default renderer switch; DOM-ABI community tabs inside the TSX shell (shown as unavailable by design).

## Spike reproduction / 探针复现装置

The throwaway spike under `qa/nsh_s2_spike/` is removed together with this receipt. Its sources and
evidence remain at commit `ec2d264ac` (`git show ec2d264ac:qa/nsh_s2_spike/README.md`).

一次性探针目录随本回执删除，原始代码与证据保留在 `ec2d264ac`。
