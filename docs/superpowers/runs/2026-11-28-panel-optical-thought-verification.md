# Optical chat verification / 光学聊天验证

## Scope / 范围

The default Leptos mounts share the same Panel bundle. This run covers the optical
chat changes in [the approved design](../specs/2026-11-27-panel-optical-thought-design.md)
and Task 7 in [the implementation plan](../plans/2026-11-27-panel-optical-thought.md).
The feature branch is `feat/panel-prism-experience`, based on `3786eed0d`.
当前 Web 与桌面 WebView 共享 Leptos 资源。本记录对应获批设计及实施计划的任务 7。

| Case / 场景 | Evidence / 证据 |
| --- | --- |
| Real context level and detail / 实测水位与明细 | The browser fixture restores 128,000 of 512,000 tokens, opens the detail by keyboard, restores focus on Escape, and preserves the same water level at phone width. / 浏览器从历史恢复 25%，键盘打开明细，Esc 恢复焦点，窄屏保持同一水位。 |
| Session and motion cleanup / 会话与动效清理 | Late details are discarded on session switch. Estimates render without a measured fill. Compaction sends 25% → 50% → 10%; hiding the page settles the surface and the working companion. / 切会话丢弃迟到明细，估值仅显示标签，压缩更新实测值，隐藏页面停止水面和像素伙伴动效。 |
| Theme and motion preferences / 主题与动态偏好 | The same composer renders distinct gradients in dark and light schemes. Reduced motion keeps the static level readable. / 同一输入区在明暗主题下采用不同渐变，减少动态时保留静态读数。 |
| Thought reading / 思考阅读 | The UI exercises original, bilingual and translated views, discloses each destination before the click, and submits one typed block reference and recipient revision. It renders an AI delta before the final translation. The Chinese phone case verifies cancellation and rejection of a late completion. / 原文、双语和译文模式均走真实组件，目标在点击之前可见，提交块引用与接收方版本。AI 译文在完成前显示增量。中文手机场景覆盖取消及丢弃迟到译文。 |

The [browser cases](../../../interfaces/webchat/tests/e2e/optical-chat.spec.ts)
load the built WASM, with deterministic replies supplied at the WebSocket boundary.
The [Core cases](../../../src/gateway/handlers/trace_replay/thought_source_tests.rs)
cover owner-scoped replay, both registered adapters, recipient replacement and
revocation before delivery. / 浏览器运行真实 WASM，WebSocket 边界使用固定应答。
Core 测试覆盖归属、两个注册引擎、接收方改向及送出前撤权。

To rerun the browser cases, serve `interfaces/webchat/dist` on `127.0.0.1:18791`
and run `npx playwright test tests/e2e/optical-chat.spec.ts` from
`interfaces/webchat` with Playwright and Chromium installed. The cases intercept
`/ws`, so the static server is sufficient. / 复跑时在 `127.0.0.1:18791` 提供
`interfaces/webchat/dist` 静态资源，并在 `interfaces/webchat` 中执行上述命令。
需要 Playwright 和 Chromium，测试自行接管 `/ws`。

## Results / 结果

| Command or environment / 命令或环境 | Result / 结果 |
| --- | --- |
| `git diff --check` | Passed before final bundle commit. / 发布资源提交前通过。 |
| `cargo fmt --all --check` | The baseline and feature both report the same 29 files. The changed Rust sources pass targeted formatting checks. / 基线和分支均报告同样 29 个文件，改动文件单独检查通过。 |
| `cargo test -p aleph-panel -p shared-ui-logic -p aleph-protocol --lib --jobs 1` | 1,580 Panel, 443 protocol and 256 shared UI tests passed. / Panel 1,580 项、协议 443 项、共享 UI 256 项全部通过。 |
| Initial `CARGO_BUILD_JOBS=1 just wasm` and four Edge cases | Optimized artifact built and four browser cases passed on the first integrated revision. / 首轮优化构建成功，Edge 四项通过。 |
| Final `CARGO_BUILD_JOBS=1 just wasm` | Optimized `wasm-release` compiled in 9m 15s. The WASM feature fence and WebView baseline check passed. All 41 JS references resolve against 44 WASM exports. The Brotli copies of JS, WASM, and CSS decompress to exactly the shipped bytes. / 正式资源通过 WASM 特性门禁、WebView 基线、JS 引用及 Brotli 内容核对。 |
| Final Edge and Playwright fixture / 最终浏览器验证 | 5 passed in 18.7s against the optimized shipping build. The phone case enters through the real History button. / Edge 在正式资源上通过 5 项，用时 18.7 秒，手机用例走实际历史入口。 |
| Windows NSH WebView2 / Windows 桌面 WebView2 | `cargo build -p aleph-nsh-shell --no-default-features --features qa-service --jobs 1` passed. Edge WebView2 `158.0.4370.45` loaded the final shipping bundle in the actual debug host with an isolated QA profile and loopback fixture. Verified 25% water level, keyboard-operated turn details, thought replay, destination disclosure and machine translation in the Chinese bilingual reader. / 调试宿主及隔离配置在实际 WebView2 加载最终资源，验证 25% 水位、键盘明细、思考回放、接收方披露和中文双语机器译文。 |
| Independent recipient and delivery review / 独立复审 | Both prior findings closed in the production path. A changed provider revision requires new disclosure, and queued text requires live recipient and Capability authorization before delivery. / 两项生产路径问题均关闭，接收方变更需重新披露，排队文本交付前复核当前接收方及能力授权。 |

The final `aleph_panel_bg.wasm` is 72,758,932 bytes with SHA-256
`0e855896e5c36b47a0a00282b6bfbb41839494fbdf0c2ce0de2f9ac6543436ba`.
The embedded Brotli copy is 3,858,211 bytes with SHA-256
`e5bb37cb7beb26b841b37706b68d91becff9243c1344a9ce200183ad4fc30c65`.
/ 最终 WASM 原件为 72,758,932 字节，嵌入的 Brotli 副本为 3,858,211 字节，
两者的 SHA-256 如上。

## Platform coverage / 平台覆盖

The host smoke used Windows NSH, and the browser suite used Edge with desktop
and simulated phone viewports. The macOS and Linux native WebViews, legacy
Tauri host and physical mobile devices remain unverified by this run. Both
browser and WebView used local fixtures for translation. / 本轮覆盖 Windows
NSH、Edge 桌面与模拟手机视口。macOS、Linux 原生 WebView、旧 Tauri 宿主和手机
真机仍待验证。浏览器与 WebView 的译文使用本地测试应答。
