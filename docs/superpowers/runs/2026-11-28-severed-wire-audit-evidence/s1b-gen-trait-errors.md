# S1B — Severed-wire audit: `src/generation/mod.rs` + `src/generation/error.rs`

Slice: `src/generation/mod.rs` (798 lines, read fully), `src/generation/error.rs` (1145 lines, read fully).
Repo root: `/home/zou/data/workspace/Aleph`. Audit is READ-ONLY (no edits, no git ops, no cargo).
`graphify` was used for orientation only; every claim below is confirmed with `rg`.

---

## A. Findings

| ID | Form | Severity | Title | Producer | Consumer |
|----|------|----------|-------|----------|----------|
| SW-S1B-GEN-TRAIT-ERRORS-1 | 1 | high | `impl From<GenerationError> for AlephError` has **zero** production consumers — the entire rich classification is dead code in production | src/generation/error.rs:645 | none |
| SW-S1B-GEN-TRAIT-ERRORS-2 | 2 | high | Live tool path collapses RateLimit / Timeout / Quota / Cancelled into a generic, **non-retryable** `ToolError::Execution`; a 429 is banned by the harness cross-batch memo | src/builtin_tools/generation/mod.rs:19-37 | src/tools/scoped/builder.rs:679-708 |
| SW-S1B-GEN-TRAIT-ERRORS-3 | 2 | medium | `AuthenticationError` maps to `ToolError::InvalidArgs` → reported to the model as "invalid arguments" | src/builtin_tools/generation/mod.rs:26-27 | src/builtin_tools/error.rs:34 |
| SW-S1B-GEN-TRAIT-ERRORS-4 | 1 | medium | `GenerationError::needs_user_action()` has zero production callers | src/generation/error.rs:401 | none |
| SW-S1B-GEN-TRAIT-ERRORS-5 | 1 | medium | `GenerationError::should_fallback()` has zero production callers | src/generation/error.rs:433 | none |
| SW-S1B-GEN-TRAIT-ERRORS-6 | 1 | medium | `GenerationError::user_friendly_message()` has zero production callers | src/generation/error.rs:488 | none |
| SW-S1B-GEN-TRAIT-ERRORS-7 | 1 | medium | `GenerationError::retry_after()` has zero production callers — the upstream `Retry-After` hint is dropped | src/generation/error.rs:463 | none (private duplicate at src/generation/providers/http.rs:131-136) |
| SW-S1B-GEN-TRAIT-ERRORS-8 | 1 | low | `GenerationError::provider_name()` has zero production callers | src/generation/error.rs:472 | none |
| SW-S1B-GEN-TRAIT-ERRORS-9 | 1 | medium | `GenerationProvider::color()` is overridden by ~19 providers but has **no** production consumer (the UI reads `config.color` instead) | src/generation/mod.rs:208 | none |
| SW-S1B-GEN-TRAIT-ERRORS-10 | 5 | medium | `GenerationProvider::default_model()` (trait) vs `GenerationProviderConfig::default_model()` (config) — two "default model" sources of truth; the trait side is dark | src/generation/mod.rs:213 | none |
| SW-S1B-GEN-TRAIT-ERRORS-11 | 1 | medium | `GenerationProvider::check_progress()` — `FalProvider` implements real queue polling, but nothing ever calls it | src/generation/providers/fal/mod.rs:548 | none |
| SW-S1B-GEN-TRAIT-ERRORS-12 | 1 | medium | `GenerationProvider::cancel()` — **no** provider implements it and **no** caller exists | src/generation/mod.rs:269 | none |
| SW-S1B-GEN-TRAIT-ERRORS-13 | 1 | medium | `GenerationProvider::edit_image()` — `OpenAiCompatProvider` implements it, but nothing ever calls it | src/generation/providers/openai_compat/generate.rs:206 | none |
| SW-S1B-GEN-TRAIT-ERRORS-14 | 3 | medium | `edit_url` config knob is settable in Settings but inert in production | src/config/types/generation/provider.rs:96 | src/generation/providers/openai_compat/helpers.rs:45 (reachable only from a test-only entry point) |
| SW-S1B-GEN-TRAIT-ERRORS-15 | 5 | medium | `openai_compat` voice-list drift: live registry returns `[]`, static catalog returns the OpenAI voice list — the "cannot drift" doc claim is false | src/generation/voice_catalog.rs:45 | src/gateway/handlers/generation_providers/voices.rs:199 |
| SW-S1B-GEN-TRAIT-ERRORS-16 | 2 | low | `supports_image_editing()` is a constant-`true` predicate that ignores `edit_endpoint` | src/generation/providers/openai_compat/generate.rs:202 | none (test-only assert) |
| SW-S1B-GEN-TRAIT-ERRORS-17 | 2 | high | `DuplicateProvider` error is **discarded** at registration; in the legacy multi-capability path the second capability is silently dropped | src/generation/registry.rs:112 | src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:77 (error dropped) |
| SW-S1B-GEN-TRAIT-ERRORS-18 | 2 | medium | TTS path drops the typed error entirely: `synth_with_retry` → `None` → `TtsOutcome::Failed` → `None` | src/gateway/voice/outbound.rs:64-100 | src/gateway/voice/outbound.rs:157-159 |
| SW-S1B-GEN-TRAIT-ERRORS-19 | 1 | low | `GenerationError::Cancelled` and `GenerationError::InternalError` have **no production producer** — their `From` arms are unreachable | src/generation/error.rs:96, :100 | none |
| SW-S1B-GEN-TRAIT-ERRORS-20 | 1 | low | `MockGenerationProvider` / `create_mock_generation_provider()` ship in the production lib with zero production consumers | src/generation/mod.rs:386, :554 | none |
| SW-S1B-GEN-TRAIT-ERRORS-21 | 1 | medium | `retry_transient` (the only consumer of `is_retryable()` inside the provider layer) is used by 1 of ~22 providers | src/generation/providers/http.rs:99 | src/generation/providers/openai_image.rs:437 (only caller) |

**Severity tally**: high 3 · medium 14 · low 4 · (UNCERTAIN 0)

**Form tally**: form 1 → 13 · form 2 → 5 · form 3 → 1 · form 4 → 0 · form 5 → 2 · form 6 → 0

---

### SW-S1B-GEN-TRAIT-ERRORS-1 — `From<GenerationError> for AlephError` is dead in production
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/error.rs:645` — `impl From<GenerationError> for AlephError` mapping all 19 variants onto the `AlephError` taxonomy (`RateLimitError`, `Timeout`, `NetworkError`, `authentication`, `invalid_config`, `provider`, `cancelled`, `other`, `NotFound`).
- **Consumer**: **none found**.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'AlephError::from|map_err\(AlephError::from\)|AlephError::from\(' src/generation src/gateway src/builtin_tools src/tools` → only the two impls themselves (`src/builtin_tools/generation/mod.rs:21`, `src/generation/error.rs:646`); no call site.
  - Every production `.generate(` call site was enumerated and checked for an `AlephError` in scope: `src/builtin_tools/generation/speech_generate.rs:218`, `image_generate.rs:149`, `video_generate.rs:99`, `audio_generate.rs:85` (all `map_err(ToolError::from)`) and `src/gateway/voice/outbound.rs:266` (error dropped). `rg -c 'AlephError'` on those files → `speech_generate.rs` 3 hits (all `// Error is now AlephError` comments at :502/:529/:556), `image_generate.rs` 2 hits (comment at :326/:354), `video_generate.rs` 0, `audio_generate.rs` 0, `outbound.rs` 0, `src/generation/registry.rs` 0.
  - `rg -n --glob '*.rs' 'GenerationError' src | cut -d: -f1 | sort -u` cross-checked against files that also mention `AlephError` → only `src/generation/error.rs` itself.
  - The impl's own doc comment (`src/generation/error.rs:636-644`) states the `retry_after` suggestion exists "so gateway/voice/outbound.rs retry classifiers can wait the right amount" — `src/gateway/voice/outbound.rs` contains **no** `AlephError` at all (it consumes `GenerationError::is_retryable()` directly at :82), so the documented consumer does not exist.
- **Symptom**: none directly user-visible (this is a dormant parallel mapping); it is the reason findings 2/3/4/5/6/7 exist — the only mapping that preserves `retry_after`, provider name, HTTP status and `suggestion` is the one nobody calls.
- **Hypothesis**: **CUT** — the impl was written as the intended error boundary and then bypassed when the generation tools were re-homed onto the `AlephTool`/`ToolError` boundary.
- **Confidence**: high
- **Severity**: high

### SW-S1B-GEN-TRAIT-ERRORS-2 — Rate-limit / timeout lose retryability and are banned for the rest of the run
- **Form**: 2 (stub far-end — the far end is a generic bucket that drops the classification the caller needs)
- **Producer**: `src/builtin_tools/generation/mod.rs:19-37` — `impl From<GenerationError> for ToolError`; `RateLimitError` → `Self::Execution(format!("Rate limited: {message}"))`, `ContentFilteredError` → `Execution`, and `_ => Self::Execution(err.to_string())` swallows `TimeoutError`, `QuotaExceededError`, `UnsupportedFeatureError`, `ProviderError`, `DownloadError`, `JobFailedError`, `ModelNotFoundError`, `UnsupportedFormatError`, `UnsupportedDimensionError`, `Cancelled`, `SerializationError`.
- **Consumer**: `src/tools/scoped/builder.rs:679-708` (`tool_result_to_output`) → `src/tools/retry.rs:64-100` (one-shot retry) and `src/harness/agent/act.rs:720`, `:1067-1077` (cross-batch failure memo).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'impl From<GenerationError> for ToolError' src` → `src/builtin_tools/generation/mod.rs:19` only.
  - `rg -n --glob '*.rs' 'map_err\(Into::into\)|map_err\(ToolError::from\)' src/builtin_tools/generation` → `image_generate.rs:226`, `speech_generate.rs:333`, `video_generate.rs:153`, `audio_generate.rs:139`.
  - `rg -n 'fn tool_result_to_output' -A 30 src/tools/scoped/builder.rs` → `ToolResult::Error { retryable: true }` → `ToolError::Transport`; `ToolResult::Error { error, .. }` → `ToolError::Execution`.
  - `sed -n '160,180p' crates/aleph-slot-api/src/tool.rs` → `ToolError::is_retryable()` matches only `Timeout | Transport | ApprovalExpired | Cancelled` → `Execution` is **not** retryable.
  - `rg -n 'retryable' src/tools/adapters/registry_adapter.rs` → `retryable = matches!(e, AlephError::NetworkError | IoError | Timeout)` → `AlephError::Other` is **not** retryable.
  - `rg -n 'AlephError::tool' crates/aleph-slot-api/src/error.rs` → `Self::Other { message, suggestion: None }` (the `suggestion` is dropped).
  - `rg -n 'retryable' src/harness/agent/act.rs` → :720 `if let Some((name, args)) = cache_key.filter(|_| !e.is_retryable())` and :1073 `if !retryable { ... }` — only a NON-retryable failure enters the cross-batch memo.
- **Symptom**: an image/video/speech call that hits a provider 429 arrives at the harness as `retryable: false`. The one-shot retry never fires, and the identical `(tool, args)` pair is **banned for the remainder of the run** by the cross-batch failure memo; the model is handed a persistence hint to climb the tool ladder even though a retry 100 ms later would have succeeded. Same for `TimeoutError` and `DownloadError` (5xx/408), which `GenerationError::is_retryable()` calls retryable (`src/generation/error.rs:351`).
- **Hypothesis**: **CUT** — the typed error is converted at the wrong boundary; the classifier that knows about 429/timeout is never consulted downstream.
- **Confidence**: high
- **Severity**: high

### SW-S1B-GEN-TRAIT-ERRORS-3 — Missing API key is reported to the model as "invalid arguments"
- **Form**: 2 (stub far-end — mis-classifying far end)
- **Producer**: `src/builtin_tools/generation/mod.rs:26-27` — `GenerationError::AuthenticationError { message, .. } => Self::InvalidArgs(format!("Authentication failed: {message}"))`.
- **Consumer**: `src/builtin_tools/error.rs:34` — `ToolError::InvalidArgs(msg) => Self::tool(msg)` → `AlephError::Other`.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'AuthenticationError' src crates | head -30` → producers in `src/generation/providers/factory.rs` (missing/empty API key) and in each provider's HTTP status mapping; the only mapping to a tool error is `src/builtin_tools/generation/mod.rs:26`.
  - `rg -n --glob '*.rs' 'needs_user_action' src crates` → only doctests + `#[cfg(test)]` asserts in `src/generation/error.rs` — **no production caller**, so the "this needs the user to fix a credential" signal is never raised.
- **Symptom**: with a wrong API key the model is told "Invalid arguments: Authentication failed: …" and typically rewrites its arguments and retries, instead of surfacing "your provider credential is invalid" to the user. The `provider` field of `AuthenticationError` (the provider name) is also discarded by this arm.
- **Hypothesis**: **CUT** — the "invalid arguments" bucket is for model-supplied schema errors, not provider credential failures.
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-4 — `needs_user_action()` is never consulted
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/error.rs:401` — `pub const fn needs_user_action(&self) -> bool` (true for `AuthenticationError | QuotaExceededError | InvalidParametersError | ContentFilteredError | ModelNotFoundError`).
- **Consumer**: **none found**.
- **Live caller search**: `rg -n --glob '*.rs' 'needs_user_action' src crates interfaces shared` → `src/generation/error.rs:26` (doctest), `:395`, `:398`, `:401` (definition), `:897-908`, `:1123` — every hit outside the definition is a doctest or a `#[cfg(test)]` assert in `src/generation/error.rs`. **Zero** production callers.
- **Symptom**: none user-visible directly; the "stop and ask the user" class of failure (bad key, exhausted quota, bad model id) is indistinguishable from a transient failure downstream (see finding 2), so the model retries instead of escalating.
- **Hypothesis**: **CUT** — the classifier was written for a prompt-side routing hint that was never wired.
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-5 — `should_fallback()` is never consulted
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/error.rs:433` — `pub const fn should_fallback(&self) -> bool` (true for `UnsupportedFeatureError | UnsupportedGenerationTypeError | UnsupportedFormatError | UnsupportedDimensionError | ModelNotFoundError`).
- **Consumer**: **none found**.
- **Live caller search**: `rg -n --glob '*.rs' 'should_fallback' src crates interfaces shared` → `src/generation/error.rs:427`, `:430` (doctests), `:433` (definition), `:912-923`, `:1124` (tests). **Zero** production callers.
- **Symptom**: `GenerationError::unsupported_generation_type` is produced in production (23 factory calls, e.g. `src/generation/providers/openai_compat/generate.rs:38`, `src/generation/providers/fal/mod.rs:461`) and `unsupported_dimension` at `src/generation/providers/openai_image.rs:194` — the intended "try another provider" hop never happens; the tool just fails, and the model has no route to the second registered provider.
- **Hypothesis**: **CUT** — the registry has `providers_for_type` / `first_for_type` but no error-driven fallback loop.
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-6 — `user_friendly_message()` is never produced
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/error.rs:488` — `pub fn user_friendly_message(&self) -> String`, the only variant that formats a message intended for a human.
- **Consumer**: **none found**.
- **Live caller search**: `rg -n --glob '*.rs' 'user_friendly_message' src crates interfaces shared` → `src/generation/error.rs:488` (definition) + `:964-1141` (tests). A same-named method on a **different** type exists at `crates/aleph-slot-api/src/error.rs` (not this one). **Zero** production callers of the `GenerationError` method.
- **Symptom**: every generation failure the user sees is the raw `Display` of the enum (`src/generation/error.rs` `impl Display`), not the curated human message.
- **Hypothesis**: **CUT**
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-7 — `retry_after()` is never read; the upstream `Retry-After` hint is dropped
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/error.rs:463` — `pub const fn retry_after(&self) -> Option<Duration>` (reads `RateLimitError { retry_after, .. }`).
- **Consumer**: **none found** (a private duplicate exists at `src/generation/providers/http.rs:131-136`).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'retry_after' src crates interfaces shared` → `src/generation/error.rs:460` (doctest), `:463` (definition), `:797`, `:929`, `:932`, `:935` (tests) — **zero** production callers of the method.
  - `sed -n '125,140p' src/generation/providers/http.rs` → `fn retry_after_of(e: &GenerationError)` matches `GenerationError::RateLimitError { retry_after, .. }` directly, i.e. the provider layer re-implements the accessor instead of calling it.
  - `src/gateway/voice/outbound.rs:82` uses `is_retryable()` only; the backoff there is a fixed `TTS_RETRY_BACKOFF` constant, not the provider-supplied delay.
- **Symptom**: a provider that says "retry after 12 s" is retried on the fixed backoff schedule (or not retried at all — finding 2), so the rate limit is re-hit.
- **Hypothesis**: **CUT**
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-8 — `provider_name()` is never read
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/error.rs:472` — `pub const fn provider_name(&self) -> Option<&str>`.
- **Consumer**: **none found**.
- **Live caller search**: `rg -n --glob '*.rs' 'provider_name' src crates interfaces shared` → `src/generation/error.rs:472` (definition) + `:838`, `:846`, `:854`, `:941`, `:945`, `:949`, `:954`, `:956` (all `#[cfg(test)]` asserts) — **zero** production callers.
- **Symptom**: none user-visible; the provider name is present in `AuthenticationError`/`ProviderError`/`ModelNotFoundError` and is dropped by the `ToolError` mapping (`src/builtin_tools/generation/mod.rs:26-37` keeps only `message`), so the user cannot tell which of several configured providers failed.
- **Hypothesis**: **CUT**
- **Confidence**: high
- **Severity**: low

### SW-S1B-GEN-TRAIT-ERRORS-9 — `GenerationProvider::color()` — ~19 overrides, no consumer
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/mod.rs:208` — `fn color(&self) -> &str` (default `"#808080"`), overridden by ~19 providers (e.g. `src/generation/providers/google_veo/provider.rs:608`, `midjourney/provider.rs:265`, `openai_tts/mod.rs:454`, `elevenlabs/mod.rs:493`, `openai_image.rs:530`, `openai_compat/generate.rs:194`).
- **Consumer**: **none found**.
- **Live caller search**:
  - `rg -n --glob '*.rs' '\.color\(\)' src crates interfaces shared` → every hit that is a `GenerationProvider` is a test assert: `src/generation/providers/google_veo/mod.rs:149`, `midjourney/mod.rs:279`, `openai_tts/tests.rs:200`, `replicate/mod.rs:380`, `elevenlabs/tests.rs:166`, `openai_image.rs:692`, `openai_compat/mod.rs:144`, `:312`, `:322`, `src/generation/mod.rs:578`. `src/providers/metering.rs:423` and `src/providers/model_override_provider.rs:78` are the **LLM** `AiProvider::color()` — a different trait, not a consumer.
  - `rg -n 'color' src/gateway/handlers/generation_providers/` → `mod.rs:92`, `:141`, `wire.rs:96`, `:123` — the Settings RPC takes the color from **`c.color` (the config struct)**, not from the provider object.
  - `rg -n 'color' src/generation/registry.rs src/generation/probe.rs src/generation/types/` → no hits outside the trait default/doc.
- **Symptom**: none user-visible. The provider list in the Panel gets its color from `GenerationProviderConfig::color` (`src/config/types/generation/provider.rs:65`), so the trait's per-provider brand colors (e.g. ElevenLabs `#00c7b7`) are never shown; a provider that is not built through the factory (e.g. `LocalVoiceProvider`) would render with the config color anyway.
- **Hypothesis**: **CUT** — the trait method is a duplicate of the config field; the factory copies `config.color` into the builder (`src/generation/providers/factory.rs:248`, `:347-348`, `:384-385`) but nobody reads it back out.
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-10 — Two "default model" sources of truth; the trait side is dark
- **Form**: 5 (name / path drift)
- **Producer**: `src/generation/mod.rs:213` — `fn default_model(&self) -> Option<&str>` (trait), overridden by ~17 providers (`google_veo/provider.rs:612`, `openai_image.rs:534`, `midjourney/provider.rs:269`, `deepgram_tts/mod.rs:386`, `openai_whisper/mod.rs:348`, `deepgram_stt/mod.rs:306`, `openai_compat/generate.rs:198`, `google_imagen.rs:593`, `minimax_tts/mod.rs:418`, `azure_speech/mod.rs:348`, `suno/mod.rs:409`, `stability.rs:546`, `elevenlabs/mod.rs:497`, `cartesia/mod.rs:362`, `bfl/mod.rs:417`).
- **Consumer**: **none found** (the config-side twin *is* used).
- **Live caller search**:
  - `rg -n --glob '*.rs' '\.default_model\(\)' src crates interfaces shared` → the only hits on a **generation provider** are test asserts: `src/generation/providers/replicate/mod.rs:386`, `src/generation/providers/openai_compat/mod.rs:695`, `deepgram_tts/tests.rs:28`, `suno/tests.rs:25`, `cartesia/tests.rs:32`, `deepgram_stt/tests.rs:27`, `openai_whisper/tests.rs:27`.
  - The live path is a **different method on a different type**: `src/config/types/generation/provider.rs:171` `GenerationProviderConfig::default_model()`, consumed by `src/generation/providers/factory.rs:128`, `:138`, `:149`, … `:370` (`config.default_model().map(|s| s.to_string())` → builder). Confirmed by `rg -n 'default_model' src/generation/providers/factory.rs`.
- **Symptom**: the "default model" shown to the user comes from the config, while each provider also carries a hard-coded default model. When the config omits the model, the provider's own default is used inside the provider (`self.model`), so the two can disagree with no signal; e.g. `src/generation/providers/replicate/mod.rs:386` asserts `default_model() == Some(DEFAULT_MODEL)` while the config path may set another model.
- **Hypothesis**: **DECIDE** — decide which of the two is the fact source (config or provider) and delete or wire the other.
- **Confidence**: high (the "no consumer" half is certain; the drift risk is a design call)
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-11 — `check_progress()` — Fal's real queue polling is never called
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/providers/fal/mod.rs:548` — `fn check_progress(&self, job_id)` → `poll_status` → `Ok(GenerationProgress::new(pct, step))` (fal/mod.rs:564), reporting Fal queue position.
- **Consumer**: **none found**.
- **Live caller search**:
  - `rg -n --glob '*.rs' '\.check_progress\(' src crates interfaces shared` → **one** hit: `src/generation/mod.rs:717`, inside the `#[cfg(test)] mod tests` block (it asserts the **default** unsupported behaviour on `MockGenerationProvider`, not Fal's real implementation).
  - `rg -n --glob '*.rs' 'fn check_progress' src` → the trait default `src/generation/mod.rs:242` and the Fal override `src/generation/providers/fal/mod.rs:548` only.
  - `rg -n --glob '*.rs' 'GenerationProgress' src crates interfaces shared` → `src/generation/mod.rs:67`, `:236`, `:245`; `src/generation/types/mod.rs:14`, `:25`, `:339-398` (tests); `src/generation/types/progress.rs:8`, `:21`, `:74`; `src/generation/providers/fal/mod.rs:21`, `:551`, `:564`. No UI / RPC / tool consumer.
  - Fal's own `generate` (`src/generation/providers/fal/mod.rs:455`) polls to a terminal status internally (loop at :483-505) and returns the finished asset — so the async-job progress surface is redundant *as currently used*.
- **Symptom**: a long Fal video/image job shows no progress at all; the caller only sees the final result or a timeout (`GenerationError::timeout` at fal/mod.rs:485 after `JOB_DEADLINE_SECS`).
- **Hypothesis**: **CUT** — the trait surface was designed for async job polling, but the provider polls inline and the progress API was never connected to any UI.
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-12 — `cancel()` — no implementation, no caller
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/mod.rs:269` — `fn cancel(&self, _job_id) -> ...` default → `Err(GenerationError::unsupported_feature("...", "cancel", self.name()))`.
- **Consumer**: **none found**.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'fn cancel' src/generation src/gateway` → **no** `GenerationProvider` override anywhere (the hits are unrelated types: gateway cancellation tokens, the teams coordinator, `in_flight` maps).
  - `rg -n --glob '*.rs' '\.cancel\(' src crates interfaces shared` → no call on a `GenerationProvider` (hits are `CancellationToken::cancel`, etc.).
- **Symptom**: a user cannot cancel a running generation job; the only cancellation is the harness's own tool-call cancellation token (`src/tools/adapters/registry_adapter.rs:462-468`), which does **not** reach the provider's HTTP request.
- **Hypothesis**: **CUT**
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-13 — `edit_image()` — OpenAI-compatible image editing is implemented but never invoked
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/providers/openai_compat/generate.rs:206` — `fn edit_image(&self, request)` → `super::edit::edit_image_impl(self, request)` (`src/generation/providers/openai_compat/edit.rs:25`).
- **Consumer**: **none found**.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'edit_image' src crates interfaces shared` → definition `src/generation/mod.rs:320`, the OpenAI-compat override `src/generation/providers/openai_compat/generate.rs:206`, the doc at `src/generation/mod.rs:316`, and **only** `src/generation/providers/openai_compat/mod.rs:755` and `:778` — both `#[tokio::test]` / `#[test]` functions.
  - `rg -n 'edit' src/builtin_tools/generation/image_generate.rs` → **no** hits: the image generation tool has no edit mode, and there is no `image_edit` tool (`rg -n --glob '*.rs' 'image_edit|edit_image|edit_url' src/builtin_tools src/executor src/gateway src/bin` → only the Settings-RPC `edit_url` plumbing).
- **Symptom**: the "edit this image" capability exists in code and is fully configurable, but the model has no tool to reach it.
- **Hypothesis**: **CUT**
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-14 — `edit_url` is an inert config knob
- **Form**: 3 (inert config)
- **Producer**: `src/config/types/generation/provider.rs:96` — `pub edit_url: Option<String>`, writable through the Settings RPC (`src/gateway/handlers/generation_providers/mod.rs:98`, `:153`; `wire.rs:105`, `:133`).
- **Consumer**: `src/generation/providers/openai_compat/helpers.rs:45` (`edits_url()`), reached only from `edit_image_impl` ← `edit_image` ← **tests only** (finding 13).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'edit_url' src` → config field `src/config/types/generation/provider.rs:96`, `:118`; factory `src/generation/providers/factory.rs:253-254` (`builder = builder.edit_endpoint(edit_url)`); helper `src/generation/providers/openai_compat/helpers.rs:45-46`; Settings RPC `src/gateway/handlers/generation_providers/mod.rs:98`, `:153` and `wire.rs:105`, `:133` (plus `wire.rs:221`, `:244` = tests).
  - The factory arm is the only producer→consumer link and it *is* wired; the severed link is downstream: `edit_endpoint` is only read by `edits_url()`, which is only called from `edit_image_impl`, which is only called from `edit_image`, which has no production caller.
- **Symptom**: a user configures an image-editing endpoint in Settings, the server accepts and persists it, the provider stores it — and nothing ever uses it. No error, no log, no warning.
- **Hypothesis**: **CUT** (the config → provider hop is connected; the provider → behaviour hop is not)
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-15 — `openai_compat` voice list: live path `[]` vs static catalog = OpenAI voices
- **Form**: 5 (name / path drift between two accessors for the same fact)
- **Producer**: `src/generation/voice_catalog.rs:45` — `static_voices_for_provider_type` maps `"openai_compat" | "openai-compat"` to `OpenAiTtsProvider::static_voice_list()` (the OpenAI voice table).
- **Consumer**: `src/gateway/handlers/generation_providers/voices.rs:199` — the Settings RPC `generation_providers.voices` (RPC registered at `src/bin/aleph-server/commands/start/builder/handlers/settings.rs:668-669`).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'fn list_voices' src` → 7 overrides only: `openai_tts/mod.rs:462`, `minimax_tts/mod.rs:422`, `elevenlabs/mod.rs:501`, `cartesia/mod.rs:366`, `volcengine_tts/mod.rs:404`, `deepgram_tts/mod.rs:390`, `azure_speech/mod.rs:352`. **`OpenAiCompatProvider` does not implement it** — its `impl GenerationProvider` block is `src/generation/providers/openai_compat/generate.rs:29` and contains only `generate`, `name`, `supported_types`, `color`, `default_model`, `supports_image_editing`, `edit_image`.
  - `rg -n --glob '*.rs' '\.list_voices\(' src crates` → exactly two hits: `src/generation/registry.rs:452` (production) and the trait definition `src/generation/mod.rs:354`. `registry.get_voices_for_provider` is called from `src/builtin_tools/generation/speech_generate.rs:197`.
  - The module doc of `src/generation/voice_catalog.rs` states the two accessors "cannot drift" — for `openai_compat` they do.
  - `openai_compat` **can** be configured with the speech capability: `src/generation/providers/factory.rs:234` honours `config.capabilities`, and `src/generation/providers/openai_compat/generate.rs:190` returns `supported_types` from the config.
- **Symptom**: the Settings screen shows OpenAI voices for an `openai_compat` provider, but the live registry returns `[]`, so the speech tool's unpublished-voice hint (`unpublished_voice_hint` in `src/builtin_tools/generation/speech_generate.rs`, used on the failed-synthesis path at :218-228) can never fire for that provider — the user gets a bare failure instead of "voice X is not published by this provider".
- **Hypothesis**: **CUT** — the static catalog aliases `openai_compat` to the OpenAI table without the provider implementing `list_voices`.
- **Confidence**: medium (the drift is verified; whether `openai_compat` is *intended* to serve OpenAI voices is a product call — the alias in `voice_catalog.rs:45` says it is, but there is no `list_voices` implementation to honour it)
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-16 — `supports_image_editing()` is a constant-true predicate
- **Form**: 2 (stub far-end)
- **Producer**: `src/generation/providers/openai_compat/generate.rs:202` — `fn supports_image_editing(&self) -> bool { true }` (hard-coded, never consults `self.edit_endpoint`).
- **Consumer**: **none found** (only `src/generation/providers/openai_compat/mod.rs:704`, a `#[test]` assert).
- **Live caller search**: `rg -n --glob '*.rs' 'supports_image_editing' src crates` → definition `src/generation/mod.rs:342`, override `src/generation/providers/openai_compat/generate.rs:202`, assert `src/generation/providers/openai_compat/mod.rs:704`. **Zero** production callers.
- **Symptom**: none user-visible today (nothing calls it). If it is wired as a capability gate, it will claim editing support for an endpoint that may not expose `/edits` — `edits_url()` (`src/generation/providers/openai_compat/helpers.rs:43-51`) falls back to `endpoint.replace("/generations", "/edits")`, which for a non-OpenAI-shaped URL returns the *generations* URL unchanged, so the "edit" request would go to the generate endpoint.
- **Hypothesis**: **DECIDE** — either make it `self.edit_endpoint.is_some()` (or check the derived URL) or delete it.
- **Confidence**: high
- **Severity**: low

### SW-S1B-GEN-TRAIT-ERRORS-17 — `DuplicateProvider` is discarded at registration; a capability is silently dropped
- **Form**: 2 (stub far-end — the far end discards the error that carries the only actionable text)
- **Producer**: `src/generation/registry.rs:112` — `GenerationError::duplicate_provider(name)` inside `register`.
- **Consumer**: `src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:77` — `if registry.register(name.clone(), provider).is_ok() { tracing::info!(...) }` — the `Err` is **dropped without logging**; the hot-reload path at `:133` does `new_registry.register(name.clone(), provider).ok();` — also dropped.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'DuplicateProvider|duplicate_provider' src crates` → producer `src/generation/registry.rs:112`; the only consumers of the variant are `src/generation/error.rs:626` (`user_friendly_message`) and `:766` (`AlephError::invalid_config("Duplicate generation provider '{name}' — remove the duplicate [generation.{name}] section from your config")`) — **both inside the unreachable mapping of finding 1**; `src/generation/error.rs:1041-1044` (test).
  - `rg -n 'register' src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs` → `:77` and `:133` (both discard). By contrast the *provider construction* error **is** logged at `:82`: `tracing::warn!(provider = %name, error = %e, "Skip generation provider")` — so the asymmetry is deliberate-looking but the duplicate case has no log at all.
  - The condition is reachable: `src/config/types/generation/config.rs:194-223` `merged_providers()` legacy branch pushes **one entry per capability** with the **same name** (`for gen_type in &cfg.capabilities { result.push((name.clone(), cfg.clone(), *gen_type)); }`), and `registry.register` (`src/generation/registry.rs:110-112`) rejects the second with the same name.
  - Consequence: for `provider_type = "local"`, `LocalVoiceProvider::from_config` binds exactly one capability (`src/gateway/voice/local_provider.rs:149-159`), so the second registration is the one that carries the second capability — and it is dropped, silently. For `openai_compat` the first registration already covers all capabilities (`src/generation/providers/openai_compat/generate.rs:190`), so nothing is lost there.
  - The typed maps are de-duplicated (`src/config/types/generation/config.rs:345-356`, a shared `seen` set) and set `cfg.capabilities = vec![gen_type]`, so the legacy branch is the leaky one.
- **Symptom**: a user who configures `provider_type = "local"` with two capabilities (e.g. `speech` + `stt`) gets only the first one; the provider list in the Panel shows one capability; nothing is logged; the actionable text ("remove the duplicate [generation.X] section from your config") never appears anywhere.
- **Hypothesis**: **CUT**
- **Confidence**: high
- **Severity**: high

### SW-S1B-GEN-TRAIT-ERRORS-18 — The TTS path drops the typed error entirely
- **Form**: 2 (stub far-end — the error is formatted into a log line and then discarded)
- **Producer**: `src/gateway/voice/outbound.rs:64-100` — `synth_with_retry` returns `Option<GenerationOutput>`; on failure it only does `warn!(provider = %provider_id, attempt = n, error = %e, "TTS generation failed")` and returns `None`.
- **Consumer**: `src/gateway/voice/outbound.rs:275-278` → `return Some(TtsOutcome::Failed)`; `src/gateway/voice/outbound.rs:157-159` → `TtsOutcome::Failed => None` in `generate_tts`.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'synth_with_retry' src` → defined and called only inside `src/gateway/voice/outbound.rs`.
  - `rg -n 'TtsOutcome' src/gateway/voice/outbound.rs` → `:157-159` (`Failed` → `None`), `:275-278`, enum at `:336-340`.
  - `rg -n --glob '*.rs' 'generate_tts\(|generate_tts_outcome\(' src` → callers in the voice gateway; the `Option`/`TtsOutcome` distinction is lost at `:157-159` for the legacy path.
  - `rg -c 'AlephError' src/gateway/voice/outbound.rs` → **0**: the outbound voice path never converts the `GenerationError` into the shared error taxonomy.
- **Symptom**: when TTS fails (bad key, quota, network), the user gets **silence** — no message, no attachment, no error surfaced; the only trace is a server-side `warn!` log. The typed error (which knows auth vs. quota vs. network) is dropped.
- **Hypothesis**: **CUT**
- **Confidence**: high
- **Severity**: medium

### SW-S1B-GEN-TRAIT-ERRORS-19 — `Cancelled` and `InternalError` have no production producer
- **Form**: 1 (no consumer — the producing arms are unreachable in production)
- **Producer**: `src/generation/error.rs:96` (`Cancelled`) and `:100` (`InternalError { message }`).
- **Consumer**: **none found** (their `From` arms at `src/generation/error.rs:721` and `:722` are therefore unreachable).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'GenerationError::cancelled\(' src crates` → only `src/generation/error.rs:893`, `:1103`, `:1119` — all inside `#[cfg(test)] mod tests`.
  - `rg -n --glob '*.rs' 'GenerationError::internal\(' src crates` → `src/generation/error.rs:1130` (test) and `src/generation/registry.rs:216`, inside `pub(crate) fn get_or_err` (registry.rs:214). `get_or_err` callers: `src/generation/registry.rs:207`, `:210` (doc comments) and `:572`, `:578`, `:584`, `:587` (**all inside `#[cfg(test)] mod tests`**, which starts at registry.rs:496) → **no production caller**.
  - Near-miss for cancellation: `src/generation/providers/openai_compat/generate.rs:312-313` matches a provider status of `"cancelled"`/`"canceled"` but maps it to `GenerationError::job_failed(reason, None)` at `:321`, not to `Cancelled`.
- **Symptom**: none user-visible; the taxonomy has two dead branches, and the "provider cancelled the job" condition is reported as a generic job failure.
- **Hypothesis**: **CUT**
- **Confidence**: high
- **Severity**: low

### SW-S1B-GEN-TRAIT-ERRORS-20 — `MockGenerationProvider` ships in the production library with no production consumer
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/mod.rs:386` — `pub struct MockGenerationProvider`; `src/generation/mod.rs:554` — `pub fn create_mock_generation_provider()`.
- **Consumer**: **none found** (outside doc examples and tests).
- **Live caller search**: `rg -n --glob '*.rs' 'MockGenerationProvider::new|create_mock_generation_provider\(' src crates interfaces shared | grep -v '^\s*///'` → `src/builtin_tools/generation/speech_generate.rs:345-346` (inside `#[cfg(test)] mod tests`), `src/generation/registry.rs:502` (test helper), `src/generation/mod.rs:567`, `:629`, `:647`, `:656`, `:663`, `:673`, `:685`, `:696`, `:715`, `:729`, `:745` (all inside `#[cfg(test)] mod tests` in `src/generation/mod.rs`), plus `src/generation/registry.rs` doc comments.
- **Symptom**: none user-visible; a test double is part of the public API surface of the production crate.
- **Hypothesis**: **CUT** (deliberate public test double — recommend `#[doc(hidden)]` or a `testing` feature rather than removal).
- **Confidence**: high
- **Severity**: low

### SW-S1B-GEN-TRAIT-ERRORS-21 — `retry_transient` is used by 1 of ~22 providers
- **Form**: 1 (no consumer — for the other providers)
- **Producer**: `src/generation/providers/http.rs:99` — `pub(crate) async fn retry_transient<F, Fut, T>(op_name, op)`; it is the **only** in-provider consumer of `GenerationError::is_retryable()` (at `src/generation/providers/http.rs:111`: `Err(e) if e.is_retryable() && (attempt as usize) < BACKOFFS.len() =>`).
- **Consumer**: `src/generation/providers/openai_image.rs:437` — the only call site.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'retry_transient' src` → `src/generation/providers/http.rs:99` (definition), `src/generation/providers/openai_image.rs:373` (comment), `src/generation/providers/openai_image.rs:437` (the single call).
  - `rg -n --glob '*.rs' 'is_retryable' src crates` → `src/generation/providers/http.rs:111` (above) and `src/gateway/voice/outbound.rs:82` (`if !e.is_retryable() { return None; }` — the TTS path). Every other provider maps 429/5xx to a `RateLimitError`/`ProviderError` and returns it; none retries.
- **Symptom**: 11 of 12 providers that can receive a 429 do not retry at all, so a transient rate limit surfaces to the model as a hard failure — and, per finding 2, is then banned for the rest of the run.
- **Hypothesis**: **CUT** — the shared retry helper was adopted by one provider only.
- **Confidence**: high
- **Severity**: medium

---

## B. Clean / verified symbols (no severed wire)

| Symbol | Producer | Production consumer | Evidence |
|--------|----------|---------------------|----------|
| `GenerationProvider::name()` | `src/generation/mod.rs:160` | `src/generation/registry.rs:120` (canonical-name index inside `register`) | `rg -n 'provider\.\w+\(' src/generation/registry.rs` → `:120` `provider.name()`, `:452` `provider.list_voices()`, `:786` `provider.generate(...)` (inside `#[cfg(test)] mod tests` starting registry.rs:496) |
| `GenerationProvider::supports()` | `src/generation/mod.rs:195` (default) | `src/generation/registry.rs:306` (`providers_for_type`), `:319` (`has_provider_for_type`), `:482` (`first_for_type`); `src/generation/providers/factory.rs:93`, `:409`; `src/builtin_tools/generation/speech_generate.rs:187` | `rg -n 'fn supports' src/generation` → only the trait default `src/generation/mod.rs:195` and one override `src/generation/providers/replicate/provider.rs:264` |
| `GenerationProvider::supported_types()` | `src/generation/mod.rs:180` | reached in production **through** the `supports()` default (`src/generation/mod.rs:196` → `self.supported_types().contains(&gen_type)`); no direct production caller | `rg -n 'supported_types\(\)' src crates` → direct calls are all test asserts (`google_veo/mod.rs:124`, `midjourney/mod.rs:254`, `openai_tts/tests.rs:181`, `replicate/mod.rs:400`, `elevenlabs/tests.rs:187`, `openai_image.rs:673`, `openai_compat/mod.rs:276`, `:288`, `google_imagen.rs:683`, `fal/mod.rs:602`) plus the unrelated `src/media/provider.rs:26` |
| `GenerationProvider::list_voices()` | `src/generation/mod.rs:354` | `src/generation/registry.rs:452` (`get_voices_for_provider`) → `src/builtin_tools/generation/speech_generate.rs:197` → `unpublished_voice_hint` | `rg -n '\.list_voices\(' src crates` → exactly 2 hits (registry.rs:452 + the trait definition) |
| `GenerationError::is_retryable()` | `src/generation/error.rs:351` | `src/generation/providers/http.rs:111`, `src/gateway/voice/outbound.rs:82` | `rg -n --glob '*.rs' 'is_retryable' src crates` — the only 2 non-test hits |
| `GenerationError` → `builtin_tools::error::ToolError` | `src/builtin_tools/generation/mod.rs:19` | `src/builtin_tools/generation/{image_generate.rs:226, speech_generate.rs:333, video_generate.rs:153, audio_generate.rs:139}` via `map_err(Into::into)` | the wire is **connected** — it is the *content* of the mapping that loses information (finding 2/3) |
| `GenerationProviderRegistry::register / get / providers_for_type / first_for_type / iter / remove / clear` | `src/generation/registry.rs:102`, `:171`, `:304`, `:478`, `:437`, `:389` | `src/tools/probes/generation.rs:51`, `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:134`, `src/executor/builtin_registry/builder/optional_tools.rs:300,313,325,337`, `src/builtin_tools/generation/*.rs`, `src/gateway/voice/outbound.rs:124` | all are production call sites |
| `GenerationProvider::generate()` | `src/generation/mod.rs:~150` | `src/builtin_tools/generation/speech_generate.rs:218`, `image_generate.rs:149`, `video_generate.rs:99`, `audio_generate.rs:85`, `src/gateway/voice/outbound.rs:266` | the 5 production `.generate(` call sites; `src/generation/registry.rs:786` is a test |
| `probe_generation_provider` / `GenerationProbeOutcome` | `src/generation/probe.rs` | `src/gateway/handlers/generation_providers/` (probe RPC) and `src/tools/probes/generation.rs:51` | production probe path is wired |

---

## C. New observations (not findings)

1. **The `AlephError` mapping's own doc comment points at a consumer that does not exist.** `src/generation/error.rs:654-660` says the `retry_after` suggestion exists "so gateway/voice/outbound.rs retry classifiers can wait the right amount instead of falling back to the Aleph-side default (60 s)". `src/gateway/voice/outbound.rs` uses `GenerationError` directly (`is_retryable()` at :82) and contains no `AlephError` at all (`rg -c 'AlephError' src/gateway/voice/outbound.rs` → 0). The documented beneficiary of the mapping is on a different error path.
2. **Two independent "rate-limit" representations coexist.** `AlephError::RateLimitError { message, suggestion }` (used only by the dead mapping) vs. the live `ToolError::Execution("Rate limited: …")` string prefix. Any future consumer keying on `AlephError::RateLimitError` will find nothing in production.
3. **`GenerationProviderConfig::color` and `GenerationProviderConfig::default_model` are the fact sources actually used** (`src/config/types/generation/provider.rs:65`, `:171`), and the factory copies them into every provider builder (`src/generation/providers/factory.rs:248`, `:347-348`, `:384-385`, `:128`…`:370`). The trait's `color()`/`default_model()` are therefore a *shadow* copy that can silently disagree with the config for any provider not built through the factory (e.g. `LocalVoiceProvider`, `src/gateway/voice/local_provider.rs:236`).
4. **`AlephError::tool()` drops the `suggestion`** (`crates/aleph-slot-api/src/error.rs:590-595` → `Self::Other { message, suggestion: None }`), so even a correctly-classified generation error would lose its remediation hint at that hop.
5. **`GenerationError::Display` is the only user-facing text today.** Because `user_friendly_message()` has no caller, the raw enum `Display` is what reaches the model and (in the TTS path) what is discarded entirely.
6. **`FalProvider` polls to completion inside `generate`** (`src/generation/providers/fal/mod.rs:483-505`), so `check_progress` is a redundant second polling path rather than a missing one — but a user still sees no progress during the wait, and the deadline surfaces as a bare `GenerationError::timeout` (fal/mod.rs:485).
7. **`openai_compat` maps a provider status of `"cancelled"`/`"canceled"` to `job_failed`** (`src/generation/providers/openai_compat/generate.rs:312-313`, `:321`), so `GenerationError::Cancelled` is never produced in production (finding 19).
8. **`registry.rs` has no colour accessor at all** — `rg -n 'color' src/generation/registry.rs src/generation/probe.rs src/generation/types/` returns nothing, confirming the trait's `color()` never had a registry-level consumer.

---

## D. Coverage statement

**Files read in full**: `src/generation/mod.rs` (798 lines), `src/generation/error.rs` (1145 lines).

**Files read in part (to anchor both ends of a wire)**: `src/generation/registry.rs`, `src/generation/probe.rs`, `src/generation/voice_catalog.rs`, `src/generation/providers/openai_compat/{generate.rs, helpers.rs, mod.rs}`, `src/generation/providers/fal/mod.rs`, `src/generation/providers/factory.rs`, `src/builtin_tools/generation/{mod.rs, image_generate.rs, speech_generate.rs}`, `src/builtin_tools/error.rs`, `src/gateway/voice/outbound.rs`, `src/gateway/voice/local_provider.rs`, `src/gateway/handlers/generation_providers/{mod.rs, wire.rs, voices.rs}`, `src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs`, `src/tools/adapters/registry_adapter.rs`, `src/tools/scoped/builder.rs`, `src/tools/scoped/dispatch.rs`, `src/tools/retry.rs`, `src/harness/agent/act.rs`, `crates/aleph-slot-api/src/tool.rs`, `crates/aleph-slot-api/src/error.rs`, `src/config/types/generation/{provider.rs, config.rs}`.

**Lenses applied**:
1. *Registration parity* — trait methods vs. registry accessors; `registry.rs` uses only `name`, `supports`, `list_voices` in production (`rg -n 'provider\.\w+\(' src/generation/registry.rs`).
2. *Call-vs-handler parity* — every production `.generate(`/`.check_progress(`/`.cancel(`/`.edit_image(`/`.list_voices(`/`.color(`/`.default_model(` call site enumerated repo-wide.
3. *Classifier-vs-handler parity* — `is_retryable` / `needs_user_action` / `should_fallback` / `retry_after` / `provider_name` vs. the handlers that actually consume an error; the live consumption path (`ToolError` → `AlephError` → `ToolResult::Error { retryable }` → `tool_result_to_output` → harness memo) was traced end to end.
4. *Event emit-vs-subscribe parity* — n/a for this slice (the generation registry is not event-based; the closest analogue, `GenerationProgress`, was checked and is emitted only by Fal and consumed by nobody).
5. *Config-reader parity* — `GenerationProviderConfig` (`color`, `default_model`, `edit_url`, `capabilities`) vs. every non-test read of it, plus the `merged_providers()` legacy branch that creates the duplicate-name condition of finding 17.
6. *Path / route parity* — `generation_providers.voices` RPC (Settings) vs. `static_voices_for_provider_type` vs. the live registry path (finding 15); `generation_providers` CRUD RPC vs. the `edit_url` knob (finding 14).
7. *Stub sweep* — `todo!()` / `unimplemented!()` / `// TODO` inside the slice: none found in `src/generation/mod.rs` or `src/generation/error.rs`; the stub-shaped far ends found were `supports_image_editing` (constant `true`), the discarded `DuplicateProvider`, the dropped TTS error, and the dead `From<GenerationError> for AlephError`.

**Seams NOT covered (and why)**:
- **The 22 provider implementations themselves** (`google_veo`, `midjourney`, `openai_tts`, `bfl`, `fal`, `replicate`, `openai_whisper`, `deepgram_stt`, `minimax_tts`, `suno`, `stability`, `elevenlabs`, `cartesia`, `openai_image`, `volcengine_tts`, `deepgram_tts`, `azure_speech`, `google_imagen`, `openai_compat`, `local`, `mock`) were **not** audited file-by-file; only their trait-method implementations and error-mapping arms were grepped. A provider-internal severed wire (e.g. a per-provider HTTP retry that never fires) is therefore out of scope here.
- **`src/gateway/voice/inbound/` (the STT path)** was not read; it does not reference `GenerationError` (`rg` over `src/gateway` found no `GenerationError` outside `voice/outbound.rs` and `voice/local_provider.rs`), but its own end-to-end wiring was not traced.
- **The Panel / TSX front-end** was not searched for a consumer of provider `color`/`default_model`; the check was limited to the Rust side. If the Panel reads these fields over RPC, finding 9/10 would change from "no consumer" to "drift between RPC and trait".
- **`crates/aleph-slot-api/src/error_kind.rs` classification** was read only at the `is_retryable` level (`crates/aleph-slot-api/src/tool.rs:164-171`); the richer `kind()` taxonomy was not swept for generation-specific cases.
- **Runtime behaviour was not executed** (no `cargo` per the read-only constraint): every claim is a static-code claim about reachability, confirmed by whole-repo `rg`.
- **`src/gateway/handlers/generation_providers/` request/response wire structs** were checked for `color`/`edit_url`/`voices` only; the full CRUD surface was not audited for other ghost fields.
