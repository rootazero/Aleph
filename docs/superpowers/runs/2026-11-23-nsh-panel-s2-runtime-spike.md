# S2 NSH runtime spike / S2 NSH 运行时探针回执

> The reproduction under `qa/nsh_s2_spike/` was removed from the tree with the [S2 shell receipt](2026-11-23-nsh-panel-s2-shell-receipt.md); links below resolve at commit `ec2d264ac`. / 探针目录已随 S2 外壳回执删除，下文链接请在 `ec2d264ac` 查看。

## Scope / 范围

S2 (customizable Panel shell) chose approach B: keep the Rust-root NSH host in `desktop/nsh-shell` and embed an NSH `AppView` that runs the shared TSX shell as one native component, while the Web target packages the same TSX. Before writing the S2 design, this spike answered four questions on NSH 0.6.0 (`nsh 0.6.0 (5541637)`, the `v0.6.0` tag, matching the `=0.6.0` crate pins):

- **Q1:** an `AppView` running TSX embeds inside a Rust-root NSH host and renders.
- **Q2:** native components receive real props and deliver events, at the platform level (an `NSView` component registered by the host) and at the UI-set level (a Rust component used from TSX).
- **Q3:** the same TSX source renders as an NSH Web package in a real browser.
- **Q4:** after the surface unmounts, JS timers stop, Rust objects held by JS are freed, the `AppView` is dropped, and threads return to baseline.

方案 B：保留 Rust-root 宿主，把运行共享 TSX 的 `AppView` 作为一个原生组件嵌入；Web 端打包同一份 TSX。本探针在写 S2 设计文档前验证上述四问。探针代码是一次性复现装置，见 [`qa/nsh_s2_spike/`](../../../qa/nsh_s2_spike/README.md)，生产代码未改动。

## Evidence / 验证证据

Branch `nsh-panel-s2-spine` from `main` `7f263df93`. macOS 27.2.1 arm64, Rust/Cargo 1.99.1, Node 26.13.0, Chrome 158.0.8127.37, Playwright from the workspace `node_modules`. The spike sources and evidence files are committed together with this receipt.

- **Q1 PASS (desktop embed).** The [host](../../../qa/nsh_s2_spike/host/src/main.rs) registers `aleph/panel-surface` through `nsh_platform_macos::set_components`; its factory creates `AppView::new(mtm, AppOptions::default())` and loads `Bundle::read("out-bundle/bundle.js")`. The Rust root mounts it with `native(SURFACE)` inside `when(shown, …)`. JS reported `app-rendered:tracked`, and the window capture shows the TSX content ("Not pressed yet", the Badge, `pulse-from-tsx`, "Pulse fired") while mounted and an empty surface after unmount ([mounted](../../../qa/nsh_s2_spike/evidence/desktop-mounted.png), [unmounted](../../../qa/nsh_s2_spike/evidence/desktop-unmounted.png)). The window was never foregrounded, so both captures are Stage Manager thumbnails, not full-size frames.
- **Q2 PASS (props and events, both levels).** Platform level: `set_prop` received `label="from-rust-root"` from the Rust root, and `ComponentContext::emit("loaded", …)` reached the root's `on_native_event` listener. UI-set level: the spike crate's `Pulse` recorded `pulse-props:pulse-from-tsx` from TSX props and its `Fire` event, emitted by a `set_timeout` with no input, ran the TSX `onFire` handler (`pulse-fired`). The same props and events recur in every cycle of the [three-cycle report](../../../qa/nsh_s2_spike/evidence/host-three-cycles.json).
- **Q3 PASS (same source on Web).** `nsh package --target web` built the identical `App.tsx` plus the Rust crate compiled to wasm (7.5 MB). Headless Chrome via [web-probe.cjs](../../../qa/nsh_s2_spike/tools/web-probe.cjs) saw "Pulse fired", then two Badge presses produced "Pressed twice" from the wasm `Counter` ([screenshot](../../../qa/nsh_s2_spike/evidence/web-pressed-twice.png)). Console noise: a `/favicon.ico` 404 and an `Unrecognized feature: 'web-share'` warning. `nsh test app` also passed (1 test).
- **Q4 PASS (teardown, three cycles).** Each cycle mounts at +0.5 s, samples and unmounts at +2.5 s, and samples again at +4.0 s ([three cycles](../../../qa/nsh_s2_spike/evidence/host-three-cycles.json)):

  | phase | ticks | live `Tracked` | threads |
  |---|---|---|---|
  | baseline | 0 | 0 | 8 |
  | cycle 0 mounted / +1.5 s | 38 / 38 | 1 / 0 | 15 / 14 |
  | cycle 1 mounted / +1.5 s | 76 / 76 | 1 / 0 | 9 / 8 |
  | cycle 2 mounted / +1.5 s | 114 / 114 | 1 / 0 | 9 / 8 |
  | final (+4 s hold) | 114 | 0 | 7 |

  The JS `setInterval` stops at every unmount, the JS-held `Tracked` is dropped, and every `PanelSurface` drop saw `Rc::strong_count(&AppView) == 1`, so the `AppView` was freed. Threads do not grow across cycles. An external window list counted two windows throughout: the main window plus one 500×500 offscreen window created on the first mount, which stays for the life of the process and does not grow per surface.

## Design consequences / 对设计的约束

1. **The production shell cannot run TSX today.** `desktop/nsh-shell/Cargo.toml` builds `nsh-platform-macos` with `default-features = false`, which drops the `js` feature (Linux and Windows enable only `webview`). Embedding TSX means turning `js` on for the shell; the binary-size and build-time cost is not measured yet.
2. **Bytecode needs a trusted source.** `Bundle::read` refuses `bundle.nshc`; only `Bundle::read_trusted` (the app's own resources) or a verified `.nshpkg` may load bytecode. A plain `bundle.js` read from a writable path runs whatever file is there, so the production loader must read from the signed app resources or a verified package, tied to the existing updater trust chain.
3. **Root component code is not on the main thread.** Inside the Rust root, `MainThreadMarker::new()` returned `None`; native-component factories run on the main thread. AppKit work belongs in the factory and the `NativeComponent`, not in root components.
4. **Registrations are process-wide.** `set_ui_sets` and `set_components` apply to every `AppView`, so an embedded surface sees `aleph/panel-surface` too and could nest itself. The design must forbid or scope nesting.
5. **Teardown is asynchronous.** Timers and JS-held objects are gone by 1.5 s after unmount, but engine threads took up to about 3 s to return to baseline on the first cycle. Any "resources at zero" check must wait for quiescence rather than sample immediately.
6. **Timers are owner-scoped.** `set_timeout` returns a detached task (`nsh-core-0.6.0/src/element/task.rs:1023`); dropping the handle does not cancel it, and the owner's cleanup does.
7. **The Web package has build and serving requirements.** Building it on macOS needs `AR_wasm32_unknown_unknown` and `CC_wasm32_unknown_unknown` for `zstd-sys`. Serving it under a Content-Security-Policy needs `script-src 'self' 'wasm-unsafe-eval'; img-src 'self' blob: data:`, which the Gateway route serving it must send.

## Not verified / 未验证

- Linux (GTK `AppView`) and Windows (HWND `AppView`) embedding; only macOS ran. S1-B Linux native and Windows Named Pipe verification are still open as well.
- A full-size, foregrounded desktop frame; the captures are Stage Manager thumbnails.
- The `nsh/host` `HostChannel` (`set_params`, `send`, `handle`). The data path from Aleph Core into the embedded TSX was not exercised, only props and events.
- Invalidation of stale handles after dispose (for example a retained `ComponentContext` emitting after its element is gone). The S2 exit criterion "old handles invalid" will rest on the Core capability lease generation, not on this spike.
- Release-build binary size with `js` enabled, and the production loader through `Bundle::read_trusted` or a signed package.
- The file-descriptor count stayed at 7 in every sample; reading `/dev/fd` from the root does not show engine resources and proves nothing either way.

## Replay / 复跑

See [`qa/nsh_s2_spike/README.md`](../../../qa/nsh_s2_spike/README.md). It needs an `nsh 0.6.0` CLI with the `web` component, Chrome, and the workspace Playwright install; no Gateway, service, or credentials.
