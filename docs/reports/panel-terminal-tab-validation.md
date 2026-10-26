# Panel Terminal Tab validation / Panel Terminal Tab 验收记录

## Scope / 范围

本记录对应已合并的 Terminal Tab migration。它记录构建、静态浏览器 fixture 与当前 live Gateway 阻塞；静态证据不等价于真实 Gateway/PT​​Y 验证。

The migration is merged in `main` at `63623f160` (`merge(panel): integrate terminal tab migration`). This report records the completed build and static evidence and keeps the live Gateway limitation explicit.

## Implemented / 已实现

- Narrow PTY SDK with connection epoch, exact PTY topics and bounded event handling.
- Shared `aleph-terminal-ui` crate used by the legacy adapter and standalone module.
- Standalone `panel-tab-terminal` WASM entry with `aleph:terminal` identity, two DOM roots, one local owner/state and idempotent UI cleanup.
- Opt-in `panel-modular` host navigation while the default legacy build remains available.
- Same-build host + terminal WASM packaging under `just wasm-modular`.
- Versioned module entry: `./tabs/terminal/cb296d71a70d9eaa/terminal.js`.

## Commands and evidence / 命令与证据

- `cargo test --lib` in `interfaces/webchat/tabs/terminal`: 5 passed, 0 failed.
- WASM check and WASM clippy with `-D warnings`: passed for the standalone module.
- `just wasm-modular`: host and terminal wasm-release builds, wasm-bindgen, wasm-opt, Tailwind, manifest generation and asset checks passed.
- `PANEL_TABS_OUTPUT=.panel-tabs bun run panel:tabs:test`: 5 passed, 0 failed.
- Static Playwright fixture: 1 passed, 0 failed. It requested the page, loader, host JS/WASM, CSS, versioned terminal JS/WASM and checked HTTP 200, no console errors and no failed requests.
- `node --test tests/panel_loader.test.mjs`: 8 passed, 1 skipped because generated feature glue was not provided.
- Commit hook: `cargo fmt --check` and alephcore clippy passed.

## Size estimate / 体积估算

The candidate report used the same per-file Brotli quality 11 estimate:

- First-page assets: 70,448,615 raw bytes / 3,970,390 Brotli bytes.
- Terminal module: 662,689 raw bytes / 141,605 Brotli bytes.
- Combined candidate: 71,111,304 raw bytes / 4,111,995 Brotli bytes.

These are build-time transfer estimates. They are not browser timing, WASM instantiation timing, RSS, or OS memory-reclamation measurements.

## Live limitation / Live 阻塞

`opencode service status` returned `/usr/bin/bash: opencode: command not found` in the implementation environment. Therefore the following remain unverified:

- real Gateway Shell/PTY/WebSocket path;
- echo marker rendered by the real terminal Canvas;
- Unicode/bracketed paste and resize against a real session;
- switch away and back while preserving the same PTY;
- transport drop/reconnect and epoch recovery;
- real legacy/modular parity.

The next session must either run this smoke test against an already authorized disposable Gateway or record `BLOCKED` again. It must not restart, replace, or interfere with a user's existing service or PTY sessions, and must not relabel fixture evidence as live evidence.
