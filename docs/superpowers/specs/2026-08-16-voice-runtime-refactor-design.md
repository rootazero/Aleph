# Voice Runtime Deep Refactor — Design Spec

**Date:** 2026-08-16
**Scope:** §2.4 Voice-as-Context + §6.5 Voice Conversation Runtime
**Branch:** `voice-deep-refactor-2026-08-16`
**Status:** Approved (方案 B)

## 1. Background

Voice subsystem is "✅ 已实现" with 4 rounds of hardening. Three files with
similar names carry three distinct concepts, and the integration between
`voice_mode::set/get` (registry) and `prompt_build` (reader) has only one
end-to-end coverage via `AgentRunManager` (handlers/agent.rs:1482) — the
prompt-render path itself is not regression-locked.

## 2. Real Issues (severity-ranked)

### P0 — Naming/Concept conflation
Three files share `voice_mode` in name with three different concepts:

| File | Concept | Unit of keying |
|------|---------|----------------|
| `gateway/voice/voice_mode.rs` | session-keyed turn registry | `session_key` → `VoiceTurnState{transcribed, vocabulary}` |
| `gateway/voice/state.rs` | channel-keyed state | `channel_id` → `VoiceState{enabled, provider, voice, consecutive_failures}` |
| `gateway/voice/voice_mode_set.rs` (tool) | LLM-callable toggle | mutates `VoiceState` only |

R10 risk: future readers see `voice_mode::set(sk, ...)` and read it as
"set channel voice mode", which writes per-session state. The two states
are different concepts; the names should communicate that.

### P1 — `voice_mode` → `prompt_build` end-to-end test missing
- `voice_mode.rs` (registry) has 7 unit tests
- `voice_mode.rs` (layer) has 11 unit tests
- `agent.rs` has 3 integration tests around `AgentRunManager`
- The "registry writes → prompt assembly reads → prompt renders voice
  guidelines" path has no dedicated regression test. Theure was a
  documented `metadata["voice_mode_active"]` bug that was "dead but
  reached `build_system_prompt`" — this is the bug shape we want to
  prevent from recurring.

### P2 — Empty bytes defense (low priority, post-fix)
Both `whisperlive.rs` and `deepgram.rs` already handle empty `text`
inside segments and serde-parse failures. The defense is sufficient
per the WLK docs ("empty bytes trigger end-of-audio"); adding belt-and-
braces `if msg.is_empty() return None` does not hurt and aligns with
the operand.

### P2 — TTS failure counter scope
`outbound.rs::generate_tts` runs primary + fallback in one call. The
caller in `reply_emitter/helpers.rs` logs `record_failure` once per
call, not once per provider attempt. Verify the contract; add a test.

## 3. Design Decisions

### Decision 1: Do NOT rename files (R10 YAGNI)
The 2026-07-21 rename `session_mode.rs` → `voice_mode.rs` was a
prior decision that fixed the `session_mode` collision. Adding another
rename now risks breaking the git history clarity. Instead, strengthen
**module-level docs** with explicit cross-references that name the
three concepts and the four files.

### Decision 2: Keep `VoiceTurnState::vocabulary: Option<String>` (capture-time)
The vocabulary is **read fresh from config at dispatch time** in
`inbound_router/executor.rs:345-365`. The registry stores a snapshot
for the turn's prompt render. Re-reading at render time would require
injecting `Arc<RwLock<VoiceLocalConfig>>` into the rendered prompt
path, which crosses R10 (premature flexibility). The current
capture-time design is correct: voice vocabulary is a per-turn fact,
not a per-render fact.

### Decision 3: Add end-to-end regression test, not a refactor
The test will:
1. Push a `VoiceTurnState` for a fake session_key
2. Call `prompt_build::resolve_prompt_context` (or the smallest
   seam that exposes the resolved `voice`/`voice_vocabulary` fields)
3. Build a `LayerInput` with the resolved context
4. Render `VoiceModeLayer`
5. Assert: contains "## Voice Mode", contains "Domain vocabulary", and
   the rendered-vocabulary equals the source list

This is the minimal test that pins the no-future-regression contract.

### Decision 4: Empty-bytes defense as test-only
Add a unit test that demonstrates `WhisperLiveDecoder::push` returns
`None` for a `"null"` JSON envelope AND for an empty array
`{"segments":[]}`. No new code paths needed — these are existing
defenses; the test pins them.

### Decision 5: TTS failure counter is correct as-is
A single `generate_tts` call that hits primary + fallback where
both fail returns `None`. The current `record_failure` is called
once in `send_as_voice` (the caller). Adding a counter increment per
provider-attempt would double-count failures and disable channels
faster than the 3-strike contract intends. The decision is **no
change**, but add a guard test that asserts the failure-counting
contract via a stubbed `record_failure` surface.

## 4. Files to Change

| File | Change | Reason |
|------|--------|--------|
| `src/gateway/voice/voice_mode.rs` | Module doc clarity + cross-link | P0 |
| `src/gateway/voice/state.rs` | Module doc clarity + cross-link | P0 |
| `src/builtin_tools/voice_tools/voice_mode_set.rs` | Module doc clarity + cross-link | P0 |
| `src/thinker/layers/voice_mode.rs` | Module doc clarity + cross-link | P0 |
| `src/gateway/voice/mod.rs` | **NEW** crate overview doc | P0 |
| `src/gateway/voice/streaming/whisperlive.rs` | Add 2 tests: empty array / null envelope | P2 |
| `src/gateway/voice/streaming/deepgram.rs` | Add 1 test: empty channel alternatives | P2 |
| `src/gateway/voice/state.rs` | Add 1 test: failure counter not double-counted across fallback | P2 |
| `src/gateway/voice/state.rs` OR `src/gateway/voice/outbound.rs` | Add 1 integration test: `voice_state` + outbound produce exactly one failure increment per call | P2 |
| `src/gateway/voice/voice_mode.rs` (or new) | Add 1 integration test: registry → prompt_build → rendered prompt | P1 |

## 5. Out of Scope (deliberate skips)

- Renaming any voice file (R10 + git history risk)
- Moving vocabulary to read-time (premature flexibility)
- Adding DashMap or similar (no current concurrency pressure)
- Extracting voice into a separate crate (R3 / R7 violation)
- New ASR backend adapters (whisperlive + deepgram cover self-hosted
  and cloud)
- Streaming TTS trait (V26, deferred per §6.5 discipline)

## 6. Verification

```bash
# Compile
cargo check -p alephcore

# Lint
cargo clippy -p alephcore -- -D warnings

# Voice tests (Lib only)
cargo test -p alephcore --lib voice::

# Targeted
cargo test -p alephcore --lib \
  gateway::voice::voice_mode \
  gateway::voice::state \
  gateway::voice::streaming \
  thinker::layers::voice_mode

# Workspace
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

## 7. Rollback

Single branch, no `Cargo.toml` changes, no public API changes.
Revert with `git revert <merge>` if the integration test breaks a
production prompt path.
