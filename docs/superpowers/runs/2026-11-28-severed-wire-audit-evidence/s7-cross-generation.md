# S7 — Cross-generation severed-wire audit

Slice: `src/gateway/handlers/generation_providers/**`, `src/gateway/voice/{local_provider.rs,inbound/provider.rs,outbound.rs}`, `src/builtin_tools/generation/**`, `src/gateway/inbound_router/**` (generation + voice paths).

Status: **IN PROGRESS** — findings appended incrementally as confirmed.

## A. Findings

| ID | Form | Severity | Title | Producer | Consumer |
|----|------|----------|-------|----------|----------|
| SW-S7-CROSS-GENERATION-1 | 1 | medium | `CATALOG_GENERATED_AT` voice-catalog audit date has zero consumers | `src/generation/voice_catalog.rs:32` | none |
| SW-S7-CROSS-GENERATION-2 | 3 | high | 12 of 14 `GenerationDefaults` knobs are inert — `to_params()` has no production caller | `src/config/types/generation/defaults.rs:172` | none (test only) |
| SW-S7-CROSS-GENERATION-3 | 3 | medium | `generation.output_dir` is stored and echoed but never read | `src/config/types/generation/config.rs:63` | none |
| SW-S7-CROSS-GENERATION-4 | 3 | medium | `auto_paste_threshold_mb` / `background_task_threshold_seconds` round-trip to the Panel but nothing consumes them | `src/config/types/generation/config.rs:69` | none |
| SW-S7-CROSS-GENERATION-5 | 1 | low | `GenerationConfig::get_default_provider` has only a test caller | `src/config/types/generation/config.rs:139` | none (test only) |

**Severity tally**: high 1 · medium 3 · low 1 · (UNCERTAIN 0)

### SW-S7-CROSS-GENERATION-1 — `CATALOG_GENERATED_AT` voice-catalog audit date has zero consumers
- **Form**: 1 (no consumer)
- **Producer**: `src/generation/voice_catalog.rs:32` — `pub const CATALOG_GENERATED_AT: &str = "2026-08-27";`, documented as *"Surfaced in the Settings RPC payload so the UI can show a 'catalog as of …' hint"*.
- **Consumer**: none found — the `generation_providers.voices` handler returns a bare `Vec<VoiceInfo>` array (`src/gateway/handlers/generation_providers/voices.rs:90-97`), which has no field to carry a date; the Panel decodes it as `Vec<VoiceInfo>` (`interfaces/webchat/src/api/generation_providers.rs:157-163`).
- **Live caller search**: `rg -n --glob '*.rs' 'CATALOG_GENERATED_AT' .` → 1 hit, the definition itself (`src/generation/voice_catalog.rs:32`). Zero references, zero tests.
- **Symptom**: none user-visible today; the doc comment describes a UI affordance ("catalog as of 2026-08-27") that does not exist. The "catalog is stale" signal the constant exists for never reaches a user.
- **Hypothesis**: CUT — the producer was written and its doc claims the wire, but the wire type (`Vec<VoiceInfo>`) cannot carry it, so the constant is dark.
- **Confidence**: high
- **Severity**: medium

### SW-S7-CROSS-GENERATION-2 — 12 of 14 `GenerationDefaults` knobs are inert — `to_params()` has no production caller
- **Form**: 3 (inert config)
- **Producer**: `src/config/types/generation/defaults.rs:172` — `GenerationDefaults::to_params()` is the only code that turns the per-provider defaults into a `GenerationParams` (width/height/aspect_ratio/quality/style/n/format/duration_seconds/fps/voice/speed/language/guidance_scale/steps). The struct is `src/config/types/generation/defaults.rs:27-95`, serialized into `config.toml` (`[generation.providers.X.defaults]`) and into the wire (`shared/protocol/src/providers/generation.rs:133` `GenerationDefaultsJson`).
- **Consumer**: none found in production — `rg -n --glob '*.rs' 'to_params' .` → `src/config/types/generation/defaults.rs:172` (def) and `src/config/types/generation/mod.rs:259` (inside `fn test_defaults_to_params()`, a `#[cfg(test)]` test). No other caller.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'to_params' .` → 12 hits, of which the only Rust-side hits are the definition and one test assertion; the rest are `interfaces/webchat/src/api/projects.rs` (an unrelated local `to_params` helper).
  - `rg -n --glob '*.rs' '\.defaults\.(width|height|aspect_ratio|quality|style|format|duration_seconds|fps|speed|language|guidance_scale|steps|n)\b' .` → 27 hits; the only *production* reads are `src/gateway/voice/local_provider.rs:156` (`defaults.format`, BYO `local` provider only), `src/config/types/voice_local.rs:363,393` (write-only), and `interfaces/webchat/src/platform/wide/views/settings/generation_providers/detail_view.rs:49,106` (Panel write/read of `speed`). Every other hit is a definition site, a validation message, or a test assertion.
  - `rg -n 'config.defaults' src/generation/providers/factory.rs` → 5 hits, all `config.defaults.voice.clone()` (lines 139, 209, 219, 229, 331). `voice` is the ONLY default any provider consumes.
- **Symptom**: an operator sets `[generation.providers.dalle.defaults] width = 2048 / quality = "hd" / n = 2 / steps = 40 / guidance_scale = 7.5` in `config.toml`, or the Panel's provider detail form sets `speed`/`format`, and the values are validated (`GenerationDefaults::validate`, `defaults.rs:108-169`) and stored, but **every** generation request ignores them: `image_generate` builds `GenerationParams { width: args.width, height: args.height, quality: args.quality, style: args.style, ..default }` from the *model's* arguments only (`src/builtin_tools/generation/image_generate.rs:146-151`), and the provider factory never merges the stored defaults into a request. The image comes out at the provider's own default size/quality.
- **Hypothesis**: CUT — `to_params()` is the missing arm between the stored defaults and the provider; it is exercised only by a test that asserts the function itself, never by the request path.
- **Confidence**: high
- **Severity**: high

### SW-S7-CROSS-GENERATION-3 — `generation.output_dir` is stored and echoed but never read
- **Form**: 3 (inert config)
- **Producer**: `src/config/types/generation/config.rs:63` — `pub output_dir: Option<PathBuf>` ("Output directory for generated files. When None, the workspace `ToolContext` fallback is used."), parsed from `config.toml` `[generation] output_dir`, written by `src/gateway/handlers/generation_config.rs:103`, and returned to the Panel at `src/gateway/handlers/generation_config.rs:30` / `shared/protocol/src/providers/generation.rs:99`.
- **Consumer**: none found — the doc says the fallback is `ToolContext`'s, and `ToolContext` computes its own `workspace_path.join("output")` with no reference to the generation config (`src/tools/context.rs:16-32`).
- **Live caller search**: `rg -n --glob '*.rs' 'output_dir' src/` → 194 hits; filtering out `src/tools/context.rs` and `src/builtin_tools/file_ops/*` (both are `ToolContext::output_dir`, a different field on a different type) leaves only `src/config/types/generation/config.rs:63,77,112`, `src/gateway/handlers/generation_config.rs:103,156` and one test assertion (`generation_config.rs:242`). No production read of `generation.output_dir` anywhere. `rg -n 'output_dir' interfaces/` → the Panel only stores/echoes it (`settings/generation_providers/settings_panel.rs:20,55,66,78`).
- **Symptom**: an operator sets `[generation] output_dir = "/data/gen"`; generated media still lands wherever the provider returns it (URL) or wherever the media cache puts it. The value is displayed back in Settings and looks saved.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: medium

### SW-S7-CROSS-GENERATION-4 — `auto_paste_threshold_mb` / `background_task_threshold_seconds` round-trip to the Panel but nothing consumes them
- **Form**: 3 (inert config)
- **Producer**: `src/config/types/generation/config.rs:69` and `:74` — `pub auto_paste_threshold_mb: u32` ("File size threshold (MB) for auto-pasting to clipboard") and `pub background_task_threshold_seconds: u32` ("Duration threshold (seconds) for running generation as background task"); parsed from TOML, echoed to the Panel at `src/gateway/handlers/generation_config.rs:37-38`, and written back from the Panel at `:109-110` (validated at `:76-88`).
- **Consumer**: none found — the only non-definition references are the `generation_config.get/update` handler echoing them.
- **Live caller search**: `rg -n --glob '*.rs' 'auto_paste_threshold_mb' .` → 12 hits: definition (`config.rs:69`), default fn (`config.rs:101`), `Default` impl (`config.rs:118`), the get/update handler (`generation_config.rs:37,76,80,109`), the wire type (`shared/protocol/src/providers/generation.rs:100,357,369,383`), and one test. No production consumer. Same shape for `background_task_threshold_seconds` (13 hits, all in the same files). `rg -n 'auto_paste_threshold_mb|background_task_threshold_seconds' interfaces/` → 8 hits, all in `interfaces/webchat/src/platform/wide/views/settings/generation_providers/settings_panel.rs` (signal set + signal write-back).
- **Symptom**: the Settings page shows "Auto-paste threshold (MB)" and "Background task threshold (s)" inputs; saving them changes `config.toml` and the values come back, but no code path ever compares a media file size or a generation duration against them — large outputs are never auto-pasted and long generations are never backgrounded on account of the setting.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: medium

### SW-S7-CROSS-GENERATION-5 — `GenerationConfig::get_default_provider` has only a test caller
- **Form**: 1 (no consumer)
- **Producer**: `src/config/types/generation/config.rs:139` — `pub fn get_default_provider(&self, gen_type: GenerationType) -> Option<&str>`, the single accessor that maps a `GenerationType` to the matching `default_*_provider` field.
- **Consumer**: none found in production.
- **Live caller search**: `rg -n --glob '*.rs' 'get_default_provider' .` → 6 hits: the definition (`config.rs:139`) and 4 assertions inside `src/config/types/generation/mod.rs:48-64` (`fn test_generation_config_get_default_provider`, a `#[cfg(test)]` test). No production caller.
- **Symptom**: none user-visible — it is a dead accessor. Its *siblings* are live: `default_speech_provider` is read at `src/gateway/voice/outbound.rs:121`, `default_transcription_provider` at `src/gateway/voice/inbound/provider.rs:79` and `src/media/resolve.rs:61`, and the image/video/audio defaults are read via the per-handler `if` ladder at `src/gateway/handlers/generation_providers/handlers.rs:363-378`.
- **Hypothesis**: CUT (or DECIDE — it is a natural candidate for the one place that would resolve the image/video/audio default provider for the tools, whose absence is part of SW-S7-CROSS-GENERATION-2's neighbourhood).
- **Confidence**: high
- **Severity**: low

## 附录：子代理最终摘要里给出、但报告文件未写全的四条 / Appendix: four findings the child listed in its final summary but did not write into the report file

子代理在收尾消息里给出了 9 条发现，而落盘的报告文件只写了 5 条（表格与逐条正文都止于第 5 条）。下面 4 条来自它的收尾消息，逐字保留。The child listed nine findings in its closing message but only five reached the file. The four rows below are verbatim from that message.

| ID | Form | Severity | Title | Producer | Consumer |
|----|------|----------|-------|----------|----------|
| SW-S7-CROSS-GENERATION-6 | 3 | high | `default_image/video/audio_provider` are written by `setDefault` and shown as a badge, but the four generation tools resolve their provider via `registry.first_for_type()` (alphabetically first) — "Set as default" never selects a provider. | src/config/types/generation/config.rs:42 | none |
| SW-S7-CROSS-GENERATION-7 | 5 | high | `[voice.local] tts_format` defaults to "opus"; the local provider then emits `audio/ogg`/`audio/opus`, but `voice.turn.synthesize` accepts only mpeg/mp3/wav → typed `unsupported_output`. | src/config/types/voice_local.rs:47 | src/gateway/handlers/voice_turn.rs:566 |
| SW-S7-CROSS-GENERATION-8 | 5 | medium | legacy `[generation.providers.*]` entries are validated, listed and registered, but both STT resolvers read `transcription_providers` only → "No transcription provider configured". | src/config/types/generation/config.rs:77 | none |
| SW-S7-CROSS-GENERATION-9 | 1 | medium | `resolve_model` is dead; `model_aliases` is honoured only by the `replicate` factory arm. | src/config/types/generation/provider.rs:177 | none (test only) |
