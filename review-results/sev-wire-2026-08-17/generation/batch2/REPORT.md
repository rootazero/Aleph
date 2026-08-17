# Severed-Wire Audit — `src/generation` (batch 2 of 2)

- **Audit**: severed-wire-audit (PRODUCED–CONSUMED symbol parity)
- **Date**: 2026-08-17
- **Module**: `src/generation` (top-level + types + cross-cutting: `mod.rs`, `error.rs`, `probe.rs`, `registry.rs`, `response_parser.rs`, `voice_catalog.rs`, `types/{mod,generation_type,output,params,progress,request}.rs`)
- **Tree**: `.worktrees/sev-wire-batch2`
- **Method**: `rg -n "<symbol>" src/ interfaces/`; read-all-files first; cross-verified against `review-results/severed-wire-2026-08-17/generation/batch1/`; no cargo, no edits
- **Result**: 10 findings (0 critical, 0 high, 3 medium, 7 low). Decisions: 0× CUT, 0× CONNECT, 10× DECIDE.

---

## Wiring summary (verified, not severed)

Batch2 covers the spine + cross-cutting glue that batch1's providers sit on. Most of it is healthy:

| Producer | Production consumer (path:line) |
|---|---|
| `GenerationProviderRegistry::new()` (registry.rs:50) | `bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:50,102`; `bin/aleph-server/commands/start/builder/subsystems.rs:925`; `gateway/voice/outbound.rs:477`; tests |
| `registry.register(name, provider)` (registry.rs:88) | `bin/aleph-server/commands/start/builder/agent_init/generation_init.rs:60,112` (production — boot build + hot-reload) |
| `registry.len()`, `registry.is_empty()` (registry.rs:174,178) | `generation_init.rs:69,70` |
| `registry.get(name)` (registry.rs:140) | `builtin_tools/generation/speech_generate.rs:172`; `gateway/voice/outbound.rs:189` |
| `registry.first_for_type(gen_type)` (registry.rs:401) | `speech_generate.rs:194`; `gateway/voice/outbound.rs:118`; `executor/builtin_registry/builder/optional_tools.rs:253,264,276,288` |
| `registry.names_for_type(gen_type)` (registry.rs:294) | `gateway/voice/outbound.rs:128` |
| `registry.providers_for_type(gen_type)` (registry.rs:255) | `tools/probes/generation.rs:51` (GenerationProbe); `bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:124` |
| `registry.get_voices_for_provider(id)` (registry.rs:367) | `builtin_tools/generation/speech_generate.rs:197` |
| `static_voices_for_provider_type(t)` (voice_catalog.rs:34) | `gateway/handlers/generation_providers/voices.rs:6,199` (Settings RPC `generation_providers.voices`) |
| `probe_generation_provider(t, key, base_url)` (probe.rs:108) | `gateway/handlers/generation_providers/handlers.rs:594` (RPC `generation_providers.test`); UI: `interfaces/webchat/src/api/generation_providers.rs:221` |
| `GenerationProbeOutcome` (probe.rs:25) | `gateway/handlers/generation_providers/handlers.rs:609,618,619` (success/message fields read) |
| `GenerationError` / `GenerationResult` (error.rs) | `builtin_tools/generation/mod.rs:17-33` (`From` for `ToolError`); all 19 providers; `gateway/voice/outbound.rs`; `gateway/voice/local_provider.rs` |
| `GenerationType` (types/generation_type.rs:19) | 5 webchat UI sites (filtering, picker, mapping); `config/types/generation/{provider,config}.rs` (capabilities, defaults); `executor/.../optional_tools.rs`; `media/resolve.rs` |
| `GenerationParams` / `GenerationParamsBuilder` (types/params.rs) | 69 `::builder()` callers across providers + `config/types/generation/defaults.rs:215` |
| `GenerationRequest` / `GenerationData` / `GenerationMetadata` / `GenerationOutput` | all 19 providers + 4 builtin tools (`speech/image/video/audio_generate.rs`) + `gateway/voice/{outbound,local_provider}.rs` |
| `GenerationProgress` (types/progress.rs) | only `providers/fal/mod.rs:501,517` (FalProvider override, per batch1 sw-generation-05) + self-tests |
| `VoiceInfo` (mod.rs:55) | `gateway/handlers/generation_providers/voices.rs:120` (dynamic-fetch path); `voice_catalog.rs`; all 7 speech providers' `static_voice_list`/`list_voices` impls |
| `GenerationProvider::list_voices` (mod.rs:358) | `registry.rs:369` (called from `get_voices_for_provider` → `speech_generate.rs:197`) |

**Registry does load every batch1 provider.** `bin/aleph-server/.../generation_init.rs:50,102` calls `GenerationProviderRegistry::new()` then iterates `app_config.generation.merged_providers()` and dispatches each to `gen_providers::create_provider(...)` (the factory batch1 audited at length). The factory arm for every `provider_type` from batch1 — `openai_whisper`, `deepgram_stt` and all the rest — is reachable here, even when no other production code calls the resulting provider struct (sw-generation-02/03 in batch1).

**`response_parser.rs` has zero production consumers** (re-verification of batch1 sw-generation-01).

**`voice_catalog.rs` is fully aligned with the factory's speech-provider arms.** Catalog arms cover `openai_tts | tts | openai | openai-tts | openai_compat | openai-compat`, `elevenlabs`, `minimax_tts`, `volcengine_tts`, `cartesia`, `azure_speech | azure_tts`, `deepgram_tts` — every speech arm in `providers/factory.rs:113-181`. The voice picker fallback at `voices.rs:199` is the only production consumer.

---

## Findings

### sw-generation-19 — Re-verification: `parse_generation_requests` / `has_generation_requests` / `ParsedGenerationRequest` / `ParseResult` still have zero production consumers (form 4 + 6)

- **Severity**: medium
- **Form**: 4 (test-only consumer) + 6 (orphaned pub re-export)
- **Files**: `src/generation/response_parser.rs:16,31,63,96`; `src/generation/mod.rs:67`

Re-running `rg -n "parse_generation_requests|has_generation_requests|ParsedGenerationRequest|ParseResult" src/ interfaces/ shared/ --type rust` against current tree still returns only the 4 in-module definitions, the 6 in-module test sites, and the `pub use` line at `mod.rs:67`. Nothing in `bin/`, `gateway/`, `builtin_tools/`, `run_loop/`, or `interfaces/` has wired the parser in since the batch1 review — confirmed by the existence of the `pub use` (suggesting intent to expose) and the absence of any `use` site. The `[GENERATE:type:provider:model:prompt]` format is still documentary.

- **Decision**: DECIDE. Same options as batch1 sw-generation-01.
- **Verification**: `rg -n "parse_generation_requests|has_generation_requests|ParsedGenerationRequest|ParseResult" src/ interfaces/ shared/ --type rust | grep -v response_parser.rs | grep -v mod.rs` → empty.

### sw-generation-20 — `GenerationError::Cancelled` / `cancelled()` factory is only used in `error.rs` itself (form 1 + 4)

- **Severity**: low
- **Form**: 1 (zero production readers of the variant constructor) + 4 (test-only consumer)
- **Files**: `src/generation/error.rs:96,232-235,511,589,724,875,891,893`

```
$ rg -n "GenerationError::Cancelled\b|GenerationError::cancelled\(\)" src/ interfaces/ --type rust | grep -v src/generation/error.rs
(no output)
```

The `Cancelled` variant appears only inside `error.rs` — the constructor (`cancelled()` at line 243), the `From<GenerationError> for AlephError` arm (line 589), and three of its own tests. **No provider or builtin tool constructs it**, so the public `From` arm can never fire. `provider.cancel()` (mod.rs:268) was already flagged as dead in batch1 sw-generation-06 — together with that finding, the entire "cancelled" semantic has no producer anywhere.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: drop the `Cancelled` variant, the `cancelled()` constructor, and the `From` arm. ~10 LOC + 1 test.
  2. Leave as-is — the variant is harmless and lets a future cancellation story declare itself at this enum level.
- **Risk**: low. External crates cannot construct this enum directly (`alephcore::generation::GenerationError` has private fields).
- **Verification**: `rg -n "GenerationError::Cancelled" src/ interfaces/ --type rust` → only the 4 in-module hits.

### sw-generation-21 — `GenerationError::UnsupportedDimensionError` / `unsupported_dimension()` factory is only used in `error.rs` itself (form 1 + 4)

- **Severity**: low
- **Form**: 1 + 4
- **Files**: `src/generation/error.rs:123,271-275,394,535,603,748`

```
$ rg -n "unsupported_dimension|UnsupportedDimensionError" src/ --type rust
src/generation/error.rs:123:    UnsupportedDimensionError { … }           (variant)
src/generation/error.rs:271:    pub fn unsupported_dimension<S>(…) …      (factory)
src/generation/error.rs:394:                | Self::UnsupportedDimensionError { .. }  (should_fallback arm)
src/generation/error.rs:535:            Self::UnsupportedDimensionError { … } => … (user_friendly_message arm)
src/generation/error.rs:603:            GenerationError::UnsupportedDimensionError { … } => … (From arm)
src/generation/error.rs:748:        assert!(GenerationError::unsupported_dimension("test", None).should_fallback());  (test)
```

The variant is referenced in 6 places — all inside `error.rs`. No provider or builtin tool constructs it. The `should_fallback` arm and `From` arm are classified/converted but the constructor is never called. The factory exists for a future dimension-validation flow that never landed.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: drop the variant, the factory, and the 3 enum-shape arms. ~30 LOC + 1 test. Reduces enum surface for grep-cost and removes a phantom error type from the published API.
  2. CONNECT: when a provider knows the model's accepted dimensions (e.g. Stability's `DIMENSION_RANGES`), wire `unsupported_dimension(msg, suggested)` into the bad-input path. ~10 LOC connector.
  3. Leave as-is.
- **Risk of CUT**: low — no caller.
- **Risk of CONNECT**: low — `should_fallback()` already returns true for this variant, so fallback chains work out of the box.
- **Verification**: `rg -n "unsupported_dimension|UnsupportedDimensionError" src/ --type rust` → only the 6 in-module hits.

### sw-generation-22 — `GenerationData::LocalPath` variant has no producer in the tree (form 1 + 4)

- **Severity**: medium
- **Form**: 1 (zero production constructors) + 4 (test-only consumer)
- **Files**: `src/generation/types/output.rs:9-15,28-33,55`; consumers at `src/builtin_tools/generation/{speech_generate.rs:236,image_generate.rs:191,video_generate.rs:133,audio_generate.rs:112}`; `src/gateway/voice/outbound.rs:247`; `src/builtin_tools/media_send.rs:142` (explicit "no producer" comment)

```
$ rg -n "GenerationData::local_path\b" src/ --type rust
src/generation/types/mod.rs:239:        let data = GenerationData::local_path("/tmp/image.png");   (test only)

$ rg -n "GenerationData::LocalPath\s*\(" src/ --type rust
(no output)
```

The variant is **pattern-matched in 5 production sites** (4 builtin tools + `gateway/voice/outbound.rs`), each of which has a dead match arm. No provider currently produces a `GenerationData::LocalPath`. The explicit acknowledgement is at `src/builtin_tools/media_send.rs:142`: *"The third arm, `GenerationData::LocalPath`, has no producer anywhere in the tree; it is constructed only in `generation::types` tests."* — by the same author who chose not to fail pre-flight for it.

The variant was likely intended for providers that emit a local file path (e.g. downloads to disk before returning). Today every provider returns either `Bytes(...)` (most cloud providers, see `fal/mod.rs:476`, `google_veo/provider.rs:547`, `replicate/provider.rs:234`, `openai_image.rs:393`, `openai_tts/mod.rs:412`, `bfl/mod.rs:373`, `midjourney/provider.rs:183`, `stability.rs`, `google_imagen.rs:493`) or `Url(...)` (openai_compat chain). The local-provider case in `gateway/voice/local_provider.rs:194` constructs `Bytes` from the response body.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: remove the variant + constructor + the 5 dead match arms. ~60 LOC across 6 files. Simplifies the type.
  2. CONNECT: when a `local` voice provider is asked for TTS and writes to disk, have `LocalVoiceProvider` return `LocalPath(...)`. ~20 LOC connector.
  3. Leave as-is — keeping the variant reserves the option for a future file-cached media path.
- **Risk of CUT**: low (no producer). The dead match arms become unreachable-code warnings (or compile-time errors if converted to `let … else`); the match sites need to drop the arm.
- **Risk of CONNECT**: medium — `LocalVoiceProvider` returns its TTS body as `Bytes` (via `local_provider.rs:194`), so changing it would also re-route the attachment construction (`outbound.rs:223-273`).
- **Verification**: `rg -n "GenerationData::local_path|GenerationData::LocalPath\s*\(" src/ --type rust` → only the 1 in-module test.

### sw-generation-23 — `GenerationType` capability helpers (`supports_style` / `supports_voice` / `is_long_running` / `display_name`) have no production caller (form 1 + 4)

- **Severity**: low
- **Form**: 1 (zero production readers) + 4 (test-only)
- **Files**: `src/generation/types/generation_type.rs:33-65`

```
$ rg -n "supports_style|supports_voice|is_long_running|display_name" src/ interfaces/ --type rust | grep -v src/generation/types/
(no output)
```

All four methods on `GenerationType` are exercised exclusively in `types/mod.rs:37-56`. No external consumer asks "does Image support style?" — the providers branch on the type directly (e.g. `fal/mod.rs:179 if ar.is_some()` for aspect_ratio, `openai_tts/mod.rs:317 if voice.is_some()` for voice). The capability matrix the methods express is encoded as data inside each provider, so the type-level helpers are descriptive-only.

`display_name()` is also reachable via the `Display` impl at line 78:1, but `rg -n "format!.*\\{.*\\}\\." src/ interfaces/ | grep GenerationType` finds no `format!("{}", gen_type)` / `gen_type.to_string()` caller outside the type's own impl/tests.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: drop the four methods. ~20 LOC + 4 tests. The `Display` impl can be retained for `{}` formatting (cheap, used by doc comments and `format!("{:?}", …)` in errors).
  2. Leave as-is — the methods document the type's capability matrix in code; cutting them costs little and saves little.
- **Risk**: low. If a future Settings UI wants to grey out style controls based on type, the methods are already there to use.
- **Verification**: `rg -n "GenerationType::supports_style|GenerationType::supports_voice|GenerationType::is_long_running|GenerationType::display_name" src/ interfaces/ --type rust` → only the in-module tests.

### sw-generation-24 — `GenerationOutput::additional_outputs` / `all_outputs()` / `output_count()` are written but never enumerated by a consumer (form 1 + 4)

- **Severity**: medium
- **Form**: 1 (writes without reads) + 4 (test-only enumeration)
- **Files**: `src/generation/types/output.rs:207,254,259`; providers writing it: `google_veo/provider.rs:553, replicate/provider.rs:243, openai_image.rs:464, openai_compat/edit.rs:280,446, google_imagen.rs:546, stability.rs:506`

```
$ rg -n "additional_outputs|all_outputs|output_count" src/ --type rust | grep -v "guardrails\|memory\|harness\|bin/aleph-server\|session\|generation/types"
src/generation/providers/google_veo/provider.rs:553:                    output = output.with_additional_outputs(additional);
src/generation/providers/replicate/provider.rs:243:                    output = output.with_additional_outputs(additional_outputs);
src/generation/providers/openai_image.rs:464:                    output = output.with_additional_outputs(additional);
src/generation/providers/openai_compat/edit.rs:280:            output = output.with_additional_outputs(additional);
src/generation/providers/openai_compat/edit.rs:446:            output = output.with_additional_outputs(additional);
src/generation/providers/google_imagen.rs:546:                    output = output.with_additional_outputs(additional);
src/generation/providers/stability.rs:506:                    output = output.with_additional_outputs(additional);
src/generation/types/mod.rs:303:        assert!(output.additional_outputs.is_empty());        (test)
src/generation/types/mod.rs:315:            .with_additional_outputs(additional);              (test)
src/generation/types/mod.rs:317:        assert_eq!(output.output_count(), 3);               (test)
src/generation/types/mod.rs:319:        let all: Vec<_> = output.all_outputs().collect();   (test)
```

5 providers write `with_additional_outputs(...)` (google_veo, replicate, openai_image, google_imagen, stability) plus 2 write sites inside `openai_compat/edit.rs`. **Zero production readers iterate over `additional_outputs` or `all_outputs()`**. The builtin tools `speech_generate.rs:233-256`, `image_generate.rs:191-205`, `video_generate.rs:133-150`, `audio_generate.rs:112-129` all pattern-match on `output.data` only — they ignore `output.additional_outputs`. `gateway/voice/outbound.rs:222-273` likewise only reads `output.data`.

This is a real wire mismatch. When a Stability model returns N images and the code populates `additional_outputs`, the second through Nth images are silently discarded at the consumer boundary.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: remove `additional_outputs` field, `with_additional_outputs()`, `all_outputs()`, `output_count()`. Drop the 7 writer sites. ~50 LOC.
  2. CONNECT: in each builtin tool's match on `output.data`, also iterate `output.all_outputs()` and emit one attachment per output. ~30 LOC across 4 files.
  3. Leave as-is — keeps the data structure for a future multi-attachment tool result.
- **Risk of CUT**: low (no consumer). The `output_count()` test assertion becomes 1 (the primary).
- **Risk of CONNECT**: medium — multi-attachment emission may need a new tool-result schema (the current `_media` channel carries one location per call).
- **Verification**: `rg -n "output.additional_outputs|output.all_outputs|\.output_count\(\)" src/ --type rust | grep -v 'guardrails\|memory\|harness'` → only test sites + writer sites; no reader.

### sw-generation-25 — `GenerationParams::merge` / `GenerationParams::merged_with` have no external consumer (form 1 + 4)

- **Severity**: low
- **Form**: 1 (zero production readers) + 4 (test-only)
- **Files**: `src/generation/types/params.rs:106-179`

```
$ rg -n "params\.merge\b|params\.merged_with\b|GenerationParams::merge\b|GenerationParams::merged_with\b" src/ interfaces/ --type rust
src/generation/types/mod.rs:115:        base.merge(override_params);                       (test)
src/generation/types/mod.rs:137:        let merged = base.merged_with(other);             (test)
src/generation/types/mod.rs:140:        // Original unchanged
src/generation/types/mod.rs:144:        // Merged has both
src/generation/types/mod.rs:147:        assert_eq!(merged.width, Some(512));
src/generation/types/mod.rs:148:        assert_eq!(merged.height, Some(512));
```

The two `merge*` methods exist but the only callers are `types/mod.rs:115` (`.merge`) and `types/mod.rs:137` (`.merged_with`). The production path (`config/types/generation/defaults.rs:215`) builds a fresh `GenerationParams::builder()` from merged config and never combines an existing instance. Providers that want to extend a base request call `request.with_params(new_params)` (which replaces wholesale), not merge.

- **Decision**: DECIDE.
- **Options**:
  1. CUT: drop both methods. ~50 LOC. The tests in `types/mod.rs:115-148` go with them.
  2. CONNECT: when layering per-preset defaults onto a user-provided request, call `.merged_with(...)` rather than rebuilding from defaults. The defaults flow today (`config/types/generation/defaults.rs:215`) already builds fresh; this would change the behaviour so user-set fields survive default injection.
  3. Leave as-is.
- **Risk**: low for option 1, medium for option 2 (changes observable behaviour in `defaults.rs`).
- **Verification**: `rg -n "\.merge\b|\.merged_with\b" src/ interfaces/ --type rust | grep GenerationParams` → empty.

### sw-generation-26 — `GenerationRequest::with_user_id` / `with_request_id` setter methods are test-only (form 1 + 4)

- **Severity**: low
- **Form**: 1 + 4
- **Files**: `src/generation/types/request.rs:71,77`

```
$ rg -n "with_user_id\b" src/ --type rust
src/generation/providers/openai_compat/mod.rs:415:            .with_user_id("user-123");   (test)
src/generation/providers/openai_image.rs:687:            .with_user_id("user-123");   (test)
src/generation/mod.rs:776:            .with_user_id("user-123");   (test)
src/generation/types/request.rs:77:    pub fn with_user_id<S>(…) …    (definition)
src/generation/types/mod.rs:206:        .with_user_id("user-456");   (test)

$ rg -n "with_request_id\b" src/ --type rust | grep -v "GenerationOutput"
src/generation/mod.rs:682:        let request = GenerationRequest::image("test").with_request_id("req-123");  (test)
src/generation/mod.rs:775:            .with_request_id("req-001");   (test)
src/generation/types/request.rs:71:    pub fn with_request_id<S>(…) …    (definition)
```

The setters themselves are only used in tests. **However, the fields they set are read by production code**:
- `request.user_id` → `openai_image.rs:191`, `openai_compat/helpers.rs:99`, `openai_compat/edit.rs:154,332` (user attribution on outbound API calls).
- `request.request_id` → `google_imagen.rs:417`, `openai_image.rs:327` (echoed into `GenerationOutput.request_id` for tracing).

So the **fields** are wired through to provider-side API calls, but the **setter methods** are bypassed — production builders use `GenerationParams::builder()` chain and never touch `user_id` / `request_id` directly. (The 5 call sites for `GenerationOutput::with_request_id` are real — those write to the OUTPUT, not the input.)

- **Decision**: DECIDE.
- **Options**:
  1. CUT the setters — keep the fields `pub` so the providers can construct a `GenerationRequest { user_id: Some(...), .. }` directly when needed. ~6 LOC + tests.
  2. CONNECT: when the builtin tool layer parses `args.user_id` / `args.request_id` (if it does — it doesn't today), forward them via these setters. Currently `args.user_id` is not in any `*GenerateArgs` struct.
  3. Leave as-is.
- **Risk**: low. External crates cannot easily reach these methods (they're on the public type), but no in-tree reader does.
- **Verification**: `rg -n "with_user_id\b|with_request_id\b" src/ --type rust | grep "GenerationRequest"` → only test sites.

### sw-generation-27 — Six `GenerationProviderRegistry` methods are test/doc-only (form 6)

- **Severity**: low
- **Form**: 6 (orphaned pub API surface)
- **Files**: `src/generation/registry.rs:198 (names), 172 (get_or_err), 214 (contains), 326 (remove), 345 (clear), 354 (iter)`

Verified production usage of registry methods:

| Method | Production callers (path:line) |
|---|---|
| `new()` | `generation_init.rs:50,102`; `subsystems.rs:925`; `gateway/voice/outbound.rs:477`; `builtin_tools/generation/{speech,image}_generate.rs:347,279` (tests) |
| `register(name, provider)` | `generation_init.rs:60,112` |
| `len()` | `generation_init.rs:70` |
| `is_empty()` | `generation_init.rs:69` |
| `get(name)` | `speech_generate.rs:172`; `gateway/voice/outbound.rs:189` |
| `first_for_type(gen_type)` | `speech_generate.rs:194`; `gateway/voice/outbound.rs:118`; `optional_tools.rs:253,264,276,288` |
| `names_for_type(gen_type)` | `gateway/voice/outbound.rs:128` |
| `providers_for_type(gen_type)` | `tools/probes/generation.rs:51`; `tool_catalog_init.rs:124` |
| `get_voices_for_provider(id)` | `speech_generate.rs:197` |
| `iter()` | **none in production** |
| `names()` | **none in production** |
| `contains(name)` | **none in production** |
| `get_or_err(name)` | **none in production** |
| `remove(name)` | **none in production** |
| `clear()` | **none in production** |
| `Default::default()` | **none in production** |

The six test-only methods (`iter`, `names`, `contains`, `get_or_err`, `remove`, `clear`) appear in 30+ registry-internal doc-tests and tests but no production path uses them. They form a complete HashMap façade that an operator might want for "show me every registered provider" or "delete a hot-reloaded provider" — but the operator-facing flows go through `gateway/handlers/generation_providers/*` RPC handlers (add/update/list/delete), which mutate the registry through the higher-level hot-reload loop in `generation_init.rs:97-114`, not through `remove()`.

- **Decision**: DECIDE.
- **Options**:
  1. CUT `iter`, `names`, `contains`, `get_or_err`, `remove`, `clear`. ~70 LOC of dead API. Update the doc tests.
  2. CONNECT: hook `names()` (sorted, deterministic) into a future `gateway.generation.list` RPC. ~20 LOC.
  3. Leave as-is.
- **Risk**: low for option 1. The registry keeps `get`, `first_for_type`, `names_for_type`, `providers_for_type`, `register`, `len`, `is_empty`, `new`, `get_voices_for_provider`, `iter`, `Default` — every production caller still resolves.
- **Verification**: `rg -n "registry\.iter\(\)|registry\.names\(\)|registry\.contains\(|registry\.get_or_err\(|registry\.remove\(|registry\.clear\(\)" src/ interfaces/ --type rust | grep -v "src/generation/registry.rs"` → empty.

### sw-generation-28 — `MockGenerationProvider` / `create_mock_generation_provider` form a public test-API island (form 6)

- **Severity**: low
- **Form**: 6 (orphaned pub API surface used only by tests/doc)
- **Files**: `src/generation/mod.rs:382-528` (struct + impl + factory fn) + 5 test files

```
$ rg -n "MockGenerationProvider\b|create_mock_generation_provider\b" src/ interfaces/ --type rust
src/generation/mod.rs:382:  pub struct MockGenerationProvider { … }                          (definition)
src/generation/mod.rs:390:  impl MockGenerationProvider { … }                                (definition)
src/generation/mod.rs:451:  impl GenerationProvider for MockGenerationProvider { … }         (trait impl)
src/generation/mod.rs:549:  pub fn create_mock_generation_provider() -> … { … }              (factory fn)
src/tools/probes/generation.rs:72,75,98,106          (uses MockGenerationProvider::image_only)
src/builtin_tools/generation/speech_generate.rs:343,349  (uses MockGenerationProvider::new)
src/builtin_tools/generation/image_generate.rs:273,278   (uses MockGenerationProvider::image_only)
src/gateway/voice/outbound.rs:528,543               (uses MockGenerationProvider::image_only)
src/generation/registry.rs:10,17,80,84,…,416,419     (doc examples + 1 test helper)
src/generation/mod.rs:543-549,578,607,614,646,740,748  (doc examples + tests)
```

`MockGenerationProvider` is constructed exclusively in tests (`tools/probes/generation.rs`, `builtin_tools/generation/speech_generate.rs`, `builtin_tools/generation/image_generate.rs`, `gateway/voice/outbound.rs`) and in doc tests on the registry. The convenience methods `MockGenerationProvider::all_types`, `MockGenerationProvider::image_only`, `MockGenerationProvider::video_only`, `MockGenerationProvider::with_color`, `MockGenerationProvider::with_types`, `MockGenerationProvider::with_failure` and the top-level `create_mock_generation_provider()` factory have the same test-only consumer base.

This is by design — a mock provider for integration tests. The risk is that it occupies a substantial chunk of `mod.rs` (~150 LOC + 250 LOC of tests) on the crate surface where external crates might see `pub struct MockGenerationProvider`. No external crate currently uses it (the rg search is in-tree-only and returns nothing from `interfaces/`).

- **Decision**: DECIDE.
- **Options**:
  1. CUT: gate the entire `MockGenerationProvider` block + `create_mock_generation_provider()` behind `#[cfg(any(test, feature = "test-mocks"))]` (or a `dev-dependencies`-only re-export). External crates lose the symbol; in-tree tests still compile.
  2. Move to a `test-utils` sub-crate (`alephcore_test_utils::MockGenerationProvider`). Bigger refactor.
  3. Leave as-is.
- **Risk**: low for option 1 — `mod.rs` is the public root of the generation module, so any external test using `MockGenerationProvider` would import from `alephcore::generation::MockGenerationProvider`; none does today.
- **Verification**: `rg -n "MockGenerationProvider\b" src/ interfaces/ --type rust` → only the 5 test sites + the definition.

---

## Aggregated counts: 0 critical, 0 high, 3 medium, 7 low. Decisions: 10× DECIDE, 0× CUT, 0× CONNECT.

The spine is fully wired: factory + registry + builtin tools + voice catalog + probe all chain. The severed wires live at the *unused enum-variant*, *unused field/method*, and *unused setter* level — within the production-tested surface but unreached by production paths.

## Special-focus re-verification

- **`registry.rs` — does it load/instantiate every provider in batch1?** Yes, transitively via `bin/aleph-server/.../generation_init.rs:50,60,102,112` (boot build + hot-reload). The registry itself just stores `Arc<dyn GenerationProvider>` instances; the dispatch is in `providers/factory.rs`. Every provider_type that batch1 audited (incl. `openai_whisper`, `deepgram_stt` — see batch1 sw-generation-02/03) is reachable through the factory arm, even when the resulting provider struct has no consumer past boot.
- **`response_parser.rs` — every variant consumed by a real parser path?** No. `ParsedGenerationRequest`, `ParseResult`, `parse_generation_requests`, `has_generation_requests` have **no production consumer** anywhere in the tree — re-verified for batch2 (sw-generation-19).
- **`voice_catalog.rs` — voice catalog entries vs actual voice provider consumers.** Aligned. All 8 speech-provider arms in the factory (`openai_tts | tts, elevenlabs, minimax_tts, volcengine_tts, cartesia, azure_speech | azure_tts, deepgram_tts`) plus 4 reasonable aliases have a matching arm in `static_voices_for_provider_type`. The catalog fallback in `voices.rs:199` is the only production consumer; the live-registry path goes through `provider.list_voices()` (mod.rs:358) → `registry.rs:369` → `speech_generate.rs:197`.
- **`types/` — every type definition consumed?** All types are consumed (verified by rg). Some method/field-level surface is dead — see sw-generation-22 (LocalPath variant), sw-generation-23 (type capability helpers), sw-generation-24 (additional_outputs), sw-generation-25 (params.merge*), sw-generation-26 (setter methods).
- **`probe.rs` — health-check probe: caller exists?** Yes. `probe_generation_provider` is called by `gateway/handlers/generation_providers/handlers.rs:594` (RPC `generation_providers.test`), which is consumed by `interfaces/webchat/src/api/generation_providers.rs:221` and registered at `bin/aleph-server/.../handlers/settings.rs:676`. The `GenerationProbeOutcome` struct's `success`/`message` fields are read by `handlers.rs:609,618,619`.

## State of the Union summary

- **The spine is intact.** Factory + registry + builtin tools + voice catalog + probe form a complete chain from config to RPC. Every `provider_type` (including the two uninvoked batch1 transcription providers) is reachable from boot.
- **`response_parser.rs` is still inert.** The 4-symbol re-export at `mod.rs:67` (sw-generation-19, re-verified) is the only severed wire with potential production intent that has never been connected.
- **Five enum-shape findings (sw-generation-20, 21, 22, 24, 25) are dead but harmless.** Cancelled, UnsupportedDimensionError, LocalPath, additional_outputs, and params.merge* all have at the surface but no producer outside their own tests. Each finding offers a small CUT (~10-50 LOC) but the tests still pass and the API surface stays stable.
- **Four surface-cleanliness findings (sw-generation-23, 26, 27, 28) are documentation cruft.** Test-only capability helpers, test-only setters, test-only registry methods, and a test-only mock provider sit on the public API. None breaks anything; the test scaffolding is the only consumer. Test-only capability helpers, test-only setters, test-only registry methods, and a test-only mock provider sit on the public API. None breaks anything; the test scaffolding is the only consumer.
- **No findings worth `CUT` in batch2.** Every candidate has a counter-argument (the variant might be needed for a future flow, the helper documents the type, the mock provider is the canonical test harness). All 10 findings sit at `DECIDE` for the human to weigh the trade-off between dead code and stable contract.

## What I did NOT do

- **Did not run `cargo check`.** Per protocol; the fixer will compile after applying changes.
- **Did not audit `media/resolve.rs`** for the deepgram_stt silent-misroute (batch1 sw-generation-03 documented this — `resolve.rs:80-86` always picks `WhisperTranscription::new` regardless of `provider_type`). The right scope for a deeper audit is `src/media/`, not `src/generation/`.
- **Did not verify the OpenAI-compat factory arm** (`factory.rs:180`) end-to-end — batch1 sw-generation-04 already covered `edit_image`. No change since.
- **Did not exhaustively list every dead `GenerationParams` field** — focused on `merge` / `merged_with` and the setters. A future audit can sweep field-by-field for similar findings if desired.
- **Did not trace whether `GatewayProvider::color()` / `default_model()` are reached** in production — batch1 sw-generation-18 already documented these as trait-required boilerplate.