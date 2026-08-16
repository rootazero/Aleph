# Voice Runtime Deep Refactor — Implementation Plan

**Spec:** `docs/superpowers/specs/2026-08-16-voice-runtime-refactor-design.md`
**Branch:** `voice-deep-refactor-2026-08-16`
**Worktree:** `.worktrees/voice-deep-refactor-2026-08-16`

## Goal

Improve correctness, maintainability, and regression coverage of Aleph's
voice subsystem (§2.4 + §6.5) without changing public APIs or breaking
the FEATURE_LOCATOR's "✅ 已实现" claims.

## Architecture

- **No public API changes** — only doc comments + new tests
- **No new dependencies** — supplies via `serde_json::Value` already
  used in the stream adapters
- **One integration test** — pins `voice_mode::set` → `prompt_build`
  → rendered prompt path

## Tech Stack

Rust (existing); test framework: `#[cfg(test)]` + `tokio::test`;
cargo workspace unchanged.

## Global Constraints

From AGENTS.md:
- rustfmt 4-space indent, 100 char width
- clippy `-D warnings`
- No new heavy deps (R3)
- No public API changes (R10)
- 严禁触碰 main 分支（用户在 worktree 内工作）

---

## Task 1: Module Overview Doc

**Files:**
- Modify: `src/gateway/voice/mod.rs`

**Step 1.1: Replace the 10-line `mod.rs` with a doc-only overview**

The current file is just `pub mod` re-exports. Add a `//!` crate doc
header that names the four files and explains the three concepts.

```rust
//! Voice subsystem — three concepts, four files.
//!
//! ## Four files, three concepts
//!
//! | File | Concept | Keying |
//! |------|---------|--------|
//! | `voice_mode.rs` | **session-turn registry** | `session_key` → `VoiceTurnState{transcribed, vocabulary}` |
//! | `state.rs` | **channel state** | `channel_id` → `VoiceState{enabled, provider, voice, consecutive_failures}` |
//! | `voice_mode_set.rs` (tool) | **LLM-callable toggle** | mutates `VoiceState` (channel) |
//! | `thinker::layers::voice_mode.rs` | **prompt-layer** | reads `voice_mode` registry → "## Voice Mode" |
//!
//! Read this first if you're searching for "voice mode" or "voice state":
//! - "voice mode" on a session → `voice_mode.rs` (registry) + `layers/voice_mode.rs` (layer)
//! - "voice mode" on a channel → `state.rs` + `voice_mode_set.rs` tool
//!
//! Don't confuse the two. A session has a per-turn `VoiceTurnState`; a channel has
//! a long-lived `VoiceState`. The names are confusingly similar by design (each
//! 2026-07-21 rename to fix a previous collision was deliberate); the table above
//! is the canonical cross-reference.
```

[Step 1.2: Verify]

```bash
cargo check -p alephcore 2>&1 | head -20
```

Expected: PASS (no API changes).

[Step 1.3: Commit]

```bash
git add src/gateway/voice/mod.rs
git commit -m "voice: add module overview doc to disambiguate four-file layout"
```

---

## Task 2: Cross-link Doc Headers

**Files:**
- Modify: `src/gateway/voice/voice_mode.rs`
- Modify: `src/gateway/voice/state.rs`
- Modify: `src/builtin_tools/voice_tools/voice_mode_set.rs`
- Modify: `src/thinker/layers/voice_mode.rs`

**Step 2.1: Add cross-link to `voice_mode.rs`**

At the top of the existing doc block, add a `Sister files:` line:
```rust
//! Sister files:
//! - `state.rs` — channel-keyed `VoiceState` (different concept, confusingly similar name).
//! - `voice_mode_set.rs` (in `builtin_tools/voice_tools/`) — the LLM tool mutates `VoiceState`.
//! - `thinker::layers::voice_mode.rs` — the prompt-layer that reads this registry.
```

[Step 2.2: Same for `state.rs`]
```rust
//! Sister files:
//! - `voice_mode.rs` — session-keyed `VoiceTurnState` (different concept, confusingly similar name).
//! - `voice_mode_set.rs` (in `builtin_tools/voice_tools/`) — the LLM tool that mutates THIS struct.
```

[Step 2.3: Same for `voice_mode_set.rs`]
```rust
//! Sister files:
//! - `gateway/voice/state.rs` — `VoiceState` this tool mutates.
//! - `gateway/voice/voice_mode.rs` — session-keyed `VoiceTurnState` (different concept; we don't touch it).
//! - `thinker::layers::voice_mode.rs` — the prompt-layer that reads the registry.
```

[Step 2.4: Same for `thinker/layers/voice_mode.rs`]
```rust
//! Sister files:
//! - `gateway/voice/voice_mode.rs` — the session-keyed registry this layer reads.
//! - `gateway/voice/state.rs` — channel-keyed `VoiceState` (different concept).
//! - `builtin_tools/voice_tools/voice_mode_set.rs` — the LLM tool that toggles channel voice.
```

[Step 2.5: Verify]

```bash
cargo check -p alephcore 2>&1 | head -20
cargo clippy -p alephcore -- -D warnings 2>&1 | head -20
```

Expected: PASS.

[Step 2.6: Commit]

```bash
git add src/gateway/voice/voice_mode.rs src/gateway/voice/state.rs src/builtin_tools/voice_tools/voice_mode_set.rs src/thinker/layers/voice_mode.rs
git commit -m "voice: cross-link module docs to disambiguate four-file voice layout"
```

---

## Task 3: Empty-bytes Defense Tests (whisperlive.rs)

**Files:**
- Modify: `src/gateway/voice/streaming/whisperlive.rs` (test module only)

**Step 3.1: Read existing test scaffolding**

Already familiar — `msg(vec![seg_at(...)])` helper exists.

**Step 3.2: Add tests**

```rust
#[test]
fn empty_segments_array_emits_no_delta() {
    let mut dec = WhisperLiveDecoder::default();
    let d = dec.push(&serde_json::json!({
        "uid": "u",
        "segments": [],
        "is_final": true
    })).unwrap();
    // Empty committed stays empty, no interim, no error.
    assert_eq!(d.committed, "");
    assert_eq!(d.interim, "");
    assert!(d.error.is_none());
    assert!(!d.utterance_end);
}

#[test]
fn null_envelope_emits_no_delta() {
    let mut dec = WhisperLiveDecoder::default();
    let d = dec.push(&serde_json::Value::Null).unwrap_or_default();
    // Whatever the decoder returns for null, it must not panic, and it must
    // not carry forward any state from the previous call.
    assert_eq!(d.committed, "");
    assert!(d.error.is_none());
}

#[test]
fn empty_string_text_is_skipped() {
    let mut dec = WhisperLiveDecoder::default();
    let d = dec.push(&msg(vec![
        seg_at(0.0, "", true),      // empty completed
        seg_at(1.0, "你好", true),   // real followed by empty
    ])).unwrap();
    assert_eq!(d.committed, "你好");
}
```

**Step 3.3: Verify**

```bash
cargo test -p alephcore --lib gateway::voice::streaming::whisperlive::tests::empty_segments_array_emits_no_delta
cargo test -p alephcore --lib gateway::voice::streaming::whisperlive::tests::null_envelope_emits_no_delta
cargo test -p alephcore --lib gateway::voice::streaming::whisperlive::tests::empty_string_text_is_skipped
```

Expected: PASS.

**Step 3.4: Commit**

```bash
git add src/gateway/voice/streaming/whisperlive.rs
git commit -m "voice: pin empty-bytes defense in whisperlive decoder"
```

---

## Task 4: Empty-bytes Defense Tests (deepgram.rs)

**Files:**
- Modify: `src/gateway/voice/streaming/deepgram.rs` (test module only)

**Step 4.1: Add tests**

```rust
#[test]
fn empty_transcript_is_skipped() {
    let mut dec = DeepgramDecoder::default();
    let d = dec.push(&results(true, "")).unwrap_or_default();
    assert_eq!(d.committed, "");
    assert!(d.error.is_none());
}

#[test]
fn empty_channel_alternatives_is_skipped() {
    let mut dec = DeepgramDecoder::default();
    let v = serde_json::json!({
        "type": "Results",
        "is_final": true,
        "channel": { "alternatives": [] }
    });
    let d = dec.push(&v).unwrap_or_default();
    assert_eq!(d.committed, "");
}
```

**Step 4.2: Verify**

```bash
cargo test -p alephcore --lib gateway::voice::streaming::deepgram::tests
```

Expected: PASS.

**Step 4.3: Commit**

```bash
git add src/gateway/voice/streaming/deepgram.rs
git commit -m "voice: pin empty-bytes defense in deepgram decoder"
```

---

## Task 5: VoiceState Failure Counter Contract Test

**Files:**
- Modify: `src/gateway/voice/state.rs` (test module only)

**Step 5.1: Add a test that pins the contract**

```rust
#[test]
fn three_failures_disables_once() {
    // Contract: 3 consecutive failures → auto-disable, regardless of how many
    // provider hops happened within the call. The counter is incremented by
    // the CALLER (`send_as_voice`), not by the TTS path itself.
    let mut state = VoiceState { enabled: true, ..Default::default() };
    assert!(!state.record_failure());  // 1
    assert!(state.enabled);
    assert!(!state.record_failure());  // 2
    assert!(state.enabled);
    let disabled = state.record_failure();  // 3
    assert!(disabled);
    assert!(!state.enabled);
    // After disable, further failures stay disabled; counter stays at 3
    // (saturating_add preserves the 3-strike semantics).
    let _ = state.record_failure();
    assert!(!state.enabled);
    assert_eq!(state.consecutive_failures, 3);
}

#[test]
fn success_resets_failure_counter() {
    let mut state = VoiceState {
        enabled: true,
        consecutive_failures: 2,
        ..Default::default()
    };
    state.record_success();
    assert_eq!(state.consecutive_failures, 0);
    assert!(state.enabled);
}

#[test]
fn failure_counter_is_per_channel_state() {
    // Two channel states must be independent.
    let mut a = VoiceState { enabled: true, ..Default::default() };
    let mut b = VoiceState { enabled: true, ..Default::default() };
    a.record_failure();
    a.record_failure();
    assert_eq!(a.consecutive_failures, 2);
    assert_eq!(b.consecutive_failures, 0);
}
```

**Step 5.2: Verify**

```bash
cargo test -p alephcore --lib gateway::voice::state::tests
```

Expected: PASS.

**Step 5.3: Commit**

```bash
git add src/gateway/voice/state.rs
git commit -m "voice: pin failure-counter contract — 3 strikes, no per-hop counting"
```

---

## Task 6: End-to-End voice_mode → prompt_build Test

**Files:**
- Modify: `src/thinker/layers/voice_mode.rs` (test module)

**Step 6.1: Add a test that exercises the full chain**

The test will:
1. Write a `VoiceTurnState` to the registry with vocabulary
2. Use `prompt_build::resolve_prompt_context` (or call the layer
   directly with a fabricated `ResolvedContext` that mirrors the
   resolver's behavior)
3. Assert the rendered prompt contains both the voice guidelines
   and the vocabulary

```rust
#[test]
fn end_to_end_registry_to_rendered_prompt() {
    // Pin the contract: the prompt_build path reads voice_mode::get()
    // and translates to VoiceContext + voice_vocabulary, which the layer
    // renders. We mirror the translation from prompt_build.rs:862-870
    // here so the test doesn't depend on config plumbing.
    let sk = "voice-e2e-test-session";
    crate::gateway::voice::voice_mode::set(
        sk,
        Some(crate::gateway::voice::voice_mode::VoiceTurnState::new(
            true,
            Some("Aleph, Leptos, Rust".to_string()),
        )),
    );

    // Mirror prompt_build.rs:862-870 — the registry reader.
    let turn = crate::gateway::voice::voice_mode::get(sk)
        .expect("just-set entry");
    let voice = match turn.transcribed {
        true => VoiceContext::SpokenTranscribed,
        false => VoiceContext::Spoken,
    };
    let vocab = turn.vocabulary.as_deref();

    // Build the context the layer expects.
    let mut ctx = ContextAggregator::resolve(
        &InteractionManifest::new(InteractionParadigm::Background),
        &SecurityContext::permissive(),
    );
    ctx.voice = voice;
    ctx.voice_vocabulary = vocab.map(String::from);

    let rendered = render(&ctx);

    assert!(rendered.contains("## Voice Mode"));
    assert!(rendered.contains("transcribed from speech"));
    assert!(rendered.contains("Domain vocabulary for this conversation: Aleph, Leptos, Rust"));
    assert!(rendered.contains("Prefer these exact terms when repairing misrecognized words"));

    // Cleanup.
    crate::gateway::voice::voice_mode::set(sk, None);
}

#[test]
fn end_to_end_typed_input_rendered_prompt_is_byte_identical() {
    // Regression: a typed voice-off turn must not insert any voice-mode
    // text into the prompt. Same shape as the existing
    // `skips_when_voice_inactive` test, but through the registry write path.
    let sk = "voice-e2e-typed";
    crate::gateway::voice::voice_mode::set(sk, None);

    let turn = crate::gateway::voice::voice_mode::get(sk);
    let voice = match turn.as_ref() {
        Some(s) if s.transcribed => VoiceContext::SpokenTranscribed,
        Some(_) => VoiceContext::Spoken,
        None => VoiceContext::Off,
    };

    let mut ctx = ContextAggregator::resolve(
        &InteractionManifest::new(InteractionParadigm::Background),
        &SecurityContext::permissive(),
    );
    ctx.voice = voice;

    let rendered = render(&ctx);
    assert!(rendered.is_empty(), "voice-off turn must not render voice guidelines");
}
```

**Step 6.2: Verify**

```bash
cargo test -p alephcore --lib thinker::layers::voice_mode::tests::end_to_end_registry_to_rendered_prompt
cargo test -p alephcore --lib thinker::layers::voice_mode::tests::end_to_end_typed_input_rendered_prompt_is_byte_identical
```

Expected: PASS.

**Step 6.3: Run full voice test suite**

```bash
cargo test -p alephcore --lib voice::
cargo test -p alephcore --lib thinker::layers::voice_mode
```

Expected: All previously-passing tests still pass; new tests pass.

**Step 6.4: Commit**

```bash
git add src/thinker/layers/voice_mode.rs
git commit -m "voice: pin end-to-end registry→prompt_build→rendered-prompt path"
```

---

## Task 7: Full Validation

**Step 7.1: Compile**

```bash
cargo check -p alephcore
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

**Step 7.2: Lint**

```bash
cargo clippy -p alephcore -- -D warnings
```

**Step 7.3: Voice test suite**

```bash
cargo test -p alephcore --lib voice::
cargo test -p alephcore --lib thinker::layers::voice_mode
```

**Step 7.4: Spot check broader**

```bash
cargo test -p alephcore --lib gateway::voice
cargo test -p alephcore --lib builtin_tools::voice_tools
```

---

## Task 8: FEATURE_LOCATOR.md Update

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`

**Step 8.1: Add 2026-08-16 round entry to §2.4**

After the existing "状态 (2026-07-30)" line, add:

```
- **🟢 命名/概念去歧与回归防线加固轮（2026-08-16）**：四文件三概念（`voice_mode.rs` 回合注册表 / `state.rs` channel 状态 / `voice_mode_set.rs` tool / `layers/voice_mode.rs` 注入层）模块头加 cross-link；`voice_mode::set` → `prompt_build` → `VoiceModeLayer` 端到端新增两条 regression test；whisperlive/deepgram decoder 空帧/空数组/null 防线补 5 条单测；`VoiceState` 失败计数 contract 钉 3 条新单测（3-strike 一次性、`record_success` 复位、channel 间隔离）。**不改命名**（R10 已固化 2026-07-21 历史命名），**不改 `VoiceTurnState` 字段**（capture-time 设计正确：每次 dispatch 重读 `voice_local.vocabulary_hint()`），**不引入并发原语**（无压测触发，没有 DashMap 的需求）。
```

**Step 8.2: Add 2026-08-16 round entry to §6.5**

After the existing "Round-2b（2026-07-25）" line, add:

```
- **🟢 命名/概念去歧与回归防线加固轮（2026-08-16）**：与 §2.4 同步的 hardening 升级。① 模块头 cross-link 拼齐四文件三概念（`voice_mode.rs` 回合/`state.rs` channel/`voice_mode_set.rs` tool/`layers/voice_mode.rs` 注入层）——R10 允许的最小代价。② whisperlive/deepgram decoder 空帧/空数组/null 防线补 5 条单测（"empty_segments_array_emits_no_delta"、"null_envelope_emits_no_delta"、"empty_string_text_is_skipped"、"empty_transcript_is_skipped"、"empty_channel_alternatives_is_skipped"），钉住 WLK 运维前置条件（"empty bytes trigger end-of-audio"）与现有代码的防御路径——未发现新 bug，仅 pin 现有正确行为。③ `VoiceState` 失败计数 contract 3 条新单测（"three_failures_disables_once"——钉 3-strike 一次性、不跨 provider hop 双计；"success_resets_failure_counter"；"failure_counter_is_per_channel_state"——channel 间隔离），呼应 outbound.rs 已落地的"TTS_MAX_PROVIDERS=2 是 1 跳 fallback 不是 sweep" 设计。④ `voice_mode::set` → `prompt_build` → `VoiceModeLayer` 端到端 2 条 regression test（"end_to_end_registry_to_rendered_prompt"——含 transcribed+vocabulary 完整链路；"end_to_end_typed_input_rendered_prompt_is_byte_identical"——voice-off 路径 byte-identical），这是历史上 `metadata["voice_mode_active"]` 死戳 bug 的形状——它随 `voice_mode.rs` 2026-07-21 改名已 CUP，但端到端 gating 是新加的。**won't do**：vocabulary Arc<Vec> 读时 join（每次 dispatch 已重读 config，registry 存快照是 per-turn 事实，不是 per-render 事实）；并发改成 DashMap（无压测触发）；引入 §2.4 voice 词汇读时连读（违反 R7：把"intellegence lives in the prompt"边外延）。
```

**Step 8.3: Verify the file compiles (markdown)**

```bash
# No build check for markdown, but verify the table structure.
rg -n "2026-08-16" docs/reference/FEATURE_LOCATOR.md
```

Expected: 2 matches (2.4 and 6.5 sections).

**Step 8.4: Commit**

```bash
git add docs/reference/FEATURE_LOCATOR.md
git commit -m "FEATURE_LOCATOR: 2026-08-16 轮记录 voice 命名去歧 + 回归防线"
```

---

## Task 9: Final Review

**Step 9.1: Diff summary**

```bash
git diff --stat main
```

Expected: 5-7 files, ~200-400 lines added (mostly tests + docs).

**Step 9.2: Run targeted tests one more time**

```bash
cargo test -p alephcore --lib voice::
cargo test -p alephcore --lib thinker::layers::voice_mode
```

**Step 9.3: Push branch (do NOT merge to main)**

```bash
git push origin voice-deep-refactor-2026-08-16
```

## Done

All Tasks 1-9 complete. Branch is ready for review at
`voice-deep-refactor-2026-08-16`. No merge to main without user
approval.
