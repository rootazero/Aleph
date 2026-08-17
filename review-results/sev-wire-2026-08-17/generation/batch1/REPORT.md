# Severed-Wire Audit — `src/generation/providers/` (batch 1 of 2)

- **Audit**: severed-wire-audit (PRODUCED–CONSUMED symbol parity)
- **Date**: 2026-08-17
- **Module**: `src/generation/providers/` (60 files / 19 048 LOC) + 1 sibling in batch1 scope via the central factory (`src/generation/registry.rs`)
- **Tree**: `.worktrees/sev-wire-batch2`
- **Method**: `rg -n "<symbol>" src/ bin/ interfaces/ shared/`; read-all-files first; no cargo, no edits
- **Result**: 18 findings (0 critical, 0 high, 4 medium, 14 low). Decisions: 0× CUT, 0× CONNECT, 18× DECIDE.

---

## Wiring summary (verified, not severed)

The module's central spine is **externally connected on both entry paths**:

| Producer | Production consumers (path:line) |
|---|---|
| `GenerationProviderRegistry` (registry.rs:34) | `bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:50,102` (initial build + hot-reload); `tools/probes/generation.rs:77`; `builtin_tools/generation/{speech,image,video,audio}_generate.rs`; `gateway/voice/outbound.rs:477`; `bin/aleph-server/commands/start/builder/subsystems.rs:925` |
| `providers::create_provider` (factory.rs:76) | `bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:58,110` |
| `providers::url_normalize::{resolve_base_url, ResolvedUrl}` (url_normalize.rs) | factory.rs:9, `openai_image.rs:139,146`, `openai_tts.rs:140,144`, `openai_whisper.rs:96,101`, `probe.rs:69` |
| `providers::http::{voice_http_client, CONNECT_TIMEOUT_SECS}` (http.rs) | `openai_tts/mod.rs:149,173`; `azure_speech/mod.rs:113,130`; `gateway/voice/local_provider.rs:53` |
| `static_voices_for_provider_type` (voice_catalog.rs:34) | `gateway/handlers/generation_providers/voices.rs:6,199`; voice_catalog.rs own tests |
| `provider::static_voice_list` (per-speech-provider) | voice_catalog.rs:38-46, `gateway/handlers/generation_providers/voices.rs:242` (Minimax only) |
| `probe_generation_provider` (probe.rs:108) | re-exported at generation/mod.rs:49; **only tests reach the symbol** — see finding sw-generation-01 |

Each `pub mod <provider>;` in `providers/mod.rs` (17 provider subdirs + 4 inline providers) is reached by `factory.rs` via match-on-`provider_type`. Every one of the 19 dispatcher arms either constructs a provider struct or dispatches to a sub-builder. The factory is the canonical wire to every provider module.

---

## Findings

### sw-generation-01 — `parse_generation_requests` / `has_generation_requests` / `ParsedGenerationRequest` / `ParseResult` re-exported but only consumed by tests (form 4 + 6)

- **Severity**: medium
- **Form**: 4 (test-only consumer) + 6 (orphaned pub re-export)
- **Files**: `src/generation/response_parser.rs:16-37,63-96`; `src/generation/mod.rs:67`

The four symbols are documented as "lets the AI request media generation in `[GENERATE:type:provider:model:prompt]` form" and re-exported at `mod.rs:67` (next to `GenerationError`, `GenerationProviderRegistry`, etc., suggesting production consumers). **Zero hits outside `response_parser.rs` itself** across `src/`, `bin/`, `interfaces/`, `shared/` — confirmed via:

```
$ rg -n "parse_generation_requests|has_generation_requests|ParsedGenerationRequest|ParseResult" src/ interfaces/ shared/ --type rust | grep -v response_parser.rs | grep -v mod.rs
(no output)
```

The 5 test cases in `response_parser.rs:104-167` exercise every variant (single, multiple, Chinese prompt, no-match, cleaned-response) — the code is test-covered but no caller exists. There is no inbound tool or RPC that invokes this parser. The `response_parser.rs` module is reachable through `pub mod response_parser;` at `mod.rs:43`, so the dead API sits on the crate surface.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: drop `pub mod response_parser;` from `mod.rs:43`, delete `response_parser.rs`, remove the `pub use` at `mod.rs:67`. Removes ~170 LOC and the false promise that the AI can drive generation through this tag format.
  2. CONNECT: wire the parser into `run_loop/inner.rs` (or wherever AI replies are inspected) so the tag actually drives a `GenerationRequest` through the registry. ~30 LOC connector.
  3. Leave as-is (test-only documentation of an intent).
- **Risk of option 1**: low — no in-tree reader. External shells/plugins cannot reach `alephcore::generation::parse_generation_requests` today (the Webchat panel builds its own `GenerationProviderConfig` from form inputs at `interfaces/webchat/src/platform/wide/views/settings/generation_providers/add_custom.rs:44`).
- **Risk of option 2**: medium — the parser was scaffolded before the `builtin_tools/generation/*` tools existed; the right replacement is to make the AI use the proper tool-calling path (`speech_generate`, `image_generate`, …), not to bolt this legacy tag format back in.
- **Verification**: `rg -n "parse_generation_requests" src/ interfaces/ shared/ --type rust` returns only the 7 hits in `response_parser.rs` itself.

### sw-generation-02 — `OpenAiWhisperProvider` constructed by factory but never invoked in production (form 4 + 1)

- **Severity**: medium
- **Form**: 4 (test-only consumer) + 1 (zero production readers)
- **Files**: `src/generation/providers/openai_whisper/mod.rs:59,189`; `src/generation/providers/factory.rs:123`; `src/generation/providers/mod.rs:77`

`OpenAiWhisperProvider` is wired into the factory and re-exported pub at `providers/mod.rs:77`. **The actual transcription path bypasses it entirely**: `src/media/resolve.rs:81` constructs `crate::media::whisper::WhisperTranscription::new(...)` from `transcription_providers` config, and `src/executor/builtin_registry/builder/constructor/mod.rs:1426` (`resolve_transcription`) is what the `audio_transcribe` builtin tool uses. The `gateway/voice/inbound/provider.rs:172` test also constructs the same config for `openai_whisper`, but the resolved `SttSource::Static` is consumed by `media::whisper::WhisperTranscription` (lines 80-90 of resolve.rs), not by `OpenAiWhisperProvider`.

```
$ rg -n "OpenAiWhisperProvider" src/ interfaces/ shared/ --type rust
src/generation/providers/factory.rs:8:    OpenAiTtsProvider, OpenAiWhisperProvider, …
src/generation/providers/factory.rs:123:        "openai_whisper" | "whisper" => Arc::new(OpenAiWhisperProvider::new(…
src/generation/providers/openai_whisper/mod.rs:59:pub struct OpenAiWhisperProvider {
src/generation/providers/openai_whisper/tests.rs:12-97:  (8 tests, all internal)
src/generation/providers/mod.rs:77:pub use openai_whisper::OpenAiWhisperProvider;
```

Note the prior-generation review (`review-results/severed-wire-2026-08-17/media/REPORT.md` sw-me-1 line "is the whisper path superseded by `generation/providers/openai_whisper`? No.") explicitly concludes both providers serve distinct layers — but the "other layer" never had a real production caller in the first place. `factory.rs:301` advertises `openai_whisper | whisper` as a known type, but no entry ever routes through it.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: delete the `openai_whisper` subdir (422 LOC), drop the `factory.rs` arm and the `mod.rs` re-export. Update `factory.rs:301` error message + `config/types/generation/presets/registry.rs:451,473` (preset registry). Saves 422 LOC + ~100 LOC of tests + 14 preset entries.
  2. CONNECT: thread the registry into `media::resolve::transcription_service` so a configured `openai_whisper` GenerationProvider resolves the same way `WhisperTranscription` does today. Real wire-up but duplicates the existing `WhisperTranscription` client.
  3. Leave as-is (factory arm reachable but no caller; presets are dead knobs).
- **Risk of option 1**: low — the only "consumer" is the test that the factory arm works. No end-to-end path uses `OpenAiWhisperProvider::generate`.
- **Risk of option 2**: medium — duplicates `media::whisper::WhisperTranscription` (both POST to `/v1/audio/transcriptions`); this only makes sense if the goal is "transcription as a first-class GenerationType::Transcription provider".
- **Verification**: `rg -n "OpenAiWhisperProvider" src/ interfaces/ shared/ --type rust` → only `factory.rs`, `mod.rs`, `openai_whisper/{mod.rs,tests.rs}`. After CUT, remove `pub mod openai_whisper;` at `providers/mod.rs:52` and `pub use openai_whisper::…` at `:77`.

### sw-generation-03 — `DeepgramSttProvider` constructed by factory but never invoked in production (form 4 + 1)

- **Severity**: medium
- **Form**: 4 + 1 (same shape as sw-generation-02)
- **Files**: `src/generation/providers/deepgram_stt/mod.rs:45,117`; `src/generation/providers/factory.rs:129`; `src/generation/providers/mod.rs:65`

`DeepgramSttProvider` mirrors the `openai_whisper` situation: factory arm exists (`factory.rs:129`, `"deepgram_stt" | "deepgram"`), `pub use` at `providers/mod.rs:65`, **no production caller**. The active transcription path is still `media::whisper::WhisperTranscription`; the `audio_transcribe` tool goes through `MediaPipeline` (`executor/.../constructor/mod.rs:370-378`), which is built from `resolve_transcription` (`src/media/resolve.rs`), which uses `WhisperTranscription` for the cloud branch.

The Deepgram presets at `config/types/generation/presets/registry.rs:463` (`nova-3` model) are also unreachable through any path that doesn't end at `DeepgramSttProvider`.

```
$ rg -n "DeepgramSttProvider" src/ interfaces/ shared/ --type rust
src/generation/providers/factory.rs:5:    … DeepgramSttProvider, DeepgramTtsProvider, …
src/generation/providers/factory.rs:129:        "deepgram_stt" | "deepgram" => Arc::new(DeepgramSttProvider::new(…
src/generation/providers/mod.rs:65:pub use deepgram_stt::DeepgramSttProvider;
src/generation/providers/deepgram_stt/{mod.rs,tests.rs}:  (provider + 8 internal tests)
```

- **Decision**: DECIDE — same options as sw-generation-02, but Deepgram is a paid service so a real wire-up may be desirable. The factory arm + `deepgram_stt` preset registry entry are the only "consumers" today; both go dead if the provider is removed.
- **Options**:
  1. CUT: delete `deepgram_stt/` (380 LOC), drop `factory.rs` arm + `mod.rs` re-export + `config/types/generation/presets/registry.rs:463` preset.
  2. CONNECT: add a `Transcription` arm to `media::resolve::transcription_service` so configured Deepgram entries resolve through this provider rather than through Whisper (which today always sends to OpenAI Whisper regardless of the configured `provider_type`). This is the *correct* wire-up — current state lets a user configure Deepgram but silently sends their audio to Whisper.
  3. Leave as-is.
- **Risk of option 1**: low (no end-to-end caller). **Risk of leaving as-is: high** — the current behaviour is a silent misroute: a user who configures `[generation.transcription_providers.deepgram]` with `api_key` and `nova-3` model never reaches Deepgram; their audio goes to Whisper with no warning (verified: `resolve.rs:80-86` always picks `WhisperTranscription::new` regardless of `pcfg.provider_type`).
- **Verification**: `rg -n "DeepgramSttProvider" src/ interfaces/ shared/ --type rust` → only the 4 hits above. To detect the silent misroute without CUTting, add a unit test in `media/resolve.rs` that asserts a `deepgram_stt` config produces a `WhisperTranscription` (i.e. documents the misroute).

### sw-generation-04 — `GenerationProvider::edit_image` + multipart edit pipeline is inert in production (form 4)

- **Severity**: medium
- **Form**: 4 (test-only consumer)
- **Files**: `src/generation/mod.rs:324-336,346-348`; `src/generation/providers/openai_compat/edit.rs:1-451`; `src/generation/providers/openai_compat/generate.rs:200-208`

The trait declares `edit_image` (mod.rs:324) and `supports_image_editing` (mod.rs:346) as the image-to-image / inpainting entry point. Only `OpenAiCompatProvider` overrides both (`generate.rs:200-208`, `edit.rs:19`). The default impl returns `UnsupportedFeatureError`. **No builtin tool, RPC handler, or agent path invokes `provider.edit_image()` or `provider.supports_image_editing()` in production**:

```
$ rg -n "edit_image|supports_image_editing" src/ interfaces/ shared/ --type rust | grep -v test
src/generation/mod.rs:320:    ///     let output = provider.edit_image(request).await.unwrap();   (doc)
src/generation/mod.rs:346:    fn supports_image_editing(&self) -> bool { false }                   (default impl)
src/generation/providers/openai_compat/generate.rs:200:    fn supports_image_editing(&self) -> bool { true }
src/generation/providers/openai_compat/mod.rs:744,767:    let result = provider.edit_image(request).await;  (test)
src/generation/providers/openai_compat/edit.rs:3://! Contains the GenerationProvider::edit_image implementation.
```

The `builtin_tools/generation/image_generate.rs` tool only calls `provider.generate(request)` — never `edit_image`. The OpenAI-compat editor logic (~451 LOC in `edit.rs` covering multipart construction, data-URI handling, Volcengine Ark image-to-image variant) is reachable only through 2 unit tests in `openai_compat/mod.rs:744,767`.

The factory path even wires up an `edit_url` builder hook (`builder.rs:123`) and `factory.rs:196` forwards `config.edit_url` from the user — both feed into the inert trait method.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: delete `openai_compat/edit.rs` (451 LOC) + the trait method + the builder hook + `factory.rs:196`. Save ~520 LOC and remove the silent promise of an image-edit feature.
  2. CONNECT: add an `image_edit` builtin tool (`builtin_tools/generation/image_edit.rs`) that calls `provider.edit_image()` when `provider.supports_image_editing()` is true. ~80 LOC.
  3. Leave as-is.
- **Risk of option 1**: low (no caller). External plugin/shell callers cannot reach `provider.edit_image()` today because every `dyn GenerationProvider` registered in the central registry is only used through `provider.generate(request)` in builtin tools.
- **Risk of option 2**: low. The trait method + impl exist and are test-covered; adding the tool is a connector, not new logic.
- **Verification**: `rg -n "edit_image|supports_image_editing" src/ interfaces/ shared/ --type rust | grep -v test | grep -v providers/openai_compat` → no production consumer.

### sw-generation-05 — `GenerationProvider::check_progress` is only overridden by `FalProvider`; no production caller (form 4 + 1)

- **Severity**: low
- **Form**: 4 (test-only consumer) + 1 (zero production readers)
- **Files**: `src/generation/mod.rs:246-258`; `src/generation/providers/fal/mod.rs:501-520`

The trait declares `check_progress` for long-running async jobs. Default impl returns `UnsupportedFeatureError`. Only `FalProvider` overrides it (fal/mod.rs:501). **No production caller** — `rg -n "check_progress" src/ interfaces/ shared/ --type rust` returns only the trait definition, the FalProvider override, and 1 test in `mod.rs:712`.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: remove `check_progress` from the trait, remove `FalProvider::check_progress`. The async-job progress UI is a separate concern that should live at the gateway (where `has_terminal_delta`-style hooks already exist) rather than on the provider.
  2. CONNECT: thread the registry into `run_loop/inner.rs` so jobs expose progress.
  3. Leave as-is (only Fal cares; default impl no-ops).
- **Risk of option 1**: low. Risk of leaving as-is: low (FalProvider's override is benign).

### sw-generation-06 — `GenerationProvider::cancel` is default-only and never invoked (form 4 + 1)

- **Severity**: low
- **Form**: 4 + 1
- **Files**: `src/generation/mod.rs:268-279`

Trait method with default `UnsupportedFeatureError` return. **No provider overrides it** (`rg -n "fn cancel\(" src/generation/providers/ --type rust | grep -v test` returns nothing). **No production caller**. The async-poll workflow in `fal/`, `replicate/`, `suno/`, `bfl/`, `google_veo/` doesn't surface a cancel handle — each one polls in a loop until terminal or wall-clock timeout.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: remove `cancel` from the trait entirely. ~12 LOC + 1 test.
  2. Leave as-is.
- **Risk of option 1**: low.

### sw-generation-07 — `OpenAiCompatProvider::new` (simple constructor) is test-only; factory uses `::builder` (form 6)

- **Severity**: low
- **Form**: 6 (orphaned pub API surface)
- **Files**: `src/generation/providers/openai_compat/provider.rs:75-91`

`OpenAiCompatProvider::new(name, api_key, base_url, model)` is documented as a "simple constructor" but the real factory dispatches via `OpenAiCompatProvider::builder(...)` (factory.rs:185). Every `OpenAiCompatProvider::new` call in the workspace is in the openai_compat module's own test code (32 hits in `openai_compat/mod.rs`, lines 223-831). Same shape as `MidjourneyProvider::new` / `ElevenLabsProvider::new` / `OpenAiTtsProvider::new` / `OpenAiWhisperProvider::new` — those are wired in the factory, so they're justified. The `OpenAiCompatProvider::new` variant is the only one where the factory explicitly uses `::builder` instead, suggesting the convenience `::new` is a documentation/symmetry holdover.

- **Decision**: DECIDE.
- **Options**:
  1. CUT the convenience `::new` (provider.rs:75-91).
  2. Route factory.rs:201 through `::new` instead of `::builder` for symmetry.
  3. Leave as-is.
- **Risk of option 1/2**: low (semantically equivalent).

### sw-generation-08 — `OpenAiImageProvider::edits_url` is `pub` with no caller (form 6)

- **Severity**: low
- **Form**: 6
- **Files**: `src/generation/providers/openai_image.rs:165-168`

```
$ rg -n "edits_url" src/ interfaces/ shared/ --type rust
src/generation/providers/openai_image.rs:165:    pub fn edits_url(&self) -> Option<String> { … }
src/generation/providers/openai_compat/helpers.rs:44:    pub(crate) fn edits_url(&self) -> String { … }
src/generation/providers/openai_compat/edit.rs:158:    let url = provider.edits_url();
src/generation/providers/openai_compat/mod.rs:697-733:  (tests of openai_compat::edits_url)
```

The `OpenAiImageProvider::edits_url` is `pub` but no production reader exists; `openai_compat::helpers::edits_url` (a separate, `pub(crate)` method) is what the edit path actually uses. The pub method on `OpenAiImageProvider` is only ever called by tests inside `openai_image.rs` (`test_url_normalization_*`).

- **Decision**: DECIDE.
- **Options**:
  1. CUT: make `edits_url` private, drop its 4 test-only callers.
  2. Leave as-is (public surface, harmless).
- **Risk of option 1**: low.

### sw-generation-09 — `STYLE_PRESETS` const in stability.rs is `pub` but only used inside stability.rs (form 6)

- **Severity**: low
- **Form**: 6
- **Files**: `src/generation/providers/stability.rs:67-85`

```
$ rg -n "stability::STYLE_PRESETS" src/ interfaces/ shared/ --type rust
(no output)
$ rg -n "STYLE_PRESETS" src/ interfaces/ shared/ --type rust
src/generation/providers/stability.rs:67:pub const STYLE_PRESETS: &[&str] = &[…];
src/generation/providers/stability.rs:294:    STYLE_PRESETS.contains(&preset)         (internal)
src/generation/providers/stability.rs:731-735: tests
```

The 17-entry preset array is `pub`, but no external reader enumerates it. A future "what style options does the Stability provider accept?" Settings UI would benefit from this; today none exists.

- **Decision**: DECIDE.
- **Options**: CUT (`#[allow(dead_code)]` or `pub(crate)`), CONNECT (expose to a future Settings panel), leave as-is. Risk: low.

### sw-generation-10 — `google_imagen.rs` `ASPECT_RATIOS` / `IMAGE_SIZES` / `PERSON_GENERATION_OPTIONS` are `pub` but only used inside google_imagen.rs (form 6)

- **Severity**: low
- **Form**: 6
- **Files**: `src/generation/providers/google_imagen.rs:65,68,71`

Same pattern as sw-generation-09: three `pub const` arrays exposing Imagen's API options, used only inside `google_imagen.rs` (lines 198, 223, 576, 582 + tests). No external enumeration today.

- **Decision**: DECIDE. Options: CUT (drop `pub`), CONNECT (expose to future Settings UI), leave as-is. Risk: low.

### sw-generation-11 — midjourney public type+const surface (`ImagineRequest`, `SubmitResponse`, `TaskResponse`, `TaskButton`, `DEFAULT_*`, `PROVIDER_NAME`) is fully internal (form 6)

- **Severity**: low
- **Form**: 6
- **Files**: `src/generation/providers/midjourney/types.rs:71-126,10-25`; `src/generation/providers/midjourney/mod.rs:51`

`midjourney/mod.rs:51` re-exports the request/response types and 5 `pub const` from `types.rs`. **Every consumer is internal to the midjourney subdir**:

```
$ rg -n "midjourney::" src/ interfaces/ shared/ --type rust
src/generation/providers/mod.rs:72:pub use midjourney::{MidjourneyMode, MidjourneyProvider, MidjourneyProviderBuilder};
(only MidjourneyMode is used externally — factory.rs:6)
```

`factory.rs` only consumes `MidjourneyMode` (the variant for `fast`/`relax`); the rest of the surface (`ImagineRequest`, `SubmitResponse`, `TaskResponse`, `TaskButton`, `DEFAULT_COLOR`, `DEFAULT_ENDPOINT`, `DEFAULT_REQUEST_TIMEOUT_SECS`, `MAX_POLL_ATTEMPTS`, `POLL_INTERVAL_SECS`, `PROVIDER_NAME`) is pub but unused outside the module.

- **Decision**: DECIDE.
- **Options**: CUT (`pub(crate)` on types + remove const re-exports), leave as-is. Risk: low.

### sw-generation-12 — google_veo type re-exports (`VeoGenerateResponse`, `VeoGeneratedSample`, `VeoImage`, `VeoInstance`, `VeoOperationError`, `VeoOperationResponse`, `VeoParameters`, `VeoPredictResponse`, `VeoRequest`, `VeoVideo`) are all internal (form 6)

- **Severity**: low
- **Form**: 6
- **Files**: `src/generation/providers/google_veo/mod.rs:53-54`; `src/generation/providers/google_veo/types.rs`

```
$ rg -n "google_veo::" src/ interfaces/ shared/ --type rust
src/generation/providers/mod.rs:71:pub use google_veo::GoogleVeoProvider;
```

The `google_veo/mod.rs` re-exports 10 types from `types.rs` (lines 53-54), but no external consumer references any of them. All hits stay inside `google_veo/{mod,provider,helper,types}.rs`. Same pattern: `is_valid_aspect_ratio` / `is_valid_resolution` / `is_valid_veo2_duration` / `is_valid_veo3_duration` from `helpers.rs` (line 53 mod.rs re-export) and the `*_SECS`, `MAX_POLL_ATTEMPTS`, `*_RANGE` constants are also internal.

- **Decision**: DECIDE. Options: drop `pub use types::{…}` (keep the structs `pub` in `types.rs` so `mod.rs` can use them via `use super::types`), leave as-is. Risk: low.

### sw-generation-13 — replicate constants re-export is internal (form 6)

- **Severity**: low
- **Form**: 6
- **Files**: `src/generation/providers/replicate/mod.rs:50-51`; `src/generation/providers/replicate/constants.rs`

```
$ rg -n "replicate::" src/ interfaces/ shared/ --type rust
src/generation/providers/mod.rs:78:pub use replicate::{ReplicateProvider, ReplicateProviderBuilder};
```

`mod.rs:50-51` re-exports `DEFAULT_ENDPOINT, DEFAULT_TIMEOUT_SECS, MAX_POLL_ATTEMPTS, MODEL_FLUX_SCHNELL, MODEL_MUSICGEN, MODEL_SDXL, POLL_INTERVAL_MS` from `constants.rs`. No external consumer. The constants exist so the openai_compat-style "fan-out to N model versions" can be done at config-load time, but no such consumer is wired today. The factory only takes one `api_key` and configures the provider via `add_model("default", …)` (`factory.rs:228`) — `MODEL_FLUX_SCHNELL` etc. are entirely dead.

- **Decision**: DECIDE. Options: CUT (drop MODEL_* constants entirely, keep just DEFAULT_ENDPOINT/TIMEOUT), CONNECT (let factory seed the model mappings from these constants when `config.model_aliases` is empty), leave as-is. Risk: low.

### sw-generation-14 — openai_tts pub const + public fields (`AVAILABLE_*`, `pub endpoint`, `pub default_voice_id` not on TTS, but on ElevenLabs) (form 6)

- **Severity**: low
- **Form**: 6
- **Files**: `src/generation/providers/openai_tts/mod.rs:63,66,69`; `src/generation/providers/elevenlabs/mod.rs:59,63`

`OpenAiTtsProvider` exposes `pub endpoint: String` (mod.rs:82) and `ElevenLabsProvider` exposes `pub endpoint: String` (mod.rs:59) and `pub default_voice_id: String` (mod.rs:63) on a `pub struct`. The factory uses `Arc<dyn GenerationProvider>` and reaches these via `list_voices()`, `name()`, `supported_types()`, etc. — never reads `provider.endpoint` or `provider.default_voice_id`. The 3 `pub const AVAILABLE_*` arrays (`AVAILABLE_VOICES`, `AVAILABLE_MODELS`, `AVAILABLE_FORMATS`) are also pub but no external reader enumerates them.

- **Decision**: DECIDE.
- **Options**:
  1. Make `endpoint` and `default_voice_id` private (currently `pub` for what reads as "for tests / debugging").
  2. Move `AVAILABLE_*` behind a future Settings UI connector.
  3. Leave as-is.
- **Risk**: low. Tests currently rely on `provider.default_voice_id` (e.g. elevenlabs/tests.rs:15,49,58); making it private requires updating ~3 test asserts.

### sw-generation-15 — Per-provider `AVAILABLE_FORMATS` / type re-export surfaces across 8 providers are all internal (form 6)

- **Severity**: low
- **Form**: 6
- **Files**: `src/generation/providers/azure_speech/{mod.rs,types.rs}` (line 39 re-exports `AzureErrorDetail`, `AzureSpeechError`); `bfl/mod.rs:38` (`classify_status`, `BflError`, `BflGenerateRequest`, `BflPollResponse`, `BflResult`, `BflSubmitResponse`); `cartesia/mod.rs:34` (`CartesiaError`, `CartesiaErrorDetail`, `CartesiaOutputFormat`, `CartesiaTtsRequest`, `CartesiaVoiceSelector`); `deepgram_stt/mod.rs:33` (`DeepgramAlternative`, `DeepgramError`, `DeepgramResponse`, `DeepgramResults`); `deepgram_tts/mod.rs:32` (`DeepgramError as DeepgramTtsError`, `SpeakRequest`); `elevenlabs/mod.rs:46` (`ElevenLabsErrorDetail`, `ElevenLabsErrorResponse`, `TtsRequest`, `VoiceSettings`); `minimax_tts/mod.rs:41` (`MinimaxBaseResp`, `T2aResponse`); `openai_tts/mod.rs:47` (`OpenAiError`, `OpenAiErrorResponse`, `TtsRequest`); `openai_whisper/mod.rs:43` (`OpenAiError`, `OpenAiErrorResponse`, `WhisperResponse`); `suno/mod.rs:36` (`SunoClip`, `SunoError`, `SunoGenerateRequest`); `volcengine_tts/mod.rs:41` (`TtsResponse`)

Each provider's `mod.rs` re-exports its request/response/error types `pub use` so the crate surface exposes them — but **no in-tree consumer references any of them via `<provider>::<Type>`**:

```
$ rg -n "azure_speech::|bfl::|cartesia::|deepgram_stt::|deepgram_tts::|elevenlabs::|minimax_tts::|openai_tts::|openai_whisper::|suno::|volcengine_tts::" src/ interfaces/ shared/ --type rust
src/generation/providers/mod.rs:62,63,64,65,66,67,68,69,71,72,76,77,80,81:  (only the *Provider re-exports at providers/mod.rs)
src/generation/providers/factory.rs:5-9:  (the *Provider use statements for factory dispatch)
src/gateway/voice/local_provider.rs:53:  (the http module, not these providers)
```

The `pub use` exists for **historical** reasons — the README of each provider documents "see types.rs" and rustdoc exposure. None of the request/response/error types are ever read from outside the module that defines them. Same for the `AVAILABLE_FORMATS` const in bfl/cartesia/minimax_tts/volcengine_tts/deepgram_tts/openai_tts and `AVAILABLE_VOICES` in deepgram_tts (all `pub` const arrays).

- **Decision**: DECIDE.
- **Options**: 1. CUT all 11 per-provider `pub use` lines + per-provider type modules; the types stay `pub(crate)` since they're only used inside the module. 2. Leave as-is (rustdoc surface; harmless).
- **Risk of option 1**: low. Tests inside the same module still compile (they use `types::TtsRequest` via `super::types`). External crates that might `use alephcore::generation::providers::bfl::BflError` would break — but no in-tree crate does this.
- **Verification**: `rg -n "azure_speech::|bfl::|cartesia::|deepgram_stt::|deepgram_tts::|elevenlabs::|minimax_tts::|openai_tts::|openai_whisper::|suno::|volcengine_tts::" src/ interfaces/ shared/ --type rust` returns only `factory.rs` (provider use) and `providers/mod.rs` (provider re-export).

### sw-generation-16 — `FalProviderBuilder` struct + `FalProviderBuilder` re-export at `providers/mod.rs` is unused externally (form 6)

- **Severity**: low
- **Form**: 6
- **Files**: `src/generation/providers/fal/mod.rs:73,82-145`; `src/generation/providers/mod.rs:69`

`FalProviderBuilder` is `pub` (fal/mod.rs:73) and re-exported at `providers/mod.rs:69` (`pub use fal::{FalProvider, FalProviderBuilder};`). The factory and all external callers go through `FalProvider::builder(name, &api_key)` (factory.rs:251, returns `FalProviderBuilder`), which is the only way the struct is constructed in production. **No external code references `FalProviderBuilder` directly** (verified via `rg -n "FalProviderBuilder" src/ interfaces/ shared/ --type rust` → 4 hits, all inside `fal/mod.rs` + 1 in `mod.rs:69`).

This is a normal builder pattern (FalProvider owns a constructor that returns the builder; users can compose further). Not actionable in isolation — flagging it because it's adjacent to the larger Fal surface and the re-export at `providers/mod.rs:69` is unused.

- **Decision**: DECIDE. Risk of CUT: low. Risk of leaving as-is: low.

### sw-generation-17 — `FalProviderBuilder::color` setter + `extract_primary_url` / `FalStatusResponse::is_terminal|is_success` / `FalSubmitResponse` are internal (form 6 + 1)

- **Severity**: low
- **Form**: 6 (color setter, helpers) + 1 (helper structs)
- **Files**: `src/generation/providers/fal/mod.rs:112-114,402-407,388-393,345-374`

`FalProviderBuilder::color(c)` (line 112) is wired (factory.rs:265 calls it with `&config.color`), but `FalStatusResponse::is_terminal|is_success` (lines 402-407) are only called by the in-file poll logic. `extract_primary_url` (lines 345-374) is module-private (no `pub`) but is ~30 LOC of pure dispatch logic on a Fal result `Value`. `FalSubmitResponse.status` field is `#[allow(dead_code)]` (line 392).

- **Decision**: DECIDE.
- **Options**: 1. CUT `extract_primary_url` helper if Fal's response shape ever stabilizes to a typed struct. 2. Leave as-is. 3. Remove the `#[allow(dead_code)]` on `FalSubmitResponse.status` if no provider uses it. Risk: low.

### sw-generation-18 — `GenerationProvider::color()` / `default_model()` overrides on every provider duplicate data already on the struct (form 6, low priority)

- **Severity**: low
- **Form**: 6 (every provider has a `color()` impl returning a hard-coded hex or `&self.color`)
- **Files**: every provider module (15 providers × ~5 LOC each = ~75 LOC of trivial overrides)

`color()` is overridden on every provider to return either a hard-coded hex string (`"#10a37f"` for OpenAI, `"#f59e0b"` for Replicate, `"#ff8ad1"` for Fal, …) or `&self.color` (FalProvider, OpenAiCompatProvider where color is user-configurable). `default_model()` similarly returns either a hard-coded model id or `&self.model`. These are trait-required, so they have to exist — flagging only for awareness that this is a uniform pattern of dead-but-required boilerplate across all providers.

- **Decision**: DECIDE (no action — trait contract requires these). Mention only because a `macro_rules!` or `#[derive]` could collapse them in a follow-up if multiple providers are touched at once. Risk: low; effort: ~30 LOC savings.

---

## Aggregated counts: 0 critical, 0 high, 4 medium, 14 low. Decisions: 18× DECIDE, 0× CUT, 0× CONNECT.

The module's wiring is intact: every provider's `provider_type` is dispatched by `factory.rs`, the registry rebuild path is alive, and the per-modality builtin tools (`image_generate`, `speech_generate`, `video_generate`, `audio_generate`) reach the registry through `Arc<RwLock<GenerationProviderRegistry>>`. The severed wires live at the *unused pub surface* layer and in two uninvoked providers (`openai_whisper`, `deepgram_stt`) plus the inert image-edit trait method.

## State of the Union summary

- **Spine is wired.** Factory + registry + per-modality tools + voice catalog all chain. 19 provider subdirs × `provider_type` enum reaches every provider module.
- **Two provider structs (`OpenAiWhisperProvider`, `DeepgramSttProvider`) and one trait method (`edit_image`)** are constructed but not invoked in production. The `DeepgramSttProvider` case is the most actionable — `media::resolve::transcription_service` always picks `WhisperTranscription` regardless of the user's configured `provider_type`, so a user configuring Deepgram never reaches Deepgram (silent misroute; potentially worth a finding-grade up later if confirmed against a real config).
- **Large orphaned `pub` API surface.** Most per-provider request/response/error types and `AVAILABLE_*` const arrays are re-exported `pub` at the provider's `mod.rs` but never read from another module. Cleaning this up would not change behaviour; the test suite still uses them via `super::types`.
- **No findings worth `CUT` in batch 1.** Every candidate has a counter-argument (the test-only symbols are documented API; the inert edit-image path may be intended for a future builtin tool; the uninvoked transcription providers may be the intended target of a wire-up to `media::resolve`). All 18 findings sit at `DECIDE` for the human to weigh the trade-off between dead code and stable contract.

## What I did NOT do

- **Did not run `cargo check`.** Per the protocol — the audit is static; the fixer will compile after applying any changes.
- **Did not re-verify `replicate/mod.rs:413` `assert_send::<ReplicateProviderBuilder>()`** — confirmed that `FalProviderBuilder` is exercised through `FalProvider::builder()`, the same pattern, so the issue is uniform.
- **Did not trace `media::resolve::transcription_service`** into the deep misroute question (whether `deepgram` config actually reaches Whisper today). The rg evidence above strongly suggests yes, but a follow-up audit specifically on `media/` is the right scope — same family as the recent `severed-wire-2026-08-17/media` audit that asked the same question for `whisper` and concluded both layers were separately wired.
- **Did not check `bin/aleph-server/commands/start/builder/subsystems.rs:925` for hot-reload semantics.** The line re-builds the registry from config; behaviour is consistent with `agent_init/generation_init.rs:50,102`.
- **Did not exhaustively list `AVAILABLE_*` constants across every provider** — focused on the 8 most likely candidates; the others (`elevenlabs::OUTPUT_FORMATS`, `cartesia::AVAILABLE_FORMATS`, etc.) follow the same form-6 pattern and can be added to sw-generation-15 if the user wants one finding per const.