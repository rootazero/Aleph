# Production Voice Conversation Implementation Plan / 默认语音对话实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal** — Make the default immersive voice entry complete each utterance once, speak progressively, and release only the work it owns. / 默认沉浸语音每句话只提交一次，边生成边播，关闭和打断只撤回本轮工作。

**Architecture** — First connect the shipped Leptos WASM view to the existing exact Chat receipt and connection epoch. Then use the existing capture, STT relay, WAV batch path, and TTS player under a single turn owner. Core keeps authorization, provider selection, and execution authority. / 先接通精确回执与连接 epoch，再由回合所有者协调既有采集、转写和播放，授权和执行留在 Core。

**Tech Stack** — Rust, Tokio, Leptos/WASM, Web Audio, NSH 0.6.0, JSON-RPC, Python and Chromium QA.

**Spec** — [Approved design / 已批准设计](../specs/2026-11-28-voice-conversation-refactor-design.md), `228b21a95`.

## Global Constraints / 全局约束

- Work in `.worktrees/voice-conversation-refactor` on `voice-conversation-refactor`, based on `1b959fdf4`. Commit there. / 仅在隔离分支提交。
- Default browser/NSH Leptos entry first. Preserve the composer dictation and opt-in TSX contracts. / 首轮只迁移默认沉浸入口，兼容普通听写与 TSX。
- Keep `voice_input: true`, `run_only: true`, `chat.abort { run_id, run_only: true }`, existing Chat dials, streaming captions, sentence-by-sentence synthesis, and barge-in. / 保留会话参数、字幕、分句与打断。
- Reuse the Core providers, relay, and existing Capability contracts. The installed recognizer still requires authorized Desktop IPC. A native document marker alone carries no authorization. / 复用能力与授权入口。
- No automatic Chat resend after lost receipt, no inferred run from a same-session event, and no cleanup RPC on a replacement epoch. / 回执丢失时结果未知，禁止重发、猜测 run 或跨连接清理。
- Main session owns integration. Delegate bounded work with independent outputs, expert review, or isolated investigation under the operator's model policy. One writer per checkout. / 主会话负责验收和集成，按已授权的委派场景选模型。

## Review Focus / 审查重点

1. An exact queued receipt is cancelled while a sibling is active. The sibling and a later successor both finish in the configured-provider lane (Tasks 1, 6).
2. A terminal precedes the receipt for a newly created or existing conversation. The matching receipt never creates a second live route or borrows another run (Tasks 2, 3, 6).
3. Close or re-entry lands during permission, formatting, pending Chat, synthesis, decode, or playback finish. The old operation cannot update or enqueue work into the new view (Tasks 3, 6).
4. A callback exceeds the remaining sample budget and ends on a partial frame. A fresh utterance starts with its own accounting and audio (Task 4).
5. The last frame is delayed, dropped or rejected while a plausible preview is already visible. The turn uses exactly one retained WAV and ignores late streaming results (Tasks 4, 5, 6).

---

### Task 0 — Record the baseline / 固定基线

**Files** — Read `qa/nsh_voice/README.md`, `qa/nsh_voice/run.py`, `justfile`, `docs/reference/DEVELOPMENT.md`. Retain fixture receipts under a private directory outside the checkout, as required by `run.py`.

**Interfaces** — Produces the baseline asset/server hashes and short/long fixture observations consumed by Task 6.

- [ ] Run `cargo test --locked -p shared-ui-logic voice:: --lib`, `node --test qa/nsh_voice/*.test.cjs`, and `python3 -B -m unittest discover -s qa/nsh_voice -p 'test_*.py'`. Expect 9 Rust, 10 Node and 4 Python tests at the design baseline.
- [ ] Run `just wasm` and then `cargo build --locked -p alephcore --bin aleph-server` to embed that WASM in the baseline Gateway. Run `python3 qa/nsh_voice/run.py --mount classic --case success --server "$voice_server" --panel-web "$voice_panel" --output "$voice_receipt_dir"` for `--clip short` and `--clip long`, each with `--warm 1` and its own new output directory. Use `interfaces/webchat/dist` for `voice_panel`. Record accepted inputs, STT/Chat/TTS counts, playback measures and whether the fixture can observe each stage. Use the same material and configuration in Task 6.

### Task 1 — Carry existing paired provenance through exact Chat / 保留精确回执的配对上下文

**Files** — Modify `src/bin/aleph-server/server_init.rs`, `src/gateway/busy_queue/spawn.rs`. Extend the worker tests and `src/gateway/handlers/chat.rs` fixtures only where they can exercise the affected boundary. Task 6 covers the configured-provider binary.

**Interfaces** — Extend `pub async fn spawn_queued_chat_run<P, R, E>(engine: Arc<ExecutionEngine<P, R>>, request: RunRequest, agent: Arc<AgentInstance>, emitter: Arc<E>, cfg: BusyQueueConfig, locale: Locale, label: &'static str, run_manager: &AgentRunManager, owners: Option<PanelShellOwners>)` using the existing bounds and shared worker. Reuse `PairedShellRun::capture` from the ordinary Panel lane.

- [ ] Add `exact_chat_worker_retains_only_matching_paired_shell_origin` around the worker boundary. Assert `Some` for the qualifying owner with identical run/session, `None` for unpaired or mismatched origin, and no context inheritance in a nested mismatched request. Preserve the existing exact-cancel tests and queue ticket assertions.
- [ ] Run `cargo test --locked -p alephcore --lib exact_chat_worker_retains_only_matching_paired_shell_origin`. Expect the matching case to fail before wiring. Paired Shell effect tests are Linux-gated, so exercise the shared provenance capture on macOS and carry the Linux conditional evidence to Task 6.
- [ ] Forward the optional owners from `server_init` into the shared worker, preserving ticket registration and exact receipt authority. The exact lane remains process-local and bypasses durable enqueue recording. Use the same eligibility check as the ordinary lane.
- [ ] Run `cargo test --locked -p alephcore --lib exact_chat_worker_retains_only_matching_paired_shell_origin`, `cargo test --locked -p alephcore --lib chat_abort_run_only`, and `cargo check --locked -p alephcore --bin aleph-server`. Expect passing provenance, existing cancellation tests and binary compilation.
- [ ] Commit as `fix: retain paired origin for exact chat runs`.

### Task 2 — Connect the epoch and terminal receipt / 接通连接与终态回执

**Files** — Modify `interfaces/webchat/src/context.rs`, `interfaces/webchat/src/api/chat.rs`, `interfaces/webchat/src/state/sessions.rs` and `interfaces/webchat/src/platform/wide/views/chat/events.rs`.

**Interfaces** — Expose `DashboardState::current_connection_epoch(&self) -> Option<u64>` within the crate and add `DashboardState::rpc_call_at_epoch_with_code(&self, epoch: u64, method: &str, params: Value) -> Result<Value, RpcFailure>` as an async method. Add `ChatApi::send_voice_at_epoch(state: &DashboardState, epoch: u64, message: &str, session_key: Option<&str>, agent_id: Option<&str>, project_root: Option<&str>, project_id: Option<&str>, model_override: Option<&ModelOverride>, dials: &SendDials) -> Result<ChatSendResponse, RpcFailure>` and `ChatApi::abort_run_at_epoch(state: &DashboardState, epoch: u64, run_id: &str) -> Result<(), RpcFailure>` as async methods. Keep `ChatApi::send` and the existing string-error API for their callers. Expose `SessionMap::bind_run_receipt(&self, run_id: &str, conv: ConvId, session_key: &str) -> Option<RunTerminal>` where `RunTerminal` is the observed `Complete`, `Failed`, or `Joined` outcome from Chat event routing.

- [ ] Add tests `exact_voice_send_preserves_chat_params`, `voice_abort_uses_exact_run_only`, `epoch_replacement_rejects_voice_rpc_before_enqueue`, and `late_receipt_cannot_rebind_terminal_route`. Assert both wire flags, dials, `run_id` with no `session_key` on abort, no enqueue on epoch mismatch, and running count stays at zero for each terminal on existing and new conversations. Include a queue rejection with `run_error` as its first event. Assert unrelated run, session, conversation, and epoch terminals cannot be consumed as the receipt's terminal.
- [ ] Run the named tests using `cargo test --locked -p aleph-panel --lib <test_name>`. Expect missing interface or failing assertion before wiring.
- [ ] Reuse the existing serialization/response and `record_own_run` path for both send methods. Keep the exact wire flag opt-in. Preserve server `RpcFailure.code` via the existing pinned enqueue path. Collect admitted terminal observations during the pending send at the Chat event routing boundary. Queue rejection can emit a session-keyed `run_error` without `run_accepted` or `run_queued`, so capture that eligible observation even when route resolution returns no target. Require matching receipt run ID and compatible session/conversation on the same epoch before applying it. Release observations on send completion or invalidation. Use the normal terminal projection once for a matching failure that had no route. Keep canonical session identity backfill even if the run already settled.
- [ ] Run `cargo test --locked -p aleph-panel --lib`, and `cargo check --locked -p aleph-panel --lib --target wasm32-unknown-unknown`. Expect all tests and the shipped target build to pass.
- [ ] Commit as `fix: pin voice chat receipts to the originating connection`.

### Task 3 — Give the production voice turn one owner / 收敛回合归属

**Files** — Modify `interfaces/webchat/src/platform/wide/views/voice/mod.rs` and `audio.rs`. Create `interfaces/webchat/src/platform/wide/views/voice/turn.rs` for the private, pure ownership transitions and their tests.

**Interfaces** — A private `VoiceTurn` owns `TurnToken { epoch: u64, generation: u64 }`, its capture/submission state and the accepted run. `VoiceTurn::begin(&mut self, epoch: u64) -> TurnToken`, `VoiceTurn::owns(&self, token: TurnToken) -> bool`, `VoiceTurn::begin_send(&mut self, token: TurnToken) -> bool`, `VoiceTurn::accept_run(&mut self, token: TurnToken, run_id: String) -> bool`, and `VoiceTurn::invalidate(&mut self) -> Option<(u64, String)>` drive synchronous transitions. A failed `accept_run` leaves the receipt with the caller for same-epoch cleanup. Use `RunTerminal` and the APIs from Task 2.

- [ ] Add turn tests `one_submission_per_capture`, `close_and_barge_in_revoke_before_cleanup`, `late_acceptance_cannot_replace_current_run`, and `unmatched_terminal_has_no_audio_effect`. Assert the old run/epoch is returned at most once, a late receipt does not change the successor, and `Failed`/`Joined` stop pending audio while only `Complete` can flush the unsaid tail. Add the close/reopen and permission/formatting/decoding effect assertions to Task 6 where browser resources are observable.
- [ ] Run `cargo test --locked -p aleph-panel --lib voice::turn`. Expect the unimplemented transition tests to fail before wiring.
- [ ] Connect both STT submission paths to `begin_send`. Move conversation/dials capture before the send await, consume any early terminal by exact receipt, and gate transcript, formatting, queue, player and terminal updates on the same turn/session context. A local loss after enqueue is outcome unknown and does not retry. Close, context switch, and reconnect synchronously invalidate before device cleanup. Barge-in invalidates the old reply before arming its successor. Release a microphone obtained after the long permission await when ownership was lost. Pin synthesis and stream cleanup to the originating epoch. Reuse the player epoch for already-enqueued output callbacks, and dispatch play/finish only for the current player and turn.
- [ ] Remove `speak_gen`, `speak_run`, `active_run`, the bare immersive `abort_run`, and the superseded parallel cleanup branches in `voice/mod.rs`. Preserve the composer abort and speech pipeline contracts.
- [ ] Run `cargo test --locked -p aleph-panel --lib voice::` and `cargo check --locked -p aleph-panel --lib --target wasm32-unknown-unknown`. Expect passing owner/audio tests and target compilation.
- [ ] Commit as `fix: isolate immersive voice turn ownership`.

### Task 4 — Make capture bounds and relay admission observable / 接通采集边界与接纳回执

**Files** — Modify `interfaces/webchat/src/platform/wide/views/voice/audio.rs`, `src/gateway/handlers/voice.rs`, and the tests at those sites. Reuse `src/gateway/voice/streaming/relay.rs` access checks and queue.

**Interfaces** — Keep `ingest_pcm(cap: &Rc<RefCell<Capture>>, data: &[f32], sample_rate: u32, frame_chunk: usize)` and the existing device adapters. Add `MicSession::seal_streaming(&self) -> bool` (emit the final non-empty partial frame through the current sink and return whether opening/backlog lost a frame). Add optional `ack: true` to `voice.stream.audio`. An authorized relay enqueue returns `{ "accepted": true }`, and refusal under queue pressure or closure returns `{ "accepted": false }`. Legacy and empty/unknown/foreign requests still return `{}`.

- [ ] Add `segment_respects_remaining_sample_budget`, `seal_streaming_sends_one_partial_frame`, `pending_overflow_is_reported_and_reset`, and `audio_ack_reports_only_authorized_admission`. Assert overflow samples are outside the segment, the final partial frame is sent once, the next stream has clean accounting, legacy/empty/unknown/foreign replies are `{}`, and full/closed paths cannot report admission. Check adjacent callback and frame boundaries at 44.1 and 48 kHz against the continuous input's emitted sample count and ordering. Keep the existing resampler unless the measurements identify a defect.
- [ ] Run the named tests in their owning crates with `cargo test --locked -p aleph-panel --lib <test_name>` or `cargo test --locked -p alephcore --lib audio_ack_reports_only_authorized_admission`. Expect failing bounds/ack assertions or missing seam before changes.
- [ ] Bound the callback slice by remaining sample capacity and preserve pre-roll and short-WAV padding policy. Use the existing streaming buffer to emit only complete frames until seal, then emit the remaining bytes once. Keep a sticky pending-overflow bit until seal or explicit stop. Keep `try_send` nonblocking and derive its acknowledgment from the actual authorized send result.
- [ ] Run `cargo test --locked -p aleph-panel --lib voice::audio` and `cargo test --locked -p alephcore --lib gateway::handlers::voice`. Expect all legacy and new cases to pass.
- [ ] Commit as `fix: bound voice capture and report relay admission`.

### Task 5 — Finish streaming or recover the same utterance / 同回合识别与降级

**Files** — Modify `interfaces/webchat/src/platform/wide/views/voice/{mod,turn}.rs`. Reuse the capture APIs from Task 4 and `voice.transcribe`, `voice.stream.start`, `voice.stream.audio`, `voice.stream.stop`.

**Interfaces** — Add `VoiceTurn::finish_recognition(&mut self, token: TurnToken, usable_preview: bool, stream_intact: bool) -> Option<RecognitionPath>` where `RecognitionPath` is `Streaming` or `Batch`, consumed at most once. Keep the retained sample segment until the decision. Use a private `VOICE_FRAME_ACK_GRACE_MS: u32 = 250` from the moment capture seals. This is the first measured client deadline, adjustable from the Task 6 evidence without changing the wire contract.

- [ ] Add `healthy_stream_skips_wav_fallback`, `unconfirmed_final_frame_uses_batch_once`, `empty_or_closed_stream_uses_batch_once`, and `late_stream_delta_cannot_replace_batch_result`. Assert the healthy path requires usable text and all known frames admitted. Missing ack at grace expiry, closed/open failure, or loss chooses one batch fallback even with plausible captions. Race the timeout and last ack and assert a single recognition winner.
- [ ] Run `cargo test --locked -p aleph-panel --lib voice::turn`. Expect missing or failing recognition transitions before implementation.
- [ ] Seal capture, stop sending live PCM, and allow the bounded frame acknowledgment window. Bound simultaneously outstanding frame requests using the existing 25-frame backlog ceiling. Keep enqueue order and mark any skipped frame as lost. Derive `stream_intact` from local backlog, actual ack, stream closure and the currently owned stream ID. Revoke old captions and frame admission before stopping the stream. A late open gets stopped only on the original epoch. Extract/encode one WAV only for batch fallback, then apply the same nonempty and ownership gates to its result. Preserve existing repeated-failure strikes for later capture mode selection. Remove the branches that discarded the only WAV after an empty/untrusted stream and those that bypassed the owner when formatting returned.
- [ ] Run `cargo test --locked -p aleph-panel --lib voice::` and the WASM target check. Expect no redundant STT call on the healthy path and no asynchronous UI effect from a revoked turn.
- [ ] Commit as `fix: recover incomplete streaming transcripts from captured audio`.

### Task 6 — Verify production effects and reconcile the references / 验收及文档校对

**Files** — Extend `qa/nsh_voice/{classic.cjs,classic.test.cjs,fixture.py,test_fixture.py,run.py,README.md}` as the scenario needs. Reuse `qa/nsh_voice/wire.cjs` and the existing paired fixtures. Update `docs/reference/FEATURE_LOCATOR.md` §6.5, `docs/reference/DESKTOP_SHELL.md` and any live reference contradicted by the verified paths. Write a bilingual `docs/superpowers/runs/2026-11-28-voice-conversation-refactor-receipt.md` with measured results and coverage limits.

**Interfaces** — Keep `run.py --mount classic` and current TSX cases. Extend the Classic scenario selector for healthy-stream, fallback, and interruption/failure evidence. Drive the actual Gateway binary with two distinguishable requests and deterministic providers. Emit the existing `receipt.json` format with additive observed event/count fields.

- [ ] Add real entry scenarios. `healthy-stream` must observe live preview, a single Chat send, a first playback start before `run_complete`, and zero batch transcribes. `fallback` must observe a usable preview followed by full/late final admission failure and by an empty stream in separate runs, one retained-WAV transcribe and one Chat send each. Also cover an empty batch reply, where Chat count is zero. Failure after partial assistant text must stop audio without a tail synthesis request. Keep separate assertion clocks for STT finish, Chat first sentence, synthesis and audio start.
- [ ] Add exact run/resource scenarios for close during device permission/capture, STT, pending send, accepted run, and playback, plus barge-in and reconnection. Assert actual device track stop and audio pause/end, no stale synthesis, and no RPC sent on replacement epoch. Exercise a late receipt on the same epoch and a lost receipt, checking the abort payload and absence of retries. Verify the earlier terminal response on both existing and new conversations. Use the configured-provider server for queued and running exact cancel with distinguishable sibling/successor output. Assert exact run stops and other work completes, including an independent active sibling during queue cancellation. Run the existing simulated exact verification as a supplemental case.
- [ ] Run `node --test qa/nsh_voice/*.test.cjs` and `python3 -B -m unittest discover -s qa/nsh_voice -p 'test_*.py'`. Build `just wasm` and the real Gateway binary from the feature checkout, using only isolated runtime data and the controlled provider. Run each Classic scenario against the build and the compatible TSX fixture. Repeat Task 0's short/long cold and warm runs using matched media and report actual timestamps, missing observations and fixture boundaries.
- [ ] Exercise the shipped build in NSH on the available platform with its real permission and clean close path. Use the existing package recipe and an isolated test environment. Record device-backed test evidence separately from fake-device browser evidence and list steps blocked by permissions, unavailable hardware, or OS once permitted alternatives are exhausted. On Linux verify the paired Shell effect cases if the environment is available. Report costs of the acknowledgment grace window from observed timings.
- [ ] Update voice locator and references with the roles of the default and opt-in mounts, precise cancel/delivery semantics, relay admission meaning, and device/provider authorization. Resolve relative links and check `git diff --check`. Review private code removal and callsites in both mounts. Run the focused Rust, binary, WASM and fixture suites from Tasks 1–5.
- [ ] Obtain authorized independent whole-branch review with GPT Sol or Kimi Pro, specifically checking the configured-provider lane, early terminals, disconnect and resource cleanup. Integrate findings and rerun affected verification. Commit the QA and reference changes as `docs: verify the production voice conversation pipeline` or split fixture and evidence into separate commits. Confirm the isolated branch is clean.
