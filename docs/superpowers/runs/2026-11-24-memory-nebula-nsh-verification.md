# 记忆星云验证 / Memory Nebula verification — 2026-11-24

## 范围 / Scope

Base `b2c00e624`; worktree branch `refactor/memory-nebula-nsh`. Core's shared read contract, the v1 mount, shared NSH renderer, and classic Panel changes were checked separately. No default renderer change, storage migration, NSH write surface, or existing user data was involved. / 在隔离分支分别验证 Core 读契约、v1 接线、共享 NSH 渲染与旧面板；未切换默认入口，未改存储格式和现有用户数据。

## 已验证 / Verified checks

| Surface / 表面 | Evidence / 证据 |
| --- | --- |
| Shared protocol / 共享协议 | 440 unit/integration tests on the shared protocol baseline, with two ignored doc tests; the exact-address wire regression passed separately. The graph SQL tie regression passed after ordering by path. |
| Gateway unit and transport fixtures / Gateway 定向与传输夹具 | Windows: publication 3, graph 24, RPC 8, Web v1 6, Desktop IPC 3 passed. Ran the normally ignored Chrome Web v1 case separately: 1 passed, including pairing, hello, member scope, graph-read allowlist, revocation and legacy WebSocket compatibility. The Desktop IPC tests used Windows named pipes in the test harness, not a native NSH Panel. Ordinary Core codegen ran out of memory before executing tests; the same source and selectors passed with a serial LLVM backend, debug info disabled and 512 CGUs. |
| Shared NSH component and SDK / 共享组件及 SDK | Pinned NSH CLI 0.6.0: `nsh check` passed; `nsh test` 20/20 in a minimal workspace containing the branch's Panel, protocol and shared UI sources. Tests cover absent/disabled/malformed publication, exact org/personal addressing, off-window detail, identity invalidation and stale SDK success/rejection. Test hosts are mocked. Node web-host and projected draw/pick tests: 12/12. |
| Classic Panel / 旧面板 | Native Rust selectors `memory_graph`, `galaxy`, and `views::memory::data` passed 24, 68 and 35 tests respectively. Both classic and opt-in `panel-modular` WASM `cargo check` builds passed (`panel-modular` uses `--no-default-features`). The modular build includes the TSX-only declaration filter before the DOM registry. This is not a classic browser gesture or edit round-trip. |
| Shared Rust scene / 共享 Rust 场景 | `cargo test -p shared-ui-logic --lib nebula`: 9 passed, including a 500-node deterministic solver fixture and the distinction between reciprocal rows and one spring. The 500-note fixture is not a product request window or a frame-rate measurement. |
| Candidate Web package / 候选 Web 包 | `nsh package --target web` produced 17.3 MB using local LLVM 20.1.8 and reduced-memory profile overrides `CARGO_PROFILE_WEB_LTO=false CARGO_PROFILE_WEB_CODEGEN_UNITS=16`. The stock fat-LTO host build exhausted memory; this is not a release-size measurement. |

## 实际 Gateway 与 Chrome / Production Gateway and Chrome

Use the [isolated fixture recipe](../../../qa/nsh_s2_shell/README.md). The `normal` and `dense` datasets are identical except for one versus 24 outgoing links per ordinary note. The fixtures carry personal, organization and foreign notes with the same path, plus a deliberately oversized escaped UTF-8 note. The `off` dataset keeps the three reads mounted while the runtime is disabled. Real browser timings here are not interchangeable with the classic 3D renderer; the prior NSH shell had no Nebula canvas.

An isolated preflight routed only the graph replies through a scripted fixture while the previous Gateway served the packaged UI and handled real pairing. Chrome painted the 100-node scene, selected a node by its zoomed pixel, and the actual Canvas stroke reached the incident-edge highlight color. This proves the Web drawing and interaction path, not candidate Core data, authorization, or wire-size behavior. / 脚本响应预检验证了真实 Chrome 绘制、缩放后拾取与选中边高亮，不作 Core 或传输证据。

**Pending at this checkpoint:** Candidate Gateway startup and all three browser runs. The previous Gateway binary with the final Web package passed the isolated pairing, modules, persisted layout, and cookie-loss flow. It was also used to preflight fixture indexing (2,400 active dense links), not to verify the new graph capability. / 候选 Gateway 的实际回放待完成；基线 Gateway 仅验证夹具和原有壳流程。

## 评审和未覆盖门禁 / Review and outstanding gates

Independent source review initially blocked on cross-partition bare wikilinks and directional arrows on virtual similarity edges. The follow-up found no code blocker: markdown links now follow active Core resolutions in the source partition, and the shared Canvas has a distinct non-directional stroke for similarity. / 独立复审确认两项阻塞已修复，无代码阻塞。

No macOS native NSH renderer was exercised on the Windows host. Windows pipe and real-browser checks do not stand in for macOS Desktop IPC + Core Graphics; keep the native sign-off open. The legacy camera/3D view, standard launch defaults, release packaging/publishing, and existing user installations were not changed. / macOS 原生绘制与 Desktop IPC 仍待真机验收；未改旧相机、默认入口或现有安装，未发版。
