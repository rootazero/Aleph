# S1A — Severed-wire audit: `src/generation/{registry.rs, probe.rs, voice_catalog.rs, types/*.rs}`

- Auditor: `sev-auditor` (child subagent), read-only.
- Repo: `/home/zou/data/workspace/Aleph`, HEAD `1b959fdf4` ("Merge origin/main with the Prism Panel integration"), working tree clean at audit time.
- Slice read in full: `src/generation/registry.rs` (835 L), `src/generation/probe.rs` (308 L), `src/generation/voice_catalog.rs` (104 L), `src/generation/types/mod.rs` (404 L), `src/generation/types/output.rs` (262 L), `src/generation/types/params.rs` (356 L), `src/generation/types/progress.rs` (78 L), `src/generation/types/request.rs` (106 L), `src/generation/types/generation_type.rs` (80 L).
- Method: every candidate was re-confirmed with `rg` over the **whole repo** (`src/`, `interfaces/`, `shared/`, `crates/`, `tests/`). `graphify` used only for orientation; no graph answer was trusted without an `rg` confirmation.
- Every "no consumer" claim below was checked against `#[cfg(test)]` / `mod tests` / `#[test] fn` boundaries; where a symbol is test-only that is stated explicitly.

## A. Findings

| ID | Form | Severity | Title | Producer | Consumer |
|----|------|----------|-------|----------|----------|
| SW-S1A-GEN-REGISTRY-TYPES-01 | 1 | high | `GenerationProgress` is produced by the FAL provider but no production code ever calls `check_progress` | `src/generation/types/progress.rs:8` / `src/generation/providers/fal/mod.rs:564` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-02 | 1 | medium | `GenerationProvider::cancel` has zero production callers — async generation jobs cannot be cancelled | `src/generation/mod.rs:269` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-03 | 1 | medium | `GenerationProvider::edit_image` has zero production callers — the image-edit / `reference_image` path is unreachable | `src/generation/providers/openai_compat/generate.rs:206` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-04 | 1 | high | `GenerationOutput.additional_outputs` is filled by 6 providers but read by nobody — extra images are silently dropped | `src/generation/types/output.rs:207` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-05 | 1 | high | `GenerationMetadata.extra` is filled by 4 providers (Suno's alternative clips, BFL, Deepgram, Midjourney) but read by nobody | `src/generation/types/output.rs:115` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-06 | 1 | medium | `GenerationMetadata.provider/duration/seed/width/height/duration_seconds` and `GenerationOutput.generation_type` are written but have no production reader | `src/generation/types/output.rs:94-112`, `:203` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-07 | 1 | medium | `GenerationRequest.request_id` / `.user_id` are never written in production, so 18 providers' read sites are permanently `None` | `src/generation/types/request.rs:29`, `:31` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-08 | 1 | high | `GenerationRequest::transcription()` has zero callers — the Whisper/Deepgram-STT providers are registered but unreachable | `src/generation/types/request.rs:103` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-09 | 1 | high | 12 of 20 `GenerationParams` fields have no production producer (`n, seed, duration_seconds, fps, language, model, negative_prompt, guidance_scale, steps, reference_image, reference_audio, extra`) | `src/generation/types/params.rs:37-76` | providers read them, nothing writes them |
| SW-S1A-GEN-REGISTRY-TYPES-10 | 1 | low | `GenerationParamsBuilder` and `GenerationParams::new/merge/merged_with` have zero production callers (test-only) | `src/generation/types/params.rs:82`, `:99`, `:128`, `:206`, `:217` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-11 | 1 | low | `GenerationData::as_url/as_bytes/as_local_path/is_bytes/is_url/is_local_path` have zero production callers (test-only) | `src/generation/types/output.rs:43-88` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-12 | 1 | low | `GenerationType::supports_style/supports_voice/is_long_running` have zero production callers (test-only) | `src/generation/types/generation_type.rs:39`, `:49`, `:59` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-13 | 1 | medium | 5 `GenerationProviderRegistry` methods have no production consumer: `has_provider_for_type`, `remove`, `clear`, `iter`, `contains` | `src/generation/registry.rs:263`, `:269`, `:275`, `:304`, `:322`, `:389`, `:427`, `:437` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-14 | 1 | low | `CATALOG_GENERATED_AT` has no reader; its doc claims it is surfaced in the Settings RPC payload | `src/generation/voice_catalog.rs:32` | none found |
| SW-S1A-GEN-REGISTRY-TYPES-15 | 3 | high | The probe cannot resolve a URL for the `google` / `google_veo` presets — those providers can never be marked `verified`, so the Panel's "set as default" stays disabled | `src/generation/probe.rs:68-78` | `src/generation/providers/google_imagen.rs:50`, `src/generation/providers/google_veo/provider.rs:116` (built-in defaults never consulted) |
| SW-S1A-GEN-REGISTRY-TYPES-16 | 5 | medium | The probe's `NO_MODELS_ENDPOINT` list is missing factory aliases (`mj`, `imagen`, `google_imagen`) → a 404 is reported as "连接成功" and `verified=true` is persisted | `src/generation/probe.rs:98-107` | `src/generation/providers/factory.rs:273`, `:362` |
| SW-S1A-GEN-REGISTRY-TYPES-17 | 5 | high | The 6 `openai_compat` presets store a full POST endpoint in `base_url`; the probe GETs it, so any status except 401/403 is reported as "连接成功" | `src/generation/probe.rs:74-77` | `src/config/types/generation/presets/registry.rs:58-119` |
| SW-S1A-GEN-REGISTRY-TYPES-18 | 3 | high | `[generation.providers.*.defaults]` — 13 of 14 `GenerationDefaults` fields are parsed, validated, edited in the Panel and sent over the wire, but never applied to any generation request | `src/config/types/generation/defaults.rs:30-83` | none found (only `.voice` is read, `src/generation/providers/factory.rs:139,209,219,229,331`) |
| SW-S1A-GEN-REGISTRY-TYPES-19 | 5 | low | `static_voices_for_provider_type("openai")` returns the OpenAI-TTS voice list, but the factory maps `provider_type="openai"` to the **image** provider | `src/generation/voice_catalog.rs:45` | `src/generation/providers/factory.rs:124` |

**Severity tally**: high 7 · medium 6 · low 6 · (UNCERTAIN 0)

---

### SW-S1A-GEN-REGISTRY-TYPES-01 — `GenerationProgress` is produced but never consumed
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/types/progress.rs:8` — `pub struct GenerationProgress { percentage, step, eta, is_complete, preview_url }`; the only production construction is `src/generation/providers/fal/mod.rs:564` `Ok(GenerationProgress::new(pct, step))` inside `impl GenerationProvider::check_progress` (`src/generation/providers/fal/mod.rs:548`).
- **Consumer**: none found.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'check_progress' .` → `src/generation/providers/fal/mod.rs:548` (impl), `src/generation/mod.rs:242` (trait default), `src/generation/mod.rs:249` (error string), `src/generation/mod.rs:714`/`717` (`fn test_check_progress_not_supported` — inside `#[cfg(test)] mod tests`).
  - `rg -n --glob '*.rs' '\.progress\s*\(' .` → 0 hits for a `GenerationProvider` receiver (the only hit is a doc comment in `src/agents/progress.rs:6`).
  - `rg -n --glob '*.rs' 'GenerationProgress' .` → the only non-test uses are the FAL impl and the type's own definition/re-export (`src/generation/types/mod.rs:25`, `src/generation/mod.rs:67`).
- **Symptom**: A FAL video/audio job runs to completion with no progress signal anywhere; the Panel never shows a percentage or a preview URL. The whole `GenerationProgress` surface (percentage / step / eta / is_complete / preview_url) is dark.
- **Hypothesis**: CUT — the far end (an RPC, event or polling handler that would surface progress) was never written; the trait method exists and compiles, so the feature "looks present but is dark".
- **Confidence**: high
- **Severity**: high

### SW-S1A-GEN-REGISTRY-TYPES-02 — `GenerationProvider::cancel` has zero production callers
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/mod.rs:269` — `fn cancel(&self, job_id: &str) -> ...` (trait method with an `unsupported_feature` default).
- **Consumer**: none found.
- **Live caller search**: `rg -n --glob '*.rs' 'check_progress|cancel' src/generation/mod.rs` → only the trait defs and `src/generation/mod.rs:728`/`731` (`fn test_cancel_not_supported`, `#[cfg(test)]`). Repo-wide `rg -n --glob '*.rs' '\.cancel\(' .` matches only `CancellationToken::cancel` in `crates/aleph-slot-api/src/cancellation.rs:86`, `src/acp/manager/**` — no `Arc<dyn GenerationProvider>` receiver anywhere.
- **Symptom**: A long-running FAL/Replicate generation cannot be cancelled; the user can only wait for the poll loop to time out.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: medium

### SW-S1A-GEN-REGISTRY-TYPES-03 — `edit_image` is unreachable, so `params.reference_image` is dead
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/providers/openai_compat/generate.rs:206` — `fn edit_image(...)` → `src/generation/providers/openai_compat/edit.rs:25` (`edit_image_impl`), which reads `request.params.reference_image` (`edit.rs:152`, `:174`) and `request.params.extra["mask"]` (`edit.rs:151`, `:305`).
- **Consumer**: none found.
- **Live caller search**: `rg -n --glob '*.rs' 'edit_image' .` → trait decl `src/generation/mod.rs:320`, doc example `:309`/`:316`, impl `src/generation/providers/openai_compat/generate.rs:206`, impl body `src/generation/providers/openai_compat/edit.rs:25`, and **only two call sites, both in the test module** `src/generation/providers/openai_compat/mod.rs:755` and `:778` (`fn test_edit_image_requires_reference_image`, `fn test_edit_image_wrong_type_fails`). The builtin tool `src/builtin_tools/generation/image_generate.rs:147` calls `provider.generate(request)` only.
- **Symptom**: Image editing (reference image → new image) is unreachable from the product; `GenerationParams.reference_image` can never be non-`None` (see SW-09).
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: medium

### SW-S1A-GEN-REGISTRY-TYPES-04 — `additional_outputs` (extra generated images) are silently dropped
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/types/output.rs:207` — `pub additional_outputs: Vec<GenerationData>`; written by 6 providers: `src/generation/providers/openai_image.rs:515`, `src/generation/providers/openai_compat/edit.rs:434`, `src/generation/providers/stability.rs:526`, `src/generation/providers/google_imagen.rs:573`, `src/generation/providers/replicate/provider.rs:247`, `src/generation/providers/google_veo/provider.rs:592` (all `output.with_additional_outputs(...)`).
- **Consumer**: none found. The only reader of the field is `GenerationOutput::all_outputs()` (`src/generation/types/output.rs:259-261`), and `all_outputs()`'s only caller is `src/generation/types/mod.rs:319`, which is inside `#[cfg(test)] mod tests` (`src/generation/types/mod.rs:28`).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'additional_outputs|all_outputs' .` → 18 hits; the only `.all_outputs()` call is `src/generation/types/mod.rs:319` (test).
  - `rg -n --glob '*.rs' 'output\.data\b' src/builtin_tools/generation/` → `image_generate.rs:153`, `video_generate.rs:103`, `audio_generate.rs:89`, `speech_generate.rs:229` — every tool reads **only** `output.data` (the first image), never `output.all_outputs()`.
- **Symptom**: Asking for `n = 4` images costs 4 generations and the user receives 1; the other 3 are discarded in memory.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: high

### SW-S1A-GEN-REGISTRY-TYPES-05 — `GenerationMetadata.extra` (Suno alternative clips, BFL/Deepgram/Midjourney extras) has no reader
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/types/output.rs:115` — `#[serde(flatten)] pub extra: HashMap<String, serde_json::Value>`; written by `src/generation/providers/midjourney/provider.rs:218` and `:222`, `src/generation/providers/bfl/mod.rs:378` and `:383`, `src/generation/providers/deepgram_stt/mod.rs:258` and `:269`, and `src/generation/providers/suno/mod.rs:376-378` (`metadata.extra.insert("clips", …)` — the comment at `:361` says *"Surface alternative clips so callers can fetch them"*).
- **Consumer**: none found.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'extra\.get\(|extra\[|extra\.iter|extra\.keys|extra\.values' src/generation/ src/builtin_tools/generation/ src/gateway/voice/` → only **request-side** reads (`request.params.extra.get("mask")` at `openai_compat/edit.rs:151`, `:305`; doc mentions at `minimax_tts/mod.rs:17`, `:449`, `volcengine_tts/mod.rs:19`). No `metadata.extra` read anywhere.
  - `rg -n --glob '*.rs' 'metadata\.(provider|model|duration|seed|revised_prompt|content_type|size_bytes|width|height|duration_seconds|extra)\b' src/ interfaces/ shared/ crates/` → `metadata.extra` appears only as `metadata.extra.insert(...)` producers.
  - Suno's module doc `src/generation/providers/suno/mod.rs:18` claims *"Additional clips are surfaced via `metadata.extra["clips"]`"* — they are surfaced to nobody.
- **Symptom**: A Suno generation returns 2 clips (the provider polls *"typically 2 clips per request"*, `suno/mod.rs:8`); the user gets only the first, and the alternative clip URLs (`audio_url`, `video_url`, `image_url`) are lost. Same for BFL/Deepgram/Midjourney extras.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: high

### SW-S1A-GEN-REGISTRY-TYPES-06 — Metadata / output fields written but never read
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/types/output.rs:94` (`provider`), `:98` (`duration`), `:100` (`seed`), `:108`/`:110` (`width`/`height`), `:112` (`duration_seconds`), `:203` (`GenerationOutput.generation_type`); written by providers via `with_provider` / `with_duration` / `with_seed` / `with_dimensions` / `with_duration_seconds` / `GenerationOutput::new(type, data)` (e.g. `src/generation/providers/stability.rs:486-487`, `src/generation/providers/openai_image.rs:496`, `src/generation/providers/suno/mod.rs:352-359`, `src/generation/providers/replicate/provider.rs:212`).
- **Consumer**: none found.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'metadata\.(provider|seed|width|height|duration_seconds|duration)\b' src/ interfaces/ shared/ crates/` → only `src/generation/types/mod.rs:272-279` (test asserts), `src/generation/mod.rs:701`/`:790` (test asserts), and `src/memory/store/markdown/store_impl.rs:511`/`:526` — a **different** `metadata` type (`MarkdownMetadata`).
  - `rg -n --glob '*.rs' 'output\.generation_type' .` → only `src/generation/types/mod.rs:300` and `src/generation/mod.rs:574,588,594,600,606,787` — all inside `#[cfg(test)]`.
  - Production readers of `metadata` exist, but only for `model` (`image_generate.rs:202`, `video_generate.rs:131`, `audio_generate.rs:117`), `revised_prompt` (`image_generate.rs:200`), `content_type` (`image_generate.rs:158`) and `size_bytes` (`speech_generate.rs:231`, `:234`).
- **Symptom**: The provider name, the seed, the pixel dimensions, the elapsed time and the generation type are computed and then thrown away; a user cannot see "seed 12345" or "1024×1024" for a generated image.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: medium

### SW-S1A-GEN-REGISTRY-TYPES-07 — `GenerationRequest.request_id` / `.user_id` are never written in production
- **Form**: 1 (no producer / no consumer)
- **Producer**: `src/generation/types/request.rs:29` (`pub request_id: Option<String>`), `:31` (`pub user_id: Option<String>`); setters `with_request_id` (`:71`) and `with_user_id` (`:77`).
- **Consumer**: none found for the *write* side (the read side is large but always sees `None`).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'with_request_id' src/ interfaces/ shared/ crates/` → **all** hits are `output.with_request_id(id)` (i.e. `GenerationOutput::with_request_id`, `src/generation/types/output.rs:247`), never `GenerationRequest::with_request_id`.
  - `rg -n --glob '*.rs' 'with_user_id' .` → `src/generation/providers/openai_image.rs:737` (test), `src/generation/providers/openai_compat/mod.rs:426` (test), `src/generation/types/mod.rs:206` (test), `src/generation/mod.rs:781` (test) + the definition `src/generation/types/request.rs:77`.
  - Read sites that are consequently dead: `request.request_id` at `src/generation/providers/{google_veo/provider.rs:426, midjourney/provider.rs:174, openai_tts/mod.rs:350, bfl/mod.rs:398, fal/mod.rs:525, replicate/provider.rs:137, openai_whisper/mod.rs:329, deepgram_stt/mod.rs:287, minimax_tts/mod.rs:399, suno/mod.rs:390, elevenlabs/mod.rs:384, cartesia/mod.rs:343, openai_image.rs:358, volcengine_tts/mod.rs:385, deepgram_tts/mod.rs:367, openai_compat/edit.rs:413, openai_compat/generate.rs:44, google_imagen.rs:440, azure_speech/mod.rs:329}`; `request.user_id` at `src/generation/providers/openai_image.rs:215`, `src/generation/providers/openai_compat/helpers.rs:105`, `src/generation/providers/openai_compat/edit.rs:194`, `:301`.
  - `rg -n --glob '*.rs' 'output\.request_id' .` → only `src/generation/types/mod.rs:336` (test).
- **Symptom**: `GenerationOutput.request_id` is always `None` in production, so provider request IDs (Midjourney task IDs, Replicate prediction IDs, FAL queue IDs) never reach the caller — no correlation between a generation and the vendor job. `user` is never sent to OpenAI-compatible endpoints.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: medium

### SW-S1A-GEN-REGISTRY-TYPES-08 — `GenerationRequest::transcription()` has zero callers → transcription providers unreachable
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/types/request.rs:103` — `pub fn transcription<S: Into<String>>(audio_path: S) -> Self` (the only constructor for `GenerationType::Transcription`).
- **Consumer**: none found.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'GenerationRequest::transcription\(' .` → **1 hit**: `src/generation/providers/openai_whisper/mod.rs:127`, a doc comment (`///    `GenerationRequest::transcription(path)`).)`. No call site at all.
  - The two transcription providers are registered and reachable by type: `src/generation/providers/factory.rs:145` (`"openai_whisper" | "whisper"`) and `:155` (`"deepgram_stt" | "deepgram"`); `src/generation/providers/openai_whisper/mod.rs:341` `vec![GenerationType::Transcription]`; `src/generation/providers/deepgram_stt/mod.rs:129`.
  - No builtin tool exists for transcription: `rg -l 'GenerationRequest' src/builtin_tools/` → only `generation/{image,speech,video,audio}_generate.rs`, and `src/builtin_tools/generation/` has no transcription tool.
  - The inbound voice STT path is a separate implementation that bypasses the registry entirely: `src/gateway/voice/inbound/stt.rs:1-7` ("the HTTP core every transcription path shares … No provider selection here (see `super::provider`)") — it uses its own `reqwest` client (`:37`), not `GenerationProviderRegistry`.
- **Symptom**: The Settings UI lists Whisper / Deepgram-STT providers (they are in the registry and in `providers_for_type(GenerationType::Transcription)`), the user configures an API key, but no transcription can ever be issued through them; the actual STT path silently uses a different, config-driven endpoint.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: high

### SW-S1A-GEN-REGISTRY-TYPES-09 — 12 of 20 `GenerationParams` fields have no production producer
- **Form**: 1 (no producer — the consumer side exists, the wire's other end does not)
- **Producer**: `src/generation/types/params.rs:37-76` — fields `n` (:37), `seed` (:39), `duration_seconds` (:45), `fps` (:47), `language` (:55), `model` (:59), `negative_prompt` (:61), `guidance_scale` (:63), `steps` (:65), `reference_image` (:69), `reference_audio` (:71), `extra` (:76).
- **Consumer**: providers read them (e.g. `params.seed` at `stability.rs`, `openai_compat/helpers.rs`, `google_veo/provider.rs`, `fal/mod.rs`, `replicate/input.rs`; `params.steps` at `stability.rs`, `fal/mod.rs`, `replicate/input.rs`; `params.guidance_scale` / `params.negative_prompt` at `stability.rs`, `fal/mod.rs`, `replicate/input.rs`, `google_veo/provider.rs`; `params.fps` / `params.duration_seconds` at `fal/mod.rs`, `replicate/input.rs`, `openai_compat/helpers.rs`; `params.model` at `fal/mod.rs`, `deepgram_tts/mod.rs`; `params.language` at `openai_whisper/mod.rs`; `params.reference_audio` at `openai_whisper/mod.rs`, `deepgram_stt/mod.rs`, `replicate/input.rs`; `params.extra` at `openai_compat/edit.rs`, `minimax_tts/mod.rs`, `elevenlabs/mod.rs`, `volcengine_tts/mod.rs`, `fal/mod.rs`, `replicate/input.rs`) — but **nothing in production ever sets them**.
- **Live caller search**: the only production constructions of a `GenerationParams` are
  - `src/builtin_tools/generation/image_generate.rs:138-143` → `{ width, height, quality, style }`;
  - `src/builtin_tools/generation/speech_generate.rs:205-210` → `{ voice, speed, format }`;
  - `src/builtin_tools/generation/video_generate.rs:96-97` → `request.params.aspect_ratio = args.aspect_ratio`;
  - `src/gateway/voice/outbound.rs:249-252` → `params.voice = Some(...)`.
  - `rg -n --glob '*.rs' 'GenerationParams\s*\{|GenerationParams::default\(\)|GenerationParams::builder\(\)' src/ interfaces/ shared/ crates/` → every other hit is a `#[cfg(test)]` module (`google_veo/mod.rs:189+`, `fal/mod.rs:643+`, `replicate/mod.rs:150+`, `stability.rs:695+`, `src/generation/types/mod.rs:80+`, `src/generation/mod.rs:770+`) or a doc example.
  - `GenerationParams` is never deserialized from the wire: no gateway handler struct contains it (`rg -l 'GenerationParams' src/ interfaces/ shared/ crates/` → no `src/gateway/**` file).
- **Symptom**: Stability's seed/steps/guidance/negative-prompt controls, FAL's per-model `model` selector and fps, Replicate's duration/fps, Whisper's language and Deepgram's reference audio are all permanently inert — the provider code paths exist and compile, but the values can never arrive.
- **Hypothesis**: CUT (the missing end is the producer; see also SW-18 — the config `defaults` that were meant to feed these fields are never applied)
- **Confidence**: high
- **Severity**: high

### SW-S1A-GEN-REGISTRY-TYPES-10 — `GenerationParamsBuilder` / `new` / `merge` / `merged_with` are test-only
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/types/params.rs:82` (`new`), `:99` (`builder`), `:128` (`merge`), `:206` (`merged_with`), `:217` (`pub struct GenerationParamsBuilder` with 13 setters + `build`).
- **Consumer**: none found.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'GenerationParams::builder\(\)' src/ interfaces/ shared/ crates/` → 30 hits, all inside `#[cfg(test)] mod tests` (`google_veo/mod.rs:189+` — `mod tests` at `:57`; `fal/mod.rs:643+` — `mod tests` at `:569`; `replicate/mod.rs:150+` — `mod tests` at `:64`; `stability.rs:695+` — `mod tests` at `:551`; `src/generation/types/mod.rs:80+`; `src/generation/mod.rs:770+`) or doc examples.
  - `rg -n --glob '*.rs' 'GenerationParams::new\(\)' src/ interfaces/ shared/ crates/` → **0 hits** (the constructor is referenced by nobody at all).
  - `rg -n --glob '*.rs' 'merged_with\(' src/ interfaces/ shared/ crates/` → `src/generation/types/params.rs:200` (doc) and `src/generation/types/mod.rs:119`/`:130` (test only).
  - `rg -n --glob '*.rs' 'params\.merge\(|params_mut\.merge\(' src/ interfaces/ shared/ crates/` → 0 hits.
- **Symptom**: none user-visible directly — but the builder/merge machinery is the natural place where the config `defaults` would have been folded onto a request (SW-18), and it is unused, so nothing fills the gap.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: low

### SW-S1A-GEN-REGISTRY-TYPES-11 — `GenerationData` accessors are test-only
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/types/output.rs:43` (`is_bytes`), `:47` (`is_url`), `:51` (`is_local_path`), `:62` (`as_url`), `:71` (`as_bytes`), `:80` (`as_local_path`).
- **Consumer**: none found.
- **Live caller search**: `rg -n --glob '*.rs' '\.as_url\(\)' src/ interfaces/ shared/ crates/` → `src/generation/mod.rs:589,595,601,607` (tests) + `src/generation/types/mod.rs:223,233` (tests); `rg -n --glob '*.rs' '\.as_local_path\(\)' …` → `src/generation/types/mod.rs:244` (test); `rg -n --glob '*.rs' '\.is_bytes\(\)' …` → `src/generation/types/mod.rs:219,230,241` (tests); `.is_url()` → `src/generation/types/mod.rs:220,231,242,301` + `src/generation/registry.rs:790` (test). Production code matches the enum directly (`match &output.data { GenerationData::Url(u) => … }` at `image_generate.rs:153`, `video_generate.rs:103`, `audio_generate.rs:89`, `speech_generate.rs:229`).
- **Symptom**: none user-visible (the same information is obtained by pattern matching) — the accessors are dead API surface.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: low

### SW-S1A-GEN-REGISTRY-TYPES-12 — `GenerationType::supports_style/supports_voice/is_long_running` are test-only
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/types/generation_type.rs:39` (`supports_style`), `:49` (`supports_voice`), `:59` (`is_long_running`).
- **Consumer**: none found.
- **Live caller search**: `rg -n --glob '*.rs' '\.supports_style\(\)' src/ interfaces/ shared/ crates/` → `src/generation/types/mod.rs:37-40` (test) + `src/generation/types/generation_type.rs:11` (doc); `rg -n --glob '*.rs' '\.supports_voice\(\)' …` → `src/generation/types/mod.rs:45-48` (test) + doc `:12`; `rg -n --glob '*.rs' '\.is_long_running\(\)' …` → `src/generation/types/mod.rs:53-56` (test) only.
- **Symptom**: none user-visible. Note the contrast with the one method that *is* used — `supports()` (`src/generation/mod.rs:195`) is called by every builtin tool (`image_generate.rs:119`, `speech_generate.rs`, `video_generate.rs:80`, `audio_generate.rs`), and `is_long_running()` — the predicate that would decide to poll asynchronously — is never consulted.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: low

### SW-S1A-GEN-REGISTRY-TYPES-13 — 5 `GenerationProviderRegistry` methods have no production consumer
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/registry.rs` — `contains` (:263), `len` (:269), `is_empty` (:275), `providers_for_type` (:304), `has_provider_for_type` (:322), `names_for_type` (:351), `remove` (:389), `clear` (:427), `iter` (:437).
- **Consumer**:
  - `len` → `src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:87` ✔
  - `is_empty` → `generation_init.rs:88` ✔
  - `providers_for_type` → `src/tools/probes/generation.rs:51`, `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:134` ✔
  - `names_for_type` → `src/gateway/voice/outbound.rs:128` ✔
  - `first_for_type` → `src/executor/builtin_registry/builder/optional_tools.rs:300,313,325,337`, `src/builtin_tools/generation/{image_generate.rs:127,speech_generate.rs:193,video_generate.rs:87,audio_generate.rs:76}`, `src/gateway/voice/outbound.rs:124` ✔
  - `get_voices_for_provider` → `src/builtin_tools/generation/speech_generate.rs:197` ✔
  - `get` → `src/builtin_tools/generation/{image_generate.rs:114,speech_generate.rs:179,video_generate.rs:77,audio_generate.rs:66}`, `src/gateway/handlers/voice.rs:374` ✔
  - `names` → `src/gateway/handlers/voice.rs:372`, `src/bin/aleph-server/commands/start/builder/subsystems.rs:1288` ✔
  - `register` → `src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:76` (boot) and `:134` (hot reload) ✔
  - **`contains` (:263), `has_provider_for_type` (:322), `remove` (:389), `clear` (:427), `iter` (:437) — no production consumer.**
- **Live caller search**:
  - `rg -n --glob '*.rs' '\.(has_provider_for_type)\(' src/ interfaces/ shared/ crates/` → **0 hits** (the only repo mention outside the definition is a prose note in `docs/superpowers/runs/2026-11-09-occams-generation-goal-groupchat/findings/GEN1.md:42`).
  - `rg -n --glob '*.rs' 'registry\.contains\(' src/ interfaces/` → the hits are a **different** registry (agent registry: `src/builtin_tools/agent_manage/*.rs`, `src/gateway/handlers/channel.rs:477,811`); no `GenerationProviderRegistry::contains` call exists.
  - `rg -n --glob '*.rs' 'providers\.remove\(|providers\.clear\(|\.providers\.iter\(' src/ interfaces/ shared/ crates/` → hits are `src/providers/registry.rs` (LLM provider registry), `src/providers/model_catalog/**`, `src/memory/**` — never `GenerationProviderRegistry`.
  - `GenerationProviderRegistry::{remove,clear,iter,contains}` are exercised only by `#[cfg(test)] mod tests` in `src/generation/registry.rs` (`mod tests` starts at `:483`; call sites `:604,605,604,710,721,722,728,729,752,707,724,736,770,806-808,826`).
- **Symptom**: The registry cannot answer "does a provider of type X exist?" without materialising the whole list, and a provider cannot be removed at runtime — the registry is append-only and rebuilt wholesale on config reload (`generation_init.rs:134`). Not user-visible today, but the removal path is the one a "delete provider" RPC would need.
- **Hypothesis**: DECIDE — either these are deliberate API surface for a future runtime-edit path, or they are leftovers from a removed feature. The `has_provider_for_type` helper in particular has a name that reads like an intended capability check.
- **Confidence**: high
- **Severity**: medium

### SW-S1A-GEN-REGISTRY-TYPES-14 — `CATALOG_GENERATED_AT` is documented as surfaced but has no reader
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/voice_catalog.rs:32` — `pub const CATALOG_GENERATED_AT: &str = "2026-08-27";`, doc at `:26-31`: *"Surfaced in the Settings RPC payload so the UI can show a 'catalog as of …' hint; bump this constant whenever a `static_voice_list()` is updated."*
- **Consumer**: none found.
- **Live caller search**:
  - `rg -n 'CATALOG_GENERATED_AT' .` → **2 hits**: the definition `src/generation/voice_catalog.rs:32`, and a prose artifact `./review-results/2026-08-31-fetch-gateway-generation-goal/generation.json:94` ("voice_catalog CATALOG_GENERATED_AT may drift"). No code reads it.
  - It is not even re-exported: `src/generation/mod.rs:50` re-exports only `static_voices_for_provider_type` from `voice_catalog`.
  - The voices RPC returns a bare list: `src/gateway/handlers/generation_providers/voices.rs:94` `JsonRpcResponse::success(request.id, serde_json::to_value(voices).unwrap_or_default())` — `voices: Vec<VoiceInfo>`, no timestamp field.
- **Symptom**: The UI cannot show the "catalog as of …" hint; a stale voice list is indistinguishable from a current one. The doc comment states a behaviour that does not exist.
- **Hypothesis**: CUT (the RPC payload was never extended with the field)
- **Confidence**: high
- **Severity**: low

### SW-S1A-GEN-REGISTRY-TYPES-15 — The probe cannot resolve a URL for the `google` / `google_veo` presets
- **Form**: 3 (inert config — the built-in default endpoint is never read)
- **Producer**: `src/generation/providers/google_imagen.rs:50` — `const DEFAULT_ENDPOINT: &str = "https://generativelanguage.googleapis.com";` (used at `:161`), and `src/generation/providers/google_veo/provider.rs:116` (same default). These are the providers' own fallback when `base_url` is `None`.
- **Consumer**: `src/generation/probe.rs:68-78` — `probe_url()` resolves a URL from **only** two sources: the caller-supplied `base_url`, or `get_preset_by_type(provider_type).and_then(|p| p.base_url)`. It never consults the provider's `DEFAULT_ENDPOINT`.
- **Live caller search**:
  - The presets: `src/config/types/generation/presets/registry.rs:50` — `GenerationPreset::new("google", "imagen-3.0-generate-002", None)` and `:145` — `GenerationPreset::new("google_veo", "veo-2.0-generate-001", None)`. `None` is the third argument (`base_url`).
  - The Panel sends exactly those values: `interfaces/webchat/src/platform/wide/views/settings/generation_providers/preset_setup.rs:22` `RwSignal::new(preset.base_url.clone().unwrap_or_default())` → `""` → `base_url: None` (`:55-60`); `:30` `provider_type = preset.provider_type` = `"google"` / `"google_veo"`.
  - `src/generation/probe.rs:147-152` → `probe_url` returns `None` → `GenerationProbeOutcome { success: false, message: "未配置 base_url，无法测试连接" }`.
  - `src/config/types/generation/presets/mod.rs:129-144` — `get_preset_by_type("google")` returns the google-imagen preset, whose `base_url` is `None`, so `.and_then(|p| p.base_url)` is `None` → same result.
  - Consequence chain confirmed: `src/gateway/handlers/generation_providers/handlers.rs:737` `p.verified = outcome.success;` → `src/gateway/handlers/generation_providers/wire.rs:104` `verified: c.verified` → `interfaces/webchat/src/platform/wide/views/settings/generation_providers/detail_view.rs:614` `disabled=move || setting_default.get() || is_default || !config_verified` — the "set as default" button stays **disabled** for a correctly configured Google Imagen / Google Veo provider, and `mod.rs:368` `RowDot::Verified` never lights up.
- **Symptom**: The user enters a valid Google AI Studio key for Imagen or Veo, clicks "Test connection", and always gets "未配置 base_url，无法测试连接". The provider is permanently marked unverified and cannot be made the default.
- **Hypothesis**: CUT — the probe's URL-resolution table is missing the provider default-endpoint table that the providers themselves carry.
- **Confidence**: high
- **Severity**: high

### SW-S1A-GEN-REGISTRY-TYPES-16 — `NO_MODELS_ENDPOINT` misses factory aliases → false "连接成功"
- **Form**: 5 (name drift between the probe's alias table and the factory's alias table)
- **Producer**: `src/generation/probe.rs:98-107` — `const NO_MODELS_ENDPOINT: &[&str] = &["google_veo","veo","elevenlabs","bfl","bfl_flux","flux","suno","midjourney"];`, with the doc at `:83-89` explaining that a 404 for those types must **not** be read as success.
- **Consumer**: `src/generation/probe.rs:108` `if matches!(status, Some(404)) && NO_MODELS_ENDPOINT.contains(&normalized.as_str())`.
- **Live caller search**:
  - Factory aliases not covered by the probe list: `src/generation/providers/factory.rs:273` `"google" | "google_imagen" | "imagen"` and `:362` `"midjourney" | "mj"`. The probe list has `"midjourney"` but **not** `"mj"`, and has no `"google"`/`"imagen"`/`"google_imagen"` entry.
  - The Panel's "add custom" form takes `provider_type` as **free text**: `interfaces/webchat/src/platform/wide/views/settings/generation_providers/add_custom.rs:202-204` (`on:input` → `provider_type.set(...)`), so a user may legitimately type `mj` or `imagen` (both accepted by the factory).
  - `rg -n --glob '*.rs' 'NO_MODELS_ENDPOINT' .` → only `src/generation/probe.rs` (definition + use).
  - For `"mj"`: `probe_url(None,"mj")` → `get_preset_by_type("mj")` → `None` (the preset is keyed `"midjourney"`, `src/config/types/generation/presets/registry.rs`), so a `base_url` must be supplied; the probe then GETs `{base}/v1/models` → 404 → `"mj" ∉ NO_MODELS_ENDPOINT` → `interpret` returns `success = true, "连接成功 (HTTP 404)"` → `handlers.rs:737` persists `verified = true`.
- **Symptom**: A Midjourney provider configured with the `mj` alias and a **wrong** API key is stamped "verified" (green badge) — exactly the failure the probe's own doc comment says it fixed for the `midjourney` spelling. The same holds for `imagen` / `google_imagen` with a custom base URL (Google has no `/v1/models`, only `/v1beta/models`).
- **Hypothesis**: CUT — the alias list was fixed for one spelling but not for the aliases the factory also accepts.
- **Confidence**: high
- **Severity**: medium

### SW-S1A-GEN-REGISTRY-TYPES-17 — The `openai_compat` presets make the probe report a false "连接成功"
- **Form**: 5 (path/semantic drift: `base_url` means "full operation endpoint" to the provider but "base" to the probe)
- **Producer**: `src/config/types/generation/presets/registry.rs` — 6 presets with `provider_type = "openai_compat"` whose `base_url` is a **full POST endpoint**: `:61` `https://api.ideogram.ai/openai/v1/images/generations`, `:74` `https://external.api.recraft.ai/v1/images/generations`, `:87` `https://ark.cn-beijing.volces.com/api/v3/images/generations`, `:100` `https://api.siliconflow.cn/v1/images/generations`, `:113` `https://api.x.ai/v1/images/generations`, and one more in the same block.
- **Consumer**: `src/generation/probe.rs:74-77` — `match resolve_base_url(raw) { ResolvedUrl::Standard(base) => Some(format!("{base}/v1/models")), ResolvedUrl::Custom(url) => Some(url) }`. `resolve_base_url` (`src/generation/providers/url_normalize.rs:62-75`) classifies any URL with a path beyond `/v1` as `Custom` → the probe GETs the **image-generation endpoint** itself.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'GenerationPreset::new\(' src/config/types/generation/presets/registry.rs` → the 6 `openai_compat` entries at `:58, :71, :84, :97, :110, …`.
  - `src/generation/providers/url_normalize.rs:57-60` documents the rule: *"Domain-only (no path after scheme) → Standard; Domain + /v1 → Standard; Anything else → Custom (use as-is)"*.
  - `src/generation/probe.rs:161-169` → `client.get(&url)` — a **GET** on a POST-only endpoint.
  - `src/generation/probe.rs:121-129` → `match status { Some(401|403) => failure, Some(s) => success }` — every other status is a success.
- **Symptom**: Testing an Ideogram / Recraft / Seedream / SiliconFlow / xAI provider with a **wrong** API key returns "连接成功 (HTTP 405)" (or 404), `verified` is persisted as `true`, the Panel shows a green badge and unlocks "set as default" — the key is never actually exercised.
- **Hypothesis**: CUT (the probe's URL derivation has no arm for a custom OpenAI-compatible endpoint; it needs a real generation-free probe such as `GET {base}/v1/models` derived from the base, not the operation URL)
- **Confidence**: high (the exact status code returned by each vendor is not verified here — but every plausible status other than 401/403 yields "success", and a wrong key on these vendors does not return 401/403 on a GET of a POST-only path; what would settle the residual doubt is one live probe per vendor)
- **Severity**: high

### SW-S1A-GEN-REGISTRY-TYPES-18 — `GenerationDefaults` are inert for 13 of 14 fields
- **Form**: 3 (inert config)
- **Producer**: `src/config/types/generation/defaults.rs:30-83` — `width`, `height`, `aspect_ratio`, `quality`, `style`, `n`, `format`, `duration_seconds`, `fps`, `voice`, `speed`, `language`, `guidance_scale`, `steps`; doc at `:14-16`: *"These defaults are applied to generation requests when the corresponding parameter is not explicitly specified."* Per-provider config section `[generation.providers.<name>.defaults]` (`src/config/types/generation/provider.rs:84`, validated at `:217-218`).
- **Consumer**: none for 13 of the 14 fields. The **only** production read of a `defaults` field is `.voice`: `src/generation/providers/factory.rs:139`, `:209`, `:219`, `:229`, `:331`.
- **Live caller search**:
  - `rg -n --glob '*.rs' '\bdefaults\b' src/config/types/generation/ src/gateway/handlers/generation_providers/ src/builtin_tools/generation/ src/generation/` → the production hits are the five `config.defaults.voice.clone()` calls in `factory.rs`; everything else is `defaults_to_wire` / `defaults_from_wire` (`src/gateway/handlers/generation_providers/wire.rs` ≈`:48-83`) — i.e. the values are shipped to the Panel and written back, never applied to a request.
  - `rg -n --glob '*.rs' 'GenerationParams\s*\{' src/builtin_tools/generation/` → `image_generate.rs:138` sets `{width, height, quality, style}` only; `speech_generate.rs:205` sets `{voice, speed, format}` only; `video_generate.rs:97` sets `aspect_ratio` only. No tool reads `config.defaults`.
  - No TS/TSX file touches generation defaults: `rg -n 'defaults' --glob '*.tsx' --glob '*.ts' interfaces/ | rg -i 'generat|provider|width|height|aspect'` → 0 hits.
  - `GenerationParams::merge` / `merged_with` — the mechanism that would fold defaults onto a request — have no production caller (see SW-10).
- **Symptom**: The Settings UI lets the operator set a default width/height/aspect-ratio/quality/style/n/format/duration/fps/speed/language/guidance/steps per provider; the value is stored, displayed, and exported to the wire, but the next generation ignores it entirely. Only the default **voice** is honoured.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: high

### SW-S1A-GEN-REGISTRY-TYPES-19 — `static_voices_for_provider_type("openai")` returns TTS voices for an image provider
- **Form**: 5 (name drift between the catalog's alias table and the factory's alias table)
- **Producer**: `src/generation/voice_catalog.rs:45` — the arm `"openai" | "openai_tts" | "openai-tts" | "tts" | "openai_compat" | "openai-compat"` returns `OpenAiTtsProvider::static_voice_list()`. The module doc at `:34-36` claims *"using the same aliases the provider factory accepts (`create_provider`) so a type that constructs a speech provider always resolves a catalog here too."*
- **Consumer**: `src/gateway/handlers/generation_providers/voices.rs:199` — `static_voices_for_provider_type(provider_type)` inside `detect_voices_by_model` (`:163`), the last fallback of `handle_voices` (`:17`), exposed as RPC `generation_providers.voices` (`src/bin/aleph-server/commands/start/builder/handlers/settings.rs:665-670`).
- **Live caller search**:
  - `src/generation/providers/factory.rs:124` — `"openai" | "openai_image" | "dalle" => OpenAiImageProvider::new(...)` — i.e. `provider_type = "openai"` constructs the **image** provider, whose `supported_types()` is image-only.
  - The catalog's other two arms `"openai-tts"` / `"openai-compat"` (kebab) are not factory aliases at all (`factory.rs` uses `"openai_tts"`, `"openai_compat"`).
  - The Panel only asks for voices when the row is a speech provider: `interfaces/webchat/src/platform/wide/views/settings/generation_providers/detail_view.rs:64` `if is_speech { … fetch_voices … }`.
- **Symptom**: A provider configured as `provider_type = "openai"` (image) shows the OpenAI-TTS voice list if the row is ever treated as a speech provider; conversely a config whose `provider_type` is spelled `"openai-tts"` gets a catalog although the factory would reject that spelling (`factory.rs` has no `"openai-tts"` arm → provider construction fails). The kebab arms are unreachable aliases.
- **Hypothesis**: DECIDE — the alias table needs to be derived from the factory's speech arms rather than hand-listed, otherwise it will keep drifting.
- **Confidence**: medium (the user-visible impact depends on how `effective_modality` is derived server-side for a row whose `capabilities` were not set explicitly — the Panel's `is_speech` gate is derived from the server-written modality, which I traced only as far as `src/gateway/handlers/generation_providers/wire.rs:97-101` / `helpers.rs:74-89`)
- **Severity**: low

---

## B. Clean / verified symbols (no severed wire)

- `GenerationProviderRegistry::register` — producer `src/bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:76` (boot) and `:134` (hot reload); consumer is the registry itself. Duplicate registration is rejected with `GenerationError::DuplicateProvider` (`registry.rs:102-…`) — the error is handled, not swallowed.
- `GenerationProviderRegistry::get` — consumed by `src/builtin_tools/generation/image_generate.rs:114`, `speech_generate.rs:179`, `video_generate.rs:77`, `audio_generate.rs:66`, `src/gateway/handlers/voice.rs:374`. The canonical-name fallback (`registry.rs:171-181`) is symmetric with `contains` (`registry.rs:263`).
- `GenerationProviderRegistry::{names, len, is_empty, providers_for_type, names_for_type, first_for_type, get_voices_for_provider}` — all have production consumers (see SW-13 table).
- `GenerationProvider::list_voices` (`src/generation/mod.rs:354`) → `registry.rs:452` (`get_voices_for_provider`) → `src/builtin_tools/generation/speech_generate.rs:197` → the voice picker. Fully wired.
- `probe_generation_provider` (`src/generation/probe.rs:139`) is reachable from a real RPC: `src/gateway/handlers/generation_providers/handlers.rs:710` in `handle_test_connection` (`:658`), registered as `generation_providers.test` (`src/bin/aleph-server/commands/start/builder/handlers/settings.rs:658-664`); the Panel client `interfaces/webchat/src/api/generation_providers.rs:136` (`test_connection`) is called from `preset_setup.rs:103`, `add_custom.rs:103`, `detail_view.rs:211`. The `verified` flag round-trips: `handlers.rs:737` → config → `wire.rs:104`/`132` → `interfaces/webchat/.../detail_view.rs:96,158,321,614`, `mod.rs:357,368,499,515`. **No severed wire in the probe↔verified path** (its *content* problems are SW-15/16/17).
- `static_voices_for_provider_type` (`src/generation/voice_catalog.rs:42`) → `src/gateway/handlers/generation_providers/voices.rs:199` → RPC `generation_providers.voices` → `interfaces/webchat/src/api/generation_providers.rs:153` → `detail_view.rs:68`. Wired end to end; the catalog's coverage of every factory speech alias (`openai_tts`, `tts`, `deepgram_tts`, `azure_speech`, `azure_tts`, `cartesia`, `minimax_tts`, `volcengine_tts`, `openai_compat`, `elevenlabs`) was checked against `src/generation/providers/factory.rs:134,164,173,204,214,224,234,326` — none is missing.
- `GenerationParams` fields that **do** have both ends: `width`, `height`, `quality`, `style` (`image_generate.rs:139-142`), `aspect_ratio` (`video_generate.rs:97`), `voice` (`speech_generate.rs:206`, `outbound.rs:252`), `speed` (`speech_generate.rs:207`), `format` (`speech_generate.rs:208`) — each is read by ≥1 provider.
- `GenerationMetadata` fields with a production reader: `model` (`image_generate.rs:202`, `video_generate.rs:131`, `audio_generate.rs:117`), `revised_prompt` (`image_generate.rs:200`), `content_type` (`image_generate.rs:158`), `size_bytes` (`speech_generate.rs:231,234`).
- `GenerationOutput.data` — read by all four builtin tools (`image_generate.rs:153`, `video_generate.rs:103`, `audio_generate.rs:89`, `speech_generate.rs:229`).
- `GenerationRequest::image/video/audio/speech` — production constructors (`image_generate.rs:146`, `video_generate.rs:96`, `audio_generate.rs:84`, `speech_generate.rs:213`). (`transcription` is SW-08.)
- `GenerationType::display_name` / `Display` (`generation_type.rs:65,76-79`) — used in production error messages (e.g. `google_veo/provider.rs:420`, `stability.rs:383`).
- `GenerationProvider::supports` (`src/generation/mod.rs:195`) — called by every builtin tool.

## C. New observations (not findings)

1. `GenerationDefaults` is **not** read by the voice/TTS path either: `src/gateway/voice/outbound.rs:249-252` sets `params.voice` from the *candidate* voice, overriding the config default voice that `factory.rs:139` already applied to the provider — so the config default voice is applied at provider construction, not per request. Consistent, but worth knowing when reading SW-18.
2. `src/generation/providers/openai_compat/edit.rs:261` documents that the edit path takes its mask through `params.extra["mask"]`; since `extra` has no production writer (SW-09) and `edit_image` has no production caller (SW-03), that whole path is doubly dark.
3. `src/generation/providers/deepgram_stt/types.rs:11` carries `#[allow(dead_code)] // surfaced via JSON dump in metadata.extra` — the comment points at the same `metadata.extra` channel that has no reader (SW-05).
4. The `NO_MODELS_ENDPOINT` list is a *literal* in `probe.rs` while the factory's alias table is a `match` in `factory.rs` — the same fact is written twice, which is how SW-16 arose.
5. `get_preset_by_type` (`src/config/types/generation/presets/mod.rs:129-144`) returns `None` whenever two presets share a `provider_type` with different base URLs — that happens for `openai_compat` (6 presets) and for `openai_tts` / `openai_whisper` (OpenAI + SiliconFlow). The probe therefore cannot resolve a base URL for those three provider types either (the preset flow still works because the Panel sends the explicit `base_url`).
6. `src/generation/providers/factory.rs:87` handles `provider_type == "local"` before the api-key gate; the probe's `auth_header_for("local", …)` falls through to `Authorization: Bearer` and `get_preset_by_type("local")` is `None`, so a local BYO provider can only be probed if the user supplies a `base_url`. Not a defect on its own (the BYO server is usually unauthenticated), but the probe result for it is not meaningful.
7. `src/gateway/handlers/generation_providers/voices.rs:94` returns a bare `Vec<VoiceInfo>`; there is no envelope, so adding the `CATALOG_GENERATED_AT` hint (SW-14) would require a wire-shape change, not just a field addition.

## D. Coverage statement

**Files read in full (slice)**: `src/generation/registry.rs`, `src/generation/probe.rs`, `src/generation/voice_catalog.rs`, `src/generation/types/mod.rs`, `src/generation/types/output.rs`, `src/generation/types/params.rs`, `src/generation/types/progress.rs`, `src/generation/types/request.rs`, `src/generation/types/generation_type.rs`.

**Files read partially (adjacent, for the consumer side)**: `src/generation/mod.rs` (trait surface), `src/generation/providers/factory.rs` (alias table), `src/generation/providers/url_normalize.rs`, `src/generation/providers/google_imagen.rs`, `google_veo/provider.rs`, `openai_tts/mod.rs`, `openai_whisper/mod.rs`, `deepgram_stt/mod.rs`, `deepgram_tts/mod.rs`, `azure_speech/mod.rs`, `openai_compat/{generate.rs,edit.rs,helpers.rs,mod.rs}`, `src/gateway/handlers/generation_providers/{handlers.rs,voices.rs,wire.rs,helpers.rs}`, `src/config/types/generation/{defaults.rs,provider.rs}`, `src/config/types/generation/presets/{mod.rs,registry.rs}`, `src/builtin_tools/generation/{image,speech,video,audio}_generate.rs`, `src/gateway/voice/{outbound.rs,inbound/stt.rs}`, `interfaces/webchat/src/api/generation_providers.rs`, `interfaces/webchat/src/platform/wide/views/settings/generation_providers/{preset_setup.rs,add_custom.rs,detail_view.rs,mod.rs,picker.rs}`.

**Lenses applied**: registration parity (registry `register`/`names` vs the config-driven boot and reload path); call-vs-handler parity (`generation_providers.{list,get,create,update,delete,set_default,test,voices,list_presets}` — all nine registered at `src/bin/aleph-server/commands/start/builder/handlers/settings.rs:658-676` and all nine have Panel callers); classifier-vs-handler parity (the probe's `NO_MODELS_ENDPOINT` classifier vs the factory's handler table — SW-16; the voice catalog's alias table vs the factory's speech arms — SW-19); event emit-vs-subscribe parity (no event bus is involved in this slice; the closest analogue is the `verified` flag, which is a config write/read, not an event — checked and clean); config-reader parity (`GenerationDefaults` — SW-18; `generation.verified` — clean; `CATALOG_GENERATED_AT` — SW-14; `GenerationProviderConfig.defaults/base_url/voices_url` — `voices_url` read at `voices.rs:62`, `base_url` read at `voices.rs:64`, both wired); path/route parity (the 9 generation RPCs and the Panel's `interfaces/webchat/src/api/generation_providers.rs` wrapper — all 9 wrappers have production callers, no client ghosts); stub sweep (no `todo!()` / `unimplemented!()` in the slice; the stub-shaped items found are the `unsupported_feature` trait defaults at `src/generation/mod.rs:242-252` and `:269-279`, which are legitimate "not supported by this provider" errors — but they are the far end of a wire nobody calls, SW-01/SW-02).

**Seams not covered and why**:
1. **Non-Rust front-ends**: no `.ts`/`.tsx` file in the repo references the generation providers or `GenerationDefaults` (verified by grep), so the TSX Panel could not be a hidden consumer; the Rust/Leptos Panel (`interfaces/webchat`) was read where it matters. The `interfaces/panel` TSX app was not read line by line — grep found no generation-provider references in it.
2. **Live network behaviour**: I did not run any probe against a real vendor (no network calls were made). The residual uncertainty in SW-17 is the exact HTTP status each OpenAI-compatible vendor returns for `GET` on a POST-only path; the finding does not depend on the exact code because every status other than 401/403 is classified as success.
3. **`desktop/` and `crates/`**: `desktop/nsh-shell` and `desktop/{linux,windows}` were not swept for generation-provider consumers; grep across `desktop/` for `GenerationProviderRegistry` / `GenerationProgress` / `GenerationParams` returned no hits, so the seam is closed by grep but not by reading.
4. **The `voice_local` / BYO local path**: `src/config/types/voice_local.rs` and `src/gateway/voice/local_provider.rs` were read only where the generation registry meets them; the local provider's own voice handling was not audited (it is a voice-path, not a generation-registry concern).
5. **`src/gateway/voice/outbound.rs` full read**: only the registry-touching parts (`:113-137`, `:243-265`, `:502`) were read; the rest of the TTS candidate-hopping logic was not audited.
6. **Provider-internal request builders**: the per-provider request-body builders (`build_request_body` and friends) were read only where they consume `GenerationParams`; their response parsing was not audited.
7. **`graphify`**: used for orientation only; every claim above was confirmed with `rg` and, where a line was cited, by reading that line.
8. **`docs/` prose**: a doc mention of `has_provider_for_type` exists at `docs/superpowers/runs/2026-11-09-occams-generation-goal-groupchat/findings/GEN1.md:42`; I did not audit whether that document claims the method is wired.
