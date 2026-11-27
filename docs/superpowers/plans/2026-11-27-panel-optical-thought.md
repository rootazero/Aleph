# Aleph Panel 光学界面实施计划 / Optical Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**目标 / Goal**  在现行 Leptos Panel 交付原创光学界面、统一阻尼动效、实测上下文小瓶、像素伙伴和双引擎思考阅读器。 Ship these experiences through the current Panel's Web and desktop mounts.

**架构 / Architecture**  Core 提供会话、上下文及 Capability 授权事实。共享 Rust 投影供呈现层稳定消费。宽屏和手机复用组件与偏好。 Core owns session and capability facts, a shared Rust projection serves renderers, and wide and phone layouts share components and preferences.

**技术 / Tech stack**  Rust, Leptos CSR/WASM, existing theme CSS, Canvas 2D, Tokio, JSON-RPC, Aleph Capability, MCP, existing provider registry, Playwright.

**规格 / Spec**  [已批准设计 / Approved design](../specs/2026-11-27-panel-optical-thought-design.md).

## 全局约束 / Global Constraints

- 提交落在 `feat/panel-prism-experience`。默认 Web 与桌面宿主同源，TSX 可复用共享投影与契约。 Keep the shipping Leptos renderer shared across hosts. Preserve the separate TSX rollout.
- R1–R10。Core 授权和解析数据，Panel 只呈现，外部可组合的能力经 Capability。 Use existing scope, leases, providers and session persistence. Reuse existing Tokio work and cancellation where execution can overlap.
- 原创代码、配色、角色及资产，沿用 MIT。 Use original implementations and assets.
- `512K` 来自实际模型窗口或测试样例。`stream.context_gauge`、终态和历史的实测值与 `chat.context_estimate`、累计 `session.usage` 分开处理。 Distinguish measured occupancy, capacity-only, and estimates.
- 机器和 AI 共用当前 Panel 界面语言。每块显式触发并展示接收方。 Invalidate in-flight and displayed translations on session or locale changes. Keep translations ephemeral.

## 审查重点 / Review Focus

| 情况 / Condition | 预期 / Expected behavior | 任务 / Task |
| --- | --- | --- |
| 512K 对话换用容量未知的模型，或换会话时旧历史回包 | 保留可归属的最后一次测量，未知状态清晰呈现，每次请求的占用和窗口成对。 Preserve measurement attribution and discard stale hydration. | 1 |
| 已停止的动画收到延迟帧，或页面切后台 | 终态正确，释放绘制与监听，键盘位置和静态水位可用。 Release rendering and retain keyboard and static behavior. | 2–3 |
| 复合思考流包含工具、斜杠命令或旧记录 | 仅以 Core 能证明来源的已完成模型块为翻译输入。 Resolve only settled model blocks with trusted provenance. | 4 |
| 工具撤权、配置指向新接收方、重连取消 | 原授权或原接收方失效即停止发送或输出。 Stop dispatch or delivery upon invalidation. | 5 |
| 手机上的双语布局遇到请求错误、切语言和晚到分片 | 原文可读，旧片段不会进入新语言或新会话。 Preserve original text and reject late chunks. | 6–7 |

## 文件分工 / File Map

| 模块 / Module | 文件 / Paths |
| --- | --- |
| 上下文类型、数据归并 / Context projection and ingress | `shared/ui_logic/src/transcript/{mod.rs,context.rs}`, `interfaces/webchat/src/components/chat_sidebar.rs`, `interfaces/webchat/src/platform/wide/views/chat/{state/mod.rs,events.rs}`, `interfaces/webchat/src/api/chat.rs` |
| 光学主题和动效 / Optical tokens and motion | `interfaces/webchat/styles/tailwind.css`, `interfaces/webchat/src/components/{mod.rs,motion.rs}`, existing chat and navigation component classes |
| 小瓶和伙伴 / Vial and companion | `interfaces/webchat/src/platform/wide/views/chat/{mod.rs,context_gauge.rs,context_vial.rs,pixel_companion.rs,composer/mod.rs}`, `interfaces/webchat/src/platform/phone/chat/composer.rs`, `interfaces/webchat/locales/{zh,en}.json` |
| 块来源与授权 / Thought source and access | `shared/protocol/src/{lib.rs,thought.rs}`, `src/gateway/handlers/{trace_replay.rs,thought_translation.rs}`, existing authoritative model-thought producer and trace presentation projection |
| 引擎与 dispatch / Translation engines and dispatch | `src/capability/`, `src/config/`, `src/tools/scoped/`, `src/gateway/{handlers/mod.rs,method_visibility.rs}`, `src/bin/aleph-server/commands/start/builder/agent_init/common_handlers.rs` |
| 阅读器 / Reader | `interfaces/webchat/src/api/{mod.rs,thought.rs}`, `interfaces/webchat/src/platform/wide/views/chat/{reasoning.rs,messages.rs,events.rs}`, `interfaces/webchat/src/components/chat_sidebar.rs`, locale resources |
| 生产接线证据 / Runtime evidence | `interfaces/webchat/tests/e2e/`, `docs/reference/{FEATURE_LOCATOR,DESKTOP_SHELL}.md`, plus the owner reference for the new Capability |

---

### Task 1: Context projection and history / 上下文投影与历史恢复

**接口 / Interfaces.** `shared_ui_logic::transcript::ContextOccupancy`, re-exported from `context.rs`, represents `Measured { used_tokens: u32, window_tokens: u32, total_tokens: Option<u64> }`, `CapacityOnly { window_tokens: u32 }`, or `Unknown`. Expose `pub fn project_occupancy(used_tokens: Option<u32>, window_tokens: Option<u32>, total_tokens: Option<u64>) -> ContextOccupancy`. The Leptos state keeps the existing independent run metrics and estimate classification. `ContextBreakdown` is consumed through the existing `panel_context_view` projection. Keep the context projection importable from both renderers without enabling the experimental tab renderer.

- [ ] 在共享投影及现有历史测试中加入 `measured_zero_is_valid`, `capacity_only_has_no_fraction`, `newer_unknown_model_does_not_borrow_old_window`, `late_history_cannot_replace_live_occupancy`, and acceptance of `128_000 / 512_000 == 0.25`. Test missing, zero and overflowing wire fields, plus compaction decreasing the next live measurement.
- [ ] Run `cargo test -p shared-ui-logic --lib` and targeted Panel history/event tests. Confirm the new assertions fail against current behavior.
- [ ] Implement the shared projection. Route `apply_context_gauge` and `occupancy_from_history` through it and preserve `await_current_history` isolation. A provider zero is a measurement when valid fields are present. A zero or absent window yields unavailable occupancy. Reuse `chat.context_estimate` only as explicitly labelled estimated information. Keep capacity paired with the originating prompt and mark retained previous measurements as such.
- [ ] Run `cargo test -p shared-ui-logic --lib` and `cargo test -p aleph-panel --lib`. Confirm restore, older records, live ordering and compaction pass.
- [ ] 提交上下文数据 / Commit the projection and ingress together.

### Task 2: Optical surfaces and damped motion / 光学界面与阻尼动效

**接口 / Interfaces.** `components::motion::Spring` supplies `pub fn retarget(&mut self, target: f64)` and `pub fn advance(&mut self, seconds: f64) -> f64`. A Leptos-owned frame controller consumes it, publishes position and releases the pending callback on settle, visibility loss, preference change or disposal. `Spring` remains pure Rust. CSS uses existing theme semantic tokens as fallbacks.

- [ ] Add `spring_retarget_keeps_current_position`, `spring_settles_after_large_frame_gap`, and `reduced_motion_finishes_and_cancels_frame`. Assert reversal retains position and velocity, and disabled motion resolves without a pending callback.
- [ ] Run `cargo test -p aleph-panel --lib motion` and confirm the regression assertions fail until the controller and solver exist.
- [ ] Implement the shared controller and bounded damped spring. Add scoped optical surface styles for navigation selection, composer, detail overlay and thought card. Reuse existing focus, font and theme semantics and keep touch and keyboard targets.
- [ ] Run the motion tests and `cargo check -p aleph-panel --target wasm32-unknown-unknown`. Verify dark and light contrast, visible focus and reduced motion in the browser slice.
- [ ] 提交主题和动效 / Commit the theme and motion together.

### Task 3: Context vial and pixel companion / 上下文小瓶与像素伙伴

**接口 / Interfaces.** Shared Leptos `<ContextVial />` reads the context projection and canonical selected session. `<PixelCompanion />` reads existing `ChatPhase` and local appearance preferences. Both use the shared motion lifecycle. Preserve `context_gauge.rs::CacheStat` and current cost UI.

- [ ] Add `vial_static_level_keeps_a11y_when_canvas_unavailable` and `vial_late_detail_is_discarded` to real browser coverage, plus Rust shallow-water tests `equal_level_is_stable` and `disturbance_settles_with_bounded_substeps`. Assert a 25% accessible fill at 128K / 512K, no measured level for estimates, and no pending companion frames after hide or preference off.
- [ ] Run the Rust tests and browser slice against the pre-change production mount to exercise the missing behavior.
- [ ] Replace the wide ring and add the same vial to the phone composer. Use bounded fixed-step shallow-water updates with reflecting endpoints and damping, then settle on the projected fraction. A semantic button opens accessible `context.breakdown` detail using the shared projection. Keep request details independent where no common identity exists. Use static rendering when inactive, reduced-motion or Canvas unavailable. Create an original CSS/Canvas pixel character with local on/off state and bounded presentation; map idle, processing and completion to existing state transitions.
- [ ] Run both Rust and WASM checks and the context/browser cases. Exercise pointer and keyboard overlay open/close, 512K actual data, layout at mobile width, and visibility teardown.
- [ ] 提交组件和替换点 / Commit the mounted widgets and removal of the superseded ring paths.

### Task 4: Authorized thought-block identity / 授权思考块标识

**接口 / Interfaces.** `aleph_protocol::thought::{ThoughtBlockId, ThoughtBlockRef, TranslationEngine, TranslationTarget}` is shared. `ThoughtBlockRef { session_key, run_id, block_id }` identifies a settled displayable block. `TranslationEngine` is `Machine | Ai`. `TranslationTarget` is `En | Zh` for current shipped locales. `trace.by_runs` retains its current projected content and may attach an optional block reference only to eligible rows. Core source resolution returns owned text after existing session and run access checks.

- [ ] Add Core source tests `thought_source_rejects_foreign_run`, `thought_source_rejects_unfinished_block`, `thought_source_rejects_mixed_or_legacy_rows`, and `two_blocks_in_one_iteration_have_distinct_ids`. Assert identical visible source text and selected input for an eligible masked row. Include unrecorded stream-only thoughts, tool summaries and synthetic `StreamEvent::Reasoning` producers.
- [ ] Run `cargo test -p alephcore --lib thought_source` and confirm the tests expose the missing contract.
- [ ] Add shared typed references and mark eligibility at the existing trusted provider-to-trace boundary. Reuse persisted trace row identity where the store guarantees uniqueness and stability. If a trace shape can originate outside a provider, persist optional provenance at that boundary and leave old rows readable. Reuse the existing owner-scoped trace authorization and stored redaction path. Avoid a caller-supplied text path or new transcript store. Keep the harness unchanged where existing provenance suffices and check the R10 ratchet if the producing boundary needs an adjustment.
- [ ] Run the new tests, existing scoped trace tests, and `cargo test -p aleph-protocol --lib`. Verify serialized references round-trip and older rows still deserialize.
- [ ] 提交类型和授权源 / Commit the shared identity and source authorization.

### Task 5: Capability-backed translation engines / 基于 Capability 的翻译引擎

**接口 / Interfaces.** `ThoughtTranslationRequest { block: ThoughtBlockRef, engine: TranslationEngine, target: TranslationTarget, expected_destination: String, request_id: String }` carries caller intent; availability returns an optional destination per engine. Panel-facing `thought.translation.engines`, `thought.translation.start`, and `thought.translation.cancel` use the existing method scope registry. `start` acknowledges after registering the authorized job and returns immediately. AI progress and terminal results from both engines use the same request-correlated event, confined to the initiating authorized connection. `cancel` checks the same effective caller, connection, request ID and scope. Existing typed config patches provide operator-controlled binding to a machine capability and an AI provider/model. Initial bounds are 16 KiB of source, 32 KiB of translated UTF-8, 30 seconds of execution and one concurrent request per connection, with a service semaphore of eight.

- [ ] Add transport-level tests `thought_translation_uses_authorized_capability_and_exact_target`, `thought_translation_rejects_revoked_lease_and_destination_change`, `thought_translation_cancel_is_scoped_and_stops_output`, and `thought_translation_is_bounded_and_deduplicated`. Use fake enabled backends at the real handler seam and assert actual invocations, exact source and locale, and absence of unauthorized egress. Include duplicate start and foreign cancel.
- [ ] Run `cargo test -p alephcore --lib thought_translation` before implementing the path. Verify failures describe missing behavior.
- [ ] Use the existing canonical Tool Capability registration and scoped dispatch for both externally named translation actions, with the machine adapter executing the configured MCP Tool and its expected structured input/output. The AI adapter uses existing provider resolution and bounded cancellable text generation, without agent tools or session transcript. Resolve the source in Core, verify destination at dispatch, authorize and validate the capability lease immediately before invocation, and wrap bounded execution in existing cancellation infrastructure. Tie event delivery to caller scope, connection, request and active operation. Terminate work when its connection closes. Keep translated text out of transcript history and durable request audit payloads. Expose authorized availability plus configured destinations and surface unavailable or denied states through typed errors. Assert the input, output, execution and concurrency limits.
- [ ] Run targeted capability, translation, and gateway admission tests, then `cargo check -p alephcore --bin aleph-server`. Verify enabled, unavailable and unauthorized configurations through production handler registration and test engines.
- [ ] 提交授权与引擎接线 / Commit the service, config and production mounts.

### Task 6: Thought reader and current Panel locales / 思考阅读器与界面语言

**接口 / Interfaces.** Extend the existing `<ReasoningPanel />` and history rendering to share an original/translated/bilingual view. `api::thought::ThoughtApi` provides typed availability, start and cancel, consuming the contracts from Tasks 4–5. Presentation preference and engine selection follow current Panel preference mechanisms. Locale is read from the active Panel locale on each start.

- [ ] Add `thought_reader_keeps_original_on_error`, `thought_late_chunk_cannot_cross_locale_or_session`, and `thought_mobile_reader_fits_and_preserves_think_control`. Assert that switching `En` to `Zh` cancels both engines' pending work, clears previous results, and leaves original blocks reachable.
- [ ] Run targeted Panel tests and the browser cases before connecting the reader. Confirm failures at the missing interaction or request boundary.
- [ ] Keep original stream playback and the existing thinking dial. Render eligible settled live and restored blocks with a per-block action, engine and disclosed destination. Read authorized availability. Scope in-flight state and ephemeral output to the mounted session, run, block, engine and locale. Require the active matching request before applying streamed or terminal output. Retain the original when unavailable, cancelled or failed.
- [ ] Run `cargo test -p aleph-panel --lib`, the WASM check and browser tests at wide and phone widths. Verify keyboard operation and bilingual strings in `en` and `zh`.
- [ ] 提交阅读器 / Commit UI, API integration and localized copy.

### Task 7: Integration, cleanup and reference docs / 集成、清理与参考文档

- [ ] Inspect changed mounts and call sites for orphaned selectors, timers, duplicate projections and legacy ring references. Check new externally callable names against the Capability policy and method-scope census. Review the whole branch with a fresh integration reviewer.
- [ ] Run `git diff --check`, `cargo fmt --all --check`, the shared protocol/UI and full Panel unit suites, specified targeted Core tests, and shipping Panel WASM build/check. Use the current repository Panel build entry and local runtime API; record exact commands and results.
- [ ] Use the shipped Leptos browser mount for 512K fixture occupancy, compaction, replay, session switches, narrow layout, dark/light schemes, reduced motion, hidden-tab cleanup, and both translation adapters with authorized local test endpoints. Record which desktop WebView was actually exercised and evidence for any platform that could not be run.
- [ ] Update bilingual `FEATURE_LOCATOR.md` and the current Panel/Capability reference entries with mount, data and preference anchors. Record runtime QA as evidence. Verify links and check the root and feature worktrees' status.
- [ ] Commit integration docs and cleanup. Report branch, commits, evidence, and remaining validation gaps.

## 执行分工 / Execution sequencing

父会话维护契约、交叉改动、验收和 Git。上下文（1→3）、动效（2→3）与思考来源（4→5→6）按依赖推进。互不写同一工作树的实现可并行，交叉文件由父会话串行合入。按用户指定的模型分工执行。 The parent owns contracts, shared-file integration, verification and Git. Independent implementation tracks use isolated worktrees. The parent schedules integration in dependency order and preserves the requested model tiers.
